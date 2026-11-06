// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Render the mission hot-key help overlay (`MissionInterfaceManager::drawHotKeys`, which draws
//! `KeyboardRef`, `code/keyboardref.cpp`) to a PNG without opening a window.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_hotkeys -- 1024 768 out.png`
//!
//! The layout is `mcui_keyref_<res>.fit` with the rows of `mcui_keyref_entry.fit` in its list box,
//! as `KeyboardRef::init` and `KeyboardRef::reseed` build it. The rows come from the default
//! [`HotKeys`] table in both control styles (they are the same).
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::mission_keys::{help_rows, ControlStyle, HotKeys};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::screen::{LogisticsScreen, Prefixes};
use mc2_ui::strings::StringTable;
use mc2_ui::widgets::listbox::{ListBox, TextListItem};
use mc2_ui::{Loader, RenderCtx, UiAssets};

/// `IDS_KEYBOARD_REF_FONT` (`test_scripts/res_conv/resource.h:1234`).
const IDS_KEYBOARD_REF_FONT: u32 = 28134;
/// `KeyboardRef::init` picks the first of these at least as wide as the screen
/// (`keyboardref.cpp:34-60`).
const RESOLUTIONS: [(u32, &str); 5] = [
    (640, "_640"),
    (1024, "_1024"),
    (1280, "_1280"),
    (1600, "_1600"),
    (1920, "_1920"),
];

fn resolution_suffix(width: u32) -> &'static str {
    RESOLUTIONS
        .iter()
        .find(|(w, _)| *w == width)
        .or_else(|| RESOLUTIONS.iter().find(|(w, _)| *w >= width))
        .map_or("_1920", |(_, s)| s)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let width: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let height: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(768);
    let output = args.get(3).map_or("hotkeys.png", String::as_str);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("VFS");
    let mut assets = UiAssets::default();

    let hk = HotKeys::new(ControlStyle::Aoe);
    let rows = help_rows(&hk, StringTable::builtin());

    let (screen, mut list, key_font, mut entry) = {
        let mut loader = Loader::new(&mut vfs, &mut assets);
        let name = format!("mcui_keyref{}", resolution_suffix(width));
        let screen =
            LogisticsScreen::load(&mut loader, &name, Prefixes::default()).expect("keyref FIT");
        let key_font = loader.font(IDS_KEYBOARD_REF_FONT);
        let entry = loader.art_fit("mcui_keyref_entry").expect("entry FIT");
        let list_rect = screen.rect(2).expect("list rect").obj.clone();
        let list = ListBox::new(
            &mut loader,
            list_rect.left(),
            list_rect.top(),
            list_rect.width(),
            list_rect.height(),
        );
        (screen, list, key_font, entry)
    };
    let mut key_template = TextListItem::new(&assets, key_font);
    key_template.init_fit(&mut entry, "Text0");
    let mut desc_template = TextListItem::new(&assets, key_font);
    desc_template.init_fit(&mut entry, "Text1");
    // `KeyboardRef::reseed`: a key item, then a description item on the same row.
    let desc_x = desc_template.obj.left() + list.obj.left();
    for row in &rows {
        let mut key = key_template.clone();
        key.set_text(&row.key);
        key.alignment = 1;
        let idx = list.add_item(Box::new(key)).expect("row fits");
        let row_y = list.item(idx).expect("item").obj().global_y() as f32;
        let mut desc = desc_template.clone();
        desc.set_text(&row.description);
        desc.alignment = 0;
        list.add_item(Box::new(desc));
        list.item_mut(idx + 1)
            .expect("description item")
            .move_to(desc_x, row_y);
    }

    let mut dl = DrawList::new(width as i32, height as i32);
    {
        let mut ctx = RenderCtx {
            dl: &mut dl,
            assets: &assets,
            dt: 0.0,
        };
        list.render(&mut ctx);
    }
    let mut screen = screen;
    {
        let mut ctx = RenderCtx {
            dl: &mut dl,
            assets: &assets,
            dt: 0.0,
        };
        screen.render(&mut ctx);
    }

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, width, height);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((width, height), (width, height), ScaleMode::Fit);
    renderer.draw(&gpu, &mut encoder, &target.target(), &layout, &dl, &assets);
    let pixels = target.read_rgba(&gpu, encoder).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(output), &pixels, width, height).expect("PNG");
    println!("{} rows, {} quads -> {output}", rows.len(), dl.quad_count());
}
