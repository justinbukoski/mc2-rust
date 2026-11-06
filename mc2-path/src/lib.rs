// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Movement and pathfinding for MechCommander 2.
//!
//! Port of the game-logic parts of `mclib/move.cpp` (+ `move.h`), `pqueue.cpp/.h`,
//! and the map-packet reading that `MOVE_readData` performs. See
//! `rust/docs/porting-log.md` for provenance and the list of deliberate deviations.
//!
//! Layering:
//! - [`pqueue`]: the binary-heap open list (identical ordering and tie behaviour).
//! - [`mission`]: per-cell passability/terrain flags ([`MissionMap`], [`MapCell`]).
//! - [`global`]: the area/door graph ([`GlobalMap`]) and the area-level A*.
//! - [`movemap`]: the cell-level A* over a bounded window ([`MoveMap`]).
//! - [`movepath`]: output path structures ([`MovePath`], [`PathStep`]).
//! - [`planner`]: [`MoveData`] (all move packets of one mission) and [`Planner`] queries.
//!
//! Data arrives as decoded packet payloads through [`PacketSource`]; this crate does not
//! parse the `.fst`/`.pak` containers (that is `mc2-formats`' job).

pub mod geometry;
pub mod global;
pub mod mission;
pub mod movemap;
pub mod movepath;
pub mod packets;
pub mod planner;
pub mod pqueue;
mod tables;

pub use geometry::{cell_center, map_side, Vec3};
pub use global::{
    AreaType, DoorInfo, DoorLink, GlobalMap, GlobalMapArea, GlobalMapDoor, GlobalMapParts,
    GlobalPathStep, GLOBALPATH_EXISTS_FALSE, GLOBALPATH_EXISTS_TRUE, GLOBALPATH_EXISTS_UNKNOWN,
    GLOBAL_CONFIDENCE_AT_LEAST, GLOBAL_CONFIDENCE_BAD, GLOBAL_CONFIDENCE_GOOD,
};
pub use mission::{MapCell, MissionMap};
pub use movemap::{GateOracle, MoveMap, MoveMapNode, NoGates, PathContext, PathError, PathResult};
pub use movepath::{MovePath, PathStep};
pub use packets::{LoadError, PacketSource};
pub use planner::{FootPrint, MoveData, Mover, MoverKind, PlannedPath, Planner};
pub use pqueue::{PqNode, PriorityQueue};
