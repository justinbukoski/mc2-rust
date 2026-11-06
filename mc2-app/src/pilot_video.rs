// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The pilot talking-head video in the unit bar (`ForceGroupBar::setPilotVideo`, `code/forcegroupbar.cpp:411-512`;
//! `ControlGui::playPilotVideo` / `endPilotVideo`, `code/controlgui.cpp:3170-3197`).
//!
//! A radio message with a movie code starts `data/movies/<first 7 letters of the pilot's name><code>.mpg` in the
//! pilot's unit-bar slot ([`mc2_ui::unitbar::UnitBar::pilot_video_rect`]); it ends with the message
//! (`PilotVideoEnd`), with the movie itself, or when the unit shuts down. One video plays at a time, none while a
//! mission video plays, and none when the Pilot Videos option is off. The movie plays on the same
//! [`MovieControl`] as the logistics and mission videos, without sound: the pilot's voice is the radio message.

use crate::movie_control::{MovieControl, Playback};
use mc2_formats::vfs::Vfs;
use mc2_platform::movie::VideoFrame;
use mc2_ui::logistics::movie::MovieRequest;

/// The movie file stem of a pilot's message: `ControlGui::playPilotVideo` copies seven letters of the name, then the
/// movie code character. The file system is case-insensitive about it.
pub fn video_name(pilot_name: &str, movie_code: u8) -> String {
    let mut name: String = pilot_name.chars().take(7).collect();
    name.push(char::from(movie_code));
    name.to_ascii_lowercase()
}

/// The video of the unit-bar slot that is playing, if any.
pub struct PilotVideo {
    control: MovieControl,
    /// The pilot (`pilot_slot` of the unit) the video belongs to.
    pilot: Option<u32>,
}

impl Default for PilotVideo {
    fn default() -> Self {
        Self::new()
    }
}

impl PilotVideo {
    pub fn new() -> PilotVideo {
        PilotVideo {
            control: MovieControl::new(None),
            pilot: None,
        }
    }

    /// The pilot whose video plays.
    pub fn pilot(&self) -> Option<u32> {
        self.pilot
    }

    /// `ForceGroupBar::isPlayingVideo`.
    pub fn is_playing(&self) -> bool {
        self.pilot.is_some() && self.control.playback() == Playback::Playing
    }

    /// The picture to draw.
    pub fn frame(&self) -> Option<&VideoFrame> {
        self.pilot
            .and(self.control.frame())
            .filter(|frame| frame.time > 0.0)
    }

    /// Starts the video of `pilot` (`setPilotVideo(name, pilot)`). Returns `false` when it does not play: one is
    /// already playing, `enabled` (the option) is off, or the movie is not in the data.
    pub fn start(
        &mut self,
        vfs: &mut Vfs,
        pilot: u32,
        pilot_name: &str,
        movie_code: u8,
        enabled: bool,
    ) -> bool {
        if self.is_playing() || !enabled {
            return false;
        }
        let name = video_name(pilot_name, movie_code);
        match self.control.apply(vfs, &MovieRequest::Open(name.clone())) {
            Ok(()) => {
                self.pilot = Some(pilot);
                true
            }
            Err(e) => {
                log::warn!("pilot video {name}: {e}");
                self.pilot = None;
                false
            }
        }
    }

    /// `setPilotVideo(NULL, NULL)`.
    pub fn end(&mut self) {
        let _ = self.control.apply_close();
        self.pilot = None;
    }

    /// Moves the video on; a video that has played to its end closes (`bMovie->update()` false).
    pub fn advance(&mut self, vfs: &mut Vfs, dt: f32) {
        if self.pilot.is_none() {
            return;
        }
        if let Err(e) = self.control.advance(vfs, dt) {
            log::warn!("pilot video: {e}");
        }
        if self.control.playback() != Playback::Playing {
            self.end();
        }
    }
}
