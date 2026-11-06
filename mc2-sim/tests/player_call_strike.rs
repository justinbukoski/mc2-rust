// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `PlayerCommand::CallStrike` reaches `IfaceCallStrike` through `Sim::step` (artlry.cpp:159-232): an id that is not a strike type
//! is ignored like the C++ (no strike, no report); every valid type creates the matching shipped strike object. The production
//! object data is essential — a data-less world must not fabricate an artillery object. The line-of-sight aimed vs random-offset
//! choice is covered in `tests/player_strike_los.rs`.

mod common;

use mc2_sim::combat::artillery::STRIKE_OBJECT_IDS;
use mc2_sim::combat::artillery_math::{
    normalize_strike, ARTILLERY_LARGE, ARTILLERY_SENSOR, ARTILLERY_SMALL,
};
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::sim::FrameInput;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{PlayerCommand, Sim};
use std::rc::Rc;

fn load() -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let outcome = load_world(&mut vfs, &LoadSpec::quickstart("mc2_01", 7, RngMode::Pcg32))
        .expect("load mc2_01");
    assert!(outcome.complete, "mc2_01 load is complete");
    Some(Sim::new(outcome))
}

fn step_with(sim: &mut Sim, strike: i32) -> Vec<mc2_sim::SimEvent> {
    let at = sim
        .world
        .objects
        .movers
        .first()
        .and_then(|h| sim.world.objects.get(*h))
        .expect("mc2_01 has a live mover")
        .c
        .position;
    sim.step(FrameInput::fixed(
        30,
        1,
        Rc::new(AllVisible),
        vec![PlayerCommand::CallStrike { strike, at }],
    ))
    .expect("step")
    .events
}

#[test]
fn call_strike_is_never_silently_dropped() {
    let Some(mut ignored) = load() else {
        return;
    };
    let invalid_events = step_with(&mut ignored, 99);
    assert!(!invalid_events
        .iter()
        .any(|e| matches!(e, mc2_sim::SimEvent::Support(_))));
    assert_eq!(
        ignored.world.objects.artillery_index, 0,
        "a bad id must not touch the strike pool"
    );
    assert!(ignored.world.objects.artillery.iter().all(|h| ignored
        .world
        .objects
        .get(*h)
        .is_none_or(|o| !o.exists())));

    for strike in [ARTILLERY_LARGE, ARTILLERY_SMALL, ARTILLERY_SENSOR] {
        let Some(mut sim) = load() else {
            return;
        };
        let events = step_with(&mut sim, strike);
        let expected_voice = if strike == ARTILLERY_SENSOR { 2 } else { 0 };
        assert!(
            events
                .iter()
                .any(|e| matches!(e, mc2_sim::SimEvent::Support(id) if *id == expected_voice)),
            "strike {strike} should emit its support voice"
        );
        let h = sim
            .world
            .objects
            .artillery
            .iter()
            .copied()
            .find(|h| sim.world.objects.get(*h).is_some_and(|o| o.exists()))
            .unwrap_or_else(|| panic!("strike {strike} must reach CallArtillery"));
        let normalized = normalize_strike(strike, false).expect("a real strike id");
        assert_eq!(
            sim.world.objects.get(h).unwrap().c.type_handle,
            STRIKE_OBJECT_IDS[normalized as usize],
            "strike {strike} reaches its normalized artillery type"
        );
    }
}
