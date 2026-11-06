// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MechWarrior` methods (warrior.cpp), part 1: access to the pilot's mover, the three order slots and `curTacOrder`, the move-order
//! accessors lane B uses, targets, the order queue and alarms. Every function takes the world and the pilot's pool slot (the C++
//! `this`), so the borrow of the pilot never overlaps the objects it reads.
//!
//! Frame position: all of this runs inside `ai::pilot_update` (= `MechWarrior::mainDecisionTree`) except the order setters, which
//! `Mover::handleTacticalOrder` and the ABL natives call from other points of the frame.

use super::moveorders::{self as mo, clear_move_path, MOVEGOAL_LOCATION, MOVEGOAL_NONE};
use super::pilot::{AttackOrders, OrderState, Pilot};
use crate::command::{OrderCode, OrderOrigin, TacticalOrder};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::World;

/// `MAX_ALARM_TRIGGERS`, `PILOT_ALARM_*` codes used here (warrior.h:296-315).
pub mod alarm {
    /// `PILOT_ALARM_TARGET_OF_WEAPONFIRE`.
    pub const TARGET_OF_WEAPONFIRE: usize = 0;
    /// `PILOT_ALARM_HIT_BY_WEAPONFIRE`.
    pub const HIT_BY_WEAPONFIRE: usize = 1;
    /// `PILOT_ALARM_DAMAGE_TAKEN_RATE`.
    pub const DAMAGE_TAKEN_RATE: usize = 2;
    /// `PILOT_ALARM_DEATH_OF_MATE`.
    pub const DEATH_OF_MATE: usize = 3;
    /// `PILOT_ALARM_FRIENDLY_VEHICLE_CRIPPLED`.
    pub const FRIENDLY_VEHICLE_CRIPPLED: usize = 4;
    /// `PILOT_ALARM_FRIENDLY_VEHICLE_DESTROYED`.
    pub const FRIENDLY_VEHICLE_DESTROYED: usize = 5;
    /// `PILOT_ALARM_VEHICLE_INCAPACITATED`.
    pub const VEHICLE_INCAPACITATED: usize = 6;
    /// `PILOT_ALARM_VEHICLE_DESTROYED`.
    pub const VEHICLE_DESTROYED: usize = 7;
    /// `PILOT_ALARM_VEHICLE_WITHDRAWN`.
    pub const VEHICLE_WITHDRAWN: usize = 8;
    /// `PILOT_ALARM_ATTACK_ORDER`.
    pub const ATTACK_ORDER: usize = 9;
    /// `PILOT_ALARM_COLLISION`.
    pub const COLLISION: usize = 10;
    /// `PILOT_ALARM_GUARD_RADIUS_BREACH`.
    pub const GUARD_RADIUS_BREACH: usize = 11;
    /// `PILOT_ALARM_KILLED_TARGET`.
    pub const KILLED_TARGET: usize = 12;
    /// `PILOT_ALARM_MATE_FIRED_WEAPON`.
    pub const MATE_FIRED_WEAPON: usize = 13;
    /// `PILOT_ALARM_PLAYER_ORDER`.
    pub const PLAYER_ORDER: usize = 14;
    /// `PILOT_ALARM_NO_MOVEPATH`.
    pub const NO_MOVEPATH: usize = 15;
    /// `PILOT_ALARM_GATE_CLOSING`.
    pub const GATE_CLOSING: usize = 16;
    /// `PILOT_ALARM_FIRED_WEAPON`.
    pub const FIRED_WEAPON: usize = 17;
    /// `PILOT_ALARM_NEW_MOVER`.
    pub const NEW_MOVER: usize = 18;
}

/// `TACORDER_FAILURE` (tacordr.h:41).
pub const TACORDER_FAILURE: i32 = 0;
/// `TACORDER_SUCCESS`.
pub const TACORDER_SUCCESS: i32 = 1;

/// `BrainUpdateFrequency` (warrior.cpp:218).
pub const BRAIN_UPDATE_FREQUENCY: f32 = 2.25;

// ---------------------------------------------------------------------------------------------------------------------
// Access
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::getVehicle()` as a handle: the object `vehicleWID` points to.
pub fn vehicle(w: &World, slot: u16) -> Option<ObjectHandle> {
    let wid = w.pilots.get(slot)?.mover;
    w.objects.by_watch_id(wid)
}

/// The pilot slot flying `mover` (a mech or vehicle with a pilot).
pub fn slot_of(w: &World, mover: ObjectHandle) -> Option<u16> {
    crate::combat::fx::pilot_slot(w, mover)
}

/// `scenarioTime`.
pub fn now(w: &World) -> f32 {
    w.clock.scenario_time
}

/// The shared mover fields of the pilot's vehicle.
pub fn core(w: &World, slot: u16) -> Option<&crate::mover::MoverCore> {
    let h = vehicle(w, slot)?;
    crate::combat::core_of(w.objects.get(h)?)
}

/// Mutable [`core`].
pub fn core_mut(w: &mut World, slot: u16) -> Option<&mut crate::mover::MoverCore> {
    let h = vehicle(w, slot)?;
    match &mut w.objects.get_mut(h)?.kind {
        crate::object::Kind::Mech(m) => Some(&mut m.m),
        crate::object::Kind::Vehicle(v) => Some(&mut v.m),
        _ => None,
    }
}

/// `MechWarrior::teamId`: the team of the pilot (`Mover::setTeamId` keeps it equal to the mover's).
pub fn team_id(w: &World, slot: u16) -> i32 {
    vehicle(w, slot)
        .and_then(|h| w.objects.get(h))
        .map_or(-1, |o| o.team_id())
}

/// `MechWarrior::underHomeCommand()`.
pub fn under_home_command(w: &World, slot: u16) -> bool {
    vehicle(w, slot)
        .and_then(|h| w.objects.get(h))
        .is_some_and(|o| o.commander_id() == crate::combat::HOME_COMMANDER)
}

/// `MechWarrior::getCommander() == Commander::home`.
pub fn commander_is_home(w: &World, slot: u16) -> bool {
    under_home_command(w, slot)
}

/// `brainsEnabled[teamId]` (a team of -1 is not enabled; callers test `teamId == -1` themselves where the C++ does).
pub fn brains_enabled(w: &World, team: i32) -> bool {
    usize::try_from(team)
        .ok()
        .and_then(|t| w.pilots.brains_enabled.get(t))
        .copied()
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------------------------------
// Alarms
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::triggerAlarm(alarmCode, triggerId)` (warrior.cpp:4558): -1 when either list is full.
pub fn trigger_alarm(w: &mut World, slot: u16, code: usize, trigger: i32) -> i32 {
    let Some(p) = w.pilots.get_mut(slot) else {
        return -1;
    };
    if p.alarms[code].len() == super::pilot::MAX_ALARM_TRIGGERS
        || p.alarm_history[code].len() == super::pilot::MAX_ALARM_TRIGGERS
    {
        return -1;
    }
    p.alarms[code].push(trigger);
    p.alarm_history[code].push(trigger);
    0
}

// ---------------------------------------------------------------------------------------------------------------------
// Orders: the three slots, curTacOrder, clearing
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::setGeneralTacOrder` (warrior.cpp:4535).
pub fn set_general_tac_order(w: &mut World, slot: u16, order: TacticalOrder) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.tac_orders[0] = order;
        p.new_order_received[0] = true;
    }
}

/// `MechWarrior::setPlayerTacOrder(order, fromQueue)` (warrior.cpp:4543): a player order that does not come from the queue also
/// clears the queue.
pub fn set_player_tac_order(w: &mut World, slot: u16, order: TacticalOrder, from_queue: bool) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.tac_orders[1] = order;
        p.new_order_received[1] = true;
        p.cur_player_order_from_queue = from_queue;
    }
    if !from_queue {
        clear_tac_order_queue(w, slot);
    }
}

/// `MechWarrior::setAlarmTacOrder(order, priority)` (warrior.cpp:4555): only an order at least as urgent as the current alarm
/// priority is taken.
pub fn set_alarm_tac_order(w: &mut World, slot: u16, order: TacticalOrder, priority: i32) {
    if let Some(p) = w.pilots.get_mut(slot) {
        if priority >= p.alarm_priority {
            p.tac_orders[2] = order;
            p.new_order_received[2] = true;
            p.alarm_priority = priority;
        }
    }
}

/// `MechWarrior::setCurTacOrder(order)` (warrior.cpp:4515): current and last order, and the in-combat count.
pub fn set_cur_tac_order(w: &mut World, slot: u16, order: TacticalOrder) {
    let combat = order.is_combat_order();
    if let Some(p) = w.pilots.get_mut(slot) {
        p.cur_order = order.clone();
        p.last_order = order;
    }
    if combat {
        w.pilots.num_warriors_in_combat += 1;
    }
}

/// `MechWarrior::setMainGoal(action, obj, location, range)` (warrior.cpp:1790).
pub fn set_main_goal(
    w: &mut World,
    slot: u16,
    action: i32,
    object: Option<ObjectHandle>,
    location: Option<Vec3>,
    range: f32,
) {
    let wid = object.map_or(WatchId(0), |h| w.objects.watch_id(h));
    if let Some(p) = w.pilots.get_mut(slot) {
        p.main_goal.action = action;
        p.main_goal.object = wid;
        p.main_goal.location = location.unwrap_or_default();
        p.main_goal.control_radius = range;
    }
}

/// `MechWarrior::clearAttackOrders()` (warrior.cpp:2406).
pub fn clear_attack_orders(w: &mut World, slot: u16) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.attack_orders.clear();
    }
}

/// `AttackOrders::init()` (warrior.cpp:1273) at the current time.
pub fn init_attack_orders(w: &mut World, slot: u16) {
    let t = now(w);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.attack_orders = AttackOrders::init(t);
    }
}

/// `MechWarrior::clearMoveOrders()` (warrior.cpp:2418): no goal, no way path, both paths cleared, forward move state, yield, wait and
/// last-step timers off, no twist, no global path, and the path manager forgets the pilot.
pub fn clear_move_orders(w: &mut World, slot: u16) {
    if let Some(p) = w.pilots.get_mut(slot) {
        let m = &mut p.lane_b_move;
        m.clear_goal(); // setMoveGoal(MOVEGOAL_NONE, NULL) -> clearMoveGoal
        m.goal_type = MOVEGOAL_NONE;
        m.way_path.clear(); // setMoveWayPath(0): numWayPts = 0
        m.cur_way_pt = 0;
        m.cur_way_dir = 0;
        for path in &mut m.paths {
            clear_move_path(path);
        }
        m.move_state = mo::MOVESTATE_FORWARD;
        m.move_state_goal = mo::MOVESTATE_FORWARD;
        m.set_yield_time(-1.0);
        m.yield_state = 0;
        m.wait_for_point_time = -1.0;
        m.time_of_last_step = -1.0;
        m.twisting = false;
        // setMoveGlobalPath(NULL, 0): the count is zeroed, the step array keeps its stale contents
        m.num_global_steps = 0;
        m.cur_global_step = 0;
    }
    w.paths.remove(slot);
}

/// `MechWarrior::clearCurTacOrder(updateTacOrder)` (warrior.cpp:4397-4496).
pub fn clear_cur_tac_order(w: &mut World, slot: u16, update_tac_order: bool) {
    let t = now(w);
    let Some(p) = w.pilots.get(slot) else { return };
    if p.cur_order.is_combat_order() {
        w.pilots.num_warriors_in_combat -= 1;
    }
    if let Some(core) = core_mut(w, slot) {
        core.suppression_fire = 0;
    }
    if let Some(p) = w.pilots.get_mut(slot) {
        p.cur_order = TacticalOrder::init();
    }
    clear_move_orders(w, slot);
    trigger_alarm(w, slot, alarm::NO_MOVEPATH, mo::err::TACORDER_CLEARED);
    clear_attack_orders(w, slot);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.last_order.last_time = t;
    }
    if !update_tac_order {
        return;
    }
    let new_order: Option<TacticalOrder>;
    let Some(p) = w.pilots.get_mut(slot) else {
        return;
    };
    match p.order_state {
        OrderState::General => {
            if !p.new_order_received[0] {
                p.tac_orders[0] = TacticalOrder::init();
            }
            // If we reach this, then we have no orders... (newTacOrder keeps code NONE)
            new_order = None;
        }
        OrderState::Player => {
            if !p.new_order_received[1] {
                p.tac_orders[1] = TacticalOrder::init();
                p.cur_player_order_from_queue = false;
            }
            new_order = Some(p.tac_orders[0].clone());
            p.order_state = OrderState::General;
        }
        OrderState::Alarm => {
            if !p.new_order_received[2] {
                p.tac_orders[2] = TacticalOrder::init();
            }
            p.alarm_priority = 0;
            if p.tac_orders[1].code != OrderCode::None {
                new_order = Some(p.tac_orders[1].clone());
                p.order_state = OrderState::Player;
            } else {
                new_order = Some(p.tac_orders[0].clone());
                p.order_state = OrderState::General;
            }
        }
    }
    if let Some(order) = new_order.filter(|o| o.code != OrderCode::None) {
        start_order(w, slot, order, false);
        if let Some(p) = w.pilots.get_mut(slot) {
            p.time_of_last_orders = -1.0;
        }
    }
    if let Some(p) = w.pilots.get_mut(slot) {
        p.new_order_received = [false; 3];
    }
}

/// The block both `clearCurTacOrder` and `mainDecisionTree` run to start a new order (warrior.cpp:4451-4487, 5051-5100): reset the
/// move and attack orders around the two allocated paths (`keep_move_params` leaves `numSteps` of a move order's current path alone,
/// the `mainDecisionTree` form), `execute` the order, play its acknowledgement, make it current and set the goal plan's main goal.
pub fn start_order(w: &mut World, slot: u16, order: TacticalOrder, main_tree: bool) {
    let move_order = matches!(order.code, OrderCode::MoveToPoint | OrderCode::MoveToObject);
    let mut running = false;
    if let Some(p) = w.pilots.get_mut(slot) {
        let m = &mut p.lane_b_move;
        if main_tree && move_order {
            m.paths[1].num_steps = 0;
        } else {
            m.paths[0].num_steps = 0;
            m.paths[1].num_steps = 0;
        }
        let move_state = m.move_state;
        let move_state_goal = m.move_state_goal;
        running = m.run;
        let t = w.clock.scenario_time;
        m.init(t);
        if main_tree {
            m.run = running;
        }
        m.move_state = move_state;
        m.move_state_goal = move_state_goal;
    }
    let _ = running;
    w.paths.remove(slot);
    init_attack_orders(w, slot);
    let mut order = order;
    let message = super::tacorder::execute(w, slot, &mut order);
    let state_is_player = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.order_state == OrderState::Player);
    if state_is_player {
        radio_message(w, slot, message);
    }
    set_cur_tac_order(w, slot, order.clone());
    let use_goal_plan = w.pilots.get(slot).is_some_and(|p| p.use_goal_plan);
    if use_goal_plan {
        let move_point = order
            .move_params
            .way_path
            .points
            .first()
            .copied()
            .unwrap_or_default();
        if !order.sub_order {
            match order.code {
                OrderCode::MoveToPoint => set_main_goal(
                    w,
                    slot,
                    super::pilot::goal_action::MOVE,
                    None,
                    Some(move_point),
                    -1.0,
                ),
                OrderCode::AttackObject => {
                    let target = w.objects.by_watch_id(order.target);
                    set_main_goal(
                        w,
                        slot,
                        super::pilot::goal_action::ATTACK,
                        target,
                        None,
                        -1.0,
                    )
                }
                OrderCode::Capture => {
                    let target = w.objects.by_watch_id(order.target);
                    set_main_goal(
                        w,
                        slot,
                        super::pilot::goal_action::CAPTURE,
                        target,
                        None,
                        400.0,
                    )
                }
                _ => set_main_goal(w, slot, super::pilot::goal_action::NONE, None, None, -1.0),
            }
        }
    }
}

/// `MechWarrior::radioMessage(message, propagate)`: a sound event for the pilot's mover (the radio log's rate limits are
/// presentation: `radio` is a sound object).
pub fn radio_message(w: &mut World, slot: u16, message: i32) {
    if message < 0 {
        return;
    }
    if let Some(h) = vehicle(w, slot) {
        if under_home_command(w, slot) && w.clock.turn > 0 {
            crate::combat::fx::radio_message(w, h, message);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Order queue
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::clearTacOrderQueue()` (warrior.cpp:1763).
pub fn clear_tac_order_queue(w: &mut World, slot: u16) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.queue.clear();
        p.queue_index = 0;
        p.queue_executing = false;
        p.queue_looping = false;
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------------------------------------

/// The answer `getLastTarget` would give, without its clean-up of a stale target (`has_current_target` for read-only queries).
pub fn peek_current_target(w: &World, slot: u16) -> Option<ObjectHandle> {
    let p = w.pilots.get(slot)?;
    let t = w.objects.by_watch_id(p.last_target.0)?;
    let o = w.objects.get(t)?;
    let me = vehicle(w, slot);
    let stale = o.c.is_destroyed()
        || (o.c.is_disabled() && !p.last_target_flags[0])
        || (me.is_some_and(|m| crate::object::hooks::is_friendly(w, t, m))
            && !p.last_target_flags[1]);
    (!stale).then_some(t)
}

/// `MechWarrior::getLastTarget()` (warrior.cpp:2323): a destroyed target, a disabled one (unless obliterating) or a friendly one
/// (unless it was targeted as such) is dropped, with the combat order that pursued it.
pub fn get_last_target(w: &mut World, slot: u16) -> Option<ObjectHandle> {
    let (wid, obliterate, friendly_ok, conserve) = {
        let p = w.pilots.get(slot)?;
        (
            p.last_target.0,
            p.last_target_flags[0],
            p.last_target_flags[1],
            p.last_target_flags[2],
        )
    };
    let target = w.objects.by_watch_id(wid);
    if let Some(t) = target {
        let me = vehicle(w, slot);
        let clear = {
            let o = w.objects.get(t)?;
            o.c.is_destroyed()
                || (o.c.is_disabled() && !obliterate)
                || (me.is_some_and(|m| crate::object::hooks::is_friendly(w, t, m)) && !friendly_ok)
        };
        if clear {
            set_last_target(w, slot, None, false, false);
            if let Some(p) = w.pilots.get_mut(slot) {
                p.last_target.1 = -1.0;
                p.last_target_flags = [false; 3];
            }
            if w.pilots
                .get(slot)
                .is_some_and(|p| p.cur_order.is_combat_order())
            {
                clear_cur_tac_order(w, slot, true);
            }
            return None;
        }
    }
    if conserve {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.cur_order.attack_params.attack_type = 3; // ATTACK_CONSERVING_AMMO
        }
    }
    target
}

/// `MechWarrior::getCurrentTarget()` = `getLastTarget()` (warrior.h).
pub fn get_current_target(w: &mut World, slot: u16) -> Option<ObjectHandle> {
    get_last_target(w, slot)
}

/// `MechWarrior::setLastTarget(target, obliterate, conserveAmmo)` (warrior.cpp:2350). For the home commander the old target loses an
/// attacker and the new one gains one (`GameObject::numAttackers`, gameobj.h:1070-1080).
pub fn set_last_target(
    w: &mut World,
    slot: u16,
    target: Option<ObjectHandle>,
    obliterate: bool,
    conserve_ammo: bool,
) {
    let t = now(w);
    let me = vehicle(w, slot);
    if commander_is_home(w, slot) {
        let old = w.pilots.get(slot).map_or(WatchId(0), |p| p.last_target.0);
        if let Some(h) = w.objects.by_watch_id(old) {
            if let Some(o) = w.objects.get_mut(h) {
                o.c.decrement_attackers();
            }
        }
        if let Some(h) = target {
            if let Some(o) = w.objects.get_mut(h) {
                o.c.increment_attackers();
            }
        }
    }
    let (wid, friendly) = match target {
        Some(h) => {
            let friendly = me.is_some_and(|m| crate::object::hooks::is_friendly(w, h, m));
            (w.objects.watch_id(h), friendly)
        }
        None => (WatchId(0), false),
    };
    if let Some(p) = w.pilots.get_mut(slot) {
        p.last_target = (wid, t);
        p.last_target_flags = [obliterate, friendly, conserve_ammo];
    }
}

/// `MechWarrior::updateAttackerStatus(attackerWID, time)` (warrior.cpp:2221): add a watch ID once,
/// refresh its timestamp on later hits, and stop accepting entries at `MAX_ATTACKERS`.
pub fn update_attacker_status(w: &mut World, slot: u16, attacker: WatchId, time: f32) {
    const MAX_ATTACKERS: usize = 50;
    let Some(pilot) = w.pilots.get_mut(slot) else {
        return;
    };
    if let Some((_, last_time)) = pilot.attackers.iter_mut().find(|(wid, _)| *wid == attacker) {
        *last_time = time;
    } else if pilot.attackers.len() < MAX_ATTACKERS {
        pilot.attackers.push((attacker, time));
    }
}

/// `MechWarrior::setAttackTarget(object)` (warrior.cpp:2310).
pub fn set_attack_target(w: &mut World, slot: u16, target: Option<ObjectHandle>) {
    let t = now(w);
    let wid = target.map_or(WatchId(0), |h| w.objects.watch_id(h));
    if let Some(p) = w.pilots.get_mut(slot) {
        p.attack_orders.target = wid;
        p.attack_orders.target_time = t;
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Move orders: goals, requests, paths
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::setMoveGoal(type, location, obj)` (warrior.cpp:2437). A location whose `z` is below -10 is given the terrain
/// elevation first (`location->z` is written back into the caller's vector: the returned location is the final one).
pub fn set_move_goal(
    w: &mut World,
    slot: u16,
    goal_type: i64,
    location: Option<Vec3>,
) -> Option<Vec3> {
    if goal_type == MOVEGOAL_NONE {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.lane_b_move.goal_type = MOVEGOAL_NONE;
            p.lane_b_move.clear_goal();
        }
        return location;
    }
    let mut loc = location.unwrap_or_default();
    if loc.z < -10.0 {
        loc.z = w.terrain.elevation(&loc);
    }
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.set_goal_fields(goal_type, loc);
    }
    Some(loc)
}

/// `MechWarrior::getMoveGoal(&location, &obj)`: the goal type, its location and the goal object (if it still exists).
pub fn get_move_goal(w: &World, slot: u16) -> (i64, Vec3, Option<ObjectHandle>) {
    let Some(p) = w.pilots.get(slot) else {
        return (MOVEGOAL_NONE, Vec3::default(), None);
    };
    let m = &p.lane_b_move;
    (
        m.goal_type,
        m.goal,
        w.objects.by_watch_id(m.goal_object_wid),
    )
}

/// `MechWarrior::requestMovePath(selectionIndex, moveParams, source)` -> `PathManager->request`.
pub fn request_move_path(w: &mut World, slot: u16, selection_index: i32, params: u32, source: i32) {
    if w.pilots.get(slot).is_some() {
        w.paths.request(crate::mover::manager::PathRequest {
            pilot: slot,
            selector: selection_index,
            params,
            source,
        });
    }
}

/// `getMovePathRequest()`: the pilot is waiting in the path manager's queue.
pub fn has_path_request(w: &World, slot: u16) -> bool {
    w.paths.queue.iter().any(|r| r.pilot == slot)
}

/// `MechWarrior::pausePath()` (warrior.cpp:2499).
pub fn pause_path(w: &mut World, slot: u16) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.paths[0].num_steps = 0;
    }
}

/// `MechWarrior::resumePath()` (warrior.cpp:2507).
pub fn resume_path(w: &mut World, slot: u16) {
    if let Some(p) = w.pilots.get_mut(slot) {
        let m = &mut p.lane_b_move.paths[0];
        m.num_steps = m.num_steps_when_not_paused;
    }
}

/// `MechWarrior::getMovePath()` (warrior.cpp:2607-2688): when the current path has no steps left (`numStepsWhenNotPaused == 0`)
/// it is cleared and swapped with the spare; a spare that holds a path becomes current, with these side effects: no goal -> the new
/// path is emptied; a goal object that no longer exists -> emptied; yielding -> `yieldTime = now + 1.5` and the path paused;
/// waiting for a point -> paused; otherwise yield state cleared; the move goal is rewritten from the path's goal; and a path whose
/// `globalStep` is the last global step also rewrites `globalGoalLocation` and the current order's first way point.
pub fn get_move_path_index(w: &mut World, slot: u16) -> Option<usize> {
    let t = now(w);
    let p = w.pilots.get_mut(slot)?;
    if p.lane_b_move.paths[0].num_steps_when_not_paused != 0 {
        return Some(0);
    }
    clear_move_path(&mut p.lane_b_move.paths[0]);
    p.lane_b_move.paths.swap(0, 1);
    if p.lane_b_move.paths[0].num_steps_when_not_paused > 0 {
        let (goal_type, goal_loc, goal_obj) = get_move_goal(w, slot);
        if goal_type == MOVEGOAL_NONE {
            // No more goal, so no more path.
            let p = w.pilots.get_mut(slot)?;
            p.lane_b_move.paths[0].num_steps_when_not_paused = 0;
            return Some(0);
        }
        let _ = goal_loc;
        let mut target = None;
        if goal_type > 0 {
            // Target is an object (`ObjectManager->get(goalType)` indexes by HANDLE, a C++ quirk kept for a goal object that died)
            target = goal_obj.or_else(|| {
                usize::try_from(goal_type)
                    .ok()
                    .filter(|&h| h >= 1 && h <= w.objects.max_objects())
                    .and_then(|h| {
                        w.objects
                            .get(ObjectHandle(h as u32))
                            .map(|_| ObjectHandle(h as u32))
                    })
            });
            match target {
                Some(h) => {
                    let pos = w.objects.get(h)?.c.position;
                    w.pilots.get_mut(slot)?.lane_b_move.paths[0].target = pos;
                }
                None => {
                    // If the object no longer exists, we'll stay put...
                    w.pilots.get_mut(slot)?.lane_b_move.paths[0].num_steps_when_not_paused = 0;
                    return Some(0);
                }
            }
        }
        let p = w.pilots.get_mut(slot)?;
        let m = &mut p.lane_b_move;
        if m.is_yielding {
            m.yield_time = t + 1.5;
            m.is_yielding = f64::from(m.yield_time) > -1.0;
            m.paths[0].num_steps = 0;
        } else if m.is_waiting_for_point() {
            m.paths[0].num_steps = 0;
        } else {
            m.yield_time = -1.0;
            m.is_yielding = false;
            m.yield_state = 0;
        }
        let path_goal = m.paths[0].goal;
        let new_type = match target {
            Some(h) => i64::from(w.objects.watch_id(h).0),
            None => MOVEGOAL_LOCATION,
        };
        set_move_goal(w, slot, new_type, Some(path_goal));
        let p = w.pilots.get_mut(slot)?;
        let m = &mut p.lane_b_move;
        if m.paths[0].global_step == m.num_global_steps - 1 {
            let last = m.paths[0].num_steps_when_not_paused.saturating_sub(1);
            if let Some(step) = m.paths[0].steps.get(last) {
                m.global_goal_location = step.destination;
            }
            let loc = m.global_goal_location;
            if let Some(point) = p.cur_order.move_params.way_path.points.first_mut() {
                *point = loc;
            }
        }
    }
    Some(0)
}

/// `MechWarrior::getMovePath()` as an owned snapshot of the current path (the seam [`crate::ai::get_move_path`] exposes to lane B).
pub fn get_move_path(w: &mut World, slot: u16) -> Option<mc2_path::MovePath> {
    let i = get_move_path_index(w, slot)?;
    w.pilots.get(slot).map(|p| p.lane_b_move.paths[i].clone())
}

/// `MechWarrior::rethinkPath(strategy)` (warrior.cpp:2515-2575).
pub fn rethink_path(w: &mut World, slot: u16, _strategy: u32) {
    let have_way_point = get_next_way_point(w, slot, false).is_some();
    let Some(p) = w.pilots.get(slot) else { return };
    let path_type = p.lane_b_move.path_type;
    let selection = p.cur_order.selection_index;
    let jump = p.cur_order.move_params.jump;
    let mut params = mc2_path::movemap::MOVEPARAM_RECALC
        | mc2_path::movemap::MOVEPARAM_AVOID_PATHLOCKS
        | mc2_path::movemap::MOVEPARAM_FACE_TARGET;
    if jump {
        params |= mc2_path::movemap::MOVEPARAM_JUMP;
    }
    if path_type == mo::MOVEPATH_SIMPLE {
        if !have_way_point {
            // Since we're close to the end, just stop here...
            clear_move_orders(w, slot);
            clear_current_move_order_if_move(w, slot);
            trigger_alarm(w, slot, alarm::NO_MOVEPATH, mo::err::EARLY_STOP);
        } else {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.lane_b_move.paths[0].num_steps = 0;
            }
            request_move_path(w, slot, selection, params, 2);
        }
    } else if path_type == mo::MOVEPATH_COMPLEX {
        let on_last = p.lane_b_move.paths[0].global_step == p.lane_b_move.num_global_steps - 1;
        if on_last {
            if !have_way_point {
                clear_move_orders(w, slot);
                clear_current_move_order_if_move(w, slot);
                trigger_alarm(w, slot, alarm::NO_MOVEPATH, mo::err::EARLY_STOP);
            }
        } else {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.lane_b_move.paths[0].num_steps = 0;
            }
            request_move_path(w, slot, selection, params, 3);
        }
    }
}

/// `if (curTacOrder.isMoveOrder() || curTacOrder.isWayPathOrder()) clearCurTacOrder();`
fn clear_current_move_order_if_move(w: &mut World, slot: u16) {
    let is_move = w
        .pilots
        .get(slot)
        .is_some_and(|p| is_move_order(&p.cur_order) || is_way_path_order(&p.cur_order));
    if is_move {
        clear_cur_tac_order(w, slot, true);
    }
}

/// `TacticalOrder::isMoveOrder()` (tacordr.cpp:313): move-to-point/object, jump-to-point/object.
pub fn is_move_order(order: &TacticalOrder) -> bool {
    matches!(
        order.code,
        OrderCode::MoveToPoint
            | OrderCode::MoveToObject
            | OrderCode::JumpToPoint
            | OrderCode::JumpToObject
    )
}

/// `TacticalOrder::isWayPathOrder()` (tacordr.cpp:327).
pub fn is_way_path_order(order: &TacticalOrder) -> bool {
    matches!(order.code, OrderCode::TraversePath | OrderCode::PatrolPath)
}

/// `MechWarrior::getNextWayPoint(nextPoint, incWayPoint)` (warrior.cpp:3303-3375): the next point of a traverse/patrol way-path
/// order (`None` for every other order, and at the end of a one-time path). With `inc` the way point index, direction, run flag and the
/// order's `curPoint` advance (and the order's stage becomes 2 at the end of a one-time path). The compiled game never creates a
/// way-path order (`TacticalOrder::execute` has those cases commented out), so only the `None` answer is reachable in a run.
pub fn get_next_way_point(w: &mut World, slot: u16, inc: bool) -> Option<Vec3> {
    let p = w.pilots.get_mut(slot)?;
    if !is_way_path_order(&p.cur_order) {
        return None;
    }
    let mode_fast = |p: &Pilot, i: usize| {
        p.cur_order.move_params.way_path.mode.get(i) == Some(&crate::command::TravelMode::Fast)
    };
    let num = p.lane_b_move.way_path.len() as i32;
    if p.lane_b_move.cur_way_dir == 0 {
        // This is not a patrol path, so stop at the end...
        let next = p.lane_b_move.cur_way_pt + 1;
        let point = (next < num).then(|| p.lane_b_move.way_path[next as usize]);
        if inc {
            p.lane_b_move.cur_way_pt = next;
            p.lane_b_move.run = mode_fast(p, next.max(0) as usize);
            p.cur_order.move_params.way_path.cur_point = next.max(0) as usize;
            if next == num {
                p.cur_order.stage = 2;
            }
        }
        if next == num {
            return None;
        }
        point
    } else {
        let mut dir = p.lane_b_move.cur_way_dir;
        let next;
        if p.lane_b_move.cur_way_dir == 1 {
            if p.lane_b_move.cur_way_pt == num - 1 {
                // Reached end, so turn around and start backtracking...
                dir = -1;
                next = num - 2;
            } else {
                next = p.lane_b_move.cur_way_pt + 1;
            }
        } else if p.lane_b_move.cur_way_pt == 0 {
            dir = 1;
            next = 1;
        } else {
            next = p.lane_b_move.cur_way_pt - 1;
        }
        let point = p.lane_b_move.way_path.get(next.max(0) as usize).copied();
        if inc {
            p.lane_b_move.cur_way_dir = dir;
            p.lane_b_move.cur_way_pt = next;
            p.lane_b_move.run = mode_fast(p, next.max(0) as usize);
            p.cur_order.move_params.way_path.cur_point = next.max(0) as usize;
        }
        point.or(Some(Vec3::default()))
    }
}

/// `Pilot` accessor used by tests and the digest: the three order slots in `ORDERSTATE_*` order.
pub fn order_slots(p: &Pilot) -> [&TacticalOrder; 3] {
    [&p.tac_orders[0], &p.tac_orders[1], &p.tac_orders[2]]
}

/// `OrderOrigin` of a slot (helper for the digest).
pub fn slot_origin(p: &Pilot, i: usize) -> OrderOrigin {
    p.tac_orders[i].origin
}

// ---------------------------------------------------------------------------------------------------------------------
// Alarm history (the brain's pilot events)
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::getEventHistory(alarmCode, paramList)` (warrior.cpp:4572): the part ids (or raw ids) of the triggers in the alarm's
/// history, for the alarms whose triggers are object watch ids only the ones that still exist.
pub fn get_event_history(w: &World, slot: u16, code: usize) -> Vec<i32> {
    let Some(p) = w.pilots.get(slot) else {
        return Vec::new();
    };
    let history = &p.alarm_history[code];
    let part = |wid: i32| {
        w.objects
            .by_watch_id(WatchId(wid as u32))
            .and_then(|h| w.objects.get(h))
            .map(|o| o.c.part_id.0)
    };
    match code {
        alarm::ATTACK_ORDER
        | alarm::DEATH_OF_MATE
        | alarm::KILLED_TARGET
        | alarm::FRIENDLY_VEHICLE_CRIPPLED
        | alarm::VEHICLE_DESTROYED
        | alarm::MATE_FIRED_WEAPON
        | alarm::COLLISION
        | alarm::TARGET_OF_WEAPONFIRE
        | alarm::FIRED_WEAPON => history.iter().filter_map(|&t| part(t)).collect(),
        alarm::HIT_BY_WEAPONFIRE => history
            .iter()
            .filter_map(|&t| if t < 0 { Some(t) } else { part(t) })
            .collect(),
        _ => history.clone(),
    }
}

/// `MechWarrior::getNextEventHistory(paramList)` (warrior.cpp:4620): the next alarm with history, advancing the shared
/// `curEventID`; the event id, or -1 when there is none.
pub fn get_next_event_history(w: &mut World, slot: u16) -> (i32, Vec<i32>) {
    let n = super::pilot::NUM_ALARMS as i32;
    if w.pilots.cur_event_id < n {
        while w.pilots.cur_event_id < n
            && w.pilots
                .get(slot)
                .is_none_or(|p| p.alarm_history[w.pilots.cur_event_id as usize].is_empty())
        {
            w.pilots.cur_event_id += 1;
        }
        if w.pilots.cur_event_id < n {
            let list = get_event_history(w, slot, w.pilots.cur_event_id as usize);
            w.pilots.cur_event_id += 1;
            return (w.pilots.cur_event_id - 1, list);
        }
    }
    (-1, Vec::new())
}

/// `MechWarrior::setTeam(team)` (warrior.cpp:2046): a pilot not on the home team plans toward goals and keeps moving
/// (`useGoalPlan`, `keepMoving`); the home team's pilots do neither. (`teamId` itself is derived from the vehicle here.)
pub fn set_team(w: &mut World, slot: u16, team: Option<usize>) {
    let home = w.teams.home;
    if let Some(p) = w.pilots.get_mut(slot) {
        let not_home = team != Some(home);
        p.use_goal_plan = not_home;
        p.keep_moving = not_home;
    }
}
