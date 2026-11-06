// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The headless C++ oracle's sound state (DIFFERENCES `ORACLE-HEADLESS-AUDIO`), pinned against the music block the shipped mission
//! scripts share (`mc2_05.abl:280-282`, `mc2_07`, `mc2_09`, `mc2_24`):
//!
//!     if (getSensorsActive and ((getCurrentMusicId == -1) or ...)) then if (random(100) < 1) then playDigitalMusic(...)
//!
//! The oracle runs `DoGameLogic` only. `SoundSystem::update` (`mechcmd2.cpp:2108`) is the render path and never runs, and the gos
//! audio stubs give no stream handle, so a tune that starts sets `currentMusicId` (`soundsys.cpp:713`) and nothing clears it
//! (`soundsys.cpp:631` is in `update`). The script's `getCurrentMusicId == -1` poll, and its `random(100)` draw from the sim
//! stream behind it, therefore stop for the rest of the run after the first tune. Without that readback the Rust sim draws every
//! frame (12189 / 7093 / 5678 draws over mc2_05 / mc2_07 / mc2_09; the oracle's last draw is frame 693 / 936 / 522).
mod common;

use mc2_abl::{Config, Env, MemorySource, RngHost};
use mc2_sim::events::{AudioFileSet, AudioFiles, AudioReadback};
use mc2_sim::mission::bindings::SimBindings;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::{FrameInput, Sim};
use std::rc::Rc;

/// Tunes 0..=3 exist; nothing else does.
struct Tunes;
impl AudioFileSet for Tunes {
    fn music_file(&self, id: i64) -> bool {
        (0..4).contains(&id)
    }
    fn voice_file(&self, _name: &str) -> bool {
        false
    }
}

fn load() -> Option<(Sim, MemorySource)> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let sources = mc2_sim::mission::abl_load::collect_sources(&vfs).expect("abl sources");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_05", seed, mode))
        .expect("mc2_05 loads");
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    Some((Sim::new(outcome), sources))
}

/// One execution of the music block of the shipped scripts, with the `random(100) < 100` that always passes (the shipped test is
/// `< 1`; the draw is what is pinned), in a fresh environment as the mission brain's one frame.
fn music_block(sim: &mut Sim, sources: &MemorySource, tune: i32) {
    let mut src = sources.clone();
    src.insert(
        "t.abl",
        format!(
            "module t : integer;\ncode\n\
             if (getCurrentMusicId == -1) then\n if (random(100) < 100) then playDigitalMusic({tune}); endif;\nendif;\n\
             return(0);\nendmodule.\n"
        ),
    );
    let mut env = Env::new(
        Config::default(),
        src,
        Box::new(RngHost::new(sim.world.rng.clone())),
    );
    for lib in mc2_sim::mission::abl_load::LIBRARIES {
        env.load_library(lib).expect("library");
    }
    let h = env.pre_process("t.abl").expect("compile");
    let inst = env.new_instance(h).expect("instance");
    sim.world.mission.brain = None;
    sim.world.mission.brain_callback = None;
    for p in sim.world.pilots.slots.iter_mut() {
        p.brain = None;
    }
    {
        let mut b = SimBindings::mission(&mut sim.world);
        env.execute(inst, &mut b).expect("run");
    }
    sim.world.abl = Some(env);
}

fn draws(sim: &Sim) -> u64 {
    sim.world.rng.0.borrow().sim.draws()
}

fn frames(sim: &mut Sim, from: u64, n: u64) {
    for f in from..from + n {
        sim.step(FrameInput::fixed(
            30,
            f,
            Rc::new(mc2_sim::visibility::AllVisible),
            Vec::new(),
        ))
        .expect("step");
    }
}

#[test]
fn headless_oracle_audio_stops_the_music_poll_after_the_first_tune() {
    let Some((mut sim, sources)) = load() else {
        return;
    };
    sim.world.audio = AudioReadback::cpp_headless_oracle(AudioFiles::new(Rc::new(Tunes)));
    let base = draws(&sim);
    music_block(&mut sim, &sources, 1);
    assert_eq!(sim.world.audio.music_id, 1, "the tune started in the call");
    let after_first = draws(&sim);
    assert!(after_first > base, "the first poll drew");
    // no `SoundSystem::update`: simulation frames never end the tune, so the poll never draws again
    for round in 0..4 {
        frames(&mut sim, 1 + round * 50, 50);
        let before = draws(&sim);
        music_block(&mut sim, &sources, 1);
        assert_eq!(draws(&sim), before, "round {round}: no script draw");
        assert_eq!(sim.world.audio.music_id, 1, "the tune is still current");
    }
}

#[test]
fn headless_oracle_audio_never_reports_busy_streams_and_refuses_the_current_tune() {
    let Some((mut sim, _)) = load() else {
        return;
    };
    sim.world.audio = AudioReadback::cpp_headless_oracle(AudioFiles::new(Rc::new(Tunes)));
    let mut b = SimBindings::mission(&mut sim.world);
    mc2_sim::mission::natives_audio::play_digital_music(&mut b, 2);
    assert_eq!(mc2_sim::mission::natives_audio::get_current_music_id(&b), 2);
    // a second tune is taken (the stub streams are never active, so `soundsys.cpp:688` is always the first branch)
    mc2_sim::mission::natives_audio::play_digital_music(&mut b, 3);
    assert_eq!(mc2_sim::mission::natives_audio::get_current_music_id(&b), 3);
    // the id already playing is refused (`soundsys.cpp:681`); a tune with no file leaves it as it was
    mc2_sim::mission::natives_audio::play_digital_music(&mut b, 3);
    mc2_sim::mission::natives_audio::play_digital_music(&mut b, 9);
    assert_eq!(mc2_sim::mission::natives_audio::get_current_music_id(&b), 3);
    assert!(sim.world.audio.music_enabled && !sim.world.audio.music_streams_busy);
}

#[test]
fn without_a_sound_readback_the_music_poll_draws_every_frame() {
    let Some((mut sim, sources)) = load() else {
        return;
    };
    assert_eq!(sim.world.audio, AudioReadback::default());
    let base = draws(&sim);
    for i in 0..5u64 {
        music_block(&mut sim, &sources, 1);
        assert_eq!(draws(&sim), base + i + 1, "poll {i} drew once");
        assert_eq!(sim.world.audio.music_id, -1);
    }
}
