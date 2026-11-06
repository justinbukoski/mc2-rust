// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Weather on the shipped data. `mc2_10` turns the rain on from its `[Weather]` block (500 drops, starting rain 3,
//! chance 100); `mc2_19` has the block with `MaxRainDrops 0`, which the C++ treats as no weather (weather.cpp:160).
//! Set `MC2_DATA` to the data root; without it every test returns early. A missing GPU adapter fails the GPU test
//! unless `MC2_SKIP_GPU_TESTS=1`. `MC2_WEATHER_PNG=<path>` also writes the rain frame there.
#![allow(clippy::chunks_exact_to_as_chunks)]

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::gpu::write_png;
use mc2_render::light::LightModel;
use mc2_render::math::Vec3;
use mc2_render::mission::{Mission, TerrainParams};
use mc2_render::scene::{Frame, SceneRenderer};
use mc2_render::terrain_tex::TerrainTextures;
use mc2_render::weather::{Weather, WeatherConfig, WeatherEvent, THUNDER_SOUNDS};

fn game() -> Option<GameData> {
    match std::env::var_os("MC2_DATA") {
        Some(root) => Some(GameData::open(root).expect("open game data")),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped-data weather test");
            None
        }
    }
}

fn no_ground(_x: f32, _y: f32) -> f32 {
    0.0
}

#[test]
fn mc2_10_turns_the_rain_on_and_mc2_19_has_no_weather() {
    let Some(mut game) = game() else { return };

    let m = Mission::load(&mut game, "mc2_10").unwrap();
    let cfg = m.weather.clone().expect("mc2_10 has a [Weather] block");
    assert_eq!(
        cfg,
        WeatherConfig::new(Some(500), Some(3.0), Some(100), Some(0.04)),
        "the block's values as the shipped file holds them"
    );
    let mut w = Weather::new(cfg, 1);
    assert!(w.is_active());
    w.update(1.0 / 30.0, Vec3::new(0.0, 0.0, 100.0), &no_ground);
    // Starting rain 3 rises by one trend step (0.05 to 0.149), so it is a storm, capped at 4.
    assert!(
        (3.0..=4.0).contains(&w.rain_level()),
        "rain level {}",
        w.rain_level()
    );
    assert_eq!(w.current_drops(), 500, "the storm fills the 500-drop pool");

    let m19 = Mission::load(&mut game, "mc2_19").unwrap();
    let off = m19.weather.expect("mc2_19 has a [Weather] block");
    assert_eq!(off.max_rain_drops, 0);
    assert!(
        !Weather::new(off, 1).is_active(),
        "MaxRainDrops 0 is no weather (weather.cpp:160-171)"
    );
}

/// The rain frame and the frame without it, from the same camera and light. The rain batch is the only
/// difference, so the changed pixels are the streaks.
#[test]
fn rain_streaks_are_white_pixels_in_the_frame_and_the_frame_without_them_is_unchanged() {
    let Some(mut game) = game() else { return };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("weather rain frame", &cfg) else {
        return;
    };
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mut m = Mission::load(&mut game, "mc2_10").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);

    let (w, h) = (640u32, 360u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    let eye = cam.position_world();

    // The storm, after 20 seconds at 30 frames a second: the drops have fallen through the view. A drop spawns
    // 250 to 750 units above the eye, and the camera's near plane (400 units) clips the nearer ones, so only a band
    // of the pool is in view, as in the C++.
    let mut weather = Weather::new(m.weather.clone().unwrap(), 42);
    let ground = |x: f32, y: f32| m.terrain.elevation_at(x, y);
    for _ in 0..600 {
        weather.update(1.0 / 30.0, eye, &ground);
    }
    assert_eq!(weather.current_drops(), 500);
    let light = weather.apply_light(&m.light);
    let terrain = TerrainParams {
        light_scale: weather.terrain_light_scale(),
        lightning: weather.terrain_lightning(),
        ..m.terrain_params.clone()
    };
    let frame = Frame {
        width: w,
        height: h,
        camera: &cam,
        light: &light,
        water: &m.water,
        terrain_params: &terrain,
        time: 0.0,
        precise_time: None,
        instances: &[],
        shadows: false,
    };
    let dry = scene.render_with_fx(&frame, &[]).unwrap();
    let batch = weather
        .rain_batch(&cam, &light)
        .expect("a storm draws streaks");
    let wet = scene
        .render_with_fx(&frame, std::slice::from_ref(&batch))
        .unwrap();

    // Every streak is one quad of the batch: a drop whose ends are both in view makes two triangles. The column
    // spawns above the eye, so only a band of the 500 drops is in view at any moment: tens, not hundreds.
    let streaks = batch.indices.len() / 6;
    assert!(streaks >= 10, "{streaks} streaks in view");

    let mut changed = 0usize;
    for (a, b) in dry.chunks_exact(4).zip(wet.chunks_exact(4)) {
        if a != b {
            changed += 1;
            // White over the scene: each channel gets brighter, never darker.
            for c in 0..3 {
                assert!(b[c] >= a[c], "a streak pixel darkened: {a:?} -> {b:?}");
            }
        }
    }
    // Each streak is a one-pixel line at least a pixel long, so it changes at least one pixel. The drop lengths run
    // to 19 world units and some streaks are shorter than a pixel at this distance, so allow half.
    assert!(
        changed >= streaks / 2,
        "{changed} changed pixels for {streaks} streaks"
    );
    assert!(
        changed < w as usize * h as usize / 2,
        "streaks, not a wash: {changed} pixels"
    );

    if let Some(path) = std::env::var_os("MC2_WEATHER_PNG") {
        write_png(std::path::Path::new(&path), &wet, w, h).unwrap();
    }
}

/// A flash brightens the lit ambient by `(lightening >> 1) / 255` (weather.cpp:370, tgl.cpp:1741), and the
/// frame it lights is brighter than the same frame without it.
#[test]
fn lightning_flash_brightens_the_frame_and_thunder_is_a_table_id() {
    let Some(mut game) = game() else { return };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("weather flash frame", &cfg) else {
        return;
    };
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mut m = Mission::load(&mut game, "mc2_10").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);

    let mut weather = Weather::new(m.weather.clone().unwrap(), 42);
    // Run the storm until a flash has lit the sky and its thunder has been queued.
    let mut thunder = Vec::new();
    let mut flash_frame = None;
    for step in 0..6000 {
        weather.update(0.5, Vec3::new(0.0, 0.0, 100.0), &no_ground);
        for e in weather.take_events() {
            if let WeatherEvent::Thunder(id) = e {
                thunder.push(id);
            }
        }
        if flash_frame.is_none() && weather.lightening_level() > 100.0 {
            flash_frame = Some(step);
        }
        if flash_frame.is_some() && !thunder.is_empty() {
            break;
        }
    }
    assert!(flash_frame.is_some(), "a flash in 3000 s of storm");
    assert!(!thunder.is_empty(), "thunder after the flash");
    assert!(
        thunder.iter().all(|id| THUNDER_SOUNDS.contains(id)),
        "thunder ids are the THUNDER1..4 table entries: {thunder:?}"
    );

    // The flash on the light: the ambient gains (lightening >> 1) / 255 on top of the rain-darkened light.
    let base = m.light.clone();
    let lit = weather.apply_light(&base);
    let flash = ((weather.lightening_level() as u8) >> 1) as f32 / 255.0;
    let rain_only = base.ambient[0] / weather.rain_light_level();
    assert!(
        (lit.ambient[0] - (rain_only + flash).min(1.0)).abs() < 1e-5,
        "ambient {} vs darkened {rain_only} plus flash {flash}",
        lit.ambient[0]
    );

    // Rendered, the flash frame is brighter than the same frame with the flash taken out.
    let (w, h) = (320u32, 180u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    let terrain = TerrainParams {
        light_scale: weather.terrain_light_scale(),
        lightning: weather.terrain_lightning(),
        ..m.terrain_params.clone()
    };
    let no_flash_terrain = TerrainParams {
        lightning: 0.0,
        ..terrain.clone()
    };
    let mut no_flash: LightModel = lit.clone();
    no_flash.ambient = base.ambient.map(|c| c / weather.rain_light_level());
    let render = |light: &LightModel, terrain_params: &TerrainParams| {
        let frame = Frame {
            width: w,
            height: h,
            camera: &cam,
            light,
            water: &m.water,
            terrain_params,
            time: 0.0,
            precise_time: None,
            instances: &[],
            shadows: false,
        };
        scene.render_with_fx(&frame, &[]).unwrap()
    };
    let bright = |img: &[u8]| {
        img.chunks_exact(4)
            .map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]))
            .sum::<u64>()
    };
    let flashing = bright(&render(&lit, &terrain));
    let plain = bright(&render(&no_flash, &no_flash_terrain));
    assert!(
        flashing > plain,
        "the flash lights the frame: {flashing} vs {plain}"
    );
}
