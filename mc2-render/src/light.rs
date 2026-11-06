// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The game's light model: one infinite "sun" plus ambient, read from the mission `.fit`.
//!
//! Ported from `Camera::init` (the `[Cameras]` fields), `Camera::update` (the light direction and
//! the night flags), `Camera::getLightRed/Green/Blue` (the terrain vertex ramp), `Camera::updateDaylight`
//! (day/night interpolation: `Camera::update` keeps the per-frame call commented out, but `Camera::activate`
//! runs it once, which lights the logistics previews, [`LightModel::activate_daylight`]) and the lighting
//! loop of `TG_Shape::MultiTransformShape` (`tgl.cpp`) for shapes.
//!
//! The original evaluates all of this once per vertex on the CPU; here the same equations are
//! evaluated per pixel in WGSL (see `shaders/`), fed by [`LightUniform`].

use crate::math::{deg_to_rad, Vec3};
use mc2_formats::ini::FitIni;

/// Packed `0xAARRGGBB` DWORD colour (the layout of every colour in the fit files) to 0..1 floats.
pub fn argb_to_rgb(argb: u32) -> [f32; 3] {
    [
        ((argb >> 16) & 0xff) as f32 / 255.0,
        ((argb >> 8) & 0xff) as f32 / 255.0,
        (argb & 0xff) as f32 / 255.0,
    ]
}

const NIGHT_START_PITCH: f32 = 25.0;
const NIGHT_LIGHT_PITCH: f32 = 10.0;

#[derive(Clone, Debug, PartialEq)]
pub struct LightModel {
    /// Sun yaw in degrees (`LightDirYaw`).
    pub yaw: f32,
    /// Sun elevation in degrees (`LightDirPitch`, or `DayLightPitch` for day/night maps).
    pub pitch: f32,
    /// Sun colour, 0..1 (`LightRed/Green/Blue`).
    pub sun_color: [f32; 3],
    /// Ambient colour, 0..1 (`AmbientRed/Green/Blue`).
    pub ambient: [f32; 3],
    /// Colour of terrain in shadow when `TerrainShadowColorEnabled` (used by the colour-map bake).
    pub terrain_shadow: Option<[f32; 3]>,
    // Day/night key colours (`Camera::updateDaylight`).
    pub day_light: [f32; 3],
    pub day_ambient: [f32; 3],
    pub sunset_light: [f32; 3],
    pub night_light: [f32; 3],
    pub night_ambient: [f32; 3],
    /// Seconds for one full day; `<= 0` freezes the light at the fit values.
    pub day_to_night_time: f32,
    /// `FogColor` (0xRRGGBB) and the height-fog band (`FogStart` above `FogFull`, world z).
    pub fog_color: [f32; 3],
    pub fog_start: f32,
    pub fog_full: f32,
    pub fog_transparency: f32,
    /// Fit lighting levels: `isNight`/`nightFactor` as derived by `Camera::update`.
    pub is_night: bool,
    pub night_factor: f32,
}

impl Default for LightModel {
    fn default() -> Self {
        let mut m = LightModel {
            yaw: 0.0,
            pitch: 50.0,
            sun_color: [1.0, 1.0, 0.9],
            ambient: [0.3, 0.28, 0.28],
            terrain_shadow: None,
            day_light: [1.0, 1.0, 0.9],
            day_ambient: [0.3, 0.28, 0.28],
            // arbitrary defaults compiled into Camera::init
            sunset_light: [205.0 / 255.0, 84.0 / 255.0, 31.0 / 255.0],
            night_light: [64.0 / 255.0; 3],
            night_ambient: [32.0 / 255.0; 3],
            day_to_night_time: -1.0,
            fog_color: [0.8, 0.8, 0.85],
            fog_start: -1.0,
            fog_full: -2.0,
            fog_transparency: 1.0,
            is_night: false,
            night_factor: 0.0,
        };
        m.update_night_flags();
        m
    }
}

impl LightModel {
    /// `Camera::init`: the `[Cameras]` block of a mission fit (camera.cpp:197-355).
    ///
    /// With `DayToNightTime` in the fit the C++ `init` copies the `Day*` key colours and
    /// `DayLightPitch` into the live light (camera.cpp:341-349). `Camera::update` never calls `updateDaylight()`
    /// (commented out, camera.cpp:1511); `Camera::activate` does, once ([`LightModel::activate_daylight`]).
    /// Without the key the live light is the plain `Light*`/`Ambient*` values (camera.cpp:258-276).
    pub fn from_fit(fit: &mut FitIni) -> Self {
        let mut m = LightModel::default();
        if fit.seek_block("Cameras").is_err() {
            return m;
        }
        let byte = |fit: &FitIni, name: &str| fit.read_u8(name).ok().map(|v| v as f32 / 255.0);
        let rgb = |fit: &FitIni, r: &str, g: &str, b: &str, def: [f32; 3]| {
            [
                byte(fit, r).unwrap_or(def[0]),
                byte(fit, g).unwrap_or(def[1]),
                byte(fit, b).unwrap_or(def[2]),
            ]
        };
        m.sun_color = rgb(fit, "LightRed", "LightGreen", "LightBlue", m.sun_color);
        m.ambient = rgb(fit, "AmbientRed", "AmbientGreen", "AmbientBlue", m.ambient);
        if fit.read_u8("TerrainShadowColorEnabled").unwrap_or(0) != 0 {
            m.terrain_shadow = Some(rgb(
                fit,
                "TerrainShadowRed",
                "TerrainShadowGreen",
                "TerrainShadowBlue",
                m.ambient,
            ));
        }
        m.yaw = fit.read_f32("LightDirYaw").unwrap_or(m.yaw);
        m.pitch = fit.read_f32("LightDirPitch").unwrap_or(m.pitch);

        // `DayToNightTime` present: the day/night key colours are authoritative and the initial
        // light is the day light.  Absent: the key colours default to the plain light values.
        match fit.read_f32("DayToNightTime") {
            Err(_) => {
                m.day_to_night_time = -1.0;
                m.day_ambient = m.ambient;
                m.day_light = m.sun_color;
            }
            Ok(t) => {
                m.day_to_night_time = t;
                m.pitch = fit.read_f32("DayLightPitch").unwrap_or(m.pitch);
                m.day_light = rgb(
                    fit,
                    "DayLightRed",
                    "DayLightGreen",
                    "DayLightBlue",
                    m.sun_color,
                );
                m.day_ambient = rgb(
                    fit,
                    "DayAmbientRed",
                    "DayAmbientGreen",
                    "DayAmbientBlue",
                    m.ambient,
                );
                if fit.read_u8("SunsetLightRed").is_ok() {
                    m.sunset_light = rgb(
                        fit,
                        "SunsetLightRed",
                        "SunsetLightGreen",
                        "SunsetLightBlue",
                        m.sunset_light,
                    );
                } else {
                    m.sunset_light = [254.0 / 255.0, 68.0 / 255.0, 5.0 / 255.0];
                }
                m.night_light = rgb(
                    fit,
                    "NightLightRed",
                    "NightLightGreen",
                    "NightLightBlue",
                    m.night_light,
                );
                m.night_ambient = rgb(
                    fit,
                    "NightAmbientRed",
                    "NightAmbientGreen",
                    "NightAmbientBlue",
                    m.night_ambient,
                );
                m.ambient = m.day_ambient;
                m.sun_color = m.day_light;
            }
        }
        if let Ok(c) = fit.read_u32("FogColor") {
            m.fog_color = argb_to_rgb(c);
        }
        m.fog_start = fit.read_f32("FogStart").unwrap_or(m.fog_start);
        m.fog_full = fit.read_f32("FogFull").unwrap_or(m.fog_full);
        m.fog_transparency = fit.read_f32("FogTransparency").unwrap_or(1.0);
        m.update_night_flags();
        m
    }

    /// `Camera::update`: `isNight`/`nightFactor` from the sun elevation.
    pub fn update_night_flags(&mut self) {
        let mut altitude = self.pitch;
        if self.pitch > 90.0 {
            altitude = 180.0 - self.pitch;
        }
        if altitude < NIGHT_LIGHT_PITCH {
            self.is_night = true;
            self.night_factor = 1.0;
        } else {
            self.is_night = false;
            self.night_factor = if altitude > NIGHT_START_PITCH {
                0.0
            } else {
                (NIGHT_START_PITCH - self.pitch) / (NIGHT_START_PITCH - NIGHT_LIGHT_PITCH)
            };
        }
    }

    /// `lightDirection` of `Camera::init`/`Camera::update` (camera.cpp:266-268, 1517-1521) — points
    /// **towards** the sun in game world coordinates, normalised.  Used by the terrain vertex ramp
    /// (`vertexNormal * eye->lightDirection`).  Note the horizontal part is not scaled by
    /// `cos(pitch)`, so this is a flatter sun than the one shapes see.
    pub fn terrain_light_direction(&self) -> Vec3 {
        let a = deg_to_rad(self.yaw + 45.0);
        Vec3::new(a.cos(), a.sin(), deg_to_rad(self.pitch).sin()).normalized()
    }

    /// Direction towards the sun as the shapes see it.  `Camera::init` builds the TG infinite
    /// light with `EulerAngles(pitch, yaw + 135deg, 0)`; with Stuff's conventions its forward
    /// axis (the way the light travels) is `(cos p sin Y, -sin p, cos p cos Y)` in the Stuff
    /// frame.  Converted to world (x = -x_s, y = z_s, z = y_s) and negated that is
    /// `(cos p cos(yaw+45), cos p sin(yaw+45), sin p)`: a proper unit vector at elevation `pitch`.
    /// The matrix is camera.cpp:419-421 / `rotateLightRight` 2893.  The mission renderer and the
    /// logistics preview light every shape with this one; `terrain_light_direction` is for the
    /// terrain vertex ramp only (mapdata.cpp:1377, quad.cpp:805).
    pub fn shape_light_direction(&self) -> Vec3 {
        let p = deg_to_rad(self.pitch);
        let a = deg_to_rad(self.yaw + 45.0);
        Vec3::new(p.cos() * a.cos(), p.cos() * a.sin(), p.sin())
    }

    /// `Camera::getLightRed/Green/Blue(intensity)` as a 0..1 colour: `ambient + sun * intensity`
    /// for lit vertices, plain ambient otherwise.  This is both the terrain vertex colour ramp and,
    /// evaluated at an object's terrain position (`land->getTerrainLight`), the colour the shapes
    /// feed to their infinite light (`eye->setLightColor(0, lightRGB)` in `BldgAppearance::update`).
    pub fn ramp(&self, intensity: f32) -> [f32; 3] {
        if intensity > 0.0 {
            [
                (self.sun_color[0] * intensity + self.ambient[0]).min(1.0),
                (self.sun_color[1] * intensity + self.ambient[1]).min(1.0),
                (self.sun_color[2] * intensity + self.ambient[2]).min(1.0),
            ]
        } else {
            self.ambient
        }
    }

    /// Evaluates the day/night cycle at `pitch` (sun elevation over a full 0..360 turn).
    ///
    /// `Camera::updateDaylight` blends sunset hues in HSI space and tints the fog from the sky (the exact light
    /// and ambient of its one-off run are [`LightModel::activate_daylight`]).  This approximation keeps
    /// the interpolation of ambient/sun colour (same sine blend, same night branch) and replaces the
    /// HSI sunset hue blend with a linear RGB blend toward `sunset_light`.
    pub fn at_pitch(&self, pitch: f32) -> LightModel {
        let mut m = self.clone();
        m.pitch = pitch;
        if pitch <= 180.0 {
            let s = deg_to_rad(pitch).sin();
            for i in 0..3 {
                m.ambient[i] =
                    (self.day_ambient[i] - self.night_ambient[i]) * s + self.night_ambient[i];
            }
            let from_horizon = 90.0 - (90.0 - pitch).abs();
            const SUNSET_START: f32 = 35.0;
            if from_horizon < SUNSET_START {
                let k =
                    0.75 * deg_to_rad((SUNSET_START - from_horizon) * (90.0 / SUNSET_START)).sin();
                for i in 0..3 {
                    m.sun_color[i] = self.sun_color_day()[i] * (1.0 - k) + self.sunset_light[i] * k;
                }
                const DARK_START: f32 = 5.0;
                if from_horizon < DARK_START {
                    let keep = deg_to_rad(from_horizon * (90.0 / DARK_START)).sin();
                    for i in 0..3 {
                        m.sun_color[i] = m.sun_color[i] * keep + self.night_light[i] * (1.0 - keep);
                    }
                }
            } else {
                m.sun_color = self.day_light;
            }
        } else {
            let moon = 180.0 - (180.0 - pitch).abs();
            m.pitch = moon;
            m.ambient = self.night_ambient;
            m.sun_color = self.night_light;
        }
        m.update_night_flags();
        if pitch > 180.0 {
            m.is_night = true;
            m.night_factor = 1.0;
        }
        m
    }

    fn sun_color_day(&self) -> [f32; 3] {
        self.day_light
    }

    /// `Camera::activate` on a ready camera: `updateDaylight(true)` (mclib/camera.cpp:1775-1784, 1140-1380), the
    /// light and ambient part. `SimpleCamera::setMech`/`setVehicle`/`setComponent` call `activate()`
    /// (code/simplecamera.cpp:358, 428, 502), so every logistics/briefing model preview is lit by this, not by the fit's plain
    /// `Light*` values: the instrumented C++ reference shows `lightRed/Green/Blue` 252/159/140 and ambient 31 for
    /// the shipped `cameras.fit` (sun 255, ambient 31, pitch 22.5).
    ///
    /// Without `DayToNightTime` the transition time is taken as 3600 s (camera.cpp:1144-1146), so the pitch comes
    /// back as `DayLightPitch` (`LightDirPitch` for such a fit, camera.cpp:291). Below 35 degrees from the horizon
    /// the sun colour is the day light blended toward the sunset colour in HSI space (camera.cpp:1208-1246),
    /// below 5 degrees faded toward the night light (camera.cpp:1319-1328); the ambient is the night ambient
    /// plus `(day - night) * sin(pitch)` (camera.cpp:1200-1206); at night (pitch above 180) both are the night
    /// values (camera.cpp:1351-1366). The C++ stores the results in `unsigned char` members, which truncates
    /// (camera.cpp:1373-1379). The
    /// sky and fog colours it also derives are not modelled (the previews draw without fog). The night flags are
    /// set per branch: by day from the altitude (camera.cpp:1177-1196), above 180 degrees night with factor 1
    /// (camera.cpp:1345-1349).
    pub fn activate_daylight(&self) -> LightModel {
        let byte = |v: f32| (v * 255.0).round();
        let bytes = |c: [f32; 3]| c.map(byte);
        let (day_light, sunset, night_light) = (
            bytes(self.day_light),
            bytes(self.sunset_light),
            bytes(self.night_light),
        );
        let (day_ambient, night_ambient) = (bytes(self.day_ambient), bytes(self.night_ambient));

        let transition = if self.day_to_night_time <= 0.0 {
            3600.0f32
        } else {
            self.day_to_night_time
        };
        let mut day_time = self.pitch / 360.0 * transition;
        while day_time > transition {
            day_time -= transition;
        }
        let mut pitch = day_time * (1.0 / transition) * 360.0;

        // the renderer flags are set per branch (camera.cpp:1177-1196 by day, 1345-1349 by night)
        let (light, ambient, is_night, night_factor) = if pitch <= 180.0 {
            let altitude = if pitch > 90.0 { 180.0 - pitch } else { pitch };
            let night_factor = if altitude > NIGHT_START_PITCH {
                0.0
            } else {
                ((NIGHT_START_PITCH - altitude) / (NIGHT_START_PITCH - NIGHT_LIGHT_PITCH)).min(1.0)
            };
            let sine = deg_to_rad(pitch).sin();
            let ambient: [f32; 3] = std::array::from_fn(|i| {
                (day_ambient[i] - night_ambient[i]) * sine + night_ambient[i]
            });
            let from_horizon = 90.0 - (90.0 - pitch).abs();
            const SUNSET_START: f32 = 35.0;
            let light = if from_horizon < SUNSET_START {
                let (sh, ss, si) = rgb2hsi(sunset.map(|c| c / 255.0));
                let (dh, ds, di) = rgb2hsi(day_light.map(|c| c / 255.0));
                let k =
                    0.75 * deg_to_rad((SUNSET_START - from_horizon) * (90.0 / SUNSET_START)).sin();
                let rgb = hsi2rgb(
                    sh * k + dh * (1.0 - k),
                    ss * k + ds * (1.0 - k),
                    si * k + di * (1.0 - k),
                );
                let mut light = rgb.map(|c| c.clamp(0.0, 0.99) * 255.0);
                const DARK_START: f32 = 5.0;
                if from_horizon < DARK_START {
                    let keep = deg_to_rad(from_horizon * (90.0 / DARK_START)).sin();
                    for (l, n) in light.iter_mut().zip(night_light) {
                        *l = *l * keep + n * (1.0 - keep);
                    }
                }
                light
            } else {
                day_light
            };
            (light, ambient, altitude < NIGHT_LIGHT_PITCH, night_factor)
        } else {
            pitch = 180.0 - (180.0 - pitch).abs();
            (night_light, night_ambient, true, 1.0)
        };

        // `unsigned char` stores truncate
        let to_unit = |c: [f32; 3]| c.map(|v| v.clamp(0.0, 255.0).trunc() / 255.0);
        let mut m = self.clone();
        m.pitch = pitch;
        m.sun_color = to_unit(light);
        m.ambient = to_unit(ambient);
        m.is_night = is_night;
        m.night_factor = night_factor;
        m
    }
}

// The HSI colour wheel of camera.cpp:1057-1063: red at 0, green at 120, blue at 240 degrees.
const RHAT: (f32, f32) = (1.0, 0.0);
const GHAT: (f32, f32) = (-0.5, 0.866_025_4);
const BHAT: (f32, f32) = (-0.5, -0.866_025_4);
const TWO_PI: f32 = std::f32::consts::TAU;

/// `rgb2hsi` (camera.cpp:1066-1088): hue in 0..1 turns, saturation, intensity.
fn rgb2hsi([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let intensity = (r + g + b) / 3.0;
    if intensity <= 0.0 {
        return (0.0, 0.0, intensity);
    }
    let saturation = 1.0 - r.min(g).min(b) / intensity;
    let cx = r * RHAT.0 + g * GHAT.0 + b * BHAT.0;
    let cy = r * RHAT.1 + g * GHAT.1 + b * BHAT.1;
    let mut hue = cy.atan2(cx);
    if hue < 0.0 {
        hue += TWO_PI;
    }
    (hue / TWO_PI, saturation, intensity)
}

/// `hsi2rgb` (camera.cpp:1090-1129).
fn hsi2rgb(hue: f32, saturation: f32, intensity: f32) -> [f32; 3] {
    let third = 1.0 / 3.0;
    let thue = if hue < third {
        hue
    } else if hue < 2.0 * third {
        hue - third
    } else {
        hue - 2.0 * third
    };
    let (chatx, chaty) = ((TWO_PI * thue).cos(), (TWO_PI * thue).sin());
    let ti = 3.0 * intensity;
    let b = (1.0 - saturation) * intensity;
    let tib = ti - b;
    let mut denominator = chaty * (GHAT.0 - RHAT.0) - chatx * (GHAT.1 - RHAT.1);
    if denominator == 0.0 {
        denominator = 0.000_001;
    }
    let g =
        ((b * BHAT.1 + tib * RHAT.1) * chatx - (b * BHAT.0 + tib * RHAT.0) * chaty) / denominator;
    let r = tib - g;
    if hue < third {
        [r, g, b]
    } else if hue < 2.0 * third {
        // swap = b; b = g; g = r; r = swap
        [b, r, g]
    } else {
        // swap = b; b = r; r = g; g = swap
        [g, b, r]
    }
}

impl LightModel {
    pub fn uniform(&self) -> LightUniform {
        let ts = self.terrain_light_direction();
        let sh = self.shape_light_direction();
        LightUniform {
            terrain_dir: [ts.x, ts.y, ts.z, 0.0],
            shape_dir: [sh.x, sh.y, sh.z, 0.0],
            sun: [
                self.sun_color[0],
                self.sun_color[1],
                self.sun_color[2],
                self.night_factor,
            ],
            ambient: [
                self.ambient[0],
                self.ambient[1],
                self.ambient[2],
                if self.is_night { 1.0 } else { 0.0 },
            ],
            fog: [
                self.fog_color[0],
                self.fog_color[1],
                self.fog_color[2],
                self.fog_transparency,
            ],
            fog_height: [self.fog_start, self.fog_full, 0.0, 0.0],
        }
    }
}

/// GPU mirror of the light model (see `shaders/common.wgsl`).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightUniform {
    /// Terrain-ramp sun direction (towards the sun), w unused.
    pub terrain_dir: [f32; 4],
    /// Shape sun direction (towards the sun), w unused.
    pub shape_dir: [f32; 4],
    /// rgb = sun colour, a = night factor.
    pub sun: [f32; 4],
    /// rgb = ambient colour, a = 1 at night.
    pub ambient: [f32; 4],
    /// rgb = fog colour, a = fog transparency.
    pub fog: [f32; 4],
    /// x = fog start (world z), y = fog full.
    pub fog_height: [f32; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(text.as_bytes().to_vec()).unwrap()
    }

    const MC2_01: &str = "FITini\r\n[Cameras]\r\nf ProjectionAngle = 21.25\r\nuc LightRed = 255\r\nuc LightGreen = 255\r\nuc LightBlue = 225\r\nuc AmbientBlue = 78\r\nuc AmbientGreen = 71\r\nuc AmbientRed = 71\r\nuc TerrainShadowColorEnabled = 1\r\nuc TerrainShadowBlue = 78\r\nuc TerrainShadowGreen = 71\r\nuc TerrainShadowRed = 71\r\nuc DayLightRed = 255\r\nuc DayLightGreen = 255\r\nuc DayLightBlue = 225\r\nuc DayAmbientBlue = 78\r\nuc DayAmbientGreen = 71\r\nuc DayAmbientRed = 71\r\nuc SunsetLightRed = 205\r\nuc SunsetLightGreen = 84\r\nuc SunsetLightBlue = 31\r\nuc NightLightRed = 64\r\nuc NightLightGreen = 64\r\nuc NightLightBlue = 64\r\nuc NightAmbientBlue = 32\r\nuc NightAmbientGreen = 32\r\nuc NightAmbientRed = 32\r\nf LightDirPitch = 50.000000\r\nf DayLightPitch = 50.000000\r\nf DayToNightTime = -1.000000\r\nf LightDirYaw = 11.000000\r\nf FogStart = -1.000000\r\nf FogFull = -2.000000\r\nul FogColor = 14802644\r\nf FogTransparency = 1.000000\r\nFITend\r\n";

    #[test]
    fn reads_the_cameras_block() {
        let mut f = fit(MC2_01);
        let m = LightModel::from_fit(&mut f);
        assert!((m.sun_color[2] - 225.0 / 255.0).abs() < 1e-6);
        assert!((m.ambient[0] - 71.0 / 255.0).abs() < 1e-6);
        assert_eq!(m.pitch, 50.0);
        assert_eq!(m.yaw, 11.0);
        assert!(m.terrain_shadow.is_some());
        assert!(!m.is_night);
        assert_eq!(m.night_factor, 0.0);
        // 14802644 = 0xE1D5D4... check channel unpacking
        assert!((m.fog_color[0] - ((14802644u32 >> 16) & 0xff) as f32 / 255.0).abs() < 1e-6);
        assert_eq!(m.fog_start, -1.0);
    }

    /// `updateDaylight(true)` on the shipped preview camera (`data/cameras/cameras.fit`: sun 255, ambient 31,
    /// pitch 22.5, no `DayToNightTime`). The expected bytes are what the instrumented C++ reference wrote
    /// (rust/docs/evidence/rc-vqa-vertexdump.md): `lightRed/Green/Blue` 252 159 140, ambient 31 31 31, and light 0
    /// `0x00ffbeab` = `min(light + ambient, 255)`; `nightFactor` 0.166667.
    #[test]
    fn activation_daylight_is_the_cpp_sunset_blend() {
        let cameras = "FITini\r\n[Cameras]\r\nuc LightRed = 255\r\nuc LightGreen = 255\r\nuc LightBlue = 255\r\nuc AmbientRed = 31\r\nuc AmbientGreen = 31\r\nuc AmbientBlue = 31\r\nf LightDirYaw = -61.25\r\nf LightDirPitch = 22.5\r\nFITend\r\n";
        let m = LightModel::from_fit(&mut fit(cameras)).activate_daylight();
        let bytes = |c: [f32; 3]| c.map(|v| (v * 255.0).round() as u32);
        assert_eq!(bytes(m.sun_color), [252, 159, 140]);
        assert_eq!(bytes(m.ambient), [31, 31, 31]);
        assert_eq!(bytes(m.ramp(1.0)), [0xff, 0xbe, 0xab]);
        assert!((m.pitch - 22.5).abs() < 1e-3);
        assert!(!m.is_night);
        assert!((m.night_factor - 1.0 / 6.0).abs() < 1e-4);

        // high sun: the day light unchanged; the ambient still blends toward the night ambient by sin(pitch)
        let noon =
            LightModel::from_fit(&mut fit(&cameras.replace("22.5", "90.0"))).activate_daylight();
        assert_eq!(bytes(noon.sun_color), [255, 255, 255]);
        assert_eq!(bytes(noon.ambient), [31, 31, 31]);
        // below the horizon: the night light and ambient
        let night =
            LightModel::from_fit(&mut fit(&cameras.replace("22.5", "200.0"))).activate_daylight();
        assert_eq!(bytes(night.sun_color), [64, 64, 64]);
        assert_eq!(bytes(night.ambient), [32, 32, 32]);
        assert!(night.is_night);
    }

    /// The HSI helpers invert each other away from grey (camera.cpp:1066-1129).
    #[test]
    fn hsi_round_trips() {
        for rgb in [
            [205.0, 84.0, 31.0],
            [40.0, 200.0, 90.0],
            [30.0, 60.0, 220.0],
            [250.0, 250.0, 10.0],
        ] {
            let c = rgb.map(|v: f32| v / 255.0);
            let (h, s, i) = rgb2hsi(c);
            let back = hsi2rgb(h, s, i);
            for k in 0..3 {
                assert!((back[k] - c[k]).abs() < 1e-4, "{rgb:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn terrain_light_is_unit_and_points_up() {
        let m = LightModel::default();
        let d = m.terrain_light_direction();
        assert!((d.length() - 1.0).abs() < 1e-5);
        assert!(d.z > 0.0);
        let s = m.shape_light_direction();
        assert!((s.length() - 1.0).abs() < 1e-5);
        // shape sun is exactly at `pitch` degrees elevation
        assert!((s.z - deg_to_rad(m.pitch).sin()).abs() < 1e-6);
        // both suns share the same azimuth
        let az = |v: Vec3| v.y.atan2(v.x);
        assert!((az(d) - az(s)).abs() < 1e-5);
    }

    #[test]
    fn azimuth_follows_yaw_plus_45() {
        let m = LightModel {
            yaw: 11.0,
            ..LightModel::default()
        };
        let d = m.shape_light_direction();
        let az = d.y.atan2(d.x).to_degrees();
        assert!((az - 56.0).abs() < 1e-3, "azimuth {az}");
    }

    /// camera.cpp:341-349: with `DayToNightTime` the live light starts as the `Day*` values and
    /// `DayLightPitch`; nothing re-runs `updateDaylight()` afterwards.  Without the key the
    /// `Day*` values are copies of the plain ones (camera.cpp:258-276).
    #[test]
    fn day_to_night_key_starts_on_the_day_light() {
        let base = "FITini\r\n[Cameras]\r\nuc LightRed = 10\r\nuc LightGreen = 20\r\nuc LightBlue = 30\r\nuc AmbientRed = 40\r\nuc AmbientGreen = 50\r\nuc AmbientBlue = 60\r\nf LightDirPitch = 70\r\nf LightDirYaw = 11\r\n";
        let with_key = format!(
            "{base}f DayToNightTime = 600\r\nf DayLightPitch = 33\r\nuc DayLightRed = 200\r\nuc DayLightGreen = 210\r\nuc DayLightBlue = 220\r\nuc DayAmbientRed = 90\r\nuc DayAmbientGreen = 91\r\nuc DayAmbientBlue = 92\r\nuc NightLightRed = 1\r\nuc NightLightGreen = 1\r\nuc NightLightBlue = 1\r\nuc NightAmbientRed = 2\r\nuc NightAmbientGreen = 2\r\nuc NightAmbientBlue = 2\r\nFITend\r\n"
        );
        let m = LightModel::from_fit(&mut fit(&with_key));
        assert_eq!(m.day_to_night_time, 600.0);
        assert_eq!(m.pitch, 33.0);
        assert_eq!(m.sun_color, [200.0 / 255.0, 210.0 / 255.0, 220.0 / 255.0]);
        assert_eq!(m.ambient, [90.0 / 255.0, 91.0 / 255.0, 92.0 / 255.0]);
        // no key: the plain values are the light, the day copies equal them, pitch is LightDirPitch
        let m = LightModel::from_fit(&mut fit(&format!("{base}FITend\r\n")));
        assert_eq!(m.day_to_night_time, -1.0);
        assert_eq!(m.pitch, 70.0);
        assert_eq!(m.sun_color, [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0]);
        assert_eq!(m.day_light, m.sun_color);
        assert_eq!(m.day_ambient, m.ambient);
    }

    #[test]
    fn ramp_matches_get_light_red() {
        let mut f = fit(MC2_01);
        let m = LightModel::from_fit(&mut f);
        // intensity 0.5: 255*0.5 + 71 = 198.5 (float2long rounds in the original; here continuous)
        let c = m.ramp(0.5);
        assert!((c[0] - (255.0 * 0.5 + 71.0) / 255.0).abs() < 1e-5);
        // saturates
        assert_eq!(m.ramp(1.0)[0], 1.0);
        // no light: ambient only
        assert_eq!(m.ramp(-0.2), m.ambient);
        assert_eq!(m.ramp(0.0), m.ambient);
    }

    #[test]
    fn night_flags_follow_elevation() {
        let mut m = LightModel::default();
        m.pitch = 5.0;
        m.update_night_flags();
        assert!(m.is_night && m.night_factor == 1.0);
        m.pitch = 17.5;
        m.update_night_flags();
        assert!(!m.is_night);
        assert!((m.night_factor - 0.5).abs() < 1e-5);
        m.pitch = 60.0;
        m.update_night_flags();
        assert_eq!(m.night_factor, 0.0);
        // past 90 degrees the sun is going down again
        m.pitch = 175.0;
        m.update_night_flags();
        assert!(m.is_night);
    }

    #[test]
    fn day_cycle_runs_from_night_to_day() {
        let mut f = fit(MC2_01);
        let base = LightModel::from_fit(&mut f);
        let noon = base.at_pitch(90.0);
        assert!((noon.ambient[0] - base.day_ambient[0]).abs() < 1e-5);
        assert_eq!(noon.sun_color, base.day_light);
        let night = base.at_pitch(270.0);
        assert_eq!(night.ambient, base.night_ambient);
        assert!(night.is_night);
        // sunset tint appears near the horizon
        let dusk = base.at_pitch(15.0);
        assert!(dusk.sun_color[2] < noon.sun_color[2]);
    }

    #[test]
    fn uniform_layout_is_vec4_aligned() {
        assert_eq!(std::mem::size_of::<LightUniform>(), 6 * 16);
    }
}
