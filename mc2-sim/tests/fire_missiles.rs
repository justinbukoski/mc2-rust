// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Missile-count rounding of `BattleMech::fireWeapon` (`code/mech.cpp:8100-8120` hit branch, `code/mech.cpp:8335-8345` miss
//! branch). Expected values are derived by hand from the C++, not from the code under test.
//!
//! Hit, non-streak (mech.cpp:8103): `numMissiles = (long)(missileAmount / 2.0 + 0.5)` (double arithmetic, C-style
//! truncation toward zero), then `if (numMissiles < 1) numMissiles = 1;` then `if (numMissiles > missileAmount)
//! numMissiles = missileAmount;`. Hit, streak: `numMissiles = missileAmount` (no clamps at all).
//! Miss (mech.cpp:8338): `numMissiles = ((float)missileAmount / 2.0) + 0.5;` assigned to a `long` (same truncation), then
//! the same `< 1` then `> amount` clamps; the miss branch is never streak-aware.
//!
//! Hand derivation, non-streak: amount/2.0 + 0.5 is 1.0 for 1, 1.5 for 2, 2.0 for 3, 2.5 for 4, 3.0 for 5, 3.5 for 6,
//! 5.5 for 10, 10.5 for 20, 0.5 for 0. Truncation gives 1, 1, 2, 2, 3, 3, 5, 10, 0. The `< 1` clamp raises the 0 to 1;
//! the `> amount` clamp then lowers it back to amount 0 (the upper clamp runs last). So: 1->1, 2->1, 3->2, 4->2, 5->3,
//! 6->3, 10->5, 20->10, 0->0. Streak: the amount itself, so 6->6, 4->4, 0->0.

use mc2_sim::combat::fire_math::{
    miss_offset_points, miss_radius, missile_count_hit, missile_count_miss,
};

#[test]
fn missile_counts_follow_the_c_rounding() {
    // Hit, non-streak: (long)(amount / 2.0 + 0.5) with the <1 then >amount clamps.
    assert_eq!(missile_count_hit(1, false), 1);
    assert_eq!(missile_count_hit(2, false), 1);
    assert_eq!(missile_count_hit(3, false), 2);
    assert_eq!(missile_count_hit(4, false), 2);
    assert_eq!(missile_count_hit(5, false), 3);
    assert_eq!(missile_count_hit(6, false), 3);
    assert_eq!(missile_count_hit(10, false), 5);
    assert_eq!(missile_count_hit(20, false), 10);
    // amount 0: 0.5 truncates to 0, the <1 clamp raises it to 1, the >amount clamp lowers it back to 0.
    assert_eq!(missile_count_hit(0, false), 0);

    // Hit, streak: numMissiles = missileAmount, no clamps.
    assert_eq!(missile_count_hit(6, true), 6);
    assert_eq!(missile_count_hit(4, true), 4);
    assert_eq!(missile_count_hit(0, true), 0);

    // Miss: same table as hit non-streak; the miss branch never sees isStreakMissile.
    assert_eq!(missile_count_miss(1), 1);
    assert_eq!(missile_count_miss(2), 1);
    assert_eq!(missile_count_miss(3), 2);
    assert_eq!(missile_count_miss(4), 2);
    assert_eq!(missile_count_miss(5), 3);
    assert_eq!(missile_count_miss(6), 3);
    assert_eq!(missile_count_miss(10), 5);
    assert_eq!(missile_count_miss(20), 10);
    assert_eq!(missile_count_miss(0), 0);
}

#[test]
fn miss_offsets_are_cumulative() {
    // Test the four miss offset points for a target at (100,200,5) with r=25
    let base = mc2_sim::math::Vec3 {
        x: 100.0,
        y: 200.0,
        z: 5.0,
    };
    let r = 25.0;
    let points = miss_offset_points(base, r);
    assert_eq!(
        points,
        [
            (125.0, 200.0),
            (75.0, 200.0),
            (100.0, 225.0),
            (100.0, 175.0)
        ]
    );

    // Test with small values: base (0.5,0.25,0), r 5 -> [(5.5,0.25),(-4.5,0.25),(0.5,5.25),(0.5,-4.75)]
    let base2 = mc2_sim::math::Vec3 {
        x: 0.5,
        y: 0.25,
        z: 0.0,
    };
    let r2 = 5.0;
    let points2 = miss_offset_points(base2, r2);
    assert_eq!(
        points2,
        [(5.5, 0.25), (-4.5, 0.25), (0.5, 5.25), (0.5, -4.75)]
    );

    // Test miss_radius function
    assert_eq!(miss_radius(true), 25.0);
    assert_eq!(miss_radius(false), 5.0);
}
