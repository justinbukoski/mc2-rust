// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! PAK packet files.
//!
//! Port of `mclib/packet.{h,cpp}` (`PacketFile`), read path and the
//! `writePacket` storage choice. LZD packets are LZW streams (see [`crate::lzw`]).
//! The shipped data confirms this; zlib packets (type 4) are zlib.
//!
//! Layout (little-endian):
//!
//! ```text
//! u32 0xFEEDFACE          version word; a checksummed file stores the byte
//!                         sum of bytes 4.. here instead (see `from_bytes`)
//! u32 first_packet_offset  = 8 + 4 * count
//! u32 table[count]         (storage type << 29) | byte offset of the packet
//! packets...               each starts at its table offset
//! ```
//!
//! A packet runs from its offset to the next table offset (or end of file
//! for the last packet). Compressed packets (LZD, ZLIB) begin with a `u32`
//! unpacked size. Type 7 (NUL) packets are empty. Type 3 (Huffman) was
//! removed from the game and is rejected.

use crate::error::{FormatError, Result};
use crate::fst::inflate_bounded;
use crate::lzw;
use std::io::Write;
use std::path::Path;

/// Version word that starts an ordinary packet file.
pub const PACKET_FILE_VERSION: u32 = 0xFEED_FACE;

const FORMAT: &str = "PAK";
const TYPE_SHIFT: u32 = 29;
const OFFSET_MASK: u32 = (1 << TYPE_SHIFT) - 1;
const TABLE_START: usize = 8;
const COMPRESSED_HEADER: usize = 4;

/// Storage form of one packet (the top three bits of its table word).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageType {
    /// Stored as-is.
    Raw,
    /// File within file: read like `Raw`.
    Fwf,
    /// LZW stream after a `u32` unpacked size.
    Lzd,
    /// Huffman. Recognised but unsupported.
    Huffman,
    /// zlib stream after a `u32` unpacked size.
    Zlib,
    /// Empty packet.
    Nul,
    /// Any other value (5 and 6 are unused by the game).
    Reserved(u8),
}

impl StorageType {
    /// Decodes the three type bits.
    pub fn from_bits(bits: u8) -> Self {
        match bits {
            0x00 => StorageType::Raw,
            0x01 => StorageType::Fwf,
            0x02 => StorageType::Lzd,
            0x03 => StorageType::Huffman,
            0x04 => StorageType::Zlib,
            0x07 => StorageType::Nul,
            other => StorageType::Reserved(other),
        }
    }

    /// The three type bits as stored in the table.
    pub fn bits(self) -> u8 {
        match self {
            StorageType::Raw => 0x00,
            StorageType::Fwf => 0x01,
            StorageType::Lzd => 0x02,
            StorageType::Huffman => 0x03,
            StorageType::Zlib => 0x04,
            StorageType::Nul => 0x07,
            StorageType::Reserved(b) => b,
        }
    }
}

/// Location and sizes of one packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketInfo {
    /// Index in the table.
    pub index: usize,
    /// Storage form.
    pub storage: StorageType,
    /// Byte offset of the packet in the file.
    pub offset: u32,
    /// Bytes the packet occupies on disk, including any compression header.
    pub stored_size: u32,
    /// Size after decompression.
    pub unpacked_size: u32,
}

/// Parsed packet file over an in-memory image.
#[derive(Debug, Clone)]
pub struct PakFile {
    data: Vec<u8>,
    table: Vec<u32>,
    /// Byte offset where packet data may start: the end of the header and table.
    data_start: usize,
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl PakFile {
    /// Opens a packet file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// Parses the header and table. Files shorter than 12 bytes have no
    /// packets, matching `PacketFile::afterOpen`.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if data.len() < 12 {
            return Ok(Self {
                data,
                table: Vec::new(),
                data_start: 0,
            });
        }
        let first = le_u32(&data, 0);
        if first != PACKET_FILE_VERSION {
            // Checksummed file: the first word is the byte sum of bytes 4..
            let sum = data[4..]
                .iter()
                .fold(0u32, |acc, &b| acc.wrapping_add(u32::from(b)));
            if sum != first {
                return Err(FormatError::Corrupt {
                    format: FORMAT,
                    detail: format!("checksum {first:#010x} does not match {sum:#010x}"),
                });
            }
        }
        let first_offset = le_u32(&data, 4) as usize;
        if !first_offset.is_multiple_of(4) {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("first packet offset {first_offset} is not a multiple of 4"),
            });
        }
        let count = (first_offset / 4)
            .checked_sub(2)
            .ok_or_else(|| FormatError::Corrupt {
                format: FORMAT,
                detail: format!("first packet offset {first_offset} is too small"),
            })?;
        if first_offset > data.len() || TABLE_START + 4 * count > data.len() {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("table of {count} entries runs past end of file"),
            });
        }
        let table = (0..count)
            .map(|i| le_u32(&data, TABLE_START + 4 * i))
            .collect();
        Ok(Self {
            data,
            table,
            data_start: first_offset,
        })
    }

    /// Number of packets in the table.
    pub fn packet_count(&self) -> usize {
        self.table.len()
    }

    /// Total file size in bytes.
    pub fn file_len(&self) -> usize {
        self.data.len()
    }

    /// Location, storage type and sizes of packet `index`.
    pub fn info(&self, index: usize) -> Result<PacketInfo> {
        let word = *self.table.get(index).ok_or_else(|| FormatError::NotFound {
            what: format!("packet {index} (file has {})", self.table.len()),
        })?;
        let storage = StorageType::from_bits((word >> TYPE_SHIFT) as u8);
        let offset = word & OFFSET_MASK;
        let next = match self.table.get(index + 1) {
            Some(&w) => (w & OFFSET_MASK) as i64,
            None => self.data.len() as i64,
        };
        let stored = next - i64::from(offset);
        if offset == 0 || stored < 0 || offset as i64 + stored > self.data.len() as i64 {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!(
                    "packet {index} at offset {offset} with size {stored} is out of range"
                ),
            });
        }
        if (offset as usize) < self.data_start {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!(
                    "packet {index} at offset {offset} lies inside the header or table"
                ),
            });
        }
        let stored_size = stored as u32;
        let unpacked_size = match storage {
            StorageType::Raw | StorageType::Fwf => stored_size,
            StorageType::Lzd | StorageType::Zlib => {
                if (stored_size as usize) < COMPRESSED_HEADER {
                    return Err(FormatError::Corrupt {
                        format: FORMAT,
                        detail: format!("compressed packet {index} is smaller than its header"),
                    });
                }
                le_u32(&self.data, offset as usize)
            }
            StorageType::Nul => 0,
            StorageType::Huffman => {
                return Err(FormatError::Unsupported {
                    format: FORMAT,
                    detail: format!("Huffman packet {index}"),
                })
            }
            StorageType::Reserved(bits) => {
                return Err(FormatError::Unsupported {
                    format: FORMAT,
                    detail: format!("packet {index} has storage type {bits}"),
                })
            }
        };
        Ok(PacketInfo {
            index,
            storage,
            offset,
            stored_size,
            unpacked_size,
        })
    }

    /// Stored bytes of packet `index`, including any compression header.
    pub fn stored_bytes(&self, index: usize) -> Result<Vec<u8>> {
        let info = self.info(index)?;
        let start = info.offset as usize;
        Ok(self.data[start..start + info.stored_size as usize].to_vec())
    }

    /// Decompressed contents of packet `index`. The unpacked size from the
    /// header bounds the output, and a mismatch is an error.
    pub fn read_packet(&self, index: usize) -> Result<Vec<u8>> {
        let info = self.info(index)?;
        let start = info.offset as usize;
        let end = start + info.stored_size as usize;
        let expected = info.unpacked_size as usize;
        let out = match info.storage {
            StorageType::Raw | StorageType::Fwf => self.data[start..end].to_vec(),
            StorageType::Nul => Vec::new(),
            StorageType::Lzd => {
                let body = &self.data[start + COMPRESSED_HEADER..end];
                lzw::decompress(body, expected)?
            }
            StorageType::Zlib => {
                let body = &self.data[start + COMPRESSED_HEADER..end];
                inflate_bounded(body, expected, FORMAT, &format!("packet {index}"))?
            }
            StorageType::Huffman | StorageType::Reserved(_) => unreachable!("rejected by info()"),
        };
        if out.len() != expected {
            return Err(FormatError::Decompress {
                format: FORMAT,
                detail: format!(
                    "packet {index} produced {} bytes, header says {}",
                    out.len(),
                    expected
                ),
            });
        }
        Ok(out)
    }
}

/// How [`PakWriter::write_packet`] stores a packet. Mirrors the
/// `ANY_PACKET_TYPE`, `STORAGE_TYPE_RAW` and `STORAGE_TYPE_ZLIB` choices of
/// `PacketFile::writePacket`. The C++ `LZD` request is written as zlib.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketPolicy {
    /// zlib when it is smaller than the raw bytes, otherwise raw.
    Auto,
    /// Always raw.
    Raw,
    /// Always zlib.
    Zlib,
}

/// Builds a packet file in memory. Packets may be written in any order;
/// packets never written become NUL packets.
#[derive(Debug, Clone)]
pub struct PakWriter {
    packets: Vec<Option<(StorageType, Vec<u8>)>>,
}

impl PakWriter {
    /// Creates a writer for a table of `count` packets.
    pub fn new(count: usize) -> Self {
        Self {
            packets: vec![None; count],
        }
    }

    /// Stores packet `index` with the given policy.
    pub fn write_packet(&mut self, index: usize, data: &[u8], policy: PacketPolicy) -> Result<()> {
        let slot = self
            .packets
            .get_mut(index)
            .ok_or_else(|| FormatError::NotFound {
                what: format!("packet slot {index}"),
            })?;
        let zlib_body = |data: &[u8]| -> Result<Vec<u8>> {
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data)?;
            Ok(enc.finish()?)
        };
        *slot = Some(match policy {
            PacketPolicy::Raw => (StorageType::Raw, data.to_vec()),
            PacketPolicy::Zlib => {
                let mut body = (data.len() as u32).to_le_bytes().to_vec();
                body.extend(zlib_body(data)?);
                (StorageType::Zlib, body)
            }
            PacketPolicy::Auto => {
                let packed = zlib_body(data)?;
                if packed.len() < data.len() {
                    let mut body = (data.len() as u32).to_le_bytes().to_vec();
                    body.extend(packed);
                    (StorageType::Zlib, body)
                } else {
                    (StorageType::Raw, data.to_vec())
                }
            }
        });
        Ok(())
    }

    /// Serialises the file image.
    pub fn finish(self) -> Vec<u8> {
        let count = self.packets.len();
        let header_len = TABLE_START + 4 * count;
        let mut offsets = vec![0u32; count];
        let mut at = header_len;
        for (i, slot) in self.packets.iter().enumerate() {
            if let Some((_, body)) = slot {
                offsets[i] = at as u32;
                at += body.len();
            }
        }
        let file_end = at as u32;
        // Unwritten packets point at the start of the next written packet
        // (or the end of the file), as PacketFile::atClose arranges.
        let mut end = file_end;
        for i in (0..count).rev() {
            match self.packets[i] {
                Some(_) => end = offsets[i],
                None => offsets[i] = end,
            }
        }
        let mut out = Vec::with_capacity(at);
        out.extend_from_slice(&PACKET_FILE_VERSION.to_le_bytes());
        out.extend_from_slice(&(header_len as u32).to_le_bytes());
        for (i, slot) in self.packets.iter().enumerate() {
            let storage = slot.as_ref().map_or(StorageType::Nul, |(s, _)| *s);
            let word = (u32::from(storage.bits()) << TYPE_SHIFT) | offsets[i];
            out.extend_from_slice(&word.to_le_bytes());
        }
        for (_, body) in self.packets.into_iter().flatten() {
            out.extend(body);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_round_trip_all_policies_and_nul() {
        let text = b"packet text ".repeat(200);
        let mut w = PakWriter::new(5);
        w.write_packet(0, &text, PacketPolicy::Auto).unwrap();
        w.write_packet(1, b"tiny", PacketPolicy::Auto).unwrap();
        w.write_packet(2, b"raw bytes", PacketPolicy::Raw).unwrap();
        w.write_packet(4, &text, PacketPolicy::Zlib).unwrap();
        let pak = PakFile::from_bytes(w.finish()).unwrap();
        assert_eq!(pak.packet_count(), 5);
        assert_eq!(pak.info(0).unwrap().storage, StorageType::Zlib);
        assert_eq!(pak.info(1).unwrap().storage, StorageType::Raw);
        assert_eq!(pak.info(3).unwrap().storage, StorageType::Nul);
        assert_eq!(pak.info(3).unwrap().unpacked_size, 0);
        assert_eq!(pak.read_packet(0).unwrap(), text);
        assert_eq!(pak.read_packet(1).unwrap(), b"tiny".to_vec());
        assert_eq!(pak.read_packet(2).unwrap(), b"raw bytes".to_vec());
        assert!(pak.read_packet(3).unwrap().is_empty());
        assert_eq!(pak.read_packet(4).unwrap(), text);
        assert!(matches!(pak.info(5), Err(FormatError::NotFound { .. })));
    }

    #[test]
    fn header_layout_is_little_endian() {
        let mut w = PakWriter::new(1);
        w.write_packet(0, b"abc", PacketPolicy::Raw).unwrap();
        let img = w.finish();
        assert_eq!(&img[0..4], &[0xCE, 0xFA, 0xED, 0xFE]);
        assert_eq!(&img[4..8], &[12, 0, 0, 0]);
        assert_eq!(&img[8..12], &[12, 0, 0, 0]);
        assert_eq!(&img[12..], b"abc");
    }

    #[test]
    fn checksummed_file_is_accepted() {
        let mut w = PakWriter::new(1);
        w.write_packet(0, b"hello", PacketPolicy::Raw).unwrap();
        let mut img = w.finish();
        let sum = img[4..]
            .iter()
            .fold(0u32, |a, &b| a.wrapping_add(u32::from(b)));
        img[0..4].copy_from_slice(&sum.to_le_bytes());
        let pak = PakFile::from_bytes(img).unwrap();
        assert_eq!(pak.read_packet(0).unwrap(), b"hello".to_vec());
    }

    #[test]
    fn huffman_and_bad_checksum_are_rejected() {
        let mut w = PakWriter::new(1);
        w.write_packet(0, b"hello", PacketPolicy::Raw).unwrap();
        let mut img = w.finish();
        img[8..12].copy_from_slice(&(3u32 << 29 | 12).to_le_bytes());
        let pak = PakFile::from_bytes(img.clone()).unwrap();
        assert!(matches!(
            pak.read_packet(0),
            Err(FormatError::Unsupported { .. })
        ));

        img[0] = 1; // no longer the version word, so the checksum check runs
        assert!(matches!(
            PakFile::from_bytes(img),
            Err(FormatError::Corrupt { .. })
        ));
    }

    #[test]
    fn short_file_has_no_packets() {
        let pak = PakFile::from_bytes(vec![1, 2, 3]).unwrap();
        assert_eq!(pak.packet_count(), 0);
    }
}
