// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Output path structures. Port of `MovePath` / `PathStep` (move.h ~845-961).
//! Only the data and setters the planner needs; locking, bridge/gate crossing tests
//! and mover-relative queries are game-state features and are not ported here.

use crate::geometry::Vec3;
use crate::mission::MissionMap;

/// `MAX_STEPS_PER_MOVEPATH`.
pub const MAX_STEPS_PER_MOVEPATH: usize = 200;

/// One cell on a path (`PathStep`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PathStep {
    /// Absolute map cell (row, col).
    pub cell: [i32; 2],
    /// Metres from this step to the goal along the path.
    pub distance_to_goal: f32,
    /// World position of the cell centre.
    pub destination: Vec3,
    /// Direction 0..7 of the move into this step (door steps use `door_direction * 2`).
    pub direction: u8,
    /// Global area id of the cell.
    pub area: i32,
}

/// A cell-level path (`MovePath`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovePath {
    /// World position of the goal.
    pub goal: Vec3,
    /// World position of the target, if one was set (`Vec3(-999999)` when none).
    pub target: Vec3,
    /// Steps in use; the path is `steps[..num_steps]`.
    pub num_steps: usize,
    pub num_steps_when_not_paused: usize,
    pub cur_step: usize,
    /// Total path cost (`g` of the goal cell). `1` marks "already on the goal".
    pub cost: i32,
    pub steps: Vec<PathStep>,
    pub marked: bool,
    pub global_step: i32,
}

impl MovePath {
    pub fn new() -> Self {
        MovePath {
            global_step: -1,
            ..Default::default()
        }
    }

    /// `init(numberOfSteps)`: size the step list and zero every step.
    pub fn init(&mut self, number_of_steps: usize) {
        self.num_steps = number_of_steps;
        self.num_steps_when_not_paused = number_of_steps;
        self.steps = vec![PathStep::default(); number_of_steps];
    }

    pub fn set_direction(&mut self, step: usize, direction: u8) {
        self.steps[step].direction = direction;
    }

    pub fn direction(&self, step: usize) -> u8 {
        self.steps[step].direction
    }

    pub fn set_distance_to_goal(&mut self, step: usize, distance: f32) {
        self.steps[step].distance_to_goal = distance;
    }

    pub fn distance_to_goal(&self, step: usize) -> f32 {
        self.steps[step].distance_to_goal
    }

    pub fn set_destination(&mut self, step: usize, destination: Vec3) {
        self.steps[step].destination = destination;
    }

    pub fn set_cell(&mut self, step: usize, row: i32, col: i32) {
        self.steps[step].cell = [row, col];
    }

    /// The steps actually in use.
    pub fn path_steps(&self) -> &[PathStep] {
        &self.steps[..self.num_steps.min(self.steps.len())]
    }

    /// `MovePath::getDistanceLeft(position, stepNumber)`: world-space distance converted to metres,
    /// plus the stored path distance at that step.
    pub fn distance_left(&self, position: Vec3, step: Option<usize>) -> f32 {
        let i = step.unwrap_or(self.cur_step);
        let target = self.steps.get(i).map_or(position, |s| s.destination);
        let dx = position.x - target.x;
        let dy = position.y - target.y;
        let dz = position.z - target.z;
        let distance = (dx * dx + dy * dy + dz * dz).sqrt() * 0.2_f32;
        distance + self.steps.get(i).map_or(0.0, |s| s.distance_to_goal)
    }

    /// Set/clear path-lock bits on `range` steps beginning at `start` (`-1` means `cur_step`).
    pub fn lock(&self, map: &mut MissionMap, level: u32, start: i32, range: usize, on: bool) {
        let first = if start == -1 {
            self.cur_step
        } else {
            start.max(0) as usize
        };
        let end = first
            .saturating_add(range)
            .min(self.num_steps_when_not_paused);
        for step in self.steps.get(first..end).unwrap_or_default() {
            map.set_pathlock(level, step.cell[0], step.cell[1], on);
        }
    }

    /// Whether any step in the range is pathlocked. `reached_end` reports clipping at the path's end.
    pub fn is_locked(
        &self,
        map: &MissionMap,
        level: u32,
        start: i32,
        range: usize,
        reached_end: Option<&mut bool>,
    ) -> bool {
        self.scan_range(start, range, reached_end, |step| {
            map.in_bounds(step.cell[0], step.cell[1])
                && map.cell(step.cell[0], step.cell[1]).pathlock(level)
        })
    }

    /// Whether any step in the range is impassable.
    pub fn is_blocked(
        &self,
        map: &MissionMap,
        start: i32,
        range: usize,
        reached_end: Option<&mut bool>,
    ) -> bool {
        self.scan_range(start, range, reached_end, |step| {
            !map.in_bounds(step.cell[0], step.cell[1])
                || !map.cell(step.cell[0], step.cell[1]).passable()
        })
    }

    /// Index of the first path step in the range whose cell matches `(row,col)`.
    pub fn crosses_cell(&self, start: i32, range: usize, row: i32, col: i32) -> Option<usize> {
        let first = if start == -1 {
            self.cur_step
        } else {
            start.max(0) as usize
        };
        let end = first
            .saturating_add(range)
            .min(self.num_steps_when_not_paused)
            .min(self.steps.len());
        self.steps
            .get(first..end)?
            .iter()
            .position(|s| s.cell == [row, col])
            .map(|n| first + n)
    }

    fn scan_range(
        &self,
        start: i32,
        range: usize,
        reached_end: Option<&mut bool>,
        mut predicate: impl FnMut(&PathStep) -> bool,
    ) -> bool {
        let first = if start == -1 {
            self.cur_step
        } else {
            start.max(0) as usize
        };
        let total = self.num_steps_when_not_paused.min(self.steps.len());
        let requested_end = first.saturating_add(range);
        if let Some(out) = reached_end {
            *out = requested_end >= total;
        }
        let end = requested_end.min(total);
        self.steps
            .get(first..end)
            .unwrap_or_default()
            .iter()
            .any(&mut predicate)
    }
}
