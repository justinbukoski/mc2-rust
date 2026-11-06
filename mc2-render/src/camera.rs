// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The game camera: orbit around a ground target, perspective with a fixed horizontal FOV.
//!
//! Ported from `mclib/camera.cpp` (`Camera::init` for the fit fields, `setCameraRotation`,
//! `zoomValue`/`ZoomIn`/`ZoomOut`, `tiltValue`/`tiltUp`/`tiltDown`, `setCameraOrigin`,
//! `setOrthogonal`, `calculateProjectionConstants`) and `Camera::projectZ` in `camera.h`.
//!
//! The original builds `worldToClip` from `Stuff` row-vector matrices and projects on the CPU into
//! pre-transformed screen vertices.  Here the same matrices are built, folded into one column-major
//! matrix for the GPU, and the clip-space convention (game: x,y in 0..1 with y down and a negative
//! `w`, wgpu: x,y in -1..1 with y up and a positive `w`) is converted once in [`GameCamera::gpu_view_proj`].
//!
//! Frames: the game camera works in the `Stuff` frame (x = -world.x, y = world.z, z = world.y);
//! the camera looks along its local **-z** axis, which is why the near plane is -400.

use crate::math::{deg_to_rad, stuff_to_world, world_to_stuff, Mat4, Vec3};

/// The C++ visibility-cone literal (`Camera::init` / `calculateProjectionConstants`). It is a
/// rounded 1/sqrt(2), so keep the shipped `0.7071f` rather than substituting the exact constant.
#[allow(clippy::approx_constant)]
const FOV_FACTOR: f32 = 0.7071;

/// `[CameraSettings]` in `system.cfg` plus the compiled-in defaults of `Camera`.
#[derive(Clone, Debug)]
pub struct CameraSettings {
    pub min_perspective: f32,
    pub max_perspective: f32,
    pub altitude_minimum: f32,
    pub altitude_maximum_lo: f32,
    pub altitude_maximum_hi: f32,
    pub altitude_default: f32,
    pub near_plane: f32,
    pub far_plane: f32,
    pub fov_degrees: f32,
    /// Distance beyond which terrain is fully hazed (`Camera::MaxClipDistance`).
    pub max_clip_distance: f32,
    /// Distance at which haze starts (`Camera::MinHazeDistance`).
    pub min_haze_distance: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        // Camera::MIN/MAX_PERSPECTIVE are overwritten from system.cfg; these are the shipped values.
        Self {
            min_perspective: 20.0,
            max_perspective: 88.0,
            altitude_minimum: 560.0,
            altitude_maximum_lo: 1500.0,
            altitude_maximum_hi: 1600.0,
            altitude_default: 1200.0,
            near_plane: -400.0,
            far_plane: 61555.0,
            fov_degrees: 40.0,
            max_clip_distance: 4000.0,
            min_haze_distance: 3000.0,
        }
    }
}

impl CameraSettings {
    /// Reads `[CameraSettings]` from a parsed `system.cfg`.  Missing keys keep the defaults.
    pub fn from_system_cfg(cfg: &mut mc2_formats::ini::FitIni) -> Self {
        let mut s = Self::default();
        if cfg.seek_block("CameraSettings").is_ok() {
            if let Ok(v) = cfg.read_f32("MaxPerspective") {
                s.max_perspective = v.min(90.0);
            }
            if let Ok(v) = cfg.read_f32("MinPerspective") {
                s.min_perspective = v.max(0.0);
            }
            if let Ok(v) = cfg.read_f32("AltitudeMinimum") {
                s.altitude_minimum = v;
            }
            if let Ok(v) = cfg.read_f32("AltitudeMaximumHi") {
                s.altitude_maximum_hi = v;
            }
            if let Ok(v) = cfg.read_f32("AltitudeMaximumLo") {
                s.altitude_maximum_lo = v;
            }
        }
        s
    }
}

#[derive(Clone, Debug)]
pub struct GameCamera {
    pub settings: CameraSettings,
    /// Ground point the camera orbits (world x, y) and its elevation (`cameraShiftZ`).
    pub target: Vec3,
    /// Tilt above the horizon in degrees (`projectionAngle`).
    pub projection_angle: f32,
    /// Yaw in degrees (`cameraRotation`).
    pub rotation: f32,
    /// Distance from the target (`cameraAltitude`).
    pub altitude: f32,
    /// Viewport size in pixels, used for the aspect ratio.
    pub viewport: (f32, f32),
    /// `Camera::cosHalfFOV` for mover visibility.  It begins with the value calculated during
    /// `Camera::init`; the application carries the later value across camera activation and zoom
    /// reset, where the C++ camera deliberately changes it.
    pub mover_cos_half_fov: f32,
    /// `Camera::newScaleFactor` for the cheap appearance screen-bounds rejection.  A standalone
    /// render camera derives it from altitude; `mc2-app` supplies its authoritative frame value.
    pub mover_scale_factor: f32,
}

impl GameCamera {
    pub fn new(settings: CameraSettings, viewport: (f32, f32)) -> Self {
        let altitude = settings.altitude_default;
        let mover_cos_half_fov = deg_to_rad(settings.fov_degrees * FOV_FACTOR).cos();
        let altitude_limit = settings.altitude_maximum_lo
            + (settings.altitude_maximum_hi - settings.altitude_maximum_lo)
                * (20.0 - settings.min_perspective)
                / (settings.max_perspective - settings.min_perspective);
        let mover_scale_factor = 1.0 - (altitude - settings.altitude_minimum) / altitude_limit;
        Self {
            settings,
            target: Vec3::ZERO,
            projection_angle: 20.0,
            rotation: 0.0,
            altitude,
            viewport,
            mover_cos_half_fov,
            mover_scale_factor,
        }
    }

    /// Camera for a mission `.fit` `[Cameras]` block (position, projection angle, start rotation).
    pub fn from_fit(
        settings: CameraSettings,
        fit: &mut mc2_formats::ini::FitIni,
        viewport: (f32, f32),
    ) -> Self {
        let mut cam = Self::new(settings, viewport);
        if fit.seek_block("Cameras").is_ok() {
            if let Ok(v) = fit.read_f32("ProjectionAngle") {
                cam.projection_angle = v;
            }
            if let Ok(x) = fit.read_f32("PositionX") {
                cam.target.x = x;
            }
            if let Ok(y) = fit.read_f32("PositionY") {
                cam.target.y = y;
            }
            if let Ok(z) = fit.read_f32("PositionZ") {
                cam.target.z = z;
            }
            if let Ok(r) = fit.read_f32("StartRotation") {
                cam.set_rotation(r);
            }
        }
        cam
    }

    /// `setCameraRotation`: keep the angle in -180..180.
    pub fn set_rotation(&mut self, angle: f32) {
        let mut a = angle;
        if a > 180.0 {
            a -= 360.0;
        }
        if a < -180.0 {
            a += 360.0;
        }
        self.rotation = a;
    }

    pub fn rotate_left(&mut self, amount: f32) {
        self.set_rotation(self.rotation + amount);
    }
    pub fn rotate_right(&mut self, amount: f32) {
        self.set_rotation(self.rotation - amount);
    }

    /// Upper altitude limit for the current tilt (`testMax` in the C++).
    pub fn altitude_limit(&self) -> f32 {
        let s = &self.settings;
        let angle_percent =
            (self.projection_angle - s.min_perspective) / (s.max_perspective - s.min_perspective);
        s.altitude_maximum_lo + (s.altitude_maximum_hi - s.altitude_maximum_lo) * angle_percent
    }

    /// Zoom scale factor in 0..1 (`newScaleFactor`); 1 is fully zoomed in.
    pub fn scale_factor(&self) -> f32 {
        1.0 - (self.altitude - self.settings.altitude_minimum) / self.altitude_limit()
    }

    /// Sets the two appearance-visibility values calculated by the camera controller.  They are
    /// inputs to `*Appearance::recalcBounds`, not projection parameters.
    pub fn set_mover_visibility(&mut self, cos_half_fov: f32, scale_factor: f32) {
        self.mover_cos_half_fov = cos_half_fov;
        self.mover_scale_factor = scale_factor;
    }

    /// Keep the standalone camera's preliminary appearance-rectangle scale in lockstep with
    /// `Camera::zoomValue` (`camera.cpp:2442-2460`).  The app replaces this with the controller
    /// value through [`Self::set_mover_visibility`].
    fn sync_standalone_mover_scale(&mut self) {
        self.mover_scale_factor = self.scale_factor();
    }

    /// `zoomValue`: set the altitude, no upper clamp (the original only clamps at the minimum).
    pub fn zoom_value(&mut self, altitude: f32) {
        self.altitude = altitude.max(self.settings.altitude_minimum);
        self.sync_standalone_mover_scale();
    }
    /// `ZoomIn`.
    pub fn zoom_in(&mut self, amount: f32) {
        self.altitude = (self.altitude - amount * 1500.0).max(self.settings.altitude_minimum);
        self.sync_standalone_mover_scale();
    }
    /// `ZoomOut`.
    pub fn zoom_out(&mut self, amount: f32) {
        let limit = self.altitude_limit();
        self.altitude = (self.altitude + amount * 1500.0).min(limit);
        self.sync_standalone_mover_scale();
    }
    /// `tiltValue`.
    pub fn tilt_value(&mut self, angle: f32) {
        self.projection_angle = angle.max(self.settings.min_perspective);
    }
    /// `tiltUp`: lowers the angle towards the horizon.
    pub fn tilt_up(&mut self, amount: f32) {
        self.projection_angle = (self.projection_angle - amount).max(self.settings.min_perspective);
    }
    /// `tiltDown`: raises the angle towards a top-down view.
    pub fn tilt_down(&mut self, amount: f32) {
        self.projection_angle = (self.projection_angle + amount).min(self.settings.max_perspective);
    }

    /// `Camera::setPosition`: keeps the target inside the map border and rides it on the
    /// terrain, but never below the water level.
    pub fn set_position(
        &mut self,
        x: f32,
        y: f32,
        map_world_side: f32,
        ground: impl Fn(f32, f32) -> f32,
        water_elevation: f32,
    ) {
        // maxVisual = half the map - 2 vertices - 6 vertices (non-editor).
        let max_visual = map_world_side / 2.0 - 128.0 * 2.0 - 128.0 * 6.0;
        let x = x.clamp(-max_visual, max_visual);
        let y = y.clamp(-max_visual, max_visual);
        let z = ground(x, y).max(water_elevation);
        self.target = Vec3::new(x, y, z);
    }

    /// Horizontal half-extent tangent (`tan(fov/2)`).
    fn tan_half_fov(&self) -> f32 {
        (deg_to_rad(self.settings.fov_degrees) * 0.5).tan()
    }

    fn height_to_width(&self) -> f32 {
        self.viewport.1.max(1.0) / self.viewport.0.max(1.0)
    }

    /// Camera position in the Stuff frame: the orbit offset (`Vector3D::operator=(YawPitchRange)`
    /// with pitch = -angle) plus the target.
    pub fn position_stuff(&self) -> Vec3 {
        let p = deg_to_rad(self.projection_angle);
        let yaw = deg_to_rad(self.rotation);
        let target = world_to_stuff().transform_point(self.target);
        Vec3::new(
            target.x + self.altitude * p.cos() * yaw.sin(),
            target.y + self.altitude * p.sin(),
            target.z + self.altitude * p.cos() * yaw.cos(),
        )
    }

    /// Camera position in game world coordinates.
    pub fn position_world(&self) -> Vec3 {
        stuff_to_world().transform_point(self.position_stuff())
    }

    /// Camera axes in the Stuff frame: `cameraOrigin.BuildRotation(EulerAngles(-angle, yaw, 0))`
    /// gives the rows (local x = left of the *backwards* facing frame, local y = up, local z =
    /// from the target towards the camera).
    fn axes_stuff(&self) -> (Vec3, Vec3, Vec3) {
        let p = deg_to_rad(self.projection_angle);
        let yaw = deg_to_rad(self.rotation);
        let (sp, cp) = p.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        let x = Vec3::new(cy, 0.0, -sy);
        let y = Vec3::new(-sp * sy, cp, -sp * cy);
        let z = Vec3::new(cp * sy, sp, cp * cy);
        (x, y, z)
    }

    /// CROSS-CRATE (gosFX): the camera-to-world matrix in the Stuff frame as four rows of
    /// three: the camera's x, y and z axes, then its position
    /// (`MLRClipper::GetCameraToWorldMatrix`, used by camera-facing effects).
    pub fn camera_to_world_stuff(&self) -> [[f32; 3]; 4] {
        let (x, y, z) = self.axes_stuff();
        let t = self.position_stuff();
        [x.to_array(), y.to_array(), z.to_array(), t.to_array()]
    }

    /// `worldToCamera` of the game (Stuff-frame inputs).
    pub fn world_to_camera_stuff(&self) -> Mat4 {
        let (x, y, z) = self.axes_stuff();
        let t = self.position_stuff();
        Mat4::from_rows([
            [x.x, x.y, x.z, -x.dot(t)],
            [y.x, y.y, y.z, -y.dot(t)],
            [z.x, z.y, z.z, -z.dot(t)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    /// `cameraToClip` of `Camera::setOrthogonal` (perspective branch), written as a
    /// column-vector matrix acting on `(x, y, z, 1)` of camera space.
    pub fn camera_to_clip_game(&self) -> Mat4 {
        let near = self.settings.near_plane;
        let far = self.settings.far_plane;
        let t = self.tan_half_fov();
        let h2w = self.height_to_width();
        let left = -(near * t);
        let right = -left;
        let top = -left * h2w;
        let bottom = -top;
        let hr = 1.0 / (left - right);
        let vr = 1.0 / (top - bottom);
        let dr = 1.0 / (far - near);
        // Row-vector matrix elements (i = input axis, j = output column):
        //   [0][0]=near*hr  [1][1]=near*vr  [2][0]=-right*hr  [2][1]=-bottom*vr
        //   [2][2]=far*dr   [2][3]=1        [3][2]=-far*near*dr
        Mat4::from_rows([
            [near * hr, 0.0, -right * hr, 0.0],
            [0.0, near * vr, -bottom * vr, 0.0],
            [0.0, 0.0, far * dr, -far * near * dr],
            [0.0, 0.0, 1.0, 0.0],
        ])
    }

    /// `worldToClip` (game clip space) for **world** coordinates: the product the CPU path uses in
    /// `Camera::projectZ` after its `(-x, z, y)` reorder.
    pub fn world_to_clip_game(&self) -> Mat4 {
        self.camera_to_clip_game()
            .mul(&self.world_to_camera_stuff())
            .mul(&world_to_stuff())
    }

    /// `Camera::projectZ`: screen position (pixels) and depth of a world point.
    /// Returns `(x, y, z, w)` like `screen` in the C++, and whether it is inside the viewport.
    pub fn project_z(&self, world: Vec3) -> ([f32; 4], bool) {
        let c = self
            .world_to_clip_game()
            .transform4([world.x, world.y, world.z, 1.0]);
        let rhw = if c[3] != 0.0 { 1.0 / c[3] } else { 1.0 };
        let sx = c[0] * rhw * self.viewport.0;
        let sy = c[1] * rhw * self.viewport.1;
        let sz = c[2] * rhw;
        let inside = sx >= 0.0 && sy >= 0.0 && sx <= self.viewport.0 && sy <= self.viewport.1;
        ([sx, sy, sz, rhw.abs()], inside)
    }

    /// The `inView` of the appearances' `recalcBounds` for a bounding sphere of `radius` about `centre` (game world
    /// coordinates): within `MaxClipDistance` of the eye (`bdactor.cpp:1131`, `mech3d.cpp:2073`), in front of it, and
    /// the sphere's projected extent overlaps the viewport.  The C++ tests the projected selection box; the sphere
    /// holds that box, so an object just off the screen edge can count as in view here when it is not there.
    pub fn sphere_in_view(&self, centre: Vec3, radius: f32) -> bool {
        let eye = self.position_world();
        let to_centre = centre - eye;
        if to_centre.length() > self.settings.max_clip_distance {
            return false;
        }
        // The eye looks at `target`.
        let depth = to_centre.dot((self.target - eye).normalized());
        if depth < -radius {
            return false;
        }
        if depth < radius {
            // The sphere straddles the eye's plane, where the projection has no meaning: call it in view.
            return true;
        }
        let (c, _) = self.project_z(centre);
        let (mut rx, mut ry) = (0.0_f32, 0.0_f32);
        for axis in [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ] {
            let (p, _) = self.project_z(centre + axis * radius);
            rx = rx.max((p[0] - c[0]).abs());
            ry = ry.max((p[1] - c[1]).abs());
        }
        let (w, h) = self.viewport;
        c[0] + rx >= 0.0 && c[0] - rx <= w && c[1] + ry >= 0.0 && c[1] - ry <= h
    }

    /// The final projected-selection-box test used by the appearance `recalcBounds` paths.
    ///
    /// The C++ first does a cheap sphere/cone rejection, then projects the eight corners of
    /// `typeUpperLeft..typeLowerRight` and keeps an object when that screen rectangle overlaps
    /// the viewport (`bdactor.cpp:1163-1302`, `gvactor.cpp:1621-1732`,
    /// `mech3d.cpp:2125-2244`).  We project the same box directly; doing so is both the final
    /// C++ decision and avoids letting a bounding sphere select an LOD for an object whose box
    /// is wholly off-screen.
    pub fn selection_box_in_view(&self, _centre: Vec3, corners: [Vec3; 8]) -> bool {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for corner in corners {
            let (screen, _) = self.project_z(corner);
            min_x = min_x.min(screen[0]);
            min_y = min_y.min(screen[1]);
            max_x = max_x.max(screen[0]);
            max_y = max_y.max(screen[1]);
        }
        max_x >= 0.0 && max_y >= 0.0 && min_x <= self.viewport.0 && min_y <= self.viewport.1
    }

    /// The preliminary scaled screen rectangle in each appearance's `recalcBounds`.  C++ only
    /// pays to project the type box when this rectangle overlaps the viewport
    /// (`bdactor.cpp:1163-1182`, `gvactor.cpp:1621-1641`, `mech3d.cpp:2125-2145`).
    pub fn rough_screen_bounds_in_view(
        &self,
        centre: Vec3,
        upper_left: (f32, f32),
        lower_right: (f32, f32),
    ) -> bool {
        let (screen, _) = self.project_z(centre);
        let scale = self.mover_scale_factor;
        let ul = (
            screen[0] + upper_left.0 * scale,
            screen[1] + upper_left.1 * scale,
        );
        let lr = (
            screen[0] + lower_right.0 * scale,
            screen[1] + lower_right.1 * scale,
        );
        lr.0 >= 0.0 && lr.1 >= 0.0 && ul.0 <= self.viewport.0 && ul.1 <= self.viewport.1
    }

    /// The initial sphere/cone rejection of `BldgAppearance::recalcBounds` and
    /// `TreeAppearance::recalcBounds` (`bdactor.cpp:1088-1118`, `3598-3628`).
    ///
    /// This is deliberately separate from the final projected-type-box test: C++ rejects a
    /// structure before projecting its box when its bounding sphere is outside these padded
    /// horizontal or vertical angles.  `cameraFrame` has its forward component in the negative
    /// local-z direction in this column-vector port, whereas the original calls it `y`.
    pub fn structure_sphere_in_view(&self, centre: Vec3, radius: f32) -> bool {
        let eye = self.position_world();
        let distance = (centre - eye).length();
        if distance > self.settings.max_clip_distance {
            return false;
        }

        let centre_stuff = world_to_stuff().transform_point(centre);
        let local = self.world_to_camera_stuff().transform_point(centre_stuff);
        let forward = -local.z;
        if forward <= f32::EPSILON || distance <= f32::EPSILON {
            return false;
        }
        let diagonal = (self.viewport.0 * self.viewport.0 + self.viewport.1 * self.viewport.1)
            .sqrt()
            .max(1.0);
        let angle = deg_to_rad(self.settings.fov_degrees * FOV_FACTOR);
        let vertical_clip = (self.viewport.1 / diagonal * angle).tan();
        let horizontal_clip = (self.viewport.0 / diagonal * angle).tan();
        let extent_angle = radius.max(0.0) / distance;
        local.y.abs() / forward <= vertical_clip + extent_angle
            && local.x.abs() / forward <= horizontal_clip + extent_angle
    }

    /// The center-direction rejection used by `GVAppearance::recalcBounds` and
    /// `Mech3DAppearance::recalcBounds` before their projected type boxes
    /// (`gvactor.cpp:1585-1605`, `mech3d.cpp:2089-2110`).
    pub fn mover_centre_in_view(&self, centre: Vec3) -> bool {
        let eye = self.position_world();
        let direction = centre - eye;
        let distance = direction.length();
        if distance <= f32::EPSILON || distance > self.settings.max_clip_distance {
            return false;
        }
        let look = (self.target - eye).normalized();
        let cosine = direction.normalized().dot(look);
        cosine > self.mover_cos_half_fov
    }

    /// Converts game clip space (`x,y` in 0..1 after the divide, y down, `w < 0` in front of the
    /// camera) into wgpu clip space (`x,y` in -1..1, y up, `w > 0`, `z` in 0..1).
    ///
    /// With `W = c.w` (negative in front): `ndc_x = 2X/W - 1`, `ndc_y = 1 - 2Y/W`, `ndc_z = Z/W`.
    /// Multiplying by `-W` removes the sign flip: `(W - 2X, 2Y - W, -Z, -W)`.
    pub fn gpu_view_proj(&self) -> Mat4 {
        let remap = Mat4::from_rows([
            [-2.0, 0.0, 0.0, 1.0],
            [0.0, 2.0, 0.0, -1.0],
            [0.0, 0.0, -1.0, 0.0],
            [0.0, 0.0, 0.0, -1.0],
        ]);
        remap.mul(&self.world_to_clip_game())
    }

    /// World-space ray through a pixel: origin at the camera, unit direction.
    pub fn pixel_ray(&self, px: f32, py: f32) -> (Vec3, Vec3) {
        let inv = self
            .gpu_view_proj()
            .inverse()
            .expect("view-projection is invertible");
        let ndc_x = 2.0 * px / self.viewport.0 - 1.0;
        let ndc_y = 1.0 - 2.0 * py / self.viewport.1;
        let unproject = |z: f32| {
            let p = inv.transform4([ndc_x, ndc_y, z, 1.0]);
            Vec3::new(p[0] / p[3], p[1] / p[3], p[2] / p[3])
        };
        // Depth 1.0 would land behind the camera: the game's projection only reaches
        // far / (far - near) > 1 at infinity.  Two interior depths give the direction.
        let a = unproject(0.05);
        let b = unproject(0.9);
        (self.position_world(), (b - a).normalized())
    }

    /// `Camera::inverseProject` for a flat ground plane: where the pixel's ray meets `z = ground_z`.
    pub fn pixel_to_ground(&self, px: f32, py: f32, ground_z: f32) -> Option<Vec3> {
        let (o, d) = self.pixel_ray(px, py);
        if d.z.abs() < 1e-6 {
            return None;
        }
        let t = (ground_z - o.z) / d.z;
        (t > 0.0).then(|| o + d * t)
    }

    /// Distance haze factor of the terrain (0 = clear, 1 = fully fogged), `Terrain::geometry`.
    pub fn haze_factor(&self, distance: f32) -> f32 {
        let s = &self.settings;
        if distance > s.max_clip_distance {
            1.0
        } else if distance > s.min_haze_distance {
            (distance - s.min_haze_distance) / (s.max_clip_distance - s.min_haze_distance)
        } else {
            0.0
        }
    }

    /// Orthographic `cameraToClip` (`Camera::setOrthogonal`, `!usePerspective`).  The shipped game
    /// forces perspective (`usePerspective = true` at the end of `Camera::init`); kept for tools.
    pub fn orthographic_camera_to_clip(&self, scale_factor: f32) -> Mat4 {
        let inv = 1.0 / scale_factor;
        let left = inv * 300.0;
        let top = left * self.height_to_width();
        let right = -inv * 300.0;
        let bottom = right * self.height_to_width();
        let (near, far) = (-2000.0, 8000.0);
        Mat4::from_rows([
            [1.0 / (left - right), 0.0, 0.0, -right / (left - right)],
            [0.0, 1.0 / (top - bottom), 0.0, -bottom / (top - bottom)],
            [0.0, 0.0, 1.0 / (far - near), -near / (far - near)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> GameCamera {
        let mut c = GameCamera::new(CameraSettings::default(), (1920.0, 1080.0));
        c.target = Vec3::new(2228.96, -2076.08, 400.0);
        c.projection_angle = 21.25;
        c.set_rotation(59.625);
        c
    }

    #[test]
    fn target_projects_to_screen_centre() {
        let c = cam();
        let (s, inside) = c.project_z(c.target);
        assert!(inside);
        assert!((s[0] - 960.0).abs() < 0.01, "x {}", s[0]);
        assert!((s[1] - 540.0).abs() < 0.01, "y {}", s[1]);
        // depth is in the game's 0..1 range and the camera is `altitude` away (rhw = 1/altitude)
        assert!(s[2] > 0.0 && s[2] < 1.0);
        assert!((s[3] - 1.0 / c.altitude).abs() < 1e-6);
    }

    #[test]
    fn horizontal_fov_is_forty_degrees() {
        // A point on the focal plane offset sideways by altitude * tan(20 deg) hits the screen edge.
        let mut c = cam();
        c.projection_angle = 40.0;
        c.set_rotation(0.0);
        let right_stuff = c.axes_stuff().0; // local +x points to screen-right
                                            // screen right in the world frame
        let right_world = stuff_to_world().transform_vector(right_stuff);
        let off = c.altitude * deg_to_rad(20.0).tan();
        let p = c.target + right_world * off;
        let (s, _) = c.project_z(p);
        assert!((s[0] - 1920.0).abs() < 0.5, "right edge, got {}", s[0]);
        let p = c.target - right_world * off;
        let (s, _) = c.project_z(p);
        assert!(s[0].abs() < 0.5, "left edge, got {}", s[0]);
    }

    #[test]
    fn up_is_up_on_screen() {
        let c = cam();
        let (a, _) = c.project_z(c.target);
        let (b, _) = c.project_z(c.target + Vec3::new(0.0, 0.0, 100.0));
        assert!(b[1] < a[1], "higher points have smaller screen y");
    }

    #[test]
    fn yaw_zero_looks_south_from_the_north() {
        let mut c = cam();
        c.set_rotation(0.0);
        let p = c.position_world();
        assert!(p.y > c.target.y, "camera is north of the target at yaw 0");
        assert!(p.x.abs() - c.target.x.abs() < 1e3);
        assert!(p.z > c.target.z);
        // altitude is the orbit radius
        assert!(((p - c.target).length() - c.altitude).abs() < 0.01);
    }

    #[test]
    fn east_is_screen_left_at_yaw_zero() {
        // Derived from the Stuff Euler composition; guards the handedness of the port.
        let mut c = cam();
        c.set_rotation(0.0);
        let (t, _) = c.project_z(c.target);
        let (e, _) = c.project_z(c.target + Vec3::new(100.0, 0.0, 0.0));
        assert!(e[0] < t[0]);
    }

    #[test]
    fn gpu_matrix_agrees_with_cpu_projection() {
        let c = cam();
        let m = c.gpu_view_proj();
        for p in [
            c.target,
            c.target + Vec3::new(300.0, 200.0, 50.0),
            c.target + Vec3::new(-500.0, 100.0, 0.0),
            c.target + Vec3::new(40.0, -700.0, 120.0),
        ] {
            let (s, _) = c.project_z(p);
            let q = m.transform4([p.x, p.y, p.z, 1.0]);
            assert!(q[3] > 0.0, "positive w in front of the camera");
            let nx = q[0] / q[3];
            let ny = q[1] / q[3];
            let nz = q[2] / q[3];
            let gx = (nx * 0.5 + 0.5) * 1920.0;
            let gy = (0.5 - ny * 0.5) * 1080.0;
            assert!((gx - s[0]).abs() < 0.05, "x {gx} vs {}", s[0]);
            assert!((gy - s[1]).abs() < 0.05, "y {gy} vs {}", s[1]);
            assert!((nz - s[2]).abs() < 1e-5, "z {nz} vs {}", s[2]);
            assert!((0.0..=1.0).contains(&nz));
        }
    }

    #[test]
    fn pixel_ray_round_trips_through_ground() {
        let c = cam();
        let hit = c.pixel_to_ground(960.0, 540.0, c.target.z).unwrap();
        assert!((hit - c.target).length() < 0.5, "{hit:?}");
        let (s, _) = c.project_z(Vec3::new(2300.0, -2000.0, c.target.z));
        let back = c.pixel_to_ground(s[0], s[1], c.target.z).unwrap();
        assert!((back.x - 2300.0).abs() < 0.5 && (back.y + 2000.0).abs() < 0.5);
    }

    #[test]
    fn zoom_and_tilt_clamp_like_the_original() {
        let mut c = cam();
        c.zoom_in(10.0);
        assert_eq!(c.altitude, 560.0);
        assert!((c.scale_factor() - 1.0).abs() < 1e-6);
        c.zoom_out(10.0);
        assert!((c.altitude - c.altitude_limit()).abs() < 1e-3);
        assert!((c.mover_scale_factor - c.scale_factor()).abs() < 1e-6);
        c.tilt_up(100.0);
        assert_eq!(c.projection_angle, 20.0);
        c.tilt_down(100.0);
        assert_eq!(c.projection_angle, 88.0);
        // limit interpolates between lo and hi with the tilt
        assert!((c.altitude_limit() - 1600.0 * (88.0 - 20.0) / 68.0).abs() < 1.0);
    }

    #[test]
    fn standalone_zoom_refreshes_the_selection_rectangle_scale() {
        // `Camera::zoomValue` recalculates `newScaleFactor` before recalcBounds uses its
        // preliminary screen rectangle (camera.cpp:2442-2460; mech3d.cpp:2125-2145).
        let mut c = cam();
        let before = c.mover_scale_factor;
        c.zoom_value(c.settings.altitude_minimum + 400.0);
        assert_ne!(before, c.mover_scale_factor);
        assert!((c.mover_scale_factor - c.scale_factor()).abs() < 1e-6);
    }

    #[test]
    fn rotation_wraps() {
        let mut c = cam();
        c.set_rotation(190.0);
        assert_eq!(c.rotation, -170.0);
        c.set_rotation(-190.0);
        assert_eq!(c.rotation, 170.0);
    }

    #[test]
    fn haze_ramps_between_min_and_max_distance() {
        let c = cam();
        assert_eq!(c.haze_factor(2000.0), 0.0);
        assert!((c.haze_factor(3500.0) - 0.5).abs() < 1e-6);
        assert_eq!(c.haze_factor(5000.0), 1.0);
    }

    #[test]
    fn set_position_rides_the_terrain_above_water() {
        let mut c = cam();
        c.set_position(0.0, 0.0, 15360.0, |_, _| 100.0, 350.0);
        assert_eq!(c.target.z, 350.0);
        c.set_position(1e9, -1e9, 15360.0, |_, _| 900.0, 350.0);
        assert_eq!(c.target.x, 15360.0 / 2.0 - 1024.0);
        assert_eq!(c.target.y, -(15360.0 / 2.0 - 1024.0));
        assert_eq!(c.target.z, 900.0);
    }

    #[test]
    fn an_object_leaves_the_view_as_the_camera_pulls_back() {
        // bdactor.cpp:1131 and mech3d.cpp:2073: past MaxClipDistance (4000) the object is not in view, and the LOD
        // rule does not run for it (populate::select_shapes).  The eye is `altitude` from the target.
        let mut c = cam();
        let on_target = c.target;
        assert!(c.sphere_in_view(on_target, 50.0));
        c.altitude = 4500.0;
        assert!(!c.sphere_in_view(on_target, 50.0));
        c.altitude = 1200.0;
        assert!(c.sphere_in_view(on_target, 50.0));
    }

    #[test]
    fn an_object_off_the_screen_or_behind_the_eye_is_not_in_view() {
        let c = cam();
        let eye = c.position_world();
        // 3000 units to the side of the target at 1200 altitude: well outside the 40 degree field.
        assert!(!c.sphere_in_view(c.target + Vec3::new(3000.0, 0.0, 0.0), 50.0));
        // Behind the eye, within clip distance.
        assert!(!c.sphere_in_view(eye + (eye - c.target) * 0.1, 50.0));
    }

    #[test]
    fn selection_box_not_a_bounding_sphere_gates_lod() {
        // The exact second-stage check is the min/max of eight projected box corners:
        // bdactor.cpp:1183-1302, gvactor.cpp:1642-1732 and mech3d.cpp:2146-2244.  A large
        // sphere can overlap the viewport even though this narrow type box does not.
        let c = cam();
        let right = stuff_to_world().transform_vector(c.axes_stuff().0);
        let centre = c.target + right * (c.altitude * c.tan_half_fov() + 100.0);
        assert!(c.sphere_in_view(centre, 200.0));
        let box_corners = [
            Vec3::new(centre.x - 10.0, centre.y - 10.0, centre.z - 10.0),
            Vec3::new(centre.x - 10.0, centre.y - 10.0, centre.z + 10.0),
            Vec3::new(centre.x - 10.0, centre.y + 10.0, centre.z - 10.0),
            Vec3::new(centre.x - 10.0, centre.y + 10.0, centre.z + 10.0),
            Vec3::new(centre.x + 10.0, centre.y - 10.0, centre.z - 10.0),
            Vec3::new(centre.x + 10.0, centre.y - 10.0, centre.z + 10.0),
            Vec3::new(centre.x + 10.0, centre.y + 10.0, centre.z - 10.0),
            Vec3::new(centre.x + 10.0, centre.y + 10.0, centre.z + 10.0),
        ];
        assert!(!c.selection_box_in_view(centre, box_corners));
    }

    #[test]
    fn selection_box_uses_the_final_viewport_each_frame() {
        // The final min/max box overlap is evaluated against the current screen dimensions
        // before the C++ code changes LOD (bdactor.cpp:1263-1325, gvactor.cpp:1694-1755,
        // mech3d.cpp:2205-2267). The game's FOV is horizontal, so the taller 4:3 viewport
        // admits a box that the final 16:9 viewport rejects.
        let mut c = cam();
        c.viewport = (1024.0, 768.0);
        let up = stuff_to_world().transform_vector(c.axes_stuff().1);
        let centre = c.target + up * (c.altitude * c.tan_half_fov() * 0.65);
        let corners = [
            Vec3::new(centre.x - 10.0, centre.y - 10.0, centre.z - 10.0),
            Vec3::new(centre.x - 10.0, centre.y - 10.0, centre.z + 10.0),
            Vec3::new(centre.x - 10.0, centre.y + 10.0, centre.z - 10.0),
            Vec3::new(centre.x - 10.0, centre.y + 10.0, centre.z + 10.0),
            Vec3::new(centre.x + 10.0, centre.y - 10.0, centre.z - 10.0),
            Vec3::new(centre.x + 10.0, centre.y - 10.0, centre.z + 10.0),
            Vec3::new(centre.x + 10.0, centre.y + 10.0, centre.z - 10.0),
            Vec3::new(centre.x + 10.0, centre.y + 10.0, centre.z + 10.0),
        ];
        assert!(c.selection_box_in_view(centre, corners));
        c.viewport = (3840.0, 2160.0);
        assert!(!c.selection_box_in_view(centre, corners));
    }

    #[test]
    fn appearance_class_cones_reject_before_the_projected_box() {
        // Structures use their radius-padded horizontal/vertical cone before they project the
        // type box (bdactor.cpp:1088-1118); a vehicle or mech instead rejects its centre against
        // the look-vector cone (gvactor.cpp:1585-1605, mech3d.cpp:2089-2110).
        let c = cam();
        let eye = c.position_world();
        let forward = (c.target - eye).normalized();
        let right = stuff_to_world().transform_vector(c.axes_stuff().0);
        let centre = eye + forward * 1200.0 + right * 1000.0;
        let corners = [
            Vec3::new(centre.x - 1000.0, centre.y - 1000.0, centre.z - 1000.0),
            Vec3::new(centre.x - 1000.0, centre.y - 1000.0, centre.z + 1000.0),
            Vec3::new(centre.x - 1000.0, centre.y + 1000.0, centre.z - 1000.0),
            Vec3::new(centre.x - 1000.0, centre.y + 1000.0, centre.z + 1000.0),
            Vec3::new(centre.x + 1000.0, centre.y - 1000.0, centre.z - 1000.0),
            Vec3::new(centre.x + 1000.0, centre.y - 1000.0, centre.z + 1000.0),
            Vec3::new(centre.x + 1000.0, centre.y + 1000.0, centre.z - 1000.0),
            Vec3::new(centre.x + 1000.0, centre.y + 1000.0, centre.z + 1000.0),
        ];
        assert!(c.selection_box_in_view(centre, corners));
        assert!(!c.structure_sphere_in_view(centre, 10.0));
        assert!(!c.mover_centre_in_view(centre));
        assert!(c.structure_sphere_in_view(c.target, 10.0));
        assert!(c.mover_centre_in_view(c.target));
    }

    #[test]
    fn mover_cone_uses_the_controller_value_after_a_normal_zoom() {
        // Camera::ZoomNormal resets cosHalfFOV to cos(40 degrees), rather than the init-time
        // cos(40 * 0.7071) value (camera.cpp:2525-2529).  Mech3DAppearance and GVAppearance
        // use that stored value at mech3d.cpp:2089-2110 / gvactor.cpp:1585-1605.
        let mut c = cam();
        let eye = c.position_world();
        let look = (c.target - eye).normalized();
        let right = stuff_to_world().transform_vector(c.axes_stuff().0);
        let angle = 30.0_f32.to_radians();
        let at_thirty = eye + (look * angle.cos() + right * angle.sin()) * 1000.0;
        assert!(!c.mover_centre_in_view(at_thirty));
        c.set_mover_visibility(40.0_f32.to_radians().cos(), c.scale_factor());
        assert!(c.mover_centre_in_view(at_thirty));
    }

    #[test]
    fn rough_screen_gate_precedes_the_projected_selection_box() {
        // The cheap rectangle rejects before the expensive type-box projection
        // (mech3d.cpp:2125-2145; the same ordering is in bdactor.cpp:1163-1182 and
        // gvactor.cpp:1621-1641).  A huge final box must not revive an offscreen centre that
        // fails this first gate.
        let c = cam();
        let right = stuff_to_world().transform_vector(c.axes_stuff().0);
        let centre = c.target + right * (c.altitude * c.tan_half_fov() * 1.1);
        let corners = [
            centre + Vec3::new(-1000.0, -1000.0, -1000.0),
            centre + Vec3::new(-1000.0, -1000.0, 1000.0),
            centre + Vec3::new(-1000.0, 1000.0, -1000.0),
            centre + Vec3::new(-1000.0, 1000.0, 1000.0),
            centre + Vec3::new(1000.0, -1000.0, -1000.0),
            centre + Vec3::new(1000.0, -1000.0, 1000.0),
            centre + Vec3::new(1000.0, 1000.0, -1000.0),
            centre + Vec3::new(1000.0, 1000.0, 1000.0),
        ];
        assert!(c.selection_box_in_view(centre, corners));
        assert!(!c.rough_screen_bounds_in_view(centre, (0.0, 0.0), (0.0, 0.0)));
    }
}
