// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Environment blocks of a mission: `[Cameras]`, `[Water]`, `[Terrain]`,
//! `[Weather]` and `[Artillery]`.
//!
//! The mission loader does not read these itself. Each block is read by a
//! subsystem, named on the field or the struct, so the typed model records
//! exactly the keys the shipped data carries. Every field is optional because
//! the C++ readers keep a constructor default for a missing key and this model
//! does not reproduce those defaults.

use super::read::{opt, select};
use crate::error::Result;
use mc2_formats::ini::FitIni;

/// An 8-bit RGB triple stored as separate `...Red`, `...Green`, `...Blue` keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Reads `<prefix>Red`, `<prefix>Green` and `<prefix>Blue`. The triple is
/// `None` unless all three keys are present.
fn rgb(fit: &FitIni, prefix: &str) -> Result<Option<Rgb>> {
    let r = opt(fit.read_u8(&format!("{prefix}Red")))?;
    let g = opt(fit.read_u8(&format!("{prefix}Green")))?;
    let b = opt(fit.read_u8(&format!("{prefix}Blue")))?;
    Ok(match (r, g, b) {
        (Some(r), Some(g), Some(b)) => Some(Rgb { r, g, b }),
        _ => None,
    })
}

/// `[Cameras]`. Read by `CCamera` setup in `mclib/camera.cpp` (block seek at
/// line 178, keys through the rest of that function).
#[derive(Debug, Clone, PartialEq)]
pub struct Cameras {
    /// `f ProjectionAngle`, `f PositionX|Y|Z`, `b Ready` (camera.cpp:179-189).
    pub projection_angle: Option<f32>,
    pub position_x: Option<f32>,
    pub position_y: Option<f32>,
    pub position_z: Option<f32>,
    pub ready: Option<bool>,
    /// Colour groups, each a `...Red|Green|Blue` triple (camera.cpp:190-240).
    /// The shipped `TerrainShadow` group falls back to the ambient colour in the
    /// C++ (camera.cpp:225-232); that fallback is not applied here.
    pub light: Option<Rgb>,
    pub ambient: Option<Rgb>,
    pub terrain_shadow: Option<Rgb>,
    /// `uc TerrainShadowColorEnabled` (camera.cpp:216-222).
    pub terrain_shadow_color_enabled: Option<u8>,
    pub seen: Option<Rgb>,
    pub base: Option<Rgb>,
    pub day_light: Option<Rgb>,
    pub day_ambient: Option<Rgb>,
    pub sunset_light: Option<Rgb>,
    pub night_light: Option<Rgb>,
    pub night_ambient: Option<Rgb>,
    /// `f LightDirYaw`, `f LightDirPitch`, `f DayToNightTime`, `f DayLightPitch`.
    pub light_dir_yaw: Option<f32>,
    pub light_dir_pitch: Option<f32>,
    pub day_to_night_time: Option<f32>,
    pub day_light_pitch: Option<f32>,
    /// Further `f` keys read by camera.cpp after the colour block.
    pub elevation_adjust_factor: Option<f32>,
    pub fov_max: Option<f32>,
    pub fov_min: Option<f32>,
    pub fog_full: Option<f32>,
    pub fog_start: Option<f32>,
    pub fog_transparency: Option<f32>,
    pub new_scale: Option<f32>,
    pub start_rotation: Option<f32>,
    pub zoom_max: Option<f32>,
    pub zoom_min: Option<f32>,
    /// `f[3] LODScales`.
    pub lod_scales: Option<Vec<f32>>,
    /// `l BaseTerrain`, `l UserMin`, `l UserMax`.
    pub base_terrain: Option<i32>,
    pub user_min: Option<i32>,
    pub user_max: Option<i32>,
    /// `ul FogColor`, packed as the C++ stores it.
    pub fog_color: Option<u32>,
}

pub(crate) fn load_cameras(fit: &mut FitIni) -> Result<Option<Cameras>> {
    if !select(fit, "Cameras")? {
        return Ok(None);
    }
    Ok(Some(Cameras {
        projection_angle: opt(fit.read_f32("ProjectionAngle"))?,
        position_x: opt(fit.read_f32("PositionX"))?,
        position_y: opt(fit.read_f32("PositionY"))?,
        position_z: opt(fit.read_f32("PositionZ"))?,
        ready: opt(fit.read_bool("Ready"))?,
        light: rgb(fit, "Light")?,
        ambient: rgb(fit, "Ambient")?,
        terrain_shadow: rgb(fit, "TerrainShadow")?,
        terrain_shadow_color_enabled: opt(fit.read_u8("TerrainShadowColorEnabled"))?,
        seen: rgb(fit, "Seen")?,
        base: rgb(fit, "Base")?,
        day_light: rgb(fit, "DayLight")?,
        day_ambient: rgb(fit, "DayAmbient")?,
        sunset_light: rgb(fit, "SunsetLight")?,
        night_light: rgb(fit, "NightLight")?,
        night_ambient: rgb(fit, "NightAmbient")?,
        light_dir_yaw: opt(fit.read_f32("LightDirYaw"))?,
        light_dir_pitch: opt(fit.read_f32("LightDirPitch"))?,
        day_to_night_time: opt(fit.read_f32("DayToNightTime"))?,
        day_light_pitch: opt(fit.read_f32("DayLightPitch"))?,
        elevation_adjust_factor: opt(fit.read_f32("ElevationAdjustFactor"))?,
        fov_max: opt(fit.read_f32("FOVMax"))?,
        fov_min: opt(fit.read_f32("FOVMin"))?,
        fog_full: opt(fit.read_f32("FogFull"))?,
        fog_start: opt(fit.read_f32("FogStart"))?,
        fog_transparency: opt(fit.read_f32("FogTransparency"))?,
        new_scale: opt(fit.read_f32("NewScale"))?,
        start_rotation: opt(fit.read_f32("StartRotation"))?,
        zoom_max: opt(fit.read_f32("ZoomMax"))?,
        zoom_min: opt(fit.read_f32("ZoomMin"))?,
        lod_scales: opt(fit.read_array::<f32>("LODScales", 3))?,
        base_terrain: opt(fit.read_i32("BaseTerrain"))?,
        user_min: opt(fit.read_i32("UserMin"))?,
        user_max: opt(fit.read_i32("UserMax"))?,
        fog_color: opt(fit.read_u32("FogColor"))?,
    }))
}

/// `[Water]`. Read in `mclib/terrain.cpp:1346-1358`.
#[derive(Debug, Clone, PartialEq)]
pub struct Water {
    /// `f Elevation` (terrain.cpp:1348), stored into the map's water depth.
    pub elevation: Option<f32>,
    /// `f Frequency`, `f Ampliture` (sic; terrain.cpp:1349-1350).
    pub frequency: Option<f32>,
    pub amplitude: Option<f32>,
    /// `ul AlphaShallow`, `ul AlphaMiddle`, `ul AlphaDeep` (terrain.cpp:1351-1353).
    pub alpha_shallow: Option<u32>,
    pub alpha_middle: Option<u32>,
    pub alpha_deep: Option<u32>,
    /// `f AlphaDepth`, `f ShallowDepth` (terrain.cpp:1354-1355).
    pub alpha_depth: Option<f32>,
    pub shallow_depth: Option<f32>,
}

pub(crate) fn load_water(fit: &mut FitIni) -> Result<Option<Water>> {
    if !select(fit, "Water")? {
        return Ok(None);
    }
    Ok(Some(Water {
        elevation: opt(fit.read_f32("Elevation"))?,
        frequency: opt(fit.read_f32("Frequency"))?,
        amplitude: opt(fit.read_f32("Ampliture"))?,
        alpha_shallow: opt(fit.read_u32("AlphaShallow"))?,
        alpha_middle: opt(fit.read_u32("AlphaMiddle"))?,
        alpha_deep: opt(fit.read_u32("AlphaDeep"))?,
        alpha_depth: opt(fit.read_f32("AlphaDepth"))?,
        shallow_depth: opt(fit.read_f32("ShallowDepth"))?,
    }))
}

/// `[Terrain]`. Read in `mclib/terrain.cpp:1367-1372` for the fractal
/// keys; the tiling and bounds keys are not read by the shipped code paths
/// checked here.
#[derive(Debug, Clone, PartialEq)]
pub struct Terrain {
    /// `f TerrainMinX`, `f TerrainMinY`.
    pub terrain_min_x: Option<f32>,
    pub terrain_min_y: Option<f32>,
    /// `f DetailTextureTilingFactor`, `f WaterDetailTilingFactor`,
    /// `f WaterTextureTilingFactor`.
    pub detail_texture_tiling_factor: Option<f32>,
    pub water_detail_tiling_factor: Option<f32>,
    pub water_texture_tiling_factor: Option<f32>,
    /// `l UserMin`, `l UserMax` (terrain.cpp:1367-1372).
    pub user_min: Option<i32>,
    pub user_max: Option<i32>,
    /// `uc Noise`, `uc Threshold` (terrain.cpp:1367-1372).
    pub noise: Option<u8>,
    pub threshold: Option<u8>,
}

pub(crate) fn load_terrain(fit: &mut FitIni) -> Result<Option<Terrain>> {
    if !select(fit, "Terrain")? {
        return Ok(None);
    }
    Ok(Some(Terrain {
        terrain_min_x: opt(fit.read_f32("TerrainMinX"))?,
        terrain_min_y: opt(fit.read_f32("TerrainMinY"))?,
        detail_texture_tiling_factor: opt(fit.read_f32("DetailTextureTilingFactor"))?,
        water_detail_tiling_factor: opt(fit.read_f32("WaterDetailTilingFactor"))?,
        water_texture_tiling_factor: opt(fit.read_f32("WaterTextureTilingFactor"))?,
        user_min: opt(fit.read_i32("UserMin"))?,
        user_max: opt(fit.read_i32("UserMax"))?,
        noise: opt(fit.read_u8("Noise"))?,
        threshold: opt(fit.read_u8("Threshold"))?,
    }))
}

/// `[Weather]`. Read in `code/weather.cpp:55-86` (and again at 103-134).
#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    /// `ul MaxRainDrops` (weather.cpp:59).
    pub max_rain_drops: Option<u32>,
    /// `f StartingRainLevel` (weather.cpp:66).
    pub starting_rain_level: Option<f32>,
    /// `l ChanceOfRain` (weather.cpp:76).
    pub chance_of_rain: Option<i32>,
    /// `f BaseLighteningChance` (weather.cpp:86).
    pub base_lightening_chance: Option<f32>,
}

pub(crate) fn load_weather(fit: &mut FitIni) -> Result<Option<Weather>> {
    if !select(fit, "Weather")? {
        return Ok(None);
    }
    Ok(Some(Weather {
        max_rain_drops: opt(fit.read_u32("MaxRainDrops"))?,
        starting_rain_level: opt(fit.read_f32("StartingRainLevel"))?,
        chance_of_rain: opt(fit.read_i32("ChanceOfRain"))?,
        base_lightening_chance: opt(fit.read_f32("BaseLighteningChance"))?,
    }))
}

/// `[Artillery]` strike counts. The shipped C++ does not read these keys: the
/// `Artillery` block that `code/artlry.cpp:395` seeks is a different file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Artillery {
    pub num_camera_strikes: Option<i32>,
    pub num_large_strikes: Option<i32>,
    pub num_sensor_strikes: Option<i32>,
    pub num_small_strikes: Option<i32>,
}

pub(crate) fn load_artillery(fit: &mut FitIni) -> Result<Option<Artillery>> {
    if !select(fit, "Artillery")? {
        return Ok(None);
    }
    Ok(Some(Artillery {
        num_camera_strikes: opt(fit.read_i32("NumCameraStrikes"))?,
        num_large_strikes: opt(fit.read_i32("NumLargeStrikes"))?,
        num_sensor_strikes: opt(fit.read_i32("NumSensorStrikes"))?,
        num_small_strikes: opt(fit.read_i32("NumSmallStrikes"))?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("{text}\n").into_bytes()).expect("fit")
    }

    #[test]
    fn cameras_read_colour_triples_and_arrays() {
        let mut f = fit(
            "FITini\n[Cameras]\nf PositionX = 1.5\nuc LightRed = 10\nuc LightGreen = 20\n\
             uc LightBlue = 30\nf[3] LODScales = 1, 2, 3\nul FogColor = 0xFF00FF\n",
        );
        let c = load_cameras(&mut f).unwrap().unwrap();
        assert_eq!(c.position_x, Some(1.5));
        assert_eq!(
            c.light,
            Some(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        assert_eq!(c.ambient, None);
        assert_eq!(c.lod_scales, Some(vec![1.0, 2.0, 3.0]));
        assert_eq!(c.fog_color, Some(0xFF00FF));
    }

    #[test]
    fn water_and_terrain_read_their_keys() {
        let mut f = fit("FITini\n[Water]\nf Elevation = 2\nul AlphaDeep = 128\n");
        let w = load_water(&mut f).unwrap().unwrap();
        assert_eq!(w.elevation, Some(2.0));
        assert_eq!(w.alpha_deep, Some(128));
        let mut f = fit("FITini\n[Terrain]\nuc Noise = 4\nl UserMin = -3\n");
        let t = load_terrain(&mut f).unwrap().unwrap();
        assert_eq!(t.noise, Some(4));
        assert_eq!(t.user_min, Some(-3));
        assert_eq!(t.threshold, None);
    }

    #[test]
    fn weather_and_artillery_are_optional_blocks() {
        let mut f = fit("FITini\n[Weather]\nl ChanceOfRain = 25\n");
        assert_eq!(
            load_weather(&mut f).unwrap().unwrap().chance_of_rain,
            Some(25)
        );
        let mut f = fit("FITini\n[Other]\nl x = 1\n");
        assert!(load_artillery(&mut f).unwrap().is_none());
    }
}
