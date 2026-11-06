// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mech and vehicle static data: chassis CSVs, mech and vehicle profiles.
//!
//! - [`layout`]: body and armor location tables (`code/mech.h`, `code/mech.cpp`).
//! - [`chassis`]: the chassis CSV `data/objects/<name>.csv`: chassis block, AllowComponents,
//!   torso yaw limits, variants, inventory rows and critical slots (`BattleMech::init(DWORD)`).
//! - [`profile`]: mech profile FIT `data/missions/profiles/pm*.fit` (`BattleMech::init(FitIniFile*)`).
//! - [`vehicle`]: ground vehicle object FIT (`GroundVehicleType`, `GroundVehicle::init`),
//!   `VehicleDynamics`, and the `GroundVehicleProfile` FIT (`engine.fit`, `pv*.fit`).
//!
//! Components are stored by their id number (`CompBas#` on chassis rows, `MasterID` in
//! profiles and object packets). They are not resolved here; the component master list
//! owns that.
//!
//! Lookup is case-insensitive through [`mc2_formats::vfs::Vfs`]. Paths follow
//! `system.cfg`: `objectPath` (`data/objects/`) and `profilePath`
//! (`data/missions/profiles/`), and the stem is lower-cased as the C++ does.

pub mod chassis;
pub mod layout;
pub mod profile;
pub mod vehicle;

use crate::error::{Result, SimDataError};
use mc2_formats::csv::CsvFile;
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;

pub use chassis::{AllowComponents, Chassis, ChassisDynamics, ChassisVariant, ItemRow, ItemTotals};
pub use layout::{ArmorLocation, BodyLocation};
pub use profile::{MechProfile, ProfileComponent, ProfileItem, ProfileLocation};
pub use vehicle::{GroundVehicleObject, VehicleDynamics, VehiclePart, VehicleProfile};

/// `objectPath` from `system.cfg`: the chassis CSVs live here.
pub const OBJECT_PATH: &str = "data/objects/";
/// `profilePath` from `system.cfg`: mech and vehicle profile FITs.
pub const PROFILE_PATH: &str = "data/missions/profiles/";

/// Reads `data/objects/<name>.csv` and parses the chassis. `name` is the appearance
/// or CSV name (for example `Werewolf`); the lookup lower-cases it.
pub fn load_chassis(vfs: &mut Vfs, name: &str) -> Result<Chassis> {
    let stem = name.to_ascii_lowercase();
    let path = format!("{OBJECT_PATH}{stem}.csv");
    let bytes = vfs
        .read(&path)?
        .ok_or_else(|| SimDataError::MissingFile { name: path.clone() })?;
    let csv = CsvFile::from_bytes(bytes)?;
    Chassis::from_csv(&stem, &csv)
}

/// Reads `data/objects/variants.csv`, the designer's chassis name list.
pub fn load_variant_list(vfs: &mut Vfs) -> Result<Vec<String>> {
    let path = format!("{OBJECT_PATH}variants.csv");
    let bytes = vfs
        .read(&path)?
        .ok_or_else(|| SimDataError::MissingFile { name: path.clone() })?;
    let csv = CsvFile::from_bytes(bytes)?;
    Ok(chassis::variant_list_names(&csv))
}

/// Reads `data/missions/profiles/<name>.fit` and parses it as a mech profile.
/// `name` is the mission `ObjectProfile` value, for example `PM207300`.
pub fn load_mech_profile(vfs: &mut Vfs, name: &str) -> Result<MechProfile> {
    let (file, mut fit) = open_profile(vfs, name)?;
    MechProfile::from_fit(&file, &mut fit)
}

/// Reads `data/missions/profiles/<name>.fit` and parses it as a ground vehicle profile.
pub fn load_vehicle_profile(vfs: &mut Vfs, name: &str) -> Result<VehicleProfile> {
    let (file, mut fit) = open_profile(vfs, name)?;
    VehicleProfile::from_fit(&file, &mut fit)
}

fn open_profile(vfs: &mut Vfs, name: &str) -> Result<(String, FitIni)> {
    let stem = name.to_ascii_lowercase();
    let path = format!("{PROFILE_PATH}{stem}.fit");
    let fit = vfs
        .read_fit(&path)?
        .ok_or_else(|| SimDataError::MissingFile { name: path.clone() })?;
    Ok((path, fit))
}
