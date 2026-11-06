// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Per-cell mission map. Port of `MissionMap` / `MapCell` (move.h ~191-507, move.cpp ~790-990).
//!
//! Packet layout (mission `.pak`, first move packet index `first`):
//! - `first + 0`: height (i32)
//! - `first + 1`: width (i32)
//! - `first + 2`: planet (i32)
//! - `first + 3`: `width * height` cells, each a little-endian u32 `MapCell::data`.

use crate::packets::{get_exact, i32_at, LoadError, PacketSource};

/// Number of packets a mission map occupies (`NUM_MISSIONMAP_PACKETS`).
pub const MISSION_MAP_PACKETS: usize = 4;

pub const MAPCELL_TERRAIN_MASK: u32 = 0x0000_000F;
pub const MAPCELL_OVERLAY_MASK: u32 = 0x0000_0030;
pub const MAPCELL_MOVER_MASK: u32 = 0x0000_0040;
pub const MAPCELL_GATE_MASK: u32 = 0x0000_0100;
pub const MAPCELL_OFFMAP_MASK: u32 = 0x0000_0200;
pub const MAPCELL_PASSABLE_MASK: u32 = 0x0000_0400;
/// Pathlock bits: level 0 is bit 11, level 1 is bit 12.
pub const MAPCELL_PATHLOCK_BASE: u32 = 0x0000_0800;
pub const MAPCELL_MINE_MASK: u32 = 0x0001_E000;
pub const MAPCELL_PRESERVED_MASK: u32 = 0x0002_0000;
pub const MAPCELL_HEIGHT_MASK: u32 = 0x003C_0000;
pub const MAPCELL_WALL_MASK: u32 = 0x0100_0000;
pub const MAPCELL_ROAD_MASK: u32 = 0x0200_0000;
pub const MAPCELL_SHALLOW_MASK: u32 = 0x0400_0000;
pub const MAPCELL_DEEP_MASK: u32 = 0x0800_0000;
pub const MAPCELL_FOREST_MASK: u32 = 0x1000_0000;

/// One map cell, bit-packed exactly as `MapCell` (`#pragma pack(1)`, 4 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MapCell(pub u32);

impl MapCell {
    pub fn terrain(self) -> u32 {
        self.0 & MAPCELL_TERRAIN_MASK
    }
    pub fn overlay(self) -> u32 {
        (self.0 & MAPCELL_OVERLAY_MASK) >> 4
    }
    pub fn mover(self) -> bool {
        self.0 & MAPCELL_MOVER_MASK != 0
    }
    /// Gate value (0 or 1) as `getGate()`.
    pub fn gate(self) -> u32 {
        (self.0 & MAPCELL_GATE_MASK) >> 8
    }
    pub fn off_map(self) -> bool {
        self.0 & MAPCELL_OFFMAP_MASK != 0
    }
    pub fn passable(self) -> bool {
        self.0 & MAPCELL_PASSABLE_MASK != 0
    }
    /// `getPathlock(level)`: `level` is the C++ DWORD (0 or 1).
    pub fn pathlock(self, level: u32) -> bool {
        self.0 & (MAPCELL_PATHLOCK_BASE << level) != 0
    }
    pub fn mine(self) -> u32 {
        (self.0 & MAPCELL_MINE_MASK) >> 13
    }
    pub fn preserved(self) -> bool {
        self.0 & MAPCELL_PRESERVED_MASK != 0
    }
    pub fn local_height(self) -> u32 {
        (self.0 & MAPCELL_HEIGHT_MASK) >> 18
    }
    pub fn wall(self) -> bool {
        self.0 & MAPCELL_WALL_MASK != 0
    }
    pub fn road(self) -> bool {
        self.0 & MAPCELL_ROAD_MASK != 0
    }
    pub fn shallow_water(self) -> bool {
        self.0 & MAPCELL_SHALLOW_MASK != 0
    }
    pub fn deep_water(self) -> bool {
        self.0 & MAPCELL_DEEP_MASK != 0
    }
    pub fn forest(self) -> bool {
        self.0 & MAPCELL_FOREST_MASK != 0
    }

    fn set_bit(&mut self, mask: u32, on: bool) {
        if on {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
    }

    pub fn set_passable(&mut self, on: bool) {
        self.set_bit(MAPCELL_PASSABLE_MASK, on);
    }
    pub fn set_off_map(&mut self, on: bool) {
        self.set_bit(MAPCELL_OFFMAP_MASK, on);
    }
    pub fn set_gate(&mut self, on: bool) {
        self.set_bit(MAPCELL_GATE_MASK, on);
    }
    pub fn set_road(&mut self, on: bool) {
        self.set_bit(MAPCELL_ROAD_MASK, on);
    }
    pub fn set_forest(&mut self, on: bool) {
        self.set_bit(MAPCELL_FOREST_MASK, on);
    }
    pub fn set_shallow_water(&mut self, on: bool) {
        self.set_bit(MAPCELL_SHALLOW_MASK, on);
    }
    pub fn set_deep_water(&mut self, on: bool) {
        self.set_bit(MAPCELL_DEEP_MASK, on);
    }
    pub fn set_mover(&mut self, on: bool) {
        self.set_bit(MAPCELL_MOVER_MASK, on);
    }

    /// `MapCell::setPathlock(level,pathlock)`.
    pub fn set_pathlock(&mut self, level: u32, on: bool) {
        if level < 2 {
            self.set_bit(MAPCELL_PATHLOCK_BASE << level, on);
        }
    }
}

/// Cell grid of one mission. Cells are stored row-major with stride `width`.
#[derive(Clone, Debug)]
pub struct MissionMap {
    width: i32,
    height: i32,
    planet: i32,
    cells: Vec<MapCell>,
}

impl MissionMap {
    /// Build from raw cells (tests and tools). `cells.len()` must equal `width * height`.
    pub fn from_cells(width: i32, height: i32, cells: Vec<MapCell>) -> Result<Self, LoadError> {
        if width < 0 || height < 0 || cells.len() != (width as usize) * (height as usize) {
            return Err(LoadError::BadValue {
                what: "mission map cell count",
                value: cells.len() as i64,
            });
        }
        Ok(MissionMap {
            width,
            height,
            planet: 0,
            cells,
        })
    }

    /// Read the four mission-map packets starting at `first`. Clears the mover bit in
    /// every cell, as `MissionMap::init(PacketFile*)` does.
    pub fn from_packets(src: &dyn PacketSource, first: usize) -> Result<Self, LoadError> {
        let h = get_exact(src, first, 4, "mission height")?;
        let height = i32_at(h, 0);
        let w = get_exact(src, first + 1, 4, "mission width")?;
        let width = i32_at(w, 0);
        if width < 0 || height < 0 {
            return Err(LoadError::BadValue {
                what: "mission dimensions",
                value: width.min(height) as i64,
            });
        }
        let p = get_exact(src, first + 2, 4, "mission planet")?;
        let planet = i32_at(p, 0);
        let count = width as usize * height as usize;
        let data = get_exact(src, first + 3, count * 4, "mission cells")?;
        let cells = data
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| MapCell(u32::from_le_bytes(*c)))
            .map(|mut c| {
                c.set_mover(false);
                c
            });
        Ok(MissionMap {
            width,
            height,
            planet,
            cells: cells.collect(),
        })
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    pub fn planet(&self) -> i32 {
        self.planet
    }

    pub fn in_bounds(&self, row: i32, col: i32) -> bool {
        row >= 0 && row < self.height && col >= 0 && col < self.width
    }

    /// Cell at (row, col). Panics when out of bounds (use `in_bounds` first).
    pub fn cell(&self, row: i32, col: i32) -> MapCell {
        self.cells[(row * self.width + col) as usize]
    }

    pub fn cell_mut(&mut self, row: i32, col: i32) -> &mut MapCell {
        &mut self.cells[(row * self.width + col) as usize]
    }

    /// `MissionMap::setPathlock(level,row,col,pathlock)`. Invalid coordinates and levels are ignored, as in
    /// the sim's pinned no-op handling for out-of-range map access.
    pub fn set_pathlock(&mut self, level: u32, row: i32, col: i32, on: bool) {
        if level < 2 && self.in_bounds(row, col) {
            self.cell_mut(row, col).set_pathlock(level, on);
        }
    }

    /// Ground passability as the path code sees it: passable and not off-map.
    pub fn is_open_ground(&self, row: i32, col: i32) -> bool {
        if !self.in_bounds(row, col) {
            return false;
        }
        let c = self.cell(row, col);
        c.passable() && !c.off_map()
    }
}
