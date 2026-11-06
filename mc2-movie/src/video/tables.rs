//! Variable length code tables and constant tables of ISO/IEC 11172-2 (MPEG-1 video).
//!
//! The codes are written as bit strings exactly as the standard's tables list them and expanded
//! into direct lookup tables on first use. The table builder panics on a code that is not prefix
//! free, and the unit tests check each table's code-space usage, so a transcription slip shows
//! up as a test failure instead of a corrupt picture.

use std::sync::OnceLock;

/// One decoded lookup entry. `len == 0` marks an unused bit pattern.
#[derive(Clone, Copy, Default)]
pub struct Entry {
    pub val: i16,
    pub len: u8,
}

/// A direct lookup table indexed by the next `bits` bits of the stream.
pub struct Vlc {
    pub bits: u32,
    table: Vec<Entry>,
}

impl Vlc {
    fn build(codes: &[(&str, i16)]) -> Self {
        let bits = codes.iter().map(|(c, _)| c.len()).max().unwrap() as u32;
        let mut table = vec![Entry::default(); 1 << bits];
        for &(code, val) in codes {
            let len = code.len() as u32;
            let v = u32::from_str_radix(code, 2).expect("bit string");
            let lo = (v << (bits - len)) as usize;
            let hi = ((v + 1) << (bits - len)) as usize;
            for e in &mut table[lo..hi] {
                assert!(e.len == 0, "VLC code {code} overlaps another code");
                *e = Entry {
                    val,
                    len: len as u8,
                };
            }
        }
        Self { bits, table }
    }

    /// Look up the entry for the upcoming bits (`peek32` holds the next 32 stream bits).
    #[inline]
    pub fn lookup(&self, peek32: u32) -> Entry {
        self.table[(peek32 >> (32 - self.bits)) as usize]
    }

    /// Fraction of the code space that is assigned.
    #[cfg(test)]
    pub fn used_space(&self) -> f64 {
        self.table.iter().filter(|e| e.len != 0).count() as f64 / self.table.len() as f64
    }
}

// Macroblock type flags (decoded value of the macroblock_type tables).
pub const MB_QUANT: i16 = 1;
pub const MB_FWD: i16 = 2;
pub const MB_BWD: i16 = 4;
pub const MB_PATTERN: i16 = 8;
pub const MB_INTRA: i16 = 16;

/// Table B.1 escape (adds 33 to the address increment) and stuffing markers.
pub const ADDR_ESCAPE: i16 = 34;
pub const ADDR_STUFFING: i16 = 35;

/// Special values of the DCT coefficient table.
pub const COEF_EOB: i16 = -1;
pub const COEF_ESCAPE: i16 = -2;

pub struct Tables {
    pub mb_addr: Vlc,
    pub mb_type_i: Vlc,
    pub mb_type_p: Vlc,
    pub mb_type_b: Vlc,
    pub cbp: Vlc,
    pub motion: Vlc,
    pub dc_luma: Vlc,
    pub dc_chroma: Vlc,
    /// Value is `run << 8 | level`, or [`COEF_EOB`] / [`COEF_ESCAPE`]. Codes exclude the sign bit.
    pub coef: Vlc,
}

pub fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(build_tables)
}

/// Table B.1: macroblock_address_increment.
const MB_ADDR: [(&str, i16); 35] = [
    ("1", 1),
    ("011", 2),
    ("010", 3),
    ("0011", 4),
    ("0010", 5),
    ("00011", 6),
    ("00010", 7),
    ("0000111", 8),
    ("0000110", 9),
    ("00001011", 10),
    ("00001010", 11),
    ("00001001", 12),
    ("00001000", 13),
    ("00000111", 14),
    ("00000110", 15),
    ("0000010111", 16),
    ("0000010110", 17),
    ("0000010101", 18),
    ("0000010100", 19),
    ("0000010011", 20),
    ("0000010010", 21),
    ("00000100011", 22),
    ("00000100010", 23),
    ("00000100001", 24),
    ("00000100000", 25),
    ("00000011111", 26),
    ("00000011110", 27),
    ("00000011101", 28),
    ("00000011100", 29),
    ("00000011011", 30),
    ("00000011010", 31),
    ("00000011001", 32),
    ("00000011000", 33),
    ("00000001000", ADDR_ESCAPE),
    ("00000001111", ADDR_STUFFING),
];

/// Table B.2a: macroblock_type in I pictures.
const MB_TYPE_I: [(&str, i16); 2] = [("1", MB_INTRA), ("01", MB_QUANT | MB_INTRA)];

/// Table B.2b: macroblock_type in P pictures.
const MB_TYPE_P: [(&str, i16); 7] = [
    ("1", MB_FWD | MB_PATTERN),
    ("01", MB_PATTERN),
    ("001", MB_FWD),
    ("00011", MB_INTRA),
    ("00010", MB_QUANT | MB_FWD | MB_PATTERN),
    ("00001", MB_QUANT | MB_PATTERN),
    ("000001", MB_QUANT | MB_INTRA),
];

/// Table B.2c: macroblock_type in B pictures.
const MB_TYPE_B: [(&str, i16); 11] = [
    ("10", MB_FWD | MB_BWD),
    ("11", MB_FWD | MB_BWD | MB_PATTERN),
    ("010", MB_BWD),
    ("011", MB_BWD | MB_PATTERN),
    ("0010", MB_FWD),
    ("0011", MB_FWD | MB_PATTERN),
    ("00011", MB_INTRA),
    ("00010", MB_QUANT | MB_FWD | MB_BWD | MB_PATTERN),
    ("000011", MB_QUANT | MB_FWD | MB_PATTERN),
    ("000010", MB_QUANT | MB_BWD | MB_PATTERN),
    ("000001", MB_QUANT | MB_INTRA),
];

/// Table B.3: coded_block_pattern. Bit 5 (32) is block 0 (first luma block), bit 0 is Cr.
const CBP: [(&str, i16); 64] = [
    ("111", 60),
    ("1101", 4),
    ("1100", 8),
    ("1011", 16),
    ("1010", 32),
    ("10011", 12),
    ("10010", 48),
    ("10001", 20),
    ("10000", 40),
    ("01111", 28),
    ("01110", 44),
    ("01101", 52),
    ("01100", 56),
    ("01011", 1),
    ("01010", 61),
    ("01001", 2),
    ("01000", 62),
    ("001111", 24),
    ("001110", 36),
    ("001101", 3),
    ("001100", 63),
    ("0010111", 5),
    ("0010110", 9),
    ("0010101", 17),
    ("0010100", 33),
    ("0010011", 6),
    ("0010010", 10),
    ("0010001", 18),
    ("0010000", 34),
    ("00011111", 7),
    ("00011110", 11),
    ("00011101", 19),
    ("00011100", 35),
    ("00011011", 13),
    ("00011010", 49),
    ("00011001", 21),
    ("00011000", 41),
    ("00010111", 14),
    ("00010110", 50),
    ("00010101", 22),
    ("00010100", 42),
    ("00010011", 15),
    ("00010010", 51),
    ("00010001", 23),
    ("00010000", 43),
    ("00001111", 25),
    ("00001110", 37),
    ("00001101", 26),
    ("00001100", 38),
    ("00001011", 29),
    ("00001010", 45),
    ("00001001", 53),
    ("00001000", 57),
    ("00000111", 30),
    ("00000110", 46),
    ("00000101", 54),
    ("00000100", 58),
    ("000000111", 31),
    ("000000110", 47),
    ("000000101", 55),
    ("000000100", 59),
    ("000000011", 27),
    ("000000010", 39),
    ("000000001", 0),
];

/// Table B.4: motion_code magnitudes 1..=16 (a sign bit follows the listed prefix).
const MOTION_MAG: [&str; 16] = [
    "01",
    "001",
    "0001",
    "000011",
    "0000101",
    "0000100",
    "0000011",
    "000001011",
    "000001010",
    "000001001",
    "0000010001",
    "0000010000",
    "0000001111",
    "0000001110",
    "0000001101",
    "0000001100",
];

/// Table B.5a: dct_dc_size_luminance.
const DC_LUMA: [(&str, i16); 9] = [
    ("100", 0),
    ("00", 1),
    ("01", 2),
    ("101", 3),
    ("110", 4),
    ("1110", 5),
    ("11110", 6),
    ("111110", 7),
    ("1111110", 8),
];

/// Table B.5b: dct_dc_size_chrominance.
const DC_CHROMA: [(&str, i16); 9] = [
    ("00", 0),
    ("01", 1),
    ("10", 2),
    ("110", 3),
    ("1110", 4),
    ("11110", 5),
    ("111110", 6),
    ("1111110", 7),
    ("11111110", 8),
];

/// Table B.5c: dct_coeff_next, as (code without the sign bit, run, level).
const COEF: [(&str, u8, u8); 111] = [
    ("11", 0, 1),
    ("011", 1, 1),
    ("0100", 0, 2),
    ("0101", 2, 1),
    ("00101", 0, 3),
    ("00111", 3, 1),
    ("00110", 4, 1),
    ("000110", 1, 2),
    ("000111", 5, 1),
    ("000101", 6, 1),
    ("000100", 7, 1),
    ("0000110", 0, 4),
    ("0000100", 2, 2),
    ("0000111", 8, 1),
    ("0000101", 9, 1),
    ("00100110", 0, 5),
    ("00100001", 0, 6),
    ("00100101", 1, 3),
    ("00100100", 3, 2),
    ("00100111", 10, 1),
    ("00100011", 11, 1),
    ("00100010", 12, 1),
    ("00100000", 13, 1),
    ("0000001010", 0, 7),
    ("0000001100", 1, 4),
    ("0000001011", 2, 3),
    ("0000001111", 4, 2),
    ("0000001001", 5, 2),
    ("0000001110", 14, 1),
    ("0000001101", 15, 1),
    ("0000001000", 16, 1),
    ("000000011101", 0, 8),
    ("000000011000", 0, 9),
    ("000000010011", 0, 10),
    ("000000010000", 0, 11),
    ("000000011011", 1, 5),
    ("000000010100", 2, 4),
    ("000000011100", 3, 3),
    ("000000010010", 4, 3),
    ("000000011110", 6, 2),
    ("000000010101", 7, 2),
    ("000000010001", 8, 2),
    ("000000011111", 17, 1),
    ("000000011010", 18, 1),
    ("000000011001", 19, 1),
    ("000000010111", 20, 1),
    ("000000010110", 21, 1),
    ("0000000011010", 0, 12),
    ("0000000011001", 0, 13),
    ("0000000011000", 0, 14),
    ("0000000010111", 0, 15),
    ("0000000010110", 1, 6),
    ("0000000010101", 1, 7),
    ("0000000010100", 2, 5),
    ("0000000010011", 3, 4),
    ("0000000010010", 5, 3),
    ("0000000010001", 9, 2),
    ("0000000010000", 10, 2),
    ("0000000011111", 22, 1),
    ("0000000011110", 23, 1),
    ("0000000011101", 24, 1),
    ("0000000011100", 25, 1),
    ("0000000011011", 26, 1),
    ("00000000011111", 0, 16),
    ("00000000011110", 0, 17),
    ("00000000011101", 0, 18),
    ("00000000011100", 0, 19),
    ("00000000011011", 0, 20),
    ("00000000011010", 0, 21),
    ("00000000011001", 0, 22),
    ("00000000011000", 0, 23),
    ("00000000010111", 0, 24),
    ("00000000010110", 0, 25),
    ("00000000010101", 0, 26),
    ("00000000010100", 0, 27),
    ("00000000010011", 0, 28),
    ("00000000010010", 0, 29),
    ("00000000010001", 0, 30),
    ("00000000010000", 0, 31),
    ("000000000011000", 0, 32),
    ("000000000010111", 0, 33),
    ("000000000010110", 0, 34),
    ("000000000010101", 0, 35),
    ("000000000010100", 0, 36),
    ("000000000010011", 0, 37),
    ("000000000010010", 0, 38),
    ("000000000010001", 0, 39),
    ("000000000010000", 0, 40),
    ("000000000011111", 1, 8),
    ("000000000011110", 1, 9),
    ("000000000011101", 1, 10),
    ("000000000011100", 1, 11),
    ("000000000011011", 1, 12),
    ("000000000011010", 1, 13),
    ("000000000011001", 1, 14),
    ("0000000000010011", 1, 15),
    ("0000000000010010", 1, 16),
    ("0000000000010001", 1, 17),
    ("0000000000010000", 1, 18),
    ("0000000000010100", 6, 3),
    ("0000000000011010", 11, 2),
    ("0000000000011001", 12, 2),
    ("0000000000011000", 13, 2),
    ("0000000000010111", 14, 2),
    ("0000000000010110", 15, 2),
    ("0000000000010101", 16, 2),
    ("0000000000011111", 27, 1),
    ("0000000000011110", 28, 1),
    ("0000000000011101", 29, 1),
    ("0000000000011100", 30, 1),
    ("0000000000011011", 31, 1),
];

fn build_tables() -> Tables {
    let motion: Vec<(String, i16)> = std::iter::once(("1".to_string(), 0i16))
        .chain(MOTION_MAG.iter().enumerate().flat_map(|(i, m)| {
            let mag = i as i16 + 1;
            [(format!("{m}0"), mag), (format!("{m}1"), -mag)]
        }))
        .collect();
    let motion_refs: Vec<(&str, i16)> = motion.iter().map(|(s, v)| (s.as_str(), *v)).collect();

    let mut coef: Vec<(&str, i16)> = COEF
        .iter()
        .map(|&(c, run, level)| (c, i16::from(run) << 8 | i16::from(level)))
        .collect();
    coef.push(("10", COEF_EOB));
    coef.push(("000001", COEF_ESCAPE));

    Tables {
        mb_addr: Vlc::build(&MB_ADDR),
        mb_type_i: Vlc::build(&MB_TYPE_I),
        mb_type_p: Vlc::build(&MB_TYPE_P),
        mb_type_b: Vlc::build(&MB_TYPE_B),
        cbp: Vlc::build(&CBP),
        motion: Vlc::build(&motion_refs),
        dc_luma: Vlc::build(&DC_LUMA),
        dc_chroma: Vlc::build(&DC_CHROMA),
        coef: Vlc::build(&coef),
    }
}

/// Zig-zag scan: coefficient index in scan order -> position in the 8x8 block (row major).
pub const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Default intra quantiser matrix (row major).
pub const DEFAULT_INTRA_MATRIX: [u8; 64] = [
    8, 16, 19, 22, 26, 27, 29, 34, 16, 16, 22, 24, 27, 29, 34, 37, 19, 22, 26, 27, 29, 34, 34, 38,
    22, 22, 26, 27, 29, 34, 37, 40, 22, 26, 27, 29, 32, 35, 40, 48, 26, 27, 29, 32, 35, 40, 48, 58,
    26, 27, 29, 34, 38, 46, 56, 69, 27, 29, 35, 38, 46, 56, 69, 83,
];

/// Frame rates by frame_rate_code (index 1..=8).
pub const FRAME_RATES: [f64; 9] = [
    0.0,
    24000.0 / 1001.0,
    24.0,
    25.0,
    30000.0 / 1001.0,
    30.0,
    50.0,
    60000.0 / 1001.0,
    60.0,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_build_and_use_the_expected_code_space() {
        let t = tables();
        let near = |a: f64, b: f64| (a - b).abs() < 1e-12;
        assert!(near(t.mb_type_p.used_space(), 1.0 - 1.0 / 64.0));
        assert!(near(t.mb_type_b.used_space(), 1.0 - 1.0 / 64.0));
        assert!(near(t.mb_type_i.used_space(), 0.75));
        assert!(near(t.cbp.used_space(), 1.0 - 1.0 / 512.0));
        assert!(near(t.dc_luma.used_space(), 1.0 - 1.0 / 128.0));
        assert!(near(t.dc_chroma.used_space(), 1.0 - 1.0 / 256.0));
        // Motion codes: 0000000xxxx and 00000010xxx are unused.
        assert!(near(t.motion.used_space(), 1.0 - 1.0 / 128.0 - 1.0 / 256.0));
    }

    #[test]
    fn coefficient_table_has_every_pair_once() {
        let mut seen = std::collections::HashSet::new();
        for &(_, run, level) in &COEF {
            assert!(seen.insert((run, level)), "duplicate pair {run},{level}");
        }
        let max_level = |r: u8| {
            COEF.iter()
                .filter(|c| c.1 == r)
                .map(|c| c.2)
                .max()
                .unwrap_or(0)
        };
        assert_eq!(max_level(0), 40);
        assert_eq!(max_level(1), 18);
        assert_eq!(max_level(2), 5);
        assert_eq!(max_level(3), 4);
        assert_eq!(max_level(4), 3);
        assert_eq!(max_level(5), 3);
        assert_eq!(max_level(6), 3);
        assert_eq!(max_level(7), 2);
        assert_eq!(max_level(16), 2);
        assert_eq!(max_level(31), 1);
        assert_eq!(COEF.iter().map(|c| c.1).max(), Some(31));
    }

    #[test]
    fn zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &z in &ZIGZAG {
            assert!(!seen[z as usize]);
            seen[z as usize] = true;
        }
    }
}
