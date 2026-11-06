// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The support-vehicle VTOL end to end through `Sim::step` on a shipped mission (RC slice S8, `updateVTol`, missiongui.cpp:918):
//! a bought repair truck, scout, mine layer or guard tower is dropped at the clicked point when the shipped `vtol` animation
//! passes frame 145 and powers up at frame 300; the recovery craft (Karnov) lands, hovers for `RecoverTime`, recovers a disabled mech
//! for the home team with a pilot from the campaign roster and flies away; a recovery without a pilot returns the cost and says so.
//!
//! Declared test inputs: the mech that is recovered is disabled by a state write; the campaign roster is a stub naming a shipped
//! pilot file (`LogisticsData::getBestPilot` is the host's input). Tolerance: the drop and departure times within two frames of
//! `frame / 30` seconds of the shipped animation.

mod common;

use std::rc::Rc;

use mc2_sim::command::PlayerCommand;
use mc2_sim::events::{fx_id, FxAction, FxEffect, FxSlot, SimEvent};
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::Vec3;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::mission::trade::{RecoveryPilot, RecoveryPilots};
use mc2_sim::mission::vtol;
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim};

const FPS: u32 = 30;

fn load() -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_03", seed, mode))
        .expect("mc2_03 loads");
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    let mut sim = Sim::new(outcome);
    sim.world.objects.use_non_weapon_effects = true;
    vtol::prepare(&mut sim.world, &mut vfs)
        .expect("the support vehicles and the VTOL animations load");
    Some(sim)
}

struct Roster(String);

impl RecoveryPilots for Roster {
    fn best_pilot(&self, _tonnage: i64, _in_use: &dyn Fn(&str) -> bool) -> Option<RecoveryPilot> {
        Some(RecoveryPilot {
            file: self.0.clone(),
            skills: None,
        })
    }

    fn has_pilot(&self, _in_use: &dyn Fn(&str) -> bool) -> bool {
        true
    }
}

struct Run {
    sim: Sim,
    frame: u64,
    events: Vec<(u64, SimEvent)>,
    unavailable: Vec<String>,
}

impl Run {
    fn step(&mut self, commands: Vec<PlayerCommand>) {
        self.frame += 1;
        let out = self
            .sim
            .step(FrameInput::fixed(
                FPS,
                self.frame,
                Rc::new(AllVisible),
                commands,
            ))
            .expect("step");
        for e in out.events {
            self.events.push((self.frame, e));
        }
        for u in out.unavailable {
            self.unavailable.push(format!("{u:?}"));
        }
    }
    fn run(&mut self, seconds: u32) {
        for _ in 0..seconds * FPS {
            self.step(vec![]);
        }
    }
    /// The attached-effect calls of slot `slot`, in order, with the frame they came on.
    fn fx_calls(&self, slot: FxSlot) -> Vec<(u64, FxEffect, FxAction)> {
        self.events
            .iter()
            .filter_map(|(n, e)| match e {
                SimEvent::AppearanceFx {
                    slot: s,
                    effect,
                    action,
                    ..
                } if *s == slot => Some((*n, effect.clone(), *action)),
                _ => None,
            })
            .collect()
    }
    fn find(&self, f: impl Fn(&SimEvent) -> bool) -> Option<u64> {
        self.events.iter().find(|(_, e)| f(e)).map(|(n, _)| *n)
    }
}

/// A home mech and a point the home team sees next to it (passable: the mech stands there).
fn home_mech_and_point(sim: &Sim) -> (ObjectHandle, Vec3) {
    let home = sim.world.teams.home as i32;
    let h = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            sim.world.objects.get(h).is_some_and(|o| {
                o.team_id() == home && matches!(o.kind, Kind::Mech(_)) && !o.c.is_disabled()
            })
        })
        .expect("a home mech");
    let p = sim.world.objects.get(h).unwrap().c.position;
    let at = Vec3 { x: p.x + 40.0, ..p };
    (
        h,
        Vec3 {
            z: sim.world.terrain.elevation(&at),
            ..at
        },
    )
}

#[test]
fn every_support_vehicle_is_dropped_by_the_vtol_and_powers_up() {
    for (vehicle_id, _) in vtol::SUPPORT_VEHICLES
        .iter()
        .filter(|(id, _)| *id != vtol::RECOVERY_VEHICLE_ID)
    {
        let Some(sim) = load() else { return };
        let (_, at) = home_mech_and_point(&sim);
        let mut run = Run {
            sim,
            frame: 0,
            events: Vec::new(),
            unavailable: Vec::new(),
        };
        let before = run.sim.world.objects.movers.len();
        run.step(vec![PlayerCommand::BeginVtol {
            commander: 0,
            vehicle_id: *vehicle_id,
            at,
            target: None,
        }]);
        let voice_id = match *vehicle_id {
            120 => 1,
            182 => 4,
            393 => 5,
            415 => 6,
            _ => unreachable!(),
        };
        assert!(
            run.find(|e| matches!(e, SimEvent::Support(id) if *id == voice_id))
                .is_some(),
            "vehicle {vehicle_id} should emit its support acknowledgement"
        );
        assert!(run.sim.world.vtol.in_flight(0), "vehicle {vehicle_id}");
        assert!(
            run.find(|e| matches!(e, SimEvent::VtolRefused { .. }))
                .is_none(),
            "vehicle {vehicle_id} was refused"
        );
        run.run(12);
        assert!(run.find(|e| matches!(e, SimEvent::Sound { id, at: Some(_), .. } if *id == mc2_sim::events::sfx::VTOL_ANIMATE)).is_some(), "VTOL animation sound");
        assert!(run.find(|e| matches!(e, SimEvent::Sound { id, at: Some(_), .. } if *id == mc2_sim::events::sfx::VTOL_DROP)).is_some(), "VTOL drop sound");
        let dropped = run
            .find(|e| matches!(e, SimEvent::SupportVehicleDropped { vehicle_id: v, .. } if v == vehicle_id))
            .unwrap_or_else(|| panic!("vehicle {vehicle_id} was not dropped"));
        let left = run
            .find(|e| matches!(e, SimEvent::VtolFinished { vehicle_id: v, .. } if v == vehicle_id))
            .expect("the VTOL left");
        // frame 145 and 300 of a 30 fps animation: 4.83 s and 10 s after the start (two frames of tolerance)
        let (d, l) = (dropped as i64 - 2, left as i64 - 2);
        assert!((d - 145).abs() <= 2, "dropped at {d}");
        assert!((l - 300).abs() <= 2, "left at {l}");
        assert!(!run.sim.world.vtol.in_flight(0));
        // the VTOL's dust cloud (missiongui.cpp:4282): started once where the craft is, follows it, ends when it has flown away
        let dust = run.fx_calls(FxSlot::SupportDust(0));
        assert_eq!(
            dust.iter()
                .filter(|c| matches!(c.2, FxAction::Start { .. }))
                .count(),
            1,
            "vehicle {vehicle_id}: {dust:?}"
        );
        assert_eq!(dust[0].1, FxEffect::Id(fx_id::VTOL_DUST_CLOUD));
        let FxAction::Start { place: first, .. } = dust[0].2 else {
            panic!("the dust starts first: {dust:?}");
        };
        // `vTol->position` is where the click put the craft; the cloud stands there
        assert!(
            (first.at.x - at.x).abs() < 1.0 && (first.at.y - at.y).abs() < 1.0,
            "the dust starts at the drop point: {first:?}"
        );
        assert!(dust.len() > 2, "the dust is executed every frame");
        assert!(
            matches!(dust.last().unwrap().2, FxAction::Kill),
            "the dust is ended with the flight"
        );
        assert!(run.fx_calls(FxSlot::RecoveryBeam(0)).is_empty());
        assert_eq!(run.sim.world.objects.movers.len(), before + 1);
        let part = run.sim.world.vtol.last_dropped[0].expect("the dropped mover");
        let h = run.sim.world.objects.find_by_part_id(part).unwrap();
        let o = run.sim.world.objects.get(h).unwrap();
        assert_eq!(o.team_id(), run.sim.world.teams.home as i32);
        let commander = match &o.kind {
            Kind::Vehicle(v) => v.m.commander.0,
            // the Pegasus scout is a helicopter, a `BattleMech`-class object with the air movement type
            Kind::Mech(m) => m.m.commander.0,
            _ => panic!("vehicle {vehicle_id} is a mover"),
        };
        assert_eq!(commander, 0);
        // the Pegasus scout is a helicopter: its power-up raises the helicopter dust cloud at the craft (mech.cpp:4808)
        let fall: Vec<_> = run
            .events
            .iter()
            .filter_map(|(_, e)| match e {
                SimEvent::AppearanceFx {
                    object,
                    slot: FxSlot::FallDust,
                    effect,
                    action,
                } => Some((*object, effect.clone(), *action)),
                _ => None,
            })
            .collect();
        if matches!(o.kind, Kind::Mech(_)) {
            let start = fall
                .iter()
                .find(|c| c.0 == h && matches!(c.2, FxAction::Start { .. }))
                .unwrap_or_else(|| panic!("the helicopter dust cloud starts: {fall:?}"));
            assert_eq!(start.1, FxEffect::Id(fx_id::HELICOPTER_DUST_CLOUD));
            let FxAction::Start { place, looping } = start.2 else {
                unreachable!()
            };
            assert!(!looping);
            assert!(
                (place.at.x - at.x).abs() < 2.0 && (place.at.y - at.y).abs() < 2.0,
                "{place:?} {at:?}"
            );
        } else {
            assert!(fall.is_empty(), "only a helicopter raises it");
        }
        assert!(
            !matches!(
                o.c.status,
                ObjectStatus::ShutDown | ObjectStatus::ShuttingDown
            ),
            "vehicle {vehicle_id} powers up when the VTOL leaves"
        );
        let p = o.c.position;
        assert!(
            (p.x - at.x).abs() < 1.0 && (p.y - at.y).abs() < 1.0,
            "dropped at the click"
        );
    }
}

#[test]
fn a_purchase_off_the_map_or_during_a_flight_is_refused() {
    let Some(sim) = load() else { return };
    let (_, at) = home_mech_and_point(&sim);
    let mut run = Run {
        sim,
        frame: 0,
        events: Vec::new(),
        unavailable: Vec::new(),
    };
    run.step(vec![PlayerCommand::BeginVtol {
        commander: 0,
        vehicle_id: 182,
        at: Vec3 {
            x: -1.0e7,
            y: 0.0,
            z: 0.0,
        },
        target: None,
    }]);
    assert!(run
        .find(|e| matches!(e, SimEvent::VtolRefused { .. }))
        .is_some());
    assert!(!run.sim.world.vtol.in_flight(0));
    let go = |at| PlayerCommand::BeginVtol {
        commander: 0,
        vehicle_id: 182,
        at,
        target: None,
    };
    run.step(vec![go(at)]);
    assert!(run.sim.world.vtol.in_flight(0));
    let refused = run
        .events
        .iter()
        .filter(|(_, e)| matches!(e, SimEvent::VtolRefused { .. }))
        .count();
    run.step(vec![go(at)]);
    let now = run
        .events
        .iter()
        .filter(|(_, e)| matches!(e, SimEvent::VtolRefused { .. }))
        .count();
    assert_eq!(now, refused + 1, "a second VTOL while one flies is refused");
}

fn disabled_enemy_mech_near(run: &mut Run) -> (ObjectHandle, mc2_sim::ids::WatchId) {
    let (home, _) = home_mech_and_point(&run.sim);
    let hp = run.sim.world.objects.get(home).unwrap().c.position;
    let enemy = run
        .sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            run.sim.world.objects.get(h).is_some_and(|o| {
                o.team_id() != run.sim.world.teams.home as i32
                    && matches!(o.kind, Kind::Mech(_))
                    && !o.c.is_destroyed()
            })
        })
        .expect("an enemy mech");
    let z = run.sim.world.terrain.elevation(&Vec3 {
        x: hp.x + 60.0,
        ..hp
    });
    let o = run.sim.world.objects.get_mut(enemy).unwrap();
    o.c.position = Vec3 {
        x: hp.x + 60.0,
        y: hp.y,
        z,
    };
    o.c.set_awake(true);
    mc2_sim::combat::damage::disable(
        &mut run.sim.world,
        enemy,
        mc2_sim::combat::fx::death::POWER_USED_UP,
    );
    let wid = run.sim.world.objects.watch_id(enemy);
    (enemy, wid)
}

#[test]
fn the_recovery_craft_recovers_a_disabled_mech_for_the_home_team() {
    let Some(mut sim) = load() else { return };
    let file = sim.world.data.pilot_profiles.keys().next().unwrap().clone();
    sim.world.recovery_pilots = Some(Rc::new(Roster(file)));
    let mut run = Run {
        sim,
        frame: 0,
        events: Vec::new(),
        unavailable: Vec::new(),
    };
    let (mech, part) = disabled_enemy_mech_near(&mut run);
    let points = run.sim.world.mission.resource_points;
    let at = run.sim.world.objects.get(mech).unwrap().c.position;
    run.step(vec![PlayerCommand::BeginVtol {
        commander: 0,
        vehicle_id: vtol::RECOVERY_VEHICLE_ID,
        at,
        target: Some(part),
    }]);
    assert!(
        run.find(|e| matches!(e, SimEvent::Support(3))).is_some(),
        "recovery craft should emit SUPPORT_RECOVER"
    );
    assert!(run.sim.world.vtol.in_flight(0), "{:?}", run.events);
    run.run(40);
    let done = run
        .events
        .iter()
        .find_map(|(_, e)| match e {
            SimEvent::VtolFinished { recovered, .. } => Some(*recovered),
            _ => None,
        })
        .expect("the Karnov left");
    assert!(done, "the mech was recovered");
    // the Karnov's dust cloud and, once it hovers, its recovery beam (missiongui.cpp:1013-1033)
    let dust = run.fx_calls(FxSlot::SupportDust(0));
    assert_eq!(
        dust[0].1,
        FxEffect::Id(fx_id::KARNOV_DUST_CLOUD),
        "{dust:?}"
    );
    assert!(matches!(dust[0].2, FxAction::Start { .. }));
    let beam = run.fx_calls(FxSlot::RecoveryBeam(0));
    assert_eq!(beam[0].1, FxEffect::Id(fx_id::KARNOV_RECOVERY_BEAM));
    let FxAction::Start { place, looping } = beam[0].2 else {
        panic!("the beam starts first: {beam:?}");
    };
    assert!(!looping);
    let m = run.sim.world.objects.get(mech).unwrap().c.position;
    let dist = ((place.at.x - m.x).powi(2) + (place.at.y - m.y).powi(2)).sqrt();
    assert!(dist < 128.0, "the beam starts over the mech: {dist}");
    assert!(
        beam.iter()
            .filter(|c| matches!(c.2, FxAction::Start { .. }))
            .count()
            == 1
            && matches!(beam.last().unwrap().2, FxAction::Kill),
        "{beam:?}"
    );
    let o = run.sim.world.objects.get(mech).unwrap();
    assert_eq!(o.team_id(), run.sim.world.teams.home as i32);
    assert!(!o.c.is_disabled(), "the recovered mech is repaired");
    assert_eq!(
        run.sim.world.mission.resource_points, points,
        "no refund after a recovery"
    );
    assert!(!matches!(o.c.status, ObjectStatus::Destroyed));
}

#[test]
fn a_recovery_without_a_pilot_returns_the_cost_and_reports_it() {
    let Some(sim) = load() else { return };
    let mut run = Run {
        sim,
        frame: 0,
        events: Vec::new(),
        unavailable: Vec::new(),
    };
    let (mech, part) = disabled_enemy_mech_near(&mut run);
    let points = run.sim.world.mission.resource_points;
    let at = run.sim.world.objects.get(mech).unwrap().c.position;
    run.step(vec![PlayerCommand::BeginVtol {
        commander: 0,
        vehicle_id: vtol::RECOVERY_VEHICLE_ID,
        at,
        target: Some(part),
    }]);
    run.run(40);
    let recovered = run
        .events
        .iter()
        .find_map(|(_, e)| match e {
            SimEvent::VtolFinished { recovered, .. } => Some(*recovered),
            _ => None,
        })
        .expect("the Karnov left");
    assert!(!recovered);
    assert_eq!(
        run.sim.world.mission.resource_points,
        points + vtol::RECOVERY_REFUND
    );
    assert!(run.sim.world.objects.get(mech).unwrap().c.is_disabled());
    assert!(
        run.unavailable.iter().any(|u| u.contains("no pilot")),
        "{:?}",
        run.unavailable
    );
}

#[test]
fn resource_points_are_spent_and_returned_by_command() {
    let Some(sim) = load() else { return };
    let mut run = Run {
        sim,
        frame: 0,
        events: Vec::new(),
        unavailable: Vec::new(),
    };
    let start = run.sim.world.mission.resource_points;
    run.step(vec![PlayerCommand::AddResourcePoints(-6000)]);
    assert_eq!(run.sim.world.mission.resource_points, start - 6000);
    run.step(vec![PlayerCommand::AddResourcePoints(6000)]);
    assert_eq!(run.sim.world.mission.resource_points, start);
}

fn armor_total(sim: &Sim, h: ObjectHandle) -> f32 {
    let l = mc2_sim::combat::loadout_of(sim.world.objects.get(h).unwrap()).unwrap();
    l.armor.iter().map(|a| a.0).sum::<f32>()
        + l.body.iter().map(|b| b.cur_internal_structure).sum::<f32>()
}

/// `GroundVehicle::fireWeapon` (gvehicl.cpp:4502) through `Sim::step`: a guard tower the VTOL dropped shoots at an enemy mech
/// that comes into range, launches shots from its own weapon node, and the enemy loses armour to them.
#[test]
fn a_dropped_guard_tower_fires_at_an_enemy_and_damages_it() {
    let Some(sim) = load() else { return };
    let (_, at) = home_mech_and_point(&sim);
    let mut run = Run {
        sim,
        frame: 0,
        events: Vec::new(),
        unavailable: Vec::new(),
    };
    run.step(vec![PlayerCommand::BeginVtol {
        commander: 0,
        vehicle_id: 415,
        at,
        target: None,
    }]);
    run.run(12);
    let part = run.sim.world.vtol.last_dropped[0].expect("the tower was dropped");
    let tower = run.sim.world.objects.find_by_part_id(part).unwrap();
    assert!(matches!(
        run.sim.world.objects.get(tower).unwrap().kind,
        Kind::Vehicle(_)
    ));
    let tower_wid = run.sim.world.objects.watch_id(tower);
    // an enemy mech (declared input: placed in the tower's range)
    let enemy = run
        .sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            run.sim.world.objects.get(h).is_some_and(|o| {
                o.team_id() != run.sim.world.teams.home as i32
                    && matches!(o.kind, Kind::Mech(_))
                    && !o.c.is_destroyed()
            })
        })
        .expect("an enemy mech");
    // Declared input: the enemy stands 65 m from the tower in the first of 72 directions the tower has a line of fire in
    // (the weapon status of `calcWeaponsStatus`, which also needs the target inside the tower's fire arc).
    let slot = match &run.sim.world.objects.get(tower).unwrap().kind {
        Kind::Vehicle(v) => v.m.pilot_slot,
        _ => unreachable!(),
    };
    let tower_at = run.sim.world.objects.get(tower).unwrap().c.position;
    let mut placed = false;
    let mut stand = tower_at;
    for k in 0..72 {
        let a = (k as f32) * 5.0f32.to_radians();
        let r = 65.0 * mc2_sim::terrain::WORLD_UNITS_PER_METER;
        let spot = Vec3 {
            x: tower_at.x + r * a.cos(),
            y: tower_at.y + r * a.sin(),
            z: 0.0,
        };
        let z = run.sim.world.terrain.elevation(&spot);
        {
            let o = run.sim.world.objects.get_mut(enemy).unwrap();
            o.c.position = Vec3 { z, ..spot };
            o.c.set_awake(true);
        }
        // a frame lets the appearances and the line-of-sight cache follow the move
        run.step(vec![]);
        let status =
            mc2_sim::ai::decide::calc_weapons_status(&mut run.sim.world, slot, Some(enemy), None);
        if status > 0 {
            stand = run.sim.world.objects.get(enemy).unwrap().c.position;
            placed = true;
            break;
        }
    }
    assert!(placed, "the tower has a line of fire in some direction");
    let before = armor_total(&run.sim, enemy);
    // The tower's own target choice is its brain script (ABL natives of the AI slice); the player's attack order drives the same
    // `combatDecisionTree` -> `Mover::fireWeapon` path.
    let mut attack = mc2_sim::command::TacticalOrder::new(
        mc2_sim::command::OrderOrigin::Player,
        mc2_sim::command::OrderCode::AttackObject,
        false,
        run.sim.world.clock.scenario_time,
    );
    attack.target = run.sim.world.objects.watch_id(enemy);
    attack.target_object_class = 1;
    run.step(vec![PlayerCommand::Order {
        movers: vec![part],
        order: attack,
        queue: false,
    }]);
    // The tower's weapon is an area-effect weapon: its bolt ends in an impact event (reported as `Miss`, since an area bolt keeps
    // no target) and the explosion it creates does the damage. The enemy is held on its spot, and nothing else shoots at it in this
    // window, so its armour stays whole until the tower's shot lands and is lower after it.
    let is_tower = |e: &SimEvent, want: mc2_sim::events::WeaponImpact| matches!(e, SimEvent::WeaponEffect { owner, impact, .. } if *owner == tower_wid && *impact == want);
    let mut launched = None;
    let mut landed = None;
    for _ in 0..(40 * FPS) {
        run.sim.world.objects.get_mut(enemy).unwrap().c.position = stand;
        let seen = run.events.len();
        run.step(vec![]);
        let new = &run.events[seen..];
        if launched.is_none()
            && new
                .iter()
                .any(|(_, e)| is_tower(e, mc2_sim::events::WeaponImpact::Launch))
        {
            launched = Some(armor_total(&run.sim, enemy));
        }
        if launched.is_some()
            && new.iter().any(|(_, e)| {
                !is_tower(e, mc2_sim::events::WeaponImpact::Launch)
                    && matches!(e, SimEvent::WeaponEffect { owner, .. } if *owner == tower_wid)
            })
        {
            landed = Some(run.frame);
            break;
        }
    }
    let launched = launched.expect("the guard tower fired");
    landed.expect("its shot landed");
    assert_eq!(
        launched, before,
        "nothing else hurt the enemy before the shot"
    );
    run.run(1);
    assert!(
        armor_total(&run.sim, enemy) < before,
        "the tower's shot damaged the enemy: {before} -> {}",
        armor_total(&run.sim, enemy)
    );
    assert!(
        run.unavailable.iter().all(|u| !u.contains("FireWeapon")),
        "{:?}",
        run.unavailable
    );
}
