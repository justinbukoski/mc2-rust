// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Headless evidence for the movie host's playback of the logistics movie requests
//! (`mc2_app::movie_control`). The mission-select movie advances while it plays, holds while paused
//! (its sound with it), stops and restarts from its first picture as `MC2Movie` does, and the
//! campaign's final video ends in the main menu when it finishes or is skipped. A `cinema5` final
//! video hands over to the credits first (`code/logistics.cpp:283-298`).
//!
//! Needs `MC2_DATA`; skips cleanly without it. Nothing here opens a window or an audio device: the
//! sound goes to a [`Mixer`] rendered into a buffer.

use std::sync::Arc;

use mc2_app::audio::Cue;
use mc2_app::movie_control::{MovieControl, MovieEnd, Playback};
use mc2_app::presentation::AppPresentation;
use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_platform::audio::{Mixer, PlayMode, SharedMixer, APP_MIXER_CHANNELS, MOVIE_AUDIO_CHANNEL};
use mc2_ui::input::UiInput;
use mc2_ui::logistics::mission_select::MissionSelect;
use mc2_ui::logistics::movie::{final_video_request, MovieRequest};
use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
use mc2_ui::logistics::{Page, Placeholder};
use mc2_ui::{Loader, UiAssets, UiOutput};

/// The first group's movie (the production intro tests use the same one).
const MOVIE: &str = "cinema1";
/// The first group's final video as its campaign file names it, which the credits follow
/// (`code/logistics.cpp:285`).
const CINEMA5: &str = "cinema5.bik";
/// One frame at 30 Hz, the host's frame time.
const FRAME: f32 = 1.0 / 30.0;
/// The most steps of a tenth of a second a final video may run before the test gives up (30 minutes).
const LONGEST_RUN: usize = 30 * 60 * 10;

fn game_vfs() -> Option<Vfs> {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("skipping movie control evidence: MC2_DATA is unset");
        return None;
    };
    Some(Vfs::from_system_cfg(root).expect("open game VFS"))
}

/// The time of the picture on screen, if one is.
fn picture_time(movie: &MovieControl) -> Option<f64> {
    movie.frame().map(|frame| frame.time)
}

fn advance_frames(vfs: &mut Vfs, movie: &mut MovieControl, frames: usize) {
    for _ in 0..frames {
        movie.advance(vfs, FRAME).expect("the movie plays");
    }
}

/// Advances until `done` holds, in steps of a tenth of a second (the movie decodes the same either
/// way; the steps only keep the run short), and panics if the movie runs past [`LONGEST_RUN`] steps.
fn advance_until(vfs: &mut Vfs, movie: &mut MovieControl, done: impl Fn(&MovieControl) -> bool) {
    for _ in 0..LONGEST_RUN {
        if done(movie) {
            return;
        }
        movie.advance(vfs, 0.1).expect("the movie plays");
    }
    panic!("the movie did not reach the expected point in {LONGEST_RUN} steps");
}

/// The loudest sample the mixer renders for about one frame of output.
fn render_peak(mixer: &SharedMixer) -> f32 {
    let mut out = vec![0.0f32; 2 * 1470];
    mixer.lock().render(&mut out);
    out.iter().fold(0.0, |peak, s| peak.max(s.abs()))
}

#[test]
fn the_mission_movie_advances_holds_on_pause_and_resumes() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .expect("the mission movie opens");
    assert_eq!(movie.playback(), Playback::Playing, "a fresh opening plays");

    advance_frames(&mut vfs, &mut movie, 30);
    let first = picture_time(&movie).expect("a picture after a second of playback");
    advance_frames(&mut vfs, &mut movie, 30);
    let later = picture_time(&movie).expect("a picture");
    assert!(
        later > first,
        "the picture advances while playing: {first} then {later}"
    );
    assert!(
        movie.clock().is_some_and(|clock| clock > 1.5),
        "the clock runs with the frames: {:?}",
        movie.clock()
    );

    movie
        .apply(&mut vfs, &MovieRequest::Pause(true))
        .expect("pause");
    assert_eq!(movie.playback(), Playback::Paused);
    let held_clock = movie.clock();
    let held = picture_time(&movie);
    advance_frames(&mut vfs, &mut movie, 90);
    assert_eq!(
        movie.clock(),
        held_clock,
        "a held movie does not run its clock"
    );
    assert_eq!(
        picture_time(&movie),
        held,
        "the picture stays where the hold found it"
    );
    assert!(movie.frame().is_some(), "a held picture stays on screen");

    movie
        .apply(&mut vfs, &MovieRequest::Pause(false))
        .expect("resume");
    assert_eq!(movie.playback(), Playback::Playing);
    advance_frames(&mut vfs, &mut movie, 30);
    assert!(
        picture_time(&movie).is_some_and(|t| Some(t) > held),
        "the picture advances again after the resume"
    );
}

#[test]
fn mission_play_video_hides_only_v1a_first_picture_and_keeps_playback_synced() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut presentation = AppPresentation::with_movie(MovieControl::new(None));
    let requests = presentation.events(
        &mut vfs,
        &[mc2_sim::events::SimEvent::PlayVideo {
            name: "V1A.bik".into(),
        }],
    );
    assert_eq!(requests, [MovieRequest::Open("V1A".into())]);
    assert!(
        presentation.video_frame().is_none(),
        "no picture is presented before the mission movie advances"
    );
    advance_frames(&mut vfs, &mut presentation.movie, 1);
    assert!(
        presentation.video_frame().is_none(),
        "the timestamp-zero startup picture is omitted"
    );
    advance_frames(&mut vfs, &mut presentation.movie, 14);
    let clock = presentation.movie.clock().expect("V1A still plays");
    let picture = presentation
        .video_frame()
        .expect("the valid V1A picture at half a second remains visible");
    let luma = picture.luma_visible();
    let range = luma.iter().copied().max().unwrap_or(0) - luma.iter().copied().min().unwrap_or(0);
    assert!(
        range > 100,
        "V1A's visible picture has image detail, range {range}"
    );
    assert!(
        picture.time > 0.4 && clock >= 0.5,
        "the displayed picture and continuing movie clock remain synchronized: frame {}, clock {clock}",
        picture.time
    );
}

#[test]
fn the_movie_sound_holds_with_the_picture_and_resumes_with_it() {
    // The sound channel is paused with the picture (`PlayMode::Pause`), so a held movie is silent
    // and its queued sound waits; a resume plays on from there.
    let Some(mut vfs) = game_vfs() else { return };
    // the app's mixer, which has the movie's channel after the game's (`APP_MIXER_CHANNELS`)
    let mixer = SharedMixer::new(Mixer::new(44_100, APP_MIXER_CHANNELS));
    let mut movie = MovieControl::new(Some(mixer.clone()));
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .expect("the mission movie opens");
    assert_eq!(
        mixer.lock().mode(MOVIE_AUDIO_CHANNEL),
        PlayMode::PlayOnce,
        "the movie's sound plays with its picture"
    );
    advance_frames(&mut vfs, &mut movie, 60);
    assert!(
        render_peak(&mixer) > 0.0,
        "the movie's sound is audible while it plays"
    );

    movie
        .apply(&mut vfs, &MovieRequest::Pause(true))
        .expect("pause");
    assert_eq!(mixer.lock().mode(MOVIE_AUDIO_CHANNEL), PlayMode::Pause);
    assert_eq!(render_peak(&mixer), 0.0, "the held sound is silent");
    advance_frames(&mut vfs, &mut movie, 60);
    assert_eq!(
        render_peak(&mixer),
        0.0,
        "no sound comes out while the picture is held"
    );

    movie
        .apply(&mut vfs, &MovieRequest::Pause(false))
        .expect("resume");
    assert_eq!(mixer.lock().mode(MOVIE_AUDIO_CHANNEL), PlayMode::PlayOnce);
    advance_frames(&mut vfs, &mut movie, 60);
    assert!(
        render_peak(&mixer) > 0.0,
        "the sound comes back with the picture"
    );

    movie.apply(&mut vfs, &MovieRequest::Stop).expect("stop");
    assert_eq!(mixer.lock().mode(MOVIE_AUDIO_CHANNEL), PlayMode::Stop);
}

#[test]
fn stop_shows_nothing_and_play_restarts_from_the_first_picture() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .unwrap();
    advance_frames(&mut vfs, &mut movie, 60);
    assert!(picture_time(&movie).is_some_and(|t| t > 1.0));

    movie.apply(&mut vfs, &MovieRequest::Stop).expect("stop");
    assert_eq!(movie.playback(), Playback::Stopped);
    assert!(movie.frame().is_none(), "a stopped movie shows no picture");
    assert!(movie.clock().is_none(), "and holds no playback");
    advance_frames(&mut vfs, &mut movie, 30);
    assert_eq!(
        movie.playback(),
        Playback::Stopped,
        "nothing plays until Play"
    );
    assert!(
        movie.take_end().is_none(),
        "a screen movie has no transition"
    );

    movie.apply(&mut vfs, &MovieRequest::Play).expect("play");
    assert_eq!(movie.playback(), Playback::Playing);
    advance_frames(&mut vfs, &mut movie, 1);
    let restarted = picture_time(&movie).expect("the first picture again");
    assert!(
        restarted < 0.1,
        "Play restarts from the first picture, not where Stop left it: {restarted}"
    );

    movie.apply(&mut vfs, &MovieRequest::Close).expect("close");
    assert_eq!(movie.playback(), Playback::Idle);
    assert!(movie.frame().is_none());
}

#[test]
fn play_after_pause_resumes_rather_than_staying_held() {
    // The C++ `MN_MSG_PLAY` restarts the movie but leaves its pause flag set (a fault this port does
    // not copy; see DIFFERENCES.md). Play resumes the movie.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .unwrap();
    advance_frames(&mut vfs, &mut movie, 30);
    movie
        .apply(&mut vfs, &MovieRequest::Pause(true))
        .expect("pause");
    movie.apply(&mut vfs, &MovieRequest::Play).expect("play");
    assert_eq!(movie.playback(), Playback::Playing);
    advance_frames(&mut vfs, &mut movie, 30);
    assert!(
        movie.clock().is_some_and(|clock| clock > 0.9),
        "the clock runs after Play: {:?}",
        movie.clock()
    );
}

#[test]
fn a_hold_on_a_stopped_movie_does_not_start_it() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .unwrap();
    movie.apply(&mut vfs, &MovieRequest::Stop).unwrap();
    movie
        .apply(&mut vfs, &MovieRequest::Pause(false))
        .expect("resume");
    assert_eq!(movie.playback(), Playback::Stopped);
    movie
        .apply(&mut vfs, &MovieRequest::Pause(true))
        .expect("hold");
    assert_eq!(movie.playback(), Playback::Stopped);
}

#[test]
fn play_reports_failure_when_no_movie_is_open() {
    // A Play with nothing to play is not a success: the page's request failed, and the host says so.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    assert!(
        movie.apply(&mut vfs, &MovieRequest::Play).is_err(),
        "nothing is open to play"
    );
    movie
        .apply(
            &mut vfs,
            &MovieRequest::Open("movie_that_does_not_exist".into()),
        )
        .expect_err("a missing movie does not open");
    let error = movie
        .apply(&mut vfs, &MovieRequest::Play)
        .expect_err("a failed open leaves nothing to play");
    assert!(error.contains("no movie"), "{error}");
    assert_eq!(movie.playback(), Playback::Idle);
}

#[test]
fn skipping_a_screen_movie_ends_it_without_a_transition() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .unwrap();
    advance_frames(&mut vfs, &mut movie, 10);
    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert_eq!(movie.playback(), Playback::Stopped);
    assert!(movie.frame().is_none());
    assert_eq!(movie.take_end(), None);
}

#[test]
fn the_final_video_ends_in_the_main_menu_when_skipped() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(MOVIE.into()))
        .expect("the final video opens");
    assert_eq!(movie.playback(), Playback::Playing);
    advance_frames(&mut vfs, &mut movie, 10);
    assert!(
        movie.frame().is_some(),
        "the final video shows its pictures"
    );

    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
    assert_eq!(movie.take_end(), None, "the transition is reported once");
}

#[test]
fn the_final_video_ends_in_the_main_menu_when_it_finishes() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(MOVIE.into()))
        .unwrap();
    advance_until(&mut vfs, &mut movie, |m| m.playback() != Playback::Playing);
    assert_eq!(
        movie.playback(),
        Playback::Stopped,
        "the final video runs out"
    );
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
}

#[test]
fn a_cinema5_final_video_hands_over_to_the_credits_when_skipped() {
    // The skip ends the video as its end does (`MC2Movie::stop`), so the credits play before the menu.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(CINEMA5.into()))
        .expect("cinema5 opens");
    advance_frames(&mut vfs, &mut movie, 10);

    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert_eq!(movie.movie_name(), Some("credits"), "the credits follow");
    assert_eq!(movie.playback(), Playback::Playing);
    assert_eq!(
        movie.take_end(),
        None,
        "the game does not leave for the menu while the credits play"
    );
    advance_frames(&mut vfs, &mut movie, 10);
    assert!(
        movie.clock().is_some_and(|clock| clock < 1.0),
        "the credits start from their first picture: {:?}",
        movie.clock()
    );

    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert_eq!(movie.playback(), Playback::Stopped);
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
}

#[test]
fn a_cinema5_final_video_hands_over_to_the_credits_when_it_finishes() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(CINEMA5.into()))
        .unwrap();
    advance_until(&mut vfs, &mut movie, |m| m.movie_name() == Some("credits"));
    assert_eq!(movie.playback(), Playback::Playing, "the credits play");
    assert_eq!(
        movie.take_end(),
        None,
        "the game does not leave for the menu when the video ends"
    );

    advance_until(&mut vfs, &mut movie, |m| m.playback() != Playback::Playing);
    assert_eq!(movie.movie_name(), Some("credits"));
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
}

#[test]
fn a_final_video_that_is_missing_still_goes_to_the_main_menu() {
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    let error = movie
        .apply(
            &mut vfs,
            &MovieRequest::FinalVideo("movie_that_does_not_exist".into()),
        )
        .expect_err("a missing movie is reported");
    assert!(error.contains("movie unavailable"), "{error}");
    assert_eq!(movie.playback(), Playback::Idle);
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
}

#[test]
fn the_selection_screens_requests_play_through_the_host() {
    // The production path up to the host: a connected selection screen's begin sends its movie's
    // Open, the host applies it, and its pictures advance on the frames the sequence runs, with no
    // unavailable report from the sequence.
    let Some(mut vfs) = game_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("game data"));
    let mut lg = Logistics::with_campaign(data, "campaign").expect("campaign");
    let mut assets = UiAssets::default();
    let select = {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        MissionSelect::load(&mut ld).expect("the selection screen loads")
    };
    let mut pages: Vec<Box<dyn Page>> = vec![Box::new(select)];
    pages.extend(
        [
            "brief", "bay", "pilot", "purchase", "lab", "loading", "options",
        ]
        .iter()
        .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>),
    );
    assert_eq!(pages.len(), slot::COUNT);
    let mut seq = Sequence::single_player(pages);
    seq.connect_movie_host();
    {
        let mut ld = Loader::new(&mut vfs, &mut assets);
        seq.begin(&mut ld, &mut lg);
    }
    let mut movie = MovieControl::new(None);
    let mut input = UiInput::new(800, 600);
    let mut picture = None;
    // the first mission's movie is a two-second stand-in, so the run stays well inside it
    for _ in 0..30 {
        let flow = {
            let mut ld = Loader::new(&mut vfs, &mut assets);
            input.begin_frame(FRAME);
            let mut out = UiOutput::default();
            let flow = seq.update(&mut ld, &mut input, &mut out, FRAME, &mut lg);
            input.end_frame();
            flow
        };
        assert!(
            !matches!(flow, Flow::Unavailable(_)),
            "the selection screen's movie is not reported unavailable: {flow:?}"
        );
        for request in seq.take_movie_requests() {
            movie
                .apply(&mut vfs, &request)
                .expect("the requested movie plays");
        }
        movie.advance(&mut vfs, FRAME).expect("the movie plays");
        picture = picture.or(picture_time(&movie));
    }
    assert_eq!(movie.playback(), Playback::Playing, "the movie plays");
    assert!(lg.video_shown(), "the campaign marks the movie shown");
    assert!(
        picture_time(&movie).is_some_and(|t| Some(t) > picture),
        "the pictures advance on the sequence's frames"
    );
}

#[test]
fn a_final_video_stops_the_music_as_it_starts() {
    // `Logistics::playFullScreenVideo` stops the music before the video plays (`code/logistics.cpp:897`).
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(MOVIE.into()))
        .expect("the final video opens");
    assert_eq!(movie.take_cues(), vec![Cue::StopMusic]);
    assert!(movie.take_cues().is_empty(), "a cue is taken once");
}

#[test]
fn a_screen_movie_makes_no_music_cue() {
    // The mission-select screen starts no music for its movie: its C++ screen makes no music call.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::Open(MOVIE.into()))
        .unwrap();
    advance_frames(&mut vfs, &mut movie, 30);
    movie
        .apply(&mut vfs, &MovieRequest::Pause(true))
        .expect("pause");
    movie.apply(&mut vfs, &MovieRequest::Stop).expect("stop");
    movie.apply(&mut vfs, &MovieRequest::Play).expect("play");
    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert!(movie.take_cues().is_empty(), "no music cue from a screen");
}

#[test]
fn a_non_cinema5_final_video_stops_the_music_and_plays_no_tune_at_its_end() {
    // Once the campaign is over the logistics tune is -1 (`getCurrentLogisticsTuneId`), which plays
    // nothing, so only the stop is made.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(MOVIE.into()))
        .unwrap();
    advance_until(&mut vfs, &mut movie, |m| m.playback() != Playback::Playing);
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
    assert_eq!(movie.take_cues(), vec![Cue::StopMusic]);
}

#[test]
fn a_cinema5_hands_over_to_the_credits_tune_when_it_ends() {
    // `code/logistics.cpp:303`: the credits start on tune 33, after the video's stop.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(CINEMA5.into()))
        .unwrap();
    assert_eq!(movie.take_cues(), vec![Cue::StopMusic]);

    advance_until(&mut vfs, &mut movie, |m| m.movie_name() == Some("credits"));
    assert_eq!(
        movie.take_cues(),
        vec![Cue::Music(33)],
        "the credits play on tune 33"
    );

    advance_until(&mut vfs, &mut movie, |m| m.playback() != Playback::Playing);
    assert!(
        movie.take_cues().is_empty(),
        "the credits' end plays no tune once the campaign is over"
    );
    assert_eq!(movie.take_end(), Some(MovieEnd::MainMenu));
}

#[test]
fn a_skipped_cinema5_hands_over_to_the_credits_tune_too() {
    // A skip ends the cinema5 as its end does (`MC2Movie::stop`), so the credits tune is made too.
    let Some(mut vfs) = game_vfs() else { return };
    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &MovieRequest::FinalVideo(CINEMA5.into()))
        .unwrap();
    advance_frames(&mut vfs, &mut movie, 10);
    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert_eq!(
        movie.take_cues(),
        vec![Cue::StopMusic, Cue::Music(33)],
        "the stop, then the credits tune"
    );
    assert_eq!(movie.movie_name(), Some("credits"));
}

#[test]
fn the_campaign_final_video_request_stops_the_music_and_hands_over_to_the_credits() {
    // The production path: the campaign that is over asks for its final video
    // (`final_video_request`), the host plays it, and the cinema5 hands over to the credits.
    let Some(mut vfs) = game_vfs() else { return };
    let data = Arc::new(GameData::load(&mut vfs).expect("game data"));
    let lg = Logistics::with_campaign(Arc::clone(&data), "campaign").expect("campaign");
    // the campaign's stage moved to its end, as a save of a finished campaign names it
    let text = lg.save_to_string().expect("the campaign saves");
    let over = text.replacen(
        "current_stage: 0,",
        &format!("current_stage: {},", lg.mission_info().group_count()),
        1,
    );
    assert_ne!(over, text, "the save names its stage");
    let done = Logistics::load_from_str(data, &over)
        .expect("the campaign over loads")
        .logistics;
    let request = final_video_request(&done).expect("the final video is requested");
    assert_eq!(request, MovieRequest::FinalVideo(CINEMA5.into()));

    let mut movie = MovieControl::new(None);
    movie
        .apply(&mut vfs, &request)
        .expect("the final video opens");
    assert_eq!(movie.take_cues(), vec![Cue::StopMusic]);
    advance_frames(&mut vfs, &mut movie, 10);
    movie.apply(&mut vfs, &MovieRequest::Skip).expect("skip");
    assert_eq!(movie.take_cues(), vec![Cue::Music(33)]);
    assert_eq!(movie.movie_name(), Some("credits"));
}
