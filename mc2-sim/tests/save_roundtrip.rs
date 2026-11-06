// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Save/load round trips (DESIGN.md section 9): a save is taken from one world and loaded over a freshly initialised base world of
//! the same mission. Tests that need the game data skip cleanly when `MC2_DATA` is unset.

use mc2_abl::{Config, DefaultHost, Env, MemorySource};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use mc2_sim::mission::init::{load_world as init_world, LoadOutcome, LoadSpec};
use mc2_sim::mission::pilots::{setup_pool, PilotLoad};
use mc2_sim::mission::spawn::{replay_spawn, SpawnRecord};
use mc2_sim::object::Kind;
use mc2_sim::rng::RngMode;
use mc2_sim::save::{load_world, save_world};
use mc2_sim::Sim;
use mc2_sim_data::pilot::WarriorProfile;
use std::rc::Rc;

fn vfs() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty())?;
    Some(Vfs::from_system_cfg(std::path::Path::new(&root)).expect("system.cfg"))
}

fn init(vfs: &mut Vfs, mission: &str, seed: u32) -> LoadOutcome {
    init_world(
        vfs,
        &LoadSpec::quickstart(mission, seed, RngMode::LegacyMsvc),
    )
    .unwrap()
}

/// `Sim::new` over a world from [`load_world`] (a save load, not a mission load: it keeps the base's hook report).
fn sim_of(world: mc2_sim::World) -> Sim {
    Sim::new(LoadOutcome {
        complete: world.load_is_complete(),
        missing_hooks: world.unported_load_hooks.clone(),
        world,
    })
}

/// The initial state of the three test missions survives a save, a load over a base of another seed (whose random draws the
/// save overwrites), and a second save, byte for byte. The loaded world's digest equals the original's.
///
/// Needs `MC2_DATA`; skips without it. Not ignored: it compares bytes and digests, which do not need a complete load (only
/// `Sim::step` does). The sensor and pilot movement state are in the bytes compared here.
#[test]
fn init_state_round_trips_over_a_base_of_another_seed() {
    let Some(mut vfs) = vfs() else {
        eprintln!("MC2_DATA not set; skipping the save round trip");
        return;
    };
    for mission in ["mc2_01", "mc2_05", "mc2_17"] {
        let mut original = init(&mut vfs, mission, 1);
        // `Mover::copyTo`/`copyFrom` save `unitGroup` (code/mover.cpp:7318,7478).
        // Give one home mover a non-default Ctrl+digit membership so byte equality cannot hide
        // a dropped field.
        let grouped = original
            .world
            .objects
            .movers
            .iter()
            .copied()
            .find(|handle| {
                original
                    .world
                    .objects
                    .get(*handle)
                    .is_some_and(|obj| match &obj.kind {
                        Kind::Mech(m) => m.m.commander.0 == 0,
                        Kind::Vehicle(v) => v.m.commander.0 == 0,
                        _ => false,
                    })
            })
            .expect("mission has a home mover");
        let grouped_part = original.world.objects.get(grouped).unwrap().c.part_id;
        original.world.mission.unit_groups.insert(grouped_part, 7);
        assert!(
            original
                .world
                .mission
                .unit_groups
                .keys()
                .all(|part_id| original.world.objects.find_by_part_id(*part_id).is_some()),
            "{mission}: force-group save map contains only final mover part ids"
        );
        let bytes = save_world(&original.world);
        let original_digest = Sim::new(original).digest(0, 2);

        let base = init(&mut vfs, mission, 2);
        let loaded = load_world(base.world, Some(&mut vfs), &bytes)
            .unwrap_or_else(|e| panic!("{mission}: {e}"));
        let loaded_group = loaded.mission.unit_groups.get(&grouped_part).copied();
        assert_eq!(loaded_group, Some(7), "{mission}: saved force group");
        assert!(
            loaded
                .mission
                .unit_groups
                .keys()
                .all(|part_id| loaded.objects.find_by_part_id(*part_id).is_some()),
            "{mission}: loaded force-group save map contains only final mover part ids"
        );
        assert_eq!(save_world(&loaded), bytes, "{mission}: re-save differs");
        assert_eq!(
            sim_of(loaded).digest(0, 2),
            original_digest,
            "{mission}: digest after load differs"
        );
    }
}

/// A world with no game data, only the mission name set: every section is written and read, so the round trip is exercised
/// without `MC2_DATA`. Compared as bytes, so the test does not depend on the digest.
#[test]
fn empty_world_round_trips() {
    let mut world = mc2_sim::World::new(RngMode::LegacyMsvc, 1);
    world.mission.name = "mc2_01".into();
    let bytes = save_world(&world);
    let mut base = mc2_sim::World::new(RngMode::Pcg32, 9);
    base.mission.name = "mc2_01".into();
    let loaded = load_world(base, None, &bytes).unwrap();
    assert_eq!(save_world(&loaded), bytes);
}

/// Mid-mission state that the save used to drop (DESIGN.md section 9): a pilot's in-flight lane B path (`LaneBMoveStandin`, the
/// movement inputs the next frame reads) and the sensor pool with its contacts and round-robin team index. A load restores each
/// field and a re-save gives the same bytes. Needs no game data.
#[test]
fn mid_mission_movement_and_sensor_state_round_trips() {
    use mc2_path::{MovePath, PathStep, Vec3};
    use mc2_sim::ai::pilot::{LaneBMoveStandin, Pilot};
    use mc2_sim::ai::sensors::{Sensor, SensorManager};
    use mc2_sim::ids::ObjectHandle;

    let at = |x: f32| Vec3 { x, y: 0.5, z: -2.0 };
    let path = MovePath {
        goal: at(40.0),
        target: at(41.0),
        num_steps: 2,
        num_steps_when_not_paused: 2,
        cur_step: 1,
        cost: 7,
        steps: vec![
            PathStep {
                cell: [3, 4],
                distance_to_goal: 12.5,
                destination: at(10.0),
                direction: 2,
                area: 9,
            },
            PathStep {
                cell: [5, 4],
                distance_to_goal: 0.25,
                destination: at(40.0),
                direction: 4,
                area: 9,
            },
        ],
        marked: true,
        global_step: 5,
    };
    let mut world = mc2_sim::World::new(RngMode::LegacyMsvc, 1);
    world.mission.name = "mc2_01".into();
    world.pilots.slots.resize_with(4, Pilot::default);
    world.pilots.slots[3].lane_b_move = LaneBMoveStandin {
        paths: [path, MovePath::new()],
        goal_type: 2,
        path_type: 1,
        speed_state: 3,
        speed_throttle: 80,
        run: true,
        goal: at(40.0),
        has_goal: true,
        move_state: 4,
        move_state_goal: 5,
        is_yielding: true,
        wait_for_point_time: 0.5,
        ..LaneBMoveStandin::default()
    };
    let mut sensors = SensorManager::default();
    sensors.pool.push(Sensor {
        id: 0,
        owner: ObjectHandle(7),
        team_index: 1,
        range: 150.0,
        broken: false,
        inventory_index: 2,
        has_los_capability: true,
        not_shutdown: true,
        team_contacts_enabled: true,
        next_scan_update: 1.25,
        last_scan_update: 0.75,
        contacts: vec![(ObjectHandle(9), 4u8), (ObjectHandle(11), 2u8)],
    });
    sensors.teams[1].push(0);
    sensors.team_to_update = 1;
    sensors.enemy_in_los = false;
    world.sensors = sensors;

    let bytes = save_world(&world);
    let mut base = mc2_sim::World::new(RngMode::Pcg32, 9);
    base.mission.name = "mc2_01".into();
    let loaded = load_world(base, None, &bytes).unwrap();
    assert_eq!(save_world(&loaded), bytes, "re-save differs");
    assert_eq!(
        loaded.pilots.slots[3].lane_b_move,
        world.pilots.slots[3].lane_b_move
    );
    assert_eq!(
        loaded.sensors.pool[0].contacts,
        world.sensors.pool[0].contacts
    );
    assert_eq!(loaded.sensors.teams[1], vec![0]);
    assert_eq!(loaded.sensors.team_to_update, 1);
    assert!(!loaded.sensors.enemy_in_los);
}

/// A save is refused for another mission, a truncated save, and trailing bytes, and for a bad header.
#[test]
fn bad_saves_are_refused() {
    let mut world = mc2_sim::World::new(RngMode::LegacyMsvc, 1);
    world.mission.name = "mc2_01".into();
    let bytes = save_world(&world);
    let base = |name: &str| {
        let mut w = mc2_sim::World::new(RngMode::LegacyMsvc, 1);
        w.mission.name = name.into();
        w
    };
    assert!(
        load_world(base("mc2_05"), None, &bytes).is_err(),
        "wrong mission accepted"
    );
    assert!(
        load_world(base("mc2_01"), None, &bytes[..bytes.len() - 1]).is_err(),
        "truncated save accepted"
    );
    let mut extra = bytes.clone();
    extra.push(0);
    assert!(
        load_world(base("mc2_01"), None, &extra).is_err(),
        "trailing bytes accepted"
    );
    assert!(load_world(base("mc2_01"), None, b"not a save").is_err());
}

/// The acceptance test: save at frame k, load, run to N; the digest equals the uninterrupted run at every frame after k.
///
/// Runs on a complete load; `Sim::allow_incomplete` is not used.
#[test]
fn mc2_01_save_then_continue_matches_the_uninterrupted_run() {
    use mc2_sim::visibility::AllVisible;
    use mc2_sim::FrameInput;

    let Some(mut vfs) = vfs() else {
        return;
    };
    const FPS: u32 = 30;
    const LAST: u64 = 120;
    let frame_input = |f: u64| FrameInput::fixed(FPS, f, Rc::new(AllVisible), Vec::new());

    let mut uninterrupted = Sim::new(init(&mut vfs, "mc2_01", 1));
    let mut digests = Vec::new();
    for f in 1..=LAST {
        uninterrupted.step(frame_input(f)).unwrap();
        digests.push(uninterrupted.digest(f as i64, 2));
    }

    for k in [30u64, 75] {
        let mut run = Sim::new(init(&mut vfs, "mc2_01", 1));
        for f in 1..=k {
            run.step(frame_input(f)).unwrap();
        }
        let bytes = save_world(&run.world);
        let base = init(&mut vfs, "mc2_01", 1);
        let mut resumed = sim_of(load_world(base.world, Some(&mut vfs), &bytes).unwrap());
        for f in k + 1..=LAST {
            resumed.step(frame_input(f)).unwrap();
            assert_eq!(
                resumed.digest(f as i64, 2),
                digests[f as usize - 1],
                "save at {k}: frame {f} differs"
            );
        }
    }
}

/// A world with an ABL environment, a pilot pool and the pilot-load table of a mission, and no reinforcements. `pilot.fit` is
/// cached without a profile, which `add_mover` accepts as the C++ default pilot.
fn world_for_reinforcements() -> mc2_sim::World {
    let mut w = mc2_sim::World::new(RngMode::LegacyMsvc, 1);
    w.mission.name = "mc2_01".into();
    let mut sources = MemorySource::new();
    sources.insert(
        "data/missions/warriors/brain.abl",
        "module brain : integer;\ncode\nendmodule.\n",
    );
    w.abl = Some(Env::new(Config::default(), sources, Box::new(DefaultHost)));
    setup_pool(&mut w.pilots, 0.0);
    w.mission.pilot_loads = vec![PilotLoad::default(); w.pilots.slots.len()];
    w.data.pilot_profiles.insert("pilot.fit".into(), None);
    w
}

/// A reinforcement pilot (`add_mover`'s spawn log, lane E) is structure the save depends on: its ABL module and instance must exist
/// in the base world, or `Env::load_state` refuses the save. `load_world` replays the record on the base, so the loaded world holds
/// the same instance, and a second save is byte-identical.
#[test]
fn reinforcement_structure_is_replayed_on_load() {
    let rec = SpawnRecord {
        pilot_slot: 1,
        pilot_file: "Pilot.fit".into(),
        brain: "brain".into(),
        object_number: 0,
    };
    let mut original = world_for_reinforcements();
    replay_spawn(&mut original, None, &rec).unwrap();
    assert!(original.pilots.slots[1].brain.is_some());
    let bytes = save_world(&original);

    let base = world_for_reinforcements();
    assert!(base.pilots.slots[1].brain.is_none());
    let loaded =
        load_world(base, None, &bytes).unwrap_or_else(|e| panic!("reinforcement save: {e}"));
    assert_eq!(loaded.mission.spawn_log, vec![rec]);
    assert!(loaded.pilots.slots[1].brain.is_some());
    assert_eq!(save_world(&loaded), bytes, "re-save differs");
}

/// A reinforcement whose pilot file the base world never loaded is refused when no game data is given, not loaded with a default
/// pilot.
#[test]
fn reinforcement_with_an_unloaded_pilot_file_is_refused() {
    let rec = SpawnRecord {
        pilot_slot: 1,
        pilot_file: "Pilot.fit".into(),
        brain: "brain".into(),
        object_number: 0,
    };
    let mut original = world_for_reinforcements();
    replay_spawn(&mut original, None, &rec).unwrap();
    let bytes = save_world(&original);

    let mut base = world_for_reinforcements();
    base.data.pilot_profiles.clear();
    let err = load_world(base, None, &bytes)
        .err()
        .expect("load must fail");
    assert!(err.0.contains("not loaded"), "{err}");
}

/// A reinforcement whose mech type brings a new appearance type (the spawn loads it with `prepare_spawn`) is replayed on load with
/// that type, so `AppearanceSystem::load_into` accepts the save. Before the replay the base has no mech appearance type, which is
/// what the save-time world had not got either. Object 246 is a mech of the shipped data and `pmwjinx` a pilot file that exists
/// (`init_plan.rs`, `add_mover_matches_the_oracle`).
#[test]
fn reinforcement_with_a_new_appearance_type_loads_from_the_game_data() {
    let Some(mut vfs) = vfs() else {
        return;
    };
    let rec = SpawnRecord {
        pilot_slot: 1,
        pilot_file: "pmwjinx".into(),
        brain: "brain".into(),
        object_number: 246,
    };
    let mut original = world_for_reinforcements();
    replay_spawn(&mut original, Some(&mut vfs), &rec).unwrap();
    assert!(!original.appearance.mech_types.is_empty());
    let bytes = save_world(&original);

    let base = world_for_reinforcements();
    assert!(base.appearance.mech_types.is_empty());
    let loaded = load_world(base, Some(&mut vfs), &bytes)
        .unwrap_or_else(|e| panic!("reinforcement with a new appearance type: {e}"));
    assert_eq!(
        loaded.appearance.mech_types.len(),
        original.appearance.mech_types.len()
    );
    assert_eq!(save_world(&loaded), bytes, "re-save differs");
}

/// A pilot slot the save records without a profile stays without one, even where the base world has a profile for that slot. The
/// presence flag is what `PilotLoad::load_into` reads; a plain `Vec` overlay would hand the base profile back.
#[test]
fn cleared_pilot_slot_stays_without_a_profile() {
    const FIT: &str = "FITini\n\n[General]\nst Name = \"Force\"\n[PersonalityTraits]\n\
        c Professionalism = 50\nc Decorum = 50\nc Aggressiveness = 40\nc Courage = 50\n\
        [Skills]\nc Piloting = 56\nc Sensors = 43\nc Gunnery = 52\n[Status]\nc Wounds = 0\nFITEnd\n";
    let mut fit = FitIni::from_bytes(FIT.as_bytes().to_vec()).expect("FIT header");
    let profile = Rc::new(WarriorProfile::from_fit("pmp_base", &mut fit).expect("profile"));

    let original = world_for_reinforcements();
    assert!(original.mission.pilot_loads[1].profile.is_none());
    let bytes = save_world(&original);

    let mut base = world_for_reinforcements();
    base.mission.pilot_loads[1].profile = Some(profile);
    let loaded = load_world(base, None, &bytes).unwrap();
    assert!(loaded.mission.pilot_loads[1].profile.is_none());
    assert_eq!(save_world(&loaded), bytes, "re-save differs");
}

/// A paused world with no movers, carrying the mission state a save must keep across frames: a script timer that expires part way
/// through the run, a global value, and the time limit. Seeded differently for the base, so the save has to overwrite its draws.
fn paused_mission_world(seed: u32) -> mc2_sim::World {
    let mut w = mc2_sim::World::new(RngMode::Pcg32, seed);
    w.mission.name = "mc2_01".into();
    w.mission.paused = true;
    w.mission.time_limit = 2.0;
    w.mission.timers.get(7).expect("timer 7").set(0.5);
    mc2_sim::mission::bindings::set_global_value(&mut w.mission, 3, 2.5);
    w
}

/// One paused frame of `Sim::step` (sim.rs): the clock, the script timers, the appearance-only update and the termination logic,
/// in the same order. `PathManager::update` is left out: `Sim::step` runs it even while paused, and this helper compares only the
/// termination and clock state. The mc2_01 acceptance test stays ignored until lane C's load hook lands.
fn paused_frame(w: &mut mc2_sim::World, input: &mc2_sim::FrameInput) {
    w.visibility = input.visibility.clone();
    w.clock
        .begin_frame(input.frame_rate, input.now_ms, w.mission.paused);
    w.mission.actual_time = w.clock.scenario_time;
    w.mission.timers.update(w.clock.frame_length);
    mc2_sim::object::update_appearances_only(w);
    mc2_sim::mission::update_termination(w);
}

/// Save at frame k, load, continue: every later frame's whole save equals the uninterrupted run's, byte for byte. The digest is
/// not used: `Sim::digest` needs lane D's pilot digest. The whole save covers more state than the digest anyway.
#[test]
fn paused_mission_state_resumes_identically() {
    use mc2_sim::visibility::AllVisible;
    use mc2_sim::FrameInput;

    const FPS: u32 = 30;
    const LAST: u64 = 40;
    let frame_input = |f: u64| FrameInput::fixed(FPS, f, Rc::new(AllVisible), Vec::new());

    let mut uninterrupted = paused_mission_world(7);
    let mut saves = Vec::new();
    for f in 1..=LAST {
        paused_frame(&mut uninterrupted, &frame_input(f));
        saves.push(save_world(&uninterrupted));
    }
    assert!(
        saves.last() != saves.first(),
        "the run must change the saved state"
    );

    // 12 is before the timer (0.5 s = 15 frames at 30 fps) expires, 27 after it.
    for k in [12u64, 27] {
        let mut run = paused_mission_world(7);
        for f in 1..=k {
            paused_frame(&mut run, &frame_input(f));
        }
        let bytes = save_world(&run);
        let mut resumed = load_world(paused_mission_world(9), None, &bytes)
            .unwrap_or_else(|e| panic!("save at {k}: {e}"));
        for f in k + 1..=LAST {
            paused_frame(&mut resumed, &frame_input(f));
            assert!(
                save_world(&resumed) == saves[f as usize - 1],
                "save at {k}: frame {f} differs"
            );
        }
    }
}
