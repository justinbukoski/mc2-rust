//! Production recovery-flight SFX through the sim event and app audio bridge.

use std::path::PathBuf;
use std::rc::Rc;

use mc2_app::sim_audio::map_event;
use mc2_audio::AudioEvent;
use mc2_sim::ids::PartId;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::object::Kind;
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::FrameInput;

#[test]
fn recovery_flight_requests_one_salvage_sample_even_after_it_finishes() {
    let Some(root) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount game data");
    let mut outcome = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_03", 7, RngMode::LegacyMsvc),
    )
    .expect("load mc2_03");
    mc2_sim::mission::vtol::prepare(&mut outcome.world, &mut vfs).expect("prepare VTOL assets");
    let mech = outcome
        .world
        .objects
        .find_by_part_id(PartId(704))
        .expect("mc2_03 recovery target");
    let vehicle = outcome
        .world
        .objects
        .find_by_part_id(PartId(710))
        .expect("mc2_03 recovery vehicle");
    let home = outcome.world.teams.home as i32;
    mc2_sim::mission::trade::trade_mover(&mut outcome.world, vehicle, home, -1, None)
        .expect("make recovery vehicle available to home team");
    if let Some(Kind::Vehicle(v)) = outcome.world.objects.get_mut(vehicle).map(|o| &mut o.kind) {
        v.m.commander = mc2_sim::ids::CommanderId(mc2_sim::combat::HOME_COMMANDER as i8);
    }
    let at = outcome.world.objects.get(vehicle).unwrap().c.position;
    mc2_sim::combat::damage::disable(
        &mut outcome.world,
        mech,
        mc2_sim::combat::fx::death::POWER_USED_UP,
    );
    let target = outcome.world.objects.watch_id(mech);
    let mut sim = mc2_sim::Sim::new(outcome);
    // Run the normal visibility/sensor update before the mission-interface call.
    let seed = sim
        .step(FrameInput::fixed(30, 1, Rc::new(AllVisible), vec![]))
        .unwrap();
    let mut mapped = Vec::new();
    for event in &seed.events {
        map_event(event, &mut mapped);
    }
    sim.world.objects.get_mut(mech).unwrap().c.position = at;
    let pos = at;
    mc2_sim::mission::vtol::begin_vtol(&mut sim.world, 0, 147, pos, Some(target))
        .expect("start a recovery flight");
    let mut starts = 0;
    let mut finished = false;
    for frame in 3..=2400 {
        let output = sim
            .step(FrameInput::fixed(30, frame, Rc::new(AllVisible), vec![]))
            .unwrap();
        for event in &output.events {
            map_event(event, &mut mapped);
        }
        starts += output
            .events
            .iter()
            .filter(|event| matches!(event, mc2_sim::SimEvent::Sound { id: 116, .. }))
            .count();
        if !sim.world.vtol.in_flight(0) {
            finished = true;
            break;
        }
    }
    assert!(finished, "recovery craft completes its full animation");
    assert_eq!(
        starts, 1,
        "the salvage sample starts once over the whole flight"
    );
    assert_eq!(
        mapped
            .iter()
            .filter(|event| matches!(event, AudioEvent::Sound { id: 116, .. }))
            .count(),
        1,
        "the app bridge receives one playback request"
    );
}
