//! Visual upgrades (P5) that never change gameplay: the CPU side of the "enhanced" lighting and
//! water paths.  Both are off by default and then the renderer's output is byte-identical to the
//! output before they existed (the switches are shader override constants, so the off pipelines
//! contain none of the new code).
//!
//! * Enhanced lighting: plain terrain carries its shading burned into the colour map, computed
//!   from the per-vertex normals.  The enhanced path adds the sun/ambient ramp evaluated per
//!   pixel with a smooth surface normal (a normal map built here from a bicubic height field)
//!   as a correction over the vertex-normal ramp; road/pavement tiles use the per-pixel normal
//!   directly; shapes get a soft terminator.  Only the existing sun and ambient values are used.
//! * Enhanced water: continuous time in f64 (reduced before it reaches the f32 shader), the
//!   `waterNNNN` detail frames cross-faded instead of stepped, and a sun glint from a surface
//!   normal derived from the blended detail frames.
//!
//! Original to this project (platform-level code).  Nothing here is read by the simulation, uses
//! random numbers, or influences draw order, visibility or LOD.

use crate::math::Vec3;
use crate::terrain::{TerrainMap, WORLD_UNITS_PER_VERTEX};

/// Which visual upgrades are on.  `Default` is everything off (today's output).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct VisualOptions {
    pub enhanced_lighting: bool,
    pub enhanced_water: bool,
}

/// Strength of the sun glint on enhanced water (0 disables it).
pub const GLINT_STRENGTH: f32 = 0.9;

/// Texels of the terrain normal map per vertex spacing, per axis.
pub const NORMAL_MAP_SCALE: usize = 4;

/// RGBA8 terrain normal map covering the whole map (`map_uv` space), `normal * 0.5 + 0.5`.
pub struct NormalMap {
    pub size: usize,
    pub rgba: Vec<u8>,
}

fn catmull_rom(p: [f32; 4], t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * (2.0 * p[1]
        + (-p[0] + p[2]) * t
        + (2.0 * p[0] - 5.0 * p[1] + 4.0 * p[2] - p[3]) * t2
        + (-p[0] + 3.0 * p[1] - 3.0 * p[2] + p[3]) * t3)
}

/// Bicubic (Catmull-Rom) height at fractional grid position `(gx, gy)` (east, south), edges clamped.
pub fn smooth_height(map: &TerrainMap, gx: f32, gy: f32) -> f32 {
    let side = map.side() as i64;
    let (ix, iy) = (gx.floor(), gy.floor());
    let (fx, fy) = (gx - ix, gy - iy);
    let (ix, iy) = (ix as i64, iy as i64);
    let h = |c: i64, r: i64| {
        let (c, r) = (c.clamp(0, side - 1) as usize, r.clamp(0, side - 1) as usize);
        map.vertex(c, r).elevation
    };
    let mut col = [0.0f32; 4];
    for (k, v) in col.iter_mut().enumerate() {
        let r = iy - 1 + k as i64;
        *v = catmull_rom([h(ix - 1, r), h(ix, r), h(ix + 1, r), h(ix + 2, r)], fx);
    }
    catmull_rom(col, fy)
}

/// Smooth world-space surface normal (z up) at fractional grid position `(gx, gy)`.
pub fn smooth_normal(map: &TerrainMap, gx: f32, gy: f32) -> Vec3 {
    let e = 0.25;
    let dx = smooth_height(map, gx + e, gy) - smooth_height(map, gx - e, gy);
    let dy = smooth_height(map, gx, gy + e) - smooth_height(map, gx, gy - e);
    let per_unit = 1.0 / (2.0 * e * WORLD_UNITS_PER_VERTEX);
    // world x grows with gx, world y shrinks with gy
    Vec3::new(-dx * per_unit, dy * per_unit, 1.0).normalized()
}

/// Builds the normal map: texel `(i, j)` is the smooth normal under that texel's centre in the
/// shader's `map_uv` space (so a bilinear sample at a pixel's map uv is the pixel's normal).
pub fn build_normal_map(map: &TerrainMap) -> NormalMap {
    let side = map.side();
    let size = side * NORMAL_MAP_SCALE;
    // `map_uv` covers `side` vertex spacings and the vertex grid starts `off` spacings in
    // (0 for even sides, one half for odd ones; `TerrainMap::vertex_position`).
    let off = side as f32 / 2.0 - (side >> 1) as f32;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for j in 0..size {
        for i in 0..size {
            let gx = (i as f32 + 0.5) / NORMAL_MAP_SCALE as f32 - off;
            let gy = (j as f32 + 0.5) / NORMAL_MAP_SCALE as f32 - off;
            let n = smooth_normal(map, gx, gy);
            for c in [n.x, n.y, n.z] {
                rgba.push(((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
            }
            rgba.push(255);
        }
    }
    NormalMap { size, rgba }
}

/// Two adjacent water detail frames and the cross-fade between them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterBlend {
    pub from: usize,
    pub to: usize,
    /// 0 = all `from`, towards 1 = all `to`.
    pub t: f32,
}

fn ping_pong(step: usize, frames: usize) -> usize {
    let period = 2 * (frames - 1);
    let s = step % period;
    if s < frames {
        s
    } else {
        period - s
    }
}

/// The water detail frames at `time`: the same ping-pong order and rate as
/// [`crate::scene::detail_frame`] (equal to it, with `t == 0`, at every frame boundary), with the
/// fraction to the next frame.  Time is carried in f64.
pub fn water_blend(time: f64, rate: f32, frames: usize) -> WaterBlend {
    if frames <= 1 {
        return WaterBlend {
            from: 0,
            to: 0,
            t: 0.0,
        };
    }
    let s = (time * rate as f64).max(0.0);
    let step = s.floor();
    let t = (s - step) as f32;
    let period = 2 * (frames - 1);
    let step = (step % period as f64) as usize;
    WaterBlend {
        from: ping_pong(step, frames),
        to: ping_pong(step + 1, frames),
        t,
    }
}

/// Phase of the base water texture's circular drift (`MapData::update` cloud motion): the angle in
/// radians, from a cycle count reduced in f64 so it does not lose precision in long sessions.
pub fn cloud_angle(time: f64) -> f32 {
    ((time * 0.064).fract() * std::f64::consts::TAU) as f32
}

/// Detail layer drift in texture units, reduced to `0..1` (the texture wraps) in f64.
pub fn spray_offset(time: f64) -> f32 {
    (time * 0.02).fract() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::detail_frame;
    use crate::terrain::TerrainMap;
    use mc2_assets::map::TerrainVertex;

    fn flat_map(side: usize, f: impl Fn(usize, usize) -> f32) -> TerrainMap {
        let mut v = Vec::new();
        for r in 0..side {
            for c in 0..side {
                v.push(TerrainVertex {
                    normal: [0.0, 0.0, 1.0],
                    elevation: f(c, r),
                    texture_data: 0,
                    local_rgb_light: 0,
                    terrain_type: 0,
                    selected: 0,
                    water: 0,
                    shadow: 0,
                    highlighted: 0,
                });
            }
        }
        TerrainMap::new(side, v)
    }

    #[test]
    fn default_options_are_off() {
        let o = VisualOptions::default();
        assert!(!o.enhanced_lighting && !o.enhanced_water);
    }

    #[test]
    fn flat_ground_has_an_up_normal_and_a_slope_leans_downhill() {
        let flat = flat_map(8, |_, _| 100.0);
        let n = smooth_normal(&flat, 3.3, 4.1);
        assert!((n.z - 1.0).abs() < 1e-6 && n.x.abs() < 1e-6 && n.y.abs() < 1e-6);
        // rising to the east by 64 units per vertex: the normal leans west (-x)
        let ramp = flat_map(8, |c, _| 64.0 * c as f32);
        let n = smooth_normal(&ramp, 3.5, 3.5);
        let expect = Vec3::new(-0.5, 0.0, 1.0).normalized();
        assert!((n.x - expect.x).abs() < 1e-4 && (n.z - expect.z).abs() < 1e-4);
        // rising to the south (row grows southwards = world y falls): the normal leans north
        let ramp = flat_map(8, |_, r| 64.0 * r as f32);
        let n = smooth_normal(&ramp, 3.5, 3.5);
        assert!(n.y > 0.4 && n.x.abs() < 1e-5, "{n:?}");
    }

    #[test]
    fn normal_map_encodes_unit_normals_and_is_opaque() {
        let m = flat_map(6, |c, r| ((c * 7 + r * 3) % 5) as f32 * 20.0);
        let nm = build_normal_map(&m);
        assert_eq!(nm.size, 6 * NORMAL_MAP_SCALE);
        assert_eq!(nm.rgba.len(), nm.size * nm.size * 4);
        for p in nm.rgba.chunks_exact(4) {
            assert_eq!(p[3], 255);
            let v = |b: u8| b as f32 / 255.0 * 2.0 - 1.0;
            let len = (v(p[0]).powi(2) + v(p[1]).powi(2) + v(p[2]).powi(2)).sqrt();
            assert!((len - 1.0).abs() < 0.02, "{len}");
            assert!(v(p[2]) > 0.0);
        }
    }

    #[test]
    fn bicubic_height_passes_through_the_vertices() {
        let m = flat_map(6, |c, r| ((c * 5 + r * 11) % 7) as f32 * 13.0);
        for (c, r) in [(1usize, 1usize), (2, 3), (4, 2)] {
            let h = smooth_height(&m, c as f32, r as f32);
            assert!((h - m.vertex(c, r).elevation).abs() < 1e-3);
        }
    }

    #[test]
    fn water_blend_lands_on_the_original_frames_at_boundaries() {
        for frames in [1usize, 2, 5, 9] {
            for step in 0..40 {
                let time = step as f64 / 8.0;
                let b = water_blend(time, 8.0, frames);
                assert_eq!(b.from, detail_frame(time as f32 + 0.001, 8.0, frames));
                assert_eq!(b.t, 0.0);
            }
        }
    }

    #[test]
    fn water_blend_fades_between_neighbours() {
        let b = water_blend(3.0 / 8.0 + 0.5 / 8.0, 8.0, 4);
        assert_eq!((b.from, b.to), (3, 2)); // ping-pong turns at the last frame
        assert!((b.t - 0.5).abs() < 1e-4);
        let b = water_blend(0.5 / 8.0, 8.0, 4);
        assert_eq!((b.from, b.to), (0, 1));
        // every adjacent pair, never a jump
        for k in 0..200 {
            let b = water_blend(k as f64 * 0.037, 8.0, 6);
            assert!(b.from.abs_diff(b.to) == 1 && (0.0..1.0).contains(&b.t));
        }
    }

    #[test]
    fn long_sessions_keep_phase_precision() {
        // a day of play: the reduced phases still move smoothly in f64
        let t = 86_400.0;
        let a = cloud_angle(t);
        let b = cloud_angle(t + 0.01);
        let d = (b - a).rem_euclid(std::f32::consts::TAU);
        let want = (0.01 * 0.064 * std::f64::consts::TAU) as f32;
        assert!((d - want).abs() < 1e-4, "{d} vs {want}");
        assert!((spray_offset(t + 0.5) - spray_offset(t) - 0.01).abs() < 1e-4);
        assert!((0.0..1.0).contains(&spray_offset(1.0e7)));
    }
}
