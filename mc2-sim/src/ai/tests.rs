// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Unit tests of the pilot decision code. Every expected value is derived by hand from the C++ named in the comment of the test (file:line
//! of `code/warrior.cpp`, `code/tacordr.cpp`, `code/mover.cpp`); none comes from this port's own output. The populated oracle
//! comparison (`tests/populated_digest.rs`) checks the same code against the real game.

use super::moveorders::{MOVEGOAL_LOCATION, MOVEGOAL_NONE, MOVEPATH_COMPLEX, MOVEPATH_SIMPLE};
use super::pilot::{OrderState, Pilot};
use super::warrior::{self, alarm, TACORDER_FAILURE, TACORDER_SUCCESS};
use super::{brain, tacorder};
use super::{decide, orders, targeting, AiUnavailable};
use crate::combat::InventoryItem;
use crate::command::{AttackMethod, OrderCode, OrderOrigin, TacticalOrder, TravelMode};
use crate::ids::{CommanderId, ObjectHandle, TeamId, WatchId};
use crate::math::Vec3;
use crate::mover::{MechState, MoverCore};
use crate::object::{flags, Common, Kind, Obj, ObjectClass, ObjectStatus};
use crate::rng::RngMode;
use crate::terrain::Terrain;
use crate::SimEvent;
use crate::World;
use std::rc::Rc;

/// A flat 64x64-vertex world with two teams (0 = home), one mech per team and the pilots linked to them. Mech 1 (slot 1, team 0,
/// commander 0) stands at (100, 100); mech 2 (slot 2, team 1) at (300, 100). The gamesys tables are absent, so every update frequency
/// reads as 0 and the fire-range tables as 0 (the tests that need other values set the pilot fields directly).
pub(crate) fn world() -> (World, ObjectHandle, ObjectHandle) {
    let mut w = World::new(RngMode::LegacyMsvc, 1);
    w.terrain = Terrain::from_elevations(64, &vec![0.0; 4096]);
    w.teams.teams[0] = Some(super::team::Team::default());
    w.teams.teams[1] = Some(super::team::Team::default());
    w.teams.home = 0;
    for t in 0..crate::ids::MAX_TEAMS {
        w.teams.relations[t][t] = super::team::Relation::Friendly;
    }
    w.pilots.brains_enabled = [true; crate::ids::MAX_TEAMS];
    w.pilots.slots = (0..3)
        .map(|i| {
            Pilot::fresh(
                i,
                275.0,
                [0.2 * f32::from(i), 0.1 * f32::from(i), 0.2 * f32::from(i)],
            )
        })
        .collect();
    w.objects.list = vec![None];
    let mut handles = Vec::new();
    for (i, (x, team, commander)) in [(100.0, 0, 0), (300.0, 1, 1)].into_iter().enumerate() {
        let handle = ObjectHandle(i as u32 + 1);
        let slot = i as u16 + 1;
        let mut c = Common::new(ObjectClass::BattleMech, handle);
        c.flags |= flags::EXISTS | flags::AWAKE;
        c.part_id = crate::ids::PartId(512 + i as i32);
        c.position = Vec3 {
            x,
            y: 100.0,
            z: 0.0,
        };
        let (row, col) = w.terrain.world_to_cell(&c.position);
        c.cell = (row as u16, col as u16);
        let state = MechState {
            m: MoverCore {
                pilot_slot: slot,
                team: TeamId(team),
                commander: CommanderId(commander),
                selection_index: -1,
                attack_range: crate::command::FIRERANGE_OPTIMAL,
                ..MoverCore::default()
            },
            ..MechState::default()
        };
        w.objects.list.push(Some(Obj {
            c,
            kind: Kind::Mech(Box::new(state)),
        }));
        w.objects.movers.push(handle);
        let wid = w.objects.watch_id(handle);
        w.pilots.slots[usize::from(slot)].mover = wid;
        handles.push(handle);
    }
    (w, handles[0], handles[1])
}

fn order(code: OrderCode, origin: OrderOrigin) -> TacticalOrder {
    TacticalOrder::new(origin, code, false, 0.0)
}

fn point(x: f32, y: f32) -> Vec3 {
    Vec3 { x, y, z: 0.0 }
}

fn cell_world(w: &World, row: i32, col: i32) -> Vec3 {
    w.terrain.cell_to_world(row, col)
}

#[test]
fn group_jump_orders_use_spiral_landing_cells() {
    let (mut w, first, second) = world();
    let mut third = w.objects.get(first).unwrap().clone();
    third.c.part_id = crate::ids::PartId(514);
    if let Kind::Mech(m) = &mut third.kind {
        m.m.pilot_slot = 3;
        m.m.team_roster_index = 2;
    }
    w.pilots.slots.push(Pilot::fresh(3, 275.0, [0.0; 3]));
    w.objects.list.push(Some(third));
    w.objects.movers.push(ObjectHandle(3));
    for handle in [first, second, ObjectHandle(3)] {
        if let Kind::Mech(m) = &mut w.objects.get_mut(handle).unwrap().kind {
            m.num_jump_jets = 255;
        }
    }
    if let Kind::Mech(m) = &mut w.objects.get_mut(second).unwrap().kind {
        m.m.team = TeamId(0);
        m.m.team_roster_index = 1;
    }
    for (r, c) in [(50, 50), (49, 50), (49, 51)] {
        w.terrain.set_passable(r, c, true);
    }
    let goal = cell_world(&w, 50, 50);
    let mut order = order(OrderCode::JumpToPoint, OrderOrigin::Player);
    order.move_params.way_path.points[0] = goal;
    super::issue_player_command(
        &mut w,
        &crate::PlayerCommand::Order {
            movers: vec![
                crate::ids::PartId(512),
                crate::ids::PartId(513),
                crate::ids::PartId(514),
            ],
            order,
            queue: true,
        },
    );
    for (slot, cell) in [(1, (50, 50)), (2, (49, 50)), (3, (49, 51))] {
        assert_eq!(
            w.pilots.slots[slot].queue[0].point,
            cell_world(&w, cell.0, cell.1)
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// MoveOrders / getMovePath
// ---------------------------------------------------------------------------------------------------------------------

fn path_with_steps(n: usize, goal: Vec3) -> mc2_path::MovePath {
    let mut p = mc2_path::MovePath::default();
    p.init(n);
    p.goal = goal;
    p.global_step = -1;
    p
}

/// `getMovePath()` (warrior.cpp:2607-2609): a current path that still has steps is returned as it is, nothing is cleared or swapped.
#[test]
fn get_move_path_returns_a_path_with_steps_untouched() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(3, point(1.0, 2.0));
    p.paths[0].cur_step = 1;
    p.paths[1] = path_with_steps(4, point(9.0, 9.0));
    let got = warrior::get_move_path(&mut w, 1).unwrap();
    assert_eq!((got.num_steps_when_not_paused, got.cur_step), (3, 1));
    assert_eq!(
        w.pilots.slots[1].lane_b_move.paths[1].num_steps_when_not_paused,
        4
    );
}

/// warrior.cpp:2610-2634: an exhausted current path is cleared (`MovePath::clear`: step 0, global step -1) and swapped with the spare;
/// a spare that holds steps but belongs to no goal is emptied (`numStepsWhenNotPaused = 0`).
#[test]
fn get_move_path_swaps_in_the_spare_and_empties_it_without_a_goal() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(0, point(0.0, 0.0));
    p.paths[0].cur_step = 5;
    p.paths[0].global_step = 7;
    p.paths[1] = path_with_steps(4, point(9.0, 9.0));
    p.paths[1].cur_step = 2;
    assert_eq!(p.goal_type, MOVEGOAL_NONE);
    let got = warrior::get_move_path(&mut w, 1).unwrap();
    assert_eq!(got.num_steps_when_not_paused, 0);
    let m = &w.pilots.slots[1].lane_b_move;
    // the old current path is now the spare, cleared
    assert_eq!((m.paths[1].cur_step, m.paths[1].global_step), (0, -1));
    // the old spare is current, with its steps hidden
    assert_eq!(m.paths[0].cur_step, 2);
    assert_eq!(m.paths[0].num_steps_when_not_paused, 0);
}

/// warrior.cpp:2672-2678: a swapped-in path with a location goal and no yield / wait clears the yield state and rewrites the goal from
/// the path (`setMoveGoal(MOVEGOAL_LOCATION, &path[0]->goal)`).
#[test]
fn get_move_path_with_a_location_goal_clears_yield_and_rewrites_the_goal() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(0, Vec3::default());
    p.paths[1] = path_with_steps(4, point(50.0, 60.0));
    p.goal_type = MOVEGOAL_LOCATION;
    p.yield_state = 3;
    p.set_goal_fields(MOVEGOAL_LOCATION, point(1.0, 1.0));
    warrior::get_move_path(&mut w, 1).unwrap();
    let m = &w.pilots.slots[1].lane_b_move;
    assert_eq!(m.paths[0].num_steps_when_not_paused, 4);
    assert_eq!((m.yield_time, m.yield_state), (-1.0, 0));
    assert!(!m.is_yielding);
    assert_eq!(m.goal_type, MOVEGOAL_LOCATION);
    assert_eq!((m.goal.x, m.goal.y), (50.0, 60.0));
    // z < -10 would be resolved to the terrain elevation; the path goal's z is 0
    assert_eq!(m.goal.z, 0.0);
}

/// warrior.cpp:2658-2663: a yielding pilot's new path starts paused and `yieldTime = scenarioTime + 1.5`; a pilot waiting for a point
/// (warrior.cpp:2664-2666) only has the path paused and keeps its yield time.
#[test]
fn get_move_path_pauses_the_new_path_while_yielding_or_waiting() {
    let (mut w, _, _) = world();
    w.clock.scenario_time = 10.0;
    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(0, Vec3::default());
    p.paths[1] = path_with_steps(4, point(5.0, 5.0));
    p.set_goal_fields(MOVEGOAL_LOCATION, point(5.0, 5.0));
    p.set_yield_time(2.0);
    warrior::get_move_path(&mut w, 1).unwrap();
    let m = &w.pilots.slots[1].lane_b_move;
    assert_eq!(m.yield_time, 11.5);
    assert_eq!(
        (m.paths[0].num_steps, m.paths[0].num_steps_when_not_paused),
        (0, 4)
    );

    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(0, Vec3::default());
    p.paths[1] = path_with_steps(2, point(5.0, 5.0));
    p.set_yield_time(-1.0);
    p.wait_for_point_time = 3.0;
    warrior::get_move_path(&mut w, 1).unwrap();
    let m = &w.pilots.slots[1].lane_b_move;
    assert_eq!(m.yield_time, -1.0);
    assert_eq!(
        (m.paths[0].num_steps, m.paths[0].num_steps_when_not_paused),
        (0, 2)
    );
}

/// warrior.cpp:2636-2652: a goal object becomes the path's target; a goal object that no longer exists leaves the pilot standing.
#[test]
fn get_move_path_follows_a_goal_object_and_gives_up_when_it_is_gone() {
    let (mut w, _, enemy) = world();
    let wid = w.objects.watch_id(enemy);
    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(0, Vec3::default());
    p.paths[1] = path_with_steps(4, point(300.0, 100.0));
    p.set_goal_fields(i64::from(wid.0), point(300.0, 100.0));
    warrior::get_move_path(&mut w, 1).unwrap();
    let m = &w.pilots.slots[1].lane_b_move;
    assert_eq!(m.paths[0].target, point(300.0, 100.0)); // path->target = target->getPosition()
    assert_eq!(m.goal_type, i64::from(wid.0)); // setMoveGoal(target->getWatchID(), &path->goal)
    assert_eq!(m.goal_object_wid, wid);

    // a dead goal (watch id 4000: `ObjectManager->get(4000)` is NULL, a handle far past the list)
    let p = &mut w.pilots.slots[1].lane_b_move;
    p.paths[0] = path_with_steps(0, Vec3::default());
    p.paths[1] = path_with_steps(4, Vec3::default());
    p.set_goal_fields(4000, point(0.0, 0.0));
    warrior::get_move_path(&mut w, 1).unwrap();
    assert_eq!(
        w.pilots.slots[1].lane_b_move.paths[0].num_steps_when_not_paused,
        0
    );
}

/// warrior.cpp:2679-2686: a swapped-in path whose `globalStep` is the last global step rewrites `globalGoalLocation` from its final
/// step and the current order's first way point.
#[test]
fn get_move_path_on_the_last_global_step_rewrites_the_order_goal() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.lane_b_move.paths[0] = path_with_steps(0, Vec3::default());
    let mut spare = path_with_steps(3, point(7.0, 8.0));
    spare.global_step = 0;
    spare.steps[2].destination = point(77.0, 88.0);
    p.lane_b_move.paths[1] = spare;
    p.lane_b_move.num_global_steps = 1;
    p.lane_b_move
        .set_goal_fields(MOVEGOAL_LOCATION, point(7.0, 8.0));
    p.cur_order = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    warrior::get_move_path(&mut w, 1).unwrap();
    let p = &w.pilots.slots[1];
    assert_eq!(p.lane_b_move.global_goal_location, point(77.0, 88.0));
    assert_eq!(
        p.cur_order.move_params.way_path.points[0],
        point(77.0, 88.0)
    );
}

/// `clearMoveOrders()` (warrior.cpp:2418-2433).
#[test]
fn clear_move_orders_resets_goal_paths_timers_and_the_path_request() {
    let (mut w, _, _) = world();
    {
        let m = &mut w.pilots.slots[1].lane_b_move;
        m.set_goal_fields(5, point(1.0, 2.0));
        m.way_path = vec![point(1.0, 1.0)];
        m.paths[0] = path_with_steps(5, point(3.0, 3.0));
        m.paths[0].cur_step = 4;
        m.paths[1] = path_with_steps(2, Vec3::default());
        m.move_state = 3;
        m.move_state_goal = 5;
        m.set_yield_time(8.0);
        m.yield_state = 2;
        m.wait_for_point_time = 4.0;
        m.time_of_last_step = 6.0;
        m.twisting = true;
        m.num_global_steps = 3;
    }
    warrior::request_move_path(&mut w, 1, -1, 0, 0);
    assert!(warrior::has_path_request(&w, 1));
    warrior::clear_move_orders(&mut w, 1);
    let m = &w.pilots.slots[1].lane_b_move;
    assert_eq!(m.goal_type, MOVEGOAL_NONE);
    assert_eq!(m.goal_object_wid, WatchId(0));
    assert_eq!(m.goal.x, -999_999.0);
    assert!(m.way_path.is_empty());
    for path in &m.paths {
        assert_eq!(
            (
                path.num_steps,
                path.num_steps_when_not_paused,
                path.cur_step
            ),
            (0, 0, 0)
        );
        assert_eq!(path.global_step, -1);
    }
    assert_eq!((m.move_state, m.move_state_goal), (1, 1));
    assert_eq!(
        (
            m.yield_time,
            m.yield_state,
            m.wait_for_point_time,
            m.time_of_last_step
        ),
        (-1.0, 0, -1.0, -1.0)
    );
    assert!(!m.twisting && !m.is_yielding);
    assert_eq!(m.num_global_steps, 0);
    assert!(!warrior::has_path_request(&w, 1));
}

/// `rethinkPath` (warrior.cpp:2515-2575) for a move order (not a way-path order, so `getNextWayPoint` is false): close to the end, so the
/// pilot stops: move orders and the current move order are cleared and `PILOT_ALARM_NO_MOVEPATH` is raised with `MOVEPATH_ERR_EARLY_STOP`.
#[test]
fn rethink_path_of_a_simple_move_stops_the_pilot() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.cur_order = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    p.lane_b_move.path_type = MOVEPATH_SIMPLE;
    warrior::rethink_path(&mut w, 1, 0);
    let p = &w.pilots.slots[1];
    assert_eq!(p.cur_order.code, OrderCode::None);
    assert_eq!(p.alarms[alarm::NO_MOVEPATH], vec![-14, -9]); // clearCurTacOrder logs TACORDER_CLEARED first, then EARLY_STOP
}

/// warrior.cpp:2553-2570: a complex path that is not on its last global step pauses the current path and asks for a recalculation
/// (`MOVEPARAM_RECALC | AVOID_PATHLOCKS | FACE_TARGET`, source 3).
#[test]
fn rethink_path_of_a_complex_path_requests_a_recalc() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.cur_order = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    p.cur_order.selection_index = 4;
    p.lane_b_move.path_type = MOVEPATH_COMPLEX;
    p.lane_b_move.num_global_steps = 3;
    p.lane_b_move.paths[0] = path_with_steps(5, Vec3::default());
    p.lane_b_move.paths[0].global_step = 0;
    warrior::rethink_path(&mut w, 1, 0);
    assert_eq!(w.pilots.slots[1].lane_b_move.paths[0].num_steps, 0);
    let r = w.paths.queue.back().copied().unwrap();
    assert_eq!((r.pilot, r.selector, r.source), (1, 4, 3));
    assert_eq!(r.params, 512 | 128 | 1); // RECALC | AVOID_PATHLOCKS | FACE_TARGET
}

// ---------------------------------------------------------------------------------------------------------------------
// Order slots
// ---------------------------------------------------------------------------------------------------------------------

/// `setCurTacOrder` / `clearCurTacOrder(false)` (warrior.cpp:4397-4422, 4515-4525): the in-combat count follows combat orders, the
/// suppression flag drops, the move and attack orders are cleared, `PILOT_ALARM_NO_MOVEPATH` logs `MOVEPATH_ERR_TACORDER_CLEARED` (-14).
#[test]
fn clear_cur_tac_order_without_update_clears_orders_and_counts_combat() {
    let (mut w, h, _) = world();
    w.clock.scenario_time = 4.5;
    warrior::set_cur_tac_order(
        &mut w,
        1,
        order(OrderCode::AttackObject, OrderOrigin::Commander),
    );
    assert_eq!(w.pilots.num_warriors_in_combat, 1);
    assert_eq!(w.pilots.slots[1].last_order.code, OrderCode::AttackObject);
    if let Some(Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        m.m.suppression_fire = 1;
    }
    warrior::clear_cur_tac_order(&mut w, 1, false);
    assert_eq!(w.pilots.num_warriors_in_combat, 0);
    let p = &w.pilots.slots[1];
    assert_eq!(p.cur_order.code, OrderCode::None);
    assert_eq!(p.alarms[alarm::NO_MOVEPATH], vec![-14]);
    assert_eq!(p.last_order.last_time, 4.5);
    assert_eq!(p.attack_orders.target_time, -1.0);
    assert_eq!(warrior::core(&w, 1).unwrap().suppression_fire, 0);
}

/// `clearCurTacOrder()` with the order-slot update (warrior.cpp:4423-4492): the general order falls to nothing; the player slot falls
/// back to the general order; the alarm slot falls back to the player order, else to the general order, and resets the alarm priority.
#[test]
fn clear_cur_tac_order_falls_back_through_the_order_slots() {
    // GENERAL: the slot is re-initialised unless a new general order arrived; nothing is started
    let (mut w, _, _) = world();
    w.pilots.slots[1].tac_orders[0] = order(OrderCode::Wait, OrderOrigin::Commander);
    warrior::clear_cur_tac_order(&mut w, 1, true);
    assert_eq!(w.pilots.slots[1].tac_orders[0].code, OrderCode::None);
    assert_eq!(w.pilots.slots[1].cur_order.code, OrderCode::None);

    // PLAYER: -> GENERAL, the general order is started (executed + made current), main goal untouched for the home pilot
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.order_state = OrderState::Player;
    p.tac_orders[1] = order(OrderCode::Stop, OrderOrigin::Player);
    p.tac_orders[0] = order(OrderCode::HoldFire, OrderOrigin::Commander);
    warrior::clear_cur_tac_order(&mut w, 1, true);
    let p = &w.pilots.slots[1];
    assert_eq!(p.order_state, OrderState::General);
    assert_eq!(p.tac_orders[1].code, OrderCode::None);
    assert_eq!(p.cur_order.code, OrderCode::HoldFire);
    assert_eq!(p.time_of_last_orders, -1.0);
    assert_eq!(p.new_order_received, [false; 3]);

    // ALARM with a player order -> PLAYER; priority back to 0, the alarm slot emptied
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.order_state = OrderState::Alarm;
    p.alarm_priority = 9;
    p.tac_orders[2] = order(OrderCode::Stop, OrderOrigin::SelfOrder);
    p.tac_orders[1] = order(OrderCode::Wait, OrderOrigin::Player);
    p.tac_orders[1].delayed_time = 99.0;
    warrior::clear_cur_tac_order(&mut w, 1, true);
    let p = &w.pilots.slots[1];
    assert_eq!(p.order_state, OrderState::Player);
    assert_eq!(p.alarm_priority, 0);
    assert_eq!(p.tac_orders[2].code, OrderCode::None);
    assert_eq!(p.cur_order.code, OrderCode::Wait);

    // ALARM without a player order -> GENERAL
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.order_state = OrderState::Alarm;
    p.tac_orders[0] = order(OrderCode::HoldFire, OrderOrigin::Commander);
    warrior::clear_cur_tac_order(&mut w, 1, true);
    assert_eq!(w.pilots.slots[1].order_state, OrderState::General);
    assert_eq!(w.pilots.slots[1].cur_order.code, OrderCode::HoldFire);
}

/// `setAlarmTacOrder(order, priority)` (warrior.cpp:4555): an order below the current alarm priority is ignored.
#[test]
fn alarm_orders_need_at_least_the_current_priority() {
    let (mut w, _, _) = world();
    warrior::set_alarm_tac_order(&mut w, 1, order(OrderCode::Stop, OrderOrigin::SelfOrder), 5);
    assert_eq!(w.pilots.slots[1].alarm_priority, 5);
    assert!(w.pilots.slots[1].new_order_received[2]);
    w.pilots.slots[1].new_order_received[2] = false;
    w.pilots.slots[1].tac_orders[2] = TacticalOrder::init();
    warrior::set_alarm_tac_order(&mut w, 1, order(OrderCode::Stop, OrderOrigin::SelfOrder), 4);
    assert!(!w.pilots.slots[1].new_order_received[2]);
    assert_eq!(w.pilots.slots[1].tac_orders[2].code, OrderCode::None);
    warrior::set_alarm_tac_order(&mut w, 1, order(OrderCode::Stop, OrderOrigin::SelfOrder), 5);
    assert!(w.pilots.slots[1].new_order_received[2]);
}

/// `setPlayerTacOrder(order, fromQueue)` (warrior.cpp:4543): an order that does not come from the queue also clears the queue.
#[test]
fn a_player_order_not_from_the_queue_clears_the_queue() {
    let (mut w, _, _) = world();
    w.pilots.slots[1].queue.push(Default::default());
    w.pilots.slots[1].queue_executing = true;
    warrior::set_player_tac_order(&mut w, 1, order(OrderCode::Stop, OrderOrigin::Player), true);
    assert_eq!(w.pilots.slots[1].queue.len(), 1);
    warrior::set_player_tac_order(
        &mut w,
        1,
        order(OrderCode::Stop, OrderOrigin::Player),
        false,
    );
    assert!(w.pilots.slots[1].queue.is_empty());
    assert!(!w.pilots.slots[1].queue_executing);
    assert!(w.pilots.slots[1].new_order_received[1]);
}

/// `triggerAlarm` (warrior.cpp:4558): a full alarm list (15) silently drops further triggers.
#[test]
fn alarms_drop_triggers_past_fifteen() {
    let (mut w, _, _) = world();
    for i in 0..15 {
        assert_eq!(warrior::trigger_alarm(&mut w, 1, alarm::COLLISION, i), 0);
    }
    assert_eq!(warrior::trigger_alarm(&mut w, 1, alarm::COLLISION, 99), -1);
    assert_eq!(w.pilots.slots[1].alarms[alarm::COLLISION].len(), 15);
    assert_eq!(w.pilots.slots[1].alarm_history[alarm::COLLISION].len(), 15);
}

// ---------------------------------------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------------------------------------

/// `getLastTarget()` (warrior.cpp:2323-2347): a destroyed target is dropped (time -1, flags cleared) together with a current combat
/// order; a disabled one survives only with `lastTargetObliterate`; a friendly one only with `lastTargetFriendly`.
#[test]
fn get_last_target_drops_dead_disabled_and_friendly_targets() {
    let (mut w, _, enemy) = world();
    let wid = w.objects.watch_id(enemy);
    w.pilots.slots[1].last_target = (wid, 1.0);
    assert_eq!(warrior::get_last_target(&mut w, 1), Some(enemy));

    w.objects.get_mut(enemy).unwrap().c.status = ObjectStatus::Disabled;
    assert_eq!(warrior::get_last_target(&mut w, 1), None);
    assert_eq!(w.pilots.slots[1].last_target.0, WatchId(0));
    assert_eq!(w.pilots.slots[1].last_target.1, -1.0);

    // obliterate keeps a disabled target
    w.pilots.slots[1].last_target = (wid, 1.0);
    w.pilots.slots[1].last_target_flags = [true, false, false];
    assert_eq!(warrior::get_last_target(&mut w, 1), Some(enemy));

    // a destroyed one never survives, and takes the combat order with it
    w.objects.get_mut(enemy).unwrap().c.status = ObjectStatus::Destroyed;
    warrior::set_cur_tac_order(
        &mut w,
        1,
        order(OrderCode::AttackObject, OrderOrigin::Commander),
    );
    assert_eq!(warrior::get_last_target(&mut w, 1), None);
    assert_eq!(w.pilots.slots[1].cur_order.code, OrderCode::None);
    assert_eq!(w.pilots.num_warriors_in_combat, 0);

    // friendly target (same team) is dropped unless it was targeted as a friendly
    let (mut w, me, _) = world();
    let friend = me; // same team as itself
    let wid = w.objects.watch_id(friend);
    w.pilots.slots[1].last_target = (wid, 1.0);
    assert_eq!(warrior::get_last_target(&mut w, 1), None);
    w.pilots.slots[1].last_target = (wid, 1.0);
    w.pilots.slots[1].last_target_flags = [false, true, false];
    assert_eq!(warrior::get_last_target(&mut w, 1), Some(friend));
}

/// `setLastTarget` (warrior.cpp:2350-2378): the time is stamped, the friendly flag follows the relation, and a conserve-ammo target
/// forces `ATTACK_CONSERVING_AMMO` on the current order the next time it is read (warrior.cpp:2344).
#[test]
fn set_last_target_records_time_flags_and_conserve_ammo() {
    let (mut w, _, enemy) = world();
    w.clock.scenario_time = 3.25;
    warrior::set_last_target(&mut w, 2, Some(ObjectHandle(1)), true, true);
    let p = &w.pilots.slots[2];
    assert_eq!(p.last_target.1, 3.25);
    // pilot 2 (team 1) targets the team 0 mech: an enemy, not friendly
    assert_eq!(p.last_target_flags, [true, false, true]);
    warrior::get_last_target(&mut w, 2);
    assert_eq!(w.pilots.slots[2].cur_order.attack_params.attack_type, 3);
    let _ = enemy;
}

// ---------------------------------------------------------------------------------------------------------------------
// TacticalOrder::status / execute
// ---------------------------------------------------------------------------------------------------------------------

/// `TacticalOrder::status` (tacordr.cpp:1262-1910) for the orders a run reaches, one row each.
#[test]
fn status_table() {
    let (mut w, me, enemy) = world();
    w.clock.scenario_time = 5.0;
    let here = w.objects.get(me).unwrap().c.position;
    let st = |w: &mut World, o: &mut TacticalOrder| tacorder::status(w, 1, o);

    // WAIT: success when scenarioTime > delayedTime
    let mut o = order(OrderCode::Wait, OrderOrigin::Commander);
    o.delayed_time = 6.0;
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    o.delayed_time = 4.5;
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);

    // MOVETO_POINT: success within the goal cell (cell distance < 1)
    let mut o = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    o.move_params.way_path.points[0] = here;
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
    // ... except with `wait`: the order turns into a WAIT and is not done
    o.move_params.wait = true;
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    assert_eq!(o.code, OrderCode::Wait);
    // two cells away: not done
    let mut o = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    let (r, c) = w.terrain.world_to_cell(&here);
    o.move_params.way_path.points[0] = cell_world(&w, r + 2, c);
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    // one cell away is "cell distance 1": not done either (`cellDistanceToGoal < 1`)
    o.move_params.way_path.points[0] = cell_world(&w, r, c + 1);
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    // a delayed order that is not due yet is not done and does not execute
    o.delayed_time = 9.0;
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    assert_eq!(o.delayed_time, 9.0);

    // MOVETO_OBJECT: no target -> success; target present and the goal cell is further than 0 -> not done
    let mut o = order(OrderCode::MoveToObject, OrderOrigin::Commander);
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
    o.target = w.objects.watch_id(enemy);
    o.move_params.way_path.points[0] = cell_world(&w, r, c + 1);
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    o.move_params.way_path.points[0] = here;
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);

    // JUMPTO_POINT: done at stage 3; GUARD never; STOP always; POWERDOWN never; EJECT / WAYPOINTS_DONE success
    let mut o = order(OrderCode::JumpToPoint, OrderOrigin::Player);
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    o.stage = 3;
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
    for (code, want) in [
        (OrderCode::Guard, TACORDER_FAILURE),
        (OrderCode::Stop, TACORDER_SUCCESS),
        (OrderCode::PowerDown, TACORDER_FAILURE),
        (OrderCode::Eject, TACORDER_SUCCESS),
        (OrderCode::WaypointsDone, TACORDER_SUCCESS),
        (OrderCode::AttackPoint, TACORDER_FAILURE),
        (OrderCode::HoldFire, TACORDER_FAILURE),
        (OrderCode::Withdraw, TACORDER_FAILURE),
        (OrderCode::JumpToObject, TACORDER_SUCCESS), // USE_JUMPING is not defined
    ] {
        let mut o = order(code, OrderOrigin::Player);
        assert_eq!(st(&mut w, &mut o), want, "{code:?}");
    }

    // POWERUP: done when the vehicle status is NORMAL
    let mut o = order(OrderCode::PowerUp, OrderOrigin::Player);
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
    w.objects.get_mut(me).unwrap().c.status = ObjectStatus::ShutDown;
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    w.objects.get_mut(me).unwrap().c.status = ObjectStatus::Normal;

    // ATTACK_OBJECT: target destroyed -> success; disabled -> success unless obliterate; alive -> failure; no target -> success
    let mut o = order(OrderCode::AttackObject, OrderOrigin::Player);
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
    o.target = w.objects.watch_id(enemy);
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    w.objects.get_mut(enemy).unwrap().c.status = ObjectStatus::Disabled;
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
    o.attack_params.obliterate = true;
    assert_eq!(st(&mut w, &mut o), TACORDER_FAILURE);
    w.objects.get_mut(enemy).unwrap().c.status = ObjectStatus::Destroyed;
    assert_eq!(st(&mut w, &mut o), TACORDER_SUCCESS);
}

/// `TacticalOrder::execute` of a move order (tacordr.cpp:795-826): the move goal is the way point, the run flag follows the travel
/// mode, a path is requested with `INIT | FACE_TARGET` from source 21, the order is a player order so `PILOT_ALARM_PLAYER_ORDER` logs
/// the order code, and the result is "not done" (`TACORDER_FAILURE`).
#[test]
fn execute_move_to_point_sets_the_goal_requests_a_path_and_alarms_for_player_orders() {
    let (mut w, _, _) = world();
    w.clock.scenario_time = 2.0;
    let mut o = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    o.move_params.way_path.points[0] = Vec3 {
        x: 500.0,
        y: 400.0,
        z: 12.0,
    };
    o.move_params.way_path.mode[0] = TravelMode::Fast;
    o.selection_index = 3;
    let (result, message) = tacorder::execute_full(&mut w, 1, &mut o);
    assert_eq!(result, TACORDER_FAILURE);
    assert_eq!(message, -1);
    let m = &w.pilots.slots[1].lane_b_move;
    assert_eq!(m.goal_type, MOVEGOAL_LOCATION);
    assert_eq!(
        m.goal,
        Vec3 {
            x: 500.0,
            y: 400.0,
            z: 12.0
        }
    );
    assert!(m.run);
    assert_eq!(m.time_of_last_step, 2.0); // moveOrders.timeOfLastStep = scenarioTime
    let r = w.paths.queue.back().copied().unwrap();
    // a player order of a non-group mover also asks for the radio result (warrior.cpp:6272-6275)
    assert_eq!(
        (r.pilot, r.selector, r.params, r.source),
        (1, 3, 256 | 1 | 4096, 21)
    );
    assert_eq!(
        w.pilots.slots[1].alarms[alarm::PLAYER_ORDER],
        vec![OrderCode::MoveToPoint as i32]
    );
}

/// A commander-origin order does not raise the player-order alarm, and `message` is forgotten (tacordr.cpp:1233-1236).
#[test]
fn execute_of_a_commander_order_raises_no_player_alarm() {
    let (mut w, _, _) = world();
    let mut o = order(OrderCode::Stop, OrderOrigin::Commander);
    let (_, message) = tacorder::execute_full(&mut w, 1, &mut o);
    assert_eq!(message, -1);
    assert!(w.pilots.slots[1].alarms[alarm::PLAYER_ORDER].is_empty());
    // the same order from the player keeps RADIO_ACK (0)
    let mut o = order(OrderCode::Stop, OrderOrigin::Player);
    let (_, message) = tacorder::execute_full(&mut w, 1, &mut o);
    assert_eq!(message, 0);
}

/// tacordr.cpp:786-796: a move order that has a `delayedTime` in the future returns failure at once and does nothing else.
#[test]
fn execute_of_a_delayed_move_does_nothing_yet() {
    let (mut w, _, _) = world();
    w.clock.scenario_time = 1.0;
    let mut o = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    o.delayed_time = 2.0;
    let (result, _) = tacorder::execute_full(&mut w, 1, &mut o);
    assert_eq!(result, TACORDER_FAILURE);
    assert!(w.paths.queue.is_empty());
    assert!(w.pilots.slots[1].alarms[alarm::PLAYER_ORDER].is_empty());
    assert_eq!(o.delayed_time, 2.0);
    // due: executes and forgets the delay
    w.clock.scenario_time = 2.5;
    tacorder::execute_full(&mut w, 1, &mut o);
    assert_eq!(o.delayed_time, -1.0);
    assert_eq!(w.paths.queue.len(), 1);
}

/// `execute` of a non-withdraw order clears the vehicle's `withdrawing` flag (tacordr.cpp:1238-1244).
#[test]
fn any_order_but_withdraw_ends_withdrawing() {
    let (mut w, me, _) = world();
    warrior::core_mut(&mut w, 1).unwrap().flags.withdrawing = true;
    let mut o = order(OrderCode::Wait, OrderOrigin::Commander);
    tacorder::execute_full(&mut w, 1, &mut o);
    assert!(!warrior::core(&w, 1).unwrap().flags.withdrawing);
    let _ = me;
}

/// `execute` of an attack order plays the acknowledgement with `RollDice(50)` on the VISUAL stream (tacordr.cpp:959, a radio choice),
/// and none on the sim stream.
#[test]
fn execute_attack_object_draws_the_acknowledgement_from_the_visual_stream() {
    let (mut w, _, enemy) = world();
    // Mover::calcFireRanges needs components; without them numWeapons is 0 and orderAttackObject refuses at once
    let mut o = order(OrderCode::AttackObject, OrderOrigin::Player);
    o.target = w.objects.watch_id(enemy);
    let (sim0, vis0) = {
        let r = w.rng.0.borrow();
        (r.sim.draws(), r.visual.draws())
    };
    let (result, message) = tacorder::execute_full(&mut w, 1, &mut o);
    let (sim1, vis1) = {
        let r = w.rng.0.borrow();
        (r.sim.draws(), r.visual.draws())
    };
    // no weapons: orderAttackObject returns TACORDER_SUCCESS (warrior.cpp:6772-6775), so no acknowledgement roll
    assert_eq!(result, TACORDER_SUCCESS);
    assert_eq!(message, 17); // RADIO_ILLEGAL_ORDER stays: only FAILURE replaces it
    assert_eq!((sim1 - sim0, vis1 - vis0), (0, 0));
    assert_eq!(o.target_object_class, ObjectClass::BattleMech as i32);
    assert!(!o.attack_params.obliterate);
}

// ---------------------------------------------------------------------------------------------------------------------
// Pilot orders (order*/core*)
// ---------------------------------------------------------------------------------------------------------------------

/// `orderMoveToPoint` when the pilot is already at the goal: `status` is success, nothing else happens (warrior.cpp:6247-6250).
#[test]
fn order_move_to_point_at_the_goal_does_nothing() {
    let (mut w, me, _) = world();
    let here = w.objects.get(me).unwrap().c.position;
    let result =
        orders::order_move_to_point(&mut w, 1, false, true, OrderOrigin::Commander, here, -1, 0);
    assert_eq!(result, TACORDER_SUCCESS);
    assert!(w.paths.queue.is_empty());
    assert_eq!(w.pilots.slots[1].lane_b_move.goal_type, MOVEGOAL_NONE);
}

/// `orderMoveToPoint` (warrior.cpp:6212-6283) for a commander order away from the goal: the goal, `timeOfLastStep`, `run`, the cleared
/// attack orders and the path request (`INIT | FACE_TARGET`, plus `ESCAPE_TILE` / `JUMP` / `RADIO_RESULT` when asked, source 21); the
/// failure result also becomes the general order for a commander origin.
#[test]
fn order_move_to_point_requests_a_path_and_sets_the_general_order() {
    let (mut w, _, _) = world();
    w.clock.scenario_time = 7.0;
    let goal = point(900.0, 100.0);
    let result = orders::order_move_to_point(
        &mut w,
        1,
        false,
        true,
        OrderOrigin::Commander,
        goal,
        -1,
        orders::param::RUN | orders::param::JUMP,
    );
    assert_eq!(result, TACORDER_FAILURE);
    let p = &w.pilots.slots[1];
    assert_eq!(p.lane_b_move.goal_type, MOVEGOAL_LOCATION);
    assert_eq!(p.lane_b_move.goal.x, 900.0);
    assert_eq!(p.lane_b_move.time_of_last_step, 7.0);
    assert!(p.lane_b_move.run);
    assert_eq!(p.attack_orders.target_time, -1.0); // clearAttackOrders
    assert_eq!(p.tac_orders[0].code, OrderCode::MoveToPoint);
    assert!(p.new_order_received[0]);
    let r = w.paths.queue.back().copied().unwrap();
    assert_eq!((r.params, r.source), (256 | 1 | (1 << 20), 21)); // INIT | FACE_TARGET | JUMP
}

/// A player-origin move of a non-group order asks for the radio result (`MOVEPARAM_RADIO_RESULT` = 4096) (warrior.cpp:6272-6275) and
/// does not become the general order.
#[test]
fn order_move_to_point_for_the_player_asks_for_the_radio_result() {
    let (mut w, _, _) = world();
    orders::order_move_to_point(
        &mut w,
        1,
        false,
        true,
        OrderOrigin::Player,
        point(900.0, 100.0),
        2,
        0,
    );
    let r = w.paths.queue.back().copied().unwrap();
    assert_eq!((r.selector, r.params), (2, 256 | 1 | 4096));
    assert!(!w.pilots.slots[1].new_order_received[0]);
}

/// `coreMoveTo` (warrior.cpp:5254-5360): a second move order with the same flags is "already doing it" (0) even to ANOTHER place:
/// `TacticalOrder::equals` compares `numPoints * 3` coordinates and `orderMoveToPoint` leaves `numPoints` 0 (tacordr.cpp:1938-1955).
#[test]
fn core_move_to_ignores_a_new_destination_while_a_move_order_is_current() {
    let (mut w, _, _) = world();
    assert_eq!(
        orders::core_move_to(&mut w, 1, point(900.0, 100.0), orders::param::RUN),
        0
    );
    assert_eq!(w.pilots.slots[1].tac_orders[0].code, OrderCode::MoveToPoint);
    // make it current, as the next mainDecisionTree would
    w.pilots.slots[1].cur_order = w.pilots.slots[1].tac_orders[0].clone();
    w.paths.queue.clear();
    // same flags, different place -> equals -> return 0 without a new path request
    assert_eq!(
        orders::core_move_to(&mut w, 1, point(100.0, 900.0), orders::param::RUN),
        0
    );
    assert!(w.paths.queue.is_empty());
    assert_eq!(w.pilots.slots[1].lane_b_move.goal.x, 900.0);
    // not even a different travel mode matters (modes are compared only for `numPoints * 3` entries)
    assert_eq!(orders::core_move_to(&mut w, 1, point(100.0, 900.0), 0), 0);
    assert!(w.paths.queue.is_empty());
    // but a different `layMines` flag (compared unconditionally) makes it a new order
    assert_eq!(
        orders::core_move_to(&mut w, 1, point(100.0, 900.0), orders::param::LAY_MINES),
        0
    );
    assert_eq!(w.paths.queue.len(), 1);
    assert_eq!(w.pilots.slots[1].lane_b_move.goal.x, 100.0);
}

/// `corePower` (warrior.cpp:5506-5540): powering down a mech that is already shut down answers 1 (nothing to do); otherwise the vehicle
/// begins shutting down (`SHUTTING_DOWN`, `shutDownThisFrame`) and the order becomes the general order.
#[test]
fn core_power_shuts_the_mech_down_and_sets_the_general_order() {
    let (mut w, me, _) = world();
    assert_eq!(orders::core_power(&mut w, 1, false), 0);
    let o = w.objects.get(me).unwrap();
    assert_eq!(o.c.status, ObjectStatus::ShuttingDown);
    assert!(matches!(&o.kind, Kind::Mech(m) if m.m.flags.shut_down_this_frame));
    assert_eq!(w.pilots.slots[1].tac_orders[0].code, OrderCode::PowerDown);
    // already shutting down: Mover::shutDown does nothing and the order equals the current one
    w.objects.get_mut(me).unwrap().c.status = ObjectStatus::ShutDown;
    assert_eq!(orders::core_power(&mut w, 1, false), 1);
    // powering up a normal mech: nothing to do
    w.objects.get_mut(me).unwrap().c.status = ObjectStatus::Normal;
    assert_eq!(orders::core_power(&mut w, 1, true), 1);
}

/// `orderWait` (warrior.cpp:6125-6163): `delayedTime = scenarioTime + seconds`, no move goal, no attack orders, a commander order is
/// the general order; the result is the order's `status` (not done until the time has passed).
#[test]
fn order_wait_sets_a_delay_and_the_general_order() {
    let (mut w, _, enemy) = world();
    w.clock.scenario_time = 2.0;
    let wid = w.objects.watch_id(enemy);
    w.pilots.slots[1].last_target = (wid, 1.0);
    let result = orders::order_wait(&mut w, 1, false, OrderOrigin::Commander, 5, true);
    assert_eq!(result, TACORDER_FAILURE);
    assert_eq!(w.pilots.slots[1].tac_orders[0].delayed_time, 7.0);
    assert_eq!(w.pilots.slots[1].last_target.0, WatchId(0)); // clearLastTarget
                                                             // a non-commander order is not stored
    let (mut w, _, _) = world();
    orders::order_wait(&mut w, 1, false, OrderOrigin::Player, 5, false);
    assert!(!w.pilots.slots[1].new_order_received[0]);
}

/// `orderStop` (warrior.cpp:6165-6195): the queue is cleared and the pilot forgets its target.
#[test]
fn order_stop_clears_the_queue_and_the_target() {
    let (mut w, _, enemy) = world();
    let wid = w.objects.watch_id(enemy);
    w.pilots.slots[1].last_target = (wid, 1.0);
    w.pilots.slots[1].queue.push(Default::default());
    w.pilots.slots[1].queue_executing = true;
    let result = orders::order_stop(&mut w, 1, false, true);
    assert_eq!(result, TACORDER_SUCCESS); // STOP is always "done"
    assert!(w.pilots.slots[1].queue.is_empty() && !w.pilots.slots[1].queue_executing);
    assert_eq!(w.pilots.slots[1].last_target.0, WatchId(0));
}

/// `orderJumpToPoint` (warrior.cpp:6575-6598): a jump beyond `getJumpRange()` (jets * worldUnitsPerVertex, compared with the distance
/// in METERS, as the source does) or onto an impassable cell is refused (success = nothing to do).
#[test]
fn order_jump_to_point_checks_range_and_passability() {
    let (mut w, me, _) = world();
    if let Some(Kind::Mech(m)) = w.objects.get_mut(me).map(|o| &mut o.kind) {
        m.num_jump_jets = 5;
    }
    // 640 "range" against meters: (100 + 3200 - 100) * 0.2 = 640 -> 641 is too far
    let far = point(100.0 + 3205.0, 100.0);
    assert_eq!(
        orders::order_jump_to_point(&mut w, 1, false, true, OrderOrigin::Commander, far, -1),
        TACORDER_SUCCESS
    );
    assert!(w.pilots.slots[1].tac_orders[0].code == OrderCode::None);
    // within range: the move and attack orders are cleared and the order becomes the general order (the result is still SUCCESS)
    let near = point(100.0 + 3000.0, 100.0);
    let (r, c) = w.terrain.world_to_cell(&near);
    // an impassable landing cell refuses it first
    assert_eq!(
        orders::order_jump_to_point(&mut w, 1, false, true, OrderOrigin::Commander, near, -1),
        TACORDER_SUCCESS
    );
    assert!(w.pilots.slots[1].tac_orders[0].code == OrderCode::None);
    w.terrain.set_passable(r, c, true);
    assert_eq!(
        orders::order_jump_to_point(&mut w, 1, false, true, OrderOrigin::Commander, near, -1),
        TACORDER_SUCCESS
    );
    assert_eq!(w.pilots.slots[1].tac_orders[0].code, OrderCode::JumpToPoint);
}

// ---------------------------------------------------------------------------------------------------------------------
// handleTacticalOrder and the order queue
// ---------------------------------------------------------------------------------------------------------------------

/// `Mover::handleTacticalOrder` (mover.cpp:3161-3310): player, commander and self orders go to their own slot; a self order below
/// the alarm priority is dropped; a mech that cannot move refuses a move.
#[test]
fn handle_tactical_order_routes_orders_by_origin() {
    let (mut w, me, _) = world();
    super::handle_tactical_order(
        &mut w,
        me,
        order(OrderCode::Stop, OrderOrigin::Player),
        0,
        false,
    );
    super::handle_tactical_order(
        &mut w,
        me,
        order(OrderCode::Wait, OrderOrigin::Commander),
        0,
        false,
    );
    super::handle_tactical_order(
        &mut w,
        me,
        order(OrderCode::Eject, OrderOrigin::SelfOrder),
        3,
        false,
    );
    let p = &w.pilots.slots[1];
    assert_eq!(p.tac_orders[1].code, OrderCode::Stop);
    assert_eq!(p.tac_orders[0].code, OrderCode::Wait);
    assert_eq!(p.tac_orders[2].code, OrderCode::Eject);
    assert_eq!(p.alarm_priority, 3);
    assert_eq!(p.new_order_received, [true, true, true]);
    // NONE / SCRAMBLE fall to the default case: `return(1)`, nothing stored
    let (mut w, me, _) = world();
    assert_eq!(
        super::handle_tactical_order(
            &mut w,
            me,
            order(OrderCode::Scramble, OrderOrigin::Player),
            0,
            false
        ),
        1
    );
    assert_eq!(w.pilots.slots[1].new_order_received, [false; 3]);
}

/// mover.cpp:3187-3200: a selected mover (selectionIndex != -1) delays its move by `selectionIndex * DelayedOrderTime`; `canMove()` of a
/// mech is "legs not destroyed" (mech.h:650), so a mech with destroyed legs refuses the move.
#[test]
fn handle_tactical_order_delays_group_moves_and_refuses_when_immobile() {
    let (mut w, me, _) = world();
    w.clock.scenario_time = 10.0;
    if let Some(Kind::Mech(m)) = w.objects.get_mut(me).map(|o| &mut o.kind) {
        m.m.selection_index = 3;
    }
    let mut o = order(OrderCode::MoveToPoint, OrderOrigin::Player);
    o.move_params.way_path.points[0] = point(900.0, 100.0);
    super::handle_tactical_order(&mut w, me, o.clone(), 0, false);
    // DelayedOrderTime is 1.0 when the tables are absent
    assert_eq!(w.pilots.slots[1].tac_orders[1].delayed_time, 13.0);
    // destroyed legs: LEG_STATUS_DESTROYED = 4
    let (mut w, me, _) = world();
    if let Some(Kind::Mech(m)) = w.objects.get_mut(me).map(|o| &mut o.kind) {
        m.leg_status = crate::mover::LegStatus(4);
    }
    super::handle_tactical_order(&mut w, me, o, 0, false);
    assert!(!w.pilots.slots[1].new_order_received[1]);
}

/// mover.cpp:3201-3240: a jump needs a mech whose `getJumpRange()` covers the distance (mixed units, see `order_jump_to_point`), a
/// passable landing cell and a target that is not a teammate; a refused jump is `RADIO_ILLEGAL_ORDER` and is not stored.
#[test]
fn handle_tactical_order_gates_jumps() {
    let (mut w, me, enemy) = world();
    if let Some(Kind::Mech(m)) = w.objects.get_mut(me).map(|o| &mut o.kind) {
        m.num_jump_jets = 5;
    }
    let mut o = order(OrderCode::JumpToPoint, OrderOrigin::Player);
    o.move_params.way_path.points[0] = point(400.0, 100.0);
    // landing cell impassable: refused
    super::handle_tactical_order(&mut w, me, o.clone(), 0, false);
    assert!(!w.pilots.slots[1].new_order_received[1]);
    let (r, c) = w.terrain.world_to_cell(&point(400.0, 100.0));
    w.terrain.set_passable(r, c, true);
    super::handle_tactical_order(&mut w, me, o.clone(), 0, false);
    assert_eq!(w.pilots.slots[1].tac_orders[1].code, OrderCode::JumpToPoint);
    // too far
    let (mut w, me, _) = world();
    if let Some(Kind::Mech(m)) = w.objects.get_mut(me).map(|o| &mut o.kind) {
        m.num_jump_jets = 5;
    }
    o.move_params.way_path.points[0] = point(100.0 + 3300.0, 100.0);
    super::handle_tactical_order(&mut w, me, o.clone(), 0, false);
    assert!(!w.pilots.slots[1].new_order_received[1]);
    assert!(
        w.events
            .iter()
            .any(|e| matches!(e, crate::SimEvent::Radio { message: 17, .. }))
            || w.pilots.slots[1].tac_orders[1].code == OrderCode::None
    );
    // a teammate as target
    let (mut w, me, _) = world();
    let other = enemy; // team 1 vs team 0: not a teammate
    let mut o2 = order(OrderCode::JumpToObject, OrderOrigin::Player);
    o2.target = w.objects.watch_id(other);
    if let Some(Kind::Mech(m)) = w.objects.get_mut(me).map(|o| &mut o.kind) {
        m.num_jump_jets = 5;
    }
    o2.move_params.way_path.points[0] = point(300.0, 100.0);
    let (r, c) = w.terrain.world_to_cell(&point(300.0, 100.0));
    w.terrain.set_passable(r, c, true);
    super::handle_tactical_order(&mut w, me, o2, 0, false);
    assert_eq!(
        w.pilots.slots[1].tac_orders[1].code,
        OrderCode::JumpToObject
    );
}

/// A DFA attack becomes a jump-to-object order with the target's position as way point (mover.cpp:3240-3260), and a called shot
/// plays `RADIO_CALLED_SHOT`.
#[test]
fn a_dfa_attack_becomes_a_jump_to_object() {
    let (mut w, me, enemy) = world();
    let mut o = order(OrderCode::AttackObject, OrderOrigin::Player);
    o.attack_params.method = AttackMethod::Dfa;
    o.target = w.objects.watch_id(enemy);
    o.move_params.wait = true;
    o.move_params.way_path.mode[0] = TravelMode::Fast;
    super::handle_tactical_order(&mut w, me, o, 0, false);
    let stored = &w.pilots.slots[1].tac_orders[1];
    assert_eq!(stored.code, OrderCode::JumpToObject);
    assert_eq!(stored.move_params.way_path.points[0], point(300.0, 100.0));
    assert_eq!(stored.move_params.way_path.mode[0], TravelMode::Slow);
    assert!(!stored.move_params.wait);
}

/// `addQueuedTacOrder` (warrior.cpp:1573-1625): at most 16 queued orders (17th refused with 2); the first one starts at once when the
/// queue is already released and no player order is pending.
#[test]
fn queued_orders_cap_at_sixteen_and_the_first_starts_a_released_queue() {
    let (mut w, me, _) = world();
    for _ in 0..16 {
        assert_eq!(super::add_queued_order(&mut w, 1, Default::default()), 0);
    }
    assert_eq!(super::add_queued_order(&mut w, 1, Default::default()), 2);
    assert_eq!(w.pilots.slots[1].queue.len(), 16);
    assert!(!w.pilots.slots[1].queue_executing);
    assert!(!w.pilots.slots[1].new_order_received[1]);

    // released queue: the first order is executed at once (it becomes a player order)
    let (mut w, me2, _) = world();
    w.pilots.slots[1].queue_executing = true;
    let mut o = order(OrderCode::Stop, OrderOrigin::Player);
    o.unit_order = false;
    let packed = super::packed_order(&o, &w);
    assert_eq!(super::add_queued_order(&mut w, 1, packed), 0);
    assert!(w.pilots.slots[1].queue.is_empty());
    assert!(w.pilots.slots[1].new_order_received[1]);
    assert_eq!(w.pilots.slots[1].tac_orders[1].code, OrderCode::Stop);
    assert!(w.pilots.slots[1].cur_player_order_from_queue);
    let _ = (me, me2);
}

/// `executeTacOrderQueue` / `getNextQueuedTacOrder` (warrior.cpp:1792-1806, 1628-1668): popping unpacks the order, a looping queue walks
/// its index and wraps instead of removing.
#[test]
fn execute_tac_order_queue_pops_or_loops() {
    let (mut w, _, _) = world();
    let a = super::packed_order(&order(OrderCode::Stop, OrderOrigin::Player), &w);
    let b = super::packed_order(&order(OrderCode::Eject, OrderOrigin::Player), &w);
    w.pilots.slots[1].queue = vec![a.clone(), b.clone()];
    decide::execute_tac_order_queue(&mut w, 1);
    assert_eq!(w.pilots.slots[1].tac_orders[1].code, OrderCode::Stop);
    assert_eq!(w.pilots.slots[1].queue.len(), 1);
    assert!(w.pilots.slots[1].queue_executing);

    let (mut w, _, _) = world();
    w.pilots.slots[1].queue = vec![a, b];
    w.pilots.slots[1].queue_looping = true;
    decide::execute_tac_order_queue(&mut w, 1);
    assert_eq!(w.pilots.slots[1].tac_orders[1].code, OrderCode::Stop);
    decide::execute_tac_order_queue(&mut w, 1);
    assert_eq!(w.pilots.slots[1].tac_orders[1].code, OrderCode::Eject);
    decide::execute_tac_order_queue(&mut w, 1);
    assert_eq!(w.pilots.slots[1].tac_orders[1].code, OrderCode::Stop); // wrapped, nothing removed
    assert_eq!(w.pilots.slots[1].queue.len(), 2);
    // an empty queue stops executing
    w.pilots.slots[1].queue.clear();
    decide::execute_tac_order_queue(&mut w, 1);
    assert!(!w.pilots.slots[1].queue_executing);
}

// ---------------------------------------------------------------------------------------------------------------------
// mainDecisionTree
// ---------------------------------------------------------------------------------------------------------------------

/// The arbitration of `mainDecisionTree` (warrior.cpp:4998-5047) as a matrix of (order state, which slots received an order) ->
/// (which order becomes current, resulting state, slots purged).
#[test]
fn arbitration_matrix() {
    use OrderState::{Alarm, General, Player};
    // (state, received [general, player, alarm], expected current order code, expected state)
    let cases = [
        (
            General,
            [true, true, true],
            Some(OrderCode::HoldFire),
            Alarm,
        ),
        (General, [true, true, false], Some(OrderCode::Stop), Player),
        (
            General,
            [true, false, false],
            Some(OrderCode::Wait),
            General,
        ),
        (Player, [true, true, true], Some(OrderCode::HoldFire), Alarm),
        (Player, [true, true, false], Some(OrderCode::Stop), Player),
        (Player, [true, false, false], None, Player),
        (Alarm, [true, true, true], Some(OrderCode::Stop), Alarm), // player overrides an alarm order but the state stays ALARM
        (Alarm, [true, false, true], Some(OrderCode::HoldFire), Alarm),
        (Alarm, [true, false, false], None, Alarm),
    ];
    for (state, received, want, want_state) in cases {
        let (mut w, _, _) = world();
        let p = &mut w.pilots.slots[1];
        p.order_state = state;
        p.new_order_received = received;
        p.tac_orders[0] = order(OrderCode::Wait, OrderOrigin::Commander);
        p.tac_orders[1] = order(OrderCode::Stop, OrderOrigin::Player);
        p.tac_orders[2] = order(OrderCode::HoldFire, OrderOrigin::SelfOrder);
        // a current order that never completes, so the status check at the top does not interfere
        p.cur_order = order(OrderCode::Guard, OrderOrigin::Commander);
        decide::main_decision_tree(&mut w, 1);
        let p = &w.pilots.slots[1];
        match want {
            Some(code) => assert_eq!(p.cur_order.code, code, "{state:?} {received:?}"),
            None => assert_eq!(p.cur_order.code, OrderCode::Guard, "{state:?} {received:?}"),
        }
        assert_eq!(p.order_state, want_state, "{state:?} {received:?}");
        assert_eq!(p.new_order_received, [false; 3], "cleared every frame");
    }
}

/// warrior.cpp:5008-5012 / 5028-5030: a player order purges the general order (the brain must re-issue it); an alarm order pre-empting
/// a general one clears the current order first (`clearCurTacOrder(false)`).
#[test]
fn a_player_order_purges_the_general_order() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.new_order_received = [false, true, false];
    p.tac_orders[0] = order(OrderCode::Wait, OrderOrigin::Commander);
    p.tac_orders[1] = order(OrderCode::Stop, OrderOrigin::Player);
    p.cur_order = order(OrderCode::Guard, OrderOrigin::Commander);
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].tac_orders[0].code, OrderCode::None);
    assert_eq!(w.pilots.slots[1].order_state, OrderState::Player);
}

/// warrior.cpp:4921-4932: with nothing current and a released queue, the queued order is started in the same frame.
#[test]
fn the_queue_starts_when_nothing_is_current() {
    let (mut w, _, _) = world();
    let packed = super::packed_order(&order(OrderCode::HoldFire, OrderOrigin::Player), &w);
    w.pilots.slots[1].queue = vec![packed];
    w.pilots.slots[1].queue_executing = true;
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].cur_order.code, OrderCode::HoldFire);
    assert_eq!(w.pilots.slots[1].order_state, OrderState::Player);
    assert!(w.pilots.slots[1].queue.is_empty());
}

/// warrior.cpp:4934-4945: a completed current order is cleared (falls back through `clearCurTacOrder`), AND a player order that arrived
/// in the same frame is DROPPED (`newTacOrderReceived[PLAYER] = playerOrderQueued`).
#[test]
fn a_finished_order_is_cleared_and_drops_a_simultaneous_player_order() {
    let (mut w, _, _) = world();
    let p = &mut w.pilots.slots[1];
    p.cur_order = order(OrderCode::Stop, OrderOrigin::Player); // STOP is always done
    p.new_order_received = [false, true, false];
    p.tac_orders[1] = order(OrderCode::HoldFire, OrderOrigin::Player);
    decide::main_decision_tree(&mut w, 1);
    let p = &w.pilots.slots[1];
    assert_eq!(p.cur_order.code, OrderCode::None);
    assert_eq!(p.new_order_received, [false; 3]);
}

/// The brain timer (warrior.cpp:4980-4984): `brainUpdate += BrainUpdateFrequency` (2.25) from its OLD value, not from the current time.
#[test]
fn the_brain_timer_advances_from_its_old_value() {
    let (mut w, _, _) = world();
    w.clock.scenario_time = 5.0;
    w.pilots.slots[1].timers[0] = 0.2;
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].timers[0], 0.2 + 2.25);
    // not due: untouched
    w.pilots.slots[1].timers[0] = 9.0;
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].timers[0], 9.0);
}

/// `combatDecisionTree` sets `combatUpdate = scenarioTime + CombatUpdateFrequency` (warrior.cpp:3497) and `movementDecisionTree` sets
/// `movementUpdate = scenarioTime + MovementUpdateFrequency` (warrior.cpp:3993) when due; both are 0 without gamesys tables here.
#[test]
fn update_actions_run_the_combat_and_movement_trees_when_due() {
    let (mut w, _, _) = world();
    w.clock.scenario_time = 5.0;
    w.pilots.slots[1].timers = [99.0, 4.0, 4.0];
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].timers[1], 5.0); // 5 + 0
    assert_eq!(w.pilots.slots[1].timers[2], 5.0);
    w.pilots.slots[1].timers = [99.0, 6.0, 6.0];
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].timers[1..], [6.0, 6.0]); // not due
}

/// `updateActions` for a pilot whose team has its brains disabled (warrior.cpp:4871-4880): everything is cleared and the pilot does
/// nothing else.
#[test]
fn disabled_brains_clear_every_order() {
    let (mut w, _, _) = world();
    w.pilots.brains_enabled[0] = false;
    let p = &mut w.pilots.slots[1];
    p.cur_order = order(OrderCode::Guard, OrderOrigin::Commander);
    p.tac_orders[0] = order(OrderCode::Wait, OrderOrigin::Commander);
    decide::main_decision_tree(&mut w, 1);
    let p = &w.pilots.slots[1];
    assert_eq!(p.cur_order.code, OrderCode::None);
    assert!(p.tac_orders.iter().all(|o| o.code == OrderCode::None));
}

/// Target sanity (warrior.cpp:4963-4977): an `ATTACK_OBJECT` current order makes its target the last target.
#[test]
fn an_attack_order_makes_its_target_the_last_target() {
    let (mut w, _, enemy) = world();
    let wid = w.objects.watch_id(enemy);
    let mut o = order(OrderCode::AttackObject, OrderOrigin::Commander);
    o.target = wid;
    w.pilots.slots[1].cur_order = o;
    w.pilots.num_warriors_in_combat = 1;
    decide::main_decision_tree(&mut w, 1);
    assert_eq!(w.pilots.slots[1].last_target.0, wid);
}

/// `checkAlarms` (warrior.cpp:4743-4866): the alarm list is cleared every frame, the history is not (it is cleared by `runBrain`).
#[test]
fn check_alarms_clears_the_alarm_lists_but_not_the_history() {
    let (mut w, _, _) = world();
    warrior::trigger_alarm(&mut w, 1, alarm::COLLISION, 7);
    warrior::trigger_alarm(&mut w, 1, alarm::GATE_CLOSING, 8);
    decide::check_alarms(&mut w, 1);
    let p = &w.pilots.slots[1];
    assert!(p.alarms.iter().all(|a| a.is_empty()));
    assert_eq!(p.alarm_history[alarm::COLLISION], vec![7]);
    assert_eq!(p.alarm_history[alarm::GATE_CLOSING], vec![8]);
}

/// `handlePlayerOrder` (warrior.cpp:6600): a SHUTDOWN mech whose current order is not a power-down is woken (a self-origin
/// `orderPowerUp` -> alarm order of priority 255).
#[test]
fn a_player_order_wakes_a_shut_down_mech() {
    let (mut w, me, _) = world();
    w.objects.get_mut(me).unwrap().c.status = ObjectStatus::ShutDown;
    warrior::trigger_alarm(&mut w, 1, alarm::PLAYER_ORDER, 2);
    decide::check_alarms(&mut w, 1);
    assert_eq!(
        w.objects.get(me).unwrap().c.status,
        ObjectStatus::StartingUp
    );
    assert_eq!(w.pilots.slots[1].tac_orders[2].code, OrderCode::PowerUp);
    assert_eq!(w.pilots.slots[1].alarm_priority, 255);
}

// ---------------------------------------------------------------------------------------------------------------------
// Targeting
// ---------------------------------------------------------------------------------------------------------------------

/// `TargetPriorityList::insert` / `remove` (warrior.cpp:419-451): memmove semantics, the list never exceeds 20, an insert past `size` is
/// refused (2) and a full list refuses (1).
#[test]
fn target_priority_insert_and_remove() {
    use super::pilot::{TargetPriority, TargetPriorityList};
    let mut l = TargetPriorityList::default();
    let t = |k| TargetPriority {
        kind: k,
        params: [k, 0, 0],
    };
    assert_eq!(l.insert(-1, t(1)), 0);
    assert_eq!(l.insert(-1, t(2)), 0);
    assert_eq!(l.insert(0, t(3)), 0); // shift down
    assert_eq!(
        l.list[..3].iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![3, 1, 2]
    );
    assert_eq!(l.insert(9, t(9)), 2);
    l.remove(1);
    assert_eq!(
        l.list[..2].iter().map(|e| e.kind).collect::<Vec<_>>(),
        vec![3, 2]
    );
    l.remove(7); // out of range: ignored
    assert_eq!(l.size, 2);
    for _ in 2..20 {
        assert_eq!(l.insert(-1, t(5)), 0);
    }
    assert_eq!(l.insert(-1, t(6)), 1);
}

/// `calcTarget` adds `CONTACT_CRITERIA_ARMED` (128) to the entry's criteria EVERY call (`params[2] += ...`, warrior.cpp:507, 559), so
/// the stored value grows by 128 per call; here a `TARGET_PRIORITY_MOVER` entry with no object and criteria 0 stays "bad criteria"
/// only once the add made it non-zero.
#[test]
fn calc_target_adds_the_armed_criterion_on_every_call() {
    let (mut w, _, _) = world();
    w.pilots.slots[1].target_priorities.list[0] = super::pilot::TargetPriority {
        kind: targeting::prio::MOVER,
        params: [0, 300, 1],
    };
    w.pilots.slots[1].target_priorities.size = 1;
    let _ = targeting::calc_target(&mut w, 1, Vec3::default(), 0);
    assert_eq!(
        w.pilots.slots[1].target_priorities.list[0].params[2],
        1 + 128
    );
    let _ = targeting::calc_target(&mut w, 1, Vec3::default(), 0);
    assert_eq!(
        w.pilots.slots[1].target_priorities.list[0].params[2],
        1 + 256
    );
}

/// `TeamSensorSystem::meetsCriteria` (contact.cpp:815-865): a mover with no contact status never qualifies; `VISUAL` needs status 5,
/// `SENSOR` needs 1..4, `ENEMY` needs the relation, `NOT_DISABLED` a live target, `ARMED` a functional weapon.
#[test]
fn meets_criteria_truth_table() {
    let (mut w, _, enemy) = world();
    use targeting::criteria as c;
    let team = 0;
    assert!(!targeting::meets_criteria(&mut w, team, enemy, 0)); // CONTACT_NONE
    w.sensors.team_contact_status.insert(enemy, {
        let mut s = [0u8; crate::ids::MAX_TEAMS];
        s[0] = 5;
        s
    });
    assert!(targeting::meets_criteria(
        &mut w,
        team,
        enemy,
        c::ENEMY | c::VISUAL
    ));
    assert!(targeting::meets_criteria(
        &mut w,
        team,
        enemy,
        c::VISUAL_OR_SENSOR
    ));
    assert!(!targeting::meets_criteria(&mut w, team, enemy, c::SENSOR)); // status 5 is visual
    w.sensors.team_contact_status.get_mut(&enemy).unwrap()[0] = 3;
    assert!(targeting::meets_criteria(&mut w, team, enemy, c::SENSOR));
    assert!(!targeting::meets_criteria(&mut w, team, enemy, c::VISUAL));
    // armed: numFunctionalWeapons == 0 -> not armed
    assert!(!targeting::meets_criteria(&mut w, team, enemy, c::ARMED));
    if let Some(Kind::Mech(m)) = w.objects.get_mut(enemy).map(|o| &mut o.kind) {
        m.loadout.stats.num_functional_weapons = 2;
    }
    assert!(targeting::meets_criteria(&mut w, team, enemy, c::ARMED));
    w.objects.get_mut(enemy).unwrap().c.status = ObjectStatus::Disabled;
    assert!(!targeting::meets_criteria(
        &mut w,
        team,
        enemy,
        c::NOT_DISABLED
    ));
    // same-team looker: not an enemy
    assert!(!targeting::meets_criteria(&mut w, 1, enemy, c::ENEMY));
}

/// `coreScan` with an explicit object that is not a contact: `coreScanTargetWID = 0` and 0 is returned (warrior.cpp:5806-5835).
#[test]
fn core_scan_of_a_non_contact_returns_zero() {
    let (mut w, _, enemy) = world();
    w.pilots.slots[1].core_scan_target = WatchId(77);
    assert_eq!(orders::core_scan(&mut w, 1, Some(enemy), 0), 0);
    assert_eq!(w.pilots.slots[1].core_scan_target, WatchId(0));
}

// ---------------------------------------------------------------------------------------------------------------------
// Weapons status
// ---------------------------------------------------------------------------------------------------------------------

/// `calcWeaponsStatus`'s point-target indirect-fire fallback passes
/// `CELL_DISTANCE` (42.6667), not `MapCellDiagonal`, to
/// `Team::teamLineOfSight` (`warrior.cpp:3431`).  The direct ray sees the
/// endpoint ridge; the C++ cell-distance extent also sees and rejects it,
/// whereas the larger map-cell diagonal extent exempts it.  The fixture uses
/// an indirect component and ranges from the shipped data, never test code.
#[test]
fn calc_weapons_status_point_indirect_los_uses_cell_distance_endpoint_extent() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping shipped-data indirect-fire test");
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("system.cfg mounts");
    let tables = mc2_sim_data::gamesys::GameSystem::load(&mut vfs).expect("shipped gamesys");
    let components = mc2_sim_data::component::MasterComponentList::load(&mut vfs)
        .expect("shipped component list");
    let indirect_master = tables
        .mover_general
        .indirect_fire_weapons
        .iter()
        .copied()
        .find(|&id| {
            components
                .get(id)
                .and_then(|component| component.weapon())
                .is_some()
        })
        .expect("shipped indirect-fire component") as u8;

    let (mut w, me, _) = world();
    w.data.gamesys.visual_range_table = tables.general.visual_range_table.clone();
    w.data.gamesys.ranges = [
        tables.general.max_visual_radius,
        tables.general.base_sensor_range,
        tables.general.max_tree_los_cell_block as f32,
    ];
    w.data.gamesys.tables = Some(Rc::new(tables));
    w.data.components = Some(Rc::new(components));

    // This diagonal point/ridge pair has a direct ray blocked by the ridge.
    // The C++ CELL_DISTANCE endpoint exemption reaches past it, while the
    // smaller MapCellDiagonal exemption still tests and blocks it.
    let from = cell_world(&w, 20, 20);
    let target = cell_world(&w, 24, 14);
    let ridge = cell_world(&w, 23, 15);
    let (ridge_row, ridge_col) = w.terrain.world_to_cell(&ridge);
    w.terrain.set_local_height(ridge_row, ridge_col, 15);
    let (row, col) = w.terrain.world_to_cell(&from);
    let obj = w.objects.get_mut(me).expect("home mech");
    obj.c.position = from;
    obj.c.cell = (row as u16, col as u16);
    obj.c.rotation = -56.31;
    let Kind::Mech(mech) = &mut obj.kind else {
        panic!("home object is a mech");
    };
    mech.loadout.inventory = vec![InventoryItem {
        master_id: indirect_master,
        health: 1,
        ammo_index: 0,
        ..InventoryItem::default()
    }];
    mech.loadout.ammo_totals = vec![(i32::from(indirect_master), 99, 99)];
    mech.loadout.counts = (1, 0, 0);
    mech.loadout.stats.num_functional_weapons = 1;
    mech.loadout.stats.min_range = 0.0;
    mech.loadout.stats.max_range = 1_000.0;
    w.teams.teams[0]
        .as_mut()
        .expect("home team")
        .roster
        .push(w.objects.watch_id(me));

    assert!(
        !crate::combat::los::line_of_sight_point(&mut w, me, target),
        "the direct point ray is blocked by the endpoint ridge"
    );
    assert!(
        crate::object::hooks::team_line_of_sight(
            &mut w,
            0,
            target,
            decide::POINT_TARGET_TEAM_LOS_EXTENT,
        ),
        "CELL_DISTANCE exempts the endpoint ridge"
    );
    assert!(
        !crate::object::hooks::team_line_of_sight(
            &mut w,
            0,
            target,
            crate::combat::attack::map_cell_diagonal(),
        ),
        "the old MapCellDiagonal extent tests and blocks the ridge"
    );
    assert_eq!(
        decide::calc_weapons_status(&mut w, 1, None, Some(target)),
        1
    );
    assert!(
        w.pilots.slots[1].weapons_status[0] > 0,
        "the indirect weapon fires only through CELL_DISTANCE team LOS"
    );
}

/// `calcWeaponsStatus` (warrior.cpp:3377-3400): no target and no point -> `WEAPONS_STATUS_NO_TARGET` (-2); out of the min/max fire
/// range -> -3.
#[test]
fn calc_weapons_status_without_a_target_or_out_of_range() {
    let (mut w, _, enemy) = world();
    assert_eq!(decide::calc_weapons_status(&mut w, 1, None, None), -2);
    // a mech with no weapons has min/max range 0: any distance is out of range (>= max)
    assert_eq!(
        decide::calc_weapons_status(&mut w, 1, Some(enemy), None),
        -3
    );
}

/// `Team::teamLineOfSight` (team.cpp:762-851) checks every live roster mover
/// through the merged visual-range and terrain-ray helpers.  A 100-m table
/// entry is a 2,500-world-unit range (`radius * 25 * worldUnitsPerMeter`), so
/// the enemy at distance 200 is visible; disabling the only spotter removes
/// that sight line.
#[test]
fn team_line_of_sight_uses_live_roster_spotters() {
    let (mut w, me, enemy) = world();
    w.data.gamesys.visual_range_table = vec![100; 256];
    w.teams.teams[0]
        .as_mut()
        .unwrap()
        .roster
        .push(w.objects.watch_id(me));
    let target = w.objects.get(enemy).unwrap().c.position;
    assert!(crate::object::hooks::team_line_of_sight(
        &mut w, 0, target, 0.0
    ));
    w.objects.get_mut(me).unwrap().c.status = ObjectStatus::Disabled;
    assert!(!crate::object::hooks::team_line_of_sight(
        &mut w, 0, target, 0.0
    ));
}

/// Unreached / unported behaviour is named: a decision that needs the line of sight reports it.
#[test]
fn unavailable_kinds_are_reported_once_per_pilot() {
    let (mut w, me, _) = world();
    super::report_unavailable(&mut w, me, AiUnavailable::LineOfSight);
    super::report_unavailable(&mut w, me, AiUnavailable::LineOfSight);
    super::report_unavailable(&mut w, me, AiUnavailable::AdjacentAreaCell);
    assert_eq!(w.events.len(), 2);
    assert_eq!(w.ai_unavailable_frame.len(), 2);
}

/// `handleKilledTarget` (warrior.cpp): a killed mech counts under its class number, is remembered in the kill list (at most 85
/// watch ids) and earns `KillSkill[class]` gunnery points.
#[test]
fn killed_target_credits_the_class_and_caps_the_kill_list() {
    let (mut w, _, enemy) = world();
    let wid = w.objects.watch_id(enemy);
    warrior::trigger_alarm(&mut w, 1, alarm::KILLED_TARGET, wid.0 as i32);
    decide::handle_killed_target(&mut w, 1);
    let p = &w.pilots.slots[1];
    assert_eq!(p.num_mech_kills[0][1], 1);
    assert_eq!(p.killed, vec![wid]);
    // a full kill list stops growing but the kill is still counted
    w.pilots.slots[1].killed = vec![WatchId(1); 85];
    w.pilots.slots[1].alarms[alarm::KILLED_TARGET].clear();
    warrior::trigger_alarm(&mut w, 1, alarm::KILLED_TARGET, wid.0 as i32);
    decide::handle_killed_target(&mut w, 1);
    assert_eq!(w.pilots.slots[1].killed.len(), 85);
    assert_eq!(w.pilots.slots[1].num_mech_kills[0][1], 2);
}

// ---------------------------------------------------------------------------------------------------------------------
// Deferred behaviour must be reported unavailable, never as success (review of c238a8b)
// ---------------------------------------------------------------------------------------------------------------------

fn sim_of(w: World) -> crate::Sim {
    crate::Sim::new(crate::mission::init::LoadOutcome {
        world: w,
        complete: true,
        missing_hooks: Vec::new(),
    })
}

fn step_once(sim: &mut crate::Sim, frame: u64) -> crate::FrameOutput {
    sim.step(crate::FrameInput::fixed(
        60,
        frame,
        std::rc::Rc::new(crate::visibility::AllVisible),
        Vec::new(),
    ))
    .expect("step")
}

const SCRIPT: &str = "module t : integer;\ncode\n return(playVideo(\"t.avi\"));\nendmodule.\n";

fn script_env(w: &mut World) -> mc2_abl::InstId {
    let mut src = mc2_abl::MemorySource::new();
    src.insert("t.abl", SCRIPT);
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("t.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    inst
}

fn run_script(
    w: &mut World,
    make: impl FnOnce(&mut World) -> crate::mission::bindings::SimBindings<'_>,
) {
    let inst = script_env(w);
    let mut env = w.abl.take().expect("env");
    {
        let mut b = make(w);
        env.execute(inst, &mut b).expect("run");
    }
    w.abl = Some(env);
}

/// `playvideo` is a presentation request, not an unavailable native, whatever `CurObject` is: no object (mission script),
/// a non-mover, or a mover.
#[test]
fn presentation_requests_do_not_need_a_pilot_mover() {
    use crate::mission::bindings::SimBindings;
    let id = mc2_abl::natives::NATIVES
        .iter()
        .position(|n| n.name == "playvideo")
        .expect("native") as u16;
    // mission script: no CurObject
    let (mut w, _, _) = world();
    run_script(&mut w, |w| SimBindings::mission(w));
    assert!(
        !w.ai_unavailable_frame
            .contains(&(0, AiUnavailable::Native { id })),
        "mission"
    );
    assert!(w
        .events
        .iter()
        .any(|e| matches!(e, crate::SimEvent::PlayVideo { .. })));

    // pilot script whose CurObject is a non-mover
    let (mut w, _, _) = world();
    let handle = ObjectHandle(w.objects.list.len() as u32);
    let mut c = Common::new(ObjectClass::Building, handle);
    c.flags |= flags::EXISTS;
    c.part_id = crate::ids::PartId(900);
    w.objects.list.push(Some(Obj {
        c,
        kind: Kind::Carnage(Box::default()),
    }));
    let wid = w.objects.watch_id(handle);
    run_script(&mut w, |w| {
        let mut b = SimBindings::pilot(w, 1);
        b.cur_object = wid;
        b
    });
    assert!(
        !w.ai_unavailable_frame
            .contains(&(0, AiUnavailable::Native { id })),
        "non-mover"
    );
    assert!(w
        .events
        .iter()
        .any(|e| matches!(e, crate::SimEvent::PlayVideo { .. })));

    // pilot script on its mover: attributed to the mover as before
    let (mut w, _, _) = world();
    run_script(&mut w, |w| SimBindings::pilot(w, 1));
    assert!(
        !w.ai_unavailable_frame
            .contains(&(512, AiUnavailable::Native { id })),
        "mover"
    );
    assert!(w
        .events
        .iter()
        .any(|e| matches!(e, crate::SimEvent::PlayVideo { .. })));

    // and through Sim::step with a mission script
    let (mut w, _, _) = world();
    let inst = script_env(&mut w);
    w.mission.brain = Some(inst);
    let mut sim = sim_of(w);
    let out = step_once(&mut sim, 1);
    assert!(out.unavailable.is_empty());
    assert!(out
        .events
        .iter()
        .any(|e| matches!(e, crate::SimEvent::PlayVideo { .. })));
}

/// `calcAttackPlan` (ablmc2.cpp:212-250), hand-run: attackers (threat 10 each) a0..a3 over defenders D0 (threat 20), D1 (threat 10).
/// a0: toughest = D1 (the loop over d=0 finds ratio 0 == 0 and total 0 !> 0), total[1]=10, ratio[1]=10/10=1.0 -> D1.
/// a1: ratio[0]=0 < ratio[1]=1 -> D0, total[0]=10, ratio[0]=10/20=0 (integer division, ablmc2.cpp:242). a2: ratio[0]=0 < 1 -> D0,
/// total[0]=20, ratio[0]=20/20=1.
/// a3: ratio[0]==ratio[1]==1 and total[0]=20 > total[1]=10 -> toughest = D0. Each attacker gets `PILOT_ALARM_ATTACK_ORDER` with its
/// target's watch id, in attacker order (the targets are resolved first, then the alarms are triggered).
#[test]
fn calc_attack_plan_spreads_attackers_by_threat_ratio() {
    let (mut w, me, enemy) = world();
    for h in [me, enemy] {
        w.objects.get_mut(h).unwrap().c.threat_rating = 10;
    }
    let mut defenders = Vec::new();
    for (i, threat) in [20i16, 10].into_iter().enumerate() {
        let handle = ObjectHandle(w.objects.list.len() as u32);
        let mut o = w.objects.get(enemy).unwrap().clone();
        o.c.handle = handle;
        o.c.threat_rating = threat;
        o.c.part_id = crate::ids::PartId(700 + i as i32);
        w.objects.list.push(Some(o));
        defenders.push(handle);
    }
    let wid = |w: &mut World, h| w.objects.watch_id(h).0 as i32;
    let attackers = [me, enemy, me, enemy];
    super::natives::calc_attack_plan(&mut w, &attackers, &defenders);
    let (d0, d1) = (wid(&mut w, defenders[0]), wid(&mut w, defenders[1]));
    let order = alarm::ATTACK_ORDER;
    // pilot 1 flies `me` (a0 -> D1, a2 -> D0); pilot 2 flies `enemy` (a1 -> D0, a3 -> D0)
    assert_eq!(w.pilots.slots[1].alarms[order], vec![d1, d0]);
    assert_eq!(w.pilots.slots[2].alarms[order], vec![d0, d0]);
    // empty lists do nothing
    super::natives::calc_attack_plan(&mut w, &[], &defenders);
    super::natives::calc_attack_plan(&mut w, &attackers, &[]);
    assert_eq!(w.pilots.slots[1].alarms[order].len(), 2);
}

/// ablmc2.cpp:242 divides `long / long` before the float store. Two defenders (threat 20) and two attackers (threat 10):
/// a0: toughest = D1 (ties, totals equal), total[1]=10, ratio[1]=10/20=0 (integer). a1: ratio[0]==ratio[1]==0 and
/// total[0]=0 !> total[1]=10 -> D1 again. Both attackers go to the last defender (fractional division would split them).
#[test]
fn calc_attack_plan_uses_integer_division() {
    let (mut w, me, enemy) = world();
    for h in [me, enemy] {
        w.objects.get_mut(h).unwrap().c.threat_rating = 10;
    }
    let mut defenders = Vec::new();
    for i in 0..2 {
        let handle = ObjectHandle(w.objects.list.len() as u32);
        let mut o = w.objects.get(enemy).unwrap().clone();
        o.c.handle = handle;
        o.c.threat_rating = 20;
        o.c.part_id = crate::ids::PartId(800 + i);
        w.objects.list.push(Some(o));
        defenders.push(handle);
    }
    let d1 = w.objects.watch_id(defenders[1]).0 as i32;
    super::natives::calc_attack_plan(&mut w, &[me, enemy], &defenders);
    let order = alarm::ATTACK_ORDER;
    assert_eq!(w.pilots.slots[1].alarms[order], vec![d1]);
    assert_eq!(w.pilots.slots[2].alarms[order], vec![d1]);
}

fn add_turret_control(w: &mut World) -> (ObjectHandle, ObjectHandle) {
    let mut handles = Vec::new();
    for (class, x) in [(ObjectClass::Building, 310.0), (ObjectClass::Turret, 320.0)] {
        let handle = ObjectHandle((w.objects.list.len() + handles.len()) as u32);
        let mut c = Common::new(class, handle);
        c.flags |= flags::EXISTS | flags::AWAKE;
        c.position = point(x, 100.0);
        c.part_id = crate::ids::PartId(800 + handles.len() as i32);
        handles.push((handle, c));
    }
    let building = handles[0].0;
    let turret = handles[1].0;
    let bs = crate::object::structure::BuildingState {
        team_id: 1,
        list_id: 0,
        ..Default::default()
    };
    w.objects.list.push(Some(Obj {
        c: handles[0].1.clone(),
        kind: Kind::Building(Box::new(bs)),
    }));
    let mut ts = crate::object::structure::TurretState::new(0.0);
    ts.team_id = 1;
    ts.target = w.objects.watch_id(ObjectHandle(1));
    ts.parent = w.objects.watch_id(building);
    let mut tc = handles[1].1.clone();
    tc.threat_rating = 5;
    w.objects.list.push(Some(Obj {
        c: tc,
        kind: Kind::Turret(Box::new(ts)),
    }));
    w.objects.turret_controls = vec![building];
    w.objects.turrets = vec![turret];
    (building, turret)
}

/// `calcTacOrder` with no goal and a turret threat (warrior.cpp:7605-7630): the biggest threatening control building is chosen
/// (here: the one turret aiming at the pilot, threat 5 >= minThreat 1, 210 <= range 1000, enemy team, enabled). A mech can capture
/// (`canCapture`) so the action is CAPTURE; a ground vehicle cannot, so it attacks
/// the control building with an ATTACK_OBJECT sub-order (ranged, pursue, optimal range).
#[test]
fn calc_tac_order_picks_capture_for_a_mech_and_attack_for_a_ground_vehicle() {
    let (mut w, me, _) = world();
    let (building, _) = add_turret_control(&mut w);
    assert_eq!(
        super::goal::calc_turret_threats(&w, 1, 1000.0, 1),
        Some(building)
    );
    let mut out = TacticalOrder::init();
    let r = super::goal::calc_tac_order(
        &mut w,
        1,
        super::pilot::goal_action::NONE,
        WatchId(0),
        point(0.0, 0.0),
        -1.0,
        0,
        0,
        1000.0,
        1,
        &mut out,
    );
    assert_eq!(r, 0);
    assert_eq!(out.code, OrderCode::Capture);
    assert!(w.ai_unavailable_frame.is_empty());

    // ground vehicle: cannot capture -> ATTACK on the building (the attack goal is production).
    w.ai_unavailable_frame.clear();
    w.objects.get_mut(me).unwrap().c.class = ObjectClass::GroundVehicle;
    let mut out = TacticalOrder::init();
    let r = super::goal::calc_tac_order(
        &mut w,
        1,
        super::pilot::goal_action::NONE,
        WatchId(0),
        point(0.0, 0.0),
        -1.0,
        0,
        0,
        1000.0,
        1,
        &mut out,
    );
    assert_eq!(r, 0);
    assert_eq!(out.code, OrderCode::AttackObject);
    assert_eq!(out.target, w.objects.watch_id(building));
    assert!(out.attack_params.pursue);
    assert_eq!(out.attack_params.method, AttackMethod::Ranged);
}

/// Presentation-native state and requests are concrete bindings, not unavailable stubs.
#[test]
fn presentation_natives_are_implemented() {
    for name in [
        "playvideo",
        "setmoviemode",
        "endmoviemode",
        "fadetocolor",
        "forcemovieend",
        "animationcallout",
        "tutorialtext",
        "guiisaoe",
        "freezegui",
    ] {
        let id = mc2_abl::natives::NATIVES
            .iter()
            .position(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name} missing")) as u16;
        assert!(super::natives::IMPLEMENTED.contains(&id), "{name}");
    }
}

/// Pilot, audio and presentation natives used by shipped scripts are bound.
#[test]
fn shipped_pilot_native_ids_are_implemented() {
    for (id, name) in [
        (69, "playdigitalmusic"),
        (72, "playvideo"),
        (191, "getsensorsactive"),
        (192, "getcurrentmusicid"),
        (193, "getmissiontune"),
        (207, "playwave"),
        (224, "isplayingvoiceover"),
        (225, "getlogisticstime"),
        (226, "stopvoiceover"),
        (227, "incallout"),
        (228, "setinvulnerable"),
    ] {
        assert_eq!(mc2_abl::natives::NATIVES[id].name, name, "native id map");
        assert!(
            super::natives::IMPLEMENTED.contains(&(id as u16)),
            "{name} ({id})"
        );
    }
}

/// `getMovers` selects the roster for team ids 500..508. These ABL calls
/// exercise team targeting through the production native dispatcher.
#[test]
fn team_object_natives_use_the_team_roster() {
    let (mut w, first, second) = world();
    let first_wid = w.objects.watch_id(first);
    let second_wid = w.objects.watch_id(second);
    let Kind::Mech(second_mech) = &mut w.objects.get_mut(second).unwrap().kind else {
        unreachable!()
    };
    second_mech.m.team = crate::ids::TeamId(0);
    w.teams.teams[0].as_mut().unwrap().roster = vec![first_wid, second_wid];
    assert_eq!(
        run_pilot_script(&mut w, "", "objectSuicide(500); return(0);"),
        0
    );
    assert!(!w.objects.get(first).unwrap().exists());
    assert!(!w.objects.get(second).unwrap().exists());

    let (mut w, first, _second) = world();
    let first_wid = w.objects.watch_id(first);
    w.teams.teams[0].as_mut().unwrap().roster = vec![first_wid];
    assert_eq!(
        run_pilot_script(&mut w, "", "return(objectVisible(500, 512));"),
        1
    );

    let (mut w, first, second) = world();
    let first_wid = w.objects.watch_id(first);
    let second_wid = w.objects.watch_id(second);
    w.teams.teams[0].as_mut().unwrap().roster = vec![first_wid, second_wid];
    w.objects.get_mut(second).unwrap().c.set_awake(false);
    let vars = "real[3] areaPoint;";
    let code = "areaPoint[0] = 100.0; areaPoint[1] = 100.0; areaPoint[2] = 0.0; if (inArea(500, areaPoint, 50.0, -1)) then return(1); endif; return(0);";
    assert_eq!(run_pilot_script_with_static_vars(&mut w, vars, code), 1);

    let (mut w, first, second) = world();
    let first_wid = w.objects.watch_id(first);
    let second_wid = w.objects.watch_id(second);
    let Kind::Mech(second_mech) = &mut w.objects.get_mut(second).unwrap().kind else {
        unreachable!()
    };
    second_mech.m.team = crate::ids::TeamId(0);
    w.teams.teams[0].as_mut().unwrap().roster = vec![first_wid, second_wid];
    assert_eq!(
        run_pilot_script(&mut w, "", "objectChangeSides(500, 1); return(0);"),
        0
    );
    for h in [first, second] {
        let o = w.objects.get(h).unwrap();
        assert_eq!(
            crate::combat::core_of(o).unwrap().team,
            crate::ids::TeamId(1)
        );
        assert!(w.teams.teams[1]
            .as_ref()
            .unwrap()
            .roster
            .contains(&o.c.watch_id));
    }
    assert!(w.teams.teams[0].as_ref().unwrap().roster.is_empty());

    let (mut w, first, second) = world();
    let first_wid = w.objects.watch_id(first);
    let second_wid = w.objects.watch_id(second);
    if let Kind::Mech(m) = &mut w.objects.get_mut(first).unwrap().kind {
        m.m.squad_id = 16;
        m.m.group_id = 3;
    }
    if let Kind::Mech(m) = &mut w.objects.get_mut(second).unwrap().kind {
        m.m.squad_id = 16;
        m.m.group_id = 4;
        m.m.commander = crate::ids::CommanderId(0);
        m.m.team = crate::ids::TeamId(0);
    }
    w.teams.teams[0].as_mut().unwrap().roster = vec![first_wid, second_wid];
    assert_eq!(
        run_pilot_script(&mut w, "", "objectChangeSides(16, 1); return(0);"),
        0
    );
    for h in [first, second] {
        let o = w.objects.get(h).unwrap();
        assert_eq!(
            crate::combat::core_of(o).unwrap().team,
            crate::ids::TeamId(1)
        );
        assert!(w.teams.teams[1]
            .as_ref()
            .unwrap()
            .roster
            .contains(&o.c.watch_id));
    }
    assert!(w.teams.teams[0].as_ref().unwrap().roster.is_empty());
}

#[test]
fn end_movie_mode_updates_the_sim_owned_presentation_state() {
    let (mut w, _, _) = world();
    assert_eq!(
        run_pilot_script(&mut w, "", "setMovieMode; endMovieMode; return(0);"),
        0
    );
    let id = mc2_abl::natives::NATIVES
        .iter()
        .position(|native| native.name == "endmoviemode")
        .unwrap() as u16;
    assert!(!w
        .ai_unavailable_frame
        .contains(&(512, AiUnavailable::Native { id })));
    assert!(w.presentation.movie_mode);
    assert!(w.presentation.ending);
}

#[test]
fn relative_position_native_returns_terrain_height_through_abl() {
    let (mut w, _, _) = world();
    w.terrain = Terrain::from_elevations(64, &vec![20.0; 4096]);
    let vars = "real[3] point; real[3] output;";
    let code = "point[0] = 100.0; point[1] = 100.0; point[2] = 0.0; getRelativePositionToPoint(point, 0.0, 10.0, 0, output); return(output[2]);";
    assert_eq!(run_pilot_real_script(&mut w, vars, code), 20.0);
}

/// Object query/setter natives execute through the same ABL dispatch used by mission scripts.
/// Expected values follow `execGetPilotID`, `execGetPilotWounds`, `execGetArmorPts`,
/// `execGetObjectDmgPts`, and the corresponding setters in `code/ablmc2.cpp`.
#[test]
fn object_query_and_setter_natives_return_expected_values_through_abl() {
    let (mut w, first, _) = world();
    let id = w.objects.get(first).unwrap().c.part_id.0;
    let query = format!(
        "setPilotWounds({id}, 7); setTonnage({id}, 42.5); setExplosionDamage({id}, 12.0); setExplosionRadius({id}, 17.0); setCurrentBRValue({id}, 88); return(getPilotID({id}) + getObjectDmgPts({id}) + getObjectMaxDmg({id}) + getArmorPts({id}) + getMaxArmor({id}));"
    );
    assert_eq!(run_pilot_script(&mut w, "", &query), 1);
    let o = w.objects.get(first).unwrap();
    assert_eq!(o.c.tonnage, 42.5);
    assert_eq!(o.c.expl_damage, 12.0);
    assert_eq!(o.c.expl_radius, 17.0);
    assert_eq!(o.c.cur_cv, 88);
    assert_eq!(w.pilots.slots[1].wounds, 6.0);
    let wounds = format!("return(getPilotWounds({id}));");
    assert_eq!(run_pilot_real_script(&mut w, "", &wounds), 6.0);
    let sensors = format!("setSensorRange({id}, 1200.0); return(getSensorsWorking({id}));");
    assert_eq!(run_pilot_script(&mut w, "", &sensors), -1);
}

/// Coordinate conversion and relative-position natives are called by ABL with typed arrays,
/// matching `execConvertCoords` and `getRelativePositionToObject` in `code/ablmc2.cpp`.
#[test]
fn coordinate_and_relative_object_natives_return_expected_values_through_abl() {
    let (mut w, first, _) = world();
    let vars = "real[3] world; integer[2] cell;";
    let code = "world[0] = 3.0; world[1] = 4.0; world[2] = 0.0; convertCoords(0, world, cell); return(cell[0] * 100 + cell[1]);";
    // C++ terrain.h:346 maps (3,4) on this 64-vertex, 128-units-per-vertex
    // fixture to row 95, column 96. Keep this literal independent of world_to_cell.
    assert_eq!(run_pilot_script(&mut w, vars, code), 9596);

    let id = w.objects.get(first).unwrap().c.part_id.0;
    let vars = "real[3] output;";
    let code =
        format!("getRelativePositionToObject({id}, 0.0, 0.0, 1, output); return(output[1]);");
    let actual = run_pilot_real_script(&mut w, vars, &code);
    let expected_y = w.objects.get(first).unwrap().c.position.y;
    assert_eq!(actual, expected_y);
}

/// `execGetUnitStatus` pushes `getStatusRating() * 100` (ablmc2.cpp:4402-4418).
#[test]
fn unit_status_native_returns_live_mech_rating_through_abl() {
    let (mut w, first, _) = world();
    let id = w.objects.get(first).unwrap().c.part_id.0;
    let Kind::Mech(mech) = &mut w.objects.get_mut(first).unwrap().kind else {
        unreachable!()
    };
    mech.loadout.armor = vec![(100.0, 100); 11];
    mech.loadout.stats.weapon_effectiveness = 50.0;
    mech.loadout.stats.max_weapon_effectiveness = 100.0;
    w.pilots.slots[usize::from(mech.m.pilot_slot)].wounds = 2.0;

    let code = format!("return(getUnitStatus({id}));");
    assert_eq!(run_pilot_real_script(&mut w, "", &code), 42.5);
}

/// Home-team, alarm, active, and lifecycle results are visible through ABL and update the
/// corresponding world state (`ablmc2.cpp` `getHomeTeam`, alarm, `setObjectActive`, and object lifecycle natives).
#[test]
fn mission_object_state_natives_return_expected_values_through_abl() {
    let (mut w, first, second) = world();
    assert_eq!(run_pilot_script(&mut w, "", "return(getHomeTeam);"), 500);
    assert_eq!(
        run_pilot_script(&mut w, "", "setGeneralAlarm(3); return(getGeneralAlarm);"),
        1
    );
    assert_eq!(
        run_pilot_script(&mut w, "", "setInvulnerable(true); return(0);"),
        0
    );
    assert!(w.mission.invulnerable);
    assert_eq!(
        run_pilot_script(&mut w, "", "setInvulnerable(false); return(0);"),
        0
    );
    assert!(!w.mission.invulnerable);
    let id = w.objects.get(first).unwrap().c.part_id.0;
    let commander = format!("return(objectCommander({id}));");
    assert_eq!(run_pilot_script(&mut w, "", &commander), 0);
    let script = format!("setObjectActive({id}, false); return(getPilotID({id}));");
    assert_eq!(run_pilot_script(&mut w, "", &script), 1);
    assert!(!w.objects.get(first).unwrap().c.awake());
    assert!(w.objects.get(second).unwrap().c.awake());

    let lifecycle = format!("objectSuicide({id}); return(objectCreate({id}));");
    assert_eq!(run_pilot_script(&mut w, "", &lifecycle), id);
    assert!(w.objects.get(first).unwrap().exists());
    let remove = format!("return(objectRemove({id}));");
    assert_eq!(run_pilot_script(&mut w, "", &remove), 0);
    assert!(w.objects.get(first).unwrap().c.flag(flags::REMOVED));
}

/// These calls assert their ABL return/no-op behavior for the supplied mover: structure damage
/// does not apply to a mover, createInfantry returns zero, and disabled presentation features
/// remain no-ops (`ablmc2.cpp` setObjectDamage, createInfantry, setAnimation, setBuildingName,
/// addPrisoner). A shipped infantry spawn outcome is still unverified because its pilot brain
/// currently fails to compile in the Rust ABL parser; the limitation is recorded in porting-log.
#[test]
fn remaining_committed_object_natives_execute_through_abl() {
    let (mut w, first, _) = world();
    let id = w.objects.get(first).unwrap().c.part_id.0;
    let vars = "real[3] position;";
    let script = format!(
        "position[0] = 200.0; position[1] = 200.0; position[2] = 0.0; setAnimation({id}, 2, 1); setBuildingName({id}, 17); createInfantry(0, 0, position, 900); return(setObjectDamage({id}, 50));"
    );
    assert_eq!(run_pilot_script(&mut w, vars, &script), 0);
    assert!(w.objects.get(first).unwrap().exists());
    assert_eq!(
        run_pilot_script(&mut w, "", "return(addPrisoner(0, 0));"),
        -1
    );
}

#[test]
fn gate_natives_leave_shipped_gates_unlocked() {
    let (mut w, _, _) = world();
    let handle = ObjectHandle(w.objects.list.len() as u32);
    let mut common = Common::new(ObjectClass::Gate, handle);
    common.flags |= flags::EXISTS;
    common.part_id = crate::ids::PartId(900);
    w.objects.list.push(Some(Obj {
        c: common,
        kind: Kind::Gate(Box::default()),
    }));
    let calls = [
        "lockGateOpen(900); return(0);",
        "lockGateClosed(900); return(0);",
        "releaseGateLock(900); return(0);",
    ];
    for code in calls {
        assert_eq!(run_pilot_script(&mut w, "", code), 0);
        crate::object::structure::update_gate(&mut w, handle);
        let Some(Kind::Gate(gate)) = w.objects.get(handle).map(|o| &o.kind) else {
            unreachable!()
        };
        assert!(!gate.locked_open && !gate.locked_close);
    }
    assert_eq!(
        run_pilot_script(
            &mut w,
            "",
            "if (isGateOpen(900)) then return(1); endif; return(0);"
        ),
        0
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// requestHelp / requestTarget / requestShelter executed as ABL scripts through SimBindings (review-4)
// ---------------------------------------------------------------------------------------------------------------------

/// Run `code` (a module body returning an integer; `vars` in the var section) as pilot 1's script and return its result.
fn run_pilot_script(w: &mut World, vars: &str, code: &str) -> i32 {
    use crate::mission::bindings::SimBindings;
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "t.abl",
        format!("module t : integer;\nvar\n{vars}\ncode\n{code}\nendmodule.\n"),
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("t.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    {
        let mut b = SimBindings::pilot(w, 1);
        env.execute(inst, &mut b).expect("run");
    }
    let r = env.get_integer(inst);
    w.abl = Some(env);
    r
}

fn run_pilot_real_script(w: &mut World, vars: &str, code: &str) -> f32 {
    use crate::mission::bindings::SimBindings;
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "t.abl",
        format!("module t : real;\nvar\n{vars}\ncode\n{code}\nendmodule.\n"),
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("t.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    {
        let mut b = SimBindings::pilot(w, 1);
        env.execute(inst, &mut b).expect("run");
    }
    let r = env.get_real(inst);
    w.abl = Some(env);
    r
}

fn run_pilot_script_with_static_vars(w: &mut World, vars: &str, code: &str) -> i32 {
    use crate::mission::bindings::SimBindings;
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "t.abl",
        format!("module t : integer;\nvar\n{vars}\ncode\n{code}\nendmodule.\n"),
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("t.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    {
        let mut b = SimBindings::pilot(w, 1);
        env.execute(inst, &mut b).expect("run");
    }
    let r = env.get_integer(inst);
    w.abl = Some(env);
    r
}

/// `execPlayDigitalMusic` (ablmc2.cpp:2606-2617) returns zero and sends the
/// requested id to presentation. `getCurrentMusicId` is not inferred from
/// that request: the C++ reads SoundSystem's live playback state.
#[test]
fn play_digital_music_forwards_the_requested_id() {
    let (mut w, _, _) = world();
    assert_eq!(
        run_pilot_script(&mut w, "", "playDigitalMusic(42); return(0);"),
        0
    );
    assert_eq!(w.events, vec![SimEvent::Music(42)]);
}

/// `execPlayVideo` (`ablmc2.cpp:2655-2668`) forwards the filename to presentation and returns zero immediately.
#[test]
fn video_native_forwards_the_presentation_request() {
    let (mut w, _, _) = world();
    assert_eq!(
        run_pilot_script(&mut w, "", "return(playVideo(\"intro.bik\"));"),
        0
    );
    assert_eq!(
        w.events,
        vec![SimEvent::PlayVideo {
            name: "intro.bik".to_string(),
        }]
    );
    assert!(!w
        .ai_unavailable_frame
        .contains(&(512, AiUnavailable::Native { id: 72 })));
    assert!(w.ai_native_calls_frame.contains(&(512, 72)));
}

/// Presentation state and requests are owned by the deterministic sim bridge, not reported unavailable.
#[test]
fn presentation_state_natives_are_available() {
    let (mut w, _, _) = world();
    let id = 72;
    let _ = run_pilot_script(&mut w, "", "return(playVideo(\"intro.bik\"));");
    assert!(
        !w.ai_unavailable_frame
            .contains(&(512, AiUnavailable::Native { id })),
        "native {id}"
    );
    assert!(
        w.ai_native_calls_frame.contains(&(512, id)),
        "native {id} trace"
    );
}

/// `execGetSensorsActive` (ablmc2.cpp:2793-2796) is exactly the sensor
/// manager's `enemyInLOS` flag.
#[test]
fn get_sensors_active_reads_enemy_in_los() {
    let (mut w, _, _) = world();
    let code = "if getSensorsActive then return(1); endif; return(0);";
    w.sensors.enemy_in_los = false;
    assert_eq!(run_pilot_script(&mut w, "", code), 0);
    w.sensors.enemy_in_los = true;
    assert_eq!(run_pilot_script(&mut w, "", code), 1);
}

/// `execPlayWave` (ablmc2.cpp:6259-6268) sends the supplied support-sample
/// filename and returns zero.
#[test]
fn play_wave_forwards_the_support_sample_name() {
    let (mut w, _, _) = world();
    assert_eq!(
        run_pilot_script(&mut w, "", "return(playWave(\"help.wav\", 7));"),
        0
    );
    assert_eq!(w.events, vec![SimEvent::VoiceOver("help.wav".into())]);
}

/// `execStopVoiceOver` (ablmc2.cpp:5975-5983) emits the cancellation request;
/// only its live completion query is deliberately unavailable.
#[test]
fn stop_voice_over_forwards_the_cancellation_request() {
    let (mut w, _, _) = world();
    w.audio.voice_over = true;
    w.audio.voice_busy = true;
    let code = "stopVoiceOver; return(0);";
    assert_eq!(run_pilot_script(&mut w, "", code), 0);
    assert_eq!(w.events, vec![SimEvent::StopSupport]);
}

/// `execGetLogisticsTime` (ablmc2.cpp:5985-5993) returns `timeGetTime()`;
/// the initial deterministic frame clock is zero milliseconds.
#[test]
fn get_logistics_time_uses_the_frame_clock() {
    let (mut w, _, _) = world();
    assert_eq!(
        run_pilot_script(
            &mut w,
            "",
            "if getLogisticsTime == 0.0 then return(1); endif; return(0);",
        ),
        1
    );
}

/// The value is the current virtual `timeGetTime`, not a one-shot initial
/// value: `execGetLogisticsTime` (ablmc2.cpp:5985-5993) reads it again after
/// later frames too.
#[test]
fn get_logistics_time_advances_with_the_frame_clock() {
    let (mut w, _, _) = world();
    w.clock.begin_frame(30.0, 100_000, false);
    w.clock.begin_frame(30.0, 100_033, false);
    assert_eq!(
        run_pilot_script(
            &mut w,
            "",
            "if getLogisticsTime == 100033.0 then return(1); endif; return(0);",
        ),
        1
    );
}

/// `execSetInvulnerable` (ablmc2.cpp:5861-5869) directly controls the
/// home-team damage gate read by `combat::damage::mech_hit`.
#[test]
fn set_invulnerable_updates_the_mission_damage_gate() {
    let (mut w, _, _) = world();
    assert_eq!(
        run_pilot_script(&mut w, "", "setInvulnerable(true); return(0);"),
        0
    );
    assert!(w.mission.invulnerable);
    assert_eq!(
        run_pilot_script(&mut w, "", "setInvulnerable(false); return(0);"),
        0
    );
    assert!(!w.mission.invulnerable);
}

fn give_weapons(w: &mut World, h: ObjectHandle) {
    if let Kind::Mech(m) = &mut w.objects.get_mut(h).unwrap().kind {
        m.loadout.stats.num_functional_weapons = 1;
    }
}

/// A second team-1 mover (pilot 2's slot) with the given threat and part id, on the team roster.
fn add_enemy(w: &mut World, template: ObjectHandle, threat: i16, part: i32) -> ObjectHandle {
    let handle = ObjectHandle(w.objects.list.len() as u32);
    let mut o = w.objects.get(template).unwrap().clone();
    o.c.handle = handle;
    o.c.threat_rating = threat;
    o.c.part_id = crate::ids::PartId(part);
    w.objects.list.push(Some(o));
    w.objects.movers.push(handle);
    let wid = w.objects.watch_id(handle);
    w.teams.teams[1].as_mut().unwrap().roster.push(wid);
    handle
}

/// `execRequestTarget` (ablmc2.cpp:6031) + `calcBestTarget` (ablmc2.cpp:283-300) through the script. Friendly `me` (threat 10, team 0)
/// already targets E1 (threat 20, part 513); E2 (threat 10, part 600) is the other enemy. Sorted descending: [E1, E2]. total[0]=10,
/// ratio[0]=10/20=0 (long/long), ratio[1]=0. Scan from the back: best=1; d=0: ratio equal and total[0]=10 > total[1]=0 -> best=E1 (513).
/// A fractional ratio (0.5) would leave E2 (600). No enemies in range -> 0.
#[test]
fn request_target_script_picks_the_enemy_with_integer_ratio() {
    let (mut w, me, enemy) = world();
    for h in [me, enemy] {
        give_weapons(&mut w, h);
    }
    w.objects.get_mut(me).unwrap().c.threat_rating = 10;
    w.objects.get_mut(enemy).unwrap().c.threat_rating = 20;
    let e2 = add_enemy(&mut w, enemy, 10, 600);
    let wid_me = w.objects.watch_id(me);
    w.teams.teams[0].as_mut().unwrap().roster.push(wid_me);
    w.pilots.slots[1].will_help = true;
    let wid_e1 = w.objects.watch_id(enemy);
    w.pilots.slots[1].last_target = (wid_e1, 0.0);
    w.pilots.slots[1].last_target_flags = [true, true, false];
    let _ = e2;
    let near = "real[2] c;";
    let call = |r: &str| format!("c[0] = 100.0; c[1] = 100.0; return(requestTarget(c, {r}));");
    assert_eq!(run_pilot_script(&mut w, near, &call("500.0")), 513);
    // out of range of everything -> no enemies -> 0
    assert_eq!(run_pilot_script(&mut w, near, &call("1.0")), 0);
}

/// `execRequestHelp` (ablmc2.cpp:5996): friendly `me` (pilot 1) is told to attack. Enemy team 1 holds E1 (threat 20, part 513) and
/// E2 (threat 10, part 600); both are within the enemy radius and in the roster (sorted descending: [E1, E2]). `calcAttackPlan`
/// with one attacker (threat 10): toughest starts at the last defender (E2), d=0: ratio[0]=0 == ratio[1]=0 and total equal -> E2.
/// So pilot 1 gets exactly one ATTACK_ORDER alarm carrying E2's watch id; the native returns 0. With priority 0 (`ignoreOrder`
/// false) a friendly that already has an order is skipped; asking for help against an own-team part does nothing.
#[test]
fn request_help_script_alarms_friendlies_against_the_sorted_enemy() {
    let (mut w, me, enemy) = world();
    for h in [me, enemy] {
        give_weapons(&mut w, h);
    }
    w.objects.get_mut(me).unwrap().c.threat_rating = 10;
    w.objects.get_mut(enemy).unwrap().c.threat_rating = 20;
    let e2 = add_enemy(&mut w, enemy, 10, 600);
    let wid_e1 = w.objects.watch_id(enemy);
    w.teams.teams[1].as_mut().unwrap().roster.insert(0, wid_e1);
    let wid_me = w.objects.watch_id(me);
    w.teams.teams[0].as_mut().unwrap().roster.push(wid_me);
    for s in [1, 2] {
        w.pilots.slots[s].will_help = true;
    }
    let vars = "real[2] f; real[2] e;";
    let call = |part: i32, prio: i32| {
        format!(
            "f[0] = 100.0; f[1] = 100.0; e[0] = 300.0; e[1] = 100.0; \
             requestHelp({part}, f, 50.0, e, 50.0, {prio}); return(1);"
        )
    };
    let order = alarm::ATTACK_ORDER;
    assert_eq!(run_pilot_script(&mut w, vars, &call(513, 1)), 1);
    let wid_e2 = w.objects.watch_id(e2).0 as i32;
    assert_eq!(w.pilots.slots[1].alarms[order], vec![wid_e2]);
    assert!(w.pilots.slots[2].alarms[order].is_empty());

    // priority 0: pilot 1 now holds an order -> skipped, no new alarm
    w.pilots.slots[1].cur_order = order_of(OrderCode::Wait);
    assert_eq!(run_pilot_script(&mut w, vars, &call(513, 0)), 1);
    assert_eq!(w.pilots.slots[1].alarms[order].len(), 1);
    // an unknown part id and an own-team part do nothing
    assert_eq!(run_pilot_script(&mut w, vars, &call(9999, 1)), 1);
    assert_eq!(w.pilots.slots[1].alarms[order].len(), 1);
}

fn order_of(code: OrderCode) -> TacticalOrder {
    order(code, OrderOrigin::Player)
}

/// `execRequestShelter` (ablmc2.cpp:6065): buildings within `range` of `CurObject` (mech at (100,100)), at most ten, chosen with one
/// `RandomNumber(n)` draw. Three buildings lie within 50 (x = 110, 120, 130), one at x = 400 does not, so n = 3 and the answer is the
/// building at the index a twin RNG (same mode and seed) draws; exactly one draw is consumed. No building in range -> 0, no draw.
#[test]
fn request_shelter_script_draws_one_random_building() {
    let (mut w, _, _) = world();
    let mut parts = Vec::new();
    for (i, x) in [110.0f32, 120.0, 130.0, 400.0].into_iter().enumerate() {
        let handle = ObjectHandle(w.objects.list.len() as u32);
        let mut c = Common::new(ObjectClass::Building, handle);
        c.flags |= flags::EXISTS;
        c.part_id = crate::ids::PartId(800 + i as i32);
        c.position = Vec3 {
            x,
            y: 100.0,
            z: 0.0,
        };
        w.objects.list.push(Some(Obj {
            c,
            kind: Kind::Carnage(Box::default()),
        }));
        w.objects.buildings.push(handle);
        parts.push(800 + i as i32);
    }
    let twin = World::new(RngMode::LegacyMsvc, 1);
    let (first, second) = {
        let mut r = twin.rng.0.borrow_mut();
        (r.sim.random_number(3) as usize, r.sim.random_number(3))
    };
    let got = run_pilot_script(&mut w, "", "return(requestShelter(50));");
    assert_eq!(got, parts[first]);
    // exactly one draw was consumed: the next draw continues the twin's sequence
    assert_eq!(w.rng.0.borrow_mut().sim.random_number(3), second);
    // nothing in range -> 0 and no draw
    let (mut w2, _, _) = world();
    assert_eq!(
        run_pilot_script(&mut w2, "", "return(requestShelter(50));"),
        0
    );
    let twin2 = World::new(RngMode::LegacyMsvc, 1);
    let want = twin2.rng.0.borrow_mut().sim.random_number(3);
    assert_eq!(w2.rng.0.borrow_mut().sim.random_number(3), want);
}

/// Calls queued while the ABL environment is taken, with no native return hook to run them, are reported and dropped when the
/// outer script returns. They must not run late after `restore_env`.
#[test]
fn brain_calls_without_a_native_return_hook_are_reported_and_dropped() {
    let (mut w, _, _) = world();
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\nfunction handleattackorder;\ncode\n playVideo(\"b.avi\");\nendfunction;\ncode\n return(7);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("b.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    w.pilots.slots[1].brain = Some(inst);

    // another script is running: the environment is out of the world
    let outer = brain::take_env(&mut w).expect("env");
    brain::call_alarm_handler(&mut w, 1, 9);
    assert_eq!(run_brain_result(&mut w), 0, "run_brain waits too");
    assert_eq!(w.deferred_brain_calls.len(), 2);
    assert!(!w.events.iter().any(|e| matches!(
        e,
        crate::SimEvent::AiUnavailable {
            what: AiUnavailable::NestedBrain,
            ..
        }
    )));
    assert!(
        !w.events
            .iter()
            .any(|e| matches!(e, SimEvent::PlayVideo { .. })),
        "nothing ran yet"
    );

    // the outer script returns without having run a native: the C++ has no queue to drain afterwards, so the late requests are
    // reported and dropped, never run after the outermost script
    brain::restore_env(&mut w, outer);
    assert!(w.deferred_brain_calls.is_empty());
    assert!(w.abl.is_some() && !w.abl_running);
    assert!(!w
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::PlayVideo { .. })));
    assert!(w.events.iter().any(|e| matches!(
        e,
        SimEvent::AiUnavailable {
            what: AiUnavailable::NestedBrain,
            ..
        }
    )));
}

fn run_brain_result(w: &mut World) -> i32 {
    brain::run_brain(w, 1)
}

/// A coherent, ejectable mech fixture with intact critical spaces and the requested head armor.
fn ejectable_mech_world(head_armor: f32) -> (World, ObjectHandle) {
    let (mut w, mech, _) = world();
    if let Some(Kind::Mech(m)) = w.objects.get_mut(mech).map(|o| &mut o.kind) {
        m.loadout.body = vec![Default::default(); mc2_sim_data::mech::layout::NUM_BODY_LOCATIONS];
        for section in &mut m.loadout.body {
            section.critical_spaces = vec![(255, false); 16];
        }
        m.loadout.armor = vec![Default::default(); mc2_sim_data::mech::layout::NUM_ARMOR_LOCATIONS];
        m.loadout.armor[crate::combat::stats::body::HEAD].0 = head_armor;
    }
    (w, mech)
}

// ---------------------------------------------------------------------------------------------------------------------
// attacker list and isteamtargeting executed as ABL scripts (review-1)
// ---------------------------------------------------------------------------------------------------------------------

fn native_id(name: &str) -> u16 {
    mc2_abl::natives::NATIVES
        .iter()
        .position(|n| n.name == name)
        .expect("native") as u16
}

/// An alarm raised inside a native runs its handler inside that native call (C++ order): the handler's effect lands between the
/// outer script's statements, not after the whole outer script, and the outer script's variables and result are untouched.
#[test]
fn alarm_raised_in_a_native_runs_before_the_next_outer_statement() {
    let (mut w, _, _) = world();
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\nfunction handleattackorder;\ncode\n playDigitalMusic(99);\nendfunction;\ncode\n return(7);\nendmodule.\n",
    );
    src.insert(
        "t.abl",
        "module t : integer;\nvar\n integer n;\ncode\n n = 5;\n playDigitalMusic(1);\n playDigitalMusic(2);\n return(n);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let hb = env.pre_process("b.abl").expect("compile b");
    let brain_inst = env.new_instance(hb).expect("instance b");
    let ht = env.pre_process("t.abl").expect("compile t");
    let outer = env.new_instance(ht).expect("instance t");
    w.pilots.slots[1].brain = Some(brain_inst);
    // the first native of the outer script raises the alarm (as handleAlarm does from inside a native)
    let mut env = Some(env);
    let mut taken = env.take().expect("env");
    w.abl_running = true;
    brain::call_alarm_handler(&mut w, 1, 9);
    assert_eq!(w.deferred_brain_calls.len(), 1);
    w.abl_running = false;
    let result = {
        let mut b = crate::mission::bindings::SimBindings::pilot(&mut w, 2);
        taken.execute(outer, &mut b).expect("run");
        taken.get_integer(outer)
    };
    assert_eq!(
        result, 5,
        "the outer script's variables survive the nested run"
    );
    assert!(w.deferred_brain_calls.is_empty());
    assert_eq!(
        w.events,
        vec![SimEvent::Music(1), SimEvent::Music(99), SimEvent::Music(2)]
    );
    assert_eq!(taken.exec_depth(), 0);
}

/// `isTeamTargeting(500 + team, target, except)` through the script path: counts a roster pilot's current target, drops a
/// destroyed target (`getCurrentTarget` clears it first) and honours `except`; the native is no longer reported unavailable.
#[test]
fn abl_isteamtargeting_uses_the_current_target() {
    let (mut w, h1, h2) = world();
    let mover = w.objects.watch_id(h1);
    let target = w.objects.watch_id(h2);
    w.teams.teams[0].as_mut().expect("team").roster = vec![mover];
    let code = format!(
        "if isTeamTargeting(500, {}, 0) then\n return(1);\n endif;\n return(0);",
        target.0
    );
    let except = format!(
        "if isTeamTargeting(500, {}, {}) then\n return(1);\n endif;\n return(0);",
        target.0, mover.0
    );
    assert_eq!(run_pilot_script(&mut w, "", &code), 0);
    warrior::set_last_target(&mut w, 1, Some(h2), false, false);
    w.teams.teams[0].as_mut().unwrap().roster.clear();
    assert_eq!(
        run_pilot_script(&mut w, "", &code),
        0,
        "Team::isTargeting only checks movers in that team's roster"
    );
    w.teams.teams[0].as_mut().unwrap().roster.push(mover);
    assert_eq!(run_pilot_script(&mut w, "", &code), 1);
    assert_eq!(
        run_pilot_script(&mut w, "", &except),
        0,
        "the asker is excluded"
    );
    assert!(!w.ai_unavailable_frame.iter().any(
        |(_, k)| matches!(k, AiUnavailable::Native { id } if *id == native_id("isteamtargeting"))
    ));
    // the target is destroyed: the stale last target is not counted and is cleared
    w.objects
        .get_mut(h2)
        .expect("target")
        .c
        .set_status(ObjectStatus::Destroyed, true);
    assert_eq!(run_pilot_script(&mut w, "", &code), 0);
    assert_eq!(w.pilots.slots[1].last_target.0, WatchId(0));
}

/// Calls still queued when the outermost script returns are not drained by `restore_env` (no such drain in the C++): they do not
/// run, and are reported `NestedBrain`.
#[test]
fn restore_env_does_not_drain_the_queue() {
    let (mut w, _, _) = world();
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\ncode\n playVideo(\"b.avi\");\n return(0);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("b.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    w.pilots.slots[1].brain = Some(inst);
    let outer = brain::take_env(&mut w).expect("env");
    for _ in 0..3 {
        w.deferred_brain_calls
            .push(brain::DeferredBrainCall::Run(1));
    }
    brain::restore_env(&mut w, outer);
    assert!(w.deferred_brain_calls.is_empty());
    assert!(!w.abl_running);
    assert!(!w
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::PlayVideo { .. })));
    assert!(w.events.iter().any(|e| matches!(
        e,
        SimEvent::AiUnavailable {
            what: AiUnavailable::NestedBrain,
            ..
        }
    )));
}

/// A real alarm-producing native: `coreEject` disables the mech, and the C++ runs the brain's incapacitation handler inside
/// `Mover::disable` BEFORE `setStatus(DISABLED)` (mover.cpp:6533-6541). The handler must see the mover not yet disabled, and the
/// outer script's next statement (after the native returned) must see it disabled.
#[test]
fn eject_native_runs_incapacitation_handler_before_the_mover_is_disabled() {
    let (mut w, h1) = ejectable_mech_world(0.0);
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\nfunction handleincapacitationofvehicle;\ncode\n playDigitalMusic(100 + objectStatus(getId));\nendfunction;\ncode\n coreEject;\n playDigitalMusic(200 + objectStatus(getId));\n return(0);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("b.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    w.pilots.slots[1].brain = Some(inst);
    brain::run_brain(&mut w, 1);
    assert!(w.objects.get(h1).unwrap().c.is_disabled());
    let music: Vec<_> = w
        .events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Music(m) => Some(*m),
            _ => None,
        })
        .collect();
    assert_eq!(
        music,
        vec![
            100 + ObjectStatus::Normal as i64,
            200 + ObjectStatus::Disabled as i64
        ],
        "handler saw Normal, the outer script then saw Disabled"
    );
    assert!(w.deferred_brain_calls.is_empty());
}

/// `coreEject` on a mech with head armor: the C++ runs the incapacitation handler inside `MechWarrior::eject` (warrior.cpp:2033),
/// before `handleEjection` blows the head (mech.cpp:6796), so the handler still counts the head armor; the script's next statement
/// does not.
#[test]
fn eject_native_handler_sees_the_head_armor_before_the_head_blows() {
    let (mut w, h1) = ejectable_mech_world(10.0);
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\nfunction handleincapacitationofvehicle;\ncode\n playDigitalMusic(100 + getArmorPts(getId));\nendfunction;\ncode\n coreEject;\n playDigitalMusic(200 + getArmorPts(getId));\n return(0);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("b.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    w.pilots.slots[1].brain = Some(inst);
    brain::run_brain(&mut w, 1);
    assert!(w.objects.get(h1).unwrap().c.is_disabled());
    let music: Vec<_> = w
        .events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Music(m) => Some(*m),
            _ => None,
        })
        .collect();
    assert_eq!(
        music,
        vec![110, 200],
        "handler saw the head armor, the script after the native did not"
    );
    assert!(w.deferred_brain_calls.is_empty());
}

/// `coreEject` whose incapacitation handler gives its own general order (`corePower(false)`): the C++ runs the handler inside
/// `handleEjection`'s `eject` and installs `Eject` afterwards (warrior.cpp:5533-5535), so the final general order is `Eject`.
#[test]
fn eject_native_installs_the_eject_order_after_the_handler() {
    let (mut w, h1) = ejectable_mech_world(0.0);
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\nfunction handleincapacitationofvehicle;\ncode\n corePower(false);\nendfunction;\ncode\n coreEject;\n return(0);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("b.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    w.pilots.slots[1].brain = Some(inst);
    brain::run_brain(&mut w, 1);
    assert!(w.objects.get(h1).unwrap().c.is_disabled());
    assert_eq!(w.pilots.slots[1].tac_orders[0].code, OrderCode::Eject);
    assert!(w.deferred_brain_calls.is_empty());
}

/// One shared budget bounds the nested runs of an outermost script: handlers beyond it stay queued and are reported `NestedBrain`
/// (never run late, never an endless loop).
#[test]
fn nested_runs_share_one_budget_and_report_the_rest() {
    let (mut w, _, _) = world();
    let mut src = mc2_abl::MemorySource::new();
    src.insert(
        "b.abl",
        "module b : integer;\nfunction handleattackorder;\ncode\n playDigitalMusic(1);\nendfunction;\ncode\n return(0);\nendmodule.\n",
    );
    let mut env = mc2_abl::Env::new(
        mc2_abl::Config::default(),
        src,
        Box::new(mc2_abl::RngHost::new(w.rng.clone())),
    );
    let h = env.pre_process("b.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    w.abl = Some(env);
    w.pilots.slots[1].brain = Some(inst);
    let mut outer = brain::take_env(&mut w).expect("env");
    for _ in 0..200 {
        w.deferred_brain_calls
            .push(brain::DeferredBrainCall::Alarm(1, 9));
    }
    brain::run_nested(&mut w, &mut outer);
    let ran = w
        .events
        .iter()
        .filter(|e| matches!(e, SimEvent::Music(1)))
        .count();
    assert!(ran > 0 && ran < 200, "ran {ran}");
    assert_eq!(ran + w.deferred_brain_calls.len(), 200);
    // a further native in the same outer script cannot run more
    brain::run_nested(&mut w, &mut outer);
    assert_eq!(
        w.events
            .iter()
            .filter(|e| matches!(e, SimEvent::Music(1)))
            .count(),
        ran
    );
    brain::restore_env(&mut w, outer);
    assert!(w.deferred_brain_calls.is_empty());
    assert!(w.events.iter().any(|e| matches!(
        e,
        SimEvent::AiUnavailable {
            what: AiUnavailable::NestedBrain,
            ..
        }
    )));
}
