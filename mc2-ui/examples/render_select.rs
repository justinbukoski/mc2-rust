// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Render a drag selection over a synthetic scene to a PNG, headless (no window).
//!
//! `cargo run -p mc2-ui --example render_select -- [out.png]` (default `docs/select-drag-1024x768.png`)
//!
//! The scene is a grid of units (own ones grey, one enemy red) on a dark ground. A drag runs
//! through [`mission_select::frame`] in three frames (press, held, release), so the selected units
//! are the ones the real selection code picks: the box is (300,200)-(580,470) and the enemy inside
//! it is not selected (missiongui.cpp:3591-3616, `moverInRect` objmgr.cpp:2384).
use mc2_gfx::config::RenderConfig;
use mc2_platform::input::KeyStatus;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::{DrawList, GuiRect};
use mc2_ui::mission_select::{
    frame, ClickContext, DragInput, SelectState, SelectionIntent, UnitView,
};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::UiAssets;

const COLS: u32 = 8;
const ROWS: u32 = 6;
const CELL_X: f32 = 110.0;
const CELL_Y: f32 = 100.0;
const FIRST: f32 = 132.0;
const HALF: i32 = 12;

const GROUND: u32 = 0xff14181d;
const GRID: u32 = 0xff1d232b;
const OWN: u32 = 0xff8a9199;
const ENEMY: u32 = 0xffd0504a;
const SELECTED_RING: u32 = 0xff48e07a;
const BOX_FILL: u32 = 0x3058a8ff;
const BOX_EDGE: u32 = 0xffe8eef5;

/// The unit centre for grid cell `(col, row)`.
fn centre(col: u32, row: u32) -> [f32; 2] {
    [FIRST + col as f32 * CELL_X, FIRST + row as f32 * CELL_Y]
}

/// The enemy's cell, inside the drag box.
const ENEMY_CELL: (u32, u32) = (3, 2);

fn scene() -> Vec<UnitView> {
    let mut units = Vec::new();
    for row in 0..ROWS {
        for col in 0..COLS {
            let enemy = (col, row) == ENEMY_CELL;
            units.push(UnitView {
                id: row * COLS + col,
                team_home: !enemy,
                in_view: true,
                screen: centre(col, row),
                own: !enemy,
                on_gui: true,
                ..UnitView::default()
            });
        }
    }
    units
}

fn drag(status: KeyStatus, at: [f32; 2]) -> DragInput {
    DragInput {
        status,
        gui: false,
        camera: false,
        shift: false,
        world: [at[0], at[1], 0.0],
        mouse: at,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let output = args
        .get(1)
        .map_or("docs/select-drag-1024x768.png", String::as_str);
    let (width, height) = (1024u32, 768u32);

    let mut units = scene();
    let ctx = ClickContext::default();
    let project = |w: [f32; 3]| [w[0], w[1]];
    let mut state = SelectState::default();

    // The drag: press at (300,200), hold to (580,470), release there (doDrag 3560-3616).
    let press = [300.0, 200.0];
    let corner = [580.0, 470.0];
    frame(
        &mut state,
        drag(KeyStatus::Pressed, press),
        project,
        &units,
        &ctx,
    );
    frame(
        &mut state,
        drag(KeyStatus::Held, corner),
        project,
        &units,
        &ctx,
    );
    let held_dragging = state.drag.dragging();
    let res = frame(
        &mut state,
        drag(KeyStatus::Released, corner),
        project,
        &units,
        &ctx,
    );

    let SelectionIntent::Box { ids, .. } = res.selection else {
        panic!("expected a drag box, got {:?}", res.selection);
    };
    assert!(held_dragging, "the held frame must start the drag");
    for unit in &mut units {
        unit.selected = ids.contains(&unit.id);
    }

    let mut dl = DrawList::new(width as i32, height as i32);
    dl.fill_rect(GuiRect::new(0, 0, width as i32, height as i32), GROUND);
    // Faint terrain grid.
    for col in 0..=COLS {
        let x = (FIRST - CELL_X / 2.0 + col as f32 * CELL_X) as i32;
        dl.fill_rect(GuiRect::new(x, 0, x + 1, height as i32), GRID);
    }
    for row in 0..=ROWS {
        let y = (FIRST - CELL_Y / 2.0 + row as f32 * CELL_Y) as i32;
        dl.fill_rect(GuiRect::new(0, y, width as i32, y + 1), GRID);
    }
    for unit in &units {
        let [x, y] = unit.screen;
        let (x, y) = (x as i32, y as i32);
        let body = GuiRect::new(x - HALF, y - HALF, x + HALF, y + HALF);
        let colour = if unit.own { OWN } else { ENEMY };
        dl.fill_rect(body, colour);
        if unit.selected {
            let ring = GuiRect::new(x - HALF - 4, y - HALF - 4, x + HALF + 4, y + HALF + 4);
            dl.outline_rect(ring, SELECTED_RING, SELECTED_RING);
        }
    }
    // The drag box: a translucent fill and an edge, from the press point to the release point.
    let (l, t) = (press[0] as i32, press[1] as i32);
    let (r, b) = (corner[0] as i32, corner[1] as i32);
    dl.fill_rect(GuiRect::new(l, t, r, b), BOX_FILL);
    dl.outline_rect(GuiRect::new(l, t, r, b), BOX_EDGE, BOX_EDGE);

    let gpu = Gpu::headless(&RenderConfig::from_env()).expect("headless GPU");
    let assets = UiAssets::default();
    let mut renderer = UiRenderer::new(&gpu);
    let target = Offscreen::new(&gpu, width, height);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut encoder, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((width, height), (width, height), ScaleMode::Fit);
    renderer.draw(&gpu, &mut encoder, &target.target(), &layout, &dl, &assets);
    let pixels = target.read_rgba(&gpu, encoder).expect("readback");
    mc2_render::gpu::write_png(std::path::Path::new(output), &pixels, width, height).expect("PNG");
    println!("selected {ids:?} -> {output}");
}
