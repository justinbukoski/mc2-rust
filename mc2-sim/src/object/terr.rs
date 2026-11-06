// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The terrain queries lane A needs, in one place (thin adapter over lane T's [`Terrain`], mclib/terrain.h, mapdata, move.h).

use crate::math::Vec3;
use crate::terrain::Terrain;

pub use crate::terrain::VERTICES_BLOCK_SIDE;

/// `Terrain::realVerticesMapSide`.
pub fn vertices_per_side(t: &Terrain) -> i32 {
    t.vertices_per_side
}

/// `Terrain::blocksMapSide` (`realVerticesMapSide / verticesBlockSide`).
pub fn blocks_per_side(t: &Terrain) -> i32 {
    t.vertices_per_side / VERTICES_BLOCK_SIDE
}

/// `Terrain::numObjBlocks`.
pub fn num_obj_blocks(t: &Terrain) -> i32 {
    let b = blocks_per_side(t);
    b * b
}

/// `worldToTile`: `(tileR, tileC)`, truncated toward zero, no range check (terrain.h:335).
pub fn world_to_tile(t: &Terrain, p: &Vec3) -> (i32, i32) {
    t.world_to_tile_rc(p)
}

/// `worldToCell`: `(cellR, cellC)`, truncated toward zero (terrain.h:346).
pub fn world_to_cell(t: &Terrain, p: &Vec3) -> (i32, i32) {
    t.world_to_cell(p)
}

/// `worldToTileCell`: `(tileR, tileC, cellR, cellC)`; a position off the map resets the tile to (0,0) (terrain.h:354).
pub fn world_to_tile_cell(t: &Terrain, p: &Vec3) -> (i32, i32, i32, i32) {
    t.world_to_tile_cell(p)
}

/// `cellToWorld(row, col)`: the cell centre with `z = 0`; `(0,0,0)` out of range (terrain.h:405).
pub fn cell_to_world(t: &Terrain, row: i32, col: i32) -> Vec3 {
    t.cell_to_world(row, col)
}

/// `getTerrainElevation(position)`.
pub fn elevation(t: &Terrain, p: &Vec3) -> f32 {
    t.elevation(p)
}

/// `GameMap->getPassable(row, col)` (out-of-range cells read as impassable).
pub fn get_passable(t: &Terrain, row: i32, col: i32) -> bool {
    t.get_passable(row, col)
}

/// `GameMap->setPassable(row, col, passable)`.
pub fn set_passable(t: &mut Terrain, row: i32, col: i32, passable: bool) {
    t.set_passable(row, col, passable);
}

/// `GameMap->setGate(row, col, on)`.
pub fn set_gate(t: &mut Terrain, row: i32, col: i32, on: bool) {
    t.set_gate(row, col, u32::from(on));
}

/// `GameMap->setLocalHeight(row, col, value)`.
pub fn set_local_height(t: &mut Terrain, row: i32, col: i32, value: u32) {
    t.set_local_height(row, col, value);
}

/// `GameMap->getMine(row, col)`.
pub fn get_mine(t: &Terrain, row: i32, col: i32) -> u32 {
    t.get_mine(row, col)
}

/// `GameMap->setMine(row, col, mine)`.
pub fn set_mine(t: &mut Terrain, row: i32, col: i32, mine: u32) {
    t.set_mine(row, col, mine);
}

/// `GameMap->inBounds(row, col)`.
pub fn in_bounds(t: &Terrain, row: i32, col: i32) -> bool {
    t.in_bounds(row, col)
}

/// `GlobalMoveMap[level]` (0 ground, 1 hover): lane T owns the loaded move data.
pub fn global_map(t: &Terrain, level: usize) -> &mc2_path::GlobalMap {
    &t.move_data().global[level]
}

/// Mutable [`global_map`].
pub fn global_map_mut(t: &mut Terrain, level: usize) -> &mut mc2_path::GlobalMap {
    &mut t.move_data_mut().global[level]
}

/// `tempSpecialAreaFootPrints` (the special-area footprints `MOVE_readData` loaded).
pub fn special_areas(t: &Terrain) -> &[mc2_path::FootPrint] {
    &t.move_data().special_areas
}
