// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Render the in-mission info window and the chat window (lane U3) to PNG at 1024x768, with
//! synthetic data. Headless: no window is opened.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_hud_info -- <out_dir>`
//! writes `info-1024x768.png`, `info-top-1024x768.png` and `chat-1024x768.png` into `<out_dir>`.

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::DrawList;
use mc2_ui::hud_info::chat::{ChatFonts, ChatFrame, ChatGeometry, ChatMessage, ChatWindow};
use mc2_ui::hud_info::draw_ops;
use mc2_ui::hud_info::info::{
    InfoFonts, InfoGeometry, InfoMouse, InfoWindow, PilotInfo, UnitInfo, WeaponInfo,
    NUM_SPECIALTY_SKILLS,
};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::{Loader, RenderCtx, UiAssets};
use std::path::Path;

const W: u32 = 1024;
const H: u32 = 768;

/// A synthetic home-team mech: two PPCs (one out), an LRM/15 with 16 shots, a machine gun with
/// 200 shots, and a veteran pilot with a specialty.
fn synthetic_mech() -> UnitInfo {
    let weapon = |master_id, range, unlimited_ammo, shots, disabled| WeaponInfo {
        master_id,
        range,
        unlimited_ammo,
        shots,
        disabled,
        health: 100.0,
    };
    let mut specialties = vec![false; NUM_SPECIALTY_SKILLS];
    specialties[0] = true; // LIGHT_MECH_SPECIALIST (warrior.h)
    UnitInfo {
        id: 1,
        name: "Synthetic Atlas".into(),
        is_mech: true,
        team_is_home: true,
        contact_visual: true,
        has_team: true,
        team_not_enemy: true,
        disabled: false,
        destroyed: false,
        health: 0.35,
        weapons: vec![
            weapon(145, 1, true, 9999, false),
            weapon(121, 1, false, 16, false),
            weapon(105, 0, false, 200, false),
            weapon(145, 1, true, 9999, true),
        ],
        equipment: [None; 4],
        pilot: PilotInfo {
            active: true,
            name: "Mueller".into(),
            rank: 2,
            skills: [18, 0, 25],
            specialties,
        },
    }
}

/// Synthetic chat: a team message, a hidden name, a two-line message and a dark player colour.
fn synthetic_messages() -> Vec<ChatMessage> {
    let msg = |sender: &str, colour, text: &str, hide_name, team_text| ChatMessage {
        sender: sender.into(),
        sender_colour: colour,
        text: text.into(),
        hide_name,
        team_text,
    };
    vec![
        msg(
            "Kurita",
            0xffc8_3c3c,
            "Holding the north ridge.",
            false,
            false,
        ),
        msg(
            "Davion",
            0xff3c_64c8,
            "Team: LRMs in range of the bridge.",
            false,
            true,
        ),
        msg(
            "Steiner",
            0xff20_2020,
            "Contact at grid 4, two lights moving east, rolling up the flank now.",
            false,
            false,
        ),
        msg("Liao", 0xffe0_c040, "Pushing.", true, false),
        msg(
            "Kurita",
            0xffc8_3c3c,
            "Watch the jump jets on the Locust.",
            false,
            false,
        ),
    ]
}

/// The scene behind the info window: dark, so the window's own colours read clearly.
const DARK_SCENE: [f64; 4] = [0.10, 0.10, 0.12, 1.0];
/// A bright mission scene behind the chat list (the case the list background has to cover).
const BRIGHT_SCENE: [f64; 4] = [0.80, 0.78, 0.55, 1.0];

/// Renders `dl` over `clear`, the scene behind the window. The chat uses a bright scene so that a
/// list drawn without its background would show through.
fn render_png(gpu: &Gpu, dl: &DrawList, assets: &UiAssets, clear: [f64; 4], path: &Path) {
    let mut renderer = UiRenderer::new(gpu);
    let target = Offscreen::new(gpu, W, H);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, clear);
    let layout = UiLayout::new((W, H), (W, H), ScaleMode::Fit);
    renderer.draw(gpu, &mut encoder, &target.target(), &layout, dl, assets);
    let pixels = target.read_rgba(gpu, encoder).expect("readback");
    mc2_render::gpu::write_png(path, &pixels, W, H).expect("PNG");
    println!("{} quads -> {}", dl.quad_count(), path.display());
}

fn main() {
    let out_dir = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("VFS");
    let mut assets = UiAssets::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);

    // buttonlayout1024.fit: the info window geometry and its [Fonts] (infowindow.cpp:105-152).
    let mut buttons = ld
        .vfs
        .read_fit("data/art/buttonlayout1024.fit")
        .expect("read")
        .expect("buttonlayout1024.fit");
    let info_geom = InfoGeometry::from_fit(&mut buttons, W as i32).expect("[InfoWindow]");
    buttons.seek_block("Fonts").expect("[Fonts]");
    let info_fonts = InfoFonts {
        name: ld.font(buttons.read_i64("InfoWndFont").expect("InfoWndFont") as u32),
        component: ld.font(buttons.read_i64("ComponentFont").expect("ComponentFont") as u32),
    };

    // The chat fits (chatwindow.cpp:56, 317) and their fonts: IDS_CHAT_FONT, and Text0's Font.
    let mut main = ld
        .vfs
        .read_fit("data/art/mcl_mp_chatwidget.fit")
        .expect("read")
        .expect("mcl_mp_chatwidget.fit");
    let mut up = ld
        .vfs
        .read_fit("data/art/mcl_mp_chatwidget_up.fit")
        .expect("read")
        .expect("mcl_mp_chatwidget_up.fit");
    let chat_geom = ChatGeometry::from_fits(&mut main, &mut up).expect("chat layout");
    main.seek_block("Text0").expect("[Text0]");
    let help_font = main.read_i64("Font").expect("Text0 Font") as u32;
    let chat_fonts = ChatFonts {
        chat: ld.font(25999),
        help: ld.font(help_font),
    };

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");

    // Info window: the synthetic mech, scrolled to its end with the down button (a click moves
    // SCROLLAMOUNT = 4 px; the layout must run first so the scroll range is known).
    let mut info = InfoWindow::new(info_geom.clone(), info_fonts);
    info.set_unit(Some(synthetic_mech()));
    info.layout(&assets);
    for _ in 0..30 {
        info.update(&InfoMouse {
            left_down: true,
            ..InfoMouse::at(195, 750)
        });
    }
    let info_ops = info.render(&assets);
    let mut dl = DrawList::new(W as i32, H as i32);
    draw_ops(
        &mut RenderCtx {
            dl: &mut dl,
            assets: &assets,
            dt: 0.0,
        },
        &info_ops,
    );
    render_png(
        &gpu,
        &dl,
        &assets,
        DARK_SCENE,
        &Path::new(&out_dir).join("info-1024x768.png"),
    );

    // The same unit at scroll 0: the header, the health bar and the weapon groups.
    let mut info_top = InfoWindow::new(info_geom, info_fonts);
    info_top.set_unit(Some(synthetic_mech()));
    info_top.layout(&assets);
    let mut dl = DrawList::new(W as i32, H as i32);
    draw_ops(
        &mut RenderCtx {
            dl: &mut dl,
            assets: &assets,
            dt: 0.0,
        },
        &info_top.render(&assets),
    );
    render_png(
        &gpu,
        &dl,
        &assets,
        DARK_SCENE,
        &Path::new(&out_dir).join("info-top-1024x768.png"),
    );

    // Chat window: the synthetic messages, and the entry focused with some typed text.
    let mut chat = ChatWindow::new(chat_geom, chat_fonts, &assets);
    let messages = synthetic_messages();
    chat.update(
        &assets,
        &ChatFrame {
            messages: &messages,
            enter_pressed: false,
            commander_team: 1,
        },
    );
    chat.update(
        &assets,
        &ChatFrame {
            messages: &[],
            enter_pressed: true,
            commander_team: 1,
        },
    );
    chat.entry.type_text("Attack the bridge");
    let chat_ops = chat.render(&assets);
    let mut dl = DrawList::new(W as i32, H as i32);
    draw_ops(
        &mut RenderCtx {
            dl: &mut dl,
            assets: &assets,
            dt: 0.0,
        },
        &chat_ops,
    );
    render_png(
        &gpu,
        &dl,
        &assets,
        BRIGHT_SCENE,
        &Path::new(&out_dir).join("chat-1024x768.png"),
    );
}
