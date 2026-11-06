//! gosFX visual effects for the MechCommander 2 port.
//!
//! Ported from `mclib/gosfx/*`. Simulation (`Effect::Start`/`Execute`) is separate from drawing:
//! an effect updates over time and `Effect::draw` produces plain draw data for the renderer.

pub mod card;
pub mod cloud;
pub mod debris;
pub mod draw;
pub mod effect;
pub mod error;
pub mod fcurve;
pub mod library;
pub mod light;
pub mod math;
pub mod mlrshape;
pub mod render;
pub mod rng;
pub mod singleton;
pub mod spec;
pub mod state;
pub mod stream;
pub mod system;
pub mod tube;
pub mod weaponfx;

pub use error::{FxError, Result};
