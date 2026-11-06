// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `TacticalOrder::execute` (tacordr.cpp:755-1260) and `TacticalOrder::status` (tacordr.cpp:1262-1910).
//!
//! `execute` turns an order into pilot actions (move goal, path request, power state, attack target...) and returns the radio message
//! to play; `status` answers whether the order is accomplished (`TACORDER_SUCCESS`) and, for a few orders, advances them. Both take the
//! order by `&mut` because the C++ writes into `this` (`delayedTime`, `stage`, `code`, `targetObjectClass`, `attackParams`).
//!
//! The refit / get-fixed / recover orders are in [`super::support_orders`]. Capture uses [`super::capture`] for blocker checks and
//! changes ownership when the target is in range.

use super::orders::{
    self, order_attack_object, order_attack_point, order_eject, order_jump_to_object,
    order_jump_to_point, order_move_to_object, order_move_to_point, order_power_down,
    order_power_up, order_stop, order_wait, order_withdraw, param, radio,
};
use super::warrior::{self, trigger_alarm, vehicle, TACORDER_FAILURE, TACORDER_SUCCESS};
use super::{mover_ops, report_unavailable, AiUnavailable};
use crate::command::{AttackMethod, OrderCode, OrderOrigin, Tactic, TacticalOrder, TravelMode};
use crate::math::{Vec3, Vec3Ext};
use crate::World;

fn way_point(order: &TacticalOrder, i: usize) -> Vec3 {
    order
        .move_params
        .way_path
        .points
        .get(i)
        .copied()
        .unwrap_or_default()
}

fn is_fast(order: &TacticalOrder) -> bool {
    order.move_params.way_path.mode.first() == Some(&TravelMode::Fast)
}

/// `params |= (TACORDER_PARAM_TACTIC_FLANK_RIGHT << (tactic - 1))` (tacordr.cpp:1085): the C++ shifts the first tactic bit by the
/// enum value minus one, so `TACTIC_FLANK_LEFT` (1) sets the FLANK_RIGHT bit, `FLANK_RIGHT` (2) the FLANK_LEFT bit, and so on.
fn tactic_params(tactic: Tactic) -> u32 {
    let n = tactic as u32;
    if n == 0 {
        0
    } else {
        param::TACTIC_FLANK_RIGHT << (n - 1)
    }
}

/// `curTacOrder.status(this)`: the real member is updated, so run it on a copy and put the copy back.
pub fn cur_status(w: &mut World, slot: u16) -> i32 {
    let Some(mut cur) = w.pilots.get(slot).map(|p| p.cur_order.clone()) else {
        return TACORDER_SUCCESS;
    };
    let result = status(w, slot, &mut cur);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.cur_order = cur;
    }
    result
}

/// `curTacOrder.execute(this, message)` (the movement decision tree runs it on the member).
pub fn cur_execute(w: &mut World, slot: u16) -> (i32, i32) {
    let Some(mut cur) = w.pilots.get(slot).map(|p| p.cur_order.clone()) else {
        return (TACORDER_FAILURE, -1);
    };
    let out = execute_full(w, slot, &mut cur);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.cur_order = cur;
    }
    out
}

/// `TacticalOrder::execute(warrior, message)` returning only the message.
pub fn execute(w: &mut World, slot: u16, order: &mut TacticalOrder) -> i32 {
    execute_full(w, slot, order).1
}

/// `TacticalOrder::execute`: `(result, message)`.
pub fn execute_full(w: &mut World, slot: u16, order: &mut TacticalOrder) -> (i32, i32) {
    let mut result = TACORDER_FAILURE;
    let mut message = -1;
    let Some(me) = vehicle(w, slot) else {
        return (result, message);
    };
    // a mover in a refit or recover operation that gets a new order leaves it (tacordr.cpp:760-795)
    super::support_orders::clear_buddies(w, me);
    let target = w.objects.by_watch_id(order.target);
    let now = w.clock.scenario_time;
    match order.code {
        OrderCode::MoveToPoint => {
            if order.delayed_time != -1.0 {
                if order.delayed_time > now {
                    return (TACORDER_FAILURE, message);
                }
                order.delayed_time = -1.0;
            }
            let location = way_point(order, 0);
            let mut params = param::NONE;
            if is_fast(order) {
                params |= param::RUN;
            }
            if order.move_params.wait {
                params |= param::WAIT;
            }
            if order.move_params.mine_laying {
                params |= param::LAY_MINES;
            }
            if order.move_params.escape_tile {
                params |= param::ESCAPE_TILE;
            }
            if order.move_params.jump {
                params |= param::JUMP;
            }
            result = order_move_to_point(
                w,
                slot,
                order.unit_order,
                true,
                order.origin,
                location,
                order.selection_index,
                params,
            );
            message = -1;
        }
        OrderCode::MoveToObject => {
            let mut params = param::NONE;
            if is_fast(order) {
                params |= param::RUN;
            }
            if order.move_params.face_object {
                params |= param::FACE_OBJECT;
            }
            if order.move_params.mine_laying {
                params |= param::LAY_MINES;
            }
            order_move_to_object(
                w,
                slot,
                order.unit_order,
                true,
                order.origin,
                target,
                order.move_params.from_area,
                order.selection_index,
                params,
            );
            message = -1;
        }
        OrderCode::JumpToPoint => {
            // If we got into here, we know the jump can be done...
            if order.delayed_time != -1.0 {
                if order.delayed_time > now {
                    return (TACORDER_FAILURE, message);
                }
                order.delayed_time = -1.0;
            }
            let location = way_point(order, 0);
            result =
                order_jump_to_point(w, slot, order.unit_order, true, order.origin, location, -1);
        }
        OrderCode::JumpToObject => {
            if order.delayed_time != -1.0 {
                if order.delayed_time > now {
                    return (TACORDER_FAILURE, message);
                }
                order.delayed_time = -1.0;
            }
            let Some(t) = target else {
                return (TACORDER_SUCCESS, message);
            };
            result = order_jump_to_object(w, slot, order.unit_order, true, order.origin, t, -1);
        }
        OrderCode::Escort | OrderCode::Follow => {}
        OrderCode::Guard => {
            order.attack_params.attack_type = orders::ATTACK_TO_DESTROY;
            order.attack_params.method = AttackMethod::Ranged;
            order.attack_params.pursue = true;
            order.attack_params.range = crate::command::FIRERANGE_OPTIMAL;
            message = radio::GUARD;
        }
        OrderCode::Stop => {
            order_stop(w, slot, order.unit_order, true);
            message = radio::ACK;
        }
        OrderCode::PowerUp => {
            if order.delayed_time > -1.0 {
                if now < order.delayed_time {
                    return (TACORDER_FAILURE, message);
                }
                order.delayed_time = -1.0;
            }
            order_power_up(w, slot, order.unit_order, order.origin);
        }
        OrderCode::PowerDown => {
            if order.delayed_time > -1.0 {
                if now < order.delayed_time {
                    return (TACORDER_FAILURE, message);
                }
                order.delayed_time = -1.0;
            }
            order_power_down(w, slot, order.unit_order, order.origin);
        }
        OrderCode::WaypointsDone => {}
        OrderCode::Eject => {
            order_eject(w, slot, order.unit_order, order.origin);
        }
        OrderCode::AttackObject => {
            message = radio::ILLEGAL_ORDER;
            result = -1;
            if let Some(t) = target {
                calc_fire_ranges(w, me);
                order.target_object_class = w.objects.get(t).map_or(0, |o| o.c.class as i32);
                // If the target is already disabled, I guess that means we simply want to obliterate the sucker...
                order.attack_params.obliterate =
                    w.objects.get(t).is_some_and(|o| o.c.is_disabled());
                if !w.objects.get(t).is_some_and(|o| o.c.is_destroyed()) {
                    let mut params = param::NONE;
                    if order.attack_params.obliterate {
                        params |= param::OBLITERATE;
                    }
                    if order.attack_params.pursue {
                        params |= param::PURSUE;
                    }
                    if order.attack_params.tactic != Tactic::None {
                        params |= tactic_params(order.attack_params.tactic);
                    }
                    // 07-05-00 need to keep run info -- HKG
                    if is_fast(order) {
                        params |= param::RUN;
                    }
                    result = order_attack_object(
                        w,
                        slot,
                        order.unit_order,
                        order.origin,
                        Some(t),
                        order.attack_params.attack_type,
                        order.attack_params.method,
                        order.attack_params.range,
                        order.attack_params.aim_location,
                        order.move_params.from_area,
                        params,
                    );
                    if result == TACORDER_FAILURE {
                        // `RollDice(50)` is on the visual stream (radio chatter, tacordr.cpp:959)
                        let roll = w.rng.0.borrow_mut().visual.roll_dice(50);
                        message = if roll { radio::TARGET_ACK } else { radio::ACK };
                    }
                }
            } else {
                // Targeting a terrain location...
                warrior::set_last_target(w, slot, None, false, false);
                calc_fire_ranges(w, me);
                let mut params = param::NONE;
                if order.attack_params.obliterate {
                    params |= param::OBLITERATE;
                }
                if order.attack_params.pursue {
                    params |= param::PURSUE;
                }
                if order.attack_params.tactic != Tactic::None {
                    params |= tactic_params(order.attack_params.tactic);
                }
                result = order_attack_point(
                    w,
                    slot,
                    order.unit_order,
                    order.origin,
                    order.attack_params.target_point,
                    order.attack_params.attack_type,
                    order.attack_params.method,
                    order.attack_params.range,
                    params,
                );
                if result == TACORDER_FAILURE {
                    let roll = w.rng.0.borrow_mut().visual.roll_dice(50);
                    message = if roll { radio::TARGET_ACK } else { radio::ACK };
                }
            }
        }
        OrderCode::AttackPoint => {
            message = radio::ILLEGAL_ORDER;
            // Targeting a terrain location... (`result = -1` is overwritten by the call below)
            warrior::set_last_target(w, slot, None, false, false);
            calc_fire_ranges(w, me);
            let mut params = param::NONE;
            if order.attack_params.obliterate {
                params |= param::OBLITERATE;
            }
            if order.attack_params.pursue {
                params |= param::PURSUE;
            }
            if order.attack_params.tactic != Tactic::None {
                params |= tactic_params(order.attack_params.tactic);
            }
            result = order_attack_point(
                w,
                slot,
                order.unit_order,
                order.origin,
                order.attack_params.target_point,
                order.attack_params.attack_type,
                order.attack_params.method,
                order.attack_params.range,
                params,
            );
            if result == TACORDER_FAILURE {
                let roll = w.rng.0.borrow_mut().visual.roll_dice(50);
                message = if roll { radio::TARGET_ACK } else { radio::ACK };
            }
        }
        OrderCode::HoldFire => {
            order_wait(w, slot, order.unit_order, order.origin, 0, true);
        }
        OrderCode::Withdraw => {
            let location = way_point(order, 0);
            order_withdraw(w, slot, order.unit_order, order.origin, location);
        }
        OrderCode::Scramble => {}
        OrderCode::Refit | OrderCode::GetFixed | OrderCode::Recover => {
            result = super::support_orders::execute(w, slot, me, order, target);
        }
        OrderCode::LoadIntoCarrier => {
            // `USE_ELEMENTALS` is not defined: result stays SUCCESS and the order is finished (`stage = -1`)
            result = TACORDER_SUCCESS;
            order.stage = -1;
        }
        OrderCode::DeployElementals => {
            result = TACORDER_SUCCESS;
            if result == TACORDER_SUCCESS {
                order.stage = -1;
            }
        }
        OrderCode::Capture => {
            result = TACORDER_SUCCESS;
            let Some(t) = target else {
                report_unavailable(w, me, AiUnavailable::NullOrderTarget);
                return (TACORDER_FAILURE, message);
            };
            let my_team = w.objects.get(me).map_or(-1, |o| o.team_id());
            let target_team = w.objects.get(t).map_or(-1, |o| o.team_id());
            let friendly = crate::object::hooks::team_relation(w, my_team, target_team)
                == Some(super::Relation::Friendly);
            if my_team != target_team
                && !friendly
                && super::capture::is_captureable(w, t, my_team)
                && super::capture::blockers(w, t, me).is_empty()
            {
                let position = w.objects.get(t).map_or(Vec3::zero(), |o| o.c.position);
                if mover_ops::distance_from(w, me, &position) < 30.0 {
                    if complete_capture(w, slot, me, t, my_team) {
                        result = TACORDER_SUCCESS;
                    }
                } else {
                    let mut params = param::NONE;
                    if is_fast(order) {
                        params |= param::RUN;
                    }
                    if order.move_params.face_object {
                        params |= param::FACE_OBJECT;
                    }
                    message = radio::ACK;
                    result = order_move_to_object(
                        w,
                        slot,
                        false,
                        true,
                        order.origin,
                        Some(t),
                        order.move_params.from_area,
                        0,
                        params,
                    );
                }
            }
            if result == TACORDER_SUCCESS {
                order.stage = -1;
            }
        }
        OrderCode::TraversePath | OrderCode::PatrolPath | OrderCode::Wait | OrderCode::None => {}
    }

    if order.origin == OrderOrigin::Player {
        trigger_alarm(w, slot, warrior::alarm::PLAYER_ORDER, order.code as i32);
    } else {
        message = -1;
    }

    if order.code != OrderCode::Withdraw {
        if let Some(obj) = w.objects.get_mut(me) {
            if obj.c.status != crate::object::ObjectStatus::Destroyed {
                // Any non-withdraw order forces us out of withdraw mode, if in it...
                match &mut obj.kind {
                    crate::object::Kind::Mech(m) => m.m.flags.withdrawing = false,
                    crate::object::Kind::Vehicle(v) => v.m.flags.withdrawing = false,
                    _ => {}
                }
            }
        }
    }
    (result, message)
}

/// `Mover::calcFireRanges()`; the changed flag is ignored by callers here.
pub fn calc_fire_ranges(w: &mut World, h: crate::ids::ObjectHandle) -> bool {
    let t = w.clock.scenario_time;
    let (Some(comps), Some(tables)) = (w.data.components.clone(), w.data.gamesys.tables.clone())
    else {
        return false;
    };
    match w.objects.get_mut(h).and_then(crate::combat::loadout_of_mut) {
        Some(l) => l.calc_fire_ranges(&comps, &tables, t),
        None => false,
    }
}

/// `TacticalOrder::status(warrior)`.
pub fn status(w: &mut World, slot: u16, order: &mut TacticalOrder) -> i32 {
    let mut result = TACORDER_SUCCESS;
    let target = w.objects.by_watch_id(order.target);
    let now = w.clock.scenario_time;
    let Some(me) = vehicle(w, slot) else {
        return result;
    };
    match order.code {
        OrderCode::Wait => {
            result = if now > order.delayed_time {
                TACORDER_SUCCESS
            } else {
                TACORDER_FAILURE
            };
        }
        OrderCode::MoveToPoint => {
            // `formation` and `formationPos` are both -1 constants: the first test is never true
            if order.delayed_time > -1.0 {
                if now < order.delayed_time {
                    return TACORDER_FAILURE;
                }
                order.delayed_time = -1.0;
                execute_full(w, slot, order);
            }
            let location = way_point(order, 0);
            let cell_distance_to_goal = mover_ops::cell_distance_from(w, me, &location);
            if cell_distance_to_goal < 1 {
                if order.move_params.wait {
                    order.code = OrderCode::Wait;
                    result = TACORDER_FAILURE;
                }
            } else {
                result = TACORDER_FAILURE;
            }
        }
        OrderCode::MoveToObject => {
            if target.is_some() {
                let location = way_point(order, 0);
                let cell_distance_to_goal = mover_ops::cell_distance_from(w, me, &location);
                if cell_distance_to_goal > 0 {
                    result = TACORDER_FAILURE;
                }
            }
        }
        OrderCode::JumpToPoint => {
            if order.stage != 3 {
                result = TACORDER_FAILURE;
            }
        }
        OrderCode::JumpToObject => {
            // `USE_JUMPING` is not defined: the check is compiled out and the order is always "done"
        }
        OrderCode::Escort | OrderCode::Follow => {}
        OrderCode::Guard => result = TACORDER_FAILURE,
        OrderCode::Stop => result = TACORDER_SUCCESS,
        OrderCode::PowerUp => {
            if order.delayed_time > -1.0 {
                if now < order.delayed_time {
                    return TACORDER_FAILURE;
                }
                order.delayed_time = -1.0;
                execute_full(w, slot, order);
            }
            if w.objects.get(me).map(|o| o.c.status) != Some(crate::object::ObjectStatus::Normal) {
                result = TACORDER_FAILURE;
            }
        }
        OrderCode::PowerDown => {
            if order.delayed_time > -1.0 {
                if now < order.delayed_time {
                    return TACORDER_FAILURE;
                }
                order.delayed_time = -1.0;
                execute_full(w, slot, order);
            }
            // For now, they're always powering down until given a new order...
            result = TACORDER_FAILURE;
        }
        OrderCode::WaypointsDone | OrderCode::Eject => {}
        OrderCode::AttackObject => {
            if let Some(t) = target {
                if let Some(o) = w.objects.get(t) {
                    if o.c.is_destroyed() {
                        result = TACORDER_SUCCESS;
                    } else if o.c.is_disabled() {
                        result = if order.attack_params.obliterate {
                            TACORDER_FAILURE
                        } else {
                            TACORDER_SUCCESS
                        };
                    } else {
                        result = TACORDER_FAILURE;
                    }
                }
            }
        }
        OrderCode::AttackPoint | OrderCode::HoldFire | OrderCode::Withdraw => {
            result = TACORDER_FAILURE;
        }
        OrderCode::Scramble => {
            // result = FAILURE; the distance test has no effect
            result = TACORDER_FAILURE;
        }
        OrderCode::Refit => {
            result = super::support_orders::status_refit(w, slot, me, order, target)
        }
        OrderCode::GetFixed => {
            result = super::support_orders::status_get_fixed(w, slot, me, order, target)
        }
        OrderCode::Recover => result = super::support_orders::status_recover(w, me, order, target),
        OrderCode::LoadIntoCarrier => result = TACORDER_FAILURE,
        OrderCode::DeployElementals => {}
        OrderCode::Capture => {
            if order.stage != -1 {
                let Some(t) = target else {
                    report_unavailable(w, me, AiUnavailable::NullOrderTarget);
                    return TACORDER_FAILURE;
                };
                let my_team = w.objects.get(me).map_or(-1, |o| o.team_id());
                let target_team = w.objects.get(t).map_or(-1, |o| o.team_id());
                if my_team == target_team {
                    result = TACORDER_SUCCESS;
                } else if !super::capture::blockers(w, t, me).is_empty() {
                    order.status_code = 1;
                    warrior::radio_message(w, slot, radio::CANNOT_CAPTURE);
                    result = TACORDER_SUCCESS;
                } else if mover_ops::distance_from(
                    w,
                    me,
                    &w.objects.get(t).map_or(Vec3::zero(), |o| o.c.position),
                ) < 30.0
                    && super::capture::is_captureable(w, t, my_team)
                    && complete_capture(w, slot, me, t, my_team)
                {
                    // tacordr.cpp:1783: the unit arrived; the status check hands the target over.
                    result = TACORDER_SUCCESS;
                } else {
                    result = TACORDER_FAILURE;
                }
            }
        }
        OrderCode::TraversePath | OrderCode::PatrolPath | OrderCode::None => {}
    }
    result
}

/// The capture hand-over shared by the execute and status paths (tacordr.cpp:1139-1160, 1785-1806).
/// Returns whether the target class is capturable by hand-over (a building or ground vehicle).
fn complete_capture(
    w: &mut World,
    slot: u16,
    me: crate::ObjectHandle,
    t: crate::ObjectHandle,
    team: i32,
) -> bool {
    match w.objects.get(t).map(|o| o.c.class) {
        Some(crate::object::ObjectClass::Building) => {
            super::capture::capture_target(w, t, me, team);
            warrior::radio_message(w, slot, radio::CAPTURED_BUILDING);
            true
        }
        Some(crate::object::ObjectClass::GroundVehicle) => {
            super::capture::capture_target(w, t, me, team);
            warrior::radio_message(w, slot, radio::CAPTURED_VEHICLE);
            true
        }
        _ => false,
    }
}
