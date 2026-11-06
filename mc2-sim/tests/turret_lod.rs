// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Turret updates do not need a view-supplied structure LOD (turret.cpp:801-826). `recalcBounds` picks the LOD from the camera
//! (bdactor.cpp:1305-1325) but the turret's gesture state machine, targeting, rotation and weapon fire do not read which LOD
//! shape was picked, so a headless run (no camera: `Visibility::structure_lod` is `None`) updates turrets exactly as a viewed
//! run does. Needs `MC2_DATA` and skips without it.

mod common;

use std::rc::Rc;

use common::fixture;
use mc2_sim::ai::frame_unavailable;
use mc2_sim::ids::{ObjectHandle, PartId};
use mc2_sim::math::Vec3;
use mc2_sim::object::{hooks, structure, Kind, ObjectClass};
use mc2_sim::time::FixedStep;
use mc2_sim::visibility::{AllVisible, Visibility};
use mc2_sim::World;
use mc2_sim_data::objtype::ObjectTypeData;

/// A frame input with everything in view and the given structure LOD for every structure.
struct Lod(Option<usize>);

impl Visibility for Lod {
    fn block_active(&self, _block: u32) -> bool {
        true
    }
    fn vertex_active(&self, _vertex: u32) -> bool {
        true
    }
    fn mover_in_view(&self, _part: PartId) -> bool {
        true
    }
    fn structure_lod(&self, _handle: ObjectHandle) -> Option<usize> {
        self.0
    }
}

/// What a run of turret updates produced.
#[derive(Debug, PartialEq)]
struct Outcome {
    rotated: bool,
    facing_target: bool,
    bolts: usize,
    ready_set: bool,
    unavailable: usize,
}

const MISSIONS: [&str; 5] = ["mc2_05", "mc2_17", "mc2_03", "mc2_08", "mc2_10"];

fn weapon_turret(world: &World) -> Option<ObjectHandle> {
    world.objects.turrets.iter().copied().find(|&h| {
        world.objects.get(h).is_some_and(|o| o.exists())
            && matches!(&world.objects.type_of(h).map(|t| &t.data),
                Some(ObjectTypeData::Turret(t)) if t.weapon_master_id[0] != -1)
    })
}

/// Run the scenario for `mission` under `vis`: the first turret with a weapon, an unfriendly mech placed in the middle of the
/// weapon's range band where the turret has a line of sight, the target set, then `frames` production turret updates.
fn run(mission: &str, vis: Rc<dyn Visibility>) -> Option<Option<Outcome>> {
    let mut fx = fixture(mission, "msvc")?;
    let fps = fx.fps;
    let w = &mut fx.world;
    let Some(turret) = weapon_turret(w) else {
        return Some(None);
    };
    let weapon_master = match &w.objects.type_of(turret).unwrap().data {
        ObjectTypeData::Turret(t) => t.weapon_master_id[0],
        _ => unreachable!(),
    };
    let target = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&m| {
            w.objects.get(m).unwrap().c.class == ObjectClass::BattleMech
                && !hooks::is_friendly(w, turret, m)
        })
        .or_else(|| {
            w.objects
                .movers
                .iter()
                .copied()
                .find(|&m| w.objects.get(m).unwrap().c.class == ObjectClass::BattleMech)
        })?;
    let class = w
        .data
        .components
        .as_ref()?
        .get(weapon_master)
        .and_then(|c| c.weapon())
        .and_then(|ws| ws.range)
        .map_or(0, |r| r.index());
    let band = w.data.gamesys.tables.as_ref()?.general.weapon_ranges[class];
    let tpos = w.objects.get(turret).unwrap().c.position;
    let mut placed = false;
    'search: for bearing in 0..16 {
        let ang = bearing as f32 * std::f32::consts::TAU / 16.0;
        for step in 1..60 {
            let meters = band[0] + 0.5 + step as f32 * 2.0;
            if meters >= band[1] {
                break;
            }
            let reach = meters / mc2_sim::terrain::METERS_PER_WORLD_UNIT;
            let mut p = Vec3 {
                x: tpos.x + reach * ang.cos(),
                y: tpos.y + reach * ang.sin(),
                z: 0.0,
            };
            p.z = w.terrain.elevation(&p);
            let o = w.objects.get_mut(target).unwrap();
            o.c.position = p;
            if let Kind::Mech(m) = &mut o.kind {
                m.m.velocity = Vec3::default();
            }
            hooks::set_object_parameters(w, target);
            mc2_sim::appearance::update(w, target, false);
            let r = hooks::appear_radius(w, turret);
            w.objects.mover_los_table.iter_mut().for_each(|v| *v = -1);
            if hooks::line_of_sight(w, turret, target, r) {
                placed = true;
                break 'search;
            }
        }
    }
    if !placed {
        return Some(None);
    }
    w.visibility = vis;
    w.rng.0.borrow_mut().sim.seed(7);
    let target_id = w.objects.watch_id(target);
    let rot0 = match &w.objects.get(turret).unwrap().kind {
        Kind::Turret(t) => t.turret_rotation,
        _ => unreachable!(),
    };
    {
        let o = w.objects.get_mut(turret).unwrap();
        o.c.set_awake(true);
        if let Kind::Turret(t) = &mut o.kind {
            t.target = target_id;
        }
    }
    let weapons_before = w
        .objects
        .weapons
        .iter()
        .filter(|&&b| w.objects.get(b).unwrap().exists())
        .count();
    let mut unavailable = 0;
    for i in 1..=900u32 {
        w.clock
            .begin_frame(fps as f32, FixedStep { fps }.now_ms(i as u64), false);
        structure::update_turret(w, turret);
        unavailable += frame_unavailable(w).len();
        w.combat_unavailable_frame.clear();
    }
    let (rot1, ready, facing) = match &w.objects.get(turret).unwrap().kind {
        Kind::Turret(t) => (
            t.turret_rotation,
            t.ready_time[0] > 0.0,
            w.objects
                .get(turret)
                .unwrap()
                .c
                .flag(mc2_sim::object::flags::FACING_TARGET),
        ),
        _ => unreachable!(),
    };
    let bolts = w
        .objects
        .weapons
        .iter()
        .filter(|&&b| w.objects.get(b).unwrap().exists())
        .count()
        - weapons_before;
    Some(Some(Outcome {
        rotated: rot1 != rot0,
        facing_target: facing,
        bolts,
        ready_set: ready,
        unavailable,
    }))
}

/// A model-less turret (no structure LOD input at all, the default `AllVisible` frame input) acquires its target's bearing,
/// rotates onto it and fires, with nothing reported unavailable; a view-supplied LOD gives the same outcome.
#[test]
fn modelless_turret_tracks_and_fires_like_one_with_a_view_lod() {
    for mission in MISSIONS {
        let Some(headless) = run(mission, Rc::new(AllVisible)) else {
            return;
        };
        let Some(headless) = headless else {
            continue;
        };
        assert_eq!(headless.unavailable, 0, "{mission}: nothing unavailable");
        assert!(
            headless.rotated,
            "{mission}: turret rotated onto its target"
        );
        assert!(headless.facing_target, "{mission}: turret faces its target");
        assert!(
            headless.ready_set,
            "{mission}: weapon fired (recycle armed)"
        );
        assert!(headless.bolts >= 1, "{mission}: a bolt was created");
        let viewed = run(mission, Rc::new(Lod(Some(0)))).unwrap().unwrap();
        assert_eq!(
            viewed, headless,
            "{mission}: view LOD gives the same outcome"
        );
        return;
    }
    panic!("no shipped mission offered a turret with a weapon and a mech in line of sight");
}

/// A headless mission with turrets (the default `AllVisible` frame input supplies no structure LOD): the production turret
/// update of every turret runs for a few frames and no turret reports an unavailable LOD.
#[test]
fn headless_mission_with_turrets_reports_no_lod_unavailable() {
    let Some(mut fx) = fixture("mc2_05", "msvc") else {
        return;
    };
    let fps = fx.fps;
    let w = &mut fx.world;
    w.visibility = Rc::new(AllVisible);
    let turrets: Vec<ObjectHandle> = w.objects.turrets.clone();
    assert!(!turrets.is_empty(), "mc2_05 has turrets");
    for i in 1..=30u32 {
        w.clock
            .begin_frame(fps as f32, FixedStep { fps }.now_ms(i as u64), false);
        for &h in &turrets {
            if w.objects.get(h).is_some_and(|o| o.exists()) {
                structure::update_turret(w, h);
            }
        }
        assert!(
            frame_unavailable(w).is_empty(),
            "frame {i}: nothing unavailable"
        );
    }
    let updated = turrets
        .iter()
        .filter(|&&h| match w.objects.get(h).map(|o| &o.kind) {
            Some(Kind::Turret(t)) => t.did_reveal,
            _ => false,
        })
        .count();
    assert!(updated > 0, "turrets updated headlessly");
}
