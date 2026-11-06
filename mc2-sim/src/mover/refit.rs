// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Repair and recovery of movers: `Mover::needsRefit`, `Mover::refit`, `Mover::recover` (mover.cpp:6041, 6636, 6818) and the point pools
//! of repair vehicles, recovery vehicles and repair bays (`GroundVehicle::getRefitPoints`, `burnRefitPoints`, `getRecoverPoints`,
//! `burnRecoverPoints`, gvehicl.h:458-495, gvehicl.cpp:5580; `Building::getRefitPoints`, `burnRefitPoints`, bldng.h:372, bldng.cpp:1649).

use crate::combat::stats::{
    body, calc_leg_status, calc_torso_status, comp, IS_DAMAGE_DESTROYED, IS_DAMAGE_NONE,
    IS_DAMAGE_PARTIAL,
};
use crate::combat::{loadout_of, loadout_of_mut, Loadout, HOME_COMMANDER};
use crate::ids::{ObjectHandle, WatchId};
use crate::object::{Kind, ObjectStatus};
use crate::World;
use mc2_sim_data::component::ComponentStats;
use mc2_sim_data::objtype::ObjectTypeData;

/// `GROUNDVEHICLE_LOCATION_TURRET`: where a support vehicle keeps its refit, recover or mine points.
pub const GV_TURRET: usize = 4;
/// `ARMOR_REFIT_COST`, `INTERNAL_REFIT_COST`, `AMMO_REFIT_COST` and `REFIT_VEHICLE` (mover.h).
const ARMOR_COST: usize = 0;
const INTERNAL_COST: usize = 1;
const AMMO_COST: usize = 2;
const REFIT_VEHICLE: usize = 0;
/// `RADIO_REFIT_INCOMPLETE`, `RADIO_REFIT_DONE` (dradio.h).
pub const RADIO_REFIT_INCOMPLETE: i32 = 5;
pub const RADIO_REFIT_DONE: i32 = 6;
/// `BETTY_REPAIR_COMPLETE`, `BETTY_REPAIR_INCOMPLETE`, `BETTY_REPAIR_GONE` (sounds.h:190, 191, 221).
pub const BETTY_REPAIR_COMPLETE: u32 = 20;
pub const BETTY_REPAIR_INCOMPLETE: u32 = 21;
pub const BETTY_REPAIR_GONE: u32 = 51;

fn vehicle_type(
    w: &World,
    h: ObjectHandle,
) -> Option<&mc2_sim_data::objtype::vehicle::GroundVehicleType> {
    match w.objects.type_of(h).map(|t| &t.data) {
        Some(ObjectTypeData::GroundVehicle(v)) => Some(v),
        _ => None,
    }
}

/// `GroundVehicle::canRefit` (`refitPoints > 0`, gvehicl.cpp:931).
pub fn can_refit(w: &World, h: ObjectHandle) -> bool {
    vehicle_type(w, h).is_some_and(|v| v.refit_points > 0)
}

/// `GroundVehicle::canRecover` (`recoverPoints > 0`, gvehicl.cpp:932).
pub fn can_recover(w: &World, h: ObjectHandle) -> bool {
    matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Vehicle(v)) if v.can_recover)
}

/// `GroundVehicle::getRefitPoints()`: the turret armour of a vehicle that can refit, else 0.
pub fn vehicle_refit_points(w: &World, h: ObjectHandle) -> f32 {
    if !can_refit(w, h) {
        return 0.0;
    }
    w.objects
        .get(h)
        .and_then(loadout_of)
        .map_or(0.0, |l| l.armor[GV_TURRET].0)
}

/// `GroundVehicle::getRecoverPoints()`.
pub fn vehicle_recover_points(w: &World, h: ObjectHandle) -> f32 {
    if !can_recover(w, h) {
        return 0.0;
    }
    w.objects
        .get(h)
        .and_then(loadout_of)
        .map_or(0.0, |l| l.armor[GV_TURRET].0)
}

/// `GroundVehicle::burnRefitPoints(points)` (gvehicl.cpp:5580). The "repair gone" Betty plays for the home commander.
pub fn burn_vehicle_refit_points(w: &mut World, h: ObjectHandle, points: f32) -> bool {
    if !can_refit(w, h) {
        return false;
    }
    let mut gone = false;
    let mut home = false;
    if let Some(o) = w.objects.get_mut(h) {
        home = o.commander_id() == HOME_COMMANDER;
        if let Some(l) = loadout_of_mut(o) {
            let armor = &mut l.armor[GV_TURRET].0;
            *armor -= points;
            if *armor <= 0.0 {
                *armor = 0.0;
                gone = true;
            }
        }
    }
    if gone && home {
        w.events.push(crate::SimEvent::Betty(BETTY_REPAIR_GONE));
    }
    true
}

/// `GroundVehicle::burnRecoverPoints(points)` (gvehicl.h:480).
pub fn burn_vehicle_recover_points(w: &mut World, h: ObjectHandle, points: f32) -> bool {
    if !can_recover(w, h) {
        return false;
    }
    let Some(l) = w.objects.get_mut(h).and_then(loadout_of_mut) else {
        return false;
    };
    if points <= l.armor[GV_TURRET].0 {
        l.armor[GV_TURRET].0 -= points;
        true
    } else {
        false
    }
}

/// `Building::getRefitPoints()` (bldng.h:372): the damage a bay can still absorb.
pub fn building_refit_points(w: &World, h: ObjectHandle) -> f32 {
    let Some(Kind::Building(_)) = w.objects.get(h).map(|o| &o.kind) else {
        return 0.0;
    };
    let damage = w
        .objects
        .get(h)
        .and_then(|o| o.structure())
        .map_or(0.0, |s| s.damage);
    crate::object::structure::damage_level(w, h) - damage
}

/// `Building::burnRefitPoints(points)` (bldng.cpp:1649): `setDamage(points + getDamage())` (a bay with no points left is destroyed);
/// false when it is used up.
pub fn burn_building_refit_points(w: &mut World, h: ObjectHandle, points: f32) -> bool {
    let level = crate::object::structure::damage_level(w, h);
    let mut destroyed = false;
    if let Some(o) = w.objects.get_mut(h) {
        if let Some(s) = o.structure_mut() {
            s.damage += points;
            if s.damage >= level {
                destroyed = true;
            }
        }
        if destroyed {
            o.c.set_status(ObjectStatus::Destroyed, false);
        }
    }
    if destroyed {
        crate::object::hooks::set_obj_status(w, h, ObjectStatus::Destroyed);
        // soundSystem->playBettySample(BETTY_REPAIR_GONE)
        w.events.push(crate::SimEvent::Betty(BETTY_REPAIR_GONE));
        return false;
    }
    true
}

/// `Mover::needsRefit()` (mover.cpp:6041): an unattended mech with armour, internal structure or ammunition below its maximum
/// (destroyed arms do not count).
pub fn needs_refit(w: &World, h: ObjectHandle) -> bool {
    let Some(Kind::Mech(m)) = w.objects.get(h).map(|o| &o.kind) else {
        return false;
    };
    if m.m.refit_buddy != WatchId::NONE {
        return false;
    }
    let l = &m.loadout;
    for loc in 0..l.armor.len() {
        if loc < l.body.len()
            && (loc == body::LARM || loc == body::RARM)
            && l.body[loc].damage_state == IS_DAMAGE_DESTROYED
        {
            continue;
        }
        if (loc < l.body.len()
            && l.body[loc].cur_internal_structure < f32::from(l.body[loc].max_internal_structure))
            || l.armor[loc].0 < f32::from(l.armor[loc].1)
        {
            return true;
        }
    }
    l.ammo_totals.iter().any(|t| t.1 < t.2)
}

fn is_arm_destroyed(l: &Loadout, i: usize) -> bool {
    (i == body::LARM || i == body::RARM)
        && l.body
            .get(i)
            .is_some_and(|b| b.damage_state == IS_DAMAGE_DESTROYED)
}

/// The damage state of an internal structure after a repair (`IS_DAMAGE_NONE` / `PARTIAL` / `DESTROYED`).
fn damage_state_of(cur: f32, max: u8) -> u8 {
    if cur <= 0.0 {
        IS_DAMAGE_DESTROYED
    } else if cur / f32::from(max) <= 0.5 {
        IS_DAMAGE_PARTIAL
    } else {
        IS_DAMAGE_NONE
    }
}

/// `calcLegStatus` / `calcTorsoStatus` after a leg or the centre torso was repaired (mover.cpp:6700).
fn recalc_status(w: &mut World, h: ObjectHandle, location: usize, in_recover_update: bool) {
    if location != body::LLEG && location != body::RLEG && location != body::CTORSO {
        return;
    }
    let has_pilot = crate::combat::fx::pilot_slot(w, h).is_some();
    let mut effects = None;
    if let Some(Kind::Mech(m)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
        if location == body::LLEG || location == body::RLEG {
            effects = Some(calc_leg_status(
                &m.loadout,
                &mut m.leg_status.0,
                &mut m.sent_crippled_msg,
                has_pilot,
                in_recover_update,
            ));
        }
        if location == body::CTORSO {
            m.loadout.stats.torso_status = calc_torso_status(&m.loadout);
        }
    }
    if let Some(fx) = effects {
        crate::combat::fx::leg_effects(w, h, fx);
    }
}

/// `Mover::refit(pointsAvailable, pointsUsed, ammoOnly)` (mover.cpp:6636): one repair pass. Each of the three pools (armour, internal
/// structure, ammunition) gets a third of `RefitAmount`, shared among the locations that need it, priced by the refit cost table.
/// Returns `(done, pointsUsed)`; `done` when nothing is left to repair or the points ran out. `pointsUsed` is rounded to quarter points.
pub fn refit(
    w: &mut World,
    h: ObjectHandle,
    points_available: f32,
    ammo_only: bool,
) -> (bool, f32) {
    let (Some(tables), Some(comps)) = (w.data.gamesys.tables.clone(), w.data.components.clone())
    else {
        return (false, 0.0);
    };
    let general = &tables.mover_general;
    let armor_cost = general.refit_costs[ARMOR_COST][REFIT_VEHICLE];
    let internal_cost = general.refit_costs[INTERNAL_COST][REFIT_VEHICLE];
    let ammo_cost = general.refit_costs[AMMO_COST][REFIT_VEHICLE];
    let mut refit_points = points_available;
    let mut result = false;
    let mut leg_checks: Vec<usize> = Vec::new();
    let mut recalc_weapons = false;
    let mut home = false;
    {
        let Some(o) = w.objects.get_mut(h) else {
            return (false, 0.0);
        };
        home = home || o.commander_id() == HOME_COMMANDER;
        let Some(l) = loadout_of_mut(o) else {
            return (false, 0.0);
        };
        let num_body = l.body.len();
        let num_armor = l.armor.len();
        if refit_points > 0.0 {
            // size of the pool for each section (armour, internals and ammunition) before depletions
            let area_max = general.refit_amount / 3.0;
            let mut rearm_count = 0i32;
            let mut repair_count = 0i32;
            // how many locations need armour or internal structure repaired?
            if !ammo_only {
                for i in 0..num_armor {
                    if is_arm_destroyed(l, i) {
                        continue;
                    }
                    if i < num_body
                        && l.body[i].cur_internal_structure
                            < f32::from(l.body[i].max_internal_structure)
                    {
                        repair_count += 1;
                    }
                    if l.armor[i].0 < f32::from(l.armor[i].1) {
                        repair_count += 1;
                    }
                }
            }
            // how many ammunition types need refilling?
            for t in &l.ammo_totals {
                if t.1 < t.2 {
                    rearm_count += 1;
                }
            }
            // repair each armour and internal structure
            for i in 0..num_armor {
                if repair_count == 0 || refit_points <= 0.0 {
                    continue;
                }
                if is_arm_destroyed(l, i) {
                    continue;
                }
                if l.armor[i].0 < f32::from(l.armor[i].1) {
                    let mut points_to_use = area_max / repair_count as f32;
                    if points_to_use > refit_points {
                        points_to_use = refit_points;
                    }
                    let mut repair_value = points_to_use * armor_cost;
                    let max = f32::from(l.armor[i].1);
                    if l.armor[i].0 + repair_value > max {
                        repair_value = max - l.armor[i].0;
                        points_to_use = repair_value / armor_cost;
                    }
                    l.armor[i].0 += repair_value;
                    refit_points -= points_to_use;
                }
                if i < num_body
                    && l.body[i].cur_internal_structure
                        < f32::from(l.body[i].max_internal_structure)
                {
                    let mut points_to_use = area_max / repair_count as f32;
                    if points_to_use > refit_points {
                        points_to_use = refit_points;
                    }
                    let mut repair_value = points_to_use * internal_cost;
                    let max = f32::from(l.body[i].max_internal_structure);
                    if l.body[i].cur_internal_structure + repair_value > max {
                        repair_value = max - l.body[i].cur_internal_structure;
                        points_to_use = repair_value / internal_cost;
                    }
                    l.body[i].cur_internal_structure += repair_value;
                    l.body[i].damage_state = damage_state_of(
                        l.body[i].cur_internal_structure,
                        l.body[i].max_internal_structure,
                    );
                    leg_checks.push(i);
                    refit_points -= points_to_use;
                }
            }
            // refit each ammunition type that needs it
            for t in 0..l.ammo_totals.len() {
                let (master, cur, max) = l.ammo_totals[t];
                if rearm_count > 0 && refit_points > 0.0 && cur < max {
                    let recalc = cur == 0;
                    let per_ton = match comp(&comps, master as u8).map(|c| &c.stats) {
                        Some(ComponentStats::Ammo { ammo_per_ton, .. }) => *ammo_per_ton as f32,
                        _ => 0.0,
                    };
                    let mut points_to_use = area_max / rearm_count as f32;
                    if points_to_use > refit_points {
                        points_to_use = refit_points;
                    }
                    let mut repair_value = points_to_use * ammo_cost * per_ton;
                    if cur as f32 + repair_value > max as f32 {
                        repair_value = (max - cur) as f32;
                        points_to_use = repair_value / (ammo_cost * per_ton);
                    }
                    // `curAmount` is a long: the fractional part of the refill is dropped
                    l.ammo_totals[t].1 = (cur as f32 + repair_value) as i32;
                    refit_points -= points_to_use;
                    if recalc {
                        recalc_weapons = true;
                    }
                }
            }
            // do any locations still need repair?
            rearm_count = 0;
            repair_count = 0;
            if !ammo_only {
                for i in 0..num_armor {
                    if is_arm_destroyed(l, i) {
                        continue;
                    }
                    if (i < num_body
                        && l.body[i].cur_internal_structure
                            < f32::from(l.body[i].max_internal_structure))
                        || l.armor[i].0 < f32::from(l.armor[i].1)
                    {
                        repair_count += 1;
                        break;
                    }
                }
            }
            // if there's no more repairing, do any ammunition types need refilling?
            if repair_count == 0 && l.ammo_totals.iter().any(|t| t.1 < t.2) {
                rearm_count += 1;
            }
            if repair_count == 0 && rearm_count == 0 {
                result = true;
            }
        } else {
            // no points left, so move on
            result = true;
        }
    }
    // `calcLegStatus` / `calcTorsoStatus` per repaired location, in location order
    for location in leg_checks {
        recalc_status(w, h, location, false);
    }
    if recalc_weapons {
        recalc_weapon_state(w, h);
    }
    // done fixing (the points lasted) or out of points (none were left when the pass began)
    let done_fixing = result && points_available > 0.0;
    if result && home {
        if done_fixing {
            radio(w, h, RADIO_REFIT_DONE);
            w.events.push(crate::SimEvent::Betty(BETTY_REPAIR_COMPLETE));
            // `appearance->startSmoking(-1)`: the smoke goes out
            crate::appearance::fx::start_smoking(w, h, -1);
        } else {
            radio(w, h, RADIO_REFIT_INCOMPLETE);
            w.events.push(crate::SimEvent::Betty(BETTY_REPAIR_GONE));
        }
    }
    let mut points_used = points_available - refit_points;
    // Points used are rounded to the nearest 1/4 point (for consistency in MultiPlayer)...
    if points_used > 0.0 && points_used < 0.25 {
        points_used = 0.25;
    }
    points_used = ((points_used + 0.125) / 0.25) as i64 as f32 / 4.0;
    (result, points_used)
}

fn radio(w: &mut World, h: ObjectHandle, message: i32) {
    crate::combat::fx::radio_message(w, h, message);
}

/// `calcWeaponEffectiveness(false)` then `calcFireRanges()` after a weapon's ammunition or health came back.
fn recalc_weapon_state(w: &mut World, h: ObjectHandle) {
    let gunnery = crate::combat::stats::pilot_gunnery(w, h);
    let scenario_time = w.clock.scenario_time;
    if let (Some(comps), Some(gs)) = (w.data.components.clone(), w.data.gamesys.tables.clone()) {
        if let Some(l) = w.objects.get_mut(h).and_then(loadout_of_mut) {
            l.calc_weapon_effectiveness(gunnery, scenario_time, false);
            l.calc_fire_ranges(&comps, &gs, scenario_time);
        }
    }
}

/// `Mover::recover()` (mover.cpp:6818): the recovery team puts a disabled mech's internal structure right (with unlimited points) and
/// every component back in working order. Returns whether the recovery is complete (it is, in one pass: the C++ callers loop).
pub fn recover(w: &mut World, h: ObjectHandle) -> bool {
    let (Some(tables), Some(comps)) = (w.data.gamesys.tables.clone(), w.data.components.clone())
    else {
        return false;
    };
    let general = &tables.mover_general;
    let mut recover_points = 100_000.0f32;
    let mut leg_checks: Vec<usize> = Vec::new();
    let mut done = false;
    {
        let Some(l) = w.objects.get_mut(h).and_then(loadout_of_mut) else {
            return false;
        };
        let num_body = l.body.len();
        let mut repair_count = 0i32;
        for i in 0..num_body {
            if is_arm_destroyed(l, i) {
                continue;
            }
            if l.body[i].cur_internal_structure < f32::from(l.body[i].max_internal_structure) {
                repair_count += 1;
            }
        }
        for i in 0..num_body {
            if repair_count == 0 || recover_points <= 0.0 {
                continue;
            }
            if is_arm_destroyed(l, i) {
                continue;
            }
            if l.body[i].cur_internal_structure < f32::from(l.body[i].max_internal_structure) {
                let mut points_to_use = general.recover_amount / repair_count as f32;
                if points_to_use > recover_points {
                    points_to_use = recover_points;
                }
                let mut repair_value = points_to_use * general.recover_cost;
                let max = f32::from(l.body[i].max_internal_structure);
                if l.body[i].cur_internal_structure + repair_value > max {
                    repair_value = max - l.body[i].cur_internal_structure;
                    points_to_use = repair_value / general.recover_cost;
                }
                l.body[i].cur_internal_structure += repair_value;
                l.body[i].damage_state = damage_state_of(
                    l.body[i].cur_internal_structure,
                    l.body[i].max_internal_structure,
                );
                leg_checks.push(i);
                recover_points -= points_to_use;
            }
        }
        // do any locations still need repair?
        let still = (0..num_body).any(|i| {
            !is_arm_destroyed(l, i)
                && l.body[i].cur_internal_structure < f32::from(l.body[i].max_internal_structure)
        });
        if !still {
            // Done fixing: every component is whole again. The C++ walks `numOther` items, so the weapons its switch lists are never
            // reached (see DIFFERENCES.md); the other items and the weapons are restored here.
            let end = l.num_other() + l.num_weapons();
            for i in 0..end {
                let health = comp(&comps, l.inventory[i].master_id).map_or(0, |c| c.health);
                l.inventory[i].health = health;
                l.inventory[i].disabled = false;
            }
            for b in 0..num_body {
                for j in 0..l.body[b].critical_spaces.len() {
                    let id = l.body[b].critical_spaces[j].0;
                    if id < 255 {
                        let disabled = l
                            .inventory
                            .get(usize::from(id))
                            .is_some_and(|it| it.disabled);
                        l.body[b].critical_spaces[j].1 = disabled;
                    }
                }
            }
            done = true;
        }
    }
    // `inRecoverUpdate` is set for the whole call: the leg status is recalculated without radio messages
    for location in leg_checks {
        recalc_status(w, h, location, true);
    }
    if done {
        recalc_weapon_state(w, h);
        if w.objects
            .get(h)
            .is_some_and(|o| o.commander_id() == HOME_COMMANDER)
        {
            radio(w, h, RADIO_REFIT_DONE);
            w.events.push(crate::SimEvent::Betty(BETTY_REPAIR_COMPLETE));
        }
    }
    done
}
