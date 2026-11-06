// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! An explicitly unavailable combat action is reported in its own frame and never stops later frames: consecutive `Sim::step`s.

mod common;

use mc2_sim::combat::artillery::SENSOR_POOL_EXHAUSTED_HOOK;
use mc2_sim::combat::artillery_math::ARTILLERY_SENSOR;
use mc2_sim::math::Vec3;
use mc2_sim::mission::init::LoadOutcome;
use mc2_sim::rng::RngMode;
use mc2_sim::sim::FrameInput;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameUnavailable, PlayerCommand, Sim, World};
use std::rc::Rc;

fn frame(sim: &mut Sim, now: u64, cmds: Vec<PlayerCommand>) -> Vec<FrameUnavailable> {
    frame_with(sim, now, cmds, Rc::new(AllVisible))
}

fn frame_with(
    sim: &mut Sim,
    now: u64,
    cmds: Vec<PlayerCommand>,
    vis: Rc<dyn mc2_sim::visibility::Visibility>,
) -> Vec<FrameUnavailable> {
    sim.step(FrameInput::fixed(30, now, vis, cmds))
        .expect("a deferred combat action must not stop stepping")
        .unavailable
}

fn combat(hook: &'static str) -> FrameUnavailable {
    FrameUnavailable::Combat { hook }
}

#[test]
fn player_strikes_report_then_next_frame_steps() {
    // The sensor strike of a world whose 150-slot sensor pool is entirely occupied: refused and reported (the C++ fatals on
    // "No More Free Sensors", contact.cpp:1223). The player LARGE/SMALL strike line of sight is ported, so it is not in this test.
    for (strike, hook) in [(ARTILLERY_SENSOR, SENSOR_POOL_EXHAUSTED_HOOK)] {
        let mut sim = Sim::new(LoadOutcome {
            world: World::new(RngMode::LegacyMsvc, 1),
            complete: true,
            missing_hooks: Vec::new(),
        });
        mc2_sim::ai::sensors::init(&mut sim.world);
        for sensor in &mut sim.world.sensors.pool {
            sensor.owner = mc2_sim::ids::ObjectHandle(1);
        }
        let cmd = PlayerCommand::CallStrike {
            strike,
            at: Vec3::default(),
        };
        assert!(frame(&mut sim, 1, vec![cmd]).contains(&combat(hook)));
        assert!(sim.world.objects.artillery.is_empty(), "no strike created");
        assert!(sim.world.load_is_complete());
        assert!(frame(&mut sim, 2, Vec::new()).is_empty(), "ordinary frame");
        assert!(frame(&mut sim, 3, Vec::new()).is_empty(), "and another");
    }
}
