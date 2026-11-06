// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! The production UI host (`mc2_app::ui_host::UiHost`) driven through `GameCore` with the real game
//! data: main menu -> new campaign -> mission selection -> briefing -> mech bay -> pilot ready ->
//! loading -> mission, by pointer events at 1024x768.  Headless only; skipped when `MC2_DATA` is
//! unset or when the GPU adapter is missing and `MC2_SKIP_GPU_TESTS=1`.

use mc2_app::campaign_save::{CampaignSaveService, SaveSlot};
use mc2_app::game::DriverFactory;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{UiFrame, UiLayer};
use mc2_app::sim::{FrameInput, FrameOutput, NullSim, SimDriver};
use mc2_app::state::GameState;
use mc2_app::ui_host::{ActiveScreen, UiCue, UiCueSink, UiHost};
use mc2_campaign::interface::{LoadType, Outcome};
use mc2_formats::vfs::Vfs;
use mc2_platform::audio::{Mixer, SharedMixer, APP_MIXER_CHANNELS};
use mc2_platform::input::{InputEvent, InputState, MouseButton};
use mc2_platform::keys;
use mc2_platform::movie::Movie;
use mc2_platform::viewport::{Fit, Viewport};
use mc2_render::mission::PaintSource;
use mc2_ui::logistics::ids;
use mc2_ui::logistics::mech_bay::MechBay;
use mc2_ui::logistics::movie::MovieRequest;
use mc2_ui::logistics::sequence::slot;
use mc2_ui::screens::dialog::NO;
use mc2_ui::screens::main_menu::{
    MM_MSG_ENCYCLOPEDIA, MM_MSG_LOAD, MM_MSG_NEW_CAMPAIGN, MM_MSG_OPTIONS, MM_MSG_SAVE,
};
use std::cell::RefCell;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::rc::Rc;

const SIZE: (u32, u32) = (1024, 768);
/// Game pixels per UI pixel (the 800x600 layouts fitted to 1024x768).
const SCALE: f32 = 1.28;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

/// A headless run for direct mission launches (skipped when MC2_DATA is unset).
fn viewer(opts: &HeadlessOptions) -> Option<HeadlessRun<NullSim>> {
    match HeadlessRun::viewer(opts) {
        Ok(r) => Some(r),
        Err(e)
            if e.contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

type Cues = Rc<RefCell<Vec<UiCue>>>;

struct Log(Cues);

impl UiCueSink for Log {
    fn cues(&mut self, cues: &[UiCue]) {
        self.0.borrow_mut().extend_from_slice(cues);
    }
}

#[derive(Default)]
struct WinningNull {
    frames: u64,
}

impl SimDriver for WinningNull {
    type Command = ();
    type Event = ();
    type World = ();

    fn step(&mut self, _input: FrameInput<()>) -> FrameOutput<()> {
        self.frames += 1;
        FrameOutput {
            events: Vec::new(),
            result: if self.frames >= 8 {
                mc2_sim::mission::result::WIN_BIG
            } else {
                mc2_sim::mission::result::PLAYING
            },
        }
    }

    fn world(&self) -> &() {
        &()
    }

    fn mission_outcome(&mut self) -> Option<Outcome> {
        Some(Outcome::won())
    }
}

/// A session at the main menu (no mission on the command line) with a cue log.
/// A config directory of this session's own: an accepted options edit writes `options.cfg` there, never to the real
/// per-user directory.
fn fresh_config_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "mc2-ui-host-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Points the user directories (the saved `options.cfg`, saves) at a per-run directory under
/// `CARGO_TARGET_TMPDIR`, so a test never reads or writes the player's real configuration.
fn isolate_user_dir() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("ui-host-user-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var(mc2_platform::paths::USER_DIR_ENV, dir);
    });
}

fn session() -> Option<(HeadlessRun<NullSim>, Cues)> {
    session_with(Box::new(|_| Ok(NullSim::default())))
}

fn session_with(factory: DriverFactory<NullSim>) -> Option<(HeadlessRun<NullSim>, Cues)> {
    session_in(data()?, factory)
}

fn winning_session() -> Option<(HeadlessRun<WinningNull>, Cues)> {
    isolate_user_dir();
    let mut opts = HeadlessOptions::new(data()?, "");
    opts.size = SIZE;
    match HeadlessRun::with_driver(&opts, Box::new(|_| Ok(WinningNull::default()))) {
        Ok(mut run) => {
            let cues = Cues::default();
            host(&mut run).set_cue_sink(Box::new(Log(cues.clone())));
            host(&mut run).set_config_directory(fresh_config_dir());
            Some((run, cues))
        }
        Err(e) if skips_without_adapter(&e, std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()) => {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn campaign_before_final_win(
    data: std::sync::Arc<mc2_campaign::GameData>,
) -> mc2_campaign::Logistics {
    let mut campaign = mc2_campaign::Logistics::new(std::sync::Arc::clone(&data))
        .expect("initialize campaign for final-win fixture");
    // Advance through real campaign outcomes until the next win would finish the campaign.
    // Leave the final mission active so GameCore's next mission result must create the transition.
    for _ in 0..64 {
        assert!(!campaign.campaign_over(), "fixture stops before final win");
        let before = campaign.to_save();
        let mut probe = mc2_campaign::Logistics::from_save(std::sync::Arc::clone(&data), &before)
            .expect("clone campaign for final-mission probe")
            .logistics;
        let effect = probe
            .apply_outcome(&Outcome::won())
            .expect("probe campaign win");
        if effect.campaign_over {
            assert!(!campaign.current_mission().is_empty());
            return campaign;
        }
        campaign
            .apply_outcome(&Outcome::won())
            .expect("advance campaign to final mission");
    }
    panic!("campaign did not reach a final mission within the fixture budget")
}

/// A headless setup error skips the test only for a missing adapter, and only when
/// `MC2_SKIP_GPU_TESTS` is exactly `1` (unset, empty, `0`, `true` and `11` do not skip).
fn skips_without_adapter(err: &str, var: Option<&std::ffi::OsStr>) -> bool {
    err.contains("adapter") && var.is_some_and(|v| v == "1")
}

fn session_in(
    data: PathBuf,
    factory: DriverFactory<NullSim>,
) -> Option<(HeadlessRun<NullSim>, Cues)> {
    session_at_size(data, factory, SIZE)
}

fn session_at_size(
    data: PathBuf,
    factory: DriverFactory<NullSim>,
    size: (u32, u32),
) -> Option<(HeadlessRun<NullSim>, Cues)> {
    isolate_user_dir();
    let mut opts = HeadlessOptions::new(data, "");
    opts.size = size;
    match HeadlessRun::with_driver(&opts, factory) {
        Ok(mut r) => {
            let cues = Cues::default();
            host(&mut r).set_cue_sink(Box::new(Log(cues.clone())));
            host(&mut r).set_config_directory(fresh_config_dir());
            Some((r, cues))
        }
        Err(e) if skips_without_adapter(&e, std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()) => {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn host<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) -> &mut UiHost {
    run.core
        .ui_host_mut()
        .expect("the production host is installed")
}

fn frames<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, n: usize) {
    for _ in 0..n {
        run.step(None).unwrap();
    }
}

fn type_text<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, value: &str) {
    for ch in value.chars() {
        let key = ch.to_ascii_uppercase() as u32;
        run.event(InputEvent::Key {
            key,
            phys_id: key as u16,
            pressed: true,
            repeat: false,
            text: Some(ch.to_string()),
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
}

/// Starts the pending logistics cutscene and skips it without opening a display or audio device.
fn skip_logistics_movie<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) {
    frames(run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
    frames(run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
    frames(run, 1);
}

/// Sends the two Escape presses C++ uses to stop the menu movie and dismiss its splash.
fn skip_menu_intro<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) {
    for _ in 0..2 {
        run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
        frames(run, 1);
        run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
        frames(run, 1);
    }
}

#[test]
fn production_host_intro_advances_mixes_audio_stays_synced_and_skips() {
    let Some(root) = data() else {
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).expect("open movie VFS");
    let mut probe = Movie::open_vfs(&mut vfs, "data/movies/msft.mpg").expect("open intro");
    let first_video_pts = probe.next_video_frame().unwrap().time;
    let first_audio = probe.next_audio_chunk().unwrap();
    let first_audio_pts = first_audio.time;
    let relative_audio_start = first_audio_pts - first_video_pts;
    assert!(
        relative_audio_start.abs() <= 0.040,
        "intro PTS skew {relative_audio_start:.3}s"
    );
    let mut reference_audio = first_audio.samples.clone();
    while let Some(chunk) = probe.next_audio_chunk() {
        reference_audio.extend_from_slice(&chunk.samples);
    }

    // Drive the production UiHost with the same shared mixer channel GameCore supplies. Rendering
    // the mixer is headless and never opens a window or audio device.
    let mut host = UiHost::new();
    host.set_config_directory(fresh_config_dir());
    let mixer = SharedMixer::new(Mixer::new(
        probe.info().audio_sample_rate,
        APP_MIXER_CHANNELS,
    ));
    host.set_movie_mixer(Some(mixer.clone()));
    let state = GameState::MainMenu;
    let input = InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Letterbox));
    let mut rendered_frames = 0usize;
    let mut distinct_video_pts = Vec::new();
    let mut mixed_samples = Vec::new();
    let dt = 1.0 / 30.0;
    let playback_seconds = 12.0;
    let steps = (playback_seconds * 30.0) as usize;
    for _ in 0..steps {
        let mut frame = UiFrame {
            state: &state,
            input: &input,
            dt: dt as f32,
            game_size: (800.0, 600.0),
            camera: None,
            paused: false,
            vfs: &mut vfs,
        };
        host.update(&mut frame);
        let mut output = vec![0.0; (probe.info().audio_sample_rate as usize / 30) * 2];
        mixer.lock().render(&mut output);
        let mixed = output.len() / 2;
        rendered_frames += mixed;
        mixed_samples.extend_from_slice(&output);
        if let Some(progress) = host.movie_progress() {
            assert!(
                progress.queued_audio_frames > 0,
                "intro PCM did not reach the app mixer"
            );
            if let Some(video_pts) = progress.displayed_frame_time {
                if distinct_video_pts.last().copied() != Some(video_pts) {
                    distinct_video_pts.push(video_pts);
                }
            }
        }
    }
    let progress = host.movie_progress().expect("intro should still play");
    let expected_frames = progress.clock * probe.info().frame_rate;
    assert!(
        (distinct_video_pts.len() as f64 - expected_frames).abs() <= 2.0,
        "{} displayed frames at {} fps over {:.2}s",
        distinct_video_pts.len(),
        probe.info().frame_rate,
        progress.clock
    );
    assert!(rendered_frames >= probe.info().audio_sample_rate as usize * 11);
    // Match identifiable decoded PCM at multiple points through the signal. This measures when
    // the production host's mixer actually emitted the movie samples, rather than comparing two
    // test-driven clocks that can advance while the mixer is silent.
    let rate = probe.info().audio_sample_rate as usize;
    fn stereo(samples: &[f32], frame: usize) -> &[f32] {
        &samples[frame * 2..(frame + 512) * 2]
    }
    let expected_start = (relative_audio_start * rate as f64).round() as isize;
    let mut checked = 0;
    for second in [1usize, 3, 6, 9] {
        let nominal = second * rate;
        let Some(source_start) = (nominal..nominal + rate / 2).step_by(512).find(|&at| {
            at + 512 < reference_audio.len() / 2
                && stereo(&reference_audio, at)
                    .iter()
                    .map(|s| s * s)
                    .sum::<f32>()
                    / 1024.0
                    > 1.0e-6
        }) else {
            continue;
        };
        let expected = expected_start + source_start as isize;
        let radius = (rate as f64 * 0.040).ceil() as isize;
        let mut best = (f64::NEG_INFINITY, 0isize);
        for actual in (expected - radius).max(0)..=(expected + radius).max(0) {
            let actual = actual as usize;
            if (actual + 512) * 2 > mixed_samples.len() {
                continue;
            }
            let reference = stereo(&reference_audio, source_start);
            let emitted = stereo(&mixed_samples, actual);
            let dot = reference
                .iter()
                .zip(emitted)
                .map(|(a, b)| f64::from(*a) * f64::from(*b))
                .sum::<f64>();
            let aa = reference.iter().map(|a| f64::from(*a).powi(2)).sum::<f64>();
            let bb = emitted.iter().map(|b| f64::from(*b).powi(2)).sum::<f64>();
            let correlation = dot / (aa * bb).sqrt().max(1.0e-20);
            let offset = actual as isize - expected;
            if correlation > best.0 {
                best = (correlation, offset);
            }
        }
        assert!(
            best.0 > 0.95,
            "PCM at {second}s was not identifiable in production mix (corr {})",
            best.0
        );
        assert!(
            best.1.abs() <= radius,
            "PCM at {second}s emerged {} samples from PTS (limit {radius})",
            best.1
        );
        checked += 1;
    }
    assert!(
        checked >= 3,
        "only {checked} identifiable PCM windows were checked"
    );

    host.handle_event(
        &state,
        &mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None),
    );
    assert!(
        host.movie_progress().is_none(),
        "Escape must skip the intro"
    );
}

#[test]
fn mission_select_movie_controls_play_pause_and_stop_through_the_production_host() {
    let Some((mut run, _)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let new_campaign = host(&mut run)
        .menu_button(MM_MSG_NEW_CAMPAIGN)
        .expect("new campaign button");
    click(&mut run, new_campaign);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "mission selection", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    frames(&mut run, 3);
    assert_eq!(
        screen(&mut run),
        ActiveScreen::Logistics(slot::SELECT),
        "skipping the pre-mission movie keeps mission selection active"
    );
    host(&mut run).request_movie_for_test(MovieRequest::Open("STANDIN".to_string()));
    frames(&mut run, 1);
    assert!(host(&mut run).selection_movie_clock().is_some());
    let play = host(&mut run)
        .page_button(slot::SELECT, ids::MN_MSG_PLAY)
        .expect("mission-selection Play button");
    click(&mut run, play);
    let initial = host(&mut run).selection_movie_clock().unwrap_or_else(|| {
        panic!(
            "Play did not open the movie: {:?}",
            host(&mut run).notices()
        )
    });

    let pause = host(&mut run)
        .page_button(slot::SELECT, ids::MN_MSG_PAUSE)
        .expect("mission-selection Pause button");
    click(&mut run, pause);
    let held = host(&mut run)
        .selection_movie_clock()
        .expect("Pause keeps the movie open");
    frames(&mut run, 6);
    assert_eq!(host(&mut run).selection_movie_clock(), Some(held));

    let play = host(&mut run)
        .page_button(slot::SELECT, ids::MN_MSG_PLAY)
        .expect("mission-selection Play button");
    click(&mut run, play);
    frames(&mut run, 4);
    let resumed = host(&mut run)
        .selection_movie_clock()
        .expect("Play opens the production movie");
    assert!(held > initial && resumed < held, "Play restarts the movie");

    let stop = host(&mut run)
        .page_button(slot::SELECT, ids::MN_MSG_STOP)
        .expect("mission-selection Stop button");
    click(&mut run, stop);
    assert_eq!(host(&mut run).selection_movie_clock(), None);
    let play = host(&mut run)
        .page_button(slot::SELECT, ids::MN_MSG_PLAY)
        .expect("Play button after Stop");
    click(&mut run, play);
    assert!(host(&mut run).selection_movie_clock().is_some());
}

#[test]
fn mission_select_movie_pixels_stay_inside_vidcom_at_multiple_resolutions() {
    let Some(root) = data() else {
        return;
    };
    for size in [(1024, 768), (1920, 1080)] {
        let Some((mut run, _)) =
            session_at_size(root.clone(), Box::new(|_| Ok(NullSim::default())), size)
        else {
            return;
        };
        frames(&mut run, 2);
        skip_menu_intro(&mut run);
        frames(&mut run, 90);
        host(&mut run).start_campaign_at_selection();
        let new_campaign = host(&mut run)
            .menu_button(MM_MSG_NEW_CAMPAIGN)
            .expect("new campaign button");
        click_at_size(&mut run, new_campaign, size);
        until(&mut run, 60, "campaign list", |r| {
            screen(r) == ActiveScreen::CampaignPick
        });
        frames(&mut run, 60);
        let row = host(&mut run)
            .campaign_pick_row_centre(0)
            .expect("first campaign row");
        click_at_size(&mut run, row, size);
        for pressed in [true, false] {
            run.event(InputEvent::Key {
                key: keys::KEY_RETURN,
                phys_id: keys::KEY_RETURN as u16,
                pressed,
                repeat: false,
                text: None,
            });
            frames(&mut run, 1);
        }
        until(&mut run, 120, "mission selection", |r| {
            r.core.state() == &GameState::Logistics
        });
        skip_logistics_movie(&mut run);
        frames(&mut run, 90);
        host(&mut run).request_movie_for_test(MovieRequest::Open("STANDIN".to_string()));

        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        let with_movie = root.join(format!("vidcom-{}x{}-movie.png", size.0, size.1));
        let without_movie = root.join(format!("vidcom-{}x{}-still.png", size.0, size.1));
        run.step(Some(&with_movie)).unwrap();
        host(&mut run).request_movie_for_test(MovieRequest::Stop);
        run.step(Some(&without_movie)).unwrap();
        assert_movie_difference_is_inside_vidcom(&with_movie, &without_movie, size);
    }
}

fn click_at_size<D: SimDriver + 'static>(
    run: &mut HeadlessRun<D>,
    ui: (f32, f32),
    size: (u32, u32),
) {
    frames(run, 8);
    let layout = mc2_ui::render::UiLayout::new((800, 600), size, mc2_ui::render::ScaleMode::Fit);
    let (x, y) = layout.to_target(ui.0, ui.1);
    run.event(InputEvent::CursorMoved {
        x: f64::from(x),
        y: f64::from(y),
    });
    frames(run, 1);
    button(run, true);
    frames(run, 1);
    button(run, false);
    frames(run, 1);
}

fn assert_movie_difference_is_inside_vidcom(before: &Path, after: &Path, size: (u32, u32)) {
    let (w, h, before) = read_png(before);
    let (after_w, after_h, after) = read_png(after);
    assert_eq!((w, h), size);
    assert_eq!((after_w, after_h), size);
    let layout = mc2_ui::render::UiLayout::new((800, 600), size, mc2_ui::render::ScaleMode::Fit);
    // The FIT rectangle is the VidCom outline; playback is inset one UI pixel on every edge.
    let (left, top) = layout.to_target(117.0, 88.0);
    let (right, bottom) = layout.to_target(380.0, 351.0);
    let (left, top, right, bottom) = (
        left.floor() as usize,
        top.floor() as usize,
        right.ceil() as usize,
        bottom.ceil() as usize,
    );
    let channels = before.len() / (w as usize * h as usize);
    let mut changed_inside = 0;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let pixel = (y * w as usize + x) * channels;
            let changed = before[pixel..pixel + 3]
                .iter()
                .zip(&after[pixel..pixel + 3])
                .any(|(a, b)| a.abs_diff(*b) > 2);
            if !changed {
                continue;
            }
            assert!(
                x >= left && x < right && y >= top && y < bottom,
                "movie changed pixel ({x}, {y}) outside VidCom at {w}x{h}"
            );
            changed_inside += 1;
        }
    }
    assert!(
        changed_inside > 100,
        "the movie frame advanced between renders"
    );
}

#[test]
fn final_campaign_win_autosaves_and_loads_through_the_production_host() {
    let Some((mut run, cues)) = winning_session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-final-win-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));
    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    until(
        &mut run,
        90,
        "the mission before final-result dispatch",
        |r| r.core.state() == &GameState::InMission,
    );
    let data = std::sync::Arc::clone(host(&mut run).campaign().unwrap().data());
    let before_final_win = campaign_before_final_win(data);
    assert!(!before_final_win.campaign_over());
    host(&mut run).replace_campaign_for_result_test(before_final_win);
    assert!(!host(&mut run).campaign().unwrap().campaign_over());
    until(
        &mut run,
        90,
        "the actual final-win result transition",
        |r| {
            matches!(r.core.state(), GameState::Results(_))
                && host(r)
                    .campaign()
                    .is_some_and(|campaign| campaign.campaign_over())
        },
    );
    until(&mut run, 30, "the final-campaign autosave", |_| {
        saves.join("autosave.ron").is_file()
    });
    assert!(host(&mut run)
        .notices()
        .iter()
        .any(|notice| notice == "Campaign completed and autosaved"));
    let saved = CampaignSaveService::new(&saves)
        .load(
            SaveSlot::Autosave,
            std::sync::Arc::clone(host(&mut run).campaign().unwrap().data()),
        )
        .expect("final-win autosave is readable");
    assert!(saved.campaign_over());

    let Some((mut fresh, _)) = session() else {
        let _ = std::fs::remove_dir_all(saves);
        return;
    };
    host(&mut fresh).set_campaign_save_service(CampaignSaveService::new(&saves));
    frames(&mut fresh, 2);
    skip_menu_intro(&mut fresh);
    frames(&mut fresh, 90);
    let load = host(&mut fresh)
        .menu_button(MM_MSG_LOAD)
        .expect("Load Game button");
    click(&mut fresh, load);
    until(&mut fresh, 90, "completed campaign load row", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    let row = host(&mut fresh)
        .save_load_row_centre(0)
        .expect("list row 0");
    click(&mut fresh, row);
    frames(&mut fresh, 40); // the panel slides out
    assert_eq!(screen(&mut fresh), ActiveScreen::MainMenu);
    assert!(host(&mut fresh).campaign().unwrap().campaign_over());
    assert_eq!(
        host(&mut fresh).campaign().unwrap().to_save(),
        saved.to_save(),
        "a fresh production host restores the final campaign completion"
    );
    std::fs::remove_dir_all(saves).unwrap();
}

/// Chooses a row of the campaign list New Campaign opens (`LogisticsSaveDialog::beginCampaign`).
fn choose_campaign<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, row: usize) {
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

/// Steps until `done`, at most `max` frames.
fn until<D: SimDriver + 'static>(
    run: &mut HeadlessRun<D>,
    max: usize,
    what: &str,
    done: impl Fn(&mut HeadlessRun<D>) -> bool,
) {
    for _ in 0..max {
        if done(run) {
            return;
        }
        run.step(None).unwrap();
    }
    assert!(done(run), "{what}: not reached in {max} frames");
}

fn pointer<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, ui: (f32, f32)) {
    run.event(InputEvent::CursorMoved {
        x: f64::from(ui.0 * SCALE),
        y: f64::from(ui.1 * SCALE),
    });
}

fn button<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, down: bool) {
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed: down,
    });
}

/// A click the way a player makes one: over the target, button down, button up, each in its own
/// frame, after a pause longer than the double-click time.
fn click<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, ui: (f32, f32)) {
    frames(run, 8);
    pointer(run, ui);
    frames(run, 1);
    button(run, true);
    frames(run, 1);
    button(run, false);
    frames(run, 1);
}

/// Rewrites a screenshot as an opaque RGB PNG at the best compression, keeping the top five bits of
/// each channel (the app writes full RGBA at the default level, which is over the 300 KB budget
/// for these screens).
fn shrink(path: &std::path::Path) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    assert!(buf.chunks_exact(4).all(|p| p[3] == 255), "an opaque frame");
    let rgb: Vec<u8> = buf
        .chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]].map(|c| (c & 0xf8) | (c >> 5)))
        .collect();
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(path).unwrap()),
        info.width,
        info.height,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    enc.set_filter(png::Filter::Adaptive);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

/// Decodes a PNG written by the app (8-bit RGBA).
fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// The banner's box (40..760 x 520..580 of the 800x600 space) of a fresh frame, as RGB bytes.
fn banner_box(run: &mut HeadlessRun<NullSim>, name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    run.step(Some(&path)).unwrap();
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let w = info.width as usize;
    let px = info.buffer_size() / (w * info.height as usize);
    let (x0, x1) = ((48.0 * SCALE) as usize, (752.0 * SCALE) as usize);
    let (y0, y1) = ((524.0 * SCALE) as usize, (576.0 * SCALE) as usize);
    let mut out = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            out.extend_from_slice(&buf[(y * w + x) * px..][..3]);
        }
    }
    out
}

/// The banner is on screen: the box differs from the same screen once the banner has run out.
fn assert_banner_drawn(run: &mut HeadlessRun<NullSim>, tag: &str) {
    let with = banner_box(run, &format!("{tag}_with.png"));
    // the banner lasts a few seconds of frame time
    for _ in 0..2000 {
        if host(run).banner().is_none() {
            break;
        }
        run.step(None).unwrap();
    }
    assert!(host(run).banner().is_none(), "the banner expires");
    let without = banner_box(run, &format!("{tag}_without.png"));
    let differing = with
        .chunks_exact(3)
        .zip(without.chunks_exact(3))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing > 2000,
        "{tag}: banner drawn ({differing} pixels differ)"
    );
}

fn screen<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) -> ActiveScreen {
    host(run).screen()
}

fn return_to_main_menu<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) {
    let page = match screen(run) {
        ActiveScreen::Logistics(page) => page,
        other => panic!("expected a logistics page before main menu, got {other:?}"),
    };
    let at = host(run)
        .page_button(page, ids::MB_MSG_MAINMENU)
        .expect("current logistics page Main Menu button");
    click(run, at);
}

#[test]
fn the_host_replaces_the_null_ui_and_shows_the_main_menu() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    frames(&mut run, 2);
    assert_eq!(run.core.state(), &GameState::MainMenu);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    assert!(host(&mut run).campaign().is_none(), "no campaign yet");
    // The C++ starts menu music after the intro movie and the separate splash animation.
    assert!(!cues.borrow().iter().any(|c| matches!(c, UiCue::Music(_))));
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
    frames(&mut run, 1);
    assert!(
        host(&mut run).movie_progress().is_none(),
        "first Escape stops the movie"
    );
    assert!(
        host(&mut run).menu_intro_showing(),
        "first Escape leaves the splash showing"
    );
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
    frames(&mut run, 1);
    assert!(
        !host(&mut run).menu_intro_showing(),
        "second Escape dismisses the splash"
    );
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
    frames(&mut run, 90);
    let music: Vec<_> = cues
        .borrow()
        .iter()
        .filter(|c| matches!(c, UiCue::Music(_)))
        .cloned()
        .collect();
    assert_eq!(music.len(), 1, "{music:?}");
}

/// Plays the menu and the logistics screens up to the pilot-ready page with a crewed mech.
fn play_to_pilot_ready<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, cues: &Cues) {
    frames(run, 2);
    assert_eq!(screen(run), ActiveScreen::MainMenu);

    // skip the splash intro, let the menu slide in
    skip_menu_intro(run);
    frames(run, 90);
    assert_eq!(screen(run), ActiveScreen::MainMenu);

    // 1. NEW CAMPAIGN: the menu slides out, the campaign starts, mission selection begins
    host(run).start_campaign_at_selection();
    let at = host(run)
        .menu_button(MM_MSG_NEW_CAMPAIGN)
        .expect("new campaign button");
    click(run, at);
    choose_campaign(run, 0);
    until(run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    // The first group's pre-operation cutscene plays on entering logistics. Skip it before
    // driving the mission-selection controls below.
    skip_logistics_movie(run);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::SELECT));
    {
        // `MissionSelectionScreen::update` plays the logistics tune, once, on entering
        let tune = host(run)
            .campaign()
            .unwrap()
            .mission_info()
            .current_logistics_tune_id();
        let music: Vec<_> = cues
            .borrow()
            .iter()
            .filter(|c| matches!(c, UiCue::Music(_)))
            .cloned()
            .collect();
        assert_eq!(
            music.len(),
            3,
            "menu tune, campaign tune, then campaign tune resumed after the cutscene: {music:?}"
        );
        assert_eq!(music[1], UiCue::Music(tune));
        assert_eq!(music[2], UiCue::Music(tune));
        let lg = host(run).campaign().expect("campaign started");
        assert_eq!(lg.current_mission(), "mc2_01");
        assert!(lg.force_group().is_empty());
    }

    // the purchase screen is not ported: stand in for it with a purchase on the campaign
    {
        let lg = host(run).campaign_mut().unwrap();
        let mut variants = lg.purchasable_mechs();
        variants.sort_by_key(|v| lg.variant_cost(*v));
        lg.add_c_bills(200_000);
        lg.purchase_mech(variants[0]).expect("purchase");
        lg.purchase_mech(variants[0]).expect("purchase");
    }

    // 2. selection -> briefing
    let at = host(run)
        .page_button(slot::SELECT, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::BRIEFING));
    assert_eq!(host(run).campaign().unwrap().current_mission(), "mc2_01");

    // 3-5. the briefing, the mech bay and the pilot-ready page
    brief_deploy_and_crew(run, cues);
}

/// Briefing -> mech bay (a mech dragged onto the first slot) -> pilot ready (ADD crews it).
fn brief_deploy_and_crew<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, cues: &Cues) {
    // 3. briefing -> mech bay
    let at = host(run)
        .page_button(slot::BRIEFING, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::MECH_BAY));

    // 4. drag a stored mech onto the first deployment slot
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
    let mech = {
        let lg = host(run).campaign().unwrap();
        assert_eq!(lg.force_group().len(), 1, "the dragged mech is deployed");
        lg.force_group()[0]
    };

    // 5. mech bay -> pilot ready; ADD crews the mech
    let at = host(run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
    assert_eq!(screen(run), ActiveScreen::Logistics(slot::PILOT_READY));
    assert!(
        cues.borrow()
            .contains(&UiCue::Sound(mc2_ui::logistics::LOG_NEXTBACKBUTTONS)),
        "the navigation sound reaches the cue sink"
    );
    let at = host(run)
        .page_button(slot::PILOT_READY, ids::PR_MSG_ADD)
        .unwrap();
    click(run, at);
    {
        let lg = host(run).campaign().unwrap();
        let pilot = lg
            .mech(mech)
            .and_then(|m| m.pilot)
            .expect("a pilot is assigned");
        assert!(lg.pilots()[pilot].used);
    }
}

/// Clicks NEXT on the pilot-ready page: the loading screen starts.
fn start_loading<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) {
    let at = host(run)
        .page_button(slot::PILOT_READY, ids::MB_MSG_NEXT)
        .unwrap();
    click(run, at);
}

#[test]
fn main_menu_to_pilot_ready_and_into_the_mission() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    play_to_pilot_ready(&mut run, &cues);
    // 6. pilot ready -> loading -> the mission through the existing path (NullSim)
    start_loading(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::LOADING));
    frames(&mut run, 1);
    let doors = |cues: &Cues, id: i32| {
        cues.borrow()
            .iter()
            .filter(|c| **c == UiCue::Sound(id))
            .count()
    };
    assert_eq!(doors(&cues, mc2_ui::ctx::sfx::LOAD_DOORS_CLOSING), 1);
    assert_eq!(doors(&cues, mc2_ui::ctx::sfx::LOAD_DOORS_OPENING), 0);
    until(&mut run, 600, "the mission", |r| {
        r.core.state() == &GameState::InMission
    });
    // the load worked first: the loading screen is still up and its doors close afterwards
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::LOADING));
    assert_eq!(run.core.mission.as_ref().unwrap().name, "mc2_01");
    let spec = host(&mut run)
        .take_launch()
        .expect("the launch spec was kept");
    assert_eq!(spec.mission, "mc2_01");
    assert_eq!(
        spec.load_type,
        LoadType::SpQuickstart,
        "mission one skips logistics"
    );
    until(&mut run, 600, "the doors out", |r| {
        host(r).screen() == ActiveScreen::None
    });
    assert_eq!(run.core.state(), &GameState::InMission);
    assert_eq!(doors(&cues, mc2_ui::ctx::sfx::LOAD_DOORS_CLOSING), 1);
    assert_eq!(doors(&cues, mc2_ui::ctx::sfx::LOAD_DOORS_OPENING), 1);
}

#[test]
fn campaign_pages_save_load_and_options_work_through_the_production_host() {
    let Some((mut run, _)) = session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-slots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    // the first missions skip purchasing: win them, so the bay offers the purchase screen
    {
        let lg = host(&mut run).campaign_mut().unwrap();
        lg.add_c_bills(1_000_000);
        for _ in 0..3 {
            lg.set_mission_completed(&Outcome::won())
                .expect("completed");
        }
    }
    // selection -> briefing -> mech bay, then the purchase button of the bay
    for (from, id) in [
        (slot::SELECT, ids::MB_MSG_NEXT),
        (slot::BRIEFING, ids::MB_MSG_NEXT),
    ] {
        let at = host(&mut run).page_button(from, id).unwrap();
        click(&mut run, at);
    }
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::MECH_BAY));
    let at = host(&mut run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_BUY)
        .unwrap();
    click(&mut run, at);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::PURCHASE));
    let before = host(&mut run).campaign().unwrap().c_bills();
    let add = host(&mut run)
        .page_button(slot::PURCHASE, ids::MB_MSG_ADD)
        .expect("purchase Add button");
    click(&mut run, add);
    let after = host(&mut run).campaign().unwrap().c_bills();
    assert!(
        after < before,
        "buying a mech changes C-Bills: {before} -> {after}"
    );
    {
        let lg = host(&mut run).campaign_mut().unwrap();
        let key = lg
            .inventory_keys()
            .into_iter()
            .find(|key| lg.can_add_mech_to_force_group(*key))
            .expect("purchased mech can be deployed");
        lg.add_mech_to_force_group(key, 1)
            .expect("deploy purchased mech");
    }
    let accept = host(&mut run)
        .page_button(slot::PURCHASE, ids::MB_MSG_NEXT)
        .expect("purchase Accept button");
    click(&mut run, accept);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::MECH_BAY));
    let edit = host(&mut run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_CHANGE_LOADOUT)
        .expect("mech bay Change Loadout button");
    click(&mut run, edit);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::MECH_LAB));
    let cancel_lab = host(&mut run)
        .page_button(slot::MECH_LAB, ids::MB_MSG_PREV)
        .expect("mech lab Cancel button");
    click(&mut run, cancel_lab);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::MECH_BAY));

    // Escape opens the in-campaign Options slot over the current logistics page.
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
    frames(&mut run, 1);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::OPTIONS));
    let cancel = host(&mut run)
        .page_button(slot::OPTIONS, NO)
        .expect("campaign options Cancel button");
    click(&mut run, cancel);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::MECH_BAY));

    let saved = host(&mut run).campaign().unwrap().to_save();
    let main_menu = host(&mut run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_MAINMENU)
        .expect("mech bay Main Menu button");
    click(&mut run, main_menu);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    let save = host(&mut run)
        .menu_button(MM_MSG_SAVE)
        .expect("Save Game button");
    click(&mut run, save);
    until(&mut run, 90, "save slots", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
    frames(&mut run, 40); // the panel slides out
    assert_eq!(
        screen(&mut run),
        ActiveScreen::Logistics(slot::MECH_BAY),
        "canceling the campaign save returns to the page that opened it"
    );
    let main_menu = host(&mut run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_MAINMENU)
        .expect("mech bay Main Menu button");
    click(&mut run, main_menu);
    let save = host(&mut run)
        .menu_button(MM_MSG_SAVE)
        .expect("Save Game button");
    click(&mut run, save);
    until(&mut run, 90, "save slots after cancel", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    // click into the name box: it then edits the name itself (typing, Backspace, retyping)
    let name_box = host(&mut run).save_load_edit_point().expect("the name box");
    click(&mut run, name_box);
    type_text(&mut run, "slot-notesx");
    let (shown, kept) = host(&mut run).save_load_names().unwrap();
    assert_eq!(shown, kept, "the box and the saved name agree");
    assert_eq!(kept.to_ascii_lowercase(), "slot-notesx");
    run.event(mc2_ui::harness::events::key(keys::KEY_BACK, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_BACK, false, None));
    frames(&mut run, 1);
    let (shown, kept) = host(&mut run).save_load_names().unwrap();
    assert_eq!(shown, kept);
    assert_eq!(kept.to_ascii_lowercase(), "slot-notes");
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, false, None));
    frames(&mut run, 40); // the panel slides out, then the save is written
    assert!(saves.join("slot-notes.ron").is_file());
    assert!(host(&mut run)
        .notices()
        .iter()
        .all(|n| !n.contains("unavailable")));

    assert_eq!(
        screen(&mut run),
        ActiveScreen::Logistics(slot::MECH_BAY),
        "saving from the campaign menu returns to the page that opened it"
    );
    let main_menu = host(&mut run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_MAINMENU)
        .expect("mech bay Main Menu button");
    click(&mut run, main_menu);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    frames(&mut run, 45);

    let save = host(&mut run)
        .menu_button(MM_MSG_SAVE)
        .expect("Save Game button");
    click(&mut run, save);
    until(&mut run, 90, "the second named save", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    type_text(&mut run, "slot-1");
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, false, None));
    frames(&mut run, 40); // the panel slides out, then the save is written
    assert!(saves.join("slot-1.ron").is_file());

    // Save the changed campaign over an existing name. No must retain the old bytes; Yes must
    // atomically replace them. Both choices are driven through production-host pointer input.
    let original_named = std::fs::read(saves.join("slot-notes.ron")).unwrap();
    host(&mut run).campaign_mut().unwrap().add_c_bills(321);
    let changed = host(&mut run).campaign().unwrap().to_save();
    let main_menu = host(&mut run)
        .page_button(slot::MECH_BAY, ids::MB_MSG_MAINMENU)
        .expect("mech bay Main Menu button");
    click(&mut run, main_menu);
    let save = host(&mut run)
        .menu_button(MM_MSG_SAVE)
        .expect("Save Game button");
    click(&mut run, save);
    until(&mut run, 90, "the overwrite save picker", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    let row = host(&mut run).save_load_row_centre(1).expect("list row 1");
    click(&mut run, row); // second row: slot-notes
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, false, None));
    frames(&mut run, 1);
    frames(&mut run, 40);
    let no = host(&mut run).save_load_prompt_button(false).expect("No");
    click(&mut run, no);
    frames(&mut run, 40); // the prompt slides out
    assert_eq!(
        std::fs::read(saves.join("slot-notes.ron")).unwrap(),
        original_named
    );
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, false, None));
    frames(&mut run, 1);
    frames(&mut run, 40);
    let yes = host(&mut run).save_load_prompt_button(true).expect("Yes");
    click(&mut run, yes);
    frames(&mut run, 90); // the prompt and the panel slide out, then the save is written
    assert_ne!(
        std::fs::read(saves.join("slot-notes.ron")).unwrap(),
        original_named
    );

    for (row_index, expected) in [(0usize, saved), (1, changed)] {
        return_to_main_menu(&mut run);
        let load = host(&mut run)
            .menu_button(MM_MSG_LOAD)
            .expect("Load Game button");
        click(&mut run, load);
        until(&mut run, 90, "the named campaign saves", |r| {
            screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
        });
        let row = host(&mut run)
            .save_load_row_centre(row_index)
            .expect("list row");
        click(&mut run, row);
        frames(&mut run, 40); // the panel slides out
        assert_eq!(run.core.state(), &GameState::Logistics);
        assert_eq!(
            host(&mut run).campaign().unwrap().to_save(),
            expected,
            "the production load screen restores the selected named save exactly"
        );
    }
    std::fs::remove_dir_all(saves).unwrap();
}

#[test]
fn the_load_dialog_shows_the_save_details_and_ignores_clicks_on_empty_list_space() {
    let Some((mut run, _)) = session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-load-info-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    let service = CampaignSaveService::new(&saves);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
    let data = std::sync::Arc::new(mc2_campaign::GameData::load(&mut vfs).unwrap());
    let fixture = mc2_campaign::Logistics::new(data).unwrap();
    service.save_named("alpha", &fixture).unwrap();
    host(&mut run).set_campaign_save_service(service);
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    let load = host(&mut run)
        .menu_button(MM_MSG_LOAD)
        .expect("Load Game button");
    click(&mut run, load);
    until(&mut run, 120, "the load dialog opens", |r| {
        screen(r) == ActiveScreen::SaveLoad
    });
    // Enter during the entrance animation does not confirm (`enterAnim.isDone()` gate)
    assert!(!host(&mut run).save_load_ready(), "still sliding in");
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, false, None));
    frames(&mut run, 1);
    assert_eq!(screen(&mut run), ActiveScreen::SaveLoad);
    until(&mut run, 120, "the load dialog", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    let (info, picture) = host(&mut run).save_load_info().unwrap();
    assert!(info.contains("C-BILLS"), "campaign details: {info}");
    assert!(info.contains("TIME :"), "save time: {info}");
    assert!(
        info.lines().count() <= 6,
        "the details fit the info box: {info}"
    );
    assert!(picture, "the mission picture of the save's current mission");
    // a click under the last row neither loads nor changes anything
    let blank = host(&mut run)
        .save_load_blank_point()
        .expect("room under the one row");
    let before = (
        host(&mut run).save_load_selected(),
        host(&mut run).save_load_names(),
    );
    click(&mut run, blank);
    frames(&mut run, 60);
    assert_eq!(screen(&mut run), ActiveScreen::SaveLoad);
    assert_eq!(
        before,
        (
            host(&mut run).save_load_selected(),
            host(&mut run).save_load_names()
        )
    );
    // Enter after the entrance confirms the selected save
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_RETURN, false, None));
    frames(&mut run, 60);
    assert_ne!(screen(&mut run), ActiveScreen::SaveLoad);
    std::fs::remove_dir_all(saves).unwrap();
}

#[test]
fn loading_a_campaign_that_skips_selection_restarts_on_its_loading_screen() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    play_to_pilot_ready(&mut run, &cues);
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-restart-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    let campaign = host(&mut run).campaign().expect("initialized logistics");
    let info = campaign.mission_info();
    let Some((stage, mission, mission_name)) =
        info.groups.iter().enumerate().find_map(|(stage, group)| {
            group
                .missions
                .iter()
                .enumerate()
                .find(|(_, mission)| !mission.play_logistics && !mission.play_mission_selection)
                .map(|(mission_index, mission)| (stage, mission_index, mission.file_name.clone()))
        })
    else {
        panic!("campaign data has a mission that skips logistics and selection");
    };
    let mut save = campaign.to_save();
    for group in save.groups.iter_mut().take(stage) {
        for mission in &mut group.missions {
            mission.completed = true;
        }
    }
    save.current_stage = stage as i64;
    save.current_mission = mission as i64;
    save.current_mission_name = mission_name.clone();
    let loaded = mc2_campaign::Logistics::from_save(std::sync::Arc::clone(campaign.data()), &save)
        .expect("prepared skip-logistics campaign state");
    assert!(loaded.logistics.skip_logistics());
    assert!(!loaded.logistics.show_choose_mission());
    CampaignSaveService::new(&saves)
        .save_named("skip-selection", &loaded.logistics)
        .unwrap();
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));
    let main_menu = host(&mut run)
        .page_button(slot::PILOT_READY, ids::MB_MSG_MAINMENU)
        .expect("pilot-ready Main Menu button");
    click(&mut run, main_menu);
    frames(&mut run, 90);
    let load = host(&mut run)
        .menu_button(MM_MSG_LOAD)
        .expect("Load Game button");
    click(&mut run, load);
    until(&mut run, 90, "campaign load file", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    let row = host(&mut run).save_load_row_centre(0).expect("list row 0");
    click(&mut run, row);
    frames(&mut run, 40); // the panel slides out
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::LOADING));
    until(
        &mut run,
        90,
        "the mission selected by the loaded campaign",
        |r| r.core.state() == &GameState::InMission,
    );
    assert_eq!(run.core.mission.as_ref().unwrap().name, mission_name);
    std::fs::remove_dir_all(saves).unwrap();
}

#[test]
fn in_campaign_options_apply_to_the_next_mission_and_persist_for_a_fresh_host() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let config = temp_root.join(format!("mc2-ui-host-options-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&config);
    host(&mut run).set_config_directory(config.clone());
    play_to_pilot_ready(&mut run, &cues);

    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_ESCAPE, false, None));
    frames(&mut run, 1);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::OPTIONS));
    let tab = host(&mut run)
        .page_button(slot::OPTIONS, mc2_ui::screens::options::MSB_TAB0 + 2)
        .expect("campaign gameplay options tab");
    click(&mut run, tab);
    let elite = host(&mut run)
        .campaign_options_control_point(mc2_ui::screens::options::MSG_ELITE)
        .expect("Elite difficulty control");
    click(&mut run, elite);
    let ammo = host(&mut run)
        .campaign_options_control_point(mc2_ui::screens::options::MSG_UNLIMITED_AMMO)
        .expect("Unlimited Ammo control");
    click(&mut run, ammo);
    let yes = host(&mut run)
        .page_button(slot::OPTIONS, mc2_ui::screens::dialog::YES)
        .expect("campaign options Yes button");
    click(&mut run, yes);
    frames(&mut run, 2);

    let accepted = host(&mut run).prefs().unwrap();
    assert_eq!(accepted.game_difficulty, 3);
    assert!(!accepted.use_unlimited_ammo);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::PILOT_READY));

    start_loading(&mut run);
    until(&mut run, 90, "the options-adjusted launch", |r| {
        host(r).launch_spec().is_some()
    });
    let spec = host(&mut run).launch_spec().unwrap();
    assert_eq!(
        spec.difficulty, 3,
        "accepted difficulty reaches the mission"
    );
    assert!(
        !spec.unlimited_ammo,
        "accepted ammo setting reaches the mission"
    );

    let Some((mut fresh, _)) = session() else {
        let _ = std::fs::remove_dir_all(config);
        return;
    };
    host(&mut fresh).set_config_directory(config.clone());
    frames(&mut fresh, 2);
    let persisted = host(&mut fresh).prefs().unwrap();
    assert_eq!(persisted.game_difficulty, 3);
    assert!(!persisted.use_unlimited_ammo);
    std::fs::remove_dir_all(config).unwrap();
}

#[test]
fn production_host_autosaves_after_salvage_and_pilot_review() {
    let Some((mut run, cues)) = winning_session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-autosave-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    host(&mut run).set_campaign_save_service(CampaignSaveService::new(&saves));
    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    until(&mut run, 90, "mission start", |r| {
        r.core.state() == &GameState::InMission
    });
    until(&mut run, 90, "winning mission result", |r| {
        matches!(r.core.state(), GameState::Results(_))
    });
    frames(&mut run, 2);
    assert_eq!(screen(&mut run), ActiveScreen::Results);

    let salvage_done = host(&mut run)
        .results_button(mc2_ui::mission_results::SALVAGE_DONE)
        .expect("salvage Done button");
    click(&mut run, salvage_done);
    // the review slides in and reveals its pilots before its Done is enabled
    until(&mut run, 600, "the pilot review list", |r| {
        host(r).results().is_some_and(|r| {
            r.stage() == Some(mc2_campaign::results::ResultsStage::PilotReview)
                && r.pilots.list.is_done()
        })
    });
    let pilot_done = host(&mut run)
        .results_button(mc2_ui::mission_results::PILOT_DONE)
        .expect("pilot review Done button");
    click(&mut run, pilot_done);
    // Pilot review's authored one-second fade starts on the click-release frame, so allow a
    // complete fade plus scheduling margin before checking that the production flow advanced.
    until(&mut run, 60, "logistics after results", |r| {
        r.core.state() == &GameState::Logistics
    });

    let autosave = saves.join("autosave.ron");
    assert!(
        autosave.is_file(),
        "the production results path wrote autosave"
    );
    let campaign = host(&mut run).campaign().expect("active campaign");
    let restored = CampaignSaveService::new(&saves)
        .load(SaveSlot::Autosave, std::sync::Arc::clone(campaign.data()))
        .expect("production autosave can be loaded");
    let autosaved = restored.to_save();
    assert_eq!(autosaved.last_mission_name, "mc2_01");
    assert_eq!(autosaved.current_mission_name, "mc2_02");

    for number in 1..=10 {
        CampaignSaveService::new(&saves)
            .save_slot(number, &restored)
            .unwrap();
    }
    let Some((mut fresh, _)) = session() else {
        let _ = std::fs::remove_dir_all(saves);
        return;
    };
    host(&mut fresh).set_campaign_save_service(CampaignSaveService::new(&saves));
    frames(&mut fresh, 2);
    skip_menu_intro(&mut fresh);
    frames(&mut fresh, 90);
    let load = host(&mut fresh)
        .menu_button(MM_MSG_LOAD)
        .expect("Load Game button");
    click(&mut fresh, load);
    until(&mut fresh, 90, "eleven save rows", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    for _ in 0..10 {
        fresh.event(mc2_ui::harness::events::key(keys::KEY_DOWN, true, None));
        frames(&mut fresh, 1);
        fresh.event(mc2_ui::harness::events::key(keys::KEY_DOWN, false, None));
        frames(&mut fresh, 1);
    }
    // After ten Down keys the autosave is the last visible row; clicking it loads immediately.
    let row = host(&mut fresh)
        .save_load_row_centre(10)
        .expect("list row 10");
    click(&mut fresh, row);
    frames(&mut fresh, 40); // the panel slides out
    assert_eq!(fresh.core.state(), &GameState::Logistics);
    assert_eq!(
        host(&mut fresh).campaign().unwrap().to_save(),
        autosaved,
        "the visible autosave row can be loaded with the pointer"
    );
    std::fs::remove_dir_all(saves).unwrap();
}

/// Tolerance of the paint oracle, per channel (of 255).  The prediction is exact arithmetic on
/// the texel and the light; the sampled byte is the 8-bit target's rounding of the shader's
/// float result, and the sample sits on a pixel that is not exactly the triangle centre (the
/// light varies a little across the face): a few levels at most.
const PAINT_TOLERANCE: f32 = 4.0;
/// `options.cfg`'s `BaseColor` and `Highlightcolor` (`gamedata/mc2exe/options.cfg:50-51`).
const OPTIONS_BASE: u32 = 0x00fa_e525;
const OPTIONS_HIGHLIGHT: u32 = 0x0007_083e;

/// Renders the running mission's frame to `name` and reads it back as RGBA.
fn paint_frame(run: &mut HeadlessRun<NullSim>, name: &str) -> Vec<u8> {
    let shot = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    // a close-up: the nearest zoom magnifies the mech's texture, so a pixel is under a texel
    run.core.mission.as_mut().unwrap().rig.zoom_max();
    // The camera eases in from the mover's pre-snap Z (`GameCamera::activate`): the mechs are on
    // screen from about frame 100 (headless.rs `mc2_01_frame`).
    frames(run, 100);
    run.step(Some(&shot)).unwrap();
    let (w, h, px) = read_png(&shot);
    assert_eq!((w, h), SIZE);
    px
}

/// The colour `Mech3DAppearance::setPaintScheme` (`mclib/mech3d.cpp:1619-1717`) makes of a pure
/// mask texel whose one non-zero channel has value `v`, for the paint colour `c` as the caller
/// stores it (the Windows build uses it unswapped, `mech3d.cpp:1738-1741`): each colour channel is
/// scaled by `1 - (1 - v/256)^2` and truncated to a byte (`mech3d.cpp:1636-1657`).  Returns
/// (r, g, b) as 0..255.
fn painted_texel_by_hand(v: u8, c: u32) -> [f32; 3] {
    let mut k = v as f32 * 0.003_906_25; // "Divide by 256"
    k = 1.0 - k;
    k *= k;
    k = 1.0 - k;
    let ch = |shift: u32| (((c >> shift) & 0xff) as f32 * k) as u8 as f32;
    [ch(16), ch(8), ch(0)]
}

/// The mask classes of `Mech3DAppearance::setPaintScheme` (`mclib/mech3d.cpp:1636-1700`), in its
/// order: a texel with `g = b = 0` takes the red mask (also pure black, `r = 0`), else `r = b = 0`
/// the green one, else `r = g = 0` the blue one; any other texel is kept.
fn mask_of(t: [u8; 4]) -> Option<(usize, u8)> {
    if t[1] == 0 && t[2] == 0 {
        Some((0, t[0]))
    } else if t[0] == 0 && t[2] == 0 {
        Some((1, t[1]))
    } else if t[0] == 0 && t[1] == 0 {
        Some((2, t[2]))
    } else {
        None
    }
}

/// One spot of a player mech's mask texture, found in the raw texture and located on screen.
struct Probe {
    /// The four texels the bilinear filter blends, their weights, and the filter position.
    texels: [[u8; 4]; 4],
    weights: [f32; 4],
    at: (f32, f32),
    /// The pixel to sample, the triangle's projected area in pixels, and the texel footprint of a
    /// pixel (texels per pixel along the longer axis; below one the filter is plain bilinear).
    pixel: (usize, usize),
    area: f32,
    footprint: f32,
    /// The lit colour factor and the fog visibility at the sample (`fs_shape`, `scene.wgsl`).
    lit: [f32; 3],
    fog: [f32; 3],
    vis: f32,
}

/// Finds, among the triangles of the player mech `inst` that are textured from its mask
/// (texture 0) and face the camera, the one with the smallest texel footprint (the most magnified)
/// among those of at least `min_area` pixels, whose four filter texels at the centre of the pixel
/// holding the triangle's centroid are all masks of the wanted kind: `blue` the base mask (blue),
/// otherwise a highlight mask (red or green), with a value >= 64.  The raw texture comes from the
/// game data's TGA decoder; no paint code is involved.  The UV at the pixel is the perspective
/// correct interpolation; the light is evaluated by hand from the scene's lights and the vertex
/// data (`fs_shape`, `scene.wgsl`).
fn find_probe(
    run: &HeadlessRun<NullSim>,
    img: &(i64, i64, Vec<u8>),
    inst: &mc2_render::scene::ShapeInstance,
    blue: bool,
    min_area: f32,
) -> Option<Probe> {
    use mc2_render::math::Vec3;
    use mc2_render::shape::{instance_node_matrices, NodeKind};
    let rt = run.core.mission.as_ref().unwrap();
    let cam = rt.render_camera(
        (SIZE.0 as f32, SIZE.1 as f32),
        run.core.config.interpolate_camera,
    );
    let vp = cam.gpu_view_proj();
    let light = rt.view.mission.light.uniform();
    let model = run.core.scene.model(inst.model);
    let mats = instance_node_matrices(&inst.pose, inst.position, inst.yaw_degrees);
    let &(tw, th, ref pixels) = img;
    let texel = |x: i64, y: i64| -> [u8; 4] {
        let at = ((y * tw + x) * 4) as usize;
        [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
    };
    let eye = cam.position_world();
    let mut best: Option<Probe> = None;
    for b in &model.batches {
        if b.texture != 0 || b.kind != NodeKind::Normal || b.alpha {
            continue;
        }
        let first = b.first_vertex as usize;
        for t in model.vertices[first..first + b.vertex_count as usize].chunks_exact(3) {
            // no vertex light marker: the emissive term is zero (`vertex_emissive`)
            if t.iter().any(|v| v.argb & 0x00ff_ffff != 0) {
                continue;
            }
            let w: Vec<Vec3> = t
                .iter()
                .map(|v| mats[v.node as usize].transform_point(Vec3::from_array(v.position)))
                .collect();
            let nrm: Vec<Vec3> = t
                .iter()
                .map(|v| mats[v.node as usize].transform_vector(Vec3::from_array(v.normal)))
                .collect();
            let clip: Vec<[f32; 4]> = w
                .iter()
                .map(|p| vp.transform4([p.x, p.y, p.z, 1.0]))
                .collect();
            if clip.iter().any(|c| c[3] <= 0.0) {
                continue;
            }
            // pixel coordinates of the vertices (ndc -> viewport)
            let scr: Vec<(f32, f32)> = clip
                .iter()
                .map(|c| {
                    (
                        (c[0] / c[3] * 0.5 + 0.5) * SIZE.0 as f32,
                        (0.5 - c[1] / c[3] * 0.5) * SIZE.1 as f32,
                    )
                })
                .collect();
            let det = (scr[1].0 - scr[0].0) * (scr[2].1 - scr[0].1)
                - (scr[2].0 - scr[0].0) * (scr[1].1 - scr[0].1);
            let area = det.abs() * 0.5;
            if area < min_area {
                continue;
            }
            // the pixel holding the centroid, and its centre
            let cx = (scr[0].0 + scr[1].0 + scr[2].0) / 3.0;
            let cy = (scr[0].1 + scr[1].1 + scr[2].1) / 3.0;
            if cx < 1.0 || cy < 1.0 || cx >= SIZE.0 as f32 - 1.0 || cy >= SIZE.1 as f32 - 1.0 {
                continue;
            }
            let (px, py) = (cx.floor(), cy.floor());
            // perspective-correct attributes at a screen position
            let bary = |sx: f32, sy: f32| -> [f32; 3] {
                let l1 = ((sx - scr[0].0) * (scr[2].1 - scr[0].1)
                    - (scr[2].0 - scr[0].0) * (sy - scr[0].1))
                    / det;
                let l2 = ((scr[1].0 - scr[0].0) * (sy - scr[0].1)
                    - (sx - scr[0].0) * (scr[1].1 - scr[0].1))
                    / det;
                let s = [1.0 - l1 - l2, l1, l2];
                let q: Vec<f32> = (0..3).map(|i| s[i] / clip[i][3]).collect();
                let sum: f32 = q.iter().sum();
                [q[0] / sum, q[1] / sum, q[2] / sum]
            };
            let uv_at = |sx: f32, sy: f32| -> (f32, f32) {
                let l = bary(sx, sy);
                (
                    (0..3).map(|i| l[i] * t[i].uv[0]).sum::<f32>() * tw as f32,
                    (0..3).map(|i| l[i] * t[i].uv[1]).sum::<f32>() * th as f32,
                )
            };
            let (sx, sy) = (px + 0.5, py + 0.5);
            let (u, v) = uv_at(sx, sy);
            let (ux, vx) = uv_at(sx + 1.0, sy);
            let (uy, vy) = uv_at(sx, sy + 1.0);
            let footprint = ((ux - u).hypot(vx - v)).max((uy - u).hypot(vy - v));
            if best.as_ref().is_some_and(|p| p.footprint <= footprint) {
                continue;
            }
            // `textureSample`, linear filter at mip level 0: the four texels around (u, v)
            let (fu, fv) = (u - 0.5, v - 0.5);
            let (i0, j0) = (fu.floor() as i64, fv.floor() as i64);
            if i0 < 0 || j0 < 0 || i0 + 1 >= tw || j0 + 1 >= th {
                continue;
            }
            let (a, c) = (fu - i0 as f32, fv - j0 as f32);
            let texels = [
                texel(i0, j0),
                texel(i0 + 1, j0),
                texel(i0, j0 + 1),
                texel(i0 + 1, j0 + 1),
            ];
            let weights = [(1.0 - a) * (1.0 - c), a * (1.0 - c), (1.0 - a) * c, a * c];
            let wanted = texels
                .iter()
                .all(|&t| matches!(mask_of(t), Some((ch, val)) if val >= 64 && (ch == 2) == blue));
            // the longer axis in texels per pixel: filtering is plain bilinear while it is below one
            if !wanted || footprint > 0.9 {
                continue;
            }
            // facing the camera, and the lit colour at the sample (`fs_shape`):
            // lit = min(emissive + infinite * max(n.l, 0) + ambient, 1), emissive 0
            let l = bary(sx, sy);
            let world = Vec3::new(
                (0..3).map(|i| l[i] * w[i].x).sum(),
                (0..3).map(|i| l[i] * w[i].y).sum(),
                (0..3).map(|i| l[i] * w[i].z).sum(),
            );
            let n = Vec3::new(
                (0..3).map(|i| l[i] * nrm[i].x).sum(),
                (0..3).map(|i| l[i] * nrm[i].y).sum(),
                (0..3).map(|i| l[i] * nrm[i].z).sum(),
            )
            .normalized();
            if n.dot((eye - world).normalized()) < 0.3 {
                continue;
            }
            let sun = Vec3::new(light.shape_dir[0], light.shape_dir[1], light.shape_dir[2]);
            let ndl = n.dot(sun).max(0.0);
            let lit =
                std::array::from_fn(|i| (inst.infinite_light[i] * ndl + light.ambient[i]).min(1.0));
            // `visibility` + `apply_fog`: haze by eye distance, then the low-ground fog
            let dist = (world - eye).length();
            let haze = ((dist - cam.settings.min_haze_distance)
                / (cam.settings.max_clip_distance - cam.settings.min_haze_distance).max(1.0))
            .clamp(0.0, 1.0);
            let mut vis = 1.0 - haze;
            let span = light.fog_height[0] - light.fog_height[1];
            if span.abs() > 1.0e-3 && world.z < light.fog_height[0] {
                let f = ((light.fog_height[0] - world.z) / span).clamp(0.0, 1.0);
                vis = vis.min(1.0 - f * f);
            }
            best = Some(Probe {
                texels,
                weights,
                at: (u, v),
                pixel: (px as usize, py as usize),
                area,
                footprint,
                lit,
                fog: [light.fog[0], light.fog[1], light.fog[2]],
                vis,
            });
        }
    }
    best
}

/// The frame colour (0..255 per channel) predicted by hand for `probe` when the red and green
/// masks are painted `highlight` and the blue mask `base` (`resetPaintScheme(highlight,
/// highlight, base)`): each filter texel recoloured by the C++ formula, blended by the bilinear
/// weights, times the light, mixed with the fog.
fn predict(probe: &Probe, base: u32, highlight: u32) -> [f32; 3] {
    let mut tex = [0.0_f32; 3];
    for (t, w) in probe.texels.iter().zip(probe.weights) {
        let rgb = match mask_of(*t) {
            Some((2, v)) => painted_texel_by_hand(v, base),
            Some((_, v)) => painted_texel_by_hand(v, highlight),
            None => [t[0] as f32, t[1] as f32, t[2] as f32],
        };
        for i in 0..3 {
            tex[i] += rgb[i] / 255.0 * w;
        }
    }
    std::array::from_fn(|i| {
        let lit = tex[i] * probe.lit[i];
        ((probe.fog[i] + (lit - probe.fog[i]) * probe.vis) * 255.0).round()
    })
}

/// Checks the rendered `frame` of the running mission against the hand calculation for one texel
/// of the base (blue) mask and one of the highlight masks of the first player mech.
///
/// Home units are painted `resetPaintScheme(highlight, highlight, base)` (`code/mission.cpp:1070-1085`):
/// the red and green mask regions take the highlight colour, the blue region the base colour.
/// For each texel the sample must match the prediction for the right colour within
/// `PAINT_TOLERANCE`, and must NOT match the prediction made with base and highlight swapped.
fn assert_texel_oracle(
    run: &mut HeadlessRun<NullSim>,
    data: &Path,
    frame: &[u8],
    base: u32,
    highlight: u32,
    tag: &str,
) {
    let rt = run.core.mission.as_ref().unwrap();
    assert!(!run.core.scene.visual().enhanced_lighting, "{tag}");
    let home: Vec<usize> = rt
        .view
        .mission
        .parts
        .iter()
        .filter(|p| p.exists)
        .zip(&rt.view.part_instances)
        .filter(|(p, _)| p.team_id == 0 && p.commander_id == Some(0))
        .map(|(_, &i)| i)
        .collect();
    assert!(!home.is_empty(), "{tag}: no player part");
    let mut game = mc2_render::game::GameData::open(data).unwrap();
    let mut checked = Vec::new();
    for (blue, name) in [(true, "base"), (false, "highlight")] {
        // the first player part that has a usable texel of this mask
        let found = home.iter().find_map(|&i| {
            let inst = &rt.view.instances[i];
            let model = run.core.scene.model(inst.model);
            let tex = mc2_render::shape::texture_path(&model.texture_names[0]);
            let img = game.tga(&tex).unwrap();
            let img = (img.width as i64, img.height as i64, img.pixels);
            find_probe(run, &img, inst, blue, 30.0).map(|p| (i, tex, p))
        });
        let (i, tex, probe) = found.unwrap_or_else(|| panic!("{tag}: no {name} texel found"));
        assert!(probe.vis > 0.99, "{tag}: {name} is fogged ({})", probe.vis);
        let at = (probe.pixel.1 * SIZE.0 as usize + probe.pixel.0) * 4;
        let sample = [frame[at] as f32, frame[at + 1] as f32, frame[at + 2] as f32];
        let want = predict(&probe, base, highlight);
        let swapped = predict(&probe, highlight, base);
        let diff =
            |a: [f32; 3], b: [f32; 3]| (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f32::max);
        eprintln!(
            "{tag} {name}: part instance {i}, {tex} filter texels {:?} at {:?}, pixel {:?} \
             ({} px2, {} texels/px), lit {:?}, sample {sample:?}, predicted {want:?}, swapped {swapped:?}",
            probe.texels, probe.at, probe.pixel, probe.area, probe.footprint, probe.lit
        );
        assert!(
            diff(sample, want) <= PAINT_TOLERANCE,
            "{tag}: {name} filter texels {:?} of {tex} at pixel {:?}: sample {sample:?}, predicted {want:?}",
            probe.texels,
            probe.pixel
        );
        // negative control: base and highlight swapped must not match
        assert!(
            diff(sample, swapped) > PAINT_TOLERANCE,
            "{tag}: {name}: the swapped colours would match too: sample {sample:?}, swapped {swapped:?}"
        );
        checked.push(name);
    }
    assert_eq!(checked, ["base", "highlight"]);
}

/// Starts a campaign through the UI host with the prefs colours `base` and `highlight` (the colours
/// the host hands to the launch as the campaign's paint), launches mc2_01 and renders its first
/// frame. None when the adapter is missing and `MC2_SKIP_GPU_TESTS=1` (see `session`).
fn campaign_run(base: u32, highlight: u32, name: &str) -> Option<(HeadlessRun<NullSim>, Vec<u8>)> {
    let (mut run, cues) = session()?;
    // the oracle predicts the unshadowed, uninterpolated frame
    run.core.config.shadows = false;
    run.core.config.interpolate_camera = false;

    // Wait for the UI host to load and menu to be shown
    frames(&mut run, 10);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);

    let mut prefs = host(&mut run).prefs().unwrap().clone();
    prefs.base_color = base;
    prefs.highlight_color = highlight;
    host(&mut run).set_prefs(prefs);

    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::LOADING));
    frames(&mut run, 1);
    until(&mut run, 600, "the mission", |r| {
        r.core.state() == &GameState::InMission
    });

    let view = &run.core.mission.as_ref().unwrap().view;
    assert!(
        matches!(view.home_paint, PaintSource::Scheme { base: b, highlight: h } if b == base && h == highlight),
        "campaign paint matches: {:?}",
        view.home_paint
    );
    assert!(
        view.report.unpainted.is_empty(),
        "no unpainted mechs: {:?}",
        view.report.unpainted
    );
    let px = paint_frame(&mut run, name);
    Some((run, px))
}

/// The player mechs of a campaign launch wear the campaign's colours, not the options.cfg ones.
/// The campaign colours here are non-default (base 0x112233, highlight 0x445566).  One base and
/// one highlight texel of a player mech's mask texture are located in the raw texture and on
/// screen, and the rendered pixel is compared with the hand-computed recolour
/// (`assert_texel_oracle`).
#[test]
fn campaign_launch_uses_the_player_colours_from_options() {
    const BASE: u32 = 0xff11_2233;
    const HIGHLIGHT: u32 = 0xff44_5566;
    let Some((mut run, frame)) = campaign_run(BASE, HIGHLIGHT, "campaign_launch_painted.png")
    else {
        return;
    };
    let data = data().unwrap();
    assert_texel_oracle(&mut run, &data, &frame, BASE, HIGHLIGHT, "campaign launch");
}

#[test]
fn the_options_screen_opens_from_the_menu_and_cancel_returns() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    let at = host(&mut run).menu_button(MM_MSG_OPTIONS).unwrap();
    click(&mut run, at);
    frames(&mut run, 3);
    assert_eq!(screen(&mut run), ActiveScreen::Options);
    // the audio tab reports its volumes every frame: they reach the cue sink
    assert!(!cues.borrow().iter().any(|c| matches!(c, UiCue::Volumes(_))));
    host(&mut run).select_options_tab(1);
    frames(&mut run, 2);
    assert!(cues.borrow().iter().any(|c| matches!(c, UiCue::Volumes(_))));
    // CANCEL closes the options and the menu is back
    let at = host(&mut run).options_button(NO).expect("cancel button");
    click(&mut run, at);
    frames(&mut run, 3);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
}

#[test]
fn mechlopedia_enters_from_the_menu_and_exit_returns_to_it() {
    let Some((mut run, _)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);

    let at = host(&mut run)
        .menu_button(MM_MSG_ENCYCLOPEDIA)
        .expect("encyclopedia menu button");
    click(&mut run, at);
    assert_eq!(screen(&mut run), ActiveScreen::Mechlopedia);

    let at = host(&mut run)
        .mechlopedia_button(136)
        .expect("Mechlopedia exit button");
    click(&mut run, at);
    frames(&mut run, 2);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
}

#[test]
fn three_screens_render_through_the_production_path() {
    let Some((mut run, _)) = session() else {
        return;
    };
    let screenshots = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR")
        .join(format!("mc2-ui-host-shots-{}", std::process::id()));
    std::fs::create_dir_all(&screenshots).unwrap();
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    pointer(&mut run, (400.0, 300.0));
    run.step(Some(&screenshots.join("ui_host_main_menu_1024x768.png")))
        .unwrap();

    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    frames(&mut run, 40);
    assert!(host(&mut run)
        .notices()
        .iter()
        .all(|notice| !notice.contains("mission movie playback is not available yet")));
    run.step(Some(
        &screenshots.join("ui_host_mission_selection_1024x768.png"),
    ))
    .unwrap();

    let at = host(&mut run)
        .page_button(slot::SELECT, ids::MB_MSG_NEXT)
        .unwrap();
    click(&mut run, at);
    frames(&mut run, 40);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::BRIEFING));
    run.step(Some(&screenshots.join("ui_host_briefing_1024x768.png")))
        .unwrap();

    for p in &run.screenshots {
        shrink(p);
        let len = std::fs::metadata(p).unwrap().len();
        assert!(len < 600 * 1024, "{}: {len} bytes", p.display());
    }
    assert_eq!(run.screenshots.len(), 3);
    std::fs::remove_dir_all(screenshots).unwrap();
}

#[test]
fn a_failed_mission_load_is_reported_and_the_loading_screen_never_completes() {
    let Some((mut run, cues)) = session_with(Box::new(|_| Err("no driver".to_string()))) else {
        return;
    };
    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    // the core tries the load in the frame the loading screen asks for it; the headless step
    // reports the failure
    let mut reported = None;
    for _ in 0..600 {
        if let Err(e) = run.step(None) {
            reported = Some(e);
            break;
        }
        assert_ne!(
            screen(&mut run),
            ActiveScreen::None,
            "the loading screen did not finish before the load failed"
        );
    }
    let reported = reported.expect("the failed load is reported");
    assert!(reported.contains("no driver"), "{reported}");
    assert_eq!(run.core.state(), &GameState::MainMenu);
    assert!(
        run.core.mission.is_none(),
        "no mission was started by the failed load"
    );
    frames(&mut run, 2);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    let banner = host(&mut run)
        .banner()
        .expect("the player is told")
        .to_string();
    assert!(
        banner.contains("could not be loaded") && banner.contains("no driver"),
        "{banner}"
    );
    assert_banner_drawn(&mut run, "load_failed");
}

#[test]
fn accepted_options_reach_the_running_game() {
    let Some((mut run, _)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    let at = host(&mut run).menu_button(MM_MSG_OPTIONS).unwrap();
    click(&mut run, at);
    frames(&mut run, 3);
    let wanted = host(&mut run).prefs().unwrap().use_shadows;
    run.core.config.shadows = !wanted;
    let at = host(&mut run)
        .options_button(mc2_ui::screens::dialog::YES)
        .expect("accept button");
    click(&mut run, at);
    frames(&mut run, 3);
    assert_eq!(run.core.config.shadows, wanted, "the core took the option");
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
}

#[test]
fn a_campaign_win_without_driver_outcome_is_explained_and_returns_to_menu() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    until(&mut run, 600, "the doors out", |r| {
        r.core.state() == &GameState::InMission && host(r).screen() == ActiveScreen::None
    });
    assert!(run
        .core
        .states
        .request(mc2_app::state::StateRequest::EndMission(5)));
    UiLayer::mission_ended(host(&mut run), 5, None);
    frames(&mut run, 2);
    assert_eq!(run.core.state(), &GameState::MainMenu);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    assert!(
        host(&mut run)
            .notices()
            .iter()
            .any(|notice| notice.contains("no outcome")),
        "the host must report the missing driver outcome"
    );
    for _ in 0..300 {
        if host(&mut run)
            .banner()
            .is_some_and(|banner| banner.contains("no outcome"))
        {
            break;
        }
        frames(&mut run, 1);
    }
    assert!(host(&mut run).banner().unwrap().contains("no outcome"));
    assert_banner_drawn(&mut run, "outcome");
}

/// A direct launch paints the player mechs with the options.cfg colours (base 0xfae525, highlight
/// 0x07083e; `code/mission.cpp:1077-1081`).  One base and one highlight texel of a player mech's
/// mask texture are located in the raw texture and on screen, and the rendered pixel is compared
/// with the hand-computed recolour (`assert_texel_oracle`).
#[test]
fn direct_launch_uses_options_cfg_colors() {
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = SIZE;
    opts.game.cursor = false;
    opts.game.shadows = false;
    opts.game.interpolate_camera = false;
    let Some(mut run) = viewer(&opts) else { return };
    until(&mut run, 600, "the mission", |r| {
        r.core.state() == &GameState::InMission
    });
    let view = &run.core.mission.as_ref().unwrap().view;
    assert_eq!(
        view.home_paint,
        PaintSource::Scheme {
            base: OPTIONS_BASE,
            highlight: OPTIONS_HIGHLIGHT
        },
        "the paint source of this launch"
    );
    assert!(
        view.report.unpainted.is_empty(),
        "no unpainted mechs: {:?}",
        view.report.unpainted
    );
    let frame = paint_frame(&mut run, "direct_launch_painted.png");
    assert_texel_oracle(
        &mut run,
        &data,
        &frame,
        OPTIONS_BASE,
        OPTIONS_HIGHLIGHT,
        "direct launch",
    );
}

/// Skips the intro and opens the options screen from the main menu.
fn open_options<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) {
    frames(run, 2);
    skip_menu_intro(run);
    frames(run, 90);
    let at = host(run).menu_button(MM_MSG_OPTIONS).unwrap();
    click(run, at);
    frames(run, 3);
    assert_eq!(screen(run), ActiveScreen::Options);
}

/// Clicks a check box or radio button of the shown options tab.
fn click_control<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, id: i32) {
    let at = host(run)
        .options_tab_control(id)
        .unwrap_or_else(|| panic!("control {id} on the shown tab"));
    click(run, at);
    frames(run, 2);
}

fn click_tab<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, tab: i32) {
    let at = host(run)
        .options_button(mc2_ui::screens::options::MSB_TAB0 + tab)
        .expect("tab button");
    click(run, at);
    frames(run, 2);
}

fn press_yes<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) {
    let at = host(run)
        .options_button(mc2_ui::screens::dialog::YES)
        .expect("accept button");
    click(run, at);
    frames(run, 3);
}

/// Drags the pointer 60 pixels across the middle of the frame and reports how far the mission
/// camera turned.
fn turn_by_drag(
    run: &mut HeadlessRun<NullSim>,
    hold: impl Fn(&mut HeadlessRun<NullSim>, bool),
) -> f32 {
    let centre = (SIZE.0 as f64 / 2.0, SIZE.1 as f64 / 2.0);
    let rotation = |r: &HeadlessRun<NullSim>| r.core.mission.as_ref().unwrap().rig.rotation;
    run.event(InputEvent::CursorMoved {
        x: centre.0,
        y: centre.1,
    });
    frames(run, 4);
    let start = rotation(run);
    hold(run, true);
    frames(run, 2);
    for i in 1..=6 {
        run.event(InputEvent::CursorMoved {
            x: centre.0 + f64::from(i) * 10.0,
            y: centre.1,
        });
        frames(run, 1);
    }
    hold(run, false);
    frames(run, 2);
    let turned = rotation(run) - start;
    run.event(InputEvent::CursorMoved {
        x: centre.0,
        y: centre.1,
    });
    frames(run, 4);
    turned
}

#[test]
fn shadows_and_mouse_options_reach_the_render_setting_and_the_mission() {
    let Some((mut run, cues)) = session() else {
        return;
    };
    open_options(&mut run);
    let before = host(&mut run).prefs().unwrap().clone();
    // the saved options are in force before Options has been accepted
    assert_eq!(run.core.config.shadows, before.use_shadows, "saved shadows");
    assert_eq!(
        run.core.prefs.left_right_mouse_profile, before.use_left_right_mouse_profile,
        "saved mouse profile"
    );
    // graphics tab: shadows
    click_control(&mut run, mc2_ui::screens::options::MSG_SHADOWS);
    // game play tab: the mouse profile
    click_tab(&mut run, 2);
    assert_eq!(host(&mut run).screen(), ActiveScreen::Options);
    click_control(&mut run, mc2_ui::screens::options::MSG_LEFT_CLICK);
    let edited = host(&mut run).options_edited_prefs().unwrap();
    assert_eq!(edited.use_shadows, !before.use_shadows);
    assert_eq!(
        edited.use_left_right_mouse_profile,
        !before.use_left_right_mouse_profile
    );
    press_yes(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    assert_eq!(
        run.core.config.shadows, !before.use_shadows,
        "render setting"
    );
    let profile = !before.use_left_right_mouse_profile;
    assert_eq!(run.core.prefs.left_right_mouse_profile, profile);
    assert_eq!(
        host(&mut run).prefs().unwrap().use_shadows,
        !before.use_shadows
    );

    // the mission uses it: the profile's camera button turns the camera, the other one does not
    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    until(&mut run, 600, "the doors out", |r| {
        r.core.state() == &GameState::InMission && host(r).screen() == ActiveScreen::None
    });
    let right = |r: &mut HeadlessRun<NullSim>, down: bool| {
        r.event(InputEvent::Button {
            button: MouseButton::Right,
            pressed: down,
        })
    };
    let alt = |r: &mut HeadlessRun<NullSim>, down: bool| {
        r.event(mc2_ui::harness::events::key(keys::KEY_LMENU, down, None))
    };
    let by_right = turn_by_drag(&mut run, right);
    let by_alt = turn_by_drag(&mut run, alt);
    if profile {
        assert!(
            by_alt.abs() > 0.0 && by_right == 0.0,
            "alt {by_alt}, right {by_right}"
        );
    } else {
        assert!(
            by_right.abs() > 0.0 && by_alt == 0.0,
            "alt {by_alt}, right {by_right}"
        );
    }
}

/// A private copy of the game data root (hard links, so it costs no space) with room to put
/// broken files over the shared data; the shared `MC2_DATA` is never written.
fn fixture(name: &str) -> Option<PathBuf> {
    fn link(from: &std::path::Path, to: &std::path::Path) {
        if from.is_dir() {
            std::fs::create_dir_all(to).unwrap();
            for e in std::fs::read_dir(from).unwrap() {
                let e = e.unwrap();
                link(&e.path(), &to.join(e.file_name()));
            }
        } else if std::fs::hard_link(from, to).is_err() {
            std::fs::copy(from, to).unwrap();
        }
    }
    let src = data()?;
    let dst = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::create_dir_all(&dst).unwrap();
    for e in std::fs::read_dir(&src).unwrap() {
        let e = e.unwrap();
        // the loose sound and video trees are large and these screens do not read them
        if e.file_name() == "data" || e.file_name() == "mc2" {
            continue;
        }
        link(&e.path(), &dst.join(e.file_name()));
    }
    // Keep the two menu and first-group movies available in fixtures that omit the large loose
    // data trees, so their tests only exercise the asset they intentionally break.
    let movie_dir = dst.join("data/movies");
    std::fs::create_dir_all(&movie_dir).unwrap();
    for movie in ["msft.mpg", "cinema1.mpg"] {
        let original = src.join("data/movies").join(movie);
        let fixture_movie = movie_dir.join(movie);
        if std::fs::hard_link(&original, &fixture_movie).is_err() {
            std::fs::copy(original, fixture_movie).unwrap();
        }
    }
    // the campaign data reads the vehicles and buildings of the object packet (the Mechlopedia)
    let objects = dst.join("data/objects");
    std::fs::create_dir_all(&objects).unwrap();
    let packet = src.join("data/objects/object2.pak");
    if std::fs::hard_link(&packet, objects.join("object2.pak")).is_err() {
        std::fs::copy(&packet, objects.join("object2.pak")).unwrap();
    }
    Some(dst)
}

/// Steps `n` frames; the errors the core reports on the way are returned.
fn frames_reporting(run: &mut HeadlessRun<NullSim>, n: usize) -> Vec<String> {
    (0..n).filter_map(|_| run.step(None).err()).collect()
}

fn break_file(root: &std::path::Path, name: &str) {
    let path = root.join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, [0xff, 0xfe, 0x00, b'[', 0x80, 0x81, 0x00]).unwrap();
}

#[test]
fn a_broken_page_asset_fails_the_campaign_start_with_a_banner() {
    let Some(root) = fixture("fixture_page_asset") else {
        return;
    };
    let Some((mut run, _)) = session_in(root.clone(), Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    // the pages read their layouts when the campaign starts
    break_file(&root, "data/art/mcl_mn.fit");
    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    frames(&mut run, 8);
    pointer(&mut run, at);
    frames(&mut run, 1);
    button(&mut run, true);
    frames(&mut run, 1);
    button(&mut run, false);
    until(&mut run, 60, "campaign list", |r| {
        screen(r) == ActiveScreen::CampaignPick
    });
    // the chosen campaign starts here, so the click's own frames report the fault
    frames(&mut run, 60);
    let first_row = host(&mut run).campaign_pick_row_centre(0).unwrap();
    pointer(&mut run, first_row);
    let mut reported = frames_reporting(&mut run, 1);
    button(&mut run, true);
    reported.extend(frames_reporting(&mut run, 1));
    button(&mut run, false);
    // the click selected the row; Enter starts it
    for pressed in [true, false] {
        run.event(InputEvent::Key {
            key: keys::KEY_RETURN,
            phys_id: keys::KEY_RETURN as u16,
            pressed,
            repeat: false,
            text: None,
        });
        reported.extend(frames_reporting(&mut run, 1));
    }
    reported.extend(frames_reporting(&mut run, 30));
    assert_eq!(run.core.state(), &GameState::MainMenu);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    let banner = host(&mut run)
        .banner()
        .expect("the player is told")
        .to_string();
    assert!(banner.contains("could not be started"), "{banner}");
    reported.retain(|e| e.contains("bad signature"));
    assert!(!reported.is_empty(), "the core reports the fault");
    assert_banner_drawn(&mut run, "page_asset");
}

#[test]
fn a_broken_campaign_dialog_layout_starts_no_campaign_and_keeps_the_menu() {
    let Some(root) = fixture("fixture_campaign_dialog") else {
        return;
    };
    let Some((mut run, _)) = session_in(root.clone(), Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    break_file(&root, "data/art/mcl_sm.fit");
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    frames(&mut run, 120);
    assert_ne!(screen(&mut run), ActiveScreen::CampaignPick);
    assert_eq!(
        run.core.state(),
        &GameState::MainMenu,
        "no campaign starts behind the player's back"
    );
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    // the menu takes input again: the New Campaign button answers a second click
    frames(&mut run, 8);
    let again = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN);
    assert!(again.is_some(), "the menu is still usable");
}

#[test]
fn a_broken_mission_script_fails_the_sequence_with_a_banner() {
    let Some(root) = fixture("fixture_mission_script") else {
        return;
    };
    let Some((mut run, _)) = session_in(root.clone(), Box::new(|_| Ok(NullSim::default()))) else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    // the campaign is running; the briefing reads the mission script when it begins
    break_file(&root, "data/missions/mc2_01.fit");
    let at = host(&mut run)
        .page_button(slot::SELECT, ids::MB_MSG_NEXT)
        .unwrap();
    frames(&mut run, 8);
    pointer(&mut run, at);
    frames(&mut run, 1);
    button(&mut run, true);
    frames(&mut run, 1);
    button(&mut run, false);
    let mut reported = frames_reporting(&mut run, 30);
    assert_eq!(run.core.state(), &GameState::MainMenu);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    let banner = host(&mut run)
        .banner()
        .expect("the player is told")
        .to_string();
    assert!(banner.contains("campaign screens failed"), "{banner}");
    reported.retain(|e| e.contains("bad signature"));
    assert!(!reported.is_empty(), "the core reports the fault");
    assert_banner_drawn(&mut run, "mission_script");
}

/// A session with no cue sink: the cues go through the core to its own `AppAudio`.
fn session_core_audio() -> Option<HeadlessRun<NullSim>> {
    isolate_user_dir();
    let mut opts = HeadlessOptions::new(data()?, "");
    opts.size = SIZE;
    match HeadlessRun::with_driver(&opts, Box::new(|_| Ok(NullSim::default()))) {
        Ok(mut r) => {
            // the player's own `options.cfg` (volumes already at the slider's end) must not decide these tests
            host(&mut r).set_config_directory(fresh_config_dir());
            Some(r)
        }
        Err(e) if skips_without_adapter(&e, std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()) => {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

#[test]
fn gpu_opt_out_needs_exactly_one() {
    use std::ffi::OsStr;
    let adapter = "no usable GPU adapter";
    assert!(skips_without_adapter(adapter, Some(OsStr::new("1"))));
    for v in ["", "0", "true", "yes", " 1", "1 ", "11"] {
        assert!(
            !skips_without_adapter(adapter, Some(OsStr::new(v))),
            "{v:?} must not skip"
        );
    }
    assert!(!skips_without_adapter(adapter, None), "unset must not skip");
    // Any other setup failure is never skipped, even with the opt-out.
    assert!(!skips_without_adapter(
        "bad signature",
        Some(OsStr::new("1"))
    ));
}

fn live_volumes(run: &HeadlessRun<NullSim>) -> [i32; 5] {
    let v = run
        .core
        .audio
        .as_ref()
        .expect("the core has a game sound")
        .game()
        .sound
        .sys
        .volumes();
    [v.digital_master, v.music, v.sfx, v.radio, v.betty].map(i32::from)
}

fn pref_volumes(p: &mc2_ui::screens::Prefs) -> [i32; 5] {
    [
        p.digital_master_volume,
        p.music_volume,
        p.sfx_volume,
        p.radio_volume,
        p.betty_volume,
    ]
}

#[test]
fn accepted_volumes_match_the_live_mixer_through_the_core_audio() {
    let Some(mut run) = session_core_audio() else {
        return;
    };
    assert!(run.core.audio.is_some(), "headless installs a game sound");
    open_options(&mut run);
    // audio is there: no unavailable banner
    assert!(
        !host(&mut run)
            .notices()
            .iter()
            .any(|n| n.contains("Audio is not available")),
        "{:?}",
        host(&mut run).notices()
    );
    let before = host(&mut run).prefs().unwrap().clone();
    click_tab(&mut run, 1);
    // Exercise both controls: music steps down, and SFX steps right from below its maximum.
    for _ in 0..4 {
        let at = host(&mut run)
            .options_volume_decrease(1)
            .expect("music slider step");
        click(&mut run, at);
        frames(&mut run, 2);
    }
    for _ in 0..4 {
        let at = host(&mut run)
            .options_volume_step(2)
            .expect("SFX slider step");
        click(&mut run, at);
        frames(&mut run, 2);
    }
    let edited = host(&mut run).options_edited_prefs().unwrap();
    assert_ne!(
        pref_volumes(&edited),
        pref_volumes(&before),
        "the slider moved"
    );
    press_yes(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    frames(&mut run, 3);
    let kept = host(&mut run).prefs().unwrap().clone();
    assert_eq!(pref_volumes(&kept), pref_volumes(&edited), "accepted");
    assert_eq!(live_volumes(&run), pref_volumes(&kept), "live mixer");
    assert!(
        !host(&mut run)
            .notices()
            .iter()
            .any(|n| n.contains("Volumes")),
        "{:?}",
        host(&mut run).notices()
    );
}

#[test]
fn with_no_game_sound_at_all_volumes_are_refused_and_the_player_is_told() {
    let Some(mut run) = session_core_audio() else {
        return;
    };
    // genuinely no audio: neither a cue sink nor an `AppAudio`
    run.core.audio = None;
    open_options(&mut run);
    assert!(
        host(&mut run)
            .notices()
            .iter()
            .any(|n| n.contains("Audio is not available")),
        "{:?}",
        host(&mut run).notices()
    );
    let before = host(&mut run).prefs().unwrap().clone();
    click_tab(&mut run, 1);
    for _ in 0..4 {
        let at = host(&mut run).options_volume_decrease(1).unwrap();
        click(&mut run, at);
        frames(&mut run, 2);
    }
    for _ in 0..4 {
        let at = host(&mut run).options_volume_step(2).unwrap();
        click(&mut run, at);
        frames(&mut run, 2);
    }
    assert_ne!(
        pref_volumes(&host(&mut run).options_edited_prefs().unwrap()),
        pref_volumes(&before)
    );
    press_yes(&mut run);
    assert_eq!(
        pref_volumes(host(&mut run).prefs().unwrap()),
        pref_volumes(&before),
        "old volumes kept"
    );
    assert!(host(&mut run)
        .notices()
        .iter()
        .any(|n| n.contains("Volumes")));
    assert_banner_drawn(&mut run, "volumes_refused");
}

#[test]
fn solo_missions_lists_the_single_player_maps_and_launches_the_chosen_one() {
    use mc2_ui::screens::main_menu::MM_MSG_SINGLE_MISSION;
    let Some((mut run, cues)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);

    let at = host(&mut run)
        .menu_button(MM_MSG_SINGLE_MISSION)
        .expect("the single mission button");
    click(&mut run, at);
    until(&mut run, 120, "the solo mission list", |r| {
        screen(r) == ActiveScreen::SoloMissions
    });
    let files = host(&mut run).solo_mission_files().expect("list is open");
    let names = host(&mut run).solo_mission_names().expect("list is open");
    assert!(
        files.len() >= 3,
        "the shipped game has single-player maps: {files:?}"
    );
    assert!(names.iter().all(|n| !n.is_empty()));
    // the campaign's maps are there, the multiplayer ones (`IsSinglePlayer = 0`) are not
    for wanted in ["mc2_01", "mc2_24", "tut_01"] {
        assert!(files.iter().any(|f| f == wanted), "{wanted} is listed");
    }
    assert!(!files.iter().any(|f| f == "mc2_m01"), "no multiplayer map");
    // every listed map really is a single-player map of the mission directory
    let mut vfs = Vfs::from_system_cfg(data().unwrap()).expect("vfs");
    for file in &files {
        let mut fit = vfs
            .read_fit(&format!("data/missions/{file}.fit"))
            .expect("read")
            .unwrap_or_else(|| panic!("{file} is a mission file"));
        fit.seek_block("MissionSettings").expect("settings");
        assert_ne!(
            fit.read_u32("IsSinglePlayer").unwrap_or(0),
            0,
            "{file} is a single-player map"
        );
    }

    // No returns to the menu
    let no = host(&mut run).solo_mission_button(NO).expect("No");
    click(&mut run, no);
    until(&mut run, 60, "the menu", |r| {
        screen(r) == ActiveScreen::MainMenu
    });
    let at = host(&mut run).menu_button(MM_MSG_SINGLE_MISSION).unwrap();
    click(&mut run, at);
    until(&mut run, 120, "the solo mission list", |r| {
        screen(r) == ActiveScreen::SoloMissions
    });

    // pick the second map by clicking its row, then Yes
    let row = host(&mut run).solo_mission_row(1).expect("second row");
    click(&mut run, row);
    let chosen = files[1].clone();
    let yes = host(&mut run).solo_mission_button(7).expect("Yes");
    click(&mut run, yes);
    until(&mut run, 120, "the briefing", |r| {
        r.core.state() == &GameState::Logistics
    });
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::BRIEFING));
    {
        let lg = host(&mut run).campaign().expect("a one-mission game");
        assert!(lg.mission_info().is_single_mission());
        assert_eq!(lg.current_mission(), chosen);
    }

    // into the mission through the existing path (a purchase stands in for the purchase screen)
    {
        let lg = host(&mut run).campaign_mut().unwrap();
        let mut variants = lg.purchasable_mechs();
        variants.sort_by_key(|v| lg.variant_cost(*v));
        lg.add_c_bills(200_000);
        lg.purchase_mech(variants[0]).expect("purchase");
    }
    brief_deploy_and_crew(&mut run, &cues);
    start_loading(&mut run);
    until(&mut run, 600, "the mission", |r| {
        r.core.state() == &GameState::InMission
    });
    assert_eq!(run.core.mission.as_ref().unwrap().name, chosen);
    let spec = host(&mut run)
        .take_launch()
        .expect("the launch spec was kept");
    assert_eq!(spec.mission, chosen);
}

#[test]
fn return_to_game_from_the_menu_goes_back_to_the_logistics_page() {
    use mc2_ui::screens::main_menu::{MM_MSG_NEW_CAMPAIGN, MM_MSG_RETURN_TO_GAME};
    let Some((mut run, _cues)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::SELECT));

    // the logistics pages' Main Menu button opens the menu without a background; RETURN TO GAME
    // is enabled there and goes back to the page that opened it
    return_to_main_menu(&mut run);
    until(&mut run, 120, "the menu", |r| {
        screen(r) == ActiveScreen::MainMenu
    });
    frames(&mut run, 90);
    let back = host(&mut run)
        .menu_button(MM_MSG_RETURN_TO_GAME)
        .expect("return to game button");
    click(&mut run, back);
    until(&mut run, 120, "the logistics page", |r| {
        matches!(screen(r), ActiveScreen::Logistics(_))
    });
    assert_eq!(run.core.state(), &GameState::Logistics);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::SELECT));
    assert!(host(&mut run).campaign().is_some(), "the campaign goes on");
}

#[test]
fn the_mech_bay_draws_the_3d_picture_of_the_picked_mech() {
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::screens::main_menu::MM_MSG_NEW_CAMPAIGN;
    let Some((mut run, _cues)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    {
        let lg = host(&mut run).campaign_mut().unwrap();
        let mut variants = lg.purchasable_mechs();
        variants.sort_by_key(|v| lg.variant_cost(*v));
        lg.add_c_bills(200_000);
        lg.purchase_mech(variants[0]).expect("purchase");
    }
    let at = host(&mut run)
        .page_button(slot::SELECT, ids::MB_MSG_NEXT)
        .unwrap();
    click(&mut run, at);
    let at = host(&mut run)
        .page_button(slot::BRIEFING, ids::MB_MSG_NEXT)
        .unwrap();
    click(&mut run, at);
    assert_eq!(screen(&mut run), ActiveScreen::Logistics(slot::MECH_BAY));

    // pick the stored mech by clicking its entry; the bay asks for its picture and the host,
    // drawing the page, renders it
    let entry = {
        let bay = host(&mut run)
            .sequence_mut()
            .unwrap()
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        let r = bay.list.entries().next().expect("a stored mech").box_rect();
        (
            (r.left + r.right) as f32 / 2.0,
            (r.top + r.bottom) as f32 / 2.0,
        )
    };
    click(&mut run, entry);
    let request = {
        let bay = host(&mut run)
            .sequence_mut()
            .unwrap()
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        bay.preview.clone().expect("the bay asked for a picture")
    };
    let key = request.key();
    until(&mut run, 120, "the mech picture", |r| {
        host(r).picture_rendered(&key)
    });
}

/// Opens the New Campaign list from the main menu of a fresh session.
fn open_campaign_list(run: &mut HeadlessRun<NullSim>) {
    frames(run, 2);
    skip_menu_intro(run);
    frames(run, 90);
    let at = host(run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(run, at);
    until(run, 60, "campaign list", |r| {
        screen(r) == ActiveScreen::CampaignPick
    });
}

fn press(run: &mut HeadlessRun<NullSim>, key: u32) {
    for pressed in [true, false] {
        run.event(mc2_ui::harness::events::key(key, pressed, None));
        frames(run, 1);
    }
}

/// `LogisticsSaveDialog::update` (`code/logisticsdialog.cpp:807-825`): one click on a campaign row only selects it and updates
/// the mission info; YES or Enter starts the selected campaign; Escape and NO start nothing.
#[test]
fn campaign_list_click_selects_and_yes_or_enter_starts_it() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_campaign_list(&mut run);
    // the panel slides in; the first row is selected on its first update
    frames(&mut run, 60);
    let first = host(&mut run).campaign_pick_selection().unwrap();
    assert_eq!(first.0, "campaign");
    assert!(!first.1.is_empty(), "mission info of the first row");

    // A click on the tutorial row selects it, updates the mission info, and starts nothing.
    frames(&mut run, 60);
    let tutorial = host(&mut run).campaign_pick_row_centre(1).unwrap();
    click(&mut run, tutorial);
    frames(&mut run, 30);
    assert_eq!(screen(&mut run), ActiveScreen::CampaignPick);
    let second = host(&mut run).campaign_pick_selection().unwrap();
    assert_eq!(second.0, "tutorial");
    assert!(!second.1.is_empty());
    assert_ne!(second.1, first.1, "the mission info follows the selection");
    assert!(host(&mut run).campaign().is_none());

    // Escape cancels: nothing starts, and the panel slides out over the menu.
    press(&mut run, keys::KEY_ESCAPE);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    assert!(host(&mut run).campaign_pick_leaving());
    frames(&mut run, 60);
    assert!(!host(&mut run).campaign_pick_leaving());
    assert!(host(&mut run).campaign().is_none());

    // NO cancels too.
    frames(&mut run, 90);
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    until(&mut run, 60, "campaign list", |r| {
        screen(r) == ActiveScreen::CampaignPick
    });
    frames(&mut run, 60);
    let (yes, no) = host(&mut run).campaign_pick_buttons().unwrap();
    click(&mut run, no);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    assert!(host(&mut run).campaign().is_none());

    // YES starts the selected row (the tutorial).
    frames(&mut run, 90);
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    until(&mut run, 60, "campaign list", |r| {
        screen(r) == ActiveScreen::CampaignPick
    });
    frames(&mut run, 60);
    let tutorial = host(&mut run).campaign_pick_row_centre(1).unwrap();
    click(&mut run, tutorial);
    assert!(
        host(&mut run).campaign().is_none(),
        "a click does not start"
    );
    click(&mut run, yes);
    until(&mut run, 60, "the tutorial", |r| {
        host(r).campaign().is_some()
    });
    assert_eq!(
        host(&mut run).campaign().unwrap().current_mission(),
        "tut_01"
    );
}

/// Enter starts the selected campaign (the first row by default: the main campaign).
#[test]
fn campaign_list_enter_starts_the_selected_campaign() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_campaign_list(&mut run);
    frames(&mut run, 60);
    let tutorial = host(&mut run).campaign_pick_row_centre(1).unwrap();
    click(&mut run, tutorial);
    assert_eq!(
        host(&mut run).campaign_pick_selection().unwrap().0,
        "tutorial"
    );
    press(&mut run, keys::KEY_RETURN);
    until(&mut run, 60, "the tutorial", |r| {
        host(r).campaign().is_some()
    });
    assert_eq!(
        host(&mut run).campaign().unwrap().current_mission(),
        "tut_01"
    );
}

// ---------------------------------------------------------------------------------------------
// results presentation (gap-audit row 14): the slide-ins, the promotion pop-up with its skill
// icons, the kill markers as unit icons, the salvage panel with its 3D view and row icons

/// A won mission whose outcome is whatever the test put in `outcome`.
struct RichWin {
    frames: u64,
    outcome: Rc<RefCell<Option<Outcome>>>,
}

impl SimDriver for RichWin {
    type Command = ();
    type Event = ();
    type World = ();

    fn step(&mut self, _input: FrameInput<()>) -> FrameOutput<()> {
        self.frames += 1;
        FrameOutput {
            events: Vec::new(),
            result: if self.frames >= 8 {
                mc2_sim::mission::result::WIN_BIG
            } else {
                mc2_sim::mission::result::PLAYING
            },
        }
    }

    fn world(&self) -> &() {
        &()
    }

    fn mission_outcome(&mut self) -> Option<Outcome> {
        self.outcome.borrow_mut().take()
    }
}

fn visual_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// How many pixels of the UI rectangle `r` (800x600 space) a frame draws lit (any channel over 40).
fn lit(img: &(u32, u32, Vec<u8>), r: (f32, f32, f32, f32)) -> usize {
    let (w, h, px) = img;
    let (l, t) = ((r.0 * SCALE) as u32, (r.1 * SCALE) as u32);
    let (rr, b) = (
        ((r.2 * SCALE) as u32).min(*w),
        ((r.3 * SCALE) as u32).min(*h),
    );
    let mut n = 0;
    for y in t..b {
        for x in l..rr {
            let p = &px[((y * w + x) * 4) as usize..][..3];
            if p.iter().any(|&c| c > 40) {
                n += 1;
            }
        }
    }
    n
}

/// A sum over the pixels of `r`, to tell two pictures apart.
fn signature(img: &(u32, u32, Vec<u8>), r: (f32, f32, f32, f32)) -> u64 {
    let (w, h, px) = img;
    let (l, t) = ((r.0 * SCALE) as u32, (r.1 * SCALE) as u32);
    let (rr, b) = (
        ((r.2 * SCALE) as u32).min(*w),
        ((r.3 * SCALE) as u32).min(*h),
    );
    let mut sum = 0u64;
    for y in t..b {
        for x in l..rr {
            for (i, &c) in px[((y * w + x) * 4) as usize..][..3].iter().enumerate() {
                sum = sum
                    .wrapping_add(u64::from(c) * (1 + (x + 3 * y) as u64 % 251) * (i as u64 + 1));
            }
        }
    }
    sum
}

/// The leftmost column holding a lit pixel.
fn leftmost_lit(img: &(u32, u32, Vec<u8>)) -> u32 {
    let (w, h, px) = img;
    (0..*w)
        .find(|&x| {
            (0..*h).any(|y| {
                px[((y * w + x) * 4) as usize..][..3]
                    .iter()
                    .any(|&c| c > 40)
            })
        })
        .unwrap_or(*w)
}

/// Steps a frame, saving it as `<name>.png` under `visual/` and returning its pixels.
fn shot<D: SimDriver + 'static>(run: &mut HeadlessRun<D>, name: &str) -> Shot {
    let path = visual_dir().join(format!("{name}.png"));
    run.step(Some(&path)).unwrap();
    let img = read_png(&path);
    (path, img)
}

/// The frames as one strip, each shrunk to a quarter, left to right.
type Shot = (PathBuf, (u32, u32, Vec<u8>));

fn write_strip(name: &str, frames: &[Shot]) {
    let (w, h) = (frames[0].1 .0 as usize / 4, frames[0].1 .1 as usize / 4);
    let mut out = vec![0u8; w * frames.len() * h * 3];
    for (n, (_, (fw, _, px))) in frames.iter().enumerate() {
        for y in 0..h {
            for x in 0..w {
                let src = ((y * 4 * *fw as usize) + x * 4) * 4;
                let dst = (y * w * frames.len() + n * w + x) * 3;
                out[dst..dst + 3].copy_from_slice(&px[src..src + 3]);
            }
        }
    }
    let path = visual_dir().join(format!("{name}.png"));
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(path).unwrap()),
        (w * frames.len()) as u32,
        h as u32,
    );
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&out).unwrap();
}

#[test]
fn results_screens_slide_in_and_show_icons_through_the_production_host() {
    use mc2_campaign::interface::{MechSummary, MissionResult, MoverReport};
    use mc2_campaign::pilot::{KilledKind, KilledUnit, PilotReport, WarriorStatus};
    use mc2_campaign::results::ResultsStage;
    use mc2_ui::results_salvage::SalvageRow;
    use mc2_ui::widgets::ListItem;

    let Some(root) = data() else { return };
    let slot_outcome: Rc<RefCell<Option<Outcome>>> = Rc::default();
    let for_driver = Rc::clone(&slot_outcome);
    let mut opts = HeadlessOptions::new(root, "");
    opts.size = SIZE;
    let (mut run, cues) = match HeadlessRun::with_driver(
        &opts,
        Box::new(move |_| {
            Ok(RichWin {
                frames: 0,
                outcome: Rc::clone(&for_driver),
            })
        }),
    ) {
        Ok(mut run) => {
            let cues = Cues::default();
            host(&mut run).set_cue_sink(Box::new(Log(cues.clone())));
            (run, cues)
        }
        Err(e) if skips_without_adapter(&e, std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()) => {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    play_to_pilot_ready(&mut run, &cues);
    start_loading(&mut run);
    until(&mut run, 90, "mission start", |r| {
        r.core.state() == &GameState::InMission
    });

    // the campaign as the mission leaves it: the crewed mech's pilot is Dagger (promoted by a
    // mission of 54/52), a second pilot dies, two wrecks are on the field
    let (outcome, dagger) = {
        let lg = host(&mut run).campaign_mut().unwrap();
        lg.add_c_bills(10_000_000);
        // the first mission is won without a salvage screen: the mission that ends here is the
        // second one
        lg.apply_outcome(&Outcome::won())
            .expect("the first mission");
        let designs = lg.purchasable_mechs();
        let dagger = lg
            .pilot_index("pmwdagger")
            .expect("the pilot is in the roster");
        let mech = lg.purchase_mech(designs[0]).expect("a mech");
        lg.add_mech_to_force_group(mech, 1).expect("force group");
        lg.set_mech_pilot(mech, Some(dagger));
        let second = lg.purchase_mech(designs[0]).expect("a second mech");
        let other = (0..lg.pilots().len())
            .find(|&i| i != dagger && lg.pilots()[i].available)
            .expect("a second pilot");
        lg.add_mech_to_force_group(second, 2).expect("force group");
        lg.set_mech_pilot(second, Some(other));
        let name = |lg: &mc2_campaign::Logistics, m| {
            let v = lg.mech(m).expect("mech").variant;
            lg.variant(v).expect("variant").name.clone()
        };
        let mover = |variant: String, file: String, pilot: PilotReport, dead: bool| MoverReport {
            on_gui: true,
            is_battle_mech: true,
            home_commander: true,
            is_air: false,
            destroyed: dead,
            disabled: false,
            variant_name: variant,
            pilot_file: Some(file),
            paint_scheme: [0; 3],
            pilot,
        };
        let wreck = |lg: &mc2_campaign::Logistics, v, paint| MechSummary {
            paint_scheme: paint,
            ..MechSummary::wreck(&lg.variant(v).expect("variant").name, 50.0)
        };
        let outcome = Outcome {
            result: MissionResult::PlayerWinBig,
            movers: vec![
                mover(
                    name(lg, mech),
                    lg.pilots()[dagger].file_name.clone(),
                    PilotReport {
                        gunnery_rank: 50.0,
                        piloting_rank: 48.0,
                        gunnery_points: 100.0,
                        piloting_points: 400.0,
                        killed: vec![
                            KilledKind::BattleMech { air: false },
                            KilledKind::BattleMech { air: false },
                            KilledKind::GroundVehicle,
                            KilledKind::BattleMech { air: false },
                        ],
                        // the first mech is shot to pieces, the second is untouched
                        killed_units: vec![
                            KilledUnit {
                                picture: 3,
                                armor: [(0.0, 10.0, true); 11],
                            },
                            KilledUnit {
                                picture: 9,
                                armor: [(10.0, 10.0, false); 11],
                            },
                            KilledUnit {
                                picture: 2,
                                armor: [(10.0, 10.0, false); 11],
                            },
                            KilledUnit {
                                picture: 14,
                                armor: [(10.0, 10.0, false); 11],
                            },
                        ],
                        ..PilotReport::default()
                    },
                    false,
                ),
                mover(
                    name(lg, second),
                    lg.pilots()[other].file_name.clone(),
                    PilotReport {
                        status: WarriorStatus::Dead,
                        ..PilotReport::default()
                    },
                    true,
                ),
            ],
            mechs: vec![
                // the first wreck is shot to pieces (armour gone, every location destroyed), the
                // second is untouched
                MechSummary {
                    armor: [(0.0, 10.0, true); 11],
                    ..wreck(lg, designs[1], [0x00c0_2020, 0x0020_c020, 0x0020_20c0])
                },
                MechSummary {
                    armor: [(10.0, 10.0, false); 11],
                    ..wreck(lg, designs[2], [0; 3])
                },
            ],
            ..Outcome::won()
        };
        (outcome, dagger)
    };
    *slot_outcome.borrow_mut() = Some(outcome);
    until(&mut run, 90, "winning mission result", |r| {
        matches!(r.core.state(), GameState::Results(_))
    });
    frames(&mut run, 2);
    assert_eq!(screen(&mut run), ActiveScreen::Results);

    // ---- the salvage screen: row icons, the selected mech's panel and its 3D view
    // the screen fades in from black over one second (`beginFadeIn(1.0)`)
    let mut fade = vec![shot(&mut run, "results_salvage_fade_00")];
    for n in 1..4 {
        frames(&mut run, 12);
        fade.push(shot(&mut run, &format!("results_salvage_fade_{n:02}")));
    }
    frames(&mut run, 90);
    let (_, picked) = shot(&mut run, "results_salvage_first");
    fade.push((PathBuf::new(), picked.clone()));
    {
        let whole = (0.0, 0.0, 4000.0, 4000.0);
        let seen: Vec<usize> = fade.iter().map(|(_, img)| lit(img, whole)).collect();
        assert!(
            seen.windows(2).all(|w| w[1] >= w[0]) && seen[0] * 2 < seen[seen.len() - 1],
            "the salvage screen fades in from black and settles fully visible: {seen:?}"
        );
        write_strip("results_salvage_fade_strip", &fade);
    }
    {
        let r = host(&mut run).results().expect("results");
        assert_eq!(r.stage(), Some(ResultsStage::Salvage));
        assert_eq!(r.salvage.list.item_count(), 2, "both wrecks are offered");
        for i in 0..2 {
            let row = r
                .salvage
                .list
                .item(i)
                .and_then(|it| it.as_any().downcast_ref::<SalvageRow>())
                .unwrap();
            assert!(
                lit(&picked, row.unit_icon_rect()) > 100,
                "row {i} draws its unit icon"
            );
            let o = row.obj();
            let (l, t) = (o.left(), o.top());
            // the weight and C-Bills icons and the check box
            assert!(
                lit(&picked, (l + 295.0, t + 4.0, l + 310.0, t + 19.0)) > 20,
                "weight icon"
            );
            assert!(
                lit(&picked, (l + 295.0, t + 27.0, l + 310.0, t + 42.0)) > 20,
                "C-Bills icon"
            );
            assert!(
                lit(&picked, (l + 2.0, t, l + 48.0, t + 46.0)) > 100,
                "check box"
            );
        }
        let first = r
            .salvage
            .list
            .item(0)
            .and_then(|it| it.as_any().downcast_ref::<SalvageRow>())
            .unwrap();
        let panel = &r.salvage.panel;
        assert_eq!(
            panel.unit(),
            Some(first.variant),
            "the first row fills the panel"
        );
        assert!(panel.loadout.list.item_count() > 0, "the loadout is listed");
        assert!(
            panel.meter_values().iter().all(|&m| m > 0.0),
            "the meters are set"
        );
        assert!(!panel.text(0).is_empty() && !panel.text(3).is_empty());
        assert!(panel.preview.is_some(), "the 3D view is asked for");
        let inner = (
            panel.camera.0 + 4.0,
            panel.camera.1 + 4.0,
            panel.camera.2 - 4.0,
            panel.camera.3 - 4.0,
        );
        assert!(
            lit(&picked, inner) > 1500,
            "the 3D view draws the mech into its frame"
        );
    }
    // the shot-up wreck's row icon wears damage colours, the untouched one's does not
    {
        let r = host(&mut run).results().unwrap();
        let grey = |row: &SalvageRow| {
            let r = row.unit_icon_rect();
            let (w, _, px) = &picked;
            let mut count = 0;
            for y in (r.1 * SCALE) as u32..(r.3 * SCALE) as u32 {
                for x in (r.0 * SCALE) as u32..(r.2 * SCALE) as u32 {
                    let p = &px[((y * w + x) * 4) as usize..][..3];
                    if p[0] > 40 && p[0].abs_diff(p[1]) < 12 && p[1].abs_diff(p[2]) < 12 {
                        count += 1;
                    }
                }
            }
            count
        };
        let rows: Vec<&SalvageRow> = (0..2)
            .map(|i| {
                r.salvage
                    .list
                    .item(i)
                    .and_then(|it| it.as_any().downcast_ref::<SalvageRow>())
                    .unwrap()
            })
            .collect();
        assert!(
            rows.iter().all(|row| row.icon_image().is_some()),
            "the row icons were composed"
        );
        let counts: Vec<usize> = rows.iter().map(|row| grey(row)).collect();
        assert!(
            counts.iter().max().unwrap() > &40
                && *counts.iter().min().unwrap() < counts.iter().max().unwrap() / 4,
            "one row icon is drawn grey with damage, the other is not: {counts:?}"
        );
    }
    // an unaffordable row clicked: its mech still fills the panel, nothing is bought
    {
        let (dear_row, dear_variant, cheap_cost, dear_cost) = {
            let r = host(&mut run).results().unwrap();
            let flow_offers = r.offers();
            let o = r.salvage.list.item(1).unwrap().obj();
            (
                (o.left() + 150.0, o.top() + o.height() / 2.0),
                flow_offers[1].variant,
                flow_offers[0].cost,
                flow_offers[1].cost,
            )
        };
        assert!(cheap_cost < dear_cost, "the second row is dearer");
        let before = host(&mut run).campaign_mut().unwrap().c_bills();
        let lg = host(&mut run).campaign_mut().unwrap();
        let have = i64::from(dear_cost) - 1;
        lg.decrement_c_bills(before - have);
        frames(&mut run, 3);
        click(&mut run, dear_row);
        frames(&mut run, 10);
        let (_, dear_shot) = shot(&mut run, "results_salvage_unaffordable");
        let r = host(&mut run).results().unwrap();
        let panel = &r.salvage.panel;
        assert_eq!(
            panel.unit(),
            Some(dear_variant),
            "the unaffordable mech is shown"
        );
        assert!(panel.loadout.list.item_count() > 0 && !panel.text(0).is_empty());
        assert_ne!(
            signature(&picked, panel.camera),
            signature(&dear_shot, panel.camera),
            "and its 3D view with it"
        );
        for i in 0..2 {
            let row = r
                .salvage
                .list
                .item(i)
                .and_then(|it| it.as_any().downcast_ref::<SalvageRow>())
                .unwrap();
            assert!(!row.is_checked(), "no row was ticked");
        }
        assert_eq!(host(&mut run).campaign_mut().unwrap().c_bills(), have);
        host(&mut run)
            .campaign_mut()
            .unwrap()
            .add_c_bills(before - have);
    }
    // the second row: another mech in the panel, another picture in the 3D view
    let (second_row, second_variant) = {
        let r = host(&mut run).results().unwrap();
        let row = r
            .salvage
            .list
            .item(1)
            .and_then(|it| it.as_any().downcast_ref::<SalvageRow>())
            .unwrap();
        let o = row.obj();
        ((o.left() + 150.0, o.top() + o.height() / 2.0), row.variant)
    };
    click(&mut run, second_row);
    frames(&mut run, 10);
    let (_, second_shot) = shot(&mut run, "results_salvage_second");
    {
        let panel = &host(&mut run).results().unwrap().salvage.panel;
        assert_eq!(
            panel.unit(),
            Some(second_variant),
            "the clicked row fills the panel"
        );
        let inner = (
            panel.camera.0 + 4.0,
            panel.camera.1 + 4.0,
            panel.camera.2 - 4.0,
            panel.camera.3 - 4.0,
        );
        assert_ne!(
            signature(&picked, inner),
            signature(&second_shot, inner),
            "the 3D view changed with the mech"
        );
    }
    let salvage_done = host(&mut run)
        .results_button(mc2_ui::mission_results::SALVAGE_DONE)
        .expect("salvage Done button");
    click(&mut run, salvage_done);
    until(&mut run, 200, "the pilot review", |r| {
        host(r)
            .results()
            .is_some_and(|r| r.stage() == Some(ResultsStage::PilotReview))
    });

    // ---- the pilot review slides in over 0.7 s
    let mut offsets = Vec::new();
    let mut strip = Vec::new();
    for i in 0..60 {
        let at = host(&mut run).results().unwrap().pilots.slide_offset().0;
        offsets.push(at);
        if i % 8 == 0 {
            strip.push(shot(&mut run, &format!("results_review_slide_{i:02}")));
        } else {
            frames(&mut run, 1);
        }
    }
    assert!(
        offsets[0] > 400,
        "the review starts off to the right: {offsets:?}"
    );
    assert!(
        offsets.windows(2).all(|w| w[1] <= w[0]),
        "it only moves left: {offsets:?}"
    );
    assert_eq!(*offsets.last().unwrap(), 0, "and arrives: {offsets:?}");
    assert!(
        offsets
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            >= 15,
        "over many frames, not a jump: {offsets:?}"
    );
    let lefts: Vec<u32> = strip.iter().map(|(_, img)| leftmost_lit(img)).collect();
    assert!(
        lefts.windows(2).all(|w| w[1] <= w[0]) && lefts[0] > lefts[lefts.len() - 1],
        "the drawn screen moves in from the right: {lefts:?}"
    );
    write_strip("results_review_slide_strip", &strip);

    // ---- the killed pilot's box, then the active pilot's kill markers as unit icons
    let mut dead_shot = None;
    for _ in 0..1500 {
        let shown = {
            let r = host(&mut run).results().unwrap();
            r.pilots
                .list
                .dead_cards()
                .first()
                .is_some_and(|c| c.obj().show)
        };
        if shown {
            frames(&mut run, 12);
            dead_shot = Some(shot(&mut run, "results_review_dead"));
            break;
        }
        frames(&mut run, 1);
    }
    let (_, dead_img) = dead_shot.expect("the killed pilot's box appears");
    {
        let r = host(&mut run).results().unwrap();
        let dead = r.pilots.list.dead_cards();
        assert_eq!(dead.len(), 1);
        assert!(
            lit(&dead_img, dead[0].photo_rect()) > 100,
            "the pilot's photo"
        );
        assert!(lit(&dead_img, dead[0].skull_rect()) > 100, "the skull");
    }
    let mut kills_img = None;
    for _ in 0..2500 {
        let all = {
            let r = host(&mut run).results().unwrap();
            r.pilots
                .list
                .active_cards()
                .first()
                .is_some_and(|c| c.kills_shown() == 4)
        };
        if all {
            frames(&mut run, 4);
            kills_img = Some(shot(&mut run, "results_review_kills"));
            break;
        }
        frames(&mut run, 1);
    }
    let (_, kills_img) = kills_img.expect("the four kill markers appear");
    {
        let r = host(&mut run).results().unwrap();
        let card = r.pilots.list.active_cards()[0];
        let sigs: Vec<u64> = (0..4)
            .map(|n| {
                assert!(
                    lit(&kills_img, card.kill_icon_rect(n)) > 100,
                    "kill marker {n} is a unit icon"
                );
                signature(&kills_img, card.kill_icon_rect(n))
            })
            .collect();
        let distinct: std::collections::BTreeSet<_> = sigs.iter().collect();
        assert_eq!(distinct.len(), 4, "each kill shows its own unit's icon");
        // `MechIcon::update`: a destroyed unit's locations are grey, an intact one's are blue
        let grey = |n: usize| {
            let r = card.kill_icon_rect(n);
            let (w, _, px) = &kills_img;
            let mut count = 0;
            for y in (r.1 * SCALE) as u32..(r.3 * SCALE) as u32 {
                for x in (r.0 * SCALE) as u32..(r.2 * SCALE) as u32 {
                    let p = &px[((y * w + x) * 4) as usize..][..3];
                    // the destroyed colours (`damageColors[3]`) are grey: no channel stands out
                    if p[0] > 40 && p[0].abs_diff(p[1]) < 12 && p[1].abs_diff(p[2]) < 12 {
                        count += 1;
                    }
                }
            }
            count
        };
        assert!(grey(0) > 40, "the shot-up mech is drawn grey: {}", grey(0));
        assert!(
            grey(1) < grey(0) / 4,
            "the intact mech is not: {} {}",
            grey(1),
            grey(0)
        );
        assert!(
            lit(&kills_img, card.photo_rect()) > 100,
            "the card's pilot photo"
        );
    }

    // the Valor medal: its ribbon and its text appear after the kill markers, then the
    // promotion header flashes
    let mut medal_img = None;
    for _ in 0..1200 {
        let n = {
            let r = host(&mut run).results().unwrap();
            r.pilots.list.active_cards()[0].medals_shown()
        };
        if n >= 1 {
            frames(&mut run, 6);
            medal_img = Some(shot(&mut run, "results_review_medal"));
            break;
        }
        frames(&mut run, 1);
    }
    let (_, medal_img) = medal_img.expect("the medal appears");
    {
        let r = host(&mut run).results().unwrap();
        let card = r.pilots.list.active_cards()[0];
        let rect = card.medal_rect(0).expect("a medal");
        assert!(
            lit(
                &medal_img,
                (
                    rect.left as f32,
                    rect.top as f32,
                    rect.right as f32,
                    rect.bottom as f32
                )
            ) > 300,
            "the medal ribbon is drawn"
        );
    }
    let mut header_colours = std::collections::BTreeSet::new();
    for _ in 0..90 {
        frames(&mut run, 1);
        let r = host(&mut run).results().unwrap();
        if let Some(card) = r.pilots.list.active_cards().first() {
            let (text, colour) = card.promotion_header();
            if r.pilots.picker_pilot().is_some() {
                break;
            }
            if colour != 0 {
                assert!(
                    text.contains("REGULAR"),
                    "the promotion header names the rank: {text}"
                );
                header_colours.insert(colour);
            }
        }
    }
    assert!(
        !header_colours.is_empty(),
        "the promotion header flashes in"
    );

    // ---- the promotion pop-up: both halves slide in, the card shows the pilot and the list
    // offers skills with their type icons
    until(&mut run, 3000, "the promotion pop-up", |r| {
        host(r)
            .results()
            .is_some_and(|r| r.pilots.picker_pilot().is_some())
    });
    let mut left = Vec::new();
    let mut right = Vec::new();
    for _ in 0..60 {
        let r = host(&mut run).results().unwrap();
        left.push(r.pilots.promotion.left_offset().0);
        right.push(r.pilots.promotion.right_offset().0);
        frames(&mut run, 1);
    }
    assert!(
        left[0] < -100 && right[0] > 100,
        "the halves start outside: {left:?} {right:?}"
    );
    assert_eq!(
        (*left.last().unwrap(), *right.last().unwrap()),
        (0, 0),
        "and settle"
    );
    assert!(
        left.iter().collect::<std::collections::BTreeSet<_>>().len() >= 8,
        "over many frames: {left:?}"
    );
    frames(&mut run, 60);
    let (_, promo) = shot(&mut run, "results_review_promotion");
    let rank = host(&mut run).campaign().unwrap().pilots()[dagger].rank;
    {
        let r = host(&mut run).results().unwrap();
        let area = &r.pilots.promotion;
        assert!(area.picker_pilot_is(dagger));
        assert!(
            area.left_text(1).contains("Dagger") || !area.left_text(1).is_empty(),
            "the name"
        );
        assert_eq!(
            area.rank_pictures_shown(),
            vec![rank as usize],
            "the rank picture"
        );
        assert!(
            lit(&promo, area.photo_rect().unwrap()) > 100,
            "the pilot's photo on the card"
        );
        let skills: Vec<_> = (0..area.list.item_count())
            .filter_map(|i| {
                area.list
                    .item(i)?
                    .as_any()
                    .downcast_ref::<mc2_ui::results_promotion::SkillRow>()
            })
            .collect();
        assert!(skills.len() >= 3, "skills to pick from");
        let drawn = skills
            .iter()
            .filter(|s| s.obj().show && s.obj().global_bottom() < 430 && s.obj().global_y() > 208)
            .collect::<Vec<_>>();
        assert!(!drawn.is_empty());
        for s in &drawn {
            assert!(
                lit(&promo, s.icon_rect()) > 60,
                "{}: the skill type icon",
                s.description()
            );
        }
    }
    // pick the first skill and confirm: both halves slide away
    let choice = host(&mut run)
        .results_specialty_option()
        .expect("a skill to pick");
    click(&mut run, choice);
    let choose = host(&mut run)
        .results_button(mc2_ui::mission_results::PILOT_CHOOSE)
        .expect("Confirm");
    click(&mut run, choose);
    let mut exiting = Vec::new();
    for _ in 0..80 {
        let r = host(&mut run).results().unwrap();
        exiting.push(r.pilots.promotion.left_offset().0);
        frames(&mut run, 1);
    }
    assert!(
        exiting.iter().any(|&x| x < -200),
        "the card slides off to the left: {exiting:?}"
    );
    until(&mut run, 400, "the pop-up closing", |r| {
        host(r)
            .results()
            .is_some_and(|r| r.pilots.picker_pilot().is_none())
    });
    assert_eq!(
        host(&mut run).campaign().unwrap().pilots()[dagger]
            .specialty_skill_indices()
            .len(),
        1,
        "the pick was written to the pilot"
    );
    until(&mut run, 3000, "the list done", |r| {
        host(r).results().is_some_and(|r| r.pilots.list.is_done())
    });
    let pilot_done = host(&mut run)
        .results_button(mc2_ui::mission_results::PILOT_DONE)
        .expect("Done");
    click(&mut run, pilot_done);
    until(&mut run, 120, "logistics after results", |r| {
        r.core.state() == &GameState::Logistics
    });
}

/// The `options.cfg` the host wrote, read back.
fn saved_options<D: SimDriver + 'static>(
    run: &mut HeadlessRun<D>,
) -> mc2_ui::screens::prefs::Prefs {
    let path = host(run)
        .options_file()
        .expect("a config file")
        .to_path_buf();
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path:?} was not written: {e}"));
    mc2_ui::screens::prefs::Prefs::from_fit(
        &mut mc2_formats::ini::FitIni::from_bytes(bytes).expect("options.cfg parses"),
    )
}

#[test]
fn every_graphics_and_game_option_applies_persists_and_is_not_reverted() {
    use mc2_ui::screens::options::*;
    let Some((mut run, _)) = session() else {
        return;
    };
    open_options(&mut run);
    let before = host(&mut run).prefs().unwrap().clone();
    for id in [
        MSG_TERRAIN_DETAIL,
        MSG_PILOT_VIDS,
        MSG_OBJECT_DETAIL,
        MSG_LOCAL_SHADOWS,
        MSG_ASYNC_MOUSE,
        MSG_HARDWARE_RASTERIZER,
    ] {
        click_control(&mut run, id);
    }
    let edited = host(&mut run).options_edited_prefs().unwrap();
    press_yes(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    // no revert: the accepted values are the host's, with no refusal banner
    let after = host(&mut run).prefs().unwrap().clone();
    assert_eq!(after, edited, "the accepted options are kept as edited");
    assert_ne!(
        after.use_water_interest_texture,
        before.use_water_interest_texture
    );
    assert_ne!(after.pilot_videos, before.pilot_videos);
    assert_ne!(after.use_high_object_detail, before.use_high_object_detail);
    assert_ne!(after.use_local_shadows, before.use_local_shadows);
    assert_ne!(after.async_mouse, before.async_mouse);
    assert_ne!(after.renderer, before.renderer);
    assert!(
        host(&mut run)
            .notices()
            .iter()
            .all(|n| !n.contains("not available yet")),
        "{:?}",
        host(&mut run).notices()
    );
    assert!(
        host(&mut run)
            .notices()
            .iter()
            .any(|n| n.contains("Hardware Rasterizer")),
        "the adapter restart notice stays: {:?}",
        host(&mut run).notices()
    );
    // both are live, so neither gets a false deferral banner
    for what in ["Local Shadows", "Async Mouse"] {
        assert!(
            host(&mut run).notices().iter().all(|n| !n.contains(what)),
            "false deferral notice for {what}: {:?}",
            host(&mut run).notices()
        );
    }
    // the live settings changed
    let live = &run.core.prefs;
    assert_eq!(live.terrain_detail, after.use_water_interest_texture);
    assert_eq!(live.pilot_videos, after.pilot_videos);
    assert_eq!(live.high_object_detail, after.use_high_object_detail);
    assert_eq!(live.local_shadows, after.use_local_shadows);
    assert_eq!(live.async_mouse, after.async_mouse);
    assert_eq!(live.renderer, after.renderer);
    // Local Shadows reaches the renderer's shadow switch (Shadows stays on)
    assert_eq!(run.core.config.shadows, after.use_shadows);
    assert_eq!(run.core.sun_shadows(false), after.use_shadows);
    assert!(!run.core.sun_shadows(true), "never at night");
    // Async Mouse: the cursor follows the freshest pointer only with it on
    let sampled = run.core.cursor_position();
    let moved = (sampled.0 + 7.0, sampled.1 + 5.0);
    run.core
        .handle_event(mc2_platform::input::InputEvent::CursorMoved {
            x: moved.0 as f64,
            y: moved.1 as f64,
        });
    let fresh = run.core.input().mouse_position();
    assert_ne!(fresh, sampled);
    assert_eq!(
        run.core.cursor_position(),
        if after.async_mouse { fresh } else { sampled }
    );
    // Hardware Rasterizer: what the next start-up reads is what was saved
    assert_eq!(
        mc2_app::windowed::saved_renderer(
            host(&mut run).options_file().and_then(|p| p.parent()),
            std::path::Path::new("/nonexistent-mc2-data"),
        ),
        after.renderer
    );
    // the accepted choice names another adapter than the one running (hardware at start): the windowed loop
    // relaunches, and the new process reads the same saved choice and so asks for the software adapter
    assert!(run.core.settings_applied());
    assert_eq!(
        mc2_app::windowed::restart_needed(false, run.core.prefs.renderer),
        after.renderer != 0
    );
    assert!(host(&mut run)
        .notices()
        .iter()
        .all(|n| !n.contains("next time the game starts")));
    // and the config file has them
    // `Prefs::load` (`code/prefs.cpp`) forces 16 bit colour with the software renderer, so that one field reads back changed
    let mut expected = after.clone();
    if expected.renderer == 3 {
        expected.bit_depth = 16;
    }
    assert_eq!(saved_options(&mut run), expected, "options.cfg round trip");
}

#[test]
fn a_new_resolution_resizes_the_game_screen_and_the_window_and_is_saved() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_options(&mut run);
    let before = host(&mut run).prefs().unwrap().clone();
    let same = Some((before.resolution_x, before.resolution_y));
    let mut picked = false;
    for i in 0..6 {
        let at = host(&mut run)
            .options_resolution_button()
            .expect("drop list button");
        click(&mut run, at);
        frames(&mut run, 2);
        let Some(item) = host(&mut run).options_resolution_item(i) else {
            break;
        };
        click(&mut run, item);
        frames(&mut run, 2);
        if host(&mut run).options_edited_resolution() != same {
            picked = true;
            break;
        }
    }
    assert!(picked, "another display mode was picked through the list");
    let (w, h) = {
        let (w, h) = host(&mut run).options_edited_resolution().unwrap();
        (w as u32, h as u32)
    };
    press_yes(&mut run);
    let after = host(&mut run).prefs().unwrap().clone();
    assert_eq!(
        (after.resolution_x, after.resolution_y),
        (w as i32, h as i32)
    );
    assert_eq!(
        run.core.game_size(),
        (w as f32, h as f32),
        "the game screen"
    );
    assert_eq!(
        run.core.take_window_request(),
        Some((w, h)),
        "the windowed front end is asked to resize its window"
    );
    assert_eq!(run.core.take_window_request(), None, "asked once");
    assert_eq!(saved_options(&mut run).resolution_x, w as i32);
    assert!(
        host(&mut run)
            .notices()
            .iter()
            .all(|n| !n.contains("resolution")),
        "{:?}",
        host(&mut run).notices()
    );
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
}

#[test]
fn a_rebound_hot_key_persists_and_takes_effect_in_the_mission_table() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_options(&mut run);
    host(&mut run).select_options_tab(3);
    frames(&mut run, 3);
    let before = host(&mut run).prefs().unwrap().keys.clone();
    let row = host(&mut run).options_hotkey_row(0).expect("a hot key row");
    click(&mut run, row);
    frames(&mut run, 2);
    run.event(mc2_ui::harness::events::key(keys::KEY_F12, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_F12, false, None));
    frames(&mut run, 3);
    let edited = host(&mut run).options_edited_keys().unwrap();
    assert_ne!(edited, before, "the edit took in the options screen");
    let changed = (0..edited.len())
        .find(|&i| edited[i] != before[i])
        .expect("a changed binding");
    press_yes(&mut run);
    assert_eq!(host(&mut run).prefs().unwrap().keys, edited, "kept");
    assert_eq!(saved_options(&mut run).keys, edited, "persisted");
    assert_eq!(run.core.prefs.hot_keys, edited, "reaches the mission");
    // the mission's table: the command that was on the old key is on F12 now
    let table = mc2_app::hotkeys::table_for(&run.core.prefs.hot_keys);
    let handler = mc2_ui::hotkeys::COMMANDS[changed].handler;
    let old = mc2_app::hotkeys::table_for(&[]);
    let moved = table
        .iter()
        .zip(&old)
        .filter(|(n, o)| n.key != o.key)
        .collect::<Vec<_>>();
    assert!(!moved.is_empty(), "{handler}: some table entry moved");
    assert!(moved.iter().all(|(n, _)| n.key & 0xff == keys::KEY_F12));
}

#[test]
fn a_rebound_key_fires_its_command_and_the_old_key_no_longer_does() {
    use mc2_app::hotkeys::{dispatch_with, table_for, Command, Fired};
    let mut keys_table: Vec<u32> = mc2_ui::hotkeys::COMMANDS.iter().map(|c| c.key).collect();
    let index = mc2_ui::hotkeys::COMMANDS
        .iter()
        .position(|c| c.handler == "attackShort")
        .unwrap();
    keys_table[index] = keys::KEY_F12;
    let table = table_for(&keys_table);
    let mut input = mc2_platform::input::InputState::new(mc2_platform::viewport::Viewport::new(
        800,
        600,
        800.0,
        600.0,
        mc2_platform::viewport::Fit::Letterbox,
    ));
    let press = |input: &mut mc2_platform::input::InputState, key| {
        input.handle(mc2_ui::harness::events::key(key, true, None));
        let mut out = Vec::new();
        dispatch_with(input, keys::KEY_LCONTROL, &table, &mut out);
        input.end_frame();
        out
    };
    let new = press(&mut input, keys::KEY_F12);
    assert!(new.contains(&Fired::Run(Command::AttackShort)), "{new:?}");
    let mut input2 = mc2_platform::input::InputState::new(mc2_platform::viewport::Viewport::new(
        800,
        600,
        800.0,
        600.0,
        mc2_platform::viewport::Fit::Letterbox,
    ));
    let old = press(&mut input2, keys::KEY_S);
    assert!(!old.contains(&Fired::Run(Command::AttackShort)), "{old:?}");
}

#[test]
fn the_saved_resolution_is_taken_at_start_up() {
    let Some((mut run, _)) = session() else {
        return;
    };
    let dir = fresh_config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let prefs = mc2_ui::screens::prefs::Prefs {
        resolution_x: 1024,
        resolution_y: 768,
        ..Default::default()
    };
    std::fs::write(dir.join("options.cfg"), prefs.to_fit()).unwrap();
    host(&mut run).set_config_directory(dir);
    run.core.config.use_saved_resolution = true;
    frames(&mut run, 3);
    assert_eq!(run.core.game_size(), (1024.0, 768.0));
    assert_eq!(run.core.take_window_request(), Some((1024, 768)));
}

#[test]
fn a_rebound_key_moves_its_waypoint_twin_through_the_options_screen() {
    use mc2_app::hotkeys::{dispatch_with, table_for, Command, Fired};
    let Some((mut run, _)) = session() else {
        return;
    };
    open_options(&mut run);
    host(&mut run).select_options_tab(3);
    frames(&mut run, 3);
    let jump = mc2_ui::hotkeys::COMMANDS
        .iter()
        .position(|c| c.handler == "jump")
        .unwrap();
    // the list skips the hidden (description -1) entries
    let listed = mc2_ui::hotkeys::COMMANDS[..jump]
        .iter()
        .filter(|c| c.description != -1)
        .count();
    let row = host(&mut run).options_hotkey_row(listed).expect("jump row");
    click(&mut run, row);
    frames(&mut run, 2);
    run.event(mc2_ui::harness::events::key(keys::KEY_F12, true, None));
    frames(&mut run, 1);
    run.event(mc2_ui::harness::events::key(keys::KEY_F12, false, None));
    frames(&mut run, 3);
    press_yes(&mut run);
    assert_eq!(saved_options(&mut run).keys[jump] & 0xff, keys::KEY_F12);
    let table = table_for(&saved_options(&mut run).keys);
    let fires = |key, ctrl: bool| {
        let mut input =
            mc2_platform::input::InputState::new(mc2_platform::viewport::Viewport::new(
                800,
                600,
                800.0,
                600.0,
                mc2_platform::viewport::Fit::Letterbox,
            ));
        if ctrl {
            input.handle(mc2_ui::harness::events::key(keys::KEY_LCONTROL, true, None));
        }
        input.handle(mc2_ui::harness::events::key(key, true, None));
        let mut out = Vec::new();
        dispatch_with(&input, keys::KEY_LCONTROL, &table, &mut out);
        out.contains(&Fired::Run(Command::Jump))
    };
    assert!(fires(keys::KEY_F12, false));
    assert!(fires(keys::KEY_F12, true), "Ctrl+F12 is the waypoint jump");
    assert!(!fires(keys::KEY_J, false));
    assert!(
        !fires(keys::KEY_J, true),
        "the old waypoint binding is gone"
    );
}

/// Renders the current frame of `run` into its own target without advancing the game, and reads it back.
fn render_only<D: SimDriver + 'static>(run: &mut HeadlessRun<D>) -> Vec<u8> {
    let gpu = run.core.scene.gpu_arc().clone();
    let target = run.target.target();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test"),
        });
    run.core.render(&mut enc, &target).unwrap();
    let rb = target.encode_readback(&gpu, &mut enc).unwrap();
    gpu.queue.submit(Some(enc.finish()));
    rb.finish(&gpu).unwrap()
}

#[test]
fn async_mouse_draws_the_cursor_at_the_pointer_that_moved_after_the_update() {
    use mc2_platform::input::InputEvent;
    let Some((mut run, _)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    let at = |x: f64, y: f64| InputEvent::CursorMoved { x, y };
    // the pointer is in the empty top-left corner when the frame's update samples it
    run.core.handle_event(InputEvent::CursorEntered);
    run.core.handle_event(at(20.0, 20.0));
    frames(&mut run, 3);
    // the production schedule of Async Mouse: update, then the pointer events that arrived, then the render
    run.core.frame(std::time::Duration::from_millis(16));
    let sampled = render_only(&mut run);
    run.core.handle_event(at(900.0, 700.0));
    let (w, _) = run.target.size();
    run.core.prefs.async_mouse = false;
    let off = render_only(&mut run);
    run.core.prefs.async_mouse = true;
    let on = render_only(&mut run);
    assert!(
        off == sampled,
        "without Async Mouse the cursor stays where the update sampled it"
    );
    // the pixels that differ are the cursor leaving the sampled position (top-left) and appearing at the fresh
    // one (bottom-right)
    let (mut near_sampled, mut near_fresh, mut elsewhere) = (0, 0, 0);
    for (i, (a, b)) in on.chunks_exact(4).zip(off.chunks_exact(4)).enumerate() {
        if a != b {
            let (x, y) = (i as u32 % w, i as u32 / w);
            if x < 120 && y < 120 {
                near_sampled += 1;
            } else if x > 800 && y > 600 {
                near_fresh += 1;
            } else {
                elsewhere += 1;
            }
        }
    }
    assert!(
        near_sampled > 0,
        "the cursor is gone from the sampled position"
    );
    assert!(near_fresh > 0, "the cursor is drawn at the fresh pointer");
    assert_eq!(elsewhere, 0, "nothing else changed");
}

#[test]
fn a_fullscreen_resolution_the_monitor_lacks_is_rendered_at_the_chosen_size_and_scaled() {
    use mc2_render::gpu::OffscreenTarget;
    use mc2_render::present::ScaledPresenter;
    let Some((mut run, _)) = session() else {
        return;
    };
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    // the chosen mode (SIZE) rendered natively
    let native = render_only(&mut run);
    // a borderless surface twice as large: the monitor has no SIZE mode
    let surface_size = (SIZE.0 * 2, SIZE.1 * 2);
    run.core.resize(surface_size);
    run.core.set_scaled_fullscreen(true);
    assert_eq!(run.core.render_size(surface_size), SIZE);
    let gpu = run.core.scene.gpu_arc().clone();
    let surface =
        OffscreenTarget::with_format(&gpu, surface_size.0, surface_size.1, gpu.target_format);
    let target = surface.target();
    let mut presenter = ScaledPresenter::new();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("scaled"),
        });
    run.core
        .render_presented(&mut enc, &target, &mut presenter)
        .unwrap();
    let scaled = target.read_rgba(&gpu, enc).unwrap();
    assert_eq!(
        presenter.frame_size(),
        Some(SIZE),
        "the scene was rendered into attachments of the chosen size"
    );
    // the surface shows that frame stretched: each 2 x 2 block averages to the native pixel
    let (nw, nh) = (SIZE.0 as usize, SIZE.1 as usize);
    let mut total = 0u64;
    for y in 0..nh {
        for x in 0..nw {
            for c in 0..3 {
                let mut sum = 0u32;
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    sum += u32::from(scaled[(((y * 2 + dy) * nw * 2) + x * 2 + dx) * 4 + c]);
                }
                let avg = (sum / 4) as i32;
                total += (avg - i32::from(native[(y * nw + x) * 4 + c])).unsigned_abs() as u64;
            }
        }
    }
    let mean = total as f64 / (nw * nh * 3) as f64;
    assert!(
        mean < 6.0,
        "the stretched frame is the native one (mean error {mean})"
    );
    // leaving fullscreen renders at the surface size again
    run.core.set_scaled_fullscreen(false);
    assert_eq!(run.core.render_size(surface_size), surface_size);
}

#[test]
fn a_surface_resize_keeps_the_chosen_fullscreen_resolution() {
    use mc2_platform::input::InputEvent;
    let Some((mut run, _)) = session() else {
        return;
    };
    let chosen = run.core.game_size();
    // the Resolution option chose SIZE; the monitor has no such mode, so the borderless surface keeps its size
    run.core.set_scaled_fullscreen(true);
    // the real event sequence after acceptance and at start-up: the window system reports its size
    for _ in 0..2 {
        run.core.handle_event(InputEvent::Resized {
            logical_w: 1920.0,
            logical_h: 1080.0,
        });
        run.core.resize((1920, 1080));
        assert_eq!(
            run.core.game_size(),
            chosen,
            "the choice survives the resize"
        );
        assert_eq!(run.core.render_size((1920, 1080)), SIZE);
    }
    // pointer coordinates still map through the chosen game screen onto the surface
    run.core
        .handle_event(InputEvent::CursorMoved { x: 960.0, y: 540.0 });
    let (mx, my) = run.core.input().mouse_position();
    assert!((mx - chosen.0 / 2.0).abs() < 1.0 && (my - chosen.1 / 2.0).abs() < 1.0);
    // a windowed run is the window again
    run.core.set_scaled_fullscreen(false);
    run.core.handle_event(InputEvent::Resized {
        logical_w: 800.0,
        logical_h: 600.0,
    });
    assert_eq!(run.core.game_size(), (800.0, 600.0));
}

/// Confirming a campaign: the panel slides out over the menu first (the retained dialog is
/// drawn and advances), and only then does the campaign start.
#[test]
fn confirming_a_campaign_plays_the_leave_slide_before_starting_it() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_campaign_list(&mut run);
    frames(&mut run, 60);
    press(&mut run, keys::KEY_RETURN);
    assert!(
        host(&mut run).campaign_pick_leaving(),
        "the panel slides out"
    );
    assert!(host(&mut run).campaign().is_none(), "nothing starts yet");
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    until(&mut run, 120, "campaign start", |r| {
        host(r).campaign().is_some()
    });
    assert!(!host(&mut run).campaign_pick_leaving());
}

/// The confirmed dialog is modal while it slides out: clicking Options on the menu behind it
/// neither opens Options nor stops the slide, and the campaign starts exactly once afterwards.
#[test]
fn menu_clicks_cannot_interrupt_the_campaign_leave_slide() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_campaign_list(&mut run);
    frames(&mut run, 60);
    press(&mut run, keys::KEY_RETURN);
    assert!(host(&mut run).campaign_pick_leaving());
    // click within the slide's first frames (the slide lasts about 20 frames)
    pointer(&mut run, (402.0, 347.0));
    frames(&mut run, 1);
    button(&mut run, true);
    frames(&mut run, 1);
    button(&mut run, false);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    assert!(
        host(&mut run).campaign_pick_leaving(),
        "the slide keeps playing"
    );
    until(&mut run, 120, "campaign start", |r| {
        host(r).campaign().is_some()
    });
    assert_ne!(screen(&mut run), ActiveScreen::Options);
}

/// The modal closing guard drops clicks but keeps the pointer where it is and visible, through the
/// slide and after the campaign starts navigation.
#[test]
fn campaign_leave_slide_keeps_the_pointer() {
    let Some((mut run, _)) = session() else {
        return;
    };
    open_campaign_list(&mut run);
    frames(&mut run, 60);
    pointer(&mut run, (402.0, 347.0));
    frames(&mut run, 1);
    press(&mut run, keys::KEY_RETURN);
    assert!(host(&mut run).campaign_pick_leaving());
    let at = host(&mut run).menu_pointer().expect("loaded").0;
    for _ in 0..8 {
        frames(&mut run, 1);
        if !host(&mut run).campaign_pick_leaving() {
            break;
        }
        assert_eq!(
            host(&mut run).menu_pointer(),
            Some((at, true)),
            "pointer stays put and visible while the dialog slides out"
        );
    }
    assert_eq!(at, (402, 347));
}

/// Triage 3d111c56 (visual-qa settle): the original's save dialog enables DELETE on every frame
/// (`buttons[0].disable( 0 )`, logisticsdialog.cpp:684-685) and its DELETE asks, then removes the
/// file (logisticsdialog.cpp:765-796, 1119-1145). NO keeps the save, YES removes it and the list
/// is rebuilt.
#[test]
fn delete_in_the_load_dialog_asks_then_removes_the_save() {
    let Some((mut run, _)) = session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-delete-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    let service = CampaignSaveService::new(&saves);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
    let data = std::sync::Arc::new(mc2_campaign::GameData::load(&mut vfs).unwrap());
    let fixture = mc2_campaign::Logistics::new(data).unwrap();
    service.save_named("alpha", &fixture).unwrap();
    service.save_named("bravo", &fixture).unwrap();
    host(&mut run).set_campaign_save_service(service);
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    let load = host(&mut run)
        .menu_button(MM_MSG_LOAD)
        .expect("Load Game button");
    click(&mut run, load);
    until(&mut run, 120, "the load dialog", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    frames(&mut run, 2);
    let (delete, enabled) = host(&mut run).save_load_delete_button().expect("DELETE");
    assert!(enabled, "DELETE is enabled in the original's save dialog");
    // NO keeps the file
    click(&mut run, delete);
    frames(&mut run, 40);
    let no = host(&mut run).save_load_prompt_button(false).expect("No");
    click(&mut run, no);
    frames(&mut run, 40);
    assert!(saves.join("alpha.ron").is_file());
    assert!(saves.join("bravo.ron").is_file());
    // YES removes the selected (first) save and the list is rebuilt without it
    click(&mut run, delete);
    frames(&mut run, 40);
    let yes = host(&mut run).save_load_prompt_button(true).expect("Yes");
    click(&mut run, yes);
    frames(&mut run, 40);
    assert!(
        !saves.join("alpha.ron").exists(),
        "the selected save is gone"
    );
    assert!(saves.join("bravo.ron").is_file(), "the other save stays");
    assert_eq!(screen(&mut run), ActiveScreen::SaveLoad, "the dialog stays");
    assert!(host(&mut run).save_load_row_centre(0).is_some());
    assert!(host(&mut run).save_load_row_centre(1).is_none());
    assert_eq!(host(&mut run).save_load_selected(), Some(0));
    // deleting the last save leaves an empty list, LOAD disabled and CANCEL working
    // (logisticsdialog.cpp:728-742)
    click(&mut run, delete);
    frames(&mut run, 40);
    let yes = host(&mut run).save_load_prompt_button(true).expect("Yes");
    click(&mut run, yes);
    frames(&mut run, 40);
    assert!(!saves.join("bravo.ron").exists());
    assert!(host(&mut run).save_load_row_centre(0).is_none());
    let (_, enabled) = host(&mut run).save_load_confirm_button().expect("LOAD");
    assert!(!enabled, "LOAD is disabled with no saves");
    let (cancel, enabled) = host(&mut run).save_load_cancel_button().expect("CANCEL");
    assert!(enabled);
    click(&mut run, cancel);
    frames(&mut run, 90);
    assert_ne!(screen(&mut run), ActiveScreen::SaveLoad);
    std::fs::remove_dir_all(saves).unwrap();
}

/// Review of bc1ede8b: after a DELETE in the Save dialog the surviving numbered slot is still the
/// save target (`slot-NN`, not its display label), so SAVE asks before overwriting it
/// (logisticsdialog.cpp:789-794).
#[test]
fn delete_in_the_save_dialog_keeps_the_numbered_slot_as_the_save_target() {
    let Some((mut run, _)) = session() else {
        return;
    };
    let temp_root = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .expect("TMPDIR");
    let saves = temp_root.join(format!("mc2-ui-host-delete-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&saves);
    let service = CampaignSaveService::new(&saves);
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
    let data = std::sync::Arc::new(mc2_campaign::GameData::load(&mut vfs).unwrap());
    let fixture = mc2_campaign::Logistics::new(data).unwrap();
    service.save_slot(1, &fixture).unwrap();
    service.save_slot(2, &fixture).unwrap();
    host(&mut run).set_campaign_save_service(service);
    frames(&mut run, 2);
    skip_menu_intro(&mut run);
    frames(&mut run, 90);
    host(&mut run).start_campaign_at_selection();
    let at = host(&mut run).menu_button(MM_MSG_NEW_CAMPAIGN).unwrap();
    click(&mut run, at);
    choose_campaign(&mut run, 0);
    until(&mut run, 120, "logistics", |r| {
        r.core.state() == &GameState::Logistics
    });
    skip_logistics_movie(&mut run);
    return_to_main_menu(&mut run);
    assert_eq!(screen(&mut run), ActiveScreen::MainMenu);
    frames(&mut run, 45);
    let save = host(&mut run)
        .menu_button(MM_MSG_SAVE)
        .expect("Save Game button");
    click(&mut run, save);
    until(&mut run, 120, "the save dialog", |r| {
        screen(r) == ActiveScreen::SaveLoad && host(r).save_load_ready()
    });
    frames(&mut run, 2);
    let (delete, _) = host(&mut run).save_load_delete_button().expect("DELETE");
    click(&mut run, delete);
    frames(&mut run, 40);
    let yes = host(&mut run).save_load_prompt_button(true).expect("Yes");
    click(&mut run, yes);
    frames(&mut run, 40);
    let left: Vec<u32> = [1u32, 2]
        .into_iter()
        .filter(|n| saves.join(format!("slot-{n:02}.ron")).is_file())
        .collect();
    assert_eq!(left.len(), 1, "exactly one numbered slot is deleted");
    assert_eq!(
        host(&mut run).save_load_filename(),
        Some(format!("slot-{:02}", left[0])),
        "the surviving slot keeps its file identity"
    );
    let before = std::fs::read(saves.join(format!("slot-{:02}.ron", left[0]))).unwrap();
    let (confirm, enabled) = host(&mut run).save_load_confirm_button().expect("SAVE");
    assert!(enabled);
    click(&mut run, confirm);
    frames(&mut run, 40);
    assert!(
        host(&mut run).save_load_prompt_button(true).is_some(),
        "an existing slot asks before it is overwritten"
    );
    assert!(!saves.join(format!("Slot {:02}.ron", left[0])).exists());
    assert_eq!(
        std::fs::read(saves.join(format!("slot-{:02}.ron", left[0]))).unwrap(),
        before
    );
    std::fs::remove_dir_all(saves).unwrap();
}

/// A launch straight into a mission reads the saved `options.cfg` too (`CPrefs::load`,
/// `code/prefs.cpp:133`): Shadows FALSE stops the sun's shadows (`code/gamecam.cpp:159`) and Local
/// Shadows FALSE the point lights' (`mclib/tgl.cpp:3049`), though no menu was ever shown.
#[test]
fn direct_launch_applies_the_saved_shadow_options() {
    let Some(data) = data() else { return };
    let dir = fresh_config_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = "FITini\r\n[MechCommander2]\r\nb Shadows = FALSE\r\nb UseLocalShadows = FALSE\r\n[End]\r\nFITend\r\n";
    std::fs::write(dir.join("options.cfg"), cfg).unwrap();
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = SIZE;
    opts.game.cursor = false;
    opts.user_config_dir = Some(dir.clone());
    assert!(opts.game.shadows, "the command line leaves shadows on");
    let Some(mut run) = viewer(&opts) else { return };
    until(&mut run, 600, "the mission", |r| {
        r.core.state() == &GameState::InMission
    });
    frames(&mut run, 2);
    assert!(!run.core.config.shadows, "saved Shadows FALSE");
    assert!(!run.core.prefs.local_shadows, "saved UseLocalShadows FALSE");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The windowed front end never names a config directory: the host resolves the normal user one
/// (`MC2_USER_DIR` first) and a launch straight into a mission still applies the saved Shadows
/// (`CPrefs::load` at start-up, `code/mechcmd2.cpp:1519`; gate `code/gamecam.cpp:159`). No GPU needed.
#[test]
fn windowed_default_host_applies_the_saved_shadows_without_menus() {
    let Some(data) = data() else { return };
    isolate_user_dir();
    let config = mc2_platform::paths::UserPaths::resolve()
        .expect("user paths")
        .config;
    std::fs::create_dir_all(&config).unwrap();
    let cfg = "FITini\r\n[MechCommander2]\r\nb Shadows = FALSE\r\nb UseLocalShadows = FALSE\r\n[End]\r\nFITend\r\n";
    std::fs::write(config.join("options.cfg"), cfg).unwrap();
    let mut vfs = Vfs::from_system_cfg(data).expect("vfs");
    let mut host = UiHost::new(); // as windowed.rs builds it: no set_config_directory
    let input = InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Letterbox));
    let state = GameState::InMission;
    let mut frame = UiFrame {
        state: &state,
        input: &input,
        dt: 1.0 / 30.0,
        game_size: (800.0, 600.0),
        camera: None,
        paused: false,
        vfs: &mut vfs,
    };
    host.update(&mut frame);
    let applied = host.take_applied_settings().expect("saved options applied");
    assert!(!applied.shadows, "saved Shadows FALSE");
    assert!(!applied.local_shadows, "saved UseLocalShadows FALSE");
    let _ = std::fs::remove_file(config.join("options.cfg"));
}
