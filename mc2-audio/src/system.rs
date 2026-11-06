// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The sound system: effects, Betty, support officer, voice-overs, rain and music.
//!
//! Port of `mclib/soundsys.cpp` / `soundsys.h` (`SoundSystem`). The sample channel map is the C++ one and
//! maps one-to-one onto [`mc2_platform::audio::Mixer`] channels:
//!
//! | channel | use |
//! |---|---|
//! | 0 | pilot radio (`RADIO_CHANNEL`) |
//! | 1..=12 | effects, handed out by [`SoundSystem::play_digital_sample`] (`findOpenChannel(1, 13)`) |
//! | 13 | support officer samples (`SUPPORT_CHANNEL`) |
//! | 14 | Betty (`BETTY_CHANNEL`) |
//! | 15 | rain/fire loop (`FIRE_CHANNEL`) |
//! | 16, 17 | the two music streams (`STREAM1CHANNEL`, `STREAM2CHANNEL`) |
//! | 18 | in-mission voice-overs (`STREAM3CHANNEL`) |
//!
//! Volume groups come from the options: master, music, effects, radio and Betty, each a byte divided by 256
//! (`setSFXVolume(byte)` ...). The voice-over stream uses the Betty volume. Nothing here touches an audio
//! device: the mixer is rendered by the caller, so the whole system runs in tests.
//!
//! Intentional differences from the C++ (all in places where the original would crash or read garbage):
//! samples whose packet is empty or not a WAV are skipped instead of dereferencing a null format; the
//! `Result` of each play call is an `Option<usize>` channel instead of `-1`.

use crate::attenuation::{Listener, Spatial, Vec3};
use crate::bank::{Packet, SoundBank};
use crate::data::AudioData;
use crate::ids::sfx;
use crate::snd::SoundTable;
use mc2_platform::audio::{PlayMode, SharedMixer};
use std::sync::Arc;

/// `MAX_DIGITAL_SAMPLES`.
pub const MAX_DIGITAL_SAMPLES: usize = 16;
/// `MAX_SAMPLE_VOLUME`.
pub const MAX_SAMPLE_VOLUME: u32 = 127;
pub const RADIO_CHANNEL: usize = 0;
pub const FIRE_CHANNEL: usize = MAX_DIGITAL_SAMPLES - 1;
pub const BETTY_CHANNEL: usize = FIRE_CHANNEL - 1;
pub const SUPPORT_CHANNEL: usize = BETTY_CHANNEL - 1;
pub const AMBIENT_CHANNEL_START: usize = 11;
pub const STREAM1_CHANNEL: usize = 16;
pub const STREAM2_CHANNEL: usize = 17;
pub const STREAM3_CHANNEL: usize = 18;
/// `MAX_QUEUED_MESSAGES`.
pub const MAX_QUEUED_MESSAGES: usize = 8;
/// `MAX_SENSOR_UPDATE_TIME`: seconds between sensor-contact beeps.
pub const MAX_SENSOR_UPDATE_TIME: f32 = 5.0;

/// Why a play request was turned down (the C++ returns -1 for all of these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The tune is already the current one.
    AlreadyCurrent,
    /// Both music streams are in use.
    StreamsBusy,
    /// A voice-over is already running.
    VoiceOverBusy,
    /// The audio file does not exist or cannot be opened.
    FileMissing,
    /// The radio queue has no room for a message this unimportant.
    QueueFull,
}

/// The five volume groups of the options screen, as bytes (0..=255, divided by 256 when applied).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volumes {
    pub digital_master: u8,
    pub music: u8,
    pub sfx: u8,
    pub radio: u8,
    pub betty: u8,
}

impl Default for Volumes {
    /// The option screen's defaults (`optionsarea.cpp`): master 255, the others 64.
    fn default() -> Self {
        Self {
            digital_master: 255,
            music: 64,
            sfx: 64,
            radio: 64,
            betty: 64,
        }
    }
}

fn byte_to_gain(v: u8) -> f32 {
    f32::from(v) / 256.0
}

/// One of the two cross-fading music streams (`streamNHandle`, `streamNActive`, `streamNTime`).
#[derive(Clone, Copy, Debug, Default)]
struct MusicStream {
    /// `streamNHandle != 0`: a resource is assigned to the channel.
    handle: bool,
    /// Playback has been started (done in `update`, never in the same call that creates the stream).
    active: bool,
    /// > 0 fading up, < 0 fading down, 0 steady. Seconds remaining.
    time: f32,
    /// Music id (`streamNId`).
    id: i32,
}

/// Public view of a music stream, for tests and the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicStreamState {
    pub has_resource: bool,
    pub active: bool,
    pub fade_time: f32,
    pub music_id: i32,
}

pub struct SoundSystem {
    pub(crate) mixer: SharedMixer,
    table: Arc<SoundTable>,
    bank: Arc<SoundBank>,
    data: Box<dyn AudioData>,

    /// `useSound` / `useMusic` (`[UseSound]` / `[UseMusic]` in `system.cfg`).
    pub use_sound: bool,
    pub use_music: bool,

    channel_in_use: [bool; MAX_DIGITAL_SAMPLES],
    channel_sample_id: [i64; MAX_DIGITAL_SAMPLES],
    fade_down: [bool; MAX_DIGITAL_SAMPLES],

    digital_master_volume: f32,
    sfx_volume: f32,
    music_volume: f32,
    radio_volume: f32,
    betty_volume: f32,
    volumes: Volumes,

    streams: [MusicStream; 2],
    current_music_id: i64,
    stream3_handle: bool,
    stream3_active: bool,

    /// `bettySoundBite != NULL`: a Betty sample is loaded or playing.
    betty_loaded: bool,
    last_betty_id: i64,
    support_loaded: bool,
    last_support_id: i64,

    is_raining: u32,
    old_raining: u32,

    sensor_beep_update_time: f32,
    /// `SoundSystem::largestSensorContact`: >= 0 while a sensor contact is held, which beeps every 5 s.
    pub largest_sensor_contact: i64,
    game_paused: bool,

    listener: Option<Listener>,
}

impl SoundSystem {
    /// `SoundSystem::init(const char *)` after the data is loaded: all state is reset, the volume groups
    /// are applied, and the mixer channels are left as they are.
    pub fn new(
        mixer: SharedMixer,
        table: Arc<SoundTable>,
        bank: Arc<SoundBank>,
        data: Box<dyn AudioData>,
        volumes: Volumes,
    ) -> Self {
        Self {
            mixer,
            table,
            bank,
            data,
            use_sound: true,
            use_music: true,
            channel_in_use: [false; MAX_DIGITAL_SAMPLES],
            channel_sample_id: [-1; MAX_DIGITAL_SAMPLES],
            fade_down: [false; MAX_DIGITAL_SAMPLES],
            digital_master_volume: byte_to_gain(volumes.digital_master),
            sfx_volume: byte_to_gain(volumes.sfx),
            music_volume: byte_to_gain(volumes.music),
            radio_volume: byte_to_gain(volumes.radio),
            betty_volume: byte_to_gain(volumes.betty),
            volumes,
            streams: [MusicStream::default(); 2],
            current_music_id: -1,
            stream3_handle: false,
            stream3_active: false,
            betty_loaded: false,
            last_betty_id: -1,
            support_loaded: false,
            last_support_id: -1,
            is_raining: 0,
            old_raining: 0,
            sensor_beep_update_time: 0.0,
            largest_sensor_contact: -1,
            game_paused: false,
            listener: None,
        }
    }

    pub fn mixer(&self) -> &SharedMixer {
        &self.mixer
    }

    pub fn table(&self) -> &Arc<SoundTable> {
        &self.table
    }

    pub fn bank(&self) -> &Arc<SoundBank> {
        &self.bank
    }

    /// The camera used for positional sounds (`eye`). With no listener, positions are ignored and every
    /// sound plays centred at full distance volume, as when `eye` is null.
    pub fn set_listener(&mut self, listener: Option<Listener>) {
        self.listener = listener;
    }

    pub fn listener(&self) -> Option<Listener> {
        self.listener
    }

    // ---------------------------------------------------------------- volumes

    pub fn digital_master_gain(&self) -> f32 {
        self.digital_master_volume
    }
    pub fn sfx_gain(&self) -> f32 {
        self.sfx_volume
    }
    pub fn music_gain(&self) -> f32 {
        self.music_volume
    }
    pub fn radio_gain(&self) -> f32 {
        self.radio_volume
    }
    pub fn betty_gain(&self) -> f32 {
        self.betty_volume
    }
    pub fn volumes(&self) -> Volumes {
        self.volumes
    }

    pub fn set_digital_master_volume(&mut self, volume: u8) {
        if self.use_sound {
            self.volumes.digital_master = volume;
            self.digital_master_volume = byte_to_gain(volume);
        }
    }

    pub fn digital_master_volume(&self) -> u8 {
        if self.use_sound {
            self.volumes.digital_master
        } else {
            0
        }
    }

    pub fn set_sfx_volume(&mut self, volume: u8) {
        if self.use_sound {
            self.volumes.sfx = volume;
            self.sfx_volume = byte_to_gain(volume);
        }
    }

    pub fn sfx_volume(&self) -> u8 {
        if self.use_sound {
            self.volumes.sfx
        } else {
            0
        }
    }

    pub fn set_radio_volume(&mut self, volume: u8) {
        if self.use_sound {
            self.volumes.radio = volume;
            self.radio_volume = byte_to_gain(volume);
        }
    }

    pub fn radio_volume(&self) -> u8 {
        if self.use_sound {
            self.volumes.radio
        } else {
            0
        }
    }

    /// `setMusicVolume`: running streams are set to `master * music` (without the per-track volume or
    /// the fade, exactly as the C++ does).
    pub fn set_music_volume(&mut self, volume: u8) {
        if self.use_sound {
            self.volumes.music = volume;
            self.music_volume = byte_to_gain(volume);
            if self.use_music {
                let v = self.digital_master_volume * self.music_volume;
                let mut m = self.mixer.lock();
                for (i, ch) in [STREAM1_CHANNEL, STREAM2_CHANNEL].into_iter().enumerate() {
                    if self.streams[i].active && self.streams[i].handle {
                        m.set_volume(ch, v);
                    }
                }
            }
        }
    }

    pub fn music_volume(&self) -> u8 {
        if self.use_sound {
            self.volumes.music
        } else {
            0
        }
    }

    pub fn set_betty_volume(&mut self, volume: u8) {
        if self.use_sound {
            self.volumes.betty = volume;
            self.betty_volume = byte_to_gain(volume);
        }
    }

    pub fn betty_volume(&self) -> u8 {
        if self.use_sound {
            self.volumes.betty
        } else {
            0
        }
    }

    // ---------------------------------------------------------------- pause, rain

    pub fn set_is_paused(&mut self) {
        self.game_paused = true;
    }

    pub fn clear_is_paused(&mut self) {
        self.game_paused = false;
    }

    /// `setIsRaining(DWORD)`: the sound bite that loops on the fire channel (0 = none).
    pub fn set_is_raining(&mut self, sound_id: u32) {
        self.is_raining = sound_id;
    }

    // ---------------------------------------------------------------- channels

    /// `findOpenChannel(start, end)`: the first channel in `start..end` not marked in use.
    pub fn find_open_channel(&self, start: usize, end: usize) -> Option<usize> {
        let mut channel = start;
        while channel < end && self.channel_in_use.get(channel).copied().unwrap_or(false) {
            channel += 1;
        }
        (channel != end).then_some(channel)
    }

    /// `isPlayingSample`: some effect channel was started with this bite and has not finished.
    pub fn is_playing_sample(&self, sample_id: u32) -> bool {
        self.channel_sample_id.contains(&i64::from(sample_id))
    }

    /// `isChannelPlaying`: the channel is in play-once or loop mode.
    pub fn is_channel_playing(&self, channel: usize) -> bool {
        // The C++ range test is `channelId > MAX_DIGITAL_SAMPLES` (one past the end is accepted).
        if channel > MAX_DIGITAL_SAMPLES {
            return false;
        }
        matches!(
            self.mixer.lock().mode(channel),
            PlayMode::PlayOnce | PlayMode::Loop
        )
    }

    pub fn is_playing_voice_over(&self) -> bool {
        self.stream3_handle
    }

    pub fn is_digital_music_playing(&self) -> bool {
        self.streams[0].handle || self.streams[1].handle
    }

    pub fn current_music_id(&self) -> i64 {
        self.current_music_id
    }

    pub fn music_stream_state(&self, stream: usize) -> MusicStreamState {
        let s = self.streams[stream & 1];
        MusicStreamState {
            has_resource: s.handle,
            active: s.active,
            fade_time: s.time,
            music_id: s.id,
        }
    }

    /// The bite a channel is playing (`channelSampleId`), if any.
    pub fn channel_sample_id(&self, channel: usize) -> Option<u32> {
        let id = *self.channel_sample_id.get(channel)?;
        u32::try_from(id).ok()
    }

    pub fn last_betty_id(&self) -> i64 {
        self.last_betty_id
    }

    pub fn last_support_id(&self) -> i64 {
        self.last_support_id
    }

    // ---------------------------------------------------------------- effects

    /// `playDigitalSample(sampleId, pos, allowDupes)`.
    ///
    /// Without `allowDupes` a bite that is already playing is not started again. The result is the
    /// channel used, or `None` when nothing was started: unknown id, no free channel (1..=12), too far
    /// from the listener, or a volume of 0. A bite whose packet is empty still claims a channel but makes
    /// no sound, as in the C++.
    pub fn play_digital_sample(
        &mut self,
        sample_id: u32,
        pos: Option<Vec3>,
        allow_dupes: bool,
    ) -> Option<usize> {
        let allowed = if allow_dupes {
            self.use_sound
        } else {
            !self.is_playing_sample(sample_id)
        };
        if !allowed {
            return None;
        }
        let bite = self.table.bites.get(sample_id as usize)?;
        let channel = self.find_open_channel(1, SUPPORT_CHANNEL)?;

        let mut spatial = Spatial::CENTRE;
        if let (Some(eye), Some(p)) = (self.listener, pos) {
            if p.has_position() {
                // Too far away: do not play.
                spatial = eye.spatialize(p)?;
            }
        }

        let mut vol = bite.volume * spatial.distance_volume;
        if vol > 1.0 {
            vol = 1.0;
        } else if vol <= 0.0 {
            return None;
        }

        {
            let mut m = self.mixer.lock();
            m.set_pan(channel, spatial.pan);
            m.set_volume(channel, self.digital_master_volume * vol * self.sfx_volume);
        }
        self.channel_sample_id[channel] = i64::from(sample_id);
        self.channel_in_use[channel] = true;

        if let Some(sound) = self.bank.effect(sample_id as usize) {
            let mut m = self.mixer.lock();
            m.assign_sound(channel, sound);
            m.set_mode(channel, PlayMode::PlayOnce);
        }
        Some(channel)
    }

    /// `stopDigitalSample(channel)`: fade the channel out over the next updates.
    pub fn stop_digital_sample(&mut self, channel: usize) {
        if self.use_sound && channel < MAX_DIGITAL_SAMPLES {
            let mode = self.mixer.lock().mode(channel);
            if matches!(mode, PlayMode::PlayOnce | PlayMode::Loop) {
                self.fade_down[channel] = true;
            }
        }
    }

    /// `playABLSFX`.
    pub fn play_abl_sfx(&mut self, sfx_id: u32) {
        self.play_digital_sample(sfx_id, None, false);
    }

    // ---------------------------------------------------------------- Betty and support

    /// `playBettySample`: only one Betty line at a time (a request while one is loaded is dropped), the
    /// radio channel is ducked to half volume while she speaks. Returns [`BETTY_CHANNEL`] on success.
    pub fn play_betty_sample(&mut self, betty_sample_id: u32) -> Option<usize> {
        if !(self.use_sound && !self.betty_loaded) {
            return None;
        }
        if betty_sample_id as usize >= self.bank.betty.len() {
            return None;
        }
        {
            let mut m = self.mixer.lock();
            m.set_pan(BETTY_CHANNEL, 0.0);
            m.set_volume(
                BETTY_CHANNEL,
                self.digital_master_volume * self.betty_volume,
            );
            m.set_volume(
                RADIO_CHANNEL,
                self.digital_master_volume * self.radio_volume * 0.5,
            );
        }
        self.last_betty_id = i64::from(betty_sample_id);
        self.channel_in_use[BETTY_CHANNEL] = true;

        let sound = match self.bank.betty.packet(betty_sample_id as usize) {
            Packet::Sound(s) => Arc::clone(s),
            _ => return None,
        };
        self.betty_loaded = true;
        let mut m = self.mixer.lock();
        m.assign_sound(BETTY_CHANNEL, &sound);
        m.set_mode(BETTY_CHANNEL, PlayMode::PlayOnce);
        Some(BETTY_CHANNEL)
    }

    /// Whether a Betty line is loaded (`bettySoundBite != NULL`): a request now is refused, as `playBettySample` refuses it.
    pub fn is_betty_loaded(&self) -> bool {
        self.betty_loaded
    }

    /// `stopBettySample`.
    pub fn stop_betty_sample(&mut self) {
        if self.betty_loaded {
            let mut m = self.mixer.lock();
            m.set_mode(BETTY_CHANNEL, PlayMode::Stop);
            m.release(BETTY_CHANNEL);
            self.betty_loaded = false;
        }
    }

    /// `playSupportSample(id, fileName)`. With a file name the line is played as a voice-over stream and
    /// [`STREAM3_CHANNEL`] is returned whether or not the file was found (as in the C++).
    pub fn play_support_sample(
        &mut self,
        support_sample_id: u32,
        file_name: Option<&str>,
    ) -> Option<usize> {
        if !(self.use_sound && !self.support_loaded) {
            return None;
        }
        if let Some(name) = file_name {
            let _ = self.play_digital_stream(name);
            return Some(STREAM3_CHANNEL);
        }
        if support_sample_id as usize >= self.bank.support.len() {
            return None;
        }
        {
            let mut m = self.mixer.lock();
            m.set_pan(SUPPORT_CHANNEL, 0.0);
            m.set_volume(
                SUPPORT_CHANNEL,
                self.digital_master_volume * self.betty_volume,
            );
        }
        self.last_support_id = i64::from(support_sample_id);
        self.channel_in_use[SUPPORT_CHANNEL] = true;
        let sound = match self.bank.support.packet(support_sample_id as usize) {
            Packet::Sound(s) => Arc::clone(s),
            _ => return None,
        };
        self.support_loaded = true;
        let mut m = self.mixer.lock();
        m.assign_sound(SUPPORT_CHANNEL, &sound);
        m.set_mode(SUPPORT_CHANNEL, PlayMode::PlayOnce);
        Some(SUPPORT_CHANNEL)
    }

    /// `stopSupportSample`: stops the voice-over stream (the C++ does not touch channel 13).
    pub fn stop_support_sample(&mut self) {
        if self.stream3_handle {
            self.mixer.lock().release(STREAM3_CHANNEL);
            self.stream3_handle = false;
            self.stream3_active = false;
        }
    }

    // ---------------------------------------------------------------- voice-over stream

    /// `playDigitalStream(name)`: play `data/sound/<base name>.wav` on the voice-over channel. `Err` when
    /// a voice-over is already running or the file is missing.
    pub fn play_digital_stream(&mut self, stream_name: &str) -> Result<(), Refused> {
        if !self.use_sound {
            return Ok(());
        }
        let actual = split_path_stem(stream_name);
        let path = self.data.paths().sound(actual, ".wav");
        if !self.data.exists(&path) {
            return Err(Refused::FileMissing);
        }
        if self.stream3_active {
            return Err(Refused::VoiceOverBusy);
        }
        if self.stream3_handle {
            self.mixer.lock().release(STREAM3_CHANNEL);
            self.stream3_handle = false;
        }
        let Some(source) = self.data.open_stream(&path) else {
            return Err(Refused::FileMissing);
        };
        self.stream3_active = false;
        self.stream3_handle = true;
        let mut m = self.mixer.lock();
        m.assign(STREAM3_CHANNEL, source);
        m.set_volume(
            STREAM3_CHANNEL,
            self.digital_master_volume * self.betty_volume,
        );
        m.set_pan(STREAM3_CHANNEL, 0.0);
        Ok(())
    }

    // ---------------------------------------------------------------- music

    /// `playDigitalMusic(musicId)`. `Ok(())` when the tune was started (or music is off / the entry is
    /// `NONE` / the file is missing, which the C++ also reports as success); `Err(())` when the tune is
    /// already current, or both streams are busy.
    ///
    /// The first tune goes to stream 1 with a fade-in; a new tune while one plays starts on the idle
    /// stream while the old one fades out, the cross-fade. The stream starts playing in the next
    /// [`update`](Self::update).
    pub fn play_digital_music(&mut self, music_id: i64) -> Result<(), Refused> {
        if !self.use_music {
            return Ok(());
        }
        let Some(track) = usize::try_from(music_id)
            .ok()
            .and_then(|i| self.table.music.get(i))
        else {
            return Ok(());
        };
        if track.is_none() {
            return Ok(());
        }
        if music_id == self.current_music_id {
            return Err(Refused::AlreadyCurrent);
        }
        if !self.use_sound {
            return Ok(());
        }
        let path = self.data.paths().sound(&track.name, ".wav");
        if !self.data.exists(&path) {
            return Ok(());
        }
        let fade = self.table.stream_fade_down_time;

        // Which stream takes the new tune, and what happens to the other one.
        let (new, old): (usize, Option<usize>) =
            if !self.streams[0].active && !self.streams[1].active {
                (0, None)
            } else if !self.streams[1].active {
                (1, Some(0))
            } else if !self.streams[0].active {
                (0, Some(1))
            } else {
                return Err(Refused::StreamsBusy);
            };

        let Some(source) = self.data.open_stream(&path) else {
            return Ok(());
        };
        let new_channel = STREAM1_CHANNEL + new;
        if self.streams[new].handle {
            self.mixer.lock().release(new_channel);
            self.streams[new].handle = false;
        }
        if let Some(old) = old {
            // Check whether the old stream ever got all the way faded up.
            let t = self.streams[old].time;
            self.streams[old].time = if t > 0.0 { -t } else { -fade };
        }
        {
            let mut m = self.mixer.lock();
            m.assign(new_channel, source);
            m.set_volume(new_channel, 0.0);
            m.set_pan(new_channel, 0.0);
        }
        self.streams[new].handle = true;
        self.streams[new].time = fade;
        self.streams[new].id = music_id as i32;
        self.current_music_id = music_id;
        Ok(())
    }

    /// `useMusic && useSound`: [`play_digital_music`](Self::play_digital_music) can start a tune.
    pub fn music_enabled(&self) -> bool {
        self.use_music && self.use_sound
    }

    /// Both music streams are active, so a new tune is refused.
    pub fn music_streams_busy(&self) -> bool {
        self.streams[0].active && self.streams[1].active
    }

    /// How many tunes the music table lists.
    pub fn music_table_len(&self) -> usize {
        self.table.music.len()
    }

    /// The music table has a tune for `music_id` and its file exists.
    pub fn music_file_exists(&self, music_id: i64) -> bool {
        let Some(track) = usize::try_from(music_id)
            .ok()
            .and_then(|i| self.table.music.get(i))
        else {
            return false;
        };
        !track.is_none()
            && self
                .data
                .exists(&self.data.paths().sound(&track.name, ".wav"))
    }

    /// `useSound && no support sample loaded`: [`play_support_sample`](Self::play_support_sample) accepts a voice-over file.
    pub fn voice_over_available(&self) -> bool {
        self.use_sound && !self.support_loaded
    }

    /// A voice-over stream is active, so a second file is refused.
    pub fn voice_over_busy(&self) -> bool {
        self.stream3_active
    }

    /// The voice-over file `name` exists.
    pub fn voice_over_file_exists(&self, name: &str) -> bool {
        self.data
            .exists(&self.data.paths().sound(split_path_stem(name), ".wav"))
    }

    /// `stopDigitalMusic`: stops both active streams at once, no fade.
    pub fn stop_digital_music(&mut self) {
        if self.use_sound && self.use_music {
            for i in 0..2 {
                if self.streams[i].active && self.streams[i].handle {
                    self.mixer.lock().release(STREAM1_CHANNEL + i);
                    self.streams[i].handle = false;
                    self.streams[i].time = 0.0;
                    self.streams[i].active = false;
                }
            }
            self.current_music_id = -1;
        }
    }

    pub fn play_abl_digital_music(&mut self, music_id: i64) {
        let _ = self.play_digital_music(music_id);
    }

    pub fn stop_abl_music(&mut self) {
        self.stop_digital_music();
    }

    fn current_track_volume(&self) -> f32 {
        usize::try_from(self.current_music_id)
            .ok()
            .and_then(|i| self.table.music.get(i))
            .map_or(1.0, |t| t.volume)
    }

    // ---------------------------------------------------------------- per-frame update

    /// `SoundSystem::update`. `frame_length` is the duration of the frame in seconds (`frameLength`).
    pub fn update(&mut self, frame_length: f32) {
        self.update_rain();
        self.update_betty_and_support();
        self.update_voice_over();

        if self.largest_sensor_contact > -1 && self.sensor_beep_update_time > MAX_SENSOR_UPDATE_TIME
        {
            self.play_digital_sample(sfx::SENSOR2, None, false);
            self.sensor_beep_update_time = 0.0;
        }

        self.update_music(frame_length);

        if self.use_sound {
            // Check all samples to see if one should end.
            for i in 0..MAX_DIGITAL_SAMPLES {
                let mode = self.mixer.lock().mode(i);
                if mode == PlayMode::PlayOnce {
                    if self.fade_down[i] {
                        let mut m = self.mixer.lock();
                        let mut vol = m.volume(i);
                        if vol <= 0.1 {
                            vol = 0.1;
                        }
                        m.set_volume(i, vol - 0.1);
                        if m.volume(i) == 0.0 {
                            self.fade_down[i] = false;
                            m.set_mode(i, PlayMode::Stop);
                        }
                    }
                } else {
                    self.fade_down[i] = false;
                    self.channel_sample_id[i] = -1;
                    self.channel_in_use[i] = false;
                }
            }
        }

        if !self.game_paused {
            self.sensor_beep_update_time += frame_length;
        }
    }

    fn update_rain(&mut self) {
        if !(self.use_sound && self.is_raining != self.old_raining) {
            return;
        }
        if self.is_raining != 0 && (self.is_raining as usize) < self.table.bites.len() {
            let ch = FIRE_CHANNEL;
            let id = self.is_raining as usize;
            let vol = self.table.bites[id].volume;
            let mut m = self.mixer.lock();
            m.set_pan(ch, 0.0);
            m.set_volume(ch, self.digital_master_volume * vol * self.sfx_volume);
            self.channel_sample_id[ch] = i64::from(self.is_raining);
            self.channel_in_use[ch] = true;
            if let Some(sound) = self.bank.effect(id) {
                m.assign_sound(ch, sound);
                m.set_mode(ch, PlayMode::Loop);
            }
            self.old_raining = self.is_raining;
        } else {
            // "Stop the sound effect by fading it to zero." As in the C++, the check at the end of
            // `update` clears this flag again for a looping channel, so a started rain loop keeps playing
            // until the sound system is purged.
            self.old_raining = self.is_raining;
            let mode = self.mixer.lock().mode(FIRE_CHANNEL);
            if matches!(mode, PlayMode::PlayOnce | PlayMode::Loop) {
                self.fade_down[FIRE_CHANNEL] = true;
            }
        }
    }

    fn update_betty_and_support(&mut self) {
        if self.use_sound && self.betty_loaded {
            // Betty is done: free her sample.
            let mode = self.mixer.lock().mode(BETTY_CHANNEL);
            if mode != PlayMode::PlayOnce {
                self.mixer.lock().release(BETTY_CHANNEL);
                self.betty_loaded = false;
            }
        }
        if self.use_sound && self.support_loaded {
            let mode = self.mixer.lock().mode(SUPPORT_CHANNEL);
            if mode != PlayMode::PlayOnce {
                self.mixer.lock().release(SUPPORT_CHANNEL);
                self.support_loaded = false;
            }
        }
    }

    fn update_voice_over(&mut self) {
        if !(self.use_sound && self.stream3_handle) {
            return;
        }
        let mode = self.mixer.lock().mode(STREAM3_CHANNEL);
        if !self.stream3_active && mode != PlayMode::PlayOnce {
            self.stream3_active = true;
            self.mixer
                .lock()
                .set_mode(STREAM3_CHANNEL, PlayMode::PlayOnce);
        }
        let mode = self.mixer.lock().mode(STREAM3_CHANNEL);
        if self.stream3_active && mode != PlayMode::PlayOnce {
            self.mixer.lock().release(STREAM3_CHANNEL);
            self.stream3_handle = false;
            self.stream3_active = false;
        }
    }

    fn update_music(&mut self, frame_length: f32) {
        // Start the music change we requested. (Never more than one stream per frame.)
        for i in 0..2 {
            if self.streams[i].handle && !self.streams[i].active {
                self.streams[i].active = true;
                let looping = usize::try_from(self.current_music_id)
                    .ok()
                    .and_then(|k| self.table.music.get(k))
                    .is_some_and(|t| t.looping);
                self.mixer.lock().set_mode(
                    STREAM1_CHANNEL + i,
                    if looping {
                        PlayMode::Loop
                    } else {
                        PlayMode::PlayOnce
                    },
                );
            }
        }

        if !(self.use_music && (self.streams[0].active || self.streams[1].active)) {
            return;
        }

        let fade = self.table.stream_fade_down_time;
        for i in 0..2 {
            let ch = STREAM1_CHANNEL + i;
            if !(self.streams[i].active && self.streams[i].time != 0.0) {
                continue;
            }
            let track_volume = self.current_track_volume();
            let base = self.digital_master_volume * self.music_volume * track_volume;
            if self.streams[i].time < 0.0 {
                // Fading down.
                self.streams[i].time += frame_length;
                if self.streams[i].time >= 0.0 {
                    self.streams[i].time = 0.0;
                    if self.streams[i].handle {
                        self.mixer.lock().release(ch);
                    }
                    self.streams[i].handle = false;
                    self.streams[i].active = false;
                } else {
                    let level = (self.streams[i].time.abs() / fade).clamp(0.0, 1.0);
                    self.mixer.lock().set_volume(
                        ch,
                        self.digital_master_volume * level * self.music_volume * track_volume,
                    );
                }
            } else {
                // Fading up.
                self.streams[i].time -= frame_length;
                if self.streams[i].time <= 0.0 {
                    self.streams[i].time = 0.0;
                    self.mixer.lock().set_volume(ch, base);
                } else {
                    let level = ((fade - self.streams[i].time.abs()) / fade).clamp(0.0, 1.0);
                    self.mixer.lock().set_volume(
                        ch,
                        self.digital_master_volume * level * self.music_volume * track_volume,
                    );
                }
            }
        }

        // A tune that ran to its end.
        for i in 0..2 {
            let ch = STREAM1_CHANNEL + i;
            if self.streams[i].active
                && self.streams[i].handle
                && self.mixer.lock().mode(ch) == PlayMode::Stop
            {
                self.mixer.lock().release(ch);
                self.streams[i].handle = false;
                self.streams[i].time = 0.0;
                self.streams[i].active = false;
            }
        }

        if !self.streams[0].active && !self.streams[1].active {
            self.current_music_id = -1;
        }
    }

    /// The channel-level part of `GameSoundSystem::purgeSoundSystem`: stop every sample channel except
    /// Betty's and forget their bookkeeping, and reset rain and the sensor contact.
    pub(crate) fn purge_channels(&mut self) {
        self.is_raining = 0;
        self.old_raining = 0;
        self.largest_sensor_contact = -1;
        let mut m = self.mixer.lock();
        for i in 0..MAX_DIGITAL_SAMPLES {
            if i != BETTY_CHANNEL {
                m.set_mode(i, PlayMode::Stop);
                self.fade_down[i] = false;
                self.channel_sample_id[i] = -1;
                self.channel_in_use[i] = false;
            }
        }
    }

    pub(crate) fn data_mut(&mut self) -> &mut dyn AudioData {
        self.data.as_mut()
    }
}

/// `_splitpath(name, NULL, NULL, stem, NULL)`: the file name without directory and extension.
pub(crate) fn split_path_stem(name: &str) -> &str {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(0) | None => base,
        Some(i) => &base[..i],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_stem_matches_splitpath() {
        assert_eq!(split_path_stem("tut_4a"), "tut_4a");
        assert_eq!(split_path_stem("data/sound/tut_4a.wav"), "tut_4a");
        assert_eq!(split_path_stem("c:\\x\\y\\v1a.WAV"), "v1a");
        assert_eq!(split_path_stem("a.b.c"), "a.b");
    }

    #[test]
    fn channel_map_matches_the_header() {
        assert_eq!(FIRE_CHANNEL, 15);
        assert_eq!(BETTY_CHANNEL, 14);
        assert_eq!(SUPPORT_CHANNEL, 13);
    }
}
