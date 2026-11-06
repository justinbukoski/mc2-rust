// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `WeaponBoltType` (weaponbolt.cpp:85-320). A bolt projectile (`[BoltProjectileData]`)
//! or a beam (`[BeamProjectileData]`, read when the bolt block is absent).

use mc2_formats::ini::FitIni;

use crate::error::{Result, SimDataError};

/// `Stuff::SMALL`, the `UnitLength` floor (weaponbolt.cpp:140-141). The value is
/// `Stuff::SMALL=1e-4f` in `mclib/stuff/style.cpp:21`.
pub const STUFF_SMALL: f32 = 1.0e-4;

/// Swaps the red and blue channels of a packed colour. The editor writes BGR and
/// the game converts it (weaponbolt.cpp:305-309, `bgrTorgb` at mclib/mech3d.cpp:1722).
pub fn bgr_to_rgb(c: u32) -> u32 {
    ((c & 0x00ff_0000) >> 16) | (c & 0x0000_ff00) | ((c & 0x0000_00ff) << 16)
}

/// `LightRGB`, `MaxRadius`, `OutRadius`, `Intensity` for a light-source bolt or beam
/// (weaponbolt.cpp:177-190 for beams, 286-299 for bolts). All four are required when
/// the projectile is a light source. The C++ `Fatal`s when one is missing.
#[derive(Debug, Clone, PartialEq)]
pub struct BoltLight {
    /// `LightRGB` (weaponbolt.cpp:177 / 286). Swapped from BGR on bolts (line 309).
    pub light_rgb: u32,
    /// `MaxRadius` (weaponbolt.cpp:181 / 290).
    pub max_radius: f32,
    /// `OutRadius` (weaponbolt.cpp:185 / 294).
    pub out_radius: f32,
    /// `Intensity` (weaponbolt.cpp:189 / 298).
    pub intensity: f32,
}

/// A `[BeamProjectileData]` projectile (weaponbolt.cpp:105-190).
#[derive(Debug, Clone, PartialEq)]
pub struct BeamProjectile {
    /// `IsBeam` (weaponbolt.cpp:105). Default false.
    pub is_beam: bool,
    /// `BeamDuration` (weaponbolt.cpp:109). Default 0.0.
    pub beam_duration: f32,
    /// `BeamWiggle` (weaponbolt.cpp:113). Default 0.0.
    pub beam_wiggle: f32,
    /// `PointLight` (weaponbolt.cpp:117). Default false.
    pub point_light: bool,
    /// `TextureName` (weaponbolt.cpp:122). Default `"NONE"`.
    pub texture_name: String,
    /// `UVAnimRate` (weaponbolt.cpp:129). Default 0.0.
    pub uv_anim_rate: f32,
    /// `UVRepeat` (weaponbolt.cpp:133). Default 1.0.
    pub uv_repeat: f32,
    /// `UnitLength` (weaponbolt.cpp:137). Default 1.0. A value below
    /// [`STUFF_SMALL`] is replaced by 1.0 (weaponbolt.cpp:141-142).
    pub unit_length: f32,
    /// `MipTexture` (weaponbolt.cpp:144). Default false.
    pub mip_texture: bool,
    /// `FiringSoundFX` (weaponbolt.cpp:148). Default 0.
    pub firing_sound_fx: i32,
    /// `BulgeWidth` (weaponbolt.cpp:152). Default 1.0.
    pub bulge_width: f32,
    /// `MidEdgeRGB` (weaponbolt.cpp:156). Default `0x00FFFFFF`. Not swapped.
    pub mid_edge_rgb: u32,
    /// `MiddleRGB` (weaponbolt.cpp:160). Default `0x00FFFFFF`. Not swapped.
    pub middle_rgb: u32,
    /// `EdgeAlpha` (weaponbolt.cpp:164). Default 0xFF.
    pub edge_alpha: u8,
    /// `BoltAlpha` (weaponbolt.cpp:168). Default 0xFF.
    pub bolt_alpha: u8,
    /// `projLength`, fixed at 1.0 for beams (weaponbolt.cpp:173).
    pub proj_length: f32,
    /// `velocity`, fixed at 1.0 for beams (weaponbolt.cpp:173).
    pub velocity: f32,
    /// Present when `PointLight` is true.
    pub light: Option<BoltLight>,
}

/// A `[BoltProjectileData]` projectile (weaponbolt.cpp:196-298). Colours are stored
/// in RGB order, after the BGR swap at weaponbolt.cpp:305-309.
#[derive(Debug, Clone, PartialEq)]
pub struct BoltProjectile {
    /// `ProjectileLength` (weaponbolt.cpp:196). Required.
    pub projectile_length: f32,
    /// `BulgeLength` (weaponbolt.cpp:200). Required.
    pub bulge_length: f32,
    /// `BulgeWidth` (weaponbolt.cpp:204). Required.
    pub bulge_width: f32,
    /// `Velocity` (weaponbolt.cpp:208). Required.
    pub velocity: f32,
    /// `HitEffect` (weaponbolt.cpp:212). Required. An object type number.
    pub hit_effect: i32,
    /// `MissEffect` (weaponbolt.cpp:216). Required. An object type number.
    pub miss_effect: i32,
    /// `FiringSoundFX` (weaponbolt.cpp:220). Required.
    pub firing_sound_fx: i32,
    /// `FrontRGB` (weaponbolt.cpp:224), in RGB order after the swap. Required.
    pub front_rgb: u32,
    /// `MidEdgeRGB` (weaponbolt.cpp:228), in RGB order after the swap. Required.
    pub mid_edge_rgb: u32,
    /// `MiddleRGB` (weaponbolt.cpp:232), in RGB order after the swap. Required.
    pub middle_rgb: u32,
    /// `BackRGB` (weaponbolt.cpp:236), in RGB order after the swap. Required.
    pub back_rgb: u32,
    /// `EdgeAlpha` (weaponbolt.cpp:240). Required.
    pub edge_alpha: u8,
    /// `BoltAlpha` (weaponbolt.cpp:244). Required.
    pub bolt_alpha: u8,
    /// `IsBeam` (weaponbolt.cpp:248). Default false.
    pub is_beam: bool,
    /// `BeamDuration` (weaponbolt.cpp:252). Default 0.0.
    pub beam_duration: f32,
    /// `BeamWiggle` (weaponbolt.cpp:256). Default 0.0.
    pub beam_wiggle: f32,
    /// `PointLight` (weaponbolt.cpp:260). Default false.
    pub point_light: bool,
    /// `ArcEffect` (weaponbolt.cpp:264). Default false.
    pub arc_effect: bool,
    /// `ArcHeight` (weaponbolt.cpp:268). Default -1.0.
    pub arc_height: f32,
    /// `AfterHitTime` (weaponbolt.cpp:272). Default 0.0.
    pub after_hit_time: f32,
    /// `AreaEffectDamage` (weaponbolt.cpp:276). Default 0.0.
    pub area_effect_damage: f32,
    /// `AreaEffectRadius` (weaponbolt.cpp:280). Default 0.0.
    pub area_effect_radius: f32,
    /// Present when `PointLight` is true. Its `LightRGB` is swapped too (line 309).
    pub light: Option<BoltLight>,
}

/// Either projectile form, chosen by which block the packet has.
#[derive(Debug, Clone, PartialEq)]
pub enum WeaponBoltType {
    /// `[BoltProjectileData]` present.
    Bolt(BoltProjectile),
    /// `[BoltProjectileData]` absent and `[BeamProjectileData]` present
    /// (weaponbolt.cpp:94-100).
    Beam(BeamProjectile),
}

fn read_light(fit: &FitIni) -> Result<BoltLight> {
    Ok(BoltLight {
        light_rgb: fit.read_u32("LightRGB")?,
        max_radius: fit.read_f32("MaxRadius")?,
        out_radius: fit.read_f32("OutRadius")?,
        intensity: fit.read_f32("Intensity")?,
    })
}

impl WeaponBoltType {
    /// Reads the projectile block. Both forms read `[ObjectType]` afterwards (the
    /// caller does that).
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        if fit.seek_block("BoltProjectileData").is_ok() {
            Self::read_bolt(fit).map(WeaponBoltType::Bolt)
        } else {
            fit.seek_block("BeamProjectileData").map_err(|_| {
                SimDataError::schema(
                    "object2.pak",
                    "weapon bolt has neither [BoltProjectileData] nor [BeamProjectileData]",
                )
            })?;
            Self::read_beam(fit).map(WeaponBoltType::Beam)
        }
    }

    fn read_beam(fit: &mut FitIni) -> Result<BeamProjectile> {
        let is_beam = fit.read_bool("IsBeam").unwrap_or(false);
        let beam_duration = fit.read_f32("BeamDuration").unwrap_or(0.0);
        let beam_wiggle = fit.read_f32("BeamWiggle").unwrap_or(0.0);
        let point_light = fit.read_bool("PointLight").unwrap_or(false);
        let texture_name = fit
            .read_string("TextureName", 1024)
            .unwrap_or_else(|_| "NONE".to_string());
        let uv_anim_rate = fit.read_f32("UVAnimRate").unwrap_or(0.0);
        let uv_repeat = fit.read_f32("UVRepeat").unwrap_or(1.0);
        let mut unit_length = fit.read_f32("UnitLength").unwrap_or(1.0);
        if unit_length < STUFF_SMALL {
            unit_length = 1.0;
        }
        let mip_texture = fit.read_bool("MipTexture").unwrap_or(false);
        let firing_sound_fx = fit.read_i32("FiringSoundFX").unwrap_or(0);
        let bulge_width = fit.read_f32("BulgeWidth").unwrap_or(1.0);
        let mid_edge_rgb = fit.read_u32("MidEdgeRGB").unwrap_or(0x00FF_FFFF);
        let middle_rgb = fit.read_u32("MiddleRGB").unwrap_or(0x00FF_FFFF);
        let edge_alpha = fit.read_u8("EdgeAlpha").unwrap_or(0xFF);
        let bolt_alpha = fit.read_u8("BoltAlpha").unwrap_or(0xFF);
        let light = if point_light {
            Some(read_light(fit)?)
        } else {
            None
        };
        Ok(BeamProjectile {
            is_beam,
            beam_duration,
            beam_wiggle,
            point_light,
            texture_name,
            uv_anim_rate,
            uv_repeat,
            unit_length,
            mip_texture,
            firing_sound_fx,
            bulge_width,
            mid_edge_rgb,
            middle_rgb,
            edge_alpha,
            bolt_alpha,
            proj_length: 1.0,
            velocity: 1.0,
            light,
        })
    }

    fn read_bolt(fit: &mut FitIni) -> Result<BoltProjectile> {
        let projectile_length = fit.read_f32("ProjectileLength")?;
        let bulge_length = fit.read_f32("BulgeLength")?;
        let bulge_width = fit.read_f32("BulgeWidth")?;
        let velocity = fit.read_f32("Velocity")?;
        let hit_effect = fit.read_i32("HitEffect")?;
        let miss_effect = fit.read_i32("MissEffect")?;
        let firing_sound_fx = fit.read_i32("FiringSoundFX")?;
        let front_rgb = fit.read_u32("FrontRGB")?;
        let mid_edge_rgb = fit.read_u32("MidEdgeRGB")?;
        let middle_rgb = fit.read_u32("MiddleRGB")?;
        let back_rgb = fit.read_u32("BackRGB")?;
        let edge_alpha = fit.read_u8("EdgeAlpha")?;
        let bolt_alpha = fit.read_u8("BoltAlpha")?;
        let is_beam = fit.read_bool("IsBeam").unwrap_or(false);
        let beam_duration = fit.read_f32("BeamDuration").unwrap_or(0.0);
        let beam_wiggle = fit.read_f32("BeamWiggle").unwrap_or(0.0);
        let point_light = fit.read_bool("PointLight").unwrap_or(false);
        let arc_effect = fit.read_bool("ArcEffect").unwrap_or(false);
        let arc_height = fit.read_f32("ArcHeight").unwrap_or(-1.0);
        let after_hit_time = fit.read_f32("AfterHitTime").unwrap_or(0.0);
        let area_effect_damage = fit.read_f32("AreaEffectDamage").unwrap_or(0.0);
        let area_effect_radius = fit.read_f32("AreaEffectRadius").unwrap_or(0.0);
        let light = if point_light {
            Some(read_light(fit)?)
        } else {
            None
        };
        // weaponbolt.cpp:305-309. The swap runs even without a light source.
        let light = light.map(|l| BoltLight {
            light_rgb: bgr_to_rgb(l.light_rgb),
            ..l
        });
        Ok(BoltProjectile {
            projectile_length,
            bulge_length,
            bulge_width,
            velocity,
            hit_effect,
            miss_effect,
            firing_sound_fx,
            front_rgb: bgr_to_rgb(front_rgb),
            mid_edge_rgb: bgr_to_rgb(mid_edge_rgb),
            middle_rgb: bgr_to_rgb(middle_rgb),
            back_rgb: bgr_to_rgb(back_rgb),
            edge_alpha,
            bolt_alpha,
            is_beam,
            beam_duration,
            beam_wiggle,
            point_light,
            arc_effect,
            arc_height,
            after_hit_time,
            area_effect_damage,
            area_effect_radius,
            light,
        })
    }
}
