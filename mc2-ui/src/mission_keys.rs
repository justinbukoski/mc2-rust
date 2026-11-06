// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission hot keys, command dispatch and cursor choice: `MissionInterfaceManager` in
//! `code/missiongui.cpp`.
//!
//! * [`HotKeys`]: the `commands[]` table with its key bindings, `setHotKey`/`getHotKey`,
//!   `hotKeyIsPressed`, `saveHotKeys`/`loadHotKeys` (the `[Keyboard]` block of the FIT file) and the
//!   mouse-profile flag behind `setAOEStyle`/`setMCStyle`.
//! * [`HotKeys::dispatch`]: the key loop at the end of `MissionInterfaceManager::update`, returning
//!   the [`MissionCommand`]s that ran, the cursor they set, the waypoint release and the force-group
//!   actions. Nothing is executed here.
//! * [`HotKeyList`]: the rebinding rules of `OptionsHotKeys::update` (`code/optionsarea.cpp`).
//! * [`choose_cursor`]: `makeTargetCursor` / `makeNoTargetCursor` and their helpers, as a pure
//!   function of a plain [`CursorContext`] the app fills in. The camera-derived rotation
//!   (`calcRotation`) and the sim queries (`canSalvage`, `canJumpToWPos`, ...) are inputs.
//!
//! Key values are the C++ `gosEnum_KeyIndex` values with the modifier bits of [`crate::hotkeys`].
//! The extended-key flag `0x100` is part of the value, as in the table, so the frame must report
//! extended keys under the same index. Ported from `code/missiongui.cpp` and `code/optionsarea.cpp`;
//! debug entries are [`MissionCommand::Unavailable`].

use std::collections::BTreeMap;

use mc2_formats::ini::{FitIni, FitWriter};
use mc2_platform::input::KeyStatus;
use mc2_platform::keys::{
    self, KEY_0, KEY_9, KEY_LALT, KEY_LCONTROL, KEY_LSHIFT, KEY_RALT, KEY_RCONTROL, KEY_RSHIFT,
};

use crate::hotkeys::{self, ALT, CTRL, SHIFT, WAYPT};

/// `MAX_COMMAND`.
pub const MAX_COMMAND: usize = hotkeys::MAX_COMMAND;
/// `mState_NUMMOUSESTATES`: `UserInput::setMouseCursor` ignores states outside `0..243`.
const NUM_MOUSE_STATES: i32 = 243;
/// The chat entries, `commands[100..102]` (`missiongui.cpp:1700-1716`).
const CHAT_FIRST: usize = 100;
const CHAT_LAST: usize = 102;

/// `IDS`-free names of the mouse states (`mclib/userinput.h:33-79`).
pub mod state {
    pub const MOVE: u32 = 0;
    pub const MOVE_LOS: u32 = 1;
    pub const RUN: u32 = 2;
    pub const RUN_LOS: u32 = 3;
    pub const JUMP: u32 = 4;
    pub const JUMP_LOS: u32 = 5;
    pub const WALKWAYPT: u32 = 6;
    pub const WALKWAYPT_LOS: u32 = 7;
    pub const RUNWAYPT: u32 = 8;
    pub const RUNWAYPT_LOS: u32 = 9;
    pub const JUMPWAYPT: u32 = 10;
    pub const JUMPWAYPT_LOS: u32 = 11;
    pub const LINK: u32 = 12;
    pub const GENERIC_ATTACK: u32 = 13;
    pub const ATTACK_LOS: u32 = 14;
    pub const CURPOS_ATTACK: u32 = 15;
    pub const CURPOS_ATTACK_LOS: u32 = 16;
    pub const ENERGY_WEAPONS: u32 = 17;
    pub const ENERGY_WEAPONS_LOS: u32 = 18;
    pub const AIMED_ATTACK: u32 = 19;
    pub const AIMED_ATTACK_LOS: u32 = 20;
    pub const NORMAL: u32 = 21;
    pub const INFO: u32 = 22;
    pub const LAYMINES: u32 = 23;
    pub const ROTATE_CAMERA: u32 = 24;
    pub const UNCERTAIN_AIRSTRIKE: u32 = 25;
    pub const AIRSTRIKE: u32 = 26;
    pub const SENSORSTRIKE: u32 = 27;
    pub const VEHICLE: u32 = 28;
    pub const CANTVEHICLE: u32 = 29;
    pub const DONT: u32 = 30;
    pub const REPAIR: u32 = 31;
    pub const XREPAIR: u32 = 32;
    pub const EJECT: u32 = 33;
    pub const XEJECT: u32 = 34;
    pub const CAPTURE: u32 = 35;
    pub const XCAPTURE: u32 = 36;
    pub const SALVAGE: u32 = 37;
    pub const XSALVAGE: u32 = 38;
    pub const GUARD: u32 = 39;
    pub const GUARD_LOS: u32 = 39;
    pub const GUARDTOWER: u32 = 40;
    pub const GUARDTOWER_LOS: u32 = 41;
    pub const XMINES: u32 = 42;
    pub const LOGISTICS: u32 = 43;
    pub const TUTORIALS: u32 = 44;
    pub const LONGRNG_ATTACK: u32 = 45;
    pub const SHRTRNG_ATTACK: u32 = 46;
    pub const MEDRNG_ATTACK: u32 = 47;
    pub const WALK1: u32 = 48;
    pub const WALKLOS1: u32 = 80;
    pub const RUN1: u32 = 112;
    pub const RUNLOS1: u32 = 144;
    pub const JUMP1: u32 = 176;
    pub const JUMPLOS1: u32 = 208;
    pub const LONGRNG_LOS: u32 = 240;
    pub const MEDRNG_LOS: u32 = 241;
    pub const SHRTRNG_LOS: u32 = 242;
}

use state::*;

/// The two control styles. The key table is the same in both (`missiongui.cpp:197` has no style
/// input); the style is the flag `useLeftRightMouseProfile`, read by the mouse code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlStyle {
    /// `useLeftRightMouseProfile` true (`mechcmd2.cpp:106`).
    Aoe,
    Mc,
}

/// What a table entry does (the `commands[]` handler, `missiongui.cpp:197-307`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionCommand {
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
    /// `cameraDefault`, `cameraMaxIn`, `cameraTight`, `cameraFour`: `eye->setCameraView(0..3)`.
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
    /// A debug handler (`#ifndef FINAL`) or one not ported here. The name is the C++ handler.
    Unavailable(&'static str),
}

/// Maps a `commands[]` handler name to its command. Any name not listed is a debug handler and
/// becomes [`MissionCommand::Unavailable`]; a test checks that only ALT-modified entries do.
pub fn command_for_handler(name: &'static str) -> MissionCommand {
    use MissionCommand as M;
    match name {
        "attackShort" => M::AttackShort,
        "attackMedium" => M::AttackMedium,
        "attackLong" => M::AttackLong,
        "energyWeapons" => M::EnergyWeapons,
        "defaultAttack" => M::DefaultAttack,
        "jump" => M::Jump,
        "stopJump" => M::StopJump,
        "fireFromCurrentPos" => M::FireFromCurrentPos,
        "stopFireFromCurrentPos" => M::StopFireFromCurrentPos,
        "guard" => M::Guard,
        "stopGuard" => M::StopGuard,
        "conserveAmmo" => M::ConserveAmmo,
        "selectVisible" => M::SelectVisible,
        "forceShot" => M::ForceShot,
        "cameraNormal" => M::CameraNormal,
        "cameraDefault" => M::CameraView(0),
        "cameraMaxIn" => M::CameraView(1),
        "cameraTight" => M::CameraView(2),
        "cameraFour" => M::CameraView(3),
        "cameraAssign0" => M::CameraAssign(0),
        "cameraAssign1" => M::CameraAssign(1),
        "cameraAssign2" => M::CameraAssign(2),
        "cameraAssign3" => M::CameraAssign(3),
        "aimLeg" => M::AimLeg,
        "aimArm" => M::AimArm,
        "aimHead" => M::AimHead,
        "removeCommand" => M::RemoveCommand,
        "powerDown" => M::PowerDown,
        "powerUp" => M::PowerUp,
        "eject" => M::Eject,
        "changeSpeed" => M::ChangeSpeed,
        "stopChangeSpeed" => M::StopChangeSpeed,
        "scrollUp" => M::ScrollUp,
        "scrollDown" => M::ScrollDown,
        "scrollLeft" => M::ScrollLeft,
        "scrollRight" => M::ScrollRight,
        "zoomOut" => M::ZoomOut,
        "zoomIn" => M::ZoomIn,
        "rotateLeft" => M::RotateLeft,
        "rotateRight" => M::RotateRight,
        "tiltUp" => M::TiltUp,
        "tiltDown" => M::TiltDown,
        "centerCamera" => M::CenterCamera,
        "rotateLightLeft" => M::RotateLightLeft,
        "rotateLightRight" => M::RotateLightRight,
        "rotateLightUp" => M::RotateLightUp,
        "rotateLightDown" => M::RotateLightDown,
        "vehicleCommand" => M::VehicleCommand,
        "showObjectives" => M::ShowObjectives,
        "togglePause" => M::TogglePause,
        "toggleHotKeys" => M::ToggleHotKeys,
        "switchTab" => M::SwitchTab,
        "reverseSwitchTab" => M::ReverseSwitchTab,
        "infoCommand" => M::InfoCommand,
        "infoButtonReleased" => M::InfoButtonReleased,
        "gotoNextNavMarker" => M::GotoNextNavMarker,
        "sendAirstrike" => M::SendAirstrike,
        "sendSensorStrike" => M::SendSensorStrike,
        "toggleCompass" => M::ToggleCompass,
        "toggleHoldPosition" => M::ToggleHoldPosition,
        "handleChatKey" => M::ChatKey,
        "handleTeamChatKey" => M::TeamChatKey,
        "addVisibleToSelection" => M::AddVisibleToSelection,
        other => M::Unavailable(other),
    }
}

/// One `commands[]` entry, with its C++ handler mapped to a [`MissionCommand`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    /// Key index plus modifier bits (`SHIFT`, `CTRL`, `ALT`, `WAYPT`).
    pub key: u32,
    /// `cursor` and `cursorLOS`: mouse states, `-1` for none.
    pub cursor: i32,
    pub cursor_los: i32,
    /// `singleClick`: fires once per press rather than every frame while held.
    pub single_click: bool,
    /// `hotKeyDescriptionText`: string id listed in the options screen, `-1` if not listed.
    pub description: i32,
    pub command: MissionCommand,
    /// The release handler, if any.
    pub release: Option<MissionCommand>,
}

/// The frame's key states, keyed by the table's key values (`gos_GetKeyStatus`). A key that is not
/// in the frame is `Free`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyFrame {
    statuses: BTreeMap<u32, KeyStatus>,
}

impl KeyFrame {
    /// Adds a key state; a builder for tests and the app's adapter.
    pub fn with(mut self, key: u32, status: KeyStatus) -> Self {
        self.set(key, status);
        self
    }

    pub fn set(&mut self, key: u32, status: KeyStatus) {
        self.statuses.insert(key, status);
    }

    pub fn status(&self, key: u32) -> KeyStatus {
        self.statuses.get(&key).copied().unwrap_or(KeyStatus::Free)
    }

    /// `UserInput::getKeyDown`: pressed or held.
    pub fn key_down(&self, key: u32) -> bool {
        matches!(self.status(key), KeyStatus::Held | KeyStatus::Pressed)
    }

    /// `UserInput::shift()`: either shift.
    pub fn shift(&self) -> bool {
        self.key_down(KEY_LSHIFT) || self.key_down(KEY_RSHIFT)
    }

    /// `UserInput::ctrl()`: either control.
    pub fn ctrl(&self) -> bool {
        self.key_down(KEY_LCONTROL) || self.key_down(KEY_RCONTROL)
    }

    /// `UserInput::alt()`: the dedicated alt indices.
    pub fn alt(&self) -> bool {
        self.key_down(KEY_LALT) || self.key_down(KEY_RALT)
    }
}

/// The per-frame inputs of the dispatch loop that are not key states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DispatchContext {
    /// `controlGui.updateChat()`: only the chat entries are scanned.
    pub chatting: bool,
    /// `bLOS`: the line-of-sight cursor is used.
    pub line_of_sight: bool,
}

/// A command the loop ran (`(this->*commands[i].function)()` or its release function).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fired {
    pub command: MissionCommand,
    /// The table index, so the app can tell apart entries that share a command.
    pub index: usize,
    /// Whether the entry is a waypoint (`WAYPT`) entry.
    pub waypoint: bool,
    /// True when this is the release function of a held entry.
    pub release: bool,
}

/// A force-group action for a digit key (`missiongui.cpp:1740-1753`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForceGroup {
    /// `makeForceGroup(n)`: ctrl and a digit.
    Make(u8),
    /// `selectForceGroup(n, deselect)`: `exclusive` is `deselect = true` (plain digit, clears the
    /// selection first), otherwise the group is toggled (shift and a digit).
    Select { group: u8, exclusive: bool },
}

/// What one pass of the dispatch loop decided.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dispatch {
    /// The commands run, in table order.
    pub commands: Vec<Fired>,
    /// The mouse cursor the loop set last (`setMouseCursor`), `None` if it set none. The loop sets
    /// the cursor before the single-click test, so a held single-click entry still sets it.
    pub cursor: Option<u32>,
    /// The waypoint key was released: the selected movers run their queued waypoints
    /// (`missiongui.cpp:1700-1718`).
    pub waypoints_done: bool,
    pub force_groups: Vec<ForceGroup>,
}

/// `setMouseCursor`: a state outside `0..243` is ignored.
fn cursor_value(state: i32) -> Option<u32> {
    (0..NUM_MOUSE_STATES)
        .contains(&state)
        .then_some(state as u32)
}

/// The mission hot keys: `MissionInterfaceManager::commands[]`, its waypoint key and the mouse
/// profile flag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotKeys {
    bindings: Vec<Binding>,
    /// `OldKeys`: the default key of each entry, for "reset to defaults" and conflicts.
    defaults: Vec<u32>,
    /// `WAYPOINT_KEY`.
    waypoint_key: u32,
    /// `useLeftRightMouseProfile`.
    aoe_profile: bool,
}

impl HotKeys {
    /// The default table (`missiongui.cpp:197-307`) with the waypoint key `KEY_LCONTROL`
    /// (`missiongui.cpp:849-850`). The style only sets the profile flag.
    pub fn new(style: ControlStyle) -> Self {
        let bindings: Vec<Binding> = hotkeys::COMMANDS
            .iter()
            .map(|c| Binding {
                key: c.key,
                cursor: c.cursor,
                cursor_los: c.cursor_los,
                single_click: c.single_click,
                description: c.description,
                command: command_for_handler(c.handler),
                release: c.release_handler.map(command_for_handler),
            })
            .collect();
        let defaults = bindings.iter().map(|b| b.key).collect();
        Self {
            bindings,
            defaults,
            waypoint_key: KEY_LCONTROL,
            aoe_profile: style == ControlStyle::Aoe,
        }
    }

    pub fn bindings(&self) -> &[Binding] {
        &self.bindings
    }

    pub fn binding(&self, index: usize) -> &Binding {
        &self.bindings[index]
    }

    pub fn default_key(&self, index: usize) -> u32 {
        self.defaults[index]
    }

    pub fn waypoint_key(&self) -> u32 {
        self.waypoint_key
    }

    /// `setWayPointKey`.
    pub fn set_waypoint_key(&mut self, key: u32) {
        self.waypoint_key = key;
    }

    /// `isAOEStyle`.
    pub fn is_aoe(&self) -> bool {
        self.aoe_profile
    }

    /// `isMCStyle`.
    pub fn is_mc(&self) -> bool {
        !self.aoe_profile
    }

    /// `setAOEStyle` / `setMCStyle`.
    pub fn set_style(&mut self, style: ControlStyle) {
        self.aoe_profile = style == ControlStyle::Aoe;
    }

    /// `setHotKey` (`missiongui.cpp:5319-5348`). The modifiers are ORed into the key. A WAYPT entry
    /// keeps its WAYPT bit. For another entry with no modifiers, its waypoint twin (the same key
    /// with WAYPT) follows the new key.
    pub fn set_hot_key(&mut self, index: usize, key: u32, shift: bool, ctrl: bool, alt: bool) {
        let old = self.bindings[index].key;
        let mut new = key;
        if shift {
            new |= SHIFT;
        }
        if ctrl {
            new |= CTRL;
        }
        if alt {
            new |= ALT;
        }
        if old & WAYPT != 0 {
            new |= WAYPT;
        } else {
            for b in &mut self.bindings {
                if (b.key & 0x0000_ffff) == old && b.key & WAYPT != 0 {
                    b.key = new | WAYPT;
                }
            }
        }
        self.bindings[index].key = new;
    }

    /// `getHotKey`: the key without modifiers, then whether shift, ctrl and alt are in it.
    pub fn get_hot_key(&self, index: usize) -> (u32, bool, bool, bool) {
        let key = self.bindings[index].key;
        (
            key & 0x0000_ffff,
            key & SHIFT != 0,
            key & CTRL != 0,
            key & ALT != 0,
        )
    }

    /// `hotKeyIsPressed` (`missiongui.cpp:5408-5439`): the key is pressed or held, and shift, ctrl
    /// and alt match the binding's exactly.
    pub fn is_pressed(&self, index: usize, frame: &KeyFrame) -> bool {
        let (key, shift, ctrl, alt) = self.get_hot_key(index);
        let down = matches!(
            frame.status(key & 0xff),
            KeyStatus::Pressed | KeyStatus::Held
        );
        down && frame.shift() == shift && frame.ctrl() == ctrl && frame.alt() == alt
    }

    /// The command loop at the end of `MissionInterfaceManager::update` (`missiongui.cpp:1700-1795`).
    ///
    /// * Chat mode runs only the chat entries and skips force groups (`bRetVal` starts at 1).
    /// * A released waypoint key sets `waypoints_done` and skips force groups.
    /// * Digits 0-9 that are not held make force-group actions by modifier.
    /// * A WAYPT entry needs the waypoint key and ignores shift, ctrl and alt. Any other entry
    ///   needs its exact modifiers.
    /// * A single-click entry runs when its key is not held. A held entry runs every frame.
    /// * When an entry's key is not down and its release status is `Released`, its release runs.
    pub fn dispatch(&self, frame: &KeyFrame, ctx: DispatchContext) -> Dispatch {
        let mut out = Dispatch::default();
        let shift_dn = frame.shift();
        let ctrl_dn = frame.ctrl();
        let alt_dn = frame.alt();
        let way_pt_dn = frame.key_down(self.waypoint_key);

        if frame.status(self.waypoint_key) == KeyStatus::Released {
            out.waypoints_done = true;
        }

        if !ctx.chatting && !out.waypoints_done {
            for j in KEY_0..=KEY_9 {
                if frame.key_down(j) && frame.status(j) != KeyStatus::Held {
                    let group = (j - KEY_0) as u8;
                    if ctrl_dn {
                        out.force_groups.push(ForceGroup::Make(group));
                    } else {
                        out.force_groups.push(ForceGroup::Select {
                            group,
                            exclusive: !shift_dn,
                        });
                    }
                }
            }
        }

        let range = if ctx.chatting {
            CHAT_FIRST..CHAT_LAST
        } else {
            0..MAX_COMMAND
        };
        for i in range {
            let b = &self.bindings[i];
            let key = b.key;
            let waypoint = key & WAYPT != 0;
            if frame.key_down(key & 0xff) {
                if waypoint {
                    if !way_pt_dn {
                        continue;
                    }
                } else if (key & SHIFT != 0) != shift_dn
                    || (key & CTRL != 0) != ctrl_dn
                    || (key & ALT != 0) != alt_dn
                {
                    continue;
                }
                let state = if ctx.line_of_sight {
                    b.cursor_los
                } else {
                    b.cursor
                };
                if let Some(cursor) = cursor_value(state) {
                    out.cursor = Some(cursor);
                }
                if !b.single_click || frame.status(key & 0x000f_ffff) != KeyStatus::Held {
                    out.commands.push(Fired {
                        command: b.command,
                        index: i,
                        waypoint,
                        release: false,
                    });
                }
            } else if frame.status(key & 0x000f_ffff) == KeyStatus::Released {
                if let Some(release) = b.release {
                    out.commands.push(Fired {
                        command: release,
                        index: i,
                        waypoint,
                        release: true,
                    });
                }
            }
        }
        out
    }

    /// `saveHotKeys`: the `[Keyboard]` block of the FIT file, as the C++ writes it.
    pub fn save(&self) -> Vec<u8> {
        let mut w = FitWriter::new();
        w.block("Keyboard");
        for (i, b) in self.bindings.iter().enumerate() {
            w.write_i64(&format!("Key{i}"), i64::from(b.key));
        }
        w.write_i64("WayPointKey", i64::from(self.waypoint_key));
        w.write_bool("UseLeftRightMouseProfile", self.aoe_profile);
        w.finish()
    }

    /// `loadHotKeys`: reads the `[Keyboard]` block. Missing keys keep their value, as
    /// `readIdLong` leaves them. A file without the block changes nothing and is an error.
    pub fn load(&mut self, data: Vec<u8>) -> Result<(), LoadError> {
        let mut file = FitIni::from_bytes(data).map_err(|e| LoadError::Malformed(e.to_string()))?;
        if !file.has_block("Keyboard") {
            return Err(LoadError::NoKeyboardBlock);
        }
        file.seek_block("Keyboard")
            .map_err(|e| LoadError::Malformed(e.to_string()))?;
        for (i, b) in self.bindings.iter_mut().enumerate() {
            if let Ok(v) = file.read_i64(&format!("Key{i}")) {
                b.key = v as u32;
            }
        }
        if let Ok(v) = file.read_i64("WayPointKey") {
            self.waypoint_key = v as u32;
        }
        if let Ok(v) = file.read_bool("UseLeftRightMouseProfile") {
            self.aoe_profile = v;
        }
        Ok(())
    }
}

/// Why [`HotKeys::load`] left the table alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// `seekBlock("Keyboard")` failed (`missiongui.cpp:5288-5317` returns -1).
    NoKeyboardBlock,
    Malformed(String),
}

/// One listed command of the options list: a [`Binding`] index with its current and default key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotKeyItem {
    pub index: usize,
    pub hot_key: u32,
    pub default: u32,
}

/// The options screen's hot-key list (`OptionsHotKeys`, `code/optionsarea.cpp:1288-1302`). Only
/// commands with a description are listed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HotKeyList {
    items: Vec<HotKeyItem>,
}

impl HotKeyList {
    /// The list with the current keys.
    pub fn from_bindings(hk: &HotKeys) -> Self {
        Self::build(hk, |i| hk.binding(i).key)
    }

    /// The list with the default keys (`useOld`, `optionsarea.cpp:1288-1296`).
    pub fn from_defaults(hk: &HotKeys) -> Self {
        Self::build(hk, |i| hk.default_key(i))
    }

    fn build(hk: &HotKeys, key_of: impl Fn(usize) -> u32) -> Self {
        let items = (0..MAX_COMMAND)
            .filter(|&i| hk.binding(i).description != -1)
            .map(|i| HotKeyItem {
                index: i,
                hot_key: key_of(i),
                default: hk.default_key(i),
            })
            .collect();
        Self { items }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The list position of table entry `index`, if it is listed.
    pub fn position(&self, index: usize) -> Option<usize> {
        self.items.iter().position(|it| it.index == index)
    }

    pub fn hot_key(&self, pos: usize) -> u32 {
        self.items[pos].hot_key
    }

    /// The other positions already using `key` (the conflict dialog of `optionsarea.cpp:1135-1145`).
    pub fn conflicts(&self, pos: usize, key: u32) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|&(i, it)| i != pos && it.hot_key == key)
            .map(|(i, _)| i)
            .collect()
    }

    /// Sets position `pos` to `key` once the user has accepted any conflict. Each command that
    /// holds `key` takes its own default, if no listed command uses that default now. Otherwise
    /// it takes the key `pos` had (`optionsarea.cpp:1061-1101`).
    pub fn set(&mut self, pos: usize, key: u32) {
        for i in 0..self.items.len() {
            if i == pos || self.items[i].hot_key != key {
                continue;
            }
            let default = self.items[i].default;
            let default_free = !self.items.iter().any(|it| it.hot_key == default);
            let new_key = if default_free {
                default
            } else {
                self.items[pos].hot_key
            };
            self.items[i].hot_key = new_key;
        }
        self.items[pos].hot_key = key;
    }

    /// Applies the list to the table with [`HotKeys::set_hot_key`] (the apply step after the
    /// options screen is accepted).
    pub fn apply(&self, hk: &mut HotKeys) {
        for it in &self.items {
            let k = it.hot_key;
            hk.set_hot_key(
                it.index,
                k & 0x0000_ffff,
                k & SHIFT != 0,
                k & CTRL != 0,
                k & ALT != 0,
            );
        }
    }
}

/// One row of the hot-key help overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpRow {
    pub key: String,
    pub description: String,
}

/// `KeyboardRef::reseed` (`code/keyboardref.cpp:110-160`): one row per listed command, in table
/// order. The key name is followed by ` + <modifier>` for each modifier, so `CTRL` reads
/// `Cursor Left + Control` (the options list in `code/optionsarea.cpp` puts them in front).
pub fn help_rows(hk: &HotKeys, strings: &crate::strings::StringTable) -> Vec<HelpRow> {
    hk.bindings()
        .iter()
        .filter(|b| b.description != -1)
        .map(|b| {
            let mut key = keys::key_name(b.key & 0xff).to_string();
            for (bit, id) in [
                (SHIFT, hotkeys::IDS_SHIFT),
                (CTRL, hotkeys::IDS_CONTROL),
                (ALT, hotkeys::IDS_ALT),
            ] {
                if b.key & bit != 0 {
                    key.push_str(" + ");
                    key.push_str(strings.load(id));
                }
            }
            HelpRow {
                key,
                description: strings.load(b.description as u32).to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Cursor choice

/// `FIRERANGE_*`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FireRange {
    Short,
    Medium,
    Long,
    #[default]
    Optimal,
    Current,
}

/// The object classes the cursor code branches on (`missiongui.cpp:3851-3915`). Any other class is
/// [`ObjectClass::Other`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectClass {
    Mech,
    GroundVehicle,
    Elemental,
    Building,
    TreeBuilding,
    Turret,
    Gate,
    Tree,
    TerrainObject,
    Bridge,
    Debris,
    #[default]
    Other,
}

/// The target's team against the home team.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Relation {
    /// The object has no team.
    #[default]
    NoTeam,
    Friendly,
    Neutral,
    Enemy,
}

/// The object under the cursor, reduced to the facts the cursor code reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Target {
    pub class: ObjectClass,
    pub relation: Relation,
    pub disabled: bool,
    pub is_mover: bool,
    /// `isCaptureable(home)`.
    pub captureable: bool,
    /// `canSalvage(target)`.
    pub can_salvage: bool,
    /// `canRepair(target)`.
    pub can_repair: bool,
    /// `OBJECT_FLAG_CANREFIT && OBJECT_FLAG_MECHBAY`.
    pub refit_bay: bool,
    /// `canRepairBay(target)`.
    pub can_repair_bay: bool,
    /// `getObjectType()->getSubType() == BUILDING_SUBTYPE_LANDBRIDGE`.
    pub landbridge: bool,
    /// `GameMap->getPassable` at the cursor, for a landbridge (`missiongui.cpp:3870-3878`).
    pub landbridge_passable: bool,
}

/// One selected mover of the home commander (`Team::home` roster, `missiongui.cpp:3792-3814`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelectedMover {
    /// `BATTLEMECH` on `MOVETYPE_GROUND`.
    pub mech_on_ground: bool,
    /// `target->getCaptureBlocker(mover)`.
    pub capture_blocked: bool,
}

/// The `ControlGui` state the cursor reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controls {
    pub info_select: bool,
    pub adding_vehicle: bool,
    pub adding_salvage: bool,
    pub adding_airstrike: bool,
    /// `getButton(SENSOR_PROBE)->state == PRESSED`.
    pub sensor_probe_pressed: bool,
    /// `isButtonPressed(SENSOR_PROBE)`.
    pub sensor_probe_held: bool,
    /// `isOverTacMap()`.
    pub over_tac_map: bool,
    pub salvage: bool,
    pub repair: bool,
    pub mines: bool,
    pub guard_tower: bool,
    pub guard: bool,
    pub jump: bool,
    pub run: bool,
}

/// Everything the cursor choice reads, filled in by the app. Sim and camera queries are inputs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorContext {
    /// `lineOfSight`.
    pub line_of_sight: bool,
    /// `userInput->getKeyDown(WAYPOINT_KEY)`.
    pub waypoint_key_down: bool,
    /// `ctrlDn`, passed to the no-target cursor.
    pub ctrl_down: bool,
    /// `bGui`: the mouse is over a GUI element.
    pub gui: bool,
    /// `passable` at the cursor.
    pub passable: bool,
    /// `moverCount` and `nonMoverCount`: the selection size.
    pub movers: u32,
    pub non_movers: u32,
    /// `calcRotation()`: 0..32, from the camera and the selected movers (not ported here).
    pub rotation: u32,
    /// `KEY_HELD` on the force-fire key.
    pub force_fire_held: bool,
    /// `paintingVtol[commander]`.
    pub painting_vtol: bool,
    /// `selectionIsHelicopters()`.
    pub selection_helicopters: bool,
    /// `bEnergyWeapons`.
    pub energy_weapons: bool,
    /// `controlGui.getCurrentRange()`.
    pub range: FireRange,
    /// `controlGui.getFireFromCurrentPos()`.
    pub fire_from_current_pos: bool,
    pub controls: Controls,
    /// `canAddVehicle(wPos)`.
    pub can_add_vehicle: bool,
    /// `canRecover(wPos)`.
    pub can_recover: bool,
    /// `canJumpToWPos()`.
    pub can_jump_to_wpos: bool,
    /// The home commander's selected movers, in roster order.
    pub selected: Vec<SelectedMover>,
    /// The object under the cursor, if any.
    pub target: Option<Target>,
}

/// The cursor and the side effects the choice asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorChoice {
    /// A mouse state (see [`state`]).
    pub cursor: u32,
    /// `target->setTargeted(true)`: the object is highlighted.
    pub targeted: bool,
    /// The C++ sets `target = NULL` (a bridge, or a landbridge without fire).
    pub target_cleared: bool,
}

impl CursorChoice {
    fn of(cursor: u32) -> Self {
        Self {
            cursor,
            targeted: false,
            target_cleared: false,
        }
    }
}

/// `makeMoveCursor`: `mState_WALK1 + 32 * bLOS + rotation` (`missiongui.cpp:5544-5552`).
pub fn move_cursor(line_of_sight: bool, rotation: u32) -> u32 {
    debug_assert!(rotation < 32);
    WALK1 + 32 * u32::from(line_of_sight) + rotation
}

/// `makeJumpCursor`: `mState_JUMP1 + 32 * bLOS + rotation` (`missiongui.cpp:5555-5563`).
pub fn jump_cursor(line_of_sight: bool, rotation: u32) -> u32 {
    debug_assert!(rotation < 32);
    JUMP1 + 32 * u32::from(line_of_sight) + rotation
}

/// `makeRunCursor`: `mState_RUN1 + 32 * bLOS + rotation` (`missiongui.cpp:5567-5575`).
pub fn run_cursor(line_of_sight: bool, rotation: u32) -> u32 {
    debug_assert!(rotation < 32);
    RUN1 + 32 * u32::from(line_of_sight) + rotation
}

/// `makeRangeCursor` (`missiongui.cpp:3940-3971`).
pub fn range_cursor(ctx: &CursorContext) -> u32 {
    range_cursor_at(ctx, ctx.line_of_sight)
}

fn range_cursor_at(ctx: &CursorContext, los: bool) -> u32 {
    if ctx.energy_weapons {
        return if los {
            ENERGY_WEAPONS_LOS
        } else {
            ENERGY_WEAPONS
        };
    }
    let cursor = match ctx.range {
        FireRange::Short => {
            if los {
                SHRTRNG_LOS
            } else {
                SHRTRNG_ATTACK
            }
        }
        FireRange::Medium => {
            if los {
                MEDRNG_LOS
            } else {
                MEDRNG_ATTACK
            }
        }
        FireRange::Long => {
            if los {
                LONGRNG_LOS
            } else {
                LONGRNG_ATTACK
            }
        }
        FireRange::Optimal => {
            if los {
                ATTACK_LOS
            } else {
                GENERIC_ATTACK
            }
        }
        FireRange::Current => {
            if los {
                CURPOS_ATTACK_LOS
            } else {
                CURPOS_ATTACK
            }
        }
    };
    // 3964-3966: firing from the current position overrides the range.
    if ctx.fire_from_current_pos {
        return if los {
            CURPOS_ATTACK_LOS
        } else {
            CURPOS_ATTACK
        };
    }
    cursor
}

/// `makeTargetCursor` (`missiongui.cpp:3700-3938`): the cursor over an object.
fn target_cursor(ctx: &CursorContext, t: &Target) -> CursorChoice {
    let los = ctx.line_of_sight;
    let controls = &ctx.controls;
    let mut cursor = range_cursor(ctx);
    if ctx.movers == 0 && ctx.non_movers == 0 {
        cursor = NORMAL;
    }
    // 3711-3713: the waypoint key short-circuits.
    if ctx.waypoint_key_down {
        return CursorChoice::of(DONT);
    }
    // 3716-3725: selecting an info object.
    if controls.info_select {
        return CursorChoice::of(if t.is_mover { INFO } else { DONT });
    }
    // 3729-3733: adding a vehicle or salvage or an airstrike (unless painting a vtol).
    if controls.adding_vehicle && !ctx.painting_vtol {
        return CursorChoice::of(DONT);
    }
    if controls.adding_salvage && !ctx.painting_vtol {
        return salvage_choice(t);
    }
    if controls.adding_airstrike && !ctx.painting_vtol {
        let cursor = match (controls.sensor_probe_pressed, los) {
            (true, true) => SENSORSTRIKE,
            (true, false) => UNCERTAIN_AIRSTRIKE,
            (false, true) => AIRSTRIKE,
            (false, false) => UNCERTAIN_AIRSTRIKE,
        };
        return CursorChoice::of(cursor);
    }
    // 3751-3763: the control's salvage and repair modes.
    if controls.salvage {
        return salvage_choice(t);
    }
    if controls.repair {
        return CursorChoice {
            cursor: if t.can_repair { REPAIR } else { XREPAIR },
            targeted: true,
            target_cleared: false,
        };
    }
    if controls.mines {
        return CursorChoice::of(DONT);
    }
    if controls.guard_tower {
        return CursorChoice::of(if los { GUARDTOWER_LOS } else { GUARDTOWER });
    }
    // 3792-3816: a capturable object with selected movers.
    if t.captureable && ctx.movers > 0 {
        for m in &ctx.selected {
            if m.mech_on_ground {
                if m.capture_blocked {
                    cursor = XCAPTURE;
                    break;
                }
                cursor = CAPTURE;
            } else {
                cursor = XCAPTURE;
            }
        }
    }

    // 3818-3915: by object class.
    let mut targeted = false;
    match t.class {
        ObjectClass::Mech | ObjectClass::GroundVehicle | ObjectClass::Elemental => {
            // 3826-3847: an enemy keeps the cursor unless disabled; any other side gets NORMAL.
            targeted = true;
            if t.relation != Relation::Enemy || t.disabled {
                cursor = NORMAL;
            }
        }
        ObjectClass::Building
        | ObjectClass::TreeBuilding
        | ObjectClass::Turret
        | ObjectClass::Gate
        | ObjectClass::Tree
        | ObjectClass::TerrainObject => {
            // 3858-3880: a landbridge with no fire goes to the no-target cursor on its terrain.
            if t.landbridge && !(ctx.force_fire_held || ctx.fire_from_current_pos) {
                return CursorChoice {
                    target_cleared: true,
                    ..no_target_at(ctx, t.landbridge_passable, false, false, false)
                };
            }
            targeted = true;
            if t.relation == Relation::Friendly && t.refit_bay {
                // 3886-3896: a friendly refit bay.
                cursor = if t.can_repair_bay { REPAIR } else { XREPAIR };
            } else if t.relation == Relation::Friendly
                && !controls.guard
                && !ctx.force_fire_held
                && ctx.range == FireRange::Optimal
            {
                // 3901-3904: a friendly building at optimal range is a no-target click.
                return CursorChoice {
                    targeted,
                    ..no_target_at(ctx, false, los, false, false)
                };
            }
        }
        ObjectClass::Bridge => {
            // 3909-3912: a bridge clears the target.
            return CursorChoice {
                target_cleared: true,
                ..no_target_at(ctx, false, los, false, false)
            };
        }
        ObjectClass::Debris | ObjectClass::Other => {}
    }

    // 3921-3933: guard and the forced or limited ranges.
    if controls.guard {
        cursor = GUARD;
        targeted = true;
    } else if ctx.force_fire_held
        || (ctx.range != FireRange::Optimal && ctx.range != FireRange::Current)
    {
        cursor = range_cursor(ctx);
    }
    CursorChoice {
        cursor,
        targeted,
        target_cleared: false,
    }
}

/// The salvage branch of `makeTargetCursor` (`missiongui.cpp:3735-3741, 3755-3766`).
fn salvage_choice(t: &Target) -> CursorChoice {
    if t.can_salvage {
        CursorChoice {
            cursor: SALVAGE,
            targeted: true,
            target_cleared: false,
        }
    } else {
        CursorChoice::of(XSALVAGE)
    }
}

/// `makeNoTargetCursor` (`missiongui.cpp:3974-4130`) for the cursor's own position.
fn no_target_cursor(ctx: &CursorContext) -> CursorChoice {
    no_target_at(ctx, ctx.passable, ctx.line_of_sight, ctx.ctrl_down, ctx.gui)
}

/// `makeNoTargetCursor` with the passability, line of sight, ctrl and gui the caller passes. The
/// target's landbridge and bridge branches pass their own passability and `0, 0` for ctrl and gui
/// (`missiongui.cpp:3909-3912, 3869-3880`).
fn no_target_at(
    ctx: &CursorContext,
    passable: bool,
    los: bool,
    ctrl: bool,
    gui: bool,
) -> CursorChoice {
    let c = &ctx.controls;
    let heli = ctx.selection_helicopters;
    let rot = ctx.rotation;
    let mut cursor = move_cursor(los, rot);
    if ctx.movers == 0 {
        cursor = NORMAL;
    }
    // 3987-3995: forcing fire, or firing from the current position, shows the range cursor.
    if ctx.force_fire_held || ctx.fire_from_current_pos {
        return CursorChoice::of(range_cursor_at(ctx, los));
    }
    // 3998-3999: impassable ground is DONT unless helicopters.
    if !passable && !heli {
        cursor = DONT;
    }
    // 4001-4004: selecting an info object.
    if c.info_select {
        return CursorChoice::of(DONT);
    }
    // 4006-4018: air strikes.
    if c.adding_airstrike && !ctx.painting_vtol {
        if gui && (!c.over_tac_map || !c.sensor_probe_held) {
            cursor = NORMAL;
        } else {
            cursor = match (c.sensor_probe_pressed, los) {
                (true, true) => SENSORSTRIKE,
                (true, false) => UNCERTAIN_AIRSTRIKE,
                (false, true) => AIRSTRIKE,
                (false, false) => UNCERTAIN_AIRSTRIKE,
            };
        }
        return CursorChoice::of(cursor);
    } else if c.adding_vehicle {
        // 4021-4029.
        if ctx.can_add_vehicle && !gui {
            cursor = VEHICLE;
        } else if !ctx.painting_vtol && !gui {
            cursor = CANTVEHICLE;
        } else if gui {
            cursor = NORMAL;
        }
    } else if c.adding_salvage {
        // 4030-4038.
        if ctx.can_recover && !gui {
            cursor = SALVAGE;
        } else if !ctx.painting_vtol && !gui {
            cursor = XSALVAGE;
        } else if gui {
            cursor = NORMAL;
        }
    } else if ctx.waypoint_key_down || c.mines {
        // 4040-4059: the waypoint key, or laying mines (the mines test here means the later
        // mines branch of the C++ is never reached).
        if c.mines {
            cursor = if passable { LAYMINES } else { XMINES };
        } else {
            cursor = if los { WALKWAYPT_LOS } else { WALKWAYPT };
            if c.run {
                cursor = if los { RUNWAYPT_LOS } else { RUNWAYPT };
            } else if c.jump {
                cursor = if los { JUMPWAYPT_LOS } else { JUMPWAYPT };
            }
        }
        return CursorChoice::of(cursor);
    } else if c.jump {
        // 4065-4080: a jump needs canJumpToWPos.
        cursor = if !ctx.can_jump_to_wpos {
            DONT
        } else if ctrl {
            if los {
                JUMPWAYPT_LOS
            } else {
                JUMPWAYPT
            }
        } else {
            jump_cursor(los, rot)
        };
        if !passable && !heli {
            cursor = DONT;
        }
    } else if c.run && ctx.movers > 0 {
        // 4083-4093.
        cursor = if ctrl {
            if los {
                RUNWAYPT_LOS
            } else {
                RUNWAYPT
            }
        } else {
            run_cursor(los, rot)
        };
        if !passable && !heli {
            cursor = DONT;
        }
    } else if c.salvage || c.repair {
        // 4100-4110: move, DONT when impassable.
        cursor = move_cursor(los, rot);
        if !passable {
            cursor = DONT;
        }
    } else if c.guard {
        // 4113-4117.
        cursor = if los { GUARD_LOS } else { GUARD };
        if !passable {
            cursor = DONT;
        }
    } else if c.guard_tower {
        // 4119-4121.
        cursor = if los { GUARDTOWER_LOS } else { GUARDTOWER };
    }
    CursorChoice::of(cursor)
}

/// The cursor for the current mouse position (`MissionInterfaceManager`'s caller,
/// `missiongui.cpp:3428-3458`): the target cursor over an object, otherwise the no-target cursor.
pub fn choose_cursor(ctx: &CursorContext) -> CursorChoice {
    match &ctx.target {
        Some(t) => target_cursor(ctx, t),
        None => no_target_cursor(ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_values_fit_the_mouse_state_range() {
        assert_eq!(cursor_value(-1), None);
        assert_eq!(cursor_value(242), Some(242));
        assert_eq!(cursor_value(243), None);
    }
}
