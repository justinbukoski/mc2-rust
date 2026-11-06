// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! FST "fast file" archives.
//!
//! Port of `mclib/ffile.{h,cpp}`, `mclib/fastfile.{h,cpp}` and `mclib/ffent.h`.
//!
//! Layout (all integers little-endian):
//!
//! ```text
//! u32 version      0xFADDECAF or 0xCADDECAF
//! u32 count
//! count x entry    { u32 offset, u32 packed_size, u32 real_size, u32 hash,
//!                    char name[250] (NUL padded) }   // 266 bytes each
//! payloads         stored at the absolute offsets in the entry table
//! ```
//!
//! Payload codec. `0xCADDECAF` archives hold zlib streams (`uncompress`).
//! `0xFADDECAF` archives may hold zlib streams (the `LINUX_BUILD` `LZDecomp`
//! path) or LZW streams (the Windows assembly path). The shipped archives are
//! zlib. When a payload starts with a zlib header it is inflated, and LZW is the
//! fallback if that fails. Every decode is capped at `real_size`.
//!
//! Lookup follows `FastFileFind` and `FastFile::openFast`. The name is
//! lowercased, its `elfHash` must equal the stored hash, and the stored name
//! must match case-insensitively. `\` and `/` are different characters here,
//! as in the C++ (the backslash retry is `#if 0`).

use crate::error::{FormatError, Result};
use crate::lzw;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Version word of archives whose payloads are zlib (`uncompress`).
pub const FST_VERSION_ZLIB: u32 = 0xCADD_ECAF;
/// Version word of LZ archives. Payloads are zlib or LZW (see module docs).
pub const FST_VERSION_LZ: u32 = 0xFADD_ECAF;

const HEADER_LEN: u64 = 8;
const ENTRY_LEN: usize = 266;
const NAME_FIELD_LEN: usize = 250;
const FORMAT: &str = "FST";

/// Version word of the archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastFileKind {
    /// `0xCADDECAF`: payloads are zlib streams.
    Zlib,
    /// `0xFADDECAF`: payloads are zlib or LZW streams, decided per payload.
    Lz,
}

/// One table entry of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastFileEntry {
    /// Name as stored (lossy UTF-8, NUL padding removed).
    pub name: String,
    /// Absolute byte offset of the stored payload.
    pub offset: u32,
    /// Stored (compressed) size in bytes.
    pub packed_size: u32,
    /// Uncompressed size in bytes.
    pub real_size: u32,
    /// `elfHash` value stored in the table.
    pub hash: u32,
}

/// Open archive: parsed table plus a seekable reader for the payloads.
#[derive(Debug)]
pub struct FastFile<R> {
    reader: R,
    kind: FastFileKind,
    file_len: u64,
    entries: Vec<FastFileEntry>,
    /// Entry indices keyed by stored hash, in table order.
    index: HashMap<u32, Vec<usize>>,
}

/// The game's `elfHash` (`elfHash` in `fastfile.cpp`).
///
/// The C code uses `unsigned long`, which is 64-bit on macOS. Only the low 32
/// bits are returned, and those match the 32-bit computation. `char` is signed,
/// so bytes at or above 0x80 are sign-extended before they are added.
pub fn elf_hash(name: &[u8]) -> u32 {
    let mut h: u32 = 0;
    for &b in name {
        if b == 0 {
            break;
        }
        h = (h << 4).wrapping_add(i32::from(b as i8) as u32);
        let g = h & 0xF000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

impl FastFile<File> {
    /// Opens an archive from disk and parses its table.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_reader(File::open(path)?)
    }
}

impl<R: Read + Seek> FastFile<R> {
    /// Parses the header and entry table from any seekable reader.
    pub fn from_reader(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        if file_len < HEADER_LEN {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("{file_len} bytes is shorter than the 8-byte header"),
            });
        }
        let mut header = [0u8; 8];
        reader.read_exact(&mut header)?;
        let version = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let count = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        let kind = match version {
            FST_VERSION_LZ => FastFileKind::Lz,
            FST_VERSION_ZLIB => FastFileKind::Zlib,
            other => {
                return Err(FormatError::BadSignature {
                    format: FORMAT,
                    found: format!("{other:#010x}"),
                })
            }
        };
        // The table must fit in the file before it is allocated.
        let table_len = count
            .checked_mul(ENTRY_LEN)
            .filter(|&n| HEADER_LEN + n as u64 <= file_len)
            .ok_or_else(|| FormatError::Corrupt {
                format: FORMAT,
                detail: format!("entry table of {count} entries runs past end of file"),
            })?;
        let mut table = vec![0u8; table_len];
        reader.read_exact(&mut table)?;

        let mut entries = Vec::with_capacity(count);
        let mut index: HashMap<u32, Vec<usize>> = HashMap::new();
        let (records, _) = table.as_chunks::<ENTRY_LEN>();
        for (i, rec) in records.iter().enumerate() {
            let name_field = &rec[16..16 + NAME_FIELD_LEN];
            let name_len = name_field
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(NAME_FIELD_LEN);
            let entry = FastFileEntry {
                offset: le_u32(rec, 0),
                packed_size: le_u32(rec, 4),
                real_size: le_u32(rec, 8),
                hash: le_u32(rec, 12),
                name: String::from_utf8_lossy(&name_field[..name_len]).into_owned(),
            };
            index.entry(entry.hash).or_default().push(i);
            entries.push(entry);
        }
        Ok(Self {
            reader,
            kind,
            file_len,
            entries,
            index,
        })
    }

    /// Version word kind of the archive.
    pub fn kind(&self) -> FastFileKind {
        self.kind
    }

    /// All table entries, in table order.
    pub fn entries(&self) -> &[FastFileEntry] {
        &self.entries
    }

    /// Index of the first entry matching `name` as `FastFileFind` does: the
    /// `elfHash` of the lowercased name must equal the stored hash, and the
    /// stored name must match case-insensitively.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        let hash = elf_hash(name.to_ascii_lowercase().as_bytes());
        self.index
            .get(&hash)?
            .iter()
            .copied()
            .find(|&i| self.entries[i].name.eq_ignore_ascii_case(name))
    }

    /// Entry matching `name`.
    pub fn find(&self, name: &str) -> Option<&FastFileEntry> {
        self.index_of(name).map(|i| &self.entries[i])
    }

    /// Stored (compressed) bytes of entry `index`. The payload must lie inside
    /// the archive, which is checked before anything is allocated.
    pub fn read_packed(&mut self, index: usize) -> Result<Vec<u8>> {
        let entry = self.entry_at(index)?.clone();
        let end = u64::from(entry.offset) + u64::from(entry.packed_size);
        if end > self.file_len {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("payload of {} runs past the end of the archive", entry.name),
            });
        }
        self.reader.seek(SeekFrom::Start(u64::from(entry.offset)))?;
        let mut buf = vec![0u8; entry.packed_size as usize];
        self.reader.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Decompressed bytes of entry `index`, checked against `real_size`.
    pub fn read_index(&mut self, index: usize) -> Result<Vec<u8>> {
        let entry = self.entry_at(index)?.clone();
        let packed = self.read_packed(index)?;
        let expected = entry.real_size as usize;
        let out = if packed.is_empty() {
            Vec::new()
        } else {
            decode_payload(self.kind, &packed, expected, &entry.name)?
        };
        if out.len() != expected {
            return Err(FormatError::Decompress {
                format: FORMAT,
                detail: format!(
                    "{}: produced {} bytes, table says {}",
                    entry.name,
                    out.len(),
                    entry.real_size
                ),
            });
        }
        Ok(out)
    }

    /// Decompressed bytes of the entry named `name`, or `None` when absent.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        match self.index_of(name) {
            Some(i) => self.read_index(i).map(Some),
            None => Ok(None),
        }
    }

    fn entry_at(&self, index: usize) -> Result<&FastFileEntry> {
        self.entries
            .get(index)
            .ok_or_else(|| FormatError::NotFound {
                what: format!("archive entry #{index}"),
            })
    }
}

/// Decodes one payload. For `Lz` archives, a zlib header selects inflate with
/// LZW as the fallback. Otherwise the payload is LZW.
fn decode_payload(
    kind: FastFileKind,
    packed: &[u8],
    expected: usize,
    name: &str,
) -> Result<Vec<u8>> {
    match kind {
        FastFileKind::Zlib => inflate_bounded(packed, expected, FORMAT, name),
        FastFileKind::Lz => {
            let mut first_error = None;
            if looks_like_zlib(packed) {
                match inflate_bounded(packed, expected, FORMAT, name) {
                    Ok(out) if out.len() == expected => return Ok(out),
                    Ok(_) => {}
                    Err(e) => first_error = Some(e),
                }
            }
            match lzw::decompress(packed, expected) {
                Ok(out) => Ok(out),
                Err(e) => Err(first_error.unwrap_or(e)),
            }
        }
    }
}

/// True when the bytes start with a valid zlib header (RFC 1950): deflate
/// method, window size at most 32 KiB, and a header check that is a multiple of 31.
pub(crate) fn looks_like_zlib(b: &[u8]) -> bool {
    match b {
        [cmf, flg, ..] => {
            cmf & 0x0F == 8 && cmf >> 4 <= 7 && (u16::from(*cmf) << 8 | u16::from(*flg)) % 31 == 0
        }
        _ => false,
    }
}

/// zlib inflate that refuses more than `limit` bytes of output. The initial
/// allocation is also bounded by what the input could plausibly expand to.
pub(crate) fn inflate_bounded(
    packed: &[u8],
    limit: usize,
    format: &'static str,
    what: &str,
) -> Result<Vec<u8>> {
    // The declared size never reserves memory. Start small and let the capped
    // read grow the buffer as real output arrives.
    const START_CAP: usize = 1 << 20;
    let mut out = Vec::with_capacity(limit.min(packed.len().saturating_mul(4)).min(START_CAP));
    let mut limited = flate2::read::ZlibDecoder::new(packed).take(limit as u64 + 1);
    limited
        .read_to_end(&mut out)
        .map_err(|e| FormatError::Decompress {
            format,
            detail: format!("{what}: {e}"),
        })?;
    if out.len() > limit {
        return Err(FormatError::Decompress {
            format,
            detail: format!("{what}: output exceeds the declared size of {limit} bytes"),
        });
    }
    Ok(out)
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    /// Builds an archive image in memory: header, table, payloads.
    fn build(version: u32, files: &[(&str, &[u8], Vec<u8>)]) -> Vec<u8> {
        let table_end = 8 + files.len() * ENTRY_LEN;
        let mut payload_at = table_end as u32;
        let mut table = Vec::new();
        let mut payloads = Vec::new();
        for (name, real, packed) in files {
            table.extend_from_slice(&payload_at.to_le_bytes());
            table.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            table.extend_from_slice(&(real.len() as u32).to_le_bytes());
            table.extend_from_slice(&elf_hash(name.to_ascii_lowercase().as_bytes()).to_le_bytes());
            let mut field = [0u8; NAME_FIELD_LEN];
            field[..name.len()].copy_from_slice(name.as_bytes());
            table.extend_from_slice(&field);
            payloads.extend_from_slice(packed);
            payload_at += packed.len() as u32;
        }
        let mut out = version.to_le_bytes().to_vec();
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        out.extend(table);
        out.extend(payloads);
        out
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(data).unwrap();
        enc.finish().unwrap()
    }

    #[test]
    fn elf_hash_known_values() {
        assert_eq!(elf_hash(b""), 0);
        assert_eq!(elf_hash(b"a"), 97);
        assert_eq!(elf_hash(b"abc"), 0x6783);
        // High-bit bytes sign-extend like a signed C char.
        assert_eq!(elf_hash(&[0xFF]), 0x0FFF_FF0F);
    }

    #[test]
    fn zlib_archive_round_trip_and_case_insensitive_lookup() {
        let data = b"hello fast file".to_vec();
        let image = build(FST_VERSION_ZLIB, &[("data/Hello.TXT", &data, zlib(&data))]);
        let mut ff = FastFile::from_reader(Cursor::new(image)).unwrap();
        assert_eq!(ff.kind(), FastFileKind::Zlib);
        assert_eq!(ff.read("DATA/hello.txt").unwrap(), Some(data.clone()));
        assert_eq!(ff.read("missing.txt").unwrap(), None);
        assert_eq!(ff.entries()[0].real_size as usize, data.len());
    }

    #[test]
    fn lz_archive_with_zlib_payload() {
        let data = b"zlib payload ".repeat(50);
        let image = build(FST_VERSION_LZ, &[("a.bin", &data, zlib(&data))]);
        let mut ff = FastFile::from_reader(Cursor::new(image)).unwrap();
        assert_eq!(ff.kind(), FastFileKind::Lz);
        assert_eq!(ff.read_index(0).unwrap(), data);
    }

    #[test]
    fn lz_archive_with_lzw_payload_reads_through_fallback() {
        // Windows-path archives store LZW streams under the same version word.
        let data = b"lzw payload ".repeat(50);
        let packed = lzw::compress(&data);
        assert!(!looks_like_zlib(&packed));
        let image = build(FST_VERSION_LZ, &[("a.bin", &data, packed)]);
        let mut ff = FastFile::from_reader(Cursor::new(image)).unwrap();
        assert_eq!(ff.read_index(0).unwrap(), data);
    }

    #[test]
    fn empty_entry_and_first_match_wins() {
        let image = build(
            FST_VERSION_LZ,
            &[
                ("x.dat", b"", Vec::new()),
                ("X.DAT", b"second", zlib(b"second")),
            ],
        );
        let mut ff = FastFile::from_reader(Cursor::new(image)).unwrap();
        assert_eq!(ff.index_of("x.dat"), Some(0));
        assert_eq!(ff.read("x.DAT").unwrap(), Some(Vec::new()));
    }

    #[test]
    fn size_mismatch_is_an_error() {
        let mut image = build(FST_VERSION_LZ, &[("a", b"abcd", zlib(b"abcd"))]);
        // Patch real_size (entry field at 8 + 8) to 5.
        image[16..20].copy_from_slice(&5u32.to_le_bytes());
        let mut ff = FastFile::from_reader(Cursor::new(image)).unwrap();
        assert!(matches!(
            ff.read_index(0),
            Err(FormatError::Decompress { .. })
        ));
    }

    #[test]
    fn bad_version_is_rejected() {
        let image = build(0x1234_5678, &[]);
        assert!(matches!(
            FastFile::from_reader(Cursor::new(image)),
            Err(FormatError::BadSignature { .. })
        ));
    }

    #[test]
    fn table_past_end_is_rejected() {
        let mut image = build(FST_VERSION_LZ, &[]);
        image[4..8].copy_from_slice(&1000u32.to_le_bytes());
        assert!(matches!(
            FastFile::from_reader(Cursor::new(image)),
            Err(FormatError::Corrupt { .. })
        ));
    }
}
