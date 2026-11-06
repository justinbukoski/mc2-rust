// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission HUD's sounds on the live simulation, through `GameCore` (headless, never a window; skipped
//! without `MC2_DATA` or when the GPU adapter is missing and `MC2_SKIP_GPU_TESTS=1`). The live session's HUD-only audio route
//! ([`HeadlessRun::live_hud_audio`]) is mixed offline every frame, so each test reads what the HUD asked for
//! (`HudRuntime::sound_log`) and the rendered audio of the same frames. Production battle audio is deliberately
//! absent: its ambient sounds can outlive a UI action and would make a quiet-HUD frame observably non-silent.
//!
//! The sample ids are the values of `mclib/sounds.h` (read from that header by [`c_header_value`]), never the Rust
//! constants of [`mc2_audio::ids`] the HUD plays through; the referenced samples are checked in the shipped banks.

#[path = "common/mod.rs"]
mod common;

use mc2_app::audio::Cue;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::hud::HudRuntime;
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::object::Kind;
use mc2_ui::control::ButtonVisualState;
use mc2_ui::mission_hud::HitTarget;
use std::path::PathBuf;

fn data() -> Option<PathBuf> {
    // the sample ids come from the C++ header, which the published tree does not carry
    let header = concat!(env!("CARGO_MANIFEST_DIR"), "/../../mclib/sounds.h");
    if !std::path::Path::new(header).exists() {
        eprintln!("{header} not found; skipping");
        return None;
    }
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

/// A started `mc2_01` at 1920x1080 with only HUD audio mixed offline, at least `sim_frames`
/// frames in and with its opening movie dismissed through the production Escape path.
fn start(sim_frames: u64) -> Option<HeadlessRun<LiveSim>> {
    let data = data()?;
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    opts.record_audio = true;
    let mut run = match HeadlessRun::live_hud_audio(&opts) {
        Ok(r) => r,
        Err(e) if common::skip_gpu_test_without_adapter(&e) => {
            return None;
        }
        Err(e) => panic!("{e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    // `mc2_01` enters movie mode in its first script frame. Normal HUD interaction is correctly
    // gated until the player dismisses it, so use the same Escape path as the game before
    // asserting ordinary HUD sounds. `live_hud_audio` deliberately excludes video audio.
    run.step(None).unwrap();
    dismiss_opening_movie(&mut run);
    while sim_frame(&run) < sim_frames {
        run.step(None).unwrap();
    }
    // the audio of the start-up frames is not the test's
    run.audio_samples.clear();
    hud_mut(&mut run).capture_sounds = true;
    hud_mut(&mut run).sound_log.clear();
    Some(run)
}

/// End mc2_01's opening movie through `ForceMovieEnd`, rather than changing presentation state
/// directly. The app's GUI pass observes the completed end on the following frame.
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

fn sim_frame(run: &HeadlessRun<LiveSim>) -> u64 {
    run.core.mission.as_ref().unwrap().clock.frame()
}

fn hud(run: &HeadlessRun<LiveSim>) -> &HudRuntime {
    run.core
        .mission
        .as_ref()
        .unwrap()
        .hud
        .as_ref()
        .expect("the live simulation has a HUD")
}

fn hud_mut(run: &mut HeadlessRun<LiveSim>) -> &mut HudRuntime {
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .expect("the live simulation has a HUD")
}

fn world_mut(run: &mut HeadlessRun<LiveSim>) -> &mut mc2_sim::World {
    &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world
}

/// The value of a `#define NAME value` line of `mclib/sounds.h`, the C++ header the ids come from.
fn c_header_value(name: &str) -> i32 {
    let header = concat!(env!("CARGO_MANIFEST_DIR"), "/../../mclib/sounds.h");
    let text = std::fs::read_to_string(header).unwrap_or_else(|e| panic!("{header}: {e}"));
    text.lines()
        .map(str::split_whitespace)
        .filter_map(
            |mut words| match (words.next(), words.next(), words.next()) {
                (Some("#define"), Some(n), Some(v)) if n == name => Some(v.parse::<i32>().ok()?),
                _ => None,
            },
        )
        .next()
        .unwrap_or_else(|| panic!("{name} is not defined in mclib/sounds.h"))
}

/// The effect sample `name` of `mclib/sounds.h`, as the sound bite the game plays it with.
fn sample(name: &str) -> Cue {
    Cue::Sound(c_header_value(name))
}

/// The Betty line `name` of `mclib/sounds.h`, a `betty.pak` packet.
fn betty(name: &str) -> Cue {
    Cue::Betty(c_header_value(name))
}

/// The HUD sounds asked for since the last call.
fn take_sounds(run: &mut HeadlessRun<LiveSim>) -> Vec<Cue> {
    std::mem::take(&mut hud_mut(run).sound_log)
}

/// One frame: the mixed audio of that frame and the HUD's sounds for it.
fn frame(run: &mut HeadlessRun<LiveSim>) -> (Vec<f32>, Vec<Cue>) {
    let before = run.audio_samples.len();
    run.step(None).unwrap();
    let samples = run.audio_samples[before..].to_vec();
    (samples, take_sounds(run))
}

/// Whether the mixed audio of a frame has any sound in it.
fn audible(samples: &[f32]) -> bool {
    samples.iter().any(|v| v.abs() > 1e-3)
}

fn mouse(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    run.event(InputEvent::CursorMoved { x, y });
}

fn button(run: &mut HeadlessRun<LiveSim>, pressed: bool) {
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed,
    });
}

/// The `ControlGui` ids of the buttons the tests press (`code/controlgui.h`).
const STOP: i32 = 8;
const OBJECTIVES: i32 = 10;
const TACMAP_TAB: i32 = 12;

fn control_button(run: &HeadlessRun<LiveSim>, cxx_id: i32) -> mc2_ui::control::ControlButton {
    hud(run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.cxx_id == cxx_id)
        .copied()
        .unwrap_or_else(|| panic!("the control panel has button {cxx_id}"))
}

/// The centre of a control button, in game pixels.
fn control_centre(run: &HeadlessRun<LiveSim>, cxx_id: i32) -> (f64, f64) {
    let b = control_button(run, cxx_id);
    let (x, y) = hud(run).hud_to_game((
        (b.rect.left + b.rect.right) / 2,
        (b.rect.top + b.rect.bottom) / 2,
    ));
    (f64::from(x), f64::from(y))
}

fn state_of(run: &HeadlessRun<LiveSim>, cxx_id: i32) -> ButtonVisualState {
    control_button(run, cxx_id).state
}

/// A left click on a point, as the window delivers it: the pointer moves, then the button is pressed and released,
/// each on its own frame. Returns the frames of the press and of the release: (audio, HUD sounds).
type Frame = (Vec<f32>, Vec<Cue>);
fn click_at(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) -> (Frame, Frame) {
    mouse(run, x, y);
    run.step(None).unwrap();
    take_sounds(run);
    // the move requested the highlight; let it end, so the release's audio is the click's alone
    until_quiet(run);
    button(run, true);
    let press = frame(run);
    button(run, false);
    let release = frame(run);
    (press, release)
}

/// Frames until the mixed audio has been silent for three frames in a row (a bound stops a runaway sound), so that
/// a sound already playing is over before the next input is made.
fn until_quiet(run: &mut HeadlessRun<LiveSim>) {
    let mut quiet = 0;
    for _ in 0..600 {
        let (audio, cues) = frame(run);
        assert!(cues.is_empty(), "a frame with no input asked for {cues:?}");
        quiet = if audible(&audio) { 0 } else { quiet + 1 };
        if quiet == 3 {
            return;
        }
    }
    panic!("the sounds of the last input did not end within 600 frames");
}

fn selected(run: &HeadlessRun<LiveSim>) -> Vec<i32> {
    let mut s = hud(run).selection.selected.clone();
    s.sort_unstable();
    s
}

/// The home commander's movers on the GUI, straight from the simulation.
fn lance(run: &HeadlessRun<LiveSim>) -> Vec<i32> {
    let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
    let mut parts: Vec<i32> = w
        .objects
        .movers
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .filter(|o| o.exists())
        .filter(|o| match &o.kind {
            Kind::Mech(m) => i32::from(m.m.commander.0) == HOME_COMMANDER && m.m.flags.on_gui,
            Kind::Vehicle(v) => i32::from(v.m.commander.0) == HOME_COMMANDER && v.m.flags.on_gui,
            _ => false,
        })
        .map(|o| o.c.part_id.0)
        .collect();
    parts.sort_unstable();
    parts
}

/// The select-visible hot key (`E`), as the player presses it.
fn select_lance(run: &mut HeadlessRun<LiveSim>) -> Vec<i32> {
    let code = script::key_code("E").expect("key name");
    for e in script::key_events(code, true) {
        run.event(e);
    }
    run.step(None).unwrap();
    for e in script::key_events(code, false) {
        run.event(e);
    }
    run.step(None).unwrap();
    let got = selected(run);
    assert_eq!(got, lance(run), "the select-visible key selected the lance");
    got
}

#[test]
fn a_control_panel_click_plays_its_click_sample_and_is_audible() {
    let Some(mut run) = start(130) else { return };
    // The objectives button plays `LOG_CLICKONBUTTON` (`code/controlgui.cpp:1621`).
    assert_eq!(
        state_of(&run, OBJECTIVES),
        ButtonVisualState::Enabled,
        "the objectives button is enabled at mission start"
    );
    let (x, y) = control_centre(&run, OBJECTIVES);
    let ((press, press_cues), (release, cues)) = click_at(&mut run, x, y);
    assert!(press_cues.is_empty(), "the press asked for {press_cues:?}");
    assert!(
        !audible(&press),
        "the press is silent: the click is on the release"
    );
    assert_eq!(cues, vec![sample("LOG_CLICKONBUTTON")]);
    assert!(audible(&release), "the click is heard in its frame");
}

#[test]
fn a_tab_click_plays_the_select_sample() {
    let Some(mut run) = start(130) else { return };
    // Every other control button's click plays `LOG_SELECT` (`code/controlgui.cpp:1552`).
    assert_eq!(
        state_of(&run, TACMAP_TAB),
        ButtonVisualState::Enabled,
        "the tactical map tab is enabled"
    );
    let (x, y) = control_centre(&run, TACMAP_TAB);
    let (_, (release, cues)) = click_at(&mut run, x, y);
    assert_eq!(cues, vec![sample("LOG_SELECT")]);
    assert!(audible(&release));
}

#[test]
fn a_disabled_control_button_plays_invalid_gui_and_does_nothing_else() {
    let Some(mut run) = start(130) else { return };
    // With nothing selected the mech-only buttons are disabled (`ControlGui::handleClick`, `code/controlgui.cpp:1543`).
    // Which ones depends on the mission's loadout, so the test takes the first disabled button it finds.
    let disabled = hud(&run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.state == ButtonVisualState::Disabled)
        .copied()
        .expect("with nothing selected some control button is disabled");
    let (x, y) = hud(&run).hud_to_game((
        (disabled.rect.left + disabled.rect.right) / 2,
        (disabled.rect.top + disabled.rect.bottom) / 2,
    ));
    let (_, (release, cues)) = click_at(&mut run, f64::from(x), f64::from(y));
    assert_eq!(
        cues,
        vec![sample("INVALID_GUI")],
        "button {}",
        disabled.cxx_id
    );
    assert!(audible(&release));
}

#[test]
fn stop_plays_the_button_sample_then_the_select_sample() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    assert_eq!(
        state_of(&run, STOP),
        ButtonVisualState::Enabled,
        "stop is enabled with the lance selected"
    );
    let (x, y) = control_centre(&run, STOP);
    let (_, (release, cues)) = click_at(&mut run, x, y);
    // `doStop` plays BUTTON5 (`code/controlgui.cpp:1673`); then `handleClick` plays LOG_SELECT.
    assert_eq!(cues, vec![sample("BUTTON5"), sample("LOG_SELECT")]);
    assert!(audible(&release));
}

#[test]
fn a_unit_bar_click_selects_and_plays_the_select_sample() {
    let Some(mut run) = start(130) else { return };
    assert!(selected(&run).is_empty(), "nothing starts selected");
    // The unit bar's icons select on the press (`ForceGroupIcon::click`, `code/mechicon.cpp:427`). Probe the bar
    // until a press selects a unit, then check that the press made the select sound.
    let probes: Vec<(i32, i32)> = (0..1080 / 12)
        .flat_map(|r| (0..1920 / 12).map(move |c| (c * 12, r * 12)))
        .filter(|p| hud(&run).hit_test(*p) == HitTarget::UnitBar)
        .collect();
    assert!(!probes.is_empty(), "the unit bar is on screen");
    let mut found = None;
    for (x, y) in probes {
        let ((press, press_cues), (_, release_cues)) =
            click_at(&mut run, f64::from(x), f64::from(y));
        if !selected(&run).is_empty() {
            found = Some((press, press_cues, release_cues));
            break;
        }
    }
    let (press, press_cues, release_cues) = found.expect("a unit bar icon selects a unit");
    assert_eq!(press_cues, vec![sample("LOG_SELECT")]);
    assert!(release_cues.is_empty(), "the release makes no sound");
    assert!(audible(&press), "the select sound is heard in its frame");
}

#[test]
fn a_world_click_that_selects_a_mech_makes_no_hud_sound() {
    let Some(mut run) = start(130) else { return };
    // A world click selects a friendly mover with `setSelected` alone (`code/missiongui.cpp:1440`, no sample);
    // only the unit bar's icons play `LOG_SELECT`.
    let part = lance(&run)[0];
    let (x, y) = screen_of(&run, part);
    let ((_, press_cues), (_, release_cues)) = click_at(&mut run, x, y);
    assert!(
        selected(&run).contains(&part),
        "the world click selected the mech"
    );
    assert!(press_cues.is_empty(), "the press asked for {press_cues:?}");
    assert!(
        release_cues.is_empty(),
        "the release asked for {release_cues:?}"
    );
}

#[test]
fn a_quiet_frame_makes_no_hud_sound() {
    let Some(mut run) = start(130) else { return };
    // The pointer parked away from the panel and nothing pressed: no HUD sound, no audio.
    mouse(&mut run, 960.0, 540.0);
    run.step(None).unwrap();
    take_sounds(&mut run);
    for _ in 0..5 {
        let (audio, cues) = frame(&mut run);
        assert!(cues.is_empty(), "a quiet frame asked for {cues:?}");
        assert!(!audible(&audio), "a quiet frame is silent");
    }
}

#[test]
fn the_mission_timer_warns_two_minutes_then_thirty_seconds_once_each() {
    let Some(mut run) = start(130) else { return };
    // 100 s of the limit left: the two-minute line (`code/controlgui.cpp:383-391`, an `else if`).
    let now = world_mut(&mut run).mission.actual_time;
    world_mut(&mut run).mission.time_limit = now + 100.0;
    let (audio, cues) = frame(&mut run);
    assert_eq!(cues, vec![betty("BETTY_TWO_MIN_LEFT")]);
    assert!(audible(&audio), "the two-minute line is heard in its frame");
    // Betty speaks the whole line before the next warning can start: a request while she is still
    // speaking is dropped by the mixer (`playBettySample`), so the thirty-second line waits for silence.
    let mut quiet = 0;
    for _ in 0..1200 {
        let (audio, cues) = frame(&mut run);
        assert!(cues.is_empty(), "a warning is played once: {cues:?}");
        quiet = if audible(&audio) { 0 } else { quiet + 1 };
        if quiet == 10 {
            break;
        }
    }
    assert_eq!(quiet, 10, "the two-minute line ends");
    // Now 25 s are left: the thirty-second line, heard in the frame it is asked for.
    let now = world_mut(&mut run).mission.actual_time;
    world_mut(&mut run).mission.time_limit = now + 25.0;
    let (audio, cues) = frame(&mut run);
    assert_eq!(cues, vec![betty("BETTY_THIRTY_LEFT")]);
    assert!(
        audible(&audio),
        "the thirty-second line is heard in its frame"
    );
    for _ in 0..5 {
        let (_, cues) = frame(&mut run);
        assert!(cues.is_empty(), "a warning is played once: {cues:?}");
    }
}

/// The game-pixel centre of a unit on the screen.
fn screen_of(run: &HeadlessRun<LiveSim>, part: i32) -> (f64, f64) {
    let rt = run.core.mission.as_ref().unwrap();
    let u = hud(run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.part_id == part)
        .expect("the unit is on the GUI");
    let (s, _) = rt.rig.view().project_z(mc2_render::math::Vec3::new(
        u.position.0,
        u.position.1,
        u.position.2,
    ));
    (f64::from(s[0]), f64::from(s[1]))
}

/// `frames` frames with no input, so that the sounds the last input made have ended.
fn settle(run: &mut HeadlessRun<LiveSim>, frames: usize) {
    for _ in 0..frames {
        frame(run);
    }
}

/// The world point the gesture tests start from: a point of the game view that is not on the panel.
const WORLD_PX: (i32, i32) = (960, 540);

#[test]
fn a_press_on_the_world_and_a_release_on_a_button_is_no_click() {
    let Some(mut run) = start(130) else { return };
    assert_eq!(hud(&run).hit_test(WORLD_PX), HitTarget::World);
    let (x, y) = control_centre(&run, OBJECTIVES);
    // pressed in the world, the pointer moves onto the objectives button, and the button is released there
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    run.step(None).unwrap();
    take_sounds(&mut run);
    button(&mut run, true);
    frame(&mut run);
    mouse(&mut run, x, y);
    settle(&mut run, 30);
    button(&mut run, false);
    let (audio, cues) = frame(&mut run);
    assert!(cues.is_empty(), "the release asked for {cues:?}");
    assert!(!audible(&audio), "no click is heard");
}

#[test]
fn a_press_on_a_button_and_a_release_elsewhere_is_no_click() {
    let Some(mut run) = start(130) else { return };
    let (x, y) = control_centre(&run, OBJECTIVES);
    // pressed on the objectives button, dragged off it, and released in the world
    mouse(&mut run, x, y);
    run.step(None).unwrap();
    take_sounds(&mut run);
    button(&mut run, true);
    frame(&mut run);
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    settle(&mut run, 30);
    button(&mut run, false);
    let (audio, cues) = frame(&mut run);
    assert!(cues.is_empty(), "the release asked for {cues:?}");
    assert!(!audible(&audio), "no click is heard");
}

#[test]
fn a_press_on_a_button_dragged_away_and_back_in_one_frame_is_no_click() {
    let Some(mut run) = start(130) else { return };
    let (x, y) = control_centre(&run, OBJECTIVES);
    mouse(&mut run, x, y);
    run.step(None).unwrap();
    take_sounds(&mut run);
    let before = hud(&run).last_output().objectives_visible;
    button(&mut run, true);
    frame(&mut run);
    // the pointer goes far away and comes back onto the button, and the button is released there, all in one frame:
    // the UI router latches the drag on the way out, so the release cancels the action and makes no click
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    mouse(&mut run, x, y);
    button(&mut run, false);
    let (audio, cues) = frame(&mut run);
    assert!(
        !cues.contains(&sample("LOG_CLICKONBUTTON")),
        "a dragged press asked for a click: {cues:?}"
    );
    assert!(cues.is_empty(), "the release asked for {cues:?}");
    assert!(!audible(&audio), "no click is heard");
    assert_eq!(
        hud(&run).last_output().objectives_visible,
        before,
        "the drag cancelled the objectives action"
    );
    // the same button, clicked without the drag, does run the action: the check above is not vacuous
    settle(&mut run, 2);
    let (_, (_, cues)) = click_at(&mut run, x, y);
    assert_eq!(cues, vec![sample("LOG_CLICKONBUTTON")]);
    assert_ne!(
        hud(&run).last_output().objectives_visible,
        before,
        "a plain click runs the objectives action"
    );
}

#[test]
fn a_pointer_entering_a_button_plays_the_highlight_once() {
    let Some(mut run) = start(130) else { return };
    let (x, y) = control_centre(&run, OBJECTIVES);
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    run.step(None).unwrap();
    take_sounds(&mut run);
    mouse(&mut run, x, y);
    let (_, cues) = frame(&mut run);
    assert_eq!(cues, vec![sample("LOG_HIGHLIGHTBUTTONS")]);
    let (_, cues) = frame(&mut run);
    assert!(
        cues.is_empty(),
        "the highlight is played on entering only: {cues:?}"
    );
}

/// The shipped banks hold a sample for every id the HUD plays, so each one is a real sound, not a silent packet.
#[test]
fn the_referenced_samples_are_in_the_shipped_banks() {
    let Some(root) = data() else { return };
    let vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("vfs");
    let mut audio = mc2_audio::data::VfsData::new(vfs, false);
    let bank = mc2_audio::bank::SoundBank::load(&mut audio, "sound").expect("the sound banks");
    for name in [
        "LOG_SELECT",
        "LOG_CLICKONBUTTON",
        "LOG_HIGHLIGHTBUTTONS",
        "BUTTON5",
        "INVALID_GUI",
    ] {
        let id = c_header_value(name) as usize;
        assert!(
            bank.effects.sound(id).is_some(),
            "{name} ({id}) is in sound.pak"
        );
    }
    for name in ["BETTY_TWO_MIN_LEFT", "BETTY_THIRTY_LEFT"] {
        let id = c_header_value(name) as usize;
        assert!(
            bank.betty.sound(id).is_some(),
            "{name} ({id}) is in betty.pak"
        );
    }
}

#[test]
fn a_press_in_the_world_and_a_move_and_release_onto_a_button_in_one_frame_is_no_click() {
    let Some(mut run) = start(130) else { return };
    let (x, y) = control_centre(&run, OBJECTIVES);
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    run.step(None).unwrap();
    take_sounds(&mut run);
    button(&mut run, true);
    frame(&mut run);
    // the move and the release arrive in the same frame
    mouse(&mut run, x, y);
    button(&mut run, false);
    let (_, cues) = frame(&mut run);
    // the pointer entering the button is a highlight; the release makes no click
    assert_eq!(cues, vec![sample("LOG_HIGHLIGHTBUTTONS")]);
}

/// The `ControlGui` id of the vehicle tab button (`code/controlgui.h`).
const VEHICLE_TAB: i32 = 14;

/// One press and release of a hot key, as the player makes it; the frame after it, with the keys up.
fn tap(run: &mut HeadlessRun<LiveSim>, name: &str) {
    let code = script::key_code(name).expect("key name");
    for e in script::key_events(code, true) {
        run.event(e);
    }
    run.step(None).unwrap();
    for e in script::key_events(code, false) {
        run.event(e);
    }
    run.step(None).unwrap();
    take_sounds(run);
}

/// The game-pixel centre of the first support-vehicle button that is enabled (or ambiguous) and so clickable.
fn enabled_vehicle_centre(run: &HeadlessRun<LiveSim>) -> (f64, f64) {
    let v = hud(run)
        .control_panel()
        .vehicles
        .iter()
        .find(|v| {
            matches!(
                v.state,
                ButtonVisualState::Enabled | ButtonVisualState::Ambiguous
            )
        })
        .expect("a support vehicle is enabled on the vehicle tab");
    let (x, y) = hud(run).hud_to_game((
        (v.rect.left + v.rect.right) / 2,
        (v.rect.top + v.rect.bottom) / 2,
    ));
    (f64::from(x), f64::from(y))
}

#[test]
fn a_tab_key_shows_the_vehicle_buttons_and_their_sounds_follow_the_shown_tab() {
    let Some(mut run) = start(130) else { return };
    assert_eq!(hud(&run).tab(), mc2_ui::mission_hud::MissionTab::TacMap);
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    run.step(None).unwrap();
    take_sounds(&mut run);

    // Tab from the tactical map shows the vehicle tab (`switchTabs`, controlgui.cpp:2824-2844) without a click on
    // the tab button; the tab key presses the vehicle tab button, so the control panel's `vehicle_tab` follows it
    tap(&mut run, "Tab");
    assert_eq!(hud(&run).tab(), mc2_ui::mission_hud::MissionTab::Vehicle);
    assert!(hud(&run).control_panel().vehicle_tab);
    let (x, y) = enabled_vehicle_centre(&run);

    // the pointer entering a shown vehicle button plays the highlight, once
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    frame(&mut run);
    mouse(&mut run, x, y);
    let (_, cues) = frame(&mut run);
    assert_eq!(cues, vec![sample("LOG_HIGHLIGHTBUTTONS")]);

    // a click on it plays `LOG_SELECT` on the release (`handleVehicleClick`, controlgui.cpp:1991), and is heard
    let (press, release) = click_at(&mut run, x, y);
    assert!(press.1.is_empty(), "the press asked for {:?}", press.1);
    assert_eq!(release.1, vec![sample("LOG_SELECT")]);
    assert!(
        audible(&release.0),
        "the vehicle click is heard in its frame"
    );

    // Tab again shows the infantry tab (`INFO_TAB`): the vehicle buttons are no longer drawn, so they make no sound
    tap(&mut run, "Tab");
    assert_eq!(hud(&run).tab(), mc2_ui::mission_hud::MissionTab::Info);
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    run.step(None).unwrap();
    take_sounds(&mut run);
    mouse(&mut run, x, y);
    let (_, cues) = frame(&mut run);
    assert!(
        cues.is_empty(),
        "a hidden vehicle button asked for {cues:?}"
    );
    let (press, release) = click_at(&mut run, x, y);
    assert!(press.1.is_empty() && release.1.is_empty());
    assert!(!audible(&release.0), "no click is heard on a hidden button");
}

#[test]
fn a_vehicle_tab_click_then_a_tab_key_hides_the_vehicle_buttons_and_their_sounds() {
    let Some(mut run) = start(130) else { return };
    // the tab button is clicked: the vehicle tab is shown and the control panel's `vehicle_tab` is set
    let (x, y) = control_centre(&run, VEHICLE_TAB);
    let (_, (_, cues)) = click_at(&mut run, x, y);
    assert_eq!(cues, vec![sample("LOG_SELECT")]);
    assert_eq!(hud(&run).tab(), mc2_ui::mission_hud::MissionTab::Vehicle);
    assert!(hud(&run).control_panel().vehicle_tab);

    // Tab moves on to the infantry tab; the vehicle buttons are not drawn, so their hover makes no sound
    tap(&mut run, "Tab");
    assert_eq!(hud(&run).tab(), mc2_ui::mission_hud::MissionTab::Info);
    let (x, y) = enabled_vehicle_centre(&run);
    mouse(&mut run, f64::from(WORLD_PX.0), f64::from(WORLD_PX.1));
    run.step(None).unwrap();
    take_sounds(&mut run);
    mouse(&mut run, x, y);
    let (_, cues) = frame(&mut run);
    assert!(
        cues.is_empty(),
        "a hidden vehicle button asked for {cues:?}"
    );
}

/// A Tab-key vehicle click: the buttons are drawn, the click plays `LOG_SELECT`, and the input router makes the
/// vehicle intent (the router tests `ControlPanel::vehicle_tab`, which the tab key now presses as the drawn tab does).
#[test]
fn a_tab_key_vehicle_click_makes_its_action_or_says_unavailable() {
    let Some(mut run) = start(130) else { return };
    tap(&mut run, "Tab");
    assert_eq!(hud(&run).tab(), mc2_ui::mission_hud::MissionTab::Vehicle);
    let (x, y) = enabled_vehicle_centre(&run);
    let (_, (_, cues)) = click_at(&mut run, x, y);
    assert_eq!(cues, vec![sample("LOG_SELECT")]);
    let intents = &hud(&run).last_output().intents;
    assert!(
        intents.iter().any(|i| matches!(
            i,
            mc2_ui::mission_hud::HudIntent::Control(
                mc2_ui::control::ControlIntent::BuildVehicle(_)
                    | mc2_ui::control::ControlIntent::CancelVehicle { .. }
                    | mc2_ui::control::ControlIntent::Stop
            )
        )),
        "the vehicle click made no vehicle intent: {intents:?}"
    );
}
