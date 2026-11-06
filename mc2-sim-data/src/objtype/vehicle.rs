// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GroundVehicleType` class fields from `[General]` and `[InternalStructure]`
//! (gvehicl.cpp:292-395).
//!
//! `dynamics.init(&vehicleFile)` (gvehicl.cpp:390) reads the `[VehicleDynamics]` block;
//! it is typed through [`crate::mech::vehicle::VehicleDynamics`].

use mc2_formats::ini::FitIni;

use crate::error::Result;
use crate::mech::vehicle::VehicleDynamics;

use super::mech::MOVETYPE_GROUND;

/// Name used in error messages for the packet this type is read from.
const PACKET_FILE: &str = "object2.pak";

/// Body locations in `[InternalStructure]` order (gvehicl.cpp:300-306).
pub const GROUND_VEHICLE_LOCATIONS: [&str; 5] = ["Front", "Left", "Right", "Rear", "Turret"];

/// Class-level fields of a ground vehicle type.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundVehicleType {
    /// `MoveType` (gvehicl.cpp:313). Default [`MOVETYPE_GROUND`].
    pub move_type: i32,
    /// `LOSFactor` (gvehicl.cpp:317). Default 1.0.
    pub los_factor: f32,
    /// `Chassis` (gvehicl.cpp:321). Required, `Fatal` when missing.
    pub chassis: u8,
    /// `RefitPoints` (gvehicl.cpp:328). Default 0.
    pub refit_points: i32,
    /// `ResourcePoints` (gvehicl.cpp:332). Default 0.
    pub resource_points: i32,
    /// `RecoverPoints` (gvehicl.cpp:336). Default 0.
    pub recover_points: i32,
    /// `MineSweeper` (gvehicl.cpp:340). Default false.
    pub mine_sweeper: bool,
    /// `MineLayer` (gvehicl.cpp:344). Default 0.
    pub mine_layer: i32,
    /// `HoverCraft` (gvehicl.cpp:348). Default false.
    pub hover_craft: bool,
    /// `AeroSpaceSpotter` (gvehicl.cpp:352). Default false.
    pub aerospace_spotter: bool,
    /// `IsSensorContact` (gvehicl.cpp:356). Default true.
    pub is_sensor_contact: bool,
    /// `ExplosionRadius` (gvehicl.cpp:362). Default 0.0.
    pub explosion_radius: f32,
    /// `ExplosionDamage` (gvehicl.cpp:366). Default 0.0.
    pub explosion_damage: f32,
    /// `PathLocks` (gvehicl.cpp:370). Default true.
    pub path_locks: bool,
    /// Max internal structure per location, in [`GROUND_VEHICLE_LOCATIONS`] order
    /// (gvehicl.cpp:376-386). All required.
    pub max_internal_structure: [u8; 5],
    /// `[VehicleDynamics]` (`MoverDynamics::init`, mover.cpp:1631-1667): turret and
    /// vehicle yaw limits, pivot rate and speed. Required, as the C++ asserts on each key.
    pub dynamics: VehicleDynamics,
}

impl GroundVehicleType {
    /// Reads `[General]` and `[InternalStructure]`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("General")?;
        let move_type = fit.read_i32("MoveType").unwrap_or(MOVETYPE_GROUND);
        let los_factor = fit.read_f32("LOSFactor").unwrap_or(1.0);
        let chassis = fit.read_u8("Chassis")?;
        let refit_points = fit.read_i32("RefitPoints").unwrap_or(0);
        let resource_points = fit.read_i32("ResourcePoints").unwrap_or(0);
        let recover_points = fit.read_i32("RecoverPoints").unwrap_or(0);
        let mine_sweeper = fit.read_bool("MineSweeper").unwrap_or(false);
        let mine_layer = fit.read_i32("MineLayer").unwrap_or(0);
        let hover_craft = fit.read_bool("HoverCraft").unwrap_or(false);
        let aerospace_spotter = fit.read_bool("AeroSpaceSpotter").unwrap_or(false);
        let is_sensor_contact = fit.read_bool("IsSensorContact").unwrap_or(true);
        let explosion_radius = fit.read_f32("ExplosionRadius").unwrap_or(0.0);
        let explosion_damage = fit.read_f32("ExplosionDamage").unwrap_or(0.0);
        let path_locks = fit.read_bool("PathLocks").unwrap_or(true);

        fit.seek_block("InternalStructure")?;
        let mut max_internal_structure = [0u8; 5];
        for (slot, name) in max_internal_structure
            .iter_mut()
            .zip(GROUND_VEHICLE_LOCATIONS)
        {
            *slot = fit.read_u8(name)?;
        }

        let dynamics = VehicleDynamics::from_fit(PACKET_FILE, fit)?;

        Ok(Self {
            dynamics,
            move_type,
            los_factor,
            chassis,
            refit_points,
            resource_points,
            recover_points,
            mine_sweeper,
            mine_layer,
            hover_craft,
            aerospace_spotter,
            is_sensor_contact,
            explosion_radius,
            explosion_damage,
            path_locks,
            max_internal_structure,
        })
    }
}
