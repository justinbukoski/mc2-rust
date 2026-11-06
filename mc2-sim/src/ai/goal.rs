// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The goal plan: `MechWarrior::calcTacOrder` (warrior.cpp:7592) and `calcTurretThreats` (warrior.cpp:2237), run by `runBrain` for
//! every pilot not under the home commander (`useGoalPlan`). The brain's orders (`coreMoveTo`, `coreAttack`) set a main goal; this
//! turns the main goal into the sub-order the pilot executes (a move to the goal, or an attack on the gate/wall in the way).
//!
//! `Mover::calcGlobalPath` (mover.cpp:4291) is the lane B global area search over `MoveData::global[moveLevel]`.

use super::mover_ops;
use super::pilot::goal_action;
use super::warrior::{self, vehicle};
use crate::command::{AttackMethod, OrderCode, OrderOrigin, TacticalOrder, FIRERANGE_OPTIMAL};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::object::{flags, Kind, ObjectClass};
use crate::World;
use mc2_path::global::{AreaType, GlobalPathStep};

/// `Building::calcAdjacentAreaCell(moveLevel, areaID, ...)` (terrobj.cpp:1138-1194).
pub fn calc_adjacent_area_cell(
    w: &World,
    building: ObjectHandle,
    mover: ObjectHandle,
    area_id: i32,
) -> Option<Vec3> {
    let level = w.objects.get(mover)?.move_level().clamp(0, 2) as usize;
    let cells = w.objects.get(building)?.structure()?.cells_covered.clone();
    let global = &w.terrain.move_data().global[level];
    for (row, col) in cells {
        let (row, col) = (i32::from(row), i32::from(col));
        for (r, c) in [
            (row - 1, col),
            (row, col + 1),
            (row + 1, col),
            (row, col - 1),
        ] {
            let area = global.calc_area(r, c);
            if if area_id == -1 {
                area > -1
            } else {
                area == area_id
            } {
                return Some(w.terrain.cell_to_world(r, c));
            }
        }
    }
    None
}

fn thread_of(
    order: &TacticalOrder,
) -> (
    crate::command::TravelMode,
    bool,
    bool,
    crate::command::Tactic,
) {
    (
        order
            .move_params
            .way_path
            .mode
            .first()
            .copied()
            .unwrap_or_default(),
        order.move_params.jump,
        order.move_params.keep_moving,
        order.attack_params.tactic,
    )
}

/// `Mover::calcGlobalPath(globalPath, obj, location, useClosedAreas)` (mover.cpp:4291): the area steps from the mover's cell to the
/// object's (or location's) cell, 0 steps when there is no goal area, -1 when the start area is closed.
pub fn calc_global_path(
    w: &mut World,
    me: ObjectHandle,
    obj: Option<ObjectHandle>,
    location: Option<Vec3>,
    use_closed_areas: bool,
) -> (i32, Vec<GlobalPathStep>) {
    let Some(mo) = w.objects.get(me) else {
        return (0, Vec::new());
    };
    let level = mo.move_level().clamp(0, 2) as usize;
    let (my_row, my_col) = (i32::from(mo.c.cell.0), i32::from(mo.c.cell.1));
    let start_area = w.terrain.move_data().global[level].calc_area(my_row, my_col);
    let mut goal_cell = (-1, -1);
    let mut goal_area = -1;
    if let Some(o) = obj {
        let Some(t) = w.objects.get(o) else {
            return (0, Vec::new());
        };
        goal_area = w.terrain.move_data().global[level]
            .calc_area(i32::from(t.c.cell.0), i32::from(t.c.cell.1));
        if goal_area == -1 {
            let pos = t.c.position;
            let is_building = matches!(t.kind, Kind::Building(_));
            let goal_pos = if is_building {
                calc_adjacent_area_cell(w, o, me, -1)
                    .unwrap_or_else(|| mover_ops::calc_offset_move_goal(w, me, &pos))
            } else {
                mover_ops::calc_offset_move_goal(w, me, &pos)
            };
            goal_cell = w.terrain.world_to_cell(&goal_pos);
            goal_area = w.terrain.move_data().global[level].calc_area(goal_cell.0, goal_cell.1);
        }
    } else if let Some(l) = location {
        goal_cell = w.terrain.world_to_cell(&l);
        goal_area = w.terrain.move_data().global[level].calc_area(goal_cell.0, goal_cell.1);
    }
    if goal_area == -1 {
        return (0, Vec::new());
    }
    let start_open = start_area >= 0
        && w.terrain.move_data().global[level]
            .areas()
            .get(start_area as usize)
            .is_some_and(|a| a.open);
    if !start_open {
        return (-1, Vec::new());
    }
    let functional = w
        .objects
        .get(me)
        .and_then(crate::combat::loadout_of)
        .map_or(0, |l| l.stats.num_functional_weapons);
    let team = w.objects.get(me).map_or(-1, |o| o.team_id());
    let gates = crate::object::gate_oracle::GateSnapshot::from_store(&w.objects);
    let map = &mut w.terrain.move_data_mut().global[level];
    if functional > 0 {
        map.use_closed_areas = use_closed_areas;
    }
    map.mover_team_id = team as i8;
    let mut ctx = mc2_path::movemap::PathContext::new();
    let result = map.calc_path(
        &mut ctx,
        start_area,
        goal_area,
        Some((my_row, my_col)),
        Some(goal_cell),
        &gates,
    );
    map.use_closed_areas = false;
    match result {
        Ok(steps) => (steps.len() as i32, steps),
        Err(_) => (-1, Vec::new()),
    }
}

/// `MechWarrior::calcTurretThreats(threatRange, minThreat)` (warrior.cpp:2237): the enemy turret control building whose turrets that
/// aim at this pilot's vehicle add up to the biggest threat (at least `minThreat`) within `threatRange`.
pub fn calc_turret_threats(
    w: &World,
    slot: u16,
    threat_range: f32,
    min_threat: i32,
) -> Option<ObjectHandle> {
    let min_threat = min_threat.max(1);
    let me = vehicle(w, slot)?;
    let my_wid = w.pilots.get(slot)?.mover;
    let my_team = w.objects.get(me)?.team_id();
    let controls = &w.objects.turret_controls;
    let mut control_threat = vec![0i32; controls.len()];
    for &t in &w.objects.turrets {
        let Some(o) = w.objects.get(t) else { continue };
        let Kind::Turret(ts) = &o.kind else { continue };
        if !o.c.is_disabled() && ts.target == my_wid {
            let rating = i32::from(o.c.threat_rating);
            if ts.parent != WatchId(0) {
                if let Some(Kind::Building(b)) =
                    w.objects.get_by_watch_id(ts.parent).map(|p| &p.kind)
                {
                    if let Some(slot) = control_threat.get_mut(usize::from(b.list_id)) {
                        *slot += rating;
                    }
                }
            }
        }
    }
    let mut biggest: Option<usize> = None;
    for (i, &c) in controls.iter().enumerate() {
        let Some(cb) = w.objects.get(c) else { continue };
        let distance = mover_ops::distance_from(w, me, &cb.c.position);
        if distance > threat_range {
            control_threat[i] = 0;
        }
        if cb.team_id() == my_team {
            control_threat[i] = 0;
        }
        if cb.c.is_disabled() {
            control_threat[i] = 0;
        }
        if control_threat[i] < min_threat {
            control_threat[i] = 0;
        } else {
            match biggest {
                None => biggest = Some(i),
                Some(b) if control_threat[i] > control_threat[b] => biggest = Some(i),
                _ => {}
            }
        }
    }
    biggest.map(|i| controls[i])
}

/// `MechWarrior::calcTacOrder` (warrior.cpp:7592-8097). Returns 0 with `new_tac_order` set (code `NONE` when the plan has nothing to
/// order), -1 when the goal object is gone, 1 when the goal is done (an attack target already disabled).
#[allow(clippy::too_many_arguments)]
pub fn calc_tac_order(
    w: &mut World,
    slot: u16,
    goal_action_in: i32,
    goal_wid: WatchId,
    goal_location: Vec3,
    control_radius: f32,
    aggressiveness: i32,
    search_depth: i32,
    turret_range: f32,
    turret_threat: i32,
    new_tac_order: &mut TacticalOrder,
) -> i32 {
    let Some(me) = vehicle(w, slot) else {
        return 0;
    };
    let class = w.objects.get(me).map(|o| o.c.class);
    let can_capture = class != Some(ObjectClass::GroundVehicle);
    let mut goal_action_v = goal_action_in;
    let mut goal_wid = goal_wid;
    let mut goal_location = goal_location;
    let mut turret_threat = turret_threat;

    // If we have no goal, we may want to at least assess turret threat...
    let mut already_assessed_turrets = false;
    if goal_wid == WatchId(0) && turret_range > 0.0 {
        if let Some(t) = calc_turret_threats(w, slot, turret_range, turret_threat) {
            let is_turret = w
                .objects
                .get(t)
                .is_some_and(|o| o.c.class == ObjectClass::Turret);
            goal_action_v = if is_turret {
                goal_action::ATTACK
            } else {
                if can_capture && crate::ai::capture::blockers(w, t, me).is_empty() {
                    goal_action::CAPTURE
                } else {
                    goal_action::ATTACK
                }
            };
            goal_wid = w.objects.watch_id(t);
            goal_location = Vec3::default();
        }
        already_assessed_turrets = true;
    }

    // At this point, if we don't have any goal, we're done.
    let mut main_target: Option<ObjectHandle> = None;
    if goal_wid != WatchId(0) {
        main_target = w.objects.by_watch_id(goal_wid);
        if main_target.is_none() {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.main_goal.action = goal_action::NONE;
                p.main_goal.object = WatchId(0);
                p.main_goal.location = Vec3::default();
                p.last_goal_path.clear();
            }
            return -1;
        }
    }

    // Preserve our original goal, as it may change short-term...
    let mut current_action = goal_action_v;
    let mut current_location = goal_location;
    let mut current_target = main_target;

    let order_origin = if warrior::commander_is_home(w, slot) {
        OrderOrigin::Player
    } else {
        OrderOrigin::Commander
    };

    if current_action == goal_action::UNDECIDED {
        // Pick one for me...
        if let Some(t) = current_target {
            let my_team = w.objects.get(me).map_or(-1, |o| o.team_id());
            let (is_mover, team, disabled, cls, control) = {
                let o = w.objects.get(t).expect("target");
                (
                    o.is_mover(),
                    o.team_id(),
                    o.c.is_disabled(),
                    o.c.class,
                    o.c.flag(flags::CONTROLBUILDING),
                )
            };
            if is_mover {
                if crate::object::hooks::team_relation(w, my_team, team)
                    == Some(super::Relation::Enemy)
                {
                    if !disabled {
                        current_action = goal_action::ATTACK;
                    }
                } else if !disabled {
                    current_action = goal_action::GUARD;
                }
            } else {
                match cls {
                    ObjectClass::Building => {
                        if control {
                            current_action = goal_action::ATTACK;
                        }
                    }
                    ObjectClass::Turret => current_action = goal_action::ATTACK,
                    _ => {}
                }
            }
        }
    }

    if !already_assessed_turrets && turret_range > 0.0 {
        if current_action == goal_action::ATTACK {
            if let Some(lt) = warrior::get_last_target(w, slot) {
                let rating = i32::from(w.objects.get(lt).map_or(0, |o| o.c.threat_rating));
                if rating > turret_threat {
                    turret_threat = rating;
                }
            }
        }
        if let Some(t) = calc_turret_threats(w, slot, turret_range, turret_threat) {
            let is_turret = w
                .objects
                .get(t)
                .is_some_and(|o| o.c.class == ObjectClass::Turret);
            if is_turret {
                current_action = goal_action::ATTACK;
            } else {
                current_action = if can_capture && crate::ai::capture::blockers(w, t, me).is_empty()
                {
                    goal_action::CAPTURE
                } else {
                    goal_action::ATTACK
                };
            }
            current_location = Vec3::default();
            current_target = Some(t);
        }
    }

    let newest = {
        let p = w.pilots.get(slot).expect("pilot");
        if p.new_order_received[0] {
            p.tac_orders[0].clone()
        } else if p.new_order_received[1] {
            p.tac_orders[1].clone()
        } else if p.cur_order.code != OrderCode::None {
            p.cur_order.clone()
        } else {
            TacticalOrder::init()
        }
    };
    let (newest_mode, newest_jump, newest_keep_moving, newest_tactic) = thread_of(&newest);

    *new_tac_order = TacticalOrder::new(
        OrderOrigin::Commander,
        OrderCode::None,
        false,
        w.clock.scenario_time,
    );
    let sub_order = |w: &World, origin: OrderOrigin, code: OrderCode| {
        let mut o = TacticalOrder::new(origin, code, false, w.clock.scenario_time);
        o.sub_order = true;
        o
    };

    if current_action == goal_action::MOVE {
        let (num_steps, path) = if let Some(t) = current_target {
            calc_global_path(w, me, Some(t), None, true)
        } else {
            calc_global_path(w, me, None, Some(current_location), true)
        };
        if num_steps > 0 {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.last_goal_path = path.iter().map(|s| s.thru_area as i16).collect();
            }
        }
        // What's the next subgoal to overcome before we can move on?
        if let Some(order) = blocking_sub_order(
            w,
            slot,
            me,
            &path,
            num_steps,
            order_origin,
            can_capture,
            search_depth,
            aggressiveness,
            (newest_mode, newest_jump, newest_keep_moving, newest_tactic),
            true,
            new_tac_order,
        ) {
            return order;
        }
        // If we made it here, then there are no steps in the global path blocked. So, let's just go straight for the main goal...
        if let Some(t) = current_target {
            let (is_mover, is_building, pos) = {
                let o = w.objects.get(t).expect("target");
                (
                    o.is_mover(),
                    matches!(o.kind, Kind::Building(_)),
                    o.c.position,
                )
            };
            let goal = if is_mover {
                pos
            } else {
                if is_building {
                    calc_adjacent_area_cell(w, t, me, -1)
                        .unwrap_or_else(|| mover_ops::calc_offset_move_goal(w, me, &pos))
                } else {
                    mover_ops::calc_offset_move_goal(w, me, &pos)
                }
            };
            let mut o = sub_order(w, order_origin, OrderCode::MoveToObject);
            o.target = w.objects.watch_id(t);
            o.selection_index = -1;
            o.move_params.from_area = -1;
            set_way_point(&mut o, goal);
            set_mode(&mut o, newest_mode);
            o.move_params.face_object = true;
            o.move_params.wait = false;
            o.move_params.mine_laying = false;
            o.move_params.jump = newest_jump;
            o.move_params.keep_moving = newest_keep_moving;
            *new_tac_order = o;
        } else {
            let mut o = sub_order(w, order_origin, OrderCode::MoveToPoint);
            set_way_point(&mut o, current_location);
            set_mode(&mut o, newest_mode);
            o.move_params.wait = false;
            o.move_params.mine_laying = false;
            o.move_params.jump = newest_jump;
            o.move_params.keep_moving = newest_keep_moving;
            *new_tac_order = o;
        }
    } else if current_action == goal_action::ATTACK {
        let Some(t) = current_target else { return 0 };
        let (num_steps, path) = calc_global_path(w, me, Some(t), None, true);
        if num_steps > 0 {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.last_goal_path = path.iter().map(|s| s.thru_area as i16).collect();
            }
        }
        if let Some(order) = blocking_sub_order(
            w,
            slot,
            me,
            &path,
            num_steps,
            order_origin,
            can_capture,
            search_depth,
            aggressiveness,
            (newest_mode, newest_jump, newest_keep_moving, newest_tactic),
            true,
            new_tac_order,
        ) {
            return order;
        }
        // If we made it here, then there are no steps in the global path blocked. So, let's just go straight for the main target...
        if w.objects.get(t).is_some_and(|o| o.c.is_disabled()) {
            return 1;
        }
        let mut o = sub_order(w, order_origin, OrderCode::AttackObject);
        o.target = w.objects.watch_id(t);
        o.attack_params.attack_type = 1;
        o.attack_params.method = AttackMethod::Ranged;
        o.attack_params.range = FIRERANGE_OPTIMAL;
        o.attack_params.tactic = newest_tactic;
        o.attack_params.pursue = true;
        set_mode(&mut o, newest_mode);
        o.move_params.jump = newest_jump;
        o.move_params.keep_moving = newest_keep_moving;
        *new_tac_order = o;
    } else if current_action == goal_action::CAPTURE {
        let Some(target) = current_target else {
            return 0;
        };
        if w.objects
            .get(target)
            .is_some_and(|o| o.team_id() == w.objects.get(me).map_or(-1, |m| m.team_id()))
        {
            return 1;
        }
        let blockers = crate::ai::capture::blockers(w, target, me);
        if let Some(blocker) = blockers.first().copied() {
            let mut o = attack_sub_order(
                w,
                order_origin,
                blocker,
                (newest_mode, newest_jump, newest_keep_moving, newest_tactic),
            );
            o.sub_order = true;
            *new_tac_order = o;
            return 0;
        }
        let (num_steps, path) = calc_global_path(w, me, Some(target), None, true);
        if num_steps > 0 {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.last_goal_path = path.iter().map(|s| s.thru_area as i16).collect();
            }
        }
        if let Some(order) = blocking_sub_order(
            w,
            slot,
            me,
            &path,
            num_steps,
            order_origin,
            can_capture,
            search_depth,
            aggressiveness,
            (newest_mode, newest_jump, newest_keep_moving, newest_tactic),
            false,
            new_tac_order,
        ) {
            return order;
        }
        let mut o = sub_order(w, order_origin, OrderCode::Capture);
        o.target = w.objects.watch_id(target);
        o.attack_params.method = AttackMethod::Ramming;
        o.attack_params.pursue = true;
        o.move_params.from_area = path
            .get(path.len().saturating_sub(2))
            .map_or(-1, |s| s.thru_area);
        set_mode(&mut o, newest_mode);
        o.move_params.jump = newest_jump;
        o.move_params.keep_moving = newest_keep_moving;
        *new_tac_order = o;
    }
    // GOAL_ACTION_GUARD does nothing
    let _ = (control_radius, new_tac_order.code);
    0
}

fn set_way_point(o: &mut TacticalOrder, p: Vec3) {
    if o.move_params.way_path.points.is_empty() {
        o.move_params.way_path.points.push(p);
    } else {
        o.move_params.way_path.points[0] = p;
    }
}

fn set_mode(o: &mut TacticalOrder, mode: crate::command::TravelMode) {
    if o.move_params.way_path.mode.is_empty() {
        o.move_params.way_path.mode.push(mode);
    } else {
        o.move_params.way_path.mode[0] = mode;
    }
}

/// The scan "what's the next subgoal to overcome" over the global path's areas (gate -> capture its control building or shoot the gate;
/// wall -> shoot the wall). Returns `Some(result)` when a blocking area produced the order, `None` when the path is clear.
#[allow(clippy::too_many_arguments)]
fn blocking_sub_order(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    path: &[GlobalPathStep],
    num_steps: i32,
    order_origin: OrderOrigin,
    can_capture: bool,
    search_depth: i32,
    aggressiveness: i32,
    newest: (
        crate::command::TravelMode,
        bool,
        bool,
        crate::command::Tactic,
    ),
    capture_first: bool,
    new_tac_order: &mut TacticalOrder,
) -> Option<i32> {
    let _ = (slot, aggressiveness);
    let level = w.objects.get(me).map_or(0, |o| o.move_level()).clamp(0, 2) as usize;
    let my_team = w.objects.get(me).map_or(-1, |o| o.team_id());
    for step in path.iter().take(num_steps.max(0) as usize) {
        let area = w.terrain.move_data().global[level]
            .areas()
            .get(step.thru_area as usize)
            .copied();
        let Some(area) = area else { continue };
        match area.area_type {
            AreaType::Gate => {
                let gate = w.objects.by_watch_id(WatchId(area.owner_wid as u32));
                if let Some(g) = gate {
                    let (destroyed, team, parent) = {
                        let o = w.objects.get(g).expect("gate");
                        let parent = match &o.kind {
                            Kind::Gate(gs) => gs.parent,
                            _ => WatchId(0),
                        };
                        (o.c.is_destroyed(), o.team_id(), parent)
                    };
                    if !destroyed && team != my_team {
                        let control = w.objects.by_watch_id(parent);
                        if let Some(control) =
                            control.filter(|_| can_capture && search_depth > 0 && capture_first)
                        {
                            let wid = w.objects.watch_id(control);
                            let mut sub = TacticalOrder::init();
                            let result = calc_tac_order(
                                w,
                                slot,
                                goal_action::CAPTURE,
                                wid,
                                Vec3::default(),
                                -1.0,
                                aggressiveness,
                                search_depth - 1,
                                -1.0,
                                0,
                                &mut sub,
                            );
                            *new_tac_order = sub;
                            return Some(result);
                        }
                        let mut o = attack_sub_order(w, order_origin, g, newest);
                        o.sub_order = true;
                        *new_tac_order = o;
                        return Some(0);
                    }
                }
            }
            AreaType::Wall => {
                if let Some(wall) = w.objects.by_watch_id(WatchId(area.owner_wid as u32)) {
                    if !w.objects.get(wall).is_some_and(|o| o.c.is_destroyed()) {
                        let o = attack_sub_order(w, order_origin, wall, newest);
                        *new_tac_order = o;
                        return Some(0);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn attack_sub_order(
    w: &mut World,
    origin: OrderOrigin,
    target: ObjectHandle,
    newest: (
        crate::command::TravelMode,
        bool,
        bool,
        crate::command::Tactic,
    ),
) -> TacticalOrder {
    let mut o = TacticalOrder::new(
        origin,
        OrderCode::AttackObject,
        false,
        w.clock.scenario_time,
    );
    o.sub_order = true;
    o.target = w.objects.watch_id(target);
    o.attack_params.attack_type = 1;
    o.attack_params.method = AttackMethod::Ranged;
    o.attack_params.range = FIRERANGE_OPTIMAL;
    o.attack_params.tactic = newest.3;
    o.attack_params.pursue = true;
    set_mode(&mut o, newest.0);
    o.move_params.jump = newest.1;
    o.move_params.keep_moving = newest.2;
    o
}
