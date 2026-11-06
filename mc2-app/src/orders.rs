// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Player-order bridge, ported from `MissionInterfaceManager` and `ControlGui`.
//!
//! The bridge turns merged HUD intents plus the app picker's world facts into the exact
//! [`PlayerCommand`] list the C++ GUI would issue (`handleOrders`, `doStop`, `doAttack`, ...).
//! Whether the sim can *execute* an emitted command is the sim's concern: it reports
//! `Unavailable` itself (`FrameUnavailable::Ai`) when it applies a tactical order its decision tree
//! cannot run yet. The bridge reports [`Unavailable`] only for what the GUI side cannot do yet
//! (mine queues, group jumps, repair, support strikes, ...).
//!
//! Per-mover `attackRange` stamping (`handleOrders`, `doStop`) and move-goal spreading
//! (`calcMoveGoals`) are done by the sim's `issue_player_command`, so those orders carry the
//! `TacticalOrder::init` defaults for the fields the sim overwrites.

use std::collections::BTreeMap;

use mc2_sim::combat::artillery_math::ARTILLERY_SENSOR;
use mc2_sim::command::{
    AttackMethod, OrderCode, OrderOrigin, PlayerCommand, TacticalOrder, TravelMode,
    FIRERANGE_CURRENT, FIRERANGE_LONG, FIRERANGE_MEDIUM, FIRERANGE_OPTIMAL, FIRERANGE_SHORT,
};
use mc2_sim::ids::{PartId, WatchId};
use mc2_sim::math::Vec3;
use mc2_ui::control::{ControlIntent, FireRange, UnitCapabilities, VehicleKind};
use mc2_ui::mission_hud::HudIntent;
use mc2_ui::mission_keys::MissionCommand;
use mc2_ui::unitbar::UnitBarIntent;

/// `AttackType` (tacordr.h:88-93).
const ATTACK_NONE: i32 = 0;
const ATTACK_TO_DESTROY: i32 = 1;
const ATTACK_CONSERVING_AMMO: i32 = 3;
/// `MECH_BODY_LOCATION_*` (mech.h:148-156).
const BODY_HEAD: i32 = 0;
const BODY_LEFT_ARM: i32 = 4;
const BODY_RIGHT_ARM: i32 = 5;
const BODY_LEFT_LEG: i32 = 6;
const BODY_RIGHT_LEG: i32 = 7;

/// `ControlGui::RUN/GUARD/JUMP` (controlgui.cpp:125-128); `WALK` is 0.
const RUN: u8 = 0x01;
const GUARD: u8 = 0x02;
const JUMP: u8 = 0x04;

/// Persistent app-side selection and order modifiers (`MissionInterfaceManager`/`ControlGui`).
#[derive(Clone, Debug, PartialEq)]
pub struct OrdersState {
    /// Selected player movers, in team roster order as `handleOrders` traverses them.
    pub selected: Vec<PartId>,
    /// `ControlGui::curOrder & RUN`.
    pub run: bool,
    /// `ControlGui::curOrder & GUARD`.
    pub guard: bool,
    /// `ControlGui::curOrder & JUMP`.
    pub jump: bool,
    /// `ControlGui::getMines()`; applied as `MOVE_MODE_MINELAYING` on a point move.
    pub lay_mines: bool,
    /// `ControlGui::fireFromCurrentPos`.
    pub fire_from_current_position: bool,
    /// Current control-panel fire-range choice.
    pub attack_range: i32,
    /// `MissionInterfaceManager::bEnergyWeapons` (`energyWeapons` key).
    pub conserve_ammo: bool,
    /// `ControlGui::isSelectingInfoObject()` (controlgui.cpp:2175-2183).
    pub info_selecting: bool,
}

impl Default for OrdersState {
    fn default() -> Self {
        Self {
            selected: Vec::new(),
            run: false,
            guard: false,
            jump: false,
            lay_mines: false,
            fire_from_current_position: false,
            attack_range: FIRERANGE_OPTIMAL,
            conserve_ammo: false,
            info_selecting: false,
        }
    }
}

impl OrdersState {
    /// `ControlGui::curOrder`.
    fn order_bits(&self) -> u8 {
        let mut bits = 0;
        if self.run {
            bits |= RUN;
        }
        if self.guard {
            bits |= GUARD;
        }
        if self.jump {
            bits |= JUMP;
        }
        bits
    }

    fn set_order_bits(&mut self, bits: u8) {
        self.run = bits & RUN != 0;
        self.guard = bits & GUARD != 0;
        self.jump = bits & JUMP != 0;
    }

    /// `ControlGui::getWalk` / `isDefaultSpeed`: `curOrder == WALK`.
    fn is_walk(&self) -> bool {
        self.order_bits() == 0
    }

    /// `ControlGui::getGuard`: `curOrder == GUARD` exactly (Run + Guard is not "guard" for clicks).
    fn is_guard(&self) -> bool {
        self.order_bits() == GUARD
    }

    /// `ControlGui::getJump`: `curOrder == JUMP` exactly.
    fn is_jump(&self) -> bool {
        self.order_bits() == JUMP
    }

    /// `ControlGui::toggleDefaultSpeed` (controlgui.cpp:1363).
    fn toggle_default_speed(&mut self) {
        self.set_order_bits((self.order_bits() ^ RUN) & (RUN | GUARD));
    }

    /// `ControlGui::toggleGuard` (controlgui.cpp:1391).
    fn toggle_guard(&mut self) {
        self.set_order_bits((self.order_bits() ^ GUARD) & (GUARD | RUN));
    }

    /// `ControlGui::toggleJump` (controlgui.cpp:1351).
    fn toggle_jump(&mut self) {
        self.set_order_bits((self.order_bits() ^ JUMP) & JUMP);
    }

    /// `ControlGui::setDefaultSpeed`: `curOrder = 0`.
    fn set_default_speed(&mut self) {
        self.set_order_bits(0);
    }

    /// `controlGui.getWalk() ? TRAVEL_MODE_SLOW : TRAVEL_MODE_FAST`.
    fn travel_mode(&self) -> TravelMode {
        if self.is_walk() {
            TravelMode::Slow
        } else {
            TravelMode::Fast
        }
    }
}

/// Body facts of the picked target for a called shot (`aimLeg/aimArm/aimHead`), resolved by the
/// app picker from the target `BattleMech` and the selected movers' weapons.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AimFacts {
    /// The target is a `BattleMech` with `MOVETYPE_GROUND`.
    pub ground_mech: bool,
    /// `anySelectedWithoutAreaEffect()`: a selected home mover has a non-area-effect weapon.
    pub any_selected_non_area_weapon: bool,
    pub left_leg_destroyed: bool,
    pub right_leg_destroyed: bool,
    pub left_arm_destroyed: bool,
    pub right_arm_destroyed: bool,
    pub head_destroyed: bool,
}

/// World facts resolved by the app's picking and mission snapshot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BridgeContext {
    /// `Mission::scenarioTime` at the HUD dispatch point.
    pub scenario_time: f32,
    /// Current picked object's stable watch id.
    pub target: Option<WatchId>,
    /// Target class needed to distinguish `doAttack` from capture and friendly no-op paths.
    pub target_kind: Option<AttackTargetKind>,
    /// Whether the picked object is disabled; C++ suppresses default attacks on disabled targets.
    pub target_disabled: Option<bool>,
    /// Which home-roster movers are ground BattleMechs and may receive a capture order.
    pub capture_movers: BTreeMap<PartId, bool>,
    /// Whether each selected mover has a C++ `getCaptureBlocker` result for the picked target.
    pub capture_blockers: BTreeMap<PartId, bool>,
    /// Part id of a picked mover, required by `doEject`'s single-unit routing.
    pub target_part: Option<PartId>,
    /// Global `invulnerableON`; ejection also requires the target in `roster_order`.
    pub invulnerable: bool,
    /// Whether the C++ `energyWeapons` hotkey is currently held (`commands[ENERGY_WEAPON_INDEX]`).
    pub energy_weapons_held: bool,
    /// Current picked world point, including terrain height.
    pub world_point: Option<Vec3>,
    /// Mission key handlers such as `forceShot` run only on the completed click.
    pub command_clicked: bool,
    /// Waypoint modifier at dispatch time (`getKeyDown(WAYPOINT_KEY)`).
    pub waypoint_queue: bool,
    /// Support asset kind selected while painting an order (`ControlGui::isAddingAirstrike`),
    /// as the `ARTILLERY_*` id.
    pub strike: Option<i32>,
    /// The player team roster, used to match `handleOrders`' traversal order.
    pub roster_order: Vec<PartId>,
    /// Player-owned movers whose commander is `Commander::home`, for eject eligibility.
    pub home_commander_order: Vec<PartId>,
    /// Mapping from app-assigned UnitBar ids to sim mission part ids.
    pub unit_bar_parts: BTreeMap<u32, PartId>,
    /// Live, selectable members of the ten Ctrl+digit groups. Filled from the simulation snapshot
    /// (`MissionInterfaceManager::selectForceGroup`, `code/missiongui.cpp:3249-3282`).
    pub force_group_members: [Vec<PartId>; 10],
    /// Each selected mover's current `attackRange`; required for `doAttack`'s per-mover setup.
    pub mover_attack_ranges: BTreeMap<PartId, i32>,
    /// Declared upstream input: `Mover::canJump()` for each mover that may be selected
    /// (`MissionInterfaceManager::canJump`, missiongui.cpp:3475).
    pub jump_capable: BTreeMap<PartId, bool>,
    /// Declared upstream input: the passable-cell and `getJumpRange` part of `canJumpToWPos`
    /// (missiongui.cpp:3500) for the picked landing point.
    pub jump_landing_ok: Option<bool>,
    /// Called-shot body facts of the picked target.
    pub aim: Option<AimFacts>,
    /// Resolved primary-viewport click supplied by the app picker beside HUD intents.
    pub viewport_click: Option<ViewportOrderClick>,
    /// First current NAV objective marker (`gotoNextNavMarker`, missiongui.cpp:4712-4746).
    pub next_nav_marker: Option<Vec3>,
    /// Home movers whose projected bounds intersect the viewport (`selectVisible`, missiongui.cpp:2084-2106).
    pub visible_movers: Vec<PartId>,
    /// Each mover's control panel capabilities (`isMineLayer`, `isRecover`, `isRefit`, `maxMoveSpeed == 0`,
    /// `suppressionFire`: controlgui.cpp:1033-1039), from the simulation.
    pub unit_caps: BTreeMap<PartId, UnitCapabilities>,
    /// `Mover::needsRefit()` of each home mover (`canRepairBay`, missiongui.cpp:4540).
    pub needs_refit: BTreeMap<PartId, bool>,
    /// `controlGui.forceGroupBar.getIconCount() >= MAX_ICONS` (`canSalvage`, missiongui.cpp:4461).
    pub force_bar_full: bool,
    /// What `canRepair`, `canSalvage` and the repair bay test read off the picked object.
    pub support_target: Option<SupportTarget>,
    /// The support vehicle the control panel has armed and not yet placed (`isAddingVehicle` / `isAddingSalvage` while no VTOL
    /// flies): the next completed click calls it in.
    pub adding_vehicle: Option<VehicleKind>,
}

/// Facts about the picked object read by `canRepair`, `canSalvage` and the repair bay branch of the click chain
/// (missiongui.cpp:4461-4570, 1425, 1577).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SupportTarget {
    /// `isMover()`.
    pub mover: bool,
    /// `getObjectClass() == BATTLEMECH`.
    pub battle_mech: bool,
    /// `Team::home->isEnemy(getTeam())`.
    pub enemy: bool,
    /// `isDisabled()`.
    pub disabled: bool,
    /// `isDestroyed()`.
    pub destroyed: bool,
    /// `Mover::needsRefit()`.
    pub needs_refit: bool,
    /// The part id of the object `refitBuddyWID` names (a mover being repaired by that mover), if any.
    pub refit_buddy: Option<PartId>,
    /// `Team::home->teamLineOfSight(position, 0)`: a home sensor sees the object.
    pub visible: bool,
    /// A friendly repair bay for mechs that has refit points left: `isFriendly(team)`, `OBJECT_FLAG_CANREFIT`,
    /// `OBJECT_FLAG_MECHBAY` and `getRefitPoints() > 0`.
    pub mech_bay: bool,
}

/// Primary viewport action produced by the app's world picker.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewportOrderClick {
    Terrain(Vec3),
    Object(WatchId),
}

/// C++ `doAttack` chooses distinct order paths based on the picked object's team and captureability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackTargetKind {
    Enemy,
    Friendly,
    Captureable,
}

/// An action the GUI side of the port cannot encode yet. (Orders the sim accepts but cannot yet
/// execute are the sim's own `Unavailable`, not listed here.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable {
    pub action: &'static str,
    pub reason: &'static str,
}

/// Commands plus the app selection after applying the frame's selection intents.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BridgeOutput {
    pub commands: Vec<PlayerCommand>,
    pub unavailable: Vec<Unavailable>,
    pub selected: Vec<PartId>,
    /// The `curOrder` bits (`RUN | GUARD | JUMP`) the control panel must show after this call; the
    /// caller applies them with `ControlPanel::set_order_bits` so the panel and bridge never diverge
    /// (`setDefaultSpeed` after a move, hotkey toggles).
    pub panel_order_bits: u8,
    /// A strike button press was reported `Unavailable`: the caller must un-press it and refund
    /// its cost with `ControlPanel::release_vehicle_button` (same frame, nothing was called).
    pub release_strike_button: bool,
    /// `playDigitalSample(INVALID_GUI)` when a capture click is refused by `doAttack`.
    pub play_invalid_gui: bool,
    /// A strike was called this frame: the caller un-presses the strike button without a refund
    /// (`addAirstrike` -> `ControlGui::unPressAllVehicleButtons`, missiongui.cpp:4144), with
    /// `ControlPanel::unpress_vehicle_buttons`.
    pub strike_called: bool,
    /// UnitBar or info-mode target to display in the production info window.
    pub show_info: Option<PartId>,
}

/// The production order route: the HUD runs its order intents and world clicks through
/// [`intents_to_commands`] and this hook hands the resulting commands to the frame's simulation
/// step.
#[derive(Debug, Default)]
pub struct BridgeOrders;

impl crate::layers::OrderHook<PlayerCommand> for BridgeOrders {
    fn emits_sim_commands(&self) -> bool {
        true
    }

    fn player_commands(&mut self, commands: &[PlayerCommand], out: &mut Vec<PlayerCommand>) {
        out.extend_from_slice(commands);
    }
}

/// Translate the merged HUD output to sim commands and update app selection state.
///
/// `MissionInterfaceManager::handleOrders` walks the player roster; callers keep `selected` in
/// that order. The sim applies each order and performs its own per-mover attack-range stamping
/// and move-goal assignment (`mc2-sim/src/ai/mod.rs`, `issue_player_command`).
pub fn intents_to_commands(
    intents: &[HudIntent],
    state: &mut OrdersState,
    context: &BridgeContext,
) -> BridgeOutput {
    let mut out = BridgeOutput::default();
    let mut context = context.clone();
    roster_sort(&mut state.selected, &context);
    let mut consumed = false;
    for intent in intents {
        match intent {
            HudIntent::UnitBar(intent) => apply_selection(intent, state, &context, &mut out),
            HudIntent::Control(intent) => apply_control(*intent, state, &mut context, &mut out),
            HudIntent::Mission(command) => {
                consumed |= apply_mission(*command, state, &mut context, &mut out)
            }
            HudIntent::TacMapMoveOrder((x, y)) => {
                let Some(point) = context.world_point else {
                    unavailable(
                        &mut out,
                        "move",
                        "TacMap intent has no resolved terrain elevation",
                    );
                    continue;
                };
                if point.x != *x || point.y != *y {
                    unavailable(
                        &mut out,
                        "move",
                        "resolved world point does not match the TacMap click",
                    );
                    continue;
                }
                // `GameTacMap::update` calls `doMove(world)` (gametacmap.cpp:109).
                do_move(point, state, &context, &mut out);
            }
            HudIntent::TacMapCameraTarget(_) | HudIntent::Panel(_) | HudIntent::Pause(_) => {}
        }
    }
    // missiongui.cpp's update() returns as soon as a completed single-click hotkey handles the
    // click, including commands that intentionally issue no tactical order.
    if context.command_clicked && consumed {
        out.selected.clone_from(&state.selected);
        out.panel_order_bits = state.order_bits();
        return out;
    }
    if context.command_clicked {
        if let Some(click) = context.viewport_click {
            viewport_click(click, state, &context, &mut out);
        }
    }
    roster_sort(&mut state.selected, &context);
    out.selected.clone_from(&state.selected);
    out.panel_order_bits = state.order_bits();
    out
}

/// Apply the control-panel range to each selected mover, as `ControlGui::setRange` does.
fn set_attack_range(
    range: i32,
    update_current_order: bool,
    state: &mut OrdersState,
    context: &mut BridgeContext,
    out: &mut BridgeOutput,
) {
    state.attack_range = range;
    let movers = commanded(state, context);
    for mover in &movers {
        context.mover_attack_ranges.insert(*mover, range);
    }
    if !movers.is_empty() {
        out.commands.push(PlayerCommand::SetAttackRange {
            movers,
            range,
            update_current_order,
            fire_from_current_position: state.fire_from_current_position,
        });
    }
}

/// `updateOldStyle`/`updateAOEStyle` order chain for a completed primary click (missiongui.cpp
/// 1340-1450, 1533-1600).
fn viewport_click(
    click: ViewportOrderClick,
    state: &mut OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    if context.strike.is_some() {
        // `addAirstrike` at `wPos` (missiongui.cpp:1340): the click is consumed whatever it hit.
        let point = match click {
            ViewportOrderClick::Terrain(point) => Some(point),
            ViewportOrderClick::Object(_) => None,
        };
        if let Some(point) = context.world_point.or(point) {
            add_airstrike(point, context, out);
        } else {
            unavailable(
                out,
                "support strike",
                "the clicked world point is missing from world context",
            );
        }
        return;
    }
    if let Some(kind) = context.adding_vehicle {
        place_vehicle(kind, click, context, out);
        return;
    }
    match click {
        _ if state.info_selecting => match click {
            ViewportOrderClick::Object(_) if context.target_part.is_some() => {
                out.show_info = context.target_part;
                state.info_selecting = false;
            }
            ViewportOrderClick::Terrain(_) => state.info_selecting = false,
            _ => {}
        },
        ViewportOrderClick::Terrain(_)
            if support_mode(state, context) == SupportMode::GuardTower =>
        {
            do_guard_tower(state, context, out)
        }
        ViewportOrderClick::Terrain(point) if state.is_guard() => {
            do_guard(None, point, state, context, out)
        }
        ViewportOrderClick::Terrain(point) => do_move(point, state, context, out),
        ViewportOrderClick::Object(target)
            if support_mode(state, context) == SupportMode::Salvage =>
        {
            do_salvage(target, state, context, out)
        }
        ViewportOrderClick::Object(target) if state.is_guard() => {
            do_guard(Some(target), Vec3::default(), state, context, out)
        }
        ViewportOrderClick::Object(target)
            if support_mode(state, context) == SupportMode::Repair
                && can_repair(state, context) =>
        {
            do_repair(target, state, context, out)
        }
        ViewportOrderClick::Object(_)
            if support_mode(state, context) == SupportMode::GuardTower =>
        {
            do_guard_tower(state, context, out)
        }
        ViewportOrderClick::Object(target) if can_repair_bay(state, context) => {
            do_repair_bay(target, state, context, out)
        }
        ViewportOrderClick::Object(target) => {
            if attack_orders(target, state, context, out) {
                state.fire_from_current_position = false;
            }
        }
    }
}

/// The object numbers of `vehicleIDs[]` that name a vehicle (`support_vehicle_id`).
pub const SUPPORT_VEHICLE_IDS: [i32; 5] = [182, 393, 120, 147, 415];

/// `vehicleIDs[]` (controlgui.cpp:96): the object number of the vehicle a support button calls in.
pub fn support_vehicle_id(kind: VehicleKind) -> Option<i32> {
    match kind {
        VehicleKind::RepairTruck => Some(182),
        VehicleKind::Scout => Some(393),
        VehicleKind::Minelayer => Some(120),
        VehicleKind::RecoveryTeam => Some(147),
        VehicleKind::GuardTower => Some(415),
        VehicleKind::Airstrike | VehicleKind::SensorProbe => None,
    }
}

/// The completed click of `updateOldStyle`/`updateAOEStyle` while a support vehicle is armed: `beginVtol` at the clicked point
/// (missiongui.cpp:1324-1331). `canAddVehicle` / `canRecover` are the sim's: it refuses an invalid placement with an event
/// (`INVALID_GUI`) and the button stays armed. The recovery craft takes the clicked mech.
fn place_vehicle(
    kind: VehicleKind,
    click: ViewportOrderClick,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    let Some(vehicle_id) = support_vehicle_id(kind) else {
        return;
    };
    let Some(at) = context.world_point else {
        unavailable(
            out,
            "support vehicle",
            "the clicked world point is missing from world context",
        );
        return;
    };
    let target = match click {
        ViewportOrderClick::Object(t) => Some(t),
        ViewportOrderClick::Terrain(_) => None,
    };
    out.commands.push(PlayerCommand::BeginVtol {
        commander: 0,
        vehicle_id,
        at,
        target,
    });
}

fn player_order(code: OrderCode, unit_order: bool, time: f32) -> TacticalOrder {
    TacticalOrder::new(OrderOrigin::Player, code, unit_order, time)
}

/// `MissionInterfaceManager::addAirstrike` (missiongui.cpp:4133): `IfaceCallStrike` with the pressed button's strike type at the
/// target (`makeAirStrikeTarget` returns the point unchanged), then every vehicle button is un-pressed. The cost was spent when
/// the button was pressed and is not refunded; the sim applies the line-of-sight downgrade (artlry.cpp:213-218).
fn add_airstrike(point: Vec3, context: &BridgeContext, out: &mut BridgeOutput) {
    let Some(strike) = context.strike else {
        return;
    };
    out.commands
        .push(PlayerCommand::CallStrike { strike, at: point });
    out.strike_called = true;
}

/// The selected movers `handleOrders` actually commands: it skips any whose commander is not
/// `Commander::home` (missiongui.cpp:2493-2496), e.g. a selected allied mover.
fn commanded(state: &OrdersState, context: &BridgeContext) -> Vec<PartId> {
    state
        .selected
        .iter()
        .copied()
        .filter(|part| context.home_commander_order.contains(part))
        .collect()
}

/// `MissionInterfaceManager::handleOrders` (missiongui.cpp:2462) for the selected movers.
///
/// `calcMoveGoals` and the per-mover `attackRange` stamp happen in the sim. The waypoint key
/// queues without executing; the mine toggle queues *and* releases the queue (`OrderQueueExecute`).
fn handle_orders(
    order: TacticalOrder,
    state: &OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    let movers = commanded(state, context);
    if movers.is_empty() {
        return;
    }
    if !context.waypoint_queue && state.lay_mines {
        // `addQueuedTacOrder` then `setExecutingQueue(TRUE)` (missiongui.cpp:2518)
        out.commands
            .push(PlayerCommand::OrderQueueExecute { movers, order });
        return;
    }
    out.commands.push(PlayerCommand::Order {
        movers,
        order,
        queue: context.waypoint_queue,
    });
}

/// `MissionInterfaceManager::doMove` (missiongui.cpp:3130).
fn do_move(point: Vec3, state: &mut OrdersState, context: &BridgeContext, out: &mut BridgeOutput) {
    // `doMove` with the sensor probe armed is `addAirstrike` at the point (missiongui.cpp:3137); an armed large airstrike moves.
    if context.strike == Some(ARTILLERY_SENSOR) {
        add_airstrike(point, context, out);
        return;
    }
    if state.is_jump() {
        do_jump(point, state, context, out);
        return;
    }
    let mut order = player_order(OrderCode::MoveToPoint, false, context.scenario_time);
    order.move_params.way_path.points[0] = point;
    order.move_params.wait = false;
    order.move_params.way_path.mode[0] = state.travel_mode();
    order.move_params.mine_laying = state.lay_mines;
    handle_orders(order, state, context, out);
    state.set_default_speed();
}

/// `MissionInterfaceManager::doJump` (missiongui.cpp:1969) for a terrain click.
fn do_jump(point: Vec3, state: &mut OrdersState, context: &BridgeContext, out: &mut BridgeOutput) {
    // `canJumpToWPos`: passable landing, jump range, and every selected mover can jump.
    match context.jump_landing_ok {
        None => {
            unavailable(
                out,
                "jump",
                "landing passability and jump range are missing from world context",
            );
            return;
        }
        Some(false) => return,
        Some(true) => {}
    }
    match selection_can_jump(state, context) {
        None => {
            unavailable(
                out,
                "jump",
                "selected mover jump capability is missing from world context",
            );
            return;
        }
        // The GUI refuses (cursor + INVALID_GUI sound); the jump mode stays armed.
        Some(false) => return,
        Some(true) => {}
    }
    let mut order = player_order(OrderCode::JumpToPoint, false, context.scenario_time);
    order.move_params.wait = false;
    order.move_params.way_path.mode[0] = TravelMode::Jump;
    order.move_params.way_path.points[0] = point;
    handle_orders(order, state, context, out);
    state.set_default_speed();
}

/// `canJump()` (missiongui.cpp:3475); `None` when a selected mover's capability is not supplied.
fn selection_can_jump(state: &OrdersState, context: &BridgeContext) -> Option<bool> {
    let mut all = true;
    for mover in &commanded(state, context) {
        all &= *context.jump_capable.get(mover)?;
    }
    Some(all)
}

/// `MissionInterfaceManager::doGuard` (missiongui.cpp:2030).
fn do_guard(
    target: Option<WatchId>,
    point: Vec3,
    state: &mut OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    let mut order = player_order(OrderCode::Guard, false, context.scenario_time);
    match target {
        Some(target) => order.target = target,
        None => {
            order.move_params.way_path.points[0] = point;
            order.move_params.way_path.mode[0] = state.travel_mode();
        }
    }
    handle_orders(order, state, context, out);
    state.set_default_speed();
}

/// Which specialty button the control panel shows for the selection (`ControlGui::update`, controlgui.cpp:1076-1170): the buttons
/// are exclusive, in this order. Repair and salvage are pressed whenever they are shown; the guard tower button is pressed while
/// every selected tower has `suppressionFire`; the mine button is the player's toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupportMode {
    None,
    Mines,
    Salvage,
    Repair,
    GuardTower,
}

fn support_mode(state: &OrdersState, context: &BridgeContext) -> SupportMode {
    let movers = commanded(state, context);
    let caps: Option<Vec<&UnitCapabilities>> = movers
        .iter()
        .map(|part| context.unit_caps.get(part))
        .collect();
    let Some(caps) = caps.filter(|c| !c.is_empty()) else {
        return SupportMode::None;
    };
    if caps.iter().all(|c| c.can_lay_mines) {
        SupportMode::Mines
    } else if caps.iter().all(|c| c.can_salvage) {
        SupportMode::Salvage
    } else if caps.iter().all(|c| c.can_repair) {
        SupportMode::Repair
    } else if caps.iter().all(|c| c.is_guard_tower)
        && caps.iter().all(|c| c.guard_tower_suppression)
    {
        SupportMode::GuardTower
    } else {
        SupportMode::None
    }
}

/// `MissionInterfaceManager::canSalvage` (missiongui.cpp:4461).
fn can_salvage(context: &BridgeContext) -> bool {
    context
        .support_target
        .is_some_and(|t| !context.force_bar_full && t.disabled && t.visible && t.battle_mech)
}

/// `MissionInterfaceManager::canRepair` (missiongui.cpp:4488): a living mover that is not an enemy and needs a refit or is
/// already being refitted by the one selected mover.
fn can_repair(state: &OrdersState, context: &BridgeContext) -> bool {
    let Some(t) = context.support_target else {
        return false;
    };
    if !t.mover || t.enemy || t.disabled || t.destroyed {
        return false;
    }
    // the one selected home mover (`watchID` is -1 for several, 0 for none)
    let selected = commanded(state, context);
    let single = match selected.as_slice() {
        [only] => Some(*only),
        _ => None,
    };
    t.needs_refit || (single.is_some() && t.refit_buddy == single)
}

/// `MissionInterfaceManager::canRepairBay` (missiongui.cpp:4540) with the friendly mech bay test of the click chain.
fn can_repair_bay(state: &OrdersState, context: &BridgeContext) -> bool {
    context.support_target.is_some_and(|t| t.mech_bay)
        && commanded(state, context)
            .iter()
            .any(|part| context.needs_refit.get(part).copied().unwrap_or(false))
}

/// `MissionInterfaceManager::doSalvage` (missiongui.cpp:4470).
fn do_salvage(
    target: WatchId,
    state: &OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    if !can_salvage(context) {
        return;
    }
    let mut order = player_order(OrderCode::Recover, false, context.scenario_time);
    order.target = target;
    order.attack_params.attack_type = ATTACK_NONE;
    order.attack_params.method = AttackMethod::Ramming;
    order.attack_params.pursue = true;
    order.move_params.way_path.mode[0] = TravelMode::Fast;
    handle_orders(order, state, context, out);
}

/// `doRepair` and `doRepairBay` (missiongui.cpp:4523, 4557): the same order with a different code.
fn support_move_order(
    code: OrderCode,
    target: WatchId,
    state: &mut OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    let mut order = player_order(code, false, context.scenario_time);
    order.target = target;
    order.selection_index = -1;
    order.move_params.way_path.mode[0] = state.travel_mode();
    order.move_params.face_object = true;
    order.move_params.from_area = -1;
    order.move_params.wait = false;
    handle_orders(order, state, context, out);
    state.set_default_speed();
}

fn do_repair(
    target: WatchId,
    state: &mut OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    support_move_order(OrderCode::Refit, target, state, context, out);
}

fn do_repair_bay(
    target: WatchId,
    state: &mut OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    support_move_order(OrderCode::GetFixed, target, state, context, out);
}

/// `MissionInterfaceManager::doGuardTower` (missiongui.cpp:5493): every selected mover gets `suppressionFire`, then fires at the
/// clicked point.
fn do_guard_tower(state: &mut OrdersState, context: &BridgeContext, out: &mut BridgeOutput) {
    let Some(point) = context.world_point else {
        unavailable(
            out,
            "guard tower",
            "the clicked world point is missing from world context",
        );
        return;
    };
    let movers = commanded(state, context);
    if !movers.is_empty() {
        out.commands
            .push(PlayerCommand::SetSuppressionFire { movers, on: true });
    }
    let mut order = player_order(OrderCode::AttackPoint, false, context.scenario_time);
    order.attack_params.target_point = point;
    order.attack_params.attack_type = energy_attack_type(state);
    order.attack_params.method = AttackMethod::Ranged;
    order.attack_params.range = FIRERANGE_CURRENT;
    order.attack_params.pursue = false;
    order.move_params.face_object = true;
    order.move_params.from_area = -1;
    order.move_params.wait = false;
    handle_orders(order, state, context, out);
    state.set_default_speed();
}

/// `ControlGui::doStop` (controlgui.cpp:1669): issued directly, never queued.
fn do_stop(state: &mut OrdersState, context: &BridgeContext, out: &mut BridgeOutput) {
    let movers = commanded(state, context);
    if !movers.is_empty() {
        out.commands.push(PlayerCommand::Order {
            movers,
            order: player_order(OrderCode::Stop, false, context.scenario_time),
            queue: false,
        });
    }
    state.lay_mines = false;
    state.fire_from_current_position = false;
    state.set_default_speed();
}

/// A ranged `ATTACK_OBJECT`/`ATTACK_POINT` order as `forceShot` and the aimed shots build it.
fn shot_order(
    code: OrderCode,
    attack_type: i32,
    state: &OrdersState,
    context: &BridgeContext,
) -> TacticalOrder {
    let mut order = player_order(code, false, context.scenario_time);
    order.attack_params.attack_type = attack_type;
    order.attack_params.method = AttackMethod::Ranged;
    order.attack_params.range = FIRERANGE_OPTIMAL;
    order.attack_params.pursue = !state.fire_from_current_position;
    order.move_params.way_path.mode[0] = state.travel_mode();
    order
}

fn energy_attack_type(state: &OrdersState) -> i32 {
    if state.conserve_ammo {
        ATTACK_CONSERVING_AMMO
    } else {
        ATTACK_TO_DESTROY
    }
}

/// `doAttack` (missiongui.cpp:1801) for an object click. Returns whether the function ran to its
/// end (it then clears the fire-from-current-position flag); early returns leave it set.
fn attack_orders(
    target: WatchId,
    state: &OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) -> bool {
    // `!target->isDisabled()` guards the call (missiongui.cpp:1454), `getKeyDown(WAYPOINT_KEY)`
    // refuses inside it; both are silent GUI refusals.
    if context.target_disabled == Some(true) || context.waypoint_queue {
        return false;
    }
    if context.target_disabled.is_none() {
        unavailable(
            out,
            "attack",
            "picked target disabled state is missing from world context",
        );
        return false;
    }
    match context.target_kind {
        Some(AttackTargetKind::Enemy) => {}
        // C++ `doAttack` issues nothing for a friendly target (no command, no cursor feedback).
        Some(AttackTargetKind::Friendly) => return false,
        Some(AttackTargetKind::Captureable) => {
            let movers = commanded(state, context);
            if movers.is_empty() {
                return false;
            }
            if movers.iter().any(|m| {
                context.capture_movers.get(m) != Some(&true)
                    || context.capture_blockers.get(m) != Some(&false)
            }) {
                out.play_invalid_gui = true;
                return false;
            }
            let mut order = player_order(OrderCode::Capture, false, context.scenario_time);
            order.target = target;
            order.attack_params.attack_type = 0;
            order.attack_params.method = AttackMethod::Ramming;
            order.attack_params.pursue = true;
            order.move_params.way_path.mode[0] = state.travel_mode();
            out.commands.push(PlayerCommand::Order {
                movers,
                order,
                queue: false,
            });
            return true;
        }
        None => {
            unavailable(
                out,
                "attack",
                "picked target team and captureability are missing",
            );
            return false;
        }
    }
    let movers = commanded(state, context);
    let Some(ranges) = movers
        .iter()
        .map(|mover| context.mover_attack_ranges.get(mover).copied())
        .collect::<Option<Vec<_>>>()
    else {
        unavailable(
            out,
            "attack",
            "selected mover attackRange is missing from world context",
        );
        return false;
    };
    // Each mover gets its own `attackRange` and pursuit (missiongui.cpp:1869-1880); adjacent
    // movers with the same pair share one command, in roster order.
    let mut groups: Vec<(i32, bool, Vec<PartId>)> = Vec::new();
    for (mover, range) in movers.iter().zip(ranges) {
        let pursue = range != FIRERANGE_CURRENT && !state.fire_from_current_position;
        match groups.last_mut() {
            Some((r, p, movers)) if *r == range && *p == pursue => movers.push(*mover),
            _ => groups.push((range, pursue, vec![*mover])),
        }
    }
    for (range, pursue, movers) in groups {
        let mut order = player_order(OrderCode::AttackObject, false, context.scenario_time);
        order.target = target;
        order.attack_params.attack_type = energy_attack_type(state);
        order.attack_params.method = AttackMethod::Ranged;
        order.attack_params.range = range;
        order.attack_params.pursue = pursue;
        order.move_params.way_path.mode[0] = state.travel_mode();
        out.commands.push(PlayerCommand::Order {
            movers,
            order,
            queue: false,
        });
    }
    true
}

/// `forceShot` (missiongui.cpp:3216). Returns whether it consumed the click.
fn force_shot(state: &OrdersState, context: &BridgeContext, out: &mut BridgeOutput) -> bool {
    if !context.command_clicked {
        return false;
    }
    let order = match (context.target, context.world_point) {
        (Some(target), _) => {
            let mut order = shot_order(
                OrderCode::AttackObject,
                energy_attack_type(state),
                state,
                context,
            );
            order.target = target;
            order
        }
        (None, Some(point)) => {
            let mut order = shot_order(
                OrderCode::AttackPoint,
                energy_attack_type(state),
                state,
                context,
            );
            order.attack_params.target_point = point;
            order
        }
        (None, None) => {
            unavailable(
                out,
                "forced shot",
                "target and target point are both missing from world context",
            );
            return true;
        }
    };
    handle_orders(order, state, context, out);
    true
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AimPart {
    Leg,
    Arm,
    Head,
}

/// `aimLeg/aimArm/aimHead` (missiongui.cpp:2117-2270). Returns the C++ return value (1 = the
/// click is handled; 0 = fall through to the normal attack). See DIFFERENCES.md for Rust's
/// refusal to issue a shot at a location that is already destroyed.
fn aim_shot(
    part: AimPart,
    state: &OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) -> bool {
    // A missing target consumes the aimed command (missiongui.cpp:2117-2270), so a terrain click
    // does not fall through to `doMove`.
    let Some(target) = context.target else {
        return true;
    };
    let Some(facts) = context.aim else {
        unavailable(
            out,
            "aimed shot",
            "target body and weapon eligibility are not available to derive the C++ aim location",
        );
        return true;
    };
    if !facts.ground_mech {
        return part != AimPart::Leg;
    }
    if !facts.any_selected_non_area_weapon {
        return true;
    }
    if part == AimPart::Leg && facts.left_leg_destroyed && facts.right_leg_destroyed {
        return true;
    }
    // Rust deliberately avoids issuing a called shot at a location that is already destroyed;
    // the C++ checks these two cases only while setting the cursor, then lets a completed click
    // through (DIFFERENCES.md, called-shot destroyed-location fix).
    if (part == AimPart::Arm && facts.left_arm_destroyed && facts.right_arm_destroyed)
        || (part == AimPart::Head && facts.head_destroyed)
    {
        return true;
    }
    if !context.command_clicked {
        return true;
    }
    let location = match part {
        AimPart::Leg if facts.left_leg_destroyed => BODY_RIGHT_LEG,
        AimPart::Leg => BODY_LEFT_LEG,
        AimPart::Arm if facts.left_arm_destroyed => BODY_RIGHT_ARM,
        AimPart::Arm => BODY_LEFT_ARM,
        AimPart::Head => BODY_HEAD,
    };
    let mut order = shot_order(OrderCode::AttackObject, ATTACK_TO_DESTROY, state, context);
    order.target = target;
    order.attack_params.aim_location = location;
    handle_orders(order, state, context, out);
    true
}

fn unavailable(out: &mut BridgeOutput, action: &'static str, reason: &'static str) {
    out.unavailable.push(Unavailable { action, reason });
}

fn apply_selection(
    intent: &UnitBarIntent,
    state: &mut OrdersState,
    context: &BridgeContext,
    out: &mut BridgeOutput,
) {
    match intent {
        UnitBarIntent::Select(ids) => {
            if let Some(parts) = mapped_parts(ids, context, &mut out.unavailable) {
                state.selected = parts;
                roster_sort(&mut state.selected, context);
            }
        }
        UnitBarIntent::AddToSelection(id) => {
            if let Some(part) = mapped_part(*id, context, &mut out.unavailable) {
                if !state.selected.contains(&part) {
                    state.selected.push(part);
                    roster_sort(&mut state.selected, context);
                }
            }
        }
        UnitBarIntent::RemoveFromSelection(id) => {
            if let Some(part) = mapped_part(*id, context, &mut out.unavailable) {
                state.selected.retain(|selected| *selected != part);
            }
        }
        UnitBarIntent::SelectForceGroup(group) => {
            if let Some(members) = context.force_group_members.get(usize::from(*group)) {
                state.selected = members.clone();
                roster_sort(&mut state.selected, context);
            }
        }
        UnitBarIntent::CenterOn(_) => {}
        UnitBarIntent::ShowInfo(id) => {
            if let Some(part) = context.unit_bar_parts.get(id).copied() {
                out.show_info = Some(part);
            }
        }
        UnitBarIntent::LeaveAllGroups(id) => {
            if let Some(part) = mapped_part(*id, context, &mut out.unavailable) {
                out.commands
                    .push(PlayerCommand::ClearForceGroup { mover: part });
            }
        }
    }
}

fn roster_sort(selected: &mut [PartId], context: &BridgeContext) {
    if !context.roster_order.is_empty() {
        selected.sort_by_key(|part| {
            context
                .roster_order
                .iter()
                .position(|roster| roster == part)
                .unwrap_or(usize::MAX)
        });
    }
}

fn mapped_part(
    id: u32,
    context: &BridgeContext,
    unavailable: &mut Vec<Unavailable>,
) -> Option<PartId> {
    match context.unit_bar_parts.get(&id).copied() {
        Some(part) => Some(part),
        None => {
            unavailable.push(Unavailable {
                action: "select unit",
                reason: "app UnitBar id has no sim PartId mapping",
            });
            None
        }
    }
}

fn mapped_parts(
    ids: &[u32],
    context: &BridgeContext,
    unavailable: &mut Vec<Unavailable>,
) -> Option<Vec<PartId>> {
    let parts: Vec<_> = ids
        .iter()
        .map(|id| mapped_part(*id, context, unavailable))
        .collect();
    parts.into_iter().collect()
}

fn apply_control(
    intent: ControlIntent,
    state: &mut OrdersState,
    context: &mut BridgeContext,
    out: &mut BridgeOutput,
) {
    match intent {
        // The panel computed the new bits (`curOrder ^= RUN; curOrder &= RUN|GUARD`); Run and
        // Guard both clear the jump bit.
        ControlIntent::SetRun(value) => {
            state.run = value;
            state.jump = false;
        }
        ControlIntent::ToggleJump => state.toggle_jump(),
        ControlIntent::SetGuard(value) => {
            state.guard = value;
            state.jump = false;
        }
        // `CUR_RANGE` is the hold-position toggle (`toggleHoldPosition`, controlgui.cpp:2931):
        // it changes each selected mover and updates/reissues its active tactical order.
        ControlIntent::SetRange(FireRange::Current) => {
            set_attack_range(FIRERANGE_CURRENT, true, state, context, out)
        }
        ControlIntent::SetRange(range) => {
            let range = match range {
                FireRange::Optimal => FIRERANGE_OPTIMAL,
                FireRange::Short => FIRERANGE_SHORT,
                FireRange::Medium => FIRERANGE_MEDIUM,
                FireRange::Long => FIRERANGE_LONG,
                FireRange::Current => FIRERANGE_CURRENT,
            };
            set_attack_range(range, false, state, context, out);
        }
        ControlIntent::SetFireFromCurrentPosition(value) => {
            state.fire_from_current_position = value
        }
        ControlIntent::Stop => do_stop(state, context, out),
        ControlIntent::ToggleMines => state.lay_mines = !state.lay_mines,
        // Repair and salvage are pressed again by every `ControlGui::update` while their button is shown (controlgui.cpp:1130,
        // 1141), so the toggle changes nothing the order chain reads.
        ControlIntent::ToggleRepair | ControlIntent::ToggleSalvage => {}
        // `ControlGui::handleClick` GUARDTOWER (controlgui.cpp:1629): the selected towers take the pressed state
        ControlIntent::SetGuardTower(on) => {
            let towers: Vec<PartId> = commanded(state, context)
                .into_iter()
                .filter(|part| {
                    context
                        .unit_caps
                        .get(part)
                        .is_some_and(|c| c.is_guard_tower)
                })
                .collect();
            if !towers.is_empty() {
                out.commands
                    .push(PlayerCommand::SetSuppressionFire { movers: towers, on });
            }
        }
        // `ControlGui::handleVehicleClick` (controlgui.cpp:1960): the press spends the cost, Stop returns it
        ControlIntent::BuildVehicle(kind) => out.commands.push(PlayerCommand::AddResourcePoints(
            -mc2_ui::control::vehicle_cost(kind),
        )),
        ControlIntent::CancelVehicle { refund, .. } => {
            out.commands.push(PlayerCommand::AddResourcePoints(refund))
        }
    }
}

/// One `commands[]` handler (missiongui.cpp:197-307). Returns whether the C++ handler returns 1
/// (the completed click is then consumed and no viewport order follows).
fn apply_mission(
    command: MissionCommand,
    state: &mut OrdersState,
    context: &mut BridgeContext,
    out: &mut BridgeOutput,
) -> bool {
    use MissionCommand as M;
    match command {
        M::AttackShort | M::AttackMedium | M::AttackLong => {
            let range = match command {
                M::AttackShort => FIRERANGE_SHORT,
                M::AttackMedium => FIRERANGE_MEDIUM,
                M::AttackLong => FIRERANGE_LONG,
                _ => unreachable!("the range command arms are exhaustive"),
            };
            set_attack_range(range, false, state, context, out);
            // `MissionInterfaceManager::attackShort/Medium/Long` calls `doAttack` for a
            // completed target click, after `setRange` has updated the movers.
            match (context.command_clicked, context.target) {
                (true, Some(target)) => {
                    if attack_orders(target, state, context, out) {
                        state.fire_from_current_position = false;
                    }
                    true
                }
                _ => false,
            }
        }
        M::DefaultAttack => {
            set_attack_range(FIRERANGE_OPTIMAL, false, state, context, out);
            false
        }
        M::Jump => {
            match selection_can_jump(state, context) {
                None => unavailable(
                    out,
                    "jump",
                    "selected mover jump capability is missing from world context",
                ),
                // `jump()`: INVALID_GUI, mode unchanged.
                Some(false) => {}
                Some(true) => {
                    if !state.is_jump() {
                        state.toggle_jump();
                    }
                }
            }
            false
        }
        M::StopJump => {
            if state.is_jump() {
                state.toggle_jump();
            }
            true
        }
        M::ChangeSpeed => {
            if state.is_walk() {
                state.toggle_default_speed();
            }
            false
        }
        M::StopChangeSpeed => {
            if !state.is_walk() {
                state.toggle_default_speed();
            }
            true
        }
        M::Guard => {
            if !state.is_guard() {
                state.toggle_guard();
            }
            false
        }
        M::StopGuard => {
            if state.is_guard() {
                state.toggle_guard();
            }
            false
        }
        M::FireFromCurrentPos => {
            state.fire_from_current_position = true;
            force_shot(state, context, out)
        }
        M::StopFireFromCurrentPos => {
            state.fire_from_current_position = false;
            true
        }
        M::RemoveCommand => {
            do_stop(state, context, out);
            true
        }
        M::ForceShot => force_shot(state, context, out),
        M::EnergyWeapons => {
            state.conserve_ammo = context.energy_weapons_held;
            false
        }
        M::PowerUp | M::PowerDown => {
            let code = if command == M::PowerUp {
                OrderCode::PowerUp
            } else {
                OrderCode::PowerDown
            };
            handle_orders(
                player_order(code, true, context.scenario_time),
                state,
                context,
                out,
            );
            true
        }
        M::Eject => {
            if context.command_clicked {
                // `doEject` hands the order straight to the clicked mover (missiongui.cpp:5396).
                if let Some(mover) = context.target_part.filter(|mover| {
                    !context.invulnerable && context.home_commander_order.contains(mover)
                }) {
                    out.commands.push(PlayerCommand::Order {
                        movers: vec![mover],
                        order: player_order(OrderCode::Eject, true, context.scenario_time),
                        queue: false,
                    });
                } else {
                    unavailable(
                        out,
                        "eject",
                        "picked mover is missing, not owned by the home commander, or currently invulnerable",
                    );
                }
            }
            true
        }
        M::AimLeg => aim_shot(AimPart::Leg, state, context, out),
        M::AimArm => aim_shot(AimPart::Arm, state, context, out),
        M::AimHead => aim_shot(AimPart::Head, state, context, out),
        // `conserveAmmo()` is an intentional C++ no-op; the held energyWeapons command above
        // controls `bEnergyWeapons` for attack orders.
        M::ConserveAmmo => false,
        // `sendAirstrike` / `sendSensorStrike` press the panel button (controlgui.h:252-274); the HUD owns the panel and runs
        // `ControlPanel::press_vehicle` for them, which comes back as a `BuildVehicle` intent.
        M::SendAirstrike | M::SendSensorStrike => true,
        M::VehicleCommand => {
            // Repair and salvage buttons are automatically pressed while shown. The mine
            // button retains its toggle; otherwise the hotkey controls all home towers.
            match support_mode(state, context) {
                SupportMode::Mines => state.lay_mines = !state.lay_mines,
                SupportMode::Repair | SupportMode::Salvage => {}
                mode => {
                    let on = mode != SupportMode::GuardTower;
                    let towers = context
                        .home_commander_order
                        .iter()
                        .copied()
                        .filter(|part| {
                            context
                                .unit_caps
                                .get(part)
                                .is_some_and(|c| c.is_guard_tower)
                        })
                        .collect::<Vec<_>>();
                    if !towers.is_empty() {
                        out.commands
                            .push(PlayerCommand::SetSuppressionFire { movers: towers, on });
                    }
                }
            }
            false
        }
        M::GotoNextNavMarker => {
            if let Some(point) = context.next_nav_marker {
                let mut order = player_order(OrderCode::MoveToPoint, false, context.scenario_time);
                order.move_params.way_path.points[0] = point;
                order.move_params.way_path.mode[0] = state.travel_mode();
                order.move_params.wait = false;
                order.move_params.mine_laying = state.lay_mines;
                handle_orders(order, state, context, out);
                true
            } else {
                false
            }
        }
        M::SelectVisible => {
            state.selected.clone_from(&context.visible_movers);
            roster_sort(&mut state.selected, context);
            false
        }
        M::AddVisibleToSelection => {
            for part in &context.visible_movers {
                if !state.selected.contains(part) {
                    state.selected.push(*part);
                }
            }
            roster_sort(&mut state.selected, context);
            false
        }
        M::InfoCommand => {
            state.info_selecting = true;
            false
        }
        M::InfoButtonReleased => {
            state.info_selecting = false;
            false
        }
        M::ToggleHoldPosition => {
            let movers = commanded(state, context);
            let holding = !movers.is_empty()
                && movers
                    .iter()
                    .all(|part| context.mover_attack_ranges.get(part) == Some(&FIRERANGE_CURRENT));
            set_attack_range(
                if holding {
                    FIRERANGE_OPTIMAL
                } else {
                    FIRERANGE_CURRENT
                },
                true,
                state,
                context,
                out,
            );
            false
        }
        M::Unavailable(name) => {
            unavailable(
                out,
                name,
                "mission command handler is not implemented in the Rust app",
            );
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: f32 = 12.0;

    fn context() -> BridgeContext {
        BridgeContext {
            scenario_time: T,
            home_commander_order: (0..100).map(PartId).collect(),
            ..BridgeContext::default()
        }
    }

    fn pt(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    fn state(selected: &[i32]) -> OrdersState {
        OrdersState {
            selected: selected.iter().map(|id| PartId(*id)).collect(),
            ..OrdersState::default()
        }
    }

    fn mission(command: MissionCommand) -> HudIntent {
        HudIntent::Mission(command)
    }

    /// `TacticalOrder::init(ORDER_ORIGIN_PLAYER, code, unit)` at the test's scenario time.
    fn order(code: OrderCode, unit_order: bool) -> TacticalOrder {
        player_order(code, unit_order, T)
    }

    fn cmd(movers: &[i32], order: TacticalOrder, queue: bool) -> PlayerCommand {
        PlayerCommand::Order {
            movers: movers.iter().map(|id| PartId(*id)).collect(),
            order,
            queue,
        }
    }

    /// `doMove`'s order: `initWayPath`, `wait = false`, `mode[0]` from the walk test.
    fn move_order(point: Vec3, mode: TravelMode) -> TacticalOrder {
        let mut o = order(OrderCode::MoveToPoint, false);
        o.move_params.way_path.points[0] = point;
        o.move_params.way_path.mode[0] = mode;
        o.move_params.wait = false;
        o
    }

    fn click_terrain(ctx: &mut BridgeContext, point: Vec3) {
        ctx.command_clicked = true;
        ctx.viewport_click = Some(ViewportOrderClick::Terrain(point));
    }

    fn click_object(ctx: &mut BridgeContext, target: u32) {
        ctx.command_clicked = true;
        ctx.viewport_click = Some(ViewportOrderClick::Object(WatchId(target)));
        ctx.target = Some(WatchId(target));
        ctx.target_kind = Some(AttackTargetKind::Enemy);
        ctx.target_disabled = Some(false);
    }

    fn run_intents(
        intents: &[HudIntent],
        state: &mut OrdersState,
        ctx: &BridgeContext,
    ) -> BridgeOutput {
        intents_to_commands(intents, state, ctx)
    }

    fn bits(state: &OrdersState) -> u8 {
        state.order_bits()
    }

    // ---- point orders (doMove, doGuard) -------------------------------------------------

    #[test]
    fn walking_point_move_is_one_slow_order_for_the_selection_in_roster_order() {
        let mut s = state(&[21, 8]);
        let mut ctx = context();
        ctx.world_point = Some(pt(3.0, 4.0, 1.5));
        let out = run_intents(&[HudIntent::TacMapMoveOrder((3.0, 4.0))], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(
                &[21, 8],
                move_order(pt(3.0, 4.0, 1.5), TravelMode::Slow),
                false
            )]
        );
        assert!(out.unavailable.is_empty());
        assert_eq!(out.selected, [PartId(21), PartId(8)]);
    }

    #[test]
    fn run_modifier_makes_the_move_fast_and_the_click_resets_it() {
        let mut s = state(&[5]);
        let mut ctx = context();
        click_terrain(&mut ctx, pt(2.0, 3.0, 1.0));
        let out = run_intents(
            &[HudIntent::Control(ControlIntent::SetRun(true))],
            &mut s,
            &ctx,
        );
        assert_eq!(
            out.commands,
            [cmd(
                &[5],
                move_order(pt(2.0, 3.0, 1.0), TravelMode::Fast),
                false
            )]
        );
        assert_eq!(bits(&s), 0, "doMove ends with setDefaultSpeed");
        let again = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            again.commands,
            [cmd(
                &[5],
                move_order(pt(2.0, 3.0, 1.0), TravelMode::Slow),
                false
            )]
        );
    }

    #[test]
    fn waypoint_key_queues_the_move_without_replacing_the_current_order() {
        let mut s = state(&[5, 6]);
        let mut ctx = context();
        ctx.waypoint_queue = true;
        click_terrain(&mut ctx, pt(2.0, 3.0, 1.0));
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(
                &[5, 6],
                move_order(pt(2.0, 3.0, 1.0), TravelMode::Slow),
                true
            )]
        );
    }

    #[test]
    fn mine_toggle_move_queues_and_releases_the_queue_but_a_waypoint_mine_move_only_queues() {
        let mut s = state(&[3]);
        s.lay_mines = true;
        let mut ctx = context();
        click_terrain(&mut ctx, pt(4.0, 6.0, 1.0));
        let out = run_intents(&[], &mut s, &ctx);
        let mut expected = move_order(pt(4.0, 6.0, 1.0), TravelMode::Slow);
        expected.move_params.mine_laying = true;
        // missiongui.cpp:2518: addQueuedTacOrder + setExecutingQueue(TRUE)
        assert_eq!(
            out.commands,
            [PlayerCommand::OrderQueueExecute {
                movers: vec![PartId(3)],
                order: expected.clone()
            }]
        );
        assert!(out.unavailable.is_empty());
        assert!(s.lay_mines, "doMove does not clear the mine toggle");

        ctx.waypoint_queue = true;
        let queued = run_intents(&[], &mut s, &ctx);
        assert_eq!(queued.commands, [cmd(&[3], expected, true)]);
    }

    // ---- support units (repair, salvage, repair bay, guard tower) -----------------------

    fn caps(f: impl Fn(&mut UnitCapabilities)) -> UnitCapabilities {
        let mut c = UnitCapabilities::default();
        f(&mut c);
        c
    }

    fn support_ctx(unit: UnitCapabilities, target: SupportTarget) -> BridgeContext {
        let mut ctx = context();
        ctx.unit_caps = BTreeMap::from([(PartId(3), unit)]);
        ctx.needs_refit = BTreeMap::from([(PartId(3), false)]);
        ctx.world_point = Some(pt(7.0, 8.0, 1.0));
        click_object(&mut ctx, 40);
        ctx.support_target = Some(target);
        ctx
    }

    fn damaged_friend() -> SupportTarget {
        SupportTarget {
            mover: true,
            battle_mech: true,
            needs_refit: true,
            visible: true,
            ..SupportTarget::default()
        }
    }

    #[test]
    fn repair_truck_click_on_a_damaged_friend_is_a_refit_order_and_resets_the_modes() {
        let mut s = state(&[3]);
        s.run = true;
        let ctx = support_ctx(caps(|c| c.can_repair = true), damaged_friend());
        let out = run_intents(&[], &mut s, &ctx);
        // doRepair (missiongui.cpp:4523)
        let mut expected = order(OrderCode::Refit, false);
        expected.target = WatchId(40);
        expected.selection_index = -1;
        expected.move_params.way_path.mode[0] = TravelMode::Fast;
        expected.move_params.face_object = true;
        expected.move_params.from_area = -1;
        expected.move_params.wait = false;
        assert_eq!(out.commands, [cmd(&[3], expected, false)]);
        assert!(out.unavailable.is_empty());
        assert_eq!(bits(&s), 0);
    }

    #[test]
    fn repair_truck_ignores_enemies_disabled_and_healthy_targets() {
        for target in [
            SupportTarget {
                enemy: true,
                ..damaged_friend()
            },
            SupportTarget {
                disabled: true,
                ..damaged_friend()
            },
            SupportTarget {
                needs_refit: false,
                ..damaged_friend()
            },
        ] {
            let mut s = state(&[3]);
            let ctx = support_ctx(caps(|c| c.can_repair = true), target);
            let out = run_intents(&[], &mut s, &ctx);
            assert!(
                !out.commands.iter().any(|c| matches!(
                    c,
                    PlayerCommand::Order { order, .. } if order.code == OrderCode::Refit
                )),
                "{target:?}"
            );
        }
        // a healthy mech the one selected truck is already repairing still counts (`refitBuddyWID == watchID`)
        let mut s = state(&[3]);
        let ctx = support_ctx(
            caps(|c| c.can_repair = true),
            SupportTarget {
                needs_refit: false,
                refit_buddy: Some(PartId(3)),
                ..damaged_friend()
            },
        );
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(out.commands.len(), 1);
    }

    #[test]
    fn recovery_vehicle_click_on_a_visible_disabled_mech_is_a_recover_order() {
        let mut s = state(&[3]);
        let target = SupportTarget {
            mover: true,
            battle_mech: true,
            disabled: true,
            enemy: true,
            visible: true,
            ..SupportTarget::default()
        };
        let ctx = support_ctx(caps(|c| c.can_salvage = true), target);
        let out = run_intents(&[], &mut s, &ctx);
        // doSalvage (missiongui.cpp:4470)
        let mut expected = order(OrderCode::Recover, false);
        expected.target = WatchId(40);
        expected.attack_params.attack_type = 0;
        expected.attack_params.method = AttackMethod::Ramming;
        expected.attack_params.pursue = true;
        expected.move_params.way_path.mode[0] = TravelMode::Fast;
        assert_eq!(out.commands, [cmd(&[3], expected, false)]);
        assert!(out.unavailable.is_empty());

        // not seen, not disabled, not a mech, or no room on the force bar: `canSalvage` refuses
        for refused in [
            SupportTarget {
                visible: false,
                ..target
            },
            SupportTarget {
                disabled: false,
                ..target
            },
            SupportTarget {
                battle_mech: false,
                ..target
            },
        ] {
            let ctx = support_ctx(caps(|c| c.can_salvage = true), refused);
            assert!(run_intents(&[], &mut state(&[3]), &ctx).commands.is_empty());
        }
        let mut full = support_ctx(caps(|c| c.can_salvage = true), target);
        full.force_bar_full = true;
        assert!(run_intents(&[], &mut state(&[3]), &full)
            .commands
            .is_empty());
    }

    #[test]
    fn damaged_mech_click_on_a_friendly_repair_bay_is_a_get_fixed_order() {
        let mut s = state(&[3]);
        let bay = SupportTarget {
            mech_bay: true,
            ..SupportTarget::default()
        };
        let mut ctx = support_ctx(UnitCapabilities::default(), bay);
        ctx.needs_refit = BTreeMap::from([(PartId(3), true)]);
        let out = run_intents(&[], &mut s, &ctx);
        let mut expected = order(OrderCode::GetFixed, false);
        expected.target = WatchId(40);
        expected.selection_index = -1;
        expected.move_params.way_path.mode[0] = TravelMode::Slow;
        expected.move_params.face_object = true;
        expected.move_params.from_area = -1;
        expected.move_params.wait = false;
        assert_eq!(out.commands, [cmd(&[3], expected, false)]);
        assert!(out.unavailable.is_empty());

        // `canRepairBay` wants a selected mover that needs a refit
        ctx.needs_refit = BTreeMap::from([(PartId(3), false)]);
        let out = run_intents(&[], &mut state(&[3]), &ctx);
        assert!(!out.commands.iter().any(|c| matches!(
            c,
            PlayerCommand::Order { order, .. } if order.code == OrderCode::GetFixed
        )));
    }

    #[test]
    fn guard_tower_button_sets_suppression_and_a_click_fires_at_the_point() {
        let tower = caps(|c| {
            c.is_guard_tower = true;
            c.guard_tower_suppression = true;
        });
        let mut s = state(&[3]);
        let mut ctx = context();
        ctx.unit_caps = BTreeMap::from([(PartId(3), tower)]);
        ctx.world_point = Some(pt(7.0, 8.0, 1.0));
        click_terrain(&mut ctx, pt(7.0, 8.0, 1.0));
        let out = run_intents(&[], &mut s, &ctx);
        // doGuardTower (missiongui.cpp:5493)
        let mut expected = order(OrderCode::AttackPoint, false);
        expected.attack_params.target_point = pt(7.0, 8.0, 1.0);
        expected.attack_params.attack_type = ATTACK_TO_DESTROY;
        expected.attack_params.method = AttackMethod::Ranged;
        expected.attack_params.range = FIRERANGE_CURRENT;
        expected.attack_params.pursue = false;
        expected.move_params.face_object = true;
        expected.move_params.from_area = -1;
        expected.move_params.wait = false;
        assert_eq!(
            out.commands,
            [
                PlayerCommand::SetSuppressionFire {
                    movers: vec![PartId(3)],
                    on: true
                },
                cmd(&[3], expected, false)
            ]
        );
        assert!(out.unavailable.is_empty());

        // the button itself (controlgui.cpp:1629): only the selected towers take the pressed state
        let out = run_intents(
            &[HudIntent::Control(ControlIntent::SetGuardTower(false))],
            &mut state(&[3]),
            &context_with_caps(PartId(3), tower),
        );
        assert_eq!(
            out.commands,
            [PlayerCommand::SetSuppressionFire {
                movers: vec![PartId(3)],
                on: false
            }]
        );
    }

    fn context_with_caps(part: PartId, unit: UnitCapabilities) -> BridgeContext {
        let mut ctx = context();
        ctx.unit_caps = BTreeMap::from([(part, unit)]);
        ctx
    }

    #[test]
    fn vehicle_hotkey_toggles_mine_orders_and_home_tower_suppression() {
        let mut s = state(&[3]);
        let mut ctx = context_with_caps(PartId(3), caps(|c| c.can_lay_mines = true));
        run_intents(&[mission(MissionCommand::VehicleCommand)], &mut s, &ctx);
        assert!(s.lay_mines);
        click_terrain(&mut ctx, pt(2.0, 3.0, 0.0));
        let out = run_intents(&[], &mut s, &ctx);
        assert!(
            matches!(&out.commands[0], PlayerCommand::OrderQueueExecute { order, .. } if order.move_params.mine_laying)
        );
        ctx.viewport_click = None;
        run_intents(&[mission(MissionCommand::VehicleCommand)], &mut s, &ctx);
        assert!(!s.lay_mines);

        let mut tower = caps(|c| c.is_guard_tower = true);
        ctx.unit_caps.insert(PartId(3), tower);
        ctx.home_commander_order = vec![PartId(3)];
        let out = run_intents(&[mission(MissionCommand::VehicleCommand)], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [PlayerCommand::SetSuppressionFire {
                movers: vec![PartId(3)],
                on: true
            }]
        );
        tower.guard_tower_suppression = true;
        ctx.unit_caps.insert(PartId(3), tower);
        let out = run_intents(&[mission(MissionCommand::VehicleCommand)], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [PlayerCommand::SetSuppressionFire {
                movers: vec![PartId(3)],
                on: false
            }]
        );
    }

    #[test]
    fn repair_and_salvage_toggles_are_accepted_without_an_unavailable_report() {
        let mut s = state(&[3]);
        let out = run_intents(
            &[
                HudIntent::Control(ControlIntent::ToggleRepair),
                HudIntent::Control(ControlIntent::ToggleSalvage),
            ],
            &mut s,
            &context(),
        );
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
    }

    #[test]
    fn guard_point_click_is_a_fast_guard_order_and_resets_the_modes() {
        let mut s = state(&[3]);
        let mut ctx = context();
        click_terrain(&mut ctx, pt(4.0, 6.0, 0.5));
        let out = run_intents(&[mission(MissionCommand::Guard)], &mut s, &ctx);
        let mut expected = order(OrderCode::Guard, false);
        expected.move_params.way_path.points[0] = pt(4.0, 6.0, 0.5);
        expected.move_params.way_path.mode[0] = TravelMode::Fast;
        assert_eq!(out.commands, [cmd(&[3], expected, false)]);
        assert_eq!(bits(&s), 0);
    }

    #[test]
    fn guard_object_click_targets_the_watch_id_and_resets_the_modes() {
        let mut s = state(&[5]);
        s.guard = true;
        let mut ctx = context();
        ctx.command_clicked = true;
        ctx.viewport_click = Some(ViewportOrderClick::Object(WatchId(22)));
        let out = run_intents(&[], &mut s, &ctx);
        let mut expected = order(OrderCode::Guard, false);
        expected.target = WatchId(22);
        assert_eq!(out.commands, [cmd(&[5], expected, false)]);
        assert_eq!(bits(&s), 0);
    }

    #[test]
    fn run_plus_guard_is_not_getguard_so_a_terrain_click_is_a_fast_move() {
        // controlgui.cpp:1399: getGuard() is `curOrder == GUARD`; RUN|GUARD falls to doMove.
        let mut s = state(&[5]);
        s.run = true;
        s.guard = true;
        let mut ctx = context();
        click_terrain(&mut ctx, pt(1.0, 1.0, 0.0));
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(
                &[5],
                move_order(pt(1.0, 1.0, 0.0), TravelMode::Fast),
                false
            )]
        );
    }

    #[test]
    fn space_release_keeps_guard_for_the_next_terrain_click() {
        // Run (Space) -> Guard -> Space release (missiongui.cpp:2305, controlgui.cpp:1363).
        let mut s = state(&[5]);
        let ctx = context();
        run_intents(&[mission(MissionCommand::ChangeSpeed)], &mut s, &ctx);
        assert_eq!(bits(&s), RUN);
        run_intents(&[mission(MissionCommand::Guard)], &mut s, &ctx);
        assert_eq!(bits(&s), RUN | GUARD);
        run_intents(&[mission(MissionCommand::StopChangeSpeed)], &mut s, &ctx);
        assert_eq!(bits(&s), GUARD);
        let mut click = context();
        click_terrain(&mut click, pt(9.0, 8.0, 0.0));
        let out = run_intents(&[], &mut s, &click);
        let mut expected = order(OrderCode::Guard, false);
        expected.move_params.way_path.points[0] = pt(9.0, 8.0, 0.0);
        expected.move_params.way_path.mode[0] = TravelMode::Fast;
        assert_eq!(out.commands, [cmd(&[5], expected, false)]);
    }

    // ---- control-gui bit masks -----------------------------------------------------------

    #[test]
    fn mission_hotkeys_follow_the_controlgui_bit_toggles() {
        // (start bits, command, resulting bits), each derived from controlgui.cpp:1351-1397 and
        // missiongui.cpp:1949-1967, 2061-2072, 2298-2309.
        let cases = [
            (0, MissionCommand::ChangeSpeed, RUN),
            (GUARD, MissionCommand::ChangeSpeed, GUARD), // not the default speed: no-op
            (RUN, MissionCommand::StopChangeSpeed, 0),
            (0, MissionCommand::StopChangeSpeed, 0),
            (RUN | GUARD, MissionCommand::StopChangeSpeed, GUARD),
            (0, MissionCommand::Guard, GUARD),
            (RUN, MissionCommand::Guard, RUN | GUARD),
            (JUMP, MissionCommand::Guard, GUARD),
            (GUARD, MissionCommand::Guard, GUARD),
            (RUN | GUARD, MissionCommand::Guard, RUN), // not getGuard(): toggles guard off
            (GUARD, MissionCommand::StopGuard, 0),
            (RUN | GUARD, MissionCommand::StopGuard, RUN | GUARD),
            (0, MissionCommand::Jump, JUMP),
            (RUN | GUARD, MissionCommand::Jump, JUMP),
            (JUMP, MissionCommand::StopJump, 0),
            (RUN, MissionCommand::StopJump, RUN),
        ];
        for (start, command, expected) in cases {
            let mut s = state(&[5]);
            s.set_order_bits(start);
            let mut ctx = context();
            ctx.jump_capable.insert(PartId(5), true);
            run_intents(&[mission(command)], &mut s, &ctx);
            assert_eq!(bits(&s), expected, "{start:#b} {command:?}");
        }
    }

    #[test]
    fn panel_buttons_apply_the_same_masks() {
        let mut s = state(&[5]);
        let ctx = context();
        run_intents(
            &[HudIntent::Control(ControlIntent::ToggleJump)],
            &mut s,
            &ctx,
        );
        assert_eq!(bits(&s), JUMP);
        run_intents(
            &[HudIntent::Control(ControlIntent::SetRun(true))],
            &mut s,
            &ctx,
        );
        assert_eq!(bits(&s), RUN, "Run clears jump");
        run_intents(
            &[HudIntent::Control(ControlIntent::SetGuard(true))],
            &mut s,
            &ctx,
        );
        assert_eq!(bits(&s), RUN | GUARD);
        run_intents(
            &[HudIntent::Control(ControlIntent::ToggleJump)],
            &mut s,
            &ctx,
        );
        assert_eq!(bits(&s), JUMP, "Jump clears run and guard");
    }

    // ---- stop, power, eject --------------------------------------------------------------

    #[test]
    fn stop_button_and_remove_command_issue_one_direct_stop_and_reset_everything() {
        for intent in [
            HudIntent::Control(ControlIntent::Stop),
            mission(MissionCommand::RemoveCommand),
        ] {
            let mut s = state(&[6, 2]);
            s.run = true;
            s.guard = true;
            s.lay_mines = true;
            s.fire_from_current_position = true;
            let mut ctx = context();
            ctx.waypoint_queue = true; // doStop never goes through handleOrders' queue
            let out = run_intents(&[intent], &mut s, &ctx);
            assert_eq!(
                out.commands,
                [cmd(&[6, 2], order(OrderCode::Stop, false), false)]
            );
            assert_eq!(bits(&s), 0);
            assert!(!s.lay_mines && !s.fire_from_current_position);
        }
    }

    #[test]
    fn stop_with_an_empty_selection_issues_nothing_but_still_resets_modes() {
        let mut s = OrdersState {
            run: true,
            ..OrdersState::default()
        };
        let out = run_intents(
            &[HudIntent::Control(ControlIntent::Stop)],
            &mut s,
            &context(),
        );
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
        assert!(!s.run);
    }

    #[test]
    fn power_up_and_down_are_unit_orders_through_handle_orders() {
        for (command, code) in [
            (MissionCommand::PowerUp, OrderCode::PowerUp),
            (MissionCommand::PowerDown, OrderCode::PowerDown),
        ] {
            let mut s = state(&[1, 2]);
            let out = run_intents(&[mission(command)], &mut s, &context());
            assert_eq!(out.commands, [cmd(&[1, 2], order(code, true), false)]);
            let mut ctx = context();
            ctx.waypoint_queue = true;
            let queued = run_intents(&[mission(command)], &mut s, &ctx);
            assert_eq!(queued.commands, [cmd(&[1, 2], order(code, true), true)]);
        }
    }

    #[test]
    fn eject_goes_straight_to_the_clicked_home_mover() {
        let mut s = state(&[1, 2]);
        let mut ctx = context();
        ctx.command_clicked = true;
        ctx.target_part = Some(PartId(2));
        ctx.roster_order = vec![PartId(2)];
        ctx.home_commander_order = vec![PartId(2)];
        let out = run_intents(&[mission(MissionCommand::Eject)], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(&[2], order(OrderCode::Eject, true), false)]
        );

        ctx.invulnerable = true;
        let refused = run_intents(&[mission(MissionCommand::Eject)], &mut s, &ctx);
        assert!(refused.commands.is_empty());
        assert_eq!(refused.unavailable[0].action, "eject");
    }

    #[test]
    fn eject_requires_home_commander_ownership_not_only_team_membership() {
        let mut s = OrdersState::default();
        let mut ctx = context();
        ctx.command_clicked = true;
        ctx.target_part = Some(PartId(2));
        ctx.roster_order = vec![PartId(2)];
        ctx.home_commander_order.clear();
        let out = run_intents(&[mission(MissionCommand::Eject)], &mut s, &ctx);
        assert!(out.commands.is_empty());
        assert_eq!(out.unavailable[0].action, "eject");
    }

    // ---- attack (doAttack) ---------------------------------------------------------------

    fn attack_order(target: u32, range: i32, pursue: bool, mode: TravelMode) -> TacticalOrder {
        let mut o = order(OrderCode::AttackObject, false);
        o.target = WatchId(target);
        o.attack_params.attack_type = ATTACK_TO_DESTROY;
        o.attack_params.method = AttackMethod::Ranged;
        o.attack_params.range = range;
        o.attack_params.pursue = pursue;
        o.move_params.way_path.mode[0] = mode;
        o
    }

    #[test]
    fn default_attack_stamps_each_movers_range_and_pursuit() {
        let mut s = state(&[8, 9, 4]);
        let mut ctx = context();
        click_object(&mut ctx, 31);
        ctx.mover_attack_ranges = BTreeMap::from([
            (PartId(8), FIRERANGE_OPTIMAL),
            (PartId(9), FIRERANGE_OPTIMAL),
            (PartId(4), FIRERANGE_CURRENT),
        ]);
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [
                cmd(
                    &[8, 9],
                    attack_order(31, FIRERANGE_OPTIMAL, true, TravelMode::Slow),
                    false
                ),
                cmd(
                    &[4],
                    attack_order(31, FIRERANGE_CURRENT, false, TravelMode::Slow),
                    false
                ),
            ]
        );
        assert!(out.unavailable.is_empty());
    }

    #[test]
    fn attack_modifiers_run_energy_and_current_position() {
        let mut s = state(&[4]);
        s.run = true;
        s.conserve_ammo = true;
        s.fire_from_current_position = true;
        let mut ctx = context();
        click_object(&mut ctx, 31);
        ctx.mover_attack_ranges = BTreeMap::from([(PartId(4), 1)]);
        let out = run_intents(&[], &mut s, &ctx);
        let mut expected = attack_order(31, 1, false, TravelMode::Fast);
        expected.attack_params.attack_type = ATTACK_CONSERVING_AMMO;
        assert_eq!(out.commands, [cmd(&[4], expected, false)]);
        assert!(!s.fire_from_current_position, "doAttack clears it");
        assert!(s.run, "doAttack does not call setDefaultSpeed");
    }

    #[test]
    fn attack_gui_refusals_issue_nothing_and_leave_state() {
        let mut ctx = context();
        click_object(&mut ctx, 31);
        ctx.mover_attack_ranges = BTreeMap::from([(PartId(4), 1)]);

        let mut disabled = ctx.clone();
        disabled.target_disabled = Some(true);
        let mut s = state(&[4]);
        s.fire_from_current_position = true;
        let out = run_intents(&[], &mut s, &disabled);
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
        assert!(s.fire_from_current_position);

        let mut waypoint = ctx.clone();
        waypoint.waypoint_queue = true;
        let out = run_intents(&[], &mut s, &waypoint);
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
    }

    #[test]
    fn attack_reports_missing_or_unported_inputs_instead_of_guessing() {
        let mut ctx = context();
        click_object(&mut ctx, 31);
        ctx.mover_attack_ranges = BTreeMap::from([(PartId(4), 1)]);

        let mut s = state(&[4]);
        let mut missing_range = ctx.clone();
        missing_range.mover_attack_ranges.clear();
        let out = run_intents(&[], &mut s, &missing_range);
        assert!(out.commands.is_empty());
        assert_eq!(out.unavailable[0].action, "attack");

        let mut no_class = ctx.clone();
        no_class.target_kind = None;
        let out = run_intents(&[], &mut s, &no_class);
        assert!(out.commands.is_empty());
        assert_eq!(out.unavailable[0].action, "attack");

        let mut capture = ctx.clone();
        capture.target_kind = Some(AttackTargetKind::Captureable);
        capture.capture_movers.insert(PartId(4), true);
        capture.capture_blockers.insert(PartId(4), false);
        let out = run_intents(&[], &mut s, &capture);
        let mut expected_capture = order(OrderCode::Capture, false);
        expected_capture.target = WatchId(31);
        expected_capture.attack_params.attack_type = 0;
        expected_capture.attack_params.method = AttackMethod::Ramming;
        expected_capture.attack_params.pursue = true;
        expected_capture.move_params.way_path.mode[0] = TravelMode::Slow;
        assert_eq!(out.commands, [cmd(&[4], expected_capture, false)]);
        assert!(out.unavailable.is_empty());

        capture.capture_blockers.insert(PartId(4), true);
        let out = run_intents(&[], &mut s, &capture);
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
        assert!(
            out.play_invalid_gui,
            "blocked capture click plays INVALID_GUI"
        );

        capture.capture_blockers.insert(PartId(4), false);
        capture.capture_movers.insert(PartId(4), false);
        let out = run_intents(&[], &mut s, &capture);
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
        assert!(
            out.play_invalid_gui,
            "unsupported selected mover plays INVALID_GUI"
        );

        let mut friendly = ctx.clone();
        friendly.target_kind = Some(AttackTargetKind::Friendly);
        let out = run_intents(&[], &mut s, &friendly);
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
        assert!(!out.play_invalid_gui);
    }

    #[test]
    fn jump_armed_object_click_attacks_like_the_cpp_branch_chain() {
        // Only getGuard() is tested before doAttack (missiongui.cpp:1380-1451).
        let mut s = state(&[4]);
        s.jump = true;
        let mut ctx = context();
        click_object(&mut ctx, 31);
        ctx.mover_attack_ranges = BTreeMap::from([(PartId(4), FIRERANGE_OPTIMAL)]);
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(
                &[4],
                attack_order(31, FIRERANGE_OPTIMAL, true, TravelMode::Fast),
                false
            )]
        );
    }

    #[test]
    fn hold_position_sets_selected_sim_range_and_updates_the_current_order() {
        let mut s = state(&[3]);
        let out = run_intents(
            &[HudIntent::Control(ControlIntent::SetRange(
                FireRange::Current,
            ))],
            &mut s,
            &context(),
        );
        assert_eq!(s.attack_range, FIRERANGE_CURRENT);
        assert!(out.unavailable.is_empty());
        assert!(
            matches!(out.commands.as_slice(), [PlayerCommand::SetAttackRange {
            movers, range: FIRERANGE_CURRENT, update_current_order: true, ..
        }] if movers == &[PartId(3)])
        );
    }

    #[test]
    fn control_panel_ranges_set_selected_sim_movers_without_changing_current_orders() {
        for (range, expected) in [
            (FireRange::Short, FIRERANGE_SHORT),
            (FireRange::Medium, FIRERANGE_MEDIUM),
            (FireRange::Long, FIRERANGE_LONG),
            (FireRange::Optimal, FIRERANGE_OPTIMAL),
        ] {
            let mut s = state(&[3]);
            let out = run_intents(
                &[HudIntent::Control(ControlIntent::SetRange(range))],
                &mut s,
                &context(),
            );
            assert!(out.unavailable.is_empty());
            assert_eq!(s.attack_range, expected);
            assert!(
                matches!(out.commands.as_slice(), [PlayerCommand::SetAttackRange {
                movers, range, update_current_order: false, ..
            }] if movers == &[PartId(3)] && *range == expected)
            );
        }
        let mut s = state(&[5]);
        let mut ctx = context();
        click_object(&mut ctx, 33);
        ctx.mover_attack_ranges = BTreeMap::from([(PartId(5), FIRERANGE_OPTIMAL)]);
        let out = run_intents(&[mission(MissionCommand::AttackShort)], &mut s, &ctx);
        assert!(out.unavailable.is_empty());
        assert!(matches!(out.commands.as_slice(), [
            PlayerCommand::SetAttackRange { range: FIRERANGE_SHORT, .. },
            PlayerCommand::Order { order, .. }
        ] if order.code == OrderCode::AttackObject && order.attack_params.range == FIRERANGE_SHORT));
        s.attack_range = 2;
        let out = run_intents(
            &[mission(MissionCommand::DefaultAttack)],
            &mut s,
            &context(),
        );
        assert_eq!(s.attack_range, FIRERANGE_OPTIMAL);
        assert!(out.unavailable.is_empty());
        assert!(matches!(
            out.commands.as_slice(),
            [PlayerCommand::SetAttackRange {
                range: FIRERANGE_OPTIMAL,
                ..
            }]
        ));
    }

    // ---- forced and aimed shots ----------------------------------------------------------

    #[test]
    fn force_shot_at_an_object_and_at_the_ground() {
        let mut s = state(&[6]);
        let mut ctx = context();
        ctx.command_clicked = true;
        ctx.target = Some(WatchId(9));
        let at_object = run_intents(&[mission(MissionCommand::ForceShot)], &mut s, &ctx);
        assert_eq!(
            at_object.commands,
            [cmd(
                &[6],
                attack_order(9, FIRERANGE_OPTIMAL, true, TravelMode::Slow),
                false
            )]
        );

        ctx.target = None;
        ctx.world_point = Some(pt(8.0, 5.0, 2.0));
        ctx.viewport_click = Some(ViewportOrderClick::Terrain(pt(8.0, 5.0, 2.0)));
        s.run = true;
        s.conserve_ammo = true;
        s.fire_from_current_position = true;
        ctx.waypoint_queue = true;
        let at_ground = run_intents(&[mission(MissionCommand::ForceShot)], &mut s, &ctx);
        let mut expected = order(OrderCode::AttackPoint, false);
        expected.attack_params.attack_type = ATTACK_CONSERVING_AMMO;
        expected.attack_params.range = FIRERANGE_OPTIMAL;
        expected.attack_params.pursue = false;
        expected.attack_params.target_point = pt(8.0, 5.0, 2.0);
        expected.move_params.way_path.mode[0] = TravelMode::Fast;
        assert_eq!(at_ground.commands, [cmd(&[6], expected, true)]);
    }

    #[test]
    fn fire_from_current_pos_hotkey_arms_the_flag_then_force_shoots() {
        let mut s = state(&[6]);
        let mut ctx = context();
        ctx.command_clicked = true;
        ctx.target = Some(WatchId(9));
        let out = run_intents(&[mission(MissionCommand::FireFromCurrentPos)], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(
                &[6],
                attack_order(9, FIRERANGE_OPTIMAL, false, TravelMode::Slow),
                false
            )]
        );
        assert!(s.fire_from_current_position);
    }

    #[test]
    fn a_consumed_force_shot_click_does_not_fall_through_to_a_move() {
        let mut s = state(&[6]);
        let mut ctx = context();
        click_terrain(&mut ctx, pt(8.0, 5.0, 2.0));
        ctx.world_point = Some(pt(8.0, 5.0, 2.0));
        let out = run_intents(&[mission(MissionCommand::ForceShot)], &mut s, &ctx);
        assert_eq!(out.commands.len(), 1);
        assert!(matches!(
            &out.commands[0],
            PlayerCommand::Order { order, .. } if order.code == OrderCode::AttackPoint
        ));
    }

    fn aim_facts() -> AimFacts {
        AimFacts {
            ground_mech: true,
            any_selected_non_area_weapon: true,
            ..AimFacts::default()
        }
    }

    fn aimed(command: MissionCommand, facts: Option<AimFacts>) -> BridgeOutput {
        let mut s = state(&[3]);
        let mut ctx = context();
        ctx.command_clicked = true;
        ctx.target = Some(WatchId(9));
        ctx.aim = facts;
        run_intents(&[mission(command)], &mut s, &ctx)
    }

    fn aimed_order(location: i32) -> TacticalOrder {
        let mut o = attack_order(9, FIRERANGE_OPTIMAL, true, TravelMode::Slow);
        o.attack_params.aim_location = location;
        o
    }

    #[test]
    fn aimed_shots_pick_the_cpp_body_location() {
        let healthy = aim_facts();
        let cases = [
            (MissionCommand::AimHead, healthy, BODY_HEAD),
            (MissionCommand::AimArm, healthy, BODY_LEFT_ARM),
            (MissionCommand::AimLeg, healthy, BODY_LEFT_LEG),
            (
                MissionCommand::AimArm,
                AimFacts {
                    left_arm_destroyed: true,
                    ..healthy
                },
                BODY_RIGHT_ARM,
            ),
            (
                MissionCommand::AimLeg,
                AimFacts {
                    left_leg_destroyed: true,
                    ..healthy
                },
                BODY_RIGHT_LEG,
            ),
        ];
        for (command, facts, location) in cases {
            let out = aimed(command, Some(facts));
            assert_eq!(
                out.commands,
                [cmd(&[3], aimed_order(location), false)],
                "{command:?}"
            );
        }
    }

    #[test]
    fn aimed_shot_refusals_issue_nothing() {
        let healthy = aim_facts();
        let refused = [
            (
                MissionCommand::AimHead,
                AimFacts {
                    any_selected_non_area_weapon: false,
                    ..healthy
                },
            ),
            (
                MissionCommand::AimHead,
                AimFacts {
                    head_destroyed: true,
                    ..healthy
                },
            ),
            (
                MissionCommand::AimArm,
                AimFacts {
                    left_arm_destroyed: true,
                    right_arm_destroyed: true,
                    ..healthy
                },
            ),
            (
                MissionCommand::AimLeg,
                AimFacts {
                    left_leg_destroyed: true,
                    right_leg_destroyed: true,
                    ..healthy
                },
            ),
            (
                MissionCommand::AimArm,
                AimFacts {
                    ground_mech: false,
                    ..healthy
                },
            ),
        ];
        for (command, facts) in refused {
            let out = aimed(command, Some(facts));
            assert!(out.commands.is_empty(), "{command:?} {facts:?}");
            assert!(out.unavailable.is_empty(), "{command:?} {facts:?}");
        }
        let missing = aimed(MissionCommand::AimHead, None);
        assert!(missing.commands.is_empty());
        assert_eq!(missing.unavailable[0].action, "aimed shot");
    }

    #[test]
    fn aimed_shot_on_a_non_mech_leg_or_head_falls_through_to_the_normal_attack() {
        // aimLeg/aimHead return 0 for a non-mech target; aimArm returns 1.
        let mut s = state(&[3]);
        let mut ctx = context();
        click_object(&mut ctx, 9);
        ctx.mover_attack_ranges = BTreeMap::from([(PartId(3), FIRERANGE_OPTIMAL)]);
        ctx.aim = Some(AimFacts::default());
        let out = run_intents(&[mission(MissionCommand::AimLeg)], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(
                &[3],
                attack_order(9, FIRERANGE_OPTIMAL, true, TravelMode::Slow),
                false
            )]
        );
        let out = run_intents(&[mission(MissionCommand::AimArm)], &mut s, &ctx);
        assert!(out.commands.is_empty());
    }

    // ---- jump ----------------------------------------------------------------------------

    fn jump_ctx(capable: &[(i32, bool)], landing: Option<bool>) -> BridgeContext {
        let mut ctx = context();
        ctx.jump_capable = capable.iter().map(|(p, c)| (PartId(*p), *c)).collect();
        ctx.jump_landing_ok = landing;
        ctx
    }

    fn jump_order(point: Vec3) -> TacticalOrder {
        let mut o = order(OrderCode::JumpToPoint, false);
        o.move_params.wait = false;
        o.move_params.way_path.mode[0] = TravelMode::Jump;
        o.move_params.way_path.points[0] = point;
        o
    }

    #[test]
    fn jump_hotkey_requires_every_selected_mover_to_be_able_to_jump() {
        let mut s = state(&[5, 6]);
        let ineligible = jump_ctx(&[(5, true), (6, false)], None);
        let out = run_intents(&[mission(MissionCommand::Jump)], &mut s, &ineligible);
        assert!(!s.jump && out.unavailable.is_empty() && out.commands.is_empty());

        let unknown = jump_ctx(&[(5, true)], None);
        let out = run_intents(&[mission(MissionCommand::Jump)], &mut s, &unknown);
        assert!(!s.jump);
        assert_eq!(out.unavailable[0].action, "jump");

        let eligible = jump_ctx(&[(5, true), (6, true)], None);
        run_intents(&[mission(MissionCommand::Jump)], &mut s, &eligible);
        assert!(s.jump);
    }

    #[test]
    fn jump_click_for_a_capable_single_mover_is_a_jump_to_point_order() {
        let mut s = state(&[5]);
        s.jump = true;
        let mut ctx = jump_ctx(&[(5, true)], Some(true));
        click_terrain(&mut ctx, pt(6.0, 7.0, 1.0));
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(&[5], jump_order(pt(6.0, 7.0, 1.0)), false)]
        );
        assert_eq!(bits(&s), 0, "doJump ends with setDefaultSpeed");
    }

    #[test]
    fn jump_click_is_refused_for_an_incapable_mover_or_bad_landing_and_stays_armed() {
        for (capable, landing) in [(false, Some(true)), (true, Some(false))] {
            let mut s = state(&[5]);
            s.jump = true;
            let mut ctx = jump_ctx(&[(5, capable)], landing);
            click_terrain(&mut ctx, pt(6.0, 7.0, 1.0));
            let out = run_intents(&[], &mut s, &ctx);
            assert!(out.commands.is_empty() && out.unavailable.is_empty());
            assert!(s.jump);
        }
        let mut s = state(&[5]);
        s.jump = true;
        let mut ctx = jump_ctx(&[(5, true)], None);
        click_terrain(&mut ctx, pt(6.0, 7.0, 1.0));
        let out = run_intents(&[], &mut s, &ctx);
        assert!(out.commands.is_empty());
        assert_eq!(out.unavailable[0].action, "jump");
    }

    #[test]
    fn group_jump_reaches_the_sim_for_goal_assignment() {
        let mut s = state(&[4, 5]);
        s.jump = true;
        let mut ctx = jump_ctx(&[(4, true), (5, true)], Some(true));
        click_terrain(&mut ctx, pt(6.0, 7.0, 0.0));
        let out = run_intents(&[], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [cmd(&[4, 5], jump_order(pt(6.0, 7.0, 0.0)), false)]
        );
        assert!(out.unavailable.is_empty());
        assert!(!s.jump);
    }

    // ---- support strikes -----------------------------------------------------------------

    #[test]
    fn a_click_with_a_painted_strike_issues_one_call_strike_at_the_point() {
        for strike in [0, 1, 2] {
            for click in [
                ViewportOrderClick::Terrain(pt(2.0, 3.0, 0.5)),
                ViewportOrderClick::Object(WatchId(9)),
            ] {
                let mut s = state(&[1]);
                let mut ctx = context();
                ctx.strike = Some(strike);
                ctx.command_clicked = true;
                ctx.viewport_click = Some(click);
                ctx.world_point = Some(pt(2.0, 3.0, 0.5));
                let out = run_intents(&[], &mut s, &ctx);
                assert_eq!(
                    out.commands,
                    [PlayerCommand::CallStrike {
                        strike,
                        at: pt(2.0, 3.0, 0.5)
                    }]
                );
                assert!(out.strike_called && out.unavailable.is_empty());
            }
        }
    }

    #[test]
    fn a_strike_click_without_a_world_point_is_reported_and_calls_nothing() {
        let mut s = state(&[1]);
        let mut ctx = context();
        ctx.strike = Some(2);
        ctx.command_clicked = true;
        ctx.viewport_click = Some(ViewportOrderClick::Object(WatchId(9)));
        let out = run_intents(&[], &mut s, &ctx);
        assert!(out.commands.is_empty() && !out.strike_called);
        assert_eq!(out.unavailable[0].action, "support strike");
    }

    #[test]
    fn strike_panel_press_spends_the_cost_and_cancel_refunds_without_a_call() {
        for kind in [VehicleKind::Airstrike, VehicleKind::SensorProbe] {
            let cost = mc2_ui::control::vehicle_cost(kind);
            let mut s = state(&[1]);
            let out = run_intents(
                &[HudIntent::Control(ControlIntent::BuildVehicle(kind))],
                &mut s,
                &context(),
            );
            assert_eq!(out.commands, [PlayerCommand::AddResourcePoints(-cost)]);
            assert!(out.unavailable.is_empty() && !out.strike_called);
            let out = run_intents(
                &[HudIntent::Control(ControlIntent::CancelVehicle {
                    kind,
                    refund: cost,
                })],
                &mut s,
                &context(),
            );
            assert_eq!(out.commands, [PlayerCommand::AddResourcePoints(cost)]);
            assert!(!out.strike_called);
            // disarmed: the next click is an ordinary move
            let mut ctx = context();
            click_terrain(&mut ctx, pt(2.0, 3.0, 0.5));
            let out = run_intents(&[], &mut s, &ctx);
            assert!(matches!(
                &out.commands[..],
                [PlayerCommand::Order { order, .. }] if order.code == OrderCode::MoveToPoint
            ));
        }
    }

    #[test]
    fn strike_hotkeys_only_consume_the_click_the_panel_press_is_the_huds() {
        for command in [
            MissionCommand::SendAirstrike,
            MissionCommand::SendSensorStrike,
        ] {
            let mut s = state(&[1]);
            let mut ctx = context();
            click_terrain(&mut ctx, pt(2.0, 3.0, 0.5));
            let out = run_intents(&[mission(command)], &mut s, &ctx);
            assert!(out.commands.is_empty() && out.unavailable.is_empty());
        }
    }

    #[test]
    fn aim_hotkeys_without_a_target_consume_the_command_without_moving() {
        for command in [
            MissionCommand::AimLeg,
            MissionCommand::AimHead,
            MissionCommand::AimArm,
        ] {
            let mut s = state(&[1]);
            let mut ctx = context();
            click_terrain(&mut ctx, pt(4.0, 2.0, 0.0));
            let out = run_intents(&[mission(command)], &mut s, &ctx);
            assert!(out.unavailable.is_empty());
            assert!(out.commands.is_empty());
            assert_eq!(out.selected, [PartId(1)]);
        }
    }

    #[test]
    fn tacmap_click_with_painted_strike_calls_it_like_a_world_click_otherwise_moves() {
        let mut ctx = context();
        ctx.world_point = Some(pt(1.0, 2.0, 0.0));
        let mut s = state(&[1]);
        // the sensor probe's TacMap click calls the strike like a world click
        ctx.strike = Some(ARTILLERY_SENSOR);
        let out = run_intents(&[HudIntent::TacMapMoveOrder((1.0, 2.0))], &mut s, &ctx);
        assert_eq!(
            out.commands,
            [PlayerCommand::CallStrike {
                strike: ARTILLERY_SENSOR,
                at: pt(1.0, 2.0, 0.0)
            }]
        );
        assert!(out.strike_called);
        let mut click = ctx.clone();
        click_terrain(&mut click, pt(1.0, 2.0, 0.0));
        let via_world = run_intents(&[], &mut s, &click);
        assert_eq!(via_world.commands, out.commands);
        // the large airstrike keeps `doMove`'s move on a TacMap click (missiongui.cpp:3137)
        ctx.strike = Some(1);
        let out = run_intents(&[HudIntent::TacMapMoveOrder((1.0, 2.0))], &mut s, &ctx);
        assert!(!out.strike_called);
        assert!(matches!(
            &out.commands[0],
            PlayerCommand::Order { order, .. } if order.code == OrderCode::MoveToPoint
        ));
        // a TacMap point that does not match the resolved elevation point is refused, never called
        ctx.strike = Some(2);
        ctx.world_point = Some(pt(9.0, 9.0, 0.0));
        let out = run_intents(&[HudIntent::TacMapMoveOrder((1.0, 2.0))], &mut s, &ctx);
        assert!(out.commands.is_empty() && !out.strike_called);
        assert_eq!(out.unavailable[0].action, "move");
        ctx.world_point = Some(pt(1.0, 2.0, 0.0));
        ctx.strike = None;
        let out = run_intents(&[HudIntent::TacMapMoveOrder((1.0, 2.0))], &mut s, &ctx);
        assert!(matches!(
            &out.commands[0],
            PlayerCommand::Order { order, .. } if order.code == OrderCode::MoveToPoint
        ));
    }

    #[test]
    fn orders_reach_only_home_commander_movers() {
        // Part 1 and 3 are the home commander's; part 2 is an allied mover that is also selected.
        let mut ctx = context();
        ctx.home_commander_order = vec![PartId(1), PartId(3)];
        ctx.roster_order = vec![PartId(1), PartId(2), PartId(3)];
        ctx.target = Some(WatchId(9));
        ctx.target_kind = Some(AttackTargetKind::Enemy);
        ctx.target_disabled = Some(false);
        ctx.mover_attack_ranges = BTreeMap::from([
            (PartId(1), FIRERANGE_OPTIMAL),
            (PartId(2), FIRERANGE_OPTIMAL),
            (PartId(3), FIRERANGE_OPTIMAL),
        ]);
        let movers_of = |out: &BridgeOutput| -> Vec<Vec<PartId>> {
            out.commands
                .iter()
                .map(|c| match c {
                    PlayerCommand::Order { movers, .. } => movers.clone(),
                    other => panic!("unexpected {other:?}"),
                })
                .collect()
        };
        let home = vec![PartId(1), PartId(3)];

        let mut s = state(&[1, 2, 3]);
        let mut c = ctx.clone();
        click_terrain(&mut c, pt(2.0, 3.0, 0.5));
        assert_eq!(
            movers_of(&run_intents(&[], &mut s, &c)),
            std::slice::from_ref(&home)
        );

        let mut s = state(&[1, 2, 3]);
        let mut c = ctx.clone();
        click_object(&mut c, 9);
        assert_eq!(
            movers_of(&run_intents(&[], &mut s, &c)),
            std::slice::from_ref(&home)
        );

        let mut s = state(&[1, 2, 3]);
        let out = run_intents(&[HudIntent::Control(ControlIntent::Stop)], &mut s, &ctx);
        assert_eq!(movers_of(&out), std::slice::from_ref(&home));

        // Only an allied mover selected: nothing is issued.
        let mut s = state(&[2]);
        let mut c = ctx.clone();
        click_terrain(&mut c, pt(2.0, 3.0, 0.5));
        assert!(run_intents(&[], &mut s, &c).commands.is_empty());
    }

    #[test]
    fn tacmap_move_needs_a_matching_resolved_world_point() {
        let mut s = state(&[1]);
        let out = run_intents(
            &[HudIntent::TacMapMoveOrder((2.0, 3.0))],
            &mut s,
            &context(),
        );
        assert!(out.commands.is_empty());
        assert_eq!(out.unavailable.len(), 1);
        let mut ctx = context();
        ctx.world_point = Some(pt(9.0, 9.0, 0.0));
        let out = run_intents(&[HudIntent::TacMapMoveOrder((2.0, 3.0))], &mut s, &ctx);
        assert!(out.commands.is_empty());
        assert_eq!(out.unavailable.len(), 1);
    }

    // ---- selection and remaining gaps ------------------------------------------------------

    #[test]
    fn same_frame_unitbar_selection_is_sorted_before_order_emission() {
        let mut s = OrdersState::default();
        let mut ctx = context();
        ctx.unit_bar_parts = BTreeMap::from([(40, PartId(8)), (41, PartId(9))]);
        ctx.roster_order = vec![PartId(9), PartId(8)];
        ctx.world_point = Some(pt(1.0, 2.0, 0.0));
        let out = run_intents(
            &[
                HudIntent::UnitBar(UnitBarIntent::Select(vec![40, 41])),
                HudIntent::TacMapMoveOrder((1.0, 2.0)),
            ],
            &mut s,
            &ctx,
        );
        assert_eq!(
            out.commands,
            [cmd(
                &[9, 8],
                move_order(pt(1.0, 2.0, 0.0), TravelMode::Slow),
                false
            )]
        );
        assert_eq!(out.selected, [PartId(9), PartId(8)]);
    }

    #[test]
    fn unit_bar_force_group_selection_uses_the_live_snapshot_members() {
        let mut s = state(&[8, 9]);
        let mut ctx = context();
        ctx.force_group_members[2] = vec![PartId(9), PartId(8)];
        let out = run_intents(
            &[HudIntent::UnitBar(UnitBarIntent::SelectForceGroup(2))],
            &mut s,
            &ctx,
        );
        assert!(out.unavailable.is_empty());
        assert!(out.commands.is_empty());
        assert_eq!(out.selected, [PartId(9), PartId(8)]);
        let out = run_intents(
            &[HudIntent::UnitBar(UnitBarIntent::Select(vec![1]))],
            &mut s,
            &ctx,
        );
        assert_eq!(out.unavailable[0].action, "select unit");
        assert_eq!(out.selected, [PartId(9), PartId(8)]);
    }

    #[test]
    fn energy_weapons_hotkey_tracks_held_state_and_conserve_ammo_is_a_cpp_noop() {
        let mut s = state(&[6]);
        let mut ctx = context();
        ctx.energy_weapons_held = true;
        run_intents(&[mission(MissionCommand::EnergyWeapons)], &mut s, &ctx);
        assert!(s.conserve_ammo);
        ctx.energy_weapons_held = false;
        run_intents(&[mission(MissionCommand::EnergyWeapons)], &mut s, &ctx);
        assert!(!s.conserve_ammo);
        let out = run_intents(&[mission(MissionCommand::ConserveAmmo)], &mut s, &ctx);
        assert!(out.commands.is_empty() && out.unavailable.is_empty());
    }

    #[test]
    fn gui_gaps_stay_explicitly_unavailable() {
        let cases = [(
            mission(MissionCommand::Unavailable("debugOrder")),
            "debugOrder",
        )];
        for (intent, action) in cases {
            let mut s = state(&[1]);
            let out = run_intents(&[intent], &mut s, &context());
            assert!(out.commands.is_empty(), "{action}");
            assert_eq!(out.unavailable[0].action, action);
        }
    }

    #[test]
    fn nav_marker_orders_move_selected_movers_to_the_marker_with_current_panel_modes() {
        let mut s = state(&[1, 2]);
        s.run = true;
        s.lay_mines = true;
        let mut ctx = context();
        ctx.next_nav_marker = Some(pt(40.0, 50.0, 3.0));
        let out = run_intents(&[mission(MissionCommand::GotoNextNavMarker)], &mut s, &ctx);
        assert!(out.unavailable.is_empty());
        assert!(
            matches!(out.commands.as_slice(), [PlayerCommand::OrderQueueExecute { movers, order }]
            if movers == &[PartId(1), PartId(2)]
                && order.code == OrderCode::MoveToPoint
                && order.move_params.way_path.points[0] == pt(40.0, 50.0, 3.0)
                && order.move_params.way_path.mode[0] == TravelMode::Fast
                && order.move_params.mine_laying)
        );
    }

    #[test]
    fn info_selection_and_visible_selection_have_production_effects() {
        let mut s = state(&[3]);
        let mut ctx = context();
        ctx.visible_movers = vec![PartId(2), PartId(3)];
        let selected = run_intents(&[mission(MissionCommand::SelectVisible)], &mut s, &ctx);
        assert!(selected.unavailable.is_empty());
        assert_eq!(selected.selected, [PartId(2), PartId(3)]);

        let mut ctx = context();
        ctx.target_part = Some(PartId(8));
        let begin = run_intents(&[mission(MissionCommand::InfoCommand)], &mut s, &ctx);
        assert!(begin.unavailable.is_empty() && s.info_selecting);
        ctx.command_clicked = true;
        ctx.viewport_click = Some(ViewportOrderClick::Object(WatchId(8)));
        let info = run_intents(&[], &mut s, &ctx);
        assert!(info.unavailable.is_empty());
        assert_eq!(info.show_info, Some(PartId(8)));
        assert!(!s.info_selecting);
    }

    #[test]
    fn handled_command_clicks_do_not_fall_through_to_a_viewport_move() {
        for command in [
            MissionCommand::PowerUp,
            MissionCommand::RemoveCommand,
            MissionCommand::SendAirstrike,
        ] {
            let mut s = state(&[5]);
            let mut ctx = context();
            click_terrain(&mut ctx, pt(4.0, 2.0, 0.0));
            let out = run_intents(&[mission(command)], &mut s, &ctx);
            assert!(
                out.commands.iter().all(|c| !matches!(
                    c,
                    PlayerCommand::Order { order, .. } if order.code == OrderCode::MoveToPoint
                )),
                "{command:?}"
            );
        }
    }

    // ---- the sim's own Unavailable ---------------------------------------------------------

    /// Documents the dependency without blocking the bridge: the bridge emits the C++ command, and
    /// the sim applies it to the home mover's pilot only.
    #[test]
    fn bridge_ranges_and_move_orders_update_only_selected_home_movers() {
        use mc2_sim::ai::AiUnavailable;
        use mc2_sim::mission::init::{load_world, LoadSpec};
        use mc2_sim::rng::RngMode;
        use mc2_sim::visibility::AllVisible;
        use mc2_sim::{FrameInput, FrameUnavailable, Sim};
        use std::rc::Rc;

        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED sim Unavailable test: MC2_DATA is unset");
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("mount archives");
        let outcome = load_world(
            &mut vfs,
            &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
        )
        .expect("load mc2_01");
        assert!(outcome.complete, "{:?}", outcome.missing_hooks);
        let mut sim = Sim::new(outcome);
        let mover = sim.world.objects.movers[0];
        let ally_part = sim
            .world
            .objects
            .get(sim.world.objects.movers[1])
            .expect("mover")
            .c
            .part_id;
        let part = sim.world.objects.get(mover).expect("mover").c.part_id;

        assert_ne!(part, ally_part);
        // The allied mover is selected too but is not the home commander's: it gets no order.
        let mut s = OrdersState {
            selected: vec![part, ally_part],
            ..OrdersState::default()
        };
        let mut ctx = context();
        ctx.home_commander_order = vec![part];
        for (frame, range, expected) in [
            (1, FireRange::Short, FIRERANGE_SHORT),
            (2, FireRange::Medium, FIRERANGE_MEDIUM),
            (3, FireRange::Long, FIRERANGE_LONG),
            (4, FireRange::Optimal, FIRERANGE_OPTIMAL),
            (5, FireRange::Current, FIRERANGE_CURRENT),
        ] {
            let result = run_intents(
                &[HudIntent::Control(ControlIntent::SetRange(range))],
                &mut s,
                &ctx,
            );
            assert!(result.unavailable.is_empty());
            sim.step(FrameInput::fixed(
                30,
                frame,
                Rc::new(AllVisible),
                result.commands,
            ))
            .expect("range command applies to a complete load");
            let current = match &sim.world.objects.get(mover).expect("home mover").kind {
                mc2_sim::object::Kind::Mech(m) => m.m.attack_range,
                mc2_sim::object::Kind::Vehicle(v) => v.m.attack_range,
                other => panic!("not a mover: {other:?}"),
            };
            assert_eq!(current, expected);
        }
        let range = run_intents(
            &[HudIntent::Control(ControlIntent::SetRange(FireRange::Long))],
            &mut s,
            &ctx,
        );
        sim.step(FrameInput::fixed(
            30,
            6,
            Rc::new(AllVisible),
            range.commands,
        ))
        .expect("long range applies");
        let actual_range = match &sim.world.objects.get(mover).expect("home mover").kind {
            mc2_sim::object::Kind::Mech(m) => m.m.attack_range,
            mc2_sim::object::Kind::Vehicle(v) => v.m.attack_range,
            other => panic!("not a mover: {other:?}"),
        };
        let ally_range = match &sim
            .world
            .objects
            .get(sim.world.objects.find_by_part_id(ally_part).expect("ally"))
            .expect("allied mover")
            .kind
        {
            mc2_sim::object::Kind::Mech(m) => m.m.attack_range,
            mc2_sim::object::Kind::Vehicle(v) => v.m.attack_range,
            other => panic!("not a mover: {other:?}"),
        };
        assert_eq!(actual_range, FIRERANGE_LONG);
        assert_eq!(
            ally_range, FIRERANGE_OPTIMAL,
            "only the home commander is changed"
        );
        ctx.world_point = Some(pt(10.0, 10.0, 0.0));
        let out = run_intents(&[HudIntent::TacMapMoveOrder((10.0, 10.0))], &mut s, &ctx);
        assert_eq!(out.commands.len(), 1);
        assert!(out.unavailable.is_empty());
        sim.step(FrameInput::fixed(30, 7, Rc::new(AllVisible), out.commands))
            .expect("the move order is accepted");
        let hold = run_intents(
            &[HudIntent::Control(ControlIntent::SetRange(
                FireRange::Current,
            ))],
            &mut s,
            &ctx,
        );
        let mut next_frame = 8;
        loop {
            sim.step(FrameInput::fixed(
                30,
                next_frame,
                Rc::new(AllVisible),
                Vec::new(),
            ))
            .expect("the move order is processed");
            next_frame += 1;
            let slot = match &sim.world.objects.get(mover).expect("home mover").kind {
                mc2_sim::object::Kind::Mech(m) => m.m.pilot_slot,
                mc2_sim::object::Kind::Vehicle(v) => v.m.pilot_slot,
                other => panic!("not a mover: {other:?}"),
            };
            if sim
                .world
                .pilots
                .get(slot)
                .is_some_and(|pilot| pilot.cur_order.code == OrderCode::MoveToPoint)
            {
                break;
            }
            assert!(next_frame < 40, "the move order did not become current");
        }
        sim.step(FrameInput::fixed(
            30,
            next_frame,
            Rc::new(AllVisible),
            hold.commands,
        ))
        .expect("hold position updates the current order");
        next_frame += 1;
        let slot = match &sim.world.objects.get(mover).expect("home mover").kind {
            mc2_sim::object::Kind::Mech(m) => m.m.pilot_slot,
            mc2_sim::object::Kind::Vehicle(v) => v.m.pilot_slot,
            other => panic!("not a mover: {other:?}"),
        };
        assert_eq!(
            sim.world
                .pilots
                .get(slot)
                .expect("pilot")
                .cur_order
                .attack_params
                .range,
            FIRERANGE_CURRENT,
            "hold position stamps the current tactical order with the selected range"
        );
        assert!(
            !sim.world
                .pilots
                .get(slot)
                .expect("pilot")
                .cur_order
                .attack_params
                .pursue
        );
        let current_range = match &sim.world.objects.get(mover).expect("home mover").kind {
            mc2_sim::object::Kind::Mech(m) => m.m.attack_range,
            mc2_sim::object::Kind::Vehicle(v) => v.m.attack_range,
            other => panic!("not a mover: {other:?}"),
        };
        assert_eq!(current_range, FIRERANGE_CURRENT);

        let ally_code_before = {
            let h = sim.world.objects.find_by_part_id(ally_part).expect("part");
            let slot = match &sim.world.objects.get(h).expect("obj").kind {
                mc2_sim::object::Kind::Mech(m) => m.m.pilot_slot,
                mc2_sim::object::Kind::Vehicle(v) => v.m.pilot_slot,
                other => panic!("not a mover: {other:?}"),
            };
            sim.world.pilots.get(slot).expect("pilot").cur_order.code
        };
        assert_ne!(ally_code_before, OrderCode::MoveToPoint);
        let frame = sim
            .step(FrameInput::fixed(
                30,
                next_frame,
                Rc::new(AllVisible),
                Vec::new(),
            ))
            .expect("a complete load steps");
        next_frame += 1;
        // The decision tree is ported, so the sim no longer reports it as unavailable for this order.
        assert!(
            !frame.unavailable.contains(&FrameUnavailable::Ai {
                part_id: part.0,
                what: AiUnavailable::DecisionTree,
            }),
            "{:?}",
            frame.unavailable
        );
        // The home mover's pilot took the move order; the allied pilot's orders are untouched.
        let after = |sim: &Sim, p: PartId| {
            let h = sim.world.objects.find_by_part_id(p).expect("part");
            let slot = match &sim.world.objects.get(h).expect("obj").kind {
                mc2_sim::object::Kind::Mech(m) => m.m.pilot_slot,
                mc2_sim::object::Kind::Vehicle(v) => v.m.pilot_slot,
                other => panic!("not a mover: {other:?}"),
            };
            sim.world.pilots.get(slot).expect("pilot").cur_order.code
        };
        assert_eq!(after(&sim, part), OrderCode::MoveToPoint);
        assert_eq!(after(&sim, ally_part), ally_code_before);

        // Exercise the acceptance slice through the same production bridge used by HudRuntime,
        // then carry the NAV command into the loaded mc2_01 simulation.
        let nav = pt(40.0, 50.0, 0.0);
        ctx.next_nav_marker = Some(nav);
        let home_slot = match &sim.world.objects.get(mover).expect("home mover").kind {
            mc2_sim::object::Kind::Mech(m) => m.m.pilot_slot,
            mc2_sim::object::Kind::Vehicle(v) => v.m.pilot_slot,
            other => panic!("not a mover: {other:?}"),
        };
        let before_nav_point = sim
            .world
            .pilots
            .get(home_slot)
            .expect("pilot")
            .cur_order
            .move_params
            .way_path
            .points[0];
        let nav_order = run_intents(&[mission(MissionCommand::GotoNextNavMarker)], &mut s, &ctx);
        assert!(nav_order.unavailable.is_empty());
        let nav_command_order = match nav_order.commands.as_slice() {
            [PlayerCommand::Order { movers, order, .. }]
            | [PlayerCommand::OrderQueueExecute { movers, order }]
                if movers == &[part] =>
            {
                order
            }
            commands => panic!("unexpected NAV command: {commands:?}"),
        };
        assert_eq!(nav_command_order.code, OrderCode::MoveToPoint);
        assert_eq!(nav_command_order.move_params.way_path.points[0], nav);
        sim.step(FrameInput::fixed(
            30,
            next_frame,
            Rc::new(AllVisible),
            nav_order.commands,
        ))
        .expect("the NAV order reaches the loaded sim");
        next_frame += 1;
        let current_nav_order = &sim.world.pilots.get(home_slot).expect("pilot").cur_order;
        assert_eq!(current_nav_order.code, OrderCode::MoveToPoint);
        assert_ne!(
            current_nav_order.move_params.way_path.points[0], before_nav_point,
            "the NAV click updates the mover's live order; the sim applies its formation goal"
        );

        ctx.command_clicked = false;
        ctx.target_part = Some(PartId(9001));
        ctx.viewport_click = Some(ViewportOrderClick::Object(WatchId(9001)));
        let info_begin = run_intents(&[mission(MissionCommand::InfoCommand)], &mut s, &ctx);
        assert!(info_begin.unavailable.is_empty() && s.info_selecting);
        ctx.command_clicked = true;
        let info = run_intents(&[], &mut s, &ctx);
        assert_eq!(info.show_info, Some(PartId(9001)));
        assert!(!s.info_selecting);
        ctx.command_clicked = false;
        ctx.viewport_click = None;
        ctx.target_part = None;
        ctx.visible_movers = vec![part];
        let visible = run_intents(&[mission(MissionCommand::SelectVisible)], &mut s, &ctx);
        assert_eq!(visible.selected, [part]);
        ctx.target = Some(WatchId(9002));
        ctx.command_clicked = true;
        ctx.aim = Some(AimFacts {
            ground_mech: true,
            any_selected_non_area_weapon: true,
            ..AimFacts::default()
        });
        let aimed = run_intents(&[mission(MissionCommand::AimLeg)], &mut s, &ctx);
        assert!(aimed.unavailable.is_empty());
        assert!(aimed.commands.iter().any(|command| matches!(
            command,
            PlayerCommand::Order { order, .. }
                if order.code == OrderCode::AttackObject
                    && order.attack_params.aim_location == BODY_LEFT_LEG
        )));

        let slice_actions = [
            "fire range",
            "hold position",
            "attack range",
            "aimed shot",
            "show unit info",
            "nav marker",
            "selection command",
        ];
        ctx.command_clicked = false;
        ctx.target = None;
        ctx.target_part = None;
        ctx.viewport_click = None;
        ctx.aim = None;
        ctx.next_nav_marker = Some(pt(60.0, 70.0, 0.0));
        ctx.visible_movers = vec![part];
        let production_input_start = next_frame;
        for frame_no in next_frame..=2_000 {
            // Exercise every residue input from inside the 2,000-frame mc2_01 run.
            // These go through the production bridge and their emitted sim commands are
            // applied on this frame, rather than being checked only in setup beforehand.
            let input_index = frame_no - production_input_start;
            let output = match input_index {
                0 => run_intents(
                    &[HudIntent::Control(ControlIntent::SetRange(
                        FireRange::Short,
                    ))],
                    &mut s,
                    &ctx,
                ),
                1 => run_intents(
                    &[HudIntent::Control(ControlIntent::SetRange(
                        FireRange::Current,
                    ))],
                    &mut s,
                    &ctx,
                ),
                2 => run_intents(&[mission(MissionCommand::GotoNextNavMarker)], &mut s, &ctx),
                3 => {
                    ctx.target_part = Some(PartId(9001));
                    ctx.viewport_click = Some(ViewportOrderClick::Object(WatchId(9001)));
                    run_intents(&[mission(MissionCommand::InfoCommand)], &mut s, &ctx)
                }
                4 => {
                    ctx.command_clicked = true;
                    run_intents(&[], &mut s, &ctx)
                }
                5 => {
                    ctx.command_clicked = true;
                    ctx.target = Some(WatchId(9002));
                    ctx.aim = Some(AimFacts {
                        ground_mech: true,
                        any_selected_non_area_weapon: true,
                        ..AimFacts::default()
                    });
                    run_intents(&[mission(MissionCommand::AimLeg)], &mut s, &ctx)
                }
                6 => {
                    ctx.target = None;
                    ctx.target_part = None;
                    ctx.viewport_click = None;
                    ctx.aim = None;
                    run_intents(&[mission(MissionCommand::SelectVisible)], &mut s, &ctx)
                }
                _ => run_intents(&[], &mut s, &ctx),
            };
            assert!(
                output
                    .unavailable
                    .iter()
                    .all(|u| !slice_actions.contains(&u.action)),
                "frame {frame_no}: {:?}",
                output.unavailable
            );
            sim.step(FrameInput::fixed(
                30,
                frame_no,
                Rc::new(AllVisible),
                output.commands.clone(),
            ))
            .expect("mc2_01 remains step-able through the 2,000-frame smoke");
            match input_index {
                0 => assert!(
                    !output.commands.is_empty(),
                    "the in-run attack-range input emitted a sim command"
                ),
                1 => assert_eq!(
                    sim.world
                        .pilots
                        .get(home_slot)
                        .expect("pilot")
                        .cur_order
                        .attack_params
                        .range,
                    FIRERANGE_CURRENT,
                    "the in-run hold-position range is carried by the current order"
                ),
                2 => assert!(output.commands.iter().any(|command| matches!(
                    command,
                    PlayerCommand::Order { order, .. } | PlayerCommand::OrderQueueExecute { order, .. }
                        if order.code == OrderCode::MoveToPoint
                )), "the in-run NAV marker emitted its production move order"),
                3 => assert!(s.info_selecting, "the in-run Info order entered pick mode"),
                4 => assert_eq!(output.show_info, Some(PartId(9001)), "the in-run pick reached Info"),
                5 => assert!(output.commands.iter().any(|command| matches!(
                    command,
                    PlayerCommand::Order { order, .. } | PlayerCommand::OrderQueueExecute { order, .. }
                        if order.code == OrderCode::AttackObject
                            && order.attack_params.aim_location == BODY_LEFT_LEG
                )), "the in-run aimed shot emitted its production attack order"),
                6 => assert_eq!(output.selected, [part], "the in-run selection command selected the mover"),
                _ => {}
            }
        }
    }
}

/// Acceptance through the real HUD: every selection, mode and order below starts as a pointer
/// event or hotkey command fed to `MissionHud::update` (real FIT layouts from `MC2_DATA`), whose
/// intents go to the bridge. The only world facts supplied beside them are the picker's declared
/// upstream inputs: the terrain height under the TacMap point the HUD itself reported, and the
/// terrain click of the primary viewport (which is the app picker's output, not a HUD intent).
#[cfg(test)]
mod hud_input_tests {
    use super::*;
    use mc2_platform::input::KeyStatus;
    use mc2_platform::input::{InputEvent, MouseButton};
    use mc2_platform::keys::KEY_J;
    use mc2_ui::control::{
        ControlId, ControlPanel, ControlPanelInput, UnitCapabilities, VehicleAction,
        VehicleAvailability,
    };
    use mc2_ui::ctx::{Loader, UiAssets, UiOutput, UpdateCtx};
    use mc2_ui::hud_info::info::InfoGeometry;
    use mc2_ui::input::UiInput;
    use mc2_ui::mission_hud::{
        HudInput, HudLayout, HudRect, MissionChatGeometry, MissionHud, MissionHudInput,
    };
    use mc2_ui::mission_keys::{ControlStyle, DispatchContext, HotKeys, KeyFrame};
    use mc2_ui::pause_window::PauseWindow;
    use mc2_ui::tacmap::{TacMap, TerrainBounds};
    use mc2_ui::unitbar::layout::Layout;
    use mc2_ui::unitbar::{
        Armor, Art, MouseProfile, Pilot, Pointer, UnitBar, UnitBarInput, UnitBarUnit,
    };

    const T: f32 = 12.0;

    struct Rig {
        hud: MissionHud,
        control: ControlPanel,
        tac_map: TacMap,
        pause: PauseWindow,
        bar: UnitBar,
        input: UiInput,
        ui_output: UiOutput,
        assets: UiAssets,
        selected_units: [UnitCapabilities; 1],
        bar_slot: HudRect,
        map_mouse: (i32, i32),
        /// Seconds the next frame covers; right clicks need more than the 0.2 s double-click window.
        dt: f32,
        /// Vehicle-tab click delivered to the next frame (the app's hit test result).
        vehicle_click: Option<VehicleAction>,
        resource_points: i32,
    }

    fn rig(root: std::ffi::OsString) -> Rig {
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets {
            screen: (1024, 768),
            ..UiAssets::default()
        };
        let (control, bar_layout, bar, info, chat, pause) = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            let control = ControlPanel::load(&mut loader, 1024, 768).expect("control FIT");
            let pause = PauseWindow::load(&mut loader, 1024, 768).expect("pause FIT");
            let mut fit = loader.art_fit("buttonlayout1024").expect("unit-bar FIT");
            let bar_layout = Layout::from_fit(&mut fit).expect("unit-bar layout");
            let art = Art::load(&mut loader, &mut fit).expect("unit-bar art");
            let bar = UnitBar::new(bar_layout, art);
            let mut fit = loader.art_fit("buttonlayout1024").expect("info FIT");
            let info = InfoGeometry::from_fit(&mut fit, 1024).expect("info geometry");
            let mut fit = loader.art_fit("buttonlayout1024").expect("chat FIT");
            let chat = MissionChatGeometry::from_fit(&mut fit).expect("chat geometry");
            (control, bar_layout, bar, info, chat, pause)
        };
        let pause_rect = HudRect::from_gui(pause.panel_rect());
        let layout = HudLayout::from_parts(
            (1024, 768),
            &control,
            &bar_layout,
            &info,
            chat,
            Some(pause_rect),
        );
        let bar_slot = layout.unit_bar[0];
        let map_rect = layout.tac_map_region.expect("real TacMap FIT rectangle");
        let mut tac_map = TacMap::new(TerrainBounds::from_vertices(100));
        tac_map.left_right_profile = true;
        tac_map.set_pos(mc2_ui::draw::GuiRect::new(
            map_rect.left,
            map_rect.top,
            map_rect.right,
            map_rect.bottom,
        ));
        Rig {
            hud: MissionHud::new(layout),
            control,
            tac_map,
            pause,
            bar,
            input: UiInput::new(1024, 768),
            ui_output: UiOutput::default(),
            assets,
            selected_units: [UnitCapabilities::default()],
            bar_slot,
            map_mouse: (
                (map_rect.left + map_rect.right) / 2,
                (map_rect.top + map_rect.bottom) / 2,
            ),
            dt: 0.016,
            vehicle_click: None,
            resource_points: 0,
        }
    }

    impl Rig {
        /// One `MissionHud::update` frame.
        fn frame(
            &mut self,
            commands: Vec<MissionCommand>,
            mouse: (i32, i32),
            events: &[InputEvent],
            unit_bar: Option<&UnitBarInput>,
        ) -> Vec<HudIntent> {
            let mut textures = std::mem::replace(
                &mut self.assets.textures,
                mc2_ui::textures::TextureStore::new(),
            );
            let output = {
                let mut update_ctx = UpdateCtx {
                    input: &mut self.input,
                    assets: &self.assets,
                    dt: self.dt,
                    out: &mut self.ui_output,
                };
                self.hud.update(MissionHudInput {
                    state: HudInput {
                        mission_commands: commands,
                        unit_bar_icon_count: usize::from(unit_bar.is_some()),
                        ..HudInput::default()
                    },
                    control: &mut self.control,
                    control_input: ControlPanelInput {
                        selected_units: &self.selected_units,
                        mouse,
                        paused: false,
                        vtol_in_flight: false,
                        resource_points: self.resource_points,
                        vehicle_availability: VehicleAvailability {
                            can_have_airstrike: true,
                            can_have_sensor_probe: true,
                            can_have_guard_tower: true,
                            can_have_repair_truck: true,
                            can_have_scout: true,
                            can_have_minelayer: true,
                            can_have_recovery_team: true,
                            unit_capacity_available: true,
                            pilots_remaining: true,
                            player_destroyed: false,
                        },
                        click: None,
                        vehicle_click: self.vehicle_click.take(),
                    },
                    tac_map: &mut self.tac_map,
                    update_ctx: &mut update_ctx,
                    pause_window: &mut self.pause,
                    unit_bar: unit_bar.map(|input| (&mut self.bar, input, &mut textures)),
                    events,
                    dt: self.dt,
                })
            };
            self.assets.textures = textures;
            output.intents
        }

        fn click_events(position: (i32, i32), button: MouseButton) -> [InputEvent; 3] {
            [
                InputEvent::CursorMoved {
                    x: f64::from(position.0),
                    y: f64::from(position.1),
                },
                InputEvent::Button {
                    button,
                    pressed: true,
                },
                InputEvent::Button {
                    button,
                    pressed: false,
                },
            ]
        }

        fn button_center(&self, id: ControlId) -> (i32, i32) {
            let button = self
                .control
                .buttons
                .iter()
                .find(|button| button.id == Some(id))
                .expect("real control button");
            (
                (button.rect.left + button.rect.right) / 2,
                (button.rect.top + button.rect.bottom) / 2,
            )
        }

        /// A real right click on the TacMap (pointer motion on an earlier frame, as real input has),
        /// returning the frame's intents.
        fn right_click_tac_map(&mut self) -> Vec<HudIntent> {
            let map = self.map_mouse;
            let right = [
                InputEvent::CursorMoved {
                    x: f64::from(map.0),
                    y: f64::from(map.1),
                },
                InputEvent::Button {
                    button: MouseButton::Right,
                    pressed: true,
                },
            ];
            self.dt = 0.5;
            self.frame(Vec::new(), map, &right[..1], None);
            self.dt = 0.016;
            let intents = self.frame(Vec::new(), map, &right[1..], None);
            self.frame(
                Vec::new(),
                map,
                &[InputEvent::Button {
                    button: MouseButton::Right,
                    pressed: false,
                }],
                None,
            );
            intents
        }

        fn click_button(&mut self, id: ControlId) -> Vec<HudIntent> {
            let center = self.button_center(id);
            self.frame(
                Vec::new(),
                center,
                &Self::click_events(center, MouseButton::Left),
                None,
            )
        }
    }

    fn unit_bar_input(rig: &Rig) -> UnitBarInput {
        UnitBarInput {
            units: vec![UnitBarUnit {
                id: 77,
                icon_picture: 0,
                tonnage: 50,
                pilot: Pilot {
                    name: "SYNTHETIC".into(),
                    photo: 0,
                    message_playing: false,
                },
                armor: Armor::Mech(Default::default()),
                bar_status: 1.0,
                selected: false,
                groups: 1,
                shutdown: false,
                destroyed: false,
                recover_pending: false,
                power_used_up: false,
                can_jump: true,
            }],
            pointer: Pointer {
                x: (rig.bar_slot.left + rig.bar_slot.right) / 2,
                y: (rig.bar_slot.top + rig.bar_slot.bottom) / 2,
                left_click: true,
                ..Pointer::default()
            },
            profile: MouseProfile::LeftRight,
            ..UnitBarInput::default()
        }
    }

    /// Expected player order built straight from the constructor with literal C++ facts
    /// (`TacticalOrder::init(OrderOrigin::Commander, ..)` from `handleOrders`), not via the
    /// bridge's own `player_order`; the field assertions pin what the constructor must give.
    fn cpp_order(code: OrderCode, unit_order: bool, time: f32) -> TacticalOrder {
        let order = TacticalOrder::new(OrderOrigin::Player, code, unit_order, time);
        assert_eq!(order.origin, OrderOrigin::Player);
        assert_eq!(order.code, code);
        assert_eq!(order.unit_order, unit_order);
        assert_eq!(order.time, time);
        order
    }

    fn move_command(point: Vec3, mode: TravelMode) -> PlayerCommand {
        let mut order = cpp_order(OrderCode::MoveToPoint, false, T);
        order.move_params.way_path.points[0] = point;
        order.move_params.way_path.mode[0] = mode;
        order.move_params.wait = false;
        PlayerCommand::Order {
            movers: vec![PartId(5)],
            order,
            queue: false,
        }
    }

    #[test]
    fn real_hud_inputs_produce_the_exact_cpp_command_list() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED HUD-to-orders acceptance: MC2_DATA is unset");
            return;
        };
        let mut rig = rig(root);
        let mut state = OrdersState::default();
        let mut ctx = BridgeContext {
            scenario_time: T,
            unit_bar_parts: BTreeMap::from([(77, PartId(5))]),
            home_commander_order: vec![PartId(5)],
            jump_capable: BTreeMap::from([(PartId(5), true)]),
            ..BridgeContext::default()
        };
        let map = rig.map_mouse;

        // 1. A real left click on the unit-bar slot selects the unit.
        let bar = unit_bar_input(&rig);
        let intents = rig.frame(Vec::new(), map, &[], Some(&bar));
        assert!(intents.contains(&HudIntent::UnitBar(UnitBarIntent::Select(vec![77]))));
        let out = intents_to_commands(&intents, &mut state, &ctx);
        assert_eq!(out.selected, [PartId(5)]);
        assert!(out.commands.is_empty());

        // 2. Real Run-button click, then a real TacMap right click: a fast move to the point the
        //    TacMap itself reported (elevation 3.0 is the picker's declared terrain height).
        let intents = rig.click_button(ControlId::Run);
        assert_eq!(intents, [HudIntent::Control(ControlIntent::SetRun(true))]);
        intents_to_commands(&intents, &mut state, &ctx);
        let right = [
            InputEvent::CursorMoved {
                x: f64::from(map.0),
                y: f64::from(map.1),
            },
            InputEvent::Button {
                button: MouseButton::Right,
                pressed: true,
            },
        ];
        // The pointer arrives on an earlier frame than the press, as real mouse motion does;
        // `UiInput` counts the right button's idle time over the frames before the press, so
        // that frame must span more than the 0.2 s double-click window.
        rig.dt = 0.5;
        rig.frame(Vec::new(), map, &right[..1], None);
        rig.dt = 0.016;
        let intents = rig.frame(Vec::new(), map, &right[1..], None);
        let [HudIntent::TacMapMoveOrder((x, y))] = intents[..] else {
            panic!("expected one TacMap move order, got {intents:?}");
        };
        let point = Vec3 { x, y, z: 3.0 };
        ctx.world_point = Some(point);
        let out = intents_to_commands(&intents, &mut state, &ctx);
        assert_eq!(out.commands, [move_command(point, TravelMode::Fast)]);
        assert!(out.unavailable.is_empty());
        assert!(!state.run, "doMove ends with setDefaultSpeed");
        rig.frame(
            Vec::new(),
            map,
            &[InputEvent::Button {
                button: MouseButton::Right,
                pressed: false,
            }],
            None,
        );
        ctx.world_point = None;

        // 3. Space (ChangeSpeed) -> Guard -> Space release leaves Guard armed; the next terrain
        //    click of the primary viewport is a Guard order.
        for (command, expected_bits) in [
            (MissionCommand::ChangeSpeed, RUN),
            (MissionCommand::Guard, RUN | GUARD),
            (MissionCommand::StopChangeSpeed, GUARD),
        ] {
            let intents = rig.frame(vec![command], map, &[], None);
            assert_eq!(intents, [HudIntent::Mission(command)]);
            let out = intents_to_commands(&intents, &mut state, &ctx);
            assert!(out.commands.is_empty() && out.unavailable.is_empty());
            assert_eq!(state.order_bits(), expected_bits, "{command:?}");
        }
        let landing = Vec3 {
            x: 40.0,
            y: 50.0,
            z: 2.0,
        };
        ctx.command_clicked = true;
        ctx.viewport_click = Some(ViewportOrderClick::Terrain(landing));
        let out = intents_to_commands(&[], &mut state, &ctx);
        let mut guard = cpp_order(OrderCode::Guard, false, T);
        guard.move_params.way_path.points[0] = landing;
        guard.move_params.way_path.mode[0] = TravelMode::Fast;
        assert_eq!(
            out.commands,
            [PlayerCommand::Order {
                movers: vec![PartId(5)],
                order: guard,
                queue: false
            }]
        );
        assert_eq!(state.order_bits(), 0);
        ctx.command_clicked = false;
        ctx.viewport_click = None;

        // 4. A real J key frame passes through mission hotkey dispatch and MissionHud; a real
        //    primary-viewport click plus the picker's landing fact then produces a jump order.
        let hotkeys = HotKeys::new(ControlStyle::Mc);
        let j_frame = KeyFrame::default().with(KEY_J, KeyStatus::Pressed);
        let jump_dispatch = hotkeys.dispatch(&j_frame, DispatchContext::default());
        assert_eq!(
            jump_dispatch
                .commands
                .iter()
                .map(|fired| fired.command)
                .collect::<Vec<_>>(),
            [MissionCommand::Jump]
        );
        let commands: Vec<MissionCommand> = jump_dispatch
            .commands
            .into_iter()
            .map(|fired| fired.command)
            .collect();
        ctx.jump_capable.insert(PartId(5), false);
        let intents = rig.frame(commands.clone(), map, &[], None);
        intents_to_commands(&intents, &mut state, &ctx);
        assert!(!state.jump);
        ctx.jump_capable.insert(PartId(5), true);
        let intents = rig.frame(commands, map, &[], None);
        intents_to_commands(&intents, &mut state, &ctx);
        assert!(state.jump);
        ctx.command_clicked = true;
        ctx.jump_landing_ok = Some(true);
        ctx.viewport_click = Some(ViewportOrderClick::Terrain(landing));
        let click_at = (500, 400);
        let click_events = Rig::click_events(click_at, MouseButton::Left);
        let click_intents = rig.frame(Vec::new(), click_at, &click_events, None);
        let out = intents_to_commands(&click_intents, &mut state, &ctx);
        let mut jump = cpp_order(OrderCode::JumpToPoint, false, T);
        jump.move_params.wait = false;
        jump.move_params.way_path.mode[0] = TravelMode::Jump;
        jump.move_params.way_path.points[0] = landing;
        assert_eq!(
            out.commands,
            [PlayerCommand::Order {
                movers: vec![PartId(5)],
                order: jump,
                queue: false
            }]
        );
        ctx.command_clicked = false;
        ctx.viewport_click = None;

        // 5. A real Stop-button click is one direct Stop order for the selection.
        let intents = rig.click_button(ControlId::Stop);
        assert_eq!(intents, [HudIntent::Control(ControlIntent::Stop)]);
        let out = intents_to_commands(&intents, &mut state, &ctx);
        assert_eq!(
            out.commands,
            [PlayerCommand::Order {
                movers: vec![PartId(5)],
                order: cpp_order(OrderCode::Stop, false, T),
                queue: false
            }]
        );
    }

    /// Review-5 finding 1: Run button -> move -> Run button -> move, through real HUD clicks. The
    /// move ends with `setDefaultSpeed`, which resets the shared panel mode, so the second Run
    /// click turns Run on again and the second move is fast.
    #[test]
    fn run_button_move_run_button_move_is_fast_both_times() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED HUD run/move acceptance: MC2_DATA is unset");
            return;
        };
        let mut rig = rig(root);
        let mut state = OrdersState {
            selected: vec![PartId(5)],
            ..OrdersState::default()
        };
        let mut ctx = BridgeContext {
            scenario_time: T,
            home_commander_order: vec![PartId(5)],
            ..BridgeContext::default()
        };
        // Settle the button states for the first frame.
        rig.frame(Vec::new(), rig.map_mouse, &[], None);
        for round in 0..2 {
            let intents = rig.click_button(ControlId::Run);
            assert_eq!(
                intents,
                [HudIntent::Control(ControlIntent::SetRun(true))],
                "round {round}"
            );
            let out = intents_to_commands(&intents, &mut state, &ctx);
            rig.control.set_order_bits(out.panel_order_bits);
            assert!(rig.control.run && state.run);

            let intents = rig.right_click_tac_map();
            let [HudIntent::TacMapMoveOrder((x, y))] = intents[..] else {
                panic!("expected one TacMap move order, got {intents:?}");
            };
            let point = Vec3 { x, y, z: 3.0 };
            ctx.world_point = Some(point);
            let out = intents_to_commands(&intents, &mut state, &ctx);
            assert_eq!(
                out.commands,
                [PlayerCommand::Order {
                    movers: vec![PartId(5)],
                    order: {
                        let mut o = cpp_order(OrderCode::MoveToPoint, false, T);
                        o.move_params.way_path.points[0] = point;
                        o.move_params.way_path.mode[0] = TravelMode::Fast;
                        o.move_params.wait = false;
                        o
                    },
                    queue: false
                }],
                "round {round}"
            );
            rig.control.set_order_bits(out.panel_order_bits);
            assert!(!rig.control.run && !state.run, "button released after move");
            ctx.world_point = None;
        }
    }

    /// Real vehicle-tab Airstrike and SensorProbe presses: the press spends the cost, the placement click calls the strike, and
    /// un-pressing the button afterwards leaves the cost spent (`addAirstrike` -> `unPressAllVehicleButtons`).
    #[test]
    fn real_strike_button_press_arms_and_a_click_calls_the_strike() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED HUD strike-button acceptance: MC2_DATA is unset");
            return;
        };
        for (kind, strike) in [(VehicleKind::Airstrike, 1), (VehicleKind::SensorProbe, 2)] {
            let mut rig = rig(root.clone());
            let mut state = OrdersState {
                selected: vec![PartId(5)],
                ..OrdersState::default()
            };
            let ctx = BridgeContext {
                scenario_time: T,
                home_commander_order: vec![PartId(5)],
                ..BridgeContext::default()
            };
            rig.resource_points = 100_000;
            rig.frame(Vec::new(), rig.map_mouse, &[], None);
            rig.click_button(ControlId::VehicleTab);
            rig.frame(Vec::new(), rig.map_mouse, &[], None);
            rig.vehicle_click = Some(VehicleAction::Build(kind));
            let intents = rig.frame(Vec::new(), rig.map_mouse, &[], None);
            assert!(intents.contains(&HudIntent::Control(ControlIntent::BuildVehicle(kind))));
            assert_eq!(rig.control.selected_vehicle, Some(kind));
            let cost = mc2_ui::control::vehicle_cost(kind);
            assert_eq!(rig.control.resource_points, 100_000 - cost);
            let out = intents_to_commands(&intents, &mut state, &ctx);
            assert_eq!(out.commands, [PlayerCommand::AddResourcePoints(-cost)]);
            assert!(out.unavailable.is_empty() && !out.strike_called, "{kind:?}");

            let point = Vec3 {
                x: 7.0,
                y: 8.0,
                z: 1.0,
            };
            let mut click = BridgeContext {
                strike: Some(strike),
                world_point: Some(point),
                ..ctx.clone()
            };
            click.command_clicked = true;
            click.viewport_click = Some(ViewportOrderClick::Terrain(point));
            let out = intents_to_commands(&[], &mut state, &click);
            assert_eq!(
                out.commands,
                [PlayerCommand::CallStrike { strike, at: point }]
            );
            assert!(out.strike_called);
            rig.control.unpress_vehicle_buttons();
            assert_eq!(rig.control.selected_vehicle, None);
            assert_eq!(rig.control.resource_points, 100_000 - cost, "no refund");
            // Same frame: the rendered button and its alternatives are no longer pressed/greyed.
            for b in rig.control.vehicles.iter().filter(|b| b.kind.is_some()) {
                assert_ne!(b.state, mc2_ui::control::ButtonVisualState::Pressed);
            }
        }
    }

    /// The support-vehicle buttons of the production control panel (real FIT layout, real `ControlPanel::update`): the press spends
    /// the C++ cost, a click calls the VTOL in at the clicked point (the clicked mech for the recovery craft), Stop returns the
    /// cost, and the end of the flight releases the button without a refund (controlgui.cpp:1960-2080, missiongui.cpp:1324).
    #[test]
    fn real_support_vehicle_buttons_buy_place_cancel_and_release() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED support vehicle acceptance: MC2_DATA is unset");
            return;
        };
        // (kind, `vehicleCosts[]`, `vehicleIDs[]`) from controlgui.cpp:85-120
        let vehicles = [
            (VehicleKind::GuardTower, 8000, 415),
            (VehicleKind::RepairTruck, 6000, 182),
            (VehicleKind::Scout, 2000, 393),
            (VehicleKind::Minelayer, 4000, 120),
            (VehicleKind::RecoveryTeam, 10000, 147),
        ];
        for (kind, cost, id) in vehicles {
            let mut rig = rig(root.clone());
            let mut state = OrdersState {
                selected: vec![PartId(5)],
                ..OrdersState::default()
            };
            let base = BridgeContext {
                scenario_time: T,
                home_commander_order: vec![PartId(5)],
                ..BridgeContext::default()
            };
            rig.resource_points = 100_000;
            rig.frame(Vec::new(), rig.map_mouse, &[], None);
            rig.click_button(ControlId::VehicleTab);
            rig.frame(Vec::new(), rig.map_mouse, &[], None);

            // buy: the press spends the cost
            rig.vehicle_click = Some(VehicleAction::Build(kind));
            let intents = rig.frame(Vec::new(), rig.map_mouse, &[], None);
            assert!(intents.contains(&HudIntent::Control(ControlIntent::BuildVehicle(kind))));
            assert_eq!(rig.control.selected_vehicle, Some(kind));
            let out = intents_to_commands(&intents, &mut state, &base);
            assert_eq!(
                out.commands,
                [PlayerCommand::AddResourcePoints(-cost)],
                "{kind:?}"
            );
            assert!(out.unavailable.is_empty() && !out.strike_called, "{kind:?}");

            // place: the next completed click calls the VTOL in
            let point = Vec3 {
                x: 7.0,
                y: 8.0,
                z: 1.0,
            };
            let mut ctx = BridgeContext {
                adding_vehicle: rig.control.selected_vehicle,
                world_point: Some(point),
                command_clicked: true,
                viewport_click: Some(if kind == VehicleKind::RecoveryTeam {
                    ViewportOrderClick::Object(WatchId(9))
                } else {
                    ViewportOrderClick::Terrain(point)
                }),
                ..base.clone()
            };
            if kind == VehicleKind::RecoveryTeam {
                ctx.target = Some(WatchId(9));
            }
            let out = intents_to_commands(&[], &mut state, &ctx);
            assert_eq!(
                out.commands,
                [PlayerCommand::BeginVtol {
                    commander: 0,
                    vehicle_id: id,
                    at: point,
                    target: (kind == VehicleKind::RecoveryTeam).then_some(WatchId(9)),
                }],
                "{kind:?}"
            );
            assert!(out.unavailable.is_empty(), "{kind:?}");

            // while the VTOL flies the armed button is not offered and the click is an ordinary order
            let flying = BridgeContext {
                adding_vehicle: None,
                ..ctx.clone()
            };
            let out = intents_to_commands(&[], &mut state, &flying);
            assert!(
                !out.commands
                    .iter()
                    .any(|c| matches!(c, PlayerCommand::BeginVtol { .. })),
                "{kind:?}"
            );

            // the flight ends: the button is released and nothing is refunded
            let before = rig.control.resource_points;
            rig.control.unpress_vehicle_buttons();
            assert_eq!(rig.control.selected_vehicle, None);
            assert_eq!(rig.control.resource_points, before);
            assert_eq!(before, 100_000 - cost);

            // cancel: Stop on an armed button returns the cost
            rig.vehicle_click = Some(VehicleAction::Build(kind));
            rig.frame(Vec::new(), rig.map_mouse, &[], None);
            rig.vehicle_click = Some(VehicleAction::Stop);
            let intents = rig.frame(Vec::new(), rig.map_mouse, &[], None);
            assert!(
                intents.contains(&HudIntent::Control(ControlIntent::CancelVehicle {
                    kind,
                    refund: cost
                })),
                "{kind:?}: {intents:?}"
            );
            let out = intents_to_commands(&intents, &mut state, &base);
            assert_eq!(
                out.commands,
                [PlayerCommand::AddResourcePoints(cost)],
                "{kind:?}"
            );
            assert_eq!(rig.control.selected_vehicle, None);
        }
    }

    /// The support buttons of the production control panel (real FIT layout, real `ControlPanel::update`) issue their
    /// intents, and the bridge turns them into orders without any `Unavailable` (missiongui.cpp:1340-1450, controlgui.cpp:1076).
    #[test]
    fn real_support_buttons_issue_their_orders_with_nothing_unavailable() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED support button acceptance: MC2_DATA is unset");
            return;
        };
        let specialties: [(ControlId, UnitCapabilities); 4] = [
            (
                ControlId::LayMines,
                UnitCapabilities {
                    can_lay_mines: true,
                    ..UnitCapabilities::default()
                },
            ),
            (
                ControlId::Repair,
                UnitCapabilities {
                    can_repair: true,
                    ..UnitCapabilities::default()
                },
            ),
            (
                ControlId::Salvage,
                UnitCapabilities {
                    can_salvage: true,
                    ..UnitCapabilities::default()
                },
            ),
            (
                ControlId::GuardTower,
                UnitCapabilities {
                    is_guard_tower: true,
                    ..UnitCapabilities::default()
                },
            ),
        ];
        for (id, unit) in specialties {
            let mut rig = rig(root.clone());
            rig.selected_units = [unit];
            let mut state = OrdersState {
                selected: vec![PartId(5)],
                ..OrdersState::default()
            };
            let ctx = BridgeContext {
                scenario_time: T,
                home_commander_order: vec![PartId(5)],
                unit_caps: BTreeMap::from([(PartId(5), unit)]),
                world_point: Some(Vec3 {
                    x: 7.0,
                    y: 8.0,
                    z: 1.0,
                }),
                ..BridgeContext::default()
            };
            // the panel shows the button for this selection, and only this one
            rig.frame(Vec::new(), rig.map_mouse, &[], None);
            for other in [
                ControlId::LayMines,
                ControlId::Repair,
                ControlId::Salvage,
                ControlId::GuardTower,
            ] {
                let shown = rig
                    .control
                    .buttons
                    .iter()
                    .find(|b| b.id == Some(other))
                    .is_some_and(|b| b.state != mc2_ui::control::ButtonVisualState::Hidden);
                assert_eq!(shown, other == id, "{id:?}: {other:?}");
            }
            let intents = rig.click_button(id);
            let expected = match id {
                ControlId::LayMines => ControlIntent::ToggleMines,
                ControlId::Repair => ControlIntent::ToggleRepair,
                ControlId::Salvage => ControlIntent::ToggleSalvage,
                _ => ControlIntent::SetGuardTower(true),
            };
            assert_eq!(intents, [HudIntent::Control(expected)], "{id:?}");
            let out = intents_to_commands(&intents, &mut state, &ctx);
            assert!(out.unavailable.is_empty(), "{id:?}: {:?}", out.unavailable);
            match id {
                ControlId::GuardTower => assert_eq!(
                    out.commands,
                    [PlayerCommand::SetSuppressionFire {
                        movers: vec![PartId(5)],
                        on: true
                    }]
                ),
                _ => assert!(out.commands.is_empty(), "{id:?}"),
            }
            if id == ControlId::LayMines {
                // the toggle is on: the next terrain click queues a mine laying move and starts the queue
                assert!(state.lay_mines);
                let mut click = ctx.clone();
                click.command_clicked = true;
                click.viewport_click = Some(ViewportOrderClick::Terrain(Vec3 {
                    x: 7.0,
                    y: 8.0,
                    z: 1.0,
                }));
                let out = intents_to_commands(&[], &mut state, &click);
                assert!(out.unavailable.is_empty());
                assert!(matches!(
                    out.commands.as_slice(),
                    [PlayerCommand::OrderQueueExecute { order, .. }]
                        if order.code == OrderCode::MoveToPoint && order.move_params.mine_laying
                ));
            }
        }
    }
}
