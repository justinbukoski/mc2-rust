// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! In-mission selection and click orders (`code/missiongui.cpp`).
//!
//! Expected values are hand-derived from the C++; each test cites the lines it follows. Units are
//! plain [`UnitView`]s with screen points given directly (identity projection unless a test says
//! otherwise). The order cases use the AOE mouse profile, the default (`mechcmd2.cpp:106`).

use mc2_platform::input::KeyStatus;
use mc2_platform::keys::{KEY_E, KEY_F, KEY_LCONTROL, KEY_LSHIFT};
use mc2_ui::mission_keys::{
    ControlStyle, Controls, DispatchContext, ForceGroup, HotKeys, KeyFrame, MissionCommand,
    Relation, SelectedMover, Target,
};
use mc2_ui::mission_select::{
    force_group_changes, frame, in_rect, make_force_group, select_visible, waypoint_key_down,
    ClickContext, ClickTarget, DragInput, InfoWindow, OrderKind, Resolution, SelectState,
    SelectionIntent, UnitView, MAX_BOX,
};

const POINT: [f32; 3] = [100.0, 200.0, 0.0];

fn unit(id: u32, x: f32, y: f32) -> UnitView {
    UnitView {
        id,
        team_home: true,
        in_view: true,
        screen: [x, y],
        own: true,
        on_gui: true,
        ..UnitView::default()
    }
}

fn identity(w: [f32; 3]) -> [f32; 2] {
    [w[0], w[1]]
}

fn idle() -> DragInput {
    drag(KeyStatus::Free, [0.0; 3], [0.0; 2])
}

fn drag(status: KeyStatus, world: [f32; 3], mouse: [f32; 2]) -> DragInput {
    DragInput {
        status,
        gui: false,
        camera: false,
        shift: false,
        world,
        mouse,
    }
}

/// The default context for a click on open ground in the AOE profile.
fn ground() -> ClickContext {
    ClickContext {
        terrain_select: true,
        passable: true,
        point: POINT,
        ..ClickContext::default()
    }
}

/// A context for a click on a mover (`Target` with `is_mover`).
fn on(relation: Relation, own: bool) -> ClickContext {
    ClickContext {
        target: Some(ClickTarget {
            id: 7,
            facts: Target {
                is_mover: true,
                relation,
                ..Target::default()
            },
            home_team: relation == Relation::Friendly && own,
            own,
            ..ClickTarget::default()
        }),
        ..ground()
    }
}

fn run(
    state: &mut SelectState,
    input: DragInput,
    units: &[UnitView],
    ctx: &ClickContext,
) -> Resolution {
    frame(state, input, identity, units, ctx)
}

/// A drag from `(40, 40)` to `(120, 120)` with the box corner `(160, 160)` released. The start
/// point differs from the held point on both axes, so the drag starts (missiongui.cpp:3569-3573).
fn box_drag_frames(state: &mut SelectState, units: &[UnitView], shift: bool) -> Resolution {
    let mut press = drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]);
    press.shift = shift;
    run(state, press, units, &ground());
    run(
        state,
        drag(KeyStatus::Held, [120.0, 120.0, 0.0], [120.0, 120.0]),
        units,
        &ground(),
    );
    let mut release = drag(KeyStatus::Released, [160.0, 160.0, 0.0], [160.0, 160.0]);
    release.shift = shift;
    run(state, release, units, &ground())
}

// ---------------------------------------------------------------------------------------------
// Drag-box membership (moverInRect, objmgr.cpp:2384-2411; doDrag, missiongui.cpp:3554-3644)

#[test]
fn in_rect_includes_both_edges() {
    // objmgr.cpp:2410-2413: `left <= x && x <= right` and the same for y.
    assert!(in_rect([10.0, 20.0], [30.0, 40.0], [10.0, 20.0]));
    assert!(in_rect([10.0, 20.0], [30.0, 40.0], [30.0, 40.0]));
    assert!(!in_rect([10.0, 20.0], [30.0, 40.0], [9.9, 20.0]));
    assert!(!in_rect([10.0, 20.0], [30.0, 40.0], [30.5, 40.0]));
}

#[test]
fn in_rect_truncates_corners_toward_zero() {
    // objmgr.cpp:2405-2408: the corners are `long`s, so 10.9 -> 10 and 30.2 -> 30, 40.8 -> 40.
    let (start, end) = ([10.9, 20.0], [30.2, 40.8]);
    assert!(in_rect(start, end, [10.0, 40.0]));
    assert!(in_rect(start, end, [10.5, 20.0]));
    assert!(!in_rect(start, end, [9.99, 20.0]));
    assert!(!in_rect(start, end, [30.9, 20.0]));
    // -2.7 -> -2 (toward zero, not floor): x = -2.5 is outside, x = -2.0 is inside.
    assert!(in_rect([-2.7, 0.0], [0.0, 0.0], [-2.0, 0.0]));
    assert!(!in_rect([-2.7, 0.0], [0.0, 0.0], [-2.5, 0.0]));
}

#[test]
fn in_rect_ignores_corner_order() {
    // objmgr.cpp:2405-2408 takes fmin/fmax, so a drag up and left matches the same box.
    let p = [50.0, 50.0];
    assert_eq!(
        in_rect([160.0, 160.0], [40.0, 40.0], p),
        in_rect([40.0, 40.0], [160.0, 160.0], p)
    );
    assert!(in_rect([160.0, 160.0], [40.0, 40.0], p));
}

#[test]
fn drag_selects_units_inside_the_box_on_release() {
    // Units 1 (50,50), 2 (150,150), 3 (250,50) out, 4 (100,100). Box (40,40)-(160,160).
    let units = [
        unit(1, 50.0, 50.0),
        unit(2, 150.0, 150.0),
        unit(3, 250.0, 50.0),
        unit(4, 100.0, 100.0),
    ];
    let mut state = SelectState::default();
    let res = box_drag_frames(&mut state, &units, false);
    // doDrag 3583-3637: everything in the box is selected and the rest deselected (`!shift`).
    assert_eq!(
        res.selection,
        SelectionIntent::Box {
            ids: vec![1, 2, 4],
            replace: true
        }
    );
    assert!(!state.drag.dragging());
}

#[test]
fn drag_needs_movement_on_both_axes() {
    // missiongui.cpp:3569-3571 uses `&&`: a move along x alone never starts a drag.
    let units = [unit(1, 50.0, 50.0)];
    let mut state = SelectState::default();
    run(
        &mut state,
        drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]),
        &units,
        &ground(),
    );
    run(
        &mut state,
        drag(KeyStatus::Held, [120.0, 40.0, 0.0], [120.0, 40.0]),
        &units,
        &ground(),
    );
    assert!(!state.drag.dragging());
    let res = run(
        &mut state,
        drag(KeyStatus::Released, [120.0, 40.0, 0.0], [120.0, 40.0]),
        &units,
        &ground(),
    );
    assert_eq!(res.selection, SelectionIntent::Unchanged);
}

#[test]
fn press_and_release_without_movement_is_not_a_box() {
    // No drag starts, so the release is a plain click (dragStart zeroed, missiongui.cpp:3640-3642).
    let units = [unit(1, 50.0, 50.0)];
    let mut state = SelectState::default();
    run(
        &mut state,
        drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]),
        &units,
        &ground(),
    );
    let res = run(
        &mut state,
        drag(KeyStatus::Released, [40.0, 40.0, 0.0], [40.0, 40.0]),
        &units,
        &ground(),
    );
    assert_eq!(res.selection, SelectionIntent::Unchanged);
}

#[test]
fn drag_selects_at_most_sixteen_units() {
    // MAX_ICONS (forcegroupbar.h:21) caps `objsInRect` (missiongui.cpp:3605: count < MAX_ICONS).
    let units: Vec<UnitView> = (0..20).map(|i| unit(i, 50.0 + i as f32, 60.0)).collect();
    let mut state = SelectState::default();
    let res = box_drag_frames(&mut state, &units, false);
    let SelectionIntent::Box { ids, .. } = res.selection else {
        panic!("expected a box, got {:?}", res.selection);
    };
    assert_eq!(ids.len(), MAX_BOX);
    assert_eq!(
        ids,
        (0..16).collect::<Vec<u32>>(),
        "the first sixteen in roster order"
    );
}

#[test]
fn shift_drag_adds_instead_of_replacing() {
    // missiongui.cpp:3618-3622: the deselect loop runs only when `!userInput->shift()`.
    let units = [unit(1, 50.0, 50.0)];
    let mut state = SelectState::default();
    let res = box_drag_frames(&mut state, &units, true);
    assert_eq!(
        res.selection,
        SelectionIntent::Box {
            ids: vec![1],
            replace: false
        }
    );
}

#[test]
fn drag_skips_enemy_disabled_and_other_team_units() {
    // Own enemy-commanded unit (own false), disabled unit (3603-3605), and a unit not on the home
    // team (moverInRect's `getTeam() == Team::home`, objmgr.cpp:2394).
    let mut enemy = unit(1, 50.0, 50.0);
    enemy.own = false;
    let mut dead = unit(2, 60.0, 60.0);
    dead.disabled = true;
    let mut other_team = unit(3, 70.0, 70.0);
    other_team.team_home = false;
    let units = [enemy, dead, other_team, unit(4, 80.0, 80.0)];
    let mut state = SelectState::default();
    let res = box_drag_frames(&mut state, &units, false);
    assert_eq!(
        res.selection,
        SelectionIntent::Box {
            ids: vec![4],
            replace: true
        }
    );
}

#[test]
fn empty_box_changes_nothing() {
    // missiongui.cpp:3613 `if (count)`: a box around nothing leaves the selection alone.
    let units = [unit(1, 500.0, 500.0)];
    let mut state = SelectState::default();
    let res = box_drag_frames(&mut state, &units, false);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert!(!state.drag.dragging());
}

#[test]
fn drag_box_is_projected_from_the_world_start() {
    // missiongui.cpp:3598-3603: the start is `eye->projectZ(dragStart)`; the end is the mouse.
    // The projection here shifts by +100 in x, so the box is (140,40)-(160,160) on screen.
    let units = [unit(1, 50.0, 50.0), unit(2, 150.0, 50.0)];
    let shifted = |w: [f32; 3]| [w[0] + 100.0, w[1]];
    let mut state = SelectState::default();
    frame(
        &mut state,
        drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [0.0, 0.0]),
        shifted,
        &units,
        &ground(),
    );
    frame(
        &mut state,
        drag(KeyStatus::Held, [120.0, 120.0, 0.0], [120.0, 120.0]),
        shifted,
        &units,
        &ground(),
    );
    let res = frame(
        &mut state,
        drag(KeyStatus::Released, [160.0, 160.0, 0.0], [160.0, 160.0]),
        shifted,
        &units,
        &ground(),
    );
    assert_eq!(
        res.selection,
        SelectionIntent::Box {
            ids: vec![2],
            replace: true
        }
    );
}

#[test]
fn gui_press_does_not_start_a_drag() {
    // missiongui.cpp:3559: a press over the control panel (`bGui`) records no drag start.
    let units = [unit(1, 50.0, 50.0)];
    let mut state = SelectState::default();
    let mut press = drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]);
    press.gui = true;
    run(&mut state, press, &units, &ground());
    run(
        &mut state,
        drag(KeyStatus::Held, [120.0, 120.0, 0.0], [120.0, 120.0]),
        &units,
        &ground(),
    );
    assert!(!state.drag.dragging());
}

#[test]
fn box_info_follows_the_unit_count() {
    // missiongui.cpp:3613-3634: the info window clears, then shows a lone unit.
    let one = [unit(1, 50.0, 50.0)];
    let mut state = SelectState::default();
    assert_eq!(
        box_drag_frames(&mut state, &one, false).info,
        InfoWindow::Set(1)
    );

    let two = [unit(1, 50.0, 50.0), unit(2, 60.0, 60.0)];
    let mut state = SelectState::default();
    assert_eq!(
        box_drag_frames(&mut state, &two, false).info,
        InfoWindow::Clear
    );
}

// ---------------------------------------------------------------------------------------------
// Select visible (selectVisible / addVisibleToSelection, missiongui.cpp:2077-2115)

#[test]
fn select_visible_takes_every_own_enabled_unit_on_screen_without_a_cap() {
    // The screen is 1024x768. Twenty own units on screen; one off screen, one disabled, one enemy.
    let mut units: Vec<UnitView> = (0..20).map(|i| unit(i, 10.0 + i as f32, 10.0)).collect();
    units.push(unit(100, 1100.0, 10.0));
    let mut dead = unit(101, 20.0, 20.0);
    dead.disabled = true;
    units.push(dead);
    let mut enemy = unit(102, 30.0, 30.0);
    enemy.own = false;
    units.push(enemy);
    let res = select_visible(&mut SelectState::default(), &units, [1024.0, 768.0], true);
    // addVisibleToSelection has no MAX_ICONS cap (2093-2115): all twenty come back.
    assert_eq!(
        res,
        SelectionIntent::Visible {
            ids: (0..20).collect(),
            clear_home: true
        }
    );
}

#[test]
fn select_visible_includes_the_screen_edge_and_add_keeps_the_selection() {
    // The box is [0, w] x [0, h] inclusive (2093-2115 with dragStart zeroed and dragEnd the res).
    let units = [unit(1, 1024.0, 768.0), unit(2, 0.0, 0.0)];
    let res = select_visible(&mut SelectState::default(), &units, [1024.0, 768.0], false);
    assert_eq!(
        res,
        SelectionIntent::Visible {
            ids: vec![1, 2],
            clear_home: false
        }
    );
}

#[test]
fn select_visible_spends_a_pending_press() {
    // missiongui.cpp:2097 and 2110: addVisibleToSelection zeroes dragStart and dragEnd. A press
    // made before the key, still held and then released, must not become a box (3569-3573 starts
    // a drag only from a nonzero dragStart; 3583-3628 would select the unit at (100, 100)).
    let units = [unit(1, 100.0, 100.0)];
    let mut state = SelectState::default();
    run(
        &mut state,
        drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]),
        &units,
        &ground(),
    );
    select_visible(&mut state, &units, [1024.0, 768.0], false);
    run(
        &mut state,
        drag(KeyStatus::Held, [120.0, 120.0, 0.0], [120.0, 120.0]),
        &units,
        &ground(),
    );
    let res = run(
        &mut state,
        drag(KeyStatus::Released, [160.0, 160.0, 0.0], [160.0, 160.0]),
        &units,
        &ground(),
    );
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert!(!state.drag.dragging());
}

// ---------------------------------------------------------------------------------------------
// Click, shift-click and double-click (updateAOEStyle 1589-1667; update 762-824, 853, 906-907)

#[test]
fn left_click_on_own_mover_selects_only_it() {
    // missiongui.cpp:1647-1658: deselect every home mover, then select the target.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.selection, SelectionIntent::SelectOnly(7));
    // missiongui.cpp:1617-1621: a home mover sets the info window.
    assert_eq!(res.info, InfoWindow::Set(7));
    assert_eq!(res.order.kind, OrderKind::None);
}

#[test]
fn shift_click_toggles_an_own_mover() {
    // missiongui.cpp:1630-1645: an unselected mover is added, a selected one is removed.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_release = true;
    ctx.shift = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(
        res.selection,
        SelectionIntent::Toggle {
            id: 7,
            select: true
        }
    );

    ctx.target.as_mut().unwrap().selected = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(
        res.selection,
        SelectionIntent::Toggle {
            id: 7,
            select: false
        }
    );
}

#[test]
fn click_on_an_enemy_selects_nothing_but_shows_it_when_in_contact() {
    // 1647-1658: only own objects are selected; 1617-1621: a visible enemy mover feeds the info window.
    let mut ctx = on(Relation::Enemy, false);
    ctx.left_release = true;
    ctx.target.as_mut().unwrap().visible_contact = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert_eq!(res.info, InfoWindow::Set(7));
}

#[test]
fn click_on_empty_ground_is_refused_and_cancels_the_info_pick() {
    // missiongui.cpp:1663-1667: cancelInfo and the invalid sound; the selection stays.
    let mut ctx = ground();
    ctx.left_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.selection, SelectionIntent::Denied);
    assert_eq!(res.info, InfoWindow::CancelPick);
}

#[test]
fn double_click_on_a_grouped_mover_selects_its_force_group() {
    // missiongui.cpp:808-824: the first group the target belongs to (0..10), exclusively.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_double = true;
    ctx.left_down = true;
    ctx.target.as_mut().unwrap().home_team = true;
    ctx.target.as_mut().unwrap().groups = (1 << 2) | (1 << 5);
    let mut state = SelectState::default();
    let res = run(&mut state, idle(), &[], &ctx);
    assert_eq!(
        res.selection,
        SelectionIntent::Groups(ForceGroup::Select {
            group: 2,
            exclusive: true
        })
    );
    assert!(state.last_double());
}

#[test]
fn release_after_a_double_click_does_not_select() {
    // missiongui.cpp:853 and 906-907: the double-click latch suppresses the next click select
    // until the button is up again.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_double = true;
    let mut state = SelectState::default();
    run(&mut state, idle(), &[], &ctx);
    assert!(state.last_double());

    let mut next = on(Relation::Friendly, true);
    next.left_release = true;
    let res = run(&mut state, idle(), &[], &next);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert!(!state.last_double(), "the latch clears with the button up");
}

#[test]
fn double_click_on_an_ungrouped_mover_only_latches() {
    // missiongui.cpp:808-824: the latch is set even when no force group is found.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_double = true;
    ctx.target.as_mut().unwrap().home_team = true;
    let mut state = SelectState::default();
    let res = run(&mut state, idle(), &[], &ctx);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert!(state.last_double());
}

#[test]
fn left_click_is_suppressed_on_the_release_of_a_drag() {
    // missiongui.cpp:853 reads isDragging before doDrag (3469) clears it on the release frame.
    let units = [unit(1, 50.0, 50.0)];
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_release = true;
    let mut state = SelectState::default();
    run(
        &mut state,
        drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]),
        &units,
        &ctx,
    );
    run(
        &mut state,
        drag(KeyStatus::Held, [120.0, 120.0, 0.0], [120.0, 120.0]),
        &units,
        &ctx,
    );
    let res = run(
        &mut state,
        drag(KeyStatus::Released, [160.0, 160.0, 0.0], [160.0, 160.0]),
        &units,
        &ctx,
    );
    assert_eq!(
        res.selection,
        SelectionIntent::Box {
            ids: vec![1],
            replace: true
        }
    );
}

#[test]
fn terrain_key_suppresses_the_select_click() {
    // missiongui.cpp:853: `!getKeyDown(KEY_T)`.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_release = true;
    ctx.key_t = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
}

// ---------------------------------------------------------------------------------------------
// Click -> OrderIntent (updateAOEStyle right click, 1514-1586; doAttack 1801; doMove 3130)

fn right(mut ctx: ClickContext) -> Resolution {
    ctx.right_release = true;
    run(&mut SelectState::default(), idle(), &[], &ctx)
}

#[test]
fn right_click_on_open_ground_moves_at_run_speed() {
    // updateAOEStyle 1557 -> doMove (3130-3170): run, TRAVEL_MODE_FAST, not queued.
    let res = right(ground());
    assert_eq!(
        res.order.kind,
        OrderKind::MoveTo {
            point: POINT,
            walk: false
        }
    );
    assert!(!res.order.queued);
}

#[test]
fn right_click_on_open_ground_walks_when_walk_is_set() {
    // doMove 3163: `getWalk() ? TRAVEL_MODE_SLOW : TRAVEL_MODE_FAST`.
    let mut ctx = ground();
    ctx.walk = true;
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::MoveTo {
            point: POINT,
            walk: true
        }
    );
}

#[test]
fn right_click_with_the_waypoint_key_queues_the_move() {
    // handleOrders 2513-2516: with WAYPOINT_KEY held the order is queued, not executed.
    let mut ctx = ground();
    ctx.waypoint_key_down = true;
    let res = right(ctx);
    assert!(matches!(res.order.kind, OrderKind::MoveTo { .. }));
    assert!(res.order.queued);
}

#[test]
fn sensor_strike_click_while_painting_a_vtol_is_unavailable_not_a_move_or_jump() {
    // The airstrike branch is skipped while painting a VTOL (1533), so the click reaches doMove,
    // whose first test sends addAirstrike when the sensor probe is held (3137-3146), before jump
    // (3154) or move. The sensor strike is not ported: it must not become a move or a jump.
    let mut ctx = ground();
    ctx.painting_vtol = true;
    ctx.controls = Controls {
        adding_airstrike: true,
        sensor_probe_held: true,
        ..Controls::default()
    };
    ctx.can_jump_to_point = true;
    assert!(matches!(
        right(ctx.clone()).order.kind,
        OrderKind::Unavailable(_)
    ));
    ctx.controls.jump = true;
    assert!(matches!(right(ctx).order.kind, OrderKind::Unavailable(_)));
}

#[test]
fn right_click_with_jump_on_jumps_when_the_point_is_reachable() {
    // doMove 3154 -> doJump (1969-2014): TACTICAL_ORDER_JUMPTO_POINT at wPos.
    let mut ctx = ground();
    ctx.controls = Controls {
        jump: true,
        ..Controls::default()
    };
    ctx.can_jump_to_point = true;
    assert_eq!(right(ctx).order.kind, OrderKind::Jump { point: POINT });
}

#[test]
fn right_click_with_jump_on_is_refused_when_the_point_is_not_reachable() {
    // doJump 1983-2002: not canJumpToWPos -> cursor DONT and the invalid sound.
    let mut ctx = ground();
    ctx.controls = Controls {
        jump: true,
        ..Controls::default()
    };
    assert_eq!(right(ctx).order.kind, OrderKind::Denied);
}

#[test]
fn right_click_in_guard_mode_guards_the_point() {
    // updateAOEStyle 1551-1554: doGuard(NULL), a run guard on wPos (2030-2058).
    let mut ctx = ground();
    ctx.controls = Controls {
        guard: true,
        ..Controls::default()
    };
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::GuardPoint {
            point: POINT,
            walk: false
        }
    );
}

#[test]
fn right_click_on_impassable_ground_gives_no_order() {
    // updateAOEStyle 1559-1561: the cursor goes to DONT; no order and no sound.
    let mut ctx = ground();
    ctx.passable = false;
    assert_eq!(right(ctx).order.kind, OrderKind::None);
}

#[test]
fn right_click_off_the_game_terrain_gives_no_order() {
    // updateAOEStyle 1565-1566: not IsGameSelectTerrainPosition -> cursor DONT only.
    let mut ctx = ground();
    ctx.terrain_select = false;
    assert_eq!(right(ctx).order.kind, OrderKind::None);
}

#[test]
fn right_click_on_an_enemy_attacks_it() {
    // updateAOEStyle 1583-1584 -> doAttack (1819-1835): ATTACK_TO_DESTROY, not energy, run speed
    // (1827 `getWalk() ? SLOW : FAST`, walk unset).
    let res = right(on(Relation::Enemy, false));
    assert_eq!(
        res.order.kind,
        OrderKind::Attack {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn right_click_on_an_enemy_carries_the_walk_speed() {
    // doAttack 1827: the attack order's travel mode is `getWalk() ? TRAVEL_MODE_SLOW : FAST`. The
    // two speeds must give two intents, since the app cannot recover the choice otherwise.
    let mut walk_ctx = on(Relation::Enemy, false);
    walk_ctx.walk = true;
    let walking = right(walk_ctx).order.kind;
    let running = right(on(Relation::Enemy, false)).order.kind;
    assert_eq!(
        walking,
        OrderKind::Attack {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: true
        }
    );
    assert_ne!(walking, running, "walk and run attacks must differ");
}

#[test]
fn right_click_on_an_enemy_with_energy_weapons_conserves_ammo() {
    // doAttack 1823: `bEnergyWeapons ? ATTACK_CONSERVING_AMMO : ATTACK_TO_DESTROY`.
    let mut ctx = on(Relation::Enemy, false);
    ctx.energy_weapons = true;
    ctx.fire_from_current_pos = true;
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::Attack {
            target: 7,
            conserve_ammo: true,
            fire_from_current_pos: true,
            walk: false
        }
    );
}

#[test]
fn right_click_on_an_enemy_in_waypoint_mode_is_refused() {
    // doAttack 1804: `getKeyDown(WAYPOINT_KEY) || !target` -> the invalid sound, no order.
    let mut ctx = on(Relation::Enemy, false);
    ctx.waypoint_key_down = true;
    assert_eq!(right(ctx).order.kind, OrderKind::Denied);
}

#[test]
fn right_click_on_a_disabled_enemy_is_refused() {
    // updateAOEStyle 1583-1586: `getTeam() != home && !isDisabled()` fails; the final branch sounds.
    let mut ctx = on(Relation::Enemy, false);
    ctx.target.as_mut().unwrap().facts.disabled = true;
    assert_eq!(right(ctx).order.kind, OrderKind::Denied);
}

#[test]
fn right_click_on_a_capturable_building_captures_it() {
    // doAttack 1838-1851: every selected mover is a ground BattleMech with no capture blocker.
    let mut ctx = on(Relation::Enemy, false);
    ctx.target.as_mut().unwrap().facts.captureable = true;
    ctx.selected = vec![SelectedMover {
        mech_on_ground: true,
        capture_blocked: false,
    }];
    ctx.walk = true;
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::Capture {
            target: 7,
            walk: true
        }
    );
}

#[test]
fn right_click_to_capture_is_refused_when_a_selected_mover_cannot() {
    // doAttack 1838-1860: one blocked mech (or a non-mech) refuses the capture.
    let mut ctx = on(Relation::Enemy, false);
    ctx.target.as_mut().unwrap().facts.captureable = true;
    ctx.selected = vec![
        SelectedMover {
            mech_on_ground: true,
            capture_blocked: false,
        },
        SelectedMover {
            mech_on_ground: true,
            capture_blocked: true,
        },
    ];
    assert_eq!(right(ctx).order.kind, OrderKind::Denied);
}

#[test]
fn right_click_in_salvage_mode_recovers_a_disabled_mech() {
    // updateAOEStyle 1569-1570 -> doSalvage (4470-4486): TACTICAL_ORDER_RECOVER, queued in waypoint mode.
    let mut ctx = on(Relation::Enemy, false);
    ctx.controls = Controls {
        salvage: true,
        ..Controls::default()
    };
    ctx.target.as_mut().unwrap().facts.can_salvage = true;
    ctx.waypoint_key_down = true;
    let res = right(ctx);
    assert_eq!(res.order.kind, OrderKind::Salvage { target: 7 });
    assert!(res.order.queued);
}

#[test]
fn right_click_in_salvage_mode_does_nothing_when_salvage_is_not_possible() {
    // doSalvage 4472: `if (canSalvage(target))`; otherwise nothing, and no sound.
    let mut ctx = on(Relation::Enemy, false);
    ctx.controls = Controls {
        salvage: true,
        ..Controls::default()
    };
    assert_eq!(right(ctx).order.kind, OrderKind::None);
}

#[test]
fn right_click_in_repair_mode_refits_a_mech_that_needs_it() {
    // updateAOEStyle 1573-1574 -> doRepair (4523-4538): TACTICAL_ORDER_REFIT.
    let mut ctx = on(Relation::Friendly, true);
    ctx.controls = Controls {
        repair: true,
        ..Controls::default()
    };
    ctx.target.as_mut().unwrap().facts.can_repair = true;
    ctx.walk = true;
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::Repair {
            target: 7,
            walk: true
        }
    );
}

#[test]
fn right_click_on_a_friendly_repair_bay_gets_the_mech_fixed() {
    // updateAOEStyle 1577-1578 -> doRepairBay (4557-4571): TACTICAL_ORDER_GETFIXED.
    let mut ctx = on(Relation::Friendly, true);
    ctx.target.as_mut().unwrap().facts.refit_bay = true;
    ctx.target.as_mut().unwrap().facts.can_repair_bay = true;
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::GetFixed {
            target: 7,
            walk: false
        }
    );
}

#[test]
fn right_click_in_guard_mode_guards_a_unit() {
    // updateAOEStyle 1571-1572 -> doGuard(target) (2030-2042): TACTICAL_ORDER_GUARD on the object.
    let mut ctx = on(Relation::Friendly, true);
    ctx.controls = Controls {
        guard: true,
        ..Controls::default()
    };
    assert_eq!(right(ctx).order.kind, OrderKind::GuardUnit { target: 7 });
}

#[test]
fn right_click_on_an_own_unit_without_a_mode_is_refused() {
    // The final branch of updateAOEStyle (1585-1586): the invalid sound, unless an aimed shot is
    // set (1585 `!bForcedShot && !bAimedShot`).
    assert_eq!(
        right(on(Relation::Friendly, true)).order.kind,
        OrderKind::Denied
    );
    let mut aimed = on(Relation::Friendly, true);
    aimed.aimed_shot = true;
    assert_eq!(right(aimed).order.kind, OrderKind::None);
}

// ---------------------------------------------------------------------------------------------
// Force fire (forceShot, missiongui.cpp:3216-3239). It is a hot key (KEY_F, 209) run by update()
// before the click chain (updateAOEStyle 1486), so it takes the command click ahead of placement,
// the GUI test and the order chain, and it issues on any target, friendly or allied included.

/// The AOE command click with a hot key command running this frame.
fn hot(mut ctx: ClickContext, command: MissionCommand) -> Resolution {
    ctx.hot_keys = vec![command];
    right(ctx)
}

/// The AOE command click with the force-fire command running this frame.
fn forced(ctx: ClickContext) -> Resolution {
    hot(ctx, MissionCommand::ForceShot)
}

#[test]
fn forced_fire_on_open_ground_attacks_the_point() {
    // forceShot 3224: `target ? ATTACK_OBJECT : ATTACK_POINT`, with `targetPoint = wPos` (3226-3227).
    assert_eq!(
        forced(ground()).order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn forced_fire_on_a_friendly_target_attacks_the_object() {
    // forceShot 3224 has no team test: a friendly object is attacked as ATTACK_OBJECT. doAttack's
    // refusal of friendlies (1831-1835) is not reached because forceShot consumes the click.
    assert_eq!(
        forced(on(Relation::Friendly, true)).order.kind,
        OrderKind::ForceFireObject {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn forced_fire_on_an_allied_target_attacks_the_object() {
    assert_eq!(
        forced(on(Relation::Friendly, false)).order.kind,
        OrderKind::ForceFireObject {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn forced_fire_carries_energy_fire_position_and_walk() {
    // forceShot 3228 (`bEnergyWeapons ? CONSERVING_AMMO : TO_DESTROY`), 3231 (`pursue` is off
    // when firing from the current position) and 3232 (`getWalk() ? SLOW : FAST`).
    let mut ctx = ground();
    ctx.energy_weapons = true;
    ctx.fire_from_current_pos = true;
    ctx.walk = true;
    assert_eq!(
        forced(ctx).order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: true,
            fire_from_current_pos: true,
            walk: true
        }
    );
}

#[test]
fn forced_fire_in_waypoint_mode_is_queued_not_refused() {
    // forceShot does not test WAYPOINT_KEY (unlike doAttack 1804). handleOrders queues the order
    // while the key is held (2513-2516).
    let mut ctx = on(Relation::Enemy, false);
    ctx.waypoint_key_down = true;
    let res = forced(ctx);
    assert_eq!(
        res.order.kind,
        OrderKind::ForceFireObject {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
    assert!(res.order.queued);
}

#[test]
fn forced_fire_over_the_control_panel_still_fires() {
    // updateAOEStyle calls update() (1486) before the `bGui` test (1490), and forceShot
    // takes no GUI test.
    let mut ctx = ground();
    ctx.gui = true;
    assert_eq!(
        forced(ctx).order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn forced_fire_precedes_vehicle_placement() {
    // Placement (1517-1533) is in the chain after update() returns 0, so a forced shot wins over a
    // vehicle drop that would otherwise be placed.
    let mut ctx = ground();
    ctx.controls.adding_vehicle = true;
    ctx.can_add_vehicle = true;
    assert_eq!(
        forced(ctx).order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn forced_fire_without_the_command_click_gives_no_forced_order() {
    // forceShot returns 0 unless commandClicked (3221). With only the key down, the click chain
    // runs as before, and an open-ground left release selects nothing and orders nothing.
    let mut ctx = ground();
    ctx.hot_keys = vec![MissionCommand::ForceShot];
    ctx.left_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.order.kind, OrderKind::None);
}

#[test]
fn forced_fire_during_camera_hold_still_fires() {
    // With the camera key held, moveCameraAround returns 1 (3383, 3436) and update() still runs
    // from the camera branch (861), with commandClicked set at 852, so forceShot fires.
    let mut camera = idle();
    camera.camera = true;
    let mut ctx = ground();
    ctx.hot_keys = vec![MissionCommand::ForceShot];
    ctx.right_release = true;
    let res = run(&mut SelectState::default(), camera, &[], &ctx);
    assert_eq!(
        res.order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
    assert_eq!(res.selection, SelectionIntent::Unchanged);
}

#[test]
fn fire_from_current_position_command_click_forces_from_the_current_position() {
    // fireFromCurrentPos (missiongui.cpp:2016-2021) sets the toggle and returns forceShot(), so the
    // command click gives the force-fire order with fire_from_current_pos set (3231: pursue off).
    assert_eq!(
        hot(ground(), MissionCommand::FireFromCurrentPos).order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: false,
            fire_from_current_pos: true,
            walk: false
        }
    );
}

// ---------------------------------------------------------------------------------------------
// Hot key commands that consume the click (`update`, missiongui.cpp:1690, returns 1 and
// updateAOEStyle returns at 1486). The handler runs before the click chain, so the chain's tests
// (guard, placement, the team test of 1569-1586) do not apply to it.

#[test]
fn attack_range_command_click_attacks_before_guard() {
    // attackShort 1904-1912: `commandClicked && target` gives doAttack and returns 1, so the guard
    // toggle of the chain (1571-1572) is never reached. doAttack gives ATTACK_OBJECT (1801-1880).
    let mut ctx = on(Relation::Enemy, false);
    ctx.controls = Controls {
        guard: true,
        ..Controls::default()
    };
    for command in [
        MissionCommand::AttackShort,
        MissionCommand::AttackMedium,
        MissionCommand::AttackLong,
    ] {
        assert_eq!(
            hot(ctx.clone(), command).order.kind,
            OrderKind::Attack {
                target: 7,
                conserve_ammo: false,
                fire_from_current_pos: false,
                walk: false
            },
            "{command:?}"
        );
    }
}

#[test]
fn attack_range_command_click_carries_energy_position_and_walk() {
    // doAttack 1812-1814 (`bFireFromCurrentPos`), 1827 (`bEnergyWeapons`) and 1876 (`getWalk()`).
    let mut ctx = on(Relation::Enemy, false);
    ctx.energy_weapons = true;
    ctx.fire_from_current_pos = true;
    ctx.walk = true;
    assert_eq!(
        hot(ctx, MissionCommand::AttackMedium).order.kind,
        OrderKind::Attack {
            target: 7,
            conserve_ammo: true,
            fire_from_current_pos: true,
            walk: true
        }
    );
}

#[test]
fn attack_range_command_click_in_waypoint_mode_is_refused() {
    // doAttack 1804: the invalid sound, and the click is still consumed.
    let mut ctx = on(Relation::Enemy, false);
    ctx.waypoint_key_down = true;
    assert_eq!(
        hot(ctx, MissionCommand::AttackLong).order.kind,
        OrderKind::Denied
    );
}

#[test]
fn attack_range_command_click_on_an_allied_unit_issues_nothing() {
    // doAttack 1831-1835 returns with no order and no sound. The click is consumed, so the
    // own-unit refusal of the chain (1585) does not run either.
    assert_eq!(
        hot(on(Relation::Friendly, false), MissionCommand::AttackShort).order,
        Default::default()
    );
}

#[test]
fn attack_range_command_click_during_camera_hold_still_attacks() {
    // The camera branch runs update() (861) with commandClicked set (852), so doAttack issues.
    let mut camera = idle();
    camera.camera = true;
    let mut ctx = on(Relation::Enemy, false);
    ctx.hot_keys = vec![MissionCommand::AttackShort];
    ctx.right_release = true;
    let res = run(&mut SelectState::default(), camera, &[], &ctx);
    assert_eq!(
        res.order.kind,
        OrderKind::Attack {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn attack_range_command_click_on_open_ground_falls_through_to_the_chain() {
    // attackShort 1907: without a target it returns 0, so the click chain runs and moves.
    let mut ctx = ground();
    ctx.hot_keys = vec![MissionCommand::AttackShort];
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::MoveTo {
            point: POINT,
            walk: false
        }
    );
}

#[test]
fn attack_range_command_click_over_the_control_panel_issues_nothing() {
    // updateTarget clears the target when bGui is set (missiongui.cpp:1215-1219), so attackLong
    // (1929-1934) returns 0 without doAttack: the command is not consumed. Under the control
    // panel the chain does not target either (1490-1495), so no order is issued.
    let mut ctx = ground();
    ctx.gui = true;
    ctx.hot_keys = vec![MissionCommand::AttackLong];
    assert_eq!(right(ctx).order, Default::default());
}

/// An enemy ground BattleMech under the cursor, with a selected mover whose weapon is not area
/// effect: the target and weapon facts the aimed shot reads (aimLeg 2125, 2178-2180, 2229-2232).
fn aimable_mech() -> ClickContext {
    let mut ctx = on(Relation::Enemy, false);
    if let Some(t) = ctx.target.as_mut() {
        t.ground_battlemech = true;
    }
    ctx.any_without_area_effect = true;
    ctx
}

const AIMED: [MissionCommand; 3] = [
    MissionCommand::AimLeg,
    MissionCommand::AimArm,
    MissionCommand::AimHead,
];

#[test]
fn aimed_shot_command_click_on_a_mech_is_unavailable_not_an_attack() {
    // aimLeg 2134-2157, aimArm 2195 and aimHead 2244 issue ATTACK_OBJECT with an aimLocation.
    // aimHead returns 0 after issuing it (2273), so the chain would run; the issued order is
    // Unavailable whichever way the handler returned.
    for command in AIMED {
        let mut ctx = aimable_mech();
        ctx.hot_keys = vec![command];
        let res = right(ctx);
        assert!(
            matches!(res.order.kind, OrderKind::Unavailable(_)),
            "{command:?} gave {:?}",
            res.order.kind
        );
    }
}

#[test]
fn issued_aimed_shot_with_guard_active_is_unavailable_not_guard() {
    // The aimed order is issued before the chain's guard toggle (1571-1572). aimHead then returns 0,
    // so the chain runs, but its GuardUnit is never the reported order.
    let mut ctx = aimable_mech();
    ctx.controls = Controls {
        guard: true,
        ..Controls::default()
    };
    for command in AIMED {
        let mut c = ctx.clone();
        c.hot_keys = vec![command];
        assert!(
            matches!(right(c).order.kind, OrderKind::Unavailable(_)),
            "{command:?}"
        );
    }
}

#[test]
fn aimed_shot_without_a_non_area_weapon_gives_no_order() {
    // aimLeg 2125-2129, aimArm 2199-2202 and aimHead 2247-2251: with no such weapon the cursor is
    // set to DONT and the handler returns 1, so the click is consumed with no order, even in guard.
    let mut ctx = aimable_mech();
    ctx.any_without_area_effect = false;
    ctx.controls = Controls {
        guard: true,
        ..Controls::default()
    };
    for command in AIMED {
        let mut c = ctx.clone();
        c.hot_keys = vec![command];
        assert_eq!(right(c).order.kind, OrderKind::None, "{command:?}");
    }
}

#[test]
fn aimed_leg_with_both_legs_destroyed_gives_no_order() {
    // aimLeg 2131-2136: both legs destroyed sets DONT and returns 1. aimArm (2178-2196) and aimHead
    // (2229-2244) issue without testing the legs.
    let mut ctx = aimable_mech();
    if let Some(t) = ctx.target.as_mut() {
        t.both_legs_destroyed = true;
    }
    ctx.hot_keys = vec![MissionCommand::AimLeg];
    assert_eq!(right(ctx.clone()).order.kind, OrderKind::None);
    for command in [MissionCommand::AimArm, MissionCommand::AimHead] {
        ctx.hot_keys = vec![command];
        assert!(
            matches!(right(ctx.clone()).order.kind, OrderKind::Unavailable(_)),
            "{command:?}"
        );
    }
}

#[test]
fn aimed_shot_off_the_command_click_gives_no_order() {
    // aimLeg 2137-2138, aimArm 2211-2212 and aimHead 2258-2259: without commandClicked the cursor
    // is left as set and the handler returns 1, so nothing is issued and the click is consumed.
    let mut ctx = aimable_mech();
    ctx.right_release = false;
    for command in AIMED {
        let mut c = ctx.clone();
        c.hot_keys = vec![command];
        let res = run(&mut SelectState::default(), idle(), &[], &c);
        assert_eq!(res.order.kind, OrderKind::None, "{command:?}");
    }
}

#[test]
fn aimed_shot_on_a_non_mech_target_gives_no_order_or_falls_through() {
    // aimArm 2214-2217 and aimHead 2261-2264 set DONT for any target that is not a ground BattleMech
    // and return 1: consumed, no order. aimLeg returns 0 there (2169), see the test below.
    for command in [MissionCommand::AimArm, MissionCommand::AimHead] {
        assert_eq!(
            hot(on(Relation::Enemy, false), command).order.kind,
            OrderKind::None,
            "{command:?}"
        );
    }
}

#[test]
fn aimed_shot_command_click_with_no_object_gives_no_order() {
    // aimLeg 2163-2166, aimArm 2214-2217 and aimHead 2267-2270: with no target the cursor is set
    // to DONT and each handler returns 1, so the click is consumed and no move is ordered, though
    // the open ground would otherwise move.
    for command in [
        MissionCommand::AimLeg,
        MissionCommand::AimArm,
        MissionCommand::AimHead,
    ] {
        assert_eq!(
            hot(ground(), command).order.kind,
            OrderKind::None,
            "{command:?}"
        );
    }
}

#[test]
fn aimed_shot_that_returns_zero_lets_the_click_chain_run() {
    // aimLeg returns 0 when the target is not a ground BattleMech: no branch of the target case
    // returns 1, so it reaches the final `return 0` (2169). update() then does not return 1 and
    // the ordinary chain runs; a right click on the enemy gets the chain's own attack (doAttack
    // 1827, walk unset), the same as a right click with no hot key.
    let mut hot_ctx = on(Relation::Enemy, false);
    hot_ctx.hot_keys = vec![MissionCommand::AimLeg];
    assert_eq!(
        right(hot_ctx).order.kind,
        OrderKind::Attack {
            target: 7,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn consumed_command_without_a_ported_order_does_not_fall_through() {
    // eject (missiongui.cpp:2311-2330) returns 1 on every path. Its order is not ported, so the
    // click is Unavailable and the chain (a move on open ground) does not run.
    assert!(matches!(
        hot(ground(), MissionCommand::Eject).order.kind,
        OrderKind::Unavailable(_)
    ));
}

#[test]
fn right_click_on_an_allied_unit_is_silent() {
    // doAttack 1831-1835: `Team::home->isFriendly(target team)` returns with nothing issued.
    let ctx = on(Relation::Friendly, false);
    assert_eq!(right(ctx).order, Default::default());
}

#[test]
fn info_select_on_a_mover_only_sets_the_info_window() {
    // updateAOEStyle 1581-1582: `isSelectingInfoObject() && isMover()` -> setInfoWndMover.
    let mut ctx = on(Relation::Enemy, false);
    ctx.controls = Controls {
        info_select: true,
        ..Controls::default()
    };
    ctx.right_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.order.kind, OrderKind::None);
    assert_eq!(res.info, InfoWindow::Set(7));
}

#[test]
fn vehicle_drop_is_unavailable_and_refused_when_no_site_fits() {
    // updateAOEStyle 1517-1533: a valid drop calls beginVtol (not ported); an invalid one sounds.
    let mut ctx = ground();
    ctx.controls = Controls {
        adding_vehicle: true,
        ..Controls::default()
    };
    ctx.can_add_vehicle = true;
    assert!(matches!(
        right(ctx.clone()).order.kind,
        OrderKind::Unavailable(_)
    ));
    ctx.can_add_vehicle = false;
    assert_eq!(right(ctx).order.kind, OrderKind::Denied);
}

#[test]
fn minelaying_move_is_unavailable() {
    // doMove 3165: `MOVE_MODE_MINELAYING` is not ported.
    let mut ctx = ground();
    ctx.controls = Controls {
        mines: true,
        ..Controls::default()
    };
    assert!(matches!(right(ctx).order.kind, OrderKind::Unavailable(_)));
}

#[test]
fn the_mc_mouse_profile_is_unavailable_not_faked() {
    // updateOldStyle (1267-1460) is not ported: both clicks are reported as unavailable.
    let mut ctx = ground();
    ctx.style = ControlStyle::Mc;
    ctx.right_release = true;
    ctx.left_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert!(matches!(res.selection, SelectionIntent::Unavailable(_)));
    assert!(matches!(res.order.kind, OrderKind::Unavailable(_)));
}

#[test]
fn right_click_on_empty_ground_in_info_pick_cancels_the_pick() {
    // missiongui.cpp:1541-1544: with no target, `isSelectingInfoObject()` plays the invalid sound
    // and calls cancelInfo(), which leaves the pick mode whatever the terrain under the cursor.
    let mut ctx = ground();
    ctx.controls = Controls {
        info_select: true,
        ..Controls::default()
    };
    ctx.right_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.order.kind, OrderKind::Denied);
    assert_eq!(res.info, InfoWindow::CancelPick);
}

#[test]
fn camera_held_release_selects_and_orders_nothing() {
    // missiongui.cpp:855-866: with the camera key held (cameraClicked, 858), moveCameraAround
    // returns true (3380-3436) and update() returns before selectClicked and updateAOEStyle.
    let mut left = on(Relation::Friendly, true);
    left.left_release = true;
    let mut camera = idle();
    camera.camera = true;
    let res = run(&mut SelectState::default(), camera, &[], &left);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert_eq!(res.order.kind, OrderKind::None);
    assert_eq!(res.info, InfoWindow::Keep);

    // The same early return covers the AOE command click (`rightClicked`, 858).
    let mut right = ground();
    right.right_release = true;
    let res = run(&mut SelectState::default(), camera, &[], &right);
    assert_eq!(res.order.kind, OrderKind::None);
    assert_eq!(res.info, InfoWindow::Keep);
}

#[test]
fn camera_held_release_takes_no_drag_box() {
    // doDrag is not called while cameraClicked (3554-3644 sit behind the return at 3436 and the
    // `!cameraClicked` test at 3559), so the pending drag survives the camera frame unchanged.
    let units = [unit(1, 100.0, 100.0)];
    let mut state = SelectState::default();
    run(
        &mut state,
        drag(KeyStatus::Pressed, [40.0, 40.0, 0.0], [40.0, 40.0]),
        &units,
        &ground(),
    );
    run(
        &mut state,
        drag(KeyStatus::Held, [120.0, 120.0, 0.0], [120.0, 120.0]),
        &units,
        &ground(),
    );
    let mut release = drag(KeyStatus::Released, [160.0, 160.0, 0.0], [160.0, 160.0]);
    release.camera = true;
    let res = run(&mut state, release, &units, &ground());
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert!(state.drag.dragging());
}

#[test]
fn camera_held_double_click_still_selects_the_force_group() {
    // missiongui.cpp:762-824 run before moveCameraAround (853-866), so the double-click group
    // selection is not behind the camera return.
    let mut ctx = on(Relation::Friendly, true);
    ctx.left_double = true;
    ctx.left_down = true;
    ctx.target.as_mut().unwrap().home_team = true;
    ctx.target.as_mut().unwrap().groups = 1 << 4;
    let mut camera = idle();
    camera.camera = true;
    let res = run(&mut SelectState::default(), camera, &[], &ctx);
    assert_eq!(
        res.selection,
        SelectionIntent::Groups(ForceGroup::Select {
            group: 4,
            exclusive: true
        })
    );
    assert_eq!(res.order.kind, OrderKind::None);
}

#[test]
fn a_gui_click_does_nothing() {
    // updateAOEStyle 1490-1499 and 1514: over the control panel only the placement modes continue.
    // updateTarget clears any target under bGui (1215-1219), so the click has no target to act on.
    let mut ctx = ground();
    ctx.gui = true;
    ctx.right_release = true;
    ctx.left_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &ctx);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
    assert_eq!(res.order.kind, OrderKind::None);
}

#[test]
fn waypoint_key_comes_from_set_waypoint_key() {
    // missiongui.cpp:849-850 default WAYPOINT_KEY is KEY_LCONTROL; setWayPointKey (5371) rebinds it.
    let mut hk = HotKeys::new(ControlStyle::Aoe);
    let ctrl = KeyFrame::default().with(KEY_LCONTROL, KeyStatus::Held);
    let shift = KeyFrame::default().with(KEY_LSHIFT, KeyStatus::Held);
    assert!(waypoint_key_down(&hk, &ctrl));
    assert!(!waypoint_key_down(&hk, &shift));
    hk.set_waypoint_key(KEY_LSHIFT);
    assert!(!waypoint_key_down(&hk, &ctrl));
    assert!(waypoint_key_down(&hk, &shift));
}

// ---------------------------------------------------------------------------------------------
// Force groups (selectForceGroup 3249-3282, makeForceGroup 3284-3307)

#[test]
fn exclusive_force_group_selects_only_its_enabled_members() {
    // Group 3. a: member, selected. b: not a member, selected -> cleared. c: member but disabled
    // -> not selected. d: member, not on the GUI -> not touched.
    let mut a = unit(1, 0.0, 0.0);
    a.groups = 1 << 3;
    a.selected = true;
    let mut b = unit(2, 0.0, 0.0);
    b.selected = true;
    let mut c = unit(3, 0.0, 0.0);
    c.groups = 1 << 3;
    c.disabled = true;
    let mut d = unit(4, 0.0, 0.0);
    d.groups = 1 << 3;
    d.on_gui = false;
    let units = [a, b, c, d];
    assert_eq!(force_group_changes(&units, 3, true), vec![(2, false)]);
}

#[test]
fn toggling_a_full_force_group_deselects_it() {
    // selectForceGroup 3262-3282 with deselect false: all members selected -> deselect them.
    let mut x = unit(1, 0.0, 0.0);
    x.groups = 1 << 1;
    x.selected = true;
    let mut y = unit(2, 0.0, 0.0);
    y.groups = 1 << 1;
    y.selected = true;
    assert_eq!(
        force_group_changes(&[x, y], 1, false),
        vec![(1, false), (2, false)]
    );
}

#[test]
fn toggling_a_partly_selected_force_group_selects_all_of_it() {
    // selectForceGroup 3262-3282: one member unselected -> select every member.
    let mut x = unit(1, 0.0, 0.0);
    x.groups = 1 << 1;
    x.selected = true;
    let mut y = unit(2, 0.0, 0.0);
    y.groups = 1 << 1;
    assert_eq!(force_group_changes(&[x, y], 1, false), vec![(2, true)]);
}

#[test]
fn make_force_group_takes_the_selected_own_units() {
    // makeForceGroup 3284-3307: the group is removed from every own unit, then added to the
    // selected own units. An enemy-commanded unit is untouched.
    let mut a = unit(1, 0.0, 0.0);
    a.selected = true;
    let mut b = unit(2, 0.0, 0.0);
    b.groups = 1 << 4;
    let mut enemy = unit(3, 0.0, 0.0);
    enemy.own = false;
    enemy.selected = true;
    assert_eq!(
        make_force_group(&[a, b, enemy], 4),
        vec![(1, 1 << 4), (2, 0)]
    );
}

// ---------------------------------------------------------------------------------------------
// Raw key states through the dispatcher into one frame (`update`, missiongui.cpp:1690-1798). The
// app fills `hot_keys` and `waypoint_released` from `Dispatch`, as [`dispatched`] does here.

/// The click context of a frame: the dispatcher's commands and waypoint release for `keys`.
fn dispatched(keys: &KeyFrame, mut ctx: ClickContext) -> ClickContext {
    let d = HotKeys::new(ControlStyle::Aoe).dispatch(
        keys,
        DispatchContext {
            chatting: ctx.chatting,
            line_of_sight: false,
        },
    );
    ctx.hot_keys = d.commands.iter().map(|c| c.command).collect();
    ctx.waypoint_released = d.waypoints_done;
    ctx
}

#[test]
fn waypoint_release_on_a_right_click_gives_waypoints_done_not_a_move() {
    // update() sends TACTICAL_ORDER_WAYPOINTS_DONE on the release (missiongui.cpp:1714-1731) and
    // returns 1, so updateAOEStyle returns at 1486 and no move is issued.
    let keys = KeyFrame::default().with(KEY_LCONTROL, KeyStatus::Released);
    let ctx = dispatched(&keys, ground());
    assert!(ctx.waypoint_released);
    assert_eq!(right(ctx).order.kind, OrderKind::WaypointsDone);
}

#[test]
fn chatting_click_ends_the_click_chain() {
    // updateChat returns 1 (missiongui.cpp:1703), so update() returns 1 and updateAOEStyle returns
    // at 1486: neither the right click's move nor the left click's selection runs.
    let mut chat = ground();
    chat.chatting = true;
    assert_eq!(right(chat).order, Default::default());

    let mut own = on(Relation::Friendly, true);
    own.chatting = true;
    own.left_release = true;
    let res = run(&mut SelectState::default(), idle(), &[], &own);
    assert_eq!(res.selection, SelectionIntent::Unchanged);
}

#[test]
fn force_shot_from_the_dispatcher_fires_on_its_own() {
    // F is forceShot (missiongui.cpp:197-307). Alone on a right click it fires at the point.
    let keys = KeyFrame::default().with(KEY_F, KeyStatus::Pressed);
    let ctx = dispatched(&keys, ground());
    assert_eq!(
        right(ctx).order.kind,
        OrderKind::ForceFirePoint {
            point: POINT,
            conserve_ammo: false,
            fire_from_current_pos: false,
            walk: false
        }
    );
}

#[test]
fn two_hot_key_orders_on_one_frame_are_reported_not_dropped() {
    // E is selectVisible, which consumes the click, and F is forceShot. update() runs both in table
    // order (missiongui.cpp:1756-1795), and C++ fires every matching handler. One OrderIntent cannot
    // carry both, so the frame reports Unavailable rather than dropping the force-fire order.
    let keys = KeyFrame::default()
        .with(KEY_E, KeyStatus::Pressed)
        .with(KEY_F, KeyStatus::Pressed);
    let ctx = dispatched(&keys, ground());
    assert_eq!(
        ctx.hot_keys,
        vec![MissionCommand::SelectVisible, MissionCommand::ForceShot]
    );
    let res = right(ctx);
    assert!(
        matches!(res.order.kind, OrderKind::Unavailable(_)),
        "{:?}",
        res.order
    );
    // E alone gives its own unavailable order; the pair must not collapse to it.
    let e_only = KeyFrame::default().with(KEY_E, KeyStatus::Pressed);
    assert_ne!(res.order, right(dispatched(&e_only, ground())).order);
}
