// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Output of the simulation towards presentation (renderer, audio, UI). The C++ sim calls straight into the renderer and
//! sound system (`soundSystem->playDigitalSample`, `craterManager->addCrater`, `gosFX` effects, radio chatter, flash
//! effects on appearances). The port replaces every such call with a [`SimEvent`] pushed to `World::events`; the
//! application drains the queue after each step. Nothing here is ever read back by the simulation.

use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;

/// Mission SFX ids from `mclib/sounds.h`.
pub mod sfx {
    pub const MECH_COLLIDE: i32 = 4;
    pub const TREEFALL: i32 = 14;
    pub const CRITICAL_HIT_SFX: i32 = 19;
    pub const GATE_OPEN: i32 = 42;
    pub const GATE_CLOSE: i32 = 42;
    pub const POWERDOWN_SFX: i32 = 60;
    pub const POWERUP_SFX: i32 = 61;
    pub const BREAKINGFENCE: i32 = 72;
    pub const VTOL_ANIMATE: i32 = 74;
    pub const VTOL_DROP: i32 = 75;
    pub const JUMPJETS: i32 = 76;
    pub const RADAR_HUM: i32 = 38;
    pub const REPAIRBAY_FX: i32 = 101;
    pub const HELICOPTER_DEATH: i32 = 114;
    pub const SALVAGE_CRAFT: i32 = 116;
    pub const COPTER_POWERUP: i32 = 106;
    pub const COPTER_POWERDN: i32 = 107;
    pub const VEHICLE_POWERUP: i32 = 108;
    pub const VEHICLE_POWERDN: i32 = 109;
    // The shipped sound.snd names reverse the legacy C++ constant names above.
    pub const COPTER_POWER_DOWN_SAMPLE: i32 = 106;
    pub const COPTER_POWER_UP_SAMPLE: i32 = 107;
    pub const VEHICLE_POWER_DOWN_SAMPLE: i32 = 108;
    pub const VEHICLE_POWER_UP_SAMPLE: i32 = 109;
}

/// Betty sample ids from `mclib/sounds.h`.
pub mod betty {
    pub const POWERUP: u32 = 38;
    pub const RESOURCES: u32 = 39;
    pub const BUILDING_RECAPTURED: u32 = 49;
    pub const GENERATOR_DESTROYED: u32 = 50;
    pub const SENSOR_CAPTURED: u32 = 53;
}

/// Support voice ids from `mclib/sounds.h`.
pub mod support {
    pub const AIRSTRIKE: u32 = 0;
    pub const MINELAYER: u32 = 1;
    pub const PROBE: u32 = 2;
    pub const RECOVER: u32 = 3;
    pub const REPAIR: u32 = 4;
    pub const SCOUT: u32 = 5;
    pub const ARTILLERY: u32 = 6;
}

/// How a weapon bolt ended (`WeaponBolt::update` weaponbolt.cpp:614-708).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum WeaponImpact {
    /// The bolt just started (first `WeaponBolt::update` frame, weaponbolt.cpp:477-502): `from` is the muzzle, `to` the aim point.
    Launch,
    /// It reached its target object: the hit effect.
    Hit,
    /// It reached the ground at `to`: the miss effect, or the water-miss effect over water.
    Miss,
}

/// Which of an object's attached effects an [`SimEvent::AppearanceFx`] drives: the C++ appearance class keeps one `gosFX::Effect*`
/// per member (`rightDustPoofEffect[i]`, `jumpJetEffect`, `waterWake`, `activity`, ...), and this is that member.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FxSlot {
    /// `rightDustPoofEffect[i]` (`MAX_DUST_POOFS` = 3, round robin).
    RightFootPoof(u8),
    /// `leftDustPoofEffect[i]`.
    LeftFootPoof(u8),
    /// `jumpJetEffect`.
    JumpJet,
    /// `smokeEffect` (`MECH_SMOKE_ID`, the damage smoke).
    Smoke,
    /// `criticalSmoke` (`CRITICAL_SMOKE_ID`).
    CriticalSmoke,
    /// `waterWake`.
    WaterWake,
    /// `leftShoulderBoom` / `rightShoulderBoom` (`true` = left).
    ShoulderBoom(bool),
    /// `leftArmSmoke` / `rightArmSmoke` (`true` = left).
    ArmSmoke(bool),
    /// `helicopterDustCloud` (`playEjection`).
    FallDust,
    /// A vehicle's `dustCloud`.
    DustCloud,
    /// `destructFX`.
    Destruct,
    /// `activity` (`startActivity`).
    Activity,
    /// `activity1`, the second node of a building's activity effect.
    Activity1,
    /// The support VTOL / Karnov dust cloud of a commander (`dustCloud[commanderID]`); the event's object is
    /// [`ObjectHandle::NONE`](crate::ids::ObjectHandle::NONE).
    SupportDust(u8),
    /// The Karnov's `recoveryBeam[commanderID]`.
    RecoveryBeam(u8),
}

/// The effect a [`SimEvent::AppearanceFx`] plays: a row of `effects.csv` (`GetEffectName(id)`), or a library effect by name
/// (`destructEffect` of an appearance type).
#[derive(Clone, Debug, PartialEq)]
pub enum FxEffect {
    /// `weaponEffects->GetEffectName(id)`.
    Id(i32),
    /// `EffectLibrary::Find(name)`.
    Named(String),
}

/// The fixed rotation the C++ multiplies in front of the euler rotation (`gosFX::Effect_Into_Motion` / `Effect_Against_Motion`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FxBase {
    /// `shapeOrigin` alone: no rotation, just the translation.
    Plain,
    /// `Effect_Into_Motion`.
    IntoMotion,
    /// `Effect_Against_Motion`.
    AgainstMotion,
}

/// Where an attached effect stands: the C++ `shapeOrigin` (and, with `base`, `localToWorld * shapeOrigin`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FxPlacement {
    /// Game-world position (`x` east, `y` north, `z` up): the effect is placed at `(-x, z, y)`.
    pub at: Vec3,
    /// `EulerAngles(pitch, yaw, 0)` in degrees: the pitch (0 unless the C++ passes one).
    pub pitch: f32,
    /// The yaw in degrees (the object's `rotation`).
    pub yaw: f32,
    /// The base rotation in front of the euler angles.
    pub base: FxBase,
}

impl FxPlacement {
    /// `Plain`: translation only.
    pub fn plain(at: Vec3) -> FxPlacement {
        FxPlacement {
            at,
            pitch: 0.0,
            yaw: 0.0,
            base: FxBase::Plain,
        }
    }
}

/// What an attached effect does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum FxAction {
    /// `Effect::Start(&info)`. A running effect of the same slot is restarted. `looping` is the `LoopFlag` it was made with.
    Start { place: FxPlacement, looping: bool },
    /// `Effect::Execute(&info)` with a new placement (the effect follows its node). `looping` is `SetLoopOn` / `SetLoopOff`
    /// when the C++ switches it in the same call (`None` leaves it).
    Move {
        place: FxPlacement,
        looping: Option<bool>,
    },
    /// `Effect::Stop()`: the effect winds down.
    Stop,
    /// `Effect::Kill()`: the effect ends at once.
    Kill,
    /// Whether the C++ `render` draws the effect now (`!isWaking`, `currentGestureId != 20`, ...); it keeps running either way.
    Draw(bool),
}

/// `effects.csv` rows the appearances hard-code (`mclib/weaponfx.h`), the same numbers as `mc2_fx::weaponfx`.
pub mod fx_id {
    pub const MECH_WATER_WAKE: i32 = 34;
    pub const MECH_SMOKE_ID: i32 = 48;
    pub const VTOL_DUST_CLOUD: i32 = 56;
    pub const REPAIR_VEHICLE_EFFECT_ID: i32 = 57;
    pub const ARM_FLYING_ID: i32 = 61;
    pub const SHOULDER_POP_ID: i32 = 62;
    pub const CRITICAL_SMOKE_ID: i32 = 65;
    pub const VEHICLE_WATER_WAKE: i32 = 66;
    pub const HELICOPTER_DUST_CLOUD: i32 = 67;
    pub const MINELAYER_EFFECT_ID: i32 = 68;
    pub const KARNOV_RECOVERY_BEAM: i32 = 69;
    pub const VEHICLE_DUST_CLOUD: i32 = 70;
    pub const KARNOV_DUST_CLOUD: i32 = 73;
    pub const TURRET_POWER_DOWN_EFFECT: i32 = 75;
    /// `FX_POOF_ID`, `FX_JUMP_ID`, `FX_SMOKE_ID` (mech3d.cpp): the foot dust, the jump jet exhaust.
    pub const FX_POOF_ID: i32 = 32;
    pub const FX_JUMP_ID: i32 = 43;
}

/// One presentation request.
#[derive(Clone, Debug, PartialEq)]
pub enum SimEvent {
    /// A weapon effect should play (muzzle flash, bolt, beam, impact); `effect_id` indexes the weapon effect table.
    WeaponEffect {
        /// Deterministic identity of this shot, shared by its launch and end events.
        shot_id: u32,
        /// Effect table index (`WeaponFX`).
        effect_id: i32,
        /// Attacker.
        owner: WatchId,
        /// Start position.
        from: Vec3,
        /// End position.
        to: Vec3,
        /// Whether the bolt connected (`WeaponBolt::update` weaponbolt.cpp:614 target branch) or flew on to the ground.
        impact: WeaponImpact,
    },
    /// An explosion or fire object was created (`GameObjectManager::createExplosion`).
    Explosion {
        /// Effect id.
        effect_id: i32,
        /// `ExplosionType::soundEffectId` (`carnage.cpp:677-678`), if this effect is an explosion.
        sound_id: Option<i32>,
        /// Position.
        at: Vec3,
        /// Radius.
        radius: f32,
    },
    /// The strike's bomber starts its bomb run (`Artillery::update` artlry.cpp:938-947, `timeToImpact <= 2.5`): the
    /// "Shilone" bomber appearance is placed at the strike position with its bomb-run gesture. Raised once per strike for
    /// the small and large airstrikes (a sensor probe has no bomber).
    AirstrikeFlyover {
        /// The strike object.
        strike: WatchId,
        /// Strike type (`SMALL_ARTLRY` 0, `BIG_ARTLRY` 1).
        strike_type: i32,
        /// The bomber's position (the strike position).
        at: Vec3,
        /// Seconds left to impact when the run started.
        time_to_impact: f32,
    },
    /// The bomber's jet contrail effect starts (`rightContrail->Start`, artlry.cpp:950-962), on the same frame as the flyover.
    AirstrikeContrail {
        /// The strike object.
        strike: WatchId,
        /// `JET_CONTRAIL_ID` (mclib/weaponfx.h:26): the `effects.csv` row of the contrail.
        effect_id: i32,
        /// The bomber's position.
        at: Vec3,
    },
    /// An effect attached to an object's appearance starts, follows its node, winds down or ends (`Mech3DAppearance`,
    /// `GVAppearance`, `BldgAppearance` and the VTOL block of `MissionInterfaceManager`). `object` and `slot` name the C++ member
    /// (`rightDustPoofEffect[1]`, `waterWake`, ...); the application resolves `effect` against `effects.csv` / the library.
    AppearanceFx {
        object: ObjectHandle,
        slot: FxSlot,
        effect: FxEffect,
        action: FxAction,
    },
    /// `craterManager->addCrater(type, position, rotation)`: a terrain decal (a bomb crater or a mech footprint).
    Crater {
        /// `craterType` ([`crate::crater`]: footprint ids below 64, `CRATER_1..4` from 64).
        kind: i32,
        /// Position.
        at: Vec3,
        /// Rotation in degrees (`RandomNumber(180)` of the VISUAL stream for a crater, the mech's facing for a footprint).
        rotation: f32,
    },
    /// Radio chatter or sound effect request.
    Sound {
        /// Sound or radio message id.
        id: i32,
        /// Source object, if any.
        source: WatchId,
        /// World-space source position passed to `playDigitalSample`; `None` means centred audio.
        at: Option<Vec3>,
        /// `allowDupes` from the original sound call.
        allow_dupes: bool,
    },
    /// An object's appearance should flash (damage feedback).
    Flash {
        /// Object.
        object: ObjectHandle,
        /// ARGB colour.
        argb: u32,
    },
    /// A scripted/camera request from ABL (`camera*`, `playbik`, `displaytextmessage`...).
    Script {
        /// Script function name.
        what: &'static str,
        /// Integer arguments.
        args: Vec<i32>,
    },
    /// A mission script moved the camera (`SetCamera*` natives of `code/ablmc2.cpp`). The camera belongs to the application; the
    /// simulation only forwards the request (the `Get*` natives stay unavailable: they read the application's camera).
    Camera(CameraRequest),
    /// The mission reached a final result (`terminationResult` after the 5 s timer).
    MissionResult(u32),
    /// A pilot radio message (`MechWarrior::radioMessage`).
    Radio {
        pilot: u32,
        message: i32,
        under_home_command: bool,
    },
    /// Betty sample request / cancellation.
    Betty(u32),
    StopBetty,
    /// Support officer request / in-mission voice-over / cancellation.
    Support(u32),
    VoiceOver(String),
    StopSupport,
    /// Music cue / stop.
    Music(i64),
    StopMusic,
    /// Ambient and sound-system state inputs.
    Rain(u32),
    GeneralAlarm(bool),
    SensorContact(i64),
    /// The player's active pilot roster changed.
    PilotsChanged,
    /// A VTOL purchase was refused (`canAddVehicle` / `canRecover` false, or nothing prepared): the GUI plays `INVALID_GUI`.
    VtolRefused {
        /// The commander.
        commander: i32,
    },
    /// A VTOL dropped its support vehicle (`addVehicle`): the new mover.
    SupportVehicleDropped {
        /// The commander.
        commander: i32,
        /// `vehicleID`.
        vehicle_id: i32,
        /// Part id of the new mover.
        part_id: i32,
    },
    /// A VTOL or Karnov has left (`paintingVtol = 0`): the GUI un-presses the vehicle buttons
    /// (`ControlGui::unPressAllVehicleButtons`).
    VtolFinished {
        /// The commander.
        commander: i32,
        /// `vehicleID`.
        vehicle_id: i32,
        /// A recovery craft recovered its mech (otherwise its cost was returned).
        recovered: bool,
    },
    /// A mover update or path request reached a deferred decision boundary.
    /// The frame output exposes this instead of treating a stand-in as source-faithful success.
    MovementUnavailable {
        /// Mission part id.
        part_id: i32,
        /// Deferred branch.
        decision: crate::mover::MoverDecisionUnavailable,
    },
    /// An objective requested a mission-side action (video, text, technology, or other campaign effect).
    ObjectiveAction {
        /// Objective team.
        team: i32,
        /// Action definition and its parameters.
        action: mc2_sim_data::mission::Action,
    },
    /// Pilot AI behaviour that is not ported was reached (reported once per pilot and kind): the result of that call is not
    /// source-faithful, so a consumer must not treat the pilot's state as C++ state past this point.
    AiUnavailable {
        /// Mission part id of the mover the pilot flies.
        part_id: i32,
        /// What was reached.
        what: crate::ai::AiUnavailable,
    },
    /// `PlayVideo` (`ControlGui::playMovie`): play the movie `name` in the mission's video window. The application starts it unless
    /// one is already playing (`if (moviePlaying) return;`, controlgui.cpp:2858).
    PlayVideo {
        /// The script's file name (any directory and extension are dropped by the application, as `_splitpath` does).
        name: String,
    },
    /// `TutorialText` (`MissionInterfaceManager::setTutorialText`): a tutorial line for the chat log. The application resolves the
    /// string resource, and shows it under the sender `IDS_TUTORIAL` (`mission::natives_presentation`).
    TutorialText {
        /// The `StringTable` id the script passed.
        text_id: i32,
    },
    /// `setradio` (`execSetRadio`, `code/ablmc2.cpp:2672-2701`): the pilot's radio is turned on or off (`Radio::turnOn` /
    /// `turnOff`). `pilot` is the pool slot, the id [`SimEvent::Radio`] uses. Appended: lane audio.
    RadioPower {
        /// Pool slot of the pilot.
        pilot: u32,
        /// `true` for `turnOn`.
        on: bool,
    },
}

/// Queue a positional `playDigitalSample` request for an object-triggered mission sound.
pub(crate) fn emit_sound(
    world: &mut crate::World,
    object: ObjectHandle,
    id: i32,
    allow_dupes: bool,
) {
    let Some(at) = world.objects.get(object).map(|obj| obj.c.position) else {
        return;
    };
    let source = world.objects.watch_id(object);
    world.events.push(SimEvent::Sound {
        id,
        source,
        at: Some(at),
        allow_dupes,
    });
}

pub(crate) fn emit_sound_at(
    world: &mut crate::World,
    source: WatchId,
    at: Vec3,
    id: i32,
    allow_dupes: bool,
) {
    world.events.push(SimEvent::Sound {
        id,
        source,
        at: Some(at),
        allow_dupes,
    });
}

/// One `SetCamera*` native call, in the units the script passed (`code/ablmc2.cpp:5339-5765`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CameraRequest {
    /// `SetCameraPosition(camPos)` -> `eye->setPosition(pos, false)`.
    Position(Vec3),
    /// `SetCameraGoalPosition(camPos, time)` -> `setGoalPosition` + `setGoalPosTime`.
    GoalPosition(Vec3, f32),
    /// `SetCameraRotation(camRot)` -> `eye->setRotation` (x = tilt, y = rotation, z = altitude).
    Rotation(Vec3),
    /// `SetCameraGoalRotation(camRot, time)` -> `setGoalRotation` + `setGoalRotTime`.
    GoalRotation(Vec3, f32),
    /// `SetCameraZoom(fov)` -> `setFieldOfView`.
    Zoom(f32),
    /// `SetCameraGoalZoom(fov, time)` -> `setFieldOfViewGoal`.
    GoalZoom(f32, f32),
    /// `SetCameraVelocity(camVel)` -> `setVelocity`.
    Velocity(Vec3),
    /// `SetCameraGoalVelocity(camVel, time)` -> `setGoalVelocity` + `setGoalVelTime`.
    GoalVelocity(Vec3, f32),
    /// `SetCameraLookObject(partId)` -> `setCameraTargetId`.
    LookObject(i32),
}

impl From<CameraRequest> for SimEvent {
    fn from(r: CameraRequest) -> Self {
        SimEvent::Camera(r)
    }
}

/// The camera state the `GetCamera*` natives read (`code/ablmc2.cpp:5312-5810`). The application owns the camera and refreshes this
/// before each step; see [`crate::World::camera`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraState {
    /// `Camera::position`.
    pub position: Vec3,
    /// `Camera::goalPosition`.
    pub goal_position: Vec3,
    /// `Camera::getRotation`: x = `projectionAngle`, y = `cameraRotation`, z = 0.
    pub rotation: Vec3,
    /// `Camera::goalRotation`.
    pub goal_rotation: Vec3,
    /// `Camera::velocity`.
    pub velocity: Vec3,
    /// `Camera::goalVelocity`.
    pub goal_velocity: Vec3,
    /// `Camera::camera_fov`.
    pub fov: f32,
    /// `Camera::goalFOV`.
    pub goal_fov: f32,
    /// `Camera::lookTargetObject` (-1 none).
    pub look_object: i32,
}

impl Default for CameraState {
    fn default() -> Self {
        let zero = Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        };
        CameraState {
            position: zero,
            goal_position: zero,
            rotation: zero,
            goal_rotation: zero,
            velocity: zero,
            goal_velocity: zero,
            fov: 40.0,
            goal_fov: 0.0,
            look_object: -1,
        }
    }
}

/// What the application reports back to the mission scripts through the sound system (`getCurrentMusicId`,
/// `isPlayingVoiceOver`, `playBettySample`'s result). The sound system belongs to the application, which stores a **frozen
/// snapshot** of its state here before each step ([`crate::World::audio`]); the snapshot is plain data, so a recorded frame
/// replays to the same script outcomes whatever the sound system does afterwards. The audio natives move the snapshot at once
/// where the C++ `SoundSystem` moves its own state in the same call (`currentMusicId`, the stream handles, the Betty channel), so a
/// script reads back what it just asked for, in any order of calls within one execution. The default is "no sound": no tune, no
/// voice-over, no Betty sample, so `playBettySample` answers -1 exactly as the C++ does with `useSound` off.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioReadback {
    /// `SoundSystem::currentMusicId` (`getCurrentMusicId`), -1 when no tune is playing.
    pub music_id: i64,
    /// `SoundSystem::isPlayingVoiceOver` (`stream3Handle` set): a voice-over file is playing.
    pub voice_over: bool,
    /// Whether `playBettySample` would take the Betty channel: no Betty line is loaded (`bettySoundBite == NULL`).
    pub betty_free: bool,
    /// Number of Betty samples in the pak; a sample id at or past it is refused (`bettySampleId >= numBettySamples`).
    pub betty_samples: u32,
    /// `useMusic && useSound`: `playDigitalMusic` can start a tune (`mclib/soundsys.cpp:673-690`).
    pub music_enabled: bool,
    /// Both music streams are active, so a new tune is refused (`mclib/soundsys.cpp:742`). `stopmusic` clears it.
    pub music_streams_busy: bool,
    /// `useSound && !supportSampleLoaded`: `playSupportSample` accepts a voice-over file.
    pub voice_available: bool,
    /// A voice-over stream is active, so a second file is refused (`playDigitalStream`). `stopvoiceover` clears it.
    pub voice_busy: bool,
    /// Whether the tune / voice-over files exist: a property of the immutable game data, not of the sound system's state.
    pub files: AudioFiles,
}

impl AudioReadback {
    /// The sound system of the **headless C++ oracle**, as a sweep switch-off cell sees it (DIFFERENCES `ORACLE-HEADLESS-AUDIO`).
    /// The oracle runs the real `SoundSystem` (`useSound`, `useMusic` true, the real tune table and files) but drives the game with
    /// `DoGameLogic` only: `SoundSystem::update` (`mechcmd2.cpp:2108`, the render path) never runs, and the gos audio stubs hand out
    /// no stream handles, so no stream ever becomes active. A tune the script starts therefore sets `currentMusicId`
    /// (`soundsys.cpp:713`) and nothing ever clears it (`soundsys.cpp:631` is in `update`), and a second tune is never refused for
    /// busy streams. A script that polls `getCurrentMusicId == -1` stops after its first tune for the rest of the run. The real game
    /// ends a tune when its file ends, so the shipped game never uses this; Betty and voice-over keep the "no sound" answers.
    pub fn cpp_headless_oracle(files: AudioFiles) -> Self {
        AudioReadback {
            music_enabled: true,
            files,
            ..AudioReadback::default()
        }
    }
}

/// Whether a sound file exists in the game data (`fileExists`). The data set is immutable while the game runs, so the answer is a
/// pure function of the request and does not break replay.
pub trait AudioFileSet {
    /// The music table has a tune for `id` and its `.wav` exists.
    fn music_file(&self, id: i64) -> bool;
    /// The voice-over file `name` (`sound/<stem>.wav`) exists.
    fn voice_file(&self, name: &str) -> bool;
}

/// A shared [`AudioFileSet`], or none (no files exist).
#[derive(Clone, Default)]
pub struct AudioFiles(Option<std::rc::Rc<dyn AudioFileSet>>);

impl AudioFiles {
    /// No sound data: nothing exists.
    pub fn none() -> Self {
        Self(None)
    }
    pub fn new(set: std::rc::Rc<dyn AudioFileSet>) -> Self {
        Self(Some(set))
    }
    pub fn music_file(&self, id: i64) -> bool {
        self.0.as_ref().is_some_and(|q| q.music_file(id))
    }
    pub fn voice_file(&self, name: &str) -> bool {
        self.0.as_ref().is_some_and(|q| q.voice_file(name))
    }
}

impl std::fmt::Debug for AudioFiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "AudioFiles(some)"
        } else {
            "AudioFiles(none)"
        })
    }
}

impl PartialEq for AudioFiles {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(a), Some(b)) => std::rc::Rc::ptr_eq(a, b),
            _ => false,
        }
    }
}
impl Eq for AudioFiles {}

impl Default for AudioReadback {
    fn default() -> Self {
        AudioReadback {
            music_id: -1,
            voice_over: false,
            betty_free: false,
            betty_samples: 0,
            music_enabled: false,
            music_streams_busy: false,
            voice_available: false,
            voice_busy: false,
            files: AudioFiles::none(),
        }
    }
}

/// The mission interface's callout animation state (`MissionInterfaceManager::animationRunning`), the state `incallout` reads
/// (`code/missiongui.h:288`) and `animationcallout` would start (`execAnimationCallout`, `code/ablmc2.cpp:5810-5830`). The mission
/// interface belongs to the application, which stores its state here before each step ([`crate::World::callout`]); the
/// `incallout` and `animationcallout` natives read and start the presentation's own callout
/// (`natives_presentation::Callout`), so this legacy readback stays at its default.
/// Appended: lane audio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalloutState {
    /// `animationRunning`: a callout animation is running (`incallout` answers true).
    pub running: bool,
    /// The button the running animation scrolls to (`targetButtonId`): a callout for another id does not restart it.
    pub target_button: i32,
}
