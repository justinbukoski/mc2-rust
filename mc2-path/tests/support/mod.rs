// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Test-only container reader for `mission.fst` and the mission `.pak` files inside it.
//! Not part of the crate's API; `mc2-formats` will own this once it exists.
//!
//! Layout as observed in the shipped data (and matching the C++ `PacketFile`/`FastFile`):
//! - `.fst`: magic `0xFADDECAF`, u32 entry count at offset 4, then `count` records of
//!   `{u32 offset, u32 size, u32 realSize, u32 hash, char name[250]}` starting at offset 8.
//!   `size != realSize` means the entry is zlib-compressed.
//! - `.pak`: magic `0xFEEDFACE`, u32 `firstPacketOffset` at offset 4 (`count = first/4 - 2`),
//!   then `count` u32 table entries from offset 8. Top 3 bits: storage type (0 raw, 4 zlib,
//!   7 null); low 29 bits: byte offset. A zlib packet starts with its u32 unpacked size.

#![allow(dead_code)]

pub mod world;

use std::io::Read;
use std::path::PathBuf;

const FST_MAGIC: u32 = 0xFADD_ECAF;
const PAK_MAGIC: u32 = 0xFEED_FACE;
const FST_ENTRY_BYTES: usize = 266;
const OFFSET_MASK: u32 = (1 << 29) - 1;

/// `MC2_DATA` if set and non-empty (the shipped `gamedata/mc2exe` directory).
pub fn data_dir() -> Option<PathBuf> {
    std::env::var_os("MC2_DATA")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn inflate_zlib(data: &[u8], expect: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(expect);
    flate2::read::ZlibDecoder::new(data)
        .read_to_end(&mut out)
        .expect("zlib stream");
    assert_eq!(out.len(), expect, "zlib length mismatch");
    out
}

/// One entry of a `.fst` archive.
pub struct FstEntry {
    pub name: String,
    pub offset: usize,
    pub size: usize,
    pub real_size: usize,
}

/// Parsed `.fst` directory plus the raw archive bytes.
pub struct Fst {
    bytes: Vec<u8>,
    pub entries: Vec<FstEntry>,
}

impl Fst {
    pub fn open(path: &std::path::Path) -> Fst {
        let bytes = std::fs::read(path).expect("read fst");
        assert_eq!(u32_le(&bytes, 0), FST_MAGIC, "fst magic");
        let count = u32_le(&bytes, 4) as usize;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let off = 8 + i * FST_ENTRY_BYTES;
            let name_bytes = &bytes[off + 16..off + FST_ENTRY_BYTES];
            let end = name_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(name_bytes.len());
            entries.push(FstEntry {
                name: String::from_utf8_lossy(&name_bytes[..end]).into_owned(),
                offset: u32_le(&bytes, off) as usize,
                size: u32_le(&bytes, off + 4) as usize,
                real_size: u32_le(&bytes, off + 8) as usize,
            });
        }
        Fst { bytes, entries }
    }

    /// Decoded contents of an entry.
    pub fn read(&self, e: &FstEntry) -> Vec<u8> {
        let raw = &self.bytes[e.offset..e.offset + e.size];
        if e.size == e.real_size {
            raw.to_vec()
        } else {
            inflate_zlib(raw, e.real_size)
        }
    }
}

/// A decoded `.pak` whose packets are inflated on first access.
pub struct LazyPak {
    bytes: Vec<u8>,
    table: Vec<u32>,
    cache: Vec<std::cell::OnceCell<Vec<u8>>>,
}

impl LazyPak {
    pub fn new(bytes: Vec<u8>) -> LazyPak {
        assert_eq!(u32_le(&bytes, 0), PAK_MAGIC, "pak magic");
        let first = u32_le(&bytes, 4) as usize;
        let count = first / 4 - 2;
        let table: Vec<u32> = (0..count).map(|i| u32_le(&bytes, 8 + 4 * i)).collect();
        let cache = (0..count).map(|_| std::cell::OnceCell::new()).collect();
        LazyPak {
            bytes,
            table,
            cache,
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Storage type of packet `i` (0 raw, 2 LZ, 4 zlib, 7 null).
    pub fn kind(&self, i: usize) -> u32 {
        self.table[i] >> 29
    }

    fn decode(&self, i: usize) -> Vec<u8> {
        let kind = self.kind(i);
        let start = (self.table[i] & OFFSET_MASK) as usize;
        let end = if i + 1 < self.table.len() {
            (self.table[i + 1] & OFFSET_MASK) as usize
        } else {
            self.bytes.len()
        };
        let body = &self.bytes[start..end];
        match kind {
            0 => body.to_vec(),
            4 => {
                let unpacked = u32_le(body, 0) as usize;
                inflate_zlib(&body[4..], unpacked)
            }
            2 => {
                let unpacked = u32_le(body, 0) as usize;
                lzw_decode(&body[4..], unpacked)
            }
            7 => Vec::new(),
            other => panic!("unsupported packet storage type {other} in packet {i}"),
        }
    }
}

/// LZW as used by the Windows `LZDecomp` (move-side packets, storage type 2): LSB-first
/// codes of 9..=12 bits, 256 = clear, 257 = end, first free code 258. The width grows
/// when the next free code reaches 2^width (and stops at 12 bits).
fn lzw_decode(src: &[u8], expect: usize) -> Vec<u8> {
    const CLEAR: u32 = 256;
    const EOF: u32 = 257;
    let mut dict: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8]).collect();
    dict.push(Vec::new());
    dict.push(Vec::new());
    let mut width: u32 = 9;
    let mut max_index: usize = 512;
    let mut free: usize = 258;
    let mut old: Option<Vec<u8>> = None;
    let mut out: Vec<u8> = Vec::with_capacity(expect);
    let total_bits = src.len() * 8;
    let mut bitpos = 0usize;
    loop {
        if bitpos + width as usize > total_bits {
            break;
        }
        let mut code: u32 = 0;
        for k in 0..width as usize {
            let bit = (src[(bitpos + k) / 8] >> ((bitpos + k) % 8)) & 1;
            code |= (bit as u32) << k;
        }
        bitpos += width as usize;
        if code == EOF {
            break;
        }
        if code == CLEAR {
            dict.truncate(258);
            free = 258;
            width = 9;
            max_index = 512;
            old = None;
            continue;
        }
        let cur: Vec<u8> = if (code as usize) < free {
            dict[code as usize].clone()
        } else {
            let o = old.as_ref().expect("KwKwK code without a previous code");
            let mut v = o.clone();
            v.push(o[0]);
            v
        };
        if let Some(o) = old.take() {
            let mut entry = o;
            entry.push(cur[0]);
            if dict.len() > free {
                dict[free] = entry;
            } else {
                dict.push(entry);
            }
            free += 1;
            if free >= max_index && width < 12 {
                width += 1;
                max_index <<= 1;
            }
        }
        out.extend_from_slice(&cur);
        old = Some(cur);
    }
    assert_eq!(out.len(), expect, "lzw length mismatch");
    out
}

impl mc2_path::PacketSource for LazyPak {
    fn packet(&self, index: usize) -> Option<&[u8]> {
        if index >= self.cache.len() {
            return None;
        }
        Some(
            self.cache[index]
                .get_or_init(|| self.decode(index))
                .as_slice(),
        )
    }
}
