// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE B - movers: movement and physics of mechs and ground vehicles.
//!
//! Partial port of `code/mover.{h,cpp}` (path manager, `initOptimalCells`, 61x61 attack goal scoring, path locks, path searches
//! and scoped crash-decision deferral) and the `code/mech.cpp`/`gvehicl.cpp` mover update entry points.
//! Mech jumping, piloting checks and knockdown recovery are implemented alongside the scoped grounded motion.
//! Remaining weapons/damage code inside those files belongs to lane C ([`crate::combat`]); temporary lane-D pilot glue is in [`pilot_standin`].
//!
//! There is no `Mover::update` in the C++; the two entry points are [`update_mech`] and [`update_vehicle`], called by
//! lane A's [`ObjectStore::update`](crate::object::ObjectStore::update) in `mechs[]` then `vehicles[]` order. The entry
//! points currently perform only the partial work documented at each function.

pub mod digest;
pub mod manager;
pub mod mech;
pub mod planning;
pub mod refit;
pub mod support;
pub mod vehicle;

pub use mech::{crash_avoidance, update_path_lock};

pub use manager::PathManager;

use crate::ids::{CommanderId, ObjectHandle, TeamId};
use crate::math::Vec3;

/// `MAX_LOCK_RANGE`.
pub const MAX_LOCK_RANGE: usize = 10;
/// `MAX_ATTACK_CELLRANGE` (`mover.h:592`).
pub const MAX_ATTACK_CELLRANGE: usize = 30;
/// `MAX_ATTACK_INCREMENTS` (`mover.h:593`).
pub const MAX_ATTACK_INCREMENTS: usize = 32;
/// Side of the square table used to group cells by range.
pub const RANGED_CELLS_SIDE: usize = MAX_ATTACK_CELLRANGE * 2 + 1;
/// Capacity of the range-sorted cell offset table.
pub const RANGED_CELLS_DIM: usize = RANGED_CELLS_SIDE * RANGED_CELLS_SIDE;

/// `ControlGui::setRange` (`controlgui.cpp:1404-1417`) and the current-order portion of
/// `toggleHoldPosition` (`controlgui.cpp:2949-2955`).
pub fn set_attack_range(
    world: &mut crate::World,
    mover: crate::ids::ObjectHandle,
    range: i32,
    update_current_order: bool,
    fire_from_current_position: bool,
) {
    let Some(slot) = crate::ai::warrior::slot_of(world, mover) else {
        return;
    };
    let Some(core) = crate::ai::warrior::core_mut(world, slot) else {
        return;
    };
    core.attack_range = range;
    if !update_current_order {
        return;
    }
    let order = {
        let Some(pilot) = world.pilots.get_mut(slot) else {
            return;
        };
        if pilot.cur_order.code == crate::command::OrderCode::None {
            return;
        }
        pilot.cur_order.attack_params.range = range;
        pilot.cur_order.attack_params.pursue =
            range != crate::command::FIRERANGE_CURRENT && !fire_from_current_position;
        pilot.cur_order.clone()
    };
    crate::ai::handle_tactical_order(world, mover, order, 0, false);
}

/// `MoveType` / movement level: 0 ground, 1 hover, 2 air.
pub type MoveLevel = u8;

/// Fields of `Mover` (code/mover.h) that are not weapons/damage state. Armor, body locations, inventory and
/// criticals live in [`crate::combat::Loadout`] (lane C) and are reached through the owning object.
#[derive(Clone, Debug, Default)]
pub struct MoverCore {
    /// `positionNormal`.
    pub position_normal: Vec3,
    /// `velocity`: displacement per frame in world units (NOT m/s).
    pub velocity: Vec3,
    /// `moveLevel`.
    pub move_level: MoveLevel,
    /// `maxMoveSpeed`.
    pub max_move_speed: f32,
    /// `teamId`, `commanderId`, `groupId`, `squadId`.
    pub team: TeamId,
    /// See `team`.
    pub commander: CommanderId,
    /// `groupId`.
    pub group_id: i8,
    /// `squadId`.
    pub squad_id: i32,
    /// `selectionIndex` (>0 when part of the selected group; staggers delayed orders).
    pub selection_index: i32,
    /// `teamRosterIndex`.
    pub team_roster_index: i32,
    /// `attackRange` (`FireRangeType` or cells).
    pub attack_range: i32,
    /// `lastValidPosition`.
    pub last_valid_position: Vec3,
    /// `pivotDirection` (initialised to 0 in C++, not -1).
    pub pivot_direction: i8,
    /// `crashAvoidSelf`, `crashAvoidPath`, `crashBlockSelf`, `crashBlockPath`, `crashYieldTime`.
    pub crash: CrashState,
    /// `pathLockList`, `pathLockLength`.
    pub path_lock: PathLock,
    /// `pathLocks` (infantry-like movers do not lock).
    pub path_locks_enabled: bool,
    /// `yieldTimeLeft`.
    pub yield_time_left: f32,
    /// `markDistanceMoved`.
    pub mark_distance_moved: f32,
    /// `moveCenter`, `moveRadius` (leash used by some orders).
    pub move_center: Vec3,
    /// See `move_center`.
    pub move_radius: f32,
    /// `followRoads` (`Mover::init` false; `GroundVehicle::init` sets it): adds `MOVEPARAM_FOLLOW_ROADS` to every path search.
    pub follow_roads: bool,
    /// `exploding`, `withdrawing`, `salvageVehicle`, `isOnGui`, `startDisabled`.
    pub flags: MoverFlags,
    /// `creationTime`.
    pub creation_time: f32,
    /// `timeLeft` (time before `update` returns false after destruction).
    pub time_left: f32,
    /// `timeSinceMoving`, `timeSinceFiredLast`, `lastHustleTime`.
    pub timers: [f32; 3],
    /// Handle of the pilot slot in `World::pilots` (lane D).
    pub pilot_slot: u16,
    /// `lowestWeaponNodeID` / `lowestWeaponNodeZ` cache used by `Mover::getLOSPosition` (mover.cpp:2205, 2223, 3451-3509).
    /// `None` means Rust has not observed a source-equivalent weapon-node height yet.
    pub cached_los_weapon_node: Option<(i32, f32)>,
    /// `lastMapCell[2]` (row, col): the cell this mover marked as occupied in the move map, `(-1, -1)` when none (`Mover::init`).
    /// Appended by lane E (`Mover::setPosition` at load marks the start cell); lane B releases it when the mover leaves the cell.
    pub last_map_cell: (i32, i32),
    /// `teleportPosition` debug feature (kept: the oracle never uses it).
    pub teleport: Option<Vec3>,
    /// `suppressionFire` (mover.cpp:2236 zero; set by the GUI, zeroed by `MechWarrior::clearCurTacOrder`, warrior.cpp:4406).
    pub suppression_fire: i32,
    /// `refitBuddyWID` (mover.h): the repair partner of a refit in progress (0 = none).
    pub refit_buddy: crate::ids::WatchId,
    /// `recoverBuddyWID` (mover.h): the recovery partner of a recover in progress (0 = none).
    pub recover_buddy: crate::ids::WatchId,
}

/// Misc boolean fields of `Mover`.
#[derive(Clone, Copy, Debug, Default)]
pub struct MoverFlags {
    /// `exploding`.
    pub exploding: bool,
    /// `withdrawing`.
    pub withdrawing: bool,
    /// `salvageVehicle`.
    pub salvage_vehicle: bool,
    /// `isOnGui`.
    pub on_gui: bool,
    /// `startDisabled`.
    pub start_disabled: bool,
    /// `shutDownThisFrame`, `startUpThisFrame`, `disableThisFrame`.
    pub shut_down_this_frame: bool,
    /// See above.
    pub start_up_this_frame: bool,
    /// See above.
    pub disable_this_frame: bool,
}

/// Crash-avoidance parameters/state (`crashAvoid*`, `crashBlock*`, `crashYieldTime`).
#[derive(Clone, Copy, Debug, Default)]
pub struct CrashState {
    /// `crashAvoidSelf`.
    pub avoid_self: i32,
    /// `crashAvoidPath`.
    pub avoid_path: i32,
    /// `crashBlockSelf`.
    pub block_self: i32,
    /// `crashBlockPath`.
    pub block_path: i32,
    /// `crashYieldTime`.
    pub yield_time: f32,
}

/// `pathLockList[MAX_LOCK_RANGE][2]` + `pathLockLength`.
#[derive(Clone, Copy, Debug, Default)]
pub struct PathLock {
    /// Locked cells (row, col).
    pub cells: [(i32, i32); MAX_LOCK_RANGE],
    /// Number of valid entries.
    pub len: usize,
}

/// `MoverControl::settings` (a C union: the vehicle throttle aliases the mech throttle).
#[derive(Clone, Copy, Debug, Default)]
pub struct Control {
    /// `throttle` (a C `char`: assignments truncate toward zero and wrap).
    pub throttle: i8,
    /// `rotate`.
    pub rotate: f32,
    /// `rotateTorso` / `rotateTurret`.
    pub rotate_torso: f32,
    /// `facingRotate`.
    pub facing_rotate: f32,
    /// `rotateLeftArm`, `rotateRightArm`.
    pub rotate_arms: (f32, f32),
    /// `gestureGoal`.
    pub gesture_goal: i8,
    /// `blowLeftArm`, `blowRightArm`, `pivot`, `isWalking`.
    pub blow_left_arm: bool,
    /// See above.
    pub blow_right_arm: bool,
    /// See above.
    pub pivot: bool,
    /// See above.
    pub is_walking: bool,
    /// CROSS-LANE (lane E -> B): `MoverControl::type`, set by `setControl` at spawn (mover.h:281: `CONTROL_BASE` 0, `CONTROL_PLAYER` 1,
    /// `CONTROL_AI` 2, `CONTROL_NET` 3). `MoverControl::update` dispatches on it.
    pub control_type: u8,
    /// CROSS-LANE (lane E -> B): `MoverControl::dataType` (`CONTROL_DATA_MECH` 1, `CONTROL_DATA_GROUNDVEHICLE` 2).
    pub data_type: u8,
}

/// Post-decision inputs and resulting-state fields recorded by `oracle --mover-trace`.
/// This seam lets the replay test exercise the Rust movement integrators without porting pilot
/// decision-making a second time (lane D owns that code).
#[derive(Clone, Debug, Default)]
pub struct MoverReplayInput {
    /// Numeric `TacticalOrder::code` from the oracle.
    pub order: i32,
    /// `TacticalOrder::stage`.
    pub order_stage: i8,
    /// `MoveOrders` selection fields.
    pub goal_type: i64,
    pub path_type: i64,
    /// `moveState` / `moveStateGoal`.
    pub move_state: i32,
    pub move_state_goal: i32,
    /// `MoveOrders::run`.
    pub run: bool,
    /// `speedState` / `speedThrottle`.
    pub speed_state: i32,
    pub speed_throttle: i32,
    /// Current MoveOrders goal.
    pub goal: Vec3,
    /// Current `MovePath` step destination; `None` means no active step.
    /// Complete path snapshot recorded by the oracle after the pilot decision.
    pub path_steps: Vec<(Vec3, u8)>,
    /// Active path cursor from the same snapshot.
    pub path_cursor: usize,
    /// C++ `numStepsWhenNotPaused`, retained even when the active count is zero.
    pub path_unpaused_steps: usize,
    /// C++ mover tonnage used to select the vehicle turn-rate table entry.
    pub tonnage: f32,
    /// `Mover::marginOfError[0..2]` used by this frame's waypoint advancement.
    pub waypoint_margins: [f32; 2],
    /// `control.settings.mech/groundVehicle.throttle`.
    pub throttle: i8,
    /// Per-frame body rotation selected by `updateMovement`.
    pub rotate: f32,
    /// Mech `facingRotate` (vehicle value is zero).
    pub facing_rotate: f32,
    /// Mech `rotateTorso` / vehicle `rotateTurret`.
    pub turret_or_torso_rotate: f32,
    /// Mech arm rotation inputs.
    pub arm_rotate: (f32, f32),
    /// Vehicle `accel`.
    pub acceleration: f32,
    /// Mech appearance state used to derive `speed`.
    pub gesture: i32,
    /// See `gesture`.
    pub gesture_frame: f32,
    /// Whether the mech is airborne (jump replay is outside this slice).
    pub airborne: bool,
    /// Vehicle throttle mode.
    pub walking: bool,
    /// Vehicle dynamics maximum speed from the loaded mover type.
    pub max_move_speed: f32,
    /// Frame length recorded by the C++ oracle.
    pub frame_length: f32,
}

/// A production decision boundary that cannot yet claim a source-faithful answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoverDecisionUnavailable {
    /// `calcMoveGoal` cannot resolve the cached line-of-sight weapon height it needs (no source-faithful fallback).
    AttackGoalScoring,
    /// A jump order reached a vehicle; shipped jump orders are mech-only.
    JumpMovement,
    /// `BattleMech::crashAvoidanceSystem` / `GroundVehicle::crashAvoidanceSystem` are deferred.
    CrashAvoidance,
    /// A shut down mover the home team can see is powered up and put on the GUI (mech.cpp:5849); not ported.
    PowerTransition,
    /// A mover steps on a mine (`BattleMech::mineCheck`, `GroundVehicle::mineCheck`); the explosion and its damage are not ported.
    MineExplosion,
    /// A mine sweeper or mine layer vehicle (`MineSweepThrottle`, `MineLayThrottle`, laying mines) is not ported.
    MineVehicle,
    /// A vehicle's appearance-specific destruction effect is not yet delivered to the FX sink.
    VehicleDestructionEffect,
    /// `Mover::calcMoveGoal` for a move toward an object (goal scoring with a target but no attack order) is not ported.
    ObjectGoalScoring,
}

/// `LegStatus` of a mech (`calcLegStatus`, mech.cpp:3446).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegStatus(pub u8);

/// `BattleMech` state beyond [`MoverCore`] (code/mech.h:360-415).
#[derive(Clone, Debug, Default)]
pub struct MechState {
    /// Shared mover fields.
    pub m: MoverCore,
    /// `control.settings.mech`.
    pub control: Control,
    /// CROSS-LANE (lane E -> C): `BattleMech::variantName` (a custom logistics mech gets its designer's name, mission.cpp:1428).
    pub variant_name: String,
    /// CROSS-LANE (lane E -> C): `BattleMech::variantID`, the variant actually loaded.
    pub variant_id: u32,
    /// CROSS-LANE (lane E -> B/C): `BattleMech::captureable`, assigned from the part or `MoverInitData` at spawn (mission.cpp:1055, 1296).
    pub captureable: bool,
    /// `legStatus`.
    pub leg_status: LegStatus,
    /// `torsoRotation`, `leftArmRotation`, `rightArmRotation` (degrees).
    pub torso_rotation: f32,
    /// `dynamics.max.mech.torsoYaw` loaded from the chassis CSV.
    pub max_torso_yaw: f32,
    /// `dynamics.max.mech.torsoYawRate` loaded from the chassis CSV (degrees per second).
    pub max_torso_yaw_rate: f32,
    /// See `torso_rotation`.
    pub left_arm_rotation: f32,
    /// See `torso_rotation`.
    pub right_arm_rotation: f32,
    /// `inJump`, `jumpGoal`, `lastJumpTime`, `numJumpJets`.
    pub in_jump: bool,
    /// See `in_jump`.
    pub jump_goal: Vec3,
    /// See `in_jump`.
    pub last_jump_time: f32,
    /// See `in_jump`.
    pub num_jump_jets: u8,
    /// `fallen`, `failedPilotingCheck`, `hitFromFrontThisFrame`, `hitFromBehindThisFrame`,
    /// `leftArmBlownThisFrame`, `rightArmBlownThisFrame`, `sentCrippledMsg`.
    pub fallen: bool,
    /// `mechRemoved`: the post-explosion destroyed-appearance handoff has occurred.
    pub mech_removed: bool,
    /// See above.
    pub failed_piloting_check: bool,
    /// See above.
    pub hit_from_front: bool,
    /// See above.
    pub hit_from_behind: bool,
    /// See above.
    pub left_arm_blown: bool,
    /// See above.
    pub right_arm_blown: bool,
    /// See above.
    pub sent_crippled_msg: bool,
    /// `pivotTurnRate`.
    pub pivot_turn_rate: f32,
    /// `engineBlowTime`.
    pub engine_blow_time: f32,
    /// `centerTorsoBlowTime`.
    pub center_torso_blow_time: f32,
    /// Loadout (armor, internal structure, inventory, criticals): lane C.
    pub loadout: crate::combat::Loadout,
}

/// `GroundVehicle` state beyond [`MoverCore`] (code/gvehicl.h).
#[derive(Clone, Debug, Default)]
pub struct VehicleState {
    /// Shared mover fields.
    pub m: MoverCore,
    /// `control.settings.groundVehicle`.
    pub control: Control,
    /// `velocityMag` (m/s along the heading; ramped by `accel * frameLength`).
    pub velocity_mag: f32,
    /// `accel` from `GroundVehicle::init` (overwritten as `maxVelocity * 5`).
    pub acceleration: f32,
    /// `turretRotation`, `turretYaw`.
    pub turret_rotation: f32,
    /// `isVehiclePilot` (infantry-like spawn removed when out of view).
    pub is_vehicle_pilot: bool,
    /// Mission turn + 1 on which `createVehiclePilot` made this crew (0 for everything else). Not in the C++: the app binds a model
    /// (and so a view answer) only after the creation frame, so that frame's "not in view" is not yet an answer (DIFFERENCES.md).
    pub vehicle_pilot_turn: i64,
    /// `canRecover` (`GroundVehicle::isRecover`, set from recover points at init).
    pub can_recover: bool,
    /// `refitting` (gvehicl.h): the vehicle is repairing its buddy.
    pub refitting: bool,
    /// Repair-truck animation gesture (`GVAppearance::getCurrentGestureId`, gestures 3/4/5; gvehicl.cpp:3065-3150).
    pub refit_gesture: i8,
    /// Current animation frame for the repair-truck gesture, advanced at the loaded animation frame rate.
    pub refit_gesture_frame: f32,
    /// `recovering` (gvehicl.h).
    pub recovering: bool,
    /// `sweepTime` (-1 at init, gvehicl.cpp:876): seconds since the last mine this sweeper set off.
    pub sweep_time: f32,
    /// `timeInCurrent` (gvehicl.h:263): seconds since a mine layer left the cell it mined last.
    pub time_in_current: f32,
    /// `cellRowToMine`, `cellColToMine` (-1 at init, gvehicl.cpp:880): the cell a mine layer mined last.
    pub cell_to_mine: (i32, i32),
    /// Loadout: lane C.
    pub loadout: crate::combat::Loadout,
}

/// Process-wide scratch shared by every mover during one update (`NewRotation`, `DistanceToWaypoint` are file-scope floats in
/// mech.cpp: if a mover returns early they keep the previous mover's value, so this is faithful shared state, not a bug).
#[derive(Clone, Debug, Default)]
pub struct MoveScratch {
    /// `NewRotation`.
    pub new_rotation: f32,
    /// `DistanceToWaypoint`.
    pub distance_to_waypoint: f32,
    /// Debug/test observation of the last `calcMoveGoal` selected cell; never an input.
    pub last_goal_cell: Option<(i32, i32)>,
    /// Per-world copy of `Mover::optimalCells` and its companion range-sorted cells.
    /// C++ stores these as statics; keeping them in World preserves the no-globals contract.
    pub optimal_cells: OptimalCellTables,
}

/// Tables filled by `Mover::initOptimalCells` (mover.cpp:6961). The pair fields are `(row, col)`.
#[derive(Clone, Debug)]
pub struct OptimalCellTables {
    pub optimal: [[[i8; 2]; MAX_ATTACK_INCREMENTS]; MAX_ATTACK_CELLRANGE],
    pub num_increments: usize,
    pub range_indices: [[i16; 2]; MAX_ATTACK_CELLRANGE],
    pub ranged: [[i8; 2]; RANGED_CELLS_DIM],
    pub ranged_len: usize,
}

impl Default for OptimalCellTables {
    fn default() -> Self {
        Self {
            optimal: [[[0; 2]; MAX_ATTACK_INCREMENTS]; MAX_ATTACK_CELLRANGE],
            num_increments: 0,
            range_indices: [[0; 2]; MAX_ATTACK_CELLRANGE],
            ranged: [[0; 2]; RANGED_CELLS_DIM],
            ranged_len: 0,
        }
    }
}

/// `BattleMech::update()` (mech.cpp:5778-6410), the parts that change simulation state: the movement chain
/// ([`mech`]), the position and orientation integration, the path locks and the appearance. Remaining unavailable branches include
/// vehicle jump orders, mine actions, and selected goal-scoring cases.
pub fn update_mech(
    w: &mut crate::World,
    h: ObjectHandle,
) -> Result<bool, MoverDecisionUnavailable> {
    let Some(obj) = w.objects.get(h) else {
        return Ok(false);
    };
    let (part, position, rotation, status, slot, move_level) = match &obj.kind {
        crate::object::Kind::Mech(s) => (
            obj.c.part_id,
            obj.c.position,
            obj.c.rotation,
            obj.c.status,
            s.m.pilot_slot,
            s.m.move_level,
        ),
        _ => return Ok(false),
    };
    // positionNormal = land->getTerrainNormal(position)
    let normal = w.terrain.normal(&position);
    if let Some(obj) = w.objects.get_mut(h) {
        if let crate::object::Kind::Mech(s) = &mut obj.kind {
            s.m.position_normal = normal;
        }
        if obj.c.is_disabled() {
            obj.c.set_tangible(false);
        }
    }
    mech::blow_arms(w, h);
    // A disabled air mover plays its one-shot death sample at the transition position. The transition itself (DESTROYED,
    // timeLeft 0) happens at the end of the disabled branch, after that frame's slide (mech.cpp:6091-6159), in
    // `update_incapacitated_mech`; flipping the status here ran the destroyed branch one frame early (mc2_16 `ai` seed 2,
    // hostile 707 at frame 671).
    if status == crate::object::ObjectStatus::Disabled && move_level == 2 {
        if let Some(at) = w.objects.get(h).map(|obj| obj.c.position) {
            let source = w.objects.watch_id(h);
            crate::events::emit_sound_at(
                w,
                source,
                at,
                crate::events::sfx::HELICOPTER_DEATH,
                false,
            );
        }
    }
    let live = status != crate::object::ObjectStatus::Disabled
        && status != crate::object::ObjectStatus::Destroyed;
    if live {
        mech::update_live_mech(w, h, slot)?;
    } else {
        if status == crate::object::ObjectStatus::Disabled {
            mech::update_path_lock(w, h, false);
        }
        mech::update_incapacitated_mech(w, h);
    }
    let (position, rotation, status) = w
        .objects
        .get(h)
        .map_or((position, rotation, status), |obj| {
            (obj.c.position, obj.c.rotation, obj.c.status)
        });

    // Camera visibility and LOD are simulation inputs, and the mech appearance update is part of
    // the simulation because its gesture state and node matrices drive future movement/combat.
    let in_view = w.visibility.mover_in_view(part);
    let lod = w.visibility.mover_lod(part);
    w.appearance.recalc_bounds(h, in_view, lod);
    if live {
        mech::update_appearance_flags(w, h, slot);
    }
    w.appearance.set_object_parameters(h, position, rotation);
    // `BattleMech::updateAnimations` feeds the appearance the torso yaw (mech.cpp:6280); the arm angles stay 0 and are
    // never applied to a node by `Mech3DAppearance`.
    let (torso, removed) = match w.objects.get(h).map(|o| &o.kind) {
        Some(crate::object::Kind::Mech(m)) => (m.torso_rotation, m.mech_removed),
        _ => (0.0, false),
    };
    w.appearance.set_mover_parameters(h, torso, 0.0, 0.0, false);
    // `if (!mechRemoved && !isDestroyed()) appearance->setObjStatus(getStatus())` (mech.cpp:6280-6283): a dead mech's appearance keeps
    // its last live status until the removal hand-over (`update_incapacitated_mech`). Both fields follow it: `status` (an air mover that
    // turns DESTROYED at the end of its disabled branch keeps the status it had; D-52's immediate DESTROYED comes from the kill,
    // combat/damage.rs) and `cpp_status`, which gates the fall roll (a mech destroyed at the kill does not roll until its removal).
    if !removed && status != crate::object::ObjectStatus::Destroyed {
        w.appearance.set_obj_status(h, i32::from(status as u8));
        w.appearance.set_mech_cpp_status(h, i32::from(status as u8));
    }
    crate::appearance::update(w, h, true);
    // Start and stop the water wakes here (mech.cpp:6290-6301).
    let wading = crate::appearance::fx::over_water_cell(w, &position);
    crate::appearance::fx::update_water_wake(w, h, wading);
    Ok(true)
}

/// `GroundVehicle::update` (gvehicl.cpp:3150-3843).
pub fn update_vehicle(
    w: &mut crate::World,
    h: ObjectHandle,
) -> Result<bool, MoverDecisionUnavailable> {
    let Some(obj) = w.objects.get(h) else {
        return Ok(false);
    };
    let (part, position, rotation, status, is_vehicle_pilot, slot, s_born) = match &obj.kind {
        crate::object::Kind::Vehicle(s) => (
            obj.c.part_id,
            obj.c.position,
            obj.c.rotation,
            obj.c.status,
            s.is_vehicle_pilot,
            s.m.pilot_slot,
            s.vehicle_pilot_turn,
        ),
        _ => return Ok(false),
    };
    if w.pilots.get(slot).is_some_and(|pilot| {
        matches!(
            pilot.cur_order.code,
            crate::command::OrderCode::JumpToPoint | crate::command::OrderCode::JumpToObject
        )
    }) {
        return Err(MoverDecisionUnavailable::JumpMovement);
    }
    let normal = w.terrain.normal(&position);
    if let Some(obj) = w.objects.get_mut(h) {
        if let crate::object::Kind::Vehicle(s) = &mut obj.kind {
            s.m.position_normal = normal;
        }
    }
    if status != crate::object::ObjectStatus::Destroyed {
        vehicle::update_live_vehicle(w, h, slot)?;
    } else {
        mech::update_path_lock(w, h, false);
        vehicle::update_destroyed_vehicle(w, h)?;
    }
    let (position, rotation, status) = w
        .objects
        .get(h)
        .map_or((position, rotation, status), |obj| {
            (obj.c.position, obj.c.rotation, obj.c.status)
        });
    let in_view = w.visibility.mover_in_view(part);
    let lod = w.visibility.mover_lod(part);
    if status != crate::object::ObjectStatus::Destroyed {
        vehicle::update_animations(w, h); // gvehicl.cpp:3714
    }
    w.appearance.recalc_bounds(h, in_view, lod);
    w.appearance.set_object_parameters(h, position, rotation);
    w.appearance.set_obj_status(h, i32::from(status as u8));
    // Start and stop the water wakes here, then the activity effects (gvehicl.cpp:3736-3760).
    let wading = crate::appearance::fx::over_water_cell(w, &position);
    crate::appearance::fx::update_water_wake(w, h, wading);
    let mine_laying = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.move_params.mine_laying);
    let refitting = matches!(w.objects.get(h).map(|o| &o.kind), Some(crate::object::Kind::Vehicle(v)) if !v.m.refit_buddy.is_none());
    if mine_laying {
        crate::appearance::fx::set_activity(
            w,
            h,
            Some(crate::events::fx_id::MINELAYER_EFFECT_ID),
            true,
        );
    } else if refitting && w.appearance.current_gesture_id(h) == 4 {
        crate::appearance::fx::set_activity(
            w,
            h,
            Some(crate::events::fx_id::REPAIR_VEHICLE_EFFECT_ID),
            true,
        );
    } else {
        crate::appearance::fx::set_activity(w, h, None, false);
    }
    crate::appearance::update(w, h, true);
    if is_vehicle_pilot {
        // "We are a vehicle pilot. Whack us when we are no longer in view." (gvehicl.cpp:3508). The C++ tests a local `inView` that
        // starts false and is only set inside branches the crew never takes, so the original removes the crew the frame it appears
        // (a logic error; DIFFERENCES.md). Here the crew is removed when the camera no longer sees it.
        if vehicle_pilot_removed(w.cpp_vehicle_pilot_d106, in_view, w.clock.turn, s_born) {
            if let Some(obj) = w.objects.get_mut(h) {
                obj.set_exists(false);
            }
        }
        // "Check if I have nothing to do. If not, move 'em out!" (gvehicl.cpp:3512-3517): run to the map origin again.
        let idle = w
            .pilots
            .get(slot)
            .is_some_and(|p| p.cur_order.code == crate::command::OrderCode::None);
        if idle && w.objects.get(h).is_some_and(|o| o.exists()) {
            crate::ai::orders::order_move_to_point(
                w,
                slot,
                false,
                true,
                crate::command::OrderOrigin::Player,
                Vec3::default(),
                -1,
                crate::ai::orders::param::RUN,
            );
        }
    }
    Ok(true)
}

/// Whether the vehicle-crew bail-out unit is removed this update (gvehicl.cpp:3508). The Rust keeps the crew while the camera sees it
/// and always spares its creation frame (`s_born` = creation turn + 1). With the world's test-only `D-106` switch the C++ logic error
/// applies: the local `inView` is always false, so the crew goes at once.
fn vehicle_pilot_removed(cpp_d106: bool, in_view: bool, turn: i64, s_born: i64) -> bool {
    cpp_d106 || (!in_view && turn + 1 != s_born)
}

/// `BattleMech::updateMovement` / `GroundVehicle::updateMovement`, limited to the
/// grounded forward walk/run/turn cases exercised by the lane-B replay scenarios.
/// Grounded forward movement and jump setup are supported; pivot transitions remain explicitly unavailable.
fn update_movement_controls(w: &mut crate::World, h: ObjectHandle) {
    let Some((position, rotation, is_mech, tonnage, move_state, move_goal, run, waypoint)) =
        w.objects.get(h).and_then(|obj| {
            let (m, c) = match &obj.kind {
                crate::object::Kind::Mech(m) => (&m.m, &m.control),
                crate::object::Kind::Vehicle(v) => (&v.m, &v.control),
                _ => return None,
            };
            let pilot = w.pilots.get(m.pilot_slot)?;
            let path = &pilot.lane_b_move.paths[0];
            let waypoint = path.steps.get(path.cur_step).map(|s| s.destination);
            let _ = c;
            Some((
                obj.c.position,
                obj.c.rotation,
                matches!(obj.kind, crate::object::Kind::Mech(_)),
                obj.c.tonnage,
                pilot.lane_b_move.move_state,
                pilot.lane_b_move.move_state_goal,
                pilot.lane_b_move.run,
                waypoint,
            ))
        })
    else {
        return;
    };
    let Some(target) = waypoint else {
        if !is_mech {
            if let Some(obj) = w.objects.get_mut(h) {
                if let crate::object::Kind::Vehicle(v) = &mut obj.kind {
                    v.control.throttle = 0;
                    v.control.rotate = 0.0;
                    v.control.is_walking = !run;
                }
            }
        }
        return;
    };
    // The slice scenarios use MOVESTATE_FORWARD (1) and don't exercise the state
    // transitions that pause or reverse a path.
    if move_state != 1 || move_goal != 1 {
        return;
    }
    let forward = crate::math::rotation_vector(rotation);
    let mut toward = crate::math::Vec3 {
        x: target.x - position.x,
        y: target.y - position.y,
        z: 0.0,
    };
    toward.z = 0.0;
    let dot = forward.x * toward.x + forward.y * toward.y;
    let cross_z = forward.x * toward.y - forward.y * toward.x;
    // Mover::relFacingTo -> angle_from computes each vector length separately,
    // then uses my_acos(float) * RADS_TO_DEGREES (mover.cpp:2905;
    // mathfunc.cpp:115). Keep the multiplication order explicit.
    let forward_length = (forward.x * forward.x + forward.y * forward.y).sqrt();
    let toward_length = (toward.x * toward.x + toward.y * toward.y).sqrt();
    let length = forward_length * toward_length;
    let mut relative = if length == 0.0 {
        0.0
    } else {
        crate::math::my_acos(dot / length) * crate::math::RADS_TO_DEGREES
    };
    if cross_z >= 0.0 {
        relative = -relative;
    }
    let frame = w.clock.frame_length;
    let rate = if is_mech {
        720.0
    } else if tonnage < 10.0 {
        360.0
    } else if tonnage < 60.0 {
        240.0
    } else if tonnage < 70.0 {
        210.0
    } else if tonnage < 80.0 {
        140.0
    } else if tonnage < 90.0 {
        120.0
    } else {
        90.0
    };
    let mut rotate = 0.0;
    let mut throttle;
    if relative < -5.0 || relative > 5.0 {
        rotate = -relative;
        let max_rate = rate * frame;
        if rotate.abs() > max_rate {
            throttle = if rotate.abs() < 50.0 {
                (100.0 - rotate.abs()) as i8
            } else {
                0
            };
            rotate = if rotate > 0.0 { max_rate } else { -max_rate };
        } else {
            throttle = 100;
        }
    } else {
        throttle = 100;
    }
    if is_mech && !run && throttle < 35 {
        throttle = 35;
    }
    if let Some(obj) = w.objects.get_mut(h) {
        match &mut obj.kind {
            crate::object::Kind::Mech(m) => {
                m.control.rotate = rotate;
                m.control.throttle = throttle;
                m.control.is_walking = !run;
            }
            crate::object::Kind::Vehicle(v) => {
                v.control.rotate = rotate;
                v.control.throttle = throttle;
                v.control.is_walking = !run;
            }
            _ => {}
        }
    }
}

/// The waypoint/cursor portion of `updateMovePath` used by grounded movement. Path planning
/// itself remains lane D input; this step advances the supplied active path just as C++ does.
fn advance_movement_path(w: &mut crate::World, h: ObjectHandle, margins: Option<[f32; 2]>) -> f32 {
    let Some((slot, position, speed, frame)) = w.objects.get(h).and_then(|obj| {
        let (m, speed) = match &obj.kind {
            crate::object::Kind::Mech(m) => (&m.m, w.appearance.velocity_magnitude(h)),
            crate::object::Kind::Vehicle(v) => (&v.m, v.velocity_mag),
            _ => return None,
        };
        Some((m.pilot_slot, obj.c.position, speed, w.clock.frame_length))
    }) else {
        return 9999.0;
    };
    let Some(pilot) = w.pilots.get_mut(slot) else {
        return 9999.0;
    };
    let path = &mut pilot.lane_b_move.paths[0];
    if path.num_steps == 0 || path.cur_step >= path.num_steps {
        return 9999.0;
    }
    let distance = distance_between_waypoint(path.steps[path.cur_step].destination, position);
    let distance_this_frame = speed * frame;
    let margins = margins.unwrap_or([5.0, 10.0]);
    let mut cushion = if path.cur_step == path.num_steps - 1 {
        margins[1]
    } else {
        margins[0]
    };
    if cushion < distance_this_frame {
        cushion = distance_this_frame;
    }
    if distance < cushion {
        path.cur_step += 1;
        if path.cur_step >= path.num_steps {
            path.num_steps = 0;
            path.steps.clear();
        }
    }
    distance
}

/// Apply the source-ordered ground movement calculation after the pilot decision and before
/// appearance advancement. The appearance supplies mech speed; vehicle speed is ramped by dynamics.
pub(crate) fn integrate_mech_motion(
    w: &mut crate::World,
    h: ObjectHandle,
    emergency: bool,
    distance_override: Option<f32>,
    legacy_dynamics: bool,
) {
    let Some(obj) = w.objects.get(h) else { return };
    let crate::object::Kind::Mech(mech) = &obj.kind else {
        return;
    };
    let position = obj.c.position;
    let rotation = obj.c.rotation;
    let throttle = f32::from(mech.control.throttle);
    let slot = mech.m.pilot_slot;
    let turn_rate = mech.control.rotate;
    let airborne = w.appearance.is_jump_airborne(h);
    let mut speed = w.appearance.velocity_magnitude(h);
    if speed == 0.0 && airborne {
        let velocity = w.appearance.velocity(h);
        speed =
            (velocity.x * velocity.x + velocity.y * velocity.y + velocity.z * velocity.z).sqrt();
    }
    let path = crate::ai::get_move_path(w, slot);
    // `DistanceToWaypoint` is the distance from the current position to the current
    // step only (mech.cpp:4217-4310); `MovePath::distance_left` also adds the
    // remaining route length and lets multi-step movers overshoot this step.
    let distance = distance_override.unwrap_or_else(|| {
        path.as_ref()
            .map_or(9999.0, |p| distance_to_waypoint(p, position))
    });
    let frame = w.clock.frame_length;
    let slope = mc2_libm::cosf(w.terrain.angle_deg(&position).to_radians());
    let displacement = if airborne {
        let mut v = w.appearance.velocity(h);
        v.x *= frame;
        v.y *= frame;
        v.z *= frame;
        v
    } else {
        let mut velocity_magnitude = speed;
        if emergency {
            velocity_magnitude = 0.0;
        }
        velocity_magnitude *= 0.01 * throttle;
        let mut velocity_multiplier = velocity_magnitude * slope * frame;
        if velocity_multiplier > distance {
            velocity_multiplier = distance;
        }
        let amount = velocity_multiplier * 5.01;
        let mut v = crate::math::rotation_vector(rotation);
        v.x *= amount;
        v.y *= amount;
        v.z *= amount;
        v
    };
    let mut next = crate::math::Vec3 {
        x: position.x + displacement.x,
        y: position.y + displacement.y,
        z: position.z + displacement.z,
    };
    // C++ (mech.cpp:5992) pins z to the terrain every frame, jumping or not; the arc is appearance-only.
    next.z = w.terrain.elevation(&next);
    let next_rotation = rotated_angle(rotation, turn_rate);
    if let Some(obj) = w.objects.get_mut(h) {
        if let crate::object::Kind::Mech(mech) = &mut obj.kind {
            mech.m.velocity = displacement;
            // The replay seam applies the recorded torso yaw here; the live update runs `updateTorso` / `updateDynamics` after
            // `setPosition` ([`mech::update_dynamics`]).
            let mut torso_rate = mech.control.rotate_torso;
            if legacy_dynamics && torso_rate != 0.0 {
                let mut torso_rotation = mech.torso_rotation;
                if torso_rotation > mech.max_torso_yaw {
                    torso_rotation = mech.max_torso_yaw;
                    // C++ clamps the existing angle and suppresses this frame's
                    // turn when it starts beyond the chassis limit.
                    torso_rate = 0.0;
                }
                if torso_rotation < -mech.max_torso_yaw {
                    torso_rotation = -mech.max_torso_yaw;
                    torso_rate = 0.0;
                }
                if torso_rotation + torso_rate > mech.max_torso_yaw {
                    torso_rate = mech.max_torso_yaw - torso_rotation;
                }
                if torso_rotation + torso_rate < -mech.max_torso_yaw {
                    torso_rate = -mech.max_torso_yaw - torso_rotation;
                }
                mech.torso_rotation = torso_rotation + torso_rate;
            }
        }
    }
    set_position(w, h, next);
    if legacy_dynamics {
        if let Some(obj) = w.objects.get_mut(h) {
            obj.c.rotation = next_rotation;
        }
    }
}

/// Replay one recorded movement decision through the same Rust movement integrator used by
/// `update_mech` / `update_vehicle`. Pilot decisions, jump setup, piloting checks, and crash
/// avoidance are intentionally outside this seam and remain TODO stand-ins for the next slice.
pub fn replay_mover_movement(w: &mut crate::World, h: ObjectHandle, input: MoverReplayInput) {
    let Some(slot) = w.objects.get(h).and_then(|obj| match &obj.kind {
        crate::object::Kind::Mech(m) => Some(m.m.pilot_slot),
        crate::object::Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    }) else {
        return;
    };
    w.clock.frame_length = input.frame_length;
    if let Some(obj) = w.objects.get_mut(h) {
        obj.c.tonnage = input.tonnage;
    }
    if let Some(pilot) = w.pilots.get_mut(slot) {
        pilot.cur_order.code = order_code(input.order);
        pilot.cur_order.stage = input.order_stage;
        pilot.lane_b_move.goal = input.goal;
        pilot.lane_b_move.has_goal =
            input.goal.x != -999999.0 || input.goal.y != -999999.0 || input.goal.z != -999999.0;
        pilot.lane_b_move.goal_type = input.goal_type;
        pilot.lane_b_move.path_type = input.path_type as i32;
        pilot.lane_b_move.speed_state = input.speed_state;
        pilot.lane_b_move.speed_throttle = input.speed_throttle;
        pilot.lane_b_move.move_state = input.move_state;
        pilot.lane_b_move.move_state_goal = input.move_state_goal;
        pilot.lane_b_move.run = input.run;
        let path = &mut pilot.lane_b_move.paths[0];
        path.init(input.path_steps.len());
        // C++ keeps a stale cursor after `clearMovePath`; it can exceed numSteps.
        path.cur_step = input.path_cursor;
        path.num_steps_when_not_paused = input.path_unpaused_steps;
        for (step, (destination, direction)) in path.steps.iter_mut().zip(&input.path_steps) {
            step.destination = *destination;
            step.direction = *direction;
        }
    }
    let mech = matches!(
        w.objects.get(h).map(|obj| &obj.kind),
        Some(crate::object::Kind::Mech(_))
    );
    if let Some(obj) = w.objects.get_mut(h) {
        match &mut obj.kind {
            crate::object::Kind::Mech(m) => {
                m.control.rotate = 0.0;
                m.control.facing_rotate = 0.0;
                m.control.rotate_torso = input.turret_or_torso_rotate;
                m.control.rotate_arms = input.arm_rotate;
            }
            crate::object::Kind::Vehicle(v) => {
                v.control.rotate = 0.0;
                v.control.rotate_torso = input.turret_or_torso_rotate;
                v.acceleration = input.acceleration;
                v.m.max_move_speed = input.max_move_speed;
            }
            _ => return,
        }
    }
    let distance_to_waypoint = advance_movement_path(w, h, Some(input.waypoint_margins));
    update_movement_controls(w, h);
    if mech {
        if let Some(Some(crate::appearance::Appearance::Mech(appearance))) =
            w.appearance.by_handle.get_mut(h.0 as usize)
        {
            appearance.current_gesture = input.gesture;
            appearance.current_frame = input.gesture_frame;
            appearance.jump_airborne = input.airborne;
        }
        integrate_mech_motion(w, h, false, Some(distance_to_waypoint), true);
    } else {
        integrate_vehicle_motion(w, h, false, Some(distance_to_waypoint), true);
    }
}

fn order_code(value: i32) -> crate::command::OrderCode {
    use crate::command::OrderCode::*;
    match value {
        1 => Wait,
        2 => MoveToPoint,
        3 => MoveToObject,
        4 => JumpToPoint,
        5 => JumpToObject,
        6 => TraversePath,
        7 => PatrolPath,
        8 => Escort,
        9 => Follow,
        10 => Guard,
        11 => Stop,
        12 => PowerUp,
        13 => PowerDown,
        14 => WaypointsDone,
        15 => Eject,
        16 => AttackObject,
        17 => AttackPoint,
        18 => HoldFire,
        19 => Withdraw,
        20 => Scramble,
        21 => Capture,
        22 => Refit,
        23 => GetFixed,
        24 => LoadIntoCarrier,
        25 => DeployElementals,
        26 => Recover,
        _ => None,
    }
}

pub(crate) fn integrate_vehicle_motion(
    w: &mut crate::World,
    h: ObjectHandle,
    emergency: bool,
    distance_override: Option<f32>,
    legacy_dynamics: bool,
) {
    let Some(obj) = w.objects.get(h) else { return };
    let crate::object::Kind::Vehicle(vehicle) = &obj.kind else {
        return;
    };
    let position = obj.c.position;
    let rotation = obj.c.rotation;
    let mut speed = vehicle.velocity_mag;
    let old_speed = vehicle.velocity_mag;
    let acceleration = vehicle.acceleration;
    let throttle = f32::from(vehicle.control.throttle) / 100.0;
    let is_walking = vehicle.control.is_walking;
    let max_speed = vehicle.m.max_move_speed;
    let cell = (obj.c.cell.0 as i32, obj.c.cell.1 as i32);
    let frame = w.clock.frame_length;
    let turn_rate = vehicle.control.rotate;
    let slot = vehicle.m.pilot_slot;
    let move_level = vehicle.m.move_level;
    let path = crate::ai::get_move_path(w, slot);
    let distance = distance_override.unwrap_or_else(|| {
        path.as_ref()
            .map_or(9999.0, |p| distance_to_waypoint(p, position))
    });
    let mut forward = rotation_vector_separate(rotation);
    let mut use_hill_factor = move_level == 0;
    if !use_hill_factor {
        use_hill_factor = !w.terrain.map().cell(cell.0, cell.1).shallow_water()
            && !w.terrain.map().cell(cell.0, cell.1).deep_water();
    }
    if speed > crate::time::STUFF_SMALL && use_hill_factor {
        // gvehicl.cpp:3517-3540: rotate the terrain normal into vehicle space,
        // derive signed pitch from its Y component, then apply sin(pitch).
        let mut pitch_k = w.terrain.normal(&position);
        rotate_xy_separate(&mut pitch_k, rotation);
        pitch_k.x = 0.0;
        let length = ((pitch_k.y * pitch_k.y + pitch_k.z * pitch_k.z) as f64).sqrt() as f32;
        if length != 0.0 {
            let inverse_length = 1.0 / length;
            pitch_k.y *= inverse_length;
            pitch_k.z *= inverse_length;
        }
        // C++'s global `acos`/`sin` calls use the double libm overload, while the
        // stored pitch and degree constants remain `float` (`gvehicl.cpp:3538-3545`).
        let mut pitch =
            (mc2_libm::acos(f64::from(pitch_k.z)) * f64::from(crate::math::RADS_TO_DEGREES)) as f32;
        if pitch_k.y < 0.0 {
            pitch = -pitch;
        }
        let pitch_radians = pitch * crate::math::DEGREES_TO_RADS;
        let pitch = mc2_libm::sin(f64::from(pitch_radians)) as f32;
        if pitch != 0.0 {
            let hill_factor = pitch
                * speed
                * w.data
                    .gamesys
                    .tables
                    .as_ref()
                    .map_or(0.0, |g| g.vehicle.hill_speed_factor);
            speed -= hill_factor;
            if speed < hill_factor {
                speed = 2.0;
            }
        }
    }
    let move_speed = if emergency { 0.0 } else { speed };
    let amount = (move_speed * frame).min(distance.max(0.0)) * 5.01;
    forward.x *= amount;
    forward.y *= amount;
    let mut next = crate::math::Vec3 {
        x: position.x + forward.x,
        y: position.y + forward.y,
        z: position.z,
    };
    next.z = w.terrain.elevation(&next);
    if move_level == 1 && next.z < w.terrain.water_elevation {
        next.z = w.terrain.water_elevation;
    }
    set_position_and_velocity(w, h, next, forward);
    if legacy_dynamics {
        // The replay seam applies the recorded rotation and the velocity ramp here; the live update runs `updateTurret` and
        // `updateDynamics` after `setPosition` ([`vehicle::update_dynamics`]).
        let target = throttle
            * if is_walking {
                w.data
                    .gamesys
                    .tables
                    .as_ref()
                    .map_or(max_speed, |g| g.vehicle.walk_speed)
            } else {
                max_speed
            };
        let mut accel = acceleration;
        let diff = target - old_speed;
        if ((diff < 0.0) && (accel > 0.0)) || ((diff > 0.0) && (accel < 0.0)) {
            accel = -accel;
        }
        let mut change = accel * frame;
        if change.abs() > diff.abs() {
            change = diff;
        }
        let next_speed = old_speed + change;
        if let Some(obj) = w.objects.get_mut(h) {
            if let crate::object::Kind::Vehicle(vehicle) = &mut obj.kind {
                vehicle.velocity_mag = next_speed;
                vehicle.acceleration = accel;
            }
            obj.c.rotation = rotated_angle(rotation, turn_rate);
        }
    }
}

fn set_position_and_velocity(w: &mut crate::World, h: ObjectHandle, next: Vec3, velocity: Vec3) {
    if let Some(obj) = w.objects.get_mut(h) {
        if let crate::object::Kind::Vehicle(vehicle) = &mut obj.kind {
            vehicle.m.velocity = velocity;
        }
    }
    set_position(w, h, next);
}

/// `DistanceToWaypoint` is the distance to the current step, not the remaining route
/// distance (`mech.cpp:4217-4310`, `gvehicl.cpp:3470-3598`).
fn distance_to_waypoint(path: &mc2_path::MovePath, position: crate::math::Vec3) -> f32 {
    let Some(step) = path.steps.get(path.cur_step) else {
        return 9999.0;
    };
    distance_between_waypoint(step.destination, position)
}

fn distance_between_waypoint(waypoint: crate::math::Vec3, position: crate::math::Vec3) -> f32 {
    let dx = position.x - waypoint.x;
    let dy = position.y - waypoint.y;
    (dx * dx + dy * dy).sqrt() * crate::terrain::METERS_PER_WORLD_UNIT
}

/// `Mover::getRotationVector` and vehicle ramp dynamics call `Rotate` from
/// `mclib/mathfunc.cpp:30-46`. The C++ `sin`/`cos` overloads are double precision,
/// with each result narrowed to float before its vector operation.
fn rotate_xy_separate(v: &mut crate::math::Vec3, angle: f32) {
    let tx = v.x;
    let radians = angle * crate::math::DEGREES_TO_RADS;
    let sine = mc2_libm::sin(f64::from(radians)) as f32;
    let cosine = mc2_libm::cos(f64::from(radians)) as f32;
    v.x *= cosine;
    v.x += v.y * sine;
    v.y *= cosine;
    v.y -= tx * sine;
}

fn rotation_vector_separate(rotation: f32) -> crate::math::Vec3 {
    let mut v = crate::math::Vec3 {
        x: 0.0,
        y: -1.0,
        z: 0.0,
    };
    rotate_xy_separate(&mut v, -rotation);
    v
}

/// `BattleMechType::handleCollision` / `GroundVehicleType::handleCollision` (mech.cpp:744-990, gvehicl.cpp:416-534) for a mover that
/// is the collidee. A tree or terrain object deflects a mover lighter than `tonnageThreshold` by
/// `tonnageThreshold / tonnage * treeDeflection` degrees, at most once per two seconds per object (mech.cpp:965-985). Both types
/// return false. Mover-on-mover contact does nothing here: the jump-contact reaction (guard, rotate and bounce, death-from-above
/// hits, `mech.cpp:835-942`) is `mech_handle_collision`. Trains, buildings and the other classes never reach a mover type with
/// a reaction.
pub fn type_handle_collision(
    w: &mut crate::World,
    collidee: ObjectHandle,
    collider: ObjectHandle,
) -> bool {
    use crate::object::{Kind, ObjectClass};
    let Some(class) = w.objects.get(collider).map(|o| o.c.class) else {
        return false;
    };
    match class {
        ObjectClass::Tree | ObjectClass::TerrainObject => {
            let now = w.clock.scenario_time;
            let collider_wid = w.objects.watch_id(collider);
            let turn = w.clock.turn as i32;
            let Some(tables) = w.data.gamesys.tables.clone() else {
                return false;
            };
            let Some(obj) = w.objects.get_mut(collidee) else {
                return false;
            };
            let (threshold, deflection) = match &obj.kind {
                Kind::Mech(_) => (
                    tables.mech.tonnage_collision_threshold,
                    tables.mech.tree_deflection,
                ),
                Kind::Vehicle(_) => (
                    tables.vehicle.tonnage_collision_threshold,
                    tables.vehicle.tree_deflection,
                ),
                _ => return false,
            };
            if obj.c.collision_free_from == collider_wid && obj.c.collision_free_time >= now {
                return false;
            }
            // Don't allow collision between these two again for a period of time...
            obj.c.collision_free_from = collider_wid;
            obj.c.collision_free_time = now + 2.0;
            let audible = obj.c.windows_visible == turn;
            let tonnage = obj.c.tonnage;
            // A tonnage of 0 divides by zero in the C++ (infinite rotation); such a mover is not deflected.
            if tonnage > 0.0 && tonnage < threshold {
                let turn_around = (threshold / tonnage) * deflection;
                if turn_around > 0.0 {
                    obj.c.rotation = rotated_angle(obj.c.rotation, turn_around);
                }
            }
            if audible {
                crate::events::emit_sound(w, collidee, crate::events::sfx::MECH_COLLIDE, false);
            }
            false
        }
        ObjectClass::BattleMech | ObjectClass::GroundVehicle => {
            // `BattleMechType::handleCollision` (mech.cpp:747-950) for a mech collidee, `GroundVehicleType::handleCollision`
            // (gvehicl.cpp:420-495) for a vehicle collidee.
            match w.objects.get(collidee).map(|o| &o.kind) {
                Some(crate::object::Kind::Mech(_)) => mech_handle_collision(w, collidee, collider),
                Some(crate::object::Kind::Vehicle(_)) => {
                    vehicle_handle_collision(w, collidee, collider)
                }
                _ => {}
            }
            false
        }
        _ => false,
    }
}

/// `DFA_BOOM_EFFECT` (mclib/weaponfx.h:15).
const DFA_BOOM_EFFECT: i32 = 47;

/// A mech is in flight or within 0.5 s of landing (mech.cpp:831-845); with `reset` the landing time is spent (set to -1) the way
/// the collidee's check does, so a second contact in the window is no jump contact.
fn mech_jumping(w: &mut crate::World, h: ObjectHandle, reset: bool) -> bool {
    let now = w.clock.scenario_time;
    let Some(crate::object::Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) else {
        return false;
    };
    let mut jumping = m.in_jump;
    if !jumping && m.last_jump_time >= 0.0 {
        jumping = now - m.last_jump_time < 0.5;
        if reset {
            m.last_jump_time = -1.0;
        }
    }
    jumping
}

/// The BattleMech arm of `handleCollision` for a mover-on-mover contact (mech.cpp:820-948): who collides (a jump, a ram, or an
/// enemy never merely brushing), the two-second guard, the 90-degree rotate and bounce of the collidee, and the damage: a
/// death from above hits the jumping collidee (mine table, legs) and the mover it landed on (DFA table) once each with
/// `tonnage / 10 + 0.5` of the collider; a plain ram hits the collidee. A same-team contact does no damage.
fn mech_handle_collision(w: &mut crate::World, collidee: ObjectHandle, collider: ObjectHandle) {
    use crate::combat::{AttackSource, WeaponShotInfo};
    use crate::object::hooks;
    let now = w.clock.scenario_time;
    // The jump statuses: the collidee spends its landing window here, the collider's is only read (C++ order).
    let collider_jumping = mech_jumping(w, collider, false);
    let collidee_jumping = mech_jumping(w, collidee, true);
    let jump_collision = collidee_jumping || collider_jumping;
    let same_team = hooks::is_friendly(w, collidee, collider);
    let collide = if same_team {
        jump_collision
    } else if jump_collision {
        true
    } else {
        let ram_target = |h: ObjectHandle| {
            let slot = match w.objects.get(h).map(|o| &o.kind) {
                Some(crate::object::Kind::Mech(m)) => m.m.pilot_slot,
                Some(crate::object::Kind::Vehicle(v)) => v.m.pilot_slot,
                _ => return None,
            };
            crate::ai::calcpath::ram_object(w, slot)
        };
        ram_target(collidee) == Some(collider) || ram_target(collider) == Some(collidee)
    };
    if !collide {
        return;
    }
    let collider_wid = w.objects.watch_id(collider);
    let Some(obj) = w.objects.get_mut(collidee) else {
        return;
    };
    if obj.c.collision_free_from == collider_wid && obj.c.collision_free_time >= now {
        return;
    }
    // Don't allow collision between these two again for a period of time...
    obj.c.collision_free_from = collider_wid;
    obj.c.collision_free_time = now + 2.0;
    obj.c.rotation = rotated_angle(obj.c.rotation, 90.0);
    let audible = !same_team && obj.c.windows_visible == w.clock.turn as i32;
    let collider_tonnage = w.objects.get(collider).map_or(0.0, |o| o.c.tonnage);
    let damage = collider_tonnage / 10.0 + 0.5;
    let collidee_wid = w.objects.watch_id(collidee);
    let entry = |w: &crate::World, target: ObjectHandle, from: ObjectHandle| {
        let (Some(t), Some(f)) = (w.objects.get(target), w.objects.get(from)) else {
            return 0.0;
        };
        crate::math::rel_facing_to(&t.c.position, t.c.rotation, &f.c.position)
    };
    if jump_collision {
        // Only the jumper's own handler call administers the damage (an enemy only; same-team landings do none).
        if collidee_jumping && !same_team {
            // I jumped on the below, therefore my legs are preferentially damaged.
            let loc = hooks::calc_hit_location(w, collidee, collider, -1, AttackSource::Mine, 0);
            let angle = entry(w, collidee, collider);
            let shot = WeaponShotInfo::init(w, collider_wid, -1, damage, loc, angle);
            hooks::weapon_hit(w, collidee, &shot);
            let loc = hooks::calc_hit_location(w, collider, collidee, -1, AttackSource::Dfa, 0);
            let angle = entry(w, collider, collidee);
            let shot = WeaponShotInfo::init(w, collidee_wid, -1, damage, loc, angle);
            hooks::weapon_hit(w, collider, &shot);
            let position = w.objects.get(collider).map(|o| o.c.position);
            if let Some(position) = position {
                crate::object::ObjectStore::create_explosion(
                    w,
                    DFA_BOOM_EFFECT,
                    None,
                    position,
                    0.0,
                    0.0,
                );
            }
        }
    } else if !same_team {
        let loc = hooks::calc_hit_location(w, collidee, collider, -1, AttackSource::Collision, 0);
        let angle = entry(w, collidee, collider);
        let shot = WeaponShotInfo::init(w, collider_wid, -1, damage, loc, angle);
        // The original hands the same shot to `handleWeaponHit` twice.
        hooks::weapon_hit(w, collidee, &shot);
        hooks::weapon_hit(w, collidee, &shot);
    }
    bounce_to_adjacent_cell(w, collidee);
    if audible {
        if let Some(at) = w.objects.get(collidee).map(|o| o.c.position) {
            crate::events::emit_sound_at(
                w,
                collidee_wid,
                at,
                crate::events::sfx::MECH_COLLIDE,
                false,
            );
        }
    }
}

/// The mover arm of `GroundVehicleType::handleCollision` (gvehicl.cpp:431-490) for a vehicle that is the collidee of a mech or
/// vehicle: nothing between the same team; an enemy collides when either has the other as ram target or when the vehicle has no
/// path locks (something steppable); once per two seconds per collider; a vehicle that can move rotates 90 degrees and bounces
/// to an adjacent cell, then takes one collision hit of `(long)(collider tonnage / 10 + 0.5)`.
fn vehicle_handle_collision(w: &mut crate::World, collidee: ObjectHandle, collider: ObjectHandle) {
    use crate::combat::{AttackSource, WeaponShotInfo};
    use crate::object::hooks;
    let now = w.clock.scenario_time;
    if hooks::is_friendly(w, collidee, collider)
        && w.objects.get(collidee).map(|o| o.commander_id())
            == w.objects.get(collider).map(|o| o.commander_id())
    {
        return;
    }
    let ram_target = |w: &crate::World, h: ObjectHandle| {
        let slot = match w.objects.get(h).map(|o| &o.kind) {
            Some(crate::object::Kind::Mech(m)) => m.m.pilot_slot,
            Some(crate::object::Kind::Vehicle(v)) => v.m.pilot_slot,
            _ => return None,
        };
        crate::ai::calcpath::ram_object(w, slot)
    };
    let (path_locks, can_move) = match w.objects.get(collidee).map(|o| &o.kind) {
        Some(crate::object::Kind::Vehicle(v)) => {
            (v.m.path_locks_enabled, v.m.max_move_speed != 0.0)
        }
        _ => return,
    };
    let rammed =
        ram_target(w, collidee) == Some(collider) || ram_target(w, collider) == Some(collidee);
    if !(rammed || !path_locks) {
        return;
    }
    let collider_wid = w.objects.watch_id(collider);
    let Some(obj) = w.objects.get_mut(collidee) else {
        return;
    };
    if obj.c.collision_free_from == collider_wid && obj.c.collision_free_time >= now {
        return;
    }
    obj.c.collision_free_from = collider_wid;
    obj.c.collision_free_time = now + 2.0;
    let audible = obj.c.windows_visible == w.clock.turn as i32;
    if can_move {
        obj.c.rotation = rotated_angle(obj.c.rotation, 90.0);
        bounce_to_adjacent_cell(w, collidee);
    }
    if audible {
        if let Some(at) = w.objects.get(collider).map(|o| o.c.position) {
            crate::events::emit_sound_at(
                w,
                collider_wid,
                at,
                crate::events::sfx::MECH_COLLIDE,
                false,
            );
        }
    }
    let damage = (w.objects.get(collider).map_or(0.0, |o| o.c.tonnage) / 10.0 + 0.5).floor();
    let loc = hooks::calc_hit_location(w, collidee, collider, -1, AttackSource::Collision, 0);
    let angle = match (w.objects.get(collidee), w.objects.get(collider)) {
        (Some(t), Some(f)) => {
            crate::math::rel_facing_to(&t.c.position, t.c.rotation, &f.c.position)
        }
        _ => 0.0,
    };
    let shot = WeaponShotInfo::init(w, collider_wid, -1, damage, loc, angle);
    hooks::weapon_hit(w, collidee, &shot);
}

/// `Mover::rotate(angle)` (`mover.cpp:2844-2861`): the control value is already a per-frame
/// delta, and the original applies at most one 360-degree wrap.
fn rotated_angle(rotation: f32, angle: f32) -> f32 {
    let mut result = rotation + angle;
    if result < -180.0 {
        result += 360.0;
    } else if result > 180.0 {
        result -= 360.0;
    }
    result
}

/// `Mover::setPosition` (mover.cpp:2760): clamp to the map, update object cell/vertex data, trigger areas and
/// the stationary mover bit. The C++ withdrawing destruction callback is left to the owning object type lane.
pub fn set_position(w: &mut crate::World, h: ObjectHandle, mut position: Vec3) {
    let max_map = w.terrain.world_units_map_side * 0.5;
    let mut off_map = false;
    if position.x < -max_map {
        position.x = -max_map;
        off_map = true;
    }
    if position.x > max_map {
        position.x = max_map;
        off_map = true;
    }
    if position.y < -max_map {
        position.y = -max_map;
        off_map = true;
    }
    if position.y > max_map {
        position.y = max_map;
        off_map = true;
    }
    let max_visual = crate::terrain::WORLD_UNITS_PER_VERTEX
        * w.terrain.blocks_per_side as f32
        * crate::terrain::VERTICES_BLOCK_SIDE as f32
        * 0.5
        - 1300.0;
    if !off_map {
        let clip1 = position.y - position.x;
        let clip2 = position.y + position.x;
        off_map =
            clip1 > max_visual || clip1 < -max_visual || clip2 > max_visual || clip2 < -max_visual;
    }
    if off_map
        && w.objects.get(h).is_some_and(|obj| match &obj.kind {
            crate::object::Kind::Mech(s) => s.m.flags.withdrawing,
            crate::object::Kind::Vehicle(s) => s.m.flags.withdrawing,
            _ => false,
        })
    {
        crate::combat::damage::handle_destruction(w, h, None); // handleDestruction(this, NULL)
    }
    crate::object::set_position(w, h, position, true);
    crate::object::trigger::set_hit(w, h);
    let new_cell = w
        .objects
        .get(h)
        .map(|o| (o.c.cell.0 as i32, o.c.cell.1 as i32));
    let Some(obj) = w.objects.get(h) else { return };
    let (slot, withdrawing) = match &obj.kind {
        crate::object::Kind::Mech(s) => (s.m.pilot_slot, s.m.flags.withdrawing),
        crate::object::Kind::Vehicle(s) => (s.m.pilot_slot, s.m.flags.withdrawing),
        _ => return,
    };
    let path = crate::ai::get_move_path(w, slot);
    let moving = path.as_ref().is_some_and(|p| p.num_steps > 0);
    let last = w.objects.get(h).map(|obj| match &obj.kind {
        crate::object::Kind::Mech(s) => s.m.last_map_cell,
        crate::object::Kind::Vehicle(s) => s.m.last_map_cell,
        _ => (-1, -1),
    });
    if let Some((r, c)) = new_cell {
        if let Some((lr, lc)) = last {
            if lr != -1 && (lr, lc) != (r, c) {
                if w.terrain.in_bounds(lr, lc) {
                    w.terrain.map_mut().cell_mut(lr, lc).set_mover(false);
                }
                if let Some(obj) = w.objects.get_mut(h) {
                    match &mut obj.kind {
                        crate::object::Kind::Mech(s) => s.m.last_map_cell = (-1, -1),
                        crate::object::Kind::Vehicle(s) => s.m.last_map_cell = (-1, -1),
                        _ => {}
                    }
                }
            }
        }
        if !moving && w.terrain.in_bounds(r, c) {
            w.terrain.map_mut().cell_mut(r, c).set_mover(true);
            if let Some(obj) = w.objects.get_mut(h) {
                match &mut obj.kind {
                    crate::object::Kind::Mech(s) => s.m.last_map_cell = (r, c),
                    crate::object::Kind::Vehicle(s) => s.m.last_map_cell = (r, c),
                    _ => {}
                }
            }
        }
    }
    let _ = withdrawing;
}

const GOALMAP_CELL_DIM: usize = 61;
const MAX_MOVE_GOALS: usize = 60;

/// `Mover::calcMoveGoal` simple target/waypoint handling (`mover.cpp:4386-4477`) and its
/// 61x61 attack goal score (`mover.cpp:4480-4790`).
pub fn calc_move_goal(
    w: &mut crate::World,
    h: ObjectHandle,
) -> Result<Option<Vec3>, MoverDecisionUnavailable> {
    calc_move_goal_with_params(w, h, 0)
}

/// `Mover::calcMoveGoal` with the caller's source `moveParams` bits.
pub fn calc_move_goal_with_params(
    w: &mut crate::World,
    h: ObjectHandle,
    move_params: u32,
) -> Result<Option<Vec3>, MoverDecisionUnavailable> {
    calc_move_goal_to(w, h, move_params, None)
}

/// [`calc_move_goal_with_params`] for a caller that has the `moveGoal` argument of `Mover::calcMoveGoal`: the pilot's move goal at the
/// time of the calculation. An attack order scores around it, not around the target's position: `getMovePath()`'s spare-path swap
/// rewrites the pilot's goal to the path's goal after the attack code set it to the target (warrior.cpp:2674, 4380), and the INIT
/// recalculation then starts from the rewritten goal (mc2_01 mover 514, turn 267). `None` is the order's own target position.
pub fn calc_move_goal_to(
    w: &mut crate::World,
    h: ObjectHandle,
    move_params: u32,
    move_goal: Option<Vec3>,
) -> Result<Option<Vec3>, MoverDecisionUnavailable> {
    let Some(obj) = w.objects.get(h) else {
        return Ok(None);
    };
    let (slot, position, center, radius, move_level, team, loadout) = match &obj.kind {
        crate::object::Kind::Mech(s) => (
            s.m.pilot_slot,
            obj.c.position,
            s.m.move_center,
            s.m.move_radius,
            s.m.move_level,
            s.m.team.0,
            s.loadout.clone(),
        ),
        crate::object::Kind::Vehicle(s) => (
            s.m.pilot_slot,
            obj.c.position,
            s.m.move_center,
            s.m.move_radius,
            s.m.move_level,
            s.m.team.0,
            s.loadout.clone(),
        ),
        _ => return Ok(None),
    };
    let Some(pilot) = w.pilots.get(slot).cloned() else {
        return Ok(None);
    };
    let order = pilot.cur_order;
    if pilot.lane_b_move.has_goal
        && !matches!(
            order.code,
            crate::command::OrderCode::AttackObject | crate::command::OrderCode::AttackPoint
        )
    {
        return Ok(Some(pilot.lane_b_move.goal));
    }
    use crate::command::OrderCode;
    match order.code {
        OrderCode::MoveToPoint | OrderCode::TraversePath => Ok(order
            .move_params
            .way_path
            .points
            .get(order.move_params.way_path.cur_point)
            .copied()),
        OrderCode::MoveToObject => {
            let Some(target) = w.objects.by_watch_id(order.target) else {
                return Ok(None);
            };
            Ok(w.objects.get(target).map(|obj| obj.c.position))
        }
        OrderCode::AttackObject | OrderCode::AttackPoint => {
            let (target_handle, mut goal) = if order.code == OrderCode::AttackObject {
                let target = w.objects.by_watch_id(order.target);
                let Some(target_obj) = target.and_then(|target| w.objects.get(target)) else {
                    return Ok(None);
                };
                (target, target_obj.c.position)
            } else {
                (None, order.attack_params.target_point)
            };
            if let Some(move_goal) = move_goal {
                goal = move_goal;
            }
            if target_handle.is_none() {
                goal.z = w.terrain.elevation(&goal);
            }
            if move_params & mc2_path::movemap::MOVEPARAM_STEP_TOWARD_TARGET != 0 {
                let mut aim = Vec3 {
                    x: goal.x - position.x,
                    y: goal.y - position.y,
                    z: 0.0,
                };
                let length = crate::math::approximate_length(&aim);
                if length > 0.0 {
                    aim.x /= length;
                    aim.y /= length;
                    let step_length = crate::terrain::WORLD_UNITS_PER_VERTEX * 1.5;
                    aim.x *= step_length;
                    aim.y *= step_length;
                    aim.z = w.terrain.elevation(&aim);
                    let target_goal = Vec3 {
                        x: position.x + aim.x,
                        y: position.y + aim.y,
                        z: position.z + aim.z,
                    };
                    return Ok(Some(calc_offset_move_goal(w, position, target_goal)));
                }
                return Err(MoverDecisionUnavailable::AttackGoalScoring);
            }
            if move_params & mc2_path::movemap::MOVEPARAM_STEP_ADJACENT_TARGET != 0 {
                return Ok(Some(calc_offset_move_goal(w, position, goal)));
            }
            score_attack_goal(
                w,
                h,
                position,
                if radius > 0.0 { center } else { goal },
                radius,
                move_level,
                team.into(),
                &loadout,
                &order,
                target_handle,
                goal,
                move_params,
            )
            .map(Some)
        }
        _ => Ok(None),
    }
}

/// Port the score-map construction and ordered top-60 selection. `goalMapRowCol` stores the
/// loop row/column while the historical optimal-cell column expression intentionally uses
/// the row-relative offset (`goalCell[0] - mapCellUL[0]`) (`mover.cpp:4567-4580`).
#[allow(clippy::too_many_arguments)]
fn score_attack_goal(
    w: &mut crate::World,
    h: ObjectHandle,
    position: Vec3,
    center: Vec3,
    move_radius: f32,
    move_level: u8,
    _team: i32,
    loadout: &crate::combat::Loadout,
    order: &crate::command::TacticalOrder,
    target: Option<ObjectHandle>,
    goal: Vec3,
    move_params: u32,
) -> Result<Vec3, MoverDecisionUnavailable> {
    use mc2_path::movemap::{MOVEPARAM_RANDOM_OPTIMAL, MOVEPARAM_SOMEWHERE_ELSE};
    use mc2_path::{PathContext, GLOBALPATH_EXISTS_FALSE, GLOBALPATH_EXISTS_UNKNOWN};

    let center_cell = w.terrain.world_to_cell(&center);
    let goal_cell = w.terrain.world_to_cell(&goal);
    let map_ul = (
        center_cell.0 - (GOALMAP_CELL_DIM / 2) as i32,
        center_cell.1 - (GOALMAP_CELL_DIM / 2) as i32,
    );
    let current_cell = w.terrain.world_to_cell(&position);
    let current_area = w.terrain.move_data().global[usize::from(move_level.min(2))]
        .calc_area(current_cell.0, current_cell.1);

    // `hasWeaponNode()` refreshes the source cache on its first call and on
    // every call through turn 5, even when the current gesture will later use
    // the retained value in `getLOSPosition`.
    let has_weapon = mover_has_weapon_node(w, h);

    // In `getLOSPosition`, a mech after turn 6 keeps the previously observed
    // `lowestWeaponNodeZ` outside gestures 2/4/7. Do not invent the C++ 15-unit
    // sentinel fallback when that cached state has not reached Rust yet.
    let needs_cached_weapon_height = w.clock.turn >= 6
        && target.is_some()
        && matches!(
            w.objects.get(h).map(|obj| &obj.kind),
            Some(crate::object::Kind::Mech(_))
        )
        && !matches!(w.appearance.current_gesture_id(h), 2 | 4 | 7);
    if needs_cached_weapon_height {
        // `getLOSPosition` repairs a nonpositive cached height from the node position with the appearance temporarily in view
        // (mover.cpp:3498-3502), so an offscreen mover is not unavailable.
        let cached = w.objects.get(h).and_then(|obj| match &obj.kind {
            crate::object::Kind::Mech(mech) => mech.m.cached_los_weapon_node,
            _ => None,
        });
        match cached {
            Some((node, height)) if node >= 0 => {
                if height <= 0.0 {
                    mover_repair_cached_weapon_height(w, h, node);
                }
            }
            _ => return Err(MoverDecisionUnavailable::AttackGoalScoring),
        }
    }

    let move_level_index = usize::from(move_level.min(2));
    let meters_per_cell =
        crate::terrain::WORLD_UNITS_PER_CELL * crate::terrain::METERS_PER_WORLD_UNIT;
    let fire_range = match order.attack_params.range {
        crate::command::FIRERANGE_RAMMING => 0.0,
        crate::command::FIRERANGE_LONGEST => loadout.stats.max_range,
        crate::command::FIRERANGE_SHORT..=crate::command::FIRERANGE_LONG => {
            let idx = order.attack_params.range as usize;
            w.data
                .gamesys
                .tables
                .as_ref()
                .map(|g| (g.general.weapon_ranges[idx][1] - g.general.weapon_ranges[idx][0]) / 2.0)
                .unwrap_or(-1.0)
        }
        crate::command::FIRERANGE_CURRENT => -1.0,
        _ => loadout.stats.optimal_range,
    };
    let mut fire_cell_range = if fire_range > 0.0 {
        ((fire_range / meters_per_cell) as i32)
            .clamp(1, (crate::mover::MAX_ATTACK_CELLRANGE - 1) as i32)
    } else if fire_range == -1.0 {
        2
    } else {
        0
    } as usize;
    let min_cell_range = ((loadout.stats.min_range / meters_per_cell) as i32)
        .min((crate::mover::MAX_ATTACK_CELLRANGE - 1) as i32)
        .max(0) as usize;
    let max_cell_range = ((loadout.stats.max_range / meters_per_cell) as i32)
        .min((crate::mover::MAX_ATTACK_CELLRANGE - 1) as i32)
        .max(0) as usize;
    if !matches!(
        order.attack_params.range,
        crate::command::FIRERANGE_SHORT..=crate::command::FIRERANGE_LONG
    ) {
        fire_cell_range = fire_cell_range.min(crate::mover::MAX_ATTACK_CELLRANGE - 1);
    }

    let tables = w.move_scratch.optimal_cells.clone();
    let mut scores = [0_i32; GOALMAP_CELL_DIM * GOALMAP_CELL_DIM];
    if !matches!(
        order.attack_params.range,
        crate::command::FIRERANGE_SHORT..=crate::command::FIRERANGE_LONG
    ) {
        let first = usize::try_from(
            tables.range_indices[(max_cell_range + 1).min(crate::mover::MAX_ATTACK_CELLRANGE - 1)]
                [0],
        )
        .unwrap_or(0);
        let last = usize::try_from(tables.range_indices[crate::mover::MAX_ATTACK_CELLRANGE - 1][1])
            .unwrap_or(0);
        for &[dr, dc] in tables.ranged.get(first..last).unwrap_or_default() {
            let r = goal_cell.0 - map_ul.0 + i32::from(dr);
            let c = goal_cell.1 - map_ul.1 + i32::from(dc);
            if (0..GOALMAP_CELL_DIM as i32).contains(&r)
                && (0..GOALMAP_CELL_DIM as i32).contains(&c)
            {
                scores[r as usize * GOALMAP_CELL_DIM + c as usize] -= 500;
            }
        }
    }
    if fire_range > 0.0 && min_cell_range > 0 {
        let last = usize::try_from(tables.range_indices[min_cell_range][0]).unwrap_or(0);
        for &[dr, dc] in tables.ranged.iter().take(last) {
            let r = goal_cell.0 - map_ul.0 + i32::from(dr);
            let c = goal_cell.1 - map_ul.1 + i32::from(dc);
            if (0..GOALMAP_CELL_DIM as i32).contains(&r)
                && (0..GOALMAP_CELL_DIM as i32).contains(&c)
            {
                scores[r as usize * GOALMAP_CELL_DIM + c as usize] -= 500;
            }
        }
    }

    for &[dr, dc] in tables.optimal[fire_cell_range]
        .iter()
        .take(tables.num_increments)
    {
        let r = goal_cell.0 - map_ul.0 + i32::from(dr);
        // Preserve the source's `goalCell[0]` typo for the column offset.
        let c = goal_cell.0 - map_ul.0 + i32::from(dc);
        if (0..GOALMAP_CELL_DIM as i32).contains(&r) && (0..GOALMAP_CELL_DIM as i32).contains(&c) {
            let idx = r as usize * GOALMAP_CELL_DIM + c as usize;
            scores[idx] += if move_params & MOVEPARAM_RANDOM_OPTIMAL != 0 {
                500 + w.rng.0.borrow_mut().sim.random_number(40) * 5
            } else {
                500
            };
        }
    }

    let mut start_cell = w.terrain.world_to_cell(&position);
    start_cell.0 = (start_cell.0 - map_ul.0).clamp(0, GOALMAP_CELL_DIM as i32 - 1);
    start_cell.1 = (start_cell.1 - map_ul.1).clamp(0, GOALMAP_CELL_DIM as i32 - 1);
    apply_goal_proximity_bonus(&mut scores, goal_cell, map_ul, move_params);
    if move_params & MOVEPARAM_SOMEWHERE_ELSE != 0 {
        scores[start_cell.0 as usize * GOALMAP_CELL_DIM + start_cell.1 as usize] -= 10_000;
    }
    for r in 0..GOALMAP_CELL_DIM {
        for c in 0..GOALMAP_CELL_DIM {
            let distance = (r as i32 - start_cell.0).abs() + (c as i32 - start_cell.1).abs();
            scores[r * GOALMAP_CELL_DIM + c] -= distance;
        }
    }

    // Groupmates already heading somewhere make their final goal cell less attractive (`mover.cpp:4645-4663`).
    for goal in groupmate_goals(w, h) {
        let cell = w.terrain.world_to_cell(&goal);
        let (r, c) = (cell.0 - map_ul.0, cell.1 - map_ul.1);
        if (0..GOALMAP_CELL_DIM as i32).contains(&r) && (0..GOALMAP_CELL_DIM as i32).contains(&c) {
            scores[r as usize * GOALMAP_CELL_DIM + c as usize] -= 100;
        }
    }

    if move_radius > 0.0 {
        let move_cell_range = ((move_radius / meters_per_cell) as i32)
            .clamp(1, (crate::mover::MAX_ATTACK_CELLRANGE - 1) as i32)
            as usize;
        let first = usize::try_from(
            tables.range_indices[(move_cell_range + 1).min(crate::mover::MAX_ATTACK_CELLRANGE - 1)]
                [0],
        )
        .unwrap_or(0);
        let last = usize::try_from(tables.range_indices[crate::mover::MAX_ATTACK_CELLRANGE - 1][1])
            .unwrap_or(0);
        for &[dr, dc] in tables.ranged.get(first..last).unwrap_or_default() {
            let r = center_cell.0 - map_ul.0 + i32::from(dr);
            let c = center_cell.1 - map_ul.1 + i32::from(dc);
            if (0..GOALMAP_CELL_DIM as i32).contains(&r)
                && (0..GOALMAP_CELL_DIM as i32).contains(&c)
            {
                scores[r as usize * GOALMAP_CELL_DIM + c as usize] -= 5000;
            }
        }
    }
    let current_cell = w.terrain.world_to_cell(&position);
    let no_travel_off_map = w.terrain.map().in_bounds(current_cell.0, current_cell.1)
        && !w
            .terrain
            .map()
            .cell(current_cell.0, current_cell.1)
            .off_map();
    let gates = crate::object::GateSnapshot::from_store(&w.objects);
    let mut path_context = PathContext::new();
    for r in 0..GOALMAP_CELL_DIM {
        for c in 0..GOALMAP_CELL_DIM {
            let idx = r * GOALMAP_CELL_DIM + c;
            let map_row = map_ul.0 + r as i32;
            let map_col = map_ul.1 + c as i32;
            if !w.terrain.map().in_bounds(map_row, map_col) {
                scores[idx] = -999_999;
                continue;
            }
            let (passable, off_map, deep_water, mover) = {
                let cell = w.terrain.map().cell(map_row, map_col);
                (
                    cell.passable(),
                    cell.off_map(),
                    cell.deep_water(),
                    cell.mover(),
                )
            };
            if !passable {
                scores[idx] -= 10_000;
            }
            if off_map {
                scores[idx] -= 10_000;
            }
            if deep_water && move_level != 1 {
                scores[idx] -= 999_999;
            }
            if no_travel_off_map && off_map {
                scores[idx] -= 50_000;
            }
            if order.code == crate::command::OrderCode::Refit && mover {
                scores[idx] -= 1000;
            }
            if move_level < 2 {
                let global = &mut w.terrain.move_data_mut().global[move_level_index];
                let area = global.calc_area(map_row, map_col);
                if area != current_area && area != -1 && current_area != -1 {
                    if global.get_path_exists(current_area, area) == GLOBALPATH_EXISTS_UNKNOWN {
                        match global.calc_path(
                            &mut path_context,
                            current_area,
                            area,
                            None,
                            None,
                            &gates,
                        ) {
                            Ok(path) if !path.is_empty() => {
                                for from in &path {
                                    for to in &path {
                                        global.set_path_exists(
                                            from.thru_area,
                                            to.thru_area,
                                            mc2_path::GLOBALPATH_EXISTS_TRUE,
                                        );
                                    }
                                }
                            }
                            _ => {
                                global.set_path_exists(area, current_area, GLOBALPATH_EXISTS_FALSE);
                                global.set_path_exists(current_area, area, GLOBALPATH_EXISTS_FALSE);
                            }
                        }
                    }
                    if global.get_path_exists(current_area, area) == GLOBALPATH_EXISTS_FALSE {
                        scores[idx] -= 50_000;
                    }
                }
            }
        }
    }

    let mut ranked = vec![(-99_999_i32, usize::MAX); MAX_MOVE_GOALS];
    for (index, &weight) in scores.iter().enumerate() {
        if index >= MAX_MOVE_GOALS && weight <= ranked[MAX_MOVE_GOALS - 1].0 {
            continue;
        }
        let mut insertion = MAX_MOVE_GOALS as isize - 2;
        while insertion >= 0 && weight >= ranked[insertion as usize].0 {
            insertion -= 1;
        }
        let insertion = (insertion + 1) as usize;
        ranked.copy_within(insertion..MAX_MOVE_GOALS - 1, insertion + 1);
        ranked[insertion] = (weight, index);
    }
    let ranked_goal = select_ranked_los_goal(&ranked, has_weapon, |index| {
        let candidate_row = map_ul.0 + (index / GOALMAP_CELL_DIM) as i32;
        let candidate_col = map_ul.1 + (index % GOALMAP_CELL_DIM) as i32;
        let mut candidate = w.terrain.cell_to_world(candidate_row, candidate_col);
        candidate.z = w.terrain.elevation(&candidate);
        if let Some(target) = target {
            let previous = w.objects.get(h).map(|obj| obj.c.position);
            let old_cache = w.objects.use_mover_los_table;
            if let Some(obj) = w.objects.get_mut(h) {
                obj.c.position = candidate;
            }
            w.objects.use_mover_los_table = false;
            let visible = crate::object::hooks::line_of_sight(w, h, target, 0.0);
            w.objects.use_mover_los_table = old_cache;
            if let (Some(previous), Some(obj)) = (previous, w.objects.get_mut(h)) {
                obj.c.position = previous;
            }
            visible
        } else {
            let team_id = w.objects.get(h).map_or(-1, |obj| obj.team_id());
            let los_from = mover_los_position_for_candidate(w, h, candidate);
            w.terrain.is_game_select_position(&goal)
                && crate::ai::sensors::line_of_sight_ray(w, los_from, goal, team_id, 15.0, 0.0)
        }
    });
    let selected = ranked_goal.index;
    let mut row = map_ul.0 + (selected / GOALMAP_CELL_DIM) as i32;
    let mut col = map_ul.1 + (selected % GOALMAP_CELL_DIM) as i32;
    if ranked_goal.no_lof && has_weapon {
        // Nothing in the top-ranked cells sees the target: the cell the target's own line of ground says is best (mover.cpp:4893-4955).
        let fire_range_now = w
            .pilots
            .get(slot_of(w, h))
            .map_or(0.0, |p| p.situation.fire_range);
        if let Some(cell) = no_lof_fallback_goal(
            w,
            current_area,
            center_cell,
            goal_cell,
            move_radius,
            fire_range_now,
            move_level,
        ) {
            (row, col) = cell;
        }
    }
    #[cfg(debug_assertions)]
    {
        w.move_scratch.last_goal_cell = Some((row, col));
    }
    let result = calc_offset_move_goal(w, position, w.terrain.cell_to_world(row, col));
    let _ = (move_radius, target);
    Ok(result)
}

/// `MechWarrior::getMoveGlobalGoal` of every other member of `h`'s group (`MoverGroup::getMovers`): the actual final goal
/// (`moveOrders.originalGlobalGoal[1]`) of each groupmate whose path type is not `MOVEPATH_UNDEFINED` (warrior.h:1523).
pub fn groupmate_goals(w: &crate::World, h: ObjectHandle) -> Vec<Vec3> {
    let group_id = w.objects.get(h).map_or(-1, |obj| match &obj.kind {
        crate::object::Kind::Mech(mech) => i32::from(mech.m.group_id),
        crate::object::Kind::Vehicle(vehicle) => i32::from(vehicle.m.group_id),
        _ => -1,
    });
    let mut goals = Vec::new();
    if group_id < 0 {
        return goals;
    }
    for mate in w.objects.mechs.iter().chain(&w.objects.vehicles).copied() {
        if mate == h {
            continue;
        }
        let Some((mate_group, slot)) = w.objects.get(mate).map(|obj| match &obj.kind {
            crate::object::Kind::Mech(mech) => (i32::from(mech.m.group_id), mech.m.pilot_slot),
            crate::object::Kind::Vehicle(vehicle) => {
                (i32::from(vehicle.m.group_id), vehicle.m.pilot_slot)
            }
            _ => (-1, 0),
        }) else {
            continue;
        };
        if mate_group != group_id {
            continue;
        }
        if let Some(pilot) = w.pilots.get(slot) {
            if pilot.lane_b_move.path_type != crate::ai::moveorders::MOVEPATH_UNDEFINED {
                goals.push(pilot.lane_b_move.original_global_goal[1]);
            }
        }
    }
    goals
}

/// `calcMoveGoal` (mover.cpp:4412, 4611-4617) uses only the caller's player bit;
/// it adds the nine +100 cells when that bit is clear.
fn apply_goal_proximity_bonus(
    scores: &mut [i32; GOALMAP_CELL_DIM * GOALMAP_CELL_DIM],
    goal_cell: (i32, i32),
    map_ul: (i32, i32),
    move_params: u32,
) {
    if move_params & mc2_path::movemap::MOVEPARAM_PLAYER != 0 {
        return;
    }
    for dr in -1..=1 {
        for dc in -1..=1 {
            let r = goal_cell.0 - map_ul.0 + dr;
            let c = goal_cell.1 - map_ul.1 + dc;
            if (0..GOALMAP_CELL_DIM as i32).contains(&r)
                && (0..GOALMAP_CELL_DIM as i32).contains(&c)
            {
                scores[r as usize * GOALMAP_CELL_DIM + c as usize] += 100;
            }
        }
    }
}

fn slot_of(w: &crate::World, h: ObjectHandle) -> u16 {
    match w.objects.get(h).map(|o| &o.kind) {
        Some(crate::object::Kind::Mech(m)) => m.m.pilot_slot,
        Some(crate::object::Kind::Vehicle(v)) => v.m.pilot_slot,
        _ => 0,
    }
}

/// The `noLOF && hasWeaponNode()` block of `calcMoveGoal` (mover.cpp:4893-4955). With a move radius the candidates are the
/// radius-limited best cell and the centre cell; otherwise one cell per compass direction around the target, ten cells out, at the
/// pilot's situation fire range.
///
/// C++ picks `bestBest` by `resultCost[k] < resultCost[bestBest]` starting from candidate 0 with unset costs at 0, so a candidate 0
/// without a route (cost 0) can never be beaten; then `resultCost[bestBest] > 0` fails and the goal stays the last cell the line-of-sight
/// scan looked at (`None` here). The shipped game runs the fix ([`pick_cheapest_reachable`]); the C++ pick ([`pick_cpp_best_best`]) is kept only behind
/// the test-only switch `World::cpp_no_lof_best_best = true` (`DIFFERENCES.md`, id `NO-LOF-BESTBEST`). The four slots keep their index, a
/// candidate without a cell keeps cost 0.
fn no_lof_fallback_goal(
    w: &mut crate::World,
    current_area: i32,
    center_cell: (i32, i32),
    goal_cell: (i32, i32),
    move_radius: f32,
    situation_fire_range: f32,
    move_level: u8,
) -> Option<(i32, i32)> {
    let mut candidates: [Option<(i32, i32)>; 4] = [None; 4];
    if move_radius > 0.0 {
        candidates[0] = best_goal_from_target(w, center_cell, goal_cell, move_radius);
        candidates[1] = Some(center_cell);
    } else {
        for (slot, max_pos) in [
            (goal_cell.0 - 10, goal_cell.1),
            (goal_cell.0, goal_cell.1 + 10),
            (goal_cell.0 + 10, goal_cell.1),
            (goal_cell.0, goal_cell.1 - 10),
        ]
        .into_iter()
        .enumerate()
        {
            candidates[slot] = best_goal_from_target(w, goal_cell, max_pos, situation_fire_range);
        }
    }
    let level = usize::from(move_level.min(2));
    let gates = crate::object::GateSnapshot::from_store(&w.objects);
    let mut ctx = mc2_path::PathContext::new();
    let mut costs = [0_i32; 4];
    for (slot, cell) in candidates.iter().enumerate() {
        let Some(cell) = cell else { continue };
        let global = &mut w.terrain.move_data_mut().global[level];
        let area = global.calc_area(cell.0, cell.1);
        let mut confidence = 0;
        costs[slot] = global.get_path_cost(
            &mut ctx,
            current_area,
            area,
            false,
            &mut confidence,
            true,
            &gates,
        );
    }
    let slot = pick_no_lof_slot(w.cpp_no_lof_best_best, &costs);
    NO_LOF_FALLBACKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if pick_cpp_best_best(&costs) != pick_cheapest_reachable(&costs) {
        NO_LOF_FALLBACKS_DIFFERING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    slot.and_then(|slot| candidates[slot])
}

static NO_LOF_FALLBACKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static NO_LOF_FALLBACKS_DIFFERING: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// How often `no_lof_fallback_goal` ran in this process, and how often the C++ pick and the fix would have chosen differently
/// (a diagnostic for the visual-QA play-test A/B, `rust/docs/VISUAL-QA.md`; it does not influence the simulation).
pub fn no_lof_fallback_counts() -> (u64, u64) {
    (
        NO_LOF_FALLBACKS.load(std::sync::atomic::Ordering::Relaxed),
        NO_LOF_FALLBACKS_DIFFERING.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// The `bestBest` pick of `no_lof_fallback_goal`: the C++ pick when the world's test-only `NO-LOF-BESTBEST` switch
/// (`World::cpp_no_lof_best_best`) is set, else the corrected one.
fn pick_no_lof_slot(cpp: bool, costs: &[i32; 4]) -> Option<usize> {
    if cpp {
        pick_cpp_best_best(costs)
    } else {
        pick_cheapest_reachable(costs)
    }
}

/// The C++ pick (`mover.cpp:4938-4959` and the radius variant 4905-4917): `bestBest` = 0, a later candidate with cost above 0 replaces it
/// when strictly cheaper than `costs[bestBest]` (so never when candidate 0 has cost 0), and the result is used only if `costs[bestBest] > 0`.
fn pick_cpp_best_best(costs: &[i32; 4]) -> Option<usize> {
    let mut best = 0;
    for (slot, &cost) in costs.iter().enumerate().skip(1) {
        if cost > 0 && cost < costs[best] {
            best = slot;
        }
    }
    (costs[best] > 0).then_some(best)
}

/// Index of the cheapest candidate that has a route (cost above 0). `mover.cpp:4938-4959` starts `bestBest` at candidate 0 and compares
/// `resultCost[k] < resultCost[bestBest]`, so an unreachable candidate 0 (cost 0) can never be beaten; that is a logic error and is not copied
/// (`DIFFERENCES.md`, `no_lof_fallback_goal`).
fn pick_cheapest_reachable(costs: &[i32; 4]) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (slot, &cost) in costs.iter().enumerate() {
        if cost > 0 && best.is_none_or(|b| cost < costs[b]) {
            best = Some(slot);
        }
    }
    best
}

#[cfg(test)]
mod pick_cheapest_reachable_tests {
    use super::pick_cheapest_reachable;

    #[test]
    fn unreachable_first_candidate_does_not_block_a_later_route() {
        assert_eq!(pick_cheapest_reachable(&[0, 0, 30, 20]), Some(3));
        assert_eq!(pick_cheapest_reachable(&[0, 7, 0, 0]), Some(1));
        assert_eq!(pick_cheapest_reachable(&[5, 3, 9, 3]), Some(1));
        assert_eq!(pick_cheapest_reachable(&[0; 4]), None);
    }

    #[test]
    fn switch_off_changes_the_best_best_pick() {
        use super::pick_no_lof_slot;
        let costs = [0, 0, 30, 20];
        assert_eq!(pick_no_lof_slot(false, &costs), Some(3));
        assert_eq!(pick_no_lof_slot(true, &costs), None);
    }

    #[test]
    fn concurrent_worlds_keep_their_own_switch() {
        use super::pick_no_lof_slot;
        use std::sync::{Arc, Barrier};
        let barrier = Arc::new(Barrier::new(2));
        let run = |cpp: bool| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut w = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
                w.cpp_no_lof_best_best = cpp;
                barrier.wait();
                (0..10_000)
                    .map(|_| pick_no_lof_slot(w.cpp_no_lof_best_best, &[0, 0, 30, 20]))
                    .all(|slot| slot == if cpp { None } else { Some(3) })
            })
        };
        let (a, b) = (run(true), run(false));
        assert!(a.join().unwrap() && b.join().unwrap());
    }

    #[test]
    fn cpp_pick_never_beats_an_unreachable_first_candidate() {
        use super::pick_cpp_best_best;
        assert_eq!(pick_cpp_best_best(&[0, 0, 30, 20]), None);
        assert_eq!(pick_cpp_best_best(&[0, 7, 0, 0]), None);
        assert_eq!(pick_cpp_best_best(&[5, 3, 9, 3]), Some(1));
        assert_eq!(pick_cpp_best_best(&[4, 0, 9, 2]), Some(3));
    }
}

/// Result of the `MaxMoveGoalChecks` scan of the ranked goal cells (`mover.cpp:4825-4880`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RankedGoal {
    /// `curGoalCell` as the scan leaves it: the first visible cell, or the last cell looked at.
    index: usize,
    /// `noLOF`: no checked cell sees the target.
    no_lof: bool,
}

/// C++ scans the (at most `MaxMoveGoalChecks` = 60) ranked candidates; a candidate only gets the line-of-sight check when its weight is
/// above -10000. `curGoalCell` is assigned before the weight test, so a scan that finds nothing leaves the last candidate looked at.
fn select_ranked_los_goal(
    ranked: &[(i32, usize)],
    has_weapon: bool,
    mut visible: impl FnMut(usize) -> bool,
) -> RankedGoal {
    let default = ranked.first().map(|(_, index)| *index).unwrap_or(0);
    if !has_weapon {
        return RankedGoal {
            index: default,
            no_lof: false,
        };
    }
    let mut last = default;
    for &(weight, index) in ranked.iter().take(MAX_MOVE_GOALS) {
        last = index;
        if weight <= -10_000 {
            continue;
        }
        if visible(index) {
            return RankedGoal {
                index,
                no_lof: false,
            };
        }
    }
    RankedGoal {
        index: last,
        no_lof: true,
    }
}

/// `calcBestGoalFromTarget` (mover.cpp:4138): walk the cell line from `target_pos` toward `max_pos` and return the passable cell
/// that sits about `best_range` meters from the end of the line, or the last passable cell seen before the terrain hides the
/// target (a `startLocal` = 40 unit ray). `None` is the C++ `return 0`.
fn best_goal_from_target(
    w: &crate::World,
    target_pos: (i32, i32),
    max_pos: (i32, i32),
    best_range: f32,
) -> Option<(i32, i32)> {
    const START_LOCAL: f32 = 40.0;
    let t = &w.terrain;
    let passable = |r: i32, c: i32| t.in_bounds(r, c) && t.get_passable(r, c);
    let end_pos = t.get_cell_pos(max_pos.0, max_pos.1);
    let start_pos = t.get_cell_pos(target_pos.0, target_pos.1);
    let delta = Vec3 {
        x: (max_pos.1 - target_pos.1) as f32,
        y: (max_pos.0 - target_pos.0) as f32,
        z: 0.0,
    };
    let mut start_height = start_pos.z;
    let cell_length = crate::terrain::WORLD_UNITS_PER_CELL * crate::terrain::METERS_PER_WORLD_UNIT;
    let length = crate::math::approximate_length(&delta);

    let mut last_cell: Option<(i32, i32)> = None;
    if length > 1.0e-4 {
        let col_length = delta.x / length;
        let row_length = delta.y / length;
        let height_len = (end_pos.z - start_pos.z) / length;
        let last_col = (f64::from(col_length) * 2.0).abs() as f32;
        let last_row = (f64::from(row_length) * 2.0).abs() as f32;
        let mut cell_row = target_pos.0 as f32;
        let mut cell_col = target_pos.1 as f32;
        let end_row = max_pos.0 as f32;
        let end_col = max_pos.1 as f32;
        let mut current = start_pos;
        let mut remaining = crate::math::approximate_length(&Vec3 {
            x: end_pos.x - current.x,
            y: end_pos.y - current.y,
            z: end_pos.z - current.z,
        });
        let best_remaining = remaining - best_range * crate::terrain::WORLD_UNITS_PER_METER;
        let (mut col_done, mut row_done) = (false, false);
        while !col_done || !row_done {
            if (cell_row - end_row).abs() > last_row {
                cell_row += row_length;
            } else {
                cell_row = end_row - last_row;
                row_done = true;
            }
            if (cell_col - end_col).abs() > last_col {
                cell_col += col_length;
            } else {
                cell_col = end_col - last_col;
                col_done = true;
            }
            start_height += height_len;
            let (r, c) = (cell_row as i32, cell_col as i32);
            current = t.get_cell_pos(r, c);
            let local_elev = crate::terrain::WORLD_UNITS_PER_METER
                * (if t.in_bounds(r, c) {
                    t.get_local_height(r, c)
                } else {
                    0
                }) as f32;
            current.z += local_elev;

            if start_height + START_LOCAL < current.z {
                if let Some((lr, lc)) = last_cell.filter(|&(lr, lc)| passable(lr, lc)) {
                    return Some((lr, lc));
                }
            }
            remaining = crate::math::approximate_length(&Vec3 {
                x: end_pos.x - current.x,
                y: end_pos.y - current.y,
                z: end_pos.z - current.z,
            });
            if (remaining - best_remaining).abs() < cell_length
                && remaining > best_remaining
                && passable(r, c)
            {
                return Some((r, c));
            }
            if remaining < 10.0 {
                return passable(r, c).then_some((r, c));
            }
            if passable(r, c) {
                last_cell = Some((r, c));
            }
        }
    }
    last_cell
}

/// Mover::calcOffsetMoveGoal (mover.cpp:4046-4105): when a selected cell is
/// impassable, walk toward the mover by half-cell increments and return terrain Z.
pub fn calc_offset_move_goal(w: &crate::World, start: Vec3, target: Vec3) -> Vec3 {
    use crate::math::Vec3Ext;
    let mut delta = Vec3 {
        x: start.x - target.x,
        y: start.y - target.y,
        z: 0.0,
    };
    if delta.x == 0.0 && delta.y == 0.0 {
        return start;
    }
    let length = delta.length();
    delta.x /= length;
    delta.y /= length;
    let cell_length = crate::terrain::WORLD_UNITS_PER_CELL * 0.5;
    delta.x *= cell_length;
    delta.y *= cell_length;
    let mut point = target;
    let distance_to_target = Vec3 {
        x: start.x - target.x,
        y: start.y - target.y,
        z: start.z - target.z,
    }
    .length();
    let mut ray_length = 0.0;
    let cell = w.terrain.world_to_cell(&point);
    if !w.terrain.map().in_bounds(cell.0, cell.1)
        || !w.terrain.map().cell(cell.0, cell.1).passable()
    {
        while ray_length < distance_to_target {
            let cell = w.terrain.world_to_cell(&point);
            if w.terrain.map().in_bounds(cell.0, cell.1)
                && w.terrain.map().cell(cell.0, cell.1).passable()
            {
                break;
            }
            point.x += delta.x;
            point.y += delta.y;
            ray_length = Vec3 {
                x: point.x - start.x,
                y: point.y - start.y,
                z: 0.0,
            }
            .length();
        }
    }
    point.z = w.terrain.elevation(&point);
    point
}

/// `Mover::hasWeaponNode` (mover.cpp:3447-3458) gates calcMoveGoal's LOS search on an
/// appearance weapon node, not on whether the loadout contains a weapon.
fn mover_has_weapon_node(w: &mut crate::World, h: ObjectHandle) -> bool {
    let turn = w.clock.turn;
    if w.appearance.get(h).is_none() {
        return false;
    }
    let cached = w.objects.get(h).and_then(|obj| match &obj.kind {
        crate::object::Kind::Mech(mech) => mech.m.cached_los_weapon_node,
        crate::object::Kind::Vehicle(vehicle) => vehicle.m.cached_los_weapon_node,
        _ => None,
    });
    if turn >= 6 && cached.is_some_and(|(node, _)| node != -1 && node != -2) {
        return true;
    }
    // no `setInView(true)` here (mover.cpp:3447-3458 does not make one): an appearance that is not in view answers every node query with
    // its own position, so the first weapon node ties for lowest and the cached height is the mover's height above the terrain.
    let node = w.appearance.lowest_weapon_node(h);
    let node_pos = w.appearance.weapon_node_position(h, node);
    let height = node_pos.z - w.terrain.elevation(&node_pos);
    if let Some(obj) = w.objects.get_mut(h) {
        let slot = match &mut obj.kind {
            crate::object::Kind::Mech(mech) => Some(&mut mech.m.cached_los_weapon_node),
            crate::object::Kind::Vehicle(vehicle) => Some(&mut vehicle.m.cached_los_weapon_node),
            _ => None,
        };
        if let Some(slot) = slot {
            if slot.is_none_or(|(cached_node, _)| cached_node == -1 || cached_node == -2)
                || turn < 6
            {
                *slot = Some((node, height));
            }
        }
    }
    node != -1
}

/// `GameObject::lineOfSight(point, false)` starts at `Mover::getLOSPosition`, whose Z is the
/// lowest weapon node above terrain (mover.cpp:3463-3517, gameobj.cpp:1828-1837). The C++ call
/// temporarily marks the appearance in view while it queries the node, then restores the prior
/// flag. `Team::lineOfSight` also receives the point target's 15-unit extent.
pub(crate) fn mover_los_position_for_candidate(
    w: &mut crate::World,
    h: ObjectHandle,
    candidate: Vec3,
) -> Vec3 {
    let los = mover_los_position_cached(w, h, candidate);
    // `Mover::getLOSPosition` (mover.cpp:3492-3496, 3511-3515): while the mech is in the jump gesture (20) its line of sight starts
    // at the lowest weapon node's CURRENT height above the terrain under it, not at the cached standing height.
    if w.appearance.current_gesture_id(h) == 20 {
        let node = match w.objects.get(h).map(|o| &o.kind) {
            Some(crate::object::Kind::Mech(m)) => m.m.cached_los_weapon_node,
            Some(crate::object::Kind::Vehicle(v)) => v.m.cached_los_weapon_node,
            _ => None,
        }
        .map_or(-2, |c| c.0);
        // the whole C++ block runs with `appearance->setInView(true)`
        let old_in_view = w.appearance.in_view(h);
        w.appearance.set_in_view(h, true);
        let node_pos = w.appearance.weapon_node_position(h, node);
        w.appearance.set_in_view(h, old_in_view);
        let jump_lowest_node = node_pos.z - w.terrain.elevation(&node_pos);
        if jump_lowest_node > 0.0 {
            let mut p = candidate;
            p.z += jump_lowest_node;
            return p;
        }
    }
    los
}

fn mover_los_position_cached(w: &mut crate::World, h: ObjectHandle, mut candidate: Vec3) -> Vec3 {
    let Some(obj) = w.objects.get(h) else {
        return candidate;
    };
    let is_mech = matches!(obj.kind, crate::object::Kind::Mech(_));
    let normal = obj.c.status == crate::object::ObjectStatus::Normal;
    let gesture = w.appearance.current_gesture_id(h);
    let cached_node = w.objects.get(h).and_then(|obj| match &obj.kind {
        crate::object::Kind::Mech(mech) => mech.m.cached_los_weapon_node.map(|(node, _)| node),
        crate::object::Kind::Vehicle(vehicle) => {
            vehicle.m.cached_los_weapon_node.map(|(node, _)| node)
        }
        _ => None,
    });
    let refresh_height = normal
        && (matches!(gesture, 2 | 4 | 7) || !is_mech)
        && (cached_node.is_none_or(|node| node == -1 || node == -2) || w.clock.turn < 6);
    if refresh_height {
        let old_in_view = w.appearance.in_view(h);
        w.appearance.set_in_view(h, true);
        let mut node = w.appearance.lowest_weapon_node(h);
        if node == -1 {
            // "We must not have ever rendered this guy": update his geometry (in view, so the node positions are computed)
            // and ask again (mover.cpp:3482-3489).
            crate::appearance::update(w, h, true);
            node = w.appearance.lowest_weapon_node(h);
        }
        let node_pos = (node >= 0).then(|| w.appearance.weapon_node_position(h, node));
        w.appearance.set_in_view(h, old_in_view);
        if let Some(node_pos) = node_pos {
            let height = node_pos.z - w.terrain.elevation(&node_pos);
            if let Some(obj) = w.objects.get_mut(h) {
                match &mut obj.kind {
                    crate::object::Kind::Mech(mech) => {
                        mech.m.cached_los_weapon_node = Some((node, height));
                    }
                    crate::object::Kind::Vehicle(vehicle) => {
                        vehicle.m.cached_los_weapon_node = Some((node, height));
                    }
                    _ => {}
                }
            }
            candidate.z += height;
            return candidate;
        }
    }
    // Later turns retain the cached height (a mech outside gestures 2/4/7, a vehicle once it holds a node): `losPos.z +=
    // lowestWeaponNodeZ` whenever it is not the initial -9999.9 (mover.cpp:3497-3503).
    let cached = w.objects.get(h).and_then(|obj| match &obj.kind {
        crate::object::Kind::Mech(mech) => mech.m.cached_los_weapon_node,
        crate::object::Kind::Vehicle(vehicle) => vehicle.m.cached_los_weapon_node,
        _ => None,
    });
    // A mover that never took the refresh branch still holds `lowestWeaponNodeID = -2` and `lowestWeaponNodeZ = -9999.9`, which
    // is `<= 0`, so mover.cpp:3498-3502 reads node -2 off the appearance (a mech that has not yet been in gesture 2/4/7).
    let cached = cached.or_else(|| w.appearance.get(h).is_some().then_some((-2, -9999.9)));
    if let Some((node, mut height)) = cached {
        if height <= 0.0 {
            height = mover_repair_cached_weapon_height(w, h, node);
        }
        candidate.z += height;
        return candidate;
    }
    // `lowestWeaponNodeZ` starts at -9999.9, so the source fallback is +15.
    candidate.z += 15.0;
    candidate
}

/// The `lowestWeaponNodeZ <= 0` repair of `Mover::getLOSPosition` (mover.cpp:3498-3502): it runs inside the `setInView(true)` bracket,
/// so the node position is the real one even for an offscreen appearance. Stores and returns the repaired height.
fn mover_repair_cached_weapon_height(w: &mut crate::World, h: ObjectHandle, node: i32) -> f32 {
    let old_in_view = w.appearance.in_view(h);
    w.appearance.set_in_view(h, true);
    let node_pos = w.appearance.weapon_node_position(h, node);
    w.appearance.set_in_view(h, old_in_view);
    let height = node_pos.z - w.terrain.elevation(&node_pos);
    if let Some(obj) = w.objects.get_mut(h) {
        match &mut obj.kind {
            crate::object::Kind::Mech(mech) => mech.m.cached_los_weapon_node = Some((node, height)),
            crate::object::Kind::Vehicle(vehicle) => {
                vehicle.m.cached_los_weapon_node = Some((node, height));
            }
            _ => {}
        }
    }
    height
}

/// `Mover::bounceToAdjCell` (collision response, mover.cpp:5384).
pub fn bounce_to_adjacent_cell(w: &mut crate::World, h: ObjectHandle) {
    let Some(obj) = w.objects.get(h) else { return };
    let (slot, level, start) = match &obj.kind {
        crate::object::Kind::Mech(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1)),
        ),
        crate::object::Kind::Vehicle(s) => (
            s.m.pilot_slot,
            s.m.move_level,
            (i32::from(obj.c.cell.0), i32::from(obj.c.cell.1)),
        ),
        _ => return,
    };
    let offsets = [
        (-1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
        (1, 0),
        (1, -1),
        (0, -1),
        (-1, -1),
    ];
    let lock_level = u32::from(level == 2);
    for (dr, dc) in offsets {
        let cell = (start.0 + dr, start.1 + dc);
        if w.terrain.map().in_bounds(cell.0, cell.1)
            && !w.terrain.map().cell(cell.0, cell.1).pathlock(lock_level)
            && w.terrain.map().cell(cell.0, cell.1).passable()
        {
            update_path_lock(w, h, false);
            let position = w.terrain.cell_to_world(cell.0, cell.1);
            set_position(w, h, position);
            crate::ai::pause_path(w, slot);
            update_path_lock(w, h, true);
            return;
        }
    }
}

/// Mover lines of the digest (`M`/`V`): see [`digest::digest_mover`]. Reads state only.
pub fn digest_mover(w: &crate::World, h: ObjectHandle, level: u32, out: &mut Vec<String>) {
    digest::digest_mover(w, h, level, out);
}

/// CROSS-LANE (lane E -> B): `Mover::initOptimalCells(32)` (mover.cpp:6961), the last step of `Mission::init`.
pub fn init_optimal_cells(w: &mut crate::World, num_increments: i32) {
    let n = usize::try_from(num_increments)
        .unwrap_or(0)
        .min(MAX_ATTACK_INCREMENTS);
    let tables = &mut w.move_scratch.optimal_cells;
    tables.num_increments = n;
    // The source macro is a float literal, then explicitly promoted to double.
    let degrees_to_radians = f64::from(0.017_453_292_f32);
    let increment_degrees = if n == 0 { 0.0 } else { 360.0 / n as f64 };
    for r in 0..MAX_ATTACK_CELLRANGE {
        for i in 0..n {
            let radians = increment_degrees * i as f64 * degrees_to_radians;
            let x = mc2_libm::sin(radians) * r as f64;
            let y = mc2_libm::cos(radians) * r as f64;
            // C++ adds/subtracts 0.5 and then truncates to a signed char.
            tables.optimal[r][i][0] = (if x < 0.0 { x - 0.5 } else { x + 0.5 }) as i8;
            tables.optimal[r][i][1] = (if y < 0.0 { y - 0.5 } else { y + 0.5 }) as i8;
        }
    }

    let mut ranged_map = [[0_i8; RANGED_CELLS_SIDE]; RANGED_CELLS_SIDE];
    for (r, row) in ranged_map.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            let dr = r as f64 - MAX_ATTACK_CELLRANGE as f64;
            let dc = c as f64 - MAX_ATTACK_CELLRANGE as f64;
            *cell = (mc2_libm::hypot(dr, dc) + 0.5) as i8;
        }
    }
    let mut index = 0;
    for range in 0..MAX_ATTACK_CELLRANGE {
        tables.range_indices[range][0] = index as i16;
        for (r, row) in ranged_map.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if *cell == range as i8 {
                    tables.ranged[index] = [
                        (r as i32 - MAX_ATTACK_CELLRANGE as i32) as i8,
                        (c as i32 - MAX_ATTACK_CELLRANGE as i32) as i8,
                    ];
                    index += 1;
                }
            }
        }
        tables.range_indices[range][1] = index as i16 - 1;
    }
    tables.ranged_len = index;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_failed_ranked_los_checks_report_no_lof_at_the_last_cell() {
        let ranked =
            std::array::from_fn::<_, MAX_MOVE_GOALS, _>(|index| (900 - index as i32, index));
        assert_eq!(
            select_ranked_los_goal(&ranked, true, |_| false),
            RankedGoal {
                index: MAX_MOVE_GOALS - 1,
                no_lof: true
            }
        );
    }

    #[test]
    fn ranked_los_skips_low_score_candidates_and_returns_visible_cell() {
        let ranked = [(900, 8), (-10_000, 7), (700, 6)];
        assert_eq!(
            select_ranked_los_goal(&ranked, true, |index| index == 6),
            RankedGoal {
                index: 6,
                no_lof: false
            }
        );
    }

    #[test]
    fn caller_player_bit_controls_the_nine_cell_goal_bonus() {
        let mut non_player_scores = [0; GOALMAP_CELL_DIM * GOALMAP_CELL_DIM];
        apply_goal_proximity_bonus(&mut non_player_scores, (30, 30), (0, 0), 0);
        for row in 0..GOALMAP_CELL_DIM {
            for col in 0..GOALMAP_CELL_DIM {
                let expected = if (29..=31).contains(&row) && (29..=31).contains(&col) {
                    100
                } else {
                    0
                };
                assert_eq!(
                    non_player_scores[row * GOALMAP_CELL_DIM + col],
                    expected,
                    "zero-bit score at ({row}, {col})"
                );
            }
        }

        let mut player_scores = [0; GOALMAP_CELL_DIM * GOALMAP_CELL_DIM];
        apply_goal_proximity_bonus(
            &mut player_scores,
            (30, 30),
            (0, 0),
            mc2_path::movemap::MOVEPARAM_PLAYER,
        );
        assert!(player_scores.iter().all(|score| *score == 0));
    }

    #[test]
    fn calc_offset_move_goal_walks_out_of_an_impassable_selected_cell() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 17);
        world.terrain = crate::terrain::Terrain::from_elevations(64, &vec![0.0; 64 * 64]);
        let start = world.terrain.cell_to_world(96, 100);
        let target = world.terrain.cell_to_world(96, 96);
        world.terrain.set_passable(96, 96, false);
        world.terrain.set_passable(96, 100, true);

        let adjusted = calc_offset_move_goal(&world, start, target);

        assert_ne!((adjusted.x, adjusted.y), (target.x, target.y));
        assert_eq!(world.terrain.world_to_cell(&adjusted), (96, 100));
        assert!(world.terrain.map().cell(96, 100).passable());
    }

    #[test]
    fn init_optimal_cells_builds_legacy_tables_without_rng_draws() {
        let mut world = crate::World::new(crate::rng::RngMode::LegacyMsvc, 17);
        let before = world.rng.0.borrow().sim.draws();
        init_optimal_cells(&mut world, 32);
        let tables = &world.move_scratch.optimal_cells;
        assert_eq!(tables.num_increments, 32);
        assert_eq!(tables.optimal[0][0], [0, 0]);
        assert_eq!(tables.optimal[1][0], [0, 1]);
        assert_eq!(tables.optimal[1][8], [1, 0]);
        assert_eq!(tables.optimal[1][16], [0, -1]);
        assert_eq!(tables.optimal[1][24], [-1, 0]);
        assert_eq!(tables.range_indices[0], [0, 0]);
        assert_eq!(tables.ranged[0], [0, 0]);
        assert_eq!(tables.ranged_len, 2733);
        assert_eq!(world.rng.0.borrow().sim.draws(), before);
    }

    #[test]
    fn waypoint_cap_uses_only_the_current_step() {
        let mut path = mc2_path::MovePath::new();
        path.init(2);
        path.steps[0].destination = crate::math::Vec3 {
            x: 3.0,
            y: 4.0,
            z: 120.0,
        };
        path.steps[0].distance_to_goal = 100.0;
        path.steps[1].destination = crate::math::Vec3 {
            x: 30.0,
            y: 40.0,
            z: 0.0,
        };
        let current = distance_to_waypoint(&path, crate::math::Vec3::default());
        assert_eq!(current, 1.0, "3-4-5 XY length in metres; Z is ignored");
        assert!(path.distance_left(crate::math::Vec3::default(), None) > current);
    }

    #[test]
    fn rotation_uses_per_frame_delta_and_wraps_like_mover_rotate() {
        assert_eq!(rotated_angle(180.0, 24.0), -156.0);
        assert_eq!(rotated_angle(-180.0, -24.0), 156.0);
        assert_eq!(rotated_angle(180.0, 0.0), 180.0);
    }
}

// ---- save/load (lane B). No field of these types is static: every mover field is set by spawn or changes during play. ----

use crate::save::{Reader, SaveResult, Snapshot, Writer};

// `Vec3` (mc2_path) is implemented in save.rs; not repeated here.

// Path buffers (mc2-path). Pilot code does not hold a MovePath yet (see the report); these are ready for it.
crate::snapshot_struct!(mc2_path::PathStep {
    cell,
    distance_to_goal,
    destination,
    direction,
    area,
});

crate::snapshot_struct!(mc2_path::MovePath {
    goal,
    target,
    num_steps,
    num_steps_when_not_paused,
    cur_step,
    cost,
    steps,
    marked,
    global_step,
});

crate::snapshot_struct!(CrashState {
    avoid_self,
    avoid_path,
    block_self,
    block_path,
    yield_time,
});

crate::snapshot_struct!(PathLock { cells, len });

crate::snapshot_struct!(MoverFlags {
    exploding,
    withdrawing,
    salvage_vehicle,
    on_gui,
    start_disabled,
    shut_down_this_frame,
    start_up_this_frame,
    disable_this_frame,
});

crate::snapshot_struct!(MoverCore {
    position_normal,
    velocity,
    move_level,
    max_move_speed,
    team,
    commander,
    group_id,
    squad_id,
    selection_index,
    team_roster_index,
    attack_range,
    last_valid_position,
    pivot_direction,
    crash,
    path_lock,
    path_locks_enabled,
    yield_time_left,
    mark_distance_moved,
    move_center,
    move_radius,
    follow_roads,
    flags,
    creation_time,
    time_left,
    timers,
    pilot_slot,
    last_map_cell,
    teleport,
    suppression_fire,
    cached_los_weapon_node,
    refit_buddy,
    recover_buddy,
});

crate::snapshot_struct!(Control {
    throttle,
    rotate,
    rotate_torso,
    facing_rotate,
    rotate_arms,
    gesture_goal,
    blow_left_arm,
    blow_right_arm,
    pivot,
    is_walking,
    control_type,
    data_type,
});

impl Snapshot for LegStatus {
    fn save(&self, w: &mut Writer) {
        self.0.save(w);
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(LegStatus(Snapshot::load(r)?))
    }
}

crate::snapshot_struct!(MechState {
    m,
    control,
    variant_name,
    variant_id,
    captureable,
    leg_status,
    torso_rotation,
    left_arm_rotation,
    right_arm_rotation,
    in_jump,
    jump_goal,
    last_jump_time,
    num_jump_jets,
    fallen,
    mech_removed,
    failed_piloting_check,
    hit_from_front,
    hit_from_behind,
    left_arm_blown,
    right_arm_blown,
    sent_crippled_msg,
    pivot_turn_rate,
    engine_blow_time,
    center_torso_blow_time,
    loadout,
    max_torso_yaw,
    max_torso_yaw_rate,
});

crate::snapshot_struct!(VehicleState {
    m,
    control,
    velocity_mag,
    turret_rotation,
    is_vehicle_pilot,
    vehicle_pilot_turn,
    can_recover,
    loadout,
    acceleration,
    refitting,
    refit_gesture,
    refit_gesture_frame,
    recovering,
    sweep_time,
    time_in_current,
    cell_to_mine,
});

// `optimal_cells` is static (built by `init_optimal_cells`, mover.cpp:6961) and is not saved; `load` takes the default.
crate::snapshot_struct!(MoveScratch {
    new_rotation,
    distance_to_waypoint,
    last_goal_cell,
    optimal_cells => Default::default(),
});

#[cfg(test)]
mod vehicle_pilot_removal_tests {
    use super::vehicle_pilot_removed;

    #[test]
    fn switch_on_removes_the_crew_on_its_creation_frame_in_view_or_not() {
        assert!(vehicle_pilot_removed(true, true, 4, 5));
        assert!(vehicle_pilot_removed(true, false, 4, 5));
    }

    #[test]
    fn switch_off_spares_the_creation_frame_and_a_visible_crew() {
        assert!(!vehicle_pilot_removed(false, false, 4, 5));
        assert!(!vehicle_pilot_removed(false, true, 9, 5));
        assert!(vehicle_pilot_removed(false, false, 9, 5));
    }

    #[test]
    fn concurrent_worlds_keep_their_own_switch() {
        use std::sync::{Arc, Barrier};
        let barrier = Arc::new(Barrier::new(2));
        let run = |cpp: bool| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let mut w = crate::World::new(crate::rng::RngMode::LegacyMsvc, 1);
                w.cpp_vehicle_pilot_d106 = cpp;
                barrier.wait();
                (0..10_000)
                    .all(|_| vehicle_pilot_removed(w.cpp_vehicle_pilot_d106, true, 4, 5) == cpp)
            })
        };
        let (a, b) = (run(true), run(false));
        assert!(a.join().unwrap() && b.join().unwrap());
    }
}
