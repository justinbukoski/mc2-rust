// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tests for `fire_gate` in `mc2_sim::combat::fire_math`.
//!
//! Test-first port of `BattleMech::fireWeapon` early-return gates (`code/mech.cpp:7926-8040, 8070-8075`).

use mc2_sim::combat::fire_math::{fire_gate, FireGateInput};
use mc2_sim::object::ObjectStatus;

/// Helper building a passing input (status Normal, not parked, ready, on terrain, target Some(false),
/// no point, in range, num_shots 9999, node_recycle 0.0 -> None).
fn passing_input() -> FireGateInput {
    FireGateInput {
        status: ObjectStatus::Normal,
        parked_or_fallen: false,
        weapon_ready: true,
        on_game_terrain: true,
        target_destroyed: Some(false),
        has_target_point: false,
        in_range: true,
        num_shots: 9999,
        node_recycle: 0.0,
    }
}

/// `fire_gate` returns codes in C++ order.
///
/// Expected ordering cases:
/// - disabled + not ready -> 1
/// - not ready + off terrain -> 3
/// - off terrain + destroyed target -> 1
/// - destroyed target + !in_range -> 4 (any 4)
/// - target None & no point & !in_range -> 4
/// - num_shots 0 + node_recycle 0.5 -> 4
/// - num_shots -1 + node_recycle 0.01 -> 3
/// - ShutDown, ShuttingDown, Disabled, Destroyed each -> 1
/// - StartingUp passes (None)
/// - target None but has_target_point true and everything else fine -> None
#[test]
fn fire_gate_returns_codes_in_c_order() {
    // Test 1: disabled status should return 1
    let g = FireGateInput {
        status: ObjectStatus::Disabled,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(1));

    // Test 2: not ready + off terrain -> 3
    let g = FireGateInput {
        weapon_ready: false,
        on_game_terrain: false,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(3));

    // Test 3: off terrain + destroyed target -> 1 (status check comes first)
    let g = FireGateInput {
        status: ObjectStatus::Destroyed,
        on_game_terrain: false,
        target_destroyed: Some(true),
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(1));

    // Test 4: destroyed target + !in_range -> 4
    let g = FireGateInput {
        target_destroyed: Some(true),
        in_range: false,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(4));

    // Test 5: target None & no point & !in_range -> 4
    let g = FireGateInput {
        target_destroyed: None,
        has_target_point: false,
        in_range: false,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(4));

    // Test 6: num_shots 0 + node_recycle 0.5 -> 4 (num_shots check comes before node_recycle)
    let g = FireGateInput {
        num_shots: 0,
        node_recycle: 0.5,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(4));

    // Test 7: num_shots -1 + node_recycle 0.01 -> 3 (node_recycle check)
    let g = FireGateInput {
        num_shots: -1,
        node_recycle: 0.01,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(3));

    // Test 8: ShutDown -> 1
    let g = FireGateInput {
        status: ObjectStatus::ShutDown,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(1));

    // Test 9: ShuttingDown -> 1
    let g = FireGateInput {
        status: ObjectStatus::ShuttingDown,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(1));

    // Test 10: Disabled -> 1
    let g = FireGateInput {
        status: ObjectStatus::Disabled,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(1));

    // Test 11: Destroyed -> 1
    let g = FireGateInput {
        status: ObjectStatus::Destroyed,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), Some(1));

    // Test 12: StartingUp passes (None)
    let g = FireGateInput {
        status: ObjectStatus::StartingUp,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), None);

    // Test 13: target None but has_target_point true and everything else fine -> None
    let g = FireGateInput {
        target_destroyed: None,
        has_target_point: true,
        ..passing_input()
    };
    assert_eq!(fire_gate(&g), None);
}
