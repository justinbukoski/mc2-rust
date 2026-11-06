//! Synthetic game data for tests and tools: a complete, tiny sound set built in memory.
//!
//! Signals are constant ("DC") levels, so a rendered buffer's amplitude is exactly
//! `sample * volume * pan gain`, which makes level and timing assertions exact. Not part of the game API.

use crate::data::{MemoryData, SoundPaths};
use mc2_formats::pak::{PacketPolicy, PakWriter};
use std::fmt::Write as _;

/// The shipped `radio.csv` (31 message rows), kept here so synthetic voice sets use the real table.
pub const RADIO_CSV: &str =
    "title,priority,shelflife,movie,styles,style1%,style2%,style3%,pilot id?,map to\r\n\
RADIO_ACK,5,3,x,3,33,33,34,n,0\r\n\
RADIO_CALLED_SHOT,3,3,x,1,0,0,0,y,3\r\n\
RADIO_CANNOT_CAPTURE,1,3,x,1,0,0,0,n,4\r\n\
RADIO_CAPTURED_BUILDING,1,3,x,1,0,0,0,n,5\r\n\
RADIO_CAPTURED_VEHICLE,1,3,x,1,0,0,0,n,39\r\n\
RADIO_REFIT_INCOMPLETE,2,3,x,1,0,0,0,y,6\r\n\
RADIO_REFIT_DONE,2,3,1,1,0,0,0,y,7\r\n\
RADIO_OBJECT_DEAD,3,3,x,1,0,0,0,n,8\r\n\
RADIO_BUILDING_DEAD,3,3,x,1,0,0,0,n,9\r\n\
RADIO_VEHICLE_DEAD,3,3,x,2,50,50,0,n,10\r\n\
RADIO_MECH_DEAD,3,3,x,2,50,50,0,n,12\r\n\
RADIO_MECH_SALVAGE,3,3,x,2,50,50,0,n,14\r\n\
RADIO_SENSOR_CONTACT,2,3,1,2,50,50,0,y,16\r\n\
RADIO_HITTING_MINES,1,3,x,1,0,0,0,y,18\r\n\
RADIO_UNDER_ATTACK,1,3,2,2,50,50,0,y,19\r\n\
RADIO_UNDER_AIRSTRIKE,1,3,x,1,0,0,0,n,21\r\n\
RADIO_MOVE_BLOCKED,2,3,x,1,0,0,0,y,22\r\n\
RADIO_ILLEGAL_ORDER,2,3,x,1,0,0,0,y,23\r\n\
RADIO_EJECTING,0,3,4,1,0,0,0,n,24\r\n\
RADIO_DEATH,0,3,x,1,0,0,0,n,25\r\n\
RADIO_CRIPPLED,1,3,x,1,0,0,0,n,26\r\n\
RADIO_PILOT_HURT,3,3,3,1,0,0,0,n,27\r\n\
RADIO_WEAPON_DOWN,1,3,x,1,0,0,0,y,28\r\n\
RADIO_WEAPONS_OUT,1,3,x,1,0,0,0,y,29\r\n\
RADIO_AMMO_OUT,1,3,1,1,0,0,0,y,30\r\n\
RADIO_TAUNT,4,3,x,3,33,33,34,n,31\r\n\
RADIO_GUARD,2,3,x,1,0,0,0,n,34\r\n\
RADIO_TARGET_ACK,4,3,1,2,50,50,0,y,35\r\n\
RADIO_CALLSIGN,5,3,x,2,50,50,0,n,37\r\n\
RADIO_AIRSTRIKE,1,3,x,1,0,0,0,y,39\r\n\
RADIO_ARMORHOLE,1,3,3,2,50,50,0,y,40\r\n";

/// A PCM16 mono WAV image.
pub fn wav_i16(rate: u32, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 2).to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    v
}

/// `secs` seconds of a constant level (`level` in 0..=1 of full scale).
pub fn dc(rate: u32, secs: f32, level: f32) -> Vec<i16> {
    vec![(level * 32768.0) as i16; (rate as f32 * secs).round() as usize]
}

struct Bite {
    samples: Option<Vec<i16>>,
    volume: f32,
}

struct Music {
    name: String,
    samples: Vec<i16>,
    looping: bool,
    volume: f32,
}

/// Builds a [`MemoryData`] holding `sound.snd`, the four banks, `radio.csv`, voice sets and music.
pub struct FixtureBuilder {
    rate: u32,
    bites: Vec<Bite>,
    betty: Vec<Option<Vec<i16>>>,
    support: Vec<Option<Vec<i16>>>,
    noise: Vec<Option<Vec<i16>>>,
    voices: Vec<(String, Vec<Option<Vec<i16>>>)>,
    music: Vec<Music>,
    files: Vec<(String, Vec<u8>)>,
    fade_time: f32,
    paths: SoundPaths,
}

impl FixtureBuilder {
    pub fn new(rate: u32) -> Self {
        Self {
            rate,
            bites: Vec::new(),
            betty: Vec::new(),
            support: Vec::new(),
            noise: Vec::new(),
            voices: Vec::new(),
            music: Vec::new(),
            files: Vec::new(),
            fade_time: 1.0,
            paths: SoundPaths::default(),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Next sound bite (index = number of bites so far). `None` samples make an empty packet.
    pub fn bite(mut self, samples: Option<Vec<i16>>, volume: f32) -> Self {
        self.bites.push(Bite { samples, volume });
        self
    }

    pub fn betty(mut self, samples: Option<Vec<i16>>) -> Self {
        self.betty.push(samples);
        self
    }

    pub fn support(mut self, samples: Option<Vec<i16>>) -> Self {
        self.support.push(samples);
        self
    }

    pub fn noise(mut self, samples: Option<Vec<i16>>) -> Self {
        self.noise.push(samples);
        self
    }

    /// A pilot voice pak; packet `i` is `packets[i]` (`None` = empty).
    pub fn voice(mut self, name: &str, packets: Vec<Option<Vec<i16>>>) -> Self {
        self.voices.push((name.to_string(), packets));
        self
    }

    /// A music entry (`DMSn`) and its `data/sound/<name>.wav`.
    pub fn music(mut self, name: &str, samples: Vec<i16>, looping: bool, volume: f32) -> Self {
        self.music.push(Music {
            name: name.to_string(),
            samples,
            looping,
            volume,
        });
        self
    }

    /// Extra file (e.g. a voice-over wav) in the `soundPath` directory.
    pub fn file(mut self, name: &str, bytes: Vec<u8>) -> Self {
        self.files.push((name.to_string(), bytes));
        self
    }

    /// Put the files under other `soundPath` / `CDsoundPath` directories.
    pub fn paths(mut self, paths: SoundPaths) -> Self {
        self.paths = paths;
        self
    }

    pub fn fade_time(mut self, secs: f32) -> Self {
        self.fade_time = secs;
        self
    }

    fn pak(&self, packets: &[Option<Vec<i16>>]) -> Vec<u8> {
        let mut w = PakWriter::new(packets.len());
        for (i, p) in packets.iter().enumerate() {
            if let Some(s) = p {
                w.write_packet(i, &wav_i16(self.rate, s), PacketPolicy::Raw)
                    .unwrap();
            }
        }
        w.finish()
    }

    pub fn build(self) -> MemoryData {
        let mut d = MemoryData::new().with_paths(self.paths.clone());
        let mut snd = String::from(
            "FITini\n\n[SoundSetup]\nul soundHeapSize = 1000000\nf MaxSoundDistance = 2800.0\n\n",
        );
        let _ = write!(snd, "[SoundBites]\nul numBites = {}\n\n", self.bites.len());
        for (i, b) in self.bites.iter().enumerate() {
            let _ = write!(
                snd,
                "[SoundBite{i}]\nul priority = 1\nul cache = 1\nul soundId = {i}\nul preload = 1\nf volume = {}\n//end\n\n",
                b.volume
            );
        }
        let _ = write!(
            snd,
            "[DigitalMusic]\nl NumDMS = {}\nf StreamFadeDownTime = {}\nul StreamBitDepth = 16\nul StreamChannels = 1\n\n",
            self.music.len(),
            self.fade_time
        );
        for (i, m) in self.music.iter().enumerate() {
            let _ = write!(
                snd,
                "st DMS{i} = \"{}\"\nb DMSLoop{i} = {}\nf DMSVolume{i} = {}\n\n",
                m.name,
                if m.looping { "t" } else { "f" },
                m.volume
            );
            d.insert(
                &self.paths.sound(&m.name, ".wav"),
                wav_i16(self.rate, &m.samples),
            );
        }
        snd.push_str("//end\n");
        d.insert(&self.paths.sound("sound", ".snd"), snd.into_bytes());
        let bites: Vec<Option<Vec<i16>>> = self.bites.iter().map(|b| b.samples.clone()).collect();
        d.insert(&self.paths.cd_sound("sound", ".pak"), self.pak(&bites));
        d.insert(&self.paths.cd_sound("betty", ".pak"), self.pak(&self.betty));
        d.insert(
            &self.paths.cd_sound("support", ".pak"),
            self.pak(&self.support),
        );
        d.insert(&self.paths.cd_sound("noise", ".pak"), self.pak(&self.noise));
        d.insert(
            &self.paths.sound("radio", ".csv"),
            RADIO_CSV.as_bytes().to_vec(),
        );
        for (name, packets) in &self.voices {
            d.insert(&self.paths.cd_sound(name, ".pak"), self.pak(packets));
        }
        for (name, bytes) in &self.files {
            d.insert(&self.paths.sound(name, ""), bytes.clone());
        }
        d
    }
}
