//! Audio: WAV / Microsoft ADPCM decoding, a channel mixer, and (optionally) a cpal output device.
//!
//! The game uses 16 sample channels plus streaming channels with per-channel volume (0..1), pan
//! (-1..1) and play modes (once / loop / pause / stop); [`Mixer`] reproduces that model. The mixer
//! only renders into a buffer, so it is fully testable headless; [`device::AudioOutput`] (feature
//! `audio-device`) pulls from it on the real audio thread.

pub mod mixer;
pub mod stream;
pub mod wav;

#[cfg(feature = "audio-device")]
pub mod device;

pub use mixer::{
    sample_rate_is_valid, AudioSource, Mixer, PlayMode, QueueSource, QueueWriter, RealtimeSource,
    SharedMixer, Sound, APP_MIXER_CHANNELS, GAME_CHANNELS, MAX_SAMPLE_RATE, MIN_SAMPLE_RATE,
    MOVIE_AUDIO_CHANNEL,
};
pub use stream::StreamingSource;
pub use wav::{WavError, WavFormat, WavStream};
