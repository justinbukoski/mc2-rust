//! The dump must use exactly the keys (and pixels) the renderer asks the hook for.
//!
//! Renders nothing: it builds mission mc2_01's scene with `MC2_HIRES_DUMP` behaviour switched on
//! (the originals the renderer requests are written to `<dir>/_dump/<key>.png`) and compares every
//! requested key and its pixels with `mc2-texpack`'s own enumeration.  Skips cleanly without game
//! data or without a GPU adapter.

use mc2_gfx::config::RenderConfig;
use mc2_gfx::texture as tx;
use mc2_render::game::GameData;
use mc2_render::gpu::Gpu;
use mc2_render::mission::Mission;
use mc2_render::populate::populate;
use mc2_render::scene::SceneRenderer;
use mc2_render::terrain_tex::TerrainTextures;
use mc2_texpack::collect::{collect, Options};
use mc2_texpack::stats::pixel_hash;

#[test]
fn every_key_the_renderer_requests_is_in_the_dump() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let dir = std::env::temp_dir().join(format!("mc2_texpack_hook_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg = RenderConfig {
        hires_dir: Some(dir.clone()),
        hires_dump: true,
        ..RenderConfig::default()
    };
    let gpu = match Gpu::headless(&cfg) {
        Ok(g) => g,
        Err(e)
            if e.to_string().contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
        {
            eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU adapter ({e})");
            return;
        }
        Err(e) => panic!("texture-pack hook test needs a GPU adapter: {e}"),
    };
    let mut game = GameData::open(&root).unwrap();
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mission = Mission::load(&mut game, "mc2_01").unwrap();
    let textures = TerrainTextures::load(&mut game, &mission).unwrap();
    scene.set_terrain(&mission, &textures);
    populate(&mut game, &mission, &mut scene, 0).unwrap();

    let dump = collect(
        &mut GameData::open(&root).unwrap(),
        &Options {
            missions: Some(vec!["mc2_01".into()]),
            originals: None,
            verbose: false,
        },
    )
    .manifest;
    let by_key: std::collections::HashMap<_, _> =
        dump.entries.iter().map(|e| (e.key.as_str(), e)).collect();

    let requested = std::fs::read_dir(dir.join("_dump")).unwrap();
    let mut checked = 0;
    for f in requested.filter_map(|e| e.ok()) {
        let file = f.file_name().to_string_lossy().into_owned();
        let key = file.strip_suffix(".png").unwrap();
        // textures the renderer makes up itself (never in the game data)
        if matches!(
            key,
            "n_grey_2x2"
                | "n_no_water_detail_1x1"
                | "n_terrain_light_map_1x1"
                | "n_fx_white_1x1"
                | "n_no_night_lights_1x1"
        ) {
            continue;
        }
        let e = by_key
            .get(key)
            .unwrap_or_else(|| panic!("renderer asked for {key}, which the dump does not list"));
        let img = tx::load_replacement(&dir.join("_dump"), key).unwrap();
        assert_eq!((img.w, img.h), (e.width, e.height), "{key}");
        assert_eq!(pixel_hash(&img.rgba), e.hash, "pixels differ for {key}");
        checked += 1;
    }
    assert!(checked > 100, "only {checked} keys were requested");
    let _ = std::fs::remove_dir_all(&dir);
}
