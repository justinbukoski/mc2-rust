// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mission hot-key table (`MissionInterfaceManager::commands`, `code/missiongui.cpp`) and the
//! key strings the options screen shows (`OptionsHotKeys::makeKeyString` /
//! `makeInputKeyString`, `code/optionsarea.cpp`).
//!
//! A hot key is a game key index with modifier bits: `CTRL`, `SHIFT`, `ALT`, `WAYPT` (waypoint
//! mode) and `0x100` (the original marks extended keys with it). The table keeps the original's
//! order, so a command's index is the same as in the C++ (`OldKeys`, `options.cfg` `Key<n>`).

use mc2_platform::keys::{self, GameKey};

pub const CTRL: u32 = 0x1000_0000;
pub const SHIFT: u32 = 0x0100_0000;
pub const ALT: u32 = 0x0010_0000;
pub const WAYPT: u32 = 0x2000_0000;
/// `MAX_COMMAND`.
pub const MAX_COMMAND: usize = 107;

/// One `MissionInterfaceManager::Command` (the handler is named, not bound).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Command {
    pub key: u32,
    /// Cursor (`mState_*`) while the command is armed, and with line of sight.
    pub cursor: i32,
    pub cursor_los: i32,
    pub single_click: bool,
    /// String id of the description shown in the options list, or -1 when not listed.
    pub description: i32,
    /// The C++ handler (`MissionInterfaceManager::<name>`), called when the key goes down.
    pub handler: &'static str,
    /// The handler called when the key is released, if any.
    pub release_handler: Option<&'static str>,
}

const fn hk(
    key: u32,
    cursor: i32,
    cursor_los: i32,
    single_click: bool,
    description: i32,
    handler: &'static str,
    release_handler: Option<&'static str>,
) -> Command {
    Command {
        key,
        cursor,
        cursor_los,
        single_click,
        description,
        handler,
        release_handler,
    }
}

/// The default commands, in the original order.
#[rustfmt::skip]
pub const COMMANDS: [Command; MAX_COMMAND] = [
    hk(keys::KEY_S, 46, 242, false, 43200, "attackShort", Some("defaultAttack")),
    hk(keys::KEY_M, 47, 241, false, 43201, "attackMedium", Some("defaultAttack")),
    hk(keys::KEY_L, 45, 240, false, 43202, "attackLong", Some("defaultAttack")),
    hk(keys::KEY_A, 17, 18, false, 43204, "energyWeapons", Some("energyWeapons")),
    hk(keys::KEY_D, 13, 14, false, -1, "defaultAttack", None),
    hk(keys::KEY_J, 176, 5, false, 43205, "jump", Some("stopJump")),
    hk(keys::KEY_J | WAYPT, 10, 11, false, -1, "jump", Some("stopJump")),
    hk(keys::KEY_C, 15, 16, false, 43206, "fireFromCurrentPos", Some("stopFireFromCurrentPos")),
    hk(keys::KEY_G, 39, 39, false, 43207, "guard", Some("stopGuard")),
    hk(keys::KEY_W, 17, 18, false, -1, "conserveAmmo", None),
    hk(keys::KEY_E, -1, -1, false, 43209, "selectVisible", None),
    hk(keys::KEY_F, -1, -1, false, 43210, "forceShot", None),
    hk(keys::KEY_HOME, -1, -1, true, -1, "cameraNormal", None),
    hk(keys::KEY_F2, -1, -1, true, -1, "cameraDefault", None),
    hk(keys::KEY_F3, -1, -1, true, -1, "cameraMaxIn", None),
    hk(keys::KEY_F4, -1, -1, true, -1, "cameraTight", None),
    hk(keys::KEY_F5, -1, -1, true, -1, "cameraFour", None),
    hk(keys::KEY_F2 | CTRL, -1, -1, true, -1, "cameraAssign0", None),
    hk(keys::KEY_F3 | CTRL, -1, -1, true, -1, "cameraAssign1", None),
    hk(keys::KEY_F4 | CTRL, -1, -1, true, -1, "cameraAssign2", None),
    hk(keys::KEY_F5 | CTRL, -1, -1, true, -1, "cameraAssign3", None),
    hk(keys::KEY_NUMPAD2, 30, 20, false, 43211, "aimLeg", None),
    hk(keys::KEY_NUMPAD5, 30, 20, false, 43212, "aimArm", None),
    hk(keys::KEY_NUMPAD8, 30, 20, false, 43213, "aimHead", None),
    hk(keys::KEY_BACK, -1, -1, false, 43214, "removeCommand", None),
    hk(keys::KEY_NEXT | 0x100, -1, -1, false, 43215, "powerDown", None),
    hk(keys::KEY_PRIOR | 0x100, -1, -1, false, 43216, "powerUp", None),
    hk(keys::KEY_END | 0x100, 33, 33, false, 43217, "eject", None),
    hk(keys::KEY_SPACE, 2, 3, false, 43218, "changeSpeed", Some("stopChangeSpeed")),
    hk(keys::KEY_SPACE | WAYPT, 8, 9, false, -1, "changeSpeed", Some("stopChangeSpeed")),
    hk(keys::KEY_UP | 0x100, -1, -1, false, 43244, "scrollUp", None),
    hk(keys::KEY_DOWN | 0x100, -1, -1, false, 43245, "scrollDown", None),
    hk(keys::KEY_LEFT | 0x100, -1, -1, false, 43242, "scrollLeft", None),
    hk(keys::KEY_RIGHT | 0x100, -1, -1, false, 43243, "scrollRight", None),
    hk(keys::KEY_SUBTRACT, -1, -1, false, 43225, "zoomOut", None),
    hk(keys::KEY_MINUS, -1, -1, false, -1, "zoomOut", None),
    hk(keys::KEY_ADD, -1, -1, false, 43224, "zoomIn", None),
    hk(SHIFT | keys::KEY_EQUALS, -1, -1, false, -1, "zoomIn", None),
    hk(keys::KEY_LEFT | SHIFT | 0x100, -1, -1, false, 43246, "rotateLeft", None),
    hk(keys::KEY_RIGHT | SHIFT | 0x100, -1, -1, false, 43247, "rotateRight", None),
    hk(keys::KEY_UP | SHIFT | 0x100, -1, -1, false, 43248, "tiltUp", None),
    hk(keys::KEY_DOWN | SHIFT | 0x100, -1, -1, false, 43249, "tiltDown", None),
    hk(keys::KEY_HOME | 0x100, -1, -1, false, -1, "centerCamera", None),
    hk(keys::KEY_LEFT | CTRL, -1, -1, false, -1, "rotateLightLeft", None),
    hk(keys::KEY_RIGHT | CTRL, -1, -1, false, -1, "rotateLightRight", None),
    hk(keys::KEY_UP | CTRL, -1, -1, false, -1, "rotateLightUp", None),
    hk(keys::KEY_DOWN | CTRL, -1, -1, false, -1, "rotateLightDown", None),
    hk(keys::KEY_T | CTRL | ALT, -1, -1, true, -1, "drawTerrain", None),
    hk(keys::KEY_O | CTRL | ALT, -1, -1, true, -1, "drawOverlays", None),
    hk(keys::KEY_B | CTRL | ALT, -1, -1, true, -1, "drawBuildings", None),
    hk(CTRL | ALT | keys::KEY_G, -1, -1, true, -1, "showGrid", None),
    hk(CTRL | ALT | keys::KEY_Q, -1, -1, true, -1, "recalcLights", None),
    hk(CTRL | ALT | keys::KEY_C, -1, -1, true, -1, "drawClouds", None),
    hk(CTRL | ALT | keys::KEY_F, -1, -1, true, -1, "drawFog", None),
    hk(CTRL | ALT | keys::KEY_P, -1, -1, true, -1, "usePerspective", None),
    hk(CTRL | ALT | keys::KEY_S, -1, -1, true, -1, "drawTGLShapes", None),
    hk(CTRL | ALT | keys::KEY_V, -1, -1, true, -1, "drawWaterEffects", None),
    hk(CTRL | ALT | keys::KEY_W, -1, -1, true, -1, "recalcWater", None),
    hk(CTRL | ALT | keys::KEY_D, -1, -1, true, -1, "drawShadows", None),
    hk(CTRL | ALT | keys::KEY_L, -1, -1, true, -1, "changeLighting", None),
    hk(CTRL | ALT | keys::KEY_Z, -1, -1, true, -1, "toggleGUI", None),
    hk(keys::KEY_V, -1, -1, true, 43222, "vehicleCommand", None),
    hk(keys::KEY_F9, -1, -1, true, 43226, "showObjectives", None),
    hk(keys::KEY_ESCAPE, -1, -1, true, 43234, "togglePause", None),
    hk(keys::KEY_F1, -1, -1, true, -1, "toggleHotKeys", None),
    hk(keys::KEY_P, -1, -1, true, -1, "togglePause", None),
    hk(keys::KEY_TAB, -1, -1, true, -1, "switchTab", None),
    hk(keys::KEY_TAB | SHIFT, -1, -1, true, -1, "reverseSwitchTab", None),
    hk(keys::KEY_I, -1, -1, true, 43219, "infoCommand", Some("infoButtonReleased")),
    hk(keys::KEY_N, -1, -1, true, -1, "gotoNextNavMarker", None),
    hk(keys::KEY_MULTIPLY, 25, 26, true, 43220, "sendAirstrike", None),
    hk(keys::KEY_DIVIDE, 27, 27, true, 43221, "sendSensorStrike", None),
    hk(keys::KEY_BACKSLASH, -1, -1, true, 43223, "toggleCompass", None),
    hk(ALT | keys::KEY_SLASH, -1, -1, false, -1, "quickDebugInfo", None),
    hk(ALT | SHIFT | keys::KEY_SLASH, -1, -1, false, -1, "setGameObjectWindow", None),
    hk(ALT | CTRL | keys::KEY_1, -1, -1, false, -1, "pageGameObjectWindow1", None),
    hk(ALT | CTRL | keys::KEY_2, -1, -1, false, -1, "pageGameObjectWindow2", None),
    hk(ALT | CTRL | keys::KEY_3, -1, -1, false, -1, "pageGameObjectWindow3", None),
    hk(ALT | keys::KEY_1, -1, -1, false, -1, "jumpToDebugGameObject1", None),
    hk(ALT | keys::KEY_2, -1, -1, false, -1, "jumpToDebugGameObject2", None),
    hk(ALT | keys::KEY_3, -1, -1, false, -1, "jumpToDebugGameObject3", None),
    hk(ALT | keys::KEY_T, -1, -1, false, -1, "teleport", None),
    hk(ALT | keys::KEY_W, -1, -1, false, -1, "toggleDebugWins", None),
    hk(ALT | keys::KEY_M, -1, -1, false, -1, "showMovers", None),
    hk(ALT | keys::KEY_C, -1, -1, false, -1, "cullPathAreas", None),
    hk(ALT | keys::KEY_Z, -1, -1, false, -1, "zeroHPrime", None),
    hk(ALT | keys::KEY_A, -1, -1, false, -1, "calcValidAreaTable", None),
    hk(ALT | keys::KEY_G, -1, -1, false, -1, "globalMapLog", None),
    hk(ALT | keys::KEY_B, -1, -1, false, -1, "brainDead", None),
    hk(ALT | keys::KEY_P, -1, -1, false, -1, "goalPlan", None),
    hk(ALT | keys::KEY_V, -1, -1, false, -1, "showVictim", None),
    hk(ALT | CTRL | keys::KEY_P, -1, -1, false, -1, "enemyGoalPlan", None),
    hk(ALT | keys::KEY_4, -1, -1, false, -1, "damageObject1", None),
    hk(ALT | keys::KEY_5, -1, -1, false, -1, "damageObject2", None),
    hk(ALT | keys::KEY_6, -1, -1, false, -1, "damageObject3", None),
    hk(ALT | keys::KEY_7, -1, -1, false, -1, "damageObject4", None),
    hk(ALT | keys::KEY_8, -1, -1, false, -1, "damageObject5", None),
    hk(ALT | keys::KEY_9, -1, -1, false, -1, "damageObject6", None),
    hk(ALT | keys::KEY_0, -1, -1, false, -1, "damageObject0", None),
    hk(keys::KEY_H, -1, -1, true, -1, "toggleHoldPosition", None),
    hk(keys::KEY_RETURN, -1, -1, true, 43250, "handleChatKey", None),
    hk(SHIFT | keys::KEY_RETURN, -1, -1, true, 43251, "handleTeamChatKey", None),
    hk(keys::KEY_E | SHIFT, -1, -1, false, -1, "addVisibleToSelection", None),
    hk(keys::KEY_EQUALS, -1, -1, false, -1, "zoomIn", None),
    hk(ALT | keys::KEY_PERIOD, -1, -1, false, -1, "rotateObjectLeft", None),
    hk(ALT | keys::KEY_COMMA, -1, -1, false, -1, "rotateObjectRight", None),
    hk(keys::KEY_PAUSE, -1, -1, true, -1, "togglePause", None),
];

/// `IDS_SHIFT`, `IDS_CONTROL`, `IDS_ALT`.
pub const IDS_SHIFT: u32 = 43228;
pub const IDS_CONTROL: u32 = 43229;
pub const IDS_ALT: u32 = 43230;

fn modifier_prefix(strings: &crate::strings::StringTable, key: u32, out: &mut String) {
    for (bit, id) in [(SHIFT, IDS_SHIFT), (CTRL, IDS_CONTROL), (ALT, IDS_ALT)] {
        if key & bit != 0 {
            out.push_str(strings.load(id));
            out.push_str(" + ");
        }
    }
}

/// `makeKeyString`: e.g. `"Shift + Cursor Left"`.
pub fn key_string(strings: &crate::strings::StringTable, key: u32) -> String {
    let mut s = String::new();
    modifier_prefix(strings, key, &mut s);
    s.push_str(keys::key_name((key & 0xff) as GameKey));
    s
}

/// `makeInputKeyString`: the hot key a pressed key (`gos_GetKey` value) makes with the held
/// modifiers, or `None` for a lone modifier key.
pub fn input_hot_key(gos_key: u32, shift: bool, ctrl: bool, alt: bool) -> Option<u32> {
    let k = (gos_key >> 8) & 0x1ff;
    if k == keys::KEY_LSHIFT || k == keys::KEY_LMENU || k == keys::KEY_LCONTROL {
        return None;
    }
    let mut hot = k;
    if shift {
        hot |= SHIFT;
    }
    if ctrl {
        hot |= CTRL;
    }
    if alt {
        hot |= ALT;
    }
    Some(hot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strings::StringTable;

    #[test]
    fn table_matches_the_original_order() {
        assert_eq!(COMMANDS[0].key, keys::KEY_S);
        assert_eq!(COMMANDS[0].description, 43200);
        assert_eq!(COMMANDS[6].key, keys::KEY_J | WAYPT);
        assert_eq!(COMMANDS[106].handler, "togglePause");
        assert_eq!(COMMANDS[5].release_handler, Some("stopJump"));
        assert_eq!(COMMANDS[0].release_handler, Some("defaultAttack"));
        assert_eq!(COMMANDS[4].release_handler, None);
        let listed = COMMANDS.iter().filter(|c| c.description != -1).count();
        assert!(listed > 30, "{listed}");
    }

    #[test]
    fn key_strings() {
        let Some(t) = StringTable::builtin_or_skip() else {
            return;
        };
        assert_eq!(t.load(IDS_SHIFT), "Shift");
        let s = key_string(t, keys::KEY_LEFT | SHIFT | 0x100);
        assert_eq!(s, format!("{} + Cursor Left", t.load(IDS_SHIFT)));
        assert_eq!(
            input_hot_key(keys::KEY_LSHIFT << 8, true, false, false),
            None
        );
        assert_eq!(
            input_hot_key((keys::KEY_A << 8) | 97, false, true, false),
            Some(keys::KEY_A | CTRL)
        );
    }
}
