//! Button/label text must be visible: a headless render of each FIT-driven window, asserting the
//! label pixels contrast with the button background in every label rect, and writing the render
//! to `$CARGO_TARGET_TMPDIR/visual/` (or `$MC2_VISUAL_DIR`). Needs `MC2_DATA`; a missing GPU fails unless `MC2_SKIP_GPU_TESTS=1`.

use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::ctx::RenderCtx;
use mc2_ui::draw::DrawList;
use mc2_ui::pause_window::PauseWindow;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::{Loader, UiAssets};
use std::io::BufWriter;

const UI: (u32, u32) = (800, 600);

fn data_root() -> Option<std::ffi::OsString> {
    std::env::var_os("MC2_DATA")
}

fn gpu() -> Option<Gpu> {
    let cfg = RenderConfig {
        hires_dir: None,
        ..RenderConfig::default()
    };
    match Gpu::headless(&cfg) {
        Ok(g) => Some(g),
        Err(e)
            if e.to_string().contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
        {
            eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU adapter ({e})");
            None
        }
        Err(e) => panic!("render test needs a GPU adapter: {e}"),
    }
}

/// Draws `dl` 1:1 over a mid-grey field and reads it back.
fn render(gpu: &Gpu, assets: &UiAssets, dl: &DrawList) -> Vec<u8> {
    let layout = UiLayout::new(UI, UI, ScaleMode::Fit);
    let mut renderer = UiRenderer::new(gpu);
    let target = Offscreen::new(gpu, UI.0, UI.1);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    renderer.draw(gpu, &mut enc, &target.target(), &layout, dl, assets);
    target.read_rgba(gpu, enc).expect("readback")
}

pub fn write_png(name: &str, rgba: &[u8]) {
    // Renders are game-derived pictures, so they go under the cargo target tmp dir (CARGO_TARGET_TMPDIR, which
    // follows CARGO_TARGET_DIR and never lands in the checkout) unless MC2_VISUAL_DIR names a place to keep them,
    // e.g. rust/docs/visual for a local review.
    let dir = std::env::var_os("MC2_VISUAL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual"));
    std::fs::create_dir_all(&dir).expect("visual dir");
    let file = std::fs::File::create(dir.join(name)).expect("create png");
    let mut enc = png::Encoder::new(BufWriter::new(file), UI.0, UI.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

fn luma(p: &[u8]) -> i32 {
    (299 * p[0] as i32 + 587 * p[1] as i32 + 114 * p[2] as i32) / 1000
}

/// How many pixels of `rect` differ in luminance from the rect's median by more than 60: label
/// pixels over a flat or lightly shaded button.
fn contrast_pixels(img: &[u8], rect: (i32, i32, i32, i32)) -> usize {
    let (l, t, r, b) = rect;
    let mut lumas = Vec::new();
    for y in t.max(0)..b.min(UI.1 as i32) {
        for x in l.max(0)..r.min(UI.0 as i32) {
            let o = ((y as u32 * UI.0 + x as u32) * 4) as usize;
            lumas.push(luma(&img[o..o + 4]));
        }
    }
    if lumas.is_empty() {
        return 0;
    }
    let mut sorted = lumas.clone();
    sorted.sort_unstable();
    let median = sorted[sorted.len() / 2];
    lumas.iter().filter(|&&v| (v - median).abs() > 60).count()
}

#[test]
fn pause_menu_button_labels_are_visible() {
    let Some(root) = data_root() else { return };
    let Some(gpu) = gpu() else { return };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let mut assets = UiAssets {
        screen: (UI.0 as i32, UI.1 as i32),
        ..UiAssets::default()
    };
    let mut pause = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        PauseWindow::load(&mut ld, UI.0, UI.1).expect("pause window")
    };
    pause.begin(true);
    for _ in 0..90 {
        pause.update((0, 0), false, 1.0 / 30.0);
    }
    let mut dl = DrawList::new(UI.0 as i32, UI.1 as i32);
    pause.render(&mut RenderCtx {
        dl: &mut dl,
        assets: &assets,
        dt: 1.0 / 30.0,
    });
    let img = render(&gpu, &assets, &dl);
    write_png("pause_menu.png", &img);
    for index in 0..7 {
        let r = pause.button_rect(index).expect("button");
        // inset past the button's own border so only the label can contrast
        let n = contrast_pixels(&img, (r.left + 6, r.top + 6, r.right - 6, r.bottom - 6));
        eprintln!("pause button {index}: {n} label pixels");
        assert!(n >= 20, "pause button {index} label is not visible ({n})");
    }
}

/// The FIT-driven screens, each with its `LogisticsScreen`(s).
fn screens(ld: &mut Loader<'_>) -> Vec<(&'static str, Vec<mc2_ui::screen::LogisticsScreen>)> {
    use mc2_ui::logistics::{
        briefing::Briefing, mech_bay::MechBay, mech_display::MechDisplay,
        mission_select::MissionSelect, pilot_ready::PilotReady,
    };
    use mc2_ui::screens::{loading::LoadingScreen, MainMenu};
    let mut out = Vec::new();
    let menu = MainMenu::load(ld).expect("main menu");
    out.push(("main_menu", vec![menu.screen]));
    out.push(("mech_bay", vec![MechBay::load(ld).expect("mech bay").base]));
    out.push(("briefing", vec![Briefing::load(ld).expect("briefing").base]));
    let ms = MissionSelect::load(ld).expect("mission select");
    out.push(("mission_select", vec![ms.base, ms.operation]));
    out.push((
        "pilot_ready",
        vec![PilotReady::load(ld).expect("pilot ready").base],
    ));
    out.push((
        "mech_display",
        vec![MechDisplay::load(ld).expect("mech display").base],
    ));
    out.push((
        "mech_purchase",
        vec![
            mc2_ui::mech_purchase::MechPurchase::load(ld)
                .expect("purchase")
                .screen,
        ],
    ));
    out.push((
        "mech_lab",
        vec![mc2_ui::mech_lab::MechLab::load(ld).expect("lab").screen],
    ));
    out.push((
        "salvage",
        vec![
            mc2_ui::mission_results::SalvageScreen::load(ld)
                .expect("salvage")
                .screen,
        ],
    ));
    out.push((
        "pilot_screen",
        vec![
            mc2_ui::mission_results::PilotScreen::load(ld)
                .expect("pilot screen")
                .screen,
        ],
    ));
    out.push((
        "loading",
        vec![LoadingScreen::load(ld).expect("loading").screen],
    ));
    out
}

#[test]
fn every_screen_button_label_is_visible() {
    let Some(root) = data_root() else { return };
    let Some(gpu) = gpu() else { return };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let mut assets = UiAssets {
        screen: (UI.0 as i32, UI.1 as i32),
        ..UiAssets::default()
    };
    let all = screens(&mut Loader::new(&mut vfs, &mut assets));
    let mut h = mc2_ui::harness::Harness::with_assets(assets);
    let mut bad = Vec::new();
    for (name, mut list) in all {
        for (n, screen) in list.iter_mut().enumerate() {
            screen.begin();
            h.idle(45, |ctx| screen.update(ctx));
            let dl = h.render(|ctx| screen.render(ctx));
            let img = render(&gpu, &h.assets, &dl);
            write_png(&format!("{name}{n}.png"), &img);
            for (i, b) in screen.buttons.iter().enumerate() {
                let d = &b.button.data;
                if d.text_id == 0
                    || b.button.state == mc2_ui::widgets::button::ButtonState::Hidden
                    || !b.button.obj.show
                {
                    continue;
                }
                let r = d.text_rect;
                let px = contrast_pixels(&img, (r.left, r.top, r.right, r.bottom));
                eprintln!("{name}{n} button {i} (text {}): {px}", d.text_id);
                if px < 8 {
                    bad.push(format!("{name}{n} button {i} text {} ({px})", d.text_id));
                }
            }
        }
    }
    assert!(bad.is_empty(), "invisible labels: {bad:#?}");
}
