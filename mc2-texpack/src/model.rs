//! The manifest data model shared by the dump (`manifest.json` next to the originals) and by
//! upscaled packs (`manifest.json` next to the replacement PNGs).
//!
//! The *key* of a texture is the replacement hook's own key (`mc2_gfx::texture::texture_key`
//! with the VFS path as the name): `n_<sanitised path>_<w>x<h>`.  A pack is a flat folder of
//! `<key>.png`, exactly what `mc2_gfx::texture::load_replacement` reads, plus `manifest.json`.

use serde::{Deserialize, Serialize};

/// `format` field of both manifests.
pub const DUMP_FORMAT: &str = "mc2-texpack-dump/1";
pub const PACK_FORMAT: &str = "mc2-texpack-pack/1";

/// Largest side a replacement may have: wgpu's default `max_texture_dimension_2d`, which
/// `mc2_render::gpu::Gpu` keeps (bigger replacements are ignored by the game, not an error).
pub const MAX_DIMENSION: u32 = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Mission colour map (`<mission>.burnin.tga`), the big picture under the terrain.
    ColorMap,
    /// `*.detail.tga`, tiled over the terrain.
    Detail,
    /// `*.water.tga`.
    Water,
    /// `*.waterNNNN.tga`, the animated water detail frames.
    WaterDetail,
    /// A 64x64 terrain tile (a type's bitmap or a baked/composited transition).
    TerrainTile,
    /// Masks and overlays the transition compositor reads; never drawn themselves.
    TerrainInput,
    /// Object, building and mech textures referenced by the `.tgl` shapes.
    Shape,
    /// Textures named by the gosFX effect library.
    Effect,
    /// Interface art (`data/art`).
    Ui,
    Cursor,
    Insignia,
    /// Bitmap font atlases (`assets/graphics/*.bmp`).
    Font,
    /// Images no known path reaches (kept in the inventory, never upscaled).
    Other,
}

impl Category {
    pub const ALL: [Category; 13] = [
        Category::ColorMap,
        Category::Detail,
        Category::Water,
        Category::WaterDetail,
        Category::TerrainTile,
        Category::TerrainInput,
        Category::Shape,
        Category::Effect,
        Category::Ui,
        Category::Cursor,
        Category::Insignia,
        Category::Font,
        Category::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::ColorMap => "color_map",
            Category::Detail => "detail",
            Category::Water => "water",
            Category::WaterDetail => "water_detail",
            Category::TerrainTile => "terrain_tile",
            Category::TerrainInput => "terrain_input",
            Category::Shape => "shape",
            Category::Effect => "effect",
            Category::Ui => "ui",
            Category::Cursor => "cursor",
            Category::Insignia => "insignia",
            Category::Font => "font",
            Category::Other => "other",
        }
    }

    /// Whether the texture repeats across a surface, so the upscaler must pad with the opposite
    /// edges (wrap-around) before the network sees it.
    pub fn tileable(self) -> bool {
        matches!(
            self,
            Category::Detail | Category::Water | Category::WaterDetail
        )
    }

    /// Whether the renderer looks the texture up through the replacement hook (`mc2-render`
    /// for the terrain and shapes, `mc2-ui` for interface art, cursors and insignia, `mc2-fx`
    /// for effect textures).  The key is the VFS path in every case.
    pub fn hooked(self) -> bool {
        matches!(
            self,
            Category::ColorMap
                | Category::Detail
                | Category::Water
                | Category::WaterDetail
                | Category::TerrainTile
                | Category::Shape
                | Category::Effect
                | Category::Ui
                | Category::Cursor
                | Category::Insignia
        )
    }
}

/// How the source pixels use alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlphaClass {
    /// Every pixel opaque.
    None,
    /// Only 0 and 255 occur (a colour key / cut-out).
    Keyed,
    /// Intermediate values occur (soft edges, glows, masks).
    Smooth,
    /// Every pixel fully transparent (an unused alpha channel in the file).
    Zero,
}

/// Recommended upscale factor for the texture, honouring [`MAX_DIMENSION`].
pub fn recommended_scale(category: Category, w: u32, h: u32) -> u32 {
    let want = match category {
        Category::ColorMap => 4,
        Category::Detail | Category::Water | Category::WaterDetail => 4,
        Category::TerrainTile => 4,
        Category::Shape | Category::Effect => 4,
        Category::Ui | Category::Cursor | Category::Insignia => 4,
        Category::TerrainInput | Category::Font | Category::Other => 1,
    };
    let side = w.max(h).max(1);
    let mut s = want;
    while s > 1 && side * s > MAX_DIMENSION {
        s /= 2;
    }
    s
}

/// One texture of the dump.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Replacement key (`n_<sanitised name>_<w>x<h>`); the pack file is `<key>.png`.
    pub key: String,
    /// The name the hook is given: the lowercase VFS path of the source file.
    pub name: String,
    pub category: Category,
    pub width: u32,
    pub height: u32,
    pub alpha: AlphaClass,
    /// The renderer reads the alpha channel of this texture (shapes: only `a_*` textures).
    /// When `false` the engine forces alpha to 255, so the pack may as well store opaque pixels.
    pub alpha_used: bool,
    /// Pixels that are pure red, green or blue.  The mech paint scheme recolours exactly those,
    /// so an upscaler has to keep them pure.
    pub paint_mask_pixels: u32,
    pub tileable: bool,
    /// A loader (mc2-render, mc2-ui or mc2-fx) asks the texture hook for this key today.
    pub hooked: bool,
    pub recommended_scale: u32,
    /// FNV-1a 64 of the RGBA pixels (hex), to tell apart different textures behind one key.
    pub hash: String,
    /// More than one different image was seen under this key (should not happen).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ambiguous: bool,
    /// Extra names (identical tile bitmaps share a layer in the renderer).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    /// How many shapes / missions / effects reference the texture, and a few of them.
    pub use_count: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<String>,
    /// Path of the original PNG relative to the dump folder (empty for an inventory-only run).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
}

/// `manifest.json` of a dump.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DumpManifest {
    pub format: String,
    pub generator: String,
    pub missions: Vec<String>,
    pub entries: Vec<Entry>,
}

/// Source of the upscaling model, recorded in the pack manifest.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub licence: String,
    /// Where the weights came from (the ModelHub repo and file) and their SHA-256.
    pub source: String,
    pub sha256: String,
}

/// One replacement of a pack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackEntry {
    pub key: String,
    pub name: String,
    pub category: Category,
    pub original: [u32; 2],
    pub size: [u32; 2],
    pub scale: u32,
    pub model: String,
    /// What the pipeline did beyond the plain network pass (`wrap_pad`, `alpha_split`,
    /// `paint_mask`, `palette_match`, ...).
    #[serde(default)]
    pub steps: Vec<String>,
    /// Hash of the original this was made from (the dump's `hash`).
    pub source_hash: String,
}

/// `manifest.json` of a pack.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PackManifest {
    pub format: String,
    pub generator: String,
    pub created: String,
    pub models: Vec<ModelInfo>,
    pub entries: Vec<PackEntry>,
}
