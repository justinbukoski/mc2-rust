// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The `FxSystem` API: spawn by name, attach to a moving anchor, step, draw; and a smoke run of
//! every effect in the shipped library. Data tests skip without `MC2_DATA`.

use mc2_formats::vfs::Vfs;
use mc2_fx::draw::DrawList;
use mc2_fx::library::EffectLibrary;
use mc2_fx::math::{Mat, Vec3};
use mc2_fx::spec::*;
use mc2_fx::system::*;
use mc2_fx::weaponfx::WeaponEffects;

fn tiny_system() -> FxSystem {
    // "puff": a one-card cloud that lives 1 s; "trail": the same, looping by flag.
    let mut c = CardCloudSpec::default();
    c.spin.particle.particles_per_second.set_constant(10.0);
    c.spin.particle.p_life_span.age_curve.set_constant(0.5);
    c.spin.particle.max_particle_count = 8;
    c.spin.p_scale.age_curve.set_constant(1.0);
    c.half_height.age_curve.set_constant(1.0);
    c.aspect_ratio.age_curve.set_constant(1.0);
    c.uv.u_size.value = 1.0;
    c.uv.v_size.value = 1.0;
    c.uv.width = 1;
    let mut base = BaseSpec {
        name: "Puff".into(),
        ..BaseSpec::default()
    };
    base.life_span.value = 1.0;
    let puff = EffectSpec::new(base, SpecKind::CardCloud(Box::new(c)));
    let lib = EffectLibrary::from_specs(17, vec![puff]);
    let csv = "3,Effect,Muzzle,Hit,Obj,Miss,Weapon\r\n1,puff,NONE,NONE,0,NONE,w\r\n2,NONE,puff,puff,0,puff,w2\r\n3,NONE,NONE,NONE,0,NONE,pad\r\n";
    let weapons = WeaponEffects::from_csv_bytes(csv.as_bytes()).unwrap();
    FxSystem::new(lib, weapons, 5)
}

#[test]
fn spawn_by_name_is_case_insensitive_and_none_is_refused() {
    let mut fx = tiny_system();
    assert!(fx.spawn("PUFF", SpawnParams::at(Mat::IDENTITY)).is_some());
    assert!(fx
        .spawn("nothing", SpawnParams::at(Mat::IDENTITY))
        .is_none());
    assert!(fx.spawn("NONE", SpawnParams::at(Mat::IDENTITY)).is_none());
    assert_eq!(fx.active_count(), 1);
}

#[test]
fn a_one_shot_effect_removes_itself_and_its_handle_goes_stale() {
    let mut fx = tiny_system();
    let h = fx.spawn("puff", SpawnParams::at(Mat::IDENTITY)).unwrap();
    assert!(fx.is_alive(h));
    fx.update(0.5);
    assert!(fx.is_alive(h));
    // life span 1 s, particles live 0.5 s: after t = 2 everything has run out
    for step in 1..=8 {
        fx.update(0.5 + step as f64 * 0.25);
    }
    assert!(!fx.is_alive(h));
    assert_eq!(fx.active_count(), 0);
    // a new effect reuses the slot; the old handle must not see it
    let h2 = fx.spawn("puff", SpawnParams::at(Mat::IDENTITY)).unwrap();
    assert!(fx.is_alive(h2));
    assert!(!fx.is_alive(h));
}

#[test]
fn a_looped_trail_runs_until_killed() {
    let mut fx = tiny_system();
    let h = fx
        .spawn_weapon(0, WeaponFx::Trail, SpawnParams::at(Mat::IDENTITY))
        .unwrap();
    for step in 1..=40 {
        fx.update(step as f64 * 0.25);
    }
    assert!(fx.is_alive(h));
    fx.kill(h);
    fx.update(11.0);
    assert!(!fx.is_alive(h));
}

#[test]
fn weapon_slots_read_the_matching_csv_column() {
    let mut fx = tiny_system();
    // id 0 (CSV row 2): trail is puff, muzzle NONE; id 1 (row 3): only muzzle/hit/miss
    assert!(fx
        .spawn_weapon(0, WeaponFx::MuzzleFlash, SpawnParams::at(Mat::IDENTITY))
        .is_none());
    assert!(fx
        .spawn_weapon(1, WeaponFx::Trail, SpawnParams::at(Mat::IDENTITY))
        .is_none());
    assert!(fx
        .spawn_weapon(1, WeaponFx::Hit, SpawnParams::at(Mat::IDENTITY))
        .is_some());
    assert!(fx
        .spawn_weapon(99, WeaponFx::Hit, SpawnParams::at(Mat::IDENTITY))
        .is_none());
}

#[test]
fn an_attached_effect_follows_its_anchor() {
    let mut fx = tiny_system();
    let h = fx
        .spawn(
            "puff",
            SpawnParams::at(Mat::IDENTITY)
                .looped()
                .attached(7, Mat::from_translation(Vec3::new(0.0, 1.0, 0.0))),
        )
        .unwrap();
    fx.set_anchor(7, Mat::from_translation(Vec3::new(10.0, 0.0, 0.0)));
    fx.update(0.25);
    let mut list = DrawList::default();
    fx.draw(&Mat::IDENTITY, &mut list);
    let xs: Vec<f32> = list.batches[0].vertices.iter().map(|v| v.pos[0]).collect();
    let cx = xs.iter().sum::<f32>() / xs.len() as f32;
    assert!((cx - 10.0).abs() < 1e-3, "card centre x = {cx}");
    // the anchor moves; the next frame follows
    fx.set_anchor(7, Mat::from_translation(Vec3::new(20.0, 0.0, 0.0)));
    fx.update(0.5);
    let (centre, _) = fx.bounds(h).unwrap();
    assert!((centre.x - 20.0).abs() < 2.0, "bounds centre {centre:?}");
    // detach: no longer follows
    fx.detach(h);
    fx.set_anchor(7, Mat::from_translation(Vec3::new(99.0, 0.0, 0.0)));
    fx.update(0.75);
    let (centre, _) = fx.bounds(h).unwrap();
    assert!(centre.x < 50.0);
}

#[test]
fn time_never_runs_backwards() {
    let mut fx = tiny_system();
    fx.update(5.0);
    fx.update(3.0);
    assert_eq!(fx.time(), 5.0);
}

#[test]
fn game_frame_helpers_match_the_c_plus_plus_call_sites() {
    // weaponbolt.cpp: x = -position.x; y = position.z; z = position.y
    assert_eq!(game_to_fx(1.0, 2.0, 3.0), Vec3::new(-1.0, 3.0, 2.0));
    // Effect_Into_Motion (1,2) = 1, (2,1) = -1: the effect's +Z becomes the object's +Y... with
    // no yaw the placement is the matrix itself plus the translation.
    let m = FxTransform::into_motion(Vec3::new(5.0, 6.0, 7.0), 0.0);
    assert_eq!(m.translation(), Vec3::new(5.0, 6.0, 7.0));
    assert_eq!(m.get(1, 2), 1.0);
    assert_eq!(m.get(2, 1), -1.0);
    let a = FxTransform::against_motion(Vec3::ZERO, 0.0);
    assert_eq!(a.get(1, 2), -1.0);
    assert_eq!(a.get(2, 1), 1.0);
}

fn open() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA")?;
    Vfs::from_system_cfg(root).ok()
}

#[test]
fn every_shipped_effect_runs_and_draws_finite_geometry() {
    let Some(mut vfs) = open() else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let mut fx = FxSystem::from_vfs(&mut vfs, 1).unwrap();
    let n = fx.library().len();
    let names: Vec<String> = fx
        .library()
        .specs()
        .iter()
        .map(|s| s.name().to_string())
        .collect();
    let mut drew = 0usize;
    let mut max_tris = 0usize;
    for (i, name) in names.iter().enumerate() {
        // names can repeat; spawn by index through the first match is enough for coverage, the
        // duplicates are spawned by their own spec below
        let spec = fx.library().spec(i).unwrap().clone();
        let _ = name;
        let h = fx.spawn_spec(
            spec,
            SpawnParams::at(Mat::from_translation(Vec3::new(0.0, 1.0, 0.0))),
        );
        let mut saw = false;
        let mut t = fx.time();
        for _ in 0..60 {
            t += 0.05;
            fx.update(t);
            let mut list = DrawList::default();
            fx.draw(
                &Mat::from_translation(Vec3::new(0.0, 50.0, 200.0)),
                &mut list,
            );
            for b in &list.batches {
                for v in &b.vertices {
                    assert!(
                        v.pos
                            .iter()
                            .chain(v.uv.iter())
                            .chain(v.color.iter())
                            .all(|f| f.is_finite()),
                        "effect {i} '{name}' produced a non-finite vertex {v:?}"
                    );
                }
                for &ix in &b.indices {
                    assert!(
                        (ix as usize) < b.vertices.len(),
                        "effect {i} '{name}': bad index"
                    );
                }
            }
            if list.triangle_count() > 0 {
                saw = true;
                max_tris = max_tris.max(list.triangle_count());
            }
            if !fx.is_alive(h) {
                break;
            }
        }
        if saw {
            drew += 1;
        }
        fx.kill(h);
        t += 0.05;
        fx.update(t);
        assert!(!fx.is_alive(h), "effect {i} '{name}' survived a kill");
    }
    eprintln!("{drew} of {n} effects drew geometry; max {max_tris} triangles in a frame");
    // Many entries are pure containers of child events; the rest must draw.
    assert!(drew > 300, "only {drew} effects drew anything");
    assert_eq!(fx.active_count(), 0);
}
