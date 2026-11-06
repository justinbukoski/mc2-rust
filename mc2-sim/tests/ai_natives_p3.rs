//! Headless P3 evidence for the pilot-AI ABL natives reached by shipped mc2_01.
//!
//! The mission is loaded and stepped by Rust alone. No oracle state is
//! installed: the assertion is that the exact native ids called by the
//! shipped brains are reached, implemented calls stay available, and video
//! remains explicitly unavailable. The no-input span deliberately remains
//! `PLAYING`: completion needs player orders and is proven by
//! `mc2-app/tests/ai_natives_p3.rs`'s production replay.

use std::rc::Rc;

use mc2_sim::ai::AiUnavailable;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim, SimEvent};

mod common;

/// P3's winning mc2_01 trial reached these native ids.  Their names and C++
/// implementations are recorded in `rust/docs/porting-log.md`.
const HEADLESS_P3_NATIVE_IDS: [u16; 5] = [69, 191, 207, 225, 228];
/// `stopvoiceover` is reached by the full production player replay, after
/// player-driven mission voice-over. The no-input headless brain run below
/// never enters that mission sequence; its direct ABL binding is covered by
/// `ai::tests::stop_voice_over_forwards_the_cancellation_request`.
const PLAYER_REPLAY_ONLY_NATIVE_ID: u16 = 226;
/// Video playback is a simulation-to-application request; reaching it does not stall the mission script.
const VIDEO_NATIVE_ID: u16 = 72;
/// Application-owned audio and callout readbacks are mirrored into
/// deterministic simulation state.
const AUDIO_CALLOUT_READBACK_NATIVE_IDS: [u16; 3] = [192, 224, 227];

/// Run through the P3 trial's 13,200-frame duration at 30 fps with all
/// visibility supplied (the standard headless input). The trace is collected
/// from `SimBindings::native_called`, so success cannot be vacuous: every
/// claimed implementation must actually be reached. Audio and callout
/// readbacks are reached without crossing the unavailable boundary; video
/// playback remains the one explicit presentation dependency.
#[test]
fn mc2_01_pilot_natives_are_available_through_the_p3_headless_run() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("system.cfg");
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
    )
    .expect("mc2_01 loads");
    assert!(loaded.complete, "missing hooks: {:?}", loaded.missing_hooks);
    let mut sim = Sim::new(loaded);
    let mut completed_frames = 0;
    let mut reached = std::collections::BTreeSet::new();
    let mut callers = std::collections::BTreeMap::<u16, std::collections::BTreeSet<i32>>::new();
    let mut unavailable = std::collections::BTreeSet::new();
    let mut final_result = mc2_sim::mission::result::PLAYING;
    for frame in 1..=13_200 {
        let output = sim
            .step(FrameInput::fixed(
                30,
                frame,
                Rc::new(AllVisible),
                Vec::new(),
            ))
            .expect("complete loaded mission steps");
        for event in output.events {
            if let SimEvent::AiUnavailable {
                what: AiUnavailable::Native { id },
                ..
            } = event
            {
                unavailable.insert(id);
            }
        }
        for &(part_id, id) in &sim.world.ai_native_calls_frame {
            reached.insert(id);
            callers.entry(id).or_default().insert(part_id);
        }
        final_result = output.result;
        completed_frames = frame;
    }
    assert_eq!(
        completed_frames, 13_200,
        "headless run completed its P3 span"
    );
    assert_eq!(sim.world.clock.turn, 13_200, "one simulation turn per step");
    assert_eq!(
        final_result,
        mc2_sim::mission::result::PLAYING,
        "the no-input brain span remains playing; the player replay owns completion evidence"
    );
    for id in HEADLESS_P3_NATIVE_IDS {
        assert!(
            reached.contains(&id),
            "implemented native {id} was not reached"
        );
        assert!(
            !unavailable.contains(&id),
            "implemented native {id} was reported unavailable"
        );
        assert!(
            callers.get(&id) == Some(&std::collections::BTreeSet::from([0])),
            "implemented native {id} is reached by mc2_01's global mission ABL context, got {:?}",
            callers.get(&id)
        );
    }
    assert!(
        !reached.contains(&PLAYER_REPLAY_ONLY_NATIVE_ID),
        "the no-input run unexpectedly entered the player voice-over sequence"
    );
    for id in AUDIO_CALLOUT_READBACK_NATIVE_IDS {
        assert!(
            reached.contains(&id),
            "audio/callout readback native {id} was not reached"
        );
        assert!(
            !unavailable.contains(&id),
            "audio/callout readback native {id} was reported unavailable"
        );
    }
    assert!(
        reached.contains(&VIDEO_NATIVE_ID),
        "video native was not reached"
    );
    assert!(
        !unavailable.contains(&VIDEO_NATIVE_ID),
        "video playback request was reported unavailable"
    );
}
