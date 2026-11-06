// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MechWarrior::calcMovePath(selectionIndex, moveParams)` (warrior.cpp:2754-3300): what the path manager runs for a queued request.
//! It picks the goal (`Mover::calcMoveGoal`), decides between a short-range ("simple") path and a "complex" path over the global
//! map's areas and doors, runs the searches (`mover::planning`) and writes the result into the pilot's `moveOrders` (the current or
//! the spare path, `pathType`, `originalGlobalGoal`, `globalGoalLocation`, the move goal, `nextUpdate`, `yieldTime`) and into the
//! current order's first way point.
//!
//! The escape-tile search (`Mover::calcEscapePath`, the alarm order of a mover on a bad tile) is ported in `mover::planning`.
//!
//! Not ported (each reports [`MoverDecisionUnavailable`] when reached, the pilot state afterwards is not C++ state): the jump path
//! search `calcPathJUMP` where a jump step would be taken (mechs with jump jets that are not on the home team: only the walking
//! route is searched, and it is used when it costs less than a jump step), and the goal scoring of `calcMoveGoal` for moves with a
//! target object.

use super::moveorders::{self as mo, MoveOrders, MOVEGOAL_LOCATION, MOVEGOAL_NONE};
use super::warrior::{self, alarm};
use crate::command::{OrderCode, OrderOrigin, TacticalOrder, TravelMode};
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::mover::{planning, MoverDecisionUnavailable};
use crate::World;
use mc2_path::movemap::{
    MOVEPARAM_AVOID_PATHLOCKS, MOVEPARAM_ESCAPE_TILE, MOVEPARAM_FACE_TARGET, MOVEPARAM_INIT,
    MOVEPARAM_MYSTERY_PARAM, MOVEPARAM_PLAYER, MOVEPARAM_RADIO_RESULT, MOVEPARAM_RECALC,
    MOVEPARAM_STATIONARY_MOVERS, MOVEPARAM_STEP_ADJACENT_TARGET, MOVEPARAM_STEP_TOWARD_TARGET,
};
use mc2_path::GlobalMap;

/// `NO_ERR`.
pub const NO_ERR: i32 = 0;

/// `selectionIndex / GroupMoveTrailLen[1] * GroupMoveTrailLen[0]`: how many steps a grouped mover stops short of its goal.
fn trail_cut(selection_index: i32, trail: [i32; 2]) -> i64 {
    i64::from(selection_index.checked_div(trail[1]).unwrap_or(0) * trail[0])
}

/// `MovementUpdateFrequency` (gamesys.fit); 0 without tables.
fn movement_update_frequency(w: &World) -> f32 {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or(0.0, |t| t.warrior.movement_update_frequency)
}

/// `Mover::marginOfError[1]`.
fn margin_of_error_far(w: &World) -> f32 {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or(10.0, |t| t.pathfinding.margin_of_error[1])
}

/// `GroupMoveTrailLen[0..2]`.
fn group_move_trail_len(w: &World) -> [i32; 2] {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or([0, 1], |t| t.pathfinding.group_move_trail_len)
}

fn moves(w: &mut World, slot: u16) -> Option<&mut MoveOrders> {
    w.pilots.get_mut(slot).map(|p| &mut p.lane_b_move)
}

/// `triggerAlarm(PILOT_ALARM_NO_MOVEPATH, err)` and the error as the result.
fn fail(w: &mut World, slot: u16, err: i32) -> Result<i32, MoverDecisionUnavailable> {
    warrior::trigger_alarm(w, slot, alarm::NO_MOVEPATH, err);
    Ok(err)
}

/// `curTacOrder.setWayPoint(0, location)`.
pub fn set_way_point0(order: &mut TacticalOrder, location: Vec3) {
    match order.move_params.way_path.points.first_mut() {
        Some(p) => *p = location,
        None => order.move_params.way_path.points.push(location),
    }
}

/// The goal object of a move goal: the object the watch id resolves to, else `ObjectManager->get(goalType)` (the goal type used as a
/// HANDLE, a source quirk kept for goal objects that were destroyed).
pub fn goal_target(
    w: &World,
    goal_type: i64,
    goal_object: Option<ObjectHandle>,
) -> Option<ObjectHandle> {
    if goal_type <= 0 {
        return None;
    }
    goal_object.or_else(|| {
        usize::try_from(goal_type)
            .ok()
            .filter(|&h| h >= 1 && h <= w.objects.max_objects())
            .map(|h| ObjectHandle(h as u32))
            .filter(|&h| w.objects.get(h).is_some())
    })
}

/// `RamObjectWID`: the ram target of an attack order with the ramming method, whose path lock is released around a search.
pub fn ram_object(w: &World, slot: u16) -> Option<ObjectHandle> {
    let p = w.pilots.get(slot)?;
    (p.cur_order.code == OrderCode::AttackObject
        && p.cur_order.attack_params.method == crate::command::AttackMethod::Ramming)
        .then(|| w.objects.by_watch_id(p.cur_order.target))
        .flatten()
        .filter(|&h| w.objects.get(h).is_some_and(|o| o.is_mover()))
}

/// A search result that is written straight into the pilot's `moveOrders.path[pathNum]` (the C++ searches fill the path object they
/// are given, before the path locks are put back).
trait SearchPath {
    fn raw_path(&self) -> Option<&mc2_path::MovePath>;
}

impl SearchPath for planning::Search {
    fn raw_path(&self) -> Option<&mc2_path::MovePath> {
        Some(&self.path)
    }
}

impl SearchPath for planning::Segment {
    fn raw_path(&self) -> Option<&mc2_path::MovePath> {
        match self {
            planning::Segment::Found(s) => Some(&s.path),
            planning::Segment::DoorBlocked => None,
        }
    }
}

/// The mover's search wrapper: `updatePathLock(false)`, ram target lock release, `AVOID_PATHLOCKS` for non-elementals, the search, the
/// raw path left in `moveOrders.path[pathNum]` and the locks put back (warrior.cpp:2958-2979 and the same block of the complex
/// branch). The locks put back are those of the new path: `Mover::updatePathLock(true)` reads the pilot's current path, which the
/// search has just filled.
fn with_path_locks_released<T: SearchPath>(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    path_num: usize,
    params: &mut u32,
    search: impl FnOnce(&mut World, u32) -> Result<T, MoverDecisionUnavailable>,
) -> Result<T, MoverDecisionUnavailable> {
    crate::mover::update_path_lock(w, h, false);
    let ram = ram_object(w, slot);
    if let Some(r) = ram {
        crate::mover::update_path_lock(w, r, false);
    }
    if w.objects
        .get(h)
        .is_some_and(|o| o.c.class != crate::object::ObjectClass::Elemental)
    {
        *params |= MOVEPARAM_AVOID_PATHLOCKS;
    }
    let result = search(w, *params | MOVEPARAM_STATIONARY_MOVERS);
    if let Ok(found) = &result {
        // `Mover::calcMovePath` starts with `path->clear()`, so a search that found nothing leaves a cleared path
        let raw = found.raw_path().cloned().unwrap_or_default();
        if let Some(m) = moves(w, slot) {
            m.paths[path_num] = raw;
            if m.paths[path_num].num_steps == 0 {
                mo::clear_move_path(&mut m.paths[path_num]);
            }
        }
    }
    if let Some(r) = ram {
        crate::mover::update_path_lock(w, r, true);
    }
    crate::mover::update_path_lock(w, h, true);
    result
}

/// `Mover::calcMoveGoal` for the goals of move orders (mover.cpp:4400-4483): the revised goal. Goal scoring for attack orders is the
/// mover lane's ([`crate::mover::calc_move_goal_with_params`]).
#[allow(clippy::too_many_arguments)]
fn calc_move_goal(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    target: Option<ObjectHandle>,
    goal: Vec3,
    params: u32,
) -> Result<Vec3, MoverDecisionUnavailable> {
    let position = w.objects.get(h).map_or(Vec3::default(), |o| o.c.position);
    if params & MOVEPARAM_MYSTERY_PARAM != 0 {
        return Ok(goal);
    }
    if params & MOVEPARAM_STEP_TOWARD_TARGET != 0 {
        let mut aim = Vec3 {
            x: goal.x - position.x,
            y: goal.y - position.y,
            z: 0.0,
        };
        let length = crate::math::approximate_length(&aim);
        if length > 0.0 {
            aim.x /= length;
            aim.y /= length;
            let step_length = crate::terrain::WORLD_UNITS_PER_VERTEX * 1.5;
            aim.x *= step_length;
            aim.y *= step_length;
            aim.z = w.terrain.elevation(&aim);
            let target_goal = Vec3 {
                x: position.x + aim.x,
                y: position.y + aim.y,
                z: position.z + aim.z,
            };
            return Ok(crate::mover::calc_offset_move_goal(
                w,
                position,
                target_goal,
            ));
        }
        return Ok(goal);
    }
    if params & MOVEPARAM_STEP_ADJACENT_TARGET != 0 {
        return Ok(crate::mover::calc_offset_move_goal(w, position, goal));
    }
    let is_attack = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.is_combat_order());
    if target.is_none() && !is_attack {
        return Ok(crate::mover::calc_offset_move_goal(w, position, goal));
    }
    // C++ calcMoveGoal applies the 61x61 scoring map to attack orders and object goals.
    match crate::mover::calc_move_goal_to(w, h, params, Some(goal))? {
        Some(g) => Ok(g),
        None => Ok(goal),
    }
}

/// `MechWarrior::calcMovePath(selectionIndex, moveParams)`: returns the `MOVEPATH_ERR_*` code (`NO_ERR` on success).
pub fn calc_move_path(
    w: &mut World,
    slot: u16,
    selection_index: i32,
    move_params: u32,
) -> Result<i32, MoverDecisionUnavailable> {
    let mut move_params = move_params;
    let Some(h) = warrior::vehicle(w, slot) else {
        return Ok(NO_ERR);
    };
    let Some(obj) = w.objects.get(h) else {
        return Ok(NO_ERR);
    };
    let level = obj.move_level().clamp(0, 2) as usize;
    let flying = level > 0;
    let jump_goal = super::mover_ops::is_jumping(w, h);
    let jumping = jump_goal.is_some();
    let mut start = jump_goal.unwrap_or(obj.c.position);

    // If we're starting on a blocked cell, let's use our last valid position as our start...
    if !flying && !jumping {
        let (row, col) = (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1));
        if !(w.terrain.in_bounds(row, col) && w.terrain.map().cell(row, col).passable()) {
            start = match &obj.kind {
                crate::object::Kind::Mech(m) => m.m.last_valid_position,
                crate::object::Kind::Vehicle(v) => v.m.last_valid_position,
                _ => start,
            };
        }
    }

    let (pos_row, pos_col) = w.terrain.world_to_cell(&start);
    let start_area = w.terrain.move_data().global[level].calc_area(pos_row, pos_col);
    let escape_tile = move_params & MOVEPARAM_ESCAPE_TILE != 0;

    let (goal_type, mut goal, goal_object) = warrior::get_move_goal(w, slot);
    if goal_type == MOVEGOAL_NONE {
        return fail(w, slot, mo::err::NO_GOAL);
    }
    let target = goal_target(w, goal_type, goal_object);
    if goal_type > 0 && target.is_none() {
        // If the object no longer exists, we'll stay put...
        return fail(w, slot, mo::err::GOALOBJ_DEAD);
    }

    let mut path_num: i32 = -1;
    if move_params & MOVEPARAM_INIT != 0 {
        if let Some(m) = moves(w, slot) {
            m.path_type = mo::MOVEPATH_UNDEFINED;
            m.num_global_steps = 0;
            m.original_global_goal[0] = goal;
            if matches!(
                m.move_state_goal,
                mo::MOVESTATE_PIVOT_FORWARD
                    | mo::MOVESTATE_PIVOT_REVERSE
                    | mo::MOVESTATE_PIVOT_TARGET
            ) {
                m.move_state = mo::MOVESTATE_FORWARD;
                m.move_state_goal = mo::MOVESTATE_FORWARD;
            }
        }
        path_num = 0;
    }

    let yielding = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.is_yielding);
    if move_params & MOVEPARAM_RECALC != 0 {
        if let Some(t) = target {
            let pos = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
            let wid = i64::from(w.objects.watch_id(t).0);
            warrior::set_move_goal(w, slot, wid, Some(pos));
        } else {
            let original = w
                .pilots
                .get(slot)
                .map_or(Vec3::default(), |p| p.lane_b_move.original_global_goal[0]);
            warrior::set_move_goal(w, slot, MOVEGOAL_LOCATION, Some(original));
        }
        goal = warrior::get_move_goal(w, slot).1;
        if let Some(m) = moves(w, slot) {
            for path in &mut m.paths {
                mo::clear_move_path(path);
            }
            m.path_type = mo::MOVEPATH_UNDEFINED;
            m.num_global_steps = 0;
            m.move_state = mo::MOVESTATE_FORWARD;
            m.move_state_goal = mo::MOVESTATE_FORWARD;
        }
        path_num = 0;
    }

    if goal.x < -666_000.0 {
        return Ok(NO_ERR);
    }

    // Are we calcing the current or next path?
    if path_num == -1 {
        let current_empty = w
            .pilots
            .get(slot)
            .is_some_and(|p| p.lane_b_move.paths[0].num_steps_when_not_paused == 0);
        path_num = i32::from(!current_empty);
    }
    let path_num = path_num as usize;

    let path_type = w.pilots.get(slot).map_or(0, |p| p.lane_b_move.path_type);
    if path_type == mo::MOVEPATH_UNDEFINED {
        // Must be a new move order, so let's first calc the actual move goal...
        if !escape_tile {
            let order_is_player_move = w.pilots.get(slot).is_some_and(|p| {
                p.cur_order.code != OrderCode::None && p.cur_order.origin == OrderOrigin::Player
            });
            if warrior::commander_is_home(w, slot) && order_is_player_move {
                move_params |= MOVEPARAM_PLAYER;
            }
            goal = calc_move_goal(w, h, slot, target, goal, move_params)?;
        }
        // Save the revised final goal...
        if let Some(m) = moves(w, slot) {
            m.original_global_goal[1] = goal;
        }

        if escape_tile {
            // The alarm order's search: a short-range path off the bad tile (warrior.cpp:2886-2940).
            if let Some(m) = moves(w, slot) {
                m.path_type = mo::MOVEPATH_SIMPLE;
            }
            let search =
                with_path_locks_released(w, h, slot, path_num, &mut move_params, |w, params| {
                    planning::escape_search(w, h, start, goal, params)
                })?;
            let num_steps = search.path.num_steps;
            if num_steps == 0 {
                return fail(w, slot, mo::err::LR_NOT_ENABLED);
            }
            install_path(w, slot, path_num, search.path, num_steps);
            if let Some(t) = target {
                set_path_target(w, slot, path_num, t);
            }
            let wid = goal_type_for(w, target);
            warrior::set_move_goal(w, slot, wid, Some(goal));
            let next_update = w.clock.scenario_time + movement_update_frequency(w);
            if let Some(m) = moves(w, slot) {
                m.next_update = next_update;
            }
            if path_num == 0 {
                apply_yield_state(w, slot, path_num, yielding);
            }
            return Ok(NO_ERR);
        }

        let face_target = move_params & MOVEPARAM_FACE_TARGET != 0;
        if super::mover_ops::distance_from(w, h, &goal) < margin_of_error_far(w) && !face_target {
            if let Some(m) = moves(w, slot) {
                m.clear_goal();
            }
            return fail(w, slot, mo::err::ALREADY_THERE);
        }

        // If the distance between the start and goal is within our "quickmove" range, don't do any global pathfinding...
        let (goal_row, goal_col) = w.terrain.world_to_cell(&goal);
        let simple_range = planning::simple_range(w);
        let do_quick_move = (goal_row - pos_row).abs() <= simple_range
            && (goal_col - pos_col).abs() <= simple_range;

        let start_area_open = start_area >= 0
            && w.terrain.move_data().global[level]
                .areas()
                .get(start_area as usize)
                .is_some_and(|a| a.open || a.off_map);

        let mut quick = do_quick_move;
        if do_quick_move {
            if let Some(m) = moves(w, slot) {
                m.path_type = mo::MOVEPATH_SIMPLE;
            }
            let search =
                with_path_locks_released(w, h, slot, path_num, &mut move_params, |w, params| {
                    planning::simple_search(w, h, start, goal, params)
                })?;
            let mut num_steps = search.path.num_steps as i64;
            let found_path = num_steps > 0;
            if num_steps > 0 && selection_index > 0 {
                // We're supposed to stop early on this path...
                let trail = group_move_trail_len(w);
                num_steps -= trail_cut(selection_index, trail);
                if num_steps <= 0 {
                    // Found a path, but it's not far enough to even move!
                    warrior::clear_move_orders(w, slot);
                    return fail(w, slot, mo::err::ALREADY_THERE);
                }
            }
            if num_steps > 0 {
                let mut path = search.path;
                path.goal = goal;
                install_path(w, slot, path_num, path, num_steps as usize);
                if let Some(t) = target {
                    set_path_target(w, slot, path_num, t);
                }
                let wid = goal_type_for(w, target);
                warrior::set_move_goal(w, slot, wid, Some(goal));
                let next_update = w.clock.scenario_time + movement_update_frequency(w);
                if let Some(m) = moves(w, slot) {
                    m.next_update = next_update;
                }
            } else if !found_path && start_area == -1 {
                escape_alarm_order(w, slot, goal);
                return fail(w, slot, mo::err::ESCAPING_TILE);
            } else if !found_path {
                quick = false;
            } else {
                return fail(w, slot, mo::err::LR_NOT_ENABLED);
            }
        }

        if !quick {
            // The path must be a multi-part, or "complex", path that travels across globalmap areas/doors...
            if let Some(m) = moves(w, slot) {
                m.global_goal_location = goal;
            }
            if let Some(p) = w.pilots.get_mut(slot) {
                set_way_point0(&mut p.cur_order, goal);
            }
            let goal_area = w.terrain.move_data().global[level].calc_area(goal_row, goal_col);

            let mut num_steps: i32 = -1;
            let mut steps = Vec::new();
            if start_area_open {
                let use_goal_plan = w.pilots.get(slot).is_some_and(|p| p.use_goal_plan);
                let functional = w
                    .objects
                    .get(h)
                    .and_then(crate::combat::loadout_of)
                    .map_or(0, |l| l.stats.num_functional_weapons);
                let team = w.objects.get(h).map_or(-1, |o| o.team_id());
                let gates = crate::object::gate_oracle::GateSnapshot::from_store(&w.objects);
                let map: &mut GlobalMap = &mut w.terrain.move_data_mut().global[level];
                if use_goal_plan && functional > 0 {
                    map.use_closed_areas = true;
                }
                map.mover_team_id = team as i8;
                let mut ctx = mc2_path::movemap::PathContext::new();
                let result = map.calc_path(
                    &mut ctx,
                    start_area,
                    goal_area,
                    Some((pos_row, pos_col)),
                    Some((goal_row, goal_col)),
                    &gates,
                );
                map.use_closed_areas = false;
                match result {
                    Ok(path) => {
                        num_steps = path.len() as i32;
                        steps = path;
                    }
                    Err(_) => num_steps = -1,
                }
            }
            if num_steps == -1 {
                // Starting on a non-area (i.e. bad:) tile, so try to find a SR escape route...
                escape_alarm_order(w, slot, goal);
                return fail(w, slot, mo::err::ESCAPING_TILE);
            } else if num_steps == 0 {
                warrior::clear_move_orders(w, slot);
                warrior::trigger_alarm(w, slot, alarm::NO_MOVEPATH, mo::err::LR_NO_PATH);
                if move_params & MOVEPARAM_RADIO_RESULT != 0 {
                    warrior::clear_cur_tac_order(w, slot, true);
                    warrior::radio_message(w, slot, super::orders::radio::MOVE_BLOCKED);
                }
                return Ok(mo::err::LR_NO_PATH);
            }
            if let Some(m) = moves(w, slot) {
                m.global_path.resize(
                    m.global_path.len().max(steps.len()),
                    mo::GlobalStep::default(),
                );
                for (dst, src) in m.global_path.iter_mut().zip(&steps) {
                    dst.thru_area = src.thru_area;
                    dst.goal_door = src.goal_door;
                    dst.cost_to_goal = src.cost_to_goal;
                }
                m.path_type = mo::MOVEPATH_COMPLEX;
                m.num_global_steps = num_steps;
                m.cur_global_step = 0;
            }
        }
    } else if path_type == mo::MOVEPATH_COMPLEX {
        if let Some(m) = moves(w, slot) {
            m.cur_global_step += 1;
        }
    }

    let path_type = w.pilots.get(slot).map_or(0, |p| p.lane_b_move.path_type);
    if path_type == mo::MOVEPATH_COMPLEX {
        if let Some(result) = complex_step(
            w,
            slot,
            h,
            start,
            goal,
            target,
            path_num,
            selection_index,
            &mut move_params,
        )? {
            return Ok(result);
        }
    } else if path_type != mo::MOVEPATH_SIMPLE && path_type != mo::MOVEPATH_UNDEFINED {
        panic!("Bad Move Path Type");
    }

    if path_num == 0 {
        apply_yield_state(w, slot, path_num, yielding);
    }
    Ok(NO_ERR)
}

/// The `if (pathNum == 0) { if (yielding) ... }` tail shared by every successful exit.
fn apply_yield_state(w: &mut World, slot: u16, path_num: usize, yielding: bool) {
    let now = w.clock.scenario_time;
    let move_yield_time = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or(1.5, |t| t.pathfinding.move_yield_time);
    let _ = move_yield_time;
    let Some(m) = moves(w, slot) else { return };
    if yielding {
        m.set_yield_time(now + 1.5);
        m.paths[path_num].num_steps = 0;
    } else if m.is_waiting_for_point() {
        m.paths[path_num].num_steps = 0;
    } else {
        m.set_yield_time(-1.0);
        m.yield_state = 0;
    }
}

/// The complex-path part of `calcMovePath` for the current global step (warrior.cpp:3170-3290). `Some(err)` is an early return.
#[allow(clippy::too_many_arguments)]
fn complex_step(
    w: &mut World,
    slot: u16,
    h: ObjectHandle,
    start: Vec3,
    goal: Vec3,
    target: Option<ObjectHandle>,
    path_num: usize,
    selection_index: i32,
    move_params: &mut u32,
) -> Result<Option<i32>, MoverDecisionUnavailable> {
    let mut start = start;
    let mut goal = goal;
    let (cur_global_step, num_global_steps) = {
        let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
        (m.cur_global_step, m.num_global_steps)
    };
    let mut cur_global_step = cur_global_step;
    if cur_global_step == num_global_steps {
        // We've finished the global path.
        return Ok(Some(NO_ERR));
    }
    let level = w.objects.get(h).map_or(0, |o| o.move_level().clamp(0, 2)) as usize;

    // Our goal is determined by our current global step in our global path...
    if cur_global_step > 0 && cur_global_step < num_global_steps - 1 {
        let prev = w.pilots.get(slot).expect("pilot").lane_b_move.global_path
            [(cur_global_step - 1) as usize];
        start = w
            .terrain
            .cell_to_world(prev.goal_cell[0], prev.goal_cell[1]);
    }
    let step = w.pilots.get(slot).expect("pilot").lane_b_move.global_path[cur_global_step as usize];
    if cur_global_step < num_global_steps - 1 {
        let open = w.terrain.move_data().global[level]
            .door(step.goal_door as usize)
            .open;
        if !open {
            let now = w.clock.scenario_time;
            if let Some(m) = moves(w, slot) {
                m.way_path.clear();
                m.cur_way_pt = 0;
                m.cur_way_dir = 0;
                m.time_of_last_step = now;
                m.num_global_steps = 0;
                m.cur_global_step = 0;
            }
            warrior::request_move_path(
                w,
                slot,
                selection_index,
                MOVEPARAM_RECALC + MOVEPARAM_FACE_TARGET,
                19,
            );
            warrior::trigger_alarm(w, slot, alarm::NO_MOVEPATH, mo::err::LR_DOOR_CLOSED);
            return Ok(Some(mo::err::LR_DOOR_CLOSED));
        }
    }

    let mut num_steps: i64 = 0;
    if cur_global_step < num_global_steps - 2 {
        if let Some(m) = moves(w, slot) {
            mo::clear_move_path(&mut m.paths[path_num]);
        }
        let global_goal = w
            .pilots
            .get(slot)
            .expect("pilot")
            .lane_b_move
            .global_goal_location;
        let next = w.pilots.get(slot).expect("pilot").lane_b_move.global_path
            [(cur_global_step + 1) as usize];
        let thru_area = [step.thru_area, next.thru_area];
        let segment = with_path_locks_released(w, h, slot, path_num, move_params, |w, params| {
            planning::segment_search(w, h, start, thru_area, next.goal_door, global_goal, params)
        })?;
        match segment {
            planning::Segment::DoorBlocked => num_steps = -999,
            planning::Segment::Found(search) => {
                num_steps = search.path.num_steps as i64;
                let path = search.path;
                if num_steps > 0 {
                    // `goalWorldPos` / `goalCell` outputs: the segment's own goal replaces the goal
                    goal = search.goal_world.unwrap_or(path.goal);
                    if let Some(m) = moves(w, slot) {
                        m.global_path[cur_global_step as usize].goal_cell =
                            [search.goal_cell.0, search.goal_cell.1];
                    }
                    install_path(w, slot, path_num, path, num_steps as usize);
                }
            }
        }
    } else if cur_global_step == num_global_steps - 2 {
        if let Some(m) = moves(w, slot) {
            mo::clear_move_path(&mut m.paths[path_num]);
        }
        goal = w
            .pilots
            .get(slot)
            .expect("pilot")
            .lane_b_move
            .original_global_goal[1];
        let search = with_path_locks_released(w, h, slot, path_num, move_params, |w, params| {
            planning::window_search(w, h, start, goal, params)
        })?;
        num_steps = search.path.num_steps as i64;
        if num_steps > 0 {
            let mut path = search.path;
            path.goal = goal;
            install_path(w, slot, path_num, path, num_steps as usize);
            if selection_index > 0 {
                // We're supposed to stop early on this path...
                let trail = group_move_trail_len(w);
                num_steps -= trail_cut(selection_index, trail);
                if num_steps <= 0 {
                    warrior::clear_move_orders(w, slot);
                    return Ok(Some(fail(w, slot, mo::err::ALREADY_THERE)?));
                }
                // `numSteps` is only written back below
                if path_num == 0 {
                    let last = w.pilots.get(slot).expect("pilot").lane_b_move.paths[path_num]
                        .steps
                        .get(num_steps as usize - 1)
                        .map(|s| s.destination);
                    if let Some(loc) = last {
                        if let Some(m) = moves(w, slot) {
                            m.global_goal_location = loc;
                        }
                        if let Some(p) = w.pilots.get_mut(slot) {
                            set_way_point0(&mut p.cur_order, loc);
                        }
                    }
                }
            }
            cur_global_step += 1;
        }
    } else if cur_global_step != num_global_steps - 1 {
        panic!("Mechwarrior.calcMovePath: bad global step ({cur_global_step}/{num_global_steps})");
    }

    if num_steps > 0 {
        if let Some(m) = moves(w, slot) {
            let path = &mut m.paths[path_num];
            path.num_steps = num_steps as usize;
            path.num_steps_when_not_paused = num_steps as usize;
            path.global_step = cur_global_step;
        }
        if path_num == 0 {
            if let Some(t) = target {
                set_path_target(w, slot, path_num, t);
            }
            let wid = goal_type_for(w, target);
            warrior::set_move_goal(w, slot, wid, Some(goal));
        }
    } else if cur_global_step != num_global_steps - 1 {
        if let Some(m) = moves(w, slot) {
            m.cur_global_step -= 1;
        }
        let err = if num_steps == -999 {
            mo::err::LR_DOOR_BLOCKED
        } else {
            mo::err::LR_NO_SR_PATH
        };
        warrior::trigger_alarm(w, slot, alarm::NO_MOVEPATH, err);
        return Ok(Some(err));
    }
    Ok(None)
}

/// `setMoveGoal((target ? target->getWatchID() : MOVEGOAL_LOCATION), ...)`'s type.
fn goal_type_for(w: &mut World, target: Option<ObjectHandle>) -> i64 {
    match target {
        Some(t) => i64::from(w.objects.watch_id(t).0),
        None => MOVEGOAL_LOCATION,
    }
}

/// `moveOrders.path[pathNum]->target = target->getPosition()`.
fn set_path_target(w: &mut World, slot: u16, path_num: usize, target: ObjectHandle) {
    let pos = w
        .objects
        .get(target)
        .map_or(Vec3::default(), |o| o.c.position);
    if let Some(m) = moves(w, slot) {
        m.paths[path_num].target = pos;
    }
}

/// Store a searched path in `moveOrders.path[pathNum]` with `numSteps = numStepsWhenNotPaused = num_steps`; for the simple and the
/// escape paths also `globalGoalLocation` and the first way point of the current order (the last step's destination).
fn install_path(
    w: &mut World,
    slot: u16,
    path_num: usize,
    path: mc2_path::MovePath,
    num_steps: usize,
) {
    let simple = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.path_type == mo::MOVEPATH_SIMPLE);
    let mut last = None;
    if let Some(m) = moves(w, slot) {
        m.paths[path_num] = path;
        let path = &mut m.paths[path_num];
        path.num_steps = num_steps;
        path.num_steps_when_not_paused = num_steps;
        if simple {
            last = path.steps.get(num_steps - 1).map(|s| s.destination);
            if let Some(loc) = last {
                m.global_goal_location = loc;
            }
        }
    }
    if let (Some(loc), Some(p)) = (last, w.pilots.get_mut(slot)) {
        set_way_point0(&mut p.cur_order, loc);
    }
}

/// The escape alarm order (warrior.cpp:3045-3060): a move-to-point order of the pilot itself with `escapeTile`.
fn escape_alarm_order(w: &mut World, slot: u16, goal: Vec3) {
    let now = w.clock.scenario_time;
    let run = w.pilots.get(slot).is_some_and(|p| p.lane_b_move.run);
    let mut order = TacticalOrder::new(OrderOrigin::SelfOrder, OrderCode::MoveToPoint, false, now);
    set_way_point0(&mut order, goal);
    order.move_params.escape_tile = true;
    order.move_params.wait = false;
    let mode = if run {
        TravelMode::Fast
    } else {
        TravelMode::Slow
    };
    match order.move_params.way_path.mode.first_mut() {
        Some(m) => *m = mode,
        None => order.move_params.way_path.mode.push(mode),
    }
    warrior::set_alarm_tac_order(w, slot, order, 255);
}
