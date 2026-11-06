// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Spawn, decay and draw checks against values worked out by hand from the C++ formulas
//! (`mclib/gosfx/effect.cpp`, `particlecloud.cpp`, `pointcloud.cpp`, `cardcloud.cpp`,
//! `tube.cpp`, `debriscloud.cpp`). The specifications are built in code so every number is
//! visible in the test.

use mc2_fx::draw::{DrawInfo, DrawList};
use mc2_fx::effect::{
    Ctx, Effect, ExecuteInfo, DYNAMIC_WORLD_SPACE_SIMULATION, EXECUTE_FLAG, LOOP_FLAG,
};
use mc2_fx::library::EffectLibrary;
use mc2_fx::math::{Mat, Obb, Vec3};
use mc2_fx::rng::VisualRng;
use mc2_fx::spec::*;

fn base(name: &str, life: f32) -> BaseSpec {
    let mut b = BaseSpec {
        name: name.into(),
        ..BaseSpec::default()
    };
    b.life_span.value = life;
    b
}

fn plain(name: &str, life: f32) -> EffectSpec {
    EffectSpec::new(base(name, life), SpecKind::Effect)
}

struct Rig {
    lib: EffectLibrary,
    rng: VisualRng,
}

impl Rig {
    fn new(specs: Vec<EffectSpec>) -> Rig {
        Rig {
            lib: EffectLibrary::from_specs(17, specs),
            rng: VisualRng::new(42),
        }
    }
    fn effect(&self, id: usize, flags: u32) -> Effect {
        Effect::new(&self.lib, id, flags).unwrap()
    }
    fn start(&mut self, e: &mut Effect, t: f64, m: Mat) {
        let mut ctx = Ctx {
            lib: &self.lib,
            rng: &mut self.rng,
        };
        e.start(&mut ctx, &ExecuteInfo::new(t, m));
    }
    fn run(&mut self, e: &mut Effect, t: f64, m: Mat) -> bool {
        let mut ctx = Ctx {
            lib: &self.lib,
            rng: &mut self.rng,
        };
        let mut bounds = Obb::IDENTITY;
        e.execute(&mut ctx, &ExecuteInfo::new(t, m), &mut bounds)
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn an_effect_ages_by_the_inverse_life_span_and_dies_at_one() {
    // life span 2 s -> age rate 0.5 per second
    let mut rig = Rig::new(vec![plain("e", 2.0)]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 10.0, Mat::IDENTITY);
    assert_eq!(e.age(), 0.0);
    assert!(rig.run(&mut e, 10.5, Mat::IDENTITY));
    assert_eq!(e.age(), 0.25);
    assert!(rig.run(&mut e, 11.0, Mat::IDENTITY));
    assert_eq!(e.age(), 0.5);
    // 12.0: age = 0.5 + 1.0 * 0.5 = 1.0, no loop, no children -> killed, Execute returns false
    assert!(!rig.run(&mut e, 12.0, Mat::IDENTITY));
    assert!(!e.is_executed());
}

#[test]
fn life_span_is_clamped_to_a_thirtieth_of_a_second() {
    // 0.001 s is raised to 0.033333: age rate 30.0003; 0.01 s later the age is 0.300003
    let mut rig = Rig::new(vec![plain("e", 0.001)]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    assert!(rig.run(&mut e, 0.01, Mat::IDENTITY));
    assert!(close(e.age(), 0.01 / 0.033333));
}

#[test]
fn a_looped_effect_restarts_at_age_one() {
    let mut rig = Rig::new(vec![plain("e", 1.0)]);
    let mut e = rig.effect(0, EXECUTE_FLAG | LOOP_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    assert!(rig.run(&mut e, 0.75, Mat::IDENTITY));
    assert_eq!(e.age(), 0.75);
    // 1.25 s: age 1.25 >= 1 and looped -> Start() again: age back to 0, still executing
    assert!(rig.run(&mut e, 1.25, Mat::IDENTITY));
    assert_eq!(e.age(), 0.0);
    assert!(e.is_executed());
}

#[test]
fn events_spawn_children_when_the_parent_age_passes_them() {
    let mut parent = plain("parent", 2.0);
    let mut at = Mat::IDENTITY;
    at.build_translation(Vec3::new(0.0, 5.0, 0.0));
    parent.base.events.push(Event {
        time: 0.5,
        flags: 0,
        effect_id: 1,
        local_to_parent: at,
    });
    let mut rig = Rig::new(vec![parent, plain("child", 10.0)]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    // t = 1: the event is checked against the age *before* this step (0.0): not yet
    assert!(rig.run(&mut e, 1.0, Mat::IDENTITY));
    assert_eq!(e.age(), 0.5);
    assert!(e.children().is_empty());
    // t = 1.5: the old age 0.5 has reached the event time 0.5: the child appears
    assert!(rig.run(&mut e, 1.5, Mat::IDENTITY));
    assert_eq!(e.children().len(), 1);
    let child = &e.children()[0];
    assert_eq!(
        child.local_to_world().translation(),
        Vec3::new(0.0, 5.0, 0.0)
    );
    assert!((0.0..=1.0).contains(&child.seed()));
    // the parent ends at age 1 but waits for its child; killing it kills the child
    assert!(rig.run(&mut e, 2.0, Mat::IDENTITY));
    assert!(rig.run(&mut e, 3.0, Mat::IDENTITY));
    e.kill();
    assert!(!rig.run(&mut e, 3.5, Mat::IDENTITY));
}

fn point_cloud(per_second: f32, particle_life: f32, max: i32) -> EffectSpec {
    let mut p = ParticleSpec::default();
    p.particles_per_second.set_constant(per_second);
    p.p_life_span.age_curve.set_constant(particle_life);
    p.max_particle_count = max;
    EffectSpec::new(base("pc", 4.0), SpecKind::PointCloud(Box::new(p)))
}

#[test]
fn particles_are_born_at_the_requested_rate_and_slots_are_reused() {
    // 8 per second, particle life 1 s, executed every 0.25 s: 2 births per step.
    let mut rig = Rig::new(vec![point_cloud(8.0, 1.0, 20)]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    let mut alive = Vec::new();
    for step in 1..=5 {
        assert!(rig.run(&mut e, 0.25 * step as f64, Mat::IDENTITY));
        alive.push(e.cloud().unwrap().alive_count());
    }
    // ages after n steps are 0.25 n; the pair born at step 1 dies when its age reaches 1.0 at
    // step 5, and the newborn pair takes its slots
    assert_eq!(alive, vec![2, 4, 6, 8, 8]);
    assert_eq!(e.cloud_active(), 8);
}

#[test]
fn the_particle_cap_limits_the_population() {
    let mut rig = Rig::new(vec![point_cloud(8.0, 1.0, 3)]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    for step in 1..=3 {
        rig.run(&mut e, 0.25 * step as f64, Mat::IDENTITY);
    }
    assert_eq!(e.cloud().unwrap().alive_count(), 3);
}

#[test]
fn starting_population_is_added_to_the_birth_accumulator() {
    let mut spec = point_cloud(0.0, 1.0, 20);
    if let SpecKind::PointCloud(p) = &mut spec.kind {
        p.starting_population.value = 5.0;
    }
    let mut rig = Rig::new(vec![spec]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    assert!(rig.run(&mut e, 0.1, Mat::IDENTITY));
    assert_eq!(e.cloud().unwrap().alive_count(), 5);
}

#[test]
fn gravity_integrates_velocity_then_position() {
    // Acceleration -10 on y, no initial speed, no emitter size. A particle born in the step
    // that ends at t = 0.25 is animated over that whole step (the C++ passes
    // `till - m_lastRan`): v = -10 * 0.25 = -2.5, y = 0 + v * 0.25 = -0.625. One step later:
    // v = -5, y = -0.625 - 1.25 = -1.875.
    let mut spec = point_cloud(8.0, 1.0, 20);
    if let SpecKind::PointCloud(p) = &mut spec.kind {
        p.p_acceleration_y.age_curve.value = -10.0;
    }
    let mut rig = Rig::new(vec![spec]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    let pos = e.cloud().unwrap().positions();
    assert_eq!(pos.len(), 2);
    assert!(close(pos[0].y, -0.625) && close(pos[1].y, -0.625));
    assert!(close(pos[0].x, 0.0) && close(pos[0].z, 0.0));
    rig.run(&mut e, 0.5, Mat::IDENTITY);
    let pos = e.cloud().unwrap().positions();
    assert_eq!(pos.len(), 4);
    assert!(close(pos[0].y, -1.875));
    assert!(close(pos[2].y, -0.625));
}

#[test]
fn drag_pulls_velocity_towards_the_ether() {
    // initial speed 4 straight up (pitch -90 deg gives +y), drag 2 (a = -2 v), ether 0:
    // after one 0.25 s step v = 4 + (-2 * 4) * 0.25 = 2, y = 2 * 0.25 = 0.5
    let mut spec = point_cloud(4.0, 10.0, 4);
    if let SpecKind::PointCloud(p) = &mut spec.kind {
        p.starting_speed.age_curve.set_constant(4.0);
        p.p_drag.age_curve.set_constant(2.0);
        // deviation 0..0 fixes the pitch at -pi/2: straight up
    }
    let mut rig = Rig::new(vec![spec]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    let pos = e.cloud().unwrap().positions();
    assert_eq!(pos.len(), 1);
    assert!(close(pos[0].y, 0.5), "y = {}", pos[0].y);
    assert!(pos[0].x.abs() < 1e-4 && pos[0].z.abs() < 1e-4);
}

#[test]
fn a_dynamic_world_cloud_leaves_its_particles_behind() {
    // Dynamic world space: particles keep their world position when the effect moves, so
    // moving the parent by +4 on x moves the particles' local x by -4.
    let spec = point_cloud(8.0, 1.0, 20);
    let mut rig = Rig::new(vec![spec]);
    let mut e = rig.effect(0, EXECUTE_FLAG | DYNAMIC_WORLD_SPACE_SIMULATION);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    let moved = Mat::from_translation(Vec3::new(4.0, 0.0, 0.0));
    rig.run(&mut e, 0.5, moved);
    let pos = e.cloud().unwrap().positions();
    assert_eq!(pos.len(), 4);
    for p in pos {
        assert!(close(p.x, -4.0), "x = {}", p.x);
    }
}

#[test]
fn killing_a_cloud_empties_it() {
    let mut rig = Rig::new(vec![point_cloud(8.0, 1.0, 20)]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.5, Mat::IDENTITY);
    assert!(e.cloud_active() > 0);
    e.kill();
    assert_eq!(e.cloud_active(), 0);
    assert!(!e.is_executed());
}

fn card_cloud() -> EffectSpec {
    let mut s = CardCloudSpec::default();
    s.spin.particle.particles_per_second.set_constant(4.0);
    s.spin.particle.p_life_span.age_curve.set_constant(10.0);
    s.spin.particle.max_particle_count = 4;
    s.spin.particle.p_red.age_curve.set_constant(1.0);
    s.spin.particle.p_green.age_curve.set_constant(0.5);
    s.spin.particle.p_blue.age_curve.set_constant(0.25);
    s.spin.particle.p_alpha.age_curve.set_constant(0.75);
    s.spin.p_scale.age_curve.set_constant(1.0);
    s.half_height.age_curve.set_constant(2.0);
    s.aspect_ratio.age_curve.set_constant(0.5);
    s.uv.u_size.value = 0.5;
    s.uv.v_size.value = 0.5;
    s.uv.u_offset.value = 0.0;
    s.uv.v_offset.value = 0.0;
    EffectSpec::new(base("cards", 4.0), SpecKind::CardCloud(Box::new(s)))
}

#[test]
fn a_card_cloud_draws_textured_quads_with_the_particle_colour() {
    let mut rig = Rig::new(vec![card_cloud()]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    let place = Mat::from_translation(Vec3::new(10.0, 0.0, 0.0));
    rig.start(&mut e, 0.0, place);
    rig.run(&mut e, 0.25, place);
    let mut list = DrawList::default();
    e.draw(&DrawInfo::new(place, Mat::IDENTITY), &mut list);
    // one particle: half_y 2, half_x = 2 * 0.5 = 1, scale 1, no rotation, emitter at origin
    assert_eq!(list.batches.len(), 1);
    let b = &list.batches[0];
    assert_eq!(b.vertices.len(), 4);
    assert_eq!(b.indices, vec![0, 1, 2, 0, 2, 3]);
    let pos: Vec<[f32; 3]> = b.vertices.iter().map(|v| v.pos).collect();
    assert_eq!(
        pos,
        vec![
            [11.0, -2.0, 0.0],
            [9.0, -2.0, 0.0],
            [9.0, 2.0, 0.0],
            [11.0, 2.0, 0.0]
        ]
    );
    // uv rectangle (u, v) .. (u + 0.5, v + 0.5) in MLRCardCloud corner order
    let uv: Vec<[f32; 2]> = b.vertices.iter().map(|v| v.uv).collect();
    assert_eq!(uv, vec![[0.0, 0.5], [0.5, 0.5], [0.5, 0.0], [0.0, 0.0]]);
    assert_eq!(b.vertices[0].color, [1.0, 0.5, 0.25, 0.75]);
}

#[test]
fn an_animated_card_cloud_picks_its_frame_by_the_index_curve() {
    // 4x4 sheet (size 0.25), frame index 6 = row 1, column 2 -> u = 0.5, v = 0.25
    let mut spec = card_cloud();
    if let SpecKind::CardCloud(c) = &mut spec.kind {
        c.uv.u_size.value = 0.25;
        c.uv.v_size.value = 0.25;
        c.uv.animated = true;
        c.uv.width = 4;
        c.p_index.age_curve.set_constant(6.0);
    }
    let mut rig = Rig::new(vec![spec]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    let mut list = DrawList::default();
    e.draw(&DrawInfo::new(Mat::IDENTITY, Mat::IDENTITY), &mut list);
    let uv: Vec<[f32; 2]> = list.batches[0].vertices.iter().map(|v| v.uv).collect();
    assert_eq!(uv, vec![[0.5, 0.5], [0.75, 0.5], [0.75, 0.25], [0.5, 0.25]]);
}

#[test]
fn a_camera_aligned_card_cloud_faces_the_camera() {
    // Z aligned to the camera about Y only (`alignZUsingY`): the quad's normal points at a
    // camera on the +x axis, so every corner has x == 0 at the card centre.
    let mut spec = card_cloud();
    if let SpecKind::CardCloud(c) = &mut spec.kind {
        c.spin.align_z_using_y = true;
    }
    let mut rig = Rig::new(vec![spec]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    let camera = Mat::from_translation(Vec3::new(100.0, 0.0, 0.0));
    let mut list = DrawList::default();
    e.draw(&DrawInfo::new(Mat::IDENTITY, camera), &mut list);
    for v in &list.batches[0].vertices {
        assert!(v.pos[0].abs() < 1e-4, "x = {}", v.pos[0]);
    }
    // the quad spans z by half_x = 1 and y by half_y = 2
    let zs: Vec<f32> = list.batches[0].vertices.iter().map(|v| v.pos[2]).collect();
    let ys: Vec<f32> = list.batches[0].vertices.iter().map(|v| v.pos[1]).collect();
    assert!(zs.iter().all(|z| close(z.abs(), 1.0)));
    assert!(ys.iter().all(|y| close(y.abs(), 2.0)));
}

fn tube_spec() -> EffectSpec {
    let mut t = TubeSpec {
        max_profile_count: 4,
        profile_type: ProfileType::Ribbon,
        ..TubeSpec::default()
    };
    let (v, uv) = TubeSpec::build_template(ProfileType::Ribbon);
    t.vertices = v;
    t.uvs = uv;
    t.profiles_per_second.value = 4.0;
    t.p_life_span.value = 1.0;
    t.p_scale.age_curve.set_constant(1.0);
    t.p_scale.seed_curve.value = 1.0;
    t.p_alpha.age_curve.set_constant(1.0);
    t.p_u_size.age_curve.value = 1.0;
    t.p_v_size.age_curve.value = 1.0;
    EffectSpec::new(base("tube", 10.0), SpecKind::Tube(Box::new(t)))
}

#[test]
fn a_tube_grows_one_profile_per_birth_up_to_its_cap() {
    let mut rig = Rig::new(vec![tube_spec()]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    // Start sets the accumulator to 1: the first step creates the first two profiles.
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    assert_eq!(e.tube_active(), 2);
    // 4 per second * 0.25 s = 1 birth per step
    rig.run(&mut e, 0.5, Mat::IDENTITY);
    assert_eq!(e.tube_active(), 3);
    rig.run(&mut e, 0.75, Mat::IDENTITY);
    assert_eq!(e.tube_active(), 4);
    // capped at maxProfileCount
    rig.run(&mut e, 1.0, Mat::IDENTITY);
    assert!(e.tube_active() <= 4);
    // a ribbon of 4 profiles: 8 vertices, 2 * (4 - 1) * (2 - 1) = 6 triangles
    rig.run(&mut e, 1.0, Mat::IDENTITY);
    let mut list = DrawList::default();
    e.draw(&DrawInfo::new(Mat::IDENTITY, Mat::IDENTITY), &mut list);
    if e.tube_active() == 4 {
        assert_eq!(list.batches[0].vertices.len(), 8);
        assert_eq!(list.triangle_count(), 6);
    }
}

#[test]
fn a_tube_leaves_old_profiles_where_the_effect_was() {
    // Move the effect +x by 2 per step; the older profile stays put in the tube's own space,
    // so its local x goes negative as the head moves away.
    let mut rig = Rig::new(vec![tube_spec()]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    rig.run(&mut e, 0.25, Mat::IDENTITY);
    rig.run(&mut e, 0.5, Mat::from_translation(Vec3::new(2.0, 0.0, 0.0)));
    let mut list = DrawList::default();
    e.draw(
        &DrawInfo::new(
            Mat::from_translation(Vec3::new(2.0, 0.0, 0.0)),
            Mat::IDENTITY,
        ),
        &mut list,
    );
    // newest profile first: its ribbon is centred on the effect (world x = 2); the oldest was
    // frozen at the origin (world x = 0). Ribbon vertices are at +-1 on x.
    let xs: Vec<f32> = list.batches[0].vertices.iter().map(|v| v.pos[0]).collect();
    let newest: f32 = (xs[0] + xs[1]) * 0.5;
    let oldest: f32 = (xs[xs.len() - 2] + xs[xs.len() - 1]) * 0.5;
    assert!(close(newest, 2.0), "newest centre {newest}");
    assert!(close(oldest, 0.0), "oldest centre {oldest}");
}

fn debris_spec() -> EffectSpec {
    let mut d = DebrisCloudSpec {
        minimum_deviation: Default::default(),
        maximum_deviation: Default::default(),
        starting_speed: Default::default(),
        p_life_span: Default::default(),
        p_ether_velocity_y: Default::default(),
        p_acceleration_y: Default::default(),
        p_drag: Default::default(),
        p_alpha: Default::default(),
        p_spin: Default::default(),
        center_of_force: Vec3::ZERO,
        pieces: vec![DebrisPiece {
            position: Vec3::new(1.0, 0.0, 0.0),
            sphere_center: Vec3::ZERO,
            sphere_radius: 1.0,
            shape: mc2_fx::mlrshape::MlrShape::default(),
            seed: 0.0,
        }],
    };
    d.minimum_deviation.value = 1.0;
    d.maximum_deviation.value = 1.0;
    d.starting_speed.age_curve.set_constant(3.0);
    d.p_life_span.age_curve.set_constant(2.0);
    d.p_alpha.age_curve.set_constant(1.0);
    EffectSpec::new(base("debris", 2.0), SpecKind::DebrisCloud(Box::new(d)))
}

#[test]
fn debris_flies_away_from_the_centre_of_force() {
    // lerp(deviation = 1) takes the direction from "up" to the piece's offset (1, 0, 0), which
    // is normalised and scaled by the speed 3. No drag or gravity: after 0.25 s the piece is
    // at 1 + 3 * 0.25 = 1.75.
    let mut rig = Rig::new(vec![debris_spec()]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    let v = e.debris_velocities();
    assert!(close(v[0].x, 3.0) && close(v[0].y, 0.0) && close(v[0].z, 0.0));
    assert!(rig.run(&mut e, 0.25, Mat::IDENTITY));
    let p = e.debris_positions();
    assert_eq!(p.len(), 1);
    assert!(close(p[0].x, 1.75), "x = {}", p[0].x);
    // the piece lives 2 s: gone at t = 2
    assert!(rig.run(&mut e, 1.0, Mat::IDENTITY));
    assert!(rig.run(&mut e, 2.0, Mat::IDENTITY) || !e.is_executed());
    assert!(e.debris_positions().is_empty());
}

#[test]
fn a_killed_card_is_not_drawn() {
    // Card::Kill turns the quad off immediately; a draw in the same frame shows nothing.
    let mut spec = CardSpec::default();
    spec.half_height.age_curve.value = 1.0;
    spec.aspect_ratio.age_curve.value = 1.0;
    spec.singleton.scale.age_curve.set_constant(1.0);
    spec.singleton.alpha.age_curve.set_constant(1.0);
    spec.uv.u_size.value = 1.0;
    spec.uv.v_size.value = 1.0;
    let mut rig = Rig::new(vec![EffectSpec::new(
        base("card", 1.0),
        SpecKind::Card(Box::new(spec)),
    )]);
    let mut e = rig.effect(0, EXECUTE_FLAG);
    rig.start(&mut e, 0.0, Mat::IDENTITY);
    assert!(rig.run(&mut e, 0.1, Mat::IDENTITY));
    let info = DrawInfo::new(Mat::IDENTITY, Mat::IDENTITY);
    let mut list = DrawList::default();
    e.draw(&info, &mut list);
    assert_eq!(list.triangle_count(), 2);
    e.kill();
    let mut list = DrawList::default();
    e.draw(&info, &mut list);
    assert_eq!(list.triangle_count(), 0);
}
