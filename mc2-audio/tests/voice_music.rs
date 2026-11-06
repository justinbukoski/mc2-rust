//! Betty, support officer, voice-overs and music (cross-fades).

mod common;
use common::*;
use mc2_audio::engine::AudioEvent;
use mc2_audio::rng::ScriptedRng;
use mc2_audio::system::{
    BETTY_CHANNEL, RADIO_CHANNEL, STREAM1_CHANNEL, STREAM2_CHANNEL, STREAM3_CHANNEL,
    SUPPORT_CHANNEL,
};
use mc2_audio::testutil::{dc, wav_i16};
use mc2_platform::audio::PlayMode;

fn rng() -> ScriptedRng {
    ScriptedRng::new(&[0])
}

fn betty_game() -> mc2_audio::GameAudio {
    let b = builder_with_bites(1)
        .betty(Some(dc(RATE, 0.5, 0.6)))
        .betty(Some(dc(RATE, 0.25, 0.4)))
        .betty(None)
        .support(Some(dc(RATE, 0.5, 0.3)))
        .file("vo_test.wav", wav_i16(RATE, &dc(RATE, 0.4, 0.5)));
    game(b)
}

#[test]
fn betty_plays_on_channel_14_at_betty_volume_and_ducks_the_radio() {
    let mut a = betty_game();
    a.mixer().lock().set_volume(RADIO_CHANNEL, 1.0);
    assert_eq!(a.sound.sys.play_betty_sample(0), Some(BETTY_CHANNEL));
    let m = a.mixer();
    assert!(close(m.lock().volume(BETTY_CHANNEL), MASTER * 0.5));
    // radio = master * radio(0.5) * 0.5
    assert!(close(m.lock().volume(RADIO_CHANNEL), MASTER * 0.5 * 0.5));
    assert_eq!(m.lock().pan(BETTY_CHANNEL), 0.0);
    let buf = run(&mut a, 0.25);
    assert!(close(peak(&left(&buf)), 0.6 * MASTER * 0.5));
    assert_eq!(a.sound.sys.last_betty_id(), 0);
}

#[test]
fn only_one_betty_line_at_a_time() {
    let mut a = betty_game();
    assert!(a.sound.sys.play_betty_sample(0).is_some());
    // A second request while one is loaded is dropped (Betty takes precedence, no queue).
    assert!(a.sound.sys.play_betty_sample(1).is_none());
    let _ = run(&mut a, 0.6);
    // Finished and released by the update: Betty can speak again.
    assert!(a.sound.sys.play_betty_sample(1).is_some());
    // Out of range and empty packets are refused.
    let _ = run(&mut a, 0.4);
    assert!(a.sound.sys.play_betty_sample(3).is_none());
    assert!(a.sound.sys.play_betty_sample(2).is_none());
    assert!(
        a.sound.sys.play_betty_sample(1).is_some(),
        "refusals leave Betty free"
    );
}

#[test]
fn stop_betty_stops_and_frees() {
    let mut a = betty_game();
    a.sound.sys.play_betty_sample(0);
    a.sound.sys.stop_betty_sample();
    assert_eq!(a.mixer().lock().mode(BETTY_CHANNEL), PlayMode::Stop);
    assert!(a.sound.sys.play_betty_sample(1).is_some());
}

#[test]
fn support_sample_and_voice_over() {
    let mut a = betty_game();
    assert_eq!(
        a.sound.sys.play_support_sample(0, None),
        Some(SUPPORT_CHANNEL)
    );
    assert!(close(
        a.mixer().lock().volume(SUPPORT_CHANNEL),
        MASTER * 0.5
    ));
    assert!(
        a.sound.sys.play_support_sample(0, None).is_none(),
        "one at a time"
    );
    let _ = run(&mut a, 0.6);
    assert_eq!(a.mixer().lock().mode(SUPPORT_CHANNEL), PlayMode::Stop);
    assert!(
        a.sound.sys.play_support_sample(7, None).is_none(),
        "id out of range"
    );

    // A file name plays on the voice-over stream and always reports channel 18.
    assert_eq!(
        a.sound
            .sys
            .play_support_sample(0, Some("data/sound/vo_test.WAV")),
        Some(STREAM3_CHANNEL)
    );
    assert!(a.sound.sys.is_playing_voice_over());
    let buf = run(&mut a, 0.2);
    assert!(close(peak(&left(&buf)), 0.5 * MASTER * 0.5));
    // While it is playing a second voice-over is refused.
    assert!(a.sound.sys.play_digital_stream("vo_test").is_err());
    let _ = run(&mut a, 0.4);
    assert!(
        !a.sound.sys.is_playing_voice_over(),
        "released when finished"
    );
    // A missing file fails, but the support call still reports the stream channel.
    assert_eq!(
        a.sound.sys.play_support_sample(0, Some("nope")),
        Some(STREAM3_CHANNEL)
    );
    assert!(!a.sound.sys.is_playing_voice_over());
}

fn music_game() -> mc2_audio::GameAudio {
    let b = builder_with_bites(1)
        // 0: looping 1 s at 0.5, track volume 0.8; 1: looping 1 s at 0.25; 2: one-shot 0.5 s; 3: NONE
        .music("tuneA", dc(RATE, 1.0, 0.5), true, 0.8)
        .music("tuneB", dc(RATE, 1.0, 0.25), true, 1.0)
        .music("tuneC", dc(RATE, 0.5, 0.5), false, 1.0)
        .music("NONE", vec![], false, 1.0)
        .fade_time(1.0);
    game(b)
}

fn vol(a: &mc2_audio::GameAudio, ch: usize) -> f32 {
    a.mixer().lock().volume(ch)
}

#[test]
fn music_fades_in_over_the_stream_fade_time() {
    let mut a = music_game();
    a.sound.sys.play_digital_music(0).unwrap();
    // Created but not started until the next update, at volume 0.
    assert!(a.sound.sys.music_stream_state(0).has_resource);
    assert!(!a.sound.sys.music_stream_state(0).active);
    assert_eq!(vol(&a, STREAM1_CHANNEL), 0.0);
    assert_eq!(a.mixer().lock().mode(STREAM1_CHANNEL), PlayMode::Stop);

    let steady = MASTER * 0.5 * 0.8;
    let mut levels = vec![];
    for _ in 0..24 {
        a.update(0.05);
        levels.push(vol(&a, STREAM1_CHANNEL));
    }
    assert_eq!(
        a.mixer().lock().mode(STREAM1_CHANNEL),
        PlayMode::Loop,
        "looping tune"
    );
    // After k steps of 0.05 s: (fade - (1 - 0.05k)) / fade = 0.05k of the steady level.
    for (k, v) in levels.iter().take(19).enumerate() {
        let want = steady * 0.05 * (k + 1) as f32;
        assert!(close(*v, want), "step {k}: {v} vs {want}");
    }
    assert!(
        close(*levels.last().unwrap(), steady),
        "full level after the fade"
    );
    assert_eq!(a.sound.sys.current_music_id(), 0);
    assert!(a.sound.sys.is_digital_music_playing());
}

#[test]
fn second_tune_cross_fades_on_the_other_stream() {
    let mut a = music_game();
    a.sound.sys.play_digital_music(0).unwrap();
    for _ in 0..30 {
        a.update(0.05);
    }
    // Same tune again is refused.
    assert!(a.sound.sys.play_digital_music(0).is_err());
    a.sound.sys.play_digital_music(1).unwrap();
    assert_eq!(a.sound.sys.music_stream_state(0).fade_time, -1.0);
    assert_eq!(a.sound.sys.music_stream_state(1).fade_time, 1.0);
    assert_eq!(a.sound.sys.current_music_id(), 1);
    // Both streams busy: a third tune is refused until one finishes.
    a.update(0.05);
    assert!(a.sound.sys.play_digital_music(2).is_err());

    let steady_b = MASTER * 0.5 * 1.0;
    // Half way through (10 more steps since the first one above): the old one is at 1 - 0.55 of its
    // level (using the *new* tune's volume, 1.0, as the C++ does), the new at 0.55.
    for _ in 0..10 {
        a.update(0.05);
    }
    assert!(
        close(vol(&a, STREAM1_CHANNEL), MASTER * 0.5 * 0.45),
        "{}",
        vol(&a, STREAM1_CHANNEL)
    );
    assert!(close(vol(&a, STREAM2_CHANNEL), steady_b * 0.55));
    // Combined power stays continuous: the two levels sum to the steady level.
    assert!(close(
        vol(&a, STREAM1_CHANNEL) + vol(&a, STREAM2_CHANNEL),
        MASTER * 0.5
    ));
    // After the fade the old stream is gone.
    for _ in 0..12 {
        a.update(0.05);
    }
    assert!(!a.sound.sys.music_stream_state(0).has_resource);
    assert_eq!(a.mixer().lock().mode(STREAM1_CHANNEL), PlayMode::Stop);
    assert!(close(vol(&a, STREAM2_CHANNEL), steady_b));
    // And a new tune now goes to stream 1 again, fading stream 2 out.
    a.sound.sys.play_digital_music(0).unwrap();
    assert!(a.sound.sys.music_stream_state(0).has_resource);
    assert_eq!(a.sound.sys.music_stream_state(1).fade_time, -1.0);
}

#[test]
fn cross_fade_of_a_tune_that_was_still_fading_in_reverses_from_its_current_point() {
    let mut a = music_game();
    a.sound.sys.play_digital_music(0).unwrap();
    for _ in 0..10 {
        a.update(0.05); // 0.5 s into the fade-in
    }
    let t = a.sound.sys.music_stream_state(0).fade_time;
    assert!(close(t, 0.5), "{t}");
    a.sound.sys.play_digital_music(1).unwrap();
    assert!(close(a.sound.sys.music_stream_state(0).fade_time, -0.5));
}

#[test]
fn one_shot_tune_ends_and_clears_the_current_id() {
    let mut a = music_game();
    a.sound.sys.play_digital_music(2).unwrap();
    assert_eq!(a.sound.sys.current_music_id(), 2);
    let _ = run(&mut a, 1.0); // 0.5 s tune
    assert_eq!(a.mixer().lock().mode(STREAM1_CHANNEL), PlayMode::Stop);
    let _ = run(&mut a, 0.1);
    assert_eq!(a.sound.sys.current_music_id(), -1);
    assert!(!a.sound.sys.is_digital_music_playing());
}

#[test]
fn looping_tune_keeps_playing_and_stop_music_ends_it() {
    let mut a = music_game();
    a.sound.sys.play_digital_music(0).unwrap();
    let buf = run(&mut a, 3.0); // three times its length
    assert!(peak(&left(&buf)[(2.5 * RATE as f32) as usize..]) > 0.0);
    a.sound.sys.stop_digital_music();
    assert_eq!(a.sound.sys.current_music_id(), -1);
    assert_eq!(a.mixer().lock().mode(STREAM1_CHANNEL), PlayMode::Stop);
}

#[test]
fn music_requests_that_do_nothing() {
    let mut a = music_game();
    // NONE entries, ids out of range, and a tune whose file is missing all return success without effect.
    assert!(a.sound.sys.play_digital_music(3).is_ok());
    assert!(a.sound.sys.play_digital_music(99).is_ok());
    assert!(a.sound.sys.play_digital_music(-1).is_ok());
    assert!(!a.sound.sys.is_digital_music_playing());
    a.sound.sys.use_music = false;
    assert!(a.sound.sys.play_digital_music(0).is_ok());
    assert!(!a.sound.sys.is_digital_music_playing());
}

#[test]
fn music_volume_change_applies_to_running_streams() {
    let mut a = music_game();
    a.sound.sys.play_digital_music(0).unwrap();
    for _ in 0..30 {
        a.update(0.05);
    }
    a.sound.sys.set_music_volume(64);
    // `setMusicVolume` writes master * music (no per-tune volume), as the C++.
    assert!(close(vol(&a, STREAM1_CHANNEL), MASTER * 0.25));
}

#[test]
fn events_drive_betty_music_and_stops() {
    let mut a = betty_game();
    let mut r = rng();
    a.handle(&AudioEvent::Betty(0), &mut r);
    assert!(a.sound.sys.is_channel_playing(BETTY_CHANNEL));
    a.handle(&AudioEvent::StopBetty, &mut r);
    assert!(!a.sound.sys.is_channel_playing(BETTY_CHANNEL));
    a.handle(&AudioEvent::Support(0), &mut r);
    assert!(a.sound.sys.is_channel_playing(SUPPORT_CHANNEL));
    // A support sample that is still loaded blocks a voice-over too (`supportSoundBite == NULL` guard).
    a.handle(&AudioEvent::VoiceOver("vo_test".into()), &mut r);
    assert!(!a.sound.sys.is_playing_voice_over());
    let _ = run(&mut a, 0.6);
    a.handle(&AudioEvent::VoiceOver("vo_test".into()), &mut r);
    assert!(a.sound.sys.is_playing_voice_over());
    a.handle(&AudioEvent::StopSupport, &mut r);
    assert!(!a.sound.sys.is_playing_voice_over());
}
