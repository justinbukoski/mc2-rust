// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The movement chain of `GroundVehicle::update` (gvehicl.cpp:1453-2262, 2814-3025, 3178-3700): `updateAIControl`, `updateMovement`,
//! `pivotTo`, `updateMovePath`, `setNextMovePath`, `setControlSettings`, `updateMoveStateGoal`, `updateTurret` and `updateDynamics`, in
//! the order `update` calls them. The crash avoidance and the path locks are shared with the mechs ([`super::mech`]).
//!
//! Mine sweepers, mine layers and a mine under a vehicle are in [`super::support`].

use super::mech::{crash_avoidance, update_path_lock};
use super::MoverDecisionUnavailable;
use crate::ai::moveorders as mo;
use crate::ai::warrior;
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::object::{Kind, ObjectStatus};
use crate::World;
use mc2_sim_data::objtype::ObjectTypeData;

/// `vehicleTurnRate[long(tonnage)]` (gvehicl.cpp:175).
fn vehicle_turn_rate(tonnage: f32) -> f32 {
    match (tonnage as i64).clamp(0, 109) {
        0..=9 => 360.0,
        10..=59 => 240.0,
        60..=69 => 210.0,
        70..=79 => 140.0,
        80..=89 => 120.0,
        _ => 90.0,
    }
}

fn pilot_slot(w: &World, h: ObjectHandle) -> Option<u16> {
    match &w.objects.get(h)?.kind {
        Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    }
}

fn vehicle_mut(w: &mut World, h: ObjectHandle) -> Option<&mut crate::mover::VehicleState> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::Vehicle(v) => Some(v),
        _ => None,
    }
}

fn moves(w: &mut World, slot: u16) -> Option<&mut mo::MoveOrders> {
    w.pilots.get_mut(slot).map(|p| &mut p.lane_b_move)
}

/// `dynamics.max.groundVehicle` of the vehicle's type.
fn dynamics(w: &World, h: ObjectHandle) -> Option<mc2_sim_data::mech::vehicle::VehicleDynamics> {
    match w.objects.type_of(h).map(|t| &t.data) {
        Some(ObjectTypeData::GroundVehicle(v)) => Some(v.dynamics),
        _ => None,
    }
}

/// `Mover::getFireArc()`.
fn fire_arc(w: &World, h: ObjectHandle) -> f32 {
    match w.data.gamesys.tables.as_ref() {
        Some(gs) => crate::ai::decide::fire_arc(w, h, gs),
        None => 45.0,
    }
}

fn margins(w: &World) -> [f32; 2] {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or([5.0, 10.0], |t| t.pathfinding.margin_of_error)
}

/// The repair truck runs its deploy/repair/close gestures from its buddy and distance, independently of the refit order's
/// three-second power-down wait (`gvehicl.cpp:3065-3150`). The repair hum is requested only while in gesture 4
/// (`gvehicl.cpp:3754-3757`). Vehicle animation rendering is not part of this state; the loaded gesture metadata supplies
/// the same transition timing to the sound event.
fn update_refit_gesture(w: &mut World, h: ObjectHandle) {
    let Some((buddy, position, disabled)) = w.objects.get(h).and_then(|obj| match &obj.kind {
        Kind::Vehicle(v) => Some((v.m.refit_buddy, obj.c.position, obj.c.is_disabled())),
        _ => None,
    }) else {
        return;
    };
    if disabled {
        if let Some(Kind::Vehicle(vehicle)) = w.objects.get_mut(h).map(|obj| &mut obj.kind) {
            vehicle.refit_gesture = 0;
            vehicle.refit_gesture_frame = 0.0;
        }
        return;
    }
    let buddy_position = w.objects.get_by_watch_id(buddy).map(|obj| obj.c.position);
    let distance = buddy_position.map(|p| {
        crate::math::approximate_length(&Vec3 {
            x: p.x - position.x,
            y: p.y - position.y,
            z: p.z - position.z,
        })
    });
    let has_buddy = buddy_position.is_some();
    let appearance_name = w
        .objects
        .type_of(h)
        .and_then(|ty| ty.base.appearance_name.as_deref())
        .unwrap_or("APC");
    let animation = w
        .appearance
        .vehicle_types
        .iter()
        .find(|ty| ty.name.eq_ignore_ascii_case(appearance_name))
        .or_else(|| {
            w.appearance
                .vehicle_types
                .iter()
                .find(|ty| ty.name.eq_ignore_ascii_case("APC"))
        });
    let animation_timing = |gesture| {
        animation.map_or((0, 0.0), |ty| {
            (ty.num_frames(gesture), ty.frame_rate(gesture))
        })
    };
    let frame_length = w.clock.frame_length;
    let Some(Kind::Vehicle(vehicle)) = w.objects.get_mut(h).map(|obj| &mut obj.kind) else {
        return;
    };
    match vehicle.refit_gesture {
        0 if has_buddy && distance.is_some_and(|d| d <= 128.0) => {
            vehicle.refit_gesture = 3;
            vehicle.refit_gesture_frame = 0.0;
        }
        3 => {
            let (frames, rate) = animation_timing(3);
            if frames > 0 && rate > 0.0 {
                vehicle.refit_gesture_frame += frame_length * rate;
                if vehicle.refit_gesture_frame >= (frames - 1) as f32 {
                    vehicle.refit_gesture = 4;
                    vehicle.refit_gesture_frame = (frames - 1) as f32;
                }
            }
        }
        4 if !has_buddy => {
            vehicle.refit_gesture = 5;
            vehicle.refit_gesture_frame = 0.0;
        }
        5 => {
            let (frames, rate) = animation_timing(5);
            if frames > 0 && rate > 0.0 {
                vehicle.refit_gesture_frame += frame_length * rate;
                if vehicle.refit_gesture_frame >= (frames - 1) as f32 {
                    vehicle.refit_gesture = if has_buddy { 3 } else { 0 };
                    vehicle.refit_gesture_frame = 0.0;
                }
            }
        }
        _ => {}
    }
    let repairing = vehicle.refit_gesture == 4 && has_buddy;
    if repairing {
        crate::events::emit_sound(w, h, crate::events::sfx::REPAIRBAY_FX, false);
    }
}

/// Advance the destroyed vehicle's one-shot explosion timer (gvehicl.cpp:3307-3381).
pub fn update_destroyed_vehicle(
    w: &mut World,
    h: ObjectHandle,
) -> Result<(), MoverDecisionUnavailable> {
    let dt = w.clock.frame_length;
    let Some(obj) = w.objects.get(h) else {
        return Ok(());
    };
    let Kind::Vehicle(v) = &obj.kind else {
        return Ok(());
    };
    if v.m.time_left >= 0.0
        && v.m.time_left - dt < 0.0
        && crate::object::hooks::play_destruction(w, h)
    {
        return Err(MoverDecisionUnavailable::VehicleDestructionEffect);
    }
    let Some(obj) = w.objects.get_mut(h) else {
        return Ok(());
    };
    let Kind::Vehicle(v) = &mut obj.kind else {
        return Ok(());
    };
    if v.m.time_left < 0.0 {
        return Ok(());
    }
    obj.c.set_tangible(false);
    v.m.time_left -= dt;
    if v.m.time_left >= 0.0 {
        return Ok(());
    }
    let at = obj.c.position;
    let path_locks = v.m.path_locks_enabled;
    v.m.flags.exploding = true;
    let (damage, radius) = match w.objects.type_of(h).map(|t| &t.data) {
        Some(ObjectTypeData::GroundVehicle(ty)) => (ty.explosion_damage, ty.explosion_radius),
        _ => return Ok(()),
    };
    if !crate::object::hooks::play_destruction(w, h) {
        let effect = if damage > 0.0 {
            46
        } else if path_locks {
            40
        } else if w.rng.0.borrow_mut().visual.roll_dice(25) {
            82
        } else {
            55
        };
        let (damage, radius) = if damage > 0.0 {
            (damage, radius)
        } else {
            (0.0, 0.0)
        };
        crate::object::ObjectStore::create_explosion(w, effect, None, at, damage, radius);
        let rotation = w.rng.0.borrow_mut().visual.random_number(180) as f32;
        w.events.push(crate::SimEvent::Crater {
            kind: crate::crater::CRATER_4,
            at,
            rotation,
        });
    }
    // Create a Marine with the Vehicle Brain to run away from vehicle (single player only; gvehicl.cpp:3414-3419)
    if w.objects.get(h).is_some_and(|o| o.c.awake())
        && crate::mission::spawn::create_vehicle_pilot(w, h).is_err()
    {
        w.note_frame_unavailable("createVehiclePilot: the crew unit could not be created");
    }
    Ok(())
}

/// The non-destroyed branch of `GroundVehicle::update` (gvehicl.cpp:3178-3700), up to the appearance.
pub fn update_live_vehicle(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
) -> Result<(), MoverDecisionUnavailable> {
    update_refit_gesture(w, h);
    update_path_lock(w, h, false);

    // control.update(this)
    update_ai_control(w, h, slot)?;

    let mut emergency_stop = false;
    if !w.objects.get(h).is_some_and(|o| o.c.is_disabled()) {
        emergency_stop = crash_avoidance(w, h)?;
    }

    let distance = w.move_scratch.distance_to_waypoint;
    super::integrate_vehicle_motion(w, h, emergency_stop, Some(distance), false);
    let new_rotation = w.move_scratch.new_rotation;
    update_turret(w, h, slot, new_rotation);
    update_dynamics(w, h);
    if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        let vel = v.m.velocity;
        v.m.mark_distance_moved += crate::math::approximate_length(&vel);
    }
    if !w.objects.get(h).is_some_and(|o| o.c.is_disabled()) {
        update_path_lock(w, h, true);
    }
    let frame_length = w.clock.frame_length;
    if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        v.sweep_time += frame_length;
    }
    super::support::vehicle_mine_check(w, h, slot);
    super::support::lay_mine(w, h, slot);
    // glue the vehicle to the terrain
    let z = w.objects.get(h).map(|o| w.terrain.elevation(&o.c.position));
    let level = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Vehicle(v)) => v.m.move_level,
        _ => 0,
    };
    let water_depth = crate::object::hooks::altitude_range(w).2;
    if let (Some(mut z), Some(obj)) = (z, w.objects.get_mut(h)) {
        if level == 1 && z < water_depth {
            z = water_depth;
        }
        obj.c.position.z = z;
    }
    Ok(())
}

/// `MoverControl::reset()` for a ground vehicle (mover.cpp:1705): the throttle goes to zero as well.
fn control_reset(w: &mut World, h: ObjectHandle) {
    if let Some(v) = vehicle_mut(w, h) {
        v.control.rotate = 0.0;
        v.control.rotate_torso = 0.0;
        v.control.gesture_goal = -1;
        v.control.pivot = false;
        v.control.is_walking = false;
        v.control.throttle = 0;
    }
}

/// `GroundVehicle::updateAIControl()` (gvehicl.cpp:2814).
fn update_ai_control(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
) -> Result<(), MoverDecisionUnavailable> {
    control_reset(w, h);
    let Some(obj) = w.objects.get(h) else {
        return Ok(());
    };
    if !obj.c.awake() {
        return Ok(());
    }
    let disabled = obj.c.is_disabled();
    let team_id = obj.team_id();
    let (shut_down_this_frame, disable_this_frame) = match &obj.kind {
        Kind::Vehicle(v) => (v.m.flags.shut_down_this_frame, v.m.flags.disable_this_frame),
        _ => return Ok(()),
    };
    // `turretBlownThisFrame = false` and `updateDamageTakenRate()` belong to the damage code (lane C).
    let alive = w.pilots.get(slot).is_some_and(|p| p.wounds < 6.0);
    if !disabled && alive {
        if team_id > -1 {
            let _status = crate::ai::pilot_update(w, h);
        }
        update_movement(w, h)?;
    } else if shut_down_this_frame || disable_this_frame {
        update_movement(w, h)?;
    }
    Ok(())
}

/// `GroundVehicle::updateMovement()` (gvehicl.cpp:2185).
fn update_movement(w: &mut World, h: ObjectHandle) -> Result<(), MoverDecisionUnavailable> {
    let slot = pilot_slot(w, h).unwrap_or(0);
    w.move_scratch.new_rotation = 0.0;
    let Some(obj) = w.objects.get(h) else {
        return Ok(());
    };
    let Kind::Vehicle(v) = &obj.kind else {
        return Ok(());
    };
    let (disable_this_frame, shut_down_this_frame, start_up_this_frame) = (
        v.m.flags.disable_this_frame,
        v.m.flags.shut_down_this_frame,
        v.m.flags.start_up_this_frame,
    );
    // `engineBlowTime` stays -1 for a ground vehicle
    let engine_blow = -1.0_f32;
    let disabled = obj.c.is_disabled();
    if disable_this_frame {
        if let Some(obj) = w.objects.get_mut(h) {
            if let Kind::Vehicle(v) = &mut obj.kind {
                v.m.flags.disable_this_frame = false;
                v.m.flags.shut_down_this_frame = false;
                v.m.flags.start_up_this_frame = false;
                v.control.throttle = 0;
            }
            obj.c.set_status(ObjectStatus::Disabled, false);
        }
        return Ok(());
    }
    if shut_down_this_frame {
        if let Some(obj) = w.objects.get_mut(h) {
            if let Kind::Vehicle(v) = &mut obj.kind {
                v.m.flags.shut_down_this_frame = false;
                v.m.flags.start_up_this_frame = false;
                v.control.throttle = 0;
            }
            obj.c.set_status(ObjectStatus::ShutDown, false);
        }
        // sound.snd labels sample 108 Power Down; legacy constant names are
        // inverted. Emit only after the movement chain accepts the transition.
        crate::events::emit_sound(w, h, crate::events::sfx::VEHICLE_POWER_DOWN_SAMPLE, false);
        return Ok(());
    }
    // Do the vehicles have any form of "starting up"?
    if start_up_this_frame {
        if let Some(obj) = w.objects.get_mut(h) {
            if let Kind::Vehicle(v) = &mut obj.kind {
                v.m.flags.start_up_this_frame = false;
                v.control.throttle = 100;
            }
            obj.c.set_status(ObjectStatus::Normal, false);
        }
        // sound.snd labels sample 109 Power Up; see the legacy name note above.
        crate::events::emit_sound(w, h, crate::events::sfx::VEHICLE_POWER_UP_SAMPLE, false);
        return Ok(());
    }
    if disabled {
        if let Some(v) = vehicle_mut(w, h) {
            v.control.throttle = 0;
        }
        return Ok(());
    }
    if engine_blow > -1.0 {
        return Ok(());
    }
    if pivot_to(w, h, slot) {
        return Ok(());
    }

    let min_throttle = -100;
    let mut max_throttle = 100;
    // `calcThrottleLimits`: the incline is fixed at 90 degrees ("NOT WORKING RIGHT"), so the limits stay
    let mut new_rotate = 0.0;
    let mut new_throttle: i32 = 0;
    let mut new_move_state = -1;
    let goal_reached = update_move_path(
        w,
        h,
        slot,
        &mut new_rotate,
        &mut new_throttle,
        &mut new_move_state,
        &mut max_throttle,
    );
    if goal_reached {
        set_next_move_path(w, slot, &mut new_throttle);
    }
    if new_move_state != -1 {
        if let Some(m) = moves(w, slot) {
            m.move_state = new_move_state;
        }
    }
    set_control_settings(
        w,
        h,
        slot,
        new_rotate,
        new_throttle,
        min_throttle,
        max_throttle,
    );
    update_move_state_goal(w, h, slot);
    w.move_scratch.new_rotation = new_rotate;
    Ok(())
}

/// `GroundVehicle::pivotTo()` (gvehicl.cpp:1453): true when the vehicle pivots in place this frame.
fn pivot_to(w: &mut World, h: ObjectHandle, slot: u16) -> bool {
    // If we have no maxVelocity, we are an artillery piece. DO NOT PIVOT!!
    if dynamics(w, h).is_none_or(|d| d.speed == 0.0) {
        return false;
    }
    let _ = crate::ai::get_move_path(w, slot);
    let Some(pilot) = w.pilots.get(slot) else {
        return false;
    };
    let path = pilot.lane_b_move.paths[0].clone();
    let move_state = pilot.lane_b_move.move_state;
    let move_state_goal = pilot.lane_b_move.move_state_goal;
    let is_running = path.num_steps_when_not_paused > 0 && pilot.lane_b_move.run;
    let attack_point_order = pilot.cur_order.code == crate::command::OrderCode::AttackPoint;
    let attack_target_point = pilot.attack_orders.target_point;
    let tonnage = w.objects.get(h).map_or(0.0, |o| o.c.tonnage);

    let mut has_target = false;
    let mut target_position = Vec3::default();
    let mut rel_facing_to_target = 0.0;
    if let Some(t) = warrior::get_current_target(w, slot) {
        target_position = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
        rel_facing_to_target = crate::ai::decide::rel_facing_to(w, h, &target_position, -1);
        has_target = true;
    } else if attack_point_order {
        target_position = attack_target_point;
        rel_facing_to_target = crate::ai::decide::rel_facing_to(w, h, &target_position, -1);
        has_target = true;
    }
    let frame = w.clock.frame_length;
    let max_rate = vehicle_turn_rate(tonnage) * frame;
    let pivot_direction = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Vehicle(v)) => v.m.pivot_direction,
        _ => -1,
    };
    let set_pivot_direction = |w: &mut World, d: i8| {
        if let Some(v) = vehicle_mut(w, h) {
            v.m.pivot_direction = d;
        }
    };
    let pivot_turn = |w: &mut World, turn_rate: f32| {
        if let Some(v) = vehicle_mut(w, h) {
            v.control.rotate = turn_rate;
            v.control.pivot = true;
        }
        w.move_scratch.new_rotation = turn_rate;
    };
    let zero_throttle = |w: &mut World| {
        if let Some(v) = vehicle_mut(w, h) {
            v.control.throttle = 0;
        }
    };

    if move_state == mo::MOVESTATE_PIVOT_FORWARD {
        if move_state_goal == mo::MOVESTATE_PIVOT_FORWARD
            || move_state_goal == mo::MOVESTATE_FORWARD
        {
            // We want to pivot forward (if we have a path to follow)...
            if path.num_steps_when_not_paused > 0 && path.cur_step < path.num_steps_when_not_paused
            {
                let way_pt = path.steps[path.cur_step].destination;
                zero_throttle(w);
                let rel = crate::ai::decide::rel_facing_to(w, h, &way_pt, -1);
                if !(-45.0..=45.0).contains(&rel) {
                    let mut turn_rate = -rel;
                    if has_target && !is_running {
                        let mut direction = pivot_direction;
                        if direction == -1 {
                            direction = if rel_facing_to_target < 0.0 { 0 } else { 1 };
                            set_pivot_direction(w, direction);
                        }
                        turn_rate = if direction == 0 {
                            if rel < 0.0 {
                                -rel
                            } else {
                                360.0 - rel
                            }
                        } else if rel < 0.0 {
                            -360.0 - rel
                        } else {
                            -rel
                        };
                    }
                    if turn_rate.abs() > max_rate {
                        turn_rate = if turn_rate > 0.0 { max_rate } else { -max_rate };
                    }
                    pivot_turn(w, turn_rate);
                    return true;
                }
                if let Some(m) = moves(w, slot) {
                    m.move_state = mo::MOVESTATE_FORWARD;
                    if m.twisting {
                        m.twisting = false;
                    }
                }
            } else if let Some(m) = moves(w, slot) {
                m.move_state_goal = mo::MOVESTATE_FORWARD;
            }
        } else if let Some(m) = moves(w, slot) {
            m.move_state = mo::MOVESTATE_FORWARD;
        }
    } else if move_state == mo::MOVESTATE_PIVOT_REVERSE {
        if move_state_goal == mo::MOVESTATE_PIVOT_REVERSE
            || move_state_goal == mo::MOVESTATE_REVERSE
        {
            if path.num_steps_when_not_paused > 0 && path.cur_step < path.num_steps_when_not_paused
            {
                let way_pt = path.steps[path.cur_step].destination;
                zero_throttle(w);
                let rel = crate::ai::decide::rel_facing_to(w, h, &way_pt, -1);
                if rel > -175.0 && rel < 175.0 {
                    let mut turn_rate;
                    if has_target && !is_running {
                        let mut direction = pivot_direction;
                        if direction == -1 {
                            direction = if rel_facing_to_target < 0.0 { 0 } else { 1 };
                            set_pivot_direction(w, direction);
                        }
                        turn_rate = if direction == 0 {
                            180.0 - rel
                        } else {
                            -180.0 - rel
                        };
                    } else if rel < 0.0 {
                        turn_rate = -180.0 - rel;
                    } else {
                        turn_rate = 180.0 - rel;
                    }
                    if turn_rate.abs() > max_rate {
                        turn_rate = if turn_rate > 0.0 { max_rate } else { -max_rate };
                    }
                    pivot_turn(w, turn_rate);
                    return true;
                }
                // Facing reverse. Now what?
                if let Some(m) = moves(w, slot) {
                    if m.twisting {
                        m.twisting = false;
                    }
                    if move_state_goal == mo::MOVESTATE_REVERSE {
                        m.move_state = mo::MOVESTATE_REVERSE;
                    } else {
                        m.move_state_goal = mo::MOVESTATE_FORWARD;
                    }
                }
            } else if let Some(m) = moves(w, slot) {
                m.move_state_goal = mo::MOVESTATE_FORWARD;
            }
        } else if let Some(m) = moves(w, slot) {
            m.move_state = mo::MOVESTATE_FORWARD;
        }
    } else if move_state == mo::MOVESTATE_PIVOT_TARGET {
        if move_state_goal == mo::MOVESTATE_PIVOT_TARGET {
            // We want to pivot to our current target...
            if is_running || !has_target {
                if let Some(m) = moves(w, slot) {
                    m.move_state_goal = mo::MOVESTATE_FORWARD;
                }
                if !crate::ai::is_yielding(w, slot) && !is_waiting_for_point(w, slot) {
                    crate::ai::resume_path(w, slot);
                }
                set_pivot_direction(w, -1);
                return false;
            }
            zero_throttle(w);
            let rel_to_target = crate::ai::decide::rel_facing_to(w, h, &target_position, -1);
            let arc = fire_arc(w, h);
            if rel_to_target < -arc || rel_to_target > arc {
                let mut turn_rate = -rel_to_target;
                if turn_rate.abs() > max_rate {
                    turn_rate = if turn_rate > 0.0 { max_rate } else { -max_rate };
                }
                pivot_turn(w, turn_rate);
                return true;
            }
            if let Some(m) = moves(w, slot) {
                m.move_state_goal = mo::MOVESTATE_FORWARD;
            }
        } else if let Some(m) = moves(w, slot) {
            m.move_state = mo::MOVESTATE_FORWARD;
        }
    } else if matches!(
        move_state_goal,
        mo::MOVESTATE_PIVOT_TARGET | mo::MOVESTATE_PIVOT_FORWARD | mo::MOVESTATE_PIVOT_REVERSE
    ) {
        // If we're supposed to be pivoting, set the move state...
        if let Some(m) = moves(w, slot) {
            m.move_state = move_state_goal;
        }
    }

    if !crate::ai::is_yielding(w, slot) && !is_waiting_for_point(w, slot) {
        crate::ai::resume_path(w, slot);
    }
    set_pivot_direction(w, -1);
    false
}

fn is_waiting_for_point(w: &World, slot: u16) -> bool {
    w.pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.is_waiting_for_point())
}

/// `GroundVehicle::updateMovePath(...)` (gvehicl.cpp:1781): returns `goalReached`.
fn update_move_path(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    new_rotate: &mut f32,
    new_throttle: &mut i32,
    new_move_state: &mut i32,
    max_throttle: &mut i32,
) -> bool {
    w.move_scratch.distance_to_waypoint = 9999.0;
    let _ = crate::ai::get_move_path(w, slot);
    let (cur_throttle, velocity_mag, tonnage) = match w.objects.get(h) {
        Some(obj) => match &obj.kind {
            Kind::Vehicle(v) => (i32::from(v.control.throttle), v.velocity_mag, obj.c.tonnage),
            _ => return false,
        },
        None => return false,
    };
    let now = w.clock.scenario_time;
    let frame = w.clock.frame_length;
    *new_throttle = cur_throttle;
    *new_rotate = 0.0;

    // `getPoint()` is NULL in the shipped build (USE_GROUPS is not defined): no point vehicle to wait for.
    if let Some(m) = moves(w, slot) {
        m.wait_for_point_time = -1.0;
    }

    let mut goal_reached = false;
    let Some(path) = w.pilots.get(slot).map(|p| p.lane_b_move.paths[0].clone()) else {
        return false;
    };
    if path.num_steps > 0 {
        // First, make sure we are not already at the end of this path...
        if path.cur_step == path.num_steps {
            goal_reached = true;
        } else {
            let mut way_pt = path.steps[path.cur_step].destination;
            // Let's record our last valid position...
            if let Some(v) = vehicle_mut(w, h) {
                v.m.last_valid_position = way_pt;
            }
            // Have we reached the destination?
            let distance_from_way_pt = crate::ai::mover_ops::distance_from(w, h, &way_pt);
            w.move_scratch.distance_to_waypoint = distance_from_way_pt;
            // Calculate how far the vehicle will move this frame. Vel is in m/s
            let distance_this_frame = velocity_mag * frame;
            let margin = margins(w);
            let mut cushion = margin[0];
            if path.cur_step == path.num_steps - 1 {
                cushion = margin[1];
            }
            if cushion < distance_this_frame {
                cushion = distance_this_frame;
            }
            if distance_from_way_pt < cushion {
                // Reached it, so go to the next waypoint...
                let cur_step = path.cur_step + 1;
                if let Some(m) = moves(w, slot) {
                    m.paths[0].cur_step = cur_step;
                    m.time_of_last_step = now;
                }
                if cur_step < path.num_steps {
                    way_pt = path.steps[cur_step].destination;
                } else {
                    goal_reached = true;
                }
            }

            if !goal_reached {
                // First, rotate the vehicle (if moving)...
                let rel = crate::ai::decide::rel_facing_to(w, h, &way_pt, -1);
                let (move_state, move_state_goal) = {
                    let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
                    (m.move_state, m.move_state_goal)
                };
                let max_rate = vehicle_turn_rate(tonnage) * frame;
                let pause = |w: &mut World| crate::ai::pause_path(w, slot);
                if move_state == mo::MOVESTATE_FORWARD {
                    if move_state_goal == mo::MOVESTATE_FORWARD {
                        // Keep going forward...
                        *new_throttle = 100;
                        if !(-5.0..=5.0).contains(&rel) {
                            // We can and will shift facing to destination...
                            *new_rotate = -rel;
                            if new_rotate.abs() > max_rate {
                                *new_throttle = if new_rotate.abs() < 50.0 {
                                    (100.0 - new_rotate.abs()) as i32
                                } else {
                                    0
                                };
                                *new_rotate = if *new_rotate > 0.0 {
                                    max_rate
                                } else {
                                    -max_rate
                                };
                            }
                        }
                    } else if move_state_goal == mo::MOVESTATE_REVERSE
                        || move_state_goal == mo::MOVESTATE_PIVOT_REVERSE
                    {
                        pause(w);
                        *new_move_state = mo::MOVESTATE_PIVOT_REVERSE;
                    } else if move_state_goal == mo::MOVESTATE_PIVOT_FORWARD {
                        // Stop, and pivot fully forward to next path step...
                        pause(w);
                        *new_move_state = mo::MOVESTATE_PIVOT_FORWARD;
                    } else {
                        // Stop...
                        pause(w);
                        *new_move_state = mo::MOVESTATE_FORWARD;
                    }
                } else if move_state == mo::MOVESTATE_REVERSE {
                    if move_state_goal == mo::MOVESTATE_FORWARD
                        || move_state_goal == mo::MOVESTATE_PIVOT_FORWARD
                    {
                        pause(w);
                        *new_move_state = mo::MOVESTATE_PIVOT_FORWARD;
                    } else if move_state_goal == mo::MOVESTATE_REVERSE {
                        *new_throttle = -100;
                        // Keep moving in reverse...
                        *new_rotate = if rel < 0.0 {
                            -(rel + 180.0)
                        } else {
                            -(rel - 180.0)
                        };
                        if new_rotate.abs() > max_rate {
                            *new_throttle = if new_rotate.abs() < 50.0 {
                                (-50.0 + new_rotate.abs()) as i32
                            } else {
                                0
                            };
                            *new_rotate = if *new_rotate > 0.0 {
                                max_rate
                            } else {
                                -max_rate
                            };
                        } else {
                            // Try to attain current goal speed...
                            *new_throttle = -100;
                        }
                    } else if move_state_goal == mo::MOVESTATE_PIVOT_REVERSE {
                        pause(w);
                        *new_move_state = mo::MOVESTATE_PIVOT_REVERSE;
                    } else {
                        pause(w);
                        *new_move_state = mo::MOVESTATE_FORWARD;
                    }
                } else if move_state_goal == mo::MOVESTATE_FORWARD
                    || move_state_goal == mo::MOVESTATE_PIVOT_FORWARD
                {
                    // Not moving--should we be?
                    pause(w);
                    *new_move_state = mo::MOVESTATE_PIVOT_FORWARD;
                } else if move_state_goal == mo::MOVESTATE_REVERSE
                    || move_state_goal == mo::MOVESTATE_PIVOT_REVERSE
                {
                    pause(w);
                    *new_move_state = mo::MOVESTATE_PIVOT_REVERSE;
                }
            }
        }
    } else {
        // We better not be walking or running, then!
        *new_throttle = 0;
    }

    // if we're a sweeping minesweeper or a laying minelayer, take it slow
    if let Some(throttle) = super::support::mine_throttle(w, h, slot) {
        *max_throttle = throttle;
    }

    if goal_reached {
        // Did we finish just a local part of a global path?
        let (path_type, global_step, num_global_steps) = {
            let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
            (m.path_type, m.paths[0].global_step, m.num_global_steps)
        };
        if path_type == mo::MOVEPATH_COMPLEX && global_step < num_global_steps - 1 {
            goal_reached = false;
        }
        if let Some(m) = moves(w, slot) {
            mo::clear_move_path(&mut m.paths[0]);
        }
        *new_throttle = 0;
    }
    goal_reached
}

/// `GroundVehicle::setNextMovePath(...)` (gvehicl.cpp:2098).
fn set_next_move_path(w: &mut World, slot: u16, new_throttle: &mut i32) {
    if let Some(next) = warrior::get_next_way_point(w, slot, true) {
        warrior::set_move_goal(w, slot, mo::MOVEGOAL_LOCATION, Some(next));
        let selection = w
            .pilots
            .get(slot)
            .map_or(0, |p| p.cur_order.selection_index);
        warrior::request_move_path(
            w,
            slot,
            selection,
            mc2_path::movemap::MOVEPARAM_FACE_TARGET + mc2_path::movemap::MOVEPARAM_FOLLOW_ROADS,
            0,
        );
    } else {
        warrior::clear_move_orders(w, slot);
        *new_throttle = 0;
    }
}

/// `GroundVehicle::setControlSettings(...)` (gvehicl.cpp:2121).
fn set_control_settings(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    new_rotate: f32,
    new_throttle: i32,
    min_throttle: i32,
    max_throttle: i32,
) {
    // If we have no move path, then we shouldn't be moving. Let's set our throttle setting to zero, just to be safe...
    let _ = crate::ai::get_move_path(w, slot);
    let (steps, run) = w.pilots.get(slot).map_or((0, false), |p| {
        (p.lane_b_move.paths[0].num_steps, p.lane_b_move.run)
    });
    let mut throttle = new_throttle;
    if steps == 0 {
        throttle = 0;
    }
    if throttle != -1 {
        throttle = throttle.clamp(min_throttle, max_throttle);
        if let Some(v) = vehicle_mut(w, h) {
            v.control.throttle = throttle as i8;
        }
    }
    if let Some(v) = vehicle_mut(w, h) {
        if new_rotate != 0.0 {
            v.control.rotate = new_rotate;
        }
        v.control.is_walking = !run;
    }
}

/// `GroundVehicle::updateMoveStateGoal()` (gvehicl.cpp:1707).
fn update_move_state_goal(w: &mut World, h: ObjectHandle, slot: u16) {
    let mut target_position = Vec3::default();
    let mut has_target = false;
    if let Some(t) = warrior::get_last_target(w, slot) {
        target_position = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
        has_target = true;
    } else if let Some(p) = w.pilots.get(slot) {
        if p.cur_order.code == crate::command::OrderCode::AttackPoint {
            target_position = p.attack_orders.target_point;
            has_target = true;
        }
    }
    let _ = crate::ai::get_move_path(w, slot);
    let Some(pilot) = w.pilots.get(slot) else {
        return;
    };
    let path = pilot.lane_b_move.paths[0].clone();
    let move_state_goal = pilot.lane_b_move.move_state_goal;
    let run = pilot.lane_b_move.run;
    let twisting = pilot.lane_b_move.twisting;
    if path.num_steps > 0 {
        if run {
            if let Some(m) = moves(w, slot) {
                m.move_state_goal = mo::MOVESTATE_FORWARD;
            }
            return;
        }
        // Facing choice for movement goes here...
        if has_target {
            // Check if we want to go forward or in reverse...
            if path.num_steps_when_not_paused > 0 && path.cur_step < path.num_steps_when_not_paused
            {
                let way_pt = path.steps[path.cur_step].destination;
                let facing_delta = rel_facing_delta(w, h, &way_pt, &target_position);
                let turret_yaw = dynamics(w, h).map_or(0.0, |d| d.turret_yaw as f32);
                let total_fire_arc = fire_arc(w, h) + turret_yaw;
                if move_state_goal == mo::MOVESTATE_FORWARD {
                    if facing_delta > total_fire_arc
                        && (180.0 - facing_delta) <= total_fire_arc
                        && !twisting
                    {
                        if let Some(m) = moves(w, slot) {
                            m.twisting = true;
                            m.move_state_goal = mo::MOVESTATE_REVERSE;
                        }
                    }
                } else if (180.0 - facing_delta) > total_fire_arc
                    && facing_delta <= total_fire_arc
                    && !twisting
                {
                    if let Some(m) = moves(w, slot) {
                        m.twisting = true;
                        m.move_state_goal = mo::MOVESTATE_FORWARD;
                    }
                }
            }
        } else if let Some(m) = moves(w, slot) {
            m.move_state_goal = mo::MOVESTATE_FORWARD;
        }
    } else if !matches!(
        move_state_goal,
        mo::MOVESTATE_PIVOT_TARGET | mo::MOVESTATE_PIVOT_FORWARD | mo::MOVESTATE_PIVOT_REVERSE
    ) && !has_target
        && path.num_steps_when_not_paused == 0
    {
        if let Some(m) = moves(w, slot) {
            m.move_state_goal = mo::MOVESTATE_FORWARD;
        }
    }
}

/// `Mover::relFacingDelta(goalPos, targetPos)` (mover.cpp:2875).
fn rel_facing_delta(w: &World, h: ObjectHandle, goal: &Vec3, target: &Vec3) -> f32 {
    let to_goal = crate::ai::decide::rel_facing_to(w, h, goal, -1);
    let to_target = crate::ai::decide::rel_facing_to(w, h, target, -1);
    let mut delta;
    if to_goal >= 0.0 {
        if to_target >= 0.0 {
            delta = if to_goal > to_target {
                to_goal - to_target
            } else {
                to_target - to_goal
            };
        } else {
            delta = to_goal - to_target;
            if delta > 180.0 {
                delta = 360.0 - delta;
            }
        }
    } else if to_target >= 0.0 {
        delta = to_target - to_goal;
        if delta > 180.0 {
            delta = 360.0 - delta;
        }
    } else {
        delta = if to_goal > to_target {
            to_goal - to_target
        } else {
            to_target - to_goal
        };
    }
    delta
}

/// `GroundVehicle::updateTurret(newRotatePerSec)` (gvehicl.cpp:2154).
fn update_turret(w: &mut World, h: ObjectHandle, slot: u16, new_rotate_per_sec: f32) {
    let turret_rotation = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Vehicle(v)) => v.turret_rotation,
        _ => return,
    };
    let target = warrior::get_current_target(w, slot);
    let attack_point = w.pilots.get(slot).and_then(|p| {
        (p.cur_order.code == crate::command::OrderCode::AttackPoint)
            .then_some(p.attack_orders.target_point)
    });
    let turret_rel_facing = if let Some(t) = target {
        let pos = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
        crate::ai::decide::rel_facing_to(w, h, &pos, -1) + turret_rotation + new_rotate_per_sec
    } else if let Some(p) = attack_point {
        crate::ai::decide::rel_facing_to(w, h, &p, -1) + turret_rotation + new_rotate_per_sec
    } else {
        turret_rotation
    };
    let arc = fire_arc(w, h);
    if turret_rel_facing < -arc || turret_rel_facing > arc {
        let mut new_rotate_turret = -turret_rel_facing;
        // We can and will shift facing to destination...
        let turret_yaw_rate = dynamics(w, h).map_or(0.0, |d| d.turret_yaw_rate as f32);
        let max_rate = turret_yaw_rate * w.clock.frame_length;
        if new_rotate_turret.abs() > max_rate {
            new_rotate_turret = if new_rotate_turret < 0.0 {
                -max_rate
            } else {
                max_rate
            };
        }
        if let Some(v) = vehicle_mut(w, h) {
            v.control.rotate_torso = new_rotate_turret;
        }
    }
}

/// `GroundVehicle::updateDynamics()` (gvehicl.cpp:2956): turret and body rotation, then the throttle's velocity ramp.
pub fn update_dynamics(w: &mut World, h: ObjectHandle) {
    let turret_yaw_limit = dynamics(w, h).map_or(0.0, |d| d.turret_yaw as f32);
    let frame = w.clock.frame_length;
    let walk_speed = w.data.gamesys.tables.as_ref().map(|g| g.vehicle.walk_speed);
    let Some(obj) = w.objects.get_mut(h) else {
        return;
    };
    if obj.c.is_destroyed() {
        return;
    }
    let Kind::Vehicle(v) = &mut obj.kind else {
        return;
    };
    let yaw_rate = v.control.rotate;
    let mut turret_yaw_rate = v.control.rotate_torso;

    // Rotate turret by turret yaw rate, making sure we don't turn too far.
    if turret_yaw_rate != 0.0 {
        let mut new_turret_rotation = v.turret_rotation;
        if new_turret_rotation > turret_yaw_limit {
            new_turret_rotation = turret_yaw_limit;
            turret_yaw_rate = 0.0;
        }
        if new_turret_rotation < -turret_yaw_limit {
            new_turret_rotation = -turret_yaw_limit;
            turret_yaw_rate = 0.0;
        }
        if new_turret_rotation + turret_yaw_rate > turret_yaw_limit {
            turret_yaw_rate = turret_yaw_limit - new_turret_rotation;
        }
        if new_turret_rotation + turret_yaw_rate < -turret_yaw_limit {
            turret_yaw_rate = -turret_yaw_limit - new_turret_rotation;
        }
        new_turret_rotation += turret_yaw_rate;
        v.turret_rotation = new_turret_rotation;
    }

    // rotate(yawRate)
    obj.c.rotation += yaw_rate;
    if obj.c.rotation < -180.0 {
        obj.c.rotation += 360.0;
    } else if obj.c.rotation > 180.0 {
        obj.c.rotation -= 360.0;
    }

    // Ok, real vehicle dynamics. Throttle controls maxVelocity. Rate of velocity change is constant.
    let Kind::Vehicle(v) = &mut obj.kind else {
        return;
    };
    let throttle = f32::from(v.control.throttle) / 100.0;
    let mut real_max_velocity = v.m.max_move_speed;
    if v.control.is_walking {
        real_max_velocity = walk_speed.unwrap_or(real_max_velocity);
    }
    let max_throttled_vel = throttle * real_max_velocity;
    let vel_diff = max_throttled_vel - v.velocity_mag;

    // If we're slowing down, slow us down!
    if (vel_diff < 0.0 && v.acceleration > 0.0) || (vel_diff > 0.0 && v.acceleration < 0.0) {
        v.acceleration = -v.acceleration;
    }
    let mut vel_change_this_frame = v.acceleration * frame;
    if vel_change_this_frame.abs() > vel_diff.abs() {
        vel_change_this_frame = vel_diff;
    }
    v.velocity_mag += vel_change_this_frame;
}

/// `GroundVehicle::updateAnimations` (gvehicl.cpp:3026-3158): the stand / walk / run / repair gesture state machine. Gestures
/// 3 (open up), 4 (repairing) and 5 (close up) follow a refit buddy within 128 units. The C++ skips it in multiplayer, which the
/// port does not have.
pub fn update_animations(w: &mut World, h: ObjectHandle) {
    let Some(obj) = w.objects.get(h) else { return };
    let Kind::Vehicle(v) = &obj.kind else { return };
    let (velocity, buddy, position) = (v.velocity_mag, v.m.refit_buddy, obj.c.position);
    let dead = obj.c.is_disabled();
    let half_speed = dynamics(w, h).map_or(0.0, |d| d.speed * 0.5);
    if dead {
        crate::appearance::set_gesture(w, h, 0); // "He's dead."
        return;
    }
    let near_buddy = !buddy.is_none()
        && w.objects
            .by_watch_id(buddy)
            .and_then(|b| w.objects.get(b))
            .is_some_and(|b| {
                let d = Vec3 {
                    x: b.c.position.x - position.x,
                    y: b.c.position.y - position.y,
                    z: b.c.position.z - position.z,
                };
                crate::math::approximate_length(&d) <= 128.0
            });
    let in_transition = w.appearance.in_transition(h);
    let mut goal: Option<u32> = None;
    match w.appearance.current_gesture_id(h) {
        -1 => {
            if !in_transition {
                goal = Some(0);
            }
        }
        2 => {
            if velocity == 0.0 {
                goal = Some(0);
            } else if velocity <= half_speed {
                goal = Some(1);
            }
            if near_buddy {
                goal = Some(3);
            }
        }
        0 => {
            if velocity > half_speed {
                goal = Some(2);
            } else if velocity != 0.0 && velocity <= half_speed {
                goal = Some(1);
            }
            if near_buddy {
                goal = Some(3);
            }
        }
        1 => {
            if velocity == 0.0 {
                goal = Some(0);
            } else if velocity > half_speed {
                goal = Some(2);
            }
            if near_buddy {
                goal = Some(3);
            }
        }
        3 => {
            if !in_transition {
                goal = Some(4);
            }
        }
        4 => {
            if buddy.is_none() {
                goal = Some(5); // start closing up
            }
        }
        5 if !in_transition => {
            goal = Some(if buddy.is_none() { 0 } else { 3 });
        }
        _ => {}
    }
    if let Some(g) = goal {
        crate::appearance::set_gesture(w, h, g);
    }
}
