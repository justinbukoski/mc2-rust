// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mover cost and access rules, as `Mover::calcMovePath` sets them up in the runtime
//! (code/mover.cpp ~4990-5060): clear cost from speed, shallow water for mechs only,
//! hover movers on shallow and deep water, and the short-range window origin.

mod support;

use mc2_path::movemap::{
    MOVEPARAM_FOLLOW_ROADS, MOVEPARAM_SWEEP_MINES, MOVEPARAM_WATER_DEEP, MOVEPARAM_WATER_SHALLOW,
};
use mc2_path::{MapCell, MissionMap, MovePath, PathStep};
use mc2_path::{Mover, MoverKind, NoGates, PathError, Planner};
use support::world::*;

/// Open world with one column of water at col 15 that blocks the straight run from col 5 to col 25.
fn water_column_world(deep: bool) -> Planner {
    let mut m = open_mission();
    for r in 0..H {
        let cell = m.cell_mut(r, 15);
        if deep {
            cell.set_deep_water(true);
        } else {
            cell.set_shallow_water(true);
        }
    }
    Planner::new(move_data(m, parts_for(four_area_doors())))
}

#[test]
fn clear_cost_follows_the_runtime_formula() {
    // cellLength = 42.666.. * 0.2 = 8.533..; cost = cellLength / speed * 50.
    assert_eq!(Mover::mech(0, 60.0).sector_clear_cost(), 7);
    assert_eq!(Mover::mech(0, 60.0).simple_clear_cost(), 7);
    assert_eq!(Mover::ground_vehicle(0, 30.0).sector_clear_cost(), 14);
    assert_eq!(
        Mover::mech(0, 1000.0).sector_clear_cost(),
        0,
        "too fast: no sector search"
    );
    assert_eq!(
        Mover::mech(0, 1000.0).simple_clear_cost(),
        0,
        "too fast: no simple search"
    );
    assert_eq!(
        Mover::mech(0, 0.0).sector_clear_cost(),
        0,
        "zero speed: no search"
    );
}

#[test]
fn sector_clear_cost_is_long_and_simple_clear_cost_is_short() {
    // At 0.01 the raw value is 42666.67. The sector branch takes (long), so 42666.
    // The simple branch goes through float2short (short), so 42666 wraps to -22870.
    let slow = Mover::mech(0, 0.01);
    assert_eq!(slow.sector_clear_cost(), 42666);
    assert_eq!(slow.simple_clear_cost(), -22870);
}

#[test]
fn mech_wades_shallow_water_but_ground_vehicle_does_not() {
    let mut planner = water_column_world(false);
    let mech = Mover::mech(0, 60.0);
    let res = planner
        .simple_path(&mech, (10, 5), (10, 25), &NoGates)
        .unwrap();
    assert_eq!(
        res.path.num_steps, 20,
        "straight run across the shallow column"
    );
    assert_eq!(res.path.cost, 20 * 7);

    let vehicle = Mover::ground_vehicle(0, 60.0);
    let res = planner
        .simple_path(&vehicle, (10, 5), (10, 25), &NoGates)
        .unwrap();
    assert_eq!(res.path.num_steps, 0, "vehicles cannot enter shallow water");
    assert_eq!(res.path.cost, 0);
}

#[test]
fn hover_takes_deep_water_and_mech_does_not() {
    let mut planner = water_column_world(true);
    let res = planner
        .simple_path(&Mover::hover(0, 60.0), (10, 5), (10, 25), &NoGates)
        .unwrap();
    assert_eq!(res.path.num_steps, 20, "hover crosses deep water");
    let res = planner
        .simple_path(&Mover::mech(0, 60.0), (10, 5), (10, 25), &NoGates)
        .unwrap();
    assert_eq!(res.path.num_steps, 0, "mech cannot enter deep water");
    let res = planner
        .simple_path(&Mover::heli(0, 60.0), (10, 5), (10, 25), &NoGates)
        .unwrap();
    assert_eq!(res.path.num_steps, 20, "heli ignores terrain");
}

#[test]
fn zero_speed_runs_no_search_and_no_route() {
    let mut planner = water_column_world(false);
    let still = Mover::mech(0, 0.0);
    let res = planner
        .simple_path(&still, (10, 5), (10, 25), &NoGates)
        .unwrap();
    assert_eq!((res.path.num_steps, res.path.cost), (0, 0));
    assert_eq!(
        planner.plan(&still, (10, 5), (50, 50), &NoGates),
        Err(PathError::NoRoute)
    );
}

#[test]
fn mover_kinds_set_water_flags() {
    let shallow = MOVEPARAM_WATER_SHALLOW;
    let deep = MOVEPARAM_WATER_DEEP;
    assert_ne!(Mover::mech(0, 60.0).params & shallow, 0);
    assert_eq!(Mover::mech(0, 60.0).params & deep, 0);
    assert_eq!(Mover::ground_vehicle(0, 60.0).params & shallow, 0);
    assert_ne!(Mover::hover(0, 60.0).params & (shallow | deep), 0);
    assert_eq!(Mover::hover(0, 60.0).level, 1);
    assert_eq!(Mover::heli(0, 60.0).params & (shallow | deep), 0);
    assert_eq!(Mover::new(MoverKind::Mech, 3, 60.0), Mover::mech(3, 60.0));
    assert_eq!(
        Mover::mech(0, 60.0).with_follow_roads(true).params & MOVEPARAM_FOLLOW_ROADS,
        MOVEPARAM_FOLLOW_ROADS
    );
    assert_eq!(
        Mover::mech(0, 60.0).with_sweep_mines(true).params & MOVEPARAM_SWEEP_MINES,
        MOVEPARAM_SWEEP_MINES
    );
}

#[test]
fn short_range_window_clamps_only_top_and_left() {
    let planner = Planner::new(move_data(open_mission(), parts_for(four_area_doors())));
    assert_eq!(
        planner.simple_window_origin((55, 55)),
        (34, 34),
        "bottom/right edge is not shifted back"
    );
    assert_eq!(planner.simple_window_origin((5, 55)), (0, 34));
    assert_eq!(planner.simple_window_origin((5, 5)), (0, 0));
}

#[test]
fn short_range_path_near_the_bottom_right_edge() {
    let mut planner = Planner::new(move_data(open_mission(), parts_for(four_area_doors())));
    let res = planner
        .simple_path(&Mover::mech(0, 60.0), (55, 55), (57, 57), &NoGates)
        .unwrap();
    assert_eq!(res.path.num_steps, 2);
    assert_eq!(res.path.cost, 20, "two diagonal steps of 7 + 3");
    assert_eq!(res.path.path_steps().last().unwrap().cell, [57, 57]);
}

#[test]
fn jumping_movers_use_the_jump_search_for_short_and_long_routes() {
    let mut planner = Planner::new(move_data(open_mission(), parts_for(four_area_doors())));
    let jumper = Mover::mech(0, 60.0).with_jump(5, 16);
    let short = planner
        .simple_path(&jumper, (10, 10), (12, 12), &NoGates)
        .unwrap();
    assert_eq!(short.path.num_steps, 1);
    assert_eq!(short.path.path_steps()[0].direction, 11);
    assert_eq!(short.path.path_steps()[0].cell, [12, 12]);

    let long = planner.plan(&jumper, (10, 10), (50, 50), &NoGates).unwrap();
    assert_eq!(long.steps.last().unwrap().cell, [50, 50]);
    assert!(long.steps.iter().any(|step| step.direction > 7));
}

#[test]
fn local_window_origin_is_the_smaller_sector_origin_per_axis() {
    // mover.cpp sector branch: sectorUL = min(start sector, goal sector) * SECTOR_DIM on each axis.
    assert_eq!(Planner::local_window_origin((5, 35), (50, 50)), (0, 30));
    assert_eq!(Planner::local_window_origin((50, 50), (5, 35)), (0, 30));
    assert_eq!(Planner::local_window_origin((35, 35), (55, 55)), (30, 30));
    assert_eq!(Planner::local_window_origin((50, 5), (20, 20)), (0, 0));
}

#[test]
fn path_locks_and_range_queries_match_move_path_indexing() {
    let mut map = MissionMap::from_cells(4, 4, vec![MapCell(0x400); 16]).unwrap();
    let mut path = MovePath::new();
    path.init(3);
    path.num_steps_when_not_paused = 3;
    path.cur_step = 1;
    for (i, cell) in [[1, 1], [1, 2], [2, 2]].into_iter().enumerate() {
        path.steps[i] = PathStep {
            cell,
            distance_to_goal: (2 - i) as f32,
            destination: mc2_path::geometry::Vec3 {
                x: (cell[1] * 10) as f32,
                y: (cell[0] * 10) as f32,
                z: 0.0,
            },
            ..PathStep::default()
        };
    }
    path.lock(&mut map, 0, -1, 2, true);
    assert!(map.cell(1, 2).pathlock(0));
    assert!(map.cell(2, 2).pathlock(0));
    let mut reached_end = false;
    assert!(path.is_locked(&map, 0, -1, 3, Some(&mut reached_end)));
    assert!(reached_end);
    assert_eq!(path.crosses_cell(-1, 2, 2, 2), Some(2));
    assert_eq!(path.crosses_cell(-1, 1, 2, 2), None);
    assert!(!path.is_blocked(&map, -1, 3, None));
    map.cell_mut(2, 2).set_passable(false);
    assert!(path.is_blocked(&map, -1, 3, None));
    path.lock(&mut map, 0, -1, 2, false);
    assert!(!map.cell(1, 2).pathlock(0));
    assert!(!map.cell(2, 2).pathlock(0));
}

#[test]
fn plan_steps_stay_in_the_areas_of_the_global_route() {
    // Link costs include the distance from the start and goal cells (GlobalMap::calcPath), so the
    // cheapest area route here goes 1 -> 0 -> 2 -> 3, not straight through door H1. Whatever the
    // route, every step must lie in an area it crosses, and the plan must end on the goal.
    let mut planner = Planner::new(move_data(open_mission(), parts_for(four_area_doors())));
    let mover = Mover::mech(0, 60.0);
    let route = planner
        .global_path(&mover, (5, 35), (50, 50), &NoGates)
        .unwrap();
    let mut areas: Vec<i32> = route.iter().map(|s| s.thru_area).collect();
    areas.push(planner.data.global[0].calc_area(50, 50));
    let plan = planner.plan(&mover, (5, 35), (50, 50), &NoGates).unwrap();
    assert_eq!(plan.steps.last().unwrap().cell, [50, 50]);
    for s in &plan.steps {
        let area = planner.data.global[0].calc_area(s.cell[0], s.cell[1]);
        assert!(
            areas.contains(&area),
            "step {:?} in area {area}, route areas {areas:?}",
            s.cell
        );
    }
    assert_eq!(Planner::local_window_origin((30, 35), (50, 50)), (30, 30));
}

#[test]
fn zero_clear_cost_gets_no_plan_even_for_a_nearby_goal() {
    // Speed 1000 gives sector clear cost 0; the runtime runs no sector search, so neither does plan().
    let mut planner = Planner::new(move_data(open_mission(), parts_for(four_area_doors())));
    let fast = Mover::mech(0, 1000.0);
    assert_eq!(
        planner.plan(&fast, (5, 35), (50, 50), &NoGates),
        Err(PathError::NoRoute)
    );
    assert_eq!(
        planner.plan(&fast, (10, 10), (12, 12), &NoGates),
        Err(PathError::NoRoute)
    );
    let res = planner
        .simple_path(&fast, (10, 10), (12, 12), &NoGates)
        .unwrap();
    assert_eq!(res.path.num_steps, 0);
}

#[test]
fn last_leg_does_not_detour_through_an_area_outside_the_global_route() {
    // Area 3 (bottom right) has an impassable strip across row 40, cols 30..59, so the only way
    // to the goal at (50,50) from the door at row 30 runs through area 2 (cols 0..29).
    // The global route 1 -> 3 does not include area 2, so the plan must fail rather than detour.
    let mut m = open_mission();
    for c in 30..60 {
        m.cell_mut(40, c).set_passable(false);
    }
    let parts = parts_for(four_area_doors());
    let mut planner = Planner::new(move_data(m.clone(), parts.clone()));
    let mech = Mover::mech(0, 60.0);
    let route = planner
        .global_path(&mech, (25, 55), (50, 50), &NoGates)
        .unwrap();
    assert_eq!(
        route.iter().map(|s| s.thru_area).collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert_eq!(
        planner.plan(&mech, (25, 55), (50, 50), &NoGates),
        Err(PathError::NoRoute)
    );

    // Without the restriction the detour exists: a window over areas 2 and 3 finds it.
    let g = mc2_path::GlobalMap::from_parts(parts).unwrap();
    let mut mm = mc2_path::MoveMap::new(60, 60);
    mm.set_mover(0, 0, false, false);
    mm.set_up_rect(
        &m,
        &g,
        0,
        (30, 0),
        60,
        (30, 45),
        (50, 50),
        7,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let res = mm
        .calc_path(&mut mc2_path::PathContext::new(), &g, 0.0)
        .unwrap();
    assert!(res.path.num_steps > 0);
    assert!(
        res.path.path_steps().iter().any(|s| s.cell[1] < 30),
        "the detour uses area 2"
    );
}
