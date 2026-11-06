// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE D - AI, orders, forces and perception.
//!
//! Port of `code/warrior.{h,cpp}` (`MechWarrior`: pilot state, `mainDecisionTree`, brain scheduling, alarms, order
//! arbitration, `calcMovePath` glue), `code/tacordr.{h,cpp}` (`TacticalOrder::execute/status/equals`, lossy pack/unpack used by
//! the order queue), `code/group.{h,cpp}` (`MoverGroup::calcMoveGoals`, status counts), `code/team.{h,cpp}`,
//! `code/comndr.{h,cpp}`, `code/contact.{h,cpp}` (sensors, contacts, `SortList` ordering) and the line-of-sight
//! helpers `Team::lineOfSight` / `GameObject::lineOfSight` consumers. NOT ported: `goal.cpp` (never built), `radio.cpp`
//! (sound only), multiplayer, and everything under `#ifdef USE_GROUPS/USE_TEAMS/USE_ELEMENTALS/USE_JUMPING` (none defined).
//!
//! Frame position: `Mission::update` -> `missionInterface->update()` (player orders: [`issue_player_command`]) ->
//! `PathManager->update()` (lane B) -> `ObjectManager->update()` (each mover calls [`pilot_update`] =
//! `MechWarrior::mainDecisionTree` from `updateAIControl`) -> `SensorManager->update()` ([`sensors::update`], one team per
//! frame, round robin).

pub mod brain;
pub mod calcpath;
pub mod calls;
pub mod capture;
pub mod decide;
pub mod goal;
pub mod moveorders;
pub mod mover_ops;
pub mod natives;
pub mod orders;
pub mod pilot;
pub mod sensors;
pub mod support_orders;
pub mod tacorder;
pub mod targeting;
pub mod team;
pub mod warrior;

#[cfg(test)]
pub(crate) mod tests;

pub use pilot::{Alarm, Pilot, PilotPool};
pub use team::{Commander, Relation, Teams};

use crate::command::{PlayerCommand, TacticalOrder};
use crate::ids::{ObjectHandle, PartId, WatchId};

const MIN_TERRAIN_PART_ID: i32 = 4096;

/// Pilot AI behaviour that is not ported. Reaching one is reported once per pilot through
/// [`SimEvent::AiUnavailable`](crate::SimEvent::AiUnavailable) and in every frame through
/// [`FrameOutput::unavailable`](crate::FrameOutput::unavailable); the state the pilot has afterwards is not C++ state for that branch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AiUnavailable {
    /// `MechWarrior::mainDecisionTree` is ported; this variant is kept for old callers and is never raised.
    DecisionTree,
    /// The ABL alarm handler `brainAlarmCallback[alarm]` could not be run (kept for old callers; handlers run now).
    BrainAlarmHandler {
        /// `PILOT_ALARM_*`.
        alarm: u8,
    },
    /// A decision needed `GameObject::lineOfSight` for an object whose LOS inputs are not ported (the object-to-object and point rays
    /// themselves are ported: `object::line_of_sight`, `combat::los::line_of_sight_point`).
    LineOfSight,
    /// `Building::calcAdjacentAreaCell`: the free cell next to a building for a move goal (the global move map areas of buildings).
    AdjacentAreaCell,
    /// Capturing: `isCaptureable`, `getCaptureBlocker`, the capture hand-over.
    Capture,
    /// The refit / get-fixed / recover handshakes (ported; no longer reported, the variant keeps the numbering of the others).
    RefitOrder,
    /// An order's target no longer exists where the C++ dereferences it (undefined behaviour).
    NullOrderTarget,
    /// Nested brain calls run when the native returns, inside the running script. Raised only when a handler chain exceeds the
    /// shared run-count or depth bound; calls left over are dropped rather than run later.
    NestedBrain,
    /// A commander group (`MoverGroup`, lance ids) was asked about.
    Group,
    /// The ABL interpreter stopped with a runtime error.
    AblRuntimeError,
    /// Executing a jump order (`orderJumpToPoint` / `orderJumpToObject`, the jump path request) is the next slice, with the mover's
    /// jump: it did nothing.
    JumpDecision,
    /// A collision between two movers with a jump involved (`BattleMechType::handleCollision`, mech.cpp:838-925: DFA damage, the
    /// bounce to an adjacent cell) is the next slice, with the jump and lane C's damage: it did nothing.
    MoverCollision,
    /// The ABL native `NATIVES[id]` is not implemented by [`SimBindings`](crate::mission::bindings::SimBindings): it returned the
    /// trait default (zero / false) and did nothing.
    Native {
        /// Index into `mc2_abl::natives::NATIVES`.
        id: u16,
    },
}

/// Record that `what` was reached for the pilot of `mover`: one [`SimEvent::AiUnavailable`](crate::SimEvent::AiUnavailable) per pilot
/// and kind, and an entry in [`World::ai_unavailable_frame`](crate::World::ai_unavailable_frame) for the current frame.
pub fn report_unavailable(w: &mut crate::World, mover: ObjectHandle, what: AiUnavailable) {
    let Some((slot, part_id)) = w.objects.get(mover).and_then(|obj| match &obj.kind {
        crate::object::Kind::Mech(m) => Some((m.m.pilot_slot, obj.c.part_id.0)),
        crate::object::Kind::Vehicle(v) => Some((v.m.pilot_slot, obj.c.part_id.0)),
        _ => None,
    }) else {
        return;
    };
    if !w.ai_unavailable_frame.contains(&(part_id, what)) {
        w.ai_unavailable_frame.push((part_id, what));
    }
    let Some(pilot) = w.pilots.get_mut(slot) else {
        return;
    };
    if pilot.unavailable_reported.contains(&what) {
        return;
    }
    pilot.unavailable_reported.push(what);
    w.events
        .push(crate::SimEvent::AiUnavailable { part_id, what });
}

/// [`report_unavailable`] for a call that has no pilot mover to attribute it to (part id 0): once per kind as an event, every frame in
/// [`World::ai_unavailable_frame`](crate::World::ai_unavailable_frame).
pub fn report_unavailable_unowned(w: &mut crate::World, what: AiUnavailable) {
    if !w.ai_unavailable_frame.contains(&(0, what)) {
        w.ai_unavailable_frame.push((0, what));
    }
    if !w.ai_unavailable_unowned.contains(&what) {
        w.ai_unavailable_unowned.push(what);
        w.events
            .push(crate::SimEvent::AiUnavailable { part_id: 0, what });
    }
}

fn packed_order(order: &TacticalOrder, w: &crate::World) -> pilot::QueuedOrder {
    use crate::command::{OrderCode, TravelMode};

    let target = w.objects.get_by_watch_id(order.target);
    let target_is_mover = target.is_some_and(|obj| obj.is_mover());
    let code = if order.code == OrderCode::Guard && target.is_none() {
        31
    } else {
        order.code as u32
    };
    let mut first = ((order.attack_params.aim_location + 2) as u32) & 0x0f;
    first = (first << 2) | (order.attack_params.method as u32 & 0x03);
    first = (first << 2) | (order.attack_params.attack_type as u32 & 0x03);
    first = (first << 16) | (u32::from(order.group_flags) & 0xffff);
    first = (first << 1) | u32::from(order.unit_order);
    first = (first << 2) | (order.origin as u32 & 0x03);
    first = (first << 5) | (code & 0x1f);

    let mut payload = 0u32;
    match order.code {
        OrderCode::MoveToPoint | OrderCode::JumpToPoint | OrderCode::Guard if target.is_none() => {
            let point = order
                .move_params
                .way_path
                .points
                .first()
                .copied()
                .unwrap_or_default();
            let (row, col) = w.terrain.world_to_cell(&point);
            payload = ((row as u32 & 0x3ff) << 10) | (col as u32 & 0x3ff);
        }
        OrderCode::AttackPoint => {
            let (row, col) = w.terrain.world_to_cell(&order.attack_params.target_point);
            payload = ((row as u32 & 0x3ff) << 10) | (col as u32 & 0x3ff);
        }
        OrderCode::Guard
        | OrderCode::AttackObject
        | OrderCode::MoveToObject
        | OrderCode::JumpToObject
        | OrderCode::Capture
        | OrderCode::Refit
        | OrderCode::Recover
        | OrderCode::GetFixed => {
            if let Some(target) = target {
                if target_is_mover {
                    let roster_index = match &target.kind {
                        crate::object::Kind::Mech(m) => m.m.team_roster_index,
                        crate::object::Kind::Vehicle(v) => v.m.team_roster_index,
                        _ => -1,
                    };
                    payload = (((roster_index.max(0) as u32) & 0x7f) << 2) & 0x1ff;
                } else {
                    payload =
                        (((target.c.part_id.0 - MIN_TERRAIN_PART_ID) as u32 & 0xfffff) << 2) | 1;
                }
            }
        }
        _ => {}
    }
    let move_mode = u32::from(order.move_params.jump || order.move_params.mine_laying);
    let run_mode = u32::from(order.move_params.way_path.mode.first() == Some(&TravelMode::Fast));
    let mut second = payload;
    second = (second << 1) | run_mode;
    second = (second << 1) | move_mode;
    second = (second << 1) | u32::from(order.move_params.wait);
    second = (second << 1) | u32::from(order.move_params.face_object);
    second = (second << 1) | u32::from(order.attack_params.obliterate);
    second = (second << 1) | u32::from(order.attack_params.pursue);
    second = (second << 3) | ((order.attack_params.range + 4) as u32 & 0x07);

    pilot::QueuedOrder {
        data: [first, second],
        point: order
            .move_params
            .way_path
            .points
            .first()
            .copied()
            .unwrap_or_default(),
        tactic: order.attack_params.tactic,
    }
}

fn unpack_queued_order(w: &crate::World, queued: &pilot::QueuedOrder, now: f32) -> TacticalOrder {
    use crate::command::{AttackMethod, OrderCode, OrderOrigin, TravelMode};

    let mut first = queued.data[0];
    let raw_code = first & 0x1f;
    first >>= 5;
    let origin = match (first & 0x03) as u8 {
        0 => OrderOrigin::Player,
        1 => OrderOrigin::Commander,
        _ => OrderOrigin::SelfOrder,
    };
    first >>= 2;
    let unit_order = first & 1 != 0;
    first >>= 1;
    let group_flags = (first & 0xffff) as u16;
    first >>= 16;
    let attack_type = (first & 0x03) as i32;
    first >>= 2;
    let attack_method = match (first & 0x03) as u8 {
        1 => AttackMethod::Dfa,
        2 => AttackMethod::Ramming,
        _ => AttackMethod::Ranged,
    };
    first >>= 2;
    let aim_location = (first & 0x0f) as i32 - 2;
    let guard_location = raw_code == 31;
    let code = match if guard_location {
        OrderCode::Guard as u32
    } else {
        raw_code
    } {
        1 => OrderCode::Wait,
        2 => OrderCode::MoveToPoint,
        3 => OrderCode::MoveToObject,
        4 => OrderCode::JumpToPoint,
        5 => OrderCode::JumpToObject,
        6 => OrderCode::TraversePath,
        7 => OrderCode::PatrolPath,
        8 => OrderCode::Escort,
        9 => OrderCode::Follow,
        10 => OrderCode::Guard,
        11 => OrderCode::Stop,
        12 => OrderCode::PowerUp,
        13 => OrderCode::PowerDown,
        14 => OrderCode::WaypointsDone,
        15 => OrderCode::Eject,
        16 => OrderCode::AttackObject,
        17 => OrderCode::AttackPoint,
        18 => OrderCode::HoldFire,
        19 => OrderCode::Withdraw,
        20 => OrderCode::Scramble,
        21 => OrderCode::Capture,
        22 => OrderCode::Refit,
        23 => OrderCode::GetFixed,
        24 => OrderCode::LoadIntoCarrier,
        25 => OrderCode::DeployElementals,
        26 => OrderCode::Recover,
        _ => OrderCode::None,
    };
    let mut order = TacticalOrder::new(origin, code, unit_order, now);
    order.group_flags = group_flags;
    order.attack_params.attack_type = attack_type;
    order.attack_params.method = attack_method;
    order.attack_params.aim_location = aim_location;

    let mut second = queued.data[1];
    order.attack_params.range = (second & 0x07) as i32 - 4;
    second >>= 3;
    order.attack_params.pursue = second & 1 != 0;
    second >>= 1;
    order.attack_params.obliterate = second & 1 != 0;
    second >>= 1;
    order.move_params.face_object = second & 1 != 0;
    second >>= 1;
    order.move_params.wait = second & 1 != 0;
    second >>= 1;
    let move_mode = second & 1 != 0;
    order.move_params.mine_laying = move_mode;
    second >>= 1;
    order.move_params.way_path.mode[0] = if second & 1 != 0 {
        TravelMode::Fast
    } else {
        TravelMode::Slow
    };
    second >>= 1;
    if !guard_location {
        match code {
            OrderCode::MoveToPoint | OrderCode::JumpToPoint => {
                let col = (second & 0x3ff) as i32;
                second >>= 10;
                let row = (second & 0x3ff) as i32;
                order.move_params.way_path.points[0] = w.terrain.cell_to_world(row, col);
                order.move_params.way_path.num_points = 1;
            }
            OrderCode::AttackPoint => {
                let col = (second & 0x3ff) as i32;
                second >>= 10;
                let row = (second & 0x3ff) as i32;
                order.attack_params.target_point = w.terrain.cell_to_world(row, col);
                order.attack_params.target_point.z =
                    w.terrain.elevation(&order.attack_params.target_point);
            }
            OrderCode::Guard
            | OrderCode::AttackObject
            | OrderCode::MoveToObject
            | OrderCode::JumpToObject
            | OrderCode::Capture
            | OrderCode::Refit
            | OrderCode::Recover
            | OrderCode::GetFixed => {
                let target_type = second & 0x03;
                second >>= 2;
                let target_handle = match target_type {
                    // tacordr.cpp resolves a packed mover index only through
                    // MPlayer->moverRoster. In single player MPlayer is null,
                    // so queued mover targets unpack to targetWID == 0.
                    0 => None,
                    1 => w.objects.find_by_part_id(crate::ids::PartId(
                        ((second & 0x000f_ffff) as i32) + MIN_TERRAIN_PART_ID,
                    )),
                    _ => None,
                };
                order.target = target_handle
                    .and_then(|handle| w.objects.get(handle).map(|target| target.c.watch_id))
                    .unwrap_or(WatchId(0));
            }
            _ => {}
        }
    }
    if matches!(
        code,
        OrderCode::MoveToPoint | OrderCode::JumpToPoint | OrderCode::Guard
    ) {
        order.move_params.way_path.points[0] = queued.point;
        order.move_params.way_path.num_points = 1;
    }
    order.attack_params.tactic = queued.tactic;
    order
}

/// What [`pilot_update`] found.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotUpdateStatus {
    /// The object has no pilot slot or its slot is not populated.
    NoPilot,
    /// `mainDecisionTree` ran. Anything in it that is not ported was reported through [`report_unavailable`].
    Done,
}

/// `MechWarrior::mainDecisionTree` for the pilot of `mover` (warrior.cpp:4899-5115), called from `updateAIControl` of the mover
/// update. See [`decide::main_decision_tree`].
pub fn pilot_update(w: &mut crate::World, mover: ObjectHandle) -> PilotUpdateStatus {
    let Some(slot) = warrior::slot_of(w, mover) else {
        return PilotUpdateStatus::NoPilot;
    };
    // The mover calling its pilot is the pilot's vehicle (`vehicleWID`, set by `Mover::setPilotHandle`).
    let wid = w.objects.watch_id(mover);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.mover = wid;
    }
    decide::main_decision_tree(w, slot);
    PilotUpdateStatus::Done
}

/// `MechWarrior::rethinkPath(strategy)` (warrior.cpp:2515).
pub fn rethink_path(w: &mut crate::World, mover: ObjectHandle, strategy: u32) {
    if let Some(slot) = warrior::slot_of(w, mover) {
        warrior::rethink_path(w, slot, strategy);
    }
}

/// `MechWarrior::getMovePath()` with all its side effects (see [`warrior::get_move_path_index`]): lane B's seam. Returns an owned copy
/// of the (possibly just swapped in) current path; the live path is `pilot.lane_b_move.paths[0]`.
pub fn get_move_path(w: &mut crate::World, slot: u16) -> Option<mc2_path::MovePath> {
    warrior::get_move_path(w, slot)
}

/// `MechWarrior::pausePath` (`warrior.cpp:2499`).
pub fn pause_path(w: &mut crate::World, slot: u16) {
    warrior::pause_path(w, slot)
}

/// `MechWarrior::resumePath` (`warrior.cpp:2507`).
pub fn resume_path(w: &mut crate::World, slot: u16) {
    warrior::resume_path(w, slot)
}

/// `MechWarrior::isYielding()` (`moveOrders.yieldTime > -1`).
pub fn is_yielding(w: &crate::World, slot: u16) -> bool {
    w.pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.is_yielding)
}

/// `MechWarrior::requestMovePath(selectionIndex, moveParams, source)`.
pub fn request_move_path(
    w: &mut crate::World,
    slot: u16,
    selection: i32,
    params: u32,
    source: i32,
) {
    warrior::request_move_path(w, slot, selection, params, source)
}

/// `Mover::getChallenger()` (mover.cpp:4028): `ObjectManager->get(challengerWID)` (a watch id used as a HANDLE, as in the source); a
/// disabled challenger counts as none and is forgotten.
pub fn get_challenger_handle(w: &mut crate::World, mover: ObjectHandle) -> Option<ObjectHandle> {
    let slot = warrior::slot_of(w, mover)?;
    let wid = w.pilots.get(slot)?.challenger.0 as usize;
    let obj = (wid >= 1 && wid <= w.objects.max_objects())
        .then_some(ObjectHandle(wid as u32))
        .filter(|&h| w.objects.get(h).is_some())?;
    if w.objects.get(obj).is_some_and(|o| o.c.is_disabled()) {
        w.pilots.get_mut(slot)?.challenger = WatchId(0);
        return None;
    }
    Some(obj)
}

/// `MAPBUTTONS_GUI` (mclib/sounds.h:63), the click `IfaceCallStrike` plays when the player's team sees the LARGE/SMALL strike point.
pub const MAPBUTTONS_GUI: i32 = 51;

/// `IfaceCallStrike` for the player (artlry.cpp:159-232), with the merged team line of sight
/// ([`crate::object::hooks::team_line_of_sight`], team.cpp:762). An id that is not a strike type is ignored like C++.
/// `bRandom` is the negation of `Team::home->teamLineOfSight(strikeLocation, 0.0f)` (artlry.cpp:185): a LARGE strike the team cannot
/// see becomes a SMALL (random-offset) strike (artlry.cpp:212-218), and the `MAPBUTTONS_GUI` click sounds only for a LARGE strike the
/// team can see (artlry.cpp:199-205). A sensor strike needs no line of sight. `commanderID` is `Commander::home->getId()`
/// (artlry.cpp:184).
fn issue_call_strike(w: &mut crate::World, strike: i32, at: crate::math::Vec3) {
    use crate::combat::artillery_math::normalize_strike;
    let home = w.teams.home as i32;
    let in_los = crate::object::hooks::team_line_of_sight(w, home, at, 0.0);
    let Some(strike) = normalize_strike(strike, !in_los) else {
        return;
    };
    w.events.push(crate::SimEvent::Support(
        if strike == crate::combat::artillery_math::ARTILLERY_SENSOR {
            crate::events::support::PROBE
        } else {
            crate::events::support::AIRSTRIKE
        },
    ));
    // `switch (strikeID)` before the SMALL/LARGE remap: the case labels are LARGE and SMALL, so a SENSOR strike plays nothing.
    if matches!(
        strike,
        crate::combat::artillery_math::ARTILLERY_LARGE
            | crate::combat::artillery_math::ARTILLERY_SMALL
    ) && in_los
    {
        // `playDigitalSample(MAPBUTTONS_GUI)` has no position (artlry.cpp:199-205): a centered click, not attenuated by distance.
        w.events.push(crate::SimEvent::Sound {
            id: MAPBUTTONS_GUI,
            source: crate::ids::WatchId::NONE,
            at: None,
            allow_dupes: false,
        });
    }
    // `Commander::home`: the commander of the home team.
    let commander = w
        .teams
        .commanders
        .iter()
        .position(|c| c.as_ref().is_some_and(|c| c.team as usize == w.teams.home))
        .map_or(0, |i| i as i8);
    crate::combat::artillery::call_strike(w, commander, strike, at);
}

/// Apply one player command at the point `missionInterface->update()` ran (before the path manager). Builds the
/// `TacticalOrder`s the GUI would (`handleOrders`, missiongui.cpp:2462: per-mover `attackRange`, `pack`, queue/waypoint
/// logic, `MoverGroup::calcMoveGoals` goal assignment in `Team::home` roster order) and calls [`handle_tactical_order`].
pub fn issue_player_command(w: &mut crate::World, cmd: &PlayerCommand) {
    if let PlayerCommand::SetAttackRange {
        movers,
        range,
        update_current_order,
        fire_from_current_position,
    } = cmd
    {
        for part in movers {
            if let Some(handle) = w.objects.find_by_part_id(*part) {
                crate::mover::set_attack_range(
                    w,
                    handle,
                    *range,
                    *update_current_order,
                    *fire_from_current_position,
                );
            }
        }
        return;
    }
    if let PlayerCommand::SetForceGroup { group, members } = cmd {
        set_force_group(w, *group, members);
        return;
    }
    if let PlayerCommand::ClearForceGroup { mover } = cmd {
        w.mission.unit_groups.remove(mover);
        return;
    }
    if let PlayerCommand::Scripted(scripted) = cmd {
        issue_scripted_order(w, scripted);
        return;
    }
    if let PlayerCommand::CallStrike { strike, at } = cmd {
        issue_call_strike(w, *strike, *at);
        return;
    }
    if let PlayerCommand::AddResourcePoints(n) = cmd {
        w.mission.resource_points += *n;
        return;
    }
    if let PlayerCommand::BeginVtol {
        commander,
        vehicle_id,
        at,
        target,
    } = cmd
    {
        let refused = crate::mission::vtol::begin_vtol(w, *commander, *vehicle_id, *at, *target);
        if refused == Err(crate::mission::vtol::VtolRefusal::NotPrepared) {
            w.note_frame_unavailable(
                "support vehicle: the VTOL animations and vehicle files are not loaded",
            );
        }
        if refused.is_err() {
            w.events.push(crate::events::SimEvent::VtolRefused {
                commander: *commander,
            });
        }
        return;
    }
    if let PlayerCommand::SetSuppressionFire { movers, on } = cmd {
        for part in movers {
            if let Some(core) = w
                .objects
                .find_by_part_id(*part)
                .and_then(|h| warrior::slot_of(w, h))
                .and_then(|slot| warrior::core_mut(w, slot))
            {
                core.suppression_fire = i32::from(*on);
            }
        }
        return;
    }
    let (movers, order, queue, release_queue) = match cmd {
        PlayerCommand::Order {
            movers,
            order,
            queue,
        } => (movers, order, queue, false),
        PlayerCommand::OrderQueueExecute { movers, order } => (movers, order, &true, true),
        _ => return,
    };
    let mut selected: Vec<_> = movers
        .iter()
        .filter_map(|part| w.objects.find_by_part_id(*part))
        .filter_map(|handle| {
            let obj = w.objects.get(handle)?;
            let (team, roster_index, range) = match &obj.kind {
                crate::object::Kind::Mech(m) => (m.m.team, m.m.team_roster_index, m.m.attack_range),
                crate::object::Kind::Vehicle(v) => {
                    (v.m.team, v.m.team_roster_index, v.m.attack_range)
                }
                _ => return None,
            };
            Some((team, roster_index, range, handle))
        })
        .collect();
    // `MissionInterfaceManager::handleOrders` walks the team's roster, not the GUI selection
    // vector, when assigning `calcMoveGoals` results.
    selected.sort_by_key(|(team, roster, _, _)| (*team, *roster));
    let mut goals = Vec::with_capacity(selected.len());
    if matches!(order.code, crate::command::OrderCode::MoveToPoint) {
        let click = order.move_params.way_path.points[0];
        goals = calc_move_goals(w, click, selected.len());
    } else if order.code == crate::command::OrderCode::JumpToPoint && selected.len() > 1 {
        goals = calc_jump_goals(w, order.move_params.way_path.points[0], selected.len());
    }
    for (selection_index, (team, roster_index, range, handle)) in selected.into_iter().enumerate() {
        let mut per_mover = order.clone();
        per_mover.origin = crate::command::OrderOrigin::Player;
        per_mover.selection_index = selection_index as i32;
        per_mover.attack_params.range = range;
        if matches!(
            per_mover.code,
            crate::command::OrderCode::MoveToPoint | crate::command::OrderCode::JumpToPoint
        ) {
            if let Some(goal) = goals.get(selection_index) {
                per_mover.move_params.way_path.points[0] = *goal;
            }
        }
        let _ = (team, roster_index);
        if per_mover.code == crate::command::OrderCode::WaypointsDone {
            // `MissionInterfaceManager::update` releases a queued route by
            // setting `tacOrderQueueExecuting` and calling
            // `executeTacOrderQueue` (`missiongui.cpp:1711-1728`). This is
            // the explicit transition; enqueuing alone leaves it pending.
            let Some(slot) = w.objects.get(handle).and_then(|obj| match &obj.kind {
                crate::object::Kind::Mech(mech) => Some(mech.m.pilot_slot),
                crate::object::Kind::Vehicle(vehicle) => Some(vehicle.m.pilot_slot),
                _ => None,
            }) else {
                continue;
            };
            if w.pilots
                .get(slot)
                .is_some_and(|pilot| pilot.queue_executing)
            {
                continue;
            }
            if let Some(pilot) = w.pilots.get_mut(slot) {
                pilot.queue_executing = true;
            }
            decide::execute_tac_order_queue(w, slot);
            continue;
        }
        handle_tactical_order(w, handle, per_mover, 0, *queue);
        if release_queue {
            // `pilot->setExecutingQueue(TRUE)` after `addQueuedTacOrder` (missiongui.cpp:2520)
            if let Some(slot) = warrior::slot_of(w, handle) {
                if let Some(pilot) = w.pilots.get_mut(slot) {
                    pilot.queue_executing = true;
                }
            }
        }
    }
}

/// `MoverGroup::calcJumpGoals` (group.cpp:532): search the nine-cell-square
/// landing map in the original outward spiral, excluding occupied cells.
pub fn calc_jump_goals(
    w: &crate::World,
    click: crate::math::Vec3,
    n: usize,
) -> Vec<crate::math::Vec3> {
    let (row, col) = w.terrain.world_to_cell(&click);
    let occupied: std::collections::HashSet<_> = w
        .objects
        .movers
        .iter()
        .filter_map(|h| w.objects.get(*h))
        .filter(|o| {
            o.exists() && !o.c.is_disabled() && o.c.class != crate::object::ObjectClass::Elemental
        })
        .map(|o| (i32::from(o.c.cell.0), i32::from(o.c.cell.1)))
        .collect();
    let mut cells = Vec::new();
    let mut r = 0;
    let mut c = 0;
    let mut visit = |r: i32, c: i32| {
        let cell = (row + r, col + c);
        if r.abs() <= 4
            && c.abs() <= 4
            && !occupied.contains(&cell)
            && crate::object::terr::get_passable(&w.terrain, cell.0, cell.1)
        {
            cells.push(w.terrain.cell_to_world(cell.0, cell.1));
        }
    };
    visit(r, c);
    let dirs = [(-1, 0), (0, 1), (1, 0), (0, -1)];
    for segment in 0..18 {
        let (dr, dc) = dirs[segment % 4];
        for _ in 0..(segment / 2 + 1) {
            r += dr;
            c += dc;
            visit(r, c);
        }
    }
    cells.truncate(n);
    cells.resize(
        n,
        crate::math::Vec3 {
            x: -99999.0,
            y: -99999.0,
            z: -99999.0,
        },
    );
    cells
}

/// `MissionInterfaceManager::makeForceGroup` (`code/missiongui.cpp:3284-3307`).  A mover can
/// belong to one group only (`Mover::addToUnitGroup`, `code/mover.cpp:7163-7176`), so assigning
/// the selected movers also removes them from any former group.
fn set_force_group(w: &mut crate::World, group: u8, members: &[PartId]) {
    if group >= 10 {
        return;
    }
    let selected: std::collections::BTreeSet<_> = members.iter().copied().collect();
    let handles = w.objects.movers.clone();
    for handle in handles {
        let is_home = w.objects.get(handle).is_some_and(|obj| match &obj.kind {
            crate::object::Kind::Mech(m) => m.m.commander.0 == crate::combat::HOME_COMMANDER as i8,
            crate::object::Kind::Vehicle(v) => {
                v.m.commander.0 == crate::combat::HOME_COMMANDER as i8
            }
            _ => false,
        });
        if !is_home {
            continue;
        }
        let part = match w.objects.get(handle) {
            Some(obj) => obj.c.part_id,
            None => continue,
        };
        if w.mission.unit_groups.get(&part) == Some(&group) {
            w.mission.unit_groups.remove(&part);
        }
        if selected.contains(&part) {
            // Inserting replaces an earlier membership, matching `addToUnitGroup`'s overwrite.
            w.mission.unit_groups.insert(part, group);
        }
    }
}

/// The oracle's `issueOrder` (`oracle/oracle_main.cpp`): build the order of one script line at the time it is issued
/// (`TacticalOrder::init(origin, code, unitOrder)` stamps `scenarioTime`) and call `Mover::handleTacticalOrder(order)`.
fn issue_scripted_order(w: &mut crate::World, scripted: &crate::command::ScriptedOrder) {
    use crate::command::{AttackMethod, OrderCode, OrderOrigin, ScriptedOrder, TravelMode};
    let now = w.clock.scenario_time;
    let part = match *scripted {
        ScriptedOrder::Move { part, .. }
        | ScriptedOrder::Attack { part, .. }
        | ScriptedOrder::Jump { part, .. }
        | ScriptedOrder::Stop { part } => part,
    };
    let Some(handle) = w
        .objects
        .find_by_part_id(part)
        .filter(|h| w.objects.get(*h).is_some_and(|o| o.is_mover()))
    else {
        return;
    };
    let point = |w: &crate::World, x: f32, y: f32| {
        let mut p = crate::math::Vec3 { x, y, z: 0.0 };
        p.z = w.terrain.elevation(&p);
        p
    };
    let order = match *scripted {
        ScriptedOrder::Move { x, y, run, .. } => {
            let mut order =
                TacticalOrder::new(OrderOrigin::Player, OrderCode::MoveToPoint, false, now);
            order.move_params.way_path.points = vec![point(w, x, y)];
            order.move_params.wait = false;
            order.move_params.way_path.mode = vec![if run {
                TravelMode::Fast
            } else {
                TravelMode::Slow
            }];
            order
        }
        ScriptedOrder::Attack { target, .. } => {
            let Some(target_handle) = w.objects.find_by_part_id(target) else {
                return;
            };
            let range = match w.objects.get(handle).map(|o| &o.kind) {
                Some(crate::object::Kind::Mech(m)) => m.m.attack_range,
                Some(crate::object::Kind::Vehicle(v)) => v.m.attack_range,
                _ => return,
            };
            let mut order =
                TacticalOrder::new(OrderOrigin::Player, OrderCode::AttackObject, false, now);
            order.target = w.objects.watch_id(target_handle);
            order.attack_params.attack_type = 1; // ATTACK_TO_DESTROY
            order.attack_params.method = AttackMethod::Ranged;
            order.attack_params.pursue = true;
            order.move_params.way_path.mode = vec![TravelMode::Fast];
            order.attack_params.range = range;
            order
        }
        ScriptedOrder::Jump { x, y, .. } => {
            let mut order =
                TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, now);
            order.move_params.wait = false;
            order.move_params.way_path.mode = vec![TravelMode::Jump];
            order.move_params.way_path.points = vec![point(w, x, y)];
            order
        }
        ScriptedOrder::Stop { .. } => {
            TacticalOrder::new(OrderOrigin::Player, OrderCode::Stop, false, now)
        }
    };
    handle_tactical_order(w, handle, order, 0, false);
}

/// `Mover::handleTacticalOrder(tacOrder, priority, queuePlayerOrder)` (mover.cpp:3161-3310): per-code gating (a jump needs a mech that
/// can move, a landing cell that is passable and a target within `getJumpRange()`; a move needs `canMove()`), the DFA attack becomes a
/// jump-to-object order, then the order goes to the slot of its origin (`setPlayerTacOrder` / `setGeneralTacOrder` /
/// `setAlarmTacOrder(priority)`), or into the pilot's order queue for a queued player order.
pub fn handle_tactical_order(
    w: &mut crate::World,
    mover: ObjectHandle,
    mut order: TacticalOrder,
    priority: i32,
    queue: bool,
) -> i32 {
    use crate::command::{AttackMethod, OrderCode, OrderOrigin, TravelMode};
    let Some(slot) = warrior::slot_of(w, mover) else {
        return 0;
    };
    // `if (queuePlayerOrder) tacOrder.pack(NULL, NULL);` and the packed data is what the queue stores
    let packed = queue.then(|| packed_order(&order, w));
    let mut process_order = true;
    let mut message = -1;
    let (my_class, my_team, selection_index) = {
        let Some(obj) = w.objects.get(mover) else {
            return 0;
        };
        (
            obj.c.class,
            obj.team_id(),
            crate::combat::core_of(obj).map_or(-1, |c| c.selection_index),
        )
    };
    let can_move = mover_ops::can_move(w, mover);
    let disabled = w.objects.get(mover).is_some_and(|o| o.c.is_disabled());
    let mut check_move = false;
    match order.code {
        OrderCode::Wait => {
            if let Some(core) = warrior::core_mut(w, slot) {
                core.suppression_fire = 0;
            }
        }
        OrderCode::MoveToPoint => {
            // If we're part of a selected group, offset our goal slightly...
            if selection_index != -1 {
                let delayed = w
                    .data
                    .gamesys
                    .tables
                    .as_ref()
                    .map_or(1.0, |t| t.pathfinding.delayed_order_time);
                order.delayed_time = w.clock.scenario_time + (selection_index as f32 * delayed);
            }
            if !can_move {
                // DO NOT send radio message. Players will know artillery cannot move.
                process_order = false;
            }
        }
        OrderCode::JumpToPoint | OrderCode::JumpToObject => {
            let mut can_jump = my_class == crate::object::ObjectClass::BattleMech;
            let target = w.objects.get_by_watch_id(order.target);
            if let Some(t) = target {
                if t.is_mover() && t.team_id() == my_team {
                    can_jump = false;
                }
            }
            let waypoint = order
                .move_params
                .way_path
                .points
                .first()
                .copied()
                .unwrap_or_default();
            if mover_ops::jump_range(w, mover) < mover_ops::distance_from(w, mover, &waypoint) {
                can_jump = false;
            }
            if my_class == crate::object::ObjectClass::BattleMech
                && !w.terrain.passable_at(&waypoint)
            {
                can_jump = false;
            }
            if !can_jump {
                message = orders::radio::ILLEGAL_ORDER;
                process_order = false;
            }
            // No break here--falls into the move cases below
            check_move = true;
        }
        OrderCode::MoveToObject | OrderCode::TraversePath | OrderCode::PatrolPath => {
            check_move = true
        }
        OrderCode::AttackObject => {
            if order.attack_params.method == AttackMethod::Dfa {
                // Let's just make it a move/jump order, for now...
                order.code = OrderCode::JumpToObject;
                order.move_params.wait = false;
                if let Some(mode) = order.move_params.way_path.mode.first_mut() {
                    *mode = TravelMode::Slow;
                }
                if let Some(target) = w.objects.get_by_watch_id(order.target) {
                    let pos = target.c.position;
                    match order.move_params.way_path.points.first_mut() {
                        Some(point) => *point = pos,
                        None => order.move_params.way_path.points.push(pos),
                    }
                }
            }
            if order.attack_params.aim_location != -1 {
                message = 1; // RADIO_CALLED_SHOT
            }
        }
        OrderCode::PowerUp
        | OrderCode::PowerDown
        | OrderCode::Escort
        | OrderCode::Follow
        | OrderCode::Guard
        | OrderCode::Stop
        | OrderCode::WaypointsDone
        | OrderCode::Eject
        | OrderCode::HoldFire
        | OrderCode::Withdraw
        | OrderCode::AttackPoint
        | OrderCode::Refit
        | OrderCode::GetFixed
        | OrderCode::Capture
        | OrderCode::Recover
        | OrderCode::LoadIntoCarrier
        | OrderCode::DeployElementals => {}
        // `Assert(false, ..., "Mover::handleTacticalOrder->Bad TacOrder Code")`; return 1
        OrderCode::None | OrderCode::Scramble => return 1,
    }
    if check_move && (disabled || !can_move) {
        // Tell everyone we have received the order.
        process_order = false;
    }
    warrior::radio_message(w, slot, message);

    // Acknowledgement of a good order is done when the order is executed...
    if process_order {
        match order.origin {
            OrderOrigin::Player => {
                if let Some(packed) = packed {
                    add_queued_order(w, slot, packed);
                } else {
                    warrior::set_player_tac_order(w, slot, order, false);
                }
            }
            OrderOrigin::Commander => warrior::set_general_tac_order(w, slot, order),
            OrderOrigin::SelfOrder => warrior::set_alarm_tac_order(w, slot, order, priority),
        }
    }
    0
}

/// `MechWarrior::addQueuedTacOrder` (warrior.cpp:1573-1625): a seventeenth order is refused (2). The first queued order starts at once
/// when the queue is already released (`tacOrderQueueExecuting`) and no player order is pending or running from the queue.
pub fn add_queued_order(w: &mut crate::World, slot: u16, queued: pilot::QueuedOrder) -> i32 {
    let Some(p) = w.pilots.get_mut(slot) else {
        return 1;
    };
    if p.queue.len() == 16 {
        return 2;
    }
    p.queue.push(queued);
    if p.queue.len() == 1
        && !p.new_order_received[1]
        && (!p.cur_player_order_from_queue
            || p.cur_order.origin != crate::command::OrderOrigin::Player)
        && p.queue_executing
    {
        decide::execute_tac_order_queue(w, slot);
    }
    0
}

/// `Mover::getContacts`/`SensorSystem::getContacts` with criteria bits (ENEMY 1, VISUAL 2, NOT_CHALLENGED 8, SENSOR 16,
/// VISUAL_OR_SENSOR 32, NOT_DISABLED 64, ARMED 128) and sort (none / CV descending / distance ascending). Ties keep list order
/// (the oracle sorts stably). Results are object handles, matching `TeamSensorSystem::getContacts`
/// (`contact.cpp:881-936`).
pub fn contacts(
    w: &crate::World,
    looker: ObjectHandle,
    criteria: u32,
    sort: ContactSort,
) -> Vec<ObjectHandle> {
    const ENEMY: u32 = 1;
    const VISUAL: u32 = 2;
    const NOT_CHALLENGED: u32 = 8;
    const SENSOR: u32 = 16;
    const NOT_DISABLED: u32 = 64;
    const ARMED: u32 = 128;

    let Some(looker_obj) = w.objects.get(looker) else {
        return Vec::new();
    };
    let team = looker_obj.team_id();
    // `Mover::getContacts` delegates through that mover's `sensorSystem` pointer
    // (mover.cpp:5523-5527); a teammate's sensors alone do not give this mover
    // a contact-query interface.
    let Some(team_index) = sensors::mover_sensor_team(w, looker) else {
        return Vec::new();
    };
    let mut result = w.sensors.team_contact_order[team_index]
        .iter()
        .copied()
        .filter_map(|handle| {
            let object = w.objects.get(handle)?;
            let status = w
                .sensors
                .team_contact_status
                .get(&handle)
                .map_or(0, |statuses| statuses[team_index]);
            // TeamSensorSystem::meetsCriteria rejects removed movers and
            // contacts that no longer have a status (contact.cpp:821-836).
            if !object.exists()
                || object.c.flags & crate::object::flags::REMOVED != 0
                || status == 0
            {
                return None;
            }
            let target_team = object.team_id();
            let relation = crate::object::hooks::team_relation(w, team, target_team);
            let enemy = relation == Some(Relation::Enemy);
            let disabled = object.c.is_disabled();
            let has_functional_weapon = |inventory: &[crate::combat::InventoryItem]| {
                inventory.iter().any(|i| {
                    i.amount > 0
                        && i.health > 0
                        && !i.disabled
                        && w.data
                            .components
                            .as_ref()
                            .and_then(|list| list.get(i32::from(i.master_id)))
                            .is_some_and(|c| {
                                c.form == Some(mc2_sim_data::component::ComponentForm::Weapon)
                            })
                })
            };
            let armed = match &object.kind {
                crate::object::Kind::Mech(m) => has_functional_weapon(&m.loadout.inventory),
                crate::object::Kind::Vehicle(v) => has_functional_weapon(&v.loadout.inventory),
                _ => false,
            };
            let challenged = match &object.kind {
                crate::object::Kind::Mech(m) => w
                    .pilots
                    .get(m.m.pilot_slot)
                    .is_some_and(|p| p.challenger != WatchId(0)),
                crate::object::Kind::Vehicle(v) => w
                    .pilots
                    .get(v.m.pilot_slot)
                    .is_some_and(|p| p.challenger != WatchId(0)),
                _ => false,
            };
            if criteria & ENEMY != 0 && !enemy
                || criteria & VISUAL != 0 && status != 5
                || criteria & SENSOR != 0 && status == 5
                || criteria & NOT_CHALLENGED != 0 && challenged
                || criteria & NOT_DISABLED != 0 && disabled
                || criteria & ARMED != 0 && !armed
            {
                return None;
            }
            Some((handle, object.c.cur_cv, object.c.position))
        })
        .collect::<Vec<_>>();
    match sort {
        ContactSort::None => {}
        ContactSort::Cv => result.sort_by_key(|a| std::cmp::Reverse(a.1)),
        ContactSort::Distance => {
            let origin = looker_obj.c.position;
            result.sort_by(|a, b| {
                contact_distance(origin, a.2).total_cmp(&contact_distance(origin, b.2))
            });
        }
    }
    result.into_iter().map(|(handle, _, _)| handle).collect()
}

/// Everything unported reached during the frame that just ran: pilot AI, combat hooks and the in-view approximation.
pub fn frame_unavailable(w: &crate::World) -> Vec<crate::FrameUnavailable> {
    let mut out = w
        .ai_unavailable_frame
        .iter()
        .map(|&(part_id, what)| crate::FrameUnavailable::Ai { part_id, what })
        .collect::<Vec<_>>();
    for &hook in &w.combat_unavailable_frame {
        out.push(crate::FrameUnavailable::Combat { hook });
    }
    if !w.visibility.object_in_view_available() {
        out.push(crate::FrameUnavailable::ObjectInView);
    }
    out
}

fn contact_distance(a: crate::math::Vec3, b: crate::math::Vec3) -> f32 {
    crate::math::approximate_length(&crate::math::Vec3 {
        x: a.x - b.x,
        y: a.y - b.y,
        // GameObject::distanceFrom discards vertical separation (gameobj.cpp:1762-1768).
        z: 0.0,
    })
}

/// Contact list ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactSort {
    /// Insertion order.
    None,
    /// `getCurCV` descending.
    Cv,
    /// Distance ascending.
    Distance,
}

/// `MoverGroup::calcMoveGoals` (group.cpp:401): spread `n` movers around a click (static goal map persists between calls).
pub fn calc_move_goals(
    w: &mut crate::World,
    click: crate::math::Vec3,
    n: usize,
) -> Vec<crate::math::Vec3> {
    if n == 0 {
        return Vec::new();
    }
    use mc2_path::pqueue::{PqNode, PriorityQueue};

    const DIM: i32 = 61;
    const BLOCKED: i32 = i32::MAX;
    const OPEN: u8 = 1;
    const CLOSED: u8 = 2;
    const AVAILABLE: u8 = 4;
    const NO_NEIGHBORS: u8 = 8;
    #[derive(Clone, Copy, Default)]
    struct GoalNode {
        cost: i32,
        flags: u8,
        g: i32,
    }
    let (goal_row, goal_col) = w.terrain.world_to_cell(&click);
    let top_row = goal_row - DIM / 2;
    let top_col = goal_col - DIM / 2;
    let mut map = vec![GoalNode::default(); (DIM * DIM) as usize];
    for row in 0..DIM {
        for col in 0..DIM {
            let map_row = top_row + row;
            let map_col = top_col + col;
            let point = w.terrain.cell_to_world(map_row, map_col);
            if point.x == 0.0 && point.y == 0.0 {
                continue;
            }
            let cell = &mut map[(row * DIM + col) as usize];
            cell.cost = if crate::object::terr::get_passable(&w.terrain, map_row, map_col) {
                100
            } else {
                BLOCKED
            };
            cell.flags = AVAILABLE | NO_NEIGHBORS;
        }
    }
    let center = (DIM / 2 * DIM + DIM / 2) as usize;
    let mut open = PriorityQueue::with_default_key_min(5000);
    let _ = open.insert(PqNode {
        key: map[center].g,
        id: center as u32,
        row: DIM / 2,
        col: DIM / 2,
    });
    map[center].flags |= OPEN;
    let mut goals = Vec::with_capacity(n);
    const EIGHT: [(i32, i32); 8] = [
        (-1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
        (1, 0),
        (1, -1),
        (0, -1),
        (-1, -1),
    ];
    const FOUR: [(i32, i32); 4] = [(-1, 0), (0, 1), (1, 0), (0, -1)];
    while !open.is_empty() {
        let best = open.remove();
        let row = best.row;
        let col = best.col;
        let index = (row * DIM + col) as usize;
        map[index].flags &= !OPEN;
        map[index].flags |= CLOSED;
        let mut mark_neighbors = false;
        if map[index].cost < BLOCKED
            && map[index].flags & NO_NEIGHBORS != 0
            && map[index].flags & AVAILABLE != 0
        {
            map[index].flags &= !AVAILABLE;
            goals.push(w.terrain.cell_to_world(top_row + row, top_col + col));
            mark_neighbors = true;
            if goals.len() == n {
                break;
            }
        }
        if mark_neighbors {
            for (dr, dc) in EIGHT {
                // C++ group.cpp:481 uses curRow for the column base as well.
                let nr = row + dr;
                let nc = row + dc;
                if (0..DIM).contains(&nr) && (0..DIM).contains(&nc) {
                    map[(nr * DIM + nc) as usize].flags &= !NO_NEIGHBORS;
                }
            }
        }
        let best_g = map[index].g;
        for (dr, dc) in FOUR {
            let nr = row + dr;
            let nc = col + dc;
            if !(0..DIM).contains(&nr) || !(0..DIM).contains(&nc) {
                continue;
            }
            let successor = (nr * DIM + nc) as usize;
            if map[successor].cost == BLOCKED || map[successor].flags & (OPEN | CLOSED) != 0 {
                continue;
            }
            map[successor].g = best_g + map[successor].cost;
            let _ = open.insert(PqNode {
                key: map[successor].g,
                id: successor as u32,
                row: nr,
                col: nc,
            });
            map[successor].flags |= OPEN;
        }
    }
    goals
}

/// The lane D digest rows of the oracle's `--lane-d-rows` option (`emitPilotAndContactDigests`, oracle/DIGEST.md), one `P` and one `KM`
/// line per mover that has a pilot, in mover-list order:
///
/// ```text
/// P <part> <wounds> o<order> g<stage> tw<wid> tg<wid> b<brainUpdate> c<combatUpdate> m<movementUpdate> a<alarm triggers> q<queued orders>
/// KM <part> <contactStatus[team 0]> ... <contactStatus[team 7]>
/// ```
///
/// Read straight from the pilot and from the sensor manager's per-team contact status; nothing is shared with the oracle.
pub fn digest_pilots(w: &crate::World, _level: u32, out: &mut Vec<String>) {
    for &handle in &w.objects.movers {
        let Some(obj) = w.objects.get(handle) else {
            continue;
        };
        let slot = match &obj.kind {
            crate::object::Kind::Mech(m) => m.m.pilot_slot,
            crate::object::Kind::Vehicle(v) => v.m.pilot_slot,
            _ => continue,
        };
        let Some(pilot) = (slot != 0).then(|| w.pilots.get(slot)).flatten() else {
            continue;
        };
        let part = i64::from(obj.c.part_id.0);
        let alarms: usize = pilot.alarms.iter().map(Vec::len).sum();
        let mut p = crate::digest::LineBuilder::new("P");
        p.i(part)
            .f(pilot.wounds)
            .pi("o", pilot.cur_order.code as i64)
            .pi("g", i64::from(pilot.cur_order.stage))
            .pi("tw", i64::from(pilot.cur_order.target.0))
            .pi("tg", i64::from(pilot.last_target.0 .0));
        for (letter, time) in ["b", "c", "m"].iter().zip(pilot.timers) {
            p.s(&format!("{letter}{}", crate::digest::fmt_f32(time)));
        }
        p.pi("a", alarms as i64).pi("q", pilot.queue.len() as i64);
        out.push(p.build());
        out.push(pilot_decision_line(part, pilot));
        let status = w
            .sensors
            .team_contact_status
            .get(&handle)
            .copied()
            .unwrap_or([0; crate::ids::MAX_TEAMS]);
        let mut k = crate::digest::LineBuilder::new("KM");
        k.i(part);
        for s in status {
            k.i(i64::from(s));
        }
        out.push(k.build());
    }
}

/// The `PD` row of one pilot (oracle/DIGEST.md): the decision state `mainDecisionTree` works on, read from fields, never through a
/// mutating getter.
fn pilot_decision_line(part: i64, p: &Pilot) -> String {
    use crate::digest::fmt_f32 as f;
    let order = |o: &TacticalOrder| format!("{}:{}:{}", o.code as i32, o.stage, o.target.0);
    let b = |v: bool| i32::from(v);
    let m = &p.lane_b_move;
    let path = |m: &mc2_path::MovePath| {
        format!(
            "{}:{}:{}:{}",
            m.num_steps, m.num_steps_when_not_paused, m.cur_step, m.global_step
        )
    };
    let state = match p.order_state {
        pilot::OrderState::General => 0,
        pilot::OrderState::Player => 1,
        pilot::OrderState::Alarm => 2,
    };
    let mut line = format!(
        "PD {part} os{state} nr{}{}{} ap{} og{} op{} oa{} ol{} ws{} lt{} lf{}{}{}",
        b(p.new_order_received[0]),
        b(p.new_order_received[1]),
        b(p.new_order_received[2]),
        p.alarm_priority,
        order(&p.tac_orders[0]),
        order(&p.tac_orders[1]),
        order(&p.tac_orders[2]),
        p.last_order.code as i32,
        p.weapons_status_result,
        f(p.last_target.1),
        b(p.last_target_flags[0]),
        b(p.last_target_flags[1]),
        b(p.last_target_flags[2]),
    );
    line.push_str(&format!(
        " mg{} mo{} gx{} gy{} gz{} mt{} ms{} mq{} mr{} mk{} my{} mz{} mw{} ml{}",
        m.goal_type & 0xFFFF_FFFF,
        m.goal_object_wid.0,
        f(m.goal.x),
        f(m.goal.y),
        f(m.goal.z),
        m.path_type,
        m.move_state,
        m.move_state_goal,
        b(m.run),
        b(m.twisting),
        f(m.yield_time),
        m.yield_state,
        f(m.wait_for_point_time),
        f(m.time_of_last_step),
    ));
    line.push_str(&format!(
        " p0{} p1{} ng{}:{} ox{} oy{}",
        path(&m.paths[0]),
        path(&m.paths[1]),
        m.num_global_steps,
        m.cur_global_step,
        f(m.original_global_goal[0].x),
        f(m.original_global_goal[0].y),
    ));
    line.push_str(&format!(
        " bs{} tl{} wh{} km{} ug{} gm{}:{} ct{} qe{} qi{} at{}:{}:{} ar{}",
        p.brain_state,
        f(p.time_of_last_orders),
        b(p.will_help),
        b(p.keep_moving),
        b(p.use_goal_plan),
        p.main_goal.action,
        p.main_goal.object.0,
        p.core_scan_target.0,
        b(p.queue_executing),
        p.queue_index,
        p.attack_orders.attack_type,
        p.attack_orders.target.0,
        b(p.attack_orders.pursue),
        f(p.attack_radius),
    ));
    line
}

/// `PILOT_ALARM_NEW_MOVER` (warrior.h:307).
pub const PILOT_ALARM_NEW_MOVER: u8 = 18;

/// CROSS-LANE (lane E -> D): `pilot->triggerAlarm(alarm, param)` for the pilot flying `mover` (warrior.cpp:4558). `Mission::addMover`
/// raises `PILOT_ALARM_NEW_MOVER` with the new mover's watch id on every existing not-disabled mover.
pub fn trigger_alarm(w: &mut crate::World, mover: ObjectHandle, alarm: u8, param: i32) {
    let Some(obj) = w.objects.get(mover) else {
        return;
    };
    let slot = match &obj.kind {
        crate::object::Kind::Mech(m) => m.m.pilot_slot,
        crate::object::Kind::Vehicle(v) => v.m.pilot_slot,
        _ => return,
    };
    let Some(pilot) = w.pilots.get_mut(slot) else {
        return;
    };
    let Some(queue) = pilot.alarms.get_mut(alarm as usize) else {
        return;
    };
    let Some(history) = pilot.alarm_history.get_mut(alarm as usize) else {
        return;
    };
    if queue.len() == crate::ai::pilot::MAX_ALARM_TRIGGERS
        || history.len() == crate::ai::pilot::MAX_ALARM_TRIGGERS
    {
        return;
    }
    queue.push(param);
    history.push(param);
}

#[cfg(test)]
mod contact_tests {
    use super::*;

    #[test]
    fn contact_query_excludes_removed_movers() {
        let mut world = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        let looker = ObjectHandle(1);
        let target = ObjectHandle(2);
        world.objects.list.resize(3, None);
        for handle in [looker, target] {
            let mut common =
                crate::object::Common::new(crate::object::ObjectClass::BattleMech, handle);
            common.flags |= crate::object::flags::EXISTS | crate::object::flags::AWAKE;
            world.objects.list[handle.0 as usize] = Some(crate::object::Obj {
                c: common,
                kind: crate::object::Kind::Mech(Box::default()),
            });
        }
        world.objects.get_mut(target).unwrap().c.flags |= crate::object::flags::REMOVED;
        world.sensors.pool.push(crate::ai::sensors::Sensor {
            owner: looker,
            team_index: 0,
            ..crate::ai::sensors::Sensor::default()
        });
        world.sensors.team_contact_order[0].push(target);
        world
            .sensors
            .team_contact_status
            .insert(target, [1; crate::ids::MAX_TEAMS]);

        assert!(contacts(&world, looker, 0, ContactSort::None).is_empty());
    }

    #[test]
    fn contact_distance_sort_ignores_vertical_difference() {
        let mut world = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        let looker = ObjectHandle(1);
        let elevated_near = ObjectHandle(2);
        let level_far = ObjectHandle(3);
        world.objects.list.resize(4, None);
        for handle in [looker, elevated_near, level_far] {
            let mut common =
                crate::object::Common::new(crate::object::ObjectClass::BattleMech, handle);
            common.flags |= crate::object::flags::EXISTS | crate::object::flags::AWAKE;
            world.objects.list[handle.0 as usize] = Some(crate::object::Obj {
                c: common,
                kind: crate::object::Kind::Mech(Box::default()),
            });
        }
        world.objects.get_mut(elevated_near).unwrap().c.position = crate::math::Vec3 {
            x: 1.0,
            y: 0.0,
            z: 1000.0,
        };
        world.objects.get_mut(level_far).unwrap().c.position = crate::math::Vec3 {
            x: 2.0,
            y: 0.0,
            z: 0.0,
        };
        world.sensors.pool.push(crate::ai::sensors::Sensor {
            id: 0,
            owner: looker,
            team_index: 0,
            ..crate::ai::sensors::Sensor::default()
        });
        world.sensors.teams[0].push(0);
        world.sensors.team_contact_order[0] = vec![level_far, elevated_near];
        world
            .sensors
            .team_contact_status
            .insert(level_far, [1; crate::ids::MAX_TEAMS]);
        world
            .sensors
            .team_contact_status
            .insert(elevated_near, [1; crate::ids::MAX_TEAMS]);

        assert_eq!(
            contacts(&world, looker, 0, ContactSort::Distance),
            [elevated_near, level_far]
        ); // GameObject::distanceFrom, gameobj.cpp:1762-1768
    }
}
