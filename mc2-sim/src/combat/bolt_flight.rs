// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Weapon bolt flight: `WeaponBolt::update` travel step (`code/weaponbolt.cpp`).

/// Result of the bolt crossing test.
pub struct Crossing {
    pub hit: bool,
    pub distance_to_target: f32,
}

/// Test if a bolt hit its target based on distance changes.
///
/// `prev_distance_to_target` is the previous squared xy distance.
/// `distance` is the current squared xy distance (dx*dx + dy*dy).
/// Returns `hit = (prev - distance) >= distance` and always `distance_to_target = distance`.
pub fn bolt_crossing(prev_distance_to_target: f32, distance: f32) -> Crossing {
    // C++: lastDistanceMoved = distanceToTarget - distance; distanceToTarget = distance;
    // if (lastDistanceMoved >= distance) hitTarget = TRUE;
    let last_distance_moved = prev_distance_to_target - distance;
    Crossing {
        hit: last_distance_moved >= distance,
        distance_to_target: distance,
    }
}

/// Result of the arcing bolt vertical velocity step.
pub struct ArcStep {
    pub goal_height: f32,
    pub arcing: bool,
}

/// One step of the arcing bolt vertical velocity integration.
///
/// `goal_height` is actually the vertical velocity (initialised to
/// `arc_height * 10.0f` on the first frame, weaponbolt.cpp:507-509).
/// When `arc_effect` is set and `goal_height > 0.0`, gravity is applied:
/// `goal_height += (-arc_height * 10.0) * frame_length`. If the new value
/// is `>= 0.0` the bolt is arcing (and the velocity is applied to
/// `laserVelocity.z`); otherwise the (possibly negative) value is stored
/// but arcing is false. Without `arc_effect`, or when `goal_height` is not
/// positive, nothing changes and arcing is false.
pub fn arc_step(goal_height: f32, arc_height: f32, frame_length: f32, arc_effect: bool) -> ArcStep {
    // C++ (weaponbolt.cpp:583-598):
    // if (arcEffect) {
    //     if (goalHeight > 0.0f) {
    //         float accel = -arcHeight * 10.0f;
    //         goalHeight += accel * frameLength;
    //         if (goalHeight >= 0.0f) { laserVelocity.z = goalHeight; isArcing = true; }
    //     }
    // }
    let mut goal_height = goal_height;
    let mut arcing = false;
    if arc_effect && goal_height > 0.0 {
        let accel = -arc_height * 10.0;
        goal_height += accel * frame_length;
        if goal_height >= 0.0 {
            arcing = true;
        }
    }
    ArcStep {
        goal_height,
        arcing,
    }
}

/// The vector added to `laserPosition` each frame while `!hitTarget`
/// (weaponbolt.cpp:1122-1141).
///
/// `to_target` is `targetPos - ownerPosition(laserPosition)`, the value
/// `laserVelocity` is initialised with. `vel_mag` is `type.velocity *
/// frameLength` (the caller computes it). `arc_z` is `laserVelocity.z`
/// after `arc_step` when arcing.
///
/// C++ order:
///
/// ```text
/// tmpZ = laserVelocity.z;
/// if (isArcing) { laserVelocity.z = 0.0f; tmpZ *= frameLength; }
/// if (laserVelocity.GetLength() != 0.0) laserVelocity.Normalize(laserVelocity);
/// if (moveThisFrame) { laserVelocity *= velMag; if (isArcing) laserVelocity.z = tmpZ; }
/// laserPosition += laserVelocity;
/// ```
///
/// QUIRK: when `!move_this_frame` (first frame) the *unit* vector is added,
/// not scaled by `vel_mag`. When not arcing the incoming z is kept (it is
/// `to_target.z`, normalised with the rest; `arc_z` is unused). When arcing
/// the incoming z is `arc_z`.
pub fn bolt_displacement(
    to_target: crate::math::Vec3,
    vel_mag: f32,
    arcing: bool,
    arc_z: f32,
    frame_length: f32,
    move_this_frame: bool,
) -> crate::math::Vec3 {
    use crate::math::Vec3Ext;
    // laserVelocity starts as targetPos - ownerPosition; when arcing, z was
    // overwritten with the arc velocity by arc_step (weaponbolt.cpp:592).
    let mut v = to_target;
    if arcing {
        v.z = arc_z;
    }
    let mut tmp_z = v.z;
    if arcing {
        v.z = 0.0;
        tmp_z *= frame_length;
    }
    if v.length() != 0.0 {
        // Stuff::Vector3D::Normalize: len = GetLength(); r = 1.0f / len; x*r, y*r, z*r.
        let r = 1.0 / v.length();
        v = crate::math::Vec3::from_xyz(v.x * r, v.y * r, v.z * r);
    }
    if move_this_frame {
        v = crate::math::Vec3::from_xyz(v.x * vel_mag, v.y * vel_mag, v.z * vel_mag);
        if arcing {
            v.z = tmp_z;
        }
    }
    v
}

/// Compute the squared xy distance between two points.
///
/// Ignores z coordinate: returns dx*dx + dy*dy.
pub fn squared_xy_distance(a: crate::math::Vec3, b: crate::math::Vec3) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    dx * dx + dy * dy
}

// -----------------------------------------------------------------------------------------------------------------------------
// WeaponBolt::update (weaponbolt.cpp:376-1570). Effects, sounds, lights and the render vertices are presentation and not ported.

use crate::combat::weapon_bolt::{
    bolt_params, bolt_state, bolt_state_mut, position_from_hs, set_target_position,
    target_aim_point, BoltParams,
};
use crate::events::{SimEvent, WeaponImpact};
use crate::ids::ObjectHandle;
use crate::math::Vec3;
use crate::object::{flags, Kind};
use crate::World;

/// Refresh the aim point from the target while it is alive (weaponbolt.cpp:430-445, 547-562): `setTargetPosition(hot spot)`.
fn refresh_target_position(w: &mut World, h: ObjectHandle) {
    let Some(b) = bolt_state(w, h) else { return };
    let (target, target_hs) = (b.target, b.hot_spots.1);
    let Some(t) = w.objects.by_watch_id(target) else {
        return;
    };
    let p = target_aim_point(w, t, target_hs);
    if let Some(b) = bolt_state_mut(w, h) {
        set_target_position(b, p);
    }
}

/// `position = myOwner->getPositionFromHS(hotSpotNumber)`: the bolt object follows its owner's hot spot while the owner lives.
fn follow_owner(w: &mut World, h: ObjectHandle) {
    let Some(b) = bolt_state(w, h) else { return };
    let (owner, hs) = (b.owner, b.hot_spots.0);
    if let Some(oh) = w.objects.by_watch_id(owner) {
        let p = position_from_hs(w, oh, hs);
        if let Some(o) = w.objects.get_mut(h) {
            o.c.position = p;
        }
    }
}

/// The first-frame block of `WeaponBolt::update` (weaponbolt.cpp:380-512): clears `JUSTCREATED` and `TANGIBLE`, puts the bolt at its
/// owner's hot spot, takes the first aim point, and computes `distanceToTarget`, `halfDistanceToTarget`, `goalHeight` and the beam
/// time. Returns `false` when the bolt was never connected (no target position): the C++ `return false`.
pub fn first_frame(w: &mut World, h: ObjectHandle) -> bool {
    let Some(params) = bolt_params(w, h) else {
        return false;
    };
    {
        let Some(o) = w.objects.get_mut(h) else {
            return false;
        };
        o.c.set_flag(flags::JUSTCREATED, false);
        o.c.set_flag(flags::TANGIBLE, false);
        if let Kind::WeaponBolt(b) = &mut o.kind {
            b.just_created = false;
            b.hit_target = false;
        }
    }
    follow_owner(w, h);
    let position = w.objects.get(h).map(|o| o.c.position).unwrap_or_default();
    // "If we were just Created, play our sound Effect" (weaponbolt.cpp:397-399): at the bolt's position, dupes allowed.
    if params.fire_sound != 0 {
        let source = w.objects.watch_id(h);
        w.events.push(crate::SimEvent::Sound {
            id: params.fire_sound,
            source,
            at: Some(position),
            allow_dupes: true,
        });
    }
    if let Some(b) = bolt_state_mut(w, h) {
        b.laser_position = position;
    }
    refresh_target_position(w, h);
    let Some(b) = bolt_state_mut(w, h) else {
        return false;
    };
    if !b.has_target_position {
        // Somehow, we are updating a fresh weaponbolt which has not been connected.
        return false;
    }
    b.time_left = if params.is_beam {
        params.beam_duration
    } else {
        0.0
    };
    // laserVelocity = targetPos - ownerPosition; the distance ignores z
    let distance = squared_xy_distance(position, b.target_position);
    b.distance_to_target = distance;
    b.half_distance_to_target = distance / 2.0;
    b.goal_height = params.arc_height * 10.0;
    let launch = SimEvent::WeaponEffect {
        shot_id: b.shot_id,
        effect_id: b.effect_id,
        owner: b.owner,
        from: position,
        to: b.target_position,
        impact: WeaponImpact::Launch,
    };
    // the muzzle flash and the flight effects start here (weaponbolt.cpp:477-502)
    w.events.push(launch);
    true
}

/// Frames the gosFX hit effect of a bolt that hit a target runs before `hitEffect->Execute()` returns false and the bolt retires
/// (weaponbolt.cpp:1360-1402, `inView = FALSE`); 0 when the row has no hit effect. The C++ makes the effect from the row's
/// `effects.csv` hit name (`GetEffectHitName`, weaponbolt.cpp:2402-2416, made for every bolt; `NONE` makes none), so the length
/// follows that name. The effect is presentation the port does not run, so its length is the gosFX one: `Generic_hit`, `ppc_hit`,
/// `Gauss_hit`, `AC_10_Hit`, `AC_20_Hit` and `Long_Tom_Hit` have a 10 s life span, `missile_hit` and `TBolt_hit` 5 s, `mg_hit` and
/// `Flamer_hit` 3 s and 3.5 s plus their particles (101..113 and 105..114 frames, median 108 = 3.6 s; `mc2_fx` playing each
/// effect at 30 fps, `docs/evidence/rc-mc2-24-kills.md`, "mc2_20"). The oracle per-shot trace (mc2_24 both variants x seeds 1..10,
/// "Oracle per-shot trace") agrees on the rows it saw: 300 or 301 frames for the 10 s rows, 150 or 151 for rows 8 and 9, 99..118 for
/// row 11. The 300/301 split follows the effect's phase against `scenarioTime`, not the row; the shorter one is taken (a pool wrap
/// at hit + 300 re-hits, at hit + 301 it does not: 1,998 of 1,998 target-hit beams).
pub fn hit_effect_frames(w: &World, effect_id: i32) -> u32 {
    let seconds = match effect_row_name(w, effect_id, 2).as_str() {
        "none" | "" => return 0,
        "missile_hit" | "tbolt_hit" => 5.0,
        "mg_hit" | "flamer_hit" => 3.6,
        _ => 10.0,
    };
    (seconds / w.clock.frame_length).round() as u32
}

/// Column `col` of the bolt's `effects.csv` row (0 the flight effect, 2 the hit effect), lower case; empty for a row out of range.
fn effect_row_name(w: &World, effect_id: i32, col: usize) -> String {
    usize::try_from(effect_id)
        .ok()
        .and_then(|i| w.objects.effect_rows.get(i))
        .map_or_else(String::new, |r| r.names[col].to_ascii_lowercase())
}

/// Whether the bolt has a flight gosFX (`gosEffect`, weaponbolt.cpp:2360-2378: made when `GetEffectName` is not `NONE`).
fn has_flight_effect(w: &World, effect_id: i32) -> bool {
    !matches!(effect_row_name(w, effect_id, 0).as_str(), "none" | "")
}

/// `WeaponBolt::update` for a live bolt. Returns whether the bolt still exists. A bolt that has hit stays alive as the C++ one does
/// (`update` returns `inView`): while its hit effect runs ([`hit_effect_frames`], only with a target) or its flight gosFX does (until
/// `hitLeft` goes negative). A wrap of the weapon pool in that time hits its target again (`finishNow`, `bolt_impact::finish_now`).
/// `WB` in the digest counts only bolts in flight.
pub fn update(w: &mut World, h: ObjectHandle) -> bool {
    if !w
        .objects
        .get(h)
        .is_some_and(|o| o.exists() && matches!(o.kind, Kind::WeaponBolt(_)))
    {
        return false;
    }
    let Some(params) = bolt_params(w, h) else {
        return false;
    };
    let frame_length = w.clock.frame_length;
    let mut move_this_frame = true;
    if bolt_state(w, h).is_some_and(|b| b.just_created) {
        if !first_frame(w, h) {
            return false;
        }
        move_this_frame = false;
    }
    flight(w, h, &params, frame_length, move_this_frame)
}

/// The part of `WeaponBolt::update` every frame runs (weaponbolt.cpp:514-640, 1118-1145, 1341-1345).
fn flight(
    w: &mut World,
    h: ObjectHandle,
    params: &BoltParams,
    frame_length: f32,
    move_this_frame: bool,
) -> bool {
    // Update position relative to my owner
    follow_owner(w, h);
    if params.is_beam {
        if let Some(b) = bolt_state_mut(w, h) {
            b.time_left -= frame_length;
        }
    }
    // Keep projectile flying toward target. Even if Target Moves!!
    let (owner_position, target, hit_before) = {
        let b = bolt_state(w, h).expect("bolt");
        (b.laser_position, b.target, b.hit_target)
    };
    let target_obj = w.objects.by_watch_id(target);
    if target_obj.is_some() && (!hit_before || params.is_beam) {
        refresh_target_position(w, h);
    }
    let b = bolt_state_mut(w, h).expect("bolt");
    let target_pos = if b.has_target_position {
        b.target_position
    } else {
        Vec3::default()
    };
    let vel_mag = params.velocity * frame_length;
    let to_target = Vec3 {
        x: target_pos.x - owner_position.x,
        y: target_pos.y - owner_position.y,
        z: target_pos.z - owner_position.z,
    };
    let distance = squared_xy_distance(owner_position, target_pos);

    // For the first half of the distance, bring weapons up if NOT beam!!!
    let mut arcing = false;
    if params.arc_effect {
        let step = arc_step(b.goal_height, params.arc_height, frame_length, true);
        b.goal_height = step.goal_height;
        arcing = step.arcing;
    }
    let arc_z = b.goal_height;

    // Make the bullets go away when they get close enough to the target.
    let mut hit_now = false;
    if !b.hit_target {
        if !params.is_beam {
            let c = bolt_crossing(b.distance_to_target, distance);
            b.distance_to_target = c.distance_to_target;
            if c.hit {
                b.hit_target = true;
                b.hit_left = params.after_hit_time;
                hit_now = true;
            }
        } else if b.time_left < 0.0 {
            b.hit_target = true;
            hit_now = true;
        }
    }
    if hit_now {
        // Put effect ON the spot!! A beam keeps its `laserPosition` (the C++ state), but its impact is on the target's hot spot too.
        let spot = if let Some(t) = target_obj {
            target_aim_point(w, t, bolt_state(w, h).expect("bolt").hot_spots.1)
        } else {
            bolt_state(w, h).expect("bolt").target_position
        };
        if !params.is_beam {
            bolt_state_mut(w, h).expect("bolt").laser_position = spot;
        }
        // the impact event is pushed by `bolt_impact::on_hit` (Hit/Miss)
        crate::combat::bolt_impact::on_hit(w, h);
    }

    // Actually move the effect toward the target. But only AFTER the first Start for the effect
    let beam_wiggle = params.beam_wiggle;
    let b = bolt_state_mut(w, h).expect("bolt");
    if !b.hit_target {
        let d = bolt_displacement(
            to_target,
            vel_mag,
            arcing,
            arc_z,
            frame_length,
            move_this_frame,
        );
        b.laser_position = Vec3 {
            x: b.laser_position.x + d.x,
            y: b.laser_position.y + d.y,
            z: b.laser_position.z + d.z,
        };
        if params.is_beam {
            // Wiggle the hotspot: three draws on the visual stream (visual_rng_sites.txt: weaponbolt.cpp:1304-1306), x, y, z.
            // `RandomNumber(offset * 2)` takes the float product converted to a long.
            let range = (beam_wiggle * 2.0) as i32;
            let streams = w.rng.0.clone();
            let mut r = streams.borrow_mut();
            for _ in 0..3 {
                r.visual.random_number(range);
            }
        }
    }
    let effect_id = bolt_state(w, h).expect("bolt").effect_id;
    let hit_effect = if hit_now && target_obj.is_some() {
        hit_effect_frames(w, effect_id)
    } else {
        0
    };
    let flight_effect = has_flight_effect(w, effect_id);
    let b = bolt_state_mut(w, h).expect("bolt");
    if b.hit_target {
        // weaponbolt.cpp:1341-1356: the flight gosFX is killed once `hitLeft` goes negative. A beam never sets `hitLeft` (only the
        // projectile hit does, :612), so a beam's is the stale value of its slot and its gosFX normally ends on the hit frame.
        b.hit_left -= frame_length;
        // the hit effect ending sets `inView = FALSE` whatever the flight effect does (:1392-1402)
        let mut hit_effect_ended = false;
        if hit_now {
            // the hit effect starts only on a target (:614, :886); without one it is deleted at the hit (:975-980)
            b.hit_effect_left = hit_effect;
        } else if b.hit_effect_left > 0 {
            b.hit_effect_left -= 1;
            hit_effect_ended = b.hit_effect_left == 0;
        }
        // :1591-1592: no effect left, so `inView = false`. A row without a flight effect has no `gosEffect` to wait for.
        if !hit_effect_ended && (b.hit_effect_left > 0 || (flight_effect && b.hit_left >= 0.0)) {
            return true;
        }
        if let Some(o) = w.objects.get_mut(h) {
            o.set_exists(false);
        }
        return false;
    }
    true
}
