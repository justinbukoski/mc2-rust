// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE T - terrain and map queries (the simulation's read-only view of the ground).
//!
//! Port of the query side of `mclib/terrain.{h,cpp}` and `mclib/mapdata.{h,cpp}` (`Terrain::getTerrainElevation`,
//! `getTerrainNormal`, `getTerrainAngle`, `worldToTile/Cell/TileCell`, `cellToWorld`, `getCellPos`, `getWater`,
//! `IsGameSelectTerrainPosition`) plus the per-cell map data owned by `mc2-path` ([`mc2_path::MissionMap`], inside
//! [`mc2_path::MoveData`]) with the cell bit setters every lane uses (mover, path lock, gate, mine, forest, ...).
//! Nothing here renders. Rendering-only parts (texture manager, quad generation, haze, lighting, `geometry()`) are not
//! ported; `Terrain::geometry()`'s one simulation effect (marking object blocks and vertices active from the camera) is the
//! caller's [`Visibility`](crate::visibility::Visibility) input.
//!
//! Layout: [`mesh`] holds the checkerboard triangle maths (elevation, normal, angle); [`cells`] the `MapCell` bit setters
//! the C++ `MissionMap` offers and `placeObject`; [`dump`] the oracle `--dump-terrain` text format.
//!
//! Gotchas (DESIGN.md "Lane T"): `worldUnitsPerMeter = 5.01f` but `metersPerWorldUnit = 0.2f`; `worldToCell` truncates
//! toward zero; elevation/normal/angle use the checkerboard triangle split keyed by vertex parity (`mapdata.cpp:1584`,
//! `1383`, `1070`); `tileCellToWorld` leaves its output untouched when out of range while `cellToWorld` zeroes it.
//!
//! ## Out-of-range behaviour (DESIGN.md section 10, item 6)
//!
//! The C++ does no bounds checking on `MissionMap` accessors and reads whatever lies beyond. The port returns the zero
//! value for out-of-range reads (`false`, `0`) and ignores out-of-range writes; the checked C++ paths (`cellToWorld`,
//! `worldToTileCell`, the elevation range checks) behave exactly as in the source. A float that does not fit an `int`
//! in `worldToCell` is undefined behaviour in C++ (x86 yields `INT_MIN`); Rust's saturating `as` conversion is used.

pub mod cells;
pub mod dump;
pub mod mesh;

use crate::math::Vec3;
use mc2_assets::map::{MissionPacket, TerrainVertex};
use mc2_path::{MapCell, MissionMap, MoveData};

pub use mc2_path::geometry::{
    MAPCELL_DIM, METERS_PER_WORLD_UNIT, WORLD_UNITS_PER_CELL, WORLD_UNITS_PER_VERTEX,
};

/// `worldUnitsPerMeter` (terrain.cpp) - note: NOT `1 / METERS_PER_WORLD_UNIT`.
pub const WORLD_UNITS_PER_METER: f32 = 5.01;
/// `Terrain::verticesBlockSide`.
pub const VERTICES_BLOCK_SIDE: i32 = 20;
/// `MAX_MAP_CELL_WIDTH` (terrain.h).
pub const MAX_MAP_CELL_WIDTH: i32 = 720;
/// `Terrain::halfWorldUnitsPerCell`.
pub const HALF_WORLD_UNITS_PER_CELL: f32 = WORLD_UNITS_PER_CELL / 2.0;
/// `Terrain::metersPerCell`.
pub const METERS_PER_CELL: f32 = WORLD_UNITS_PER_CELL * METERS_PER_WORLD_UNIT;
/// `Terrain::worldUnitsBlockSide`.
pub const WORLD_UNITS_BLOCK_SIDE: f32 = WORLD_UNITS_PER_VERTEX * VERTICES_BLOCK_SIDE as f32;
/// `Terrain::oneOverWorldUnitsPerVertex`.
pub const ONE_OVER_WORLD_UNITS_PER_VERTEX: f32 = 1.0 / WORLD_UNITS_PER_VERTEX;
/// `Terrain::oneOverWorldUnitsPerCell`.
pub const ONE_OVER_WORLD_UNITS_PER_CELL: f32 = 1.0 / WORLD_UNITS_PER_CELL;
/// `Terrain::oneOverMetersPerCell`.
pub const ONE_OVER_METERS_PER_CELL: f32 = 1.0 / METERS_PER_CELL;
/// `Terrain::oneOverVerticesBlockSide`.
pub const ONE_OVER_VERTICES_BLOCK_SIDE: f32 = 1.0 / VERTICES_BLOCK_SIDE as f32;

/// `ObjBlockInfo` (terrain.h): per 20x20-vertex block bookkeeping the object manager fills in
/// (`countTerrainObjects`) and the camera marks active (`setObjBlockActive`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjBlockInfo {
    /// Camera-derived "block is on screen" flag; comes from [`Visibility`](crate::visibility::Visibility).
    pub active: bool,
    /// `numCollidableObjects`.
    pub num_collidable_objects: i32,
    /// `numObjects` (includes the collidable ones).
    pub num_objects: i32,
    /// `firstHandle` (collidables, followed by the rest).
    pub first_handle: i32,
}

/// Loaded ground data of one mission.
///
/// Not `Clone`: [`MoveData`] (owned by lane B's path queries through [`Terrain::move_data_mut`]) is not.
#[derive(Debug)]
pub struct Terrain {
    /// Vertices per map side (`realVerticesMapSide`).
    pub vertices_per_side: i32,
    /// Object block grid side (`blocksMapSide`).
    pub blocks_per_side: i32,
    /// `halfVerticesMapSide`.
    pub half_vertices_per_side: i32,
    /// `worldUnitsMapSide`: `realVerticesMapSide * 128.0f`.
    pub world_units_map_side: f32,
    /// `oneOverWorldUnitsMapSide`.
    pub one_over_world_units_map_side: f32,
    /// `mapTopLeft3d`: `(-side/2, +side/2, elevation of vertex 0)`.
    pub map_top_left: Vec3,
    /// `Terrain::waterElevation` (set from `[Water] Elevation` by `Terrain::load(FitIniFile)`).
    pub water_elevation: f32,
    /// `MapData::waterDepth`.
    pub water_depth_const: f32,
    /// `Terrain::userMin` (loaded from the mission's `[Terrain] UserMin`).
    pub user_min: f32,
    /// `Terrain::userMax` (loaded from the mission's `[Terrain] UserMax`).
    pub user_max: f32,
    /// `MapData::shallowDepth`.
    pub shallow_depth: f32,
    /// `MapData::alphaDepth`.
    pub alpha_depth: f32,
    /// `Terrain::waterFreq` (default 4.0).
    pub water_freq: f32,
    /// `Terrain::waterAmplitude` (default 10.0).
    pub water_amplitude: f32,
    /// `Terrain::objBlockInfo[numObjBlocks]`.
    pub obj_blocks: Vec<ObjBlockInfo>,
    vertices: Vec<TerrainVertex>,
    tile_row_to_world: Vec<f32>,
    tile_col_to_world: Vec<f32>,
    cell_to_world_coord: [f32; MAPCELL_DIM as usize],
    cell_row_to_world: Vec<f32>,
    cell_col_to_world: Vec<f32>,
    move_data: MoveData,
}

impl Default for Terrain {
    fn default() -> Terrain {
        Terrain::from_vertices(0, Vec::new(), None)
    }
}

fn blank_vertex(elevation: f32) -> TerrainVertex {
    TerrainVertex {
        normal: [0.0, 0.0, 1.0],
        elevation,
        texture_data: 0xffff_0000,
        local_rgb_light: 0,
        terrain_type: 2, // MC_MUD_TYPE, the value MapData::newInit gives every vertex before the packet overwrites it
        selected: 0,
        water: 0,
        shadow: 0,
        highlighted: 0,
    }
}

/// An empty movement data set (no cells, no areas), for terrains built without mission packets.
fn empty_move_data(cell_side: i32) -> MoveData {
    use mc2_path::{GlobalMap, GlobalMapParts};
    let global = || {
        GlobalMap::from_parts(GlobalMapParts {
            width: 0,
            height: 0,
            area_map: Vec::new(),
            areas: Vec::new(),
            doors: Vec::new(),
            links: Vec::new(),
        })
        .expect("empty global map")
    };
    let n = (cell_side.max(0) as usize) * (cell_side.max(0) as usize);
    MoveData {
        mission: MissionMap::from_cells(cell_side.max(0), cell_side.max(0), vec![MapCell(0); n])
            .expect("blank mission map"),
        global: [global(), global(), global()],
        special_areas: Vec::new(),
        packets_used: 0,
    }
}

/// `float` -> `int` the way a C cast does (truncate toward zero).
#[inline]
pub(crate) fn f2i(v: f32) -> i32 {
    v as i32
}

impl Terrain {
    // ------------------------------------------------------------------------------------------------------------
    // Construction (Terrain::init, Terrain::initMapCellArrays, MapData::newInit)
    // ------------------------------------------------------------------------------------------------------------

    /// Build a terrain from decoded vertices (`side * side` of them, row-major) and optionally the movement data
    /// (`MOVE_readData`). Without movement data a blank, all-impassable cell map of `side * 3` cells is created.
    ///
    /// `Terrain::init(verticesPerMapSide, ...)`, terrain.cpp:358-523, and `initMapCellArrays`, terrain.cpp:229-280.
    pub fn from_vertices(
        side: i32,
        vertices: Vec<TerrainVertex>,
        move_data: Option<MoveData>,
    ) -> Terrain {
        assert_eq!(
            vertices.len(),
            (side.max(0) as usize) * (side.max(0) as usize),
            "vertex count"
        );
        let real = side;
        let half = real >> 1;
        let blocks = real / VERTICES_BLOCK_SIDE;
        // worldUnitsMapSide = realVerticesMapSide * worldUnitsPerVertex  (long * float -> float)
        let wums = (real as f32) * WORLD_UNITS_PER_VERTEX;
        // `if (worldUnitsMapSide > Stuff::SMALL)`, SMALL = 1.0e-4
        let one_over = if wums > 1.0e-4 { 1.0f32 / wums } else { 0.0 };
        // mapTopLeft3d.x = -worldUnitsMapSide / 2.0f ; .y = worldUnitsMapSide / 2.0f ; .z = getTopLeftElevation()
        let top_left = Vec3 {
            x: -wums / 2.0,
            y: wums / 2.0,
            z: vertices.first().map_or(0.0, |v| v.elevation),
        };
        // initMapCellArrays: the C++ mixes float, long and double exactly as written below.
        let wums_d = f64::from(wums);
        let tile_row_to_world = (0..real)
            .map(|i| (wums_d / 2.0 - f64::from((i as f32) * WORLD_UNITS_PER_VERTEX)) as f32)
            .collect();
        let tile_col_to_world = (0..real)
            .map(|i| (f64::from((i as f32) * WORLD_UNITS_PER_VERTEX) - wums_d / 2.0) as f32)
            .collect();
        let mut cell_to_world_coord = [0.0f32; MAPCELL_DIM as usize];
        for (i, c) in cell_to_world_coord.iter_mut().enumerate() {
            *c = (WORLD_UNITS_PER_VERTEX / (MAPCELL_DIM as f32)) * (i as f32);
        }
        let cell_row_to_world = (0..real * MAPCELL_DIM)
            .map(|i| (wums_d / 2.0 - f64::from((i as f32) * WORLD_UNITS_PER_CELL)) as f32)
            .collect();
        let cell_col_to_world = (0..real * MAPCELL_DIM)
            .map(|i| (f64::from((i as f32) * WORLD_UNITS_PER_CELL) - wums_d / 2.0) as f32)
            .collect();
        Terrain {
            vertices_per_side: real,
            blocks_per_side: blocks,
            half_vertices_per_side: half,
            world_units_map_side: wums,
            one_over_world_units_map_side: one_over,
            map_top_left: top_left,
            water_elevation: 0.0,
            water_depth_const: 0.0,
            user_min: 0.0,
            user_max: 0.0,
            shallow_depth: 0.0,
            alpha_depth: 0.0,
            water_freq: 4.0,
            water_amplitude: 10.0,
            obj_blocks: vec![
                ObjBlockInfo::default();
                (blocks.max(0) as usize) * (blocks.max(0) as usize)
            ],
            vertices,
            tile_row_to_world,
            tile_col_to_world,
            cell_to_world_coord,
            cell_row_to_world,
            cell_col_to_world,
            move_data: move_data.unwrap_or_else(|| empty_move_data(real * MAPCELL_DIM)),
        }
    }

    /// A flat-structured test terrain: `side * side` elevations, blank vertices otherwise, blank cell map.
    pub fn from_elevations(side: i32, elevations: &[f32]) -> Terrain {
        let vertices = elevations.iter().map(|&e| blank_vertex(e)).collect();
        Terrain::from_vertices(side, vertices, None)
    }

    /// Terrain and movement data from the packets of a mission `.pak`: packet 0 is the vertex grid (`Terrain::init`),
    /// packets 4.. are the movement data (`MOVE_readData(&pakFile, 4)`, mission.cpp:2219). The movement data is
    /// optional only here; the C++ stops with `STOP` when packet 4 is empty.
    pub fn from_packets(src: &dyn mc2_path::PacketSource) -> Result<Terrain, String> {
        let terrain_bytes = src.packet(0).ok_or("mission pak has no terrain packet 0")?;
        let (side, vertices) =
            match mc2_assets::map::decode_packet(0, terrain_bytes).map_err(|e| e.to_string())? {
                MissionPacket::Terrain { side, vertices } => (side as i32, vertices),
                _ => return Err("packet 0 is not terrain".into()),
            };
        let md = match src.packet(4) {
            Some(b) if !b.is_empty() => Some(MoveData::load(src, 4).map_err(|e| e.to_string())?),
            _ => {
                return Err("Mission has no movement data (packet 4 empty): QuickSaved map?".into())
            }
        };
        if let Some(m) = &md {
            let cells = m.mission.width();
            if cells != side * MAPCELL_DIM || m.mission.height() != cells {
                return Err(format!(
                    "movement map is {}x{} cells but terrain is {side} vertices",
                    m.mission.width(),
                    m.mission.height()
                ));
            }
        }
        Ok(Terrain::from_vertices(side, vertices, md))
    }

    /// Load a mission's terrain from the virtual file system: `data/missions/<name>.pak` (packets as above) and the
    /// `[Water]` block of `data/missions/<name>.fit` (`Terrain::load(FitIniFile*)`, terrain.cpp:1343-1377).
    pub fn load_mission(vfs: &mut mc2_formats::vfs::Vfs, mission: &str) -> Result<Terrain, String> {
        let pak_name = format!("data/missions/{mission}.pak");
        let bytes = vfs
            .read(&pak_name)
            .map_err(|e| format!("{pak_name}: {e}"))?
            .ok_or_else(|| format!("{pak_name}: not found"))?;
        let pak =
            mc2_formats::pak::PakFile::from_bytes(bytes).map_err(|e| format!("{pak_name}: {e}"))?;
        // Decode terrain (0) and movement (4..) packets; 1..3 (objects, tac-map) belong to other lanes.
        let mut packets: Vec<Vec<u8>> = vec![Vec::new(); pak.packet_count()];
        for (i, slot) in packets.iter_mut().enumerate() {
            if i == 0 || i >= 4 {
                *slot = pak
                    .read_packet(i)
                    .map_err(|e| format!("{pak_name} packet {i}: {e}"))?;
            }
        }
        let mut t = Terrain::from_packets(&packets).map_err(|e| format!("{pak_name}: {e}"))?;
        let fit_name = format!("data/missions/{mission}.fit");
        if let Some(mut fit) = vfs
            .read_fit(&fit_name)
            .map_err(|e| format!("{fit_name}: {e}"))?
        {
            t.load_water(&mut fit);
            t.load_user_range(&mut fit);
        }
        Ok(t)
    }

    /// `Terrain::load(FitIniFile*)` water part (terrain.cpp:1346-1358). The C++ asserts that every key exists; asserts are
    /// compiled out, a missing key leaves the previous value (0 / the defaults on a fresh world).
    pub fn load_water(&mut self, fit: &mut mc2_formats::ini::FitIni) {
        if fit.seek_block("Water").is_err() {
            return;
        }
        if let Ok(v) = fit.read_f32("Elevation") {
            self.water_depth_const = v;
            self.water_elevation = v;
        }
        if let Ok(v) = fit.read_f32("Frequency") {
            self.water_freq = v;
        }
        if let Ok(v) = fit.read_f32("Ampliture") {
            self.water_amplitude = v;
        }
        if let Ok(v) = fit.read_f32("AlphaDepth") {
            self.alpha_depth = v;
        }
        if let Ok(v) = fit.read_f32("ShallowDepth") {
            self.shallow_depth = v;
        }
    }

    /// `Terrain::load(FitIniFile*)` user altitude range used by visual range selection
    /// (`terrain.cpp:1365-1369`, `gameobj.cpp:1864-1882`).
    pub fn load_user_range(&mut self, fit: &mut mc2_formats::ini::FitIni) {
        if fit.seek_block("Terrain").is_err() {
            return;
        }
        if let Ok(v) = fit.read_i32("UserMin") {
            self.user_min = v as f32;
        }
        if let Ok(v) = fit.read_i32("UserMax") {
            self.user_max = v as f32;
        }
    }

    /// Load the terrain of the mission being initialised (`Mission::init`, mission.cpp:2192-2240).
    pub fn load(ctx: &mut crate::mission::LoadCtx<'_>) -> Result<Terrain, String> {
        let name = ctx.spec.mission.clone();
        Terrain::load_mission(ctx.vfs, &name)
    }

    // ------------------------------------------------------------------------------------------------------------
    // Vertex data
    // ------------------------------------------------------------------------------------------------------------

    /// All vertices, row-major (`MapData::blocks`).
    pub fn vertices(&self) -> &[TerrainVertex] {
        &self.vertices
    }

    /// Vertex `(tile_r, tile_c)`, `None` outside the grid.
    pub fn vertex(&self, tile_r: i32, tile_c: i32) -> Option<&TerrainVertex> {
        if tile_r < 0
            || tile_c < 0
            || tile_r >= self.vertices_per_side
            || tile_c >= self.vertices_per_side
        {
            return None;
        }
        self.vertices
            .get((tile_c + tile_r * self.vertices_per_side) as usize)
    }

    /// `Terrain::getTerrainElevation(tileR, tileC)`: the raw vertex height (0 outside the grid).
    pub fn vertex_elevation(&self, tile_r: i32, tile_c: i32) -> f32 {
        self.vertex(tile_r, tile_c).map_or(0.0, |v| v.elevation)
    }

    /// `Terrain::getTerrain(tileR, tileC)`: the vertex terrain type (`TerrainType`, dmapdata.h).
    pub fn terrain_type(&self, tile_r: i32, tile_c: i32) -> u32 {
        self.vertex(tile_r, tile_c).map_or(0, |v| v.terrain_type)
    }

    /// `Terrain::getVertexHeight(index)`: -1 outside `0..side*side`.
    pub fn vertex_height(&self, index: i32) -> f32 {
        if index > -1 && index < self.vertices_per_side * self.vertices_per_side {
            self.vertices[index as usize].elevation
        } else {
            -1.0
        }
    }

    /// `Terrain::setVertexHeight(index, value)`; ignored outside the grid.
    pub fn set_vertex_height(&mut self, index: i32, value: f32) {
        if index > -1 && index < self.vertices_per_side * self.vertices_per_side {
            self.vertices[index as usize].elevation = value;
        }
    }

    /// `MapData::getOverlayTile`-style raw overlay texture handle of a vertex (`textureData >> 16`; `0xffff` = none).
    /// Mapping a handle to the `Overlays` enum needs the terrain texture manager (rendering), so only the raw handle is
    /// kept. The movement-relevant overlay (road, wall, gate, bridge) lives in the cell data: see [`Terrain::cell`].
    pub fn overlay_handle(&self, tile_r: i32, tile_c: i32) -> u32 {
        self.vertex(tile_r, tile_c)
            .map_or(0xffff, |v| v.texture_data >> 16)
    }

    // ------------------------------------------------------------------------------------------------------------
    // Coordinate conversion (terrain.h:324-398)
    // ------------------------------------------------------------------------------------------------------------

    /// `worldToTile`: `(tileR, tileC)` truncated toward zero, not range checked.
    pub fn world_to_tile_rc(&self, p: &Vec3) -> (i32, i32) {
        let tmp_x = p.x - self.map_top_left.x;
        let tmp_y = self.map_top_left.y - p.y;
        let tile_c = f2i(tmp_x * ONE_OVER_WORLD_UNITS_PER_VERTEX);
        let tile_r = f2i(tmp_y * ONE_OVER_WORLD_UNITS_PER_VERTEX);
        (tile_r, tile_c)
    }

    /// `worldToTile` as `(tileR, tileC, vertexNumber)` where the vertex number is `tileR * realVerticesMapSide + tileC`
    /// (the `Terrain::objVertexActive` / vertex array index; callers must range check it, as `setObjVertexActive` does).
    pub fn world_to_tile(&self, p: &Vec3) -> (i32, i32, i32) {
        let (r, c) = self.world_to_tile_rc(p);
        (r, c, r.wrapping_mul(self.vertices_per_side).wrapping_add(c))
    }

    /// `worldToCell`: `(cellR, cellC)` with C truncation toward zero (so -0.9 -> 0, not -1). Not range checked.
    pub fn world_to_cell(&self, p: &Vec3) -> (i32, i32) {
        let k = ONE_OVER_WORLD_UNITS_PER_VERTEX * 3.0f32;
        let cell_c = f2i((p.x - self.map_top_left.x) * k);
        let cell_r = f2i((self.map_top_left.y - p.y) * k);
        (cell_r, cell_c)
    }

    /// `worldToTileCell`: `(tileR, tileC, cellR, cellC)`. A position outside the map resets the tile to (0,0) and the
    /// cell is then computed against tile 0 (so it can be far outside `0..3`), as in the source.
    pub fn world_to_tile_cell(&self, p: &Vec3) -> (i32, i32, i32, i32) {
        let tmp_x = p.x - self.map_top_left.x;
        let tmp_y = self.map_top_left.y - p.y;
        let mut tile_c = f2i(tmp_x * ONE_OVER_WORLD_UNITS_PER_VERTEX);
        let mut tile_r = f2i(tmp_y * ONE_OVER_WORLD_UNITS_PER_VERTEX);
        if tile_c < 0
            || tile_r < 0
            || tile_c >= self.vertices_per_side
            || tile_r >= self.vertices_per_side
        {
            tile_c = 0;
            tile_r = 0;
        }
        if self.tile_col_to_world.is_empty() {
            return (tile_r, tile_c, 0, 0); // no map loaded: the C++ tables do not exist either
        }
        let cell_c =
            f2i((p.x - self.tile_col_to_world[tile_c as usize]) * ONE_OVER_WORLD_UNITS_PER_CELL);
        let cell_r =
            f2i((self.tile_row_to_world[tile_r as usize] - p.y) * ONE_OVER_WORLD_UNITS_PER_CELL);
        (tile_r, tile_c, cell_r, cell_c)
    }

    /// `tileCellToWorld`. `None` when the tile or cell is out of range: the C++ then leaves the caller's output
    /// vector unchanged (it only zeroes its own by-value copies of the indices).
    pub fn tile_cell_to_world(
        &self,
        tile_r: i32,
        tile_c: i32,
        cell_r: i32,
        cell_c: i32,
    ) -> Option<Vec3> {
        if tile_c < 0
            || tile_r < 0
            || tile_c >= self.vertices_per_side
            || tile_r >= self.vertices_per_side
            || cell_c < 0
            || cell_r < 0
            || cell_c >= MAPCELL_DIM
            || cell_r >= MAPCELL_DIM
        {
            return None;
        }
        Some(Vec3 {
            x: self.tile_col_to_world[tile_c as usize]
                + self.cell_to_world_coord[cell_c as usize]
                + HALF_WORLD_UNITS_PER_CELL,
            y: self.tile_row_to_world[tile_r as usize]
                - self.cell_to_world_coord[cell_r as usize]
                - HALF_WORLD_UNITS_PER_CELL,
            z: 0.0,
        })
    }

    /// `cellToWorld`: the centre of cell `(row, col)`, `z = 0`; `(0, 0, 0)` when out of range (terrain.h:373-392).
    pub fn cell_to_world(&self, row: i32, col: i32) -> Vec3 {
        let cells = self.vertices_per_side * MAPCELL_DIM;
        if row < 0 || col < 0 || row >= cells || col >= cells {
            return Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            };
        }
        Vec3 {
            x: self.cell_col_to_world[col as usize] + HALF_WORLD_UNITS_PER_CELL,
            y: self.cell_row_to_world[row as usize] - HALF_WORLD_UNITS_PER_CELL,
            z: 0.0,
        }
    }

    /// `getCellPos`: the centre of a cell with `z` set to the terrain elevation there (terrain.h:400-410). The first
    /// two lines are double arithmetic in the C++ (`worldUnitsPerVertex/3.` is a double divide).
    pub fn get_cell_pos(&self, cell_r: i32, cell_c: i32) -> Vec3 {
        let wupv = f64::from(WORLD_UNITS_PER_VERTEX);
        let mut p = Vec3 {
            x: (f64::from(cell_c) * (wupv / 3.0) + (wupv / 6.0)) as f32,
            y: (f64::from(cell_r) * (wupv / 3.0) + (wupv / 6.0)) as f32,
            z: 0.0,
        };
        p.x += self.map_top_left.x;
        p.y = self.map_top_left.y - p.y;
        p.z = self.elevation(&p);
        p
    }

    /// `IsValidTerrainPosition`: strictly inside the map square.
    pub fn is_valid_position(&self, p: &Vec3) -> bool {
        let m = self.world_units_map_side / 2.0f32;
        p.x > -m && p.x < m && p.y > -m && p.y < m
    }

    /// `IsEditorSelectTerrainPosition`: inside the map minus one vertex of border.
    pub fn is_editor_select_position(&self, p: &Vec3) -> bool {
        let m = (self.world_units_map_side / 2.0f32) - WORLD_UNITS_PER_VERTEX;
        p.x > -m && p.x < m && p.y > -m && p.y < m
    }

    /// `Terrain::IsGameSelectTerrainPosition` (inside the playable area: map minus two vertices of border).
    pub fn is_game_select_position(&self, p: &Vec3) -> bool {
        let m = (self.world_units_map_side / 2.0f32) - (WORLD_UNITS_PER_VERTEX * 2.0f32);
        p.x > -m && p.x < m && p.y > -m && p.y < m
    }

    // ------------------------------------------------------------------------------------------------------------
    // Ground shape (mapdata.cpp), see `mesh`
    // ------------------------------------------------------------------------------------------------------------

    /// `getTerrainElevation(position)` (`MapData::terrainElevation`, mapdata.cpp:1584).
    pub fn elevation(&self, p: &Vec3) -> f32 {
        mesh::elevation(self, p)
    }

    /// `getTerrainNormal`: `MapData::terrainNormal` (mapdata.cpp:1383), read by mover state and hill factors.
    pub fn normal(&self, p: &Vec3) -> Vec3 {
        mesh::normal(self, p)
    }

    /// The normal `MapData::terrainAngle` writes through its out parameter (mapdata.cpp:1070), which vehicle appearance
    /// pitch and roll read. Zero where the C++ writes no normal (see `mesh::angle_normal`).
    pub fn angle_normal(&self, p: &Vec3) -> Vec3 {
        mesh::angle_normal(self, p)
    }

    /// `getTerrainAngle` in degrees (pitch of the ground, `MapData::terrainAngle`, mapdata.cpp:1070).
    pub fn angle_deg(&self, p: &Vec3) -> f32 {
        mesh::angle(self, p).0
    }

    /// `getTerrainAngle(position, &normal)`: also the normal the C++ writes through its out pointer (`None` where the
    /// C++ leaves it untouched: the position is outside the playable mesh).
    pub fn angle_and_normal(&self, p: &Vec3) -> (f32, Option<Vec3>) {
        mesh::angle(self, p)
    }

    // ------------------------------------------------------------------------------------------------------------
    // Water
    // ------------------------------------------------------------------------------------------------------------

    /// `Terrain::getWater` (terrain.cpp:1542): 2 = deep, 1 = shallow, 0 = dry, from the ground elevation under `p`
    /// against `waterElevation` and `shallowDepth`.
    pub fn get_water(&self, p: &Vec3) -> i32 {
        let elevation = self.elevation(p);
        if elevation < (self.water_elevation - self.shallow_depth) {
            return 2;
        }
        if elevation < self.water_elevation {
            return 1;
        }
        0
    }

    /// Depth of the water surface above the ground at `p` (`waterElevation - elevation`; negative on dry land).
    /// A convenience for callers, not a C++ function: use [`Terrain::get_water`] for the original 0/1/2 test.
    pub fn water_depth(&self, p: &Vec3) -> f32 {
        self.water_elevation - self.elevation(p)
    }

    // ------------------------------------------------------------------------------------------------------------
    // The per-cell map (mc2_path::MissionMap inside MoveData)
    // ------------------------------------------------------------------------------------------------------------

    /// The per-cell map (passability, path locks, mover bits, gates, mines, forests, local heights): `GameMap`.
    pub fn map(&self) -> &MissionMap {
        &self.move_data.mission
    }

    /// Mutable per-cell map.
    pub fn map_mut(&mut self) -> &mut MissionMap {
        &mut self.move_data.mission
    }

    /// The movement data of the mission (`MOVE_readData`): cell map, the three global maps, special areas.
    pub fn move_data(&self) -> &MoveData {
        &self.move_data
    }

    /// Mutable movement data (lane B's path queries open/close areas and doors).
    pub fn move_data_mut(&mut self) -> &mut MoveData {
        &mut self.move_data
    }

    /// Replace the movement data (tests, tools).
    pub fn set_move_data(&mut self, data: MoveData) {
        self.move_data = data;
    }

    /// Cells per map side (`realVerticesMapSide * MAPCELL_DIM`).
    pub fn cells_per_side(&self) -> i32 {
        self.vertices_per_side * MAPCELL_DIM
    }

    /// `GameMap->inBounds(row, col)`.
    pub fn in_bounds(&self, row: i32, col: i32) -> bool {
        self.move_data.mission.in_bounds(row, col)
    }

    /// The cell at `(row, col)`; `None` outside the map.
    pub fn cell(&self, row: i32, col: i32) -> Option<MapCell> {
        self.in_bounds(row, col)
            .then(|| self.move_data.mission.cell(row, col))
    }

    /// Every cell's data word in row-major order (the terrain part of a save snapshot: mover, path lock, gate, mine and
    /// passable bits all live in these words).
    pub fn cell_words(&self) -> Vec<u32> {
        let m = &self.move_data.mission;
        (0..m.height())
            .flat_map(|r| (0..m.width()).map(move |c| (r, c)))
            .map(|(r, c)| m.cell(r, c).0)
            .collect()
    }

    /// Restore the words saved by [`Terrain::cell_words`]; fails if the cell count differs.
    pub fn set_cell_words(&mut self, words: &[u32]) -> Result<(), String> {
        let m = &mut self.move_data.mission;
        let (w, h) = (m.width(), m.height());
        if words.len() != (w as usize) * (h as usize) {
            return Err(format!("{} cell words for a {w}x{h} map", words.len()));
        }
        for r in 0..h {
            for c in 0..w {
                m.cell_mut(r, c).0 = words[(r * w + c) as usize];
            }
        }
        Ok(())
    }

    /// Apply `f` to the cell at `(row, col)`; ignored outside the map. All the setters below go through this.
    pub fn with_cell_mut(&mut self, row: i32, col: i32, f: impl FnOnce(&mut MapCell)) {
        if self.in_bounds(row, col) {
            f(self.move_data.mission.cell_mut(row, col));
        }
    }

    /// The terrain object block index `(tileRow/20) * blocksMapSide + tileCol/20` of a vertex.
    pub fn block_of_tile(&self, tile_r: i32, tile_c: i32) -> i32 {
        (tile_r / VERTICES_BLOCK_SIDE) * self.blocks_per_side + tile_c / VERTICES_BLOCK_SIDE
    }

    /// `Terrain::setObjBlockActive` (range checked like the C++).
    pub fn set_obj_block_active(&mut self, block: i32, active: bool) {
        if block >= 0 && (block as usize) < self.obj_blocks.len() {
            self.obj_blocks[block as usize].active = active;
        }
    }

    /// `Terrain::clearObjBlocksActive`.
    pub fn clear_obj_blocks_active(&mut self) {
        for b in &mut self.obj_blocks {
            b.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(side: i32, e: f32) -> Terrain {
        Terrain::from_elevations(side, &vec![e; (side * side) as usize])
    }

    #[test]
    fn constants_match_the_float_expressions_of_terrain_cpp() {
        assert_eq!(WORLD_UNITS_PER_CELL, 128.0f32 / 3.0f32);
        assert_eq!(HALF_WORLD_UNITS_PER_CELL, (128.0f32 / 3.0f32) / 2.0);
        assert_eq!(METERS_PER_CELL, (128.0f32 / 3.0f32) * 0.2f32);
        assert_eq!(WORLD_UNITS_PER_METER, 5.01f32);
        assert_ne!(WORLD_UNITS_PER_METER, 1.0 / METERS_PER_WORLD_UNIT);
    }

    #[test]
    fn map_geometry_for_120_vertices() {
        let t = flat(120, 0.0);
        assert_eq!(t.blocks_per_side, 6);
        assert_eq!(t.half_vertices_per_side, 60);
        assert_eq!(t.world_units_map_side, 15360.0);
        assert_eq!(t.map_top_left.x, -7680.0);
        assert_eq!(t.map_top_left.y, 7680.0);
        assert_eq!(t.cells_per_side(), 360);
        assert_eq!(t.obj_blocks.len(), 36);
    }

    #[test]
    fn world_to_cell_truncates_toward_zero() {
        let t = flat(60, 0.0);
        // top-left corner of the map and half a cell outside it both land in cell (0, 0)
        let tl = t.map_top_left;
        assert_eq!(t.world_to_cell(&tl), (0, 0));
        let p = Vec3 {
            x: tl.x - 20.0,
            y: tl.y + 20.0,
            z: 0.0,
        };
        assert_eq!(t.world_to_cell(&p), (0, 0));
        let p = Vec3 {
            x: tl.x + 43.0,
            y: tl.y - 85.4,
            z: 0.0,
        };
        assert_eq!(t.world_to_cell(&p), (2, 1));
    }

    #[test]
    fn cell_round_trip_and_bounds() {
        let t = flat(60, 0.0);
        for (r, c) in [(0, 0), (1, 2), (100, 77), (179, 179)] {
            let w = t.cell_to_world(r, c);
            assert_eq!(t.world_to_cell(&w), (r, c), "cell {r},{c}");
        }
        assert_eq!(
            t.cell_to_world(-1, 0),
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0
            }
        );
        assert_eq!(
            t.cell_to_world(0, 180),
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0
            }
        );
    }

    #[test]
    fn tile_cell_to_world_leaves_output_untouched_out_of_range() {
        let t = flat(60, 0.0);
        assert!(t.tile_cell_to_world(0, 0, 3, 0).is_none());
        assert!(t.tile_cell_to_world(60, 0, 0, 0).is_none());
        let p = t.tile_cell_to_world(1, 2, 1, 1).unwrap();
        // tile (1,2) cell (1,1) is cell (4,7)
        let q = t.cell_to_world(4, 7);
        assert!((p.x - q.x).abs() < 1e-3 && (p.y - q.y).abs() < 1e-3);
    }

    #[test]
    fn game_select_position_excludes_two_vertices_of_border() {
        let t = flat(60, 0.0);
        let m = 3840.0 - 256.0;
        assert!(t.is_game_select_position(&Vec3 {
            x: m - 1.0,
            y: 0.0,
            z: 0.0
        }));
        assert!(!t.is_game_select_position(&Vec3 {
            x: m,
            y: 0.0,
            z: 0.0
        }));
        assert!(!t.is_game_select_position(&Vec3 {
            x: 0.0,
            y: -m,
            z: 0.0
        }));
        assert!(t.is_valid_position(&Vec3 {
            x: 3839.0,
            y: 0.0,
            z: 0.0
        }));
        assert!(!t.is_valid_position(&Vec3 {
            x: 3840.0,
            y: 0.0,
            z: 0.0
        }));
    }

    #[test]
    fn water_levels() {
        // get_water: deep below (waterElevation - shallowDepth), shallow below waterElevation, else dry
        let mut t = flat(60, 5.0);
        let p = Vec3 {
            x: 100.0,
            y: 100.0,
            z: 0.0,
        };
        t.shallow_depth = 3.0;
        t.water_elevation = 10.0;
        assert_eq!(t.get_water(&p), 2);
        t.water_elevation = 7.5;
        assert_eq!(t.get_water(&p), 1);
        t.water_elevation = 5.0;
        assert_eq!(t.get_water(&p), 0);
        assert_eq!(t.water_depth(&p), 0.0);
    }

    #[test]
    fn cell_words_round_trip() {
        let mut t = flat(20, 0.0);
        t.set_mine(3, 4, 2);
        t.set_gate(59, 59, 1);
        let words = t.cell_words();
        assert_eq!(words.len(), 3600);
        t.set_mine(3, 4, 0);
        t.set_cell_words(&words).unwrap();
        assert_eq!(t.get_mine(3, 4), 2);
        assert_eq!(t.get_gate(59, 59), 1);
        assert!(t.set_cell_words(&words[1..]).is_err());
    }

    #[test]
    fn default_world_terrain_is_empty() {
        let t = Terrain::default();
        assert_eq!(t.vertices_per_side, 0);
        assert_eq!(t.map().width(), 0);
        assert_eq!(t.cell(0, 0), None);
        // queries on an unloaded terrain are harmless
        let p = Vec3 {
            x: 5.0,
            y: 5.0,
            z: 0.0,
        };
        assert_eq!(t.elevation(&p), 0.0);
        assert_eq!(t.world_to_tile_cell(&p), (0, 0, 0, 0));
        assert_eq!(t.get_water(&p), 0);
        assert!(!t.is_game_select_position(&p));
    }
}

// ---- save/load (DESIGN.md section 9): the terrain's dynamic state is the mission cell words (mover bits, path locks, gate and mine
// bits), the global move maps' doors and areas, and the object-block activity the camera sets each frame. The vertex grid,
// tiles, water and the static parts of the move data come from the mission packets; `load_into` keeps them from the base world.

crate::snapshot_struct!(ObjBlockInfo {
    active,
    num_collidable_objects,
    num_objects,
    first_handle,
});

impl crate::save::Snapshot for mc2_path::global::GlobalMapDynamic {
    fn save(&self, w: &mut crate::save::Writer) {
        self.use_closed_areas.save(w);
        self.mover_team_id.save(w);
        w.len(self.areas.len());
        for &(team_id, owner_wid, open) in &self.areas {
            team_id.save(w);
            owner_wid.save(w);
            open.save(w);
        }
        self.doors.save(w);
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        let use_closed_areas = bool::load(r)?;
        let mover_team_id = i8::load(r)?;
        let n = r.length()?;
        let mut areas = Vec::with_capacity(n.min(1 << 12));
        for _ in 0..n {
            areas.push((i8::load(r)?, i32::load(r)?, bool::load(r)?));
        }
        Ok(mc2_path::global::GlobalMapDynamic {
            use_closed_areas,
            mover_team_id,
            areas,
            doors: Vec::load(r)?,
        })
    }
}

impl crate::save::Snapshot for Terrain {
    fn save(&self, w: &mut crate::save::Writer) {
        self.obj_blocks.save(w);
        self.cell_words().save(w);
        for level in 0..3 {
            self.move_data().global[level].dynamic_state().save(w);
        }
    }
    /// The terrain cannot be built from a save alone (its geometry comes from the mission packets): use [`Snapshot::load_into`]
    /// on the terrain of a freshly initialised world.
    fn load(_r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        Err(crate::save::SaveError(
            "terrain state is restored over the base world (load_into)".into(),
        ))
    }
    fn load_into(&mut self, r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<()> {
        use crate::save::SaveError;
        self.obj_blocks = Vec::load(r)?;
        let words = Vec::<u32>::load(r)?;
        self.set_cell_words(&words).map_err(SaveError)?;
        for level in 0..3 {
            let state = mc2_path::global::GlobalMapDynamic::load(r)?;
            self.move_data_mut().global[level]
                .set_dynamic_state(&state)
                .map_err(SaveError)?;
        }
        Ok(())
    }
}
