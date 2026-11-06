//! Unit-level tests on small synthetic maps (no game data needed).
//!
//! The world is 60x60 cells: four 30x30 sectors, one area each. Areas are numbered
//! `sector_row * 2 + sector_col`, so area 0 is top-left, 1 top-right, 2 bottom-left, 3 bottom-right.
//! Doors: V0 (0|1), V1 (2|3) on the vertical boundary; H0 (0|2), H1 (1|3) on the horizontal one.

use mc2_path::{
    AreaType, DoorLink, GlobalMap, GlobalMapArea, GlobalMapDoor, GlobalMapParts, LoadError,
    MapCell, MissionMap, MoveData, MoveMap, Mover, NoGates, PathContext, PathError, Planner,
    GLOBALPATH_EXISTS_FALSE, GLOBALPATH_EXISTS_TRUE, GLOBALPATH_EXISTS_UNKNOWN,
    GLOBAL_CONFIDENCE_GOOD,
};

const W: i32 = 60;
const H: i32 = 60;

fn open_mission() -> MissionMap {
    let mut cells = vec![MapCell(0); (W * H) as usize];
    for c in &mut cells {
        c.set_passable(true);
    }
    MissionMap::from_cells(W, H, cells).unwrap()
}

fn set(m: &mut MissionMap, r: i32, c: i32, passable: bool) {
    m.cell_mut(r, c).set_passable(passable);
}

fn area_grid() -> Vec<i16> {
    let mut v = Vec::with_capacity((W * H) as usize);
    for r in 0..H {
        for c in 0..W {
            v.push(((r / 30) * 2 + c / 30) as i16);
        }
    }
    v
}

/// Doors as (area a, area b, row, col, length, direction). Direction 1 = door cells run
/// down a column (vertical wall), 2 = cells run along a row (horizontal wall).
fn door(a: i16, b: i16, row: i16, col: i16, len: i8, dir: i8) -> GlobalMapDoor {
    GlobalMapDoor {
        row,
        col,
        length: len,
        open: true,
        team_id: -1,
        area: [a, b],
        area_cost: [1, 1],
        direction: [dir, dir],
        ..Default::default()
    }
}

fn areas() -> Vec<GlobalMapArea> {
    (0..4)
        .map(|i| GlobalMapArea {
            sector_r: (i / 2) as i16,
            sector_c: (i % 2) as i16,
            area_type: AreaType::Normal,
            num_doors: 0,
            owner_wid: 0,
            team_id: -1,
            off_map: false,
            open: true,
        })
        .collect()
}

/// Build a `GlobalMap` for the given real doors. Links are generated the way the game
/// builds them: from each door, through one of its areas, to every other door on that area.
fn global_for(doors: Vec<GlobalMapDoor>) -> GlobalMap {
    let n = doors.len();
    let mut links: Vec<[Vec<DoorLink>; 2]> = Vec::with_capacity(n);
    for (i, d) in doors.iter().enumerate() {
        let mut pair: [Vec<DoorLink>; 2] = [Vec::new(), Vec::new()];
        for (side, slot) in pair.iter_mut().enumerate() {
            let area = d.area[side];
            for (j, e) in doors.iter().enumerate() {
                if j == i {
                    continue;
                }
                if let Some(s) = (0..2).find(|&s| e.area[s] == area) {
                    slot.push(DoorLink {
                        door_index: j as i16,
                        door_side: s as i8,
                        cost: 1,
                        open_cost: 1,
                    });
                }
            }
        }
        links.push(pair);
    }
    GlobalMap::from_parts(GlobalMapParts {
        width: W,
        height: H,
        area_map: area_grid(),
        areas: areas(),
        doors,
        links,
    })
    .unwrap()
}

fn four_area_doors() -> Vec<GlobalMapDoor> {
    vec![
        door(0, 1, 0, 29, 30, 1),  // V0
        door(2, 3, 30, 29, 30, 1), // V1
        door(0, 2, 29, 0, 30, 2),  // H0
        door(1, 3, 29, 30, 30, 2), // H1
    ]
}

fn data(mission: MissionMap, global: GlobalMap) -> MoveData {
    MoveData {
        mission,
        global: [global.clone(), global.clone(), global],
        special_areas: Vec::new(),
        packets_used: 0,
    }
}

#[test]
fn global_path_exists_table_packs_directed_area_pairs_and_resets() {
    let mut global = global_for(four_area_doors());
    assert_eq!(global.get_path_exists(0, 2), GLOBALPATH_EXISTS_UNKNOWN);
    for to_area in 0..4 {
        global.set_path_exists(0, to_area, GLOBALPATH_EXISTS_TRUE);
        assert_eq!(
            global.get_path_exists(0, to_area),
            GLOBALPATH_EXISTS_TRUE << (to_area * 2),
            "C++ returns the still-shifted mask for destination {to_area}"
        );
    }
    global.set_path_exists(1, 0, GLOBALPATH_EXISTS_FALSE);
    assert_eq!(global.get_path_exists(1, 0), GLOBALPATH_EXISTS_FALSE);
    assert_eq!(global.get_path_exists(-1, 0), GLOBALPATH_EXISTS_UNKNOWN);
    global.clear_path_exists_table();
    assert_eq!(global.get_path_exists(0, 2), GLOBALPATH_EXISTS_UNKNOWN);
    assert_eq!(global.get_path_exists(1, 0), GLOBALPATH_EXISTS_UNKNOWN);
}

#[test]
fn stationary_mover_marks_blocked_cost_but_leaves_goal_cells_open() {
    let m = open_mission();
    let g = global_for(four_area_doors());
    let mut mm = MoveMap::new(30, 30);
    mm.set_mover(1, 0, false, false);
    mm.set_up_rect(
        &m,
        &g,
        0,
        (0, 0),
        30,
        (10, 5),
        (10, 25),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let before = mm.node(10, 10).cost;
    mm.set_mover_here(10, 10, true);
    // `MoveMap::adjustCost(row, col, COST_BLOCKED * 2)`: added to the terrain cost, not clamped
    assert_eq!(
        mm.node(10, 10).cost,
        before + 2 * mc2_path::movemap::COST_BLOCKED
    );
    assert_ne!(
        mm.node(10, 10).flags & mc2_path::movemap::MOVEFLAG_MOVER_HERE,
        0
    );

    let goal = *mm.node(10, 25);
    mm.set_mover_here(10, 25, true);
    assert_eq!(mm.node(10, 25), &goal);
}

#[test]
fn global_get_path_cost_uses_the_compiled_fresh_search() {
    let mut global = global_for(four_area_doors());
    let mut ctx = PathContext::new();
    let mut confidence = -1;
    assert_eq!(
        global.get_path_cost(&mut ctx, 0, 0, false, &mut confidence, false, &NoGates),
        1
    );
    assert_eq!(
        confidence, -1,
        "same-area early return leaves confidence untouched"
    );
    let cost = global.get_path_cost(&mut ctx, 0, 3, false, &mut confidence, false, &NoGates);
    assert!(cost > 0);
    assert_eq!(confidence, GLOBAL_CONFIDENCE_GOOD);
}

// ---------------------------------------------------------------- cell level

#[test]
fn straight_line_in_open_field() {
    let m = open_mission();
    let g = global_for(four_area_doors());
    let mut mm = MoveMap::new(30, 30);
    mm.set_clear_cost(10);
    mm.set_mover(0, 0, false, false);
    let mut ctx = PathContext::new();
    mm.set_up_rect(
        &m,
        &g,
        0,
        (0, 0),
        30,
        (10, 5),
        (10, 25),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let res = mm.calc_path(&mut ctx, &g, 0.0).unwrap();
    let steps = res.path.path_steps();
    assert_eq!(steps.len(), 20);
    assert_eq!(res.path.cost, 200, "20 orthogonal steps at clear cost 10");
    for (i, s) in steps.iter().enumerate() {
        assert_eq!(s.cell, [10, 6 + i as i32]);
        assert_eq!(s.direction, 2, "east");
    }
    assert_eq!(steps.last().unwrap().distance_to_goal, 0.0);
    assert!(steps[0].distance_to_goal > steps[1].distance_to_goal);
}

#[test]
fn diagonal_costs_more_than_two_orthogonal_steps_only_when_cheaper() {
    // A pure diagonal run of 10 cells costs 10 * 15 = 150 and beats 20 orthogonal steps (200).
    let m = open_mission();
    let g = global_for(four_area_doors());
    let mut mm = MoveMap::new(30, 30);
    mm.set_mover(0, 0, false, false);
    let mut ctx = PathContext::new();
    mm.set_up_rect(
        &m,
        &g,
        0,
        (0, 0),
        30,
        (10, 10),
        (20, 20),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let res = mm.calc_path(&mut ctx, &g, 0.0).unwrap();
    assert_eq!(res.path.path_steps().len(), 10);
    assert_eq!(res.path.cost, 150);
    assert!(
        res.path.path_steps().iter().all(|s| s.direction % 2 == 1),
        "all diagonal"
    );
}

#[test]
fn tie_between_equal_cost_routes_is_deterministic() {
    // (10,10) -> (11,12): "diagonal then east" and "east then diagonal" both cost 25.
    let m = open_mission();
    let g = global_for(four_area_doors());
    let mut results = Vec::new();
    for _ in 0..3 {
        let mut mm = MoveMap::new(30, 30);
        mm.set_mover(0, 0, false, false);
        let mut ctx = PathContext::new();
        mm.set_up_rect(
            &m,
            &g,
            0,
            (0, 0),
            30,
            (10, 10),
            (11, 12),
            10,
            0,
            8,
            0,
            &NoGates,
        )
        .unwrap();
        let res = mm.calc_path(&mut ctx, &g, 0.0).unwrap();
        results.push(
            res.path
                .path_steps()
                .iter()
                .map(|s| s.cell)
                .collect::<Vec<_>>(),
        );
        assert_eq!(res.path.cost, 25);
    }
    assert!(
        results.windows(2).all(|w| w[0] == w[1]),
        "same path every run"
    );
    let a = vec![[11, 11], [11, 12]];
    let b = vec![[10, 11], [11, 12]];
    assert!(
        results[0] == a || results[0] == b,
        "one of the two optimal routes: {:?}",
        results[0]
    );
}

#[test]
fn wall_with_gap_forces_detour_through_gap() {
    let mut m = open_mission();
    for r in 0..=28 {
        set(&mut m, r, 15, false);
    }
    let g = global_for(four_area_doors());
    let mut mm = MoveMap::new(30, 30);
    mm.set_mover(0, 0, false, false);
    let mut ctx = PathContext::new();
    mm.set_up_rect(
        &m,
        &g,
        0,
        (0, 0),
        30,
        (10, 5),
        (10, 25),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let res = mm.calc_path(&mut ctx, &g, 0.0).unwrap();
    let cells: Vec<[i32; 2]> = res.path.path_steps().iter().map(|s| s.cell).collect();
    assert!(!cells.is_empty());
    assert!(
        cells.contains(&[29, 15]) || cells.contains(&[29, 14]) || cells.contains(&[29, 16]),
        "crosses at the gap"
    );
    assert!(
        cells.iter().all(|c| !(c[1] == 15 && c[0] <= 28)),
        "never enters the wall"
    );
    assert_eq!(*cells.last().unwrap(), [10, 25]);
    assert!(
        res.path.cost > 200,
        "detour costs more than the straight line"
    );
}

#[test]
fn sealed_goal_is_unreachable_and_start_goal_is_zero_length() {
    let mut m = open_mission();
    for r in 0..30 {
        set(&mut m, r, 15, false);
    }
    let g = global_for(four_area_doors());
    let mut mm = MoveMap::new(30, 30);
    mm.set_mover(0, 0, false, false);
    let mut ctx = PathContext::new();
    mm.set_up_rect(
        &m,
        &g,
        0,
        (0, 0),
        30,
        (10, 5),
        (10, 25),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let res = mm.calc_path(&mut ctx, &g, 0.0).unwrap();
    assert_eq!(res.path.num_steps, 0);
    assert_eq!(res.path.cost, 0, "no route leaves cost 0");

    let open = open_mission();
    let mut mm = MoveMap::new(30, 30);
    mm.set_mover(0, 0, false, false);
    mm.set_up_rect(
        &open,
        &g,
        0,
        (0, 0),
        30,
        (10, 5),
        (10, 5),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    let res = mm.calc_path(&mut ctx, &g, 0.0).unwrap();
    assert_eq!(res.path.num_steps, 0);
    assert_eq!(res.path.cost, 1, "already on the goal");
}

#[test]
fn start_outside_window_is_an_error() {
    let m = open_mission();
    let g = global_for(four_area_doors());
    let mut mm = MoveMap::new(30, 30);
    mm.set_mover(0, 0, false, false);
    let mut ctx = PathContext::new();
    mm.set_up_rect(
        &m,
        &g,
        0,
        (0, 0),
        30,
        (40, 40),
        (10, 10),
        10,
        0,
        8,
        0,
        &NoGates,
    )
    .unwrap();
    assert!(matches!(
        mm.calc_path(&mut ctx, &g, 0.0),
        Err(PathError::OutOfMap { .. })
    ));
}

// ---------------------------------------------------------------- area level

#[test]
fn route_crosses_two_doors_and_ends_in_goal_area() {
    let g = global_for(four_area_doors());
    let mut planner = Planner::new(data(open_mission(), g));
    let mover = Mover::mech(0, 60.0);
    let route = planner
        .global_path(&mover, (5, 5), (50, 50), &NoGates)
        .unwrap();
    assert_eq!(route.len(), 3, "two doors then the goal placeholder");
    let nd = planner.data.global[0].num_doors() as i32;
    assert_eq!(route.last().unwrap().goal_door, nd + 1);
    assert_eq!(route[0].thru_area, 0);
    assert!(route[1].thru_area == 1 || route[1].thru_area == 2);
    assert_eq!(route[2].thru_area, 3);
    let first = route[0].goal_door;
    assert!(first == 0 || first == 2, "V0 or H0 leaves area 0");
}

#[test]
fn plan_is_contiguous_and_reaches_goal() {
    let g = global_for(four_area_doors());
    let mut planner = Planner::new(data(open_mission(), g));
    let mover = Mover::mech(0, 60.0);
    let plan = planner.plan(&mover, (5, 5), (50, 50), &NoGates).unwrap();
    assert_eq!(plan.steps.last().unwrap().cell, [50, 50]);
    let mut prev = (5, 5);
    let mut crossed = false;
    for s in &plan.steps {
        let (dr, dc) = (s.cell[0] - prev.0, s.cell[1] - prev.1);
        assert!(
            dr.abs() <= 1 && dc.abs() <= 1 && (dr, dc) != (0, 0),
            "adjacent step {prev:?} -> {:?}",
            s.cell
        );
        if s.area != 0 {
            crossed = true;
        }
        prev = (s.cell[0], s.cell[1]);
    }
    assert!(crossed, "path leaves area 0");
}

#[test]
fn closed_door_forces_the_other_route() {
    let mut global = global_for(four_area_doors());
    global.close_door(0); // V0 closed: area 0 must leave through H0
    let mut planner = Planner::new(data(open_mission(), global));
    let mover = Mover::mech(0, 60.0);
    let route = planner
        .global_path(&mover, (5, 5), (50, 50), &NoGates)
        .unwrap();
    assert_eq!(route[0].goal_door, 2, "H0");
    assert_eq!(route[1].goal_door, 1, "then V1");
}

#[test]
fn goal_area_without_doors_is_unreachable() {
    // Only V0 and H0 exist, so area 3 has no doors.
    let doors = vec![door(0, 1, 0, 29, 30, 1), door(0, 2, 29, 0, 30, 2)];
    let g = global_for(doors);
    let mut planner = Planner::new(data(open_mission(), g));
    let mover = Mover::mech(0, 60.0);
    let route = planner
        .global_path(&mover, (5, 5), (50, 50), &NoGates)
        .unwrap();
    assert!(route.is_empty());
    assert_eq!(
        planner.plan(&mover, (5, 5), (50, 50), &NoGates),
        Err(PathError::NoRoute)
    );
}

#[test]
fn same_area_has_no_door_route_and_outside_map_has_no_area() {
    // The start placeholder can only leave its own area, so a goal in that area is never
    // reached at area level (C++ calcPath returns 0). Same-area moves are cell searches.
    let g = global_for(four_area_doors());
    let mut planner = Planner::new(data(open_mission(), g));
    let mover = Mover::mech(0, 60.0);
    assert!(planner
        .global_path(&mover, (5, 5), (5, 5), &NoGates)
        .unwrap()
        .is_empty());
    assert_eq!(
        planner.global_path(&mover, (-1, 0), (5, 5), &NoGates),
        Err(PathError::NoArea)
    );
}

// ---------------------------------------------------------------- loading

#[test]
fn mission_map_rejects_truncated_cell_packet() {
    let mut packets: Vec<Vec<u8>> = vec![
        60i32.to_le_bytes().to_vec(),
        60i32.to_le_bytes().to_vec(),
        0i32.to_le_bytes().to_vec(),
    ];
    packets.push(vec![0u8; 100]);
    match MissionMap::from_packets(&packets, 0) {
        Err(LoadError::BadPacketSize { index: 3, .. }) => {}
        other => panic!("expected BadPacketSize, got {other:?}"),
    }
}

#[test]
fn global_map_rejects_wrong_version() {
    let packets: Vec<Vec<u8>> = vec![0x0002_2518u32.to_le_bytes().to_vec()];
    assert!(matches!(
        GlobalMap::from_packets(&packets, 0),
        Err(LoadError::BadVersion { found: 0x0002_2518 })
    ));
}

#[test]
fn move_data_load_reports_missing_packets() {
    let packets: Vec<Vec<u8>> = Vec::new();
    assert!(matches!(
        MoveData::load(&packets, 4),
        Err(LoadError::MissingPacket { .. })
    ));
}
