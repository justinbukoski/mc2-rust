// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tests against the shipped game data. They skip (pass) when `MC2_DATA` is not set.

use mc2_audio::bank::SoundBank;
use mc2_audio::data::{AudioData, VfsData};
use mc2_audio::ids::{betty, music, sfx, support};
use mc2_audio::radio::{RadioMessageTable, RadioMessageType, VoiceSet, RADIO_MESSAGE_COUNT};
use mc2_audio::snd::SoundTable;
use mc2_audio::{GameAudio, Volumes};
use mc2_formats::vfs::Vfs;
use mc2_platform::audio::{Mixer, SharedMixer, WavStream};
use std::collections::BTreeSet;

fn data() -> Option<VfsData> {
    let root = std::env::var_os("MC2_DATA")?;
    let vfs = Vfs::from_system_cfg(root).expect("vfs");
    Some(VfsData::new(vfs, false))
}

macro_rules! need_data {
    () => {
        match data() {
            Some(d) => d,
            None => {
                eprintln!("MC2_DATA not set: skipped");
                return;
            }
        }
    };
}

#[test]
fn sound_directories_come_from_system_cfg() {
    let d = need_data!();
    assert_eq!(d.paths().sound_path, "data/sound/");
    assert_eq!(d.paths().cd_sound_path, "data/sound/");
}

fn table(d: &mut VfsData) -> SoundTable {
    SoundTable::from_bytes(d.read("data/sound/sound.snd").unwrap()).unwrap()
}

#[test]
fn sound_table_parses_and_every_bite_resolves_to_a_packet() {
    let mut d = need_data!();
    let t = table(&mut d);
    assert_eq!(t.num_bites(), 117);
    assert_eq!(t.music.len(), 34);
    assert_eq!(t.stream_fade_down_time, 1.0);
    assert_eq!(t.max_sound_distance, 2800.0);
    // The `soundId` key is stored but never used (the packet is the block index); 12 blocks disagree.
    let odd: Vec<usize> = t
        .bites
        .iter()
        .enumerate()
        .filter(|(i, b)| b.sound_id as usize != *i)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(odd, vec![20, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 116]);
    assert!(t.bites.iter().all(|b| (0.0..=1.0).contains(&b.volume)));
    let bank = SoundBank::load(&mut d, "sound").unwrap();
    let c = bank.effect_coverage(&t);
    eprintln!(
        "sound bites: {} in table, {} loadable, {} empty {:?}, {} broken",
        c.bites,
        c.loadable,
        c.empty.len(),
        c.empty,
        c.broken.len()
    );
    assert_eq!(c.bites, 117);
    assert!(
        c.broken.is_empty(),
        "no table entry points past the pak or at garbage"
    );
    assert_eq!(c.loadable, 106);
    // The 11 empty packets are the retired effects: the C++ claims a channel and plays nothing.
    assert_eq!(c.empty, vec![0, 31, 39, 40, 41, 43, 44, 45, 47, 62, 72]);
    assert_eq!(c.loadable + c.empty.len(), c.bites);
    assert_eq!(bank.effects.len(), 117);

    // Which named ids from sounds.h are silent in the shipped data?
    let named_empty: BTreeSet<&str> = sfx::ALL
        .iter()
        .filter(|(_, id)| c.empty.contains(&(*id as usize)))
        .map(|(n, _)| *n)
        .collect();
    eprintln!("named sfx ids with no sample: {named_empty:?}");
    assert_eq!(
        named_empty,
        BTreeSet::from([
            "BOMB_EXPLODE",
            "HEAVY_ROTATION",
            "ORBITAL_GUN",
            "DEFENSE_SILO",
            "TRAIN_XING",
            "HPG_ANTENNA",
            "ZOOM_GUI",
            "INCOMING_ARTILLIRY",
            "BREAKINGFENCE",
        ])
    );
    // Every named id is inside the table.
    assert!(sfx::ALL
        .iter()
        .all(|(_, id)| (*id as usize) < t.num_bites()));
    eprintln!(
        "sfx ids defined in sounds.h: {} ({} loadable)",
        sfx::ALL.len(),
        sfx::ALL
            .iter()
            .filter(|(_, id)| bank.effects.sound(*id as usize).is_some())
            .count()
    );
}

#[test]
fn betty_support_and_noise_banks_are_complete() {
    let mut d = need_data!();
    let bank = SoundBank::load(&mut d, "sound").unwrap();
    assert_eq!(bank.betty.len(), 54);
    assert_eq!(bank.betty.len() as u32, betty::MAX_BETTY_SAMPLES);
    assert!(bank.betty.missing().is_empty(), "every Betty line loads");
    for (name, id) in betty::ALL {
        assert!(bank.betty.sound(*id as usize).is_some(), "{name}");
    }
    assert_eq!(betty::ALL.len(), 54);
    assert_eq!(bank.support.len(), 7);
    assert_eq!(bank.support.len() as u32, support::MAX_SUPPORT_SAMPLES);
    assert!(bank.support.missing().is_empty());
    assert_eq!(bank.noise.len(), 4);
    assert!(bank.noise.missing().is_empty());
    eprintln!(
        "betty: {}/{} lines load, support: {}/{}, noise: {}/{}",
        bank.betty.len() - bank.betty.missing().len(),
        bank.betty.len(),
        bank.support.len(),
        bank.support.len(),
        bank.noise.len(),
        bank.noise.len()
    );
}

#[test]
fn every_music_entry_has_a_decodable_wav() {
    let mut d = need_data!();
    let t = table(&mut d);
    let mut ok = 0;
    for (i, m) in t.music.iter().enumerate() {
        if m.is_none() {
            continue;
        }
        let path = format!("data/sound/{}.wav", m.name);
        assert!(d.exists(&path), "music {i}: {path}");
        let bytes = d.read(&path).unwrap();
        let w = WavStream::new(std::io::Cursor::new(bytes)).expect(&path);
        assert!(w.total_frames() > 44100, "{path} is a real tune");
        ok += 1;
    }
    eprintln!("music: {ok}/{} entries have a playable wav", t.music.len());
    assert_eq!(ok, 34);
    // The tune ids the game names are inside the table.
    assert!(music::ALL
        .iter()
        .all(|(_, id)| (*id as usize) < t.music.len()));
    // Loop flags from the file: combat tunes loop, mission-start tunes do not.
    assert!(t.music[music::COMBAT_TUNE_0 as usize].looping);
    assert!(!t.music[music::MISSION_START_0 as usize].looping);
    assert!(t.music[music::LOGISTICS_LOOP as usize].looping);
}

#[test]
fn radio_table_matches_radio_csv() {
    let mut d = need_data!();
    let rt = RadioMessageTable::load(&mut d).unwrap();
    assert_eq!(rt.iter().count(), RADIO_MESSAGE_COUNT);
    let ack = rt.get(RadioMessageType::Ack);
    assert_eq!(
        (ack.priority, ack.style_count, ack.style_chance),
        (5, 3, [33, 33, 34])
    );
    assert_eq!(ack.message_mapping, 0);
    let ej = rt.get(RadioMessageType::Ejecting);
    assert_eq!(
        (ej.priority, ej.movie_code, ej.message_mapping),
        (0, b'4', 24)
    );
    let sc = rt.get(RadioMessageType::SensorContact);
    assert!(sc.pilot_identifies_self);
    assert_eq!(sc.movie_code, b'1');
    let cs = rt.get(RadioMessageType::Callsign);
    assert_eq!((cs.style_count, cs.message_mapping), (2, 37));
    // Only EJECTING and DEATH are priority 0.
    let zero: Vec<_> = rt
        .iter()
        .filter(|(_, i)| i.priority == 0)
        .map(|(m, _)| m)
        .collect();
    assert_eq!(
        zero,
        vec![RadioMessageType::Ejecting, RadioMessageType::Death]
    );
}

fn pilot_voice_names() -> Vec<String> {
    let mut v: Vec<String> = ["pilot1a", "pilot1b", "pilot1c", "pilot1d"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    v.extend(('a'..='z').map(|c| format!("pilot{c}")));
    v
}

/// (message, style) -> packet, for every style the table defines.
fn lines(rt: &RadioMessageTable) -> Vec<(RadioMessageType, u8, usize)> {
    let mut out = vec![];
    for (m, i) in rt.iter() {
        for s in 0..i.style_count.max(1) {
            out.push((m, s, (i.message_mapping + u32::from(s)) as usize));
        }
    }
    out
}

#[test]
fn radio_sets_are_complete_for_every_pilot_voice() {
    let mut d = need_data!();
    let rt = RadioMessageTable::load(&mut d).unwrap();
    let all = lines(&rt);
    assert_eq!(all.len(), 43, "31 message types, 43 (message, style) lines");
    let mut full = 0;
    let mut salvage_only = 0;
    let mut total_lines = 0;
    for name in pilot_voice_names() {
        let v = VoiceSet::open(&mut d, &name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let missing: Vec<_> = all
            .iter()
            .filter(|(_, _, idx)| !v.packet(*idx).is_sound())
            .map(|(m, s, _)| (*m, *s))
            .collect();
        if name.starts_with("pilot1") {
            // Support-unit voices (guard tower, mine layer, repair, scout): a short fixed repertoire.
            assert_eq!(v.packet_count(), 40, "{name}");
            for (m, s, idx) in &all {
                let must = matches!(
                    (*m, *s),
                    (RadioMessageType::Ack, 0)
                        | (RadioMessageType::Callsign, 0)
                        | (RadioMessageType::Death, 0)
                        | (RadioMessageType::CannotCapture, 0)
                        | (RadioMessageType::IllegalOrder, 0)
                        | (RadioMessageType::UnderAttack, _)
                );
                if must {
                    assert!(v.packet(*idx).is_sound(), "{name}: {} #{s}", m.name());
                }
            }
            total_lines += all.len() - missing.len();
            continue;
        }
        assert_eq!(v.packet_count(), 42, "{name}");
        // Full voices: every line except the two RADIO_MECH_SALVAGE styles in 14 of the 26 voices. That
        // message is never requested by the game code (no caller of `radioMessage(RADIO_MECH_SALVAGE)`).
        if missing.is_empty() {
            full += 1;
        } else {
            assert_eq!(
                missing,
                vec![
                    (RadioMessageType::MechSalvage, 0),
                    (RadioMessageType::MechSalvage, 1)
                ],
                "{name}"
            );
            salvage_only += 1;
        }
        total_lines += all.len() - missing.len();
        // The callsign line the radio actually uses (always style 0).
        assert!(v.packet(37).is_sound(), "{name} callsign");
    }
    eprintln!(
        "pilot voices: 30 sets ({full} complete, {salvage_only} complete except MECH_SALVAGE, 4 support-unit sets), {total_lines} lines"
    );
    assert_eq!((full, salvage_only), (12, 14));
}

#[test]
fn shipped_pilots_name_voice_sets_that_exist_except_pilotad() {
    // The warrior .fit files name: pilot1a-d, pilota-z (case varies) and "pilotAD" (pmwtool.fit), which
    // has no pak. The C++ then has a radio that never finds a line; the port gives that pilot no radio.
    let mut d = need_data!();
    assert!(VoiceSet::open(&mut d, "pilotAD").is_err());
    assert!(VoiceSet::open(&mut d, "PilotA").is_ok(), "case-insensitive");
}

fn real_game(rate: u32, volumes: Volumes) -> GameAudio {
    let d = data().expect("data");
    GameAudio::load(
        Box::new(d),
        SharedMixer::new(Mixer::for_game(rate)),
        volumes,
    )
    .expect("game audio loads")
}

#[test]
fn every_message_of_a_full_voice_plays_through_and_finishes() {
    if data().is_none() {
        eprintln!("MC2_DATA not set: skipped");
        return;
    }
    use mc2_audio::engine::AudioEvent;
    use mc2_audio::radio::{PilotId, RadioClock};
    use mc2_audio::rng::SeededRng;
    let mut a = real_game(22050, Volumes::default());
    assert!(a.add_pilot(PilotId(1), "pilotk"));
    let mut rng = SeededRng::new(7);
    let mut turn = 1;
    let mut played = 0;
    for m in RadioMessageType::ALL {
        if m == RadioMessageType::MechSalvage {
            continue; // the one line a few voices lack
        }
        turn += 1;
        a.set_clock(RadioClock {
            turn,
            scenario_time: 20.0 * turn as f32,
        });
        a.handle(
            &AudioEvent::Radio {
                pilot: PilotId(1),
                message: m.index() as i64,
                under_home_command: true,
            },
            &mut rng,
        );
        assert_eq!(a.sound.messages_in_queue(), 1, "{} queued", m.name());
        let mut out = Vec::new();
        let mut secs = 0.0;
        while (a.sound.messages_in_queue() > 0 || a.sound.current_message().is_some())
            && secs < 30.0
        {
            a.render_offline(0.05, &mut out);
            secs += 0.05;
        }
        assert!(secs < 30.0, "{} finishes", m.name());
        let peak = out.iter().fold(0.0f32, |p, v| p.max(v.abs()));
        assert!(peak > 0.0, "{} is audible", m.name());
        played += 1;
    }
    assert_eq!(played, 30);
}
