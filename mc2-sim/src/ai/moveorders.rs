// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MoveOrders` (warrior.h:403-455) and the `MechWarrior` accessors around it (warrior.cpp:1217-1270, 2418-2760): the move-order
//! state lane B reads and writes (`Pilot::lane_b_move` keeps its historical name), with every C++ field.
//!
//! Lane B's seam: [`crate::ai::get_move_path`] is `MechWarrior::getMovePath()` with all its side effects (the spare-path swap,
//! `moveOrders.yieldTime`, the goal rewrite, `curTacOrder.setWayPoint`), [`crate::ai::pause_path`] / [`crate::ai::resume_path`],
//! [`crate::ai::rethink_path`] and [`crate::ai::is_yielding`]. The fields lane B already reads (`paths`, `goal_type`, `path_type`,
//! `speed_state`, `speed_throttle`, `run`, `goal`, `has_goal`, `move_state`, `move_state_goal`, `is_yielding`,
//! `wait_for_point_time`) keep their names and meaning; `has_goal` is `goal_type != MOVEGOAL_NONE` and `is_yielding` is
//! `yield_time > -1` (both are kept in step by the setters here, never read back as an independent truth).

use crate::command::OrderOrigin;
use crate::ids::WatchId;
use crate::math::Vec3;
use mc2_path::MovePath;

/// `MOVEGOAL_NONE` (warrior.h:92; `unsigned long` is 64 bits in the oracle, so the value is not sign extended).
pub const MOVEGOAL_NONE: i64 = 0xFFFF_FFFF;
/// `MOVEGOAL_LOCATION` (warrior.h:93).
pub const MOVEGOAL_LOCATION: i64 = 0;
/// `MAX_GLOBAL_PATH` (warrior.h:419).
pub const MAX_GLOBAL_PATH: usize = 50;

/// `MovePathType` (warrior.h:421).
pub const MOVEPATH_UNDEFINED: i32 = 0;
/// `MOVEPATH_SIMPLE`.
pub const MOVEPATH_SIMPLE: i32 = 1;
/// `MOVEPATH_COMPLEX`.
pub const MOVEPATH_COMPLEX: i32 = 2;

/// `MoveStateType` (warrior.h:274).
pub const MOVESTATE_STAND: i32 = 0;
/// `MOVESTATE_FORWARD`.
pub const MOVESTATE_FORWARD: i32 = 1;
/// `MOVESTATE_REVERSE`.
pub const MOVESTATE_REVERSE: i32 = 2;
/// `MOVESTATE_PIVOT_FORWARD`.
pub const MOVESTATE_PIVOT_FORWARD: i32 = 3;
/// `MOVESTATE_PIVOT_REVERSE`.
pub const MOVESTATE_PIVOT_REVERSE: i32 = 4;
/// `MOVESTATE_PIVOT_TARGET`.
pub const MOVESTATE_PIVOT_TARGET: i32 = 5;

/// `MoveSpeedType`: `MOVE_SPEED_MODERATE` (tacordr.h:46).
pub const MOVE_SPEED_MODERATE: i32 = 3;
/// `MECH_STATE_WALKING` (mech3d.h).
pub const MECH_STATE_WALKING: i32 = 2;

/// `MOVEPATH_ERR_*` (warrior.h:106-120).
pub mod err {
    /// `MOVEPATH_ERR_NONE`.
    pub const NONE: i32 = 0;
    /// `MOVEPATH_ERR_NO_GOAL`.
    pub const NO_GOAL: i32 = -1;
    /// `MOVEPATH_ERR_GOALOBJ_DEAD`.
    pub const GOALOBJ_DEAD: i32 = -2;
    /// `MOVEPATH_ERR_NO_VALID_GOAL`.
    pub const NO_VALID_GOAL: i32 = -3;
    /// `MOVEPATH_ERR_ALREADY_THERE`.
    pub const ALREADY_THERE: i32 = -4;
    /// `MOVEPATH_ERR_LR_NOT_ENABLED`.
    pub const LR_NOT_ENABLED: i32 = -5;
    /// `MOVEPATH_ERR_LR_START_BLOCKED`.
    pub const LR_START_BLOCKED: i32 = -6;
    /// `MOVEPATH_ERR_LR_NO_PATH`.
    pub const LR_NO_PATH: i32 = -7;
    /// `MOVEPATH_ERR_LR_NO_SR_PATH`.
    pub const LR_NO_SR_PATH: i32 = -8;
    /// `MOVEPATH_ERR_EARLY_STOP`.
    pub const EARLY_STOP: i32 = -9;
    /// `MOVEPATH_ERR_TIME_OUT`.
    pub const TIME_OUT: i32 = -10;
    /// `MOVEPATH_ERR_LR_DOOR_CLOSED`.
    pub const LR_DOOR_CLOSED: i32 = -11;
    /// `MOVEPATH_ERR_LR_DOOR_BLOCKED`.
    pub const LR_DOOR_BLOCKED: i32 = -12;
    /// `MOVEPATH_ERR_ESCAPING_TILE`.
    pub const ESCAPING_TILE: i32 = -13;
    /// `MOVEPATH_ERR_TACORDER_CLEARED`.
    pub const TACORDER_CLEARED: i32 = -14;
}

/// The "no goal" coordinate `MoveOrders::init` / `clearMoveGoal` write into `goalLocation` (and `originalGlobalGoal`).
pub const NO_GOAL_COORD: f32 = -999_999.0;
/// `globalGoalLocation` after `MoveOrders::init`.
pub const NO_GLOBAL_GOAL_COORD: f32 = -666_666.0;

/// `GlobalPathStep` (move.h) as the move orders store it: `thruArea`, `goalDoor`, `costToGoal`, and the `goalCell` the sector search of
/// the step writes (the cell the step's path ends on, absolute map cell).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalStep {
    /// `thruArea`.
    pub thru_area: i32,
    /// `goalDoor`.
    pub goal_door: i32,
    /// `costToGoal`.
    pub cost_to_goal: i32,
    /// `goalCell[2]` (`(row, col)`).
    pub goal_cell: [i32; 2],
}

fn splat(v: f32) -> Vec3 {
    Vec3 { x: v, y: v, z: v }
}

/// `MoveOrders`.
#[derive(Clone, Debug, PartialEq)]
pub struct MoveOrders {
    /// `time`.
    pub time: f32,
    /// `origin`.
    pub origin: OrderOrigin,
    /// `speedType`.
    pub speed_type: i32,
    /// `speedVelocity`.
    pub speed_velocity: f32,
    /// `speedState` (a C `char`).
    pub speed_state: i32,
    /// `speedThrottle` (a C `char`).
    pub speed_throttle: i32,
    /// `goalType`: `MOVEGOAL_NONE`, `MOVEGOAL_LOCATION` or the goal object's watch id.
    pub goal_type: i64,
    /// `goalObjectWID`.
    pub goal_object_wid: WatchId,
    /// `goalObjectPosition`.
    pub goal_object_position: Vec3,
    /// `goalLocation`.
    pub goal: Vec3,
    /// `goalType != MOVEGOAL_NONE` (kept in step by [`MoveOrders::set_goal_fields`] / [`MoveOrders::clear_goal`]).
    pub has_goal: bool,
    /// `nextUpdate`.
    pub next_update: f32,
    /// `newGoal`.
    pub new_goal: bool,
    /// `wayPath[numWayPts]`.
    pub way_path: Vec<Vec3>,
    /// `curWayPt` (a C `char`).
    pub cur_way_pt: i32,
    /// `curWayDir` (a C `char`).
    pub cur_way_dir: i32,
    /// `pathType` (a C `char`).
    pub path_type: i32,
    /// `originalGlobalGoal[0]` (the original final goal), `[1]` (the actual final goal).
    pub original_global_goal: [Vec3; 2],
    /// `globalGoalLocation`.
    pub global_goal_location: Vec3,
    /// `globalPath[numGlobalSteps]`.
    pub global_path: Vec<GlobalStep>,
    /// `numGlobalSteps` (a C `char`).
    pub num_global_steps: i32,
    /// `curGlobalStep` (a C `char`).
    pub cur_global_step: i32,
    /// `path[0]` current path, `path[1]` the next (spare) path. Allocated once per pilot and never re-created: orders only swap them
    /// (`getMovePath`) or clear them (`clearMovePath`).
    pub paths: [MovePath; 2],
    /// `timeOfLastStep`.
    pub time_of_last_step: f32,
    /// `moveState`.
    pub move_state: i32,
    /// `moveStateGoal`.
    pub move_state_goal: i32,
    /// `twisting`.
    pub twisting: bool,
    /// `yieldTime`.
    pub yield_time: f32,
    /// `yieldState`.
    pub yield_state: i32,
    /// `waitForPointTime`.
    pub wait_for_point_time: f32,
    /// `run`.
    pub run: bool,
    /// `yieldTime > -1.0` (`isYielding`), kept in step by [`MoveOrders::set_yield_time`].
    pub is_yielding: bool,
}

impl Default for MoveOrders {
    /// A pilot's `moveOrders` after `MechWarrior::init(true)`: `MoveOrders::init()` and two freshly allocated (zero filled) paths.
    fn default() -> Self {
        let mut m = MoveOrders {
            time: 0.0,
            origin: OrderOrigin::Commander,
            speed_type: 0,
            speed_velocity: 0.0,
            speed_state: 0,
            speed_throttle: 0,
            goal_type: 0,
            goal_object_wid: WatchId(0),
            goal_object_position: Vec3::default(),
            goal: Vec3::default(),
            has_goal: false,
            next_update: 0.0,
            new_goal: false,
            way_path: Vec::new(),
            cur_way_pt: 0,
            cur_way_dir: 0,
            path_type: 0,
            original_global_goal: [Vec3::default(); 2],
            global_goal_location: Vec3::default(),
            global_path: Vec::new(),
            num_global_steps: 0,
            cur_global_step: 0,
            paths: [MovePath::default(), MovePath::default()],
            time_of_last_step: 0.0,
            move_state: 0,
            move_state_goal: 0,
            twisting: false,
            yield_time: 0.0,
            yield_state: 0,
            wait_for_point_time: 0.0,
            run: false,
            is_yielding: false,
        };
        m.init(0.0);
        m
    }
}

impl MoveOrders {
    /// `MoveOrders::init()` (warrior.cpp:1217-1270) at scenario time `now`. Leaves `paths` alone: the C++ nulls the pointers here and
    /// every caller saves and restores them around the call, so the allocated paths keep their contents.
    pub fn init(&mut self, now: f32) {
        self.time = now;
        self.origin = OrderOrigin::Commander;
        self.speed_type = MOVE_SPEED_MODERATE;
        self.speed_velocity = 0.0;
        self.speed_state = MECH_STATE_WALKING;
        self.speed_throttle = 100;
        self.goal_type = MOVEGOAL_NONE;
        self.has_goal = false;
        self.goal_object_wid = WatchId(0);
        self.goal_object_position = Vec3::default();
        self.goal = splat(NO_GOAL_COORD);
        self.next_update = 0.0;
        self.new_goal = false;
        self.way_path.clear();
        self.cur_way_pt = 0;
        self.cur_way_dir = 0;
        self.path_type = MOVEPATH_UNDEFINED;
        self.original_global_goal = [splat(NO_GOAL_COORD); 2];
        self.global_goal_location = splat(NO_GLOBAL_GOAL_COORD);
        self.num_global_steps = 0;
        self.cur_global_step = 0;
        self.time_of_last_step = -1.0;
        self.move_state = MOVESTATE_FORWARD;
        self.move_state_goal = MOVESTATE_FORWARD;
        self.twisting = false;
        self.set_yield_time(-1.0);
        self.yield_state = 0;
        self.wait_for_point_time = -1.0;
        self.run = false;
    }

    /// `MechWarrior::clearMoveGoal()` (warrior.h:1529): origin back to commander, no goal object, goal location -999999.
    pub fn clear_goal(&mut self) {
        self.origin = OrderOrigin::Commander;
        self.goal_type = MOVEGOAL_NONE;
        self.has_goal = false;
        self.goal_object_wid = WatchId(0);
        self.goal = splat(NO_GOAL_COORD);
    }

    /// The field writes of `MechWarrior::setMoveGoal(type, location)` (warrior.cpp:2437-2449) once the location's `z` is resolved.
    /// `type` 0 (`MOVEGOAL_LOCATION`) clears the goal object, any other value is the goal object's watch id.
    pub fn set_goal_fields(&mut self, goal_type: i64, location: Vec3) {
        self.goal_type = goal_type;
        self.has_goal = goal_type != MOVEGOAL_NONE;
        self.goal = location;
        self.goal_object_wid = if goal_type == MOVEGOAL_LOCATION {
            WatchId(0)
        } else {
            WatchId(goal_type as u32)
        };
    }

    /// `setMoveYieldTime`.
    pub fn set_yield_time(&mut self, time: f32) {
        self.yield_time = time;
        self.is_yielding = f64::from(time) > -1.0;
    }

    /// `isWaitingForPoint`.
    pub fn is_waiting_for_point(&self) -> bool {
        f64::from(self.wait_for_point_time) > -1.0
    }

    /// `hasMoveGoal`.
    pub fn has_move_goal(&self) -> bool {
        self.goal_type != MOVEGOAL_NONE
    }
}

/// `MovePath::clear()` = `init()` (move.h:881-889, move.cpp:1290): goal zero, no steps, first step, zero cost, unmarked, not part of
/// a complex path (`globalStep` -1). The step list keeps its stale contents, as in C++ (nothing reads past `numSteps`).
pub fn clear_move_path(path: &mut MovePath) {
    path.goal = Vec3::default();
    path.num_steps = 0;
    path.num_steps_when_not_paused = 0;
    path.cur_step = 0;
    path.cost = 0;
    path.marked = false;
    path.global_step = -1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_orders_init_matches_warrior_cpp() {
        // warrior.cpp:1217-1270
        let mut m = MoveOrders {
            goal_type: 77,
            has_goal: true,
            ..MoveOrders::default()
        };
        m.set_yield_time(5.0);
        m.init(12.5);
        assert_eq!(m.time, 12.5);
        assert_eq!(m.speed_type, 3);
        assert_eq!(m.speed_state, 2);
        assert_eq!(m.speed_throttle, 100);
        assert_eq!(m.goal_type, MOVEGOAL_NONE);
        assert!(!m.has_goal && !m.has_move_goal());
        assert_eq!(m.goal, splat(-999_999.0));
        assert_eq!(m.original_global_goal, [splat(-999_999.0); 2]);
        assert_eq!(m.global_goal_location, splat(-666_666.0));
        assert_eq!(m.path_type, MOVEPATH_UNDEFINED);
        assert_eq!(m.time_of_last_step, -1.0);
        assert_eq!((m.move_state, m.move_state_goal), (1, 1));
        assert_eq!(m.yield_time, -1.0);
        assert!(!m.is_yielding);
        assert_eq!(m.wait_for_point_time, -1.0);
        assert!(!m.run);
    }

    #[test]
    fn yield_and_wait_predicates_use_strictly_greater_than_minus_one() {
        // warrior.h: isYielding() = yieldTime > -1.0, isWaitingForPoint() = waitForPointTime > -1.0
        let mut m = MoveOrders::default();
        assert!(!m.is_yielding && !m.is_waiting_for_point());
        m.set_yield_time(-0.5);
        assert!(m.is_yielding);
        m.set_yield_time(-1.0);
        assert!(!m.is_yielding);
        m.wait_for_point_time = 0.0;
        assert!(m.is_waiting_for_point());
    }

    #[test]
    fn set_goal_distinguishes_location_and_object_goals() {
        // warrior.cpp:2437-2449
        let mut m = MoveOrders::default();
        let p = Vec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        m.set_goal_fields(MOVEGOAL_LOCATION, p);
        assert!(m.has_goal);
        assert_eq!((m.goal_type, m.goal_object_wid, m.goal), (0, WatchId(0), p));
        m.set_goal_fields(1234, p);
        assert_eq!((m.goal_type, m.goal_object_wid), (1234, WatchId(1234)));
        m.clear_goal();
        assert_eq!(m.goal_type, MOVEGOAL_NONE);
        assert_eq!(m.goal_object_wid, WatchId(0));
    }

    #[test]
    fn clear_move_path_is_init_not_init_zero() {
        // MovePath::clear() = init(): curStep, cost, marked, goal and globalStep are reset (move.h:881-889)
        let mut p = MovePath {
            num_steps: 5,
            num_steps_when_not_paused: 7,
            cur_step: 3,
            cost: 99,
            marked: true,
            global_step: 4,
            goal: Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            },
            ..MovePath::default()
        };
        clear_move_path(&mut p);
        assert_eq!(
            (p.num_steps, p.num_steps_when_not_paused, p.cur_step, p.cost),
            (0, 0, 0, 0)
        );
        assert!(!p.marked);
        assert_eq!(p.global_step, -1);
        assert_eq!(p.goal, Vec3::default());
    }
}
