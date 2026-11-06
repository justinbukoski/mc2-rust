// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GroundVehicle::createVehiclePilot` (gvehicl.cpp:2710-2807): the crew of a destroyed ground vehicle bails out as an infantry
//! "vehicle pilot" (object type 638) of the home team and commander 1, where the vehicle blew up, with the vehicle's own pilot.
mod common;

use mc2_sim::combat::damage::handle_destruction;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::Kind;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim};
use std::rc::Rc;

const VEHICLEPILOT_ID: i32 = 638;

fn load() -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome =
        common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_03", seed, mode)).unwrap();
    Some(Sim::new(outcome))
}

/// The first live vehicle that carries a pilot and stands on dry land.
fn vehicle_on_land(sim: &Sim) -> ObjectHandle {
    *sim.world
        .objects
        .movers
        .iter()
        .find(|&&h| {
            let o = sim.world.objects.get(h).unwrap();
            let Kind::Vehicle(v) = &o.kind else {
                return false;
            };
            let (r, c) = sim.world.terrain.world_to_cell(&o.c.position);
            v.m.path_locks_enabled
                && !sim.world.terrain.get_deep_water(r, c)
                && !sim.world.terrain.get_shallow_water(r, c)
        })
        .expect("a land vehicle with path locks")
}

fn crew(sim: &Sim) -> Vec<ObjectHandle> {
    (0..sim.world.objects.max_vehicles)
        .filter_map(|i| sim.world.objects.vehicles.get(i).copied())
        .filter(|&h| {
            let o = sim.world.objects.get(h).unwrap();
            matches!(&o.kind, Kind::Vehicle(v) if v.is_vehicle_pilot)
        })
        .collect()
}

fn run_until_exploded(sim: &mut Sim, victim: ObjectHandle) {
    let vis = Rc::new(AllVisible);
    for turn in 2..400 {
        sim.step(FrameInput::fixed(30, turn, vis.clone(), vec![]))
            .unwrap();
        let o = sim.world.objects.get(victim).unwrap();
        if matches!(&o.kind, Kind::Vehicle(v) if v.m.flags.exploding) {
            return;
        }
    }
    panic!("the destroyed vehicle never blew up");
}

#[test]
fn destroyed_vehicle_spawns_its_crew() {
    let Some(mut sim) = load() else { return };
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis, vec![])).unwrap();
    let victim = vehicle_on_land(&sim);
    let (slot, at) = {
        let o = sim.world.objects.get(victim).unwrap();
        let Kind::Vehicle(v) = &o.kind else { panic!() };
        (v.m.pilot_slot, o.c.position)
    };
    assert!(crew(&sim).is_empty());
    handle_destruction(&mut sim.world, victim, None);
    run_until_exploded(&mut sim, victim);

    let crews = crew(&sim);
    assert_eq!(crews.len(), 1, "exactly one crew unit");
    let wid = sim.world.objects.watch_id(crews[0]);
    let o = sim.world.objects.get(crews[0]).unwrap();
    let Kind::Vehicle(v) = &o.kind else { panic!() };
    // type, team, commander and place of the original (gvehicl.cpp:2776-2790)
    assert_eq!(o.c.type_handle, VEHICLEPILOT_ID);
    assert_eq!(v.m.team.0 as usize, sim.world.teams.home);
    assert_eq!(v.m.commander.0, 1);
    assert!((o.c.position.x - at.x).abs() < 1.0 && (o.c.position.y - at.y).abs() < 1.0);
    // hit points of the type's packet (the shipped pilot has 1 per location)
    let packet = &sim.world.data.vehicle_objects[&(VEHICLEPILOT_ID as u32)];
    for (i, (cur, _)) in v.loadout.armor.iter().enumerate() {
        assert_eq!(*cur, f32::from(packet.parts[i].cur_armor), "location {i}");
        assert_eq!(*cur, 1.0, "location {i}");
    }
    // it has the destroyed vehicle's pilot, without a brain, and runs to the map origin (gvehicl.cpp:2791-2800)
    assert_eq!(v.m.pilot_slot, slot);
    let pilot = sim.world.pilots.get(slot).unwrap();
    assert_eq!(pilot.mover, wid);
    assert!(pilot.brain.is_none());
    // `orderMoveToPoint` leaves a location goal at (0, 0, 0); the tactical order itself is re-issued by the crew's own update
    assert_eq!(
        pilot.lane_b_move.goal_type,
        mc2_sim::ai::moveorders::MOVEGOAL_LOCATION
    );
    let (_, goal, _) = mc2_sim::ai::warrior::get_move_goal(&sim.world, slot);
    assert_eq!((goal.x, goal.y), (0.0, 0.0));
    // Tangible, so it can be hit, and a mover (the slot is counted in `numVehicles`, DIFFERENCES.md) that stays while the camera sees it.
    assert!(o.c.tangible());
    assert!(o.exists(), "the crew stays while it is in view");
    assert!(sim.world.objects.movers.contains(&crews[0]));
}

/// "Whack us when we are no longer in view" (gvehicl.cpp:3508): the crew goes once the camera loses it.
#[test]
fn the_crew_is_removed_when_it_leaves_the_view() {
    let Some(mut sim) = load() else { return };
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis, vec![])).unwrap();
    let victim = vehicle_on_land(&sim);
    handle_destruction(&mut sim.world, victim, None);
    run_until_exploded(&mut sim, victim);
    let crews = crew(&sim);
    assert_eq!(crews.len(), 1);
    assert!(sim.world.objects.get(crews[0]).unwrap().exists());
    let turn = (sim.world.clock.turn + 1) as u64;
    sim.step(FrameInput::fixed(
        30,
        turn,
        Rc::new(mc2_sim::visibility::NoneVisible),
        vec![],
    ))
    .unwrap();
    assert!(!sim.world.objects.get(crews[0]).unwrap().exists());
}

/// A visible crew with no tactical order runs for the origin again (gvehicl.cpp:3512-3517).
#[test]
fn an_idle_crew_is_ordered_to_run_to_the_origin_again() {
    let Some(mut sim) = load() else { return };
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis.clone(), vec![]))
        .unwrap();
    let victim = vehicle_on_land(&sim);
    handle_destruction(&mut sim.world, victim, None);
    run_until_exploded(&mut sim, victim);
    let crews = crew(&sim);
    let slot = {
        let Kind::Vehicle(v) = &sim.world.objects.get(crews[0]).unwrap().kind else {
            panic!()
        };
        v.m.pilot_slot
    };
    mc2_sim::ai::warrior::clear_move_orders(&mut sim.world, slot);
    mc2_sim::ai::warrior::clear_attack_orders(&mut sim.world, slot);
    sim.world.pilots.get_mut(slot).unwrap().cur_order.code = mc2_sim::command::OrderCode::None;
    assert_ne!(
        sim.world.pilots.get(slot).unwrap().lane_b_move.goal_type,
        mc2_sim::ai::moveorders::MOVEGOAL_LOCATION,
        "the escape request is cleared"
    );
    let turn = (sim.world.clock.turn + 1) as u64;
    sim.step(FrameInput::fixed(30, turn, vis, vec![])).unwrap();
    let pilot = sim.world.pilots.get(slot).unwrap();
    assert_eq!(
        pilot.lane_b_move.goal_type,
        mc2_sim::ai::moveorders::MOVEGOAL_LOCATION
    );
    let (_, goal, _) = mc2_sim::ai::warrior::get_move_goal(&sim.world, slot);
    assert_eq!((goal.x, goal.y), (0.0, 0.0));
}

#[test]
fn vehicle_without_path_locks_leaves_no_crew() {
    let Some(mut sim) = load() else { return };
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis, vec![])).unwrap();
    let victim = vehicle_on_land(&sim);
    // powered armor and infantry (no path locks) do not bail out (gvehicl.cpp:2716)
    if let Kind::Vehicle(v) = &mut sim.world.objects.get_mut(victim).unwrap().kind {
        v.m.path_locks_enabled = false;
    }
    handle_destruction(&mut sim.world, victim, None);
    run_until_exploded(&mut sim, victim);
    assert!(crew(&sim).is_empty());
}

#[test]
fn a_crew_unit_does_not_breed() {
    let Some(mut sim) = load() else { return };
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis, vec![])).unwrap();
    let victim = vehicle_on_land(&sim);
    if let Kind::Vehicle(v) = &mut sim.world.objects.get_mut(victim).unwrap().kind {
        v.is_vehicle_pilot = true;
    }
    handle_destruction(&mut sim.world, victim, None);
    run_until_exploded(&mut sim, victim);
    assert_eq!(crew(&sim).len(), 1, "only the marked victim itself");
}

/// Test-only switch `D-106`: the C++ logic error applies, so a crew in full view is gone by the frame after it was made, and the
/// first crew is kept out of the mover lists. The shipped setting (switch off) keeps the visible crew (test above).
#[test]
fn the_cpp_switch_removes_the_crew_even_in_view() {
    let Some(mut sim) = load() else { return };
    sim.world.cpp_vehicle_pilot_d106 = true;
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis.clone(), vec![]))
        .unwrap();
    let victim = vehicle_on_land(&sim);
    handle_destruction(&mut sim.world, victim, None);
    run_until_exploded(&mut sim, victim);
    let made = crew(&sim);
    assert_eq!(made.len(), 1, "the crew is still created");
    for _ in 0..2 {
        let turn = (sim.world.clock.turn + 1) as u64;
        sim.step(FrameInput::fixed(30, turn, vis.clone(), vec![]))
            .unwrap();
    }
    let alive = |sim: &Sim| sim.world.objects.get(made[0]).unwrap().exists();
    let in_movers = sim.world.objects.movers.contains(&made[0]);
    assert!(
        !alive(&sim) || !in_movers,
        "the crew is removed or kept out of the mover lists"
    );
}
