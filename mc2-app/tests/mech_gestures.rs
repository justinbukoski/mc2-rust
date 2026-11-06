// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mech gesture animation on the live render path (headless, never a window; skipped without
//! `MC2_DATA` or, with `MC2_SKIP_GPU_TESTS=1`, without a GPU adapter).  `mc2_01`'s player mech
//! walks and its limb node transforms, as `append_instances` hands them to the renderer, change
//! from frame to frame; a standing mech only idles.

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::live_sim::LiveSim;
use mc2_app::sim::SimDriver;
use mc2_render::math::Mat4;
use mc2_render::scene::ShapeInstance;
use mc2_sim::appearance::Appearance;
use mc2_sim::command::{PlayerCommand, ScriptedOrder};
use mc2_sim::ids::PartId;
use std::path::PathBuf;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

fn start() -> Option<HeadlessRun<LiveSim>> {
    let mut opts = HeadlessOptions::new(data()?, "mc2_01");
    opts.size = (640, 480);
    opts.game.cursor = false;
    match HeadlessRun::live_bare(&opts) {
        Ok(r) => Some(r),
        Err(e)
            if e.contains("no suitable GPU adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1") =>
        {
            eprintln!("no GPU adapter; skipping: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// What the renderer is handed for one mover this frame: its node pose and the sim's gesture.
struct Drawn {
    pose: Vec<Mat4>,
    gesture: i32,
    pos: (f32, f32),
    model: usize,
    in_view: bool,
}

fn drawn(run: &mut HeadlessRun<LiveSim>, part: i32) -> Drawn {
    let core = &mut run.core;
    let rt = core.mission.as_mut().unwrap();
    let mut out: Vec<ShapeInstance> = Vec::new();
    rt.driver
        .append_instances(&mut core.scene, &rt.view, 0.0, &mut out);
    let w = &rt.driver.sim().world;
    let h = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| w.objects.get(h).is_some_and(|o| o.c.part_id.0 == part))
        .expect("part exists");
    let o = w.objects.get(h).unwrap();
    let Some(Some(Appearance::Mech(a))) = w.appearance.by_handle.get(h.0 as usize) else {
        panic!("part {part} is not a mech");
    };
    let inst = out
        .iter()
        .filter(|i| i.pose.len() > 8)
        .min_by(|x, y| {
            let d = |i: &ShapeInstance| {
                (i.position.x - o.c.position.x).hypot(i.position.y - o.c.position.y)
            };
            d(x).total_cmp(&d(y))
        })
        .expect("instances");
    Drawn {
        pose: inst.pose.clone(),
        gesture: a.current_gesture,
        pos: (o.c.position.x, o.c.position.y),
        model: inst.model.0,
        in_view: rt.driver.mover_instance(h).is_some_and(|i| i.in_view),
    }
}

fn delta(a: &[Mat4], b: &[Mat4]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| x.0.iter().zip(y.0).map(|(p, q)| (p - q).abs()).sum::<f32>())
        .sum()
}

#[test]
fn walking_mech_limbs_move_and_standing_mech_idles() {
    let Some(mut run) = start() else { return };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    let p = drawn(&mut run, 512);
    let _ = drawn(&mut run, 514);
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .inject_commands(vec![PlayerCommand::Scripted(ScriptedOrder::Move {
            part: PartId(512),
            x: p.pos.0 + 600.0,
            y: p.pos.1 + 300.0,
            run: false,
        })]);
    let mut walk = Vec::new();
    let mut stand = Vec::new();
    for i in 0..120 {
        run.step(None).unwrap();
        if i % 4 == 0 {
            let d = drawn(&mut run, 512);
            walk.push(d);
            stand.push(drawn(&mut run, 514));
        }
    }
    // Only frames the mech is drawn in count: an out-of-view mech keeps its pose (`mech3d.cpp`
    // binds the animation in `setInView`/`updateGeometry`), and a LOD switch changes the model.
    let steady = |v: &[Drawn]| {
        v.windows(2)
            .filter(|w| w[0].in_view && w[1].in_view && w[0].model == w[1].model)
            .map(|w| (delta(&w[0].pose, &w[1].pose), w[1].gesture))
            .collect::<Vec<_>>()
    };
    let walked = steady(&walk);
    assert!(walked.len() >= 10, "the walking mech is drawn: {walked:?}");
    assert!(
        walked.iter().all(|&(d, g)| g == 4 && d > 1.0),
        "every drawn walk frame moves the limbs: {walked:?}"
    );
    let stood = steady(&stand);
    assert!(stood.len() >= 10, "the standing mech is drawn");
    assert!(
        stood.iter().all(|&(d, g)| g == 2 && d == 0.0),
        "a standing mech holds its pose: {stood:?}"
    );
}

/// 6 frames of a walking mech, cropped around it, side by side: `<target tmp>/visual/mech-walk-strip.png`.
/// Game-art pictures are never committed (docs/PUBLISHING.md), so the strip goes under the cargo target dir.
#[test]
fn walking_mech_frame_strip() {
    let Some(mut run) = start() else { return };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    // Put the player's mech under the camera, then walk it across a frozen view.
    let (handle, target) = {
        let rt = run.core.mission.as_mut().unwrap();
        let target = rt.rig.shift;
        let w = &mut rt.driver.sim_mut().world;
        let h = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| w.objects.get(h).is_some_and(|o| o.c.part_id.0 == 512))
            .unwrap();
        let o = w.objects.get_mut(h).unwrap();
        o.c.position.x = target.x - 60.0;
        o.c.position.y = target.y;
        (h, target)
    };
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .inject_commands(vec![PlayerCommand::Scripted(ScriptedOrder::Move {
            part: PartId(512),
            x: target.x + 400.0,
            y: target.y,
            run: false,
        })]);
    let rig = run.core.mission.as_ref().unwrap().rig.clone();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    const TILE: usize = 120;
    let mut tiles: Vec<Vec<u8>> = Vec::new();
    let mut step = 0;
    while tiles.len() < 6 {
        run.core.mission.as_mut().unwrap().rig = rig.clone();
        step += 1;
        if step < 20 || step % 5 != 0 {
            run.step(None).unwrap();
            continue;
        }
        let file = dir.join(format!("walk-{}.png", tiles.len()));
        run.step(Some(&file)).unwrap();
        let (w, h, px) = read_png(&file);
        let b = run
            .core
            .mission
            .as_ref()
            .unwrap()
            .driver
            .mover_screen_box(handle)
            .expect("the walking mech is on screen");
        let (cx, cy) = (((b[0] + b[2]) / 2.0) as i64, ((b[1] + b[3]) / 2.0) as i64);
        let mut tile = vec![0u8; TILE * TILE * 4];
        for y in 0..TILE as i64 {
            for x in 0..TILE as i64 {
                let (sx, sy) = (cx - TILE as i64 / 2 + x, cy - TILE as i64 / 2 + y);
                if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 {
                    let s = (sy as usize * w as usize + sx as usize) * 4;
                    let d = (y as usize * TILE + x as usize) * 4;
                    tile[d..d + 4].copy_from_slice(&px[s..s + 4]);
                }
            }
        }
        tiles.push(tile);
    }
    let mut strip = vec![0u8; TILE * 6 * TILE * 4];
    for (k, t) in tiles.iter().enumerate() {
        for y in 0..TILE {
            let d = (y * TILE * 6 + k * TILE) * 4;
            strip[d..d + TILE * 4].copy_from_slice(&t[y * TILE * 4..(y + 1) * TILE * 4]);
        }
    }
    // the limbs differ between consecutive tiles
    for k in 1..6 {
        let diff = tiles[k - 1]
            .iter()
            .zip(&tiles[k])
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            diff > 200,
            "tile {k} differs from the previous ({diff} bytes)"
        );
    }
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual/mech-walk-strip.png");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(&out).unwrap()),
        (TILE * 6) as u32,
        TILE as u32,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .unwrap()
        .write_image_data(&strip)
        .unwrap();
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}
