// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Night light pools on the ground around buildings.
//!
//! Ported from `BldgAppearance::update` (`bdactor.cpp`: a `TG_LIGHT_TERRAIN` light is created from
//! the appearance's `TerrainLightRGB`/intensity/radii while `nightFactor > 0`, its intensity scaled
//! by `nightFactor`), `TG_Light::SetIntensity`/`GetFalloff` (`tgl.h`) and the terrain vertex loop in
//! `TerrainQuad::setupTextures` (`quad.cpp`: every vertex sums `colour * falloff` of all lights,
//! clamped to 255, as an additive "specular" term).
//!
//! The original evaluates that sum per terrain vertex; here it is baked into a small light map
//! (8 texels per vertex) that the terrain shader adds, which keeps the falloff smooth at any
//! resolution.  Lights also reach shape vertices in the original (`TG_LIGHT_TERRAIN` branch of
//! `MultiTransformShape`, `tgl.cpp:2023`, an additive specular term, only with shadows on); the shape
//! shader samples the same map at the pixel's ground position (`scene.wgsl`, `fs_shape`).

use crate::math::Vec3;
use crate::terrain::TerrainMap;

/// Light map texels per terrain vertex.
pub const TEXELS_PER_VERTEX: usize = 8;

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainLight {
    /// World position (`pointLight->SetPosition(&position)`), z = ground height.
    pub position: Vec3,
    /// `0x00RRGGBB`.
    pub rgb: u32,
    pub intensity: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
}

impl TerrainLight {
    /// `SetIntensity(intensity * nightFactor)` followed by the colour scale (truncating casts).
    pub fn scaled_rgb(&self, night_factor: f32) -> [u32; 3] {
        let mut k = self.intensity * night_factor;
        if k > 1.0 {
            k = 1.0;
        }
        [
            (k * ((self.rgb >> 16) & 0xff) as f32) as u32,
            (k * ((self.rgb >> 8) & 0xff) as f32) as u32,
            (k * (self.rgb & 0xff) as f32) as u32,
        ]
    }

    /// `TG_Light::GetFalloff`: `None` beyond the outer radius.
    pub fn falloff(&self, length: f32) -> Option<f32> {
        if length <= self.inner_radius {
            return Some(1.0);
        }
        if length >= self.outer_radius {
            return None;
        }
        Some((self.outer_radius - length) / (self.outer_radius - self.inner_radius))
    }
}

/// RGBA8 light map covering the whole map, north-west texel first.
#[derive(Clone, Debug, PartialEq)]
pub struct LightMap {
    pub size: usize,
    pub rgba: Vec<u8>,
}

impl LightMap {
    /// A 1x1 black map: no lights.
    pub fn empty() -> Self {
        LightMap {
            size: 1,
            rgba: vec![0, 0, 0, 255],
        }
    }

    pub fn texel(&self, x: usize, y: usize) -> [u8; 3] {
        let p = (y * self.size + x) * 4;
        [self.rgba[p], self.rgba[p + 1], self.rgba[p + 2]]
    }
}

/// Bakes the lights.  Nothing is lit unless `night_factor > 0` (the C++ creates the light only
/// then), so daylight returns the empty map.
pub fn bake(terrain: &TerrainMap, lights: &[TerrainLight], night_factor: f32) -> LightMap {
    if lights.is_empty() || night_factor <= 0.0 {
        return LightMap::empty();
    }
    let size = terrain.side() * TEXELS_PER_VERTEX;
    let world = terrain.world_side();
    let (tx, ty) = terrain.top_left();
    let texel = world / size as f32;
    let mut sums = vec![[0u32; 3]; size * size];
    for light in lights {
        let rgb = light.scaled_rgb(night_factor);
        if rgb == [0, 0, 0] {
            continue;
        }
        let r = light.outer_radius;
        let x0 = (((light.position.x - r - tx) / texel).floor().max(0.0)) as usize;
        let x1 = ((((light.position.x + r - tx) / texel).ceil()) as usize).min(size - 1);
        let y0 = (((ty - light.position.y - r) / texel).floor().max(0.0)) as usize;
        let y1 = ((((ty - light.position.y + r) / texel).ceil()) as usize).min(size - 1);
        for j in y0..=y1 {
            for i in x0..=x1 {
                let x = tx + (i as f32 + 0.5) * texel;
                let y = ty - (j as f32 + 0.5) * texel;
                let p = Vec3::new(x, y, terrain.elevation_at(x, y));
                let Some(f) = light.falloff((p - light.position).length()) else {
                    continue;
                };
                let s = &mut sums[j * size + i];
                for c in 0..3 {
                    s[c] = (s[c] + (rgb[c] as f32 * f) as u32).min(255);
                }
            }
        }
    }
    let mut rgba = Vec::with_capacity(size * size * 4);
    for s in sums {
        rgba.extend_from_slice(&[s[0] as u8, s[1] as u8, s[2] as u8, 255]);
    }
    LightMap { size, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_assets::map::TerrainVertex;

    fn flat(side: usize) -> TerrainMap {
        let v = TerrainVertex {
            normal: [0.0, 0.0, 1.0],
            elevation: 100.0,
            texture_data: 0xffff_0000,
            local_rgb_light: 0,
            terrain_type: 1,
            selected: 0,
            water: 0,
            shadow: 0,
            highlighted: 0,
        };
        TerrainMap::new(side, vec![v; side * side])
    }

    fn light(pos: Vec3) -> TerrainLight {
        TerrainLight {
            position: pos,
            rgb: 0x0078_7832,
            intensity: 0.5,
            inner_radius: 100.0,
            outer_radius: 250.0,
        }
    }

    #[test]
    fn falloff_is_flat_then_linear_then_off() {
        let l = light(Vec3::ZERO);
        assert_eq!(l.falloff(0.0), Some(1.0));
        assert_eq!(l.falloff(100.0), Some(1.0));
        assert_eq!(l.falloff(175.0), Some(0.5));
        assert_eq!(l.falloff(250.0), None);
        assert_eq!(l.falloff(1000.0), None);
    }

    #[test]
    fn colour_is_scaled_by_intensity_and_night_factor_with_truncation() {
        let l = light(Vec3::ZERO);
        assert_eq!(l.scaled_rgb(1.0), [0x78 / 2, 0x78 / 2, 0x32 / 2]);
        // intensity * nightFactor above 1 is clamped like SetIntensity
        let bright = TerrainLight {
            intensity: 4.0,
            ..l.clone()
        };
        assert_eq!(bright.scaled_rgb(1.0), [0x78, 0x78, 0x32]);
        assert_eq!(
            l.scaled_rgb(0.5),
            [0x78 / 4, 0x78 / 4, (0x32 as f32 * 0.25) as u32]
        );
    }

    #[test]
    fn daylight_bakes_nothing() {
        let m = flat(10);
        let lm = bake(&m, &[light(Vec3::new(0.0, 0.0, 100.0))], 0.0);
        assert_eq!(lm, LightMap::empty());
        assert_eq!(bake(&m, &[], 1.0), LightMap::empty());
    }

    #[test]
    fn pool_is_brightest_at_the_light_and_gone_at_the_outer_radius() {
        let m = flat(10);
        let l = light(Vec3::new(0.0, 0.0, 100.0));
        let lm = bake(&m, &[l], 1.0);
        assert_eq!(lm.size, 80);
        // map spans -640..640; the centre texel is next to world (0,0)
        let c = lm.texel(40, 40);
        assert!(c[0] > 50 && c[2] < c[0], "{c:?}");
        // 320 units away (outside 250): dark
        assert_eq!(lm.texel(40 + 20, 40), [0, 0, 0]);
        // 175 units away: half strength
        let half = lm.texel(40 + 10, 40);
        assert!(half[0] > 20 && half[0] < c[0], "{half:?} vs {c:?}");
    }

    #[test]
    fn overlapping_lights_add_and_clamp() {
        let m = flat(10);
        let strong = TerrainLight {
            intensity: 1.0,
            rgb: 0x00ff_ffff,
            ..light(Vec3::new(0.0, 0.0, 100.0))
        };
        let lm = bake(&m, &[strong.clone(), strong], 1.0);
        assert_eq!(lm.texel(40, 40), [255, 255, 255]);
    }

    #[test]
    fn height_difference_counts_in_the_distance() {
        let m = flat(10);
        // a light 300 above the ground reaches nothing on it
        let lm = bake(&m, &[light(Vec3::new(0.0, 0.0, 400.0))], 1.0);
        assert!(lm.rgba.chunks(4).all(|p| p[..3] == [0, 0, 0]));
    }
}
