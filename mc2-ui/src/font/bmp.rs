//! Minimal Windows `.bmp` reader for the font atlases (`assets/graphics/*.bmp`): uncompressed 24- or
//! 32-bit, `BI_RGB` or `BI_BITFIELDS`, bottom-up or top-down. Written from the public BMP file
//! layout (BITMAPFILEHEADER, BITMAPINFOHEADER/V4/V5); explicit little-endian reads.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// RGBA8, top row first.
    pub rgba: Vec<u8>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn i32_at(d: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn channel(px: u32, mask: u32) -> u8 {
    if mask == 0 {
        return 0;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).count_ones();
    let v = (px & mask) >> shift;
    if bits >= 8 {
        (v >> (bits - 8)) as u8
    } else {
        ((v * 255) / ((1 << bits) - 1)) as u8
    }
}

pub fn decode(d: &[u8]) -> Result<Bitmap, String> {
    if d.get(0..2) != Some(b"BM") {
        return Err("not a BMP (no BM signature)".into());
    }
    let off = u32_at(d, 10).ok_or("truncated header")? as usize;
    let hs = u32_at(d, 14).ok_or("truncated header")? as usize;
    if hs < 40 {
        return Err(format!("unsupported BMP header size {hs}"));
    }
    let width = i32_at(d, 18).ok_or("truncated header")?;
    let height = i32_at(d, 22).ok_or("truncated header")?;
    let bpp = u16_at(d, 28).ok_or("truncated header")?;
    let compression = u32_at(d, 30).ok_or("truncated header")?;
    if width <= 0 || height == 0 || width > 16384 || height.unsigned_abs() > 16384 {
        return Err(format!("bad BMP size {width}x{height}"));
    }
    let (w, h) = (width as u32, height.unsigned_abs());
    let top_down = height < 0;
    let (rm, gm, bm, am) = match (compression, bpp) {
        (0, 24) => (0xff_0000, 0xff00, 0xff, 0),
        (0, 32) => (0xff_0000, 0xff00, 0xff, 0),
        (3, 32) => {
            // masks follow the 40-byte header (inside it for V4/V5)
            let m = |i: usize| u32_at(d, 14 + 40 + 4 * i).ok_or("truncated masks");
            let a = if hs >= 56 { m(3)? } else { 0 };
            (m(0)?, m(1)?, m(2)?, a)
        }
        _ => {
            return Err(format!(
                "unsupported BMP: {bpp} bpp, compression {compression}"
            ))
        }
    };
    let bytes_pp = usize::from(bpp / 8);
    let stride = (w as usize * bytes_pp).div_ceil(4) * 4;
    let need = stride * h as usize;
    let pixels = d
        .get(off..off + need)
        .ok_or_else(|| "BMP pixel data truncated".to_string())?;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for row in 0..h as usize {
        let src_row = if top_down { row } else { h as usize - 1 - row };
        let src = &pixels[src_row * stride..];
        for x in 0..w as usize {
            let p = &src[x * bytes_pp..x * bytes_pp + bytes_pp];
            let px = if bytes_pp == 4 {
                u32::from_le_bytes([p[0], p[1], p[2], p[3]])
            } else {
                u32::from_le_bytes([p[0], p[1], p[2], 0])
            };
            let o = (row * w as usize + x) * 4;
            rgba[o] = channel(px, rm);
            rgba[o + 1] = channel(px, gm);
            rgba[o + 2] = channel(px, bm);
            rgba[o + 3] = if am == 0 { 255 } else { channel(px, am) };
        }
    }
    Ok(Bitmap {
        width: w,
        height: h,
        rgba,
    })
}

/// Writes a 32-bit `BI_BITFIELDS` bottom-up BMP with a V5 header (the layout of the shipped
/// atlases). Used by tests to build fixtures.
pub fn encode_rgba32(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    let data_len = w * h * 4;
    let mut out = Vec::with_capacity(138 + data_len as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(138 + data_len).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&138u32.to_le_bytes());
    out.extend_from_slice(&124u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&3u32.to_le_bytes());
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(&[0; 16]);
    for m in [0x00ff_0000u32, 0xff00, 0xff, 0xff00_0000] {
        out.extend_from_slice(&m.to_le_bytes());
    }
    out.resize(138, 0);
    for row in (0..h as usize).rev() {
        for x in 0..w as usize {
            let o = (row * w as usize + x) * 4;
            out.extend_from_slice(&[rgba[o + 2], rgba[o + 1], rgba[o], rgba[o + 3]]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_bitfields_bottom_up() {
        let px = [
            255u8, 0, 0, 255, 0, 255, 0, 128, //
            0, 0, 255, 0, 255, 255, 255, 255,
        ];
        let bmp = encode_rgba32(2, 2, &px);
        let d = decode(&bmp).unwrap();
        assert_eq!((d.width, d.height), (2, 2));
        assert_eq!(d.rgba, px);
    }

    #[test]
    fn decodes_24_bit_padded_rows() {
        // 1x2 bottom-up, 24 bit: rows padded to 4 bytes
        let mut b = Vec::new();
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&(54u32 + 8).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&54u32.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&1i32.to_le_bytes());
        b.extend_from_slice(&2i32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[0; 24]);
        b.extend_from_slice(&[1, 2, 3, 0, 4, 5, 6, 0]); // bottom row then top row (BGR)
        let d = decode(&b).unwrap();
        assert_eq!(d.rgba, vec![6, 5, 4, 255, 3, 2, 1, 255]);
        assert!(decode(b"XX").is_err());
    }
}
