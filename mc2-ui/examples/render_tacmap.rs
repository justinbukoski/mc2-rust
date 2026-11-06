// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Render the in-mission tactical map to PNG without opening a window.
//!
//! The shipped mc2_01 colour map and building packet come from the mission `.pak`
//! (`MissionInterfaceManager::initTacMap`, missiongui.cpp:3085-3104: packet 2 is the buildings,
//! packet 3 the colour-map TGA); the unit list is synthetic. The tac-map rect is `Rect0` of
//! `buttonlayout800.fit` (`tacMap.setPos(rectInfos[0].rect)`, controlgui.cpp:2634):
//! left 4, top 435, right 165, bottom 596.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example render_tacmap -- 1024 768 out.png`
use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::{DrawList, GuiRect};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::tacmap::{
    parse_buildings, TacMap, TacMapArt, TacMapInput, TacTeam, TacUnit, TerrainBounds,
};
use mc2_ui::{RenderCtx, UiAssets};

/// The 800x600 logical HUD the control panel lives in (`Environment.screenWidth/Height` at the
/// default resolution; the 1024x768 target is the `.fit`-scaled HUD, `UiLayout::new`).
const UI: (u32, u32) = (800, 600);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let width: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let height: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(768);
    let output = args.get(3).map_or("tacmap.png", String::as_str);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("VFS");

    let mut assets = UiAssets::default();
    // GameTacMap::init (gametacmap.cpp:57-65) + EllipseElement::init (cellip.cpp:103-108)
    let mut art = TacMapArt::load(&mut assets.textures, &mut vfs);

    // The mission terrain: mc2_01 is the shipped 100-vertex map (realVerticesMapSide = 100,
    // terrain.cpp:370 -> worldUnitsMapSide = 12800).
    let mut map = TacMap::new(TerrainBounds::from_vertices(100));
    map.set_pos(GuiRect::new(4, 435, 165, 596)); // Rect0 of buttonlayout800.fit

    // The mission .pak packets (initTacMap(&pakFile, 2)): packet 2 = buildings, packet 3 = the
    // colour-map TGA whose header sizes the UV window (gametacmap.cpp:54-55).
    let pak_bytes = vfs
        .read("data/missions/mc2_01.pak")
        .expect("pak")
        .expect("mc2_01.pak");
    let pak = mc2_formats::pak::PakFile::from_bytes(pak_bytes).expect("pak format");
    let buildings = pak.read_packet(2).expect("building packet");
    let colmap = pak.read_packet(3).expect("colour map packet");
    art.set_terrain_packet(&mut assets.textures, "mc2_01colmap", &colmap)
        .expect("colour map TGA");
    map.art = art;
    let input = TacMapInput {
        objectives: Vec::new(),
        objective_total: 0,
        buildings: parse_buildings(&buildings),
        units: synthetic_units(),
        // top-left, bottom-left, bottom-right, top-right (gametacmap.cpp:217-243)
        view_corners: [
            (800.0, 2400.0),
            (800.0, 0.0),
            (3200.0, 0.0),
            (3200.0, 2400.0),
        ],
        placing_airstrike: false,
    };

    let mut dl = DrawList::new(UI.0 as i32, UI.1 as i32);
    let mut ctx = RenderCtx {
        dl: &mut dl,
        assets: &assets,
        dt: 0.0,
    };
    map.render(&mut ctx, &input);

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, width, height);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.05, 0.06, 0.07, 1.0]);
    let layout = UiLayout::new(UI, (width, height), ScaleMode::Fit);
    renderer.draw(&gpu, &mut encoder, &target.target(), &layout, &dl, &assets);
    let pixels = target.read_rgba(&gpu, encoder).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(output), &pixels, width, height)
        .expect("write PNG");
    println!("{} quads -> {output}", dl.quad_count());
}

/// A company spread across the map's centre: our mechs (green, one selected, one with a sensor
/// ring), another commander's units (blue), and foreign contacts (red): a neutral mover and enemy
/// contacts, since the mover pass draws every foreign team red (gametacmap.cpp:396), plus an enemy
/// artillery sensor (red, blinking; gametacmap.cpp:309-345, 366-406).
fn synthetic_units() -> Vec<TacUnit> {
    let u = |team, x, y, selected, range_m| TacUnit {
        team,
        artillery: false,
        sensor_entry: false,
        pos: (x, y),
        selected,
        sensor_range_m: range_m,
    };
    vec![
        u(TacTeam::Ours, -800.0, 400.0, false, 0.0),
        u(TacTeam::Ours, -400.0, 0.0, true, 0.0),
        u(TacTeam::Ours, 0.0, 800.0, false, 160.0),
        u(TacTeam::Ours, 400.0, 400.0, false, 0.0),
        u(TacTeam::PlayerAlt, -1200.0, -400.0, false, 0.0),
        u(TacTeam::PlayerAlt, -800.0, -800.0, true, 0.0),
        u(TacTeam::Neutral, 1600.0, 1200.0, false, 0.0),
        u(TacTeam::Enemy, 2400.0, -1200.0, false, 220.0),
        u(TacTeam::Enemy, 2800.0, -800.0, false, 0.0),
        u(TacTeam::Enemy, 2000.0, -1600.0, false, 0.0),
        TacUnit {
            artillery: true,
            sensor_entry: true,
            ..u(TacTeam::Enemy, 1200.0, 1600.0, false, 220.0)
        },
    ]
}
