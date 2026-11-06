//! Dev helper: builds a screen from any `data/art/<name>.fit` layout and writes an offscreen PNG.
//! Never opens a window.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_fit -- mcl_mm 1024 768 out.png`
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::screen::{LogisticsScreen, Prefixes};
use mc2_ui::{Loader, RenderCtx, UiAssets};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let name = a.get(1).map_or("mcl_mm", String::as_str);
    let w: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let h: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(768);
    let out = a.get(4).map_or("out.png", String::as_str);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let mut assets = UiAssets::default();
    let mut screen = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        LogisticsScreen::load(&mut ld, name, Prefixes::default()).expect("screen")
    };
    let mut dl = DrawList::new(800, 600);
    let mut ctx = RenderCtx {
        dl: &mut dl,
        assets: &assets,
        dt: 0.0,
    };
    screen.render(&mut ctx);
    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("gpu");
    let mut r = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, w, h);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((800, 600), (w, h), ScaleMode::Fit);
    r.draw(&gpu, &mut enc, &target.target(), &layout, &dl, &assets);
    let px = target.read_rgba(&gpu, enc).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(out), &px, w, h).expect("png");
    println!(
        "{} quads, {} batches -> {out}",
        dl.quad_count(),
        dl.batches.len()
    );
}
