// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `0xAARRGGBB` colours as the GUI code uses them.

/// `interpolateColor` (`mclib/utilities.cpp`): per-channel linear blend, truncated toward zero,
/// `color1` below 0 and `color2` above 1.
pub fn interpolate_color(color1: u32, color2: u32, percent: f32) -> u32 {
    if percent > 1.0 {
        return color2;
    }
    if percent < 0.0 {
        return color1;
    }
    let ch = |shift: u32| -> u32 {
        let a = ((color1 >> shift) & 0xff) as i32;
        let b = ((color2 >> shift) & 0xff) as i32;
        // (float)min + (float)(max - min) * percent, converted to long (truncation)
        let v = (a as f32 + (b - a) as f32 * percent) as i32;
        (v as u32 & 0xff) << shift
    };
    ch(0) | ch(8) | ch(16) | ch(24)
}

pub fn alpha(c: u32) -> u8 {
    (c >> 24) as u8
}

/// Splits into normalised `[r, g, b, a]`.
pub fn to_rgba_f32(c: u32) -> [f32; 4] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
        (c >> 24) as f32 / 255.0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_like_the_original() {
        assert_eq!(interpolate_color(0, 0x7f000000, 0.5), 0x3f000000);
        assert_eq!(interpolate_color(0xff000000, 0, 1.5), 0);
        assert_eq!(interpolate_color(0xff000000, 0, -0.1), 0xff000000);
        assert_eq!(interpolate_color(0xffffffff, 0xfff26f00, 0.5), 0xfff8b77f);
    }
}
