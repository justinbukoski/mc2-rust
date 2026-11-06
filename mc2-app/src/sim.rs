// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The seam between the application and the simulation.
//!
//! `mc2-sim` is driven exactly like `mc2_sim::Sim::step(FrameInput { frame_rate, now_ms, visibility,
//! commands })` (see `rust/mc2-sim/DESIGN.md` sections 2, 3, 5 and 7): one call per game frame, at a
//! fixed rate, with the camera visibility computed by the application and the player commands of
//! that frame.  The application owns the clock that produces `frame_rate`/`now_ms`
//! ([`SimClock`], the `FixedStep` arithmetic of `mc2-sim`'s `time.rs`) and the camera that produces
//! the visibility ([`crate::visibility`]).
//!
//! The game runs the live [`crate::live_sim::LiveSim`] (`mc2_sim::Sim` with `Command =
//! PlayerCommand`, `Event = SimEvent`, `World = mc2_sim::World`); [`NullSim`] remains for viewer
//! tests that need no simulation.

use crate::camera::CameraRig;
use crate::mission_view::MissionView;
use crate::visibility::CameraVisibility;
use crate::visibility::ObjectVisibility;
use mc2_platform::clock::FixedClock;
use mc2_render::math::Vec3;
use mc2_render::scene::{SceneRenderer, ShapeInstance};
use std::rc::Rc;
use std::time::Duration;

/// `mis_PLAYING` (`code/mission.h`): the mission is still running.
pub const MIS_PLAYING: u32 = 0;

/// Inputs of one simulation frame: `mc2_sim::FrameInput` field for field.
pub struct FrameInput<C> {
    /// `frameRate` (`frameLength = 1 / frameRate`).
    pub frame_rate: f32,
    /// Virtual `timeGetTime()` in ms (feeds `scenarioTime`).
    pub now_ms: u32,
    /// Camera-derived inputs of this frame (`land->geometry()`, appearance `inView`).  P2 wraps
    /// it in an `impl mc2_sim::visibility::Visibility`.
    pub visibility: Rc<CameraVisibility>,
    /// Player orders issued this frame (`missionInterface->update()`).
    pub commands: Vec<C>,
    /// The player's keys the GUI read this frame and the simulation acts on: ESC in movie mode (`ForceMovieEnd`) and the
    /// mouse profile the player set (`GUIIsAOEStyle`). Like `commands`, they are player input, which the oracle pins as inputs.
    pub presentation: PresentationCommands,
    /// The camera as the mission script reads it (`GetCamera*`), after this frame's `eye->update()`.
    pub camera: CameraReadback,
    /// The sound system as the mission script reads it (`getCurrentMusicId`, `isPlayingVoiceOver`, `playBettySample`).
    pub audio: mc2_sim::events::AudioReadback,
    /// The mission interface's callout animation as `incallout` reads it (`animationRunning`).
    pub callout: mc2_sim::events::CalloutState,
}

/// The player's presentation input of one frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentationCommands {
    /// The player pressed ESC while the movie ran (`MissionInterfaceManager::update`, `missiongui.cpp:443-446`).
    pub force_movie_end: bool,
    /// The mouse profile the player set, when it changed (`useLeftRightMouseProfile`).
    pub aoe_style: Option<bool>,
}

/// The camera state mission scripts can read back (`mc2_sim::events::CameraState` in the app's types).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraReadback {
    pub position: Vec3,
    pub goal_position: Vec3,
    /// `(projectionAngle, cameraRotation, 0)`.
    pub rotation: Vec3,
    pub goal_rotation: Vec3,
    pub velocity: Vec3,
    pub goal_velocity: Vec3,
    pub fov: f32,
    pub goal_fov: f32,
    /// `lookTargetObject` (-1 none).
    pub look_object: i32,
}

impl Default for CameraReadback {
    fn default() -> Self {
        Self::from_rig(&CameraRig::new(
            mc2_render::camera::CameraSettings::default(),
            &Default::default(),
            Default::default(),
            [(1200.0, 35.0); 4],
            (800.0, 600.0),
        ))
    }
}

impl CameraReadback {
    pub fn from_rig(r: &CameraRig) -> Self {
        Self {
            position: r.position,
            goal_position: r.goal_position,
            rotation: Vec3::new(r.projection_angle, r.rotation, 0.0),
            goal_rotation: r.goal_rotation,
            velocity: r.velocity,
            goal_velocity: r.goal_velocity,
            fov: r.fov,
            goal_fov: r.goal_fov,
            look_object: r.look_target,
        }
    }
}

/// What a frame produced: `mc2_sim::FrameOutput` field for field.
#[derive(Debug)]
pub struct FrameOutput<E> {
    /// Presentation events for render, fx and audio.
    pub events: Vec<E>,
    /// `scenarioResult`; anything but [`MIS_PLAYING`] ends the mission.
    pub result: u32,
}

impl<E> Default for FrameOutput<E> {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            result: MIS_PLAYING,
        }
    }
}

/// A mover as the camera needs it for `Mech3DAppearance::recalcBounds`: its part id (the
/// `Visibility::mover_in_view` key), position and, from its appearance, the bounds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverPosition {
    pub part_id: i32,
    pub position: Vec3,
    /// `None` until the appearance lane supplies them: the screen test then uses the position
    /// alone (a zero-size box).
    pub bounds: Option<MoverBounds>,
    /// Shapes and camera thresholds from the appearance type. The producer selects the LOD.
    pub lod_profile: Option<LodProfile>,
    /// Destroyed appearances keep their current shape and do not select a new LOD.
    pub lod_active: bool,
    /// `Mech3DAppearance::currentLOD`, retained while the appearance is off screen.
    pub current_lod: usize,
}

/// A structure as the camera visibility producer sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StructurePosition {
    pub handle: u32,
    pub position: Vec3,
    pub rotation: f32,
    pub bounds: Option<MoverBounds>,
    /// `bldgShape->GetExtentRadius()` for the current shape.
    pub extent_radius: f32,
    /// Shapes and camera thresholds from the appearance type. The producer selects the LOD.
    pub lod_profile: Option<LodProfile>,
    /// Destroyed and disabled structures do not select a new LOD.
    pub lod_active: bool,
    /// `BldgAppearance::currentLOD`, retained while the appearance is off screen.
    pub current_lod: usize,
}

/// Camera LOD inputs loaded from an appearance type (`lodDistance[]`, shape availability).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodProfile {
    pub distances: [f32; mc2_sim::appearance::tables::MAX_LODS],
    pub available: [bool; mc2_sim::appearance::tables::MAX_LODS],
    pub high_detail: bool,
}

impl LodProfile {
    /// `recalcBounds`: choose the furthest available LOD whose threshold is below the eye distance.
    pub fn select(self, eye_distance: f32) -> usize {
        if !self.high_detail {
            return usize::from(self.available[1]);
        }
        let mut selected = 0;
        for lod in 1..self.available.len() {
            if self.available[lod] && eye_distance > self.distances[lod] {
                selected = lod;
            }
        }
        selected
    }
}

/// The appearance data `recalcBounds` reads (`mechType->bounds*`, `typeUpperLeft/LowerRight`,
/// `baseRootNodeDifference`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverBounds {
    /// `boundsUpperLeftX/Y` and `boundsLowerRightX/Y`: screen offsets at scale 1.
    pub screen_upper_left: (f32, f32),
    pub screen_lower_right: (f32, f32),
    /// `typeUpperLeft` / `typeLowerRight`: the selection box corners relative to the position.
    pub box_upper_left: Vec3,
    pub box_lower_right: Vec3,
    /// `baseRootNodeDifference` (animated root node height offset; jumping mechs).
    pub base_root_node_difference: f32,
}

/// The simulation as the application sees it: `mc2_sim::Sim` behind a thin adapter.
pub trait SimDriver {
    /// Player orders (`mc2_sim::command::PlayerCommand`).
    type Command;
    /// Presentation events (`mc2_sim::events::SimEvent`).
    type Event;
    /// The read-only state presentation looks at between frames (`mc2_sim::World`).
    type World;

    /// `true` only for the production `mc2_sim::Sim` adapter.  The production
    /// headless route exposes this without constructing a GPU session, so an
    /// unavailable P3 replay still reports its first app seam on machines
    /// without an offscreen adapter.
    const IS_LIVE_SIM: bool = false;

    /// One game frame: `Sim::step(FrameInput) -> FrameOutput`.
    fn step(&mut self, input: FrameInput<Self::Command>) -> FrameOutput<Self::Event>;

    /// The reason the most recent [`Self::step`] could not run (an incomplete mission load), taken
    /// once; `None` when it ran.  A failed step is an error, never a playing mission.
    fn take_failure(&mut self) -> Option<String> {
        None
    }

    /// The player's `useNonWeaponEffects` preference (`prefs.useNonWeaponEffects`): the simulation raises the effects that preference
    /// gates (footprints, dust, smoke, wakes) only while it is on. Called before every step; stand-in drivers ignore it.
    fn set_use_non_weapon_effects(&mut self, _on: bool) {}

    /// `sim.world`, read-only.
    fn world(&self) -> &Self::World;

    /// Mission clock after the most recent step, when available.
    fn scenario_time(&self) -> f32 {
        0.0
    }

    /// Mission turn after the most recent step, when available.
    fn turn(&self) -> i64 {
        0
    }

    /// Whether this is the live `mc2_sim::Sim` adapter rather than the P1
    /// viewer stand-in.  P3 uses this production-path capability rather than
    /// treating a successful state transition from `NullSim` as gameplay.
    fn is_live_sim(&self) -> bool {
        Self::IS_LIVE_SIM
    }

    /// The command that pauses or resumes the simulation (the GUI's pause toggle), if the
    /// simulation takes one.
    fn pause_command(&self, _paused: bool) -> Option<Self::Command> {
        None
    }

    /// `terminationResult` (`Termination::result` in `mc2-sim`): the result the mission is heading for,
    /// 0 while it is not set and -1 while the objectives are not evaluated. The game sound does not play
    /// a tune from it: the win or lose tune is the mission script's request (see `crate::audio::tune`).
    fn termination_result(&self) -> i32 {
        0
    }

    /// Positions of the movers (from [`Self::world`]), for the camera's `inView` test.  Called
    /// once per frame before [`Self::step`].
    fn mover_positions(&self) -> Vec<MoverPosition> {
        Vec::new()
    }

    /// Positions of structures (from [`Self::world`]), for camera `inView` and LOD inputs.
    fn structure_positions(&self) -> Vec<StructurePosition> {
        Vec::new()
    }

    /// The production renderer's object `recalcBounds` result for this camera frame. `None`
    /// retains the explicit-unavailable fallback used by viewer-only drivers.
    fn object_visibility(
        &mut self,
        _scene: &SceneRenderer,
        _camera: &mc2_render::camera::GameCamera,
        _terrain: &CameraVisibility,
    ) -> Option<ObjectVisibility> {
        None
    }

    /// Adds the simulation's dynamic objects (mechs, vehicles, bolts) to the frame being drawn,
    /// read from [`Self::world`].  `alpha` is the fraction of the way to the next frame (render
    /// interpolation, never fed back into the simulation); `view` holds the static instances and
    /// the mission's light.
    fn append_instances(
        &mut self,
        _scene: &mut SceneRenderer,
        _view: &MissionView,
        _alpha: f32,
        _out: &mut Vec<ShapeInstance>,
    ) {
    }

    /// Instances drawn whether or not the camera's cone test sees their object position: they are in view by definition
    /// (`setVisibility(true, true)`), like an airstrike's bomber, whose mesh is nowhere near the position it is placed at.
    /// [`crate::game::GameCore::render`] still chooses their shape, then leaves `in_view` set.
    fn append_forced_instances(
        &mut self,
        _scene: &SceneRenderer,
        _view: &MissionView,
        _out: &mut Vec<ShapeInstance>,
    ) {
    }

    /// The queued waypoint paths of the selected home movers (`MechWarrior::drawWaypointPath`), by part id.
    fn waypoint_paths(&self, _selected: &[i32]) -> Vec<crate::waypoints::WaypointPath> {
        Vec::new()
    }

    /// Whether the static instance `index` of [`MissionView::instances`] is replaced by what
    /// [`Self::append_instances`] draws (movers) or hidden (a destroyed structure).
    fn hides_static_instance(&self, _index: usize) -> bool {
        false
    }

    /// `getCamObject(partId, true)` for the camera's `lookTargetObject`: the position of the
    /// object the camera may follow, if it can.
    fn camera_target(&self, _part_id: i32) -> Option<Vec3> {
        None
    }

    /// The `SetCamera*` calls the mission script made in the last [`Self::step`], taken once.
    fn camera_requests(&mut self) -> Vec<mc2_sim::events::CameraRequest> {
        Vec::new()
    }

    /// The `PlayVideo` and `TutorialText` requests among this frame's events, for [`crate::presentation`]. A driver whose
    /// events are not simulation events has none.
    fn presentation_requests(&self, _events: &[Self::Event]) -> Vec<mc2_sim::events::SimEvent> {
        Vec::new()
    }

    /// The presentation state the mission script reads and the application draws (movie mode, the letterbox, the fade, the
    /// GUI freeze, the callout, the mouse profile), or `None` for a driver that has no mission script. Built from `world`.
    fn sim_presentation(
        &self,
        _world: &Self::World,
    ) -> Option<mc2_sim::mission::natives_presentation::Presentation> {
        None
    }

    /// The mission HUD's inputs read off [`Self::world`] (plain data, no simulation types), or
    /// `None` when this driver has no mission state to show: the HUD is then not built and not
    /// drawn (a viewer, not a game).
    fn hud_snapshot(&mut self) -> Option<crate::hud_data::HudSnapshot> {
        None
    }

    /// Home-team visual line of sight and `GameMap` passability at a world point
    /// (`missiongui.cpp:745-754`, `canJumpToWPos` at `:3500-3551`). `None` when this driver has
    /// no mission map (the viewer).
    fn cursor_world_facts(&mut self, _pos: mc2_sim::math::Vec3) -> Option<CursorWorldFacts> {
        None
    }

    /// The mission text messages (`DisplayTextMessage` objective actions) the last
    /// [`Self::step`] produced, taken once.
    fn hud_messages(&mut self) -> Vec<crate::hud_data::HudMessage> {
        Vec::new()
    }

    /// `ControlGui::resultsDone()` became true: the in-mission objective results have finished, so a
    /// mission whose termination timer ran out may end (`mission.cpp:569`).  A driver without a
    /// termination timer ignores it.
    fn results_screen_done(&mut self) {}

    /// What the finished mission reports to the campaign (`Outcome`: result, objectives, movers,
    /// mechs), read once when the mission ends; `None` for a driver that has none.
    fn mission_outcome(&mut self) -> Option<mc2_campaign::interface::Outcome> {
        None
    }

    /// The in-mission save (`Mission::save`, `code/saveload.cpp:249`): the mission's dynamic state as a
    /// new Rust-format byte string, or `None` for a driver that has no mission state to save.
    fn save_mission(&self) -> Option<Vec<u8>> {
        None
    }

    /// The in-mission load (`Mission::load`, `code/saveload.cpp:627`): re-initialises the mission and
    /// lays `bytes` over it. An unreadable save leaves the running mission as it was.
    fn load_mission(
        &mut self,
        _game: &mut mc2_render::game::GameData,
        _bytes: &[u8],
    ) -> Result<(), String> {
        Err("this driver has no mission state to load".into())
    }

    /// Unported or unavailable simulation behaviour reached so far (for reports and logs).
    fn unavailable_summary(&self) -> Vec<String> {
        Vec::new()
    }

    /// The `object-visibility` line of the headless visibility trace header.
    fn visibility_note(&self) -> String {
        "object-visibility=unavailable driver=NullSim (I/S/LM/LS are incomplete)".into()
    }
}

/// Cursor facts at one world point, from the live simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorWorldFacts {
    /// `Team::home->teamLineOfSight(wPos, 0)` (`missiongui.cpp:753`).
    pub line_of_sight: bool,
    /// `GameMap->getPassable` after `IsGameSelectTerrainPosition` (`missiongui.cpp:3502-3507`).
    pub passable: bool,
}

/// A driver that does nothing: the mission viewer of milestone P1.
#[derive(Debug, Default)]
pub struct NullSim {
    /// Frames stepped so far.
    pub frames: u64,
    /// The visibility of the last frame (for tests and diagnostics).
    pub last_visibility: Option<Rc<CameraVisibility>>,
}

impl SimDriver for NullSim {
    type Command = ();
    type Event = ();
    type World = ();

    fn step(&mut self, input: FrameInput<()>) -> FrameOutput<()> {
        self.frames += 1;
        self.last_visibility = Some(input.visibility);
        FrameOutput::default()
    }

    fn world(&self) -> &() {
        &()
    }
}

/// Receives each frame's presentation events and read-only world (render effects, gosFX,
/// sound, radio, Betty).  Sinks never mutate the simulation.
pub trait SimEventSink<D: SimDriver> {
    fn mission_start(&mut self, _world: &D::World) {}
    fn mission_end(&mut self) {}
    fn on_frame(&mut self, ctx: &FrameContext<'_, D>, events: &[D::Event]);
}

/// State shared with presentation sinks after one fixed simulation step.
pub struct FrameContext<'a, D: SimDriver> {
    pub frame: u64,
    pub frame_length: f32,
    pub turn: i64,
    pub scenario_time: f32,
    pub listener: Vec3,
    pub listener_rotation: f32,
    pub world: &'a D::World,
}

/// The fixed-step game clock: real time in, whole simulation frames out.
///
/// The render loop runs as fast as it likes; the simulation advances at `hz` frames per second
/// with `frameLength = 1 / hz` and the virtual millisecond clock of `mc2_sim::time::FixedStep`
/// (`now_ms(f) = 100000 + round(f * 1000 / hz)`), so a run is reproducible however fast it is drawn.
#[derive(Clone, Debug)]
pub struct SimClock {
    hz: u32,
    clock: FixedClock,
    frame: u64,
}

impl SimClock {
    pub fn new(hz: u32) -> Self {
        let hz = hz.clamp(1, 1000);
        Self {
            hz,
            // A frame that took longer than 8 ticks drops the rest (spiral-of-death guard).
            clock: FixedClock::from_hz(hz).with_max_steps(8),
            frame: 0,
        }
    }

    pub fn hz(&self) -> u32 {
        self.hz
    }

    /// `frameLength` in seconds.
    pub fn frame_length(&self) -> f32 {
        1.0 / self.hz as f32
    }

    /// Feeds real time; returns how many simulation frames are due.
    pub fn advance(&mut self, real_dt: Duration) -> u32 {
        self.clock.advance(real_dt)
    }

    /// Number of the next frame (0-based).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// `(frame, frame_rate, now_ms)` for the next simulation frame; advances the frame counter.
    ///
    /// The virtual `timeGetTime()` of the frame numbered `f` (0-based) is `now_ms(hz, f + 1)`: the
    /// clock has already advanced one frame when the first `Mission::update` runs, which is how the
    /// oracle drives its frames 1.. (`baseMs + round(f * 1000 / fps)`), so a run is comparable with
    /// the oracle frame for frame.
    pub fn next_frame(&mut self) -> (u64, f32, u32) {
        let f = self.frame;
        self.frame += 1;
        (f, self.hz as f32, now_ms(self.hz, f + 1))
    }

    /// Fraction of the way to the next frame, for render interpolation.
    pub fn alpha(&self) -> f32 {
        self.clock.alpha()
    }

    /// Simulated time in seconds (frames done plus the interpolation fraction).
    pub fn game_seconds(&self) -> f32 {
        self.game_seconds_exact() as f32
    }

    /// [`Self::game_seconds`] before the rounding to f32.
    pub fn game_seconds_exact(&self) -> f64 {
        self.frame as f64 / self.hz as f64 + self.clock.alpha() as f64 / self.hz as f64
    }
}

/// `FixedStep::now_ms`: `100000 + round(frame * 1000 / fps)` (rounded half away from zero, as
/// `f64::round`).
pub fn now_ms(fps: u32, frame: u64) -> u32 {
    100_000 + (frame as f64 * 1000.0 / fps as f64).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_clock_matches_fixed_step() {
        // At 30 fps scenarioTime runs 0, 0.034, 0.067 ... (whole ms rounding), DESIGN.md section 3.
        assert_eq!(now_ms(30, 0), 100_000);
        assert_eq!(now_ms(30, 1), 100_033);
        assert_eq!(now_ms(30, 2), 100_067);
        assert_eq!(now_ms(30, 3), 100_100);
    }

    #[test]
    fn exact_game_time_keeps_water_moving_at_large_timestamps() {
        // 2^20 s into a session at 30 Hz: the f32 time cannot tell adjacent frames apart, the
        // exact time (which the enhanced water reads) can.
        let mut c = SimClock::new(30);
        c.frame = 30 * 1_048_576;
        let (a32, a) = (c.game_seconds(), c.game_seconds_exact());
        c.frame += 1;
        let (b32, b) = (c.game_seconds(), c.game_seconds_exact());
        assert_eq!(a32, b32, "f32 time stalls");
        assert!(b > a);
        let wa = mc2_render::visual::water_blend(a, 10.0, 8);
        let wb = mc2_render::visual::water_blend(b, 10.0, 8);
        assert_ne!(wa.t, wb.t, "water phase must advance between frames");
        assert_ne!(
            mc2_render::visual::spray_offset(a),
            mc2_render::visual::spray_offset(b)
        );
    }

    #[test]
    fn render_rate_is_decoupled_from_sim_rate() {
        let mut c = SimClock::new(30);
        // 60 render frames per second: one sim frame every other render frame.
        let dt = Duration::from_nanos(1_000_000_000 / 60);
        let ticks: u32 = (0..60).map(|_| c.advance(dt)).sum();
        assert!((29..=30).contains(&ticks), "{ticks}");
        // 15 render frames per second: two sim frames per render frame.
        let mut c = SimClock::new(30);
        let dt = Duration::from_nanos(1_000_000_000 / 15);
        let ticks: Vec<u32> = (0..15).map(|_| c.advance(dt)).collect();
        assert_eq!(ticks.iter().sum::<u32>(), 30);
        assert!(ticks.iter().all(|&t| t == 2));
        assert!((c.frame_length() - 1.0 / 30.0).abs() < 1e-9);
    }

    #[test]
    fn frames_are_numbered_from_zero() {
        let mut c = SimClock::new(30);
        assert_eq!(c.next_frame(), (0, 30.0, 100_033));
        assert_eq!(c.next_frame(), (1, 30.0, 100_067));
        assert_eq!(c.frame(), 2);
    }

    #[test]
    fn null_sim_keeps_playing() {
        let mut s = NullSim::default();
        let r = s.step(FrameInput {
            frame_rate: 30.0,
            now_ms: 100_000,
            visibility: Rc::new(CameraVisibility::default()),
            commands: Vec::new(),
            presentation: PresentationCommands::default(),
            camera: CameraReadback::default(),
            audio: Default::default(),
            callout: Default::default(),
        });
        assert_eq!(r.result, MIS_PLAYING);
        assert_eq!(s.frames, 1);
    }
}
