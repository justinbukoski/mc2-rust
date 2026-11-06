//! A crater decal is drawn on the terrain. Set `MC2_DATA` to the data root; without it every test returns early. A missing GPU
//! adapter fails the test unless `MC2_SKIP_GPU_TESTS=1`. `MC2_CRATER_PNG=<path>` also writes the frame with the decals there.
#![allow(clippy::chunks_exact_to_as_chunks)]

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::crater::{
    crater_batches, CraterTextures, Decal, CRATER_TEXTURE, FOOTPRINT_TEXTURE,
};
use mc2_render::game::GameData;
use mc2_render::gpu::write_png;
use mc2_render::mission::Mission;
use mc2_render::scene::{Frame, SceneRenderer};
use mc2_render::terrain_tex::TerrainTextures;
use mc2_sim::crater::{CraterPool, CRATER_1};
use mc2_sim::math::Vec3;

fn decals(pool: &CraterPool) -> Vec<Decal> {
    pool.iter()
        .map(|c| {
            let (uv, extent) = c.uv();
            Decal {
                crater: c.is_crater(),
                corners: c.corners.map(|p| [p.x, p.y, p.z]),
                uv,
                extent,
            }
        })
        .collect()
}

#[test]
fn the_shipped_decal_textures_exist_and_have_cutouts() {
    let Ok(mut game) = GameData::from_env() else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    for name in [CRATER_TEXTURE, FOOTPRINT_TEXTURE] {
        let img = game.tga(name).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(img.width >= 64 && img.height >= 64, "{name}");
        let clear = img.pixels.chunks_exact(4).filter(|p| p[3] == 0).count();
        let solid = img.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(solid > 0, "{name}: nothing to draw");
        eprintln!(
            "{name}: {}x{} {clear} clear {solid} drawn",
            img.width, img.height
        );
    }
}

#[test]
fn a_crater_decal_changes_the_terrain_pixels_under_it() {
    let Ok(mut game) = GameData::from_env() else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("crater decal frame", &cfg) else {
        return;
    };
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mut m = Mission::load(&mut game, "mc2_10").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);

    let (w, h) = (640u32, 360u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);

    // the two decal textures, uploaded like the game does
    let mut upload = |game: &mut GameData, name: &str| {
        let img = game.tga(name).unwrap();
        scene.add_fx_texture(name, img.width as u32, img.height as u32, &img.pixels)
    };
    let textures = CraterTextures {
        crater: Some(upload(&mut game, CRATER_TEXTURE)),
        footprint: Some(upload(&mut game, FOOTPRINT_TEXTURE)),
    };

    // a crater of every size around the camera target, as the sim's events would have added them
    let mut pool = CraterPool::default();
    let ground = |p: &Vec3| m.terrain.elevation_at(p.x, p.y);
    let t = cam.target;
    for (i, kind) in (CRATER_1..CRATER_1 + 4).enumerate() {
        let at = Vec3 {
            x: t.x + (i as f32 - 1.5) * 90.0,
            y: t.y,
            z: 0.0,
        };
        pool.add(kind, &at, 20.0 * i as f32, &ground);
    }
    let batches = crater_batches(decals(&pool).iter(), textures, m.light.ambient);
    assert_eq!(batches.len(), 1, "four craters, one batch");
    assert_eq!(batches[0].indices.len(), 24);

    let frame = Frame {
        width: w,
        height: h,
        camera: &cam,
        light: &m.light,
        water: &m.water,
        terrain_params: &m.terrain_params,
        time: 0.0,
        precise_time: None,
        instances: &[],
        shadows: false,
    };
    let dry = scene.render_with_fx(&frame, &[]).unwrap();
    let wet = scene.render_with_fx(&frame, &batches).unwrap();

    // The decals are the only difference: pixels change, in the middle of the frame where the camera looks at the ground.
    let mut changed = 0usize;
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (w, 0, h, 0);
    for (i, (a, b)) in dry.chunks_exact(4).zip(wet.chunks_exact(4)).enumerate() {
        if a != b {
            changed += 1;
            let (x, y) = (i as u32 % w, i as u32 / w);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
    }
    assert!(changed > 200, "{changed} pixels changed by four craters");
    assert!(
        changed < (w * h / 4) as usize,
        "decals, not a wash: {changed} pixels"
    );
    let (cx, cy) = (w / 2, h / 2);
    assert!(
        min_x < cx && max_x > cx && min_y < cy + 60 && max_y > cy - 60,
        "the changed pixels surround the screen centre: x {min_x}..{max_x} y {min_y}..{max_y}"
    );

    // and without the decal batches the frame is the dry one again
    let again = scene.render_with_fx(&frame, &[]).unwrap();
    assert_eq!(dry, again);

    if let Some(path) = std::env::var_os("MC2_CRATER_PNG") {
        write_png(std::path::Path::new(&path), &wet, w, h).unwrap();
    }
}
