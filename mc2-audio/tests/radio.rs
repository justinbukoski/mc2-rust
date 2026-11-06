//! Pilot radio: voice sets, static, fragments, priorities and the message queue.

mod common;
use common::*;
use mc2_audio::engine::AudioEvent;
use mc2_audio::game::RadioEvent;
use mc2_audio::radio::{PilotId, RadioClock, RadioMessageType as M};
use mc2_audio::rng::ScriptedRng;
use mc2_audio::system::{BETTY_CHANNEL, RADIO_CHANNEL};
use mc2_audio::testutil::dc;
use mc2_audio::GameAudio;
use std::sync::Arc;

const NOISE_LEVEL: f32 = 0.9;

/// Voice `pilota`: packet i is a 0.1 s level of 0.02 * (i + 1). Noise: 0.05 s at 0.9.
fn radio_game() -> GameAudio {
    let mut voice = Vec::new();
    for i in 0..42 {
        voice.push(Some(dc(RATE, 0.1, 0.02 * (i + 1) as f32)));
    }
    let mut sparse: Vec<Option<Vec<i16>>> = vec![None; 42];
    sparse[0] = Some(dc(RATE, 0.1, 0.3));
    sparse[16] = Some(dc(RATE, 0.1, 0.4));
    let b = builder_with_bites(1)
        .noise(Some(dc(RATE, 0.05, NOISE_LEVEL)))
        .noise(Some(dc(RATE, 0.05, 0.5)))
        .noise(Some(dc(RATE, 0.05, 0.5)))
        .noise(Some(dc(RATE, 0.05, 0.5)))
        .voice("pilota", voice.clone())
        .voice("pilotb", voice)
        .voice("pilot1a", sparse)
        .betty(Some(dc(RATE, 1.0, 0.2)));
    let mut a = game(b);
    a.set_clock(RadioClock {
        turn: 1,
        scenario_time: 10.0,
    });
    a
}

fn say(a: &mut GameAudio, pilot: u32, m: M, rng: &mut ScriptedRng) {
    a.handle(
        &AudioEvent::Radio {
            pilot: PilotId(pilot),
            message: m.index() as i64,
            under_home_command: true,
        },
        rng,
    );
}

fn queue_types(a: &GameAudio) -> Vec<M> {
    a.sound.queued_messages().map(|m| m.msg_type).collect()
}

fn rng0() -> ScriptedRng {
    ScriptedRng::new(&[10])
}

#[test]
fn message_is_static_then_the_line_with_exact_timing() {
    let mut a = radio_game();
    assert!(a.add_pilot(PilotId(1), "pilota"));
    let mut rng = rng0();
    say(&mut a, 1, M::Ack, &mut rng); // 3 styles, roll 10 -> style 0 -> packet 0
    assert_eq!(rng.calls, vec![100]);
    assert_eq!(queue_types(&a), vec![M::Ack]);
    let buf = run(&mut a, 0.5);
    let l = left(&buf);
    let gain = MASTER * 0.5; // master * radio volume
    let r = RATE as usize;
    // 0.05 s of static at the noise level, then 0.1 s of packet 0 (0.02), then nothing.
    let seg = |a: f32, b: f32| &l[(a * r as f32) as usize..(b * r as f32) as usize];
    assert!(seg(0.0, 0.05).iter().all(|v| close(*v, NOISE_LEVEL * gain)));
    assert!(seg(0.05, 0.15).iter().all(|v| close(*v, 0.02 * gain)));
    assert!(seg(0.15, 0.5).iter().all(|v| *v == 0.0));
    // The pilot is flagged speaking while it plays and silent afterwards.
    let ev = a.take_radio_events();
    assert!(ev.contains(&RadioEvent::PilotSpeaking(PilotId(1))));
    assert_eq!(ev.last(), Some(&RadioEvent::PilotSilent(PilotId(1))));
    assert_eq!(a.sound.messages_in_queue(), 0);
    assert!(a.sound.current_message().is_none());
}

#[test]
fn callsign_then_line_and_the_extra_rng_draw() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    // SENSOR_CONTACT: 2 styles (50/50), identifies the pilot, video code '1'. Roll 70 -> style 1 (packet 17).
    let mut rng = ScriptedRng::new(&[70, 5]);
    say(&mut a, 1, M::SensorContact, &mut rng);
    assert_eq!(
        rng.calls,
        vec![100, 100],
        "style roll, then the callsign roll"
    );
    let m = a.sound.queued_messages().next().unwrap();
    assert_eq!(m.msg_id, 17);
    assert_eq!(m.fragment_count(), 2);
    let buf = run(&mut a, 0.6);
    let l = left(&buf);
    let g = MASTER * 0.5;
    let r = RATE as f32;
    let seg = |a: f32, b: f32| &l[(a * r) as usize..(b * r) as usize];
    // static, callsign (packet 37 regardless of the roll), line (packet 17)
    assert!(seg(0.0, 0.05).iter().all(|v| close(*v, NOISE_LEVEL * g)));
    assert!(seg(0.05, 0.15).iter().all(|v| close(*v, 0.02 * 38.0 * g)));
    assert!(seg(0.15, 0.25).iter().all(|v| close(*v, 0.02 * 18.0 * g)));
    assert_eq!(peak(&l[(0.26 * r) as usize..]), 0.0);
    let ev = a.take_radio_events();
    assert!(ev.contains(&RadioEvent::PilotVideoStart {
        pilot: PilotId(1),
        movie_code: b'1'
    }));
    assert!(ev.contains(&RadioEvent::PilotVideoEnd));
}

#[test]
fn style_rolls_pick_by_cumulative_chance_and_avoid_repeating_the_last_line() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    let table = a.radio_table().clone();
    let ack = table.get(M::Ack).clone();
    assert_eq!(ack.style_chance, [33, 33, 34]);
    for (roll, want) in [(0, 0u32), (32, 0), (33, 1), (65, 1), (66, 2), (99, 2)] {
        let mut a = radio_game();
        a.add_pilot(PilotId(1), "pilota");
        let mut rng = ScriptedRng::new(&[roll]);
        say(&mut a, 1, M::Ack, &mut rng);
        assert_eq!(
            a.sound.queued_messages().next().unwrap().msg_id,
            want,
            "roll {roll}"
        );
    }
    // The previous line is skipped: after saying packet 0, a roll for style 0 gives style 1.
    let mut rng = ScriptedRng::new(&[10]);
    a.pilot_radio_mut(PilotId(1)).unwrap().log.last_message = 0;
    say(&mut a, 1, M::Ack, &mut rng);
    assert_eq!(a.sound.queued_messages().next().unwrap().msg_id, 1);
    // ... and after the last style it wraps to 0.
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    a.pilot_radio_mut(PilotId(1)).unwrap().log.last_message = 2;
    let mut rng = ScriptedRng::new(&[99]);
    say(&mut a, 1, M::Ack, &mut rng);
    assert_eq!(a.sound.queued_messages().next().unwrap().msg_id, 0);
}

#[test]
fn single_style_messages_without_callsign_draw_no_random_numbers() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    let mut rng = rng0();
    say(&mut a, 1, M::Guard, &mut rng);
    assert!(rng.calls.is_empty());
    assert_eq!(a.sound.queued_messages().next().unwrap().msg_id, 34);
}

#[test]
fn the_queue_is_ordered_by_priority_most_urgent_first() {
    let mut a = radio_game();
    for p in 1..=4 {
        a.add_pilot(PilotId(p), "pilota");
    }
    let mut rng = rng0();
    say(&mut a, 1, M::Taunt, &mut rng); // 4
    say(&mut a, 2, M::Ack, &mut rng); // 5
    say(&mut a, 3, M::UnderAttack, &mut rng); // 1
    say(&mut a, 4, M::CalledShot, &mut rng); // 3
    assert_eq!(
        queue_types(&a),
        vec![M::UnderAttack, M::CalledShot, M::Taunt, M::Ack]
    );
    // Equal priority queues behind existing messages of the same priority.
    say(&mut a, 5, M::Guard, &mut rng); // pilot 5 has no radio: ignored
    assert_eq!(a.sound.messages_in_queue(), 4);
}

#[test]
fn check_message_rules() {
    let mut a = radio_game();
    for p in 1..=3 {
        a.add_pilot(PilotId(p), "pilota");
    }
    let mut rng = rng0();
    say(&mut a, 1, M::Guard, &mut rng); // priority 2
                                        // Same pilot, less urgent (larger number) than what he has queued: refused.
    say(&mut a, 1, M::Taunt, &mut rng);
    assert_eq!(queue_types(&a), vec![M::Guard]);
    // Same pilot, more urgent: fine.
    say(&mut a, 1, M::UnderAirstrike, &mut rng); // 1
    assert_eq!(queue_types(&a), vec![M::UnderAirstrike, M::Guard]);
    // Someone else already has this (non-top-priority) type queued: refused.
    say(&mut a, 2, M::Guard, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 2);
    // But top-priority (1) types may be queued by several pilots (in the same turn they would name the
    // same voice packet, and the later one replaces the earlier: see the dedupe test).
    a.set_clock(RadioClock {
        turn: 2,
        scenario_time: 10.1,
    });
    say(&mut a, 2, M::UnderAirstrike, &mut rng);
    a.set_clock(RadioClock {
        turn: 3,
        scenario_time: 10.2,
    });
    say(&mut a, 3, M::UnderAirstrike, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 4);
}

#[test]
fn same_voice_packet_in_the_same_turn_replaces_the_older_message() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    a.add_pilot(PilotId(2), "pilota");
    let mut rng = rng0();
    // RADIO_CAPTURED_VEHICLE and RADIO_AIRSTRIKE both map to packet 39.
    say(&mut a, 1, M::CapturedVehicle, &mut rng);
    say(&mut a, 2, M::Airstrike, &mut rng);
    assert_eq!(queue_types(&a), vec![M::Airstrike]);
    // A different turn keeps both.
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    a.add_pilot(PilotId(2), "pilota");
    say(&mut a, 1, M::CapturedVehicle, &mut rng);
    a.set_clock(RadioClock {
        turn: 2,
        scenario_time: 10.1,
    });
    say(&mut a, 2, M::Airstrike, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 2);
}

#[test]
fn priority_zero_cancels_the_playing_message_and_the_pilots_other_chatter() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    a.add_pilot(PilotId(2), "pilota");
    let mut rng = rng0();
    say(&mut a, 1, M::Ack, &mut rng);
    a.update(DT); // starts playing
    assert!(a.sound.current_message().is_some());
    assert!(a.mixer().lock().mode(RADIO_CHANNEL) != mc2_platform::audio::PlayMode::Stop);
    say(&mut a, 1, M::Guard, &mut rng);
    say(&mut a, 2, M::Taunt, &mut rng);
    assert_eq!(queue_types(&a), vec![M::Guard, M::Taunt]);
    a.take_radio_events();

    say(&mut a, 1, M::Death, &mut rng); // priority 0
    assert!(
        a.sound.current_message().is_none(),
        "playing message cancelled"
    );
    assert_eq!(
        a.mixer().lock().mode(RADIO_CHANNEL),
        mc2_platform::audio::PlayMode::Stop
    );
    assert_eq!(
        queue_types(&a),
        vec![M::Death, M::Taunt],
        "pilot 1's Guard removed"
    );
    assert!(a
        .take_radio_events()
        .contains(&RadioEvent::PilotSilent(PilotId(1))));
}

#[test]
fn full_queue_drops_the_least_urgent_when_something_more_urgent_arrives() {
    let mut a = radio_game();
    // Eight distinct non-top-priority types from eight pilots, in decreasing urgency so each one is
    // appended: the queue is [Guard(2) ... Ack(5)]. (Inserting at the front of a queue that already holds
    // 7 would drop the last entry, so 8 is only reachable by appending.)
    let types = [
        M::Guard,       // 2
        M::MoveBlocked, // 2
        M::PilotHurt,   // 3
        M::ObjectDead,  // 3
        M::CalledShot,  // 3
        M::TargetAck,   // 4
        M::Taunt,       // 4
        M::Ack,         // 5
    ];
    for (i, t) in types.iter().enumerate() {
        a.add_pilot(PilotId(i as u32 + 1), "pilota");
        let mut rng = rng0();
        say(&mut a, i as u32 + 1, *t, &mut rng);
    }
    assert_eq!(a.sound.messages_in_queue(), 8);
    let before = queue_types(&a);
    assert_eq!(before.first(), Some(&M::Guard));
    assert_eq!(before.last(), Some(&M::Ack));

    // A message that is not more urgent than anything queued cannot get in.
    a.add_pilot(PilotId(20), "pilota");
    let mut rng = rng0();
    say(&mut a, 20, M::Callsign, &mut rng); // priority 5, nothing is > 5
    assert_eq!(queue_types(&a), before);

    // A more urgent message goes in at the front. As in the C++ the shift pushes the two last
    // entries off the end (7 entries remain).
    a.add_pilot(PilotId(21), "pilota");
    say(&mut a, 21, M::UnderAirstrike, &mut rng); // priority 1
    let after = queue_types(&a);
    assert_eq!(after.len(), 7);
    assert_eq!(after[0], M::UnderAirstrike);
    assert_eq!(&after[1..], &before[..6]);
    assert_eq!(a.sound.messages_in_queue(), 7);
}

#[test]
fn messages_play_one_after_another_most_urgent_first() {
    let mut a = radio_game();
    for p in 1..=3 {
        a.add_pilot(PilotId(p), "pilota");
    }
    let mut rng = rng0();
    say(&mut a, 1, M::Taunt, &mut rng); // packet 31, level 0.02*32
    say(&mut a, 2, M::UnderAirstrike, &mut rng); // packet 21, level 0.02*22
    let buf = run(&mut a, 1.0);
    let l = left(&buf);
    let g = MASTER * 0.5;
    let r = RATE as f32;
    // First: UnderAirstrike (urgent), static 0.05, line 0.1. The next message starts one update after
    // the previous ended.
    assert!(close(l[(0.1 * r) as usize], 0.02 * 22.0 * g));
    // Find the second line's level somewhere later.
    let later = &l[(0.2 * r) as usize..];
    assert!(later.iter().any(|v| close(*v, 0.02 * 32.0 * g)));
    assert!(a.sound.current_message().is_none());
    assert_eq!(a.sound.messages_in_queue(), 0);
}

#[test]
fn radio_is_halved_while_betty_speaks() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    let mut r = rng0();
    a.handle(&AudioEvent::Betty(0), &mut r);
    say(&mut a, 1, M::Guard, &mut r);
    let buf = run(&mut a, 0.1);
    let l = left(&buf);
    let g = MASTER * 0.5 * 0.5;
    // Betty (0.2 * 0.498) is added to the radio static (halved).
    let betty = 0.2 * MASTER * 0.5;
    assert!(close(l[10], betty + NOISE_LEVEL * g), "{}", l[10]);
    assert!(a.sound.sys.is_channel_playing(BETTY_CHANNEL));
}

#[test]
fn messages_from_inactive_pilots_are_dropped_except_death_and_ejection() {
    let mut a = radio_game();
    for p in 1..=3 {
        a.add_pilot(PilotId(p), "pilota");
    }
    let mut rng = rng0();
    say(&mut a, 1, M::Guard, &mut rng);
    say(&mut a, 2, M::Ack, &mut rng);
    say(&mut a, 3, M::Ejecting, &mut rng); // priority 0: plays first
    a.set_pilot_active(PilotId(1), false);
    a.set_pilot_active(PilotId(2), false);
    a.set_pilot_active(PilotId(3), false);
    a.update(DT);
    assert_eq!(a.sound.current_message().unwrap().msg_type, M::Ejecting);
    let _ = run(&mut a, 1.0);
    assert_eq!(a.sound.messages_in_queue(), 0);
    assert!(a.sound.current_message().is_none());
    // Nothing else was said: the remaining two were dropped when they reached the head.
    let ev = a.take_radio_events();
    assert!(!ev
        .iter()
        .any(|e| matches!(e, RadioEvent::PilotSpeaking(PilotId(1) | PilotId(2)))));
}

#[test]
fn radio_message_throttles_like_the_warrior_code() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    let mut rng = rng0();
    let req = |m: M, home: bool| AudioEvent::Radio {
        pilot: PilotId(1),
        message: m.index() as i64,
        under_home_command: home,
    };
    // Pilots outside the home command (the enemy) never speak, whatever the message.
    a.set_clock(RadioClock {
        turn: 3,
        scenario_time: 5.0,
    });
    for m in M::ALL {
        a.handle(&req(m, false), &mut rng);
    }
    assert_eq!(a.sound.messages_in_queue(), 0);

    // Turn 0 (mission not started): silent.
    a.set_clock(RadioClock {
        turn: 0,
        scenario_time: 0.0,
    });
    a.handle(&req(M::Guard, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0);
    a.set_clock(RadioClock {
        turn: 5,
        scenario_time: 20.0,
    });

    // The same message type within 10 s is dropped.
    a.handle(&req(M::Guard, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
    let _ = run(&mut a, 0.5);
    a.set_clock(RadioClock {
        turn: 6,
        scenario_time: 25.0,
    });
    a.handle(&req(M::Guard, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0, "5 s later: throttled");
    a.set_clock(RadioClock {
        turn: 7,
        scenario_time: 30.5,
    });
    a.handle(&req(M::Guard, true), &mut rng);
    assert_eq!(
        a.sound.messages_in_queue(),
        1,
        "more than 10 s later: allowed"
    );
    let _ = run(&mut a, 0.5);

    // TARGET_ACK is never frequency limited.
    a.handle(&req(M::TargetAck, true), &mut rng);
    let _ = run(&mut a, 0.5);
    a.set_clock(RadioClock {
        turn: 8,
        scenario_time: 31.0,
    });
    a.handle(&req(M::TargetAck, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
    let _ = run(&mut a, 0.5);

    // Weapons-out once; sensor contact once per 15 s; under attack per 20 s.
    a.set_clock(RadioClock {
        turn: 9,
        scenario_time: 100.0,
    });
    a.handle(&req(M::WeaponsOut, true), &mut rng);
    let _ = run(&mut a, 0.5);
    a.set_clock(RadioClock {
        turn: 10,
        scenario_time: 200.0,
    });
    a.handle(&req(M::WeaponsOut, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0, "weapons out is said once");
    a.handle(&req(M::SensorContact, true), &mut rng);
    let _ = run(&mut a, 0.6);
    a.set_clock(RadioClock {
        turn: 11,
        scenario_time: 205.0,
    });
    a.handle(&req(M::SensorContact, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0, "contact within 15 s");
    // Under attack: once per 20 s (other types do not share the clock).
    a.set_clock(RadioClock {
        turn: 20,
        scenario_time: 300.0,
    });
    a.handle(&req(M::UnderAttack, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
    let _ = run(&mut a, 0.6);
    a.set_clock(RadioClock {
        turn: 21,
        scenario_time: 315.0,
    });
    a.handle(&req(M::UnderAttack, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0, "within 20 s");
    a.set_clock(RadioClock {
        turn: 22,
        scenario_time: 321.0,
    });
    a.handle(&req(M::UnderAttack, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
    let _ = run(&mut a, 0.6);

    // Contact again after 15 s is fine.
    a.set_clock(RadioClock {
        turn: 12,
        scenario_time: 216.0,
    });
    a.handle(&req(M::SensorContact, true), &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
}

#[test]
fn inactive_pilot_does_not_speak_and_ammo_out_is_said_once() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    let mut rng = rng0();
    a.set_pilot_active(PilotId(1), false);
    say(&mut a, 1, M::Guard, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0);
    a.set_pilot_active(PilotId(1), true);

    say(&mut a, 1, M::AmmoOut, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
    let _ = run(&mut a, 0.6);
    a.set_clock(RadioClock {
        turn: 99,
        scenario_time: 500.0,
    });
    say(&mut a, 1, M::AmmoOut, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0, "ammo out only once");
    a.pilot_radio_mut(PilotId(1)).unwrap().reset_ammo_message();
    // (The 10 s same-type throttle counts the refused attempt too.)
    a.set_clock(RadioClock {
        turn: 99,
        scenario_time: 520.0,
    });
    say(&mut a, 1, M::AmmoOut, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 1);
    a.pilot_radio_mut(PilotId(1)).unwrap().turn_off();
    let _ = run(&mut a, 0.6);
    a.set_clock(RadioClock {
        turn: 100,
        scenario_time: 600.0,
    });
    say(&mut a, 1, M::Guard, &mut rng);
    assert_eq!(a.sound.messages_in_queue(), 0, "radio turned off");
}

#[test]
fn voice_sets_are_shared_and_missing_ones_mean_no_radio() {
    let mut a = radio_game();
    assert!(a.add_pilot(PilotId(1), "pilota"));
    assert!(
        a.add_pilot(PilotId(2), "PilotA"),
        "names are case-insensitive"
    );
    assert!(Arc::ptr_eq(
        a.pilot_radio(PilotId(1)).unwrap().voice(),
        a.pilot_radio(PilotId(2)).unwrap().voice()
    ));
    assert!(!a.add_pilot(PilotId(3), "pilotAD"), "no pak for this voice");
    assert!(a.pilot_radio(PilotId(3)).is_none());
}

#[test]
fn sparse_voice_sets_skip_missing_lines() {
    // pilot1a-style sets have many empty packets. The static still plays, the missing line is silent and
    // the message completes (the C++ would crash on an empty packet).
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilot1a");
    let mut rng = rng0();
    say(&mut a, 1, M::Guard, &mut rng); // packet 34: empty in this set
    let buf = run(&mut a, 0.6);
    let l = left(&buf);
    let g = MASTER * 0.5;
    assert!(close(l[10], NOISE_LEVEL * g));
    assert_eq!(peak(&l[(0.06 * RATE as f32) as usize..]), 0.0);
    assert!(a.sound.current_message().is_none());
    // A line that exists plays; its empty callsign (packet 37) is skipped.
    say(&mut a, 1, M::SensorContact, &mut ScriptedRng::new(&[0, 0])); // packet 16, callsign 37 empty
    let m = a.sound.queued_messages().next().unwrap();
    assert_eq!(m.fragment_count(), 1);
    let buf = run(&mut a, 0.6);
    assert!(peak(&left(&buf)) > 0.0);
}

#[test]
fn purge_clears_queue_pilots_and_channels() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    let mut rng = rng0();
    say(&mut a, 1, M::Guard, &mut rng);
    a.update(DT);
    a.handle(&AudioEvent::Betty(0), &mut rng);
    a.handle(&AudioEvent::Purge, &mut rng);
    assert!(a.sound.current_message().is_none());
    assert_eq!(a.sound.messages_in_queue(), 0);
    assert!(a.pilot_radio(PilotId(1)).is_none());
    assert_eq!(
        a.mixer().lock().mode(RADIO_CHANNEL),
        mc2_platform::audio::PlayMode::Stop
    );
    // Betty keeps playing across a purge (it is the one channel the C++ leaves alone).
    assert!(a.sound.sys.is_channel_playing(BETTY_CHANNEL));
}

#[test]
fn configured_sound_directories_are_used() {
    use mc2_audio::data::SoundPaths;
    let paths = SoundPaths {
        sound_path: "audio/txt/".into(),
        cd_sound_path: "disc/snd/".into(),
    };
    let mut voice = Vec::new();
    for i in 0..42 {
        voice.push(Some(dc(RATE, 0.1, 0.02 * (i + 1) as f32)));
    }
    let b = builder_with_bites(1)
        .paths(paths.clone())
        .noise(Some(dc(RATE, 0.05, 0.5)))
        .voice("pilota", voice)
        .music("tuneZ", dc(RATE, 0.5, 0.5), false, 1.0)
        .betty(Some(dc(RATE, 0.1, 0.5)));
    let mut a = GameAudio::load(Box::new(b.build()), mixer(), volumes())
        .expect("loads from the configured dirs");
    assert!(a.add_pilot(PilotId(1), "pilota"));
    assert!(a.sound.sys.play_digital_music(0).is_ok());
    assert!(a.sound.sys.is_digital_music_playing());
    assert!(a.sound.sys.play_betty_sample(0).is_some());
    // The default directory is empty in this data set.
    let b = builder_with_bites(1).paths(paths);
    let mut data = b.build();
    use mc2_audio::data::AudioData;
    assert!(!data.exists("data/sound/sound.snd"));
    assert!(data.exists("audio/txt/sound.snd"));
    assert!(data.read("disc/snd/noise.pak").is_some());
}

#[test]
fn removed_pilots_queued_chatter_is_dropped() {
    let mut a = radio_game();
    a.add_pilot(PilotId(1), "pilota");
    a.add_pilot(PilotId(2), "pilota");
    let mut rng = rng0();
    say(&mut a, 1, M::Guard, &mut rng);
    say(&mut a, 2, M::Taunt, &mut rng);
    a.remove_pilot(PilotId(1));
    a.update(DT);
    assert_eq!(a.sound.current_message().unwrap().msg_type, M::Taunt);
    // Re-adding makes him active again.
    assert!(a.add_pilot(PilotId(1), "pilota"));
    assert!(a.pilot_is_active(PilotId(1)));
}
