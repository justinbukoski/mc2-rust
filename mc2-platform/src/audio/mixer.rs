// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The channel mixer.
//!
//! Model (matching how the game drives `gosAudio`): a fixed set of channels, each holding at most
//! one source plus volume (0..1), pan (-1 left .. +1 right), frequency ratio and play mode. The
//! game's channel map is 0..=15 for sample effects (11..=12 ambient, 13 support officer, 14
//! Betty, 15 fire), and 16..=18 for the two music streams and the voice-over stream
//! ([`GAME_CHANNELS`] = 19).
//!
//! Output is interleaved stereo `f32`, rendered by [`Mixer::render`]; nothing here touches an
//! audio device.
//!
//! Pan law: like DirectSound, panning attenuates the *opposite* side and leaves the near side at
//! full volume (pan +0.5 -> left gain 0.5, right gain 1.0).

use super::wav::{self, WavError, WavStream};
use std::collections::VecDeque;
use std::io::{Read, Seek};
use std::sync::{Arc, Mutex, MutexGuard};

/// Channels used by the game: 16 effect channels plus 3 streams.
pub const GAME_CHANNELS: usize = 19;
/// One extra streaming channel used by MPEG movie audio in the app.
pub const MOVIE_AUDIO_CHANNEL: usize = GAME_CHANNELS;
/// Mixer size for the game plus the dedicated movie soundtrack channel.
pub const APP_MIXER_CHANNELS: usize = GAME_CHANNELS + 1;

/// Channel play mode (`gosAudio_PlayMode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    PlayOnce,
    Loop,
    Pause,
    /// Resume a paused channel.
    Continue,
    Stop,
}

/// Sample rates accepted anywhere in the audio path. Keeps the resampler step bounded
/// (`MAX_SAMPLE_RATE / MIN_SAMPLE_RATE * MAX_FREQUENCY` source frames per output frame at worst).
pub const MIN_SAMPLE_RATE: u32 = 1000;
pub const MAX_SAMPLE_RATE: u32 = 192_000;

/// Whether `rate` is inside [`MIN_SAMPLE_RATE`]..=[`MAX_SAMPLE_RATE`].
pub fn sample_rate_is_valid(rate: u32) -> bool {
    (MIN_SAMPLE_RATE..=MAX_SAMPLE_RATE).contains(&rate)
}

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// A source the audio callback may pull from: reading never blocks on I/O or another thread's
/// lock and never allocates. Sealed: only in-memory sounds, [`QueueSource`] and
/// [`StreamingSource`](super::StreamingSource) qualify. Sources that do file I/O or decoding
/// (`WavStream`, anything custom) must be wrapped in `StreamingSource`, which moves that work
/// onto a worker thread.
pub trait RealtimeSource: AudioSource + sealed::Sealed {}

/// Anything that can produce audio frames. Implementations may do I/O; to play one through the
/// mixer wrap it in [`StreamingSource`](super::StreamingSource).
pub trait AudioSource: Send {
    fn channels(&self) -> u16;
    fn sample_rate(&self) -> u32;
    /// Fill `out` (a multiple of `channels()` samples) with interleaved frames. Returns the
    /// number of **frames** written; 0 means the source has ended.
    fn read(&mut self, out: &mut [f32]) -> usize;
    /// Restart from the beginning. Returns false if the source cannot rewind.
    fn rewind(&mut self) -> bool;
    /// Whether [`rewind`](Self::rewind) can succeed.
    fn can_rewind(&self) -> bool {
        true
    }
    /// The channel's loop flag changed. Sources that prefetch use it to wrap around without a gap.
    fn set_looping(&mut self, _looping: bool) {}
}

/// A fully decoded sound held in memory and shareable between channels.
#[derive(Debug)]
pub struct Sound {
    channels: u16,
    sample_rate: u32,
    samples: Vec<i16>,
}

impl Sound {
    pub fn from_pcm16(channels: u16, sample_rate: u32, samples: Vec<i16>) -> Arc<Self> {
        assert!(channels == 1 || channels == 2);
        assert!(
            sample_rate_is_valid(sample_rate),
            "sample rate {sample_rate} outside {MIN_SAMPLE_RATE}..={MAX_SAMPLE_RATE}"
        );
        let n = samples.len() / channels as usize * channels as usize;
        let mut samples = samples;
        samples.truncate(n);
        Arc::new(Self {
            channels,
            sample_rate,
            samples,
        })
    }

    /// Decode a RIFF/WAVE image (PCM or MS-ADPCM).
    pub fn from_wav(bytes: &[u8]) -> Result<Arc<Self>, WavError> {
        let w = wav::parse(bytes)?;
        Ok(Self::from_pcm16(
            w.format.channels,
            w.format.sample_rate,
            w.decode_i16(),
        ))
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / self.sample_rate as f64
    }

    /// A new playback cursor over this sound.
    pub fn source(self: &Arc<Self>) -> Box<dyn RealtimeSource> {
        Box::new(SoundSource {
            sound: Arc::clone(self),
            pos: 0,
        })
    }
}

struct SoundSource {
    sound: Arc<Sound>,
    pos: usize,
}

impl sealed::Sealed for SoundSource {}
impl RealtimeSource for SoundSource {}

impl AudioSource for SoundSource {
    fn channels(&self) -> u16 {
        self.sound.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sound.sample_rate
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let avail = self.sound.samples.len() - self.pos;
        let n = avail.min(out.len());
        for (o, s) in out[..n]
            .iter_mut()
            .zip(&self.sound.samples[self.pos..self.pos + n])
        {
            *o = *s as f32 / 32768.0;
        }
        self.pos += n;
        n / self.sound.channels as usize
    }
    fn rewind(&mut self) -> bool {
        self.pos = 0;
        true
    }
}

impl<R: Read + Seek + Send> AudioSource for WavStream<R> {
    fn channels(&self) -> u16 {
        self.format().channels
    }
    fn sample_rate(&self) -> u32 {
        self.format().sample_rate
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let mut tmp = [0i16; 2048];
        let ch = self.format().channels as usize;
        let want = out.len().min(tmp.len()) / ch * ch;
        let n = self.read_samples(&mut tmp[..want]);
        for (o, s) in out[..n].iter_mut().zip(&tmp[..n]) {
            *o = *s as f32 / 32768.0;
        }
        n / ch
    }
    fn rewind(&mut self) -> bool {
        WavStream::rewind(self).is_ok()
    }
}

#[derive(Default)]
struct QueueState {
    samples: VecDeque<f32>,
    finished: bool,
    drained: bool,
}

/// A push-fed source (movie soundtracks, generated audio). Plays silence on underrun until the
/// writer calls [`QueueWriter::finish`] and the queue drains.
pub struct QueueSource {
    shared: Arc<Mutex<QueueState>>,
    channels: u16,
    sample_rate: u32,
}

/// The producing half of a [`QueueSource`].
#[derive(Clone)]
pub struct QueueWriter {
    shared: Arc<Mutex<QueueState>>,
    channels: u16,
}

impl QueueSource {
    pub fn new(channels: u16, sample_rate: u32) -> (QueueSource, QueueWriter) {
        assert!(channels == 1 || channels == 2);
        assert!(
            sample_rate_is_valid(sample_rate),
            "sample rate {sample_rate} out of range"
        );
        let shared = Arc::new(Mutex::new(QueueState::default()));
        (
            QueueSource {
                shared: Arc::clone(&shared),
                channels,
                sample_rate,
            },
            QueueWriter { shared, channels },
        )
    }
}

impl QueueWriter {
    /// Append interleaved samples.
    pub fn push(&self, samples: &[f32]) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.samples.extend(samples.iter().copied());
    }

    /// No more data will come; the source ends once drained.
    pub fn finish(&self) {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finished = true;
    }

    /// Drop queued samples and finish immediately. Used when a streamed source is skipped.
    pub fn cancel(&self) {
        let mut s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        s.samples.clear();
        s.finished = true;
        s.drained = true;
    }

    /// Frames waiting to be played.
    pub fn queued_frames(&self) -> usize {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .samples
            .len()
            / self.channels as usize
    }

    /// Whether the mixer has consumed the final queued frame (or playback was cancelled).
    pub fn drained(&self) -> bool {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drained
    }
}

impl sealed::Sealed for QueueSource {}
impl RealtimeSource for QueueSource {}

impl AudioSource for QueueSource {
    fn channels(&self) -> u16 {
        self.channels
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn read(&mut self, out: &mut [f32]) -> usize {
        let ch = self.channels as usize;
        // Never wait for the producer: on contention play one frame of silence instead.
        let mut s = match self.shared.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                out[..ch].fill(0.0);
                return 1;
            }
        };
        let have = s.samples.len() / ch * ch;
        if have == 0 {
            if s.finished {
                s.drained = true;
                return 0;
            }
            // Underrun: one frame of silence at a time so audio pushed later is not queued behind
            // a long block of pre-rendered silence.
            out[..ch].fill(0.0);
            return 1;
        }
        let n = have.min(out.len());
        for (o, v) in out[..n].iter_mut().zip(s.samples.drain(..n)) {
            *o = v;
        }
        n / ch
    }
    fn rewind(&mut self) -> bool {
        false
    }
    fn can_rewind(&self) -> bool {
        false
    }
}

const CHUNK_FRAMES: usize = 1024;

/// Slowest playback ratio accepted by [`Mixer::set_frequency`].
pub const MIN_FREQUENCY: f32 = 1.0 / 64.0;
/// Fastest playback ratio accepted by [`Mixer::set_frequency`].
pub const MAX_FREQUENCY: f32 = 64.0;

struct Channel {
    source: Option<Box<dyn RealtimeSource>>,
    playing: bool,
    paused: bool,
    looping: bool,
    volume: f32,
    pan: f32,
    frequency: f32,
    // Resampler state.
    primed: bool,
    cur: [f32; 2],
    next: [f32; 2],
    has_next: bool,
    frac: f64,
    scratch: Vec<f32>,
    chunk: Vec<[f32; 2]>,
    chunk_pos: usize,
    frames_pulled: u64,
}

impl Channel {
    fn new() -> Self {
        Self {
            source: None,
            playing: false,
            paused: false,
            looping: false,
            volume: 1.0,
            pan: 0.0,
            frequency: 1.0,
            primed: false,
            cur: [0.0; 2],
            next: [0.0; 2],
            has_next: false,
            frac: 0.0,
            // Sized once so rendering (possibly on the audio thread) never allocates.
            scratch: Vec::with_capacity(CHUNK_FRAMES * 2),
            chunk: Vec::with_capacity(CHUNK_FRAMES),
            chunk_pos: 0,
            frames_pulled: 0,
        }
    }

    fn reset_playback(&mut self) {
        self.primed = false;
        self.has_next = false;
        self.frac = 0.0;
        self.chunk.clear();
        self.chunk_pos = 0;
        self.frames_pulled = 0;
    }

    /// Pull one stereo source frame, handling looping.
    fn pull(&mut self) -> Option<[f32; 2]> {
        if self.chunk_pos >= self.chunk.len() {
            self.chunk.clear();
            self.chunk_pos = 0;
            let looping = self.looping;
            let src = self.source.as_mut()?;
            let ch = src.channels() as usize;
            self.scratch.resize(CHUNK_FRAMES * ch, 0.0);
            let mut n = src.read(&mut self.scratch);
            if n == 0 && looping && src.rewind() {
                n = src.read(&mut self.scratch);
            }
            if n == 0 {
                return None;
            }
            for f in 0..n {
                self.chunk.push(if ch == 1 {
                    let v = self.scratch[f];
                    [v, v]
                } else {
                    [self.scratch[2 * f], self.scratch[2 * f + 1]]
                });
            }
        }
        let f = self.chunk[self.chunk_pos];
        self.chunk_pos += 1;
        self.frames_pulled += 1;
        Some(f)
    }

    fn gains(&self, master: f32) -> (f32, f32) {
        let v = self.volume.clamp(0.0, 1.0) * master;
        let p = self.pan.clamp(-1.0, 1.0);
        (v * (1.0 - p.max(0.0)), v * (1.0 + p.min(0.0)))
    }

    /// Mix this channel into `out` (interleaved stereo).
    fn mix_into(&mut self, out: &mut [f32], out_rate: u32, master: f32) {
        if !self.playing || self.paused {
            return;
        }
        let Some(src_rate) = self.source.as_ref().map(|s| s.sample_rate()) else {
            self.playing = false;
            return;
        };
        let step = src_rate as f64 / out_rate as f64 * self.frequency as f64;
        let (gl, gr) = self.gains(master);
        if !self.primed {
            match self.pull() {
                Some(f) => {
                    self.cur = f;
                    match self.pull() {
                        Some(n) => {
                            self.next = n;
                            self.has_next = true;
                        }
                        None => self.has_next = false,
                    }
                    self.frac = 0.0;
                    self.primed = true;
                }
                None => {
                    self.playing = false;
                    return;
                }
            }
        }
        for frame in out.as_chunks_mut::<2>().0.iter_mut() {
            let (l, r) = if self.has_next {
                let t = self.frac as f32;
                (
                    self.cur[0] + (self.next[0] - self.cur[0]) * t,
                    self.cur[1] + (self.next[1] - self.cur[1]) * t,
                )
            } else {
                (self.cur[0], self.cur[1])
            };
            frame[0] += l * gl;
            frame[1] += r * gr;
            self.frac += step;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                if !self.has_next {
                    self.playing = false;
                    return;
                }
                self.cur = self.next;
                match self.pull() {
                    Some(n) => self.next = n,
                    None => self.has_next = false,
                }
            }
        }
    }
}

/// The mixer. Lives behind a [`SharedMixer`] when a real device pulls from it.
pub struct Mixer {
    sample_rate: u32,
    channels: Vec<Channel>,
    master: f32,
}

impl Mixer {
    pub fn new(sample_rate: u32, channel_count: usize) -> Self {
        assert!(sample_rate_is_valid(sample_rate));
        Self {
            sample_rate,
            channels: (0..channel_count).map(|_| Channel::new()).collect(),
            master: 1.0,
        }
    }

    /// A mixer with the game's 19 channels.
    pub fn for_game(sample_rate: u32) -> Self {
        Self::new(sample_rate, GAME_CHANNELS)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.clamp(0.0, 1.0);
    }

    pub fn master_volume(&self) -> f32 {
        self.master
    }

    fn ch(&mut self, c: usize) -> Option<&mut Channel> {
        self.channels.get_mut(c)
    }

    /// Give a channel a new source (`gosAudio_AssignResourceToChannel`). The channel is stopped;
    /// start it with [`set_mode`](Self::set_mode).
    ///
    /// Only [`RealtimeSource`]s are accepted, because the audio callback renders them. A source
    /// that reads files must go through [`StreamingSource`](super::StreamingSource):
    ///
    /// ```compile_fail
    /// use mc2_platform::audio::{Mixer, WavStream};
    /// let mut m = Mixer::new(44100, 1);
    /// let s = WavStream::new(std::io::Cursor::new(Vec::<u8>::new())).unwrap();
    /// m.assign(0, Box::new(s));
    /// ```
    pub fn assign(&mut self, channel: usize, source: Box<dyn RealtimeSource>) {
        if let Some(c) = self.ch(channel) {
            c.source = Some(source);
            c.playing = false;
            c.paused = false;
            c.reset_playback();
        }
    }

    /// Assign an in-memory sound.
    pub fn assign_sound(&mut self, channel: usize, sound: &Arc<Sound>) {
        self.assign(channel, sound.source());
    }

    /// Drop the channel's source (`gosAudio_DestroyResource`).
    pub fn release(&mut self, channel: usize) {
        if let Some(c) = self.ch(channel) {
            c.source = None;
            c.playing = false;
            c.paused = false;
            c.reset_playback();
        }
    }

    /// `gosAudio_SetChannelPlayMode`. `PlayOnce`/`Loop` start a stopped channel from the
    /// beginning and resume a paused one.
    pub fn set_mode(&mut self, channel: usize, mode: PlayMode) {
        let Some(c) = self.ch(channel) else { return };
        match mode {
            PlayMode::PlayOnce | PlayMode::Loop => {
                c.looping = mode == PlayMode::Loop;
                let looping = c.looping;
                if let Some(src) = c.source.as_mut() {
                    src.set_looping(looping);
                } else {
                    return;
                }
                if c.paused {
                    c.paused = false;
                } else if !c.playing {
                    // Replay from the start (a finished sound is rewound).
                    let rewound = c.source.as_mut().is_some_and(|s| s.rewind());
                    let fresh = c.frames_pulled == 0;
                    if rewound || fresh {
                        c.reset_playback();
                        c.playing = true;
                    }
                }
            }
            PlayMode::Pause => {
                if c.playing {
                    c.paused = true;
                }
            }
            PlayMode::Continue => c.paused = false,
            PlayMode::Stop => {
                c.playing = false;
                c.paused = false;
            }
        }
    }

    /// `gosAudio_GetChannelPlayMode`: `Stop` once a sound has finished or no source is set.
    pub fn mode(&self, channel: usize) -> PlayMode {
        match self.channels.get(channel) {
            Some(c) if c.playing && c.paused => PlayMode::Pause,
            Some(c) if c.playing && c.looping => PlayMode::Loop,
            Some(c) if c.playing => PlayMode::PlayOnce,
            _ => PlayMode::Stop,
        }
    }

    pub fn set_volume(&mut self, channel: usize, v: f32) {
        if let Some(c) = self.ch(channel) {
            c.volume = v.clamp(0.0, 1.0);
        }
    }

    pub fn volume(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(0.0, |c| c.volume)
    }

    pub fn set_pan(&mut self, channel: usize, pan: f32) {
        if let Some(c) = self.ch(channel) {
            c.pan = pan.clamp(-1.0, 1.0);
        }
    }

    pub fn pan(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(0.0, |c| c.pan)
    }

    /// Playback-rate ratio (1.0 normal), clamped to [`MIN_FREQUENCY`]..=[`MAX_FREQUENCY`].
    /// Non-finite values are ignored, so the resampler step is always finite and bounded.
    pub fn set_frequency(&mut self, channel: usize, ratio: f32) {
        if !ratio.is_finite() {
            return;
        }
        if let Some(c) = self.ch(channel) {
            c.frequency = ratio.clamp(MIN_FREQUENCY, MAX_FREQUENCY);
        }
    }

    pub fn frequency(&self, channel: usize) -> f32 {
        self.channels.get(channel).map_or(1.0, |c| c.frequency)
    }

    /// First channel in `start..end` that is not playing (the game's `findOpenChannel`).
    pub fn find_free_channel(&self, start: usize, end: usize) -> Option<usize> {
        (start..end.min(self.channels.len())).find(|&c| self.mode(c) == PlayMode::Stop)
    }

    /// Render `out.len() / 2` stereo frames (interleaved L/R), overwriting `out`.
    pub fn render(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let rate = self.sample_rate;
        let master = self.master;
        for c in self.channels.iter_mut() {
            c.mix_into(out, rate, master);
        }
        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

/// A mixer shared between the game thread and the audio device callback.
#[derive(Clone)]
pub struct SharedMixer(Arc<Mutex<Mixer>>);

impl SharedMixer {
    pub fn new(mixer: Mixer) -> Self {
        Self(Arc::new(Mutex::new(mixer)))
    }

    pub fn lock(&self) -> MutexGuard<'_, Mixer> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Lock without waiting; `None` if the game thread currently holds the mixer. Used by the
    /// audio callback, which must never block.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, Mixer>> {
        match self.0.try_lock() {
            Ok(g) => Some(g),
            Err(std::sync::TryLockError::Poisoned(e)) => Some(e.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::stream::StreamingSource;
    use crate::audio::wav::testutil::pcm16_wav;

    const R: u32 = 8000;

    fn sound(samples: &[i16]) -> Arc<Sound> {
        Sound::from_pcm16(1, R, samples.to_vec())
    }

    fn render(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        m.render(&mut out);
        out
    }

    fn lefts(v: &[f32]) -> Vec<f32> {
        v.chunks(2).map(|f| f[0]).collect()
    }
    fn rights(v: &[f32]) -> Vec<f32> {
        v.chunks(2).map(|f| f[1]).collect()
    }

    #[test]
    fn silent_when_nothing_plays() {
        let mut m = Mixer::new(R, 4);
        assert!(render(&mut m, 64).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn queue_drain_waits_for_mixer_prefetch_to_be_consumed() {
        let (source, writer) = QueueSource::new(2, R);
        writer.push(&[0.5; 64]);
        writer.finish();
        let mut mixer = Mixer::new(R, 1);
        mixer.assign(0, Box::new(source));
        mixer.set_mode(0, PlayMode::PlayOnce);

        render(&mut mixer, 1);
        assert_eq!(writer.queued_frames(), 0, "mixer has prefetched the queue");
        assert!(!writer.drained(), "prefetched frames are not yet played");

        render(&mut mixer, 40);
        assert!(
            writer.drained(),
            "the mixer consumed the final queued frame"
        );
    }

    #[test]
    fn plays_once_exactly_and_stops() {
        let mut m = Mixer::new(R, 4);
        m.assign_sound(0, &sound(&[16384, -16384, 8192]));
        assert_eq!(m.mode(0), PlayMode::Stop);
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(m.mode(0), PlayMode::PlayOnce);
        let out = render(&mut m, 6);
        assert_eq!(lefts(&out), vec![0.5, -0.5, 0.25, 0.0, 0.0, 0.0]);
        assert_eq!(rights(&out), lefts(&out));
        assert_eq!(m.mode(0), PlayMode::Stop);
    }

    #[test]
    fn volume_and_master() {
        let mut m = Mixer::new(R, 2);
        m.assign_sound(0, &sound(&[16384; 8]));
        m.set_volume(0, 0.5);
        m.set_master_volume(0.5);
        m.set_mode(0, PlayMode::PlayOnce);
        let out = render(&mut m, 4);
        assert!(lefts(&out).iter().all(|&s| (s - 0.125).abs() < 1e-6));
    }

    #[test]
    fn pan_attenuates_opposite_side() {
        let mut m = Mixer::new(R, 2);
        m.assign_sound(0, &sound(&[16384; 16]));
        m.set_pan(0, 1.0);
        m.set_mode(0, PlayMode::PlayOnce);
        let out = render(&mut m, 4);
        assert!(lefts(&out).iter().all(|&s| s == 0.0));
        assert!(rights(&out).iter().all(|&s| s == 0.5));
        m.assign_sound(0, &sound(&[16384; 16]));
        m.set_pan(0, -0.5);
        m.set_mode(0, PlayMode::PlayOnce);
        let out = render(&mut m, 4);
        assert!(lefts(&out).iter().all(|&s| s == 0.5));
        assert!(rights(&out).iter().all(|&s| (s - 0.25).abs() < 1e-7));
    }

    #[test]
    fn looping_wraps_seamlessly_until_stopped() {
        let mut m = Mixer::new(R, 2);
        m.assign_sound(0, &sound(&[8192, 16384, 24576]));
        m.set_mode(0, PlayMode::Loop);
        let out = render(&mut m, 8);
        assert_eq!(
            lefts(&out),
            vec![0.25, 0.5, 0.75, 0.25, 0.5, 0.75, 0.25, 0.5]
        );
        assert_eq!(m.mode(0), PlayMode::Loop);
        m.set_mode(0, PlayMode::Stop);
        assert_eq!(m.mode(0), PlayMode::Stop);
        assert!(render(&mut m, 4).iter().all(|&s| s == 0.0));
    }

    #[test]
    fn loop_longer_than_a_chunk_does_not_glitch() {
        let n = CHUNK_FRAMES * 2 + 17;
        let samples: Vec<i16> = (0..n).map(|i| (i % 1000) as i16 * 16).collect();
        let mut m = Mixer::new(R, 1);
        m.assign_sound(0, &sound(&samples));
        m.set_mode(0, PlayMode::Loop);
        let out = lefts(&render(&mut m, n * 2 + 5));
        for (i, v) in out.iter().enumerate() {
            let expect = samples[i % n] as f32 / 32768.0;
            assert_eq!(*v, expect, "frame {i}");
        }
    }

    #[test]
    fn pause_and_continue_keep_position() {
        let mut m = Mixer::new(R, 2);
        m.assign_sound(0, &sound(&[8192, 16384, 24576, 32767]));
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(lefts(&render(&mut m, 2)), vec![0.25, 0.5]);
        m.set_mode(0, PlayMode::Pause);
        assert_eq!(m.mode(0), PlayMode::Pause);
        assert!(render(&mut m, 5).iter().all(|&s| s == 0.0));
        m.set_mode(0, PlayMode::Continue);
        assert_eq!(lefts(&render(&mut m, 2)), vec![0.75, 32767.0 / 32768.0]);
    }

    #[test]
    fn replay_restarts_from_the_beginning() {
        let mut m = Mixer::new(R, 2);
        m.assign_sound(0, &sound(&[8192, 16384]));
        m.set_mode(0, PlayMode::PlayOnce);
        render(&mut m, 4);
        assert_eq!(m.mode(0), PlayMode::Stop);
        m.set_mode(0, PlayMode::PlayOnce);
        assert_eq!(lefts(&render(&mut m, 2)), vec![0.25, 0.5]);
    }

    #[test]
    fn channels_sum_and_clip() {
        let mut m = Mixer::new(R, 3);
        m.assign_sound(0, &sound(&[16384; 4]));
        m.assign_sound(1, &sound(&[8192; 4]));
        m.set_mode(0, PlayMode::PlayOnce);
        m.set_mode(1, PlayMode::PlayOnce);
        assert!(lefts(&render(&mut m, 4)).iter().all(|&s| s == 0.75));
        m.assign_sound(0, &sound(&[32767; 4]));
        m.assign_sound(1, &sound(&[32767; 4]));
        m.set_mode(0, PlayMode::PlayOnce);
        m.set_mode(1, PlayMode::PlayOnce);
        assert!(lefts(&render(&mut m, 4)).iter().all(|&s| s == 1.0));
    }

    #[test]
    fn stereo_source_keeps_sides() {
        let mut m = Mixer::new(R, 1);
        let s = Sound::from_pcm16(2, R, vec![16384, -16384, 8192, -8192]);
        m.assign_sound(0, &s);
        m.set_mode(0, PlayMode::PlayOnce);
        let out = render(&mut m, 2);
        assert_eq!(out, vec![0.5, -0.5, 0.25, -0.25]);
    }

    #[test]
    fn resamples_from_lower_rate_with_interpolation() {
        // 4000 Hz source into an 8000 Hz mixer: each source frame spans two output frames.
        let mut m = Mixer::new(8000, 1);
        let s = Sound::from_pcm16(1, 4000, vec![0, 16384, 0]);
        m.assign_sound(0, &s);
        m.set_mode(0, PlayMode::PlayOnce);
        let out = lefts(&render(&mut m, 8));
        assert_eq!(&out[..5], &[0.0, 0.25, 0.5, 0.25, 0.0]);
        assert_eq!(m.mode(0), PlayMode::Stop);
    }

    #[test]
    fn frequency_ratio_changes_duration() {
        let mut m = Mixer::new(R, 1);
        let s = sound(&(0..100).map(|i| i as i16 * 100).collect::<Vec<_>>());
        m.assign_sound(0, &s);
        m.set_frequency(0, 2.0);
        m.set_mode(0, PlayMode::PlayOnce);
        render(&mut m, 49);
        assert_eq!(m.mode(0), PlayMode::PlayOnce);
        render(&mut m, 4);
        assert_eq!(m.mode(0), PlayMode::Stop);
    }

    #[test]
    fn non_finite_and_absurd_frequencies_cannot_hang_the_resampler() {
        let mut m = Mixer::new(R, 1);
        m.assign_sound(0, &sound(&[1000; 4000]));
        for bad in [f32::INFINITY, f32::NAN, f32::NEG_INFINITY] {
            m.set_frequency(0, bad);
            assert_eq!(m.frequency(0), 1.0);
        }
        m.set_frequency(0, 1.0e30);
        assert_eq!(m.frequency(0), MAX_FREQUENCY);
        m.set_frequency(0, -5.0);
        assert_eq!(m.frequency(0), MIN_FREQUENCY);
        m.set_frequency(0, f32::MAX);
        m.set_mode(0, PlayMode::Loop);
        // Terminates (a hang would time the test out) and stays within range.
        let out = render(&mut m, 512);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn try_lock_reports_contention_instead_of_blocking() {
        let sm = SharedMixer::new(Mixer::new(R, 1));
        let held = sm.lock();
        assert!(sm.try_lock().is_none());
        drop(held);
        assert!(sm.try_lock().is_some());
    }

    #[test]
    fn queue_source_never_blocks_on_a_held_writer() {
        let (mut src, writer) = QueueSource::new(1, R);
        writer.push(&[0.5]);
        let shared = Arc::clone(&src.shared);
        let guard = shared.lock().unwrap();
        let mut out = [9.0f32; 4];
        // Producer holds the lock: the read returns silence immediately.
        assert_eq!(src.read(&mut out), 1);
        assert_eq!(out[0], 0.0);
        drop(guard);
        assert_eq!(src.read(&mut out), 1);
        assert_eq!(out[0], 0.5);
    }

    #[test]
    #[should_panic]
    fn sounds_reject_out_of_range_sample_rates() {
        let _ = Sound::from_pcm16(1, 4_000_000_000, vec![0; 4]);
    }

    #[test]
    #[should_panic]
    fn mixer_rejects_out_of_range_output_rate() {
        let _ = Mixer::new(0, 1);
    }

    #[test]
    fn worst_case_resample_step_still_renders_quickly() {
        // Highest source rate into the lowest output rate at the fastest ratio: bounded work.
        let mut m = Mixer::new(MIN_SAMPLE_RATE, 1);
        let s = Sound::from_pcm16(1, MAX_SAMPLE_RATE, vec![1000; 5000]);
        m.assign_sound(0, &s);
        m.set_frequency(0, MAX_FREQUENCY);
        m.set_mode(0, PlayMode::Loop);
        let out = render(&mut m, 256);
        assert!(out.iter().all(|v| v.abs() <= 1.0));
    }

    #[test]
    fn find_free_channel_skips_busy_ones() {
        let mut m = Mixer::new(R, 4);
        m.assign_sound(1, &sound(&[1000; 100]));
        m.set_mode(1, PlayMode::Loop);
        assert_eq!(m.find_free_channel(1, 4), Some(2));
        assert_eq!(m.find_free_channel(1, 2), None);
        assert_eq!(m.find_free_channel(0, 4), Some(0));
    }

    #[test]
    fn release_and_out_of_range_are_safe() {
        let mut m = Mixer::new(R, 2);
        m.assign_sound(0, &sound(&[1000; 10]));
        m.set_mode(0, PlayMode::Loop);
        m.release(0);
        assert_eq!(m.mode(0), PlayMode::Stop);
        m.set_mode(99, PlayMode::Loop);
        m.set_volume(99, 1.0);
        assert_eq!(m.mode(99), PlayMode::Stop);
    }

    #[test]
    fn same_sound_on_two_channels_is_independent() {
        let mut m = Mixer::new(R, 2);
        let s = sound(&[8192, 16384, 24576]);
        m.assign_sound(0, &s);
        m.assign_sound(1, &s);
        m.set_mode(0, PlayMode::PlayOnce);
        render(&mut m, 1);
        m.set_mode(1, PlayMode::PlayOnce);
        let out = lefts(&render(&mut m, 2));
        // Channel 0 at frames 1,2 (0.5, 0.75) + channel 1 at frames 0,1 (0.25, 0.5).
        assert_eq!(out, vec![0.75, 1.0]);
    }

    #[test]
    fn wav_bytes_round_trip_through_sound() {
        let w = pcm16_wav(1, 22050, &[0, 1000, -1000]);
        let s = Sound::from_wav(&w).unwrap();
        assert_eq!((s.channels(), s.sample_rate(), s.frames()), (1, 22050, 3));
        assert!((s.duration_secs() - 3.0 / 22050.0).abs() < 1e-12);
    }

    #[test]
    fn queue_source_plays_pushed_audio_silence_on_underrun_then_ends() {
        let (src, writer) = QueueSource::new(1, R);
        let mut m = Mixer::new(R, 1);
        m.assign(0, Box::new(src));
        m.set_mode(0, PlayMode::PlayOnce);
        // Nothing queued yet: channel keeps "playing" silence.
        assert!(render(&mut m, 4).iter().all(|&s| s == 0.0));
        assert_eq!(m.mode(0), PlayMode::PlayOnce);
        writer.push(&[0.5, 0.25]);
        writer.push(&[0.125]);
        writer.finish();
        // The resampler looks one frame ahead, so a little silence may precede the data.
        let out: Vec<f32> = lefts(&render(&mut m, 8))
            .into_iter()
            .filter(|&s| s != 0.0)
            .collect();
        assert_eq!(out, vec![0.5, 0.25, 0.125]);
        assert_eq!(m.mode(0), PlayMode::Stop);
    }

    #[test]
    fn streamed_wav_source_plays() {
        let samples: Vec<i16> = (0..3000).map(|i| (i % 200) as i16 * 100).collect();
        let bytes = pcm16_wav(1, R, &samples);
        let stream = WavStream::new(std::io::Cursor::new(bytes)).unwrap();
        let mut m = Mixer::new(R, 1);
        m.assign(0, Box::new(StreamingSource::spawn(Box::new(stream))));
        m.set_mode(0, PlayMode::PlayOnce);
        let out = lefts(&render(&mut m, 3000));
        for (i, v) in out.iter().enumerate() {
            assert_eq!(*v, samples[i] as f32 / 32768.0, "frame {i}");
        }
        // The worker thread delivers the end marker asynchronously; wait for it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while m.mode(0) != PlayMode::Stop && std::time::Instant::now() < deadline {
            render(&mut m, 2);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(m.mode(0), PlayMode::Stop);
    }

    #[test]
    fn shared_mixer_locks() {
        let sm = SharedMixer::new(Mixer::for_game(44100));
        let sm2 = sm.clone();
        sm.lock().set_master_volume(0.3);
        assert_eq!(sm2.lock().master_volume(), 0.3);
        assert_eq!(sm2.lock().channel_count(), GAME_CHANNELS);
    }
}
