//! Render the Mechlopedia screen to PNG without opening a window.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_mechlopedia -- mechs 1024 768 out.png`
//! `MC2_DATA=... cargo run -p mc2-ui --example render_mechlopedia -- weapons 1024 768 out.png`

use std::sync::Arc;

use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::harness::Harness;
use mc2_ui::mechlopedia::{Category, Mechlopedia};
use mc2_ui::preview::ModelPreviews;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::{Loader, UiAssets};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let category_name = args.get(1).map_or("mechs", String::as_str);
    let category = match category_name.to_ascii_lowercase().as_str() {
        "mechs" => Category::Mechs,
        "buildings" => Category::Buildings,
        "vehicles" => Category::Vehicles,
        "weapons" => Category::Weapons,
        "personalities" => Category::Personalities,
        "history" => Category::History,
        _ => panic!("unknown category {category_name}; use mechs, buildings, vehicles, weapons, personalities, or history"),
    };
    let width: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let height: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(768);
    let output = args.get(4).map_or("mechlopedia.png", String::as_str);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(&root).expect("VFS");

    // Load game data and logistics
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let logistics = Arc::new(Logistics::new(Arc::clone(&data)).expect("Logistics::new"));

    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);

    let mut mechlopedia = Mechlopedia::load(&mut loader).expect("Mechlopedia::load");

    // Initialize the screen with actual data
    mechlopedia.begin(&assets, &data, &logistics);
    mechlopedia.load_visuals(&mut Loader::new(&mut vfs, &mut assets));
    mechlopedia.select_category(category, &assets);

    // Let the screen settle for a few frames, then resolve the requested model through the same
    // offscreen preview path used by the production UI host.
    let mut harness = Harness::with_assets(assets);
    harness.idle(100, |ctx| mechlopedia.update(ctx));
    let mut previews = ModelPreviews::new(
        Gpu::headless(&RenderConfig::from_env()).expect("preview GPU"),
        mc2_render::game::GameData::open(&root).expect("preview data"),
    )
    .expect("preview renderer");

    // The first draw queues the selected model; the second sees the resolved texture.
    harness.render(|ctx| mechlopedia.render(ctx));
    previews.resolve(&mut harness.assets);
    let dl = harness.render(|ctx| mechlopedia.render(ctx));

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, width, height);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((800, 600), (width, height), ScaleMode::Fit);
    renderer.draw(
        &gpu,
        &mut encoder,
        &target.target(),
        &layout,
        &dl,
        &harness.assets,
    );
    let pixels = target.read_rgba(&gpu, encoder).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(output), &pixels, width, height).expect("PNG");
    println!("{} quads -> {output}", dl.quad_count());
}
