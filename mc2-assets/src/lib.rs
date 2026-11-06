//! Field-by-field readers for MechCommander 2 runtime assets.
//!
//! gosFX support here is limited to its library header; the effects themselves are parsed by
//! the `mc2-fx` crate, where each class is walked according to its C++ `Load` implementation.
pub mod effect;
pub mod map;
pub mod reader;
pub mod tga;
pub mod tgl;

pub use reader::{AssetError, Reader};
pub mod stuff;
pub mod tgl_instance;
