//! MechCommander 2 container and text formats.
//!
//! Readers, and writers where the C++ has them, for:
//! - [`fst`]: FST fast-file archives (`.fst`), zlib or LZW entries.
//! - [`lzw`]: the LZW codec used by LZD packets in PAK files.
//! - [`pak`]: PAK packet files (`.pak`), with a writer.
//! - [`ini`]: FIT/INI text (`FITini`/`FITend`), with a writer.
//! - [`csv`]: CSV tables, read only (the game cannot write CSV).
//! - [`vfs`]: the game's file lookup order (loose files, then archives).
//!
//! Multi-byte integers are read with explicit little-endian conversions.
//! Source provenance is kept in `rust/docs/porting-log.md`.

pub mod csv;
pub mod error;
pub mod fst;
pub mod ini;
pub mod lzw;
pub mod pak;
mod text;
pub mod vfs;

pub use error::{FormatError, Result};
