// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Combat set-up of a mover: ammo totals, fire ranges, combat value, threat rating, weapon effectiveness
//! (`Mover::calcAmmoTotals` mover.cpp:5765, `calcFireRanges` 5842, `calcWeaponEffectiveness` 5730, `BattleMech::calcCV` mech.cpp:3428,
//! `GroundVehicle::calcCV` gvehicl.cpp:1266, `calcMaxTargetDamage` mech.cpp:8900, `calcLegStatus`/`calcTorsoStatus` 3446/3486,
//! `setThreatRating` mover.cpp:3938) and the two cross-lane entry points lane E calls: [`init_mover_stats`] (the tail of
//! `BattleMech::init(variant)` mech.cpp:1700-1960, `GroundVehicle::init` gvehicl.cpp:1160-1240 and the
//! `calcWeaponEffectiveness` pair of `Mission::createPartObject`/`addMover`) and [`reset_components`] (`BattleMech::resetComponents`
//! mech.cpp:2065).

use crate::combat::{
    fx, loadout_of, loadout_of_mut, InventoryItem, Loadout, MAX_MOVER_INVENTORY_ITEMS,
    UNLIMITED_SHOTS,
};
use crate::ids::ObjectHandle;
use crate::object::Kind;
use mc2_sim_data::component::{
    ComponentForm, ComponentStats, MasterComponent, MasterComponentList, WeaponAmmoType,
    WeaponStats,
};
use mc2_sim_data::gamesys::GameSystem;
use mc2_sim_data::mech::layout::{NUM_ARMOR_LOCATIONS, NUM_BODY_LOCATIONS};

/// `NUM_WEAPON_RANGE_TYPES`.
const NUM_WEAPON_RANGE_TYPES: usize = 5;
/// `OptimalRangeTieTable` (mover.cpp:328).
const OPTIMAL_RANGE_TIE_TABLE: [u8; 32] = [
    255, 0, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4,
    4,
];
/// `MECH_CLASS_*` (mechclass.h).
pub const MECH_CLASS_NONE: u8 = 0;
/// See [`MECH_CLASS_NONE`].
pub const MECH_CLASS_LIGHT: u8 = 1;
/// See [`MECH_CLASS_NONE`].
pub const MECH_CLASS_MEDIUM: u8 = 2;
/// See [`MECH_CLASS_NONE`].
pub const MECH_CLASS_HEAVY: u8 = 3;
/// See [`MECH_CLASS_NONE`].
pub const MECH_CLASS_ASSAULT: u8 = 4;

/// `LEG_STATUS_*` (mech.h:176).
pub const LEG_STATUS_NORMAL: u8 = 0;
/// See [`LEG_STATUS_NORMAL`].
pub const LEG_STATUS_IMPAIRED_LEFT: u8 = 2;
/// See [`LEG_STATUS_NORMAL`].
pub const LEG_STATUS_IMPAIRED_RIGHT: u8 = 3;
/// See [`LEG_STATUS_NORMAL`].
pub const LEG_STATUS_DESTROYED: u8 = 4;
/// `TORSO_STATUS_*` (mech.h:185).
pub const TORSO_STATUS_NORMAL: u8 = 0;
/// See [`TORSO_STATUS_NORMAL`].
pub const TORSO_STATUS_IMPAIRED: u8 = 1;

/// `IS_DAMAGE_*` (mover.h:105).
pub const IS_DAMAGE_NONE: u8 = 0;
/// See [`IS_DAMAGE_NONE`].
pub const IS_DAMAGE_PARTIAL: u8 = 1;
/// See [`IS_DAMAGE_NONE`].
pub const IS_DAMAGE_DESTROYED: u8 = 2;

/// `MECH_BODY_LOCATION_*` (mech.h:147).
pub mod body {
    /// Head.
    pub const HEAD: usize = 0;
    /// Center torso.
    pub const CTORSO: usize = 1;
    /// Left torso.
    pub const LTORSO: usize = 2;
    /// Right torso.
    pub const RTORSO: usize = 3;
    /// Left arm.
    pub const LARM: usize = 4;
    /// Right arm.
    pub const RARM: usize = 5;
    /// Left leg.
    pub const LLEG: usize = 6;
    /// Right leg.
    pub const RLEG: usize = 7;
}

/// `MECH_ARMOR_LOCATION_*` for the three rear sections (mech.h:160).
pub mod armor {
    /// Rear center torso.
    pub const RCTORSO: usize = 8;
    /// Rear left torso.
    pub const RLTORSO: usize = 9;
    /// Rear right torso.
    pub const RRTORSO: usize = 10;
}

/// `masterList[id]`: `None` for an id the list does not hold (the C++ would read past the table).
pub(crate) fn comp(comps: &MasterComponentList, id: u8) -> Option<&MasterComponent> {
    comps.get(i32::from(id))
}

/// The weapon half of a component (`getWeaponDamage()` and friends read the union without checking the form).
pub(crate) fn weapon_stats(c: Option<&MasterComponent>) -> Option<&WeaponStats> {
    c.and_then(|c| c.weapon())
}

/// `getForm()`.
pub(crate) fn form_of(c: Option<&MasterComponent>) -> Option<ComponentForm> {
    c.and_then(|c| c.form)
}

/// `getWeaponRange()` as an index into `WeaponRanges`. A weapon whose range column is 0 has the range type 255 in the C++ and indexes
/// the table out of bounds: pinned to the first entry (no shipped weapon is affected).
fn range_index(w: &WeaponStats) -> usize {
    w.range.map_or(0, |r| r.index())
}

/// `getWeaponAmmoType() != WEAPON_AMMO_NONE`.
fn uses_ammo(w: &WeaponStats) -> bool {
    w.ammo_type != WeaponAmmoType::Unlimited
}

impl Loadout {
    /// `Mover::isWeaponIndex`.
    pub fn is_weapon_index(&self, item: i64) -> bool {
        item >= self.num_other() as i64 && item < (self.num_other() + self.num_weapons()) as i64
    }

    /// `Mover::getWeaponShots` (mover.cpp:5660): -1 for a non-weapon, the tally of the ammo type for an ammo weapon, otherwise
    /// `UNLIMITED_SHOTS`. An ammo weapon whose `ammoIndex` is not a valid type reads outside `ammoTypeTotal` in the C++: pinned to 0.
    pub fn weapon_shots(&self, comps: &MasterComponentList, item: i64) -> i32 {
        if !self.is_weapon_index(item) {
            return -1;
        }
        let it = &self.inventory[item as usize];
        if weapon_stats(comp(comps, it.master_id)).is_some_and(uses_ammo) {
            return usize::try_from(it.ammo_index)
                .ok()
                .and_then(|i| self.ammo_totals.get(i))
                .map_or(0, |t| t.1);
        }
        UNLIMITED_SHOTS
    }

    /// `Mover::calcAmmoTotals` (mover.cpp:5765). `Err` is the `STOP` for ammo of a weapon the mover does not carry.
    pub fn calc_ammo_totals(
        &mut self,
        comps: &MasterComponentList,
        use_unlimited_ammo: bool,
    ) -> Result<(), String> {
        self.ammo_totals.clear();
        if self.num_weapons() == 0 {
            return Ok(());
        }
        let first_weapon = self.num_other();
        let first_ammo = first_weapon + self.num_weapons();
        // First, a list of all ammo types of the weapons...
        let mut totals: Vec<(i32, i32, i32)> = Vec::new();
        for i in first_weapon..first_ammo {
            let w = weapon_stats(comp(comps, self.inventory[i].master_id));
            let ammo_master = w.map_or(0, |w| i32::from(w.ammo_master_id));
            if !totals.iter().any(|t| t.0 == ammo_master) {
                if w.is_some_and(uses_ammo) {
                    totals.push((ammo_master, 0, 0));
                } else {
                    totals.push((ammo_master, UNLIMITED_SHOTS, UNLIMITED_SHOTS));
                }
            }
        }
        // ...then every ammo item is tallied into its type.
        for i in first_ammo..first_ammo + self.num_ammos() {
            let ammo_master = i32::from(self.inventory[i].master_id);
            let Some(t) = totals.iter_mut().find(|t| t.0 == ammo_master) else {
                return Err(format!(
                    "Mech has ammo for a weapon it is not carrying!  Ammo Index is {ammo_master}"
                ));
            };
            if use_unlimited_ammo {
                t.1 = UNLIMITED_SHOTS;
                t.2 = UNLIMITED_SHOTS;
            } else {
                t.1 += i32::from(self.inventory[i].amount);
                t.2 += i32::from(self.inventory[i].amount);
            }
        }
        self.ammo_totals = totals;
        Ok(())
    }

    /// The `ammoIndex` links of `BattleMech::init` (mech.cpp:1846-1883): weapons and ammo point at their ammo type, the anti-missile
    /// systems among the other items too.
    pub fn link_ammo_indices(&mut self, comps: &MasterComponentList, mech: bool) {
        let first_weapon = self.num_other();
        let first_ammo = first_weapon + self.num_weapons();
        for item in first_weapon..first_ammo {
            let ammo_master = weapon_stats(comp(comps, self.inventory[item].master_id))
                .map_or(0, |w| i32::from(w.ammo_master_id));
            if let Some(i) = self.ammo_totals.iter().position(|t| t.0 == ammo_master) {
                self.inventory[item].ammo_index = i as i16;
            }
        }
        if !mech {
            // GroundVehicle::init (gvehicl.cpp:1217) links the weapons only; its ammo items keep -1
            return;
        }
        for item in first_ammo..first_ammo + self.num_ammos() {
            let master = i32::from(self.inventory[item].master_id);
            if let Some(i) = self.ammo_totals.iter().position(|t| t.0 == master) {
                self.inventory[item].ammo_index = i as i16;
            }
        }
        let special = comps.special();
        for item in 0..self.num_other() {
            let master = i32::from(self.inventory[item].master_id);
            if master == special.clan_anti_missile || master == special.inner_sphere_anti_missile {
                let ammo_master = weapon_stats(comp(comps, self.inventory[item].master_id))
                    .map_or(0, |w| i32::from(w.ammo_master_id));
                if let Some(i) = self.ammo_totals.iter().position(|t| t.0 == ammo_master) {
                    self.inventory[item].ammo_index = i as i16;
                }
            }
        }
    }

    /// `Mover::calcFireRanges` (mover.cpp:5842): min, max and optimal range. Returns whether the optimal range changed.
    pub fn calc_fire_ranges(
        &mut self,
        comps: &MasterComponentList,
        gs: &GameSystem,
        scenario_time: f32,
    ) -> bool {
        self.stats.last_optimal_range_calc = scenario_time;
        let first = self.num_other();
        let end = first + self.num_weapons();

        // Calc min and max ranges...
        self.stats.max_range = 0.0;
        self.stats.min_range = 1_000_000.0;
        self.stats.num_functional_weapons = 0;
        for cur in first..end {
            if !self.inventory[cur].disabled && self.weapon_shots(comps, cur as i64) > 0 {
                let range =
                    weapon_stats(comp(comps, self.inventory[cur].master_id)).map_or(0, range_index);
                let min_weapon_range = gs.general.weapon_ranges[range][0];
                let max_weapon_range = gs.general.weapon_ranges[range][1];
                if max_weapon_range > self.stats.max_range {
                    self.stats.max_range = max_weapon_range;
                }
                if min_weapon_range < self.stats.min_range {
                    self.stats.min_range = min_weapon_range;
                }
                self.stats.num_functional_weapons += 1;
            }
        }

        // Now, calc optimal range...
        let mut range_totals = [0.0f32; NUM_WEAPON_RANGE_TYPES];
        for cur in first..end {
            let weapon = weapon_stats(comp(comps, self.inventory[cur].master_id));
            if !self.inventory[cur].disabled && self.weapon_shots(comps, cur as i64) > 0 {
                let Some(weapon) = weapon else { continue };
                let damage_time_rating = weapon.damage / weapon.recycle_time;
                // Go thru the five range points...
                for (i, total) in range_totals.iter_mut().enumerate() {
                    if gs.general.optimal_range_point_in_range[i][range_index(weapon)] {
                        *total += damage_time_rating;
                    }
                }
            }
        }
        let last_optimal_range = self.stats.optimal_range;
        if self.stats.num_functional_weapons == 0 {
            self.stats.optimal_range = 0.0;
            return self.stats.optimal_range != last_optimal_range;
        }
        // `Mover::sortList` (4000 nodes cleared to -3.4E38; the five range totals sorted in descending order with the stable sort
        // of DESIGN.md section 4; the unused tail sorts behind them).
        let mut sorted: [(usize, f32); NUM_WEAPON_RANGE_TYPES] =
            std::array::from_fn(|i| (i, range_totals[i]));
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        if sorted[0].1 <= 0.0 {
            self.stats.optimal_range = 0.0;
            return self.stats.optimal_range != last_optimal_range;
        }

        let mut optimal_range_type = sorted[0].0;
        if sorted[0].1 == sorted[1].1 {
            // A tie, so pick one based upon the pre-set range priorities...
            let mut tie_flags: u8 = 0;
            for s in &sorted {
                if s.1 == sorted[0].1 {
                    tie_flags |= 1 << s.0;
                }
            }
            let t = OPTIMAL_RANGE_TIE_TABLE[usize::from(tie_flags)];
            if t == 255 {
                self.stats.optimal_range = 0.0;
                return self.stats.optimal_range != last_optimal_range;
            }
            optimal_range_type = usize::from(t);
        }
        self.stats.optimal_range = gs.general.optimal_range_points[optimal_range_type];
        self.stats.optimal_range != last_optimal_range
    }

    /// `Mover::calcWeaponEffectiveness(setMax)` (mover.cpp:5730). `gunnery` is the pilot's `getSkill(MWS_GUNNERY)`, `None` without a
    /// pilot. Returns true when the effectiveness fell to 0 (the C++ plays `RADIO_WEAPONS_OUT`).
    pub fn calc_weapon_effectiveness(
        &mut self,
        gunnery: Option<i32>,
        scenario_time: f32,
        set_max: bool,
    ) -> bool {
        let mut effectiveness: i64 = 0;
        // Record time of our latest calc...
        self.stats.last_weapon_effectiveness_calc = scenario_time;
        // `(float)skill / 50.0` is a double division narrowed into the float.
        let avg_skill: f32 = match gunnery {
            Some(g) => ((g as f32) as f64 / 50.0) as f32,
            None => 1.0,
        };
        let first = self.num_other();
        for cur in first..first + self.num_weapons() {
            if set_max || !self.inventory[cur].disabled {
                // long += short * float: evaluated in float, truncated back into the long
                effectiveness = (effectiveness as f32
                    + f32::from(self.inventory[cur].effectiveness) * avg_skill)
                    as i64;
            }
        }
        if set_max {
            self.stats.max_weapon_effectiveness = effectiveness as f32;
            false
        } else {
            self.stats.weapon_effectiveness = effectiveness as f32;
            effectiveness == 0
        }
    }

    /// `BattleMech::calcCV(calcMax)` (mech.cpp:3428).
    pub fn calc_cv_mech(&self, comps: &MasterComponentList, calc_max: bool) -> i32 {
        let mut total_cv = self.stats.chassis_br as f32;
        for item in 0..self.num_items() {
            if calc_max || !self.inventory[item].disabled {
                total_cv +=
                    comp(comps, self.inventory[item].master_id).map_or(0.0, |c| c.battle_rating);
            }
        }
        total_cv as i32
    }

    /// `GroundVehicle::calcCV(calcMax)` (gvehicl.cpp:1266). `tonnage` is `getTonnage()`, `max_move_speed` the vehicle's.
    pub fn calc_cv_vehicle(
        &self,
        comps: &MasterComponentList,
        gs: &GameSystem,
        tonnage: f32,
        max_move_speed: f32,
        calc_max: bool,
    ) -> i32 {
        if self.stats.battle_rating != -1 {
            return self.stats.battle_rating;
        }
        // First, total the CV's of all weapons...
        let mut weapon_br = 0.0f32;
        let first = self.num_other();
        for w in first..first + self.num_weapons() {
            if calc_max || !self.inventory[w].disabled {
                weapon_br +=
                    comp(comps, self.inventory[w].master_id).map_or(0.0, |c| c.battle_rating);
            }
        }
        // Now, multiply weapon BR by the speed factor (a standard rate of 18m/s); `(1.0 + (x - 18.0) / 18.0)` is evaluated in
        // double and the product narrowed.
        let speed_factor = 1.0 + (f64::from(max_move_speed) - 18.0) / 18.0;
        weapon_br = (f64::from(weapon_br) * speed_factor) as f32;

        // Now, calc the defensive BR...
        let mut defensive_br = 0.0f32;
        // IS Factor...
        for b in self.body.iter().take(5) {
            defensive_br += if calc_max {
                f32::from(b.max_internal_structure)
            } else {
                b.cur_internal_structure
            };
        }
        // Armor Factor...
        for a in self.armor.iter().take(5) {
            defensive_br += if calc_max { f32::from(a.1) } else { a.0 };
        }
        // Add Tonnage Factor...
        defensive_br += tonnage;
        // Movement Factor...
        let table = &gs.mech.target_move_modifier_table;
        let mut i = 0;
        while i < 5 {
            if max_move_speed <= table[i][0] as f32 {
                break;
            }
            i += 1;
        }
        // reading row 5 of a 5-row table is out of range in the C++: a vehicle faster than every row reads the next global
        let row = table.get(i).map_or(0, |r| r[1]);
        defensive_br += (row * 10) as f32;
        // Any other special Components..?
        for item in 0..self.num_other() {
            if calc_max || !self.inventory[item].disabled {
                defensive_br +=
                    comp(comps, self.inventory[item].master_id).map_or(0.0, |c| c.battle_rating);
            }
        }
        (weapon_br + defensive_br) as i32
    }

    /// `BattleMech::calcMaxTargetDamage` (mech.cpp:8900).
    pub fn calc_max_target_damage(&self, comps: &MasterComponentList, gs: &GameSystem) -> f32 {
        let mut damage = 0.0f32;
        let first = self.num_other();
        for cur in first..first + self.num_weapons() {
            let c = comp(comps, self.inventory[cur].master_id);
            let w = weapon_stats(c);
            let mut num_clusters: i64 = 1;
            let mut cluster_size: i64 = 1;
            if form_of(c) == Some(ComponentForm::WeaponMissile) {
                // It's a missile weapon. We need to determine how many hit (and missed) the target, and in how many clusters...
                let num_missiles = i64::from(w.map_or(0, |w| w.ammo_amount) / 2);
                let (size, ok) = match w.map(|w| w.ammo_type) {
                    Some(WeaponAmmoType::Srm) => {
                        (i64::from(gs.mover_damage.cluster_size_srm), true)
                    }
                    Some(WeaponAmmoType::Lrm) => {
                        (i64::from(gs.mover_damage.cluster_size_lrm), true)
                    }
                    _ => (1, false),
                };
                if ok {
                    num_clusters = num_missiles / size;
                    if num_missiles % size != 0 {
                        num_clusters += 1;
                    }
                    cluster_size = size;
                }
            }
            let (dmg, recycle) = w.map_or((0.0, 1.0), |w| (w.damage, w.recycle_time));
            // long * long * float * int: float arithmetic, left to right
            let mut this_damage = (cluster_size * num_clusters) as f32 * dmg * 10.0;
            this_damage /= recycle;
            this_damage *= 100.0;
            if this_damage > 0.0 {
                damage += this_damage;
            }
        }
        damage
    }

    /// `inventory[].effectiveness` of a weapon (mech.cpp:1621, gvehicl.cpp:1136): damage over ten seconds scaled by its longest range.
    pub fn weapon_item_effectiveness(w: &WeaponStats, gs: &GameSystem) -> i16 {
        // (short)(damage * 10.0 / recycle): double arithmetic, truncated
        let mut eff = (f64::from(w.damage) * 10.0 / f64::from(w.recycle_time)) as i16;
        // `effectiveness *= WeaponRanges[range][1] / 24`: float division, float product, truncated into the short
        eff = (f32::from(eff) * (gs.general.weapon_ranges[range_index(w)][1] / 24.0)) as i16;
        eff
    }
}

/// `Mover::setThreatRating(rating)` (mover.cpp:3938): -1 means "calc it" (`calcCV()`, i.e. the current value).
pub fn threat_rating_for(rating: i16, cur_cv: i32) -> i16 {
    if rating == -1 {
        cur_cv as i16
    } else {
        rating
    }
}

/// `BattleMech::calcLegStatus` (mech.cpp:3446). `has_pilot` and `in_recover_update` are `pilot != NULL` and the static
/// `Mover::inRecoverUpdate`. Returns the new status and whether the "vehicle incapacitated" alarm (66) must be triggered, and whether
/// the crippled radio message was sent.
pub fn calc_leg_status(
    l: &Loadout,
    leg_status: &mut u8,
    sent_crippled_msg: &mut bool,
    has_pilot: bool,
    in_recover_update: bool,
) -> LegEffects {
    let mut fx = LegEffects::default();
    let rleg = l.body[body::RLEG].damage_state == IS_DAMAGE_DESTROYED;
    let lleg = l.body[body::LLEG].damage_state == IS_DAMAGE_DESTROYED;
    if rleg {
        if lleg {
            *leg_status = LEG_STATUS_DESTROYED;
            if has_pilot && !in_recover_update {
                fx.incapacitated_alarm = true;
            }
        } else if !*sent_crippled_msg && !in_recover_update {
            fx.crippled_message = true;
            *leg_status = LEG_STATUS_IMPAIRED_RIGHT;
            *sent_crippled_msg = true;
        }
    } else if lleg {
        if !*sent_crippled_msg && !in_recover_update {
            fx.crippled_message = true;
            *leg_status = LEG_STATUS_IMPAIRED_LEFT;
            *sent_crippled_msg = true;
        }
    } else {
        *leg_status = LEG_STATUS_NORMAL;
    }
    fx
}

/// What [`calc_leg_status`] asks the pilot to do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegEffects {
    /// `pilot->triggerAlarm(PILOT_ALARM_VEHICLE_INCAPACITATED, 66)`.
    pub incapacitated_alarm: bool,
    /// `pilot->radioMessage(RADIO_CRIPPLED)`.
    pub crippled_message: bool,
}

/// `BattleMech::calcTorsoStatus` (mech.cpp:3486).
pub fn calc_torso_status(l: &Loadout) -> u8 {
    if l.body[body::CTORSO].damage_state == IS_DAMAGE_PARTIAL {
        TORSO_STATUS_IMPAIRED
    } else {
        TORSO_STATUS_NORMAL
    }
}

/// `GameObject::getMechClass` (gameobj.cpp:2049) for a mech of `tonnage`.
pub fn mech_class(tonnage: f32, gs: &GameSystem) -> u8 {
    let w = &gs.mech.mech_class_weights;
    if tonnage < w[usize::from(MECH_CLASS_LIGHT)] {
        MECH_CLASS_LIGHT
    } else if tonnage < w[usize::from(MECH_CLASS_MEDIUM)] {
        MECH_CLASS_MEDIUM
    } else if tonnage < w[usize::from(MECH_CLASS_HEAVY)] {
        MECH_CLASS_HEAVY
    } else {
        MECH_CLASS_ASSAULT
    }
}

/// `MECH3D_WEAPONTYPE_*` of a weapon form (mech.cpp:1608-1616).
pub(crate) fn weapon_node_type(form: Option<ComponentForm>) -> i32 {
    match form {
        Some(ComponentForm::WeaponMissile) => 1,
        Some(ComponentForm::WeaponBallistic) => 2,
        Some(ComponentForm::WeaponEnergy) => 4,
        _ => 0,
    }
}

/// The data every function below needs from the world, cloned out of it so the object store can be borrowed mutably.
struct Data {
    comps: std::rc::Rc<MasterComponentList>,
    gs: std::rc::Rc<GameSystem>,
}

fn data_of(w: &crate::World) -> Result<Data, String> {
    Ok(Data {
        comps: w
            .data
            .components
            .clone()
            .ok_or("component list is not loaded")?,
        gs: w
            .data
            .gamesys
            .tables
            .clone()
            .ok_or("gamesys tables are not loaded")?,
    })
}

/// The gunnery skill of the pilot flying `h` (`pilot->getSkill(MWS_GUNNERY)`), `None` when no pilot is attached.
pub(crate) fn pilot_gunnery(w: &crate::World, h: ObjectHandle) -> Option<i32> {
    let slot = crate::combat::fx::pilot_slot(w, h)?;
    w.pilots
        .get(slot)
        .map(|p| p.skills[mc2_sim_data::pilot::Skill::Gunnery.index()])
}

/// CROSS-LANE (lane E -> C): the combat numbers a new mover derives from its inventory. Mechs: `calcAmmoTotals` and the `ammoIndex`
/// links, `calcLegStatus`/`calcTorsoStatus`, `calcFireRanges`, `calcCV(true/false)`, `setThreatRating(-1)`, `calcMaxTargetDamage`
/// and the chassis class (mech.cpp:1844-1960); vehicles: the same minus the status and damage numbers (gvehicl.cpp:1160-1240).
/// Then the `calcWeaponEffectiveness(TRUE/FALSE)` pair `createPartObject`/`addMover` run once the pilot is attached and the team set
/// (mission.cpp:1040, 1281). Per-item `effectiveness`, `startAmount` and the weapon node `facing` (which the C++ derives while it reads
/// the inventory, mech.cpp:1621-1691) are filled here, since [`crate::mission::loadout`] leaves them out.
///
/// `Err` is a fatal data error of the C++ (`STOP`): ammo for a weapon the mover does not carry.
pub fn init_mover_stats(w: &mut crate::World, mover: ObjectHandle) -> Result<(), String> {
    let d = data_of(w)?;
    let scenario_time = w.clock.scenario_time;
    let unlimited = w.mission.unlimited_ammo;
    let gunnery = pilot_gunnery(w, mover);
    let (is_mech, type_handle) = match w.objects.get(mover) {
        Some(o) => match &o.kind {
            Kind::Mech(_) => (true, o.c.type_handle),
            Kind::Vehicle(_) => (false, o.c.type_handle),
            _ => return Err(format!("object {} is not a mover", mover.0)),
        },
        None => return Err(format!("no object {}", mover.0)),
    };

    // The weapon node of each weapon (`facing = appearance->getWeaponNode(type)`, mech.cpp:1608-1623): the call marks the node used,
    // so it runs in inventory order.
    let mut facings: Vec<(usize, u8)> = Vec::new();
    if is_mech {
        let l = w
            .objects
            .get(mover)
            .and_then(loadout_of)
            .ok_or("no loadout")?;
        let first = l.num_other();
        let types: Vec<(usize, i32)> = (first..first + l.num_weapons())
            .map(|i| {
                (
                    i,
                    weapon_node_type(form_of(comp(&d.comps, l.inventory[i].master_id))),
                )
            })
            .collect();
        for (i, t) in types {
            facings.push((i, w.appearance.weapon_node(mover, t) as u8));
        }
    }
    let vehicle_type = w
        .data
        .object_types
        .as_ref()
        .and_then(|t| t.get(type_handle as u32))
        .map(|t| t.data.clone());

    let o = w.objects.get_mut(mover).ok_or("no object")?;
    let tonnage = o.c.tonnage;
    let (leg_fx, weapons_out) = match &mut o.kind {
        Kind::Mech(m) => {
            let l = &mut m.loadout;
            for (i, f) in facings {
                l.inventory[i].facing = f;
            }
            prepare_items(l, &d);
            l.calc_ammo_totals(&d.comps, unlimited)?;
            l.link_ammo_indices(&d.comps, true);
            // We need to set the status states for legs and torso based upon our current condition...
            let leg_fx = calc_leg_status(
                l,
                &mut m.leg_status.0,
                &mut m.sent_crippled_msg,
                gunnery.is_some(),
                false,
            );
            l.stats.torso_status = calc_torso_status(l);
            l.calc_fire_ranges(&d.comps, &d.gs, scenario_time);
            o.c.max_cv = l.calc_cv_mech(&d.comps, true) as i16;
            o.c.cur_cv = l.calc_cv_mech(&d.comps, false) as i16;
            o.c.threat_rating = threat_rating_for(-1, i32::from(o.c.cur_cv));
            l.stats.max_weapon_damage = l.calc_max_target_damage(&d.comps, &d.gs);
            l.stats.chassis_class = mech_class(tonnage, &d.gs);
            (leg_fx, effectiveness_pair(l, gunnery, scenario_time))
        }
        Kind::Vehicle(v) => {
            let l = &mut v.loadout;
            prepare_items(l, &d);
            l.calc_ammo_totals(&d.comps, unlimited)?;
            l.link_ammo_indices(&d.comps, false);
            if let Some(mc2_sim_data::objtype::ObjectTypeData::GroundVehicle(vt)) = &vehicle_type {
                l.stats.mine_sweeper = vt.mine_sweeper;
                l.stats.mine_layer = vt.mine_layer;
            }
            // gvehicl.cpp:1234: the pair runs before the pilot is attached (avgSkill 1), mission.cpp:1040 repeats it with the pilot
            l.calc_weapon_effectiveness(None, scenario_time, true);
            l.calc_weapon_effectiveness(None, scenario_time, false);
            l.calc_fire_ranges(&d.comps, &d.gs, scenario_time);
            let speed = v.m.max_move_speed;
            // gvehicl.cpp:1242-1260 replaces the turret armor of a refit, recover or mine layer vehicle by its point pool AFTER the CV
            // pair was calculated: the load (`build_vehicle`) already did, so the CV is taken with the packet's turret armor.
            let packet = w.data.vehicle_objects.get(&(type_handle as u32));
            let overridden = packet
                .is_some_and(|p| p.refit_points > 0 || p.recover_points > 0 || p.mine_layer != 0);
            let turret = l.armor.len().saturating_sub(1);
            let kept = l.armor.get(turret).copied();
            if let (true, Some(p), Some(_)) = (overridden, packet, kept) {
                if let Some(part) = p.parts.get(turret) {
                    l.armor[turret] = (f32::from(part.cur_armor), part.max_armor);
                }
            }
            o.c.max_cv = l.calc_cv_vehicle(&d.comps, &d.gs, tonnage, speed, true) as i16;
            o.c.cur_cv = l.calc_cv_vehicle(&d.comps, &d.gs, tonnage, speed, false) as i16;
            if let (true, Some(k)) = (overridden, kept) {
                l.armor[turret] = k;
            }
            o.c.threat_rating = threat_rating_for(-1, i32::from(o.c.cur_cv));
            (
                LegEffects::default(),
                effectiveness_pair(l, gunnery, scenario_time),
            )
        }
        _ => unreachable!("checked above"),
    };
    fx::leg_effects(w, mover, leg_fx);
    if weapons_out {
        fx::radio_message(w, mover, fx::RADIO_WEAPONS_OUT);
    }
    Ok(())
}

/// The per-item numbers the C++ sets while it reads the inventory (`ammoIndex = -1`, weapon `effectiveness`, ammo `startAmount`)
/// that [`crate::mission::loadout`] leaves out.
fn prepare_items(l: &mut Loadout, d: &Data) {
    let (n_other, n_weapons) = (l.num_other(), l.num_weapons());
    let n_items = l.num_items();
    for item in 0..n_items {
        let it = &mut l.inventory[item];
        it.ammo_index = -1;
        if item >= n_other && item < n_other + n_weapons {
            if let Some(ws) = weapon_stats(comp(&d.comps, it.master_id)) {
                it.effectiveness = Loadout::weapon_item_effectiveness(ws, &d.gs);
            }
        } else if item >= n_other + n_weapons {
            it.start_amount = it.amount;
        }
    }
}

/// `calcWeaponEffectiveness(TRUE)`, `(FALSE)` (mission.cpp:1040): true when the second fell to 0.
fn effectiveness_pair(l: &mut Loadout, gunnery: Option<i32>, scenario_time: f32) -> bool {
    l.calc_weapon_effectiveness(gunnery, scenario_time, true);
    l.calc_weapon_effectiveness(gunnery, scenario_time, false)
}

// -----------------------------------------------------------------------------------------------------------------------------
// resetComponents

/// `LogisticsMasterLocationTableEntry` (mech.cpp:1977): (clear out, body location, slot number).
const LOGISTICS_TABLE: [(bool, usize, usize); MAX_MOVER_INVENTORY_ITEMS] = [
    (false, 1, 0),
    (false, 1, 1),
    (false, 4, 1),
    (false, 5, 1),
    (false, 6, 1),
    (false, 7, 1),
    (false, 0, 0),
    (false, 0, 1),
    (false, 4, 0),
    (false, 5, 0),
    (false, 6, 0),
    (false, 7, 0),
    (false, 0, 2),
    (false, 0, 3),
    (false, 0, 4),
    (false, 0, 5),
    (true, 6, 3),
    (false, 6, 4),
    (false, 6, 5),
    (false, 7, 3),
    (false, 7, 4),
    (false, 7, 5),
    (true, 4, 4),
    (true, 2, 0),
    (true, 1, 2),
    (true, 3, 0),
    (true, 5, 4),
    (true, 4, 5),
    (true, 2, 1),
    (true, 1, 3),
    (true, 3, 1),
    (true, 5, 5),
    (true, 4, 6),
    (true, 2, 2),
    (true, 1, 4),
    (true, 3, 2),
    (true, 5, 6),
    (true, 4, 7),
    (true, 2, 3),
    (true, 1, 5),
    (true, 3, 3),
    (true, 5, 7),
    (true, 4, 8),
    (true, 2, 4),
    (true, 1, 6),
    (true, 3, 4),
    (true, 5, 8),
    (true, 4, 9),
    (true, 2, 5),
    (true, 1, 7),
    (true, 3, 5),
    (true, 5, 9),
    (true, 4, 10),
    (true, 2, 6),
    (true, 1, 8),
    (true, 3, 6),
    (true, 5, 10),
    (true, 4, 11),
    (true, 2, 7),
    (true, 1, 9),
    (true, 3, 7),
    (true, 5, 11),
    (true, 2, 8),
    (true, 1, 10),
    (true, 3, 8),
    (true, 2, 9),
    (true, 1, 11),
    (true, 3, 9),
    (true, 2, 10),
    (true, 3, 10),
    (true, 2, 11),
    (true, 3, 11),
];

const JUMPJET_SLOT: usize = 16;
const SENSOR_SLOT: usize = 12;
const ELECTRONIC_SLOT: usize = 13;
const WEAPONAMMO_SLOT: usize = 17;

/// `ARMOR_*_PERCENT` (mech.cpp:285), by armor location.
const ARMOR_PERCENT: [f32; NUM_ARMOR_LOCATIONS] = [
    0.0, 0.17, 0.125, 0.125, 0.12, 0.12, 0.13, 0.13, 0.03, 0.02, 0.02,
];
/// `ARMOR_POINTS_PER_NEWARMOR` (mech.cpp:284).
const ARMOR_POINTS_PER_NEWARMOR: f32 = 32.0;

/// `BattleMech::resetComponents(totalComponents, componentList)` (mech.cpp:2065): replace the logistics-replaceable components of a
/// freshly created mech by the player's custom loadout and rebuild everything derived from the inventory.
///
/// `ItemLocationToInvLocation` (CSV item index -> inventory slot) is recovered from the load: the item index of an inventory slot is
/// found through the critical spaces of the body location tables, so it is rebuilt here from `LOGISTICS_TABLE` and the current spaces.
pub fn reset_components(
    w: &mut crate::World,
    mech: ObjectHandle,
    components: &[i32],
) -> Result<(), String> {
    let d = data_of(w)?;
    let special = d.comps.special();
    let scenario_time = w.clock.scenario_time;
    let unlimited = w.mission.unlimited_ammo;
    let gunnery = pilot_gunnery(w, mech);

    // snapshot what the reset reads
    let (old_inventory, item_location_to_inv, old_num_armor_components) = {
        let o = w.objects.get(mech).ok_or("no object")?;
        let Kind::Mech(m) = &o.kind else {
            return Err("LogisticsMech was not a MECH!!".into());
        };
        (
            m.loadout.inventory.clone(),
            m.loadout.item_to_inv.clone(),
            m.loadout.stats.num_armor_components,
        )
    };

    // Copy current itemList to localLists; remove all logistics replaceable components.
    let mut local: [i32; MAX_MOVER_INVENTORY_ITEMS] = [0xff; MAX_MOVER_INVENTORY_ITEMS];
    for (item, it) in old_inventory.iter().enumerate() {
        local[item] = i32::from(it.master_id);
        if it.master_id != 255 {
            let item_location = (0..MAX_MOVER_INVENTORY_ITEMS)
                .find(|&i| usize::from(item_location_to_inv.get(i).copied().unwrap_or(255)) == item)
                .ok_or_else(|| format!("Could Not Find Item {item} on Mech"))?;
            if LOGISTICS_TABLE[item_location].0 {
                local[item] = 0xff;
            }
        }
    }

    // Reset the weapon node data in the mech appearance...
    w.appearance.reset_weapon_nodes(mech);

    let o = w.objects.get_mut(mech).ok_or("no object")?;
    let Kind::Mech(m) = &mut o.kind else {
        return Err("LogisticsMech was not a MECH!!".into());
    };
    let l = &mut m.loadout;

    // If we added Armor to the mech, remove the armor so that the player's new armor components are all that is added.
    if old_num_armor_components != 0 {
        let total_armor_points = ARMOR_POINTS_PER_NEWARMOR * old_num_armor_components as f32;
        for (i, a) in l.armor.iter_mut().enumerate() {
            // `long armorPercent = ARMOR_x_PERCENT * totalArmorPoints` truncates; `maxArmor -= armorPercent` wraps in the uchar
            let armor_percent = (ARMOR_PERCENT[i] * total_armor_points) as i64;
            a.1 = (i64::from(a.1) - armor_percent) as u8;
            a.0 = f32::from(a.1);
        }
    }

    let mut num_new_armors = 0i64;
    let mut num_jump_jets = 0u8;
    let mut num_electronics = 0usize;
    let mut num_weapons_ammo = 0usize;

    // Add the new components to open locations.
    for &new_item in components {
        let c = d.comps.get(new_item);
        match form_of(c) {
            Some(ComponentForm::HeatSink) => continue,
            Some(
                ComponentForm::Cockpit
                | ComponentForm::LifeSupport
                | ComponentForm::Gyroscope
                | ComponentForm::PowerAmplifier
                | ComponentForm::Case
                | ComponentForm::Actuator
                | ComponentForm::Engine,
            ) => {
                // If any of these are in the list its REALLY BAD
                return Err(format!("Invalid component added in Logistics {new_item}"));
            }
            Some(ComponentForm::Sensor) => local[SENSOR_SLOT] = new_item,
            Some(ComponentForm::Ecm | ComponentForm::Probe | ComponentForm::Jammer) => {
                local[ELECTRONIC_SLOT + num_electronics] = new_item;
                num_electronics += 1;
                if num_electronics > 3 {
                    return Err("TOO many electronic warfare components on this mech".into());
                }
            }
            Some(
                ComponentForm::Weapon
                | ComponentForm::WeaponEnergy
                | ComponentForm::WeaponBallistic
                | ComponentForm::WeaponMissile
                | ComponentForm::Ammo,
            ) => {
                local[WEAPONAMMO_SLOT + num_weapons_ammo] = new_item;
                num_weapons_ammo += 1;
                // Heidi does not give me ammo. Add manually here.
                let ammo_master = weapon_stats(c).map_or(0, |ws| i32::from(ws.ammo_master_id));
                if ammo_master != 0 {
                    local[WEAPONAMMO_SLOT + num_weapons_ammo] = ammo_master;
                    num_weapons_ammo += 1;
                }
                if num_weapons_ammo > 54 {
                    return Err("TOO many weapons and/or Ammo components on this mech".into());
                }
            }
            Some(ComponentForm::Bulk) => num_new_armors += 1,
            Some(ComponentForm::JumpJet) => local[JUMPJET_SLOT] = new_item,
            _ => {}
        }
    }

    // Now use the above lists to re-component out the mech! The C++ rewrites the 72-slot inventory in place: the fields a pass does not
    // assign keep what the slot held before (a jump jet in a slot that held a weapon keeps its `effectiveness` and `facing`).
    l.stats.anti_missile.clear();
    let mut inv72: Vec<InventoryItem> = std::mem::take(&mut l.inventory);
    inv72.extend(std::mem::take(&mut l.tail));
    inv72.resize(MAX_MOVER_INVENTORY_ITEMS, InventoryItem::default());
    let (mut num_other, mut num_weapons, mut num_ammos) = (0u8, 0u8, 0u8);
    let mut item_to_inv = [0xffu8; MAX_MOVER_INVENTORY_ITEMS];
    let mut real_item_num = 0usize;

    let is_other = |f: ComponentForm| {
        matches!(
            f,
            ComponentForm::JumpJet
                | ComponentForm::Cockpit
                | ComponentForm::Sensor
                | ComponentForm::LifeSupport
                | ComponentForm::Gyroscope
                | ComponentForm::PowerAmplifier
                | ComponentForm::Ecm
                | ComponentForm::Probe
                | ComponentForm::Jammer
                | ComponentForm::Bulk
                | ComponentForm::Case
                | ComponentForm::Actuator
                | ComponentForm::Engine
                | ComponentForm::HeatSink
        )
    };
    let valid = |m: i32| m != 0 && m != 255;
    // everything but weapons and ammo
    for cur in 0..MAX_MOVER_INVENTORY_ITEMS {
        let m = local[cur];
        if !valid(m) {
            continue;
        }
        let c = d.comps.get(m);
        let Some(form) = form_of(c) else { continue };
        if is_other(form) {
            if form == ComponentForm::JumpJet {
                num_jump_jets = 5;
            }
            num_other += 1;
            let it = &mut inv72[real_item_num];
            it.master_id = m as u8;
            it.health = c.map_or(0, |c| c.health);
            it.disabled = false;
            it.amount = 1;
            it.ammo_index = -1;
            it.ready_time = 0.0;
            it.body_location = 255;
            item_to_inv[cur] = real_item_num as u8;
            real_item_num += 1;
        }
    }
    // the weapons
    let mut weapon_facings: Vec<(usize, i32)> = Vec::new();
    for cur in 0..MAX_MOVER_INVENTORY_ITEMS {
        let m = local[cur];
        if !valid(m) {
            continue;
        }
        let c = d.comps.get(m);
        let form = form_of(c);
        if form.is_some_and(|f| f.is_weapon()) {
            num_weapons += 1;
            let it = &mut inv72[real_item_num];
            it.master_id = m as u8;
            it.health = c.map_or(0, |c| c.health);
            it.disabled = false;
            it.amount = 1;
            it.ammo_index = -1;
            it.ready_time = 0.0;
            it.body_location = 255;
            it.effectiveness =
                weapon_stats(c).map_or(0, |ws| Loadout::weapon_item_effectiveness(ws, &d.gs));
            weapon_facings.push((real_item_num, weapon_node_type(form)));
            item_to_inv[cur] = real_item_num as u8;
            real_item_num += 1;
        }
    }
    // the ammo
    for cur in 0..MAX_MOVER_INVENTORY_ITEMS {
        let m = local[cur];
        if !valid(m) {
            continue;
        }
        let c = d.comps.get(m);
        if form_of(c) == Some(ComponentForm::Ammo) {
            num_ammos += 1;
            let per_ton = match c.map(|c| &c.stats) {
                Some(ComponentStats::Ammo { ammo_per_ton, .. }) => *ammo_per_ton,
                _ => 0,
            };
            let it = &mut inv72[real_item_num];
            it.master_id = m as u8;
            it.health = c.map_or(0, |c| c.health);
            it.disabled = false;
            it.ammo_index = -1;
            it.amount = per_ton as i16;
            it.start_amount = per_ton as i16;
            it.ready_time = 0.0;
            it.body_location = 255;
            item_to_inv[cur] = real_item_num as u8;
            real_item_num += 1;
        }
    }
    l.tail = inv72.split_off(real_item_num);
    l.inventory = inv72;
    l.counts = (num_weapons, num_other, num_ammos);
    l.item_to_inv = item_to_inv.to_vec();

    // If we added BULK to the mech, adjust the max and current armor settings.
    if num_new_armors != 0 {
        let total_armor_points = ARMOR_POINTS_PER_NEWARMOR * num_new_armors as f32;
        for (i, a) in l.armor.iter_mut().enumerate() {
            let armor_percent = ARMOR_PERCENT[i] * total_armor_points;
            a.1 = (f32::from(a.1) + armor_percent) as u8;
            a.0 = f32::from(a.1);
        }
    }

    // Reset the quick access components in case they removed them in logistics.
    l.critical[crate::mission::loadout::crit::JUMP_JETS] = 255;
    let n_items = l.num_items();
    let mut gyro = l.stats.gyro;
    let mut actuator = l.stats.actuator;
    // Now, read in the component layout for each body location...
    for cur_location in 0..NUM_BODY_LOCATIONS {
        l.body[cur_location].case = true; // ALL MC2 Mechs have CASE everywhere.
        l.body[cur_location].total_spaces = 0;
        let num_spaces = l.body[cur_location].critical_spaces.len();
        for cur_space in 0..num_spaces {
            // Find out where in the space table this location lies.
            let table_entry = LOGISTICS_TABLE
                .iter()
                .position(|&(_, loc, slot)| loc == cur_location && slot == cur_space);
            let Some(table_entry) = table_entry else {
                l.body[cur_location].critical_spaces[cur_space] = (0xff, false);
                continue;
            };
            let space_data = item_to_inv[table_entry];
            l.body[cur_location].critical_spaces[cur_space] = (space_data, false);
            if space_data < 255 {
                if usize::from(space_data) >= n_items {
                    // Just ignore this space. Sage has filled them all in now.
                    continue;
                }
                let inv = &mut l.inventory[usize::from(space_data)];
                inv.body_location = cur_location as u8;
                let c = d.comps.get(i32::from(inv.master_id));
                l.body[cur_location].total_spaces += c.map_or(0, |c| i32::from(c.size));
                match form_of(c) {
                    Some(ComponentForm::Cockpit) => {
                        l.critical[crate::mission::loadout::crit::COCKPIT] = space_data
                    }
                    Some(ComponentForm::JumpJet) => {
                        l.critical[crate::mission::loadout::crit::JUMP_JETS] = space_data
                    }
                    Some(ComponentForm::Sensor) => {
                        l.critical[crate::mission::loadout::crit::SENSOR] = space_data
                    }
                    Some(ComponentForm::WeaponBallistic) => {
                        let master = i32::from(inv.master_id);
                        if master == special.clan_anti_missile
                            || master == special.inner_sphere_anti_missile
                        {
                            l.stats.anti_missile.push(space_data);
                        }
                    }
                    Some(ComponentForm::Actuator) => {
                        let master = i32::from(inv.master_id);
                        if master == special.arm_actuator {
                            if cur_location == body::LARM {
                                actuator[0] = space_data;
                            } else if cur_location == body::RARM {
                                actuator[1] = space_data;
                            }
                        } else if master == special.leg_actuator {
                            if cur_location == body::LLEG {
                                actuator[2] = space_data;
                            } else if cur_location == body::RLEG {
                                actuator[3] = space_data;
                            }
                        }
                    }
                    Some(ComponentForm::Engine) => {
                        l.critical[crate::mission::loadout::crit::ENGINE] = space_data
                    }
                    Some(ComponentForm::LifeSupport) => {
                        l.critical[crate::mission::loadout::crit::LIFE_SUPPORT] = space_data
                    }
                    Some(ComponentForm::Gyroscope) => gyro = space_data,
                    Some(ComponentForm::Ecm) => {
                        l.critical[crate::mission::loadout::crit::ECM] = space_data
                    }
                    Some(ComponentForm::Probe) => {
                        l.critical[crate::mission::loadout::crit::PROBE] = space_data
                    }
                    _ => {}
                }
            }
        }
    }
    l.stats.gyro = gyro;
    l.stats.actuator = actuator;

    l.calc_ammo_totals(&d.comps, unlimited)?;
    l.link_ammo_indices(&d.comps, true);

    // calcFireRanges, the CV pair, the threat rating and the maximum damage
    l.calc_fire_ranges(&d.comps, &d.gs, scenario_time);
    let max_cv = l.calc_cv_mech(&d.comps, true);
    let cur_cv = l.calc_cv_mech(&d.comps, false);
    l.stats.max_weapon_damage = l.calc_max_target_damage(&d.comps, &d.gs);
    m.num_jump_jets = num_jump_jets;
    o.c.max_cv = max_cv as i16;
    o.c.cur_cv = cur_cv as i16;
    o.c.threat_rating = threat_rating_for(-1, cur_cv);

    // appearance->getWeaponNode(type) per weapon, in inventory order (the reset cleared the nodes first)
    let facings: Vec<(usize, u8)> = weapon_facings
        .iter()
        .map(|&(idx, t)| (idx, w.appearance.weapon_node(mech, t) as u8))
        .collect();
    let _ = gunnery;
    if let Some(l) = w.objects.get_mut(mech).and_then(loadout_of_mut) {
        for (idx, f) in facings {
            l.inventory[idx].facing = f;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::BodyLocationState;

    fn legs(l: u8, r: u8) -> Loadout {
        let mut body = vec![BodyLocationState::default(); 8];
        body[body::LLEG].damage_state = l;
        body[body::RLEG].damage_state = r;
        Loadout {
            body,
            ..Loadout::default()
        }
    }

    #[test]
    fn leg_status_follows_the_destroyed_legs() {
        let (mut status, mut sent) = (LEG_STATUS_NORMAL, false);
        let l = legs(IS_DAMAGE_PARTIAL, IS_DAMAGE_PARTIAL);
        assert_eq!(
            calc_leg_status(&l, &mut status, &mut sent, true, false),
            LegEffects::default()
        );
        assert_eq!(status, LEG_STATUS_NORMAL);

        // one leg gone: impaired, the crippled message goes out once
        let l = legs(IS_DAMAGE_NONE, IS_DAMAGE_DESTROYED);
        let fx = calc_leg_status(&l, &mut status, &mut sent, true, false);
        assert!(fx.crippled_message && !fx.incapacitated_alarm);
        assert_eq!((status, sent), (LEG_STATUS_IMPAIRED_RIGHT, true));
        // the other leg too: destroyed, and the incapacitated alarm goes to a mech with a pilot
        let l = legs(IS_DAMAGE_DESTROYED, IS_DAMAGE_DESTROYED);
        let fx = calc_leg_status(&l, &mut status, &mut sent, true, false);
        assert!(fx.incapacitated_alarm);
        assert_eq!(status, LEG_STATUS_DESTROYED);
        // while a recover update runs nothing is announced, and a left leg gone after the message keeps the old status
        let (mut status, mut sent) = (LEG_STATUS_NORMAL, true);
        let l = legs(IS_DAMAGE_DESTROYED, IS_DAMAGE_NONE);
        assert_eq!(
            calc_leg_status(&l, &mut status, &mut sent, true, false),
            LegEffects::default()
        );
        assert_eq!(status, LEG_STATUS_NORMAL);
        // repaired legs reset the status
        let l = legs(IS_DAMAGE_NONE, IS_DAMAGE_NONE);
        calc_leg_status(&l, &mut status, &mut sent, true, false);
        assert_eq!(status, LEG_STATUS_NORMAL);
    }

    #[test]
    fn the_torso_is_impaired_only_while_the_center_torso_is_partial() {
        let mut l = Loadout {
            body: vec![BodyLocationState::default(); 8],
            ..Loadout::default()
        };
        assert_eq!(calc_torso_status(&l), TORSO_STATUS_NORMAL);
        l.body[body::CTORSO].damage_state = IS_DAMAGE_PARTIAL;
        assert_eq!(calc_torso_status(&l), TORSO_STATUS_IMPAIRED);
        l.body[body::CTORSO].damage_state = IS_DAMAGE_DESTROYED;
        assert_eq!(calc_torso_status(&l), TORSO_STATUS_NORMAL);
    }

    #[test]
    fn a_threat_rating_of_minus_one_means_the_current_combat_value() {
        assert_eq!(threat_rating_for(-1, 1234), 1234);
        assert_eq!(threat_rating_for(1, 1234), 1);
        // the rating is a short
        assert_eq!(threat_rating_for(-1, 70_000), 70_000i32 as i16);
    }

    #[test]
    fn the_tie_table_picks_the_nearest_range_point() {
        // a tie of short (bit 0) and medium (bit 1) picks point 1; short and long (bits 0 and 2) pick point 2
        assert_eq!(OPTIMAL_RANGE_TIE_TABLE[0b00011], 1);
        assert_eq!(OPTIMAL_RANGE_TIE_TABLE[0b00101], 2);
        assert_eq!(OPTIMAL_RANGE_TIE_TABLE[0b00100], 2);
        assert_eq!(OPTIMAL_RANGE_TIE_TABLE[0], 255);
        assert_eq!(OPTIMAL_RANGE_TIE_TABLE[31], 4);
    }

    #[test]
    fn weapon_effectiveness_is_summed_per_weapon_with_a_truncating_skill_factor() {
        let item = |eff: i16, disabled: bool| InventoryItem {
            effectiveness: eff,
            disabled,
            ..InventoryItem::default()
        };
        let mut l = Loadout {
            inventory: vec![
                item(0, false),
                item(15, false),
                item(15, true),
                item(7, false),
            ],
            counts: (3, 1, 0),
            ..Loadout::default()
        };
        // gunnery 45: 0.9; 15 * 0.9 = 13.5 -> 13, then 13 + 7 * 0.9 = 19.3 -> 19
        assert!(!l.calc_weapon_effectiveness(Some(45), 3.0, false));
        assert_eq!(l.stats.weapon_effectiveness, 19.0);
        assert_eq!(l.stats.last_weapon_effectiveness_calc, 3.0);
        // the maximum counts disabled weapons too: 13 + 13 + 6 = 32
        l.calc_weapon_effectiveness(Some(45), 0.0, true);
        assert_eq!(l.stats.max_weapon_effectiveness, 32.0);
        // no pilot: skill factor 1
        l.calc_weapon_effectiveness(None, 0.0, false);
        assert_eq!(l.stats.weapon_effectiveness, 22.0);
        // all weapons down: the weapons-out message
        l.inventory[1].disabled = true;
        l.inventory[3].disabled = true;
        assert!(l.calc_weapon_effectiveness(None, 0.0, false));
    }
}
