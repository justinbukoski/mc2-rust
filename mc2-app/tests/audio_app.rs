// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The game sound through the app's own frame: [`GameCore::frame`] applies what the UI layer asked for,
//! updates the sound, and the test mixes that frame with [`AppAudio::mix_frame`]. The UI layer is the
//! real `mc2-ui` main menu and options screens, hosted by [`MenuHost`] (the app's stand-in GUI does not
//! host screens yet, see the porting log). Real data (`MC2_DATA`) and a headless GPU; every test returns
//! early without them. No window, no device.

use mc2_app::audio::{self, AppAudio, Cue};
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{UiFrame, UiLayer};
use mc2_app::sim::{FrameInput, FrameOutput, NullSim, SimDriver};
use mc2_app::state::{GameState, StateRequest};
use mc2_audio::ids::music;
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_render::game::GameData;
use mc2_ui::ctx::sfx;
use mc2_ui::harness::events::{left, move_to};
use mc2_ui::harness::Harness;
use mc2_ui::screens::main_menu::{MainMenu, MainMenuAction, MM_MSG_NEW_CAMPAIGN, MM_MSG_OPTIONS};
use mc2_ui::screens::{OptionsEnv, OptionsScreen, Prefs};
use mc2_ui::{Loader, UiAssets, UpdateCtx};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// The options screen's audio tab (`OptionsScreen::update`, `cur_tab == 1`).
const AUDIO_TAB: usize = 1;
/// One game frame: 30 per second, so a frame is 1470 stereo samples at 44.1 kHz.
const DT: f32 = 1.0 / 30.0;

fn root() -> Option<PathBuf> {
    std::env::var_os("MC2_DATA").map(PathBuf::from)
}

/// The test's requests to the host, which the core owns while it runs.
#[derive(Default)]
struct Control {
    /// Drops the UI sounds of the frame (the reference run of a click).
    mute_sounds: bool,
    /// Selects a tab of the options screen at the next frame.
    options_tab: Option<usize>,
}

/// The real main menu, and the options screen it opens, hosted as the core's UI layer. It asks for no
/// state change: the core stays where the test puts it.
struct MenuHost {
    game: GameData,
    harness: Harness,
    menu: MainMenu,
    options: Option<OptionsScreen>,
    control: Rc<RefCell<Control>>,
    /// Every cue the host handed to the core, for the test to read per frame.
    log: Rc<RefCell<Vec<Cue>>>,
    pending: Vec<Cue>,
}

impl MenuHost {
    fn open(
        root: &Path,
        control: Rc<RefCell<Control>>,
        log: Rc<RefCell<Vec<Cue>>>,
    ) -> Result<Self, String> {
        let mut game = GameData::open(root).map_err(|e| e.to_string())?;
        let mut assets = UiAssets::default();
        let mut menu = {
            let mut ld = Loader::new(&mut game.vfs, &mut assets);
            MainMenu::load(&mut ld)?
        };
        // The in-game menu: no splash intro in front of it (as the screens' own tests do).
        menu.set_draw_background(false);
        Ok(Self {
            game,
            harness: Harness::with_assets(assets),
            menu,
            options: None,
            control,
            log,
            pending: Vec::new(),
        })
    }

    fn open_options(&mut self) -> Result<OptionsScreen, String> {
        let mut f = self.game.fit("options.cfg").map_err(|e| e.to_string())?;
        let prefs = Prefs::from_fit(&mut f);
        let mut ld = Loader::new(&mut self.game.vfs, &mut self.harness.assets);
        OptionsScreen::load(&mut ld, prefs.clone(), prefs, &OptionsEnv::default())
    }
}

impl UiLayer for MenuHost {
    fn handle_event(&mut self, _state: &GameState, event: &InputEvent) -> bool {
        self.harness.input.handle(event);
        false
    }

    fn update(&mut self, frame: &mut UiFrame<'_>) -> Option<StateRequest> {
        if *frame.state != GameState::MainMenu {
            return None;
        }
        self.harness.dt = frame.dt;
        let tab = self.control.borrow_mut().options_tab.take();
        let mut cues = Vec::new();
        if let Some(o) = self.options.as_mut() {
            if let Some(tab) = tab {
                o.select_tab(tab);
            }
            let of = self
                .harness
                .frame(&[], |ctx: &mut UpdateCtx<'_>| o.update(ctx));
            cues.extend(audio::options_cues(&of));
            if of.result.is_some() || o.is_done() {
                self.options = None;
            }
        } else {
            let mf = self.harness.frame(&[], |ctx| self.menu.update(ctx));
            cues.extend(audio::main_menu_cues(&mf));
            if mf.action == Some(MainMenuAction::Options) {
                match self.open_options() {
                    Ok(o) => self.options = Some(o),
                    Err(e) => panic!("options screen: {e}"),
                }
            }
        }
        cues.extend(audio::cues_from_ui(&self.harness.out));
        if self.control.borrow().mute_sounds {
            cues.retain(|c| !matches!(c, Cue::Sound(_)));
        }
        self.pending = cues;
        None
    }

    fn take_audio(&mut self) -> Vec<Cue> {
        let cues = std::mem::take(&mut self.pending);
        self.log.borrow_mut().extend(cues.iter().cloned());
        cues
    }
}

/// The middle of a button's four corners, in the screen's coordinates.
fn centre(loc: [mc2_ui::draw::Vertex; 4]) -> (f32, f32) {
    let x = loc.iter().map(|v| v.x).sum::<f32>() / 4.0;
    let y = loc.iter().map(|v| v.y).sum::<f32>() / 4.0;
    (x, y)
}

/// A headless session on `mission`'s driver, or `None` without a GPU adapter.
fn session<D: SimDriver + 'static>(
    root: &Path,
    factory: mc2_app::game::DriverFactory<D>,
) -> Option<HeadlessRun<D>> {
    let mut opts = HeadlessOptions::new(root, "mc2_01");
    opts.size = (800, 600);
    opts.game.cursor = false;
    match HeadlessRun::with_driver(&opts, factory) {
        Ok(run) => Some(run),
        Err(e) if e.contains("no suitable GPU adapter") => {
            eprintln!("no GPU adapter for headless rendering; skipping audio app test: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

/// What one frame of a run produced.
struct Frames {
    /// The mixed output, interleaved stereo, and where each frame's part starts in it.
    out: Vec<f32>,
    starts: Vec<usize>,
    /// The cues the core applied in each frame.
    cues: Vec<Vec<Cue>>,
    /// The tune the mixer was playing at the end of each frame.
    tunes: Vec<i64>,
}

/// Drives the core by one frame: `events` first, then [`GameCore::frame`], then the frame is mixed.
fn step(
    run: &mut HeadlessRun<impl SimDriver + 'static>,
    frames: &mut Frames,
    events: &[InputEvent],
) {
    for e in events {
        run.core.handle_event(e.clone());
    }
    frames.starts.push(frames.out.len());
    run.core.frame(Duration::from_secs_f32(DT));
    let audio = run.core.audio.as_mut().expect("the session has sound");
    audio.mix_frame(DT, &mut frames.out);
    frames.tunes.push(audio.game().sound.sys.current_music_id());
}

fn empty_frames() -> Frames {
    Frames {
        out: Vec::new(),
        starts: Vec::new(),
        cues: Vec::new(),
        tunes: Vec::new(),
    }
}

/// The main menu, a click on "New campaign", and the frames around it. `mute` drops the UI sounds of
/// one frame (the reference run the click is compared against).
fn run_click(root: &Path, mute: Option<usize>) -> Option<(Frames, Option<usize>, i64)> {
    let control = Rc::new(RefCell::new(Control::default()));
    let log = Rc::new(RefCell::new(Vec::new()));
    let host = MenuHost::open(root, control.clone(), log.clone()).expect("the menu");
    let menu_tune = i64::from(host.menu.tune_id);
    let (cx, cy) = centre(
        host.menu
            .screen
            .button(MM_MSG_NEW_CAMPAIGN)
            .expect("New campaign button")
            .obj
            .loc,
    );
    let mut run = session(root, Box::new(|_| Ok(NullSim::default())))?;
    run.core.ui = Box::new(host);
    run.core.states.request(StateRequest::MainMenu);

    let mut frames = empty_frames();
    let mut click_frame = None;
    // The menu slides in first; the flow test waits the same 44 frames before it clicks.
    let mut script: Vec<Vec<InputEvent>> = vec![vec![]; 44];
    script.push(vec![move_to(cx, cy)]);
    script.push(vec![left(true)]);
    script.push(vec![left(false)]);
    script.extend(std::iter::repeat_n(Vec::new(), 20));
    for (i, events) in script.iter().enumerate() {
        control.borrow_mut().mute_sounds = mute == Some(i);
        step(&mut run, &mut frames, events);
        control.borrow_mut().mute_sounds = false;
        let cues: Vec<Cue> = log.borrow_mut().drain(..).collect();
        if click_frame.is_none() && cues.contains(&Cue::Sound(sfx::LOG_VIDEOBUTTONS)) {
            click_frame = Some(i);
        }
        frames.cues.push(cues);
    }
    Some((frames, click_frame, menu_tune))
}

#[test]
fn main_menu_tune_streams_at_the_game_frame() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let Some((frames, _, menu_tune)) = run_click(&root, None) else {
        return;
    };
    // The menu asks for its `TuneId` in its first frame (`MainMenu::update`, `code/mainmenu.cpp:474-477`),
    // and the mixer plays it from then on.
    assert!(menu_tune >= 0, "mcl_mm.fit names a tune");
    assert!(
        frames.cues[0].contains(&Cue::Music(menu_tune)),
        "first frame cues: {:?}",
        frames.cues[0]
    );
    assert!(frames.tunes.iter().all(|&t| t == menu_tune));
    let end = frames.out.len();
    let tail = &frames.out[end - 44_100..];
    let energy = tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32;
    assert!(energy > 1e-7, "the menu tune is audible, energy {energy}");
}

#[test]
fn click_sample_is_mixed_into_the_frame_of_the_release() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let Some((with, click, _)) = run_click(&root, None) else {
        return;
    };
    let click = click.expect("the button asks for its press sound (90)");
    // The button's press sound (`set_press_fx(sfx::LOG_VIDEOBUTTONS)`, `code/mainmenu.cpp` buttons).
    assert!(with.cues[click].contains(&Cue::Sound(sfx::LOG_VIDEOBUTTONS)));
    let cues_of_click = with.cues[click].clone();

    // The same run with only the click's frame's UI sounds dropped: the two differ by the click alone.
    let (without, _, _) = run_click(&root, Some(click)).expect("a second run");
    assert_eq!(with.out.len(), without.out.len());
    let start = with.starts[click];
    let end = with
        .starts
        .get(click + 1)
        .copied()
        .unwrap_or(with.out.len());
    assert!(
        with.out[..start] == without.out[..start],
        "nothing before the click differs"
    );
    let diff: Vec<f32> = with.out[start..end]
        .iter()
        .zip(&without.out[start..end])
        .map(|(a, b)| a - b)
        .collect();
    assert!(
        diff.iter().any(|d| d.abs() > 1e-6),
        "the click is audible in its frame"
    );

    // The same sounds on their own, rendered by the same sound system: the difference is exactly them.
    let mut game = GameData::open(&root).expect("game");
    let mut solo =
        AppAudio::open(&root, &mut game, AppAudio::offline_mixer(), false).expect("audio");
    for cue in &cues_of_click {
        if let Cue::Sound(_) = cue {
            solo.apply(cue);
        }
    }
    solo.update(DT);
    let mut solo_out = Vec::new();
    solo.mix_frame(DT, &mut solo_out);
    assert_eq!(solo_out.len(), diff.len());
    let worst = solo_out
        .iter()
        .zip(&diff)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        worst < 1e-5,
        "the difference is the click's own samples, worst error {worst}"
    );
}

/// The options screen's audio tab reaches the mixer through the game frame: its volumes are the
/// `options.cfg` prefs, and the sound system is set to them (`code/optionsarea.cpp:757-777`).
#[test]
fn options_audio_tab_volumes_reach_the_mixer_at_the_game_frame() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let control = Rc::new(RefCell::new(Control::default()));
    let log = Rc::new(RefCell::new(Vec::new()));
    let host = MenuHost::open(&root, control.clone(), log.clone()).expect("the menu");
    let (ox, oy) = centre(
        host.menu
            .screen
            .button(MM_MSG_OPTIONS)
            .expect("Options button")
            .obj
            .loc,
    );
    let mut game = GameData::open(&root).expect("game");
    let mut f = game.fit("options.cfg").expect("options.cfg");
    let prefs = Prefs::from_fit(&mut f);
    let Some(mut run) = session(&root, Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    run.core.ui = Box::new(host);
    run.core.states.request(StateRequest::MainMenu);

    let mut frames = empty_frames();
    let mut script: Vec<Vec<InputEvent>> = vec![vec![]; 44];
    script.push(vec![move_to(ox, oy)]);
    script.push(vec![left(true)]);
    script.push(vec![left(false)]);
    script.push(vec![]);
    for events in &script {
        step(&mut run, &mut frames, events);
        frames.cues.push(log.borrow_mut().drain(..).collect());
    }
    // The menu reports Options; the next frame opens the screen. Select its audio tab, then run.
    control.borrow_mut().options_tab = Some(AUDIO_TAB);
    for _ in 0..4 {
        step(&mut run, &mut frames, &[]);
        frames.cues.push(log.borrow_mut().drain(..).collect());
    }
    let volumes = frames
        .cues
        .iter()
        .flatten()
        .find_map(|c| match c {
            Cue::Volumes(v) => Some(*v),
            _ => None,
        })
        .expect("the audio tab sends its volumes");
    let expected = [
        prefs.digital_master_volume,
        prefs.music_volume,
        prefs.sfx_volume,
        prefs.radio_volume,
        prefs.betty_volume,
    ];
    assert_eq!(
        volumes, expected,
        "the audio tab shows the options.cfg prefs"
    );
    let audio = run.core.audio.as_ref().expect("sound");
    let gain = audio.game().sound.sys.music_gain();
    assert!(
        (gain - expected[1] as f32 / 256.0).abs() < 1e-6,
        "music gain {gain} after the frame that sent the volumes"
    );
}

/// A mission whose sim reports `terminationResult` at one step and ends `ends_after` steps later.
struct ScriptedSim {
    steps: u32,
    termination_at: u32,
    ends_at: u32,
    /// The step that produces one presentation event (a sound the audio cannot map yet).
    sound_event_at: Option<u32>,
}

impl SimDriver for ScriptedSim {
    type Command = ();
    type Event = u8;
    type World = ();

    fn step(&mut self, _input: FrameInput<()>) -> FrameOutput<u8> {
        self.steps += 1;
        FrameOutput {
            events: if self.sound_event_at == Some(self.steps) {
                vec![1]
            } else {
                Vec::new()
            },
            // `mis_PLAYER_WIN_BIG`: only a win goes on to the results screens and their loop
            result: if self.steps >= self.ends_at { 5 } else { 0 },
        }
    }

    fn world(&self) -> &() {
        &()
    }

    fn termination_result(&self) -> i32 {
        if self.steps >= self.termination_at {
            1
        } else {
            0
        }
    }
}

/// The sim's sound events have no audio mapping yet (`audio::sim_events`): the mission refuses the first
/// one it produces and says so once, on the frame it is produced. Nothing is refused before it.
#[test]
fn sim_sound_events_are_refused_on_the_frame_they_are_produced() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let Some(mut run) = session(
        &root,
        Box::new(|_| {
            Ok(ScriptedSim {
                steps: 0,
                termination_at: u32::MAX,
                ends_at: u32::MAX,
                sound_event_at: Some(3),
            })
        }),
    ) else {
        return;
    };
    let mut frames = empty_frames();
    let mut refused_at = None;
    for _ in 0..200 {
        step(&mut run, &mut frames, &[]);
        // The mission loads a frame or two after the start (Boot, then MissionLoading).
        let Some(rt) = run.core.mission.as_ref() else {
            continue;
        };
        if rt.sim_sound_refused {
            refused_at = Some(rt.driver.steps);
            break;
        }
    }
    assert_eq!(
        refused_at,
        Some(3),
        "refused on the sim step that produced the event"
    );
}

/// The shipped build requests no win or lose tune at the termination: the forced request is inside `#ifndef
/// FINAL` (`code/mission.cpp:671-709`), and the scripts' own requests (`MissionWonTune0`, `MissionLostTune0`)
/// wait for the ABL music natives. So neither tune plays here, the mission's termination changes no tune,
/// and the results loop starts when the mission ends (`code/missionresults.cpp:188-192`).
#[test]
fn termination_requests_no_win_or_lose_tune() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    const TERMINATION: u32 = 10;
    const ENDS: u32 = TERMINATION + 150;
    let Some(mut run) = session(
        &root,
        Box::new(|_| {
            Ok(ScriptedSim {
                steps: 0,
                termination_at: TERMINATION,
                ends_at: ENDS,
                sound_event_at: None,
            })
        }),
    ) else {
        return;
    };
    let mut frames = empty_frames();
    let win = i64::from(music::WIN_TUNE_0);
    let lose = i64::from(music::LOSE_TUNE_0);
    let results = i64::from(music::RESULTS_LOOP);
    for _ in 0..(ENDS + 40) {
        step(&mut run, &mut frames, &[]);
    }
    assert!(
        frames.tunes.iter().all(|&t| t != win && t != lose),
        "no win or lose tune plays: {:?}",
        frames.tunes
    );
    // The scripted sim reports no outcome, so `MissionResults::begin` does not run and the results loop
    // does not play; the live-sim test `a_won_mission_reaches_the_results_screens...` pins the loop.
    assert!(
        frames.tunes.iter().all(|&t| t != results),
        "no results loop without the results screens: {:?}",
        frames.tunes
    );
}

/// The shipped scripts request the win and lose tunes, and the forced request is gone. Their ids are read
/// from the shipped `sndconst.abi` (`MissionWonTune0` = 9 at line 22, `MissionLostTune0` = 8 at line 23).
/// Those are the ids the forced block played for the same outcomes (`LOSE_TUNE_0` = 9 for a win, `WIN_TUNE_0`
/// = 8 for a loss, `mclib/sounds.h:144-145`; a win is `mis_PLAYER_WIN_BIG` = 5 > `mis_PLAYER_DRAW`, so it takes
/// the `LOSE_TUNE_0` branch of `code/mission.cpp:682-688`). The calls are in `mc2_01.abl`: `getMissionWon` at
/// lines 283-284 and `getMissionLost` at 294-295. The inversion is in the C names only.
#[test]
fn shipped_scripts_request_the_win_and_lose_tunes() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(&root).expect("the data root");
    let mut text = |name: &str| {
        let bytes = vfs
            .read(name)
            .expect("read")
            .unwrap_or_else(|| panic!("{name} is in the data"));
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let consts = text("data/missions/sndconst.abi");
    let tune_id = |name: &str| -> i64 {
        consts
            .lines()
            .find_map(|line| {
                let rest = line.trim_start().strip_prefix(name)?.trim_start();
                rest.strip_prefix('=')?
                    .split(';')
                    .next()?
                    .trim()
                    .parse()
                    .ok()
            })
            .unwrap_or_else(|| panic!("{name} in sndconst.abi"))
    };
    let mission = text("data/missions/mc2_01.abl");
    assert!(mission.contains("playDigitalMusic(MissionWonTune0)"));
    assert!(mission.contains("playDigitalMusic(MissionLostTune0)"));
    assert_eq!(tune_id("MissionWonTune0"), i64::from(music::LOSE_TUNE_0));
    assert_eq!(tune_id("MissionLostTune0"), i64::from(music::WIN_TUNE_0));
}

/// Mission start makes no tune call of its own: the mission's tune is the script's `playdigitalmusic(getmissiontune)`, which
/// runs inside the simulation (`code/ablmc2.cpp:2606-2617`). With no script asking (a `NullSim`), no tune starts.
#[test]
fn mission_start_makes_no_tune_of_its_own() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let Some(mut run) = session(&root, Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    let mut frames = empty_frames();
    for _ in 0..10 {
        step(&mut run, &mut frames, &[]);
    }
    run.core.mission.as_ref().expect("the mission has started");
    let audio = run.core.audio.as_ref().expect("sound");
    assert_eq!(
        audio.game().sound.sys.current_music_id(),
        -1,
        "no tune is started by the app at mission start"
    );
}

/// A scripted UI layer: the cues and the state request a screen would make at each frame, in order. It
/// stands in for the screens, which the stand-in layer does not host (see the porting log).
struct Timeline {
    at: BTreeMap<usize, (Vec<Cue>, Option<StateRequest>)>,
    frame: usize,
    pending: Vec<Cue>,
}

impl UiLayer for Timeline {
    fn update(&mut self, _frame: &mut UiFrame<'_>) -> Option<StateRequest> {
        let (cues, req) = self.at.remove(&self.frame).unwrap_or_default();
        self.frame += 1;
        self.pending = cues;
        req
    }

    fn take_audio(&mut self) -> Vec<Cue> {
        std::mem::take(&mut self.pending)
    }
}

/// `(active, fade time, music id, channel volume)` of music stream `idx` (0 or 1).
fn stream(run: &HeadlessRun<NullSim>, idx: usize) -> (bool, f32, i32, f32) {
    let sys = &run.core.audio.as_ref().expect("sound").game().sound.sys;
    let s = sys.music_stream_state(idx);
    let volume = sys
        .mixer()
        .lock()
        .volume(mc2_audio::system::STREAM1_CHANNEL + idx);
    (s.active, s.fade_time, s.music_id, volume)
}

/// The menu hands over to the logistics tune, and the crossfade runs from the frame after the request.
/// The sound updates before the screens' logic (`code/mechcmd2.cpp:2108` `soundSystem->update()`, then
/// `:2118` `logistics->update()` and the menu logic that requests the music), and `playDigitalMusic` only
/// sets the new stream up (`mclib/soundsys.cpp:673-736`; the stream starts in the next `update`, the first
/// fade-up step is the one of that update).
///
/// Hand-derived from the C with `F` = `StreamFadeDownTime` and `dt` the frame length, after `j` updates
/// past the request (`soundsys.cpp:522-526` fade down, `:597-601` fade up; the volume is
/// `digitalMasterVolume * level * musicVolume * digitalMusicVolume[currentMusicId]`, the track now current):
///   request frame: new stream time `F`, volume 0 (`:732-734`); old stream time `-F` (`:728-731`), its
///   volume not yet changed (no update has run).
///   stream 1 (old): time `-(F - j dt)`, level `(F - j dt) / F`.
///   stream 2 (new): time `F - j dt`, level `j dt / F`, so the first frame's gain is `dt / F`.
#[test]
fn logistics_tune_crossfades_from_the_frame_after_the_request() {
    let Some(root) = root() else {
        eprintln!("MC2_DATA not set: skipped");
        return;
    };
    let control = Rc::new(RefCell::new(Control::default()));
    let log = Rc::new(RefCell::new(Vec::new()));
    let menu_tune = i64::from(
        MenuHost::open(&root, control, log)
            .expect("the menu")
            .menu
            .tune_id,
    );
    // The logistics loop (`LogisticsTune1`, `abi/sndconst.abi`): any stage tune would show the same timing.
    let logistics_tune = i64::from(music::LOGISTICS_LOOP);
    let Some(mut run) = session(&root, Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    let (fade, master, music_gain, track_menu, track_log) = {
        let sys = &run.core.audio.as_ref().expect("sound").game().sound.sys;
        let t = sys.table();
        (
            t.stream_fade_down_time,
            sys.digital_master_gain(),
            sys.music_gain(),
            t.music[menu_tune as usize].volume,
            t.music[logistics_tune as usize].volume,
        )
    };
    // The menu's tune fades up in `ceil(F / dt)` updates after it starts (frame 1); the request comes after.
    let request = 2 + (fade / DT).ceil() as usize + 2;
    let end = request + (fade / DT).ceil() as usize + 1;
    let mut at = BTreeMap::new();
    at.insert(0, (vec![Cue::Music(menu_tune)], None));
    at.insert(
        request,
        (
            vec![Cue::Music(logistics_tune)],
            Some(StateRequest::Logistics),
        ),
    );
    run.core.ui = Box::new(Timeline {
        at,
        frame: 0,
        pending: Vec::new(),
    });
    run.core.states.request(StateRequest::MainMenu);

    let level_volume = |level: f32, track: f32| master * level * music_gain * track;
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    let dt = DT;
    for f in 0..=end {
        run.core.frame(Duration::from_secs_f32(dt));
        let (old, new) = (stream(&run, 0), stream(&run, 1));
        if f + 1 == request {
            // The menu tune is steady: time 0, full level, its own track.
            assert!(
                old.0 && old.1 == 0.0 && old.2 as i64 == menu_tune,
                "{old:?}"
            );
            assert!(close(old.3, level_volume(1.0, track_menu)), "{old:?}");
        } else if f == request {
            // The request starts nothing: the new stream is set up silent and waits for the next update.
            assert!(
                !new.0 && close(new.1, fade) && new.2 as i64 == logistics_tune,
                "{new:?}"
            );
            assert!(close(new.3, 0.0), "the new stream is silent: {new:?}");
            assert!(
                old.0 && close(old.1, -fade),
                "the old stream starts fading: {old:?}"
            );
            assert!(close(old.3, level_volume(1.0, track_menu)), "{old:?}");
        } else if f > request && f <= request + 3 {
            // The first update after the request starts the stream and fades both: `j` updates on.
            let j = (f - request) as f32;
            let left = fade - j * dt;
            assert!(new.0 && old.0, "both streams play at update {j}");
            assert!(close(old.1, -left), "old time {} at update {j}", old.1);
            assert!(close(new.1, left), "new time {} at update {j}", new.1);
            assert!(
                close(old.3, level_volume(left / fade, track_log)),
                "old stream level at update {j}: {old:?}"
            );
            assert!(
                close(new.3, level_volume((fade - left) / fade, track_log)),
                "new stream level at update {j}: {new:?}"
            );
        } else if f == end {
            // Past the fade: the old tune is released and the new one is steady at full level.
            assert!(!old.0, "the old stream is released: {old:?}");
            assert!(new.0 && new.1 == 0.0, "the new stream is steady: {new:?}");
            assert!(close(new.3, level_volume(1.0, track_log)), "{new:?}");
        }
    }
    assert_eq!(run.core.state(), &GameState::Logistics);
}
