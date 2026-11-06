// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Pure helpers of `BattleMech::fireWeapon` (the firing half of `code/mech.cpp`).

/// `miss_radius` (turret.cpp:1655-1685, mech.cpp:8372-8402): miss offset radius in world units.
/// 25.0 with a target object, otherwise 5.0.
pub fn miss_radius(has_target: bool) -> f32 {
    if has_target {
        25.0
    } else {
        5.0
    }
}

/// Four candidate (x, y) miss points, tested in order for line of sight.
///
/// The sequence is cumulative in f32:
/// - p1.x = base.x + r, p1.y = base.y
/// - p2.x = p1.x - (r * 2.0), p2.y = base.y
/// - p3.x = p2.x + r, p3.y = base.y + r
/// - p4.x = p3.x, p4.y = p3.y - (r * 2.0)
///
/// Returns the (x, y) pairs; z is recomputed from terrain by the caller.
pub fn miss_offset_points(base: crate::math::Vec3, miss_radius: f32) -> [(f32, f32); 4] {
    let r = miss_radius;
    let p1_x = base.x + r;
    let p2_x = p1_x - (r * 2.0);
    let p3_x = p2_x + r;
    let p3_y = base.y + r;
    let p4_y = p3_y - (r * 2.0);
    [(p1_x, base.y), (p2_x, base.y), (p3_x, p3_y), (p3_x, p4_y)]
}

/// `BattleMech::fireWeapon` hit branch (mech.cpp:8103): how many missiles hit the target.
///
/// Streak: `numMissiles = missileAmount` with no clamps. Otherwise
/// `numMissiles = (long)(missileAmount / 2.0 + 0.5)` (double arithmetic, C-style truncation
/// toward zero), then the `< 1` clamp and finally the `> missileAmount` clamp.
pub fn missile_count_hit(missile_amount: i32, streak: bool) -> i32 {
    if streak {
        return missile_amount;
    }
    let mut num_missiles = (missile_amount as f64 / 2.0 + 0.5) as i64;
    if num_missiles < 1 {
        num_missiles = 1;
    }
    if num_missiles > missile_amount as i64 {
        num_missiles = missile_amount as i64;
    }
    num_missiles as i32
}

/// `BattleMech::fireWeapon` miss branch (mech.cpp:8338): how many missiles are counted as missed.
///
/// Same rounding and clamps as the hit branch; the miss branch is never streak-aware.
pub fn missile_count_miss(missile_amount: i32) -> i32 {
    let mut num_missiles = (missile_amount as f64 / 2.0 + 0.5) as i64;
    if num_missiles < 1 {
        num_missiles = 1;
    }
    if num_missiles > missile_amount as i64 {
        num_missiles = missile_amount as i64;
    }
    num_missiles as i32
}

/// `Mover::isWeaponReady` (mover.cpp:5990): true if weapon is not disabled and ready_time <= scenario_time.
///
/// Returns false if `item.disabled`, false if `item.ready_time > scenario_time`, else true.
pub fn is_weapon_ready(item: crate::combat::InventoryItem, scenario_time: f32) -> bool {
    if item.disabled {
        return false;
    }
    if item.ready_time > scenario_time {
        return false;
    }
    true
}

/// `Mover::startWeaponRecycle` (mover.cpp:6001): set ready_time = scenario_time + recycle_time.
///
/// f32 add: `ready_time = scenario_time + recycle_time`.
pub fn start_weapon_recycle(
    item: &mut crate::combat::InventoryItem,
    scenario_time: f32,
    recycle_time: f32,
) {
    item.ready_time = scenario_time + recycle_time;
}

/// `Mover::weaponInRange` (mover.cpp:5549): check if meters_to_target is within range with buffer.
///
/// min = range_min - buffer; max = range_max + buffer.
/// Returns false if meters < min or meters > max (bounds exclusive for failure).
pub fn weapon_in_range(range_min: f32, range_max: f32, meters_to_target: f32, buffer: f32) -> bool {
    let min = range_min - buffer;
    let max = range_max + buffer;
    if meters_to_target < min {
        return false;
    }
    if meters_to_target > max {
        return false;
    }
    true
}

/// `BattleMech::fireWeapon` weapon-node selection (mech.cpp:8019-8032).
///
/// The C++ first picks `MECH3D_WEAPONTYPE_MISSILE` / `_BALLISTIC` / `_ENERGY` from the
/// weapon's form, then immediately overwrites it with `inventory[weaponIndex].facing`, so
/// the form branch is dead: the starting hot spot is the item's `facing`. Two *sequential*
/// ifs then force an arm-mounted weapon into the torso when that arm is gone -- the second
/// test sees the value the first one may have written.
pub fn source_hot_spot(facing: i32, right_arm_off: bool, left_arm_off: bool) -> i32 {
    let mut hs = facing;
    if right_arm_off && hs == 1 {
        hs = 3; // Force into right torso.  Arm is gone.
    }
    if left_arm_off && hs == 2 {
        hs = 5; // Force into left torso.  Arm is gone.
    }
    hs
}

/// Input to [`fire_gate`]: all conditions needed for `BattleMech::fireWeapon` early-return gates.
#[derive(Clone, Copy, Debug)]
pub struct FireGateInput {
    pub status: crate::object::ObjectStatus,
    pub parked_or_fallen: bool,
    pub weapon_ready: bool,
    pub on_game_terrain: bool,
    pub target_destroyed: Option<bool>,
    pub has_target_point: bool,
    pub in_range: bool,
    pub num_shots: i32,
    pub node_recycle: f32,
}

/// `BattleMech::fireWeapon` early-return gates (mech.cpp:7926-8040, 8070-8075).
///
/// Returns `Some(code)` if the shot is blocked, `None` if it can proceed.
/// C++ order: 1) status -> 1; 2) parked/fallen -> 1; 3) !weapon_ready -> 3; 4) !on_terrain -> 1;
/// 5) destroyed target or no target & no point -> 4; 6) !in_range -> 4; 7) num_shots == 0 -> 4;
/// 8) node_recycle > 0 -> 3. `GroundVehicle::fireWeapon` checks its node only after the hit roll (gvehicl.cpp:4600, 4643-4651), so a
/// vehicle passes 0 here and `fire::fire_weapon` checks the node itself.
///
pub fn fire_gate(g: &FireGateInput) -> Option<i32> {
    use crate::object::ObjectStatus;

    if matches!(
        g.status,
        ObjectStatus::ShutDown
            | ObjectStatus::ShuttingDown
            | ObjectStatus::Disabled
            | ObjectStatus::Destroyed
    ) {
        return Some(1);
    }
    if g.parked_or_fallen {
        return Some(1);
    }
    if !g.weapon_ready {
        return Some(3);
    }
    if !g.on_game_terrain {
        return Some(1);
    }
    match g.target_destroyed {
        Some(true) => return Some(4),
        Some(false) => {}
        None => {
            if !g.has_target_point {
                return Some(4);
            }
        }
    }
    if !g.in_range {
        return Some(4);
    }
    if g.num_shots == 0 {
        return Some(4);
    }
    if g.node_recycle > 0.0 {
        return Some(3);
    }
    None
}
