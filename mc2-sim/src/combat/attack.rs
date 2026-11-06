// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Mover::calcAttackChance` (mover.cpp:6230), reached through `BattleMech::calcAttackChance` (mech.cpp:6623) and
//! `GroundVehicle::calcAttackChance` (gvehicl.cpp:4026): the percentage chance that a weapon hits. It is the pilot's gunnery
//! (scaled by the difficulty for the home commander) plus specialist modifiers, clamped to [5, 95]; a target that is not a mover, or
//! is disabled, is always hit (100). There are no range or movement modifiers: range only decides whether the shot is possible (-1).

use crate::combat::shot::apply_difficulty_skill;
use crate::combat::stats::{comp, weapon_stats};
use crate::combat::{loadout_of, HOME_COMMANDER};
use crate::ids::ObjectHandle;
use crate::math::{approximate_length, Vec3};
use crate::object::ObjectClass;
use crate::terrain::{METERS_PER_WORLD_UNIT, WORLD_UNITS_PER_CELL, WORLD_UNITS_PER_METER};
use crate::World;
use mc2_sim_data::pilot::Skill;

/// Result of [`calc_attack_chance`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AttackChance {
    /// The chance in percent; -1 when the target is out of range, -1000 for a bad weapon index (the mech/vehicle checks), -9999
    /// when there is neither a target nor a target point.
    pub chance: f32,
    /// `*range` (`FIRERANGE_SHORT` 0, `MEDIUM` 1, `LONG` 2), set once the distance is known.
    pub range: Option<i32>,
}

/// `MAX_SHORT_RANGE` ... `MAX_LONG_RANGE` (mover.cpp:6217): float products of the meter figure and `worldUnitsPerMeter`.
const MAX_SHORT_RANGE: f32 = 60.0 * WORLD_UNITS_PER_METER;
const MIN_MEDIUM_RANGE: f32 = 31.0 * WORLD_UNITS_PER_METER;
const MAX_MEDIUM_RANGE: f32 = 120.0 * WORLD_UNITS_PER_METER;
const MIN_LONG_RANGE: f32 = 61.0 * WORLD_UNITS_PER_METER;
const MAX_LONG_RANGE: f32 = 180.0 * WORLD_UNITS_PER_METER;
/// `WeaponSpecialistModifier` (mover.cpp:209).
const WEAPON_SPECIALIST_MODIFIER: f32 = 20.0;

/// `MapCellDiagonal = Terrain::worldUnitsPerCell * 1.4142 * metersPerWorldUnit` (move.cpp:811): a double product narrowed to float.
#[allow(clippy::approx_constant)] // the literal is the game's 1.4142, not sqrt(2)
pub fn map_cell_diagonal() -> f32 {
    (f64::from(WORLD_UNITS_PER_CELL) * 1.4142 * f64::from(METERS_PER_WORLD_UNIT)) as f32
}

/// `GameObject::distanceFrom(goal)` (gameobj.cpp:1762): the planar distance in meters.
pub fn distance_from(position: &Vec3, goal: &Vec3) -> f32 {
    let result = Vec3 {
        x: position.x - goal.x,
        y: position.y - goal.y,
        z: 0.0,
    };
    approximate_length(&result) * METERS_PER_WORLD_UNIT
}

/// Specialty skill indices (`SpecialtySkill`, warrior.h:207).
mod spec {
    pub const LIGHT_MECH: usize = 0;
    pub const LASER: usize = 1;
    pub const LIGHT_AC: usize = 2;
    pub const MEDIUM_AC: usize = 3;
    pub const SRM: usize = 4;
    pub const SMALL_ARMS: usize = 5;
    pub const MEDIUM_MECH: usize = 8;
    pub const PULSE_LASER: usize = 9;
    pub const ER_LASER: usize = 10;
    pub const LRM: usize = 11;
    pub const HEAVY_MECH: usize = 14;
    pub const PPC: usize = 15;
    pub const HEAVY_AC: usize = 16;
    pub const SHORT_RANGE: usize = 17;
    pub const MEDIUM_RANGE: usize = 18;
    pub const LONG_RANGE: usize = 19;
    pub const ASSAULT_MECH: usize = 20;
    pub const GAUSS: usize = 21;
    pub const SHARPSHOOTER: usize = 22;
}

fn has(skills: &[bool], i: usize) -> bool {
    skills.get(i).copied().unwrap_or(false)
}

/// `Mover::calcAttackChance(target, aimLocation, targetTime, weaponIndex, modifiers, range, targetPoint)`. `target_time` and
/// `modifiers` are not used by the C++ and are not taken. Reads `w.mission.difficulty` for the home commander's skill scaling.
pub fn calc_attack_chance(
    w: &World,
    attacker: ObjectHandle,
    target: Option<ObjectHandle>,
    aim_location: i32,
    weapon_index: i32,
    target_point: Option<Vec3>,
) -> AttackChance {
    let none = |chance: f32| AttackChance {
        chance,
        range: None,
    };
    let Some(ao) = w.objects.get(attacker) else {
        return none(-1000.0);
    };
    let Some(l) = loadout_of(ao) else {
        return none(-1000.0);
    };
    // BattleMech/GroundVehicle::calcAttackChance: the weapon index must be a weapon (-1000)
    if weapon_index < l.num_other() as i32
        || weapon_index >= (l.num_other() + l.num_weapons()) as i32
    {
        return none(-1000.0);
    }
    let Some(comps) = w.data.components.as_ref() else {
        return none(-1000.0);
    };
    let Some(tables) = w.data.gamesys.tables.as_ref() else {
        return none(-1000.0);
    };
    let my_core = crate::combat::core_of(ao);
    let pilot = crate::combat::fx::pilot_slot(w, attacker).and_then(|s| w.pilots.get(s));

    // First, let's find out what kind of object we're targeting...
    let target_obj = target.and_then(|t| w.objects.get(t));
    let is_mech_target = target_obj.is_some_and(|t| t.c.class == ObjectClass::BattleMech);
    let target_position = if let Some(t) = target_obj {
        t.c.position
    } else if let Some(p) = target_point {
        p
    } else {
        return none(-9999.0);
    };

    let mut attack_chance = pilot.map_or(0, |p| p.skills[Skill::Gunnery.index()]) as f32;

    // Up the gunnery skill by the difficulty modifier (the home commander's)
    if my_core.is_some_and(|c| i32::from(c.commander.0) == HOME_COMMANDER) {
        attack_chance = apply_difficulty_skill(w, attack_chance, true);
    }

    // General fire range...
    let distance_to_target = distance_from(&ao.c.position, &target_position);
    let range = if distance_to_target <= tables.general.weapon_range[0] {
        0
    } else if distance_to_target <= tables.general.weapon_range[1] {
        1
    } else {
        2
    };
    let found_range = Some(range);

    // Range (per weapon)...
    let wc = weapon_stats(comp(comps, l.inventory[weapon_index as usize].master_id));
    let wr = wc.and_then(|w| w.range).map_or(0, |r| r.index());
    let weapon_min_range = tables.general.weapon_ranges[wr][0];
    let weapon_long_range = tables.general.weapon_ranges[wr][1];
    let diagonal = map_cell_diagonal();
    if distance_to_target <= (weapon_min_range - diagonal) {
        return AttackChance {
            chance: -1.0,
            range: found_range,
        };
    }
    if distance_to_target > (weapon_long_range + diagonal) {
        // Out of range...
        return AttackChance {
            chance: -1.0,
            range: found_range,
        };
    }

    // Non-movers get 100% attack chance. Nothing is better than 100%, no need to modify. Exit here!
    let target_is_mover = target_obj.is_some_and(|t| t.is_mover());
    if target_obj.is_none() || !target_is_mover || target_obj.is_some_and(|t| t.c.is_disabled()) {
        return AttackChance {
            chance: 100.0,
            range: found_range,
        };
    }
    let target_obj = target_obj.expect("a mover");

    // Mech specialist modifiers (the target's pilot)
    if is_mech_target {
        let tpilot =
            crate::combat::fx::pilot_slot(w, target.expect("target")).and_then(|s| w.pilots.get(s));
        if let Some(tp) = tpilot {
            let tonnage = target_obj.c.tonnage;
            if tonnage < 40.0 && has(&tp.specialty, spec::LIGHT_MECH) {
                attack_chance -= 30.0;
            } else if (40.0..60.0).contains(&tonnage) && has(&tp.specialty, spec::MEDIUM_MECH) {
                attack_chance -= 20.0;
            } else if ((60.0..80.0).contains(&tonnage) && has(&tp.specialty, spec::HEAVY_MECH))
                || ((80.0..100.0).contains(&tonnage) && has(&tp.specialty, spec::ASSAULT_MECH))
            {
                attack_chance -= 10.0;
            }
        }
    }

    // Weapon specialist modifiers
    let master = l.inventory[weapon_index as usize].master_id;
    let sp: &[bool] = pilot.map_or(&[], |p| &p.specialty[..]);
    let table: [(usize, &[u8]); 11] = [
        (spec::LASER, &[139, 140, 147, 156]),
        (spec::PULSE_LASER, &[142, 144, 153, 151]),
        (spec::ER_LASER, &[141, 143, 150, 152]),
        (spec::LIGHT_AC, &[100, 103, 110]),
        (spec::MEDIUM_AC, &[101, 108, 111]),
        (spec::HEAVY_AC, &[102, 109, 112]),
        (spec::LRM, &[120, 123]),
        (spec::SRM, &[124, 125]),
        (spec::PPC, &[145, 146, 154]),
        (spec::GAUSS, &[99]),
        (spec::SMALL_ARMS, &[114, 155]),
    ];
    for (skill, ids) in table {
        if has(sp, skill) && ids.contains(&master) {
            attack_chance += WEAPON_SPECIALIST_MODIFIER;
        }
    }

    // Range specialist modifiers
    if distance_to_target <= MAX_SHORT_RANGE && has(sp, spec::SHORT_RANGE) {
        attack_chance += 15.0;
    }
    if (MIN_MEDIUM_RANGE..=MAX_MEDIUM_RANGE).contains(&distance_to_target)
        && has(sp, spec::MEDIUM_RANGE)
    {
        attack_chance += 15.0;
    }
    if (MIN_LONG_RANGE..=MAX_LONG_RANGE).contains(&distance_to_target) && has(sp, spec::LONG_RANGE)
    {
        attack_chance += 15.0;
    }

    // Aimed shots go here. Aimed or called shots are a percentage of the actual attack chance, so they come after all modifiers...
    if aim_location > -1 && is_mech_target {
        let mf = &tables.mover_fire;
        match aim_location {
            0 => attack_chance *= mf.head_shot_modifier,
            1..=3 => {}
            4 | 5 => attack_chance *= mf.arm_shot_modifier,
            6 | 7 => attack_chance *= mf.leg_shot_modifier,
            _ => {}
        }
        if has(sp, spec::SHARPSHOOTER) {
            attack_chance += 20.0;
        }
    }

    // No more modifiers EXCEPT what is in here.
    attack_chance = attack_chance.clamp(5.0, 95.0);
    AttackChance {
        chance: attack_chance,
        range: found_range,
    }
}
