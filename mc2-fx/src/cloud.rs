// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Particle clouds: `ParticleCloud`, `PointCloud`, `SpinningCloud` and its subclasses
//! `ShardCloud`, `PertCloud`, `CardCloud`, `ShapeCloud` and `EffectCloud`
//! (`mclib/gosfx/{particlecloud,pointcloud,spinningcloud,shardcloud,pertcloud,cardcloud,
//! shapecloud,effectcloud}.cpp`).
//!
//! The C++ lays particles out in a byte array and uses virtual `CreateNewParticle`,
//! `AnimateParticle` and `DestroyParticle`. Here one [`Particle`] struct carries the union of
//! the fields and the three hooks are `match`es on the specification kind, calling the parent
//! level first exactly as the C++ chain does (`ParticleCloud` -> `SpinningCloud` -> subclass).

use crate::draw::{DrawInfo, DrawList, DrawState, DrawVertex};
use crate::effect::{
    Base, Ctx, Data, Effect, ExecuteInfo, DYNAMIC_WORLD_SPACE_SIMULATION, EXECUTE_FLAG,
    SIMULATION_MODE_MASK, STATIC_WORLD_SPACE_SIMULATION,
};
use crate::fcurve::FCurve;
use crate::math::{
    truncate_float_to_byte, Mat, Obb, Quat, Rgba, Vec3, PI, PI_OVER_2, TWO_PI, X_AXIS, Y_AXIS,
    Z_AXIS,
};
use crate::singleton::draw_shape;
use crate::spec::{EffectSpec, ParticleSpec, SpecKind, SpinSpec};

/// The union of `ParticleCloud__Particle`, `SpinningCloud__Particle` and the subclass fields.
pub(crate) struct Particle {
    pub local_linear_velocity: Vec3,
    pub world_linear_velocity: Vec3,
    pub seed: f32,
    pub age: f32,
    pub age_rate: f32,
    // spinning
    pub angular_velocity: Vec3,
    pub local_translation: Vec3,
    pub world_translation: Vec3,
    pub local_rotation: Quat,
    pub world_rotation: Quat,
    pub radius: f32,
    pub scale: f32,
    // shard
    pub angle: f32,
    // card
    pub half_x: f32,
    pub half_y: f32,
    pub uv: [[f32; 2]; 4],
    // colour: point/shard/card use `colors[0]`, pert uses `colors[0]` (centre) and `colors[1]`
    // (rim), shape uses `colors[0]` as the paint colour
    pub colors: [Rgba; 2],
    // pert: the polygon template
    pub vertices: Vec<Vec3>,
    // effect cloud: the child effect riding on the particle
    pub effect: Option<Box<Effect>>,
}

impl Default for Particle {
    fn default() -> Self {
        Particle {
            local_linear_velocity: Vec3::ZERO,
            world_linear_velocity: Vec3::ZERO,
            seed: 0.0,
            age: 0.0,
            age_rate: 0.0,
            angular_velocity: Vec3::ZERO,
            local_translation: Vec3::ZERO,
            world_translation: Vec3::ZERO,
            local_rotation: Quat::IDENTITY,
            world_rotation: Quat::IDENTITY,
            radius: 0.0,
            scale: 0.0,
            angle: 0.0,
            half_x: 0.0,
            half_y: 0.0,
            uv: [[0.0; 2]; 4],
            colors: [Rgba::default(); 2],
            vertices: Vec::new(),
            effect: None,
        }
    }
}

pub struct Cloud {
    pub(crate) active: usize,
    pub(crate) birth_accumulator: f32,
    pub(crate) particles: Vec<Particle>,
}

impl Cloud {
    pub(crate) fn new(spec: &EffectSpec) -> Cloud {
        let n = particle_spec(spec).map_or(0, |p| p.max_particle_count.max(0) as usize);
        Cloud {
            active: 0,
            birth_accumulator: 0.0,
            particles: (0..n).map(|_| Particle::default()).collect(),
        }
    }

    /// Particles currently alive (age below 1), for tests and statistics.
    pub fn alive_count(&self) -> usize {
        self.particles[..self.active]
            .iter()
            .filter(|p| p.age < 1.0)
            .count()
    }

    /// Cloud-space positions of the live particles, in slot order.
    pub fn positions(&self) -> Vec<Vec3> {
        self.particles[..self.active]
            .iter()
            .filter(|p| p.age < 1.0)
            .map(|p| p.local_translation)
            .collect()
    }

    /// Ages of the live particles, in slot order.
    pub fn ages(&self) -> Vec<f32> {
        self.particles[..self.active]
            .iter()
            .filter(|p| p.age < 1.0)
            .map(|p| p.age)
            .collect()
    }
}

pub(crate) fn particle_spec(spec: &EffectSpec) -> Option<&ParticleSpec> {
    Some(match &spec.kind {
        SpecKind::PointCloud(p) => p,
        SpecKind::ShardCloud(s) => &s.spin.particle,
        SpecKind::PertCloud(s) => &s.spin.particle,
        SpecKind::CardCloud(s) => &s.spin.particle,
        SpecKind::ShapeCloud(s) => &s.spin.particle,
        SpecKind::EffectCloud(s) => &s.spin.particle,
        _ => return None,
    })
}

/// The force and integration step shared by `PointCloud::AnimateParticle` and
/// `SpinningCloud::AnimateParticle`: drag towards the ether velocity plus the acceleration
/// curves, in the particle's space (`vel`/`pos`), then Euler integration over `time_slice`.
#[allow(clippy::too_many_arguments)]
fn integrate_linear(
    ps: &ParticleSpec,
    sim_mode: u32,
    age: f32,
    seed: f32,
    local_to_world: &Mat,
    vel: &mut Vec3,
    pos: &mut Vec3,
    time_slice: f32,
) {
    // "Drag can never assist velocity."
    let mut drag = -ps.p_drag.compute(age, seed);
    if drag > 0.0 {
        drag = 0.0;
    }
    let ether = Vec3::new(
        ps.p_ether_velocity_x.compute(age, seed),
        ps.p_ether_velocity_y.compute(age, seed),
        ps.p_ether_velocity_z.compute(age, seed),
    );
    let accel = if sim_mode == STATIC_WORLD_SPACE_SIMULATION {
        // Pseudo-world simulation: the forces are world-space and are brought into the
        // effect's space. (`MultiplyByInverse(ether, world_to_effect)` is what the C++ does
        // for the ether, so it is kept as is.)
        let world_to_effect = local_to_world.invert();
        let local_ether = world_to_effect.vector_by_inverse(ether);
        let rel_vel = *vel - local_ether;
        let mut a = rel_vel * drag;
        let world_accel = Vec3::new(
            ps.p_acceleration_x.compute(age, seed),
            ps.p_acceleration_y.compute(age, seed),
            ps.p_acceleration_z.compute(age, seed),
        );
        a += world_to_effect.vector(world_accel);
        a
    } else {
        let rel_vel = *vel - ether;
        let mut a = rel_vel * drag;
        a.x += ps.p_acceleration_x.compute(age, seed);
        a.y += ps.p_acceleration_y.compute(age, seed);
        a.z += ps.p_acceleration_z.compute(age, seed);
        a
    };
    *vel = vel.add_scaled(accel, time_slice);
    *pos = pos.add_scaled(*vel, time_slice);
}

/// `ParticleCloud::CreateNewParticle`.
fn create_base(ps: &ParticleSpec, spec: &EffectSpec, b: &Base, p: &mut Particle, ctx: &mut Ctx) {
    p.age = 0.0;
    let min_seed = spec.base.min_child_seed.compute(b.age, b.seed);
    let seed_range = spec.base.max_child_seed.compute(b.age, b.seed) - min_seed;
    let seed = (ctx.rng.fraction() * seed_range + min_seed).clamp(0.0, 1.0);
    p.seed = seed;
    let mut lifetime = ps.p_life_span.compute(b.age, seed);
    if lifetime < 0.033_333_3 {
        lifetime = 0.033_333_3;
    }
    p.age_rate = 1.0 / lifetime;

    // Initial position inside the emitter.
    let yaw = ctx.rng.fraction() * TWO_PI;
    let pitch = ctx.rng.fraction() * PI - PI_OVER_2;
    let range = ctx.rng.fraction();
    let position = Vec3::from_yaw_pitch_range(yaw, pitch, range);
    let translation = Vec3::new(
        position.x * ps.emitter_size_x.compute(b.age, seed),
        position.y * ps.emitter_size_y.compute(b.age, seed),
        position.z * ps.emitter_size_z.compute(b.age, seed),
    );
    p.local_translation = translation;

    // Initial velocity.
    let mut pitch_min = ps.minimum_deviation.compute(b.age, seed);
    let mut pitch_range = ps.maximum_deviation.compute(b.age, seed) - pitch_min;
    if pitch_range < 0.0 {
        pitch_range = 0.0;
    }
    pitch_min += pitch_range * ctx.rng.fraction() - PI_OVER_2;
    let vyaw = ctx.rng.fraction() * TWO_PI;
    p.local_linear_velocity =
        Vec3::from_yaw_pitch_range(vyaw, pitch_min, ps.starting_speed.compute(b.age, seed));
}

/// `SpinningCloud::CreateNewParticle` (after the base).
fn create_spin(sp: &SpinSpec, b: &Base, p: &mut Particle, ctx: &mut Ctx) {
    let seed = p.seed;
    let age = b.age;
    p.angular_velocity = Vec3::new(
        sp.p_spin.compute(age, seed),
        sp.p_spin.compute(age, seed),
        sp.p_spin.compute(age, seed),
    );
    if sp.align_y_using_velocity {
        let mut basis = Mat::IDENTITY;
        basis.align_local_axis_to_world_vector(
            p.local_linear_velocity,
            Y_AXIS,
            X_AXIS,
            Some(Z_AXIS),
        );
        p.local_rotation = Quat::from_matrix(&basis);
    } else if sp.random_starting_rotation {
        let pitch = TWO_PI * ctx.rng.fraction();
        let yaw = PI * ctx.rng.fraction();
        let roll = TWO_PI * ctx.rng.fraction();
        p.local_rotation = Quat::from_euler(pitch, yaw, roll);
    } else {
        p.local_rotation = Quat::IDENTITY;
    }
}

/// `CreateNewParticle` for the whole chain of the cloud kind.
fn create_particle(spec: &EffectSpec, b: &Base, cloud: &mut Cloud, i: usize, ctx: &mut Ctx) {
    let Some(ps) = particle_spec(spec) else {
        return;
    };
    let p = &mut cloud.particles[i];
    create_base(ps, spec, b, p, ctx);
    let seed = p.seed;
    match &spec.kind {
        SpecKind::PointCloud(_) => {}
        SpecKind::ShardCloud(s) => {
            create_spin(&s.spin, b, p, ctx);
            p.radius = s.size.compute(b.age, seed);
            p.angle = s.angularity.compute(b.age, seed).sin();
        }
        SpecKind::PertCloud(s) => {
            create_spin(&s.spin, b, p, ctx);
            // The polygon: a centre point and a ring whose points alternate in and out of the
            // plane by the perturbation.
            let verts = s.vertices as usize;
            let angle_between = TWO_PI / (verts - 2) as f32;
            let radius = s.size.compute(b.age, seed);
            let mut even = 1.0f32;
            p.vertices.clear();
            p.vertices.push(Vec3::ZERO);
            let mut bound = 0.0f32;
            for j in 1..verts - 1 {
                let perturbance = even * s.perturbation.compute(b.age, seed);
                let temp = perturbance + radius;
                p.vertices.push(Vec3::new(
                    (j as f32 * angle_between).sin() * temp,
                    (j as f32 * angle_between).cos() * temp,
                    perturbance,
                ));
                let pp = temp * temp + perturbance * perturbance;
                if pp > bound {
                    bound = pp;
                }
                even = -even;
            }
            p.radius = bound.sqrt();
            let first = p.vertices[1];
            p.vertices.push(first);
        }
        SpecKind::CardCloud(s) => {
            create_spin(&s.spin, b, p, ctx);
            p.half_y = s.half_height.compute(b.age, seed);
            p.half_x = p.half_y * s.aspect_ratio.compute(b.age, seed);
            p.radius = (p.half_x * p.half_x + p.half_y * p.half_y).sqrt();
        }
        SpecKind::ShapeCloud(s) => {
            create_spin(&s.spin, b, p, ctx);
            p.radius = s.radius;
        }
        SpecKind::EffectCloud(s) => {
            create_spin(&s.spin, b, p, ctx);
            // A new effect under the cloud, riding on the particle.
            let flags = EXECUTE_FLAG | DYNAMIC_WORLD_SPACE_SIMULATION;
            p.effect = Effect::new(ctx.lib, s.particle_effect_id as usize, flags).map(Box::new);
            p.radius = 0.0;
            let (pos, rot, particle_seed, age, age_rate) = (
                p.local_translation,
                p.local_rotation,
                p.seed,
                p.age,
                p.age_rate,
            );
            if let Some(effect) = p.effect.as_mut() {
                let mut m = Mat::IDENTITY;
                m.build_translation(pos);
                m.build_rotation(&rot);
                effect.set_local_to_parent(m);
                let mut local_info = ExecuteInfo::new(b.last_ran, b.local_to_world);
                local_info.seed = particle_seed;
                local_info.age = age;
                local_info.age_rate = age_rate;
                effect.start(ctx, &local_info);
            }
        }
        _ => {}
    }
}

/// `PointCloud::AnimateParticle`.
fn animate_point(
    ps: &ParticleSpec,
    b: &Base,
    p: &mut Particle,
    world_to_new_local: Option<&Mat>,
    till: f64,
) -> bool {
    let age = p.age;
    if age >= 1.0 {
        return false;
    }
    let sim_mode = b.flags & SIMULATION_MODE_MASK;
    let seed = p.seed;
    let time_slice = (till - b.last_ran) as f32;
    if sim_mode == DYNAMIC_WORLD_SPACE_SIMULATION {
        p.world_linear_velocity = b.local_to_world.vector(p.local_linear_velocity);
        p.world_translation = b.local_to_world.point(p.local_translation);
        let (mut vel, mut pos) = (p.world_linear_velocity, p.world_translation);
        integrate_linear(
            ps,
            sim_mode,
            age,
            seed,
            &b.local_to_world,
            &mut vel,
            &mut pos,
            time_slice,
        );
        p.world_linear_velocity = vel;
        p.world_translation = pos;
        if let Some(m) = world_to_new_local {
            p.local_linear_velocity = m.vector(p.world_linear_velocity);
            p.local_translation = m.point(p.world_translation);
        }
    } else {
        let (mut vel, mut pos) = (p.local_linear_velocity, p.local_translation);
        integrate_linear(
            ps,
            sim_mode,
            age,
            seed,
            &b.local_to_world,
            &mut vel,
            &mut pos,
            time_slice,
        );
        p.local_linear_velocity = vel;
        p.local_translation = pos;
    }
    p.colors[0] = Rgba::new(
        ps.p_red.compute(age, seed),
        ps.p_green.compute(age, seed),
        ps.p_blue.compute(age, seed),
        ps.p_alpha.compute(age, seed),
    );
    true
}

/// `SpinningCloud::AnimateParticle`.
fn animate_spin(
    ps: &ParticleSpec,
    sp: &SpinSpec,
    b: &Base,
    p: &mut Particle,
    world_to_new_local: Option<&Mat>,
    till: f64,
) -> bool {
    let age = p.age;
    if age >= 1.0 {
        return false;
    }
    let sim_mode = b.flags & SIMULATION_MODE_MASK;
    let seed = p.seed;
    let time_slice = (till - b.last_ran) as f32;
    let dynamic = sim_mode == DYNAMIC_WORLD_SPACE_SIMULATION;

    // Work in world space for a dynamic-world cloud, in cloud space otherwise.
    let (mut pos, mut rot, mut vel) = if dynamic {
        let world_pos = b.local_to_world.point(p.local_translation);
        let local_rot = Mat::from_quat(&p.local_rotation);
        let world_rot = Quat::from_matrix(&Mat::multiply(&local_rot, &b.local_to_world));
        let world_vel = b.local_to_world.vector(p.local_linear_velocity);
        (world_pos, world_rot, world_vel)
    } else {
        (
            p.local_translation,
            p.local_rotation,
            p.local_linear_velocity,
        )
    };

    integrate_linear(
        ps,
        sim_mode,
        age,
        seed,
        &b.local_to_world,
        &mut vel,
        &mut pos,
        time_slice,
    );

    if !sp.align_y_using_velocity {
        let omega = p.angular_velocity * time_slice;
        let omega_q = Quat::from_rotation_vector(omega);
        let mut r = Quat::multiply(&omega_q, &rot);
        r.normalize();
        rot = r;
    }

    if dynamic {
        p.world_linear_velocity = vel;
        p.world_translation = pos;
        p.world_rotation = rot;
        if let Some(m) = world_to_new_local {
            p.local_linear_velocity = m.vector(vel);
            p.local_translation = m.point(pos);
            let world_rot_m = Mat::from_quat(&rot);
            p.local_rotation = Quat::from_matrix(&Mat::multiply(&world_rot_m, m));
        }
    } else {
        p.local_linear_velocity = vel;
        p.local_translation = pos;
        p.local_rotation = rot;
    }

    if sp.align_y_using_velocity {
        let mut basis = Mat::IDENTITY;
        basis.align_local_axis_to_world_vector(
            p.local_linear_velocity,
            Y_AXIS,
            X_AXIS,
            Some(Z_AXIS),
        );
        p.local_rotation = Quat::from_matrix(&basis);
    }

    p.scale = sp.p_scale.compute(age, seed);
    true
}

/// `AnimateParticle` for the whole chain of the cloud kind.
fn animate_particle(
    spec: &EffectSpec,
    b: &Base,
    cloud: &mut Cloud,
    i: usize,
    world_to_new_local: Option<&Mat>,
    till: f64,
    ctx: &mut Ctx,
) -> bool {
    let Some(ps) = particle_spec(spec) else {
        return false;
    };
    let p = &mut cloud.particles[i];
    match &spec.kind {
        SpecKind::PointCloud(_) => animate_point(ps, b, p, world_to_new_local, till),
        SpecKind::ShardCloud(s) => {
            if !animate_spin(ps, &s.spin, b, p, world_to_new_local, till) {
                return false;
            }
            let (age, seed) = (p.age, p.seed);
            p.colors[0] = particle_color(ps, age, seed);
            true
        }
        SpecKind::PertCloud(s) => {
            if !animate_spin(ps, &s.spin, b, p, world_to_new_local, till) {
                return false;
            }
            let (age, seed) = (p.age, p.seed);
            p.colors[0] = Rgba::new(
                s.p_center_red.compute(age, seed),
                s.p_center_green.compute(age, seed),
                s.p_center_blue.compute(age, seed),
                s.p_center_alpha.compute(age, seed),
            );
            p.colors[1] = particle_color(ps, age, seed);
            true
        }
        SpecKind::CardCloud(s) => {
            if !animate_spin(ps, &s.spin, b, p, world_to_new_local, till) {
                return false;
            }
            let (age, seed) = (p.age, p.seed);
            p.colors[0] = particle_color(ps, age, seed);
            let uv = &s.uv;
            let mut u = uv.u_offset.value;
            let mut v = uv.v_offset.value;
            let mut u2 = uv.u_size.value;
            let mut v2 = uv.v_size.value;
            if uv.animated {
                let mut columns = truncate_float_to_byte(s.p_index.compute(age, seed));
                let rows = columns.checked_div(uv.width).unwrap_or(0);
                columns = columns.wrapping_sub(rows.wrapping_mul(uv.width));
                u += u2 * columns as f32;
                v += v2 * rows as f32;
            }
            u2 += u;
            v2 += v;
            p.uv = [[u, v2], [u2, v2], [u2, v], [u, v]];
            true
        }
        SpecKind::ShapeCloud(s) => {
            if !animate_spin(ps, &s.spin, b, p, world_to_new_local, till) {
                return false;
            }
            let (age, seed) = (p.age, p.seed);
            p.colors[0] = particle_color(ps, age, seed);
            true
        }
        SpecKind::EffectCloud(s) => {
            // Don't let the age counters of the child effects run out.
            if p.age >= 1.0 {
                p.age = 1.0 - crate::math::SMALL;
            }
            animate_spin(ps, &s.spin, b, p, world_to_new_local, till);
            let mut m = Mat::IDENTITY;
            m.build_translation(p.local_translation);
            m.build_rotation(&p.local_rotation);
            let Some(effect) = p.effect.as_mut() else {
                return false;
            };
            effect.set_local_to_parent(m);
            let mut bounds = Obb::IDENTITY;
            let info = ExecuteInfo::new(till, b.local_to_world);
            if effect.execute(ctx, &info, &mut bounds) {
                let center = bounds.local_to_parent.translation();
                p.radius = center.length() + bounds.sphere_radius;
                return true;
            }
            p.radius = 0.0;
            p.effect = None;
            false
        }
        _ => false,
    }
}

fn particle_color(ps: &ParticleSpec, age: f32, seed: f32) -> Rgba {
    Rgba::new(
        ps.p_red.compute(age, seed),
        ps.p_green.compute(age, seed),
        ps.p_blue.compute(age, seed),
        ps.p_alpha.compute(age, seed),
    )
}

/// `DestroyParticle`.
fn destroy_particle(cloud: &mut Cloud, i: usize) {
    let p = &mut cloud.particles[i];
    p.effect = None;
    p.age = 1.0;
}

impl Effect {
    /// Live particles of a cloud effect (0 for other kinds).
    pub fn cloud_active(&self) -> usize {
        match &self.data {
            Data::Cloud(c) => c.active,
            _ => 0,
        }
    }

    pub fn cloud(&self) -> Option<&Cloud> {
        match &self.data {
            Data::Cloud(c) => Some(c),
            _ => None,
        }
    }

    /// `ParticleCloud::Start`.
    pub(crate) fn cloud_start(&mut self, ctx: &mut Ctx, info: &ExecuteInfo) {
        self.effect_start(ctx, info);
        let Some(ps) = particle_spec(&self.spec) else {
            return;
        };
        let mut newbies = ps.starting_population.compute(self.b.age, self.b.seed);
        if newbies < 0.0 {
            newbies = 0.0;
        }
        if let Data::Cloud(c) = &mut self.data {
            c.birth_accumulator += newbies;
        }
    }

    /// `ParticleCloud::Kill`.
    pub(crate) fn cloud_kill(&mut self) {
        if let Data::Cloud(c) = &mut self.data {
            for i in 0..c.active {
                destroy_particle(c, i);
            }
            c.active = 0;
            c.birth_accumulator = 0.0;
        }
        self.effect_kill();
    }

    /// `ParticleCloud::Execute`, then the base `Effect::Execute`, then the bounds step of
    /// `PointCloud`/`SpinningCloud`.
    pub(crate) fn cloud_execute(
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
            let Data::Cloud(cloud) = data else {
                return false;
            };
            let Some(ps) = particle_spec(spec) else {
                return false;
            };

            // A dynamic-world cloud needs the world-to-new-local matrix to put its
            // particles back into the cloud's (moved) space.
            let sim_mode = b.flags & SIMULATION_MODE_MASK;
            let world_to_new_local = if sim_mode == DYNAMIC_WORLD_SPACE_SIMULATION {
                Some(Mat::multiply(&b.local_to_parent, &info.parent_to_world).invert())
            } else {
                None
            };

            // Birth rate.
            let d_t = (info.time - b.last_ran) as f32;
            let prev_age = b.age;
            b.age += d_t * b.age_rate;
            if b.age >= 1.0 {
                cloud.birth_accumulator = 0.0;
            } else {
                let mut new_life = ps.particles_per_second.compute(b.age, b.seed);
                if new_life < 0.0 {
                    new_life = 0.0;
                }
                cloud.birth_accumulator += d_t * new_life;
            }

            // The active particles: age them, kill the ones that die, and reuse their slots
            // for newborns.
            let mut last_real: isize = -1;
            for i in 0..cloud.active {
                if cloud.particles[i].age < 1.0 {
                    cloud.particles[i].age += d_t * cloud.particles[i].age_rate;
                    if animate_particle(
                        spec,
                        b,
                        cloud,
                        i,
                        world_to_new_local.as_ref(),
                        info.time,
                        ctx,
                    ) {
                        last_real = i as isize;
                        continue;
                    }
                    destroy_particle(cloud, i);
                }
                if cloud.birth_accumulator >= 1.0 {
                    create_particle(spec, b, cloud, i, ctx);
                    if animate_particle(
                        spec,
                        b,
                        cloud,
                        i,
                        world_to_new_local.as_ref(),
                        info.time,
                        ctx,
                    ) {
                        last_real = i as isize;
                    } else {
                        destroy_particle(cloud, i);
                    }
                    cloud.birth_accumulator -= 1.0;
                }
            }
            cloud.active = (last_real + 1) as usize;

            // Newborns beyond the current active count grow it.
            while cloud.birth_accumulator >= 1.0 && (cloud.active as i32) < ps.max_particle_count {
                let i = cloud.active;
                cloud.active += 1;
                create_particle(spec, b, cloud, i, ctx);
                if !animate_particle(
                    spec,
                    b,
                    cloud,
                    i,
                    world_to_new_local.as_ref(),
                    info.time,
                    ctx,
                ) {
                    destroy_particle(cloud, i);
                    cloud.active -= 1;
                }
                cloud.birth_accumulator -= 1.0;
            }

            // Only fractional births carry over to the next frame.
            cloud.birth_accumulator -= cloud.birth_accumulator.floor();
            b.age = prev_age;
        }

        if !self.effect_execute(ctx, info, bounds) {
            return false;
        }

        // Bounds of the live particles.
        let spec = self.spec.clone();
        let Data::Cloud(cloud) = &self.data else {
            return true;
        };
        if cloud.active > 0 {
            let is_point = matches!(spec.kind, SpecKind::PointCloud(_));
            let mut min = Vec3::ZERO;
            let mut max = Vec3::ZERO;
            let mut first = true;
            for p in &cloud.particles[..cloud.active] {
                if p.age >= 1.0 {
                    continue;
                }
                let r = if is_point { 0.0 } else { p.radius * p.scale };
                let c = p.local_translation;
                let (lo, hi) = (
                    Vec3::new(c.x - r, c.y - r, c.z - r),
                    Vec3::new(c.x + r, c.y + r, c.z + r),
                );
                if first {
                    min = lo;
                    max = hi;
                    first = false;
                } else {
                    min = Vec3::new(min.x.min(lo.x), min.y.min(lo.y), min.z.min(lo.z));
                    max = Vec3::new(max.x.max(hi.x), max.y.max(hi.y), max.z.max(hi.z));
                }
            }
            if !first {
                let local = Obb::from_extent_box(min, max);
                let parent = Obb::multiply(&local, &self.b.local_to_parent);
                *bounds = Obb::union(bounds, &parent);
            }
        }
        true
    }

    /// The `Draw` of every cloud kind.
    pub(crate) fn cloud_draw(&self, info: &DrawInfo, out: &mut DrawList) {
        let Data::Cloud(cloud) = &self.data else {
            return;
        };
        if cloud.active == 0 {
            return;
        }
        let (state, local_to_world) = self.draw_frame(info);
        let spec = &self.spec;
        let camera_in_cloud = local_to_world.point_by_inverse(info.camera_to_world.translation());
        let alive = || {
            cloud.particles[..cloud.active]
                .iter()
                .filter(|p| p.age < 1.0)
        };

        match &spec.kind {
            SpecKind::PointCloud(_) => {
                // MLRPointCloud: each point is a ~2.4 pixel triangle at its position. Emitted as a
                // small camera-facing quad whose size grows with distance.
                let cam = info.camera_to_world.translation();
                let right = info.camera_to_world.axis(X_AXIS);
                let up = info.camera_to_world.axis(Y_AXIS);
                for p in alive() {
                    let c = local_to_world.point(p.local_translation);
                    let size = (0.0019 * (c - cam).length()).max(0.02);
                    let color = p.colors[0].to_array();
                    let corners = [
                        c - right * size - up * size,
                        c + right * size - up * size,
                        c + right * size + up * size,
                        c - right * size + up * size,
                    ];
                    push_quad(out, &state, corners, [[0.0; 2]; 4], [color; 4]);
                }
            }
            SpecKind::ShardCloud(s) => {
                for p in alive() {
                    let m = Mat::multiply(&orient(&s.spin, p, camera_in_cloud), &local_to_world);
                    let sc = p.scale;
                    let pts = [
                        Vec3::new(0.0, -0.5 * sc * p.radius, 0.0),
                        Vec3::new(sc * p.angle * p.radius, sc * p.radius * 0.5, 0.0),
                        Vec3::new(-sc * p.angle * p.radius, sc * p.radius * 0.5, 0.0),
                    ];
                    let color = p.colors[0].to_array();
                    let v: Vec<DrawVertex> = pts
                        .iter()
                        .map(|q| DrawVertex {
                            pos: m.point(*q).to_array(),
                            uv: [0.0, 0.0],
                            color,
                        })
                        .collect();
                    out.push(&state, &v, &[0, 1, 2]);
                }
            }
            SpecKind::PertCloud(s) => {
                for p in alive() {
                    let m = Mat::multiply(&orient(&s.spin, p, camera_in_cloud), &local_to_world);
                    let sc = p.scale;
                    let n = s.vertices as usize;
                    let mut v = Vec::with_capacity(n);
                    for (k, q) in p.vertices.iter().enumerate().take(n) {
                        let scaled = Vec3::new(q.x * sc, q.y * sc, q.z * sc);
                        let color = if k == 0 { p.colors[0] } else { p.colors[1] };
                        v.push(DrawVertex {
                            pos: m.point(scaled).to_array(),
                            uv: [0.0, 0.0],
                            color: color.to_array(),
                        });
                    }
                    // MLRNGonCloud: a fan around vertex 0, `(0, z + 1, z)` for z in 1..n-1.
                    let mut idx = Vec::new();
                    for z in 1..n.saturating_sub(1) {
                        idx.extend_from_slice(&[0, z as u32 + 1, z as u32]);
                    }
                    out.push(&state, &v, &idx);
                }
            }
            SpecKind::CardCloud(s) => {
                for p in alive() {
                    let m = Mat::multiply(&orient(&s.spin, p, camera_in_cloud), &local_to_world);
                    let corners = card_corners(p.scale, p.half_x, p.half_y).map(|q| m.point(q));
                    let color = p.colors[0].to_array();
                    push_quad(out, &state, corners, p.uv, [color; 4]);
                }
            }
            SpecKind::ShapeCloud(s) => {
                let combined = crate::state::MlrState::combine(&info.state, &spec.base.state);
                for p in alive() {
                    let m = Mat::multiply(&orient(&s.spin, p, camera_in_cloud), &local_to_world);
                    draw_shape(
                        &s.shape,
                        &m,
                        info.camera_to_world.translation(),
                        Vec3::new(p.scale, p.scale, p.scale),
                        p.colors[0],
                        &combined,
                        out,
                    );
                }
            }
            SpecKind::EffectCloud(_) => {
                // EffectCloud::Draw hands its own `info` straight to the particle effects.
                for p in alive() {
                    if let Some(e) = &p.effect {
                        e.draw(info, out);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The per-particle orientation of `Draw`: the particle rotation, then the camera alignment
/// the specification asks for (`alignZUsingX`/`alignZUsingY`), then the translation.
fn orient(spin: &SpinSpec, p: &Particle, camera_in_cloud: Vec3) -> Mat {
    let mut m = Mat::IDENTITY;
    m.build_rotation(&p.local_rotation);
    let direction_in_cloud = camera_in_cloud - p.local_translation;
    if spin.align_z_using_x {
        if spin.align_z_using_y {
            m.align_local_axis_to_world_vector(direction_in_cloud, Z_AXIS, Y_AXIS, Some(X_AXIS));
        } else {
            m.align_local_axis_to_world_vector(direction_in_cloud, Z_AXIS, X_AXIS, None);
        }
    } else if spin.align_z_using_y {
        m.align_local_axis_to_world_vector(direction_in_cloud, Z_AXIS, Y_AXIS, None);
    }
    m.build_translation(p.local_translation);
    m
}

/// The four corners of a card in its own space, in the order `MLRCardCloud` takes them.
pub(crate) fn card_corners(scale: f32, half_x: f32, half_y: f32) -> [Vec3; 4] {
    [
        Vec3::new(scale * half_x, -scale * half_y, 0.0),
        Vec3::new(-scale * half_x, -scale * half_y, 0.0),
        Vec3::new(-scale * half_x, scale * half_y, 0.0),
        Vec3::new(scale * half_x, scale * half_y, 0.0),
    ]
}

/// Two triangles for a quad whose corners are in `MLRCardCloud` order.
pub(crate) fn push_quad(
    out: &mut DrawList,
    state: &DrawState,
    corners: [Vec3; 4],
    uvs: [[f32; 2]; 4],
    colors: [[f32; 4]; 4],
) {
    let v: Vec<DrawVertex> = (0..4)
        .map(|k| DrawVertex {
            pos: corners[k].to_array(),
            uv: uvs[k],
            color: colors[k],
        })
        .collect();
    out.push(state, &v, &[0, 1, 2, 0, 2, 3]);
}
