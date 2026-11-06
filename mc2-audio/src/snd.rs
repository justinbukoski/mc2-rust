// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The sound table: `data/sound/sound.snd`.
//!
//! Port of the parsing half of `SoundSystem::init(const char *soundFileName)` (`mclib/soundsys.cpp`): the
//! `[SoundSetup]` limits, one `[SoundBiteN]` block per effect and the `[DigitalMusic]` block. The sample
//! data itself is `sound.pak` (see [`crate::bank`]); bite `N` is packet `N` (the C++ passes the loop index to
//! `preloadSoundBite`, so the `soundId` key is only stored).

use crate::error::{AudioError, Result};
use mc2_formats::ini::FitIni;

/// One `[SoundBiteN]` block.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundBite {
    /// Cache priority (`priority`). Stored by the game, but `playDigitalSample` never looks at it.
    pub priority: u32,
    /// Cache policy (`cache`). Unused at run time: every bite is preloaded.
    pub cache: u32,
    /// `soundId` key (equal to the block index in the shipped data).
    pub sound_id: u32,
    /// `preload` key. Not read by the C++ (it always preloads).
    pub preload: bool,
    /// Bite volume, 0..1 (`volume`).
    pub volume: f32,
}

/// One digital music entry (`DMSn`, `DMSLoopn`, `DMSVolumen`).
#[derive(Clone, Debug, PartialEq)]
pub struct MusicTrack {
    /// File name without directory or extension (`music00`), or `NONE`.
    pub name: String,
    pub looping: bool,
    /// Per-track volume, 1.0 when `DMSVolumen` is absent.
    pub volume: f32,
}

impl MusicTrack {
    /// Entries named `NONE...` (compared on the first four characters) are skipped by `playDigitalMusic`.
    pub fn is_none(&self) -> bool {
        self.name.as_bytes().get(..4) == Some(b"NONE")
    }
}

/// Parsed `sound.snd`.
#[derive(Clone, Debug)]
pub struct SoundTable {
    /// `MaxSoundDistance`. Read by the game but never used: the falloff distance is the constant 1500.
    pub max_sound_distance: f32,
    /// `soundHeapSize`.
    pub sound_heap_size: u32,
    pub bites: Vec<SoundBite>,
    pub music: Vec<MusicTrack>,
    /// `StreamFadeDownTime`: seconds for a music cross-fade.
    pub stream_fade_down_time: f32,
    pub stream_bit_depth: u32,
    pub stream_channels: u32,
}

fn need<T>(r: mc2_formats::Result<T>, key: &str) -> Result<T> {
    r.map_err(|e| match e {
        mc2_formats::FormatError::NotFound { .. } => {
            AudioError::Missing(format!("sound.snd key {key}"))
        }
        other => AudioError::Format(other),
    })
}

impl SoundTable {
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let mut f = FitIni::from_bytes(bytes)?;

        need(f.seek_block("SoundSetup"), "[SoundSetup]")?;
        let sound_heap_size = need(f.read_u32("soundHeapSize"), "soundHeapSize")?;
        let max_sound_distance = need(f.read_f32("MaxSoundDistance"), "MaxSoundDistance")?;

        need(f.seek_block("SoundBites"), "[SoundBites]")?;
        let num_bites = need(f.read_u32("numBites"), "numBites")?;
        let mut bites = Vec::with_capacity(num_bites as usize);
        for i in 0..num_bites {
            need(f.seek_block(&format!("SoundBite{i}")), "[SoundBiteN]")?;
            let priority = need(f.read_u32("priority"), "priority")?;
            let cache = need(f.read_u32("cache"), "cache")?;
            let sound_id = need(f.read_u32("soundId"), "soundId")?;
            let preload = f.read_u32("preload").map(|v| v != 0).unwrap_or(false);
            let volume = need(f.read_f32("volume"), "volume")?;
            bites.push(SoundBite {
                priority,
                cache,
                sound_id,
                preload,
                volume,
            });
        }

        need(f.seek_block("DigitalMusic"), "[DigitalMusic]")?;
        let num_dms = need(f.read_i64("NumDMS"), "NumDMS")?.max(0);
        let stream_fade_down_time = need(f.read_f32("StreamFadeDownTime"), "StreamFadeDownTime")?;
        let stream_bit_depth = need(f.read_u32("StreamBitDepth"), "StreamBitDepth")?;
        let stream_channels = need(f.read_u32("StreamChannels"), "StreamChannels")?;
        let mut music = Vec::with_capacity(num_dms as usize);
        for i in 0..num_dms {
            // The C++ buffer is 30 bytes.
            let name = need(f.read_string(&format!("DMS{i}"), 29), "DMSn")?;
            let looping = need(f.read_bool(&format!("DMSLoop{i}")), "DMSLoopn")?;
            let volume = f.read_f32(&format!("DMSVolume{i}")).unwrap_or(1.0);
            music.push(MusicTrack {
                name,
                looping,
                volume,
            });
        }

        Ok(Self {
            max_sound_distance,
            sound_heap_size,
            bites,
            music,
            stream_fade_down_time,
            stream_bit_depth,
            stream_channels,
        })
    }

    pub fn num_bites(&self) -> usize {
        self.bites.len()
    }
}
