// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Queued waypoint markers and red path lines (`MechWarrior::drawWaypointPath`,
//! `warrior.cpp:7407-7473`; shown while the waypoint key is held, `missiongui.cpp:2977-2990`).
//! Headless production path (`GameCore` + `LiveSim` + shipped data); the orders go through the
//! real mouse/keyboard path. Skips without `MC2_DATA`; a missing GPU is skipped only when
//! `MC2_SKIP_GPU_TESTS=1`.
//!
//! Tolerance: expected pixel positions come from `GameCamera::project_z` of the queued world
//! points (the declared simulation input); a marker or line is accepted within 2 px of them,
//! because the C++ rasterises its 1-pixel lines and the marker shape's footprint is not asserted
//! beyond "covers the projected point".

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::hud::HudRuntime;
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_render::camera::GameCamera;
use mc2_render::math::Vec3;
use mc2_ui::mission_hud::HitTarget;
use std::path::PathBuf;

fn start() -> Option<HeadlessRun<LiveSim>> {
    let data = PathBuf::from(std::env::var_os("MC2_DATA").or_else(|| {
        eprintln!("MC2_DATA not set; skipping");
        None
    })?);
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e)
            if e.contains("no suitable GPU adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1") =>
        {
            eprintln!("no GPU adapter; MC2_SKIP_GPU_TESTS=1, skipping waypoint_markers: {e}");
            return None;
        }
        Err(e) => panic!("waypoint_markers could not start the production path: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    while run.core.mission.as_ref().unwrap().clock.frame() < 130 {
        run.step(None).unwrap();
    }
    // Isolate the world view from the opening tutorial movie (as hud_brackets does).
    let m = run.core.mission.as_mut().unwrap();
    if m.driver.sim().world.presentation.movie_mode {
        m.driver.sim_mut().world.presentation = Default::default();
        run.step(None).unwrap();
    }
    Some(run)
}

fn hud(run: &HeadlessRun<LiveSim>) -> &HudRuntime {
    run.core.mission.as_ref().unwrap().hud.as_ref().unwrap()
}

fn camera(run: &HeadlessRun<LiveSim>) -> GameCamera {
    run.core.mission.as_ref().unwrap().rig.view()
}

fn key(run: &mut HeadlessRun<LiveSim>, name: &str, pressed: bool) {
    let code = script::key_code(name).expect("key name");
    for e in script::key_events(code, pressed) {
        run.event(e);
    }
    run.step(None).unwrap();
}

fn right_click(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    run.event(InputEvent::CursorMoved { x, y });
    run.step(None).unwrap();
    for pressed in [true, false] {
        run.event(InputEvent::Button {
            button: MouseButton::Right,
            pressed,
        });
        run.step(None).unwrap();
    }
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn px(rgba: &[u8], w: u32, x: i32, y: i32) -> [u8; 3] {
    let i = ((y as u32 * w + x as u32) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

fn is_line_red(p: [u8; 3]) -> bool {
    p[0] > 200 && p[1] < 60 && p[2] < 60
}

fn tmp_png(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("waypoint-markers");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Three terrain points where the waypoint key shows the walk-waypoint cursor (passable, in
/// line of sight): the production cursor logic decides, not the test.
fn open_terrain_points(run: &mut HeadlessRun<LiveSim>) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = Vec::new();
    for y in (250..800).step_by(30) {
        for x in (800..1500).step_by(30) {
            if out.len() == 3 {
                return out;
            }
            if out
                .iter()
                .any(|&(ox, oy)| (ox - x).abs() < 150 && (oy - y).abs() < 100)
            {
                continue;
            }
            if hud(run).hit_test((x, y)) != HitTarget::World {
                continue;
            }
            run.event(InputEvent::CursorMoved {
                x: f64::from(x),
                y: f64::from(y),
            });
            run.step(None).unwrap();
            if hud(run).order_cursor() == mc2_ui::mission_keys::state::WALKWAYPT_LOS {
                out.push((x, y));
            }
        }
    }
    panic!("three walk-waypoint terrain points, found {out:?}");
}

#[test]
fn queued_waypoints_draw_markers_and_red_lines_at_their_projected_positions() {
    let Some(mut run) = start() else { return };
    // Select the lance (E), then hold the waypoint key (Ctrl) for the rest of the test.
    key(&mut run, "E", true);
    key(&mut run, "E", false);
    assert!(
        !hud(&run).selection.selected.is_empty(),
        "E selects the lance"
    );
    key(&mut run, "Ctrl", true);

    // Nothing queued yet: the key alone draws no markers or lines at the future positions.
    let points = open_terrain_points(&mut run);
    let before = tmp_png("before.png");
    run.step(Some(&before)).unwrap();
    let (w, _, before_rgba) = read_png(&before);

    for &(x, y) in &points {
        right_click(&mut run, f64::from(x), f64::from(y));
    }

    // The production path queued three moves for every selected unit.
    let selected = hud(&run).selection.selected.clone();
    let world = &run.core.mission.as_ref().unwrap().driver.sim().world;
    let queued: Vec<Vec3> = {
        let h = world
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                world
                    .objects
                    .get(h)
                    .is_some_and(|o| o.c.part_id.0 == selected[0])
            })
            .unwrap();
        let slot = mc2_sim::ai::warrior::slot_of(world, h).expect("pilot");
        let q = &world.pilots.get(slot).unwrap().queue;
        assert_eq!(q.len(), 3, "three queued waypoints");
        q.iter()
            .map(|o| {
                let z = world.terrain.elevation(&o.point);
                Vec3::new(o.point.x, o.point.y, z)
            })
            .collect()
    };
    let start_pos = {
        let o = world
            .objects
            .movers
            .iter()
            .filter_map(|&h| world.objects.get(h))
            .find(|o| o.c.part_id.0 == selected[0])
            .unwrap();
        Vec3::new(o.c.position.x, o.c.position.y, o.c.position.z)
    };

    let after = tmp_png("after.png");
    run.step(Some(&after)).unwrap();
    let (aw, ah, rgba) = read_png(&after);
    assert_eq!((aw, ah), (1920, 1080));
    let cam = camera(&run);
    let k = 1080.0 / cam.viewport.1.max(1.0);
    let proj = |p: Vec3| {
        let (s, _) = cam.project_z(p);
        (s[0] * k, s[1] * k)
    };

    // Markers: the shape changes pixels around each projected waypoint, and the changed pixels
    // are not line red.
    for (i, p) in queued.iter().enumerate() {
        let (sx, sy) = proj(*p);
        assert!(
            sx > 30.0 && sy > 30.0 && sx < 1890.0 && sy < 1000.0,
            "waypoint {i} on screen"
        );
        let mut marker = 0;
        for y in (sy as i32 - 14)..=(sy as i32 + 14) {
            for x in (sx as i32 - 14)..=(sx as i32 + 14) {
                let (a, b) = (px(&rgba, w, x, y), px(&before_rgba, w, x, y));
                if a != b && !is_line_red(a) {
                    marker += 1;
                }
            }
        }
        assert!(
            marker >= 20,
            "marker pixels around waypoint {i} at ({sx:.0},{sy:.0}): {marker}"
        );
    }

    // Lines: the mover -> wp1, wp1 -> wp2, wp2 -> wp3 chain (no current order, so it starts at the unit).
    let chain = [start_pos, queued[0], queued[1], queued[2]];
    for pair in chain.windows(2) {
        let (a, b) = (proj(pair[0]), proj(pair[1]));
        let mut hits = 0;
        for t in [0.25f32, 0.4, 0.5, 0.6, 0.75] {
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let found = (-2..=2).any(|dy| {
                (-2..=2).any(|dx| is_line_red(px(&rgba, w, x as i32 + dx, y as i32 + dy)))
            });
            hits += usize::from(found);
        }
        assert!(
            hits >= 4,
            "red line from ({:.0},{:.0}) to ({:.0},{:.0}): {hits}/5 samples",
            a.0,
            a.1,
            b.0,
            b.1
        );
    }

    // Movie mode hides the GUI (`drawGUIOn`, missiongui.cpp:2902), and with it the markers and lines;
    // they return when it ends while the key is still held.
    let red_near = |run: &mut HeadlessRun<LiveSim>, name: &str| {
        let f = tmp_png(name);
        run.step(Some(&f)).unwrap();
        let (_, _, img) = read_png(&f);
        let (sx, sy) = proj(queued[2]);
        let mut red = 0;
        for y in (sy as i32 - 14)..=(sy as i32 + 14) {
            for x in (sx as i32 - 14)..=(sx as i32 + 14) {
                red += usize::from(is_line_red(px(&img, w, x, y)));
            }
        }
        red
    };
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .sim_mut()
        .world
        .presentation
        .movie_mode = true;
    assert_eq!(red_near(&mut run, "movie.png"), 0, "movie mode hides lines");
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .sim_mut()
        .world
        .presentation
        .movie_mode = false;
    // `gui_frame` runs before the presentation refresh, so the GUI gate follows the sim by one frame.
    run.step(None).unwrap();
    assert!(red_near(&mut run, "movie_end.png") > 0, "lines return");

    // Releasing the key hides them again (nothing but the scene is left at the waypoints).
    key(&mut run, "Ctrl", false);
    let released = tmp_png("released.png");
    run.step(Some(&released)).unwrap();
    let (_, _, rel) = read_png(&released);
    let (sx, sy) = proj(queued[2]);
    let mut red = 0;
    for y in (sy as i32 - 14)..=(sy as i32 + 14) {
        for x in (sx as i32 - 14)..=(sx as i32 + 14) {
            red += usize::from(is_line_red(px(&rel, w, x, y)));
        }
    }
    assert_eq!(
        red, 0,
        "no red path line near the last waypoint without the key"
    );

    // Evidence image: the world view around the three waypoints with the key held.
    let docs = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual");
    std::fs::create_dir_all(&docs).unwrap();
    let xs: Vec<f32> = chain.iter().map(|p| proj(*p).0).collect();
    let ys: Vec<f32> = chain.iter().map(|p| proj(*p).1).collect();
    let min = |v: &[f32]| v.iter().cloned().fold(f32::MAX, f32::min);
    let max = |v: &[f32]| v.iter().cloned().fold(f32::MIN, f32::max);
    let (l, t) = (
        (min(&xs) - 80.0).max(0.0) as u32,
        (min(&ys) - 80.0).max(0.0) as u32,
    );
    let (r, b) = (
        (max(&xs) + 80.0).min(1919.0) as u32,
        (max(&ys) + 80.0).min(1079.0) as u32,
    );
    let (cw, ch) = (r - l + 1, b - t + 1);
    let mut crop = Vec::new();
    for y in t..=b {
        let row = ((y * w + l) * 4) as usize;
        crop.extend_from_slice(&rgba[row..row + cw as usize * 4]);
    }
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(docs.join("waypoint-markers.png")).unwrap()),
        cw,
        ch,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(&crop).unwrap();
    wr.finish().unwrap();
}
