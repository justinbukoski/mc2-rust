//! Headless side-by-side of the P5 visual upgrades: renders a mission view twice (original, then
//! with the enhanced switches on) at half the sheet width each and writes `original | enhanced`
//! as one `--size` PNG (default 2560x1440, indexed on a shared palette to stay small).  Never
//! opens a window.
//!
//! usage: MC2_DATA=<data root> cargo run --release -p mc2-render --example visual_compare --
//!        --out FILE [--mission mc2_01] [--size 2560x1440] [--lighting] [--water] [--time S]
//!        [--target X,Y] [--camera PITCH,YAW,ALTITUDE] [--colors N] [--sun-pitch DEG]

#![allow(clippy::chunks_exact_to_as_chunks)]

use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::gpu::Gpu;
use mc2_render::mission::Mission;
use mc2_render::populate::populate;
use mc2_render::scene::{Frame, SceneRenderer};
use mc2_render::terrain_tex::TerrainTextures;
use mc2_render::visual::VisualOptions;
use std::path::PathBuf;

fn floats(s: &str) -> Vec<f32> {
    s.split(',').map(|v| v.parse().expect("number")).collect()
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> mc2_render::Result<()> {
    let mut out = PathBuf::from("visual_compare.png");
    let mut mission_name = "mc2_01".to_string();
    let (mut w, mut h) = (2560u32, 1440u32);
    let mut on = VisualOptions::default();
    let mut time = 1.7f32;
    let mut target: Option<Vec<f32>> = None;
    let mut camera: Option<Vec<f32>> = None;
    let mut colors = 128usize;
    let mut sun_pitch: Option<f32> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(args.next().expect("--out FILE")),
            "--mission" => mission_name = args.next().expect("--mission NAME"),
            "--size" => {
                let v = args.next().expect("--size WxH");
                let (a, b) = v.split_once('x').expect("WxH");
                (w, h) = (a.parse().expect("width"), b.parse().expect("height"));
            }
            "--lighting" => on.enhanced_lighting = true,
            "--water" => on.enhanced_water = true,
            "--time" => time = args.next().expect("--time S").parse().expect("number"),
            "--target" => target = Some(floats(&args.next().expect("--target X,Y"))),
            "--camera" => camera = Some(floats(&args.next().expect("--camera P,Y,A"))),
            "--colors" => colors = args.next().expect("--colors N").parse().expect("number"),
            "--sun-pitch" => sun_pitch = Some(args.next().expect("DEG").parse().expect("number")),
            other => panic!("unknown argument {other}"),
        }
    }
    let pw = w / 2;
    let cfg = RenderConfig::from_env();
    let gpu = Gpu::headless(&cfg)?;
    println!(
        "adapter {} ({}) msaa x{}",
        gpu.adapter_name, gpu.backend, gpu.samples
    );
    let mut scene = SceneRenderer::new(gpu)?;
    let mut game = GameData::from_env()?;
    let mut mission = Mission::load(&mut game, &mission_name)?;
    if let Some(p) = sun_pitch {
        mission.light = mission.light.at_pitch(p);
    }
    let textures = TerrainTextures::load(&mut game, &mission)?;
    scene.set_terrain(&mission, &textures);
    let populated = populate(&mut game, &mission, &mut scene, 0)?;
    scene.set_terrain_lights(
        &mission,
        &populated.terrain_lights,
        mission.light.night_factor,
    );

    let settings = CameraSettings::from_system_cfg(&mut game.fit("system.cfg")?);
    let mut cam = GameCamera::from_fit(settings, &mut mission.fit, (pw as f32, h as f32));
    if let Some(c) = &camera {
        cam.projection_angle = c[0];
        cam.set_rotation(c[1]);
        cam.altitude = c[2];
    }
    let (mut tx, mut ty) = (cam.target.x, cam.target.y);
    if let Some(t) = &target {
        (tx, ty) = (t[0], t[1]);
    }
    cam.set_position(
        tx,
        ty,
        mission.terrain.world_side(),
        |x, y| mission.terrain.elevation_at(x, y),
        mission.water.elevation,
    );
    cam.viewport = (pw as f32, h as f32);

    let mut render = |opts: VisualOptions| -> mc2_render::Result<Vec<u8>> {
        scene.set_visual(opts);
        scene.render(&Frame {
            width: pw,
            height: h,
            camera: &cam,
            light: &mission.light,
            water: &mission.water,
            terrain_params: &mission.terrain_params,
            time,
            precise_time: None,
            instances: &populated.instances,
            shadows: !mission.light.is_night,
        })
    };
    let before = render(VisualOptions::default())?;
    let after = render(on)?;
    let changed = before
        .chunks_exact(4)
        .zip(after.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    println!("{changed} of {} pixels differ", pw * h);

    // Sheet: `original | enhanced`, each panel the whole `w/2 x h` frame (so the sheet is exactly
    // `w x h`).  A 2560x1440 truecolour PNG of terrain is megabytes, so the sheet is written as an
    // indexed PNG on one shared median-cut palette (`--colors`).
    let mut sheet = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for (k, img) in [&before, &after].into_iter().enumerate() {
            let src = (y * pw * 4) as usize;
            let dst = ((y * w + k as u32 * pw) * 4) as usize;
            sheet[dst..dst + (pw * 4) as usize].copy_from_slice(&img[src..src + (pw * 4) as usize]);
        }
    }
    let (palette, indices) = quantize(&sheet, colors);
    let file = std::io::BufWriter::new(std::fs::File::create(&out)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_palette(palette);
    enc.set_compression(png::Compression::High);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(&indices))
        .map_err(|e| mc2_render::RenderError::Other(e.to_string()))?;
    println!("wrote {} ({w}x{h}, {colors} colours)", out.display());
    Ok(())
}

/// Median-cut quantisation to at most `n` colours: `(RGB palette, one index per pixel)`.
fn quantize(rgba: &[u8], n: usize) -> (Vec<u8>, Vec<u8>) {
    let px: Vec<[u8; 3]> = rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
    // histogram of distinct colours, then split the box with the widest channel range at its
    // pixel-count median
    let mut hist = std::collections::HashMap::<[u8; 3], u32>::new();
    for p in &px {
        *hist.entry(*p).or_default() += 1;
    }
    let mut boxes: Vec<Vec<([u8; 3], u32)>> = vec![hist.into_iter().collect()];
    while boxes.len() < n.clamp(2, 256) {
        let widest = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let range = (0..3)
                    .map(|c| {
                        let (lo, hi) = b.iter().fold((255u8, 0u8), |(lo, hi), (p, _)| {
                            (lo.min(p[c]), hi.max(p[c]))
                        });
                        (hi - lo, c)
                    })
                    .max()
                    .unwrap();
                (
                    range.0 as u64 * b.iter().map(|(_, k)| *k as u64).sum::<u64>(),
                    i,
                    range.1,
                )
            })
            .max();
        let Some((_, i, c)) = widest else { break };
        let mut b = boxes.swap_remove(i);
        b.sort_by_key(|(p, _)| p[c]);
        let half = b.iter().map(|(_, k)| *k as u64).sum::<u64>() / 2;
        let mut acc = 0u64;
        let cut = b
            .iter()
            .position(|(_, k)| {
                acc += *k as u64;
                acc >= half
            })
            .unwrap_or(0)
            .min(b.len() - 2)
            + 1;
        let hi = b.split_off(cut);
        boxes.push(b);
        boxes.push(hi);
    }
    let mut palette = Vec::new();
    let mut lookup = std::collections::HashMap::<[u8; 3], u8>::new();
    for (idx, b) in boxes.iter().enumerate() {
        let total: u64 = b.iter().map(|(_, k)| *k as u64).sum();
        let avg = |c: usize| {
            (b.iter().map(|(p, k)| p[c] as u64 * *k as u64).sum::<u64>() / total.max(1)) as u8
        };
        palette.extend([avg(0), avg(1), avg(2)]);
        for (p, _) in b {
            lookup.insert(*p, idx as u8);
        }
    }
    (palette, px.iter().map(|p| lookup[p]).collect())
}
