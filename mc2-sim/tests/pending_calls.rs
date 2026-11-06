// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Lane D's consumer of `Pilot::pending_calls` on a real mission load (mc2_01): the load is complete, and the calls lane C's damage
//! code queues are run where they are made (warrior.cpp:4632-4690, 6941-6975). Needs `MC2_DATA`.

mod common;

use common::{data_root, load_blocker, rng_by_name};
use mc2_sim::ai::AiUnavailable;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::Kind;
use mc2_sim::{SimEvent, World};

fn pilot_slot(w: &World, h: ObjectHandle) -> Option<u16> {
    match &w.objects.get(h)?.kind {
        Kind::Mech(m) => Some(m.m.pilot_slot),
        Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    }
}

fn mover_of_slot(w: &World, slot: u16) -> ObjectHandle {
    w.objects
        .movers
        .iter()
        .copied()
        .find(|h| pilot_slot(w, *h) == Some(slot))
        .unwrap_or_else(|| panic!("no mover flies pilot slot {slot}"))
}

#[test]
fn mc2_01_load_is_complete_and_pilot_alarms_reach_their_brains() {
    let Some(root) = data_root() else { return };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = rng_by_name("msvc");
    let outcome =
        load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_01", seed, mode)).expect("load");
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    assert!(outcome.missing_hooks.is_empty());
    let mut w = outcome.world;

    // Pilots 1..3 fly with a brain that defines `handlekilledtarget` (alarm 12) but not `handlecollision` (10); pilot 4's brain
    // defines no alarm handler at all (probed from the loaded ABL environment, not assumed).
    let (with_handlers, without) = (mover_of_slot(&w, 1), mover_of_slot(&w, 4));
    let env_has = |w: &World, slot: u16, name: &str| {
        let inst = w.pilots.get(slot).and_then(|p| p.brain).expect("brain");
        w.abl
            .as_ref()
            .unwrap()
            .find_function(inst, name, true)
            .is_some()
    };
    assert!(env_has(&w, 1, "handlekilledtarget"));
    assert!(!env_has(&w, 1, "handlecollision"));
    assert!(!env_has(&w, 4, "handlekilledtarget"));

    for h in [with_handlers, without] {
        if let Some(o) = w.objects.get_mut(h) {
            o.c.flags |= mc2_sim::object::flags::AWAKE;
        }
    }
    w.events.clear();
    w.pilots.slots[1].brain_statements = 0;
    w.pilots.slots[4].brain_statements = 0;
    // an alarm with no ABL handler in the brain is a no-op, as `brainAlarmCallback[alarm]` is NULL (warrior.cpp:4675)
    mc2_sim::combat::fx::handle_alarm(&mut w, with_handlers, 10, 5);
    mc2_sim::combat::fx::handle_alarm(&mut w, without, 12, 5);
    assert_eq!(w.pilots.slots[1].brain_statements, 0);
    assert_eq!(w.pilots.slots[4].brain_statements, 0);
    // with the handler the brain's `handlekilledtarget` runs (`brain->execute(NULL, brainAlarmCallback[alarmCode])`,
    // warrior.cpp:4675-4688): ABL statements execute, and nothing is reported unported
    mc2_sim::combat::fx::handle_alarm(&mut w, with_handlers, 12, 5);
    assert!(
        w.pilots.slots[1].brain_statements > 0,
        "the ABL alarm handler must run"
    );
    assert!(
        !w.events.iter().any(|e| matches!(
            e,
            SimEvent::AiUnavailable {
                what: AiUnavailable::BrainAlarmHandler { .. },
                ..
            }
        )),
        "{:?}",
        w.events
    );
    assert!(w.pilots.slots.iter().all(|p| p.pending_calls.is_empty()));
}
