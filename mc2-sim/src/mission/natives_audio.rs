// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The sound natives of `code/ablmc2.cpp` (music, sound effects, speech, Betty, radio power, voice-over and the sound readbacks),
//! the combat-music query `getplayerincombat`:
//! `playdigitalmusic`, `stopmusic`, `playsoundeffect`, `setradio`, `playspeech`, `playbetty`, `playwave`, `stopvoiceover`,
//! `getcurrentmusicid`, `getmissiontune`, `isplayingvoiceover`, `getplayerincombat`.
//!
//! The sound system is the application's (`mclib/soundsys.cpp`, `mc2-audio`). The simulation never plays anything: each native pushes
//! the request as a [`SimEvent`] (the application's sinks play it after the step) and answers as the C++ call would, from the answers
//! the application reports back in [`World::audio`](crate::World::audio) (refreshed before each step). The natives move that readback
//! at once where the C++ `SoundSystem` moves its own state in the same call (`stopMusic`'s `currentMusicId = -1`, `stopVoiceOver`, the
//! Betty channel, and a tune or voice-over file that starts): whether a request starts is decided here from the frozen snapshot
//! ([`AudioReadback`](crate::events::AudioReadback): stream state and the file set), so a refused or missing one leaves the readback alone.
//! `incallout` and `animationcallout` are the presentation natives (`natives_presentation.rs`).

use super::bindings::SimBindings;
use crate::events::SimEvent;
use crate::ids::WatchId;
use mc2_abl::ArrayRef;

/// `BETTY_CHANNEL` (`mclib/soundsys.h:58`, `FIRE_CHANNEL - 1`): the value `playBettySample` returns when it takes the channel.
pub const BETTY_CHANNEL: i32 = 14;

/// `getWarrior(warriorIndex)` (`code/ablmc2.cpp:355-361`): -1 is `CurWarrior`. `None` when there is no such pilot.
fn warrior_slot(b: &SimBindings<'_>, warrior_index: i32) -> Option<u16> {
    let slot = if warrior_index == -1 {
        b.cur_warrior?
    } else {
        u16::try_from(warrior_index).ok()?
    };
    b.world.pilots.get(slot).is_some().then_some(slot)
}

/// `execPlayDigitalMusic` (`code/ablmc2.cpp:2606-2620`): `playABLDigitalMusic` -> `playDigitalMusic` (`mclib/soundsys.cpp:673`). A
/// negative id does nothing (`musicId >= 0`). The request goes to the application; when the sound system would start the tune
/// (`mclib/soundsys.cpp:713,738,763` set `currentMusicId` in the call) the readback moves at once, so `getcurrentmusicid` in the
/// same execution answers it. A tune it refuses (the id already playing, both streams taken) or has no file for leaves
/// `currentMusicId` as it was. Returns 0.
pub fn play_digital_music(b: &mut SimBindings<'_>, sound_num: i32) -> i32 {
    if sound_num >= 0 {
        let id = i64::from(sound_num);
        b.world.events.push(SimEvent::Music(id));
        let a = &mut b.world.audio;
        if id != a.music_id && a.music_enabled && !a.music_streams_busy && a.files.music_file(id) {
            a.music_id = id;
        }
    }
    0
}

/// `execStopMusic` (`code/ablmc2.cpp:2624-2634`): `stopABLMusic` -> `stopDigitalMusic`, which sets `currentMusicId` to -1
/// (`mclib/soundsys.cpp:1091-1103`).
pub fn stop_music(b: &mut SimBindings<'_>) -> i32 {
    b.world.audio.music_id = -1;
    b.world.audio.music_streams_busy = false;
    b.world.events.push(SimEvent::StopMusic);
    0
}

/// `execPlaySoundEffect` (`code/ablmc2.cpp:2638-2651`): `playABLSFX` -> `playDigitalSample(sfxId)` at the default position (no
/// position, so centred) and without duplicates. Returns 0.
pub fn play_sound_effect(b: &mut SimBindings<'_>, sound_num: i32) -> i32 {
    b.world.events.push(SimEvent::Sound {
        id: sound_num,
        source: WatchId::NONE,
        at: None,
        allow_dupes: false,
    });
    0
}

/// `execSetRadio` (`code/ablmc2.cpp:2672-2701`): `Radio::turnOn` / `turnOff` on the warrior's radio, when it has one. The
/// application's radio is switched by the event (a pilot without one, or an index past `MAX_WARRIORS`, does nothing).
pub fn set_radio(b: &mut SimBindings<'_>, warrior_index: i32, on: bool) -> i32 {
    if let Some(slot) = warrior_slot(b, warrior_index) {
        b.world.events.push(SimEvent::RadioPower {
            pilot: u32::from(slot),
            on,
        });
    }
    0
}

/// `execPlaySpeech` (`code/ablmc2.cpp:2704-2721`): `pilot->radioMessage(message, true)`, the same path the pilots' own messages take
/// (`ai::warrior::radio_message`, which applies the home-command and turn gates). Returns 0.
pub fn play_speech(b: &mut SimBindings<'_>, warrior_index: i32, message: i32) -> i32 {
    if let Some(slot) = warrior_slot(b, warrior_index) {
        crate::ai::warrior::radio_message(b.world, slot, message);
    }
    0
}

/// `execPlayBetty` (`code/ablmc2.cpp:2724-2738`) -> `SoundSystem::playBettySample` (`mclib/soundsys.cpp:829-880`): a Betty line
/// takes the channel only when none is loaded, the id is a sample of the pak and its packet loads; then the result is
/// [`BETTY_CHANNEL`], else -1. A negative id is a huge unsigned value, so out of range (the Rust bank has no Betty packet for it).
pub fn play_betty(b: &mut SimBindings<'_>, message_index: i32) -> i32 {
    let audio = &mut b.world.audio;
    let Ok(id) = u32::try_from(message_index) else {
        return -1;
    };
    if !audio.betty_free || id >= audio.betty_samples {
        return -1;
    }
    audio.betty_free = false;
    b.world.events.push(SimEvent::Betty(id));
    BETTY_CHANNEL
}

/// `execGetCurrentMusicId` (`code/ablmc2.cpp:2799-2802`): `SoundSystem::getCurrentMusicId`.
pub fn get_current_music_id(b: &SimBindings<'_>) -> i32 {
    i32::try_from(b.world.audio.music_id).unwrap_or(-1)
}

/// `execGetMissionTuneId` (`code/ablmc2.cpp:2804-2807`): `Mission::getMissionTuneId`, the `scenarioTuneNum` the mission loaded
/// (`code/mission.cpp:2129`), which the mission script hands to `playdigitalmusic`.
pub fn get_mission_tune_id(b: &SimBindings<'_>) -> i32 {
    i32::from(b.world.mission.tune_num)
}

/// `execPlayWave` (`code/ablmc2.cpp:6259-6269`): `playSupportSample(-1, fileName)`, a voice-over file played as stream 3. The type
/// argument is ignored by the C++ too. Returns 0.
pub fn play_wave(
    b: &mut SimBindings<'_>,
    ctx: &mc2_abl::NativeCtx<'_>,
    file_name: ArrayRef,
) -> i32 {
    let file = ctx.string(file_name);
    // the stream-3 handle is set only when the file plays (`playDigitalStream`: missing file, no sound: no handle)
    let a = &mut b.world.audio;
    if a.voice_available && !a.voice_busy && a.files.voice_file(&file) {
        a.voice_over = true;
    }
    b.world.events.push(SimEvent::VoiceOver(file));
    0
}

/// `execIsPlayingVoiceOver` (`code/ablmc2.cpp:5965-5972`): `SoundSystem::isPlayingVoiceOver` (stream 3 loaded).
pub fn is_playing_voice_over(b: &SimBindings<'_>) -> bool {
    b.world.audio.voice_over
}

/// `execStopVoiceOver` (`code/ablmc2.cpp:5975-5980`): `stopSupportSample`, which clears the stream-3 handle when there is one.
pub fn stop_voice_over(b: &mut SimBindings<'_>) {
    if b.world.audio.voice_over {
        b.world.audio.voice_over = false;
        b.world.audio.voice_busy = false;
        b.world.events.push(SimEvent::StopSupport);
    }
}

/// `execPlayerInCombat` (`code/ablmc2.cpp:2789-2794`): `MechWarrior::anyPlayerInCombat` (`code/warrior.cpp:4498-4511`), true when
/// any pilot of the player's force flies a vehicle and has a current target (`getCurrentTarget()` = `getLastTarget()`). The
/// mission scripts use it to switch to the combat tune.
pub fn get_player_in_combat(b: &mut SimBindings<'_>) -> bool {
    crate::combat::fx::any_player_in_combat(b.world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::pilot::Pilot;
    use crate::events::{AudioFileSet, AudioFiles, AudioReadback};
    use std::rc::Rc;

    /// A data set with tunes 1..=9 and the voice-over file "tut_4a".
    struct Set;
    impl AudioFileSet for Set {
        fn music_file(&self, id: i64) -> bool {
            (1..=9).contains(&id)
        }
        fn voice_file(&self, name: &str) -> bool {
            name == "tut_4a"
        }
    }
    /// A sound system that is on, idle and has that data set.
    fn probed() -> AudioReadback {
        AudioReadback {
            music_enabled: true,
            voice_available: true,
            files: AudioFiles::new(Rc::new(Set)),
            ..AudioReadback::default()
        }
    }
    use crate::rng::RngMode;
    use crate::World;

    /// A world with one pilot in slot 1 (slot 0 is never handed out) and the sound readback set to `audio`.
    fn world_with(audio: AudioReadback) -> World {
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        w.audio = audio;
        w.pilots.slots = vec![Pilot::default(), Pilot::default()];
        w
    }

    fn events(w: &World) -> Vec<&SimEvent> {
        w.events.iter().collect()
    }

    #[test]
    fn music_readback_moves_in_the_call_when_the_sound_system_starts_the_tune() {
        let mut w = world_with(probed());
        let mut b = SimBindings::mission(&mut w);
        assert_eq!(get_current_music_id(&b), -1);
        assert_eq!(play_digital_music(&mut b, 9), 0);
        assert_eq!(
            get_current_music_id(&b),
            9,
            "a started tune is current at once"
        );
        // a tune the sound system refuses (no file) leaves the old id; the request still goes out
        assert_eq!(play_digital_music(&mut b, 50), 0);
        assert_eq!(get_current_music_id(&b), 9);
        // a negative id is ignored: no request, the tune stays
        assert_eq!(play_digital_music(&mut b, -1), 0);
        assert_eq!(get_current_music_id(&b), 9);
        assert_eq!(stop_music(&mut b), 0);
        assert_eq!(get_current_music_id(&b), -1, "stopDigitalMusic sets -1");
        assert_eq!(
            events(&w),
            [
                &SimEvent::Music(9),
                &SimEvent::Music(50),
                &SimEvent::StopMusic
            ]
        );
    }

    #[test]
    fn music_without_a_sound_system_never_becomes_current() {
        let mut w = world_with(AudioReadback::default());
        let mut b = SimBindings::mission(&mut w);
        play_digital_music(&mut b, 9);
        assert_eq!(get_current_music_id(&b), -1);
    }

    #[test]
    fn sound_effect_is_a_centred_sample_without_duplicates() {
        let mut w = world_with(AudioReadback::default());
        assert_eq!(play_sound_effect(&mut SimBindings::mission(&mut w), 77), 0);
        assert_eq!(
            events(&w),
            [&SimEvent::Sound {
                id: 77,
                source: WatchId::NONE,
                at: None,
                allow_dupes: false,
            }]
        );
    }

    #[test]
    fn betty_takes_the_channel_only_when_free_and_in_range() {
        let audio = AudioReadback {
            betty_free: true,
            betty_samples: 3,
            ..AudioReadback::default()
        };
        let mut w = world_with(audio);
        let mut b = SimBindings::mission(&mut w);
        assert_eq!(play_betty(&mut b, 2), BETTY_CHANNEL);
        // Betty is busy until the application reports the line finished
        assert_eq!(play_betty(&mut b, 1), -1);
        assert_eq!(play_betty(&mut b, 3), -1, "out of range");
        assert_eq!(
            play_betty(&mut b, -1),
            -1,
            "negative ids are huge unsigned in C++"
        );
        assert_eq!(events(&w), [&SimEvent::Betty(2)]);
    }

    #[test]
    fn betty_without_sound_answers_minus_one() {
        let mut w = world_with(AudioReadback::default());
        assert_eq!(play_betty(&mut SimBindings::mission(&mut w), 0), -1);
        assert!(w.events.is_empty());
    }

    #[test]
    fn voice_over_is_cleared_by_stop_only_when_one_plays() {
        let audio = AudioReadback {
            voice_over: true, // a voice-over the application reports it is playing
            ..AudioReadback::default()
        };
        let mut w = world_with(audio);
        let mut b = SimBindings::mission(&mut w);
        assert!(is_playing_voice_over(&b));
        stop_voice_over(&mut b); // the stream-3 handle is destroyed
        assert!(!is_playing_voice_over(&b));
        stop_voice_over(&mut b); // no stream-3 handle now: nothing to destroy, no request
        assert_eq!(b.world.events, [SimEvent::StopSupport]);
    }

    /// `playWave` reads the file name from the script's string and asks for it as a voice-over; the answer is 0 (C++ pushes 0).
    #[test]
    fn play_wave_asks_for_a_voice_over_file() {
        const SCRIPT: &str = "module t : integer; \
            var static integer called; \
            code if (playWave(\"tut_4a\", 0) == 0) then called = 1; endif; return (9999); endmodule.";
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        let mut src = mc2_abl::MemorySource::new();
        src.insert("t.abl", SCRIPT);
        let mut env = mc2_abl::Env::new(
            mc2_abl::Config::default(),
            src,
            Box::new(mc2_abl::RngHost::new(w.rng.clone())),
        );
        let h = env.pre_process("t.abl").expect("compile");
        let inst = env.new_instance(h).expect("instance");
        env.execute(inst, &mut SimBindings::mission(&mut w))
            .expect("run");
        assert_eq!(env.get_static_integer(inst, "called"), Some(1));
        assert_eq!(w.events, [SimEvent::VoiceOver("tut_4a".to_string())]);
        assert!(!w.audio.voice_over, "no sound system: no stream handle");
    }

    #[test]
    fn voice_over_flag_follows_whether_the_file_plays() {
        let script = |file: &str| {
            format!(
                "module t : integer; var static integer after; char[20] f; \
                code f = \"{file}\"; playWave(f, 0); if (isPlayingVoiceOver) then after = 1; endif; return (9999); endmodule."
            )
        };
        for (file, playing) in [("tut_4a", Some(1)), ("missing", Some(0))] {
            let mut w = World::new(RngMode::LegacyMsvc, 1);
            w.audio = probed();
            let mut src = mc2_abl::MemorySource::new();
            src.insert("t.abl", script(file));
            let mut env = mc2_abl::Env::new(
                mc2_abl::Config::default(),
                src,
                Box::new(mc2_abl::RngHost::new(w.rng.clone())),
            );
            let h = env.pre_process("t.abl").expect("compile");
            let inst = env.new_instance(h).expect("instance");
            env.execute(inst, &mut SimBindings::mission(&mut w))
                .expect("run");
            assert_eq!(env.get_static_integer(inst, "after"), playing, "{file}");
            assert_eq!(w.audio.voice_over, playing == Some(1));
        }
    }

    #[test]
    fn stop_then_play_of_the_current_tune_in_one_step_reads_back_the_new_tune() {
        let mut a = probed();
        a.music_id = 4;
        a.music_streams_busy = true;
        let mut w = world_with(a);
        let mut b = SimBindings::mission(&mut w);
        stop_music(&mut b);
        assert_eq!(get_current_music_id(&b), -1);
        play_digital_music(&mut b, 4);
        assert_eq!(get_current_music_id(&b), 4, "stopmusic freed both streams");
        // with the streams still busy a different tune is refused
        let mut a = probed();
        a.music_id = 4;
        a.music_streams_busy = true;
        let mut w = world_with(a);
        let mut b = SimBindings::mission(&mut w);
        play_digital_music(&mut b, 5);
        assert_eq!(get_current_music_id(&b), 4);
    }

    #[test]
    fn stop_then_play_voice_over_in_one_step_reads_back_playing() {
        let script = "module t : integer; var static integer after; char[20] f; \
            code f = \"tut_4a\"; stopVoiceOver; playWave(f, 0); if (isPlayingVoiceOver) then after = 1; endif; return (9999); endmodule.";
        let mut a = probed();
        a.voice_over = true;
        a.voice_busy = true;
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        w.audio = a;
        let mut src = mc2_abl::MemorySource::new();
        src.insert("t.abl", script.to_string());
        let mut env = mc2_abl::Env::new(
            mc2_abl::Config::default(),
            src,
            Box::new(mc2_abl::RngHost::new(w.rng.clone())),
        );
        let h = env.pre_process("t.abl").expect("compile");
        let inst = env.new_instance(h).expect("instance");
        env.execute(inst, &mut SimBindings::mission(&mut w))
            .expect("run");
        assert_eq!(env.get_static_integer(inst, "after"), Some(1));
        assert!(w.audio.voice_over);
        assert_eq!(
            w.events,
            [
                SimEvent::StopSupport,
                SimEvent::VoiceOver("tut_4a".to_string())
            ]
        );
    }

    #[test]
    fn radio_power_goes_to_the_pilot_slot_and_needs_a_pilot() {
        let mut w = world_with(AudioReadback::default());
        let mut b = SimBindings::mission(&mut w);
        set_radio(&mut b, 1, false);
        set_radio(&mut b, 5, true); // no such pilot
        assert_eq!(
            events(&w),
            [&SimEvent::RadioPower {
                pilot: 1,
                on: false
            }]
        );
    }

    #[test]
    fn minus_one_warrior_is_the_current_pilot() {
        let mut w = world_with(AudioReadback::default());
        let mut b = SimBindings::mission(&mut w);
        set_radio(&mut b, -1, true);
        assert!(events(&w).is_empty(), "no current warrior: nothing happens");
        let mut b = SimBindings {
            cur_warrior: Some(1),
            ..SimBindings::mission(&mut w)
        };
        set_radio(&mut b, -1, true);
        assert_eq!(events(&w), [&SimEvent::RadioPower { pilot: 1, on: true }]);
    }

    #[test]
    fn mission_tune_is_the_scenario_tune_number() {
        let mut w = World::new(RngMode::LegacyMsvc, 1);
        w.mission.tune_num = 33;
        assert_eq!(get_mission_tune_id(&SimBindings::mission(&mut w)), 33);
    }

    /// `anyPlayerInCombat` (warrior.cpp:4498): a home-commanded mech with a current target puts the force in combat; a target's
    /// destruction leaves a watch id but no target (`getLastTarget` drops it), and the enemy pilot does not count.
    #[test]
    fn player_in_combat_follows_the_player_pilots_current_targets() {
        let (mut w, me, enemy) = crate::ai::tests::world();
        let mut b = SimBindings::mission(&mut w);
        assert!(!get_player_in_combat(&mut b));
        // the enemy (team 1, commander 1) aiming at the player is not the player's force
        let enemy_wid = b.world.objects.watch_id(enemy);
        b.world.pilots.slots[2].last_target = (enemy_wid, 0.0);
        assert!(!get_player_in_combat(&mut b));
        let enemy_wid_for_player = b.world.objects.watch_id(enemy);
        b.world.pilots.slots[1].last_target = (enemy_wid_for_player, 0.0);
        assert!(get_player_in_combat(&mut b));
        // the target dies: getLastTarget clears it, so the force is out of combat again
        b.world.objects.get_mut(enemy).unwrap().c.status = crate::object::ObjectStatus::Destroyed;
        assert!(!get_player_in_combat(&mut b));
        let _ = me;
    }
}
