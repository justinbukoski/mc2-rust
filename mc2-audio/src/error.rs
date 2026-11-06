//! Errors of the audio layer.

use mc2_formats::FormatError;
use mc2_platform::audio::WavError;
use std::fmt;

#[derive(Debug)]
pub enum AudioError {
    /// Container or text format problem.
    Format(FormatError),
    /// A WAV image inside a packet could not be decoded.
    Wav { what: String, error: WavError },
    /// A data file the game cannot run without is absent.
    Missing(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Format(e) => write!(f, "{e}"),
            AudioError::Wav { what, error } => write!(f, "{what}: {error:?}"),
            AudioError::Missing(name) => write!(f, "missing audio data file {name}"),
        }
    }
}

impl std::error::Error for AudioError {}

impl From<FormatError> for AudioError {
    fn from(e: FormatError) -> Self {
        AudioError::Format(e)
    }
}

pub type Result<T> = std::result::Result<T, AudioError>;
