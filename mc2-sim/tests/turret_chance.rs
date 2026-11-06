// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tests for turret attack chance: `Turret::calcAttackChance` (`code/turret.cpp:997-1087`).

use mc2_sim::combat::turret_math::{turret_attack_chance, MechSpec, TurretChance};

const SHORT: f32 = 90.0;
const MEDIUM: f32 = 180.0;
const MIN: f32 = 30.0;
const MAX: f32 = 270.0;

fn no_specialist(tonnage: f32) -> MechSpec {
    MechSpec {
        tonnage,
        light: false,
        medium: false,
        heavy: false,
        assault: false,
    }
}

/// Port of the C++ rules: skill, movement bonus, specialist malus, clamp and range checks.
#[test]
fn turret_chance_follows_the_c_rules() {
    let chance = |skill: f32, dist: f32, speed: Option<f32>, mech: Option<MechSpec>| {
        turret_attack_chance(0, skill, dist, SHORT, MEDIUM, MIN, MAX, speed, mech).chance
    };

    // Stationary non-mech: skill + 50.
    assert_eq!(chance(40.0, 100.0, Some(0.0), None), 90.0);
    // Moving non-mech: skill only.
    assert_eq!(chance(40.0, 100.0, Some(4.0), None), 40.0);
    // No target: skill only.
    assert_eq!(chance(40.0, 100.0, None, None), 40.0);

    // Light mech 30 t with light specialist: -30.
    let light = MechSpec {
        tonnage: 30.0,
        light: true,
        ..no_specialist(30.0)
    };
    assert_eq!(chance(40.0, 100.0, Some(0.0), Some(light)), 60.0);
    assert_eq!(chance(40.0, 100.0, Some(4.0), Some(light)), 10.0);

    // Skill 20, 50 t medium specialist, moving: 20 - 20 = 0, clamped to 5.
    let medium = MechSpec {
        tonnage: 50.0,
        medium: true,
        ..no_specialist(50.0)
    };
    assert_eq!(chance(20.0, 100.0, Some(4.0), Some(medium)), 5.0);

    // Skill 20, mech without matching specialist flag: no malus, no clamp.
    let heavy_no_flag = MechSpec {
        tonnage: 70.0,
        light: true,
        ..no_specialist(70.0)
    };
    assert_eq!(chance(20.0, 100.0, Some(4.0), Some(heavy_no_flag)), 20.0);

    // Skill 2, mech 30 t with no specialist, moving: clamp applies to any mech.
    assert_eq!(
        chance(2.0, 100.0, Some(4.0), Some(no_specialist(30.0))),
        5.0
    );

    // Range limits: exactly at min is out of range; exactly at max is in range; beyond max is out.
    assert_eq!(chance(40.0, MIN, Some(4.0), None), -1.0);
    assert_eq!(chance(40.0, MAX, Some(4.0), None), 40.0);
    assert_eq!(chance(40.0, 270.5, Some(4.0), None), -1.0);

    // Range classes: short <= 90, medium <= 180, long otherwise.
    let class = |dist: f32| {
        turret_attack_chance(0, 40.0, dist, SHORT, MEDIUM, MIN, MAX, Some(4.0), None).range_class
    };
    assert_eq!(class(90.0), Some(0));
    assert_eq!(class(91.0), Some(1));
    assert_eq!(class(180.0), Some(1));
    assert_eq!(class(181.0), Some(2));

    // No weapon: chance 0 and no range class.
    assert_eq!(
        turret_attack_chance(-1, 40.0, 100.0, SHORT, MEDIUM, MIN, MAX, Some(0.0), None),
        TurretChance {
            chance: 0.0,
            range_class: None
        }
    );
}
