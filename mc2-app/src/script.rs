//! Input scripts for headless runs: timed `InputEvent`s, so camera control (and later orders) can
//! be driven and tested without a window.
//!
//! One directive per line, `#` starts a comment.  Each line starts with the 0-based frame number
//! the directive is applied before:
//!
//! ```text
//! 10  key ArrowLeft down        # winit KeyCode names, or aliases: LEFT SHIFT ALT CTRL F2 A 5 ...
//! 40  key ArrowLeft up
//! 50  tap F3 [frames]           # down at 50, up `frames` later (default 1)
//! 60  move 400 300              # pointer, window logical points
//! 61  button right down         # left | right | middle | x1 | x2, down | up
//! 70  wheel 1 [dx]              # notches, positive away from the user
//! 80  text hello
//! 90  focus_lost
//! 95  enter | leave             # pointer enters / leaves the window
//! 100 resize 1280 720           # headless: new frame size (logical = physical)
//! 110 screenshot shots/a.png    # write a PNG after this frame is drawn
//! ```
//!
//! Keys go through `mc2_platform::window::translate_key`, so a script key produces exactly the
//! events a real keyboard does (Alt drives both `KEY_LMENU` and `KEY_LALT`, for instance).

use mc2_platform::input::{InputEvent, MouseButton};
use mc2_platform::window::translate_key;
use mc2_platform::winit::event::ElementState;
use mc2_platform::winit::keyboard::{KeyCode, PhysicalKey};

/// One scripted action.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Event(InputEvent),
    Resize(u32, u32),
    Screenshot(String),
}

/// A parsed script: actions sorted by frame (stable, so same-frame lines keep their order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputScript {
    pub actions: Vec<(u64, Action)>,
}

impl InputScript {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut actions = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let err = |m: &str| format!("input script line {}: {m}: {raw}", n + 1);
            let mut it = line.split_whitespace();
            let frame: u64 = it
                .next()
                .and_then(|t| t.parse().ok())
                .ok_or_else(|| err("expected a frame number"))?;
            let verb = it.next().ok_or_else(|| err("expected a directive"))?;
            let rest: Vec<&str> = it.collect();
            let num = |i: usize| -> Result<f64, String> {
                rest.get(i)
                    .and_then(|t| t.parse::<f64>().ok())
                    .ok_or_else(|| err("expected a number"))
            };
            match verb {
                "key" => {
                    let code = key_code(rest.first().copied().unwrap_or(""))
                        .ok_or_else(|| err("unknown key"))?;
                    let down = match rest.get(1).copied() {
                        Some("down") => true,
                        Some("up") => false,
                        _ => return Err(err("expected down or up")),
                    };
                    for e in key_events(code, down) {
                        actions.push((frame, Action::Event(e)));
                    }
                }
                "tap" => {
                    let code = key_code(rest.first().copied().unwrap_or(""))
                        .ok_or_else(|| err("unknown key"))?;
                    let hold = match rest.get(1) {
                        Some(t) => t.parse::<u64>().map_err(|_| err("bad frame count"))?,
                        None => 1,
                    };
                    for e in key_events(code, true) {
                        actions.push((frame, Action::Event(e)));
                    }
                    for e in key_events(code, false) {
                        actions.push((frame + hold.max(1), Action::Event(e)));
                    }
                }
                "move" => actions.push((
                    frame,
                    Action::Event(InputEvent::CursorMoved {
                        x: num(0)?,
                        y: num(1)?,
                    }),
                )),
                "button" => {
                    let button = match rest.first().copied() {
                        Some("left") => MouseButton::Left,
                        Some("right") => MouseButton::Right,
                        Some("middle") => MouseButton::Middle,
                        Some("x1") => MouseButton::X1,
                        Some("x2") => MouseButton::X2,
                        _ => return Err(err("unknown button")),
                    };
                    let pressed = match rest.get(1).copied() {
                        Some("down") => true,
                        Some("up") => false,
                        _ => return Err(err("expected down or up")),
                    };
                    actions.push((frame, Action::Event(InputEvent::Button { button, pressed })));
                }
                "wheel" => {
                    let y = num(0)? as f32;
                    let x = if rest.len() > 1 { num(1)? as f32 } else { 0.0 };
                    actions.push((frame, Action::Event(InputEvent::Wheel { x, y })));
                }
                "text" => actions.push((frame, Action::Event(InputEvent::Text(rest.join(" "))))),
                "focus_lost" => actions.push((frame, Action::Event(InputEvent::FocusLost))),
                "enter" => actions.push((frame, Action::Event(InputEvent::CursorEntered))),
                "leave" => actions.push((frame, Action::Event(InputEvent::CursorLeft))),
                "resize" => {
                    let (w, h) = (num(0)?, num(1)?);
                    if w < 1.0 || h < 1.0 || w > 16384.0 || h > 16384.0 {
                        return Err(err("size out of range"));
                    }
                    actions.push((frame, Action::Resize(w as u32, h as u32)));
                }
                "screenshot" => {
                    let path = rest.join(" ");
                    if path.is_empty() {
                        return Err(err("expected a path"));
                    }
                    actions.push((frame, Action::Screenshot(path)));
                }
                _ => return Err(err("unknown directive")),
            }
        }
        actions.sort_by_key(|(f, _)| *f);
        Ok(Self { actions })
    }

    /// Actions of `frame`, in script order.
    pub fn at(&self, frame: u64) -> impl Iterator<Item = &Action> + '_ {
        let start = self.actions.partition_point(|(f, _)| *f < frame);
        self.actions[start..]
            .iter()
            .take_while(move |(f, _)| *f == frame)
            .map(|(_, a)| a)
    }

    pub fn last_frame(&self) -> Option<u64> {
        self.actions.last().map(|(f, _)| *f)
    }
}

/// The events a physical key transition produces (as the window layer translates them).
pub fn key_events(code: KeyCode, down: bool) -> Vec<InputEvent> {
    let state = if down {
        ElementState::Pressed
    } else {
        ElementState::Released
    };
    translate_key(PhysicalKey::Code(code), state, false, None)
}

/// A key by winit `KeyCode` name (`KeyA`, `ArrowLeft`, `ShiftLeft`, `NumpadAdd` ...) or a short
/// alias (`A`, `LEFT`, `SHIFT`, `CTRL`, `ALT`, `F2`, `5`, `MINUS`, `HOME` ...), case-insensitive.
pub fn key_code(name: &str) -> Option<KeyCode> {
    use KeyCode as K;
    let n = name.to_ascii_lowercase();
    let letters = [
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
    ];
    let digits = [
        K::Digit0,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
    ];
    let fkeys = [
        K::F1,
        K::F2,
        K::F3,
        K::F4,
        K::F5,
        K::F6,
        K::F7,
        K::F8,
        K::F9,
        K::F10,
        K::F11,
        K::F12,
    ];
    let letter = |s: &str| -> Option<KeyCode> {
        let b = s.as_bytes();
        (b.len() == 1 && b[0].is_ascii_lowercase()).then(|| letters[(b[0] - b'a') as usize])
    };
    let digit = |s: &str| -> Option<KeyCode> {
        let b = s.as_bytes();
        (b.len() == 1 && b[0].is_ascii_digit()).then(|| digits[(b[0] - b'0') as usize])
    };
    if let Some(k) = letter(&n).or_else(|| n.strip_prefix("key").and_then(letter)) {
        return Some(k);
    }
    if let Some(k) = digit(&n).or_else(|| n.strip_prefix("digit").and_then(digit)) {
        return Some(k);
    }
    if let Some(i) = n.strip_prefix('f').and_then(|r| r.parse::<usize>().ok()) {
        return fkeys.get(i.wrapping_sub(1)).copied();
    }
    Some(match n.as_str() {
        "left" | "arrowleft" => K::ArrowLeft,
        "right" | "arrowright" => K::ArrowRight,
        "up" | "arrowup" => K::ArrowUp,
        "down" | "arrowdown" => K::ArrowDown,
        "shift" | "lshift" | "shiftleft" => K::ShiftLeft,
        "rshift" | "shiftright" => K::ShiftRight,
        "ctrl" | "control" | "lctrl" | "controlleft" => K::ControlLeft,
        "rctrl" | "controlright" => K::ControlRight,
        "alt" | "lalt" | "altleft" => K::AltLeft,
        "ralt" | "altright" => K::AltRight,
        "space" => K::Space,
        "enter" | "return" => K::Enter,
        "escape" | "esc" => K::Escape,
        "tab" => K::Tab,
        "backspace" | "back" => K::Backspace,
        "home" => K::Home,
        "end" => K::End,
        "pageup" | "prior" => K::PageUp,
        "pagedown" | "next" => K::PageDown,
        "insert" => K::Insert,
        "delete" => K::Delete,
        "minus" => K::Minus,
        "equal" | "equals" => K::Equal,
        "add" | "numpadadd" => K::NumpadAdd,
        "subtract" | "numpadsubtract" => K::NumpadSubtract,
        "multiply" | "numpadmultiply" => K::NumpadMultiply,
        "divide" | "numpaddivide" => K::NumpadDivide,
        "numpad2" => K::Numpad2,
        "numpad5" => K::Numpad5,
        "numpad8" => K::Numpad8,
        "backslash" => K::Backslash,
        "pause" => K::Pause,
        "bracketleft" => K::BracketLeft,
        "bracketright" => K::BracketRight,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_platform::keys;

    #[test]
    fn parses_every_directive() {
        let s = InputScript::parse(
            "# camera test\n5 key LEFT down\n9 key LEFT up\n2 tap F3 4\n3 move 10 20\n3 button right down\n4 wheel -1\n6 text hi there\n7 focus_lost\n7 enter\n8 resize 640 480\n8 screenshot out/a b.png\n",
        )
        .unwrap();
        assert_eq!(s.at(2).count(), 1);
        assert_eq!(s.at(6).count(), 2); // F3 up (tap) and the text
        assert_eq!(
            s.at(3).collect::<Vec<_>>(),
            vec![
                &Action::Event(InputEvent::CursorMoved { x: 10.0, y: 20.0 }),
                &Action::Event(InputEvent::Button {
                    button: MouseButton::Right,
                    pressed: true
                })
            ]
        );
        assert!(s
            .at(8)
            .any(|a| a == &Action::Screenshot("out/a b.png".into())));
        assert!(s.at(8).any(|a| a == &Action::Resize(640, 480)));
        assert_eq!(s.last_frame(), Some(9));
        let first = s.at(5).next().cloned();
        match first {
            Some(Action::Event(InputEvent::Key { key, pressed, .. })) => {
                assert_eq!(key, keys::KEY_LEFT);
                assert!(pressed);
            }
            a => panic!("{a:?}"),
        }
    }

    #[test]
    fn alt_drives_both_alt_indices() {
        let ev = key_events(key_code("ALT").unwrap(), true);
        assert_eq!(ev.len(), 2);
    }

    #[test]
    fn key_names_and_errors() {
        assert_eq!(key_code("KeyQ"), Some(KeyCode::KeyQ));
        assert_eq!(key_code("q"), Some(KeyCode::KeyQ));
        assert_eq!(key_code("Digit7"), Some(KeyCode::Digit7));
        assert_eq!(key_code("f12"), Some(KeyCode::F12));
        assert_eq!(key_code("f0"), None);
        assert_eq!(key_code("NumpadAdd"), Some(KeyCode::NumpadAdd));
        assert!(InputScript::parse("x key A down").is_err());
        assert!(InputScript::parse("1 key Nope down").is_err());
        assert!(InputScript::parse("1 key A sideways").is_err());
        assert!(InputScript::parse("1 resize 0 10").is_err());
        assert!(InputScript::parse("1 fly").is_err());
    }
}
