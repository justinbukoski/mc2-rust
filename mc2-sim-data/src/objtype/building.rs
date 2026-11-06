// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `BuildingType` (bldng.cpp:166-322). Handles both the BUILDING class (block
//! `[BuildingData]`) and TREEBUILDING (block `[TreeData]`, animated buildings).

use mc2_formats::ini::FitIni;

use crate::error::Result;

use super::base::ObjectTypeBase;

/// Building packet indices that are walls (bldng.cpp:115-120, `WallType[18]`).
pub const WALL_TYPE_INDICES: [u32; 18] = [
    26, 125, 152, 154, 156, 158, 160, 161, 162, 202, 204, 206, 231, 232, 233, 286, 287, 288,
];

/// The land-bridge building index (`BRIDGE_TYPE`, bldng.cpp:120).
pub const LAND_BRIDGE_TYPE_INDEX: u32 = 607;

/// `BUILDING_SUBTYPE_*` (bldng.h:43-47).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BuildingSubType {
    /// `BUILDING_SUBTYPE_NONE`.
    None = 0,
    /// `BUILDING_SUBTYPE_WALL`. Set by packet index (bldng.cpp:305-307).
    Wall = 1,
    /// `BUILDING_SUBTYPE_LANDBRIDGE`. Set by packet index 607 (bldng.cpp:312-313).
    LandBridge = 2,
}

/// Class-level fields of a building type.
#[derive(Debug, Clone, PartialEq)]
pub struct BuildingType {
    /// True when the packet has `[TreeData]` (TREEBUILDING packets). False for
    /// `[BuildingData]` (bldng.cpp:180-186).
    pub from_tree_data: bool,
    /// Packet-index subtype (bldng.cpp:305-313).
    pub sub_type: BuildingSubType,
    /// `DmgLevel` (bldng.cpp:189). Required. Stored as float (bldng.cpp:191).
    pub damage_level: f32,
    /// `CanRefit` (bldng.cpp:194). Default false.
    pub can_refit: bool,
    /// `MechBay` (bldng.cpp:200). Read only when `can_refit` is true. Otherwise false
    /// here. `init(void)` sets it false (bldng.cpp:140).
    pub mech_bay: bool,
    /// `ExplosionRadius` (bldng.cpp:205). Default 0.0.
    pub explosion_radius: f32,
    /// `ExplosionDamage` (bldng.cpp:209). Default 0.0.
    pub explosion_damage: f32,
    /// `Tonnage` (bldng.cpp:213). Default 20.0.
    pub base_tonnage: f32,
    /// `BattleRating` (bldng.cpp:217). Default 20.
    pub start_battle_rating: i32,
    /// `NumMarines` (bldng.cpp:221). Default 0.
    pub num_marines: i32,
    /// `ActivityEffectID` (bldng.cpp:230). Default `0xFFFFFFFF`.
    pub activity_effect_id: u32,
    /// `TeamID` as written (bldng.cpp:236), stored in `BuildingType::teamId` (bldng.h:58). That member shadows `ObjectType::teamId`,
    /// which `ObjectType::init` (objtype.cpp:168) forces to -1; `Building::init` reads the shadowing member (bldng.cpp:1257-1267),
    /// so this declared team is the one a new building starts on (and registers its sensor with).
    pub declared_team_id: i32,
    /// `SensorRange` (bldng.cpp:240). Default -1.0.
    pub sensor_range: f32,
    /// `BuildingName` (bldng.cpp:244). `None` when missing, meaning
    /// `IDS_BLDOBJ_NAME`.
    pub building_name_id: Option<i32>,
    /// `BuildingDescription` (bldng.cpp:248, read again at 253). Default -1.
    pub building_description_id: i32,
    /// `ResourcePoints` (bldng.cpp:257). Default 0.
    pub resource_points: i32,
    /// `ImpassableWhenDestroyed` (bldng.cpp:261). Default true.
    pub marks_impassable_when_destroyed: bool,
    /// `Capturable` (bldng.cpp:265). Default false.
    pub capturable: bool,
    /// `IsPowerSource` (bldng.cpp:269). Default false.
    pub power_source: bool,
    /// `LookoutTowerRange` (bldng.cpp:273). Default 0.0.
    pub lookout_tower_range: f32,
    /// `PerimeterAlarmRange` (bldng.cpp:277). Default 0.0. When above zero it replaces
    /// the extent radius (bldng.cpp:287-288).
    pub perimeter_alarm_range: f32,
    /// `PerimeterAlarmTimer` (bldng.cpp:281). Default 0.0.
    pub perimeter_alarm_timer: f32,
}

impl BuildingType {
    /// Reads the building block and `[ObjectType]`. `index` is the packet index,
    /// used for the subtype (bldng.cpp:305-313). Returns the base with the building's
    /// extent override applied (bldng.cpp:286-288).
    pub(crate) fn read(index: u32, fit: &mut FitIni) -> Result<(Self, ObjectTypeBase)> {
        let from_tree_data = fit.seek_block("TreeData").is_ok();
        if !from_tree_data {
            fit.seek_block("BuildingData")?;
        }

        let damage_level = fit.read_u32("DmgLevel")? as f32;
        let can_refit = fit.read_bool("CanRefit").unwrap_or(false);
        let mech_bay = if can_refit {
            fit.read_bool("MechBay").unwrap_or(false)
        } else {
            false
        };
        let explosion_radius = fit.read_f32("ExplosionRadius").unwrap_or(0.0);
        let explosion_damage = fit.read_f32("ExplosionDamage").unwrap_or(0.0);
        let base_tonnage = fit.read_f32("Tonnage").unwrap_or(20.0);
        let start_battle_rating = fit.read_i32("BattleRating").unwrap_or(20);
        let num_marines = fit.read_i32("NumMarines").unwrap_or(0);
        let real_extent = fit.read_f32("ExtentRadius").unwrap_or(-1.0);
        let activity_effect_id = fit.read_u32("ActivityEffectID").unwrap_or(0xFFFF_FFFF);
        let declared_team_id = fit.read_i32("TeamID").unwrap_or(-1);
        let sensor_range = fit.read_f32("SensorRange").unwrap_or(-1.0);
        let building_name_id = fit.read_i32("BuildingName").ok();
        let building_description_id = fit.read_i32("BuildingDescription").unwrap_or(-1);
        let resource_points = fit.read_i32("ResourcePoints").unwrap_or(0);
        let marks_impassable_when_destroyed =
            fit.read_bool("ImpassableWhenDestroyed").unwrap_or(true);
        let capturable = fit.read_bool("Capturable").unwrap_or(false);
        let power_source = fit.read_bool("IsPowerSource").unwrap_or(false);
        let lookout_tower_range = fit.read_f32("LookoutTowerRange").unwrap_or(0.0);
        let perimeter_alarm_range = fit.read_f32("PerimeterAlarmRange").unwrap_or(0.0);
        let perimeter_alarm_timer = fit.read_f32("PerimeterAlarmTimer").unwrap_or(0.0);

        let mut base = ObjectTypeBase::read(fit)?;
        base.extent_radius = real_extent;
        if perimeter_alarm_range > 0.0 {
            base.extent_radius = perimeter_alarm_range;
        }

        let sub_type = if WALL_TYPE_INDICES.contains(&index) {
            BuildingSubType::Wall
        } else if index == LAND_BRIDGE_TYPE_INDEX {
            BuildingSubType::LandBridge
        } else {
            BuildingSubType::None
        };

        Ok((
            Self {
                from_tree_data,
                sub_type,
                damage_level,
                can_refit,
                mech_bay,
                explosion_radius,
                explosion_damage,
                base_tonnage,
                start_battle_rating,
                num_marines,
                activity_effect_id,
                declared_team_id,
                sensor_range,
                building_name_id,
                building_description_id,
                resource_points,
                marks_impassable_when_destroyed,
                capturable,
                power_source,
                lookout_tower_range,
                perimeter_alarm_range,
                perimeter_alarm_timer,
            },
            base,
        ))
    }
}
