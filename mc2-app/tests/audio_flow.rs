// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The app's game sound over the shipped game data (`MC2_DATA`; every test returns early without it).
//!
//! Headless throughout: the mixer is rendered into buffers by [`AppAudio::render_offline`], and no
//! output device is opened. The screens are the real `mc2-ui` ones, driven frame by frame.

use mc2_app::audio::{self, tune, AppAudio, Cue};
use mc2_audio::ids::{music, sfx};
use mc2_campaign::{GameData as CampaignData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_render::game::GameData;
use mc2_ui::ctx::betty;
use mc2_ui::harness::events::{left, move_to};
use mc2_ui::harness::Harness;
use mc2_ui::logistics::briefing::Briefing;
use mc2_ui::logistics::mission_select::MissionSelect;
use mc2_ui::logistics::pilot_ready::PilotReady;
use mc2_ui::screens::main_menu::{MainMenu, MainMenuAction, MM_MSG_NEW_CAMPAIGN};
use mc2_ui::screens::{MainMenuFrame, OptionsFrame};
use mc2_ui::{Loader, UpdateCtx};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Game frames per second of the screens (`Harness::dt`) and of the offline render.
const DT: f32 = 1.0 / 30.0;

fn root() -> Option<PathBuf> {
    std::env::var_os("MC2_DATA").map(PathBuf::from)
}

/// What one run of the scenario produced.
struct Flow {
    /// The mixed output, interleaved stereo, for every frame.
    out: Vec<f32>,
    /// Where each frame's output begins in `out`.
    chunk_starts: Vec<usize>,
    /// The frame of the click on "New campaign" and the sound ids its button asked for.
    click: Option<(usize, Vec<i32>)>,
    /// The tune the current music is, at the end of each phase.
    tunes: Vec<(&'static str, i32)>,
    /// The main menu's `TuneId` (the screen's field) and the tune its first frame asked for.
    menu_tune: i32,
    menu_first_frame: Option<i32>,
    /// The logistics tune of the campaign's first stage.
    logistics_tune: i64,
}

/// The frame driver: runs one update of a screen, plays what it asked for, renders the frame.
struct Driver {
    h: Harness,
    audio: AppAudio,
    out: Vec<f32>,
    chunk_starts: Vec<usize>,
    frame: usize,
    /// The frame whose UI sounds are dropped: the reference run the click is compared against.
    mute_at: Option<usize>,
    /// Render each frame at its real-time length. A streamed track is decoded on a worker thread and
    /// plays silence when the mixer outruns it, so a streamed run must not render faster than real time.
    paced: bool,
}

impl Driver {
    /// One frame: `events` are fed to the input, `f` is the screen's update and returns its own cues
    /// (the music a menu frame asks for, ...). The UI's sounds and those cues are applied, then the
    /// frame is rendered.
    fn tick<R>(
        &mut self,
        events: &[InputEvent],
        f: impl FnOnce(&mut UpdateCtx<'_>) -> (R, Vec<Cue>),
    ) -> R {
        let (r, mut cues) = self.h.frame(events, f);
        cues.extend(audio::cues_from_ui(&self.h.out));
        if self.mute_at == Some(self.frame) {
            cues.retain(|c| !matches!(c, Cue::Sound(_)));
        }
        self.audio.apply_all(&cues);
        self.chunk_starts.push(self.out.len());
        if self.paced {
            std::thread::sleep(std::time::Duration::from_secs_f32(DT));
        }
        self.audio.render_offline(DT, &mut self.out);
        self.frame += 1;
        r
    }

    /// The tune the mixer is playing now.
    fn current_tune(&self) -> i64 {
        self.audio.game().sound.sys.current_music_id()
    }
}

/// Plays the scenario with the music decoded whole up front (deterministic renders).
fn run_flow(root: &Path, mute_at: Option<usize>) -> Result<Flow, String> {
    run_flow_with(root, mute_at, false)
}

/// Plays the scenario: the main menu, a click on "New campaign", the logistics mission selection and the
/// briefing. The UI sounds of frame `mute_at`, if any, are not played. `stream` opens the music as the
/// app's windowed front end does (`stream_loose_files`): loose `.wav` tracks are read from disk while
/// they play.
fn run_flow_with(root: &Path, mute_at: Option<usize>, stream: bool) -> Result<Flow, String> {
    let mut game = GameData::open(root).map_err(|e| e.to_string())?;
    let audio = AppAudio::open(root, &mut game, AppAudio::offline_mixer(), stream)?;
    let campaign = Arc::new(
        CampaignData::load(&mut Vfs::from_system_cfg(root).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?,
    );
    let mut lg = Logistics::with_campaign(campaign, "campaign").map_err(|e| e.to_string())?;
    let logistics_tune = lg.mission_info().current_logistics_tune_id();

    let mut assets = mc2_ui::UiAssets::default();
    let (mut menu, mut select, mut briefing) = {
        let mut ld = Loader::new(&mut game.vfs, &mut assets);
        let mut menu = MainMenu::load(&mut ld)?;
        // The in-game menu: no splash intro in front of it.
        menu.set_draw_background(false);
        let select = MissionSelect::load(&mut ld)?;
        let briefing = Briefing::load(&mut ld)?;
        (menu, select, briefing)
    };
    let menu_tune = menu.tune_id;
    let mut d = Driver {
        h: Harness::with_assets(assets),
        audio,
        out: Vec::new(),
        chunk_starts: Vec::new(),
        frame: 0,
        mute_at,
        paced: stream,
    };
    let mut tunes = Vec::new();

    // Main menu: its tune starts with its first update (`MainMenu::update`, `TuneId`).
    let first: MainMenuFrame = d.tick(&[], |ctx| {
        let f = menu.update(ctx);
        let cues = audio::main_menu_cues(&f);
        (f, cues)
    });
    let reported = first.music;
    let mut click = None;
    for _ in 0..44 {
        d.tick(&[], |ctx| {
            let f = menu.update(ctx);
            let cues = audio::main_menu_cues(&f);
            (f, cues)
        });
    }
    tunes.push(("main menu", d.current_tune() as i32));

    // A click on "New campaign": the button's own click sound is what the click check looks for.
    let button = menu
        .screen
        .button(MM_MSG_NEW_CAMPAIGN)
        .ok_or("no New campaign button")?;
    let (cx, cy) = centre(button.obj.loc);
    d.tick(&[move_to(cx, cy)], |ctx| (menu.update(ctx), Vec::new()));
    for event in [left(true), left(false)] {
        d.tick(&[event], |ctx| (menu.update(ctx), Vec::new()));
        if click.is_none() && !d.h.out.sounds.is_empty() {
            click = Some((d.frame - 1, d.h.out.sounds.clone()));
        }
    }
    // The menu slides away; it reports the action when the slide is done.
    let mut action = None;
    for _ in 0..600 {
        let f = d.tick(&[], |ctx| (menu.update(ctx), Vec::new()));
        if let Some(a) = f.action {
            action = Some(a);
            break;
        }
    }
    if action != Some(MainMenuAction::NewCampaign) {
        return Err(format!("the menu's action was {action:?}"));
    }
    // "New campaign" starts the logistics tune (`LogisticsData::startNewCampaign`, logisticsdata.cpp:2300).
    d.tick(&[], |_| {
        ((), tune::logistics(logistics_tune).into_iter().collect())
    });
    for _ in 0..30 {
        d.tick(&[], |_| ((), Vec::new()));
    }
    tunes.push(("logistics", d.current_tune() as i32));

    // Mission selection: its first update plays the logistics tune (missionselectionscreen.cpp:123-128).
    {
        let mut ld = Loader::new(&mut game.vfs, &mut d.h.assets);
        select.begin_screen(&mut ld, &mut lg)?;
    }
    for i in 0..30 {
        let cues = if i == 0 {
            tune::logistics(lg.mission_info().current_logistics_tune_id())
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        d.tick(&[], |ctx| {
            select.update_screen(ctx, &mut lg);
            ((), cues)
        });
    }
    tunes.push(("mission selection", d.current_tune() as i32));

    // Briefing: no music call of its own (missionbriefingscreen.cpp has none), so the tune carries on.
    {
        let mut ld = Loader::new(&mut game.vfs, &mut d.h.assets);
        briefing.begin_screen(&mut ld, &mut lg);
    }
    for _ in 0..30 {
        d.tick(&[], |ctx| {
            briefing.update_screen(ctx, &mut lg);
            ((), Vec::new())
        });
    }
    tunes.push(("briefing", d.current_tune() as i32));

    Ok(Flow {
        out: d.out,
        chunk_starts: d.chunk_starts,
        click,
        tunes,
        menu_tune,
        menu_first_frame: reported,
        logistics_tune,
    })
}

/// The middle of a button's four corners.
fn centre(loc: [mc2_ui::draw::Vertex; 4]) -> (f32, f32) {
    let x = loc.iter().map(|v| v.x).sum::<f32>() / 4.0;
    let y = loc.iter().map(|v| v.y).sum::<f32>() / 4.0;
    (x, y)
}

#[test]
fn menu_to_selection_to_briefing_streams_the_expected_tunes() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let flow = run_flow(&root, None).expect("scenario");

    // The main menu's tune is the `TuneId` its fit file names, and its first frame asks for it.
    let menu_tune = flow.menu_tune;
    assert!(menu_tune >= 0, "mcl_mm.fit names a tune");
    assert_eq!(
        flow.menu_first_frame,
        Some(menu_tune),
        "first menu frame asks for it"
    );
    assert_eq!(
        flow.tunes[0],
        ("main menu", menu_tune),
        "menu music streams"
    );

    // The logistics tune of the first stage (the campaign's `Tune`), then carried through selection and
    // briefing without a new call.
    assert!(flow.logistics_tune >= 0);
    assert_eq!(flow.tunes[1], ("logistics", flow.logistics_tune as i32));
    assert_eq!(
        flow.tunes[2],
        ("mission selection", flow.logistics_tune as i32)
    );
    assert_eq!(flow.tunes[3], ("briefing", flow.logistics_tune as i32));

    // The streamed tracks are real WAV files of the sound data (`data/sound/<name>.wav`).
    let mut game = GameData::open(&root).expect("game");
    let audio = AppAudio::open(&root, &mut game, AppAudio::offline_mixer(), false).expect("audio");
    let table = audio.game().sound.sys.table().clone();
    for (phase, id) in [
        ("main menu", menu_tune as usize),
        ("logistics", flow.logistics_tune as usize),
    ] {
        let name = &table.music[id].name;
        let path = format!("data/sound/{name}.wav");
        let bytes = game
            .vfs
            .read(&path)
            .expect("read")
            .unwrap_or_else(|| panic!("{phase}: no {path}"));
        assert_eq!(&bytes[..4], b"RIFF", "{phase}: {path} is a WAV");
    }

    // Non-silent at the end: the music is audible, not just the current id.
    let end_energy = {
        let n = flow.out.len();
        let tail = &flow.out[n - 44_100 * 2 / 2..];
        tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32
    };
    assert!(
        end_energy > 1e-7,
        "the stream is audible, energy {end_energy}"
    );
}

/// The same scenario with the music streamed from disk, as the app plays it: the same tunes stream, and
/// they are audible in the mixed output.
#[test]
fn menu_to_selection_to_briefing_streams_from_disk() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let flow = run_flow_with(&root, None, true).expect("streaming scenario");
    let menu_tune = flow.menu_tune;
    assert!(menu_tune >= 0, "mcl_mm.fit names a tune");
    assert_eq!(
        flow.tunes[0],
        ("main menu", menu_tune),
        "menu music streams"
    );
    let logistics = flow.logistics_tune as i32;
    assert!(logistics >= 0);
    assert_eq!(flow.tunes[1], ("logistics", logistics));
    assert_eq!(flow.tunes[2], ("mission selection", logistics));
    assert_eq!(flow.tunes[3], ("briefing", logistics));
    // Audible at the end: the streamed logistics tune is in the output, not only its id.
    let n = flow.out.len();
    let tail = &flow.out[n - 44_100..];
    let energy = tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32;
    assert!(
        energy > 1e-7,
        "the streamed tune is audible, energy {energy}"
    );
}

#[test]
fn button_click_sample_is_mixed_into_its_frame() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let with = run_flow(&root, None).expect("scenario with sounds");
    let (frame, ids) = with.click.clone().expect("the button asked for a click");
    // The same run with only the click's frame muted: the two differ by the click and nothing else.
    let without = run_flow(&root, Some(frame)).expect("scenario without the click");
    // The button's press sound (`set_press_fx(sfx::LOG_VIDEOBUTTONS)`, `mc2_ui::screens::main_menu`).
    assert_eq!(ids, vec![mc2_ui::ctx::sfx::LOG_VIDEOBUTTONS], "click ids");

    // The two runs differ only by the click: the first sample that differs is in the click's frame.
    assert_eq!(with.out.len(), without.out.len());
    let first_diff = with
        .out
        .iter()
        .zip(&without.out)
        .position(|(a, b)| a != b)
        .expect("the click changes the output");
    let start = with.chunk_starts[frame];
    let end = with
        .chunk_starts
        .get(frame + 1)
        .copied()
        .unwrap_or(with.out.len());
    assert!(
        (start..end).contains(&first_diff),
        "click frame {frame} spans samples {start}..{end}, the first difference is at {first_diff}"
    );
    // Nothing before the click's frame differs.
    assert!(with.out[..start] == without.out[..start]);
}

#[test]
fn ui_sound_ids_and_music_ids_resolve_to_samples() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let mut game = GameData::open(&root).expect("game");
    let audio = AppAudio::open(&root, &mut game, AppAudio::offline_mixer(), false).expect("audio");
    let table = audio.game().sound.sys.table().clone();
    let bank = audio.game().sound.sys.bank().clone();

    // The sounds the screens play: the main menu and logistics buttons and the load doors.
    let ui_ids = [
        sfx::BUTTON5,
        sfx::LOG_WRONGBUTTON,
        sfx::LOG_VIDEOBUTTONS,
        sfx::LOG_MAINMENUBUTTON,
        sfx::LOG_HIGHLIGHTBUTTONS,
        sfx::LOG_EXITGAME,
        sfx::LOG_CLICKONBUTTON,
        sfx::LOAD_DOORS_OPENING,
        sfx::LOAD_DOORS_CLOSING,
    ];
    for id in ui_ids {
        let bite = table
            .bites
            .get(id as usize)
            .unwrap_or_else(|| panic!("no bite {id}"));
        assert!(
            bank.effects.sound(bite.sound_id as usize).is_some(),
            "sound {id} has no sample in the bank"
        );
    }

    // The tunes the choice functions name, and the main menu and logistics stages' ones: real tracks.
    for id in [
        music::WIN_TUNE_0,
        music::LOSE_TUNE_0,
        music::LOGISTICS_LOOP,
        music::RESULTS_LOOP,
    ] {
        let track = table.music.get(id as usize).expect("music id in table");
        assert!(!track.is_none(), "music {id} is a real track");
    }
}

#[test]
fn options_volumes_reach_the_mixer() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let mut game = GameData::open(&root).expect("game");
    let mut audio =
        AppAudio::open(&root, &mut game, AppAudio::offline_mixer(), false).expect("audio");
    // The audio tab's five sliders: master, music, sfx, radio, Betty (`optionsarea.cpp:757-777`).
    let frame = OptionsFrame {
        volumes: Some([255, 128, 64, 64, 64]),
        ..Default::default()
    };
    audio.apply_all(&audio::options_cues(&frame));
    let gain = audio.game().sound.sys.music_gain();
    assert!((gain - 128.0 / 256.0).abs() < 1e-6, "music gain {gain}");
    let master = audio.game().sound.sys.digital_master_gain();
    assert!(
        (master - 255.0 / 256.0).abs() < 1e-6,
        "master gain {master}"
    );
}

/// The pilot-ready screen's new-pilots Betty line (`PilotReadyScreen::render`, `BETTY_NEW_PILOTS`, id 47):
/// the screen asks for it in the frame the campaign has new pilots, the app plays it through its cue
/// path, and the voice is mixed into exactly that frame. Only the cue path is covered here: the
/// logistics screens are not hosted by the app yet (see the porting log).
#[test]
fn new_pilots_betty_line_is_mixed_into_its_frame() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let mut game = GameData::open(&root).expect("game");
    let audio = AppAudio::open(&root, &mut game, AppAudio::offline_mixer(), false).expect("audio");
    let campaign = Arc::new(
        CampaignData::load(&mut Vfs::from_system_cfg(&root).expect("vfs")).expect("campaign"),
    );
    let mut lg = Logistics::with_campaign(campaign, "campaign").expect("logistics");
    assert!(
        lg.new_pilots_available(),
        "the new campaign has pilots to announce"
    );

    let mut assets = mc2_ui::UiAssets::default();
    let mut pilot_ready = {
        let mut ld = Loader::new(&mut game.vfs, &mut assets);
        PilotReady::load(&mut ld).expect("pilot ready screen")
    };
    let mut d = Driver {
        h: Harness::with_assets(assets),
        audio,
        out: Vec::new(),
        chunk_starts: Vec::new(),
        frame: 0,
        mute_at: None,
        paced: false,
    };
    {
        let mut ld = Loader::new(&mut game.vfs, &mut d.h.assets);
        pilot_ready.begin_screen(&mut ld, &mut lg);
    }
    let mut betty_frames = Vec::new();
    for i in 0..10 {
        d.tick(&[], |ctx| {
            pilot_ready.update_screen(ctx, &mut lg);
            ((), Vec::new())
        });
        if d.h.out.betty.is_empty() {
            continue;
        }
        betty_frames.push((i, d.h.out.betty.clone()));
    }
    assert_eq!(
        betty_frames,
        vec![(0, vec![betty::NEW_PILOTS])],
        "the line is asked for once, in the first frame"
    );
    assert!(!lg.new_pilots_available(), "the flag is acknowledged");

    // The voice is silent before its frame and audible in it.
    let frame = betty_frames[0].0;
    let chunk = |i: usize| {
        let start = d.chunk_starts[i];
        let end = d.chunk_starts.get(i + 1).copied().unwrap_or(d.out.len());
        &d.out[start..end]
    };
    let energy = |s: &[f32]| s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32;
    assert!(
        (0..frame).all(|i| energy(chunk(i)) == 0.0),
        "nothing plays before the line"
    );
    assert!(
        energy(chunk(frame)) > 1e-7,
        "the line is audible in its frame, energy {}",
        energy(chunk(frame))
    );
}
