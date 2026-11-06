//! Platform layer for the MechCommander 2 Rust port.
//!
//! * [`window`] - winit window + event loop, HiDPI, fullscreen toggle, raw window handle.
//! * [`viewport`] / [`input`] / [`keys`] - logical game screen mapping, keyboard and mouse state in
//!   game key codes and logical coordinates.
//! * [`audio`] - WAV/ADPCM decoding, channel mixer, optional cpal output device.
//! * [`movie`] - pure-Rust MPEG-1 cut-scene decoding and playback.
//! * [`clock`] - fixed-step game clock.
//! * [`paths`] - per-user config / save directories.
//!
//! Clean-room: written from winit/cpal documentation, the MPEG-1 standard, Microsoft's public API
//! headers and the game's callers; see `rust/docs/porting-log.md`.

pub mod audio;
pub mod clock;
pub mod input;
pub mod keys;
pub mod movie;
pub mod paths;
pub mod viewport;
pub mod window;

/// Public product display name, shared with packaging.
pub const PRODUCT_NAME: &str = env!("MC2_PRODUCT_NAME");

pub use winit;
