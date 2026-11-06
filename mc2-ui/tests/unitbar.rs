// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission unit bar against the shipped game data.
//!
//! The data-backed tests need `MC2_DATA` (the layout, fonts and icon atlases come from it). Without
//! it they return early and say so. A missing GPU fails unless `MC2_SKIP_GPU_TESTS=1`, which prints
//! a skip line for headless environments without an adapter.
//!
//! The slot table below is the `MechIcon0..15` blocks of `buttonlayout800.fit`, read from the game
//! data (`dump` of `SelectionRect*` and `MechIconArea*`, the same keys `code/mechicon.cpp:222-279`
//! reads). It has no `HiresOffsets` block, so the offsets are zero.

use mc2_formats::vfs::Vfs;
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::ctx::RenderCtx;
use mc2_ui::draw::{DrawList, GuiRect};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::unitbar::layout::Layout;
use mc2_ui::unitbar::{
    Armor, Art, Loc, MechArmor, Pilot, UnitBar, UnitBarInput, UnitBarUnit, VehicleArmor,
};
use mc2_ui::{Loader, UiAssets};
use std::io::BufWriter;

const UI: (u32, u32) = (800, 600);
const TARGET: (u32, u32) = (1024, 768);

/// The `SelectionRect` of each slot: left, top, right, bottom (`buttonlayout800.fit`).
const SELECTION: [(i32, i32, i32, i32); 16] = [
    (178, 486, 238, 540),
    (240, 486, 300, 540),
    (303, 486, 363, 540),
    (365, 486, 425, 540),
    (428, 486, 488, 540),
    (490, 486, 550, 540),
    (553, 486, 613, 540),
    (615, 486, 675, 540),
    (178, 541, 238, 595),
    (240, 541, 300, 595),
    (303, 541, 363, 595),
    (365, 541, 425, 595),
    (428, 541, 488, 595),
    (490, 541, 550, 595),
    (553, 541, 613, 595),
    (615, 541, 675, 595),
];

/// The `MechIconArea` of each slot: left, top, right, bottom (`buttonlayout800.fit`).
const ICON_AREA: [(i32, i32, i32, i32); 16] = [
    (179, 502, 210, 539),
    (241, 502, 272, 539),
    (304, 502, 335, 539),
    (366, 502, 397, 539),
    (429, 502, 460, 539),
    (491, 502, 522, 539),
    (554, 502, 585, 539),
    (616, 502, 647, 539),
    (179, 557, 210, 594),
    (241, 557, 272, 594),
    (304, 557, 335, 594),
    (366, 557, 397, 594),
    (429, 557, 460, 594),
    (491, 557, 522, 594),
    (554, 557, 585, 594),
    (616, 557, 647, 594),
];

/// The game data root, or `None` when the test should skip (`MC2_DATA` unset).
fn data_root() -> Option<std::ffi::OsString> {
    let root = std::env::var_os("MC2_DATA");
    if root.is_none() {
        eprintln!("MC2_DATA not set; skipping");
    }
    root
}

fn gpu() -> Option<Gpu> {
    let cfg = RenderConfig {
        hires_dir: None,
        ..RenderConfig::default()
    };
    match Gpu::headless(&cfg) {
        Ok(gpu) => Some(gpu),
        Err(e)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1")
                && e.to_string().contains("adapter") =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("the unit bar render test needs a GPU: {e}"),
    }
}

fn loc(ratio: f32) -> Loc {
    Loc {
        cur: ratio,
        max: 1.0,
        destroyed: false,
    }
}

fn intact() -> MechArmor {
    let f = loc(1.0);
    MechArmor {
        head: f,
        ctorso: f,
        rctorso: f,
        ltorso: f,
        rltorso: f,
        rtorso: f,
        rrtorso: f,
        larm: f,
        rarm: f,
        lleg: f,
        rleg: f,
    }
}

fn unit(id: u32, name: &str, icon: u32, tonnage: u32, armor: Armor) -> UnitBarUnit {
    UnitBarUnit {
        id,
        icon_picture: icon,
        tonnage,
        pilot: Pilot {
            name: name.to_string(),
            photo: icon,
            message_playing: false,
        },
        armor,
        bar_status: 0.8,
        selected: false,
        groups: 0,
        shutdown: false,
        destroyed: false,
        recover_pending: false,
        power_used_up: false,
        can_jump: false,
    }
}

/// The lance: 1 intact (group 1), 2 mixed damage (group 1, a destroyed left leg), 3 a vehicle in
/// group 2, selected, and 4 dead and dying.
fn lance() -> Vec<UnitBarUnit> {
    let mut alpha = unit(1, "Alpha", 0, 75, Armor::Mech(intact()));
    let mut bravo_armor = intact();
    bravo_armor.head = loc(0.2);
    bravo_armor.larm = loc(0.5);
    bravo_armor.rarm = loc(0.1);
    bravo_armor.ctorso = loc(0.4);
    bravo_armor.rctorso = loc(0.2);
    bravo_armor.lleg.destroyed = true;
    let mut bravo = unit(2, "Bravo", 1, 55, Armor::Mech(bravo_armor));
    bravo.groups = 1 << 1;
    alpha.groups = 1 << 1;
    let mut delta = unit(
        3,
        "Delta",
        0,
        30,
        Armor::Vehicle(VehicleArmor {
            front: loc(0.05),
            rear: loc(0.2),
            left: loc(0.5),
            right: loc(1.0),
            turret: loc(1.0),
        }),
    );
    delta.groups = 1 << 2;
    delta.selected = true;
    let mut charlie = unit(4, "Charlie", 2, 40, Armor::Mech(intact()));
    charlie.destroyed = true;
    vec![alpha, bravo, delta, charlie]
}

fn write_png(path: &std::path::Path, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(BufWriter::new(file), TARGET.0, TARGET.1);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(rgba).expect("png data");
}

#[test]
fn shipped_layout_places_all_sixteen_slots() {
    let Some(root) = data_root() else { return };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let mut assets = UiAssets {
        screen: (UI.0 as i32, UI.1 as i32),
        ..UiAssets::default()
    };
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let mut fit = ld.art_fit("buttonlayout800").expect("buttonlayout800.fit");
    let layout = Layout::from_fit(&mut fit).expect("the shipped layout reads");
    for i in 0..16 {
        let (l, t, r, b) = SELECTION[i];
        assert_eq!(
            layout.slots[i].selection,
            GuiRect::new(l, t, r, b),
            "slot {i} selection"
        );
        let (l, t, r, b) = ICON_AREA[i];
        assert_eq!(
            layout.slots[i].icon_area,
            GuiRect::new(l, t, r, b),
            "slot {i} icon area"
        );
    }
}

#[test]
fn unit_bar_draws_a_lance_at_1024_by_768() {
    let Some(root) = data_root() else { return };
    let Some(gpu) = gpu() else { return };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let mut assets = UiAssets {
        screen: (UI.0 as i32, UI.1 as i32),
        ..UiAssets::default()
    };
    let (layout, art) = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut fit = ld.art_fit("buttonlayout800").expect("buttonlayout800.fit");
        let layout = Layout::from_fit(&mut fit).expect("layout");
        let art = Art::load(&mut ld, &mut fit).expect("unit bar art");
        (layout, art)
    };
    // kept for the checks below, since the bar takes the layout
    let sel = layout.slots[2].selection;
    let mut bar = UnitBar::new(layout, art);

    // the first frame gives every unit its icon; the second is 6.25 seconds in, where the death
    // stage from 6.0 to 6.5 fills the dying unit with opaque black (mechicon.cpp:76-85)
    let input = UnitBarInput {
        units: lance(),
        ..UnitBarInput::default()
    };
    // the first frame: the render starts the clocks at a frame length of 0
    let mut out = bar.update(&input, &mut assets.textures);
    let mut scratch = DrawList::new(UI.0 as i32, UI.1 as i32);
    bar.render(
        &mut RenderCtx {
            dl: &mut scratch,
            assets: &assets,
            dt: 0.0,
        },
        &mut out,
    );
    bar.update(&input, &mut assets.textures);

    // the selected vehicle (unit 3) is the third icon: delta sorts after the group 1 mechs
    assert_eq!(bar.slot_of(3), Some(2));
    // the dead unit (4) is the fourth: it sorts last, without a group
    assert_eq!(bar.slot_of(4), Some(3));

    let mut dl = DrawList::new(UI.0 as i32, UI.1 as i32);
    dl.fill_rect(GuiRect::new(0, 0, UI.0 as i32, UI.1 as i32), 0xff14_181c);
    // the second frame is 6.25 seconds in: the render takes that frame length
    bar.render(
        &mut RenderCtx {
            dl: &mut dl,
            assets: &assets,
            dt: 6.25,
        },
        &mut out,
    );

    let ui_layout = UiLayout::new(UI, TARGET, ScaleMode::Fit);
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, TARGET.0, TARGET.1);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    renderer.draw(&gpu, &mut enc, &target.target(), &ui_layout, &dl, &assets);
    let rgba = target.read_rgba(&gpu, enc).expect("read back");
    let pixel = |x: u32, y: u32| {
        let o = ((y * TARGET.0 + x) * 4) as usize;
        [rgba[o], rgba[o + 1], rgba[o + 2], rgba[o + 3]]
    };

    // the selected icon's outline is white on screen: count the white pixels in its box
    let (x0, y0) = ui_layout.to_target(sel.left as f32, sel.top as f32);
    let (x1, y1) = ui_layout.to_target(sel.right as f32, sel.bottom as f32);
    let mut white = 0;
    for y in y0 as u32..=y1 as u32 {
        for x in x0 as u32..=x1 as u32 {
            if pixel(x, y) == [255, 255, 255, 255] {
                white += 1;
            }
        }
    }
    assert!(white > 0, "no white outline around the selected unit");

    // the dead unit (slot 3, selection 365..425 x 486..540) is opaque black in the stretch between
    // its name and its bar: UI x 409..411 at y 499 (text ends by 497, the health bar is at x 366..397,
    // and the pilot rect starts at y 502). Target pixels 524 to 526 on row 639 sample that stretch
    // at the UI scale of 1.28 (`to_target`).
    for x in 524..=526 {
        assert_eq!(
            pixel(x, 639),
            [0, 0, 0, 255],
            "dead unit at target ({x}, 639)"
        );
    }
    // the bar's background behind the lance, away from every icon
    assert_eq!(pixel(10, 10), [0x14, 0x18, 0x1c, 255]);

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("unit-bar-1024x768.png");
    let path = path.as_path();
    write_png(path, &rgba);
    let size = std::fs::metadata(path).expect("png").len();
    assert!(size < 300 * 1024, "the PNG is {size} bytes");
}
