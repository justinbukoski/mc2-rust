// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Weapon bolt impact: `WeaponBolt::update` hit/miss handling (`code/weaponbolt.cpp`).

/// `Stuff::SMALL` (`mclib/stuff/style.cpp:21`): the `f2 > Stuff::SMALL` test inside `mc2_atan2` (mclib/mathfunc.h:159-181).
const STUFF_SMALL: f32 = 1.0e-4;

/// The pre-hit elevation head-shot roll of `WeaponBolt::update` (weaponbolt.cpp:732-760, duplicated at 984-1010).
///
/// The C++ only rolls when the target is *below* the shooter (`target->getPosition().z < owner->getPosition().z`; `z` is the
/// elevation in this port) and there is an owner (the caller checks the owner lookup). It then takes `zDistance = owner.z -
/// target.z`, zeroes both `z` components, measures the ground distance `yDistance = (owner - target).GetApproximateLength()`
/// and rolls `RollDice(angle * BaseHeadShotElevation)` with `angle = mc2_atan2(zDistance, yDistance) * RADS_TO_DEGREES`.
///
/// This returns the percent the C++ hands to `RollDice` - truncated, because `RollDice` takes a `long` (mclib/mathfunc.h:113)
/// and the float product is converted on the call - or `None` when the target is not below the owner, in which case the caller
/// draws **no** random number. [`apply_head_shot`] consumes the draw.
///
/// `GetApproximateLength()` is `SqrtApproximate(GetLengthSquared())` (stuff/vector3d.hpp:414). `SqrtApproximate`
/// (stuff/scalar.hpp:106-127) is the bit-hack approximation only under `USE_ASSEMBLER_CODE`, which is off in the oracle build:
/// there it is `static_cast<float>(sqrt(f))`, i.e. the crate's [`approximate_length`](crate::math::approximate_length) (double
/// `sqrt` of the float length squared, narrowed back to `f32`), which is what this uses.
///
/// The arctangent is [`portable_atan2`](crate::math::portable_atan2) (own arithmetic, bit-identical on every platform) rather than a
/// libm call: the angle is truncated to an integer percent, so a one-ulp libm difference at a boundary would change the hit
/// location (owner rule: the Rust sim is deterministic across platforms; the C++ value is within a float ulp of this one).
///
/// `mc2_atan2(zDistance, yDistance)` returns the hard-coded `1.570796f` when `yDistance` is not greater than `Stuff::SMALL`
/// (a target straight below the shooter), so that case yields 89, not 90.
pub fn head_shot_percent(
    owner: crate::math::Vec3,
    target: crate::math::Vec3,
    base_head_shot_elevation: f32,
) -> Option<i64> {
    // The C++ `target->getPosition().z < owner->getPosition().z` (plain float compare; no NaN is expected here).
    if target.z >= owner.z {
        return None;
    }
    let z_distance = owner.z - target.z;
    // `targetPos.z = ownerPos.z = 0.0f; distance.Subtract(ownerPos, targetPos)`: only the x/y difference survives (its sign is
    // irrelevant to the length).
    let distance = crate::math::Vec3 {
        x: owner.x - target.x,
        y: owner.y - target.y,
        z: 0.0,
    };
    let y_distance = crate::math::approximate_length(&distance);
    let angle = if y_distance > STUFF_SMALL {
        crate::math::portable_atan2(z_distance, y_distance)
    } else {
        // The C++ literal of mc2_atan2's early result (mclib/mathfunc.h:161), not pi/2.
        #[allow(clippy::approx_constant)]
        let result = 1.570_796;
        result
    } * crate::math::RADS_TO_DEGREES;
    // `RollDice(angle * BaseHeadShotElevation)`: the float product truncates toward zero into the `long` parameter.
    Some((angle * base_head_shot_elevation) as i64)
}

/// The `RollDice` half of the elevation head shot: with a percent (the target was below the owner) draw exactly one sim number
/// and set `shot.hit_location = 0` when the roll succeeds; with `None` do nothing and draw nothing.
///
/// [`SimRng::roll_dice`](crate::rng::SimRng::roll_dice) is `((gos_rand() * 100) >> 15) < percent`, so a non-positive percent
/// still consumes its draw - exactly as the C++ `RollDice` (mclib/routines.cpp:188) does.
pub fn apply_head_shot(
    w: &mut crate::World,
    shot: &mut crate::combat::WeaponShotInfo,
    percent: Option<i64>,
) {
    let Some(percent) = percent else {
        return;
    };
    // The C++ `long` is 64-bit on the oracle build; `roll_dice` takes an `i32` (gos_rand values are 15-bit), so clamp.
    let percent = percent.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    if w.rng.0.borrow_mut().sim.roll_dice(percent) {
        shot.hit_location = 0; // Make it a head shot!
    }
}

// -----------------------------------------------------------------------------------------------------------------------------
// What a bolt does when it hits (weaponbolt.cpp:739-870, 975-1058) and `finishNow` (weaponbolt.cpp:2610-2690).

use crate::combat::weapon_bolt::{bolt_params, bolt_state, bolt_state_mut};
use crate::ids::ObjectHandle;
use crate::object::carnage::MINE_EXPLOSION_ID;
use crate::object::{hooks, ObjectStore};
use crate::World;

/// The impact of a bolt whose `hit_target` just became true. With a live target object: the elevation head shot roll (one sim draw
/// when the target is below the owner), then either the area-effect explosion (the explosion does the damage) or the hit through
/// `handleWeaponHit`. Without one (a shot at a point): the crater, the mine at the cell, and the area-effect explosion. Single
/// player only (`MPlayer == NULL`).
pub fn on_hit(w: &mut World, h: ObjectHandle) {
    let Some(params) = bolt_params(w, h) else {
        return;
    };
    let Some(b) = bolt_state(w, h) else { return };
    let (target_wid, owner_wid, target_position, shot_id, hot_spot) = (
        b.target,
        b.owner,
        b.target_position,
        b.shot_id,
        b.hot_spots.1,
    );
    let target = w.objects.by_watch_id(target_wid);
    // the hit effect starts on the target branch, the miss effect on the other (weaponbolt.cpp:614-708)
    let (effect_id, mut laser) = (b.effect_id, b.laser_position);
    // a beam's hit effect starts at the target's hot spot when its duration ends, not where the flight left the beam
    // (weaponbolt.cpp:879-900); a bolt's laserPosition was already put on the spot (weaponbolt.cpp:656)
    if let (true, Some(t)) = (params.is_beam, target) {
        laser = crate::combat::weapon_bolt::target_aim_point(w, t, hot_spot);
    }
    let from = w.objects.get(h).map_or(laser, |o| o.c.position);
    w.events.push(crate::events::SimEvent::WeaponEffect {
        shot_id,
        effect_id,
        owner: owner_wid,
        from,
        to: if target.is_some() {
            laser
        } else {
            target_position
        },
        impact: if target.is_some() {
            crate::events::WeaponImpact::Hit
        } else {
            crate::events::WeaponImpact::Miss
        },
    });
    if let Some(t) = target {
        // Figure out angle between target and Shooter if target is BELOW shooter. If no shooter, no bonus!!
        if let Some(owner) = w.objects.by_watch_id(owner_wid) {
            let (tp, op) = (
                w.objects.get(t).map(|o| o.c.position),
                w.objects.get(owner).map(|o| o.c.position),
            );
            if let (Some(tp), Some(op)) = (tp, op) {
                let elevation = w
                    .data
                    .gamesys
                    .tables
                    .as_ref()
                    .map_or(1.0, |t| t.general.base_head_shot_elevation);
                // `target->getPosition().z < owner->getPosition().z`: head_shot_percent is None when it is not
                let percent = head_shot_percent(op, tp, elevation);
                if let Some(mut shot) = bolt_state(w, h).and_then(|b| b.weapon_shot) {
                    apply_head_shot(w, &mut shot, percent);
                    if let Some(b) = bolt_state_mut(w, h) {
                        b.weapon_shot = Some(shot);
                    }
                }
            }
        }
        if params.is_area_effect() {
            // This is an area effect weapon. Hit or MISS, its the same! Damage is done by explosion NOT by hit!
            ObjectStore::create_explosion(
                w,
                params.hit_effect,
                None,
                target_position,
                params.area_damage,
                params.area_radius,
            );
        } else if let Some(shot) = bolt_state(w, h).and_then(|b| b.weapon_shot) {
            hooks::weapon_hit(w, t, &shot);
        }
    } else {
        // `targetPosition != NULL` (the first frame refused an unconnected bolt).
        // The crater (`craterManager->addCrater(.., RandomNumber(180))`) only exists with a miss effect, which needs the
        // non-weapon effects preference and the effect library: the headless oracle has neither, so there is no draw.
        // See `ObjectStore::use_non_weapon_effects`.
        if w.objects.use_non_weapon_effects && miss_effect_exists(w, h) {
            let water = {
                let (row, col) = crate::object::terr::world_to_cell(&w.terrain, &target_position);
                w.terrain
                    .cell(row, col)
                    .is_some_and(|c| c.deep_water() || c.shallow_water())
            };
            if !water {
                let rotation = {
                    let r = w.rng.0.clone();
                    let n = r.borrow_mut().visual.random_number(180);
                    n as f32
                };
                let damage = bolt_state(w, h)
                    .and_then(|b| b.weapon_shot)
                    .map_or(0.0, |s| s.damage);
                w.events.push(crate::events::SimEvent::Crater {
                    kind: crate::crater::bolt_crater_type(damage),
                    at: target_position,
                    rotation,
                });
            }
        }
        // energy weapons set off mines too (DT, 5/30/98)
        trip_mine(w, target_position);
        if params.is_area_effect() {
            ObjectStore::create_explosion(
                w,
                params.hit_effect,
                None,
                target_position,
                params.area_damage,
                params.area_radius,
            );
        }
    }
}

/// Whether the bolt's effect row names a miss effect (`GetEffectMissName != "NONE"`).
fn miss_effect_exists(w: &World, h: ObjectHandle) -> bool {
    let Some(b) = bolt_state(w, h) else {
        return false;
    };
    usize::try_from(b.effect_id)
        .ok()
        .and_then(|i| w.objects.effect_rows.get(i))
        .is_some_and(|r| !r.names[3].is_empty() && !r.names[3].eq_ignore_ascii_case("NONE"))
}

/// `WeaponBolt::finishNow` (weaponbolt.cpp:2610): a bolt still in flight when its pool slot is needed again resolves at once: the
/// hit on its target (no head shot roll), the area-effect explosion, then it ceases to exist.
pub fn finish_now(w: &mut World, h: ObjectHandle) {
    if !w.objects.get(h).is_some_and(|o| o.exists()) {
        return;
    }
    let Some(params) = bolt_params(w, h) else {
        if let Some(o) = w.objects.get_mut(h) {
            o.set_exists(false);
        }
        return;
    };
    let Some(b) = bolt_state(w, h) else { return };
    let (target_wid, shot, target_position) = (b.target, b.weapon_shot, b.target_position);
    if let Some(t) = w.objects.by_watch_id(target_wid) {
        if let Some(shot) = shot {
            hooks::weapon_hit(w, t, &shot);
        }
    }
    if params.is_area_effect() {
        ObjectStore::create_explosion(
            w,
            params.hit_effect,
            None,
            target_position,
            params.area_damage,
            params.area_radius,
        );
    }
    if let Some(o) = w.objects.get_mut(h) {
        o.set_exists(false);
    }
}

/// The mine check shared by a bolt that lands on the ground and a shot whose bolt could not be created (mech.cpp:8234): a live mine
/// (`getMine == 1`) in the cell of `at` explodes (`MINE_EXPLOSION_ID`, the gamesys splash damage and range) and is spent (`setMine(.., 2)`).
pub fn trip_mine(w: &mut World, at: crate::math::Vec3) {
    let (row, col) = crate::object::terr::world_to_cell(&w.terrain, &at);
    if crate::object::terr::get_mine(&w.terrain, row, col) == 1 {
        let (dmg, range) = hooks::mine_splash(w);
        ObjectStore::create_explosion(
            w,
            MINE_EXPLOSION_ID,
            None,
            at,
            dmg,
            range * crate::terrain::WORLD_UNITS_PER_METER,
        );
        crate::object::terr::set_mine(&mut w.terrain, row, col, 2);
    }
}
