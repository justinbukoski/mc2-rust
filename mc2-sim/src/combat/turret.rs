// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Turret fire control (turret.cpp `fireWeapon`, `handleWeaponFire`, target search in `TurretType::handleCollision`).
//!
//! UNAVAILABLE (other lanes): the
//! pilot's `updateAttackerStatus` (recorded as a [`PilotCall`](crate::ai::pilot::PilotCall)), the multiplayer weapon-fire chunk.

use mc2_sim_data::component::{ComponentForm, WEAPON_FLAG_STREAK};
use mc2_sim_data::objtype::ObjectTypeData;

use crate::combat::attack::distance_from;
use crate::combat::fire_math::{
    miss_offset_points, miss_radius, missile_count_hit, missile_count_miss,
};
use crate::combat::fx::{self, alarm};
use crate::combat::stats::{comp, form_of, weapon_stats};
use crate::combat::turret_math::{resolve_los, turret_attack_chance, LosOutcome, MechSpec};
use crate::combat::{los, ATTACK_TO_DESTROY};
use crate::combat::{weapon_bolt, AttackSource, WeaponShotInfo};
use crate::ids::ObjectHandle;
use crate::math::rel_facing_to;
use crate::object::{hooks, Kind};
use crate::World;

/// Specialty skill indices of `MechWarrior::specialtySkills` the turret reads (warrior.h:207).
const SPEC_LIGHT_MECH: usize = 0;
const SPEC_MEDIUM_MECH: usize = 8;
const SPEC_HEAVY_MECH: usize = 14;
const SPEC_ASSAULT_MECH: usize = 20;

/// `Turret::fireWeapon(target, weaponId)` (turret.cpp:1356): one shot of one turret weapon at `target`. A shot rolls `RandomNumber(100)`
/// against the turret's attack chance (skill plus 50 against a standing target), starts the weapon's recycle, and creates a bolt: at the
/// target on a hit, at a visible spot beside it on a miss. RNG order: the hit roll, the hit location draws, `connect`'s dice.
/// Returns [`LosOutcome::Clear`] when the shot proceeded (or on an early exit unrelated to sight), otherwise the failed sight check.
pub fn fire(
    w: &mut World,
    turret: ObjectHandle,
    target: ObjectHandle,
    weapon_id: usize,
) -> LosOutcome {
    let (Some(comps), Some(tables)) = (w.data.components.clone(), w.data.gamesys.tables.clone())
    else {
        return LosOutcome::Clear;
    };
    let Some(t) = w.objects.type_of(turret) else {
        return LosOutcome::Clear;
    };
    let ObjectTypeData::Turret(tt) = &t.data else {
        return LosOutcome::Clear;
    };
    let weapon_master_id = tt.weapon_master_id[weapon_id];
    let pilot_skill = tt.pilot_skill as f32;
    if weapon_master_id < 0 {
        return LosOutcome::Clear;
    }
    let master = weapon_master_id as u8;
    let ws = weapon_stats(comp(&comps, master)).cloned();
    let form = form_of(comp(&comps, master));
    let turret_pos = match w.objects.get(turret) {
        Some(o) => o.c.position,
        None => return LosOutcome::Clear,
    };

    // currentWeaponNode = appearance->getWeaponNode(0); line of sight to the target from the node's turret
    let current_weapon_node = w.appearance.weapon_node(turret, 0);
    if let Some(o) = w.objects.get_mut(turret) {
        if let Kind::Turret(ts) = &mut o.kind {
            ts.current_weapon_node = current_weapon_node;
        }
    }
    let mut los = LosOutcome::Blocked;
    if current_weapon_node != -1 {
        let _ = w
            .appearance
            .weapon_node_position(turret, current_weapon_node);
        w.appearance
            .set_weapon_node_used(turret, current_weapon_node);
        let indirect = tables
            .mover_general
            .indirect_fire_weapons
            .contains(&weapon_master_id);
        let radius = hooks::appear_radius(w, turret);
        let direct_los = hooks::line_of_sight(w, turret, target, radius);
        los = resolve_los(w, turret, target, direct_los, indirect);
    }
    if los != LosOutcome::Clear {
        return los;
    }

    let (target_pos, target_rot) = match w.objects.get(target) {
        Some(o) => (o.c.position, o.c.rotation),
        None => return LosOutcome::Clear,
    };
    let entry_angle = if w.objects.get(target).is_some_and(|o| o.is_mover()) {
        crate::ai::decide::rel_facing_to(w, target, &turret_pos, -1)
    } else {
        rel_facing_to(&target_pos, target_rot, &turret_pos)
    };
    let is_streak_missile = ws
        .as_ref()
        .is_some_and(|s| s.flags & WEAPON_FLAG_STREAK != 0);
    let is_missile = form == Some(ComponentForm::WeaponMissile);
    let weapon_damage = ws.as_ref().map_or(0.0, |s| s.damage);
    let missile_amount = ws.as_ref().map_or(0, |s| s.ammo_amount);
    let effect_type = i32::from(ws.as_ref().map_or(0, |s| s.special_effect) as u8);
    let recycle_time = ws.as_ref().map_or(0.0, |s| s.recycle_time);

    // Turret::calcAttackChance(target, &range, weaponId)
    let range_class = ws.as_ref().and_then(|s| s.range).map_or(0, |r| r.index());
    let (target_speed_sq, mech) = {
        let o = w.objects.get(target).expect("target");
        let speed_sq = match crate::combat::core_of(o) {
            Some(core) => core.velocity.x * core.velocity.x + core.velocity.y * core.velocity.y,
            None => 0.0,
        };
        let mech = if matches!(o.kind, Kind::Mech(_)) {
            fx::pilot_slot(w, target)
                .and_then(|s| w.pilots.get(s))
                .map(|p| MechSpec {
                    tonnage: o.c.tonnage,
                    light: p.specialty.get(SPEC_LIGHT_MECH).copied().unwrap_or(false),
                    medium: p.specialty.get(SPEC_MEDIUM_MECH).copied().unwrap_or(false),
                    heavy: p.specialty.get(SPEC_HEAVY_MECH).copied().unwrap_or(false),
                    assault: p.specialty.get(SPEC_ASSAULT_MECH).copied().unwrap_or(false),
                })
        } else {
            None
        };
        (speed_sq, mech)
    };
    let distance = distance_from(&turret_pos, &target_pos);
    let chance = turret_attack_chance(
        weapon_master_id,
        pilot_skill,
        distance,
        tables.general.weapon_range[0],
        tables.general.weapon_range[1],
        tables.general.weapon_ranges[range_class][0],
        tables.general.weapon_ranges[range_class][1],
        Some(target_speed_sq),
        mech,
    )
    .chance;
    let attack_chance = chance as i64;
    if attack_chance == -1 {
        return LosOutcome::Clear;
    }
    let hit_roll = {
        let r = w.rng.0.clone();
        let n = r.borrow_mut().sim.random_number(100);
        i64::from(n)
    };
    let turret_wid = w.objects.watch_id(turret);
    let scenario_time = w.clock.scenario_time;
    let target_is_mover = w.objects.get(target).is_some_and(|o| o.is_mover());
    if target_is_mover {
        // targetPilot->updateAttackerStatus(partId, scenarioTime): the C++ passes the turret's PART id where a watch id is
        // expected (turret.cpp:1416, a C++ bug); the turret's watch id is passed here (docs/DIFFERENCES.md)
        if let Some(slot) = fx::pilot_slot(w, target) {
            if let Some(p) = w.pilots.get_mut(slot) {
                p.pending_calls
                    .push(crate::ai::pilot::PilotCall::UpdateAttackerStatus {
                        attacker: turret_wid,
                        time_bits: scenario_time.to_bits(),
                    });
            }
            // the C++ call is immediate: record the attacker now, before the damage alarms
            crate::ai::calls::run_pending_calls(w, target);
        }
    }
    // startWeaponRecycle(weaponId)
    set_turret_time(w, turret, weapon_id, |ts| {
        ts.ready_time[weapon_id] = scenario_time + recycle_time;
    });
    let source_hs = current_weapon_node;
    let aim_location = -1;
    if hit_roll < attack_chance {
        set_turret_time(w, turret, weapon_id, |ts| {
            ts.last_fire_time[weapon_id] = scenario_time;
        });
        let damage = if is_missile {
            missile_count_hit(missile_amount, is_streak_missile) as f32 * weapon_damage
        } else {
            weapon_damage
        };
        // the bolt of a missile salvo exists before its hit location is rolled
        let bolt = if is_missile {
            let Some(b) = weapon_bolt::create(w, effect_type) else {
                return LosOutcome::Clear; // Fatal(-1, " couldnt create weapon FX ")
            };
            Some(b)
        } else {
            None
        };
        let mut hit_location = -2;
        if aim_location == -1 {
            hit_location = hooks::calc_hit_location(
                w,
                target,
                turret,
                weapon_master_id,
                AttackSource::WeaponFire,
                ATTACK_TO_DESTROY,
            );
        }
        debug_assert_ne!(hit_location, -2);
        let shot = WeaponShotInfo::init(
            w,
            turret_wid,
            weapon_master_id,
            damage,
            hit_location,
            entry_angle,
        );
        let bolt = match bolt {
            Some(b) => b,
            None => {
                let Some(b) = weapon_bolt::create(w, effect_type) else {
                    return LosOutcome::Clear;
                };
                b
            }
        };
        // `targetHotSpot = body[hitLocation].hotSpotNumber` for a mech target: connect() overwrites it
        weapon_bolt::connect_target(w, bolt, turret, target, Some(&shot), source_hs, 0);
    } else if !is_streak_missile {
        set_turret_time(w, turret, weapon_id, |ts| {
            ts.last_fire_time[weapon_id] = scenario_time;
        });
        let damage = if is_missile {
            missile_count_miss(missile_amount) as f32 * weapon_damage
        } else {
            weapon_damage
        };
        let mut shot =
            WeaponShotInfo::init(w, turret_wid, weapon_master_id, damage, -1, entry_angle);
        // the missile salvo's bolt is created after its shot, the other bolt before the offsets as well
        let Some(bolt) = weapon_bolt::create(w, effect_type) else {
            return LosOutcome::Clear;
        };
        // C++ turret.cpp:1655-1700 dereferences `target->getPosition()` even for "no target"; the target is never NULL here
        let radius = miss_radius(true);
        let mut position_offset = target_pos;
        let mut can_see_hit = false;
        for (x, y) in miss_offset_points(position_offset, radius) {
            position_offset.x = x;
            position_offset.y = y;
            position_offset.z = w.terrain.elevation(&position_offset);
            can_see_hit = los::line_of_sight_point(w, turret, position_offset);
            if can_see_hit {
                break;
            }
        }
        if !can_see_hit {
            shot = WeaponShotInfo::init(w, turret_wid, weapon_master_id, 0.0, -1, entry_angle);
        }
        if can_see_hit {
            // miss location is in LOS. Hit the ground
            weapon_bolt::connect_point(w, bolt, turret, position_offset, Some(&shot), source_hs);
        } else {
            // Miss location is NOT in LOS. Hit Target with ZERO damage!!!
            weapon_bolt::connect_target(w, bolt, turret, target, Some(&shot), source_hs, 0);
        }
    }
    if target_is_mover {
        fx::trigger_alarm(w, target, alarm::TARGET_OF_WEAPONFIRE, turret_wid.0 as i32);
    }
    LosOutcome::Clear
}

fn set_turret_time(
    w: &mut World,
    turret: ObjectHandle,
    _weapon_id: usize,
    f: impl FnOnce(&mut crate::object::structure::TurretState),
) {
    if let Some(o) = w.objects.get_mut(turret) {
        if let Kind::Turret(ts) = &mut o.kind {
            f(ts);
        }
    }
}
