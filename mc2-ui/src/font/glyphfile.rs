//! `.glyph` metrics + `.bmp` atlas fonts (the form shipped in `assets/graphics/`).
//!
//! Layout derived from the twelve files in the data (all 7156 bytes), all values little-endian
//! 32-bit:
//!
//! ```text
//! header   u32 count        223      number of records
//!          u32 first        32       character code of record 0
//!          u32 cell_width   8 / 13   width of one atlas cell (also the widest advance)
//!          u32 ascent       9 / 13   top of a line to the baseline
//!          u32 line_height  11 / 14  height of one atlas cell and of one text line
//! records  count x 8 x i32:
//!          min_x max_x      ink columns [min_x, max_x) relative to the pen
//!          min_y max_y      ink rows above the baseline [min_y, max_y)
//!          advance          pen advance
//!          present          1, or 0 for a character the font lacks (all-zero record)
//!          cell_x cell_y    top-left of the character's cell in the atlas
//! ```
//!
//! Each cell is `cell_width x line_height` pixels and holds the character exactly as it is drawn
//! with the pen at the cell's top-left (verified for every glyph: ink row `r` of a cell is
//! `ascent - y` for ink at height `y`). Cells are packed 32 to a row, skipping absent characters.
//! The atlas is a 32-bit bottom-up `BI_BITFIELDS` bitmap, white where there is ink, alpha 0 or 255.

use super::{bmp, Font, FontFormat, Glyph};

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphHeader {
    pub count: u32,
    pub first: u32,
    pub cell_width: u32,
    pub ascent: u32,
    pub line_height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphRecord {
    pub min_x: i32,
    pub max_x: i32,
    pub min_y: i32,
    pub max_y: i32,
    pub advance: i32,
    pub present: bool,
    pub cell_x: i32,
    pub cell_y: i32,
}

pub fn parse(d: &[u8]) -> Result<(GlyphHeader, Vec<GlyphRecord>), String> {
    let h = |i: usize| u32_at(d, i * 4).ok_or_else(|| "glyph header truncated".to_string());
    let header = GlyphHeader {
        count: h(0)?,
        first: h(1)?,
        cell_width: h(2)?,
        ascent: h(3)?,
        line_height: h(4)?,
    };
    if header.count > 256 || header.first + header.count > 256 {
        return Err(format!(
            "glyph range {}+{} exceeds 256",
            header.first, header.count
        ));
    }
    if header.cell_width == 0 || header.line_height == 0 || header.line_height > 256 {
        return Err("bad glyph cell size".into());
    }
    let need = 20 + header.count as usize * 32;
    if d.len() < need {
        return Err(format!("glyph file is {} bytes, need {need}", d.len()));
    }
    let records = (0..header.count as usize)
        .map(|i| {
            let o = 20 + i * 32;
            let v = |k: usize| u32_at(d, o + k * 4).unwrap_or(0) as i32;
            GlyphRecord {
                min_x: v(0),
                max_x: v(1),
                min_y: v(2),
                max_y: v(3),
                advance: v(4),
                present: v(5) != 0,
                cell_x: v(6),
                cell_y: v(7),
            }
        })
        .collect();
    Ok((header, records))
}

pub fn decode(glyph: &[u8], bmp_bytes: &[u8]) -> Result<Font, String> {
    let (h, records) = parse(glyph)?;
    let atlas = bmp::decode(bmp_bytes)?;
    let mut glyphs = vec![Glyph::default(); 256];
    for (i, r) in records.iter().enumerate() {
        if !r.present {
            continue;
        }
        if r.cell_x < 0 || r.cell_y < 0 {
            return Err(format!(
                "glyph {} has a negative cell",
                h.first as usize + i
            ));
        }
        // clip the cell to the atlas
        let x = r.cell_x as u32;
        let y = r.cell_y as u32;
        let w = h.cell_width.min(atlas.width.saturating_sub(x));
        let hh = h.line_height.min(atlas.height.saturating_sub(y));
        glyphs[h.first as usize + i] = Glyph {
            present: true,
            x: x as u16,
            y: y as u16,
            w: w as u16,
            h: hh as u16,
            dx: 0,
            dy: 0,
            advance: r.advance as i16,
        };
    }
    Ok(Font {
        name: String::new(),
        format: FontFormat::GlyphBmp,
        line_height: h.line_height,
        ascent: h.ascent,
        glyphs,
        atlas_width: atlas.width,
        atlas_height: atlas.height,
        // force white so the vertex colour alone sets the text colour
        atlas: atlas
            .rgba
            .chunks_exact(4)
            .flat_map(|p| [255, 255, 255, p[3]])
            .collect(),
        texture: None,
    })
}

/// Builds a `.glyph` file (tests).
pub fn encode(h: &GlyphHeader, records: &[GlyphRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    for v in [h.count, h.first, h.cell_width, h.ascent, h.line_height] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for r in records {
        for v in [
            r.min_x,
            r.max_x,
            r.min_y,
            r.max_y,
            r.advance,
            r.present as i32,
            r.cell_x,
            r.cell_y,
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_two_glyph_font() {
        let h = GlyphHeader {
            count: 2,
            first: 65,
            cell_width: 4,
            ascent: 3,
            line_height: 4,
        };
        let recs = [
            GlyphRecord {
                min_x: 0,
                max_x: 3,
                min_y: 0,
                max_y: 3,
                advance: 4,
                present: true,
                cell_x: 0,
                cell_y: 0,
            },
            GlyphRecord {
                min_x: 0,
                max_x: 0,
                min_y: 0,
                max_y: 0,
                advance: 0,
                present: false,
                cell_x: 0,
                cell_y: 0,
            },
        ];
        let mut px = vec![0u8; 8 * 4 * 4];
        px[3] = 255; // (0,0) inked
        let f = decode(&encode(&h, &recs), &bmp::encode_rgba32(8, 4, &px)).unwrap();
        assert_eq!(f.line_height, 4);
        assert_eq!(f.glyph(b'A').advance, 4);
        assert!(f.glyph(b'A').present && !f.glyph(b'B').present);
        assert_eq!(f.line_width("AAB"), 8);
        assert_eq!(&f.atlas[0..4], &[255, 255, 255, 255]);
        assert_eq!(f.atlas[7], 0);
        assert!(decode(&[0; 10], &[]).is_err());
    }
}
