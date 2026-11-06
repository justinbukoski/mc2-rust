// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The game core shared by the windowed and the headless front ends: input routing, the state
//! machine, the mission runtime (fixed-step clock, camera, visibility, sim driver) and frame
//! composition.  Neither front end has its own game logic; they only differ in where events come
//! from (winit or a script) and where the frame goes (a surface or an offscreen texture).

use crate::audio::{self, AppAudio, Cue};
use crate::camera::{
    CameraData, CameraPrefs, CameraRig, Ground, MissionGround, INITIAL_FRAME_LENGTH,
};
use crate::hotkeys::{self, Command, Fired};
use crate::hud::{HudFrameInput, HudRuntime};
use crate::layers::{
    CursorLayer, CursorRequest, DrawContext, NoOrders, OrderContext, OrderHook, UiFrame, UiLayer,
};
use crate::mission_view::MissionView;
use crate::movie_control::MovieControl;
use crate::orders::BridgeOrders;
use crate::overlay::{OverlayBatch, OverlayPainter};
use crate::presentation::AppPresentation;
use crate::sim::{CameraReadback, FrameInput, SimClock, SimDriver, SimEventSink, MIS_PLAYING};
use crate::state::{GameState, StateMachine, StateRequest};
use crate::ui_host::UiHost;
use crate::userinput::UserInput;
use crate::visibility::{self, CameraVisibility, ViewSnapshot};
use mc2_platform::input::{InputEvent, InputState, KeyStatus};
use mc2_platform::keys;
use mc2_platform::viewport::{Fit, Viewport};
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::gpu::{Gpu, RenderTarget};
use mc2_render::mission::PaintSource;
use mc2_render::populate::select_shapes;
use mc2_render::scene::{Frame, SceneRenderer, ShapeInstance};
use mc2_render::weather::{Weather, WeatherEvent};
use mc2_sim::PlayerCommand;
use mc2_ui::pause_window::PauseIntent;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// The in-mission save file in the save directory (`testgame.ims` in the original; the format is new).
pub const MISSION_SAVE_FILE: &str = "testgame.mc2s";

/// `INVALID_GUI` (`mc2-audio` ids): the refusal sound.
const INVALID_GUI: i32 = mc2_audio::ids::sfx::INVALID_GUI as i32;

/// How the pause panel ends the mission.
enum PauseExit {
    /// RESTART answered Yes: reload the same mission and launch configuration.
    Restart,
    /// EXIT answered Yes: the application ends.
    Quit,
}

/// Input events the HUD may hold between two game frames.
const HUD_EVENT_LIMIT: usize = 8192;

/// Settings of a run.
#[derive(Clone, Debug)]
pub struct GameConfig {
    /// Mission to start straight away (`-mission` on the original command line).
    pub mission: Option<String>,
    /// Simulation frames per second.
    pub sim_hz: u32,
    /// Draw the camera between simulation frames (smooth motion at high refresh rates).
    pub interpolate_camera: bool,
    /// Sun shadows from the shapes (a modern addition of `mc2-render`).
    pub shadows: bool,
    /// Draw the stand-in cursor while the pointer is over the window.
    pub cursor: bool,
    /// Seed of both simulation random streams (the original seeds from the clock; a fixed seed
    /// makes runs reproducible and comparable with the oracle).
    pub sim_seed: u32,
    /// Random number algorithm of the simulation (`--rng`).
    pub sim_rng: mc2_sim::rng::RngMode,
    /// Presentation-only visual upgrades (`GraphicsSettings::visual`); off by default.
    pub visual: mc2_render::visual::VisualOptions,
    /// The home commander's colours from a campaign launch (`ForceMech::colors`); `None` takes
    /// `options.cfg`.  The UI host sets this for a campaign launch (`UiHost::launch_campaign_paint`);
    /// direct `--mission` launches leave it `None`.  A `Some` without colours is unavailable, not a
    /// fallback.
    pub campaign_paint: Option<PaintSource>,
    /// The campaign launch (`Logistics::beginMission`) the live simulation builds its force,
    /// difficulty, ammo option and resource points from; `None` is a `--mission` quickstart.  The
    /// UI host's launch sets it when the mission loads.
    pub campaign_launch: Option<mc2_campaign::LoadSpec>,
    /// Start in the resolution saved in `options.cfg` (the windowed front end without `--size`).
    pub use_saved_resolution: bool,
}

impl Default for GameConfig {
    fn default() -> Self {
        Self {
            mission: None,
            sim_hz: 30,
            interpolate_camera: true,
            shadows: true,
            cursor: true,
            sim_seed: 1,
            sim_rng: mc2_sim::rng::RngMode::LegacyMsvc,
            visual: Default::default(),
            campaign_paint: None,
            campaign_launch: None,
            use_saved_resolution: false,
        }
    }
}

/// A running mission.
pub struct MissionRuntime<D: SimDriver> {
    pub name: String,
    pub view: MissionView,
    pub rig: CameraRig,
    pub clock: SimClock,
    pub driver: D,
    pub user_input: UserInput,
    /// `turn`: game frames run (incremented at the start of each).
    pub turn: i64,
    pub paused: bool,
    /// Visibility handed to the last simulation frame.
    pub visibility: Rc<CameraVisibility>,
    /// `scenarioResult` of the last frame.
    pub result: u32,
    /// Set once the sim has produced a presentation event that the sound cannot play yet
    /// ([`audio::sim_events`] refuses it); the mission says so once.
    pub sim_sound_refused: bool,
    /// The mission's weather (`[Weather]`), when it has one with drops (weather.cpp `Weather::init`).
    pub weather: Option<Weather>,
    /// The weather's sound events since the audio last took them ([`play_weather_event`]).
    pub weather_events: Vec<WeatherEvent>,
    /// The mission HUD on the live simulation state, when the driver has mission state to show.
    pub hud: Option<HudRuntime>,
    /// The presentation natives' application half: the mission video window, the callout, the GUI gate and the
    /// letterbox/fade overlay (`code/ablmc2.cpp`, see [`crate::presentation`]).
    pub presentation: AppPresentation,
    /// The simulation's presentation state after the last frame (the movie mode, bars, fade, callout, freeze and profile).
    pub sim_presentation: mc2_sim::mission::natives_presentation::Presentation,
    /// The mouse profile handed to the simulation last (sent again when the player changes it).
    pub last_aoe_style: Option<bool>,
    /// What the pause panel asked for since the game last acted on it (save, load, options, abort, quit).
    pub pause_actions: Vec<mc2_ui::pause_window::PauseIntent>,
    /// `GameSoundSystem` radio callbacks: pilots whose current audio fragment is playing.
    pub speaking_pilots: std::collections::BTreeSet<u32>,
    prev_camera: GameCamera,
    cur_camera: GameCamera,
}

impl<D: SimDriver> MissionRuntime<D> {
    /// The camera to draw with: between the last two simulation frames by `alpha`.
    pub fn render_camera(&self, viewport: (f32, f32), interpolate: bool) -> GameCamera {
        let mut c = if interpolate {
            lerp_camera(&self.prev_camera, &self.cur_camera, self.clock.alpha())
        } else {
            self.cur_camera.clone()
        };
        c.viewport = viewport;
        c
    }
}

fn lerp_camera(a: &GameCamera, b: &GameCamera, t: f32) -> GameCamera {
    let mut c = b.clone();
    let t = t.clamp(0.0, 1.0);
    c.target = a.target.lerp(b.target, t);
    c.projection_angle = a.projection_angle + (b.projection_angle - a.projection_angle) * t;
    c.altitude = a.altitude + (b.altitude - a.altitude) * t;
    c.mover_scale_factor = a.mover_scale_factor + (b.mover_scale_factor - a.mover_scale_factor) * t;
    let mut d = b.rotation - a.rotation;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    c.set_rotation(a.rotation + d * t);
    c
}

/// What a driver factory gets when a mission has loaded.
pub struct DriverContext<'a> {
    pub game: &'a mut GameData,
    pub view: &'a MissionView,
    pub config: &'a GameConfig,
    pub prefs: &'a CameraPrefs,
}

pub type DriverFactory<D> = Box<dyn FnMut(DriverContext<'_>) -> Result<D, String>>;

/// The production UI selection.  It draws the Results screens; headless
/// capability reporting follows this alias without creating a window or GPU
/// device.
pub type ProductionUi = UiHost;

/// Whether the production UI selection can draw a Results screen.
pub fn production_ui_has_results_screen() -> bool {
    ProductionUi::default().has_results_screen()
}

/// The one production order-route selection, shared by `GameCore` and the headless capability
/// check so the two cannot drift apart.
///
/// The bridge produces `mc2_sim::PlayerCommand`s, so it is installed for a driver whose command
/// is that type; any other command type has no route to the simulation and gets [`NoOrders`].
pub fn production_orders<C: 'static>() -> Box<dyn OrderHook<C>> {
    let bridge: Box<dyn OrderHook<PlayerCommand>> = Box::new(BridgeOrders);
    match (Box::new(bridge) as Box<dyn std::any::Any>).downcast::<Box<dyn OrderHook<C>>>() {
        Ok(hook) => *hook,
        Err(_) => Box::new(NoOrders::default()),
    }
}

/// Whether the production order route converts physical player input into
/// commands passed to the simulation (the live simulation's command type).
pub fn production_orders_reach_sim_commands() -> bool {
    production_orders::<PlayerCommand>().emits_sim_commands()
}

/// The application.
pub struct GameCore<D: SimDriver> {
    pub game: GameData,
    pub scene: SceneRenderer,
    pub states: StateMachine,
    pub config: GameConfig,
    pub mission: Option<MissionRuntime<D>>,
    pub ui: Box<dyn UiLayer>,
    /// The game sound (`mc2-audio`), when the front end has one (see [`crate::audio`]).
    pub audio: Option<AppAudio>,
    /// Whether mission-video audio attaches to the game mixer. Production keeps this enabled; the
    /// HUD-only headless audio route excludes video playback so it can measure UI cues alone.
    pub movie_audio: bool,
    pub cursor: Option<Box<dyn CursorLayer>>,
    pub orders: Box<dyn OrderHook<D::Command>>,
    pub sinks: Vec<Box<dyn SimEventSink<D>>>,
    /// gosFX effects drawn in the scene pass (fx lane).
    pub effects: Option<Box<dyn crate::layers::SceneEffects>>,
    /// The effects behind [`Self::effects`] and the effects sink, when [`crate::production::install_sinks`] ran.
    pub fx_hub: Option<crate::fx_sink::FxHub>,
    /// The battle sound sink is registered, so the sim's sound events are played (not refused).
    pub battle_sound: bool,
    /// `useNonWeaponEffects` (`code/prefs.cpp:250`): the weather updates and draws only while it is on
    /// (`code/mission.cpp:485`, `code/gamecam.cpp:184`). Set from `options.cfg` and by accepted options.
    pub non_weapon_effects: bool,
    /// `scenarioResult` of the last mission that ended (the state may have moved on from Results since).
    pub last_result: Option<u32>,
    pub camera_settings: CameraSettings,
    pub camera_data: CameraData,
    pub prefs: CameraPrefs,
    /// `Camera::cameraZoom/cameraTilt`: static in the original, so they outlive a mission.
    pub views: [(f32, f32); 4],
    /// Last error (a mission that failed to load).
    pub last_error: Option<String>,
    /// A game screen size the player accepted in the options screen; the windowed front end resizes its window to it.
    window_request: Option<(u32, u32)>,
    /// The pointer position the last `frame` started with (the cursor without Async Mouse).
    frame_pointer: (f32, f32),
    /// Borderless fullscreen at a game resolution the monitor lacks: the scene renders at the game screen size and
    /// the surface stretches it ([`Self::set_scaled_fullscreen`]).
    scaled_fullscreen: bool,
    settings_applied: bool,
    /// Input events due at a given mission simulation frame, delivered immediately before that
    /// frame's tick (exactly once, whatever the render rate).
    pub scheduled_events: std::collections::BTreeMap<u64, Vec<InputEvent>>,
    /// Frame resizes due at a mission simulation frame, applied (like events) immediately before
    /// that frame's tick.
    pub scheduled_resizes: std::collections::BTreeMap<u64, Vec<(u32, u32)>>,
    /// Simulation frames after whose tick `frame` stops, so a screenshot of that frame is rendered
    /// from exactly that state even when several ticks are due in one render frame.
    pub capture_frames: std::collections::BTreeSet<u64>,
    /// Frames of `capture_frames` reached by the last `frame` call (the render shows the state
    /// right after each one's tick; ticks still due wait for the next call).
    pub captured: Vec<u64>,
    /// The last frame size a scheduled resize asked for, until the front end has resized its target.
    pub resize_request: Option<(u32, u32)>,
    pending_ticks: u32,
    /// Input events since the last game frame, pointer in game pixels: the HUD's input.
    hud_events: Vec<InputEvent>,
    /// Real seconds of the last [`Self::frame`], the HUD's animation clock.
    last_dt: f32,
    driver_factory: DriverFactory<D>,
    input: InputState,
    physical: (u32, u32),
    overlay: OverlayPainter,
    scratch: Vec<ShapeInstance>,
    extra: Vec<ShapeInstance>,
    /// Marker shape templates by [`crate::waypoints::MarkerKind`]; the outer `None` is not yet loaded.
    waypoint_markers: [Option<Option<ShapeInstance>>; 3],
    fx_batches: Vec<mc2_render::fx::FxBatch>,
    /// Set only after a real Results UI has been drawn in the production
    /// composition path.  P3 acceptance must not confuse `GameState::Results`
    /// with a player-visible Results screen.
    results_screen_drawn: bool,
}

/// `MC2_P3_TRACE=1` prints one line per frame that emits player commands (the P3 acceptance
/// diagnosis); off by default.
pub(crate) fn p3_trace_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("MC2_P3_TRACE").is_some_and(|v| v != "0"))
}

fn describe_command(c: &mc2_sim::PlayerCommand) -> String {
    match c {
        mc2_sim::PlayerCommand::Order {
            movers,
            order,
            queue,
        } => format!(
            "Order {{ movers: {:?}, code: {:?}, target: {:?}, queue: {} }}",
            movers.iter().map(|m| m.0).collect::<Vec<_>>(),
            order.code,
            order.target,
            queue
        ),
        other => format!("{other:?}"),
    }
}

impl<D: SimDriver + 'static> GameCore<D> {
    /// `gpu` is headless or surface-backed; `physical` the frame size in native pixels,
    /// `logical` the window size in logical points (the game screen).
    pub fn new(
        gpu: Arc<Gpu>,
        game: GameData,
        config: GameConfig,
        physical: (u32, u32),
        logical: (f64, f64),
        driver_factory: DriverFactory<D>,
    ) -> mc2_render::Result<Self> {
        let mut game = game;
        let mut camera_settings = match game.fit("system.cfg") {
            Ok(mut cfg) => CameraSettings::from_system_cfg(&mut cfg),
            Err(_) => CameraSettings::default(),
        };
        let prefs = match game.fit("prefs.cfg") {
            Ok(mut f) => CameraPrefs::from_fit(&mut f),
            Err(_) => CameraPrefs::default(),
        };
        camera_settings.max_clip_distance = prefs.max_clip_distance;
        camera_settings.min_haze_distance = prefs.min_haze_distance;
        let camera_data = match game.fit("data/art/missionscrn.fit") {
            Ok(mut f) => CameraData::from_fit(&mut f),
            Err(_) => CameraData::default(),
        };
        // 2D layers draw into `RenderTarget::view`
        let ui_format = gpu.target_format;
        let overlay = OverlayPainter::new(&gpu, ui_format);
        let cursor: Option<Box<dyn CursorLayer>> = if config.cursor {
            Some(Box::new(crate::layers::StandInCursor::new(&gpu, ui_format)))
        } else {
            None
        };
        let mut scene = SceneRenderer::new(gpu)?;
        scene.set_visual(config.visual);
        let (lw, lh) = (logical.0.max(1.0), logical.1.max(1.0));
        let input = InputState::new(Viewport::new(
            lw.round() as u32,
            lh.round() as u32,
            lw,
            lh,
            Fit::Stretch,
        ));
        let ui = Box::new(ProductionUi::new());
        Ok(Self {
            game,
            scene,
            states: StateMachine::new(),
            config,
            mission: None,
            ui,
            audio: None,
            movie_audio: true,
            cursor,
            orders: production_orders(),
            sinks: Vec::new(),
            scheduled_events: std::collections::BTreeMap::new(),
            scheduled_resizes: std::collections::BTreeMap::new(),
            capture_frames: std::collections::BTreeSet::new(),
            captured: Vec::new(),
            resize_request: None,
            pending_ticks: 0,
            hud_events: Vec::new(),
            last_dt: 1.0 / 30.0,
            effects: None,
            fx_hub: None,
            battle_sound: false,
            non_weapon_effects: false,
            last_result: None,
            views: prefs.views,
            camera_settings,
            camera_data,
            prefs,
            last_error: None,
            window_request: None,
            frame_pointer: (0.0, 0.0),
            scaled_fullscreen: false,
            settings_applied: false,
            driver_factory,
            input,
            physical: (physical.0.max(1), physical.1.max(1)),
            overlay,
            scratch: Vec::new(),
            extra: Vec::new(),
            waypoint_markers: Default::default(),
            fx_batches: Vec::new(),
            results_screen_drawn: false,
        })
    }

    /// The production GUI host, while it is the installed GUI.
    pub fn ui_host_mut(&mut self) -> Option<&mut UiHost> {
        self.ui.as_any_mut()?.downcast_mut::<UiHost>()
    }

    pub fn gpu(&self) -> &Gpu {
        self.scene.gpu()
    }

    pub fn state(&self) -> &GameState {
        self.states.state()
    }

    /// The game-side input (what the GUI let through).
    pub fn input(&self) -> &InputState {
        &self.input
    }

    /// The logical game screen (pointer coordinates).
    pub fn game_size(&self) -> (f32, f32) {
        let (w, h) = self.input.viewport().game_size();
        (w as f32, h as f32)
    }

    /// The window size (logical pixels) the options screen asked for since the last call; the front end applies it.
    pub fn take_window_request(&mut self) -> Option<(u32, u32)> {
        self.window_request.take()
    }

    pub fn physical_size(&self) -> (u32, u32) {
        self.physical
    }

    /// Whether the installed production driver is a live simulation adapter.
    /// It only becomes meaningful after the mission has loaded.
    pub fn has_live_sim(&self) -> bool {
        self.mission
            .as_ref()
            .is_some_and(|mission| mission.driver.is_live_sim())
    }

    /// Whether the installed UI can draw the player-visible Results screen.
    pub fn has_results_screen(&self) -> bool {
        self.ui.has_results_screen()
    }

    /// Whether this run has actually composed a Results UI frame.
    pub fn results_screen_drawn(&self) -> bool {
        self.results_screen_drawn
    }

    /// One input event (pointer positions in window logical points): the GUI first (pointer
    /// positions converted to game coordinates), then the game.
    pub fn handle_event(&mut self, event: InputEvent) {
        let for_ui = match event {
            InputEvent::CursorMoved { x, y } => {
                let (gx, gy) = self.input.viewport().window_to_game(x, y);
                InputEvent::CursorMoved {
                    x: gx as f64,
                    y: gy as f64,
                }
            }
            ref e => e.clone(),
        };
        let consumed = self.ui.handle_event(self.states.state(), &for_ui);
        if !consumed
            && self.mission.is_some()
            && matches!(self.states.state(), GameState::InMission)
            && !matches!(for_ui, InputEvent::Resized { .. })
            && self.hud_events.len() < HUD_EVENT_LIMIT
        {
            self.hud_events.push(for_ui);
        }
        // Pointer moves, focus and size changes always reach the game state, and so do releases
        // (a key the game saw go down must be seen going up).
        let always = matches!(
            event,
            InputEvent::CursorMoved { .. }
                | InputEvent::CursorEntered
                | InputEvent::CursorLeft
                | InputEvent::FocusLost
                | InputEvent::Resized { .. }
                | InputEvent::Key { pressed: false, .. }
                | InputEvent::Button { pressed: false, .. }
        );
        if let InputEvent::Resized {
            logical_w,
            logical_h,
        } = event
        {
            self.input.handle(event);
            // The game screen is the window in logical points, except in a scaled fullscreen: there the chosen
            // Resolution stays the game screen whatever the borderless surface measures.
            if !self.scaled_fullscreen {
                self.input.set_game_size(
                    logical_w.round().max(1.0) as u32,
                    logical_h.round().max(1.0) as u32,
                );
            }
            return;
        }
        if !consumed || always {
            self.input.handle(event);
        }
    }

    /// The size the 3D scene renders at for a target of `target` pixels: the target itself, or with a scaled
    /// fullscreen the chosen game resolution (the target then shows it stretched).
    pub fn render_size(&self, target: (u32, u32)) -> (u32, u32) {
        if self.scaled_fullscreen {
            let (w, h) = self.game_size();
            (w.round().max(1.0) as u32, h.round().max(1.0) as u32)
        } else {
            target
        }
    }

    /// Fullscreen at a Resolution the monitor has no display mode for: the borderless surface keeps the monitor's
    /// size and the game renders at the chosen resolution (its effective rendering dimensions) scaled to it.
    pub fn set_scaled_fullscreen(&mut self, on: bool) {
        self.scaled_fullscreen = on;
        let (w, h) = self.render_size(self.physical);
        if let Some(rt) = &mut self.mission {
            rt.rig.viewport = (w as f32, h as f32);
        }
    }

    /// The frame size in native pixels changed (window resize, scale change).
    pub fn resize(&mut self, physical: (u32, u32)) {
        self.physical = (physical.0.max(1), physical.1.max(1));
        let (w, h) = self.render_size(self.physical);
        if let Some(rt) = &mut self.mission {
            rt.rig.viewport = (w as f32, h as f32);
        }
    }

    /// Whether the saved or accepted options have reached the game yet (until then `prefs` holds defaults).
    pub fn settings_applied(&self) -> bool {
        self.settings_applied
    }

    /// Whether the sun's shape shadows are drawn: Shadows on and not at night. Local Shadows (point/spot light
    /// shadows, `tgl.cpp:3049`) additionally needs Shadows on (`msl.cpp:1644`, `local_shadows_drawn`).
    pub fn sun_shadows(&self, is_night: bool) -> bool {
        shadows_drawn(self.config.shadows, is_night)
    }

    /// Where the software cursor is drawn: with Async Mouse the freshest pointer position, without it the
    /// position the frame's update sampled.
    pub fn cursor_position(&self) -> (f32, f32) {
        self.fresh_pointer().unwrap_or(self.frame_pointer)
    }

    /// Async Mouse: the pointer when it moved since the frame's update sampled it.
    fn fresh_pointer(&self) -> Option<(f32, f32)> {
        let now = self.input.mouse_position();
        (self.prefs.async_mouse && now != self.frame_pointer).then_some(now)
    }

    /// Advances the game by `real_dt` of wall-clock time.
    pub fn frame(&mut self, real_dt: Duration) {
        self.last_dt = real_dt.as_secs_f32();
        self.frame_pointer = self.input.mouse_position();
        // The sound first, as in the C++: `soundSystem->update()` (`code/mechcmd2.cpp:2108`) runs before
        // `logistics->update()` (`:2118`) and the menu logic, so a music request made below starts its stream
        // on the next update.
        if let Some(a) = &mut self.audio {
            a.update(real_dt.as_secs_f32());
        }
        // GUI: it may request a state change.
        let req = {
            let game_size = self.game_size();
            let (camera, paused) = match &self.mission {
                Some(rt) => (Some(&rt.rig), rt.paused),
                None => (None, false),
            };
            self.ui.set_audio_available(self.audio.is_some());
            self.ui
                .set_movie_mixer(self.audio.as_ref().map(AppAudio::mixer));
            let mut frame = UiFrame {
                state: self.states.state(),
                input: &self.input,
                dt: real_dt.as_secs_f32(),
                game_size,
                camera,
                paused,
                vfs: &mut self.game.vfs,
            };
            self.ui.update(&mut frame)
        };
        if let Some(a) = self.ui.take_applied_settings() {
            self.config.shadows = a.shadows;
            self.prefs.left_right_mouse_profile = a.left_right_mouse_profile;
            self.prefs.high_object_detail = a.high_object_detail;
            self.prefs.terrain_detail = a.terrain_detail;
            self.prefs.local_shadows = a.local_shadows;
            self.prefs.pilot_videos = a.pilot_videos;
            self.prefs.async_mouse = a.async_mouse;
            self.prefs.renderer = a.renderer;
            self.settings_applied = true;
            self.prefs.hot_keys = a.hot_keys;
            let startup = a
                .saved_resolution
                .filter(|_| self.config.use_saved_resolution);
            if let Some((w, h)) = a.resolution.or(startup) {
                // `Environment.screenWidth/Height`: the game screen takes the size; the windowed front end resizes
                // its window to it (`take_window_request`)
                self.input.set_game_size(w, h);
                self.set_scaled_fullscreen(self.scaled_fullscreen);
                self.window_request = Some((w, h));
            }
            if !self.prefs.pilot_videos {
                if let Some(hud) = self.mission.as_mut().and_then(|m| m.hud.as_mut()) {
                    hud.end_pilot_video();
                }
            }
            self.non_weapon_effects = a.use_non_weapon_effects;
            if let Some(hub) = &self.fx_hub {
                hub.set_use_non_weapon_effects(a.use_non_weapon_effects);
            }
        }
        if let Some(e) = self.ui.take_error() {
            log::error!("ui: {e}");
            self.last_error = Some(e);
        }
        if let Some(r) = req {
            self.request(r);
        }
        // The sounds and music the screens asked for; the sound updates them from the next frame on.
        let cues = self.ui.take_audio();
        if let Some(a) = &mut self.audio {
            a.apply_all(&cues);
        }

        match self.states.state().clone() {
            GameState::Boot => {
                let r = match &self.config.mission {
                    Some(m) => StateRequest::StartMission(m.clone()),
                    None => StateRequest::MainMenu,
                };
                self.request(r);
                self.input.end_frame();
            }
            GameState::MissionLoading(name) => {
                // If the UI host is installed and has a launch spec, retrieve the campaign paint
                // from it.  The UI host stores the spec in `launch`, and the first force mech's
                // colors are the campaign's home commander's paint.
                if let Some(host) = self.ui_host_mut() {
                    let paint = host.launch_campaign_paint();
                    let launch = host.launch_spec().cloned();
                    if launch.is_some() {
                        self.config.campaign_launch = launch;
                    }
                    if let Some(paint) = paint {
                        self.config.campaign_paint = Some(paint);
                    }
                }
                match self.load_mission(&name) {
                    Ok(()) => {
                        self.request(StateRequest::MissionReady);
                    }
                    Err(e) => {
                        log::error!("mission {name}: {e}");
                        let msg = format!("mission {name}: {e}");
                        self.ui
                            .report(&format!("The mission could not be loaded ({msg})"));
                        self.last_error = Some(msg);
                        self.request(StateRequest::MainMenu);
                    }
                }
                self.input.end_frame();
            }
            // an options screen over the mission stops it (`mechcmd2.cpp:2142`)
            GameState::InMission if self.ui.blocks_mission() => self.input.end_frame(),
            GameState::InMission => self.mission_frames(real_dt),
            _ => self.input.end_frame(),
        }
    }

    fn request(&mut self, r: StateRequest) {
        let was_in_mission = matches!(self.states.state(), GameState::InMission);
        let ended = match r {
            StateRequest::EndMission(result) => Some(result),
            _ => None,
        };
        let preserve_mission = ended.is_some();
        if self.states.request(r) {
            if was_in_mission {
                for sink in &mut self.sinks {
                    sink.mission_end();
                }
                if !preserve_mission {
                    self.mission = None;
                }
            }
            if ended.is_some() {
                self.last_result = ended;
            }
            if let (Some(result), Some(rt)) = (ended, self.mission.as_mut()) {
                // `Logistics::start(log_RESULTS)`: the results flow reads what the mission reports
                let outcome = rt.driver.mission_outcome();
                self.ui.mission_ended(result, outcome);
            }
            let cues = audio::state_change_cues(self.states.state());
            if let Some(a) = &mut self.audio {
                a.apply_all(&cues);
            }
        }
    }

    fn load_mission(&mut self, name: &str) -> Result<(), String> {
        self.mission = None;
        // `MissionView::load` clears the renderer's shapes, so cached marker model ids would dangle.
        self.waypoint_markers = Default::default();
        self.pending_ticks = 0;
        self.hud_events.clear();
        let view = MissionView::load(
            &mut self.game,
            &mut self.scene,
            name,
            self.config.campaign_paint,
            self.config
                .campaign_launch
                .is_some()
                .then(|| crate::live_sim::sim_load_spec(name, &self.config))
                .as_ref(),
        )
        .map_err(|e| e.to_string())?;
        let mut driver = (self.driver_factory)(DriverContext {
            game: &mut self.game,
            view: &view,
            config: &self.config,
            prefs: &self.prefs,
        })?;
        let hud = match driver.hud_snapshot() {
            Some(snapshot) => {
                let game_size = (
                    self.input.viewport().game_size().0 as f32,
                    self.input.viewport().game_size().1 as f32,
                );
                Some(
                    HudRuntime::load(&mut self.game.vfs, game_size, name, snapshot)
                        .map_err(|e| format!("mission HUD unavailable: {e}"))?,
                )
            }
            None => None,
        };
        let clock = SimClock::new(self.config.sim_hz);
        let mut rig = CameraRig::new(
            self.camera_settings.clone(),
            &view.camera_start,
            self.camera_data.clone(),
            self.views,
            {
                let (w, h) = self.render_size(self.physical);
                (w as f32, h as f32)
            },
        );
        let mut user_input = UserInput::new();
        let (_, gh) = self.game_size();
        user_input.set_drag_threshold(self.camera_data.mouse_drag_threshold / gh.max(1.0));
        user_input.set_double_click_threshold(self.camera_data.double_click_threshold);
        {
            let ground = view.ground();
            // mission start: eye->activate(); eye->update()
            rig.activate(view.first_player_mover, &ground, INITIAL_FRAME_LENGTH);
            rig.update(0, &ground, INITIAL_FRAME_LENGTH);
        }
        let cam = rig.view();
        let weather = view
            .mission
            .weather
            .clone()
            .map(|config| Weather::new(config, weather_seed(name)))
            .filter(Weather::is_active);
        self.mission = Some(MissionRuntime {
            name: name.to_string(),
            view,
            rig,
            clock,
            driver,
            user_input,
            turn: 0,
            paused: false,
            visibility: Rc::new(CameraVisibility::default()),
            result: MIS_PLAYING,
            sim_sound_refused: false,
            weather,
            weather_events: Vec::new(),
            hud,
            presentation: AppPresentation::with_movie(MovieControl::new(
                self.movie_audio
                    .then(|| self.audio.as_ref().map(AppAudio::mixer))
                    .flatten(),
            )),
            sim_presentation: Default::default(),
            last_aoe_style: None,
            pause_actions: Vec::new(),
            speaking_pilots: std::collections::BTreeSet::new(),
            prev_camera: cam.clone(),
            cur_camera: cam,
        });
        if let Some(mission) = &mut self.mission {
            mission.sim_presentation = mission
                .driver
                .sim_presentation(mission.driver.world())
                .unwrap_or_default();
            for sink in &mut self.sinks {
                sink.mission_start(mission.driver.world());
            }
        }
        // the beam textures the sinks found in the object types
        if let Some(hub) = &self.fx_hub {
            hub.upload_pending_textures(&mut self.game.vfs, &mut self.scene);
            hub.set_bomber(self.mission.as_ref().and_then(|m| m.view.bomber.clone()));
        }
        Ok(())
    }

    /// The HUD is laid out for one screen size ([`hud::hud_resolution`]); a resize that changes it
    /// loads the layout of the new size and keeps the selection and the pause state.
    fn reload_hud_on_resize(&mut self) {
        let size = self.game_size();
        let Some(rt) = self.mission.as_mut() else {
            return;
        };
        let Some(old) = rt.hud.as_ref() else {
            return;
        };
        if old.fits(size) {
            return;
        }
        let Some(snapshot) = rt.driver.hud_snapshot() else {
            return;
        };
        match HudRuntime::load(&mut self.game.vfs, size, &rt.name, snapshot) {
            Ok(mut fresh) => {
                if let Some(mut old) = rt.hud.take() {
                    fresh.carry_state_from(&mut old);
                }
                rt.hud = Some(fresh);
            }
            Err(e) => {
                log::error!("mission HUD unavailable after the resize: {e}");
                self.last_error = Some(format!("mission HUD unavailable: {e}"));
            }
        }
    }

    fn mission_frames(&mut self, real_dt: Duration) {
        self.captured.clear();
        self.reload_hud_on_resize();
        let Some(rt) = self.mission.as_mut() else {
            self.input.end_frame();
            return;
        };
        self.pending_ticks += rt.clock.advance(real_dt);
        let mut end = None;
        let mut quit = false;
        let mut failure = None;
        let mut hud_cues: Vec<Cue> = Vec::new();
        while self.pending_ticks > 0 {
            self.pending_ticks -= 1;
            // Scripted events and resizes for this simulation frame arrive just before it runs.
            let frame = self.mission.as_ref().map_or(0, |m| m.clock.frame());
            let due = self.scheduled_events.remove(&frame);
            for e in due.into_iter().flatten() {
                self.handle_event(e);
            }
            for (w, h) in self.scheduled_resizes.remove(&frame).into_iter().flatten() {
                let (w, h) = (w.max(1), h.max(1));
                self.handle_event(InputEvent::Resized {
                    logical_w: w as f64,
                    logical_h: h as f64,
                });
                self.resize((w, h));
                self.resize_request = Some((w, h));
            }
            let game_size = self.game_size();
            // What the scripts may read back this frame: the sound system (`mc2_sim::World::audio`) and the mission interface's
            // callout animation state (`mc2_sim::World::callout`). A mission's `animationcallout` runs in the simulation's own
            // presentation state (`natives_presentation::Callout`), whose pointer and button flash the application draws
            // (`AppPresentation::pointer_frame`), so this legacy readback stays at its default.
            let sound = self
                .audio
                .as_ref()
                .map(AppAudio::readback)
                .unwrap_or_default();
            let callout = mc2_sim::events::CalloutState::default();
            let Some(rt) = self.mission.as_mut() else {
                break;
            };
            let hud_events = std::mem::take(&mut self.hud_events);
            let r = mission_tick(
                rt,
                &mut self.game.vfs,
                &self.scene,
                &self.input,
                &hud_events,
                game_size,
                &self.prefs,
                self.camera_data.mouse_drag_threshold,
                self.orders.as_mut(),
                &mut self.sinks,
                self.battle_sound,
                self.non_weapon_effects,
                sound,
                callout,
                &mut hud_cues,
            );
            // The HUD's sounds of this simulation frame start after the frame's sim sound events, which the sinks
            // play inside `mission_tick` (the HUD runs before the simulation step, as `ControlGui::update` runs before
            // `Mission::update`'s sim step). Both start in the same mixed frame, so the order is not audible.
            if let Some(a) = &mut self.audio {
                a.apply_all(&hud_cues);
                // The shared production sink queues game audio but deliberately
                // leaves its frame advancement to the app.  Advance it before
                // consuming callbacks so radio start/stop reaches the HUD.
                a.set_clock(rt.turn.max(0) as u32, rt.clock.game_seconds());
                a.update(rt.clock.frame_length());
                // `GameSoundSystem` is the authority for `MechWarrior::messagePlaying`: it emits
                // these callbacks when a radio fragment actually begins or ends, not when the sim
                // merely requests chatter.  The HUD consumes the set on the next GUI update.
                for event in a.game_mut().take_radio_events() {
                    match event {
                        mc2_audio::game::RadioEvent::PilotSpeaking(pilot) => {
                            rt.speaking_pilots.insert(pilot.0);
                        }
                        mc2_audio::game::RadioEvent::PilotSilent(pilot) => {
                            rt.speaking_pilots.remove(&pilot.0);
                        }
                        mc2_audio::game::RadioEvent::PilotVideoStart { pilot, movie_code } => {
                            // `ControlGui::playPilotVideo`
                            let mission_video = matches!(
                                rt.presentation.movie.playback(),
                                crate::movie_control::Playback::Playing
                                    | crate::movie_control::Playback::Paused
                            );
                            if let Some(hud) = rt.hud.as_mut() {
                                hud.start_pilot_video(
                                    &mut self.game.vfs,
                                    pilot.0,
                                    movie_code,
                                    self.prefs.pilot_videos,
                                    mission_video,
                                );
                            }
                        }
                        mc2_audio::game::RadioEvent::PilotVideoEnd => {
                            if let Some(hud) = rt.hud.as_mut() {
                                hud.end_pilot_video();
                            }
                        }
                    }
                }
            }
            hud_cues.clear();
            // Edges and deltas are consumed by the first simulation frame that sees them.
            self.input.end_frame();
            let pause_actions = std::mem::take(&mut rt.pause_actions);
            if let Some(e) = rt.driver.take_failure() {
                failure = Some(e);
                break;
            }
            if r != MIS_PLAYING {
                end = Some(r);
                break;
            }
            match self.apply_pause_actions(pause_actions) {
                Some(PauseExit::Restart) => {
                    let name = self
                        .mission
                        .as_ref()
                        .expect("mission remains loaded")
                        .name
                        .clone();
                    self.request(StateRequest::RestartMission(name));
                    break;
                }
                Some(PauseExit::Quit) => {
                    quit = true;
                    break;
                }
                None => {}
            }
            if self.mission_stopped_by_ui() {
                break;
            }
            if self.capture_frames.remove(&frame) {
                self.captured.push(frame);
                break;
            }
        }
        if end.is_some() || failure.is_some() {
            self.pending_ticks = 0;
            // The rain loop belongs to the mission: it stops with it. The weather events of the ending
            // ticks go too, or a loop queued on the last tick would restart after the purge (mission.cpp:3372).
            if let Some(rt) = self.mission.as_mut() {
                rt.weather_events.clear();
            }
            if let Some(a) = &mut self.audio {
                a.game_mut().sound.sys.set_is_raining(0);
            }
        }
        let Some(rt) = self.mission.as_mut() else {
            return;
        };
        // The camera is the listener and the mission clock feeds the radio (`set_listener`, `set_clock`).
        if let Some(a) = &mut self.audio {
            a.set_camera(rt.rig.eye_position(), rt.rig.view().rotation);
            a.set_clock(rt.turn.max(0) as u32, rt.clock.game_seconds());
            for event in std::mem::take(&mut rt.weather_events) {
                play_weather_event(a, event);
            }
        }
        self.views = rt.rig.views;
        if let Some(e) = failure {
            log::error!("simulation unavailable: {e}");
            self.last_error = Some(format!("simulation unavailable: {e}"));
            self.request(StateRequest::MainMenu);
            return;
        }
        if quit {
            // `gos_TerminateApplication` (`pausewindow.cpp:80`)
            self.request(StateRequest::Exit);
            return;
        }
        if let Some(r) = end {
            self.request(StateRequest::EndMission(r));
        }
    }

    /// The UI took the mission over this frame (the options screen): no more ticks until it is done.
    fn mission_stopped_by_ui(&self) -> bool {
        self.ui.blocks_mission()
    }

    /// `PauseWindow::handleClick` (`pausewindow.cpp:365-425`) for the actions the HUD collected.
    fn apply_pause_actions(&mut self, actions: Vec<PauseIntent>) -> Option<PauseExit> {
        for action in actions {
            match action {
                PauseIntent::Save | PauseIntent::Load => {
                    // `!MPlayer && !LogisticsData::instance->isSingleMission()`, else `INVALID_GUI`
                    if self.ui.is_single_mission() {
                        self.play_ui_sound(INVALID_GUI);
                        continue;
                    }
                    let result = if action == PauseIntent::Save {
                        self.save_mission_file()
                    } else {
                        self.load_mission_file()
                    };
                    if let Err(e) = result {
                        log::error!("in-mission {action:?}: {e}");
                        self.ui.report(&format!(
                            "The mission could not be {} ({e})",
                            if action == PauseIntent::Save {
                                "saved"
                            } else {
                                "loaded"
                            }
                        ));
                    }
                }
                PauseIntent::Options => self.ui.open_mission_options(),
                PauseIntent::RestartMission => return Some(PauseExit::Restart),
                PauseIntent::QuitGame => return Some(PauseExit::Quit),
                PauseIntent::Resume | PauseIntent::ToggleObjectives => {}
            }
        }
        None
    }

    fn play_ui_sound(&mut self, id: i32) {
        if let Some(a) = &mut self.audio {
            a.apply_all(&[Cue::Sound(id)]);
        }
    }

    /// `Mission::save("<savePath>/testgame.ims")` (`mission.cpp:409-416`); the file is the new Rust format.
    fn save_mission_file(&mut self) -> Result<(), String> {
        let dir = self
            .ui
            .mission_save_dir()
            .ok_or("no save directory is available")?;
        let bytes = self
            .mission
            .as_ref()
            .and_then(|rt| rt.driver.save_mission())
            .ok_or("the simulation has no mission state to save")?;
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(MISSION_SAVE_FILE);
        let temp = dir.join(format!("{MISSION_SAVE_FILE}.tmp"));
        std::fs::write(&temp, bytes)
            .and_then(|()| std::fs::rename(&temp, &path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `mission->load("<savePath>/testgame.ims")` (`mechcmd2.cpp:2268-2275`).
    fn load_mission_file(&mut self) -> Result<(), String> {
        let dir = self
            .ui
            .mission_save_dir()
            .ok_or("no save directory is available")?;
        let path = dir.join(MISSION_SAVE_FILE);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let rt = self.mission.as_mut().ok_or("no mission is running")?;
        rt.driver.load_mission(&mut self.game, &bytes)
    }

    /// [`Self::render`] for a surface: with a scaled fullscreen the frame is rendered at the chosen resolution
    /// into `presenter`'s frame and stretched over `target`; otherwise it is drawn into `target` directly.
    pub fn render_presented(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        presenter: &mut mc2_render::present::ScaledPresenter,
    ) -> mc2_render::Result<()> {
        if !self.scaled_fullscreen {
            return self.render(encoder, target);
        }
        let gpu = self.scene.gpu_arc().clone();
        let (w, h) = self.render_size(target.size());
        let frame = presenter.frame(&gpu, w, h);
        self.render(encoder, &frame)?;
        presenter.present(&gpu, encoder, target);
        Ok(())
    }

    /// Records the current frame into `encoder`, drawing into `target`: scene → GUI → cursor.
    /// The caller submits the encoder (once per frame).
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
    ) -> mc2_render::Result<()> {
        let size = target.size();
        let game_size = self.game_size();
        let (vw, vh) = self.render_size(size);
        let gpu = self.scene.gpu_arc().clone();
        let fresh_pointer = self.fresh_pointer();
        let mut drew_scene = false;
        if let (GameState::InMission, Some(rt)) = (self.states.state(), self.mission.as_mut()) {
            // the camera's aspect is the render size's: a scaled fullscreen stretches it over the target
            let cam = rt.render_camera((vw as f32, vh as f32), self.config.interpolate_camera);
            self.extra.clear();
            rt.driver.append_instances(
                &mut self.scene,
                &rt.view,
                rt.clock.alpha(),
                &mut self.extra,
            );
            // `MissionInterfaceManager::render` (missiongui.cpp:2977-2990): the queued paths of the selected home
            // units while the waypoint key or the mine toggle is held.
            let waypoint_paths = match rt.hud.as_ref() {
                // Inside `drawGUIOn` (missiongui.cpp:2902): movie mode hides markers and lines.
                Some(hud)
                    if rt.presentation.gui_drawn(&rt.sim_presentation)
                        && (crate::userinput::ctrl(&self.input)
                            || hud.control_panel().lay_mines) =>
                {
                    rt.driver.waypoint_paths(&hud.selection.selected)
                }
                _ => Vec::new(),
            };
            for path in &waypoint_paths {
                for &(kind, point) in &path.queue {
                    if let Some(marker) = crate::waypoints::marker_instance(
                        &mut self.game,
                        &mut self.scene,
                        &mut self.waypoint_markers,
                        kind,
                        point,
                    ) {
                        self.extra.push(marker);
                    }
                }
            }
            let any_hidden =
                (0..rt.view.instances.len()).any(|i| rt.driver.hides_static_instance(i));
            let mut forced = Vec::new();
            rt.driver
                .append_forced_instances(&self.scene, &rt.view, &mut forced);
            let instances: &[ShapeInstance] =
                if self.extra.is_empty() && !any_hidden && forced.is_empty() {
                    // `*Appearance::recalcBounds`: choose damage/LOD only after the final camera
                    // (including this target's viewport) is known, and before the shape is drawn.
                    // `HighObjectDetail` defaults on (`mech3d.cpp:88`, `prefs.cpp:52`).
                    select_shapes(
                        &self.scene,
                        &mut rt.view.instances,
                        &cam,
                        self.prefs.high_object_detail,
                    );
                    &rt.view.instances
                } else {
                    self.scratch.clear();
                    self.scratch.extend(
                        rt.view
                            .instances
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| !rt.driver.hides_static_instance(*i))
                            .map(|(_, inst)| inst.clone()),
                    );
                    self.scratch.append(&mut self.extra);
                    let forced_from = self.scratch.len();
                    self.scratch.append(&mut forced);
                    select_shapes(
                        &self.scene,
                        &mut self.scratch,
                        &cam,
                        self.prefs.high_object_detail,
                    );
                    for inst in &mut self.scratch[forced_from..] {
                        inst.in_view = true;
                    }
                    &self.scratch
                };
            let m = &rt.view.mission;
            // The rain darkens the light and the terrain (weather.cpp:356-385); a clear sky changes neither,
            // and neither does a sky the non-weapon effects leave off (`gamecam.cpp:184`).
            let weather = rt.weather.as_ref().filter(|_| self.non_weapon_effects);
            let light = match weather {
                Some(w) => w.apply_light(&m.light),
                None => m.light.clone(),
            };
            let mut terrain_params = m.terrain_params.clone();
            terrain_params.detail_texture = self.prefs.terrain_detail;
            if let Some(w) = weather {
                terrain_params.light_scale = w.terrain_light_scale();
                terrain_params.lightning = w.terrain_lightning();
            }
            let frame = Frame {
                width: size.0,
                height: size.1,
                camera: &cam,
                light: &light,
                water: &m.water,
                terrain_params: &terrain_params,
                time: rt.clock.game_seconds(),
                precise_time: Some(rt.clock.game_seconds_exact()),
                instances,
                // `MultiTransformShadows`: the sun casts no shadows at night
                shadows: shadows_drawn(self.config.shadows, m.light.is_night),
            };
            self.fx_batches.clear();
            if let Some(fx) = self.effects.as_mut() {
                // the decals lie on the ground, so they go first (ambient `lightRGB` with the rain, crater.cpp:300)
                fx.decals(light.ambient, &mut self.fx_batches);
                fx.batches(&cam, frame.time, &mut self.fx_batches);
            }
            if let Some(rain) = weather.and_then(|w| w.rain_batch(&cam, &light)) {
                self.fx_batches.push(rain);
            }
            self.scene.set_local_shadows(local_shadows_drawn(
                self.config.shadows,
                self.prefs.local_shadows,
            ));
            self.scene
                .encode(&gpu, encoder, target, &frame, &self.fx_batches)?;
            drew_scene = true;
            if !waypoint_paths.is_empty() {
                let mut b = OverlayBatch::default();
                crate::waypoints::draw_lines(&mut b, &cam, &waypoint_paths);
                if !b.is_empty() {
                    if self.overlay.format() != target.display_format {
                        self.overlay = OverlayPainter::new(&gpu, target.display_format);
                    }
                    self.overlay
                        .draw(&gpu, encoder, &target.display_view, size, &b, None);
                }
            }
        }
        if !drew_scene {
            // Menus, loading and results without a GUI: a plain screen (a progress bar while
            // loading).
            let mut b = OverlayBatch::default();
            if let GameState::MissionLoading(_) = self.states.state() {
                let (w, h) = (size.0 as f32, size.1 as f32);
                b.rect(w * 0.3, h * 0.48, w * 0.4, h * 0.04, [0.25, 0.3, 0.25, 1.0]);
            }
            if self.overlay.format() != target.display_format {
                self.overlay = OverlayPainter::new(&gpu, target.display_format);
            }
            self.overlay.draw(
                &gpu,
                encoder,
                &target.display_view,
                size,
                &b,
                Some([0.0, 0.0, 0.0, 1.0]),
            );
        }
        if drew_scene {
            // The camera's movie overlay (`GameCamera::render`, `gamecam.cpp:219-302`): the letterbox bars and the fade colour
            // over the scene, under the GUI.
            if let Some(rt) = self.mission.as_ref() {
                let batch = rt.presentation.overlay(&rt.sim_presentation, size);
                if !batch.is_empty() {
                    if self.overlay.format() != target.display_format {
                        self.overlay = OverlayPainter::new(&gpu, target.display_format);
                    }
                    self.overlay
                        .draw(&gpu, encoder, &target.display_view, size, &batch, None);
                }
            }
            // The in-mission GUI, only while `MissionInterfaceManager` draws it: off in movie mode (the video window is
            // `ControlGui`'s own); a callout or a frozen GUI returns from update only, so the HUD still draws.
            // The mission video window (`ControlGui::render`'s `bMovie->render()`, `controlgui.cpp:367-372`) is `ControlGui`'s
            // own, outside that gate; the HUD draws it with the GUI.
            let (gui_drawn, movie_frame) = self.mission.as_ref().map_or((false, None), |m| {
                (
                    m.presentation.gui_drawn(&m.sim_presentation),
                    m.presentation.video_frame().cloned(),
                )
            });
            let hud = match self.mission.as_mut() {
                Some(m) if gui_drawn => m.hud.as_mut(),
                _ => None,
            };
            if let Some(hud) = hud {
                hud.draw(
                    &gpu,
                    encoder,
                    target,
                    self.last_dt,
                    movie_frame.as_ref(),
                    fresh_pointer,
                )
                .map_err(mc2_render::RenderError::Unavailable)?;
            }
        }
        let state = self.states.state();
        let mut ctx = DrawContext {
            gpu: &gpu,
            encoder,
            target,
            size,
            game_size,
            state,
            pointer: fresh_pointer,
        };
        let drew_results = self.ui.draw(&mut ctx);
        if matches!(ctx.state, GameState::Results(_)) && drew_results {
            self.results_screen_drawn = true;
        }
        let hud_draws_cursor = self
            .mission
            .as_ref()
            .filter(|m| m.presentation.gui_drawn(&m.sim_presentation))
            .and_then(|m| m.hud.as_ref())
            .is_some_and(|h| h.draws_cursor());
        let pointer = self.cursor_position();
        if !self.ui.draws_cursor() && !hud_draws_cursor && self.input.cursor_inside_window() {
            if let Some(c) = self.cursor.as_mut() {
                let request = CursorRequest {
                    camera: self
                        .mission
                        .as_ref()
                        .map(|m| m.rig.cursor)
                        .unwrap_or_default(),
                    order_state: self.orders.cursor(),
                };
                c.draw(&mut ctx, pointer, request);
            }
        }
        Ok(())
    }
}

/// Local Shadows (`tgl.cpp:3049`) work only under the global Shadows option: with it off `msl.cpp:1644`
/// never transforms shadows, so no point light casts one.
fn local_shadows_drawn(shadows: bool, local_shadows: bool) -> bool {
    shadows && local_shadows
}

/// The Shadows option; the sun casts no shadows at night (`MultiTransformShadows`).
fn shadows_drawn(shadows: bool, is_night: bool) -> bool {
    shadows && !is_night
}

/// The weather's sound: the rain loop (`setIsRaining`) or a crack of thunder (`playDigitalSample`).
fn play_weather_event(audio: &mut AppAudio, event: WeatherEvent) {
    let mut game = audio.game_mut();
    match event {
        WeatherEvent::RainLoop(id) => game.sound.sys.set_is_raining(id),
        WeatherEvent::Thunder(id) => {
            game.sound.sys.play_digital_sample(id, None, false);
        }
    }
}

/// The weather's random stream seed: FNV-1a of the mission name, so each mission has its own fixed sky.
fn weather_seed(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

/// One game frame of a mission (the parts of `Mission::update` the app owns, in order):
/// `turn++`, `missionInterface->update()` (camera input, hot keys, orders), `eye->update()`,
/// `land->geometry()` (visibility), then the simulation frame.  Returns `scenarioResult`.
#[allow(clippy::too_many_arguments)]
fn mission_tick<D: SimDriver>(
    rt: &mut MissionRuntime<D>,
    vfs: &mut mc2_formats::vfs::Vfs,
    scene: &SceneRenderer,
    input: &InputState,
    hud_events: &[InputEvent],
    game_size: (f32, f32),
    prefs: &CameraPrefs,
    drag_threshold_px: f32,
    orders: &mut dyn OrderHook<D::Command>,
    sinks: &mut [Box<dyn SimEventSink<D>>],
    battle_sound: bool,
    non_weapon_effects: bool,
    sound: mc2_sim::events::AudioReadback,
    callout: mc2_sim::events::CalloutState,
    hud_cues: &mut Vec<Cue>,
) -> u32 {
    rt.turn += 1;
    let (frame, frame_rate, now_ms) = rt.clock.next_frame();
    let fl = 1.0 / frame_rate;
    rt.user_input.update(input, game_size, fl);
    // `ControlGui::update`'s movie block (`controlgui.cpp:944-954`) runs before the GUI half of `MissionInterfaceManager::update`
    // (which the HUD below covers), so a frame's `PlayVideo` (issued by the previous frame's script) was opened last frame and
    // advances here.
    rt.presentation.advance_movie(vfs, fl);
    if let Some(hud) = rt.hud.as_mut() {
        hud.advance_pilot_video(vfs, fl);
    }
    // `MissionInterfaceManager::update`: in movie mode the GUI is off and only ESC is read (`missiongui.cpp:435-450`); then the
    // callout block and the GUI freeze (`:455-530`, `:586-592`), which return before the GUI takes any command.
    rt.presentation.gui_frame(&rt.sim_presentation);
    let callout_pressed = rt.presentation.callout_frame(&rt.sim_presentation);
    if let (Some((id, flashes)), Some(hud)) = (
        rt.presentation.take_objective_flash(&rt.sim_presentation),
        rt.hud.as_mut(),
    ) {
        hud.animate_tac_map(id, flashes);
    }
    // A callout that ends with `targetIsPressed` pushes its button before the freeze check (`missiongui.cpp:513`), so the press
    // reaches the HUD even while the script keeps the GUI frozen; the player's own events do not.
    let hud_gated = !rt.presentation.gui_active(&rt.sim_presentation);
    let mut routed_hud_events = if hud_gated {
        Vec::new()
    } else {
        hud_events.to_vec()
    };
    let synthetic_press = callout_pressed.is_some() && !rt.sim_presentation.movie_mode;
    if let Some(button) = callout_pressed {
        if let Some(hud) = rt.hud.as_ref() {
            if let Some(events) = hud.callout_button_events(button) {
                routed_hud_events.extend(events);
            }
        }
    }
    let ground = MissionGround {
        terrain: &rt.view.mission.terrain,
        water: rt.view.mission.water.elevation,
    };

    // `MissionInterfaceManager::update`: in movie mode, ESC asks the movie to end and nothing else is read
    // (`missiongui.cpp:443-446`), so the camera keys, the pointer and the orders wait for the movie to end.
    let movie_gates_input = !rt.presentation.gui_active(&rt.sim_presentation);
    let mut presentation = crate::sim::PresentationCommands::default();
    if rt.sim_presentation.movie_mode && input.key_status(keys::KEY_ESCAPE) == KeyStatus::Pressed {
        presentation.force_movie_end = true;
    }
    // `useLeftRightMouseProfile` (the `GUIIsAOEStyle` native): the player's profile is the simulation's every frame.
    let aoe = prefs.left_right_mouse_profile;
    if rt.last_aoe_style != Some(aoe) {
        presentation.aoe_style = Some(aoe);
        rt.last_aoe_style = Some(aoe);
    }

    // moveCameraAround: the profile's camera button
    let camera_clicked = if prefs.left_right_mouse_profile {
        input.key_status(keys::KEY_LMENU) == KeyStatus::Held
    } else {
        rt.user_input.is_right_drag()
    };
    let camera_handled = !movie_gates_input
        && rt
            .rig
            .mouse_camera(&rt.user_input, camera_clicked, game_size, &ground, fl);

    // hot keys
    let mut fired = Vec::new();
    if !movie_gates_input {
        let table = hotkeys::table_for(&prefs.hot_keys);
        hotkeys::dispatch_with(input, keys::KEY_LCONTROL, &table, &mut fired);
    }
    let mut order_keys = Vec::new();
    let mut pause_change = None;
    for f in fired {
        match f {
            Fired::Run(c) if c.is_camera() => apply_camera_command(&mut rt.rig, c, &ground, fl),
            // with a HUD the pause toggle is the HUD's (`MissionHud::paused`), applied below
            Fired::Run(Command::TogglePause) if rt.hud.is_none() => {
                rt.paused = !rt.paused;
                pause_change = Some(rt.paused);
            }
            other => order_keys.push(other),
        }
    }

    // the ground point under the pointer (the screen centre while the GUI is frozen, `missiongui.cpp:589-590`)
    let mouse_world = {
        let (mx, my) = rt.presentation.gui_pointer(
            rt.user_input.real_mouse(),
            game_size,
            &rt.sim_presentation,
        );
        let s = rt.rig.viewport.1 / game_size.1.max(1.0);
        rt.rig
            .view()
            .pixel_to_ground(mx * s, my * s, rt.rig.shift.z)
            .map(|p| mc2_render::math::Vec3::new(p.x, p.y, ground.elevation(p.x, p.y)))
    };

    // the HUD (`ControlGui::update` and the GUI half of `MissionInterfaceManager::update`)
    let mut over_hud = false;
    let mut hud_commands = Vec::new();
    let scenario_time = rt.driver.scenario_time();
    if let Some(hud) = rt.hud.as_mut() {
        hud.set_callout_flash(rt.presentation.callout_flash());
        // the tutorial pointer: `setMousePos` to the callout's target button or rect, the screen centre while frozen
        let c = &rt.presentation.callout;
        let target = hud.callout_target_point(c.target, c.target_is_button);
        let pointer = rt.presentation.pointer_frame(
            &rt.sim_presentation,
            rt.user_input.real_mouse(),
            target,
            game_size,
            fl,
        );
        hud.set_callout_pointer(pointer);
        // `setMousePos`: the game's pointer stays on the target after the callout, so the next click lands there
        if let (true, Some(p)) = (rt.presentation.callout.running, pointer) {
            rt.user_input.warp_mouse(p, input.mouse_position());
            hud.warp_mouse(p);
        }
    }
    if rt.hud.is_some() && (!movie_gates_input || synthetic_press) {
        let hud_messages = rt.driver.hud_messages();
        let snapshot = rt.driver.hud_snapshot();
        let world_facts = mouse_world.and_then(|p| {
            rt.driver.cursor_world_facts(mc2_sim::math::Vec3 {
                x: p.x,
                y: p.y,
                z: p.z,
            })
        });
        if let (Some(snapshot), Some(hud)) = (snapshot, rt.hud.as_mut()) {
            hud.messages(&hud_messages);
            let camera = rt.rig.view();
            let shift = crate::userinput::shift(input);
            let line_of_sight = world_facts.map(|f| f.line_of_sight).unwrap_or(false);
            let passable = world_facts
                .map(|f| f.passable)
                .unwrap_or(mouse_world.is_some());
            let out = hud.update(HudFrameInput {
                events: &routed_hud_events,
                user_input: &rt.user_input,
                fired: &order_keys,
                camera: &camera,
                game_size,
                dt: fl,
                drag_threshold_px,
                left_right_profile: prefs.left_right_mouse_profile,
                shift,
                waypoint_queue: crate::userinput::ctrl(input),
                turn: rt.turn.max(0) as u32,
                camera_rotate: camera_handled && camera_clicked,
                scenario_time,
                mouse_world,
                line_of_sight,
                passable,
                elevation: &|x, y| ground.elevation(x, y),
                snapshot,
                speaking_pilots: &rt.speaking_pilots,
            });
            over_hud = out.over_hud;
            rt.pause_actions.extend(out.pause_actions);
            hud_commands = out.commands;
            hud_cues.extend(out.cues);
            if let Some((x, y)) = out.camera_target {
                rt.rig.follow = None;
                rt.rig
                    .set_position(mc2_render::math::Vec3::new(x, y, 0.0), false, &ground, fl);
            }
            if let Some(part) = out.center_on {
                if let Some(u) = hud.snapshot().units.iter().find(|u| u.part_id == part) {
                    rt.rig.follow = None;
                    rt.rig.set_position(
                        mc2_render::math::Vec3::new(u.position.0, u.position.1, 0.0),
                        false,
                        &ground,
                        fl,
                    );
                }
            }
            if out.paused != rt.paused {
                rt.paused = out.paused;
                pause_change = Some(out.paused);
            }
        }
        // `ControlGui::resultsDone()`, asked by `Mission::update` once the termination timer ran out
        // (`mission.cpp:569`): the sim takes the answer in this frame's step.
        if rt.hud.as_ref().is_some_and(HudRuntime::results_done) {
            rt.driver.results_screen_done();
        }
    }

    // orders (P3)
    let mut commands = Vec::new();
    if let Some(p) = pause_change.and_then(|p| rt.driver.pause_command(p)) {
        commands.push(p);
    }
    if movie_gates_input {
        // the GUI's early returns took the pointer: no order, and the hot keys it did not see are dropped
        order_keys.clear();
    }
    {
        let ctx = OrderContext {
            frame,
            user_input: &rt.user_input,
            input,
            camera: &rt.rig,
            game_size,
            camera_handled: camera_handled || over_hud,
            mouse_world,
            paused: rt.paused,
        };
        for k in order_keys {
            orders.hotkey(k, &ctx);
        }
        orders.player_commands(&hud_commands, &mut commands);
        orders.update(&ctx, &mut commands);
    }
    if !hud_commands.is_empty() && p3_trace_enabled() {
        let selected = rt.hud.as_ref().map(|h| h.selection.selected.clone());
        eprintln!(
            "p3-trace frame {} selected {:?} mouse_world {:?} commands [{}]",
            rt.turn,
            selected.unwrap_or_default(),
            mouse_world.map(|p| (p.x, p.y)),
            hud_commands
                .iter()
                .map(describe_command)
                .collect::<Vec<_>>()
                .join("; ")
        );
    }

    // eye->update(): `GameCamera::update` first resolves the script's look target
    if rt.rig.look_target != -1 {
        rt.rig.follow = rt.driver.camera_target(rt.rig.look_target);
    }
    rt.rig.update(rt.turn, &ground, fl);
    // land->geometry(): the camera visibility input
    let mut snapshot = ViewSnapshot::from_rig(&rt.rig);
    if rt.turn == 1 {
        snapshot.clip_resolution = Some((0.0, 0.0));
    }
    let terrain_visibility = visibility::compute(
        &snapshot,
        &rt.view.mission.terrain,
        prefs.visible_vertices,
        &[],
        &[],
    );
    // This is the one app-to-render handoff for a live frame. The driver runs R1's exact
    // `select_shapes` on its bound appearances in the C++ object-update order before the sim
    // consumes `inView`; viewer drivers keep the explicit position-only/unavailable fallback.
    let vis = if let Some(objects) =
        rt.driver
            .object_visibility(scene, &snapshot.camera, &terrain_visibility)
    {
        Rc::new(visibility::with_objects(terrain_visibility, objects))
    } else {
        let movers = rt.driver.mover_positions();
        let structures = rt.driver.structure_positions();
        Rc::new(visibility::compute(
            &snapshot,
            &rt.view.mission.terrain,
            prefs.visible_vertices,
            &movers,
            &structures,
        ))
    };
    rt.visibility = vis.clone();
    rt.driver.set_use_non_weapon_effects(non_weapon_effects);
    let out = rt.driver.step(FrameInput {
        frame_rate,
        now_ms,
        visibility: vis,
        commands,
        presentation,
        camera: CameraReadback::from_rig(&rt.rig),
        audio: sound,
        callout,
    });
    // The mission script runs inside the sim step, after `eye->update()`: its camera calls take
    // effect on the camera now and are seen by the next frame's update.
    for req in rt.driver.camera_requests() {
        rt.rig.apply_script(&req, &ground, fl);
    }
    // The presentation state the script left (its movie mode, fade, callout, freeze), then its `PlayVideo` and `TutorialText`
    // requests: the video window opens on the script's request, the tutorial line joins the chat log
    // (`ControlGui::playMovie`, `missioninterface->setTutorialText`).
    rt.sim_presentation = rt
        .driver
        .sim_presentation(rt.driver.world())
        .unwrap_or_default();
    // `MissionInterfaceManager::setTutorialText`: the strings go into the chat log (`ControlGui::setChatText`). With the GUI
    // gate closed this frame (movie mode, callout, freeze) the GUI half of the update did not run, so the lines are kept for
    // the first frame the GUI takes input again.
    let requests = rt.driver.presentation_requests(&out.events);
    rt.presentation.events(vfs, &requests);
    if let Some(hud) = rt.hud.as_mut() {
        hud.tutorial_says(&std::mem::take(&mut rt.presentation.tutorial_lines));
    }
    // the render after the frame recomputes the matrices from the camera as the script left it
    rt.rig.refresh_origin(rt.turn, &ground);
    // `weather->update()` once per game frame, after the camera (weather.cpp:192); presentation only, and only
    // with the non-weapon effects on (`mission.cpp:485`).
    if let Some(weather) = rt.weather.as_mut().filter(|_| non_weapon_effects) {
        let eye = rt.rig.eye_position();
        let terrain = &rt.view.mission.terrain;
        weather.update(fl, eye, &|x, y| terrain.elevation_at(x, y));
        rt.weather_events.extend(weather.take_events());
    }
    let context = crate::sim::FrameContext::<D> {
        frame,
        frame_length: fl,
        turn: rt.driver.turn(),
        scenario_time: rt.driver.scenario_time(),
        listener: rt.rig.eye_position(),
        listener_rotation: rt.rig.rotation,
        world: rt.driver.world(),
    };
    for s in sinks.iter_mut() {
        s.on_frame(&context, &out.events);
    }
    // Without the battle sound sink the sim's events are not played: refused, and said once.
    if !battle_sound {
        if let Err(e) = audio::sim_events(&out.events) {
            if !rt.sim_sound_refused {
                log::warn!("mission {}: {e}; its sound events are not played", rt.name);
            }
            rt.sim_sound_refused = true;
        }
    }
    rt.result = out.result;
    rt.prev_camera = std::mem::replace(&mut rt.cur_camera, rt.rig.view());
    out.result
}

fn apply_camera_command(rig: &mut CameraRig, c: Command, ground: &dyn Ground, fl: f32) {
    match c {
        Command::CameraNormal => rig.all_normal(),
        Command::CameraView(n) => rig.set_camera_view(n as usize),
        Command::CameraAssign(n) => rig.assign_view(n as usize),
        Command::ScrollUp => rig.scroll_up(ground, fl),
        Command::ScrollDown => rig.scroll_down(ground, fl),
        Command::ScrollLeft => rig.scroll_left(ground, fl),
        Command::ScrollRight => rig.scroll_right(ground, fl),
        Command::ZoomOut => rig.zoom_out_cmd(fl),
        Command::ZoomIn => rig.zoom_in_cmd(fl),
        Command::RotateLeft => rig.rotate_left_cmd(fl),
        Command::RotateRight => rig.rotate_right_cmd(fl),
        Command::TiltUp => rig.tilt_up_cmd(fl),
        Command::TiltDown => rig.tilt_down_cmd(fl),
        _ => {}
    }
}

#[cfg(test)]
mod local_shadow_gate_tests {
    use super::local_shadows_drawn;

    /// `msl.cpp:1644` gates every shadow transform on Shadows, `tgl.cpp:3049` the point lights on Local Shadows.
    #[test]
    fn local_shadows_need_the_global_shadows_option() {
        assert!(local_shadows_drawn(true, true));
        assert!(
            !local_shadows_drawn(false, true),
            "Shadows FALSE, night light"
        );
        assert!(!local_shadows_drawn(true, false));
        assert!(!local_shadows_drawn(false, false));
    }
}
