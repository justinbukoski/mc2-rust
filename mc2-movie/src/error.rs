//! Errors for the movie decoders.

use std::fmt;

/// Everything that can go wrong while demuxing or decoding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MovieError {
    /// The data is not an MPEG program stream or elementary stream we understand.
    NotMpeg,
    /// The stream ended in the middle of a syntax element.
    Truncated,
    /// A syntax element holds a value the standard forbids.
    Syntax(&'static str),
    /// A legal feature this decoder does not implement.
    Unsupported(&'static str),
    /// A capability that exists in the spec but has no implementation in this crate yet.
    Unavailable(&'static str),
}

impl fmt::Display for MovieError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MovieError::NotMpeg => write!(f, "not an MPEG-1 stream"),
            MovieError::Truncated => write!(f, "stream truncated"),
            MovieError::Syntax(s) => write!(f, "invalid MPEG syntax: {s}"),
            MovieError::Unsupported(s) => write!(f, "unsupported MPEG feature: {s}"),
            MovieError::Unavailable(s) => write!(f, "not available yet: {s}"),
        }
    }
}

impl std::error::Error for MovieError {}

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, MovieError>;
