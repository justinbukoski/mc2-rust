// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Gesture and state tables of `mclib/mstates.h`, `mclib/mech3d.h` and `mclib/mech3d.cpp`, copied value for value.

/// `GesturePark` .. `GestureFallenBackward` (mech3d.h:241-265).
pub mod gesture {
    pub const PARK: i32 = 0;
    pub const STAND_TO_PARK: i32 = 1;
    pub const STAND: i32 = 2;
    pub const STAND_TO_WALK: i32 = 3;
    pub const WALK: i32 = 4;
    pub const PARK_TO_STAND: i32 = 5;
    pub const WALK_TO_RUN: i32 = 6;
    pub const RUN: i32 = 7;
    pub const RUN_TO_WALK: i32 = 8;
    pub const REVERSE: i32 = 9;
    pub const STAND_TO_REVERSE: i32 = 10;
    pub const LIMP_LEFT: i32 = 11;
    pub const LIMP_RIGHT: i32 = 12;
    pub const IDLE: i32 = 13;
    pub const FALL_BACKWARD: i32 = 14;
    pub const FALL_FORWARD: i32 = 15;
    pub const HIT_FRONT: i32 = 16;
    pub const HIT_BACK: i32 = 17;
    pub const HIT_LEFT: i32 = 18;
    pub const HIT_RIGHT: i32 = 19;
    pub const JUMP: i32 = 20;
    pub const ROLLOVER: i32 = 21;
    pub const GET_UP: i32 = 22;
    pub const FALLEN_FORWARD: i32 = 23;
    pub const FALLEN_BACKWARD: i32 = 24;
    /// `MaxGestures`.
    pub const MAX: usize = 25;
}

/// `MECH_STATE_*` (mech3d.h:270-280): the gesture *goals* `setGestureGoal` accepts.
pub mod state {
    pub const NUM: i32 = 10;
    pub const PARKED: i32 = 0;
    pub const STANDING: i32 = 1;
    pub const WALKING: i32 = 2;
    pub const RUNNING: i32 = 3;
    pub const REVERSE: i32 = 4;
    pub const LIMPING_LEFT: i32 = 5;
    pub const JUMPING: i32 = 6;
    pub const FALLEN_FORWARD: i32 = 7;
    pub const FALLEN_BACKWARD: i32 = 8;
    pub const LIMPING_RIGHT: i32 = 9;
}

/// `BODYSTATE_*` (mech3d.cpp:94-98), the values of [`MECH_STATE_BY_GESTURE`].
pub mod body_state {
    pub const NORMAL: i8 = 0;
    pub const STANDING: i8 = 1;
    pub const PARKED: i8 = 2;
    pub const FALLEN_BACKWARD: i8 = 3;
    pub const FALLEN_FORWARD: i8 = 4;
}

/// `MAX_TRANSITION_GESTURES` (mstates.h:56).
pub const MAX_TRANSITION_GESTURES: usize = 10;
/// `GESTURE_OFFSET_SIZE` (mstates.h:183): row stride of [`TRANSITION_ARRAY`] per from-state.
pub const GESTURE_OFFSET_SIZE: usize = 100;
/// `BASE_NODE_RECYCLE_TIME` (mech3d.h:282).
pub const BASE_NODE_RECYCLE_TIME: f32 = 0.25;
/// `idleMAX` (mech3d.cpp:133): seconds standing out of combat before the idle gesture.
pub const IDLE_MAX: f32 = 20.0;
/// `DEGREES_TO_RADS` (mclib/dstd.h:35), the literal the appearance code multiplies by.
#[allow(clippy::excessive_precision)]
pub const DEGREES_TO_RADS: f32 = 0.017_453_292_519_9;
/// `HELICOPTER_FACTOR` (mech3d.cpp:102).
pub const HELICOPTER_FACTOR: f32 = 25.0;
/// `MAX_LODS` (apprtype.h:37).
pub const MAX_LODS: usize = 3;

/// `hotSpotFinderArray[28]` (mstates.h:58).
pub const HOT_SPOT_FINDER_ARRAY: [i8; 28] = [
    0, 0, 0, 1, 2, 3, 4, 5, 6, 2, 3, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 11, 10, 0, 19, 19,
];

/// `mechSpeedStateArray[28]` (mstates.h:64), indexed by gesture id (`BattleMech::getSpeedState`, mech.cpp:4106).
pub const MECH_SPEED_STATE_ARRAY: [i8; 28] = [
    0, 0, 0, 1, 1, 1, 1, 2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0,
];

/// `MechStateByGesture[MAX_MECH_ANIMATIONS]` (mech3d.cpp:104).
pub const MECH_STATE_BY_GESTURE: [i8; gesture::MAX] = [
    body_state::PARKED,
    body_state::STANDING,
    body_state::STANDING,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::PARKED,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::STANDING,
    body_state::FALLEN_BACKWARD,
    body_state::FALLEN_FORWARD,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::NORMAL,
    body_state::FALLEN_FORWARD,
    body_state::FALLEN_FORWARD,
    body_state::FALLEN_FORWARD,
    body_state::FALLEN_FORWARD,
];

/// `MechAnimationNames[MaxGestures+2]` (mech3d.cpp:142): suffixes of the `.agl` files, indexed by gesture.
pub const MECH_ANIMATION_NAMES: [&str; gesture::MAX + 2] = [
    "StandToPark",
    "ParkToStand",
    "",
    "STtoWK",
    "Walk",
    "StandToPark",
    "WKtoRN",
    "Run",
    "RNToWK",
    "Walk",
    "WKtoST",
    "LimpLeft",
    "LimpRight",
    "Idle",
    "FallBackward",
    "FallForward",
    "HitFront",
    "HitBack",
    "HitLeft",
    "HitRight",
    "Jump",
    "GetupBack",
    "GetupFront",
    "FallForward",
    "FallBackward",
    "FallBackwardDam",
    "FallForwardDam",
];

/// `MAX_ANIMATION_FILES` (mech3d.cpp:132).
pub const MAX_ANIMATION_FILES: usize = 25;

/// `transitionArray[1000]` (mstates.h:68): the gesture chain from state `from` to state `to`, read as
/// `transitionArray[from * GESTURE_OFFSET_SIZE + to * MAX_TRANSITION_GESTURES + step]`, -1 = goal reached.
#[rustfmt::skip]
pub const TRANSITION_ARRAY: [i8; 1000] = [
    // 0: from Park
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     1, 2,-1,-1,-1,-1,-1,-1,-1,-1,
     1, 2, 4,-1,-1,-1,-1,-1,-1,-1,
     1, 2, 4, 7,-1,-1,-1,-1,-1,-1,
     1, 2, 9,-1,-1,-1,-1,-1,-1,-1,
     1, 2,11,-1,-1,-1,-1,-1,-1,-1,
     1, 2,20, 2,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
     1, 2,12,-1,-1,-1,-1,-1,-1,-1,
    // 1: from Stand
     5, 0,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     4,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     7,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     9,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    11,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    20, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
    12,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    // 2: from Walk
     2, 5, 0,-1,-1,-1,-1,-1,-1,-1,
     2,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     7,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     9,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    11,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    20, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
    12,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    // 3: from Run
     2, 5, 0,-1,-1,-1,-1,-1,-1,-1,
     2,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     4,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     9,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    11,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    20, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
    12,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    // 4: from Reverse
     2, 5, 0,-1,-1,-1,-1,-1,-1,-1,
     2,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     4,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     7,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    11,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    20, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
    12,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    // 5: from LimpLeft
     2, 5, 0,-1,-1,-1,-1,-1,-1,-1,
     2,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     4,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     7,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     9,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    20, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
    12,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    // 6: from Jump
     5, 0,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     4,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     7,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     9,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    11,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    12,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    // 7: from Fallen Forward
    22, 2, 5, 0,-1,-1,-1,-1,-1,-1,
    22, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    22, 2, 4,-1,-1,-1,-1,-1,-1,-1,
    22, 2, 7,-1,-1,-1,-1,-1,-1,-1,
    22, 2, 9,-1,-1,-1,-1,-1,-1,-1,
    22, 2,11,-1,-1,-1,-1,-1,-1,-1,
    22, 2,20, 2,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    22, 2,12,-1,-1,-1,-1,-1,-1,-1,
    // 8: from Fallen Backward
    21, 2, 5, 0,-1,-1,-1,-1,-1,-1,
    21, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    21, 2, 4,-1,-1,-1,-1,-1,-1,-1,
    21, 2, 7,-1,-1,-1,-1,-1,-1,-1,
    21, 2, 9,-1,-1,-1,-1,-1,-1,-1,
    21, 2,11,-1,-1,-1,-1,-1,-1,-1,
    21, 2,20, 2,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    21, 2,12,-1,-1,-1,-1,-1,-1,-1,
    // 9: from LimpRight
     2, 5, 0,-1,-1,-1,-1,-1,-1,-1,
     2,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     4,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     7,-1,-1,-1,-1,-1,-1,-1,-1,-1,
     9,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    11,-1,-1,-1,-1,-1,-1,-1,-1,-1,
    20, 2,-1,-1,-1,-1,-1,-1,-1,-1,
    15,23,-1,-1,-1,-1,-1,-1,-1,-1,
    14,24,-1,-1,-1,-1,-1,-1,-1,-1,
    -1,-1,-1,-1,-1,-1,-1,-1,-1,-1,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spot_checks() {
        // From Fallen Backward to Jump: Rollover, Stand, Jump, Stand.
        let at = |from: usize, to: usize, k: usize| {
            TRANSITION_ARRAY[from * GESTURE_OFFSET_SIZE + to * MAX_TRANSITION_GESTURES + k]
        };
        assert_eq!(
            [
                at(8, 6, 0),
                at(8, 6, 1),
                at(8, 6, 2),
                at(8, 6, 3),
                at(8, 6, 4)
            ],
            [21, 2, 20, 2, -1]
        );
        assert_eq!(at(1, 2, 0), gesture::WALK as i8);
        assert_eq!(
            MECH_STATE_BY_GESTURE[gesture::ROLLOVER as usize],
            body_state::FALLEN_FORWARD
        );
    }
}
