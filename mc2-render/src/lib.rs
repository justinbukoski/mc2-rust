//! mc2-render: the game's render front end.
//!
//! Terrain mesh, texture selection, camera, light model, shape hierarchy and animation are
//! ported from the game logic in `mclib/` (see `rust/docs/porting-log.md`).  Drawing is
//! done the modern way on the GPU with per-pixel lighting instead of the 2001 CPU
//! transform and pre-lit vertices.

#![allow(clippy::chunks_exact_to_as_chunks, clippy::field_reassign_with_default)]

pub mod appearance;
pub mod artpack;
pub mod camera;
pub mod compare;
pub mod crater;
pub mod error;
pub mod flash;
pub mod fx;
pub mod game;
pub mod gpu;
pub mod light;
pub mod math;
pub mod mission;
pub mod objects;
pub mod populate;
pub mod present;
pub mod scene;
pub mod settings;
pub mod shape;
pub mod terrain;
pub mod terrain_lights;
pub mod terrain_tex;
pub mod transitions;
pub mod vertex_light;
pub mod vertex_light_compare;
pub mod visual;
pub mod weather;

pub use error::{RenderError, Result};
