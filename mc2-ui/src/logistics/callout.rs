// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The tutorial's mouse callout (`MissionBegin::startAnimation`, `isInCalloutAnimation`, and the
//! tutorial block of `MissionBegin::update`, `code/missionbegin.cpp:98-113` and `:571-698`): the
//! pointer glides to a button (or a rect) of the current screen, then the target flashes. The
//! tutorial scripts start it through `logisticsAnimationCallout` and wait on `logisticsInCallout`.

use crate::input::UiInput;
use crate::screen::LogisticsScreen;

/// `mState_TUTORIALS` (`mclib/userinput.h`): the cursor while a callout runs.
pub const M_STATE_TUTORIALS: u32 = 44;

const WHITE: u32 = 0xffff_ffff;
const GREY: u32 = 0xff7f_7f7f;
const BLACK: u32 = 0xff00_0000;

/// The callout state the sequence keeps (the `animationRunning` and `targetButtonId` fields of
/// `MissionBegin`).
#[derive(Debug, Default)]
pub struct Callout {
    running: bool,
    /// `timeLeftToScroll`: seconds of glide left.
    time_left: f32,
    /// `targetButtonId`: a button's id, or a rect's index (`getRect`).
    target_id: i32,
    /// `targetIsButton`.
    target_is_button: bool,
    /// `buttonNumFlashes`.
    flashes: i32,
    /// `buttonFlashTime`.
    flash_time: f32,
}

impl Callout {
    /// `MissionBegin::startAnimation`: starts a callout unless one is running. Returns whether it
    /// started.
    pub fn start(&mut self, id: i32, is_button: bool, scroll_time: f32, flashes: i32) -> bool {
        if self.running {
            return false;
        }
        self.running = true;
        self.time_left = scroll_time;
        self.target_id = id;
        self.target_is_button = is_button;
        self.flashes = flashes;
        self.flash_time = 0.0;
        true
    }

    /// `isInCalloutAnimation`: whether a callout is running.
    pub fn in_callout(&self) -> bool {
        self.running
    }

    /// `MissionBegin::begin`'s reset of the callout (`animationRunning = false` and the rest).
    pub fn reset(&mut self) {
        *self = Callout::default();
    }

    /// One frame of the tutorial block: moves the pointer toward the target, then flashes it. A
    /// target that is not on `screen` ends the callout, as the C++ does. `dt` is the frame length.
    pub fn update(&mut self, screen: &mut LogisticsScreen, input: &mut UiInput, dt: f32) {
        if !self.running {
            return;
        }
        let target = if self.target_is_button {
            screen.button_mut(self.target_id).map(|b| {
                let o = &mut b.button.obj;
                (o.left(), o.right(), o.top(), o.bottom())
            })
        } else {
            usize::try_from(self.target_id)
                .ok()
                .and_then(|i| screen.rects.get(i))
                .map(|r| (r.obj.left(), r.obj.right(), r.obj.top(), r.obj.bottom()))
        };
        let Some((left, right, top, bottom)) = target else {
            self.running = false;
            return;
        };
        input.mouse_state = M_STATE_TUTORIALS;
        let (tx, ty) = ((left + right) * 0.5, (top + bottom) * 0.5);
        let (rx, ry) = input.real_mouse_pos();

        if self.time_left > 0.0 {
            let x_per_frame = (tx - rx) / self.time_left * dt;
            let y_per_frame = (ty - ry) / self.time_left * dt;
            input.set_mouse_pos(rx + x_per_frame, ry + y_per_frame);
            self.time_left -= dt;
            return;
        }

        // there: flash, then hand the cursor back
        input.set_mouse_pos(tx, ty);
        if self.flashes == 0 {
            self.running = false;
            self.set_target_color(screen, if self.target_is_button { WHITE } else { BLACK });
            return;
        }
        self.flash_time += dt;
        if self.flash_time > 0.5 {
            let c = if self.target_is_button { WHITE } else { BLACK };
            self.set_target_color(screen, c);
            self.flash_time = 0.0;
            self.flashes -= 1;
        } else if self.flash_time > 0.25 {
            let c = if self.target_is_button { GREY } else { WHITE };
            self.set_target_color(screen, c);
        }
    }

    fn set_target_color(&self, screen: &mut LogisticsScreen, c: u32) {
        if self.target_is_button {
            if let Some(b) = screen.button_mut(self.target_id) {
                b.button.obj.set_color(c, false);
            }
        } else if let Some(r) = usize::try_from(self.target_id)
            .ok()
            .and_then(|i| screen.rects.get_mut(i))
        {
            r.obj.set_color(c, false);
        }
    }
}
