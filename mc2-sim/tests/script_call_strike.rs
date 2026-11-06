// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! ABL strike natives execute through the mission brain and the real artillery pool.
mod common;

use mc2_abl::{Config, DefaultHost, Env, MemorySource};
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::object::Kind;
use mc2_sim::rng::RngMode;
use mc2_sim::sim::FrameInput;
use mc2_sim::visibility::AllVisible;
use mc2_sim::Sim;
use std::rc::Rc;

fn run(call: &str) -> Option<Sim> {
    run_program("", call)
}

fn run_program(vars: &str, call: &str) -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let mut outcome =
        load_world(&mut vfs, &LoadSpec::quickstart("mc2_01", 7, RngMode::Pcg32)).expect("load");
    let mut src = MemorySource::new();
    src.insert(
        "data/missions/strike.abl",
        format!("module strike : integer;\n{vars}\ncode\n{call}\nreturn(0);\nendmodule.\n"),
    );
    let mut env = Env::new(Config::default(), src, Box::new(DefaultHost));
    let handle = env
        .pre_process("data/missions/strike.abl")
        .expect("compile");
    outcome.world.mission.brain = Some(env.new_instance(handle).expect("instance"));
    for pilot in &mut outcome.world.pilots.slots {
        pilot.brain = None;
    }
    outcome.world.abl = Some(env);
    let mut sim = Sim::new(outcome);
    let output = sim
        .step(FrameInput::fixed(30, 1, Rc::new(AllVisible), Vec::new()))
        .expect("step");
    for entry in &output.unavailable {
        if let mc2_sim::FrameUnavailable::Ai {
            what: mc2_sim::ai::AiUnavailable::Native { id },
            ..
        } = entry
        {
            let name = mc2_abl::natives::NATIVES[usize::from(*id)].name;
            assert!(
                ![
                    "callstrike",
                    "callstrikeex",
                    "getsalvage",
                    "pathexists",
                    "setrevealed"
                ]
                .contains(&name),
                "implemented native {name} reported unavailable"
            );
        }
    }
    Some(sim)
}

#[test]
fn set_revealed_forces_sensor_scans_through_abl() {
    let Some(sim) = run_program("var\n real[2] point;", "setRevealed(0, 100.0, point);") else {
        return;
    };
    let scanned = sim
        .world
        .sensors
        .teams
        .iter()
        .flatten()
        .filter_map(|&id| sim.world.sensors.pool.get(id))
        .filter(|s| s.last_scan_update == sim.world.clock.scenario_time)
        .count();
    assert!(scanned > 0, "the script forces registered sensors to scan");
}

#[test]
fn salvage_arrays_and_path_query_run_through_abl() {
    let Some(sim) = run_program(
        "var\n integer[2] items; integer[2] quantities;",
        "if not setSalvage(0, 1, 2) then setGlobalValue(3, 1.0); endif; getSalvage(0, 2, items, quantities); if (items[0] == -1) and (items[1] == -1) and (quantities[0] == -1) and (quantities[1] == -1) then setGlobalValue(0, 1.0); endif; if pathExists(0, -1, -1, -1, -1) == 0 then setGlobalValue(1, 1.0); endif;",
    ) else { return };
    assert_eq!(sim.world.mission.global_values[0], 1.0);
    assert_eq!(sim.world.mission.global_values[1], 1.0);
    assert_eq!(sim.world.mission.global_values[3], 1.0);
    let cell = sim
        .world
        .objects
        .movers
        .iter()
        .filter_map(|h| sim.world.objects.get(*h))
        .map(|o| o.c.cell)
        .find(|&(row, col)| {
            sim.world.terrain.move_data().global[0].calc_area(row.into(), col.into()) >= 0
        })
        .expect("mission has a mover in a mapped area");
    let call = format!(
        "if pathExists(0, {}, {}, {}, {}) == 1 then setGlobalValue(2, 1.0); endif;",
        cell.0, cell.1, cell.0, cell.1
    );
    let sim = run(&call).expect("data available");
    assert_eq!(sim.world.mission.global_values[2], 1.0);
}

#[test]
fn script_strikes_create_artillery_with_requested_delay() {
    for (call, expected) in [
        ("callstrike(1, 0, 1.0, 2.0, 3.0, false);", None),
        ("callstrikeex(1, 0, 1.0, 2.0, 3.0, false, 5.0);", Some(5.0)),
    ] {
        let Some(sim) = run(call) else { return };
        let object = sim
            .world
            .objects
            .artillery
            .iter()
            .filter_map(|h| sim.world.objects.get(*h))
            .find(|o| o.exists())
            .expect("script created strike");
        let Kind::Artillery(a) = &object.kind else {
            panic!("artillery")
        };
        if let Some(seconds) = expected {
            assert_eq!(a.time_to_impact, seconds);
        }
        assert_eq!((object.c.position.x, object.c.position.y), (1.0, 2.0));
    }
}

#[test]
fn script_strike_reaches_impact() {
    let Some(mut sim) = run("callstrikeex(1, 0, 1.0, 2.0, 3.0, false, 7.0);") else {
        return;
    };
    sim.world.mission.brain = None;
    let mut landed = false;
    let mut impact_effects = 0;
    let mut impact_sounds = Vec::new();
    let mut incoming_sounds = 0;
    for frame in 2..242 {
        let output = sim
            .step(FrameInput::fixed(
                30,
                frame,
                Rc::new(AllVisible),
                Vec::new(),
            ))
            .expect("step");
        for event in &output.events {
            if let mc2_sim::SimEvent::Sound {
                id,
                at: Some(_),
                allow_dupes: true,
                ..
            } = event
            {
                incoming_sounds +=
                    usize::from(*id == mc2_sim::combat::artillery::INCOMING_AIRSTRIKE);
            }
            if let mc2_sim::SimEvent::Explosion {
                effect_id: 50,
                sound_id,
                at,
                ..
            } = event
            {
                if (at.x, at.y) == (1.0, 2.0) {
                    impact_effects += 1;
                    impact_sounds.push(*sound_id);
                }
            }
        }
        landed |= sim
            .world
            .objects
            .artillery
            .iter()
            .filter_map(|h| sim.world.objects.get(*h))
            .any(|o| matches!(&o.kind, Kind::Artillery(a) if a.boom));
    }
    assert!(landed, "the ABL strike reached impact");
    assert_eq!(impact_effects, 1, "one large-airstrike impact effect");
    // The impact event carries the sound id of its effect's ExplosionType (carnage.cpp:677-678). LARGE_AIRSTRIKE_ID is a gosFX-only
    // effect in C++ (artlry.cpp:1533,1769: no createExplosion, no ExplosionType), so the faithful id is None and the audible
    // strike cue is the INCOMING_AIRSTRIKE sample raised before impact.
    assert_eq!(
        impact_sounds,
        vec![sim.world.objects.explosion_sound_id(50)]
    );
    assert_eq!(impact_sounds, vec![None]);
    assert_eq!(
        incoming_sounds, 1,
        "one positional INCOMING_AIRSTRIKE sample"
    );
}
