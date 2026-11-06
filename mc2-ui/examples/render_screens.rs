//! Render the main menu or full options screen to PNG without opening a window.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_screens -- main-menu 1024 768 out.png`
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::harness::Harness;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::screen::{LogisticsScreen, Prefixes};
use mc2_ui::screens::{MainMenu, OptionsEnv, OptionsScreen, Prefs};
use mc2_ui::{Loader, RenderCtx, UiAssets};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let screen_name = args.get(1).map_or("main-menu", String::as_str);
    let width: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let height: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(768);
    let output = args.get(4).map_or("screen.png", String::as_str);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("VFS");
    let mut assets = UiAssets::default();
    enum Screen {
        Options(Box<OptionsScreen>),
        MainMenu(Box<MainMenu>),
        Fit(Box<LogisticsScreen>),
    }
    let mut screen = {
        let mut loader = Loader::new(&mut vfs, &mut assets);
        match screen_name {
            "main-menu" => {
                let mut menu = MainMenu::load(&mut loader).expect("main menu");
                menu.set_draw_background(true);
                menu.skip_intro();
                Screen::MainMenu(Box::new(menu))
            }
            "options" => {
                let prefs = Prefs::default();
                Screen::Options(Box::new(
                    OptionsScreen::load(&mut loader, prefs.clone(), prefs, &OptionsEnv::default())
                        .expect("options"),
                ))
            }
            fit => Screen::Fit(Box::new(
                LogisticsScreen::load(&mut loader, fit, Prefixes::default()).expect("FIT screen"),
            )),
        }
    };
    if let Screen::MainMenu(menu) = &mut screen {
        let mut harness = Harness::new();
        harness.idle(300, |ctx| menu.update(ctx));
    }
    let mut dl = DrawList::new(800, 600);
    let mut ctx = RenderCtx {
        dl: &mut dl,
        assets: &assets,
        dt: 0.0,
    };
    match &mut screen {
        Screen::Options(screen) => screen.render(&mut ctx),
        Screen::MainMenu(screen) => screen.render(&mut ctx),
        Screen::Fit(screen) => screen.render(&mut ctx),
    }
    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, width, height);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((800, 600), (width, height), ScaleMode::Fit);
    renderer.draw(&gpu, &mut encoder, &target.target(), &layout, &dl, &assets);
    let pixels = target.read_rgba(&gpu, encoder).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(output), &pixels, width, height).expect("PNG");
    println!("{} quads -> {output}", dl.quad_count());
}
