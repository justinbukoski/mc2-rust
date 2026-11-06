// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Lane E runtime fixes from the Codex review of the objectives, termination and campaign outcome (feat/rust-sim-e3).
//! Each test cites the C++ it pins. The mover pilot-status mapping is private, so it is tested in `mission::objective`.

use mc2_sim::ai::pilot::Pilot;
use mc2_sim::ai::team::Team;
use mc2_sim::ids::{CommanderId, ObjectHandle, WatchId};
use mc2_sim::mission::bindings::{
    check_objective_status, get_global_value, objective_status_changed, run_mission_brain,
    set_global_value,
};
use mc2_sim::mission::objective::{
    team_status, ObjectiveEntry, ObjectiveRuntime, ObjectiveStatus, ResolvedAction,
    ResolvedCondition, TeamObjectiveSet,
};
use mc2_sim::mission::objective_eval::{
    condition_status, Betty, BuildingView, MoverView, ObjectiveWorld,
};
use mc2_sim::mission::outcome::{self, KilledKind, MissionResult, WarriorStatus};
use mc2_sim::mission::result;
use mc2_sim::mission::MissionState;
use mc2_sim::mission::MAX_GLOBAL_MISSION_VALUES;
use mc2_sim::mover::MechState;
use mc2_sim::object::{flags, Common, Kind, Obj, ObjectClass};
use mc2_sim::rng::RngMode;
use mc2_sim::World;
use mc2_sim_data::mission::{
    Action, BooleanFlag, Condition, ObjectRef, Objective, ObjectiveModel, ObjectiveOrigin,
};

const S: ObjectiveStatus = ObjectiveStatus::Successful;
const U: ObjectiveStatus = ObjectiveStatus::Undetermined;
const F: ObjectiveStatus = ObjectiveStatus::Failed;

fn definition(priority: u32) -> Objective {
    Objective {
        origin: ObjectiveOrigin::Block { index: 0 },
        title: String::new(),
        title_use_resource_string: false,
        title_resource_string_id: 0,
        description: String::new(),
        description_use_resource_string: false,
        description_resource_string_id: 0,
        priority,
        resource_points: 0,
        previous_primary_objective_must_be_complete: false,
        all_previous_primary_objectives_must_be_complete: false,
        display_marker: false,
        marker_x: 0.0,
        marker_y: 0.0,
        marker_text: String::new(),
        is_hidden_trigger: false,
        activate_on_flag: false,
        activate_flag_id: String::new(),
        reset_status_on_flag: false,
        reset_status_flag_id: String::new(),
        model: ObjectiveModel::default(),
        is_active: true,
        activation_time: -1.0,
        resolved: false,
        resolved_status: 0,
        changed_status: false,
        conditions: Vec::new(),
        actions: Vec::new(),
        failure_conditions: Vec::new(),
        failure_actions: Vec::new(),
    }
}

/// An active objective with one condition (and optional actions), as `Team::loadObjectives` leaves it.
fn entry(priority: u32, condition: Condition, actions: Vec<Action>) -> ObjectiveEntry {
    let mut def = definition(priority);
    def.conditions = vec![condition.clone()];
    ObjectiveEntry {
        def,
        conditions: vec![ResolvedCondition {
            condition,
            wid: WatchId::NONE,
        }],
        actions: actions
            .into_iter()
            .map(|action| ResolvedAction {
                action,
                wid: WatchId::NONE,
            })
            .collect(),
        failure_conditions: Vec::new(),
        failure_actions: Vec::new(),
        runtime: ObjectiveRuntime {
            active: true,
            resolved: false,
            status: U,
            changed: false,
        },
    }
}

/// A world whose home team (team 0) has `entries` as its objectives, with no movers and no structures.
fn world_with(entries: Vec<ObjectiveEntry>) -> World {
    let mut w = World::new(RngMode::Pcg32, 1);
    w.teams.home = 0;
    w.mission.objectives = vec![TeamObjectiveSet {
        team: 0,
        boolean_flags: Vec::new(),
        entries,
        num_primary: 1,
    }];
    w
}

/// `DestroyNumberOfEnemyUnits` with no enemies is undetermined; `DestroyAllEnemyUnits` with no enemies is successful.
fn undetermined() -> Condition {
    Condition::DestroyNumberOfEnemyUnits { num: 1 }
}

fn successful() -> Condition {
    Condition::DestroyAllEnemyUnits
}

fn purchase(file: &str) -> Action {
    Action::MakeNewTechnologyAvailable {
        purchase_file_name: file.to_string(),
    }
}

/// `Mission::checkObjectiveStatus` (mission.cpp:3498-3513) asks only for the requested objective. Its actions run only when
/// it is itself evaluated, and the answer is this call's status, not the cached one (objective.cpp:1944-1989).
#[test]
fn check_objective_status_evaluates_only_the_requested_objective() {
    let mut w = world_with(vec![
        entry(1, undetermined(), Vec::new()),
        entry(1, successful(), vec![purchase("later.fit")]),
    ]);
    assert_eq!(check_objective_status(&mut w, 0), 0);
    assert!(
        w.mission.unlocked_purchase_files.is_empty(),
        "objective 1's action ran while asking about objective 0"
    );
    assert_eq!(check_objective_status(&mut w, 1), 1);
    assert_eq!(w.mission.unlocked_purchase_files, vec!["later.fit"]);
}

/// The answer is fresh: a stale cached `Failed` on an unresolved objective does not leak out (objective.cpp:1944-1989).
#[test]
fn check_objective_status_ignores_cached_status() {
    let mut e = entry(1, successful(), Vec::new());
    e.runtime.status = F;
    let mut w = world_with(vec![e]);
    assert_eq!(check_objective_status(&mut w, 0), 1);
    assert_eq!(
        w.mission.objectives[0].entries[0].runtime.status, S,
        "the evaluated status is written back"
    );
}

/// A qualifier that is not met decides the answer, and the objective's own actions do not run (objective.cpp:1958-1964).
#[test]
fn check_objective_status_applies_previous_primary_qualifier() {
    let mut second = entry(1, successful(), vec![purchase("gated.fit")]);
    second.def.previous_primary_objective_must_be_complete = true;
    let mut w = world_with(vec![entry(1, undetermined(), Vec::new()), second]);
    assert_eq!(check_objective_status(&mut w, 1), 0);
    assert!(w.mission.unlocked_purchase_files.is_empty());
}

/// Out-of-range indices are undetermined, as in `Mission::checkObjectiveStatus` (mission.cpp:3512).
#[test]
fn check_objective_status_out_of_range_is_undetermined() {
    let mut w = world_with(vec![entry(1, successful(), Vec::new())]);
    assert_eq!(check_objective_status(&mut w, -1), 0);
    assert_eq!(check_objective_status(&mut w, 5), 0);
}

/// Secondary objectives activate from their flag, not only primaries (objective.cpp:2203-2218). The flag keeps the mission open
/// only for a primary.
#[test]
fn secondary_objective_activates_from_its_flag() {
    let mut secondary = entry(2, successful(), Vec::new());
    secondary.runtime.active = false;
    secondary.def.is_active = false;
    secondary.def.activate_on_flag = true;
    secondary.def.activate_flag_id = "go".to_string();
    let mut w = world_with(vec![entry(1, successful(), Vec::new()), secondary]);
    w.mission.objectives[0].boolean_flags = vec![BooleanFlag {
        flag_id: "go".to_string(),
        value: true,
    }];
    assert_eq!(team_status(&mut w, 0), S);
    assert!(w.mission.objectives[0].entries[1].runtime.active);
}

/// An inactive primary keeps the mission open only when it can still activate (objective.cpp:2206-2209, the keep-open rule
/// inside the inactive branch at 2203-2218).
#[test]
fn inactive_primary_without_activation_flag_does_not_keep_mission_open() {
    let mut primary = entry(1, undetermined(), Vec::new());
    primary.runtime.active = false;
    let mut w = world_with(vec![entry(1, successful(), Vec::new()), primary]);
    assert_eq!(team_status(&mut w, 0), S);
}

/// `CaptureOrDestroySpecificStructure` succeeds only when the structure is destroyed or belongs to the objective's alignment
/// (objective.cpp:636-651). A structure merely captured by another team does not satisfy it.
#[test]
fn capture_or_destroy_structure_needs_own_team_or_destruction() {
    struct Buildings(Vec<(i32, BuildingView)>);
    impl ObjectiveWorld for Buildings {
        fn num_movers(&self) -> usize {
            0
        }
        fn mover(&self, _: usize) -> MoverView {
            unreachable!()
        }
        fn mover_by_ref(&self, _: &ObjectRef) -> Option<MoverView> {
            None
        }
        fn building_by_ref(&self, r: &ObjectRef) -> Option<BuildingView> {
            match r {
                ObjectRef::Wid(id) => self.0.iter().find(|(i, _)| i == id).map(|&(_, b)| b),
                ObjectRef::Position { .. } => None,
            }
        }
        fn home_commander_id(&self) -> i32 {
            0
        }
        fn actual_time(&self) -> f32 {
            0.0
        }
        fn time_limit(&self) -> f32 {
            -1.0
        }
        fn max_extract_unit_distance(&self) -> f32 {
            1280.0
        }
        fn play_betty(&mut self, _: Betty) {}
        fn show_objectives(&mut self) {}
        fn execute_action(&mut self, _: i32, _: &Action) {}
    }
    let structure = Condition::CaptureOrDestroySpecificStructure {
        structure: ObjectRef::Wid(7),
    };
    let view = |team_id, destroyed, captured| {
        Buildings(vec![(
            7,
            BuildingView {
                team_id,
                destroyed,
                captured,
            },
        )])
    };
    let check = |world: &Buildings| condition_status(world, &[], 1, &structure);
    assert_eq!(check(&view(2, false, true)), U, "captured by another team");
    assert_eq!(check(&view(1, false, false)), S, "already ours");
    assert_eq!(check(&view(2, true, false)), S, "destroyed");
}

/// The campaign outcome asks each objective for its status now, in list order (logisticsdata.cpp:1329-1336), not the cached
/// runtime status. Pilot kills, paint scheme and pilot state come from the mission, not from placeholders (logisticsdata.cpp:
/// 1351-1390, logisticspilot.cpp:307-345).
#[test]
fn outcome_reports_live_objective_status_pilot_kills_and_paint() {
    let mut stale = entry(1, successful(), Vec::new());
    stale.def.resource_points = 500;
    stale.runtime.status = F;
    let mut w = world_with(vec![stale]);

    // The home team's roster: one mech, pilot slot 1.
    let home_handle = ObjectHandle(1);
    let kill_handle = ObjectHandle(2);
    let mech = |slot: u16, move_level: u8| {
        let mut m = MechState::default();
        m.m.commander = CommanderId(0);
        m.m.flags.on_gui = true;
        m.m.pilot_slot = slot;
        m.m.move_level = move_level;
        m.variant_name = format!("mech{slot}");
        m
    };
    let mut home_common = Common::new(ObjectClass::BattleMech, home_handle);
    home_common.flags |= flags::EXISTS;
    let mut kill_common = Common::new(ObjectClass::BattleMech, kill_handle);
    kill_common.flags |= flags::EXISTS;
    w.objects.list = vec![
        None,
        Some(Obj {
            c: home_common,
            kind: Kind::Mech(Box::new(mech(1, 0))),
        }),
        Some(Obj {
            c: kill_common,
            kind: Kind::Mech(Box::new(mech(2, 0))),
        }),
    ];
    let home_wid = w.objects.watch_id(home_handle);
    let kill_wid = w.objects.watch_id(kill_handle);
    w.teams.teams = vec![Some(Team {
        roster: vec![home_wid],
        num_primary_objectives: 1,
    })];

    w.pilots.slots = vec![Pilot::default(), Pilot::default(), Pilot::default()];
    w.pilots.slots[1].num_mech_kills[1][1] = 2;
    w.pilots.slots[1].killed = vec![kill_wid];
    w.pilots.slots[1].status = WarriorStatus::Withdrawn;
    w.mission.pilot_loads = vec![Default::default(), Default::default()];
    w.mission.paint_schemes = vec![(home_wid, [0x11, 0x22, 0x33])];

    let report = outcome::build(&mut w);

    assert_eq!(report.objectives.len(), 1);
    assert_eq!(report.objectives[0].status, S, "live status, not cached F");
    assert_eq!(report.objectives[0].resource_points, 500);
    assert_eq!(report.movers.len(), 1);
    let mover = &report.movers[0];
    assert_eq!(mover.paint_scheme, [0x11, 0x22, 0x33]);
    assert_eq!(mover.pilot.mission_kills, [0, 2, 0, 0, 0, 0, 0]);
    assert_eq!(
        mover.pilot.killed,
        vec![KilledKind::BattleMech { air: false }]
    );
    assert_eq!(mover.pilot.status, WarriorStatus::Withdrawn);
    assert_eq!(report.result, MissionResult::Playing);
}

/// `globalMissionValues` has `MAX_GLOBAL_MISSION_VALUES` slots; the natives ignore other indices (ablmc2.cpp:3416, 3435).
#[test]
fn global_values_are_bounded_by_max_global_mission_values() {
    assert_eq!(MAX_GLOBAL_MISSION_VALUES, 50);
    let mut m = MissionState::default();
    assert_eq!(m.global_values.len(), MAX_GLOBAL_MISSION_VALUES);

    set_global_value(&mut m, 49, 3.0);
    assert_eq!(get_global_value(&m, 49), 3.0);

    set_global_value(&mut m, MAX_GLOBAL_MISSION_VALUES as i32, 9.0);
    set_global_value(&mut m, -1, 9.0);
    assert_eq!(get_global_value(&m, MAX_GLOBAL_MISSION_VALUES as i32), 0.0);
    assert_eq!(get_global_value(&m, -1), 0.0);
    assert!(m.global_values.iter().all(|&v| v == 0.0 || v == 3.0));
}

/// `handleKilledTarget` (warrior.cpp:7060-7120): a kill counts under the target's weight class and earns `KillSkill[class]`
/// (`gamesys.skills.kill_skill`, mover.cpp:2154). An air mech counts as a ground vehicle, a ground vehicle only with path locks
/// (infantry, gv->pathLocks), and any other object earns nothing. The counts and the list go to the attacker's pilot.
#[test]
fn kills_are_credited_to_the_attackers_pilot_by_class() {
    use mc2_sim::mission::pilots::{credit_kill, setup_pool, KillValues, PilotLoad};

    let mut w = World::new(RngMode::Pcg32, 1);
    let values = KillValues {
        mech_class_weights: [0.0, 35.0, 75.0, 75.0, 100.0],
        kill_skill: [45.0, 60.0, 70.0, 80.0, 20.0, 6.0],
    };
    setup_pool(&mut w.pilots, 0.0);
    w.mission.pilot_loads = vec![PilotLoad::default(); w.pilots.slots.len()];

    let add = |w: &mut World,
               handle: u32,
               tonnage: f32,
               air: bool,
               pilot: u16,
               path_locks: Option<bool>| {
        let handle = ObjectHandle(handle);
        let mut common = Common::new(ObjectClass::BattleMech, handle);
        common.flags |= flags::EXISTS;
        common.tonnage = tonnage;
        let kind = match path_locks {
            None => {
                let mut m = MechState::default();
                m.m.commander = CommanderId(0);
                m.m.pilot_slot = pilot;
                m.m.move_level = if air { 2 } else { 0 };
                Kind::Mech(Box::new(m))
            }
            Some(locks) => {
                let mut v = mc2_sim::mover::VehicleState::default();
                v.m.commander = CommanderId(0);
                v.m.pilot_slot = pilot;
                v.m.path_locks_enabled = locks;
                Kind::Vehicle(Box::new(v))
            }
        };
        let class = if path_locks.is_none() {
            ObjectClass::BattleMech
        } else {
            ObjectClass::GroundVehicle
        };
        common.class = class;
        let idx = handle.0 as usize;
        if w.objects.list.len() <= idx {
            w.objects.list.resize_with(idx + 1, || None);
        }
        w.objects.list[idx] = Some(Obj { c: common, kind });
        w.objects.watch_id(handle)
    };
    let attacker = add(&mut w, 1, 50.0, false, 1, None);
    let light = add(&mut w, 2, 30.0, false, 2, None);
    let assault = add(&mut w, 3, 90.0, false, 2, None);
    let air = add(&mut w, 4, 60.0, true, 2, None);
    let infantry = add(&mut w, 5, 0.5, false, 2, Some(true));
    let vehicle = add(&mut w, 6, 20.0, false, 2, Some(false));
    let _ = vehicle;
    for wid in [light, assault, air, infantry, vehicle] {
        let target = w.objects.by_watch_id(wid).expect("watch id");
        credit_kill(&mut w, attacker, target, &values);
    }

    let pilot = &w.pilots.slots[1];
    assert_eq!(
        pilot.num_mech_kills.map(|c| c[1]),
        [0, 1, 0, 0, 1, 2, 0],
        "light, assault, then air and infantry as ground"
    );
    assert_eq!(pilot.killed, vec![light, assault, air, infantry]);
    // KillSkill[1] for the light mech, KillSkill[4] for the assault mech (the C++ index is the class), then 20 for each ground kill
    let points = w.mission.pilot_loads[1].skill_points[mc2_sim_data::pilot::Skill::Gunnery.index()];
    assert_eq!(points, 60.0 + 20.0 + 20.0 + 20.0);
}

/// `friendlyDestroyed` / `enemyDestroyed` (gamesound.cpp:31-32): the latch of the destroyed mover's side, read and cleared by the
/// natives (ablmc2.cpp:2775-2783).
#[test]
fn destruction_sets_the_latch_of_its_side() {
    let mut w = World::new(RngMode::Pcg32, 1);
    mc2_sim::mission::note_destroyed(&mut w, false);
    assert_eq!(w.mission.destroyed_latches, (false, true));
    mc2_sim::mission::note_destroyed(&mut w, true);
    assert_eq!(w.mission.destroyed_latches, (true, true));
}

/// A mission part without a `BaseColor`/`HighlightColor` key takes the player's preference colours (mission.cpp:2647-2653).
#[test]
fn parts_without_paint_take_the_player_preferences() {
    use mc2_sim::mission::init::{fill_paint_from_prefs, LoadSpec, DEFAULT_PREFS_COLOR};
    use mc2_sim::mission::plan::PartRec;

    let mut spec = LoadSpec::quickstart("mc2_01", 1, RngMode::Pcg32);
    assert_eq!(spec.prefs_base_color, DEFAULT_PREFS_COLOR, "prefs.cpp:73");
    spec.prefs_base_color = 0x1111_1111;
    spec.prefs_highlight_color = 0x2222_2222;
    let mut parts = vec![
        PartRec::default(),
        PartRec {
            base_color: Some(0x3333_3333),
            ..PartRec::default()
        },
        PartRec::default(),
    ];
    fill_paint_from_prefs(&mut parts, &spec);
    assert_eq!(
        parts[1].base_color,
        Some(0x3333_3333),
        "a file colour is kept"
    );
    assert_eq!(parts[1].highlight_color1, Some(0x2222_2222));
    assert_eq!(parts[2].base_color, Some(0x1111_1111));
    assert_eq!(parts[2].highlight_color2, Some(0x2222_2222));
}

/// `MaxExtractUnitDistance` is read from gamesys.fit (mission.cpp:1794-1796, objective.cpp:1084), not fixed at 1280. Without the
/// tables the C++ default applies; with the shipped tables the file's value is used, which a changed copy shows.
#[test]
fn extraction_distance_is_read_from_gamesys() {
    use mc2_formats::vfs::Vfs;
    use std::rc::Rc;

    let mut w = World::new(RngMode::Pcg32, 1);
    assert_eq!(
        w.max_extract_unit_distance(),
        1280.0,
        "no tables: the C++ default"
    );

    let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
        eprintln!("MC2_DATA not set; skipping the gamesys.fit part");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(std::path::Path::new(&root)).expect("system.cfg");
    let mut tables = mc2_sim_data::gamesys::GameSystem::load(&mut vfs).expect("gamesys.fit");
    assert_eq!(tables.general.max_extract_unit_distance, 1280.0);
    tables.general.max_extract_unit_distance = 640.0;
    w.data.gamesys.tables = Some(Rc::new(tables));
    assert_eq!(w.max_extract_unit_distance(), 640.0);
}

/// The objective success and failure natives (ablmc2.cpp) share one change latch per objective. Either query clears it
/// (`StatusChangedSuccess`/`StatusChangedFailed`, objective.cpp:2001-2035): a failure query consumes a success's change, and a
/// later change is reported once.
#[test]
fn objective_change_latch_is_consumed_by_either_query() {
    let mut e = entry(1, successful(), Vec::new());
    e.runtime.status = S;
    e.runtime.changed = true;
    let mut w = world_with(vec![e]);
    assert!(
        !objective_status_changed(&mut w, false),
        "a success is not a failure"
    );
    assert!(
        !objective_status_changed(&mut w, true),
        "the change was consumed"
    );

    w.mission.objectives[0].entries[0].runtime.changed = true;
    assert!(objective_status_changed(&mut w, true));
    assert!(!objective_status_changed(&mut w, true), "reported once");
}

/// `if (missionBrain)` (mission.cpp:530-541): without a brain the script block does not run, so the script never returns a result
/// that could overwrite `terminationResult`. Frames here run the production pieces `Sim::step` calls: [`run_script_phase`] and the
/// termination logic. The force-death loss (mission.cpp:558-563) survives every frame. After the 5 s timer the mission waits for the
/// results screen (mission.cpp:566-569): `scenarioResult` stays `mis_PLAYING`. Once `results_screen_done` is called
/// (controlgui.cpp:2901), the result is set and `MissionResult` is emitted once, on the first frame past the next expiry
/// (mission.cpp:566-594).
#[test]
fn no_mission_brain_keeps_the_force_death_result_across_frames() {
    use mc2_sim::events::SimEvent;
    use mc2_sim::mission::{results_screen_done, update_termination};
    use mc2_sim::sim::run_script_phase;

    const FPS: f32 = 30.0;
    let mut w = World::new(RngMode::Pcg32, 1);
    assert_eq!(run_mission_brain(&mut w), None, "no brain");

    // Frame 1: the empty home force is dead (mission.cpp:547-563); the timer ends 5 s later.
    for f in 1..=181 {
        w.mission.actual_time = f as f32 / FPS;
        assert_eq!(
            run_script_phase(&mut w),
            None,
            "frame {f}: no script result"
        );
        assert_eq!(update_termination(&mut w), result::PLAYING, "frame {f}");
        assert_eq!(
            w.mission.term.result,
            result::LOST_BIG as i32,
            "frame {f}: the loss must survive a frame without a brain"
        );
    }
    assert!(
        w.mission.term.awaiting_results,
        "mission.cpp:566-569: the timer ran out, the results screen is not done"
    );
    assert!(w.events.is_empty());

    results_screen_done(&mut w);
    assert!(!w.mission.term.awaiting_results);
    let expiry = w.mission.term.end_time;
    let mut f = 182;
    let scenario = loop {
        let before = w.mission.actual_time;
        w.mission.actual_time = f as f32 / FPS;
        run_script_phase(&mut w);
        let scenario = update_termination(&mut w);
        if scenario != result::PLAYING {
            assert!(
                f64::from(before) < expiry && f64::from(w.mission.actual_time) >= expiry,
                "emitted on the first frame past the expiry {expiry}"
            );
            break scenario;
        }
        assert!(w.events.is_empty(), "frame {f}: nothing before the expiry");
        f += 1;
        assert!(f < 600, "no result 10 s after the results screen");
    };
    assert_eq!(scenario, result::LOST_BIG);
    assert!(matches!(w.events[..], [SimEvent::MissionResult(1)]));

    // Emitted once: the next frame keeps the result and emits nothing.
    w.events.clear();
    w.mission.actual_time = (f + 1) as f32 / FPS;
    run_script_phase(&mut w);
    assert_eq!(update_termination(&mut w), result::LOST_BIG);
    assert!(w.events.is_empty());
}

/// `if (missionBrain)` (mission.cpp:530-541) needs a brain that can execute. A brain instance with no loaded script environment runs
/// nothing, so it must not overwrite `terminationResult` with 0 (`bindings::run_mission_brain` used to return `Some(0)` here).
#[test]
fn brain_without_script_environment_keeps_the_termination_result() {
    use mc2_sim::sim::run_script_phase;

    let mut w = World::new(RngMode::Pcg32, 1);
    w.mission.brain = Some(mc2_abl::InstId(0));
    assert!(w.abl.is_none());
    w.mission.term.result = result::LOST_BIG as i32;
    assert_eq!(run_script_phase(&mut w), None);
    assert_eq!(w.mission.term.result, result::LOST_BIG as i32);
}

/// The same termination path through `Sim::step` (mission.cpp:530-663), on a synthetic world with no mission loaded: nothing
/// unported runs, so the world is complete. Real `mc2_01` loss and awaiting-results evidence is in `populated_digest.rs`.
#[test]
fn synthetic_sim_frames_keep_the_loss_and_emit_once_after_results_screen() {
    use mc2_sim::events::SimEvent;
    use mc2_sim::mission::init::LoadOutcome;
    use mc2_sim::visibility::AllVisible;
    use mc2_sim::{FrameInput, Sim};
    use std::rc::Rc;

    const FPS: u32 = 30;
    let frame = |f: u64| FrameInput::fixed(FPS, f, Rc::new(AllVisible), Vec::new());
    let world = World::new(RngMode::Pcg32, 1);
    let complete = world.load_is_complete();
    let missing_hooks = world.unported_load_hooks.clone();
    let mut sim = Sim::new(LoadOutcome {
        world,
        complete,
        missing_hooks,
    });

    // An empty home force is dead at once: the loss is set on frame 1 and the timer runs 5 s of scenario time.
    let out = sim.step(frame(1)).unwrap();
    assert_eq!(out.result, result::PLAYING);
    assert_eq!(sim.world.mission.term.result, result::LOST_BIG as i32);
    assert!(!out.awaiting_results_screen);

    let mut f = 2;
    while sim.world.mission.actual_time <= 6.0 {
        let out = sim.step(frame(f)).unwrap();
        assert_eq!(
            out.result,
            result::PLAYING,
            "frame {f}: no results screen yet"
        );
        assert_eq!(
            sim.world.mission.term.result,
            result::LOST_BIG as i32,
            "frame {f}: the loss must survive a frame without a brain"
        );
        f += 1;
    }
    let out = sim.step(frame(f)).unwrap();
    assert_eq!(out.result, result::PLAYING);
    assert!(
        out.awaiting_results_screen,
        "frame {f}: the timer ran out and the host has not called results_screen_done"
    );
    assert!(!out
        .events
        .iter()
        .any(|e| matches!(e, SimEvent::MissionResult(_))));

    // The results check runs only when the timer has run out (mission.cpp:568), and the force-death path re-armed it at the last
    // check (mission.cpp:587-600), so the result is emitted at the next expiry, not on the frame the results screen finishes.
    sim.results_screen_done();
    let expiry = sim.world.mission.term.end_time;
    let mut g = f + 1;
    loop {
        let before = sim.world.mission.actual_time;
        let out = sim.step(frame(g)).unwrap();
        g += 1;
        if out.result != result::PLAYING {
            assert_eq!(out.result, result::LOST_BIG);
            assert!(!out.awaiting_results_screen);
            assert!(matches!(out.events[..], [SimEvent::MissionResult(1)]));
            assert!(
                f64::from(before) < expiry && f64::from(sim.world.mission.actual_time) >= expiry,
                "emitted on the first frame past the expiry {expiry}"
            );
            break;
        }
        assert!(
            !out.awaiting_results_screen,
            "frame {g}: the results screen is done, the mission is not waiting on it"
        );
        assert!(out.events.is_empty());
        assert!(
            g < f + 1 + 10 * FPS as u64,
            "no result 10 s after the results screen"
        );
    }

    let out = sim.step(frame(g)).unwrap();
    assert_eq!(out.result, result::LOST_BIG);
    assert!(out.events.is_empty());
}

/// A complete mc2_01 load remains PLAYING and outside the results wait during its first 120 real frames.
/// The scripted force-loss and oracle results-gate comparison runs in `populated_digest.rs`.
///
/// Needs `MC2_DATA`; skips without it. The populated oracle digest comparison runs in `populated_digest.rs`.
#[test]
fn mission_frames_stay_playing_on_a_complete_load() {
    use mc2_formats::vfs::Vfs;
    use mc2_sim::mission::init::{load_world, LoadSpec};
    use mc2_sim::visibility::AllVisible;
    use mc2_sim::{FrameInput, Sim};
    use std::rc::Rc;

    let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
        return;
    };
    let mut vfs = Vfs::from_system_cfg(std::path::Path::new(&root)).expect("system.cfg");
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
    )
    .unwrap();
    assert!(loaded.complete, "missing hooks: {:?}", loaded.missing_hooks);
    assert!(loaded.missing_hooks.is_empty());
    let mut sim = Sim::new(loaded);
    let frame = |f: u64| FrameInput::fixed(30, f, Rc::new(AllVisible), Vec::new());
    for f in 1..=120 {
        let output = sim.step(frame(f)).expect("a complete load steps");
        assert!(!output.awaiting_results_screen, "frame {f}");
        assert_eq!(output.result, result::PLAYING, "frame {f}");
    }
}
