//! MechCommander 2 texture pack tooling.
//!
//! * [`collect`] enumerates every texture the game can show and files it under the key the
//!   hi-res replacement hook looks up (`mc2_gfx::texture::texture_key` with the VFS path as the
//!   name).
//! * [`model`] is the manifest format of a dump and of a pack.
//! * [`verify`] checks a pack folder against a dump.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod bmp;
pub mod builtin;
pub mod collect;
pub mod model;
pub mod report;
pub mod sha256;
pub mod stats;
pub mod verify;
