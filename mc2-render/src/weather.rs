// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Weather: rain, lightning, thunder and the darkening they cause (`code/weather.cpp`, `code/weather.h`).
//!
//! The C++ `Weather` object is set up from the mission's `[Weather]` block and runs once per game frame: `update`
//! moves the rain level, the drop pool, the lightning and the thunder countdown, and `render` draws the rain
//! streaks. It never changes gameplay, so here it is presentation state. Its random draws use the same `gos_rand`
//! helpers as the C++ (`RandomNumber` and `RollDice`, `mclib/routines.cpp`) on a stream of its own, and the sounds it
//! asks for leave as [`WeatherEvent`]s for the host to play.
//!
//! Where the C++ departs from what its own comments describe, the port follows the comments. Each departure is one
//! entry in `rust/docs/DIFFERENCES.md`.

use crate::camera::GameCamera;
use crate::fx::{FxBatch, FxBlend, FxVertex};
use crate::light::LightModel;
use crate::math::Vec3;
use mc2_formats::ini::FitIni;

/// `BASE_RAIN_UPDATE_TIME` (weather.h): seconds between rain-level steps.
pub const BASE_RAIN_UPDATE_TIME: f32 = 30.0;
/// `BASE_RAIN_HEIGHT`: drops spawn this far above the eye.
pub const BASE_RAIN_HEIGHT: f32 = 500.0;
/// `BASE_RAIN_LENGTH`: nominal streak length, world units.
pub const BASE_RAIN_LENGTH: f32 = 10.0;
/// `BASE_RAIN_VEL`: fall speed at rain level 1, world units per second.
pub const BASE_RAIN_VEL: f32 = 300.0;
/// `BASE_RAIN_LIGHT_RATE`: how fast the light level follows the rain level, per second.
pub const BASE_RAIN_LIGHT_RATE: f32 = 0.01;
/// `BASE_RAIN_RANDOM_POS_FACTOR`: horizontal spawn spread (each side).
pub const BASE_RAIN_RANDOM_POS_FACTOR: i32 = 500;
/// `BASE_RAIN_RANDOM_HGT_FACTOR`: vertical spawn spread (each side).
pub const BASE_RAIN_RANDOM_HGT_FACTOR: i32 = 250;
/// `BASE_RAIN_RANDOM_LEN_FACTOR`: streak length spread (each side).
pub const BASE_RAIN_RANDOM_LEN_FACTOR: i32 = 10;
/// `BASE_LIGHTENING_CHANCE`: the default `BaseLighteningChance`, per check, as a fraction.
pub const BASE_LIGHTENING_CHANCE: f32 = 0.03;
/// `BASE_LIGHTENING_FALLOFF`: how fast a flash dies, per second.
pub const BASE_LIGHTENING_FALLOFF: i32 = 400;
/// `BASE_LIGHTENING_FLASH_CHECK`: seconds between lightning checks.
pub const BASE_LIGHTENING_FLASH_CHECK: f32 = 1.0;
/// `BASE_THUNDER_RANDOM_START`: thunder is heard after `RandomNumber(5)` seconds.
pub const BASE_THUNDER_RANDOM_START: i32 = 5;
/// `MaxRainDrops` is clamped to this (weather.cpp:63).
pub const MAX_RAIN_DROPS: u32 = 500;

/// `RAIN1`..`RAIN4` (`mclib/sounds.h`): the rain loop, by rain level. `mc2_audio::ids::sound` names the same ids.
pub const RAIN_SOUNDS: [u32; 4] = [81, 82, 83, 84];
/// `THUNDER1`..`THUNDER4` (`mclib/sounds.h`): the crack of thunder, by flash brightness.
pub const THUNDER_SOUNDS: [u32; 4] = [77, 78, 79, 80];

/// What the weather asks of the sound system during an update (`soundSystem->...` in weather.cpp).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherEvent {
    /// `setIsRaining(id)`: the rain loop on the fire channel. `0` = none.
    RainLoop(u32),
    /// `playDigitalSample(id)`: a crack of thunder.
    Thunder(u32),
}

/// The presentation stream: PCG32 (O'Neill), top 15 bits, as `gos_rand()`'s 15-bit values. The same algorithm and
/// seeding as the sim's visual stream (`mc2-sim/src/rng.rs`), kept here so the renderer does not depend on the sim.
struct PresentationRng {
    state: u64,
    inc: u64,
}

impl PresentationRng {
    /// `pcg32_srandom(seed, 54)`.
    fn new(seed: u32) -> Self {
        let mut rng = PresentationRng {
            state: 0,
            inc: (54u64 << 1) | 1,
        };
        rng.step();
        rng.state = rng.state.wrapping_add(u64::from(seed));
        rng.step();
        rng
    }

    fn step(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// `gos_rand()`: a value in `0..32768`.
    fn next15(&mut self) -> i32 {
        (self.step() >> 17) as i32
    }
}

/// The `[Weather]` block, with the C++ defaults and clamps applied (weather.cpp:55-96).
#[derive(Clone, Debug, PartialEq)]
pub struct WeatherConfig {
    /// `MaxRainDrops`, default 500, clamped to 500. Zero switches the weather off (weather.cpp:160-171).
    pub max_rain_drops: u32,
    /// `StartingRainLevel`, default 0, clamped to `0..=4`.
    pub starting_rain_level: f32,
    /// `ChanceOfRain`, default 0, clamped to `0..=100`: the percent chance of rain starting at each check.
    pub chance_of_rain: i32,
    /// `BaseLighteningChance`, default [`BASE_LIGHTENING_CHANCE`], clamped to `0..=0.1`.
    pub base_lightening_chance: f32,
}

impl WeatherConfig {
    /// The block's values as read (`None` = key absent), with the C++ defaults and clamps.
    pub fn new(
        max_rain_drops: Option<u32>,
        starting_rain_level: Option<f32>,
        chance_of_rain: Option<i32>,
        base_lightening_chance: Option<f32>,
    ) -> Self {
        Self {
            max_rain_drops: max_rain_drops.unwrap_or(MAX_RAIN_DROPS).min(MAX_RAIN_DROPS),
            starting_rain_level: starting_rain_level.unwrap_or(0.0).clamp(0.0, 4.0),
            chance_of_rain: chance_of_rain.unwrap_or(0).clamp(0, 100),
            base_lightening_chance: base_lightening_chance
                .unwrap_or(BASE_LIGHTENING_CHANCE)
                .clamp(0.0, 0.1),
        }
    }

    /// The `[Weather]` block of a mission fit, or `None` when the mission has none (`seekBlock` fails).
    pub fn from_fit(fit: &mut FitIni) -> Option<Self> {
        fit.seek_block("Weather").ok()?;
        // `readIdULong` on `MaxRainDrops`: read as `ul`, or as the `l` the fit may hold.
        let max = fit
            .read_u32("MaxRainDrops")
            .or_else(|_| fit.read_i32("MaxRainDrops").map(|v| v as u32))
            .ok();
        Some(Self::new(
            max,
            fit.read_f32("StartingRainLevel").ok(),
            fit.read_i32("ChanceOfRain").ok(),
            fit.read_f32("BaseLighteningChance").ok(),
        ))
    }
}

/// One rain drop: its head position (world units) and streak length (`RainDrops`, weather.h).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RainDrop {
    pub position: Vec3,
    pub length: f32,
}

/// The weather of one mission (`Weather`, weather.h).
pub struct Weather {
    config: WeatherConfig,
    /// `weatherActive`: false when the mission has no block or no drops.
    active: bool,
    /// Presentation-only draws (`gos_rand` in the C++): a stream of its own, so the sky never moves the sim's numbers.
    rng: PresentationRng,
    /// `rainLevel`: 0 dry, 1 drizzle, 2 rain, 3 storm, capped at 4.
    rain_level: f32,
    /// `rainLightLevel`: follows `rain_level` slowly; darkens the light while above 1.
    rain_light_level: f32,
    /// `rainTrend` (+1 or -1) and `rainFactor`: the change in `rain_level` per 30 s check.
    rain_trend: f32,
    rain_factor: f32,
    /// `rainUpdateTime`: seconds until the next rain-level check.
    rain_update_time: f32,
    /// `lighteningLevel`: brightness of the current flash, 0..255.
    lightening_level: f32,
    /// `lighteningCheck`: seconds until the next lightning check.
    lightening_check: f32,
    /// `thunderTime` and `thunderSFX`: the crack of thunder waiting to be heard.
    thunder_time: f32,
    thunder_sfx: Option<u32>,
    /// `rainDrops`, of `totalRainDrops` slots, and how many are in service.
    drops: Vec<RainDrop>,
    current_drops: usize,
    /// Events raised by the last updates, drained by [`Weather::take_events`].
    events: Vec<WeatherEvent>,
}

impl Weather {
    /// `Weather::init(maxDrops, startingRain, baseRainChance, baseLighteningChance)` plus the start-up draw of the
    /// trend when the mission starts already raining. `seed` seeds the presentation stream.
    pub fn new(config: WeatherConfig, seed: u32) -> Self {
        let active = config.max_rain_drops > 0;
        let mut w = Weather {
            active,
            rng: PresentationRng::new(seed),
            rain_level: config.starting_rain_level,
            rain_light_level: config.starting_rain_level,
            rain_trend: 1.0,
            rain_factor: 0.0,
            // Force an update on the first frame, in case it is already raining (weather.cpp:186).
            rain_update_time: 0.0,
            lightening_level: 0.0,
            lightening_check: BASE_LIGHTENING_FLASH_CHECK,
            thunder_time: 0.0,
            thunder_sfx: None,
            drops: vec![RainDrop::default(); config.max_rain_drops as usize],
            current_drops: 0,
            events: Vec::new(),
            config,
        };
        if w.rain_level != 0.0 {
            w.rain_factor = w.new_rain_factor();
            w.rain_trend = 1.0;
        }
        w
    }

    /// Whether the mission has weather at all (`weatherActive`).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The current rain level (`rainLevel`).
    pub fn rain_level(&self) -> f32 {
        self.rain_level
    }

    /// The darkening level (`rainLightLevel`).
    pub fn rain_light_level(&self) -> f32 {
        self.rain_light_level
    }

    /// The current lightning brightness (`lighteningLevel`), 0..255.
    pub fn lightening_level(&self) -> f32 {
        self.lightening_level
    }

    /// The drops in service (`currentRainDrops`).
    pub fn current_drops(&self) -> usize {
        self.current_drops
    }

    /// The events raised since the last call, in order.
    pub fn take_events(&mut self) -> Vec<WeatherEvent> {
        std::mem::take(&mut self.events)
    }

    /// `RollDice(percent)` (`mclib/routines.cpp:188`): `((gos_rand() * 100) >> 15) < percent`.
    fn roll_dice(&mut self, percent: i32) -> bool {
        let r = i64::from(self.rng.next15());
        ((r * 100) >> 15) < i64::from(percent)
    }

    /// `RandomNumber(range)`: `gos_rand() % range`.
    fn random(&mut self, range: i32) -> i32 {
        self.rng.next15() % range
    }

    /// `0.05f + float(RandomNumber(100)) / 1000.0f`: a new rain trend step.
    fn new_rain_factor(&mut self) -> f32 {
        0.05 + self.random(100) as f32 / 1000.0
    }

    /// The rain-loop sound for a rain level (weather.cpp:204-214).
    fn rain_loop_sound(level: f32) -> u32 {
        if level > 3.0 {
            RAIN_SOUNDS[0]
        } else if level > 2.5 {
            RAIN_SOUNDS[1]
        } else if level > 2.0 {
            RAIN_SOUNDS[2]
        } else if level > 1.0 {
            RAIN_SOUNDS[3]
        } else {
            0
        }
    }

    /// The crack of thunder for a flash (weather.cpp:294-310).
    fn thunder_sound(flash: f32) -> u32 {
        if flash > 210.0 {
            THUNDER_SOUNDS[0]
        } else if flash > 176.0 {
            THUNDER_SOUNDS[2]
        } else if flash > 128.0 {
            THUNDER_SOUNDS[1]
        } else {
            THUNDER_SOUNDS[3]
        }
    }

    /// A drop's respawn (weather.cpp:402-411 and 425-434): a spot around `start`, a fresh length. Draws in the C++
    /// order: x, y, z, then the length.
    fn respawn(&mut self, index: usize, start: Vec3) {
        let spread = BASE_RAIN_RANDOM_POS_FACTOR;
        let x = self.random(spread * 2) - spread;
        let y = self.random(spread * 2) - spread;
        let hgt = BASE_RAIN_RANDOM_HGT_FACTOR;
        let z = self.random(hgt * 2) - hgt;
        let len = self.random(BASE_RAIN_RANDOM_LEN_FACTOR * 2) - BASE_RAIN_RANDOM_LEN_FACTOR;
        self.drops[index] = RainDrop {
            position: Vec3::new(start.x + x as f32, start.y + y as f32, start.z + z as f32),
            length: BASE_RAIN_LENGTH + len as f32,
        };
    }

    /// `Weather::update` (weather.cpp:192). `dt` is the frame length in seconds. `ground_at(x, y)` is the terrain
    /// height under a point (`land->getTerrainElevation`); `eye` is the camera position in game world coordinates.
    pub fn update(&mut self, dt: f32, eye: Vec3, ground_at: &dyn Fn(f32, f32) -> f32) {
        if !self.active {
            return;
        }

        // Check whether the rain level changes. The check runs every BASE_RAIN_UPDATE_TIME seconds.
        self.rain_update_time -= dt;
        if self.rain_update_time <= 0.0 {
            self.rain_level += self.rain_factor * self.rain_trend;
            // The C++ leaves this at zero when it is dry, so every later frame repeats the check; here the
            // cadence holds in every case (see DIFFERENCES.md).
            self.rain_update_time = BASE_RAIN_UPDATE_TIME;
            self.events
                .push(WeatherEvent::RainLoop(Self::rain_loop_sound(
                    self.rain_level,
                )));
            let mut started = false;

            if self.rain_level <= 1.5 && (self.rain_trend < 0.0 || self.rain_level <= 0.0) {
                // Not raining (or dry): check whether it should start.
                let chance = self.config.chance_of_rain;
                if chance != 0 && self.roll_dice(chance) {
                    self.rain_factor = self.new_rain_factor();
                    self.rain_trend = 1.0;
                    started = true;
                }
            } else if self.rain_level > 3.0 {
                // A storm: check whether it should ease off.
                let inv = 100 - self.config.chance_of_rain;
                if inv != 0 && self.roll_dice(inv) {
                    self.rain_factor = self.new_rain_factor();
                    self.rain_trend = -1.0;
                }
                if self.rain_level > 4.0 {
                    self.rain_level = 4.0;
                }
            }

            // A rain that has just started keeps its step (from a dry level of zero, the C++ clamp would undo it).
            if self.rain_level <= 0.0 && !started {
                self.rain_level = 0.0;
                self.rain_factor = 0.0;
                self.rain_trend = 1.0;
            }
        }

        // The light follows the rain level (weather.cpp:268-272). The C++ does this only while it is raining and
        // freezes it on the early return above; here it keeps fading to zero (see DIFFERENCES.md).
        let target = self.rain_level.max(0.0);
        let step = BASE_RAIN_LIGHT_RATE * dt;
        if self.rain_light_level < target {
            self.rain_light_level = (self.rain_light_level + step).min(target);
        } else if self.rain_light_level > target {
            self.rain_light_level = (self.rain_light_level - step).max(target);
        }

        // Falling and at or below 1.5 is the state where the C++ found no rain (weather.cpp:218-229): it returns
        // before the lightning and the drops, on every frame of the fall (the restart check fails on each one). The
        // Rust checks less often (D-95), so this holds the state between checks as well: no flash, no streaks.
        let stopped = self.rain_trend < 0.0 && self.rain_level <= 1.5;
        if self.rain_light_level > 1.0 && !stopped {
            self.update_lightning(dt, eye, ground_at);
        } else {
            // Out of the storm light, or once the rain has stopped, the flash and the thunder countdown stop; a flash
            // does not outlive it.
            self.lightening_level = 0.0;
        }

        if stopped {
            self.current_drops = 0;
            return;
        }
        self.update_drops(dt, eye, ground_at);
    }

    /// Lightning and thunder, while the light is past 1 (weather.cpp:280-354).
    fn update_lightning(&mut self, dt: f32, eye: Vec3, _ground_at: &dyn Fn(f32, f32) -> f32) {
        let _ = eye;
        self.lightening_check -= dt;
        if self.lightening_check < 0.0 {
            // Did lightning flash? The chance is the mission's `BaseLighteningChance`, per percent of rain
            // (weather.cpp:285 used the constant, so the key never had an effect).
            let flash_chance =
                (self.config.base_lightening_chance * self.rain_level * 100.0) as i32;
            if self.roll_dice(flash_chance) {
                self.lightening_level = 64.0 + self.random(192) as f32;
                // A crack of thunder is queued unless one is already on its way.
                if self.thunder_time == 0.0 {
                    self.thunder_time = self.random(BASE_THUNDER_RANDOM_START) as f32;
                    self.thunder_sfx = Some(Self::thunder_sound(self.lightening_level));
                }
            }
            self.lightening_check = BASE_LIGHTENING_FLASH_CHECK;
        }

        if self.lightening_level > 0.0 {
            // The flicker (weather.cpp:321-335): a bright flash can rise again before it falls.
            let falloff = BASE_LIGHTENING_FALLOFF;
            let dropoff = if self.lightening_level > 192.0 {
                if self.roll_dice(50) {
                    -(self.random(falloff) as f32)
                } else {
                    self.random((falloff as f32 * 0.33) as i32) as f32
                }
            } else {
                -(self.random(falloff) as f32)
            };
            self.lightening_level = (self.lightening_level + dropoff * dt).clamp(0.0, 255.0);
        }

        if self.thunder_time > 0.0 {
            self.thunder_time -= dt;
        }
        if let Some(sfx) = self.thunder_sfx {
            if self.thunder_time <= 0.0 {
                self.thunder_time = 0.0;
                self.events.push(WeatherEvent::Thunder(sfx));
                self.thunder_sfx = None;
            }
        }
    }

    /// The drop pool (weather.cpp:387-435): drops enter service as the rain level passes 1, fall at
    /// `BASE_RAIN_VEL * rain_level`, and respawn when they reach the ground under the eye.
    fn update_drops(&mut self, dt: f32, eye: Vec3, ground_at: &dyn Fn(f32, f32) -> f32) {
        if self.rain_level <= 0.0 {
            return;
        }
        let start = Vec3::new(eye.x, eye.y, eye.z + BASE_RAIN_HEIGHT);
        let ground = ground_at(start.x, start.y);

        if self.rain_level >= 1.0 {
            // How many drops are in service: a fraction of the pool, as the level passes 1 (weather.cpp:393).
            let total = self.drops.len();
            let wanted = (((self.rain_level - 1.0) * total as f32) as usize).min(total);
            for i in self.current_drops..wanted {
                self.respawn(i, start);
            }
            self.current_drops = wanted;
        }

        for i in 0..self.current_drops {
            self.drops[i].position.z -= BASE_RAIN_VEL * dt * self.rain_level;
            if self.drops[i].position.z <= ground {
                self.respawn(i, start);
            }
        }
    }

    /// The light for this frame: the mission's light, darkened by the rain (weather.cpp:356-385, 437-447).
    /// Above a rain light of 1 the sun and the ambient light are divided by it, the night factor rises with it, and
    /// lightning adds to the ambient light. Terrain has its own darkening, see [`Weather::terrain_light_scale`].
    pub fn apply_light(&self, base: &LightModel) -> LightModel {
        let mut out = base.clone();
        if !self.active || self.rain_light_level <= 1.0 {
            return out;
        }
        let k = 1.0 / self.rain_light_level;
        for c in out.ambient.iter_mut() {
            *c *= k;
        }
        for c in out.sun_color.iter_mut() {
            *c *= k;
        }
        // `eye->nightFactor` rises with the light (weather.cpp:274-278).
        let weather_night = self.rain_light_level / 4.0;
        if weather_night > out.night_factor {
            out.night_factor = weather_night;
        }
        // The flash: `lightening >> 1` as the terrain and shape specular (weather.cpp:370, tgl.cpp:1741).
        let flash = ((self.lightening_level as u8) >> 1) as f32 / 255.0;
        for c in out.ambient.iter_mut() {
            *c = (*c + flash).min(1.0);
        }
        out
    }

    /// The terrain's darkening (`TerrainQuad::rainLightLevel = rainLightLevel * 0.2`, weather.cpp:357), multiplied
    /// into the terrain's lit colour. `1.0` when the rain does not darken.
    pub fn terrain_light_scale(&self) -> f32 {
        if self.active && self.rain_light_level > 1.0 {
            self.rain_light_level * 0.2
        } else {
            1.0
        }
    }

    /// The lightning on the terrain: `(lightening >> 1) / 255`, added after the texture (`TerrainQuad::lighteningLevel`,
    /// weather.cpp:370). Only while the light is past 1, as in the C++ block that sets it.
    pub fn terrain_lightning(&self) -> f32 {
        if self.active && self.rain_light_level > 1.0 {
            ((self.lightening_level as u8) >> 1) as f32 / 255.0
        } else {
            0.0
        }
    }

    /// The rain streaks as one batch of alpha-blended quads, drawn after the scene (`Weather::render`,
    /// weather.cpp:452). `light` is the frame's light from [`Weather::apply_light`]: the streak alpha follows the
    /// ambient light. `None` when no streak is drawn this frame.
    pub fn rain_batch(&self, camera: &GameCamera, light: &LightModel) -> Option<FxBatch> {
        if !self.active || self.rain_level < 1.0 || self.current_drops == 0 {
            return None;
        }
        let ambient = ambient_factor(light);
        let eye = camera.position_world();
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for drop in &self.drops[..self.current_drops] {
            let top = drop.position;
            let bottom = Vec3::new(top.x, top.y, top.z - drop.length);
            let (top_screen, top_in) = camera.project_z(top);
            let (_, bottom_in) = camera.project_z(bottom);
            if !(top_in && bottom_in) {
                continue;
            }
            // `unsigned char amb = ambientFactor * (1.0f - screen1.z)`: the fade with distance, as a linear depth
            // over the camera's range (see DIFFERENCES.md D-64). The C++ `screen.z` is about 1 to 3 for these drops
            // with this camera's near plane, so its `1 - z` is negative and the unsigned conversion of it is undefined.
            let (near, far) = (camera.settings.near_plane, camera.settings.far_plane);
            let distance = 1.0 / top_screen[3];
            let depth = ((distance - near) / (far - near)).clamp(0.0, 1.0);
            let amb = (ambient * (1.0 - depth)).clamp(0.0, 255.0) as u8;
            let alpha = f32::from(amb) / 255.0;

            // A one-pixel line: a quad across the streak, facing the eye, half a pixel either side.
            let dir = bottom - top;
            let view = top - eye;
            let side = dir.cross(view).normalized();
            let half = pixel_half_width(camera, top, side);
            let Some(half) = half else { continue };
            let side = side * half;

            let base = vertices.len() as u32;
            for p in [top + side, top - side, bottom - side, bottom + side] {
                vertices.push(FxVertex {
                    pos: p.to_array(),
                    uv: [0.0, 0.0],
                    color: [1.0, 1.0, 1.0, alpha],
                });
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }

        if vertices.is_empty() {
            return None;
        }
        Some(FxBatch {
            texture: None,
            blend: FxBlend::Alpha,
            z_write: false,
            z_compare: true,
            clamp: false,
            ground_fade: false,
            priority: 0,
            vertices,
            indices,
        })
    }
}

/// `ambientFactor` of `Weather::render` (weather.cpp:459-468): the average ambient in 0..255, lifted by half when
/// dark, and at least 96.
fn ambient_factor(light: &LightModel) -> f32 {
    let avg = light.ambient.iter().sum::<f32>() / 3.0 * 255.0;
    let lifted = if avg < 170.0 { avg * 1.5 } else { 255.0 };
    lifted.max(96.0)
}

/// Half the world width that makes a line `side` wide one pixel on screen, at `point`. `None` when the
/// side vector does not move the point on screen (a degenerate streak).
fn pixel_half_width(camera: &GameCamera, point: Vec3, side: Vec3) -> Option<f32> {
    let (a, _) = camera.project_z(point);
    let (b, _) = camera.project_z(point + side);
    let d = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
    if d < 1.0e-6 {
        None
    } else {
        Some(0.5 / d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(_x: f32, _y: f32) -> f32 {
        0.0
    }

    fn cfg(drops: u32, start: f32, chance: i32) -> WeatherConfig {
        WeatherConfig::new(Some(drops), Some(start), Some(chance), None)
    }

    #[test]
    fn config_clamps_and_defaults_match_the_c_code() {
        let d = WeatherConfig::new(None, None, None, None);
        assert_eq!(d.max_rain_drops, 500);
        assert_eq!(d.starting_rain_level, 0.0);
        assert_eq!(d.chance_of_rain, 0);
        assert_eq!(d.base_lightening_chance, BASE_LIGHTENING_CHANCE);
        let c = WeatherConfig::new(Some(9000), Some(9.0), Some(250), Some(0.5));
        assert_eq!(c.max_rain_drops, 500);
        assert_eq!(c.starting_rain_level, 4.0);
        assert_eq!(c.chance_of_rain, 100);
        assert_eq!(c.base_lightening_chance, 0.1);
        let n = WeatherConfig::new(None, Some(-2.0), Some(-5), Some(-1.0));
        assert_eq!(
            (
                n.starting_rain_level,
                n.chance_of_rain,
                n.base_lightening_chance
            ),
            (0.0, 0, 0.0)
        );
    }

    #[test]
    fn zero_drops_switch_the_weather_off() {
        let w = Weather::new(cfg(0, 2.0, 50), 1);
        assert!(!w.is_active());
    }

    #[test]
    fn rain_loop_and_thunder_sounds_follow_the_c_thresholds() {
        assert_eq!(Weather::rain_loop_sound(0.5), 0);
        assert_eq!(Weather::rain_loop_sound(1.5), RAIN_SOUNDS[3]);
        assert_eq!(Weather::rain_loop_sound(2.2), RAIN_SOUNDS[2]);
        assert_eq!(Weather::rain_loop_sound(2.7), RAIN_SOUNDS[1]);
        assert_eq!(Weather::rain_loop_sound(3.5), RAIN_SOUNDS[0]);
        assert_eq!(Weather::thunder_sound(255.0), THUNDER_SOUNDS[0]);
        assert_eq!(Weather::thunder_sound(200.0), THUNDER_SOUNDS[2]);
        assert_eq!(Weather::thunder_sound(150.0), THUNDER_SOUNDS[1]);
        assert_eq!(Weather::thunder_sound(70.0), THUNDER_SOUNDS[3]);
    }

    #[test]
    fn starting_rain_puts_the_drop_pool_in_service_in_proportion() {
        let mut w = Weather::new(cfg(500, 1.5, 0), 7);
        // The first update is the forced one (rain_update_time starts at zero) and it adds one trend step, so the
        // level is 1.5 + step. The drops in service are (level - 1) of the pool, truncated (weather.cpp:393).
        w.update(0.05, Vec3::new(0.0, 0.0, 100.0), &flat);
        let level = w.rain_level();
        assert!((1.55..=1.65).contains(&level), "{level}");
        assert_eq!(w.current_drops(), ((level - 1.0) * 500.0) as usize);
    }

    #[test]
    fn the_same_seed_gives_the_same_weather() {
        let run = |seed: u32| {
            let mut w = Weather::new(cfg(120, 1.5, 40), seed);
            let mut out = Vec::new();
            for step in 0..4000 {
                w.update(0.1, Vec3::new(step as f32 * 0.3, 0.0, 50.0), &flat);
                out.extend(w.take_events());
            }
            (out, w.rain_level().to_bits(), w.current_drops())
        };
        assert_eq!(run(99), run(99));
        assert_ne!(run(99).0, run(3).0, "a different seed is a different sky");
    }

    #[test]
    fn a_dry_start_with_a_chance_of_rain_begins_to_rain() {
        // Starting dry with ChanceOfRain 100: the first check starts the rain and the next steps raise the level.
        let mut w = Weather::new(cfg(50, 0.0, 100), 2);
        w.update(1.0, Vec3::ZERO, &flat);
        assert_eq!(w.rain_level(), 0.0, "the start itself adds no level yet");
        w.update(30.0, Vec3::ZERO, &flat);
        assert!(w.rain_level() > 0.0, "{}", w.rain_level());
    }

    #[test]
    fn a_falling_storm_has_no_drops_once_it_is_at_one_and_a_half() {
        // A storm of 3.5 with no chance of rain: the first check eases it (trend -1), then it falls a step per 30 s.
        // At or below 1.5 and falling, the failed restart check leaves no drops in service (weather.cpp:218-229), on
        // every frame until the rain stops; the drops must not refill between the checks.
        let mut w = Weather::new(cfg(50, 3.5, 0), 9);
        let mut storm_drops = false;
        let mut crossed = false;
        for _ in 0..1500 {
            w.update(1.0, Vec3::ZERO, &flat);
            if w.rain_trend < 0.0 && w.rain_level() > 1.5 && w.current_drops() > 0 {
                storm_drops = true;
            }
            if w.rain_trend < 0.0 && w.rain_level() <= 1.5 {
                crossed = true;
                assert_eq!(w.current_drops(), 0, "level {}", w.rain_level());
            }
        }
        assert!(storm_drops, "the storm itself has drops in service");
        assert!(crossed, "the storm falls through 1.5");
    }

    #[test]
    fn the_rain_check_runs_every_thirty_seconds_even_when_dry() {
        // Dry, with no chance of rain: the check still runs once per 30 s, so 1 s frames raise one RainLoop
        // event per 30 frames (plus the forced first). The C++ ran it every frame once dry.
        let mut w = Weather::new(cfg(50, 0.0, 0), 5);
        let mut loops = 0;
        for _ in 0..300 {
            w.update(1.0, Vec3::ZERO, &flat);
            loops += w
                .take_events()
                .iter()
                .filter(|e| matches!(e, WeatherEvent::RainLoop(_)))
                .count();
        }
        assert_eq!(
            loops, 10,
            "checks at frames 1, 31, ..., 271 of 300 one-second frames"
        );
        assert_eq!(w.rain_level(), 0.0);
    }

    #[test]
    fn light_darkens_above_one_and_is_untouched_below() {
        let base = LightModel::default();
        let dry = Weather::new(cfg(50, 0.0, 0), 1);
        assert_eq!(dry.apply_light(&base).ambient, base.ambient);
        assert_eq!(dry.terrain_light_scale(), 1.0);

        // Start at a rain light of 2: the light is halved and the terrain takes 0.4 (2 * 0.2).
        let mut w = Weather::new(cfg(50, 2.0, 0), 1);
        w.update(0.0, Vec3::ZERO, &flat);
        assert_eq!(w.rain_light_level(), 2.0);
        let lit = w.apply_light(&base);
        for (a, b) in lit.ambient.iter().zip(base.ambient.iter()) {
            assert!((a - b / 2.0).abs() < 1e-6);
        }
        assert!((w.terrain_light_scale() - 0.4).abs() < 1e-6);
        assert!((lit.night_factor - 0.5).abs() < 1e-6, "rain light / 4");
    }

    #[test]
    fn the_darkening_fades_out_when_the_rain_stops() {
        // The rain stops with the light at 2: the light steps back by 0.01 per second, not all at once, and the
        // terrain's darkening returns to 1 once the light is 1 again (the C++ left both stale).
        let mut w = Weather::new(cfg(50, 2.0, 0), 1);
        // Hold the next check off, and stop the rain level at zero.
        w.rain_update_time = 1.0e6;
        w.rain_level = 0.0;
        w.rain_factor = 0.0;
        w.rain_trend = 1.0;
        w.update(0.0, Vec3::ZERO, &flat);
        assert!(w.terrain_light_scale() < 1.0);
        for _ in 0..10 {
            w.update(1.0, Vec3::ZERO, &flat);
        }
        let light = w.rain_light_level();
        assert!((1.85..1.95).contains(&light), "{light}");
        for _ in 0..200 {
            w.update(1.0, Vec3::ZERO, &flat);
        }
        assert_eq!(w.rain_light_level(), 0.0);
        assert_eq!(w.terrain_light_scale(), 1.0);
    }

    #[test]
    fn a_storm_produces_lightning_and_a_thunder_from_the_table() {
        let mut w = Weather::new(cfg(100, 3.5, 0), 11);
        let mut thunder = Vec::new();
        for _ in 0..4000 {
            w.update(0.5, Vec3::new(0.0, 0.0, 100.0), &flat);
            for e in w.take_events() {
                if let WeatherEvent::Thunder(id) = e {
                    thunder.push(id);
                }
            }
            if thunder.len() >= 2 {
                break;
            }
        }
        assert!(thunder.len() >= 2, "a storm has thunder: {thunder:?}");
        assert!(
            thunder.iter().all(|id| THUNDER_SOUNDS.contains(id)),
            "{thunder:?}"
        );
    }

    #[test]
    fn lightning_flash_darkness_is_removed_when_light_drops() {
        // A flash brightens the lit surfaces; out of the storm light it does not.
        let base = LightModel::default();
        let mut w = Weather::new(cfg(100, 3.0, 0), 4);
        for _ in 0..4000 {
            w.update(0.5, Vec3::ZERO, &flat);
            if w.lightening_level() > 0.0 {
                break;
            }
        }
        assert!(w.lightening_level() > 0.0, "a flash in 2000 s of storm");
        let lit = w.apply_light(&base);
        assert!(lit.ambient[0] > base.ambient[0] / w.rain_light_level());
    }

    #[test]
    fn ambient_factor_lifts_dark_light_and_keeps_a_floor() {
        let dark = LightModel {
            ambient: [0.0, 0.0, 0.0],
            ..LightModel::default()
        };
        assert_eq!(ambient_factor(&dark), 96.0);
        let mid = LightModel {
            ambient: [0.4, 0.4, 0.4],
            ..LightModel::default()
        };
        assert!((ambient_factor(&mid) - 0.4 * 255.0 * 1.5).abs() < 1e-3);
        let bright = LightModel {
            ambient: [0.9, 0.9, 0.9],
            ..LightModel::default()
        };
        assert_eq!(ambient_factor(&bright), 255.0);
    }
}
