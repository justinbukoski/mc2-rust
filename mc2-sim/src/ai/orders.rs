// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MechWarrior::order*` (warrior.cpp:6600-7060) and `MechWarrior::core*` (warrior.cpp:5225-5700): the pilot's order constructors.
//! Each builds a [`TacticalOrder`] exactly like the C++ (including the fields it forgets to set), tests `status`, resets the move and
//! attack orders, asks the path manager for a path and hands the order to one of the three order slots.

use super::mover_ops;
use super::pilot::goal_action;
use super::tacorder;
use super::warrior::{
    self, clear_attack_orders, clear_cur_tac_order, clear_move_orders, get_last_target,
    request_move_path, set_alarm_tac_order, set_attack_target, set_general_tac_order,
    set_last_target, set_main_goal, set_move_goal, vehicle, TACORDER_FAILURE, TACORDER_SUCCESS,
};
use crate::command::{
    AttackMethod, OrderCode, OrderOrigin, Tactic, TacticalOrder, TravelMode, FIRERANGE_LONG,
    FIRERANGE_LONGEST, FIRERANGE_MEDIUM, FIRERANGE_OPTIMAL, FIRERANGE_RAMMING, FIRERANGE_SHORT,
};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::World;
use mc2_path::movemap::{
    MOVEPARAM_ESCAPE_TILE, MOVEPARAM_FACE_TARGET, MOVEPARAM_INIT, MOVEPARAM_JUMP,
    MOVEPARAM_RADIO_RESULT, MOVEPARAM_STEP_ADJACENT_TARGET,
};

/// `TACORDER_PARAM_*` (mclib/move.h:139-167).
pub mod param {
    /// `TACORDER_PARAM_NONE`.
    pub const NONE: u32 = 0;
    /// `TACORDER_PARAM_RUN`.
    pub const RUN: u32 = 1 << 0;
    /// `TACORDER_PARAM_WAIT`.
    pub const WAIT: u32 = 1 << 1;
    /// `TACORDER_PARAM_FACE_OBJECT`.
    pub const FACE_OBJECT: u32 = 1 << 2;
    /// `TACORDER_PARAM_LAY_MINES`.
    pub const LAY_MINES: u32 = 1 << 3;
    /// `TACORDER_PARAM_PURSUE`.
    pub const PURSUE: u32 = 1 << 4;
    /// `TACORDER_PARAM_OBLITERATE`.
    pub const OBLITERATE: u32 = 1 << 5;
    /// `TACORDER_PARAM_ESCAPE_TILE`.
    pub const ESCAPE_TILE: u32 = 1 << 6;
    /// `TACORDER_PARAM_SCAN`.
    pub const SCAN: u32 = 1 << 7;
    /// `TACORDER_PARAM_ATTACK_DFA`.
    pub const ATTACK_DFA: u32 = 1 << 8;
    /// `TACORDER_PARAM_ATTACK_RAMMING`.
    pub const ATTACK_RAMMING: u32 = 1 << 9;
    /// `TACORDER_PARAM_RANGE_RAMMING`.
    pub const RANGE_RAMMING: u32 = 1 << 10;
    /// `TACORDER_PARAM_RANGE_LONGEST`.
    pub const RANGE_LONGEST: u32 = 1 << 11;
    /// `TACORDER_PARAM_RANGE_OPTIMAL`.
    pub const RANGE_OPTIMAL: u32 = 1 << 12;
    /// `TACORDER_PARAM_RANGE_SHORT`.
    pub const RANGE_SHORT: u32 = 1 << 13;
    /// `TACORDER_PARAM_RANGE_MEDIUM`.
    pub const RANGE_MEDIUM: u32 = 1 << 14;
    /// `TACORDER_PARAM_RANGE_LONG`.
    pub const RANGE_LONG: u32 = 1 << 15;
    /// `TACORDER_PARAM_AIM_HEAD`.
    pub const AIM_HEAD: u32 = 1 << 16;
    /// `TACORDER_PARAM_AIM_LEG`.
    pub const AIM_LEG: u32 = 1 << 17;
    /// `TACORDER_PARAM_AIM_ARM`.
    pub const AIM_ARM: u32 = 1 << 18;
    /// `TACORDER_PARAM_DONT_SET_ORDER`.
    pub const DONT_SET_ORDER: u32 = 1 << 19;
    /// `TACORDER_PARAM_JUMP`.
    pub const JUMP: u32 = 1 << 20;
    /// `TACORDER_PARAM_DONT_KEEP_MOVING`.
    pub const DONT_KEEP_MOVING: u32 = 1 << 21;
    /// `TACORDER_PARAM_TACTIC_FLANK_RIGHT`.
    pub const TACTIC_FLANK_RIGHT: u32 = 1 << 22;
    /// `TACORDER_PARAM_TACTIC_FLANK_LEFT`.
    pub const TACTIC_FLANK_LEFT: u32 = 1 << 23;
    /// `TACORDER_PARAM_TACTIC_FLANK_REAR`.
    pub const TACTIC_FLANK_REAR: u32 = 1 << 24;
    /// `TACORDER_PARAM_TACTIC_STOP_AND_FIRE`.
    pub const TACTIC_STOP_AND_FIRE: u32 = 1 << 25;
    /// `TACORDER_PARAM_TACTIC_TURRET`.
    pub const TACTIC_TURRET: u32 = 1 << 26;
    /// `TACORDER_PARAM_TACTIC_JOUST`.
    pub const TACTIC_JOUST: u32 = 1 << 27;
    /// `TACORDER_ATTACK_MASK`.
    pub const ATTACK_MASK: u32 = ATTACK_DFA | ATTACK_RAMMING;
    /// `TACORDER_RANGE_MASK`.
    pub const RANGE_MASK: u32 =
        RANGE_RAMMING | RANGE_LONGEST | RANGE_OPTIMAL | RANGE_SHORT | RANGE_MEDIUM | RANGE_LONG;
    /// `TACORDER_AIM_MASK`.
    pub const AIM_MASK: u32 = AIM_HEAD | AIM_LEG | AIM_ARM;
    /// `TACORDER_TACTIC_MASK`.
    pub const TACTIC_MASK: u32 = TACTIC_FLANK_RIGHT
        | TACTIC_FLANK_LEFT
        | TACTIC_FLANK_REAR
        | TACTIC_STOP_AND_FIRE
        | TACTIC_TURRET
        | TACTIC_JOUST;
}

/// `ATTACK_TO_DESTROY` ... (tacordr.h `AttackType`).
pub const ATTACK_NONE: i32 = 0;
/// `ATTACK_TO_DESTROY`.
pub const ATTACK_TO_DESTROY: i32 = 1;
/// `ATTACK_CONSERVING_AMMO`.
pub const ATTACK_CONSERVING_AMMO: i32 = 3;

/// `MECH_BODY_LOCATION_HEAD`, `RARM`, `RLEG` (mech.h:149).
const BODY_HEAD: i32 = 0;
const BODY_RARM: i32 = 5;
const BODY_RLEG: i32 = 7;

/// `RADIO_*` ids used by the orders (dradio.h; `MSG_TOTAL_MSGS` = 0).
pub mod radio {
    /// `RADIO_ACK`.
    pub const ACK: i32 = 0;
    /// `RADIO_CANNOT_CAPTURE`.
    pub const CANNOT_CAPTURE: i32 = 2;
    /// `RADIO_CAPTURED_BUILDING`.
    pub const CAPTURED_BUILDING: i32 = 3;
    /// `RADIO_CAPTURED_VEHICLE`.
    pub const CAPTURED_VEHICLE: i32 = 4;
    /// `RADIO_MOVE_BLOCKED`.
    pub const MOVE_BLOCKED: i32 = 16;
    /// `RADIO_ILLEGAL_ORDER`.
    pub const ILLEGAL_ORDER: i32 = 17;
    /// `RADIO_AMMO_OUT`.
    pub const AMMO_OUT: i32 = 24;
    /// `RADIO_TAUNT`.
    pub const TAUNT: i32 = 25;
    /// `RADIO_GUARD`.
    pub const GUARD: i32 = 26;
    /// `RADIO_TARGET_ACK`.
    pub const TARGET_ACK: i32 = 27;
}

fn way_point_zero(order: &mut TacticalOrder, p: Vec3) {
    if order.move_params.way_path.points.is_empty() {
        order.move_params.way_path.points.push(p);
    } else {
        order.move_params.way_path.points[0] = p;
    }
}

fn mode_zero(order: &mut TacticalOrder, run: bool) {
    let mode = if run {
        TravelMode::Fast
    } else {
        TravelMode::Slow
    };
    if order.move_params.way_path.mode.is_empty() {
        order.move_params.way_path.mode.push(mode);
    } else {
        order.move_params.way_path.mode[0] = mode;
    }
}

fn new_order(w: &World, origin: OrderOrigin, code: OrderCode, unit_order: bool) -> TacticalOrder {
    TacticalOrder::new(origin, code, unit_order, w.clock.scenario_time)
}

// ---------------------------------------------------------------------------------------------------------------------
// order*
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::orderWait` (warrior.cpp:6125).
pub fn order_wait(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    origin: OrderOrigin,
    seconds: i32,
    clear_last_target: bool,
) -> i32 {
    let mut order = new_order(w, origin, OrderCode::Wait, unit_order);
    order.delayed_time = w.clock.scenario_time + seconds as f32;
    clear_move_orders(w, slot);
    clear_attack_orders(w, slot);
    if clear_last_target {
        set_last_target(w, slot, None, false, false);
    }
    if origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order.clone());
    }
    tacorder::status(w, slot, &mut order)
}

/// `MechWarrior::orderStop` (warrior.cpp:6165).
pub fn order_stop(w: &mut World, slot: u16, unit_order: bool, _set_tac_order: bool) -> i32 {
    let mut order = new_order(w, OrderOrigin::Player, OrderCode::Stop, unit_order);
    warrior::clear_tac_order_queue(w, slot);
    clear_move_orders(w, slot);
    clear_attack_orders(w, slot);
    set_last_target(w, slot, None, false, false);
    if let Some(core) = warrior::core_mut(w, slot) {
        core.suppression_fire = 0;
    }
    tacorder::status(w, slot, &mut order)
}

/// `MechWarrior::orderMoveToPoint` (warrior.cpp:6212).
#[allow(clippy::too_many_arguments)]
pub fn order_move_to_point(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    set_tac_order: bool,
    origin: OrderOrigin,
    location: Vec3,
    selection_index: i32,
    params: u32,
) -> i32 {
    let jump = params & param::JUMP != 0;
    let run = params & param::RUN != 0;
    let wait = params & param::WAIT != 0;
    let lay_mines = params & param::LAY_MINES != 0;
    let escape = params & param::ESCAPE_TILE != 0;

    let mut order = new_order(w, origin, OrderCode::MoveToPoint, unit_order);
    way_point_zero(&mut order, location);
    order.selection_index = selection_index;
    mode_zero(&mut order, run);
    order.move_params.wait = wait;
    order.move_params.mine_laying = lay_mines;
    order.move_params.escape_tile = escape;
    order.move_params.jump = jump;

    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }

    set_move_goal(w, slot, super::moveorders::MOVEGOAL_NONE, None);
    if let Some(p) = w.pilots.get_mut(slot) {
        // setMoveWayPath(NULL, 0)
        p.lane_b_move.way_path.clear();
        p.lane_b_move.cur_way_pt = 0;
        p.lane_b_move.cur_way_dir = 0;
    }
    set_move_goal(
        w,
        slot,
        super::moveorders::MOVEGOAL_LOCATION,
        Some(location),
    );
    let t = w.clock.scenario_time;
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.time_of_last_step = t;
        p.lane_b_move.run = run;
    }
    // Move order has no attack target...
    if set_tac_order {
        clear_attack_orders(w, slot);
    }
    let mut move_params = MOVEPARAM_INIT | MOVEPARAM_FACE_TARGET;
    if escape {
        move_params |= MOVEPARAM_ESCAPE_TILE;
    }
    if jump {
        move_params |= MOVEPARAM_JUMP;
    }
    // `getPoint()` is NULL (USE_GROUPS is not defined): `!unitOrder || (NULL == getVehicle())`
    if set_tac_order && origin == OrderOrigin::Player && !unit_order {
        move_params |= MOVEPARAM_RADIO_RESULT;
    }
    request_move_path(
        w,
        slot,
        selection_index,
        move_params,
        if set_tac_order { 21 } else { 23 },
    );
    if set_tac_order && result == TACORDER_FAILURE && origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    result
}

/// `MechWarrior::orderMoveToObject` (warrior.cpp:6286).
#[allow(clippy::too_many_arguments)]
pub fn order_move_to_object(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    set_tac_order: bool,
    origin: OrderOrigin,
    target: Option<ObjectHandle>,
    from_area: i32,
    selection_index: i32,
    params: u32,
) -> i32 {
    let jump = params & param::JUMP != 0;
    let run = params & param::RUN != 0;
    let face_object = params & param::FACE_OBJECT != 0;
    let lay_mines = params & param::LAY_MINES != 0;
    let Some(target) = target else {
        return TACORDER_SUCCESS;
    };
    let Some(me) = vehicle(w, slot) else {
        return TACORDER_SUCCESS;
    };
    // Pick the best move goal based upon the object...
    let (is_mover, is_building, position) = match w.objects.get(target) {
        Some(o) => (
            o.is_mover(),
            matches!(o.kind, crate::object::Kind::Building(_)),
            o.c.position,
        ),
        None => return TACORDER_SUCCESS,
    };
    let goal = if is_mover {
        position
    } else if is_building {
        super::goal::calc_adjacent_area_cell(w, target, me, from_area)
            .unwrap_or_else(|| mover_ops::calc_offset_move_goal(w, me, &position))
    } else {
        mover_ops::calc_offset_move_goal(w, me, &position)
    };
    let wid = w.objects.watch_id(target);
    let mut order = new_order(w, origin, OrderCode::MoveToObject, unit_order);
    order.target = wid;
    order.selection_index = selection_index;
    order.move_params.from_area = from_area;
    way_point_zero(&mut order, goal);
    mode_zero(&mut order, run);
    order.move_params.face_object = face_object;
    order.move_params.wait = false;
    order.move_params.mine_laying = lay_mines;
    order.move_params.jump = jump;

    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    order.target = w.objects.watch_id(target);
    let pos = w.objects.get(target).map_or(position, |o| o.c.position);
    set_move_goal(w, slot, i64::from(wid.0), Some(pos));
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.run = run;
    }
    if set_tac_order {
        clear_attack_orders(w, slot);
    }
    let mut move_params = MOVEPARAM_INIT;
    if jump {
        move_params |= MOVEPARAM_JUMP;
    }
    if face_object {
        move_params |= MOVEPARAM_FACE_TARGET;
    }
    if set_tac_order && !is_mover {
        move_params |= MOVEPARAM_STEP_ADJACENT_TARGET;
    }
    if set_tac_order && origin == OrderOrigin::Player && !unit_order {
        move_params |= MOVEPARAM_RADIO_RESULT;
    }
    request_move_path(w, slot, selection_index, move_params, 16);
    let obj_pos = w.objects.get(target).map_or(position, |o| o.c.position);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.goal_object_position = obj_pos;
    }
    if set_tac_order && result == TACORDER_FAILURE && origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    result
}

/// `getMapPassable` for a world position (`GameMap->getPassable(cellR, cellC)` after `land->worldToCell`).
fn passable_at(w: &World, p: &Vec3) -> bool {
    w.terrain.passable_at(p)
}

/// `MechWarrior::orderJumpToPoint` (warrior.cpp:6575). Note the C++ never starts a jump here: after the checks it only clears the
/// move and attack orders and, for a commander failure, sets the general order; it returns `TACORDER_SUCCESS` regardless.
pub fn order_jump_to_point(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    set_tac_order: bool,
    origin: OrderOrigin,
    location: Vec3,
    selection_index: i32,
) -> i32 {
    let Some(me) = vehicle(w, slot) else {
        return TACORDER_SUCCESS;
    };
    if mover_ops::jump_range(w, me) < mover_ops::distance_from(w, me, &location) {
        return TACORDER_SUCCESS;
    }
    let is_mech = matches!(
        w.objects.get(me).map(|o| o.c.class),
        Some(crate::object::ObjectClass::BattleMech)
    );
    if is_mech && !passable_at(w, &location) {
        return TACORDER_SUCCESS;
    }
    let mut order = new_order(w, origin, OrderCode::JumpToPoint, unit_order);
    order.selection_index = selection_index;
    way_point_zero(&mut order, location);
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    // Move order has no attack target...
    if set_tac_order {
        clear_move_orders(w, slot);
        clear_attack_orders(w, slot);
    }
    if set_tac_order && result == TACORDER_FAILURE && origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    TACORDER_SUCCESS
}

/// `MechWarrior::orderJumpToObject` (warrior.cpp:6620): builds a `JUMPTO_POINT` order for the target's position.
pub fn order_jump_to_object(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    set_tac_order: bool,
    origin: OrderOrigin,
    target: ObjectHandle,
    selection_index: i32,
) -> i32 {
    let Some(me) = vehicle(w, slot) else {
        return TACORDER_SUCCESS;
    };
    let Some(t) = w.objects.get(target) else {
        return TACORDER_SUCCESS;
    };
    let location = t.c.position;
    if t.is_mover() && t.team_id() == w.objects.get(me).map_or(-1, |o| o.team_id()) {
        return TACORDER_SUCCESS;
    }
    if mover_ops::jump_range(w, me) < mover_ops::distance_from(w, me, &location) {
        return TACORDER_SUCCESS;
    }
    let is_mech = matches!(
        w.objects.get(me).map(|o| o.c.class),
        Some(crate::object::ObjectClass::BattleMech)
    );
    if is_mech && !passable_at(w, &location) {
        return TACORDER_SUCCESS;
    }
    let mut order = new_order(w, origin, OrderCode::JumpToPoint, unit_order);
    order.selection_index = selection_index;
    way_point_zero(&mut order, location);
    order.target = w.objects.watch_id(target);
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    if set_tac_order {
        clear_move_orders(w, slot);
        clear_attack_orders(w, slot);
    }
    if set_tac_order && result == TACORDER_FAILURE && origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    TACORDER_SUCCESS
}

/// `MechWarrior::orderPowerUp` (warrior.cpp:6886).
pub fn order_power_up(w: &mut World, slot: u16, unit_order: bool, origin: OrderOrigin) -> i32 {
    let Some(me) = vehicle(w, slot) else {
        return TACORDER_SUCCESS;
    };
    if w.objects.get(me).map(|o| o.c.status) != Some(crate::object::ObjectStatus::ShutDown) {
        return TACORDER_SUCCESS;
    }
    let mut order = new_order(w, origin, OrderCode::PowerUp, unit_order);
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    // Order has no move goal... no attack target...
    clear_move_orders(w, slot);
    clear_attack_orders(w, slot);
    // Check if we've cooled enough to restart (no heat: BattleMech::canPowerUp is always true)...
    mover_ops::start_up(w, me);
    match origin {
        OrderOrigin::Commander => set_general_tac_order(w, slot, order),
        OrderOrigin::SelfOrder => set_alarm_tac_order(w, slot, order, 255),
        OrderOrigin::Player => {}
    }
    result
}

/// `MechWarrior::orderPowerDown` (warrior.cpp:6934).
pub fn order_power_down(w: &mut World, slot: u16, unit_order: bool, origin: OrderOrigin) -> i32 {
    let Some(me) = vehicle(w, slot) else {
        return TACORDER_SUCCESS;
    };
    let Some(obj) = w.objects.get(me) else {
        return TACORDER_SUCCESS;
    };
    use crate::object::ObjectStatus as S;
    if matches!(obj.c.status, S::ShutDown | S::ShuttingDown) || obj.c.is_disabled() {
        return TACORDER_SUCCESS;
    }
    let mut order = new_order(w, origin, OrderCode::PowerDown, unit_order);
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    clear_move_orders(w, slot);
    clear_attack_orders(w, slot);
    mover_ops::shut_down(w, me);
    if origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    result
}

/// `MechWarrior::orderAttackObject` (warrior.cpp:6706).
#[allow(clippy::too_many_arguments)]
pub fn order_attack_object(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    origin: OrderOrigin,
    target: Option<ObjectHandle>,
    attack_type: i32,
    method: AttackMethod,
    range: i32,
    aim_location: i32,
    from_area: i32,
    params: u32,
) -> i32 {
    let run = params & param::RUN != 0;
    let mut jump = params & param::JUMP != 0;
    let mut pursue = params & param::PURSUE != 0;
    let obliterate = params & param::OBLITERATE != 0;
    let conserve = attack_type == ATTACK_CONSERVING_AMMO;
    let mut keep_moving = params & param::DONT_KEEP_MOVING == 0;
    let mut range = range;

    let mut tactic = Tactic::None;
    match params & param::TACTIC_MASK {
        param::TACTIC_FLANK_RIGHT => tactic = Tactic::FlankRight,
        param::TACTIC_FLANK_LEFT => tactic = Tactic::FlankLeft,
        param::TACTIC_FLANK_REAR => tactic = Tactic::FlankRear,
        param::TACTIC_STOP_AND_FIRE => {
            tactic = Tactic::StopAndFire;
            pursue = true;
        }
        param::TACTIC_TURRET => {
            tactic = Tactic::Turret;
            jump = false;
            pursue = false;
            keep_moving = false;
        }
        param::TACTIC_JOUST => tactic = Tactic::Joust,
        _ => {}
    }

    let (attack_range, num_weapons) = match warrior::core(w, slot) {
        Some(c) => (
            c.attack_range,
            vehicle(w, slot)
                .and_then(|h| w.objects.get(h))
                .and_then(crate::combat::loadout_of)
                .map_or(0, |l| l.num_weapons()),
        ),
        None => return TACORDER_SUCCESS,
    };
    if attack_range == crate::command::FIRERANGE_CURRENT {
        range = crate::command::FIRERANGE_CURRENT;
        pursue = false;
        jump = false;
    }

    let Some(target) = target else {
        // Clear Attack orders...
        clear_attack_orders(w, slot);
        return TACORDER_SUCCESS;
    };
    if num_weapons == 0 && method != AttackMethod::Ramming {
        clear_attack_orders(w, slot);
        return TACORDER_SUCCESS;
    }

    let target_wid = w.objects.watch_id(target);
    let mut order = new_order(w, origin, OrderCode::AttackObject, unit_order);
    order.target = target_wid;
    order.attack_params.attack_type = attack_type;
    order.attack_params.method = method;
    order.attack_params.aim_location = aim_location;
    order.attack_params.tactic = tactic;
    if method == AttackMethod::Ramming {
        range = FIRERANGE_RAMMING;
    }
    order.attack_params.range = range;
    order.attack_params.pursue = pursue;
    order.attack_params.obliterate = obliterate;
    order.move_params.from_area = from_area;
    order.move_params.jump = jump;
    order.move_params.keep_moving = keep_moving;
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }

    set_situation_fire_range(w, slot, Some(range));

    // If we also want to pursue it, set up the move order here...
    if pursue {
        // Order moveToObject, making sure we face the object (the call itself is commented out in the source)...
        if let Some(p) = w.pilots.get_mut(slot) {
            p.lane_b_move.run = run;
        }
    } else {
        clear_move_orders(w, slot);
    }

    // Now, set up the tactical order, and pass it to the pilot...
    set_attack_orders(w, slot, attack_type, Some(target), aim_location, pursue);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.situation.open_fire = true;
    }

    // Remember the most recent target...
    set_last_target(w, slot, Some(target), obliterate, conserve);

    if origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    TACORDER_FAILURE
}

/// `setAttackType` / `setAttackTarget` / `setAttackAimLocation` / `setAttackPursuit` (warrior.h).
fn set_attack_orders(
    w: &mut World,
    slot: u16,
    attack_type: i32,
    target: Option<ObjectHandle>,
    aim_location: i32,
    pursue: bool,
) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.attack_orders.attack_type = attack_type;
    }
    set_attack_target(w, slot, target);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.attack_orders.aim_location = aim_location;
        p.attack_orders.pursue = pursue;
    }
}

/// `setSituationFireRange(getVehicle()->getOrderedFireRange(&range))` (warrior.cpp:6781): the situation's fire range in meters.
fn set_situation_fire_range(w: &mut World, slot: u16, range: Option<i32>) {
    let value = ordered_fire_range(w, slot, range);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.situation.fire_range = value;
    }
}

/// `Mover::getOrderedFireRange(&attackRange)` (mover.cpp:5919): the range the order asks for, in meters, -1 for an unlisted value
/// (`FIRERANGE_CURRENT`). `None` uses the current order's range.
pub fn ordered_fire_range(w: &World, slot: u16, attack_range: Option<i32>) -> f32 {
    let fire_range = attack_range.unwrap_or_else(|| {
        w.pilots
            .get(slot)
            .map_or(FIRERANGE_OPTIMAL, |p| p.cur_order.attack_params.range)
    });
    let optimal = vehicle(w, slot)
        .and_then(|h| w.objects.get(h))
        .and_then(crate::combat::loadout_of)
        .map_or(0.0, |l| l.stats.optimal_range);
    let max = vehicle(w, slot)
        .and_then(|h| w.objects.get(h))
        .and_then(crate::combat::loadout_of)
        .map_or(0.0, |l| l.stats.max_range);
    let ranges = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or([[0.0f32; 2]; 3], |t| {
            let r = &t.general.weapon_ranges;
            [r[0], r[1], r[2]]
        });
    let half = |i: usize| ((f64::from(ranges[i][1]) - f64::from(ranges[i][0])) / 2.0) as f32;
    match fire_range {
        crate::command::FIRERANGE_DEFAULT => optimal,
        FIRERANGE_RAMMING => 0.0,
        FIRERANGE_LONGEST => max,
        FIRERANGE_OPTIMAL => optimal,
        FIRERANGE_SHORT => half(0),
        FIRERANGE_MEDIUM => half(1),
        FIRERANGE_LONG => half(2),
        _ => -1.0,
    }
}

/// `MechWarrior::orderAttackPoint` (warrior.cpp:6828).
#[allow(clippy::too_many_arguments)]
pub fn order_attack_point(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    origin: OrderOrigin,
    location: Vec3,
    attack_type: i32,
    method: AttackMethod,
    range: i32,
    params: u32,
) -> i32 {
    let pursue = params & param::PURSUE != 0;
    let jump = params & param::JUMP != 0;
    let mut order = new_order(w, origin, OrderCode::AttackPoint, unit_order);
    order.attack_params.attack_type = attack_type;
    order.attack_params.method = method;
    order.attack_params.range = range;
    order.attack_params.pursue = pursue;
    order.attack_params.target_point = location;
    order.move_params.jump = jump;
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return TACORDER_SUCCESS;
    }
    set_situation_fire_range(w, slot, Some(range));
    // If we also want to pursue it, set up the move order here...
    if pursue {
        order_move_to_point(w, slot, unit_order, false, origin, location, -1, params);
    } else {
        clear_move_orders(w, slot);
    }
    set_attack_orders(w, slot, attack_type, None, -1, pursue);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.attack_orders.target_point = location;
        p.situation.open_fire = true;
    }
    set_last_target(w, slot, None, false, false);
    if origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    TACORDER_FAILURE
}

/// `MechWarrior::orderEject` (warrior.cpp:6927 area).
pub fn order_eject(w: &mut World, slot: u16, unit_order: bool, origin: OrderOrigin) -> i32 {
    let order = new_order(w, origin, OrderCode::Eject, unit_order);
    match vehicle(w, slot) {
        Some(me) if origin == OrderOrigin::Commander => {
            eject_then_set_order(w, slot, me, order, unit_order)
        }
        Some(me) => {
            crate::combat::damage::handle_ejection(w, me);
        }
        None if origin == OrderOrigin::Commander => set_general_tac_order(w, slot, order),
        None => {}
    }
    TACORDER_SUCCESS
}

/// `calcWithdrawGoal` is `getVehicle()->getPosition()` in the compiled game (`USE_TEAMS` is not defined, warrior.cpp:3860).
pub fn calc_withdraw_goal(w: &World, slot: u16) -> Vec3 {
    vehicle(w, slot)
        .and_then(|h| w.objects.get(h))
        .map_or_else(Vec3::default, |o| o.c.position)
}

/// `MechWarrior::orderWithdraw` (warrior.cpp:6865).
pub fn order_withdraw(
    w: &mut World,
    slot: u16,
    unit_order: bool,
    origin: OrderOrigin,
    location: Vec3,
) -> i32 {
    let mut order = new_order(w, origin, OrderCode::Withdraw, unit_order);
    way_point_zero(&mut order, location);
    let goal = calc_withdraw_goal(w, slot);
    let result = order_move_to_point(w, slot, unit_order, true, origin, goal, -1, param::RUN);
    if let Some(core) = warrior::core_mut(w, slot) {
        core.flags.withdrawing = true;
    }
    if origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    if let Some(p) = w.pilots.get_mut(slot) {
        p.cur_order.code = OrderCode::Withdraw;
    }
    result
}

/// `MechWarrior::orderGetFixed` (warrior.cpp:6677): sends the mover to a repair bay. The `USE_BUILDINGS` bay-kind check is not
/// compiled; `execGetFixed` has already checked the bay kind. Returns `NO_ERR` (0) or, for a bay with no refit points left, success.
pub fn order_get_fixed(
    w: &mut World,
    slot: u16,
    origin: OrderOrigin,
    target: ObjectHandle,
    params: u32,
) -> i32 {
    if crate::mover::refit::building_refit_points(w, target) <= 0.0 {
        return TACORDER_SUCCESS;
    }
    let run = params & param::RUN != 0;
    let mut order = new_order(w, origin, OrderCode::GetFixed, false);
    order.target = w.objects.watch_id(target);
    order.selection_index = -1;
    mode_zero(&mut order, run);
    order.move_params.face_object = true;
    order.move_params.wait = false;
    if origin == OrderOrigin::Commander {
        set_general_tac_order(w, slot, order);
    }
    0
}

// ---------------------------------------------------------------------------------------------------------------------
// core*
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::coreMoveTo` (warrior.cpp:5254).
pub fn core_move_to(w: &mut World, slot: u16, location: Vec3, params: u32) -> i32 {
    let run = params & param::RUN != 0;
    let jump = params & param::JUMP != 0;
    let wait = params & param::WAIT != 0;
    let lay_mines = params & param::LAY_MINES != 0;
    let escape = params & param::ESCAPE_TILE != 0;
    let set_order = params & param::DONT_SET_ORDER == 0;
    let keep_moving = params & param::DONT_KEEP_MOVING == 0;

    let mut order = new_order(w, OrderOrigin::Commander, OrderCode::MoveToPoint, false);
    way_point_zero(&mut order, location);
    order.selection_index = -1;
    mode_zero(&mut order, run);
    order.move_params.wait = wait;
    order.move_params.mine_laying = lay_mines;
    order.move_params.escape_tile = escape;
    order.move_params.jump = jump;
    order.move_params.keep_moving = keep_moving;

    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return 1;
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }
    if set_order {
        clear_cur_tac_order(w, slot, true);
    }
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.way_path.clear(); // setMoveWayPath(NULL, 0)
        p.lane_b_move.cur_way_pt = 0;
        p.lane_b_move.cur_way_dir = 0;
    }
    set_move_goal(
        w,
        slot,
        super::moveorders::MOVEGOAL_LOCATION,
        Some(location),
    );
    let t = w.clock.scenario_time;
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.time_of_last_step = t;
        p.lane_b_move.run = run;
    }
    if set_order {
        clear_attack_orders(w, slot);
    }
    let mut move_params = MOVEPARAM_INIT | MOVEPARAM_FACE_TARGET;
    if jump {
        move_params |= MOVEPARAM_JUMP;
    }
    w.paths.request(crate::mover::manager::PathRequest {
        pilot: slot,
        selector: -1,
        params: move_params,
        source: 20,
    });
    if set_order {
        set_general_tac_order(w, slot, order);
        let use_goal_plan = w.pilots.get(slot).is_some_and(|p| p.use_goal_plan);
        if use_goal_plan {
            set_main_goal(w, slot, goal_action::MOVE, None, Some(location), -1.0);
        } else {
            set_main_goal(w, slot, goal_action::NONE, None, None, -1.0);
        }
    }
    0
}

/// `MechWarrior::coreMoveToObject` (warrior.cpp:5361).
pub fn core_move_to_object(
    w: &mut World,
    slot: u16,
    object: Option<ObjectHandle>,
    params: u32,
) -> i32 {
    let run = params & param::RUN != 0;
    let jump = params & param::JUMP != 0;
    let face_object = params & param::FACE_OBJECT != 0;
    let lay_mines = params & param::LAY_MINES != 0;
    let set_order = params & param::DONT_SET_ORDER == 0;
    let keep_moving = params & param::DONT_KEEP_MOVING == 0;

    let mut object = object;
    if object.is_none() {
        if w.pilots
            .get(slot)
            .is_some_and(|p| p.core_scan_target == WatchId(0))
        {
            core_scan(w, slot, None, params);
        }
        let wid = w
            .pilots
            .get(slot)
            .map_or(WatchId(0), |p| p.core_scan_target);
        object = w.objects.by_watch_id(wid);
    }
    let Some(object) = object else {
        return 1;
    };
    let Some(me) = vehicle(w, slot) else {
        return 1;
    };
    let (is_mover, position) = match w.objects.get(object) {
        Some(o) => (o.is_mover(), o.c.position),
        None => return 1,
    };
    let goal = if is_mover {
        position
    } else {
        mover_ops::calc_offset_move_goal(w, me, &position)
    };
    let wid = w.objects.watch_id(object);
    let mut order = new_order(w, OrderOrigin::Commander, OrderCode::MoveToObject, false);
    order.target = wid;
    order.selection_index = -1;
    order.move_params.from_area = -1;
    way_point_zero(&mut order, goal);
    mode_zero(&mut order, run);
    order.move_params.face_object = face_object;
    order.move_params.wait = false;
    order.move_params.mine_laying = lay_mines;
    order.move_params.jump = jump;
    order.move_params.keep_moving = keep_moving;

    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return 1;
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }
    if set_order {
        clear_cur_tac_order(w, slot, true);
    }
    order.target = w.objects.watch_id(object);
    set_move_goal(w, slot, i64::from(wid.0), Some(position));
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.run = run;
    }
    if set_order {
        clear_attack_orders(w, slot);
    }
    let mut move_params = MOVEPARAM_INIT;
    if jump {
        move_params |= MOVEPARAM_JUMP;
    }
    if face_object {
        move_params |= MOVEPARAM_FACE_TARGET;
    }
    if set_order && !is_mover {
        move_params |= MOVEPARAM_STEP_ADJACENT_TARGET;
    }
    request_move_path(w, slot, -1, move_params, 15);
    let pos = w.objects.get(object).map_or(position, |o| o.c.position);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.goal_object_position = pos;
    }
    if set_order {
        set_general_tac_order(w, slot, order);
        let use_goal_plan = w.pilots.get(slot).is_some_and(|p| p.use_goal_plan);
        if use_goal_plan {
            set_main_goal(w, slot, goal_action::MOVE, Some(object), None, -1.0);
        } else {
            set_main_goal(w, slot, goal_action::NONE, None, None, -1.0);
        }
    }
    0
}

/// `MechWarrior::coreEject` (warrior.cpp:5455).
pub fn core_eject(w: &mut World, slot: u16) -> i32 {
    let Some(me) = vehicle(w, slot) else { return 1 };
    let Some(obj) = w.objects.get(me) else {
        return 1;
    };
    if obj.c.class != crate::object::ObjectClass::BattleMech {
        return 1;
    }
    if obj.c.is_disabled() {
        return 1;
    }
    let order = new_order(w, OrderOrigin::Commander, OrderCode::Eject, false);
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }
    // Order has no move goal... no attack target...
    clear_move_orders(w, slot);
    clear_attack_orders(w, slot);
    eject_then_set_order(w, slot, me, order, false);
    1
}

/// `handleEjection(); setGeneralTacOrder(order)`: when the ejection's rest is still queued behind the incapacitation handler (inside an
/// ABL native), the order is installed after it too, so a handler that gives its own general order cannot be overwritten by `Eject`.
fn eject_then_set_order(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    order: crate::command::TacticalOrder,
    unit_order: bool,
) {
    crate::combat::damage::handle_ejection(w, me);
    if w.abl_running
        && w.deferred_brain_calls
            .iter()
            .any(|c| matches!(c, crate::ai::brain::DeferredBrainCall::EjectRest(d) if *d == me))
    {
        w.deferred_brain_calls
            .push(crate::ai::brain::DeferredBrainCall::EjectOrder(
                slot, unit_order,
            ));
    } else {
        set_general_tac_order(w, slot, order);
    }
}

/// The queued tail of [`eject_then_set_order`].
pub(crate) fn finish_eject_order(w: &mut World, slot: u16, unit_order: bool) {
    let order = new_order(w, OrderOrigin::Commander, OrderCode::Eject, unit_order);
    set_general_tac_order(w, slot, order);
}

/// `MechWarrior::corePower` (warrior.cpp:5506).
pub fn core_power(w: &mut World, slot: u16, power_up: bool) -> i32 {
    let Some(me) = vehicle(w, slot) else { return 1 };
    let Some(obj) = w.objects.get(me) else {
        return 1;
    };
    if obj.c.class != crate::object::ObjectClass::BattleMech {
        return 1;
    }
    if obj.c.is_disabled() {
        return 1;
    }
    use crate::object::ObjectStatus as S;
    if power_up {
        if obj.c.status == S::Normal {
            return 1;
        }
    } else if obj.c.status == S::ShutDown {
        return 1;
    }
    let code = if power_up {
        OrderCode::PowerUp
    } else {
        OrderCode::PowerDown
    };
    let mut order = new_order(w, OrderOrigin::Commander, code, false);
    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return 1;
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }
    clear_move_orders(w, slot);
    clear_attack_orders(w, slot);
    // Check if we've cooled enough to restart...
    if power_up {
        mover_ops::start_up(w, me); // canPowerUp is true: there is no heat
    } else {
        mover_ops::shut_down(w, me);
    }
    set_general_tac_order(w, slot, order);
    0
}

/// `MechWarrior::coreAttack` (warrior.cpp:5596).
pub fn core_attack(w: &mut World, slot: u16, target: Option<ObjectHandle>, params: u32) -> i32 {
    let mut pursue = params & param::PURSUE != 0;
    let obliterate = params & param::OBLITERATE != 0;
    let mut jump = params & param::JUMP != 0;
    let mut keep_moving = params & param::DONT_KEEP_MOVING == 0;

    let mut attack_method = AttackMethod::Ranged;
    match params & param::ATTACK_MASK {
        param::ATTACK_DFA => attack_method = AttackMethod::Dfa,
        param::ATTACK_RAMMING => attack_method = AttackMethod::Ramming,
        _ => {}
    }
    let mut attack_range = FIRERANGE_OPTIMAL;
    match params & param::RANGE_MASK {
        param::RANGE_RAMMING => attack_range = FIRERANGE_RAMMING,
        param::RANGE_LONGEST => attack_range = FIRERANGE_LONGEST,
        param::RANGE_OPTIMAL => attack_range = FIRERANGE_OPTIMAL,
        param::RANGE_SHORT => attack_range = FIRERANGE_SHORT,
        param::RANGE_MEDIUM => attack_range = FIRERANGE_MEDIUM,
        param::RANGE_LONG => attack_range = FIRERANGE_LONG,
        _ => {}
    }
    let mut aim_location = -1;
    match params & param::AIM_MASK {
        param::AIM_HEAD => aim_location = BODY_HEAD,
        param::AIM_ARM => aim_location = BODY_RARM,
        param::AIM_LEG => aim_location = BODY_RLEG,
        _ => {}
    }
    let mut tactic = Tactic::None;
    match params & param::TACTIC_MASK {
        param::TACTIC_FLANK_RIGHT => tactic = Tactic::FlankRight,
        param::TACTIC_FLANK_LEFT => tactic = Tactic::FlankLeft,
        param::TACTIC_FLANK_REAR => tactic = Tactic::FlankRear,
        param::TACTIC_STOP_AND_FIRE => {
            tactic = Tactic::StopAndFire;
            pursue = true;
        }
        param::TACTIC_TURRET => {
            tactic = Tactic::Turret;
            pursue = false;
            jump = false;
            keep_moving = false;
        }
        param::TACTIC_JOUST => tactic = Tactic::Joust,
        _ => {}
    }

    let num_weapons = vehicle(w, slot)
        .and_then(|h| w.objects.get(h))
        .and_then(crate::combat::loadout_of)
        .map_or(0, |l| l.num_weapons());
    if num_weapons == 0 && attack_method != AttackMethod::Ramming {
        clear_attack_orders(w, slot);
        return 1;
    }

    let mut target = target;
    if target.is_none() {
        if w.pilots
            .get(slot)
            .is_some_and(|p| p.core_scan_target == WatchId(0))
        {
            core_scan(w, slot, None, params);
        }
        let wid = w
            .pilots
            .get(slot)
            .map_or(WatchId(0), |p| p.core_scan_target);
        target = w.objects.by_watch_id(wid);
    }
    let Some(target) = target else {
        return 1;
    };

    let target_wid = w.objects.watch_id(target);
    let mut order = new_order(w, OrderOrigin::Commander, OrderCode::AttackObject, false);
    order.target = target_wid;
    order.attack_params.method = attack_method;
    order.attack_params.aim_location = aim_location;
    if attack_method == AttackMethod::Ramming {
        attack_range = FIRERANGE_RAMMING;
    }
    order.attack_params.range = attack_range;
    order.attack_params.pursue = pursue;
    order.attack_params.obliterate = obliterate;
    order.attack_params.tactic = tactic;
    order.move_params.keep_moving = keep_moving;
    order.move_params.jump = jump;

    let result = tacorder::status(w, slot, &mut order);
    if result == TACORDER_SUCCESS {
        return 1;
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }
    clear_cur_tac_order(w, slot, true);
    set_situation_fire_range(w, slot, Some(attack_range));

    // If we also want to pursue it, set up the move order here (the call is commented out in the source)...
    if !pursue {
        clear_move_orders(w, slot);
    }
    // Now, set up the tactical order, and pass it to the pilot...
    set_attack_orders(
        w,
        slot,
        ATTACK_TO_DESTROY,
        Some(target),
        aim_location,
        pursue,
    );
    if let Some(p) = w.pilots.get_mut(slot) {
        p.situation.open_fire = true;
    }
    // Remember the most recent target...
    set_last_target(w, slot, Some(target), obliterate, false);
    set_general_tac_order(w, slot, order);
    let use_goal_plan = w.pilots.get(slot).is_some_and(|p| p.use_goal_plan);
    if use_goal_plan {
        set_main_goal(w, slot, goal_action::ATTACK, Some(target), None, -1.0);
    } else {
        set_main_goal(w, slot, goal_action::NONE, None, None, -1.0);
    }
    0
}

/// `MechWarrior::coreCapture` (warrior.cpp:5750): validate the target, ask the current capture-order status, then install it.
pub fn core_capture(w: &mut World, slot: u16, object: Option<ObjectHandle>, params: u32) -> i32 {
    let mut object = object;
    if object.is_none() {
        if w.pilots
            .get(slot)
            .is_some_and(|p| p.core_scan_target == WatchId::NONE)
        {
            super::targeting::core_scan(w, slot, None, params);
        }
        let target = w
            .pilots
            .get(slot)
            .map_or(WatchId::NONE, |p| p.core_scan_target);
        object = w.objects.by_watch_id(target);
    }
    let Some(me) = vehicle(w, slot) else { return 1 };
    let Some(target) = object else { return 1 };
    let Some(team) = w.objects.get(me).map(|o| o.team_id()) else {
        return 1;
    };
    if !super::capture::is_captureable(w, target, team)
        || crate::object::hooks::is_friendly(w, target, me)
    {
        return 1;
    }
    let mut order = new_order(w, OrderOrigin::Commander, OrderCode::Capture, false);
    order.target = w.objects.watch_id(target);
    order.selection_index = -1;
    order.move_params.from_area = -1;
    mode_zero(&mut order, params & param::RUN != 0);
    order.move_params.face_object = params & param::FACE_OBJECT != 0;
    order.move_params.wait = false;
    order.move_params.keep_moving = false;
    order.move_params.jump = params & param::JUMP != 0;
    if tacorder::status(w, slot, &mut order) == TACORDER_SUCCESS {
        return 1;
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.equals(&order))
    {
        return 0;
    }
    set_general_tac_order(w, slot, order);
    if w.pilots.get(slot).is_some_and(|p| p.use_goal_plan) {
        set_main_goal(w, slot, goal_action::CAPTURE, Some(target), None, 250.0);
    } else {
        set_main_goal(w, slot, goal_action::NONE, None, None, -1.0);
    }
    0
}

/// `MechWarrior::coreScan` (warrior.cpp:5806).
pub fn core_scan(w: &mut World, slot: u16, object: Option<ObjectHandle>, params: u32) -> i32 {
    super::targeting::core_scan(w, slot, object, params)
}

/// `MechWarrior::coreControl` (warrior.cpp:5860).
pub fn core_control(w: &mut World, slot: u16, object: Option<ObjectHandle>, params: u32) -> i32 {
    let mut object = object;
    if object.is_none() {
        if w.pilots
            .get(slot)
            .is_some_and(|p| p.core_scan_target == WatchId(0))
        {
            core_scan(w, slot, None, params);
        }
        let wid = w
            .pilots
            .get(slot)
            .map_or(WatchId(0), |p| p.core_scan_target);
        object = w.objects.by_watch_id(wid);
    }
    let Some(object) = object else { return 1 };
    let action = super::targeting::calc_action(w, slot, object);
    if action == OrderCode::AttackObject {
        core_attack(w, slot, Some(object), params)
    } else if action == OrderCode::Capture {
        core_capture(w, slot, Some(object), params)
    } else {
        1
    }
}

/// `getLastTarget` re-export for the decision code.
pub fn last_target(w: &mut World, slot: u16) -> Option<ObjectHandle> {
    get_last_target(w, slot)
}
