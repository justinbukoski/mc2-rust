// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GroundVehicle::fireWeapon` checks its ripple-fire weapon node only after the hit roll (rc-ai-mc2-04).

mod common;

use mc2_sim::combat::fire::fire_weapon;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim};
use std::rc::Rc;

/// The C++ vehicle tells the target pilot (gvehicl.cpp:4586-4590) and draws the hit roll (:4600) before the node check
/// (:4643-4651). A shot from a recycling node returns 3 with that draw spent and the pilot told, but uses no node on either side
/// (`setWeaponNodeUsed` :4653, the target's `getWeaponNode` :4659) and does not recycle the weapon (:4664). The mech checks its node
/// first (mech.cpp:8034-8036, roll :8056), which is unchanged. mc2_04 `ai` seed 1 ran three draws short at frame 418 without this.
#[test]
fn vehicle_node_check_follows_the_hit_roll() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode))
        .expect("mc2_02 loads");
    let mut sim = Sim::new(outcome);
    sim.step(FrameInput::fixed(30, 1, Rc::new(AllVisible), Vec::new()))
        .unwrap();

    let live = |sim: &Sim, h: ObjectHandle| {
        sim.world
            .objects
            .get(h)
            .is_some_and(|o| o.c.status == ObjectStatus::Normal)
    };
    let vehicle = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            live(&sim, h)
                && sim.world.objects.get(h).is_some_and(|o| {
                    matches!(o.kind, Kind::Vehicle(_))
                        && mc2_sim::combat::loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
                })
                && sim
                    .world
                    .appearance
                    .vehicle(h)
                    .is_some_and(|a| a.type_index != usize::MAX)
        })
        .expect("mc2_02 has an armed vehicle");
    let team = sim.world.objects.get(vehicle).unwrap().team_id();
    let target = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            live(&sim, h)
                && sim
                    .world
                    .objects
                    .get(h)
                    .is_some_and(|o| o.team_id() != team && matches!(o.kind, Kind::Mech(_)))
        })
        .expect("mc2_02 has an opposing mech");
    let weapon = mc2_sim::combat::loadout_of(sim.world.objects.get(vehicle).unwrap())
        .unwrap()
        .num_other();
    let pilot_slot = match &sim.world.objects.get(target).unwrap().kind {
        Kind::Mech(m) => m.m.pilot_slot,
        _ => unreachable!(),
    };
    let vehicle_watch = sim.world.objects.watch_id(vehicle);

    // Put the target well inside the weapon's range, on the vehicle's terrain.
    let mut pos = sim.world.objects.get(vehicle).unwrap().c.position;
    pos.z = sim.world.terrain.elevation(&pos);
    let mut target_pos = pos;
    target_pos.y -= 300.0;
    mc2_sim::object::set_position(&mut sim.world, vehicle, pos, true);
    mc2_sim::object::set_position(&mut sim.world, target, target_pos, true);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, target);

    #[derive(Debug, PartialEq)]
    struct Snap {
        draws: u64,
        vehicle_nodes_used: i64,
        target_nodes_used: i64,
        weapon_ready_time: f32,
        pilot_knows_attacker: bool,
    }
    let snap = |sim: &mut Sim| Snap {
        draws: sim.world.rng.0.borrow().sim.draws(),
        vehicle_nodes_used: sim
            .world
            .appearance
            .vehicle(vehicle)
            .unwrap()
            .node_used
            .iter()
            .map(|&n| i64::from(n))
            .sum(),
        target_nodes_used: sim
            .world
            .appearance
            .mech_mut(target)
            .unwrap()
            .0
            .node_used
            .iter()
            .map(|&n| i64::from(n))
            .sum(),
        weapon_ready_time: mc2_sim::combat::loadout_of(sim.world.objects.get(vehicle).unwrap())
            .unwrap()
            .inventory[weapon]
            .ready_time,
        pilot_knows_attacker: sim
            .world
            .pilots
            .get(pilot_slot)
            .unwrap()
            .attackers
            .iter()
            .any(|&(wid, _)| wid == vehicle_watch),
    };
    // A ready weapon (Mover::isWeaponReady) whose vehicle nodes all have the given recycle left.
    let arm = |sim: &mut Sim, node_recycle: f32| {
        sim.world.clock.scenario_time += 1.0;
        mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap())
            .unwrap()
            .inventory[weapon]
            .ready_time = 0.0;
        let (appearance, _) = sim.world.appearance.vehicle_mut(vehicle).unwrap();
        appearance.node_recycle.fill(node_recycle);
        if let Some(p) = sim.world.pilots.get_mut(pilot_slot) {
            p.attackers.clear();
        }
    };
    let fire = |sim: &mut Sim| {
        fire_weapon(
            &mut sim.world,
            vehicle,
            weapon as i32,
            Some(target),
            -1,
            None,
            0,
        )
        .code
    };

    // Recycling node: rejected after the roll and the pilot call, nothing else changes.
    arm(&mut sim, 0.5);
    let before = snap(&mut sim);
    assert!(!before.pilot_knows_attacker);
    assert_eq!(fire(&mut sim), 3, "a recycling weapon node is not ready");
    assert_eq!(
        snap(&mut sim),
        Snap {
            draws: before.draws + 1,
            pilot_knows_attacker: true,
            ..before
        },
        "only the hit roll and the target pilot call precede the node check"
    );

    // Green node: the shot goes out and uses one node on each side.
    arm(&mut sim, 0.0);
    let before = snap(&mut sim);
    assert_eq!(fire(&mut sim), 0);
    let after = snap(&mut sim);
    assert!(after.draws > before.draws, "the hit roll is drawn");
    assert!(after.pilot_knows_attacker);
    assert_eq!(after.vehicle_nodes_used, before.vehicle_nodes_used + 1);
    assert_eq!(after.target_nodes_used, before.target_nodes_used + 1);
    assert!(after.weapon_ready_time > sim.world.clock.scenario_time);
}
