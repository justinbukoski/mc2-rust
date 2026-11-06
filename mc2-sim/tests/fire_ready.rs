// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tests for weapon ready/recycle and range functions (mover.cpp:5549, 5990, 6001).

use mc2_sim::combat::fire_math::{
    is_weapon_ready, source_hot_spot, start_weapon_recycle, weapon_in_range,
};
use mc2_sim::combat::InventoryItem;

/// `ready_recycle_and_range_follow_the_c_comparisons`: verify the three functions match C++ behavior exactly.
#[test]
fn ready_recycle_and_range_follow_the_c_comparisons() {
    // is_weapon_ready tests
    // item ready_time 5.0: ready at time 5.0 (not >)
    let item = InventoryItem {
        disabled: false,
        ready_time: 5.0,
        ..Default::default()
    };
    assert!(is_weapon_ready(item, 5.0));

    // not ready at 4.9
    assert!(!is_weapon_ready(item, 4.9));

    // ready at 5.1
    assert!(is_weapon_ready(item, 5.1));

    // disabled item never ready
    let disabled_item = InventoryItem {
        disabled: true,
        ready_time: 5.0,
        ..Default::default()
    };
    assert!(!is_weapon_ready(disabled_item, 5.0));
    assert!(!is_weapon_ready(disabled_item, 4.9));
    assert!(!is_weapon_ready(disabled_item, 5.1));

    // start_weapon_recycle tests
    let mut item = InventoryItem {
        disabled: false,
        ready_time: 5.0,
        ..Default::default()
    };
    start_weapon_recycle(&mut item, 10.0, 2.5);
    assert_eq!(item.ready_time, 12.5);

    // not ready at 12.0
    assert!(!is_weapon_ready(item, 12.0));

    // ready at 12.5
    assert!(is_weapon_ready(item, 12.5));

    // weapon_in_range tests with range 30-270
    // buffer 0: 30.0 true (inclusive), 29.9 false, 270.0 true, 270.1 false
    assert!(weapon_in_range(30.0, 270.0, 30.0, 0.0));
    assert!(!weapon_in_range(30.0, 270.0, 29.9, 0.0));
    assert!(weapon_in_range(30.0, 270.0, 270.0, 0.0));
    assert!(!weapon_in_range(30.0, 270.0, 270.1, 0.0));

    // buffer 5: 25.0 true, 24.9 false, 275.0 true, 275.1 false
    assert!(weapon_in_range(30.0, 270.0, 25.0, 5.0));
    assert!(!weapon_in_range(30.0, 270.0, 24.9, 5.0));
    assert!(weapon_in_range(30.0, 270.0, 275.0, 5.0));
    assert!(!weapon_in_range(30.0, 270.0, 275.1, 5.0));
}

/// `hot_spot_is_forced_into_the_torso_when_an_arm_is_gone`: verify the two sequential remap ifs
/// of mech.cpp:8027-8032, where the starting hot spot is the item facing.
#[test]
fn hot_spot_is_forced_into_the_torso_when_an_arm_is_gone() {
    // Right arm gone: right-arm weapon (1) is forced into the right torso (3).
    assert_eq!(source_hot_spot(1, true, false), 3);

    // Both arms on: facing is used as-is.
    assert_eq!(source_hot_spot(1, false, false), 1);

    // Left arm gone: left-arm weapon (2) is forced into the left torso (5).
    assert_eq!(source_hot_spot(2, false, true), 5);

    // Left arm gone does not touch a right-arm weapon.
    assert_eq!(source_hot_spot(2, true, false), 2);

    // Both arms gone: the left check sees 3 (written by the right check), not 2, so it stays 3.
    assert_eq!(source_hot_spot(1, true, true), 3);

    // Both arms gone with a left-arm facing: only the left remap applies.
    assert_eq!(source_hot_spot(2, true, true), 5);

    // Torso and other hot spots are never remapped.
    assert_eq!(source_hot_spot(3, true, true), 3);
    assert_eq!(source_hot_spot(0, true, true), 0);
}
