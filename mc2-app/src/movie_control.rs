// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The movie host's playback for the logistics pages' movie requests ([`MovieRequest`]).
//!
//! The pages state what the C++ does to its movie (`MissionSelectionScreen::handleMessage`,
//! `Logistics::playFullScreenVideo`). This module carries those actions out on the same
//! [`MoviePlayer`] the production host plays its own movies with, as a value the host can drive:
//!
//! - [`MovieControl::apply`] takes a request (open, play, pause, stop, skip, close, final video);
//! - [`MovieControl::advance`] moves the picture and sound on by the frame time, unless held;
//! - [`MovieControl::frame`] is the picture to draw (none while stopped, the held picture while
//!   paused, as `MC2Movie::render` draws nothing once stopped);
//! - [`MovieControl::take_end`] says when the final video is over, so the game goes to the main
//!   menu (`missionBegin->beginSplash()`, `code/logistics.cpp:144-157`).
//!
//! Play starts the movie again from its first picture, as `MC2Movie::restart` does. The movie is
//! opened again from the file system for it, because a [`MoviePlayer`] cannot be rewound, and its
//! sound gets a fresh source in the mixer. A hold pauses that sound channel with the picture.
//!
//! A final video is followed by the credits when it is `cinema5` (`Logistics::update`,
//! `code/logistics.cpp:283-298`); the end of the credits, or of any other final video, goes to
//! the main menu. A skip or a stop ends a movie the same way its natural end does, as the C++
//! `MC2Movie::stop` makes its next `update` report the end.
//!
//! The music the C++ starts and stops around these movies comes out as [`Cue`]s, which the host
//! takes with [`MovieControl::take_cues`] and plays through the audio like any other cue:
//!
//! - a final video starts: [`Cue::StopMusic`] (`Logistics::playFullScreenVideo`, `code/logistics.cpp:897`);
//! - a `cinema5` hands over to the credits: `Cue::Music(33)` (`code/logistics.cpp:303`), whether it
//!   ended or was skipped;
//! - the credits end: no cue. The C++ then asks for the logistics tune, which is -1 once the campaign
//!   is over (`LogisticsMissionInfo::getCurrentLogisticsTuneId`), and a negative id plays nothing.
//!
//! A screen's movie (mission select) makes no music cue, as its C++ screen makes none.

use crate::audio::{tune, Cue};
use mc2_formats::vfs::Vfs;
use mc2_platform::audio::mixer::QueueWriter;
use mc2_platform::audio::{PlayMode, QueueSource, SharedMixer, MOVIE_AUDIO_CHANNEL};
use mc2_platform::movie::{Movie, MoviePlayer, VideoFrame};
use mc2_ui::logistics::movie::{movie_vfs_path, MovieRequest};

/// The movie that follows `cinema5` in a final video (`"credits"`, `code/logistics.cpp:293`).
const CREDITS: &str = "credits";
/// Where the movie stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Playback {
    /// No movie is open.
    Idle,
    Playing,
    /// Held on its current picture (`MC2Movie::pause`), its sound held with it.
    Paused,
    /// Stopped by a request, skipped, or finished: nothing is shown until the next `Play`.
    Stopped,
}

/// What the game does once a final video is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovieEnd {
    /// The splash, which is the main menu (`beginSplash`, `code/logistics.cpp:144-157`).
    MainMenu,
}

/// The playback of the logistics movies. Owns at most one movie at a time.
pub struct MovieControl {
    /// The app mixer the movie's sound goes to; `None` when the app has no audio.
    mixer: Option<SharedMixer>,
    /// The movie the last open named, which `Play` opens again.
    name: Option<String>,
    /// Whether the movie is the campaign's final video, or the credits that follow a `cinema5`
    /// final video. Its end is [`MovieEnd::MainMenu`] once no credits follow.
    is_final: bool,
    playback: Playback,
    /// Mission videos skip only their unusable first decoded picture.
    hide_first_picture: bool,
    player: Option<MoviePlayer>,
    frame: Option<VideoFrame>,
    end: Option<MovieEnd>,
    /// The music cues the movies have asked for since the host last took them.
    cues: Vec<Cue>,
}

impl MovieControl {
    pub fn new(mixer: Option<SharedMixer>) -> MovieControl {
        MovieControl {
            mixer,
            name: None,
            is_final: false,
            playback: Playback::Idle,
            hide_first_picture: false,
            player: None,
            frame: None,
            end: None,
            cues: Vec::new(),
        }
    }

    /// The music cues since the last call, in the order the C++ makes them. The host plays them
    /// through its audio (see the module docs for which movie makes which cue).
    pub fn take_cues(&mut self) -> Vec<Cue> {
        std::mem::take(&mut self.cues)
    }

    /// Where the movie stands.
    pub fn playback(&self) -> Playback {
        self.playback
    }

    /// The name of the movie that is open (the credits, once a `cinema5` final video hands over).
    pub fn movie_name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The picture to draw: the newest one shown, held while paused, and none once stopped.
    pub fn frame(&self) -> Option<&VideoFrame> {
        match self.playback {
            Playback::Playing | Playback::Paused => self
                .frame
                .as_ref()
                .filter(|frame| !self.hide_first_picture || frame.time > 0.0),
            Playback::Idle | Playback::Stopped => None,
        }
    }

    /// Hide the first decoded picture while playback and audio continue in sync.
    pub fn hide_first_picture(&mut self) {
        self.hide_first_picture = true;
    }

    /// The playback clock in seconds from the first picture, while a movie is open.
    pub fn clock(&self) -> Option<f64> {
        self.player.as_ref().map(MoviePlayer::clock)
    }

    /// The end of a final video, once: the game goes to the main menu.
    pub fn take_end(&mut self) -> Option<MovieEnd> {
        self.end.take()
    }

    /// Carries out one movie request. A movie that cannot be opened is reported as an error;
    /// a final video that cannot be opened also ends in the main menu (C++ goes straight to the
    /// splash when the video is not there, since the splash does not wait for it). `Play` with no
    /// movie open is an error: nothing was there to play.
    pub fn apply(&mut self, vfs: &mut Vfs, request: &MovieRequest) -> Result<(), String> {
        match request {
            MovieRequest::Open(name) => self.start(vfs, name, false),
            MovieRequest::FinalVideo(name) => {
                // `playFullScreenVideo` stops the music as the video starts (`code/logistics.cpp:897`)
                self.cues.push(tune::full_screen_video());
                self.start(vfs, name, true)
            }
            MovieRequest::Play => match self.name.clone() {
                Some(name) => {
                    let is_final = self.is_final;
                    self.start(vfs, &name, is_final)
                }
                None => Err("no movie is open to play".to_string()),
            },
            MovieRequest::Pause(hold) => {
                self.hold(*hold);
                Ok(())
            }
            // `MC2Movie::stop`, and the logistics skip (space, escape, left mouse) that ends a
            // movie the same way: nothing is shown after it, and a final video goes on.
            MovieRequest::Stop | MovieRequest::Skip => self.finish(vfs),
            MovieRequest::Close => {
                self.release();
                self.name = None;
                self.is_final = false;
                self.playback = Playback::Idle;
                Ok(())
            }
        }
    }

    /// Closes the movie and forgets its name (`MovieRequest::Close`).
    pub fn apply_close(&mut self) -> Result<(), String> {
        self.release();
        self.name = None;
        self.is_final = false;
        self.playback = Playback::Idle;
        Ok(())
    }

    /// Moves a playing movie on by `dt` seconds. A held or stopped movie does not move. A movie
    /// that ends is finished (see [`MovieControl::apply`]): a `cinema5` final video starts the
    /// credits, and any other movie that ends is stopped, a final video going to the menu. A movie
    /// that fails to decode is stopped.
    pub fn advance(&mut self, vfs: &mut Vfs, dt: f32) -> Result<(), String> {
        if self.playback != Playback::Playing {
            return Ok(());
        }
        let Some(player) = self.player.as_mut() else {
            return Ok(());
        };
        let frame = player.advance(f64::from(dt.max(0.0)));
        let error = player.take_error();
        let finished = player.is_finished();
        if let Some(frame) = frame {
            self.frame = Some(frame);
        }
        if let Some(error) = error {
            self.end_movie();
            return Err(format!("movie playback failed: {error}"));
        }
        if finished {
            return self.finish(vfs);
        }
        Ok(())
    }

    /// Opens `name` and starts it from its first picture, playing (`MC2Movie::init`, `restart`).
    fn start(&mut self, vfs: &mut Vfs, name: &str, is_final: bool) -> Result<(), String> {
        self.release();
        self.name = Some(name.to_string());
        self.is_final = is_final;
        let movie = match Movie::open_vfs(vfs, &movie_vfs_path(name)) {
            Ok(movie) => movie,
            Err(error) => {
                self.name = None;
                self.is_final = false;
                self.playback = Playback::Idle;
                if is_final {
                    self.end = Some(MovieEnd::MainMenu);
                }
                return Err(format!("movie unavailable ({name}): {error}"));
            }
        };
        let audio = self.open_audio(&movie);
        self.player = Some(MoviePlayer::new(movie, audio));
        self.playback = Playback::Playing;
        // a first picture that fails to decode is the same failure as a later one
        let error = self.player.as_mut().and_then(MoviePlayer::take_error);
        if let Some(error) = error {
            self.end_movie();
            return Err(format!("movie playback failed: {error}"));
        }
        Ok(())
    }

    /// Holds or resumes the picture and its sound. A movie that is not playing keeps its state.
    fn hold(&mut self, hold: bool) {
        let next = match (self.playback, hold) {
            (Playback::Playing, true) => Playback::Paused,
            (Playback::Paused, false) => Playback::Playing,
            (state, _) => state,
        };
        if next == self.playback {
            return;
        }
        self.playback = next;
        if let Some(mixer) = &self.mixer {
            let mode = if next == Playback::Paused {
                PlayMode::Pause
            } else {
                PlayMode::Continue
            };
            mixer.lock().set_mode(MOVIE_AUDIO_CHANNEL, mode);
        }
    }

    /// The movie is over, stopped, skipped or finished. A `cinema5` final video is followed by the
    /// credits (`code/logistics.cpp:283-298`), which play in its place; any other movie is over,
    /// and the end of a final video is handed to the menu.
    fn finish(&mut self, vfs: &mut Vfs) -> Result<(), String> {
        let credits_follow = self.is_final && self.name.as_deref().is_some_and(is_cinema5);
        if credits_follow {
            self.release();
            // the credits' tune, once the cinema5 is gone (`code/logistics.cpp:303`); the C++ makes
            // the call whether or not the credits open
            self.cues.push(Cue::Music(tune::CREDITS_TUNE_DMS));
            return self.start(vfs, CREDITS, true);
        }
        self.end_movie();
        Ok(())
    }

    /// Stops the movie for good: nothing is shown until the next `Play`, and a final video is
    /// handed to the menu. No credits follow from here (see [`MovieControl::finish`]).
    fn end_movie(&mut self) {
        let was_final = self.is_final;
        self.release();
        self.playback = Playback::Stopped;
        if was_final {
            self.end = Some(MovieEnd::MainMenu);
        }
    }

    /// Drops the player and its picture, and frees the movie's sound channel.
    fn release(&mut self) {
        self.player = None;
        self.frame = None;
        if let Some(mixer) = &self.mixer {
            mixer.lock().release(MOVIE_AUDIO_CHANNEL);
        }
    }

    /// A fresh sound source for the movie on the movie channel, when it has sound and the app has
    /// a mixer. The writer receives the decoded samples.
    fn open_audio(&self, movie: &Movie) -> Option<QueueWriter> {
        let info = movie.info();
        if !info.has_audio {
            return None;
        }
        let mixer = self.mixer.as_ref()?;
        let (source, queue) = QueueSource::new(2, info.audio_sample_rate);
        let mut mixer = mixer.lock();
        mixer.assign(MOVIE_AUDIO_CHANNEL, Box::new(source));
        mixer.set_mode(MOVIE_AUDIO_CHANNEL, PlayMode::PlayOnce);
        Some(queue)
    }
}

/// Whether a movie is `cinema5`: the C++ compares the first seven characters of its name without
/// regard to case (`S_strnicmp(name, "cinema5", 7)`).
fn is_cinema5(name: &str) -> bool {
    name.as_bytes()
        .get(..7)
        .is_some_and(|head| head.eq_ignore_ascii_case(b"cinema5"))
}
