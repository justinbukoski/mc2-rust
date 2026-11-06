// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The elevation head-shot percent of `WeaponBolt::update` (weaponbolt.cpp:732-760, duplicated at 984-1010).

use mc2_sim::combat::bolt_impact::{apply_head_shot, head_shot_percent};
use mc2_sim::combat::WeaponShotInfo;
use mc2_sim::ids::WatchId;
use mc2_sim::math::{Vec3, Vec3Ext};
use mc2_sim::rng::RngMode;
use mc2_sim::World;

/// `head_shot_percent_follows_the_elevation_angle`: the percent is `trunc(atan2(dz, groundDistance) * RADS_TO_DEGREES *
/// BaseHeadShotElevation)`, and only when the target sits below the owner.
#[test]
fn head_shot_percent_follows_the_elevation_angle() {
    // owner (0,0,10), target (10,0,0): dz = 10, the ground distance is GetApproximateLength() of (10,0,0), i.e. sqrt(100) = 10.
    let owner = Vec3::from_xyz(0.0, 0.0, 10.0);
    let target = Vec3::from_xyz(10.0, 0.0, 0.0);
    let p = head_shot_percent(owner, target, 1.0).expect("the target is below the owner");
    // atan2(10, 10) is pi/4; the portable arctangent is exact to f32 here and the product with RADS_TO_DEGREES is pinned (golden):
    // the same integer on every platform, with no 44-or-45 tolerance.
    assert_eq!(p, 45);
    assert_eq!(
        mc2_sim::math::portable_atan2(10.0, 10.0).to_bits(),
        0x3f49_0fdb
    );

    // Target above the owner, and target at the same elevation: no roll at all (the C++ `if` never runs).
    assert_eq!(
        head_shot_percent(owner, Vec3::from_xyz(0.0, 0.0, 12.0), 1.0),
        None
    );
    assert_eq!(
        head_shot_percent(owner, Vec3::from_xyz(0.0, 0.0, 10.0), 1.0),
        None
    );

    // BaseHeadShotElevation 2.0 doubles the percent (45 -> 90; the C++ default is 1.0, mission.cpp:300).
    let doubled = head_shot_percent(owner, target, 2.0).expect("still below");
    assert_eq!(doubled, 90);

    // owner (0,0,10), target (20,0,0): atan(10/20) = 26.565 degrees -> 26.
    assert_eq!(
        head_shot_percent(owner, Vec3::from_xyz(20.0, 0.0, 0.0), 1.0),
        Some(26)
    );

    // A target straight below: yDistance = 0 is not > Stuff::SMALL, so mc2_atan2 returns its hard-coded 1.570796f and the angle
    // is 89.999985 -> 89, not 90.
    assert_eq!(
        head_shot_percent(owner, Vec3::from_xyz(0.0, 0.0, 0.0), 1.0),
        Some(89)
    );
}

/// `apply_head_shot` draws exactly one sim number when there is a percent, none when there is not, and sets the head location on
/// a success.
#[test]
fn apply_head_shot_draws_one_number_only_with_a_percent() {
    let mut w = World::new(RngMode::LegacyMsvc, 1);
    let shot = |loc: i32| WeaponShotInfo {
        attacker: WatchId::NONE,
        master_id: 0,
        damage: 1.0,
        hit_location: loc,
        entry_angle: 0.0,
    };

    // No percent: no draw.
    let mut shot_no = shot(3);
    apply_head_shot(&mut w, &mut shot_no, None);
    assert_eq!(w.rng.0.borrow().sim.draws(), 0);
    assert_eq!(shot_no.hit_location, 3);

    // percent 100 always hits ((gos_rand() * 100) >> 15 <= 99 < 100) and draws exactly one number.
    let mut shot_yes = shot(3);
    apply_head_shot(&mut w, &mut shot_yes, Some(100));
    assert_eq!(w.rng.0.borrow().sim.draws(), 1);
    assert_eq!(shot_yes.hit_location, 0);

    // percent 0 still draws (the C++ RollDice always calls gos_rand) and cannot hit.
    let mut shot_zero = shot(3);
    apply_head_shot(&mut w, &mut shot_zero, Some(0));
    assert_eq!(w.rng.0.borrow().sim.draws(), 2);
    assert_eq!(shot_zero.hit_location, 3);
}

/// The portable arctangent against the quadrant/axis identities and a dense sweep versus the platform `atan2` (tolerance 2 f32 ulps;
/// the platform value is a cross-check only, the sim never uses it).
#[test]
fn portable_atan2_is_accurate_and_total() {
    use mc2_sim::math::portable_atan2 as at;
    assert_eq!(at(0.0, 0.0), 0.0);
    assert_eq!(at(0.0, 1.0), 0.0);
    assert_eq!(at(1.0, 0.0), std::f32::consts::FRAC_PI_2);
    assert_eq!(at(0.0, -1.0), std::f32::consts::PI);
    assert_eq!(at(-1.0, 0.0), -std::f32::consts::FRAC_PI_2);
    for i in 1..400 {
        for j in [-3.0f32, -0.5, 0.25, 1.0, 7.0, 130.0] {
            let y = i as f32 * 0.37;
            let want = y.atan2(j);
            let got = at(y, j);
            assert!(
                (got - want).abs() <= 2.0 * f32::EPSILON * want.abs().max(1.0),
                "atan2({y},{j}) = {got}, libm {want}"
            );
        }
    }
}
