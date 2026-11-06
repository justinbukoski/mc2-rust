// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Terrain texture selection: the burned-in colour map, detail and water bitmaps, and the
//! small tiles (roads, pavement, transitions) that sit on top of it.
//!
//! Ported from `Terrain::init` (which colour map file to use), `TerrainColorMap::init`
//! (`terrtxm2.cpp`: file names and fallbacks for the colour map, `.detail`, `.water`, `.waterNNNN`),
//! `TerrainTextures::init` (`terrtxm.cpp`: the `textures.fit` terrain-type table),
//! `TerrainTextures::createTransition` (the precomputed `%08d.%08d.txm` files: LZ compressed
//! 64x64 BGRA) and `TerrainQuad::setupTextures` (`quad.cpp`: which tiles use which bitmap).
//!
//! Transitions come from the baked `.txm` when the data ships one and are composited from masks
//! like `createTransition` does otherwise (see `transitions.rs`).  A tile whose bitmaps cannot
//! be found degrades to the colour map.

use crate::error::Result;
use crate::game::GameData;
use crate::mission::Mission;
use crate::terrain::{pack_tile_word, TerrainMap, TileInfo, TileKind};
use crate::transitions::{to_tile, Compositor};
use mc2_assets::tga::RgbaImage;
use std::collections::HashMap;

/// Edge length of the terrain tile bitmaps (`TERRAIN_TXM_SIZE`).
pub const TILE_SIZE: u32 = 64;
pub const TILE_BYTES: usize = (TILE_SIZE * TILE_SIZE * 4) as usize;

/// Decodes a `.txm`: a 64x64 image in DWORD `0xAARRGGBB` order (bytes B, G, R, A), returned as RGBA.
///
/// The Windows game stores them LZ compressed (`LZCompress`); the Linux build (`LINUX_BUILD`
/// branches of `createTransition`) writes zlib streams.  Both are accepted, told apart by the zlib
/// header byte.
pub fn decode_txm(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut raw =
        if bytes.len() > 2 && bytes[0] == 0x78 && matches!(bytes[1], 0x01 | 0x5e | 0x9c | 0xda) {
            let mut out = Vec::with_capacity(TILE_BYTES);
            flate2::read::ZlibDecoder::new(bytes)
                .take(TILE_BYTES as u64 + 1)
                .read_to_end(&mut out)?;
            out
        } else {
            mc2_formats::lzw::decompress(bytes, TILE_BYTES)?
        };
    if raw.len() != TILE_BYTES {
        return Err(crate::RenderError::Other(format!(
            "txm decompressed to {} bytes, expected {TILE_BYTES}",
            raw.len()
        )));
    }
    for px in raw.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Ok(raw)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum TileKey {
    /// A type's own bitmap (all four corners the same type, no overlay).
    Base(u32),
    /// A precomputed transition: `(type_info, overlay)`.
    Transition(u32, u32),
}

/// All bitmaps the terrain pass needs for one mission.
pub struct TerrainTextures {
    pub color_map: RgbaImage,
    pub color_map_name: String,
    pub detail: RgbaImage,
    /// VFS path the detail bitmap was read from (the hi-res replacement name).
    pub detail_name: String,
    pub water: RgbaImage,
    pub water_name: String,
    pub water_detail: Vec<RgbaImage>,
    pub water_detail_names: Vec<String>,
    /// RGBA tile bitmaps (64x64) forming the atlas layers.
    pub tile_layers: Vec<Vec<u8>>,
    /// Per layer, the VFS paths of the bitmaps that composite to it (the hi-res replacement
    /// names): a type's own bitmap, or the baked transition `.txm` path.
    pub tile_names: Vec<Vec<String>>,
    /// One word per tile (see [`pack_tile_word`]).
    pub tile_words: Vec<u32>,
    /// Tiles whose bitmap was not found and that fell back to the colour map.
    pub missing_tiles: Vec<String>,
}

impl TerrainTextures {
    pub fn load(game: &mut GameData, mission: &Mission) -> Result<Self> {
        let base = color_map_base_name(mission);
        let (color_map_name, color_map) = game
            .tga_first(&[
                &format!("data/textures/{base}.burnin.tga"),
                &format!("data/textures/{base}.tga"),
            ])?
            .ok_or_else(|| crate::RenderError::Missing(format!("colour map for {base}")))?;
        let (detail_name, detail) = game
            .tga_first(&[
                &format!("data/textures/{base}.detail.tga"),
                "data/textures/defaults/default_detail.tga",
            ])?
            .ok_or_else(|| crate::RenderError::Missing("detail texture".into()))?;
        let (water_name, water) = game
            .tga_first(&[
                &format!("data/textures/{base}.water.tga"),
                "data/textures/defaults/default_water.tga",
            ])?
            .ok_or_else(|| crate::RenderError::Missing("water texture".into()))?;
        let mut water_detail = Vec::new();
        let mut water_detail_names = Vec::new();
        // `resetWaterDetailTextures`: frames <base>.water0000, 0001, ... until one is missing.
        for i in 0..256 {
            let name = format!("data/textures/{base}.water{i:04}.tga");
            if !game.exists(&name) {
                break;
            }
            water_detail.push(game.tga(&name)?);
            water_detail_names.push(name);
        }

        let mut compositor = Compositor::load(game)?;
        let tiles = build_tiles(game, &mission.terrain, &mut compositor)?;
        Ok(Self {
            color_map,
            color_map_name,
            detail,
            detail_name,
            water,
            water_name,
            water_detail,
            water_detail_names,
            tile_layers: tiles.layers,
            tile_names: tiles.names,
            tile_words: tiles.words,
            missing_tiles: tiles.missing,
        })
    }
}

/// `Terrain::colorMapName`: the optional `[ColorMap] ColorMapName` of the fit, else the mission.
fn color_map_base_name(mission: &Mission) -> String {
    mission
        .color_map_name
        .clone()
        .unwrap_or_else(|| mission.name.clone())
}

/// Atlas layers, per-tile words and the names of tiles that could not be found.
struct TileSet {
    layers: Vec<Vec<u8>>,
    names: Vec<Vec<String>>,
    words: Vec<u32>,
    missing: Vec<String>,
}

fn build_tiles(
    game: &mut GameData,
    terrain: &TerrainMap,
    compositor: &mut Compositor,
) -> Result<TileSet> {
    let tiles = terrain.tiles();
    let mut layers: Vec<Vec<u8>> = Vec::new();
    let mut names: Vec<Vec<String>> = Vec::new();
    // identical bitmaps (different type/overlay codes that composite to the same pixels) share a layer
    let mut by_content: HashMap<Vec<u8>, u32> = HashMap::new();
    let mut layer_of: HashMap<TileKey, Option<u32>> = HashMap::new();
    let mut words = Vec::with_capacity(tiles * tiles);
    let mut missing = Vec::new();
    for row in 0..tiles {
        for col in 0..tiles {
            let info: TileInfo = terrain.tile_info(col, row);
            if info.kind == TileKind::ColorMap {
                words.push(pack_tile_word(0xffff, TileKind::ColorMap));
                continue;
            }
            let key = if info.is_uniform() {
                TileKey::Base(info.type_info & 0xff)
            } else {
                TileKey::Transition(info.type_info, info.overlay)
            };
            let layer = match layer_of.get(&key) {
                Some(l) => *l,
                None => {
                    let loaded = load_tile(game, compositor, &key, &info)?;
                    let l = match loaded {
                        Some(px) => {
                            let l = *by_content.entry(px.clone()).or_insert_with(|| {
                                layers.push(px);
                                names.push(Vec::new());
                                layers.len() as u32 - 1
                            });
                            if let Some(n) = tile_name(compositor, &key, &info) {
                                if !names[l as usize].contains(&n) {
                                    names[l as usize].push(n);
                                }
                            }
                            Some(l)
                        }
                        None => {
                            missing.push(match &key {
                                TileKey::Base(t) => format!("type {t}"),
                                TileKey::Transition(..) => info.transition_file(),
                            });
                            None
                        }
                    };
                    layer_of.insert(key, l);
                    l
                }
            };
            words.push(match layer {
                Some(l) => pack_tile_word(l, info.kind),
                None => pack_tile_word(0xffff, TileKind::ColorMap),
            });
        }
    }
    Ok(TileSet {
        layers,
        names,
        words,
        missing,
    })
}

/// The VFS path a tile layer is known by for the hi-res replacement lookup.
fn tile_name(compositor: &Compositor, key: &TileKey, info: &TileInfo) -> Option<String> {
    match key {
        TileKey::Base(t) => compositor.types.bitmap_path(*t),
        TileKey::Transition(..) => Some(format!("data/textures/{}", info.transition_file())),
    }
}

fn load_tile(
    game: &mut GameData,
    compositor: &mut Compositor,
    key: &TileKey,
    info: &TileInfo,
) -> Result<Option<Vec<u8>>> {
    match key {
        TileKey::Base(t) => {
            let Some(path) = compositor.types.bitmap_path(*t) else {
                return Ok(None);
            };
            if !game.exists(&path) {
                return Ok(None);
            }
            Ok(Some(to_tile(&game.tga(&path)?)))
        }
        TileKey::Transition(..) => {
            // The baked tile if the data ships one, else composite it like the game does.
            let path = format!("data/textures/{}", info.transition_file());
            match game.read_opt(&path)? {
                Some(bytes) => match decode_txm(&bytes) {
                    Ok(px) => Ok(Some(px)),
                    // an unreadable baked tile is not fatal: make it like the game would
                    Err(_) => Ok(Some(compositor.transition(
                        game,
                        info.type_info,
                        info.overlay,
                    )?)),
                },
                None => Ok(Some(compositor.transition(
                    game,
                    info.type_info,
                    info.overlay,
                )?)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txm_round_trips_through_the_game_codec() {
        // BGRA source -> LZ -> decode_txm -> RGBA
        let mut bgra = vec![0u8; TILE_BYTES];
        for (i, px) in bgra.chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, 10, 200, (i % 7) as u8]);
        }
        let packed = mc2_formats::lzw::compress(&bgra);
        let rgba = decode_txm(&packed).unwrap();
        assert_eq!(rgba.len(), TILE_BYTES);
        assert_eq!(rgba[0..4], [200, 10, 0, 0]);
        assert_eq!(rgba[4..8], [200, 10, 1, 1]);
        // a stream that decodes to the wrong size is rejected
        let short = mc2_formats::lzw::compress(&[1, 2, 3, 4]);
        assert!(decode_txm(&short).is_err());
        // the Linux build's zlib form decodes to the same pixels
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&bgra).unwrap();
        assert_eq!(decode_txm(&z.finish().unwrap()).unwrap(), rgba);
    }
}
