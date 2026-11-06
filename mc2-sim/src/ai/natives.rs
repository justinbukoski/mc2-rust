// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Lane D's ABL natives (`code/ablmc2.cpp`: pilot and brain state, contacts and targets, tactical orders, the `core*` orders) over the
//! simulation world. [`SimBindings`](crate::mission::bindings::SimBindings) forwards the matching [`GameBindings`] methods here.
//!
//! Conventions of the C++ kept: a part id of -1 means `CurObject` (the running pilot's mover); ids 1..511 are unit / team ids (500..508
//! are teams); `CurWarrior`, `CurContact` and `CurAlarm` are fields of the bindings. Natives that the C++ calls with a null
//! `CurObject` / `CurWarrior` (the mission script) dereference null there; here they answer the neutral value.

// Several loops mirror ablmc2.cpp branch for branch (`calcAttackPlan`, `calcBestTarget`): keep the shape of the source.
#![allow(clippy::if_same_then_else)]

use super::pilot::NUM_ALARMS;
use super::targeting::{self, SORT_DISTANCE};
use super::warrior::{self, vehicle};
use super::{mover_ops, orders, report_unavailable, AiUnavailable};
use crate::command::{AttackMethod, OrderCode, OrderOrigin};
use crate::ids::{ObjectHandle, WatchId};
use crate::math::Vec3;
use crate::mission::bindings::SimBindings;
use mc2_abl::{ArrayRef, NativeCtx};

/// `MIN_UNIT_PART_ID`, `MAX_UNIT_PART_ID` (unitdesg.h).
const MIN_UNIT_PART_ID: i32 = 1;
const MAX_UNIT_PART_ID: i32 = 511;
/// `OBJ_ID_FIRST_TEAM`, `OBJ_ID_LAST_TEAM`.
const OBJ_ID_FIRST_TEAM: i32 = 500;
const OBJ_ID_LAST_TEAM: i32 = 508;
/// `OBJ_ID_FIRST_COMMANDER`, `OBJ_ID_LAST_COMMANDER`.
const OBJ_ID_FIRST_COMMANDER: i32 = 492;
const OBJ_ID_LAST_COMMANDER: i32 = 499;

fn is_unit_id(id: i32) -> bool {
    (MIN_UNIT_PART_ID..=MAX_UNIT_PART_ID).contains(&id)
}

impl SimBindings<'_> {
    /// The mover `CurObject` points to.
    fn cur_object_handle(&self) -> Option<ObjectHandle> {
        self.world.objects.by_watch_id(self.cur_object)
    }

    /// `getObject(partId)`: -1 is `CurObject`, anything else `findByPartId`.
    fn object(&self, part_id: i32) -> Option<ObjectHandle> {
        if part_id == -1 {
            self.cur_object_handle()
        } else {
            self.world
                .objects
                .find_by_part_id(crate::ids::PartId(part_id))
        }
    }

    /// `getObject(partId, existsOnly)`.
    fn object_exists_only(&self, part_id: i32) -> Option<ObjectHandle> {
        self.object(part_id)
            .filter(|&h| self.world.objects.get(h).is_some_and(|o| o.exists()))
    }

    /// `CurWarrior`.
    fn warrior_slot(&self) -> Option<u16> {
        self.cur_warrior
    }

    /// The pilot slot of a mover.
    fn pilot_of(&self, h: ObjectHandle) -> Option<u16> {
        warrior::slot_of(self.world, h)
    }

    /// `getMovers(partId, list, existsOnly)` (ablmc2.cpp:344): a team's roster, or every mover whose squad id is `part_id`.
    fn movers_of(&self, part_id: i32, exists_only: bool) -> Vec<ObjectHandle> {
        let w = &*self.world;
        if (OBJ_ID_FIRST_COMMANDER..=OBJ_ID_LAST_COMMANDER).contains(&part_id) {
            // Fatal(3, " ABL.getMovers: bad id ")
            return Vec::new();
        }
        if (OBJ_ID_FIRST_TEAM..=OBJ_ID_LAST_TEAM).contains(&part_id) {
            let team = (part_id - OBJ_ID_FIRST_TEAM) as usize;
            return w
                .teams
                .teams
                .get(team)
                .and_then(|t| t.as_ref())
                .map(|t| {
                    t.roster
                        .iter()
                        .filter_map(|&wid| w.objects.by_watch_id(wid))
                        .filter(|&h| !exists_only || w.objects.get(h).is_some_and(|o| o.exists()))
                        .collect()
                })
                .unwrap_or_default();
        }
        w.objects
            .movers
            .iter()
            .copied()
            .filter(|&h| {
                w.objects
                    .get(h)
                    .and_then(crate::combat::core_of)
                    .is_some_and(|c| c.squad_id == part_id)
            })
            .collect()
    }

    fn part_id_of(&self, h: ObjectHandle) -> i32 {
        self.world.objects.get(h).map_or(0, |o| o.c.part_id.0)
    }

    fn unavailable(&mut self, what: AiUnavailable) {
        if let Some(h) = self.cur_object_handle() {
            report_unavailable(self.world, h, what);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Selection and queries
// ---------------------------------------------------------------------------------------------------------------------

/// `execGetId`.
pub fn get_id(b: &mut SimBindings<'_>) -> i32 {
    b.cur_object_handle().map_or(0, |h| b.part_id_of(h))
}

/// `execSelectObject`: `CurObject = getObject(id)`; returns the previous part id (-1 when the object does not exist).
pub fn select_object(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    let cur_id = b.cur_object_handle().map_or(0, |h| b.part_id_of(h));
    match b.object(object_id) {
        Some(h) => {
            b.cur_object = b.world.objects.watch_id(h);
            cur_id
        }
        None => -1,
    }
}

/// `execSelectWarrior`: `CurWarrior = getWarrior(index)` (-1 keeps it); returns the previous warrior index.
pub fn select_warrior(b: &mut SimBindings<'_>, warrior_index: i32) -> i32 {
    let cur = b.cur_warrior.map_or(0, i32::from);
    b.cur_warrior = if warrior_index == -1 {
        b.cur_warrior
    } else {
        u16::try_from(warrior_index)
            .ok()
            .filter(|&i| usize::from(i) < b.world.pilots.slots.len())
    };
    cur
}

/// `execGetWarriorStatus`.
pub fn get_warrior_status(b: &mut SimBindings<'_>, warrior_id: i32) -> i32 {
    let slot = if warrior_id == -1 {
        b.cur_warrior
    } else {
        u16::try_from(warrior_id).ok()
    };
    slot.and_then(|s| b.world.pilots.get(s))
        .map_or(-1, |p| p.status as i32)
}

/// `execGetContacts`: part ids of the contacts of `CurObject`'s team sensor, filtered and sorted.
pub fn get_contacts(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    criteria: i32,
    sort: i32,
) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    let contacts = targeting::get_contacts(b.world, me, criteria, sort);
    for (i, c) in contacts.iter().enumerate() {
        let part = b.part_id_of(*c);
        ctx.set_i32(list, i, part);
    }
    contacts.len() as i32
}

/// `execGetEnemyCount`: the number of contacts of a team's sensor system.
pub fn get_enemy_count(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    let team = if is_unit_id(object_id) {
        if (OBJ_ID_FIRST_TEAM..=OBJ_ID_LAST_TEAM).contains(&object_id) {
            let t = (object_id - OBJ_ID_FIRST_TEAM) as usize;
            b.world
                .teams
                .teams
                .get(t)
                .is_some_and(|x| x.is_some())
                .then_some(t)
        } else {
            None
        }
    } else {
        b.object(object_id).and_then(|h| {
            let o = b.world.objects.get(h)?;
            let t = o.team_id();
            let ok = o.is_mover() && crate::object::hooks::team_exists(b.world, t)
                || matches!(
                    o.c.class,
                    crate::object::ObjectClass::Building | crate::object::ObjectClass::Artillery
                ) && t > -1;
            ok.then_some(t as usize)
        })
    };
    match team {
        Some(t) => b.world.sensors.team_contact_order[t % crate::ids::MAX_TEAMS].len() as i32,
        None => -1,
    }
}

/// `execSelectContact`: `CurContact = findByPartId(id)` when the object is a contact of `CurObject`'s team.
pub fn select_contact(b: &mut SimBindings<'_>, _type: i32, id: i32) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return -1;
    };
    if !b.world.objects.get(me).is_some_and(|o| o.is_mover()) {
        return -1;
    }
    let team = b.world.objects.get(me).map_or(-1, |o| o.team_id());
    let object = b.world.objects.find_by_part_id(crate::ids::PartId(id));
    match object {
        Some(o)
            if b.world
                .objects
                .get(o)
                .is_some_and(|x| crate::object::hooks::team_exists(b.world, x.team_id()))
                && targeting::contact_status(b.world, o, team) != 0 =>
        {
            b.cur_contact = Some(o);
            0
        }
        _ => 1,
    }
}

/// `execIsContact`.
pub fn is_contact(b: &mut SimBindings<'_>, object_id: i32, criteria: i32, select: i32) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return -1;
    };
    let my_team = b.world.objects.get(me).map_or(-1, |o| o.team_id());
    if !(b.world.objects.get(me).is_some_and(|o| o.is_mover())
        && crate::object::hooks::team_exists(b.world, my_team))
    {
        return -1;
    }
    let Some(object) = b.object(object_id) else {
        return 0;
    };
    if b.world.objects.get(object).is_some_and(|o| o.is_mover())
        && targeting::meets_criteria(b.world, my_team, object, criteria)
    {
        if select != 0 {
            b.cur_contact = Some(object);
        }
        object_id
    } else {
        0
    }
}

/// `execGetContactId`.
pub fn get_contact_id(b: &mut SimBindings<'_>) -> i32 {
    b.cur_contact.map_or(0, |c| b.part_id_of(c))
}

/// `execGetContactStatus`: the `contactStatus` of `CurContact` for `CurObject`'s team; the tagged flag is cleared.
pub fn get_contact_status(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    tagged: ArrayRef,
) -> i32 {
    ctx.set_i32(tagged, 0, 0);
    let (Some(c), Some(me)) = (b.cur_contact, b.cur_object_handle()) else {
        return 0;
    };
    let team = b.world.objects.get(me).map_or(-1, |o| o.team_id());
    if !b.world.objects.get(me).is_some_and(|o| o.is_mover()) {
        return 0;
    }
    i32::from(targeting::contact_status(b.world, c, team))
}

/// `execGetContactRelativePosition`: the by-reference outputs are stack temporaries in the C++ (lost); only the result code is seen.
pub fn get_contact_relative_position(b: &mut SimBindings<'_>) -> i32 {
    if b.cur_contact.is_some() && b.cur_object_handle().is_some() {
        0
    } else {
        1
    }
}

/// `execSetTarget`.
pub fn set_target(b: &mut SimBindings<'_>, attacker_id: i32, target_id: i32) {
    let target = b.object(target_id);
    let movers: Vec<ObjectHandle> = if is_unit_id(attacker_id) {
        b.movers_of(attacker_id, false)
    } else {
        b.object(attacker_id).into_iter().collect()
    };
    for m in movers {
        if let Some(slot) = b.pilot_of(m) {
            warrior::set_last_target(b.world, slot, target, false, false);
            super::tacorder::calc_fire_ranges(b.world, m);
        }
    }
}

/// `execGetTarget`.
pub fn get_target(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    if is_unit_id(object_id) {
        return 0;
    }
    let Some(obj) = b.object(object_id) else {
        return 0;
    };
    if !b.world.objects.get(obj).is_some_and(|o| o.is_mover()) {
        return 0;
    }
    let Some(slot) = b.pilot_of(obj) else {
        return 0;
    };
    warrior::get_current_target(b.world, slot).map_or(0, |t| b.part_id_of(t))
}

/// `Mover::getWeaponsReady(list, listSize)` for `execGetWeaponsReady`.
pub fn get_weapons_ready(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    list_size: i32,
) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    let Some(l) = b.world.objects.get(me).and_then(crate::combat::loadout_of) else {
        return 0;
    };
    let (num_other, num_weapons) = (l.num_other(), l.num_weapons());
    let mut n = 0usize;
    if list_size == -1 {
        for item in num_other..num_other + num_weapons {
            if mover_ops::is_weapon_ready(b.world, me, item) {
                ctx.set_i32(list, n, item as i32);
                n += 1;
            }
        }
    } else {
        for i in 0..list_size.max(0) as usize {
            let item = ctx.get_i32(list, i);
            if item >= 0 && mover_ops::is_weapon_ready(b.world, me, item as usize) {
                ctx.set_i32(list, n, item);
                n += 1;
            }
        }
    }
    n as i32
}

fn target_of_cur_warrior(b: &mut SimBindings<'_>) -> Option<ObjectHandle> {
    let slot = b.warrior_slot()?;
    warrior::get_current_target(b.world, slot)
}

/// `Mover::getWeaponsLocked`.
pub fn get_weapons_locked(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    list_size: i32,
) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    // `pilot->getCurrentTarget()` of the MOVER's pilot (the same pilot as CurWarrior inside a brain)
    let Some(slot) = b.pilot_of(me) else { return 0 };
    let Some(target) = warrior::get_current_target(b.world, slot) else {
        return -2;
    };
    let target_position = b
        .world
        .objects
        .get(target)
        .map(|o| o.c.position)
        .unwrap_or_default();
    let Some(gs) = b.world.data.gamesys.tables.clone() else {
        return 0;
    };
    let fire_arc = super::decide::fire_arc(b.world, me, &gs);
    let (num_other, num_weapons) = b
        .world
        .objects
        .get(me)
        .and_then(crate::combat::loadout_of)
        .map_or((0, 0), |l| (l.num_other(), l.num_weapons()));
    let locked = |b: &SimBindings<'_>, item: usize| {
        let body = b
            .world
            .objects
            .get(me)
            .and_then(crate::combat::loadout_of)
            .and_then(|l| l.inventory.get(item))
            .map_or(0, |i| i32::from(i.body_location));
        let rel = super::decide::rel_facing_to(b.world, me, &target_position, body);
        rel >= -fire_arc && rel <= fire_arc
    };
    let mut n = 0usize;
    if list_size == -1 {
        for item in num_other..num_other + num_weapons {
            if locked(b, item) {
                ctx.set_i32(list, n, item as i32);
                n += 1;
            }
        }
    } else {
        for i in 0..list_size.max(0) as usize {
            let item = ctx.get_i32(list, i);
            if item >= 0 && locked(b, item as usize) {
                ctx.set_i32(list, n, item);
                n += 1;
            }
        }
    }
    n as i32
}

/// `Mover::getWeaponsInRange`.
pub fn get_weapons_in_range(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    list_size: i32,
) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    let Some(target) = target_of_cur_warrior(b) else {
        return 0;
    };
    let target_pos = b
        .world
        .objects
        .get(target)
        .map(|o| o.c.position)
        .unwrap_or_default();
    let range = mover_ops::distance_from(b.world, me, &target_pos);
    let diag = crate::combat::attack::map_cell_diagonal();
    let (num_other, num_weapons) = b
        .world
        .objects
        .get(me)
        .and_then(crate::combat::loadout_of)
        .map_or((0, 0), |l| (l.num_other(), l.num_weapons()));
    let mut n = 0usize;
    if list_size == -1 {
        for item in num_other..num_other + num_weapons {
            if mover_ops::weapon_in_range(b.world, me, item, range, diag) {
                ctx.set_i32(list, n, item as i32);
                n += 1;
            }
        }
    } else {
        for i in 0..list_size.max(0) as usize {
            let item = ctx.get_i32(list, i);
            if item >= 0 && mover_ops::weapon_in_range(b.world, me, item as usize, range, diag) {
                ctx.set_i32(list, n, item);
                n += 1;
            }
        }
    }
    n as i32
}

/// `execGetWeaponShots`.
pub fn get_weapon_shots(b: &mut SimBindings<'_>, weapon_index: i32) -> i32 {
    match b.cur_object_handle() {
        Some(me) => mover_ops::weapon_shots(b.world, me, weapon_index),
        None => 0,
    }
}

/// `execGetWeaponRanges`: min, optimal and max fire range of a mover.
pub fn get_weapon_ranges(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    list: ArrayRef,
) {
    let ranges = b
        .object(object_id)
        .and_then(|h| b.world.objects.get(h))
        .filter(|o| o.is_mover())
        .and_then(crate::combat::loadout_of)
        .map_or([0.0; 3], |l| {
            [l.stats.min_range, l.stats.optimal_range, l.stats.max_range]
        });
    for (i, r) in ranges.iter().enumerate() {
        ctx.set_f32(list, i, *r);
    }
}

/// `execGetObjectPosition`.
pub fn get_object_position(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    list: ArrayRef,
) {
    let pos = b
        .object(object_id)
        .and_then(|h| b.world.objects.get(h))
        .map_or(Vec3::default(), |o| o.c.position);
    ctx.set_f32(list, 0, pos.x);
    ctx.set_f32(list, 1, pos.y);
    ctx.set_f32(list, 2, pos.z);
}

/// `execGetIntegerMemory` / `execGetRealMemory`: the memory cells are a union of int and float (bit patterns).
pub fn get_integer_memory(b: &mut SimBindings<'_>, index: i32) -> i32 {
    b.warrior_slot()
        .and_then(|s| b.world.pilots.get(s))
        .and_then(|p| usize::try_from(index).ok().and_then(|i| p.memory.get(i)))
        .copied()
        .unwrap_or(0)
}

/// See [`get_integer_memory`].
pub fn get_real_memory(b: &mut SimBindings<'_>, index: i32) -> f32 {
    f32::from_bits(get_integer_memory(b, index) as u32)
}

/// `execSetIntegerMemory`.
pub fn set_integer_memory(b: &mut SimBindings<'_>, index: i32, value: i32) {
    if let Some(p) = b.warrior_slot().and_then(|s| b.world.pilots.get_mut(s)) {
        if let Some(cell) = usize::try_from(index)
            .ok()
            .and_then(|i| p.memory.get_mut(i))
        {
            *cell = value;
        }
    }
}

/// `execSetRealMemory`.
pub fn set_real_memory(b: &mut SimBindings<'_>, index: i32, value: f32) {
    set_integer_memory(b, index, value.to_bits() as i32);
}

/// `execGetAlarmTriggers`: `getEventHistory(CurAlarm, list)`.
pub fn get_alarm_triggers(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, list: ArrayRef) -> i32 {
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    let code = usize::try_from(b.cur_alarm)
        .unwrap_or(0)
        .min(NUM_ALARMS - 1);
    let triggers = warrior::get_event_history(b.world, slot, code);
    for (i, t) in triggers.iter().enumerate() {
        ctx.set_i32(list, i, *t);
    }
    triggers.len() as i32
}

/// `execGetChallenger`.
pub fn get_challenger(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    if is_unit_id(object_id) {
        return 0;
    }
    let Some(obj) = b.object(object_id) else {
        return 0;
    };
    if !b.world.objects.get(obj).is_some_and(|o| o.is_mover()) {
        return 0;
    }
    super::get_challenger_handle(b.world, obj).map_or(0, |c| b.part_id_of(c))
}

/// `execSetChallenger`.
pub fn set_challenger(b: &mut SimBindings<'_>, victim_id: i32, challenger_id: i32) -> i32 {
    if is_unit_id(challenger_id) {
        return -1;
    }
    let challenger = b.object(challenger_id);
    let victim = b.object(victim_id);
    match victim.filter(|&v| b.world.objects.get(v).is_some_and(|o| o.is_mover())) {
        Some(v) => {
            // `challenger->getWatchID()` (a NULL challenger is undefined behaviour in the source: forgotten here)
            let wid = challenger.map_or(WatchId(0), |c| b.world.objects.watch_id(c));
            if let Some(slot) = b.pilot_of(v) {
                if let Some(p) = b.world.pilots.get_mut(slot) {
                    p.challenger = wid;
                }
            }
            0
        }
        None => -2,
    }
}

/// `execGetTimeWithoutOrders`.
pub fn get_time_without_orders(b: &mut SimBindings<'_>) -> f32 {
    let now = b.world.clock.scenario_time;
    match b.warrior_slot().and_then(|s| b.world.pilots.get(s)) {
        Some(p) if p.time_of_last_orders >= 0.0 => now - p.time_of_last_orders,
        _ => 0.0,
    }
}

/// `execGetFireRanges`: `WeaponRange[SHORT/MEDIUM/LONG]` and 250.
pub fn get_fire_ranges(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, ranges: ArrayRef) {
    // `WeaponRange[]` (mission.cpp:1789): the long end of each range class
    let wr = b
        .world
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or([0.0f32; 4], |t| t.general.weapon_range);
    // WeaponRange[FIRERANGE_SHORT = 0], [MEDIUM = 1], [LONG = 2]
    ctx.set_f32(ranges, 0, wr[0]);
    ctx.set_f32(ranges, 1, wr[1]);
    ctx.set_f32(ranges, 2, wr[2]);
    ctx.set_f32(ranges, 3, 250.0);
}

/// `execGetAttackers`.
pub fn get_attackers(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    seconds: f32,
) -> i32 {
    let now = b.world.clock.scenario_time;
    let Some(p) = b.warrior_slot().and_then(|s| b.world.pilots.get(s)) else {
        return 0;
    };
    let earliest = now - seconds;
    let mut n = 0usize;
    for &(wid, t) in &p.attackers {
        if t >= earliest {
            ctx.set_i32(list, n, wid.0 as i32);
            n += 1;
        }
    }
    n as i32
}

/// `execGetAttackerInfo`.
pub fn get_attacker_info(b: &mut SimBindings<'_>, attacker_id: i32) -> f32 {
    let now = b.world.clock.scenario_time;
    let mut since = 1_000_000.0;
    if !is_unit_id(attacker_id) {
        if let Some(p) = b.warrior_slot().and_then(|s| b.world.pilots.get(s)) {
            if let Some(&(_, t)) = p.attackers.iter().find(|(w, _)| w.0 == attacker_id as u32) {
                since = now - t;
            }
        }
    }
    since
}

/// `execHasMoveGoal`.
pub fn has_move_goal(b: &mut SimBindings<'_>) -> bool {
    b.warrior_slot()
        .and_then(|s| b.world.pilots.get(s))
        .is_some_and(|p| p.lane_b_move.new_goal && p.lane_b_move.has_move_goal())
}

/// `execHasMovePath`: `getMovePath() && !getMoveNewGoal()` (`getMovePath` always returns a path pointer; it also runs the spare swap).
pub fn has_move_path(b: &mut SimBindings<'_>) -> bool {
    let Some(slot) = b.warrior_slot() else {
        return false;
    };
    warrior::get_move_path_index(b.world, slot);
    !b.world
        .pilots
        .get(slot)
        .is_some_and(|p| p.lane_b_move.new_goal)
}

/// `execSortWeapons` (`WEAPONSORT_ATTACKCHANCE` only).
pub fn sort_weapons(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    list_size: i32,
    sort_type: i32,
) {
    let Some(me) = b.cur_object_handle() else {
        return;
    };
    if !b.world.objects.get(me).is_some_and(|o| o.is_mover()) {
        return;
    }
    let Some(slot) = b.pilot_of(me) else { return };
    let Some(target) = warrior::get_current_target(b.world, slot) else {
        return;
    };
    if sort_type != 0 {
        return;
    }
    let aim = b
        .world
        .pilots
        .get(slot)
        .filter(|p| p.cur_order.is_combat_order())
        .map_or(-1, |p| p.cur_order.attack_params.aim_location);
    let (num_other, num_weapons) = b
        .world
        .objects
        .get(me)
        .and_then(crate::combat::loadout_of)
        .map_or((0, 0), |l| (l.num_other(), l.num_weapons()));
    let chance = |b: &SimBindings<'_>, item: i32| {
        crate::combat::attack::calc_attack_chance(b.world, me, Some(target), aim, item, None).chance
    };
    // SortList::sort is a stable descending sort (DESIGN.md section 4)
    let mut entries: Vec<(i32, f32)> = if list_size == -1 {
        (num_other..num_other + num_weapons)
            .map(|i| (i as i32, chance(b, i as i32)))
            .collect()
    } else {
        (0..list_size.max(0) as usize)
            .map(|i| {
                let item = ctx.get_i32(list, i);
                (item, if item == -1 { -999.0 } else { chance(b, item) })
            })
            .collect()
    };
    entries.sort_by(|a, c| c.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (i, (item, _)) in entries.iter().enumerate() {
        ctx.set_i32(list, i, *item);
    }
}

/// `execGetVisualRange`: `MechWarrior::maxVisualRadius`.
pub fn get_visual_range(b: &mut SimBindings<'_>, object_id: i32) -> f32 {
    if is_unit_id(object_id) {
        return -1.0;
    }
    match b.object(object_id) {
        Some(h) if b.world.objects.get(h).is_some_and(|o| o.is_mover()) => b
            .world
            .data
            .gamesys
            .tables
            .as_ref()
            .map_or(100.0, |t| t.general.max_visual_radius),
        _ => 0.0,
    }
}

/// `execGetUnitMates` (groups are `USE_GROUPS`-only: a single mover has none).
pub fn get_unit_mates(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    list: ArrayRef,
) -> i32 {
    if is_unit_id(object_id) {
        let movers = b.movers_of(object_id, false);
        for (i, m) in movers.iter().enumerate() {
            let part = b.part_id_of(*m);
            ctx.set_i32(list, i, part);
        }
        return movers.len() as i32;
    }
    0
}

/// `TacticalOrder::getParamData`: the order's parameter list, `code` returned.
fn param_data(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    order: &crate::command::TacticalOrder,
    list: ArrayRef,
) -> i32 {
    use crate::command::TravelMode;
    let target = b.world.objects.by_watch_id(order.target);
    let target_part = target.map_or(0, |t| b.part_id_of(t));
    let p = &order.move_params;
    let fast = i32::from(p.way_path.mode.first() == Some(&TravelMode::Fast));
    let set = |ctx: &mut NativeCtx<'_>, i: usize, v: i32| ctx.set_i32(list, i, v);
    set(ctx, 0, order.code as i32);
    set(ctx, 1, order.origin as i32);
    set(ctx, 2, i32::from(order.unit_order));
    set(ctx, 3, target_part);
    set(ctx, 4, order.target_object_class);
    set(ctx, 5, i32::from(order.sub_order));
    set(ctx, 6, order.selection_index);
    let pt = |i: usize| {
        p.way_path
            .points
            .first()
            .map_or(0.0, |v| [v.x, v.y, v.z][i])
    };
    match order.code {
        OrderCode::MoveToPoint | OrderCode::Guard => {
            set(ctx, 7, fast);
            set(ctx, 8, i32::from(p.wait));
            set(ctx, 9, pt(0) as i32);
            set(ctx, 10, pt(1) as i32);
            set(ctx, 11, pt(2) as i32);
        }
        OrderCode::MoveToObject => {
            set(ctx, 7, fast);
            set(ctx, 8, i32::from(p.wait));
            set(ctx, 9, i32::from(p.face_object));
        }
        OrderCode::AttackObject => {
            let a = &order.attack_params;
            set(ctx, 7, fast);
            set(ctx, 8, a.attack_type);
            set(ctx, 9, a.method as i32);
            set(ctx, 10, a.range);
            set(ctx, 11, a.aim_location);
            set(ctx, 12, i32::from(a.pursue));
            set(ctx, 13, i32::from(a.obliterate));
        }
        _ => {}
    }
    order.code as i32
}

/// `execGetTacOrder`.
pub fn get_tac_order(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    list: ArrayRef,
) -> i32 {
    if is_unit_id(object_id) {
        return 0;
    }
    let Some(obj) = b
        .object(object_id)
        .filter(|&o| b.world.objects.get(o).is_some_and(|x| x.is_mover()))
    else {
        return 0;
    };
    let Some(slot) = b.pilot_of(obj) else {
        return 0;
    };
    let order = b.world.pilots.get(slot).expect("pilot").cur_order.clone();
    let code = param_data(b, ctx, &order, list);
    let (goal_action, goal_obj) = {
        let g = &b.world.pilots.get(slot).expect("pilot").main_goal;
        (g.action, b.world.objects.by_watch_id(g.object))
    };
    ctx.set_i32(list, 17, goal_action);
    ctx.set_i32(list, 18, goal_obj.map_or(0, |o| b.part_id_of(o)));
    code
}

/// `execGetLastTacOrder`.
pub fn get_last_tac_order(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    list: ArrayRef,
) -> i32 {
    if is_unit_id(object_id) {
        return 0;
    }
    let Some(obj) = b
        .object(object_id)
        .filter(|&o| b.world.objects.get(o).is_some_and(|x| x.is_mover()))
    else {
        return 0;
    };
    let Some(slot) = b.pilot_of(obj) else {
        return 0;
    };
    let order = b.world.pilots.get(slot).expect("pilot").last_order.clone();
    param_data(b, ctx, &order, list)
}

/// `execGetObjects`.
pub fn get_objects(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    criteria: i32,
    list: ArrayRef,
) -> i32 {
    let w = &*b.world;
    let part = |h: &ObjectHandle| w.objects.get(*h).map(|o| o.c.part_id.0);
    let mut ids: Vec<i32> = Vec::new();
    let movers = || {
        w.objects
            .movers
            .iter()
            .filter(|h| w.objects.get(**h).is_some_and(|o| o.exists()))
            .filter_map(part)
            .collect::<Vec<_>>()
    };
    match criteria {
        0 => ids = movers(),
        1 => ids = w.objects.terrain_objects.iter().filter_map(part).collect(),
        2 => ids = w.objects.buildings.iter().filter_map(part).collect(),
        3 => ids = w.objects.turrets.iter().filter_map(part).collect(),
        4 => ids = w.objects.gates.iter().filter_map(part).collect(),
        5 => {
            ids.extend(w.objects.terrain_objects.iter().filter_map(part));
            ids.extend(w.objects.buildings.iter().filter_map(part));
            ids.extend(w.objects.turrets.iter().filter_map(part));
            ids.extend(w.objects.gates.iter().filter_map(part));
        }
        _ => {}
    }
    for (i, id) in ids.iter().enumerate() {
        ctx.set_i32(list, i, *id);
    }
    ids.len() as i32
}

// ---------------------------------------------------------------------------------------------------------------------
// order*
// ---------------------------------------------------------------------------------------------------------------------

/// `double2long` (ablmc2.cpp:318): `(int)round-to-nearest-even` via the magic-number trick.
fn double2long(x: f32) -> i32 {
    let v = f64::from(x) + 6_755_399_441_055_744.0;
    (v.to_bits() & 0xffff_ffff) as u32 as i32
}

/// `execOrderWait`.
pub fn order_wait(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    seconds: f32,
    clear_last_target: bool,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    orders::order_wait(
        b.world,
        slot,
        false,
        OrderOrigin::Commander,
        double2long(seconds),
        clear_last_target,
    )
}

/// `execOrderMoveTo`.
pub fn order_move_to(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    coords: ArrayRef,
    run: bool,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    let location = Vec3 {
        x: ctx.get_f32(coords, 0),
        y: ctx.get_f32(coords, 1),
        z: ctx.get_f32(coords, 2),
    };
    let params = if run {
        orders::param::RUN
    } else {
        orders::param::NONE
    };
    orders::order_move_to_point(
        b.world,
        slot,
        false,
        true,
        OrderOrigin::Commander,
        location,
        -1,
        params,
    )
}

/// `execOrderMoveToObject`.
pub fn order_move_to_object(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    run: bool,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    if is_unit_id(object_id) {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 1;
    };
    match b.object(object_id) {
        // `orderMoveToObject(.., fromArea -1, selectionIndex -1, params = run)`: the C++ passes the bool as the params mask
        Some(obj) => orders::order_move_to_object(
            b.world,
            slot,
            false,
            true,
            OrderOrigin::Commander,
            Some(obj),
            -1,
            -1,
            u32::from(run),
        ),
        None => 1,
    }
}

/// `execOrderMoveToContact`.
pub fn order_move_to_contact(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, run: bool) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    match b.cur_contact {
        Some(c) => orders::order_move_to_object(
            b.world,
            slot,
            false,
            true,
            OrderOrigin::Commander,
            Some(c),
            -1,
            -1,
            u32::from(run),
        ),
        None => 0,
    }
}

/// `execOrderPowerDown` (`TacOrderOrigin` is `ORDER_ORIGIN_COMMANDER`, never changed).
pub fn order_power_down(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    orders::order_power_down(b.world, slot, false, OrderOrigin::Commander)
}

/// `execOrderPowerUp`.
pub fn order_power_up(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    orders::order_power_up(b.world, slot, false, OrderOrigin::Commander)
}

fn attack_method(v: i32) -> AttackMethod {
    match v {
        1 => AttackMethod::Dfa,
        2 => AttackMethod::Ramming,
        _ => AttackMethod::Ranged,
    }
}

/// `execOrderAttackObject`.
pub fn order_attack_object(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    attack_type: i32,
    method: i32,
    range: i32,
    pursue: bool,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    if is_unit_id(object_id) {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return 1;
    };
    let mut params = orders::param::NONE;
    if pursue {
        params |= orders::param::PURSUE;
    }
    let object = (object_id > 0)
        .then(|| {
            b.world
                .objects
                .find_by_part_id(crate::ids::PartId(object_id))
        })
        .flatten();
    orders::order_attack_object(
        b.world,
        slot,
        false,
        OrderOrigin::Commander,
        object,
        attack_type,
        attack_method(method),
        range,
        -1,
        -1,
        params,
    )
}

/// `execOrderAttackContact`.
pub fn order_attack_contact(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    attack_type: i32,
    method: i32,
    range: i32,
    pursue: bool,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let Some(slot) = b.warrior_slot() else {
        return -2;
    };
    let mut params = orders::param::NONE;
    if pursue {
        params |= orders::param::PURSUE;
    }
    match b.cur_contact {
        Some(c) => orders::order_attack_object(
            b.world,
            slot,
            false,
            OrderOrigin::Commander,
            Some(c),
            attack_type,
            attack_method(method),
            range,
            -1,
            -1,
            params,
        ),
        None => -2,
    }
}

/// `execOrderWithdraw`: pushes 1, then (unless skipping) pushes 0 and orders the withdrawal; the script sees the last pushed value.
pub fn order_withdraw(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    match b.warrior_slot() {
        Some(slot) => {
            orders::order_withdraw(
                b.world,
                slot,
                false,
                OrderOrigin::Commander,
                Vec3::default(),
            );
            0
        }
        None => -2,
    }
}

/// `execGetFixed` (ablmc2.cpp:4487): sends a mech or vehicle to a repair bay; 0 on success, 1..9 and -1 as the C++ table. The order of
/// the checks is the C++'s. As in the C++, a ground vehicle also needs a `MECHBAY` bay to get past code 2 (and `needsRefit` is false
/// for anything but a mech, so it answers 6).
pub fn get_fixed(b: &mut SimBindings<'_>, fixee_id: i32, bay_id: i32, params: i32) -> i32 {
    use crate::object::{flags, Kind};
    let w = &*b.world;
    let bay = b.object(bay_id).filter(|&h| {
        w.objects
            .get(h)
            .is_some_and(|o| matches!(o.kind, Kind::Building(_)) && o.c.flag(flags::CANREFIT))
    });
    let Some(bay) = bay else {
        return 8;
    };
    if crate::mover::refit::building_refit_points(w, bay) <= 0.0 {
        return 1;
    }
    let Some(fixee) = b.object(fixee_id).filter(|&h| {
        matches!(
            w.objects.get(h).map(|o| &o.kind),
            Some(Kind::Mech(_) | Kind::Vehicle(_))
        )
    }) else {
        return 7;
    };
    if !crate::object::hooks::is_friendly(w, bay, fixee) {
        return 9;
    }
    let bay_buddy = super::support_orders::refit_buddy(w, bay);
    if let Some(buddy) = w.objects.by_watch_id(bay_buddy) {
        return if buddy == fixee { 3 } else { 4 };
    }
    if !w.objects.get(bay).is_some_and(|o| o.c.flag(flags::MECHBAY)) {
        return 2;
    }
    let fixee_buddy = super::support_orders::refit_buddy(w, fixee);
    if w.objects.by_watch_id(fixee_buddy).is_some() {
        return 5;
    }
    if !crate::mover::refit::needs_refit(w, fixee) {
        return 6;
    }
    match b.pilot_of(fixee) {
        Some(slot)
            if orders::order_get_fixed(
                b.world,
                slot,
                OrderOrigin::Commander,
                bay,
                params as u32,
            ) == 0 =>
        {
            0
        }
        _ => -1,
    }
}

/// `execObjectInWithdrawal`.
pub fn object_in_withdrawal(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    // Mover::isWithdrawing: the pilot's current order is a withdrawal
    let withdrawing = |b: &SimBindings<'_>, h: ObjectHandle| {
        b.pilot_of(h)
            .and_then(|s| b.world.pilots.get(s))
            .is_some_and(|p| p.cur_order.code == OrderCode::Withdraw)
    };
    if is_unit_id(object_id) {
        for m in b.movers_of(object_id, false) {
            if !withdrawing(b, m) {
                return 0;
            }
        }
        1
    } else {
        match b.object(object_id) {
            Some(h) if !withdrawing(b, h) => 0,
            _ => 1,
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// core*
// ---------------------------------------------------------------------------------------------------------------------

/// `execCoreMoveTo` (also `newMoveTo`).
pub fn core_move_to(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    location: ArrayRef,
    params: i32,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let x = ctx.get_f32(location, 0);
    let y = ctx.get_f32(location, 1);
    if x.is_nan() || y.is_nan() {
        // STOP(("Move to order location is Not a Number in Brain ...")): a fatal data error of the C++
        b.unavailable(AiUnavailable::NullOrderTarget);
        return 0;
    }
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    orders::core_move_to(b.world, slot, Vec3 { x, y, z: 0.0 }, params as u32)
}

/// `execCoreMoveToObject`.
pub fn core_move_to_object(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    params: i32,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let target = b.object(object_id);
    match b.warrior_slot() {
        Some(slot) => orders::core_move_to_object(b.world, slot, target, params as u32),
        None => 0,
    }
}

/// `execCorePower`.
pub fn core_power(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, up: bool) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    match b.warrior_slot() {
        Some(slot) => orders::core_power(b.world, slot, up),
        None => 0,
    }
}

/// `execCoreAttack`.
pub fn core_attack(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    params: i32,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    let target = b.object(object_id);
    match b.warrior_slot() {
        Some(slot) => orders::core_attack(b.world, slot, target, params as u32),
        None => 0,
    }
}

/// `execCoreCapture` / `execCoreScan` / `execCoreControl` share the `objectID == 0 -> NULL` convention.
fn object_or_null(b: &SimBindings<'_>, object_id: i32) -> Result<Option<ObjectHandle>, ()> {
    let obj = b.object(object_id);
    if object_id == 0 {
        Ok(None)
    } else if obj.is_some() {
        Ok(obj)
    } else {
        Err(())
    }
}

/// `execCoreCapture`.
pub fn core_capture(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    params: i32,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    match (b.warrior_slot(), object_or_null(b, object_id)) {
        (Some(slot), Ok(obj)) => orders::core_capture(b.world, slot, obj, params as u32),
        _ => 0,
    }
}

/// `execCoreScan`.
pub fn core_scan(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    params: i32,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    match (b.warrior_slot(), object_or_null(b, object_id)) {
        (Some(slot), Ok(obj)) => orders::core_scan(b.world, slot, obj, params as u32),
        _ => 0,
    }
}

/// `execCoreControl`.
pub fn core_control(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    params: i32,
) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    match (b.warrior_slot(), object_or_null(b, object_id)) {
        (Some(slot), Ok(obj)) => orders::core_control(b.world, slot, obj, params as u32),
        _ => 0,
    }
}

/// `execCoreEject`.
pub fn core_eject(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>) -> i32 {
    if ctx.skip_order() {
        return 1;
    }
    match b.warrior_slot() {
        Some(slot) => orders::core_eject(b.world, slot),
        None => 0,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Pilot state
// ---------------------------------------------------------------------------------------------------------------------

/// `execSetPilotState`.
pub fn set_pilot_state(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, new_state: i32) -> i32 {
    let cur = ctx.current_state_handle();
    if cur > 0 {
        ctx.trans_state(new_state);
        cur
    } else {
        ctx.reset_orders();
        match b.warrior_slot().and_then(|s| b.world.pilots.get_mut(s)) {
            Some(p) => std::mem::replace(&mut p.brain_state, new_state),
            None => 0,
        }
    }
}

/// `execGetPilotState`.
pub fn get_pilot_state(b: &mut SimBindings<'_>) -> i32 {
    b.warrior_slot()
        .and_then(|s| b.world.pilots.get(s))
        .map_or(0, |p| p.brain_state)
}

/// `execGetNextPilotEvent`.
pub fn get_next_pilot_event(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
) -> i32 {
    let Some(slot) = b.warrior_slot() else {
        return -1;
    };
    let (event, params) = warrior::get_next_event_history(b.world, slot);
    for (i, p) in params.iter().enumerate() {
        ctx.set_i32(list, i, *p);
    }
    event
}

/// `execSetTargetPriority`.
pub fn set_target_priority(
    b: &mut SimBindings<'_>,
    index: i32,
    kind: i32,
    p1: i32,
    p2: i32,
    p3: i32,
) -> i32 {
    let Some(p) = b.warrior_slot().and_then(|s| b.world.pilots.get_mut(s)) else {
        return 0;
    };
    let Some(entry) = usize::try_from(index)
        .ok()
        .filter(|&i| i < super::pilot::MAX_TARGET_PRIORITIES)
    else {
        return 0;
    };
    p.target_priorities.list[entry] = super::pilot::TargetPriority {
        kind,
        params: [p1, p2, p3],
    };
    if index >= p.target_priorities.size {
        p.target_priorities.size = index + 1;
    }
    0
}

/// `execGetWeapons`.
pub fn get_weapons(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    list: ArrayRef,
    info_type: i32,
) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    if !b.world.objects.get(me).is_some_and(|o| o.is_mover()) {
        return 0;
    }
    let Some(l) = b.world.objects.get(me).and_then(crate::combat::loadout_of) else {
        return 0;
    };
    let (num_other, num_weapons) = (l.num_other(), l.num_weapons());
    let mut n = 0usize;
    for cur in num_other..num_other + num_weapons {
        let item = &l.inventory[cur];
        let include = match info_type {
            0 => true,
            1 => !item.disabled && mover_ops::weapon_shots(b.world, me, cur as i32) > 0,
            _ => false,
        };
        if include {
            ctx.set_i32(list, n, i32::from(item.master_id));
            n += 1;
        }
    }
    n as i32
}

/// `execGetWeaponsStatus`: `MechWarrior::getWeaponsStatus(list)` copies `weaponsStatus` for the mover's weapons and returns the result.
pub fn get_weapons_status(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, list: ArrayRef) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    if !b.world.objects.get(me).is_some_and(|o| o.is_mover()) {
        return 0;
    }
    let Some(slot) = b.pilot_of(me) else { return 0 };
    let n = b
        .world
        .objects
        .get(me)
        .and_then(crate::combat::loadout_of)
        .map_or(0, |l| l.num_weapons());
    let p = b.world.pilots.get(slot).expect("pilot");
    for i in 0..n {
        ctx.set_i32(list, i, p.weapons_status.get(i).copied().unwrap_or(0));
    }
    p.weapons_status_result
}

/// `execSetMoveArea`.
pub fn set_move_area(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    center: ArrayRef,
    radius: f32,
) {
    let Some(me) = b.cur_object_handle() else {
        return;
    };
    let mut loc = Vec3 {
        x: ctx.get_f32(center, 0),
        y: ctx.get_f32(center, 1),
        z: 0.0,
    };
    loc.z = b.world.terrain.elevation(&loc);
    if let Some(core) = b.world.objects.get_mut(me).and_then(|o| match &mut o.kind {
        crate::object::Kind::Mech(m) => Some(&mut m.m),
        crate::object::Kind::Vehicle(v) => Some(&mut v.m),
        _ => None,
    }) {
        core.move_center = loc;
        core.move_radius = radius;
    }
}

/// `execClearTacOrder`: `CurObject->getPilot()->clearCurTacOrder()` (with the order-slot update).
pub fn clear_tac_order(b: &mut SimBindings<'_>) {
    if let Some(slot) = b.cur_object_handle().and_then(|h| b.pilot_of(h)) {
        warrior::clear_cur_tac_order(b.world, slot, true);
    }
}

/// `execSetWillHelp`.
pub fn set_will_help(b: &mut SimBindings<'_>, will_help: bool) -> bool {
    match b.warrior_slot().and_then(|s| b.world.pilots.get_mut(s)) {
        Some(p) => std::mem::replace(&mut p.will_help, will_help),
        None => false,
    }
}

/// `execGetLastScan`.
pub fn get_last_scan(b: &mut SimBindings<'_>) -> i32 {
    let wid = b
        .warrior_slot()
        .and_then(|s| b.world.pilots.get(s))
        .map_or(WatchId(0), |p| p.core_scan_target);
    b.world
        .objects
        .by_watch_id(wid)
        .map_or(0, |h| b.part_id_of(h))
}

/// `execGetMapInfo`: `GameMap->getHeight()`, `getWidth()` (cells).
pub fn get_map_info(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, info: ArrayRef) {
    let side = b.world.terrain.cells_per_side();
    ctx.set_i32(info, 0, side);
    ctx.set_i32(info, 1, side);
}

/// `execIsOffMap`.
pub fn is_off_map(b: &mut SimBindings<'_>, ctx: &mut NativeCtx<'_>, world_pos: ArrayRef) -> bool {
    let mut pos = Vec3 {
        x: ctx.get_f32(world_pos, 0),
        y: ctx.get_f32(world_pos, 1),
        z: 0.0,
    };
    if pos.x > 999_000.0 {
        match b.cur_object_handle().and_then(|h| b.world.objects.get(h)) {
            Some(o) => pos = o.c.position,
            None => return false,
        }
    }
    let (row, col) = b.world.terrain.world_to_cell(&pos);
    b.world.terrain.map().in_bounds(row, col) && b.world.terrain.map().cell(row, col).off_map()
}

/// `execSetGoalPlanning`.
pub fn set_goal_planning(b: &mut SimBindings<'_>, setting: bool) -> bool {
    let Some(slot) = b.cur_object_handle().and_then(|h| b.pilot_of(h)) else {
        return false;
    };
    let old = std::mem::replace(
        &mut b.world.pilots.get_mut(slot).expect("pilot").use_goal_plan,
        setting,
    );
    if old != setting {
        warrior::set_main_goal(
            b.world,
            slot,
            super::pilot::goal_action::NONE,
            None,
            None,
            -1.0,
        );
    }
    old
}

/// `execSetEject`.
pub fn set_eject(b: &mut SimBindings<'_>, setting: bool) -> bool {
    match b
        .cur_object_handle()
        .and_then(|h| b.pilot_of(h))
        .and_then(|s| b.world.pilots.get_mut(s))
    {
        Some(p) => std::mem::replace(&mut p.escapes_thru_ejection, setting),
        None => false,
    }
}

/// `execSetKeepMoving`.
pub fn set_keep_moving(b: &mut SimBindings<'_>, setting: bool) -> bool {
    match b
        .cur_object_handle()
        .and_then(|h| b.pilot_of(h))
        .and_then(|s| b.world.pilots.get_mut(s))
    {
        Some(p) => std::mem::replace(&mut p.keep_moving, setting),
        None => false,
    }
}

/// `execSetAttackRadius`: returns the previous radius.
pub fn set_attack_radius(b: &mut SimBindings<'_>, radius: f32) -> f32 {
    match b.warrior_slot().and_then(|s| b.world.pilots.get_mut(s)) {
        Some(p) => std::mem::replace(&mut p.attack_radius, radius),
        None => 0.0,
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// requestHelp / requestTarget / requestShelter
// ---------------------------------------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
/// `execRequestHelp` (ablmc2.cpp:5996): the friendly movers near the requester's centre are told which enemy to attack
/// (`PILOT_ALARM_ATTACK_ORDER` with the enemy's watch id), spreading the threat (`calcAttackPlan`).
pub fn request_help(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    enemy_part_id: i32,
    friendly_center: ArrayRef,
    friendly_radius: f32,
    enemy_center: ArrayRef,
    enemy_radius: f32,
    priority: i32,
) -> f32 {
    let Some(slot) = b.warrior_slot() else {
        return 0.0;
    };
    let my_team = warrior::team_id(b.world, slot);
    if !crate::object::hooks::team_exists(b.world, my_team) {
        return 0.0;
    }
    let friendly_pos = Vec3 {
        x: ctx.get_f32(friendly_center, 0),
        y: ctx.get_f32(friendly_center, 1),
        z: 0.0,
    };
    let enemy_pos = Vec3 {
        x: ctx.get_f32(enemy_center, 0),
        y: ctx.get_f32(enemy_center, 1),
        z: 0.0,
    };
    let commander_id = if my_team == b.world.teams.home as i32 {
        warrior::core(b.world, slot).map_or(-1, |c| i32::from(c.commander.0))
    } else {
        -1
    };
    let friendlies = movers_within_radius(
        b.world,
        friendly_pos,
        friendly_radius,
        my_team,
        commander_id,
        false,
        priority > 0,
    );
    let Some(enemy) = b
        .world
        .objects
        .find_by_part_id(crate::ids::PartId(enemy_part_id))
    else {
        return 0.0;
    };
    let enemy_team = b.world.objects.get(enemy).map_or(-1, |o| o.team_id());
    if !crate::object::hooks::team_exists(b.world, enemy_team) || enemy_team == my_team {
        return 0.0;
    }
    let enemies = movers_within_radius(
        b.world,
        enemy_pos,
        enemy_radius,
        enemy_team,
        -1,
        false,
        true,
    );
    calc_attack_plan(b.world, &friendlies, &enemies);
    0.0
}

/// `getMoversWithinRadius` (ablmc2.cpp:139) for the non-enemy branch (`getEnemies == false`) and `sortDescending == true`, plus the
/// enemy branch when `team` is passed with `get_enemies`. Sorted by threat rating, descending, stable.
fn movers_within_radius(
    w: &crate::World,
    center: Vec3,
    radius: f32,
    team: i32,
    commander_id: i32,
    get_enemies: bool,
    ignore_order: bool,
) -> Vec<ObjectHandle> {
    let mut found: Vec<(ObjectHandle, i16)> = Vec::new();
    let order_free = |slot: u16| {
        ignore_order
            || w.pilots
                .get(slot)
                .is_some_and(|p| p.cur_order.code == OrderCode::None)
    };
    let functional = |o: &crate::object::Obj| {
        crate::combat::loadout_of(o).map_or(0, |l| l.stats.num_functional_weapons)
    };
    if get_enemies {
        for &h in &w.objects.movers {
            let Some(o) = w.objects.get(h) else { continue };
            let enemy = crate::object::hooks::team_relation(w, o.team_id(), team)
                == Some(super::Relation::Enemy)
                && crate::object::hooks::team_exists(w, o.team_id());
            if o.exists()
                && !o.c.is_disabled()
                && enemy
                && functional(o) > 0
                && f64::from(mover_distance(o, &center)) < f64::from(radius)
            {
                if let Some(slot) = crate::combat::fx::pilot_slot(w, h) {
                    if order_free(slot) {
                        found.push((h, o.c.threat_rating));
                    }
                }
            }
        }
    } else if let Some(Some(t)) = w.teams.teams.get(team.max(0) as usize) {
        for &wid in &t.roster {
            let Some(h) = w.objects.by_watch_id(wid) else {
                continue;
            };
            let Some(o) = w.objects.get(h) else { continue };
            let commander_ok = commander_id == -1
                || i32::from(crate::combat::core_of(o).map_or(-1, |c| c.commander.0))
                    == commander_id;
            if commander_ok
                && o.exists()
                && !o.c.is_disabled()
                && functional(o) > 0
                && f64::from(mover_distance(o, &center)) < f64::from(radius)
            {
                if let Some(slot) = crate::combat::fx::pilot_slot(w, h) {
                    if w.pilots.get(slot).is_some_and(|p| p.will_help) && order_free(slot) {
                        found.push((h, o.c.threat_rating));
                    }
                }
            }
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.1));
    found.into_iter().map(|(h, _)| h).collect()
}

fn mover_distance(o: &crate::object::Obj, center: &Vec3) -> f32 {
    crate::combat::attack::distance_from(&o.c.position, center)
}

/// `calcAttackPlan` (ablmc2.cpp:228): spreads the attackers over the defenders by threat and tells each attacker its target with a
/// `PILOT_ALARM_ATTACK_ORDER` trigger.
pub(crate) fn calc_attack_plan(
    w: &mut crate::World,
    attackers: &[ObjectHandle],
    defenders: &[ObjectHandle],
) {
    if defenders.is_empty() || attackers.is_empty() {
        return;
    }
    let threat = |w: &crate::World, h: ObjectHandle| {
        i64::from(w.objects.get(h).map_or(0, |o| o.c.threat_rating))
    };
    let n = defenders.len();
    let mut ratio = vec![0.0f32; n];
    let mut total = vec![0i64; n];
    let mut target: Vec<Option<ObjectHandle>> = vec![None; attackers.len()];
    for (a, &attacker) in attackers.iter().enumerate() {
        let mut toughest = n - 1;
        for d in (0..n.saturating_sub(1)).rev() {
            if ratio[d] < ratio[toughest] {
                toughest = d;
            } else if ratio[d] == ratio[toughest] && total[d] > total[toughest] {
                toughest = d;
            }
        }
        total[toughest] += threat(w, attacker);
        // C++ divides long by long before the float assignment (ablmc2.cpp:242); a zero rating is a C++ crash, here 0
        ratio[toughest] = total[toughest]
            .checked_div(threat(w, defenders[toughest]))
            .unwrap_or(0) as f32;
        target[a] = Some(defenders[toughest]);
    }
    for (a, &attacker) in attackers.iter().enumerate() {
        if let (Some(t), Some(slot)) = (target[a], crate::combat::fx::pilot_slot(w, attacker)) {
            let wid = w.objects.watch_id(t);
            warrior::trigger_alarm(w, slot, warrior::alarm::ATTACK_ORDER, wid.0 as i32);
        }
    }
}

/// `execRequestTarget`.
pub fn request_target(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    center: ArrayRef,
    radius: f32,
) -> i32 {
    let Some(slot) = b.warrior_slot() else {
        return 0;
    };
    let my_team = warrior::team_id(b.world, slot);
    if !crate::object::hooks::team_exists(b.world, my_team) {
        return 0;
    }
    let pos = Vec3 {
        x: ctx.get_f32(center, 0),
        y: ctx.get_f32(center, 1),
        z: 0.0,
    };
    let commander_id = if my_team == b.world.teams.home as i32 {
        warrior::core(b.world, slot).map_or(-1, |c| i32::from(c.commander.0))
    } else {
        -1
    };
    let friendlies = movers_within_radius(b.world, pos, radius, my_team, commander_id, false, true);
    let enemies = movers_within_radius(b.world, pos, radius, my_team, -1, true, true);
    if enemies.is_empty() {
        return 0;
    }
    let Some(me) = vehicle(b.world, slot) else {
        return 0;
    };
    // calcBestTarget (ablmc2.cpp:283)
    let threat = |w: &crate::World, h: ObjectHandle| {
        i64::from(w.objects.get(h).map_or(0, |o| o.c.threat_rating))
    };
    let n = enemies.len();
    let mut total = vec![0i64; n];
    for &attacker in &friendlies {
        let t = crate::combat::fx::pilot_slot(b.world, attacker)
            .and_then(|s| warrior::get_current_target(b.world, s));
        for (d, &def) in enemies.iter().enumerate() {
            if Some(def) == t {
                total[d] += threat(b.world, attacker);
                break;
            }
        }
    }
    let ratio: Vec<f32> = (0..n)
        .map(|d| {
            // long / long before the float assignment (ablmc2.cpp:286)
            total[d]
                .checked_div(threat(b.world, enemies[d]))
                .unwrap_or(0) as f32
        })
        .collect();
    let mut best = n - 1;
    if n > 1 {
        for d in (0..n - 1).rev() {
            if ratio[d] < ratio[best] {
                best = d;
            } else if ratio[d] == ratio[best] && total[d] > total[best] {
                best = d;
            }
        }
    }
    let _ = me;
    b.part_id_of(enemies[best])
}

/// `execRequestShelter`: one of the first ten buildings within `range` of `CurObject`, chosen with `RandomNumber(n)`.
pub fn request_shelter(b: &mut SimBindings<'_>, range: f32) -> i32 {
    let Some(me) = b.cur_object_handle() else {
        return 0;
    };
    let mut valid: Vec<ObjectHandle> = Vec::new();
    for &bld in &b.world.objects.buildings {
        let Some(o) = b.world.objects.get(bld) else {
            continue;
        };
        if mover_ops::distance_from(b.world, me, &o.c.position) < range {
            valid.push(bld);
            if valid.len() == 10 {
                break;
            }
        }
    }
    if valid.is_empty() {
        return 0;
    }
    let i = b
        .world
        .rng
        .0
        .borrow_mut()
        .sim
        .random_number(valid.len() as i32) as usize;
    b.part_id_of(valid[i])
}

// ---------------------------------------------------------------------------------------------------------------------
// Object queries scripts use
// ---------------------------------------------------------------------------------------------------------------------

/// `execObjectExists`.
pub fn object_exists(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    if is_unit_id(object_id) {
        let movers = b.movers_of(object_id, false);
        i32::from(
            movers
                .iter()
                .any(|&m| b.world.objects.get(m).is_some_and(|o| o.exists())),
        )
    } else {
        i32::from(b.object_exists_only(object_id).is_some())
    }
}

/// `execObjectStatus`.
pub fn object_status(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    use crate::object::ObjectStatus as S;
    if is_unit_id(object_id) {
        let mut disabled = true;
        for m in b.movers_of(object_id, false) {
            let Some(o) = b.world.objects.get(m) else {
                continue;
            };
            if o.c.status != S::Destroyed && o.c.status != S::Disabled {
                let pilot_status = b
                    .pilot_of(m)
                    .and_then(|s| b.world.pilots.get(s))
                    .map(|p| p.status);
                if pilot_status.is_some_and(|s| s != crate::combat::fx::warrior_status::WITHDRAWN) {
                    disabled = false;
                    break;
                }
            }
        }
        if disabled {
            S::Disabled as i32
        } else {
            S::Normal as i32
        }
    } else {
        b.object(object_id)
            .and_then(|h| b.world.objects.get(h))
            .map_or(-1, |o| o.c.status as i32)
    }
}

/// `execObjectTeam`: `MIN_TEAM_PART_ID + object->getTeamId()`.
pub fn object_team(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    b.object(object_id)
        .and_then(|h| b.world.objects.get(h))
        .map_or(-1, |o| 500 + o.team_id())
}

/// `execObjectClass`.
pub fn object_class(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    b.object(object_id)
        .and_then(|h| b.world.objects.get(h))
        .map_or(-1, |o| o.c.class as i32)
}

/// `execDistanceToObject`: -1 when either object is missing; for a unit id the nearest awake mover.
pub fn distance_to_object(b: &mut SimBindings<'_>, object_id1: i32, object_id2: i32) -> f32 {
    let Some(o2) = b.object(object_id2) else {
        return -1.0;
    };
    let p2 = b
        .world
        .objects
        .get(o2)
        .map(|o| o.c.position)
        .unwrap_or_default();
    planar_distance_result(b, object_id1, p2)
}

/// `execDistanceToPosition`.
pub fn distance_to_position(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    coords: ArrayRef,
) -> f32 {
    let mut p2 = Vec3::default();
    let x = ctx.get_f32(coords, 0);
    let y = ctx.get_f32(coords, 1);
    if !x.is_nan() {
        p2.x = x;
    }
    if !y.is_nan() {
        p2.y = y;
    }
    planar_distance_result(b, object_id, p2)
}

/// The shared tail of `execDistanceToObject` / `execDistanceToPosition`: `position2.z = position1.z`, so the distance is planar, in
/// meters.
fn planar_distance_result(b: &mut SimBindings<'_>, object_id1: i32, mut p2: Vec3) -> f32 {
    if is_unit_id(object_id1) {
        let mut min = 3.4e38f32;
        for m in b.movers_of(object_id1, false) {
            let Some(o) = b.world.objects.get(m) else {
                continue;
            };
            // getExistsAndAwake
            if !(o.exists() && o.c.awake()) {
                continue;
            }
            let p1 = o.c.position;
            p2.z = p1.z;
            let d = crate::math::approximate_length(&Vec3 {
                x: p2.x - p1.x,
                y: p2.y - p1.y,
                z: p2.z - p1.z,
            });
            if d < min {
                min = d;
            }
        }
        if f64::from(min) < 3.4e38 {
            return min * crate::terrain::METERS_PER_WORLD_UNIT;
        }
        -1.0
    } else if let Some(o1) = b.object(object_id1).and_then(|h| b.world.objects.get(h)) {
        let p1 = o1.c.position;
        p2.z = p1.z;
        crate::math::approximate_length(&Vec3 {
            x: p2.x - p1.x,
            y: p2.y - p1.y,
            z: p2.z - p1.z,
        }) * crate::terrain::METERS_PER_WORLD_UNIT
    } else {
        -1.0
    }
}

/// `execObjectTypeID`: `getObjectType()->whatAmI()` (the type number).
pub fn object_type_id(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    b.object(object_id)
        .and_then(|h| b.world.objects.get(h))
        .map_or(-1, |o| o.c.type_handle)
}

/// `execObjectStatusCount`: `tallyList[status]++` for one object; for a team `Team::statusCount` (warrior.cpp-like tally with 6 =
/// withdrawn, 7 = asleep, 8 = gone). Commander groups use the loaded `MoverGroup` roster.
pub fn object_status_count(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    object_id: i32,
    tally: ArrayRef,
) {
    let add = |ctx: &mut NativeCtx<'_>, i: usize| {
        let v = ctx.get_i32(tally, i);
        ctx.set_i32(tally, i, v + 1);
    };
    if is_unit_id(object_id) {
        if (OBJ_ID_FIRST_TEAM..=OBJ_ID_LAST_TEAM).contains(&object_id) {
            for m in b.movers_of(object_id, false) {
                let Some(o) = b.world.objects.get(m) else {
                    continue;
                };
                let withdrawn = b
                    .pilot_of(m)
                    .and_then(|s| b.world.pilots.get(s))
                    .is_some_and(|p| p.status == crate::combat::fx::warrior_status::WITHDRAWN);
                let index = if !o.exists() {
                    8
                } else if !o.c.awake() {
                    7
                } else if withdrawn {
                    6
                } else {
                    o.c.status as usize
                };
                add(ctx, index);
            }
        } else {
            // `Commander::commanders[c]->getGroup(g)->statusCount` (ablmc2.cpp:2304-2342).
            let (commander, group) = if (16..32).contains(&object_id) {
                (0, object_id - 16)
            } else if (165..181).contains(&object_id) {
                (1, object_id - 165)
            } else if (329..345).contains(&object_id) {
                (2, object_id - 329)
            } else {
                return;
            };
            let members: Vec<_> = b
                .world
                .mission
                .groups
                .iter()
                .filter(|m| m.commander == commander && m.group == group as usize)
                .filter_map(|m| {
                    b.world
                        .objects
                        .find_by_part_id(crate::ids::PartId(m.part as i32))
                })
                .collect();
            for h in members {
                let Some(o) = b.world.objects.get(h) else {
                    continue;
                };
                let withdrawn = b
                    .pilot_of(h)
                    .and_then(|s| b.world.pilots.get(s))
                    .is_some_and(|p| p.status == crate::combat::fx::warrior_status::WITHDRAWN);
                let index = if !o.exists() {
                    8
                } else if !o.c.awake() {
                    7
                } else if withdrawn {
                    6
                } else {
                    o.c.status as usize
                };
                add(ctx, index);
            }
        }
    } else if let Some(o) = b.object(object_id).and_then(|h| b.world.objects.get(h)) {
        add(ctx, o.c.status as usize);
    }
}

/// `execGetObjectDamage`: the percentage of a terrain object's damage level (100 when the level is 0), 0 for everything else.
pub fn get_object_damage(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    let Some(h) = b.object(object_id) else {
        return 0;
    };
    let Some(o) = b.world.objects.get(h) else {
        return 0;
    };
    let Some(s) = o.structure() else { return 0 };
    let level = crate::object::structure::damage_level(b.world, h);
    let mut result = 100.0f64;
    if level > 0.0 {
        result = (f64::from(s.damage / level) * 100.0).floor();
    }
    result as i32
}

/// `execGetObjectActive`.
pub fn get_object_active(b: &mut SimBindings<'_>, object_id: i32) -> i32 {
    if is_unit_id(object_id) {
        b.movers_of(object_id, false)
            .iter()
            .filter(|&&m| b.world.objects.get(m).is_some_and(|o| o.c.awake()))
            .count() as i32
    } else {
        i32::from(
            b.object(object_id)
                .and_then(|h| b.world.objects.get(h))
                .is_some_and(|o| o.c.awake()),
        )
    }
}

/// `execGetCurrentBRValue`: `getCurCV()`.
pub fn get_current_br_value(b: &mut SimBindings<'_>, object_num: i32) -> i32 {
    b.object(object_num)
        .and_then(|h| b.world.objects.get(h))
        .map_or(-1, |o| i32::from(o.c.cur_cv))
}

/// `execGetTerrainObjectPartID`.
pub fn get_terrain_object_part_id(row: i32, col: i32) -> i32 {
    crate::object::MIN_TERRAIN_PART_ID + row * crate::object::MAX_MAP_CELL_WIDTH + col
}

/// Marker so `SORT_DISTANCE` stays referenced for the contact sort constants this module documents.
pub const CONTACT_SORT_DISTANCE: i32 = SORT_DISTANCE;

/// The `NATIVES` indices [`SimBindings`] implements (lane D's here, lane E's mission natives in `mission/bindings.rs`, plus the
/// debug / GUI natives that have no simulation effect in the C++). A script that reaches any other native gets the trait default
/// (zero / false) and the call is reported [`AiUnavailable::Native`].
pub static IMPLEMENTED: std::sync::LazyLock<Vec<u16>> = std::sync::LazyLock::new(|| {
    const METHODS: &[&str] = &[
        "call_strike",
        "call_strike_ex",
        "get_salvage",
        "path_exists",
        "set_salvage",
        "set_revealed",
        // lane D
        "get_id",
        "select_object",
        "select_warrior",
        "get_warrior_status",
        "get_contacts",
        "get_enemy_count",
        "select_contact",
        "get_contact_id",
        "is_contact",
        "get_contact_status",
        "get_contact_relative_position",
        "set_target",
        "get_target",
        "get_weapons_ready",
        "get_weapons_locked",
        "get_weapons_in_range",
        "get_weapon_shots",
        "get_weapon_ranges",
        "get_object_position",
        "get_integer_memory",
        "get_real_memory",
        "get_alarm_triggers",
        "get_challenger",
        "get_time_without_orders",
        "get_fire_ranges",
        "get_attackers",
        "get_attacker_info",
        "set_challenger",
        "set_integer_memory",
        "set_real_memory",
        "has_move_goal",
        "has_move_path",
        "sort_weapons",
        "get_visual_range",
        "get_unit_mates",
        "get_tac_order",
        "get_last_tac_order",
        "get_objects",
        "order_wait",
        "order_move_to",
        "order_move_to_object",
        "order_move_to_contact",
        "order_power_down",
        "order_power_up",
        "order_attack_object",
        "order_attack_contact",
        "order_withdraw",
        "get_fixed",
        // `orderdeployelementals`: the whole C++ body is under `#ifdef USE_ELEMENTALS`, never defined in the shipped game, so it is a
        // silent no-op (the argument is popped by the dispatcher).
        "order_deploy_elementals",
        "object_in_withdrawal",
        "set_attack_radius",
        "distance_to_object",
        "distance_to_position",
        "object_exists",
        "object_status",
        "object_team",
        "object_class",
        "get_terrain_object_part_id",
        "get_current_br_value",
        "get_object_active",
        "object_type_id",
        "object_status_count",
        "get_object_damage",
        "core_move_to",
        "core_move_to_object",
        "core_power",
        "core_attack",
        "core_capture",
        "set_captured",
        "set_capturable",
        "is_captured",
        "is_capturable",
        "is_team_targeting",
        "is_team_capturing",
        "core_scan",
        "core_control",
        "core_eject",
        "set_pilot_state",
        "get_pilot_state",
        "get_next_pilot_event",
        "set_target_priority",
        "set_debug_window",
        "set_debug_string",
        "mc_print",
        "request_help",
        "request_target",
        "request_shelter",
        "get_weapons",
        "set_move_area",
        "get_weapons_status",
        "clear_tac_order",
        "set_will_help",
        "get_last_scan",
        "get_map_info",
        "is_off_map",
        "set_goal_planning",
        "set_eject",
        "set_keep_moving",
        // Pilot brains also call these output-only presentation / mission-state
        // bindings. Presentation queries and movie playback remain explicitly
        // unavailable until the application returns authoritative live state.
        "play_digital_music",
        "get_sensors_active",
        "play_wave",
        "get_logistics_time",
        "stop_voice_over",
        "set_invulnerable",
        // lane E (mission natives, mission/bindings.rs)
        "get_time",
        "get_time_left",
        "set_timer",
        "check_timer",
        "end_timer",
        "set_objective_status",
        "check_objective_status",
        "get_global_value",
        "set_global_value",
        "set_objective_pos",
        "send_message",
        "get_message",
        "get_mission_won",
        "get_mission_lost",
        "get_objective_success",
        "get_objective_failed",
        "get_enemy_destroyed",
        "get_friendly_destroyed",
        "get_mission_status",
        "add_trigger_area",
        "is_trigger_area_hit",
        "reset_trigger_area",
        "remove_trigger_area",
        // Presentation (mission/natives_presentation.rs): state and events are real bindings.
        "play_video",
        "set_movie_mode",
        "end_movie_mode",
        "fade_to_color",
        "force_movie_end",
        "animation_callout",
        "tutorial_text",
        "gui_is_aoe_style",
        "freeze_gui",
        // The camera API is implemented by mission/bindings.rs. Keep all 23 entries here as well so
        // mission scripts are not reported as unavailable when the camera natives are exercised.
        "set_camera_position",
        "set_camera_goal_position",
        "set_camera_rotation",
        "set_camera_goal_rotation",
        "set_camera_zoom",
        "set_camera_goal_zoom",
        "set_camera_velocity",
        "set_camera_goal_velocity",
        "set_camera_look_object",
        "get_camera_position",
        "get_camera_goal_position",
        "get_camera_rotation",
        "get_camera_goal_rotation",
        "get_camera_zoom",
        "get_camera_goal_zoom",
        "get_camera_velocity",
        "get_camera_goal_velocity",
        "get_camera_look_object",
        "get_camera_frame_length",
        "get_logistics_time",
        "get_sensors_active",
        "get_general_alarm",
        "set_general_alarm",
        "set_invulnerable",
        "play_wave",
        "is_team_capturing",
        "is_team_targeting",
        "get_sensors_working",
        "set_tonnage",
        "set_explosion_damage",
        "set_explosion_radius",
        "get_object_dmg_pts",
        "get_object_max_dmg",
        "get_pilot_id",
        "get_pilot_wounds",
        "set_pilot_wounds",
        "get_armor_pts",
        "get_max_armor",
        "set_current_br_value",
        "set_object_active",
        "object_change_sides",
        "object_suicide",
        "object_create",
        "object_remove",
        "object_commander",
        "object_visible",
        "in_area",
        "create_infantry",
        "set_sensor_range",
        "set_object_damage",
        "get_home_team",
        "convert_coords",
        "get_relative_position_to_point",
        "get_relative_position_to_object",
        "get_unit_status",
        "lock_gate_open",
        "lock_gate_closed",
        "release_gate_lock",
        "is_gate_open",
        "set_animation",
        "set_building_name",
        "add_prisoner",
        // lane audio (mission/natives_audio.rs): sound system requests/readback and combat-music query.
        "play_digital_music",
        "stop_music",
        "play_sound_effect",
        "set_radio",
        "play_speech",
        "play_betty",
        "get_current_music_id",
        "get_mission_tune_id",
        "is_playing_voice_over",
        "stop_voice_over",
        "in_callout",
        "player_in_combat",
    ];
    mc2_abl::natives::NATIVES
        .iter()
        .enumerate()
        .filter(|(_, d)| METHODS.contains(&d.method))
        .map(|(i, _)| i as u16)
        .collect()
});

#[cfg(test)]
mod implemented_tests {
    use super::IMPLEMENTED;
    use mc2_abl::natives::NATIVES;

    // These methods have concrete forwarding implementations in mission/bindings.rs.
    const MISSION_METHODS: &[&str] = &[
        "set_camera_position",
        "set_camera_goal_position",
        "set_camera_rotation",
        "set_camera_goal_rotation",
        "set_camera_zoom",
        "set_camera_goal_zoom",
        "set_camera_velocity",
        "set_camera_goal_velocity",
        "set_camera_look_object",
        "get_camera_position",
        "get_camera_goal_position",
        "get_camera_rotation",
        "get_camera_goal_rotation",
        "get_camera_zoom",
        "get_camera_goal_zoom",
        "get_camera_velocity",
        "get_camera_goal_velocity",
        "get_camera_look_object",
        "get_camera_frame_length",
        "get_logistics_time",
        "get_sensors_active",
        "get_general_alarm",
        "set_general_alarm",
        "set_invulnerable",
        "play_wave",
        "is_team_capturing",
        "is_team_targeting",
        "set_animation",
        "set_building_name",
        "add_prisoner",
        "object_change_sides",
        "object_suicide",
        "object_create",
        "object_remove",
        "object_commander",
        "object_visible",
        "in_area",
        "create_infantry",
        "get_sensors_working",
        "set_sensor_range",
        "set_tonnage",
        "set_explosion_damage",
        "set_explosion_radius",
        "get_object_dmg_pts",
        "get_object_max_dmg",
        "get_pilot_id",
        "get_pilot_wounds",
        "set_pilot_wounds",
        "get_armor_pts",
        "get_max_armor",
        "get_current_br_value",
        "set_current_br_value",
        "set_object_active",
        "set_object_damage",
        "get_home_team",
        "convert_coords",
        "get_relative_position_to_point",
        "get_relative_position_to_object",
        "get_unit_status",
        "lock_gate_open",
        "lock_gate_closed",
        "release_gate_lock",
        "is_gate_open",
        "play_digital_music",
        "stop_music",
        "play_sound_effect",
        "set_radio",
        "play_speech",
        "play_betty",
        "get_current_music_id",
        "get_mission_tune_id",
        "is_playing_voice_over",
        "stop_voice_over",
        "in_callout",
        "player_in_combat",
    ];

    #[test]
    fn every_mission_native_id_is_bound_and_implemented_once() {
        let expected: Vec<u16> = NATIVES
            .iter()
            .enumerate()
            .filter(|(_, native)| MISSION_METHODS.contains(&native.method))
            .map(|(id, _)| id as u16)
            .collect();
        let actual: Vec<u16> = IMPLEMENTED
            .iter()
            .copied()
            .filter(|id| {
                NATIVES
                    .get(usize::from(*id))
                    .is_some_and(|n| MISSION_METHODS.contains(&n.method))
            })
            .collect();
        assert_eq!(
            actual, expected,
            "each mission native id must be listed exactly once"
        );
        assert_eq!(
            expected.len(),
            MISSION_METHODS.len(),
            "every expected mission binding must be checked"
        );
        assert!(
            IMPLEMENTED.windows(2).all(|w| w[0] < w[1]),
            "IMPLEMENTED ids must be unique and ordered"
        );
    }
}
