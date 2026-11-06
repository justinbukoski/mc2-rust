// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Tube` (`mclib/gosfx/tube.cpp`): a ribbon or extruded cross-section trailing behind the
//! effect. The tube keeps a ring of profiles (cross-sections); every `1 / profilesPerSecond`
//! the head profile is frozen where the effect was and a new one starts, and each profile
//! drifts, scales, fades and scrolls its texture over its own life.

use crate::draw::{DrawInfo, DrawList, DrawVertex};
use crate::effect::{Ctx, Data, Effect, ExecuteInfo};
use crate::fcurve::FCurve;
use crate::math::{Mat, Obb, Rgba, Vec3, SMALL, Y_AXIS, Z_AXIS};
use crate::spec::{ProfileType, SpecKind, TubeSpec};

#[derive(Clone, Debug)]
pub(crate) struct Profile {
    pub age: f32,
    pub age_rate: f32,
    pub seed: f32,
    pub profile_to_world: Mat,
    pub direction: Vec3,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            age: 0.0,
            age_rate: 0.0,
            seed: 0.0,
            profile_to_world: Mat::IDENTITY,
            direction: Vec3::ZERO,
        }
    }
}

pub(crate) struct TubeState {
    pub profiles: Vec<Profile>,
    /// `maxProfileCount * vertexCount` positions in the tube's own space, newest profile first.
    pub vertices: Vec<Vec3>,
    pub colors: Vec<Rgba>,
    pub uvs: Vec<[f32; 2]>,
    /// `BuildMesh`: two triangles per panel per profile pair.
    pub indices: Vec<u32>,
    pub active: usize,
    pub head: i32,
    pub tail: i32,
    pub birth_accumulator: f32,
}

impl TubeState {
    pub(crate) fn new(spec: &TubeSpec) -> TubeState {
        let max = spec.max_profile_count.max(0) as usize;
        let vc = spec.vertices.len();
        TubeState {
            profiles: vec![Profile::default(); max],
            vertices: vec![Vec3::ZERO; max * vc],
            colors: vec![Rgba::default(); max * vc],
            uvs: vec![[0.0; 2]; max * vc],
            indices: build_mesh(spec),
            active: 0,
            head: -1,
            tail: -1,
            birth_accumulator: 0.0,
        }
    }
}

/// `Tube::BuildMesh`.
fn build_mesh(spec: &TubeSpec) -> Vec<u32> {
    let vc = spec.vertices.len() as u32;
    let max = spec.max_profile_count.max(0) as u32;
    let mut idx = Vec::new();
    if vc < 2 {
        return idx;
    }
    if spec.profile_type != ProfileType::Cross {
        for profile in 0..max.saturating_sub(1) {
            let base = profile * vc;
            for panel in 0..vc - 1 {
                if spec.inside_out {
                    idx.extend_from_slice(&[
                        base + panel + 1,
                        base + panel,
                        base + panel + vc + 1,
                        base + panel + vc,
                        base + panel + vc + 1,
                        base + panel,
                    ]);
                } else {
                    idx.extend_from_slice(&[
                        base + panel,
                        base + panel + 1,
                        base + panel + vc + 1,
                        base + panel + vc + 1,
                        base + panel + vc,
                        base + panel,
                    ]);
                }
            }
        }
    } else {
        for profile in 0..max.saturating_sub(1) {
            let base = profile * vc;
            for panel in 0..4u32 {
                if spec.inside_out {
                    idx.extend_from_slice(&[
                        base + panel + 1,
                        base,
                        base + panel + 6,
                        base + 5,
                        base + panel + 6,
                        base,
                    ]);
                } else {
                    idx.extend_from_slice(&[
                        base,
                        base + panel + 1,
                        base + panel + 6,
                        base + panel + 6,
                        base + 5,
                        base,
                    ]);
                }
            }
        }
    }
    idx
}

/// `Tube::CreateNewProfile`.
fn create_profile(
    ts: &TubeSpec,
    spec: &crate::spec::EffectSpec,
    b: &crate::effect::Base,
    t: &mut TubeState,
    index: usize,
    origin: &Mat,
    ctx: &mut Ctx,
) {
    let min_seed = spec.base.min_child_seed.compute(b.age, b.seed);
    let seed_range = spec.base.max_child_seed.compute(b.age, b.seed) - min_seed;
    let seed = (ctx.rng.fraction() * seed_range + min_seed).clamp(0.0, 1.0);
    let mut lifetime = ts.p_life_span.compute(b.age, seed);
    if lifetime < 0.033_333_3 {
        lifetime = 0.033_333_3;
    }
    let pitch_min = ts.minimum_deviation.compute(b.age, seed);
    let mut pitch_range = ts.maximum_deviation.compute(b.age, seed) - pitch_min;
    if pitch_range < 0.0 {
        pitch_range = 0.0;
    }
    let angle = pitch_min + ctx.rng.fraction() * pitch_range;
    let (sine, cosine) = angle.sin_cos();
    let x = if ctx.rng.fraction() >= 0.5 {
        sine
    } else {
        -sine
    };
    let p = &mut t.profiles[index];
    p.age = 0.0;
    p.seed = seed;
    p.age_rate = 1.0 / lifetime;
    p.profile_to_world = *origin;
    p.direction = Vec3::new(x, 0.0, cosine);
}

/// `Tube::AnimateProfile`: returns the profile's bounding sphere (centre, radius) or `None`
/// once it is too old.
fn animate_profile(
    ts: &TubeSpec,
    t: &mut TubeState,
    index: usize,
    profile_index: usize,
    world_to_new_local: &Mat,
) -> Option<(Vec3, f32)> {
    let p = &t.profiles[index];
    let age = p.age;
    if age >= 1.0 {
        return None;
    }
    let seed = p.seed;
    let scale = ts.p_scale.compute(age, seed);
    let disp = ts.p_displacement.compute(age, seed);
    let offset = p.direction * disp;

    let mut template_to_profile = Mat::IDENTITY;
    template_to_profile.m[0][0] = scale;
    template_to_profile.m[1][1] = scale;
    template_to_profile.m[2][2] = scale;
    template_to_profile.build_translation(offset);
    let template_to_world = Mat::multiply(&template_to_profile, &p.profile_to_world);
    let template_to_new_local = Mat::multiply(&template_to_world, world_to_new_local);
    let center = template_to_new_local.translation();

    let vc = ts.vertices.len();
    let vertex_index = profile_index * vc;
    if ts.profile_type != ProfileType::AlignedRibbon {
        for i in 0..vc {
            t.vertices[vertex_index + i] = template_to_new_local.point(ts.vertices[i]);
        }
    }

    // UV adjustments.
    let mut u = ts.p_u_offset.compute(age, seed);
    let v = ts.p_v_offset.compute(age, seed);
    let u2 = ts.p_u_size.compute(age, seed);
    let v2 = ts.p_v_size.compute(age, seed);
    u += u2 * profile_index as f32 + ts.u_bias;
    for i in 0..vc {
        t.uvs[vertex_index + i] = [u, ts.uvs[i][1] * v2 + v];
    }

    let color = Rgba::new(
        ts.p_red.compute(age, seed),
        ts.p_green.compute(age, seed),
        ts.p_blue.compute(age, seed),
        ts.p_alpha.compute(age, seed),
    );
    for i in 0..vc {
        t.colors[vertex_index + i] = color;
    }
    Some((center, scale))
}

impl Effect {
    pub fn tube_active(&self) -> usize {
        match &self.data {
            Data::Tube(t) => t.active,
            _ => 0,
        }
    }

    /// `Tube::Start`.
    pub(crate) fn tube_start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        self.effect_start(ctx, info);
        if let Data::Tube(t) = &mut self.data {
            t.birth_accumulator = 1.0;
        }
    }

    /// `Tube::Kill`.
    pub(crate) fn tube_kill(&mut self) {
        if let (SpecKind::Tube(ts), Data::Tube(t)) = (&self.spec.kind, &mut self.data) {
            // The C++ marks every profile of the ring dead before resetting.
            for p in t.profiles.iter_mut() {
                p.age = 1.0;
            }
            let _ = ts;
            t.active = 0;
            t.head = -1;
            t.tail = -1;
            t.birth_accumulator = 0.0;
        }
        self.effect_kill();
    }

    /// `Tube::Execute`.
    pub(crate) fn tube_execute(
        &mut self,
        ctx: &mut Ctx,
        info: &ExecuteInfo,
        bounds: &mut Obb,
    ) -> bool {
        if !self.is_executed() {
            return false;
        }
        let mut tube_box: Option<(Vec3, Vec3)> = None;
        let had_profiles;
        {
            let Effect { spec, b, data } = self;
            let (SpecKind::Tube(ts), Data::Tube(t)) = (&spec.kind, data) else {
                return false;
            };

            // Head of the tube at the current location; the inverse keeps old profiles in place.
            let local_to_world = Mat::multiply(&b.local_to_parent, &info.parent_to_world);
            let new_world_to_local = local_to_world.invert();
            let max_count = ts.max_profile_count;

            let d_t = (info.time - b.last_ran) as f32;
            let prev_age = b.age;
            b.age += d_t * b.age_rate;
            if b.age >= 1.0 {
                t.birth_accumulator = 0.0;
            } else {
                let mut new_life = ts.profiles_per_second.compute(b.age, b.seed);
                if new_life < 0.0 {
                    new_life = 0.0;
                }
                t.birth_accumulator += d_t * new_life;
                if t.birth_accumulator >= 1.0 && (t.active as i32) < max_count {
                    if t.active == 0 {
                        t.head = 1;
                        t.tail = max_count - 1;
                        create_profile(ts, spec, b, t, 0, &local_to_world, ctx);
                        t.active = 2;
                    } else {
                        t.profiles[t.head as usize].profile_to_world = local_to_world;
                        t.head += 1;
                        if t.head == max_count {
                            t.head = 0;
                        }
                    }
                    create_profile(ts, spec, b, t, t.head as usize, &local_to_world, ctx);
                    t.birth_accumulator -= t.birth_accumulator.floor();
                } else if t.head >= 0 {
                    t.profiles[t.head as usize].profile_to_world = local_to_world;
                }
            }

            had_profiles = t.active != 0;
            if had_profiles {
                // All the active profiles, newest first.
                let mut i = t.head;
                let mut profile_count = 0usize;
                loop {
                    let idx = i as usize;
                    t.profiles[idx].age += d_t * t.profiles[idx].age_rate;
                    let Some((center, radius)) =
                        animate_profile(ts, t, idx, profile_count, &new_world_to_local)
                    else {
                        t.tail = i;
                        break;
                    };
                    let lo = Vec3::new(center.x - radius, center.y - radius, center.z - radius);
                    let hi = Vec3::new(center.x + radius, center.y + radius, center.z + radius);
                    tube_box = Some(match tube_box {
                        None => (lo, hi),
                        Some((mn, mx)) => (
                            Vec3::new(mn.x.min(lo.x), mn.y.min(lo.y), mn.z.min(lo.z)),
                            Vec3::new(mx.x.max(hi.x), mx.y.max(hi.y), mx.z.max(hi.z)),
                        ),
                    });
                    profile_count += 1;
                    i -= 1;
                    if i < 0 {
                        i = max_count - 1;
                    }
                    if i == t.tail {
                        break;
                    }
                }
                t.active = profile_count;
            }
            b.age = prev_age;
        }

        if !self.effect_execute(ctx, info, bounds) {
            return false;
        }
        if !had_profiles {
            return true;
        }
        if let Some((mn, mx)) = tube_box {
            let mut local = Obb::from_extent_box(mn, mx);
            // `Tube::Execute` has no minimum-radius clamp.
            local.sphere_radius = local.axis_extents.length();
            let parent = Obb::multiply(&local, &self.b.local_to_parent);
            *bounds = Obb::union(bounds, &parent);
        }
        true
    }

    /// `Tube::Draw`.
    pub(crate) fn tube_draw(&self, info: &DrawInfo, out: &mut DrawList) {
        let (SpecKind::Tube(ts), Data::Tube(t)) = (&self.spec.kind, &self.data) else {
            return;
        };
        if t.active <= 1 {
            return;
        }
        let (state, local_to_world) = self.draw_frame(info);
        let vc = ts.vertices.len();
        let n_verts = t.active * vc;
        let n_tris = 2 * (t.active - 1) * (vc - 1);

        let mut positions: Vec<Vec3> = t.vertices[..n_verts.min(t.vertices.len())].to_vec();
        if ts.profile_type == ProfileType::AlignedRibbon {
            // Orient each profile to face the camera, then place its vertices.
            let camera_in_world = info.camera_to_world.translation();
            let world_to_local = local_to_world.invert();
            positions.clear();
            let max = ts.max_profile_count;
            let mut i = t.head;
            loop {
                let p = &t.profiles[i as usize];
                let camera_in_profile = p.profile_to_world.point_by_inverse(camera_in_world);
                let scale = ts.p_scale.compute(p.age, p.seed);
                let disp = ts.p_displacement.compute(p.age, p.seed);
                let offset_in_profile = p.direction * disp;
                let direction = camera_in_profile - offset_in_profile;
                let mut rot = Mat::IDENTITY;
                rot.align_local_axis_to_world_vector(direction, Z_AXIS, Y_AXIS, None);
                let mut template_to_profile = Mat::IDENTITY;
                for r in 0..3 {
                    for c in 0..3 {
                        template_to_profile.m[r][c] = scale * rot.m[r][c];
                    }
                }
                template_to_profile.build_translation(offset_in_profile);
                let template_to_world = Mat::multiply(&template_to_profile, &p.profile_to_world);
                let template_to_local = Mat::multiply(&template_to_world, &world_to_local);
                for v in &ts.vertices {
                    positions.push(template_to_local.point(*v));
                }
                i -= 1;
                if i < 0 {
                    i = max - 1;
                }
                if i == t.tail {
                    break;
                }
            }
        }

        let verts: Vec<DrawVertex> = positions
            .iter()
            .enumerate()
            .map(|(k, p)| DrawVertex {
                pos: local_to_world.point(*p).to_array(),
                uv: t.uvs.get(k).copied().unwrap_or([0.0; 2]),
                color: t.colors.get(k).copied().unwrap_or_default().to_array(),
            })
            .collect();
        let count = (n_tris * 3).min(t.indices.len());
        let indices: Vec<u32> = t.indices[..count]
            .iter()
            .copied()
            .filter(|&i| (i as usize) < verts.len())
            .collect();
        if indices.len() == count {
            out.push(&state, &verts, &indices);
        }
        let _ = SMALL;
    }
}
