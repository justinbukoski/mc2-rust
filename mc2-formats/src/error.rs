//! Error type shared by every format reader and writer in this crate.

use std::fmt;

/// Failure while reading, writing or looking up game data.
#[derive(Debug)]
pub enum FormatError {
    /// Underlying I/O failure (open, read, seek).
    Io(std::io::Error),
    /// The leading magic or signature does not match the format.
    BadSignature { format: &'static str, found: String },
    /// Structure is truncated or internally inconsistent.
    Corrupt {
        format: &'static str,
        detail: String,
    },
    /// Decompression produced an error or a size that disagrees with the header.
    Decompress {
        format: &'static str,
        detail: String,
    },
    /// A named block, key, archive entry or packet was not found.
    NotFound { what: String },
    /// A caller-supplied buffer or element capacity is too small for the value.
    BufferTooSmall { what: &'static str },
    /// Storage form the game format defines but this port does not implement.
    Unsupported {
        format: &'static str,
        detail: String,
    },
    /// A name would resolve outside the data root (absolute, drive-qualified or `..`).
    InvalidPath { detail: String },
}

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, FormatError>;

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::Io(e) => write!(f, "I/O error: {e}"),
            FormatError::BadSignature { format, found } => {
                write!(f, "{format}: bad signature {found}")
            }
            FormatError::Corrupt { format, detail } => {
                write!(f, "{format}: corrupt data: {detail}")
            }
            FormatError::Decompress { format, detail } => {
                write!(f, "{format}: decompression failed: {detail}")
            }
            FormatError::NotFound { what } => write!(f, "not found: {what}"),
            FormatError::BufferTooSmall { what } => {
                write!(f, "{what} does not fit the caller's buffer")
            }
            FormatError::Unsupported { format, detail } => {
                write!(f, "{format}: unsupported: {detail}")
            }
            FormatError::InvalidPath { detail } => write!(f, "invalid path: {detail}"),
        }
    }
}

impl std::error::Error for FormatError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FormatError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for FormatError {
    fn from(e: std::io::Error) -> Self {
        FormatError::Io(e)
    }
}
