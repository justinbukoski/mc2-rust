// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Ground vehicle data: the object-type FIT (`GroundVehicleType`), the vehicle
//! dynamics block, and the ground vehicle profile FIT.
//!
//! Ports `GroundVehicleType::init(FilePtr, ...)` for the `General` and `InternalStructure`
//! blocks (`code/gvehicl.cpp:292-396`), `GroundVehicle::init(FitIniFile*)`
//! (`code/gvehicl.cpp:985-1230`), and `MoverDynamics::init(FitIniFilePtr)`
//! (`code/mover.cpp:1631-1670`). Those read an object packet from `object2.pak`,
//! so the input is the packet's FIT image.
//!
//! Ground vehicles have no chassis CSV. `GroundVehicle` never opens a `.csv`.
//!
//! [`VehicleProfile`] parses `data/missions/profiles/engine.fit` and the `pv*.fit`
//! files (`FileType = "GroundVehicleProfile"`). No C++ loader for that type exists in
//! this tree (`grep GroundVehicleProfile code/` is empty), so the schema comes from
//! the files and this module is the only reader.

use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::FormatError;

/// Number of ground vehicle body parts (`NUM_GROUNDVEHICLE_LOCATIONS`, `code/gvehicl.h:86`).
pub const NUM_VEHICLE_PARTS: usize = 5;
/// Part block names in order (`bodyLocationString`, `code/gvehicl.cpp:292`; `BodyLocationBlockString`, `code/gvehicl.cpp:985`).
pub const VEHICLE_PARTS: [&str; NUM_VEHICLE_PARTS] = ["Front", "Left", "Right", "Rear", "Turret"];
/// Minimum yaw rate, applied after reading (`code/mover.cpp:1653`).
pub const MIN_VEHICLE_YAW_RATE: i32 = 720;
/// `GroundVehiclePivotYawMultiplier` (`code/mover.cpp:239`), used for the pivot default.
pub const PIVOT_YAW_MULTIPLIER: f32 = 0.25;
/// Accel is set to speed times this factor (`Frank Hack`, `code/mover.cpp:1667`).
pub const ACCEL_PER_SPEED: f32 = 5.0;

/// `[VehicleDynamics]` after `MoverDynamics::init` (`code/mover.cpp:1631-1667`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VehicleDynamics {
    /// `maxTurretYawRate` (long).
    pub turret_yaw_rate: i32,
    /// `maxTurretYaw` (long).
    pub turret_yaw: i32,
    /// `maxVehicleYawRate` (long), raised to at least 720.
    pub yaw_rate: i32,
    /// `maxVehiclePivotRate` (long). Defaults to `(long)(yaw_rate * 0.25)` when absent.
    pub pivot_rate: i32,
    /// Accel, set to `speed * 5.0` (the `maxAccel` value is overwritten).
    pub accel: f32,
    /// `maxVelocity` (float).
    pub speed: f32,
}

impl VehicleDynamics {
    /// Reads `[VehicleDynamics]` from the current FIT image. Seeks the block itself.
    pub fn from_fit(file: &str, fit: &mut FitIni) -> Result<VehicleDynamics> {
        required(file, "VehicleDynamics", fit.seek_block("VehicleDynamics"))?;
        let turret_yaw_rate = required(file, "maxTurretYawRate", fit.read_i32("maxTurretYawRate"))?;
        let turret_yaw = required(file, "maxTurretYaw", fit.read_i32("maxTurretYaw"))?;
        let mut yaw_rate = required(file, "maxVehicleYawRate", fit.read_i32("maxVehicleYawRate"))?;
        if yaw_rate < MIN_VEHICLE_YAW_RATE {
            yaw_rate = MIN_VEHICLE_YAW_RATE;
        }
        let pivot_rate = match fit.read_i32("maxVehiclePivotRate") {
            Ok(v) => v,
            Err(FormatError::NotFound { .. }) => (yaw_rate as f32 * PIVOT_YAW_MULTIPLIER) as i32,
            Err(e) => return Err(schema(file, "maxVehiclePivotRate", e)),
        };
        // `maxAccel` is read and asserted (mover.cpp:1661), then overwritten by the
        // Frank hack below, so the value itself is not kept.
        required(file, "maxAccel", fit.read_f32("maxAccel"))?;
        let speed = required(file, "maxVelocity", fit.read_f32("maxVelocity"))?;
        Ok(VehicleDynamics {
            turret_yaw_rate,
            turret_yaw,
            yaw_rate,
            pivot_rate,
            accel: speed * ACCEL_PER_SPEED,
            speed,
        })
    }
}

/// Body part of a ground vehicle: internal structure and armor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VehiclePart {
    /// Maximum internal structure (`[InternalStructure]`, `GroundVehicleType::init`).
    pub max_internal: u8,
    /// Current internal structure (`CurInternalStructure` in the part block).
    pub cur_internal: u8,
    /// `MaxArmorPoints`.
    pub max_armor: u8,
    /// `CurArmorPoints`.
    pub cur_armor: u8,
}

/// A ground vehicle object type: the `GroundVehicleType` fields and the `GroundVehicle`
/// instance fields read from the same packet.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundVehicleObject {
    /// `[General] MoveType` (long). Absent means the `MOVETYPE_GROUND` default in C++.
    pub move_type: Option<i64>,
    /// `LOSFactor` (float). Absent means 1.0.
    pub los_factor: Option<f32>,
    /// `Chassis` (uc), required by `GroundVehicleType::init`.
    pub chassis: u8,
    /// `RefitPoints` (long), default 0.
    pub refit_points: i64,
    /// `ResourcePoints` (long), default 0.
    pub resource_points: i64,
    /// `RecoverPoints` (long), default 0.
    pub recover_points: i64,
    /// `MineSweeper` (bool), default false.
    pub mine_sweeper: bool,
    /// `MineLayer` (long), default 0.
    pub mine_layer: i64,
    /// `HoverCraft` (bool), default false.
    pub hover_craft: bool,
    /// `AeroSpaceSpotter` (bool), default false.
    pub aerospace_spotter: bool,
    /// `IsSensorContact` (bool), default true.
    pub is_sensor_contact: bool,
    /// `ExplosionRadius` (float), default 0.
    pub explosion_radius: f32,
    /// `ExplosionDamage` (float), default 0.
    pub explosion_damage: f32,
    /// `PathLocks` (bool), default true.
    pub path_locks: bool,
    /// `[General] CurTonnage` (float), required by `GroundVehicle::init`.
    pub tonnage: f32,
    /// `[General] IconIndex` (int), default 0.
    pub icon_index: i32,
    /// `[General] DescIndex` (int), default -1.
    pub desc_id: i32,
    /// `[General] Status` (char), required.
    pub status: i8,
    /// `[General] BattleRating` (int), default -1.
    pub battle_rating: i32,
    /// Dynamics from `[VehicleDynamics]`.
    pub dynamics: VehicleDynamics,
    /// Body parts in `VEHICLE_PARTS` order.
    pub parts: [VehiclePart; NUM_VEHICLE_PARTS],
    /// Inventory counts (`NumOther`, `NumWeapons`, `NumAmmo`).
    pub num_other: u8,
    /// Inventory counts: weapons.
    pub num_weapons: u8,
    /// Inventory counts: ammo.
    pub num_ammo: u8,
    /// `MasterID` of each inventory item, in the order other, weapons, ammo.
    pub items: Vec<u8>,
}

impl GroundVehicleObject {
    /// Parses a `GroundVehicleType` packet image. Errors match the C++ `Fatal`
    /// and `return` points (`code/gvehicl.cpp:292-396` and `:985-1230`).
    pub fn from_fit(file: &str, fit: &mut FitIni) -> Result<GroundVehicleObject> {
        required(file, "General", fit.seek_block("General"))?;
        let move_type = opt(file, "MoveType", fit.read_i64("MoveType"))?;
        let los_factor = opt(file, "LOSFactor", fit.read_f32("LOSFactor"))?;
        let chassis = required(file, "Chassis", fit.read_u8("Chassis"))?;
        let refit_points = opt(file, "RefitPoints", fit.read_i64("RefitPoints"))?.unwrap_or(0);
        let resource_points =
            opt(file, "ResourcePoints", fit.read_i64("ResourcePoints"))?.unwrap_or(0);
        let recover_points =
            opt(file, "RecoverPoints", fit.read_i64("RecoverPoints"))?.unwrap_or(0);
        let mine_sweeper = opt(file, "MineSweeper", fit.read_bool("MineSweeper"))?.unwrap_or(false);
        let mine_layer = opt(file, "MineLayer", fit.read_i64("MineLayer"))?.unwrap_or(0);
        let hover_craft = opt(file, "HoverCraft", fit.read_bool("HoverCraft"))?.unwrap_or(false);
        let aerospace_spotter =
            opt(file, "AeroSpaceSpotter", fit.read_bool("AeroSpaceSpotter"))?.unwrap_or(false);
        let is_sensor_contact =
            opt(file, "IsSensorContact", fit.read_bool("IsSensorContact"))?.unwrap_or(true);
        let explosion_radius =
            opt(file, "ExplosionRadius", fit.read_f32("ExplosionRadius"))?.unwrap_or(0.0);
        let explosion_damage =
            opt(file, "ExplosionDamage", fit.read_f32("ExplosionDamage"))?.unwrap_or(0.0);
        let path_locks = opt(file, "PathLocks", fit.read_bool("PathLocks"))?.unwrap_or(true);

        required(
            file,
            "InternalStructure",
            fit.seek_block("InternalStructure"),
        )?;
        let mut max_internal = [0u8; NUM_VEHICLE_PARTS];
        for (i, part) in VEHICLE_PARTS.iter().enumerate() {
            max_internal[i] = required(file, part, fit.read_u8(part))?;
        }

        let dynamics = VehicleDynamics::from_fit(file, fit)?;

        // `GroundVehicle::init` reads its own General block (`code/gvehicl.cpp:1003-1038`).
        required(file, "ObjectType", fit.seek_block("ObjectType"))?;
        required(file, "General", fit.seek_block("General"))?;
        let icon_index = opt(file, "IconIndex", fit.read_i32("IconIndex"))?.unwrap_or(0);
        let tonnage = required(file, "CurTonnage", fit.read_f32("CurTonnage"))?;
        let desc_id = opt(file, "DescIndex", fit.read_i32("DescIndex"))?.unwrap_or(-1);
        let status = required(file, "Status", fit.read_i8("Status"))?;
        let battle_rating = opt(file, "BattleRating", fit.read_i32("BattleRating"))?.unwrap_or(-1);

        required(file, "InventoryInfo", fit.seek_block("InventoryInfo"))?;
        let num_other = required(file, "NumOther", fit.read_u8("NumOther"))?;
        let num_weapons = required(file, "NumWeapons", fit.read_u8("NumWeapons"))?;
        let num_ammo = required(file, "NumAmmo", fit.read_u8("NumAmmo"))?;
        let total = num_other as usize + num_weapons as usize + num_ammo as usize;
        let mut items = Vec::with_capacity(total);
        for n in 0..total {
            let block = format!("Item:{n}");
            required(file, &block, fit.seek_block(&block))?;
            items.push(required(
                file,
                &format!("{block} MasterID"),
                fit.read_u8("MasterID"),
            )?);
        }

        let mut parts = [VehiclePart::default(); NUM_VEHICLE_PARTS];
        for (i, part) in VEHICLE_PARTS.iter().enumerate() {
            required(file, part, fit.seek_block(part))?;
            let cur_internal = required(
                file,
                "CurInternalStructure",
                fit.read_u8("CurInternalStructure"),
            )?;
            let max_armor = required(file, "MaxArmorPoints", fit.read_u8("MaxArmorPoints"))?;
            let cur_armor = required(file, "CurArmorPoints", fit.read_u8("CurArmorPoints"))?;
            parts[i] = VehiclePart {
                max_internal: max_internal[i],
                cur_internal,
                max_armor,
                cur_armor,
            };
        }

        Ok(GroundVehicleObject {
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
            tonnage,
            icon_index,
            desc_id,
            status,
            battle_rating,
            dynamics,
            parts,
            num_other,
            num_weapons,
            num_ammo,
            items,
        })
    }
}

/// A ground vehicle profile (`FileType = "GroundVehicleProfile"`), as in `engine.fit`.
///
/// Sections: `[General]`, `[Engine]`, `[Armor]`, then the five part blocks and
/// `[InventoryInfo]`. Counts are kept as written.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleProfile {
    /// `[General] ID` (ul).
    pub id: u64,
    /// `[General] Crew` (st).
    pub crew: Option<String>,
    /// `[General] Chassis` (ul), the chassis type id.
    pub chassis: u64,
    /// `[General] Name` (st).
    pub name: String,
    /// `[General] VehicleTypeID` (ul).
    pub vehicle_type_id: u64,
    /// `[General] Variation` (uc). Absent in some files (for example `pv20000.fit`).
    pub variation: Option<u8>,
    /// `[General] NameIndex` (l), when written.
    pub name_index: Option<i64>,
    /// `[General] BattleRating` (l), when written.
    pub battle_rating: Option<i64>,
    /// `[General] DescIndex` (l).
    pub desc_index: Option<i64>,
    /// `[General] CurTonnage` (f).
    pub cur_tonnage: f32,
    /// `[General] icon` (st).
    pub icon: Option<String>,
    /// `[General] Status` (c).
    pub status: i8,
    /// `[General] ResourcePoints` (l).
    pub resource_points: Option<i64>,
    /// `[General] Morale` (l).
    pub morale: Option<i64>,
    /// `[General] MoraleModifier` (l).
    pub morale_modifier: Option<i64>,
    /// `[Engine] Tonnage` (f).
    pub engine_tonnage: f32,
    /// `[Engine] Rating` (ul).
    pub engine_rating: u64,
    /// `[Engine] MaxMoveSpeed` (uc).
    pub max_move_speed: u8,
    /// `[Armor] Type` (uc).
    pub armor_type: u8,
    /// `[Armor] Tonnage` (f).
    pub armor_tonnage: f32,
    /// Parts in `VEHICLE_PARTS` order (`CurInternalStructure`, `MaxArmorPoints`, `CurArmorPoints`).
    pub parts: [VehiclePart; NUM_VEHICLE_PARTS],
    /// `[InventoryInfo]` counts.
    pub num_other: u8,
    /// `[InventoryInfo]` counts.
    pub num_weapons: u8,
    /// `[InventoryInfo]` counts.
    pub num_ammo: u8,
    /// `MasterID` of each `[Item:n]` block, for the counted entries, in order other, weapons, ammo.
    pub items: Vec<u8>,
}

impl VehicleProfile {
    /// Parses a ground vehicle profile from its FIT image.
    pub fn from_fit(file: &str, fit: &mut FitIni) -> Result<VehicleProfile> {
        required(file, "Header", fit.seek_block("Header"))?;
        let file_type = required(file, "FileType", fit.read_string("FileType", 127))?;
        if file_type != "GroundVehicleProfile" {
            return Err(SimDataError::schema(
                file,
                format!("FileType is '{file_type}', expected 'GroundVehicleProfile'"),
            ));
        }
        required(file, "General", fit.seek_block("General"))?;
        let id = required(file, "ID", fit.read_u64("ID"))?;
        let crew = opt(file, "Crew", fit.read_string("Crew", 127))?;
        let chassis = required(file, "Chassis", fit.read_u64("Chassis"))?;
        let name = required(file, "Name", fit.read_string("Name", 127))?;
        let vehicle_type_id = required(file, "VehicleTypeID", fit.read_u64("VehicleTypeID"))?;
        let variation = opt(file, "Variation", fit.read_u8("Variation"))?;
        let name_index = opt(file, "NameIndex", fit.read_i64("NameIndex"))?;
        let battle_rating = opt(file, "BattleRating", fit.read_i64("BattleRating"))?;
        let desc_index = opt(file, "DescIndex", fit.read_i64("DescIndex"))?;
        let cur_tonnage = required(file, "CurTonnage", fit.read_f32("CurTonnage"))?;
        let icon = opt(file, "icon", fit.read_string("icon", 127))?;
        let status = required(file, "Status", fit.read_i8("Status"))?;
        let resource_points = opt(file, "ResourcePoints", fit.read_i64("ResourcePoints"))?;
        let morale = opt(file, "Morale", fit.read_i64("Morale"))?;
        let morale_modifier = opt(file, "MoraleModifier", fit.read_i64("MoraleModifier"))?;

        required(file, "Engine", fit.seek_block("Engine"))?;
        let engine_tonnage = required(file, "Tonnage", fit.read_f32("Tonnage"))?;
        let engine_rating = required(file, "Rating", fit.read_u64("Rating"))?;
        let max_move_speed = required(file, "MaxMoveSpeed", fit.read_u8("MaxMoveSpeed"))?;

        required(file, "Armor", fit.seek_block("Armor"))?;
        let armor_type = required(file, "Type", fit.read_u8("Type"))?;
        let armor_tonnage = required(file, "Tonnage", fit.read_f32("Tonnage"))?;

        let mut parts = [VehiclePart::default(); NUM_VEHICLE_PARTS];
        for (i, part) in VEHICLE_PARTS.iter().enumerate() {
            required(file, part, fit.seek_block(part))?;
            parts[i] = VehiclePart {
                max_internal: 0,
                cur_internal: required(
                    file,
                    "CurInternalStructure",
                    fit.read_u8("CurInternalStructure"),
                )?,
                max_armor: required(file, "MaxArmorPoints", fit.read_u8("MaxArmorPoints"))?,
                cur_armor: required(file, "CurArmorPoints", fit.read_u8("CurArmorPoints"))?,
            };
        }

        required(file, "InventoryInfo", fit.seek_block("InventoryInfo"))?;
        let num_other = required(file, "NumOther", fit.read_u8("NumOther"))?;
        let num_weapons = required(file, "NumWeapons", fit.read_u8("NumWeapons"))?;
        let num_ammo = required(file, "NumAmmo", fit.read_u8("NumAmmo"))?;
        let total = num_other as usize + num_weapons as usize + num_ammo as usize;
        let mut items = Vec::with_capacity(total);
        for n in 0..total {
            let block = format!("Item:{n}");
            required(file, &block, fit.seek_block(&block))?;
            items.push(required(
                file,
                &format!("{block} MasterID"),
                fit.read_u8("MasterID"),
            )?);
        }

        Ok(VehicleProfile {
            id,
            crew,
            chassis,
            name,
            vehicle_type_id,
            variation,
            name_index,
            battle_rating,
            desc_index,
            cur_tonnage,
            icon,
            status,
            resource_points,
            morale,
            morale_modifier,
            engine_tonnage,
            engine_rating,
            max_move_speed,
            armor_type,
            armor_tonnage,
            parts,
            num_other,
            num_weapons,
            num_ammo,
            items,
        })
    }
}

fn schema(file: &str, key: &str, e: FormatError) -> SimDataError {
    SimDataError::schema(file, format!("{key}: {e}"))
}

fn required<T>(file: &str, key: &str, r: mc2_formats::Result<T>) -> Result<T> {
    r.map_err(|e| schema(file, key, e))
}

fn opt<T>(file: &str, key: &str, r: mc2_formats::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(FormatError::NotFound { .. }) => Ok(None),
        Err(e) => Err(schema(file, key, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("FITini\n{text}\nFITEnd\n").into_bytes()).unwrap()
    }

    #[test]
    fn dynamics_raise_yaw_and_default_pivot() {
        let mut f = fit("[VehicleDynamics]\nl maxTurretYawRate = 120\nl maxTurretYaw = 360\nl maxVehicleYawRate = 360\nf maxAccel = 15.0\nf maxVelocity = 15.0\n");
        let d = VehicleDynamics::from_fit("vehicle.fit", &mut f).unwrap();
        assert_eq!(d.yaw_rate, MIN_VEHICLE_YAW_RATE, "360 is raised to 720");
        assert_eq!(
            d.pivot_rate,
            (720.0f32 * 0.25) as i32,
            "pivot default uses the raised yaw"
        );
        assert_eq!(d.accel, 15.0 * ACCEL_PER_SPEED, "accel is speed times 5");
        assert_eq!(d.speed, 15.0);
        assert_eq!(d.turret_yaw_rate, 120);
        assert_eq!(d.turret_yaw, 360);
    }

    #[test]
    fn dynamics_keep_a_written_pivot_rate() {
        let mut f = fit("[VehicleDynamics]\nl maxTurretYawRate = 1\nl maxTurretYaw = 2\nl maxVehicleYawRate = 1000\nl maxVehiclePivotRate = 333\nf maxAccel = 1.0\nf maxVelocity = 2.0\n");
        let d = VehicleDynamics::from_fit("vehicle.fit", &mut f).unwrap();
        assert_eq!(d.yaw_rate, 1000);
        assert_eq!(d.pivot_rate, 333);
    }

    /// Minimal ground vehicle packet: every block the loader reads.
    fn vehicle_packet() -> String {
        let mut s = String::from("[ObjectType]\nst AppearanceName = \"tank\"\n\n[General]\nuc Chassis = 7\nf CurTonnage = 40.0\nl DescIndex = 1234\nc Status = 0\nl IconIndex = 3\nl BattleRating = 57\n\n");
        s.push_str("[InternalStructure]\nuc Front = 1\nuc Left = 2\nuc Right = 3\nuc Rear = 4\nuc Turret = 5\n\n");
        s.push_str("[VehicleDynamics]\nl maxTurretYawRate = 120\nl maxTurretYaw = 360\nl maxVehicleYawRate = 360\nf maxAccel = 15.0\nf maxVelocity = 15.0\n\n");
        s.push_str("[InventoryInfo]\nuc NumOther = 1\nuc NumWeapons = 1\nuc NumAmmo = 0\n\n");
        s.push_str("[Item:0]\nuc MasterID = 9\n\n[Item:1]\nuc MasterID = 6\n\n");
        for (i, p) in VEHICLE_PARTS.iter().enumerate() {
            s.push_str(&format!("[{p}]\nuc CurInternalStructure = {}\nuc MaxArmorPoints = {}\nuc CurArmorPoints = {}\n\n", i + 1, 10 + i, 5 + i));
        }
        s
    }

    #[test]
    fn reads_a_ground_vehicle_object() {
        let mut f = fit(&vehicle_packet());
        let v = GroundVehicleObject::from_fit("tank", &mut f).unwrap();
        assert_eq!(v.chassis, 7);
        assert!(
            v.is_sensor_contact,
            "absent IsSensorContact defaults to true"
        );
        assert!(v.path_locks);
        assert_eq!(v.tonnage, 40.0);
        assert_eq!(v.desc_id, 1234);
        assert_eq!(v.battle_rating, 57);
        assert_eq!(v.dynamics.yaw_rate, 720);
        assert_eq!(v.parts[0].max_internal, 1);
        assert_eq!(v.parts[4].max_internal, 5);
        assert_eq!(v.parts[2].cur_internal, 3);
        assert_eq!(v.parts[3].max_armor, 13);
        assert_eq!(v.items, vec![9, 6]);
        assert_eq!((v.num_other, v.num_weapons, v.num_ammo), (1, 1, 0));
    }

    #[test]
    fn reads_a_ground_vehicle_profile() {
        let text = "[Header]\nst FileType = \"GroundVehicleProfile\"\n\n[General]\nul ID = 32500\nst Crew = \"PCREWB\"\nul Chassis = 433\nst Name = \"Train Engine\"\nul VehicleTypeID = 433\nuc Variation = 0\nl DescIndex = 12\nf CurTonnage = 0\nst icon = \"na.gif\"\nc Status = 0\nl ResourcePoints = 5\nl Morale = 0\nl MoraleModifier = 0\n\n[Engine]\nf Tonnage = 0\nul Rating = 0\nuc MaxMoveSpeed = 9\n\n[Armor]\nuc Type = 0\nf Tonnage = 0\n\n";
        let mut body = String::from(text);
        for p in VEHICLE_PARTS {
            body.push_str(&format!("[{p}]\nuc CurInternalStructure = 1\nuc MaxArmorPoints = 2\nuc CurArmorPoints = 3\n\n"));
        }
        body.push_str("[InventoryInfo]\nuc NumOther = 0\nuc NumWeapons = 0\nuc NumAmmo = 0\n");
        let mut f = fit(&body);
        let p = VehicleProfile::from_fit("engine.fit", &mut f).unwrap();
        assert_eq!(p.id, 32500);
        assert_eq!(p.name, "Train Engine");
        assert_eq!(p.crew.as_deref(), Some("PCREWB"));
        assert_eq!(p.max_move_speed, 9);
        assert_eq!(p.resource_points, Some(5));
        assert_eq!(p.variation, Some(0));
        assert_eq!(p.name_index, None);
        assert_eq!(p.parts[1].cur_armor, 3);
    }
}
