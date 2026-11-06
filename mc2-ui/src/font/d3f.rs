//! `.d3f` fonts, from the structures declared in the public `GameOS/include/font3d.hpp`:
//!
//! * `D3DFontData` ("original FontEdit"): `dwSig` 0x46443344, `dwWidth`, `dwFontHeight`,
//!   `dwHeight`, then `dwX[256]`, `dwY[256]`, `dwWidths[256]`, `nA[256]`, `nC[256]` (32-bit each)
//!   and the pixels.
//! * `D3DFontData1` ("FontEdit 4.0+", byte packed): `dwSig`, `szFaceName[64]`, `iSize`, `bItalic`
//!   (1 byte), `iWeight`, `iTextureCount`, `dwFontHeight`, then `bTexture`, `bX`, `bY`, `bW`
//!   (bytes) and `cA`, `cC` (signed bytes), 256 each, followed by `iTextureCount` x
//!   `D3DFontTexture { dwSize; bPixels[] }`.
//!
//! The header does not say how the pixels are stored, and no `.d3f` file is in the available data.
//! The reader therefore infers the bytes per pixel from the payload size (1 = alpha, 2 = 4444,
//! 4 = 8888 ARGB) and rejects files where no square/rectangular reading fits exactly. Character
//! placement follows the field comments: `A` pixels of space before the character, its width, `C`
//! after it. Treat this module as unverified against real files.

use super::{Font, FontFormat, Glyph};

pub const SIGNATURE: u32 = 0x4644_3344;
const V0_HEADER: usize = 16 + 5 * 256 * 4;
const V1_HEADER: usize = 4 + 64 + 4 + 1 + 4 + 4 + 4 + 6 * 256;

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Converts `n` pixels of `bpp` bytes each to white-with-alpha RGBA.
fn to_rgba(px: &[u8], bpp: usize) -> Vec<u8> {
    px.chunks_exact(bpp)
        .flat_map(|p| {
            let a = match bpp {
                1 => p[0],
                2 => {
                    // ARGB 4444: alpha in the top nibble
                    let v = u16::from_le_bytes([p[0], p[1]]);
                    (((v >> 12) & 0xf) * 17) as u8
                }
                _ => p[3],
            };
            [255, 255, 255, a]
        })
        .collect()
}

fn bpp_for(len: usize, w: usize, h: usize) -> Option<usize> {
    [1usize, 2, 4].into_iter().find(|&b| w * h * b == len)
}

pub fn decode(d: &[u8]) -> Result<Font, String> {
    if u32_at(d, 0) != Some(SIGNATURE) {
        return Err("not a .d3f (signature)".into());
    }
    if let Some(f) = decode_v0(d) {
        return Ok(f);
    }
    decode_v1(d)
}

fn decode_v0(d: &[u8]) -> Option<Font> {
    let w = u32_at(d, 4)? as usize;
    let font_h = u32_at(d, 8)?;
    let h = u32_at(d, 12)? as usize;
    if d.len() < V0_HEADER || w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let bpp = bpp_for(d.len() - V0_HEADER, w, h)?;
    let arr = |k: usize, c: usize| u32_at(d, 16 + k * 1024 + c * 4).unwrap_or(0);
    let mut glyphs = vec![Glyph::default(); 256];
    for (c, g) in glyphs.iter_mut().enumerate() {
        let (x, y, gw) = (arr(0, c), arr(1, c), arr(2, c));
        let (a, cc) = (arr(3, c) as i32, arr(4, c) as i32);
        if gw == 0 && a == 0 && cc == 0 {
            continue;
        }
        *g = Glyph {
            present: true,
            x: x as u16,
            y: y as u16,
            w: gw as u16,
            h: font_h as u16,
            dx: a as i16,
            dy: 0,
            advance: (a + gw as i32 + cc) as i16,
        };
    }
    Some(Font {
        name: String::new(),
        format: FontFormat::D3f,
        line_height: font_h,
        ascent: font_h,
        glyphs,
        atlas_width: w as u32,
        atlas_height: h as u32,
        atlas: to_rgba(&d[V0_HEADER..], bpp),
        texture: None,
    })
}

fn decode_v1(d: &[u8]) -> Result<Font, String> {
    if d.len() < V1_HEADER {
        return Err(".d3f too short for either header layout".into());
    }
    let tex_count = u32_at(d, 4 + 64 + 4 + 1 + 4).unwrap_or(0) as usize;
    let font_h = u32_at(d, 4 + 64 + 4 + 1 + 4 + 4).unwrap_or(0);
    if tex_count == 0 || tex_count > 16 || font_h == 0 {
        return Err(format!(
            ".d3f: no usable layout (textures {tex_count}, height {font_h})"
        ));
    }
    let base = 4 + 64 + 4 + 1 + 4 + 4 + 4;
    let table = |k: usize, c: usize| d[base + k * 256 + c];
    // textures: dwSize then the pixels; dwSize read as the payload byte count
    let mut pos = V1_HEADER;
    let mut pages: Vec<(usize, Vec<u8>)> = Vec::new();
    for _ in 0..tex_count {
        let size = u32_at(d, pos).ok_or(".d3f texture header truncated")? as usize;
        pos += 4;
        let px = d.get(pos..pos + size).ok_or(".d3f texture truncated")?;
        pos += size;
        let (side, bpp) = [1usize, 2, 4]
            .into_iter()
            .find_map(|b| {
                let n = size / b;
                let s = (n as f64).sqrt() as usize;
                (s * s * b == size && s > 0).then_some((s, b))
            })
            .ok_or(".d3f texture is not square for 1, 2 or 4 bytes per pixel")?;
        pages.push((side, to_rgba(px, bpp)));
    }
    // stack the pages vertically into one atlas
    let width = pages.iter().map(|p| p.0).max().unwrap_or(0);
    let mut atlas = Vec::new();
    let mut page_y = Vec::new();
    let mut y = 0usize;
    for (side, rgba) in &pages {
        page_y.push(y);
        for row in 0..*side {
            atlas.extend_from_slice(&rgba[row * side * 4..(row + 1) * side * 4]);
            atlas.resize(atlas.len() + (width - side) * 4, 0);
        }
        y += side;
    }
    let mut glyphs = vec![Glyph::default(); 256];
    for (c, g) in glyphs.iter_mut().enumerate() {
        let page = usize::from(table(0, c));
        let w = table(3, c);
        let a = table(4, c) as i8;
        let cc = table(5, c) as i8;
        if w == 0 || page >= pages.len() {
            continue;
        }
        *g = Glyph {
            present: true,
            x: u16::from(table(1, c)),
            y: (page_y[page] + usize::from(table(2, c))) as u16,
            w: u16::from(w),
            h: font_h as u16,
            dx: i16::from(a),
            dy: 0,
            advance: i16::from(a) + i16::from(w) + i16::from(cc),
        };
    }
    Ok(Font {
        name: String::new(),
        format: FontFormat::D3f,
        line_height: font_h,
        ascent: font_h,
        glyphs,
        atlas_width: width as u32,
        atlas_height: y as u32,
        atlas,
        texture: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_synthetic_v0_file() {
        let (w, h) = (16u32, 8u32);
        let mut d = Vec::new();
        for v in [SIGNATURE, w, 7, h] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let mut arrays = vec![[0u32; 256]; 5];
        arrays[0][b'A' as usize] = 4; // x
        arrays[1][b'A' as usize] = 1; // y
        arrays[2][b'A' as usize] = 5; // width
        arrays[3][b'A' as usize] = 1; // A
        arrays[4][b'A' as usize] = (-1i32) as u32; // C
        for a in &arrays {
            for v in a {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        d.extend(std::iter::repeat_n(200u8, (w * h) as usize));
        let f = decode(&d).unwrap();
        let g = f.glyph(b'A');
        assert_eq!((g.x, g.y, g.w, g.h, g.dx, g.advance), (4, 1, 5, 7, 1, 5));
        assert_eq!(f.atlas[3], 200);
        assert!(!f.glyph(b'B').present);
        // wrong payload size: rejected
        d.pop();
        assert!(decode(&d).is_err());
    }

    #[test]
    fn decodes_a_synthetic_v1_file() {
        let mut d = Vec::new();
        d.extend_from_slice(&SIGNATURE.to_le_bytes());
        d.extend_from_slice(&[b'x'; 64]);
        d.extend_from_slice(&11i32.to_le_bytes());
        d.push(0);
        d.extend_from_slice(&400i32.to_le_bytes());
        d.extend_from_slice(&1i32.to_le_bytes()); // textures
        d.extend_from_slice(&9u32.to_le_bytes()); // height
        let mut tables = vec![[0u8; 256]; 6];
        tables[1][b'Z' as usize] = 2;
        tables[2][b'Z' as usize] = 3;
        tables[3][b'Z' as usize] = 6;
        tables[4][b'Z' as usize] = 1;
        tables[5][b'Z' as usize] = 0xff; // -1
        for t in &tables {
            d.extend_from_slice(t);
        }
        d.extend_from_slice(&(16u32 * 16).to_le_bytes());
        d.extend(std::iter::repeat_n(9u8, 256));
        let f = decode(&d).unwrap();
        assert_eq!((f.atlas_width, f.atlas_height), (16, 16));
        let g = f.glyph(b'Z');
        assert_eq!((g.x, g.y, g.w, g.h, g.dx, g.advance), (2, 3, 6, 9, 1, 6));
    }
}
