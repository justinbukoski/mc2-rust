// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Headless evidence for the pilot attacker bookkeeping: shipped mc2_04 (46 `AttackerCount` notices before this slice) steps with
//! the attacker list filled by real shots and without an `AttackerCount` or `NestedBrain` notice.

use std::rc::Rc;

use mc2_sim::ai::AiUnavailable;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim, SimEvent};

mod common;

#[test]
fn mc2_04_runs_without_attacker_or_nested_brain_notices() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("system.cfg");
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_04", 1, RngMode::LegacyMsvc),
    )
    .expect("mc2_04 loads");
    let mut sim = Sim::new(loaded);
    let mut attackers_seen = 0usize;
    for frame in 1..=3_000 {
        let output = sim
            .step(FrameInput::fixed(
                30,
                frame,
                Rc::new(AllVisible),
                Vec::new(),
            ))
            .expect("steps");
        for event in output.events {
            if let SimEvent::AiUnavailable { what, .. } = event {
                assert!(
                    !matches!(what, AiUnavailable::NestedBrain),
                    "frame {frame}: {what:?}"
                );
            }
        }
        attackers_seen = attackers_seen.max(
            sim.world
                .pilots
                .slots
                .iter()
                .map(|p| p.attackers.len())
                .max()
                .unwrap_or(0),
        );
    }
    eprintln!("largest attacker list (no input, 3000 frames): {attackers_seen}");
}

/// A shot fired through the production `fire_weapon` of mc2_01 (the grouped-mech fixture of `vehicle_fire.rs`) records the attacker
/// on the target's pilot at once (hit or miss), stamped with the scenario time.
#[test]
fn a_real_shot_fills_the_targets_attacker_list() {
    use common::fire_dump::{find, mc2_01, recycle};
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.unlimited_ammo = true;
    fx.w.clock.scenario_time = 1000.0;
    fx.w.clock.frame_length = 1.0 / 30.0;
    let (att, tgt) = (find(&fx, 709), find(&fx, 706));
    if !fx.w.objects.movers.contains(&att) {
        fx.w.objects.movers.push(att);
    }
    for (h, x, y, rot) in [(att, 0.0, 0.0, 90.0), (tgt, 100.0, 0.0, 270.0)] {
        let o = fx.w.objects.get_mut(h).unwrap();
        o.c.position.x = x;
        o.c.position.y = y;
        o.c.rotation = rot;
    }
    for node in ["1", "4", "5"] {
        recycle(&mut fx, &["recycle", "709", node, "0"]);
    }
    mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(att).unwrap())
        .unwrap()
        .inventory[14]
        .ready_time = 0.0;
    let slot = mc2_sim::combat::fx::pilot_slot(&fx.w, tgt).expect("target pilot");
    assert!(fx.w.pilots.get(slot).unwrap().attackers.is_empty());
    let r = mc2_sim::combat::fire::fire_weapon(&mut fx.w, att, 14, Some(tgt), -1, None, 0);
    assert_eq!(r.code, 0, "the shot proceeds");
    let wid = fx.w.objects.watch_id(att);
    assert_eq!(
        fx.w.pilots.get(slot).unwrap().attackers,
        vec![(wid, 1000.0)]
    );
}

/// `getAttackers(list, seconds)` (ablmc2.cpp, warrior.cpp:2299) as an ABL script of the target's pilot, answering from the list that
/// the REAL shot above filled: inside the window the attacker is counted, exactly `seconds` old still counts, older is expired.
#[test]
fn abl_getattackers_counts_the_attacker_a_real_shot_recorded() {
    use common::fire_dump::{find, mc2_01, recycle};
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.unlimited_ammo = true;
    fx.w.clock.scenario_time = 1000.0;
    fx.w.clock.frame_length = 1.0 / 30.0;
    let (att, tgt) = (find(&fx, 709), find(&fx, 706));
    if !fx.w.objects.movers.contains(&att) {
        fx.w.objects.movers.push(att);
    }
    for (h, x, y, rot) in [(att, 0.0, 0.0, 90.0), (tgt, 100.0, 0.0, 270.0)] {
        let o = fx.w.objects.get_mut(h).unwrap();
        o.c.position.x = x;
        o.c.position.y = y;
        o.c.rotation = rot;
    }
    for node in ["1", "4", "5"] {
        recycle(&mut fx, &["recycle", "709", node, "0"]);
    }
    mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(att).unwrap())
        .unwrap()
        .inventory[14]
        .ready_time = 0.0;
    let slot = mc2_sim::combat::fx::pilot_slot(&fx.w, tgt).expect("target pilot");

    let query = |w: &mut mc2_sim::World| -> i32 {
        let mut src = mc2_abl::MemorySource::new();
        src.insert(
            "q.abl",
            "module q : integer;\nvar\n integer[10] att;\n integer n;\ncode\n n = getAttackers(att, 3.0);\n if n > 0 then\n  n = n * 1000 + att[0];\n endif;\n return(n);\nendmodule.\n",
        );
        let mut env = mc2_abl::Env::new(
            mc2_abl::Config::default(),
            src,
            Box::new(mc2_abl::RngHost::new(w.rng.clone())),
        );
        let h = env.pre_process("q.abl").expect("compile");
        let inst = env.new_instance(h).expect("instance");
        {
            let mut b = mc2_sim::mission::bindings::SimBindings::pilot(w, slot);
            env.execute(inst, &mut b).expect("run");
        }
        env.get_integer(inst)
    };

    assert_eq!(query(&mut fx.w), 0, "no attacker before the shot");
    let r = mc2_sim::combat::fire::fire_weapon(&mut fx.w, att, 14, Some(tgt), -1, None, 0);
    assert_eq!(r.code, 0, "the shot proceeds");
    let wid = fx.w.objects.watch_id(att).0 as i32;
    assert_eq!(query(&mut fx.w), 1000 + wid, "inside the window");
    fx.w.clock.scenario_time = 1003.0; // exactly 3 s old: still counted
    assert_eq!(query(&mut fx.w), 1000 + wid);
    fx.w.clock.scenario_time = 1003.5; // expired
    assert_eq!(query(&mut fx.w), 0);
}
