// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission tactical map (`code/gametacmap.cpp` and the `TacMap` base it uses in
//! `mclib/tacmap.cpp`): the coordinate maps, the click behaviour and the drawing.
//!
//! The widget takes plain input data ([`TacMapInput`]) and knows nothing about the simulation.
//!
//! Every expected value below is hand-derived from the C++ cited in the test. The constants:
//!
//! ```text
//! terrain.h:57        MAPCELL_DIM = 3
//! terrain.cpp:64      metersPerWorldUnit = 0.2f
//! terrain.cpp:76      worldUnitsPerVertex = 128.0f
//! terrain.cpp:77      worldUnitsPerCell = worldUnitsPerVertex / MAPCELL_DIM   (float divide)
//! terrain.cpp:79      metersPerCell = worldUnitsPerCell * metersPerWorldUnit
//! terrain.cpp:81      oneOverWorldUnitsPerVertex = 1.0f / 128.0f
//! terrain.cpp:370     worldUnitsMapSide = realVerticesMapSide * worldUnitsPerVertex
//! terrain.cpp:445-446 mapTopLeft3d = (-worldUnitsMapSide/2.0f, +worldUnitsMapSide/2.0f)
//! terrain.cpp:275     cellRowToWorldCoord[i] = (worldUnitsMapSide/2) - i*worldUnitsPerCell
//! terrain.cpp:279     cellColToWorldCoord[i] = i*worldUnitsPerCell - (worldUnitsMapSide/2)
//! terrain.h:346-350   worldToCell:
//!                     cellC = (int)((x - mapTopLeft.x) * (oneOverWorldUnitsPerVertex*3.0f))
//!                     cellR = (int)((mapTopLeft.y - y) * (oneOverWorldUnitsPerVertex*3.0f))
//!                     ((int) truncates toward zero, mathfunc.h:115 float2long)
//! ```
//!
//! The test map is the shipped 100-vertex one: `realVerticesMapSide = 100`,
//! `worldUnitsMapSide = 100 * 128 = 12800`, `mapTopLeft = (-6400, +6400)`, and
//! `realVerticesMapSide * MAPCELL_DIM = 300` cells per side. The widget is placed in the rect
//! `left=10, top=10, right=130, bottom=110` (`GameTacMap::setPos`, gametacmap.cpp:553-559, from
//! the control panel layout's `Rect0`), so `xSize = right-left = 120`, `ySize = bottom-top = 100`.
//!
//! The pixel expectations are written as the *same f32 expression* the C++ evaluates —
//! `cell * (xSize / cells) + offset` with every literal an `f32` (tacmap.cpp:35-39) — because
//! e.g. `173.0f * 120.0f / 300.0f + 10.0f` is `79.2`, while `173.0f * (120.0f / 300.0f) + 10.0f`
//! is `79.200005`. The module evaluates the C++'s expression, so the tests must too.

use mc2_platform::input::{InputEvent, MouseButton};
use mc2_ui::ctx::UiAssets;
use mc2_ui::draw::{Blend, GuiRect, Vertex};
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::tacmap::{
    parse_buildings, TacMap, TacMapArt, TacMapInput, TacTeam, TacUnit, TerrainBounds,
    IDS_TACMAP_HELP, UNIT_COLOR_ENEMY, UNIT_COLOR_ENEMY_SELECTED, UNIT_COLOR_NEUTRAL,
    UNIT_COLOR_NEUTRAL_SELECTED, UNIT_COLOR_OURS, UNIT_COLOR_OURS_SELECTED, UNIT_COLOR_PLAYER_ALT,
    UNIT_COLOR_PLAYER_ALT_SELECTED,
};

/// A right-button event (the tac map's move-order button, gametacmap.cpp:100).
fn right(pressed: bool) -> InputEvent {
    InputEvent::Button {
        button: MouseButton::Right,
        pressed,
    }
}

const HALF: f32 = 6400.0; // worldUnitsMapSide / 2 (terrain.cpp:370,445)
const CELLS: i32 = 300; // realVerticesMapSide * MAPCELL_DIM (tacmap.cpp:30)

/// The widget rect: `ControlGui::readRects`' `Rect0` (`tacMap.setPos(rectInfos[0].rect)`,
/// controlgui.cpp:2634), as a 120x100 tac-map panel at (10,10).
const RECT: GuiRect = GuiRect {
    left: 10,
    top: 10,
    right: 130,
    bottom: 110,
};

/// `tacmap.cpp:35-39` in f32, exactly: `(float)cell * (float)xSize / (float)cells + xOffset`.
fn px(cell: f32, size: f32, offset: f32) -> f32 {
    cell * size / 300.0 + offset
}

/// A widget on the 100-vertex map with a 128x128 colour map (the shipped thumbnail size).
fn tac_map() -> TacMap {
    let mut m = TacMap::new(TerrainBounds {
        cells_per_side: CELLS,
        map_side: 12800.0,
    });
    m.set_pos(RECT);
    m.set_terrain_size(128, 128);
    m
}

/// A harness whose double-click clock has run past `DOUBLE_CLICK_THRESHOLD` (0.2 s) with no
/// button down, as it always has in the game: `UserInput::update` counts `mouseLeftUpTime` from
/// start-up (userinput.cpp:222-232), so a press in the first frames would be a double click.
fn settled_harness() -> Harness {
    let mut h = Harness::new();
    h.idle(10, |_| ());
    h
}

/// A mover (`ObjectManager`, gametacmap.cpp:347-406).
fn unit(team: TacTeam, pos: (f32, f32), selected: bool, range_m: f32) -> TacUnit {
    TacUnit {
        team,
        artillery: false,
        sensor_entry: false,
        pos,
        selected,
        sensor_range_m: range_m,
    }
}

/// A non-mover sensor entry (`SensorManager`, gametacmap.cpp:274-345).
fn sensor_unit(team: TacTeam, pos: (f32, f32), selected: bool, range_m: f32) -> TacUnit {
    TacUnit {
        sensor_entry: true,
        ..unit(team, pos, selected, range_m)
    }
}

/// An artillery sensor of `team` (`objClass == ARTILLERY`, gametacmap.cpp:327): a sensor entry.
fn artillery_unit(team: TacTeam, pos: (f32, f32), selected: bool, range_m: f32) -> TacUnit {
    TacUnit {
        artillery: true,
        ..sensor_unit(team, pos, selected, range_m)
    }
}

fn input() -> TacMapInput {
    TacMapInput {
        buildings: vec![(0, 0), (5, 7)],
        units: vec![],
        objectives: vec![],
        objective_total: 0,
        view_corners: [(0.0, 0.0); 4],
        placing_airstrike: false,
    }
}

fn quads(dl: &mc2_ui::draw::DrawList) -> Vec<&[Vertex]> {
    dl.vertices.chunks(4).collect()
}

/// The quad of batch `i` (every batch in these tests holds one quad).
fn batch_quad(dl: &mc2_ui::draw::DrawList, i: usize) -> &[Vertex] {
    let b = &dl.batches[i];
    &dl.vertices[(b.first_quad as usize) * 4..(b.first_quad as usize + b.quad_count as usize) * 4]
}

/// The blip quads: the ones with the blip UV window `0 .. 0.250001` (`triangle[2].u =
/// triangle[3].u = .250001f`, `triangle[1].v = triangle[2].v = .250001f`, gametacmap.cpp:540-548
/// — all four corners carry `u = v = 0` first, lines 529-531). The ring's UV window is the full
/// `0..1` (cellip.cpp:45-46), so the filter on `u` cannot catch it.
fn blips(dl: &mc2_ui::draw::DrawList) -> Vec<(f32, f32, u32, f32)> {
    dl.vertices
        .chunks(4)
        .filter(|q| {
            (q[2].u - 0.250001).abs() < 1e-6
                && (q[3].u - 0.250001).abs() < 1e-6
                && (q[1].v - 0.250001).abs() < 1e-6
        })
        .map(|q| (q[0].x, q[0].y, q[0].argb, q[2].x))
        .collect()
}

/// The widget under test, plus the input it is driven with. The original has one `GameTacMap`
/// (`ControlGui::tacMap`, controlgui.h) whose update the control panel calls every frame
/// (controlgui.cpp:1202).
struct Widget {
    m: TacMap,
    inp: TacMapInput,
}

impl Widget {
    fn new() -> Widget {
        Widget {
            m: tac_map(),
            inp: input(),
        }
    }

    /// One frame the way the HUD does it: route `events` through the widget's routing screen (the
    /// app's routing, [`mc2_ui::screen::LogisticsScreen::route`]), then update the widget
    /// (`GameTacMap::update`, controlgui.cpp:1202). The screen is taken out of the widget with
    /// [`std::mem::take`] so the frame's two borrows — the screen and the widget — do not
    /// overlap; it is put back when the frame ends.
    fn frame(&mut self, h: &mut Harness, events: &[InputEvent]) -> Vec<bool> {
        let Self { m, inp } = self;
        let mut screen = std::mem::take(&mut m.screen);
        let taken = h
            .routed_frame(&mut screen, events, |_, ctx| m.update(ctx, inp))
            .0;
        m.screen = screen;
        taken
    }
}

// ---------------------------------------------------------------------------------------------
// Step 1: the coordinate maps, exactly as the C++ computes them

/// `TacMap::worldToTacMap` (mclib/tacmap.cpp:20-41) through `GameTacMap::worldToTacMap`
/// (code/gametacmap.cpp:434-437, which passes `left`, `top`, `right - left`, `bottom - top`).
///
/// Point A, the map centre `(0, 0)`. `worldToCell` (terrain.h:346):
///   cellC = trunc((0 - (-6400)) * ((1/128) * 3)) = trunc(6400 * 0.0234375) = trunc(150) = 150
///   cellR = trunc((6400 - 0) * 0.0234375) = 150
/// then tacmap.cpp:35-40 with xSize = 120, ySize = 100, xOffset = 10, yOffset = 10:
///   tac.x = 150 * 120 / 300 + 10 = 70    tac.y = 150 * 100 / 300 + 10 = 60
#[test]
fn world_to_tac_map_centre() {
    let m = tac_map();
    assert_eq!(m.world_to_tac_map(0.0, 0.0), (70.0, 60.0));
}

/// Point B, the north-west corner `mapTopLeft3d = (-6400, +6400)` (terrain.cpp:445-446):
/// cellC = trunc(0 * k) = 0, cellR = trunc(0 * k) = 0, so (tacmap.cpp:35-40)
///   tac.x = 0 * 120/300 + 10 = 10    tac.y = 0 * 100/300 + 10 = 10
#[test]
fn world_to_tac_map_north_west_corner() {
    let m = tac_map();
    assert_eq!(m.world_to_tac_map(-HALF, HALF), (10.0, 10.0));
}

/// Point C, the south-east corner `(6400, -6400)`:
/// cellC = cellR = trunc(12800 * 0.0234375) = trunc(300) = 300, which is exactly the clamp
/// value `realVerticesMapSide * MAPCELL_DIM` (tacmap.cpp:30-33), so
///   tac.x = 300 * 120/300 + 10 = 130    tac.y = 300 * 100/300 + 10 = 110
#[test]
fn world_to_tac_map_south_east_corner() {
    let m = tac_map();
    assert_eq!(m.world_to_tac_map(HALF, -HALF), (130.0, 110.0));
}

/// Rounding case 1 (truncation toward zero, terrain.h:346): `(6399.999, 6399.999)`.
///   cellC = trunc(12799.999 * 0.0234375) = trunc(299.99997) = 299
///   cellR = trunc(0.001 * 0.0234375) = 0
///   tac.x = 299 * 120/300 + 10   tac.y = 0 + 10 = 10
#[test]
fn world_to_tac_map_truncates_toward_zero() {
    let m = tac_map();
    let (x, y) = m.world_to_tac_map(6399.999, 6399.999);
    assert_eq!(y, 10.0);
    assert_eq!(x, px(299.0, 120.0, 10.0), "299 cells");
}

/// Rounding case 2 (a position inside a cell): `(1000, -1000)`.
///   cellC = trunc((1000 + 6400) * 0.0234375) = trunc(173.4375) = 173
///   cellR = trunc((6400 + 1000) * 0.0234375) = 173
///   tac.x = 173 * 120/300 + 10   tac.y = 173 * 100/300 + 10
#[test]
fn world_to_tac_map_fractional_cell() {
    let m = tac_map();
    let (x, y) = m.world_to_tac_map(1000.0, -1000.0);
    assert_eq!(x, px(173.0, 120.0, 10.0));
    assert_eq!(y, px(173.0, 100.0, 10.0));
}

/// Clamping (tacmap.cpp:26-33): the cells are clamped to `0..=300`, so far-off positions land on
/// the rect corners; just outside the north-west corner the truncation still gives cell 0
/// (trunc(-0.0234) = 0), and just outside the south-east corner trunc(300.00003) = 300.
#[test]
fn world_to_tac_map_clamps_outside() {
    let m = tac_map();
    assert_eq!(m.world_to_tac_map(-99999.0, 99999.0), (10.0, 10.0));
    assert_eq!(m.world_to_tac_map(99999.0, -99999.0), (130.0, 110.0));
    assert_eq!(m.world_to_tac_map(-6400.001, 6400.001), (10.0, 10.0));
    let (x, y) = m.world_to_tac_map(6400.001, -6400.001);
    assert_eq!(x, px(300.0, 120.0, 10.0));
    assert_eq!(y, px(300.0, 100.0, 10.0));
}

/// `TacMap::tacMapToWorld` (mclib/tacmap.cpp:43-61): the click position relative to the rect
/// (`screen.x -= left; screen.y -= top`, gametacmap.cpp:85-86) becomes
///   cellX = (long)(sx / xSize * 300)   cellY = (long)(sy / ySize * 300)   [truncation]
/// and the world position is the *corner* (not the centre) of that cell:
///   world.x = cellColToWorldCoord[cellX]   (terrain.cpp:279)
///   world.y = cellRowToWorldCoord[cellY]   (terrain.cpp:275)
///
/// Click (0, 0) -> cell (0, 0) -> (-6400, +6400), the north-west corner.
#[test]
fn tac_map_click_top_left_is_map_corner() {
    let m = tac_map();
    assert_eq!(m.tac_map_to_world(0, 0), Some((-HALF, HALF)));
}

/// Click (119, 99), the last pixel of the 120x100 rect:
///   cellX = trunc(119/120 * 300) = trunc(297.5) = 297
///   cellY = trunc(99/100 * 300) = trunc(297) = 297
///   x = 297 * (128/3) - 6400 = 88704/3 - 19200/3 = 69504/3 = 23168   [f32: exact]
///   y = 6400 - 23168 = -16768
/// (`worldUnitsPerCell = 128.0f / 3.0f` is inexact, so the corner table's f32 value is what the
/// C++ returns; `px`-style expressions keep the same rounding.)
#[test]
fn tac_map_click_bottom_right() {
    let m = tac_map();
    let (x, y) = m.tac_map_to_world(119, 99).expect("inside");
    assert_eq!(x, 297.0 * (128.0 / 3.0) - 6400.0);
    assert_eq!(y, 6400.0 - 297.0 * (128.0 / 3.0));
}

/// Click (60, 50), the centre: cell (60/120*300, 50/100*300) = (150, 150):
///   x = 150 * (128/3) - 6400 = 19200/3 - 6400 = 0    y = 6400 - 19200/3 = 0
#[test]
fn tac_map_click_centre() {
    let m = tac_map();
    assert_eq!(m.tac_map_to_world(60, 50), Some((0.0, 0.0)));
}

/// Out of the cell range the C++ keeps the camera position
/// (`world = eye->getPosition()`, tacmap.cpp:49-53): cell 300 is `>= 300`, and negatives fail the
/// `< 0` test.
#[test]
fn tac_map_click_outside_falls_back_to_camera() {
    let m = tac_map();
    assert_eq!(m.tac_map_to_world(120, 50), None);
    assert_eq!(m.tac_map_to_world(-1, 5), None);
    assert_eq!(m.tac_map_to_world(60, 100), None);
}

/// Round trip: a click lands on a cell corner whose `worldToTacMap` returns the same cell. The
/// corner table (terrain.cpp:275-279) and the cell multiply (tacmap.cpp:35) agree exactly: the
/// corner of cell `c` maps back to `c * xSize / 300`.
#[test]
fn click_round_trips_to_its_cell() {
    let m = tac_map();
    for (sx, sy) in [(0, 0), (60, 50), (119, 99), (37, 12)] {
        let (wx, wy) = m.tac_map_to_world(sx, sy).expect("inside");
        let (tx, ty) = m.world_to_tac_map(wx, wy);
        // the click's cell (mclib/tacmap.cpp:46-47)
        let cell_x = (sx as f32 / 120.0 * 300.0) as i32;
        let cell_y = (sy as f32 / 100.0 * 300.0) as i32;
        assert_eq!(tx, px(cell_x as f32, 120.0, 10.0), "x of cell {cell_x}");
        assert_eq!(ty, px(cell_y as f32, 100.0, 10.0), "y of cell {cell_y}");
    }
}

// ---------------------------------------------------------------------------------------------
// Step 2: the widget — click handling (GameTacMap::update, gametacmap.cpp:68-112)

#[test]
fn left_click_inside_moves_camera_and_publishes_help() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    // gametacmap.cpp:78-81: the rollover help text is set whenever the mouse is in the region
    // (inRegion, gametacmap.h:44: strict, exclusive borders).
    let taken = w.frame(&mut h, &[move_to(70.0, 60.0)]);
    assert!(taken[0], "the tac map area is part of the HUD");
    assert_eq!(h.out.help_text_id, IDS_TACMAP_HELP);
    assert_eq!(w.m.camera_target, None);

    // gametacmap.cpp:83-98: a left click inside moves the camera (eye->setPosition)
    w.frame(&mut h, &[move_to(70.0, 60.0), left(true), left(false)]);
    // mouse (70,60) -> relative (60,50) -> cell (150,150) -> world (0,0)
    assert_eq!(w.m.last_click, Some((0.0, 0.0)));
    assert_eq!(w.m.camera_target, Some((0.0, 0.0)));
    assert_eq!(
        w.m.last_move_order, None,
        "a plain click is not a move order"
    );
}

#[test]
fn clicks_outside_the_region_do_nothing() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    // outside the rect: update returns before even setting the help text (gametacmap.cpp:78-79)
    w.frame(&mut h, &[move_to(5.0, 5.0)]);
    assert_ne!(h.out.help_text_id, IDS_TACMAP_HELP);
    w.frame(&mut h, &[move_to(5.0, 5.0), left(true), left(false)]);
    assert_eq!(w.m.last_click, None);
    assert_eq!(w.m.camera_target, None);
}

/// `leftClick` is the press edge only: `UserInput::update` clears it every frame
/// (userinput.cpp:171) and `KEY_PRESSED` sets it (userinput.cpp:242), while `KEY_HELD` sets
/// nothing for the left button. A button held over the map therefore moves the camera once, on
/// the press frame; the held frames and the release frame order nothing.
#[test]
fn held_left_button_moves_the_camera_once() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    w.frame(&mut h, &[move_to(70.0, 60.0), left(true)]);
    assert_eq!(
        w.m.camera_target,
        Some((0.0, 0.0)),
        "the press frame clicks"
    );
    w.m.camera_target = None;
    w.m.last_click = None;
    for _ in 0..5 {
        w.frame(&mut h, &[]);
        assert_eq!(w.m.last_click, None, "held: no left click");
        assert_eq!(
            w.m.camera_target, None,
            "held: the camera does not move again"
        );
    }
    w.frame(&mut h, &[left(false)]);
    assert_eq!(w.m.last_click, None, "release: no left click");
}

/// A quick second press is `leftDoubleClick`, not `leftClick` (userinput.cpp:235-242): the
/// double click does not move the camera. The press after it is a click again.
#[test]
fn double_click_does_not_move_the_camera() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    w.frame(&mut h, &[move_to(70.0, 60.0), left(true)]);
    assert_eq!(
        w.m.camera_target,
        Some((0.0, 0.0)),
        "the first press clicks"
    );
    w.frame(&mut h, &[left(false)]);
    w.m.camera_target = None;
    w.frame(&mut h, &[left(true)]);
    assert_eq!(w.m.last_click, None, "the second press is a double click");
    assert_eq!(w.m.camera_target, None);
}

/// The right button keeps the C++ held behaviour: `KEY_HELD` sets `rightClick` on every held
/// frame (userinput.cpp:327), so a held right button with `useLeftRightMouseProfile` orders a
/// move each frame. Only the release frame orders nothing.
#[test]
fn held_right_button_repeats_the_move_order_like_the_cpp() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    w.m.left_right_profile = true;
    w.frame(&mut h, &[move_to(70.0, 60.0), right(true)]);
    assert_eq!(w.m.last_move_order, Some((0.0, 0.0)));
    for _ in 0..3 {
        w.m.last_move_order = None;
        w.frame(&mut h, &[]);
        assert_eq!(w.m.last_move_order, Some((0.0, 0.0)), "held right button");
    }
    w.m.last_move_order = None;
    w.frame(&mut h, &[right(false)]);
    assert_eq!(w.m.last_move_order, None, "release: no move order");
}

/// `useLeftRightMouseProfile` (gametacmap.cpp:100-110): the right click orders a move to the
/// same world position the left click would move the camera to.
#[test]
fn right_click_orders_move_when_profile_enabled() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    w.m.left_right_profile = true;
    w.frame(&mut h, &[move_to(70.0, 60.0), right(true), right(false)]);
    assert_eq!(w.m.last_move_order, Some((0.0, 0.0)));
    assert_eq!(
        w.m.camera_target, None,
        "the right click does not move the camera"
    );

    // with the profile off the right click does nothing
    w.m.last_move_order = None;
    w.m.left_right_profile = false;
    w.frame(&mut h, &[move_to(70.0, 60.0), right(true), right(false)]);
    assert_eq!(w.m.last_move_order, None);
}

/// `isAddingAirstrike() && isButtonPressed(SENSOR_PROBE)` (gametacmap.cpp:91-93): with the
/// airstrike probe armed a left click orders a move (`doMove`) instead of moving the camera.
#[test]
fn left_click_orders_move_while_airstrike_probe_armed() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    w.inp.placing_airstrike = true;
    w.frame(&mut h, &[move_to(70.0, 60.0), left(true), left(false)]);
    assert_eq!(w.m.last_move_order, Some((0.0, 0.0)), "doMove(world)");
    assert_eq!(
        w.m.camera_target, None,
        "the airstrike click does not move the camera (line 96 is not reached)"
    );
    assert_eq!(w.m.last_click, Some((0.0, 0.0)), "tacMapToWorld still ran");
}

/// A click on the rect's border pixel does nothing: `GameTacMap::inRegion` (gametacmap.h:44) is
/// strict (`x < right`), so the pixel at `x == right == 130` is outside the region and
/// `GameTacMap::update` returns at gametacmap.cpp:78-79 — the camera stays where it is.
#[test]
fn click_outside_cell_range_keeps_camera() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    w.m.camera_target = Some((123.0, -456.0));
    w.frame(&mut h, &[move_to(130.0, 109.0), left(true), left(false)]);
    assert_eq!(w.m.last_click, None);
    assert_eq!(w.m.camera_target, Some((123.0, -456.0)));
}

/// The tac-map click fallback (`world = eye->getPosition()`, tacmap.cpp:49-53) applies to cells
/// outside the grid: [`TacMap::tac_map_to_world`] returns `None` there and the camera stays. The
/// shipped layout (a 120 px rect on the 300-cell grid) has no such click inside the region:
/// `inRegion` is strict (`x < right`, gametacmap.h:44), so the first out-of-range column
/// `sx == xSize` is outside it. This test pins the map's `None` at those two points. It then
/// checks that a click on the last in-range cell of a 100-cell grid moves the camera. The
/// fallback is not reached through an in-region click here; only the map function covers it.
#[test]
fn fallback_is_pinned_on_the_map_and_last_cell_click_moves_camera() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    // The first out-of-range column and row of the shipped 120x100 rect:
    assert_eq!(
        w.m.tac_map_to_world(120, 50),
        None,
        "sx = xSize is the first out-of-range column"
    );
    assert_eq!(
        w.m.tac_map_to_world(60, 100),
        None,
        "sy = ySize is out of range"
    );

    // A degenerate 1-cell grid: the click still lands on cell (0, 0) everywhere inside the
    // region, as in the C++ — the fallback needs a rect/grid mismatch the shipped control.fit
    // never has. Pin the camera-keeping behaviour with a grid the rect outgrows: cells = 100
    // on the 120x100 rect, the click at relative (119, 99) -> cell (99, 99), valid, moves the
    // camera; the map's `None` keeps it.
    w.m.bounds = TerrainBounds {
        cells_per_side: 100,
        map_side: 12800.0,
    };
    w.m.camera_target = Some((123.0, -456.0));
    w.frame(&mut h, &[move_to(129.0, 109.0), left(true), left(false)]);
    assert!(w.m.last_click.is_some(), "the click lands on cell (99, 99)");
    assert_eq!(w.m.camera_target, w.m.last_click);
    assert_eq!(w.m.last_move_order, None);
}

// ---------------------------------------------------------------------------------------------
// Step 3: drawing

/// The tac-map area is part of the HUD: the widget's routing screen covers the rect, so the
/// pointer over the map is taken by the UI (`ControlGui::inRegion`, controlgui.cpp:1278-1285,
/// routes the control panel's `Rect0` to the UI).
#[test]
fn the_routing_screen_covers_the_tac_map_rect() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    let taken = w.frame(&mut h, &[move_to(70.0, 60.0)]);
    assert!(taken[0]);
    // and it does not reach outside the rect
    let taken = w.frame(&mut h, &[move_to(5.0, 5.0)]);
    assert!(!taken[0]);
}

/// `GameTacMap::setPos` replaces the rect (gametacmap.cpp:553-559), so a layout change moves the
/// tac map's input region with it: the old area releases the pointer and the new area takes it.
/// The routing screen must not keep the old rect behind (the widget registers it on every call).
#[test]
fn moving_the_tac_map_moves_its_input_region() {
    let mut h = settled_harness();
    let mut w = Widget::new();
    let moved = GuiRect {
        left: 300,
        top: 300,
        right: 420,
        bottom: 400,
    };
    w.m.set_pos(moved);
    assert_eq!(w.m.screen.rects.len(), 1, "one routing rect after a move");

    // the old area (inside RECT, 10..130 x 10..110) is no longer the map's
    let taken = w.frame(&mut h, &[move_to(70.0, 60.0), left(true), left(false)]);
    assert_eq!(
        taken,
        vec![false, false, false],
        "the old area releases input"
    );
    assert_eq!(w.m.last_click, None, "and the map does not see the click");

    // the new area takes the pointer, and its click reaches the map
    let taken = w.frame(&mut h, &[move_to(350.0, 350.0), left(true), left(false)]);
    assert_eq!(
        taken,
        vec![true, true, true],
        "the press and its release stay with the map"
    );
    assert!(
        w.m.last_click.is_some(),
        "a click in the new area moves the camera"
    );
}

/// Terrain colours: the colour map is drawn into the rect with the UV window the C++ computes
/// from the bitmap size (gametacmap.cpp:54-55, 160-167). Read as the assignments are — the
/// `corners[3]` lines run first, so corners[0]/[2] keep `2/bmpWidth` — the window for a 128x128
/// map is `corners[0] = (2/128, 2/128)`, `corners[1] = (1, 2/128)`, `corners[2] = (2/128, 1)`,
/// `corners[3] = (1, 1)`, and the blend is `gos_Alpha_OneZero` (gametacmap.cpp:135).
#[test]
fn terrain_thumbnail_uses_the_cpp_uv_window() {
    let mut h = settled_harness();
    let mut m = tac_map();
    m.art.terrain = Some(
        h.assets
            .textures
            .insert("terrain.tga", 128, 128, vec![7; 128 * 128 * 4]),
    );
    let dl = h.render(|ctx| m.render(ctx, &input()));
    let b = &dl.batches[0];
    assert_eq!(b.blend, Blend::Opaque);
    assert_eq!(b.clip, Some(RECT));
    let q = quads(&dl)[0];
    let u0 = 2.0f32 / 128.0;
    // The C++'s corner table (gametacmap.cpp:154-167):
    //   corners[0] = (left, top, 2/bmpWidth, 2/bmpWidth)
    //   corners[1] = (right, top, 128/bmpWidth, 2/bmpWidth)
    //   corners[2] = (left, bottom, 2/bmpWidth, 128/bmpHeight)
    //   corners[3] = (right, bottom, 128/bmpWidth, 128/bmpHeight)
    // Drawn as two triangles (0,1,2) and (1,3,2); the draw list stores quads in its own
    // top-left/bottom-left/bottom-right/top-right order, i.e. [c0, c2, c3, c1].
    let (c0, c2, c3, c1) = (q[0], q[1], q[2], q[3]);
    assert_eq!((c0.x, c0.y), (10.0, 10.0));
    assert_eq!((c0.u, c0.v), (u0, u0));
    assert_eq!((c1.x, c1.y), (130.0, 10.0));
    assert_eq!((c1.u, c1.v), (1.0, u0));
    assert_eq!((c2.x, c2.y), (10.0, 110.0));
    assert_eq!((c2.u, c2.v), (u0, 1.0));
    assert_eq!((c3.x, c3.y), (130.0, 110.0));
    assert_eq!((c3.u, c3.v), (1.0, 1.0));
    for v in q {
        assert_eq!(v.argb, 0xffff_ffff); // corners[i].argb = 0xffffffff (gametacmap.cpp:146-152)
    }
}

/// A colour map of another size keeps the same normalised window: the C++ divides by the header's
/// width/height, so a 256-wide map starts at u = 2/256 (gametacmap.cpp:160-167).
#[test]
fn terrain_uv_window_follows_bitmap_size() {
    let mut h = settled_harness();
    let mut m = tac_map();
    m.set_terrain_size(256, 256);
    m.art.terrain = Some(
        h.assets
            .textures
            .insert("terrain.tga", 256, 256, vec![7; 256 * 256 * 4]),
    );
    let dl = h.render(|ctx| m.render(ctx, &input()));
    let q = quads(&dl)[0];
    let (c0, _c2, _c3, c1) = (q[0], q[1], q[2], q[3]);
    assert_eq!(c0.u, 2.0 / 256.0);
    assert_eq!(c1.u, 128.0 / 256.0);
}

/// Unit blips (`drawBlip`, gametacmap.cpp:513-551): the `blip.tga` quad at the unit —
/// `triangle[0].x -= 2.f`, `+4.1f` across, `triangle[0].y -= 2.f`, `+4.1f` down, UV
/// `0 .. 0.250001` (lines 540-548) — in the team colour, brighter when selected (lines 309-325,
/// 366-406).
#[test]
fn unit_blips_use_team_colours_and_selection() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![
        unit(TacTeam::Ours, (0.0, 0.0), false, 0.0),
        unit(TacTeam::Ours, (0.0, 0.0), true, 0.0),
        unit(TacTeam::Neutral, (0.0, 0.0), false, 0.0),
        unit(TacTeam::Enemy, (0.0, 0.0), true, 0.0),
        unit(TacTeam::PlayerAlt, (0.0, 0.0), false, 0.0),
    ];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let b = blips(&dl);
    assert_eq!(b.len(), 5, "one blip quad per unit");
    let colours: Vec<u32> = b.iter().map(|q| q.2).collect();
    // gametacmap.cpp:372 (ours), 396 (a neutral mover is foreign: the mover pass draws every
    // foreign contact red, not blue), 382 (other commander's units) in the unselected pass, then
    // 372 selected and 396 selected in the selected pass (lines 417-428: `bSel = 0` first, then
    // `bSel = 1`).
    assert_eq!(
        colours,
        vec![
            UNIT_COLOR_OURS,
            UNIT_COLOR_ENEMY,
            UNIT_COLOR_PLAYER_ALT,
            UNIT_COLOR_OURS_SELECTED,
            UNIT_COLOR_ENEMY_SELECTED,
        ]
    );
    // centre (70, 60) -> x0 = 70-2 = 68, y0 = 58, x1 = 68+4.1, y1 = 58+4.1
    assert_eq!(b[0].0, 68.0);
    assert_eq!(b[0].1, 58.0);
    assert!((b[0].3 - (68.0 + 4.1)).abs() < 1e-4, "{}", b[0].3);
    assert_eq!(*colours.last().unwrap(), UNIT_COLOR_ENEMY_SELECTED);
}

/// Selection brightens only the blip: `colorBlip` takes the `...selected` value while
/// `colorRing` stays the team colour (gametacmap.cpp:309-325 for the sensor pass, 372 and 382 for
/// the movers, which set `colorRing` to the plain team colour in every branch). A selected unit
/// with a sensor range draws its ring in the base colour, under a brighter blip.
#[test]
fn selected_units_keep_the_base_ring_colour() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![
        unit(TacTeam::Ours, (0.0, 0.0), true, 100.0),
        unit(TacTeam::Enemy, (0.0, 0.0), true, 100.0),
    ];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    // terrain, the view rect, then the two rings in unit order, then the two blips
    let q = quads(&dl);
    assert_eq!(q.len(), 6);
    assert_eq!(
        q[2][0].argb, UNIT_COLOR_OURS,
        "ring of the selected Ours unit"
    );
    assert_eq!(q[3][0].argb, UNIT_COLOR_ENEMY, "ring of the selected enemy");
    assert_eq!(q[4][0].argb, UNIT_COLOR_OURS_SELECTED, "selected Ours blip");
    assert_eq!(
        q[5][0].argb, UNIT_COLOR_ENEMY_SELECTED,
        "selected enemy blip"
    );
}

/// The blip colours, verbatim from the C++: `0xff00cc00`/`0xff4bff4b` (line 372),
/// `0xff0000cc`/`0xff4b4bff` (line 382), `0xff0000ff`/`0xff4c4cff` (line 315),
/// `0xffff0000`/`0xffff3f3f` (line 322).
#[test]
fn blip_colours_match_the_cpp_constants() {
    assert_eq!(UNIT_COLOR_OURS, 0xff00cc00);
    assert_eq!(UNIT_COLOR_OURS_SELECTED, 0xff4bff4b);
    assert_eq!(UNIT_COLOR_PLAYER_ALT, 0xff0000cc);
    assert_eq!(UNIT_COLOR_PLAYER_ALT_SELECTED, 0xff4b4bff);
    assert_eq!(UNIT_COLOR_NEUTRAL, 0xff0000ff);
    assert_eq!(UNIT_COLOR_NEUTRAL_SELECTED, 0xff4c4cff);
    assert_eq!(UNIT_COLOR_ENEMY, 0xffff0000);
    assert_eq!(UNIT_COLOR_ENEMY_SELECTED, 0xffff3f3f);
}

/// Sensor rings (`drawSensor`, gametacmap.cpp:465-511): the radius in metres gets the
/// `+= metersPerCell * 6` fudge above range 1 (lines 471-472), is scaled by
/// `(right-left) / (metersPerCell * realVerticesMapSide * 3)` (line 474) and becomes the
/// `ring.tga` ellipse of `EllipseElement` (mclib/cellip.cpp:24-53) centred on the blip's
/// `sqare[1]` pixel with radii `2 * trunc(radius + 0.5)` (gametacmap.cpp:498-501), clipped to the
/// tac-map rect (lines 504-507).
#[test]
fn sensor_ring_radius_matches_the_cpp_formula() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    // metersPerCell = (128/3) * 0.2 = 8.533333 (terrain.cpp:64,79). A 100 m sensor:
    // 100 + 8.533333*6 = 151.2 m; scale = 120 / (8.533333 * 100 * 3) = 0.046875 -> 7.0875 px;
    // radii = 2 * trunc(7.0875 + 0.5) = 2 * 7 = 14.
    inp.units = vec![unit(TacTeam::Ours, (0.0, 0.0), false, 100.0)];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let ring = batch_quad(&dl, 2); // terrain, the view rect, then the ring
                                   // center = (float2long(sqare[1].x), float2long(sqare[1].y)) with
                                   // sqare[1] = (sqare[0].x, sqare[0].y + 1) (gametacmap.cpp:484-487,
                                   // 496-497): x = 70.0, y + 1 = 61.0.
    assert_eq!(ring[0].x, 70.0 - 7.0); // cntr.x - ortho.x/2 (cellip.cpp:39-42)
    assert_eq!(ring[0].y, 61.0 - 7.0); // cntr.y - ortho.y/2
    assert_eq!(ring[2].x, 70.0 + 7.0);
    assert_eq!(ring[2].y, 61.0 + 7.0);
    assert_eq!(ring[0].argb, UNIT_COLOR_OURS); // the ring colour is the team colour (line 310)
    assert_eq!(ring[0].u, 0.0); // location[i].u = 0.f (cellip.cpp:31-32)
    assert_eq!(ring[2].u, 1.0); // location[3].u = location[2].u = 1.0f (cellip.cpp:45)
    assert_eq!(dl.batches[2].blend, Blend::Alpha); // gos_Alpha_AlphaInvAlpha (cellip.cpp:62)
    assert_eq!(
        dl.batches[2].clip,
        Some(RECT),
        "circle.setClip(rect) (gametacmap.cpp:504-506)"
    );
}

/// `ranges[]` is an `unsigned long` (gametacmap.cpp:277), filled from `getRange()` (lines 344 and
/// 405) and passed to `drawSensor` at line 415, so the ring is sized from the whole metres of the
/// range. Hand-derived for a 108.9 m sensor at the map centre on the 120 px rect:
///   range cut to 108 m (the `unsigned long` store), and 108 > 1, so radius += 8.533333 * 6 = 51.2
///   radius = 159.2; scale = 120 / (8.533333 * 100 * 3) = 0.046875; 159.2 * 0.046875 = 7.4625
///   radii = 2 * trunc(7.4625 + 0.5) = 2 * 7 = 14, so the ring spans 70 +- 7 (not 16 px: the
///   uncut 160.1 * 0.046875 = 7.5047 would give 2 * 8).
#[test]
fn sensor_range_is_cut_to_whole_metres_like_the_cpp_array() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![unit(TacTeam::Ours, (0.0, 0.0), false, 108.9)];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let ring = batch_quad(&dl, 2); // terrain, the view rect, then the ring
    assert_eq!(ring[0].x, 70.0 - 7.0);
    assert_eq!(ring[2].x, 70.0 + 7.0);
    assert_eq!(ring[0].y, 61.0 - 7.0);
    assert_eq!(ring[2].y, 61.0 + 7.0);
}

/// The draw order of gametacmap.cpp:413-428. The C++ array holds the non-mover sensor entries
/// first (lines 274-345), then the movers (347-406). Rings draw in array order, so a sensor
/// entry's ring is under a mover's. Blips draw in two passes: the first holds every sensor entry
/// and every unselected mover in array order, and a sensor entry is always unselected in that
/// pass (`selected[count] = 0`, line 345) whatever its owner's selection. So a selected sensor
/// draws under an unselected mover that comes before it in the input; the selected mover is last.
/// The brightness still follows the owner's selection (line 309).
#[test]
fn selected_sensor_entry_draws_before_an_unselected_mover() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![
        unit(TacTeam::Enemy, (0.0, 0.0), false, 100.0), // unselected mover, input first
        sensor_unit(TacTeam::Ours, (0.0, 0.0), true, 100.0), // selected sensor entry
        unit(TacTeam::Neutral, (0.0, 0.0), true, 0.0),  // selected mover
    ];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let q = quads(&dl);
    // terrain, the view rect, then the rings in array order: the sensor entry's, then the mover's
    assert_eq!(q[2][0].argb, UNIT_COLOR_OURS, "sensor entry ring first");
    assert_eq!(q[3][0].argb, UNIT_COLOR_ENEMY, "mover ring second");
    let colours: Vec<u32> = blips(&dl).iter().map(|b| b.2).collect();
    assert_eq!(
        colours,
        vec![
            UNIT_COLOR_OURS_SELECTED, // the selected sensor entry, in the unselected pass
            UNIT_COLOR_ENEMY,         // the unselected mover covers it
            UNIT_COLOR_ENEMY_SELECTED, // the selected mover (neutral, so red) is drawn last
        ]
    );
}

/// The artillery blink (`s_lastBlinkTime += frameLength; if (> s_blinkLength) colours = 0,
/// timer = 0`, gametacmap.cpp:332-338) runs inside the sensor pass of `GameTacMap::render`, once
/// per artillery sensor that gets past the pass's filters (range below 1.1 is skipped, line 316).
/// While the timer exceeds `s_blinkLength = 0.5` the artillery's colours are 0, which skips both
/// its draws (lines 467-468, 515-516), and the timer resets to 0. A unit with range 1.0 draws no
/// sensor ring and does not count.
#[test]
fn blinking_artillery_skips_its_draws() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![
        artillery_unit(TacTeam::Ours, (0.0, 0.0), false, 100.0),
        unit(TacTeam::Ours, (0.0, 0.0), false, 0.0),
    ];
    // below the blink length: terrain, view rect, the artillery ring, then both blips (the
    // Ours unit has no sensor range, so no ring)
    m.blink_time = 0.0;
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert_eq!(quads(&dl).len(), 5);
    assert_eq!(blips(&dl).len(), 2);

    // past s_blinkLength the artillery colours are 0: its ring and blip are skipped
    m.blink_time = 0.6;
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let q = quads(&dl);
    assert_eq!(q.len(), 3, "terrain + view rect + the plain unit's blip");
    assert_eq!(blips(&dl), vec![(68.0, 58.0, UNIT_COLOR_OURS, 68.0 + 4.1)]);
    // the C++ resets s_lastBlinkTime = 0.f when the blink fires (gametacmap.cpp:337)
    assert_eq!(m.blink_time, 0.0);
}

/// The timer runs only while artillery is drawn (gametacmap.cpp:329-331 sits in the artillery
/// branch of the sensor pass), so artillery that appears after a long time with none does not
/// blink at once. With `frameLength` 0.25 s (exact in f32): the first two frames are visible
/// (0.25, then 0.5, which is not `> 0.5`), the third blinks (0.75) and resets the timer, and the
/// fourth starts the cycle again.
#[test]
fn artillery_timer_runs_only_while_artillery_is_drawn() {
    let mut h = settled_harness();
    h.dt = 0.25;
    let mut m = tac_map();
    let mut inp = input();
    // 40 frames (10 s) with no artillery: the timer does not move
    for _ in 0..40 {
        h.render(|ctx| m.render(ctx, &inp));
    }
    assert_eq!(m.blink_time, 0.0, "no artillery, no timer");

    inp.units = vec![artillery_unit(TacTeam::Ours, (0.0, 0.0), false, 100.0)];
    let mut visible = Vec::new();
    for _ in 0..4 {
        let dl = h.render(|ctx| m.render(ctx, &inp));
        visible.push(blips(&dl).len());
    }
    assert_eq!(
        visible,
        vec![1, 1, 0, 1],
        "visible, visible, blink, visible"
    );
}

/// Each artillery sensor adds its own `frameLength` to the shared timer, in the order the
/// sensor pass visits them (gametacmap.cpp:329-338), so two artillery in one frame blink on the
/// second of them. With `frameLength` 0.25 s: frame 1 shows both (0.25, 0.5); frame 2's first
/// unit reaches 0.75 and blinks, resetting the timer, so the second unit reaches 0.25 and shows.
#[test]
fn each_artillery_sensor_advances_the_timer() {
    let mut h = settled_harness();
    h.dt = 0.25;
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![
        artillery_unit(TacTeam::Ours, (0.0, 0.0), false, 100.0),
        artillery_unit(TacTeam::Ours, (0.0, 0.0), false, 100.0),
    ];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert_eq!(blips(&dl).len(), 2, "0.25 then 0.5: neither exceeds 0.5");
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert_eq!(
        blips(&dl).len(),
        1,
        "0.75 blinks, the second unit shows at 0.25"
    );
}

/// The colour of a unit depends on its pass as well as its team (gametacmap.cpp:309-325 for the
/// non-mover sensors, 372-396 for the movers). A neutral owner is blue as a sensor entry
/// (line 315) but red as a mover (line 396, every foreign contact). Another commander's sensor is
/// green (the sensor pass only special-cases neutral and enemy, line 309) while its mover is dark
/// blue (line 382). Rings take the unselected colour in every case; selection brightens the blip.
#[test]
fn colours_follow_the_pass_as_well_as_the_team() {
    let cases = [
        // (team, sensor entry, selected, blip, ring)
        (TacTeam::Ours, true, false, UNIT_COLOR_OURS, UNIT_COLOR_OURS),
        (
            TacTeam::Ours,
            true,
            true,
            UNIT_COLOR_OURS_SELECTED,
            UNIT_COLOR_OURS,
        ),
        (
            TacTeam::PlayerAlt,
            true,
            false,
            UNIT_COLOR_OURS,
            UNIT_COLOR_OURS,
        ),
        (
            TacTeam::PlayerAlt,
            true,
            true,
            UNIT_COLOR_OURS_SELECTED,
            UNIT_COLOR_OURS,
        ),
        (
            TacTeam::Neutral,
            true,
            false,
            UNIT_COLOR_NEUTRAL,
            UNIT_COLOR_NEUTRAL,
        ),
        (
            TacTeam::Neutral,
            true,
            true,
            UNIT_COLOR_NEUTRAL_SELECTED,
            UNIT_COLOR_NEUTRAL,
        ),
        (
            TacTeam::Enemy,
            true,
            false,
            UNIT_COLOR_ENEMY,
            UNIT_COLOR_ENEMY,
        ),
        (
            TacTeam::Enemy,
            true,
            true,
            UNIT_COLOR_ENEMY_SELECTED,
            UNIT_COLOR_ENEMY,
        ),
        (
            TacTeam::Ours,
            false,
            false,
            UNIT_COLOR_OURS,
            UNIT_COLOR_OURS,
        ),
        (
            TacTeam::Ours,
            false,
            true,
            UNIT_COLOR_OURS_SELECTED,
            UNIT_COLOR_OURS,
        ),
        (
            TacTeam::PlayerAlt,
            false,
            false,
            UNIT_COLOR_PLAYER_ALT,
            UNIT_COLOR_PLAYER_ALT,
        ),
        (
            TacTeam::PlayerAlt,
            false,
            true,
            UNIT_COLOR_PLAYER_ALT_SELECTED,
            UNIT_COLOR_PLAYER_ALT,
        ),
        (
            TacTeam::Neutral,
            false,
            false,
            UNIT_COLOR_ENEMY,
            UNIT_COLOR_ENEMY,
        ),
        (
            TacTeam::Neutral,
            false,
            true,
            UNIT_COLOR_ENEMY_SELECTED,
            UNIT_COLOR_ENEMY,
        ),
        (
            TacTeam::Enemy,
            false,
            false,
            UNIT_COLOR_ENEMY,
            UNIT_COLOR_ENEMY,
        ),
        (
            TacTeam::Enemy,
            false,
            true,
            UNIT_COLOR_ENEMY_SELECTED,
            UNIT_COLOR_ENEMY,
        ),
    ];
    for (team, sensor_entry, selected, blip_colour, ring_colour) in cases {
        let h = settled_harness();
        let mut m = tac_map();
        let mut inp = input();
        let mut u = unit(team, (0.0, 0.0), selected, 100.0);
        u.sensor_entry = sensor_entry;
        inp.units = vec![u];
        let dl = h.render(|ctx| m.render(ctx, &inp));
        let label = format!("{team:?} sensor={sensor_entry} selected={selected}");
        assert_eq!(quads(&dl)[2][0].argb, ring_colour, "{label}: ring");
        assert_eq!(blips(&dl)[0].2, blip_colour, "{label}: blip");
    }
}

/// Artillery keeps its owner's team colour: the sensor pass colours it by owner team
/// (gametacmap.cpp:309-325), and the blink only zeroes both colours (lines 327-338). So a neutral
/// or enemy artillery sensor blinks in its own colour, not in the green of ours. With
/// `frameLength` 0.25 s and one artillery the cycle is the one of
/// `artillery_timer_runs_only_while_artillery_is_drawn`: shown, shown, blinked, shown. The ring
/// (cellip.cpp:24-53, drawn at quad index 2 after the terrain and the view rect) takes the team
/// colour too, and selection brightens only the blip.
#[test]
fn artillery_blinks_in_its_owners_team_colour() {
    let cases = [
        (
            TacTeam::Neutral,
            false,
            UNIT_COLOR_NEUTRAL,
            UNIT_COLOR_NEUTRAL,
        ),
        (
            TacTeam::Neutral,
            true,
            UNIT_COLOR_NEUTRAL_SELECTED,
            UNIT_COLOR_NEUTRAL,
        ),
        (TacTeam::Enemy, false, UNIT_COLOR_ENEMY, UNIT_COLOR_ENEMY),
        (
            TacTeam::Enemy,
            true,
            UNIT_COLOR_ENEMY_SELECTED,
            UNIT_COLOR_ENEMY,
        ),
    ];
    for (team, selected, blip_colour, ring_colour) in cases {
        let mut h = settled_harness();
        h.dt = 0.25;
        let mut m = tac_map();
        let mut inp = input();
        inp.units = vec![artillery_unit(team, (0.0, 0.0), selected, 100.0)];
        let mut shown = Vec::new();
        for _ in 0..4 {
            let dl = h.render(|ctx| m.render(ctx, &inp));
            let b = blips(&dl);
            if b.is_empty() {
                // blinked: no ring and no blip, only terrain and view rect
                assert_eq!(quads(&dl).len(), 2, "{team:?} selected={selected}: blink");
                shown.push(false);
            } else {
                assert_eq!(b.len(), 1);
                assert_eq!(b[0].2, blip_colour, "{team:?} selected={selected}: blip");
                assert_eq!(
                    quads(&dl)[2][0].argb,
                    ring_colour,
                    "{team:?} selected={selected}: ring"
                );
                shown.push(true);
            }
        }
        assert_eq!(
            shown,
            vec![true, true, false, true],
            "{team:?} selected={selected}: visible, visible, blink, visible"
        );
    }
}

/// Camera view rectangle (`viewingrect.tga` quad, gametacmap.cpp:212-272): the four corners are
/// the camera's ground corners mapped through `worldToTacMap`, drawn with the UVs of
/// gametacmap.cpp:249-252 —
///   corners[0].u = corners[1].u = 0.078125   corners[3].u = corners[2].u = 0.99875
///   corners[0].v = corners[3].v = 0.078125   corners[1].v = corners[2].v = 0.99875
/// and `gos_Alpha_AlphaInvAlpha` (line 255). The C++ fills `corners[0..4]` in the order
/// top-left, bottom-left, bottom-right, top-right (lines 217-243: `corners[1]`, `corners[2]` are
/// the lower screen corners, sampled at `nScreen.y = screenHeight * 0.6667f - 1`, line 221), so
/// [`TacMapInput::view_corners`] takes them in that order.
///
/// The corners are deliberately asymmetric, so a swapped order cannot pass. Expected pixels by
/// hand (worldToCell, terrain.h:346, then tacmap.cpp:35-40 with 120x100 at (10,10)):
///   TL (-3200, 3200): cells (75, 75)   -> (40, 35)
///   BL (-1600,-2400): cells (112, 206) -> (54.8, 78.67)
///   BR ( 3200,-2000): cells (225, 196) -> (100, 75.33)
///   TR ( 2400, 1600): cells (206, 112) -> (92.4, 47.33)
#[test]
fn view_rect_uses_the_cpp_uvs() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.view_corners = [
        (-3200.0, 3200.0),  // top-left
        (-1600.0, -2400.0), // bottom-left
        (3200.0, -2000.0),  // bottom-right
        (2400.0, 1600.0),   // top-right
    ];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let vr = quads(&dl)[1]; // terrain, then the view rect
    assert_eq!(dl.batches[1].blend, Blend::Alpha);
    assert_eq!(
        (vr[0].x, vr[0].y),
        (px(75.0, 120.0, 10.0), px(75.0, 100.0, 10.0)),
        "TL"
    );
    assert_eq!(
        (vr[1].x, vr[1].y),
        (px(112.0, 120.0, 10.0), px(206.0, 100.0, 10.0)),
        "BL"
    );
    assert_eq!(
        (vr[2].x, vr[2].y),
        (px(225.0, 120.0, 10.0), px(196.0, 100.0, 10.0)),
        "BR"
    );
    assert_eq!(
        (vr[3].x, vr[3].y),
        (px(206.0, 120.0, 10.0), px(112.0, 100.0, 10.0)),
        "TR"
    );
    assert_eq!(vr[0].u, 0.078125);
    assert_eq!(vr[1].u, 0.078125);
    assert_eq!(vr[2].u, 0.99875);
    assert_eq!(vr[3].u, 0.99875);
    assert_eq!(vr[0].v, 0.078125);
    assert_eq!(vr[1].v, 0.99875);
    assert_eq!(vr[2].v, 0.99875);
    assert_eq!(vr[3].v, 0.078125);
    for v in vr {
        assert_eq!(v.argb, 0xffff_ffff); // corners[i].argb = 0xffffffff (lines 245-248)
    }
}

/// `GameTacMap::render` never draws `buildingPoints` (the C++ keeps them from `initBuildings`,
/// gametacmap.cpp:438-463, and nothing in the render path reads them), so a nonempty building
/// packet changes no pixels versus an empty one.
#[test]
fn a_building_packet_changes_no_pixels() {
    let h = settled_harness();
    let mut with_packet = input();
    with_packet.units = vec![unit(TacTeam::Ours, (0.0, 0.0), true, 100.0)];
    assert!(!with_packet.buildings.is_empty());
    let mut without_packet = with_packet.clone();
    without_packet.buildings.clear();
    let mut m = tac_map();
    let drawn = h.render(|ctx| m.render(ctx, &with_packet));
    let mut m = tac_map();
    let empty = h.render(|ctx| m.render(ctx, &without_packet));
    assert_eq!(drawn.vertices, empty.vertices);
    assert_eq!(drawn.quad_count(), empty.quad_count());
}

/// `GameTacMap::render` returns while the terrain is not set up (`if (turn < 2)`,
/// gametacmap.cpp:130-131): with no bounds the widget draws nothing.
#[test]
fn nothing_draws_without_terrain_bounds() {
    let h = settled_harness();
    let mut m = TacMap::new(TerrainBounds {
        cells_per_side: 0,
        map_side: 0.0,
    });
    let dl = h.render(|ctx| m.render(ctx, &input()));
    assert_eq!(dl.quad_count(), 0);
}

/// The art the widget needs (`blip.tga`, `viewingrect.tga`, `ring.tga`) is optional: without it
/// the blips and rings fall back to flat quads so the map still shows the units.
#[test]
fn art_is_optional() {
    let h = settled_harness();
    let mut m = tac_map();
    let mut inp = input();
    inp.units = vec![unit(TacTeam::Ours, (0.0, 0.0), false, 100.0)];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert_eq!(
        dl.quad_count(),
        4,
        "terrain panel + view rect + ring + blip"
    );
    assert!(dl.batches.iter().all(|b| b.texture.is_none()));
}

/// With the art loaded the view rect, ring and blip quads carry their textures (the
/// `blipHandle` / `viewRectHandle` / ring texture of gametacmap.cpp:57-65 and cellip.cpp:103-108).
#[test]
fn art_textures_are_used_when_loaded() {
    let mut h = settled_harness();
    let blip = h
        .assets
        .textures
        .insert("data/art/blip.tga", 8, 8, vec![255; 8 * 8 * 4]);
    let view_rect =
        h.assets
            .textures
            .insert("data/art/viewingrect.tga", 8, 8, vec![255; 8 * 8 * 4]);
    let ring = h
        .assets
        .textures
        .insert("data/art/ring.tga", 8, 8, vec![255; 8 * 8 * 4]);
    let mut m = tac_map();
    m.art = TacMapArt {
        view_rect: Some(view_rect),
        blip: Some(blip),
        ring: Some(ring),
        ..TacMapArt::default()
    };
    let mut inp = input();
    inp.units = vec![unit(TacTeam::Ours, (0.0, 0.0), false, 100.0)];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    let tex: Vec<Option<mc2_ui::textures::TexId>> = dl.batches.iter().map(|b| b.texture).collect();
    assert_eq!(tex.len(), 4, "terrain, view rect, ring, blip");
    assert!(tex[0].is_none(), "no terrain thumbnail in this test");
    assert!(tex[1].is_some(), "the view rect uses viewingrect.tga");
    assert!(tex[2].is_some(), "the ring uses ring.tga");
    assert!(tex[3].is_some(), "the blip uses blip.tga");
}

/// The shipped mc2_01 tac map, from the mission `.pak` as the game reads it: the building packet
/// (packet 2) and the colour map (packet 3, `MissionInterfaceManager::initTacMap`,
/// missiongui.cpp:3085-3104), with the art from the VFS (gametacmap.cpp:57-65). Decodes the
/// packets and renders one frame with a synthetic unit list, checking that every building of the
/// packet is drawn. Skips cleanly without `MC2_DATA`.
#[test]
fn shipped_mc2_01_tac_map_decodes_and_renders() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("VFS");
    let mut assets = UiAssets::default();
    let mut art = TacMapArt::load(&mut assets.textures, &mut vfs);
    assert!(art.view_rect.is_some(), "viewingrect.tga");
    assert!(art.blip.is_some(), "blip.tga");
    assert!(art.ring.is_some(), "ring.tga");

    let pak_bytes = vfs
        .read("data/missions/mc2_01.pak")
        .expect("read the pak")
        .expect("mc2_01.pak is shipped");
    let pak = mc2_formats::pak::PakFile::from_bytes(pak_bytes).expect("pak format");
    // mc2_01 ships an empty building packet: the 4-byte count 0 (probed from the .pak; the C++
    // reads the same packet, missiongui.cpp:3085-3095)
    let building_packet = pak.read_packet(2).expect("building packet");
    assert_eq!(
        building_packet,
        vec![0, 0, 0, 0],
        "mc2_01 has no tactical buildings"
    );
    let buildings = parse_buildings(&building_packet);
    let colmap = pak.read_packet(3).expect("colour map packet");
    let id = art
        .set_terrain_packet(&mut assets.textures, "mc2_01colmap", &colmap)
        .expect("colour map TGA");
    assert_eq!(art.terrain, Some(id));
    assert!(art.terrain_size.0 > 0 && art.terrain_size.1 > 0);

    // the mission's terrain and the Rect0 of buttonlayout800.fit (controlgui.cpp:2634)
    let mut m = TacMap::new(TerrainBounds::from_vertices(100));
    m.set_pos(GuiRect::new(4, 435, 165, 596));
    m.art = art;
    let inp = TacMapInput {
        buildings,
        units: vec![unit(TacTeam::Ours, (0.0, 0.0), true, 100.0)],
        objectives: vec![],
        objective_total: 0,
        view_corners: [
            (800.0, 2400.0),
            (800.0, 0.0),
            (3200.0, 0.0),
            (3200.0, 2400.0),
        ],
        placing_airstrike: false,
    };
    let h = Harness::with_assets(assets);
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert_eq!(blips(&dl).len(), 1, "the synthetic unit");
}

/// `GameTacMap::initBuildings` (gametacmap.cpp:438-463): the count, then the rect-relative pixel
/// pairs. Whole entries decode in order.
#[test]
fn building_packet_decodes_its_pairs() {
    let mut packet = Vec::new();
    for w in [2i32, 1, 2, 30, -4] {
        packet.extend_from_slice(&w.to_le_bytes());
    }
    assert_eq!(parse_buildings(&packet), vec![(1, 2), (30, -4)]);
}

/// A packet that ends inside an entry keeps the complete entries before it (the example's old
/// length check read one word fewer than it used, so a truncated packet could panic). An empty or
/// one-word packet has no entries.
#[test]
fn truncated_building_packet_keeps_whole_entries() {
    let mut packet = Vec::new();
    for w in [3i32, 1, 2, 30] {
        packet.extend_from_slice(&w.to_le_bytes());
    }
    assert_eq!(
        parse_buildings(&packet),
        vec![(1, 2)],
        "the second entry is cut off"
    );
    assert_eq!(parse_buildings(&[]), Vec::new());
    assert_eq!(parse_buildings(&2i32.to_le_bytes()), Vec::new());
}

/// A tiny synthetic colour-map packet (an 8x8 TGA) sizes the UV window from its header, like the
/// mission packet does.
#[test]
fn terrain_packet_sizes_the_uv_window() {
    // an uncompressed 8x8 32-bit TGA: 18-byte header + 8*8*4 pixels
    let mut tga = vec![0u8; 18 + 8 * 8 * 4];
    tga[2] = 2; // 32-bit RGB(A)
    tga[12] = 8; // width
    tga[14] = 8; // height
    tga[16] = 32; // depth
    let mut assets = UiAssets::default();
    let mut art = TacMapArt::default();
    let id = art.set_terrain_packet(&mut assets.textures, "colmap", &tga);
    assert!(id.is_some());
    assert_eq!(art.terrain_size, (8, 8));
    assert_eq!(art.terrain, id);
}

/// A colour-map packet that does not decode leaves no thumbnail: `set_terrain_packet` returns
/// `None` and the size `(0, 0)` (its documented failure state), even when an earlier packet
/// succeeded. A reused [`TacMapArt`] therefore cannot draw the previous mission's map; the widget
/// falls back to its flat panel.
#[test]
fn failed_terrain_packet_clears_the_previous_thumbnail() {
    let mut tga = vec![0u8; 18 + 8 * 8 * 4];
    tga[2] = 2;
    tga[12] = 8;
    tga[14] = 8;
    tga[16] = 32;
    let mut assets = UiAssets::default();
    let mut art = TacMapArt::default();
    assert!(art
        .set_terrain_packet(&mut assets.textures, "mission_a", &tga)
        .is_some());
    assert_eq!(art.terrain_size, (8, 8));

    let bad = art.set_terrain_packet(&mut assets.textures, "mission_b", &[1, 2, 3]);
    assert_eq!(bad, None, "three bytes are not a TGA");
    assert_eq!(art.terrain, None, "the old thumbnail is gone");
    assert_eq!(art.terrain_size, (0, 0), "and its size with it");

    let mut m = tac_map();
    m.art = art;
    let inp = input();
    let h = Harness::with_assets(assets);
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert!(
        dl.batches.iter().all(|b| b.texture.is_none()),
        "nothing draws the failed packet's texture"
    );
}

/// `CObjective::RenderMarkers` (objective.cpp:2290-2301): the numeral is `drawShadowText(0xffffffff,
/// 0xff000000, ..., pos.x - width/2, pos.y - height/2, ..., -2, 2)` at the objective's tac-map
/// position, so the black shadow quad sits two pixels left and down of the white one, and the
/// text is centred on `worldToTacMap(marker)`. Skips cleanly without `MC2_DATA`.
#[test]
fn objective_numeral_is_drawn_white_over_a_black_shadow_at_the_marker() {
    use mc2_ui::tacmap::{TacObjective, OBJECTIVE_MARKER_FONT_ID};
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("VFS");
    let mut assets = UiAssets::default();
    let font = mc2_ui::ctx::Loader::new(&mut vfs, &mut assets).font(OBJECTIVE_MARKER_FONT_ID);
    assert!(font.id.is_some(), "arialBlack9.d3f");
    let mut m = tac_map();
    m.art.marker_font = font;
    let mut inp = input();
    let h = Harness::with_assets(assets);
    let base = h.render(|ctx| m.render(ctx, &inp)).vertices.len();
    inp.objectives = vec![TacObjective {
        index: 0,
        pos: (0.0, 0.0),
        text: "1".to_string(),
    }];
    let dl = h.render(|ctx| m.render(ctx, &inp));
    assert_eq!(dl.vertices.len(), base + 8, "one glyph twice");
    // after the terrain quad and before the view rect: the shadow quad, then the text quad
    let glyphs = &dl.vertices[4..12];
    let (shadow, text) = (&glyphs[..4], &glyphs[4..]);
    assert!(shadow.iter().all(|v| v.argb == 0xff00_0000));
    assert!(text.iter().all(|v| v.argb == 0xffff_ffff));
    assert_eq!(text[0].x - shadow[0].x, 2.0);
    assert_eq!(shadow[0].y - text[0].y, 2.0);
    // the map centre (0, 0) is tac (70, 60) in this fixture (see `world_to_tac_map_centre`)
    let (cx, cy) = ((text[0].x + text[2].x) / 2.0, (text[0].y + text[2].y) / 2.0);
    assert!(
        (cx - 70.0).abs() <= 3.0 && (cy - 60.0).abs() <= 6.0,
        "{cx},{cy}"
    );
}

/// `GameTacMap::animate` and the objective loop of `GameTacMap::render` (gametacmap.cpp:114-126,
/// 184-205): the flashed objective (`abs(buttonId) - 1`) is hidden for the first quarter second of
/// each half-second flash, shown until 0.5 s, hidden on the frame the flash wraps, and draws
/// steadily once the last flash ends; the other marker draws throughout. Skips without `MC2_DATA`.
#[test]
fn objective_flash_hides_and_shows_only_the_flashed_marker() {
    use mc2_ui::tacmap::{TacObjective, OBJECTIVE_MARKER_FONT_ID};
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("VFS");
    let mut assets = UiAssets::default();
    let font = mc2_ui::ctx::Loader::new(&mut vfs, &mut assets).font(OBJECTIVE_MARKER_FONT_ID);
    let mut m = tac_map();
    m.art.marker_font = font;
    let mut inp = input();
    inp.objectives = vec![
        TacObjective {
            index: 0,
            pos: (0.0, 0.0),
            text: "1".to_string(),
        },
        TacObjective {
            index: 1,
            pos: (500.0, 500.0),
            text: "2".to_string(),
        },
    ];
    inp.objective_total = 2;
    let mut h = Harness::with_assets(assets);
    h.dt = 0.125; // exact in binary, so the 0.25 / 0.5 thresholds fall on whole frames
    let glyphs = |m: &mut TacMap, h: &Harness, inp: &TacMapInput| {
        h.render(|ctx| m.render(ctx, inp)).vertices.len()
    };
    let steady = glyphs(&mut m, &h, &inp);
    let bare = {
        let mut none = inp.clone();
        none.objectives.clear();
        glyphs(&mut m, &h, &none)
    };
    assert_eq!(steady, bare + 16, "two markers, shadow and text each");
    // `ControlGui::animateTacMap(-2, ..)`: objective id 2 flashes the second objective, twice
    assert!(m.animate_objective(2, 2));
    assert!(
        !m.animate_objective(1, 5),
        "only the first animate takes effect"
    );
    // per frame: (flash time) .125 hidden, .25 hidden, .375 shown, .5 shown, .625 wraps (hidden),
    // then the second flash the same, and the frame it ends steady again
    let drawn = [
        false, false, true, true, false, false, false, true, true, false, true, true,
    ];
    for (frame, &shown) in drawn.iter().enumerate() {
        let want = if shown { steady } else { steady - 8 };
        assert_eq!(glyphs(&mut m, &h, &inp), want, "frame {frame}");
    }
}

#[test]
fn objective_flash_returns_to_idle_so_later_flashes_start() {
    let mut m = tac_map();
    assert!(
        m.animate_objective(2, 0),
        "a zero-flash request is accepted"
    );
    assert!(m.animate_objective(2, 1), "...and does not lock the state");
}
