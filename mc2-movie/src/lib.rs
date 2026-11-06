//! Pure-Rust decoders for the MPEG-1 movies of MechCommander 2.
//!
//! Written clean-room from ISO/IEC 11172 (parts 1 to 3); see `rust/docs/porting-log.md` for the
//! provenance of every table.

pub mod audio;
mod bits;
pub mod demux;
mod error;
pub mod video;

pub use error::{MovieError, Result};

pub use audio::{AudioDecoder, AudioPcm, FrameHeader};
pub use video::{Frame, PictureType, SequenceInfo, VideoDecoder};
