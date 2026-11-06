// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Game sound in the app: which music and UI sounds the screens and the state machine ask for, and the
//! camera and clock that feed [`mc2_audio::GameAudio`].
//!
//! The music choices are ported from the C++ call sites, each cited on its function in [`tune`].
//! They come out as [`Cue`]s; [`AppAudio::apply`] plays them. The UI sounds are the ids the screens put
//! in `UiOutput::sounds` and `UiOutput::betty` (`mclib/sounds.h`), played as they are.
//!
//! **`SimEvent` -> `AudioEvent`** (weapon sounds, radio, mission Betty lines, ABL music cues) is
//! [`crate::sim_audio::map_event`], played by the battle sink [`crate::sim_audio::sim_audio_sink`].
//! The production paths register that sink (`production::install_sinks`); [`sim_events`] is the refusal
//! `GameCore` reports for a run that did not register it.
//!
//! Nothing here opens a device. [`AppAudio::open`] takes the mixer from the caller: the windowed front
//! end passes the output device's mixer, the headless one a plain mixer rendered with
//! [`AppAudio::render_offline`].

use crate::state::GameState;
use mc2_audio::data::{AudioData, VfsData};
use mc2_audio::rng::SeededRng;
use mc2_audio::{
    AudioError, AudioEvent, GameAudio, Listener, RadioClock, Vec3 as AudioVec3, VoicePack, Volumes,
};
use mc2_formats::vfs::Vfs;
use mc2_platform::audio::{Mixer, SharedMixer, APP_MIXER_CHANNELS};
use mc2_render::game::GameData;
use mc2_render::math::Vec3;
use mc2_sim::events::{AudioFileSet, AudioFiles, AudioReadback};
use mc2_ui::screens::{MainMenuFrame, OptionsFrame, Prefs};
use mc2_ui::UiOutput;
use std::cell::{Ref, RefCell, RefMut};
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// One thing the game asks the audio to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Cue {
    /// `playDigitalSample(id)`: a UI sound, centred, no duplicates (`mclib/soundsys.h:238`).
    Sound(i32),
    /// `playBettySample(id)`.
    Betty(i32),
    /// `playDigitalMusic(id)`.
    Music(i64),
    /// `stopDigitalMusic()`.
    StopMusic,
    /// The options screen's audio tab, applied on the fly: master, music, sfx, radio, Betty
    /// (`code/optionsarea.cpp:761-773`), each 0..=255.
    Volumes([i32; 5]),
}

/// Which tune plays where. Each function returns the cue the C++ would make, or `None` when it makes no
/// call (or a call that does nothing: `playDigitalMusic` ignores negative ids, `mclib/soundsys.cpp:679`).
///
/// The briefing (`code/missionbriefingscreen.cpp`, `code/missionbegin.cpp`, sound effects only)
/// and the loading screen (`code/loadscreen.cpp`, door samples only) do not change music. Mission
/// start uses the loaded mission's scenario tune (`code/mission.cpp:2124-2129`); ABL music requests
/// use `execPlayDigitalMusic` (`code/ablmc2.cpp:2606-2620`) and arrive as `SimEvent::Music`.
pub mod tune {
    use super::Cue;
    use mc2_audio::ids::music;

    /// `playDigitalMusic(33)` after the `cinema5` movie (`code/logistics.cpp:303`). The number is a
    /// literal in the C code, not one of the `sounds.h` names.
    pub const CREDITS_TUNE_DMS: i64 = 33;

    /// The main menu's tune: `TuneId` from its fit file (`code/mainmenu.cpp:100-102`), started once when
    /// the menu's music starts (`code/mainmenu.cpp:474-477`).
    pub fn main_menu(tune_id: i32) -> Option<Cue> {
        (tune_id >= 0).then(|| Cue::Music(i64::from(tune_id)))
    }

    /// The logistics tune of the current campaign stage (`LogisticsData::getCurrentMissionTune`,
    /// `code/logisticsdata.cpp:1795-1798`), played when the campaign starts
    /// (`LogisticsData::startNewCampaign`, `code/logisticsdata.cpp:2305`) and when mission selection begins
    /// (`code/missionselectionscreen.cpp:123-128`). It is -1 once the campaign has run past its last
    /// stage (`code/logisticsmissioninfo.cpp:922-931`).
    pub fn logistics(stage_tune: i64) -> Option<Cue> {
        (stage_tune >= 0).then_some(Cue::Music(stage_tune))
    }

    /// The mission tune replaces the logistics tune when the mission starts. The ABL native uses
    /// the same `playDigitalMusic` path (`code/ablmc2.cpp:2606-2620`) for subsequent script calls.
    pub fn mission_start_tune(tune_id: i32) -> Option<Cue> {
        (tune_id >= 0).then(|| Cue::Music(i64::from(tune_id)))
    }

    /// Back to the game from the menu: the logistics tune, not the mission's own
    /// (`code/mainmenu.cpp:325-328` calls `getCurrentMissionTune`).
    pub fn return_to_game(stage_tune: i64) -> Option<Cue> {
        logistics(stage_tune)
    }

    /// A movie has ended (`code/logistics.cpp:285-312`). `cinema5` hands over to the credits tune; any
    /// other movie to the logistics tune. The name test is `S_strnicmp(name, "cinema5", 7)`.
    pub fn after_movie(movie: &str, stage_tune: i64) -> Option<Cue> {
        let is_credits = movie
            .as_bytes()
            .get(..7)
            .is_some_and(|m| m.eq_ignore_ascii_case(b"cinema5"));
        if is_credits {
            Some(Cue::Music(CREDITS_TUNE_DMS))
        } else {
            logistics(stage_tune)
        }
    }

    /// A full-screen video starts (`code/logistics.cpp:880-897`).
    pub fn full_screen_video() -> Cue {
        Cue::StopMusic
    }

    // No win or lose tune is requested here. `Mission::update` forces one from `terminationResult`
    // (`code/mission.cpp:671-709`), but only inside `#ifndef FINAL`, and the Release build defines FINAL
    // (`MechCmd2.vcproj:53`). The shipped scripts make the same request themselves: a won mission plays
    // `MissionWonTune0` and a lost one `MissionLostTune0` (`getMissionWon` and `getMissionLost` in
    // `mc2_01.abl`, lines 283-284 and 294-295; `MissionWonTune0 = 9`, `MissionLostTune0 = 8`, `sndconst.abi`
    // lines 22-23). Those calls go through `execPlayDigitalMusic` (`code/ablmc2.cpp:2606-2617`), the ABL
    // music natives, which wait for the sim and ABL integration. Until then no win or lose tune plays.
    // The script's ids are the ones the forced block played for the same outcomes: a win is `LOSE_TUNE_0`
    // (9) and a loss is `WIN_TUNE_0` (8) (`mclib/sounds.h:144-145`). The C names are inverted, the tunes are not.

    /// The results screens' loop (`code/missionresults.cpp:188-192`).
    pub fn results() -> Cue {
        Cue::Music(i64::from(music::RESULTS_LOOP))
    }
}

/// The `MIS_PLAYER_WIN_BIG` result code: the one result that shows the results screens.
pub const MIS_PLAYER_WIN_BIG: u32 = 5;

/// The cues the state machine makes when it moves to `to`: none. The results loop starts when
/// `MissionResults::begin` runs (`code/missionresults.cpp:188-192`), which the UI host does, not
/// when the state becomes `Results`: a loss, or a win that skips the screens (a single mission, the
/// final video), never plays it (`Logistics::start(log_RESULTS)`, `code/logistics.cpp:121-180`).
/// The main menu, logistics and loading screens make their own cues (see [`tune`]).
pub fn state_change_cues(_to: &GameState) -> Vec<Cue> {
    Vec::new()
}

/// The cues a frame of `UiOutput` asks for: the sound effects in order, then the Betty lines.
pub fn cues_from_ui(out: &UiOutput) -> Vec<Cue> {
    out.sounds
        .iter()
        .map(|&id| Cue::Sound(id))
        .chain(out.betty.iter().map(|&id| Cue::Betty(id)))
        .collect()
}

/// The music a main menu frame asks for (`MainMenuFrame::music`).
pub fn main_menu_cues(frame: &MainMenuFrame) -> Vec<Cue> {
    frame.music.and_then(tune::main_menu).into_iter().collect()
}

/// The volumes an options frame carries while the audio tab is open.
pub fn options_cues(frame: &OptionsFrame) -> Vec<Cue> {
    frame.volumes.map(Cue::Volumes).into_iter().collect()
}

/// `Volumes` from the options screen's preferences (`CPrefs`), clamped to bytes.
pub fn volumes(p: &Prefs) -> Volumes {
    let b = |v: i32| v.clamp(0, 255) as u8;
    Volumes {
        digital_master: b(p.digital_master_volume),
        music: b(p.music_volume),
        sfx: b(p.sfx_volume),
        radio: b(p.radio_volume),
        betty: b(p.betty_volume),
    }
}

/// The mapping exists ([`crate::sim_audio::map_event`], every `SimEvent` that carries a sound call) but the
/// production `GameCore` does not register the battle sink yet, so the events it is handed are not played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SimAudioUnavailable;

impl std::fmt::Display for SimAudioUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "the battle sound sink (sim_audio::sim_audio_sink) is not registered in GameCore.sinks yet \
             (the P3 integration slice does that)",
        )
    }
}

impl std::error::Error for SimAudioUnavailable {}

/// What `GameCore` asks while the battle sink is not registered: no events map to nothing, and any event is
/// refused, so a caller cannot drop sound silently. The mapping itself is
/// [`crate::sim_audio::map_event`]; with the sink pushed into `GameCore.sinks` (one line in the integration
/// slice) the sink plays the events and this refusal goes away.
pub fn sim_events<E>(events: &[E]) -> Result<Vec<AudioEvent>, SimAudioUnavailable> {
    if events.is_empty() {
        Ok(Vec::new())
    } else {
        Err(SimAudioUnavailable)
    }
}

/// The game's `GameAudio`, shared by [`AppAudio`] and the battle sink.
pub type SharedGameAudio = Rc<RefCell<GameAudio>>;

/// The tune / voice-over files that exist, frozen when the readback is taken: the tune ids are listed up front, and a voice-over name
/// is resolved once against the (immutable) sound data and remembered. No live handle on the game audio is kept, so a recorded
/// frame answers the same after later playback.
struct GameFiles {
    music: Vec<bool>,
    voices: RefCell<std::collections::BTreeMap<String, bool>>,
    probe: Box<dyn Fn(&str) -> bool>,
}

impl AudioFileSet for GameFiles {
    fn music_file(&self, id: i64) -> bool {
        usize::try_from(id)
            .ok()
            .and_then(|i| self.music.get(i).copied())
            .unwrap_or(false)
    }
    fn voice_file(&self, name: &str) -> bool {
        *self
            .voices
            .borrow_mut()
            .entry(name.to_string())
            .or_insert_with(|| (self.probe)(name))
    }
}

/// A frozen snapshot of `game`'s sound state for the mission scripts (see [`AppAudio::readback`]).
pub fn readback_of(game: &SharedGameAudio) -> AudioReadback {
    let g = game.borrow();
    let sys = &g.sound.sys;
    let music = (0..sys.music_table_len() as i64)
        .map(|i| sys.music_file_exists(i))
        .collect();
    // the voice probe reads the immutable sound data only; it owns a strong handle so the answer for a
    // recorded frame never depends on whether the app still holds the audio (the data does not change)
    let owned = Rc::clone(game);
    let probe: Box<dyn Fn(&str) -> bool> = Box::new(move |name| {
        owned
            .try_borrow()
            .is_ok_and(|g| g.sound.sys.voice_over_file_exists(name))
    });
    AudioReadback {
        music_id: sys.current_music_id(),
        voice_over: sys.is_playing_voice_over(),
        betty_free: !sys.is_betty_loaded(),
        betty_samples: u32::try_from(sys.bank().betty.len()).unwrap_or(u32::MAX),
        music_enabled: sys.music_enabled(),
        music_streams_busy: sys.music_streams_busy(),
        voice_available: sys.voice_over_available(),
        voice_busy: sys.voice_over_busy(),
        files: AudioFiles::new(Rc::new(GameFiles {
            music,
            voices: RefCell::new(Default::default()),
            probe,
        })),
    }
}

/// The game sound, as the app drives it.
pub struct AppAudio {
    /// The game's one `GameAudio`: the app's cues and the battle sink ([`crate::sim_audio::AudioSink`]) both
    /// drive it, because its channel table must be the only one on its mixer.
    game: SharedGameAudio,
    mixer: SharedMixer,
    /// The radio's style rolls. They draw from the sim's visual stream once the sim hook lands; until
    /// then a fixed seed.
    rng: SeededRng,
}

/// The optional pilot voice pack directory (`--voice-dir`, `$MC2_VOICE_DIR`, `voices =` in mc2.cfg), set once by
/// the front end before audio opens. Unset (the default): the original pilot voices only.
static VOICE_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Records the voice pack directory for every [`AppAudio`] opened afterwards. `None` and repeat calls are no-ops.
pub fn set_voice_dir(dir: Option<PathBuf>) {
    if let Some(dir) = dir {
        let _ = VOICE_DIR.set(dir);
    }
}

impl AppAudio {
    /// Loads the sound data under `root` on `mixer`. The volumes come from `options.cfg` in the data
    /// root (defaults when it is missing), as the game reads them at start. `stream_loose_files` decides
    /// how loose music files are played: `true` streams them from disk while they play (the game),
    /// `false` decodes them whole up front (deterministic offline renders, tests).
    pub fn open(
        root: &Path,
        game: &mut GameData,
        mixer: SharedMixer,
        stream_loose_files: bool,
    ) -> Result<Self, String> {
        let prefs = match game.fit("options.cfg") {
            Ok(mut f) => Prefs::from_fit(&mut f),
            Err(_) => Prefs::default(),
        };
        let vfs = Vfs::from_system_cfg(root).map_err(|e| e.to_string())?;
        Self::new(vfs, mixer, volumes(&prefs), stream_loose_files).map_err(|e| e.to_string())
    }

    /// Loads the sound data from `vfs` on `mixer` with `volumes`.
    pub fn new(
        vfs: Vfs,
        mixer: SharedMixer,
        volumes: Volumes,
        stream_loose_files: bool,
    ) -> Result<Self, AudioError> {
        let data: Box<dyn AudioData> = Box::new(VfsData::new(vfs, stream_loose_files));
        let mut game = GameAudio::load(data, mixer.clone(), volumes)?;
        if let Some(dir) = VOICE_DIR.get() {
            match VoicePack::open(dir) {
                Some(pack) => game.set_voice_pack(Some(pack)),
                None => eprintln!("voice pack {} is not a directory: ignored", dir.display()),
            }
        }
        Ok(Self {
            game: Rc::new(RefCell::new(game)),
            mixer,
            rng: SeededRng::new(0x4d43_3241),
        })
    }

    /// A mixer for an offline render at the game's rate, with no device behind it.
    pub fn offline_mixer() -> SharedMixer {
        SharedMixer::new(Mixer::new(OFFLINE_RATE, APP_MIXER_CHANNELS))
    }

    /// The shared game mixer. The UI host uses its reserved movie channel.
    pub fn mixer(&self) -> SharedMixer {
        self.mixer.clone()
    }

    /// The audio side, for tests and the app's own queries.
    pub fn game(&self) -> Ref<'_, GameAudio> {
        self.game.borrow()
    }

    pub fn game_mut(&mut self) -> RefMut<'_, GameAudio> {
        self.game.borrow_mut()
    }

    /// The shared handle the battle sink drives ([`crate::sim_audio::AudioSink::shared`]).
    pub fn shared_game(&self) -> SharedGameAudio {
        Rc::clone(&self.game)
    }

    /// What the sound system reports to the mission scripts (`getCurrentMusicId`, `isPlayingVoiceOver`, `playBettySample`'s
    /// answer): read before each simulation step (`mc2_sim::World::audio`), so the sim sees only what the sound system holds.
    pub fn readback(&self) -> AudioReadback {
        readback_of(&self.game)
    }

    /// Plays one cue.
    pub fn apply(&mut self, cue: &Cue) {
        let event = match cue {
            Cue::Sound(id) => {
                // A negative id is `setPressFX(-1)`: silent, as `UpdateCtx::play` already drops it.
                let Ok(id) = u32::try_from(*id) else { return };
                AudioEvent::Sound {
                    id,
                    at: None,
                    allow_dupes: false,
                }
            }
            Cue::Betty(id) => match u32::try_from(*id) {
                Ok(id) => AudioEvent::Betty(id),
                Err(_) => return,
            },
            Cue::Music(id) => AudioEvent::Music(*id),
            Cue::StopMusic => AudioEvent::StopMusic,
            Cue::Volumes(v) => {
                let b = |x: i32| x.clamp(0, 255) as u8;
                self.game.borrow_mut().set_volumes(Volumes {
                    digital_master: b(v[0]),
                    music: b(v[1]),
                    sfx: b(v[2]),
                    radio: b(v[3]),
                    betty: b(v[4]),
                });
                return;
            }
        };
        self.game.borrow_mut().handle(&event, &mut self.rng);
    }

    /// Plays each cue in order.
    pub fn apply_all(&mut self, cues: &[Cue]) {
        for c in cues {
            self.apply(c);
        }
    }

    /// The camera is the listener (`eye`): its position and yaw in degrees (`cameraRotation`).
    pub fn set_camera(&mut self, eye: Vec3, yaw_degrees: f32) {
        self.game.borrow_mut().set_listener(Some(Listener {
            position: AudioVec3 {
                x: eye.x,
                y: eye.y,
                z: eye.z,
            },
            rotation_y: yaw_degrees,
        }));
    }

    /// The mission clock: the turn and the scenario time (`SoundSystem` radio timing).
    pub fn set_clock(&mut self, turn: u32, scenario_time: f32) {
        self.game.borrow_mut().set_clock(RadioClock {
            turn,
            scenario_time,
        });
    }

    /// Advances the sound by `dt` real seconds (`GameSoundSystem::update`).
    pub fn update(&mut self, dt: f32) {
        self.game.borrow_mut().update(dt);
    }

    /// Headless: [`update`](Self::update) for `dt` seconds, then the mixed audio appended to `out`
    /// (interleaved stereo). Never touches a device.
    pub fn render_offline(&mut self, dt: f32, out: &mut Vec<f32>) {
        self.game.borrow_mut().render_offline(dt, out);
    }

    /// Headless: mixes one frame of `dt` seconds into `out` without updating the sound first (the
    /// app's frame has already run [`update`](Self::update)). Interleaved stereo, no device.
    pub fn mix_frame(&mut self, dt: f32, out: &mut Vec<f32>) {
        self.game.borrow_mut().render_frame(dt, out);
    }
}

/// The rate of [`AppAudio::offline_mixer`]: the one the sound tests use.
pub const OFFLINE_RATE: u32 = 44_100;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_change_makes_no_cue() {
        // the results loop belongs to `MissionResults::begin`, not to the state change
        assert!(state_change_cues(&GameState::Results(MIS_PLAYER_WIN_BIG)).is_empty());
        assert!(state_change_cues(&GameState::Results(1)).is_empty());
        // Entering the menu or the logistics screens makes no state cue.
        assert!(state_change_cues(&GameState::MainMenu).is_empty());
        assert!(state_change_cues(&GameState::Logistics).is_empty());
    }

    #[test]
    fn ui_output_becomes_cues_in_order() {
        let out = UiOutput {
            sounds: vec![95, 90],
            betty: vec![47],
            ..Default::default()
        };
        assert_eq!(
            cues_from_ui(&out),
            vec![Cue::Sound(95), Cue::Sound(90), Cue::Betty(47)]
        );
    }

    #[test]
    fn main_menu_frame_music_is_its_tune() {
        let frame = MainMenuFrame {
            music: Some(7),
            ..Default::default()
        };
        assert_eq!(main_menu_cues(&frame), vec![Cue::Music(7)]);
        assert!(main_menu_cues(&MainMenuFrame::default()).is_empty());
    }

    #[test]
    fn mission_start_uses_the_scenario_tune_id() {
        assert_eq!(tune::mission_start_tune(9), Some(Cue::Music(9)));
        assert_eq!(tune::mission_start_tune(-1), None);
    }

    #[test]
    fn options_volumes_are_clamped_to_bytes() {
        let p = Prefs {
            digital_master_volume: 300,
            music_volume: -4,
            sfx_volume: 64,
            radio_volume: 10,
            betty_volume: 200,
            ..Prefs::default()
        };
        let v = volumes(&p);
        assert_eq!(
            (v.digital_master, v.music, v.sfx, v.radio, v.betty),
            (255, 0, 64, 10, 200)
        );
    }
}
