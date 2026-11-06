//! Per-frame combat-unavailable record: `World::note_combat_unavailable` must be visible to callers through
//! `mc2_sim::ai::frame_unavailable`, once per hook name per frame (order of first occurrence).

use mc2_sim::rng::RngMode;
use mc2_sim::FrameUnavailable;

#[test]
fn note_combat_unavailable_is_reported_once_per_frame() {
    let mut w = mc2_sim::World::new(RngMode::LegacyMsvc, 1);
    w.note_combat_unavailable("hook-a");
    w.note_combat_unavailable("hook-a");
    w.note_combat_unavailable("hook-b");

    assert_eq!(
        mc2_sim::ai::frame_unavailable(&w),
        vec![
            FrameUnavailable::Combat { hook: "hook-a" },
            FrameUnavailable::Combat { hook: "hook-b" },
        ]
    );
}
