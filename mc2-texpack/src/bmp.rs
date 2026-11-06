//! Minimal BMP reader for the font atlases (`assets/graphics/*.bmp`): uncompressed or
//! bit-field 24/32 bit, bottom-up or top-down.  Returns top-left RGBA8.

pub struct Bmp {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

fn u16le(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// (shift, bits) of a contiguous channel mask.
fn mask_shift(mask: u32) -> Option<(u32, u32)> {
    if mask == 0 {
        return None;
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).trailing_ones();
    Some((shift, bits))
}

pub fn decode(b: &[u8]) -> Result<Bmp, String> {
    if b.get(0..2) != Some(b"BM") {
        return Err("not a BMP".into());
    }
    let data_off = u32le(b, 10).ok_or("short file")? as usize;
    let hdr = u32le(b, 14).ok_or("short file")? as usize;
    if hdr < 40 {
        return Err(format!("unsupported BMP header size {hdr}"));
    }
    let w = u32le(b, 18).ok_or("short file")? as i32;
    let h = u32le(b, 22).ok_or("short file")? as i32;
    let bpp = u16le(b, 28).ok_or("short file")?;
    let compression = u32le(b, 30).ok_or("short file")?;
    if w <= 0 || h == 0 || w > 16384 || h.unsigned_abs() > 16384 {
        return Err(format!("bad BMP size {w}x{h}"));
    }
    if !matches!(bpp, 24 | 32) || !matches!(compression, 0 | 3) {
        return Err(format!(
            "unsupported BMP: {bpp} bpp, compression {compression}"
        ));
    }
    let (w, top_down, h) = (w as usize, h < 0, h.unsigned_abs() as usize);
    // default masks: BGR(A) byte order
    let (mut r, mut g, mut bl, mut a) = (0x00ff_0000u32, 0x0000_ff00u32, 0x0000_00ffu32, 0u32);
    if bpp == 32 {
        if compression == 3 {
            // bit fields: the masks follow the 40 byte header (V4/V5 headers embed them there)
            r = u32le(b, 54).ok_or("short file")?;
            g = u32le(b, 58).ok_or("short file")?;
            bl = u32le(b, 62).ok_or("short file")?;
            if hdr >= 56 {
                a = u32le(b, 66).ok_or("short file")?;
            }
        } else {
            a = 0xff00_0000;
        }
    }
    let ch = |m: u32| mask_shift(m).filter(|(_, bits)| *bits == 8).map(|(s, _)| s);
    let (rs, gs, bs) = (
        ch(r).ok_or("unsupported colour mask")?,
        ch(g).ok_or("unsupported colour mask")?,
        ch(bl).ok_or("unsupported colour mask")?,
    );
    let a_s = ch(a);
    let bytes_pp = (bpp / 8) as usize;
    let stride = (w * bytes_pp).div_ceil(4) * 4;
    let mut rgba = vec![255u8; w * h * 4];
    for row in 0..h {
        let src_row = if top_down { row } else { h - 1 - row };
        let base = data_off + src_row * stride;
        for x in 0..w {
            let o = base + x * bytes_pp;
            let px = b.get(o..o + bytes_pp).ok_or("pixel data truncated")?;
            let v = if bytes_pp == 4 {
                u32::from_le_bytes([px[0], px[1], px[2], px[3]])
            } else {
                u32::from_le_bytes([px[0], px[1], px[2], 0])
            };
            let d = (row * w + x) * 4;
            rgba[d] = (v >> rs) as u8;
            rgba[d + 1] = (v >> gs) as u8;
            rgba[d + 2] = (v >> bs) as u8;
            rgba[d + 3] = a_s.map_or(255, |s| (v >> s) as u8);
        }
    }
    Ok(Bmp {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bmp32(w: u32, h: i32, pixels_bgra: &[u8]) -> Vec<u8> {
        let mut v = b"BM".to_vec();
        v.extend_from_slice(&(54 + pixels_bgra.len() as u32).to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(&54u32.to_le_bytes());
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&(w as i32).to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&32u16.to_le_bytes());
        v.extend_from_slice(&[0; 24]);
        v.extend_from_slice(pixels_bgra);
        v
    }

    #[test]
    fn bottom_up_bgra_becomes_top_left_rgba() {
        // 1x2 image, bottom row first: blue then red
        let file = bmp32(1, 2, &[255, 0, 0, 255, 0, 0, 255, 255]);
        let img = decode(&file).unwrap();
        assert_eq!((img.width, img.height), (1, 2));
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]); // top = red
        assert_eq!(&img.rgba[4..8], &[0, 0, 255, 255]); // bottom = blue
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope").is_err());
        assert!(decode(&bmp32(1, 1, &[0, 0, 0, 0])[..40]).is_err());
    }
}
