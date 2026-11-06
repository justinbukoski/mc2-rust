// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission move data and the path queries built on it.
//!
//! [`MoveData::load`] reads what `MOVE_readData` reads (mission map, the ground, hover and
//! heli global maps, and the special-area footprints) and checks every index the searches
//! will follow. [`Planner`] answers path queries:
//! - [`Planner::global_path`]: the area-level route (`GlobalMap::calcPath`);
//! - [`Planner::simple_path`]: a cell path inside one square window (`setUp` rectangle form),
//!   with the origin `Mover::calcMovePath` uses ([`Planner::simple_window_origin`]);
//! - [`Planner::plan`]: the full route, one sector search per area crossed.
//!
//! [`Mover`]s carry the runtime's costs: clear cost from maximum movement speed, shallow water
//! for mechs (and hover movers, which also take deep water), including jet-assisted jump steps.
//!
//! What `plan` does not reproduce: `warrior.cpp`'s `calcMovePath` pairs two areas per
//! sector search, adds path-lock/ram/group handling, and chooses quick versus complex
//! moves. Its waypoints therefore match the C++ only for the primitives above.

use crate::geometry::{cell_length_meters, map_side, Vec3};
use crate::global::{GlobalMap, GlobalPathStep};
use crate::mission::{MissionMap, MISSION_MAP_PACKETS};
use crate::movemap::{
    GateOracle, MoveMap, PathContext, PathError, PathResult, MOVEPARAM_FOLLOW_ROADS,
    MOVEPARAM_NONE, MOVEPARAM_STATIONARY_MOVERS, MOVEPARAM_SWEEP_MINES, MOVEPARAM_WATER_DEEP,
    MOVEPARAM_WATER_SHALLOW, SECTOR_DIM,
};
use crate::movepath::PathStep;
use crate::packets::{i16_at, i32_at, LoadError, PacketSource};

/// `SimpleMovePathRange` default (MOVE_init requires > 20).
pub const DEFAULT_SIMPLE_RANGE: i32 = 21;
/// `MAX_GAME_OBJECT_CELLS`.
pub const MAX_GAME_OBJECT_CELLS: usize = 64;
/// Bytes per `GameObjectFootPrint` record (pack(1)).
pub const FOOTPRINT_BYTES: usize = 8 + MAX_GAME_OBJECT_CELLS * 4;

/// `GameObjectFootPrint`: the cells a special (wall/gate/building) area occupies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FootPrint {
    pub cell_position_row: i16,
    pub cell_position_col: i16,
    pub pre_num_cells: i16,
    pub num_cells: i16,
    pub cells: [[i16; 2]; MAX_GAME_OBJECT_CELLS],
}

/// Everything `MOVE_readData` loads for one mission.
#[derive(Clone, Debug)]
pub struct MoveData {
    pub mission: MissionMap,
    /// Indexed by movement level: 0 ground, 1 hover, 2 heli.
    pub global: [GlobalMap; 3],
    pub special_areas: Vec<FootPrint>,
    /// Packets consumed, as `MOVE_readData` returns.
    pub packets_used: usize,
}

impl MoveData {
    pub const LEVEL_GROUND: usize = 0;
    pub const LEVEL_HOVER: usize = 1;
    pub const LEVEL_HELI: usize = 2;

    /// Load the move packets starting at `first` (the C++ `whichPacket`, 4 in mission paks).
    pub fn load(src: &dyn PacketSource, first: usize) -> Result<MoveData, LoadError> {
        let mission = MissionMap::from_packets(src, first)?;
        let mut p = first + MISSION_MAP_PACKETS;
        let (g0, n0) = GlobalMap::from_packets(src, p)?;
        p += n0;
        let (mut g1, n1) = GlobalMap::from_packets(src, p)?;
        g1.hover = true;
        p += n1;
        let (mut g2, n2) = GlobalMap::from_packets(src, p)?;
        g2.blank = true;
        p += n2;

        // Special areas: a count packet, then one footprint packet. The count is written as
        // sizeof(long): 4 bytes on Win32 builds, 8 on LP64 builds, so its width is read from
        // the packet length. The footprint packet's length is checked against the count
        // before anything is allocated. A zero count is followed by a filler packet of the
        // same width.
        let count_bytes = src.packet(p).ok_or(LoadError::MissingPacket { index: p })?;
        let width = count_bytes.len();
        let count: u64 = match width {
            4 => {
                let v = i32_at(count_bytes, 0);
                if v < 0 {
                    return Err(LoadError::BadValue {
                        what: "special area count",
                        value: v as i64,
                    });
                }
                v as u64
            }
            8 => {
                let mut raw = [0u8; 8];
                raw.copy_from_slice(count_bytes);
                let v = i64::from_le_bytes(raw);
                if v < 0 {
                    return Err(LoadError::BadValue {
                        what: "special area count",
                        value: v,
                    });
                }
                v as u64
            }
            other => {
                return Err(LoadError::BadPacketSize {
                    index: p,
                    what: "special area count",
                    expected: 4,
                    actual: other,
                })
            }
        };
        p += 1;
        let fp = src.packet(p).ok_or(LoadError::MissingPacket { index: p })?;
        let mut special_areas = Vec::new();
        if count == 0 {
            if fp.len() != width {
                return Err(LoadError::BadPacketSize {
                    index: p,
                    what: "zero special-area filler",
                    expected: width,
                    actual: fp.len(),
                });
            }
        } else {
            let need = count
                .checked_mul(FOOTPRINT_BYTES as u64)
                .ok_or(LoadError::BadValue {
                    what: "special area count",
                    value: count as i64,
                })?;
            if fp.len() as u64 != need {
                return Err(LoadError::BadPacketSize {
                    index: p,
                    what: "special area footprints",
                    expected: need as usize,
                    actual: fp.len(),
                });
            }
            // The count is now known to match the packet, so this allocation is bounded by it.
            special_areas.reserve_exact(count as usize);
            for rec in fp.as_chunks::<FOOTPRINT_BYTES>().0 {
                let mut cells = [[0i16; 2]; MAX_GAME_OBJECT_CELLS];
                for (i, cell) in cells.iter_mut().enumerate() {
                    cell[0] = i16_at(rec, 8 + i * 4);
                    cell[1] = i16_at(rec, 10 + i * 4);
                }
                special_areas.push(FootPrint {
                    cell_position_row: i16_at(rec, 0),
                    cell_position_col: i16_at(rec, 2),
                    pre_num_cells: i16_at(rec, 4),
                    num_cells: i16_at(rec, 6),
                    cells,
                });
            }
        }
        p += 1;
        Ok(MoveData {
            mission,
            global: [g0, g1, g2],
            special_areas,
            packets_used: p - first,
        })
    }
}

/// The kinds of mover the runtime distinguishes when it sets up a path search
/// (`mover.cpp` `calcMovePath`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoverKind {
    /// Walking mech: level 0, may wade shallow water.
    Mech,
    /// Ground vehicle: level 0, no shallow-water access.
    GroundVehicle,
    /// Hover vehicle: level 1, takes shallow and deep water.
    Hover,
    /// Helicopter or other aerial mover: level 2.
    Heli,
}

/// Mover description: which movement class, team, and cost/param settings a query uses.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mover {
    /// Watch id used by `MoveMap` callbacks to ignore the querying mover's own map marker.
    pub watch_id: i32,
    /// 0 ground, 1 hover, 2 heli.
    pub level: usize,
    pub team_id: i8,
    /// `MOVEPARAM_*` bits.
    pub params: u32,
    /// Maximum movement speed; the clear-cell cost is derived from it (see
    /// [`Mover::sector_clear_cost`] and [`Mover::simple_clear_cost`]).
    pub max_move_speed: f32,
    pub jump_cost: i32,
    /// Neighbour count; above 8 enables jump steps.
    pub offsets: i32,
    pub withdrawing: bool,
    /// `Mover::isLayingMines()`, passed to `MoveMap::setMover`.
    pub laying_mines: bool,
}

impl Mover {
    /// `clearCost` for sector and local-window searches (`Mover::calcMovePath`, sector
    /// branch, code/mover.cpp ~5066-5110): `(long)(cellLength / maxMoveSpeed * 50.0)`. The value
    /// is truncated and not narrowed. Zero means the runtime runs no search.
    pub fn sector_clear_cost(&self) -> i32 {
        clear_cost_long(self.max_move_speed)
    }

    /// `clearCost` for the short-range search (`Mover::calcMovePath`, simple branch,
    /// code/mover.cpp ~5004-5006): `(float2short)(cellLength / maxMoveSpeed * 50.0)`. That
    /// function returns a `short`, so the value wraps like the x86 build's narrowing does.
    pub fn simple_clear_cost(&self) -> i32 {
        clear_cost_short(self.max_move_speed)
    }

    /// A mover of `kind` with the runtime's water flags: shallow water for mechs, and shallow
    /// plus deep for level-1 (hover) movers. Clear cost comes from `max_move_speed`.
    pub fn new(kind: MoverKind, team_id: i8, max_move_speed: f32) -> Mover {
        let level = match kind {
            MoverKind::Mech | MoverKind::GroundVehicle => 0,
            MoverKind::Hover => 1,
            MoverKind::Heli => 2,
        };
        let mut params = MOVEPARAM_NONE;
        if kind == MoverKind::Mech {
            params |= MOVEPARAM_WATER_SHALLOW;
        }
        if level == 1 {
            params |= MOVEPARAM_WATER_SHALLOW | MOVEPARAM_WATER_DEEP;
        }
        Mover {
            watch_id: 0,
            level,
            team_id,
            params,
            max_move_speed,
            jump_cost: 0,
            offsets: 8,
            withdrawing: false,
            laying_mines: false,
        }
    }

    /// Attach the mover's `GameObjectWatchID` to path-search callbacks.
    pub fn with_watch_id(mut self, watch_id: i32) -> Mover {
        self.watch_id = watch_id;
        self
    }

    /// Walking mech (level 0, shallow water allowed).
    pub fn mech(team_id: i8, max_move_speed: f32) -> Mover {
        Self::new(MoverKind::Mech, team_id, max_move_speed)
    }

    /// Ground vehicle (level 0, no shallow water).
    pub fn ground_vehicle(team_id: i8, max_move_speed: f32) -> Mover {
        Self::new(MoverKind::GroundVehicle, team_id, max_move_speed)
    }

    /// Hover vehicle (level 1, shallow and deep water).
    pub fn hover(team_id: i8, max_move_speed: f32) -> Mover {
        Self::new(MoverKind::Hover, team_id, max_move_speed)
    }

    /// Helicopter (level 2).
    pub fn heli(team_id: i8, max_move_speed: f32) -> Mover {
        Self::new(MoverKind::Heli, team_id, max_move_speed)
    }

    /// Set the jump settings read from `getJumpRange`.
    pub fn with_jump(mut self, jump_cost: i32, offsets: i32) -> Mover {
        self.jump_cost = jump_cost;
        self.offsets = offsets;
        self
    }

    /// `MOVEPARAM_FOLLOW_ROADS`, set when the mover follows roads.
    pub fn with_follow_roads(mut self, on: bool) -> Mover {
        self.set_param(MOVEPARAM_FOLLOW_ROADS, on);
        self
    }

    /// `MOVEPARAM_SWEEP_MINES`, set for mine sweepers.
    pub fn with_sweep_mines(mut self, on: bool) -> Mover {
        self.set_param(MOVEPARAM_SWEEP_MINES, on);
        self
    }

    fn set_param(&mut self, bit: u32, on: bool) {
        if on {
            self.params |= bit;
        } else {
            self.params &= !bit;
        }
    }
}

/// A full route from `Planner::plan`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedPath {
    /// Cells from the first step after the start to the goal (absolute cells).
    pub steps: Vec<PathStep>,
    /// The area-level route it follows.
    pub global: Vec<GlobalPathStep>,
}

/// `cellLength / speed * 50` as the C++ computes it: float division, then a double multiply.
fn clear_cost_raw(speed: f32) -> f64 {
    (cell_length_meters() / speed) as f64 * 50.0
}

/// Upper bound on a clear cost, so the search's `i32` arithmetic cannot overflow. Only
/// speeds far below any mover's reach hit it.
const CLEAR_COST_CAP: i64 = 1 << 24;

/// Sector clear cost: `(long)` truncation of the raw value.
fn clear_cost_long(speed: f32) -> i32 {
    if speed == 0.0 || speed.is_nan() {
        return 0;
    }
    (clear_cost_raw(speed) as i64).clamp(-CLEAR_COST_CAP, CLEAR_COST_CAP) as i32
}

/// Simple-search clear cost: `float2short`, i.e. `floor` then a `short` conversion. The
/// conversion wraps modulo 2^16 for values past `short` range, as the x86 build does.
fn clear_cost_short(speed: f32) -> i32 {
    if speed == 0.0 || speed.is_nan() {
        return 0;
    }
    let as_float = clear_cost_raw(speed) as f32;
    (as_float.floor() as i64) as i16 as i32
}

/// Path queries over one loaded mission.
pub struct Planner {
    pub data: MoveData,
    ctx: PathContext,
    sector: MoveMap,
    simple: MoveMap,
    simple_range: i32,
    stationary_movers: Vec<(i32, i32)>,
}

impl Planner {
    pub fn new(data: MoveData) -> Planner {
        Self::with_simple_range(data, DEFAULT_SIMPLE_RANGE)
    }

    /// Test-only switch `JUMP-EDGE-CELLS` (`DIFFERENCES.md`): both windows refuse row 0 and column 0 to jump-capable searches, as the
    /// community port's `calcPathJUMP` does (move.cpp:5605). The shipped game never sets it.
    pub fn set_cpp_jump_edge_cells_excluded(&mut self, on: bool) {
        self.sector.cpp_jump_edge_cells_excluded = on;
        self.simple.cpp_jump_edge_cells_excluded = on;
    }

    /// `MOVE_init(range)`: the simple window is `2*range+1` cells square.
    pub fn with_simple_range(data: MoveData, range: i32) -> Planner {
        let dim = range * 2 + 1;
        Planner {
            data,
            ctx: PathContext::new(),
            sector: MoveMap::new(SECTOR_DIM * 2, SECTOR_DIM * 2),
            simple: MoveMap::new(dim, dim),
            simple_range: range,
            stationary_movers: Vec::new(),
        }
    }

    /// The half width of the short-range window this planner was built with (`SimpleMovePathRange`).
    pub fn simple_range(&self) -> i32 {
        self.simple_range
    }

    /// Supply the stationary mover cells for `MOVEPARAM_STATIONARY_MOVERS` searches, as window indices (see
    /// [`MoveMap::place_stationary_movers`]).
    pub fn set_stationary_movers(&mut self, cells: Vec<(i32, i32)>) {
        self.stationary_movers = cells;
    }

    /// Area-level route from `start` to `goal` for this mover's movement class.
    pub fn global_path(
        &mut self,
        mover: &Mover,
        start: (i32, i32),
        goal: (i32, i32),
        oracle: &dyn GateOracle,
    ) -> Result<Vec<GlobalPathStep>, PathError> {
        let g = &mut self.data.global[mover.level];
        g.mover_team_id = mover.team_id;
        g.use_closed_areas = false;
        g.calc_path_cells(&mut self.ctx, start, goal, oracle)
    }

    /// Origin of the 60x60 local window (two sectors square) for a search from `start` to
    /// `goal`, as the sector branch of `Mover::calcMovePath` sets it (code/mover.cpp ~5066-5110):
    /// the smaller of the two sector origins on each axis. Both endpoints must lie inside it.
    pub fn local_window_origin(start: (i32, i32), goal: (i32, i32)) -> (i32, i32) {
        let sr = (start.0 / SECTOR_DIM).min(goal.0 / SECTOR_DIM) * SECTOR_DIM;
        let sc = (start.1 / SECTOR_DIM).min(goal.1 / SECTOR_DIM) * SECTOR_DIM;
        (sr, sc)
    }

    /// Origin of the short-range window, as `Mover::calcMovePath` sets it: `pos - range`, clamped
    /// only at the top and left (0). The window may run past the bottom and right edges; cells
    /// off the map stay blocked.
    pub fn simple_window_origin(&self, start: (i32, i32)) -> (i32, i32) {
        (
            (start.0 - self.simple_range).max(0),
            (start.1 - self.simple_range).max(0),
        )
    }

    /// Cell path inside a square window of side `2*range+1` (see [`Planner::simple_window_origin`]).
    pub fn simple_path(
        &mut self,
        mover: &Mover,
        start: (i32, i32),
        goal: (i32, i32),
        oracle: &dyn GateOracle,
    ) -> Result<PathResult, PathError> {
        let clear_cost = mover.simple_clear_cost();
        if clear_cost <= 0 {
            return Ok(no_search_result());
        }
        let dim = self.simple_range * 2 + 1;
        let w = self.data.mission.width();
        let (ul_r, ul_c) = self.simple_window_origin(start);
        self.simple.set_mover(
            mover.watch_id,
            mover.team_id,
            mover.laying_mines,
            mover.withdrawing,
        );
        self.simple.set_up_rect(
            &self.data.mission,
            &self.data.global[mover.level],
            mover.level,
            (ul_r, ul_c),
            dim,
            start,
            goal,
            mover.simple_clear_cost(),
            mover.jump_cost,
            mover.offsets,
            mover.params,
            oracle,
        )?;
        if mover.params & MOVEPARAM_STATIONARY_MOVERS != 0 {
            self.simple.place_stationary_movers(&self.stationary_movers);
        }
        self.simple
            .calc_path(&mut self.ctx, &self.data.global[mover.level], map_side(w))
    }

    /// `Mover::calcMovePath(path, MOVEPATH_COMPLEX, start, goal, goalCell, params)` (mover.cpp:5093-5170): one search over the
    /// two-sector window ([`Planner::local_window_origin`]) from `start` to `goal`, with no area restriction.
    pub fn window_path(
        &mut self,
        mover: &Mover,
        start: (i32, i32),
        goal: (i32, i32),
        oracle: &dyn GateOracle,
    ) -> Result<PathResult, PathError> {
        if mover.sector_clear_cost() <= 0 {
            return Ok(no_search_result());
        }
        let ul = Self::local_window_origin(start, goal);
        self.sector.set_mover(
            mover.watch_id,
            mover.team_id,
            mover.laying_mines,
            mover.withdrawing,
        );
        self.sector.area_filter = None;
        self.sector.set_up_rect(
            &self.data.mission,
            &self.data.global[mover.level],
            mover.level,
            ul,
            SECTOR_DIM * 2,
            start,
            goal,
            mover.sector_clear_cost(),
            mover.jump_cost,
            mover.offsets,
            mover.params,
            oracle,
        )?;
        if mover.params & MOVEPARAM_STATIONARY_MOVERS != 0 {
            self.sector.place_stationary_movers(&self.stationary_movers);
        }
        let w = self.data.mission.width();
        self.sector
            .calc_path(&mut self.ctx, &self.data.global[mover.level], map_side(w))
    }

    /// One sector search: from `start` through `thru` to `goal_door` (or to `final_goal`
    /// if that lies beyond the door). Returns the path with absolute cells.
    pub fn segment_path(
        &mut self,
        mover: &Mover,
        start: (i32, i32),
        thru: [i32; 2],
        goal_door: usize,
        final_goal: (i32, i32),
        oracle: &dyn GateOracle,
    ) -> Result<PathResult, PathError> {
        if mover.sector_clear_cost() <= 0 {
            return Ok(no_search_result());
        }
        self.sector.set_mover(
            mover.watch_id,
            mover.team_id,
            mover.laying_mines,
            mover.withdrawing,
        );
        self.sector.set_up_area(
            &self.data.mission,
            &self.data.global[mover.level],
            mover.level,
            start,
            thru,
            goal_door,
            final_goal,
            mover.sector_clear_cost(),
            mover.jump_cost,
            mover.offsets,
            mover.params,
            oracle,
        )?;
        if mover.params & MOVEPARAM_STATIONARY_MOVERS != 0 {
            self.sector.place_stationary_movers(&self.stationary_movers);
        }
        let w = self.data.mission.width();
        self.sector
            .calc_path(&mut self.ctx, &self.data.global[mover.level], map_side(w))
    }

    /// Full route: the area sequence from [`Planner::global_path`], then one sector search
    /// per area that exits through a door, then a window search inside the goal area.
    pub fn plan(
        &mut self,
        mover: &Mover,
        start: (i32, i32),
        goal: (i32, i32),
        oracle: &dyn GateOracle,
    ) -> Result<PlannedPath, PathError> {
        if mover.sector_clear_cost() <= 0 {
            // The runtime runs no sector search at this speed.
            return Err(PathError::NoRoute);
        }
        let global = self.global_path(mover, start, goal, oracle)?;
        if global.is_empty() {
            return Err(PathError::NoRoute);
        }
        let mut steps: Vec<PathStep> = Vec::new();
        let mut cur = start;
        let num_real = global.len() - 1;
        for step in global.iter().take(num_real) {
            let res = self.segment_path(
                mover,
                cur,
                [step.thru_area, -1],
                step.goal_door as usize,
                goal,
                oracle,
            )?;
            if res.path.num_steps == 0 && res.path.cost != 1 {
                return Err(PathError::NoRoute);
            }
            steps.extend_from_slice(res.path.path_steps());
            cur = res.goal_cell;
        }
        // Last leg: the 60x60 window of the sector branch (see local_window_origin), limited to
        // the areas the global route crosses, so the search cannot detour through another area.
        // (The C++ sector branch does not limit areas; this keeps the plan on its global route.)
        let goal_area = self.data.global[mover.level].calc_area(goal.0, goal.1);
        if goal_area < 0 {
            return Err(PathError::NoArea);
        }
        let mut allowed: Vec<i32> = global.iter().map(|s| s.thru_area).collect();
        allowed.push(goal_area);
        let ul = Self::local_window_origin(cur, goal);
        self.sector.set_mover(
            mover.watch_id,
            mover.team_id,
            mover.laying_mines,
            mover.withdrawing,
        );
        self.sector.area_filter = Some(allowed);
        let setup = self.sector.set_up_rect(
            &self.data.mission,
            &self.data.global[mover.level],
            mover.level,
            ul,
            SECTOR_DIM * 2,
            cur,
            goal,
            mover.sector_clear_cost(),
            mover.jump_cost,
            mover.offsets,
            mover.params,
            oracle,
        );
        if setup.is_ok() && mover.params & MOVEPARAM_STATIONARY_MOVERS != 0 {
            self.sector.place_stationary_movers(&self.stationary_movers);
        }
        let searched = match setup {
            Ok(()) => {
                let w = self.data.mission.width();
                self.sector
                    .calc_path(&mut self.ctx, &self.data.global[mover.level], map_side(w))
            }
            Err(e) => Err(e),
        };
        self.sector.area_filter = None;
        let res = searched?;
        if res.path.num_steps == 0 && res.path.cost != 1 {
            return Err(PathError::NoRoute);
        }
        steps.extend_from_slice(res.path.path_steps());
        Ok(PlannedPath { steps, global })
    }

    /// World position of the centre of a cell on the loaded mission.
    pub fn cell_world(&self, row: i32, col: i32) -> Vec3 {
        crate::geometry::cell_center(map_side(self.data.mission.width()), row, col)
    }
}

/// Result of a search the runtime never starts (clear cost 0): empty path, cost 0.
fn no_search_result() -> PathResult {
    PathResult {
        path: crate::movepath::MovePath::new(),
        goal_cell: (-1, -1),
        trimmed: false,
    }
}
