// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tests for bolt math: `bolt_crossing` and `squared_xy_distance`.

use mc2_sim::combat::bolt_flight::{
    arc_step, bolt_crossing, bolt_displacement, squared_xy_distance,
};
use mc2_sim::math::{Vec3, Vec3Ext};

/// Test the squared xy distance calculation.
#[test]
fn squared_xy_distance_works() {
    let a = Vec3::from_xyz(1.0, 2.0, 9.0);
    let b = Vec3::from_xyz(4.0, 6.0, -3.0);
    assert_eq!(squared_xy_distance(a, b), 25.0);
}

/// Test the bolt crossing logic.
///
/// The C++ logic: lastDistanceMoved = distanceToTarget - distance; distanceToTarget = distance;
/// if (lastDistanceMoved >= distance) hitTarget = TRUE;
///
/// Returns hit = (prev - distance) >= distance and distance_to_target = distance.
#[test]
fn bolt_crossing_uses_the_squared_xy_distance_rule() {
    // (prev 100, dist 40) -> hit (60 >= 40), dist 40
    let result = bolt_crossing(100.0, 40.0);
    assert!(result.hit);
    assert_eq!(result.distance_to_target, 40.0);

    // (100, 60) -> no hit (40 < 60), dist 60
    let result = bolt_crossing(100.0, 60.0);
    assert!(!result.hit);
    assert_eq!(result.distance_to_target, 60.0);

    // (100, 50) -> hit boundary (50 >= 50), dist 50
    let result = bolt_crossing(100.0, 50.0);
    assert!(result.hit);
    assert_eq!(result.distance_to_target, 50.0);

    // (100, 50.5) -> no hit (49.5 < 50.5), dist 50.5
    let result = bolt_crossing(100.0, 50.5);
    assert!(!result.hit);
    assert_eq!(result.distance_to_target, 50.5);

    // (0, 0) -> hit (0 >= 0), dist 0
    let result = bolt_crossing(0.0, 0.0);
    assert!(result.hit);
    assert_eq!(result.distance_to_target, 0.0);

    // (10, 20) -> no hit (-10 < 20), dist 20
    let result = bolt_crossing(10.0, 20.0);
    assert!(!result.hit);
    assert_eq!(result.distance_to_target, 20.0);

    // (40, 0) -> hit (40 >= 0), dist 0
    let result = bolt_crossing(40.0, 0.0);
    assert!(result.hit);
    assert_eq!(result.distance_to_target, 0.0);
}

/// Test the arcing bolt vertical velocity step.
///
/// The C++ logic (weaponbolt.cpp:583-598): with `arcEffect` and
/// `goalHeight > 0.0f`, `accel = -arcHeight * 10.0f;` and
/// `goalHeight += accel * frameLength;`. Arcing is true only when the new
/// value is `>= 0.0f`; a negative value is stored but arcing stays false.
#[test]
fn arc_step_integrates_vertical_velocity() {
    // arc_height 2.0, frame_length 0.5: accel = -20.0, delta = -10.0 per step.
    // Start goal 20.0 -> 10.0 (arcing) -> 0.0 (arcing: 0 >= 0) -> stays 0.0, not arcing.
    let s = arc_step(20.0, 2.0, 0.5, true);
    assert_eq!(s.goal_height, 10.0);
    assert!(s.arcing);

    let s = arc_step(s.goal_height, 2.0, 0.5, true);
    assert_eq!(s.goal_height, 0.0);
    assert!(s.arcing);

    // Third call: goal 0.0 is NOT > 0, so nothing changes and arcing is false.
    let s = arc_step(s.goal_height, 2.0, 0.5, true);
    assert_eq!(s.goal_height, 0.0);
    assert!(!s.arcing);

    // Start 4.0: 4.0 - 10.0 = -6.0 is stored, but arcing is false (-6.0 < 0).
    let s = arc_step(4.0, 2.0, 0.5, true);
    assert_eq!(s.goal_height, -6.0);
    assert!(!s.arcing);

    // Next call with -6.0: not > 0, so unchanged and not arcing.
    let s = arc_step(-6.0, 2.0, 0.5, true);
    assert_eq!(s.goal_height, -6.0);
    assert!(!s.arcing);

    // arc_effect false with goal 20.0: unchanged, not arcing.
    let s = arc_step(20.0, 2.0, 0.5, false);
    assert_eq!(s.goal_height, 20.0);
    assert!(!s.arcing);
}

/// Test the per-frame displacement vector (weaponbolt.cpp:1122-1141).
///
/// All lengths are powers of two so every value is exact.
#[test]
fn bolt_displacement_matches_the_c_order() {
    // Not arcing, moving: normalise (0,8,0) -> (0,1,0), scale by vel_mag 4.
    let d = bolt_displacement(Vec3::from_xyz(0.0, 8.0, 0.0), 4.0, false, 0.0, 0.5, true);
    assert_eq!((d.x, d.y, d.z), (0.0, 4.0, 0.0));

    // QUIRK: first frame (!moveThisFrame) adds the unit vector, not scaled.
    let d = bolt_displacement(Vec3::from_xyz(0.0, 8.0, 0.0), 4.0, false, 0.0, 0.5, false);
    assert_eq!((d.x, d.y, d.z), (0.0, 1.0, 0.0));

    // Arcing, moving: z is arc_z, zeroed before normalise, then restored as
    // tmpZ = arc_z * frame_length: (0,8,0)->(0,1,0)*4, z = 6.0*0.5 = 3.0.
    let d = bolt_displacement(Vec3::from_xyz(0.0, 8.0, 99.0), 4.0, true, 6.0, 0.5, true);
    assert_eq!((d.x, d.y, d.z), (0.0, 4.0, 3.0));

    // Arcing, first frame: the tmpZ assignment is inside the moveThisFrame
    // branch, so z stays 0 and the unit vector is added unscaled.
    let d = bolt_displacement(Vec3::from_xyz(0.0, 8.0, 99.0), 4.0, true, 6.0, 0.5, false);
    assert_eq!((d.x, d.y, d.z), (0.0, 1.0, 0.0));

    // Zero length skips normalisation: (0,0,0) * vel_mag = (0,0,0).
    let d = bolt_displacement(Vec3::from_xyz(0.0, 0.0, 0.0), 4.0, false, 0.0, 0.5, true);
    assert_eq!((d.x, d.y, d.z), (0.0, 0.0, 0.0));
}
