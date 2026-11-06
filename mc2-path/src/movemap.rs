// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Cell-level A* over a bounded window. Port of `MoveMap` (move.h ~1395-1647) and its
//! methods in move.cpp: `init`, `clear`, `setStart`, `setGoal` (both forms), `markGoals`,
//! `setUp` (rectangle and sector/area forms), `adjacentCellOpen` (non-jump),
//! `calcHPrime`, `propogateCost`, `propogateCostJUMP` and `calcPath` / `calcPathJUMP`.
//!
//! Not ported: `calcEscapePath`/`markEscapeGoals`
//! (escape tiles), the stationary-mover and blocked-door callbacks (replaced by
//! [`MoveMap::set_mover_here`] and [`MoveMap::set_blocked_door_callback`]), mine checks
//! (`USE_MINES_IN_MC2` is off in the shipped build), and the debug/log output.

use crate::geometry::{agsqrt, cell_center, cell_length_meters, Vec3};
use crate::global::GlobalMap;
use crate::mission::{MapCell, MissionMap};
use crate::movepath::{MovePath, MAX_STEPS_PER_MOVEPATH};
use crate::pqueue::{PqNode, PriorityQueue};
use crate::tables::{
    is_diagonal_step, reverse_shift, team_relation, ADJ_TILE, CELL_SHIFT, RELATION_FRIENDLY,
    STEP_ADJ_DIR,
};

/// `SECTOR_DIM`: cells per global area sector side.
pub const SECTOR_DIM: i32 = 30;
/// `COST_CLEAR`.
pub const COST_CLEAR: i32 = 100;
/// `COST_BLOCKED`.
pub const COST_BLOCKED: i32 = 10000;
/// `HPRIME_NOT_CALCED` (C++ 0xFFFFFFFF stored in an int).
pub const HPRIME_NOT_CALCED: i32 = -1;
/// `MOVEFLAG_*` bits.
pub const MOVEFLAG_OPEN: u32 = 1;
pub const MOVEFLAG_CLOSED: u32 = 2;
pub const MOVEFLAG_STEP: u32 = 4;
pub const MOVEFLAG_GOAL: u32 = 8;
pub const MOVEFLAG_MOVER_HERE: u32 = 16;
pub const MOVEFLAG_GOAL_BLOCKED: u32 = 32;
pub const MOVEFLAG_OFFMAP: u32 = 64;
/// Default open-list capacity (`openList->init(5000)`).
pub const OPEN_LIST_CAPACITY: usize = crate::global::OPEN_LIST_CAPACITY;

/// `MOVEPARAM_*` bits that the path setup reads.
pub const MOVEPARAM_NONE: u32 = 0;
/// Face the target while pathing.
pub const MOVEPARAM_FACE_TARGET: u32 = 1;
/// Offset the path direction from the target.
pub const MOVEPARAM_DIR_OFFSET: u32 = 2;
/// Back up while pathing.
pub const MOVEPARAM_BACK_UP: u32 = 4;
/// Choose a nearby alternative goal.
pub const MOVEPARAM_SOMEWHERE_ELSE: u32 = 8;
/// Include moving movers in the path search.
pub const MOVEPARAM_MOVING_MOVERS: u32 = 16;
/// Legacy mystery parameter used by `calcMoveGoal`.
pub const MOVEPARAM_MYSTERY_PARAM: u32 = 32;
/// Mark stationary movers as occupied cells during setup.
pub const MOVEPARAM_STATIONARY_MOVERS: u32 = 64;
/// Avoid cells reserved by mover path locks.
pub const MOVEPARAM_AVOID_PATHLOCKS: u32 = 128;
/// Initialise a new path request.
pub const MOVEPARAM_INIT: u32 = 256;
/// Recalculate an existing path.
pub const MOVEPARAM_RECALC: u32 = 512;
/// Take a short step toward the target.
pub const MOVEPARAM_STEP_TOWARD_TARGET: u32 = 1024;
/// The request came from the player.
pub const MOVEPARAM_PLAYER: u32 = 2048;
/// Emit the path result radio event.
pub const MOVEPARAM_RADIO_RESULT: u32 = 4096;
/// Find an escape tile.
pub const MOVEPARAM_ESCAPE_TILE: u32 = 8192;
/// End adjacent to the target.
pub const MOVEPARAM_STEP_ADJACENT_TARGET: u32 = 16384;
pub const MOVEPARAM_SWEEP_MINES: u32 = 32768;
pub const MOVEPARAM_FOLLOW_ROADS: u32 = 65536;
pub const MOVEPARAM_WATER_SHALLOW: u32 = 131072;
pub const MOVEPARAM_WATER_DEEP: u32 = 262144;
/// Prefer an optimal attack distance with a randomized tie weight.
pub const MOVEPARAM_RANDOM_OPTIMAL: u32 = 524288;
pub const MOVEPARAM_JUMP: u32 = 1048576;

/// Forest step surcharge (`MoveMap::forestCost`).
pub const DEFAULT_FOREST_COST: i32 = 50;

/// Sentinel for "no world position" (C++ `-999999.0`).
const NO_WORLD_POS: f32 = -999_999.0;

/// Gate state queries (the C++ `isGateDisabledCallback` / `isGateOpenCallback`), keyed by
/// the gate's owner watch id.
pub trait GateOracle {
    /// Gate is disabled (destroyed or switched off): impassable for everyone.
    fn is_gate_disabled(&self, owner_wid: i32) -> bool;
    /// Gate is open to a non-team mover.
    fn is_gate_open(&self, owner_wid: i32) -> bool;
}

/// Gate oracle with no gates open and none disabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoGates;

impl GateOracle for NoGates {
    fn is_gate_disabled(&self, _owner_wid: i32) -> bool {
        false
    }
    fn is_gate_open(&self, _owner_wid: i32) -> bool {
        false
    }
}

/// Errors from the path code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathError {
    /// An endpoint is outside every area (`calcPath` returns -1 in C++).
    NoArea,
    /// The open list is full.
    OpenListFull,
    /// Path needs more steps than `MAX_STEPS_PER_MOVEPATH` (C++ gosASSERT).
    PathTooLong { steps: usize, max: usize },
    /// Requested window is larger than the allocated map.
    TooLarge { dim: i32, max: i32 },
    /// Sector pair is not horizontal or vertical neighbours (C++ STOP).
    DiagonalSectors,
    /// Door direction is not 0..3 (C++ gosASSERT / table overrun).
    BadDoorDirection(i8),
    /// The door has no goal cells inside the window (C++ `markGoals` returned 0).
    NoGoalCells,
    /// No area-level route between the endpoints.
    NoRoute,
    /// A required cell or area is outside the map.
    OutOfMap { row: i32, col: i32 },
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathError::NoArea => write!(f, "endpoint is not inside any area"),
            PathError::OpenListFull => write!(f, "open list is full"),
            PathError::PathTooLong { steps, max } => {
                write!(f, "path has {steps} steps (max {max})")
            }
            PathError::TooLarge { dim, max } => write!(f, "window {dim} exceeds allocated {max}"),
            PathError::DiagonalSectors => {
                write!(f, "sectors must be horizontal or vertical neighbours")
            }
            PathError::BadDoorDirection(d) => write!(f, "bad door direction {d}"),
            PathError::NoGoalCells => write!(f, "no goal cells for door"),
            PathError::NoRoute => write!(f, "no area-level route"),
            PathError::OutOfMap { row, col } => write!(f, "cell ({row},{col}) outside map"),
        }
    }
}

impl std::error::Error for PathError {}

/// Shared search state: the open list and the heuristic cut-off. The C++ keeps both as
/// globals shared by every move map and global map; here one context is passed explicitly.
#[derive(Clone, Debug)]
pub struct PathContext {
    pub open_list: PriorityQueue,
    /// `MaxHPrime`: nodes whose heuristic reaches this are not expanded.
    pub max_h_prime: i32,
}

impl Default for PathContext {
    fn default() -> Self {
        Self::new()
    }
}

impl PathContext {
    pub fn new() -> Self {
        PathContext {
            open_list: PriorityQueue::with_default_key_min(OPEN_LIST_CAPACITY),
            max_h_prime: 0,
        }
    }
}

/// One search node (`MoveMapNode`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveMapNode {
    /// Cost to enter this cell (terrain-derived), or `COST_BLOCKED`.
    pub cost: i32,
    /// Direction from this cell back to its parent.
    pub parent: i32,
    pub flags: u32,
    /// Known cost from the start.
    pub g: i32,
    /// Heuristic to the goal.
    pub h_prime: i32,
    /// `g + h_prime`.
    pub f_prime: i32,
    /// Index of each neighbour in the node array, or -1.
    pub adj_cells: [i32; 8],
}

/// Result of [`MoveMap::calc_path`].
#[derive(Clone, Debug, PartialEq)]
pub struct PathResult {
    pub path: MovePath,
    /// Cell reached: the goal cell, or the cell beyond a door, or the trimmed end
    /// when the path is cut at its second thru area. Absolute coordinates for doors and
    /// trims, window-relative for plain goals (as the C++ writes them).
    pub goal_cell: (i32, i32),
    /// The path was cut where it first entered the second thru area: the C++ then sets the `goalWorldPos` output with
    /// `Terrain::cellToWorld` of `goal_cell` and leaves `path.goal` at the goal door, so the caller must do the former.
    pub trimmed: bool,
}

#[derive(Clone, Copy)]
struct CostOpts {
    ground: bool,
    follow_roads: bool,
    traverse_shallow: bool,
    traverse_deep: bool,
    travel_off_map: bool,
    thru: Option<[i32; 2]>,
}

type BlockedDoorFn = Box<dyn Fn(usize, usize, &mut [bool])>;

/// Cell-level search window. `new(max_w, max_h)` mirrors `MoveMap::init(maxW, maxH)`.
pub struct MoveMap {
    pub ul_r: i32,
    pub ul_c: i32,
    width: i32,
    height: i32,
    max_width: i32,
    max_height: i32,
    min_row: i32,
    max_row: i32,
    min_col: i32,
    max_col: i32,
    map: Vec<MoveMapNode>,
    move_level: usize,
    start_r: i32,
    start_c: i32,
    goal_r: i32,
    goal_c: i32,
    thru_areas: [i32; 2],
    door: i32,
    door_side: i32,
    door_direction: i32,
    target: Vec3,
    clear_cost: i32,
    jump_cost: i32,
    num_offsets: i32,
    pub mover_wid: i32,
    pub mover_team_id: i8,
    pub mover_laying_mines: bool,
    pub mover_withdrawing: bool,
    pub travel_off_map: bool,
    pub cannot_enter_off_map: bool,
    /// `CullPathAreas` (off in the shipped build).
    pub cull_path_areas: bool,
    /// If set, only cells whose area is in this list can be entered (others are blocked).
    /// The planner uses it to keep the last leg on the areas of its global route.
    pub area_filter: Option<Vec<i32>>,
    /// `EditorSave` (off during play: gates are checked against the oracle).
    pub editor_save: bool,
    /// `MoveMap::forestCost`.
    pub forest_cost: i32,
    /// `ZeroHPrime` (off: heuristics are computed lazily).
    pub zero_h_prime: bool,
    /// Test-only switch `JUMP-EDGE-CELLS` (`DIFFERENCES.md`): when set, a jump-capable search (`num_offsets > 8`) never enters a cell of
    /// window row 0 or column 0, as the community port's `MoveMap::calcPathJUMP` does (move.cpp:5605, `succRow > 0 && succCol > 0`).
    /// Off in the shipped game: the original used `inMapBounds` (`>= 0`).
    pub cpp_jump_edge_cells_excluded: bool,
    /// Optional `blockedDoorCallback`; see [`MoveMap::set_blocked_door_callback`].
    blocked_door: Option<BlockedDoorFn>,
}

impl MoveMap {
    /// `MoveMap::init(maxW, maxH)`: allocate and build the neighbour table for the full window.
    pub fn new(max_width: i32, max_height: i32) -> Self {
        let n = (max_width.max(0) * max_height.max(0)) as usize;
        let mut m = MoveMap {
            ul_r: 0,
            ul_c: 0,
            width: max_width,
            height: max_height,
            max_width,
            max_height,
            min_row: 0,
            max_row: 0,
            min_col: 0,
            max_col: 0,
            map: vec![MoveMapNode::default(); n],
            move_level: 0,
            start_r: -1,
            start_c: -1,
            goal_r: -1,
            goal_c: -1,
            thru_areas: [-1, -1],
            door: -1,
            door_side: -1,
            door_direction: -1,
            target: Vec3 {
                x: NO_WORLD_POS,
                y: NO_WORLD_POS,
                z: NO_WORLD_POS,
            },
            clear_cost: 1,
            jump_cost: 0,
            num_offsets: 8,
            mover_wid: 0,
            mover_team_id: 0,
            mover_laying_mines: false,
            mover_withdrawing: false,
            travel_off_map: false,
            cannot_enter_off_map: true,
            cull_path_areas: false,
            area_filter: None,
            editor_save: false,
            forest_cost: DEFAULT_FOREST_COST,
            zero_h_prime: false,
            cpp_jump_edge_cells_excluded: false,
            blocked_door: None,
        };
        for r in 0..max_height {
            for c in 0..max_width {
                let idx = (r * max_width + c) as usize;
                for d in 0..8usize {
                    let ar = r + CELL_SHIFT[d * 2];
                    let ac = c + CELL_SHIFT[d * 2 + 1];
                    m.map[idx].adj_cells[d] = if ar >= 0 && ar < m.height && ac >= 0 && ac < m.width
                    {
                        ar * max_width + ac
                    } else {
                        -1
                    };
                }
            }
        }
        m.clear();
        m
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn target(&self) -> Vec3 {
        self.target
    }

    /// Node at window-relative (row, col).
    pub fn node(&self, row: i32, col: i32) -> &MoveMapNode {
        &self.map[(row * self.max_width + col) as usize]
    }

    /// `setMover`.
    pub fn set_mover(&mut self, watch_id: i32, team_id: i8, laying_mines: bool, withdrawing: bool) {
        self.mover_wid = watch_id;
        self.mover_team_id = team_id;
        self.mover_laying_mines = laying_mines;
        self.mover_withdrawing = withdrawing;
    }

    pub fn set_clear_cost(&mut self, cost: i32) {
        self.clear_cost = cost;
    }

    pub fn set_jump_cost(&mut self, cost: i32, offsets: i32) {
        self.jump_cost = cost;
        self.num_offsets = offsets;
    }

    /// `setTarget`.
    pub fn set_target(&mut self, target: Vec3) {
        self.target = target;
    }

    /// Mark or unmark a window cell as occupied by a stationary mover; marking raises its entry cost by `COST_BLOCKED * 2`
    /// (`MoveMap::adjustCost`, no upper clamp).
    pub fn set_mover_here(&mut self, row: i32, col: i32, on: bool) {
        if row < 0 || col < 0 || row >= self.height || col >= self.width {
            return;
        }
        let idx = (row * self.max_width + col) as usize;
        if on {
            if self.map[idx].flags & MOVEFLAG_GOAL != 0 {
                return;
            }
            self.map[idx].flags |= MOVEFLAG_MOVER_HERE;
            self.adjust_cost(idx, COST_BLOCKED * 2);
        } else {
            self.map[idx].flags &= !MOVEFLAG_MOVER_HERE;
        }
    }

    /// `PlaceStationaryMovers` (mover.cpp:7127), per mover cell. `cells` are *window indices*: the C++ indexes the window with the
    /// mover's absolute cell (and compares it with the window size and the window-relative start cell), so a caller passes the
    /// absolute cells of the movers it has already filtered. Cells outside the window, on the start cell or on a goal cell are skipped.
    pub fn place_stationary_movers(&mut self, cells: &[(i32, i32)]) {
        for &(row, col) in cells {
            if row < 0 || row >= self.height || col < 0 || col >= self.width {
                continue;
            }
            if row == self.start_r && col == self.start_c {
                continue;
            }
            self.set_mover_here(row, col, true);
        }
    }

    /// Callback run by `markGoals` before door goal cells are chosen. It receives
    /// (move level, door index, per-door-cell open flags) and may clear entries to block them.
    pub fn set_blocked_door_callback(&mut self, f: impl Fn(usize, usize, &mut [bool]) + 'static) {
        self.blocked_door = Some(Box::new(f));
    }

    fn idx(&self, row: i32, col: i32) -> usize {
        (row * self.max_width + col) as usize
    }

    fn in_bounds(&self, row: i32, col: i32) -> bool {
        row >= self.min_row && row <= self.max_row && col >= self.min_col && col <= self.max_col
    }

    /// `clear()`: reset costs, parents, flags and heuristics over the current window.
    pub fn clear(&mut self) {
        let num = (self.max_width * self.height) as usize;
        let init_h = if self.zero_h_prime {
            0
        } else {
            HPRIME_NOT_CALCED
        };
        for node in self.map.iter_mut().take(num) {
            node.cost = COST_BLOCKED;
            node.parent = -1;
            node.flags = 0;
            node.h_prime = init_h;
        }
        self.target = Vec3 {
            x: NO_WORLD_POS,
            y: NO_WORLD_POS,
            z: NO_WORLD_POS,
        };
    }

    fn set_flag(&mut self, idx: usize, flag: u32) {
        self.map[idx].flags |= flag;
    }

    fn clear_flag(&mut self, idx: usize, flag: u32) {
        self.map[idx].flags &= !flag;
    }

    fn adjust_cost(&mut self, idx: usize, adj: i32) {
        let cost = (self.map[idx].cost + adj).max(1);
        self.map[idx].cost = cost;
    }

    /// `calcHPrime(r, c)`: Manhattan distance to the goal window cell.
    fn calc_h_prime(&self, r: i32, c: i32) -> i32 {
        (r - self.goal_r).abs() + (c - self.goal_c).abs()
    }

    /// Terrain cost for one cell (no path-lock adjustment). Mirrors the C++ branch order.
    fn cell_cost(
        &self,
        global: &GlobalMap,
        abs_r: i32,
        abs_c: i32,
        cell: MapCell,
        o: CostOpts,
        oracle: &dyn GateOracle,
    ) -> i32 {
        if let Some(allowed) = &self.area_filter {
            if !allowed.contains(&global.calc_area(abs_r, abs_c)) {
                return COST_BLOCKED;
            }
        }
        let mut cost = self.clear_cost;
        let off_map = cell.off_map();
        if self.cull_path_areas {
            if let Some(t) = o.thru {
                let area = global.calc_area(abs_r, abs_c);
                if area != t[0] && area != t[1] {
                    return COST_BLOCKED;
                }
            }
        }
        if off_map && !o.travel_off_map {
            cost = COST_BLOCKED;
        } else if o.ground {
            if cell.shallow_water() {
                if !o.traverse_shallow || !cell.passable() {
                    cost = COST_BLOCKED;
                }
            } else if cell.deep_water() {
                if !o.traverse_deep || !cell.passable() {
                    cost = COST_BLOCKED;
                }
            } else if cell.gate() != 0 {
                let area = global.calc_area(abs_r, abs_c);
                let mut team_id: i8 = -1;
                let mut owner = 0;
                if area > -1 {
                    let a = global.areas()[area as usize];
                    team_id = a.team_id;
                    owner = a.owner_wid;
                }
                if !self.editor_save && area > -1 && oracle.is_gate_disabled(owner) {
                    cost = COST_BLOCKED;
                } else if team_id > -1
                    && team_relation(team_id, self.mover_team_id) != RELATION_FRIENDLY
                {
                    if cell.passable() {
                        cost <<= 2;
                    } else {
                        cost = COST_BLOCKED;
                    }
                } else if cell.road() && o.follow_roads {
                    cost >>= 2;
                }
            } else if !cell.passable() {
                cost = COST_BLOCKED;
            } else if cell.road() && o.follow_roads {
                cost >>= 2;
            }
            if cell.forest() {
                cost += self.forest_cost;
            }
        }
        cost
    }

    /// Shared setup: window geometry, start, mover flags, and the per-cell cost pass.
    fn fill_costs(
        &mut self,
        mission: &MissionMap,
        global: &GlobalMap,
        params: u32,
        oracle: &dyn GateOracle,
        thru: Option<[i32; 2]>,
        window_check: bool,
    ) {
        let (rows, cols) = (self.height, self.width);
        let path_lock_cost = self.clear_cost << 3;
        let level = self.move_level;
        let ground = level == 0 || level == 1;
        let opts = CostOpts {
            ground,
            follow_roads: params & MOVEPARAM_FOLLOW_ROADS != 0,
            traverse_shallow: params & (MOVEPARAM_WATER_SHALLOW | MOVEPARAM_WATER_DEEP) != 0,
            traverse_deep: params & MOVEPARAM_WATER_DEEP != 0,
            travel_off_map: self.travel_off_map,
            thru,
        };
        for cell_row in 0..rows {
            for cell_col in 0..cols {
                let abs_r = self.ul_r + cell_row;
                let abs_c = self.ul_c + cell_col;
                if !mission.in_bounds(abs_r, abs_c) {
                    continue;
                }
                if window_check && !self.in_bounds(cell_row, cell_col) {
                    continue;
                }
                let cell = mission.cell(abs_r, abs_c);
                let idx = self.idx(cell_row, cell_col);
                if cell.off_map() {
                    self.set_flag(idx, MOVEFLAG_OFFMAP);
                }
                let cost = self.cell_cost(global, abs_r, abs_c, cell, opts, oracle);
                self.map[idx].cost = cost;
                if cell.pathlock((level == 2) as u32) {
                    self.adjust_cost(idx, path_lock_cost);
                }
            }
        }
    }

    /// Set `move_level`, the window origin, and the window extent in cells.
    fn set_window(
        &mut self,
        ul: (i32, i32),
        min_row: i32,
        max_row: i32,
        min_col: i32,
        max_col: i32,
        level: usize,
    ) {
        self.ul_r = ul.0;
        self.ul_c = ul.1;
        self.min_row = min_row;
        self.max_row = max_row;
        self.min_col = min_col;
        self.max_col = max_col;
        self.move_level = level;
    }

    /// Start cell, window-relative. `start` is absolute.
    fn set_start_rel(&mut self, start: (i32, i32)) {
        self.start_r = start.0 - self.ul_r;
        self.start_c = start.1 - self.ul_c;
    }

    /// Travel-off-map flags as set in both `setUp` variants.
    fn set_travel_flags(&mut self, mission: &MissionMap, start_abs: (i32, i32)) {
        self.travel_off_map = false;
        self.cannot_enter_off_map = true;
        if mission.in_bounds(start_abs.0, start_abs.1)
            && mission.cell(start_abs.0, start_abs.1).off_map()
        {
            self.travel_off_map = true;
        }
        if self.mover_withdrawing {
            self.travel_off_map = true;
            self.cannot_enter_off_map = false;
        }
    }

    fn effective_jump_cost(clear_cost: i32, jump_cost: i32, params: u32) -> i32 {
        if params & MOVEPARAM_JUMP != 0 && jump_cost > 0 {
            clear_cost
        } else {
            jump_cost
        }
    }

    /// `setUp(ULr, ULc, dim, dim, level, start, goal, costs, params)`: a square window of
    /// `dim` cells whose corner is `ul`, with start and goal given as absolute cells.
    /// Used for the short-range "simple" path and sector-local searches to a fixed goal cell.
    #[allow(clippy::too_many_arguments)]
    pub fn set_up_rect(
        &mut self,
        mission: &MissionMap,
        global: &GlobalMap,
        level: usize,
        ul: (i32, i32),
        dim: i32,
        start: (i32, i32),
        goal: (i32, i32),
        clear_cost: i32,
        jump_cost: i32,
        offsets: i32,
        params: u32,
        oracle: &dyn GateOracle,
    ) -> Result<(), PathError> {
        if dim < 1 || dim > self.max_width || dim > self.max_height {
            return Err(PathError::TooLarge {
                dim,
                max: self.max_width.min(self.max_height),
            });
        }
        self.width = dim;
        self.height = dim;
        let jump = Self::effective_jump_cost(clear_cost, jump_cost, params);
        self.set_clear_cost(clear_cost);
        self.set_jump_cost(jump, offsets);
        self.clear();
        self.thru_areas = [-1, -1];
        self.set_window(ul, 0, dim - 1, 0, dim - 1, level);
        self.set_start_rel(start);
        self.goal_r = goal.0 - self.ul_r;
        self.goal_c = goal.1 - self.ul_c;
        self.door_direction = -1;
        self.set_travel_flags(mission, start);
        self.fill_costs(mission, global, params, oracle, None, false);
        if self.goal_r >= 0
            && self.goal_r < self.height
            && self.goal_c >= 0
            && self.goal_c < self.width
        {
            let gi = self.idx(self.goal_r, self.goal_c);
            self.set_flag(gi, MOVEFLAG_GOAL);
        }
        Ok(())
    }

    /// `setUp(level, start, thruArea, goalDoor, finalGoal, ...)`: the sector form.
    /// `thru` names one or two adjacent areas (the second may be -1). The window is the
    /// 30x30 sector (or the 60x30/30x60 pair) that covers them, and the search ends at the
    /// cells of `goal_door`, or at `final_goal` if it lies beyond that door.
    #[allow(clippy::too_many_arguments)]
    pub fn set_up_area(
        &mut self,
        mission: &MissionMap,
        global: &GlobalMap,
        level: usize,
        start: (i32, i32),
        thru: [i32; 2],
        goal_door: usize,
        final_goal: (i32, i32),
        clear_cost: i32,
        jump_cost: i32,
        offsets: i32,
        params: u32,
        oracle: &dyn GateOracle,
    ) -> Result<(), PathError> {
        let dim2 = SECTOR_DIM * 2;
        if dim2 > self.max_width || dim2 > self.max_height {
            return Err(PathError::TooLarge {
                dim: dim2,
                max: self.max_width.min(self.max_height),
            });
        }
        self.width = dim2;
        self.height = dim2;
        self.clear();
        self.thru_areas = thru;
        let areas = global.areas();
        if thru[0] < 0 || thru[0] as usize >= areas.len() {
            return Err(PathError::NoArea);
        }
        let a0 = areas[thru[0] as usize];
        let (fr, fc) = (a0.sector_r as i32, a0.sector_c as i32);
        let (ul, min_r, max_r, min_c, max_c) = if thru[1] == -1 {
            (
                (fr * SECTOR_DIM, fc * SECTOR_DIM),
                0,
                SECTOR_DIM - 1,
                0,
                SECTOR_DIM - 1,
            )
        } else {
            if thru[1] as usize >= areas.len() {
                return Err(PathError::NoArea);
            }
            let a1 = areas[thru[1] as usize];
            let (sr, sc) = (a1.sector_r as i32, a1.sector_c as i32);
            if sr > fr {
                if sc != fc {
                    return Err(PathError::DiagonalSectors);
                }
                (
                    (fr * SECTOR_DIM, fc * SECTOR_DIM),
                    0,
                    SECTOR_DIM * 2 - 1,
                    0,
                    SECTOR_DIM - 1,
                )
            } else if sr < fr {
                if sc != fc {
                    return Err(PathError::DiagonalSectors);
                }
                (
                    (sr * SECTOR_DIM, sc * SECTOR_DIM),
                    0,
                    SECTOR_DIM * 2 - 1,
                    0,
                    SECTOR_DIM - 1,
                )
            } else if sc > fc {
                (
                    (fr * SECTOR_DIM, fc * SECTOR_DIM),
                    0,
                    SECTOR_DIM - 1,
                    0,
                    SECTOR_DIM * 2 - 1,
                )
            } else if sc < fc {
                (
                    (sr * SECTOR_DIM, sc * SECTOR_DIM),
                    0,
                    SECTOR_DIM - 1,
                    0,
                    SECTOR_DIM * 2 - 1,
                )
            } else {
                (
                    (fr * SECTOR_DIM, fc * SECTOR_DIM),
                    0,
                    SECTOR_DIM - 1,
                    0,
                    SECTOR_DIM - 1,
                )
            }
        };
        self.set_window(ul, min_r, max_r, min_c, max_c, level);
        let jump = Self::effective_jump_cost(clear_cost, jump_cost, params);
        self.set_clear_cost(clear_cost);
        self.set_jump_cost(jump, offsets);
        self.set_start_rel(start);
        let goal_thru = if thru[1] == -1 { thru[0] } else { thru[1] };
        self.set_goal_door(global, goal_thru, goal_door)?;
        self.set_travel_flags(mission, start);
        let thru_opt = Some(thru);
        self.fill_costs_area(mission, global, params, oracle, thru_opt);
        if self.mark_goals(global, final_goal) == 0 {
            return Err(PathError::NoGoalCells);
        }
        Ok(())
    }

    /// Cost pass of the sector form: only cells inside the window get costs.
    fn fill_costs_area(
        &mut self,
        mission: &MissionMap,
        global: &GlobalMap,
        params: u32,
        oracle: &dyn GateOracle,
        thru: Option<[i32; 2]>,
    ) {
        self.fill_costs(mission, global, params, oracle, thru, true);
    }

    /// `setGoal(thruArea, goalDoor)`: the goal is a door; `door_direction` and the window
    /// goal cell are derived from the door's geometry.
    fn set_goal_door(
        &mut self,
        global: &GlobalMap,
        thru_area: i32,
        door: usize,
    ) -> Result<(), PathError> {
        let nd = global.num_doors() + 2;
        if door >= nd {
            return Err(PathError::NoArea);
        }
        self.door = door as i32;
        let d = *global.door(door);
        self.door_side = (d.area[1] as i32 == thru_area) as i32;
        const DOOR_DIR_TABLE: [[i32; 2]; 4] = [[-1, -1], [1, 3], [2, 0], [-1, -1]];
        let dir = d.direction[0];
        if !(0..4).contains(&dir) {
            return Err(PathError::BadDoorDirection(dir));
        }
        self.door_direction = DOOR_DIR_TABLE[dir as usize][self.door_side as usize];
        let len = d.length as i32;
        if self.door_direction == 0 || self.door_direction == 2 {
            self.goal_r = d.row as i32 + self.door_side - self.ul_r;
            self.goal_c = d.col as i32 + len / 2 - self.ul_c;
        } else if self.door_direction == 1 || self.door_direction == 3 {
            self.goal_r = d.row as i32 + len / 2 - self.ul_r;
            self.goal_c = d.col as i32 + self.door_side - self.ul_c;
        }
        Ok(())
    }

    /// `markGoals`: flag the door's cells (or the final goal cell when it lies beyond the
    /// door) and bias their costs toward the door centre. Returns the number of goal cells.
    fn mark_goals(&mut self, global: &GlobalMap, final_goal: (i32, i32)) -> i32 {
        let door = self.door as usize;
        let door_length = global.door(door).length as i32;
        let mut state = vec![true; door_length.max(0) as usize];
        if let Some(cb) = &self.blocked_door {
            cb(self.move_level, door, &mut state);
        }
        let final_r = final_goal.0 - self.ul_r;
        let final_c = final_goal.1 - self.ul_c;
        let mut num_goal_cells = 0;
        let w = self.max_width;
        let len = door_length;
        let in_alloc =
            |s: &Self, r: i32, c: i32| r >= 0 && c >= 0 && r < s.max_height && c < s.max_width;

        if self.door_direction == 0 || self.door_direction == 2 {
            let cell_r = self.goal_r;
            let cell_c = self.goal_c - len / 2;
            let mut next_to_goal = false;
            if self.door_side == 0 {
                if final_r == cell_r + 1
                    && final_c >= cell_c
                    && final_c < cell_c + len
                    && in_alloc(self, cell_r, final_c)
                {
                    self.set_flag((cell_r * w + final_c) as usize, MOVEFLAG_GOAL);
                    num_goal_cells += 1;
                    next_to_goal = true;
                }
            } else if final_r == cell_r - 1
                && final_c >= cell_c
                && final_c < cell_c + len
                && in_alloc(self, cell_r, final_c)
            {
                self.set_flag((cell_r * w + final_c) as usize, MOVEFLAG_GOAL);
                num_goal_cells += 1;
                next_to_goal = true;
            }
            if !next_to_goal {
                let adj_cost = self.clear_cost / 2;
                let door_center = len / 2;
                let mut cur_cost = door_center * adj_cost;
                for c in 0..door_center {
                    let cell_index = cell_r * w + cell_c + c;
                    if state[c as usize] && in_alloc(self, cell_r, cell_c + c) {
                        self.set_flag(cell_index as usize, MOVEFLAG_GOAL);
                        num_goal_cells += 1;
                        self.adjust_cost(cell_index as usize, cur_cost);
                    }
                    cur_cost -= adj_cost;
                }
                let mut cur_cost = 0;
                for c in door_center..len {
                    let cell_index = cell_r * w + cell_c + c;
                    if state[c as usize] && in_alloc(self, cell_r, cell_c + c) {
                        self.set_flag(cell_index as usize, MOVEFLAG_GOAL);
                        num_goal_cells += 1;
                        self.adjust_cost(cell_index as usize, cur_cost);
                    }
                    cur_cost += adj_cost;
                }
            }
        } else if self.door_direction == 1 || self.door_direction == 3 {
            let cell_r = self.goal_r - len / 2;
            let cell_c = self.goal_c;
            let mut next_to_goal = false;
            if final_c == cell_c + 1 - self.door_side * 2
                && final_r >= cell_r
                && final_r < cell_r + len
                && in_alloc(self, final_r, cell_c)
            {
                self.set_flag((final_r * w + cell_c) as usize, MOVEFLAG_GOAL);
                num_goal_cells += 1;
                next_to_goal = true;
            }
            if !next_to_goal {
                let adj_cost = self.clear_cost / 2;
                let door_center = len / 2;
                let mut cur_cost = door_center * adj_cost;
                for r in 0..door_center {
                    let cell_index = (cell_r + r) * w + cell_c;
                    if state[r as usize] && in_alloc(self, cell_r + r, cell_c) {
                        self.set_flag(cell_index as usize, MOVEFLAG_GOAL);
                        num_goal_cells += 1;
                        self.adjust_cost(cell_index as usize, cur_cost);
                    }
                    cur_cost -= adj_cost;
                }
                let mut cur_cost = 0;
                for r in door_center..len {
                    let cell_index = (cell_r + r) * w + cell_c;
                    if state[r as usize] && in_alloc(self, cell_r + r, cell_c) {
                        self.set_flag(cell_index as usize, MOVEFLAG_GOAL);
                        num_goal_cells += 1;
                        self.adjust_cost(cell_index as usize, cur_cost);
                    }
                    cur_cost += adj_cost;
                }
            }
        }
        num_goal_cells
    }

    /// Non-jump `adjacentCellOpen` is folded into `propogate_cost`/`calc_path` as in C++.
    fn diagonal_ok(&self, idx: usize, dir: usize) -> bool {
        let mut adj1_open = false;
        let a1 = self.map[idx].adj_cells[STEP_ADJ_DIR[dir] as usize];
        if a1 > -1 && self.map[a1 as usize].flags & MOVEFLAG_MOVER_HERE == 0 {
            adj1_open = self.map[a1 as usize].cost < COST_BLOCKED;
        }
        let mut adj2_open = false;
        let a2 = self.map[idx].adj_cells[STEP_ADJ_DIR[dir + 1] as usize];
        if a2 > -1 && self.map[a2 as usize].flags & MOVEFLAG_MOVER_HERE == 0 {
            adj2_open = self.map[a2 as usize].cost < COST_BLOCKED;
        }
        adj1_open || adj2_open
    }

    /// `propogateCost` (non-jump), recursive as in C++.
    fn propogate_cost(&mut self, ctx: &mut PathContext, cur: usize, cost: i32, g: i32) {
        if self.map[cur].g > g + cost {
            self.map[cur].cost = cost;
            self.map[cur].g = g + cost;
            self.map[cur].f_prime = self.map[cur].g + self.map[cur].h_prime;
            if self.map[cur].flags & MOVEFLAG_OPEN != 0 {
                let open_index = ctx.open_list.find(cur as u32);
                ctx.open_list.change(open_index, self.map[cur].f_prime);
            } else {
                let n_off = self.num_offsets.clamp(0, 8) as usize;
                for dir in 0..n_off {
                    let diag = is_diagonal_step(dir);
                    if diag && !self.diagonal_ok(cur, dir) {
                        continue;
                    }
                    let succ = self.map[cur].adj_cells[dir];
                    if succ > -1 {
                        let s = succ as usize;
                        if self.map[s].cost < COST_BLOCKED
                            && self.map[s].h_prime != HPRIME_NOT_CALCED
                            && self.map[s].h_prime < ctx.max_h_prime
                        {
                            let dir_to_parent = reverse_shift(dir) as i32;
                            let mut step = self.map[s].cost;
                            if diag {
                                step += step << 1;
                            }
                            if dir_to_parent == self.map[s].parent {
                                let g_cur = self.map[cur].g;
                                self.propogate_cost(ctx, s, step, g_cur);
                            } else if self.map[cur].g + step < self.map[s].g {
                                self.map[s].parent = dir_to_parent;
                                let g_cur = self.map[cur].g;
                                self.propogate_cost(ctx, s, step, g_cur);
                            }
                        }
                    }
                }
            }
        }
    }

    /// `propogateCostJUMP` (move.cpp:4907-4973), including jump-offset descendants.
    fn propogate_cost_jump(&mut self, ctx: &mut PathContext, cur: usize, cost: i32, g: i32) {
        if self.map[cur].g <= g + cost {
            return;
        }
        self.map[cur].cost = cost;
        self.map[cur].g = g + cost;
        self.map[cur].f_prime = self.map[cur].g + self.map[cur].h_prime;
        if self.map[cur].flags & MOVEFLAG_OPEN != 0 {
            let open_index = ctx.open_list.find(cur as u32);
            ctx.open_list.change(open_index, self.map[cur].f_prime);
            return;
        }

        let num_offsets = self.num_offsets.clamp(0, 128) as usize;
        let row = cur as i32 / self.max_width;
        let col = cur as i32 % self.max_width;
        for dir in 0..num_offsets {
            let diag = is_diagonal_step(dir);
            if diag && !self.diagonal_ok(cur, dir) {
                continue;
            }
            let offset = dir * 2;
            let sr = row + CELL_SHIFT[offset];
            let sc = col + CELL_SHIFT[offset + 1];
            if !self.in_bounds(sr, sc) {
                continue;
            }
            let succ = self.idx(sr, sc);
            let node = &self.map[succ];
            if node.cost >= COST_BLOCKED
                || node.h_prime == HPRIME_NOT_CALCED
                || node.h_prime >= ctx.max_h_prime
            {
                continue;
            }
            let mut step = node.cost;
            if dir > 7 {
                step = step.saturating_add(self.jump_cost);
            } else if diag {
                step += step / 2;
            }
            let dir_to_parent = reverse_shift(dir) as i32;
            if dir_to_parent == node.parent {
                self.propogate_cost_jump(ctx, succ, step, self.map[cur].g);
            } else if self.map[cur].g + step < node.g {
                self.map[succ].parent = dir_to_parent;
                self.propogate_cost_jump(ctx, succ, step, self.map[cur].g);
            }
        }
    }

    /// `calcPath(path, ...)` (non-jump). Searches from the start to the goal flagged in the
    /// window and writes the path. Returns an empty path (`num_steps == 0`, `cost == 0`) when
    /// there is no route; `cost == 1` with no steps means the start is already the goal.
    /// `map_side` is the world extent used for step positions (see [`crate::geometry::map_side`]).
    pub fn calc_path(
        &mut self,
        ctx: &mut PathContext,
        global: &GlobalMap,
        map_side: f32,
    ) -> Result<PathResult, PathError> {
        let h0 = self.calc_h_prime(self.start_r, self.start_c);
        ctx.max_h_prime = ((h0 as f64) * 2.5) as i32;
        if ctx.max_h_prime < 500 {
            ctx.max_h_prime = 500;
        }

        if !self.in_bounds(self.start_r, self.start_c) {
            return Err(PathError::OutOfMap {
                row: self.start_r + self.ul_r,
                col: self.start_c + self.ul_c,
            });
        }
        let start_idx = self.idx(self.start_r, self.start_c);
        let start_r = self.start_r;
        let start_c = self.start_c;
        {
            let h = if self.zero_h_prime {
                0
            } else {
                self.calc_h_prime(start_r, start_c)
            };
            let node = &mut self.map[start_idx];
            node.g = 0;
            node.h_prime = h;
            node.f_prime = h;
        }
        ctx.open_list.clear();
        let start_f = self.map[start_idx].f_prime;
        ctx.open_list
            .insert(PqNode {
                key: start_f,
                id: start_idx as u32,
                row: start_r,
                col: start_c,
            })
            .map_err(|_| PathError::OpenListFull)?;
        self.set_flag(start_idx, MOVEFLAG_OPEN);

        let mut goal_found = false;
        let mut best_row = -1;
        let mut best_col = -1;
        while !ctx.open_list.is_empty() {
            let best = ctx.open_list.remove();
            best_row = best.row;
            best_col = best.col;
            let best_idx = best.id as usize;
            self.clear_flag(best_idx, MOVEFLAG_OPEN);
            let best_g = self.map[best_idx].g;
            self.set_flag(best_idx, MOVEFLAG_CLOSED);
            if self.map[best_idx].flags & MOVEFLAG_GOAL != 0 {
                goal_found = true;
                break;
            }
            let num_offsets = self.num_offsets.clamp(8, 128) as usize;
            for dir in 0..num_offsets {
                let diag = is_diagonal_step(dir);
                if diag && !self.diagonal_ok(best_idx, dir) {
                    continue;
                }
                let offset = dir * 2;
                let sr = best.row + CELL_SHIFT[offset];
                let sc = best.col + CELL_SHIFT[offset + 1];
                // `JUMP-EDGE-CELLS` (test-only): the jump search of the C++ build skips row 0 and column 0 of the window.
                let c_plus_plus_edge = self.cpp_jump_edge_cells_excluded
                    && self.num_offsets > 8
                    && (sr <= 0 || sc <= 0);
                if self.in_bounds(sr, sc) && !c_plus_plus_edge {
                    let s = self.idx(sr, sc);
                    if self.map[s].flags & MOVEFLAG_OFFMAP != 0
                        && self.cannot_enter_off_map
                        && self.map[best_idx].flags & MOVEFLAG_OFFMAP == 0
                    {
                        continue;
                    }
                    if self.map[s].cost < COST_BLOCKED {
                        if self.map[s].h_prime == HPRIME_NOT_CALCED {
                            self.map[s].h_prime = self.calc_h_prime(sr, sc);
                        }
                        if self.map[s].h_prime < ctx.max_h_prime {
                            let dir_to_parent = reverse_shift(dir) as i32;
                            let mut cost = self.map[s].cost;
                            if dir > 7 {
                                cost = cost.saturating_add(self.jump_cost);
                            } else if diag {
                                cost += cost / 2;
                            }
                            let succ_g = best_g + cost;
                            if self.map[s].flags & MOVEFLAG_OPEN != 0 {
                                if succ_g < self.map[s].g {
                                    self.map[s].parent = dir_to_parent;
                                    self.map[s].g = succ_g;
                                    self.map[s].f_prime = succ_g + self.map[s].h_prime;
                                    let open_index = ctx.open_list.find(s as u32);
                                    let f = self.map[s].f_prime;
                                    ctx.open_list.change(open_index, f);
                                }
                            } else if self.map[s].flags & MOVEFLAG_CLOSED != 0 {
                                if succ_g < self.map[s].g {
                                    self.map[s].parent = dir_to_parent;
                                    if self.num_offsets > 8 {
                                        self.propogate_cost_jump(ctx, s, cost, best_g);
                                    } else {
                                        self.propogate_cost(ctx, s, cost, best_g);
                                    }
                                }
                            } else {
                                self.map[s].parent = dir_to_parent;
                                self.map[s].g = succ_g;
                                self.map[s].f_prime = succ_g + self.map[s].h_prime;
                                let node = PqNode {
                                    key: self.map[s].f_prime,
                                    id: s as u32,
                                    row: sr,
                                    col: sc,
                                };
                                ctx.open_list
                                    .insert(node)
                                    .map_err(|_| PathError::OpenListFull)?;
                                self.set_flag(s, MOVEFLAG_OPEN);
                            }
                        }
                    }
                }
            }
        }

        let mut result = PathResult {
            path: MovePath::new(),
            goal_cell: (best_row, best_col),
            trimmed: false,
        };
        if !goal_found {
            return Ok(result);
        }

        // Count the steps by walking parents back from the goal.
        let mut cur_row = best_row;
        let mut cur_col = best_col;
        let mut num_cells = 0usize;
        while cur_row != self.start_r || cur_col != self.start_c {
            num_cells += 1;
            let off = (self.map[self.idx(cur_row, cur_col)].parent as usize) << 1;
            cur_row += CELL_SHIFT[off];
            cur_col += CELL_SHIFT[off + 1];
        }
        if self.door_direction != -1 {
            num_cells += 1;
        }
        if num_cells > MAX_STEPS_PER_MOVEPATH {
            return Err(PathError::PathTooLong {
                steps: num_cells,
                max: MAX_STEPS_PER_MOVEPATH,
            });
        }

        let mut path = MovePath::new();
        let mut goal_cell = (best_row, best_col);
        let mut trimmed = false;
        if num_cells > 0 {
            path.init(num_cells);
            path.target = self.target;
            path.cost = self.map[self.idx(best_row, best_col)].g;
            let step_len = cell_length_meters();
            let cell_shift_distance: [f32; 128] = {
                let mut t = [0f32; 128];
                for (i, slot) in t.iter_mut().enumerate() {
                    *slot =
                        agsqrt(CELL_SHIFT[i * 2] as f32, CELL_SHIFT[i * 2 + 1] as f32) * step_len;
                }
                t
            };

            let mut cur_cell = num_cells as i64;
            if self.door_direction == -1 {
                path.goal = cell_center(map_side, self.ul_r + best_row, self.ul_c + best_col);
            } else {
                cur_cell -= 1;
                path.set_direction(cur_cell as usize, (self.door_direction * 2) as u8);
                let dd = self.door_direction as usize;
                let door_r = best_row + ADJ_TILE[dd][0];
                let door_c = best_col + ADJ_TILE[dd][1];
                goal_cell = (self.ul_r + door_r, self.ul_c + door_c);
                let step_dest = cell_center(map_side, goal_cell.0, goal_cell.1);
                path.set_destination(cur_cell as usize, step_dest);
                path.set_distance_to_goal(cur_cell as usize, 0.0);
                path.set_cell(cur_cell as usize, goal_cell.0, goal_cell.1);
                path.goal = step_dest;
            }

            cur_row = best_row;
            cur_col = best_col;
            while cur_row != self.start_r || cur_col != self.start_c {
                cur_cell -= 1;
                let cu = cur_cell as usize;
                let parent_field = self.map[self.idx(cur_row, cur_col)].parent as usize;
                let parent_dir = reverse_shift(parent_field) as u8;
                path.set_direction(cu, parent_dir);
                let cell_r = self.ul_r + cur_row;
                let cell_c = self.ul_c + cur_col;
                let dest = cell_center(map_side, cell_r, cell_c);
                if cur_cell == num_cells as i64 - 1 {
                    path.set_distance_to_goal(cu, 0.0);
                } else {
                    let temp_dir = path.direction(cu + 1) as usize;
                    let temp_dist = path.distance_to_goal(cu + 1);
                    path.set_distance_to_goal(cu, cell_shift_distance[temp_dir] + temp_dist);
                }
                path.set_destination(cu, dest);
                path.set_cell(cu, cell_r, cell_c);
                path.steps[cu].area = global.calc_area(cell_r, cell_c);
                let i = self.idx(cur_row, cur_col);
                self.set_flag(i, MOVEFLAG_STEP);
                let off = parent_field << 1;
                cur_row += CELL_SHIFT[off];
                cur_col += CELL_SHIFT[off + 1];
            }

            // Cut the path where it first enters the second thru area.
            if self.thru_areas[1] != -1 {
                let last = path.steps[path.num_steps - 1].area;
                if self.thru_areas[1] != last {
                    for i in 0..path.num_steps {
                        if path.steps[i].area == self.thru_areas[1] {
                            path.num_steps = i + 1;
                            path.num_steps_when_not_paused = path.num_steps;
                            let cell = path.steps[i].cell;
                            goal_cell = (cell[0], cell[1]);
                            // `path->goal` keeps the door position: only the `goalWorldPos` output follows the cut (move.cpp:5398)
                            trimmed = true;
                            break;
                        }
                    }
                }
            }
            path.steps.truncate(path.num_steps);
        }
        if num_cells == 0 {
            path.cost = 1;
        }
        result.path = path;
        result.goal_cell = goal_cell;
        result.trimmed = trimmed;
        Ok(result)
    }
}

#[cfg(test)]
mod jump_propagation_tests {
    use super::*;

    #[test]
    fn cheaper_closed_route_propagates_through_jump_descendants() {
        let mut map = MoveMap::new(30, 30);
        map.num_offsets = 9;
        map.jump_cost = 10;
        map.min_row = 0;
        map.max_row = 29;
        map.min_col = 0;
        map.max_col = 29;
        let cur = map.idx(10, 10);
        let child = map.idx(8, 10);
        let grandchild = map.idx(6, 10);
        for (idx, g, parent) in [
            (cur, 50, -1),
            (child, 100, reverse_shift(8) as i32),
            (grandchild, 120, reverse_shift(8) as i32),
        ] {
            map.map[idx].cost = 5;
            map.map[idx].g = g;
            map.map[idx].h_prime = 1;
            map.map[idx].parent = parent;
            map.map[idx].flags = MOVEFLAG_CLOSED;
        }
        let mut ctx = PathContext::new();
        ctx.max_h_prime = 500;

        map.propogate_cost_jump(&mut ctx, cur, 1, 0);

        assert_eq!(map.map[child].g, 16);
        assert_eq!(map.map[grandchild].g, 31);
        assert_eq!(map.map[child].parent, reverse_shift(8) as i32);
        assert_eq!(map.map[grandchild].parent, reverse_shift(8) as i32);
    }
}
