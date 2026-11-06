// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `CallArtillery` / `IfaceCallStrike` parameters (artlry.cpp:99-232): strike-type normalisation, time to impact, and the cap.

use mc2_sim::combat::artillery_math::{
    normalize_strike, seconds_from_time_to_impact, single_player_strike_cap, strike_time_to_impact,
    ARTILLERY_LARGE, ARTILLERY_SENSOR, ARTILLERY_SMALL,
};

#[test]
fn strike_parameters_follow_call_artillery() {
    // Enum order (dartlry.h).
    assert_eq!(
        (ARTILLERY_SMALL, ARTILLERY_LARGE, ARTILLERY_SENSOR),
        (0, 1, 2)
    );

    // IfaceCallStrike's type normalisation.
    assert_eq!(
        normalize_strike(ARTILLERY_SMALL, false),
        Some(ARTILLERY_LARGE)
    );
    assert_eq!(
        normalize_strike(ARTILLERY_SMALL, true),
        Some(ARTILLERY_SMALL)
    );
    assert_eq!(
        normalize_strike(ARTILLERY_LARGE, true),
        Some(ARTILLERY_SMALL)
    );
    assert_eq!(
        normalize_strike(ARTILLERY_LARGE, false),
        Some(ARTILLERY_LARGE)
    );
    assert_eq!(
        normalize_strike(ARTILLERY_SENSOR, true),
        Some(ARTILLERY_SENSOR)
    );
    assert_eq!(normalize_strike(3, false), None);
    assert_eq!(normalize_strike(-1, true), None);

    // CallArtillery's time to impact: -1 and anything under 2 s is the full-duration marker.
    assert_eq!(strike_time_to_impact(-1), -1.0);
    assert_eq!(strike_time_to_impact(0), -1.0);
    assert_eq!(strike_time_to_impact(1), -1.0);
    assert_eq!(strike_time_to_impact(2), 2.0);
    assert_eq!(strike_time_to_impact(7), 7.0);

    // IfaceCallStrike truncates the float before it reaches CallArtillery.
    assert_eq!(seconds_from_time_to_impact(3.9), 3);
    assert_eq!(seconds_from_time_to_impact(-1.0), -1);
    assert_eq!(seconds_from_time_to_impact(0.5), 0);

    // Single player: no strike cap.
    assert_eq!(single_player_strike_cap(), None);
}

#[test]
fn artillery_hit_plan_and_extent_gate() {
    use mc2_path::Vec3;
    use mc2_sim::combat::artillery_math::{
        artillery_hit_plan, extent_blocks_damage, range_meters, HitPlan,
    };

    // Major range 10, major hits 3.0, minor hits 1.0, head range 4.
    let plan = |range| artillery_hit_plan(range, 10.0, 3.0, 1.0, 4.0);
    assert_eq!(
        plan(3.0),
        HitPlan {
            count: 3,
            head_range: true
        }
    );
    assert_eq!(
        plan(8.0),
        HitPlan {
            count: 3,
            head_range: false
        }
    );
    // Boundary: range == nominal major range is still a major hit.
    assert_eq!(
        plan(10.0),
        HitPlan {
            count: 3,
            head_range: false
        }
    );
    assert_eq!(
        plan(10.5),
        HitPlan {
            count: 1,
            head_range: false
        }
    );
    // numHits = (long)float truncates.
    assert_eq!(
        artillery_hit_plan(10.5, 10.0, 3.0, 1.9, 4.0),
        HitPlan {
            count: 1,
            head_range: false
        }
    );

    // Extent gate: little extent 2.
    assert!(!extent_blocks_damage(2.0, 1.0, 2.0));
    // 2 < 2 is false, so not blocked.
    assert!(!extent_blocks_damage(2.0, 2.0, 2.0));
    // realRange 3 > major 2: blocked.
    assert!(extent_blocks_damage(2.0, 5.0, 2.0));
    // realRange 2 > major 2 is false: not blocked.
    assert!(!extent_blocks_damage(2.0, 4.0, 2.0));

    // Range ignores elevation (z).
    let origin = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };
    let collider = Vec3 {
        x: 3.0,
        y: 4.0,
        z: 50.0,
    };
    assert_eq!(range_meters(collider, origin, 1.0), 5.0);
    assert_eq!(range_meters(collider, origin, 0.5), 2.5);
}
