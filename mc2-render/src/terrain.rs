// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Terrain mesh from the mission's vertex grid.
//!
//! Ported from `mclib/terrain.cpp` (world <-> grid mapping, `IsGameSelectTerrainPosition`),
//! `mclib/mapdata.cpp` (`makeLists` diagonal choice, `terrainElevation`, `terrainLight`,
//! `calcWater` flags) and `mclib/quad.cpp` (tile corner order and triangle split).
//!
//! The original rebuilds a camera-centred window of `visibleVerticesPerSide`^2 vertices every frame
//! and projects them on the CPU.  Here the whole map becomes one static indexed mesh in world
//! coordinates (x east, y north, z up); culling and projection happen on the GPU.

use crate::math::Vec3;
use mc2_assets::map::TerrainVertex;

/// `Terrain::worldUnitsPerVertex`.
pub const WORLD_UNITS_PER_VERTEX: f32 = 128.0;
/// `MAPCELL_DIM`: move-map cells per tile edge.
pub const CELLS_PER_TILE: usize = 3;
/// `Terrain::verticesBlockSide`.
pub const VERTICES_BLOCK_SIDE: usize = 20;

/// Which way a tile is split into two triangles (`TerrainQuad::uvMode`).
///
/// `BottomRight`: the diagonal runs from the north-west to the south-east corner.
/// `BottomLeft`: from the north-east to the south-west corner.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Diagonal {
    BottomRight,
    BottomLeft,
}

/// Terrain type numbers that make up the road / pavement ("cement") family
/// (`BASE_CEMENT_TYPE`, `START_CEMENT_TYPE`, `END_CEMENT_TYPE` in `terrtxm.h`).
pub fn is_cement_type(t: u32) -> bool {
    t == 10 || (13..=20).contains(&t)
}

/// How a tile is textured in colour-map mode (`TerrainTextures::setTexture` flags +
/// `TerrainQuad::setupTextures`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TileKind {
    /// Plain terrain: the burned-in colour map plus the detail texture.
    ColorMap,
    /// Road / transition tile whose own texture has an alpha channel, drawn over the colour map.
    CementOverlay,
    /// Solid pavement: the tile texture replaces the colour map.
    CementSolid,
}

/// Everything needed to pick a tile's texture.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TileInfo {
    /// Four terrain types packed `v0 | v1 << 8 | v2 << 16 | v3 << 24` (`MapData::calcTransitions`).
    pub type_info: u32,
    /// Overlay index from the top word of `textureData`; `0xffff` means none.
    pub overlay: u32,
    pub kind: TileKind,
}

impl TileInfo {
    /// Name of the precomputed transition texture shipped for this tile
    /// (`createTransition`: `"%08d.%08d.txm"`).
    pub fn transition_file(&self) -> String {
        format!("{:08}.{:08}.txm", self.type_info, self.overlay)
    }

    /// True when the tile is a single base terrain type without overlay (the first branch of
    /// `setTexture`): its texture is the type's own bitmap, not a composited transition.
    pub fn is_uniform(&self) -> bool {
        let t = self.type_info;
        let v0 = t & 0xff;
        self.overlay == 0xffff && (t >> 8) & 0xff == v0 && (t >> 16) & 0xff == v0 && t >> 24 == v0
    }
}

/// GPU vertex of the terrain mesh.  Tiles do not share vertices so every tile can carry its own
/// texture selection as a flat attribute.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TerrainGpuVertex {
    pub position: [f32; 3],
    /// Bit 0..7: the vertex's `water` byte (bit 0 = below water, bits 6/7 = wave phase).
    /// Bit 8: vertex lies in the fully hazed border ring.
    pub flags: u32,
    /// Stored vertex normal.  Its length is left alone: the game's terrain light ramp uses
    /// `vertexNormal * lightDirection` unnormalised, so rough ground is slightly darker.
    pub normal: [f32; 3],
    /// Tile texture selection, see [`pack_tile_word`].
    pub tile: u32,
    /// Position inside the tile, `(0,0)` at the north-west corner.
    pub tile_uv: [f32; 2],
}

/// Tile texture word: atlas layer in the low 16 bits (`0xffff` = none) and the draw mode above.
pub fn pack_tile_word(layer: u32, mode: TileKind) -> u32 {
    let m = match mode {
        TileKind::ColorMap => 0,
        TileKind::CementOverlay => 1,
        TileKind::CementSolid => 2,
    };
    (layer & 0xffff) | (m << 16)
}

pub const FLAG_EDGE_HAZE: u32 = 1 << 8;

#[derive(Clone)]
pub struct TerrainMap {
    side: usize,
    vertices: Vec<TerrainVertex>,
}

#[derive(Debug)]
pub struct TerrainMesh {
    /// Four vertices per tile in the C++ quad order `v0` NW, `v1` NE, `v2` SE, `v3` SW.
    pub vertices: Vec<TerrainGpuVertex>,
    /// Two triangles per tile, counter-clockwise seen from above.
    pub indices: Vec<u32>,
    /// Indices of the tiles that carry water (any corner with `water & 1`), same triangulation.
    pub water_indices: Vec<u32>,
}

impl TerrainMap {
    pub fn new(side: usize, vertices: Vec<TerrainVertex>) -> Self {
        Self::try_new(side, vertices).expect("terrain grid must be a square of at least 2x2")
    }

    /// Validating constructor for untrusted mission data: a square grid with at least one tile.
    pub fn try_new(side: usize, vertices: Vec<TerrainVertex>) -> Result<Self, String> {
        if side < 2 || side.checked_mul(side) != Some(vertices.len()) {
            return Err(format!(
                "terrain packet is {} vertices, not a square grid of at least 2x2 (side {side})",
                vertices.len()
            ));
        }
        Ok(Self { side, vertices })
    }

    /// Vertices per map edge (`Terrain::realVerticesMapSide`).
    pub fn side(&self) -> usize {
        self.side
    }
    /// Tiles per map edge.
    pub fn tiles(&self) -> usize {
        self.side.saturating_sub(1)
    }
    /// `Terrain::worldUnitsMapSide`.
    pub fn world_side(&self) -> f32 {
        self.side as f32 * WORLD_UNITS_PER_VERTEX
    }
    /// `Terrain::mapTopLeft3d` (x, y).
    pub fn top_left(&self) -> (f32, f32) {
        let h = self.world_side() / 2.0;
        (-h, h)
    }
    pub fn vertex(&self, col: usize, row: usize) -> &TerrainVertex {
        &self.vertices[row * self.side + col]
    }
    pub fn vertices(&self) -> &[TerrainVertex] {
        &self.vertices
    }

    /// World position of vertex `(col, row)` (`makeLists`: `vx`, `vy`).
    pub fn vertex_position(&self, col: usize, row: usize) -> Vec3 {
        let half = (self.side >> 1) as f32;
        Vec3::new(
            (col as f32 - half) * WORLD_UNITS_PER_VERTEX,
            (half - row as f32) * WORLD_UNITS_PER_VERTEX,
            self.vertex(col, row).elevation,
        )
    }

    /// `Terrain::worldToTile`: grid cell containing a world position (may be out of range).
    pub fn world_to_tile(&self, x: f32, y: f32) -> (i32, i32) {
        let (tx, ty) = self.top_left();
        (
            ((x - tx) / WORLD_UNITS_PER_VERTEX).floor() as i32,
            ((ty - y) / WORLD_UNITS_PER_VERTEX).floor() as i32,
        )
    }

    /// `Terrain::IsValidTerrainPosition`.
    pub fn is_valid_position(&self, x: f32, y: f32) -> bool {
        let h = self.world_side() / 2.0;
        x > -h && x < h && y > -h && y < h
    }

    /// `Terrain::IsGameSelectTerrainPosition`: inside the map minus two vertices.
    pub fn is_game_select_position(&self, x: f32, y: f32) -> bool {
        let m = self.world_side() / 2.0 - WORLD_UNITS_PER_VERTEX * 2.0;
        x > -m && x < m && y > -m && y < m
    }

    /// The diagonal `MapData::makeLists` gives tile `(col, row)`: it depends only on the parity of
    /// `col + row` (the C++ derives it from the visible window's origin and offset, which is the
    /// absolute parity).
    pub fn diagonal(col: usize, row: usize) -> Diagonal {
        if (col + row).is_multiple_of(2) {
            Diagonal::BottomRight
        } else {
            Diagonal::BottomLeft
        }
    }

    /// Triangle index triples of a tile.  Corner order of the C++ quad is
    /// `v0 = (col,row)`, `v1 = (col+1,row)`, `v2 = (col+1,row+1)`, `v3 = (col,row+1)`; the
    /// triples here are re-ordered counter-clockwise when seen from above (the grid's `row` axis
    /// points south).
    pub fn tile_triangles(&self, col: usize, row: usize) -> [[u32; 3]; 2] {
        let s = self.side as u32;
        let v0 = row as u32 * s + col as u32;
        let v1 = v0 + 1;
        let v3 = v0 + s;
        let v2 = v3 + 1;
        match Self::diagonal(col, row) {
            Diagonal::BottomRight => [[v0, v3, v2], [v0, v2, v1]],
            Diagonal::BottomLeft => [[v0, v3, v1], [v1, v3, v2]],
        }
    }

    /// Triangle triples over a tile's four local corners `[v0 NW, v1 NE, v2 SE, v3 SW]`,
    /// counter-clockwise seen from above.
    pub fn local_triangles(col: usize, row: usize) -> [[u32; 3]; 2] {
        match Self::diagonal(col, row) {
            Diagonal::BottomRight => [[0, 3, 2], [0, 2, 1]],
            Diagonal::BottomLeft => [[0, 3, 1], [1, 3, 2]],
        }
    }

    /// `MapData::terrainElevation`: planar interpolation on the triangle containing `(x, y)`.
    /// Positions off the map return 0 like the original.
    pub fn elevation_at(&self, x: f32, y: f32) -> f32 {
        let Some((col, row, fx, fy)) = self.locate(x, y) else {
            return 0.0;
        };
        let h = |c: usize, r: usize| self.vertex(c, r).elevation;
        let (h0, h1, h2, h3) = (
            h(col, row),
            h(col + 1, row),
            h(col + 1, row + 1),
            h(col, row + 1),
        );
        // fx grows east, fy grows south, both in 0..1.
        match Self::diagonal(col, row) {
            Diagonal::BottomRight => {
                if fx > fy {
                    // upper-right triangle (v0, v1, v2)
                    h0 + (h1 - h0) * fx + (h2 - h1) * fy
                } else {
                    // lower-left triangle (v0, v2, v3)
                    h0 + (h2 - h3) * fx + (h3 - h0) * fy
                }
            }
            Diagonal::BottomLeft => {
                if 1.0 - fx > fy {
                    // upper-left triangle (v0, v1, v3)
                    h0 + (h1 - h0) * fx + (h3 - h0) * fy
                } else {
                    // lower-right triangle (v1, v2, v3)
                    h3 + (h2 - h3) * fx + (h2 - h1) * (fy - 1.0)
                }
            }
        }
    }

    /// Tile and fractional position of a world point; `None` when outside the tile grid.
    fn locate(&self, x: f32, y: f32) -> Option<(usize, usize, f32, f32)> {
        if !self.is_valid_position(x, y) {
            return None;
        }
        let (tx, ty) = self.top_left();
        let gx = (x - tx) / WORLD_UNITS_PER_VERTEX;
        let gy = (ty - y) / WORLD_UNITS_PER_VERTEX;
        let col = gx.floor() as i64;
        let row = gy.floor() as i64;
        if col < 0 || row < 0 || col as usize >= self.side - 1 || row as usize >= self.side - 1 {
            return None;
        }
        Some((col as usize, row as usize, gx - col as f32, gy - row as f32))
    }

    /// `MapData::terrainLight`: the stored vertex normals weighted towards the nearest corners,
    /// dotted with the light direction.  The game feeds this to every building and tree.
    pub fn light_intensity_at(&self, x: f32, y: f32, light_dir: Vec3) -> f32 {
        let Some((col, row, _, _)) = self.locate(x, y) else {
            return 1.0;
        };
        let corners = [
            (col, row),
            (col + 1, row),
            (col + 1, row + 1),
            (col, row + 1),
        ];
        let p = Vec3::new(x, y, self.elevation_at(x, y));
        let mut dist = [0.0f32; 4];
        for (d, (c, r)) in dist.iter_mut().zip(corners) {
            *d = (p - self.vertex_position(c, r)).length();
        }
        let max = dist.iter().cloned().fold(0.0f32, f32::max);
        let mut weight = 0.0;
        let mut n = Vec3::ZERO;
        for (d, (c, r)) in dist.iter().zip(corners) {
            let w = max - d;
            weight += w;
            n = n + Vec3::from_array(self.vertex(c, r).normal) * w;
        }
        let n = if weight > 1e-6 {
            n * (1.0 / weight)
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        };
        n.dot(light_dir)
    }

    /// Smooth surface normal at a point, from the finite-difference gradient of the height field.
    pub fn normal_at(&self, x: f32, y: f32) -> Vec3 {
        let e = WORLD_UNITS_PER_VERTEX * 0.5;
        let dx = self.elevation_at(x + e, y) - self.elevation_at(x - e, y);
        let dy = self.elevation_at(x, y + e) - self.elevation_at(x, y - e);
        Vec3::new(-dx / (2.0 * e), -dy / (2.0 * e), 1.0).normalized()
    }

    /// `MapData::calcLight` for the normals: eight triangle normals around the vertex, averaged.
    /// The mission packet already stores these; this recomputes them for tools and for tests.
    pub fn computed_normal(&self, col: usize, row: usize) -> Vec3 {
        if col == 0 || row == 0 || col + 1 >= self.side || row + 1 >= self.side {
            return Vec3::new(0.0, 0.0, 1.0);
        }
        let w = WORLD_UNITS_PER_VERTEX;
        let h = |c: usize, r: usize| self.vertex(c, r).elevation;
        let h0 = h(col, row);
        // Neighbour offsets as in `calcLight` (v1..v8 walk the ring starting at the north-west).
        let ring = [
            (col - 1, row - 1),
            (col, row - 1),
            (col + 1, row - 1),
            (col + 1, row),
            (col + 1, row + 1),
            (col, row + 1),
            (col - 1, row + 1),
            (col - 1, row),
        ];
        let off = |c: usize, r: usize| {
            Vec3::new(
                (c as f32 - col as f32) * w,
                -(r as f32 - row as f32) * w,
                h(c, r) - h0,
            )
        };
        let mut sum = Vec3::ZERO;
        for i in 0..8 {
            let a = off(ring[i].0, ring[i].1);
            let b = off(ring[(i + 1) % 8].0, ring[(i + 1) % 8].1);
            let mut n = a.cross(b).normalized();
            if n.z < 0.0 {
                n = -n;
            }
            sum = sum + n;
        }
        (sum * (1.0 / 8.0)).normalized()
    }

    /// Classifies a tile for colour-map mode (`TerrainTextures::setTexture` + `setupTextures`).
    pub fn tile_info(&self, col: usize, row: usize) -> TileInfo {
        let t = |c: usize, r: usize| self.vertex(c, r).terrain_type & 0xff;
        let (t0, t1, t2, t3) = (
            t(col, row),
            t(col + 1, row),
            t(col + 1, row + 1),
            t(col, row + 1),
        );
        let type_info = t0 | (t1 << 8) | (t2 << 16) | (t3 << 24);
        let overlay = self.vertex(col, row).texture_data >> 16;
        let any_cement = [t0, t1, t2, t3].iter().any(|&v| is_cement_type(v));
        let uniform = t0 == t1 && t1 == t2 && t2 == t3 && overlay == 0xffff;
        let kind = if uniform {
            if any_cement {
                TileKind::CementSolid
            } else {
                TileKind::ColorMap
            }
        } else if overlay != 0xffff || any_cement {
            // CEMENT flag set; ALPHA when any corner is not a cement type.
            if [t0, t1, t2, t3].iter().any(|&v| !is_cement_type(v)) {
                TileKind::CementOverlay
            } else {
                TileKind::CementSolid
            }
        } else {
            TileKind::ColorMap
        };
        TileInfo {
            type_info,
            overlay,
            kind,
        }
    }

    /// Builds the static mesh.  `tile_words` holds one [`pack_tile_word`] per tile (row-major);
    /// pass `None` for plain colour-map tiles everywhere.
    pub fn build_mesh(&self, tile_words: Option<&[u32]>) -> TerrainMesh {
        let tiles = self.tiles();
        if let Some(w) = tile_words {
            assert_eq!(w.len(), tiles * tiles, "one tile word per tile");
        }
        let corner_uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let mut vertices = Vec::with_capacity(tiles * tiles * 4);
        let mut indices = Vec::with_capacity(tiles * tiles * 6);
        let mut water_indices = Vec::new();
        for row in 0..tiles {
            for col in 0..tiles {
                let word = tile_words
                    .map(|w| w[row * tiles + col])
                    .unwrap_or_else(|| pack_tile_word(0xffff, TileKind::ColorMap));
                let corners = [
                    (col, row),
                    (col + 1, row),
                    (col + 1, row + 1),
                    (col, row + 1),
                ];
                let base = vertices.len() as u32;
                let mut wet = false;
                for (k, (c, r)) in corners.iter().enumerate() {
                    let p = self.vertex_position(*c, *r);
                    let v = self.vertex(*c, *r);
                    let mut n = Vec3::from_array(v.normal);
                    if n.length() < 0.5 || n.z <= 0.0 {
                        n = self.computed_normal(*c, *r);
                    }
                    let mut flags = v.water as u32;
                    if !self.is_game_select_position(p.x, p.y) {
                        flags |= FLAG_EDGE_HAZE;
                    }
                    wet |= v.water & 1 != 0;
                    vertices.push(TerrainGpuVertex {
                        position: p.to_array(),
                        flags,
                        normal: n.to_array(),
                        tile: word,
                        tile_uv: corner_uv[k],
                    });
                }
                for tri in Self::local_triangles(col, row) {
                    for i in tri {
                        indices.push(base + i);
                    }
                }
                if wet {
                    for tri in Self::local_triangles(col, row) {
                        for i in tri {
                            water_indices.push(base + i);
                        }
                    }
                }
            }
        }
        TerrainMesh {
            vertices,
            indices,
            water_indices,
        }
    }

    /// Lowest and highest vertex elevation.
    pub fn elevation_range(&self) -> (f32, f32) {
        self.vertices
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), v| {
                (lo.min(v.elevation), hi.max(v.elevation))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(elevation: f32, ty: u32, texture_data: u32, water: u8) -> TerrainVertex {
        TerrainVertex {
            normal: [0.0, 0.0, 1.0],
            elevation,
            texture_data,
            local_rgb_light: 0,
            terrain_type: ty,
            selected: 0,
            water,
            shadow: 0,
            highlighted: 0,
        }
    }

    fn flat_map(side: usize, h: f32) -> TerrainMap {
        TerrainMap::new(
            side,
            (0..side * side)
                .map(|_| vertex(h, 1, 0xffff0000, 0))
                .collect(),
        )
    }

    /// Height field `z = a*x + b*y` sampled on the grid.
    fn ramp_map(side: usize, a: f32, b: f32) -> TerrainMap {
        let mut m = flat_map(side, 0.0);
        for row in 0..side {
            for col in 0..side {
                let p = m.vertex_position(col, row);
                m.vertices[row * side + col].elevation = a * p.x + b * p.y;
            }
        }
        m
    }

    #[test]
    fn malformed_grids_are_errors() {
        assert!(TerrainMap::try_new(0, vec![]).is_err());
        assert!(TerrainMap::try_new(1, vec![vertex(0.0, 1, 0, 0)]).is_err());
        assert!(TerrainMap::try_new(3, vec![vertex(0.0, 1, 0, 0); 8]).is_err());
        let m = TerrainMap::try_new(2, vec![vertex(0.0, 1, 0, 0); 4]).unwrap();
        assert_eq!(m.tiles(), 1);
    }

    #[test]
    fn grid_maps_to_world_coordinates() {
        let m = flat_map(100, 7.0);
        assert_eq!(m.world_side(), 12800.0);
        assert_eq!(m.top_left(), (-6400.0, 6400.0));
        // vertex (0,0) is the north-west corner, (50,50) the centre
        let nw = m.vertex_position(0, 0);
        assert_eq!((nw.x, nw.y, nw.z), (-6400.0, 6400.0, 7.0));
        let c = m.vertex_position(50, 50);
        assert_eq!((c.x, c.y), (0.0, 0.0));
        assert_eq!(m.world_to_tile(-6400.0 + 1.0, 6400.0 - 1.0), (0, 0));
        assert_eq!(m.world_to_tile(0.0, 0.0), (50, 50));
    }

    #[test]
    fn diagonal_alternates_like_a_checkerboard() {
        assert_eq!(TerrainMap::diagonal(0, 0), Diagonal::BottomRight);
        assert_eq!(TerrainMap::diagonal(1, 0), Diagonal::BottomLeft);
        assert_eq!(TerrainMap::diagonal(0, 1), Diagonal::BottomLeft);
        assert_eq!(TerrainMap::diagonal(1, 1), Diagonal::BottomRight);
        assert_eq!(TerrainMap::diagonal(7, 4), Diagonal::BottomLeft);
    }

    #[test]
    fn mesh_has_two_triangles_per_tile_facing_up() {
        let m = ramp_map(6, 0.05, -0.03);
        let mesh = m.build_mesh(None);
        assert_eq!(mesh.vertices.len(), 5 * 5 * 4);
        assert_eq!(mesh.indices.len(), 5 * 5 * 6);
        assert!(mesh.water_indices.is_empty());
        for t in mesh.indices.chunks(3) {
            let p = |i: u32| Vec3::from_array(mesh.vertices[i as usize].position);
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            assert!(n.z > 0.0, "triangle {t:?} faces down");
        }
        // every index is in range
        assert!(mesh
            .indices
            .iter()
            .all(|&i| (i as usize) < mesh.vertices.len()));
        // grid-indexed triangulation uses 4 distinct corners per tile
        let tri = m.tile_triangles(2, 3);
        let mut corners: Vec<u32> = tri.iter().flatten().copied().collect();
        corners.sort_unstable();
        corners.dedup();
        assert_eq!(corners.len(), 4);
    }

    #[test]
    fn diagonal_connects_the_expected_corners() {
        let m = flat_map(4, 0.0);
        let s = 4u32;
        // tile (0,0): NW-SE diagonal => triangles share v0 and v2
        let tri = m.tile_triangles(0, 0);
        let (v0, v2) = (0, s + 1);
        assert!(tri[0].contains(&v0) && tri[0].contains(&v2));
        assert!(tri[1].contains(&v0) && tri[1].contains(&v2));
        // tile (1,0): NE-SW diagonal => triangles share v1 and v3
        let tri = m.tile_triangles(1, 0);
        let (v1, v3) = (2, s + 1);
        assert!(tri[0].contains(&v1) && tri[0].contains(&v3));
        assert!(tri[1].contains(&v1) && tri[1].contains(&v3));
    }

    #[test]
    fn elevation_interpolates_a_plane_exactly_on_both_diagonals() {
        let m = ramp_map(8, 0.05, -0.03);
        // sample inside tiles of both diagonal kinds, in both triangles of each
        for &(col, row) in &[(2usize, 2usize), (3, 2), (2, 3), (4, 4)] {
            for &(fx, fy) in &[
                (0.2f32, 0.7f32),
                (0.8, 0.3),
                (0.5, 0.5),
                (0.1, 0.1),
                (0.9, 0.9),
            ] {
                let p0 = m.vertex_position(col, row);
                let x = p0.x + fx * WORLD_UNITS_PER_VERTEX;
                let y = p0.y - fy * WORLD_UNITS_PER_VERTEX;
                let want = 0.05 * x - 0.03 * y;
                let got = m.elevation_at(x, y);
                assert!(
                    (got - want).abs() < 1e-2,
                    "{col},{row} {fx},{fy}: {got} vs {want}"
                );
            }
        }
        // corners hit the stored heights
        let p = m.vertex_position(3, 3);
        assert!((m.elevation_at(p.x + 0.01, p.y - 0.01) - p.z).abs() < 0.1);
        // off the map the original returns zero
        assert_eq!(m.elevation_at(1.0e6, 0.0), 0.0);
    }

    #[test]
    fn elevation_uses_the_split_of_a_saddle() {
        // One tile whose corners alternate high/low: the diagonal decides the surface.
        let mut m = flat_map(4, 0.0);
        // tile (0,0) is BottomRight (NW-SE diagonal); raise v1 (NE) only
        m.vertices[1].elevation = 100.0;
        let p0 = m.vertex_position(0, 0);
        // centre of the tile lies on the NW-SE diagonal: height = (h0+h2)/2 = 0
        let mid = m.elevation_at(p0.x + 64.0, p0.y - 64.0);
        assert!(mid.abs() < 1e-3, "{mid}");
        // tile (1,0) is BottomLeft (NE-SW diagonal); its NE... raise v(2,0)
        let mut m2 = flat_map(4, 0.0);
        m2.vertices[2].elevation = 100.0; // vertex (2,0), NE corner of tile (1,0)
        let q0 = m2.vertex_position(1, 0);
        let mid = m2.elevation_at(q0.x + 64.0, q0.y - 64.0);
        assert!((mid - 50.0).abs() < 1e-3, "{mid}"); // lies on the NE-SW diagonal: (h1+h3)/2
    }

    #[test]
    fn computed_normals_match_the_slope() {
        let m = ramp_map(8, 0.25, 0.0);
        let n = m.computed_normal(4, 4);
        let want = Vec3::new(-0.25, 0.0, 1.0).normalized();
        assert!(
            (n.x - want.x).abs() < 1e-3 && (n.z - want.z).abs() < 1e-3,
            "{n:?}"
        );
        let flat = flat_map(8, 5.0).computed_normal(4, 4);
        assert!((flat.z - 1.0).abs() < 1e-6);
        // border vertices fall back to straight up like `calcLight`'s default
        assert_eq!(
            flat_map(8, 5.0).computed_normal(0, 3),
            Vec3::new(0.0, 0.0, 1.0)
        );
    }

    #[test]
    fn light_intensity_is_dot_of_weighted_normal() {
        let m = flat_map(8, 0.0);
        let l = Vec3::new(0.0, 0.0, 1.0);
        assert!((m.light_intensity_at(10.0, 10.0, l) - 1.0).abs() < 1e-5);
        // off map: full bright
        assert_eq!(m.light_intensity_at(1e9, 0.0, l), 1.0);
    }

    #[test]
    fn border_ring_is_hazed_and_water_tiles_are_collected() {
        let mut m = flat_map(10, 0.0);
        let mesh = m.build_mesh(None);
        let hazed = mesh
            .vertices
            .iter()
            .filter(|v| v.flags & FLAG_EDGE_HAZE != 0)
            .count();
        assert!(hazed > 0 && hazed < mesh.vertices.len());
        // the north-west corner of the map is in the ring, the centre is not
        let at = |x: f32, y: f32| {
            mesh.vertices
                .iter()
                .find(|v| v.position[0] == x && v.position[1] == y)
                .unwrap()
        };
        assert!(at(-640.0, 640.0).flags & FLAG_EDGE_HAZE != 0);
        assert!(at(0.0, 0.0).flags & FLAG_EDGE_HAZE == 0);

        m.vertices[5 * 10 + 5].water = 1;
        let mesh = m.build_mesh(None);
        // vertex (5,5) belongs to 4 tiles
        assert_eq!(mesh.water_indices.len(), 4 * 6);
    }

    #[test]
    fn tile_words_travel_with_the_tile_corners() {
        let m = flat_map(4, 0.0);
        let mut words = vec![pack_tile_word(0xffff, TileKind::ColorMap); 9];
        words[4] = pack_tile_word(7, TileKind::CementOverlay);
        let mesh = m.build_mesh(Some(&words));
        let centre = &mesh.vertices[4 * 4..4 * 4 + 4];
        assert!(centre.iter().all(|v| v.tile == (7 | (1 << 16))));
        assert_eq!(centre[0].tile_uv, [0.0, 0.0]);
        assert_eq!(centre[2].tile_uv, [1.0, 1.0]);
        assert_eq!(mesh.vertices[0].tile, 0xffff);
        // the four corner positions are the tile's NW, NE, SE, SW
        assert!(centre[1].position[0] > centre[0].position[0]);
        assert!(centre[3].position[1] < centre[0].position[1]);
    }

    #[test]
    fn tile_classification_follows_set_texture() {
        // plain terrain
        let mut m = flat_map(4, 0.0);
        assert_eq!(m.tile_info(0, 0).kind, TileKind::ColorMap);
        assert!(m.tile_info(0, 0).is_uniform());
        // all-cement tile without overlay: solid
        for v in m.vertices.iter_mut() {
            v.terrain_type = 13;
        }
        assert_eq!(m.tile_info(1, 1).kind, TileKind::CementSolid);
        // cement tile touching another type: alpha overlay
        m.vertices[2 * 4 + 2].terrain_type = 1;
        let t = m.tile_info(1, 1);
        assert_eq!(t.kind, TileKind::CementOverlay);
        assert_eq!(t.type_info, 13 | (13 << 8) | (1 << 16) | (13 << 24));
        // overlay with all-cement corners is solid (road on pavement)
        let mut m = flat_map(4, 0.0);
        for v in m.vertices.iter_mut() {
            v.terrain_type = 13;
        }
        m.vertices[4 + 1].texture_data = 283 << 16;
        let t = m.tile_info(1, 1);
        assert_eq!(t.kind, TileKind::CementSolid);
        assert_eq!(t.transition_file(), "218959117.00000283.txm");
        // overlay on plain terrain: road, drawn as alpha overlay
        let mut m = flat_map(4, 0.0);
        m.vertices[4 + 1].texture_data = 262 << 16;
        let t = m.tile_info(1, 1);
        assert_eq!(t.kind, TileKind::CementOverlay);
        assert!(!t.is_uniform());
        // non-cement type change without overlay stays on the colour map
        let mut m = flat_map(4, 0.0);
        m.vertices[4 + 2].terrain_type = 9;
        assert_eq!(m.tile_info(1, 1).kind, TileKind::ColorMap);
    }
}
