// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Camera control: how the player and the game move the mission camera.
//!
//! The camera *model* (projection, orbit, matrices) is `mc2_render::camera::GameCamera`
//! (`mclib/camera.cpp`).  This module ports the camera *behaviour* on top of it:
//! * `mclib/camera.cpp`: `Camera::init` (fit fields), `setPosition` (map border clamp, terrain
//!   riding with the "swoopy" height easing), `update`, `setCameraOrigin` (the line-of-sight pitch
//!   correction that lifts the camera out of hills after turn 3), `calculateProjectionConstants`,
//!   `zoomValue`/`ZoomIn`/`ZoomOut`/`ZoomNormal`/`ZoomTight`/`ZoomMax`/`ZoomMin`/`ZoomDefault`,
//!   `tiltValue`/`tiltUp`/`tiltDown`/`tiltNormal`, `rotateLeft`/`rotateRight`/`rotateNormal`,
//!   `setCameraRotation`, `moveLeft`/`moveRight`/`moveUp`/`moveDown`, `setCameraView`,
//!   `allNormal`, the `cameraZoom`/`cameraTilt` view tables;
//! * `code/gamecam.cpp`: `GameCamera::activate` (start on the first player mover, all normal) and
//!   `GameCamera::update` (altitude cap from the tilt, near/far planes from the altitude, target
//!   following);
//! * `code/missiongui.cpp`: the camera commands (`scrollUp` ... `tiltDown`, `zoomChoiceIn/Out`,
//!   `cameraNormal`, `cameraDefault` ... `cameraAssign3`), `moveCameraAround` (edge scrolling,
//!   camera-button drag rotation and tilt, wheel zoom, middle-click reset) and
//!   `MissionInterfaceManager::init` (`[CameraData]` of `missionScrn.fit`).
//!
//! Scripted camera moves (`updateGoalVelocity/Position/Rotation/FOV`, driven from ABL) are not part
//! of P1: they arrive as simulation events in P2/P3 and go through [`CameraRig::follow`] /
//! [`CameraRig::set_position`].

use mc2_formats::ini::FitIni;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::math::Vec3;
use mc2_render::terrain::{TerrainMap, WORLD_UNITS_PER_VERTEX};

/// `DEGREES_TO_RADS` (`mclib/dstd.h`).
pub const DEGREES_TO_RADS: f32 = std::f32::consts::PI / 180.0;
/// The `0.7071f` factor of the view-cone and sphere-clip constants (`Camera::init`,
/// `calculateProjectionConstants`): a rounded 1/sqrt(2), kept as written.
#[allow(clippy::approx_constant)]
pub const FOV_FACTOR: f32 = 0.7071;
/// `NORM_PERSPECTIVE`.
pub const NORM_PERSPECTIVE: f32 = 35.0;
/// `Camera::AltitudeTight`.
pub const ALTITUDE_TIGHT: f32 = 800.0;
/// `CAM_THRESHOLD`: height difference that makes the camera ease to the new ground height.
pub const CAM_THRESHOLD: f32 = 150.0;
/// `ELEVATION_BUFFER` / `ANGLE_UP_INC` of `setCameraOrigin`.
pub const ELEVATION_BUFFER: f32 = 125.0;
pub const ANGLE_UP_INC: f32 = 0.25;
/// `Camera::MinNearPlane` ... `MaxFarPlane`.
pub const MIN_NEAR_PLANE: f32 = -100.0;
pub const MAX_NEAR_PLANE: f32 = -400.0;
pub const MIN_FAR_PLANE: f32 = 61500.0;
pub const MAX_FAR_PLANE: f32 = 61555.0;
/// `rotateNormal`.
pub const NORMAL_ROTATION: f32 = -170.0;
/// Initial global `frameLength` before the first `DoGameLogic` update (`mclib/timing.cpp`).
pub const INITIAL_FRAME_LENGTH: f32 = 0.05;

/// What the camera rides on: `land->getTerrainElevation`, the map size and the water level.
pub trait Ground {
    fn elevation(&self, x: f32, y: f32) -> f32;
    /// `Terrain::worldUnitsMapSide`.
    fn world_side(&self) -> f32;
    /// `Terrain::waterElevation`.
    fn water_elevation(&self) -> f32;
}

/// A mission's terrain.
pub struct MissionGround<'a> {
    pub terrain: &'a TerrainMap,
    pub water: f32,
}

impl Ground for MissionGround<'_> {
    fn elevation(&self, x: f32, y: f32) -> f32 {
        self.terrain.elevation_at(x, y)
    }
    fn world_side(&self) -> f32 {
        self.terrain.world_side()
    }
    fn water_elevation(&self) -> f32 {
        self.water
    }
}

/// A flat square map (tests and tools).
#[derive(Clone, Copy, Debug)]
pub struct FlatGround {
    pub side: f32,
    pub height: f32,
    pub water: f32,
}

impl Ground for FlatGround {
    fn elevation(&self, _x: f32, _y: f32) -> f32 {
        self.height
    }
    fn world_side(&self) -> f32 {
        self.side
    }
    fn water_elevation(&self) -> f32 {
        self.water
    }
}

/// `[CameraData]` of `data/art/missionScrn.fit` (`MissionInterfaceManager::init`).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraData {
    pub scroll_inc: f32,
    pub rotation_inc: f32,
    pub zoom_inc: f32,
    pub scroll_left: f32,
    pub scroll_right: f32,
    pub scroll_up: f32,
    pub scroll_down: f32,
    pub base_frame_length: f32,
    /// `RotationDegPerSec` ("actually degrees per frameLength").
    pub deg_per_sec_rot: f32,
    /// `MouseDragThreshold`, pixels.
    pub mouse_drag_threshold: f32,
    /// `MouseDoubleClickThreshold`, seconds.
    pub double_click_threshold: f32,
}

impl Default for CameraData {
    /// The shipped `missionScrn.fit` values.
    fn default() -> Self {
        Self {
            scroll_inc: 5000.0,
            rotation_inc: 11.25,
            zoom_inc: 10.0,
            scroll_left: 5.0,
            scroll_right: 5.0,
            scroll_up: 5.0,
            scroll_down: 5.0,
            base_frame_length: 15.0,
            deg_per_sec_rot: 6000.0,
            mouse_drag_threshold: 10.0,
            double_click_threshold: 0.2,
        }
    }
}

impl CameraData {
    /// Reads `[CameraData]`; missing keys keep the shipped values.
    pub fn from_fit(fit: &mut FitIni) -> Self {
        let mut d = Self::default();
        if fit.seek_block("CameraData").is_err() {
            return d;
        }
        let f = |name: &str, v: &mut f32| {
            if let Ok(x) = fit.read_f32(name) {
                *v = x;
            } else if let Ok(x) = fit.read_i32(name) {
                *v = x as f32;
            }
        };
        f("ScrollIncrement", &mut d.scroll_inc);
        f("RotationIncrement", &mut d.rotation_inc);
        f("ZoomIncrement", &mut d.zoom_inc);
        f("ScrollLeft", &mut d.scroll_left);
        f("ScrollRight", &mut d.scroll_right);
        f("ScrollUp", &mut d.scroll_up);
        f("ScrollDown", &mut d.scroll_down);
        f("BaseFrameLength", &mut d.base_frame_length);
        f("RotationDegPerSec", &mut d.deg_per_sec_rot);
        f("MouseDragThreshold", &mut d.mouse_drag_threshold);
        f("MouseDoubleClickThreshold", &mut d.double_click_threshold);
        d
    }
}

/// The camera values `prefs.cfg` overrides (`code/mechcmd2.cpp` prefs loading).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraPrefs {
    /// `GameVisibleVertices`: side of the terrain vertex window around the camera.
    pub visible_vertices: i32,
    pub max_clip_distance: f32,
    pub min_haze_distance: f32,
    /// `View0Zoom/Tilt` ... `View3Zoom/Tilt` (`Camera::cameraZoom/cameraTilt`).
    pub views: [(f32, f32); 4],
    /// `useLeftRightMouseProfile`: true is the default "AOE" style (Alt rotates the camera),
    /// false the MechCommander 1 style (right drag rotates).
    pub left_right_mouse_profile: bool,
    /// `useHighObjectDetail` (`HighObjectDetail`): the camera may pick the lower LODs of shapes.
    pub high_object_detail: bool,
    /// `useWaterInterestTexture`: the terrain and water detail bitmaps.
    pub terrain_detail: bool,
    /// `useLocalShadows`.
    pub local_shadows: bool,
    /// `pilotVideos`.
    pub pilot_videos: bool,
    /// `asyncMouse` (`mc2UseAsyncMouse`).
    pub async_mouse: bool,
    /// `renderer` (0 hardware).
    pub renderer: i32,
    /// The options screen's hot keys by `commands[]` index; empty is the default table.
    pub hot_keys: Vec<u32>,
}

impl Default for CameraPrefs {
    /// The fallbacks of the prefs loader.
    fn default() -> Self {
        Self {
            visible_vertices: 30,
            max_clip_distance: 3000.0,
            min_haze_distance: 2000.0,
            views: [(1200.0, 35.0); 4],
            left_right_mouse_profile: true,
            high_object_detail: true,
            terrain_detail: true,
            local_shadows: true,
            pilot_videos: true,
            async_mouse: false,
            renderer: 0,
            hot_keys: Vec::new(),
        }
    }
}

impl CameraPrefs {
    /// Reads `[MechCommander2]` of `prefs.cfg`.
    pub fn from_fit(fit: &mut FitIni) -> Self {
        let mut p = Self::default();
        if fit.seek_block("MechCommander2").is_err() {
            return p;
        }
        if let Ok(v) = fit.read_i32("GameVisibleVertices") {
            p.visible_vertices = v;
        }
        if let Ok(v) = fit.read_f32("MaxClipDistance") {
            p.max_clip_distance = v;
        }
        if let Ok(v) = fit.read_f32("MinHazeDistance") {
            p.min_haze_distance = v;
        }
        for (i, view) in p.views.iter_mut().enumerate() {
            if let Ok(v) = fit.read_f32(&format!("View{i}Zoom")) {
                view.0 = v;
            }
            if let Ok(v) = fit.read_f32(&format!("View{i}Tilt")) {
                view.1 = v;
            }
        }
        if let Ok(v) = fit.read_bool("useLeftRightMouseProfile") {
            p.left_right_mouse_profile = v;
        }
        if let Ok(v) = fit.read_bool("HighObjectDetail") {
            p.high_object_detail = v;
        }
        p
    }
}

/// The `[Cameras]` block of a mission `.fit` (`Camera::init`).
#[derive(Clone, Debug, PartialEq)]
pub struct MissionCameraStart {
    pub projection_angle: f32,
    pub position: Vec3,
    pub start_rotation: f32,
    pub new_scale: f32,
}

impl Default for MissionCameraStart {
    fn default() -> Self {
        Self {
            projection_angle: NORM_PERSPECTIVE,
            position: Vec3::ZERO,
            start_rotation: 0.0,
            new_scale: 0.5,
        }
    }
}

impl MissionCameraStart {
    pub fn from_fit(fit: &mut FitIni) -> Self {
        let mut s = Self::default();
        if fit.seek_block("Cameras").is_err() {
            return s;
        }
        if let Ok(v) = fit.read_f32("ProjectionAngle") {
            s.projection_angle = v;
        }
        if let Ok(v) = fit.read_f32("PositionX") {
            s.position.x = v;
        }
        if let Ok(v) = fit.read_f32("PositionY") {
            s.position.y = v;
        }
        if let Ok(v) = fit.read_f32("PositionZ") {
            s.position.z = v;
        }
        if let Ok(v) = fit.read_f32("StartRotation") {
            s.start_rotation = v;
        }
        if let Ok(v) = fit.read_f32("NewScale") {
            s.new_scale = v;
        }
        s
    }
}

/// Which pointer the camera wants (the GUI maps it to a cursor sprite).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CameraCursor {
    #[default]
    Normal,
    /// `mState_ROTATE_CAMERA`.
    Rotate,
}

/// The mission camera's state and behaviour.
#[derive(Clone, Debug)]
pub struct CameraRig {
    /// Projection constants: perspective limits, altitude limits, near/far planes, haze.
    pub settings: CameraSettings,
    /// Viewport in native pixels (`screenResolution`, `viewMulX/Y`).
    pub viewport: (f32, f32),
    pub data: CameraData,
    /// `projectionAngle`: the tilt the player asked for.
    pub projection_angle: f32,
    /// `cameraRotation`.
    pub rotation: f32,
    /// `worldCameraRotation` (moves are relative to it; it changes in step with `rotation`).
    pub world_rotation: f32,
    /// `cameraAltitude` (distance from the target).
    pub altitude: f32,
    /// `cameraAltitudeDesired`.
    pub altitude_desired: f32,
    /// `newScaleFactor` (`getScaleFactor`): 1 fully zoomed in.
    pub scale_factor: f32,
    /// `camera_fov`.
    pub fov: f32,
    /// `cosHalfFOV` (the mover `inView` cone).
    pub cos_half_fov: f32,
    /// `position`: the point the camera looks at, before the next `update`.
    pub position: Vec3,
    /// `goalPositionZ`.
    pub goal_position_z: f32,
    /// `cameraShift` / `cameraShiftZ`: the target of the matrices of the last update.
    pub shift: Vec3,
    /// The pitch actually used by the view after the line-of-sight correction of
    /// `setCameraOrigin` (`localAngle`).
    pub view_angle: f32,
    /// `useLOSAngle` (debug toggle Ctrl+Alt+[ in the original).
    pub use_los_angle: bool,
    /// `cameraZoom[]` / `cameraTilt[]`.
    pub views: [(f32, f32); 4],
    /// `MissionInterfaceManager::realRotation` (accumulates key rotation into 11.25 degree steps).
    pub real_rotation: f32,
    /// `GameCamera::targetObject` position: the camera follows it while set (P3: `setTarget`).
    pub follow: Option<Vec3>,
    /// `Camera::lookTargetObject`: part id the mission script asked the camera to follow (-1 none).
    /// The application resolves it to [`Self::follow`] each frame (`getCamObject`).
    pub look_target: i32,
    /// `Camera::goalPosition` / `goalPosTime`.
    pub goal_position: Vec3,
    pub goal_pos_time: f32,
    /// `Camera::goalRotation` (tilt, rotation, altitude) / `goalRotTime`.
    pub goal_rotation: Vec3,
    pub goal_rot_time: f32,
    /// `Camera::velocity`, `goalVelocity` / `goalVelTime`.
    pub velocity: Vec3,
    pub goal_velocity: Vec3,
    pub goal_vel_time: f32,
    /// `Camera::goalFOV` / `goalFOVTime`.
    pub goal_fov: f32,
    pub goal_fov_time: f32,
    /// The cursor the camera asked for in the last mouse update.
    pub cursor: CameraCursor,
}

impl CameraRig {
    /// `Camera::init` for a mission (`[Cameras]` of the mission fit).
    pub fn new(
        settings: CameraSettings,
        start: &MissionCameraStart,
        data: CameraData,
        views: [(f32, f32); 4],
        viewport: (f32, f32),
    ) -> Self {
        let altitude = settings.altitude_default;
        let mut rig = Self {
            settings,
            viewport,
            data,
            projection_angle: start.projection_angle,
            rotation: 0.0,
            world_rotation: 0.0,
            altitude,
            altitude_desired: altitude,
            scale_factor: start.new_scale,
            fov: 40.0,
            cos_half_fov: (40.0 * FOV_FACTOR * DEGREES_TO_RADS).cos(),
            position: start.position,
            // `Camera::init()` zeroes it and `init(fit)` does not set it.
            goal_position_z: 0.0,
            shift: Vec3::new(0.0, 0.0, start.position.z),
            view_angle: start.projection_angle,
            use_los_angle: true,
            views,
            real_rotation: 0.0,
            follow: None,
            look_target: -1,
            goal_position: Vec3::ZERO,
            goal_pos_time: 0.0,
            goal_rotation: Vec3::ZERO,
            goal_rot_time: 0.0,
            velocity: Vec3::ZERO,
            goal_velocity: Vec3::ZERO,
            goal_vel_time: 0.0,
            goal_fov: 0.0,
            goal_fov_time: 0.0,
            cursor: CameraCursor::Normal,
        };
        rig.set_camera_rotation(start.start_rotation, start.start_rotation);
        rig.scale_factor = 1.0 - (rig.altitude - rig.settings.altitude_minimum) / rig.test_max();
        rig
    }

    /// `GameCamera::activate`: look at the first player mover (if any) and reset tilt, zoom and
    /// rotation (`allNormal`).
    pub fn activate(&mut self, first_mover: Option<Vec3>, ground: &dyn Ground, frame_length: f32) {
        if let Some(p) = first_mover {
            // `GameCamera::activate` calls `Camera::setPosition` with its default `swoopy=true`.
            // Startup then calls `eye->update()` once before the first game tick.
            self.set_position(p, true, ground, frame_length);
        }
        self.all_normal();
    }

    // ---- limits ---------------------------------------------------------------------------

    /// `testMax`: the altitude cap for the current tilt.
    pub fn test_max(&self) -> f32 {
        let s = &self.settings;
        let angle_percent =
            (self.projection_angle - s.min_perspective) / (s.max_perspective - s.min_perspective);
        s.altitude_maximum_lo + (s.altitude_maximum_hi - s.altitude_maximum_lo) * angle_percent
    }

    fn update_scale_factor(&mut self) {
        self.scale_factor =
            1.0 - (self.altitude - self.settings.altitude_minimum) / self.test_max();
    }

    // ---- zoom -----------------------------------------------------------------------------

    /// `zoomValue`.
    pub fn zoom_value(&mut self, value: f32) {
        self.altitude = value;
        self.altitude_desired = self.altitude;
        if self.altitude < self.settings.altitude_minimum {
            self.altitude = self.settings.altitude_minimum;
        }
        self.update_scale_factor();
        self.shift.z = self.position.z;
    }

    /// `ZoomIn`.
    pub fn zoom_in(&mut self, amount: f32) {
        self.altitude -= amount * 1500.0;
        self.altitude_desired = self.altitude;
        if self.altitude < self.settings.altitude_minimum {
            self.altitude = self.settings.altitude_minimum;
        }
        self.update_scale_factor();
        self.shift.z = self.position.z;
    }

    /// `ZoomOut`.
    pub fn zoom_out(&mut self, amount: f32) {
        let test_max = self.test_max();
        self.altitude += amount * 1500.0;
        self.altitude_desired = self.altitude;
        if self.altitude > test_max {
            self.altitude = test_max;
        }
        self.update_scale_factor();
        self.shift.z = self.position.z;
    }

    fn zoom_to(&mut self, altitude: f32) {
        self.fov = 40.0;
        self.cos_half_fov = (self.fov * DEGREES_TO_RADS).cos();
        self.altitude = altitude;
        self.altitude_desired = altitude;
        self.update_scale_factor();
        self.shift.z = self.position.z;
    }

    /// `ZoomNormal` / `ZoomDefault`.
    pub fn zoom_normal(&mut self) {
        self.zoom_to(self.settings.altitude_default);
    }
    /// `ZoomTight`.
    pub fn zoom_tight(&mut self) {
        self.zoom_to(ALTITUDE_TIGHT);
    }
    /// `ZoomMax` (all the way in).
    pub fn zoom_max(&mut self) {
        self.zoom_to(self.settings.altitude_minimum);
    }
    /// `ZoomMin` (all the way out).
    pub fn zoom_min(&mut self) {
        let m = self.test_max();
        self.zoom_to(m);
    }

    // ---- tilt -----------------------------------------------------------------------------

    /// `tiltValue`.
    pub fn tilt_value(&mut self, value: f32) {
        self.projection_angle = value.max(self.settings.min_perspective);
    }
    /// `tiltUp`: towards the horizon.
    pub fn tilt_up(&mut self, amount: f32) {
        self.projection_angle -= amount;
        if self.projection_angle < self.settings.min_perspective {
            self.projection_angle = self.settings.min_perspective;
        }
    }
    /// `tiltDown`: towards a top-down view.
    pub fn tilt_down(&mut self, amount: f32) {
        self.projection_angle += amount;
        if self.projection_angle > self.settings.max_perspective {
            self.projection_angle = self.settings.max_perspective;
        }
    }
    /// `tiltNormal`.
    pub fn tilt_normal(&mut self) {
        self.projection_angle = NORM_PERSPECTIVE;
    }

    // ---- rotation -------------------------------------------------------------------------

    /// `setCameraRotation`: both angles wrapped into -180..180.
    pub fn set_camera_rotation(&mut self, angle: f32, world_angle: f32) {
        let wrap = |mut a: f32| {
            if a > 180.0 {
                a -= 360.0;
            }
            if a < -180.0 {
                a += 360.0;
            }
            a
        };
        self.world_rotation = wrap(world_angle);
        self.rotation = wrap(angle);
    }
    /// `rotateLeft`.
    pub fn rotate_left(&mut self, amount: f32) {
        self.set_camera_rotation(self.rotation + amount, self.world_rotation + amount);
        self.shift.z = self.position.z;
    }
    /// `rotateRight`.
    pub fn rotate_right(&mut self, amount: f32) {
        self.set_camera_rotation(self.rotation - amount, self.world_rotation - amount);
        self.shift.z = self.position.z;
    }
    /// `rotateNormal`.
    pub fn rotate_normal(&mut self) {
        self.set_camera_rotation(NORMAL_ROTATION, NORMAL_ROTATION);
        self.shift.z = self.position.z;
    }

    /// `allNormal`: tilt, zoom and rotation back to the defaults.
    pub fn all_normal(&mut self) {
        self.tilt_normal();
        self.zoom_normal();
        self.rotate_normal();
    }

    /// `setCameraView`: one of the four stored zoom/tilt views.
    pub fn set_camera_view(&mut self, n: usize) {
        if let Some(&(zoom, tilt)) = self.views.get(n) {
            self.zoom_value(zoom);
            self.tilt_value(tilt);
        }
    }

    /// `cameraAssign0..3`: store the current altitude and tilt as a view.
    pub fn assign_view(&mut self, n: usize) {
        if let Some(v) = self.views.get_mut(n) {
            *v = (self.altitude, self.projection_angle);
        }
    }

    // ---- position -------------------------------------------------------------------------

    /// `Camera::setPosition`: clamp to the map border, ride the terrain (never below the
    /// water); with `swoopy` the height eases towards a new ground level only once it differs
    /// by more than [`CAM_THRESHOLD`].
    pub fn set_position(&mut self, p: Vec3, swoopy: bool, ground: &dyn Ground, frame_length: f32) {
        self.position = p;
        let mut max_visual = ground.world_side() / 2.0 - WORLD_UNITS_PER_VERTEX * 2.0;
        // !drawTerrainGrid: keep the camera away from the edge
        max_visual -= WORLD_UNITS_PER_VERTEX * 6.0;
        if self.position.x > max_visual {
            self.position.x = max_visual;
        }
        if self.position.y > max_visual {
            self.position.y = max_visual;
        }
        if self.position.x < -max_visual {
            self.position.x = -max_visual;
        }
        if self.position.y < -max_visual {
            self.position.y = -max_visual;
        }
        let mut shift_z = ground.elevation(self.position.x, self.position.y);
        if shift_z < ground.water_elevation() {
            shift_z = ground.water_elevation();
        }
        if swoopy {
            if (self.position.z - shift_z).abs() > CAM_THRESHOLD {
                self.goal_position_z = shift_z;
            }
            if (self.goal_position_z - self.position.z).abs() > 10.0 {
                let offset = (self.goal_position_z - self.position.z) * 0.5 * frame_length;
                self.position.z += offset;
            }
            self.shift.z = self.position.z;
        } else {
            self.position.z = shift_z;
            self.goal_position_z = shift_z;
            self.shift.z = shift_z;
        }
    }

    /// `OppRotate`.
    fn opp_rotate(v: (f32, f32), angle: f32) -> (f32, f32) {
        let (s, c) = (angle * DEGREES_TO_RADS).sin_cos();
        (v.0 * c - v.1 * s, v.1 * c + v.0 * s)
    }

    fn move_by(&mut self, dir: (f32, f32), amount: f32, ground: &dyn Ground, fl: f32) {
        // `GameCamera::moveLeft` & co: scrolling drops the followed object
        self.follow = None;
        let (dx, dy) = Self::opp_rotate(dir, self.world_rotation);
        let p = Vec3::new(
            self.position.x + dx * amount,
            self.position.y + dy * amount,
            self.position.z,
        );
        self.set_position(p, true, ground, fl);
    }

    /// `moveLeft` (perspective branch).
    pub fn move_left(&mut self, amount: f32, ground: &dyn Ground, fl: f32) {
        self.move_by((1.0, 0.0), amount, ground, fl);
    }
    pub fn move_right(&mut self, amount: f32, ground: &dyn Ground, fl: f32) {
        self.move_by((-1.0, 0.0), amount, ground, fl);
    }
    pub fn move_up(&mut self, amount: f32, ground: &dyn Ground, fl: f32) {
        self.move_by((0.0, -1.0), amount, ground, fl);
    }
    pub fn move_down(&mut self, amount: f32, ground: &dyn Ground, fl: f32) {
        self.move_by((0.0, 1.0), amount, ground, fl);
    }

    // ---- per-frame update -----------------------------------------------------------------

    /// `GameCamera::update` + `Camera::update` + `calculateProjectionConstants`: called once per
    /// game frame after the input (`Mission::update` step 5).  `turn` is the game turn of this
    /// frame (already incremented).
    pub fn update(&mut self, turn: i64, ground: &dyn Ground, frame_length: f32) {
        if let Some(t) = self.follow {
            self.set_position(t, false, ground, frame_length);
        }
        // Force the altitude under the tilt-dependent maximum (keeps the poly load even).
        let test_max = self.test_max();
        if self.altitude > test_max {
            self.altitude = test_max;
        }
        if self.altitude < test_max && self.altitude_desired > test_max {
            self.altitude = test_max;
        }
        let altitude_percent = (self.altitude - self.settings.altitude_minimum)
            / (test_max - self.settings.altitude_minimum);
        self.settings.near_plane =
            MIN_NEAR_PLANE + (MAX_NEAR_PLANE - MIN_NEAR_PLANE) * altitude_percent;
        self.settings.far_plane =
            MIN_FAR_PLANE + (MAX_FAR_PLANE - MIN_FAR_PLANE) * altitude_percent;

        // Camera::update: the scripted velocity and goals move the position, rotation and field of view
        let mut new_position = self.position;
        self.update_goal_velocity(frame_length);
        if self.velocity.x != 0.0 && self.velocity.y != 0.0 && self.velocity.z != 0.0 {
            new_position = new_position + self.velocity * frame_length;
        }
        self.update_goal_position(&mut new_position, frame_length);
        self.update_goal_rotation(frame_length);
        self.update_goal_fov(frame_length);
        self.set_position(new_position, true, ground, frame_length);
        self.shift = new_position;
        self.set_camera_origin(turn, ground);
    }

    /// `Camera::updateGoalVelocity`.
    fn update_goal_velocity(&mut self, fl: f32) {
        if self.goal_vel_time > 0.0 {
            let d = self.goal_velocity - self.velocity;
            let t = self.goal_vel_time;
            let diff = Vec3::new(d.x / t * fl, d.y / t * fl, d.z / t * fl);
            self.goal_vel_time -= fl;
            if self.goal_vel_time < 0.0 {
                self.velocity = self.goal_velocity;
            } else {
                self.velocity = self.velocity + diff;
            }
        } else {
            self.goal_vel_time = 0.0;
            self.goal_velocity = self.velocity;
        }
    }

    /// `Camera::updateGoalPosition`. The original adds the step to `position`, not to the position
    /// it was given (so a scripted velocity is lost while a goal position is active); kept.
    fn update_goal_position(&mut self, pos: &mut Vec3, fl: f32) {
        if self.goal_pos_time > 0.0 {
            let diff = (self.goal_position - *pos) * (fl / self.goal_pos_time);
            self.goal_pos_time -= fl;
            if self.goal_pos_time < 0.0 {
                *pos = self.goal_position;
            } else {
                *pos = self.position + diff;
            }
        } else {
            self.goal_pos_time = 0.0;
            self.goal_position = *pos;
        }
    }

    /// `Camera::updateGoalRotation`.
    fn update_goal_rotation(&mut self, fl: f32) {
        if self.goal_rot_time > 0.0 {
            let d_tilt = (self.goal_rotation.x - self.projection_angle) / self.goal_rot_time * fl;
            let d_rot = (self.goal_rotation.y - self.rotation) / self.goal_rot_time * fl;
            self.goal_rot_time -= fl;
            if self.goal_rot_time < 0.0 {
                self.projection_angle = self.goal_rotation.x;
                self.rotation = self.goal_rotation.y;
            } else {
                self.projection_angle += d_tilt;
                self.rotation += d_rot;
            }
            self.set_camera_rotation(self.rotation, self.rotation);
        } else {
            self.goal_rot_time = 0.0;
        }
    }

    /// `Camera::updateGoalFOV` (`cosHalfFOV` is only recomputed by the zoom commands, as in the original).
    fn update_goal_fov(&mut self, fl: f32) {
        if self.goal_fov_time > 0.0 {
            let diff = (self.goal_fov - self.fov) / self.goal_fov_time * fl;
            self.goal_fov_time -= fl;
            if self.goal_fov_time < 0.0 {
                self.fov = self.goal_fov;
            } else {
                self.fov += diff;
            }
        } else {
            self.goal_fov_time = 0.0;
        }
    }

    /// The mission script's `SetCamera*` natives (`code/ablmc2.cpp:5339-5765`), applied to the camera
    /// where the script ran (after the frame's `eye->update()`).
    pub fn apply_script(
        &mut self,
        req: &mc2_sim::events::CameraRequest,
        ground: &dyn Ground,
        frame_length: f32,
    ) {
        use mc2_sim::events::CameraRequest as R;
        let v3 = |v: &mc2_sim::math::Vec3| Vec3::new(v.x, v.y, v.z);
        match req {
            // `eye->setPosition(pos, false)`
            R::Position(p) => self.set_position(v3(p), false, ground, frame_length),
            R::GoalPosition(p, t) => {
                self.goal_position = v3(p);
                self.goal_pos_time = *t;
            }
            // `Camera::setRotation`
            R::Rotation(r) => {
                self.projection_angle = r.x;
                self.set_camera_rotation(r.y, r.y);
                self.altitude = r.z.max(self.settings.altitude_minimum);
                self.update_scale_factor();
            }
            R::GoalRotation(r, t) => {
                self.goal_rotation = v3(r);
                self.goal_rot_time = *t;
            }
            R::Zoom(f) => self.fov = *f,
            R::GoalZoom(f, t) => {
                self.goal_fov = *f;
                self.goal_fov_time = *t;
            }
            R::Velocity(v) => self.velocity = v3(v),
            R::GoalVelocity(v, t) => {
                self.goal_velocity = v3(v);
                self.goal_vel_time = *t;
            }
            R::LookObject(id) => self.look_target = *id,
        }
    }

    /// `calculateProjectionConstants` -> `setCameraOrigin` outside `update`: the original recomputes
    /// the camera matrices (and with them the look vector) every time it renders, after the
    /// mission script may have moved the camera this frame.
    pub fn refresh_origin(&mut self, turn: i64, ground: &dyn Ground) {
        self.set_camera_origin(turn, ground);
    }

    /// `setCameraOrigin`, perspective branch: after turn 3 the pitch is raised in
    /// [`ANGLE_UP_INC`] steps until the eye is 75 units above the terrain under it (plus the
    /// [`ELEVATION_BUFFER`]).
    fn set_camera_origin(&mut self, turn: i64, ground: &dyn Ground) {
        let mut local_angle = self.projection_angle;
        if turn > 3 && self.use_los_angle {
            // The original loops until the eye clears the ground; a straight-down view always
            // does, so 2000 steps (500 degrees) is only a guard against bad data.
            for _ in 0..2000 {
                let eye = self.eye_at(local_angle);
                let z = eye.z - ELEVATION_BUFFER;
                if z > ground.elevation(eye.x, eye.y) + 75.0 {
                    break;
                }
                local_angle += ANGLE_UP_INC;
            }
        }
        self.view_angle = local_angle;
    }

    fn eye_at(&self, angle: f32) -> Vec3 {
        let mut c = self.render_camera();
        c.projection_angle = angle;
        c.position_world()
    }

    fn render_camera(&self) -> GameCamera {
        let mut c = GameCamera::new(self.settings.clone(), self.viewport);
        c.target = self.shift;
        c.projection_angle = self.view_angle;
        // `GameCamera::set_rotation` wraps the same way `setCameraRotation` does
        c.set_rotation(self.rotation);
        c.altitude = self.altitude;
        // `GameCamera::activate` and every normal zoom reset overwrite the original
        // `Camera::init` cone (camera.cpp:2528), while the matrix FOV remains 40 degrees.
        // Preserve that visibility-only state for `*Appearance::recalcBounds` in mc2-render.
        c.set_mover_visibility(self.cos_half_fov, self.scale_factor);
        c
    }

    /// The camera the scene is drawn with (the matrices of the last [`Self::update`]).
    pub fn view(&self) -> GameCamera {
        self.render_camera()
    }

    /// Eye position in world coordinates (`getCameraOrigin`, converted out of the Stuff frame).
    pub fn eye_position(&self) -> Vec3 {
        self.render_camera().position_world()
    }

    /// `lookVector` in world coordinates: unit vector from the eye towards the target.
    pub fn look_vector(&self) -> Vec3 {
        (self.shift - self.eye_position()).normalized()
    }

    /// The oracle records the C++ `Stuff::Vector3D` look vector. Its pitch uses Stuff axes
    /// (`x=-world.x`, `y=world.z`, `z=world.y`), rather than the app's game-world look vector.
    pub fn los_pitch(&self) -> f32 {
        let camera = self.render_camera();
        let back = camera.camera_to_world_stuff()[2];
        (-back[2]).atan2(back[0].hypot(back[1])).to_degrees()
    }

    // ---- missiongui commands --------------------------------------------------------------

    fn frame_factor(&self, frame_length: f32) -> f32 {
        frame_length / self.data.base_frame_length
    }

    fn scroll_amount(&self, frame_length: f32) -> f32 {
        self.data.scroll_inc / self.scale_factor * self.frame_factor(frame_length)
    }

    /// `MissionInterfaceManager::scrollUp` (and the three siblings).
    pub fn scroll_up(&mut self, ground: &dyn Ground, fl: f32) {
        let a = self.scroll_amount(fl);
        self.move_up(a, ground, fl);
    }
    pub fn scroll_down(&mut self, ground: &dyn Ground, fl: f32) {
        let a = self.scroll_amount(fl);
        self.move_down(a, ground, fl);
    }
    pub fn scroll_left(&mut self, ground: &dyn Ground, fl: f32) {
        let a = self.scroll_amount(fl);
        self.move_left(a, ground, fl);
    }
    pub fn scroll_right(&mut self, ground: &dyn Ground, fl: f32) {
        let a = self.scroll_amount(fl);
        self.move_right(a, ground, fl);
    }
    /// `zoomOut` / `zoomIn` (keys, every frame while held).
    pub fn zoom_out_cmd(&mut self, fl: f32) {
        let a = self.data.zoom_inc * self.frame_factor(fl) * self.scale_factor;
        self.zoom_out(a);
    }
    pub fn zoom_in_cmd(&mut self, fl: f32) {
        let a = self.data.zoom_inc * self.frame_factor(fl) * self.scale_factor;
        self.zoom_in(a);
    }
    /// `zoomChoiceOut` / `zoomChoiceIn` (the wheel: five times the key step).
    pub fn zoom_choice_out(&mut self, fl: f32) {
        let a = self.data.zoom_inc * self.frame_factor(fl) * 5.0 * self.scale_factor;
        self.zoom_out(a);
    }
    pub fn zoom_choice_in(&mut self, fl: f32) {
        let a = self.data.zoom_inc * self.frame_factor(fl) * 5.0 * self.scale_factor;
        self.zoom_in(a);
    }
    /// `MissionInterfaceManager::rotateLeft`: accumulate, then turn in `RotationIncrement` steps.
    pub fn rotate_left_cmd(&mut self, fl: f32) {
        self.real_rotation += self.data.deg_per_sec_rot * self.frame_factor(fl);
        if self.real_rotation >= self.data.rotation_inc {
            let inc = self.data.rotation_inc;
            self.rotate_left(inc);
            self.real_rotation = 0.0;
        }
    }
    pub fn rotate_right_cmd(&mut self, fl: f32) {
        self.real_rotation -= self.data.deg_per_sec_rot * self.frame_factor(fl);
        if self.real_rotation <= -self.data.rotation_inc {
            let inc = self.data.rotation_inc;
            self.rotate_right(inc);
            self.real_rotation = 0.0;
        }
    }
    /// `MissionInterfaceManager::tiltUp`: note the original calls `eye->tiltDown` (the view
    /// goes towards top-down) and multiplies the frame factor in twice.
    pub fn tilt_up_cmd(&mut self, fl: f32) {
        let s = self.scroll_amount(fl);
        let a = s * self.frame_factor(fl) * 30.0;
        self.tilt_down(a);
    }
    /// `MissionInterfaceManager::tiltDown` (calls `eye->tiltUp`).
    pub fn tilt_down_cmd(&mut self, fl: f32) {
        let s = self.scroll_amount(fl);
        let a = s * self.frame_factor(fl) * 30.0;
        self.tilt_up(a);
    }

    /// The camera half of `MissionInterfaceManager::moveCameraAround`: edge scrolling, the
    /// camera-button drag (rotation and tilt), wheel zoom and the middle-click reset.
    ///
    /// `camera_clicked` is the profile's camera button (Alt held, or a right drag in the MC1
    /// profile); `mouse` the pointer in game pixels; `screen` the game screen size.  Returns
    /// `true` when the camera consumed the mouse this frame (the original returns early and skips
    /// the order logic).
    pub fn mouse_camera(
        &mut self,
        ui: &crate::userinput::UserInput,
        camera_clicked: bool,
        screen: (f32, f32),
        ground: &dyn Ground,
        fl: f32,
    ) -> bool {
        let mut handled = false;
        self.cursor = CameraCursor::Normal;
        let middle_clicked = !ui.is_left_drag() && !ui.is_right_drag() && ui.is_middle_click();
        let (mx, my) = (ui.mouse_x() as f32, ui.mouse_y() as f32);
        // Edge scrolling.  (alariq's build also stops it while paused; the original scrolls.)
        if !camera_clicked {
            if mx <= self.data.scroll_left {
                self.scroll_left(ground, fl);
            }
            if mx >= screen.0 - self.data.scroll_right {
                self.scroll_right(ground, fl);
            }
            if my <= self.data.scroll_up && my >= -self.data.scroll_up {
                self.scroll_up(ground, fl);
            }
            if my >= screen.1 - self.data.scroll_down {
                self.scroll_down(ground, fl);
            }
        }
        if camera_clicked {
            handled = true;
            let dx = ui.mouse_x_delta();
            let rot = self.data.rotation_inc * 0.1 * dx.unsigned_abs() as f32;
            if dx > 0 {
                self.rotate_right(rot);
            } else if dx < 0 {
                self.rotate_left(rot);
            }
            let dy = ui.mouse_y_delta();
            let tilt = self.data.rotation_inc * 0.1 * dy.unsigned_abs() as f32;
            if dy > 0 {
                self.tilt_down(tilt);
            } else if dy < 0 {
                self.tilt_up(tilt);
            }
            self.cursor = CameraCursor::Rotate;
            self.wheel_zoom(ui, fl);
            return handled;
        }
        if self.wheel_zoom(ui, fl) {
            handled = true;
        }
        if middle_clicked {
            self.all_normal();
        }
        handled
    }

    /// "Mouse wheel just picks zooms now": negative (away from the user) zooms in.
    fn wheel_zoom(&mut self, ui: &crate::userinput::UserInput, fl: f32) -> bool {
        let d = ui.mouse_wheel_delta();
        if d > 0 {
            self.zoom_choice_out(fl);
        } else if d < 0 {
            self.zoom_choice_in(fl);
        }
        d != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FL: f32 = 1.0 / 30.0;

    fn ground() -> FlatGround {
        FlatGround {
            side: 120.0 * 128.0,
            height: 400.0,
            water: 350.0,
        }
    }

    fn rig() -> CameraRig {
        let s = CameraSettings {
            max_clip_distance: 3000.0,
            min_haze_distance: 2000.0,
            ..CameraSettings::default()
        };
        let start = MissionCameraStart {
            projection_angle: 21.25,
            position: Vec3::new(1000.0, -500.0, 400.0),
            start_rotation: 59.625,
            new_scale: 0.5,
        };
        let mut r = CameraRig::new(
            s,
            &start,
            CameraData::default(),
            [(1200.0, 35.0), (960.0, 35.0), (800.0, 35.0), (1200.0, 35.0)],
            (1920.0, 1080.0),
        );
        r.activate(Some(Vec3::new(1000.0, -500.0, 400.0)), &ground(), FL);
        r.update(1, &ground(), FL);
        r
    }

    #[test]
    fn script_goal_rotation_reaches_its_goal_in_the_given_time() {
        use mc2_sim::events::CameraRequest as R;
        use mc2_sim::math::Vec3 as SV;
        let g = ground();
        let mut r = rig();
        // 0.1 s = 3 frames at 30 fps: a third of the way each frame, then exactly the goal
        r.apply_script(
            &R::GoalRotation(
                SV {
                    x: 18.0,
                    y: 10.0,
                    z: 0.0,
                },
                0.1,
            ),
            &g,
            FL,
        );
        let (t0, y0) = (r.projection_angle, r.rotation);
        r.update(2, &g, FL);
        assert!((r.projection_angle - (t0 + (18.0 - t0) / 0.1 * FL)).abs() < 1e-4);
        assert!((r.rotation - (y0 + (10.0 - y0) / 0.1 * FL)).abs() < 1e-3);
        r.update(3, &g, FL);
        r.update(4, &g, FL);
        r.update(5, &g, FL);
        assert_eq!((r.projection_angle, r.rotation), (18.0, 10.0));
        assert_eq!(r.goal_rot_time, 0.0);
    }

    #[test]
    fn script_position_is_snapped_to_the_terrain_and_look_object_is_recorded() {
        use mc2_sim::events::CameraRequest as R;
        use mc2_sim::math::Vec3 as SV;
        let g = ground();
        let mut r = rig();
        r.apply_script(
            &R::Position(SV {
                x: 300.0,
                y: 200.0,
                z: 5.0,
            }),
            &g,
            FL,
        );
        // `setPosition(pos, false)`: z is the terrain under it, never below the water
        assert_eq!(
            (r.position.x, r.position.y, r.position.z),
            (300.0, 200.0, 400.0)
        );
        r.apply_script(&R::LookObject(512), &g, FL);
        assert_eq!(r.look_target, 512);
        // scrolling drops the followed position but not the script's look target
        r.follow = Some(Vec3::new(1.0, 2.0, 3.0));
        r.move_left(10.0, &g, FL);
        assert_eq!(r.follow, None);
        assert_eq!(r.look_target, 512);
    }

    #[test]
    fn script_rotation_sets_tilt_rotation_and_altitude_with_the_minimum() {
        use mc2_sim::events::CameraRequest as R;
        use mc2_sim::math::Vec3 as SV;
        let g = ground();
        let mut r = rig();
        r.apply_script(
            &R::Rotation(SV {
                x: 24.5,
                y: 178.75,
                z: 10.0,
            }),
            &g,
            FL,
        );
        assert_eq!(r.projection_angle, 24.5);
        assert_eq!(r.rotation, 178.75);
        assert_eq!(r.altitude, r.settings.altitude_minimum);
    }

    #[test]
    fn activate_is_all_normal() {
        let r = rig();
        assert_eq!(r.projection_angle, 35.0);
        assert_eq!(r.altitude, 1200.0);
        assert_eq!(r.rotation, -170.0);
        assert_eq!(r.world_rotation, -170.0);
        // scale factor of the default altitude at the normal tilt
        let tm = 1500.0 + 100.0 * (35.0 - 20.0) / 68.0;
        assert!((r.scale_factor - (1.0 - (1200.0 - 560.0) / tm)).abs() < 1e-6);
    }

    #[test]
    fn activation_eases_target_like_gamecamera() {
        let mut r = rig();
        let start = Vec3::new(1000.0, -500.0, 700.0);
        r.activate(Some(start), &ground(), FL);
        assert_eq!(r.position, Vec3::new(start.x, start.y, 695.0));
    }

    #[test]
    fn los_pitch_uses_stuff_frame_axes() {
        let start = MissionCameraStart {
            projection_angle: 35.0,
            position: Vec3::ZERO,
            start_rotation: -170.0,
            new_scale: 0.5,
        };
        let rig = CameraRig::new(
            CameraSettings::default(),
            &start,
            CameraData::default(),
            [(1200.0, 35.0); 4],
            (800.0, 600.0),
        );
        assert!((rig.los_pitch() - 53.77546).abs() < 1e-4);
    }

    #[test]
    fn zoom_limits_follow_the_tilt() {
        let mut r = rig();
        for _ in 0..1000 {
            r.zoom_in_cmd(FL);
        }
        assert_eq!(r.altitude, 560.0);
        assert!((r.scale_factor - 1.0).abs() < 1e-6);
        for _ in 0..1000 {
            r.zoom_out_cmd(FL);
        }
        assert!((r.altitude - r.test_max()).abs() < 1e-3);
        // a lower tilt lowers the cap; update() pulls the altitude down to it
        let before = r.altitude;
        for _ in 0..100 {
            r.tilt_down_cmd(FL);
        }
        assert_eq!(r.projection_angle, 20.0);
        r.update(10, &ground(), FL);
        assert_eq!(r.altitude, 1500.0);
        assert!(r.altitude < before);
        // tilting back up does not restore the old altitude on its own (desired > cap)
        for _ in 0..100 {
            r.tilt_up_cmd(FL);
        }
        assert_eq!(r.projection_angle, 88.0);
        r.update(11, &ground(), FL);
        assert_eq!(r.altitude, 1500.0);
    }

    #[test]
    fn key_rotation_steps_by_the_increment() {
        let mut r = rig();
        // 6000 * (1/30) / 15 = 13.33 per frame >= 11.25: one step every frame
        r.rotate_left_cmd(FL);
        assert!((r.rotation - (-170.0 + 11.25)).abs() < 1e-4);
        for _ in 0..32 {
            r.rotate_left_cmd(FL);
        }
        // 33 steps of 11.25 = 371.25: wrapped
        let expect = -170.0 + 33.0 * 11.25 - 360.0;
        assert!((r.rotation - expect).abs() < 1e-3, "{}", r.rotation);
        assert!((-180.0..=180.0).contains(&r.rotation));
        assert_eq!(r.rotation, r.world_rotation);
        // at 60 fps the accumulator needs two frames per step
        let mut r = rig();
        r.rotate_right_cmd(1.0 / 60.0);
        assert_eq!(r.rotation, -170.0);
        r.rotate_right_cmd(1.0 / 60.0);
        assert!((r.rotation - (-170.0 - 11.25 + 360.0)).abs() < 1e-3);
    }

    #[test]
    fn scrolling_moves_relative_to_the_view_and_stops_at_the_border() {
        let mut r = rig();
        r.set_camera_rotation(0.0, 0.0);
        let p0 = r.position;
        r.scroll_up(&ground(), FL);
        // yaw 0 looks south: "up" on screen is south (-y)
        assert!(r.position.y < p0.y && (r.position.x - p0.x).abs() < 1e-3);
        let step = p0.y - r.position.y;
        assert!((step - 5000.0 / r.scale_factor * (FL / 15.0)).abs() < 1e-2);
        r.scroll_left(&ground(), FL);
        assert!(r.position.x > p0.x, "screen-left is east at yaw 0");
        for _ in 0..10_000 {
            r.scroll_right(&ground(), FL);
        }
        let max_visual = 120.0 * 128.0 / 2.0 - 128.0 * 8.0;
        assert_eq!(r.position.x, -max_visual);
    }

    #[test]
    fn the_eye_rises_out_of_hills_after_turn_three() {
        struct Hill;
        impl Ground for Hill {
            fn elevation(&self, x: f32, y: f32) -> f32 {
                // a 2000-unit wall north of the target, where a yaw-0 camera sits
                if y > 300.0 && x.abs() < 5000.0 {
                    2000.0
                } else {
                    0.0
                }
            }
            fn world_side(&self) -> f32 {
                20000.0
            }
            fn water_elevation(&self) -> f32 {
                -100.0
            }
        }
        let mut r = rig();
        r.set_camera_rotation(0.0, 0.0);
        r.set_position(Vec3::ZERO, false, &Hill, FL);
        r.update(2, &Hill, FL);
        assert_eq!(r.view_angle, r.projection_angle);
        r.update(4, &Hill, FL);
        assert!(r.view_angle > r.projection_angle);
        let eye = r.eye_position();
        assert!(eye.z - ELEVATION_BUFFER > Hill.elevation(eye.x, eye.y) + 75.0);
        // the player's tilt is unchanged
        assert_eq!(r.projection_angle, 35.0);
    }

    #[test]
    fn swoopy_height_eases_only_past_the_threshold() {
        let mut r = rig();
        let g = ground();
        r.set_position(Vec3::new(0.0, 0.0, 400.0), false, &g, FL);
        assert_eq!(r.position.z, 400.0);
        // 100 units higher ground: under the threshold, the height stays
        let higher = FlatGround { height: 500.0, ..g };
        r.set_position(Vec3::new(10.0, 0.0, 400.0), true, &higher, FL);
        assert_eq!(r.position.z, 400.0);
        // 300 units: eases half the difference per second
        let cliff = FlatGround { height: 700.0, ..g };
        r.set_position(Vec3::new(20.0, 0.0, 400.0), true, &cliff, FL);
        assert!((r.position.z - (400.0 + 300.0 * 0.5 * FL)).abs() < 1e-3);
    }

    #[test]
    fn views_store_and_recall() {
        let mut r = rig();
        r.set_camera_view(2);
        assert_eq!((r.altitude, r.projection_angle), (800.0, 35.0));
        r.zoom_value(1000.0);
        r.tilt_value(50.0);
        r.assign_view(3);
        r.all_normal();
        r.set_camera_view(3);
        assert_eq!((r.altitude, r.projection_angle), (1000.0, 50.0));
        // tiltValue only clamps the minimum
        r.tilt_value(5.0);
        assert_eq!(r.projection_angle, 20.0);
    }

    #[test]
    fn near_plane_tracks_the_altitude() {
        let mut r = rig();
        r.zoom_max();
        r.update(5, &ground(), FL);
        assert_eq!(r.settings.near_plane, -100.0);
        r.zoom_min();
        r.update(6, &ground(), FL);
        assert!((r.settings.near_plane + 400.0).abs() < 1e-3);
        assert!((r.settings.far_plane - 61555.0).abs() < 1e-2);
    }
}
