//! Windows code page 1252, the encoding of the game's string resources (`#pragma code_page(1252)`
//! in `Script1.rc`) and the glyph order of its fonts (glyph `n` is code-page byte `n`).

/// Unicode code points of bytes 0x80..=0x9F (the part of 1252 that differs from Latin-1).
/// Undefined bytes map to the C1 control of the same value, so decoding never fails.
const HIGH: [u32; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039,
    0x0152, 0x008D, 0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x009D, 0x017E, 0x0178,
];

/// Decodes one code-page byte.
pub fn byte_to_char(b: u8) -> char {
    let cp = match b {
        0x80..=0x9F => HIGH[usize::from(b - 0x80)],
        _ => u32::from(b),
    };
    char::from_u32(cp).unwrap_or('?')
}

/// Encodes one character, or `None` when 1252 has no byte for it.
pub fn char_to_byte(c: char) -> Option<u8> {
    let cp = c as u32;
    if cp < 0x80 || (0xA0..=0xFF).contains(&cp) {
        return Some(cp as u8);
    }
    HIGH.iter().position(|&h| h == cp).map(|i| 0x80 + i as u8)
}

/// Decodes a byte string.
pub fn decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| byte_to_char(b)).collect()
}

/// Encodes a string; characters without a 1252 byte become `?`.
pub fn encode(s: &str) -> Vec<u8> {
    s.chars().map(|c| char_to_byte(c).unwrap_or(b'?')).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_byte() {
        for b in 0..=255u8 {
            assert_eq!(char_to_byte(byte_to_char(b)), Some(b), "byte {b:#x}");
        }
        assert_eq!(byte_to_char(0x92), '\u{2019}');
        assert_eq!(encode("Victor\u{2019}s"), b"Victor\x92s");
        assert_eq!(char_to_byte('\u{4e00}'), None);
    }
}
