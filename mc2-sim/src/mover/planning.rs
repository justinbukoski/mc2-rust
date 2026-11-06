// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The path-search entry points of `Mover` (`Mover::calcMovePath`, mover.cpp:4972-5170 and 5424-5560, and
//! `Mover::calcEscapePath`): they set up the path-finding map for one mover and return the raw path. The decisions of
//! `MechWarrior::calcMovePath` (which search to run, what to do with its result) are in `ai::calcpath`.
//!
//! Every function returns the C++ `numSteps` result as `path.num_steps` (0 when the search found no path or the mover cannot move:
//! `clearCost` 0) and never touches the pilot's paths.

use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::mover::MoverDecisionUnavailable;
use mc2_path::movemap::{MOVEPARAM_FOLLOW_ROADS, MOVEPARAM_SWEEP_MINES};
use mc2_path::{MovePath, PathError, Planner};

/// The result of one search: the path (`path.num_steps` = the C++ return value), the `goalCell` output and the `goalWorldPos`
/// output (`path.goal`, or `goal_world` when the path was cut).
#[derive(Clone, Debug)]
pub struct Search {
    /// The raw path. `num_steps` is 0 for "no path".
    pub path: MovePath,
    /// The `goalCell[2]` output of `calcPath`.
    pub goal_cell: (i32, i32),
    /// The path was cut where it entered the second thru area (the `goalWorldPos` output is `Terrain::cellToWorld(goal_cell)`;
    /// `path.goal` stays the goal door).
    pub trimmed: bool,
    /// The `goalWorldPos` output when the path was cut (`trimmed`).
    pub goal_world: Option<Vec3>,
}

/// What a segment search (`calcMovePath(path, start, thruArea, goalDoor, ...)`) can answer.
#[derive(Clone, Debug)]
pub enum Segment {
    /// The goal door has no open goal cell (`setUp` returned -1; `calcMovePath` returns -999).
    DoorBlocked,
    /// The search ran.
    Found(Search),
}

struct Setup {
    mover: mc2_path::Mover,
    planner: Planner,
}

/// The mover-side inputs of a search (everything `Mover::calcMovePath` reads from `this`) and a planner over the mission's move data.
fn setup(
    w: &crate::World,
    h: ObjectHandle,
    params: u32,
) -> Result<Option<Setup>, MoverDecisionUnavailable> {
    let Some(obj) = w.objects.get(h) else {
        return Ok(None);
    };
    let (slot, level, team, speed, withdrawing, jets, mine_sweeper, follow_roads) = match &obj.kind
    {
        crate::object::Kind::Mech(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            s.m.team.0,
            s.m.max_move_speed,
            s.m.flags.withdrawing,
            Some(s.num_jump_jets),
            false,
            s.m.follow_roads,
        ),
        crate::object::Kind::Vehicle(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            s.m.team.0,
            s.m.max_move_speed,
            s.m.flags.withdrawing,
            None,
            s.loadout.stats.mine_sweeper,
            s.m.follow_roads,
        ),
        _ => return Ok(None),
    };
    let kind = match level {
        1 => mc2_path::MoverKind::Hover,
        2 => mc2_path::MoverKind::Heli,
        _ => match obj.c.class {
            crate::object::ObjectClass::GroundVehicle => mc2_path::MoverKind::GroundVehicle,
            _ => mc2_path::MoverKind::Mech,
        },
    };
    // `!pilot->onHomeTeam() && !MPlayer`: `getJumpRange(&numOffsets, &jumpCost)` (mech.cpp:6744).
    let mut offsets = 8;
    if obj.team_id() != w.teams.home as i32 {
        if let Some(jets) = jets {
            let long_jump = w
                .pilots
                .get(slot)
                .is_some_and(|p| p.specialty.get(13).copied().unwrap_or(false));
            let num_jets = i32::from(jets) + i32::from(long_jump);
            const JETS_TO_OFFSETS: [i32; 6] = [8, 32, 56, 80, 104, 128];
            offsets = JETS_TO_OFFSETS[num_jets.clamp(0, 5) as usize];
        }
    }
    let jump_cost = if offsets > 8 {
        w.data
            .gamesys
            .tables
            .as_ref()
            .and_then(|t| t.mech.default_jump_cost)
            .unwrap_or(mc2_path::movemap::COST_BLOCKED / 3)
    } else {
        0
    };
    let laying_mines = matches!(obj.kind, crate::object::Kind::Vehicle(_))
        && w.pilots
            .get(slot)
            .is_some_and(|p| p.cur_order.move_params.mine_laying);
    let mut mover = mc2_path::Mover::new(kind, team, speed)
        .with_watch_id(obj.c.watch_id.0 as i32)
        .with_sweep_mines(mine_sweeper || params & MOVEPARAM_SWEEP_MINES != 0)
        .with_follow_roads(follow_roads || params & MOVEPARAM_FOLLOW_ROADS != 0)
        .with_jump(jump_cost, offsets);
    mover.params |= params;
    mover.withdrawing = withdrawing;
    mover.laying_mines = laying_mines;
    let mut planner = Planner::with_simple_range(w.terrain.move_data().clone(), simple_range(w));
    planner.set_cpp_jump_edge_cells_excluded(w.cpp_jump_edge_cells);
    Ok(Some(Setup { mover, planner }))
}

/// `SimpleMovePathRange`: the half width of the short-range window, from `[Pathfinding] SimplePathTileRange` in the loaded game data
/// (21 when the data is absent, the original's `move.cpp` default).
pub fn simple_range(w: &crate::World) -> i32 {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or(mc2_path::planner::DEFAULT_SIMPLE_RANGE, |t| {
            t.pathfinding.simple_move_path_range
        })
}

/// `PlaceStationaryMovers(map)` (mover.cpp:7127) for a search window `dim` cells on a side. The C++ compares the movers' *absolute*
/// cells with the window size and the window-relative start cell, and then indexes the window with the absolute cell, so the cells
/// returned are window indices (see `MoveMap::place_stationary_movers`). `start` and `goal` are the window-relative start and goal
/// cells when the caller knows them (the cells of those two are skipped before the mover's path is fetched, which has a side effect).
fn stationary_movers(
    w: &mut crate::World,
    h: ObjectHandle,
    dim: i32,
    start: Option<(i32, i32)>,
    goal: Option<(i32, i32)>,
) -> Vec<(i32, i32)> {
    let my_wid = w.objects.watch_id(h);
    let mut cells = Vec::new();
    for other in w.objects.movers.clone() {
        let Some(obj) = w.objects.get(other) else {
            continue;
        };
        if obj.c.class == crate::object::ObjectClass::Elemental
            || obj.c.watch_id == my_wid
            || obj.c.is_disabled()
        {
            continue;
        }
        let slot = match &obj.kind {
            crate::object::Kind::Mech(s) => s.m.pilot_slot,
            crate::object::Kind::Vehicle(s) => s.m.pilot_slot,
            _ => continue,
        };
        let cell = (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1));
        if !(0..dim).contains(&cell.0) || !(0..dim).contains(&cell.1) {
            continue;
        }
        if Some(cell) == start || Some(cell) == goal {
            continue;
        }
        // `mover->getPilot()->getMovePath()` has its spare-path swap side effect, then `path->numSteps == 0`.
        let Some(index) = crate::ai::warrior::get_move_path_index(w, slot) else {
            continue;
        };
        if w.pilots
            .get(slot)
            .is_some_and(|p| p.lane_b_move.paths[index].num_steps == 0)
        {
            cells.push(cell);
        }
    }
    cells
}

fn finish(
    result: Result<mc2_path::PathResult, PathError>,
) -> Result<Search, MoverDecisionUnavailable> {
    match result {
        Ok(r) => {
            let mut path = r.path;
            if path.num_steps == 0 {
                // `path->init()`: the "already on the goal" marker is a Rust convenience, the C++ path has cost 0.
                path.cost = 0;
            }
            Ok(Search {
                path,
                goal_cell: r.goal_cell,
                trimmed: r.trimmed,
                goal_world: None,
            })
        }
        Err(_) => Ok(Search {
            path: MovePath::new(),
            goal_cell: (-1, -1),
            trimmed: false,
            goal_world: None,
        }),
    }
}

/// `Mover::calcMovePath(path, MOVEPATH_SIMPLE, start, goal, NULL, moveParams)` (mover.cpp:5008-5088): the short-range window search.
pub fn simple_search(
    w: &mut crate::World,
    h: ObjectHandle,
    start: Vec3,
    goal: Vec3,
    params: u32,
) -> Result<Search, MoverDecisionUnavailable> {
    let Some(mut s) = setup(w, h, params)? else {
        return finish(Ok(empty()));
    };
    let start_cell = w.terrain.world_to_cell(&start);
    let goal_cell = w.terrain.world_to_cell(&goal);
    let ul = s.planner.simple_window_origin(start_cell);
    let dim = s.planner.simple_range() * 2 + 1;
    let stationary = stationary_movers(
        w,
        h,
        dim,
        Some((start_cell.0 - ul.0, start_cell.1 - ul.1)),
        Some((goal_cell.0 - ul.0, goal_cell.1 - ul.1)),
    );
    s.planner.set_stationary_movers(stationary);
    let gates = crate::object::gate_oracle::GateSnapshot::from_store(&w.objects);
    let result = s
        .planner
        .simple_path(&s.mover, start_cell, goal_cell, &gates);
    let mut found = finish(result)?;
    found.path.goal = if found.path.num_steps > 0 {
        found.path.goal
    } else {
        Vec3::default()
    };
    Ok(found)
}

/// `Mover::calcEscapePath` (mover.cpp:5170) with `MoveMap::markEscapeGoals` (move.cpp:4323) and `MoveMap::calcEscapePath`
/// (move.cpp:5926): the short-range search from a start on a bad (area-less) tile to the cheapest cell that is inside the final goal's
/// area or has a long-range route to it.
///
/// The C++ escape search is the move map's A* with the heuristic fixed at 10, i.e. a uniform-cost search over the window's cell
/// costs, ending at the first node flagged as an escape goal. That search is run here over the costs the move map's `setUp` leaves
/// (`MoveMap::node(..).cost`, diagonals one and a half times as dear, corners not cut); the route to the cell it ends on is then
/// laid by the ordinary short-range search, so the path record is the one every other mover path is. Ties between equally dear
/// cells may break differently from the C++ priority queue (rust/docs/DIFFERENCES.md).
pub fn escape_search(
    w: &mut crate::World,
    h: ObjectHandle,
    start: Vec3,
    goal: Vec3,
    params: u32,
) -> Result<Search, MoverDecisionUnavailable> {
    use mc2_path::movemap::COST_BLOCKED;
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let Some(mut s) = setup(w, h, params)? else {
        return finish(Ok(empty()));
    };
    // `if (clearCost > 0)`: a mover that cannot move gets a cleared path.
    if s.mover.simple_clear_cost() <= 0 {
        return finish(Ok(empty()));
    }
    let start_cell = w.terrain.world_to_cell(&start);
    let goal_cell = w.terrain.world_to_cell(&goal);
    let level = s.mover.level;
    let ul = s.planner.simple_window_origin(start_cell);
    let dim = s.planner.simple_range() * 2 + 1;
    let stationary = stationary_movers(
        w,
        h,
        dim,
        Some((start_cell.0 - ul.0, start_cell.1 - ul.1)),
        Some((goal_cell.0 - ul.0, goal_cell.1 - ul.1)),
    );
    let gates = crate::object::gate_oracle::GateSnapshot::from_store(&w.objects);

    let mut map = mc2_path::MoveMap::new(dim, dim);
    map.set_mover(
        s.mover.watch_id,
        s.mover.team_id,
        s.mover.laying_mines,
        s.mover.withdrawing,
    );
    if map
        .set_up_rect(
            &s.planner.data.mission,
            &s.planner.data.global[level],
            level,
            ul,
            dim,
            start_cell,
            goal_cell,
            s.mover.simple_clear_cost(),
            s.mover.jump_cost,
            s.mover.offsets,
            s.mover.params,
            &gates,
        )
        .is_err()
    {
        return finish(Ok(empty()));
    }
    map.place_stationary_movers(&stationary);

    // `markEscapeGoals`: a cell is a goal when its area is the final goal's area or a long-range route joins them. The cost depends
    // on the area pair only, so it is asked once per area.
    let final_area = s.planner.data.global[level].calc_area(goal_cell.0, goal_cell.1);
    let mut ctx = mc2_path::PathContext::new();
    let mut area_is_goal: std::collections::HashMap<i32, bool> = std::collections::HashMap::new();
    let mut is_goal = |s: &mut Setup, row: i32, col: i32| -> bool {
        let (abs_r, abs_c) = (ul.0 + row, ul.1 + col);
        if !s.planner.data.mission.in_bounds(abs_r, abs_c) {
            return false;
        }
        let area = s.planner.data.global[level].calc_area(abs_r, abs_c);
        *area_is_goal.entry(area).or_insert_with(|| {
            let mut confidence = 0;
            s.planner.data.global[level].get_path_cost(
                &mut ctx,
                area,
                final_area,
                false,
                &mut confidence,
                true,
                &gates,
            ) > 0
        })
    };

    // N, NE, E, SE, S, SW, W, NW (`cellShift`).
    const SHIFT: [(i32, i32); 8] = [
        (-1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
        (1, 0),
        (1, -1),
        (0, -1),
        (-1, -1),
    ];
    let open = |r: i32, c: i32| {
        r >= 0 && r < dim && c >= 0 && c < dim && map.node(r, c).cost < COST_BLOCKED
    };
    let (sr, sc) = (start_cell.0 - ul.0, start_cell.1 - ul.1);
    let cells = (dim * dim) as usize;
    let mut best_g = vec![i32::MAX; cells];
    let mut closed = vec![false; cells];
    let mut heap = BinaryHeap::new();
    let mut seq = 0u32;
    if sr >= 0 && sr < dim && sc >= 0 && sc < dim {
        best_g[(sr * dim + sc) as usize] = 0;
        heap.push(Reverse((0i32, seq, sr, sc)));
    }
    let mut found = None;
    while let Some(Reverse((g, _, r, c))) = heap.pop() {
        let index = (r * dim + c) as usize;
        if closed[index] || g > best_g[index] {
            continue;
        }
        closed[index] = true;
        if is_goal(&mut s, r, c) {
            found = Some((r, c));
            break;
        }
        for (dir, (dr, dc)) in SHIFT.iter().copied().enumerate() {
            let diagonal = dir % 2 == 1;
            if diagonal {
                let (a, b) = (SHIFT[dir - 1], SHIFT[(dir + 1) % 8]);
                if !open(r + a.0, c + a.1) && !open(r + b.0, c + b.1) {
                    continue;
                }
            }
            let (nr, nc) = (r + dr, c + dc);
            // `succRow > 0 && succRow < height && succCol > 0 && succCol < width`
            if nr <= 0 || nr >= dim || nc <= 0 || nc >= dim {
                continue;
            }
            let mut cost = map.node(nr, nc).cost;
            if cost >= COST_BLOCKED {
                continue;
            }
            if diagonal {
                cost += cost / 2;
            }
            let next_g = g.saturating_add(cost);
            let next_index = (nr * dim + nc) as usize;
            if !closed[next_index] && next_g < best_g[next_index] {
                best_g[next_index] = next_g;
                seq += 1;
                heap.push(Reverse((next_g, seq, nr, nc)));
            }
        }
    }
    let Some((er, ec)) = found else {
        // A mover that can jump might still leave by a jump: not ported.
        return finish(Ok(empty()));
    };
    let escape_cell = (ul.0 + er, ul.1 + ec);
    s.planner.set_stationary_movers(stationary);
    let result = s
        .planner
        .simple_path(&s.mover, start_cell, escape_cell, &gates);
    finish(result)
}

/// `Mover::calcMovePath(path, MOVEPATH_COMPLEX, start, goal, goalCell, moveParams)` (mover.cpp:5093-5170): the two-sector window
/// search of the last leg of a complex path.
pub fn window_search(
    w: &mut crate::World,
    h: ObjectHandle,
    start: Vec3,
    goal: Vec3,
    params: u32,
) -> Result<Search, MoverDecisionUnavailable> {
    let Some(mut s) = setup(w, h, params)? else {
        return finish(Ok(empty()));
    };
    let start_cell = w.terrain.world_to_cell(&start);
    let goal_cell = w.terrain.world_to_cell(&goal);
    let ul = Planner::local_window_origin(start_cell, goal_cell);
    let stationary = stationary_movers(
        w,
        h,
        mc2_path::movemap::SECTOR_DIM * 2,
        Some((start_cell.0 - ul.0, start_cell.1 - ul.1)),
        Some((goal_cell.0 - ul.0, goal_cell.1 - ul.1)),
    );
    s.planner.set_stationary_movers(stationary);
    let gates = crate::object::gate_oracle::GateSnapshot::from_store(&w.objects);
    let result = s
        .planner
        .window_path(&s.mover, start_cell, goal_cell, &gates);
    finish(result)
}

/// `Mover::calcMovePath(path, start, thruArea, goalDoor, finalGoal, &goal, goalCell, moveParams)` (mover.cpp:5424-5540): the search
/// through one area to its goal door.
pub fn segment_search(
    w: &mut crate::World,
    h: ObjectHandle,
    start: Vec3,
    thru_area: [i32; 2],
    goal_door: i32,
    final_goal: Vec3,
    params: u32,
) -> Result<Segment, MoverDecisionUnavailable> {
    let Some(mut s) = setup(w, h, params)? else {
        return Ok(Segment::Found(finish(Ok(empty()))?));
    };
    let start_cell = w.terrain.world_to_cell(&start);
    let final_cell = w.terrain.world_to_cell(&final_goal);
    // The window of a sector search is chosen inside `set_up_area`; the start and goal cells are compared there.
    let stationary = stationary_movers(w, h, mc2_path::movemap::SECTOR_DIM * 2, None, None);
    s.planner.set_stationary_movers(stationary);
    let gates = crate::object::gate_oracle::GateSnapshot::from_store(&w.objects);
    match s.planner.segment_path(
        &s.mover,
        start_cell,
        thru_area,
        usize::try_from(goal_door).unwrap_or(0),
        final_cell,
        &gates,
    ) {
        Err(PathError::NoGoalCells) => Ok(Segment::DoorBlocked),
        other => {
            let mut search = finish(other)?;
            if search.trimmed {
                // `land->cellToWorld(goalCell, *goalWorldPos)` (move.cpp:5396-5398)
                search.goal_world = Some(
                    w.terrain
                        .cell_to_world(search.goal_cell.0, search.goal_cell.1),
                );
            }
            Ok(Segment::Found(search))
        }
    }
}

fn empty() -> mc2_path::PathResult {
    mc2_path::PathResult {
        path: MovePath::new(),
        goal_cell: (-1, -1),
        trimmed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::tests::world;
    use crate::object::{Kind, Obj};
    use crate::World;
    use mc2_path::geometry::MAPCELL_DIM;
    use mc2_path::{MapCell, MissionMap};
    use std::rc::Rc;

    /// The shipped `gamesys.fit` with its `SimplePathTileRange` set to `range`; `None` when `MC2_DATA` is not set.
    fn tables_with_range(range: i32) -> Option<mc2_sim_data::gamesys::GameSystem> {
        let root = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty())?;
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(std::path::Path::new(&root)).ok()?;
        let mut tables = mc2_sim_data::gamesys::GameSystem::load(&mut vfs).ok()?;
        tables.pathfinding.simple_move_path_range = range;
        Some(tables)
    }

    /// The flat test world with mech 1 (team 0) moving at a normal speed, and a goal `distance` cells east of it in the same row.
    /// Its terrain has no movement packets, so every cell is made passable: the search then depends only on the window.
    fn mech_and_goal(distance: i32) -> (World, ObjectHandle, Vec3, Vec3) {
        let (mut w, h, _) = world();
        // `world()` builds a 64-vertex terrain; the mission map has `MAPCELL_DIM` cells per vertex.
        let n = 64 * MAPCELL_DIM;
        let mut open = MapCell(0);
        open.set_passable(true);
        w.terrain.move_data_mut().mission =
            MissionMap::from_cells(n, n, vec![open; (n * n) as usize]).expect("open map");
        if let Some(Obj {
            kind: Kind::Mech(s),
            ..
        }) = w.objects.get_mut(h)
        {
            s.m.max_move_speed = 10.0;
        }
        let start = w.objects.get(h).expect("mech 1").c.position;
        let (row, col) = w.terrain.world_to_cell(&start);
        let goal = w.terrain.cell_to_world(row, col + distance);
        (w, h, start, goal)
    }

    #[test]
    fn simple_range_is_21_without_game_data() {
        let (w, _, _, _) = mech_and_goal(0);
        assert!(w.data.gamesys.tables.is_none());
        assert_eq!(simple_range(&w), 21);
    }

    /// A goal 25 cells away in the same area is inside the 30-cell window the shipped data gives (`SimplePathTileRange = 30`),
    /// so the short-range search finds a path. Before the range was read from the data this search used 21 and found none.
    #[test]
    fn a_goal_25_cells_away_gets_a_short_range_path_with_the_shipped_range() {
        let Some(tables) = tables_with_range(30) else {
            eprintln!("MC2_DATA not set; skipping the gamesys.fit part");
            return;
        };
        let (mut w, h, start, goal) = mech_and_goal(25);
        w.data.gamesys.tables = Some(Rc::new(tables));
        assert_eq!(simple_range(&w), 30);
        let found = simple_search(&mut w, h, start, goal, 0).expect("search");
        assert!(
            found.path.num_steps > 0,
            "no short-range path 25 cells away"
        );
    }

    /// The same goal with the data's range at 21 (the old compiled-in window): no short-range path. This is the behaviour the data
    /// range replaces, and it shows the search reads the range from the loaded data.
    #[test]
    fn a_range_of_21_from_the_data_gives_no_short_range_path_25_cells_away() {
        let Some(tables) = tables_with_range(21) else {
            eprintln!("MC2_DATA not set; skipping the gamesys.fit part");
            return;
        };
        let (mut w, h, start, goal) = mech_and_goal(25);
        w.data.gamesys.tables = Some(Rc::new(tables));
        assert_eq!(simple_range(&w), 21);
        let found = simple_search(&mut w, h, start, goal, 0).expect("search");
        assert_eq!(found.path.num_steps, 0);
    }
}
