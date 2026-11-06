// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Sound, radio, Betty and music for the game: `mc2-audio`'s [`GameAudio`] driven from the
//! simulation frames.
//!
//! Per simulation frame the app hands the audio side what `Mission::update` / the sound system
//! read in the original: the listener (`eye->getPosition()`, `cameraRotation`), the mission clock
//! (`turn`, `scenarioTime`) and the frame length (`GameSoundSystem::update`), plus the frame's
//! events translated by an [`AudioBridge`].  The mixer is pulled by the output device in a window
//! (`mc2_platform::audio::AudioOutput`), or rendered into a buffer headless
//! ([`GameAudio::render_offline`]); headless runs never open an audio device.
//!
//! The `SimEvent` → [`AudioEvent`] mapping lives in the bridge P2 writes next to the `mc2-sim`
//! adapter; the variants the simulation needs to emit for it are listed on [`AudioBridge`].

use crate::audio::SharedGameAudio;
use crate::sim::{FrameContext, SimDriver, SimEventSink};
use mc2_audio::{AudioEvent, GameAudio, Listener, PilotId, RadioClock, Vec3 as AVec3, Volumes};
use mc2_platform::audio::{Mixer, SharedMixer};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

/// Output sample rate of headless rendering.
pub const OFFLINE_SAMPLE_RATE: u32 = 44_100;

/// Translates a simulation's events into audio requests.
///
/// The simulation-side events this needs (proposed `mc2_sim::events::SimEvent` variants, one per
/// C++ call site class):
/// * `Sound { id, at: Option<[f32; 3]>, allow_dupes }`: every `soundSystem->playDigitalSample`
///   reached from sim code (weapon fire and hits, explosions, footsteps, building collapse,
///   turret pop-up, gate open), with the world position the C++ passes;
/// * `Radio { pilot, message, under_home_command }`: `MechWarrior::radioMessage` (message is the
///   `RadioMessageType` index; the pilot's voice set comes from the pilot record);
/// * `Betty(id)` / `StopBetty`: `playBettySample` (objectives, unit lost, reinforcements);
/// * `Support(id)`, `VoiceOver(name)`, `StopSupport`: `playSupportSample` (ABL `playSpeech`,
///   tutorial voice-overs);
/// * `Music(tune)` / `StopMusic`: ABL `playMusic`/`stopMusic` and the win/lose tunes of
///   `Mission::update` (`WIN_TUNE_0`, `LOSE_TUNE_0`);
/// * `Rain(sound id)`, `GeneralAlarm(bool)`, `SensorContact(level)`: the sound system's state
///   flags the sim sets (`setIsRaining`, `GeneralAlarm`, `largestSensorContact`);
/// * `PilotsChanged`: a pilot joined or left the player's force (reinforcements, ejection), so the
///   app can call [`Self::pilots`] again.
pub trait AudioBridge<D: SimDriver> {
    /// Appends the audio requests of one event.
    fn translate(&mut self, event: &D::Event, world: &D::World, out: &mut Vec<AudioEvent>);
    /// A number in `0..range` from the simulation's *visual* random stream (the radio's style
    /// rolls; DESIGN.md section 4).
    fn random_number(&mut self, world: &D::World, range: i32) -> i32;
    /// Pilots that get a radio, with their voice set (`pilotAudio`, e.g. `"pilota"`) and whether
    /// they are active.
    fn pilots(&mut self, _world: &D::World) -> Vec<(PilotId, String, bool)> {
        Vec::new()
    }
    /// `scenarioTime` of the last frame (the radio's clock).
    fn scenario_time(&self, world: &D::World) -> Option<f32>;
    /// Mission tune selected by the loaded scenario, if the driver has one.
    fn mission_tune_id(&self, _world: &D::World) -> Option<i32> {
        None
    }
    /// Whether this event changes the pilot-radio roster.
    fn pilots_changed(&self, _event: &D::Event) -> bool {
        false
    }
}

/// The bridge of the null simulation: no events, a fixed-seed visual stream.
#[derive(Debug, Default)]
pub struct NullAudioBridge {
    seed: u64,
}

impl<D: SimDriver> AudioBridge<D> for NullAudioBridge {
    fn translate(&mut self, _event: &D::Event, _world: &D::World, _out: &mut Vec<AudioEvent>) {}
    fn random_number(&mut self, _world: &D::World, range: i32) -> i32 {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.seed >> 33) as i32).rem_euclid(range.max(1))
    }
    fn scenario_time(&self, _world: &D::World) -> Option<f32> {
        None
    }
}

/// `BETTY_MISSION_LOST` (`mclib/sounds.h:181`).
pub const BETTY_MISSION_LOST: u32 = 11;
/// `BETTY_MISSION_WON` (`mclib/sounds.h:188`).
pub const BETTY_MISSION_WON: u32 = 18;

/// What one [`mc2_sim::SimEvent`] asks of the sound system, appended to `out`.
///
/// Every sound-system call the C++ makes from sim code arrives as its own event, with the arguments the call
/// had (so the ids and positions are the C++'s). Events that are not sound at all (effects, camera, AI and
/// objective reports) map to nothing.
pub fn map_event(event: &mc2_sim::SimEvent, out: &mut Vec<AudioEvent>) {
    use mc2_sim::SimEvent as S;
    match event {
        // `soundSystem->playDigitalSample(id, position, allowDupes)`; no position is the centred default.
        S::Sound {
            id,
            at,
            allow_dupes,
            ..
        } => {
            // A negative id is not a sample (`playDigitalSample` ignores ids it has no sound for).
            if let Ok(id) = u32::try_from(*id) {
                out.push(AudioEvent::Sound {
                    id,
                    at: at.map(|p| AVec3::new(p.x, p.y, p.z)),
                    allow_dupes: *allow_dupes,
                });
            }
        }
        // `MechWarrior::radioMessage` (`code/warrior.cpp:1304`); the pilot's radio and its rate limits are in `mc2-audio`.
        S::Radio {
            pilot,
            message,
            under_home_command,
        } => out.push(AudioEvent::Radio {
            pilot: PilotId(*pilot),
            message: i64::from(*message),
            under_home_command: *under_home_command,
        }),
        S::Betty(id) => out.push(AudioEvent::Betty(*id)),
        S::StopBetty => out.push(AudioEvent::StopBetty),
        S::Support(id) => out.push(AudioEvent::Support(*id)),
        S::VoiceOver(name) => out.push(AudioEvent::VoiceOver(name.clone())),
        S::StopSupport => out.push(AudioEvent::StopSupport),
        S::Music(id) => out.push(AudioEvent::Music(*id)),
        S::StopMusic => out.push(AudioEvent::StopMusic),
        S::Rain(id) => out.push(AudioEvent::Rain(*id)),
        S::GeneralAlarm(on) => out.push(AudioEvent::GeneralAlarm(*on)),
        S::SensorContact(level) => out.push(AudioEvent::SensorContact(*level)),
        // `setradio` (`code/ablmc2.cpp:2672-2701`): `Radio::turnOn` / `turnOff` on the pilot's radio.
        S::RadioPower { pilot, on } => out.push(AudioEvent::RadioPower {
            pilot: PilotId(*pilot),
            on: *on,
        }),
        // `GameApp` plays Betty when `mission->update()` returns a result (`code/mechcmd2.cpp:2160-2164`): lost for
        // `result < mis_PLAYER_DRAW`, won for `result > mis_PLAYER_DRAW`, nothing for a draw. The win and lose tunes of
        // `Mission::update` (`code/mission.cpp:671-709`) are `#ifndef FINAL` only, so the shipped game plays none; the
        // mission script's own `playDigitalMusic` arrives as `Music`.
        S::MissionResult(r) => {
            use mc2_sim::mission::result;
            if *r != result::PLAYING {
                match (*r).cmp(&result::DRAW) {
                    std::cmp::Ordering::Less => out.push(AudioEvent::Betty(BETTY_MISSION_LOST)),
                    std::cmp::Ordering::Greater => out.push(AudioEvent::Betty(BETTY_MISSION_WON)),
                    std::cmp::Ordering::Equal => {}
                }
            }
        }
        // `soundSystem->playDigitalSample(INVALID_GUI)` of a click `canAddVehicle` / `canRecover` refused (missiongui.cpp:1336)
        S::VtolRefused { .. } => out.push(AudioEvent::Sound {
            id: mc2_audio::ids::sfx::INVALID_GUI,
            at: None,
            allow_dupes: false,
        }),
        S::Explosion {
            sound_id: Some(id),
            at,
            ..
        } => out.push(AudioEvent::Sound {
            id: *id as u32,
            at: Some(AVec3::new(at.x, at.y, at.z)),
            allow_dupes: true,
        }),
        S::PilotsChanged
        | S::SupportVehicleDropped { .. }
        | S::VtolFinished { .. }
        | S::WeaponEffect { .. }
        | S::Explosion { sound_id: None, .. }
        | S::Crater { .. }
        | S::AppearanceFx { .. }
        | S::Flash { .. }
        | S::Script { .. }
        | S::Camera(_)
        | S::AiUnavailable { .. }
        | S::MovementUnavailable { .. }
        | S::ObjectiveAction { .. }
        | S::PlayVideo { .. }
        | S::TutorialText { .. }
        | S::AirstrikeFlyover { .. }
        | S::AirstrikeContrail { .. } => {}
    }
}

/// Audio mapping for the `mc2-sim` event contract.
#[derive(Debug, Default)]
pub struct SimAudioBridge;

impl<D> AudioBridge<D> for SimAudioBridge
where
    D: SimDriver<Event = mc2_sim::SimEvent, World = mc2_sim::World>,
{
    fn translate(
        &mut self,
        event: &mc2_sim::SimEvent,
        _world: &mc2_sim::World,
        out: &mut Vec<AudioEvent>,
    ) {
        map_event(event, out);
    }

    fn random_number(&mut self, world: &mc2_sim::World, range: i32) -> i32 {
        world.rng.0.borrow_mut().visual.random_number(range)
    }

    fn scenario_time(&self, world: &mc2_sim::World) -> Option<f32> {
        Some(world.clock.scenario_time)
    }

    fn mission_tune_id(&self, world: &mc2_sim::World) -> Option<i32> {
        Some(i32::from(world.mission.tune_num))
    }

    fn pilots(&mut self, world: &mc2_sim::World) -> Vec<(PilotId, String, bool)> {
        world
            .mission
            .pilot_loads
            .iter()
            .enumerate()
            .skip(1)
            .filter_map(|(slot, load)| {
                if !load.used {
                    return None;
                }
                let voice = load.profile.as_ref()?.general.pilot_audio.clone()?;
                let pilot = world.pilots.slots.get(slot)?;
                // `MechWarrior::active()` is `status == WARRIOR_STATUS_NORMAL` (warrior.h:1100); death and ejection
                // (warrior.cpp:1982-1993, 2020-2041) leave it, whatever state the mover is in.
                let active = pilot.status == mc2_sim::mission::outcome::WarriorStatus::Normal;
                Some((PilotId(slot as u32), voice, active))
            })
            .collect()
    }

    fn pilots_changed(&self, event: &mc2_sim::SimEvent) -> bool {
        matches!(event, mc2_sim::SimEvent::PilotsChanged)
    }
}

/// Where the mixed audio goes.
pub enum AudioSinkOutput {
    /// A device pulls the mixer (windowed).
    Device,
    /// Rendered per frame into `samples` (interleaved stereo f32 at the mixer's rate).
    Offline { samples: Vec<f32> },
    /// The sink drives a `GameAudio` that the app owns ([`AudioSink::shared`]); the app updates and mixes it once a
    /// frame, so the sink only hands it the frame's requests.
    Shared,
}

/// The audio side of the game as a [`SimEventSink`].
pub struct AudioSink<D: SimDriver> {
    pub audio: SharedGameAudio,
    bridge: Box<dyn AudioBridge<D>>,
    output: AudioSinkOutput,
    scratch: Vec<AudioEvent>,
    registered_pilots: HashSet<PilotId>,
    /// Events handled so far (diagnostics).
    pub handled: u64,
    /// Every request handed to the audio side, in order (tests and diagnostics read it; it is not
    /// cleared, so a long run should take it with [`Self::take_log`]).
    log: Vec<AudioEvent>,
}

impl<D: SimDriver> AudioSink<D> {
    /// Loads the sound data (`data/sound`, banks, `radio.csv`) through a new VFS on `data_root`.
    pub fn load(
        data_root: &Path,
        mixer: SharedMixer,
        output: AudioSinkOutput,
        bridge: Box<dyn AudioBridge<D>>,
    ) -> Result<Self, String> {
        let vfs = mc2_formats::vfs::Vfs::from_system_cfg(data_root).map_err(|e| e.to_string())?;
        // Loose music streams from disk in a window; an offline render decodes it whole, deterministically.
        let stream_loose_files = matches!(output, AudioSinkOutput::Device);
        let data = mc2_audio::data::VfsData::new(vfs, stream_loose_files);
        let audio = GameAudio::load(Box::new(data), mixer, Volumes::default())
            .map_err(|e| e.to_string())?;
        Ok(Self::new(Rc::new(RefCell::new(audio)), bridge, output))
    }

    /// A sink over the app's own `GameAudio` (the production wiring): two `GameAudio`s on one mixer would each
    /// believe they own its channels.  The app updates and mixes the audio, the sink only feeds it.
    pub fn shared(audio: SharedGameAudio, bridge: Box<dyn AudioBridge<D>>) -> Self {
        Self::new(audio, bridge, AudioSinkOutput::Shared)
    }

    fn new(
        audio: SharedGameAudio,
        bridge: Box<dyn AudioBridge<D>>,
        output: AudioSinkOutput,
    ) -> Self {
        Self {
            audio,
            bridge,
            output,
            scratch: Vec::new(),
            registered_pilots: HashSet::new(),
            handled: 0,
            log: Vec::new(),
        }
    }

    /// Headless: an offline mixer at [`OFFLINE_SAMPLE_RATE`].
    pub fn offline(data_root: &Path, bridge: Box<dyn AudioBridge<D>>) -> Result<Self, String> {
        let mixer = SharedMixer::new(Mixer::new(
            OFFLINE_SAMPLE_RATE,
            mc2_platform::audio::mixer::GAME_CHANNELS,
        ));
        Self::load(
            data_root,
            mixer,
            AudioSinkOutput::Offline {
                samples: Vec::new(),
            },
            bridge,
        )
    }

    /// The requests handled since the last call, oldest first.
    pub fn take_log(&mut self) -> Vec<AudioEvent> {
        std::mem::take(&mut self.log)
    }

    /// The rendered samples of a headless run (interleaved stereo).
    pub fn offline_samples(&self) -> Option<&[f32]> {
        match &self.output {
            AudioSinkOutput::Offline { samples } => Some(samples),
            AudioSinkOutput::Device | AudioSinkOutput::Shared => None,
        }
    }
}

impl<D: SimDriver> SimEventSink<D> for AudioSink<D> {
    fn mission_start(&mut self, world: &D::World) {
        if let Some(crate::audio::Cue::Music(id)) = self
            .bridge
            .mission_tune_id(world)
            .and_then(crate::audio::tune::mission_start_tune)
        {
            let event = AudioEvent::Music(id);
            let mut rng = |range| self.bridge.random_number(world, range);
            self.audio.borrow_mut().handle(&event, &mut rng);
            self.log.push(event);
            self.handled += 1;
        }
        self.refresh_pilots(world);
    }

    fn mission_end(&mut self) {
        let mut rng = |_: i32| 0;
        self.audio.borrow_mut().handle(&AudioEvent::Purge, &mut rng);
    }

    fn on_frame(&mut self, ctx: &FrameContext<'_, D>, events: &[D::Event]) {
        if events.iter().any(|e| self.bridge.pilots_changed(e)) {
            self.refresh_pilots(ctx.world);
        }
        let mut audio = self.audio.borrow_mut();
        audio.set_listener(Some(Listener {
            position: AVec3::new(ctx.listener.x, ctx.listener.y, ctx.listener.z),
            rotation_y: ctx.listener_rotation,
        }));
        let scenario_time = self
            .bridge
            .scenario_time(ctx.world)
            .unwrap_or(ctx.scenario_time);
        audio.set_clock(RadioClock {
            turn: ctx.turn.max(0) as u32,
            scenario_time,
        });
        self.scratch.clear();
        for e in events {
            self.bridge.translate(e, ctx.world, &mut self.scratch);
        }
        let bridge = &mut self.bridge;
        let world = ctx.world;
        let mut rng = |range: i32| bridge.random_number(world, range);
        for a in &self.scratch {
            audio.handle(a, &mut rng);
            self.handled += 1;
            self.log.push(a.clone());
        }
        drop(audio);
        // The pilot's death/ejection line (`radioMessage` runs before `status` changes, warrior.cpp:1981, 2017) was
        // handled above with last frame's flags; from here on the pilot is inactive and later or queued chatter drops.
        self.sync_pilot_activity(ctx.world);
        match &mut self.output {
            AudioSinkOutput::Device => self.audio.borrow_mut().update(ctx.frame_length),
            AudioSinkOutput::Offline { samples } => self
                .audio
                .borrow_mut()
                .render_offline(ctx.frame_length, samples),
            AudioSinkOutput::Shared => {}
        }
    }
}

impl<D: SimDriver> AudioSink<D> {
    fn sync_pilot_activity(&mut self, world: &D::World) {
        if self.registered_pilots.is_empty() {
            return;
        }
        for (pilot, _, active) in self.bridge.pilots(world) {
            if self.registered_pilots.contains(&pilot) {
                self.audio.borrow_mut().set_pilot_active(pilot, active);
            }
        }
    }

    fn refresh_pilots(&mut self, world: &D::World) {
        for pilot in self.registered_pilots.drain() {
            self.audio.borrow_mut().remove_pilot(pilot);
        }
        for (pilot, voice, active) in self.bridge.pilots(world) {
            self.audio.borrow_mut().add_pilot(pilot, &voice);
            self.audio.borrow_mut().set_pilot_active(pilot, active);
            self.registered_pilots.insert(pilot);
        }
    }
}

/// A shared handle on an [`AudioSink`]: the game owns one clone as its sink, the caller keeps another to read
/// what the sink did.
pub type SharedAudioSink<D> = Rc<RefCell<AudioSink<D>>>;

/// What [`sim_audio_sink`] returns: the sink to push into `GameCore.sinks`, and the handle that reads it back.
pub type BattleSink<D> = (Box<dyn SimEventSink<D>>, SharedAudioSink<D>);

struct SinkHandle<D: SimDriver>(SharedAudioSink<D>);

impl<D: SimDriver> SimEventSink<D> for SinkHandle<D> {
    fn mission_start(&mut self, world: &D::World) {
        self.0.borrow_mut().mission_start(world);
    }

    fn mission_end(&mut self) {
        self.0.borrow_mut().mission_end();
    }

    fn on_frame(&mut self, ctx: &FrameContext<'_, D>, events: &[D::Event]) {
        self.0.borrow_mut().on_frame(ctx, events);
    }
}

/// The battle sound sink for the `mc2-sim` events: weapon sounds, radio, Betty, support voices, music, alarm and
/// sensor state, mapped by [`map_event`]. Push the box into `GameCore.sinks`; the handle reads the sink back.
///
/// Sound is positional from the sim's event position and the frame's listener (the camera). `data_root` is the
/// game data root (`system.cfg`).
pub fn sim_audio_sink<D>(
    data_root: &Path,
    mixer: SharedMixer,
    output: AudioSinkOutput,
) -> Result<BattleSink<D>, String>
where
    D: SimDriver<Event = mc2_sim::SimEvent, World = mc2_sim::World> + 'static,
{
    let sink = AudioSink::<D>::load(data_root, mixer, output, Box::new(SimAudioBridge))?;
    let shared = Rc::new(RefCell::new(sink));
    Ok((Box::new(SinkHandle(Rc::clone(&shared))), shared))
}

/// Writes interleaved stereo f32 samples as a 16-bit PCM WAV.
pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&2u16.to_le_bytes()); // stereo
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        out.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{FrameInput, FrameOutput};
    use mc2_sim::rng::RngMode;
    use mc2_sim::{SimEvent, World};

    struct TestDriver {
        world: World,
    }

    impl SimDriver for TestDriver {
        type Command = ();
        type Event = SimEvent;
        type World = World;

        fn step(&mut self, _input: FrameInput<()>) -> FrameOutput<SimEvent> {
            FrameOutput::default()
        }

        fn world(&self) -> &World {
            &self.world
        }
    }

    #[test]
    fn mission_start_requests_the_scenario_tune_and_updates_audio_readback() {
        let Some(data_root) = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from) else {
            return;
        };
        let mut driver = TestDriver {
            world: World::new(RngMode::LegacyMsvc, 1),
        };
        driver.world.mission.tune_num = 9;
        let mut sink = AudioSink::<TestDriver>::offline(&data_root, Box::new(SimAudioBridge))
            .expect("load game audio");

        // A logistics tune is already playing when the app enters the mission.
        let mut rng = |_: i32| 0;
        sink.audio
            .borrow_mut()
            .handle(&AudioEvent::Music(8), &mut rng);
        sink.take_log();

        sink.mission_start(&driver.world);

        assert_eq!(sink.take_log(), vec![AudioEvent::Music(9)]);
        assert_eq!(
            sink.audio.borrow().sound.sys.current_music_id(),
            9,
            "the app readback reports the mission tune after replacing logistics music"
        );
    }

    #[test]
    fn a_recorded_readback_does_not_change_when_the_sound_system_does() {
        let Some(data_root) = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from) else {
            return;
        };
        let sink = AudioSink::<TestDriver>::offline(&data_root, Box::new(SimAudioBridge))
            .expect("load game audio");
        let recorded = crate::audio::readback_of(&sink.audio);
        assert_eq!(recorded.music_id, -1);
        let mut rng = |_: i32| 0;
        sink.audio
            .borrow_mut()
            .handle(&AudioEvent::Music(9), &mut rng);
        let later = crate::audio::readback_of(&sink.audio);
        assert_eq!(later.music_id, 9, "the sound system moved on");
        // the frame recorded earlier still reads the state it was recorded with
        assert_eq!(recorded.music_id, -1);
        assert!(!recorded.music_streams_busy);
        // the recorded file queries answer as recorded, whatever the sound system does later
        let (m, v) = (
            recorded.files.music_file(9),
            recorded.files.voice_file("tut_4a"),
        );
        assert!(m, "tune 9 exists in the data");
        assert!(v, "tut_4a exists in the data");
        drop(sink);
        assert_eq!(recorded.files.music_file(9), m);
        // a name never asked before the sound owner is gone still resolves against the (immutable) data
        assert!(recorded.files.voice_file("tut_4b") || recorded.files.voice_file("tut_5a"));
        assert!(!recorded.files.voice_file("no_such_voice_file"));
        assert!(!recorded.files.music_file(99_999));
    }

    /// Maps one event.
    fn map(e: SimEvent) -> Vec<AudioEvent> {
        let mut out = Vec::new();
        map_event(&e, &mut out);
        out
    }

    // The expected values below are read off the C++ call sites and `mclib/sounds.h` / `code/dradio.h`,
    // not off `map_event`.

    #[test]
    fn positional_sound_keeps_id_position_and_dupes() {
        // `soundSystem->playDigitalSample(INCOMING_AIRSTRIKE, position, true)` (code/artlry.cpp:1039;
        // INCOMING_AIRSTRIKE = 63, mclib/sounds.h:75).
        let at = mc2_sim::math::Vec3 {
            x: 120.0,
            y: -40.0,
            z: 8.0,
        };
        assert_eq!(
            map(SimEvent::Sound {
                id: 63,
                source: mc2_sim::WatchId::NONE,
                at: Some(at),
                allow_dupes: true
            }),
            vec![AudioEvent::Sound {
                id: 63,
                at: Some(AVec3::new(120.0, -40.0, 8.0)),
                allow_dupes: true
            }]
        );
    }

    #[test]
    fn explosion_uses_its_data_sound_id_and_position() {
        let at = mc2_sim::math::Vec3 {
            x: 12.0,
            y: 34.0,
            z: 5.0,
        };
        assert_eq!(
            map(SimEvent::Explosion {
                effect_id: 9,
                sound_id: Some(19),
                at,
                radius: 10.0,
            }),
            vec![AudioEvent::Sound {
                id: 19,
                at: Some(AVec3::new(12.0, 34.0, 5.0)),
                allow_dupes: true,
            }]
        );
    }

    #[test]
    fn airstrike_plays_its_incoming_sample_and_the_gosfx_impact_stays_silent() {
        let at = mc2_sim::math::Vec3 {
            x: 1.0,
            y: 2.0,
            z: 0.0,
        };
        // The impact is a gosFX-only effect (no ExplosionType), so its event carries no sound id (artlry.cpp:1533,1769).
        assert!(map(SimEvent::Explosion {
            effect_id: 50,
            sound_id: None,
            at,
            radius: 0.0,
        })
        .is_empty());
        assert_eq!(
            map(SimEvent::Sound {
                id: mc2_sim::combat::artillery::INCOMING_AIRSTRIKE,
                source: mc2_sim::WatchId::NONE,
                at: Some(at),
                allow_dupes: true,
            }),
            vec![AudioEvent::Sound {
                id: 63,
                at: Some(AVec3::new(1.0, 2.0, 0.0)),
                allow_dupes: true,
            }]
        );
    }

    #[test]
    fn mission_sound_samples_become_positional_playback_requests() {
        let at = mc2_sim::math::Vec3 {
            x: 3.0,
            y: 4.0,
            z: 5.0,
        };
        for id in [
            4, 14, 19, 38, 42, 60, 61, 72, 74, 75, 76, 101, 106, 107, 108, 109, 114, 116,
        ] {
            assert_eq!(
                map(SimEvent::Sound {
                    id,
                    source: mc2_sim::WatchId::NONE,
                    at: Some(at),
                    allow_dupes: false,
                }),
                vec![AudioEvent::Sound {
                    id: id as u32,
                    at: Some(AVec3::new(3.0, 4.0, 5.0)),
                    allow_dupes: false,
                }],
                "sample {id}"
            );
        }
    }

    #[test]
    fn centred_sound_has_no_position() {
        // `soundSystem->playDigitalSample(PING_SFX)` (code/bldng.cpp:769; PING_SFX = 20, sounds.h:35): default
        // position, `allowDupes = false`.
        assert_eq!(
            map(SimEvent::Sound {
                id: 20,
                source: mc2_sim::WatchId::NONE,
                at: None,
                allow_dupes: false
            }),
            vec![AudioEvent::Sound {
                id: 20,
                at: None,
                allow_dupes: false
            }]
        );
    }

    #[test]
    fn a_negative_sound_id_plays_nothing() {
        assert!(map(SimEvent::Sound {
            id: -1,
            source: mc2_sim::WatchId::NONE,
            at: None,
            allow_dupes: false
        })
        .is_empty());
    }

    #[test]
    fn radio_message_keeps_pilot_message_and_command() {
        // `radioMessage(RADIO_DEATH)` (code/warrior.cpp:1304); RADIO_DEATH is the 20th name of the enum in
        // code/dradio.h (index 19). `underHomeCommand()` is the gate at warrior.cpp:1316.
        assert_eq!(
            map(SimEvent::Radio {
                pilot: 3,
                message: 19,
                under_home_command: true
            }),
            vec![AudioEvent::Radio {
                pilot: PilotId(3),
                message: 19,
                under_home_command: true
            }]
        );
    }

    #[test]
    fn betty_and_support_and_music_requests() {
        // `playBettySample(BETTY_OBJECTIVE_COMPLETE)` (code/objective.cpp:1932; = 4, sounds.h:174).
        assert_eq!(map(SimEvent::Betty(4)), vec![AudioEvent::Betty(4)]);
        assert_eq!(map(SimEvent::StopBetty), vec![AudioEvent::StopBetty]);
        // `playSupportSample(-1, "tut_4a")` (code/missionresults.cpp:211, code/ablmc2.cpp:6264).
        assert_eq!(
            map(SimEvent::VoiceOver("tut_4a".into())),
            vec![AudioEvent::VoiceOver("tut_4a".into())]
        );
        assert_eq!(map(SimEvent::Support(7)), vec![AudioEvent::Support(7)]);
        for id in 0..=6 {
            assert_eq!(map(SimEvent::Support(id)), vec![AudioEvent::Support(id)]);
        }
        // `stopSupportSample()` (code/ablmc2.cpp:5981).
        assert_eq!(map(SimEvent::StopSupport), vec![AudioEvent::StopSupport]);
        // `playABLDigitalMusic(MissionWonTune0 = 9)` (mc2_01.abl:283, mclib/soundsys.cpp:1275).
        assert_eq!(map(SimEvent::Music(9)), vec![AudioEvent::Music(9)]);
        // `stopABLMusic()` (mclib/soundsys.cpp:1263).
        assert_eq!(map(SimEvent::StopMusic), vec![AudioEvent::StopMusic]);
    }

    #[test]
    fn sound_system_state_flags() {
        // `GeneralAlarm = true` (code/bldng.cpp:774), `largestSensorContact = currentScan` (code/contact.cpp:534)
        // and `-1` for none (contact.cpp:762), `setIsRaining(id)`.
        assert_eq!(
            map(SimEvent::GeneralAlarm(true)),
            vec![AudioEvent::GeneralAlarm(true)]
        );
        assert_eq!(
            map(SimEvent::SensorContact(2)),
            vec![AudioEvent::SensorContact(2)]
        );
        assert_eq!(
            map(SimEvent::SensorContact(-1)),
            vec![AudioEvent::SensorContact(-1)]
        );
        assert_eq!(map(SimEvent::Rain(5)), vec![AudioEvent::Rain(5)]);
    }

    #[test]
    fn mission_result_plays_betty_won_or_lost() {
        // code/mechcmd2.cpp:2160-2164: result < mis_PLAYER_DRAW (3) -> BETTY_MISSION_LOST (11, sounds.h:181);
        // result > 3 -> BETTY_MISSION_WON (18, sounds.h:188); a draw and mis_PLAYING (0) play nothing.
        for lost in [1, 2] {
            assert_eq!(
                map(SimEvent::MissionResult(lost)),
                vec![AudioEvent::Betty(11)]
            );
        }
        for won in [4, 5] {
            assert_eq!(
                map(SimEvent::MissionResult(won)),
                vec![AudioEvent::Betty(18)]
            );
        }
        assert!(map(SimEvent::MissionResult(3)).is_empty());
        assert!(map(SimEvent::MissionResult(0)).is_empty());
    }

    #[test]
    fn events_without_a_sound_call_map_to_nothing() {
        assert!(map(SimEvent::PilotsChanged).is_empty());
        assert!(map(SimEvent::Crater {
            kind: 0,
            at: mc2_sim::math::Vec3::default(),
            rotation: 0.0,
        })
        .is_empty());
        assert!(map(SimEvent::Script {
            what: "x",
            args: Vec::new()
        })
        .is_empty());
    }

    #[test]
    fn sim_sound_event_renders_to_headless_wav() {
        let Some(data_root) = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from) else {
            return;
        };
        let driver = TestDriver {
            world: World::new(RngMode::LegacyMsvc, 1),
        };
        let events = [
            SimEvent::Sound {
                id: 1,
                source: mc2_sim::WatchId::NONE,
                at: None,
                allow_dupes: false,
            },
            SimEvent::Betty(4),
        ];
        let mut sink = AudioSink::offline(&data_root, Box::new(SimAudioBridge))
            .expect("load game audio for headless rendering");
        let ctx = FrameContext::<TestDriver> {
            frame: 0,
            frame_length: 1.0 / 30.0,
            turn: 1,
            scenario_time: 0.0,
            listener: mc2_render::math::Vec3::ZERO,
            listener_rotation: 0.0,
            world: &driver.world,
        };
        sink.on_frame(&ctx, &events);
        assert_eq!(sink.handled, 2);
        let samples = sink.offline_samples().expect("offline samples");
        assert!(!samples.is_empty());

        let path = std::env::temp_dir().join(format!("mc2-app-audio-{}.wav", std::process::id()));
        write_wav(&path, samples, OFFLINE_SAMPLE_RATE).expect("write wav");
        let bytes = std::fs::read(&path).expect("read wav");
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(bytes.len() > 44);
        std::fs::remove_file(path).expect("remove temporary wav");
    }

    #[test]
    fn a_frame_with_no_events_stays_silent_and_a_sound_event_is_heard() {
        let Some(data_root) = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from) else {
            return;
        };
        let driver = TestDriver {
            world: World::new(RngMode::LegacyMsvc, 1),
        };
        let (mut sink, _) = {
            let s = AudioSink::<TestDriver>::offline(&data_root, Box::new(SimAudioBridge))
                .expect("load game audio");
            (s, ())
        };
        let ctx = FrameContext::<TestDriver> {
            frame: 0,
            frame_length: 1.0 / 30.0,
            turn: 1,
            scenario_time: 0.0,
            listener: mc2_render::math::Vec3::ZERO,
            listener_rotation: 0.0,
            world: &driver.world,
        };
        sink.on_frame(&ctx, &[]);
        let quiet = sink.offline_samples().expect("offline").to_vec();
        assert!(!quiet.is_empty());
        assert!(quiet.iter().all(|&s| s == 0.0), "no events, no sound");
        assert!(sink.take_log().is_empty());

        // PING_SFX (sensor contact), centred (code/bldng.cpp:769)
        let ping = [SimEvent::Sound {
            id: 20,
            source: mc2_sim::WatchId::NONE,
            at: None,
            allow_dupes: false,
        }];
        sink.on_frame(&ctx, &ping);
        sink.on_frame(&ctx, &[]);
        let all = sink.offline_samples().expect("offline");
        assert!(
            all[quiet.len()..].iter().any(|&s| s != 0.0),
            "the ping is rendered"
        );
        assert!(sink.audio.borrow().sound.sys.is_playing_sample(20) || sink.handled == 1);
        assert_eq!(sink.take_log().len(), 1);
    }

    #[test]
    fn sim_audio_bridge_registers_loaded_pilot_voice_sets() {
        let mut world = World::new(RngMode::LegacyMsvc, 1);
        world.pilots.slots = vec![mc2_sim::ai::Pilot::default(); 2];
        let profile = mc2_sim_data::pilot::WarriorProfile {
            file_name: "testpilot".into(),
            general: mc2_sim_data::pilot::General {
                pilot_audio: Some("pilota".into()),
                ..Default::default()
            },
            personality: None,
            skills: Default::default(),
            extra_skills: Default::default(),
            original_skills: Default::default(),
            starting_skills: Default::default(),
            skill_points: [0.0; mc2_sim_data::pilot::NUM_SKILLS],
            specialty: [false; mc2_sim_data::pilot::NUM_SPECIALTY_SKILLS],
            specialty_extra: Vec::new(),
            awards: None,
            affinities: None,
            wounds: None,
        };
        world.mission.pilot_loads = vec![
            Default::default(),
            mc2_sim::mission::pilots::PilotLoad {
                used: true,
                profile: Some(std::rc::Rc::new(profile)),
                ..Default::default()
            },
        ];
        assert_eq!(
            <SimAudioBridge as AudioBridge<TestDriver>>::pilots(&mut SimAudioBridge, &world),
            vec![(PilotId(1), "pilota".into(), true)]
        );
    }

    fn radio_world(pilots: usize) -> World {
        let mut world = World::new(RngMode::LegacyMsvc, 1);
        world.pilots.slots = vec![mc2_sim::ai::Pilot::default(); pilots + 1];
        world.mission.pilot_loads = vec![Default::default()];
        for _ in 0..pilots {
            let profile = mc2_sim_data::pilot::WarriorProfile {
                file_name: "testpilot".into(),
                general: mc2_sim_data::pilot::General {
                    pilot_audio: Some("pilota".into()),
                    ..Default::default()
                },
                personality: None,
                skills: Default::default(),
                extra_skills: Default::default(),
                original_skills: Default::default(),
                starting_skills: Default::default(),
                skill_points: [0.0; mc2_sim_data::pilot::NUM_SKILLS],
                specialty: [false; mc2_sim_data::pilot::NUM_SPECIALTY_SKILLS],
                specialty_extra: Vec::new(),
                awards: None,
                affinities: None,
                wounds: None,
            };
            world
                .mission
                .pilot_loads
                .push(mc2_sim::mission::pilots::PilotLoad {
                    used: true,
                    profile: Some(std::rc::Rc::new(profile)),
                    ..Default::default()
                });
        }
        world
    }

    /// `MechWarrior::active()` is `status == WARRIOR_STATUS_NORMAL` (code/warrior.h:1100). `injure`/`eject` call
    /// `radioMessage(RADIO_DEATH)` (a priority-0 line) while the status is still NORMAL, then set DEAD/EJECTED and
    /// turn the radio off (code/warrior.cpp:1981-1993, 2017-2041): the final line is heard, everything queued from that
    /// pilot and everything he says later is dropped.
    #[test]
    fn a_dead_or_ejected_pilot_gets_his_last_line_and_then_no_chatter() {
        use mc2_audio::RadioMessageType as M;
        use mc2_sim::mission::outcome::WarriorStatus;
        let Some(data_root) = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from) else {
            return;
        };
        let mut driver = TestDriver {
            world: radio_world(2),
        };
        let mut sink = AudioSink::<TestDriver>::offline(&data_root, Box::new(SimAudioBridge))
            .expect("load game audio");
        sink.mission_start(&driver.world);
        let say = |pilot: u32, m: M| SimEvent::Radio {
            pilot,
            message: m.index() as i32,
            under_home_command: true,
        };
        let mut turn = 1;
        let mut frame = |sink: &mut AudioSink<TestDriver>, world: &World, events: &[SimEvent]| {
            let ctx = FrameContext::<TestDriver> {
                frame: turn as u64,
                frame_length: 1.0 / 30.0,
                turn,
                scenario_time: turn as f32,
                listener: mc2_render::math::Vec3::ZERO,
                listener_rotation: 0.0,
                world,
            };
            turn += 1;
            sink.on_frame(&ctx, events);
        };
        let queued = |sink: &AudioSink<TestDriver>, pilot: u32| {
            let audio = sink.audio.borrow();
            audio
                .sound
                .queued_messages()
                .filter(|m| m.pilot == PilotId(pilot))
                .map(|m| m.msg_type)
                .collect::<Vec<_>>()
        };

        // Pilot 2 queues chatter behind pilot 1's playing line.
        frame(&mut sink, &driver.world, &[say(1, M::Ack)]);
        frame(&mut sink, &driver.world, &[say(2, M::Guard)]);
        assert_eq!(queued(&sink, 2), vec![M::Guard]);

        // Pilot 2 dies this frame: `radioMessage(RADIO_DEATH)` runs first (status NORMAL), then the status changes.
        driver.world.pilots.slots[2].status = WarriorStatus::Dead;
        frame(&mut sink, &driver.world, &[say(2, M::Death)]);
        assert!(
            queued(&sink, 2)
                .into_iter()
                .chain(
                    sink.audio
                        .borrow()
                        .sound
                        .current_message()
                        .map(|m| m.msg_type)
                )
                .any(|t| t == M::Death),
            "the death line is heard"
        );
        assert!(
            !queued(&sink, 2).contains(&M::Guard),
            "his queued chatter is gone"
        );

        // Later chatter of the dead pilot is refused; a living pilot still talks.
        let before = sink.audio.borrow().sound.messages_in_queue();
        frame(&mut sink, &driver.world, &[say(2, M::Taunt)]);
        assert!(!queued(&sink, 2).contains(&M::Taunt));
        assert!(sink.audio.borrow().sound.messages_in_queue() <= before);
        frame(&mut sink, &driver.world, &[say(1, M::Taunt)]);
        assert!(
            queued(&sink, 1).contains(&M::Taunt)
                || sink
                    .audio
                    .borrow()
                    .sound
                    .current_message()
                    .is_some_and(|m| m.pilot == PilotId(1)),
            "a living pilot still talks"
        );

        // Ejection works the same way.
        driver.world.pilots.slots[1].status = WarriorStatus::Ejected;
        frame(&mut sink, &driver.world, &[say(1, M::Ejecting)]);
        frame(&mut sink, &driver.world, &[say(1, M::Guard)]);
        assert!(!queued(&sink, 1).contains(&M::Guard));
    }
}
