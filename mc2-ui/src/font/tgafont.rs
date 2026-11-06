// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `.tga` grid fonts, following the rules in the comment above `gos_LoadFont` in the public
//! `GameOS/include/gameos.hpp`:
//!
//! * characters sit on a regular grid that starts at the top-left (at `start_line`) and spans the
//!   texture width; rows continue directly below;
//! * the first cell holds a one-pixel box the size of the grid cell (that is how the grid size is
//!   measured); the second cell is `!` (33) and the rest follow in code order up to
//!   `char_count - 1`;
//! * letters must not use the right column or bottom row of their cell;
//! * a proportional font need not be centred: the leading blank columns and the width of each
//!   character are measured from the pixels.
//!
//! Choices the header leaves open (porting log): a pixel is ink when its alpha is non-zero and, for
//! an image without any translucent pixel, when it is not black; a proportional character advances
//! by its ink width plus one pixel; the space (whose cell is the box) advances by half a cell.

use super::{Font, FontFormat, Glyph};

pub fn decode(tga: &[u8], start_line: u32, char_count: u32) -> Result<Font, String> {
    let img = mc2_assets::tga::decode(tga).map_err(|e| e.to_string())?;
    let (w, h) = (u32::from(img.width), u32::from(img.height));
    let px = &img.pixels;
    let has_alpha = px.chunks_exact(4).any(|p| p[3] != 255);
    let ink = |x: u32, y: u32| -> u8 {
        if x >= w || y >= h {
            return 0;
        }
        let p = &px[((y * w + x) * 4) as usize..][..4];
        if has_alpha {
            p[3]
        } else {
            // coverage from brightness for opaque images
            (u32::from(p[0]).max(u32::from(p[1])).max(u32::from(p[2]))) as u8
        }
    };
    let y0 = start_line;
    let gw = (0..w).take_while(|&x| ink(x, y0) != 0).count() as u32;
    let gh = (y0..h).take_while(|&y| ink(0, y) != 0).count() as u32;
    if gw < 2 || gh < 2 {
        return Err("tga font: no grid box in the first cell".into());
    }
    let across = w / gw;
    let mut glyphs = vec![Glyph::default(); 256];
    let mut atlas = vec![0u8; (w * h * 4) as usize];
    for (i, a) in atlas.chunks_exact_mut(4).enumerate() {
        let (x, y) = (i as u32 % w, i as u32 / w);
        a.copy_from_slice(&[255, 255, 255, ink(x, y)]);
    }
    for c in 33..char_count.min(256) {
        let cell = c - 32;
        let (cx, cy) = ((cell % across) * gw, y0 + (cell / across) * gh);
        if cy + gh > h {
            break;
        }
        // letters use the cell minus its right column and bottom row
        let cols: Vec<u32> = (0..gw - 1)
            .filter(|&x| (0..gh - 1).any(|y| ink(cx + x, cy + y) != 0))
            .collect();
        let (blank, used) = match (cols.first(), cols.last()) {
            (Some(&a), Some(&b)) => (a, b - a + 1),
            _ => (0, 0),
        };
        glyphs[c as usize] = Glyph {
            present: used > 0,
            x: (cx + blank) as u16,
            y: cy as u16,
            w: used as u16,
            h: (gh - 1) as u16,
            dx: 0,
            dy: 0,
            advance: if used > 0 {
                used as i16 + 1
            } else {
                (gw / 2) as i16
            },
        };
    }
    glyphs[32] = Glyph {
        present: false,
        advance: (gw / 2) as i16,
        ..Glyph::default()
    };
    Ok(Font {
        name: String::new(),
        format: FontFormat::TgaGrid,
        line_height: gh,
        ascent: gh - 1,
        glyphs,
        atlas_width: w,
        atlas_height: h,
        atlas,
        texture: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 32-bit uncompressed top-down TGA.
    pub(crate) fn tga(w: u16, h: u16, rgba: &[u8]) -> Vec<u8> {
        let mut d = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        d.extend_from_slice(&w.to_le_bytes());
        d.extend_from_slice(&h.to_le_bytes());
        d.push(32);
        d.push(0x28); // top-down, 8 alpha bits
        for p in rgba.chunks_exact(4) {
            d.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
        d
    }

    #[test]
    fn measures_grid_and_proportional_widths() {
        // 4 cells of 6x5 across a 24x5 texture: box, '!', '"', '#'
        let (w, h) = (24u32, 5u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        let mut set = |x: u32, y: u32| px[((y * w + x) * 4 + 3) as usize] = 255;
        for x in 0..6 {
            set(x, 0);
            set(x, 4);
        }
        for y in 0..5 {
            set(0, y);
            set(5, y);
        }
        // '!' : one column at x=2 of cell 1
        for y in 0..3 {
            set(6 + 2, y);
        }
        // '"' : columns 1 and 3 of cell 2
        set(12 + 1, 0);
        set(12 + 3, 0);
        let f = decode(&tga(w as u16, h as u16, &px), 0, 35).unwrap();
        assert_eq!(f.line_height, 5);
        let ex = f.glyph(b'!');
        assert_eq!((ex.x, ex.w, ex.advance), (8, 1, 2));
        let q = f.glyph(b'"');
        assert_eq!((q.x, q.w, q.advance), (13, 3, 4));
        assert_eq!(f.glyph(b' ').advance, 3);
        assert!(!f.glyph(b'#').present);
    }
}
