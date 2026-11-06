// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission hot keys, command dispatch, rebinding and cursor choice (`code/missiongui.cpp`).
//!
//! Expected values are hand-derived from the C++; each test cites the lines it follows. Table
//! indices are positions in `MissionInterfaceManager::commands[]` (`missiongui.cpp:197-307`), the
//! same numbers `options.cfg` `Key<n>` uses.

use mc2_platform::input::KeyStatus;
use mc2_platform::keys::*;
use mc2_ui::hotkeys::{ALT, CTRL, SHIFT, WAYPT};
use mc2_ui::mission_keys::state::*;
use mc2_ui::mission_keys::{
    choose_cursor, ControlStyle, Controls, CursorContext, DispatchContext, FireRange, ForceGroup,
    HotKeyList, HotKeys, KeyFrame, MissionCommand, ObjectClass, Relation, SelectedMover, Target,
};

fn hot(style: ControlStyle) -> HotKeys {
    HotKeys::new(style)
}

fn frame(keys: &[(u32, KeyStatus)]) -> KeyFrame {
    keys.iter()
        .fold(KeyFrame::default(), |f, &(k, s)| f.with(k, s))
}

fn dispatch(hk: &HotKeys, f: &KeyFrame) -> mc2_ui::mission_keys::Dispatch {
    hk.dispatch(
        f,
        DispatchContext {
            chatting: false,
            line_of_sight: false,
        },
    )
}

fn commands(d: &mc2_ui::mission_keys::Dispatch) -> Vec<(MissionCommand, usize, bool)> {
    d.commands
        .iter()
        .map(|c| (c.command, c.index, c.release))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Default table and styles

#[test]
fn table_has_107_entries_in_the_original_order() {
    // missiongui.cpp:197 starts the table, 307 ends it; 309 closes the initialiser.
    let hk = hot(ControlStyle::Aoe);
    assert_eq!(hk.bindings().len(), 107);
    let b = hk.bindings();
    // 198: KEY_S, SHRTRNG_ATTACK 46 / SHRTRNG_LOS 242, single-click false, attackShort / defaultAttack.
    assert_eq!(b[0].key, KEY_S);
    assert_eq!((b[0].cursor, b[0].cursor_los), (46, 242));
    assert!(!b[0].single_click);
    assert_eq!(b[0].command, MissionCommand::AttackShort);
    assert_eq!(b[0].release, Some(MissionCommand::DefaultAttack));
    // 203-204: J is jump (cursor JUMP1 176), J|WAYPT is the waypoint jump (JUMPWAYPT 10 / 11).
    assert_eq!(b[5].command, MissionCommand::Jump);
    assert_eq!((b[5].cursor, b[5].cursor_los), (176, 5));
    assert_eq!(b[6].key, KEY_J | WAYPT);
    assert_eq!((b[6].cursor, b[6].cursor_los), (10, 11));
    // 263 P is pause; 307 PAUSE is pause too.
    assert_eq!(b[65].command, MissionCommand::TogglePause);
    assert_eq!(b[106].key, KEY_PAUSE);
    assert_eq!(b[106].command, MissionCommand::TogglePause);
    // 301-302: chat keys are the only entries the chat mode scans (missiongui.cpp:1700-1790, i=100..102).
    assert_eq!(b[100].command, MissionCommand::ChatKey);
    assert_eq!(b[101].command, MissionCommand::TeamChatKey);
}

/// One expected `commands[]` row: key, cursor, cursorLOS (`None` for the C++ `-1`), singleClick,
/// hotKeyDescriptionText, the press handler and the release handler.
type Row = (
    u32,
    Option<u32>,
    Option<u32>,
    bool,
    i32,
    MissionCommand,
    Option<MissionCommand>,
);

fn row(
    key: u32,
    cursor: Option<u32>,
    cursor_los: Option<u32>,
    single_click: bool,
    description: i32,
    command: MissionCommand,
    release: Option<MissionCommand>,
) -> Row {
    (
        key,
        cursor,
        cursor_los,
        single_click,
        description,
        command,
        release,
    )
}

/// The C++ `cursor` / `cursorLOS` column: `-1` is no cursor.
fn cursor_column(c: i32) -> Option<u32> {
    (c >= 0).then_some(c as u32)
}

/// Every row of `MissionInterfaceManager::commands[]`, transcribed by hand from
/// `code/missiongui.cpp:198-307` (`ALT|8`, `ALT|9`, `ALT|0` are commented out at 297-299 and are not
/// entries). Row `i` sits at line `198 + i` up to `i = 98`, then at `201 + i` (rows 99-106 are
/// lines 300-307). Mouse states are `mState_*` (`mclib/userinput.h:38-80`), description ids are
/// `resource.h` values (`IDS_HOTKEY_*`).
fn c_table() -> Vec<Row> {
    use MissionCommand as M;
    vec![
        row(
            KEY_S,
            Some(SHRTRNG_ATTACK),
            Some(SHRTRNG_LOS),
            false,
            43200,
            M::AttackShort,
            Some(M::DefaultAttack),
        ),
        row(
            KEY_M,
            Some(MEDRNG_ATTACK),
            Some(MEDRNG_LOS),
            false,
            43201,
            M::AttackMedium,
            Some(M::DefaultAttack),
        ),
        row(
            KEY_L,
            Some(LONGRNG_ATTACK),
            Some(LONGRNG_LOS),
            false,
            43202,
            M::AttackLong,
            Some(M::DefaultAttack),
        ),
        row(
            KEY_A,
            Some(ENERGY_WEAPONS),
            Some(ENERGY_WEAPONS_LOS),
            false,
            43204,
            M::EnergyWeapons,
            Some(M::EnergyWeapons),
        ),
        row(
            KEY_D,
            Some(GENERIC_ATTACK),
            Some(ATTACK_LOS),
            false,
            -1,
            M::DefaultAttack,
            None,
        ),
        row(
            KEY_J,
            Some(JUMP1),
            Some(JUMP_LOS),
            false,
            43205,
            M::Jump,
            Some(M::StopJump),
        ),
        row(
            KEY_J | WAYPT,
            Some(JUMPWAYPT),
            Some(JUMPWAYPT_LOS),
            false,
            -1,
            M::Jump,
            Some(M::StopJump),
        ),
        row(
            KEY_C,
            Some(CURPOS_ATTACK),
            Some(CURPOS_ATTACK_LOS),
            false,
            43206,
            M::FireFromCurrentPos,
            Some(M::StopFireFromCurrentPos),
        ),
        row(
            KEY_G,
            Some(GUARD),
            Some(GUARD_LOS),
            false,
            43207,
            M::Guard,
            Some(M::StopGuard),
        ),
        row(
            KEY_W,
            Some(ENERGY_WEAPONS),
            Some(ENERGY_WEAPONS_LOS),
            false,
            -1,
            M::ConserveAmmo,
            None,
        ),
        row(KEY_E, None, None, false, 43209, M::SelectVisible, None),
        row(KEY_F, None, None, false, 43210, M::ForceShot, None),
        row(KEY_HOME, None, None, true, -1, M::CameraNormal, None),
        row(KEY_F2, None, None, true, -1, M::CameraView(0), None),
        row(KEY_F3, None, None, true, -1, M::CameraView(1), None),
        row(KEY_F4, None, None, true, -1, M::CameraView(2), None),
        row(KEY_F5, None, None, true, -1, M::CameraView(3), None),
        row(
            KEY_F2 | CTRL,
            None,
            None,
            true,
            -1,
            M::CameraAssign(0),
            None,
        ),
        row(
            KEY_F3 | CTRL,
            None,
            None,
            true,
            -1,
            M::CameraAssign(1),
            None,
        ),
        row(
            KEY_F4 | CTRL,
            None,
            None,
            true,
            -1,
            M::CameraAssign(2),
            None,
        ),
        row(
            KEY_F5 | CTRL,
            None,
            None,
            true,
            -1,
            M::CameraAssign(3),
            None,
        ),
        row(
            KEY_NUMPAD2,
            Some(DONT),
            Some(AIMED_ATTACK_LOS),
            false,
            43211,
            M::AimLeg,
            None,
        ),
        row(
            KEY_NUMPAD5,
            Some(DONT),
            Some(AIMED_ATTACK_LOS),
            false,
            43212,
            M::AimArm,
            None,
        ),
        row(
            KEY_NUMPAD8,
            Some(DONT),
            Some(AIMED_ATTACK_LOS),
            false,
            43213,
            M::AimHead,
            None,
        ),
        row(KEY_BACK, None, None, false, 43214, M::RemoveCommand, None),
        row(
            KEY_NEXT | 0x100,
            None,
            None,
            false,
            43215,
            M::PowerDown,
            None,
        ),
        row(
            KEY_PRIOR | 0x100,
            None,
            None,
            false,
            43216,
            M::PowerUp,
            None,
        ),
        row(
            KEY_END | 0x100,
            Some(EJECT),
            Some(EJECT),
            false,
            43217,
            M::Eject,
            None,
        ),
        row(
            KEY_SPACE,
            Some(RUN),
            Some(RUN_LOS),
            false,
            43218,
            M::ChangeSpeed,
            Some(M::StopChangeSpeed),
        ),
        row(
            KEY_SPACE | WAYPT,
            Some(RUNWAYPT),
            Some(RUNWAYPT_LOS),
            false,
            -1,
            M::ChangeSpeed,
            Some(M::StopChangeSpeed),
        ),
        row(KEY_UP | 0x100, None, None, false, 43244, M::ScrollUp, None),
        row(
            KEY_DOWN | 0x100,
            None,
            None,
            false,
            43245,
            M::ScrollDown,
            None,
        ),
        row(
            KEY_LEFT | 0x100,
            None,
            None,
            false,
            43242,
            M::ScrollLeft,
            None,
        ),
        row(
            KEY_RIGHT | 0x100,
            None,
            None,
            false,
            43243,
            M::ScrollRight,
            None,
        ),
        row(KEY_SUBTRACT, None, None, false, 43225, M::ZoomOut, None),
        row(KEY_MINUS, None, None, false, -1, M::ZoomOut, None),
        row(KEY_ADD, None, None, false, 43224, M::ZoomIn, None),
        row(SHIFT | KEY_EQUALS, None, None, false, -1, M::ZoomIn, None),
        row(
            KEY_LEFT | SHIFT | 0x100,
            None,
            None,
            false,
            43246,
            M::RotateLeft,
            None,
        ),
        row(
            KEY_RIGHT | SHIFT | 0x100,
            None,
            None,
            false,
            43247,
            M::RotateRight,
            None,
        ),
        row(
            KEY_UP | SHIFT | 0x100,
            None,
            None,
            false,
            43248,
            M::TiltUp,
            None,
        ),
        row(
            KEY_DOWN | SHIFT | 0x100,
            None,
            None,
            false,
            43249,
            M::TiltDown,
            None,
        ),
        row(
            KEY_HOME | 0x100,
            None,
            None,
            false,
            -1,
            M::CenterCamera,
            None,
        ),
        row(
            KEY_LEFT | CTRL,
            None,
            None,
            false,
            -1,
            M::RotateLightLeft,
            None,
        ),
        row(
            KEY_RIGHT | CTRL,
            None,
            None,
            false,
            -1,
            M::RotateLightRight,
            None,
        ),
        row(KEY_UP | CTRL, None, None, false, -1, M::RotateLightUp, None),
        row(
            KEY_DOWN | CTRL,
            None,
            None,
            false,
            -1,
            M::RotateLightDown,
            None,
        ),
        // Debug rows 47-60 (`#ifndef FINAL` handlers), reported as Unavailable.
        row(
            KEY_T | CTRL | ALT,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawTerrain"),
            None,
        ),
        row(
            KEY_O | CTRL | ALT,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawOverlays"),
            None,
        ),
        row(
            KEY_B | CTRL | ALT,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawBuildings"),
            None,
        ),
        row(
            CTRL | ALT | KEY_G,
            None,
            None,
            true,
            -1,
            M::Unavailable("showGrid"),
            None,
        ),
        row(
            CTRL | ALT | KEY_Q,
            None,
            None,
            true,
            -1,
            M::Unavailable("recalcLights"),
            None,
        ),
        row(
            CTRL | ALT | KEY_C,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawClouds"),
            None,
        ),
        row(
            CTRL | ALT | KEY_F,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawFog"),
            None,
        ),
        row(
            CTRL | ALT | KEY_P,
            None,
            None,
            true,
            -1,
            M::Unavailable("usePerspective"),
            None,
        ),
        row(
            CTRL | ALT | KEY_S,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawTGLShapes"),
            None,
        ),
        row(
            CTRL | ALT | KEY_V,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawWaterEffects"),
            None,
        ),
        row(
            CTRL | ALT | KEY_W,
            None,
            None,
            true,
            -1,
            M::Unavailable("recalcWater"),
            None,
        ),
        row(
            CTRL | ALT | KEY_D,
            None,
            None,
            true,
            -1,
            M::Unavailable("drawShadows"),
            None,
        ),
        row(
            CTRL | ALT | KEY_L,
            None,
            None,
            true,
            -1,
            M::Unavailable("changeLighting"),
            None,
        ),
        row(
            CTRL | ALT | KEY_Z,
            None,
            None,
            true,
            -1,
            M::Unavailable("toggleGUI"),
            None,
        ),
        row(KEY_V, None, None, true, 43222, M::VehicleCommand, None),
        row(KEY_F9, None, None, true, 43226, M::ShowObjectives, None),
        row(KEY_ESCAPE, None, None, true, 43234, M::TogglePause, None),
        row(KEY_F1, None, None, true, -1, M::ToggleHotKeys, None),
        row(KEY_P, None, None, true, -1, M::TogglePause, None),
        row(KEY_TAB, None, None, true, -1, M::SwitchTab, None),
        row(
            KEY_TAB | SHIFT,
            None,
            None,
            true,
            -1,
            M::ReverseSwitchTab,
            None,
        ),
        row(
            KEY_I,
            None,
            None,
            true,
            43219,
            M::InfoCommand,
            Some(M::InfoButtonReleased),
        ),
        row(KEY_N, None, None, true, -1, M::GotoNextNavMarker, None),
        row(
            KEY_MULTIPLY,
            Some(UNCERTAIN_AIRSTRIKE),
            Some(AIRSTRIKE),
            true,
            43220,
            M::SendAirstrike,
            None,
        ),
        row(
            KEY_DIVIDE,
            Some(SENSORSTRIKE),
            Some(SENSORSTRIKE),
            true,
            43221,
            M::SendSensorStrike,
            None,
        ),
        row(
            KEY_BACKSLASH,
            None,
            None,
            true,
            43223,
            M::ToggleCompass,
            None,
        ),
        // Debug rows 73-98 (`ALT` debug handlers, `ALT|8`/`9`/`0` commented out).
        row(
            ALT | KEY_SLASH,
            None,
            None,
            false,
            -1,
            M::Unavailable("quickDebugInfo"),
            None,
        ),
        row(
            ALT | SHIFT | KEY_SLASH,
            None,
            None,
            false,
            -1,
            M::Unavailable("setGameObjectWindow"),
            None,
        ),
        row(
            ALT | CTRL | KEY_1,
            None,
            None,
            false,
            -1,
            M::Unavailable("pageGameObjectWindow1"),
            None,
        ),
        row(
            ALT | CTRL | KEY_2,
            None,
            None,
            false,
            -1,
            M::Unavailable("pageGameObjectWindow2"),
            None,
        ),
        row(
            ALT | CTRL | KEY_3,
            None,
            None,
            false,
            -1,
            M::Unavailable("pageGameObjectWindow3"),
            None,
        ),
        row(
            ALT | KEY_1,
            None,
            None,
            false,
            -1,
            M::Unavailable("jumpToDebugGameObject1"),
            None,
        ),
        row(
            ALT | KEY_2,
            None,
            None,
            false,
            -1,
            M::Unavailable("jumpToDebugGameObject2"),
            None,
        ),
        row(
            ALT | KEY_3,
            None,
            None,
            false,
            -1,
            M::Unavailable("jumpToDebugGameObject3"),
            None,
        ),
        row(
            ALT | KEY_T,
            None,
            None,
            false,
            -1,
            M::Unavailable("teleport"),
            None,
        ),
        row(
            ALT | KEY_W,
            None,
            None,
            false,
            -1,
            M::Unavailable("toggleDebugWins"),
            None,
        ),
        row(
            ALT | KEY_M,
            None,
            None,
            false,
            -1,
            M::Unavailable("showMovers"),
            None,
        ),
        row(
            ALT | KEY_C,
            None,
            None,
            false,
            -1,
            M::Unavailable("cullPathAreas"),
            None,
        ),
        row(
            ALT | KEY_Z,
            None,
            None,
            false,
            -1,
            M::Unavailable("zeroHPrime"),
            None,
        ),
        row(
            ALT | KEY_A,
            None,
            None,
            false,
            -1,
            M::Unavailable("calcValidAreaTable"),
            None,
        ),
        row(
            ALT | KEY_G,
            None,
            None,
            false,
            -1,
            M::Unavailable("globalMapLog"),
            None,
        ),
        row(
            ALT | KEY_B,
            None,
            None,
            false,
            -1,
            M::Unavailable("brainDead"),
            None,
        ),
        row(
            ALT | KEY_P,
            None,
            None,
            false,
            -1,
            M::Unavailable("goalPlan"),
            None,
        ),
        row(
            ALT | KEY_V,
            None,
            None,
            false,
            -1,
            M::Unavailable("showVictim"),
            None,
        ),
        row(
            ALT | CTRL | KEY_P,
            None,
            None,
            false,
            -1,
            M::Unavailable("enemyGoalPlan"),
            None,
        ),
        row(
            ALT | KEY_4,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject1"),
            None,
        ),
        row(
            ALT | KEY_5,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject2"),
            None,
        ),
        row(
            ALT | KEY_6,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject3"),
            None,
        ),
        row(
            ALT | KEY_7,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject4"),
            None,
        ),
        row(
            ALT | KEY_8,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject5"),
            None,
        ),
        row(
            ALT | KEY_9,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject6"),
            None,
        ),
        row(
            ALT | KEY_0,
            None,
            None,
            false,
            -1,
            M::Unavailable("damageObject0"),
            None,
        ),
        // 300-307
        row(KEY_H, None, None, true, -1, M::ToggleHoldPosition, None),
        row(KEY_RETURN, None, None, true, 43250, M::ChatKey, None),
        row(
            SHIFT | KEY_RETURN,
            None,
            None,
            true,
            43251,
            M::TeamChatKey,
            None,
        ),
        row(
            KEY_E | SHIFT,
            None,
            None,
            false,
            -1,
            M::AddVisibleToSelection,
            None,
        ),
        row(KEY_EQUALS, None, None, false, -1, M::ZoomIn, None),
        row(
            ALT | KEY_PERIOD,
            None,
            None,
            false,
            -1,
            M::Unavailable("rotateObjectLeft"),
            None,
        ),
        row(
            ALT | KEY_COMMA,
            None,
            None,
            false,
            -1,
            M::Unavailable("rotateObjectRight"),
            None,
        ),
        row(KEY_PAUSE, None, None, true, -1, M::TogglePause, None),
    ]
}

#[test]
fn key_constants_used_by_the_table_are_the_gameos_values() {
    // GameOS/include/gameos.hpp:1218-1340 (the gosEnum_KeyIndex values the table is written in).
    assert_eq!(KEY_NEXT, 0x22);
    assert_eq!(KEY_PRIOR, 0x21);
    assert_eq!(KEY_END, 0x23);
    assert_eq!(KEY_HOME, 0x24);
    assert_eq!(KEY_PAUSE, 0x13);
    assert_eq!(KEY_ESCAPE, 0x1B);
    assert_eq!(KEY_MULTIPLY, 0x6A);
    assert_eq!(KEY_DIVIDE, 0x6F);
    assert_eq!(KEY_ADD, 0x6B);
    assert_eq!(KEY_SUBTRACT, 0x6D);
    assert_eq!(KEY_EQUALS, 0xBB);
    assert_eq!(KEY_MINUS, 0xBD);
    assert_eq!(KEY_COMMA, 0xBC);
    assert_eq!(KEY_PERIOD, 0xBE);
    assert_eq!(KEY_SLASH, 0xBF);
    assert_eq!(KEY_BACKSLASH, 0xDC);
    assert_eq!(KEY_LCONTROL, 0x11);
    assert_eq!(KEY_LALT, 0xE2);
    // Modifier bits: missiongui.cpp:155-158.
    assert_eq!(
        (CTRL, SHIFT, ALT, WAYPT),
        (0x1000_0000, 0x0100_0000, 0x0010_0000, 0x2000_0000)
    );
}

#[test]
fn every_default_binding_matches_the_c_table_in_both_styles() {
    // The C++ table has no style input (missiongui.cpp:197), so both styles must produce the same
    // rows. Checking every row in each style covers the styles' equivalence without assuming it.
    let expected = c_table();
    assert_eq!(expected.len(), 107);
    for style in [ControlStyle::Aoe, ControlStyle::Mc] {
        let hk = hot(style);
        assert_eq!(hk.bindings().len(), expected.len());
        for (i, (b, e)) in hk.bindings().iter().zip(&expected).enumerate() {
            let line = if i < 99 { 198 + i } else { 201 + i };
            let got = (
                b.key,
                cursor_column(b.cursor),
                cursor_column(b.cursor_los),
                b.single_click,
                b.description,
                b.command,
                b.release,
            );
            assert_eq!(got, *e, "{style:?} table index {i}, missiongui.cpp:{line}");
        }
    }
}

#[test]
fn both_control_styles_share_one_table() {
    // missiongui.cpp:197 has no style parameter. The style is the single flag
    // useLeftRightMouseProfile (setAOEStyle/setMCStyle, missiongui.cpp:5378-5393), and the
    // table never reads it. Its default is true (mechcmd2.cpp:106), i.e. AOE.
    let aoe = hot(ControlStyle::Aoe);
    let mc = hot(ControlStyle::Mc);
    assert_eq!(aoe.bindings(), mc.bindings());
    assert!(aoe.is_aoe() && !aoe.is_mc());
    assert!(mc.is_mc() && !mc.is_aoe());
}

#[test]
fn set_style_flips_the_flag_only() {
    // missiongui.cpp:5378-5393: setAOEStyle sets the flag true, setMCStyle false.
    let mut hk = hot(ControlStyle::Mc);
    let before = hk.bindings().to_vec();
    hk.set_style(ControlStyle::Aoe);
    assert!(hk.is_aoe());
    hk.set_style(ControlStyle::Mc);
    assert!(hk.is_mc());
    assert_eq!(hk.bindings(), before.as_slice());
}

#[test]
fn default_waypoint_key_is_left_control() {
    // missiongui.cpp:849-850: WAYPOINT_KEY defaults to KEY_LCONTROL when unset.
    assert_eq!(hot(ControlStyle::Aoe).waypoint_key(), KEY_LCONTROL);
}

#[test]
fn debug_entries_are_explicitly_unavailable() {
    // Every ALT-modified entry is a debug key under #ifndef FINAL (e.g. drawTerrain 2632-2636,
    // rotateObjectLeft 4853-4857). Indices 47-60 (CTRL|ALT), 73-98 and 104-105 (ALT).
    let hk = hot(ControlStyle::Aoe);
    for (i, b) in hk.bindings().iter().enumerate() {
        let debug = (47..=60).contains(&i) || (73..=98).contains(&i) || (104..=105).contains(&i);
        match b.command {
            MissionCommand::Unavailable(name) => {
                assert!(debug, "index {i} {name} should not be debug");
            }
            _ => assert!(!debug, "index {i} must be explicitly unavailable"),
        }
    }
    assert_eq!(
        hk.binding(47).command,
        MissionCommand::Unavailable("drawTerrain")
    );
    assert_eq!(
        hk.binding(81).command,
        MissionCommand::Unavailable("teleport")
    );
    assert_eq!(
        hk.binding(82).command,
        MissionCommand::Unavailable("toggleDebugWins")
    );
    assert_eq!(
        hk.binding(83).command,
        MissionCommand::Unavailable("showMovers")
    );
    assert_eq!(
        hk.binding(88).command,
        MissionCommand::Unavailable("brainDead")
    );
}

// ---------------------------------------------------------------------------------------------
// Dispatch: default bindings and modifiers

#[test]
fn plain_key_fires_its_command_with_the_table_cursor() {
    // 198: KEY_S, cursor 46 (SHRTRNG_ATTACK). Dispatch sets the cursor for a fired command
    // (missiongui.cpp:1775-1776).
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(&hk, &frame(&[(KEY_S, KeyStatus::Pressed)]));
    assert_eq!(commands(&d), vec![(MissionCommand::AttackShort, 0, false)]);
    assert_eq!(d.cursor, Some(46));
}

#[test]
fn line_of_sight_selects_the_los_cursor() {
    // 198: cursor_los 242 (SHRTRNG_LOS) when bLOS is set (missiongui.cpp:1776).
    let hk = hot(ControlStyle::Aoe);
    let d = hk.dispatch(
        &frame(&[(KEY_S, KeyStatus::Pressed)]),
        DispatchContext {
            chatting: false,
            line_of_sight: true,
        },
    );
    assert_eq!(d.cursor, Some(242));
}

#[test]
fn shift_blocks_an_unmodified_binding() {
    // missiongui.cpp:1763-1771: a binding without SHIFT is skipped when shift is down.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(
        &hk,
        &frame(&[(KEY_LSHIFT, KeyStatus::Held), (KEY_S, KeyStatus::Pressed)]),
    );
    assert!(d.commands.is_empty());
}

#[test]
fn shift_matches_the_shift_binding_only() {
    // 37 SHIFT|EQUALS zoomIn and 103 EQUALS zoomIn: with shift down only 37 fires; without, only 103.
    let hk = hot(ControlStyle::Aoe);
    let shifted = dispatch(
        &hk,
        &frame(&[
            (KEY_RSHIFT, KeyStatus::Held),
            (KEY_EQUALS, KeyStatus::Pressed),
        ]),
    );
    assert_eq!(
        commands(&shifted),
        vec![(MissionCommand::ZoomIn, 37, false)]
    );
    let plain = dispatch(&hk, &frame(&[(KEY_EQUALS, KeyStatus::Pressed)]));
    assert_eq!(commands(&plain), vec![(MissionCommand::ZoomIn, 103, false)]);
}

#[test]
fn ctrl_f2_is_camera_assign_and_plain_f2_is_camera_view() {
    // 13 F2 cameraDefault (view 0) requires no CTRL; 17 F2|CTRL cameraAssign0 (missiongui.cpp:211, 215).
    let hk = hot(ControlStyle::Aoe);
    let ctrl = dispatch(
        &hk,
        &frame(&[
            (KEY_LCONTROL, KeyStatus::Held),
            (KEY_F2, KeyStatus::Pressed),
        ]),
    );
    // LCONTROL is also the waypoint key, but no waypoint entry matches F2, so only index 17 fires.
    assert_eq!(
        commands(&ctrl),
        vec![(MissionCommand::CameraAssign(0), 17, false)]
    );
    let plain = dispatch(&hk, &frame(&[(KEY_F2, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&plain),
        vec![(MissionCommand::CameraView(0), 13, false)]
    );
}

#[test]
fn single_click_fires_on_press_not_while_held() {
    // 210-214: camera keys are single-click (true); missiongui.cpp:1778-1790 fires only when the
    // key status is not KEY_HELD.
    let hk = hot(ControlStyle::Aoe);
    let pressed = dispatch(&hk, &frame(&[(KEY_F5, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&pressed),
        vec![(MissionCommand::CameraView(3), 16, false)]
    );
    let held = dispatch(&hk, &frame(&[(KEY_F5, KeyStatus::Held)]));
    assert!(held.commands.is_empty());
}

#[test]
fn continuous_binding_fires_every_frame_while_held() {
    // 226: SPACE changeSpeed is not single-click (false), so a held key fires again.
    let hk = hot(ControlStyle::Aoe);
    let held = dispatch(&hk, &frame(&[(KEY_SPACE, KeyStatus::Held)]));
    assert_eq!(
        commands(&held),
        vec![(MissionCommand::ChangeSpeed, 28, false)]
    );
}

#[test]
fn release_handler_fires_when_the_key_goes_up() {
    // 206: G guard, release stopGuard (missiongui.cpp:1790-1795). A release sets no cursor.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(&hk, &frame(&[(KEY_G, KeyStatus::Released)]));
    assert_eq!(commands(&d), vec![(MissionCommand::StopGuard, 8, true)]);
    assert_eq!(d.cursor, None);
}

#[test]
fn waypoint_entries_ignore_modifiers_and_need_the_waypoint_key() {
    // 204 J|WAYPT: missiongui.cpp:1763 skips shift/ctrl/alt checks for WAYPT entries and tests
    // only the waypoint key. With LCONTROL held the plain J (5) needs CTRL unset, so only the
    // waypoint jump fires, with cursor JUMPWAYPT 10.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(
        &hk,
        &frame(&[(KEY_LCONTROL, KeyStatus::Held), (KEY_J, KeyStatus::Pressed)]),
    );
    assert_eq!(commands(&d), vec![(MissionCommand::Jump, 6, false)]);
    assert!(d.commands[0].waypoint);
    assert_eq!(d.cursor, Some(10));
}

#[test]
fn waypoint_key_with_shift_still_fires_the_waypoint_entry() {
    // The WAYPT entry matches regardless of SHIFT (missiongui.cpp:1763-1765).
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(
        &hk,
        &frame(&[
            (KEY_LCONTROL, KeyStatus::Held),
            (KEY_LSHIFT, KeyStatus::Held),
            (KEY_J, KeyStatus::Pressed),
        ]),
    );
    assert_eq!(commands(&d), vec![(MissionCommand::Jump, 6, false)]);
}

#[test]
fn plain_j_without_waypoint_key_fires_the_jump_with_its_cursor() {
    // 203: J jump, cursor JUMP1 176, los JUMP_LOS 5, continuous (false), release stopJump.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(&hk, &frame(&[(KEY_J, KeyStatus::Pressed)]));
    assert_eq!(commands(&d), vec![(MissionCommand::Jump, 5, false)]);
    assert_eq!(d.cursor, Some(176));
}

#[test]
fn waypoint_release_reports_waypoints_done_and_suppresses_force_groups() {
    // missiongui.cpp:1700-1718: releasing the waypoint key tells the selected movers to run
    // their queued waypoints. bRetVal then skips the force-group block (1740-1753).
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(
        &hk,
        &frame(&[
            (KEY_LCONTROL, KeyStatus::Released),
            (KEY_3, KeyStatus::Pressed),
        ]),
    );
    assert!(d.waypoints_done);
    assert!(d.force_groups.is_empty());
}

#[test]
fn force_group_digits_follow_ctrl_shift_and_plain() {
    // missiongui.cpp:1740-1753: ctrl makes the group; shift toggles it (deselect=false); plain
    // digit selects exclusively (deselect=true, selectForceGroup at 3249).
    let hk = hot(ControlStyle::Aoe);
    let make = dispatch(
        &hk,
        &frame(&[(KEY_LCONTROL, KeyStatus::Held), (KEY_3, KeyStatus::Pressed)]),
    );
    assert_eq!(make.force_groups, vec![ForceGroup::Make(3)]);
    let toggle = dispatch(
        &hk,
        &frame(&[(KEY_LSHIFT, KeyStatus::Held), (KEY_7, KeyStatus::Pressed)]),
    );
    assert_eq!(
        toggle.force_groups,
        vec![ForceGroup::Select {
            group: 7,
            exclusive: false
        }]
    );
    let plain = dispatch(&hk, &frame(&[(KEY_0, KeyStatus::Pressed)]));
    assert_eq!(
        plain.force_groups,
        vec![ForceGroup::Select {
            group: 0,
            exclusive: true
        }]
    );
}

#[test]
fn a_held_digit_does_not_repeat_a_force_group_command() {
    // missiongui.cpp:1739: digits fire only when status != KEY_HELD.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(&hk, &frame(&[(KEY_1, KeyStatus::Held)]));
    assert!(d.force_groups.is_empty());
}

#[test]
fn chat_mode_only_scans_the_chat_entries() {
    // missiongui.cpp:1700-1716: while chatting, i = 100 and last = 102, so only 100 and 101 run.
    let hk = hot(ControlStyle::Aoe);
    let chatting = DispatchContext {
        chatting: true,
        line_of_sight: false,
    };
    let s = hk.dispatch(&frame(&[(KEY_S, KeyStatus::Pressed)]), chatting);
    assert!(s.commands.is_empty());
    let ret = hk.dispatch(&frame(&[(KEY_RETURN, KeyStatus::Pressed)]), chatting);
    assert_eq!(commands(&ret), vec![(MissionCommand::ChatKey, 100, false)]);
    let team = hk.dispatch(
        &frame(&[
            (KEY_LSHIFT, KeyStatus::Held),
            (KEY_RETURN, KeyStatus::Pressed),
        ]),
        chatting,
    );
    assert_eq!(
        commands(&team),
        vec![(MissionCommand::TeamChatKey, 101, false)]
    );
    // Force groups are skipped while chatting (bRetVal set at 1700-1716).
    let digit = hk.dispatch(&frame(&[(KEY_2, KeyStatus::Pressed)]), chatting);
    assert!(digit.force_groups.is_empty());
}

#[test]
fn ctrl_alt_debug_key_is_reported_unavailable() {
    // 47 T|CTRL|ALT drawTerrain: dispatch reports the explicit unavailable value.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(
        &hk,
        &frame(&[
            (KEY_LCONTROL, KeyStatus::Held),
            (KEY_LALT, KeyStatus::Held),
            (KEY_T, KeyStatus::Pressed),
        ]),
    );
    assert_eq!(
        commands(&d),
        vec![(MissionCommand::Unavailable("drawTerrain"), 47, false)]
    );
}

#[test]
fn ctrl_arrow_rotates_light_and_plain_arrow_scrolls() {
    // 30-33 scroll with no modifier; 43-46 CTRL rotateLight (missiongui.cpp:228-231, 241-244).
    let hk = hot(ControlStyle::Aoe);
    let plain = dispatch(&hk, &frame(&[(KEY_UP, KeyStatus::Held)]));
    assert_eq!(
        commands(&plain),
        vec![(MissionCommand::ScrollUp, 30, false)]
    );
    let ctrl = dispatch(
        &hk,
        &frame(&[(KEY_LCONTROL, KeyStatus::Held), (KEY_UP, KeyStatus::Held)]),
    );
    // The waypoint key is also held, so the waypoint entries are checked and the CTRL arrow fires
    // rotateLightUp (45); plain UP (30) is skipped because CTRL is down.
    assert_eq!(
        commands(&ctrl),
        vec![(MissionCommand::RotateLightUp, 45, false)]
    );
}

#[test]
fn vehicle_command_and_objectives_keys() {
    // 61 V: vehicleCommand (43222); 62 F9: showObjectives (43226).
    let hk = hot(ControlStyle::Mc);
    let v = dispatch(&hk, &frame(&[(KEY_V, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&v),
        vec![(MissionCommand::VehicleCommand, 61, false)]
    );
    let f9 = dispatch(&hk, &frame(&[(KEY_F9, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&f9),
        vec![(MissionCommand::ShowObjectives, 62, false)]
    );
}

#[test]
fn air_and_sensor_strikes() {
    // 70 MULTIPLY sendAirstrike (cursor 25/26); 71 DIVIDE sendSensorStrike (27/27).
    let hk = hot(ControlStyle::Aoe);
    let a = dispatch(&hk, &frame(&[(KEY_MULTIPLY, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&a),
        vec![(MissionCommand::SendAirstrike, 70, false)]
    );
    assert_eq!((a.cursor), Some(25));
    let s = dispatch(&hk, &frame(&[(KEY_DIVIDE, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&s),
        vec![(MissionCommand::SendSensorStrike, 71, false)]
    );
    assert_eq!(s.cursor, Some(27));
}

#[test]
fn info_nav_marker_and_pause_keys() {
    // 68 I infoCommand (release infoButtonReleased); 69 N gotoNextNavMarker; 63 ESC and 65 P both pause.
    let hk = hot(ControlStyle::Aoe);
    let i = dispatch(&hk, &frame(&[(KEY_I, KeyStatus::Released)]));
    assert_eq!(
        commands(&i),
        vec![(MissionCommand::InfoButtonReleased, 68, true)]
    );
    let n = dispatch(&hk, &frame(&[(KEY_N, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&n),
        vec![(MissionCommand::GotoNextNavMarker, 69, false)]
    );
    let esc = dispatch(&hk, &frame(&[(KEY_ESCAPE, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&esc),
        vec![(MissionCommand::TogglePause, 63, false)]
    );
}

#[test]
fn aim_keys_use_the_aimed_attack_cursors() {
    // 21-23 numpad 2/5/8: cursor DONT 30, los AIMED_ATTACK_LOS 20 (missiongui.cpp:219-221).
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(&hk, &frame(&[(KEY_NUMPAD5, KeyStatus::Pressed)]));
    assert_eq!(commands(&d), vec![(MissionCommand::AimArm, 22, false)]);
    assert_eq!(d.cursor, Some(30));
}

// ---------------------------------------------------------------------------------------------
// hotKeyIsPressed and get/set

#[test]
fn hot_key_is_pressed_requires_exact_modifiers() {
    // missiongui.cpp:5408-5439: the key is PRESSED or HELD and shift/ctrl/alt must equal the binding's.
    let hk = hot(ControlStyle::Aoe);
    let plain_l = frame(&[(KEY_L, KeyStatus::Pressed)]);
    assert!(hk.is_pressed(2, &plain_l));
    let shifted = frame(&[(KEY_L, KeyStatus::Pressed), (KEY_LSHIFT, KeyStatus::Held)]);
    assert!(!hk.is_pressed(2, &shifted));
    // 13 F2 (no ctrl) is not pressed while ctrl is down, but 17 F2|CTRL is.
    let ctrl_f2 = frame(&[(KEY_F2, KeyStatus::Held), (KEY_LCONTROL, KeyStatus::Held)]);
    assert!(!hk.is_pressed(13, &ctrl_f2));
    assert!(hk.is_pressed(17, &ctrl_f2));
}

#[test]
fn get_and_set_hot_key_round_trip_with_modifiers() {
    // missiongui.cpp:5319-5366: setHotKey ORs SHIFT/CTRL/ALT into the key; getHotKey reports them.
    let mut hk = hot(ControlStyle::Aoe);
    hk.set_hot_key(5, KEY_K, true, false, true);
    assert_eq!(hk.binding(5).key, KEY_K | SHIFT | ALT);
    assert_eq!(hk.get_hot_key(5), (KEY_K, true, false, true));
}

#[test]
fn set_hot_key_moves_the_waypoint_twin_of_an_unmodified_key() {
    // missiongui.cpp:5331-5342: when the old key has no modifiers, the WAYPT entry with the same
    // low 16 bits follows it. Index 6 is J|WAYPT.
    let mut hk = hot(ControlStyle::Aoe);
    hk.set_hot_key(5, KEY_K, false, false, false);
    assert_eq!(hk.binding(6).key, KEY_K | WAYPT);
    // Setting the WAYPT entry itself keeps its WAYPT bit (missiongui.cpp:5331-5333).
    hk.set_hot_key(6, KEY_Q, false, false, false);
    assert_eq!(hk.binding(6).key, KEY_Q | WAYPT);
    assert_eq!(hk.binding(5).key, KEY_K);
}

// ---------------------------------------------------------------------------------------------
// Save and load in the C++ file format

#[test]
fn save_writes_the_keyboard_block_in_the_c_format() {
    // missiongui.cpp:5272-5285 and mclib/inifile.cpp:2679-2717: [Keyboard], "l Key<n> = <long>",
    // "l WayPointKey = ...", "b UseLeftRightMouseProfile = TRUE|FALSE".
    let hk = hot(ControlStyle::Aoe);
    let text = String::from_utf8(hk.save()).expect("ascii");
    assert!(text.starts_with("FITini "), "{text:?}");
    assert!(text.contains("\r\n[Keyboard]\r\n"));
    assert!(text.contains("l Key0 = 83\r\n")); // KEY_S 0x53
    assert!(text.contains("l Key5 = 74\r\n")); // KEY_J 0x4A
    assert!(text.contains("l Key106 = 19\r\n")); // KEY_PAUSE 0x13
    assert!(text.contains("l WayPointKey = 17\r\n"));
    assert!(text.contains("b UseLeftRightMouseProfile = TRUE\r\n"));
    assert!(text.ends_with("FITend \r\n"));
}

#[test]
fn save_load_round_trip_keeps_bindings_waypoint_key_and_style() {
    let mut hk = hot(ControlStyle::Aoe);
    hk.set_hot_key(0, KEY_Q, true, false, false);
    hk.set_hot_key(65, KEY_T, false, true, false); // togglePause on P -> CTRL|T
    hk.set_waypoint_key(KEY_RCONTROL);
    hk.set_style(ControlStyle::Mc);
    let bytes = hk.save();

    let mut loaded = hot(ControlStyle::Aoe);
    loaded.load(bytes).expect("[Keyboard] present");
    assert_eq!(loaded.bindings(), hk.bindings());
    assert_eq!(loaded.waypoint_key(), KEY_RCONTROL);
    assert!(loaded.is_mc());
}

#[test]
fn load_without_a_keyboard_block_changes_nothing() {
    // missiongui.cpp:5288-5317: seekBlock("Keyboard") failing returns -1 and leaves the table.
    let mut hk = hot(ControlStyle::Aoe);
    hk.set_hot_key(0, KEY_Q, false, false, false);
    let before = hk.bindings().to_vec();
    let other = b"FITini \r\n\r\n[Other]\r\nl Key0 = 5\r\nFITend \r\n".to_vec();
    assert!(hk.load(other).is_err());
    assert_eq!(hk.bindings(), before.as_slice());
}

// ---------------------------------------------------------------------------------------------
// Rebinding conflicts (optionsarea.cpp:1043-1100, OptionsHotKeys::update)

#[test]
fn rebinding_onto_a_default_held_by_the_other_command_swaps_the_keys() {
    // C (7) moves to G. G (8) is the conflict. G's default is G, which G itself still holds, so
    // the default is not free and G takes C's old key (optionsarea.cpp:1075-1098).
    let hk = hot(ControlStyle::Aoe);
    let mut list = HotKeyList::from_bindings(&hk);
    let c = list.position(7).expect("C listed");
    let g = list.position(8).expect("G listed");
    assert_eq!(list.conflicts(c, KEY_G), vec![g]);
    list.set(c, KEY_G);
    assert_eq!(list.hot_key(c), KEY_G);
    assert_eq!(list.hot_key(g), KEY_C);
    let mut applied = hot(ControlStyle::Aoe);
    list.apply(&mut applied);
    assert_eq!(applied.binding(7).key, KEY_G);
    assert_eq!(applied.binding(8).key, KEY_C);
}

#[test]
fn rebinding_gives_the_other_command_its_default_when_that_is_free() {
    // Move AttackShort (0) from S to Q first, so its default S is free. Then AttackLong (2) to Q
    // conflicts with AttackShort. Its default S is now unused, so it gets S back
    // (optionsarea.cpp:1075-1087).
    let hk = hot(ControlStyle::Aoe);
    let mut list = HotKeyList::from_bindings(&hk);
    let s = list.position(0).expect("S listed");
    let l = list.position(2).expect("L listed");
    list.set(s, KEY_Q);
    assert_eq!(list.conflicts(l, KEY_Q), vec![s]);
    list.set(l, KEY_Q);
    assert_eq!(list.hot_key(s), KEY_S);
    assert_eq!(list.hot_key(l), KEY_Q);
}

#[test]
fn rebinding_to_a_free_key_is_a_plain_set() {
    let hk = hot(ControlStyle::Aoe);
    let mut list = HotKeyList::from_bindings(&hk);
    let p = list.position(3).expect("A listed");
    assert!(list.conflicts(p, KEY_Y).is_empty());
    list.set(p, KEY_Y);
    assert_eq!(list.hot_key(p), KEY_Y);
}

#[test]
fn only_listed_commands_take_part_in_conflicts() {
    // Unlisted commands (description -1, e.g. the waypoint jump 6) never appear in the options list.
    let hk = hot(ControlStyle::Aoe);
    let list = HotKeyList::from_bindings(&hk);
    assert!(list.position(6).is_none());
    assert!(list.position(5).is_some());
}

// ---------------------------------------------------------------------------------------------
// Cursor choice (missiongui.cpp:3700-4130; move/jump/run at 5544-5575)

/// A selected passable mover context: one mover, passable ground, no target, no controls.
fn ctx() -> CursorContext {
    CursorContext {
        passable: true,
        movers: 1,
        ..CursorContext::default()
    }
}

fn target(class: ObjectClass, relation: Relation) -> Target {
    Target {
        class,
        relation,
        ..Target::default()
    }
}

#[test]
fn no_selection_shows_the_normal_cursor() {
    // 3988-3989: with no movers, makeNoTargetCursor starts from NORMAL.
    let c = choose_cursor(&CursorContext {
        passable: true,
        movers: 0,
        ..CursorContext::default()
    });
    assert_eq!(c.cursor, NORMAL);
}

#[test]
fn passable_ground_gets_the_move_cursor_for_the_rotation() {
    // 5544-5552: WALK1 (48) + 32 * bLOS + rotation.
    let c = choose_cursor(&CursorContext {
        rotation: 5,
        ..ctx()
    });
    assert_eq!(c.cursor, WALK1 + 5);
    assert_eq!(c.cursor, 53);
}

#[test]
fn line_of_sight_moves_to_the_los_walk_cursors() {
    // WALKLOS1 = 80 (userinput.h:73), so WALK1 + 32 + rotation (3-line sum at 5551).
    let c = choose_cursor(&CursorContext {
        line_of_sight: true,
        rotation: 5,
        ..ctx()
    });
    assert_eq!(c.cursor, 85);
}

#[test]
fn impassable_ground_gets_dont() {
    // 3998-3999: no passable ground and no helicopters gives DONT (30).
    let c = choose_cursor(&CursorContext {
        passable: false,
        ..ctx()
    });
    assert_eq!(c.cursor, DONT);
}

#[test]
fn helicopters_may_cross_impassable_ground() {
    // 3998: the DONT test is skipped when selectionIsHelicopters().
    let c = choose_cursor(&CursorContext {
        passable: false,
        selection_helicopters: true,
        ..ctx()
    });
    assert_eq!(c.cursor, WALK1);
}

#[test]
fn jump_mode_with_a_jump_gives_the_jump_cursor() {
    // 4066-4074 then makeJumpCursor 5555-5563: JUMP1 (176) + rotation.
    let c = choose_cursor(&CursorContext {
        rotation: 3,
        can_jump_to_wpos: true,
        controls: Controls {
            jump: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(c.cursor, JUMP1 + 3);
}

#[test]
fn jump_mode_without_a_jump_gives_dont() {
    // 4076: canJumpToWPos() false gives DONT.
    let c = choose_cursor(&CursorContext {
        can_jump_to_wpos: false,
        controls: Controls {
            jump: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(c.cursor, DONT);
}

#[test]
fn run_mode_gives_the_run_cursor_and_ctrl_gives_the_run_waypoint() {
    // 4083-4088: RUN1 (112) + rotation; with ctrl, RUNWAYPT (8).
    let run = choose_cursor(&CursorContext {
        rotation: 2,
        controls: Controls {
            run: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(run.cursor, RUN1 + 2);
    let ctrl = choose_cursor(&CursorContext {
        ctrl_down: true,
        controls: Controls {
            run: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(ctrl.cursor, RUNWAYPT);
}

#[test]
fn waypoint_key_gives_the_walk_waypoint_cursor() {
    // 4040-4058: the waypoint key shows WALKWAYPT (6), or WALKWAYPT_LOS (7) with a line of sight.
    let c = choose_cursor(&CursorContext {
        waypoint_key_down: true,
        ..ctx()
    });
    assert_eq!(c.cursor, WALKWAYPT);
    let los = choose_cursor(&CursorContext {
        waypoint_key_down: true,
        line_of_sight: true,
        ..ctx()
    });
    assert_eq!(los.cursor, WALKWAYPT_LOS);
}

#[test]
fn waypoint_key_with_jump_mode_gives_the_jump_waypoint() {
    // 4052-4054: with the waypoint key and jump mode, JUMPWAYPT (10).
    let c = choose_cursor(&CursorContext {
        waypoint_key_down: true,
        controls: Controls {
            jump: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(c.cursor, JUMPWAYPT);
}

#[test]
fn force_fire_shows_the_range_cursor() {
    // 3994 and makeRangeCursor 3940-3971: medium range attack (MEDRNG_ATTACK 47).
    let c = choose_cursor(&CursorContext {
        force_fire_held: true,
        range: FireRange::Medium,
        ..ctx()
    });
    assert_eq!(c.cursor, MEDRNG_ATTACK);
}

#[test]
fn energy_weapons_show_the_energy_cursor_when_forcing_fire() {
    // makeRangeCursor 3941-3942: bEnergyWeapons returns ENERGY_WEAPONS (17) or the LOS one (18).
    let c = choose_cursor(&CursorContext {
        force_fire_held: true,
        energy_weapons: true,
        line_of_sight: true,
        ..ctx()
    });
    assert_eq!(c.cursor, ENERGY_WEAPONS_LOS);
}

#[test]
fn fire_from_current_position_shows_the_current_position_cursor() {
    // makeRangeCursor 3964-3966: fireFromCurrentPos overrides the range (CURPOS_ATTACK_LOS 16).
    let c = choose_cursor(&CursorContext {
        force_fire_held: true,
        fire_from_current_pos: true,
        line_of_sight: true,
        ..ctx()
    });
    assert_eq!(c.cursor, CURPOS_ATTACK_LOS);
}

#[test]
fn mines_on_passable_ground_lay_mines_and_on_blocked_ground_do_not() {
    // 4095-4105: LAYMINES (23) on passable ground; XMINES (42) otherwise.
    let lay = choose_cursor(&CursorContext {
        controls: Controls {
            mines: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(lay.cursor, LAYMINES);
    let blocked = choose_cursor(&CursorContext {
        passable: false,
        controls: Controls {
            mines: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(blocked.cursor, XMINES);
}

#[test]
fn guard_mode_on_passable_ground_gives_the_guard_cursor() {
    // 4113-4117: GUARD_LOS and GUARD are both 39; impassable ground gives DONT.
    let c = choose_cursor(&CursorContext {
        controls: Controls {
            guard: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(c.cursor, GUARD);
    let blocked = choose_cursor(&CursorContext {
        passable: false,
        controls: Controls {
            guard: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(blocked.cursor, DONT);
}

#[test]
fn adding_a_vehicle_checks_the_landing_spot() {
    // 4023-4030: VEHICLE (28) where canAddVehicle; CANTVEHICLE (29) otherwise when not over the GUI.
    let ok = choose_cursor(&CursorContext {
        can_add_vehicle: true,
        controls: Controls {
            adding_vehicle: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(ok.cursor, VEHICLE);
    let no = choose_cursor(&CursorContext {
        can_add_vehicle: false,
        controls: Controls {
            adding_vehicle: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(no.cursor, CANTVEHICLE);
}

#[test]
fn airstrike_with_the_sensor_probe_and_line_of_sight_gives_the_sensor_strike() {
    // 4006-4017: a pressed sensor probe with LOS gives SENSORSTRIKE (27); without LOS UNCERTAIN (25).
    let los = choose_cursor(&CursorContext {
        line_of_sight: true,
        controls: Controls {
            adding_airstrike: true,
            sensor_probe_pressed: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(los.cursor, SENSORSTRIKE);
    let no_los = choose_cursor(&CursorContext {
        controls: Controls {
            adding_airstrike: true,
            sensor_probe_pressed: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(no_los.cursor, UNCERTAIN_AIRSTRIKE);
}

#[test]
fn info_selection_over_terrain_gives_dont_and_over_a_mover_gives_info() {
    // 3716-3724 (target path) and 4001-4004 (no-target path).
    let terrain = choose_cursor(&CursorContext {
        controls: Controls {
            info_select: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(terrain.cursor, DONT);
    let mover = choose_cursor(&CursorContext {
        controls: Controls {
            info_select: true,
            ..Controls::default()
        },
        target: Some(Target {
            is_mover: true,
            ..target(ObjectClass::Mech, Relation::Enemy)
        }),
        ..ctx()
    });
    assert_eq!(mover.cursor, INFO);
}

#[test]
fn enemy_mech_under_the_cursor_keeps_the_range_cursor_and_is_targeted() {
    // 3705-3707: the base is the range cursor (Optimal gives GENERIC_ATTACK 13); the enemy branch
    // 3826-3836 marks it targeted and keeps the cursor.
    let c = choose_cursor(&CursorContext {
        target: Some(target(ObjectClass::Mech, Relation::Enemy)),
        ..ctx()
    });
    assert_eq!(c.cursor, GENERIC_ATTACK);
    assert!(c.targeted);
    assert!(!c.target_cleared);
}

#[test]
fn disabled_enemy_mech_gives_the_normal_cursor() {
    // 3832-3834: a disabled enemy mech gives NORMAL.
    let c = choose_cursor(&CursorContext {
        target: Some(Target {
            disabled: true,
            ..target(ObjectClass::Mech, Relation::Enemy)
        }),
        ..ctx()
    });
    assert_eq!(c.cursor, NORMAL);
    assert!(c.targeted);
}

#[test]
fn friendly_mech_under_the_cursor_gives_the_normal_cursor() {
    // 3838-3846: friendlies give NORMAL and are targeted (for their names).
    let c = choose_cursor(&CursorContext {
        target: Some(target(ObjectClass::Mech, Relation::Friendly)),
        ..ctx()
    });
    assert_eq!(c.cursor, NORMAL);
    assert!(c.targeted);
}

#[test]
fn capturable_building_with_a_ground_mech_gives_capture() {
    // 3792-3811: a capturable target with a selected ground mech shows CAPTURE (35).
    let c = choose_cursor(&CursorContext {
        selected: vec![SelectedMover {
            mech_on_ground: true,
            capture_blocked: false,
        }],
        target: Some(Target {
            captureable: true,
            ..target(ObjectClass::Building, Relation::Enemy)
        }),
        ..ctx()
    });
    assert_eq!(c.cursor, CAPTURE);
}

#[test]
fn capture_blocked_gives_the_blocked_capture_cursor() {
    // 3800-3806: a capture blocker makes XCAPTURE (36) and the loop breaks.
    let c = choose_cursor(&CursorContext {
        selected: vec![SelectedMover {
            mech_on_ground: true,
            capture_blocked: true,
        }],
        target: Some(Target {
            captureable: true,
            ..target(ObjectClass::Building, Relation::Enemy)
        }),
        ..ctx()
    });
    assert_eq!(c.cursor, XCAPTURE);
}

#[test]
fn friendly_repair_bay_gives_repair_when_repairable_and_xrepair_otherwise() {
    // 3886-3896: a friendly refit mech bay (CANREFIT and MECHBAY) gives REPAIR (31) or XREPAIR (32).
    let yes = choose_cursor(&CursorContext {
        target: Some(Target {
            refit_bay: true,
            can_repair_bay: true,
            ..target(ObjectClass::Building, Relation::Friendly)
        }),
        ..ctx()
    });
    assert_eq!(yes.cursor, REPAIR);
    let no = choose_cursor(&CursorContext {
        target: Some(Target {
            refit_bay: true,
            can_repair_bay: false,
            ..target(ObjectClass::Building, Relation::Friendly)
        }),
        ..ctx()
    });
    assert_eq!(no.cursor, XREPAIR);
}

#[test]
fn guard_mode_over_an_enemy_gives_the_guard_cursor() {
    // 3922-3926: controlGui.getGuard() overrides the cursor with GUARD (39) and targets it.
    let c = choose_cursor(&CursorContext {
        controls: Controls {
            guard: true,
            ..Controls::default()
        },
        target: Some(target(ObjectClass::Mech, Relation::Enemy)),
        ..ctx()
    });
    assert_eq!(c.cursor, GUARD);
    assert!(c.targeted);
}

#[test]
fn salvage_mode_gives_salvage_or_xsalvage() {
    // 3755-3766 (control salvage mode): SALVAGE (37) or XSALVAGE (38) by canSalvage.
    let yes = choose_cursor(&CursorContext {
        controls: Controls {
            salvage: true,
            ..Controls::default()
        },
        target: Some(Target {
            can_salvage: true,
            ..target(ObjectClass::Other, Relation::NoTeam)
        }),
        ..ctx()
    });
    assert_eq!(yes.cursor, SALVAGE);
    let no = choose_cursor(&CursorContext {
        controls: Controls {
            salvage: true,
            ..Controls::default()
        },
        target: Some(target(ObjectClass::Other, Relation::NoTeam)),
        ..ctx()
    });
    assert_eq!(no.cursor, XSALVAGE);
}

#[test]
fn waypoint_key_over_a_target_gives_dont() {
    // 3711-3713: the waypoint key short-circuits to DONT and does not target.
    let c = choose_cursor(&CursorContext {
        waypoint_key_down: true,
        target: Some(target(ObjectClass::Mech, Relation::Enemy)),
        ..ctx()
    });
    assert_eq!(c.cursor, DONT);
    assert!(!c.targeted);
}

#[test]
fn bridge_under_the_cursor_is_cleared_and_gives_the_no_target_cursor() {
    // 3909-3912: a bridge clears the target and calls makeNoTargetCursor(passable=false, los).
    let c = choose_cursor(&CursorContext {
        line_of_sight: true,
        target: Some(target(ObjectClass::Bridge, Relation::NoTeam)),
        ..ctx()
    });
    assert_eq!(c.cursor, DONT);
    assert!(c.target_cleared);
    assert!(!c.targeted);
}

#[test]
fn landbridge_without_fire_uses_the_terrain_passability() {
    // 3858-3880: a landbridge with no force fire clears the target; the no-target cursor then uses
    // the passability under the cursor (here passable, so WALK1 + rotation).
    let c = choose_cursor(&CursorContext {
        rotation: 4,
        target: Some(Target {
            landbridge: true,
            landbridge_passable: true,
            ..target(ObjectClass::Building, Relation::NoTeam)
        }),
        ..ctx()
    });
    assert_eq!(c.cursor, WALK1 + 4);
    assert!(c.target_cleared);
}

#[test]
fn no_selection_over_an_enemy_keeps_the_normal_cursor() {
    // 3706-3707: with no movers or non-movers the base cursor is NORMAL, and the enemy branch
    // leaves it alone.
    let c = choose_cursor(&CursorContext {
        movers: 0,
        target: Some(target(ObjectClass::Mech, Relation::Enemy)),
        ..ctx()
    });
    assert_eq!(c.cursor, NORMAL);
}

#[test]
fn energy_weapons_forcing_fire_on_empty_ground_use_the_energy_cursor() {
    // 3994 and 3941-3942 through makeNoTargetCursor: forcing fire returns the range cursor.
    let c = choose_cursor(&CursorContext {
        force_fire_held: true,
        energy_weapons: true,
        ..ctx()
    });
    assert_eq!(c.cursor, ENERGY_WEAPONS);
}

#[test]
fn guard_over_impassable_ground_gives_dont_for_the_no_target_path() {
    // 4113-4117 again on the no-target path, here with LOS to show GUARD_LOS is also 39.
    let c = choose_cursor(&CursorContext {
        line_of_sight: true,
        passable: false,
        controls: Controls {
            guard: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(c.cursor, DONT);
}

#[test]
fn guard_tower_gives_its_cursor_with_and_without_los() {
    // 4119-4121: a guard tower on any ground is GUARDTOWER_LOS (41) or GUARDTOWER (40).
    let los = choose_cursor(&CursorContext {
        line_of_sight: true,
        controls: Controls {
            guard_tower: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(los.cursor, GUARDTOWER_LOS);
    let plain = choose_cursor(&CursorContext {
        controls: Controls {
            guard_tower: true,
            ..Controls::default()
        },
        ..ctx()
    });
    assert_eq!(plain.cursor, GUARDTOWER);
}

#[test]
fn hot_key_list_reflects_listed_commands_only() {
    // optionsarea.cpp:1288-1290: an item per command with a description id (not -1).
    let hk = hot(ControlStyle::Aoe);
    let list = HotKeyList::from_bindings(&hk);
    assert!(list.len() > 30, "{}", list.len());
}

#[test]
fn fire_range_and_mouse_states_are_the_c_values() {
    // userinput.h:33-79: spot-check ids used by the cursor choice.
    assert_eq!(NORMAL, 21);
    assert_eq!(DONT, 30);
    assert_eq!(WALK1, 48);
    assert_eq!(WALKLOS1, 80);
    assert_eq!(RUN1, 112);
    assert_eq!(JUMP1, 176);
    assert_eq!(SHRTRNG_LOS, 242);
    assert_eq!(LONGRNG_LOS, 240);
}

#[test]
fn help_overlay_rows_follow_the_listed_commands() {
    // keyboardref.cpp:110-160: one row per listed command; modifiers are appended after the key
    // name (" + Shift", " + Control", " + Alt").
    let hk = hot(ControlStyle::Aoe);
    let Some(strings) = mc2_ui::strings::StringTable::builtin_or_skip() else {
        return;
    };
    let rows = mc2_ui::mission_keys::help_rows(&hk, strings);
    assert_eq!(rows.len(), HotKeyList::from_bindings(&hk).len());
    // 38 LEFT|SHIFT|0x100 rotateLeft (IDS_HOTKEY_ROTATEL 43246), keyboardref.cpp:157-166.
    let rotate_left = rows
        .iter()
        .find(|r| r.description == strings.load(43246))
        .expect("row");
    assert_eq!(
        rotate_left.key,
        format!("Cursor Left + {}", strings.load(43228))
    );
    // 0 S attackShort (43200): the key name alone.
    assert_eq!(rows[0].key, "S");
    assert_eq!(rows[0].description, strings.load(43200));
    // 43 LEFT|CTRL rotateLightLeft has description -1, so no row shows "Cursor Left + Control".
    assert!(rows
        .iter()
        .all(|r| r.key != format!("Cursor Left + {}", strings.load(43229))));
}

#[test]
fn plain_home_fires_both_home_entries_as_the_c_does() {
    // 210 KEY_HOME cameraNormal and 240 KEY_HOME|0x100 centerCamera. getKeyDown tests the low
    // byte (missiongui.cpp:1755, `key & 0x000000ff`), so both match a plain Home press. Each
    // single-click check then reads its own index (`key & 0x000fffff`), neither held.
    let hk = hot(ControlStyle::Aoe);
    let d = dispatch(&hk, &frame(&[(KEY_HOME, KeyStatus::Pressed)]));
    assert_eq!(
        commands(&d),
        vec![
            (MissionCommand::CameraNormal, 12, false),
            (MissionCommand::CenterCamera, 42, false)
        ]
    );
    assert_eq!(d.cursor, None);
}
