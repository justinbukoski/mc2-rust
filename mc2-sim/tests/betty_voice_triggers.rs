// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Production mission-data checks for the newly wired building Betty cues.
//! C++ trigger: `Building::setTeamId` (`code/bldng.cpp:898,952,966,970`).

mod common;

use mc2_sim::events::{betty, SimEvent};
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::object::structure::{set_building_team, BuildingState};
use mc2_sim::object::{flags, Common, Kind, Obj, ObjectClass};
use mc2_sim::rng::RngMode;
use mc2_sim_data::objtype::ObjectTypeData;

fn load() -> Option<mc2_sim::World> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let outcome = load_world(&mut vfs, &LoadSpec::quickstart("mc2_03", 7, RngMode::Pcg32))
        .expect("load mc2_03");
    assert!(outcome.complete, "mc2_03 loads completely");
    Some(outcome.world)
}

#[test]
fn building_and_mover_triggers_emit_the_expected_betty_samples() {
    let Some(mut world) = load() else { return };
    world.clock.turn = 12;

    let find_type = |matches: fn(&mc2_sim_data::objtype::BuildingType) -> bool| {
        (0..4096)
            .find(|&n| {
                world.objects.type_by_num(n).is_some_and(|t| match &t.data {
                    ObjectTypeData::Building(b) => matches(b),
                    _ => false,
                })
            })
            .expect("loaded object type table contains the requested building")
    };
    let add_building = |world: &mut mc2_sim::World, type_handle: i32| {
        let h = mc2_sim::ids::ObjectHandle(world.objects.list.len() as u32);
        let mut c = Common::new(ObjectClass::Building, h);
        c.type_handle = type_handle;
        c.flags |= flags::EXISTS;
        world.objects.list.push(Some(Obj {
            c,
            kind: Kind::Building(Box::new(BuildingState {
                team_id: 1,
                ..BuildingState::default()
            })),
        }));
        world.objects.buildings.push(h);
        h
    };
    let sensor_type = find_type(|b| b.sensor_range > -1.0 && b.lookout_tower_range <= 0.0);
    let resource_type = find_type(|b| b.resource_points != 0);
    let sensor = add_building(&mut world, sensor_type);
    set_building_team(&mut world, sensor, 0, false);
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::Betty(id) if *id == betty::SENSOR_CAPTURED)));

    world.events.clear();
    let resource = add_building(&mut world, resource_type);
    set_building_team(&mut world, resource, 0, false);
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::Betty(id) if *id == betty::RESOURCES)));

    world.events.clear();
    let parent = add_building(&mut world, sensor_type);
    let child = add_building(&mut world, sensor_type);
    let parent_wid = world.objects.watch_id(parent);
    if let Some(Kind::Building(b)) = world.objects.get_mut(child).map(|o| &mut o.kind) {
        b.team_id = 0;
        b.parent = parent_wid;
    }
    if let Some(o) = world.objects.get_mut(parent) {
        o.c.set_awake(true);
    }
    mc2_sim::object::structure::update_building(&mut world, child);
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::Betty(id) if *id == betty::BUILDING_RECAPTURED)));

    world.events.clear();
    let turret_type = (0..4096)
        .find(|&n| {
            world
                .objects
                .type_by_num(n)
                .is_some_and(|t| matches!(&t.data, ObjectTypeData::Turret(_)))
        })
        .expect("loaded object type table contains a turret");
    let parent = add_building(&mut world, sensor_type);
    if let Some(o) = world.objects.get_mut(parent) {
        o.c.status = mc2_sim::object::ObjectStatus::Disabled;
    }
    let turret = mc2_sim::ids::ObjectHandle(world.objects.list.len() as u32);
    let parent_wid = world.objects.watch_id(parent);
    let mut c = Common::new(ObjectClass::Turret, turret);
    c.type_handle = turret_type;
    c.flags |= flags::EXISTS | flags::AWAKE;
    world.objects.list.push(Some(Obj {
        c,
        kind: Kind::Turret(Box::new(mc2_sim::object::structure::TurretState {
            parent: parent_wid,
            ..mc2_sim::object::structure::TurretState::new(0.0)
        })),
    }));
    world.objects.turrets.push(turret);
    mc2_sim::object::structure::update_turret(&mut world, turret);
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::Betty(id) if *id == betty::GENERATOR_DESTROYED)));

    world.events.clear();
    let home = world.teams.home as i32;
    let enemy_mech = world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            world.objects.get(h).is_some_and(|o| {
                o.team_id() != home && matches!(&o.kind, Kind::Mech(m) if m.m.move_level != 2)
            })
        })
        .expect("mission contains an enemy ground mech");
    if let Some(o) = world.objects.get_mut(enemy_mech) {
        if let Kind::Mech(m) = &mut o.kind {
            m.m.flags.start_up_this_frame = true;
        }
        o.c.status = mc2_sim::object::ObjectStatus::StartingUp;
    }
    mc2_sim::mover::mech::update_movement(&mut world, enemy_mech).expect("startup update");
    assert!(world
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::Betty(id) if *id == betty::POWERUP)));
}
