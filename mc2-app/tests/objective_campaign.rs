// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Objective harness, campaign half (`feat/rust-objective-harness`): what the sim reports for a won mission goes through the
//! production campaign path and the campaign moves on to the mission the campaign data says.
//!
//! * `every_campaign_mission_win_advances_the_campaign`: for all 24 missions, the sim plays the mission by the test-only
//!   world-state hooks (`mc2_sim::test_hooks`, see `mc2-sim/tests/objective_harness.rs`), its end-of-mission report
//!   (`Outcome`) is converted by the app's own `outcome::to_campaign` and fed to the production `mc2_campaign::Logistics`
//!   end-of-mission path (`apply_outcome` = `LogisticsData::setMissionCompleted`, then the results flow: salvage, pilot review,
//!   the autosave decision, then the mission selection). Asserted: the campaign offered exactly the mission just played, the
//!   mission completes, the objectives' resource points are paid (`logisticsdata.cpp:1315-1408`), and the campaign advances to the
//!   next mission of `data/campaign/campaign.fit` (`logisticsmissioninfo.cpp:785-870`, `logistics.cpp:121-180`), or is over after
//!   the last.
//! * `mc2_01_/mc2_10_/mc2_24_...results_screens...`: the same win played inside the production headless app
//!   (`HeadlessRun::production`, the session behind `mc2 --headless`), through the app's results screens to logistics. A missing
//!   GPU adapter skips them only with `MC2_SKIP_GPU_TESTS=1`.
//!
//! Headless only. Needs `MC2_DATA`.

use std::path::PathBuf;
use std::sync::Arc;

use mc2_app::campaign_save::CampaignSaveService;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::live_sim::LiveSim;
use mc2_app::outcome::to_campaign;
use mc2_app::state::GameState;
use mc2_app::ui_host::ActiveScreen;
use mc2_campaign::interface::{ObjectiveStatus, Outcome};
use mc2_campaign::results::ResultsStage;
use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_sim::test_hooks::{Harness, MissionRun, Player};
use mc2_sim::{PlayerCommand, World};
use mc2_sim_data::mission::{Action, Mission};

/// `mis_PLAYER_WIN_BIG` (`code/mission.h:41-46`).
const WIN_BIG: u32 = 5;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

fn vfs() -> Option<Vfs> {
    Some(Vfs::from_system_cfg(data()?).expect("mount the game archives"))
}

/// The campaign's missions in file order (`data/campaign/campaign.fit`, `Group<i>Mission<j> FileName`,
/// `logisticsmissioninfo.cpp:124-268`). In this campaign the default route (the first open mission each time) plays them in
/// exactly this order: the quota groups' remaining missions follow the played ones, the mandatory hidden mission last.
fn campaign_order(vfs: &mut Vfs, script: &str) -> Vec<String> {
    let mut fit = vfs
        .read_fit(&format!("data/campaign/{script}.fit"))
        .unwrap_or_else(|e| panic!("{script}.fit: {e}"))
        .unwrap_or_else(|| panic!("{script}.fit exists"));
    fit.seek_block("Campaign").expect("[Campaign]");
    let groups = fit.read_i64("GroupCount").expect("GroupCount");
    let mut order = Vec::new();
    for i in 0..groups {
        fit.seek_block(&format!("Group{i}")).expect("group");
        let count = fit.read_i64("MissionCount").unwrap_or(0);
        for j in 0..count {
            fit.seek_block(&format!("Group{i}Mission{j}"))
                .expect("mission");
            order.push(fit.read_string("FileName", 255).expect("FileName"));
        }
    }
    order
}

/// What the mission file says: the resource points of each objective of the home team and the purchase lists its actions unlock.
fn mission_file(vfs: &mut Vfs, mission: &str) -> Vec<(u32, Vec<String>)> {
    let m =
        Mission::load_named(vfs, &format!("data/missions/{mission}.fit")).expect("mission file");
    let team = m.teams.iter().find(|t| t.team == 0).expect("home team");
    team.objectives
        .iter()
        .map(|o| {
            let files = o
                .actions
                .iter()
                .filter_map(|a| match a {
                    Action::MakeNewTechnologyAvailable { purchase_file_name } => {
                        Some(purchase_file_name.clone())
                    }
                    _ => None,
                })
                .collect();
            (o.resource_points, files)
        })
        .collect()
}

/// Plays the 24 missions in parallel (each world stays in its own thread).
fn play_all(names: &[String]) -> Vec<MissionRun> {
    let root = data().expect("MC2_DATA");
    let mut runs: Vec<Option<MissionRun>> = vec![None; names.len()];
    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for chunk in (0..names.len()).collect::<Vec<_>>().chunks(6) {
            let chunk = chunk.to_vec();
            let root = root.clone();
            handles.push(scope.spawn(move || {
                let mut vfs = Vfs::from_system_cfg(&root).expect("mount the game archives");
                chunk
                    .into_iter()
                    .map(|i| (i, Harness::play_mission(&mut vfs, &names[i])))
                    .collect::<Vec<_>>()
            }));
        }
        for h in handles {
            for (i, run) in h.join().expect("a mission thread") {
                runs[i] = Some(run.unwrap_or_else(|e| panic!("{}: {e}", names[i])));
            }
        }
    });
    runs.into_iter().map(Option::unwrap).collect()
}

#[test]
fn every_campaign_mission_win_advances_the_campaign() {
    win_advances_the_campaign("campaign", 24);
}

/// The tutorial (`tutorial.fit`, offered beside the campaign): its five single-stage missions in order, each won by the sim's own
/// hooks, go through the same end-of-mission path, and the last one ends the tutorial.
#[test]
fn every_tutorial_mission_win_advances_the_tutorial() {
    win_advances_the_campaign("tutorial", 5);
}

fn win_advances_the_campaign(script: &str, count: usize) {
    let Some(mut vfs) = vfs() else { return };
    let order = campaign_order(&mut vfs, script);
    assert_eq!(
        order.len(),
        count,
        "{script} lists {count} missions: {order:?}"
    );
    let runs = play_all(&order);
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let mut l = Logistics::new(Arc::clone(&data)).expect("Logistics::new");
    l.start_new_campaign(script).expect("start the campaign");

    for (k, run) in runs.iter().enumerate() {
        let mission = &order[k];
        assert_eq!(&run.mission, mission);
        // The campaign offers the mission about to be played (the one the sim played).
        assert_eq!(
            l.current_mission(),
            mission,
            "step {k}: the campaign's current mission"
        );
        assert!(!l.campaign_over(), "step {k}");

        // The sim's own win (see objective_harness.rs for the per-objective assertions).
        assert_eq!(run.result, WIN_BIG, "{mission}: the sim's result");
        let file = mission_file(&mut vfs, mission);
        assert_eq!(
            file.len(),
            run.outcome.objectives.len(),
            "{mission}: objective count"
        );
        let outcome = to_campaign(run.outcome.clone());
        // Resource points: the file's points of the objectives the sim reports done.
        let expected_points: i64 = outcome
            .objectives
            .iter()
            .zip(&file)
            .filter(|(o, _)| o.status == ObjectiveStatus::Successful)
            .map(|(_, f)| i64::from(f.0))
            .sum();
        let visible_points: i64 = run.visible.iter().map(|v| i64::from(file[v.index].0)).sum();
        // mc2_17 (the escort) is the one mission whose objectives pay nothing (`ResourcePoints` 0 in the file).
        assert!(
            expected_points >= visible_points
                && (expected_points > 0 || mission == "mc2_17" || script == "tutorial"),
            "{mission}: the won objectives pay"
        );
        // The purchase lists the won objectives unlock are the file's, no more.
        let mut expected_files: Vec<String> = outcome
            .objectives
            .iter()
            .zip(&file)
            .filter(|(o, _)| o.status == ObjectiveStatus::Successful)
            .flat_map(|(_, f)| f.1.clone())
            .collect();
        expected_files.sort();
        let mut got_files = outcome.unlocked_purchase_files.clone();
        got_files.sort();
        assert_eq!(
            got_files, expected_files,
            "{mission}: unlocked purchase lists"
        );

        let cbills = l.c_bills();
        let effect = l.apply_outcome(&outcome).expect("the end-of-mission path");
        assert!(
            effect.completed,
            "{mission}: a big win completes the mission"
        );
        assert_eq!(
            l.c_bills() - cbills,
            expected_points,
            "{mission}: the successful objectives' resource points are paid"
        );

        // The results flow the screens run: salvage, then the pilot review (promotions pick a specialty), then the autosave.
        let mut flow = l.begin_results(&outcome);
        let last = k + 1 == order.len();
        if last {
            assert!(l.campaign_over(), "the last mission ends the campaign");
            assert!(flow.is_done(), "no results screens after the last mission");
            assert!(flow.end(&l).is_none(), "no autosave after the last mission");
            continue;
        }
        if flow.stage() == ResultsStage::Salvage {
            flow.finish_salvage(&mut l);
        }
        while let Some(pilot) = flow.pending_specialty(&l) {
            let skill = l.pilots()[pilot].specialty_offers()[0];
            flow.choose_specialty(&mut l, pilot, skill)
                .expect("pick a specialty");
        }
        flow.finish_pilot_review(&mut l);
        assert!(flow.is_done(), "{mission}: the results flow finished");
        let save = flow.end(&l).expect("the autosave after the results");
        assert_eq!(save.mission(), mission);

        // The mission selection screen presses the campaign's mission, which is the next one of the data.
        let choices = l.begin_mission_selection().expect("mission selection");
        let pressed: Vec<&str> = choices
            .iter()
            .filter(|c| c.selected)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(
            pressed,
            [order[k + 1].as_str()],
            "{mission}: the campaign advances to the next mission"
        );
        assert_eq!(l.current_mission(), order[k + 1]);
        assert_eq!(l.last_mission(), mission);
    }
    assert!(l.campaign_over(), "{script} is over after mission {count}");
}

// ---------------------------------------------------------------------------------------------------------------------------
// The same win in the production headless app: results screens to logistics.
// ---------------------------------------------------------------------------------------------------------------------------

/// Game pixels per UI pixel (the 800x600 layouts fitted to 1024x768).
const SCALE: f32 = 1.28;
const SIZE: (u32, u32) = (1024, 768);

/// The app's live simulation as a [`Player`]: the hooks reach the world the app steps; `step_frame` is a real app frame.
struct AppPlayer<'a> {
    run: &'a mut HeadlessRun<LiveSim>,
}

impl AppPlayer<'_> {
    fn sim_world(&self) -> &World {
        &self
            .run
            .core
            .mission
            .as_ref()
            .expect("a mission is running")
            .driver
            .sim()
            .world
    }
}

impl Player for AppPlayer<'_> {
    fn world(&self) -> &World {
        self.sim_world()
    }
    fn world_mut(&mut self) -> &mut World {
        &mut self
            .run
            .core
            .mission
            .as_mut()
            .expect("a mission is running")
            .driver
            .sim_mut()
            .world
    }
    fn step_frame(&mut self) -> Result<(), String> {
        self.run.step(None).map(|_| ())
    }
    fn ended(&self) -> bool {
        matches!(self.run.core.state(), GameState::Results(_))
            || self.sim_world().mission.term.awaiting_results
    }
    fn note(&mut self, _line: String) {}
}

fn frames(run: &mut HeadlessRun<LiveSim>, n: usize) {
    for _ in 0..n {
        run.step(None).unwrap();
    }
}

/// Objective hooks advance simulation state, so end an authored mission movie
/// through the script-visible `ForceMovieEnd` state before driving them. The
/// player Escape path itself is covered by `presentation_live` and `hud_live`.
fn dismiss_opening_movie(run: &mut HeadlessRun<LiveSim>) {
    if !run
        .core
        .mission
        .as_ref()
        .is_some_and(|mission| mission.sim_presentation.movie_mode)
    {
        return;
    }
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .inject_commands(vec![PlayerCommand::ForceMovieEnd]);
    for _ in 0..400 {
        if !run
            .core
            .mission
            .as_ref()
            .is_some_and(|mission| mission.sim_presentation.movie_mode)
        {
            frames(run, 1);
            return;
        }
        frames(run, 1);
    }
    panic!("the opening movie did not end after Escape");
}

fn click(run: &mut HeadlessRun<LiveSim>, ui: (f32, f32)) {
    frames(run, 8);
    run.event(InputEvent::CursorMoved {
        x: f64::from(ui.0 * SCALE),
        y: f64::from(ui.1 * SCALE),
    });
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

/// A campaign whose current mission is `mission`: the missions before it won (`Outcome::won()`), selected the way the screen does.
fn campaign_at(data: &Arc<GameData>, mission: &str) -> Logistics {
    let mut l = Logistics::new(Arc::clone(data)).expect("Logistics::new");
    l.start_new_campaign("campaign")
        .expect("start the campaign");
    for _ in 0..30 {
        if l.current_mission() == mission {
            return l;
        }
        l.apply_outcome(&Outcome::won()).expect("win");
        let current = l.current_mission().to_string();
        l.set_current_mission(&current)
            .expect("select the next mission");
    }
    panic!("the campaign never reached {mission}");
}

/// The production headless app at its main menu (no mission on the command line), skipping the test without a GPU adapter.
fn start_at_menu() -> Option<HeadlessRun<LiveSim>> {
    let mut opts = HeadlessOptions::new(data()?, "");
    opts.size = SIZE;
    opts.game.cursor = false;
    match HeadlessRun::production(&opts) {
        Ok(r) => Some(r),
        Err(e)
            if e.contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("the production headless session needs a GPU adapter and the data: {e}"),
    }
}

fn host(run: &mut HeadlessRun<LiveSim>) -> &mut mc2_app::ui_host::UiHost {
    run.core
        .ui_host_mut()
        .expect("the production host is installed")
}

fn screen(run: &mut HeadlessRun<LiveSim>) -> ActiveScreen {
    host(run).screen()
}

/// Chooses a row of the campaign list New Campaign opens (`LogisticsSaveDialog::beginCampaign`).
fn choose_campaign(run: &mut HeadlessRun<LiveSim>, row: usize) {
    until(run, 60, "campaign list", |r| {
        screen(r) == ActiveScreen::CampaignPick
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
    run: &mut HeadlessRun<LiveSim>,
    max: usize,
    what: &str,
    done: impl Fn(&mut HeadlessRun<LiveSim>) -> bool,
) {
    for _ in 0..max {
        if done(run) {
            return;
        }
        run.step(None).unwrap();
    }
    assert!(done(run), "{what}: not reached in {max} frames");
}

fn key(run: &mut HeadlessRun<LiveSim>, key: u32) {
    run.event(mc2_ui::harness::events::key(key, true, None));
    frames(run, 1);
    run.event(mc2_ui::harness::events::key(key, false, None));
    frames(run, 1);
}

fn pointer(run: &mut HeadlessRun<LiveSim>, ui: (f32, f32)) {
    run.event(InputEvent::CursorMoved {
        x: f64::from(ui.0 * SCALE),
        y: f64::from(ui.1 * SCALE),
    });
}

fn button(run: &mut HeadlessRun<LiveSim>, down: bool) {
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed: down,
    });
}

/// The menu path of a new campaign by pointer events (the way `ui_host.rs` does it): main menu, new campaign, mission selection,
/// briefing, mech bay (a stored mech dragged onto the first slot), pilot ready.
fn to_pilot_ready(run: &mut HeadlessRun<LiveSim>) {
    use mc2_ui::logistics::ids;
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::sequence::slot;
    frames(run, 2);
    assert_eq!(screen(run), ActiveScreen::MainMenu);
    for _ in 0..2 {
        key(run, mc2_platform::keys::KEY_ESCAPE);
    }
    frames(run, 90);
    // the quickstart RESTART route (`missionbegin.cpp:478`) skips the pages walked here: start at mission selection
    host(run).start_campaign_at_selection();
    let at = host(run)
        .menu_button(mc2_ui::screens::main_menu::MM_MSG_NEW_CAMPAIGN)
        .expect("new campaign button");
    click(run, at);
    choose_campaign(run, 0);
    until(run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    // the first group's pre-operation cutscene
    frames(run, 1);
    key(run, mc2_platform::keys::KEY_ESCAPE);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::SELECT));
    assert_eq!(host(run).campaign().unwrap().current_mission(), "mc2_01");
    {
        // the purchase screen is not ported: stand in for it with a purchase on the campaign
        let lg = host(run).campaign_mut().unwrap();
        let mut variants = lg.purchasable_mechs();
        variants.sort_by_key(|v| lg.variant_cost(*v));
        lg.add_c_bills(200_000);
        lg.purchase_mech(variants[0]).expect("purchase");
        lg.purchase_mech(variants[0]).expect("purchase");
    }
    let at = host(run)
        .page_button(slot::SELECT, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
    let at = host(run)
        .page_button(slot::BRIEFING, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::MECH_BAY));
    let (from, to) = {
        let seq = host(run).sequence_mut().unwrap();
        let bay = seq
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        let r = bay.list.entries().next().expect("a stored mech").box_rect();
        let s = bay.icons[0].outline.global_rect();
        (
            (
                (r.left + r.right) as f32 / 2.0,
                (r.top + r.bottom) as f32 / 2.0,
            ),
            (
                (s.left + s.right) as f32 / 2.0,
                (s.top + s.bottom) as f32 / 2.0,
            ),
        )
    };
    frames(run, 8);
    pointer(run, from);
    frames(run, 1);
    button(run, true);
    frames(run, 1);
    pointer(run, to);
    frames(run, 1);
    button(run, false);
    frames(run, 1);
    let at = host(run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::PILOT_READY));
    let at = host(run)
        .page_button(slot::PILOT_READY, ids::PR_MSG_ADD)
        .unwrap();
    click(run, at);
}

/// Gives a campaign positioned at a later mission a deployed, crewed mech (the purchase and bay screens the test does not replay).
fn deploy_a_mech(lg: &mut Logistics) {
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    lg.add_c_bills(200_000);
    let mech = lg.purchase_mech(variants[0]).expect("purchase");
    lg.add_mech_to_force_group(mech, 1).expect("deploy");
    let pilot = lg
        .pilots()
        .iter()
        .position(|p| p.available && !p.used)
        .expect("a free pilot");
    lg.set_mech_pilot(mech, Some(pilot));
}

/// Plays `mission` in the production app by hooks: new campaign (positioned at `mission`) -> loading -> mission -> win -> results
/// screens -> logistics. `expected_next` is the mission the campaign must offer afterwards; `None` for the last mission.
fn win_in_the_app(mission: &str, expected_next: Option<&str>) {
    let Some(mut run) = start_at_menu() else {
        return;
    };
    let mut vfs = vfs().unwrap();
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let temp = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp.join(format!(
        "mc2-objective-harness-{mission}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&saves);
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));
    to_pilot_ready(&mut run);
    if mission != "mc2_01" {
        // The campaign as it stands before `mission` (earlier missions won), with a crewed mech deployed.
        let mut campaign = campaign_at(&data, mission);
        deploy_a_mech(&mut campaign);
        host(&mut run).replace_campaign_for_result_test(campaign);
    }
    let cbills_before = host(&mut run).campaign().unwrap().c_bills();
    let at = host(&mut run)
        .page_button(
            mc2_ui::logistics::sequence::slot::PILOT_READY,
            mc2_ui::logistics::ids::MB_MSG_NEXT,
        )
        .unwrap();
    click(&mut run, at);
    until(&mut run, 900, "the mission", |r| {
        r.core.state() == &GameState::InMission
    });
    assert_eq!(
        run.core.mission.as_ref().unwrap().name,
        mission,
        "the app loaded the campaign's mission"
    );
    frames(&mut run, 10);
    dismiss_opening_movie(&mut run);

    // Play the objectives by world facts, inside the app's own frame loop (the HUD's results and the sim's 5 s timer run).
    let mut player = AppPlayer { run: &mut run };
    let order = player.play_to_win().expect("play the mission");
    assert!(!order.is_empty());
    until(&mut run, 40 * 30, "the results", |r| {
        matches!(r.core.state(), GameState::Results(_))
    });
    assert_eq!(
        run.core.state(),
        &GameState::Results(WIN_BIG),
        "{mission}: the app reaches Results with a big win"
    );
    frames(&mut run, 2);

    match expected_next {
        Some(next) => {
            assert_eq!(screen(&mut run), ActiveScreen::Results);
            let salvage_done = host(&mut run)
                .results_button(mc2_ui::mission_results::SALVAGE_DONE)
                .expect("salvage Done button");
            click(&mut run, salvage_done);
            // the salvage screen's exit animation runs before the pilot review shows
            until(&mut run, 120, "the pilot review", |r| {
                host(r)
                    .results_button(mc2_ui::mission_results::PILOT_DONE)
                    .is_some()
            });
            // the review slides in and reveals each pilot; a promoted pilot's pop-up is picked
            // by pointer, and the review's Done is enabled once the list is done
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
            let pilot_done = host(&mut run)
                .results_button(mc2_ui::mission_results::PILOT_DONE)
                .expect("pilot review Done button");
            click(&mut run, pilot_done);
            until(&mut run, 60, "logistics after the results", |r| {
                r.core.state() == &GameState::Logistics
            });
            let campaign = host(&mut run).campaign().expect("the campaign");
            assert_eq!(campaign.last_mission(), mission);
            assert_eq!(
                campaign.current_mission(),
                next,
                "{mission}: the campaign moved on"
            );
            assert!(
                campaign.c_bills() > cbills_before,
                "{mission}: the objectives paid"
            );
            assert!(
                saves.join("autosave.ron").is_file(),
                "the results path autosaved"
            );
        }
        None => {
            // The last mission: no results screens, the campaign is over and autosaved as completed.
            until(&mut run, 60, "the final autosave", |r| {
                saves.join("autosave.ron").is_file()
                    && host(r).campaign().is_some_and(Logistics::campaign_over)
            });
            assert!(host(&mut run)
                .notices()
                .iter()
                .any(|n| n == "Campaign completed and autosaved"));
        }
    }
    let _ = std::fs::remove_dir_all(saves);
}

#[test]
fn mc2_01_win_by_hooks_reaches_the_results_screens_and_logistics() {
    win_in_the_app("mc2_01", Some("mc2_02"));
}

#[test]
fn mc2_10_win_by_hooks_reaches_the_results_screens_and_logistics() {
    win_in_the_app("mc2_10", Some("mc2_11"));
}

#[test]
fn mc2_24_final_win_by_hooks_ends_the_campaign() {
    win_in_the_app("mc2_24", None);
}

/// Starts tutorial 1 through the production menus, completes its result screens, then launches tutorial 2.
#[test]
fn tutorial_results_flow_reaches_the_next_mission_launch() {
    let Some(mut run) = start_at_menu() else {
        return;
    };
    frames(&mut run, 2);
    key(&mut run, mc2_platform::keys::KEY_ESCAPE);
    key(&mut run, mc2_platform::keys::KEY_ESCAPE);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let new_campaign = host(&mut run)
        .menu_button(mc2_ui::screens::main_menu::MM_MSG_NEW_CAMPAIGN)
        .expect("New Campaign button");
    click(&mut run, new_campaign);
    choose_campaign(&mut run, 1);
    until(&mut run, 120, "tutorial logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    assert_eq!(
        host(&mut run).campaign().unwrap().current_mission(),
        "tut_01"
    );
    if host(&mut run).movie_progress().is_some() {
        key(&mut run, mc2_platform::keys::KEY_ESCAPE);
    }
    walk_logistics_to_loading(&mut run);
    until(&mut run, 600, "tutorial 1 mission", |r| {
        r.core.state() == &GameState::InMission
    });
    assert_eq!(run.core.mission.as_ref().unwrap().name, "tut_01");
    frames(&mut run, 10);
    dismiss_opening_movie(&mut run);

    let mut player = AppPlayer { run: &mut run };
    let order = player
        .play_to_win()
        .expect("complete tutorial 1 objectives");
    assert!(!order.is_empty());
    until(&mut run, 40 * 30, "tutorial 1 results", |r| {
        matches!(r.core.state(), GameState::Results(_))
    });
    assert_eq!(run.core.state(), &GameState::Results(WIN_BIG));
    until(
        &mut run,
        120,
        "tutorial 2 logistics after skipped results",
        |r| r.core.state() == &GameState::Logistics,
    );
    assert_eq!(
        host(&mut run).campaign().unwrap().current_mission(),
        "tut_02"
    );

    if host(&mut run).movie_progress().is_some() {
        key(&mut run, mc2_platform::keys::KEY_ESCAPE);
    }
    walk_logistics_to_loading(&mut run);
    until(&mut run, 600, "tutorial 2 mission", |r| {
        r.core.state() == &GameState::InMission
    });
    assert_eq!(run.core.mission.as_ref().unwrap().name, "tut_02");
}

fn walk_logistics_to_loading(run: &mut HeadlessRun<LiveSim>) {
    use mc2_ui::logistics::sequence::slot;
    for _ in 0..8 {
        match screen(run) {
            ActiveScreen::Logistics(slot::LOADING) => return,
            ActiveScreen::Logistics(page) => {
                if page == slot::PILOT_READY {
                    if let Some(add) =
                        host(run).page_button(page, mc2_ui::logistics::ids::PR_MSG_ADD)
                    {
                        click(run, add);
                    }
                }
                let next = host(run)
                    .page_button(page, mc2_ui::logistics::ids::MB_MSG_NEXT)
                    .unwrap_or_else(|| panic!("logistics page {page} has no Next button"));
                click(run, next);
            }
            other => panic!("unexpected screen {other:?} on the way to the mission"),
        }
    }
    panic!("the logistics pages did not reach loading");
}
