// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The hooks other lanes plug into: the GUI (`mc2-ui`: screens, HUD, cursor), the order logic
//! (P3: selection and orders from `code/missiongui.cpp`), and the frame composition order.
//!
//! Every frame is composed in this order into one view: the scene (`mc2-render`, with `mc2-fx`
//! particles added through the sim driver's instances / the fx lane's sink) → the GUI layer
//! ([`UiLayer::draw`]) → the cursor ([`CursorLayer::draw`]).  Input goes the other way: the GUI is
//! offered each event first ([`UiLayer::handle_event`]); what it does not take reaches the game
//! (camera, hot keys, orders).

use crate::camera::{CameraCursor, CameraRig};
use crate::hotkeys::Fired;
use crate::overlay::{cursor_shape, OverlayBatch, OverlayPainter};
use crate::state::{GameState, StateRequest};
use crate::userinput::UserInput;
use mc2_platform::input::{InputEvent, InputState};
use mc2_render::gpu::{Gpu, RenderTarget};
use mc2_render::math::Vec3;

/// Everything a layer needs to draw into the frame.
pub struct DrawContext<'a> {
    pub gpu: &'a Gpu,
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// The frame's target (surface texture or offscreen), already holding the scene.  2D layers
    /// draw single-sample into `target.display_view` (no depth).
    pub target: &'a RenderTarget,
    /// Size of the target in native pixels.
    pub size: (u32, u32),
    /// The logical game screen (pointer coordinates).
    pub game_size: (f32, f32),
    pub state: &'a GameState,
    /// Async Mouse: the pointer (game pixels) when it moved after this frame's update sampled it; the cursor is
    /// drawn there instead of at the update-sampled position.
    pub pointer: Option<(f32, f32)>,
}

impl DrawContext<'_> {
    /// Native pixels per game pixel.
    pub fn scale(&self) -> f32 {
        self.size.1 as f32 / self.game_size.1.max(1.0)
    }
}

/// What the GUI sees each rendered frame.
pub struct UiFrame<'a> {
    pub state: &'a GameState,
    /// The game-side input state (events the GUI did not consume).
    pub input: &'a InputState,
    /// Real seconds since the previous frame.
    pub dt: f32,
    pub game_size: (f32, f32),
    /// The mission camera, while in a mission (tac map, compass, HUD placement).
    pub camera: Option<&'a CameraRig>,
    /// The mission is paused (pause menu).
    pub paused: bool,
    /// The mounted game data: the screens load their layouts and art from it.
    pub vfs: &'a mut mc2_formats::vfs::Vfs,
}

/// Options the player accepted that the core applies to the running game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedSettings {
    /// Sun shadows from the shapes.
    pub shadows: bool,
    /// The left/right mouse profile (the camera button is Alt, not the right button).
    pub left_right_mouse_profile: bool,
    /// `UseNonWeaponEffects`: the muzzle flash and the miss effects (`weaponbolt.cpp:2382`, `2422`).
    pub use_non_weapon_effects: bool,
    /// `useWaterInterestTexture` (Terrain Detail): the detail bitmap on the ground and the sky reflection on the water.
    pub terrain_detail: bool,
    /// `useHighObjectDetail` (Object Detail): off draws the lower shape detail.
    pub high_object_detail: bool,
    /// `useLocalShadows` (Local Shadows).
    pub local_shadows: bool,
    /// `pilotVideos`: the talking-head videos in the unit bar.
    pub pilot_videos: bool,
    /// `asyncMouse` (`mc2UseAsyncMouse`).
    pub async_mouse: bool,
    /// `renderer` (Hardware Rasterizer: 0 hardware, 1 software, 2 Direct3D reference in the original).
    pub renderer: i32,
    /// The hot key of every `commands[]` entry (`Key<n>`); the mission reads its key table from these.
    pub hot_keys: Vec<u32>,
    /// A resolution the player just accepted: the window and the game screen take this size (`None`: unchanged).
    pub resolution: Option<(u32, u32)>,
    /// The resolution saved in `options.cfg`, offered once at start-up; the core takes it only when
    /// `GameConfig::use_saved_resolution` is set (no `--size`).
    pub saved_resolution: Option<(u32, u32)>,
}

/// The GUI: menus, logistics screens, in-mission HUD (`mc2-ui`).
pub trait UiLayer {
    /// Offered every input event before the game. Return `true` to consume it.
    fn handle_event(&mut self, _state: &GameState, _event: &InputEvent) -> bool {
        false
    }
    /// Once per rendered frame before drawing; may ask for a state change (menu buttons,
    /// "start mission", "back to logistics").
    fn update(&mut self, _frame: &mut UiFrame<'_>) -> Option<StateRequest> {
        None
    }
    /// Draws the current screen or HUD over the scene, returning `true` only
    /// when this invocation actually composed a Results screen. Capability
    /// (`has_results_screen`) is deliberately not enough for P3: a mission
    /// can end after the UI update for this frame, leaving a host with no
    /// Results composition yet.
    fn draw(&mut self, _ctx: &mut DrawContext<'_>) -> bool {
        false
    }
    /// `true` when the GUI draws the cursor itself (the cursor layer is skipped).
    fn draws_cursor(&self) -> bool {
        false
    }
    /// `true` only when this layer has a real Results screen that it can draw.
    ///
    /// This is deliberately separate from [`GameState::Results`]: the state is
    /// a transition, while P3 acceptance must observe the player-visible
    /// screen.  A Results host overrides this before it is eligible for the
    /// production replay.
    fn has_results_screen(&self) -> bool {
        false
    }
    /// The mission ended with `result` (`scenarioResult`) and the core is in [`GameState::Results`]:
    /// `outcome` is what the mission reports to the campaign (`None` for a driver that has none).
    /// Called once, before the first update in the Results state (`Logistics::start(log_RESULTS)`).
    fn mission_ended(&mut self, _result: u32, _outcome: Option<mc2_campaign::interface::Outcome>) {}
    /// The core failed at something the player asked for (a mission that would not load): the GUI
    /// shows `message` to the player.
    fn report(&mut self, _message: &str) {}
    /// Options accepted since the last call; the core applies them. Returned once.
    fn take_applied_settings(&mut self) -> Option<AppliedSettings> {
        None
    }
    /// An error the GUI hit (a screen that failed to load); the core reports it as
    /// [`crate::game::GameCore::last_error`].  Returned once.
    fn take_error(&mut self) -> Option<String> {
        None
    }
    /// The campaign is a single mission (`LogisticsData::isSingleMission`): the pause menu's save and load
    /// refuse (`pausewindow.cpp:383-393`).
    fn is_single_mission(&self) -> bool {
        false
    }
    /// Where the in-mission save is kept (the campaign save directory), if the GUI has one.
    fn mission_save_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// The pause menu's OPTIONS: the options screen opens over the mission (`bInvokeOptionsScreenFlag`).
    fn open_mission_options(&mut self) {}
    /// A GUI screen is over the mission, so the mission does not run (`mechcmd2.cpp:2142`).
    fn blocks_mission(&self) -> bool {
        false
    }
    /// The GUI as its concrete type, for the front ends and the tests.
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        None
    }
    /// Whether the core has a game sound to send cues to; told before every [`UiLayer::update`].
    fn set_audio_available(&mut self, _available: bool) {}
    /// The app mixer used by screen-owned movie playback.
    fn set_movie_mixer(&mut self, _mixer: Option<mc2_platform::audio::SharedMixer>) {}
    /// The sounds and music the GUI asked for since the last call, in order (see [`crate::audio`]).
    fn take_audio(&mut self) -> Vec<crate::audio::Cue> {
        Vec::new()
    }
}

/// A GUI that draws nothing: no drawing, and minimal screen behaviour so the app
/// can run end to end.  Main menu and logistics start `mission` at once; the results screen goes
/// back to the main menu on any key or click.
#[derive(Clone, Debug)]
pub struct NullUi {
    /// The mission the main menu / logistics stubs start.
    pub mission: String,
}

impl Default for NullUi {
    fn default() -> Self {
        Self {
            mission: "mc2_01".into(),
        }
    }
}

impl UiLayer for NullUi {
    fn update(&mut self, frame: &mut UiFrame<'_>) -> Option<StateRequest> {
        match frame.state {
            GameState::MainMenu | GameState::Logistics => {
                Some(StateRequest::StartMission(self.mission.clone()))
            }
            GameState::Results(_) => {
                let any =
                    (0..mc2_platform::keys::KEY_SLOTS as u32).any(|k| frame.input.key_pressed(k));
                any.then_some(StateRequest::MainMenu)
            }
            _ => None,
        }
    }
}

/// Which pointer to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct CursorRequest {
    /// From the camera (rotate while the camera button is held).
    pub camera: CameraCursor,
    /// From the order logic: a `mState_*` cursor id (P3), `None` for the normal pointer.
    pub order_state: Option<u32>,
}

/// Draws the pointer last, over everything.
pub trait CursorLayer {
    /// `pos` in game pixels; `ctx.scale()` converts to the view's pixels.
    fn draw(&mut self, ctx: &mut DrawContext<'_>, pos: (f32, f32), request: CursorRequest);
}

/// A plain arrow (or a diamond while rotating the camera) until `mc2-ui` draws the game's
/// cursor sprites.
pub struct StandInCursor {
    painter: OverlayPainter,
}

impl StandInCursor {
    pub fn new(gpu: &Gpu, format: wgpu::TextureFormat) -> Self {
        Self {
            painter: OverlayPainter::new(gpu, format),
        }
    }
}

impl CursorLayer for StandInCursor {
    fn draw(&mut self, ctx: &mut DrawContext<'_>, pos: (f32, f32), request: CursorRequest) {
        if self.painter.format() != ctx.target.display_format {
            self.painter = OverlayPainter::new(ctx.gpu, ctx.target.display_format);
        }
        let s = ctx.scale();
        let mut b = OverlayBatch::default();
        cursor_shape(
            &mut b,
            pos.0 * s,
            pos.1 * s,
            s,
            request.camera == CameraCursor::Rotate,
        );
        self.painter.draw(
            ctx.gpu,
            ctx.encoder,
            &ctx.target.display_view,
            ctx.size,
            &b,
            None,
        );
    }
}

/// The game frame as the order logic sees it.
pub struct OrderContext<'a> {
    pub frame: u64,
    pub user_input: &'a UserInput,
    pub input: &'a InputState,
    pub camera: &'a CameraRig,
    /// The game screen size (pointer coordinates).
    pub game_size: (f32, f32),
    /// `true` when the camera took the mouse this frame (`moveCameraAround` returned 1: the
    /// original then skips selection and orders).
    pub camera_handled: bool,
    /// The terrain point under the pointer (`eye->inverseProject(mouseXY, wPos)`), if any.
    pub mouse_world: Option<Vec3>,
    pub paused: bool,
}

/// Selection and orders (P3: the order half of `code/missiongui.cpp`: `updateOldStyle`,
/// `updateAOEStyle`, `doMove`, `doAttack`, `doJump`, `forceShot`, force groups, waypoints).
pub trait OrderHook<C> {
    /// `true` only when this route turns physical player input into commands
    /// sent to the simulation.  P3 readiness uses this instead of treating a
    /// hook's mere presence as a playable order path.
    fn emits_sim_commands(&self) -> bool {
        false
    }
    /// A non-camera hot key ran (attack range keys, jump, guard, force groups ...).
    fn hotkey(&mut self, _fired: Fired, _ctx: &OrderContext<'_>) {}
    /// The player orders the HUD produced this frame (control panel, tactical map and world
    /// clicks through the order bridge); push them to the frame's commands.
    fn player_commands(&mut self, _commands: &[mc2_sim::PlayerCommand], _out: &mut Vec<C>) {}
    /// Mouse selection and orders for this frame; push the resulting player commands.
    fn update(&mut self, _ctx: &OrderContext<'_>, _out: &mut Vec<C>) {}
    /// The cursor the order logic wants (`mState_*`), if any.
    fn cursor(&self) -> Option<u32> {
        None
    }
}

/// No selection or orders: the viewer of milestone P1.
#[derive(Debug, Default)]
pub struct NoOrders {
    /// Hot keys that reached the hook (diagnostics and tests).
    pub hotkeys: Vec<Fired>,
}

impl<C> OrderHook<C> for NoOrders {
    fn hotkey(&mut self, fired: Fired, _ctx: &OrderContext<'_>) {
        self.hotkeys.push(fired);
    }
}

/// Particle and card effects drawn inside the scene pass (`mc2-fx`'s `FxSystem` behind the fx lane's
/// adapter: it spawns effects from the frame's `SimEvent`s through a [`crate::sim::SimEventSink`]
/// and hands its draw list over here).
pub trait SceneEffects {
    /// Effect batches for this frame, seen from `camera`; `time` is the game time in seconds.
    fn batches(
        &mut self,
        camera: &mc2_render::camera::GameCamera,
        time: f32,
        out: &mut Vec<mc2_render::fx::FxBatch>,
    );

    /// The crater and footprint decals on the terrain (`CraterManager::render`), lit by the scene's `ambient`.
    fn decals(&mut self, _ambient: [f32; 3], _out: &mut Vec<mc2_render::fx::FxBatch>) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{CameraData, CameraRig, MissionCameraStart};
    use crate::userinput::UserInput;
    use mc2_platform::input::{InputEvent, KeyStatus, MouseButton};
    use mc2_platform::viewport::{Fit, Viewport};
    use mc2_render::camera::CameraSettings;

    /// A deliberately small a3-orders stand-in.  It models the contract P3
    /// needs: physical input selects something first, then a later player
    /// gesture emits a simulation command.
    #[derive(Default)]
    struct StubOrderRoute {
        selected: bool,
    }

    impl OrderHook<&'static str> for StubOrderRoute {
        fn emits_sim_commands(&self) -> bool {
            true
        }

        fn update(&mut self, ctx: &OrderContext<'_>, out: &mut Vec<&'static str>) {
            if ctx.input.mouse_button_status(MouseButton::Left) == KeyStatus::Pressed {
                self.selected = true;
            }
            if self.selected
                && ctx.input.mouse_button_status(MouseButton::Right) == KeyStatus::Released
            {
                out.push("attack");
            }
        }
    }

    fn context<'a>(
        input: &'a InputState,
        user_input: &'a UserInput,
        camera: &'a CameraRig,
    ) -> OrderContext<'a> {
        OrderContext {
            frame: 0,
            user_input,
            input,
            camera,
            game_size: (800.0, 600.0),
            camera_handled: false,
            mouse_world: None,
            paused: false,
        }
    }

    #[test]
    fn physical_input_flows_through_selection_to_an_emitted_sim_command() {
        let mut input = InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch));
        let user_input = UserInput::new();
        let camera = CameraRig::new(
            CameraSettings::default(),
            &MissionCameraStart::default(),
            CameraData::default(),
            [(1200.0, 35.0); 4],
            (800.0, 600.0),
        );
        let mut route = StubOrderRoute::default();
        let mut commands = Vec::new();

        input.handle(InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        });
        route.update(&context(&input, &user_input, &camera), &mut commands);
        assert!(
            route.selected,
            "the physical selection click reaches the route"
        );
        assert!(commands.is_empty());
        input.end_frame();

        input.handle(InputEvent::Button {
            button: MouseButton::Right,
            pressed: true,
        });
        input.end_frame();
        input.handle(InputEvent::Button {
            button: MouseButton::Right,
            pressed: false,
        });
        route.update(&context(&input, &user_input, &camera), &mut commands);

        assert!(route.emits_sim_commands());
        assert_eq!(commands, ["attack"]);
    }
}
