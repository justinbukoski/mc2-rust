// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `DebrisCloud` (`mclib/gosfx/debriscloud.cpp`): the pieces of an exploded object flying
//! outward from a centre of force, each a scalable MLR shape that fades by its own alpha curve.

use crate::draw::{DrawInfo, DrawList};
use crate::effect::{Ctx, Data, Effect, ExecuteInfo};
use crate::fcurve::FCurve;
use crate::math::{lerp, Mat, Obb, Quat, Rgba, Vec3};
use crate::singleton::draw_shape;
use crate::spec::{DebrisCloudSpec, SpecKind};
use crate::state::MlrState;

#[derive(Clone, Debug)]
pub(crate) struct Piece {
    pub local_to_parent: Mat,
    pub age: f32,
    pub age_rate: f32,
    pub seed: f32,
    pub angular_velocity: Vec3,
    pub linear_velocity: Vec3,
    pub alpha: f32,
}

pub(crate) struct DebrisState {
    pub pieces: Vec<Piece>,
}

impl DebrisState {
    pub(crate) fn new(spec: &DebrisCloudSpec) -> DebrisState {
        DebrisState {
            pieces: vec![
                Piece {
                    local_to_parent: Mat::IDENTITY,
                    age: 0.0,
                    age_rate: 0.0,
                    seed: 0.0,
                    angular_velocity: Vec3::ZERO,
                    linear_velocity: Vec3::ZERO,
                    alpha: 0.0,
                };
                spec.pieces.len()
            ],
        }
    }
}

impl Effect {
    /// Positions of the debris pieces in the cloud's space (pieces that are still flying).
    pub fn debris_positions(&self) -> Vec<Vec3> {
        match &self.data {
            Data::Debris(d) => d
                .pieces
                .iter()
                .filter(|p| p.age < 1.0)
                .map(|p| p.local_to_parent.translation())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Linear velocities of the debris pieces that are still flying.
    pub fn debris_velocities(&self) -> Vec<Vec3> {
        match &self.data {
            Data::Debris(d) => d
                .pieces
                .iter()
                .filter(|p| p.age < 1.0)
                .map(|p| p.linear_velocity)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// `DebrisCloud::Start`.
    pub(crate) fn debris_start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        self.effect_start(ctx, info);
        let Effect { spec, data, .. } = self;
        let (SpecKind::DebrisCloud(ds), Data::Debris(d)) = (&spec.kind, data) else {
            return;
        };
        for (i, piece) in d.pieces.iter_mut().enumerate() {
            let sp = &ds.pieces[i];
            piece.local_to_parent = Mat::IDENTITY;
            piece.local_to_parent.build_translation(sp.position);
            piece.age = 0.0;
            piece.seed = sp.seed;
            let mut lifetime = ds.p_life_span.compute(0.0, piece.seed);
            if lifetime < 0.033_333_3 {
                lifetime = 0.033_333_3;
            }
            piece.age_rate = 1.0 / lifetime;
            let v = sp.position - ds.center_of_force;
            let lerp_factor = lerp(
                ds.minimum_deviation.compute(0.0, piece.seed),
                ds.maximum_deviation.compute(0.0, piece.seed),
                ctx.rng.fraction(),
            );
            let dir = Vec3::UP.lerp(v, lerp_factor).normalized();
            piece.linear_velocity = dir * ds.starting_speed.compute(0.0, piece.seed);
            let temp = ds.p_spin.compute(0.0, piece.seed);
            piece.angular_velocity = Vec3::new(
                temp * (2.0 * ctx.rng.fraction() - 1.0),
                temp * (2.0 * ctx.rng.fraction() - 1.0),
                temp * (2.0 * ctx.rng.fraction() - 1.0),
            );
        }
    }

    /// `DebrisCloud::Kill`.
    pub(crate) fn debris_kill(&mut self) {
        if let Data::Debris(d) = &mut self.data {
            for p in d.pieces.iter_mut() {
                p.age = 1.0;
            }
        }
        self.effect_kill();
    }

    /// `DebrisCloud::Execute`.
    pub(crate) fn debris_execute(
        &mut self,
        ctx: &mut Ctx,
        info: &ExecuteInfo,
        bounds: &mut Obb,
    ) -> bool {
        if !self.is_executed() {
            return false;
        }
        {
            let Effect { spec, b, data } = self;
            let (SpecKind::DebrisCloud(ds), Data::Debris(d)) = (&spec.kind, data) else {
                return false;
            };
            let d_t = (info.time - b.last_ran) as f32;
            let prev_age = b.age;
            b.age += d_t * b.age_rate;
            for piece in d.pieces.iter_mut() {
                if piece.age < 1.0 {
                    piece.age += d_t * piece.age_rate;
                    if !animate_piece(ds, &b.local_to_world, b.last_ran, piece, info.time) {
                        piece.age = 1.0;
                    }
                }
            }
            b.age = prev_age;
        }
        if !self.effect_execute(ctx, info, bounds) {
            return false;
        }

        // Bounds: "the first legal point we find" then the rest. The C++ steps the index twice
        // per skipped piece in the first loop (a `for` that also does `i++`); kept.
        let Effect { spec, b, data } = self;
        let (SpecKind::DebrisCloud(ds), Data::Debris(d)) = (&spec.kind, data) else {
            return true;
        };
        let n = d.pieces.len();
        let mut min = Vec3::ZERO;
        let mut max = Vec3::ZERO;
        let mut i = 0usize;
        while i < n {
            let piece = &d.pieces[i];
            if piece.age < 1.0 {
                let r = ds.pieces[i].sphere_radius;
                let pt = piece.local_to_parent.point(ds.pieces[i].sphere_center);
                max = Vec3::new(pt.x + r, pt.y + r, pt.z + r);
                min = Vec3::new(pt.x - r, pt.y - r, pt.z - r);
                break;
            }
            i += 2;
        }
        while i < n {
            let piece = &d.pieces[i];
            if piece.age < 1.0 {
                let r = ds.pieces[i].sphere_radius;
                let pt = piece.local_to_parent.point(ds.pieces[i].sphere_center);
                min = Vec3::new(
                    min.x.min(pt.x - r),
                    min.y.min(pt.y - r),
                    min.z.min(pt.z - r),
                );
                max = Vec3::new(
                    max.x.max(pt.x + r),
                    max.y.max(pt.y + r),
                    max.z.max(pt.z + r),
                );
            }
            i += 1;
        }
        let local = Obb::from_extent_box(min, max);
        let parent = Obb::multiply(&local, &b.local_to_parent);
        *bounds = Obb::union(bounds, &parent);
        true
    }

    /// `DebrisCloud::Draw`.
    pub(crate) fn debris_draw(&self, info: &DrawInfo, out: &mut DrawList) {
        let (SpecKind::DebrisCloud(ds), Data::Debris(d)) = (&self.spec.kind, &self.data) else {
            return;
        };
        let combined = MlrState::combine(&info.state, &self.spec.base.state);
        let local_to_world = Mat::multiply(&self.b.local_to_parent, &info.parent_to_world);
        for (i, piece) in d.pieces.iter().enumerate() {
            if piece.age < 1.0 {
                let shape_to_world = Mat::multiply(&piece.local_to_parent, &local_to_world);
                draw_shape(
                    &ds.pieces[i].shape,
                    &shape_to_world,
                    info.camera_to_world.translation(),
                    Vec3::new(1.0, 1.0, 1.0),
                    Rgba::new(1.0, 1.0, 1.0, piece.alpha),
                    &combined,
                    out,
                );
            }
        }
    }
}

/// `DebrisCloud::AnimateParticle`.
fn animate_piece(
    ds: &DebrisCloudSpec,
    local_to_world: &Mat,
    last_ran: f64,
    piece: &mut Piece,
    till: f64,
) -> bool {
    let seed = piece.seed;
    let age = piece.age;
    if age >= 1.0 {
        return false;
    }
    piece.alpha = ds.p_alpha.compute(age, seed);

    let mut translation = piece.local_to_parent.translation();
    let mut rotation = Quat::from_matrix(&piece.local_to_parent);

    let mut drag = -ds.p_drag.compute(age, seed);
    if drag > 0.0 {
        drag = 0.0;
    }
    let ether = Vec3::new(0.0, ds.p_ether_velocity_y.compute(age, seed), 0.0);

    // The forces are world-space; bring them into the effect's space.
    let world_to_effect = local_to_world.invert();
    let local_ether = world_to_effect.vector_by_inverse(ether);
    let rel_vel = piece.linear_velocity - local_ether;
    let mut accel = rel_vel * drag;
    let world_accel = Vec3::new(0.0, ds.p_acceleration_y.compute(age, seed), 0.0);
    accel += world_to_effect.vector(world_accel);

    let time_slice = (till - last_ran) as f32;
    piece.linear_velocity = piece.linear_velocity.add_scaled(accel, time_slice);
    translation = translation.add_scaled(piece.linear_velocity, time_slice);

    let omega = piece.angular_velocity * time_slice;
    let omega_q = Quat::from_rotation_vector(omega);
    let mut r = Quat::multiply(&omega_q, &rotation);
    r.normalize();
    rotation = r;

    piece.local_to_parent.build_rotation(&rotation);
    piece.local_to_parent.build_translation(translation);
    true
}
