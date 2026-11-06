// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Sample banks: `sound.pak` (effects), `betty.pak`, `support.pak`, `noise.pak`.
//!
//! Port of the loading in `SoundSystem::init` / `preloadSoundBite` / `playBettySample` /
//! `playSupportSample` (`mclib/soundsys.cpp`) and the noise file open in `Radio::init` (`code/radio.cpp`).
//! Every packet is a RIFF/WAVE image (`wave_ParseWaveMemory` finds `fmt ` and `data`); here
//! [`Sound::from_wav`] decodes it, so the banks hold ready-to-mix 16-bit samples.
//!
//! A packet can be absent (index past the end of the file), empty (a `NUL` packet, as the unused
//! "No Longer Valid" effects are) or fail to parse. The C++ treats an empty packet as a bite with no sample:
//! the channel is claimed but nothing plays. The same happens here (the bite is [`Packet::Empty`]).

use crate::data::AudioData;
use crate::error::{AudioError, Result};
use crate::snd::SoundTable;
use mc2_formats::pak::PakFile;
use mc2_platform::audio::Sound;
use std::sync::Arc;

/// What one packet of a sound pak holds.
#[derive(Clone, Debug)]
pub enum Packet {
    /// The index is past the end of the pak (`seekPacket` fails).
    Absent,
    /// The packet exists but has no bytes (a `NUL` packet).
    Empty,
    /// The packet could not be read or is not a decodable WAV.
    Bad(String),
    /// A decoded sample.
    Sound(Arc<Sound>),
}

impl Packet {
    pub fn sound(&self) -> Option<&Arc<Sound>> {
        match self {
            Packet::Sound(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_sound(&self) -> bool {
        matches!(self, Packet::Sound(_))
    }
}

/// Read packet `index` of `pak` and decode it.
pub fn read_sound_packet(pak: &PakFile, index: usize) -> Packet {
    if index >= pak.packet_count() {
        return Packet::Absent;
    }
    match pak.read_packet(index) {
        Err(e) => Packet::Bad(e.to_string()),
        Ok(bytes) if bytes.is_empty() => Packet::Empty,
        Ok(bytes) => match Sound::from_wav(&bytes) {
            Ok(s) => Packet::Sound(s),
            Err(e) => Packet::Bad(format!("{e:?}")),
        },
    }
}

/// A fully decoded pak.
#[derive(Clone, Debug)]
pub struct PakSamples {
    packets: Vec<Packet>,
}

impl PakSamples {
    pub fn load(pak: &PakFile) -> Self {
        Self {
            packets: (0..pak.packet_count())
                .map(|i| read_sound_packet(pak, i))
                .collect(),
        }
    }

    /// Number of packets in the file (`getNumPackets`).
    pub fn len(&self) -> usize {
        self.packets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packets.is_empty()
    }

    pub fn packet(&self, index: usize) -> &Packet {
        self.packets.get(index).unwrap_or(&Packet::Absent)
    }

    pub fn sound(&self, index: usize) -> Option<&Arc<Sound>> {
        self.packet(index).sound()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Packet> {
        self.packets.iter()
    }

    /// Indices of the packets that hold no usable sample.
    pub fn missing(&self) -> Vec<usize> {
        self.packets
            .iter()
            .enumerate()
            .filter(|(_, p)| !p.is_sound())
            .map(|(i, _)| i)
            .collect()
    }
}

fn load_pak(data: &mut dyn AudioData, name: &str) -> Result<PakFile> {
    let path = data.paths().cd_sound(name, ".pak");
    let bytes = data.read(&path).ok_or(AudioError::Missing(path))?;
    Ok(PakFile::from_bytes(bytes)?)
}

/// Open a pak by sound name, e.g. `"pilota"` (used for the pilot voice sets as well).
pub fn open_sound_pak(data: &mut dyn AudioData, name: &str) -> Result<PakFile> {
    load_pak(data, name)
}

/// All shared sample banks of a mission.
#[derive(Clone, Debug)]
pub struct SoundBank {
    /// `sound.pak`, indexed by sound bite.
    pub effects: PakSamples,
    /// `betty.pak`.
    pub betty: PakSamples,
    /// `support.pak`.
    pub support: PakSamples,
    /// `noise.pak`: the radio static (`SHORT_STATIC`, `LONG_STATIC`, `CLICK`...).
    pub noise: PakSamples,
}

impl SoundBank {
    /// Load every bank. `sound_name` is the `.snd` base name the game passes to `init` (`"sound"`).
    pub fn load(data: &mut dyn AudioData, sound_name: &str) -> Result<Self> {
        Ok(Self {
            effects: PakSamples::load(&load_pak(data, sound_name)?),
            betty: PakSamples::load(&load_pak(data, "betty")?),
            support: PakSamples::load(&load_pak(data, "support")?),
            noise: PakSamples::load(&load_pak(data, "noise")?),
        })
    }

    /// Sample of sound bite `id`, if the bite is loaded.
    pub fn effect(&self, id: usize) -> Option<&Arc<Sound>> {
        self.effects.sound(id)
    }
}

/// Counts for the sound table against `sound.pak`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectCoverage {
    /// Bites in the table.
    pub bites: usize,
    /// Bites whose packet decodes to a sample.
    pub loadable: usize,
    /// Bites whose packet is empty (no sample, silent).
    pub empty: Vec<usize>,
    /// Bites whose packet is absent or fails to decode.
    pub broken: Vec<usize>,
}

impl SoundBank {
    /// Check every table entry resolves to a packet of `sound.pak`.
    pub fn effect_coverage(&self, table: &SoundTable) -> EffectCoverage {
        let mut c = EffectCoverage {
            bites: table.num_bites(),
            loadable: 0,
            empty: Vec::new(),
            broken: Vec::new(),
        };
        for i in 0..table.num_bites() {
            match self.effects.packet(i) {
                Packet::Sound(_) => c.loadable += 1,
                Packet::Empty => c.empty.push(i),
                _ => c.broken.push(i),
            }
        }
        c
    }
}
