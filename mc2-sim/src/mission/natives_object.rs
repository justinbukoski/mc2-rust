// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! ABL object and mission native bodies (gameplay references are cited per function).
//!
//! The ABL dispatch stays in `bindings.rs`; keeping state changes here keeps this slice out of
//! the lane D native implementation and makes the object rules independently testable.

use crate::ids::{ObjectHandle, PartId};
use crate::mission::bindings::SimBindings;
use crate::object::Kind;
use mc2_abl::NativeCtx;

fn object(b: &SimBindings<'_>, id: i32) -> Option<ObjectHandle> {
    if id == -1 {
        b.world.objects.by_watch_id(b.cur_object)
    } else {
        b.world.objects.find_by_part_id(PartId(id))
    }
}

/// `objectCreate` must also find an object whose exists flag was cleared by `objectSuicide`.
fn object_including_removed(b: &SimBindings<'_>, id: i32) -> Option<ObjectHandle> {
    if id == -1 {
        b.world.objects.by_watch_id(b.cur_object)
    } else {
        b.world
            .objects
            .list
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(index, entry)| {
                let obj = entry.as_ref()?;
                (obj.c.part_id == PartId(id)).then_some(ObjectHandle(index as u32))
            })
    }
}

/// `getMovers` (`code/ablmc2.cpp:363-389`) treats team IDs as roster queries and
/// unit IDs as squad queries. Preserve each source roster's order.
fn movers_for_group_id(b: &SimBindings<'_>, id: i32) -> Option<Vec<ObjectHandle>> {
    if (500..=508).contains(&id) {
        return Some(
            b.world
                .teams
                .teams
                .get((id - 500) as usize)
                .and_then(Option::as_ref)
                .map(|team| {
                    team.roster
                        .iter()
                        .filter_map(|wid| b.world.objects.by_watch_id(*wid))
                        .collect()
                })
                .unwrap_or_default(),
        );
    }
    (1..=511).contains(&id).then(|| {
        b.world
            .objects
            .movers
            .iter()
            .copied()
            .filter(|h| {
                b.world
                    .objects
                    .get(*h)
                    .and_then(crate::combat::core_of)
                    .is_some_and(|m| m.squad_id == id)
            })
            .collect()
    })
}

fn change_mover_team(b: &mut SimBindings<'_>, h: ObjectHandle, new_team: i32) {
    let Some((old_team, slot, wid)) =
        b.world.objects.get(h).and_then(|o| {
            crate::combat::core_of(o).map(|c| (c.team.0, c.pilot_slot, o.c.watch_id))
        })
    else {
        return;
    };
    if crate::mission::spawn::set_team(b.world, h, new_team as i8, slot as usize).is_err() {
        return;
    }
    if let Some(team) = b
        .world
        .teams
        .teams
        .get_mut(usize::try_from(old_team).unwrap_or(usize::MAX))
        .and_then(Option::as_mut)
    {
        team.roster.retain(|member| *member != wid);
    }
    if let Some(team) = b
        .world
        .teams
        .teams
        .get_mut(new_team as usize)
        .and_then(Option::as_mut)
    {
        if !team.roster.contains(&wid) {
            team.roster.push(wid);
        }
    }
}

/// `execSetGeneralAlarm` / `execGetGeneralAlarm` (code/ablmc2.cpp:5229-5258).
pub fn get_general_alarm(b: &SimBindings<'_>) -> i32 {
    i32::from(b.world.mission.general_alarm)
}

pub fn set_general_alarm(b: &mut SimBindings<'_>, value: i32) {
    b.world.mission.general_alarm = value != 0;
}

/// `execSetInvulnerable` (code/ablmc2.cpp:5861-5874).
pub fn set_invulnerable(b: &mut SimBindings<'_>, value: bool) {
    b.world.mission.invulnerable = value;
}

/// `execGetSensorsWorking` (code/ablmc2.cpp:3073-3090): -1 unless a mover has an enabled sensor component.
pub fn sensors_working(b: &SimBindings<'_>, id: i32) -> i32 {
    let Some(h) = object(b, id) else { return -1 };
    let Some(sensor) = b.world.sensors.pool.iter().find(|s| s.owner == h) else {
        return -1;
    };
    let Some(obj) = b.world.objects.get(h) else {
        return -1;
    };
    if !obj.exists() || !obj.c.awake() || obj.c.is_disabled() || sensor.team_index < 0 {
        return 0;
    }
    let inventory_enabled = match &obj.kind {
        Kind::Mech(m) => m
            .loadout
            .inventory
            .get(sensor.inventory_index)
            .is_some_and(|item| !item.disabled),
        Kind::Vehicle(v) => v
            .loadout
            .inventory
            .get(sensor.inventory_index)
            .is_some_and(|item| !item.disabled),
        _ => return -1,
    };
    let enabled = sensor.not_shutdown && inventory_enabled;
    i32::from(enabled)
}

/// `execSetTonnage` (code/ablmc2.cpp:3484-3507).
pub fn set_tonnage(b: &mut SimBindings<'_>, id: i32, tonnage: f32) {
    if let Some(h) = object(b, id) {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.c.tonnage = tonnage;
        }
    }
}

/// `execSetExplosionDamage` (code/ablmc2.cpp:3510-3532).
pub fn set_explosion_damage(b: &mut SimBindings<'_>, id: i32, damage: f32) {
    if let Some(h) = object(b, id) {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.c.expl_damage = damage;
        }
    }
}

/// `execSetExplosionRadius` (code/ablmc2.cpp:3536-3558).
pub fn set_explosion_radius(b: &mut SimBindings<'_>, id: i32, radius: f32) {
    if let Some(h) = object(b, id) {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.c.expl_radius = radius;
        }
    }
}

/// `execGetObjectDmgPts` (code/ablmc2.cpp:3317-3335): integer truncation of structure damage, 0 otherwise.
pub fn object_damage_points(b: &SimBindings<'_>, id: i32) -> i32 {
    object(b, id)
        .and_then(|h| b.world.objects.get(h))
        .and_then(|o| o.structure())
        .map_or(0, |s| s.damage as i32)
}

/// `execGetObjectMaxDmg` (code/ablmc2.cpp:3339-3357): current damage points, despite the legacy native name.
pub fn object_max_damage(b: &SimBindings<'_>, id: i32) -> i32 {
    object_damage_points(b, id)
}

/// `execGetPilotID` (code/ablmc2.cpp:3185-3204).
pub fn pilot_id(b: &SimBindings<'_>, id: i32) -> i32 {
    object(b, id)
        .and_then(|h| b.world.objects.get(h))
        .and_then(|o| match &o.kind {
            Kind::Mech(m) => Some(i32::from(m.m.pilot_slot)),
            Kind::Vehicle(v) => Some(i32::from(v.m.pilot_slot)),
            _ => None,
        })
        .unwrap_or(-1)
}

/// `execGetPilotWounds` (code/ablmc2.cpp:3208-3227).
pub fn pilot_wounds(b: &SimBindings<'_>, id: i32) -> f32 {
    let Some(h) = object(b, id) else { return 0.0 };
    let slot = b.world.objects.get(h).and_then(|o| match &o.kind {
        Kind::Mech(m) => Some(m.m.pilot_slot),
        Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    });
    slot.and_then(|s| b.world.pilots.get(s))
        .map_or(0.0, |p| p.wounds)
}

/// `execSetPilotWounds` (code/ablmc2.cpp:3231-3253): the C++ clamps the upper bound to 6.
pub fn set_pilot_wounds(b: &mut SimBindings<'_>, id: i32, wounds: i32) {
    let Some(h) = object(b, id) else { return };
    let slot = b.world.objects.get(h).and_then(|o| match &o.kind {
        Kind::Mech(m) => Some(m.m.pilot_slot),
        Kind::Vehicle(v) => Some(v.m.pilot_slot),
        _ => None,
    });
    if let Some(p) = slot.and_then(|s| b.world.pilots.get_mut(s)) {
        p.wounds = wounds.min(6) as f32;
    }
}

/// `execGetArmorPts` (code/ablmc2.cpp:3135-3155).
pub fn armor_points(b: &SimBindings<'_>, id: i32, maximum: bool) -> i32 {
    let Some(h) = object(b, id) else { return 0 };
    let Some(o) = b.world.objects.get(h) else {
        return 0;
    };
    let loadout = match &o.kind {
        Kind::Mech(m) => &m.loadout,
        Kind::Vehicle(v) => &v.loadout,
        _ => return 0,
    };
    if maximum {
        loadout.armor.iter().map(|(_, max)| i32::from(*max)).sum()
    } else {
        loadout
            .armor
            .iter()
            .map(|(current, _)| *current)
            .sum::<f32>() as i32
    }
}

/// `execGetCurrentBRValue` (code/ablmc2.cpp:3093-3111).
pub fn current_br_value(b: &SimBindings<'_>, id: i32) -> i32 {
    object(b, id)
        .and_then(|h| b.world.objects.get(h))
        .map_or(-1, |o| i32::from(o.c.cur_cv))
}

/// `execSetCurrentBRValue` (code/ablmc2.cpp:3115-3130).
pub fn set_current_br_value(b: &mut SimBindings<'_>, id: i32, value: i32) {
    if let Some(h) = object(b, id) {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.c.cur_cv = value as i16;
        }
    }
}

/// `execSetObjectActive` (code/ablmc2.cpp:2811-2857). `getMovers` roster order is preserved.
pub fn set_object_active(b: &mut SimBindings<'_>, id: i32, active: bool) -> i32 {
    let handles: Vec<ObjectHandle> = if (1..=511).contains(&id) {
        if (500..=508).contains(&id) {
            b.world
                .teams
                .teams
                .get((id - 500) as usize)
                .and_then(Option::as_ref)
                .map(|t| {
                    t.roster
                        .iter()
                        .filter_map(|wid| b.world.objects.by_watch_id(*wid))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            b.world
                .objects
                .movers
                .iter()
                .copied()
                .filter(|h| {
                    b.world
                        .objects
                        .get(*h)
                        .and_then(crate::combat::core_of)
                        .is_some_and(|c| c.squad_id == id)
                })
                .collect()
        }
    } else {
        object(b, id).into_iter().collect()
    };
    let mut changed = 0;
    for h in handles {
        if let Some(o) = b.world.objects.get_mut(h) {
            if o.c.awake() != active {
                o.c.set_awake(active);
                changed += 1;
            }
        }
    }
    changed
}

/// `execObjectChangeSides` (code/ablmc2.cpp:2029-2048). Unlike the C++ fatal branch, mover team changes are applied safely.
pub fn change_sides(b: &mut SimBindings<'_>, id: i32, new_team: i32) {
    let handles = if (1..=511).contains(&id) {
        movers_for_group_id(b, id)
    } else {
        None
    };
    if let Some(handles) = handles {
        for h in handles {
            change_mover_team(b, h, new_team);
        }
        return;
    }
    let Some(h) = object(b, id) else { return };
    if b.world.objects.get(h).is_some_and(|o| o.is_mover()) {
        change_mover_team(b, h, new_team);
        return;
    }
    crate::object::structure::set_team_id(b.world, h, new_team, false);
}

/// `execObjectSuicide` (code/ablmc2.cpp:2177-2200): marks the selected object(s) nonexistent.
pub fn object_suicide(b: &mut SimBindings<'_>, id: i32) {
    let handles = movers_for_group_id(b, id).unwrap_or_else(|| object(b, id).into_iter().collect());
    for h in handles {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.set_exists(false);
        }
    }
}

/// `execObjectCreate` (code/ablmc2.cpp:2206-2230): restore the object and refresh sensors against it.
pub fn object_create(b: &mut SimBindings<'_>, id: i32) -> i32 {
    let Some(h) = object_including_removed(b, id) else {
        return 0;
    };
    let Some(o) = b.world.objects.get_mut(h) else {
        return 0;
    };
    if o.exists() {
        return 0;
    }
    o.set_exists(true);
    let lookers = b.world.objects.movers.clone();
    for looker in lookers {
        crate::ai::sensors::scan_mover(b.world, looker, h);
    }
    id
}

/// `execObjectRemove` (code/ablmc2.cpp:2920-3030): mover removal is deferred when called by that mover's own brain.
pub fn object_remove(b: &mut SimBindings<'_>, id: i32) -> i32 {
    let Some(h) = object(b, id) else { return 0 };
    if !b.world.objects.get(h).is_some_and(|o| o.is_mover()) {
        return 1;
    }
    if b.world
        .objects
        .get(h)
        .is_some_and(|o| o.c.watch_id == b.cur_object)
    {
        if let Some(o) = b.world.objects.get_mut(h) {
            o.c.set_flag(crate::object::flags::REMOVED, true);
        }
    } else {
        crate::mission::remove_mover(b.world, h);
    }
    0
}

/// `execObjectCommander` (code/ablmc2.cpp:2405-2428). Group ids have no commander result in the original.
pub fn object_commander(b: &SimBindings<'_>, id: i32) -> i32 {
    if (1..=511).contains(&id) {
        return -1;
    }
    object(b, id)
        .and_then(|h| b.world.objects.get(h))
        .map_or(-1, |o| o.commander_id())
}

/// `execObjectVisible` (code/ablmc2.cpp:2345-2382). For a group, test each live member;
/// this also avoids the original's null `object1` dereference in its group branch.
pub fn object_visible(b: &mut SimBindings<'_>, from_id: i32, to_id: i32) -> i32 {
    let Some(to) = object(b, to_id) else { return 0 };
    let froms =
        movers_for_group_id(b, from_id).unwrap_or_else(|| object(b, from_id).into_iter().collect());
    i32::from(
        froms
            .into_iter()
            .any(|from| crate::object::hooks::line_of_sight(b.world, from, to, 0.0)),
    )
}

/// `execInArea` (code/ablmc2.cpp:2990-3030): radius is in world units; `min_in == -1`
/// requires every non-disabled member to be inside and at least one eligible member.
pub fn in_area(
    b: &SimBindings<'_>,
    id: i32,
    center: crate::math::Vec3,
    radius: f32,
    min_in: i32,
) -> bool {
    let handles = movers_for_group_id(b, id).unwrap_or_else(|| object(b, id).into_iter().collect());
    let distance = |h: ObjectHandle| {
        b.world.objects.get(h).map(|o| {
            let p = o.c.position;
            crate::math::approximate_length(&crate::math::Vec3 {
                x: p.x - center.x,
                y: p.y - center.y,
                z: p.z - center.z,
            })
        })
    };
    if min_in == -1 {
        let eligible: Vec<_> = handles
            .iter()
            .filter(|h| b.world.objects.get(**h).is_some_and(|o| !o.c.is_disabled()))
            .collect();
        !eligible.is_empty()
            && eligible.iter().all(|h| {
                b.world.objects.get(**h).is_some_and(|o| {
                    !o.exists() || !o.c.awake() || distance(**h).is_some_and(|d| d <= radius)
                })
            })
    } else if (1..=511).contains(&id) {
        min_in > 0
            && handles
                .iter()
                .filter(|h| {
                    b.world.objects.get(**h).is_some_and(|o| {
                        o.exists()
                            && o.c.awake()
                            && !o.c.is_disabled()
                            && !o.c.is_destroyed()
                            && distance(**h).is_some_and(|d| d <= radius)
                    })
                })
                .count()
                >= min_in as usize
    } else if min_in == 0 {
        true
    } else {
        handles.first().is_some_and(|h| {
            b.world.objects.get(*h).is_some_and(|o| {
                o.exists()
                    && o.c.awake()
                    && !o.c.is_disabled()
                    && !o.c.is_destroyed()
                    && distance(*h).is_some_and(|d| d <= radius)
            })
        })
    }
}

/// `execSetSensorRange` (code/ablmc2.cpp:3477-3504).
pub fn set_sensor_range(b: &mut SimBindings<'_>, id: i32, range: f32) {
    let Some(h) = object(b, id) else { return };
    match b.world.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Building(_)) => {
            crate::ai::sensors::set_building_sensor_data(b.world, h, range, true)
        }
        _ => {
            if let Some(sensor) = b.world.sensors.pool.iter_mut().find(|s| s.owner == h) {
                sensor.range = range;
            }
        }
    }
}

/// `execCreateInfantry` (code/ablmc2.cpp:3034-3076): adds the shipped infantry mover and
/// stores the building part id in pilot integer memory zero.
pub fn create_infantry(
    b: &mut SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    team: i32,
    commander: i32,
    position: mc2_abl::ArrayRef,
    building_part_id: i32,
) -> i32 {
    let spec = crate::mission::init::MoverInitData {
        pilot_file: "pmw00080".into(),
        brain: "infantry001".into(),
        csv_file: "PM101100".into(),
        object_number: 53,
        roster_index: 255,
        control_type: 2,
        control_data_type: 0,
        variant: 0,
        position: crate::math::Vec3 {
            x: ctx.get_f32(position, 0),
            y: ctx.get_f32(position, 1),
            z: 0.0,
        },
        rotation: 0,
        team: team as i8,
        commander: commander as i8,
        gesture_id: 2,
        active: true,
        exists: true,
        capturable: false,
        ..Default::default()
    };
    let h = match crate::mission::spawn::add_mover(b.world, &spec) {
        Ok(h) => h,
        Err(_) => {
            b.world
                .note_combat_unavailable("createInfantry: mover construction failed");
            return 0;
        }
    };
    if let Some(slot) = crate::combat::fx::pilot_slot(b.world, h) {
        if let Some(cell) = b
            .world
            .pilots
            .get_mut(slot)
            .and_then(|pilot| pilot.memory.first_mut())
        {
            *cell = building_part_id;
        }
    }
    0
}

/// `execSetObjectDamage` (code/ablmc2.cpp:3374-3410): damage only rises, percentage caps at 100.
pub fn set_object_damage(b: &mut SimBindings<'_>, id: i32, percentage: i32) -> i32 {
    let Some(h) = object(b, id) else { return 0 };
    let max = crate::object::structure::damage_level(b.world, h);
    let cur = b
        .world
        .objects
        .get(h)
        .and_then(|o| o.structure())
        .map_or(0.0, |s| s.damage);
    if max <= 0.0 || percentage <= 0 {
        return 0;
    }
    let wanted = (percentage.min(100) as f32 / 100.0) * max;
    if wanted <= cur {
        return 0;
    }
    let damage = wanted - cur + 1.0;
    let shot =
        crate::combat::WeaponShotInfo::init(b.world, crate::ids::WatchId::NONE, 0, damage, 0, 0.0);
    crate::object::hooks::weapon_hit(b.world, h, &shot);
    1
}

/// `execGetHomeTeam` (code/ablmc2.cpp:4745-4762).
pub fn get_home_team(b: &SimBindings<'_>) -> i32 {
    crate::object::hooks::home_team(b.world) + 500
}

/// `execIsTeamCapturing` (code/ablmc2.cpp:4670-4688).
pub fn is_team_capturing(b: &mut SimBindings<'_>, team: i32, target: i32) -> bool {
    let Some(h) = object(b, target) else {
        return false;
    };
    let wid = b.world.objects.watch_id(h);
    let team = match team {
        0 => None,
        500..=508 => Some((team - 500) as usize),
        _ => return false,
    };
    b.world.objects.is_team_capturing(team, wid)
}

/// `execSetAnimation` is a shipped no-op because `USE_BUILDING_APPR` is disabled (ablmc2.cpp:3645-3674).
pub fn set_animation(_b: &mut SimBindings<'_>, _id: i32, _state: i32, _sub_state: i32) {}

/// `execSetBuildingName` is a shipped no-op because `USE_RESOURCE_STRINGS` is disabled (ablmc2.cpp:3963-3987).
pub fn set_building_name(_b: &mut SimBindings<'_>, _id: i32, _name: i32) {}

/// `execAddPrisoner` returns -1 in the shipped build; its implementation is behind `USE_PRISONS` (ablmc2.cpp:4161-4231).
pub fn add_prisoner() -> i32 {
    -1
}

/// `execEndMovieMode` has no simulation effect in the headless build (ablmc2.cpp:5209-5227).
pub fn end_movie_mode() {}

/// `execPlayWave` queues the same voice-over event consumed by the app audio layer (ablmc2.cpp:6259-6270).
pub fn play_wave(b: &mut SimBindings<'_>, file_name: String) -> i32 {
    b.world
        .events
        .push(crate::events::SimEvent::VoiceOver(file_name));
    0
}

/// `execGetUnitStatus` returns `GameObject::getStatusRating() * 100` (ablmc2.cpp:4402-4418).
pub fn unit_status(b: &SimBindings<'_>, id: i32) -> f32 {
    let Some(h) = object(b, id) else { return 0.0 };
    let Some(o) = b.world.objects.get(h) else {
        return 0.0;
    };
    let armor_ratio = |loadout: &crate::combat::Loadout, index: usize| {
        let (current, maximum) = loadout.armor.get(index).copied().unwrap_or_default();
        current / f32::from(maximum)
    };
    let pilot_factor = |slot: u16| {
        const FACTORS: [f32; 7] = [1.0, 0.95, 0.85, 0.75, 0.50, 0.30, 0.0];
        b.world
            .pilots
            .get(slot)
            .map_or(1.0, |pilot| FACTORS[(pilot.wounds as usize).min(6)])
    };
    let rating = match &o.kind {
        Kind::Mech(m) => {
            let a = &m.loadout.armor;
            let head = armor_ratio(&m.loadout, 0) * 0.6 + 0.4;
            let ct = armor_ratio(&m.loadout, 1);
            let rct = armor_ratio(&m.loadout, 8);
            let center_torso = if a
                .get(1)
                .is_some_and(|x| x.0 > a.get(8).map_or(0.0, |r| r.0))
            {
                rct * 0.5 + 0.5
            } else {
                ct * 0.5 + 0.5
            };
            let sum_ratio = |indices: &[usize]| {
                let cur: f32 = indices.iter().map(|i| a.get(*i).map_or(0.0, |x| x.0)).sum();
                let max: f32 = indices
                    .iter()
                    .map(|i| a.get(*i).map_or(0.0, |x| f32::from(x.1)))
                    .sum();
                cur / max
            };
            let armor_effect = head
                * center_torso
                * (sum_ratio(&[9, 2, 10, 3]) * 0.25 + 0.75)
                * (sum_ratio(&[4, 5]) * 0.25 + 0.75)
                * (sum_ratio(&[6, 7]) * 0.25 + 0.75);
            let weapons =
                m.loadout.stats.weapon_effectiveness / m.loadout.stats.max_weapon_effectiveness;
            let alive = if o.c.is_destroyed() || o.c.is_disabled() {
                0.0
            } else {
                1.0
            };
            weapons * armor_effect * pilot_factor(m.m.pilot_slot) * alive
        }
        Kind::Vehicle(v) => {
            let armor_effect: f32 = (0..4)
                .map(|i| armor_ratio(&v.loadout, i) * 0.6 + 0.4)
                .product();
            let stats = &v.loadout.stats;
            let weapon_effect = if stats.max_weapon_effectiveness == 0.0 {
                1.0
            } else {
                stats.weapon_effectiveness / stats.max_weapon_effectiveness
            };
            let alive = if o.c.is_destroyed() || o.c.is_disabled() {
                0.0
            } else {
                1.0
            };
            weapon_effect * armor_effect * pilot_factor(v.m.pilot_slot) * alive
        }
        _ => o.structure().map_or(0.0, |s| {
            let max_health = crate::object::structure::damage_level(b.world, h);
            let mut value = (max_health - s.damage) / max_health;
            if value < 0.0 {
                value = 0.0;
            }
            value
        }),
    };
    rating * 100.0
}

/// `relativePositionToPoint` in `mclib/move.cpp:361-458`: -distance from the point along the rotated north vector.
fn adjust_relative_position(
    b: &SimBindings<'_>,
    from: crate::math::Vec3,
    to: crate::math::Vec3,
    flags: i32,
) -> crate::math::Vec3 {
    let passable_start = flags & 2 != 0;
    let (start, goal) = if passable_start {
        (from, to)
    } else {
        (to, from)
    };
    let dx = goal.x - start.x;
    let dy = goal.y - start.y;
    let max_length = (dx * dx + dy * dy).sqrt();
    let mut delta = crate::math::Vec3 {
        x: dx,
        y: dy,
        z: 0.0,
    };
    let step_length = crate::terrain::WORLD_UNITS_PER_CELL * 0.5;
    let len = max_length;
    if len == 0.0 {
        return from;
    }
    delta.x = delta.x / len * step_length;
    delta.y = delta.y / len * step_length;
    let mut current = start;
    let mut last_good = current;
    let mut ray_length = 0.0;
    let mut clear = b.world.terrain.passable_at(&current);
    while (if passable_start { clear } else { !clear }) && ray_length < max_length {
        last_good = current;
        current.x += delta.x;
        current.y += delta.y;
        let rx = current.x - start.x;
        let ry = current.y - start.y;
        ray_length = (rx * rx + ry * ry).sqrt();
        clear = b.world.terrain.passable_at(&current);
    }
    let safety =
        b.world.terrain.world_units_map_side * 0.5 - crate::terrain::WORLD_UNITS_PER_VERTEX;
    last_good.x = last_good.x.clamp(-safety, safety);
    last_good.y = last_good.y.clamp(-safety, safety);
    last_good.z = b.world.terrain.elevation(&last_good);
    last_good
}

pub fn relative_position_to_point(
    b: &SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    point: mc2_abl::ArrayRef,
    angle: f32,
    distance: f32,
    flags: i32,
    output: mc2_abl::ArrayRef,
) {
    let mut shift = crate::math::Vec3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    crate::math::rotate_xy(&mut shift, angle);
    let scale = -distance * crate::terrain::WORLD_UNITS_PER_METER;
    let from = crate::math::Vec3 {
        x: ctx.get_f32(point, 0),
        y: ctx.get_f32(point, 1),
        z: 0.0,
    };
    let to = crate::math::Vec3 {
        x: from.x + shift.x * scale,
        y: from.y + shift.y * scale,
        z: 0.0,
    };
    let to = adjust_relative_position(b, from, to, flags);
    write_relative_position(ctx, output, to);
}

/// `Mover::relativePosition` (code/mover.cpp:3037-3115), with the same orientation and units; flags bit 0 selects absolute north.
pub fn relative_position_to_object(
    b: &SimBindings<'_>,
    ctx: &mut NativeCtx<'_>,
    id: i32,
    angle: f32,
    distance: f32,
    flags: i32,
    output: mc2_abl::ArrayRef,
) {
    let Some(h) = object(b, id) else { return };
    let Some(o) = b.world.objects.get(h) else {
        return;
    };
    let mut shift = crate::math::Vec3 {
        x: 0.0,
        y: 1.0,
        z: 0.0,
    };
    if flags & 1 == 0 {
        shift.y = -1.0;
        crate::math::rotate_xy(&mut shift, -o.c.rotation);
    }
    crate::math::rotate_xy(&mut shift, angle);
    let scale = -distance * crate::terrain::WORLD_UNITS_PER_METER;
    let from = crate::math::Vec3 {
        x: o.c.position.x,
        y: o.c.position.y,
        z: 0.0,
    };
    let to = crate::math::Vec3 {
        x: from.x + shift.x * scale,
        y: from.y + shift.y * scale,
        z: 0.0,
    };
    let to = adjust_relative_position(b, from, to, flags);
    write_relative_position(ctx, output, to);
}

/// `execConvertCoords` (code/ablmc2.cpp:4870-4903): type zero converts world coordinates to cell row/column.
pub fn convert_coords(
    ctx: &mut NativeCtx<'_>,
    b: &SimBindings<'_>,
    kind: i32,
    world: mc2_abl::ArrayRef,
    cell: mc2_abl::ArrayRef,
) -> i32 {
    if kind == 0 {
        let p = crate::math::Vec3 {
            x: ctx.get_f32(world, 0),
            y: ctx.get_f32(world, 1),
            z: ctx.get_f32(world, 2),
        };
        let (row, col) = b.world.terrain.world_to_cell(&p);
        ctx.set_i32(cell, 0, row);
        ctx.set_i32(cell, 1, col);
    } else {
        let p = b
            .world
            .terrain
            .cell_to_world(ctx.get_i32(cell, 0), ctx.get_i32(cell, 1));
        ctx.set_f32(world, 0, p.x);
        ctx.set_f32(world, 1, p.y);
        ctx.set_f32(world, 2, 0.0);
    }
    0
}

pub fn lock_gate(_b: &mut SimBindings<'_>, _id: i32, _open: Option<bool>) {
    // Shipped C++ compiles this implementation out under USE_GATES (ablmc2.cpp:4238-4318).
}

pub fn is_gate_open(_b: &SimBindings<'_>, _id: i32) -> bool {
    false
}

/// `MoverGroup::handleMateFiredWeapon` / `triggerAlarm` (code/group.cpp:1056-1064): notify
/// every pilot in the shooter's commander-local group with the shooter's watch id.
pub fn handle_mate_fired_weapon(world: &mut crate::World, shooter: ObjectHandle) {
    let Some((commander, group)) = world.objects.get(shooter).and_then(|o| {
        crate::combat::core_of(o)
            .filter(|c| c.group_id >= 0)
            .map(|c| (o.commander_id(), c.group_id))
    }) else {
        return;
    };
    let wid = world.objects.watch_id(shooter).0 as i32;
    let members: Vec<_> = world
        .objects
        .movers
        .iter()
        .copied()
        .filter(|h| {
            world
                .objects
                .get(*h)
                .is_some_and(|o| o.commander_id() == commander)
                && world
                    .objects
                    .get(*h)
                    .and_then(crate::combat::core_of)
                    .is_some_and(|c| c.group_id == group)
        })
        .collect();
    for h in members {
        if let Some(slot) = crate::combat::fx::pilot_slot(world, h) {
            crate::ai::warrior::trigger_alarm(
                world,
                slot,
                crate::ai::warrior::alarm::MATE_FIRED_WEAPON,
                wid,
            );
        }
    }
}

/// Array helper used by `getrelativepositiontoobject` / point variant (code/ablmc2.cpp:4321-4398).
pub fn write_relative_position(
    ctx: &mut NativeCtx<'_>,
    output: mc2_abl::ArrayRef,
    to: crate::math::Vec3,
) {
    ctx.set_f32(output, 0, to.x);
    ctx.set_f32(output, 1, to.y);
    ctx.set_f32(output, 2, to.z);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::RngMode;

    #[test]
    fn general_alarm_and_invulnerability_round_trip() {
        let mut w = crate::World::new(RngMode::Pcg32, 7);
        let mut b = SimBindings::mission(&mut w);
        assert_eq!(get_general_alarm(&b), 0);
        set_general_alarm(&mut b, -4);
        set_invulnerable(&mut b, true);
        assert_eq!(get_general_alarm(&b), 1);
        assert!(b.world.mission.invulnerable);
        set_general_alarm(&mut b, 0);
        set_invulnerable(&mut b, false);
        assert_eq!(get_general_alarm(&b), 0);
        assert!(!b.world.mission.invulnerable);
    }

    #[test]
    fn object_queries_return_the_cxx_neutral_values_for_missing_ids() {
        let mut w = crate::World::new(RngMode::Pcg32, 7);
        let b = SimBindings::mission(&mut w);
        assert_eq!(sensors_working(&b, 99_999), -1);
        assert_eq!(pilot_id(&b, 99_999), -1);
        assert_eq!(pilot_wounds(&b, 99_999), 0.0);
        assert_eq!(armor_points(&b, 99_999, false), 0);
        assert_eq!(armor_points(&b, 99_999, true), 0);
        assert_eq!(object_damage_points(&b, 99_999), 0);
        assert_eq!(object_max_damage(&b, 99_999), 0);
        assert_eq!(unit_status(&b, 99_999), 0.0);
        assert_eq!(current_br_value(&b, 99_999), -1);
        assert_eq!(object_commander(&b, 99_999), -1);
    }

    #[test]
    fn object_lifecycle_queries_fail_safely_for_missing_ids() {
        let mut w = crate::World::new(RngMode::Pcg32, 7);
        let mut b = SimBindings::mission(&mut w);
        assert_eq!(object_create(&mut b, 99_999), 0);
        assert_eq!(object_remove(&mut b, 99_999), 0);
        assert_eq!(set_object_active(&mut b, 99_999, true), 0);
    }

    #[test]
    fn headless_native_returns_match_shipped_noop_semantics() {
        let mut w = crate::World::new(RngMode::Pcg32, 7);
        let mut b = SimBindings::mission(&mut w);
        set_animation(&mut b, 99_999, 2, 1);
        set_building_name(&mut b, 99_999, 12);
        end_movie_mode();
        assert_eq!(add_prisoner(), -1);
        assert_eq!(play_wave(&mut b, "test_wave".into()), 0);
        assert!(matches!(
            b.world.events.last(),
            Some(crate::events::SimEvent::VoiceOver(name)) if name == "test_wave"
        ));
        assert!(!is_team_capturing(&mut b, 0, 99_999));
    }
}
