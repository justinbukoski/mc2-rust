// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mouse and keyboard state as the GUI reads it: a port of `UserInput::update` and its getters
//! (`mclib/userinput.{h,cpp}`: click, double click, drag threshold, held time, release edge, drag
//! origin) on top of the platform's `gos_GetKeyStatus`/`gos_GetKey` state
//! ([`mc2_platform::input::InputState`]).
//!
//! Events arrive as [`InputEvent`]s whose pointer positions are already in UI (game) coordinates
//! (the app converts window points with its `InputState`). Per frame:
//! [`UiInput::handle`] for each event, [`UiInput::begin_frame`] (the `userInput->update()` at the top
//! of the original frame), the screen's update, then [`UiInput::end_frame`].

use mc2_platform::input::{InputEvent, InputState, KeyStatus};
use mc2_platform::keys::{self, GameKey};
use mc2_platform::viewport::{Fit, Viewport};

/// `mouseDragThreshold` (fraction of the screen) and `mouseDblClickThreshold` (seconds) as set in
/// `UserInput::init` / `mechcmd2.cpp`.
pub const DRAG_THRESHOLD: f32 = 0.016_666_7;
pub const DOUBLE_CLICK_THRESHOLD: f32 = 0.2;

#[derive(Clone, Copy, Default, Debug)]
struct Button {
    down: bool,
    last_down: bool,
    click: bool,
    double_click: bool,
    just_up: bool,
    drag: bool,
    was_drag: bool,
    up_time: f32,
    held_time: f32,
}

pub struct UiInput {
    gos: InputState,
    size: (u32, u32),
    pos: (f32, f32),
    last_pos: (f32, f32),
    left: Button,
    right: Button,
    middle: Button,
    drag: (f32, f32),
    pub drag_threshold: f32,
    pub double_click_threshold: f32,
    /// Pointer visibility (`mouseOn` / `mouseOff`).
    pub draw_mouse: bool,
    /// Current cursor (`setMouseCursor`, `mState_*`).
    pub mouse_state: u32,
    /// Where `set_mouse_pos` moved the pointer, until the next cursor move (the C++ warps the OS
    /// cursor with `gos_SetMousePosition`, so the hardware read starts from there).
    warp: Option<(f32, f32)>,
}

impl UiInput {
    /// Input for a UI of `w` x `h` logical pixels.
    pub fn new(w: u32, h: u32) -> UiInput {
        let gos = InputState::new(Viewport::new(
            w,
            h,
            f64::from(w),
            f64::from(h),
            Fit::Stretch,
        ));
        let pos = gos.mouse_position();
        UiInput {
            gos,
            size: (w, h),
            pos,
            last_pos: pos,
            left: Button::default(),
            right: Button::default(),
            middle: Button::default(),
            drag: (0.0, 0.0),
            drag_threshold: DRAG_THRESHOLD,
            double_click_threshold: DOUBLE_CLICK_THRESHOLD,
            draw_mouse: true,
            mouse_state: crate::cursor::M_STATE_NORMAL,
            warp: None,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Feeds one event (pointer positions in UI coordinates). Window resizes are the app's
    /// business and are ignored here.
    pub fn handle(&mut self, ev: &InputEvent) {
        if matches!(ev, InputEvent::Resized { .. }) {
            return;
        }
        if matches!(ev, InputEvent::CursorMoved { .. }) {
            self.warp = None;
        }
        self.gos.handle(ev.clone());
    }

    /// Feeds an event after routing: events the UI took always; of the rest pointer position,
    /// focus changes and every release. A press or a key the game took never reaches the widgets,
    /// so a game-owned gesture cannot click or focus them; releases always arrive so nothing
    /// stays held here when ownership changes (a release of something not down here makes no
    /// edge).
    pub fn handle_routed(&mut self, ev: &InputEvent, consumed: bool) {
        let always = matches!(
            ev,
            InputEvent::CursorMoved { .. }
                | InputEvent::CursorEntered
                | InputEvent::CursorLeft
                | InputEvent::FocusLost
                | InputEvent::Button { pressed: false, .. }
                | InputEvent::Key { pressed: false, .. }
        );
        if consumed || always {
            self.handle(ev);
        }
    }

    /// The pointer position of the events fed so far (before this frame's `begin_frame`).
    pub fn pending_pointer(&self) -> (i32, i32) {
        let (x, y) = self.gos.mouse_position();
        (x.round() as i32, y.round() as i32)
    }

    /// `UserInput::update` with `frameLength = dt`.
    pub fn begin_frame(&mut self, dt: f32) {
        self.last_pos = self.pos;
        self.left.last_down = self.left.down;
        self.right.last_down = self.right.down;
        self.middle.last_down = self.middle.down;
        self.left.just_up = false;
        self.right.just_up = false;
        let was_double = self.left.double_click;
        for b in [&mut self.left, &mut self.right, &mut self.middle] {
            b.click = false;
            b.double_click = false;
        }
        self.pos = self.real_mouse_pos();
        self.left.down = self.gos.key_down(keys::KEY_LMOUSE);
        self.right.down = self.gos.key_down(keys::KEY_RMOUSE);
        self.middle.down = self.gos.key_down(keys::KEY_MMOUSE);

        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let moved_past_threshold = |drag: (f32, f32), pos: (f32, f32), thr: f32| {
            ((drag.0 - pos.0) / w).abs() > thr || ((drag.1 - pos.1) / h).abs() > thr
        };

        // ---- left
        let l = &mut self.left;
        if !l.down && l.last_down {
            l.was_drag = l.drag;
            self.right.was_drag = self.right.drag;
            l.drag = false;
            l.up_time += dt;
            l.just_up = true;
            l.held_time = 0.0;
        }
        if !l.down && !l.last_down {
            l.up_time += dt;
            l.held_time = 0.0;
        }
        match self.gos.key_status(keys::KEY_LMOUSE) {
            KeyStatus::Pressed => {
                if l.up_time > 0.0 && l.up_time < self.double_click_threshold && !was_double {
                    l.double_click = true;
                    l.up_time = 0.0;
                } else {
                    l.click = true;
                    l.up_time = 0.001;
                }
                self.drag = self.last_pos;
                l.drag = false;
            }
            KeyStatus::Held => {
                if !l.drag && moved_past_threshold(self.drag, self.pos, self.drag_threshold) {
                    l.drag = true;
                }
                l.up_time = 0.0;
                l.held_time += dt;
            }
            _ => {}
        }

        // ---- right
        let r = &mut self.right;
        if !r.down && r.last_down {
            r.was_drag = r.drag;
            r.drag = false;
            r.up_time = 0.001;
            r.just_up = true;
            r.held_time = 0.0;
        }
        if !r.down && !r.last_down {
            r.up_time += dt;
            r.drag = false;
            r.held_time = 0.0;
        }
        match self.gos.key_status(keys::KEY_RMOUSE) {
            KeyStatus::Pressed => {
                if r.up_time != 0.0 && r.up_time < self.double_click_threshold {
                    r.double_click = true;
                } else {
                    r.click = true;
                }
                self.drag = self.last_pos;
                r.drag = false;
            }
            KeyStatus::Held => {
                if !r.drag && moved_past_threshold(self.drag, self.pos, self.drag_threshold) {
                    r.drag = true;
                }
                r.click = true;
                r.up_time = 0.0;
                r.held_time += dt;
            }
            _ => {}
        }

        // ---- middle
        let m = &mut self.middle;
        if !m.down && m.last_down {
            m.up_time = 0.0;
        }
        if !m.down && !m.last_down {
            m.up_time += dt;
        }
        if self.gos.key_status(keys::KEY_MMOUSE) == KeyStatus::Pressed {
            if m.up_time < self.double_click_threshold {
                m.double_click = true;
            } else {
                m.click = true;
            }
        }
    }

    /// Clears this frame's edges (pressed/released keys, wheel).
    pub fn end_frame(&mut self) {
        self.gos.end_frame();
    }

    // ---- getters (UserInput)

    /// `getMouseX` (rounded like `float2long`).
    pub fn mouse_x(&self) -> i32 {
        self.pos.0.round() as i32
    }
    pub fn mouse_y(&self) -> i32 {
        self.pos.1.round() as i32
    }
    pub fn mouse_pos_f(&self) -> (f32, f32) {
        self.pos
    }
    /// `UserInput::setMousePos` (the tutorial's callout): moves the pointer. The move holds until
    /// the next cursor event, as the OS cursor the C++ warps does.
    pub fn set_mouse_pos(&mut self, x: f32, y: f32) {
        self.warp = Some((x, y));
        self.pos = (x, y);
    }
    /// `realMouseX`, `realMouseY`: the pointer as the last `set_mouse_pos` left it, else the
    /// hardware pointer.
    pub fn real_mouse_pos(&self) -> (f32, f32) {
        self.warp.unwrap_or_else(|| self.gos.mouse_position())
    }
    /// Drag origin: where the pointer was when the last button went down (`getMouseDragX`).
    pub fn mouse_drag_x(&self) -> i32 {
        self.drag.0 as i32
    }
    pub fn mouse_drag_y(&self) -> i32 {
        self.drag.1 as i32
    }
    pub fn is_left_click(&self) -> bool {
        self.left.click
    }
    pub fn is_left_double_click(&self) -> bool {
        self.left.double_click
    }
    pub fn is_right_click(&self) -> bool {
        self.right.click
    }
    pub fn is_left_drag(&self) -> bool {
        self.left.drag
    }
    pub fn was_left_drag(&self) -> bool {
        self.left.was_drag
    }
    pub fn is_right_drag(&self) -> bool {
        self.right.drag
    }
    pub fn left_mouse_released(&self) -> bool {
        self.left.just_up
    }
    pub fn right_mouse_released(&self) -> bool {
        self.right.just_up
    }
    /// Seconds the left button has been held (`getMouseLeftHeld`).
    pub fn mouse_left_held(&self) -> f32 {
        self.left.held_time
    }
    pub fn mouse_right_held(&self) -> f32 {
        self.right.held_time
    }
    pub fn wheel(&self) -> f32 {
        self.gos.mouse_wheel()
    }

    /// `gos_GetKeyStatus`.
    pub fn key_status(&self, key: GameKey) -> KeyStatus {
        self.gos.key_status(key)
    }
    /// `getKeyDown`: held or pressed.
    pub fn key_down(&self, key: GameKey) -> bool {
        matches!(self.key_status(key), KeyStatus::Held | KeyStatus::Pressed)
    }
    pub fn shift(&self) -> bool {
        self.key_down(keys::KEY_LSHIFT)
    }
    pub fn ctrl(&self) -> bool {
        self.key_down(keys::KEY_LCONTROL)
    }
    pub fn alt(&self) -> bool {
        self.key_down(keys::KEY_LALT) || self.key_down(keys::KEY_RALT)
    }
    /// `gos_GetKey`: `(key << 8) | ascii`, 0 when the queue is empty.
    pub fn pop_key(&mut self) -> u32 {
        self.gos.pop_key()
    }
    /// `gos_KeyboardFlush`.
    pub fn flush_keys(&mut self) {
        self.gos.flush_keys();
    }
    /// Raw platform state.
    pub fn gos(&self) -> &InputState {
        &self.gos
    }
}

/// Builders for synthetic events (tests, scripted input).
pub mod events {
    use mc2_platform::input::{InputEvent, MouseButton};
    use mc2_platform::keys::GameKey;

    pub fn move_to(x: f32, y: f32) -> InputEvent {
        InputEvent::CursorMoved {
            x: f64::from(x),
            y: f64::from(y),
        }
    }
    pub fn left(pressed: bool) -> InputEvent {
        InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        }
    }
    pub fn key(key: GameKey, pressed: bool, text: Option<&str>) -> InputEvent {
        InputEvent::Key {
            key,
            phys_id: key as u16,
            pressed,
            repeat: false,
            text: text.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::events::*;
    use super::*;

    fn frame(i: &mut UiInput, evs: &[InputEvent]) {
        for e in evs {
            i.handle(e);
        }
        i.begin_frame(1.0 / 30.0);
    }

    #[test]
    fn click_release_and_drag_origin() {
        let mut i = UiInput::new(800, 600);
        // the double-click clock runs from start-up: idle past it first
        for _ in 0..10 {
            frame(&mut i, &[move_to(100.0, 100.0)]);
            i.end_frame();
        }
        frame(&mut i, &[left(true)]);
        assert!(i.is_left_click());
        assert_eq!((i.mouse_drag_x(), i.mouse_drag_y()), (100, 100));
        i.end_frame();
        frame(&mut i, &[move_to(105.0, 100.0)]);
        assert!(!i.is_left_click() && !i.is_left_drag());
        assert!(i.mouse_left_held() > 0.0);
        i.end_frame();
        // 0.0166667 * 800 = 13.3 pixels
        frame(&mut i, &[move_to(120.0, 100.0)]);
        assert!(i.is_left_drag());
        i.end_frame();
        frame(&mut i, &[left(false)]);
        assert!(i.left_mouse_released() && i.was_left_drag() && !i.is_left_drag());
        i.end_frame();
        frame(&mut i, &[]);
        assert!(!i.left_mouse_released());
    }

    #[test]
    fn quick_second_click_is_a_double_click_not_a_click() {
        let mut i = UiInput::new(800, 600);
        for _ in 0..10 {
            frame(&mut i, &[]);
            i.end_frame();
        }
        frame(&mut i, &[left(true)]);
        assert!(i.is_left_click());
        i.end_frame();
        frame(&mut i, &[left(false)]);
        i.end_frame();
        frame(&mut i, &[left(true)]);
        assert!(i.is_left_double_click() && !i.is_left_click());
        i.end_frame();
        frame(&mut i, &[left(false)]);
        i.end_frame();
        // after the double click the next press is a click again
        for _ in 0..10 {
            frame(&mut i, &[]);
            i.end_frame();
        }
        frame(&mut i, &[left(true)]);
        assert!(i.is_left_click());
    }

    #[test]
    fn keys_queue_and_status() {
        let mut i = UiInput::new(800, 600);
        frame(&mut i, &[key(keys::KEY_A, true, Some("a"))]);
        assert_eq!(i.key_status(keys::KEY_A), KeyStatus::Pressed);
        assert_eq!(i.pop_key(), (keys::KEY_A << 8) | u32::from(b'a'));
        assert_eq!(i.pop_key(), 0);
        i.end_frame();
        frame(&mut i, &[]);
        assert_eq!(i.key_status(keys::KEY_A), KeyStatus::Held);
        assert!(i.key_down(keys::KEY_A));
    }
}
