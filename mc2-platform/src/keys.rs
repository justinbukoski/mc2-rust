// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Game key codes and the mapping from winit physical keys.
//!
//! The key code values are the game's `gosEnum_KeyIndex` constants (taken from the public
//! `GameOS/include/gameos.hpp` header): they follow the Windows virtual-key numbering, with the
//! mouse buttons occupying the otherwise unused indices 1..=5. Both Shift keys report
//! `KEY_LSHIFT`, both Controls `KEY_LCONTROL` and both Alt keys `KEY_LMENU`, exactly as the
//! original did; the separate `KEY_LALT` / `KEY_RALT` indices are driven additionally.

use winit::keyboard::KeyCode;

/// A game key index (`gosEnum_KeyIndex`).
pub type GameKey = u32;

macro_rules! game_keys {
    ($($name:ident = $val:expr),* $(,)?) => {
        $(pub const $name: GameKey = $val;)*
    };
}

game_keys! {
    KEY_LMOUSE = 1, KEY_MMOUSE = 2, KEY_RMOUSE = 3, KEY_MOUSEX1 = 4, KEY_MOUSEX2 = 5,
    KEY_BACK = 0x08, KEY_TAB = 0x09, KEY_RETURN = 0x0D, KEY_NUMPADENTER = 0x0D,
    KEY_LSHIFT = 0x10, KEY_RSHIFT = 0x10, KEY_LCONTROL = 0x11, KEY_RCONTROL = 0x11,
    KEY_LMENU = 0x12, KEY_RMENU = 0x12, KEY_PAUSE = 0x13, KEY_CAPITAL = 0x14,
    KEY_ESCAPE = 0x1B, KEY_SPACE = 0x20,
    KEY_PRIOR = 0x21, KEY_NEXT = 0x22, KEY_END = 0x23, KEY_HOME = 0x24,
    KEY_LEFT = 0x25, KEY_UP = 0x26, KEY_RIGHT = 0x27, KEY_DOWN = 0x28,
    KEY_SYSRQ = 0x2C, KEY_INSERT = 0x2D, KEY_DELETE = 0x2E,
    KEY_0 = 0x30, KEY_1 = 0x31, KEY_2 = 0x32, KEY_3 = 0x33, KEY_4 = 0x34,
    KEY_5 = 0x35, KEY_6 = 0x36, KEY_7 = 0x37, KEY_8 = 0x38, KEY_9 = 0x39,
    KEY_A = 0x41, KEY_B = 0x42, KEY_C = 0x43, KEY_D = 0x44, KEY_E = 0x45, KEY_F = 0x46,
    KEY_G = 0x47, KEY_H = 0x48, KEY_I = 0x49, KEY_J = 0x4A, KEY_K = 0x4B, KEY_L = 0x4C,
    KEY_M = 0x4D, KEY_N = 0x4E, KEY_O = 0x4F, KEY_P = 0x50, KEY_Q = 0x51, KEY_R = 0x52,
    KEY_S = 0x53, KEY_T = 0x54, KEY_U = 0x55, KEY_V = 0x56, KEY_W = 0x57, KEY_X = 0x58,
    KEY_Y = 0x59, KEY_Z = 0x5A,
    KEY_LWIN = 0x5B, KEY_RWIN = 0x5C, KEY_APPS = 0x5D,
    KEY_NUMPAD0 = 0x60, KEY_NUMPAD1 = 0x61, KEY_NUMPAD2 = 0x62, KEY_NUMPAD3 = 0x63,
    KEY_NUMPAD4 = 0x64, KEY_NUMPAD5 = 0x65, KEY_NUMPAD6 = 0x66, KEY_NUMPAD7 = 0x67,
    KEY_NUMPAD8 = 0x68, KEY_NUMPAD9 = 0x69,
    KEY_MULTIPLY = 0x6A, KEY_ADD = 0x6B, KEY_NUMPADCOMMA = 0x6C, KEY_SUBTRACT = 0x6D,
    KEY_DECIMAL = 0x6E, KEY_DIVIDE = 0x6F,
    KEY_F1 = 0x70, KEY_F2 = 0x71, KEY_F3 = 0x72, KEY_F4 = 0x73, KEY_F5 = 0x74, KEY_F6 = 0x75,
    KEY_F7 = 0x76, KEY_F8 = 0x77, KEY_F9 = 0x78, KEY_F10 = 0x79, KEY_F11 = 0x7A, KEY_F12 = 0x7B,
    KEY_F13 = 0x7C, KEY_F14 = 0x7D, KEY_F15 = 0x7E,
    KEY_NUMLOCK = 0x90, KEY_NUMPADEQUALS = 0x92,
    KEY_SEMICOLON = 0xBA, KEY_EQUALS = 0xBB, KEY_COMMA = 0xBC, KEY_MINUS = 0xBD,
    KEY_PERIOD = 0xBE, KEY_SLASH = 0xBF, KEY_GRAVE = 0xC0,
    KEY_LBRACKET = 0xDB, KEY_BACKSLASH = 0xDC, KEY_RBRACKET = 0xDD, KEY_APOSTROPHE = 0xDE,
    KEY_LALT = 0xE2, KEY_RALT = 0xE6,
}

/// Number of game key slots (indices `0..KEY_SLOTS`).
pub const KEY_SLOTS: usize = 256;

/// Physical key -> primary game key.
const TABLE: &[(KeyCode, GameKey)] = &[
    (KeyCode::Escape, KEY_ESCAPE),
    (KeyCode::Digit1, KEY_1),
    (KeyCode::Digit2, KEY_2),
    (KeyCode::Digit3, KEY_3),
    (KeyCode::Digit4, KEY_4),
    (KeyCode::Digit5, KEY_5),
    (KeyCode::Digit6, KEY_6),
    (KeyCode::Digit7, KEY_7),
    (KeyCode::Digit8, KEY_8),
    (KeyCode::Digit9, KEY_9),
    (KeyCode::Digit0, KEY_0),
    (KeyCode::Minus, KEY_MINUS),
    (KeyCode::Equal, KEY_EQUALS),
    (KeyCode::Backspace, KEY_BACK),
    (KeyCode::Tab, KEY_TAB),
    (KeyCode::KeyQ, KEY_Q),
    (KeyCode::KeyW, KEY_W),
    (KeyCode::KeyE, KEY_E),
    (KeyCode::KeyR, KEY_R),
    (KeyCode::KeyT, KEY_T),
    (KeyCode::KeyY, KEY_Y),
    (KeyCode::KeyU, KEY_U),
    (KeyCode::KeyI, KEY_I),
    (KeyCode::KeyO, KEY_O),
    (KeyCode::KeyP, KEY_P),
    (KeyCode::BracketLeft, KEY_LBRACKET),
    (KeyCode::BracketRight, KEY_RBRACKET),
    (KeyCode::Enter, KEY_RETURN),
    (KeyCode::ControlLeft, KEY_LCONTROL),
    (KeyCode::ControlRight, KEY_RCONTROL),
    (KeyCode::KeyA, KEY_A),
    (KeyCode::KeyS, KEY_S),
    (KeyCode::KeyD, KEY_D),
    (KeyCode::KeyF, KEY_F),
    (KeyCode::KeyG, KEY_G),
    (KeyCode::KeyH, KEY_H),
    (KeyCode::KeyJ, KEY_J),
    (KeyCode::KeyK, KEY_K),
    (KeyCode::KeyL, KEY_L),
    (KeyCode::Semicolon, KEY_SEMICOLON),
    (KeyCode::Quote, KEY_APOSTROPHE),
    (KeyCode::Backquote, KEY_GRAVE),
    (KeyCode::ShiftLeft, KEY_LSHIFT),
    (KeyCode::ShiftRight, KEY_RSHIFT),
    (KeyCode::Backslash, KEY_BACKSLASH),
    (KeyCode::IntlBackslash, KEY_BACKSLASH),
    (KeyCode::KeyZ, KEY_Z),
    (KeyCode::KeyX, KEY_X),
    (KeyCode::KeyC, KEY_C),
    (KeyCode::KeyV, KEY_V),
    (KeyCode::KeyB, KEY_B),
    (KeyCode::KeyN, KEY_N),
    (KeyCode::KeyM, KEY_M),
    (KeyCode::Comma, KEY_COMMA),
    (KeyCode::Period, KEY_PERIOD),
    (KeyCode::Slash, KEY_SLASH),
    (KeyCode::NumpadMultiply, KEY_MULTIPLY),
    (KeyCode::AltLeft, KEY_LMENU),
    (KeyCode::AltRight, KEY_RMENU),
    (KeyCode::Space, KEY_SPACE),
    (KeyCode::CapsLock, KEY_CAPITAL),
    (KeyCode::F1, KEY_F1),
    (KeyCode::F2, KEY_F2),
    (KeyCode::F3, KEY_F3),
    (KeyCode::F4, KEY_F4),
    (KeyCode::F5, KEY_F5),
    (KeyCode::F6, KEY_F6),
    (KeyCode::F7, KEY_F7),
    (KeyCode::F8, KEY_F8),
    (KeyCode::F9, KEY_F9),
    (KeyCode::F10, KEY_F10),
    (KeyCode::F11, KEY_F11),
    (KeyCode::F12, KEY_F12),
    (KeyCode::F13, KEY_F13),
    (KeyCode::F14, KEY_F14),
    (KeyCode::F15, KEY_F15),
    (KeyCode::Numpad0, KEY_NUMPAD0),
    (KeyCode::Numpad1, KEY_NUMPAD1),
    (KeyCode::Numpad2, KEY_NUMPAD2),
    (KeyCode::Numpad3, KEY_NUMPAD3),
    (KeyCode::Numpad4, KEY_NUMPAD4),
    (KeyCode::Numpad5, KEY_NUMPAD5),
    (KeyCode::Numpad6, KEY_NUMPAD6),
    (KeyCode::Numpad7, KEY_NUMPAD7),
    (KeyCode::Numpad8, KEY_NUMPAD8),
    (KeyCode::Numpad9, KEY_NUMPAD9),
    (KeyCode::NumpadSubtract, KEY_SUBTRACT),
    (KeyCode::NumpadAdd, KEY_ADD),
    (KeyCode::NumpadDecimal, KEY_DECIMAL),
    (KeyCode::NumpadEnter, KEY_NUMPADENTER),
    (KeyCode::NumpadComma, KEY_NUMPADCOMMA),
    (KeyCode::NumpadEqual, KEY_NUMPADEQUALS),
    (KeyCode::NumpadDivide, KEY_DIVIDE),
    (KeyCode::PrintScreen, KEY_SYSRQ),
    (KeyCode::Home, KEY_HOME),
    (KeyCode::ArrowUp, KEY_UP),
    (KeyCode::PageUp, KEY_PRIOR),
    (KeyCode::ArrowLeft, KEY_LEFT),
    (KeyCode::ArrowRight, KEY_RIGHT),
    (KeyCode::End, KEY_END),
    (KeyCode::ArrowDown, KEY_DOWN),
    (KeyCode::PageDown, KEY_NEXT),
    (KeyCode::Insert, KEY_INSERT),
    (KeyCode::Delete, KEY_DELETE),
    (KeyCode::SuperLeft, KEY_LWIN),
    (KeyCode::SuperRight, KEY_RWIN),
    (KeyCode::ContextMenu, KEY_APPS),
    (KeyCode::NumLock, KEY_NUMLOCK),
    (KeyCode::Pause, KEY_PAUSE),
];

/// Physical keys that additionally drive a second, side-specific game key.
const AUX_TABLE: &[(KeyCode, GameKey)] =
    &[(KeyCode::AltLeft, KEY_LALT), (KeyCode::AltRight, KEY_RALT)];

/// Offset added to a primary table index to form the id of an auxiliary entry.
pub const AUX_ID_BASE: u16 = 128;

/// One resolved key transition source: a stable id for the physical key and the game key it drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyMapping {
    /// Unique per (physical key, game key) pair, `< 256`. Used to keep a game key held while any
    /// physical key mapped to it is still down (e.g. two Shift keys).
    pub phys_id: u16,
    pub key: GameKey,
}

/// Resolve a winit physical key to the game keys it drives (primary first, then the optional
/// side-specific one). Unmapped keys yield an empty list.
pub fn map_physical_key(code: KeyCode) -> Vec<KeyMapping> {
    let mut out = Vec::with_capacity(2);
    if let Some(i) = TABLE.iter().position(|&(c, _)| c == code) {
        out.push(KeyMapping {
            phys_id: i as u16,
            key: TABLE[i].1,
        });
    }
    if let Some(i) = AUX_TABLE.iter().position(|&(c, _)| c == code) {
        out.push(KeyMapping {
            phys_id: AUX_ID_BASE + i as u16,
            key: AUX_TABLE[i].1,
        });
    }
    out
}

/// Displayable name of a game key (the `gos_DescribeKey` equivalent).
pub fn key_name(key: GameKey) -> &'static str {
    match key {
        KEY_LMOUSE => "Left Mouse",
        KEY_MMOUSE => "Middle Mouse",
        KEY_RMOUSE => "Right Mouse",
        KEY_MOUSEX1 => "Mouse Button 4",
        KEY_MOUSEX2 => "Mouse Button 5",
        KEY_ESCAPE => "Escape",
        KEY_BACK => "Backspace",
        KEY_TAB => "Tab",
        KEY_RETURN => "Enter",
        KEY_LSHIFT => "Shift",
        KEY_LCONTROL => "Ctrl",
        KEY_LMENU => "Alt",
        KEY_LALT => "Left Alt",
        KEY_RALT => "Right Alt",
        KEY_PAUSE => "Pause",
        KEY_CAPITAL => "Caps Lock",
        KEY_SPACE => "Space",
        KEY_PRIOR => "Page Up",
        KEY_NEXT => "Page Down",
        KEY_END => "End",
        KEY_HOME => "Home",
        KEY_LEFT => "Cursor Left",
        KEY_UP => "Cursor Up",
        KEY_RIGHT => "Cursor Right",
        KEY_DOWN => "Cursor Down",
        KEY_SYSRQ => "Print Screen",
        KEY_INSERT => "Insert",
        KEY_DELETE => "Delete",
        KEY_0 => "0",
        KEY_1 => "1",
        KEY_2 => "2",
        KEY_3 => "3",
        KEY_4 => "4",
        KEY_5 => "5",
        KEY_6 => "6",
        KEY_7 => "7",
        KEY_8 => "8",
        KEY_9 => "9",
        KEY_A => "A",
        KEY_B => "B",
        KEY_C => "C",
        KEY_D => "D",
        KEY_E => "E",
        KEY_F => "F",
        KEY_G => "G",
        KEY_H => "H",
        KEY_I => "I",
        KEY_J => "J",
        KEY_K => "K",
        KEY_L => "L",
        KEY_M => "M",
        KEY_N => "N",
        KEY_O => "O",
        KEY_P => "P",
        KEY_Q => "Q",
        KEY_R => "R",
        KEY_S => "S",
        KEY_T => "T",
        KEY_U => "U",
        KEY_V => "V",
        KEY_W => "W",
        KEY_X => "X",
        KEY_Y => "Y",
        KEY_Z => "Z",
        KEY_LWIN => "Left Windows",
        KEY_RWIN => "Right Windows",
        KEY_APPS => "Menu",
        KEY_NUMPAD0 => "Numpad 0",
        KEY_NUMPAD1 => "Numpad 1",
        KEY_NUMPAD2 => "Numpad 2",
        KEY_NUMPAD3 => "Numpad 3",
        KEY_NUMPAD4 => "Numpad 4",
        KEY_NUMPAD5 => "Numpad 5",
        KEY_NUMPAD6 => "Numpad 6",
        KEY_NUMPAD7 => "Numpad 7",
        KEY_NUMPAD8 => "Numpad 8",
        KEY_NUMPAD9 => "Numpad 9",
        KEY_MULTIPLY => "Numpad *",
        KEY_ADD => "Numpad +",
        KEY_NUMPADCOMMA => "Numpad ,",
        KEY_SUBTRACT => "Numpad -",
        KEY_DECIMAL => "Numpad .",
        KEY_DIVIDE => "Numpad /",
        KEY_F1 => "F1",
        KEY_F2 => "F2",
        KEY_F3 => "F3",
        KEY_F4 => "F4",
        KEY_F5 => "F5",
        KEY_F6 => "F6",
        KEY_F7 => "F7",
        KEY_F8 => "F8",
        KEY_F9 => "F9",
        KEY_F10 => "F10",
        KEY_F11 => "F11",
        KEY_F12 => "F12",
        KEY_F13 => "F13",
        KEY_F14 => "F14",
        KEY_F15 => "F15",
        KEY_NUMLOCK => "Num Lock",
        KEY_NUMPADEQUALS => "Numpad =",
        KEY_SEMICOLON => ";",
        KEY_EQUALS => "=",
        KEY_COMMA => ",",
        KEY_MINUS => "-",
        KEY_PERIOD => ".",
        KEY_SLASH => "/",
        KEY_GRAVE => "`",
        KEY_LBRACKET => "[",
        KEY_BACKSLASH => "\\",
        KEY_RBRACKET => "]",
        KEY_APOSTROPHE => "'",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits_follow_ascii() {
        assert_eq!(map_physical_key(KeyCode::KeyA)[0].key, b'A' as u32);
        assert_eq!(map_physical_key(KeyCode::KeyZ)[0].key, b'Z' as u32);
        assert_eq!(map_physical_key(KeyCode::Digit0)[0].key, b'0' as u32);
        assert_eq!(map_physical_key(KeyCode::Digit9)[0].key, b'9' as u32);
    }

    #[test]
    fn matches_gameos_header_values() {
        assert_eq!(map_physical_key(KeyCode::Escape)[0].key, 0x1B);
        assert_eq!(map_physical_key(KeyCode::Space)[0].key, 0x20);
        assert_eq!(map_physical_key(KeyCode::Enter)[0].key, 0x0D);
        assert_eq!(map_physical_key(KeyCode::F4)[0].key, 0x73);
        assert_eq!(map_physical_key(KeyCode::ArrowLeft)[0].key, 0x25);
        assert_eq!(map_physical_key(KeyCode::Minus)[0].key, 0xBD);
        assert_eq!(map_physical_key(KeyCode::Backquote)[0].key, 0xC0);
        assert_eq!(map_physical_key(KeyCode::Numpad7)[0].key, 0x67);
    }

    #[test]
    fn both_shifts_share_a_key_but_not_an_id() {
        let l = map_physical_key(KeyCode::ShiftLeft)[0];
        let r = map_physical_key(KeyCode::ShiftRight)[0];
        assert_eq!(l.key, r.key);
        assert_ne!(l.phys_id, r.phys_id);
    }

    #[test]
    fn alt_drives_menu_and_side_specific_key() {
        let l = map_physical_key(KeyCode::AltLeft);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].key, KEY_LMENU);
        assert_eq!(l[1].key, KEY_LALT);
        let r = map_physical_key(KeyCode::AltRight);
        assert_eq!(r[1].key, KEY_RALT);
        assert!(l
            .iter()
            .chain(r.iter())
            .all(|m| (m.phys_id as usize) < KEY_SLOTS));
    }

    #[test]
    fn unmapped_key_is_empty() {
        assert!(map_physical_key(KeyCode::BrowserBack).is_empty());
    }

    #[test]
    fn ids_are_unique_and_in_range() {
        let mut seen = std::collections::HashSet::new();
        for (i, (code, _)) in TABLE.iter().enumerate() {
            assert!(i < AUX_ID_BASE as usize);
            assert!(seen.insert(*code), "duplicate table entry {code:?}");
        }
    }

    #[test]
    fn names() {
        assert_eq!(key_name(KEY_A), "A");
        assert_eq!(key_name(KEY_LEFT), "Cursor Left");
        assert_eq!(key_name(0xFE), "");
    }
}
