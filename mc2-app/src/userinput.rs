// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The per-frame mouse interpretation of the game: clicks, double clicks, drags, deltas and the
//! wheel, as the mission GUI and the camera read them.
//!
//! Ported from `mclib/userinput.cpp` (`UserInput::update`) and the accessors in
//! `mclib/userinput.h` (`getKeyDown`, `shift/ctrl/alt`, `getMouseX/Y`, `isLeftDrag` ...).  The
//! cursor-animation half of `UserInput::update` belongs to the GUI (`mc2-ui`).  Input comes from
//! [`mc2_platform::input::InputState`], which is built from `InputEvent`s only.

use mc2_platform::input::{InputState, KeyStatus, MouseButton};
use mc2_platform::keys::{self, GameKey};

/// Mouse and keyboard state of one game frame.
#[derive(Clone, Debug)]
pub struct UserInput {
    /// Pointer position as a fraction of the game screen (`mouseXPosition`).
    mouse_x: f32,
    /// The physical pointer when `setMousePos` last warped the game's pointer: the warp holds until the physical pointer moves.
    warp_from: Option<(f32, f32)>,
    mouse_y: f32,
    last_mouse_x: f32,
    last_mouse_y: f32,
    mouse_x_delta: i32,
    mouse_y_delta: i32,
    mouse_wheel_delta: i32,
    left_down: bool,
    right_down: bool,
    middle_down: bool,
    last_left_down: bool,
    last_right_down: bool,
    left_click: bool,
    right_click: bool,
    middle_click: bool,
    left_double_click: bool,
    right_double_click: bool,
    middle_double_click: bool,
    left_drag: bool,
    right_drag: bool,
    was_left_drag: bool,
    was_right_drag: bool,
    left_just_up: bool,
    right_just_up: bool,
    left_up_time: f32,
    right_up_time: f32,
    middle_up_time: f32,
    left_held_time: f32,
    right_held_time: f32,
    drag_x: f32,
    drag_y: f32,
    /// Normalised distance the pointer must move before a held button drags.
    drag_threshold: f32,
    double_click_threshold: f32,
    /// Game screen size (`viewMulX/Y`).
    view: (f32, f32),
}

impl Default for UserInput {
    fn default() -> Self {
        Self::new()
    }
}

impl UserInput {
    pub fn new() -> Self {
        Self {
            mouse_x: 0.0,
            warp_from: None,
            mouse_y: 0.0,
            last_mouse_x: 0.0,
            last_mouse_y: 0.0,
            mouse_x_delta: 0,
            mouse_y_delta: 0,
            mouse_wheel_delta: 0,
            left_down: false,
            right_down: false,
            middle_down: false,
            last_left_down: false,
            last_right_down: false,
            left_click: false,
            right_click: false,
            middle_click: false,
            left_double_click: false,
            right_double_click: false,
            middle_double_click: false,
            left_drag: false,
            right_drag: false,
            was_left_drag: false,
            was_right_drag: false,
            left_just_up: false,
            right_just_up: false,
            left_up_time: 0.0,
            right_up_time: 0.0,
            middle_up_time: 0.0,
            left_held_time: 0.0,
            right_held_time: 0.0,
            drag_x: 0.0,
            drag_y: 0.0,
            // UserInput constructor values
            drag_threshold: 0.016_666_7,
            double_click_threshold: 0.2,
            view: (800.0, 600.0),
        }
    }

    /// `setMouseDragThreshold` (normalised units; the mission GUI passes pixels / screen height).
    pub fn set_drag_threshold(&mut self, normalized: f32) {
        self.drag_threshold = normalized;
    }

    /// `setMouseDoubleClickThreshold` (seconds).
    pub fn set_double_click_threshold(&mut self, seconds: f32) {
        self.double_click_threshold = seconds;
    }

    /// `UserInput::update`: interpret this frame's input.  `game_size` is the logical game screen
    /// (`viewMulX/Y`), `frame_length` the game frame length in seconds.
    pub fn update(&mut self, input: &InputState, game_size: (f32, f32), frame_length: f32) {
        self.view = (game_size.0.max(1.0), game_size.1.max(1.0));
        self.last_left_down = self.left_down;
        self.last_right_down = self.right_down;
        self.last_mouse_x = self.mouse_x;
        self.last_mouse_y = self.mouse_y;
        self.left_just_up = false;
        self.right_just_up = false;
        let was_double = self.left_double_click;
        self.left_click = false;
        self.right_click = false;
        self.middle_click = false;
        self.left_double_click = false;
        self.right_double_click = false;
        self.middle_double_click = false;

        // gos_GetMouseInfo
        let (gx, gy) = input.mouse_position();
        if self.warp_from.is_some_and(|w| w != (gx, gy)) {
            self.warp_from = None;
        }
        if self.warp_from.is_none() {
            self.mouse_x = gx / self.view.0;
            self.mouse_y = gy / self.view.1;
            let (dx, dy) = input.mouse_delta();
            self.mouse_x_delta = dx.round() as i32;
            self.mouse_y_delta = dy.round() as i32;
        } else {
            self.mouse_x_delta = 0;
            self.mouse_y_delta = 0;
        }
        // gos reports the wheel positive away from the user; the game flips it ("old broken
        // Win2K values": mouseWheelDelta *= -100), so scrolling away from the user is negative.
        let notches = input.mouse_wheel();
        self.mouse_wheel_delta = if notches.abs() < 1e-3 {
            0
        } else {
            let v = -(notches * 100.0).round() as i32;
            if v == 0 {
                -(notches.signum() as i32)
            } else {
                v
            }
        };
        self.left_down = input.mouse_button_down(MouseButton::Left)
            || input.mouse_button_status(MouseButton::Left) == KeyStatus::Pressed;
        self.right_down = input.mouse_button_down(MouseButton::Right)
            || input.mouse_button_status(MouseButton::Right) == KeyStatus::Pressed;
        self.middle_down = input.mouse_button_down(MouseButton::Middle);

        let left_status = input.key_status(keys::KEY_LMOUSE);
        let right_status = input.key_status(keys::KEY_RMOUSE);
        let middle_status = input.key_status(keys::KEY_MMOUSE);

        // ---- left button
        if !self.left_down && self.last_left_down {
            // just lifted: drags are off, the double click clock starts
            self.was_left_drag = self.left_drag;
            self.was_right_drag = self.right_drag;
            self.left_drag = false;
            self.left_up_time += frame_length;
            self.left_just_up = true;
            self.left_held_time = 0.0;
        }
        if !self.left_down && !self.last_left_down {
            self.left_up_time += frame_length;
            self.left_held_time = 0.0;
        }
        if left_status == KeyStatus::Pressed {
            if self.left_up_time > 0.0
                && self.left_up_time < self.double_click_threshold
                && !was_double
            {
                self.left_double_click = true;
                self.left_up_time = 0.0;
            } else {
                self.left_click = true;
                self.left_up_time = 0.001;
            }
            self.drag_x = self.last_mouse_x;
            self.drag_y = self.last_mouse_y;
            self.left_drag = false;
        } else if left_status == KeyStatus::Held {
            if !self.left_drag && self.beyond_drag_threshold() {
                self.left_drag = true;
            }
            self.left_up_time = 0.0;
            self.left_held_time += frame_length;
        }

        // ---- right button
        if !self.right_down && self.last_right_down {
            self.was_right_drag = self.right_drag;
            self.right_drag = false;
            self.right_up_time = 0.001;
            self.right_just_up = true;
            self.right_held_time = 0.0;
        }
        if !self.right_down && !self.last_right_down {
            self.right_up_time += frame_length;
            self.right_drag = false;
            self.right_held_time = 0.0;
        }
        if right_status == KeyStatus::Pressed {
            if self.right_up_time > 0.0 && self.right_up_time < self.double_click_threshold {
                self.right_double_click = true;
            } else {
                self.right_click = true;
            }
            self.drag_x = self.last_mouse_x;
            self.drag_y = self.last_mouse_y;
            self.right_drag = false;
        } else if right_status == KeyStatus::Held {
            if !self.right_drag && self.beyond_drag_threshold() {
                self.right_drag = true;
            }
            self.right_click = true;
            self.right_up_time = 0.0;
            self.right_held_time += frame_length;
        }

        // ---- middle button.  `lastMiddleMouseButtonState` is never saved in the original, so it
        // always reads "up": the up time only grows while the button is up and is never reset, and
        // after the first double-click threshold every press is a plain middle click.
        if !self.middle_down {
            self.middle_up_time += frame_length;
        }
        if middle_status == KeyStatus::Pressed {
            if self.middle_up_time < self.double_click_threshold {
                self.middle_double_click = true;
            } else {
                self.middle_click = true;
            }
        }
    }

    fn beyond_drag_threshold(&self) -> bool {
        (self.drag_x - self.mouse_x).abs() > self.drag_threshold
            || (self.drag_y - self.mouse_y).abs() > self.drag_threshold
    }

    /// `getMouseX`: game pixels, truncated (`float2long`).
    pub fn mouse_x(&self) -> i32 {
        (self.mouse_x * self.view.0) as i32
    }
    pub fn mouse_y(&self) -> i32 {
        (self.mouse_y * self.view.1) as i32
    }
    /// `UserInput::setMousePos` (game pixels): the game's pointer sits at `pos` and stays there, for hit testing too, until the
    /// physical pointer moves (the platform cannot warp the OS pointer, so the warp is virtual). `physical` is the OS
    /// pointer of this frame (`InputState::mouse_position`).
    pub fn warp_mouse(&mut self, pos: (f32, f32), physical: (f32, f32)) {
        self.mouse_x = pos.0 / self.view.0;
        self.mouse_y = pos.1 / self.view.1;
        self.warp_from = Some(physical);
    }
    /// `realMouseX/Y`.
    pub fn real_mouse(&self) -> (f32, f32) {
        (self.mouse_x * self.view.0, self.mouse_y * self.view.1)
    }
    pub fn mouse_x_delta(&self) -> i32 {
        self.mouse_x_delta
    }
    pub fn mouse_y_delta(&self) -> i32 {
        self.mouse_y_delta
    }
    pub fn mouse_wheel_delta(&self) -> i32 {
        self.mouse_wheel_delta
    }
    pub fn is_left_click(&self) -> bool {
        self.left_click
    }
    pub fn is_right_click(&self) -> bool {
        self.right_click
    }
    pub fn is_middle_click(&self) -> bool {
        self.middle_click
    }
    pub fn is_left_double_click(&self) -> bool {
        self.left_double_click
    }
    pub fn is_right_double_click(&self) -> bool {
        self.right_double_click
    }
    pub fn is_middle_double_click(&self) -> bool {
        self.middle_double_click
    }
    pub fn is_left_drag(&self) -> bool {
        self.left_drag
    }
    pub fn is_right_drag(&self) -> bool {
        self.right_drag
    }
    pub fn was_left_drag(&self) -> bool {
        self.was_left_drag
    }
    pub fn was_right_drag(&self) -> bool {
        self.was_right_drag
    }
    pub fn left_mouse_released(&self) -> bool {
        self.left_just_up
    }
    pub fn right_mouse_released(&self) -> bool {
        self.right_just_up
    }
    /// Start of the current drag, game pixels.
    pub fn drag_start(&self) -> (f32, f32) {
        (self.drag_x * self.view.0, self.drag_y * self.view.1)
    }
    pub fn left_held_time(&self) -> f32 {
        self.left_held_time
    }

    /// `getMouseLeftButtonState() == MC2_MOUSE_DOWN` (`mclib/userinput.cpp:189`).
    pub fn left_button_down(&self) -> bool {
        self.left_down
    }
    pub fn right_held_time(&self) -> f32 {
        self.right_held_time
    }
}

/// `UserInput::getKeyDown`: the key is held or went down this frame.
pub fn key_down(input: &InputState, key: GameKey) -> bool {
    matches!(input.key_status(key), KeyStatus::Held | KeyStatus::Pressed)
}

/// `UserInput::shift()`.
pub fn shift(input: &InputState) -> bool {
    key_down(input, keys::KEY_LSHIFT) || key_down(input, keys::KEY_RSHIFT)
}

/// `UserInput::ctrl()`.
pub fn ctrl(input: &InputState) -> bool {
    key_down(input, keys::KEY_LCONTROL) || key_down(input, keys::KEY_RCONTROL)
}

/// `UserInput::alt()` (the dedicated Alt indices, not `KEY_LMENU`).
pub fn alt(input: &InputState) -> bool {
    key_down(input, keys::KEY_LALT) || key_down(input, keys::KEY_RALT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_platform::input::InputEvent;
    use mc2_platform::viewport::{Fit, Viewport};

    fn state() -> InputState {
        InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch))
    }

    fn frame(ui: &mut UserInput, input: &mut InputState) {
        ui.update(input, (800.0, 600.0), 1.0 / 30.0);
        input.end_frame();
    }

    fn button(b: MouseButton, pressed: bool) -> InputEvent {
        InputEvent::Button { button: b, pressed }
    }

    #[test]
    fn a_warped_pointer_holds_for_clicks_until_the_physical_mouse_moves() {
        let mut ui = UserInput::new();
        let mut input = state();
        input.handle(InputEvent::CursorMoved { x: 100.0, y: 100.0 });
        for _ in 0..10 {
            frame(&mut ui, &mut input);
        }
        ui.warp_mouse((400.0, 300.0), input.mouse_position());
        assert_eq!(ui.real_mouse(), (400.0, 300.0));
        // no physical movement: the click lands on the warp target
        input.handle(button(MouseButton::Left, true));
        frame(&mut ui, &mut input);
        assert!(ui.is_left_click());
        assert_eq!(ui.real_mouse(), (400.0, 300.0));
        // the physical mouse moves: the game's pointer follows it again
        input.handle(InputEvent::CursorMoved { x: 120.0, y: 100.0 });
        frame(&mut ui, &mut input);
        assert!(
            (ui.real_mouse().0 - 120.0).abs() < 1e-3 && (ui.real_mouse().1 - 100.0).abs() < 1e-3
        );
    }

    #[test]
    fn right_drag_starts_beyond_the_threshold() {
        let mut ui = UserInput::new();
        ui.set_drag_threshold(10.0 / 600.0);
        let mut input = state();
        input.handle(InputEvent::CursorMoved { x: 400.0, y: 300.0 });
        // past the double-click window first (the up clock starts at 0)
        for _ in 0..10 {
            frame(&mut ui, &mut input);
        }
        input.handle(button(MouseButton::Right, true));
        frame(&mut ui, &mut input);
        assert!(ui.is_right_click() && !ui.is_right_drag());
        // 8 px down: not yet a drag (the threshold is 10 / 600 of the screen, i.e. 10 px
        // vertically and 13.3 px horizontally)
        input.handle(InputEvent::CursorMoved { x: 400.0, y: 308.0 });
        frame(&mut ui, &mut input);
        assert!(!ui.is_right_drag());
        input.handle(InputEvent::CursorMoved { x: 400.0, y: 312.0 });
        frame(&mut ui, &mut input);
        assert!(ui.is_right_drag());
        assert_eq!(ui.mouse_y_delta(), 4);
        input.handle(button(MouseButton::Right, false));
        frame(&mut ui, &mut input);
        assert!(!ui.is_right_drag() && ui.was_right_drag() && ui.right_mouse_released());
    }

    #[test]
    fn left_double_click_within_threshold() {
        let mut ui = UserInput::new();
        let mut input = state();
        // let the up clock pass the threshold first
        for _ in 0..10 {
            frame(&mut ui, &mut input);
        }
        input.handle(button(MouseButton::Left, true));
        frame(&mut ui, &mut input);
        assert!(ui.is_left_click());
        input.handle(button(MouseButton::Left, false));
        frame(&mut ui, &mut input);
        assert!(ui.left_mouse_released());
        input.handle(button(MouseButton::Left, true));
        frame(&mut ui, &mut input);
        assert!(ui.is_left_double_click() && !ui.is_left_click());
    }

    #[test]
    fn wheel_away_from_user_is_negative() {
        let mut ui = UserInput::new();
        let mut input = state();
        input.handle(InputEvent::Wheel { x: 0.0, y: 1.0 });
        frame(&mut ui, &mut input);
        assert_eq!(ui.mouse_wheel_delta(), -100);
        frame(&mut ui, &mut input);
        assert_eq!(ui.mouse_wheel_delta(), 0);
        input.handle(InputEvent::Wheel { x: 0.0, y: -0.5 });
        frame(&mut ui, &mut input);
        assert_eq!(ui.mouse_wheel_delta(), 50);
    }

    #[test]
    fn middle_click_after_the_first_threshold() {
        let mut ui = UserInput::new();
        let mut input = state();
        input.handle(button(MouseButton::Middle, true));
        frame(&mut ui, &mut input);
        // first press inside the first 0.2 s counts as a double click (never-saved last state)
        assert!(ui.is_middle_double_click() && !ui.is_middle_click());
        input.handle(button(MouseButton::Middle, false));
        for _ in 0..10 {
            frame(&mut ui, &mut input);
        }
        input.handle(button(MouseButton::Middle, true));
        frame(&mut ui, &mut input);
        assert!(ui.is_middle_click());
    }
}
