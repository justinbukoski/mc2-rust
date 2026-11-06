// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The timed effects of an icon: the death animation (`ForceGroupIcon::drawDeathEffect`,
//! `code/mechicon.cpp:1440-1502`) and the jumper flash (`code/forcegroupbar.cpp:329-363`).

/// `ForceGroupIcon::animationInfos` (`code/mechicon.cpp:76-85`): the time in seconds and the colour
/// at each stage of the death animation.
pub const DEATH_STAGES: [(f32, u32); 6] = [
    (0.0, 0x0000_0000),
    (2.5, 0x0000_0000),
    (5.0, 0x7fff_0000),
    (5.35, 0x7fff_0000),
    (6.0, 0xff00_0000),
    (6.5, 0xff00_0000),
];

/// `deathAnimationTime` after `beginDeathAnimation` (`code/mechicon.h:97`).
pub const DEATH_START: f32 = 0.0001;

/// The colour the finished animation leaves on the icon: the last stage (`mechicon.cpp:1495`).
pub const FINAL_COLOR: u32 = DEATH_STAGES[5].1;

/// One frame of the death animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    /// Fill the icon's selection area with `fill`; in the first stage also outline it with
    /// `outline` (`mechicon.cpp:1476-1488`).
    Dying { fill: u32, outline: Option<u32> },
    /// No stage covers this time: the animation is over and the unit leaves its force groups
    /// (`mechicon.cpp:1493-1501`).
    Finished,
}

/// `deathAnimationOver()` (`code/mechicon.h:98`): past the last stage.
pub fn over(t: f32) -> bool {
    t > DEATH_STAGES[5].0
}

/// What `drawDeathEffect` draws at death time `t`.
pub fn death_frame(t: f32) -> Frame {
    for i in 0..DEATH_STAGES.len() - 1 {
        let (t0, c0) = DEATH_STAGES[i];
        let (t1, c1) = DEATH_STAGES[i + 1];
        if t0 < t && t1 > t {
            let percent = (t - t0) / (t1 - t0);
            let fill = blend(c0, c1, percent);
            let outline = (i == 0).then(|| outline_colour(t));
            return Frame::Dying { fill, outline };
        }
    }
    Frame::Finished
}

/// The first stage's outline: red in the flashes, black otherwise (`mechicon.cpp:1481-1487`).
fn outline_colour(t: f32) -> u32 {
    if t < 0.4 || (t > 0.8 && t < 1.2) || (t > 1.6 && t < 2.0) {
        0xffff_0000
    } else {
        0xff00_0000
    }
}

/// One channel between two stages, with the C++ arithmetic: `(float)min + (float)(max - min) * p`
/// with the unsigned difference wrapping (`mechicon.cpp:1457-1472`), truncated to an integer.
fn channel(min: u32, max: u32, p: f32) -> u32 {
    (min as f32 + (max.wrapping_sub(min) as f32) * p) as u32
}

/// The colour between two stage colours. The C++ sums the channels as unsigned integers, so the
/// sums wrap (`mechicon.cpp:1474`).
fn blend(a: u32, b: u32, p: f32) -> u32 {
    let alpha = channel(a >> 24 & 0xff, b >> 24 & 0xff, p);
    let red = channel(a >> 16 & 0xff, b >> 16 & 0xff, p);
    let green = channel(a >> 8 & 0xff, b >> 8 & 0xff, p);
    let blue = channel(a & 0xff, b & 0xff, p);
    blue.wrapping_add(green << 8)
        .wrapping_add(red << 16)
        .wrapping_add(alpha << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_are_the_original_table() {
        // mechicon.cpp:76-85
        assert_eq!(DEATH_STAGES[2], (5.0, 0x7fff_0000));
        assert_eq!(DEATH_STAGES[5], (6.5, 0xff00_0000));
        assert_eq!(FINAL_COLOR, 0xff00_0000);
    }

    #[test]
    fn over_is_strictly_after_the_last_stage() {
        // mechicon.h:98 `deathAnimationTime > animationInfos[NUM_DEATH_INFOS-1].time`
        assert!(!over(6.5));
        assert!(over(6.51));
    }

    #[test]
    fn a_stage_blends_its_channels_linearly() {
        // t = 3.75 is halfway through stage 1 (2.5 to 5.0) from colour 0 to 0x7fff0000:
        // alpha 0 + 127 * 0.5 = 63.5 -> 63 (0x3f), red 0 + 255 * 0.5 = 127.5 -> 127 (0x7f)
        assert_eq!(
            death_frame(3.75),
            Frame::Dying {
                fill: 0x3f7f_0000,
                outline: None
            }
        );
    }

    #[test]
    fn the_first_stage_outlines_in_red_during_the_flashes() {
        // mechicon.cpp:1481-1487: red for t < .4, .8 < t < 1.2, 1.6 < t < 2.0, else black
        let red = |t| match death_frame(t) {
            Frame::Dying { outline, .. } => outline,
            Frame::Finished => None,
        };
        assert_eq!(red(0.3), Some(0xffff_0000));
        assert_eq!(red(1.0), Some(0xffff_0000));
        assert_eq!(red(1.5), Some(0xff00_0000));
        assert_eq!(red(1.9), Some(0xffff_0000));
    }

    #[test]
    fn the_animation_finishes_after_the_last_stage() {
        // no stage matches past 6.5: bFinished (mechicon.cpp:1493)
        assert_eq!(death_frame(6.6), Frame::Finished);
        // and the last stage fills black
        assert_eq!(
            death_frame(6.25),
            Frame::Dying {
                fill: 0xff00_0000,
                outline: None
            }
        );
    }
}
