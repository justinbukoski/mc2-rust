// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! In-mission mouse selection and click orders: `MissionInterfaceManager` in `code/missiongui.cpp`.
//!
//! * [`DragSelect`]: the box drag of `doDrag` (missiongui.cpp:3554-3644) and the box test of
//!   `GameObjectManager::moverInRect` (`code/objmgr.cpp:2384`), as [`in_rect`].
//! * [`select_visible`]: `selectVisible` and `addVisibleToSelection` (2077-2115).
//! * [`force_group_changes`] and [`make_force_group`]: `selectForceGroup` (3249-3282) and
//!   `makeForceGroup` (3284-3307).
//! * [`frame`]: one frame of the AOE mouse profile (`useLeftRightMouseProfile`, the default
//!   `mechcmd2.cpp:106`). The double-click latch (missiongui.cpp:762-824), the left click
//!   (`selectClicked`, updateAOEStyle 1462-1670) and the right click (the order chain of the same
//!   function, with `handleOrders` 2462-2531 and `doAttack` 1801, `doMove` 3130, `doJump` 1969,
//!   `doGuard` 2030, `doSalvage` 4470, `doRepair` 4523, `doRepairBay` 4557).
//!
//! Plain values in, plain values out. The app fills [`UnitView`]s and a [`ClickContext`] from its
//! objects, projects world points to the screen and applies the returned intents; nothing here
//! calls the sim, and nothing here executes an order. Not ported, reported as `Unavailable` and
//! never faked: the MC mouse profile (`updateOldStyle`, 1267-1460), vehicle and salvage-craft drop
//! (`beginVtol`), airstrikes (`addAirstrike`), guard towers (`doGuardTower`), minelaying
//! (`MOVE_MODE_MINELAYING`) and the patrol path helper `makePatrolPath` (3176), whose calls are
//! commented out in the C++ (1282 and 1476). The waypoint key is `setWayPointKey` (5371), read through
//! [`waypoint_key_down`]. Cursors belong to `mission_keys`, hot keys too.

use mc2_platform::input::KeyStatus;

use crate::mission_keys::{
    ControlStyle, Controls, ForceGroup, HotKeys, KeyFrame, MissionCommand, Relation, SelectedMover,
    Target,
};

/// The app's handle for a mover or building (`GameObject` in the C++).
pub type ObjId = u32;

/// `MAX_ICONS` (`code/forcegroupbar.h:21`): the most movers one drag box selects
/// (`objsInRect[MAX_ICONS]`, missiongui.cpp:3605).
pub const MAX_BOX: usize = 16;

/// The number of force groups (the digit keys 0-9, `missiongui.cpp:1740-1753`, and the
/// `isInUnitGroup(i)` loop of the double-click, 762-824).
pub const FORCE_GROUPS: u8 = 10;

/// The reason an intent is unavailable in this slice (the MC mouse profile).
const MC_PROFILE: &str = "updateOldStyle (MC mouse profile, missiongui.cpp:1267) not ported";

/// The reason an issued aimed shot is unavailable: `aimLeg`, `aimArm` and `aimHead` order
/// `TACTICAL_ORDER_ATTACK_OBJECT` with an `aimLocation` (missiongui.cpp:2134-2157), which the
/// attack order does not carry.
const AIMED_SHOT: &str =
    "aimed shot (aimLocation on ATTACK_OBJECT, missiongui.cpp:2134) not ported";

/// The reason a consumed hot key command is unavailable when its order is not ported. Its click
/// still ends the click chain (updateAOEStyle 1486), so it never falls through to a move or attack.
const HOT_KEY_ORDER: &str = "hot key command consumes the click, its order is not ported";

/// The reason a frame with more than one order from `update()` is unavailable: one
/// [`OrderIntent`] cannot carry them all, and none of them is dropped silently.
const SEVERAL_EFFECTS: &str =
    "several hot key commands or the waypoint release on one frame, one order cannot carry them";

/// One object of the scene as the selection code reads it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitView {
    pub id: ObjId,
    /// `getTeam() == Team::home` (the first `moverInRect` test).
    pub team_home: bool,
    /// `getExists()` and a non-null appearance (`moverInRect`, objmgr.cpp:2394-2400).
    pub in_view: bool,
    /// `getAppearance()->getScreenPos()`.
    pub screen: [f32; 2],
    /// `getCommanderId() == Commander::home->getId()`.
    pub own: bool,
    pub disabled: bool,
    /// `isOnGUI()`.
    pub on_gui: bool,
    pub selected: bool,
    /// `isInUnitGroup(i)` as bit `i`.
    pub groups: u16,
}

/// The object under the cursor (`MissionInterfaceManager::target`), with the facts the click
/// chain reads.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClickTarget {
    pub id: ObjId,
    /// The cursor code's facts (`mission_keys::Target`): relation, disabled, `is_mover`,
    /// `captureable`, `can_salvage` (`canSalvage`), `can_repair` (`canRepair`), `refit_bay`,
    /// `can_repair_bay` (`canRepairBay`). The app computes the `can_*` predicates.
    pub facts: Target,
    /// `getTeam() == Team::home`. The attack test needs it apart from `Relation::Friendly`, which
    /// also covers allies (`doAttack`, missiongui.cpp:1819-1835).
    pub home_team: bool,
    /// `getCommanderId() == Commander::home->getId()`.
    pub own: bool,
    /// `isSelected()`.
    pub selected: bool,
    /// `CONTACT_VISUAL == getContactStatus(home, true)` (missiongui.cpp:1617-1621).
    pub visible_contact: bool,
    /// `BattleMech` on `MOVETYPE_GROUND`: the only target the aimed-shot commands aim at
    /// (aimLeg 2125, aimArm 2175, aimHead 2227). Other targets fail their test.
    pub ground_battlemech: bool,
    /// Both legs destroyed (`aimLeg` 2131-2132). Read only for a ground BattleMech.
    pub both_legs_destroyed: bool,
    /// `isInUnitGroup(i)` as bit `i`.
    pub groups: u16,
}

/// What the click and command state is, for one frame. Filled by the app from the same frame as
/// [`DragInput`]. Every field is named after the C++ expression it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct ClickContext {
    /// `useLeftRightMouseProfile`.
    pub style: ControlStyle,
    /// `bGui`: the cursor is over the control panel.
    pub gui: bool,
    /// `shiftDn`.
    pub shift: bool,
    /// `leftMouseReleased()`.
    pub left_release: bool,
    /// `isLeftDoubleClick()`.
    pub left_double: bool,
    /// `getMouseLeftButtonState() == MC2_MOUSE_DOWN` (the double-click latch reset, 906-907).
    pub left_down: bool,
    /// `rightMouseReleased() && !wasRightDrag()`: the AOE command click.
    pub right_release: bool,
    /// `getKeyDown(KEY_T)`: the terrain key suppresses select clicks (853).
    pub key_t: bool,
    /// `wPos`: the world point under the cursor.
    pub point: [f32; 3],
    /// `Terrain::IsGameSelectTerrainPosition(wPos)`.
    pub terrain_select: bool,
    /// `passable`: every selected mover can move to `wPos`.
    pub passable: bool,
    /// `selectionIsHelicopters()`.
    pub helicopters: bool,
    /// `getKeyDown(WAYPOINT_KEY)`, see [`waypoint_key_down`].
    pub waypoint_key_down: bool,
    /// The waypoint key was released this frame (`Dispatch::waypoints_done`, from the same key
    /// frame as `hot_keys`). `update()` sends the selected movers their waypoints-done order and
    /// returns 1, which ends the click chain (missiongui.cpp:1711-1731, 1486).
    pub waypoint_released: bool,
    /// `controlGui.updateChat()`: the chat line has the keyboard. `update()` returns 1 while it
    /// does (missiongui.cpp:1703), which ends the click chain.
    pub chatting: bool,
    /// `controlGui.getWalk()`: the walk speed (else run).
    pub walk: bool,
    /// The `ControlGui` toggles (jump, guard, salvage, repair, ...).
    pub controls: Controls,
    /// `bEnergyWeapons`.
    pub energy_weapons: bool,
    /// `controlGui.getFireFromCurrentPos()`.
    pub fire_from_current_pos: bool,
    /// The hot key commands the dispatch ran this frame, in table order (`update`,
    /// missiongui.cpp:1700-1795). Each handler's return value, which ends the click chain when it
    /// is 1 (1690), is derived from the command and the click facts by [`hot_key_consumes`]; the
    /// caller does not supply it. A frame whose commands would give two orders reports
    /// `Unavailable` (see [`update_click`]).
    pub hot_keys: Vec<MissionCommand>,
    /// `bAimedShot`: an aimed shot is set (`aimLeg`, `aimArm`, `aimHead`).
    pub aimed_shot: bool,
    /// `anySelectedWithoutAreaEffect()`: a selected mover has a weapon that is not area effect
    /// (aimLeg 2125, aimArm 2178, aimHead 2229).
    pub any_without_area_effect: bool,
    /// `paintingVtol[commander]`.
    pub painting_vtol: bool,
    /// `canAddVehicle(wPos)`.
    pub can_add_vehicle: bool,
    /// `canRecover(wPos)`.
    pub can_recover: bool,
    /// `canJumpToWPos()`.
    pub can_jump_to_point: bool,
    /// The home commander's selected movers, in roster order.
    pub selected: Vec<SelectedMover>,
    /// The object under the cursor (`updateTarget`).
    pub target: Option<ClickTarget>,
}

impl Default for ClickContext {
    fn default() -> Self {
        Self {
            style: ControlStyle::Aoe,
            gui: false,
            shift: false,
            left_release: false,
            left_double: false,
            left_down: false,
            right_release: false,
            key_t: false,
            point: [0.0; 3],
            terrain_select: false,
            passable: false,
            helicopters: false,
            waypoint_key_down: false,
            waypoint_released: false,
            chatting: false,
            walk: false,
            controls: Controls::default(),
            energy_weapons: false,
            fire_from_current_pos: false,
            hot_keys: Vec::new(),
            aimed_shot: false,
            any_without_area_effect: false,
            painting_vtol: false,
            can_add_vehicle: false,
            can_recover: false,
            can_jump_to_point: false,
            selected: Vec::new(),
            target: None,
        }
    }
}

/// `keyboard`'s waypoint modifier: `getKeyDown(WAYPOINT_KEY)` with the key set by
/// `setWayPointKey` (missiongui.cpp:5371), which [`HotKeys::waypoint_key`] holds.
pub fn waypoint_key_down(hk: &HotKeys, keys: &KeyFrame) -> bool {
    keys.key_down(hk.waypoint_key())
}

/// The selection change one frame asks for.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SelectionIntent {
    #[default]
    Unchanged,
    /// A click that finds nothing selectable: the invalid-action sound, no change (`selectClicked`
    /// on empty ground, missiongui.cpp:1663-1667).
    Denied,
    /// Click on an own mover without shift: every home mover is deselected and this one selected
    /// (missiongui.cpp:1647-1658).
    SelectOnly(ObjId),
    /// Shift-click on an own object: `select` is true when the object was not selected
    /// (missiongui.cpp:1630-1645).
    Toggle { id: ObjId, select: bool },
    /// A drag box (`doDrag`, missiongui.cpp:3603-3628). `replace` clears every mover first (no
    /// shift). `ids` are at most [`MAX_BOX`], in roster order.
    Box { ids: Vec<ObjId>, replace: bool },
    /// `selectVisible` (`clear_home`) or `addVisibleToSelection`: every own, enabled, on-screen
    /// unit, with no cap (2093-2115).
    Visible { ids: Vec<ObjId>, clear_home: bool },
    /// A double-click on a grouped mover (missiongui.cpp:808-824): `selectForceGroup`, see
    /// [`force_group_changes`].
    Groups(ForceGroup),
    /// The MC mouse profile (not ported).
    Unavailable(&'static str),
}

/// The info window (`controlGui.setInfoWndMover`, `cancelInfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InfoWindow {
    #[default]
    Keep,
    /// `setInfoWndMover(NULL)`.
    Clear,
    /// `setInfoWndMover(mover)`.
    Set(ObjId),
    /// `controlGui.cancelInfo()` after a click on nothing, leaving the info-object pick mode.
    CancelPick,
}

/// The order a click asks for (`handleOrders` and the `do*` helpers). Execution is the app's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum OrderKind {
    /// No order and no sound.
    #[default]
    None,
    /// The action was refused: `playDigitalSample(INVALID_GUI)` and no order.
    Denied,
    /// `doMove` (3130-3170): `TACTICAL_ORDER_MOVETO_POINT`, run or walk.
    MoveTo { point: [f32; 3], walk: bool },
    /// `doJump` (1969-2014): `TACTICAL_ORDER_JUMPTO_POINT`.
    Jump { point: [f32; 3] },
    /// `doGuard(NULL)` (2030-2059): a guard on a point, run or walk.
    GuardPoint { point: [f32; 3], walk: bool },
    /// `doGuard(target)`: `TACTICAL_ORDER_GUARD` on the object.
    GuardUnit { target: ObjId },
    /// `doAttack` (1801-1900): `TACTICAL_ORDER_ATTACK_OBJECT` on an enemy object. The pursue flag
    /// is per mover (`attackRange == FIRERANGE_CURRENT`, 1882-1888) and is applied at execution.
    /// `walk` is the travel mode (1827, `getWalk() ? TRAVEL_MODE_SLOW : TRAVEL_MODE_FAST`).
    Attack {
        target: ObjId,
        conserve_ammo: bool,
        fire_from_current_pos: bool,
        walk: bool,
    },
    /// `doAttack` on a capturable object: `TACTICAL_ORDER_CAPTURE` (1870 sets the travel mode).
    Capture { target: ObjId, walk: bool },
    /// `forceShot` (3216-3239) with an object under the cursor: `TACTICAL_ORDER_ATTACK_OBJECT` on
    /// it, whatever its team (3224), with `ATTACK_CONSERVING_AMMO` for energy weapons (3228) and
    /// `pursue` off when firing from the current position (3231). `handleOrders` (2462) sets the
    /// range per mover, and the order is queued while the waypoint key is held.
    ForceFireObject {
        target: ObjId,
        conserve_ammo: bool,
        fire_from_current_pos: bool,
        walk: bool,
    },
    /// The waypoint key was released: `TACTICAL_ORDER_WAYPOINTS_DONE` to each selected home mover
    /// that is not running its queue (missiongui.cpp:1714-1731). Sent directly, never queued.
    WaypointsDone,
    /// `forceShot` with no object under the cursor: `TACTICAL_ORDER_ATTACK_POINT` at the world
    /// point (3226-3227). The other fields are as [`OrderKind::ForceFireObject`].
    ForceFirePoint {
        point: [f32; 3],
        conserve_ammo: bool,
        fire_from_current_pos: bool,
        walk: bool,
    },
    /// `doSalvage` (4470-4486): `TACTICAL_ORDER_RECOVER`.
    Salvage { target: ObjId },
    /// `doRepair` (4523-4538): `TACTICAL_ORDER_REFIT`.
    Repair { target: ObjId, walk: bool },
    /// `doRepairBay` (4557-4572): `TACTICAL_ORDER_GETFIXED`.
    GetFixed { target: ObjId, walk: bool },
    /// Explicitly outside this slice (never silently faked).
    Unavailable(&'static str),
}

/// An order with its `handleOrders` routing. `queued` is true when the waypoint key is held: the
/// order is queued for the unit instead of replacing its orders (missiongui.cpp:2513-2516).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OrderIntent {
    pub kind: OrderKind,
    pub queued: bool,
}

/// What one frame decided.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Resolution {
    pub selection: SelectionIntent,
    pub order: OrderIntent,
    pub info: InfoWindow,
}

/// `doDrag`'s inputs for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragInput {
    /// `gos_GetKeyStatus(KEY_LMOUSE)`.
    pub status: KeyStatus,
    /// `bGui`.
    pub gui: bool,
    /// `cameraClicked`: the camera key (Left Alt in the AOE profile) is held.
    pub camera: bool,
    /// `shiftDn`.
    pub shift: bool,
    /// `wPos`: the world point under the cursor.
    pub world: [f32; 3],
    /// `mouseX`, `mouseY`: the cursor in screen pixels.
    pub mouse: [f32; 2],
}

/// The mouse state that lives across frames: the drag and the double-click latch.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SelectState {
    pub drag: DragSelect,
    last_double: bool,
}

impl SelectState {
    /// `lastUpdateDoubleClick`.
    pub fn last_double(&self) -> bool {
        self.last_double
    }
}

/// `dragStart`, `dragEnd` and `isDragging` (missiongui.h). A `start` whose x and y are both zero is
/// "not set", the C++ `Zero()` test (`!dragStart.x`, `!(dragStart.x == 0 && dragStart.y == 0)`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DragSelect {
    start: [f32; 3],
    end: [f32; 2],
    dragging: bool,
}

impl DragSelect {
    /// `isDragging`.
    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// `doDrag` (missiongui.cpp:3554-3644). `project` is `eye->projectZ`: the screen point of a
    /// world point. Returns the box selection on the release of a drag that caught at least one
    /// unit; an empty box changes nothing (3613, `if (count)`).
    pub fn update(
        &mut self,
        input: DragInput,
        units: &[UnitView],
        project: impl Fn([f32; 3]) -> [f32; 2],
    ) -> Option<SelectionIntent> {
        let start = self.start;
        if input.status == KeyStatus::Pressed && !input.gui && !input.camera && start[0] == 0.0 {
            // missiongui.cpp:3559-3566: the press point is recorded; no drag yet.
            self.start = input.world;
            self.dragging = false;
        } else if input.status == KeyStatus::Held
            && !self.dragging
            && !(start[0] == 0.0 && start[1] == 0.0)
            && start[0] != input.world[0]
            && start[1] != input.world[1]
        {
            // missiongui.cpp:3569-3573: the drag starts once the cursor differs from the press
            // point on both axes (`&&`: a move along one axis alone never starts one).
            self.dragging = true;
            self.end = input.mouse;
        } else if self.dragging {
            self.end = input.mouse;
            if matches!(input.status, KeyStatus::Released | KeyStatus::Free) {
                // missiongui.cpp:3583-3637: select everything in the box, deselect the rest.
                let picked = box_selection(project(self.start), self.end, units, input.shift);
                self.start = [0.0; 3];
                self.end = [0.0; 2];
                self.dragging = false;
                return picked;
            }
        } else if input.status == KeyStatus::Released {
            // missiongui.cpp:3640-3642.
            self.start = [0.0; 3];
        }
        None
    }
}

/// `moverInRect` (objmgr.cpp:2384-2413) on screen points. The box corners are `long`s, so each
/// corner truncates toward zero, and the test includes both edges.
pub fn in_rect(start: [f32; 2], end: [f32; 2], p: [f32; 2]) -> bool {
    let left = start[0].min(end[0]) as i64 as f32;
    let right = start[0].max(end[0]) as i64 as f32;
    let top = start[1].min(end[1]) as i64 as f32;
    let bottom = start[1].max(end[1]) as i64 as f32;
    left <= p[0] && p[0] <= right && top <= p[1] && p[1] <= bottom
}

/// `moverInRect` for one unit: the home team, an existing unit with an appearance, and the screen
/// point in the box.
fn unit_in_rect(unit: &UnitView, start: [f32; 2], end: [f32; 2]) -> bool {
    unit.team_home && unit.in_view && in_rect(start, end, unit.screen)
}

/// The box membership of `doDrag` (3603-3628): own, enabled, in the box, at most [`MAX_BOX`].
fn box_selection(
    start: [f32; 2],
    end: [f32; 2],
    units: &[UnitView],
    shift: bool,
) -> Option<SelectionIntent> {
    let ids: Vec<ObjId> = units
        .iter()
        .filter(|u| unit_in_rect(u, start, end) && u.own && !u.disabled)
        .take(MAX_BOX)
        .map(|u| u.id)
        .collect();
    if ids.is_empty() {
        return None;
    }
    Some(SelectionIntent::Box {
        ids,
        replace: !shift,
    })
}

/// `selectVisible` (`clear_home`) and `addVisibleToSelection` (2077-2115): every own, enabled
/// unit whose screen point is inside `[0, screen.0] x [0, screen.1]`, with no cap. The drag points
/// are spent (`dragStart.Zero()` 2097 and `dragEnd.Zero()` 2110), so a press still held cannot
/// start a box afterwards. `isDragging` is left as it is, as in the C++.
pub fn select_visible(
    state: &mut SelectState,
    units: &[UnitView],
    screen: [f32; 2],
    clear_home: bool,
) -> SelectionIntent {
    let ids = units
        .iter()
        .filter(|u| unit_in_rect(u, [0.0; 2], screen) && u.own && !u.disabled)
        .map(|u| u.id)
        .collect();
    state.drag.start = [0.0; 3];
    state.drag.end = [0.0; 2];
    SelectionIntent::Visible { ids, clear_home }
}

/// `selectForceGroup` (3249-3282): the changes to the own, on-GUI units. `exclusive` (`deselect`)
/// clears the rest and selects the enabled members. Otherwise the group toggles: if every enabled
/// member is selected they are all deselected, else all are selected. Only changed units appear.
pub fn force_group_changes(units: &[UnitView], group: u8, exclusive: bool) -> Vec<(ObjId, bool)> {
    assert!(group < FORCE_GROUPS, "force group {group} out of range");
    let bit = 1u16 << group;
    let member = |u: &UnitView| u.groups & bit != 0 && !u.disabled && u.on_gui && u.own;
    let all_selected = units.iter().filter(|u| member(u)).all(|u| u.selected);
    units
        .iter()
        .filter(|u| u.own && u.on_gui)
        .filter_map(|u| {
            let new = if exclusive {
                member(u)
            } else if member(u) {
                !all_selected
            } else {
                u.selected
            };
            (new != u.selected).then_some((u.id, new))
        })
        .collect()
}

/// `makeForceGroup` (3284-3307): the group becomes exactly the own selected units. Returns the new
/// group mask of each own unit whose mask changes.
pub fn make_force_group(units: &[UnitView], group: u8) -> Vec<(ObjId, u16)> {
    assert!(group < FORCE_GROUPS, "force group {group} out of range");
    let bit = 1u16 << group;
    units
        .iter()
        .filter(|u| u.own)
        .filter_map(|u| {
            let mask = (u.groups & !bit) | if u.selected { bit } else { 0 };
            (mask != u.groups).then_some((u.id, mask))
        })
        .collect()
}

/// One frame of the mouse, in the order of `MissionInterfaceManager::update`:
///
/// 1. the double-click on a grouped mover latches and selects its force group (762-824);
/// 2. with the camera key held the frame ends here: no click, no drag, no order, and the latch
///    is kept (855-866; the reset at 906 is skipped), except that a consumed hot key command still
///    gives its order (861, 852);
/// 3. the left click is read against the drag state before this frame's drag update (853);
/// 4. `doDrag` (3469, 3554);
/// 5. the click chain: `update()` (the waypoint release, the chat line and the hot key commands)
///    ends the click when it consumes it, as updateAOEStyle 1486 returns on its 1 (see
///    [`update_click`]); otherwise a right click gives an order (`else if` makes it exclusive of
///    the left click), a left click selects, and nothing happens over the control panel (`bGui`).
///
/// The selection is the first of double-click, drag box and click, so one frame carries one
/// selection change. The left-button latch is reset at the end (906-907).
pub fn frame(
    state: &mut SelectState,
    drag: DragInput,
    project: impl Fn([f32; 3]) -> [f32; 2],
    units: &[UnitView],
    ctx: &ClickContext,
) -> Resolution {
    let mut double = None;
    if ctx.left_double {
        if let Some(t) = ctx.target.filter(|t| t.facts.is_mover && t.home_team) {
            state.last_double = true;
            if let Some(group) = (0..FORCE_GROUPS).find(|g| t.groups & (1 << g) != 0) {
                double = Some(SelectionIntent::Groups(ForceGroup::Select {
                    group,
                    exclusive: true,
                }));
            }
        }
    }

    if drag.camera {
        // missiongui.cpp:855-866 and 878-884: with the camera key held, moveCameraAround returns
        // true and update() returns before selectClicked, doDrag and updateAOEStyle. The
        // double-click above has already run (762-824), and the latch reset at 906-907 is skipped
        // by that return, so the latch is left alone here too. The camera branch still runs the
        // hot keys (861), so a consumed command click still gives its order (`commandClicked`, 852).
        let order = match update_click(ctx) {
            Some(order) if ctx.style == ControlStyle::Aoe => order,
            _ => OrderIntent::default(),
        };
        return Resolution {
            selection: double.unwrap_or_default(),
            order,
            ..Resolution::default()
        };
    }

    let click = ctx.left_release
        && !ctx.left_double
        && !state.last_double
        && !ctx.key_t
        && !state.drag.dragging();
    let boxed = state.drag.update(drag, units, project);

    let mut res = Resolution::default();
    if ctx.style == ControlStyle::Mc {
        if ctx.left_release || ctx.right_release || ctx.left_double {
            res.selection = SelectionIntent::Unavailable(MC_PROFILE);
            if ctx.right_release {
                res.order = unavailable(MC_PROFILE, false);
            }
        }
    } else if let Some(order) = update_click(ctx) {
        // updateAOEStyle 1486: update() runs the hot keys and the waypoint release, and a return of
        // 1 ends updateAOEStyle, so the rest of it (the GUI test, placement, the order chain and the
        // left click) does not run.
        res.order = order;
    } else if !ctx.gui {
        if ctx.right_release {
            let (order, info) = resolve_right(ctx);
            res.order = order;
            res.info = info;
        } else if click {
            let (selection, info, order) = resolve_left(ctx);
            res.selection = selection;
            res.info = info;
            res.order = order;
        }
    }
    if let Some(group) = double {
        res.selection = group;
    } else if let Some(picked) = boxed {
        if let SelectionIntent::Box { ids, .. } = &picked {
            // missiongui.cpp:3613-3634: the info window clears, then shows a lone unit.
            res.info = match ids.as_slice() {
                [only] => InfoWindow::Set(*only),
                _ => InfoWindow::Clear,
            };
        }
        res.selection = picked;
    }

    if !ctx.left_double && !(state.last_double && ctx.left_down) {
        state.last_double = false;
    }
    res
}

fn order(kind: OrderKind, queued: bool) -> OrderIntent {
    // Only the orders `handleOrders` routes can be queued (2462-2531). doAttack and capture refuse
    // or never reach it in waypoint mode, forceShot does reach it (3239), and the others carry no
    // order at all.
    let routed = matches!(
        kind,
        OrderKind::MoveTo { .. }
            | OrderKind::Jump { .. }
            | OrderKind::GuardPoint { .. }
            | OrderKind::GuardUnit { .. }
            | OrderKind::Salvage { .. }
            | OrderKind::Repair { .. }
            | OrderKind::GetFixed { .. }
            | OrderKind::ForceFireObject { .. }
            | OrderKind::ForceFirePoint { .. }
    );
    OrderIntent {
        kind,
        queued: queued && routed,
    }
}

fn unavailable(reason: &'static str, queued: bool) -> OrderIntent {
    order(OrderKind::Unavailable(reason), queued)
}

/// The vehicle, salvage craft and airstrike placement tests, which come first in both the left
/// and the right click (missiongui.cpp:1517-1533 and 1593-1613). Placing is not ported, so a
/// placement that would succeed is `Unavailable`, a refused one `Denied`.
fn placement(ctx: &ClickContext) -> Option<OrderKind> {
    let c = &ctx.controls;
    let vehicle = c.adding_vehicle && !ctx.painting_vtol;
    let salvage = c.adding_salvage && !ctx.painting_vtol;
    if (vehicle && ctx.can_add_vehicle) || (salvage && ctx.can_recover) {
        return Some(OrderKind::Unavailable(
            "beginVtol (vehicle and salvage craft drop, missiongui.cpp:4240) not ported",
        ));
    }
    if (vehicle && !ctx.can_add_vehicle) || (salvage && !ctx.can_recover) {
        return Some(OrderKind::Denied);
    }
    if c.adding_airstrike && !ctx.painting_vtol {
        return Some(OrderKind::Unavailable(
            "addAirstrike (missiongui.cpp:4133) not ported",
        ));
    }
    None
}

/// The left click of the AOE profile (updateAOEStyle, 1589-1667 and the placement part above).
fn resolve_left(ctx: &ClickContext) -> (SelectionIntent, InfoWindow, OrderIntent) {
    if let Some(kind) = placement(ctx) {
        return (
            SelectionIntent::Unchanged,
            InfoWindow::Keep,
            order(kind, false),
        );
    }
    let Some(t) = ctx.target else {
        // missiongui.cpp:1663-1667: cancelInfo and the invalid sound.
        return (
            SelectionIntent::Denied,
            InfoWindow::CancelPick,
            OrderIntent::default(),
        );
    };
    // missiongui.cpp:1617-1621: the info window follows a visible or home or disabled mover.
    let info = if t.facts.is_mover && (t.home_team || t.visible_contact || t.facts.disabled) {
        InfoWindow::Set(t.id)
    } else {
        InfoWindow::Keep
    };
    let selection = if t.own && !t.facts.disabled {
        if ctx.shift {
            SelectionIntent::Toggle {
                id: t.id,
                select: !t.selected,
            }
        } else if t.facts.is_mover {
            SelectionIntent::SelectOnly(t.id)
        } else {
            SelectionIntent::Unchanged
        }
    } else {
        SelectionIntent::Unchanged
    };
    (selection, info, OrderIntent::default())
}

/// The right click of the AOE profile: the order chain of updateAOEStyle (1514-1586), with its
/// `doMove`, `doAttack` and the helpers it calls.
fn resolve_right(ctx: &ClickContext) -> (OrderIntent, InfoWindow) {
    if let Some(kind) = placement(ctx) {
        return (order(kind, false), InfoWindow::Keep);
    }
    let Some(t) = ctx.target else {
        if ctx.controls.info_select {
            // missiongui.cpp:1541-1544: the invalid sound, then cancelInfo() (leaves the pick).
            return (order(OrderKind::Denied, false), InfoWindow::CancelPick);
        }
        return (resolve_empty_ground(ctx), InfoWindow::Keep);
    };
    let c = &ctx.controls;
    let f = &t.facts;
    let id = t.id;
    let wp = ctx.waypoint_key_down;
    let walk = ctx.walk;
    // missiongui.cpp:1569-1586, in the C++ order.
    if c.salvage {
        // doSalvage: only when canSalvage, else nothing (4470-4480).
        let kind = if f.can_salvage {
            OrderKind::Salvage { target: id }
        } else {
            OrderKind::None
        };
        return (order(kind, wp), InfoWindow::Keep);
    }
    if c.guard {
        return (
            order(OrderKind::GuardUnit { target: id }, wp),
            InfoWindow::Keep,
        );
    }
    if c.repair && f.can_repair {
        let kind = OrderKind::Repair { target: id, walk };
        return (order(kind, wp), InfoWindow::Keep);
    }
    if c.guard_tower {
        return (
            unavailable("doGuardTower (guard towers) not ported", false),
            InfoWindow::Keep,
        );
    }
    if f.relation == Relation::Friendly && f.refit_bay && f.can_repair_bay {
        let kind = OrderKind::GetFixed { target: id, walk };
        return (order(kind, wp), InfoWindow::Keep);
    }
    if c.info_select && f.is_mover {
        return (OrderIntent::default(), InfoWindow::Set(id));
    }
    if !t.home_team && !f.disabled {
        return (attack(ctx, &t), InfoWindow::Keep);
    }
    // 1585: the invalid sound unless an aimed shot is set (a forced shot never reaches this chain).
    if ctx.aimed_shot {
        (OrderIntent::default(), InfoWindow::Keep)
    } else {
        (order(OrderKind::Denied, false), InfoWindow::Keep)
    }
}

/// What one hot key command does to the click, as `update()` sees it.
enum HotKeyEffect {
    /// The command does not run, or it does not end the click.
    None,
    /// The command ends the click and gives no order.
    Consumed,
    /// The command ends the click and gives this order (or its unported effect).
    Order(OrderIntent),
}

/// What `update()` (missiongui.cpp:1690) does with the click this frame. `None` lets the click
/// chain run as usual. Otherwise the click ends (1486) and the result is the frame's order, which
/// is the default when no order was issued.
///
/// C++ runs every command of the frame, and every one of them can act: the waypoint release and
/// each consuming command, in table order. A frame has one [`OrderIntent`], so when more than one
/// of them issues an order the frame reports `Unavailable` instead of dropping all but the first.
fn update_click(ctx: &ClickContext) -> Option<OrderIntent> {
    // updateChat (1703) returns 1 while chatting, and the waypoint release (1731) sets bRetVal.
    let mut consumed = ctx.chatting || ctx.waypoint_released;
    let mut orders = Vec::new();
    if ctx.waypoint_released {
        orders.push(order(OrderKind::WaypointsDone, false));
    }
    for &command in &ctx.hot_keys {
        match hot_key_effect(ctx, command) {
            HotKeyEffect::None => {}
            HotKeyEffect::Consumed => consumed = true,
            HotKeyEffect::Order(order) => {
                consumed = true;
                orders.push(order);
            }
        }
    }
    match orders.as_slice() {
        [] => consumed.then(OrderIntent::default),
        [only] => Some(*only),
        _ => Some(unavailable(SEVERAL_EFFECTS, false)),
    }
}

/// The effect of one hot key command on the frame (see [`hot_key_consumes`] for the return value).
/// An aimed command with an issued shot is an order whatever its handler returned (see
/// [`aimed_order_issued`]); the other aimed commands only consume the click.
fn hot_key_effect(ctx: &ClickContext, command: MissionCommand) -> HotKeyEffect {
    use MissionCommand as M;
    if matches!(command, M::AimLeg | M::AimArm | M::AimHead) {
        return if aimed_order_issued(ctx, command) {
            HotKeyEffect::Order(unavailable(AIMED_SHOT, false))
        } else if hot_key_consumes(ctx, command) {
            HotKeyEffect::Consumed
        } else {
            HotKeyEffect::None
        };
    }
    if !hot_key_consumes(ctx, command) {
        return HotKeyEffect::None;
    }
    let order = hot_key_order(ctx, command);
    if order == OrderIntent::default() {
        HotKeyEffect::Consumed
    } else {
        HotKeyEffect::Order(order)
    }
}

/// The return value of a hot key handler on this frame, which is 1 when it ends the click
/// (`update`, missiongui.cpp:1690). Each arm is the handler's `return` in the C++, checked against
/// the commands[] table (missiongui.cpp:197-307): most handlers return 1 or 0 on every path, and
/// the click-dependent ones test the facts below.
///
/// * `commandClicked` is the right click (852) in the AOE profile, `ctx.right_release`.
/// * `attackShort`, `attackMedium` and `attackLong` return 1 only for `commandClicked && target`
///   (1904-1934); a target cleared under the control panel (1215-1219) gives 0.
/// * `forceShot` returns 1 only on `commandClicked` (3221); `fireFromCurrentPos` returns it.
/// * The aimed commands (2117-2273) return 1 on every path except the non-ground target, which
///   falls to `return 0` (2169), and an aimed shot that is issued, which `aimHead` alone returns 0
///   from (2273). `aimLeg` and `aimArm` return 1 after an issued shot too.
/// * `gotoNextNavMarker` (objectives) is not modelled: it is taken as consumed and reported
///   `Unavailable`, so the click chain never moves in its place.
fn hot_key_consumes(ctx: &ClickContext, command: MissionCommand) -> bool {
    use MissionCommand as M;
    match command {
        M::AttackShort | M::AttackMedium | M::AttackLong => {
            ctx.right_release && ctx.target.is_some()
        }
        M::ForceShot | M::FireFromCurrentPos => ctx.right_release,
        M::AimLeg => ctx.target.is_none_or(|t| t.ground_battlemech),
        M::AimArm => true,
        M::AimHead => !aimed_order_issued(ctx, command),
        M::EnergyWeapons
        | M::DefaultAttack
        | M::Jump
        | M::Guard
        | M::StopGuard
        | M::ConserveAmmo
        | M::ChangeSpeed
        | M::CenterCamera
        | M::VehicleCommand
        | M::ShowObjectives
        | M::ToggleCompass => false,
        M::StopJump
        | M::StopFireFromCurrentPos
        | M::SelectVisible
        | M::AddVisibleToSelection
        | M::CameraNormal
        | M::CameraView(_)
        | M::CameraAssign(_)
        | M::RemoveCommand
        | M::PowerDown
        | M::PowerUp
        | M::Eject
        | M::StopChangeSpeed
        | M::ScrollUp
        | M::ScrollDown
        | M::ScrollLeft
        | M::ScrollRight
        | M::ZoomOut
        | M::ZoomIn
        | M::RotateLeft
        | M::RotateRight
        | M::TiltUp
        | M::TiltDown
        | M::RotateLightLeft
        | M::RotateLightRight
        | M::RotateLightUp
        | M::RotateLightDown
        | M::TogglePause
        | M::ToggleHotKeys
        | M::SwitchTab
        | M::ReverseSwitchTab
        | M::InfoCommand
        | M::InfoButtonReleased
        | M::GotoNextNavMarker
        | M::SendAirstrike
        | M::SendSensorStrike
        | M::ToggleHoldPosition
        | M::ChatKey
        | M::TeamChatKey
        | M::Unavailable(_) => true,
    }
}

/// The order of a hot key command that consumed the click. The C++ handler runs before the click
/// chain, so none of the chain's tests apply: no GUI test, no placement, no team test (except
/// `doAttack`'s own), and no control toggle such as guard. A command whose order is not ported
/// is `Unavailable` and never falls back to a move, attack or guard.
fn hot_key_order(ctx: &ClickContext, command: MissionCommand) -> OrderIntent {
    match command {
        MissionCommand::ForceShot => forced_fire(ctx, ctx.fire_from_current_pos),
        // fireFromCurrentPos (missiongui.cpp:2016-2021) sets the fire-from-current-position toggle
        // and then returns forceShot(), so the shot is fired from the current position.
        MissionCommand::FireFromCurrentPos => forced_fire(ctx, true),
        // attackShort, attackMedium and attackLong (1904-1934): `doAttack` on the target, which
        // uses each mover's own range, not the key's (1882-1888).
        MissionCommand::AttackShort | MissionCommand::AttackMedium | MissionCommand::AttackLong => {
            match ctx.target {
                Some(t) => attack(ctx, &t),
                None => OrderIntent::default(),
            }
        }
        _ => unavailable(HOT_KEY_ORDER, false),
    }
}

/// Whether an aimed command issues its ATTACK_OBJECT with an aim location (`aimLeg` 2131-2157,
/// `aimArm` 2178-2196, `aimHead` 2229-2244). The command click is `commandClicked` (852), and the
/// target must be a ground BattleMech with a weapon that is not area effect among the selected.
/// `aimLeg` also needs a leg left (2131-2132); it aims the other one when one is gone (2146).
fn aimed_order_issued(ctx: &ClickContext, command: MissionCommand) -> bool {
    let Some(t) = ctx.target.filter(|t| t.ground_battlemech) else {
        return false;
    };
    let aimable = ctx.right_release && ctx.any_without_area_effect;
    match command {
        MissionCommand::AimLeg => aimable && !t.both_legs_destroyed,
        MissionCommand::AimArm | MissionCommand::AimHead => aimable,
        _ => false,
    }
}

/// `forceShot` (missiongui.cpp:3216-3239) on the command click: the force-fire order, at the
/// object under the cursor or at the world point. It has no team, placement or GUI test, and the
/// waypoint key queues it through `handleOrders`. `fire_from_current_pos` is the toggle as forceShot
/// reads it (3231), which `fireFromCurrentPos` sets before it calls forceShot.
fn forced_fire(ctx: &ClickContext, fire_from_current_pos: bool) -> OrderIntent {
    let conserve_ammo = ctx.energy_weapons;
    let walk = ctx.walk;
    let kind = match ctx.target {
        Some(t) => OrderKind::ForceFireObject {
            target: t.id,
            conserve_ammo,
            fire_from_current_pos,
            walk,
        },
        None => OrderKind::ForceFirePoint {
            point: ctx.point,
            conserve_ammo,
            fire_from_current_pos,
            walk,
        },
    };
    order(kind, ctx.waypoint_key_down)
}

/// The right click on ground, with no object (missiongui.cpp:1541-1561).
fn resolve_empty_ground(ctx: &ClickContext) -> OrderIntent {
    if !ctx.terrain_select {
        return OrderIntent::default();
    }
    if !(ctx.passable || ctx.helicopters) {
        // The cursor goes to DONT; no order and no sound.
        return OrderIntent::default();
    }
    let point = ctx.point;
    let wp = ctx.waypoint_key_down;
    if ctx.controls.guard_tower {
        return unavailable("doGuardTower (guard towers) not ported", false);
    }
    if ctx.controls.guard {
        return order(
            OrderKind::GuardPoint {
                point,
                walk: ctx.walk,
            },
            wp,
        );
    }
    // doMove (3137-3146) starts with the sensor strike: airstrike mode with the sensor probe
    // button held calls addAirstrike at the click and returns, before any jump or move. The
    // painting-VTOL click reaches here too, since `placement` skips the airstrike branch then.
    if ctx.controls.adding_airstrike && ctx.controls.sensor_probe_held {
        return unavailable(
            "addAirstrike sensor strike (doMove, missiongui.cpp:3137-3146) not ported",
            false,
        );
    }
    // doMove (3130-3170): a jump toggle sends doJump, minelaying changes the move mode.
    if ctx.controls.jump {
        let kind = if ctx.can_jump_to_point {
            OrderKind::Jump { point }
        } else {
            OrderKind::Denied
        };
        return order(kind, wp);
    }
    if ctx.controls.mines {
        return unavailable(
            "minelaying move mode (MOVE_MODE_MINELAYING) not ported",
            false,
        );
    }
    order(
        OrderKind::MoveTo {
            point,
            walk: ctx.walk,
        },
        wp,
    )
}

/// `doAttack` (missiongui.cpp:1801-1900). The right-click chain tests the team and disabled state
/// first (1569-1586); a hot key does not, and `doAttack` itself refuses an allied object.
fn attack(ctx: &ClickContext, t: &ClickTarget) -> OrderIntent {
    // 1804: no attack in waypoint mode.
    if ctx.waypoint_key_down {
        return order(OrderKind::Denied, false);
    }
    if t.facts.captureable {
        // 1838-1851: every selected mover must be a ground BattleMech free of capture blockers.
        let blocked = ctx
            .selected
            .iter()
            .any(|s| !s.mech_on_ground || s.capture_blocked);
        if blocked {
            return order(OrderKind::Denied, false);
        }
        return order(
            OrderKind::Capture {
                target: t.id,
                walk: ctx.walk,
            },
            false,
        );
    }
    if t.facts.relation == Relation::Friendly {
        // 1831-1835: an allied object is not attacked; nothing is issued and nothing sounds.
        return OrderIntent::default();
    }
    order(
        OrderKind::Attack {
            target: t.id,
            conserve_ammo: ctx.energy_weapons,
            fire_from_current_pos: ctx.fire_from_current_pos,
            walk: ctx.walk,
        },
        false,
    )
}
