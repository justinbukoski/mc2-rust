// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
use crate::{AssetError, Reader};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
}

pub fn decode(bytes: &[u8]) -> crate::reader::Result<RgbaImage> {
    let mut r = Reader::new(bytes);
    let id_len = r.u8()? as usize;
    let cmap = r.u8()?;
    let kind = r.u8()?;
    let _cmap_first = r.u16()?;
    let _cmap_len = r.u16()?;
    let _cmap_bits = r.u8()?;
    let _x = r.u16()?;
    let _y = r.u16()?;
    let width = r.u16()?;
    let height = r.u16()?;
    let depth = r.u8()?;
    let descriptor = r.u8()?;
    if cmap != 0 {
        return Err(AssetError("color-mapped TGA is unsupported".into()));
    }
    let rle = match kind {
        2 | 3 => false,
        10 | 11 => true,
        _ => return Err(AssetError(format!("unsupported TGA image type {kind}"))),
    };
    if (kind == 3 || kind == 11) && depth != 8 {
        return Err(AssetError("grayscale TGA must be 8-bit".into()));
    }
    if !matches!(depth, 8 | 16 | 24 | 32) {
        return Err(AssetError(format!("unsupported TGA depth {depth}")));
    }
    if depth == 8 && kind != 3 && kind != 11 {
        return Err(AssetError("8-bit color mapped TGA is unsupported".into()));
    }
    r.take(id_len)?;
    let count = usize::from(width)
        .checked_mul(usize::from(height))
        .ok_or_else(|| AssetError("TGA dimensions overflow".into()))?;
    let bytes_per_pixel = usize::from(depth).div_ceil(8);
    let available = r.remaining();
    let max_pixels_from_input = if rle {
        available
            .checked_mul(128)
            .and_then(|n| n.checked_div(1 + bytes_per_pixel))
            .unwrap_or(usize::MAX)
    } else {
        available / bytes_per_pixel
    };
    if count > max_pixels_from_input {
        return Err(AssetError(
            "TGA dimensions exceed available pixel data".into(),
        ));
    }
    let pixel_bytes = count
        .checked_mul(4)
        .ok_or_else(|| AssetError("TGA pixel buffer size overflow".into()))?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(count)
        .map_err(|_| AssetError("TGA raw pixel allocation failed".into()))?;
    if rle {
        while raw.len() < count {
            let packet = r.u8()?;
            let n = usize::from(packet & 0x7f) + 1;
            if raw.len() + n > count {
                return Err(AssetError("TGA RLE packet exceeds image dimensions".into()));
            }
            if packet & 0x80 != 0 {
                let px = read_pixel(&mut r, depth)?;
                raw.extend(std::iter::repeat_n(px, n));
            } else {
                for _ in 0..n {
                    raw.push(read_pixel(&mut r, depth)?);
                }
            }
        }
    } else {
        for _ in 0..count {
            raw.push(read_pixel(&mut r, depth)?);
        }
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(pixel_bytes)
        .map_err(|_| AssetError("TGA output allocation failed".into()))?;
    pixels.resize(pixel_bytes, 0);
    for sy in 0..usize::from(height) {
        for sx in 0..usize::from(width) {
            let x = if descriptor & 0x10 != 0 {
                usize::from(width) - 1 - sx
            } else {
                sx
            };
            let y = if descriptor & 0x20 != 0 {
                sy
            } else {
                usize::from(height) - 1 - sy
            };
            let dst = (y * usize::from(width) + x) * 4;
            pixels[dst..dst + 4].copy_from_slice(&raw[sy * usize::from(width) + sx]);
        }
    }
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}

fn read_pixel(r: &mut Reader<'_>, depth: u8) -> crate::reader::Result<[u8; 4]> {
    Ok(match depth {
        8 => {
            let l = r.u8()?;
            [l, l, l, 255]
        }
        16 => {
            let p = r.u16()?;
            let c = |s: u32| (((p >> s) & 31u16) as u8) << 3;
            [c(10), c(5), c(0), if p & 0x8000 != 0 { 255 } else { 0 }]
        }
        24 => {
            let b = r.u8()?;
            let g = r.u8()?;
            let rr = r.u8()?;
            [rr, g, b, 255]
        }
        32 => {
            let b = r.u8()?;
            let g = r.u8()?;
            let rr = r.u8()?;
            let a = r.u8()?;
            [rr, g, b, a]
        }
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn header(w: u16, h: u16, depth: u8, kind: u8, desc: u8) -> Vec<u8> {
        let mut b = vec![0, 0, kind, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        b.extend(w.to_le_bytes());
        b.extend(h.to_le_bytes());
        b.extend([depth, desc]);
        b
    }
    #[test]
    fn uncompressed_bgr_origin_and_alpha() {
        let mut b = header(1, 1, 24, 2, 0);
        b.extend([3, 2, 1]);
        assert_eq!(decode(&b).unwrap().pixels, [1, 2, 3, 255]);
    }
    #[test]
    fn rle_and_top_origin() {
        let mut b = header(2, 1, 32, 10, 0x20);
        b.extend([0x81, 3, 2, 1, 4]);
        assert_eq!(decode(&b).unwrap().pixels, [1, 2, 3, 4, 1, 2, 3, 4]);
    }
    #[test]
    fn grayscale_and_origin() {
        let mut b = header(1, 2, 8, 3, 0x20);
        b.extend([10, 20]);
        assert_eq!(
            decode(&b).unwrap().pixels,
            [10, 10, 10, 255, 20, 20, 20, 255]
        );
    }
    #[test]
    fn rejects_dimensions_that_claim_missing_pixels_without_allocating() {
        let b = header(u16::MAX, u16::MAX, 24, 2, 0);
        assert!(decode(&b).is_err());
    }
}
