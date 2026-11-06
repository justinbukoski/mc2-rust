//! Sound effects through the full stack: table -> channels -> mixer -> rendered buffer.

mod common;
use common::*;
use mc2_audio::attenuation::{Listener, Vec3};
use mc2_audio::engine::AudioEvent;
use mc2_audio::ids::sfx;
use mc2_audio::rng::ScriptedRng;
use mc2_audio::system::{FIRE_CHANNEL, SUPPORT_CHANNEL};
use mc2_audio::testutil::{dc, wav_i16};
use mc2_platform::audio::PlayMode;

fn sfx_game() -> mc2_audio::GameAudio {
    // bite 1: 1 s at 0.5, volume 1.0; bite 2: 1 s at 0.5, volume 0.5; bite 3: 0.2 s at 1.0; bite 4: empty
    let b = builder_with_bites(0)
        .bite(None, 1.0)
        .bite(Some(dc(RATE, 1.0, 0.5)), 1.0)
        .bite(Some(dc(RATE, 1.0, 0.5)), 0.5)
        .bite(Some(dc(RATE, 0.2, 1.0)), 1.0)
        .bite(None, 1.0);
    game(b)
}

#[test]
fn unpositioned_sound_uses_master_bite_and_sfx_volume() {
    let mut a = sfx_game();
    let ch = a.sound.sys.play_digital_sample(2, None, false).unwrap();
    assert_eq!(ch, 1);
    let buf = run(&mut a, 0.5);
    // sample 0.5 * (255/256 master) * bite 0.5 * (128/256 sfx)
    let want = 0.5 * MASTER * 0.5 * 0.5;
    assert!(
        close(peak(&left(&buf)), want),
        "{} vs {want}",
        peak(&left(&buf))
    );
    assert!(close(peak(&right(&buf)), want));
}

#[test]
fn positional_attenuation_and_pan_reach_the_mix() {
    let mut a = sfx_game();
    a.set_listener(Some(Listener {
        position: Vec3::new(0.0, 0.0, 0.0),
        rotation_y: 0.0,
    }));
    // 750 units straight ahead: distance volume 0.5, centred.
    a.sound
        .sys
        .play_digital_sample(1, Some(Vec3::new(0.0, 750.0, 0.0)), false)
        .unwrap();
    let buf = run(&mut a, 0.25);
    let want = 0.5 * MASTER * 0.5 * 0.5; // sample * master * (bite 1.0 * 0.5) * sfx
    assert!(close(peak(&left(&buf)), want));
    assert!(close(peak(&right(&buf)), want));

    // Camera minus source has x = -375: pan -0.5, so the right side is attenuated by half.
    let mut a = sfx_game();
    a.set_listener(Some(Listener::default()));
    a.sound
        .sys
        .play_digital_sample(1, Some(Vec3::new(375.0, 0.0, 0.0)), false)
        .unwrap();
    let buf = run(&mut a, 0.25);
    let dv = (1500.0 - 375.0) / 1500.0;
    let v = 0.5 * MASTER * dv * 0.5;
    assert!(close(peak(&left(&buf)), v));
    assert!(close(peak(&right(&buf)), v * 0.5));
}

#[test]
fn too_far_sounds_do_not_play_or_take_a_channel() {
    let mut a = sfx_game();
    a.set_listener(Some(Listener::default()));
    assert!(a
        .sound
        .sys
        .play_digital_sample(1, Some(Vec3::new(1500.0, 0.0, 0.0)), false)
        .is_none());
    assert!(!a.sound.sys.is_playing_sample(1));
    // Without a listener (`eye == NULL`) the position is ignored.
    a.set_listener(None);
    assert!(a
        .sound
        .sys
        .play_digital_sample(1, Some(Vec3::new(9000.0, 0.0, 0.0)), false)
        .is_some());
}

#[test]
fn duplicates_channels_and_completion() {
    let mut a = sfx_game();
    let s = &mut a.sound.sys;
    assert_eq!(s.play_digital_sample(1, None, false), Some(1));
    assert!(s.is_playing_sample(1));
    // Same bite again is refused unless dupes are allowed.
    assert_eq!(s.play_digital_sample(1, None, false), None);
    assert_eq!(s.play_digital_sample(1, None, true), Some(2));
    // Out of range ids.
    assert_eq!(s.play_digital_sample(99, None, false), None);
    // Channels 1..=12 are the pool; 12 dupes fill it and the 13th request fails.
    for expect in 3..=12 {
        assert_eq!(s.play_digital_sample(1, None, true), Some(expect));
    }
    assert_eq!(s.play_digital_sample(1, None, true), None);
    assert_eq!(s.find_open_channel(1, SUPPORT_CHANNEL), None);

    // After the sample ends and an update runs, the channels are free again.
    let _ = run(&mut a, 1.2);
    assert!(!a.sound.sys.is_playing_sample(1));
    assert_eq!(a.sound.sys.play_digital_sample(1, None, false), Some(1));
}

#[test]
fn empty_packet_claims_a_channel_but_is_silent() {
    let mut a = sfx_game();
    assert_eq!(a.sound.sys.play_digital_sample(4, None, false), Some(1));
    let buf = run(&mut a, 0.2);
    assert_eq!(peak(&buf), 0.0);
    // The next update released it.
    assert!(!a.sound.sys.is_playing_sample(4));
}

#[test]
fn zero_volume_bite_is_not_played() {
    let b = builder_with_bites(0).bite(Some(dc(RATE, 0.5, 0.5)), 0.0);
    let mut a = game(b);
    assert_eq!(a.sound.sys.play_digital_sample(0, None, false), None);
}

#[test]
fn stop_fades_the_channel_in_tenth_steps() {
    let mut a = sfx_game();
    // Master 255/256 * bite 1.0 * sfx 0.5 = about 0.498.
    let ch = a.sound.sys.play_digital_sample(1, None, false).unwrap();
    let start = a.mixer().lock().volume(ch);
    assert!(close(start, MASTER * 0.5));
    a.sound.sys.stop_digital_sample(ch);
    let mut vols = vec![];
    for _ in 0..8 {
        a.update(DT);
        vols.push(a.mixer().lock().volume(ch));
        if a.mixer().lock().mode(ch) == PlayMode::Stop {
            break;
        }
    }
    // 0.498 -> 0.398 -> 0.298 -> 0.198 -> 0.098 -> (clamped to 0.1 then 0.0) -> stopped
    assert!(close(vols[0], start - 0.1), "{vols:?}");
    assert!(close(vols[1], start - 0.2));
    assert_eq!(*vols.last().unwrap(), 0.0, "{vols:?}");
    assert_eq!(a.mixer().lock().mode(ch), PlayMode::Stop);
}

#[test]
fn rain_loops_on_the_fire_channel_until_purged() {
    let mut a = sfx_game();
    a.sound.sys.set_is_raining(3);
    a.update(DT);
    assert_eq!(a.mixer().lock().mode(FIRE_CHANNEL), PlayMode::Loop);
    let buf = run(&mut a, 1.0); // five times the 0.2 s sample
    assert!(peak(&buf[buf.len() - 400..]) > 0.0, "still looping");
    // As in the C++, clearing the rain flag does not stop a started loop.
    a.sound.sys.set_is_raining(0);
    let buf = run(&mut a, 0.5);
    assert!(peak(&buf) > 0.0);
    // Purging the sound system does.
    let mut rng = ScriptedRng::new(&[0]);
    a.handle(&AudioEvent::Purge, &mut rng);
    assert_eq!(a.mixer().lock().mode(FIRE_CHANNEL), PlayMode::Stop);
}

#[test]
fn sensor_beep_repeats_every_five_seconds_while_a_contact_is_held() {
    let b = builder_with_bites(sfx::SENSOR2 as usize).bite(Some(dc(RATE, 0.05, 0.5)), 1.0); // index 86
    let mut a = game(b);
    let mut rng = ScriptedRng::new(&[0]);
    a.handle(&AudioEvent::SensorContact(2), &mut rng);
    // The timer is `> 5.0`, so no beep until just past 5 s.
    let mut first = None;
    for k in 0..200 {
        a.update(DT);
        if a.sound.sys.is_playing_sample(sfx::SENSOR2) && first.is_none() {
            first = Some(k as f32 * DT);
        }
    }
    let t = first.expect("beeped");
    assert!((5.0..5.3).contains(&t), "first beep at {t}");
    // No contact: no beeps.
    a.handle(&AudioEvent::SensorContact(-1), &mut rng);
    let _ = run(&mut a, 6.0);
    a.update(DT);
    assert!(!a.sound.sys.is_playing_sample(sfx::SENSOR2));
}

#[test]
fn general_alarm_buzzes_for_five_seconds() {
    let b = builder_with_bites(sfx::BUZZER1 as usize).bite(Some(dc(RATE, 0.5, 0.5)), 1.0);
    let mut a = game(b);
    let mut rng = ScriptedRng::new(&[0]);
    a.handle(&AudioEvent::GeneralAlarm(true), &mut rng);
    let buf = run(&mut a, 8.0);
    let l = left(&buf);
    let sec = |s: f32| &l[(s * RATE as f32) as usize..((s + 0.5) * RATE as f32) as usize];
    assert!(peak(sec(0.0)) > 0.0);
    assert!(peak(sec(4.0)) > 0.0);
    assert_eq!(
        peak(&l[(5.6 * RATE as f32) as usize..]),
        0.0,
        "alarm over after ALARM_TIME"
    );
}

#[test]
fn volume_groups_follow_the_option_bytes() {
    let mut a = sfx_game();
    a.sound.sys.set_sfx_volume(255);
    a.sound.sys.set_digital_master_volume(128);
    assert_eq!(a.sound.sys.sfx_volume(), 255);
    let ch = a.sound.sys.play_digital_sample(1, None, false).unwrap();
    // 128/256 master * bite 1.0 * 255/256 sfx
    assert!(close(a.mixer().lock().volume(ch), 0.5 * 255.0 / 256.0));
    a.sound.sys.use_sound = false;
    a.sound.sys.set_sfx_volume(7);
    assert_eq!(
        a.sound.sys.sfx_volume(),
        0,
        "getters report 0 without sound"
    );
    let _ = wav_i16;
}
