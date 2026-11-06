// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Real-time output through cpal (CoreAudio, WASAPI, ALSA/PulseAudio/PipeWire).
//!
//! [`AudioOutput::open`] opens the default output device, builds a [`SharedMixer`] at the device's
//! sample rate and starts a stream that pulls from it. Tests never call this (it makes sound);
//! they render the [`Mixer`](super::Mixer) into a buffer instead.

use super::mixer::{Mixer, SharedMixer, GAME_CHANNELS};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};

/// Most frames rendered in one slice of the device callback.
const MAX_RENDER_FRAMES: usize = 8192;

#[derive(Debug)]
pub enum AudioDeviceError {
    NoDevice,
    Config(String),
    Stream(String),
}

impl std::fmt::Display for AudioDeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioDeviceError::NoDevice => write!(f, "no default audio output device"),
            AudioDeviceError::Config(e) => write!(f, "audio config error: {e}"),
            AudioDeviceError::Stream(e) => write!(f, "audio stream error: {e}"),
        }
    }
}

impl std::error::Error for AudioDeviceError {}

/// An open output stream. Dropping it stops playback. Not `Send` on every platform: keep it on
/// the thread that created it and share the [`SharedMixer`] instead.
pub struct AudioOutput {
    _stream: cpal::Stream,
    mixer: SharedMixer,
}

impl AudioOutput {
    /// Open the default device with the game's 19-channel mixer.
    pub fn open() -> Result<Self, AudioDeviceError> {
        Self::open_with_channels(GAME_CHANNELS)
    }

    pub fn open_with_channels(channels: usize) -> Result<Self, AudioDeviceError> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or(AudioDeviceError::NoDevice)?;
        let config = device
            .default_output_config()
            .map_err(|e| AudioDeviceError::Config(e.to_string()))?;
        let rate = config.sample_rate();
        let mixer = SharedMixer::new(Mixer::new(rate, channels));
        let stream_config: cpal::StreamConfig = config.clone().into();
        let stream = match config.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &stream_config, &mixer),
            SampleFormat::I16 => build::<i16>(&device, &stream_config, &mixer),
            SampleFormat::U16 => build::<u16>(&device, &stream_config, &mixer),
            SampleFormat::I32 => build::<i32>(&device, &stream_config, &mixer),
            other => Err(AudioDeviceError::Config(format!(
                "unsupported sample format {other}"
            ))),
        }?;
        stream
            .play()
            .map_err(|e| AudioDeviceError::Stream(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            mixer,
        })
    }

    pub fn mixer(&self) -> SharedMixer {
        self.mixer.clone()
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mixer: &SharedMixer,
) -> Result<cpal::Stream, AudioDeviceError>
where
    T: SizedSample + FromSample<f32>,
{
    let out_channels = config.channels as usize;
    let mixer = mixer.clone();
    // Allocated once, up front: the callback never allocates. Larger device buffers are rendered
    // in slices of this size.
    let mut scratch: Vec<f32> = vec![0.0; MAX_RENDER_FRAMES * 2];
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _| {
                for slice in data.chunks_mut(MAX_RENDER_FRAMES * out_channels) {
                    let frames = slice.len() / out_channels;
                    let stereo = &mut scratch[..frames * 2];
                    // Never wait for the game thread: if it holds the mixer, play silence.
                    match mixer.try_lock() {
                        Some(mut m) => m.render(stereo),
                        None => stereo.fill(0.0),
                    }
                    for (i, frame) in slice.chunks_exact_mut(out_channels).enumerate() {
                        let (l, r) = (stereo[2 * i], stereo[2 * i + 1]);
                        for (c, s) in frame.iter_mut().enumerate() {
                            let v = match (out_channels, c) {
                                (1, _) => (l + r) * 0.5,
                                (_, 0) => l,
                                (_, 1) => r,
                                _ => 0.0,
                            };
                            *s = T::from_sample(v);
                        }
                    }
                }
            },
            |e| log::error!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| AudioDeviceError::Stream(e.to_string()))
}
