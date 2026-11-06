// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Keyboard and mouse state, polled once per frame like the original `gos_GetKeyStatus` /
//! `gos_GetKey` / `gos_GetMouseInfo` interface.
//!
//! The window layer translates winit events into [`InputEvent`]s; tests can feed [`InputEvent`]s
//! directly, so everything here is exercised headless.

use crate::keys::{self, GameKey, KEY_SLOTS};
use crate::viewport::{Fit, Viewport};
use std::collections::VecDeque;

/// Pixel-scroll units that count as one wheel notch (touchpads report pixels).
pub const PIXELS_PER_WHEEL_NOTCH: f64 = 40.0;

/// Most buffered `gos_GetKey` entries kept; older ones are dropped.
const KEY_QUEUE_LIMIT: usize = 64;
/// Most buffered text characters kept.
const TEXT_QUEUE_LIMIT: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    X1,
    X2,
}

impl MouseButton {
    /// The game key index of this button (`KEY_LMOUSE` ...).
    pub fn game_key(self) -> GameKey {
        match self {
            MouseButton::Left => keys::KEY_LMOUSE,
            MouseButton::Middle => keys::KEY_MMOUSE,
            MouseButton::Right => keys::KEY_RMOUSE,
            MouseButton::X1 => keys::KEY_MOUSEX1,
            MouseButton::X2 => keys::KEY_MOUSEX2,
        }
    }

    /// Bit in [`InputState::mouse_button_mask`]: 0 = left, 1 = middle, 2 = right, 3/4 = X1/X2.
    pub fn mask_bit(self) -> u32 {
        match self {
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 4,
            MouseButton::X1 => 8,
            MouseButton::X2 => 16,
        }
    }
}

/// Status of a key this frame (`gosEnum_KeyStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyStatus {
    Free,
    /// Went down since the previous frame (also reported if it was released again within the frame).
    Pressed,
    Held,
    /// Went up since the previous frame.
    Released,
}

/// A platform-independent input event. Pointer positions are window **logical points**.
#[derive(Clone, Debug, PartialEq)]
pub enum InputEvent {
    Key {
        key: GameKey,
        /// Id of the physical key (see [`crate::keys::KeyMapping`]).
        phys_id: u16,
        pressed: bool,
        repeat: bool,
        /// Text produced by the key press, if any (used for ASCII and text entry).
        text: Option<String>,
    },
    /// Text committed outside of key presses (IME, paste).
    Text(String),
    CursorMoved {
        x: f64,
        y: f64,
    },
    CursorEntered,
    CursorLeft,
    Button {
        button: MouseButton,
        pressed: bool,
    },
    /// Wheel movement in notches; positive y is away from the user (scroll up).
    Wheel {
        x: f32,
        y: f32,
    },
    /// Window lost focus: all keys and buttons are released.
    FocusLost,
    /// Window logical size changed (resize or scale factor change).
    Resized {
        logical_w: f64,
        logical_h: f64,
    },
}

#[derive(Clone, Copy, Default)]
struct Slot {
    down: bool,
    pressed: bool,
    released: bool,
}

/// Per-frame input state.
pub struct InputState {
    viewport: Viewport,
    slots: [Slot; KEY_SLOTS],
    phys_down: [bool; KEY_SLOTS],
    phys_key: [u16; KEY_SLOTS],
    key_queue: VecDeque<u32>,
    text_queue: VecDeque<char>,
    /// Last pointer position in window logical points.
    window_pos: (f64, f64),
    mouse: (f32, f32),
    frame_start_mouse: (f32, f32),
    wheel: (f32, f32),
    inside: bool,
}

impl InputState {
    pub fn new(viewport: Viewport) -> Self {
        let mouse = viewport.window_to_game(
            viewport.window_logical_size().0 * 0.5,
            viewport.window_logical_size().1 * 0.5,
        );
        let window_pos = (
            viewport.window_logical_size().0 * 0.5,
            viewport.window_logical_size().1 * 0.5,
        );
        Self {
            viewport,
            slots: [Slot::default(); KEY_SLOTS],
            phys_down: [false; KEY_SLOTS],
            phys_key: [0; KEY_SLOTS],
            key_queue: VecDeque::new(),
            text_queue: VecDeque::new(),
            window_pos,
            mouse,
            frame_start_mouse: mouse,
            wheel: (0.0, 0.0),
            inside: false,
        }
    }

    pub fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Change the logical game resolution. The pointer is re-mapped from its window position.
    pub fn set_game_size(&mut self, w: u32, h: u32) {
        self.viewport.set_game_size(w, h);
        self.remap_pointer();
    }

    pub fn set_fit(&mut self, fit: Fit) {
        self.viewport.set_fit(fit);
        self.remap_pointer();
    }

    fn remap_pointer(&mut self) {
        self.mouse = self
            .viewport
            .window_to_game(self.window_pos.0, self.window_pos.1);
        self.frame_start_mouse = self.mouse;
    }

    /// Apply one event.
    pub fn handle(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key {
                key,
                phys_id,
                pressed,
                text,
                ..
            } => self.handle_key(key, phys_id, pressed, text),
            InputEvent::Text(s) => self.push_text(&s),
            InputEvent::CursorMoved { x, y } => {
                self.window_pos = (x, y);
                self.mouse = self.viewport.window_to_game(x, y);
                self.inside = true;
            }
            InputEvent::CursorEntered => self.inside = true,
            InputEvent::CursorLeft => self.inside = false,
            InputEvent::Button { button, pressed } => {
                self.set_slot(button.game_key(), pressed);
            }
            InputEvent::Wheel { x, y } => {
                self.wheel.0 += x;
                self.wheel.1 += y;
            }
            InputEvent::FocusLost => self.release_all(),
            InputEvent::Resized {
                logical_w,
                logical_h,
            } => {
                self.viewport.set_window_logical_size(logical_w, logical_h);
                self.remap_pointer();
            }
        }
    }

    fn handle_key(&mut self, key: GameKey, phys_id: u16, pressed: bool, text: Option<String>) {
        let id = phys_id as usize % KEY_SLOTS;
        let k = key as usize % KEY_SLOTS;
        if pressed {
            self.phys_down[id] = true;
            self.phys_key[id] = key as u16;
            if !self.slots[k].down {
                self.slots[k].down = true;
                self.slots[k].pressed = true;
            }
            // Queue the key (including auto-repeat) the way gos_GetKey delivers it: the key index
            // in bits 8.., the ASCII character (if any) in the low byte.
            let ascii = text
                .as_deref()
                .and_then(|t| {
                    let mut it = t.chars();
                    match (it.next(), it.next()) {
                        (Some(c), None) if c.is_ascii() => Some(c as u32),
                        _ => None,
                    }
                })
                .unwrap_or(0);
            if key != keys::KEY_LALT && key != keys::KEY_RALT {
                self.push_key((key << 8) | ascii);
            }
            if let Some(t) = text {
                self.push_text(&t);
            }
        } else {
            self.phys_down[id] = false;
            let still_held =
                (0..KEY_SLOTS).any(|i| self.phys_down[i] && self.phys_key[i] as usize == k);
            if !still_held && self.slots[k].down {
                self.slots[k].down = false;
                self.slots[k].released = true;
            }
        }
    }

    fn set_slot(&mut self, key: GameKey, down: bool) {
        let s = &mut self.slots[key as usize % KEY_SLOTS];
        if down && !s.down {
            s.down = true;
            s.pressed = true;
        } else if !down && s.down {
            s.down = false;
            s.released = true;
        }
    }

    fn release_all(&mut self) {
        for i in 0..KEY_SLOTS {
            if self.slots[i].down {
                self.slots[i].down = false;
                self.slots[i].released = true;
            }
            self.phys_down[i] = false;
        }
    }

    fn push_key(&mut self, v: u32) {
        if self.key_queue.len() >= KEY_QUEUE_LIMIT {
            self.key_queue.pop_front();
        }
        self.key_queue.push_back(v);
    }

    fn push_text(&mut self, s: &str) {
        for c in s.chars().filter(|c| !c.is_control()) {
            if self.text_queue.len() >= TEXT_QUEUE_LIMIT {
                self.text_queue.pop_front();
            }
            self.text_queue.push_back(c);
        }
    }

    /// End of frame: clears edge flags (pressed/released), the wheel and the mouse delta base.
    pub fn end_frame(&mut self) {
        for s in self.slots.iter_mut() {
            s.pressed = false;
            s.released = false;
        }
        self.wheel = (0.0, 0.0);
        self.frame_start_mouse = self.mouse;
    }

    // ---- keyboard -------------------------------------------------------------------------

    /// `gos_GetKeyStatus`. Mouse buttons are keys 1..=5.
    pub fn key_status(&self, key: GameKey) -> KeyStatus {
        let s = self.slots[key as usize % KEY_SLOTS];
        if s.pressed {
            KeyStatus::Pressed
        } else if s.released {
            KeyStatus::Released
        } else if s.down {
            KeyStatus::Held
        } else {
            KeyStatus::Free
        }
    }

    pub fn key_down(&self, key: GameKey) -> bool {
        self.slots[key as usize % KEY_SLOTS].down
    }

    pub fn key_pressed(&self, key: GameKey) -> bool {
        self.slots[key as usize % KEY_SLOTS].pressed
    }

    pub fn key_released(&self, key: GameKey) -> bool {
        self.slots[key as usize % KEY_SLOTS].released
    }

    /// `gos_GetKey`: next buffered key as `(key << 8) | ascii`, or 0 when none.
    pub fn pop_key(&mut self) -> u32 {
        self.key_queue.pop_front().unwrap_or(0)
    }

    /// `gos_KeyboardFlush`.
    pub fn flush_keys(&mut self) {
        self.key_queue.clear();
        self.text_queue.clear();
    }

    /// Next typed character for text entry fields.
    pub fn pop_text(&mut self) -> Option<char> {
        self.text_queue.pop_front()
    }

    // ---- mouse ----------------------------------------------------------------------------

    /// Pointer position in logical game coordinates, clamped to `[0, size - 1]`.
    pub fn mouse_position(&self) -> (f32, f32) {
        self.mouse
    }

    /// Pointer position as a fraction of the game screen (`0.0..=1.0`).
    pub fn mouse_position_normalized(&self) -> (f32, f32) {
        self.viewport.game_to_normalized(self.mouse.0, self.mouse.1)
    }

    /// Pointer movement since the start of this frame, in logical game pixels.
    pub fn mouse_delta(&self) -> (f32, f32) {
        (
            self.mouse.0 - self.frame_start_mouse.0,
            self.mouse.1 - self.frame_start_mouse.1,
        )
    }

    /// Wheel notches this frame; positive is scroll-up (away from the user).
    pub fn mouse_wheel(&self) -> f32 {
        self.wheel.1
    }

    pub fn mouse_wheel_horizontal(&self) -> f32 {
        self.wheel.0
    }

    pub fn mouse_button_down(&self, b: MouseButton) -> bool {
        self.key_down(b.game_key())
    }

    pub fn mouse_button_status(&self, b: MouseButton) -> KeyStatus {
        self.key_status(b.game_key())
    }

    /// Bit mask of held buttons (bit 0 left, 1 middle, 2 right, 3 X1, 4 X2), the layout the
    /// original game code reads.
    pub fn mouse_button_mask(&self) -> u32 {
        [
            MouseButton::Left,
            MouseButton::Middle,
            MouseButton::Right,
            MouseButton::X1,
            MouseButton::X2,
        ]
        .into_iter()
        .filter(|b| self.mouse_button_down(*b))
        .fold(0, |m, b| m | b.mask_bit())
    }

    pub fn cursor_inside_window(&self) -> bool {
        self.inside
    }

    /// Move the pointer to a game coordinate (cursor warp). Updates the state immediately without
    /// creating a delta and returns the window logical point the OS cursor must be moved to
    /// (centre of the target game pixel).
    pub fn warp_mouse(&mut self, game_x: i32, game_y: i32) -> (f64, f64) {
        let (wx, wy) = self.viewport.game_pixel_center_to_window(game_x, game_y);
        self.window_pos = (wx, wy);
        self.mouse = self.viewport.window_to_game(wx, wy);
        self.frame_start_mouse = self.mouse;
        (wx, wy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::*;

    fn state() -> InputState {
        InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch))
    }

    fn key(k: GameKey, id: u16, pressed: bool) -> InputEvent {
        InputEvent::Key {
            key: k,
            phys_id: id,
            pressed,
            repeat: false,
            text: None,
        }
    }

    #[test]
    fn key_press_lifecycle() {
        let mut s = state();
        assert_eq!(s.key_status(KEY_A), KeyStatus::Free);
        s.handle(key(KEY_A, 1, true));
        assert_eq!(s.key_status(KEY_A), KeyStatus::Pressed);
        s.end_frame();
        assert_eq!(s.key_status(KEY_A), KeyStatus::Held);
        s.end_frame();
        assert_eq!(s.key_status(KEY_A), KeyStatus::Held);
        s.handle(key(KEY_A, 1, false));
        assert_eq!(s.key_status(KEY_A), KeyStatus::Released);
        s.end_frame();
        assert_eq!(s.key_status(KEY_A), KeyStatus::Free);
    }

    #[test]
    fn press_and_release_within_one_frame_is_not_lost() {
        let mut s = state();
        s.handle(key(KEY_B, 2, true));
        s.handle(key(KEY_B, 2, false));
        assert_eq!(s.key_status(KEY_B), KeyStatus::Pressed);
        assert!(s.key_released(KEY_B));
        assert!(!s.key_down(KEY_B));
        s.end_frame();
        assert_eq!(s.key_status(KEY_B), KeyStatus::Free);
    }

    #[test]
    fn two_physical_keys_one_game_key() {
        let mut s = state();
        s.handle(key(KEY_LSHIFT, 10, true));
        s.handle(key(KEY_LSHIFT, 11, true));
        s.end_frame();
        s.handle(key(KEY_LSHIFT, 10, false));
        // The other shift is still down.
        assert!(s.key_down(KEY_LSHIFT));
        assert!(!s.key_released(KEY_LSHIFT));
        s.handle(key(KEY_LSHIFT, 11, false));
        assert!(!s.key_down(KEY_LSHIFT));
        assert!(s.key_released(KEY_LSHIFT));
    }

    #[test]
    fn gos_get_key_encodes_key_and_ascii() {
        let mut s = state();
        s.handle(InputEvent::Key {
            key: KEY_A,
            phys_id: 3,
            pressed: true,
            repeat: false,
            text: Some("a".into()),
        });
        s.handle(key(KEY_LEFT, 4, true));
        assert_eq!(s.pop_key(), (KEY_A << 8) | b'a' as u32);
        assert_eq!(s.pop_key(), KEY_LEFT << 8);
        assert_eq!(s.pop_key(), 0);
        assert_eq!(s.pop_text(), Some('a'));
        assert_eq!(s.pop_text(), None);
    }

    #[test]
    fn non_ascii_text_has_zero_ascii_byte_but_reaches_text_queue() {
        let mut s = state();
        s.handle(InputEvent::Key {
            key: KEY_A,
            phys_id: 3,
            pressed: true,
            repeat: false,
            text: Some("é".into()),
        });
        assert_eq!(s.pop_key(), KEY_A << 8);
        assert_eq!(s.pop_text(), Some('é'));
    }

    #[test]
    fn key_queue_is_bounded_and_flushable() {
        let mut s = state();
        for _ in 0..200 {
            s.handle(key(KEY_A, 1, true));
            s.handle(key(KEY_A, 1, false));
        }
        let mut n = 0;
        while s.pop_key() != 0 {
            n += 1;
        }
        assert_eq!(n, KEY_QUEUE_LIMIT);
        s.handle(key(KEY_B, 2, true));
        s.flush_keys();
        assert_eq!(s.pop_key(), 0);
    }

    #[test]
    fn side_specific_alt_not_queued() {
        let mut s = state();
        s.handle(key(KEY_LALT, 128, true));
        assert_eq!(s.pop_key(), 0);
        assert!(s.key_down(KEY_LALT));
    }

    #[test]
    fn mouse_position_follows_logical_window_points() {
        let mut s = InputState::new(Viewport::new(1024, 768, 512.0, 384.0, Fit::Stretch));
        s.handle(InputEvent::CursorMoved { x: 256.0, y: 192.0 });
        assert_eq!(s.mouse_position(), (512.0, 384.0));
        assert!(s.cursor_inside_window());
        s.handle(InputEvent::CursorMoved { x: -5.0, y: 9999.0 });
        assert_eq!(s.mouse_position(), (0.0, 767.0));
        s.handle(InputEvent::CursorLeft);
        assert!(!s.cursor_inside_window());
        // Position is retained when the cursor leaves.
        assert_eq!(s.mouse_position(), (0.0, 767.0));
    }

    #[test]
    fn hidpi_pipeline_physical_to_game() {
        use crate::viewport::{physical_to_logical, Viewport};
        // 2x display, 1600x1200 physical = 800x600 logical, game 1024x768.
        let scale = 2.0;
        let mut s = InputState::new(Viewport::from_physical(
            1024,
            768,
            1600,
            1200,
            scale,
            Fit::Stretch,
        ));
        // Pointer in the middle of the window, reported in physical pixels.
        s.handle(InputEvent::CursorMoved {
            x: physical_to_logical(800.0, scale),
            y: physical_to_logical(600.0, scale),
        });
        assert_eq!(s.mouse_position(), (512.0, 384.0));
        let n = s.mouse_position_normalized();
        assert!((n.0 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resize_remaps_pointer_without_new_cursor_event() {
        let mut s = state();
        s.handle(InputEvent::CursorMoved { x: 400.0, y: 300.0 });
        assert_eq!(s.mouse_position(), (400.0, 300.0));
        s.handle(InputEvent::Resized {
            logical_w: 1600.0,
            logical_h: 1200.0,
        });
        assert_eq!(s.mouse_position(), (200.0, 150.0));
    }

    #[test]
    fn mouse_delta_resets_each_frame() {
        let mut s = state();
        s.handle(InputEvent::CursorMoved { x: 100.0, y: 100.0 });
        s.end_frame();
        s.handle(InputEvent::CursorMoved { x: 130.0, y: 90.0 });
        assert_eq!(s.mouse_delta(), (30.0, -10.0));
        s.end_frame();
        assert_eq!(s.mouse_delta(), (0.0, 0.0));
    }

    #[test]
    fn warp_sets_position_without_delta_and_returns_window_point() {
        let mut s = InputState::new(Viewport::new(800, 600, 1600.0, 1200.0, Fit::Stretch));
        s.handle(InputEvent::CursorMoved { x: 10.0, y: 10.0 });
        s.end_frame();
        let (wx, wy) = s.warp_mouse(400, 300);
        assert_eq!((wx, wy), (801.0, 601.0));
        assert_eq!(s.mouse_position(), (400.5, 300.5));
        assert_eq!(s.mouse_delta(), (0.0, 0.0));
        // The OS then reports the cursor at the warped point: still pixel (400, 300).
        s.handle(InputEvent::CursorMoved { x: wx, y: wy });
        assert_eq!(s.mouse_position().0.floor(), 400.0);
    }

    #[test]
    fn buttons_wheel_and_mask() {
        let mut s = state();
        s.handle(InputEvent::Button {
            button: MouseButton::Left,
            pressed: true,
        });
        s.handle(InputEvent::Button {
            button: MouseButton::Right,
            pressed: true,
        });
        assert_eq!(s.key_status(KEY_LMOUSE), KeyStatus::Pressed);
        assert_eq!(s.mouse_button_mask(), 1 | 4);
        s.handle(InputEvent::Wheel { x: 0.0, y: 1.0 });
        s.handle(InputEvent::Wheel { x: 0.0, y: 2.0 });
        assert_eq!(s.mouse_wheel(), 3.0);
        s.end_frame();
        assert_eq!(s.mouse_wheel(), 0.0);
        assert_eq!(s.mouse_button_status(MouseButton::Left), KeyStatus::Held);
        s.handle(InputEvent::Button {
            button: MouseButton::Left,
            pressed: false,
        });
        assert_eq!(
            s.mouse_button_status(MouseButton::Left),
            KeyStatus::Released
        );
        assert_eq!(s.mouse_button_mask(), 4);
    }

    #[test]
    fn focus_loss_releases_everything() {
        let mut s = state();
        s.handle(key(KEY_W, 5, true));
        s.handle(InputEvent::Button {
            button: MouseButton::Middle,
            pressed: true,
        });
        s.end_frame();
        s.handle(InputEvent::FocusLost);
        assert!(!s.key_down(KEY_W));
        assert!(s.key_released(KEY_W));
        assert!(s.key_released(KEY_MMOUSE));
        // The stale physical key must not keep a later press "held".
        s.end_frame();
        s.handle(key(KEY_W, 5, true));
        s.handle(key(KEY_W, 5, false));
        assert!(!s.key_down(KEY_W));
    }

    #[test]
    fn changing_game_size_remaps_pointer() {
        let mut s = state();
        s.handle(InputEvent::CursorMoved { x: 400.0, y: 300.0 });
        s.set_game_size(1024, 768);
        assert_eq!(s.mouse_position(), (512.0, 384.0));
    }
}
