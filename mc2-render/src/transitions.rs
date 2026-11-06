// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Road / pavement transition tiles composited from the terrain-type bitmaps, masks and overlays.
//!
//! Ported from `TerrainTextures::init` (`terrtxm.cpp`: the `textures.fit` tables, the order of the
//! runtime texture slots), `TerrainTextures::createTransition` (priority sort, mask selection),
//! `combineTxm` and `combineOverlayTxm`.  Only the colour-map ("new terrain") branch is ported:
//! the base starts as a transparent `0x01` fill unless the lowest-priority type is a cement type,
//! and only cement types and overlays are blended in.
//!
//! The shipped data carries the composited result for the missions the artists baked
//! (`<typeInfo>.<overlay>.txm`); for the other missions the game composites at load time, which is
//! what [`Compositor`] does.  A test compares both for every baked tile of `mc2_01`.

use crate::error::Result;
use crate::game::GameData;
use crate::terrain::is_cement_type;
use crate::terrain_tex::{TILE_BYTES, TILE_SIZE};
use mc2_assets::tga::RgbaImage;
use mc2_formats::ini::FitIni;
use std::collections::HashMap;

/// `MaxTerrainTypes` slots, each of which owns one runtime texture slot.
const DEFAULT_TYPE_SLOTS: u32 = 255;

/// `data/textures/textures.fit` terrain types: bitmap name, blend priority and mask set.
#[derive(Debug, Default)]
pub struct TerrainTypeTable {
    names: Vec<Option<String>>,
    priority: Vec<i32>,
    masks: Vec<Option<String>>,
    /// `MaxTerrainTypes`.
    pub slots: u32,
}

impl TerrainTypeTable {
    pub fn parse(fit: &mut FitIni) -> Self {
        let slots = if fit.seek_block("Main").is_ok() {
            fit.read_i32("MaxTerrainTypes")
                .map(|v| v.clamp(0, 255) as u32)
                .unwrap_or(DEFAULT_TYPE_SLOTS)
        } else {
            DEFAULT_TYPE_SLOTS
        };
        let mut t = TerrainTypeTable {
            slots,
            ..Default::default()
        };
        for n in 0..slots {
            if fit.seek_block(&format!("TerrainType{n}")).is_ok() {
                t.names.push(fit.read_string("TerrainName", 255).ok());
                t.priority
                    .push(fit.read_i32("TerrainPriority").unwrap_or(0));
                t.masks.push(fit.read_string("MaskName", 255).ok());
            } else {
                // `TerrainTextures::init`: blank types copy type 0's priority.
                t.names.push(None);
                t.priority.push(t.priority.first().copied().unwrap_or(0));
                t.masks.push(None);
            }
        }
        t
    }

    pub fn load(game: &mut GameData) -> Result<Self> {
        let mut fit = game.fit("data/textures/textures.fit")?;
        Ok(Self::parse(&mut fit))
    }

    pub fn name(&self, ty: u32) -> Option<&str> {
        self.names.get(ty as usize)?.as_deref()
    }

    pub fn priority(&self, ty: u32) -> i32 {
        self.priority.get(ty as usize).copied().unwrap_or(0)
    }

    pub fn mask_name(&self, ty: u32) -> Option<&str> {
        self.masks.get(ty as usize)?.as_deref()
    }

    /// Path of the bitmap `initTexture` loads for a type (`<texturePath>64/<TerrainName>.tga`).
    pub fn bitmap_path(&self, ty: u32) -> Option<String> {
        self.name(ty).map(|n| format!("data/textures/64/{n}.tga"))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OverlayEntry {
    pub name: String,
    pub count: u32,
    /// Runtime texture slot of the first bitmap (`baseTXMIndex`).
    pub base: u32,
}

/// `OverlayTypeN` blocks: road, runway and bridge overlays; their slots follow the type slots.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayTable {
    pub entries: Vec<OverlayEntry>,
}

impl OverlayTable {
    pub fn parse(fit: &mut FitIni, first_slot: u32) -> Self {
        let n = if fit.seek_block("Main").is_ok() {
            fit.read_i32("MaxTerrainOverlays").unwrap_or(0).max(0)
        } else {
            0
        };
        let mut entries = Vec::new();
        let mut base = first_slot;
        for i in 0..n {
            if fit.seek_block(&format!("OverlayType{i}")).is_err() {
                break;
            }
            let name = fit.read_string("OverlayName", 255).unwrap_or_default();
            let count = fit.read_i32("NumOverlays").unwrap_or(0).max(0) as u32;
            entries.push(OverlayEntry { name, count, base });
            base += count;
        }
        OverlayTable { entries }
    }

    /// `(entry index, offset)` of a runtime texture slot, or `None` below the first overlay.
    pub fn locate(&self, slot: u32) -> Option<(usize, u32)> {
        self.entries
            .iter()
            .enumerate()
            .find(|(_, e)| slot >= e.base && slot < e.base + e.count)
            .map(|(i, e)| (i, slot - e.base))
    }

    pub fn bitmap_path(&self, entry: usize, offset: u32) -> String {
        format!(
            "data/textures/64overlays/{}{:04}.tga",
            self.entries[entry].name, offset
        )
    }
}

/// Resamples an RGBA image to the tile size (nearest) if it is not already 64x64.
pub fn to_tile(img: &RgbaImage) -> Vec<u8> {
    if img.width as u32 == TILE_SIZE && img.height as u32 == TILE_SIZE {
        return img.pixels.clone();
    }
    let mut out = vec![0u8; TILE_BYTES];
    for y in 0..TILE_SIZE as usize {
        for x in 0..TILE_SIZE as usize {
            let sx = x * img.width as usize / TILE_SIZE as usize;
            let sy = y * img.height as usize / TILE_SIZE as usize;
            let s = (sy * img.width as usize + sx) * 4;
            let d = (y * TILE_SIZE as usize + x) * 4;
            out[d..d + 4].copy_from_slice(&img.pixels[s..s + 4]);
        }
    }
    out
}

/// `combineTxm`: blends `src` over `dest` where the 8-bit `mask` is set; the mask also raises the
/// alpha channel.  Arithmetic follows the C++ (float blend, truncating cast).
pub fn combine_mask(dest: &mut [u8], src: &[u8], mask: &[u8]) {
    for (i, &m) in mask.iter().enumerate().take(dest.len() / 4) {
        if m == 0 {
            continue;
        }
        let comb = m as f32 / 255.0;
        let rest = 1.0 - comb;
        let p = i * 4;
        if comb == 1.0 {
            dest[p..p + 3].copy_from_slice(&src[p..p + 3]);
        } else {
            for c in 0..3 {
                let mut v = comb * src[p + c] as f32;
                v += rest * dest[p + c] as f32;
                dest[p + c] = v as u8;
            }
        }
        if dest[p + 3] < m {
            dest[p + 3] = m;
        }
    }
}

/// `combineOverlayTxm`: like [`combine_mask`] with the overlay's own alpha as the mask.
pub fn combine_overlay(dest: &mut [u8], overlay: &[u8]) {
    for p in (0..dest.len().min(overlay.len())).step_by(4) {
        let a = overlay[p + 3];
        if a == 0 {
            continue;
        }
        let comb = a as f32 / 255.0;
        let rest = 1.0 - comb;
        if comb == 1.0 {
            dest[p..p + 3].copy_from_slice(&overlay[p..p + 3]);
        } else {
            for c in 0..3 {
                let mut v = comb * overlay[p + c] as f32;
                v += rest * dest[p + c] as f32;
                dest[p + c] = v as u8;
            }
        }
        if dest[p + 3] < a {
            dest[p + 3] = a;
        }
    }
}

/// `forceAlphaOpaque`.
pub fn force_alpha_opaque(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        p[3] = 255;
    }
}

/// The blend order of `createTransition`: terrain types sorted by ascending priority (the
/// C++'s double loop, which swaps whenever a later entry has strictly lower priority).
pub fn priority_order(types: [u8; 4], priority: impl Fn(u8) -> i32) -> [u8; 4] {
    let mut pri = types;
    for i in 0..4 {
        for j in i..4 {
            if priority(pri[j]) < priority(pri[i]) {
                pri.swap(i, j);
            }
        }
    }
    pri
}

/// The mask number of `createTransition`: one bit per tile corner that has type `ty`
/// (`v0` is the most significant).
pub fn mask_bin(types: [u8; 4], ty: u8) -> u32 {
    ((types[0] == ty) as u32) * 8
        + ((types[1] == ty) as u32) * 4
        + ((types[2] == ty) as u32) * 2
        + (types[3] == ty) as u32
}

pub struct Compositor {
    pub types: TerrainTypeTable,
    pub overlays: OverlayTable,
    bitmaps: HashMap<u32, Vec<u8>>,
    masks: HashMap<(String, u32), Vec<u8>>,
    overlay_bitmaps: HashMap<(usize, u32), Vec<u8>>,
}

impl Compositor {
    pub fn load(game: &mut GameData) -> Result<Self> {
        let mut fit = game.fit("data/textures/textures.fit")?;
        let types = TerrainTypeTable::parse(&mut fit);
        let overlays = OverlayTable::parse(&mut fit, types.slots);
        Ok(Self::new(types, overlays))
    }

    pub fn new(types: TerrainTypeTable, overlays: OverlayTable) -> Self {
        Self {
            types,
            overlays,
            bitmaps: HashMap::new(),
            masks: HashMap::new(),
            overlay_bitmaps: HashMap::new(),
        }
    }

    fn bitmap(&mut self, game: &mut GameData, ty: u32) -> Result<Vec<u8>> {
        if let Some(b) = self.bitmaps.get(&ty) {
            return Ok(b.clone());
        }
        let px = match self.types.bitmap_path(ty) {
            Some(path) if game.exists(&path) => to_tile(&game.tga(&path)?),
            // blank placeholder types have no bitmap
            _ => vec![0u8; TILE_BYTES],
        };
        self.bitmaps.insert(ty, px.clone());
        Ok(px)
    }

    fn mask(&mut self, game: &mut GameData, ty: u32, bin: u32) -> Result<Option<Vec<u8>>> {
        let Some(set) = self.types.mask_name(ty).map(str::to_string) else {
            return Ok(None);
        };
        let key = (set.to_ascii_lowercase(), bin - 1);
        if let Some(m) = self.masks.get(&key) {
            return Ok(Some(m.clone()));
        }
        let path = format!("data/textures/64mask/{}{:04}.tga", set, bin - 1);
        if !game.exists(&path) {
            return Ok(None);
        }
        let img = game.tga(&path)?;
        let tile = to_tile(&img);
        // grayscale decodes to r = g = b = value; the mask is the red channel
        let mask: Vec<u8> = tile.chunks_exact(4).map(|p| p[0]).collect();
        self.masks.insert(key, mask.clone());
        Ok(Some(mask))
    }

    fn overlay(&mut self, game: &mut GameData, slot: u32) -> Result<Option<Vec<u8>>> {
        let Some((entry, offset)) = self.overlays.locate(slot) else {
            return Ok(None);
        };
        if let Some(b) = self.overlay_bitmaps.get(&(entry, offset)) {
            return Ok(Some(b.clone()));
        }
        let path = self.overlays.bitmap_path(entry, offset);
        if !game.exists(&path) {
            return Ok(None);
        }
        let px = to_tile(&game.tga(&path)?);
        self.overlay_bitmaps.insert((entry, offset), px.clone());
        Ok(Some(px))
    }

    /// `createTransition` for the colour-map branch.  `overlay` is the runtime texture slot
    /// (`0xffff` = none).  Returns 64x64 RGBA.
    pub fn transition(
        &mut self,
        game: &mut GameData,
        type_info: u32,
        overlay: u32,
    ) -> Result<Vec<u8>> {
        let v = [
            (type_info & 0xff) as u8,
            ((type_info >> 8) & 0xff) as u8,
            ((type_info >> 16) & 0xff) as u8,
            (type_info >> 24) as u8,
        ];
        let order = priority_order(v, |t| self.types.priority(t as u32));
        let base = self.bitmap(game, order[0] as u32)?;
        let mut out = if is_cement_type(order[0] as u32) {
            let mut o = base;
            force_alpha_opaque(&mut o);
            o
        } else {
            // transparent fill: every byte 0x01
            vec![0x01u8; TILE_BYTES]
        };
        for k in 1..4 {
            if order[k - 1] == order[k] {
                continue;
            }
            let ty = order[k];
            let bin = mask_bin(v, ty);
            if !is_cement_type(ty as u32) || bin == 0 {
                continue;
            }
            let src = self.bitmap(game, ty as u32)?;
            if let Some(mask) = self.mask(game, ty as u32, bin)? {
                combine_mask(&mut out, &src, &mask);
            }
        }
        // Overlays 285..=292 are the rough-ground decals the shipped game does not draw.
        if overlay != 0xffff && !(285..=292).contains(&overlay) {
            if let Some(ov) = self.overlay(game, overlay)? {
                combine_overlay(&mut out, &ov);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_blend_matches_the_c_arithmetic() {
        let mut dest = vec![100, 100, 100, 1, 10, 20, 30, 1];
        let src = vec![200, 0, 255, 255, 50, 50, 50, 255];
        // pixel 0: half mask; pixel 1: full mask
        combine_mask(&mut dest, &src, &[128, 255]);
        let p = 128.0f32 / 255.0;
        assert_eq!(dest[0], (p * 200.0 + (1.0 - p) * 100.0) as u8);
        assert_eq!(dest[3], 128, "alpha raised to the mask");
        assert_eq!(&dest[4..8], &[50, 50, 50, 255]);
        // an empty mask leaves everything alone
        let before = dest.clone();
        combine_mask(&mut dest, &src, &[0, 0]);
        assert_eq!(dest, before);
        // alpha never decreases
        let mut d = vec![0, 0, 0, 200];
        combine_mask(&mut d, &[9, 9, 9, 255], &[50]);
        assert_eq!(d[3], 200);
    }

    #[test]
    fn overlay_blend_uses_its_own_alpha() {
        let mut dest = vec![0, 0, 0, 255, 0, 0, 0, 255, 7, 7, 7, 255];
        let ov = vec![255, 255, 255, 255, 255, 255, 255, 0, 100, 100, 100, 128];
        combine_overlay(&mut dest, &ov);
        assert_eq!(&dest[0..4], &[255, 255, 255, 255]);
        assert_eq!(
            &dest[4..8],
            &[0, 0, 0, 255],
            "transparent overlay texels do nothing"
        );
        let p = 128.0f32 / 255.0;
        assert_eq!(dest[8], (p * 100.0 + (1.0 - p) * 7.0) as u8);
    }

    #[test]
    fn priority_sort_matches_the_double_loop() {
        // priorities: type 1 -> 60, 13 -> 152, 14 -> 153, 2 -> 50
        let pr = |t: u8| match t {
            1 => 60,
            13 => 152,
            14 => 153,
            2 => 50,
            _ => 0,
        };
        assert_eq!(priority_order([13, 1, 14, 2], pr), [2, 1, 13, 14]);
        // ties keep the original relative order
        assert_eq!(priority_order([5, 6, 7, 8], |_| 0), [5, 6, 7, 8]);
    }

    #[test]
    fn mask_bin_is_a_bit_per_corner() {
        assert_eq!(mask_bin([13, 1, 1, 1], 13), 8);
        assert_eq!(mask_bin([1, 1, 1, 13], 13), 1);
        assert_eq!(mask_bin([13, 13, 1, 1], 13), 12);
        assert_eq!(mask_bin([13, 13, 13, 13], 13), 15);
        assert_eq!(mask_bin([13, 13, 13, 13], 1), 0);
    }

    #[test]
    fn overlay_slots_follow_the_type_slots() {
        let text = "FITini\r\n[Main]\r\nl MaxTerrainOverlays = 3\r\n[OverlayType0]\r\nst OverlayName = \"dirtroad\"\r\nl NumOverlays = 15\r\n[OverlayType1]\r\nst OverlayName = \"pavedRoad\"\r\nl NumOverlays = 15\r\n[OverlayType2]\r\nst OverlayName = \"rough\"\r\nl NumOverlays = 8\r\nFITend\r\n";
        let mut fit = FitIni::from_bytes(text.as_bytes().to_vec()).unwrap();
        let t = OverlayTable::parse(&mut fit, 255);
        assert_eq!(t.entries[1].base, 270);
        assert_eq!(t.entries[2].base, 285);
        assert_eq!(t.locate(255), Some((0, 0)));
        assert_eq!(t.locate(284), Some((1, 14)));
        assert_eq!(t.locate(292), Some((2, 7)));
        assert_eq!(t.locate(293), None);
        assert_eq!(t.locate(100), None);
        assert_eq!(
            t.bitmap_path(1, 3),
            "data/textures/64overlays/pavedRoad0003.tga"
        );
    }

    #[test]
    fn type_table_reads_priority_and_masks() {
        let text = "FITini\r\n[Main]\r\nl MaxTerrainTypes = 4\r\n[TerrainType0]\r\nst TerrainName = \"water_ocean\"\r\nl TerrainPriority = 0\r\nst MaskName = \"mask00\"\r\n[TerrainType1]\r\nst TerrainName = \"Undefined\"\r\nl TerrainPriority = 60\r\nst MaskName = \"mask06\"\r\n[TerrainType2]\r\nst TerrainName = \"cement_2\"\r\nl TerrainPriority = 153\r\nst MaskName = \"MASK01\"\r\nFITend\r\n";
        let mut fit = FitIni::from_bytes(text.as_bytes().to_vec()).unwrap();
        let t = TerrainTypeTable::parse(&mut fit);
        assert_eq!(t.slots, 4);
        assert_eq!(t.priority(1), 60);
        assert_eq!(t.priority(2), 153);
        assert_eq!(t.mask_name(2), Some("MASK01"));
        assert_eq!(
            t.bitmap_path(2).as_deref(),
            Some("data/textures/64/cement_2.tga")
        );
        // block 3 is missing: blank type with type 0's priority
        assert_eq!(t.name(3), None);
        assert_eq!(t.priority(3), 0);
    }
}
