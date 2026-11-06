// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mission hot keys: the `MissionInterfaceManager::commands[]` table and its dispatch loop.
//!
//! Ported from `code/missiongui.cpp` (the `commands[]` initialiser and the loop at the end of
//! `MissionInterfaceManager::update(bool, bool, ...)`).  The app executes the camera and pause
//! commands itself; every other command is handed to the order hook (P3) as a [`Fired`] value.
//! The `#ifndef FINAL` debug toggles (Ctrl+Alt+letter, Alt+number ...) are not ported.

use crate::userinput::{alt, ctrl, key_down, shift};
use mc2_platform::input::{InputState, KeyStatus};
use mc2_platform::keys::*;

/// Modifier bits of a table entry (`CTRL`, `SHIFT`, `ALT`, `WAYPT` in missiongui.cpp).
pub const CTRL: u32 = 0x1000_0000;
pub const SHIFT: u32 = 0x0100_0000;
pub const ALT: u32 = 0x0010_0000;
pub const WAYPT: u32 = 0x2000_0000;

/// A mission command (the member function a table entry calls).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    AttackShort,
    AttackMedium,
    AttackLong,
    EnergyWeapons,
    DefaultAttack,
    Jump,
    StopJump,
    FireFromCurrentPos,
    StopFireFromCurrentPos,
    Guard,
    StopGuard,
    ConserveAmmo,
    SelectVisible,
    ForceShot,
    CameraNormal,
    /// `cameraDefault`, `cameraMaxIn`, `cameraTight`, `cameraFour`: `setCameraView(n)`.
    CameraView(u8),
    /// `cameraAssign0..3`.
    CameraAssign(u8),
    AimLeg,
    AimArm,
    AimHead,
    RemoveCommand,
    PowerDown,
    PowerUp,
    Eject,
    ChangeSpeed,
    StopChangeSpeed,
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
    ZoomOut,
    ZoomIn,
    RotateLeft,
    RotateRight,
    TiltUp,
    TiltDown,
    /// `centerCamera` (returns 0, does nothing).
    CenterCamera,
    RotateLightLeft,
    RotateLightRight,
    RotateLightUp,
    RotateLightDown,
    VehicleCommand,
    ShowObjectives,
    TogglePause,
    ToggleHotKeys,
    SwitchTab,
    ReverseSwitchTab,
    InfoCommand,
    InfoButtonReleased,
    GotoNextNavMarker,
    SendAirstrike,
    SendSensorStrike,
    ToggleCompass,
    ToggleHoldPosition,
    ChatKey,
    TeamChatKey,
    AddVisibleToSelection,
}

impl Command {
    /// Commands the app runs itself (camera, pause); the rest go to the order hook.
    pub fn is_camera(self) -> bool {
        use Command::*;
        matches!(
            self,
            CameraNormal
                | CameraView(_)
                | CameraAssign(_)
                | ScrollUp
                | ScrollDown
                | ScrollLeft
                | ScrollRight
                | ZoomOut
                | ZoomIn
                | RotateLeft
                | RotateRight
                | TiltUp
                | TiltDown
                | CenterCamera
        )
    }
}

/// One `commands[]` entry.
#[derive(Clone, Copy, Debug)]
pub struct HotKey {
    /// Key index plus modifier bits.
    pub key: u32,
    /// `singleClick`: run once per press instead of every frame while held.
    pub single_click: bool,
    pub function: Command,
    pub release: Option<Command>,
}

const fn hk(key: u32, single_click: bool, function: Command, release: Option<Command>) -> HotKey {
    HotKey {
        key,
        single_click,
        function,
        release,
    }
}

use Command as C;

/// `MissionInterfaceManager::commands[]` in table order (non-debug entries).  The `| 0x100`
/// extended-key flag of the original is implied by the platform key codes.
pub const COMMANDS: &[HotKey] = &[
    hk(KEY_S, false, C::AttackShort, Some(C::DefaultAttack)),
    hk(KEY_M, false, C::AttackMedium, Some(C::DefaultAttack)),
    hk(KEY_L, false, C::AttackLong, Some(C::DefaultAttack)),
    hk(KEY_A, false, C::EnergyWeapons, Some(C::EnergyWeapons)),
    hk(KEY_D, false, C::DefaultAttack, None),
    hk(KEY_J, false, C::Jump, Some(C::StopJump)),
    hk(KEY_J | WAYPT, false, C::Jump, Some(C::StopJump)),
    hk(
        KEY_C,
        false,
        C::FireFromCurrentPos,
        Some(C::StopFireFromCurrentPos),
    ),
    hk(KEY_G, false, C::Guard, Some(C::StopGuard)),
    hk(KEY_W, false, C::ConserveAmmo, None),
    hk(KEY_E, false, C::SelectVisible, None),
    hk(KEY_F, false, C::ForceShot, None),
    hk(KEY_HOME, true, C::CameraNormal, None),
    hk(KEY_F2, true, C::CameraView(0), None),
    hk(KEY_F3, true, C::CameraView(1), None),
    hk(KEY_F4, true, C::CameraView(2), None),
    hk(KEY_F5, true, C::CameraView(3), None),
    hk(KEY_F2 | CTRL, true, C::CameraAssign(0), None),
    hk(KEY_F3 | CTRL, true, C::CameraAssign(1), None),
    hk(KEY_F4 | CTRL, true, C::CameraAssign(2), None),
    hk(KEY_F5 | CTRL, true, C::CameraAssign(3), None),
    hk(KEY_NUMPAD2, false, C::AimLeg, None),
    hk(KEY_NUMPAD5, false, C::AimArm, None),
    hk(KEY_NUMPAD8, false, C::AimHead, None),
    hk(KEY_BACK, false, C::RemoveCommand, None),
    hk(KEY_NEXT, false, C::PowerDown, None),
    hk(KEY_PRIOR, false, C::PowerUp, None),
    hk(KEY_END, false, C::Eject, None),
    hk(KEY_SPACE, false, C::ChangeSpeed, Some(C::StopChangeSpeed)),
    hk(
        KEY_SPACE | WAYPT,
        false,
        C::ChangeSpeed,
        Some(C::StopChangeSpeed),
    ),
    hk(KEY_UP, false, C::ScrollUp, None),
    hk(KEY_DOWN, false, C::ScrollDown, None),
    hk(KEY_LEFT, false, C::ScrollLeft, None),
    hk(KEY_RIGHT, false, C::ScrollRight, None),
    hk(KEY_SUBTRACT, false, C::ZoomOut, None),
    hk(KEY_MINUS, false, C::ZoomOut, None),
    hk(KEY_ADD, false, C::ZoomIn, None),
    hk(SHIFT | KEY_EQUALS, false, C::ZoomIn, None),
    hk(KEY_LEFT | SHIFT, false, C::RotateLeft, None),
    hk(KEY_RIGHT | SHIFT, false, C::RotateRight, None),
    hk(KEY_UP | SHIFT, false, C::TiltUp, None),
    hk(KEY_DOWN | SHIFT, false, C::TiltDown, None),
    hk(KEY_HOME, false, C::CenterCamera, None),
    hk(KEY_LEFT | CTRL, false, C::RotateLightLeft, None),
    hk(KEY_RIGHT | CTRL, false, C::RotateLightRight, None),
    hk(KEY_UP | CTRL, false, C::RotateLightUp, None),
    hk(KEY_DOWN | CTRL, false, C::RotateLightDown, None),
    hk(KEY_V, true, C::VehicleCommand, None),
    hk(KEY_F9, true, C::ShowObjectives, None),
    hk(KEY_ESCAPE, true, C::TogglePause, None),
    hk(KEY_F1, true, C::ToggleHotKeys, None),
    hk(KEY_P, true, C::TogglePause, None),
    hk(KEY_TAB, true, C::SwitchTab, None),
    hk(KEY_TAB | SHIFT, true, C::ReverseSwitchTab, None),
    hk(KEY_I, true, C::InfoCommand, Some(C::InfoButtonReleased)),
    hk(KEY_N, true, C::GotoNextNavMarker, None),
    hk(KEY_MULTIPLY, true, C::SendAirstrike, None),
    hk(KEY_DIVIDE, true, C::SendSensorStrike, None),
    hk(KEY_BACKSLASH, true, C::ToggleCompass, None),
    hk(KEY_H, true, C::ToggleHoldPosition, None),
    hk(KEY_RETURN, true, C::ChatKey, None),
    hk(SHIFT | KEY_RETURN, true, C::TeamChatKey, None),
    hk(KEY_E | SHIFT, false, C::AddVisibleToSelection, None),
    hk(KEY_EQUALS, false, C::ZoomIn, None),
    hk(KEY_PAUSE, true, C::TogglePause, None),
];

/// What the dispatch loop decided this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fired {
    /// A command's function ran (once for single-click entries, every frame while held otherwise).
    Run(Command),
    /// A held entry's release function ran.
    Release(Command),
    /// Digit `n` went down: `makeForceGroup` (ctrl), `selectForceGroup(n, false)` (shift) or
    /// `selectForceGroup(n, true)` (`code/missiongui.cpp:1741-1752`).  The HUD applies the
    /// sim-owned membership in the same frame; dispatch deliberately has no group-state mirror.
    ForceGroup { group: u8, ctrl: bool, shift: bool },
    /// The waypoint modifier was released. `MissionInterfaceManager::update` executes the
    /// selected movers' queued orders first and skips the digit-group block for this frame
    /// (`code/missiongui.cpp:1708-1754`).
    WaypointsReleased,
}

/// The C++ handler name of a command, as the options screen's table (`mc2_ui::hotkeys::COMMANDS`) names it.
fn handler_name(c: Command) -> String {
    use Command::*;
    let fixed = match c {
        AttackShort => "attackShort",
        AttackMedium => "attackMedium",
        AttackLong => "attackLong",
        EnergyWeapons => "energyWeapons",
        DefaultAttack => "defaultAttack",
        Jump => "jump",
        StopJump => "stopJump",
        FireFromCurrentPos => "fireFromCurrentPos",
        StopFireFromCurrentPos => "stopFireFromCurrentPos",
        Guard => "guard",
        StopGuard => "stopGuard",
        ConserveAmmo => "conserveAmmo",
        SelectVisible => "selectVisible",
        ForceShot => "forceShot",
        CameraNormal => "cameraNormal",
        CameraView(0) => "cameraDefault",
        CameraView(1) => "cameraMaxIn",
        CameraView(2) => "cameraTight",
        CameraView(_) => "cameraFour",
        CameraAssign(n) => return format!("cameraAssign{n}"),
        AimLeg => "aimLeg",
        AimArm => "aimArm",
        AimHead => "aimHead",
        RemoveCommand => "removeCommand",
        PowerDown => "powerDown",
        PowerUp => "powerUp",
        Eject => "eject",
        ChangeSpeed => "changeSpeed",
        StopChangeSpeed => "stopChangeSpeed",
        ScrollUp => "scrollUp",
        ScrollDown => "scrollDown",
        ScrollLeft => "scrollLeft",
        ScrollRight => "scrollRight",
        ZoomOut => "zoomOut",
        ZoomIn => "zoomIn",
        RotateLeft => "rotateLeft",
        RotateRight => "rotateRight",
        TiltUp => "tiltUp",
        TiltDown => "tiltDown",
        CenterCamera => "centerCamera",
        RotateLightLeft => "rotateLightLeft",
        RotateLightRight => "rotateLightRight",
        RotateLightUp => "rotateLightUp",
        RotateLightDown => "rotateLightDown",
        VehicleCommand => "vehicleCommand",
        ShowObjectives => "showObjectives",
        TogglePause => "togglePause",
        ToggleHotKeys => "toggleHotKeys",
        SwitchTab => "switchTab",
        ReverseSwitchTab => "reverseSwitchTab",
        InfoCommand => "infoCommand",
        InfoButtonReleased => "infoButtonReleased",
        GotoNextNavMarker => "gotoNextNavMarker",
        SendAirstrike => "sendAirstrike",
        SendSensorStrike => "sendSensorStrike",
        ToggleCompass => "toggleCompass",
        ToggleHoldPosition => "toggleHoldPosition",
        ChatKey => "handleChatKey",
        TeamChatKey => "handleTeamChatKey",
        AddVisibleToSelection => "addVisibleToSelection",
    };
    fixed.to_string()
}

/// The mission's key table with the player's bindings: `keys` is the options screen's hot key per `commands[]`
/// index (`Prefs::keys`, `options.cfg` `Key<n>`). Each entry of [`COMMANDS`] takes the binding of the options
/// entry with the same default key and handler; an entry the options screen does not list keeps its key.
/// An empty `keys` is the default table. The options screen marks extended keys with `0x100`, which the
/// platform key codes already imply, so it is dropped here.
pub fn table_for(keys: &[u32]) -> Vec<HotKey> {
    let defaults = &mc2_ui::hotkeys::COMMANDS;
    let mut claimed = vec![false; defaults.len()];
    COMMANDS
        .iter()
        .map(|entry| {
            let name = handler_name(entry.function);
            // the Ctrl (waypoint) twin of an entry follows its plain entry's binding
            // (`MissionInterfaceManager::setHotKey`, `code/missiongui.cpp:5319-5348`)
            let twin = entry.key & WAYPT != 0;
            let base = entry.key & !WAYPT;
            let found = defaults.iter().enumerate().position(|(j, d)| {
                (twin || !claimed[j]) && d.handler == name && (d.key & !0x100) == base
            });
            let mut e = *entry;
            if let Some(j) = found {
                if !twin {
                    claimed[j] = true;
                }
                if let Some(&k) = keys.get(j) {
                    e.key = (k & !0x100) | (entry.key & WAYPT);
                }
            }
            e
        })
        .collect()
}

/// [`dispatch_with`] over the default table.
pub fn dispatch(input: &InputState, waypoint_key: GameKey, out: &mut Vec<Fired>) {
    dispatch_with(input, waypoint_key, COMMANDS, out);
}

/// The dispatch loop.  `waypoint_key` is `WAYPOINT_KEY` (`KEY_LCONTROL` in both mouse profiles).
pub fn dispatch_with(
    input: &InputState,
    waypoint_key: GameKey,
    table: &[HotKey],
    out: &mut Vec<Fired>,
) {
    let shift_dn = shift(input);
    let ctrl_dn = ctrl(input);
    let alt_dn = alt(input);
    let way_pt_dn = key_down(input, waypoint_key);
    let waypoints_released = input.key_status(waypoint_key) == KeyStatus::Released;

    // The original releases queued orders before testing digits, and its `bRetVal` suppresses
    // the complete digit loop in that release frame. This matters when Ctrl is released and a
    // digit is pressed in one input batch.
    if waypoints_released {
        out.push(Fired::WaypointsReleased);
    } else {
        for j in KEY_0..=KEY_9 {
            if key_down(input, j) && input.key_status(j) != KeyStatus::Held {
                out.push(Fired::ForceGroup {
                    group: (j - KEY_0) as u8,
                    ctrl: ctrl_dn,
                    shift: !ctrl_dn && shift_dn,
                });
            }
        }
    }

    for c in table {
        let key = c.key & 0xff;
        if key_down(input, key) {
            if c.key & WAYPT != 0 {
                if !way_pt_dn {
                    continue;
                }
            } else if (c.key & SHIFT != 0) != shift_dn
                || (c.key & CTRL != 0) != ctrl_dn
                || (c.key & ALT != 0) != alt_dn
            {
                continue;
            }
            if !c.single_click || input.key_status(key) != KeyStatus::Held {
                out.push(Fired::Run(c.function));
            }
        } else if input.key_status(key) == KeyStatus::Released {
            if let Some(r) = c.release {
                out.push(Fired::Release(r));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_platform::input::InputEvent;
    use mc2_platform::viewport::{Fit, Viewport};

    fn state() -> InputState {
        InputState::new(Viewport::new(800, 600, 800.0, 600.0, Fit::Stretch))
    }

    fn key(k: GameKey, pressed: bool) -> InputEvent {
        InputEvent::Key {
            key: k,
            phys_id: k as u16,
            pressed,
            repeat: false,
            text: None,
        }
    }

    fn fired(input: &InputState) -> Vec<Fired> {
        let mut v = Vec::new();
        dispatch(input, KEY_LCONTROL, &mut v);
        v
    }

    #[test]
    fn rebinding_jump_and_change_speed_moves_their_waypoint_twins() {
        let mut keys: Vec<u32> = mc2_ui::hotkeys::COMMANDS.iter().map(|c| c.key).collect();
        for (handler, to) in [("jump", KEY_F12), ("changeSpeed", KEY_F11)] {
            let at = mc2_ui::hotkeys::COMMANDS
                .iter()
                .position(|c| c.handler == handler)
                .unwrap();
            keys[at] = to;
        }
        let table = table_for(&keys);
        let run = |k: GameKey, ctrl_down: bool| {
            let mut input = state();
            if ctrl_down {
                input.handle(key(KEY_LCONTROL, true));
            }
            input.handle(key(k, true));
            let mut out = Vec::new();
            dispatch_with(&input, KEY_LCONTROL, &table, &mut out);
            out
        };
        assert!(run(KEY_F12, false).contains(&Fired::Run(Command::Jump)));
        assert!(
            run(KEY_F12, true).contains(&Fired::Run(Command::Jump)),
            "waypoint twin"
        );
        assert!(!run(KEY_J, false).contains(&Fired::Run(Command::Jump)));
        assert!(
            !run(KEY_J, true).contains(&Fired::Run(Command::Jump)),
            "old waypoint key"
        );
        assert!(run(KEY_F11, true).contains(&Fired::Run(Command::ChangeSpeed)));
        assert!(!run(KEY_SPACE, true).contains(&Fired::Run(Command::ChangeSpeed)));
    }

    #[test]
    fn held_entries_repeat_single_click_entries_do_not() {
        let mut input = state();
        input.handle(key(KEY_LEFT, true));
        assert_eq!(fired(&input), vec![Fired::Run(Command::ScrollLeft)]);
        input.end_frame();
        assert_eq!(fired(&input), vec![Fired::Run(Command::ScrollLeft)]);
        input.handle(key(KEY_F3, true));
        assert!(fired(&input).contains(&Fired::Run(Command::CameraView(1))));
        input.end_frame();
        assert!(!fired(&input).contains(&Fired::Run(Command::CameraView(1))));
    }

    #[test]
    fn held_order_mode_keys_repeat_until_release() {
        for (key_code, command, release) in [
            (KEY_J, Command::Jump, Command::StopJump),
            (KEY_G, Command::Guard, Command::StopGuard),
            (KEY_SPACE, Command::ChangeSpeed, Command::StopChangeSpeed),
        ] {
            let mut input = state();
            input.handle(key(key_code, true));
            assert_eq!(fired(&input), vec![Fired::Run(command)]);
            input.end_frame();
            assert_eq!(fired(&input), vec![Fired::Run(command)]);
            input.handle(key(key_code, false));
            let released = fired(&input);
            let release_count = if key_code == KEY_G { 1 } else { 2 };
            assert_eq!(released.len(), release_count);
            assert!(released
                .iter()
                .all(|fired| matches!(fired, Fired::Release(command) if *command == release)));
        }
    }

    #[test]
    fn modifiers_must_match_exactly() {
        let mut input = state();
        input.handle(key(KEY_LSHIFT, true));
        input.handle(key(KEY_LEFT, true));
        assert_eq!(fired(&input), vec![Fired::Run(Command::RotateLeft)]);
        let mut input = state();
        input.handle(key(KEY_LCONTROL, true));
        input.handle(key(KEY_F2, true));
        assert_eq!(fired(&input), vec![Fired::Run(Command::CameraAssign(0))]);
    }

    #[test]
    fn waypoint_entries_need_the_waypoint_key_only() {
        let mut input = state();
        input.handle(key(KEY_LCONTROL, true));
        input.handle(key(KEY_J, true));
        // plain J does not match (ctrl is down), J|WAYPT does
        assert_eq!(fired(&input), vec![Fired::Run(Command::Jump)]);
        input.end_frame();
        input.handle(key(KEY_J, false));
        // both entries run their release function
        assert_eq!(
            fired(&input),
            vec![
                Fired::Release(Command::StopJump),
                Fired::Release(Command::StopJump)
            ]
        );
    }

    #[test]
    fn home_runs_both_entries_and_digits_make_groups() {
        let mut input = state();
        input.handle(key(KEY_HOME, true));
        assert_eq!(
            fired(&input),
            vec![
                Fired::Run(Command::CameraNormal),
                Fired::Run(Command::CenterCamera)
            ]
        );
        let mut input = state();
        input.handle(key(KEY_LCONTROL, true));
        input.handle(key(KEY_3, true));
        assert_eq!(
            fired(&input),
            vec![Fired::ForceGroup {
                group: 3,
                ctrl: true,
                shift: false
            }]
        );
    }

    #[test]
    fn waypoint_release_suppresses_a_digit_pressed_in_the_same_frame() {
        let mut input = state();
        input.handle(key(KEY_LCONTROL, true));
        input.end_frame();
        input.handle(key(KEY_LCONTROL, false));
        input.handle(key(KEY_2, true));
        assert_eq!(fired(&input), vec![Fired::WaypointsReleased]);
    }

    #[test]
    fn every_camera_command_is_in_the_table() {
        for c in [
            Command::ScrollUp,
            Command::ZoomIn,
            Command::ZoomOut,
            Command::RotateRight,
            Command::TiltDown,
            Command::CameraView(3),
            Command::CameraAssign(3),
        ] {
            assert!(COMMANDS.iter().any(|h| h.function == c), "{c:?}");
            assert!(c.is_camera());
        }
        assert!(!Command::Jump.is_camera());
    }
}
