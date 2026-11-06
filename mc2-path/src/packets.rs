//! Access to already-decoded packet payloads and little-endian field reads.
//!
//! The C++ reads each packet with `PacketFile::readPacket` and then casts the bytes to
//! `#pragma pack(1)` structs. Here every field is read explicitly and little-endian,
//! and the packet length is checked against the expected layout. Container parsing
//! (the `.fst`/`.pak` headers, zlib/RAW storage) is the caller's job.

use std::fmt;

/// Something that yields the decoded payload of packet `index`.
pub trait PacketSource {
    fn packet(&self, index: usize) -> Option<&[u8]>;
}

impl PacketSource for [Vec<u8>] {
    fn packet(&self, index: usize) -> Option<&[u8]> {
        self.get(index).map(|v| v.as_slice())
    }
}

impl PacketSource for Vec<Vec<u8>> {
    fn packet(&self, index: usize) -> Option<&[u8]> {
        self.as_slice().packet(index)
    }
}

/// Errors from loading move data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The packet index is past the end of the source.
    MissingPacket { index: usize },
    /// A packet's byte length does not match the layout the C++ expects.
    BadPacketSize {
        index: usize,
        what: &'static str,
        expected: usize,
        actual: usize,
    },
    /// Global map version word is not `0x022523` (C++ sets `badLoad`).
    BadVersion { found: u32 },
    /// A count in the header disagrees with the packets that follow.
    Inconsistent {
        what: &'static str,
        expected: usize,
        actual: usize,
    },
    /// A value is out of range for its field (e.g. unknown area type, negative count).
    BadValue { what: &'static str, value: i64 },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::MissingPacket { index } => write!(f, "missing packet {index}"),
            LoadError::BadPacketSize {
                index,
                what,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "packet {index} ({what}): expected {expected} bytes, got {actual}"
                )
            }
            LoadError::BadVersion { found } => {
                write!(f, "global map version 0x{found:06X} (want 0x022523)")
            }
            LoadError::Inconsistent {
                what,
                expected,
                actual,
            } => {
                write!(f, "{what}: header says {expected}, packets give {actual}")
            }
            LoadError::BadValue { what, value } => write!(f, "{what}: bad value {value}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Fetch packet `index`, or report it missing.
pub fn get_packet(src: &dyn PacketSource, index: usize) -> Result<&[u8], LoadError> {
    src.packet(index).ok_or(LoadError::MissingPacket { index })
}

/// Fetch packet `index` and require exactly `len` bytes.
pub fn get_exact<'a>(
    src: &'a dyn PacketSource,
    index: usize,
    len: usize,
    what: &'static str,
) -> Result<&'a [u8], LoadError> {
    let b = get_packet(src, index)?;
    if b.len() != len {
        return Err(LoadError::BadPacketSize {
            index,
            what,
            expected: len,
            actual: b.len(),
        });
    }
    Ok(b)
}

pub fn u8_at(b: &[u8], off: usize) -> u8 {
    b[off]
}

pub fn i8_at(b: &[u8], off: usize) -> i8 {
    b[off] as i8
}

pub fn bool_at(b: &[u8], off: usize) -> bool {
    b[off] != 0
}

pub fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

pub fn i16_at(b: &[u8], off: usize) -> i16 {
    i16::from_le_bytes([b[off], b[off + 1]])
}

pub fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

pub fn i32_at(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Read a whole packet holding a single little-endian `i32` (the C++ `long` scalars).
pub fn read_i32_packet(
    src: &dyn PacketSource,
    index: usize,
    what: &'static str,
) -> Result<i32, LoadError> {
    Ok(i32_at(get_exact(src, index, 4, what)?, 0))
}
