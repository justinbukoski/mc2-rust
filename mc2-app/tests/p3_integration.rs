// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! P3 integration, through the production headless path (`HeadlessRun::production`, the session behind
//! `mc2 --headless`): the live simulation, the registered effects and battle sound sinks, and the end of a
//! mission driving the state machine into the results flow.  Never opens a window; skipped without `MC2_DATA`,
//! and a missing GPU adapter fails unless `MC2_SKIP_GPU_TESTS=1` (these are the acceptance evidence).
//!
//! Nothing here calls a sink or the results host directly: events come from the live sim stepped by
//! `GameCore`, and a mission is ended by putting the sim in the state the objectives would
//! (`terminationResult`, the counter and its end time, `mission.cpp:610-660`) and letting the HUD's
//! results and the sim's own termination run.

use mc2_app::fx_sink::FxHub;
use mc2_app::headless::{production_capabilities, HeadlessOptions, HeadlessRun};
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_app::sim::{FrameContext, SimEventSink};
use mc2_app::state::GameState;
use mc2_app::ui_host::ActiveScreen;
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::mission::result;
use mc2_sim::object::Kind;
use mc2_sim::SimEvent;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

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

fn start() -> Option<HeadlessRun<LiveSim>> {
    let mut opts = HeadlessOptions::new(data()?, "mc2_01");
    opts.size = (800, 600);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::production(&opts) {
        Ok(r) => r,
        Err(e) if e.contains("no suitable GPU adapter") && gpu_skip_requested() => {
            eprintln!("no GPU adapter; MC2_SKIP_GPU_TESTS=1, skipping P3 integration: {e}");
            return None;
        }
        Err(e) => {
            panic!("the production headless session needs a GPU adapter and the game data: {e}")
        }
    };
    while run.core.mission.is_none() {
        run.step(None).expect("load mission");
    }
    Some(run)
}

fn world(run: &mut HeadlessRun<LiveSim>) -> &mut mc2_sim::World {
    &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world
}

/// End `mc2_01`'s opening movie through production input before driving normal mission time.
/// Movie mode intentionally freezes the simulation until the player dismisses it.
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

fn commander(o: &mc2_sim::object::Obj) -> Option<i32> {
    match &o.kind {
        Kind::Mech(m) => Some(i32::from(m.m.commander.0)),
        Kind::Vehicle(v) => Some(i32::from(v.m.commander.0)),
        _ => None,
    }
}

#[test]
fn the_production_headless_session_runs_the_live_sim_with_the_production_sinks() {
    assert!(production_capabilities().live_sim);
    let Some(run) = start() else { return };
    assert!(
        run.core.has_live_sim(),
        "the production driver is the live sim"
    );
    assert!(run.core.fx_hub.is_some(), "the effects hub is registered");
    assert!(
        run.core.effects.is_some(),
        "the scene pass draws the effects"
    );
    assert!(
        run.core.audio.is_some(),
        "headless has the offline game sound"
    );
    assert!(run.core.battle_sound, "the battle sound sink is registered");
    assert_eq!(
        run.core.sinks.len(),
        2,
        "the effects sink and the battle sound sink"
    );
}

/// What each frame's sinks had done when the frame ended (a sink pushed after the production ones sees their
/// state at the end of the same `on_frame` round).
#[derive(Clone, Default)]
struct Probe {
    frames: Rc<RefCell<Vec<Seen>>>,
    hub: Option<FxHub>,
    audio: Option<mc2_app::audio::SharedGameAudio>,
}

#[derive(Debug, Clone)]
struct Seen {
    /// The frame's events that make an effect (`WeaponEffect`, `Explosion`).
    fx_events: usize,
    /// The positional sounds of the frame (`Sound`), by id.
    sounds: Vec<u32>,
    /// Which of those samples are playing at the end of the frame.
    playing: Vec<u32>,
}

impl SimEventSink<LiveSim> for Probe {
    fn on_frame(&mut self, _ctx: &FrameContext<'_, LiveSim>, events: &[SimEvent]) {
        let sounds: Vec<u32> = events
            .iter()
            .filter_map(|e| match e {
                SimEvent::Sound { id, .. } => u32::try_from(*id).ok(),
                _ => None,
            })
            .collect();
        let audio = self.audio.as_ref().unwrap().borrow();
        let playing = sounds
            .iter()
            .copied()
            .filter(|&id| audio.sound.sys.is_playing_sample(id))
            .collect();
        let _ = &self.hub;
        self.frames.borrow_mut().push(Seen {
            fx_events: events
                .iter()
                .filter(|e| {
                    matches!(
                        e,
                        SimEvent::WeaponEffect { .. } | SimEvent::Explosion { .. }
                    )
                })
                .count(),
            sounds,
            playing,
        });
    }
}

#[test]
fn a_weapon_fired_in_the_live_sim_makes_an_effect_and_a_sample_in_the_same_frame() {
    let Some(mut run) = start() else { return };
    let hub = run.core.fx_hub.clone().expect("effects hub");
    // the options toggle: the muzzle flash is a "non-weapon" effect, so the launch frame has an effect to show
    hub.set_use_non_weapon_effects(true);
    let probe = Probe {
        frames: Rc::default(),
        hub: Some(hub.clone()),
        audio: Some(run.core.audio.as_ref().unwrap().shared_game()),
    };
    run.core.sinks.push(Box::new(probe.clone()));
    for _ in 0..3 {
        run.step(None).unwrap();
    }

    // the player's mech and the nearest enemy are put in front of the camera (the only state set)
    let (attacker, target_watch) = {
        let eye = run.core.mission.as_ref().unwrap().rig.eye_position();
        let w = world(&mut run);
        let movers = w.objects.movers.clone();
        let attacker = movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    matches!(o.kind, Kind::Mech(_)) && commander(o) == Some(HOME_COMMANDER)
                })
            })
            .expect("a player mech");
        let enemy = movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.exists() && commander(o).is_some_and(|c| c != HOME_COMMANDER)
                })
            })
            .expect("an enemy mover");
        for (h, dx) in [(attacker, -150.0), (enemy, 150.0)] {
            let o = w.objects.get_mut(h).unwrap();
            o.c.position.x = eye.x + dx;
            o.c.position.y = eye.y;
        }
        (attacker, w.objects.watch_id(enemy))
    };

    // fire (a weapon is refused while its nodes settle after the mission start: `fireWeapon` code 3)
    let mut fired_in = None;
    for _ in 0..120 {
        let fired = {
            let w = world(&mut run);
            let target = w.objects.by_watch_id(target_watch);
            let items = mc2_sim::combat::loadout_of(w.objects.get(attacker).unwrap())
                .unwrap()
                .inventory
                .len();
            (0..items).any(|weapon| {
                mc2_sim::combat::fire::fire_weapon(w, attacker, weapon as i32, target, -1, None, 0)
                    .code
                    == 0
            })
        };
        let spawned_before = hub.spawned().len();
        let frames_before = probe.frames.borrow().len();
        run.step(None).unwrap();
        if fired {
            let frames = probe.frames.borrow();
            let seen = frames[frames_before..]
                .last()
                .expect("the frame ran")
                .clone();
            fired_in = Some((seen, hub.spawned().len() - spawned_before));
            break;
        }
    }
    let (seen, new_effects) = fired_in.expect("a weapon of the player mech fires at the enemy");
    assert!(
        seen.fx_events > 0,
        "the firing frame carries a weapon effect event: {seen:?}"
    );
    assert!(
        !seen.sounds.is_empty(),
        "the firing frame carries a fire sound: {seen:?}"
    );
    assert_eq!(
        seen.playing, seen.sounds,
        "every sample of the firing frame is playing when the frame ends"
    );
    assert!(
        new_effects > 0,
        "the firing frame started a live FxSystem effect (muzzle flash)"
    );
    assert!(hub.active_count() > 0, "the effect is running");
}

/// Decides the home team's objectives as won: none is left active, so `Status()` is successful
/// (`objective.cpp:2165`) and the sim's own termination starts the 5 s counter with `mis_PLAYER_WIN_BIG`.
fn win_the_objectives(run: &mut HeadlessRun<LiveSim>) {
    let w = world(run);
    let home = w.teams.home;
    for set in w.mission.objectives.iter_mut().filter(|s| s.team == home) {
        for e in &mut set.entries {
            e.runtime.active = false;
            e.def.activate_on_flag = false;
        }
    }
}

/// The mission's time limit has passed: the sim's termination starts the counter with `mis_PLAYER_LOST_BIG`.
fn run_out_the_clock(run: &mut HeadlessRun<LiveSim>) {
    let w = world(run);
    w.mission.time_limit = w.mission.actual_time;
}

/// The music the production headless game sound is playing (`-1`: none).
fn playing_music(run: &HeadlessRun<LiveSim>) -> i64 {
    let audio = run
        .core
        .audio
        .as_ref()
        .expect("headless game sound")
        .shared_game();
    let id = audio.borrow().sound.sys.current_music_id();
    id
}

/// Steps until the core is in Results.
fn run_until_results(run: &mut HeadlessRun<LiveSim>, budget: u32) -> u32 {
    for n in 0..budget {
        run.step(None).unwrap();
        if let GameState::Results(r) = run.core.state() {
            return *r;
        }
        assert!(
            matches!(run.core.state(), GameState::InMission),
            "left the mission without a result: {:?} after {n} frames",
            run.core.state()
        );
    }
    panic!("the mission did not end within {budget} frames");
}

#[test]
fn a_won_mission_reaches_the_results_screens_through_the_hud_results_and_the_sim_timer() {
    let Some(mut run) = start() else { return };
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    dismiss_opening_movie(&mut run);
    win_the_objectives(&mut run);
    // The production P3 path renders the in-mission status before the five-second termination
    // counter reaches the Results host.  This is deliberately observed through the HUD's draw
    // path, not by calling the result presenter directly.
    let mut status_drawn = false;
    let mut status_frame = None;
    for _ in 0..30 {
        run.step(None).unwrap();
        let hud = run.core.mission.as_ref().and_then(|m| m.hud.as_ref());
        if let Some(hud) = hud.filter(|hud| hud.results_status_frame_drawn()) {
            status_drawn = true;
            status_frame = Some((
                hud.results_status_text(),
                hud.objectives_panel().is_open(),
                hud.objectives_view().results_time,
            ));
            break;
        }
    }
    assert!(
        status_drawn,
        "the mc2_01 P3 production replay drew the in-mission status frame"
    );
    // `renderResults` draws the status frame from the frame that starts the results (`.001f` in
    // `ControlGui::render`, controlgui.cpp:430-436) with `ResultsTextBox` in
    // `IDS_MISSION_SUCCESS` because every primary objective is decided (`:565-569`); the
    // objectives stay closed until `resultsTime >= .24` (`:611-615`). The first frame draws at
    // `.001f` before its own `frameLength` is added and the clock then advances with the HUD's
    // `frameLength` (a presentation tolerance of a few frames), so this checks that the drawn frame
    // is the start of the slide, not its exact number.
    let (text, objectives_open, results_time) = status_frame.expect("a drawn status frame");
    assert_eq!(text, Some("MISSION SUCCESS"), "every primary is decided");
    assert!(
        !objectives_open,
        "the objectives are closed during the slide ({results_time})"
    );
    assert!(
        (0.0..0.24).contains(&results_time),
        "the status frame is still sliding in ({results_time})"
    );
    // the 5 s timer, the results frame (0.49 s) and the objective count + 4 s: well inside 40 s
    let r = run_until_results(&mut run, 40 * 30);
    assert_eq!(r, result::WIN_BIG);
    assert_eq!(run.core.last_result, Some(result::WIN_BIG));
    // not before the in-mission results ended: the sim waited for `resultsDone` after the timer
    let frame = run.core.mission.as_ref().unwrap().clock.frame();
    assert!(
        frame > 10 + 5 * 30,
        "the mission cannot end before its 5 s timer ({frame})"
    );
    // the next frame the results host begins (the win is recorded, the screens are built) and draws
    run.step(None).unwrap();
    run.step(None).unwrap();
    // `MissionResults::begin` ran and started the results loop (missionresults.cpp:188-192)
    assert_eq!(
        playing_music(&run),
        i64::from(mc2_audio::ids::music::RESULTS_LOOP)
    );
    assert!(
        run.core.results_screen_drawn(),
        "a Results frame was composed"
    );
    let host = run.core.ui_host_mut().expect("production UI host");
    assert_eq!(host.screen(), ActiveScreen::Results);
    assert_eq!(host.last_mission_result(), result::WIN_BIG);
    assert_eq!(run.core.state().name(), "results");
}

#[test]
fn a_lost_mission_reaches_results_with_the_loss_and_the_command_line_game_then_quits() {
    let Some(mut run) = start() else { return };
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    dismiss_opening_movie(&mut run);
    run_out_the_clock(&mut run);
    let r = run_until_results(&mut run, 40 * 30);
    assert_eq!(r, result::LOST_BIG);
    assert_eq!(run.core.last_result, Some(result::LOST_BIG));
    // a loss skips `MissionResults::begin`, so the results loop never starts
    assert_ne!(
        playing_music(&run),
        i64::from(mc2_audio::ids::music::RESULTS_LOOP)
    );
    // `Logistics::start(log_RESULTS)` for a mission started from the command line that was not won:
    // `quitGame` (code/logistics.cpp:176-178)
    run.step(None).unwrap();
    assert_eq!(run.core.state(), &GameState::Exit);
    assert_eq!(run.core.last_result, Some(result::LOST_BIG));
}
