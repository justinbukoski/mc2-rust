// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Pilot radio chatter: message table, per-pilot voice sets, and the `Radio` object.
//!
//! Port of `code/radio.cpp` / `radio.h` / `dradio.h` and the radio-gating half of
//! `MechWarrior::radioMessage` (`code/warrior.cpp`). The playing side (queue, priorities, fragment
//! sequencing) is in [`crate::game`].
//!
//! A message is a short static burst from `noise.pak`, then (for messages where the pilot "identifies
//! himself") the callsign line, then the line itself, all from the pilot's own voice set
//! (`pilotAudio` in the warrior `.fit`, e.g. `pilota` -> `data/sound/pilota.pak`). `radio.csv` says, for each
//! of the 31 [`RadioMessageType`]s, its priority (0 is the most urgent), shelf life, video code, how many
//! recorded styles exist and their chances, whether the callsign is prefixed, and the first packet.

use crate::bank::{open_sound_pak, read_sound_packet, Packet};
use crate::data::AudioData;
use crate::error::{AudioError, Result};
use crate::game::{GameSoundSystem, MAX_FRAGMENTS};
use crate::rng::AudioRng;
use crate::voicepack::VoicePack;
use mc2_formats::pak::PakFile;
use mc2_platform::audio::Sound;
use std::sync::Arc;

/// `RADIO_MESSAGE_COUNT`.
pub const RADIO_MESSAGE_COUNT: usize = 31;

macro_rules! radio_messages {
    ($($variant:ident = $name:literal),+ $(,)?) => {
        /// `RadioMessageType` (`dradio.h`), in the order of the rows of `radio.csv`.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        pub enum RadioMessageType { $($variant),+ }

        impl RadioMessageType {
            /// Every message type in index order.
            pub const ALL: [RadioMessageType; RADIO_MESSAGE_COUNT] = [$(RadioMessageType::$variant),+];

            /// The C++ enumerator name.
            pub fn name(self) -> &'static str {
                match self { $(RadioMessageType::$variant => $name),+ }
            }
        }
    };
}

radio_messages! {
    Ack = "RADIO_ACK",
    CalledShot = "RADIO_CALLED_SHOT",
    CannotCapture = "RADIO_CANNOT_CAPTURE",
    CapturedBuilding = "RADIO_CAPTURED_BUILDING",
    CapturedVehicle = "RADIO_CAPTURED_VEHICLE",
    RefitIncomplete = "RADIO_REFIT_INCOMPLETE",
    RefitDone = "RADIO_REFIT_DONE",
    ObjectDead = "RADIO_OBJECT_DEAD",
    BuildingDead = "RADIO_BUILDING_DEAD",
    VehicleDead = "RADIO_VEHICLE_DEAD",
    MechDead = "RADIO_MECH_DEAD",
    MechSalvage = "RADIO_MECH_SALVAGE",
    SensorContact = "RADIO_SENSOR_CONTACT",
    HittingMines = "RADIO_HITTING_MINES",
    UnderAttack = "RADIO_UNDER_ATTACK",
    UnderAirstrike = "RADIO_UNDER_AIRSTRIKE",
    MoveBlocked = "RADIO_MOVE_BLOCKED",
    IllegalOrder = "RADIO_ILLEGAL_ORDER",
    Ejecting = "RADIO_EJECTING",
    Death = "RADIO_DEATH",
    Crippled = "RADIO_CRIPPLED",
    PilotHurt = "RADIO_PILOT_HURT",
    WeaponDown = "RADIO_WEAPON_DOWN",
    WeaponsOut = "RADIO_WEAPONS_OUT",
    AmmoOut = "RADIO_AMMO_OUT",
    Taunt = "RADIO_TAUNT",
    Guard = "RADIO_GUARD",
    TargetAck = "RADIO_TARGET_ACK",
    Callsign = "RADIO_CALLSIGN",
    Airstrike = "RADIO_AIRSTRIKE",
    ArmorHole = "RADIO_ARMORHOLE",
}

impl RadioMessageType {
    pub fn index(self) -> usize {
        self as usize
    }

    /// `None` for values outside `0..RADIO_MESSAGE_COUNT` (the C++ returns early for those).
    pub fn from_index(i: i64) -> Option<Self> {
        usize::try_from(i)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
    }
}

/// `RadioNoise`: packet index in `noise.pak`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioNoise {
    ShortStatic = 0,
    LongStatic = 1,
    Click = 2,
}

/// One row of `radio.csv` (`RadioMessageInfo`).
#[derive(Clone, Debug, PartialEq)]
pub struct RadioMessageInfo {
    /// 0 is most urgent.
    pub priority: u8,
    /// Seconds a queued message stays valid. Read by the game; nothing consumes it
    /// (`expirationDate` is stored in the message and never compared).
    pub shelf_life: f32,
    /// Pilot-video code (`'1'`...), 0 for none (`x` in the file).
    pub movie_code: u8,
    /// Recorded variants of the line.
    pub style_count: u8,
    /// Percentage chance of each variant.
    pub style_chance: [u8; 3],
    /// Packet index of style 0 in the pilot's voice pak.
    pub message_mapping: u32,
    /// The callsign line is prefixed.
    pub pilot_identifies_self: bool,
    pub kludge_style: bool,
}

/// `atoi`.
fn atoi(s: &str) -> i64 {
    let s = s.trim_start();
    let (neg, rest) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let v: i64 = digits.parse().unwrap_or(0);
    if neg {
        -v
    } else {
        v
    }
}

/// `radio.csv`, loaded once (`Radio::loadMessageInfo`).
#[derive(Clone, Debug)]
pub struct RadioMessageTable {
    info: Vec<RadioMessageInfo>,
}

impl RadioMessageTable {
    /// Parse the table. Fields are split like `strtok(line, ",")`, which skips empty fields, and missing
    /// fields take the C++ defaults (priority 4, shelf life 0, no video, 1 style, 0 %).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let text = String::from_utf8_lossy(bytes);
        let mut lines = text.split('\n').map(|l| l.trim_end_matches('\r'));
        lines.next(); // title line
        let mut info = Vec::with_capacity(RADIO_MESSAGE_COUNT);
        for i in 0..RADIO_MESSAGE_COUNT {
            let line = lines.next().filter(|l| !l.is_empty()).ok_or_else(|| {
                AudioError::Missing(format!("radio.csv row {} of {RADIO_MESSAGE_COUNT}", i + 1))
            })?;
            let mut fields = line.split(',').filter(|f| !f.is_empty());
            fields.next(); // command name
            let priority = fields.next().map_or(4, |f| atoi(f) as u8);
            let shelf_life = fields.next().map_or(0.0, |f| atoi(f) as f32);
            let movie_code = match fields.next() {
                Some(f) if !f.starts_with('x') => f.as_bytes()[0],
                _ => 0,
            };
            let style_count = fields.next().map_or(1, |f| atoi(f) as u8);
            let mut style_chance = [0u8; 3];
            for c in style_chance.iter_mut() {
                *c = fields.next().map_or(0, |f| atoi(f) as u8);
            }
            let pilot_identifies_self = fields.next().is_some_and(|f| f.starts_with('y'));
            let message_mapping = fields.next().map_or(0, |f| atoi(f) as u32);
            // The C++ reads one more field and then another before looking at 'x' (a column the shipped
            // file does not have), so `kludgeStyle` is false for the shipped data.
            let kludge_style = match fields.next() {
                Some(_) => fields.next().is_some_and(|f| f.starts_with('x')),
                None => false,
            };
            info.push(RadioMessageInfo {
                priority,
                shelf_life,
                movie_code,
                style_count,
                style_chance,
                message_mapping,
                pilot_identifies_self,
                kludge_style,
            });
        }
        Ok(Self { info })
    }

    /// Load `data/sound/radio.csv`.
    pub fn load(data: &mut dyn AudioData) -> Result<Self> {
        let path = data.paths().sound("radio", ".csv");
        let bytes = data.read(&path).ok_or(AudioError::Missing(path))?;
        Self::from_bytes(&bytes)
    }

    pub fn get(&self, msg: RadioMessageType) -> &RadioMessageInfo {
        &self.info[msg.index()]
    }

    pub fn iter(&self) -> impl Iterator<Item = (RadioMessageType, &RadioMessageInfo)> {
        RadioMessageType::ALL.into_iter().zip(self.info.iter())
    }
}

/// A pilot (the game's `MechWarrior`) as the audio layer knows it: an opaque handle chosen by the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PilotId(pub u32);

/// A pilot's voice recordings: `data/sound/<pilotAudio>.pak`.
pub struct VoiceSet {
    name: String,
    pak: PakFile,
    pack: Option<Arc<VoicePack>>,
}

impl VoiceSet {
    pub fn open(data: &mut dyn AudioData, name: &str) -> Result<Self> {
        Self::open_with_pack(data, name, None)
    }

    /// As [`open`](Self::open), with an optional voice pack whose clips replace single packets.
    pub fn open_with_pack(
        data: &mut dyn AudioData,
        name: &str,
        pack: Option<Arc<VoicePack>>,
    ) -> Result<Self> {
        Ok(Self {
            name: name.to_string(),
            pak: open_sound_pak(data, name)?,
            pack,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn packet_count(&self) -> usize {
        self.pak.packet_count()
    }

    /// Read and decode a packet (the C++ reads it from the packet file when the message is created).
    /// A voice-pack clip for `(pak, index)` replaces the packet when the pak has that packet; an index past
    /// the end of the pak stays absent, so the radio logic around missing lines is unchanged.
    pub fn packet(&self, index: usize) -> Packet {
        if index < self.pak.packet_count() {
            if let Some(clip) = self.pack.as_ref().and_then(|p| p.clip(&self.name, index)) {
                return Packet::Sound(clip);
            }
        }
        read_sound_packet(&self.pak, index)
    }
}

/// A message's static burst: either loaded, or "present but failed to load" (the C++ marks that with
/// `(MemoryPtr)-1`: the noise stage still happens, silently).
#[derive(Clone, Debug)]
pub enum Noise {
    Sound(Arc<Sound>),
    Failed,
}

/// `RadioData`: one queued or playing message.
pub struct RadioData {
    /// Packet index in the voice pak (`messageMapping + style`); also the dedupe key.
    pub msg_id: u32,
    pub msg_type: RadioMessageType,
    /// Packet in `noise.pak`.
    pub noise_id: u32,
    /// Fragments to speak in order: the callsign (if any), then the line.
    pub data: [Option<Arc<Sound>>; MAX_FRAGMENTS],
    /// Static before each fragment; only index 0 is ever filled.
    pub noise: [Option<Noise>; MAX_FRAGMENTS],
    pub turn_queued: u32,
    /// 0 is the most urgent.
    pub priority: u8,
    pub movie_code: u8,
    pub expiration_date: f32,
    pub pilot: PilotId,
}

impl RadioData {
    /// Number of spoken fragments (callsign plus line).
    pub fn fragment_count(&self) -> usize {
        self.data.iter().filter(|d| d.is_some()).count()
    }
}

/// `MechWarrior::radioLog`: what the pilot said last, and the throttles on repeating it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RadioLog {
    /// Packet id of the last line (`DWORD`; `u32::MAX` is the C++ `-1`).
    pub last_message: u32,
    pub last_message_type: Option<RadioMessageType>,
    pub last_message_time: f32,
    pub last_under_fire: f32,
    pub last_contact: f32,
    pub weapons_out: bool,
    pub weapons_ineffective: bool,
}

impl Default for RadioLog {
    fn default() -> Self {
        Self {
            last_message: u32::MAX,
            last_message_type: None,
            last_message_time: -1000.0,
            last_under_fire: -1000.0,
            last_contact: -1000.0,
            weapons_out: false,
            weapons_ineffective: false,
        }
    }
}

/// Mission time as the radio sees it: `turn` (the frame counter, `turn`/`GetMissionTurn()`) and
/// `scenario_time` in seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RadioClock {
    pub turn: u32,
    pub scenario_time: f32,
}

/// What the simulation tells the radio when a pilot wants to speak.
#[derive(Clone, Copy, Debug)]
pub struct RadioRequest {
    /// `RadioMessageType` index; out of range or -1 is ignored.
    pub message: i64,
    /// `MechWarrior::active()`.
    pub pilot_active: bool,
    /// `underHomeCommand()`: the pilot belongs to the player's own force. Only those pilots ever speak;
    /// for everyone else the request is ignored.
    pub under_home_command: bool,
}

/// `Radio`: one pilot's radio.
pub struct PilotRadio {
    owner: PilotId,
    enabled: bool,
    ammo_out_played: bool,
    voice: Arc<VoiceSet>,
    table: Arc<RadioMessageTable>,
    pub log: RadioLog,
}

impl PilotRadio {
    pub fn new(owner: PilotId, voice: Arc<VoiceSet>, table: Arc<RadioMessageTable>) -> Self {
        Self {
            owner,
            enabled: true,
            ammo_out_played: false,
            voice,
            table,
            log: RadioLog::default(),
        }
    }

    pub fn owner(&self) -> PilotId {
        self.owner
    }

    pub fn voice(&self) -> &Arc<VoiceSet> {
        &self.voice
    }

    pub fn turn_on(&mut self) {
        self.enabled = true;
    }

    pub fn turn_off(&mut self) {
        self.enabled = false;
    }

    pub fn reset_ammo_message(&mut self) {
        self.ammo_out_played = false;
    }

    /// `MechWarrior::radioMessage`: throttle, then [`play_message`](Self::play_message).
    pub fn radio_message(
        &mut self,
        sound: &mut GameSoundSystem,
        req: RadioRequest,
        clock: RadioClock,
        rng: &mut dyn AudioRng,
    ) {
        let Some(message) = RadioMessageType::from_index(req.message) else {
            return;
        };
        // `if (radio && active() && message != -1 && GetMissionTurn() > 0) { if (underHomeCommand()) {...} }`:
        // only the player's own pilots ever speak ("according to Derek this never happens" for the others).
        if !(req.pilot_active && clock.turn > 0 && req.under_home_command) {
            return;
        }
        let t = f64::from(clock.scenario_time);
        match message {
            RadioMessageType::WeaponsOut => {
                if self.log.weapons_out {
                    return;
                }
                self.log.weapons_out = true;
            }
            RadioMessageType::SensorContact => {
                if f64::from(self.log.last_contact) > t - 15.0 {
                    return;
                }
                self.log.last_contact = clock.scenario_time;
            }
            RadioMessageType::UnderAttack => {
                if f64::from(self.log.last_under_fire) > t - 20.0 {
                    return;
                }
                self.log.last_under_fire = clock.scenario_time;
            }
            _ => {}
        }
        let limit_frequency = !matches!(
            message,
            RadioMessageType::TargetAck
                | RadioMessageType::MoveBlocked
                | RadioMessageType::IllegalOrder
        );
        if limit_frequency
            && self.log.last_message_type == Some(message)
            && f64::from(self.log.last_message_time) > t - 10.0
        {
            return;
        }
        self.log.last_message_time = clock.scenario_time;
        self.log.last_message = self
            .play_message(sound, message, clock, rng)
            .unwrap_or(u32::MAX);
        self.log.last_message_type = Some(message);
    }

    /// `Radio::playMessage`: pick a style, load the fragments and queue the message. Returns the packet
    /// id of the line (`msgData->msgId`), or `None` for `NO_PLAY`.
    ///
    /// Random draws, in order: one `random_number(100)` when the message has several styles; one more
    /// when the message is prefixed by the callsign and `RADIO_CALLSIGN` has several styles (the C++
    /// draws it and then always uses style 0).
    pub fn play_message(
        &mut self,
        sound: &mut GameSoundSystem,
        msg_type: RadioMessageType,
        clock: RadioClock,
        rng: &mut dyn AudioRng,
    ) -> Option<u32> {
        if !sound.sys.use_sound || !self.enabled {
            return None;
        }
        let info = self.table.get(msg_type).clone();
        if !sound.check_message(self.owner, info.priority, msg_type) {
            return None;
        }
        if msg_type == RadioMessageType::AmmoOut && self.ammo_out_played {
            return None;
        }

        let mut drop_out = 0i32;
        let style = if info.style_count > 1 {
            let roll = rng.random_number(100);
            let mut i = 0u8;
            while i < info.style_count {
                drop_out += i32::from(info.style_chance.get(usize::from(i)).copied().unwrap_or(0));
                if roll < drop_out {
                    break;
                }
                i += 1;
            }
            if i != 0 && i == info.style_count {
                return None;
            }
            if info.message_mapping.wrapping_add(u32::from(i)) == self.log.last_message {
                i += 1;
            }
            if i >= info.style_count {
                i = 0;
            }
            i
        } else {
            0
        };

        let mut msg = RadioData {
            msg_id: info.message_mapping + u32::from(style),
            msg_type,
            noise_id: RadioNoise::ShortStatic as u32,
            data: Default::default(),
            noise: Default::default(),
            turn_queued: clock.turn,
            priority: info.priority,
            movie_code: info.movie_code,
            expiration_date: clock.scenario_time + info.shelf_life,
            pilot: self.owner,
        };

        // The callsign. The C++ rolls for a style and then uses style 0 whatever the roll was.
        let mut callsign = 0u32;
        if info.pilot_identifies_self {
            let cs = self.table.get(RadioMessageType::Callsign);
            if cs.style_count > 1 {
                let roll = rng.random_number(100);
                let mut i = 0u8;
                while i < cs.style_count {
                    drop_out +=
                        i32::from(cs.style_chance.get(usize::from(i)).copied().unwrap_or(0));
                    if roll < drop_out {
                        break;
                    }
                    i += 1;
                }
            }
            callsign += cs.message_mapping;
        }

        let mut fragment = 0usize;
        if callsign != 0 {
            // Empty or undecodable packets are skipped (the C++ would dereference a null format).
            if let Packet::Sound(s) = self.voice.packet(callsign as usize) {
                msg.data[fragment] = Some(s);
                fragment += 1;
            }
        }

        let main = self.voice.packet(msg.msg_id as usize);
        if !matches!(main, Packet::Absent) {
            if let Packet::Sound(s) = main {
                msg.data[fragment] = Some(s);
            }
            msg.noise[0] = match sound.sys.bank().noise.packet(msg.noise_id as usize) {
                Packet::Absent => None,
                Packet::Sound(s) => Some(Noise::Sound(Arc::clone(s))),
                Packet::Empty | Packet::Bad(_) => Some(Noise::Failed),
            };
        }

        let msg_id = msg.msg_id;
        sound.queue_radio_message(Box::new(msg)).ok()?;
        if msg_type == RadioMessageType::AmmoOut {
            self.ammo_out_played = true;
        }
        Some(msg_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str =
        "title,priority,shelflife,movie,styles,style1%,style2%,style3%,pilot id?,map to\r\n\
RADIO_ACK,5,3,x,3,33,33,34,n,0\r\n";

    #[test]
    fn atoi_like_c() {
        assert_eq!(atoi("33"), 33);
        assert_eq!(atoi("  12abc"), 12);
        assert_eq!(atoi("abc"), 0);
        assert_eq!(atoi("-4"), -4);
    }

    #[test]
    fn message_names_and_indices() {
        assert_eq!(RadioMessageType::Ack.index(), 0);
        assert_eq!(RadioMessageType::ArmorHole.index(), RADIO_MESSAGE_COUNT - 1);
        assert_eq!(RadioMessageType::Callsign.name(), "RADIO_CALLSIGN");
        assert_eq!(RadioMessageType::from_index(31), None);
        assert_eq!(RadioMessageType::from_index(-1), None);
        assert_eq!(
            RadioMessageType::from_index(18),
            Some(RadioMessageType::Ejecting)
        );
    }

    #[test]
    fn short_table_is_an_error() {
        assert!(RadioMessageTable::from_bytes(CSV.as_bytes()).is_err());
    }
}
