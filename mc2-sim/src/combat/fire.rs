// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `BattleMech::fireWeapon`: weapon firing in the world (`code/mech.cpp:7926-8590`), single player.
//!
//! A shot is gated (status, body state, recycle, range, ammunition, the weapon node's ripple-fire recycle), rolls its hit once
//! (`RandomNumber(100)` against the integer attack chance), starts the weapon's recycle, spends ammunition, and creates a
//! [`WeaponBolt`](crate::combat::weapon_bolt) connected to the target (a hit), to a point beside it (a miss) or to a target point.
//! RNG order, as in the C++: the hit roll, the hit location draws, the target-hot-spot dice of `connect`.
//!
//! Not ported (presentation or other lanes), each marked `UNAVAILABLE` below: the pilot's skill-use counters
//! (`incNumSkillUses/Successes`), `MechWarrior::updateAttackerStatus` (lane D: recorded as [`PilotCall`]s), the multiplayer
//! weapon-fire chunk, the debug print. `MoverGroup::handleMateFiredWeapon` is routed through the mission object natives. Vehicles:
//! see [`fire_weapon`].

/// The hook name for the group notification of a shot (`MoverGroup::handleMateFiredWeapon`, group.cpp:1030,1073-1075).
pub const MATE_FIRED_HOOK: &str =
    "lane D: MoverGroup::handleMateFiredWeapon (PILOT_ALARM_MATE_FIRED_WEAPON to group pilots)";

use crate::appearance::tables::{body_state, MECH_STATE_BY_GESTURE};
use crate::combat::ammo::reduce_ammo;
use crate::combat::attack::{calc_attack_chance, distance_from, map_cell_diagonal};
use crate::combat::fire_math::{
    fire_gate, is_weapon_ready, miss_offset_points, miss_radius, missile_count_hit,
    missile_count_miss, source_hot_spot, start_weapon_recycle, weapon_in_range, FireGateInput,
};
use crate::combat::fx::{self, alarm, RADIO_AMMO_OUT};
use crate::combat::stats::{comp, form_of, pilot_gunnery, weapon_node_type, weapon_stats};
use crate::combat::{loadout_of, loadout_of_mut, weapon_bolt, AttackSource, UNLIMITED_SHOTS};
use crate::combat::{los, WeaponShotInfo};
use crate::ids::ObjectHandle;
use crate::math::{rel_facing_to, Vec3};
use crate::object::{hooks, Kind, ObjectStatus};
use crate::World;
use mc2_sim_data::component::ComponentForm;
use mc2_sim_data::pilot::Skill;

/// What `fireWeapon` returned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FireResult {
    /// The return code: 0 fired (hit or miss), 1 the mover cannot fire, 3 the weapon is not ready, 4 the target is gone or out of range.
    pub code: i32,
    /// `dmgDone`: the damage of the shot after difficulty scaling (hits only; 0 otherwise).
    pub dmg_done: f32,
}

/// `Mover::fireWeapon(target, targetTime, weaponIndex, attackType, aimLocation, targetPoint, dmgDone)` for a mech or ground vehicle attacker.
pub fn fire_weapon(
    w: &mut World,
    attacker: ObjectHandle,
    weapon_index: i32,
    target: Option<ObjectHandle>,
    aim_location: i32,
    target_point: Option<Vec3>,
    attack_type: i32,
) -> FireResult {
    let mut result = FireResult {
        code: 0,
        dmg_done: 0.0,
    };
    let Some(ao) = w.objects.get(attacker) else {
        return FireResult {
            code: 1,
            dmg_done: 0.0,
        };
    };
    // `GroundVehicle::fireWeapon` (gvehicl.cpp:4502-5100) is the same shot as the mech's, with a vehicle's own gates: no body
    // state, the weapon node comes from the vehicle appearance, and no gunnery points are added.
    let is_vehicle = matches!(ao.kind, Kind::Vehicle(_));
    // `GroundVehicle::fireWeapon` checks status before the weapon index (gvehicl.cpp:4504): every non-normal status returns 1.
    if is_vehicle && ao.c.status != ObjectStatus::Normal {
        return FireResult {
            code: 1,
            dmg_done: 0.0,
        };
    }
    if !is_vehicle && !matches!(ao.kind, Kind::Mech(_)) {
        return FireResult {
            code: 3,
            dmg_done: 0.0,
        };
    }
    let (Some(comps), Some(tables)) = (w.data.components.clone(), w.data.gamesys.tables.clone())
    else {
        return FireResult {
            code: 1,
            dmg_done: 0.0,
        };
    };
    let Some(l) = loadout_of(ao) else {
        return FireResult {
            code: 1,
            dmg_done: 0.0,
        };
    };
    if weapon_index < 0 || !l.is_weapon_index(i64::from(weapon_index)) {
        // Mover::isWeaponReady on a bad index reads outside the inventory in the C++.
        // GroundVehicle::fireWeapon returns 2 here (gvehicl.cpp:4507); BattleMech::fireWeapon returns 3.
        return FireResult {
            code: if is_vehicle { 2 } else { 3 },
            dmg_done: 0.0,
        };
    }
    let wi = weapon_index as usize;
    let item = l.inventory[wi];
    let master_id = item.master_id;
    let ws = weapon_stats(comp(&comps, master_id)).cloned();
    let form = form_of(comp(&comps, master_id));
    let attacker_position = ao.c.position;
    let scenario_time = w.clock.scenario_time;
    let num_shots = l.weapon_shots(&comps, i64::from(weapon_index));

    // getBodyState(): parked or fallen mechs cannot fire
    let gesture = w.appearance.current_gesture_id(attacker);
    let body = usize::try_from(gesture)
        .ok()
        .and_then(|g| MECH_STATE_BY_GESTURE.get(g))
        .copied()
        .unwrap_or(body_state::NORMAL);
    let parked_or_fallen = !is_vehicle
        && matches!(
            body,
            body_state::PARKED | body_state::FALLEN_BACKWARD | body_state::FALLEN_FORWARD
        );

    // the target (or point) and the distance to it in meters
    let target_obj = target.and_then(|t| w.objects.get(t));
    let target_destroyed = target_obj.map(|t| t.c.is_destroyed());
    let distance_to_target = if let Some(t) = target_obj {
        distance_from(&attacker_position, &t.c.position)
    } else if let Some(p) = target_point {
        distance_from(&attacker_position, &p)
    } else {
        0.0
    };
    let range_class = ws.as_ref().and_then(|s| s.range).map_or(0, |r| r.index());
    let (range_min, range_max) = (
        tables.general.weapon_ranges[range_class][0],
        tables.general.weapon_ranges[range_class][1],
    );
    let in_range = weapon_in_range(
        range_min,
        range_max,
        distance_to_target,
        map_cell_diagonal(),
    );

    // Ripple fire: the weapon node this weapon fires from, and whether its recycle is green
    let (source_hs, node_recycle) = if is_vehicle {
        // gvehicl.cpp:4628-4651: the least used weapon node of the weapon's form; no node, or a node past the first four
        // hot spots, is "not ready" (code 3). The C++ test `sourceHotSpot > 3` compares a node index that already includes the
        // appearance's two smoke nodes, so a third weapon node could never fire and its use count never moved on (a vehicle
        // with three or four weapon nodes stopped firing for good): the port accepts every weapon node (DIFFERENCES.md).
        let node = w.appearance.weapon_node(attacker, weapon_node_type(form));
        let first = crate::appearance::vehicle::GV_SMOKE_NODES;
        let last = first + crate::appearance::vehicle::GV_WEAPON_NODES;
        if (first..last).contains(&node) {
            (node, w.appearance.weapon_node_recycle(attacker, node))
        } else {
            (node, 9999.0)
        }
    } else {
        let hs = source_hot_spot(
            i32::from(item.facing),
            w.appearance.arm_off(attacker, false),
            w.appearance.arm_off(attacker, true),
        );
        (hs, w.appearance.weapon_node_recycle(attacker, hs))
    };

    let status = ao.c.status;
    let on_game_terrain = w.terrain.is_game_select_position(&attacker_position);
    // `AeroSpaceSpotter` vehicles never fire (gvehicl.cpp:4517). The flag comes from the shipped vehicle object packet.
    let aerospace_spotter = is_vehicle
        && w.objects.get(attacker).is_some_and(|o| {
            w.data
                .vehicle_objects
                .get(&(o.c.type_handle as u32))
                .is_some_and(|vehicle| vehicle.aerospace_spotter)
        });
    // BattleMech::isWeaponReady also refuses a weapon with no health; Mover::isWeaponReady (vehicles) does not.
    let weapon_ready = (is_vehicle || item.health != 0) && is_weapon_ready(item, scenario_time);
    let gate = FireGateInput {
        status,
        parked_or_fallen,
        weapon_ready,
        on_game_terrain,
        target_destroyed,
        has_target_point: target_point.is_some(),
        in_range,
        num_shots,
        // GroundVehicle::fireWeapon checks its weapon node only after the hit roll (roll gvehicl.cpp:4600, node 4643-4651): below
        node_recycle: if is_vehicle { 0.0 } else { node_recycle },
    };
    if let Some(code) = fire_gate(&gate) {
        // The spotter test sits after the weapon-ready and terrain tests (gvehicl.cpp:4510-4517) and returns 4.
        let code = if aerospace_spotter && weapon_ready && on_game_terrain {
            4
        } else {
            code
        };
        return FireResult {
            code,
            dmg_done: 0.0,
        };
    }
    if aerospace_spotter {
        return FireResult {
            code: 4,
            dmg_done: 0.0,
        };
    }

    // Set the hotspot for the target. A vehicle does it after its node check (gvehicl.cpp:4655-4660, below): `getWeaponNode`
    // on a mech target uses up one of that mech's nodes.
    let mut target_hot_spot = 0;
    if !is_vehicle {
        target_hot_spot = target_weapon_node(w, target);
    }
    // GroundVehicle::fireWeapon tells the target pilot before the entry angle, the attack chance and the roll (gvehicl.cpp:4586-4594)
    if is_vehicle {
        notify_target_pilot(w, attacker, target, scenario_time);
    }
    let entry_angle = match target.and_then(|t| w.objects.get(t).map(|o| (t, o))) {
        // `target->relFacingTo(position)`: a mover measures it in the plane (Mover::relFacingTo, mover.cpp:2909)
        Some((t, o)) if o.is_mover() => {
            crate::ai::decide::rel_facing_to(w, t, &attacker_position, -1)
        }
        Some((_, t)) => rel_facing_to(&t.c.position, t.c.rotation, &attacker_position),
        None => 0.0,
    };
    let is_streak_missile = ws
        .as_ref()
        .is_some_and(|s| s.flags & mc2_sim_data::component::WEAPON_FLAG_STREAK != 0);
    let is_missile = form == Some(ComponentForm::WeaponMissile);
    let weapon_damage = ws.as_ref().map_or(0.0, |s| s.damage);
    let missile_amount = ws.as_ref().map_or(0, |s| s.ammo_amount);
    let ammo_master_id = ws.as_ref().map_or(0, |s| s.ammo_master_id);
    let effect_type = i32::from(ws.as_ref().map_or(0, |s| s.special_effect) as u8);
    let recycle_time = ws.as_ref().map_or(0.0, |s| s.recycle_time);

    let attack_chance = calc_attack_chance(
        w,
        attacker,
        target,
        aim_location,
        weapon_index,
        target_point,
    )
    .chance as i64;
    let hit_roll = {
        let r = w.rng.0.clone();
        let n = r.borrow_mut().sim.random_number(100);
        i64::from(n)
    };
    let hit = hit_roll < attack_chance;

    // Gunnery skill points (the stat counters `incNumSkillUses` / `incNumSkillSuccesses` are UNAVAILABLE: skill counters)
    if let (Some(t), false) = (target, is_vehicle) {
        let team2 = w.objects.get(t).is_some_and(|o| o.team_id() == 1);
        let try_ = skill_gain(w, true);
        add_gunnery_points(w, attacker, if team2 { try_ } else { try_ / 10.0 });
        if hit {
            let success = skill_gain(w, false);
            add_gunnery_points(w, attacker, if team2 { success } else { success / 10.0 });
        }
    }
    let mut hit_location = -2;
    if target.is_some() && hit && aim_location != -1 {
        // If it's an aimed shot, the desired location on the target
        hit_location = aim_location;
    }

    // BattleMech::fireWeapon tells the target pilot after its roll (mech.cpp:8092); a vehicle did it before its roll, above
    if !is_vehicle {
        notify_target_pilot(w, attacker, target, scenario_time);
    }
    let attacker_wid = w.objects.watch_id(attacker);
    let target_is_mover = target
        .and_then(|t| w.objects.get(t))
        .is_some_and(|o| o.is_mover());

    if is_vehicle {
        // Ripple fire (gvehicl.cpp:4643-4651): no usable node, or a node still recycling, returns 3 with the roll already drawn
        // and the target pilot already told.
        if node_recycle > 0.0 {
            return FireResult {
                code: 3,
                dmg_done: 0.0,
            };
        }
        // appearance->setWeaponNodeUsed(sourceHotSpot) (gvehicl.cpp:4653)
        w.appearance.set_weapon_node_used(attacker, source_hs);
        target_hot_spot = target_weapon_node(w, target);
    }

    // Weapon must recycle...
    if let Some(l) = w.objects.get_mut(attacker).and_then(loadout_of_mut) {
        start_weapon_recycle(&mut l.inventory[wi], scenario_time, recycle_time);
    }

    if hit {
        if num_shots != UNLIMITED_SHOTS {
            deduct_weapon_shot(w, attacker, ammo_master_id);
        }
        // Attack hit. A missile salvo counts its missiles; any other weapon is one hit.
        let damage = if is_missile {
            let num_missiles = missile_count_hit(missile_amount, is_streak_missile);
            num_missiles as f32 * weapon_damage
        } else {
            weapon_damage
        };
        if let Some(t) = target {
            if aim_location == -1 {
                hit_location = hooks::calc_hit_location(
                    w,
                    t,
                    attacker,
                    weapon_index,
                    AttackSource::WeaponFire,
                    attack_type,
                );
            }
        } else {
            hit_location = -1;
        }
        debug_assert_ne!(hit_location, -2, "Mech.FireWeapon: Bad Hit Location");
        let shot = WeaponShotInfo::init(
            w,
            attacker_wid,
            i32::from(master_id),
            damage,
            hit_location,
            entry_angle,
        );
        result.dmg_done = shot.damage;
        match weapon_bolt::create(w, effect_type) {
            None => {
                // Unlike GroundVehicle::fireWeapon, Mech::fireWeapon does not synchronously hit a target when bolt creation fails
                // (mech.cpp:8195-8220): it only trips a mine at a target point.
                if let (true, Some(t)) = (is_vehicle, target) {
                    // GroundVehicle::fireWeapon applies a hit synchronously if the weapon effect has no bolt type or the bolt
                    // pool cannot supply a slot (gvehicl.cpp:4758-4762, 4852-4856).
                    hooks::weapon_hit(w, t, &shot);
                } else if let Some(p) = target_point {
                    crate::combat::bolt_impact::trip_mine(w, p);
                }
            }
            Some(b) => {
                if let Some(t) = target {
                    weapon_bolt::connect_target(
                        w,
                        b,
                        attacker,
                        t,
                        Some(&shot),
                        source_hs,
                        target_hot_spot,
                    );
                } else if let Some(p) = target_point {
                    // Hit the target point/terrain...
                    weapon_bolt::connect_point(w, b, attacker, p, Some(&shot), source_hs);
                }
            }
        }
    } else if !is_streak_missile {
        if num_shots != UNLIMITED_SHOTS {
            deduct_weapon_shot(w, attacker, ammo_master_id);
        }
        // Miss, so check for possible miss resolution here...
        let damage = if is_missile {
            let num_missiles = missile_count_miss(missile_amount);
            num_missiles as f32 * weapon_damage
        } else {
            weapon_damage
        };
        let mut shot = WeaponShotInfo::init(
            w,
            attacker_wid,
            i32::from(master_id),
            damage,
            -1,
            entry_angle,
        );
        // pick sights away from the target and check LOS to each one. If all are invisible, just hit the target with zero damage.
        let radius = miss_radius(target.is_some());
        let mut position_offset = match target.and_then(|t| w.objects.get(t)) {
            Some(t) => t.c.position,
            None => target_point.unwrap_or_default(),
        };
        let mut can_see_hit = false;
        for (x, y) in miss_offset_points(position_offset, radius) {
            position_offset.x = x;
            position_offset.y = y;
            position_offset.z = w.terrain.elevation(&position_offset);
            can_see_hit = los::line_of_sight_point(w, attacker, position_offset);
            if can_see_hit {
                break;
            }
        }
        if !can_see_hit {
            // OK, no miss location is visible. Hit the target with ZERO damage!!
            shot =
                WeaponShotInfo::init(w, attacker_wid, i32::from(master_id), 0.0, -1, entry_angle);
        }
        match weapon_bolt::create(w, effect_type) {
            None => crate::combat::bolt_impact::trip_mine(w, position_offset),
            Some(b) => {
                if can_see_hit {
                    // miss location is in LOS. Hit the ground
                    weapon_bolt::connect_point(
                        w,
                        b,
                        attacker,
                        position_offset,
                        Some(&shot),
                        source_hs,
                    );
                } else if let Some(t) = target {
                    // Miss location is NOT in LOS. Hit Target with ZERO damage!!!
                    weapon_bolt::connect_target(
                        w,
                        b,
                        attacker,
                        t,
                        Some(&shot),
                        source_hs,
                        target_hot_spot,
                    );
                }
                // OTHERWISE, we tried to hit the ground but we can't see the location we shot at: draw no weapon effect.
            }
        }
    }

    // Trigger the WEAPON TARGET event.
    let target_wid = target.map_or(0, |t| w.objects.watch_id(t).0 as i32);
    fx::trigger_alarm(w, attacker, alarm::FIRED_WEAPON, target_wid);
    if let (Some(t), true) = (target, target_is_mover) {
        fx::trigger_alarm(w, t, alarm::TARGET_OF_WEAPONFIRE, attacker_wid.0 as i32);
    }
    crate::mission::natives_object::handle_mate_fired_weapon(w, attacker);
    result
}

/// `target->getAppearance()->getWeaponNode(MECH3D_WEAPONTYPE_ANY)`, 0 without a target appearance.
fn target_weapon_node(w: &mut World, target: Option<ObjectHandle>) -> i32 {
    match target {
        Some(t) if w.appearance.get(t).is_some() => w.appearance.weapon_node(t, 0),
        _ => 0,
    }
}

/// `targetPilot->updateAttackerStatus(getWatchID(), scenarioTime)` for a mover target (lane D records it as a pilot call).
fn notify_target_pilot(
    w: &mut World,
    attacker: ObjectHandle,
    target: Option<ObjectHandle>,
    scenario_time: f32,
) {
    let Some(t) = target.filter(|&t| w.objects.get(t).is_some_and(|o| o.is_mover())) else {
        return;
    };
    let attacker_wid = w.objects.watch_id(attacker);
    if let Some(slot) = fx::pilot_slot(w, t) {
        if let Some(p) = w.pilots.get_mut(slot) {
            p.pending_calls
                .push(crate::ai::pilot::PilotCall::UpdateAttackerStatus {
                    attacker: attacker_wid,
                    time_bits: scenario_time.to_bits(),
                });
        }
        // the C++ call is immediate: record the attacker now, before the damage alarms
        crate::ai::calls::run_pending_calls(w, t);
    }
}

/// `SkillTry[MWS_GUNNERY]` / `SkillSuccess[MWS_GUNNERY]` of `[SkillSettings]`.
fn skill_gain(w: &World, try_: bool) -> f32 {
    w.data.gamesys.tables.as_ref().map_or(0.0, |t| {
        let i = Skill::Gunnery.index();
        if try_ {
            t.skills.skill_try[i]
        } else {
            t.skills.skill_success[i]
        }
    })
}

/// `pilot->skillPoints[MWS_GUNNERY] += amount`.
fn add_gunnery_points(w: &mut World, attacker: ObjectHandle, amount: f32) {
    if let Some(slot) = fx::pilot_slot(w, attacker) {
        if let Some(l) = w.mission.pilot_loads.get_mut(usize::from(slot)) {
            l.skill_points[Skill::Gunnery.index()] += amount;
        }
    }
}

/// `Mover::deductWeaponShot(weaponIndex)` = `reduceAmmo(ammoMasterId, 1)`. When the ammo type runs out the C++ recomputes the weapon
/// effectiveness and fire ranges and the pilot radios `RADIO_AMMO_OUT`.
fn deduct_weapon_shot(w: &mut World, attacker: ObjectHandle, ammo_master_id: u8) {
    let spent = match w.objects.get_mut(attacker).and_then(loadout_of_mut) {
        Some(l) => reduce_ammo(l, i32::from(ammo_master_id), 1).spent,
        None => return,
    };
    if !spent {
        return;
    }
    let gunnery = pilot_gunnery(w, attacker);
    let scenario_time = w.clock.scenario_time;
    if let (Some(comps), Some(gs)) = (w.data.components.clone(), w.data.gamesys.tables.clone()) {
        if let Some(l) = w.objects.get_mut(attacker).and_then(loadout_of_mut) {
            l.calc_weapon_effectiveness(gunnery, scenario_time, false);
            l.calc_fire_ranges(&comps, &gs, scenario_time);
        }
    }
    fx::radio_message(w, attacker, RADIO_AMMO_OUT);
}
