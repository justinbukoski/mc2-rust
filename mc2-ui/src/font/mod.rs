// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Game fonts (`gos_LoadFont`, `aFont`).
//!
//! The GUI names fonts by resource string (`aFont::loadFont`: `cLoadString(id)` gives e.g.
//! `"AgencyFB17.d3f"`, an optional `",size"` suffix sets the scale) and loads
//! `assets/graphics/<name>` lower-cased. Three on-disk forms are decoded, all written fresh (GameOS'
//! font code is not available; alariq's is platform code and is not used):
//!
//! * `.glyph` + `.bmp` ([`glyphfile`]): the form present in the available game data
//!   (`assets/graphics/*.glyph`); the layout was derived from the files and is documented in
//!   `rust/docs/porting-log.md`.
//! * `.tga` grid fonts ([`tgafont`]): the rules in the comment above `gos_LoadFont` in the public
//!   `GameOS/include/gameos.hpp` header.
//! * `.d3f` ([`d3f`]): the `D3DFontData` layout declared in the public
//!   `GameOS/include/font3d.hpp` header. No `.d3f` file is in the available data, so this reader is
//!   tested on synthetic files only.
//!
//! A name is resolved by trying the named file first, then the same stem with the other
//! extensions (`.glyph`, `.d3f`, `.tga`), so `"AgencyFB17.d3f"` finds `agencyfb17.glyph`.

pub mod bmp;
pub mod d3f;
pub mod glyphfile;
pub mod text;
pub mod tgafont;

use crate::strings::StringTable;
use crate::textures::{TexId, TextureStore};
use mc2_formats::vfs::Vfs;
use std::collections::HashMap;

/// Where fonts live (`"assets" PATH_SEPARATOR "graphics" PATH_SEPARATOR`).
pub const FONT_PATH: &str = "assets/graphics/";

/// One character of a font: its atlas rectangle and placement relative to the pen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Glyph {
    pub present: bool,
    /// Atlas rectangle in pixels.
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Offset of the rectangle's top-left from the pen (pen y is the top of the text line).
    pub dx: i16,
    pub dy: i16,
    /// Pen advance in pixels.
    pub advance: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontFormat {
    GlyphBmp,
    D3f,
    TgaGrid,
}

/// A decoded bitmap font. Glyphs are indexed by code-page 1252 byte.
#[derive(Clone, Debug)]
pub struct Font {
    /// Path it was loaded from.
    pub name: String,
    pub format: FontFormat,
    /// Height of one text line in pixels (the height `gos_TextStringLength` reports per line).
    pub line_height: u32,
    /// Distance from the top of a line to the baseline.
    pub ascent: u32,
    pub glyphs: Vec<Glyph>,
    pub atlas_width: u32,
    pub atlas_height: u32,
    /// RGBA8 atlas, white with coverage in alpha.
    pub atlas: Vec<u8>,
    /// Handle of the atlas once registered in a [`TextureStore`].
    pub texture: Option<TexId>,
}

impl Font {
    pub fn glyph(&self, byte: u8) -> &Glyph {
        &self.glyphs[usize::from(byte)]
    }

    /// Advance of one character (characters 1252 cannot encode count as `?`).
    pub fn advance(&self, c: char) -> i32 {
        let b = crate::cp1252::char_to_byte(c).unwrap_or(b'?');
        i32::from(self.glyph(b).advance)
    }

    /// Width of a single line of plain text (no codes, no newlines).
    pub fn line_width(&self, s: &str) -> i32 {
        s.chars().map(|c| self.advance(c)).sum()
    }

    /// Loads a font file by full data path, trying the alternative extensions of the same stem.
    pub fn load(vfs: &mut Vfs, path: &str) -> Result<Font, String> {
        let path = path.to_ascii_lowercase();
        let stem = match path.rfind('.') {
            Some(dot) if !path[dot..].contains('/') => &path[..dot],
            _ => path.as_str(),
        };
        let mut candidates = vec![path.clone()];
        for ext in [".glyph", ".d3f", ".tga"] {
            let c = format!("{stem}{ext}");
            if !candidates.contains(&c) {
                candidates.push(c);
            }
        }
        let mut errors = Vec::new();
        for c in candidates {
            let bytes = match vfs.read(&c) {
                Ok(Some(b)) => b,
                Ok(None) => continue,
                Err(e) => {
                    errors.push(format!("{c}: {e}"));
                    continue;
                }
            };
            let result = if c.ends_with(".glyph") {
                let bmp_path = format!("{}.bmp", &c[..c.len() - 6]);
                match vfs.read(&bmp_path) {
                    Ok(Some(bmp)) => glyphfile::decode(&bytes, &bmp),
                    _ => Err(format!("{bmp_path} missing")),
                }
            } else if c.ends_with(".d3f") {
                d3f::decode(&bytes)
            } else if c.ends_with(".tga") {
                tgafont::decode(&bytes, 0, 256)
            } else {
                Err("unknown font extension".to_string())
            };
            match result {
                Ok(mut f) => {
                    f.name = c;
                    return Ok(f);
                }
                Err(e) => errors.push(format!("{c}: {e}")),
            }
        }
        Err(format!(
            "font {path} not loadable ({})",
            if errors.is_empty() {
                "no file".to_string()
            } else {
                errors.join("; ")
            }
        ))
    }
}

/// Handle of a font in a [`FontCache`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontId(pub u32);

/// A font reference as the GUI holds it (`aFont`): the resource id it came from and the size
/// (`1` = native; `aFont::loadFont` stores `-n` for a `",n"` suffix).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FontRef {
    pub id: Option<FontId>,
    pub res_id: u32,
    pub size: i32,
}

/// Loaded fonts, shared by every widget.
#[derive(Default)]
pub struct FontCache {
    fonts: Vec<Font>,
    by_path: HashMap<String, FontId>,
}

impl FontCache {
    pub fn new() -> FontCache {
        FontCache::default()
    }

    pub fn get(&self, id: FontId) -> &Font {
        &self.fonts[id.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.fonts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fonts.is_empty()
    }

    /// Adds a decoded font and registers its atlas as a texture.
    pub fn insert(&mut self, mut font: Font, textures: &mut TextureStore) -> FontId {
        if let Some(&id) = self.by_path.get(&font.name) {
            return id;
        }
        let tex = textures.insert(
            &font.name,
            font.atlas_width,
            font.atlas_height,
            font.atlas.clone(),
        );
        font.texture = Some(tex);
        let id = FontId(self.fonts.len() as u32);
        self.by_path.insert(font.name.clone(), id);
        self.fonts.push(font);
        id
    }

    /// `gos_LoadFont(path)`.
    pub fn load_path(
        &mut self,
        vfs: &mut Vfs,
        textures: &mut TextureStore,
        path: &str,
    ) -> Option<FontId> {
        let key = path.to_ascii_lowercase();
        if let Some(&id) = self.by_path.get(&key) {
            return Some(id);
        }
        match Font::load(vfs, &key) {
            Ok(f) => {
                let id = self.insert(f, textures);
                // the requested name resolves to the same font next time
                self.by_path.insert(key, id);
                Some(id)
            }
            Err(e) => {
                log::warn!("{e}");
                None
            }
        }
    }

    /// `aFont::loadFont(resourceID, size)`: the font a resource string names.
    pub fn load_resource(
        &mut self,
        vfs: &mut Vfs,
        textures: &mut TextureStore,
        strings: &StringTable,
        res_id: u32,
    ) -> FontRef {
        let (name, size) = split_font_string(strings.load(res_id));
        if name.is_empty() {
            return FontRef {
                id: None,
                res_id,
                size,
            };
        }
        let id = self.load_path(vfs, textures, &format!("{FONT_PATH}{name}"));
        FontRef { id, res_id, size }
    }
}

/// Splits `"name,size"` the way `aFont::loadFont` does: `size = -atoi(suffix)`, 1 without one.
pub fn split_font_string(s: &str) -> (String, i32) {
    match s.find(',') {
        Some(i) => {
            let digits: String = s[i + 1..]
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '+')
                .collect();
            (s[..i].to_string(), -digits.parse::<i32>().unwrap_or(0))
        }
        None => (s.to_string(), 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_strings_split_like_load_font() {
        assert_eq!(
            split_font_string("AgencyFB17.d3f"),
            ("AgencyFB17.d3f".into(), 1)
        );
        assert_eq!(split_font_string("arial.ttf,12"), ("arial.ttf".into(), -12));
    }
}
