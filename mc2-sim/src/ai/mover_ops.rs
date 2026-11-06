// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The `Mover` / `GameObject` calls the pilot code makes that no other lane exposes as a function yet (each one cites the C++ it
//! stands for). They read or write the mover behind a handle; none draws random numbers.

use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::object::{Kind, ObjectStatus};
use crate::terrain::{METERS_PER_WORLD_UNIT, WORLD_UNITS_PER_CELL, WORLD_UNITS_PER_VERTEX};
use crate::World;

/// `LEG_STATUS_DESTROYED` (mech.h:181).
const LEG_STATUS_DESTROYED: u8 = 4;

/// `Mover::canMove()`: a mech can move unless both legs are gone (mech.h:650); a ground vehicle while movement is enabled and it has
/// a top speed (gvehicl.h:380; `movementEnabled` is always true here: only tutorial scripts clear it).
pub fn can_move(w: &World, h: ObjectHandle) -> bool {
    match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.leg_status.0 != LEG_STATUS_DESTROYED,
        Some(Kind::Vehicle(v)) => v.m.max_move_speed != 0.0,
        _ => true,
    }
}

/// `BattleMech::getJumpRange()` (mech.cpp:6744): `numJets * worldUnitsPerVertex`, one more jet for a long-jump specialist. The
/// number of offsets and the cost are not needed by the order code. 0 for anything but a mech (`Mover::getJumpRange`).
pub fn jump_range(w: &World, h: ObjectHandle) -> f32 {
    let Some(obj) = w.objects.get(h) else {
        return 0.0;
    };
    let Kind::Mech(m) = &obj.kind else {
        return 0.0;
    };
    let mut jets = i32::from(m.num_jump_jets);
    let long_jump = w
        .pilots
        .get(m.m.pilot_slot)
        .is_some_and(|p| p.specialty.get(13).copied().unwrap_or(false));
    if long_jump {
        jets += 1;
    }
    jets as f32 * WORLD_UNITS_PER_VERTEX
}

/// `Mover::isJumping(&goal)`: `BattleMech::inJump` and `jumpGoal`.
pub fn is_jumping(w: &World, h: ObjectHandle) -> Option<Vec3> {
    match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) if m.in_jump => Some(m.jump_goal),
        _ => None,
    }
}

/// `Mover::shutDown()` (mover.cpp:6565).
pub fn shut_down(w: &mut World, h: ObjectHandle) {
    let Some(obj) = w.objects.get_mut(h) else {
        return;
    };
    if !matches!(&obj.kind, Kind::Mech(_) | Kind::Vehicle(_)) {
        return;
    }
    if obj.c.is_disabled()
        || matches!(
            obj.c.status,
            ObjectStatus::ShutDown | ObjectStatus::ShuttingDown
        )
    {
        return;
    }
    // Immediately begin shutting down...
    obj.c.set_status(ObjectStatus::ShuttingDown, false);
    match &mut obj.kind {
        Kind::Mech(m) => m.m.flags.shut_down_this_frame = true,
        Kind::Vehicle(v) => v.m.flags.shut_down_this_frame = true,
        _ => (),
    }
}

/// `Mover::startUp()` (mover.cpp:6581).
pub fn start_up(w: &mut World, h: ObjectHandle) {
    let Some(obj) = w.objects.get_mut(h) else {
        return;
    };
    if !matches!(&obj.kind, Kind::Mech(_) | Kind::Vehicle(_)) {
        return;
    }
    if obj.c.is_disabled()
        || matches!(
            obj.c.status,
            ObjectStatus::StartingUp | ObjectStatus::Normal
        )
    {
        return;
    }
    // Immediately begin starting up...
    obj.c.set_status(ObjectStatus::StartingUp, false);
    match &mut obj.kind {
        Kind::Mech(m) => m.m.flags.start_up_this_frame = true,
        Kind::Vehicle(v) => v.m.flags.start_up_this_frame = true,
        _ => (),
    }
}

/// `GameObject::cellDistanceFrom(goal)` (gameobj.cpp:1773): the larger of the row and column distance between the object's map cell
/// and the cell of `goal`.
pub fn cell_distance_from(w: &World, h: ObjectHandle, goal: &Vec3) -> i32 {
    let Some(obj) = w.objects.get(h) else {
        return 0;
    };
    let (goal_row, goal_col) = w.terrain.world_to_cell(goal);
    let (row, col) = (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1));
    let row_delta = (row - goal_row).abs();
    let col_delta = (col - goal_col).abs();
    row_delta.max(col_delta)
}

/// `GameObject::distanceFrom(goal)` in meters (gameobj.cpp:1762).
pub fn distance_from(w: &World, h: ObjectHandle, goal: &Vec3) -> f32 {
    w.objects.get(h).map_or(0.0, |o| {
        crate::combat::attack::distance_from(&o.c.position, goal)
    })
}

/// `Mover::calcOffsetMoveGoal(target)` (mover.cpp:4042): the point on the line from the target toward the mover, no farther than the
/// mover, that is the first passable cell (the target itself when it is passable).
pub fn calc_offset_move_goal(w: &World, h: ObjectHandle, target: &Vec3) -> Vec3 {
    let Some(obj) = w.objects.get(h) else {
        return *target;
    };
    let start = obj.c.position;
    let mut delta = (start.x - target.x, start.y - target.y);
    if delta.0 == 0.0 && delta.1 == 0.0 {
        return start;
    }
    // Vector2DOf::Normalize: divide by the (float) length
    let length = ((delta.0 * delta.0 + delta.1 * delta.1) as f64).sqrt() as f32;
    delta = (delta.0 / length, delta.1 / length);
    let mut cell_length = WORLD_UNITS_PER_CELL;
    cell_length *= 0.5;
    delta = (delta.0 * cell_length, delta.1 * cell_length);
    // distance_from(start, target): world units (the metersPerWorldUnit factor is commented out in the source)
    let distance_to_target = crate::math::approximate_length(&Vec3 {
        x: start.x - target.x,
        y: start.y - target.y,
        z: start.z - target.z,
    });
    let mut cur = (target.x, target.y);
    let mut ray_length = 0.0f32;
    let passable = |p: (f32, f32)| {
        w.terrain.passable_at(&Vec3 {
            x: p.0,
            y: p.1,
            z: 0.0,
        })
    };
    if !passable(cur) {
        while ray_length < distance_to_target {
            if passable(cur) {
                break;
            }
            cur = (cur.0 + delta.0, cur.1 + delta.1);
            let ray = (cur.0 - start.x, cur.1 - start.y);
            ray_length = ((ray.0 * ray.0 + ray.1 * ray.1) as f64).sqrt() as f32;
        }
    }
    let mut out = Vec3 {
        x: cur.0,
        y: cur.1,
        z: 0.0,
    };
    out.z = w.terrain.elevation(&out);
    out
}

/// `metersPerWorldUnit` (0.2f), re-exported for the order code.
pub const METERS_PER_WU: f32 = METERS_PER_WORLD_UNIT;

/// `BattleMech::isWeaponReady` (mech.cpp:6637: also needs health) / `Mover::isWeaponReady` (mover.cpp:5990, ground vehicles).
pub fn is_weapon_ready(w: &World, h: ObjectHandle, weapon_index: usize) -> bool {
    let Some(obj) = w.objects.get(h) else {
        return false;
    };
    let Some(l) = crate::combat::loadout_of(obj) else {
        return false;
    };
    let Some(item) = l.inventory.get(weapon_index) else {
        return false;
    };
    if item.disabled {
        return false;
    }
    if matches!(obj.kind, Kind::Mech(_)) && item.health == 0 {
        return false;
    }
    item.ready_time <= w.clock.scenario_time
}

/// `Mover::getWeaponShots(weaponIndex)` (mover.cpp:5660): -1 for a non weapon index, `UNLIMITED_SHOTS` (9999) for energy weapons.
pub fn weapon_shots(w: &World, h: ObjectHandle, weapon_index: i32) -> i32 {
    let (Some(obj), Some(comps)) = (w.objects.get(h), w.data.components.as_ref()) else {
        return -1;
    };
    let Some(l) = crate::combat::loadout_of(obj) else {
        return -1;
    };
    if !l.is_weapon_index(i64::from(weapon_index)) {
        return -1;
    }
    l.weapon_shots(comps, i64::from(weapon_index))
}

/// `Mover::weaponInRange(index, metersToTarget, buffer)` (mover.cpp:5549).
pub fn weapon_in_range(
    w: &World,
    h: ObjectHandle,
    weapon_index: usize,
    meters: f32,
    buffer: f32,
) -> bool {
    let (Some(obj), Some(comps), Some(gs)) = (
        w.objects.get(h),
        w.data.components.as_ref(),
        w.data.gamesys.tables.as_ref(),
    ) else {
        return false;
    };
    let Some(l) = crate::combat::loadout_of(obj) else {
        return false;
    };
    let Some(item) = l.inventory.get(weapon_index) else {
        return false;
    };
    let range_index =
        crate::combat::stats::weapon_stats(crate::combat::stats::comp(comps, item.master_id))
            .and_then(|ws| ws.range)
            .map_or(0, |r| r.index());
    let wr = gs.general.weapon_ranges[range_index];
    let min = wr[0] - buffer;
    let max = wr[1] + buffer;
    !(meters < min || meters > max)
}
