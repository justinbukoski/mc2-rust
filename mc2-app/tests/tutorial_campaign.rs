// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The tutorial campaign (`feat/rust-tutorials`): `tutorial.fit` is offered beside the campaign by the New Campaign list
//! (`LogisticsSaveDialog::beginCampaign`, `code/logisticsdialog.cpp:491-603`), starts its first mission, and each of its five
//! single-stage missions loads and runs headless for 9,000 frames without a crash.
//!
//! * `new_campaign_offers_the_tutorial_and_starts_tutorial_one`: the production host (`HeadlessRun::production`) from the main
//!   menu: New Campaign opens the list (campaign, tutorial), the tutorial row starts `tut_01`, and the pages lead to the loaded
//!   mission. A missing GPU adapter skips it only with `MC2_SKIP_GPU_TESTS=1`.
//! * `every_tutorial_mission_runs_9000_frames_headless`: `Logistics::with_campaign("tutorial")` hands each mission's `LoadSpec` to
//!   the production mission loader (`mc2 --headless`); the mission runs 9,000 frames with the live simulation.
//!
//! Needs `MC2_DATA`.

use mc2_app::campaign_player::play_mission;
use mc2_app::headless::{HeadlessOptions, HeadlessRun, ProductionDriver};
use mc2_app::state::GameState;
use mc2_app::ui_host::{ActiveScreen, UiHost};
use mc2_campaign::interface::{MissionOptions, Outcome};
use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_platform::keys;
use mc2_ui::logistics::ids;
use mc2_ui::logistics::sequence::slot;
use mc2_ui::screens::main_menu::MM_MSG_NEW_CAMPAIGN;
use mc2_ui::strings::StringTable;
use std::path::PathBuf;
use std::sync::Arc;

const TUTORIAL: [&str; 5] = ["tut_01", "tut_02", "tut_03", "tut_04", "tut_05"];
/// `IDS_*` of the two names: `NameID` of `campaign.fit` and `tutorial.fit`.
const CAMPAIGN_NAME_ID: u32 = 60021;
const TUTORIAL_NAME_ID: u32 = 60022;
const SIZE: (u32, u32) = (800, 600);

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
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

/// A player's click: move, press, release, each in its own frame.
fn click(run: &mut HeadlessRun<ProductionDriver>, ui: (f32, f32)) {
    frames(run, 8);
    run.event(InputEvent::CursorMoved {
        x: f64::from(ui.0),
        y: f64::from(ui.1),
    });
    frames(run, 1);
    for pressed in [true, false] {
        run.event(InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        });
        frames(run, 1);
    }
}

fn key(run: &mut HeadlessRun<ProductionDriver>, key: u32) {
    for pressed in [true, false] {
        run.event(InputEvent::Key {
            key,
            phys_id: key as u16,
            pressed,
            repeat: false,
            text: None,
        });
        frames(run, 1);
    }
}

#[test]
fn new_campaign_offers_the_tutorial_and_starts_tutorial_one() {
    let Some(root) = data() else { return };
    let mut options = HeadlessOptions::new(root, "");
    options.size = SIZE;
    options.frames = 100_000;
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
    frames(&mut run, 2);
    key(&mut run, keys::KEY_ESCAPE);
    key(&mut run, keys::KEY_ESCAPE);
    frames(&mut run, 90);
    assert_eq!(host(&mut run).screen(), ActiveScreen::MainMenu);

    // New Campaign opens the list, where the C++ puts the campaign and then the tutorial.
    let new_campaign = host(&mut run)
        .menu_button(MM_MSG_NEW_CAMPAIGN)
        .expect("New Campaign button");
    click(&mut run, new_campaign);
    until(&mut run, 120, "the campaign list", |r| {
        host(r).screen() == ActiveScreen::CampaignPick
    });
    let table = StringTable::builtin();
    let rows = host(&mut run).campaign_pick_rows().expect("list is open");
    assert_eq!(
        rows,
        [
            (
                "campaign".to_string(),
                table.load(CAMPAIGN_NAME_ID).to_string()
            ),
            (
                "tutorial".to_string(),
                table.load(TUTORIAL_NAME_ID).to_string()
            ),
        ]
    );
    assert!(rows.iter().all(|(_, name)| !name.is_empty()));

    // Escape leaves the list for the menu without starting anything.
    // (the dialog takes input once its enter slide is done)
    frames(&mut run, 60);
    key(&mut run, keys::KEY_ESCAPE);
    until(&mut run, 120, "the main menu", |r| {
        host(r).screen() == ActiveScreen::MainMenu
    });
    assert!(host(&mut run).campaign().is_none());
    frames(&mut run, 90);

    // The second row is the tutorial.
    let new_campaign = host(&mut run)
        .menu_button(MM_MSG_NEW_CAMPAIGN)
        .expect("New Campaign button");
    click(&mut run, new_campaign);
    until(&mut run, 120, "the campaign list", |r| {
        host(r).screen() == ActiveScreen::CampaignPick
    });
    frames(&mut run, 60);
    let tutorial = host(&mut run).campaign_pick_row_centre(1).unwrap();
    click(&mut run, tutorial);
    // the click selects the row, Enter starts it
    key(&mut run, keys::KEY_RETURN);
    until(&mut run, 120, "tutorial logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    assert_eq!(
        host(&mut run)
            .campaign()
            .expect("campaign")
            .current_mission(),
        "tut_01",
        "the tutorial starts with its first mission"
    );
    // Any pre-operation movie is skipped, then the pages lead to the mission (the first group's logistics route as the data
    // says: the screens it shows are walked by their Next buttons).
    frames(&mut run, 2);
    if host(&mut run).movie_progress().is_some() {
        key(&mut run, keys::KEY_ESCAPE);
    }
    walk_to_loading(&mut run);
    until(&mut run, 600, "live mission", |r| {
        r.core.state() == &GameState::InMission
    });
    assert_eq!(
        run.core.mission.as_ref().map(|m| m.name.as_str()),
        Some("tut_01"),
        "the production loading page started tutorial mission 1"
    );
    frames(&mut run, 30);
    assert!(run.core.last_error.is_none(), "{:?}", run.core.last_error);
}

/// Presses Next on whatever logistics page is up until the loading page (or the mission) is reached.
fn walk_to_loading(run: &mut HeadlessRun<ProductionDriver>) {
    for _ in 0..8 {
        match host(run).screen() {
            ActiveScreen::Logistics(slot::LOADING) => return,
            ActiveScreen::Logistics(page) => {
                if page == slot::PILOT_READY {
                    if let Some(add) = host(run).page_button(page, ids::PR_MSG_ADD) {
                        click(run, add);
                    }
                }
                let next = host(run)
                    .page_button(page, ids::MB_MSG_NEXT)
                    .unwrap_or_else(|| panic!("logistics page {page} has no Next button"));
                click(run, next);
            }
            other => panic!("unexpected screen {other:?} on the way to the mission"),
        }
    }
    panic!("the pages did not reach the loading page");
}

#[test]
fn every_tutorial_mission_runs_9000_frames_headless() {
    let Some(root) = data() else { return };
    let mut vfs = Vfs::from_system_cfg(&root).expect("mount the game archives");
    let game = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let mut logistics =
        Logistics::with_campaign(Arc::clone(&game), "tutorial").expect("the tutorial loads");
    for mission in TUTORIAL {
        assert_eq!(logistics.current_mission(), mission);
        let spec = logistics.load_spec(MissionOptions::default());
        assert_eq!(spec.mission, mission);

        let mut options = HeadlessOptions::new(&root, mission);
        options.size = SIZE;
        options.frames = 9_000;
        options.game.cursor = false;
        options.game.campaign_launch = Some(spec);
        let run = match play_mission(&options) {
            Ok(run) => run,
            Err(e)
                if e.contains("adapter")
                    && std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1") =>
            {
                eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
                return;
            }
            Err(e) => panic!("{mission}: {e}"),
        };
        assert!(
            run.report.sim_frames >= 8_900 || run.report.result.is_some(),
            "{mission}: ran {} simulation frames",
            run.report.sim_frames
        );
        eprintln!(
            "TUTORIAL {mission}: frames={} sim_frames={} result={:?} unavailable={:?}",
            run.report.frames, run.report.sim_frames, run.report.result, run.report.sim_unavailable
        );
        // The campaign moves on, as the results flow would have it.
        logistics
            .apply_outcome(&Outcome::won())
            .unwrap_or_else(|e| panic!("{mission}: advance: {e}"));
    }
    assert!(logistics.campaign_over(), "five wins end the tutorial");
}
