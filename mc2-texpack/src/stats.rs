//! Per-image measurements recorded in the manifest.

use crate::model::AlphaClass;

/// FNV-1a 64 over the RGBA bytes (the same function `mc2-gfx` uses for unnamed texture keys).
pub fn pixel_hash(rgba: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in rgba {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn alpha_class(rgba: &[u8]) -> AlphaClass {
    let (mut zero, mut full, mut mid) = (0usize, 0usize, 0usize);
    for p in rgba.chunks_exact(4) {
        match p[3] {
            0 => zero += 1,
            255 => full += 1,
            _ => mid += 1,
        }
    }
    if mid > 0 {
        AlphaClass::Smooth
    } else if full == 0 && zero > 0 {
        AlphaClass::Zero
    } else if zero > 0 {
        AlphaClass::Keyed
    } else {
        AlphaClass::None
    }
}

/// Pixels `mc2_render::shape::apply_paint_scheme` would recolour: exactly one non-zero channel,
/// and that channel is red, green or blue.  (The scheme also claims pure black, which stays
/// black, so black is not counted.)
pub fn paint_mask_pixels(rgba: &[u8]) -> u32 {
    rgba.chunks_exact(4)
        .filter(|p| {
            let (r, g, b) = (p[0], p[1], p[2]);
            (r > 0 && g == 0 && b == 0)
                || (g > 0 && r == 0 && b == 0)
                || (b > 0 && r == 0 && g == 0)
        })
        .count() as u32
}

/// Mean absolute difference (0..255) between opposite edges of the image, relative to the mean
/// difference between neighbouring columns/rows inside it.  Near 1 means the image tiles; much
/// more means a visible seam.  Informational only (printed in the inventory).
pub fn seam_ratio(rgba: &[u8], w: usize, h: usize) -> f32 {
    if w < 4 || h < 4 {
        return 1.0;
    }
    let px = |x: usize, y: usize, c: usize| rgba[(y * w + x) * 4 + c] as f32;
    let mut edge = 0.0f64;
    let mut inner = 0.0f64;
    let mut n_edge = 0u64;
    let mut n_inner = 0u64;
    for y in 0..h {
        for c in 0..3 {
            edge += (px(0, y, c) - px(w - 1, y, c)).abs() as f64;
            n_edge += 1;
            let xm = w / 2;
            inner += (px(xm, y, c) - px(xm - 1, y, c)).abs() as f64;
            n_inner += 1;
        }
    }
    for x in 0..w {
        for c in 0..3 {
            edge += (px(x, 0, c) - px(x, h - 1, c)).abs() as f64;
            n_edge += 1;
            let ym = h / 2;
            inner += (px(x, ym, c) - px(x, ym - 1, c)).abs() as f64;
            n_inner += 1;
        }
    }
    let e = edge / n_edge as f64;
    let i = (inner / n_inner as f64).max(0.5);
    (e / i) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_classes() {
        assert_eq!(alpha_class(&[0, 0, 0, 255, 1, 1, 1, 255]), AlphaClass::None);
        assert_eq!(alpha_class(&[0, 0, 0, 255, 1, 1, 1, 0]), AlphaClass::Keyed);
        assert_eq!(alpha_class(&[0, 0, 0, 255, 1, 1, 1, 7]), AlphaClass::Smooth);
        assert_eq!(alpha_class(&[0, 0, 0, 0, 1, 1, 1, 0]), AlphaClass::Zero);
    }

    #[test]
    fn paint_mask_counts_pure_primaries_only() {
        let px = [
            255, 0, 0, 255, // red
            0, 7, 0, 255, // green
            0, 0, 1, 255, // blue
            0, 0, 0, 255, // black: not counted
            9, 9, 0, 255, // mixed
        ];
        assert_eq!(paint_mask_pixels(&px), 3);
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(pixel_hash(&[1, 2, 3, 4]), pixel_hash(&[1, 2, 3, 4]));
        assert_ne!(pixel_hash(&[1, 2, 3, 4]), pixel_hash(&[1, 2, 3, 5]));
    }

    #[test]
    fn seam_ratio_flags_a_seam() {
        // a horizontal gradient does not tile: left and right edges differ a lot
        let (w, h) = (8usize, 8usize);
        let mut img = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    img[(y * w + x) * 4 + c] = (x * 30) as u8;
                }
            }
        }
        assert!(seam_ratio(&img, w, h) > 3.0);
        // a constant image tiles
        assert!(seam_ratio(&vec![9u8; w * h * 4], w, h) <= 1.0);
    }
}
