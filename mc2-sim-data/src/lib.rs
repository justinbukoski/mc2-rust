//! Static game data model and loaders for MechCommander 2.
//!
//! No simulation lives here. Every type is loaded from the shipped `.fit`, `.csv`
//! and `.pak` data through `mc2_formats::Vfs`, following the C++ loaders:
//! - [`objtype`]: object type packets (`data/objects/object2.pak`) and their typed classes.
//! - [`mech`]: chassis CSVs and mech/vehicle profiles.
//! - [`component`]: the master component and weapon list (`compbas.csv`).
//! - [`pilot`]: warrior (pilot) profiles.
//! - [`campaign`]: the campaign script and purchase lists.
//! - [`mission`]: mission `.fit` files, objectives included.
//! - [`gamesys`]: the `gamesys.fit` tuning tables (`Mission::init`, `loadGameSystem`).
//!
//! Provenance and deliberate deviations are kept in `rust/docs/porting-log.md`.

mod bounds;
pub mod campaign;
pub mod component;
pub mod error;
pub mod gamesys;
pub mod mech;
pub mod mission;
pub mod objtype;
pub mod pilot;

pub use error::{Result, SimDataError};
