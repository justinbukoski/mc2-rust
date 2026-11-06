// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Damage application: `BattleMech::handleWeaponHit` (mech.cpp:7412) and `GroundVehicle::handleWeaponHit` (gvehicl.cpp:4290) with
//! everything they call: armor -> internal structure -> transfer to the neighbouring location (`injureBodyLocation`,
//! `transferHitLocation`), ammunition explosions vented by CASE, critical hits (`calcCriticalHit` mech.cpp:7052,
//! `hitInventoryItem` 6828, `destroyBodyLocation` 6983), the piloting check (`pilotingCheck` 3500), `Mover::disable` (mover.cpp:6508),
//! `Mover::ammoExplosion` (6458), `BattleMech::handleEjection` (6744) and the two `handleDestruction`s (mech.cpp:994,
//! gvehicl.cpp:521).
//!
//! Pilot-side effects go through [`crate::combat::fx`]; explosions are lane A's `ObjectStore::create_explosion`.
//!
//! Pinned where the C++ reads out of bounds or uninitialised memory: `MasterComponent::disableLevel` is never assigned (zero in the
//! oracle's zero-filled heap), so a component is disabled by its first critical hit ([`DISABLE_LEVEL`]); `calcCriticalHit` with a
//! location that is not one of the eight body locations does nothing; a critical space that names an inventory slot beyond the
//! installed items counts as size 0 and `hitInventoryItem` ignores it; a hit location outside the armor table is ignored.

use crate::combat::fx::{self, alarm, death};
use crate::combat::hit::transfer_hit_location;
use crate::combat::stats::{
    armor as armor_loc, body, calc_leg_status, calc_torso_status, comp, form_of, threat_rating_for,
    IS_DAMAGE_DESTROYED, IS_DAMAGE_NONE, IS_DAMAGE_PARTIAL,
};
use crate::combat::{core_of, loadout_of, loadout_of_mut, WeaponShotInfo, HOME_COMMANDER};
use crate::ids::ObjectHandle;
use crate::mover::MechState;
use crate::object::{flags, Kind, ObjectStatus};
use crate::World;
use mc2_sim_data::component::ComponentForm;
use mc2_sim_data::objtype::ObjectTypeData;
use mc2_sim_data::pilot::Skill;

/// `MasterComponent::disableLevel` (cmponent.h:167): a `char` no loader assigns, so it is zero.
pub const DISABLE_LEVEL: i64 = 0;
/// `MECH_CRITICAL_HIT_ID` (weaponfx.h:17).
pub const MECH_CRITICAL_HIT_ID: i32 = 35;
/// `BETTY_MINES_GONE` (sounds.h:222). Sound ids from 31 up are Betty samples, below are `RADIO_*` messages.
pub const BETTY_MINES_GONE: i32 = 52;
/// `HIT_TYPE_*` (mech.h:78).
const HIT_TYPE_WEAK: i32 = 0;
const HIT_TYPE_MODERATE: i32 = 1;
/// `NumLocationCriticalSpaces` (mech.cpp:198).
const NUM_LOCATION_CRITICAL_SPACES: [usize; 8] = [6, 12, 12, 12, 12, 12, 6, 6];
/// `MechArmorToBodyLocation` (mech.cpp:427): the first eight armor locations are the body locations, the rear sections belong to
/// the torsos.
const ARMOR_TO_BODY_LOCATION: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 1, 2, 3];
/// `GROUNDVEHICLE_LOCATION_FRONT`, `TURRET`.
const GV_FRONT: usize = 0;
const GV_TURRET: usize = 4;

// ---------------------------------------------------------------------------------------------------------------------------
// small accessors

fn mech_mut(w: &mut World, h: ObjectHandle) -> Option<&mut MechState> {
    match &mut w.objects.get_mut(h)?.kind {
        Kind::Mech(m) => Some(m),
        _ => None,
    }
}

fn is_mech(w: &World, h: ObjectHandle) -> bool {
    w.objects
        .get(h)
        .is_some_and(|o| matches!(o.kind, Kind::Mech(_)))
}

fn is_disabled(w: &World, h: ObjectHandle) -> bool {
    w.objects.get(h).is_some_and(|o| o.c.is_disabled())
}

fn rn(w: &mut World, range: i32) -> i32 {
    let r = w.rng.0.clone();
    let n = r.borrow_mut().sim.random_number(range);
    n
}

fn set_status(w: &mut World, h: ObjectHandle, status: ObjectStatus) {
    if let Some(o) = w.objects.get_mut(h) {
        o.c.set_status(status, false);
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// Mover::disable, handleEjection, ammoExplosion, pilotingCheck

/// `Mover::disable(cause)` (mover.cpp:6508) with `GroundVehicle::disable` (gvehicl.cpp:2543).
pub fn disable(w: &mut World, h: ObjectHandle, cause: u32) {
    if is_disabled(w, h)
        || w.deferred_brain_calls
            .iter()
            .any(|c| matches!(c, crate::ai::brain::DeferredBrainCall::DisableRest(d, _) if *d == h))
    {
        // (a disable already under way inside an ABL native counts as done: the C++ finished it before returning)
        return;
    }
    // Immediately begin shutting down, then disable...
    fx::handle_alarm(w, h, alarm::VEHICLE_INCAPACITATED, cause);
    if w.abl_running {
        // The alarm raised inside an ABL native: the C++ runs the brain's handler inside `handleAlarm`, before `setStatus(DISABLED)`.
        // The handler is queued; the rest of `disable` is queued behind it and runs right after it (see `ai::brain::run_nested`).
        w.deferred_brain_calls
            .push(crate::ai::brain::DeferredBrainCall::DisableRest(h, cause));
        return;
    }
    disable_after_alarm(w, h, cause);
}

/// `Mover::disable` after the incapacitation alarm: the status change, the shutdown effects and the sensor loss.
pub fn disable_after_alarm(w: &mut World, h: ObjectHandle, cause: u32) {
    set_status(w, h, ObjectStatus::Disabled);
    let mech = is_mech(w, h);
    let air = w
        .objects
        .get(h)
        .and_then(core_of)
        .is_some_and(|c| c.move_level == 2);
    if let Some(o) = w.objects.get_mut(h) {
        match &mut o.kind {
            Kind::Mech(m) => m.m.flags.disable_this_frame = true,
            Kind::Vehicle(v) => v.m.flags.disable_this_frame = true,
            _ => {}
        }
        if let Some(l) = loadout_of_mut(o) {
            l.stats.cause_of_death = cause as i32;
        }
    }
    // (friendlyDestroyed / enemyDestroyed pick the mood music: presentation only.) Immediately lose contacts...
    fx::sensor_disable(w, h, false);

    if mech && !air {
        // We now want to play a BUNCH of critical hit explosions on the mech to indicate that we are going down.
        for name in [
            "cockpit",
            "hit_left",
            "hit_right",
            "weapon_righttorso",
            "weapon_lefttorso",
        ] {
            let at = w.appearance.node_name_position(h, name);
            crate::object::ObjectStore::create_explosion(
                w,
                MECH_CRITICAL_HIT_ID,
                None,
                at,
                0.0,
                0.0,
            );
        }
    }
    if !mech {
        // GroundVehicle::disable: timeLeft = 0
        if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
            v.m.time_left = 0.0;
        }
    }
}

/// `BattleMech::handleEjection` / `Mover::handleEjection` (mech.cpp:6744, mover.cpp:7160): the pilot ejects, the head blows (mechs) and the
/// mover is disabled. Always true.
pub fn handle_ejection(w: &mut World, h: ObjectHandle) -> bool {
    let normal = fx::pilot_slot(w, h)
        .and_then(|s| w.pilots.get(s))
        .is_some_and(|p| {
            p.status == fx::warrior_status::NORMAL || p.status == fx::warrior_status::WITHDRAWING
        });
    if normal {
        // First, eject...
        fx::eject(w, h);
        if w.abl_running
            && w.deferred_brain_calls.iter().any(
                |c| matches!(c, crate::ai::brain::DeferredBrainCall::DisableRest(d, _) if *d == h),
            )
        {
            // `eject` disabled the mover inside an ABL native: its incapacitation handler runs inside that call, before the head blows
            // (mech.cpp:6796). The rest of `handleEjection` waits behind the handler and the rest of `disable`.
            w.deferred_brain_calls
                .push(crate::ai::brain::DeferredBrainCall::EjectRest(h));
            return true;
        }
        eject_after_disable(w, h);
    }
    true
}

/// `handleEjection` after `eject`: the order flag, the head blowing (mechs) and, if not already, the disable.
pub fn eject_after_disable(w: &mut World, h: ObjectHandle) {
    if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
        l.stats.eject_order_given = true;
    }
    if is_mech(w, h) {
        // The head blows...
        destroy_body_location(w, h, body::HEAD);
    }
    // If we aren't already disabled, we are now...
    disable(w, h, death::EJECTION);
}

/// `BattleMech::pilotingCheck(situation, modifier)` (mech.cpp:3500): one `RandomNumber(100)`; a failed check is remembered until the
/// mech recovers (`failedPilotingCheck`). `modifier` is unused by the C++.
pub fn piloting_check(w: &mut World, h: ObjectHandle, situation: u32) {
    let Some(m) = mech_mut(w, h) else { return };
    if m.failed_piloting_check {
        return;
    }
    let mut pilot_roll = rn(w, 100) as f32;
    let tables = w.data.gamesys.tables.clone();
    let comps = w.data.components.clone();
    let slot = fx::pilot_slot(w, h);
    let piloting = slot
        .and_then(|s| w.pilots.get(s))
        .map_or(0, |p| p.skills[Skill::Piloting.index()]);
    let Some(m) = mech_mut(w, h) else { return };
    // Mech's situation... (PILOTCHECK_SITUATION_COLLISION)
    if situation & 2 != 0 {
        pilot_roll += 20.0;
    }
    // If leg(s) gone, it's hopeless...
    let l = &m.loadout;
    if l.body[body::RLEG].cur_internal_structure == 0.0
        || l.body[body::LLEG].cur_internal_structure == 0.0
    {
        pilot_roll += 100.0;
    }
    // Is gyro damaged? (A gyro that is not an inventory index is a fatal data error in the C++.)
    let gyro = usize::from(l.stats.gyro);
    if gyro < l.num_items() {
        let gyro_health = i64::from(l.inventory[gyro].health);
        let max = comps
            .as_ref()
            .and_then(|c| comp(c, l.inventory[gyro].master_id))
            .map_or(0, |c| i64::from(c.health));
        if gyro_health == 0 {
            pilot_roll += 100.0;
        } else if gyro_health < max {
            pilot_roll += 30.0;
        }
    }
    // Leg actuator(s) destroyed? (the index guards are alariq's: an actuator that was never found is 255)
    for a in [l.stats.actuator[2], l.stats.actuator[3]] {
        if usize::from(a) < crate::combat::MAX_MOVER_INVENTORY_ITEMS
            && usize::from(a) < l.inventory.len()
            && l.inventory[usize::from(a)].health == 0
        {
            pilot_roll += 10.0;
        }
    }
    // So, did we pass?
    let failed = pilot_roll >= piloting as f32;
    m.failed_piloting_check = failed;
    let (try_, success) = tables.as_ref().map_or((0.0, 0.0), |t| {
        (t.skills.skill_try[0], t.skills.skill_success[0])
    });
    if let Some(s) = slot {
        if let Some(pl) = w.mission.pilot_loads.get_mut(usize::from(s)) {
            pl.skill_points[Skill::Piloting.index()] += try_;
            if !failed {
                pl.skill_points[Skill::Piloting.index()] += success;
            }
        }
    }
}

/// `Mover::ammoExplosion(ammoIndex)` (mover.cpp:6458): the pilot takes 2 wounds and the remaining ammunition goes off in its body
/// location as a shot of `amount * explosiveDamage` (at most 255).
pub fn ammo_explosion(w: &mut World, h: ObjectHandle, ammo_index: usize) {
    fx::injure(w, h, 2.0, true);
    let comps = w.data.components.clone();
    let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) else {
        return;
    };
    let it = l.inventory[ammo_index];
    let explosive = comps
        .as_ref()
        .and_then(|c| comp(c, it.master_id))
        .map_or(0.0, |c| match &c.stats {
            mc2_sim_data::component::ComponentStats::Ammo {
                explosive_damage, ..
            } => *explosive_damage,
            _ => 0.0,
        });
    let remaining = i64::from(it.amount);
    let mut damage = remaining as f32 * explosive;
    l.inventory[ammo_index].amount = 0;
    // Make sure the ammoTypeTotal list is adjusted...
    if it.ammo_index >= 0 {
        if let Some(t) = l.ammo_totals.get_mut(it.ammo_index as usize) {
            t.1 -= remaining as i32;
        }
    }
    // We can use the bodyLocation as the hitLocation since the location codes for all body locations are the same as the armor
    // location codes. This is big damage for an ammo explosion!
    if damage > 255.0 {
        damage = 255.0;
    }
    let mut shot = WeaponShotInfo::init(
        w,
        crate::ids::WatchId::NONE,
        i32::from(it.master_id),
        damage,
        i32::from(it.body_location),
        0.0,
    );
    handle_weapon_hit_mut(w, h, &mut shot);
}

// ---------------------------------------------------------------------------------------------------------------------------
// criticals

/// `BattleMech::hitInventoryItem(itemIndex, setupOnly)` (mech.cpp:6828). Returns false (the C++ always does).
pub fn hit_inventory_item(w: &mut World, h: ObjectHandle, item: usize, setup_only: bool) -> bool {
    let comps = w.data.components.clone();
    let scenario_time = w.clock.scenario_time;
    let (master_id, body_location, health_left, num_items) = {
        let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) else {
            return false;
        };
        // HOW ABOUT... BEFORE we go trapsing through memory, we assure ourselves that this itemIndex is in the mech?
        if item >= l.num_items() {
            return false;
        }
        l.inventory[item].health = l.inventory[item].health.wrapping_sub(1);
        (
            l.inventory[item].master_id,
            l.inventory[item].body_location,
            l.inventory[item].health,
            l.num_items(),
        )
    };
    let _ = num_items;
    let Some(comps) = comps else { return false };
    let c = comp(&comps, master_id);
    let form = form_of(c);
    let max_health = c.map_or(0, |c| i64::from(c.health));
    let in_leg =
        usize::from(body_location) == body::LLEG || usize::from(body_location) == body::RLEG;

    // First, apply any effects caused whenever this component is hit (regardless of whether it's been disabled or destroyed with
    // this hit)...
    match form {
        Some(ComponentForm::Actuator) if in_leg => piloting_check(w, h, 0),
        Some(ComponentForm::Gyroscope) => piloting_check(w, h, 0),
        _ => {}
    }

    // getInventoryDamage(itemIndex) >= getDisableLevel()
    if max_health - i64::from(health_left) >= DISABLE_LEVEL {
        // Component is disabled with this hit...
        if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
            l.inventory[item].disabled = true;
        }
        // Now, we need to apply any special effects caused by this component's disabling blow...
        match form {
            Some(ComponentForm::Engine) => {
                if let Some(m) = mech_mut(w, h) {
                    m.engine_blow_time = scenario_time + 5.0;
                }
                disable(w, h, death::ENGINE);
            }
            Some(
                ComponentForm::Weapon
                | ComponentForm::WeaponEnergy
                | ComponentForm::WeaponBallistic
                | ComponentForm::WeaponMissile,
            ) => {
                recalc_weapons(w, h);
                fx::radio_message(w, h, fx::RADIO_WEAPON_DOWN);
            }
            Some(ComponentForm::Case) => {
                if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                    if let Some(b) = l.body.get_mut(usize::from(body_location)) {
                        b.case = false;
                    }
                }
            }
            _ => {}
        }
    }

    if health_left == 0 {
        match form {
            Some(ComponentForm::Cockpit) => {
                // Kill the pilot...
                fx::injure(w, h, 6.0, false);
            }
            Some(ComponentForm::Actuator) if in_leg => piloting_check(w, h, 0),
            Some(ComponentForm::Ammo) => ammo_explosion(w, h, item),
            _ => {}
        }

        let played = w
            .objects
            .get(h)
            .and_then(loadout_of)
            .is_some_and(|l| l.stats.played_critical_hit);
        if !setup_only && !played {
            // Play critical hit effect here.
            let at = w.appearance.node_position(h, 0);
            crate::object::ObjectStore::create_explosion(
                w,
                MECH_CRITICAL_HIT_ID,
                None,
                at,
                0.0,
                0.0,
            );
            crate::appearance::fx::start_smoking(w, h, 1);
            if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                l.stats.played_critical_hit = true;
            }
        }
    }
    false
}

/// `calcWeaponEffectiveness(false); calcFireRanges();` after a weapon went down or ran out of ammunition.
fn recalc_weapons(w: &mut World, h: ObjectHandle) {
    let (Some(comps), Some(gs)) = (w.data.components.clone(), w.data.gamesys.tables.clone()) else {
        return;
    };
    let gunnery = crate::combat::stats::pilot_gunnery(w, h);
    let t = w.clock.scenario_time;
    let mut out = false;
    if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
        out = l.calc_weapon_effectiveness(gunnery, t, false);
        l.calc_fire_ranges(&comps, &gs, t);
    }
    if out {
        fx::radio_message(w, h, crate::combat::fx::RADIO_WEAPONS_OUT);
    }
}

/// `BattleMech::destroyBodyLocation(bodyLocation)` (mech.cpp:6983).
pub fn destroy_body_location(w: &mut World, h: ObjectHandle, location: usize) {
    let scenario_time = w.clock.scenario_time;
    {
        let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) else {
            return;
        };
        if l.body[location].damage_state == IS_DAMAGE_DESTROYED {
            return;
        }
        // Effectively wipe out all critical spaces in the destroyed location...
        l.body[location].cur_internal_structure = 0.0;
        l.body[location].damage_state = IS_DAMAGE_DESTROYED;
        l.armor[location].0 = 0.0;
    }
    if location == body::LLEG || location == body::RLEG {
        leg_status(w, h);
        // Force a fall...
        piloting_check(w, h, 0);
    } else if location == body::CTORSO {
        torso_status(w, h);
    }

    let n = NUM_LOCATION_CRITICAL_SPACES[location];
    for cur_space in 0..n {
        let todo = {
            let Some(l) = w.objects.get(h).and_then(loadout_of) else {
                return;
            };
            let (id, hit) = l.body[location].critical_spaces[cur_space];
            (!hit && id != 255).then_some(id)
        };
        if let Some(id) = todo {
            if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                l.body[location].critical_spaces[cur_space].1 = true;
            }
            hit_inventory_item(w, h, usize::from(id), false);
        }
    }

    // If the location is a side torso, destroy the attached arm, as well...
    if location == body::CTORSO {
        // Mech essentially destroyed. Does it explode or simply shutdown?
        let (blow, engine_blow) = mech_mut(w, h).map_or((-1.0, -1.0), |m| {
            (m.center_torso_blow_time, m.engine_blow_time)
        });
        if scenario_time <= blow || (engine_blow > -1.0 && scenario_time <= engine_blow) {
            // Should explode...
            fx::handle_alarm(w, h, alarm::VEHICLE_INCAPACITATED, 0);
            handle_destruction(w, h, None);
        } else {
            // Should shutdown...
            disable(w, h, death::ENGINE);
        }
    } else if location == body::RTORSO {
        destroy_body_location(w, h, body::RARM);
    } else if location == body::LTORSO {
        destroy_body_location(w, h, body::LARM);
    } else if location == body::LARM {
        crate::events::emit_sound(w, h, crate::events::sfx::CRITICAL_HIT_SFX, true);
        if let Some(m) = mech_mut(w, h) {
            m.left_arm_blown = true;
        }
    } else if location == body::RARM {
        crate::events::emit_sound(w, h, crate::events::sfx::CRITICAL_HIT_SFX, true);
        if let Some(m) = mech_mut(w, h) {
            m.right_arm_blown = true;
        }
    }
}

/// `BattleMech::calcLegStatus` with the pilot-side effects applied.
fn leg_status(w: &mut World, h: ObjectHandle) {
    let has_pilot = fx::pilot_slot(w, h).is_some();
    let Some(m) = mech_mut(w, h) else { return };
    let effects = calc_leg_status(
        &m.loadout,
        &mut m.leg_status.0,
        &mut m.sent_crippled_msg,
        has_pilot,
        false,
    );
    fx::leg_effects(w, h, effects);
}

/// `BattleMech::calcTorsoStatus`.
fn torso_status(w: &mut World, h: ObjectHandle) {
    if let Some(m) = mech_mut(w, h) {
        m.loadout.stats.torso_status = calc_torso_status(&m.loadout);
    }
}

/// The number of critical hits a roll of 0..99 gives on `body_location` (`critical_hit_table` = {58, 83, 97, 100}): none below the
/// first entry, one below the second, two below the third, three on a torso location above that, otherwise the location is destroyed (0).
pub fn critical_hit_count(roll: i32, table: &[i8; 4], body_location: usize) -> i32 {
    if roll < i32::from(table[0]) {
        0
    } else if roll < i32::from(table[1]) {
        1
    } else if roll < i32::from(table[2]) {
        2
    } else if (body::CTORSO..=body::RTORSO).contains(&body_location) {
        3
    } else {
        0
    }
}

/// `BattleMech::calcCriticalHit(hitLocation)` (mech.cpp:7052): RNG `RandomNumber(100)` for the severity, then
/// `RandomNumber(totalSpaces)` per hit. `critical_hit_table` is `{58, 83, 97, 100}`: below the first nothing happens, below the
/// second one component is hit, below the third two; above that a torso takes three and any other location is destroyed.
pub fn calc_critical_hit(w: &mut World, h: ObjectHandle, hit_location: usize) {
    // Cannot critically hit nothing! (`body[hitLocation]` is only defined for the eight body locations: pinned, see the module docs.)
    let total_spaces = match w
        .objects
        .get(h)
        .and_then(loadout_of)
        .and_then(|l| l.body.get(hit_location))
    {
        Some(b) => b.total_spaces,
        None => return,
    };
    if total_spaces == 0 {
        return;
    }
    let table = w
        .data
        .gamesys
        .tables
        .as_ref()
        .map_or([58i8, 83, 97, 100], |t| t.mech.critical_hit_table);
    let body_location = ARMOR_TO_BODY_LOCATION[hit_location];
    let num_body_spaces = NUM_LOCATION_CRITICAL_SPACES[body_location];

    let critical_roll = rn(w, 100);
    let num_critical_hits = critical_hit_count(critical_roll, &table, body_location);
    if critical_roll < i32::from(table[0]) {
        return;
    }

    if num_critical_hits > 0 {
        for _ in 0..num_critical_hits {
            let spaces = w
                .objects
                .get(h)
                .and_then(loadout_of)
                .map_or(0, |l| l.body[body_location].total_spaces);
            let mut crit_hit_roll = rn(w, spaces);
            let comps = w.data.components.clone();
            let mut hit_space = 0;
            {
                let Some(l) = w.objects.get(h).and_then(loadout_of) else {
                    return;
                };
                while hit_space < num_body_spaces {
                    let inv_id = l.body[body_location].critical_spaces[hit_space].0;
                    let num_spaces = if inv_id < 255 && usize::from(inv_id) < l.inventory.len() {
                        comps
                            .as_ref()
                            .and_then(|c| comp(c, l.inventory[usize::from(inv_id)].master_id))
                            .map_or(0, |c| i32::from(c.size))
                    } else {
                        0
                    };
                    if crit_hit_roll < num_spaces {
                        break;
                    }
                    crit_hit_roll -= num_spaces;
                    hit_space += 1;
                }
            }
            // If we are out of range, no critical hit!
            if hit_space >= num_body_spaces {
                return;
            }
            let inv_id = {
                let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) else {
                    return;
                };
                l.body[body_location].critical_spaces[hit_space].1 = true;
                l.body[body_location].critical_spaces[hit_space].0
            };
            hit_inventory_item(w, h, usize::from(inv_id), false);
        }
    } else {
        destroy_body_location(w, h, body_location);
    }
}

/// `BattleMech::injureBodyLocation(bodyLocation, damage)` (mech.cpp:7316): true when the location was destroyed.
pub fn injure_body_location(
    w: &mut World,
    h: ObjectHandle,
    body_location: usize,
    damage: f32,
) -> bool {
    let scenario_time = w.clock.scenario_time;
    let remaining = {
        let Some(m) = mech_mut(w, h) else {
            return false;
        };
        if body_location == body::CTORSO && m.center_torso_blow_time < 0.0 {
            m.center_torso_blow_time = scenario_time;
        }
        let loc = &mut m.loadout.body[body_location];
        if damage > loc.cur_internal_structure {
            loc.cur_internal_structure = 0.0;
        } else {
            loc.cur_internal_structure -= damage;
        }
        if loc.cur_internal_structure > 0.0 {
            let damage_level = loc.cur_internal_structure / f32::from(loc.max_internal_structure);
            loc.damage_state = if damage_level <= 0.5 {
                IS_DAMAGE_PARTIAL
            } else {
                IS_DAMAGE_NONE
            };
            true
        } else {
            false
        }
    };
    if remaining {
        if body_location == body::LLEG || body_location == body::RLEG {
            leg_status(w, h);
        } else if body_location == body::CTORSO {
            torso_status(w, h);
        }
        calc_critical_hit(w, h, body_location);
        // Location was not destroyed, so return false...
        false
    } else {
        destroy_body_location(w, h, body_location);
        true
    }
}

/// `GroundVehicle::injureBodyLocation` (gvehicl.cpp:4098): true when the location was destroyed.
fn injure_vehicle_location(w: &mut World, h: ObjectHandle, location: usize, damage: f32) -> bool {
    let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) else {
        return false;
    };
    let loc = &mut l.body[location];
    if damage >= loc.cur_internal_structure {
        loc.cur_internal_structure = 0.0;
        loc.damage_state = IS_DAMAGE_DESTROYED;
        return true;
    }
    loc.cur_internal_structure -= damage;
    let damage_level = loc.cur_internal_structure / f32::from(loc.max_internal_structure);
    loc.damage_state = if damage_level == 0.0 {
        IS_DAMAGE_DESTROYED
    } else if damage_level <= 0.5 {
        IS_DAMAGE_PARTIAL
    } else {
        IS_DAMAGE_NONE
    };
    false
}

// ---------------------------------------------------------------------------------------------------------------------------
// destruction

/// `BattleMechType::handleDestruction` / `GroundVehicleType::handleDestruction` (mech.cpp:994, gvehicl.cpp:521). `collider` is the
/// watch id of what destroyed it (0 for none). Returns true.
pub fn handle_destruction(
    w: &mut World,
    h: ObjectHandle,
    collider: Option<crate::ids::WatchId>,
) -> bool {
    let collider_wid = collider.map_or(0, |c| c.0);
    if is_mech(w, h) {
        // Immediately lose contacts...
        fx::sensor_disable(w, h, false);
        let (withdrawing, start_disabled) = mech_mut(w, h).map_or((false, false), |m| {
            (m.m.flags.withdrawing, m.m.flags.start_disabled)
        });
        if withdrawing {
            if let Some(m) = mech_mut(w, h) {
                m.m.time_left = 0.8;
            }
            // Let the pilot know we are about to withdraw...
            fx::handle_alarm(w, h, alarm::VEHICLE_WITHDRAWN, 0);
        } else {
            if !start_disabled {
                if let Some(m) = mech_mut(w, h) {
                    m.m.time_left = 0.8;
                }
            }
            // Let the pilot know we are about to die...
            fx::handle_alarm(w, h, alarm::VEHICLE_DESTROYED, collider_wid);
            let is_helicopter = mech_mut(w, h).is_some_and(|m| m.m.move_level == 2);
            if is_helicopter {
                crate::events::emit_sound(w, h, crate::events::sfx::HELICOPTER_DEATH, false);
            }
            set_status(w, h, ObjectStatus::Destroyed);
            // A dead mech still needs its appearance clock to select and finish the fall. The
            // original delays this handoff until after removal (`mech.cpp:6282-6283`), which
            // leaves the visible fall on the intact shape. The Rust port intentionally fixes
            // that player-visible defect; `MechAppearance` latches the shipped damage shape
            // when the chosen fall reaches its terminal gesture.
            w.appearance
                .set_obj_status(h, ObjectStatus::Destroyed as i32);
            if let Some(m) = mech_mut(w, h) {
                m.fallen = false;
                m.m.flags.exploding = false;
            }
            // Blow all of the components. NO SALVAGE!!
            for i in 0..8 {
                destroy_body_location(w, h, i);
            }
        }
    } else {
        // Immediately lose contacts...
        fx::sensor_disable(w, h, true);
        let withdrawing = w
            .objects
            .get(h)
            .and_then(core_of)
            .is_some_and(|c| c.flags.withdrawing);
        if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
            v.m.time_left = 0.0;
        }
        if withdrawing {
            // Let the pilot know we are about to withdraw...
            fx::trigger_alarm(w, h, alarm::VEHICLE_WITHDRAWN, 0);
        } else {
            // Let the pilot know we are about to die...
            fx::trigger_alarm(w, h, alarm::VEHICLE_DESTROYED, collider_wid as i32);
            set_status(w, h, ObjectStatus::Destroyed);
            w.appearance
                .set_obj_status(h, ObjectStatus::Destroyed as i32);
            crate::appearance::update(w, h, true);
            if let Some(Kind::Vehicle(v)) = w.objects.get_mut(h).map(|o| &mut o.kind) {
                v.m.flags.exploding = false;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------------------------------------------------------
// handleWeaponHit

/// `handleWeaponHit(shotInfo)` on a mech or ground vehicle. The C++ takes the shot by pointer and leaves the caller's copy holding
/// `init(startShotInfo)` (scaled a second time); this wrapper works on a copy, [`handle_weapon_hit_mut`] keeps the write-back.
pub fn handle_weapon_hit(w: &mut World, target: ObjectHandle, shot: &WeaponShotInfo) -> i32 {
    let mut s = *shot;
    handle_weapon_hit_mut(w, target, &mut s)
}

/// [`handle_weapon_hit`] with the in/out shot of the C++.
pub fn handle_weapon_hit_mut(
    w: &mut World,
    target: ObjectHandle,
    shot: &mut WeaponShotInfo,
) -> i32 {
    match w.objects.get(target).map(|o| &o.kind) {
        Some(Kind::Mech(_)) => mech_hit(w, target, shot),
        Some(Kind::Vehicle(_)) => vehicle_hit(w, target, shot),
        _ => 0,
    }
}

fn team_of(w: &World, h: ObjectHandle) -> i32 {
    w.objects.get(h).map_or(-1, |o| o.team_id())
}

/// The transfer / vent target of an ammunition explosion that CASE holds in (mech.cpp:7651).
fn case_vent_location(body_location: usize) -> usize {
    match body_location {
        body::LTORSO | body::LARM | body::LLEG => armor_loc::RLTORSO,
        body::RTORSO | body::RARM | body::RLEG => armor_loc::RRTORSO,
        _ => armor_loc::RCTORSO,
    }
}

fn mech_hit(w: &mut World, h: ObjectHandle, shot: &mut WeaponShotInfo) -> i32 {
    let home_team = w.teams.home as i32;
    let local_invulnerable = w.mission.invulnerable && team_of(w, h) == home_team;

    // Team::noPain is a cheat code's flag (mechcmd2.cpp:2587): never set in the port.
    if local_invulnerable {
        return 0;
    }
    let attacker = w.objects.by_watch_id(shot.attacker);

    // Is this possible? Just in case...
    if shot.damage <= 0.0 || shot.hit_location == -1 {
        return 0;
    }
    // The armor tables hold eleven locations; anything else reads outside them in the C++.
    if !(0..11).contains(&shot.hit_location) {
        return 0;
    }
    // Possibly someone nails a mech after it's been destroyed, but before it's deleted from the object list...
    if w.objects.get(h).is_some_and(|o| o.c.is_destroyed()) {
        return 0;
    }
    if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
        l.stats.num_weapon_hits_handled += 1;
    }

    let disabled = is_disabled(w, h);
    let fallen = mech_mut(w, h).is_some_and(|m| m.fallen);
    if disabled && fallen {
        // A grounded, disabled mech: damage piles up until it blows for real.
        let move_level = w
            .objects
            .get(h)
            .and_then(core_of)
            .map_or(0, |c| c.move_level);
        if move_level != 2 {
            let destruct = w
                .objects
                .get(h)
                .map(|o| o.c.type_handle)
                .and_then(|t| w.data.object_types.as_ref().and_then(|ot| ot.get(t as u32)))
                .and_then(|t| match &t.data {
                    ObjectTypeData::BattleMech(m) => Some(m.destruct_damage),
                    _ => None,
                })
                .unwrap_or(20.0);
            let total = {
                let l = w
                    .objects
                    .get_mut(h)
                    .and_then(|o| loadout_of_mut(o))
                    .expect("mech");
                l.stats.damage_after_disabled += shot.damage;
                l.stats.damage_after_disabled
            };
            if total > destruct {
                if let Some(m) = mech_mut(w, h) {
                    m.m.time_left = 1.0;
                }
                set_status(w, h, ObjectStatus::Destroyed);
                if let Some(m) = mech_mut(w, h) {
                    m.m.flags.exploding = false;
                }
                // Blow all of the components. NO SALVAGE!!
                for i in 0..8 {
                    destroy_body_location(w, h, i);
                }
            }
        } else {
            // It's a helicopter. Blow it up. No salvage.
            set_status(w, h, ObjectStatus::Destroyed);
            if let Some(m) = mech_mut(w, h) {
                m.m.flags.exploding = false;
            }
            for i in 0..8 {
                destroy_body_location(w, h, i);
            }
        }
        return 0;
    }

    // Preserve the shot and restore it at the end of this function!
    let start_shot_info = *shot;
    let already_disabled = is_disabled(w, h);

    let comps = w.data.components.clone();
    let tables = w.data.gamesys.tables.clone();
    let mut is_ammo_explosion = shot.master_id == -4;
    if shot.master_id > 0 {
        is_ammo_explosion = form_of(comps.as_ref().and_then(|c| c.get(shot.master_id)))
            == Some(ComponentForm::Ammo);
    }

    // Tally the damage for rate check and pilot check...
    // Did the hit cause us to stumble?
    let torso_rotation = mech_mut(w, h).map_or(0.0, |m| m.torso_rotation);
    let torso_entry_angle = shot.entry_angle + torso_rotation;
    let hit_level = tables.as_ref().map_or([0, 0], |t| t.mover_damage.hit_level);
    // (The C++ also classifies the single shot, `hitType`, for a switch on the gyro whose cases are empty.)
    let total_hit_type;
    {
        let m = mech_mut(w, h).expect("mech");
        m.loadout.stats.damage_rate_tally += shot.damage;
        m.loadout.stats.pilot_check_damage_tally += shot.damage;
        if (-90.0..=90.0).contains(&torso_entry_angle) {
            m.hit_from_front = true;
        } else if torso_entry_angle <= -91.0 || torso_entry_angle >= 91.0 {
            m.hit_from_behind = true;
        }
        // First, what type of hit is this...
        m.loadout.stats.damage_this_frame += shot.damage;
        let dtf = m.loadout.stats.damage_this_frame;
        total_hit_type = if dtf < hit_level[0] as f32 {
            HIT_TYPE_WEAK
        } else if dtf < hit_level[1] as f32 {
            HIT_TYPE_MODERATE
        } else {
            2
        };
    }

    // If this was a heavy hit, play the hit gesture.
    let commander = w.objects.get(h).map_or(-1, |o| o.commander_id());
    if total_hit_type == HIT_TYPE_MODERATE && commander != HOME_COMMANDER {
        use crate::appearance::HitDirection;
        if (-45.0..=45.0).contains(&torso_entry_angle) {
            w.appearance.hit(h, HitDirection::Front);
        } else if (-135.0..=-45.0).contains(&torso_entry_angle) {
            w.appearance.hit(h, HitDirection::Left);
        } else if (45.0..=135.0).contains(&torso_entry_angle) {
            w.appearance.hit(h, HitDirection::Right);
        } else if torso_entry_angle <= -135.0 || torso_entry_angle >= 135.0 {
            w.appearance.hit(h, HitDirection::Back);
        }
    }

    // Note that the hitLocation is the ARMOR hit location...
    let hit_location = shot.hit_location as usize;
    let body_location = ARMOR_TO_BODY_LOCATION[hit_location];

    // Check for any special pilot injuries...
    if body_location == body::HEAD && shot.damage >= 2.0 {
        fx::injure(w, h, 1.0, true);
    }

    let cur_armor = |w: &World| {
        w.objects
            .get(h)
            .and_then(loadout_of)
            .map_or(0.0, |l| l.armor[hit_location].0)
    };
    let cur_is = |w: &World| {
        w.objects
            .get(h)
            .and_then(loadout_of)
            .map_or(0.0, |l| l.body[body_location].cur_internal_structure)
    };

    if cur_armor(w) > 0.0 && !is_ammo_explosion {
        let armor_now = cur_armor(w);
        if shot.damage > armor_now {
            shot.set_damage(shot.damage - armor_now);
            if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                l.armor[hit_location].0 = 0.0;
            }
            fx::radio_message(w, h, fx::RADIO_ARMORHOLE);
            // (IN the tutorial missions, your mechs can only have armor damaged: NO INTERNALS ALLOWED, via `localInvulnerable`.)
            let is_now = cur_is(w);
            if shot.damage >= is_now {
                shot.set_damage(shot.damage - is_now);
                injure_body_location(w, h, body_location, is_now);
                if shot.damage > 0.0 {
                    if body_location == body::CTORSO || body_location == body::HEAD {
                        // If hit in the center torso, no where else to go... If hit in the head, it is disabled.
                    } else {
                        let mut new_shot = *shot;
                        new_shot.hit_location = transfer_hit_location(w, body_location);
                        mech_hit(w, h, &mut new_shot);
                    }
                }
            } else {
                injure_body_location(w, h, body_location, shot.damage);
            }
        } else if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
            l.armor[hit_location].0 -= shot.damage;
        }
    } else {
        let is_now = cur_is(w);
        let case = w
            .objects
            .get(h)
            .and_then(loadout_of)
            .is_some_and(|l| l.body[body_location].case);
        if is_now > 0.0 {
            if shot.damage >= is_now {
                shot.set_damage(shot.damage - is_now);
                injure_body_location(w, h, body_location, is_now);
                if shot.damage > 0.0 {
                    if is_ammo_explosion && case {
                        // Have CASE here, so vent the excess damage...
                        vent_case(w, h, body_location, shot);
                    } else if body_location == body::CTORSO || body_location == body::HEAD {
                        // Hit center torso, already, or head wiped out... what to do?
                    } else {
                        let mut new_shot = *shot;
                        new_shot.hit_location = transfer_hit_location(w, body_location);
                        mech_hit(w, h, &mut new_shot);
                    }
                }
            } else {
                injure_body_location(w, h, body_location, shot.damage);
            }
        } else if is_ammo_explosion && case {
            // Have CASE here, so vent the excess damage...
            vent_case(w, h, body_location, shot);
        } else if body_location == body::CTORSO || body_location == body::HEAD {
            // Already in center torso, or head wiped out... what to do?
        } else {
            let mut new_shot = *shot;
            new_shot.hit_location = transfer_hit_location(w, body_location);
            mech_hit(w, h, &mut new_shot);
        }
    }

    // (The gyro-disabled effects of the C++ are empty switch cases.)

    // Let anyone who should know that I was hit... Trigger the WEAPON HIT event...
    match attacker {
        Some(a) => {
            let wid = w.objects.watch_id(a).0 as i32;
            if shot.master_id > -1 || shot.master_id == -4 {
                fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, wid);
            } else {
                fx::trigger_alarm(w, h, alarm::COLLISION, wid);
            }
        }
        None => {
            if shot.master_id == -4 {
                fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, 0);
            } else if shot.master_id < 0 {
                // If no attacker and a negative weapon masterId, then it must be a mine or artillery...
                fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, shot.master_id);
            } else {
                fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, 0);
            }
        }
    }

    // Calc current CV, after this damage taken...
    if let (Some(comps), Some(o)) = (comps.as_ref(), w.objects.get_mut(h)) {
        let cv = loadout_of(o).map_or(0, |l| l.calc_cv_mech(comps, false));
        o.c.cur_cv = cv as i16;
        o.c.threat_rating = if o.c.is_disabled() {
            1
        } else {
            threat_rating_for(-1, cv)
        };
    }

    // If this mech was destroyed, let the attacker know...
    if !already_disabled && is_disabled(w, h) {
        if let Some(a) = attacker {
            if w.objects.get(a).is_some_and(|o| o.is_mover()) {
                let me = w.objects.watch_id(h).0 as i32;
                fx::trigger_alarm(w, a, alarm::KILLED_TARGET, me);
            }
        }
    }

    shot.assign(w, start_shot_info);
    0
}

fn vent_case(w: &mut World, h: ObjectHandle, body_location: usize, shot: &mut WeaponShotInfo) {
    let al = case_vent_location(body_location);
    if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
        if shot.damage > l.armor[al].0 {
            l.armor[al].0 = 0.0;
        } else {
            l.armor[al].0 -= shot.damage;
        }
    }
    shot.set_damage(0.0);
}

fn vehicle_hit(w: &mut World, h: ObjectHandle, shot: &mut WeaponShotInfo) -> i32 {
    // Team::noPain: never set. `CantTouchThis`: a debug flag, false.
    if shot.damage <= 0.0 {
        return 0;
    }
    if w.objects.get(h).is_some_and(|o| o.c.is_destroyed()) {
        return 0;
    }
    if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
        l.stats.num_weapon_hits_handled += 1;
    }
    // Preserve the shot and restore it at the end of this function!
    let start_shot_info = *shot;
    let attacker = w.objects.by_watch_id(shot.attacker);

    // (A location outside the five is a `Fatal` in the C++.)
    if !(0..5).contains(&shot.hit_location) {
        return -1;
    }
    let loc = shot.hit_location as usize;

    // Does it get past the armor?
    let armor_now = w
        .objects
        .get(h)
        .and_then(loadout_of)
        .map_or(0.0, |l| l.armor[loc].0);
    if armor_now > 0.0 {
        if shot.damage > armor_now {
            shot.set_damage(shot.damage - armor_now);
            if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                l.armor[loc].0 = 0.0;
            }
        } else {
            if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                l.armor[loc].0 -= shot.damage;
            }
            shot.set_damage(0.0);
        }
        // Make sure, in case it's a minesweeper, it ain't no more :)
        if loc == GV_FRONT {
            if let Some(l) = w.objects.get_mut(h).and_then(|o| loadout_of_mut(o)) {
                l.stats.mine_sweeper = false;
            }
        }
    }

    // If any damage gets thru, check internal structure...
    let vehicle_already_disabled = is_disabled(w, h);
    let mut vehicle_destroyed = false;
    if shot.damage > 0.0 {
        vehicle_destroyed = injure_vehicle_location(w, h, loc, shot.damage);
    }

    if vehicle_destroyed {
        fx::own_vehicle_incapacitation(w, h, 0);
        handle_destruction(w, h, None);
        if let Some(o) = w.objects.get_mut(h) {
            o.c.set_flag(flags::TANGIBLE, false);
        }
    }

    // Calc current CV, after this damage taken...
    let (comps, gs) = (w.data.components.clone(), w.data.gamesys.tables.clone());
    if let (Some(comps), Some(gs)) = (comps.as_ref(), gs.as_ref()) {
        if let Some(o) = w.objects.get_mut(h) {
            let tonnage = o.c.tonnage;
            let speed = core_of(o).map_or(0.0, |c| c.max_move_speed);
            let cv =
                loadout_of(o).map_or(0, |l| l.calc_cv_vehicle(comps, gs, tonnage, speed, false));
            o.c.cur_cv = cv as i16;
            o.c.threat_rating = if o.c.is_disabled() {
                1
            } else {
                threat_rating_for(-1, cv)
            };
        }
    }

    if !vehicle_already_disabled && is_disabled(w, h) {
        // If this vehicle was destroyed, let the attacker know...
        if let Some(a) = attacker {
            if w.objects.get(a).is_some_and(|o| o.is_mover()) {
                let me = w.objects.watch_id(h).0 as i32;
                fx::trigger_alarm(w, a, alarm::KILLED_TARGET, me);
            }
        }
    } else {
        // Trigger the WEAPON HIT event...
        match attacker {
            Some(a) => {
                let wid = w.objects.watch_id(a).0 as i32;
                if shot.master_id > -1 || shot.master_id == -4 {
                    fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, wid);
                } else {
                    fx::trigger_alarm(w, h, alarm::COLLISION, wid);
                }
            }
            None => {
                // If it was -5, it was a repair vehicle burning refit points: do not say you are under fire.
                if shot.master_id != -5 {
                    if shot.master_id == -4 {
                        fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, 0);
                    } else if shot.master_id < 0 {
                        fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, shot.master_id);
                    } else {
                        fx::trigger_alarm(w, h, alarm::HIT_BY_WEAPONFIRE, 0);
                    }
                }
            }
        }
    }

    if shot.master_id == -2 && loc == GV_TURRET {
        let (left, commander) = {
            let o = w.objects.get_mut(h).expect("vehicle");
            let c = o.commander_id();
            let l = loadout_of_mut(o).expect("vehicle");
            l.stats.mine_layer -= 1;
            (l.stats.mine_layer, c)
        };
        // NOT ANY MORE. Minelayer still active. (Change per design meeting 4/23/01.)
        if left == 0 && commander == HOME_COMMANDER {
            // `soundSystem->playBettySample(BETTY_MINES_GONE)` (gvehicl.cpp:4486)
            w.events
                .push(crate::SimEvent::Betty(BETTY_MINES_GONE as u32));
        }
    }

    if shot.master_id == -5 {
        // getRefitPoints(): the turret armor of a vehicle that can refit
        let can_refit = w
            .objects
            .get(h)
            .map(|o| o.c.type_handle)
            .and_then(|t| w.data.object_types.as_ref().and_then(|ot| ot.get(t as u32)))
            .is_some_and(
                |t| matches!(&t.data, ObjectTypeData::GroundVehicle(v) if v.refit_points > 0),
            );
        let points = if can_refit {
            w.objects
                .get(h)
                .and_then(loadout_of)
                .map_or(0.0, |l| l.armor[GV_TURRET].0)
        } else {
            0.0
        };
        if points == 0.0 {
            disable(w, h, death::POWER_USED_UP);
        }
    }

    shot.assign(w, start_shot_info);
    0
}

#[cfg(test)]
mod crit_table_tests {
    use super::*;

    const TABLE: [i8; 4] = [58, 83, 97, 100];

    #[test]
    fn roll_boundaries_of_the_critical_table() {
        let torso = body::CTORSO;
        let arm = body::LARM;
        for (roll, torso_hits, arm_hits) in [
            (0, 0, 0),
            (57, 0, 0),
            (58, 1, 1),
            (82, 1, 1),
            (83, 2, 2),
            (96, 2, 2),
            (97, 3, 0),
            (99, 3, 0),
        ] {
            assert_eq!(
                critical_hit_count(roll, &TABLE, torso),
                torso_hits,
                "torso roll {roll}"
            );
            assert_eq!(
                critical_hit_count(roll, &TABLE, arm),
                arm_hits,
                "arm roll {roll}"
            );
        }
    }
}
