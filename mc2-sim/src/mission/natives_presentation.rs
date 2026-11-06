// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Presentation natives of `code/ablmc2.cpp`: movie mode with its letterbox and fade (`Camera::setMovieMode`, `endMovieMode`,
//! `fadeToColor`, `forceMovieToEnd` and `updateLetterboxAndFade`, `mclib/camera.h:724-760` and `mclib/camera.cpp:1440-1490`),
//! `PlayVideo` (`ControlGui::playMovie`, `controlgui.cpp:2851`), `TutorialText` (`MissionInterfaceManager::setTutorialText`,
//! `missiongui.cpp:418`), the mouse callout (`MissionInterfaceManager::startAnimation`, `missiongui.cpp:367-410`, and its frame
//! block, `:455-530`), `FreezeGUI` and `GUIIsAOEStyle`.
//!
//! The simulation owns this state because the scripts read part of it back (`ForceMovieEnd`, `InCallout`, `GUIIsAOEStyle`); the
//! application draws what it says (the letterbox bars, the fade overlay, the callout's flash, the movie in its video window) and
//! sends the player's keys (ESC in movie mode, the mouse profile) as [`PlayerCommand`]s. The application also gates its own input:
//! the C++ GUI update returns before any command while the movie runs, the GUI is frozen or a callout runs (`missiongui.cpp:443-
//! 455`, `:476`).
//!
//! The C++ defects fixed here (each is also in `rust/docs/DIFFERENCES.md`):
//! - `EndMovieMode` right after `SetMovieMode` (same frame, before the first letterbox step) never closed: the ending branch
//!   required the bars to be open (`letterBoxPos != 0`), so the camera stayed in movie mode for the rest of the mission and the
//!   GUI stayed hidden. The bars now close from wherever they are.
//! - `FadeToColor` took its time as a `DWORD` (`ablmc2.cpp:5281`): the script's real is read by `ABLi_popReal` and stored into
//!   an integer, so 2.5 s became 2 s and any time under one second did nothing. The real time is used.
//! - The fade of a movie that ends (or that its script closes) in this frame still steps in this frame; the C++ cleared
//!   `inMovieMode` in the letterbox pass first and skipped it (`mclib/camera.cpp:1479`).

use crate::events::SimEvent;
use crate::world::World;
use mc2_sim_data::objtype::weaponbolt::STUFF_SMALL;

/// `Camera::MaxLetterBoxTime` (`mclib/camera.cpp:145`): seconds for the bars to open or close.
pub const MAX_LETTERBOX_TIME: f32 = 5.0;
/// `Camera::MaxLetterBoxPos` (`mclib/camera.cpp:146`): the fraction of the screen each black bar covers when open.
pub const MAX_LETTERBOX_POS: f32 = 0.15;
/// `TACMAP_ID` (`code/missiongui.cpp:364`): the rect a callout on the tactical map targets.
pub const TACMAP_ID: i32 = 0;
/// `FLASH_JUMPERS` (`code/missiongui.cpp:366`): flashes the mech icons and ends at once.
pub const FLASH_JUMPERS: i32 = 31798;
/// `RPTOTAL_CALLOUT` (`code/controlgui.h:56`): the resource-point total on the support palette.
pub const RPTOTAL_CALLOUT: i32 = 21185;
/// `IDS_TUTORIAL` (`resource.h:4360`): the sender name a tutorial line is shown under.
pub const IDS_TUTORIAL: u32 = 60000;
/// `buttonFlashTime > .5f` (`missiongui.cpp:501`): one flash of the callout target.
pub const FLASH_PERIOD: f32 = 0.5;

/// `MissionInterfaceManager`'s callout: `animationRunning` and the fields `startAnimation` sets.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Callout {
    /// `animationRunning`.
    pub running: bool,
    /// `timeLeftToScroll`: seconds of pointer glide left.
    pub time_left: f32,
    /// `targetButtonId`: a button id, or a rect id (`TACMAP_ID` for the tactical map).
    pub target: i32,
    /// `targetIsButton`.
    pub target_is_button: bool,
    /// `targetIsPressed`: the button is pressed when the callout ends (`pushButton`).
    pub pressed_at_end: bool,
    /// `buttonNumFlashes`: flashes left.
    pub flashes: i32,
    /// `buttonFlashTime`: seconds into the current flash.
    pub flash_time: f32,
    /// `abs(buttonId)` of the last `startAnimation` that flashed a tac-map objective (`missiongui.cpp:381-383`), 0 if none.
    pub objective_id: i32,
    /// `numFlashes` of that start (`flashes` counts down while the callout runs).
    pub objective_flashes: i32,
    /// Counts those starts, so the application sees each one once (`controlGui.animateTacMap`).
    pub objective_serial: u32,
}

impl Callout {
    /// `MissionInterfaceManager::startAnimation` (`missiongui.cpp:367-410`). Returns false, and changes nothing, while a callout
    /// runs. The tactical-map and resource-point targets are rects; `FLASH_JUMPERS` ends at once.
    pub fn start(
        &mut self,
        button_id: i32,
        is_button: bool,
        is_pressed: bool,
        time_to_scroll: f32,
        flashes: i32,
    ) -> bool {
        if self.running {
            return false;
        }
        let Some(objective_id) = (if button_id < 0 {
            button_id.checked_neg()
        } else {
            Some(0)
        }) else {
            return false;
        };
        let serial = self.objective_serial;
        *self = Callout {
            running: true,
            time_left: time_to_scroll,
            target: button_id,
            target_is_button: is_button,
            pressed_at_end: is_pressed,
            flashes,
            flash_time: 0.0,
            objective_id: 0,
            objective_flashes: 0,
            objective_serial: serial,
        };
        if button_id < 0 {
            self.objective_id = objective_id;
            self.objective_flashes = flashes;
            self.objective_serial = serial.wrapping_add(1);
            // the tactical map's objective flash
            self.target = TACMAP_ID;
            self.target_is_button = false;
            self.pressed_at_end = false;
        }
        if button_id == RPTOTAL_CALLOUT {
            self.target_is_button = false;
            self.pressed_at_end = false;
        }
        if button_id == FLASH_JUMPERS {
            self.running = false;
        }
        true
    }

    /// One frame of the callout block of `MissionInterfaceManager::update` (`missiongui.cpp:455-530`): the pointer glides for
    /// `time_left`, then the target flashes `flashes` times, then the callout ends on the following frame. The glide itself is the
    /// application's; the timing is exact.
    pub fn advance(&mut self, frame_length: f32) {
        if !self.running {
            return;
        }
        if self.time_left > 0.0 {
            self.time_left -= frame_length;
        } else if self.flashes != 0 {
            self.flash_time += frame_length;
            if self.flash_time > FLASH_PERIOD {
                self.flash_time = 0.0;
                self.flashes -= 1;
            }
        } else {
            self.running = false;
        }
    }

    /// Whether the target is shown in its dark half (`buttonFlashTime > .25f`, `missiongui.cpp:507`) while flashing.
    pub fn dark_phase(&self) -> bool {
        self.running
            && self.time_left <= 0.0
            && self.flashes != 0
            && self.flash_time > FLASH_PERIOD / 2.0
    }
}

/// The presentation state of the mission: `Camera`'s movie fields, the GUI's freeze and mouse profile, and the callout.
#[derive(Clone, Debug, PartialEq)]
pub struct Presentation {
    /// `Camera::inMovieMode`.
    pub movie_mode: bool,
    /// `Camera::startEnding`: the bars are closing.
    pub ending: bool,
    /// `Camera::forceMovieEnd`: the player pressed ESC in movie mode.
    pub force_movie_end: bool,
    /// `Camera::letterBoxTime`: seconds of the bars' opening, 0 to [`MAX_LETTERBOX_TIME`] while the movie runs.
    pub letterbox_time: f32,
    /// `Camera::letterBoxPos`: the fraction of the screen each bar covers, the value the camera draws (`letterBoxPercent *
    /// MaxLetterBoxPos`, `mclib/camera.cpp:1453` and `:1473`). It is kept because the closing pass tests it for zero, and it does
    /// not follow `letterbox_time` exactly at the ends (a closing movie's time goes below zero once it has ended).
    pub letterbox_pos: f32,
    /// `Camera::inFadeMode`.
    pub in_fade: bool,
    /// `Camera::fadeColor`: the ARGB the fade is heading to.
    pub fade_color: u32,
    /// `Camera::fadeStart`: the ARGB the fade started from (the previous target).
    pub fade_start: u32,
    /// `Camera::timeLeftToFade`.
    pub fade_time_left: f32,
    /// `Camera::startFadeTime`.
    pub fade_time: f32,
    /// `Camera::fadeAlpha`: the overlay's current alpha, 0 to 255.
    pub fade_alpha: u32,
    /// `MissionInterfaceManager::guiFrozen` (`freezeGUI`).
    pub gui_frozen: bool,
    /// `useLeftRightMouseProfile` (`isAOEStyle`); the default is the AOE profile (`mechcmd2.cpp:106`).
    pub aoe_style: bool,
    /// The mission's mouse callout.
    pub callout: Callout,
}

impl Default for Presentation {
    fn default() -> Self {
        Presentation {
            movie_mode: false,
            ending: false,
            force_movie_end: false,
            letterbox_time: 0.0,
            letterbox_pos: 0.0,
            in_fade: false,
            fade_color: 0,
            fade_start: 0,
            fade_time_left: 0.0,
            fade_time: 0.0,
            fade_alpha: 0,
            gui_frozen: false,
            aoe_style: true,
            callout: Callout::default(),
        }
    }
}

impl Presentation {
    /// The letterbox the camera draws (`gamecam.cpp:199-226`): the height fraction each black bar covers, and the bars' alpha.
    /// Both are zero outside movie mode.
    pub fn letterbox(&self) -> (f32, u8) {
        if !self.movie_mode {
            return (0.0, 0);
        }
        let percent = self.letterbox_time / MAX_LETTERBOX_TIME;
        (self.letterbox_pos, (percent * 255.0) as u8)
    }

    /// The ARGB of the fade overlay, if one covers the screen (`gamecam.cpp:302`): only in movie mode and with a non-zero alpha.
    pub fn fade_overlay(&self) -> Option<u32> {
        (self.movie_mode && self.fade_alpha != 0)
            .then_some((self.fade_alpha << 24) | (self.fade_color & 0x00ff_ffff))
    }
}

/// `Camera::update`'s `updateLetterboxAndFade` (`mclib/camera.cpp:1440-1490`, called from `Camera::update`, `:1601`), one frame.
/// It runs inside `eye->update()` (`mission.cpp:478`), after `missionInterface->update()` and so after the whole rest of the
/// mission update: the movie mode and fade a frame's script sets or ends are stepped by the same frame. The bars open while movie
/// mode runs and close after [`end_movie_mode`]; the fade moves only in movie mode (`We only fades during movie Mode at present!!`).
///
/// One C++ defect is fixed here (see `rust/docs/DIFFERENCES.md`): a running fade still steps in the frame the movie ends, which
/// the C++ skipped because `inMovieMode` had been cleared first (ending the movie mid-fade left the screen at its last alpha).
pub fn advance_camera(w: &mut World) {
    let fl = w.clock.frame_length;
    let p = &mut w.presentation;
    let movie_ran = p.movie_mode;
    // The opening pass (camera.cpp:1443-1454).
    if !p.ending && p.movie_mode && p.letterbox_time != MAX_LETTERBOX_TIME {
        p.letterbox_time += fl;
        let mut percent = p.letterbox_time / MAX_LETTERBOX_TIME;
        if p.letterbox_time > MAX_LETTERBOX_TIME {
            percent = 1.0;
            p.letterbox_time = MAX_LETTERBOX_TIME;
        }
        p.letterbox_pos = percent * MAX_LETTERBOX_POS;
    }
    // The closing pass (camera.cpp:1458-1474), without the C++ guard `letterBoxPos != 0.0f`: a movie that SetMovieMode started
    // and EndMovieMode closed in the same script run reaches this pass with the bars still shut (the opening pass is skipped
    // while it is ending), the guard fails and the bars never close, so the camera stayed in movie mode for the rest of the
    // mission. A closing movie closes from wherever its bars are.
    if p.ending && p.movie_mode {
        if p.letterbox_time <= fl {
            // the bars are shut (or were never opened: a movie closed in the frame it started): the movie is over
            p.letterbox_time = 0.0;
            p.letterbox_pos = 0.0;
            p.movie_mode = false;
            p.ending = false;
        } else {
            p.letterbox_time -= fl;
            p.letterbox_pos = p.letterbox_time / MAX_LETTERBOX_TIME * MAX_LETTERBOX_POS;
        }
    }
    if (p.movie_mode || movie_ran) && p.in_fade {
        p.fade_time_left += fl;
        let mut percent = p.fade_time_left / p.fade_time;
        if p.fade_time_left > p.fade_time {
            percent = 1.0;
            p.in_fade = false;
        }
        let from = (p.fade_start >> 24) as f32;
        let to = (p.fade_color >> 24) as f32;
        p.fade_alpha = (from + percent * (to - from)) as u32;
    }
}

/// The callout block of `MissionInterfaceManager::update` (`missiongui.cpp:455-530`), one frame. It runs in the GUI update, which
/// returns before it while the movie plays (`:443-448`): a callout waits for the movie to end.
pub fn advance_callout(w: &mut World) {
    let fl = w.clock.frame_length;
    if w.presentation.callout.running && !w.presentation.movie_mode {
        w.presentation.callout.advance(fl);
    }
}

/// `SetMovieMode` (`ablmc2.cpp:5189`): `Camera::setMovieMode`. Entering movie mode starts the bars opening; a second call while
/// in movie mode does nothing.
pub fn set_movie_mode(w: &mut World) {
    let p = &mut w.presentation;
    if !p.movie_mode {
        p.movie_mode = true;
        p.ending = false;
        p.force_movie_end = false;
        p.letterbox_time = 0.0;
        p.letterbox_pos = 0.0;
    }
}

/// `EndMovieMode` (`ablmc2.cpp:5209`): `Camera::endMovieMode`. The bars start closing; movie mode ends when they are shut.
pub fn end_movie_mode(w: &mut World) {
    let p = &mut w.presentation;
    if p.movie_mode {
        p.ending = true;
    }
}

/// `FadeToColor` (`ablmc2.cpp:5267`): `Camera::fadeToColor`. The fade starts from the previous target's alpha and is ignored while
/// another fade runs or when its time is below [`STUFF_SMALL`]. The time is the real value (see the module docs).
pub fn fade_to_color(w: &mut World, color: i32, time: f32) {
    let p = &mut w.presentation;
    if !p.in_fade && time > STUFF_SMALL {
        p.in_fade = true;
        p.fade_start = p.fade_color;
        p.fade_color = color as u32;
        p.fade_time_left = 0.0;
        p.fade_time = time;
    }
}

/// `ForceMovieEnd` (`ablmc2.cpp:5290`): 1 if the player asked the movie to end, 0 otherwise.
pub fn force_movie_end(w: &World) -> i32 {
    i32::from(w.presentation.force_movie_end)
}

/// `PlayVideo` (`ablmc2.cpp:2655`): `ControlGui::playMovie` starts the movie in the video window; the script goes on at once and
/// gets 0. The application decides whether a movie already playing lets the new one start (`if (moviePlaying) return;`).
pub fn play_video(w: &mut World, name: String) -> i32 {
    w.events.push(SimEvent::PlayVideo { name });
    0
}

/// `TutorialText` (`ablmc2.cpp:5832`): the text id goes to the chat log under the tutorial's name. The application resolves the
/// string.
pub fn tutorial_text(w: &mut World, text_id: i32) {
    w.events.push(SimEvent::TutorialText { text_id });
}

/// `AnimationCallout` (`ablmc2.cpp:5811`): `MissionInterfaceManager::startAnimation`.
pub fn animation_callout(
    w: &mut World,
    button_id: i32,
    is_button: bool,
    is_pressed: bool,
    time_to_scroll: f32,
    num_flashes: i32,
) -> bool {
    w.presentation.callout.start(
        button_id,
        is_button,
        is_pressed,
        time_to_scroll,
        num_flashes,
    )
}

/// `InCallout` (`ablmc2.cpp:5948`): `MissionInterfaceManager::isInCalloutAnimation`.
pub fn in_callout(w: &World) -> bool {
    w.presentation.callout.running
}

/// `GUIIsAOEStyle` (`ablmc2.cpp:5851`): `isAOEStyle`.
pub fn gui_is_aoe_style(w: &World) -> bool {
    w.presentation.aoe_style
}

/// `FreezeGUI` (`ablmc2.cpp:5872`): `MissionInterfaceManager::freezeGUI`.
pub fn freeze_gui(w: &mut World, flag: bool) {
    w.presentation.gui_frozen = flag;
}

/// The player pressed ESC (`MissionInterfaceManager::update`, `missiongui.cpp:443-446`: `eye->forceMovieToEnd()`): it asks the
/// movie to end. The application sends the command only when the GUI update would read the key, which is in movie mode only, and
/// the `ForceMovieEnd` native reads the flag whatever the mode: so, like the C++, a request made in movie mode stays readable after
/// the movie has ended (mc2_01 reads it in the same script run that calls `EndMovieMode`).
pub fn player_force_movie_end(w: &mut World) {
    w.presentation.force_movie_end = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    const FL: f32 = 0.25;

    fn world() -> World {
        World::new(crate::rng::RngMode::Pcg32, 1)
    }

    /// `frames` frames of the camera's end of frame and the next frame's GUI callout block, in their order, at `FL` (0.25 s).
    /// `begin_frame` sets `frameLength` as `DoGameLogic` does: the first frame of a world has the default 0.05 s as in the C++
    /// (`mclib/timing.cpp:20`), so every frame here starts the clock first.
    fn run(w: &mut World, frames: u32) {
        for _ in 0..frames {
            w.clock.begin_frame(1.0 / FL, 0, false);
            advance_camera(w);
            advance_callout(w);
        }
    }

    #[test]
    fn the_letterbox_opens_over_five_seconds() {
        let mut w = world();
        set_movie_mode(&mut w);
        run(&mut w, 10); // 2.5 s
        let (pos, alpha) = w.presentation.letterbox();
        assert!((pos - MAX_LETTERBOX_POS / 2.0).abs() < 1e-6, "pos {pos}");
        assert_eq!(alpha, 127);
        run(&mut w, 10); // 5 s, and no further
        run(&mut w, 10);
        let (pos, alpha) = w.presentation.letterbox();
        assert!((pos - MAX_LETTERBOX_POS).abs() < 1e-6);
        assert_eq!(alpha, 255);
        assert!(w.presentation.movie_mode && !w.presentation.ending);
    }

    #[test]
    fn end_movie_mode_closes_the_bars_and_then_leaves_movie_mode() {
        let mut w = world();
        set_movie_mode(&mut w);
        run(&mut w, 20); // fully open
        end_movie_mode(&mut w);
        run(&mut w, 10); // 2.5 s: half closed, still in movie mode
        assert!(w.presentation.movie_mode);
        let (pos, _) = w.presentation.letterbox();
        assert!((pos - MAX_LETTERBOX_POS / 2.0).abs() < 1e-6);
        run(&mut w, 10);
        assert!(!w.presentation.movie_mode && !w.presentation.ending);
        assert_eq!(w.presentation.letterbox(), (0.0, 0));
    }

    #[test]
    fn a_movie_ended_in_the_frame_it_starts_still_ends() {
        // SetMovieMode and EndMovieMode in the same script run: the frame's closing pass shuts the bars at once. The C++ kept
        // this movie open for good (camera.cpp:1458 only closes from `letterBoxPos != 0`, which an opening pass skipped by
        // `startEnding` never sets), so the camera and the GUI stayed in movie mode for the rest of the mission.
        let mut w = world();
        set_movie_mode(&mut w);
        end_movie_mode(&mut w);
        w.clock.begin_frame(1.0 / FL, 0, false);
        advance_camera(&mut w);

        assert!(!w.presentation.movie_mode && !w.presentation.ending);
        assert_eq!(w.presentation.letterbox(), (0.0, 0));
        assert_eq!(w.presentation.letterbox_time, 0.0);
    }

    #[test]
    fn set_movie_mode_is_ignored_while_in_movie_mode_and_clears_the_force_flag() {
        let mut w = world();
        set_movie_mode(&mut w);
        run(&mut w, 4);
        let before = w.presentation.letterbox_time;
        set_movie_mode(&mut w);
        assert_eq!(
            w.presentation.letterbox_time, before,
            "no reset of the opening"
        );
        w.presentation.force_movie_end = true;
        end_movie_mode(&mut w);
        set_movie_mode(&mut w); // ignored: ending keeps going
        assert!(w.presentation.ending);
        // a fresh start (after the movie has ended) clears the request; the closing bars take five seconds
        let mut frames = 0;
        while w.presentation.movie_mode {
            run(&mut w, 1);
            frames += 1;
            assert!(frames < 60, "the movie never ended");
        }
        set_movie_mode(&mut w);
        assert!(!w.presentation.force_movie_end);
    }

    #[test]
    fn end_movie_mode_outside_movie_mode_does_nothing() {
        let mut w = world();
        end_movie_mode(&mut w);
        assert!(!w.presentation.ending);
    }

    #[test]
    fn force_movie_end_reads_the_player_request() {
        let mut w = world();
        assert_eq!(force_movie_end(&w), 0, "nothing asked yet");
        set_movie_mode(&mut w);
        player_force_movie_end(&mut w);
        assert_eq!(force_movie_end(&w), 1);
        // The request survives the script closing the movie in the same run (mc2_01 reads ForceMovieEnd, then calls
        // EndMovieMode); a later SetMovieMode clears it (Camera::setMovieMode, mclib/camera.h:730).
        end_movie_mode(&mut w);
        run(&mut w, 20);
        assert!(!w.presentation.movie_mode);
        assert_eq!(force_movie_end(&w), 1);
        set_movie_mode(&mut w);
        assert_eq!(force_movie_end(&w), 0);
    }

    #[test]
    fn a_movie_ended_during_a_fade_still_steps_the_fade_in_that_frame() {
        let mut w = world();
        set_movie_mode(&mut w);
        // The bars open over MaxLetterBoxTime: 0.25 s adds up exactly and is capped on the 20th step.
        run(&mut w, 25);
        assert_eq!(w.presentation.letterbox_time, MAX_LETTERBOX_TIME);
        fade_to_color(&mut w, 0xff00_0000u32 as i32, 2.0); // FadeToColor(color, time)
        run(&mut w, 2); // 0.5 s of 2 s: a quarter of the way
        assert_eq!(w.presentation.fade_alpha, 63);
        // The movie ends now. The C++ steps the fade only while `inMovieMode && inFadeMode` (mclib/camera.cpp:1479). With the
        // bars fully open, the closing pass runs in the next frame and clears `inMovieMode` before the fade pass of that same
        // frame, so the C++ freeze leaves the fade at 63 for good. The Rust ports the frame's own fade step (the scene fades
        // out over the whole closing, then the fade finishes with the bars): it keeps stepping while the bars close.
        end_movie_mode(&mut w);
        let mut frames = 0;
        while w.presentation.movie_mode {
            run(&mut w, 1);
            frames += 1;
            assert!(frames < 40, "the closing bars never shut");
        }
        assert!(
            w.presentation.fade_alpha > 63,
            "the fade stepped while the bars closed: {}",
            w.presentation.fade_alpha
        );
        // the movie is over: the fade no longer moves
        let ended = w.presentation.fade_alpha;
        run(&mut w, 4);
        assert_eq!(w.presentation.fade_alpha, ended);
    }

    #[test]
    fn a_fade_runs_only_in_movie_mode_and_reaches_the_target() {
        let mut w = world();
        fade_to_color(&mut w, 0xff00_0000u32 as i32, 2.0);
        run(&mut w, 12); // 3 s outside movie mode: nothing moves
        assert_eq!(w.presentation.fade_alpha, 0);
        assert!(w.presentation.in_fade);
        set_movie_mode(&mut w);
        run(&mut w, 4); // 1 s into a 2 s fade: half way
        assert_eq!(w.presentation.fade_alpha, 127);
        assert_eq!(w.presentation.fade_overlay(), Some(0x7f00_0000));
        run(&mut w, 4); // 2 s: the target alpha, the fade ends once its time is passed (`>`)
        assert_eq!(w.presentation.fade_alpha, 255);
        assert!(w.presentation.in_fade);
        run(&mut w, 1);
        assert!(!w.presentation.in_fade);
        assert_eq!(w.presentation.fade_alpha, 255);
    }

    #[test]
    fn the_fade_takes_fractional_seconds() {
        // C++ stored the time as a DWORD: 0.5 s became 0 and no fade started.
        let mut w = world();
        set_movie_mode(&mut w);
        fade_to_color(&mut w, 0xff00_0000u32 as i32, 0.5);
        assert!(w.presentation.in_fade);
        run(&mut w, 3);
        assert_eq!(w.presentation.fade_alpha, 255);
        assert!(!w.presentation.in_fade);
    }

    #[test]
    fn a_fade_below_stuff_small_or_during_a_fade_is_ignored() {
        let mut w = world();
        set_movie_mode(&mut w);
        fade_to_color(&mut w, 0xff00_0000u32 as i32, 0.0);
        assert!(!w.presentation.in_fade);
        fade_to_color(&mut w, 0xff00_0000u32 as i32, 4.0);
        let started = w.presentation.fade_time;
        fade_to_color(&mut w, 0x80ff_ffff_u32 as i32, 1.0);
        assert_eq!(
            w.presentation.fade_time, started,
            "the running fade is kept"
        );
        assert_eq!(w.presentation.fade_color, 0xff00_0000);
    }

    #[test]
    fn the_next_fade_starts_from_the_previous_target_alpha() {
        let mut w = world();
        set_movie_mode(&mut w);
        run(&mut w, 20);
        // the first fade ends at 0x80 alpha (the default start is 0)
        fade_to_color(&mut w, 0x8012_3456_u32 as i32, 1.0);
        run(&mut w, 5);
        assert_eq!(w.presentation.fade_alpha, 0x80);
        // the next one starts at the previous target's alpha, 0x80, and keeps the old colour as its start
        fade_to_color(&mut w, 0x0000_0000, 1.0);
        assert_eq!(w.presentation.fade_start, 0x8012_3456);
        run(&mut w, 1);
        assert_eq!(
            w.presentation.fade_alpha, 0x60,
            "a quarter of the way from 0x80 to 0"
        );
    }

    #[test]
    fn the_callout_runs_its_glide_then_its_flashes_then_ends() {
        let mut w = world();
        assert!(animation_callout(&mut w, 7, true, false, 1.0, 2));
        assert!(in_callout(&w));
        // 1.0 s glide = 4 frames of 0.25 s; then 2 flashes of 0.5 s (3 frames each: 0.25, 0.5, 0.75); then one frame to end
        run(&mut w, 4 + 3 + 3);
        assert!(
            in_callout(&w),
            "the last flash has ended, the callout ends on the next frame"
        );
        run(&mut w, 1);
        assert!(!in_callout(&w));
    }

    #[test]
    fn a_callout_with_no_flashes_ends_after_its_glide() {
        let mut w = world();
        assert!(animation_callout(&mut w, 7, true, true, 0.5, 0));
        run(&mut w, 2); // glide 0.5 s
        assert!(in_callout(&w));
        run(&mut w, 1);
        assert!(!in_callout(&w));
        assert!(
            w.presentation.callout.pressed_at_end,
            "the button is pressed at the end"
        );
    }

    #[test]
    fn a_tac_map_objective_callout_keeps_its_objective_id() {
        let mut c = Callout::default();
        assert!(c.start(-3, false, false, 0.0, 2));
        assert_eq!(
            (c.target, c.objective_id, c.objective_serial),
            (TACMAP_ID, 3, 1)
        );
        let mut b = Callout::default();
        assert!(b.start(7, true, false, 0.0, 2));
        assert_eq!((b.objective_id, b.objective_serial), (0, 0));
    }

    #[test]
    fn the_most_negative_button_id_is_refused_without_changing_state() {
        let mut w = world();
        assert!(!animation_callout(&mut w, i32::MIN, false, false, 1.0, 2));
        assert!(!w.presentation.callout.running);
        assert_eq!(w.presentation.callout.objective_serial, 0);
        assert!(animation_callout(&mut w, -3, false, false, 1.0, 2));
    }

    #[test]
    fn a_second_callout_is_refused_while_one_runs() {
        let mut w = world();
        assert!(animation_callout(&mut w, 7, true, false, 1.0, 1));
        assert!(!animation_callout(&mut w, 8, true, false, 0.0, 0));
        assert_eq!(w.presentation.callout.target, 7);
    }

    #[test]
    fn a_tactical_map_callout_targets_the_map_rect() {
        let mut w = world();
        assert!(animation_callout(&mut w, -12, true, true, 0.0, 0));
        assert_eq!(w.presentation.callout.target, TACMAP_ID);
        assert!(!w.presentation.callout.target_is_button);
        assert!(!w.presentation.callout.pressed_at_end);
    }

    #[test]
    fn the_jumper_flash_starts_and_ends_at_once() {
        let mut w = world();
        assert!(animation_callout(
            &mut w,
            FLASH_JUMPERS,
            true,
            false,
            5.0,
            3
        ));
        assert!(!in_callout(&w));
    }

    #[test]
    fn the_callout_does_not_run_in_movie_mode() {
        let mut w = world();
        animation_callout(&mut w, 7, true, false, 0.0, 0);
        set_movie_mode(&mut w);
        run(&mut w, 8);
        assert!(in_callout(&w), "the GUI update returns in movie mode");
        end_movie_mode(&mut w);
        run(&mut w, 20); // the bars close and the movie ends
        assert!(!w.presentation.movie_mode);
        run(&mut w, 1);
        assert!(!in_callout(&w));
    }

    #[test]
    fn freeze_and_profile_are_stored() {
        let mut w = world();
        assert!(gui_is_aoe_style(&w), "the C++ default is the AOE profile");
        freeze_gui(&mut w, true);
        assert!(w.presentation.gui_frozen);
        freeze_gui(&mut w, false);
        assert!(!w.presentation.gui_frozen);
        w.presentation.aoe_style = false;
        assert!(!gui_is_aoe_style(&w));
    }

    #[test]
    fn play_video_and_tutorial_text_queue_their_requests() {
        let mut w = world();
        assert_eq!(play_video(&mut w, "cinema1".to_string()), 0);
        tutorial_text(&mut w, 1234);
        assert_eq!(
            w.events,
            vec![
                SimEvent::PlayVideo {
                    name: "cinema1".to_string()
                },
                SimEvent::TutorialText { text_id: 1234 },
            ]
        );
    }

    #[test]
    fn the_fade_overlay_is_drawn_only_in_movie_mode_and_above_zero_alpha() {
        let mut w = world();
        fade_to_color(&mut w, 0xff00_0000u32 as i32, 1.0);
        assert_eq!(w.presentation.fade_overlay(), None, "not in movie mode");
        set_movie_mode(&mut w);
        assert_eq!(w.presentation.fade_overlay(), None, "alpha is still zero");
        run(&mut w, 4);
        assert_eq!(
            w.presentation.fade_overlay(),
            Some(0xff00_0000),
            "the full fade covers the screen"
        );
    }
}
