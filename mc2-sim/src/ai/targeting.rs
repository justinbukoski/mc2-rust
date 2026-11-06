// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `TargetPriorityList::calcTarget` / `calcAction` (warrior.cpp:454-800), `MechWarrior::coreScan` (warrior.cpp:5806) and the team
//! contact queries they use (`TeamSensorSystem::meetsCriteria` / `getContacts`, contact.cpp:815-935).

use super::pilot::TargetPriorityList;
use crate::command::OrderCode;
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::object::{flags, Kind, ObjectClass};
use crate::World;

/// `CONTACT_CRITERIA_*` (dcontact.h:29-38).
pub mod criteria {
    /// `CONTACT_CRITERIA_ENEMY`.
    pub const ENEMY: i32 = 1;
    /// `CONTACT_CRITERIA_VISUAL`.
    pub const VISUAL: i32 = 2;
    /// `CONTACT_CRITERIA_NOT_CHALLENGED`.
    pub const NOT_CHALLENGED: i32 = 8;
    /// `CONTACT_CRITERIA_SENSOR`.
    pub const SENSOR: i32 = 16;
    /// `CONTACT_CRITERIA_VISUAL_OR_SENSOR`.
    pub const VISUAL_OR_SENSOR: i32 = 32;
    /// `CONTACT_CRITERIA_NOT_DISABLED`.
    pub const NOT_DISABLED: i32 = 64;
    /// `CONTACT_CRITERIA_ARMED`.
    pub const ARMED: i32 = 128;
}

/// `CONTACT_SORT_*`.
pub const SORT_NONE: i32 = 0;
/// `CONTACT_SORT_CV`.
pub const SORT_CV: i32 = 1;
/// `CONTACT_SORT_DISTANCE`.
pub const SORT_DISTANCE: i32 = 2;

/// `TARGET_PRIORITY_*` (warrior.h:355-376).
pub mod prio {
    /// `TARGET_PRIORITY_NONE`.
    pub const NONE: i32 = 0;
    /// `TARGET_PRIORITY_GAMEOBJECT`.
    pub const GAMEOBJECT: i32 = 1;
    /// `TARGET_PRIORITY_MOVER`.
    pub const MOVER: i32 = 2;
    /// `TARGET_PRIORITY_BUILDING`.
    pub const BUILDING: i32 = 3;
    /// `TARGET_PRIORITY_CURTARGET`.
    pub const CURTARGET: i32 = 4;
    /// `TARGET_PRIORITY_TURRET`.
    pub const TURRET: i32 = 5;
    /// `TARGET_PRIORITY_TURRET_CONTROL`.
    pub const TURRET_CONTROL: i32 = 6;
    /// `TARGET_PRIORITY_GATE`.
    pub const GATE: i32 = 7;
    /// `TARGET_PRIORITY_GATE_CONTROL`.
    pub const GATE_CONTROL: i32 = 8;
}

/// `ContactInfo::contactStatus[team]` of a mover (`Mover::getContactStatus(team, true)`): 0 for a removed mover.
pub fn contact_status(w: &World, mover: ObjectHandle, team: i32) -> u8 {
    if team < 0 {
        return 0;
    }
    if w.objects
        .get(mover)
        .is_some_and(|o| o.c.flag(flags::REMOVED))
    {
        return 0;
    }
    w.sensors
        .team_contact_status
        .get(&mover)
        .map_or(0, |s| s[team as usize % crate::ids::MAX_TEAMS])
}

/// `TeamSensorSystem::meetsCriteria(looker, mover, criteria)` for the team `team_id` (contact.cpp:815).
pub fn meets_criteria(w: &mut World, team_id: i32, mover: ObjectHandle, criteria: i32) -> bool {
    let status = contact_status(w, mover, team_id);
    let Some(obj) = w.objects.get(mover) else {
        return false;
    };
    if obj.c.flag(flags::REMOVED) {
        return false;
    }
    if status == 0 {
        return false;
    }
    let is_sensor = (1..=4).contains(&status);
    if criteria & criteria::VISUAL_OR_SENSOR != 0 && status != 5 && !is_sensor {
        return false;
    }
    if criteria & criteria::VISUAL != 0 && status != 5 {
        return false;
    }
    if criteria & criteria::SENSOR != 0 && !is_sensor {
        return false;
    }
    if criteria & criteria::NOT_CHALLENGED != 0 && super::get_challenger_handle(w, mover).is_some()
    {
        return false;
    }
    let Some(obj) = w.objects.get(mover) else {
        return false;
    };
    if criteria & criteria::NOT_DISABLED != 0 && obj.c.is_disabled() {
        return false;
    }
    if criteria & criteria::ENEMY != 0 {
        let their_team = obj.team_id();
        if team_id == -1 || !crate::object::hooks::team_exists(w, their_team) {
            return false;
        }
        if crate::object::hooks::team_relation(w, team_id, their_team)
            != Some(super::Relation::Enemy)
        {
            return false;
        }
    }
    if criteria & criteria::ARMED != 0 {
        let functional = w
            .objects
            .get(mover)
            .and_then(crate::combat::loadout_of)
            .map_or(0, |l| l.stats.num_functional_weapons);
        if functional == 0 {
            return false;
        }
    }
    true
}

/// `Team::isContact(looker, mover, criteria)`.
pub fn team_is_contact(
    w: &mut World,
    looker: ObjectHandle,
    mover: ObjectHandle,
    criteria: i32,
) -> bool {
    let team = w.objects.get(looker).map_or(-1, |o| o.team_id());
    if !crate::object::hooks::team_exists(w, team) {
        return false;
    }
    meets_criteria(w, team, mover, criteria)
}

/// `Mover::getContacts(contactList, criteria, sortType)` (mover.cpp:5523): the contacts of the mover's sensor system's team, in the
/// team's contact order, filtered by `criteria` and stably sorted (CV descending / distance ascending).
pub fn get_contacts(
    w: &mut World,
    looker: ObjectHandle,
    criteria: i32,
    sort: i32,
) -> Vec<ObjectHandle> {
    let Some(team_index) = super::sensors::mover_sensor_team(w, looker) else {
        return Vec::new();
    };
    let candidates = w.sensors.team_contact_order[team_index].clone();
    let origin = w
        .objects
        .get(looker)
        .map(|o| o.c.position)
        .unwrap_or_default();
    let mut valid: Vec<(ObjectHandle, f32)> = Vec::new();
    for handle in candidates {
        if !meets_criteria(w, team_index as i32, handle, criteria) {
            continue;
        }
        let Some(obj) = w.objects.get(handle) else {
            continue;
        };
        let value = match sort {
            SORT_CV => f32::from(obj.c.cur_cv),
            SORT_DISTANCE => crate::combat::attack::distance_from(&origin, &obj.c.position),
            _ => 0.0,
        };
        valid.push((handle, value));
    }
    match sort {
        SORT_CV => valid.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)),
        SORT_DISTANCE => {
            valid.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        }
        _ => {}
    }
    valid.into_iter().map(|(h, _)| h).collect()
}

/// `TargetPriorityList::calcAction(pilot, target)` (warrior.cpp:454).
pub fn calc_action(w: &World, slot: u16, target: ObjectHandle) -> OrderCode {
    let Some(obj) = w.objects.get(target) else {
        return OrderCode::None;
    };
    let me = super::warrior::vehicle(w, slot).and_then(|h| w.objects.get(h));
    let my_team = me.map_or(-1, |o| o.team_id());
    if obj.is_mover() {
        let enemy = !crate::object::hooks::team_exists(w, my_team)
            || crate::object::hooks::team_relation(w, my_team, obj.team_id())
                == Some(super::Relation::Enemy);
        if enemy && !obj.c.is_disabled() {
            return OrderCode::AttackObject;
        }
    } else if obj.c.class == ObjectClass::Building && obj.team_id() != my_team {
        if obj.c.flag(flags::CONTROLBUILDING) {
            return OrderCode::Capture;
        }
        return OrderCode::AttackObject;
    }
    OrderCode::None
}

/// `TargetPriorityList::calcTarget(pilot, location, contactCriteria, action)` (warrior.cpp:483): the watch id of the first
/// priority entry that yields a target, 0 for none. `list[i].params[2] += CONTACT_CRITERIA_ARMED` is an ADD on every call (so the
/// armed bit toggles and carries), exactly as in the source.
pub fn calc_target(w: &mut World, slot: u16, _location: Vec3, _contact_criteria: i32) -> i32 {
    let team_id = super::warrior::team_id(w, slot);
    if team_id < 0 || !crate::object::hooks::team_exists(w, team_id) {
        return 0;
    }
    let Some(me) = super::warrior::vehicle(w, slot) else {
        return 0;
    };
    let size = w.pilots.get(slot).map_or(0, |p| p.target_priorities.size);
    for i in 0..size.max(0) as usize {
        let entry = w.pilots.get(slot).expect("pilot").target_priorities.list[i];
        match entry.kind {
            prio::NONE => return prio::NONE,
            prio::GAMEOBJECT => {
                // param 0 = part id, 1 = engage radius, 2 = contact criteria
                let mut obj = (entry.params[0] != 0)
                    .then(|| {
                        w.objects
                            .find_by_part_id(crate::ids::PartId(entry.params[0]))
                    })
                    .flatten();
                // HACK! For now, we will not target unarmed movers...
                bump_armed(w, slot, i);
                let params2 =
                    w.pilots.get(slot).expect("pilot").target_priorities.list[i].params[2];
                if let Some(o) = obj.take() {
                    let pos = w.objects.get(o).map(|x| x.c.position).unwrap_or_default();
                    if f64::from(super::mover_ops::distance_from(w, me, &pos))
                        < f64::from(entry.params[1])
                    {
                        let is_mover = w.objects.get(o).is_some_and(|x| x.is_mover());
                        if params2 != 0 && is_mover {
                            if team_is_contact(w, me, o, params2) {
                                return i32::try_from(w.objects.watch_id(o).0).unwrap_or(0);
                            }
                        } else {
                            return i32::try_from(w.objects.watch_id(o).0).unwrap_or(0);
                        }
                    }
                }
            }
            prio::MOVER => {
                let mut obj = (entry.params[0] != 0)
                    .then(|| {
                        w.objects
                            .find_by_part_id(crate::ids::PartId(entry.params[0]))
                    })
                    .flatten();
                bump_armed(w, slot, i);
                let params2 =
                    w.pilots.get(slot).expect("pilot").target_priorities.list[i].params[2];
                if obj.is_none() {
                    // select best gameobject that fits params: for now, just returns the closest contact...
                    if params2 != 0 {
                        let mut smallest = 999_999.0f32;
                        for c in get_contacts(w, me, params2, SORT_DISTANCE) {
                            // `isMarine()` is false for mechs and vehicles
                            let pos = w.objects.get(c).map(|x| x.c.position).unwrap_or_default();
                            let dist = super::mover_ops::distance_from(w, me, &pos);
                            if dist < smallest {
                                obj = Some(c);
                                smallest = dist;
                            }
                        }
                    }
                }
                if let Some(o) = obj.filter(|&o| w.objects.get(o).is_some_and(|x| x.is_mover())) {
                    let pos = w.objects.get(o).map(|x| x.c.position).unwrap_or_default();
                    if f64::from(super::mover_ops::distance_from(w, me, &pos))
                        < f64::from(entry.params[1])
                    {
                        if params2 != 0 {
                            if team_is_contact(w, me, o, params2) {
                                return i32::try_from(w.objects.watch_id(o).0).unwrap_or(0);
                            }
                        } else {
                            return i32::try_from(w.objects.watch_id(o).0).unwrap_or(0);
                        }
                    }
                }
            }
            prio::BUILDING => {
                let mut obj = (entry.params[0] != 0)
                    .then(|| {
                        w.objects
                            .find_by_part_id(crate::ids::PartId(entry.params[0]))
                    })
                    .flatten();
                if obj.is_none() && entry.params[2] == -1 {
                    // for now, return the closest enemy building...
                    let mut smallest = 999_999.0f32;
                    for b in w.objects.buildings.clone() {
                        let Some(o) = w.objects.get(b) else { continue };
                        let enemy = crate::object::hooks::team_relation(w, team_id, o.team_id())
                            == Some(super::Relation::Enemy)
                            && crate::object::hooks::team_exists(w, o.team_id());
                        let wall = w
                            .objects
                            .type_of(b)
                            .is_some_and(|t| {
                                matches!(&t.data, mc2_sim_data::objtype::ObjectTypeData::Building(bt) if bt.sub_type == mc2_sim_data::objtype::building::BuildingSubType::Wall)
                            });
                        if enemy && !wall && !o.c.is_destroyed() {
                            let dist = super::mover_ops::distance_from(w, me, &o.c.position);
                            if dist < smallest {
                                obj = Some(b);
                                smallest = dist;
                            }
                        }
                    }
                }
                if let Some(o) = obj {
                    let pos = w.objects.get(o).map(|x| x.c.position).unwrap_or_default();
                    if f64::from(super::mover_ops::distance_from(w, me, &pos))
                        < f64::from(entry.params[1])
                    {
                        return i32::try_from(w.objects.watch_id(o).0).unwrap_or(0);
                    }
                }
            }
            prio::CURTARGET => {
                // param 0 = disengage relative to point #, 1 = disengage radius, 2 = contact criteria (0 for doesn't matter)
                let mut target: Option<ObjectHandle> = None;
                let (cur_code, cur_target, obliterate) = {
                    let p = w.pilots.get(slot).expect("pilot");
                    (
                        p.cur_order.code,
                        p.cur_order.target,
                        p.cur_order.attack_params.obliterate,
                    )
                };
                if cur_code != OrderCode::None {
                    // `ObjectManager->get(targetWID)`: indexes by HANDLE (a watch id used as a handle)
                    target = (cur_target.0 >= 1
                        && (cur_target.0 as usize) <= w.objects.max_objects())
                    .then_some(ObjectHandle(cur_target.0))
                    .filter(|&h| w.objects.get(h).is_some());
                    if let Some(t) = target {
                        if w.objects.get(t).is_some_and(|o| o.c.is_disabled()) && !obliterate {
                            target = None;
                        }
                    }
                }
                if target.is_none() {
                    target = super::warrior::get_last_target(w, slot);
                }
                if let Some(t) = target {
                    let my_pos = w.objects.get(me).map(|o| o.c.position).unwrap_or_default();
                    let tpos = w.objects.get(t).map(|o| o.c.position).unwrap_or_default();
                    let delta = Vec3 {
                        x: my_pos.x - tpos.x,
                        y: my_pos.y - tpos.y,
                        z: 0.0,
                    };
                    let distance = delta.length_f() * crate::terrain::METERS_PER_WORLD_UNIT;
                    if f64::from(distance) < f64::from(entry.params[1]) {
                        let is_mover = w.objects.get(t).is_some_and(|o| o.is_mover());
                        if entry.params[2] != 0 && is_mover {
                            if team_is_contact(w, me, t, entry.params[2]) {
                                return i32::try_from(w.objects.watch_id(t).0).unwrap_or(0);
                            }
                        } else {
                            return i32::try_from(w.objects.watch_id(t).0).unwrap_or(0);
                        }
                    }
                }
            }
            prio::TURRET => {
                // For now, we'll pick the closest Turret... (the final `list[i]` below reads the entry at the shadowing loop index,
                // `ObjectManager->getNumTurrets()`: zero when past the end of the 20-entry list)
                let mut shortest = 999_999.0f32;
                let mut closest: Option<ObjectHandle> = None;
                let turrets = w.objects.turrets.clone();
                for &t in &turrets {
                    let Some(o) = w.objects.get(t) else { continue };
                    let friendly = crate::object::hooks::team_relation(w, o.team_id(), team_id)
                        == Some(super::Relation::Friendly);
                    if !friendly {
                        let d = super::mover_ops::distance_from(w, me, &o.c.position);
                        if d < shortest {
                            shortest = d;
                            closest = Some(t);
                        }
                    }
                }
                if let Some(c) = closest {
                    let radius = w
                        .pilots
                        .get(slot)
                        .and_then(|p| p.target_priorities.list.get(turrets.len()))
                        .map_or(0, |e| e.params[1]);
                    let pos = w.objects.get(c).map(|x| x.c.position).unwrap_or_default();
                    if f64::from(super::mover_ops::distance_from(w, me, &pos)) < f64::from(radius) {
                        return i32::try_from(w.objects.watch_id(c).0).unwrap_or(0);
                    }
                }
                return 0;
            }
            prio::TURRET_CONTROL | prio::GATE_CONTROL => {
                let controls = if entry.kind == prio::TURRET_CONTROL {
                    w.objects.turret_controls.clone()
                } else {
                    w.objects.gate_controls.clone()
                };
                let mut closest: Option<ObjectHandle> = None;
                let mut shortest = f32::INFINITY;
                for control in controls {
                    let Some(obj) = w.objects.get(control) else {
                        continue;
                    };
                    let relation = crate::object::hooks::team_relation(w, team_id, obj.team_id());
                    if relation != Some(super::Relation::Friendly)
                        && super::capture::is_captureable(w, control, team_id)
                    {
                        let distance = super::mover_ops::distance_from(w, me, &obj.c.position);
                        if distance < shortest {
                            shortest = distance;
                            closest = Some(control);
                        }
                    }
                }
                if let Some(control) = closest {
                    return i32::try_from(w.objects.watch_id(control).0).unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    0
}

/// `list[i].params[2] += CONTACT_CRITERIA_ARMED`.
fn bump_armed(w: &mut World, slot: u16, i: usize) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.target_priorities.list[i].params[2] =
            p.target_priorities.list[i].params[2].wrapping_add(criteria::ARMED);
    }
}

/// `MechWarrior::coreScan(object, params)` (warrior.cpp:5806): `coreScanTargetWID` and its return value.
pub fn core_scan(w: &mut World, slot: u16, object: Option<ObjectHandle>, params: u32) -> i32 {
    let Some(me) = super::warrior::vehicle(w, slot) else {
        return 0;
    };
    let my_team = super::warrior::team_id(w, slot);
    if let Some(obj) = object.filter(|&o| {
        w.objects
            .get(o)
            .is_some_and(|x| crate::object::hooks::team_exists(w, x.team_id()))
    }) {
        // If this object is currently a contact, set myTarget to it and return true. Otherwise, return false.
        let is_mover = w.objects.get(obj).is_some_and(|o| o.is_mover());
        if is_mover {
            if team_is_contact(w, me, obj, params as i32) {
                let wid = w.objects.watch_id(obj);
                set_scan_target(w, slot, wid);
                return wid.0 as i32;
            }
        } else {
            if crate::object::line_of_sight(w, me, obj, 0.0, true) {
                let wid = w.objects.watch_id(obj);
                set_scan_target(w, slot, wid);
                return wid.0 as i32;
            }
        }
    } else {
        // Scan for anything matching the criteria...
        let my_position = w.objects.get(me).map(|o| o.c.position).unwrap_or_default();
        let _ = my_team;
        let wid = calc_target(w, slot, my_position, params as i32);
        set_scan_target(w, slot, WatchId(wid as u32));
        return wid;
    }
    set_scan_target(w, slot, WatchId(0));
    0
}

fn set_scan_target(w: &mut World, slot: u16, wid: WatchId) {
    if let Some(p) = w.pilots.get_mut(slot) {
        p.core_scan_target = wid;
    }
}

/// `Vector3D::GetLength()` for the planar vector of the disengage test.
trait LenExt {
    fn length_f(&self) -> f32;
}
impl LenExt for Vec3 {
    fn length_f(&self) -> f32 {
        crate::math::approximate_length(self)
    }
}

#[allow(dead_code)]
fn _kinds(_: &Kind, _: &TargetPriorityList) {}
