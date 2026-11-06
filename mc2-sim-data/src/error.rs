//! Error type for the static data model.

use mc2_formats::FormatError;
use std::fmt;

/// Failure while loading static game data.
#[derive(Debug)]
pub enum SimDataError {
    /// A container, text or table error from `mc2-formats`.
    Format(FormatError),
    /// The file parsed, but a required block, key, row or column is missing or malformed.
    Schema { file: String, detail: String },
    /// A file the loader needs is not in the data root or any mounted archive.
    MissingFile { name: String },
    /// A value is outside the range the game accepts (for example an unknown enum code).
    OutOfRange { file: String, detail: String },
}

/// Convenience alias used across the crate.
pub type Result<T> = std::result::Result<T, SimDataError>;

impl SimDataError {
    /// Shorthand for [`SimDataError::Schema`].
    pub fn schema(file: impl Into<String>, detail: impl Into<String>) -> Self {
        SimDataError::Schema {
            file: file.into(),
            detail: detail.into(),
        }
    }

    /// Shorthand for [`SimDataError::OutOfRange`].
    pub fn out_of_range(file: impl Into<String>, detail: impl Into<String>) -> Self {
        SimDataError::OutOfRange {
            file: file.into(),
            detail: detail.into(),
        }
    }
}

impl From<FormatError> for SimDataError {
    fn from(e: FormatError) -> Self {
        SimDataError::Format(e)
    }
}

impl fmt::Display for SimDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SimDataError::Format(e) => write!(f, "{e}"),
            SimDataError::Schema { file, detail } => write!(f, "{file}: {detail}"),
            SimDataError::MissingFile { name } => write!(f, "missing file {name}"),
            SimDataError::OutOfRange { file, detail } => {
                write!(f, "{file}: out of range: {detail}")
            }
        }
    }
}

impl std::error::Error for SimDataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SimDataError::Format(e) => Some(e),
            _ => None,
        }
    }
}
