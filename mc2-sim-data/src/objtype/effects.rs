// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Effect types: `ExplosionType` (carnage.cpp:255-340), `FireType`
//! (carnage.cpp:128-190) and `LightType` (light.cpp:51-76).

use mc2_formats::ini::FitIni;

use crate::error::Result;

/// `EXPLOSION_TIME`, the default `DelayUntilCollidable` (carnage.cpp:56).
pub const EXPLOSION_TIME: f32 = 0.5;

/// `[LightData]` of an explosion (carnage.cpp:296-331). All eight keys are required.
/// The C++ calls `STOP` on a missing key and then keeps going. This crate returns an
/// error instead.
#[derive(Debug, Clone, PartialEq)]
pub struct ExplosionLight {
    /// `LightMinMaxRadius` (carnage.cpp:299).
    pub min_max_radius: f32,
    /// `LightMaxMaxRadius` (carnage.cpp:303).
    pub max_max_radius: f32,
    /// `LightOutMinRadius` (carnage.cpp:307).
    pub out_min_radius: f32,
    /// `LightOutMaxRadius` (carnage.cpp:311).
    pub out_max_radius: f32,
    /// `LightRGB` (carnage.cpp:315). Not swapped from BGR, unlike weapon bolts.
    pub light_rgb: u32,
    /// `MaxIntensity` (carnage.cpp:319).
    pub max_intensity: f32,
    /// `MinIntensity` (carnage.cpp:323).
    pub min_intensity: f32,
    /// `Duration` (carnage.cpp:327).
    pub duration: f32,
}

/// Class-level fields of an explosion type (`[ExplosionData]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExplosionType {
    /// `DmgLevel` (carnage.cpp:271). Required, stored as float (carnage.cpp:273).
    pub damage_level: f32,
    /// `SoundEffectId` (carnage.cpp:276). Required.
    pub sound_effect_id: u32,
    /// `ExplosionRadius` (carnage.cpp:280). Default 0.
    pub explosion_radius: i32,
    /// `LightObjectId` (carnage.cpp:284). Default `0xFFFFFFFF`.
    pub light_object_id: u32,
    /// `DamageChunkSize` (carnage.cpp:288). Default 5.0.
    pub chunk_size: f32,
    /// `DelayUntilCollidable` (carnage.cpp:292). Default [`EXPLOSION_TIME`].
    pub delay_until_collidable: f32,
    /// `[LightData]`, when the block exists (carnage.cpp:296).
    pub light: Option<ExplosionLight>,
}

impl ExplosionType {
    /// Reads `[ExplosionData]` and the optional `[LightData]`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("ExplosionData")?;
        let damage_level = fit.read_u32("DmgLevel")? as f32;
        let sound_effect_id = fit.read_u32("SoundEffectId")?;
        let explosion_radius = fit.read_i32("ExplosionRadius").unwrap_or(0);
        let light_object_id = fit.read_u32("LightObjectId").unwrap_or(0xFFFF_FFFF);
        let chunk_size = fit.read_f32("DamageChunkSize").unwrap_or(5.0);
        let delay_until_collidable = fit
            .read_f32("DelayUntilCollidable")
            .unwrap_or(EXPLOSION_TIME);

        let light = if fit.seek_block("LightData").is_ok() {
            Some(ExplosionLight {
                min_max_radius: fit.read_f32("LightMinMaxRadius")?,
                max_max_radius: fit.read_f32("LightMaxMaxRadius")?,
                out_min_radius: fit.read_f32("LightOutMinRadius")?,
                out_max_radius: fit.read_f32("LightOutMaxRadius")?,
                light_rgb: fit.read_u32("LightRGB")?,
                max_intensity: fit.read_f32("MaxIntensity")?,
                min_intensity: fit.read_f32("MinIntensity")?,
                duration: fit.read_f32("Duration")?,
            })
        } else {
            None
        };

        Ok(Self {
            damage_level,
            sound_effect_id,
            explosion_radius,
            light_object_id,
            chunk_size,
            delay_until_collidable,
            light,
        })
    }
}

/// Class-level fields of a fire type (`[FireData]`).
#[derive(Debug, Clone, PartialEq)]
pub struct FireType {
    /// `DmgLevel` (carnage.cpp:142). Required, stored as float (carnage.cpp:144).
    pub damage_level: f32,
    /// `SoundEffectId` (carnage.cpp:147). Required.
    pub sound_effect_id: u32,
    /// `LightObjectId` (carnage.cpp:151). Default `0xFFFFFFFF`.
    pub light_object_id: u32,
    /// `startLoopFrame` (carnage.cpp:155). Required.
    pub start_loop_frame: u32,
    /// `numLoops` (carnage.cpp:159). Required.
    pub num_loops: u32,
    /// `endLoopFrame` (carnage.cpp:163). Required.
    pub end_loop_frame: u32,
    /// `maxExtentRadius` (carnage.cpp:167). Default 0.0.
    pub max_extent: f32,
    /// `TimeToMaxExtent` (carnage.cpp:171). Default 0.0.
    pub time_to_max: f32,
    /// `TotalFireShapes` (carnage.cpp:175). Default 1.
    pub total_fires: i32,
}

impl FireType {
    /// Reads `[FireData]`. The C++ sets the object class to FIRE after the base init
    /// (carnage.cpp:183).
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("FireData")?;
        let damage_level = fit.read_u32("DmgLevel")? as f32;
        let sound_effect_id = fit.read_u32("SoundEffectId")?;
        let light_object_id = fit.read_u32("LightObjectId").unwrap_or(0xFFFF_FFFF);
        let start_loop_frame = fit.read_u32("startLoopFrame")?;
        let num_loops = fit.read_u32("numLoops")?;
        let end_loop_frame = fit.read_u32("endLoopFrame")?;
        let max_extent = fit.read_f32("maxExtentRadius").unwrap_or(0.0);
        let time_to_max = fit.read_f32("TimeToMaxExtent").unwrap_or(0.0);
        let total_fires = fit.read_i32("TotalFireShapes").unwrap_or(1);
        Ok(Self {
            damage_level,
            sound_effect_id,
            light_object_id,
            start_loop_frame,
            num_loops,
            end_loop_frame,
            max_extent,
            time_to_max,
            total_fires,
        })
    }
}

/// Class-level fields of a light type (`[LightData]`, light.cpp:51-76).
#[derive(Debug, Clone, PartialEq)]
pub struct LightType {
    /// `OneShotFlag` (light.cpp:64). Required.
    pub one_shot_flag: bool,
    /// `AltitudeOffset` (light.cpp:68). Required.
    pub altitude_offset: f32,
}

impl LightType {
    /// Reads `[LightData]`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("LightData")?;
        let one_shot_flag = fit.read_bool("OneShotFlag")?;
        let altitude_offset = fit.read_f32("AltitudeOffset")?;
        Ok(Self {
            one_shot_flag,
            altitude_offset,
        })
    }
}
