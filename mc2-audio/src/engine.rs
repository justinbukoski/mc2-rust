// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The API the application and sim drive: [`GameAudio`] and [`AudioEvent`].
//!
//! `GameAudio` owns the whole audio side of a game: the sound table and banks, the radio message table,
//! the pilots' radios and the [`GameSoundSystem`] on a shared mixer. The app feeds it
//!
//! - [`AudioEvent`]s translated from `SimEvent`s (play a sound id at a world position, a radio message for a
//!   pilot, a Betty line, a music track...), via [`GameAudio::handle`], with an RNG from the caller for the
//!   radio's style rolls;
//! - the camera, as a [`Listener`], every frame;
//! - the mission clock ([`RadioClock`]) and the active/inactive state of pilots;
//! - [`GameAudio::update`] once per frame with the frame length.
//!
//! The mixer is rendered by whoever owns the output device (`mc2_platform::audio::AudioOutput` pulls from
//! the [`SharedMixer`]), or, headless, by [`GameAudio::render_offline`].

use crate::attenuation::{Listener, Vec3};
use crate::bank::SoundBank;
use crate::data::AudioData;
use crate::error::{AudioError, Result};
use crate::game::{GameSoundSystem, RadioEvent};
use crate::radio::{PilotId, PilotRadio, RadioClock, RadioMessageTable, RadioRequest, VoiceSet};
use crate::rng::AudioRng;
use crate::snd::SoundTable;
use crate::system::{SoundSystem, Volumes};
use crate::voicepack::VoicePack;
use mc2_platform::audio::SharedMixer;
use std::collections::HashMap;
use std::sync::Arc;

/// A request from the game to the audio side.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioEvent {
    /// `playDigitalSample(id, position, allowDupes)`. `at: None` plays centred at full volume.
    Sound {
        id: u32,
        at: Option<Vec3>,
        allow_dupes: bool,
    },
    /// A pilot radio message (`MechWarrior::radioMessage`); `message` is a `RadioMessageType` index.
    Radio {
        pilot: PilotId,
        message: i64,
        under_home_command: bool,
    },
    /// `playBettySample`.
    Betty(u32),
    StopBetty,
    /// `playSupportSample(id)`.
    Support(u32),
    /// `playSupportSample(-1, file)`: an in-mission voice-over file such as `"tut_4a"`.
    VoiceOver(String),
    StopSupport,
    /// `playDigitalMusic(id)`.
    Music(i64),
    StopMusic,
    /// `setIsRaining(sound id)`; 0 = none.
    Rain(u32),
    /// The `GeneralAlarm` flag.
    GeneralAlarm(bool),
    /// `SoundSystem::largestSensorContact`; negative = no contact.
    SensorContact(i64),
    /// `setIsPaused` / `clearIsPaused`.
    Pause(bool),
    /// `purgeSoundSystem` (end of mission).
    Purge,
    /// `setradio` (`execSetRadio`, `code/ablmc2.cpp:2672-2701`): `Radio::turnOn` / `turnOff` on the pilot's radio. A pilot without a
    /// radio ignores it. Appended: lane audio.
    RadioPower {
        pilot: PilotId,
        on: bool,
    },
}

pub struct GameAudio {
    pub sound: GameSoundSystem,
    radio_table: Arc<RadioMessageTable>,
    voices: HashMap<String, Option<Arc<VoiceSet>>>,
    voice_pack: Option<Arc<VoicePack>>,
    pilots: HashMap<PilotId, PilotRadio>,
    pilot_active: HashMap<PilotId, bool>,
    clock: RadioClock,
    /// Fractional output frames carried between offline slices.
    frame_carry: f64,
}

impl GameAudio {
    /// Load `data/sound/sound.snd`, the banks and `radio.csv`, and build the system on `mixer`.
    pub fn load(
        mut data: Box<dyn AudioData>,
        mixer: SharedMixer,
        volumes: Volumes,
    ) -> Result<Self> {
        let snd_path = data.paths().sound("sound", ".snd");
        let snd = data.read(&snd_path).ok_or(AudioError::Missing(snd_path))?;
        let table = Arc::new(SoundTable::from_bytes(snd)?);
        let bank = Arc::new(SoundBank::load(data.as_mut(), "sound")?);
        let radio_table = Arc::new(RadioMessageTable::load(data.as_mut())?);
        let sys = SoundSystem::new(mixer, table, bank, data, volumes);
        Ok(Self {
            sound: GameSoundSystem::new(sys),
            radio_table,
            voices: HashMap::new(),
            voice_pack: None,
            pilots: HashMap::new(),
            pilot_active: HashMap::new(),
            clock: RadioClock::default(),
            frame_carry: 0.0,
        })
    }

    pub fn mixer(&self) -> SharedMixer {
        self.sound.sys.mixer().clone()
    }

    pub fn radio_table(&self) -> &Arc<RadioMessageTable> {
        &self.radio_table
    }

    /// Use `pack`'s clips in place of the original pilot voice packets (`None`: the originals only, the default).
    /// Applies to voice sets opened from now on, so call it before [`add_pilot`](Self::add_pilot); pilots that
    /// already have a radio keep what they have.
    pub fn set_voice_pack(&mut self, pack: Option<VoicePack>) {
        self.voice_pack = pack.map(Arc::new);
        self.voices.clear();
    }

    /// Give a pilot a radio with the voice set `pilotAudio` (e.g. `"pilota"`). Returns false when the voice
    /// pak does not exist: the pilot then has no radio, as when the C++ `Radio::init` fails. Voice sets
    /// are shared between pilots.
    pub fn add_pilot(&mut self, pilot: PilotId, voice_name: &str) -> bool {
        let key = voice_name.to_ascii_lowercase();
        if !self.voices.contains_key(&key) {
            let voice =
                VoiceSet::open_with_pack(self.sound.sys.data_mut(), &key, self.voice_pack.clone())
                    .ok()
                    .map(Arc::new);
            self.voices.insert(key.clone(), voice);
        }
        match self.voices.get(&key).and_then(|v| v.clone()) {
            Some(voice) => {
                self.pilots.insert(
                    pilot,
                    PilotRadio::new(pilot, voice, Arc::clone(&self.radio_table)),
                );
                self.pilot_active.insert(pilot, true);
                true
            }
            None => false,
        }
    }

    /// Take a pilot's radio away. The pilot counts as inactive from now on, so anything of his still queued
    /// is dropped when it reaches the head of the queue (death and ejection calls excepted).
    pub fn remove_pilot(&mut self, pilot: PilotId) {
        self.pilots.remove(&pilot);
        self.pilot_active.insert(pilot, false);
    }

    pub fn pilot_radio(&self, pilot: PilotId) -> Option<&PilotRadio> {
        self.pilots.get(&pilot)
    }

    pub fn pilot_radio_mut(&mut self, pilot: PilotId) -> Option<&mut PilotRadio> {
        self.pilots.get_mut(&pilot)
    }

    /// `MechWarrior::active()`: inactive pilots are silent and their queued chatter is dropped.
    pub fn set_pilot_active(&mut self, pilot: PilotId, active: bool) {
        self.pilot_active.insert(pilot, active);
    }

    pub fn pilot_is_active(&self, pilot: PilotId) -> bool {
        self.pilot_active.get(&pilot).copied().unwrap_or(true)
    }

    /// Mission turn and scenario time, from the sim.
    pub fn set_clock(&mut self, clock: RadioClock) {
        self.clock = clock;
    }

    /// The camera (`eye`): used for positional sounds from now on.
    pub fn set_listener(&mut self, listener: Option<Listener>) {
        self.sound.sys.set_listener(listener);
    }

    pub fn set_volumes(&mut self, v: Volumes) {
        let s = &mut self.sound.sys;
        s.set_digital_master_volume(v.digital_master);
        s.set_sfx_volume(v.sfx);
        s.set_radio_volume(v.radio);
        s.set_betty_volume(v.betty);
        s.set_music_volume(v.music);
    }

    /// Callbacks for pilots and GUI (speaking flags, pilot videos) since the last call.
    pub fn take_radio_events(&mut self) -> Vec<RadioEvent> {
        self.sound.take_radio_events()
    }

    /// Apply one request. `rng` supplies the radio style rolls (`RandomNumber(100)`): pass the sim's
    /// visual stream.
    pub fn handle(&mut self, event: &AudioEvent, rng: &mut dyn AudioRng) {
        match event {
            AudioEvent::Sound {
                id,
                at,
                allow_dupes,
            } => {
                self.sound.sys.play_digital_sample(*id, *at, *allow_dupes);
            }
            AudioEvent::Radio {
                pilot,
                message,
                under_home_command,
            } => {
                let req = RadioRequest {
                    message: *message,
                    pilot_active: self.pilot_is_active(*pilot),
                    under_home_command: *under_home_command,
                };
                if let Some(radio) = self.pilots.get_mut(pilot) {
                    radio.radio_message(&mut self.sound, req, self.clock, rng);
                }
            }
            AudioEvent::Betty(id) => {
                self.sound.sys.play_betty_sample(*id);
            }
            AudioEvent::StopBetty => self.sound.sys.stop_betty_sample(),
            AudioEvent::Support(id) => {
                self.sound.sys.play_support_sample(*id, None);
            }
            AudioEvent::VoiceOver(name) => {
                self.sound.sys.play_support_sample(u32::MAX, Some(name));
            }
            AudioEvent::StopSupport => self.sound.sys.stop_support_sample(),
            AudioEvent::Music(id) => {
                let _ = self.sound.sys.play_digital_music(*id);
            }
            AudioEvent::StopMusic => self.sound.sys.stop_digital_music(),
            AudioEvent::Rain(id) => self.sound.sys.set_is_raining(*id),
            AudioEvent::GeneralAlarm(on) => self.sound.general_alarm = *on,
            AudioEvent::SensorContact(c) => self.sound.sys.largest_sensor_contact = *c,
            AudioEvent::Pause(p) => {
                if *p {
                    self.sound.sys.set_is_paused()
                } else {
                    self.sound.sys.clear_is_paused()
                }
            }
            AudioEvent::Purge => {
                self.sound.purge_sound_system();
                self.pilots.clear();
                self.pilot_active.clear();
            }
            AudioEvent::RadioPower { pilot, on } => {
                if let Some(radio) = self.pilots.get_mut(pilot) {
                    if *on {
                        radio.turn_on();
                    } else {
                        radio.turn_off();
                    }
                }
            }
        }
    }

    /// Per-frame update (`GameSoundSystem::update`).
    pub fn update(&mut self, frame_length: f32) {
        let active = &self.pilot_active;
        self.sound
            .update_with(frame_length, &|p| active.get(&p).copied().unwrap_or(true));
    }

    /// Headless driver: run [`update`](Self::update) for `frame_length` seconds, then render the same span
    /// of audio from the mixer and append it to `out` as interleaved stereo `f32`. Fractional frames carry
    /// over so a long run stays exactly on time. Never touches an audio device.
    pub fn render_offline(&mut self, frame_length: f32, out: &mut Vec<f32>) {
        self.update(frame_length);
        self.render_frame(frame_length, out);
    }

    /// Mixes the audio of one frame of `frame_length` seconds, without [`update`](Self::update) (the
    /// caller already ran it, as the app's frame does). Fractional frames carry over as in
    /// [`render_offline`](Self::render_offline).
    pub fn render_frame(&mut self, frame_length: f32, out: &mut Vec<f32>) {
        let rate = self.mixer().lock().sample_rate();
        let exact = f64::from(frame_length) * f64::from(rate) + self.frame_carry;
        let frames = exact.floor() as usize;
        self.frame_carry = exact - frames as f64;
        let start = out.len();
        out.resize(start + frames * 2, 0.0);
        self.mixer().lock().render(&mut out[start..]);
    }
}
