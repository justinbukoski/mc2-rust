// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! World/cell geometry used by the path code. Constants from `mclib/terrain.cpp`
//! (`worldUnitsPerVertex`, `MAPCELL_DIM`, `metersPerWorldUnit`) and the cell-centre
//! formulas from `move.cpp` (`cellToWorld` inlined in `MoveMap::calcPath`).

/// Cells per terrain tile side (`MAPCELL_DIM` in terrain.h).
pub const MAPCELL_DIM: i32 = 3;
/// `Terrain::worldUnitsPerVertex`.
pub const WORLD_UNITS_PER_VERTEX: f32 = 128.0;
/// `Terrain::worldUnitsPerCell` = worldUnitsPerVertex / MAPCELL_DIM (float division, as in C++).
pub const WORLD_UNITS_PER_CELL: f32 = WORLD_UNITS_PER_VERTEX / MAPCELL_DIM as f32;
/// `metersPerWorldUnit` (terrain.cpp).
pub const METERS_PER_WORLD_UNIT: f32 = 0.2;

/// Stuff::Vector3D equivalent (only what the path code stores).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Total world extent of a map `cell_width` cells across: `realVerticesMapSide * worldUnitsPerVertex`
/// where `realVerticesMapSide = cell_width / MAPCELL_DIM`.
pub fn map_side(cell_width: i32) -> f32 {
    let real_vertices = cell_width / MAPCELL_DIM;
    real_vertices as f32 * WORLD_UNITS_PER_VERTEX
}

/// World position of the centre of cell (`row`, `col`), y pointing up, map centred on origin.
pub fn cell_center(map_side: f32, row: i32, col: i32) -> Vec3 {
    let wpc = WORLD_UNITS_PER_CELL;
    Vec3 {
        x: (col as f32) * wpc + wpc / 2.0 - map_side / 2.0,
        y: (map_side / 2.0) - (row as f32) * wpc - wpc / 2.0,
        z: 0.0,
    }
}

/// `agsqrt(a, b)` from move.cpp.
pub fn agsqrt(a: f32, b: f32) -> f32 {
    (a * a + b * b).sqrt()
}

/// Metres per cell step length, as used for `distanceToGoal` (`worldUnitsPerCell * metersPerWorldUnit`).
pub fn cell_length_meters() -> f32 {
    WORLD_UNITS_PER_CELL * METERS_PER_WORLD_UNIT
}
