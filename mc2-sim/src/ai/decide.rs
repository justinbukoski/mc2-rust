// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The pilot's per-frame decision code: `MechWarrior::mainDecisionTree` (warrior.cpp:4899), `checkAlarms` (4743) and its handlers
//! (6384-6990), `updateActions` (4867), `calcWeaponsStatus` (3377), `combatDecisionTree` (3492) and `movementDecisionTree` (3910).
//!
//! DESIGN.md names `MechWarrior::update` as the entry, but that method is declared and never defined: the per-frame entry is
//! `mainDecisionTree`, called from `updateAIControl` of the mover update (lane B's `update_mech` / `update_vehicle` call
//! [`crate::ai::pilot_update`]).
//!
//! What is NOT ported here is reported through [`AiUnavailable`]: the team-contact line of sight (`Team::teamLineOfSight`).

// The branch structure mirrors warrior.cpp one to one (including branches that set the same values): keep it greppable against the source.
#![allow(clippy::collapsible_match, clippy::if_same_then_else)]

use super::moveorders::{self as mo, MOVEGOAL_LOCATION, MOVEGOAL_NONE, MOVESTATE_PIVOT_TARGET};
use super::orders;
use super::pilot::{
    OrderState, WEAPONS_STATUS_CANNOT_FIRE, WEAPONS_STATUS_NO_TARGET, WEAPONS_STATUS_OUT_OF_RANGE,
    WEAPON_STATUS_NOT_LOCKED, WEAPON_STATUS_NOT_READY, WEAPON_STATUS_NO_CHANCE,
    WEAPON_STATUS_OUT_OF_AMMO, WEAPON_STATUS_OUT_OF_RANGE, WEAPON_STATUS_TOO_HOT,
};
use super::tacorder;
use super::warrior::{
    self, alarm, clear_attack_orders, clear_cur_tac_order, clear_move_orders, commander_is_home,
    get_last_target, get_move_path_index, now, request_move_path, set_last_target, set_main_goal,
    start_order, team_id, trigger_alarm, under_home_command, vehicle, BRAIN_UPDATE_FREQUENCY,
    TACORDER_SUCCESS,
};
use super::{mover_ops, report_unavailable, AiUnavailable};
use crate::command::{OrderCode, OrderOrigin, Tactic, TacticalOrder};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::object::{Kind, ObjectClass};
use crate::World;
use mc2_path::movemap::{
    MOVEPARAM_FACE_TARGET, MOVEPARAM_INIT, MOVEPARAM_JUMP, MOVEPARAM_NONE,
    MOVEPARAM_RANDOM_OPTIMAL, MOVEPARAM_RECALC, MOVEPARAM_SOMEWHERE_ELSE,
    MOVEPARAM_STEP_ADJACENT_TARGET, MOVEPARAM_STEP_TOWARD_TARGET,
};

/// `UNLIMITED_SHOTS` (mover.h).
const UNLIMITED_SHOTS: i32 = 9999;
/// `CELL_DISTANCE` (`warrior.cpp:118`), passed to `Team::teamLineOfSight`
/// for an indirect weapon aimed at a point (`warrior.cpp:3431`).  This is
/// deliberately not `MapCellDiagonal`: it controls the endpoint terrain-ray
/// exemption, rather than weapon-range tolerance.
pub(super) const POINT_TARGET_TEAM_LOS_EXTENT: f32 = 42.6667;

fn tables(w: &World) -> Option<std::rc::Rc<mc2_sim_data::gamesys::GameSystem>> {
    w.data.gamesys.tables.clone()
}

/// `MovementUpdateFrequency` / `CombatUpdateFrequency` / `MoveTimeOut` / `MoveYieldTime` (gamesys.fit).
fn movement_update_frequency(w: &World) -> f32 {
    tables(w).map_or(0.0, |t| t.warrior.movement_update_frequency)
}
fn combat_update_frequency(w: &World) -> f32 {
    tables(w).map_or(0.0, |t| t.warrior.combat_update_frequency)
}
fn move_time_out(w: &World) -> f32 {
    tables(w).map_or(30.0, |t| t.pathfinding.move_time_out)
}
fn move_yield_time(w: &World) -> f32 {
    tables(w).map_or(1.5, |t| t.pathfinding.move_yield_time)
}

// ---------------------------------------------------------------------------------------------------------------------
// Order queue
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::executeTacOrderQueue()` (warrior.cpp:1792): the next queued order becomes the new player order.
pub fn execute_tac_order_queue(w: &mut World, slot: u16) {
    let queued = w.pilots.get(slot).is_some_and(|p| !p.queue.is_empty());
    if queued {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.queue_executing = true;
        }
        let now = now(w);
        let next = {
            let Some(p) = w.pilots.get(slot) else { return };
            if p.queue_looping {
                // getNextQueuedTacOrder, looping: peek at tacOrderQueueIndex and advance (wrapping)
                let i = p.queue_index.min(p.queue.len() - 1);
                Some((p.queue[i].clone(), false))
            } else {
                Some((p.queue[0].clone(), true))
            }
        };
        if let Some((entry, remove)) = next {
            let order = super::unpack_queued_order(w, &entry, now);
            if let Some(p) = w.pilots.get_mut(slot) {
                if remove {
                    p.queue.remove(0);
                    p.queue_index = 0;
                } else {
                    p.queue_index += 1;
                    if p.queue_index >= p.queue.len() {
                        p.queue_index = 0;
                    }
                }
            }
            warrior::set_player_tac_order(w, slot, order, true);
        }
    } else if let Some(p) = w.pilots.get_mut(slot) {
        p.queue_executing = false;
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Alarms
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::checkAlarms()` (warrior.cpp:4743): for every alarm that fired this frame, apply its game-system effect, then call
/// the brain's ABL handler for it (if the brain defines one), then clear the alarm.
pub fn check_alarms(w: &mut World, slot: u16) {
    for code in 0..super::pilot::NUM_ALARMS {
        let fired = w
            .pilots
            .get(slot)
            .is_some_and(|p| !p.alarms[code].is_empty());
        if !fired {
            continue;
        }
        match code {
            alarm::HIT_BY_WEAPONFIRE => handle_hit_by_weapon_fire(w, slot),
            alarm::VEHICLE_INCAPACITATED => {
                // NOTE: This will never happen, since a disabled mech won't get into this routine (handled immediately)
                if let Some(me) = vehicle(w, slot) {
                    crate::combat::fx::own_vehicle_incapacitation(w, me, 0);
                }
            }
            alarm::VEHICLE_WITHDRAWN => {
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.status = crate::combat::fx::warrior_status::WITHDRAWN;
                }
            }
            alarm::DEATH_OF_MATE => handle_unit_mate_death(w, slot),
            alarm::KILLED_TARGET => handle_killed_target(w, slot),
            alarm::PLAYER_ORDER => handle_player_order(w, slot),
            alarm::NO_MOVEPATH => handle_no_move_path(w, slot),
            // TARGET_OF_WEAPONFIRE, DAMAGE_TAKEN_RATE, FRIENDLY_VEHICLE_*, VEHICLE_DESTROYED, ATTACK_ORDER, COLLISION, MATE_FIRED_WEAPON,
            // GATE_CLOSING, FIRED_WEAPON, NEW_MOVER: the handlers do nothing (compiled out or empty)
            _ => {}
        }
        // After applying game-system effects for the alarm, check for ABL-handlers in the pilot's brain module...
        super::brain::call_alarm_handler(w, slot, code);
        // Clear these alarms every frame...
        if let Some(p) = w.pilots.get_mut(slot) {
            p.alarms[code].clear();
        }
    }
}

/// `handleHitByWeaponFire` (warrior.cpp:6345): the radio message only (sound).
fn handle_hit_by_weapon_fire(w: &mut World, slot: u16) {
    let Some(&attacker) = w
        .pilots
        .get(slot)
        .and_then(|p| p.alarms[alarm::HIT_BY_WEAPONFIRE].first())
    else {
        return;
    };
    if attacker == 0 {
        // Ammo Explosion?
    } else if attacker == -3 {
        warrior::radio_message(w, slot, 15); // RADIO_UNDER_AIRSTRIP... RADIO_UNDER_AIRSTRIKE
    } else if attacker == -2 {
        warrior::radio_message(w, slot, 13); // RADIO_HITTING_MINES
    } else {
        let target_wid = w
            .pilots
            .get(slot)
            .map_or(WatchId(0), |p| p.cur_order.target);
        let has_order = w
            .pilots
            .get(slot)
            .is_some_and(|p| p.cur_order.code != OrderCode::None);
        let target_wid = if has_order { target_wid } else { WatchId(0) };
        if target_wid == WatchId(0) {
            warrior::radio_message(w, slot, 14); // RADIO_UNDER_ATTACK
        }
    }
}

/// `handleUnitMateDeath`: reads the mate; no effect.
fn handle_unit_mate_death(_w: &mut World, _slot: u16) {}

/// `handleKilledTarget` (warrior.cpp:7040-7125): credits the kill. A mech counts under its chassis class (an air mover and a ground
/// vehicle with path locks under `VEHICLE_CLASS_GROUND`), the kill list keeps at most `MAX_MOVERS / 3` = 85 watch ids, and the pilot
/// earns `KillSkill[vehicleClass]` gunnery points (the table is indexed by the class number itself, so light mechs earn the "medium"
/// value: a quirk of the source). Buildings, turrets and gates only play a radio message.
pub(super) fn handle_killed_target(w: &mut World, slot: u16) {
    let Some(&wid) = w
        .pilots
        .get(slot)
        .and_then(|p| p.alarms[alarm::KILLED_TARGET].first())
    else {
        return;
    };
    let Some(target) = w.objects.by_watch_id(WatchId(wid as u32)) else {
        return;
    };
    let Some(obj) = w.objects.get(target) else {
        return;
    };
    let kill_skill = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or([0.0f32; 6], |t| t.skills.kill_skill);
    // (vehicleClass, skillIncrement, message, counts as a kill)
    let (vehicle_class, skill_increment, message) = match obj.c.class {
        ObjectClass::BattleMech => {
            let air = matches!(&obj.kind, Kind::Mech(m) if m.m.move_level == 2);
            if !air {
                let class = usize::from(match &obj.kind {
                    Kind::Mech(m) => m.loadout.stats.chassis_class,
                    _ => 0,
                });
                (
                    Some(class),
                    kill_skill.get(class).copied().unwrap_or(0.0),
                    10,
                )
            } else {
                (Some(5), kill_skill[4], 9)
            }
        }
        ObjectClass::GroundVehicle => {
            // Killing Infantry/Powered Armor is meaningless!
            if matches!(&obj.kind, Kind::Vehicle(v) if v.m.path_locks_enabled) {
                (Some(5), kill_skill[4], 9)
            } else {
                (None, 0.0, -1)
            }
        }
        ObjectClass::Turret | ObjectClass::Gate => (None, 0.0, 7),
        ObjectClass::Elemental => (Some(6), kill_skill[5], 8),
        ObjectClass::Building => (None, 0.0, 8),
        _ => (None, 0.0, 7),
    };
    if let Some(class) = vehicle_class {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.num_mech_kills[class][1] += 1;
            if p.killed.len() < 85 && obj_is_mover_kill(class) {
                p.killed.push(WatchId(wid as u32));
            }
        }
    }
    warrior::radio_message(w, slot, message);
    // skillPoints[MWS_GUNNERY] += skillIncrement
    if let Some(l) = w.mission.pilot_loads.get_mut(usize::from(slot)) {
        l.skill_points[mc2_sim_data::pilot::Skill::Gunnery.index()] += skill_increment;
    }
}

/// Only mech and ground-vehicle kills enter `killed[]`.
fn obj_is_mover_kill(class: usize) -> bool {
    class != 6
}

/// `handlePlayerOrder` (warrior.cpp:6600): a shut-down mech wakes up when the player orders it (unless the order is power down).
fn handle_player_order(w: &mut World, slot: u16) {
    let Some(me) = vehicle(w, slot) else { return };
    let shut_down = w
        .objects
        .get(me)
        .is_some_and(|o| o.c.status == crate::object::ObjectStatus::ShutDown);
    let power_down = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.code == OrderCode::PowerDown);
    if shut_down && !power_down {
        orders::order_power_up(w, slot, false, OrderOrigin::SelfOrder);
    }
}

/// `handleNoMovePath` (warrior.cpp:6620): a get-fixed order whose path failed is dropped.
fn handle_no_move_path(w: &mut World, slot: u16) {
    let get_fixed = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.code == OrderCode::GetFixed);
    if get_fixed {
        clear_cur_tac_order(w, slot, true);
        warrior::radio_message(w, slot, orders::radio::MOVE_BLOCKED);
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Weapons status
// ---------------------------------------------------------------------------------------------------------------------

/// `Mover::canFireWeapons()`: `true` for mechs and vehicles (mover.h:1147; only the elemental override differs).
pub fn can_fire_weapons(_w: &World, _h: ObjectHandle) -> bool {
    true
}

/// `Mover::relFacingTo(goal)` (mover.cpp:2905) plus the per-body-location rotation of `BattleMech::relFacingTo` /
/// `GroundVehicle::relFacingTo` (mech.cpp:6585, gvehicl.cpp:4040).
pub fn rel_facing_to(w: &World, h: ObjectHandle, goal: &Vec3, body_location: i32) -> f32 {
    let Some(obj) = w.objects.get(h) else {
        return 0.0;
    };
    let facing_vec = crate::math::rotation_vector(obj.c.rotation);
    let goal_vec = Vec3 {
        x: goal.x - obj.c.position.x,
        y: goal.y - obj.c.position.y,
        z: 0.0,
    };
    let mut angle = crate::math::angle_from(&facing_vec, &goal_vec);
    let sign_z = facing_vec.x * goal_vec.y - facing_vec.y * goal_vec.x;
    if sign_z >= 0.0 {
        angle = -angle;
    }
    let mut rel = angle;
    match &obj.kind {
        Kind::Mech(m) => match body_location {
            0..=3 => rel += m.torso_rotation,
            8..=10 => rel += m.torso_rotation, // MECH_ARMOR_LOCATION_RCTORSO.. (never a body location, kept as in the source)
            4 => rel += m.torso_rotation + m.left_arm_rotation,
            5 => rel += m.torso_rotation + m.right_arm_rotation,
            _ => {}
        },
        Kind::Vehicle(v) => {
            if body_location == 0 {
                // GROUNDVEHICLE_LOCATION_TURRET
                rel += v.turret_rotation;
            }
            if rel < -180.0 {
                rel += 360.0;
            } else if rel > 180.0 {
                rel -= 360.0;
            }
        }
        _ => {}
    }
    rel
}

/// `MechWarrior::calcWeaponsStatus(target, weaponList, targetPoint)` (warrior.cpp:3377): fills `weapons_status` and returns the result.
pub fn calc_weapons_status(
    w: &mut World,
    slot: u16,
    target: Option<ObjectHandle>,
    target_point: Option<Vec3>,
) -> i32 {
    let Some(me) = vehicle(w, slot) else {
        return WEAPONS_STATUS_NO_TARGET;
    };
    if !can_fire_weapons(w, me) {
        return WEAPONS_STATUS_CANNOT_FIRE;
    }
    let target_position = if let Some(t) = target {
        match w.objects.get(t) {
            Some(o) => o.c.position,
            None => return WEAPONS_STATUS_NO_TARGET,
        }
    } else if let Some(p) = target_point {
        p
    } else {
        return WEAPONS_STATUS_NO_TARGET;
    };
    // Am I even within range?
    let distance_to_target = mover_ops::distance_from(w, me, &target_position);
    let (min_range, max_range, num_other, num_weapons) =
        match w.objects.get(me).and_then(crate::combat::loadout_of) {
            Some(l) => (
                l.stats.min_range,
                l.stats.max_range,
                l.num_other(),
                l.num_weapons(),
            ),
            None => return WEAPONS_STATUS_NO_TARGET,
        };
    if distance_to_target <= min_range || distance_to_target >= max_range {
        return WEAPONS_STATUS_OUT_OF_RANGE;
    }
    let Some(gs) = tables(w) else {
        return WEAPONS_STATUS_NO_TARGET;
    };
    let map_cell_diagonal = crate::combat::attack::map_cell_diagonal();
    let fire_arc = fire_arc(w, me, &gs);
    let aim_location = {
        let p = w.pilots.get(slot);
        match p {
            Some(p) if p.cur_order.is_combat_order() => p.cur_order.attack_params.aim_location,
            _ => -1,
        }
    };
    let mut num_weapons_with_shot = 0;
    for cur_weapon in 0..num_weapons {
        let weapon_index = num_other + cur_weapon;
        let ready = mover_ops::is_weapon_ready(w, me, weapon_index);
        let shots = mover_ops::weapon_shots(w, me, weapon_index as i32);
        let in_range =
            mover_ops::weapon_in_range(w, me, weapon_index, distance_to_target, map_cell_diagonal);
        let (body_location, indirect) = {
            let l = w
                .objects
                .get(me)
                .and_then(crate::combat::loadout_of)
                .expect("loadout");
            let item = &l.inventory[weapon_index];
            let indirect = gs
                .mover_general
                .indirect_fire_weapons
                .iter()
                .any(|&id| id == i32::from(item.master_id));
            // GroundVehicle::weaponLocked ignores weaponIndex and always calls
            // relFacingTo(targetPosition, GROUNDVEHICLE_LOCATION_TURRET) (gvehicl.cpp:4317-4320).
            // Mechs use each weapon's mounted location.
            let body_location =
                if matches!(w.objects.get(me).map(|o| &o.kind), Some(Kind::Vehicle(_))) {
                    0
                } else {
                    i32::from(item.body_location)
                };
            (body_location, indirect)
        };
        let status = if !ready {
            WEAPON_STATUS_NOT_READY
        } else if shots < 1 {
            WEAPON_STATUS_OUT_OF_AMMO
        } else if !in_range {
            WEAPON_STATUS_OUT_OF_RANGE
        } else {
            let rel_angle = rel_facing_to(w, me, &target_position, body_location);
            // ALWAYS do the SIMPLE calculation first. If they have indirect fire weapons, direct LOS is still VALID!
            let mut line_of_fire = match target {
                Some(t) => crate::object::line_of_sight(w, me, t, 0.0, true),
                None => crate::combat::los::line_of_sight_point(w, me, target_position),
            };
            // Indirect fire weapons can fire if ANYONE can see target!
            if !line_of_fire && indirect {
                // `Team::teamLineOfSight` (team.cpp:762-851) uses the target's LOS
                // position and appearance radius, or `CELL_DISTANCE` for a point.
                // Direct LOS stays authoritative when it is already true.
                let team = w.objects.get(me).map_or(-1, |o| o.team_id());
                line_of_fire = if let Some(t) = target {
                    let pos = crate::object::hooks::los_position(w, t);
                    let ext = crate::object::hooks::appear_radius(w, t);
                    crate::object::hooks::team_line_of_sight(w, team, pos, ext)
                } else {
                    crate::object::hooks::team_line_of_sight(
                        w,
                        team,
                        target_position,
                        POINT_TARGET_TEAM_LOS_EXTENT,
                    )
                };
            }
            if !line_of_fire || rel_angle < -fire_arc || rel_angle > fire_arc {
                WEAPON_STATUS_NOT_LOCKED
            } else {
                let chance = crate::combat::attack::calc_attack_chance(
                    w,
                    me,
                    target,
                    aim_location,
                    weapon_index as i32,
                    target_point,
                );
                if chance.chance < 1.0 {
                    WEAPON_STATUS_NO_CHANCE
                } else {
                    num_weapons_with_shot += 1;
                    chance.chance as i32
                }
            }
        };
        if let Some(p) = w.pilots.get_mut(slot) {
            if let Some(s) = p.weapons_status.get_mut(cur_weapon) {
                *s = status;
            }
        }
    }
    num_weapons_with_shot
}

/// `Mover::getFireArc()` (mover.cpp:2994): `FireArc[0]` for a mech, `[1]` for a ground vehicle, `[2]` for an elemental, 45 otherwise.
pub fn fire_arc(w: &World, h: ObjectHandle, gs: &mc2_sim_data::gamesys::GameSystem) -> f32 {
    match w.objects.get(h).map(|o| o.c.class) {
        Some(ObjectClass::BattleMech) => gs.mover_fire.fire_arc[0],
        Some(ObjectClass::GroundVehicle) => gs.mover_fire.fire_arc[1],
        Some(ObjectClass::Elemental) => gs.mover_fire.fire_arc[2],
        _ => 45.0,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Combat decision tree
// ---------------------------------------------------------------------------------------------------------------------

/// `GameObject::getDestructLevel()`: the damage needed to be sure of destroying the object. A mech is always shot at (mech.h:558);
/// a vehicle needs one more than its best armor location (gvehicl.h:388); structures and turrets and gates the type's damage level
/// less the damage taken (bldng.h:266, turret.h:291, gate.h:215); anything else keeps shooting (gameobj.h:422).
pub fn destruct_level(w: &World, h: ObjectHandle) -> f32 {
    let Some(o) = w.objects.get(h) else {
        return 5000.0;
    };
    match &o.kind {
        Kind::Vehicle(_) => {
            let max_armor = crate::combat::loadout_of(o)
                .map_or(0.0, |l| l.armor.iter().map(|a| a.0).fold(0.0f32, f32::max));
            max_armor + 1.0
        }
        Kind::Building(_) | Kind::Turret(_) | Kind::Gate(_) => {
            crate::object::structure::damage_level(w, h) - o.structure().map_or(0.0, |s| s.damage)
        }
        _ => 5000.0,
    }
}

/// `Mover::getWeaponIsEnergy()` (mover.cpp:5740).
fn weapon_is_energy(w: &World, me: ObjectHandle, weapon_index: usize) -> bool {
    let (Some(comps), Some(l)) = (
        w.data.components.as_ref(),
        w.objects.get(me).and_then(crate::combat::loadout_of),
    ) else {
        return false;
    };
    let id = l.inventory[weapon_index].master_id;
    crate::combat::stats::form_of(crate::combat::stats::comp(comps, id))
        == Some(mc2_sim_data::component::ComponentForm::WeaponEnergy)
}

/// `MechWarrior::combatDecisionTree()` (warrior.cpp:3492): the ammo check, target sanity, the taunt roll, firing every weapon with a
/// shot through [`crate::combat::fire::fire_weapon`], `STOP_AND_FIRE` and conserve-ammo.
pub fn combat_decision_tree(w: &mut World, slot: u16) -> i32 {
    let mut result = -1;
    let t = now(w);
    let freq = combat_update_frequency(w);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.timers[1] = t + freq;
    }
    let Some(me) = vehicle(w, slot) else {
        return result;
    };
    // Let's see if we have ANY ammo...
    let out_of_ammo = {
        let l = w.objects.get(me).and_then(crate::combat::loadout_of);
        l.is_some_and(|l| !l.ammo_totals.iter().any(|a| a.1 > 0)) || l.is_none()
    };
    let target = get_last_target(w, slot);
    let (cur_is_combat, cur_code, cur_attack_type, cur_aim) = {
        let p = w.pilots.get(slot).expect("pilot");
        (
            p.cur_order.is_combat_order(),
            p.cur_order.code,
            p.cur_order.attack_params.attack_type,
            p.cur_order.attack_params.aim_location,
        )
    };
    let mut attack_type = orders::ATTACK_TO_DESTROY;
    let mut aim_location = -1;
    let mut target_point: Option<Vec3> = None;
    if cur_is_combat {
        attack_type = cur_attack_type;
        aim_location = cur_aim;
        if cur_code == OrderCode::AttackPoint {
            target_point = w.pilots.get(slot).map(|p| p.attack_orders.target_point);
        }
    } else if w.pilots.get(slot).is_some_and(|p| p.last_target_flags[2]) {
        attack_type = orders::ATTACK_CONSERVING_AMMO;
    }

    let mut target = target;
    if let Some(t) = target {
        if !cur_is_combat {
            let pos = w.objects.get(t).map(|o| o.c.position).unwrap_or_default();
            let attack_radius = w.pilots.get(slot).map_or(0.0, |p| p.attack_radius);
            if mover_ops::distance_from(w, me, &pos) > attack_radius {
                set_last_target(w, slot, None, false, false);
                target = None;
            }
        }
    }
    if target.is_none() && cur_code != OrderCode::AttackPoint {
        return result;
    }
    if let Some(t) = target {
        let (destroyed, disabled) = w
            .objects
            .get(t)
            .map_or((true, true), |o| (o.c.is_destroyed(), o.c.is_disabled()));
        if destroyed {
            return result;
        }
        let obliterate = w.pilots.get(slot).is_some_and(|p| p.last_target_flags[0]);
        if disabled && !obliterate {
            return result;
        }
    }

    // So we don't muck up the brains at this late date, simply report we're out of ammo...
    if attack_type != orders::ATTACK_CONSERVING_AMMO && out_of_ammo {
        warrior::radio_message(w, slot, orders::radio::AMMO_OUT);
    }
    let mut cant_conserve = attack_type == orders::ATTACK_CONSERVING_AMMO;

    let weapons_result = w.pilots.get(slot).map_or(0, |p| p.weapons_status_result);
    let mut fired_weapon = false;
    if can_fire_weapons(w, me) && weapons_result > 0 {
        // First, a taunt... Only Taunt Moving things!!
        if let Some(t) = target {
            if w.objects.get(t).is_some_and(|o| o.is_mover())
                && w.rng.0.borrow_mut().sim.roll_dice(1)
            {
                warrior::radio_message(w, slot, orders::radio::TAUNT);
            }
        }
        // Let loose... (warrior.cpp:3668-3699; `targetTime` only feeds Mover::fireWeapon's network/replay bookkeeping)
        // Track how much damage I've done to the target when weapons finally hit. If I've done enough to blow the target, STOP firing!!
        let mut damage_done_to_target = 0.0f32;
        let (num_other, num_weapons) = w
            .objects
            .get(me)
            .and_then(crate::combat::loadout_of)
            .map_or((0, 0), |l| (l.num_other(), l.num_weapons()));
        let destruct_level = target.map(|t| destruct_level(w, t));
        for cur_weapon in 0..num_weapons {
            let weapon_index = num_other + cur_weapon;
            let status = w
                .pilots
                .get(slot)
                .and_then(|p| p.weapons_status.get(cur_weapon).copied())
                .unwrap_or(0);
            if status > 0
                && destruct_level.is_none_or(|d| damage_done_to_target < d)
                && (attack_type != orders::ATTACK_CONSERVING_AMMO
                    || weapon_is_energy(w, me, weapon_index))
            {
                let r = crate::combat::fire::fire_weapon(
                    w,
                    me,
                    weapon_index as i32,
                    target,
                    aim_location,
                    target_point,
                    attack_type,
                );
                if r.code == 0 {
                    cant_conserve = false;
                }
                fired_weapon = true;
                damage_done_to_target += r.dmg_done;
            }
            result = 0;
        }
    }

    let stop_and_fire = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.attack_params.tactic == Tactic::StopAndFire);
    if stop_and_fire && fired_weapon {
        clear_move_orders(w, slot);
    }
    if cant_conserve {
        // probably a lie: a working energy weapon that is just recycling means we can conserve
        let cons = w
            .objects
            .get(me)
            .and_then(crate::combat::loadout_of)
            .is_some_and(|l| {
                let Some(comps) = w.data.components.as_ref() else {
                    return false;
                };
                (l.num_other()..l.num_other() + l.num_weapons()).any(|i| {
                    l.weapon_shots(comps, i as i64) == UNLIMITED_SHOTS && !l.inventory[i].disabled
                })
            });
        if cons {
            cant_conserve = false;
        }
    }
    if cant_conserve {
        warrior::radio_message(w, slot, orders::radio::ILLEGAL_ORDER);
        set_last_target(w, slot, None, false, false);
        clear_cur_tac_order(w, slot, true); // so we don't keep trying (and announcing that it doesn't work!!)
    }
    result
}

// ---------------------------------------------------------------------------------------------------------------------
// Movement decision tree
// ---------------------------------------------------------------------------------------------------------------------

fn set_move_state_goal(w: &mut World, slot: u16, state: i32) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.lane_b_move.move_state_goal = state;
    }
}

/// `MechWarrior::movementDecisionTree()` (warrior.cpp:3910).
pub fn movement_decision_tree(w: &mut World, slot: u16) -> bool {
    let t = now(w);
    let Some(me) = vehicle(w, slot) else {
        return true;
    };
    let timeout = move_time_out(w);
    let time_of_last_step = w
        .pilots
        .get(slot)
        .map_or(-1.0, |p| p.lane_b_move.time_of_last_step);
    if f64::from(time_of_last_step) > -1.0 && time_of_last_step < (t - timeout) {
        // Abort move order, 'cause we are unable to take a step!
        clear_move_orders(w, slot);
        let (is_move, is_way, order_time) = w
            .pilots
            .get(slot)
            .map(|p| {
                (
                    warrior::is_move_order(&p.cur_order),
                    warrior::is_way_path_order(&p.cur_order),
                    p.cur_order.time,
                )
            })
            .unwrap_or_default();
        if (is_move || is_way) && order_time < (t - timeout) {
            warrior::radio_message(w, slot, orders::radio::MOVE_BLOCKED);
            clear_cur_tac_order(w, slot, true);
        }
        trigger_alarm(w, slot, alarm::NO_MOVEPATH, mo::err::TIME_OUT);
    }

    if w.pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.is_yielding)
    {
        let yield_time = w
            .pilots
            .get(slot)
            .map_or(-1.0, |p| p.lane_b_move.yield_time);
        if yield_time < t {
            let yt = move_yield_time(w);
            if let Some(p) = w.pilots.get_mut(slot) {
                p.lane_b_move.set_yield_time(t + yt);
            }
            // For now, we'll use random chance to decide if we should recalc or not. (USE_OVERLAYS is not defined: no blown bridge)
            let on_blown_bridge = false;
            let mut reroute = on_blown_bridge || w.rng.0.borrow_mut().sim.random_number(100) < 75;
            // `getMovePath()->crossesBridge(-1, 3)`: bridges are USE_BRIDGES-only, always -1; the getMovePath side effects still run
            get_move_path_index(w, slot);
            let crosses_bridge = false;
            if crosses_bridge {
                reroute = reroute || false;
            }
            if reroute {
                let (selection, jump) = {
                    let p = w.pilots.get(slot).expect("pilot");
                    (p.cur_order.selection_index, p.cur_order.move_params.jump)
                };
                let mut params = MOVEPARAM_RECALC | MOVEPARAM_FACE_TARGET;
                if jump {
                    params |= MOVEPARAM_JUMP;
                }
                request_move_path(w, slot, selection, params, 4);
            }
        }
    }

    if w.pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.path_type == mo::MOVEPATH_COMPLEX)
    {
        // Currently following a complex (LR) path--make sure we queue up the next path segment if we haven't already...
        let (cur, num, spare_steps) = {
            let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
            (
                m.cur_global_step,
                m.num_global_steps,
                m.paths[1].num_steps_when_not_paused,
            )
        };
        if cur < num - 1 && spare_steps == 0 && !warrior::has_path_request(w, slot) {
            let (selection, jump) = {
                let p = w.pilots.get(slot).expect("pilot");
                (p.cur_order.selection_index, p.cur_order.move_params.jump)
            };
            let mut params = MOVEPARAM_FACE_TARGET;
            if jump {
                params |= MOVEPARAM_JUMP;
            }
            request_move_path(w, slot, selection, params, 5);
        }
    }

    // Everything up to this point is done EVERY frame update. Now, see if it's time to do the ugly stuff...
    let movement_update = w.pilots.get(slot).map_or(0.0, |p| p.timers[2]);
    if movement_update > t {
        return true;
    }
    let freq = movement_update_frequency(w);
    if let Some(p) = w.pilots.get_mut(slot) {
        p.timers[2] = t + freq;
    }

    let cur_code = w
        .pilots
        .get(slot)
        .map_or(OrderCode::None, |p| p.cur_order.code);
    if cur_code == OrderCode::None || cur_code == OrderCode::Stop {
        if get_last_target(w, slot).is_some() {
            set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
        }
        return true;
    }

    let attack_range = warrior::core(w, slot).map_or(0, |c| c.attack_range);
    if attack_range == crate::command::FIRERANGE_CURRENT {
        let (is_move, is_capture) = {
            let p = w.pilots.get(slot).expect("pilot");
            (
                warrior::is_move_order(&p.cur_order),
                p.cur_order.code == OrderCode::Capture,
            )
        };
        if !is_move && !is_capture {
            if get_last_target(w, slot).is_some() {
                set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
            }
            return true;
        }
    }

    let (goal_type, _goal_loc, goal_object) = warrior::get_move_goal(w, slot);
    let (selection, jump, tactic, method, pursue_flag, keep_moving_order) = {
        let o = &w.pilots.get(slot).expect("pilot").cur_order;
        (
            o.selection_index,
            o.move_params.jump,
            o.attack_params.tactic,
            o.attack_params.method,
            o.attack_params.pursue,
            o.move_params.keep_moving,
        )
    };
    let init_params = |jump: bool| {
        let mut p = MOVEPARAM_INIT | MOVEPARAM_FACE_TARGET;
        if jump {
            p |= MOVEPARAM_JUMP;
        }
        p
    };

    if goal_type == MOVEGOAL_NONE {
        // Are we currently pursuing an attack target? Is this what we really want?
        let cur_code = w.pilots.get(slot).expect("pilot").cur_order.code;
        if cur_code == OrderCode::Withdraw {
            // We are on the run, so let's choose a new escape goal...
            let escape = orders::calc_withdraw_goal(w, slot);
            warrior::set_move_goal(w, slot, MOVEGOAL_LOCATION, Some(escape));
            request_move_path(w, slot, selection, init_params(jump), 6);
        } else if cur_code == OrderCode::MoveToPoint {
            tacorder::cur_execute(w, slot);
        } else if cur_code == OrderCode::MoveToObject || cur_code == OrderCode::Capture {
            let move_target = w
                .objects
                .by_watch_id(w.pilots.get(slot).expect("pilot").cur_order.target);
            let Some(move_target) = move_target else {
                // `moveTarget->getPosition()` on NULL is undefined behaviour in the source; the zero-filled oracle reads position 0,0,0.
                report_unavailable(w, me, AiUnavailable::NullOrderTarget);
                return true;
            };
            // Pick the best move goal based upon the object...
            let (is_mover, is_building, pos, from_area) = {
                let o = w.objects.get(move_target).expect("target");
                (
                    o.is_mover(),
                    matches!(o.kind, Kind::Building(_)),
                    o.c.position,
                    w.pilots
                        .get(slot)
                        .map_or(-1, |p| p.cur_order.move_params.from_area),
                )
            };
            let target_pos = if is_mover {
                pos
            } else {
                if is_building {
                    super::goal::calc_adjacent_area_cell(w, move_target, me, from_area)
                        .unwrap_or_else(|| mover_ops::calc_offset_move_goal(w, me, &pos))
                } else {
                    mover_ops::calc_offset_move_goal(w, me, &pos)
                }
            };
            let wid = w.objects.watch_id(move_target);
            warrior::set_move_goal(w, slot, i64::from(wid.0), Some(target_pos));
            let params = init_params(jump);
            if !is_mover {
                request_move_path(
                    w,
                    slot,
                    selection,
                    params | MOVEPARAM_STEP_ADJACENT_TARGET,
                    7,
                );
            } else {
                request_move_path(w, slot, selection, params, 8);
            }
        } else if w
            .pilots
            .get(slot)
            .expect("pilot")
            .cur_order
            .is_combat_order()
        {
            attack_goal_none(
                w,
                slot,
                me,
                selection,
                jump,
                tactic,
                method,
                pursue_flag,
                keep_moving_order,
            );
        } else if get_last_target(w, slot).is_some() {
            set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
        }
    } else if goal_type == MOVEGOAL_LOCATION {
        let (yielding, twisting, waiting, steps) = {
            let m = &w.pilots.get(slot).expect("pilot").lane_b_move;
            (
                m.is_yielding,
                m.twisting,
                m.is_waiting_for_point(),
                m.paths[0].num_steps,
            )
        };
        let jumping = mover_ops::is_jumping(w, me).is_some();
        // getMovePath() side effects run when the short-circuit chain reaches them
        let reaches_path = !yielding && !jumping && !twisting && !waiting;
        if reaches_path {
            let i = get_move_path_index(w, slot).unwrap_or(0);
            let steps_now = w
                .pilots
                .get(slot)
                .map_or(steps, |p| p.lane_b_move.paths[i].num_steps);
            if steps_now == 0 {
                // We may be so close that we don't need a move path...
                if !warrior::has_path_request(w, slot) {
                    request_move_path(w, slot, selection, init_params(jump), 12);
                }
            }
        }
    } else if let Some(goal_object) = goal_object {
        // Check if our target is still around. If not, clear our current move goal...
        let target_pos = w
            .objects
            .get(goal_object)
            .map(|o| o.c.position)
            .unwrap_or_default();
        let is_combat = w
            .pilots
            .get(slot)
            .expect("pilot")
            .cur_order
            .is_combat_order();
        if !is_combat {
            let goal_object_position = w
                .pilots
                .get(slot)
                .expect("pilot")
                .lane_b_move
                .goal_object_position;
            let far = {
                let d = crate::math::approximate_length(&Vec3 {
                    x: target_pos.x - goal_object_position.x,
                    y: target_pos.y - goal_object_position.y,
                    z: target_pos.z - goal_object_position.z,
                });
                d > 50.0
            };
            if far {
                let wid = w.objects.watch_id(goal_object);
                warrior::set_move_goal(w, slot, i64::from(wid.0), Some(target_pos));
                request_move_path(w, slot, selection, init_params(jump), 13);
            } else {
                let i = get_move_path_index(w, slot).unwrap_or(0);
                let unpaused = w
                    .pilots
                    .get(slot)
                    .map_or(0, |p| p.lane_b_move.paths[i].num_steps_when_not_paused);
                if unpaused == 0 {
                    let wid = w.objects.watch_id(goal_object);
                    warrior::set_move_goal(w, slot, i64::from(wid.0), Some(target_pos));
                    request_move_path(w, slot, selection, init_params(jump), 13);
                }
            }
        } else {
            attack_goal_object(w, slot, me, goal_object, selection, jump, tactic);
        }
    }
    true
}

/// The `isCombatOrder()` branch of the `MOVEGOAL_NONE` case (warrior.cpp:4062-4175).
#[allow(clippy::too_many_arguments)]
fn attack_goal_none(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    selection: i32,
    jump: bool,
    tactic: Tactic,
    method: crate::command::AttackMethod,
    pursue: bool,
    keep_moving: bool,
) {
    let init_params = |jump: bool| {
        let mut p = MOVEPARAM_INIT | MOVEPARAM_FACE_TARGET;
        if jump {
            p |= MOVEPARAM_JUMP;
        }
        p
    };
    let target = warrior::get_current_target(w, slot);
    let cur_code = w.pilots.get(slot).expect("pilot").cur_order.code;
    let target_position = if let Some(t) = target {
        w.objects.get(t).map(|o| o.c.position).unwrap_or_default()
    } else if cur_code == OrderCode::AttackPoint {
        w.pilots
            .get(slot)
            .expect("pilot")
            .attack_orders
            .target_point
    } else {
        return;
    };
    if method == crate::command::AttackMethod::Ramming {
        let Some(t) = target else {
            report_unavailable(w, me, AiUnavailable::NullOrderTarget);
            return;
        };
        let pos = w.objects.get(t).map(|o| o.c.position).unwrap_or_default();
        let wid = w.objects.watch_id(t);
        warrior::set_move_goal(w, slot, i64::from(wid.0), Some(pos));
        request_move_path(w, slot, selection, init_params(jump), 9);
    } else if !pursue {
        set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
    } else {
        let mut recalc_path = false;
        let mut move_params = MOVEPARAM_NONE;
        // We will want to re-evaluate optimal range if our optimal range has changed...
        if let Some(t) = target {
            if w.objects.get(t).is_some_and(|o| o.is_mover()) {
                let target_calc = w
                    .objects
                    .get(t)
                    .and_then(crate::combat::loadout_of)
                    .map_or(0.0, |l| l.stats.last_weapon_effectiveness_calc);
                let my_calc = w
                    .objects
                    .get(me)
                    .and_then(crate::combat::loadout_of)
                    .map_or(0.0, |l| l.stats.last_optimal_range_calc);
                if target_calc > my_calc && tacorder::calc_fire_ranges(w, me) {
                    recalc_path = true;
                    move_params = MOVEPARAM_SOMEWHERE_ELSE;
                }
            }
        }
        let distance_to_target = mover_ops::distance_from(w, me, &target_position);
        let desired_attack_range = orders::ordered_fire_range(w, slot, None);
        let attack_range_delta = distance_to_target - desired_attack_range;
        let recalc_delta =
            crate::terrain::WORLD_UNITS_PER_VERTEX * crate::terrain::METERS_PER_WORLD_UNIT;
        let weapons_result = w.pilots.get(slot).map_or(0, |p| p.weapons_status_result);
        if tactic != Tactic::StopAndFire
            && tactic != Tactic::Turret
            && (attack_range_delta > recalc_delta || attack_range_delta < -recalc_delta)
        {
            // Try to maintain attack range...
            recalc_path = true;
            move_params = MOVEPARAM_SOMEWHERE_ELSE;
        } else if tactic != Tactic::Turret && weapons_result == WEAPONS_STATUS_OUT_OF_RANGE {
            recalc_path = true;
            move_params = MOVEPARAM_SOMEWHERE_ELSE;
        } else if weapons_result >= 0 {
            let counts = weapon_counts(w, slot, me);
            let class = w.objects.get(me).map(|o| o.c.class);
            let moving_steps = get_move_path_steps(w, slot);
            let twisting = w.pilots.get(slot).is_some_and(|p| p.lane_b_move.twisting);
            match class {
                Some(ObjectClass::BattleMech) => {
                    if weapons_result == 0 {
                        let los = match target {
                            Some(t) => crate::object::line_of_sight(w, me, t, 0.0, true),
                            None => crate::combat::los::line_of_sight_point(w, me, target_position),
                        };
                        if !los {
                            recalc_path = true;
                            move_params = MOVEPARAM_SOMEWHERE_ELSE;
                        } else if counts.not_ready > 0
                            || counts.not_locked > 0
                            || counts.too_hot > 0
                        {
                            if moving_steps == 0 && !twisting {
                                set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
                            }
                        } else {
                            recalc_path = true;
                            move_params = MOVEPARAM_SOMEWHERE_ELSE;
                        }
                    }
                }
                Some(ObjectClass::GroundVehicle) => {
                    if weapons_result == 0 {
                        let los = match target {
                            Some(t) => crate::object::line_of_sight(w, me, t, 0.0, true),
                            None => crate::combat::los::line_of_sight_point(w, me, target_position),
                        };
                        if !los {
                            recalc_path = true;
                            move_params = MOVEPARAM_SOMEWHERE_ELSE;
                        } else if counts.not_ready == 0 {
                            if counts.not_locked > 0 || counts.too_hot > 0 {
                                if moving_steps == 0 && !twisting {
                                    set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
                                }
                            } else {
                                recalc_path = true;
                                move_params = MOVEPARAM_SOMEWHERE_ELSE;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let keep_moving_flag = w.pilots.get(slot).is_some_and(|p| p.keep_moving);
        if !recalc_path
            && tactic != Tactic::StopAndFire
            && keep_moving_flag
            && keep_moving
            && w.rng.0.borrow_mut().sim.random_number(100) < 60
        {
            recalc_path = true;
            move_params = MOVEPARAM_SOMEWHERE_ELSE + MOVEPARAM_RANDOM_OPTIMAL;
        }
        if recalc_path && tactic != Tactic::Turret {
            if let Some(t) = target {
                let wid = w.objects.watch_id(t);
                warrior::set_move_goal(w, slot, i64::from(wid.0), Some(target_position));
                if jump {
                    move_params |= MOVEPARAM_JUMP;
                }
                request_move_path(
                    w,
                    slot,
                    selection,
                    move_params | MOVEPARAM_INIT | MOVEPARAM_FACE_TARGET,
                    10,
                );
            } else {
                warrior::set_move_goal(w, slot, MOVEGOAL_LOCATION, Some(target_position));
                request_move_path(w, slot, selection, init_params(jump), 11);
            }
        }
    }
}

/// The attack branch of the goal-object case (warrior.cpp:4235-4380).
fn attack_goal_object(
    w: &mut World,
    slot: u16,
    me: ObjectHandle,
    goal_object: ObjectHandle,
    selection: i32,
    jump: bool,
    tactic: Tactic,
) {
    let target = warrior::get_current_target(w, slot);
    let attack_pursuit = w.pilots.get(slot).is_some_and(|p| p.attack_orders.pursue);
    let Some(target) = target else { return };
    if target != goal_object || !attack_pursuit {
        return;
    }
    let mut recalc_path = false;
    let mut move_params = MOVEPARAM_NONE;
    let target_is_mover = w.objects.get(target).is_some_and(|o| o.is_mover());
    if target_is_mover {
        let target_calc = w
            .objects
            .get(target)
            .and_then(crate::combat::loadout_of)
            .map_or(0.0, |l| l.stats.last_weapon_effectiveness_calc);
        let my_calc = w
            .objects
            .get(me)
            .and_then(crate::combat::loadout_of)
            .map_or(0.0, |l| l.stats.last_optimal_range_calc);
        if target_calc > my_calc && tacorder::calc_fire_ranges(w, me) {
            recalc_path = true;
            move_params = MOVEPARAM_SOMEWHERE_ELSE;
        }
    } else if get_move_path_steps(w, slot) > 0 {
        return;
    }
    let target_pos = w
        .objects
        .get(target)
        .map(|o| o.c.position)
        .unwrap_or_default();
    let distance_to_target = mover_ops::distance_from(w, me, &target_pos);
    let desired_attack_range = orders::ordered_fire_range(w, slot, None);
    let attack_range_delta = distance_to_target - desired_attack_range;
    let recalc_delta =
        crate::terrain::WORLD_UNITS_PER_VERTEX * crate::terrain::METERS_PER_WORLD_UNIT;
    let weapons_result = w.pilots.get(slot).map_or(0, |p| p.weapons_status_result);
    if tactic != Tactic::StopAndFire
        && tactic != Tactic::Turret
        && (attack_range_delta > recalc_delta || attack_range_delta < -recalc_delta)
    {
        recalc_path = true;
        move_params = MOVEPARAM_SOMEWHERE_ELSE;
    } else if tactic != Tactic::Turret && weapons_result == WEAPONS_STATUS_OUT_OF_RANGE {
        recalc_path = true;
        move_params = MOVEPARAM_SOMEWHERE_ELSE;
    } else if weapons_result >= 0 {
        let counts = weapon_counts(w, slot, me);
        let class = w.objects.get(me).map(|o| o.c.class);
        let moving_steps = get_move_path_steps(w, slot);
        let twisting = w.pilots.get(slot).is_some_and(|p| p.lane_b_move.twisting);
        match class {
            Some(ObjectClass::BattleMech) => {
                if weapons_result == 0 {
                    if !crate::object::line_of_sight(w, me, target, 0.0, true) {
                        recalc_path = true;
                        move_params = MOVEPARAM_SOMEWHERE_ELSE;
                    } else if counts.not_ready > 0 || counts.too_hot > 0 || counts.not_locked > 0 {
                        // If we already have a path, don't pivot, finish the move...
                        if moving_steps == 0 && !twisting {
                            set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
                        }
                    } else {
                        recalc_path = true;
                        move_params = MOVEPARAM_SOMEWHERE_ELSE;
                    }
                }
            }
            Some(ObjectClass::GroundVehicle) => {
                if weapons_result == 0 {
                    if !crate::object::line_of_sight(w, me, target, 0.0, true) {
                        recalc_path = true;
                        move_params = MOVEPARAM_SOMEWHERE_ELSE;
                    } else if counts.not_ready == 0 {
                        if counts.not_locked > 0 || counts.too_hot > 0 {
                            if moving_steps == 0 && !twisting {
                                set_move_state_goal(w, slot, MOVESTATE_PIVOT_TARGET);
                            }
                        } else {
                            recalc_path = true;
                            move_params = MOVEPARAM_STEP_TOWARD_TARGET;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // --added 6/3/01 by GD
    let (keep_moving_flag, order_keep_moving) = {
        let p = w.pilots.get(slot).expect("pilot");
        (p.keep_moving, p.cur_order.move_params.keep_moving)
    };
    if !recalc_path
        && tactic != Tactic::StopAndFire
        && keep_moving_flag
        && order_keep_moving
        && w.rng.0.borrow_mut().sim.random_number(100) < 60
    {
        recalc_path = true;
        move_params = MOVEPARAM_SOMEWHERE_ELSE + MOVEPARAM_RANDOM_OPTIMAL;
    }
    if recalc_path && tactic != Tactic::Turret {
        let wid = w.objects.watch_id(target);
        warrior::set_move_goal(w, slot, i64::from(wid.0), Some(target_pos));
        if jump {
            move_params |= MOVEPARAM_JUMP;
        }
        request_move_path(
            w,
            slot,
            selection,
            move_params | MOVEPARAM_INIT | MOVEPARAM_FACE_TARGET,
            14,
        );
    }
}

struct WeaponCounts {
    not_ready: i32,
    not_locked: i32,
    too_hot: i32,
}

/// The tallies of `weaponsStatus[]` by status (the others are counted in the source but never read).
fn weapon_counts(w: &World, slot: u16, me: ObjectHandle) -> WeaponCounts {
    let num_weapons = w
        .objects
        .get(me)
        .and_then(crate::combat::loadout_of)
        .map_or(0, |l| l.num_weapons());
    let mut c = WeaponCounts {
        not_ready: 0,
        not_locked: 0,
        too_hot: 0,
    };
    if let Some(p) = w.pilots.get(slot) {
        for &s in p.weapons_status.iter().take(num_weapons) {
            if s == WEAPON_STATUS_NOT_READY {
                c.not_ready += 1;
            }
            if s == WEAPON_STATUS_NOT_LOCKED {
                c.not_locked += 1;
            }
            if s == WEAPON_STATUS_TOO_HOT {
                c.too_hot += 1;
            }
            let _ = (
                WEAPON_STATUS_OUT_OF_AMMO,
                WEAPON_STATUS_OUT_OF_RANGE,
                WEAPON_STATUS_NO_CHANCE,
            );
        }
    }
    c
}

/// `getMovePath()->numSteps` (with the `getMovePath` side effects).
fn get_move_path_steps(w: &mut World, slot: u16) -> usize {
    let i = get_move_path_index(w, slot).unwrap_or(0);
    w.pilots
        .get(slot)
        .map_or(0, |p| p.lane_b_move.paths[i].num_steps)
}

// ---------------------------------------------------------------------------------------------------------------------
// updateActions and mainDecisionTree
// ---------------------------------------------------------------------------------------------------------------------

/// `MechWarrior::updateActions()` (warrior.cpp:4867).
pub fn update_actions(w: &mut World, slot: u16) {
    let team = team_id(w, slot);
    if team == -1 || !warrior::brains_enabled(w, team) {
        clear_cur_tac_order(w, slot, false);
        clear_move_orders(w, slot);
        clear_attack_orders(w, slot);
        if let Some(p) = w.pilots.get_mut(slot) {
            for o in &mut p.tac_orders {
                *o = TacticalOrder::init();
            }
        }
        set_main_goal(w, slot, super::pilot::goal_action::NONE, None, None, -1.0);
        return;
    }
    let Some(me) = vehicle(w, slot) else { return };
    if w.objects
        .get(me)
        .is_some_and(|o| o.c.flag(crate::object::flags::CAPTURED))
    {
        clear_cur_tac_order(w, slot, true);
        set_last_target(w, slot, None, false, false);
        return;
    }
    let t = now(w);
    if w.pilots.get(slot).is_some_and(|p| p.timers[1] <= t) {
        combat_decision_tree(w, slot);
    }
    movement_decision_tree(w, slot);
}

/// `MechWarrior::mainDecisionTree()` (warrior.cpp:4899-5115).
pub fn main_decision_tree(w: &mut World, slot: u16) {
    let team = team_id(w, slot);
    if team == -1 || !warrior::brains_enabled(w, team) {
        clear_cur_tac_order(w, slot, false);
        if let Some(p) = w.pilots.get_mut(slot) {
            for o in &mut p.tac_orders {
                *o = TacticalOrder::init();
            }
        }
        set_main_goal(w, slot, super::pilot::goal_action::NONE, None, None, -1.0);
    }

    // If we currently have no order, check if we have any player orders queued...
    {
        let Some(p) = w.pilots.get(slot) else { return };
        if p.cur_order.code == OrderCode::None
            && p.queue_executing
            && !p.queue.is_empty()
            && !p.new_order_received[1]
        {
            // First queued order just given, so begin executing it immediately...
            execute_tac_order_queue(w, slot);
        }
    }

    // Is the current tac order accomplished?
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.code != OrderCode::None)
        && tacorder::cur_status(w, slot) == TACORDER_SUCCESS
    {
        let mut player_order_queued = false;
        let (state, executing) = w
            .pilots
            .get(slot)
            .map_or((OrderState::General, false), |p| {
                (p.order_state, p.queue_executing)
            });
        if state == OrderState::Player && executing {
            execute_tac_order_queue(w, slot);
            player_order_queued = w.pilots.get(slot).is_some_and(|p| p.new_order_received[1]);
        }
        clear_cur_tac_order(w, slot, true);
        if let Some(p) = w.pilots.get_mut(slot) {
            p.new_order_received[1] = player_order_queued;
        }
    }

    // If we're without orders now, let's record the time...
    let t = now(w);
    if under_home_command(w, slot) {
        if let Some(p) = w.pilots.get_mut(slot) {
            if p.cur_order.code == OrderCode::None && p.time_of_last_orders < 0.0 {
                p.time_of_last_orders = t;
            }
        }
    }

    // Update Weapons Status, if needed this frame...
    let due = w
        .pilots
        .get(slot)
        .is_some_and(|p| p.timers[0] <= t || p.timers[1] <= t || p.timers[2] <= t);
    if due {
        let target = get_last_target(w, slot);
        if let Some(target) = target {
            let r = calc_weapons_status(w, slot, Some(target), None);
            set_weapons_status_result(w, slot, r);
        } else if w
            .pilots
            .get(slot)
            .is_some_and(|p| p.cur_order.code == OrderCode::AttackPoint)
        {
            let pos = w.pilots.get(slot).map(|p| p.attack_orders.target_point);
            let r = calc_weapons_status(w, slot, None, pos);
            set_weapons_status_result(w, slot, r);
        }
    }

    // Make sure, if I have a target, it's still legit.
    let mut tac_order_target = if w
        .pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.code != OrderCode::None)
    {
        w.pilots
            .get(slot)
            .and_then(|p| w.objects.by_watch_id(p.cur_order.target))
    } else {
        None
    };
    let mut target = get_last_target(w, slot);
    if let Some(tg) = target {
        let obliterate = w.pilots.get(slot).is_some_and(|p| p.last_target_flags[0]);
        let clear_target = w
            .objects
            .get(tg)
            .is_some_and(|o| o.c.is_destroyed() || (o.c.is_disabled() && !obliterate));
        if clear_target {
            set_last_target(w, slot, None, false, false);
            if w.pilots
                .get(slot)
                .is_some_and(|p| p.cur_order.code == OrderCode::AttackObject)
                && Some(tg) == tac_order_target
            {
                clear_cur_tac_order(w, slot, true);
                target = None;
                tac_order_target = None;
            }
        }
    }
    if w.pilots
        .get(slot)
        .is_some_and(|p| p.cur_order.code == OrderCode::AttackObject)
        && target != tac_order_target
    {
        set_last_target(w, slot, tac_order_target, false, false);
    }

    // Update pilot brain...
    let brain_due = w.pilots.get(slot).is_some_and(|p| p.timers[0] <= t);
    if brain_due && (team == -1 || warrior::brains_enabled(w, team)) {
        super::brain::run_brain(w, slot);
        if let Some(p) = w.pilots.get_mut(slot) {
            p.timers[0] += BRAIN_UPDATE_FREQUENCY;
        }
    }

    // In case the brain set a new target, let's recalc weaponsStatus...
    let target = get_last_target(w, slot);
    if let Some(tg) = target {
        let last_target_time = w.pilots.get(slot).map_or(-1.0, |p| p.last_target.1);
        if last_target_time == t {
            let r = calc_weapons_status(w, slot, Some(tg), None);
            set_weapons_status_result(w, slot, r);
        }
    }

    // Any Alarms?
    check_alarms(w, slot);

    // Now, check if we have a new current tac order...
    let new_order = select_new_order(w, slot);
    if let Some(order) = new_order.filter(|o| o.code != OrderCode::None) {
        start_order(w, slot, order, true);
        if let Some(p) = w.pilots.get_mut(slot) {
            p.time_of_last_orders = -1.0;
        }
    }

    // These must be cleared every frame...
    if let Some(p) = w.pilots.get_mut(slot) {
        p.new_order_received = [false; 3];
    }

    // Now that we know what we're doing, do it...
    update_actions(w, slot);
}

fn set_weapons_status_result(w: &mut World, slot: u16, r: i32) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.weapons_status_result = r;
    }
}

/// The `switch (orderState)` of `mainDecisionTree` (warrior.cpp:4998-5047): arbitration between the alarm, player and general
/// order slots. Returns the order that becomes current (code `NONE` when none does).
fn select_new_order(w: &mut World, slot: u16) -> Option<TacticalOrder> {
    let mut new_order: Option<TacticalOrder> = None;
    let state = w.pilots.get(slot)?.order_state;
    let received = w.pilots.get(slot)?.new_order_received;
    match state {
        OrderState::General => {
            if received[2] {
                clear_cur_tac_order(w, slot, false);
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[2].clone());
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.order_state = OrderState::Alarm;
                }
            } else if received[1] {
                // If we're currently executing a general (brain) order, purge it permanently (NOW, we DON'T want them to fall back
                // to the general order--the brain must re-order it)...
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.tac_orders[0] = TacticalOrder::init();
                }
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[1].clone());
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.order_state = OrderState::Player;
                }
            } else if received[0] {
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[0].clone());
            }
        }
        OrderState::Player => {
            if received[2] {
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[2].clone());
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.order_state = OrderState::Alarm;
                }
            } else if received[1] {
                // Make sure we have no general order we fall back to when we finish this, per Tim's request...
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.tac_orders[0] = TacticalOrder::init();
                }
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[1].clone());
            }
        }
        OrderState::Alarm => {
            // If the player issues a new order, let it override any current alarm (self-issued) order...
            if received[1] {
                if let Some(p) = w.pilots.get_mut(slot) {
                    p.alarm_priority = 0;
                    p.tac_orders[2] = TacticalOrder::init();
                }
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[1].clone());
            } else if received[2] {
                new_order = w.pilots.get(slot).map(|p| p.tac_orders[2].clone());
            }
        }
    }
    new_order
}

#[allow(dead_code)]
const _UNUSED: (u32, u32, u32) = (MOVEPARAM_NONE, u32::MAX, 0);

/// `commander_is_home` re-export for the tests of this module's callers.
pub fn is_home(w: &World, slot: u16) -> bool {
    commander_is_home(w, slot)
}
