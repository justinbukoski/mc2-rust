// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mapping between window coordinates and the logical game screen.
//!
//! The game renders to a fixed logical resolution (800x600, 1024x768, ...) that is independent of
//! the window size and of the display's pixel density. Mouse input must therefore be converted
//! from the window to the game screen in one consistent unit.
//!
//! **The rule (lesson from the earlier C++ attempt):** winit reports pointer positions in
//! *physical* pixels. They are converted to *logical points* by dividing by the window's live
//! scale factor, and the window's logical size is `inner_size / scale_factor` using the same scale
//! factor. Only logical points ever meet the game resolution; physical or drawable pixel counts
//! are never compared with or divided into mouse coordinates here.

/// How the logical game screen is fitted into the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fit {
    /// Keep the game aspect ratio; centre the screen and leave bars (the default).
    #[default]
    Letterbox,
    /// Fill the whole window, changing the aspect ratio.
    Stretch,
}

/// An axis-aligned rectangle in window logical points (origin top-left, y down).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Convert a physical pixel measure to logical points at `scale_factor`.
pub fn physical_to_logical(physical: f64, scale_factor: f64) -> f64 {
    physical / scale_factor
}

/// Convert logical points to physical pixels at `scale_factor`.
pub fn logical_to_physical(logical: f64, scale_factor: f64) -> f64 {
    logical * scale_factor
}

/// Window <-> game screen coordinate mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    game_w: u32,
    game_h: u32,
    win_w: f64,
    win_h: f64,
    fit: Fit,
}

impl Viewport {
    /// `win_w`/`win_h` are the window's inner size in logical points.
    pub fn new(game_w: u32, game_h: u32, win_w: f64, win_h: f64, fit: Fit) -> Self {
        Self {
            game_w: game_w.max(1),
            game_h: game_h.max(1),
            win_w,
            win_h,
            fit,
        }
    }

    /// Build from the values winit reports: physical inner size and the live scale factor.
    pub fn from_physical(
        game_w: u32,
        game_h: u32,
        phys_w: u32,
        phys_h: u32,
        scale_factor: f64,
        fit: Fit,
    ) -> Self {
        Self::new(
            game_w,
            game_h,
            physical_to_logical(phys_w as f64, scale_factor),
            physical_to_logical(phys_h as f64, scale_factor),
            fit,
        )
    }

    pub fn game_size(&self) -> (u32, u32) {
        (self.game_w, self.game_h)
    }

    pub fn window_logical_size(&self) -> (f64, f64) {
        (self.win_w, self.win_h)
    }

    pub fn fit(&self) -> Fit {
        self.fit
    }

    pub fn set_game_size(&mut self, w: u32, h: u32) {
        self.game_w = w.max(1);
        self.game_h = h.max(1);
    }

    pub fn set_window_logical_size(&mut self, w: f64, h: f64) {
        self.win_w = w;
        self.win_h = h;
    }

    pub fn set_fit(&mut self, fit: Fit) {
        self.fit = fit;
    }

    /// The part of the window (in logical points) that shows the game screen.
    pub fn content_rect(&self) -> Rect {
        match self.fit {
            Fit::Stretch => Rect {
                x: 0.0,
                y: 0.0,
                w: self.win_w,
                h: self.win_h,
            },
            Fit::Letterbox => {
                let s = (self.win_w / self.game_w as f64).min(self.win_h / self.game_h as f64);
                let w = self.game_w as f64 * s;
                let h = self.game_h as f64 * s;
                Rect {
                    x: (self.win_w - w) * 0.5,
                    y: (self.win_h - h) * 0.5,
                    w,
                    h,
                }
            }
        }
    }

    /// The content rectangle in physical pixels (for the renderer's viewport), rounded to whole
    /// pixels.
    pub fn content_rect_physical(&self, scale_factor: f64) -> Rect {
        let r = self.content_rect();
        let x0 = logical_to_physical(r.x, scale_factor).round();
        let y0 = logical_to_physical(r.y, scale_factor).round();
        let x1 = logical_to_physical(r.x + r.w, scale_factor).round();
        let y1 = logical_to_physical(r.y + r.h, scale_factor).round();
        Rect {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
        }
    }

    /// Window logical point -> game screen coordinate, clamped to `[0, size - 1]`.
    pub fn window_to_game(&self, x: f64, y: f64) -> (f32, f32) {
        let r = self.content_rect();
        if r.w <= 0.0 || r.h <= 0.0 {
            return (0.0, 0.0);
        }
        let gx = (x - r.x) / r.w * self.game_w as f64;
        let gy = (y - r.y) / r.h * self.game_h as f64;
        (
            gx.clamp(0.0, (self.game_w - 1) as f64) as f32,
            gy.clamp(0.0, (self.game_h - 1) as f64) as f32,
        )
    }

    /// Like [`window_to_game`](Self::window_to_game) but the integer pixel the point falls on.
    pub fn window_to_game_pixel(&self, x: f64, y: f64) -> (i32, i32) {
        let (gx, gy) = self.window_to_game(x, y);
        (gx.floor() as i32, gy.floor() as i32)
    }

    /// Game coordinate -> window logical point (the exact inverse of `window_to_game` for
    /// unclamped values).
    pub fn game_to_window(&self, gx: f64, gy: f64) -> (f64, f64) {
        let r = self.content_rect();
        (
            r.x + gx / self.game_w as f64 * r.w,
            r.y + gy / self.game_h as f64 * r.h,
        )
    }

    /// Window logical point at the centre of game pixel `(px, py)`: warping the OS cursor there
    /// maps back to the same pixel regardless of rounding.
    pub fn game_pixel_center_to_window(&self, px: i32, py: i32) -> (f64, f64) {
        let px = px.clamp(0, self.game_w as i32 - 1);
        let py = py.clamp(0, self.game_h as i32 - 1);
        self.game_to_window(px as f64 + 0.5, py as f64 + 0.5)
    }

    /// Game coordinate as a fraction of the game screen (`x / width`), the unit
    /// `gos_GetMouseInfo` reports. The game multiplies it back by the viewport width
    /// (`userinput.h` `getMouseX`), so the last pixel `width - 1` stays inside the screen.
    pub fn game_to_normalized(&self, gx: f32, gy: f32) -> (f32, f32) {
        (gx / self.game_w as f32, gy / self.game_h as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn stretch_1x_exact_size() {
        let v = Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch);
        assert_eq!(v.window_to_game(0.0, 0.0), (0.0, 0.0));
        assert_eq!(v.window_to_game(400.0, 300.0), (400.0, 300.0));
        // The far edge clamps to size - 1.
        assert_eq!(v.window_to_game(800.0, 600.0), (799.0, 599.0));
        assert_eq!(v.window_to_game(10_000.0, -50.0), (799.0, 0.0));
    }

    #[test]
    fn retina_2x_uses_logical_points_not_pixels() {
        // 800x600 logical window on a 2x display: 1600x1200 physical pixels.
        let scale = 2.0;
        let v = Viewport::from_physical(800, 600, 1600, 1200, scale, Fit::Stretch);
        assert_eq!(v.window_logical_size(), (800.0, 600.0));
        // winit reports the pointer in physical pixels; convert with the same scale factor.
        let (lx, ly) = (
            physical_to_logical(800.0, scale),
            physical_to_logical(600.0, scale),
        );
        assert_eq!(v.window_to_game(lx, ly), (400.0, 300.0));
        // The window centre in physical pixels is (800, 600): the middle of the game screen.
        let (cx, cy) = v.window_to_game(physical_to_logical(800.0, scale), 150.0);
        assert_eq!((cx, cy), (400.0, 150.0));
    }

    #[test]
    fn game_larger_than_window_and_non_integer_scale() {
        // 1024x768 game in a 640x480 logical window at 1.5x scale (960x720 physical).
        let v = Viewport::from_physical(1024, 768, 960, 720, 1.5, Fit::Stretch);
        assert_eq!(v.window_logical_size(), (640.0, 480.0));
        let (gx, gy) = v.window_to_game(320.0, 240.0);
        assert!(close(gx as f64, 512.0) && close(gy as f64, 384.0));
    }

    #[test]
    fn resize_changes_mapping_live() {
        let mut v = Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch);
        assert_eq!(v.window_to_game(400.0, 300.0), (400.0, 300.0));
        v.set_window_logical_size(1600.0, 900.0);
        assert_eq!(v.window_to_game(800.0, 450.0), (400.0, 300.0));
        v.set_window_logical_size(400.0, 300.0);
        assert_eq!(v.window_to_game(200.0, 150.0), (400.0, 300.0));
    }

    #[test]
    fn letterbox_centres_and_clamps_in_bars() {
        // 4:3 game in a wide 1600x900 window: scale 1.5, 1200x900 content, 200pt bars each side.
        let v = Viewport::new(800, 600, 1600.0, 900.0, Fit::Letterbox);
        let r = v.content_rect();
        assert!(close(r.x, 200.0) && close(r.y, 0.0) && close(r.w, 1200.0) && close(r.h, 900.0));
        assert_eq!(v.window_to_game(800.0, 450.0), (400.0, 300.0));
        // In the left bar: clamps to game x = 0.
        assert_eq!(v.window_to_game(50.0, 450.0).0, 0.0);
        // In the right bar: clamps to game x = width - 1.
        assert_eq!(v.window_to_game(1590.0, 450.0).0, 799.0);
    }

    #[test]
    fn letterbox_tall_window_has_horizontal_bars() {
        let v = Viewport::new(800, 600, 400.0, 600.0, Fit::Letterbox);
        let r = v.content_rect();
        assert!(close(r.w, 400.0) && close(r.h, 300.0) && close(r.y, 150.0));
        assert_eq!(v.window_to_game(200.0, 300.0), (400.0, 300.0));
        assert_eq!(v.window_to_game(200.0, 10.0).1, 0.0);
        assert_eq!(v.window_to_game(200.0, 590.0).1, 599.0);
    }

    #[test]
    fn round_trip_game_window_game() {
        for &(gw, gh, ww, wh, fit) in &[
            (800u32, 600u32, 800.0, 600.0, Fit::Stretch),
            (1024, 768, 1366.0, 768.0, Fit::Letterbox),
            (640, 480, 1440.0, 900.0, Fit::Letterbox),
            (1600, 1200, 777.0, 333.0, Fit::Stretch),
        ] {
            let v = Viewport::new(gw, gh, ww, wh, fit);
            for &(x, y) in &[
                (0.0, 0.0),
                (1.0, 1.0),
                (123.4, 56.7),
                (gw as f64 - 1.0, gh as f64 - 1.0),
            ] {
                let (wx, wy) = v.game_to_window(x, y);
                let (rx, ry) = v.window_to_game(wx, wy);
                assert!(
                    (rx as f64 - x).abs() < 1e-3 && (ry as f64 - y).abs() < 1e-3,
                    "{x},{y} -> {rx},{ry}"
                );
            }
        }
    }

    #[test]
    fn pixel_centre_round_trips_for_every_pixel() {
        for &(gw, gh, ww, wh, fit) in &[
            (800u32, 600u32, 1366.0, 768.0, Fit::Letterbox),
            (800, 600, 700.0, 500.0, Fit::Stretch),
            (1024, 768, 2560.0, 1440.0, Fit::Letterbox),
        ] {
            let v = Viewport::new(gw, gh, ww, wh, fit);
            for py in (0..gh as i32).step_by(7).chain([gh as i32 - 1]) {
                for px in (0..gw as i32).step_by(5).chain([gw as i32 - 1]) {
                    let (wx, wy) = v.game_pixel_center_to_window(px, py);
                    assert_eq!(v.window_to_game_pixel(wx, wy), (px, py));
                }
            }
        }
    }

    #[test]
    fn warp_round_trip_at_2x_scale() {
        let scale = 2.0;
        let v = Viewport::from_physical(1024, 768, 2560, 1440, scale, Fit::Letterbox);
        let (wx, wy) = v.game_pixel_center_to_window(300, 200);
        // The OS wants logical points for the warp; converting to physical and back is lossless.
        let (px, py) = (
            logical_to_physical(wx, scale),
            logical_to_physical(wy, scale),
        );
        let back = v.window_to_game_pixel(
            physical_to_logical(px, scale),
            physical_to_logical(py, scale),
        );
        assert_eq!(back, (300, 200));
    }

    #[test]
    fn content_rect_physical_is_whole_pixels() {
        let v = Viewport::new(800, 600, 1000.5, 700.25, Fit::Letterbox);
        let r = v.content_rect_physical(2.0);
        assert_eq!(r.x.fract(), 0.0);
        assert_eq!(r.w.fract(), 0.0);
        assert_eq!(r.h.fract(), 0.0);
    }

    #[test]
    fn degenerate_window_does_not_divide_by_zero() {
        let v = Viewport::new(800, 600, 0.0, 0.0, Fit::Letterbox);
        assert_eq!(v.window_to_game(10.0, 10.0), (0.0, 0.0));
        let v = Viewport::new(800, 600, 0.0, 0.0, Fit::Stretch);
        assert_eq!(v.window_to_game(10.0, 10.0), (0.0, 0.0));
    }

    #[test]
    fn normalized_matches_original_unit() {
        let v = Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch);
        assert_eq!(v.game_to_normalized(0.0, 0.0), (0.0, 0.0));
        // The game multiplies by the viewport size again: the edge pixel must stay on screen.
        let (nx, ny) = v.game_to_normalized(799.0, 599.0);
        assert_eq!((nx * 800.0).round(), 799.0);
        assert_eq!((ny * 600.0).round(), 599.0);
        assert!(nx < 1.0 && ny < 1.0);
        assert_eq!(v.game_to_normalized(400.0, 300.0), (0.5, 0.5));
    }
}
