// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The faces of a live weapon bolt, built the way `WeaponBolt::render` builds them (code/weaponbolt.cpp).
//!
//! * A beam (`isBeam`) is four triangles from the muzzle to the target hot spot, two across the ground plane (offset by `bulgeWidth`
//!   along the horizontal normal of the bolt direction) and two standing up (offset by `bulgeWidth` in z): weaponbolt.cpp:1266-1318
//!   (vertices), 1874-2110 (colours, texture coordinates, `OneInvAlpha` blend, no z write).
//! * A projectile with no gosFX trail (`!gosEffect`) is the diamond of `laserPosition`, the bulge `bulgeLength` behind it and the
//!   back `projLength` behind it: weaponbolt.cpp:1236-1264 (vertices), 1619-1790 (colours).
//!
//! The beam wiggle (`beamWiggle`, weaponbolt.cpp:1302-1308) is a per-frame random offset of the target end, each axis within
//! plus or minus the type's wiggle distance; [`wiggle_offset`] draws it from the caller's visual stream and [`beam_batch`] adds it
//! to the target end.

use mc2_render::fx::{FxBatch, FxBlend, FxTextureId, FxVertex};
use mc2_sim::math::Vec3;
use mc2_sim_data::objtype::WeaponBoltType;

/// Priority of bolt faces among the effect batches (`MC2_DRAWONEIN` effects draw after the terrain; any value works, equal to a
/// plain effect's).
const PRIORITY: u32 = 0;

/// What the type data says about how a bolt looks.
#[derive(Clone, Debug, PartialEq)]
pub enum BoltLook {
    Beam(BeamLook),
    Shot(ShotLook),
}

#[derive(Clone, Debug, PartialEq)]
pub struct BeamLook {
    pub width: f32,
    /// `textureName` unless `NONE`.
    pub texture: Option<String>,
    pub uv_anim_rate: f32,
    pub uv_repeat: f32,
    pub unit_length: f32,
    pub mid_edge_rgb: u32,
    pub middle_rgb: u32,
    pub edge_alpha: u8,
    pub bolt_alpha: u8,
    pub duration: f32,
    /// `beamWiggle`: the half range of the per-frame target jitter on each axis.
    pub wiggle: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShotLook {
    pub length: f32,
    pub bulge_length: f32,
    pub bulge_width: f32,
    pub front_rgb: u32,
    pub mid_edge_rgb: u32,
    pub middle_rgb: u32,
    pub back_rgb: u32,
    pub edge_alpha: u8,
    pub bolt_alpha: u8,
}

impl BoltLook {
    /// The look of a bolt type.
    pub fn of_type(t: &WeaponBoltType) -> BoltLook {
        match t {
            WeaponBoltType::Beam(b) if b.is_beam => BoltLook::Beam(BeamLook {
                width: b.bulge_width,
                texture: (!b.texture_name.eq_ignore_ascii_case("NONE"))
                    .then(|| b.texture_name.clone()),
                uv_anim_rate: b.uv_anim_rate,
                uv_repeat: b.uv_repeat,
                unit_length: b.unit_length,
                mid_edge_rgb: b.mid_edge_rgb,
                middle_rgb: b.middle_rgb,
                edge_alpha: b.edge_alpha,
                bolt_alpha: b.bolt_alpha,
                duration: b.beam_duration,
                wiggle: b.beam_wiggle,
            }),
            WeaponBoltType::Beam(b) => BoltLook::Shot(ShotLook {
                length: b.proj_length,
                bulge_length: 0.0,
                bulge_width: b.bulge_width,
                front_rgb: b.mid_edge_rgb,
                mid_edge_rgb: b.mid_edge_rgb,
                middle_rgb: b.middle_rgb,
                back_rgb: b.mid_edge_rgb,
                edge_alpha: b.edge_alpha,
                bolt_alpha: b.bolt_alpha,
            }),
            WeaponBoltType::Bolt(b) if b.is_beam => BoltLook::Beam(BeamLook {
                width: b.bulge_width,
                texture: None,
                uv_anim_rate: 0.0,
                uv_repeat: 1.0,
                unit_length: 1.0,
                mid_edge_rgb: b.mid_edge_rgb,
                middle_rgb: b.middle_rgb,
                edge_alpha: b.edge_alpha,
                bolt_alpha: b.bolt_alpha,
                duration: b.beam_duration,
                wiggle: b.beam_wiggle,
            }),
            WeaponBoltType::Bolt(b) => BoltLook::Shot(ShotLook {
                length: b.projectile_length,
                bulge_length: b.bulge_length,
                bulge_width: b.bulge_width,
                front_rgb: b.front_rgb,
                mid_edge_rgb: b.mid_edge_rgb,
                middle_rgb: b.middle_rgb,
                back_rgb: b.back_rgb,
                edge_alpha: b.edge_alpha,
                bolt_alpha: b.bolt_alpha,
            }),
        }
    }
}

/// `alpha << 24 | rgb` as straight RGBA floats.
fn argb(alpha: u8, rgb: u32) -> [f32; 4] {
    [
        ((rgb >> 16) & 0xff) as f32 / 255.0,
        ((rgb >> 8) & 0xff) as f32 / 255.0,
        (rgb & 0xff) as f32 / 255.0,
        f32::from(alpha) / 255.0,
    ]
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    Vec3 {
        x: a.x - b.x,
        y: a.y - b.y,
        z: a.z - b.z,
    }
}

fn add_scaled(a: Vec3, d: Vec3, s: f32) -> Vec3 {
    Vec3 {
        x: a.x + d.x * s,
        y: a.y + d.y * s,
        z: a.z + d.z * s,
    }
}

fn unit(v: Vec3) -> Option<Vec3> {
    let l = (v.x * v.x + v.y * v.y + v.z * v.z).sqrt();
    (l > 1.0e-4).then(|| Vec3 {
        x: v.x / l,
        y: v.y / l,
        z: v.z / l,
    })
}

fn vert(p: Vec3, u: f32, v: f32, color: [f32; 4]) -> FxVertex {
    FxVertex {
        pos: [p.x, p.y, p.z],
        uv: [u, v],
        color,
    }
}

fn batch(texture: Option<FxTextureId>, vertices: Vec<FxVertex>) -> FxBatch {
    let indices = (0..vertices.len() as u32).collect();
    FxBatch {
        texture,
        blend: FxBlend::Premultiplied,
        z_write: false,
        z_compare: true,
        clamp: false,
        ground_fade: false,
        priority: PRIORITY,
        vertices,
        indices,
    }
}

/// `Vector3D offset(w,w,w); offset.x = RandomNumber(offset.x * 2) - offset.x; ...` (weaponbolt.cpp:1302-1306): one offset per
/// frame, three draws from the visual stream (`unit` yields fractions in `[0, 1)`).
pub fn wiggle_offset(look: &BeamLook, mut unit: impl FnMut() -> f32) -> Vec3 {
    let w = look.wiggle;
    if w <= 0.0 {
        return Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
    }
    let mut axis = || unit() * w * 2.0 - w;
    Vec3 {
        x: axis(),
        y: axis(),
        z: axis(),
    }
}

/// The four faces of a beam from `from` (the muzzle) to `to` (the target hot spot) displaced by `wiggle` (see [`wiggle_offset`]),
/// `elapsed` seconds into its duration.
pub fn beam_batch(
    look: &BeamLook,
    from: Vec3,
    to: Vec3,
    wiggle: Vec3,
    elapsed: f32,
    texture: Option<FxTextureId>,
) -> Option<FxBatch> {
    // `laserDirection` is fixed before the wiggle moves the hot spot, so the faces keep their orientation
    let dir = unit(sub(from, to))?;
    let to = Vec3 {
        x: to.x + wiggle.x,
        y: to.y + wiggle.y,
        z: to.z + wiggle.z,
    };
    let w = look.width;
    let side = |p: Vec3, sign: f32| Vec3 {
        x: p.x + sign * dir.y * w,
        y: p.y - sign * dir.x * w,
        z: p.z,
    };
    let up = |p: Vec3, sign: f32| Vec3 {
        x: p.x,
        y: p.y,
        z: p.z + sign * w,
    };
    // laserVertices[0..3] and laserSide[0..3] (weaponbolt.cpp:1285-1318)
    let (v0, v1, v2, v3) = (
        side(from, -1.0),
        side(from, 1.0),
        side(to, -1.0),
        side(to, 1.0),
    );
    let (s0, s1, s2, s3) = (up(from, 1.0), up(from, -1.0), up(to, 1.0), up(to, -1.0));
    let dist = {
        let l = sub(to, from);
        (l.x * l.x + l.y * l.y + l.z * l.z).sqrt()
    };
    let repeat = look.uv_repeat * dist / look.unit_length.max(1.0e-4);
    let start = look.uv_anim_rate * elapsed.max(0.0);
    let end = start + repeat;
    let edge_mid = argb(look.edge_alpha, look.mid_edge_rgb);
    let edge_mid_end = argb(look.edge_alpha, look.middle_rgb);
    let bolt_mid = argb(look.bolt_alpha, look.mid_edge_rgb);
    let bolt_end = argb(look.bolt_alpha, look.middle_rgb);
    let vt = 0.999_999;
    // lq1..lq4 (weaponbolt.cpp:1913-2000)
    let vertices = vec![
        vert(v0, start, 0.0, edge_mid),
        vert(v2, end, 0.0, edge_mid_end),
        vert(v3, end, vt, bolt_end),
        vert(v0, start, 0.0, edge_mid),
        vert(v1, start, vt, bolt_mid),
        vert(v3, end, vt, bolt_end),
        vert(s0, start, 0.0, edge_mid),
        vert(s2, end, 0.0, edge_mid_end),
        vert(s3, end, vt, bolt_end),
        vert(s0, start, 0.0, edge_mid),
        vert(s1, start, vt, bolt_mid),
        vert(s3, end, vt, bolt_end),
    ];
    Some(batch(texture, vertices))
}

/// The diamond of a projectile whose type has no gosFX trail, flying from `front` toward `target`.
pub fn shot_batch(look: &ShotLook, front: Vec3, target: Vec3) -> Option<FxBatch> {
    let dir = unit(sub(front, target))?;
    let mid = add_scaled(front, dir, look.bulge_length);
    let back = add_scaled(front, dir, look.length);
    let w = look.bulge_width;
    let v0 = Vec3 {
        x: mid.x - dir.y * w,
        y: mid.y + dir.x * w,
        z: mid.z,
    };
    let v1 = Vec3 {
        x: mid.x + dir.y * w,
        y: mid.y - dir.x * w,
        z: mid.z,
    };
    let edge = |rgb| argb(look.edge_alpha, rgb);
    let c_front = edge(look.front_rgb);
    let c_side = edge(look.mid_edge_rgb);
    let c_mid = argb(look.bolt_alpha, look.middle_rgb);
    let c_back = edge(look.back_rgb);
    let vertices = vec![
        vert(front, 0.0, 0.0, c_front),
        vert(v0, 0.0, 0.0, c_side),
        vert(mid, 0.0, 0.0, c_mid),
        vert(front, 0.0, 0.0, c_front),
        vert(v1, 0.0, 0.0, c_side),
        vert(mid, 0.0, 0.0, c_mid),
        vert(v0, 0.0, 0.0, c_side),
        vert(back, 0.0, 0.0, c_back),
        vert(mid, 0.0, 0.0, c_mid),
        vert(v1, 0.0, 0.0, c_side),
        vert(back, 0.0, 0.0, c_back),
        vert(mid, 0.0, 0.0, c_mid),
    ];
    Some(batch(None, vertices))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    fn look() -> BeamLook {
        BeamLook {
            width: 2.0,
            texture: None,
            uv_anim_rate: 1.0,
            uv_repeat: 2.0,
            unit_length: 10.0,
            mid_edge_rgb: 0x00ff_0000,
            middle_rgb: 0x0000_ff00,
            edge_alpha: 0x80,
            bolt_alpha: 0xff,
            duration: 0.5,
            wiggle: 3.0,
        }
    }

    #[test]
    fn beam_spans_muzzle_to_target_with_data_colour_and_uv() {
        let b = beam_batch(
            &look(),
            v(0.0, 0.0, 5.0),
            v(100.0, 0.0, 5.0),
            v(0.0, 0.0, 0.0),
            0.25,
            None,
        )
        .unwrap();
        assert_eq!(b.vertices.len(), 12);
        let xs: Vec<f32> = b.vertices.iter().map(|p| p.pos[0]).collect();
        assert!(xs.contains(&0.0) && xs.contains(&100.0));
        // ground faces are offset by the width along the horizontal normal
        assert!(b
            .vertices
            .iter()
            .any(|p| p.pos[1] == 2.0 || p.pos[1] == -2.0));
        // uv repeats `uvRepeat * dist / unitLength` = 20 along the beam, scrolled by uvAnimRate * elapsed
        assert_eq!(b.vertices[0].uv[0], 0.25);
        assert_eq!(b.vertices[1].uv[0], 20.25);
        // colour: edge alpha 0x80 on the edge vertices, MidEdgeRGB at the muzzle end
        assert_eq!(b.vertices[0].color, [1.0, 0.0, 0.0, 128.0 / 255.0]);
        assert_eq!(b.vertices[2].color, [0.0, 1.0, 0.0, 1.0]);
        assert_eq!(b.blend, FxBlend::Premultiplied);
        assert!(!b.z_write && b.z_compare);
    }

    #[test]
    fn wiggle_moves_only_the_target_end_within_the_type_range() {
        let mut seq = [0.0f32, 0.5, 0.999].into_iter();
        let o = wiggle_offset(&look(), || seq.next().unwrap());
        assert_eq!((o.x, o.y), (-3.0, 0.0));
        assert!((o.z - 2.994).abs() < 1.0e-4);
        let calm = BeamLook {
            wiggle: 0.0,
            ..look()
        };
        let z = wiggle_offset(&calm, || 0.9);
        assert_eq!((z.x, z.y, z.z), (0.0, 0.0, 0.0));
        let flat = beam_batch(
            &look(),
            v(0.0, 0.0, 5.0),
            v(100.0, 0.0, 5.0),
            v(0.0, 0.0, 0.0),
            0.0,
            None,
        )
        .unwrap();
        let moved =
            beam_batch(&look(), v(0.0, 0.0, 5.0), v(100.0, 0.0, 5.0), o, 0.0, None).unwrap();
        // muzzle-end vertices stay, target-end vertices move by the offset
        assert_eq!(flat.vertices[0].pos, moved.vertices[0].pos);
        assert_ne!(flat.vertices[1].pos, moved.vertices[1].pos);
        assert!((moved.vertices[2].pos[2] - flat.vertices[2].pos[2] - o.z).abs() < 1.0e-4);
    }

    #[test]
    fn beam_geometry_varies_from_frame_to_frame_within_the_wiggle_range() {
        let mut rng = mc2_fx::rng::VisualRng::new(1);
        let frames: Vec<Vec<[f32; 3]>> = (0..8)
            .map(|_| {
                let o = wiggle_offset(&look(), || rng.fraction());
                assert!(o.x.abs() <= 3.0 && o.y.abs() <= 3.0 && o.z.abs() <= 3.0);
                beam_batch(&look(), v(0.0, 0.0, 5.0), v(100.0, 0.0, 5.0), o, 0.0, None)
                    .unwrap()
                    .vertices
                    .iter()
                    .map(|p| p.pos)
                    .collect()
            })
            .collect();
        for pair in frames.windows(2) {
            assert_ne!(pair[0], pair[1]);
        }
    }

    #[test]
    fn degenerate_beam_has_no_faces() {
        assert!(beam_batch(
            &look(),
            v(1.0, 1.0, 1.0),
            v(1.0, 1.0, 1.0),
            v(0.0, 0.0, 0.0),
            0.0,
            None
        )
        .is_none());
    }

    #[test]
    fn shot_diamond_trails_the_front_toward_the_source() {
        let look = ShotLook {
            length: 12.0,
            bulge_length: 4.0,
            bulge_width: 1.0,
            front_rgb: 0xffffff,
            mid_edge_rgb: 0xff0000,
            middle_rgb: 0x00ff00,
            back_rgb: 0x0000ff,
            edge_alpha: 0xff,
            bolt_alpha: 0xff,
        };
        let b = shot_batch(&look, v(50.0, 0.0, 3.0), v(100.0, 0.0, 3.0)).unwrap();
        let xs: Vec<f32> = b.vertices.iter().map(|p| p.pos[0]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 50.0);
        assert!(xs.contains(&46.0) && xs.contains(&38.0));
    }
}
