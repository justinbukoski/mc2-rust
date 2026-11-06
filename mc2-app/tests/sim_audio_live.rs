// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The battle sound sink on the live `mc2_01` simulation (headless, never a window; skipped without
//! `MC2_DATA` or a GPU (`MC2_SKIP_GPU_TESTS=1` enables the GPU skip). The sink is pushed into `GameCore.sinks`, a weapon is fired in the live world and
//! the audio is rendered offline. The mission-start tune may continue across a frame with no new
//! audio request, so those frames are identified by an empty sink log rather than expecting silence.
//!
//! **What is declared here.** The pilot AI's attack decision (`AttackDecision`, `LineOfSight`, `Contacts`) is
//! not ported, so no mech in the live sim chooses to fire (the sim's own `unavailable()` report says so). The
//! test therefore makes the decision itself and calls the sim's real `Mover::fireWeapon`
//! (`mc2_sim::combat::fire_weapon`) on a mech of the live world against an enemy mover; the weapon bolt, its
//! first-frame fire sound (`code/weaponbolt.cpp:397-399`), the event queue, `GameCore`'s frame loop, the sink
//! and the offline render are all the real ones. The player mech and the nearest enemy are put 300 units apart in front of the camera first (they start about
//! 2700 units apart, out of every weapon's range, and the camera starts away from both, beyond the sound's 1500-unit
//! falloff); that placement is the only state the test sets.

use mc2_app::audio::AppAudio;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::live_sim::LiveSim;
use mc2_app::sim_audio::{sim_audio_sink, AudioSinkOutput, SharedAudioSink};
use mc2_audio::AudioEvent;
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::object::Kind;
use std::path::PathBuf;
use std::time::Duration;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

fn gpu_skip_requested() -> bool {
    std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1")
}

fn rms(s: &[f32]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
}

fn commander(o: &mc2_sim::object::Obj) -> Option<i32> {
    match &o.kind {
        Kind::Mech(m) => Some(i32::from(m.m.commander.0)),
        Kind::Vehicle(v) => Some(i32::from(v.m.commander.0)),
        _ => None,
    }
}

#[test]
fn a_weapon_firing_on_mc2_01_is_heard_after_a_frame_without_new_audio_requests() {
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (320, 240);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e) if e.contains("no suitable GPU adapter") && gpu_skip_requested() => {
            eprintln!("no GPU adapter; MC2_SKIP_GPU_TESTS=1, skipping live audio test: {e}");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    let (sink, handle): (_, SharedAudioSink<LiveSim>) = sim_audio_sink(
        &data,
        AppAudio::offline_mixer(),
        AudioSinkOutput::Offline {
            samples: Vec::new(),
        },
    )
    .expect("the battle sink loads the game's sound data");
    run.core.sinks.push(sink);

    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }

    // The attacker (a player mech) and the nearest enemy mover, both put in front of the camera.
    let (attacker, target_watch) = {
        let sim = run.core.mission.as_mut().unwrap().driver.sim_mut();
        let w = &mut sim.world;
        let movers: Vec<_> = w.objects.movers.clone();
        let attacker = movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    matches!(o.kind, Kind::Mech(_)) && commander(o) == Some(HOME_COMMANDER)
                })
            })
            .expect("mc2_01 starts with a player mech");
        let from = w.objects.get(attacker).unwrap().c.position;
        let enemy = movers
            .iter()
            .copied()
            .filter(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.exists() && commander(o).is_some_and(|c| c != HOME_COMMANDER)
                })
            })
            .min_by(|&a, &b| {
                let d = |h| {
                    let p = w.objects.get(h).unwrap().c.position;
                    (p.x - from.x).powi(2) + (p.y - from.y).powi(2)
                };
                d(a).total_cmp(&d(b))
            })
            .expect("mc2_01 has an enemy mover");
        (attacker, w.objects.watch_id(enemy))
    };

    let dt = Duration::from_nanos(1_000_000_000 / 30);
    let frame = |run: &mut HeadlessRun<LiveSim>| {
        let before = handle.borrow().offline_samples().map_or(0, <[f32]>::len);
        run.core.frame(dt);
        if let Some(e) = run.core.last_error.take() {
            panic!("{e}");
        }
        let mut sink = handle.borrow_mut();
        let samples = sink.offline_samples().unwrap();
        (
            samples[before.min(samples.len())..].to_vec(),
            sink.take_log(),
        )
    };

    let is_weapon_sound = |e: &AudioEvent| {
        matches!(
            e,
            AudioEvent::Sound {
                allow_dupes: true,
                at: Some(_),
                ..
            }
        )
    };
    // Frames before the shot: nothing has fired, so no weapon sound is asked for (the mission's own music or
    // radio may be). Each frame, until it fires, the test tries the mech's weapons (a weapon is refused while
    // the mech's weapon nodes and recycle timers settle after the mission starts: `fireWeapon` code 3).
    let mut fired = false;
    let mut quiet_frames = 0;
    let mut fired_master: Option<u8> = None;
    let mut heard: Option<(u32, Vec<f32>, Vec<AudioEvent>)> = None;
    for n in 0..1800 {
        if !fired {
            // in front of the camera as it is now (the opening shot flies it about)
            let eye = run.core.mission.as_ref().unwrap().rig.eye_position();
            let sim = run.core.mission.as_mut().unwrap().driver.sim_mut();
            let target = sim.world.objects.by_watch_id(target_watch);
            for (h, dx) in [(attacker, -150.0), (target.unwrap(), 150.0)] {
                let o = sim.world.objects.get_mut(h).unwrap();
                o.c.position.x = eye.x + dx;
                o.c.position.y = eye.y;
            }
            let items = mc2_sim::combat::loadout_of(sim.world.objects.get(attacker).unwrap())
                .expect("a mech has a loadout")
                .inventory
                .len();
            for weapon in 0..items {
                let r = mc2_sim::combat::fire::fire_weapon(
                    &mut sim.world,
                    attacker,
                    weapon as i32,
                    target,
                    -1,
                    None,
                    0,
                );
                if r.code == 0 {
                    fired = true;
                    fired_master = Some(
                        mc2_sim::combat::loadout_of(sim.world.objects.get(attacker).unwrap())
                            .unwrap()
                            .inventory[weapon]
                            .master_id,
                    );
                    break;
                }
            }
        }
        let (window, log) = frame(&mut run);
        if !fired {
            assert!(
                !log.iter().any(is_weapon_sound),
                "no weapon has fired yet: {log:?}"
            );
            // No request on this frame, though the mission-start tune may still be playing.
            if log.is_empty() {
                quiet_frames += 1;
            }
        } else if let Some(id) = log.iter().find_map(|e| match e {
            // a weapon bolt's fire sound: at the bolt's position, dupes allowed (weaponbolt.cpp:399)
            AudioEvent::Sound {
                id,
                at: Some(_),
                allow_dupes: true,
            } => Some(*id),
            _ => None,
        }) {
            heard = Some((id, window, log));
            break;
        } else if n > 1790 {
            break;
        }
    }
    assert!(fired, "a weapon of the player mech fires at the enemy");
    assert!(
        quiet_frames > 0,
        "at least one no-event live frame was checked"
    );
    let (id, window, log) = heard.expect("the bolt's fire sound reached the sink");

    // The fired weapon is master component 125; the shipped `compbas.csv` names it an SRM (read as plain text,
    // not through the sim's parser) and the C++ plays `SRM_FIRE` for it (`mclib/sounds.h:26`, `#define SRM_FIRE 11`).
    assert_eq!(fired_master, Some(125));
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&data).unwrap();
    let csv = String::from_utf8_lossy(&vfs.read("data/objects/compbas.csv").unwrap().unwrap())
        .into_owned();
    let row = csv
        .lines()
        .find(|l| l.split(',').next().is_some_and(|c| c.trim() == "125"))
        .expect("compbas.csv has master id 125");
    let name = row.split(',').nth(2).unwrap_or("");
    assert!(
        name.to_ascii_uppercase().contains("SRM"),
        "master 125 is {name:?}"
    );
    assert_eq!(id, 11, "SRM_FIRE");

    // The audible window holds only that sample: the frame's requests are the bolt's sound(s), nothing else.
    assert!(
        log.iter()
            .all(|e| matches!(e, AudioEvent::Sound { id: 11, .. })),
        "only SRM_FIRE was requested in the firing frame: {log:?}"
    );
    let sink = handle.borrow();
    assert!(sink.audio.borrow().sound.sys.is_playing_sample(id));
    assert!(rms(&window) > 0.0, "the frame it fired in is audible");
}
