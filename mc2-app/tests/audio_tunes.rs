// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Which tune and which UI sound the app asks for, hand-derived from the C++ (cited per assertion).
//! Needs no game data.

use mc2_app::audio::{self, tune, Cue};
use mc2_audio::ids::{music, sfx};

/// `mclib/sounds.h` values for the UI sounds the screens play (`mc2_ui::ctx::sfx`).
#[test]
fn ui_sound_ids_match_the_c_header() {
    use mc2_ui::ctx::sfx as ui;
    assert_eq!(ui::LOG_WRONGBUTTON, 89); // sounds.h:101
    assert_eq!(ui::LOG_VIDEOBUTTONS, 90); // sounds.h:102
    assert_eq!(ui::LOG_SELECT, 90); // sounds.h:104 "same as video buttons"
    assert_eq!(ui::LOG_MAINMENUBUTTON, 95); // sounds.h:107
    assert_eq!(ui::LOG_HIGHLIGHTBUTTONS, 96); // sounds.h:108
    assert_eq!(ui::LOG_DIGITALHIGHLIGHT, 96); // sounds.h:112
    assert_eq!(ui::LOG_EXITGAME, 97); // sounds.h:109
    assert_eq!(ui::LOG_CLICKONBUTTON, 98); // sounds.h:110
    assert_eq!(ui::LOAD_DOORS_CLOSING, 110); // sounds.h:122
    assert_eq!(ui::LOAD_DOORS_OPENING, 111); // sounds.h:123
}

/// The same ids in the audio crate's table, which the app plays through.
#[test]
fn audio_sound_ids_match_the_c_header() {
    assert_eq!(sfx::BUTTON5, 15); // sounds.h:30
    assert_eq!(sfx::BUZZER1, 18); // sounds.h:33
    assert_eq!(sfx::LOG_WRONGBUTTON, 89); // sounds.h:101
    assert_eq!(sfx::LOG_MAINMENUBUTTON, 95); // sounds.h:107
    assert_eq!(sfx::LOG_EXITGAME, 97); // sounds.h:109
    assert_eq!(sfx::LOG_UNREADY, 16); // sounds.h:127
}

/// `[DigitalMusic]` indices (`mclib/sounds.h:137-167`).
#[test]
fn music_ids_match_the_c_header() {
    assert_eq!(music::WIN_TUNE_0, 8);
    assert_eq!(music::LOSE_TUNE_0, 9);
    assert_eq!(music::LOGISTICS_LOOP, 22);
    assert_eq!(music::CREDITS_TUNE, 23);
    assert_eq!(music::RESULTS_LOOP, 28);
}

/// Main menu: `MainMenu::init` reads `TuneId` from the `[Tunes]` block (`code/mainmenu.cpp:100-102`,
/// default -1 at line 82), and `MainMenu::update` starts it once with `playDigitalMusic(tuneId)`
/// (`code/mainmenu.cpp:474-477`).
#[test]
fn main_menu_plays_its_fit_tune_once() {
    assert_eq!(tune::main_menu(7), Some(Cue::Music(7)));
    // -1 is `playDigitalMusic(-1)`: the `musicId >= 0` guard does nothing (`mclib/soundsys.cpp:679`).
    assert_eq!(tune::main_menu(-1), None);
}

/// Logistics tune: `LogisticsData::getCurrentMissionTune` is the stage's `Tune`
/// (`code/logisticsdata.cpp:1795-1798`, `code/logisticsmissioninfo.cpp:140,922-931`). It is -1 once
/// the campaign has run past its last stage (`logisticsmissioninfo.cpp:925-926`).
#[test]
fn logistics_plays_the_stage_tune() {
    assert_eq!(tune::logistics(20), Some(Cue::Music(20)));
    assert_eq!(tune::logistics(-1), None);
}

/// Returning to the game from the menu replays the logistics tune, not the mission's own
/// (`code/mainmenu.cpp:325-328`).
#[test]
fn return_to_game_replays_the_logistics_tune() {
    assert_eq!(tune::logistics(20), Some(Cue::Music(20)));
    assert_eq!(tune::return_to_game(20), Some(Cue::Music(20)));
    assert_eq!(tune::return_to_game(-1), None);
}

/// A movie that ends hands over to the logistics music, or to the credits tune for `cinema5`
/// (`code/logistics.cpp:285-312`: `S_strnicmp(name, "cinema5", 7)`, `playDigitalMusic(33)`).
#[test]
fn movie_end_hands_over_to_logistics_or_credits() {
    assert_eq!(tune::after_movie("cinema5", 20), Some(Cue::Music(33)));
    assert_eq!(tune::after_movie("CINEMA5_end", 20), Some(Cue::Music(33)));
    assert_eq!(tune::after_movie("cinema4", 20), Some(Cue::Music(20)));
    assert_eq!(tune::after_movie("bigvideo", -1), None);
}

/// A full-screen video stops the music first (`code/logistics.cpp:880-897`).
#[test]
fn full_screen_video_stops_the_music() {
    assert_eq!(tune::full_screen_video(), Cue::StopMusic);
}

/// Entering the results screens plays their loop (`code/missionresults.cpp:188-192`). Ending the mission
/// plays nothing: the win or lose tune already started with the termination.
#[test]
fn ending_the_mission_plays_only_the_results_loop() {
    use mc2_app::state::GameState;
    // the loop starts in `MissionResults::begin` (the UI host), not at the state change
    assert!(audio::state_change_cues(&GameState::Results(audio::MIS_PLAYER_WIN_BIG)).is_empty());
    // a loss skips the results screens and their loop
    assert!(audio::state_change_cues(&GameState::Results(1)).is_empty());
    assert!(audio::state_change_cues(&GameState::MainMenu).is_empty());
    assert!(audio::state_change_cues(&GameState::Logistics).is_empty());
}

/// The sim's `SimEvent`s have no audio mapping yet: none is dropped silently (the hook refuses them).
#[test]
fn sim_events_are_refused_until_the_sim_lanes_map_them() {
    assert_eq!(audio::sim_events::<u8>(&[]), Ok(Vec::new()));
    assert_eq!(audio::sim_events(&[7u8]), Err(audio::SimAudioUnavailable));
}

/// The results screens loop `RESULTS_LOOP` (`code/missionresults.cpp:188-192`).
#[test]
fn results_loop_is_the_results_tune() {
    assert_eq!(tune::results(), Cue::Music(28));
}

/// Loading and mission start make no tune call of their own: the loading screen plays door samples
/// (`code/loadscreen.cpp:284-286`), and the mission's tune is the ABL script's
/// (`code/ablmc2.cpp:2606-2617`), which is not run here.
#[test]
fn loading_and_mission_start_make_no_tune_cue() {
    use mc2_app::state::GameState;
    assert!(audio::state_change_cues(&GameState::MissionLoading("mc2_01".into())).is_empty());
    assert!(audio::state_change_cues(&GameState::InMission).is_empty());
}
