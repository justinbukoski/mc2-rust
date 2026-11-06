// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Campaign and logistics state for the MechCommander 2 port.
//!
//! This crate is the game logic of the logistics screens, without the screens: the player's mech
//! inventory and pilots, C-Bills and purchases, mech variants and their legality rules, the
//! mission sequence and what each mission unlocks, the hand-over to and from a mission
//! ([`LoadSpec`], [`Outcome`]), and a versioned save format.
//!
//! Port of `code/logisticsdata.cpp`, `logisticsmech.cpp`, `logisticsvariant.cpp`,
//! `logisticscomponent.cpp`, `logisticspilot.cpp` and `logisticsmissioninfo.cpp`. Provenance and
//! deliberate deviations are listed in `rust/docs/porting-log.md`.
//!
//! Layout:
//! - [`data`]: [`GameData`], the static data (components, chassis, designer variants, pilots,
//!   campaign scripts, purchase lists) loaded once from the game files.
//! - [`component`], [`variant`], [`pilot`], [`mission_info`]: the model types.
//! - [`logistics`]: [`Logistics`], the mutable campaign state and all operations on it.
//! - [`interface`]: the mission hand-over types.
//! - [`results`]: the results flow after a mission (salvage, pilot review) and mission selection.
//! - [`save`]: the save format.
//!
//! Nothing in this crate opens a window or does I/O besides reading data files in
//! [`GameData::load`].

pub mod component;
pub mod data;
pub mod error;
pub mod interface;
pub mod logistics;
pub mod mission_info;
pub mod pilot;
pub mod results;
pub mod save;
pub mod strings;
#[cfg(test)]
mod testdata;
pub mod variant;

pub use data::{DataError, GameData};
pub use error::{LogisticsError, LogisticsResult};
pub use interface::{LoadSpec, MissionOptions, MissionResult, Outcome};
pub use logistics::{Logistics, MechKey, VariantKey};
pub use save::{SaveError, SaveFile};
