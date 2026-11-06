// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Per-cell map accessors: the `MissionMap` / `MapCell` getters and setters (`mclib/move.h` 191-860) every lane uses
//! (mover bit, path lock, gate, mine, forest, passable, ...), `MissionMap::getPassable(Vector3D)`, `placeObject` and
//! `spreadState` (`mclib/move.cpp` 901-1056).
//!
//! The setters operate on the raw 32-bit `MapCell::data` exactly like the C++ (`data &= ~MASK; data |= value << SHIFT`,
//! no masking of the stored value), through [`Terrain::with_cell_mut`], so a write outside the map is ignored and a read
//! outside the map returns zero/false (see the module docs of [`super`]).

use super::{Terrain, METERS_PER_WORLD_UNIT, WORLD_UNITS_PER_CELL};
use crate::math::Vec3;
use mc2_path::mission as bits;

const TERRAIN_SHIFT: u32 = 0;
const OVERLAY_SHIFT: u32 = 4;
const GATE_SHIFT: u32 = 8;
const MINE_SHIFT: u32 = 13;
const HEIGHT_SHIFT: u32 = 18;
/// `MAPCELL_PATHLOCK_BASE`.
const PATHLOCK_BASE: u32 = bits::MAPCELL_PATHLOCK_BASE;

fn put(data: &mut u32, mask: u32, value: u32) {
    *data &= !mask;
    *data |= value;
}

fn put_flag(data: &mut u32, mask: u32, on: bool) {
    *data &= !mask;
    if on {
        *data |= mask;
    }
}

/// `rowShift` / `colShift` (move.cpp:85-86), the 8 neighbour directions.
const ROW_SHIFT: [i32; 8] = [-1, -1, 0, 1, 1, 1, 0, -1];
const COL_SHIFT: [i32; 8] = [0, 1, 1, 1, 0, -1, -1, -1];

impl Terrain {
    fn data_at(&self, row: i32, col: i32) -> u32 {
        self.cell(row, col).map_or(0, |c| c.0)
    }

    /// `MapCell::getTerrain`.
    pub fn cell_terrain(&self, row: i32, col: i32) -> u32 {
        (self.data_at(row, col) & bits::MAPCELL_TERRAIN_MASK) >> TERRAIN_SHIFT
    }
    /// `MapCell::setTerrain`.
    pub fn set_cell_terrain(&mut self, row: i32, col: i32, terrain: u32) {
        self.with_cell_mut(row, col, |c| {
            put(
                &mut c.0,
                bits::MAPCELL_TERRAIN_MASK,
                terrain << TERRAIN_SHIFT,
            )
        });
    }
    /// `MapCell::getOverlay` (2 bits).
    pub fn cell_overlay(&self, row: i32, col: i32) -> u32 {
        (self.data_at(row, col) & bits::MAPCELL_OVERLAY_MASK) >> OVERLAY_SHIFT
    }
    /// `MapCell::setOverlay`.
    pub fn set_cell_overlay(&mut self, row: i32, col: i32, overlay: u32) {
        self.with_cell_mut(row, col, |c| {
            put(
                &mut c.0,
                bits::MAPCELL_OVERLAY_MASK,
                overlay << OVERLAY_SHIFT,
            )
        });
    }

    /// `MissionMap::getMover`.
    pub fn get_mover(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_MOVER_MASK != 0
    }
    /// `MissionMap::setMover`.
    pub fn set_mover(&mut self, row: i32, col: i32, mover_here: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_MOVER_MASK, mover_here)
        });
    }

    /// `MissionMap::getGate` (0 or 1 for map data; the field is one bit wide).
    pub fn get_gate(&self, row: i32, col: i32) -> u32 {
        (self.data_at(row, col) & bits::MAPCELL_GATE_MASK) >> GATE_SHIFT
    }
    /// `MissionMap::setGate(row, col, gate)`: `data |= gate << 8` with the old bit cleared first.
    pub fn set_gate(&mut self, row: i32, col: i32, gate: u32) {
        self.with_cell_mut(row, col, |c| {
            put(&mut c.0, bits::MAPCELL_GATE_MASK, gate << GATE_SHIFT)
        });
    }

    /// `MissionMap::getPassable(row, col)`.
    pub fn get_passable(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_PASSABLE_MASK != 0
    }
    /// `MissionMap::setPassable(row, col, passable)`.
    pub fn set_passable(&mut self, row: i32, col: i32, passable: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_PASSABLE_MASK, passable)
        });
    }
    /// `MissionMap::getPassable(Vector3D cellPosition)`: `worldToCell` then the cell test (false outside the map).
    pub fn passable_at(&self, p: &Vec3) -> bool {
        let (r, c) = self.world_to_cell(p);
        self.get_passable(r, c)
    }

    /// `MissionMap::getPathlock(level, row, col)`; `level` is the movement level (0 ground, 1 hover) as a `DWORD` shift.
    pub fn get_pathlock(&self, level: u32, row: i32, col: i32) -> bool {
        self.data_at(row, col) & PATHLOCK_BASE.wrapping_shl(level) != 0
    }
    /// `MissionMap::setPathlock(level, row, col, pathlock)`.
    pub fn set_pathlock(&mut self, level: u32, row: i32, col: i32, pathlock: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, PATHLOCK_BASE.wrapping_shl(level), pathlock)
        });
    }

    /// `MissionMap::getMine` (`MINE_NONE` 0, `MINE_INTACT` 1, `MINE_EXPLODED` 2; 4 bits).
    pub fn get_mine(&self, row: i32, col: i32) -> u32 {
        (self.data_at(row, col) & bits::MAPCELL_MINE_MASK) >> MINE_SHIFT
    }
    /// `MissionMap::setMine(row, col, mine)`: the value is shifted in without masking, as in the C++.
    pub fn set_mine(&mut self, row: i32, col: i32, mine: u32) {
        self.with_cell_mut(row, col, |c| {
            put(&mut c.0, bits::MAPCELL_MINE_MASK, mine << MINE_SHIFT)
        });
    }

    /// `MissionMap::getPreserved`.
    pub fn get_preserved(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_PRESERVED_MASK != 0
    }
    /// `MissionMap::setPreserved`.
    pub fn set_preserved(&mut self, row: i32, col: i32, preserved: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_PRESERVED_MASK, preserved)
        });
    }

    /// `MissionMap::getLocalHeight` (4 bits).
    pub fn get_local_height(&self, row: i32, col: i32) -> u32 {
        (self.data_at(row, col) & bits::MAPCELL_HEIGHT_MASK) >> HEIGHT_SHIFT
    }
    /// `MissionMap::setLocalHeight`.
    pub fn set_local_height(&mut self, row: i32, col: i32, local_elevation: u32) {
        self.with_cell_mut(row, col, |c| {
            put(
                &mut c.0,
                bits::MAPCELL_HEIGHT_MASK,
                local_elevation << HEIGHT_SHIFT,
            )
        });
    }

    /// `MissionMap::getWall`.
    pub fn get_wall(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_WALL_MASK != 0
    }
    /// `MissionMap::setWall`.
    pub fn set_wall(&mut self, row: i32, col: i32, wall_here: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_WALL_MASK, wall_here)
        });
    }
    /// `MissionMap::getRoad`.
    pub fn get_road(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_ROAD_MASK != 0
    }
    /// `MissionMap::setRoad`.
    pub fn set_road(&mut self, row: i32, col: i32, road_here: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_ROAD_MASK, road_here)
        });
    }
    /// `MissionMap::getShallowWater`.
    pub fn get_shallow_water(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_SHALLOW_MASK != 0
    }
    /// `MissionMap::setShallowWater`.
    pub fn set_shallow_water(&mut self, row: i32, col: i32, on: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_SHALLOW_MASK, on)
        });
    }
    /// `MissionMap::getDeepWater`.
    pub fn get_deep_water(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_DEEP_MASK != 0
    }
    /// `MissionMap::setDeepWater`.
    pub fn set_deep_water(&mut self, row: i32, col: i32, on: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_DEEP_MASK, on)
        });
    }
    /// `MissionMap::getForest` (the forest-cell flag the path costs read; `forestMoveCost`).
    pub fn get_forest(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_FOREST_MASK != 0
    }
    /// `MissionMap::setForest`.
    pub fn set_forest(&mut self, row: i32, col: i32, set: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_FOREST_MASK, set)
        });
    }
    /// `MissionMap::getOffMap`.
    pub fn get_off_map(&self, row: i32, col: i32) -> bool {
        self.data_at(row, col) & bits::MAPCELL_OFFMAP_MASK != 0
    }
    /// `MissionMap::setOffMap`.
    pub fn set_off_map(&mut self, row: i32, col: i32, set: bool) {
        self.with_cell_mut(row, col, |c| {
            put_flag(&mut c.0, bits::MAPCELL_OFFMAP_MASK, set)
        });
    }

    /// `MissionMap::spreadState` (move.cpp:1017): grow a box of impassable, preserved cells around a cell, depth first.
    pub fn spread_state(&mut self, cell_row: i32, cell_col: i32, radius: i32) {
        if self.in_bounds(cell_row, cell_col) && radius > 0 {
            // If we're placing moving objects for this frame only, preserve the cell state of this tile.
            if !self.get_preserved(cell_row, cell_col) {
                self.set_preserved(cell_row, cell_col, true);
            }
            self.set_passable(cell_row, cell_col, false);
            for dir in 0..8 {
                let adj_r = cell_row + ROW_SHIFT[dir];
                let adj_c = cell_col + COL_SHIFT[dir];
                self.spread_state(adj_r, adj_c, radius - 1);
            }
        }
    }

    /// `MissionMap::placeObject(position, radius)` (move.cpp:1040): block the cells an object of `radius` (metres) covers.
    pub fn place_object(&mut self, position: &Vec3, radius: f32) {
        let (cell_row, cell_col) = self.world_to_cell(position);
        // cellRadius = radius / (worldUnitsPerCell * metersPerWorldUnit)
        let mut cell_radius = radius / (WORLD_UNITS_PER_CELL * METERS_PER_WORLD_UNIT);
        if cell_radius > 0.5 && cell_radius < 1.0 {
            cell_radius = 1.0;
        }
        self.spread_state(cell_row, cell_col, cell_radius as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terrain() -> Terrain {
        Terrain::from_elevations(20, &[0.0; 400])
    }

    #[test]
    fn setters_touch_only_their_bits() {
        let mut t = terrain();
        t.set_passable(5, 6, true);
        assert!(t.get_passable(5, 6));
        t.set_mover(5, 6, true);
        t.set_gate(5, 6, 1);
        t.set_pathlock(0, 5, 6, true);
        t.set_pathlock(1, 5, 6, true);
        t.set_mine(5, 6, 2);
        t.set_forest(5, 6, true);
        t.set_local_height(5, 6, 9);
        assert_eq!(
            t.cell(5, 6).unwrap().0,
            0x400 | 0x40 | 0x100 | 0x800 | 0x1000 | (2 << 13) | 0x1000_0000 | (9 << 18)
        );
        assert!(
            t.get_mover(5, 6)
                && t.get_pathlock(0, 5, 6)
                && t.get_pathlock(1, 5, 6)
                && t.get_forest(5, 6)
        );
        assert_eq!(
            (t.get_gate(5, 6), t.get_mine(5, 6), t.get_local_height(5, 6)),
            (1, 2, 9)
        );
        t.set_pathlock(0, 5, 6, false);
        t.set_mover(5, 6, false);
        t.set_mine(5, 6, 0);
        assert!(!t.get_pathlock(0, 5, 6) && t.get_pathlock(1, 5, 6) && !t.get_mover(5, 6));
        assert_eq!(t.get_mine(5, 6), 0);
        assert!(t.get_passable(5, 6), "other bits kept");
        // neighbours untouched
        assert_eq!(t.cell(5, 7).unwrap().0, 0);
    }

    #[test]
    fn out_of_range_reads_are_zero_and_writes_ignored() {
        let mut t = terrain();
        t.set_passable(-1, 0, true);
        t.set_mover(0, 60, true);
        t.set_mine(99, 99, 3);
        assert!(!t.get_passable(-1, 0) && !t.get_mover(0, 60));
        assert_eq!(t.get_mine(99, 99), 0);
        assert_eq!(t.cell(0, 60), None);
    }

    #[test]
    fn place_object_blocks_a_box_and_preserves() {
        let mut t = terrain();
        for r in 0..60 {
            for c in 0..60 {
                t.set_passable(r, c, true);
            }
        }
        let p = t.cell_to_world(30, 30);
        // radius 17 m: 17 / (42.67 * 0.2) = 1.99 -> 1 cell: only the centre (radius 1 stops after the first cell)
        t.place_object(&p, 17.0);
        assert!(!t.get_passable(30, 30) && t.get_preserved(30, 30));
        assert!(t.get_passable(30, 31));
        // radius 25 m = 2.93 -> 2 cells: centre and its 8 neighbours
        t.place_object(&p, 25.0);
        for (dr, dc) in [
            (-1, -1),
            (-1, 0),
            (-1, 1),
            (0, -1),
            (0, 1),
            (1, -1),
            (1, 0),
            (1, 1),
        ] {
            assert!(!t.get_passable(30 + dr, 30 + dc));
        }
        assert!(t.get_passable(30, 32) && t.get_passable(28, 30));
    }
}
