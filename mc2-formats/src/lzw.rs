// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LZ-coded LZW streams, as used by LZD packets in PAK files.
//!
//! Ported from `mclib/lzdecomp.cpp` and `mclib/lzcomp.cpp`, following the
//! x86 assembly paths (the code the Windows game runs). The `LINUX_BUILD`
//! branches in those files use zlib instead, and they do not read these
//! streams. The shipped LZD packets are LZW; the zlib check in the
//! integration tests shows this.
//!
//! Stream format: codes are packed least-significant-bit first. Code width
//! starts at 9 bits and grows to 12 as the table fills. Code 256 clears the
//! table, 257 ends the stream, and 258 is the first free table slot. Table
//! entries are (prefix code, suffix byte) pairs.

use crate::error::{FormatError, Result};

const CLEAR_CODE: u32 = 256;
const EOF_CODE: u32 = 257;
const FIRST_FREE: u32 = 258;
/// Table capacity: codes never exceed 12 bits.
const MAX_CODES: u32 = 4096;
/// Sentinel for "no child" / "no sibling" in the encoder table.
const NONE: u32 = u32::MAX;

/// Decodes an LZW stream into at most `limit` bytes.
///
/// Decoding stops at the end code or when the input runs out, as the C reader
/// does. Callers compare the length with the expected size. Codes that cannot
/// occur in a valid stream are errors, and so is output beyond `limit`. Chain
/// walks are bounded: every table entry points at a smaller code, and the walk
/// is also capped at the table size.
pub fn decompress(src: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut prefix = vec![0u32; MAX_CODES as usize];
    let mut suffix = vec![0u8; MAX_CODES as usize];
    let mut stack: Vec<u8> = Vec::new();

    let mut reader = BitReader {
        src,
        pos: 0,
        bit: 0,
    };
    let mut width: u32 = 9;
    let mut max_index: u32 = 512;
    let mut free: u32 = FIRST_FREE;
    let mut old_chain: u32 = 0;
    let mut old_suffix: u8 = 0;

    while let Some(code) = reader.next(width) {
        if code == EOF_CODE {
            break;
        }
        if code == CLEAR_CODE {
            width = 9;
            max_index = 512;
            free = FIRST_FREE;
            let Some(first) = reader.next(width) else {
                break;
            };
            if first >= 256 {
                return Err(invalid(format!("code {first} follows a clear code")));
            }
            old_chain = first;
            old_suffix = first as u8;
            if out.len() >= limit {
                return Err(too_large(limit));
            }
            out.push(first as u8);
            continue;
        }
        // A valid code names a table entry, or the entry being defined now.
        if code > free {
            return Err(invalid(format!(
                "code {code} is beyond the table (free slot {free})"
            )));
        }

        // Expand the code into its byte string. The string is pushed in
        // reverse, then emitted from the top of the stack.
        stack.clear();
        let mut cur = if code < free {
            code
        } else {
            // The code is the entry being defined right now (KwKwK case).
            prefix[code as usize] = old_chain;
            suffix[code as usize] = old_suffix;
            stack.push(old_suffix);
            old_chain
        };
        let mut steps = 0u32;
        while cur >= 256 {
            steps += 1;
            if steps > MAX_CODES {
                return Err(invalid("prefix chain longer than the table".to_string()));
            }
            stack.push(suffix[cur as usize]);
            cur = prefix[cur as usize];
        }
        let literal = cur as u8;
        old_suffix = literal;
        stack.push(literal);
        if out.len() + stack.len() > limit {
            return Err(too_large(limit));
        }
        out.extend(stack.iter().rev());

        // Define the entry for the slot that is now free.
        if free < MAX_CODES {
            prefix[free as usize] = old_chain;
            suffix[free as usize] = literal;
        }
        free += 1;
        old_chain = code;

        if free < max_index || width == 12 {
            continue;
        }
        width += 1;
        max_index *= 2;
    }
    Ok(out)
}

fn invalid(detail: String) -> FormatError {
    FormatError::Decompress {
        format: "LZW",
        detail,
    }
}

fn too_large(limit: usize) -> FormatError {
    FormatError::Decompress {
        format: "LZW",
        detail: format!("output exceeds the declared size of {limit} bytes"),
    }
}

/// Encodes `src` as an LZW stream that `decompress` and the game both accept.
///
/// Port of `LZCompress`. An empty input yields an empty stream: the C++ code
/// would emit a stray zero code there, and the reader already treats empty
/// packed data as empty output.
pub fn compress(src: &[u8]) -> Vec<u8> {
    if src.is_empty() {
        return Vec::new();
    }
    let mut table = EncodeTable::new();
    let mut writer = BitWriter::default();
    let mut width: u32 = 9;
    let mut max_code: u32 = 512;
    let mut free: u32 = FIRST_FREE;

    writer.put(CLEAR_CODE, width);
    let mut prefix = u32::from(src[0]);

    for &k in &src[1..] {
        match table.lookup(prefix, k) {
            Ok(code) => {
                prefix = code;
            }
            Err(tail) => {
                let assigned = free;
                if assigned < MAX_CODES {
                    table.insert(prefix, tail, k, assigned);
                    free += 1;
                }
                writer.put(prefix, width);
                if assigned >= max_code {
                    if width < 12 {
                        width += 1;
                        max_code *= 2;
                    } else {
                        writer.put(CLEAR_CODE, width);
                        table.clear();
                        width = 9;
                        max_code = 512;
                        free = FIRST_FREE;
                    }
                }
                prefix = u32::from(k);
            }
        }
    }
    writer.put(prefix, width);
    writer.put(EOF_CODE, width);
    writer.finish()
}

struct BitReader<'a> {
    src: &'a [u8],
    pos: usize,
    bit: u32,
}

impl BitReader<'_> {
    /// Reads the next `width`-bit code. Returns `None` when the C reader would
    /// stop: it needs four readable bytes at the current position.
    fn next(&mut self, width: u32) -> Option<u32> {
        let len = self.src.len() as isize;
        if self.pos as isize > len - 3 {
            return None;
        }
        let mut word: u32 = 0;
        for i in 0..4 {
            let byte = self.src.get(self.pos + i).copied().unwrap_or(0);
            word |= u32::from(byte) << (8 * i);
        }
        let mask = (1u32 << width) - 1;
        let code = (word >> self.bit) & mask;
        let total = self.bit + width;
        self.pos += (total / 8) as usize;
        self.bit = total % 8;
        Some(code)
    }
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    bit: u32,
}

impl BitWriter {
    fn put(&mut self, code: u32, width: u32) {
        for i in 0..width {
            if self.bit == 0 {
                self.out.push(0);
            }
            if (code >> i) & 1 == 1 {
                let last = self.out.len() - 1;
                self.out[last] |= 1 << self.bit;
            }
            self.bit = (self.bit + 1) % 8;
        }
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// Encoder dictionary as a sibling-linked trie. Entry `p` holds the first
/// child code of prefix `p`; each child holds its suffix byte and the next
/// sibling.
struct EncodeTable {
    first_child: Vec<u32>,
    next_sibling: Vec<u32>,
    suffix: Vec<u8>,
}

impl EncodeTable {
    fn new() -> Self {
        let mut t = EncodeTable {
            first_child: vec![NONE; MAX_CODES as usize],
            next_sibling: vec![NONE; MAX_CODES as usize],
            suffix: vec![0; MAX_CODES as usize],
        };
        t.clear();
        t
    }

    fn clear(&mut self) {
        self.first_child.fill(NONE);
        self.next_sibling.fill(NONE);
    }

    /// `Ok(code)` when `(prefix, byte)` is already defined. `Err(tail)` gives
    /// the last sibling in the chain (or `prefix` when it has no children),
    /// which is where a new child attaches.
    fn lookup(&self, prefix: u32, byte: u8) -> std::result::Result<u32, u32> {
        let mut child = self.first_child[prefix as usize];
        if child == NONE {
            return Err(prefix);
        }
        loop {
            if self.suffix[child as usize] == byte {
                return Ok(child);
            }
            let next = self.next_sibling[child as usize];
            if next == NONE {
                return Err(child);
            }
            child = next;
        }
    }

    fn insert(&mut self, prefix: u32, tail: u32, byte: u8, code: u32) {
        let c = code as usize;
        self.suffix[c] = byte;
        self.first_child[c] = NONE;
        self.next_sibling[c] = NONE;
        if self.first_child[prefix as usize] == NONE && tail == prefix {
            self.first_child[prefix as usize] = code;
        } else {
            self.next_sibling[tail as usize] = code;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pseudo_random(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed;
        (0..len)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (state >> 16) as u8
            })
            .collect()
    }

    #[test]
    fn round_trip_small_and_repetitive() {
        for input in [
            b"A".to_vec(),
            b"ABABABABABABABABABAB".to_vec(),
            vec![0u8; 5000],
            b"the quick brown fox jumps over the lazy dog".repeat(40),
        ] {
            let packed = compress(&input);
            assert_eq!(decompress(&packed, input.len()).unwrap(), input);
        }
    }

    #[test]
    fn round_trip_forces_table_growth_and_clears() {
        // Enough distinct phrases to grow past 9, 10, 11 and 12 bits and to
        // trigger at least one table reset.
        let input = pseudo_random(400_000, 7);
        let packed = compress(&input);
        assert_eq!(decompress(&packed, input.len()).unwrap(), input);
    }

    #[test]
    fn empty_input_is_empty_stream() {
        assert!(compress(&[]).is_empty());
        assert!(decompress(&[], 0).unwrap().is_empty());
    }

    #[test]
    fn decoder_stops_at_truncated_input() {
        let input = pseudo_random(1000, 3);
        let packed = compress(&input);
        let partial = decompress(&packed[..packed.len() / 2], input.len()).unwrap();
        assert!(partial.len() < input.len());
        assert_eq!(&partial[..], &input[..partial.len()]);
    }

    #[test]
    fn hand_built_stream_decodes() {
        // Clear, 'A', 'B', code 258 (= "AB" via prefix), EOF, all at 9 bits.
        let mut w = BitWriter::default();
        for code in [CLEAR_CODE, 65, 66, 258, EOF_CODE] {
            w.put(code, 9);
        }
        let stream = w.finish();
        assert_eq!(decompress(&stream, 4).unwrap(), b"ABAB".to_vec());
    }

    #[test]
    fn invalid_codes_are_errors_not_hangs() {
        // Clear, 511, 511: the second 511 names the entry being defined and
        // loops on itself if it is accepted.
        let mut w = BitWriter::default();
        for code in [CLEAR_CODE, 511, 511, EOF_CODE, EOF_CODE] {
            w.put(code, 9);
        }
        assert!(decompress(&w.finish(), 1 << 20).is_err());

        // A code past the free slot is never valid.
        let mut w = BitWriter::default();
        for code in [CLEAR_CODE, 65, 300, EOF_CODE, EOF_CODE] {
            w.put(code, 9);
        }
        assert!(decompress(&w.finish(), 1 << 20).is_err());

        // Padding codes keep the final bad code inside the four-byte read window
        // that the C reader requires.
        // A non-literal directly after a clear is never valid.
        let mut w = BitWriter::default();
        for code in [CLEAR_CODE, 300, EOF_CODE, EOF_CODE] {
            w.put(code, 9);
        }
        assert!(decompress(&w.finish(), 1 << 20).is_err());
    }

    #[test]
    fn output_is_capped_at_the_limit() {
        let input = vec![7u8; 10_000];
        let packed = compress(&input);
        assert!(decompress(&packed, input.len() - 1).is_err());
        assert_eq!(decompress(&packed, input.len()).unwrap(), input);
    }
}
