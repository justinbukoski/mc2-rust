// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Turret, gate and artillery types: `TurretType` (turret.cpp:129-240), `GateType`
//! (gate.cpp:91-165), `ArtilleryType` (artlry.cpp:384-530).

use mc2_formats::ini::FitIni;

use crate::error::Result;

/// Value C++ `readIdULong` writes when the key is missing (`value = 0`, inifile.cpp:1236).
/// The turret and gate effect ids are read with no default assignment, so this is their effective default.
pub const NO_EFFECT_ID: u32 = 0;

/// Class-level fields of a turret type.
///
/// The C++ `punch` (turret.cpp:189-210) is the sum of each weapon's CV from the
/// master component list. The component lane computes it. It is not stored here.
#[derive(Debug, Clone, PartialEq)]
pub struct TurretType {
    /// `LOSFactor` (turret.cpp:144). Default 1.0.
    pub los_factor: f32,
    /// `DmgLevel` (turret.cpp:149). Required, stored as float.
    pub damage_level: f32,
    /// `BlownEffectId` (turret.cpp:154). Default 0 (inifile.cpp:1236). The C++ check at 155
    /// tests the stale `result`, so the default is 0 (see [`NO_EFFECT_ID`]).
    pub blown_effect_id: u32,
    /// `NormalEffectId` (turret.cpp:158). Same quirk, default 0.
    pub normal_effect_id: u32,
    /// `DamageEffectId` (turret.cpp:162). Same quirk, default 0.
    pub damage_effect_id: u32,
    /// `ExplosionRadius` (turret.cpp:166). Default 0.0.
    pub explosion_radius: f32,
    /// `ExplosionDamage` (turret.cpp:170). Default 0.0.
    pub explosion_damage: f32,
    /// `Tonnage` (turret.cpp:174). Default 20.0.
    pub base_tonnage: f32,
    /// `AttackRadius` (turret.cpp:178). Required, the engage radius.
    pub engage_radius: f32,
    /// `MaxTurretYawRate` (turret.cpp:182). Required.
    pub turret_yaw_rate: f32,
    /// `WeaponType`, `WeaponType1..3` (turret.cpp:186-208). `WeaponType` is required.
    /// The others default to -1 (no weapon).
    pub weapon_master_id: [i32; 4],
    /// `PilotSkill` (turret.cpp:214). Required.
    pub pilot_skill: i32,
    /// `LittleExtent` (turret.cpp:218). Default 20.0.
    pub little_extent: f32,
    /// `BuildingName` (turret.cpp:222). `None` when missing (`IDS_TRTOBJ_NAME`).
    pub turret_type_name_id: Option<i32>,
    /// `BuildingDescription` (turret.cpp:226). Default -1.
    pub building_description_id: i32,
}

impl TurretType {
    /// Reads `[TurretData]` and `[ObjectType]`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("TurretData")?;
        let los_factor = fit.read_f32("LOSFactor").unwrap_or(1.0);
        let damage_level = fit.read_u32("DmgLevel")? as f32;
        let blown_effect_id = fit.read_u32("BlownEffectId").unwrap_or(NO_EFFECT_ID);
        let normal_effect_id = fit.read_u32("NormalEffectId").unwrap_or(NO_EFFECT_ID);
        let damage_effect_id = fit.read_u32("DamageEffectId").unwrap_or(NO_EFFECT_ID);
        let explosion_radius = fit.read_f32("ExplosionRadius").unwrap_or(0.0);
        let explosion_damage = fit.read_f32("ExplosionDamage").unwrap_or(0.0);
        let base_tonnage = fit.read_f32("Tonnage").unwrap_or(20.0);
        let engage_radius = fit.read_f32("AttackRadius")?;
        let turret_yaw_rate = fit.read_f32("MaxTurretYawRate")?;
        let weapon0 = fit.read_i32("WeaponType")?;
        let weapon1 = fit.read_i32("WeaponType1").unwrap_or(-1);
        let weapon2 = fit.read_i32("WeaponType2").unwrap_or(-1);
        let weapon3 = fit.read_i32("WeaponType3").unwrap_or(-1);
        let pilot_skill = fit.read_i32("PilotSkill")?;
        let little_extent = fit.read_f32("LittleExtent").unwrap_or(20.0);
        let turret_type_name_id = fit.read_i32("BuildingName").ok();
        let building_description_id = fit.read_i32("BuildingDescription").unwrap_or(-1);

        Ok(Self {
            los_factor,
            damage_level,
            blown_effect_id,
            normal_effect_id,
            damage_effect_id,
            explosion_radius,
            explosion_damage,
            base_tonnage,
            engage_radius,
            turret_yaw_rate,
            weapon_master_id: [weapon0, weapon1, weapon2, weapon3],
            pilot_skill,
            little_extent,
            turret_type_name_id,
            building_description_id,
        })
    }
}

/// Class-level fields of a gate type.
#[derive(Debug, Clone, PartialEq)]
pub struct GateType {
    /// `DmgLevel` (gate.cpp:106). Required. The C++ class keeps it as an integer and
    /// `getDamageLevel` returns it (gate.h:41, 103).
    pub dmg_level: u32,
    /// `BlownEffectId` (gate.cpp:110). Default 0 (inifile.cpp:1236). Same stale-result quirk
    /// as turret.cpp:155.
    pub blown_effect_id: u32,
    /// `NormalEffectId` (gate.cpp:114). Same quirk.
    pub normal_effect_id: u32,
    /// `DamageEffectId` (gate.cpp:118). Same quirk.
    pub damage_effect_id: u32,
    /// `BasePixelOffsetX` (gate.cpp:122). Default 0.
    pub base_pixel_offset_x: i32,
    /// `BasePixelOffsetY` (gate.cpp:126). Default 0.
    pub base_pixel_offset_y: i32,
    /// `ExplosionRadius` (gate.cpp:130). Default 0.0.
    pub explosion_radius: f32,
    /// `ExplosionDamage` (gate.cpp:134). Default 0.0.
    pub explosion_damage: f32,
    /// `OpenRadius` (gate.cpp:138). Required.
    pub open_radius: f32,
    /// `LittleExtent` (gate.cpp:142). Default 20.0.
    pub little_extent: f32,
    /// `BuildingName` (gate.cpp:146). `None` when missing (`IDS_BLDOBJ_NAME`).
    pub gate_type_name_id: Option<i32>,
    /// `BuildingDescription` (gate.cpp:150). Default -1.
    pub building_description_id: i32,
    /// `BlocksLineOfFire` (gate.cpp:155). Default false.
    pub blocks_line_of_fire: bool,
}

impl GateType {
    /// Reads `[GateData]` and `[ObjectType]`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("GateData")?;
        let dmg_level = fit.read_u32("DmgLevel")?;
        let blown_effect_id = fit.read_u32("BlownEffectId").unwrap_or(NO_EFFECT_ID);
        let normal_effect_id = fit.read_u32("NormalEffectId").unwrap_or(NO_EFFECT_ID);
        let damage_effect_id = fit.read_u32("DamageEffectId").unwrap_or(NO_EFFECT_ID);
        let base_pixel_offset_x = fit.read_i32("BasePixelOffsetX").unwrap_or(0);
        let base_pixel_offset_y = fit.read_i32("BasePixelOffsetY").unwrap_or(0);
        let explosion_radius = fit.read_f32("ExplosionRadius").unwrap_or(0.0);
        let explosion_damage = fit.read_f32("ExplosionDamage").unwrap_or(0.0);
        let open_radius = fit.read_f32("OpenRadius")?;
        let little_extent = fit.read_f32("LittleExtent").unwrap_or(20.0);
        let gate_type_name_id = fit.read_i32("BuildingName").ok();
        let building_description_id = fit.read_i32("BuildingDescription").unwrap_or(-1);
        let blocks_line_of_fire = fit.read_bool("BlocksLineOfFire").unwrap_or(false);

        Ok(Self {
            dmg_level,
            blown_effect_id,
            normal_effect_id,
            damage_effect_id,
            base_pixel_offset_x,
            base_pixel_offset_y,
            explosion_radius,
            explosion_damage,
            open_radius,
            little_extent,
            gate_type_name_id,
            building_description_id,
            blocks_line_of_fire,
        })
    }
}

/// One explosion of an artillery strike (artlry.cpp:486-500).
#[derive(Debug, Clone, PartialEq)]
pub struct ArtilleryExplosion {
    /// `ExplosionDelay<i>`.
    pub delay: f32,
    /// `ExplosionOffsetX<i>`.
    pub offset_x: f32,
    /// `ExplosionOffsetY<i>`.
    pub offset_y: f32,
}

/// Explosion data of an artillery type. Read only when `NominalDamage` is non-zero
/// (artlry.cpp:468).
#[derive(Debug, Clone, PartialEq)]
pub struct ArtilleryExplosions {
    /// `NumExplosions` entries (artlry.cpp:470).
    pub explosions: Vec<ArtilleryExplosion>,
    /// `ExplosionsPerExplosion` (artlry.cpp:503).
    pub per_explosion: i32,
    /// `ExplosionRandomOffsetX` (artlry.cpp:507).
    pub random_offset_x: i32,
    /// `ExplosionRandomOffsetY` (artlry.cpp:511).
    pub random_offset_y: i32,
    /// `MinArtilleryHeadRange` (artlry.cpp:515). Default 5.
    pub min_artillery_head_range: i32,
}

/// Class-level fields of an artillery type.
#[derive(Debug, Clone, PartialEq)]
pub struct ArtilleryType {
    /// `ArtillerySpriteName` (artlry.cpp:400). Required. The C buffer is 80 bytes.
    pub sprite_name: String,
    /// `FrameCount` (artlry.cpp:404).
    pub frame_count: u32,
    /// `StartFrame` (artlry.cpp:408).
    pub start_frame: u32,
    /// `FrameRate` (artlry.cpp:412).
    pub frame_rate: f32,
    /// `NominalTimeToImpact` (artlry.cpp:416).
    pub nominal_time_to_impact: f32,
    /// `NominalTimeToLaunch` (artlry.cpp:420-422). Default `NominalTimeToImpact - 10`.
    pub nominal_time_to_launch: f32,
    /// `NominalDamage` (artlry.cpp:424).
    pub nominal_damage: f32,
    /// `NominalMajorRange` (artlry.cpp:428).
    pub nominal_major_range: f32,
    /// `NominalMajorHits` (artlry.cpp:432).
    pub nominal_major_hits: f32,
    /// `NominalMinorRange` (artlry.cpp:436).
    pub nominal_minor_range: f32,
    /// `NominalMinorHits` (artlry.cpp:440).
    pub nominal_minor_hits: f32,
    /// `NominalSensorTime` (artlry.cpp:444).
    pub nominal_sensor_time: f32,
    /// `NominalSensorRange` (artlry.cpp:448).
    pub nominal_sensor_range: f32,
    /// `fontScale` (artlry.cpp:452).
    pub font_scale: f32,
    /// `fontXOffset` (artlry.cpp:456).
    pub font_x_offset: f32,
    /// `fontYOffset` (artlry.cpp:460).
    pub font_y_offset: f32,
    /// `fontColor` (artlry.cpp:464).
    pub font_color: u32,
    /// Explosion data. `None` when `NominalDamage` is zero (artlry.cpp:468, 519-522).
    pub explosions: Option<ArtilleryExplosions>,
}

impl ArtilleryType {
    /// Reads `[Artillery]` and `[ObjectType]`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("Artillery")?;
        let sprite_name = fit.read_string("ArtillerySpriteName", 80)?;
        let frame_count = fit.read_u32("FrameCount")?;
        let start_frame = fit.read_u32("StartFrame")?;
        let frame_rate = fit.read_f32("FrameRate")?;
        let nominal_time_to_impact = fit.read_f32("NominalTimeToImpact")?;
        let nominal_time_to_launch = fit
            .read_f32("NominalTimeToLaunch")
            .unwrap_or(nominal_time_to_impact - 10.0);
        let nominal_damage = fit.read_f32("NominalDamage")?;
        let nominal_major_range = fit.read_f32("NominalMajorRange")?;
        let nominal_major_hits = fit.read_f32("NominalMajorHits")?;
        let nominal_minor_range = fit.read_f32("NominalMinorRange")?;
        let nominal_minor_hits = fit.read_f32("NominalMinorHits")?;
        let nominal_sensor_time = fit.read_f32("NominalSensorTime")?;
        let nominal_sensor_range = fit.read_f32("NominalSensorRange")?;
        let font_scale = fit.read_f32("fontScale")?;
        let font_x_offset = fit.read_f32("fontXOffset")?;
        let font_y_offset = fit.read_f32("fontYOffset")?;
        let font_color = fit.read_u32("fontColor")?;

        let explosions = if nominal_damage != 0.0 {
            let count = fit.read_i32("NumExplosions")?;
            if count < 0 {
                return Err(crate::error::SimDataError::out_of_range(
                    "object2.pak",
                    format!("NumExplosions {count} is negative (artlry.cpp:474-476)"),
                ));
            }
            let mut explosions = Vec::with_capacity(count as usize);
            for i in 0..count {
                let delay = fit.read_f32(&format!("ExplosionDelay{i}"))?;
                let offset_x = fit.read_f32(&format!("ExplosionOffsetX{i}"))?;
                let offset_y = fit.read_f32(&format!("ExplosionOffsetY{i}"))?;
                explosions.push(ArtilleryExplosion {
                    delay,
                    offset_x,
                    offset_y,
                });
            }
            let per_explosion = fit.read_i32("ExplosionsPerExplosion")?;
            let random_offset_x = fit.read_i32("ExplosionRandomOffsetX")?;
            let random_offset_y = fit.read_i32("ExplosionRandomOffsetY")?;
            let min_artillery_head_range = fit.read_i32("MinArtilleryHeadRange").unwrap_or(5);
            Some(ArtilleryExplosions {
                explosions,
                per_explosion,
                random_offset_x,
                random_offset_y,
                min_artillery_head_range,
            })
        } else {
            None
        };

        Ok(Self {
            sprite_name,
            frame_count,
            start_frame,
            frame_rate,
            nominal_time_to_impact,
            nominal_time_to_launch,
            nominal_damage,
            nominal_major_range,
            nominal_major_hits,
            nominal_minor_range,
            nominal_minor_hits,
            nominal_sensor_time,
            nominal_sensor_range,
            font_scale,
            font_x_offset,
            font_y_offset,
            font_color,
            explosions,
        })
    }
}
