// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The info window's icon, pilot portrait and skill label art against the shipped game data
//! (`infowindow.cpp:606-617`, `:820`, `:861-879`). Needs `MC2_DATA`; a missing GPU fails unless
//! `MC2_SKIP_GPU_TESTS=1`.

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::{DrawList, GuiRect};
use mc2_ui::hud_info::draw_ops;
use mc2_ui::hud_info::icon::IconAtlases;
use mc2_ui::hud_info::info::{
    InfoFonts, InfoGeometry, InfoMouse, InfoWindow, PilotInfo, UnitInfo, WeaponInfo,
    NUM_SPECIALTY_SKILLS,
};
use mc2_ui::hud_info::HudOp;
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::unitbar::{Armor, Art, Loc, MechArmor, Pilot, UnitBarUnit};
use mc2_ui::widgets::button::Button;
use mc2_ui::widgets::object::Object;
use mc2_ui::{Loader, RenderCtx, UiAssets};

const W: u32 = 1024;
const H: u32 = 768;

fn mech_info() -> UnitInfo {
    let mut specialties = vec![false; NUM_SPECIALTY_SKILLS];
    specialties[0] = true;
    UnitInfo {
        id: 7,
        name: "Atlas".into(),
        is_mech: true,
        team_is_home: true,
        contact_visual: true,
        has_team: true,
        team_not_enemy: true,
        disabled: false,
        destroyed: false,
        health: 0.8,
        weapons: [(145, 0), (121, 1), (105, 2), (146, 1), (122, 2), (106, 0)]
            .iter()
            .map(|&(master_id, range)| WeaponInfo {
                master_id,
                range,
                unlimited_ammo: false,
                shots: 20,
                disabled: false,
                health: 100.0,
            })
            .collect(),
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

fn bar_unit() -> UnitBarUnit {
    let full = Loc {
        cur: 1.0,
        max: 1.0,
        destroyed: false,
    };
    let armor = MechArmor {
        head: full,
        ctorso: full,
        rctorso: full,
        ltorso: full,
        rltorso: full,
        rtorso: full,
        rrtorso: full,
        larm: full,
        rarm: full,
        lleg: full,
        rleg: full,
    };
    UnitBarUnit {
        id: 7,
        icon_picture: 3,
        tonnage: 100,
        pilot: Pilot {
            name: "Mueller".into(),
            photo: 5,
            message_playing: false,
        },
        armor: Armor::Mech(armor),
        bar_status: 0.8,
        selected: true,
        groups: 0,
        shutdown: false,
        destroyed: false,
        recover_pending: false,
        power_used_up: false,
        can_jump: false,
    }
}

struct Fixture {
    info: InfoWindow,
    assets: UiAssets,
    geom: InfoGeometry,
}

fn fixture(root: &std::ffi::OsStr) -> Fixture {
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("vfs");
    let mut assets = UiAssets::default();
    let (geom, info) = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut fit = ld.art_fit("buttonlayout1024").expect("layout");
        let geom = InfoGeometry::from_fit(&mut fit, W as i32).expect("[InfoWindow]");
        let mut fit = ld.art_fit("buttonlayout1024").expect("layout");
        fit.seek_block("Fonts").expect("[Fonts]");
        let fonts = InfoFonts {
            name: ld.font(fit.read_i64("InfoWndFont").expect("font") as u32),
            component: ld.font(fit.read_i64("ComponentFont").expect("font") as u32),
        };
        let mut fit = ld.art_fit("buttonlayout1024").expect("layout");
        let art = Art::load(&mut ld, &mut fit).expect("unit bar art");
        let mut info = InfoWindow::new(geom.clone(), fonts);
        info.set_atlases(IconAtlases {
            mech: art.mech_atlas,
            vehicle: art.vehicle_atlas,
            pilot: art.pilot_atlas,
        });
        let mut fit = ld.art_fit("buttonlayout1024").expect("layout");
        info.set_skill_art(std::array::from_fn(|i| {
            Object::from_fit(&mut ld, &mut fit, &format!("Skill{i}"))
        }));
        let mut fit = ld.art_fit("buttonlayout1024").expect("layout");
        let up = Button::from_fit(&mut ld, &mut fit, "InfoButton0");
        let down = Button::from_fit(&mut ld, &mut fit, "InfoButton1");
        info.set_scroll_buttons(up, down);
        (geom, info)
    };
    let mut f = Fixture { info, assets, geom };
    f.info.set_unit(Some(mech_info()));
    f.info.sync_icon(&mut f.assets.textures, Some(&bar_unit()));
    f.info.layout(&f.assets);
    f
}

fn gpu() -> Option<Gpu> {
    match Gpu::headless(&RenderConfig::from_env()) {
        Ok(g) => Some(g),
        Err(e)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1")
                && e.to_string().contains("adapter") =>
        {
            eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU ({e})");
            None
        }
        Err(e) => panic!("info art test needs a GPU: {e}"),
    }
}

fn draw(gpu: &Gpu, f: &mut Fixture) -> Vec<u8> {
    let ops = f.info.render(&f.assets);
    let mut dl = DrawList::new(W as i32, H as i32);
    draw_ops(
        &mut RenderCtx {
            dl: &mut dl,
            assets: &f.assets,
            dt: 0.0,
        },
        &ops,
    );
    let mut renderer = UiRenderer::new(gpu);
    let target = Offscreen::new(gpu, W, H);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((W, H), (W, H), ScaleMode::Fit);
    renderer.draw(gpu, &mut encoder, &target.target(), &layout, &dl, &f.assets);
    target.read_rgba(gpu, encoder).expect("readback")
}

/// The bounding box of a `Tex` op's quad.
fn tex_rect(op: &HudOp) -> Option<GuiRect> {
    let HudOp::Tex { quad, .. } = op else {
        return None;
    };
    let xs = quad.iter().map(|v| v.x);
    let ys = quad.iter().map(|v| v.y);
    Some(GuiRect::new(
        xs.clone().fold(f32::MAX, f32::min) as i32,
        ys.clone().fold(f32::MAX, f32::min) as i32,
        xs.fold(f32::MIN, f32::max) as i32,
        ys.fold(f32::MIN, f32::max) as i32,
    ))
}

/// Lit (non-black) pixels of `r` clipped to `clip`.
fn lit(img: &[u8], r: GuiRect, clip: GuiRect) -> usize {
    let r = r.intersect(&clip);
    let mut n = 0;
    for y in r.top.max(0)..r.bottom.min(H as i32) {
        for x in r.left.max(0)..r.right.min(W as i32) {
            let o = ((y as u32 * W + x as u32) * 4) as usize;
            if img[o] as u32 + img[o + 1] as u32 + img[o + 2] as u32 > 30 {
                n += 1;
            }
        }
    }
    n
}

fn save(img: &[u8], name: &str) {
    let dir = visual_dir();
    std::fs::create_dir_all(&dir).expect("docs/visual");
    mc2_render::gpu::write_png(&dir.join(name), img, W, H).expect("PNG");
}

#[test]
fn the_info_window_draws_the_unit_icons_portrait_and_skill_labels() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let Some(gpu) = gpu() else { return };
    let mut f = fixture(&root);
    let g = f.geom.clone();

    // top of the content: the front and rear cells of the icon in their FIT rects
    let ops = f.info.layout_for(&mech_info(), 0.0, &f.assets).ops;
    let rects: Vec<GuiRect> = ops.iter().filter_map(tex_rect).collect();
    let icon_top = g.content_box.top + g.section_skip;
    let front = GuiRect::new(
        g.mech_x.0,
        icon_top,
        g.mech_x.1,
        icon_top + g.mech_icon_height,
    );
    let back = GuiRect::new(
        g.mech_back_x.0,
        icon_top,
        g.mech_back_x.1,
        icon_top + g.mech_icon_height,
    );
    assert!(rects.contains(&front), "front cell in {front:?}: {rects:?}");
    assert!(rects.contains(&back), "rear cell in {back:?}: {rects:?}");
    let img = draw(&gpu, &mut f);
    assert!(lit(&img, front, g.content_box) > 150, "front icon pixels");
    assert!(lit(&img, back, g.content_box) > 50, "rear icon pixels");
    save(&img, "info-window-icon.png");

    // scrolled to the end: the portrait and the three skill labels
    for _ in 0..60 {
        f.info.update(&InfoMouse {
            left_down: true,
            ..InfoMouse::at(195, 750)
        });
    }
    let ops = f.info.layout(&f.assets).ops;
    let tex: Vec<GuiRect> = ops.iter().filter_map(tex_rect).collect();
    let portrait = tex
        .iter()
        .find(|r| r.left == g.pilot_x.0 && r.right == g.pilot_x.1)
        .copied()
        .expect("the portrait quad in PilotLeft..PilotRight");
    assert_eq!(portrait.height(), g.pilot_height);
    let labels: Vec<GuiRect> = tex
        .iter()
        .filter(|r| r.left == g.skill_labels[0].x && r.width() == g.skill_labels[0].width)
        .copied()
        .collect();
    assert_eq!(labels.len(), 3, "three skill label quads: {tex:?}");
    let img = draw(&gpu, &mut f);
    assert!(lit(&img, portrait, g.content_box) > 100, "portrait pixels");
    let visible = labels
        .iter()
        .filter(|r| r.intersect(&g.content_box).height() >= 10)
        .collect::<Vec<_>>();
    assert!(
        !visible.is_empty(),
        "a skill label is in view: {labels:?} scroll {} box {:?}",
        f.info.scroll_pos(),
        g.content_box
    );
    for r in visible {
        assert!(lit(&img, *r, g.content_box) > 100, "skill label {r:?}");
    }
    // the scroll buttons' art sits on the FIT button rects while the content scrolls
    let ops = f.info.render(&f.assets);
    let tex: Vec<GuiRect> = ops.iter().filter_map(tex_rect).collect();
    for button in [g.up_button, g.down_button] {
        assert!(
            tex.iter()
                .any(|r| r.left == button.left && r.top == button.top && r.right == button.right),
            "scroll button art at {button:?}: {tex:?}"
        );
        assert!(
            lit(&img, button, g.content_box) > 10,
            "button pixels {button:?}"
        );
    }
    save(&img, "info-window-pilot.png");
}

/// PNG output dir: `MC2_VISUAL_DIR` (e.g. rust/docs/visual) or the cargo test tmp dir, so a plain
/// `cargo test` never writes into the source tree.
fn visual_dir() -> std::path::PathBuf {
    std::env::var_os("MC2_VISUAL_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("visual"))
}
