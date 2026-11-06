// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! P3 integration, campaign launch through the production headless path: the campaign's `LoadSpec`
//! (`Logistics::beginMission`) reaches the live simulation's load, and the results loop starts only
//! when `MissionResults::begin` runs.  Never opens a window; skipped without `MC2_DATA` or with
//! `MC2_SKIP_GPU_TESTS=1` when no adapter is available.

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_app::state::GameState;
use mc2_campaign::interface::{ForceMech, LoadType, SupportEnables};
use mc2_campaign::LoadSpec;
use mc2_sim::mission::result;
use mc2_sim::object::Kind;
use std::path::PathBuf;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

fn gpu_skip_requested() -> bool {
    std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1")
}

fn start(launch: Option<LoadSpec>) -> Option<HeadlessRun<LiveSim>> {
    let mut opts = HeadlessOptions::new(data()?, "mc2_01");
    opts.size = (800, 600);
    opts.game.cursor = false;
    opts.game.campaign_launch = launch;
    let mut run = match HeadlessRun::production(&opts) {
        Ok(run) => run,
        Err(e) if e.contains("no suitable GPU adapter") && gpu_skip_requested() => {
            eprintln!("no GPU adapter; MC2_SKIP_GPU_TESTS=1, skipping P3 campaign: {e}");
            return None;
        }
        Err(e) => panic!("the production headless session needs a GPU and the data: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).expect("load mission");
    }
    Some(run)
}

fn world(run: &mut HeadlessRun<LiveSim>) -> &mut mc2_sim::World {
    &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world
}

/// Dismiss `mc2_01`'s opening movie through the production Escape path before a test drives
/// ordinary mission time. Presentation correctly freezes normal simulation while it is active.
fn dismiss_opening_movie(run: &mut HeadlessRun<LiveSim>) {
    if !run
        .core
        .mission
        .as_ref()
        .is_some_and(|mission| mission.sim_presentation.movie_mode)
    {
        return;
    }
    let code = script::key_code("escape").expect("Escape key name");
    for event in script::key_events(code, true) {
        run.event(event);
    }
    run.step(None).unwrap();
    for event in script::key_events(code, false) {
        run.event(event);
    }
    run.step(None).unwrap();
    for _ in 0..400 {
        if !run
            .core
            .mission
            .as_ref()
            .is_some_and(|mission| mission.sim_presentation.movie_mode)
        {
            run.step(None).unwrap();
            return;
        }
        run.step(None).unwrap();
    }
    panic!("mc2_01's Escape dismissal never ended the opening movie");
}

/// `(type number, variant name, count)` of the home commander's mechs.
fn home_mechs(run: &mut HeadlessRun<LiveSim>) -> Vec<(i32, String)> {
    let w = world(run);
    w.objects
        .movers
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .filter(|o| o.exists())
        .filter_map(|o| match &o.kind {
            Kind::Mech(m) if m.m.commander.0 == 0 => {
                Some((o.c.type_handle, m.variant_name.clone()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_campaign_launch_reaches_the_live_sim_load() {
    // the `-mission` quickstart: the mission file defines the player's units; defaults
    let Some(mut quick) = start(None) else { return };
    let quick_mechs = home_mechs(&mut quick);
    assert!(quick_mechs.len() > 1, "{quick_mechs:?}");
    assert_eq!(world(&mut quick).mission.difficulty, 1);
    assert!(world(&mut quick).mission.unlimited_ammo);
    eprintln!("quickstart home mechs: {quick_mechs:?}");
    let (object_number, _) = quick_mechs[0].clone();

    let launch = LoadSpec {
        mission: "mc2_01".into(),
        load_type: LoadType::SpLogistics,
        difficulty: 2,
        unlimited_ammo: false,
        resource_points: 12_345,
        drop_weight_limit: 100,
        force: vec![ForceMech {
            variant_name: "TEST-FORCE".into(),
            chassis_file: String::new(),
            object_number: i64::from(object_number),
            variant_id: 1,
            designer: true,
            components: Vec::new(),
            pilot_file: "pmw00001".into(),
            gunnery: 61.0,
            piloting: 62.0,
            specialty_skills: [false; 23],
            force_group: 1,
            colors: [0xff11_2233, 0xff44_5566, 0xff44_5566],
        }],
        abl_script: "NONE".into(),
        roster: Vec::new(),
        support: SupportEnables {
            salvage_craft: false,
            repair_truck: false,
            scout_copter: false,
            artillery_piece: false,
            air_strike: false,
            sensor_strike: false,
            mine_layer: false,
        },
    };
    let Some(mut run) = start(Some(launch)) else {
        return;
    };
    let mechs = home_mechs(&mut run);
    assert_eq!(
        mechs.len(),
        1,
        "only the selected force is loaded: {mechs:?}"
    );
    assert_eq!(mechs[0].0, object_number);
    let w = world(&mut run);
    assert_eq!(w.mission.difficulty, 2);
    assert!(!w.mission.unlimited_ammo);
    assert_eq!(w.mission.resource_points, 12_345);
}

/// A win of a single mission (a script with one group of one mission) skips the results screens
/// (`Logistics::start(log_RESULTS)`, `code/logistics.cpp:121-180`), so their loop never starts.
#[test]
fn a_single_mission_win_skips_the_results_and_their_loop() {
    // the `-mission` path is a command-line start; reach the same code with the campaign flag set
    let Some(mut run) = start(None) else { return };
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    dismiss_opening_movie(&mut run);
    let host = run.core.ui_host_mut().expect("production UI host");
    host.set_single_mission_on_next_result("mc2_01");
    let w = world(&mut run);
    let home = w.teams.home;
    for set in w.mission.objectives.iter_mut().filter(|s| s.team == home) {
        for e in &mut set.entries {
            e.runtime.active = false;
            e.def.activate_on_flag = false;
        }
    }
    let mut r = None;
    for _ in 0..40 * 30 {
        run.step(None).unwrap();
        if let GameState::Results(x) = run.core.state() {
            r = Some(*x);
            break;
        }
    }
    assert_eq!(r, Some(result::WIN_BIG));
    for _ in 0..3 {
        run.step(None).unwrap();
    }
    let music = run
        .core
        .audio
        .as_ref()
        .unwrap()
        .shared_game()
        .borrow()
        .sound
        .sys
        .current_music_id();
    assert_ne!(music, i64::from(mc2_audio::ids::music::RESULTS_LOOP));
    assert!(!run.core.results_screen_drawn());
    // the win was recorded and the game went on (no error), not to a results screen
    let host = run.core.ui_host_mut().unwrap();
    assert_ne!(host.screen(), mc2_app::ui_host::ActiveScreen::Results);
    assert!(
        host.notices().iter().all(|n| !n.contains("could not")),
        "{:?}",
        host.notices()
    );
}

/// `LogisticsData::getBestPilot` through the production launch: the campaign's roster reaches the live sim, a recovered mech gets
/// the best free pilot (rank, then gunnery, then piloting; pilots on a mech or already used rank last), and a quickstart or an
/// empty roster is refused, never faked (RC slice S8).
#[test]
fn a_recovered_mech_gets_the_campaigns_best_free_pilot() {
    use mc2_campaign::interface::RosterPilot;
    let pilot = |file: &str, rank, gunnery, used, in_inventory| RosterPilot {
        file_name: file.into(),
        available: true,
        used,
        in_inventory,
        rank,
        gunnery,
        piloting: 40.0,
        specialty_skills: [false; 23],
    };
    let roster = vec![
        pilot("pmw00001", 5, 90.0, true, true), // flying the force: used
        pilot("pmw00002", 2, 70.0, false, false),
        pilot("pmw00003", 3, 60.0, false, false), // the best free one: highest rank
        pilot("pmw00004", 3, 55.0, false, false),
        pilot("pmw00005", 4, 99.0, false, true), // kept on an inventory mech: last
    ];
    let launch = LoadSpec {
        mission: "mc2_01".into(),
        load_type: LoadType::SpLogistics,
        difficulty: 1,
        unlimited_ammo: true,
        resource_points: 1000,
        drop_weight_limit: 100,
        force: Vec::new(),
        abl_script: "NONE".into(),
        support: SupportEnables {
            salvage_craft: true,
            repair_truck: true,
            scout_copter: true,
            artillery_piece: true,
            air_strike: true,
            sensor_strike: true,
            mine_layer: true,
        },
        roster,
    };
    let Some(mut run) = start(Some(launch)) else {
        return;
    };
    let w = world(&mut run);
    let chooser = w
        .recovery_pilots
        .clone()
        .expect("the launch installs the roster");
    let free = |_: &str| false;
    assert!(chooser.has_pilot(&free));
    let first = chooser.best_pilot(50, &free).expect("a pilot");
    assert_eq!(first.file, "pmw00003");
    assert_eq!(first.skills.map(|s| s.0), Some(60.0));
    // it is marked used: the next recovery gets the next best, then the inventory pilot, then none
    let second = chooser.best_pilot(50, &free).expect("a second pilot");
    assert_eq!(second.file, "pmw00004");
    // `warriorInUse` excludes a pilot flying in the mission
    let third = chooser
        .best_pilot(50, &|f| f.eq_ignore_ascii_case("pmw00005"))
        .expect("a third pilot");
    assert_eq!(third.file, "pmw00002");
    assert!(chooser
        .best_pilot(50, &|f| f.eq_ignore_ascii_case("pmw00005"))
        .is_none());
    assert!(!chooser.has_pilot(&|f| f.eq_ignore_ascii_case("pmw00005")));
}

#[test]
fn a_quickstart_has_no_recovery_pilot_and_a_recovery_is_refused() {
    let Some(mut run) = start(None) else { return };
    let w = world(&mut run);
    assert!(w.recovery_pilots.is_none());
    let mech = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            w.objects.get(h).is_some_and(|o| {
                o.team_id() != w.teams.home as i32 && matches!(o.kind, Kind::Mech(_))
            })
        })
        .expect("an enemy mech");
    assert_eq!(
        mc2_sim::ai::support_orders::take_over(w, mech),
        Err(mc2_sim::mission::trade::TradeError::NoPilot)
    );
}

/// The in-mission results the 2,000-frame sweep could not reach on `mc2_01` (its objectives are
/// still undecided after 2,000 frames): through the production headless session, a win shows the
/// status frame in `MISSION SUCCESS` and a loss in `MISSION FAILED`, each after the authored slide
/// (`code/controlgui.cpp:510-615`), and the mission then ends in its own result.
#[test]
fn decided_missions_show_the_results_status_frame_with_its_text_in_production() {
    let mut saw = Vec::new();
    // a win: every home objective decided successful (`CObjectives::Status` is successful)
    {
        let Some(mut run) = start(None) else { return };
        for _ in 0..10 {
            run.step(None).expect("frame");
        }
        dismiss_opening_movie(&mut run);
        let w = world(&mut run);
        let home = w.teams.home;
        for set in w.mission.objectives.iter_mut().filter(|s| s.team == home) {
            for e in &mut set.entries {
                e.runtime.active = false;
                e.def.activate_on_flag = false;
            }
        }
        saw.push(("win", status_frames(&mut run, 40 * 30)));
    }
    // a loss: the mission's own time limit passes (`mission.cpp` termination, `mis_PLAYER_LOST`)
    {
        let Some(mut run) = start(None) else { return };
        for _ in 0..10 {
            run.step(None).expect("frame");
        }
        dismiss_opening_movie(&mut run);
        let w = world(&mut run);
        w.mission.time_limit = w.mission.actual_time;
        saw.push(("loss", status_frames(&mut run, 40 * 30)));
    }
    for (kind, (texts, frames, state)) in &saw {
        let expected = if *kind == "win" {
            "MISSION SUCCESS"
        } else {
            "MISSION FAILED"
        };
        assert!(
            texts.iter().all(|t| *t == expected),
            "{kind}: every status frame is {expected}: {texts:?}"
        );
        // the slide gets under way in the frames after the first draw
        assert!(
            frames.iter().any(|(r, _)| (0.0..0.24).contains(r)),
            "{kind}: the frame slides in ({frames:?})"
        );
        // once it has arrived it is greyed and the objective list opens (`controlgui.cpp:547-550`,
        // `:611-615`). `resultsTime` and the panel's open flag are one frame apart here (the
        // draw uses the time of the frame before), so the pairing allows that one frame.
        let arrived = frames.iter().position(|(r, _)| *r >= 0.24);
        let arrived = arrived.unwrap_or_else(|| panic!("{kind}: the frame settles ({frames:?})"));
        assert!(
            frames[arrived..].iter().all(|(_, open)| *open),
            "{kind}: the objectives are open once it settled ({frames:?})"
        );
        assert!(
            frames[..arrived - 1].iter().all(|(_, open)| !open),
            "{kind}: the objectives are closed during the slide ({frames:?})"
        );
        assert_eq!(state, "results", "{kind}: the mission reached its results");
    }
}

/// Steps until the session leaves the mission, collecting for each frame that drew the status
/// frame its text, its `resultsTime` and whether the objectives panel was open.
fn status_frames(
    run: &mut HeadlessRun<LiveSim>,
    budget: u32,
) -> (Vec<&'static str>, Vec<(f32, bool)>, String) {
    let mut texts = Vec::new();
    let mut frames = Vec::new();
    for _ in 0..budget {
        run.step(None).expect("frame");
        if let Some(hud) = run.core.mission.as_ref().and_then(|m| m.hud.as_ref()) {
            if hud.results_status_frame_drawn() {
                texts.push(hud.results_status_text().expect("a drawn frame has a text"));
                frames.push((
                    hud.objectives_view().results_time,
                    hud.objectives_panel().is_open(),
                ));
            }
        }
        if matches!(run.core.state(), GameState::Results(_)) {
            break;
        }
    }
    (texts, frames, run.core.state().name().to_string())
}

/// A support vehicle a VTOL drops after the load is drawn and pickable in the production view: the dropped mover is bound to the
/// home-painted template of its object type, has a rendered instance and a selection box, and nothing is reported unbound.
#[test]
fn a_dropped_support_vehicle_is_drawn_and_pickable() {
    use mc2_sim::command::PlayerCommand;
    let Some(mut run) = start(None) else { return };
    let (home, at) = {
        let w = world(&mut run);
        let home = w.teams.home as i32;
        let h = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.team_id() == home && matches!(o.kind, Kind::Mech(_)) && !o.c.is_disabled()
                })
            })
            .expect("a home mech");
        let p = w.objects.get(h).unwrap().c.position;
        let at = mc2_sim::math::Vec3 { x: p.x + 40.0, ..p };
        let z = w.terrain.elevation(&at);
        (h, mc2_sim::math::Vec3 { z, ..at })
    };
    let _ = home;
    for _ in 0..5 {
        run.step(None).expect("frame");
    }
    let before = world(&mut run).objects.movers.len();
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .inject_commands(vec![PlayerCommand::BeginVtol {
            commander: 0,
            vehicle_id: 182,
            at,
            target: None,
        }]);
    for _ in 0..600 {
        run.step(None).expect("frame");
        if world(&mut run).objects.movers.len() > before {
            break;
        }
    }
    let w = world(&mut run);
    assert_eq!(w.objects.movers.len(), before + 1, "the truck was dropped");
    let part = w.vtol.last_dropped[0].expect("the dropped mover");
    let handle = w.objects.find_by_part_id(part).unwrap();
    // two more frames: the visibility pass binds it and projects its box
    for _ in 0..3 {
        run.step(None).expect("frame");
    }
    let rt = run.core.mission.as_ref().unwrap();
    assert!(
        rt.driver.mover_instance(handle).is_some(),
        "the dropped mover has a rendered instance"
    );
    assert!(
        rt.driver.mover_screen_box(handle).is_some(),
        "the dropped mover has a pick box"
    );
    assert!(
        !rt.driver
            .unavailable()
            .keys()
            .any(|k| k.contains("created after the load")),
        "{:?}",
        rt.driver.unavailable()
    );
}
