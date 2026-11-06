// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Jump-landing contact and death from above (`BattleMechType::handleCollision`, mech.cpp:820-948).
//!
//! Every test loads the shipped mc2_01 (needs `MC2_DATA`, skipped without it), places two movers next to each other and drives the
//! production collision reaction `mover::type_handle_collision` the way `CollisionSystem::checkExtents` does (collidee/collider in
//! both orders). Expected damage is `collider tonnage / 10 + 0.5` through `WeaponShotInfo::init` (difficulty scaling included);
//! the expected hit location is drawn from a probe of the same combat function on a copy of the RNG stream.
//!
//! Tolerances: none needed for damage and rotation (the reaction is arithmetic on f32 values the test reads back); the bounce is
//! checked as "moved to a neighbouring cell centre" because the first open neighbour depends on the map.

use mc2_formats::vfs::Vfs;
use mc2_sim::combat::{AttackSource, WeaponShotInfo};
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::{Vec3, Vec3Ext};
use mc2_sim::mission::init::{load_world as init_world, LoadOutcome, LoadSpec};
use mc2_sim::object::{hooks, Kind};
use mc2_sim::rng::RngMode;
use mc2_sim::save::{load_world, save_world};

const AIRFIELD: (f32, f32) = (2981.0, -744.0);
const NOW: f32 = 100.0;

fn vfs() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty())?;
    Some(Vfs::from_system_cfg(std::path::Path::new(&root)).expect("system.cfg"))
}

fn init(vfs: &mut Vfs, seed: u32) -> LoadOutcome {
    init_world(
        vfs,
        &LoadSpec::quickstart("mc2_01", seed, RngMode::LegacyMsvc),
    )
    .unwrap()
}

fn movers(w: &mc2_sim::World, mech: bool) -> Vec<ObjectHandle> {
    (1..3000)
        .filter_map(|p| w.objects.find_by_part_id(mc2_sim::ids::PartId(p)))
        .filter(|&h| match &w.objects.get(h).unwrap().kind {
            Kind::Mech(_) => mech,
            Kind::Vehicle(_) => !mech,
            _ => false,
        })
        .collect()
}

/// Two mechs (or a mech and a vehicle) of the wanted alignment, the first one the jumper.
fn pair(w: &mc2_sim::World, enemy: bool, vehicle_target: bool) -> (ObjectHandle, ObjectHandle) {
    let mechs = movers(w, true);
    let targets = if vehicle_target {
        movers(w, false)
    } else {
        mechs.clone()
    };
    for &a in &mechs {
        for &b in &targets {
            if a != b && hooks::is_enemy(w, a, b) == enemy && hooks::is_friendly(w, a, b) != enemy {
                return (a, b);
            }
        }
    }
    panic!("no pair enemy={enemy} vehicle={vehicle_target}");
}

fn place(w: &mut mc2_sim::World, h: ObjectHandle, x: f32, y: f32) {
    mc2_sim::mover::set_position(w, h, Vec3::from_xyz(x, y, 0.0));
}

/// `armor + internal structure` of a mover, and its per-location armor.
fn health(w: &mc2_sim::World, h: ObjectHandle) -> (f32, Vec<f32>) {
    let l = match &w.objects.get(h).unwrap().kind {
        Kind::Mech(m) => &m.loadout,
        Kind::Vehicle(v) => &v.loadout,
        _ => unreachable!(),
    };
    let armor: Vec<f32> = l.armor.iter().map(|a| a.0).collect();
    let total =
        armor.iter().sum::<f32>() + l.body.iter().map(|b| b.cur_internal_structure).sum::<f32>();
    (total, armor)
}

fn set_jump(w: &mut mc2_sim::World, h: ObjectHandle, in_flight: bool) {
    let Kind::Mech(m) = &mut w.objects.get_mut(h).unwrap().kind else {
        panic!("not a mech")
    };
    if in_flight {
        m.in_jump = true;
    } else {
        m.in_jump = false;
        m.last_jump_time = NOW - 0.1;
    }
}

fn last_jump_time(w: &mc2_sim::World, h: ObjectHandle) -> f32 {
    match &w.objects.get(h).unwrap().kind {
        Kind::Mech(m) => m.last_jump_time,
        _ => unreachable!(),
    }
}

fn rotation(w: &mc2_sim::World, h: ObjectHandle) -> f32 {
    w.objects.get(h).unwrap().c.rotation
}

fn position(w: &mc2_sim::World, h: ObjectHandle) -> Vec3 {
    w.objects.get(h).unwrap().c.position
}

fn tonnage(w: &mc2_sim::World, h: ObjectHandle) -> f32 {
    w.objects.get(h).unwrap().c.tonnage
}

fn wrap(a: f32) -> f32 {
    let mut a = a;
    if a < -180.0 {
        a += 360.0;
    } else if a > 180.0 {
        a -= 360.0;
    }
    a
}

/// The hit locations the jumper's handler will draw, in its order: the jumper `j` hit by `t` from the mine table after the
/// 90-degree rotate, then `t` hit by `j` from the DFA table. Probes on the live world and restores the rotation and the RNG.
fn expected_locations(w: &mut mc2_sim::World, j: ObjectHandle, t: ObjectHandle) -> (i32, i32) {
    let saved = w.rng.0.borrow().clone();
    let old = rotation(w, j);
    w.objects.get_mut(j).unwrap().c.rotation = wrap(old + 90.0);
    let loc_j = mc2_sim::combat::calc_hit_location(w, j, t, -1, AttackSource::Mine, 0);
    w.objects.get_mut(j).unwrap().c.rotation = old;
    let loc_t = mc2_sim::combat::calc_hit_location(w, t, j, -1, AttackSource::Dfa, 0);
    *w.rng.0.borrow_mut() = saved;
    (loc_j, loc_t)
}

fn expected_damage(w: &mc2_sim::World, attacker: ObjectHandle, from: ObjectHandle) -> f32 {
    let wid = w.objects.watch_id_peek(attacker);
    WeaponShotInfo::init(w, wid, -1, tonnage(w, from) / 10.0 + 0.5, 0, 0.0).damage
}

fn assert_bounced(w: &mc2_sim::World, h: ObjectHandle, from: Vec3) {
    let now = position(w, h);
    assert!(
        (now.x - from.x).abs() + (now.y - from.y).abs() > 1.0,
        "mover was not displaced"
    );
    let a = w.terrain.world_to_cell(&from);
    let b = w.terrain.world_to_cell(&now);
    assert!(
        (a.0 - b.0).abs() <= 1 && (a.1 - b.1).abs() <= 1,
        "bounce left the adjacent cells: {a:?} -> {b:?}"
    );
}

struct Scene {
    w: mc2_sim::World,
    jumper: ObjectHandle,
    target: ObjectHandle,
}

fn scene(enemy: bool, vehicle_target: bool, in_flight: bool) -> Option<Scene> {
    let mut v = vfs()?;
    let mut w = init(&mut v, 1).world;
    let (jumper, target) = pair(&w, enemy, vehicle_target);
    w.clock.scenario_time = NOW;
    w.objects.watch_id(jumper);
    w.objects.watch_id(target);
    place(&mut w, jumper, AIRFIELD.0, AIRFIELD.1);
    place(&mut w, target, AIRFIELD.0 + 20.0, AIRFIELD.1);
    set_jump(&mut w, jumper, in_flight);
    Some(Scene { w, jumper, target })
}

fn contact(w: &mut mc2_sim::World, collidee: ObjectHandle, collider: ObjectHandle) {
    mc2_sim::mover::type_handle_collision(w, collidee, collider);
}

/// An enemy mech lands on another mover, in flight or within 0.5 s of landing, whichever mover's handler the collision system
/// calls first: death from above is applied exactly once to each side, at the C++ location, and the mechs are displaced.
#[test]
fn enemy_landing_applies_death_from_above_once_and_displaces() {
    for in_flight in [false, true] {
        for jumper_first in [true, false] {
            let Some(mut s) = scene(true, false, in_flight) else {
                return;
            };
            let (j, t) = (s.jumper, s.target);
            let (j0, t0) = (position(&s.w, j), position(&s.w, t));
            let (jr0, tr0) = (rotation(&s.w, j), rotation(&s.w, t));
            let (jh0, ja0) = health(&s.w, j);
            let (th0, ta0) = health(&s.w, t);
            let dmg_j = expected_damage(&s.w, t, t); // collider = target: the jumper takes the target's tonnage
            let dmg_t = expected_damage(&s.w, j, t);
            let step = |w: &mut mc2_sim::World| {
                if jumper_first {
                    contact(w, j, t);
                    contact(w, t, j);
                } else {
                    contact(w, t, j);
                    contact(w, j, t);
                }
            };
            if !jumper_first {
                contact(&mut s.w, t, j); // displaces the target, draws nothing
            }
            let (loc_j, loc_t) = expected_locations(&mut s.w, j, t);
            contact(&mut s.w, j, t);
            contact(&mut s.w, t, j);
            let tag = format!("in_flight={in_flight} jumper_first={jumper_first}");

            let (jh1, ja1) = health(&s.w, j);
            let (th1, ta1) = health(&s.w, t);
            // exactly one hit each: the whole armor + structure loss is one shot's damage, in the C++ location
            assert_eq!(jh0 - jh1, dmg_j, "{tag}: jumper loss");
            assert_eq!(th0 - th1, dmg_t, "{tag}: target loss");
            assert!(dmg_j > 0.0 && dmg_t > 0.0);
            for (i, (a, b)) in ja0.iter().zip(&ja1).enumerate() {
                let want = if i as i32 == loc_j { dmg_j } else { 0.0 };
                assert_eq!(a - b, want, "{tag}: jumper armor {i}");
            }
            for (i, (a, b)) in ta0.iter().zip(&ta1).enumerate() {
                let want = if i as i32 == loc_t { dmg_t } else { 0.0 };
                assert_eq!(a - b, want, "{tag}: target armor {i}");
            }
            // the jumper is turned 90 degrees and bounced to a neighbouring cell
            assert_eq!(rotation(&s.w, j), wrap(jr0 + 90.0), "{tag}: jumper rotate");
            assert_bounced(&s.w, j, j0);
            if in_flight || !jumper_first {
                // the target's own handler saw the jump (the jumper in flight, or its landing time not yet spent)
                assert_eq!(rotation(&s.w, t), wrap(tr0 + 90.0), "{tag}: target rotate");
                assert_bounced(&s.w, t, t0);
            }

            // the same landing again, in the same window: the two-second guard blocks every further hit and move
            let after = (
                position(&s.w, j),
                position(&s.w, t),
                rotation(&s.w, j),
                rotation(&s.w, t),
            );
            s.w.clock.scenario_time = NOW + 0.2;
            step(&mut s.w);
            assert_eq!(health(&s.w, j).0, jh1, "{tag}: second hit on the jumper");
            assert_eq!(health(&s.w, t).0, th1, "{tag}: second hit on the target");
            assert_eq!(
                after,
                (
                    position(&s.w, j),
                    position(&s.w, t),
                    rotation(&s.w, j),
                    rotation(&s.w, t)
                ),
                "{tag}: second displacement"
            );
        }
    }
}

/// A same-team landing does no damage but still rotates and bounces the mech that landed.
#[test]
fn same_team_landing_does_no_damage_and_displaces() {
    for in_flight in [false, true] {
        let Some(mut s) = scene(false, false, in_flight) else {
            return;
        };
        let (j, t) = (s.jumper, s.target);
        let j0 = position(&s.w, j);
        let jr0 = rotation(&s.w, j);
        let (jh0, _) = health(&s.w, j);
        let (th0, _) = health(&s.w, t);
        contact(&mut s.w, j, t);
        contact(&mut s.w, t, j);
        assert_eq!(health(&s.w, j).0, jh0, "friendly jumper damaged");
        assert_eq!(health(&s.w, t).0, th0, "friendly target damaged");
        assert_eq!(rotation(&s.w, j), wrap(jr0 + 90.0));
        assert_bounced(&s.w, j, j0);
    }
}

/// Two enemy mechs that merely brush (no jump, no ram order) do not collide: no damage, no turn.
#[test]
fn enemy_walking_contact_without_ram_does_nothing() {
    let Some(mut s) = scene(true, false, false) else {
        return;
    };
    let (j, t) = (s.jumper, s.target);
    s.w.clock.scenario_time = NOW + 5.0; // the landing window is over
    let (jh0, _) = health(&s.w, j);
    let jr0 = rotation(&s.w, j);
    contact(&mut s.w, j, t);
    contact(&mut s.w, t, j);
    assert_eq!(health(&s.w, j).0, jh0);
    assert_eq!(rotation(&s.w, j), jr0);
}

fn set_path_locks(w: &mut mc2_sim::World, h: ObjectHandle, on: bool) {
    let Kind::Vehicle(v) = &mut w.objects.get_mut(h).unwrap().kind else {
        panic!("not a vehicle")
    };
    v.m.path_locks_enabled = on;
}

/// A ground vehicle as the mover that is landed on (path locks off, "something steppable"): it takes the death-from-above hit
/// from the jumper's handler plus its own `GroundVehicleType::handleCollision` hit of `(long)(tonnage / 10 + 0.5)`, rotates 90
/// degrees and bounces; the mech takes the vehicle's `tonnage / 10 + 0.5`. A repeat contact inside two seconds does nothing more.
#[test]
fn landing_on_a_ground_vehicle_hits_both() {
    for in_flight in [false, true] {
        let Some(mut s) = scene(true, true, in_flight) else {
            return;
        };
        let (j, t) = (s.jumper, s.target);
        set_path_locks(&mut s.w, t, false);
        let (jh0, _) = health(&s.w, j);
        let (th0, _) = health(&s.w, t);
        let dmg_j = expected_damage(&s.w, t, t);
        let dfa = expected_damage(&s.w, j, t);
        let wid = s.w.objects.watch_id_peek(j);
        let crush = WeaponShotInfo::init(
            &s.w,
            wid,
            -1,
            (tonnage(&s.w, j) / 10.0 + 0.5).floor(),
            0,
            0.0,
        )
        .damage;
        assert!(dfa > 0.0 && crush > 0.0);
        let (j0, t0, tr0) = (position(&s.w, j), position(&s.w, t), rotation(&s.w, t));
        contact(&mut s.w, j, t);
        contact(&mut s.w, t, j);
        let (jh1, _) = health(&s.w, j);
        let (th1, _) = health(&s.w, t);
        assert_eq!(jh0 - jh1, dmg_j, "in_flight={in_flight}: mech loss");
        assert_eq!(
            th0 - th1,
            dfa + crush,
            "in_flight={in_flight}: vehicle loss"
        );
        assert_eq!(rotation(&s.w, t), wrap(tr0 + 90.0));
        assert_bounced(&s.w, j, j0);
        assert_bounced(&s.w, t, t0);
        // the guards hold: the same contacts again change nothing
        let after = (health(&s.w, j).0, health(&s.w, t).0, position(&s.w, t));
        contact(&mut s.w, j, t);
        contact(&mut s.w, t, j);
        assert_eq!(
            after,
            (health(&s.w, j).0, health(&s.w, t).0, position(&s.w, t))
        );
    }
}

/// A vehicle with path locks that is not a ram target is not reacting to a brush from an enemy that is not jumping.
#[test]
fn vehicle_with_path_locks_ignores_a_plain_brush() {
    let Some(mut s) = scene(true, true, false) else {
        return;
    };
    let (j, t) = (s.jumper, s.target);
    set_path_locks(&mut s.w, t, true);
    s.w.clock.scenario_time = NOW + 5.0;
    let (th0, _) = health(&s.w, t);
    let tr0 = rotation(&s.w, t);
    contact(&mut s.w, t, j);
    assert_eq!(health(&s.w, t).0, th0);
    assert_eq!(rotation(&s.w, t), tr0);
}

#[test]
fn visible_jump_contact_sound_uses_the_post_bounce_position() {
    let Some(mut s) = scene(true, false, false) else {
        return;
    };
    let (jumper, target) = (s.jumper, s.target);
    let before = position(&s.w, jumper);
    s.w.events.clear();
    contact(&mut s.w, jumper, target);
    let after = position(&s.w, jumper);
    assert_bounced(&s.w, jumper, before);
    assert!(
        s.w.events.iter().any(|event| matches!(
            event,
            mc2_sim::events::SimEvent::Sound { id: 4, at: Some(at), allow_dupes: false, .. }
                if *at == after
        )),
        "MECH_COLLIDE must be emitted at the bounced position {after:?}"
    );
}

/// A save in the middle of the 0.5 s window keeps the guard: the loaded world takes no second hit and moves nothing, even when the
/// landing time is put back inside the window by hand (the guard alone must hold).
#[test]
fn save_load_inside_the_window_keeps_the_guard() {
    let Some(mut v) = vfs() else { return };
    let Some(mut s) = scene(true, false, false) else {
        return;
    };
    let (j, t) = (s.jumper, s.target);
    contact(&mut s.w, j, t);
    contact(&mut s.w, t, j);
    let (jh1, _) = health(&s.w, j);
    let (th1, _) = health(&s.w, t);
    assert_eq!(last_jump_time(&s.w, j), -1.0, "landing time is spent");
    s.w.clock.scenario_time = NOW + 0.3; // still inside the original window
    let bytes = save_world(&s.w);
    let base = init(&mut v, 2).world;
    let mut loaded = load_world(base, Some(&mut v), &bytes).expect("load");
    assert_eq!(save_world(&loaded), bytes, "re-save differs");
    let (lj, lt) = (j, t);
    assert_eq!(health(&loaded, lj).0, jh1);
    assert_eq!(health(&loaded, lt).0, th1);
    let after = (
        position(&loaded, lj),
        position(&loaded, lt),
        rotation(&loaded, lj),
        rotation(&loaded, lt),
    );
    for restore_window in [false, true] {
        if restore_window {
            set_jump(&mut loaded, lj, false);
            loaded.clock.scenario_time = NOW + 0.3;
            let Kind::Mech(m) = &mut loaded.objects.get_mut(lj).unwrap().kind else {
                unreachable!()
            };
            m.last_jump_time = NOW + 0.2;
        }
        contact(&mut loaded, lj, lt);
        if !restore_window {
            contact(&mut loaded, lt, lj);
        }
        assert_eq!(health(&loaded, lj).0, jh1, "second hit on the jumper");
        assert_eq!(health(&loaded, lt).0, th1, "second hit on the target");
        assert_eq!(
            after,
            (
                position(&loaded, lj),
                position(&loaded, lt),
                rotation(&loaded, lj),
                rotation(&loaded, lt)
            ),
            "restore_window={restore_window}: displaced again"
        );
    }
}
