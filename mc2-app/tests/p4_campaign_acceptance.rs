// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! P4's production-route campaign acceptance: physical player input from the
//! main menu through a real mc2_01 win, its two Results pages, a mission-two
//! launch, and a production-menu campaign save reloaded by a fresh host that
//! also launches mission two.  Headless only.  In-mission (pause-menu) save/load is
//! a tracked follow-up owned by ui-screens (see the porting log); the campaign save is Logistics-only.
//!
//! This is intentionally longer than ordinary UI coverage.  Like the P3
//! acceptance replay it runs in the normal test suite because its core claim
//! is the live, player-visible route rather than a collection of fixtures.

use mc2_app::campaign_save::{CampaignSaveService, SaveSlot};
use mc2_app::headless::{HeadlessOptions, HeadlessRun, ProductionDriver};
use mc2_app::p3_acceptance::FRAME_BUDGET;
use mc2_app::p3_driver::PlayerDriver;
use mc2_app::state::GameState;
use mc2_app::ui_host::{ActiveScreen, UiHost};
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_platform::keys;
use mc2_ui::logistics::ids;
use mc2_ui::logistics::sequence::slot;
use mc2_ui::screens::main_menu::{MM_MSG_LOAD, MM_MSG_NEW_CAMPAIGN, MM_MSG_SAVE};
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

/// The P3 driver's physical tactical-map and world clicks target the shipped 800x600 HUD.
const SIZE: (u32, u32) = (800, 600);
const SCALE: f32 = 1.0;

fn data() -> PathBuf {
    std::env::var_os("MC2_DATA")
        .map(PathBuf::from)
        .expect("P4 acceptance needs MC2_DATA")
}

fn host(run: &mut HeadlessRun<ProductionDriver>) -> &mut UiHost {
    run.core
        .ui_host_mut()
        .expect("the production UI host is installed")
}

fn frames(run: &mut HeadlessRun<ProductionDriver>, count: usize) {
    for _ in 0..count {
        run.step(None).expect("production headless frame");
    }
}

/// Chooses a row of the campaign list New Campaign opens (`LogisticsSaveDialog::beginCampaign`).
fn choose_campaign(run: &mut HeadlessRun<ProductionDriver>, row: usize) {
    until(run, 60, "campaign list", |r| {
        host(r).screen() == ActiveScreen::CampaignPick
    });
    // the panel takes input once it has slid in
    frames(run, 60);
    let at = host(run)
        .campaign_pick_row_centre(row)
        .expect("row of the list");
    click(run, at);
    // a click only selects the row; Enter confirms it
    for pressed in [true, false] {
        run.event(mc2_platform::input::InputEvent::Key {
            key: mc2_platform::keys::KEY_RETURN,
            phys_id: mc2_platform::keys::KEY_RETURN as u16,
            pressed,
            repeat: false,
            text: None,
        });
        frames(run, 1);
    }
}

fn until(
    run: &mut HeadlessRun<ProductionDriver>,
    max: usize,
    what: &str,
    done: impl Fn(&mut HeadlessRun<ProductionDriver>) -> bool,
) {
    for _ in 0..max {
        if done(run) {
            return;
        }
        run.step(None).expect("production headless frame");
    }
    assert!(done(run), "{what}: not reached in {max} frames");
}

fn pointer(run: &mut HeadlessRun<ProductionDriver>, ui: (f32, f32)) {
    run.event(InputEvent::CursorMoved {
        x: f64::from(ui.0 * SCALE),
        y: f64::from(ui.1 * SCALE),
    });
}

/// A normal player click: move, press, and release in separate production frames.
fn click(run: &mut HeadlessRun<ProductionDriver>, ui: (f32, f32)) {
    frames(run, 8);
    pointer(run, ui);
    frames(run, 1);
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed: true,
    });
    frames(run, 1);
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed: false,
    });
    frames(run, 1);
}

fn key(run: &mut HeadlessRun<ProductionDriver>, key: u32, text: Option<String>) {
    run.event(InputEvent::Key {
        key,
        phys_id: key as u16,
        pressed: true,
        repeat: false,
        text,
    });
    frames(run, 1);
    run.event(InputEvent::Key {
        key,
        phys_id: key as u16,
        pressed: false,
        repeat: false,
        text: None,
    });
    frames(run, 1);
}

fn type_text(run: &mut HeadlessRun<ProductionDriver>, value: &str) {
    for ch in value.chars() {
        key(run, ch.to_ascii_uppercase() as u32, Some(ch.to_string()));
    }
}

fn skip_menu_intro(run: &mut HeadlessRun<ProductionDriver>) {
    key(run, keys::KEY_ESCAPE, None);
    key(run, keys::KEY_ESCAPE, None);
}

fn skip_logistics_movie(run: &mut HeadlessRun<ProductionDriver>) {
    frames(run, 1);
    key(run, keys::KEY_ESCAPE, None);
}

fn trace(trace: &mut Vec<String>, run: &mut HeadlessRun<ProductionDriver>, stage: &str) {
    let screen = host(run).screen();
    let state = run.core.state().name();
    let line = format!("{stage}: state={state} screen={screen:?}");
    // Written to the real stderr: `eprintln!` is captured by the test harness on success.
    let _ = writeln!(std::io::stderr(), "P4 stage trace: {line}");
    trace.push(line);
}

fn assert_no_unavailable(run: &mut HeadlessRun<ProductionDriver>) {
    let notices = host(run).notices().to_vec();
    assert!(
        notices.iter().all(|notice| {
            let notice = notice.to_ascii_lowercase();
            !notice.contains("unavailable") && !notice.contains("not available")
        }),
        "production route reported an unavailable notice: {notices:?}"
    );
}

/// A loading frame only constructs the runtime and changes `GameState`; it does not tick the
/// simulation.  The acceptance boundary requires a live frame, so prove the mission clock moves
/// while the production host remains in the mission without reporting an error.
fn assert_first_live_frame(
    run: &mut HeadlessRun<ProductionDriver>,
    trace_log: &mut Vec<String>,
    mission: &str,
) {
    let before = run
        .core
        .mission
        .as_ref()
        .expect("in-mission state owns a runtime")
        .clock
        .frame();
    frames(run, 1);
    assert_eq!(run.core.state(), &GameState::InMission);
    let after = run
        .core
        .mission
        .as_ref()
        .expect("live mission keeps its runtime")
        .clock
        .frame();
    assert!(
        after > before,
        "{mission} did not advance a simulation frame"
    );
    assert!(
        run.core.last_error.is_none(),
        "{mission} first simulation frame reported {:?}",
        run.core.last_error
    );
    trace(trace_log, run, &format!("{mission} first simulation frame"));
}

/// Drive the logistics pages with pointer input until the loading page hands a `LoadSpec` to the
/// real production mission loader.  Mission one has the campaign's quickstart route, so this does
/// not add or alter campaign state behind the UI's back.
fn launch_current_mission(
    run: &mut HeadlessRun<ProductionDriver>,
    trace_log: &mut Vec<String>,
    expected_mission: &str,
) {
    if host(run).screen() == ActiveScreen::Logistics(slot::LOADING) {
        // A loaded campaign whose mission skips selection restarts on its loading page.
        trace(trace_log, run, "loading (restored)");
        return finish_loading(run, expected_mission);
    }
    assert_eq!(host(run).screen(), ActiveScreen::Logistics(slot::SELECT));
    trace(trace_log, run, "mission selection");
    let next = host(run)
        .page_button(slot::SELECT, ids::MB_MSG_NEXT)
        .expect("mission selection Next button");
    click(run, next);

    assert_eq!(host(run).screen(), ActiveScreen::Logistics(slot::BRIEFING));
    trace(trace_log, run, "briefing");
    let next = host(run)
        .page_button(slot::BRIEFING, ids::MB_MSG_NEXT)
        .expect("briefing Next button");
    click(run, next);

    assert_eq!(host(run).screen(), ActiveScreen::Logistics(slot::MECH_BAY));
    trace(trace_log, run, "mech bay");
    let next = host(run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_NEXT)
        .expect("mech bay Next button");
    click(run, next);

    assert_eq!(
        host(run).screen(),
        ActiveScreen::Logistics(slot::PILOT_READY)
    );
    trace(trace_log, run, "pilot ready room");
    // `PilotReady::update` assigns the selected available pilot through its own ADD control.
    // On the first campaign mission this also makes the Next transition eligible; no campaign
    // state is edited directly by the test.
    let add = host(run)
        .page_button(slot::PILOT_READY, ids::PR_MSG_ADD)
        .expect("pilot ready Add button");
    click(run, add);
    let next = host(run)
        .page_button(slot::PILOT_READY, ids::MB_MSG_NEXT)
        .expect("pilot ready Next button");
    click(run, next);

    assert_eq!(host(run).screen(), ActiveScreen::Logistics(slot::LOADING));
    trace(trace_log, run, "loading");
    finish_loading(run, expected_mission);
}

fn finish_loading(run: &mut HeadlessRun<ProductionDriver>, expected_mission: &str) {
    until(run, 600, "live mission", |session| {
        session.core.state() == &GameState::InMission
    });
    assert_eq!(
        run.core
            .mission
            .as_ref()
            .map(|mission| mission.name.as_str()),
        Some(expected_mission),
        "the production loading page started the requested campaign mission"
    );
}

#[test]
fn production_campaign_loop_saves_loads_and_resumes_mission_two() {
    let started = Instant::now();
    let mut options = HeadlessOptions::new(data(), "");
    options.size = SIZE;
    options.frames = FRAME_BUDGET;
    let mut run = match HeadlessRun::production(&options) {
        Ok(run) => run,
        Err(e)
            if e.contains("adapter")
                && std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1") =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            return;
        }
        Err(e) => panic!("production headless host: {e}"),
    };
    let mut trace_log = Vec::new();
    let saves = std::env::temp_dir().join(format!("mc2-p4-acceptance-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));

    frames(&mut run, 2);
    assert_eq!(host(&mut run).screen(), ActiveScreen::MainMenu);
    trace(&mut trace_log, &mut run, "main menu");
    skip_menu_intro(&mut run);
    frames(&mut run, 90);

    let new_campaign = host(&mut run)
        .menu_button(MM_MSG_NEW_CAMPAIGN)
        .expect("New Campaign button");
    click(&mut run, new_campaign);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "campaign logistics", |session| {
        session.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    trace(&mut trace_log, &mut run, "new campaign logistics");

    launch_current_mission(&mut run, &mut trace_log, "mc2_01");
    // `PlayLogistics = 0`: the C++ RESTART route (`missionbegin.cpp:478`) goes to Loading (or
    // mission selection), never through briefing, mech bay and pilot ready.
    for page in ["briefing", "mech bay", "pilot ready room"] {
        assert!(
            !trace_log.iter().any(|t| t.contains(page)),
            "the quickstart mission visited {page}: {trace_log:?}"
        );
    }

    // The existing P3 player watches only the live HUD and sends only physical keyboard/mouse
    // input.  It is deliberately driven inside the already-running production host so the host,
    // campaign LoadSpec, and mission are one continuous app route.
    let mut player = PlayerDriver::new();
    let mut won = false;
    for _ in 0..FRAME_BUDGET {
        player.tick(&mut run);
        run.step(None)
            .expect("P3 production frame through campaign host");
        if matches!(run.core.state(), GameState::Results(_)) {
            won = true;
            break;
        }
    }
    assert!(won, "P3 player did not win within {FRAME_BUDGET} frames");
    assert_eq!(
        run.core.last_result,
        Some(mc2_sim::mission::result::WIN_BIG)
    );
    until(&mut run, 30, "salvage results screen", |session| {
        host(session).screen() == ActiveScreen::Results
    });
    trace(&mut trace_log, &mut run, "mission 1 won: salvage");

    let salvage_done = host(&mut run)
        .results_button(mc2_ui::mission_results::SALVAGE_DONE)
        .expect("salvage Done button");
    click(&mut run, salvage_done);
    trace(&mut trace_log, &mut run, "pilot review");
    // The review slides in and reveals each pilot in turn; a promoted pilot's pop-up opens when
    // its reveal has run, and the review's Done is enabled once the whole list has. Pick through
    // each pop-up by pointer.
    for _ in 0..4000 {
        if host(&mut run)
            .results()
            .is_some_and(|r| r.pilots.list.is_done() && r.pilots.picker_pilot().is_none())
        {
            break;
        }
        match host(&mut run).results_specialty_option() {
            Some(choice) => {
                click(&mut run, choice);
                let choose = host(&mut run)
                    .results_button(mc2_ui::mission_results::PILOT_CHOOSE)
                    .expect("pilot specialty Confirm button");
                click(&mut run, choose);
            }
            None => frames(&mut run, 1),
        }
    }
    assert!(
        host(&mut run).results_specialty_option().is_none(),
        "all promoted pilots have a physical specialty selection"
    );
    let pilot_done = host(&mut run)
        .results_button(mc2_ui::mission_results::PILOT_DONE)
        .expect("pilot review Done button");
    click(&mut run, pilot_done);
    until(&mut run, 60, "logistics after results", |session| {
        session.core.state() == &GameState::Logistics
    });
    if host(&mut run).movie_progress().is_some() {
        skip_logistics_movie(&mut run);
    }
    assert_eq!(
        host(&mut run).campaign().unwrap().current_mission(),
        "mc2_02",
        "the completed results flow advances the campaign"
    );
    trace(
        &mut trace_log,
        &mut run,
        "mission 1 results complete: logistics",
    );

    // SAVE at the logistics checkpoint before launching mission 2, through the production main
    // menu (Main Menu button -> Save Game -> named slot).  C++: the campaign save between
    // missions, `logistics.cpp` (`Logistics::save`); the in-mission pause-menu save is owned by
    // the ui-screens lane and is a tracked follow-up, not part of this slice.
    let saved = host(&mut run).campaign().unwrap().to_save();
    let menu = host(&mut run)
        .page_button(slot::SELECT, ids::MB_MSG_MAINMENU)
        .expect("selection Main Menu button");
    click(&mut run, menu);
    let save = host(&mut run)
        .menu_button(MM_MSG_SAVE)
        .expect("Save Game button");
    click(&mut run, save);
    until(&mut run, 90, "save slots", |r| {
        host(r).screen() == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    trace(&mut trace_log, &mut run, "save slot picker");
    type_text(&mut run, "p4accept");
    key(&mut run, keys::KEY_RETURN, None);
    frames(&mut run, 40); // the panel slides out, then the save is written
    assert!(saves.join("p4accept.ron").is_file(), "save written");
    assert_eq!(
        host(&mut run).screen(),
        ActiveScreen::Logistics(slot::SELECT)
    );
    trace(&mut trace_log, &mut run, "campaign saved");
    assert_no_unavailable(&mut run);

    launch_current_mission(&mut run, &mut trace_log, "mc2_02");
    // The mission-selection movie is a separate physical input consumer.  Clear it before
    // verifying the mission's Escape hotkey, exactly as a player would; otherwise that first
    // key legitimately skips the movie rather than reaching `MissionInterfaceManager`.
    if host(&mut run).movie_progress().is_some() {
        skip_logistics_movie(&mut run);
    }
    assert!(host(&mut run).movie_progress().is_none());
    assert_first_live_frame(&mut run, &mut trace_log, "mc2_02");
    assert_no_unavailable(&mut run);
    drop(run);

    // A FRESH host loads the slot through the production Load screen.
    let mut run = match HeadlessRun::production(&options) {
        Ok(run) => run,
        Err(e)
            if e.contains("adapter")
                && std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1") =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            return;
        }
        Err(e) => panic!("fresh production host: {e}"),
    };
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    let load = host(&mut run)
        .menu_button(MM_MSG_LOAD)
        .expect("Load Game button");
    click(&mut run, load);
    until(&mut run, 90, "load slots", |r| {
        host(r).screen() == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    trace(&mut trace_log, &mut run, "fresh host load picker");
    let row = host(&mut run).save_load_row_centre(0).expect("list row 0");
    click(&mut run, row);
    frames(&mut run, 40); // the panel slides out
    until(&mut run, 120, "loaded logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    assert_eq!(
        host(&mut run).campaign().unwrap().to_save(),
        saved,
        "the loaded campaign equals the saved campaign field for field"
    );
    assert_eq!(
        CampaignSaveService::new(&saves)
            .load(
                SaveSlot::Named("p4accept".into()),
                host(&mut run).campaign().unwrap().data().clone()
            )
            .unwrap()
            .to_save(),
        saved
    );
    trace(&mut trace_log, &mut run, "fresh host loaded");
    if host(&mut run).movie_progress().is_some() {
        skip_logistics_movie(&mut run);
    }
    launch_current_mission(&mut run, &mut trace_log, "mc2_02");
    if host(&mut run).movie_progress().is_some() {
        skip_logistics_movie(&mut run);
    }
    assert_first_live_frame(&mut run, &mut trace_log, "mc2_02 (loaded host)");
    assert_no_unavailable(&mut run);
    let _ = std::fs::remove_dir_all(&saves);
    let _ = writeln!(
        std::io::stderr(),
        "P4 campaign acceptance wall time: {:.1}s ({} stages)",
        started.elapsed().as_secs_f64(),
        trace_log.len()
    );
}
