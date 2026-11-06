// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The repair, get-fixed and recover orders (`TacticalOrder::execute` tacordr.cpp:1047-1105, 1219-1241 and `TacticalOrder::status`
//! tacordr.cpp:1429-1630, 1822-1900): a repair vehicle drives to a damaged mech and refits it, a mech or vehicle drives into a repair
//! bay, a recovery vehicle drives to a disabled mech and recovers it. The orders keep their state in the buddy watch ids of the
//! two movers (`refitBuddyWID`, `recoverBuddyWID`) and in the order's `stage` and `time`.
//!
//! The C++ `OrderCode::Recover` status is `STOP("RECOVER TACORDER NO LONGER VALID")` followed by `#if 0` code that does not compile
//! (it calls `Mover::recover(points, used)`, which takes no arguments), so the GUI's salvage order would crash a debug build and stop a
//! release one. The `#if 0` sequence is ported as written, with the compile errors resolved by `Mover::recover()` and the status
//! forced as the live recovery craft does (see `rust/docs/DIFFERENCES.md`).

use super::orders::{order_move_to_point, order_power_down, order_power_up, param};
use super::warrior::{slot_of, TACORDER_FAILURE, TACORDER_SUCCESS};
use crate::combat::fx::death;
use crate::command::{OrderCode, OrderOrigin, TacticalOrder, TravelMode};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::mover::refit as rf;
use crate::object::{flags, terr, Kind, ObjectStatus};
use crate::World;

fn is_vehicle(w: &World, h: ObjectHandle) -> bool {
    matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Vehicle(_)))
}

fn is_mech(w: &World, h: ObjectHandle) -> bool {
    matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(_)))
}

fn is_building(w: &World, h: ObjectHandle) -> bool {
    matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Building(_)))
}

/// `refitBuddyWID` of a mover or a building.
pub(super) fn refit_buddy(w: &World, h: ObjectHandle) -> WatchId {
    match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.m.refit_buddy,
        Some(Kind::Vehicle(v)) => v.m.refit_buddy,
        Some(Kind::Building(b)) => b.refit_buddy,
        _ => WatchId::NONE,
    }
}

fn set_refit_buddy(w: &mut World, h: ObjectHandle, wid: WatchId) {
    match w.objects.get_mut(h).map(|o| &mut o.kind) {
        Some(Kind::Mech(m)) => m.m.refit_buddy = wid,
        Some(Kind::Vehicle(v)) => v.m.refit_buddy = wid,
        Some(Kind::Building(b)) => b.refit_buddy = wid,
        _ => {}
    }
}

fn recover_buddy(w: &World, h: ObjectHandle) -> WatchId {
    match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.m.recover_buddy,
        Some(Kind::Vehicle(v)) => v.m.recover_buddy,
        _ => WatchId::NONE,
    }
}

fn set_recover_buddy(w: &mut World, h: ObjectHandle, wid: WatchId) {
    match w.objects.get_mut(h).map(|o| &mut o.kind) {
        Some(Kind::Mech(m)) => m.m.recover_buddy = wid,
        Some(Kind::Vehicle(v)) => v.m.recover_buddy = wid,
        _ => {}
    }
}

fn set_refitting(w: &mut World, h: ObjectHandle, on: bool) {
    if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        v.refitting = on;
    }
}

fn set_recovering(w: &mut World, h: ObjectHandle, on: bool) {
    if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        v.recovering = on;
    }
}

/// The start of `TacticalOrder::execute` (tacordr.cpp:760-795): a mover in a refit or recover operation that gets any new order
/// leaves it, and so does its buddy.
pub fn clear_buddies(w: &mut World, me: ObjectHandle) {
    let wid = refit_buddy(w, me);
    if wid != WatchId::NONE {
        let buddy = w.objects.by_watch_id(wid);
        // turn off refitting (may not be on, but make sure)
        set_refitting(w, me, false);
        if let Some(b) = buddy {
            set_refitting(w, b, false);
            set_refit_buddy(w, b, WatchId::NONE);
        }
        set_refit_buddy(w, me, WatchId::NONE);
    }
    let wid = recover_buddy(w, me);
    if wid != WatchId::NONE {
        let buddy = w.objects.by_watch_id(wid);
        set_recovering(w, me, false);
        if let Some(b) = buddy {
            set_recovering(w, b, false);
            set_recover_buddy(w, b, WatchId::NONE);
        }
        set_recover_buddy(w, me, WatchId::NONE);
    }
}

fn move_params(order: &TacticalOrder) -> u32 {
    let mut params = param::FACE_OBJECT;
    if order.move_params.way_path.mode.first() == Some(&TravelMode::Fast) {
        params |= param::RUN;
    }
    params
}

/// `warrior->getMovePath()->numStepsWhenNotPaused == 0 && warrior->getMovePathRequest() == NULL`.
fn move_idle(w: &mut World, slot: u16) -> bool {
    let steps = super::get_move_path(w, slot).map_or(0, |p| p.num_steps_when_not_paused);
    steps == 0 && !super::warrior::has_path_request(w, slot)
}

fn position_of(w: &World, h: ObjectHandle) -> Vec3 {
    w.objects.get(h).map(|o| o.c.position).unwrap_or_default()
}

/// The centre of the cell `(row + dr, col + dc)` of `h`'s position (`worldToCell`, `cellToWorld`).
fn cell_destination(w: &World, h: ObjectHandle, dr: i32, dc: i32) -> Vec3 {
    let (row, col) = terr::world_to_cell(&w.terrain, &position_of(w, h));
    terr::cell_to_world(&w.terrain, row + dr, col + dc)
}

/// `TacticalOrder::execute` for `Refit`, `GetFixed` and `Recover`: check the pair can work together, drive the mover to the other, and
/// link the two. `order.stage` -1 marks an order that cannot be carried out.
pub fn execute(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    order: &mut TacticalOrder,
    target: Option<ObjectHandle>,
) -> i32 {
    let mut result = TACORDER_SUCCESS;
    let params = move_params(order);
    let my_wid = w.objects.watch_id(me);
    match order.code {
        OrderCode::Refit => {
            if let Some(t) = target.filter(|&t| is_mech(w, t) && rf::needs_refit(w, t)) {
                if is_vehicle(w, me)
                    && rf::vehicle_refit_points(w, me) > 0.0
                    && refit_buddy(w, me) == WatchId::NONE
                {
                    let destination = cell_destination(w, t, 0, 0);
                    result = order_move_to_point(
                        w,
                        slot,
                        order.unit_order,
                        false,
                        order.origin,
                        destination,
                        order.selection_index,
                        params,
                    );
                    order.time = 0.0;
                    if result == TACORDER_FAILURE {
                        let t_wid = w.objects.watch_id(t);
                        set_refit_buddy(w, t, my_wid);
                        set_refit_buddy(w, me, t_wid);
                    }
                }
                if result != TACORDER_FAILURE {
                    order.stage = -1; // order failed
                }
            }
        }
        OrderCode::GetFixed => {
            if let Some(bay) = target.filter(|&t| is_building(w, t)) {
                let mech_bay = w.objects.get(bay).is_some_and(|o| o.c.flag(flags::MECHBAY));
                if rf::building_refit_points(w, bay) > 0.0
                    && refit_buddy(w, bay) == WatchId::NONE
                    && ((is_mech(w, me) && mech_bay) || (is_vehicle(w, me) && !mech_bay))
                {
                    let destination = position_of(w, bay);
                    result = order_move_to_point(
                        w,
                        slot,
                        order.unit_order,
                        false,
                        order.origin,
                        destination,
                        order.selection_index,
                        params,
                    );
                    order.time = 0.0;
                    if result == TACORDER_FAILURE {
                        let bay_wid = w.objects.watch_id(bay);
                        set_refit_buddy(w, bay, my_wid);
                        set_refit_buddy(w, me, bay_wid);
                    }
                }
                if result != TACORDER_FAILURE {
                    order.stage = -1;
                }
            }
        }
        OrderCode::Recover => {
            if let Some(t) = target.filter(|&t| is_mech(w, t)) {
                if is_vehicle(w, me)
                    && rf::vehicle_recover_points(w, me) > 0.0
                    && recover_buddy(w, me) == WatchId::NONE
                {
                    // C++ `orderMoveToObject`; the move toward an object without an attack order needs the mover lane's
                    // goal scoring (`MoverDecisionUnavailable::ObjectGoalScoring`), so the vehicle drives to the cell
                    // beside the mech as a repair vehicle does (rust/docs/DIFFERENCES.md)
                    let destination = cell_destination(w, t, 1, 1);
                    result = order_move_to_point(
                        w,
                        slot,
                        order.unit_order,
                        false,
                        order.origin,
                        destination,
                        order.selection_index,
                        params,
                    );
                    order.time = 0.0;
                    if result == TACORDER_FAILURE {
                        let t_wid = w.objects.watch_id(t);
                        set_recover_buddy(w, t, my_wid);
                        set_recover_buddy(w, me, t_wid);
                    }
                }
                if result != TACORDER_FAILURE {
                    order.stage = -1; // order failed
                }
            }
        }
        _ => {}
    }
    result
}

fn power_up(w: &mut World, h: ObjectHandle, order: &TacticalOrder) {
    if let Some(slot) = slot_of(w, h) {
        order_power_up(w, slot, order.unit_order, OrderOrigin::SelfOrder);
    }
}

fn power_down(w: &mut World, h: ObjectHandle, order: &TacticalOrder) {
    if let Some(slot) = slot_of(w, h) {
        order_power_down(w, slot, order.unit_order, OrderOrigin::SelfOrder);
    }
}

/// `TacticalOrder::status` for `Refit`: the repair vehicle's side of the sequence (move, power the mech down, repair, power it up).
pub fn status_refit(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    order: &mut TacticalOrder,
    target: Option<ObjectHandle>,
) -> i32 {
    let now = w.clock.scenario_time;
    let Some(t) = target else {
        return TACORDER_SUCCESS;
    };
    // has refitter or refitee gotten distracted?
    if refit_buddy(w, me) == WatchId::NONE || refit_buddy(w, t) == WatchId::NONE {
        return TACORDER_SUCCESS;
    }
    // refitter and refitee *should* be pointing at each other...
    let (my_wid, t_wid) = (w.objects.watch_id(me), w.objects.watch_id(t));
    if refit_buddy(w, me) != t_wid {
        set_refit_buddy(w, me, t_wid);
    }
    if refit_buddy(w, t) != my_wid {
        set_refit_buddy(w, t, my_wid);
    }
    let general = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map(|t| t.mover_general.clone());
    let Some(general) = general else {
        return TACORDER_FAILURE;
    };
    let mut result = TACORDER_FAILURE;
    match order.stage {
        1 => {
            // move
            if move_idle(w, slot) {
                if crate::ai::mover_ops::distance_from(w, me, &position_of(w, t))
                    < general.refit_range
                {
                    order.stage += 1;
                } else {
                    let destination = cell_destination(w, t, 1, 1);
                    result = order_move_to_point(
                        w,
                        slot,
                        order.unit_order,
                        false,
                        order.origin,
                        destination,
                        order.selection_index,
                        param::FACE_OBJECT,
                    );
                }
            }
        }
        2 => {
            // power-down
            if order.time == 0.0 {
                power_down(w, t, order);
                order.time = now;
            } else if now > order.time + 3.0 {
                order.stage += 1;
                order.time = now;
            }
        }
        3 => {
            // do the deed
            set_refitting(w, me, true);
            if now > order.time + general.refit_time {
                let available = rf::vehicle_refit_points(w, me);
                let (done, used) = rf::refit(w, t, available, false);
                rf::burn_vehicle_refit_points(w, me, used);
                if rf::vehicle_refit_points(w, me) <= 0.0 {
                    // I died trying to repair someone. Power the someone back up.
                    power_up(w, t, order);
                    set_refit_buddy(w, t, WatchId::NONE);
                    set_refit_buddy(w, me, WatchId::NONE);
                }
                order.stage += i8::from(done);
                if !done {
                    order.time = now;
                }
                if rf::vehicle_refit_points(w, me) == 0.0 {
                    crate::combat::damage::disable(w, me, death::POWER_USED_UP);
                }
            }
        }
        4 => {
            // done: power-up and report
            set_refitting(w, me, false);
            power_up(w, t, order);
            result = TACORDER_SUCCESS;
            set_refit_buddy(w, t, WatchId::NONE);
            set_refit_buddy(w, me, WatchId::NONE);
        }
        -1 => result = TACORDER_SUCCESS, // execute failed
        _ => {}
    }
    result
}

/// `TacticalOrder::status` for `GetFixed`: the mover's side of a repair bay visit (drive in, power down, be repaired, walk off).
pub fn status_get_fixed(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    order: &mut TacticalOrder,
    target: Option<ObjectHandle>,
) -> i32 {
    let now = w.clock.scenario_time;
    let Some(bay) = target else {
        return TACORDER_SUCCESS;
    };
    if refit_buddy(w, me) == WatchId::NONE || refit_buddy(w, bay) == WatchId::NONE {
        return TACORDER_SUCCESS;
    }
    let Some(general) = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map(|t| t.mover_general.clone())
    else {
        return TACORDER_FAILURE;
    };
    let mut result = TACORDER_FAILURE;
    match order.stage {
        1 => {
            // move
            if move_idle(w, slot) {
                order.stage += 1;
            }
        }
        2 => {
            // power-down
            if order.time == 0.0 {
                power_down(w, me, order);
                order.time = now;
            } else if now > order.time + 3.0 {
                order.stage += 1;
                order.time = now;
                // C++ starts the bay sound when repair begins, after the
                // shutdown wait, at the repair-bay position (tacordr.cpp:1560).
                crate::events::emit_sound(w, bay, crate::events::sfx::REPAIRBAY_FX, false);
            }
        }
        3 => {
            // do the deed
            if now > order.time + general.refit_time {
                let available = rf::building_refit_points(w, bay);
                let (done, used) = rf::refit(w, me, available, false);
                rf::burn_building_refit_points(w, bay, used);
                order.stage += i8::from(done);
                if !done {
                    order.time = now;
                } else {
                    // power-up and walk off
                    crate::ai::mover_ops::start_up(w, me);
                    let (tile_row, tile_col) =
                        terr::world_to_tile(&w.terrain, &position_of(w, bay));
                    if let Some(destination) =
                        w.terrain
                            .tile_cell_to_world(tile_row + 1, tile_col + 1, 2, 2)
                    {
                        order_move_to_point(
                            w,
                            slot,
                            order.unit_order,
                            false,
                            OrderOrigin::SelfOrder,
                            destination,
                            order.selection_index,
                            0,
                        );
                    }
                }
            }
        }
        4 => {
            // walking off...
            if move_idle(w, slot) {
                order.stage += 1;
            }
        }
        5 => {
            // done: report
            result = TACORDER_SUCCESS;
            set_refit_buddy(w, bay, WatchId::NONE);
            set_refit_buddy(w, me, WatchId::NONE);
        }
        -1 => result = TACORDER_SUCCESS,
        _ => {}
    }
    result
}

/// Stage 2 of the Recover status (tacordr.cpp:1838-1851): the mech's arm-blown flags are cleared, a disabled mech is shut down (so it
/// can be started), and it changes sides with the campaign's best pilot for its tonnage (`Mission::tradeMover`; the new pilot
/// replaces the old one, so `clearAlarms` has nothing left to clear). `Err` when the campaign has no pilot for it, in which case
/// nothing was changed.
pub fn take_over(
    w: &mut World,
    mech: ObjectHandle,
) -> Result<(), crate::mission::trade::TradeError> {
    use crate::mission::trade::{trade_mover, TradeError};
    // `LogisticsData::getBestPilot(tonnage)` first: a refused trade must leave the mech as it was
    let tonnage = w.objects.get(mech).map_or(0.0, |o| o.c.tonnage);
    let Some(chooser) = w.recovery_pilots.clone() else {
        return Err(TradeError::NoPilot);
    };
    let pilot = {
        let world: &World = w;
        chooser.best_pilot(tonnage as i64, &|file| {
            crate::mission::trade::warrior_in_use(world, file)
        })
    }
    .ok_or(TradeError::NoPilot)?;
    if let Some(Kind::Mech(m)) = w.objects.get_mut(mech).map(|o| &mut o.kind) {
        m.left_arm_blown = false;
        m.right_arm_blown = false;
    }
    let disabled = w.objects.get(mech).is_some_and(|o| o.c.is_disabled());
    if disabled {
        if let Some(o) = w.objects.get_mut(mech) {
            o.c.set_status(ObjectStatus::ShutDown, true);
        }
    }
    let (home_team, home_commander) = (w.teams.home as i32, crate::combat::HOME_COMMANDER);
    trade_mover(w, mech, home_team, home_commander, Some(&pilot))
}

/// `TacticalOrder::status` for `Recover`: the recovery vehicle drives to the mech, recovers it for the home team and is spent.
pub fn status_recover(
    w: &mut World,
    me: ObjectHandle,
    order: &mut TacticalOrder,
    target: Option<ObjectHandle>,
) -> i32 {
    let now = w.clock.scenario_time;
    let Some(t) = target else {
        return TACORDER_SUCCESS;
    };
    if recover_buddy(w, me) == WatchId::NONE || recover_buddy(w, t) == WatchId::NONE {
        return TACORDER_SUCCESS;
    }
    let Some(general) = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map(|t| t.mover_general.clone())
    else {
        return TACORDER_FAILURE;
    };
    let mut result = TACORDER_FAILURE;
    match order.stage {
        1 => {
            // move
            if crate::ai::mover_ops::distance_from(w, me, &position_of(w, t))
                < general.recover_range
            {
                order.stage += 1;
            }
        }
        2 => {
            // the recovered mech joins the home team
            match take_over(w, t) {
                Ok(()) => {
                    let (my_wid, t_wid) = (w.objects.watch_id(me), w.objects.watch_id(t));
                    set_recover_buddy(w, t, my_wid);
                    set_recover_buddy(w, me, t_wid);
                    order.stage += 1;
                }
                Err(_) => {
                    // no pilot to give it: the order is over, nothing was changed, and the host is told
                    w.note_frame_unavailable(
                        "recovery: the campaign has no pilot for the recovered mech",
                    );
                    order.stage = -1;
                }
            }
        }
        3 => {
            if order.time <= 0.0 {
                power_down(w, t, order);
                order.time = now;
            } else if now > order.time + 3.0 {
                order.stage += 1;
                order.time = now;
            }
        }
        4 => {
            // do the deed
            set_recovering(w, me, true);
            if now > order.time + general.refit_time {
                let done = rf::recover(w, t);
                rf::burn_vehicle_recover_points(w, me, 0.0);
                order.stage += i8::from(done);
                if !done {
                    order.time = now;
                }
            }
        }
        5 => {
            // done: power-up and report; the recovery vehicle is used up
            set_recovering(w, me, false);
            power_up(w, t, order);
            result = TACORDER_SUCCESS;
            set_recover_buddy(w, t, WatchId::NONE);
            set_recover_buddy(w, me, WatchId::NONE);
            crate::combat::damage::disable(w, me, death::POWER_USED_UP);
        }
        -1 => {
            set_recovering(w, me, false);
            result = TACORDER_SUCCESS;
        }
        _ => {}
    }
    result
}
