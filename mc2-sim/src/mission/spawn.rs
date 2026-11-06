// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Creating a mover: `Mission::createPartObject` (mission.cpp:969-1137) for the parts of the mission file and the shared half of
//! `Mission::addMover` (mission.cpp:1144-1378) for movers that appear later (reinforcements, `create_infantry`, logistics force).
//!
//! The object already has a slot in `ObjectStore::mechs[]` / `vehicles[]` (lane A allocates the slots before any mover is created);
//! this module fills it. Step order follows the C++, because watch ids, sensor registration and the cell a mover occupies depend on it:
//!
//! 1. `getWatchID()` (parts only: the first thing `createPartObject` does, so ids follow the creation order);
//! 2. `init(true, objType)`: type handle, tangible, crash parameters, dynamics, appearance, obscured, `markDistanceMoved = 1000`;
//! 3. `setAwake(active)`, `setHandle`;
//! 4. `init(variant)` (mech) or `init(profile packet)` (vehicle): chassis/packet read into the [`Loadout`](crate::combat::Loadout);
//! 5. `setPilotHandle(pilot)`, `setTeamId(team, true)`, `setSquadId` (parts), `calcWeaponEffectiveness`;
//! 6. `setControl`, `setPosition`, `setLastValidPosition`, `setRotation`, `setCommanderId`, `captureable`;
//! 7. `setExists`, then for pre-destroyed parts the status rules (`SHUTDOWN` for the home commander's, `DISABLED` for the rest) and
//!    `setOnGUI`.
//!
//! Flags a fresh mover carries (digest `flags`): `USEME | AWAKE` from `GameObject::init`, `TANGIBLE` and `SENSOR` from `Mover::init`,
//! `OBSCURED` from the mech/vehicle init; `EXISTS` and `AWAKE` then follow the part. The GUI's `initMechs` adds `SELECTED` later.
//!
//! What is not reproduced here and why (lane dependencies, listed in the porting log): the weapon `facing` node of the inventory (lane F
//! node ids), the sensor system a mech/vehicle owns and its registration with the team sensors (lane D:
//! [`ai::sensors::register_mover`](crate::ai::sensors::register_mover)), the derived combat numbers (lane C:
//! [`combat::init_mover_stats`](crate::combat::init_mover_stats)). `Mover::setPosition` for the start position is
//! [`set_start_position`] (lane B's per-frame version is theirs).

use crate::ids::{CommanderId, ObjectHandle, TeamId, WatchId};
use crate::math::Vec3;
use crate::mission::loadout;
use crate::mission::plan::{MoverClass, PartRec};
use crate::mover::{Control, MechState, MoverCore, MoverFlags, VehicleState};
use crate::object::{flags, Common, Kind, Obj, ObjectClass, ObjectStatus};
use mc2_sim_data::objtype::{ObjectTypeClass, ObjectTypeData};

/// `FIRERANGE_OPTIMAL` (tacordr.h: DEFAULT -4, RAMMING -3, LONGEST -2, OPTIMAL -1). The scaffold's `command::FIRERANGE_OPTIMAL` says -2.
pub const FIRERANGE_OPTIMAL: i32 = -1;
/// `DefaultMechCrash*` / `DefaultGroundVehicleCrash*` initialisers when `gamesys.fit` has no value (mech.cpp:479-483, gvehicl.cpp:255-260).
const DEFAULT_CRASH: (i32, i32, i32, i32) = (1, 1, 1, 1);
/// `DefaultMechCrashYieldTime` (mech.cpp:483).
const DEFAULT_MECH_CRASH_YIELD: f32 = 2.0;
/// `DefaultGroundVehicleCrashYieldTime` (gvehicl.cpp:260), copied to the type (:289) and the instance (:925); the 1.5 in `Mover::init` (mover.cpp:2336) is overwritten by that copy.
const DEFAULT_VEHICLE_CRASH_YIELD: f32 = 2.0;
/// `UNDETERMINED_DEATH` (mover.h:147), unused here because `disable` is a no-op after `setStatus(DISABLED)`.
pub const UNDETERMINED_DEATH: u32 = 4;

/// Everything `createPartObject` / `addMover` hand to the new mover.
#[derive(Clone, Debug)]
pub struct SpawnSpec {
    /// Object type number (`objNumber`).
    pub obj_number: u32,
    /// Chassis variant (mechs).
    pub variant: u32,
    /// Pilot pool slot.
    pub pilot_slot: usize,
    /// `teamId`.
    pub team: i8,
    /// `commanderID`.
    pub commander: i32,
    /// `squadId` (`setSquadId` runs for parts only).
    pub squad: Option<i8>,
    /// `controlType == CONTROL_PLAYER`.
    pub control_player: bool,
    /// Start position (z is whatever the caller passes: -1 for parts).
    pub position: Vec3,
    /// Rotation in degrees.
    pub rotation: f32,
    /// `active`.
    pub active: bool,
    /// `exists`.
    pub exists: bool,
    /// `captureable` (mechs).
    pub captureable: bool,
    /// Pre-destroyed part (`parts[].destroyed`); `None` for `addMover`.
    pub destroyed: Option<bool>,
    /// `baseColor`, `highlightColor1`, `highlightColor2` the mover is painted with (`resetPaintScheme`, mission.cpp:1072, 1302).
    /// A part without colours in the file has the player's prefs here (`init::fill_paint_from_prefs`, mission.cpp:2647-2653).
    pub paint: [u32; 3],
    /// `GroundVehicle::createVehiclePilot` (gvehicl.cpp:2710): the crew unit of a destroyed vehicle. It goes straight to the home team (no
    /// sensor registration, no roster, the pilot's own team is left alone), has no pilot attached while the stats are derived, and is not
    /// put on the GUI. `Some(mark_cell)`: whether `Mover::setPosition` marks the cell (the borrowed pilot has no path steps left).
    pub vehicle_pilot: Option<bool>,
}

impl SpawnSpec {
    /// From a part record.
    pub fn from_part(p: &PartRec) -> SpawnSpec {
        SpawnSpec {
            obj_number: p.obj_number,
            variant: p.variant_num,
            pilot_slot: p.pilot as usize,
            team: p.team_id,
            commander: p.commander_id,
            squad: Some(p.squad_id as u8 as i8),
            control_player: p.control_type == 1,
            position: p.position,
            rotation: p.rotation,
            active: p.active != 0,
            exists: p.exists != 0,
            captureable: p.captureable,
            destroyed: Some(p.destroyed),
            paint: [
                p.base_color.unwrap_or(0),
                p.highlight_color1.unwrap_or(0),
                p.highlight_color2.unwrap_or(0),
            ],
            vehicle_pilot: None,
        }
    }
}

fn base_common(class: ObjectClass, handle: ObjectHandle, type_handle: i32, tonnage: f32) -> Common {
    // GameObject::init: USEME | AWAKE (lane A's `Common::new`); Mover::init: setTangible(true), setFlag(SENSOR) and
    // collisionFreeTime = -1; the mech/vehicle init: setObscured(true), the type handle and the tonnage.
    let mut c = Common::new(class, handle);
    c.type_handle = type_handle;
    c.tonnage = tonnage;
    c.flags |= flags::TANGIBLE | flags::SENSOR | flags::OBSCURED;
    c.collision_free_time = -1.0;
    c
}

/// `Mover::setMoveType` (mover.cpp:4274-4286): `MOVETYPE_GROUND` is level 0, `MOVETYPE_AIR` level 2 (any other type is a `STOP`).
fn move_level_of_type(move_type: i32) -> u8 {
    if move_type == mc2_sim_data::objtype::mech::MOVETYPE_GROUND {
        0
    } else {
        2
    }
}

/// Type crash yield time of a ground vehicle: the `gamesys.fit` value, else `DefaultGroundVehicleCrashYieldTime` (gvehicl.cpp:260, :289, :925).
fn vehicle_crash_yield(table: Option<f32>) -> f32 {
    table.unwrap_or(DEFAULT_VEHICLE_CRASH_YIELD)
}

/// `Mover::init(bool)` defaults plus the per-type overrides.
#[allow(clippy::too_many_arguments)]
fn base_core(
    max_move_speed: f32,
    crash: (i32, i32, i32, i32, f32),
    max_time_revealed: f32,
    path_locks: bool,
    move_level: u8,
) -> MoverCore {
    MoverCore {
        position_normal: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        velocity: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        move_level,
        max_move_speed,
        team: TeamId(-1),
        commander: CommanderId(-1),
        group_id: -1,
        squad_id: -1,
        selection_index: -1,
        team_roster_index: -1,
        attack_range: FIRERANGE_OPTIMAL,
        last_valid_position: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        pivot_direction: 0,
        crash: crate::mover::CrashState {
            avoid_self: crash.0,
            avoid_path: crash.1,
            block_self: crash.2,
            block_path: crash.3,
            yield_time: crash.4,
        },
        path_lock: crate::mover::PathLock::default(),
        path_locks_enabled: path_locks,
        yield_time_left: 0.0,
        // BattleMech::init / GroundVehicle::init: markDistanceMoved = 1000
        mark_distance_moved: 1000.0,
        recover_buddy: crate::ids::WatchId::NONE,
        move_center: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        move_radius: -1.0,
        follow_roads: false,
        flags: MoverFlags::default(),
        creation_time: 0.0,
        // Mover::init: timeLeft = 1.0
        time_left: 1.0,
        // timeSinceMoving = 0, timeSinceFiredLast = MaxTimeRevealed, lastHustleTime = -999
        timers: [0.0, max_time_revealed, -999.0],
        pilot_slot: 0,
        cached_los_weapon_node: None,
        refit_buddy: crate::ids::WatchId::NONE,
        last_map_cell: (-1, -1),
        teleport: None,
        suppression_fire: 0,
    }
}

/// Build the mover object for the slot `handle` and store it. Returns the new object's watch id.
///
/// `assign_watch_id_first` mirrors `createPartObject` (`parts[i].objectWID = mover->getWatchID()` is its first statement); `addMover`
/// asks for the watch id only later (`modifyMoverLists` / roster), which is the same counter, so the order of calls decides the numbers.
pub fn spawn_into_slot(
    w: &mut crate::World,
    handle: ObjectHandle,
    spec: &SpawnSpec,
) -> Result<WatchId, String> {
    let table = w
        .data
        .object_types
        .clone()
        .ok_or("object types are not loaded")?;
    let comps = w
        .data
        .components
        .clone()
        .ok_or("component list is not loaded")?;
    let tables = w
        .data
        .gamesys
        .tables
        .clone()
        .ok_or("gamesys tables are not loaded")?;
    let ty = table
        .get(spec.obj_number)
        .ok_or_else(|| format!("unable to get objType {}", spec.obj_number))?;
    let max_time_revealed = tables.mover_fire.max_time_revealed;

    let obj = match (&ty.data, ty.object_type_class) {
        (ObjectTypeData::BattleMech(mt), ObjectTypeClass::BattleMech) => {
            let csv =
                ty.base.appearance_name.clone().ok_or_else(|| {
                    format!("mech type {} has no appearance name", spec.obj_number)
                })?;
            let chassis = w
                .data
                .chassis
                .get(&csv.to_ascii_lowercase())
                .cloned()
                .ok_or_else(|| format!("chassis {csv} is not loaded"))?;
            let build = loadout::build_mech(&chassis, spec.variant, &comps)?;
            let t = &tables.mech;
            let crash = (
                t.default_crash_avoid_self.unwrap_or(DEFAULT_CRASH.0),
                t.default_crash_avoid_path.unwrap_or(DEFAULT_CRASH.1),
                t.default_crash_block_self.unwrap_or(DEFAULT_CRASH.2),
                t.default_crash_block_path.unwrap_or(DEFAULT_CRASH.3),
                t.default_crash_yield_time
                    .unwrap_or(DEFAULT_MECH_CRASH_YIELD),
            );
            // `setMoveType(mechT->moveType)` (mech.cpp:1275): an air mech (a helicopter) has moveLevel 2.
            let mut m = base_core(
                build.max_move_speed,
                crash,
                max_time_revealed,
                true,
                move_level_of_type(mt.move_type),
            );
            m.pilot_slot = spec.pilot_slot as u16;
            let state = MechState {
                m,
                control: Control {
                    // BattleMech::init(bool): control.settings.mech.throttle = 100
                    throttle: 100,
                    gesture_goal: -1,
                    // setControl(CONTROL_PLAYER / CONTROL_AI) -> control.init(type, CONTROL_DATA_MECH)
                    control_type: control_kind(spec),
                    data_type: 1,
                    ..Control::default()
                },
                // BattleMech::captureable = parts[i].captureable (mission.cpp:1055)
                captureable: spec.captureable,
                variant_name: build.variant_name.clone(),
                variant_id: build.variant_id,
                max_torso_yaw: chassis.dynamics.max_torso_yaw as f32,
                max_torso_yaw_rate: chassis.dynamics.torso_yaw_rate as f32,
                num_jump_jets: build.num_jump_jets,
                // BattleMech::init(bool): lastJumpTime = -100, centerTorsoBlowTime = -1
                last_jump_time: -100.0,
                center_torso_blow_time: -1.0,
                // Mover::init: engineBlowTime = -1
                engine_blow_time: -1.0,
                loadout: build.loadout,
                ..MechState::default()
            };
            Obj {
                c: base_common(
                    ObjectClass::BattleMech,
                    handle,
                    spec.obj_number as i32,
                    build.tonnage,
                ),
                kind: Kind::Mech(Box::new(state)),
            }
        }
        (ObjectTypeData::GroundVehicle(vt), ObjectTypeClass::Vehicle) => {
            let v = w
                .data
                .vehicle_objects
                .get(&spec.obj_number)
                .cloned()
                .ok_or_else(|| format!("vehicle packet {} is not loaded", spec.obj_number))?;
            let build = loadout::build_vehicle(&v, &comps)?;
            let t = &tables.vehicle;
            let crash = (
                t.default_crash_avoid_self.unwrap_or(DEFAULT_CRASH.0),
                t.default_crash_avoid_path.unwrap_or(DEFAULT_CRASH.1),
                t.default_crash_block_self.unwrap_or(DEFAULT_CRASH.2),
                t.default_crash_block_path.unwrap_or(DEFAULT_CRASH.3),
                vehicle_crash_yield(t.default_crash_yield_time),
            );
            let mut m = base_core(
                build.max_move_speed,
                crash,
                max_time_revealed,
                vt.path_locks,
                // `setMoveType(vehicleT->moveType)`, then `if (vehicleT->hoverCraft) moveLevel = 1` (gvehicl.cpp:926-928).
                if vt.hover_craft {
                    1
                } else {
                    move_level_of_type(vt.move_type)
                },
            );
            // the crew unit's pilot is attached after `init`, by `createVehiclePilot` itself
            m.pilot_slot = if spec.vehicle_pilot.is_some() {
                0
            } else {
                spec.pilot_slot as u16
            };
            // `setFollowRoads(true)` (gvehicl.cpp:929)
            m.follow_roads = true;
            let state = VehicleState {
                m,
                acceleration: build.acceleration,
                control: Control {
                    gesture_goal: -1,
                    // setControl -> control.init(type, CONTROL_DATA_GROUNDVEHICLE)
                    control_type: control_kind(spec),
                    data_type: 2,
                    ..Control::default()
                },
                loadout: build.loadout,
                can_recover: v.recover_points > 0,
                sweep_time: -1.0,
                cell_to_mine: (-1, -1),
                ..VehicleState::default()
            };
            Obj {
                c: base_common(
                    ObjectClass::GroundVehicle,
                    handle,
                    spec.obj_number as i32,
                    build.tonnage,
                ),
                kind: Kind::Vehicle(Box::new(state)),
            }
        }
        _ => {
            return Err(format!(
                "object type {} is neither a battle mech nor a ground vehicle",
                spec.obj_number
            ))
        }
    };
    let is_mech = matches!(obj.kind, Kind::Mech(_));
    let slot = handle.0 as usize;
    if w.objects.list.len() <= slot {
        w.objects.list.resize_with(slot + 1, || None);
    }
    // `newMech`/`newVehicle` already gave the slot its partId (the next reinforcement id, objmgr.cpp:552); the fresh object keeps it
    let part_id = w.objects.get(handle).map(|o| o.c.part_id);
    w.objects.list[slot] = Some(obj);
    if let (Some(part_id), Some(o)) = (part_id, w.objects.get_mut(handle)) {
        o.c.part_id = part_id;
    }
    // getWatchID(): first statement of createPartObject, so ids follow the creation order
    let wid = w.objects.watch_id(handle);

    // init(true, objType): the appearance
    create_appearance(w, handle, is_mech, &ty.base.appearance_name)?;

    // setAwake(active): GameObject::setAwake (the pilot is not attached yet, so no power-up order)
    if let Some(o) = w.objects.get_mut(handle) {
        if spec.active {
            o.c.flags |= flags::AWAKE;
        } else {
            o.c.flags &= !flags::AWAKE;
        }
    }

    // setPilotHandle(pilot): pilot->setVehicle(this)
    if spec.vehicle_pilot.is_none() {
        let slot = spec.pilot_slot;
        if slot == 0 || slot >= w.pilots.slots.len() {
            return Err(format!(
                "mover of type {}: bad pilot slot {slot}",
                spec.obj_number
            ));
        }
        w.pilots.slots[slot].mover = wid;
    }

    // setTeamId(team, true): sensor registration (lane D) and pilot->setTeam
    if spec.vehicle_pilot.is_some() {
        // `vehiclePilot->teamId = Team::home->getId()` (gvehicl.cpp:2778)
        if let Some(core) = mover_core_mut(w, handle) {
            core.team = TeamId(spec.team);
        }
    } else {
        set_team(w, handle, spec.team, spec.pilot_slot)?;
    }

    if let Some(sq) = spec.squad {
        if let Some(core) = mover_core_mut(w, handle) {
            core.squad_id = i32::from(sq);
        }
    }
    crate::combat::init_mover_stats(w, handle)?;

    // setControl: stored in the spawn above (`Control::control_type`/`data_type`)

    // setPosition(parts[i].position): Mover::setPosition, then setLastValidPosition, setRotation, setCommanderId
    let position =
        set_position_marking(w, handle, spec.position, spec.vehicle_pilot.unwrap_or(true));
    if let Some(o) = w.objects.get_mut(handle) {
        o.c.rotation = spec.rotation;
        match &mut o.kind {
            Kind::Mech(m) => {
                m.m.last_valid_position = position;
                m.m.commander = CommanderId(spec.commander as i8);
            }
            Kind::Vehicle(v) => {
                v.m.last_valid_position = position;
                v.m.commander = CommanderId(spec.commander as i8);
            }
            _ => {}
        }
    }

    // captureable (mechs only; vehicles take it from the type's resource points)
    if let Some(o) = w.objects.get_mut(handle) {
        if let Kind::Vehicle(_) = &o.kind {
            // GroundVehicle::init(bool, type): captureable = captureable || (resourcePoints > 0)
            if let ObjectTypeData::GroundVehicle(vt) = &ty.data {
                if vt.resource_points > 0 {
                    o.c.flags |= flags::CAPTURABLE;
                }
            }
        }
    }

    // setExists
    if let Some(o) = w.objects.get_mut(handle) {
        o.set_exists(spec.exists);
    }
    // Paint is appearance state in the original (Mech3DAppearance::getPaintScheme); the headless mover keeps the scheme it was
    // created with here, for the campaign outcome. TODO(lane F): move onto the mech appearance when lane F models paint.
    let paint = (wid, spec.paint);
    match w
        .mission
        .paint_schemes
        .iter_mut()
        .find(|(w2, _)| *w2 == wid)
    {
        Some(slot) => *slot = paint,
        None => w.mission.paint_schemes.push(paint),
    }
    Ok(wid)
}

/// `setControl(parts[i].controlType == CONTROL_PLAYER ? CONTROL_PLAYER : CONTROL_AI)` (single player: `MPlayer == NULL`, mission.cpp:1043).
fn control_kind(spec: &SpawnSpec) -> u8 {
    if spec.control_player {
        1
    } else {
        2
    }
}

/// `Mover::setPosition(position)` as `createPartObject`/`addMover` use it for the start position (mover.cpp:2760-2838): clamp to the
/// playable map, `GameObject::setPosition` (cell and vertex), the trigger areas, release a
/// previously marked cell and mark the current one as occupied (the pilot has no move path yet). Returns the position after the clamp
/// (the C++ clamps the caller's vector in place, so `parts[i].position` changes too). The withdrawing branch (a mover leaving the map is
/// destroyed) cannot happen at load.
pub fn set_start_position(w: &mut crate::World, h: ObjectHandle, p: Vec3) -> Vec3 {
    set_position_marking(w, h, p, true)
}

/// [`set_start_position`] with the last step of `Mover::setPosition` optional: the cell is marked only `if (pilot->getMovePath()->numSteps
/// == 0)` (mover.cpp:2832), which `mark_cell` stands for.
fn set_position_marking(
    w: &mut crate::World,
    h: ObjectHandle,
    mut p: Vec3,
    mark_cell: bool,
) -> Vec3 {
    let max_map = w.terrain.world_units_map_side / 2.0;
    p.x = p.x.clamp(-max_map, max_map);
    p.y = p.y.clamp(-max_map, max_map);
    // The second clip (the diamond the camera can show, `maxVisual = worldUnitsPerVertex * blocksMapSide * verticesBlockSide / 2 -
    // 1300`) and `offMap` only matter to a withdrawing mover (destroyed when it leaves the map): never at load.
    crate::object::set_position(w, h, p, true);
    crate::object::trigger::set_hit(w, h);
    let (row, col) = match w.objects.get(h) {
        Some(o) => (i32::from(o.c.cell.0), i32::from(o.c.cell.1)),
        None => return p,
    };
    let last = mover_core_mut(w, h)
        .map(|c| c.last_map_cell)
        .unwrap_or((-1, -1));
    if last.0 != -1 && last != (row, col) {
        w.terrain.set_mover(last.0, last.1, false);
        if let Some(c) = mover_core_mut(w, h) {
            c.last_map_cell = (-1, -1);
        }
    }
    // pilot->getMovePath()->numSteps == 0: a new pilot has no path
    if mark_cell {
        w.terrain.set_mover(row, col, true);
        if let Some(c) = mover_core_mut(w, h) {
            c.last_map_cell = (row, col);
        }
    }
    p
}

fn mover_core_mut(w: &mut crate::World, h: ObjectHandle) -> Option<&mut MoverCore> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::Mech(m) => Some(&mut m.m),
        Kind::Vehicle(v) => Some(&mut v.m),
        _ => None,
    }
}

/// `Mover::setTeamId(team, true)`.
pub fn set_team(
    w: &mut crate::World,
    h: ObjectHandle,
    team: i8,
    pilot_slot: usize,
) -> Result<(), String> {
    if team < 0
        || usize::from(team as u8) >= w.teams.teams.len()
        || w.teams.teams[team as usize].is_none()
    {
        return Err(format!("Mover.setTeamId: NULL team {team}"));
    }
    if let Some(core) = mover_core_mut(w, h) {
        core.team = TeamId(team);
    }
    crate::ai::sensors::register_mover(w, h);
    // `pilot->setTeam(team)` (mover.cpp:2682): use-goal-plan and keep-moving for every pilot off the home team
    crate::ai::warrior::set_team(w, pilot_slot as u16, Some(team as usize));
    if let Some(l) = w.mission.pilot_loads.get_mut(pilot_slot) {
        l.team = i32::from(team);
    }
    Ok(())
}

/// The status rules of `createPartObject` for a part that starts destroyed (mission.cpp:1086-1110): the home commander's mover starts
/// `SHUTDOWN` and off the GUI, any other starts `DISABLED` (`disable()` is a no-op then: `isDisabled()` is already true) and on the GUI.
pub fn apply_destroyed_part(w: &mut crate::World, h: ObjectHandle, home_commander: i8) {
    let is_home = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.m.commander.0 == home_commander,
        Some(Kind::Vehicle(v)) => v.m.commander.0 == home_commander,
        _ => false,
    };
    if let Some(o) = w.objects.get_mut(h) {
        let (status, start_disabled, on_gui) = if is_home {
            (ObjectStatus::ShutDown, false, false)
        } else {
            (ObjectStatus::Disabled, true, true)
        };
        o.c.status = status;
        match &mut o.kind {
            Kind::Mech(m) => {
                m.m.flags.start_disabled = start_disabled;
                m.m.flags.on_gui = on_gui;
            }
            Kind::Vehicle(v) => {
                v.m.flags.start_disabled = start_disabled;
                v.m.flags.on_gui = on_gui;
            }
            _ => {}
        }
    }
}

/// `setOnGUI(true)` for a part that is not destroyed.
pub fn set_on_gui(w: &mut crate::World, h: ObjectHandle, on_gui: bool) {
    if let Some(core) = mover_core_mut(w, h) {
        core.flags.on_gui = on_gui;
    }
}

/// `createPartObject` (mission.cpp:969) for one planned mover. Writes the new watch id into the part record.
pub fn create_part_object(
    w: &mut crate::World,
    part_index: usize,
    class: MoverClass,
    slot: usize,
    home_commander: i8,
) -> Result<(), String> {
    let handle = match class {
        MoverClass::Mech => w.objects.mechs.get(slot).copied(),
        MoverClass::Vehicle => w.objects.vehicles.get(slot).copied(),
    }
    .ok_or_else(|| format!("no {class:?} slot {slot} for part {part_index}"))?;
    let spec = SpawnSpec::from_part(&w.mission.parts[part_index]);
    let wid = spawn_into_slot(w, handle, &spec)?;
    w.mission.parts[part_index].object_wid = wid;
    // `setPosition(parts[i].position)` clamps the part's own vector (passed by reference)
    if let Some(o) = w.objects.get(handle) {
        w.mission.parts[part_index].position = o.c.position;
    }
    let destroyed = w.mission.parts[part_index].destroyed;
    if destroyed {
        apply_destroyed_part(w, handle, home_commander);
    } else {
        // setOnGUI(true): "Even for non-player Team Parts. It only FALSE if we link up with it later!!!!!"
        set_on_gui(w, handle, true);
    }
    Ok(())
}

/// One `addMover` pilot creation, in creation order (`MissionState::spawn_log`). The pilot's profile and brain module/instance are
/// structure that a save depends on (`Env::load_state` needs the same instances), and the base world of `save::load_world` does not
/// have them: [`replay_spawn`] recreates them there from this record.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpawnRecord {
    /// The pilot pool slot `add_mover` handed out.
    pub pilot_slot: usize,
    /// `pilotFileName` as the caller gave it (the profile cache key is its lower-case form).
    pub pilot_file: String,
    /// `brainFileName`.
    pub brain: String,
    /// `objectNumber` of the mover the pilot drives: its chassis and mech appearance type are loaded by the spawn
    /// (`init::prepare_spawn`), so the base world needs them too.
    pub object_number: u32,
}

crate::snapshot_struct!(SpawnRecord {
    pilot_slot,
    pilot_file,
    brain,
    object_number,
});

/// Recreates what `add_mover` left in the world for one reinforcement, on a freshly initialised base world (`save::load_world`):
/// the chassis, mech appearance type and pilot profile the caller prepared (`init::prepare_spawn_in`, which needs `vfs`), and the
/// brain's module and instance, appended in the original creation order. The sim draws of `init_pilot` are not replayed: the save
/// overwrites the RNG. A brain that fails to compile fails here as it did in `add_mover` (the record is kept, as the pilot slot was used).
///
/// Without `vfs` the base must already hold the profile (`init::prepare_spawn` on it); the appearance types are then not loaded, so
/// a save with a new appearance type fails in `AppearanceSystem::load_into`.
pub fn replay_spawn(
    w: &mut crate::World,
    vfs: Option<&mut mc2_formats::vfs::Vfs>,
    rec: &SpawnRecord,
) -> Result<(), String> {
    if let Some(vfs) = vfs {
        crate::mission::init::prepare_spawn_in(
            w,
            vfs,
            &[rec.object_number],
            &[rec.pilot_file.as_str()],
        )?;
    }
    let key = rec.pilot_file.to_ascii_lowercase();
    // a file the game data does not have is cached as `None`: the default pilot, as `add_mover` treats it
    let profile = w
        .data
        .pilot_profiles
        .get(&key)
        .cloned()
        .ok_or_else(|| {
            format!(
                "pilot file {} is not loaded in the base world (pass the game data, or call init::prepare_spawn with it first)",
                rec.pilot_file
            )
        })?;
    let load = w
        .mission
        .pilot_loads
        .get_mut(rec.pilot_slot)
        .ok_or_else(|| format!("pilot slot {} is out of range", rec.pilot_slot))?;
    load.profile = profile;
    let _ = crate::mission::abl_load::make_pilot_brain(w, rec.pilot_slot, &rec.brain);
    w.mission.spawn_log.push(rec.clone());
    Ok(())
}

/// `Mission::addMover(MoverInitData*)` (mission.cpp:1144-1378): a pilot and a mover that appear after the load (reinforcements, the
/// infantry of `create_infantry`, the logistics force). Returns the new mover's handle (an `Err` is the C++ `-1`/`0`).
///
/// Order: `newWarrior` + `MechWarrior::init(profile)` (one sim draw) + optional skill override + brain; `newMech`/`newVehicle`; the
/// shared mover init ([`spawn_into_slot`]); then `setLocalMoverId`, `modifyMoverLists(ADD)`, `Team::addToRoster`, the GUI hook
/// `missionInterface->addMover` (selection flag only), and the "new mover" notification of every existing, not disabled mover: its
/// sensor scans the newcomer and its pilot gets `PILOT_ALARM_NEW_MOVER`.
pub fn add_mover(
    w: &mut crate::World,
    spec: &crate::mission::init::MoverInitData,
) -> Result<ObjectHandle, String> {
    use crate::mission::pilots;
    // the pilot first (mission.cpp:1146-1175)
    if spec.team < 0 || spec.team as usize >= crate::ids::MAX_TEAMS {
        return Err(format!("addMover: bad team {}", spec.team));
    }
    if spec.commander < 0 || spec.commander as usize >= crate::ids::MAX_COMMANDERS {
        return Err(format!("addMover: bad commander {}", spec.commander));
    }
    // The C++ opens the pilot file here. The cache (`init::prepare_spawn`) holds every file the caller named: `None` for a file that is
    // missing (`pmw00080`), which leaves a default pilot as in the C++; a file nobody loaded is an error, never a silent default.
    let profile = match w
        .data
        .pilot_profiles
        .get(&spec.pilot_file.to_ascii_lowercase())
    {
        Some(p) => p.clone(),
        None => {
            return Err(format!(
                "addMover: pilot file {} was not loaded (call init::prepare_spawn first)",
                spec.pilot_file
            ))
        }
    };
    let slot = pilots::new_warrior(&mut w.mission.pilot_loads)
        .ok_or("Too many pilots in this mission!")?;
    {
        let streams = w.rng.0.clone();
        let mut r = streams.borrow_mut();
        let _progress = pilots::init_pilot(
            &mut w.pilots,
            &mut w.mission.pilot_loads,
            slot,
            profile,
            &spec.brain,
            &mut r.sim,
        );
    }
    if spec.override_loaded_pilot {
        // skills[] = skillRank[] = value; specialtySkills copied; calcRank() (the ejection roll is not redone)
        let p = &mut w.pilots.slots[slot];
        let l = &mut w.mission.pilot_loads[slot];
        use mc2_sim_data::pilot::Skill;
        l.skill_rank[Skill::Gunnery.index()] = spec.gunnery_skill;
        p.skills[Skill::Gunnery.index()] = i32::from(spec.gunnery_skill as i8);
        l.skill_rank[Skill::Piloting.index()] = spec.piloting_skill;
        p.skills[Skill::Piloting.index()] = i32::from(spec.piloting_skill as i8);
        p.specialty = spec.specialty_skills;
        let avg =
            (l.skill_rank[Skill::Gunnery.index()] + l.skill_rank[Skill::Piloting.index()]) / 2.0;
        l.rank = Some(mc2_sim_data::pilot::rank_for_average(avg));
    }
    // logged before the brain: a brain that fails to compile fails here, and `replay_spawn` fails the same way
    w.mission.spawn_log.push(SpawnRecord {
        pilot_slot: slot,
        pilot_file: spec.pilot_file.clone(),
        brain: spec.brain.clone(),
        object_number: spec.object_number as u32,
    });
    // Empty is reserved for a caller that creates the mover while separately reporting its missing AI behavior.
    if !spec.brain.is_empty() {
        if w.abl_running && spec.brain.eq_ignore_ascii_case("infantry001") {
            w.deferred_brain_calls
                .push(crate::ai::brain::DeferredBrainCall::AttachInfantry(
                    slot as u16,
                ));
        } else {
            crate::mission::abl_load::make_pilot_brain(w, slot, &spec.brain)?;
        }
    }

    // the mover (mission.cpp:1215-1300)
    let table = w
        .data
        .object_types
        .clone()
        .ok_or("object types are not loaded")?;
    let ty = table
        .get(spec.object_number as u32)
        .ok_or_else(|| format!("unknown object type {}", spec.object_number))?;
    let class = match ty.data {
        ObjectTypeData::BattleMech(_) => ObjectClass::BattleMech,
        ObjectTypeData::GroundVehicle(_) => ObjectClass::GroundVehicle,
        _ => return Err(format!("object type {} is not a mover", spec.object_number)),
    };
    let handle = w
        .objects
        .new_mover(class)
        .ok_or("no free mover slot (newMech/newVehicle)")?;
    let sspec = SpawnSpec {
        obj_number: spec.object_number as u32,
        variant: spec.variant,
        pilot_slot: slot,
        team: spec.team,
        commander: i32::from(spec.commander),
        squad: None,
        control_player: spec.control_type == 1,
        position: spec.position,
        rotation: spec.rotation as f32,
        active: spec.active,
        exists: spec.exists,
        captureable: spec.capturable,
        destroyed: None,
        paint: spec.colors,
        vehicle_pilot: None,
    };
    let wid = spawn_into_slot(w, handle, &sspec)?;

    // home commander movers get localMoverId 0 (not modeled); lists and roster
    w.objects
        .modify_mover_lists(&w.teams, handle, crate::object::store::MoverListAction::Add);
    if let Some(Some(team)) = w.teams.teams.get_mut(spec.team as usize) {
        team.roster.push(wid);
    }
    // `Mover::init` starts in unit group 1 (`mover.cpp:2217,2278`).  Unlike a mission
    // part, this reinforcement's part id is already its final id, so it can be keyed now.
    if let Some(part_id) = w.objects.get(handle).map(|obj| obj.c.part_id) {
        w.mission.unit_groups.insert(part_id, 1);
    }
    // missionInterface->addMover(mover): GUI group bar (selection is added by initMechs/`ControlGui::addMover` for the home team only)
    // `ControlGui::addMover` clears the constructor's group-1 membership for a home mover added after turn 3
    // (controlgui.cpp:1293-1304). Initial units still take their group-1 default through `initMechs`.
    if i32::from(spec.commander) == crate::combat::HOME_COMMANDER && w.clock.turn > 3 {
        if let Some(part_id) = w.objects.get(handle).map(|obj| obj.c.part_id) {
            w.mission.unit_groups.remove(&part_id);
        }
    }
    // every existing, not disabled mover: sensorSystem->scanMover(mover) and triggerAlarm(PILOT_ALARM_NEW_MOVER, wid)
    let others: Vec<ObjectHandle> = w.objects.movers.clone();
    for other in others {
        let disabled = w
            .objects
            .get(other)
            .map(|o| matches!(o.c.status, ObjectStatus::Disabled | ObjectStatus::Destroyed))
            .unwrap_or(true);
        if !disabled {
            crate::ai::sensors::scan_mover(w, other, handle);
            crate::ai::trigger_alarm(w, other, crate::ai::PILOT_ALARM_NEW_MOVER, wid.0 as i32);
        }
    }
    // resetComponents(numComponents, components) for logistics mechs: the custom loadout (lane C edits the inventory)
    if let (Some(name), Some(o)) = (&spec.variant_name, w.objects.get_mut(handle)) {
        if let Kind::Mech(m) = &mut o.kind {
            m.variant_name = name.clone();
        }
    }
    if !spec.components.is_empty() || spec.custom_loadout {
        if !matches!(class, ObjectClass::BattleMech) {
            return Err("LogisticsMech was not a MECH!!".into());
        }
        crate::combat::reset_components(w, handle, &spec.components)?;
    }
    // setOnGUI(true) (sebi: "set it so that object will be drawn")
    set_on_gui(w, handle, true);
    Ok(handle)
}

/// `VEHICLEPILOT_ID`: the infantry "vehicle pilot" object type (gvehicl.cpp:2707).
pub const VEHICLEPILOT_ID: u32 = 638;
/// Colours of the crew unit's `resetPaintScheme(0x00ffffff, 0x00ffffff, 0x00ff8000)` (gvehicl.cpp:2803).
const VEHICLE_PILOT_PAINT: [u32; 3] = [0x00ff_ffff, 0x00ff_ffff, 0x00ff_8000];

/// `GroundVehicle::createVehiclePilot` (gvehicl.cpp:2710-2807): the crew of the destroyed vehicle `from` bails out as a one-hit-point
/// infantry unit of the home team (commander 1, so the GUI shows it as an enemy) standing where the vehicle blew up. It takes the first
/// vehicle slot that does not exist (`getOpenVehicle`), keeps the vehicle's own pilot (its brain is removed, `lobotomy`) and is ordered
/// to run to the map origin. `Ok(false)` when the C++ returns without creating it.
pub fn create_vehicle_pilot(w: &mut crate::World, from: ObjectHandle) -> Result<bool, String> {
    let Some(obj) = w.objects.get(from) else {
        return Ok(false);
    };
    let Kind::Vehicle(v) = &obj.kind else {
        return Ok(false);
    };
    // We are ALREADY a vehicle pilot, stop breeding. / Don't let powered armor or infantry create a vehicle pilot.
    if v.is_vehicle_pilot || !v.m.path_locks_enabled {
        return Ok(false);
    }
    let (position, slot) = (obj.c.position, v.m.pilot_slot);
    let Some(open) = w.objects.open_vehicle() else {
        return Ok(false);
    };
    let has_type = w
        .data
        .object_types
        .as_ref()
        .is_some_and(|t| t.get(VEHICLEPILOT_ID).is_some());
    if !has_type {
        return Ok(false);
    }
    // No guys allowed to come out in the water!!
    let (row, col) = w.terrain.world_to_cell(&position);
    if w.terrain.get_deep_water(row, col) || w.terrain.get_shallow_water(row, col) {
        return Ok(false);
    }
    // `Mover::setPosition` marks the cell only for a pilot without path steps; `getMovePath()` is called there, after setVehicle
    let no_steps = crate::ai::warrior::get_move_path_index(w, slot)
        .and_then(|i| {
            w.pilots
                .get(slot)
                .map(|p| p.lane_b_move.paths[i].num_steps == 0)
        })
        .unwrap_or(true);
    let spec = SpawnSpec {
        obj_number: VEHICLEPILOT_ID,
        variant: 0,
        pilot_slot: usize::from(slot),
        // They get added to the home team so they are always visible. BUT they are enemies on the GUI!!
        team: w.teams.home as i8,
        commander: 1,
        squad: None,
        control_player: false,
        position,
        rotation: 0.0,
        active: true,
        exists: true,
        captureable: false,
        destroyed: None,
        paint: VEHICLE_PILOT_PAINT,
        vehicle_pilot: Some(no_steps),
    };
    let wid = spawn_into_slot(w, open, &spec)?;
    let born = w.clock.turn + 1;
    if let Some(Kind::Vehicle(v)) = w.objects.get_mut(open).map(|o| &mut o.kind) {
        v.is_vehicle_pilot = true;
        v.vehicle_pilot_turn = born;
        v.m.pilot_slot = slot;
    }
    // pilot->setVehicle(vehiclePilot); pilot->lobotomy()
    if let Some(p) = w.pilots.get_mut(slot) {
        p.mover = wid;
        p.brain = None;
        p.brain_alarm_handlers = [false; crate::ai::pilot::NUM_ALARMS];
    }
    // `getOpenVehicle` only raises `numVehicles` to the slot index, and `buildMoverLists` counts `< numVehicles`, so the C++ leaves the
    // first crew out of the mover lists (never updated, drawn or selectable). Count the slot so the crew is a mover (DIFFERENCES.md).
    // The test-only `D-106` switch keeps the C++ lists.
    let slot_index = w.objects.vehicles.iter().position(|&v| v == open);
    if let Some(i) = slot_index.filter(|_| !w.cpp_vehicle_pilot_d106) {
        w.objects.num_vehicles = w.objects.num_vehicles.max(i + 1);
    }
    w.objects.build_mover_lists(&w.teams);
    // We need to override the TacOrder of the pilot so that the little marine goes running about like he is trying to escape.
    crate::ai::warrior::clear_attack_orders(w, slot);
    crate::ai::warrior::clear_move_orders(w, slot);
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
    if let Some(o) = w.objects.get_mut(open) {
        o.c.set_tangible(true);
    }
    w.objects.rebuild_collidables = true;
    Ok(true)
}

/// `appearance = new Mech3DAppearance / GVAppearance; appearance->init(type, owner)` (mech.cpp:1304, gvehicl.cpp:970).
///
/// RNG: `new Mech3DAppearance` runs `Mech3DAppearance::init` in its constructor and `appearance->init(type, this)` runs it again;
/// both draw `idleTime = RandomNumber(idleMAX)` (mech3d.cpp:1023), a SIM draw since the scaffold's RNG audit. The first value is
/// overwritten, so lane F's single-draw `create_mech` gets the second; the first is drawn here. The mech appearance type must be loaded
/// (`AppearanceSystem::mech_type`, done by `populate` for the mission's types); an unknown name falls back to the MADCAT like the C++.
fn create_appearance(
    w: &mut crate::World,
    h: ObjectHandle,
    is_mech: bool,
    appearance_name: &Option<String>,
) -> Result<(), String> {
    if !is_mech {
        // `GroundVehicle::init`: the vehicle's appearance type, or the APC when it has none (gvehicl.cpp:962-977).
        let find = |sys: &crate::appearance::AppearanceSystem, n: &str| {
            sys.vehicle_types
                .iter()
                .position(|t| t.name.eq_ignore_ascii_case(n))
        };
        let idx = appearance_name
            .as_deref()
            .and_then(|n| find(&w.appearance, n))
            .or_else(|| find(&w.appearance, "APC"));
        match idx {
            Some(idx) => w.appearance.create_vehicle(h, idx),
            None => {
                return Err("vehicle appearance type (and the APC fallback) is not loaded".into())
            }
        }
        return Ok(());
    }
    let name = appearance_name
        .clone()
        .ok_or("mech type has no appearance name")?;
    let find = |sys: &crate::appearance::AppearanceSystem, n: &str| {
        sys.mech_types
            .iter()
            .position(|t| t.name.eq_ignore_ascii_case(n))
    };
    let idx = find(&w.appearance, &name)
        .or_else(|| find(&w.appearance, "MADCAT"))
        .ok_or_else(|| format!("mech appearance type {name} is not loaded"))?;
    let streams = w.rng.0.clone();
    let mut r = streams.borrow_mut();
    r.sim.random_number(IDLE_MAX);
    w.appearance.create_mech(h, idx, &mut r.sim);
    Ok(())
}

/// `idleMAX` (mech3d.cpp:133), the range of the idle timer draw.
const IDLE_MAX: i32 = 20;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::RngMode;

    fn world_with_mover(commander: i8) -> (crate::World, ObjectHandle) {
        let mut w = crate::World::new(RngMode::LegacyMsvc, 1);
        let h = ObjectHandle(1);
        let mut m = MechState::default();
        m.m.commander = CommanderId(commander);
        w.objects.list = vec![
            None,
            Some(Obj {
                c: base_common(ObjectClass::BattleMech, h, 7, 50.0),
                kind: Kind::Mech(Box::new(m)),
            }),
        ];
        (w, h)
    }

    #[test]
    fn a_fresh_mover_carries_the_flags_and_timers_of_the_c_plus_plus_init() {
        let c = base_common(ObjectClass::BattleMech, ObjectHandle(3), 7, 50.0);
        // USEME | AWAKE | TANGIBLE | SENSOR | OBSCURED: the digest's 0x300017 before EXISTS/SELECTED
        assert_eq!(c.flags, 0x0030_0015);
        assert_eq!(c.collision_free_time, -1.0);
        let core = base_core(80.0, (1, 1, 1, 1, 2.0), 10.0, true, 0);
        assert_eq!(core.time_left, 1.0);
        assert_eq!(core.timers, [0.0, 10.0, -999.0]);
        assert_eq!(core.mark_distance_moved, 1000.0);
        assert_eq!(core.attack_range, FIRERANGE_OPTIMAL);
        assert_eq!(
            (core.group_id, core.squad_id, core.selection_index),
            (-1, -1, -1)
        );
    }

    #[test]
    fn exists_on_top_of_the_init_flags_gives_the_oracle_flag_words() {
        // GameObject::init + Mover::init + BattleMech::init + setExists(true) = 0x300017; initMechs adds SELECTED (0x20) = 0x300037
        let (mut w, h) = world_with_mover(1);
        let o = w.objects.get_mut(h).unwrap();
        o.set_exists(true);
        assert_eq!(o.c.flags, 3_145_751);
        o.c.flags |= flags::SELECTED;
        assert_eq!(o.c.flags, 3_145_783);
    }

    #[test]
    fn destroyed_parts_start_shut_down_for_the_home_commander_and_disabled_for_others() {
        let (mut w, h) = world_with_mover(0);
        apply_destroyed_part(&mut w, h, 0);
        let o = w.objects.get(h).unwrap();
        assert_eq!(o.c.status, ObjectStatus::ShutDown);
        let Kind::Mech(m) = &o.kind else { panic!() };
        assert!(!m.m.flags.start_disabled && !m.m.flags.on_gui);

        let (mut w, h) = world_with_mover(1);
        apply_destroyed_part(&mut w, h, 0);
        let o = w.objects.get(h).unwrap();
        assert_eq!(o.c.status, ObjectStatus::Disabled);
        let Kind::Mech(m) = &o.kind else { panic!() };
        assert!(m.m.flags.start_disabled && m.m.flags.on_gui);
    }
}

#[cfg(test)]
mod crash_yield_tests {
    use super::*;

    /// gvehicl.cpp:260/289/925: with no `CrashYieldTime` in `[GroundVehicle:Movement]` a ground vehicle yields for 2.0 s (a data value wins, gvehicl.cpp:778-780, read by `VehicleTables`).
    #[test]
    fn ground_vehicle_crash_yield_defaults_to_two_seconds() {
        assert_eq!(vehicle_crash_yield(None), 2.0);
        assert_eq!(vehicle_crash_yield(Some(0.75)), 0.75);
        assert_eq!(DEFAULT_MECH_CRASH_YIELD, 2.0);
    }
}
