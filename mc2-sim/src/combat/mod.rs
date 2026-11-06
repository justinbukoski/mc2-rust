// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE C - weapons, damage, criticals, ammo, artillery, turret fire control.
//!
//! Port of `code/weaponbolt.{h,cpp}`, `code/artlry.{h,cpp}`, the weapon/damage halves of `code/mover.cpp`, `code/mech.cpp`,
//! `code/gvehicl.cpp` (`fireWeapon`, `handleWeaponFire`, `handleWeaponHit`, `calcHitLocation`, `calcCriticalHit`,
//! `calcAttackChance`, ammo, `loadGameSystem` constants) and `code/turret.cpp` fire control. Multiplayer chunk code
//! (`WeaponFireChunk`, `WeaponHitChunk`) is not ported. The unit-behaviour (who decides to shoot) is lane D; this lane
//! owns what happens once a weapon is fired.
//!
//! Visual-only draws (craters, debris, impact offsets) use `World::rng.visual`; everything that can change game state uses
//! `World::rng.sim`, in the same order as the C++ (the oracle's `visual_rng_sites.txt` is the authority).
//!
//! Slice 1 (combat set-up and hit maths): [`stats`] (`init_mover_stats`, `reset_components`, ammo totals, fire ranges, CV, threat
//! rating, weapon effectiveness), [`shot`] (`WeaponShotInfo`), [`attack`] (`calcAttackChance`), [`hit`] (`calcHitLocation`),
//! [`damage`] (`handleWeaponHit`: armor, internal structure, transfer, CASE, criticals, destruction), [`fx`] (the calls the
//! damage code makes on the pilot). Weapon firing in the world, bolts, artillery and turret fire control are the next slice.

pub mod ammo;
pub mod artillery;
pub mod artillery_math;
pub mod artillery_static;
pub mod attack;
pub mod bolt_flight;
pub mod bolt_impact;
pub mod damage;
pub mod fire;
pub mod fire_math;
pub mod fx;
pub mod hit;
pub mod los;
pub mod shot;
pub mod stats;
pub mod turret;
pub mod turret_math;
pub mod weapon_bolt;
pub mod weapon_fx;

use crate::ids::{ObjectHandle, WatchId};
use crate::mover::MoverCore;
use crate::object::{Kind, Obj};

pub use attack::{calc_attack_chance, AttackChance};
pub use damage::handle_weapon_hit;
pub use hit::calc_hit_location;
pub use stats::{init_mover_stats, reset_components};

/// `UNLIMITED_SHOTS` (mover.h): the ammo tally of a weapon without ammunition, and of every weapon under `useUnlimitedAmmo`.
pub const UNLIMITED_SHOTS: i32 = 9999;
/// `ATTACK_TO_DESTROY` (`AttackType`, tacordr.h:90): the attack type a turret passes to `calcHitLocation`.
pub const ATTACK_TO_DESTROY: i32 = 1;
/// `MAX_MOVER_INVENTORY_ITEMS` (mover.h:588).
pub const MAX_MOVER_INVENTORY_ITEMS: usize = 72;

/// `AttackSourceType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum AttackSource {
    WeaponFire,
    Collision,
    Dfa,
    Mine,
    Artillery,
}

impl AttackSource {
    /// The enum value (`ATTACKSOURCE_*`, gameobj.h:78).
    pub fn code(self) -> i32 {
        self as i32
    }

    /// From the enum value; anything else is `None` (the C++ would index `MechHitSectionTable` out of range).
    pub fn from_code(code: i32) -> Option<AttackSource> {
        [
            AttackSource::WeaponFire,
            AttackSource::Collision,
            AttackSource::Dfa,
            AttackSource::Mine,
            AttackSource::Artillery,
        ]
        .get(usize::try_from(code).ok()?)
        .copied()
    }
}

/// `WeaponShotInfo` (gameobj.h:86-103): one damaging hit. Build it with [`WeaponShotInfo::init`] (difficulty scaling for known
/// attackers); the plain fields can be written directly when the C++ copy-constructs instead.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponShotInfo {
    /// Attacker.
    pub attacker: WatchId,
    /// Attack weapon master component id.
    pub master_id: i32,
    /// Damage (difficulty-scaled and quantised to 0.25 in `init`, gameobj.cpp:139).
    pub damage: f32,
    /// Hit location.
    pub hit_location: i32,
    /// Entry angle.
    pub entry_angle: f32,
}

/// Armor, internal structure, inventory and critical state of a mover (`Mover::armor/body/inventory/...`).
#[derive(Clone, Debug, Default)]
pub struct Loadout {
    /// `armor[i].curArmor`, `maxArmor`.
    pub armor: Vec<(f32, u8)>,
    /// `body[i]`.
    pub body: Vec<BodyLocationState>,
    /// `inventory[]`: `numOther` other items, then `numWeapons` weapons, then `numAmmos` ammo (the valid prefix of the C++ array).
    pub inventory: Vec<InventoryItem>,
    /// `ammoTypeTotal[]` (`numAmmoTypes` of them): (master id, current, max).
    pub ammo_totals: Vec<(i32, i32, i32)>,
    /// `numWeapons`, `numOther`, `numAmmos` (counts at the front of `inventory`), in that order.
    pub counts: (u8, u8, u8),
    /// Indices of critical components in the order `cockpit`, `engine`, `lifeSupport`, `sensor`, `ecm`, `probe`, `jumpJets`
    /// (255 = none).
    pub critical: Vec<u8>,
    /// The scalar combat state of the mover.
    pub stats: MoverStats,
    /// The inventory slots past `numOther + numWeapons + numAmmos` (the C++ array is 72 long): `resetComponents` rewrites the array in
    /// place, so the fields it does not assign keep what the slot held.
    pub tail: Vec<InventoryItem>,
    /// `BattleMech::ItemLocationToInvLocation[72]`: inventory slot of each chassis CSV item (255 = not installed). Mechs only.
    pub item_to_inv: Vec<u8>,
}

impl Loadout {
    /// `numOther`.
    pub fn num_other(&self) -> usize {
        usize::from(self.counts.1)
    }
    /// `numWeapons`.
    pub fn num_weapons(&self) -> usize {
        usize::from(self.counts.0)
    }
    /// `numAmmos`.
    pub fn num_ammos(&self) -> usize {
        usize::from(self.counts.2)
    }
    /// `numOther + numWeapons + numAmmos`.
    pub fn num_items(&self) -> usize {
        self.num_other() + self.num_weapons() + self.num_ammos()
    }
}

/// The scalar combat fields of `Mover`, `BattleMech` and `GroundVehicle` that slice 1 reads or writes.
#[derive(Clone, Debug)]
pub struct MoverStats {
    /// `BattleMech::gyro` (inventory index, 255 = none).
    pub gyro: u8,
    /// `BattleMech::actuator[ACTUATOR_LSHOULDER, RSHOULDER, LHIP, RHIP]` (255 = none).
    pub actuator: [u8; 4],
    /// `Mover::numAntiMissileSystems` and `antiMissileSystem[]`.
    pub anti_missile: Vec<u8>,
    /// `minRange`, `maxRange`, `optimalRange` (`calcFireRanges`).
    pub min_range: f32,
    /// See `min_range`.
    pub max_range: f32,
    /// See `min_range`.
    pub optimal_range: f32,
    /// `numFunctionalWeapons`.
    pub num_functional_weapons: i32,
    /// `weaponEffectiveness`, `maxWeaponEffectiveness`.
    pub weapon_effectiveness: f32,
    /// See `weapon_effectiveness`.
    pub max_weapon_effectiveness: f32,
    /// `BattleMech::maxWeaponDamage`.
    pub max_weapon_damage: f32,
    /// `lastWeaponEffectivenessCalc`, `lastOptimalRangeCalc` (scenario time).
    pub last_weapon_effectiveness_calc: f32,
    /// See above.
    pub last_optimal_range_calc: f32,
    /// `BattleMech::torsoStatus` (`TORSO_STATUS_*`).
    pub torso_status: u8,
    /// `BattleMech::chassisClass` (`MechClass`).
    pub chassis_class: u8,
    /// `BattleMech::chassisBR` (chassis CSV row 10 column 5).
    pub chassis_br: i32,
    /// `GroundVehicle::battleRating`: the packet's override of `calcCV`, -1 when absent.
    pub battle_rating: i32,
    /// `BattleMech::numArmorComponents`: BULK components the load folded into the armor.
    pub num_armor_components: i32,
    /// `Mover::causeOfDeath` (`DeathCause`).
    pub cause_of_death: i32,
    /// `Mover::ejectOrderGiven`.
    pub eject_order_given: bool,
    /// `damageRateTally`, `pilotCheckDamageTally`, `BattleMech::damageThisFrame`, `damageAfterDisabled`.
    pub damage_rate_tally: f32,
    /// See above.
    pub pilot_check_damage_tally: f32,
    /// See above.
    pub damage_this_frame: f32,
    /// See above.
    pub damage_after_disabled: f32,
    /// `numWeaponHitsHandled`.
    pub num_weapon_hits_handled: i32,
    /// `BattleMech::playedCriticalHit`.
    pub played_critical_hit: bool,
    /// `GroundVehicle::mineLayer`: mines left (`mineLayer--` when a mine-layer's turret is hit by a mine).
    pub mine_layer: i32,
    /// `GroundVehicle::mineSweeper` (cleared by a hit on the front).
    pub mine_sweeper: bool,
    /// `Mover::lowestWeaponNodeID`. Not read any more: the one cache of `getLOSPosition` / `hasWeaponNode` is `MoverState::cached_los_weapon_node`
    /// (kept only for the snapshot layout).
    pub lowest_weapon_node_id: i32,
    /// `Mover::lowestWeaponNodeZ`. Not read any more (see above).
    pub lowest_weapon_node_z: f32,
}

impl Default for MoverStats {
    fn default() -> Self {
        MoverStats {
            gyro: 255,
            actuator: [255; 4],
            anti_missile: Vec::new(),
            min_range: 0.0,
            max_range: 0.0,
            optimal_range: 0.0,
            num_functional_weapons: 0,
            weapon_effectiveness: 0.0,
            max_weapon_effectiveness: 0.0,
            max_weapon_damage: 0.0,
            last_weapon_effectiveness_calc: 0.0,
            last_optimal_range_calc: 0.0,
            torso_status: 0,
            chassis_class: 0,
            chassis_br: 0,
            battle_rating: -1,
            num_armor_components: 0,
            cause_of_death: 0,
            eject_order_given: false,
            damage_rate_tally: 0.0,
            pilot_check_damage_tally: 0.0,
            damage_this_frame: 0.0,
            damage_after_disabled: 0.0,
            num_weapon_hits_handled: 0,
            played_critical_hit: false,
            mine_layer: 0,
            mine_sweeper: false,
            lowest_weapon_node_id: -2,
            lowest_weapon_node_z: -9999.9,
        }
    }
}

/// One body location.
#[derive(Clone, Debug, Default)]
pub struct BodyLocationState {
    /// `curInternalStructure`.
    pub cur_internal_structure: f32,
    /// `maxInternalStructure`.
    pub max_internal_structure: u8,
    /// `damageState` (`IS_DAMAGE_*`).
    pub damage_state: u8,
    /// `criticalSpaces[]`: (inventory id, hit).
    pub critical_spaces: Vec<(u8, bool)>,
    /// `CASE` (mechs: true everywhere, set by the load; a CASE component destroyed by a critical clears it).
    pub case: bool,
    /// `totalSpaces`: the summed size of the components installed here.
    pub total_spaces: i32,
}

/// `InventoryItem` (mover.h:396).
#[derive(Clone, Copy, Debug, Default)]
pub struct InventoryItem {
    /// `masterID`.
    pub master_id: u8,
    /// `health`.
    pub health: u8,
    /// `disabled`.
    pub disabled: bool,
    /// `facing` (weapon node id).
    pub facing: u8,
    /// `startAmount`: the ammo's mission-start level.
    pub start_amount: i16,
    /// `amount`.
    pub amount: i16,
    /// `ammoIndex`: index into the ammo type totals, -1 for none.
    pub ammo_index: i16,
    /// `readyTime`.
    pub ready_time: f32,
    /// `bodyLocation`.
    pub body_location: u8,
    /// `effectiveness` (weapons: damage over 10 s scaled by the longest range).
    pub effectiveness: i16,
}

/// Weapon bolt state (`WeaponBolt`, weaponbolt.h:188-222).
#[derive(Clone, Debug, Default)]
pub struct BoltState {
    /// Deterministic identity assigned when this shot is created.
    pub shot_id: u32,
    /// `effectId`.
    pub effect_id: i32,
    /// Owner.
    pub owner: WatchId,
    /// Target object (or none) and position.
    pub target: WatchId,
    /// `targetPosition`.
    pub target_position: crate::math::Vec3,
    /// `hotSpotNumber`, `targetHotSpot`.
    pub hot_spots: (i32, i32),
    /// `hitTarget`.
    pub hit_target: bool,
    /// `timeLeft`.
    pub time_left: f32,
    /// `distanceToTarget`.
    pub distance_to_target: f32,
    /// `weaponShot` (filled at fire time; `None` until then).
    pub weapon_shot: Option<WeaponShotInfo>,
    /// `laserPosition`.
    pub laser_position: crate::math::Vec3,
    /// `halfDistanceToTarget`.
    pub half_distance_to_target: f32,
    /// `hitLeft`.
    pub hit_left: f32,
    /// Frames the gosFX hit effect of a bolt that hit a target still runs (`hitEffect`, weaponbolt.cpp:1360-1402); 0 when none.
    pub hit_effect_left: u32,
    /// `goalHeight`.
    pub goal_height: f32,
    /// Set on the frame a bolt is created (the C++ `created` flag pattern); cleared after the first update.
    pub just_created: bool,
    /// `targetPosition != NULL`.
    pub has_target_position: bool,
}

/// Artillery strike state (artlry.h).
#[derive(Clone, Debug, Default)]
pub struct ArtilleryState {
    /// Strike type.
    pub strike_type: i32,
    /// Remaining time to impact.
    pub time_to_impact: f32,
    /// Owner commander.
    pub commander: i8,
    /// `bombRunStarted`.
    pub bomb_run_started: bool,
    /// `effectId`.
    pub effect_id: i32,
    /// `timeToLaunch`.
    pub time_to_launch: f32,
    /// `timeToBlind`.
    pub time_to_blind: f32,
    /// `sensorRange`.
    pub sensor_range: f32,
    /// `boom` (impact sound pending).
    pub boom: bool,
    /// `whoosh` (flyover sound pending).
    pub whoosh: bool,
    /// Set on the frame a strike is created (the C++ `created` flag pattern); cleared after the first update.
    pub just_created: bool,
    /// `OBJECT_FLAG_SENSORS_GOING`.
    pub sensors_going: bool,
    /// `getTeamId()` of the strike (`setTeamId(commander's team)`).
    pub team: i32,
    /// `iFacePosition`: where the strike was called (presentation: the target marker).
    pub i_face_position: crate::math::Vec3,
    /// `info.strike.sensorSystemIndex` (artlry.h:171): the sensor pool slot a sensor strike owns, -1 until
    /// [`set_just_created`](crate::combat::artillery::set_just_created) takes one from the pool (`SensorManager->newSensor()`).
    pub sensor_system_index: i32,
}

// -----------------------------------------------------------------------------------------------------------------------------
// Access to the mover behind a handle.

/// The shared mover fields of a mech or vehicle.
pub(crate) fn core_of(o: &Obj) -> Option<&MoverCore> {
    match &o.kind {
        Kind::Mech(m) => Some(&m.m),
        Kind::Vehicle(v) => Some(&v.m),
        _ => None,
    }
}

/// The loadout of a mech or vehicle.
pub fn loadout_of(o: &Obj) -> Option<&Loadout> {
    match &o.kind {
        Kind::Mech(m) => Some(&m.loadout),
        Kind::Vehicle(v) => Some(&v.loadout),
        _ => None,
    }
}

/// Mutable [`loadout_of`].
pub fn loadout_of_mut(o: &mut Obj) -> Option<&mut Loadout> {
    match &mut o.kind {
        Kind::Mech(m) => Some(&mut m.loadout),
        Kind::Vehicle(v) => Some(&mut v.loadout),
        _ => None,
    }
}

/// `Commander::home->getId()`: the home commander of a single-player mission (`Mission::init` loads commander 0 as home).
pub const HOME_COMMANDER: i32 = 0;

// -----------------------------------------------------------------------------------------------------------------------------
// Entry points other lanes call.

/// `Mover::fireWeapon(target, ...)` for the shot at the object a watch id names: the full result is [`fire::fire_weapon`]'s; this
/// answers whether the weapon fired (return code 0).
pub fn fire_weapon(
    w: &mut crate::World,
    attacker: ObjectHandle,
    weapon_index: usize,
    target: WatchId,
) -> bool {
    let target = w.objects.by_watch_id(target);
    fire::fire_weapon(w, attacker, weapon_index as i32, target, -1, None, 0).code == 0
}

/// `WeaponBolt::update` of one pooled bolt (called from the object update after turrets). Returns whether the bolt still exists.
pub fn update_bolt(w: &mut crate::World, h: ObjectHandle) -> bool {
    bolt_flight::update(w, h)
}

// ---- save/load (lane C). Everything is dynamic. `ammo_totals` is derived by `init_mover_stats` at spawn, but it changes with
// every shot and reload, so it is saved rather than recomputed (a load must not depend on the spawn path having run). ----

use crate::save::{Reader, SaveResult, Snapshot, Writer};

crate::snapshot_struct!(WeaponShotInfo {
    attacker,
    master_id,
    damage,
    hit_location,
    entry_angle,
});

crate::snapshot_struct!(BodyLocationState {
    cur_internal_structure,
    max_internal_structure,
    damage_state,
    critical_spaces,
    case,
    total_spaces,
});

crate::snapshot_struct!(InventoryItem {
    master_id,
    health,
    disabled,
    facing,
    start_amount,
    amount,
    ammo_index,
    ready_time,
    body_location,
    effectiveness,
});

crate::snapshot_struct!(MoverStats {
    gyro,
    actuator,
    anti_missile,
    min_range,
    max_range,
    optimal_range,
    num_functional_weapons,
    weapon_effectiveness,
    max_weapon_effectiveness,
    max_weapon_damage,
    last_weapon_effectiveness_calc,
    last_optimal_range_calc,
    torso_status,
    chassis_class,
    chassis_br,
    battle_rating,
    num_armor_components,
    cause_of_death,
    eject_order_given,
    damage_rate_tally,
    pilot_check_damage_tally,
    damage_this_frame,
    damage_after_disabled,
    num_weapon_hits_handled,
    played_critical_hit,
    mine_layer,
    mine_sweeper,
    lowest_weapon_node_id,
    lowest_weapon_node_z,
});

impl Snapshot for Loadout {
    fn save(&self, w: &mut Writer) {
        self.armor.save(w);
        self.body.save(w);
        self.inventory.save(w);
        w.len(self.ammo_totals.len());
        for &(master, cur, max) in &self.ammo_totals {
            master.save(w);
            cur.save(w);
            max.save(w);
        }
        self.counts.0.save(w);
        self.counts.1.save(w);
        self.counts.2.save(w);
        self.critical.save(w);
        self.stats.save(w);
        self.tail.save(w);
        self.item_to_inv.save(w);
    }

    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        let armor = Vec::<(f32, u8)>::load(r)?;
        let body = Vec::<BodyLocationState>::load(r)?;
        let inventory = Vec::<InventoryItem>::load(r)?;
        let n = r.length()?;
        let mut ammo_totals = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            ammo_totals.push((i32::load(r)?, i32::load(r)?, i32::load(r)?));
        }
        let counts = (u8::load(r)?, u8::load(r)?, u8::load(r)?);
        let critical = Vec::<u8>::load(r)?;
        let stats = MoverStats::load(r)?;
        let tail = Vec::<InventoryItem>::load(r)?;
        let item_to_inv = Vec::<u8>::load(r)?;
        Ok(Loadout {
            armor,
            body,
            inventory,
            ammo_totals,
            counts,
            critical,
            stats,
            tail,
            item_to_inv,
        })
    }
}

crate::snapshot_struct!(BoltState {
    shot_id,
    effect_id,
    owner,
    target,
    target_position,
    hot_spots,
    hit_target,
    time_left,
    distance_to_target,
    weapon_shot,
    laser_position,
    half_distance_to_target,
    hit_left,
    hit_effect_left,
    goal_height,
    just_created,
    has_target_position,
});

crate::snapshot_struct!(ArtilleryState {
    strike_type,
    time_to_impact,
    commander,
    bomb_run_started,
    effect_id,
    time_to_launch,
    time_to_blind,
    sensor_range,
    boom,
    whoosh,
    just_created,
    sensors_going,
    team,
    i_face_position,
    sensor_system_index,
});
