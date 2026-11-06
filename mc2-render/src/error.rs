//! One error type for the render front end.

use std::fmt;

#[derive(Debug)]
pub enum RenderError {
    Format(mc2_formats::FormatError),
    Asset(mc2_assets::AssetError),
    /// A file the mission needs is not in the VFS.
    Missing(String),
    Io(std::io::Error),
    /// No usable GPU adapter, device creation failure, readback failure.
    Gpu(String),
    /// An input the render needs is absent, so the result would be wrong; the message names it.
    Unavailable(String),
    Other(String),
}

pub type Result<T> = std::result::Result<T, RenderError>;

/// An optional file: `None` when it is not in the data, the value when it loads, and the error when it is there
/// but fails.  The C++ looks for these and does without them (e.g. `FallForwardDam`, `.agl` clips), so only an
/// absent file may be left out; a broken one must not turn into a shape the appearance did not declare.
pub fn optional_file<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(RenderError::Missing(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// An optional key of a FIT block: `None` when the key is absent, the value when it parses, and the error when it
/// is present but malformed.
pub fn optional_key<T>(r: mc2_formats::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(mc2_formats::FormatError::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::Format(e) => write!(f, "{e}"),
            RenderError::Asset(e) => write!(f, "{e}"),
            RenderError::Missing(n) => write!(f, "missing game file: {n}"),
            RenderError::Io(e) => write!(f, "{e}"),
            RenderError::Gpu(e) => write!(f, "gpu: {e}"),
            RenderError::Unavailable(e) => write!(f, "{e}"),
            RenderError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<mc2_formats::FormatError> for RenderError {
    fn from(e: mc2_formats::FormatError) -> Self {
        RenderError::Format(e)
    }
}
impl From<mc2_assets::AssetError> for RenderError {
    fn from(e: mc2_assets::AssetError) -> Self {
        RenderError::Asset(e)
    }
}
impl From<std::io::Error> for RenderError {
    fn from(e: std::io::Error) -> Self {
        RenderError::Io(e)
    }
}
