// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The movement chain of `BattleMech::update` (mech.cpp:3813-4920, 5411-5500, 5506-5525, 5663-5775): `updateAIControl`,
//! `updateMovement`, `updateJump`, `pivotTo`, `updateMovePath`, `setNextMovePath`, `setControlSettings`, `updateMoveStateGoal`,
//! `updateTorso`, `updateDynamics` and `crashAvoidanceSystem`, in the order `update` calls them. All of it reads and writes the
//! pilot's `moveOrders` through the same accessors the pilot code uses.
//!
//! Jump flight, piloting checks and falls are ported, and so is the jump-landing contact with another mover (guard, bounce and
//! death-from-above, `mech.cpp:820-948`): see `mech_handle_collision` in `mover/mod.rs`.

use super::{MoverDecisionUnavailable, MAX_LOCK_RANGE};
use crate::ai::moveorders as mo;
use crate::ai::warrior;
use crate::appearance::tables::{body_state, state as mech_state};
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::object::{Kind, ObjectStatus};
use crate::World;

/// `LEG_STATUS_*` (mech.h:181).
mod leg {
    pub const NORMAL: u8 = 0;
    pub const HURTING: u8 = 1;
    pub const IMPAIRED_LEFT: u8 = 2;
    pub const IMPAIRED_RIGHT: u8 = 3;
}

/// `tonnageTurnRate[long(tonnage)]` (mech.cpp:264): 720 degrees per second for every tonnage.
const TONNAGE_TURN_RATE: f32 = 720.0;

/// `MECH_BODY_LOCATION_LLEG` (mech.h): no torso rotation is added to the facing.
const MECH_BODY_LOCATION_LLEG: i32 = 6;

/// What `updateMovePath` hands back through its reference parameters.
#[derive(Clone, Copy, Debug)]
struct Decision {
    new_rotate: f32,
    /// A C `char`; -1 means "no change".
    new_throttle: i32,
    new_gesture_goal: i32,
    new_move_state: i32,
    facing_rotate: f32,
}

impl Decision {
    fn new() -> Decision {
        Decision {
            new_rotate: 0.0,
            new_throttle: -1,
            new_gesture_goal: -1,
            new_move_state: -1,
            facing_rotate: 0.0,
        }
    }
}

fn pilot_slot(w: &World, h: ObjectHandle) -> Option<u16> {
    match &w.objects.get(h)?.kind {
        Kind::Mech(m) => Some(m.m.pilot_slot),
        Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    }
}

fn mech_mut(w: &mut World, h: ObjectHandle) -> Option<&mut crate::mover::MechState> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::Mech(m) => Some(m),
        _ => None,
    }
}

fn moves(w: &mut World, slot: u16) -> Option<&mut mo::MoveOrders> {
    w.pilots.get_mut(slot).map(|p| &mut p.lane_b_move)
}

/// `MovePath::getDirection(step)` of the current path.
fn step_direction(path: &mc2_path::MovePath, step: usize) -> u8 {
    path.steps.get(step).map_or(0, |s| s.direction)
}

/// `Mover::marginOfError[0..2]`.
fn margins(w: &World) -> [f32; 2] {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or([5.0, 10.0], |t| t.pathfinding.margin_of_error)
}

// ---------------------------------------------------------------------------------------------------------------------
// Path locks
// ---------------------------------------------------------------------------------------------------------------------

/// `Mover::setPathRangeLock(set, range)` (mover.cpp:5291-5340): the pathlock of the next `range` cells of the current path.
fn set_path_range_lock(w: &mut World, h: ObjectHandle, slot: u16, set: bool, range: i32) {
    let Some(obj) = w.objects.get(h) else { return };
    let level = match &obj.kind {
        Kind::Mech(m) => m.m.move_level,
        Kind::Vehicle(v) => v.m.move_level,
        _ => return,
    };
    let lock_level = u32::from(level == 2);
    // `MovePathPtr path = pilot->getMovePath();` runs first for both branches: its spare-path swap (an emptied path 0 takes path 1)
    // is a side effect of every `updatePathLock`, including the unlock before a path is recalculated.
    let _ = crate::ai::get_move_path(w, slot);
    if set {
        let previous = match &w.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Mech(m)) => m.m.path_lock.len,
            Some(Kind::Vehicle(v)) => v.m.path_lock.len,
            _ => 0,
        };
        if previous > 0 {
            set_path_range_lock(w, h, slot, false, 0);
        }
        let path = w.pilots.get(slot).map(|p| p.lane_b_move.paths[0].clone());
        let mut cells: Vec<(i32, i32)> = Vec::new();
        if let Some(path) = path.filter(|p| p.num_steps > 0) {
            let start = path.cur_step;
            let mut last_step = start as i64 + i64::from(range);
            if last_step >= path.num_steps_when_not_paused as i64 {
                last_step = path.num_steps_when_not_paused as i64;
            }
            let mut i = start as i64;
            while i < last_step {
                let Some(step) = path.steps.get(i as usize) else {
                    break;
                };
                let (r, c) = (step.cell[0], step.cell[1]);
                if w.terrain.in_bounds(r, c) && w.terrain.map().cell(r, c).pathlock(lock_level) {
                    break;
                }
                w.terrain.map_mut().set_pathlock(lock_level, r, c, true);
                if cells.len() < MAX_LOCK_RANGE {
                    cells.push((r, c));
                }
                i += 1;
            }
        }
        if let Some(obj) = w.objects.get_mut(h) {
            let core = match &mut obj.kind {
                Kind::Mech(m) => &mut m.m,
                Kind::Vehicle(v) => &mut v.m,
                _ => return,
            };
            core.path_lock.len = cells.len();
            for (dst, src) in core.path_lock.cells.iter_mut().zip(&cells) {
                *dst = *src;
            }
        }
    } else {
        let list = match w.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Mech(m)) => Some((m.m.path_lock.cells, m.m.path_lock.len)),
            Some(Kind::Vehicle(v)) => Some((v.m.path_lock.cells, v.m.path_lock.len)),
            _ => None,
        };
        if let Some((cells, len)) = list {
            for &(r, c) in cells.iter().take(len.min(MAX_LOCK_RANGE)) {
                w.terrain.map_mut().set_pathlock(lock_level, r, c, false);
            }
        }
        if let Some(obj) = w.objects.get_mut(h) {
            match &mut obj.kind {
                Kind::Mech(m) => m.m.path_lock.len = 0,
                Kind::Vehicle(v) => v.m.path_lock.len = 0,
                _ => {}
            }
        }
    }
}

/// `Mover::updatePathLock(set)` (mover.cpp:5342-5358).
pub fn update_path_lock(w: &mut World, h: ObjectHandle, set: bool) {
    let Some(obj) = w.objects.get(h) else { return };
    let (slot, level, cell, enabled, in_jump, range) = match &obj.kind {
        Kind::Mech(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1)),
            s.m.path_locks_enabled,
            s.in_jump,
            s.m.crash.block_path,
        ),
        Kind::Vehicle(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1)),
            s.m.path_locks_enabled,
            false,
            s.m.crash.block_path,
        ),
        _ => return,
    };
    if in_jump || !enabled {
        return;
    }
    w.terrain
        .map_mut()
        .set_pathlock(u32::from(level == 2), cell.0, cell.1, set);
    // If we're yielding, we should NOT set our path range on. If we are yielding, we can always remove what we've already pathlocked...
    if !set || !crate::ai::is_yielding(w, slot) {
        set_path_range_lock(w, h, slot, set, range);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Crash avoidance
// ---------------------------------------------------------------------------------------------------------------------

/// `adjClippedCell[8][2]` (mech.cpp:320).
const ADJ_CLIPPED_CELL: [[usize; 2]; 8] = [
    [0, 0],
    [0, 2],
    [2, 2],
    [2, 4],
    [4, 4],
    [4, 6],
    [6, 6],
    [6, 0],
];

/// `Mover::getAdjacentCellPathLocked(level, row, col, dir)` (mover.cpp:5268).
fn adjacent_cell_path_locked(w: &World, level: i32, row: i32, col: i32, dir: usize) -> bool {
    const ADJ: [[i32; 2]; 8] = [
        [-1, 0],
        [-1, 1],
        [0, 1],
        [1, 1],
        [1, 0],
        [1, -1],
        [0, -1],
        [-1, -1],
    ];
    if !(0..=1).contains(&level) {
        return false;
    }
    let (r, c) = (row + ADJ[dir][0], col + ADJ[dir][1]);
    w.terrain.in_bounds(r, c) && w.terrain.map().cell(r, c).pathlock(level as u32)
}

/// `BattleMech::crashAvoidanceSystem` (mech.cpp:5411) and `GroundVehicle::crashAvoidanceSystem` (gvehicl.cpp:2617): true when the
/// mover must stop this frame. The brake control is applied here.
pub fn crash_avoidance(w: &mut World, h: ObjectHandle) -> Result<bool, MoverDecisionUnavailable> {
    let Some(obj) = w.objects.get(h) else {
        return Ok(false);
    };
    let (slot, level, is_mech, cell) = match &obj.kind {
        Kind::Mech(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            true,
            (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1)),
        ),
        Kind::Vehicle(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            false,
            (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1)),
        ),
        _ => return Ok(false),
    };
    let crash_avoid_path = match &obj.kind {
        Kind::Mech(s) => s.m.crash.avoid_path,
        Kind::Vehicle(s) => s.m.crash.avoid_path,
        _ => 0,
    };
    let crash_yield_time = match &obj.kind {
        Kind::Mech(s) => s.m.crash.yield_time,
        Kind::Vehicle(s) => s.m.crash.yield_time,
        _ => 0.0,
    };
    // `pilot->getMovePath()` (with its spare-path swap)
    let Some(path) = crate::ai::get_move_path(w, slot) else {
        return Ok(false);
    };
    if path.num_steps_when_not_paused == 0 {
        return Ok(false);
    }
    let wait_for_point = w
        .pilots
        .get(slot)
        .map_or(-1.0, |p| p.lane_b_move.wait_for_point_time);
    if f64::from(wait_for_point) > 999_990.0 {
        return Ok(false);
    }

    // To avoid corner stops...
    let mut clipping_corner = false;
    let dir = usize::from(step_direction(&path, path.cur_step));
    if matches!(dir, 1 | 3 | 5 | 7) {
        let lock_level = i32::from(level == 2);
        let first =
            adjacent_cell_path_locked(w, lock_level, cell.0, cell.1, ADJ_CLIPPED_CELL[dir][0]);
        let second =
            adjacent_cell_path_locked(w, lock_level, cell.0, cell.1, ADJ_CLIPPED_CELL[dir][1]);
        clipping_corner = first && second;
    }

    let range = usize::try_from(crash_avoid_path).unwrap_or(0);
    let mut reached_end = false;
    let near = path.is_locked(
        w.terrain.map(),
        u32::from(level == 2),
        -1,
        range,
        Some(&mut reached_end),
    );
    let mut block_reached_end = false;
    // `getPathRangeBlocked`: ground movers only (the end flag is garbage for flyers; they are not ported)
    let blocked =
        level == 0 && path.is_blocked(w.terrain.map(), -1, range, Some(&mut block_reached_end));
    // `crossesClosedGate` always answers -1 in the shipped source (the overlay code is compiled out)
    let step_into_gate = false;

    let path_locked = near;
    let now = w.clock.scenario_time;
    let yielding = crate::ai::is_yielding(w, slot);
    if yielding {
        if path_locked || blocked || clipping_corner || step_into_gate {
            crate::ai::pause_path(w, slot);
            return Ok(true);
        }
        // It's clear, now...
        crate::ai::resume_path(w, slot);
        if let Some(m) = moves(w, slot) {
            m.set_yield_time(-1.0);
        }
        return Ok(false);
    }
    if path_locked || blocked || clipping_corner || step_into_gate {
        if reached_end || block_reached_end {
            // Since we're practically there, let's just kill the current move orders...
            crate::ai::rethink_path(w, h, 0);
        } else {
            crate::ai::pause_path(w, slot);
            if let Some(m) = moves(w, slot) {
                m.set_yield_time(now + crash_yield_time);
            }
        }
        brake(w, h, is_mech);
        return Ok(true);
    }
    Ok(false)
}

/// `MoverControl::brake()` (mover.cpp:1720).
fn brake(w: &mut World, h: ObjectHandle, is_mech: bool) {
    match w.objects.get_mut(h).map(|o| &mut o.kind) {
        Some(Kind::Mech(m)) if is_mech => {
            m.control.throttle = 100;
            m.control.gesture_goal = mech_state::STANDING as i8;
        }
        Some(Kind::Vehicle(v)) => v.control.throttle = 0,
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// update
// ---------------------------------------------------------------------------------------------------------------------

/// `SensorSystem::setShutdown(set)` of the mover's own sensor system (contact.cpp:169): the manager publishes the change to the team.
fn set_sensor_shutdown(w: &mut World, h: ObjectHandle, shutdown: bool) {
    if let Some(sensor) = w.sensors.pool.iter_mut().find(|s| s.owner == h) {
        sensor.not_shutdown = !shutdown;
    }
}

/// `MoverControl::update(this)` for an AI mover = `BattleMech::updateAIControl()` (mech.cpp:5506).
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
    // `updateDamageTakenRate()`: the damage rate alarm needs the damage tally of the combat lane; without damage it only moves the
    // check time.
    let disabled = obj.c.is_disabled();
    let (shut_down_this_frame, disable_this_frame, team_id) = match &obj.kind {
        Kind::Mech(m) => (
            m.m.flags.shut_down_this_frame,
            m.m.flags.disable_this_frame,
            obj.team_id(),
        ),
        _ => return Ok(()),
    };
    let alive = w.pilots.get(slot).is_some_and(|p| p.wounds < 6.0);
    let ejected = w.pilots.get(slot).is_some_and(|p| {
        matches!(
            p.status,
            crate::mission::outcome::WarriorStatus::Ejected
                | crate::mission::outcome::WarriorStatus::Mia
                | crate::mission::outcome::WarriorStatus::Captured
        )
    });
    if !disabled && alive && !ejected {
        if team_id > -1 {
            let _status = crate::ai::pilot_update(w, h);
        }
        update_movement(w, h)?;
    } else if shut_down_this_frame || disable_this_frame {
        update_movement(w, h)?;
    }
    Ok(())
}

/// The non-destroyed, non-disabled branch of `BattleMech::update` (mech.cpp:5845-6075), up to the appearance bounds.
pub fn update_live_mech(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
) -> Result<(), MoverDecisionUnavailable> {
    update_path_lock(w, h, false);

    // Check if we are SHUTDOWN and NOT onGui. If we are both, we are waiting to be linked up with...
    let waiting_for_link = w.objects.get(h).is_some_and(|obj| {
        obj.c.status == ObjectStatus::ShutDown
            && !matches!(&obj.kind, Kind::Mech(m) if m.m.flags.on_gui)
    });
    if waiting_for_link {
        shutdown_link_up(w, h)?;
    }

    update_ai_control(w, h, slot)?;

    let mut emergency_stop = false;
    if !w.objects.get(h).is_some_and(|o| o.c.is_disabled()) {
        let ram = crate::ai::calcpath::ram_object(w, slot);
        if let Some(r) = ram {
            update_path_lock(w, r, false);
        }
        let result = crash_avoidance(w, h);
        if let Some(r) = ram {
            update_path_lock(w, r, true);
        }
        emergency_stop = result?;
    }

    // At this point, the other updates have completed, its time to apply the velocity and rotations to the mech.
    let status = w
        .objects
        .get(h)
        .map_or(ObjectStatus::Normal, |o| o.c.status);
    let distance = w.move_scratch.distance_to_waypoint;
    super::integrate_mech_motion(w, h, emergency_stop, Some(distance), false);
    let new_rotation = w.move_scratch.new_rotation;
    update_torso(w, h, slot, new_rotation);
    // Don't let them move if they are shutdown!!
    if status != ObjectStatus::ShutDown {
        update_dynamics(w, h);
    }
    if let Some(Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        let v = m.m.velocity;
        m.m.mark_distance_moved += crate::math::approximate_length(&v);
    }
    if !w.objects.get(h).is_some_and(|o| o.c.is_disabled()) {
        update_path_lock(w, h, true);
    }
    super::support::mech_mine_check(w, h, slot);
    let z = w.objects.get(h).map(|o| w.terrain.elevation(&o.c.position));
    if let (Some(z), Some(obj)) = (z, w.objects.get_mut(h)) {
        obj.c.position.z = z;
    }
    Ok(())
}

/// `Team::home->teamLineOfSight(...)` of the "waiting to be linked up with" block (mech.cpp:5849): a shut down mover that nobody on
/// the home team can see powers up and joins the home roster (`mech.cpp:5849`).
fn shutdown_link_up(w: &mut World, h: ObjectHandle) -> Result<(), MoverDecisionUnavailable> {
    if crate::object::hooks::home_team_sees(w, h) {
        let slot = pilot_slot(w, h).unwrap_or(0);
        crate::mission::spawn::set_on_gui(w, h, true);
        let _ = crate::ai::orders::core_power(w, slot, true);
        if let Some(sensor) = w.sensors.pool.iter_mut().find(|s| s.owner == h) {
            sensor.broken = false;
        }
        return Ok(());
    }
    if let Some(sensor) = w.sensors.pool.iter_mut().find(|s| s.owner == h) {
        sensor.broken = true;
    }
    Ok(())
}

/// The arm blow-off at the top of `BattleMech::update` (mech.cpp:5812-5836), regardless of the mech's status: a left arm that was
/// blown this frame asks for the right arm to go ("Frank says, blow the right one anyway") and the other way round, then each
/// flagged arm is blown off the appearance (shoulder pop, flying arm, smoke).
pub fn blow_arms(w: &mut World, h: ObjectHandle) {
    let Some(m) = mech_mut(w, h) else { return };
    if m.left_arm_blown {
        m.left_arm_blown = false;
        m.control.blow_right_arm = true;
    }
    if m.right_arm_blown {
        m.right_arm_blown = false;
        m.control.blow_left_arm = true;
    }
    let (left, right) = (m.control.blow_left_arm, m.control.blow_right_arm);
    for (flag, is_left) in [(left, true), (right, false)] {
        if !flag {
            continue;
        }
        crate::appearance::fx::sync_preference(w, h);
        {
            let mut streams = w.rng.0.borrow_mut();
            w.appearance.blow_arm(h, is_left, &mut streams.visual);
        }
        crate::appearance::fx::flush(w, h);
        if let Some(m) = mech_mut(w, h) {
            if is_left {
                m.control.blow_left_arm = false;
            } else {
                m.control.blow_right_arm = false;
            }
        }
    }
}

/// `appearance->setBrake(...)`, `setCombatMode(...)` and the tangible flag after `recalcBounds` (mech.cpp:6082-6106).
pub fn update_appearance_flags(w: &mut World, h: ObjectHandle, slot: u16) {
    let path = crate::ai::get_move_path(w, slot);
    let brake = !path.is_some_and(|p| p.num_steps > 0 && p.cur_step < p.num_steps);
    w.appearance.set_brake(h, brake);
    let target = warrior::get_current_target(w, slot).is_some();
    let combat = target
        || w.pilots.get(slot).is_some_and(|p| {
            matches!(
                p.cur_order.code,
                crate::command::OrderCode::AttackObject | crate::command::OrderCode::AttackPoint
            )
        });
    w.appearance.set_combat_mode(h, combat);
    let jumping = matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(m)) if m.in_jump);
    if let Some(obj) = w.objects.get_mut(h) {
        if jumping {
            obj.c.set_tangible(false);
        } else if !obj.c.is_disabled() {
            obj.c.set_tangible(true);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// updateMovement
// ---------------------------------------------------------------------------------------------------------------------

/// `MoverControl::reset()` for a mech (mover.cpp:1684): the per-frame controls back to nothing, the throttle kept.
pub fn control_reset(w: &mut World, h: ObjectHandle) {
    if let Some(m) = mech_mut(w, h) {
        m.control.rotate = 0.0;
        m.control.facing_rotate = 0.0;
        m.control.rotate_torso = 0.0;
        m.control.rotate_arms = (0.0, 0.0);
        m.control.gesture_goal = -1;
        m.control.blow_left_arm = false;
        m.control.blow_right_arm = false;
        m.control.pivot = false;
    }
}

/// `BattleMech::updateMovement()` (mech.cpp:4774).
pub fn update_movement(w: &mut World, h: ObjectHandle) -> Result<(), MoverDecisionUnavailable> {
    let slot = pilot_slot(w, h).unwrap_or(0);
    let min_throttle = 35;
    let max_throttle = 100;
    w.move_scratch.new_rotation = 0.0;

    let Some(obj) = w.objects.get(h) else {
        return Ok(());
    };
    let Kind::Mech(mech) = &obj.kind else {
        return Ok(());
    };
    let (disable_this_frame, shut_down_this_frame, start_up_this_frame) = (
        mech.m.flags.disable_this_frame,
        mech.m.flags.shut_down_this_frame,
        mech.m.flags.start_up_this_frame,
    );
    let (team_id, move_level) = (i32::from(mech.m.team.0), mech.m.move_level);
    let (hit_from_front, hit_from_behind) = (mech.hit_from_front, mech.hit_from_behind);
    let engine_blow = mech.engine_blow_time;
    let failed_piloting_check = mech.failed_piloting_check;
    let status = obj.c.status;
    let air = matches!(
        w.objects.type_of(h).map(|t| &t.data),
        Some(mc2_sim_data::objtype::ObjectTypeData::BattleMech(m))
            if m.move_type == crate::combat::artillery_math::MOVETYPE_AIR
    );
    if status == ObjectStatus::Disabled {
        update_path_lock(w, h, false);
    }

    if disable_this_frame {
        let mut fall_gesture = if w.rng.0.borrow_mut().sim.random_number(2) != 0 {
            mech_state::FALLEN_FORWARD
        } else {
            mech_state::FALLEN_BACKWARD
        };
        if hit_from_behind {
            fall_gesture = mech_state::FALLEN_FORWARD;
        } else if hit_from_front {
            fall_gesture = mech_state::FALLEN_BACKWARD;
        }
        crate::appearance::set_gesture_goal(w, h, fall_gesture);
        // If we just switched to running, we need to make sure the throttle is set to max...
        if let Some(m) = mech_mut(w, h) {
            m.m.flags.disable_this_frame = false;
            m.m.flags.shut_down_this_frame = false;
            m.m.flags.start_up_this_frame = false;
            m.hit_from_front = false;
            m.hit_from_behind = false;
            m.control.throttle = max_throttle as i8;
        }
        return Ok(());
    }
    if shut_down_this_frame {
        // Confusing, but is used to display the helicopter dust cloud (mech.cpp:4808).
        if air {
            crate::appearance::fx::play_ejection(w, h);
        }
        crate::appearance::set_gesture_goal(w, h, mech_state::PARKED);
        if let Some(obj) = w.objects.get_mut(h) {
            if let Kind::Mech(m) = &mut obj.kind {
                m.m.flags.shut_down_this_frame = false;
                m.m.flags.start_up_this_frame = false;
                m.control.throttle = max_throttle as i8;
            }
            obj.c.set_status(ObjectStatus::ShutDown, false);
        }
        set_sensor_shutdown(w, h, true);
        let (team, move_level) = w.objects.get(h).map_or((-1, 0), |obj| match &obj.kind {
            Kind::Mech(m) => (obj.team_id(), m.m.move_level),
            _ => (-1, 0),
        });
        // C++ requests the sample when movement consumes the transition flag,
        // after the disable-priority path. Enemy shutdown audio is suppressed
        // during the opening eight seconds of the mission.
        if team == w.teams.home as i32 || w.clock.scenario_time >= 8.0 {
            let id = if move_level == 2 {
                crate::events::sfx::COPTER_POWER_DOWN_SAMPLE
            } else {
                crate::events::sfx::POWERDOWN_SFX
            };
            crate::events::emit_sound(w, h, id, false);
        }
        return Ok(());
    }

    let gesture_id = w.appearance.current_gesture_id(h);
    let powering_up = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.code == crate::command::OrderCode::PowerUp);
    if powering_up {
        if gesture_id != 2 {
            crate::appearance::set_gesture_goal(w, h, mech_state::STANDING);
        } else {
            // Otherwise, we were ordered to powerUp and we are. Clear it!
            warrior::clear_cur_tac_order(w, slot, true);
        }
    }
    if start_up_this_frame {
        if air {
            crate::appearance::fx::play_ejection(w, h);
        }
        if let Some(obj) = w.objects.get_mut(h) {
            if let Kind::Mech(m) = &mut obj.kind {
                m.m.flags.start_up_this_frame = false;
                m.m.flags.shut_down_this_frame = false;
                m.control.throttle = max_throttle as i8;
            }
            obj.c.set_status(ObjectStatus::Normal, false);
        }
        if move_level != 2
            && crate::object::hooks::team_relation(w, team_id, crate::object::hooks::home_team(w))
                == Some(crate::ai::Relation::Enemy)
        {
            w.events
                .push(crate::SimEvent::Betty(crate::events::betty::POWERUP));
        }
        set_sensor_shutdown(w, h, false);
        let move_level = mech_mut(w, h).map_or(0, |m| m.m.move_level);
        let id = if move_level == 2 {
            crate::events::sfx::COPTER_POWER_UP_SAMPLE
        } else {
            crate::events::sfx::POWERUP_SFX
        };
        crate::events::emit_sound(w, h, id, false);
        return Ok(());
    }
    if matches!(
        status,
        ObjectStatus::ShuttingDown | ObjectStatus::ShutDown | ObjectStatus::Disabled
    ) {
        return Ok(());
    }
    // `isCaptured()` (a captured mech is not owned by its team) and a blown engine stop the mech
    if engine_blow > -1.0 {
        return Ok(());
    }
    // mech.cpp:4871: an air mover (MoveType AIR) never takes the fall branch; its failed check stays set and it moves on.
    if failed_piloting_check && !air {
        handle_failed_piloting_check(w, h, slot);
        return Ok(());
    }
    if update_jump(w, h, slot)? {
        return Ok(());
    }
    if pivot_to(w, h, slot)? {
        return Ok(());
    }

    let mut d = Decision::new();
    let goal_reached = update_move_path(w, h, slot, &mut d, min_throttle, max_throttle)?;
    if goal_reached {
        set_next_move_path(w, slot, &mut d);
    }
    if d.new_move_state != -1 {
        if let Some(m) = moves(w, slot) {
            m.move_state = d.new_move_state;
        }
    }
    set_control_settings(w, h, slot, &mut d, min_throttle, max_throttle)?;
    update_move_state_goal(w, h, slot);
    w.move_scratch.new_rotation = d.new_rotate;
    Ok(())
}

/// `BattleMech::updateJump()` (mech.cpp:3813-3907): takeoff, airborne pause, and landing.
fn update_jump(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
) -> Result<bool, MoverDecisionUnavailable> {
    let in_jump = matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(m)) if m.in_jump);
    if !in_jump {
        return Ok(false);
    }
    if w.appearance.is_jump_setup(h) {
        crate::appearance::set_gesture_goal(w, h, mech_state::JUMPING);
        if let Some(m) = mech_mut(w, h) {
            m.control.throttle = 100;
        }
        return Ok(true);
    }
    if w.appearance.is_jump_airborne(h) {
        if let Some(m) = mech_mut(w, h) {
            m.control.rotate = 0.0;
        }
        return Ok(true);
    }
    if !w.appearance.is_in_jump(h) {
        let pos = w.objects.get(h).map_or(Vec3::default(), |o| o.c.position);
        let now = w.clock.scenario_time;
        if let Some(m) = mech_mut(w, h) {
            m.in_jump = false;
            m.last_jump_time = now;
            m.m.last_valid_position = pos;
        }
        crate::ai::resume_path(w, slot);
        if let Some(p) = w.pilots.get_mut(slot) {
            let path = &mut p.lane_b_move.paths[0];
            path.cur_step = (path.cur_step + 1).min(path.num_steps);
            p.cur_order.stage = 3;
        }
        piloting_check(w, h, slot, false);
    }
    Ok(true)
}

/// `BattleMech::pilotingCheck()` (mech.cpp:3498-3565). The shipped `#if 0` excludes the jumping skill branch.
fn piloting_check(w: &mut World, h: ObjectHandle, slot: u16, collision: bool) {
    if matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(m)) if m.failed_piloting_check) {
        return;
    }
    let mut roll = w.rng.0.borrow_mut().sim.random_number(100) as f32;
    if collision {
        roll += 20.0;
    }
    let (leg_failed, gyro_health, gyro_master_id, failed_hip_actuators) =
        match w.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Mech(m)) => {
                let leg_failed = [6usize, 7].into_iter().any(|i| {
                    m.loadout
                        .body
                        .get(i)
                        .is_some_and(|b| b.cur_internal_structure == 0.0)
                });
                let gyro = m.loadout.inventory.get(usize::from(m.loadout.stats.gyro));
                let gyro_master_id = gyro.map_or(-1, |v| i32::from(v.master_id));
                let failed_hip_actuators = failed_hip_actuator_count(&m.loadout) as f32;
                (
                    leg_failed,
                    gyro.map_or(0, |v| v.health),
                    gyro_master_id,
                    failed_hip_actuators,
                )
            }
            _ => return,
        };
    if leg_failed {
        roll += 100.0;
    }
    let gyro_max = w
        .data
        .components
        .as_ref()
        .and_then(|cs| cs.get(gyro_master_id))
        .map_or(0, |c| c.health);
    if gyro_health == 0 {
        roll += 100.0;
    } else if gyro_health < gyro_max {
        roll += 30.0;
    }
    roll += failed_hip_actuators * 10.0;
    let index = mc2_sim_data::pilot::Skill::Piloting.index();
    let skill = w.pilots.get(slot).map_or(0, |p| p.skills[index]);
    let failed = roll >= skill as f32;
    let (try_points, success_points) =
        w.data.gamesys.tables.as_ref().map_or((0.0, 0.0), |tables| {
            (
                tables.skills.skill_try[index],
                tables.skills.skill_success[index],
            )
        });
    if let Some(p) = w.mission.pilot_loads.get_mut(usize::from(slot)) {
        p.skill_points[index] += try_points;
        if !failed {
            p.skill_points[index] += success_points;
        }
    }
    if let Some(m) = mech_mut(w, h) {
        m.failed_piloting_check = failed;
    }
}

fn failed_hip_actuator_count(loadout: &crate::combat::Loadout) -> usize {
    [loadout.stats.actuator[2], loadout.stats.actuator[3]]
        .into_iter()
        .filter(|i| {
            usize::from(*i) < loadout.inventory.len()
                && loadout.inventory[usize::from(*i)].health == 0
        })
        .count()
}

/// Select a fall direction once; the appearance completes the fall and stand-up gestures.
fn handle_failed_piloting_check(w: &mut World, h: ObjectHandle, _slot: u16) {
    if w.appearance.have_fallen(h) {
        if let Some(m) = mech_mut(w, h) {
            m.failed_piloting_check = false;
            m.fallen = true;
        }
        return;
    }
    let (front, behind) = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => (m.hit_from_front, m.hit_from_behind),
        _ => return,
    };
    // mech.cpp:4872: `fallGesture = RandomNumber(2) ? FORWARD : BACKWARD` always draws; the hit direction then overrides it.
    let random_forward = w.rng.0.borrow_mut().sim.random_number(2) != 0;
    let gesture = if behind {
        mech_state::FALLEN_FORWARD
    } else if front {
        mech_state::FALLEN_BACKWARD
    } else if random_forward {
        mech_state::FALLEN_FORWARD
    } else {
        mech_state::FALLEN_BACKWARD
    };
    // mech.cpp:4874-4879 clears `failedPilotingCheck` after ONE `setGestureGoal` whether or not the appearance took it (a refused goal
    // means no fall, including the knockdown of a failed landing check while the jump goal is still current: the C++ drops it).
    // Keeping the check pending redrew `RandomNumber(2)` every following frame and desynchronised the sim stream.
    crate::appearance::set_gesture_goal(w, h, gesture);
    if let Some(m) = mech_mut(w, h) {
        m.control.throttle = 100;
        m.failed_piloting_check = false;
    }
}

/// `BattleMech::pivotTo()` (mech.cpp:3915): true when the mech pivots in place this frame.
fn pivot_to(w: &mut World, h: ObjectHandle, slot: u16) -> Result<bool, MoverDecisionUnavailable> {
    let _ = crate::ai::get_move_path(w, slot);
    let Some(pilot) = w.pilots.get(slot) else {
        return Ok(false);
    };
    let path = pilot.lane_b_move.paths[0].clone();
    let move_state = pilot.lane_b_move.move_state;
    let move_state_goal = pilot.lane_b_move.move_state_goal;
    let is_running = path.num_steps_when_not_paused > 0 && pilot.lane_b_move.run;
    let attack_point_order = pilot.cur_order.code == crate::command::OrderCode::AttackPoint;
    let attack_target_point = pilot.attack_orders.target_point;

    let mut has_target = false;
    let mut rel_facing_to_target = 0.0;
    if let Some(t) = warrior::get_current_target(w, slot) {
        let pos = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
        rel_facing_to_target = crate::ai::decide::rel_facing_to(w, h, &pos, -1);
        has_target = true;
    } else if attack_point_order {
        rel_facing_to_target = crate::ai::decide::rel_facing_to(w, h, &attack_target_point, -1);
        has_target = true;
    }
    let frame = w.clock.frame_length;
    let max_rate = TONNAGE_TURN_RATE * frame;
    let pivot_direction = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.m.pivot_direction,
        _ => -1,
    };
    let set_pivot_direction = |w: &mut World, v: i8| {
        if let Some(m) = mech_mut(w, h) {
            m.m.pivot_direction = v;
        }
    };
    let pivot_turn = |w: &mut World, turn_rate: f32| {
        if let Some(m) = mech_mut(w, h) {
            m.control.rotate = turn_rate;
            m.control.pivot = true;
        }
        w.move_scratch.new_rotation = turn_rate;
    };
    let stand_full_throttle = |w: &mut World| {
        crate::appearance::set_gesture_goal(w, h, mech_state::STANDING);
        if let Some(m) = mech_mut(w, h) {
            m.control.throttle = 100;
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
                stand_full_throttle(w);
                let rel = crate::ai::decide::rel_facing_to(w, h, &way_pt, -1);
                if !(-5.0..=5.0).contains(&rel) {
                    let mut turn_rate = -rel;
                    let mut max = max_rate;
                    if turn_rate < 0.0 {
                        max = -max;
                    }
                    if turn_rate.abs() > max {
                        turn_rate = if turn_rate > 0.0 { max } else { -max };
                    }
                    pivot_turn(w, turn_rate);
                    return Ok(true);
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
                stand_full_throttle(w);
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
                    return Ok(true);
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
                return Ok(false);
            }
            stand_full_throttle(w);
            let fire_arc = fire_arc(w, h);
            if rel_facing_to_target < -fire_arc || rel_facing_to_target > fire_arc {
                let mut turn_rate = -rel_facing_to_target;
                if turn_rate.abs() > max_rate {
                    turn_rate = if turn_rate > 0.0 { max_rate } else { -max_rate };
                }
                pivot_turn(w, turn_rate);
                return Ok(true);
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
    Ok(false)
}

fn is_waiting_for_point(w: &World, slot: u16) -> bool {
    w.pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.is_waiting_for_point())
}

/// `Mover::getFireArc()`.
fn fire_arc(w: &World, h: ObjectHandle) -> f32 {
    match w.data.gamesys.tables.as_ref() {
        Some(gs) => crate::ai::decide::fire_arc(w, h, gs),
        None => 45.0,
    }
}

/// `BattleMech::updateMovePath(...)` (mech.cpp:4232): returns `goalReached`.
fn update_move_path(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    d: &mut Decision,
    _min_throttle: i32,
    _max_throttle: i32,
) -> Result<bool, MoverDecisionUnavailable> {
    w.move_scratch.distance_to_waypoint = 9999.0;
    let _ = crate::ai::get_move_path(w, slot);
    let Some(obj) = w.objects.get(h) else {
        return Ok(false);
    };
    let Kind::Mech(mech) = &obj.kind else {
        return Ok(false);
    };
    let leg_status = mech.leg_status.0;
    let can_run = leg_status == leg::NORMAL;
    let cur_throttle = i32::from(mech.control.throttle);
    let tonnage_rate = TONNAGE_TURN_RATE;
    let now = w.clock.scenario_time;
    let frame = w.clock.frame_length;
    let Some(pilot) = w.pilots.get(slot) else {
        return Ok(false);
    };
    let run = pilot.lane_b_move.run;
    let allowed_to_run = can_run && run;
    d.new_throttle = cur_throttle;
    d.new_rotate = 0.0;

    // `getPoint()` is NULL in the shipped build (USE_GROUPS is not defined): no point vehicle to wait for.
    let _ = allowed_to_run;
    if let Some(m) = moves(w, slot) {
        m.wait_for_point_time = -1.0;
    }

    let mut goal_reached = false;
    if matches!(
        leg_status,
        leg::NORMAL | leg::HURTING | leg::IMPAIRED_LEFT | leg::IMPAIRED_RIGHT
    ) {
        let path = w.pilots.get(slot).map(|p| p.lane_b_move.paths[0].clone());
        let Some(path) = path else { return Ok(false) };
        if path.num_steps > 0 {
            // First, make sure we are not already at the end of this path...
            if path.cur_step == path.num_steps {
                goal_reached = true;
            } else {
                let mut way_pt = path.steps[path.cur_step].destination;
                // Let's record our last valid position...
                if let Some(obj) = w.objects.get_mut(h) {
                    if let Kind::Mech(m) = &mut obj.kind {
                        m.m.last_valid_position = way_pt;
                    }
                }
                // Have we reached the destination?
                let distance_from_way_pt = crate::ai::mover_ops::distance_from(w, h, &way_pt);
                w.move_scratch.distance_to_waypoint = distance_from_way_pt;
                // Calculate how far the mech will move this frame.
                let vel = w.appearance.velocity_magnitude(h);
                let distance_this_frame = vel * frame;
                let margin = margins(w);
                let mut cushion = margin[0];
                if path.cur_step == path.num_steps - 1 {
                    cushion = margin[1];
                }
                if cushion < distance_this_frame {
                    cushion = distance_this_frame;
                }
                let mut cur_step = path.cur_step;
                if distance_from_way_pt < cushion {
                    // Reached it, so go to the next waypoint...
                    cur_step += 1;
                    if let Some(m) = moves(w, slot) {
                        m.paths[0].cur_step = cur_step;
                        m.time_of_last_step = now;
                    }
                    if cur_step < path.num_steps {
                        if path.steps[cur_step].direction > 7 {
                            // The destination is installed by `set_control_settings` below.
                            d.new_gesture_goal = mech_state::JUMPING;
                            return Ok(false);
                        }
                        way_pt = path.steps[cur_step].destination;
                    } else {
                        goal_reached = true;
                    }
                } else if step_direction(&path, cur_step) > 7 {
                    // Not there yet. Should we be jumping?
                    d.new_gesture_goal = mech_state::JUMPING;
                    return Ok(false);
                }

                if !goal_reached {
                    // First, rotate the mech's body (if moving)...
                    let rel_facing_to_way_pt =
                        crate::ai::decide::rel_facing_to(w, h, &way_pt, MECH_BODY_LOCATION_LLEG);
                    let (move_state, move_state_goal) = {
                        let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
                        (m.move_state, m.move_state_goal)
                    };
                    move_state_transition(
                        w,
                        h,
                        slot,
                        d,
                        move_state,
                        move_state_goal,
                        rel_facing_to_way_pt,
                        leg_status,
                        allowed_to_run,
                        tonnage_rate * frame,
                    );
                }
            }
        } else {
            // We better not be walking or running, then!
            d.new_gesture_goal = mech_state::STANDING;
        }
    } else {
        // Cannot move, period!
        d.new_gesture_goal = mech_state::STANDING;
    }

    if goal_reached {
        // Did we finish just a local part of a global path?
        let (path_type, global_step, num_global_steps) = {
            let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
            (m.path_type, m.paths[0].global_step, m.num_global_steps)
        };
        if path_type == mo::MOVEPATH_COMPLEX && global_step < num_global_steps - 1 {
            // We've reached the end of our current short-range path, but we have more to go...
            goal_reached = false;
        }
        if let Some(m) = moves(w, slot) {
            mo::clear_move_path(&mut m.paths[0]);
        }
    }
    Ok(goal_reached)
}

/// The `moveState` / `moveStateGoal` block of `updateMovePath` (mech.cpp:4393-4560).
#[allow(clippy::too_many_arguments)]
fn move_state_transition(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    d: &mut Decision,
    move_state: i32,
    move_state_goal: i32,
    rel_facing_to_way_pt: f32,
    leg_status: u8,
    allowed_to_run: bool,
    max_rate: f32,
) {
    let pause = |w: &mut World| crate::ai::pause_path(w, slot);
    // `calcMoveSpeedThrottle(...)` always answers 100 in MC2
    let attain_goal_speed = 100;
    if move_state == mo::MOVESTATE_FORWARD {
        if move_state_goal == mo::MOVESTATE_FORWARD {
            // Keep going forward...
            d.new_gesture_goal = if leg_status == leg::IMPAIRED_LEFT {
                mech_state::LIMPING_LEFT
            } else if leg_status == leg::IMPAIRED_RIGHT {
                mech_state::LIMPING_RIGHT
            } else if allowed_to_run {
                mech_state::RUNNING
            } else {
                mech_state::WALKING
            };
            if !(-5.0..=5.0).contains(&rel_facing_to_way_pt) {
                // We can and will shift facing to destination...
                d.new_rotate = -rel_facing_to_way_pt;
                if d.new_rotate.abs() > max_rate {
                    if d.new_rotate.abs() < 50.0 {
                        d.new_throttle = (100.0 - d.new_rotate.abs()) as i32;
                    } else {
                        d.new_throttle = 0;
                    }
                    d.new_rotate = if d.new_rotate > 0.0 {
                        max_rate
                    } else {
                        -max_rate
                    };
                } else {
                    // Try to attain current goal speed...
                    d.new_throttle = attain_goal_speed;
                }
            } else {
                d.new_throttle = 100;
            }
        } else if move_state_goal == mo::MOVESTATE_REVERSE
            || move_state_goal == mo::MOVESTATE_PIVOT_REVERSE
        {
            // Stop, and pivot fully reverse to next path step...
            pause(w);
            d.new_move_state = mo::MOVESTATE_PIVOT_REVERSE;
        } else if move_state_goal == mo::MOVESTATE_PIVOT_FORWARD {
            pause(w);
            d.new_move_state = mo::MOVESTATE_PIVOT_FORWARD;
        } else {
            // Stop...
            pause(w);
            d.new_move_state = mo::MOVESTATE_FORWARD;
        }
    } else if move_state == mo::MOVESTATE_REVERSE {
        if move_state_goal == mo::MOVESTATE_FORWARD
            || move_state_goal == mo::MOVESTATE_PIVOT_FORWARD
        {
            pause(w);
            d.new_move_state = mo::MOVESTATE_PIVOT_FORWARD;
        } else if move_state_goal == mo::MOVESTATE_REVERSE {
            d.new_gesture_goal = mech_state::REVERSE;
            // Keep moving in reverse...
            d.new_rotate = if rel_facing_to_way_pt < 0.0 {
                -(rel_facing_to_way_pt + 180.0)
            } else {
                -(rel_facing_to_way_pt - 180.0)
            };
            if d.new_rotate.abs() > max_rate {
                if d.new_rotate.abs() < 50.0 {
                    d.new_throttle = (100.0 - d.new_rotate.abs()) as i32;
                } else {
                    d.new_throttle = 0;
                }
                d.new_rotate = if d.new_rotate > 0.0 {
                    max_rate
                } else {
                    -max_rate
                };
            } else {
                d.new_throttle = attain_goal_speed;
            }
        } else if move_state_goal == mo::MOVESTATE_PIVOT_REVERSE {
            pause(w);
            d.new_move_state = mo::MOVESTATE_PIVOT_REVERSE;
        } else {
            pause(w);
            d.new_move_state = mo::MOVESTATE_FORWARD;
        }
    } else {
        // Not moving--should we be?
        if move_state_goal == mo::MOVESTATE_FORWARD
            || move_state_goal == mo::MOVESTATE_PIVOT_FORWARD
        {
            pause(w);
            d.new_move_state = mo::MOVESTATE_PIVOT_FORWARD;
        } else if move_state_goal == mo::MOVESTATE_REVERSE
            || move_state_goal == mo::MOVESTATE_PIVOT_REVERSE
        {
            pause(w);
            d.new_move_state = mo::MOVESTATE_PIVOT_REVERSE;
        }
    }
    let _ = h;
}

/// `BattleMech::setNextMovePath(...)` (mech.cpp:4599).
fn set_next_move_path(w: &mut World, slot: u16, d: &mut Decision) {
    // If this is only an intermediate path, let's check where we need to go next...
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
            mc2_path::movemap::MOVEPARAM_FACE_TARGET,
            1,
        );
    } else {
        warrior::clear_move_orders(w, slot);
        d.new_gesture_goal = mech_state::STANDING;
    }
}

/// `BattleMech::setControlSettings(...)` (mech.cpp:4695).
fn set_control_settings(
    w: &mut World,
    h: ObjectHandle,
    slot: u16,
    d: &mut Decision,
    min_throttle: i32,
    max_throttle: i32,
) -> Result<(), MoverDecisionUnavailable> {
    // Clean up and handle any jump settings...
    let order_is_jump = w.pilots.get(slot).is_some_and(|p| {
        matches!(
            p.cur_order.code,
            crate::command::OrderCode::JumpToPoint | crate::command::OrderCode::JumpToObject
        )
    });
    if d.new_gesture_goal == mech_state::JUMPING || order_is_jump {
        let starting_jump = !matches!(
            w.objects.get(h).map(|o| &o.kind),
            Some(crate::object::Kind::Mech(m)) if m.in_jump
        );
        let destination = if order_is_jump {
            w.pilots
                .get(slot)
                .and_then(|p| p.cur_order.move_params.way_path.points.first().copied())
        } else {
            w.pilots.get(slot).and_then(|p| {
                let path = &p.lane_b_move.paths[0];
                path.steps.get(path.cur_step).map(|step| step.destination)
            })
        };
        if let Some(destination) = destination {
            crate::ai::pause_path(w, slot);
            if let Some(m) = mech_mut(w, h) {
                m.jump_goal = destination;
                m.in_jump = true;
                m.control.gesture_goal = mech_state::JUMPING as i8;
                m.control.throttle = 100;
            }
            w.appearance.set_jump_parameters(h, destination);
            d.new_gesture_goal = mech_state::JUMPING;
            if starting_jump {
                crate::events::emit_sound(w, h, crate::events::sfx::JUMPJETS, true);
            }
            return Ok(());
        }
    }
    if d.new_gesture_goal != -1 {
        let goal = d.new_gesture_goal;
        let throttle = d.new_throttle;
        if let Some(m) = mech_mut(w, h) {
            m.control.gesture_goal = goal as i8;
            match goal {
                mech_state::WALKING => {
                    if throttle != -1 {
                        let clamped = throttle.clamp(min_throttle, max_throttle);
                        d.new_throttle = clamped;
                        m.control.throttle = clamped as i8;
                    }
                }
                _ => m.control.throttle = throttle as i8,
            }
        }
    }
    if d.new_rotate != 0.0 {
        if let Some(m) = mech_mut(w, h) {
            m.control.rotate = d.new_rotate;
        }
    }
    if d.facing_rotate != 0.0 {
        if let Some(m) = mech_mut(w, h) {
            m.control.facing_rotate = d.facing_rotate;
        }
    }
    Ok(())
}

/// `BattleMech::updateMoveStateGoal()` (mech.cpp:4111).
fn update_move_state_goal(w: &mut World, h: ObjectHandle, slot: u16) {
    let _ = crate::ai::get_move_path(w, slot);
    let Some(pilot) = w.pilots.get(slot) else {
        return;
    };
    let path = pilot.lane_b_move.paths[0].clone();
    let attack_point_order = pilot.cur_order.code == crate::command::OrderCode::AttackPoint;
    let attack_target_point = pilot.attack_orders.target_point;
    let mut target_position = Vec3::default();
    let mut has_target = false;
    if let Some(t) = warrior::get_last_target(w, slot) {
        target_position = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
        has_target = true;
    } else if attack_point_order {
        target_position = attack_target_point;
        has_target = true;
    }
    let Some(pilot) = w.pilots.get(slot) else {
        return;
    };
    let move_state_goal = pilot.lane_b_move.move_state_goal;
    let run = pilot.lane_b_move.run;
    let leg_status = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.leg_status.0,
        _ => leg::NORMAL,
    };
    if path.num_steps > 0 {
        if run || leg_status == leg::IMPAIRED_LEFT || leg_status == leg::IMPAIRED_RIGHT {
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
                let max_torso_yaw = match w.objects.get(h).map(|o| &o.kind) {
                    Some(Kind::Mech(m)) => m.max_torso_yaw,
                    _ => 0.0,
                };
                let total_fire_arc = fire_arc(w, h) + max_torso_yaw;
                let twisting = pilot.lane_b_move.twisting;
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

// ---------------------------------------------------------------------------------------------------------------------
// updateTorso / updateDynamics
// ---------------------------------------------------------------------------------------------------------------------

/// `BattleMech::updateTorso(newRotatePerSec)` (mech.cpp:4628).
pub fn update_torso(w: &mut World, h: ObjectHandle, slot: u16, new_rotate_per_sec: f32) {
    let target = warrior::get_current_target(w, slot);
    let attack_point = w.pilots.get(slot).and_then(|p| {
        (p.cur_order.code == crate::command::OrderCode::AttackPoint)
            .then_some(p.attack_orders.target_point)
    });
    let torso_rotation = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.torso_rotation,
        _ => return,
    };
    let torso_rel_facing = if let Some(t) = target {
        let pos = w.objects.get(t).map_or(Vec3::default(), |o| o.c.position);
        crate::ai::decide::rel_facing_to(w, h, &pos, -1) + torso_rotation + new_rotate_per_sec
    } else if let Some(p) = attack_point {
        crate::ai::decide::rel_facing_to(w, h, &p, -1) + torso_rotation + new_rotate_per_sec
    } else {
        torso_rotation
    };
    let fire_arc = fire_arc(w, h);
    if torso_rel_facing < -fire_arc || torso_rel_facing > fire_arc {
        let mut turn_rate = -torso_rel_facing;
        // We can and will shift facing to destination...
        let max_torso_yaw_rate = match w.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Mech(m)) => m.max_torso_yaw_rate,
            _ => 0.0,
        };
        let max_rate = max_torso_yaw_rate * w.clock.frame_length;
        if turn_rate.abs() > max_rate {
            turn_rate = if turn_rate < 0.0 { -max_rate } else { max_rate };
        }
        if let Some(m) = mech_mut(w, h) {
            m.control.rotate_torso = turn_rate;
        }
    }
}

/// `BattleMech::updateDynamics()` (mech.cpp:5663).
pub fn update_dynamics(w: &mut World, h: ObjectHandle) {
    let Some(Kind::Mech(mech)) = w.objects.get(h).map(|o| &o.kind) else {
        return;
    };
    let yaw_rate = mech.control.rotate;
    let mut torso_yaw_rate = mech.control.rotate_torso;
    let mut right_arm_yaw_rate = mech.control.rotate_arms.1;
    let mut left_arm_yaw_rate = mech.control.rotate_arms.0;
    let gesture_goal = mech.control.gesture_goal;
    let max_torso_yaw = mech.max_torso_yaw;
    let mut torso_rotation = mech.torso_rotation;
    let mut left_arm_rotation = mech.left_arm_rotation;
    let mut right_arm_rotation = mech.right_arm_rotation;

    // The gesture is how we control speed. So, at this point, check the control gesture goal and see if we need to change goals
    if gesture_goal != -1 {
        crate::appearance::set_gesture_goal(w, h, i32::from(gesture_goal));
    }

    // Rotate torso by torso yaw rate, making sure we don't turn too far.
    if torso_yaw_rate != 0.0 {
        let mut new_torso_rotation = torso_rotation;
        if new_torso_rotation > max_torso_yaw {
            new_torso_rotation = max_torso_yaw;
            torso_yaw_rate = 0.0;
        }
        if new_torso_rotation < -max_torso_yaw {
            new_torso_rotation = -max_torso_yaw;
            torso_yaw_rate = 0.0;
        }
        if new_torso_rotation + torso_yaw_rate > max_torso_yaw {
            torso_yaw_rate = max_torso_yaw - new_torso_rotation;
        }
        if new_torso_rotation + torso_yaw_rate < -max_torso_yaw {
            torso_yaw_rate = -max_torso_yaw - new_torso_rotation;
        }
        new_torso_rotation += torso_yaw_rate;
        torso_rotation = new_torso_rotation;
    }

    // Rotate rightArm by rightArm yaw rate, making sure we don't turn too far.
    if right_arm_yaw_rate != 0.0 {
        let limit = TONNAGE_TURN_RATE;
        let mut new_rotation = right_arm_rotation;
        if new_rotation > limit {
            new_rotation = limit;
            right_arm_yaw_rate = 0.0;
        }
        if new_rotation < -limit {
            new_rotation = -limit;
            right_arm_yaw_rate = 0.0;
        }
        if new_rotation + right_arm_yaw_rate > limit {
            new_rotation = limit;
            right_arm_yaw_rate = 0.0;
        }
        if new_rotation + right_arm_yaw_rate < -limit {
            new_rotation = -limit;
            right_arm_yaw_rate = 0.0;
        }
        new_rotation += right_arm_yaw_rate;
        right_arm_rotation = new_rotation;
    }

    // (the source labels this block "rightArm" again; it rotates the left arm)
    if left_arm_yaw_rate != 0.0 {
        let limit = TONNAGE_TURN_RATE;
        let mut new_rotation = left_arm_rotation;
        if new_rotation < -limit {
            new_rotation = -limit;
            left_arm_yaw_rate = 0.0;
        }
        if new_rotation > limit {
            new_rotation = limit;
            left_arm_yaw_rate = 0.0;
        }
        if new_rotation + left_arm_yaw_rate < -limit {
            new_rotation = -limit;
            left_arm_yaw_rate = 0.0;
        }
        if new_rotation + left_arm_yaw_rate > limit {
            new_rotation = limit;
            left_arm_yaw_rate = 0.0;
        }
        new_rotation += left_arm_yaw_rate;
        left_arm_rotation = new_rotation;
    }

    if let Some(obj) = w.objects.get_mut(h) {
        if let Kind::Mech(m) = &mut obj.kind {
            m.torso_rotation = torso_rotation;
            m.left_arm_rotation = left_arm_rotation;
            m.right_arm_rotation = right_arm_rotation;
        }
        // `rotate(yawRate, facingYawRate)`
        obj.c.rotation += yaw_rate;
        if obj.c.rotation < -180.0 {
            obj.c.rotation += 360.0;
        } else if obj.c.rotation > 180.0 {
            obj.c.rotation -= 360.0;
        }
    }
}

/// `BODYSTATE_STANDING` compare helper used by callers that need the body state of a gesture.
pub fn body_state_of(w: &World, h: ObjectHandle) -> i8 {
    let g = w.appearance.current_gesture_id(h);
    crate::appearance::tables::MECH_STATE_BY_GESTURE
        .get(usize::try_from(g).unwrap_or(0))
        .copied()
        .unwrap_or(body_state::NORMAL)
}

/// Advance the disabled and destroyed branches of `BattleMech::update` (mech.cpp:6091-6235).
pub fn update_incapacitated_mech(w: &mut crate::World, h: crate::ids::ObjectHandle) {
    use crate::object::{Kind, ObjectStatus, ObjectStore};
    use mc2_sim_data::objtype::ObjectTypeData;

    if let Some(Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        m.control.gesture_goal = -1;
    }
    update_dynamics(w, h);
    let Some(obj) = w.objects.get(h) else { return };
    let mut velocity = crate::math::rotation_vector(obj.c.rotation);
    let distance = w.appearance.velocity_magnitude(h)
        * crate::terrain::WORLD_UNITS_PER_METER
        * w.clock.frame_length;
    velocity.x *= distance;
    velocity.y *= distance;
    velocity.z *= distance;
    let mut position = obj.c.position;
    position.x += velocity.x;
    position.y += velocity.y;
    position.z = w.terrain.elevation(&position);
    if let Some(Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        m.m.velocity = velocity;
    }
    super::set_position(w, h, position);
    let Some(obj) = w.objects.get(h) else { return };
    let status = obj.c.status;
    let (air, damage, radius) = match w.objects.type_of(h).map(|t| &t.data) {
        Some(ObjectTypeData::BattleMech(t)) => {
            (t.move_type == 1, t.explosion_damage, t.explosion_radius)
        }
        _ => return,
    };
    let appearance_fallen = w.appearance.have_fallen(h);
    if status == ObjectStatus::Destroyed {
        w.appearance
            .set_gesture_goal(h, crate::appearance::tables::state::FALLEN_BACKWARD);
    }
    let dt = w.clock.frame_length;
    let Some(obj) = w.objects.get_mut(h) else {
        return;
    };
    let Kind::Mech(m) = &mut obj.kind else { return };
    let mut effect = None;
    let mut crater = false;
    let mut put_out_smoke = false;
    let mut push_status = false;
    if status == ObjectStatus::Disabled {
        if !m.fallen {
            m.fallen = appearance_fallen;
        }
        if !m.m.flags.start_disabled {
            m.m.time_left -= dt;
            if m.m.time_left < 1.0 && !m.m.flags.exploding {
                m.m.flags.exploding = true;
                if w.pilots.get(m.m.pilot_slot).is_some_and(|p| p.wounds < 6.0) {
                    effect = Some((49, 0.0, 0.0));
                }
                // `appearance->startSmoking(-1)` (mech.cpp:6150): a disabled mech stops smoking once it is down
                put_out_smoke = true;
            }
        }
        if air {
            obj.c.status = ObjectStatus::Destroyed;
            m.m.time_left = 0.0;
        }
    } else {
        if !m.fallen {
            m.fallen = appearance_fallen;
            m.m.time_left = 1.0;
        }
        if m.fallen {
            m.m.time_left -= dt;
            if m.m.time_left < 0.4 && !m.m.flags.exploding {
                m.m.flags.exploding = true;
                effect = Some(if air {
                    (40, 0.0, 0.0)
                } else {
                    (39, damage, radius)
                });
                crater = true;
            } else if !m.mech_removed && m.m.time_left < if air { -1.5 } else { -2.5 } {
                m.mech_removed = true;
            } else if m.mech_removed {
                // `appearance->setObjStatus(getStatus())` (mech.cpp:6231-6232): the only status push a destroyed mech gets
                push_status = true;
            }
        }
    }
    let at = obj.c.position;
    if push_status {
        // Both fields: `status` (D-52 already holds DESTROYED for a mech killed by damage) and the roll gate `cpp_status`.
        w.appearance
            .set_obj_status(h, ObjectStatus::Destroyed as i32);
        w.appearance
            .set_mech_cpp_status(h, ObjectStatus::Destroyed as i32);
    }
    if put_out_smoke {
        crate::appearance::fx::start_smoking(w, h, -1);
    }
    if let Some((id, damage, radius)) = effect {
        let cockpit = w.appearance.node_name_position(h, "cockpit");
        ObjectStore::create_explosion(w, id, None, cockpit, damage, radius);
    }
    if crater {
        let rotation = w.rng.0.borrow_mut().visual.random_number(180) as f32;
        w.events.push(crate::SimEvent::Crater {
            kind: crate::crater::CRATER_4,
            at,
            rotation,
        });
    }
}

#[cfg(test)]
mod piloting_tests {
    use super::*;

    #[test]
    fn each_destroyed_hip_actuator_adds_ten_to_the_landing_check() {
        let mut loadout = crate::combat::Loadout::default();
        loadout.stats.actuator[2] = 0;
        loadout.stats.actuator[3] = 1;
        loadout.inventory = vec![
            crate::combat::InventoryItem {
                health: 0,
                ..Default::default()
            },
            crate::combat::InventoryItem {
                health: 0,
                ..Default::default()
            },
        ];

        let roll = 5.0;
        let skill = 20.0;
        let both_penalties = failed_hip_actuator_count(&loadout) as f32 * 10.0;
        assert_eq!(both_penalties, 20.0);
        assert!(roll + both_penalties >= skill);
        loadout.inventory[1].health = 1;
        let one_penalty = failed_hip_actuator_count(&loadout) as f32 * 10.0;
        assert_eq!(one_penalty, 10.0);
        assert!(roll + one_penalty < skill);
    }
}
