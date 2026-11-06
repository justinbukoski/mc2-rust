// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `data/missions/gamesys.fit`: the tuning tables `Mission::init` loads before it creates anything.
//!
//! Port of the reads in `Mission::init` (`code/mission.cpp:1726-1900`), `InitDifficultySettings`
//! (`mission.cpp:908`), `Mover::loadGameSystem` (`mover.cpp:1764`), `BattleMech::loadGameSystem` (`mech.cpp:501`) and
//! `GroundVehicle::loadGameSystem` (`gvehicl.cpp:700`). Each C++ global becomes a field.
//!
//! Rules kept from the C++:
//! - A required key or block that is missing is an error (the loaders return its failure code or `gosASSERT`).
//! - An optional key falls back to the default the C++ assigns (`MaxTreeLOSBlock` 5, `DefaultAttackRange` 75, ...).
//! - An array read asks for the capacity the C++ passes and keeps the elements the file declares; the rest of the
//!   fixed array stays zero, like a zero-initialised global. (`c[131] HitLocationTable` declares 131 of the 132
//!   cells; the last cell is a zero in the data.)
//! - Blocks and keys the C++ never reads are left out (`Multiplayer`, `Mech:Heat`, `LongRangeMovementEnabled`,
//!   `OverlayCellCosts`, `WeaponFireModifiers`, `SensorModifiers`, `SensorRangeMultipliers`, `SkillWeightings`, ...).
//!   `Mech:Heat` is only read under `USEHEAT`, which is never defined.
//!
//! The mission-start order is: weapon ranges, `[General]`, the master component list (which needs `BaseSensorRange`),
//! the remaining `[General]` keys, difficulty, `Mover`/`Mech`/`GroundVehicle` tables, `[Mine]`, `[Smoke]`, `[Fire]`.
//! [`GameSystem::from_fit`] reads all of them from one open file, so the order of the reads is not observable.
//!
//! Quirks reproduced:
//! - `MechClassWeights[MEDIUM]` and `[HEAVY]` both come from the key `MaxHeavyMech` (mech.cpp:512-518); `MaxMediumMech`
//!   is never read.
//! - `ClusterSizeSRM`/`ClusterSizeLRM` are read and then overwritten by the constants 2 and 5 (mover.cpp:2032-2042).
//! - `MaxVisualRadius = maxVisualRange * 1.4142` is evaluated in double and narrowed (mission.cpp:1773).
//! - `FireArc` is halved after the read (mover.cpp:1862).

use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::FormatError;

/// Path of the file inside the data root.
pub const GAMESYS_FILE: &str = "data/missions/gamesys.fit";

/// `NUM_MECH_ARMOR_LOCATIONS`.
pub const NUM_MECH_ARMOR_LOCATIONS: usize = 11;
/// `NUM_MECH_HIT_ARCS`.
pub const NUM_MECH_HIT_ARCS: usize = 4;
/// `NUM_MECH_HIT_SECTIONS`.
pub const NUM_MECH_HIT_SECTIONS: usize = 3;
/// `NUM_MECH_STATES` (mech3d.h:86).
pub const NUM_MECH_STATES: usize = 10;
/// `NUM_BODY_LOCATIONS` (cmponent.h:40).
pub const NUM_BODY_LOCATIONS: usize = 8;
/// `NUM_ATTITUDES` (tacordr.h:63).
pub const NUM_ATTITUDES: usize = 6;
/// `NUM_WARRIOR_RANKS` (warrior.h: green, regular, veteran, elite, ace).
pub const NUM_WARRIOR_RANKS: usize = 5;
/// `NUM_FIRERANGES` (tacordr.h:85): the enum runs -4..=3 so the count is 4.
pub const NUM_FIRERANGES: usize = 4;
/// `NUM_GROUNDVEHICLE_CRITICAL_HIT_TYPES` (gvehicl.h).
pub const NUM_GROUNDVEHICLE_CRITICAL_HIT_TYPES: usize = 11;
/// `MAX_GVACTOR_STATES`.
pub const MAX_GVACTOR_STATES: usize = 3;
/// Length of `MechHitLocationTable`.
pub const MECH_HIT_LOCATION_TABLE_LEN: usize =
    NUM_MECH_HIT_SECTIONS * NUM_MECH_HIT_ARCS * NUM_MECH_ARMOR_LOCATIONS;
/// Number of `visualRangeTable` entries.
pub const VISUAL_RANGE_TABLE_LEN: usize = 256;
/// `NUM_MECH_CLASSES` (mechclass.h): none, light, medium, heavy, assault.
pub const NUM_MECH_CLASS_WEIGHTS: usize = 5;

/// Ground vehicle refit cost rows (`ARMOR_REFIT_COST`, `INTERNAL_REFIT_COST`, `AMMO_REFIT_COST`) by place
/// (`REFIT_VEHICLE`, `REFIT_BAY`): `refit_costs[cost][place]`.
pub type RefitCosts = [[f32; 2]; 3];

/// `[General]` and the `[WeaponRanges]` block (mission.cpp:1726-1840).
#[derive(Debug, Clone, PartialEq)]
pub struct General {
    /// `WeaponRanges[SHORT/MEDIUM/LONG]` as `[min, max]`.
    pub weapon_ranges: [[f32; 2]; 3],
    /// `OptimalRangePoints`.
    pub optimal_range_points: [f32; 5],
    /// `OptimalRangePointInRange[i][j]`: point `i` lies in `(min, max]` of range `j` (mission.cpp:1749-1755).
    pub optimal_range_point_in_range: [[bool; 3]; 5],
    /// `maxVisualRange`.
    pub max_visual_range: f32,
    /// `MaxVisualRadius = maxVisualRange * 1.4142` (double product, narrowed).
    pub max_visual_radius: f32,
    /// `fireVisualRange`.
    pub fire_visual_range: f32,
    /// `MaxTreeLOSCellBlock`; 5 when the key is absent.
    pub max_tree_los_cell_block: u32,
    /// `WeaponRange[NUM_FIRERANGES]` (the file declares three values).
    pub weapon_range: [f32; NUM_FIRERANGES],
    /// `DefaultAttackRange`; 75 when absent.
    pub default_attack_range: f32,
    /// `baseSensorRange`.
    pub base_sensor_range: f32,
    /// `visualRangeTable[256]`.
    pub visual_range_table: Vec<i32>,
    /// `BaseHeadShotElevation`; 1 when absent.
    pub base_head_shot_elevation: f32,
    /// `forestMoveCost` (a local in `Mission::init`; it sets `PathFindMap[..]->forestCost`).
    pub forest_move_cost: i32,
    /// `MaxExtractUnitDistance`; 1280 when absent.
    pub max_extract_unit_distance: f32,
    /// `godMode`; 0 when absent.
    pub god_mode: u8,
    /// `revealTacMap` (a local); 0 when absent.
    pub reveal_tac_map: u8,
    /// `footPrints`; 1 when absent.
    pub foot_prints: u8,
    /// `tonnageDivisor` (`BonusTonnageDivisor`).
    pub tonnage_divisor: i32,
    /// `resourcesPerTonDivided` (`BonusPointsPerTon`).
    pub resources_per_ton_divided: i32,
    /// `CheatHitDamage`; 5 when absent (debug builds only in the C++, kept as data).
    pub cheat_hit_damage: f32,
}

/// `[DifficultySettings]` (mission.cpp:908): percent scalers per `GameDifficulty` 0..3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DifficultySettings {
    /// `globalPlayerSkills`.
    pub player_skills: [i32; 4],
    /// `globalEnemySkills`.
    pub enemy_skills: [i32; 4],
    /// `globalPlayerWeapons`.
    pub player_weapons: [i32; 4],
    /// `globalEnemyWeapons`.
    pub enemy_weapons: [i32; 4],
}

/// `[Pathfinding]` (mover.cpp:1771-1814).
#[derive(Debug, Clone, PartialEq)]
pub struct Pathfinding {
    /// `SimpleMovePathRange` (`SimplePathTileRange`); 21 when absent.
    pub simple_move_path_range: i32,
    /// `DelayedOrderTime`.
    pub delayed_order_time: f32,
    /// `MoveTimeOut`; 30 when absent.
    pub move_time_out: f32,
    /// `MoveYieldTime`; 1.5 when absent.
    pub move_yield_time: f32,
    /// `GroupMoveTrailLen`; `{0, 1}` when absent.
    pub group_move_trail_len: [i32; 2],
    /// `GroupOrderGoalOffset`.
    pub group_order_goal_offset: f32,
    /// `Mover::marginOfError`.
    pub margin_of_error: [f32; 2],
}

/// `[Mover:General]` (mover.cpp:1816-1888).
#[derive(Debug, Clone, PartialEq)]
pub struct MoverGeneral {
    /// `newThreatMultiplier`; 3 when absent.
    pub new_threat_multiplier: f32,
    /// `GameObject::blockCaptureRange`.
    pub block_capture_range: f32,
    /// `Mover::refitTime`.
    pub refit_time: f32,
    /// `Mover::refitRange`.
    pub refit_range: f32,
    /// `Mover::refitAmount`.
    pub refit_amount: f32,
    /// `Mover::recoverTime`.
    pub recover_time: f32,
    /// `Mover::recoverRange`.
    pub recover_range: f32,
    /// `Mover::recoverAmount`.
    pub recover_amount: f32,
    /// `Mover::recoverCost`.
    pub recover_cost: f32,
    /// `Mover::refitCostArray[cost][place]`.
    pub refit_costs: RefitCosts,
    /// `Mover::IndirectFireWeapons[20]` (`-1` padded).
    pub indirect_fire_weapons: [i32; 20],
    /// `Mover::AreaEffectWeapons[20]`.
    pub area_effect_weapons: [i32; 20],
}

/// `[Mover:FireWeapon]` (mover.cpp:1890-1952).
#[derive(Debug, Clone, PartialEq)]
pub struct MoverFireWeapon {
    /// `FireArc[3]` after the halving: mech, vehicle, elemental.
    pub fire_arc: [f32; 3],
    /// `AimedFireAbort`.
    pub aimed_fire_abort: i32,
    /// `AimedFireHitTable`.
    pub aimed_fire_hit_table: [i32; 3],
    /// `DisableAttackModifier`.
    pub disable_attack_modifier: f32,
    /// `DisableGunneryModifier`.
    pub disable_gunnery_modifier: f32,
    /// `SalvageAttackModifier`.
    pub salvage_attack_modifier: f32,
    /// `MaxStationaryTime`.
    pub max_stationary_time: f32,
    /// `MaxTimeRevealed`.
    pub max_time_revealed: f32,
    /// `HeadShotModifier`; 0.08 when absent.
    pub head_shot_modifier: f32,
    /// `ArmShotModifier`; 0.15 when absent.
    pub arm_shot_modifier: f32,
    /// `LegShotModifier`; 0.20 when absent.
    pub leg_shot_modifier: f32,
}

/// `[Mover:Damage]` and `[Components]` (mover.cpp:1954-2004).
#[derive(Debug, Clone, PartialEq)]
pub struct MoverDamage {
    /// `hitLevel`: weak/moderate thresholds.
    pub hit_level: [i32; 2],
    /// `PilotingCheckFactor`.
    pub piloting_check_factor: f32,
    /// `ClusterSizeSRM`: the C++ hard-codes 2 after reading.
    pub cluster_size_srm: i32,
    /// `ClusterSizeLRM`: the C++ hard-codes 5 after reading.
    pub cluster_size_lrm: i32,
    /// `AntiMissileSystemStats[0]` (inner sphere), `[1]` (clan).
    pub anti_missile_system_stats: [[i32; 2]; 2],
}

/// `[Warrior]` (mover.cpp:2006-2100).
#[derive(Debug, Clone, PartialEq)]
pub struct WarriorTables {
    /// `DefaultAttackRadius`; 275 when absent.
    pub default_attack_radius: f32,
    /// `WarriorRankScale[NUM_WARRIOR_RANKS]`: starts as `{50, 60, 70, 79, 999}` (warrior.cpp:237); the four values the file
    /// declares overwrite the first four, so the last stays 999.
    pub warrior_rank_scale: [f32; NUM_WARRIOR_RANKS],
    /// `PilotCheckHalfRate`.
    pub pilot_check_half_rate: f32,
    /// `PilotCheckModifierTable`.
    pub pilot_check_modifier_table: [i32; 2],
    /// `DamageRateFrequency`.
    pub damage_rate_frequency: f32,
    /// `AttitudeEffect[NUM_ATTITUDES][6]` (signed chars).
    pub attitude_effect: [[i8; 6]; NUM_ATTITUDES],
    /// `MovementUpdateFrequency`.
    pub movement_update_frequency: f32,
    /// `CombatUpdateFrequency`.
    pub combat_update_frequency: f32,
    /// `CommandUpdateFrequency`.
    pub command_update_frequency: f32,
    /// `PilotCheckUpdateFrequency`.
    pub pilot_check_update_frequency: f32,
    /// `MechWarrior::increaseCap` (`SkillIncreaseCap`).
    pub skill_increase_cap: i32,
    /// `MechWarrior::maxSkill`.
    pub skill_max: f32,
    /// `MechWarrior::minSkill`.
    pub skill_min: f32,
}

/// `[Sensors]` (mover.cpp:2102-2176).
#[derive(Debug, Clone, PartialEq)]
pub struct SensorTables {
    /// `SensorBaseChance` (`BaseSensorRollTarget`).
    pub base_chance: f32,
    /// `SensorSkillFactor`.
    pub skill_factor: f32,
    /// `SensorBlockingObjectModifier`.
    pub blocking_object_modifier: f32,
    /// `SensorShutDownMechModifier` (`ShutdownMech`).
    pub shut_down_mech_modifier: f32,
    /// `SensorRangeModifier[4][2]`.
    pub range_modifier: [[f32; 2]; 4],
    /// `SensorSizeModifier[3][2]`.
    pub size_modifier: [[f32; 2]; 3],
    /// `masterIDs[9]` (`SensorMasterIDs`), read but unused after the commented-out loop.
    pub master_ids: [i32; 9],
    /// `SensorBlockingTerrain[2]`.
    pub blocking_terrain: [f32; 2],
}

/// `[Skills]` (mover.cpp:2178-2204): the skill attempt/success tables and kill values.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillTables {
    /// `SkillTry[4]` (`Skill Attempt`): piloting, jumping, sensors, gunnery.
    pub skill_try: [f32; 4],
    /// `SkillSuccess[4]` (`Skill Success`).
    pub skill_success: [f32; 4],
    /// `WeaponHit`.
    pub weapon_hit: f32,
    /// `KillSkill[6]` (`KillSkillValues`): light, medium, heavy, assault mech, vehicle, elemental.
    pub kill_skill: [f32; 6],
    /// `SensorSkill` (`Sensor Contact Skill`).
    pub sensor_skill: f32,
}

/// `[Mech:Class]` .. `[Mech:Collision]` (mech.cpp:501-612).
#[derive(Debug, Clone, PartialEq)]
pub struct MechTables {
    /// `MechClassWeights[NUM_MECH_CLASSES]`: starts as `{0, 35, 55, 75, 100}` (mech.cpp); the loader overwrites `[LIGHT]`
    /// with `MaxLightMech` and `[MEDIUM]`, `[HEAVY]` with `MaxHeavyMech` (a C++ bug), leaving `[NONE]` 0 and `[ASSAULT]` 100.
    pub mech_class_weights: [f32; NUM_MECH_CLASS_WEIGHTS],
    /// `DefaultMechJumpCost` (`JumpCost`); `None` when the key is absent (the global keeps its initialiser).
    pub default_jump_cost: Option<i32>,
    /// `DefaultMechCrashAvoidSelf` and friends; `None` when absent.
    pub default_crash_avoid_self: Option<i32>,
    /// `DefaultMechCrashAvoidPath`.
    pub default_crash_avoid_path: Option<i32>,
    /// `DefaultMechCrashBlockSelf`.
    pub default_crash_block_self: Option<i32>,
    /// `DefaultMechCrashBlockPath`.
    pub default_crash_block_path: Option<i32>,
    /// `DefaultMechCrashYieldTime`.
    pub default_crash_yield_time: Option<f32>,
    /// `MechPilotCheckConditions`.
    pub pilot_check_conditions: [i32; 2],
    /// `MechPilotCheckTerrainEffect[64]`.
    pub pilot_check_terrain_effect: Vec<i32>,
    /// `AttackerMoveModifier[NUM_MECH_STATES]` (the file declares nine).
    pub attacker_move_modifier: [i32; NUM_MECH_STATES],
    /// `MechHitLocationTable[132]` (chars): 12 rows (3 sections x 4 arcs) of 11 armor locations.
    pub hit_location_table: Vec<i8>,
    /// `TargetMoveModifierTable[5][2]`.
    pub target_move_modifier_table: [[i32; 2]; 5],
    /// `CriticalHitTable[4]` (chars).
    pub critical_hit_table: [i8; 4],
    /// `MechTransferHitTable[8]` (chars).
    pub transfer_hit_table: [i8; NUM_BODY_LOCATIONS],
    /// `MechSalvageChance`.
    pub salvage_chance: i32,
    /// `mechCollisionThreshold`.
    pub collision_threshold: f32,
    /// `objectCollisionThreshold`.
    pub object_collision_threshold: f32,
    /// `tonnageCollisionThreshold`.
    pub tonnage_collision_threshold: f32,
    /// `treeDeflection`.
    pub tree_deflection: f32,
    /// `mechPivotAngle`.
    pub pivot_angle: f32,
    /// `mechPivotThrottle`.
    pub pivot_throttle: f32,
}

/// `[GroundVehicle:*]` (gvehicl.cpp:700-767).
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleTables {
    /// `GroundVehicleAttackerMoveModifier[MAX_GVACTOR_STATES]`.
    pub attacker_move_modifier: [i32; MAX_GVACTOR_STATES],
    /// `GroundVehicleCriticalHitTable[11]`.
    pub critical_hit_table: [i32; NUM_GROUNDVEHICLE_CRITICAL_HIT_TYPES],
    /// `gvCollisionThreshold`.
    pub collision_threshold: f32,
    /// `gvObjectCollisionThreshold`.
    pub object_collision_threshold: f32,
    /// `gvTonnageCollisionThreshold`.
    pub tonnage_collision_threshold: f32,
    /// `gvTreeDeflection`.
    pub tree_deflection: f32,
    /// `DefaultGroundVehicleCrashAvoidSelf` etc.; `None` when absent.
    pub default_crash_avoid_self: Option<i32>,
    /// See above.
    pub default_crash_avoid_path: Option<i32>,
    /// See above.
    pub default_crash_block_self: Option<i32>,
    /// See above.
    pub default_crash_block_path: Option<i32>,
    /// See above.
    pub default_crash_yield_time: Option<f32>,
    /// `gvSweepTime` (`SweeperSlowTime`).
    pub sweep_time: f32,
    /// `gvWalkSpeed`.
    pub walk_speed: f32,
    /// `gvHillSpeedFactor`.
    pub hill_speed_factor: f32,
}

/// `[Mine]`, `[Smoke]`, `[Fire]` (mission.cpp:1857-1925).
#[derive(Debug, Clone, PartialEq)]
pub struct MineTables {
    /// `MineDamage` (`BaseDamage`).
    pub mine_damage: f32,
    /// `MineSplashDamage`.
    pub mine_splash_damage: f32,
    /// `MineSplashRange`.
    pub mine_splash_range: f32,
    /// `MineExplosion`: object type of the mine explosion.
    pub mine_explosion: i32,
    /// `MineLayThrottle`; 50 when absent.
    pub mine_lay_throttle: i32,
    /// `MineSweepThrottle`; 50 when absent.
    pub mine_sweep_throttle: i32,
    /// `MineWaitTime`.
    pub mine_wait_time: f32,
    /// `StrikeWaitTime`.
    pub strike_wait_time: f32,
    /// `StrikeTimeToImpact`.
    pub strike_time_to_impact: f32,
    /// `totalSmokeSpheres` (`MaxSmokeSpheres`).
    pub total_smoke_spheres: i32,
    /// `totalSmokeShapeSize`.
    pub total_smoke_shape_size: i32,
    /// `maxFiresBurning`.
    pub max_fires_burning: i32,
    /// `maxFireBurnTime`.
    pub max_fire_burn_time: f32,
}

/// Everything `Mission::init` reads from `gamesys.fit`.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSystem {
    /// `[WeaponRanges]` and `[General]`.
    pub general: General,
    /// `[DifficultySettings]`.
    pub difficulty: DifficultySettings,
    /// `[Pathfinding]`.
    pub pathfinding: Pathfinding,
    /// `[Mover:General]`.
    pub mover_general: MoverGeneral,
    /// `[Mover:FireWeapon]`.
    pub mover_fire: MoverFireWeapon,
    /// `[Mover:Damage]`, `[Components]`.
    pub mover_damage: MoverDamage,
    /// `[Warrior]`.
    pub warrior: WarriorTables,
    /// `[Sensors]`.
    pub sensors: SensorTables,
    /// `[Skills]`.
    pub skills: SkillTables,
    /// `[Mech:*]`.
    pub mech: MechTables,
    /// `[GroundVehicle:*]`.
    pub vehicle: VehicleTables,
    /// `[Mine]`, `[Smoke]`, `[Fire]`.
    pub mine: MineTables,
}

// ---------------------------------------------------------------------------------------------------------------------
// reading helpers

struct R<'a> {
    fit: &'a mut FitIni,
    block: String,
}

fn missing<T>(r: Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(SimDataError::Format(FormatError::NotFound { .. })) => Ok(None),
        Err(e) => Err(e),
    }
}

impl<'a> R<'a> {
    fn new(fit: &'a mut FitIni) -> R<'a> {
        R {
            fit,
            block: String::new(),
        }
    }

    fn seek(&mut self, block: &str) -> Result<()> {
        self.block = block.to_string();
        self.fit.seek_block(block).map_err(|e| match e {
            FormatError::NotFound { .. } => {
                SimDataError::schema(GAMESYS_FILE, format!("missing block [{block}]"))
            }
            other => other.into(),
        })
    }

    fn need_msg(&self, key: &str) -> SimDataError {
        SimDataError::schema(
            GAMESYS_FILE,
            format!("[{}] is missing key {key}", self.block),
        )
    }

    fn get<T>(&self, key: &str, v: mc2_formats::Result<T>) -> Result<T> {
        match v {
            Ok(x) => Ok(x),
            Err(FormatError::NotFound { .. }) => Err(self.need_msg(key)),
            Err(e) => Err(e.into()),
        }
    }

    fn opt<T>(&self, v: mc2_formats::Result<T>) -> Result<Option<T>> {
        missing(v.map_err(SimDataError::from))
    }

    fn f32(&self, key: &str) -> Result<f32> {
        self.get(key, self.fit.read_f32(key))
    }
    fn f32_or(&self, key: &str, default: f32) -> Result<f32> {
        Ok(self.opt(self.fit.read_f32(key))?.unwrap_or(default))
    }
    fn i32(&self, key: &str) -> Result<i32> {
        self.get(key, self.fit.read_i32(key))
    }
    fn opt_i32(&self, key: &str) -> Result<Option<i32>> {
        self.opt(self.fit.read_i32(key))
    }
    fn opt_f32(&self, key: &str) -> Result<Option<f32>> {
        self.opt(self.fit.read_f32(key))
    }

    /// Required array. `cap` is the element capacity the C++ passes; the returned vector holds the declared elements.
    fn f32s<const N: usize>(&self, key: &str) -> Result<[f32; N]> {
        self.f32s_over(key, [0.0; N])
    }
    /// Like [`f32s`](Self::f32s) but the elements the file does not declare keep the values of `base` (the C++
    /// global's initialiser).
    fn f32s_over<const N: usize>(&self, key: &str, mut base: [f32; N]) -> Result<[f32; N]> {
        let v = self.get(key, self.fit.read_array::<f32>(key, N))?;
        base[..v.len()].copy_from_slice(&v);
        Ok(base)
    }
    fn i32s<const N: usize>(&self, key: &str) -> Result<[i32; N]> {
        let v = self.get(key, self.fit.read_array::<i32>(key, N))?;
        let mut out = [0; N];
        out[..v.len()].copy_from_slice(&v);
        Ok(out)
    }
    fn opt_i32s<const N: usize>(&self, key: &str) -> Result<Option<[i32; N]>> {
        match self.opt(self.fit.read_array::<i32>(key, N))? {
            None => Ok(None),
            Some(v) => {
                let mut out = [0; N];
                out[..v.len()].copy_from_slice(&v);
                Ok(Some(out))
            }
        }
    }
    fn i8s<const N: usize>(&self, key: &str) -> Result<[i8; N]> {
        let v = self.get(key, self.fit.read_array::<i8>(key, N))?;
        let mut out = [0; N];
        out[..v.len()].copy_from_slice(&v);
        Ok(out)
    }
}

impl General {
    fn read(r: &mut R<'_>) -> Result<General> {
        r.seek("WeaponRanges")?;
        let short = r.f32s::<2>("Short")?;
        let medium = r.f32s::<2>("Medium")?;
        let long = r.f32s::<2>("Long")?;
        let weapon_ranges = [short, medium, long];
        let optimal_range_points = r.f32s::<5>("OptimalRangePoints")?;
        let mut in_range = [[false; 3]; 5];
        for (i, row) in in_range.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                if optimal_range_points[i] > weapon_ranges[j][0]
                    && optimal_range_points[i] <= weapon_ranges[j][1]
                {
                    *cell = true;
                }
            }
        }

        r.seek("General")?;
        let max_visual_range = r.f32("MaxVisualRange")?;
        // The literal is the C++ constant 1.4142, not sqrt(2).
        #[allow(clippy::approx_constant)]
        let max_visual_radius = (f64::from(max_visual_range) * 1.4142) as f32;
        let fire_visual_range = r.f32("FireVisualRange")?;
        let max_tree_los_cell_block = r.opt(r.fit.read_u32("MaxTreeLOSBlock"))?.unwrap_or(5);
        let weapon_range = r.f32s::<NUM_FIRERANGES>("WeaponRange")?;
        let default_attack_range = r.f32_or("DefaultAttackRange", 75.0)?;
        let base_sensor_range = r.f32("BaseSensorRange")?;
        let table = r.i32s::<VISUAL_RANGE_TABLE_LEN>("VisualRangeTable")?;
        let base_head_shot_elevation = r.f32_or("BaseHeadShotElevation", 1.0)?;
        let forest_move_cost = r.i32("ForestMoveCost")?;
        let max_extract_unit_distance = r.f32_or("MaxUnitExtractDistance", 1280.0)?;
        let god_mode = r.opt(r.fit.read_u8("GodMode"))?.unwrap_or(0);
        let reveal_tac_map = r.opt(r.fit.read_u8("RevealTacMap"))?.unwrap_or(0);
        let foot_prints = r.opt(r.fit.read_u8("FootPrints"))?.unwrap_or(1);
        let tonnage_divisor = r.i32("BonusTonnageDivisor")?;
        let resources_per_ton_divided = r.i32("BonusPointsPerTon")?;
        let cheat_hit_damage = r.f32_or("CheatHitDamage", 5.0)?;
        Ok(General {
            weapon_ranges,
            optimal_range_points,
            optimal_range_point_in_range: in_range,
            max_visual_range,
            max_visual_radius,
            fire_visual_range,
            max_tree_los_cell_block,
            weapon_range,
            default_attack_range,
            base_sensor_range,
            visual_range_table: table.to_vec(),
            base_head_shot_elevation,
            forest_move_cost,
            max_extract_unit_distance,
            god_mode,
            reveal_tac_map,
            foot_prints,
            tonnage_divisor,
            resources_per_ton_divided,
            cheat_hit_damage,
        })
    }
}

impl DifficultySettings {
    fn read(r: &mut R<'_>) -> Result<DifficultySettings> {
        r.seek("DifficultySettings")?;
        Ok(DifficultySettings {
            player_skills: r.i32s::<4>("PlayerSkills")?,
            enemy_skills: r.i32s::<4>("EnemySkills")?,
            player_weapons: r.i32s::<4>("PlayerWeapons")?,
            enemy_weapons: r.i32s::<4>("EnemyWeapons")?,
        })
    }
}

impl Pathfinding {
    fn read(r: &mut R<'_>) -> Result<Pathfinding> {
        r.seek("Pathfinding")?;
        // `SimpleMovePathRange = 21` (move.cpp:82) when the key is absent.
        let simple_move_path_range = r.opt_i32("SimplePathTileRange")?.unwrap_or(21);
        let delayed_order_time = r.f32("DelayedOrderTime")?;
        let move_time_out = r.f32_or("MoveTimeOut", 30.0)?;
        let move_yield_time = r.f32_or("MoveYieldTime", 1.5)?;
        let group_move_trail_len = r.opt_i32s::<2>("GroupMoveTrailLength")?.unwrap_or([0, 1]);
        let group_order_goal_offset = r.f32("GroupOrderGoalOffset")?;
        let margin_of_error = r.f32s::<2>("MoveMarginOfError")?;
        Ok(Pathfinding {
            simple_move_path_range,
            delayed_order_time,
            move_time_out,
            move_yield_time,
            group_move_trail_len,
            group_order_goal_offset,
            margin_of_error,
        })
    }
}

impl MoverGeneral {
    fn read(r: &mut R<'_>) -> Result<MoverGeneral> {
        r.seek("Mover:General")?;
        let new_threat_multiplier = r.f32_or("NewThreatMultiplier", 3.0)?;
        let block_capture_range = r.f32("BlockCaptureRange")?;
        let refit_time = r.f32("RefitTime")?;
        let refit_range = r.f32("RefitRange")?;
        let refit_amount = r.f32("RefitAmount")?;
        let mut refit_costs = [[0.0f32; 2]; 3];
        refit_costs[0][0] = r.f32("RefitVehicleArmorCost")?;
        let recover_time = r.f32("RecoverTime")?;
        let recover_range = r.f32("RecoverRange")?;
        let recover_amount = r.f32("RecoverAmount")?;
        let recover_cost = r.f32("RecoverCost")?;
        refit_costs[1][0] = r.f32("RefitVehicleInternalCost")?;
        refit_costs[2][0] = r.f32("RefitVehiclePointsToAmmo")?;
        refit_costs[0][1] = r.f32("RefitBayArmorCost")?;
        refit_costs[1][1] = r.f32("RefitBayInternalCost")?;
        refit_costs[2][1] = r.f32("RefitBayAmmoCost")?;
        let indirect_fire_weapons = r.i32s::<20>("IndirectFireWeaponComponentIndex")?;
        let area_effect_weapons = r.i32s::<20>("AreaEffectWeaponComponentIndex")?;
        Ok(MoverGeneral {
            new_threat_multiplier,
            block_capture_range,
            refit_time,
            refit_range,
            refit_amount,
            recover_time,
            recover_range,
            recover_amount,
            recover_cost,
            refit_costs,
            indirect_fire_weapons,
            area_effect_weapons,
        })
    }
}

impl MoverFireWeapon {
    fn read(r: &mut R<'_>) -> Result<MoverFireWeapon> {
        r.seek("Mover:FireWeapon")?;
        let mut fire_arc = r.f32s::<3>("FireArc")?;
        for a in &mut fire_arc {
            *a /= 2.0;
        }
        Ok(MoverFireWeapon {
            fire_arc,
            aimed_fire_abort: r.i32("AimedFireAbort")?,
            aimed_fire_hit_table: r.i32s::<3>("AimedFireHitTable")?,
            disable_attack_modifier: r.f32("DisableAttackModifier")?,
            disable_gunnery_modifier: r.f32("DisableGunneryModifier")?,
            salvage_attack_modifier: r.f32("SalvageAttackModifier")?,
            max_stationary_time: r.f32("MaxStationaryTime")?,
            max_time_revealed: r.f32("MaxTimeRevealed")?,
            head_shot_modifier: r.f32_or("HeadShotModifier", 0.08)?,
            arm_shot_modifier: r.f32_or("ArmShotModifier", 0.15)?,
            leg_shot_modifier: r.f32_or("LegShotModifier", 0.20)?,
        })
    }
}

impl MoverDamage {
    fn read(r: &mut R<'_>) -> Result<MoverDamage> {
        r.seek("Mover:Damage")?;
        let hit_level = r.i32s::<2>("HitLevel")?;
        let piloting_check_factor = r.f32("PilotingCheckFactor")?;
        r.seek("Components")?;
        // The values are read (and must exist) but the C++ then hard-codes them.
        let _ = r.i32("ClusterSizeSRM")?;
        let _ = r.i32("ClusterSizeLRM")?;
        let inner = r.i32s::<2>("InnerSphereAntiMissile")?;
        let clan = r.i32s::<2>("ClanAntiMissile")?;
        Ok(MoverDamage {
            hit_level,
            piloting_check_factor,
            cluster_size_srm: 2,
            cluster_size_lrm: 5,
            anti_missile_system_stats: [inner, clan],
        })
    }
}

impl WarriorTables {
    fn read(r: &mut R<'_>) -> Result<WarriorTables> {
        r.seek("Warrior")?;
        let default_attack_radius = r.f32_or("DefaultAttackRadius", 275.0)?;
        let warrior_rank_scale =
            r.f32s_over("WarriorRankScale", [50.0, 60.0, 70.0, 79.0, 999.0])?;
        let pilot_check_half_rate = r.f32("PilotCheckHalfRate")?;
        let pilot_check_modifier_table = r.i32s::<2>("PilotCheckModifiers")?;
        let damage_rate_frequency = r.f32("DamageRateFrequency")?;
        let flat = r.i8s::<{ NUM_ATTITUDES * 6 }>("AttitudeEffect")?;
        let mut attitude_effect = [[0i8; 6]; NUM_ATTITUDES];
        for (i, row) in attitude_effect.iter_mut().enumerate() {
            row.copy_from_slice(&flat[i * 6..i * 6 + 6]);
        }
        let movement_update_frequency = r.f32("MovementUpdateFrequency")?;
        let combat_update_frequency = r.f32("CombatUpdateFrequency")?;
        let command_update_frequency = r.f32("CommandUpdateFrequency")?;
        let pilot_check_update_frequency = r.f32("PilotCheckUpdateFrequency")?;
        let skill_increase_cap = r.i32("SkillIncreaseCap")?;
        let skill_max = r.f32("SkillMax")?;
        let skill_min = r.f32("SkillMin")?;
        Ok(WarriorTables {
            default_attack_radius,
            warrior_rank_scale,
            pilot_check_half_rate,
            pilot_check_modifier_table,
            damage_rate_frequency,
            attitude_effect,
            movement_update_frequency,
            combat_update_frequency,
            command_update_frequency,
            pilot_check_update_frequency,
            skill_increase_cap,
            skill_max,
            skill_min,
        })
    }
}

impl SensorTables {
    fn read(r: &mut R<'_>) -> Result<SensorTables> {
        r.seek("Sensors")?;
        let base_chance = r.f32("BaseSensorRollTarget")?;
        let skill_factor = r.f32("SensorSkillFactor")?;
        let blocking_object_modifier = r.f32("BlockingObjectModifier")?;
        let shut_down_mech_modifier = r.f32("ShutdownMech")?;
        let range = r.f32s::<8>("SensorRangeModifier")?;
        let mut range_modifier = [[0.0; 2]; 4];
        for (i, row) in range_modifier.iter_mut().enumerate() {
            *row = [range[i * 2], range[i * 2 + 1]];
        }
        let size = r.f32s::<6>("SizeModifier")?;
        let mut size_modifier = [[0.0; 2]; 3];
        for (i, row) in size_modifier.iter_mut().enumerate() {
            *row = [size[i * 2], size[i * 2 + 1]];
        }
        let master_ids = r.i32s::<9>("SensorMasterIDs")?;
        let blocking_terrain = r.f32s::<2>("BlockingTerrainModifiers")?;
        Ok(SensorTables {
            base_chance,
            skill_factor,
            blocking_object_modifier,
            shut_down_mech_modifier,
            range_modifier,
            size_modifier,
            master_ids,
            blocking_terrain,
        })
    }
}

impl SkillTables {
    fn read(r: &mut R<'_>) -> Result<SkillTables> {
        r.seek("Skills")?;
        Ok(SkillTables {
            skill_try: r.f32s::<4>("Skill Attempt")?,
            skill_success: r.f32s::<4>("Skill Success")?,
            weapon_hit: r.f32("WeaponHit")?,
            kill_skill: r.f32s::<6>("KillSkillValues")?,
            sensor_skill: r.f32("Sensor Contact Skill")?,
        })
    }
}

impl MechTables {
    fn read(r: &mut R<'_>) -> Result<MechTables> {
        r.seek("Mech:Class")?;
        let light = r.f32("MaxLightMech")?;
        // mech.cpp:512-518: both the medium and the heavy slot read "MaxHeavyMech".
        let heavy = r.f32("MaxHeavyMech")?;
        let mech_class_weights = [0.0, light, heavy, heavy, 100.0];

        r.seek("Mech:Movement")?;
        let default_jump_cost = r.opt_i32("JumpCost")?;
        let default_crash_avoid_self = r.opt_i32("CrashAvoidSelf")?;
        let default_crash_avoid_path = r.opt_i32("CrashAvoidPath")?;
        let default_crash_block_self = r.opt_i32("CrashBlockSelf")?;
        let default_crash_block_path = r.opt_i32("CrashBlockPath")?;
        let default_crash_yield_time = r.opt_f32("CrashYieldTime")?;
        let pilot_check_conditions = r.i32s::<2>("PilotCheckConditions")?;
        let terrain = r.i32s::<64>("PilotCheckTerrainEffect")?;

        r.seek("Mech:FireWeapon")?;
        let attacker_move_modifier = r.i32s::<NUM_MECH_STATES>("AttackerMoveModifier")?;
        let hit_location_table = r
            .i8s::<MECH_HIT_LOCATION_TABLE_LEN>("HitLocationTable")?
            .to_vec();
        let flat = r.i32s::<10>("TargetMoveModifierTable")?;
        let mut target_move_modifier_table = [[0; 2]; 5];
        for (i, row) in target_move_modifier_table.iter_mut().enumerate() {
            *row = [flat[2 * i], flat[2 * i + 1]];
        }

        r.seek("Mech:Damage")?;
        let critical_hit_table = r.i8s::<4>("CriticalHitTable")?;
        let transfer_hit_table = r.i8s::<NUM_BODY_LOCATIONS>("MechTransferHitTable")?;
        let salvage_chance = r.i32("MechSalvageChance")?;

        r.seek("Mech:Collision")?;
        Ok(MechTables {
            mech_class_weights,
            default_jump_cost,
            default_crash_avoid_self,
            default_crash_avoid_path,
            default_crash_block_self,
            default_crash_block_path,
            default_crash_yield_time,
            pilot_check_conditions,
            pilot_check_terrain_effect: terrain.to_vec(),
            attacker_move_modifier,
            hit_location_table,
            target_move_modifier_table,
            critical_hit_table,
            transfer_hit_table,
            salvage_chance,
            collision_threshold: r.f32("collisionThreshold")?,
            object_collision_threshold: r.f32("objectThreshold")?,
            tonnage_collision_threshold: r.f32("tonnageThreshold")?,
            tree_deflection: r.f32("treeDeflection")?,
            pivot_angle: r.f32("pivotAngle")?,
            pivot_throttle: r.f32("pivotThrottle")?,
        })
    }
}

impl VehicleTables {
    fn read(r: &mut R<'_>) -> Result<VehicleTables> {
        r.seek("GroundVehicle:FireWeapon")?;
        let attacker_move_modifier = r.i32s::<MAX_GVACTOR_STATES>("AttackerMoveModifier")?;
        r.seek("GroundVehicle:Damage")?;
        let critical_hit_table =
            r.i32s::<NUM_GROUNDVEHICLE_CRITICAL_HIT_TYPES>("CriticalHitTable")?;
        r.seek("GroundVehicle:Collision")?;
        let collision_threshold = r.f32("collisionThreshold")?;
        let object_collision_threshold = r.f32("objectThreshold")?;
        let tonnage_collision_threshold = r.f32("tonnageThreshold")?;
        let tree_deflection = r.f32("treeDeflection")?;
        r.seek("GroundVehicle:Movement")?;
        Ok(VehicleTables {
            attacker_move_modifier,
            critical_hit_table,
            collision_threshold,
            object_collision_threshold,
            tonnage_collision_threshold,
            tree_deflection,
            default_crash_avoid_self: r.opt_i32("CrashAvoidSelf")?,
            default_crash_avoid_path: r.opt_i32("CrashAvoidPath")?,
            default_crash_block_self: r.opt_i32("CrashBlockSelf")?,
            default_crash_block_path: r.opt_i32("CrashBlockPath")?,
            default_crash_yield_time: r.opt_f32("CrashYieldTime")?,
            sweep_time: r.f32("SweeperSlowTime")?,
            walk_speed: r.f32("WalkSpeed")?,
            hill_speed_factor: r.f32("HillSpeedFactor")?,
        })
    }
}

impl MineTables {
    fn read(r: &mut R<'_>) -> Result<MineTables> {
        r.seek("Mine")?;
        let mine_damage = r.f32("BaseDamage")?;
        let mine_splash_damage = r.f32("SplashDamage")?;
        let mine_splash_range = r.f32("SplashRange")?;
        let mine_explosion = r.i32("Explosion")?;
        let mine_lay_throttle = r.opt_i32("MineLayThrottle")?.unwrap_or(50);
        let mine_sweep_throttle = r.opt_i32("MineSweepThrottle")?.unwrap_or(50);
        let mine_wait_time = r.f32("MineWaitTime")?;
        let strike_wait_time = r.f32("StrikeWaitTime")?;
        let strike_time_to_impact = r.f32("StrikeTimeToImpact")?;
        r.seek("Smoke")?;
        let total_smoke_spheres = r.i32("MaxSmokeSpheres")?;
        let total_smoke_shape_size = r.i32("TotalSmokeShapeSize")?;
        r.seek("Fire")?;
        let max_fires_burning = r.i32("MaxFiresBurning")?;
        let max_fire_burn_time = r.f32("MaxFireBurnTime")?;
        Ok(MineTables {
            mine_damage,
            mine_splash_damage,
            mine_splash_range,
            mine_explosion,
            mine_lay_throttle,
            mine_sweep_throttle,
            mine_wait_time,
            strike_wait_time,
            strike_time_to_impact,
            total_smoke_spheres,
            total_smoke_shape_size,
            max_fires_burning,
            max_fire_burn_time,
        })
    }
}

impl GameSystem {
    /// Reads every table from an open `gamesys.fit`, in the order `Mission::init` reads them.
    pub fn from_fit(fit: &mut FitIni) -> Result<GameSystem> {
        let mut r = R::new(fit);
        let general = General::read(&mut r)?;
        let difficulty = DifficultySettings::read(&mut r)?;
        let pathfinding = Pathfinding::read(&mut r)?;
        let mover_general = MoverGeneral::read(&mut r)?;
        let mover_fire = MoverFireWeapon::read(&mut r)?;
        let mover_damage = MoverDamage::read(&mut r)?;
        let warrior = WarriorTables::read(&mut r)?;
        let sensors = SensorTables::read(&mut r)?;
        let skills = SkillTables::read(&mut r)?;
        let mech = MechTables::read(&mut r)?;
        let vehicle = VehicleTables::read(&mut r)?;
        let mine = MineTables::read(&mut r)?;
        Ok(GameSystem {
            general,
            difficulty,
            pathfinding,
            mover_general,
            mover_fire,
            mover_damage,
            warrior,
            sensors,
            skills,
            mech,
            vehicle,
            mine,
        })
    }

    /// Loads `data/missions/gamesys.fit` through the VFS.
    pub fn load(vfs: &mut mc2_formats::vfs::Vfs) -> Result<GameSystem> {
        let mut fit = vfs
            .read_fit(GAMESYS_FILE)?
            .ok_or_else(|| SimDataError::MissingFile {
                name: GAMESYS_FILE.to_string(),
            })?;
        GameSystem::from_fit(&mut fit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_points_in_range_follow_the_half_open_test() {
        let weapon_ranges = [[0.0f32, 60.0], [30.0, 120.0], [60.0, 180.0]];
        let points = [20.0f32, 45.0, 100.0, 100.0, 160.0];
        let mut got = [[false; 3]; 5];
        for i in 0..5 {
            for j in 0..3 {
                got[i][j] = points[i] > weapon_ranges[j][0] && points[i] <= weapon_ranges[j][1];
            }
        }
        assert_eq!(got[0], [true, false, false]);
        assert_eq!(got[1], [true, true, false]);
        assert_eq!(got[4], [false, false, true]);
    }

    #[test]
    fn missing_block_is_a_schema_error() {
        let mut fit =
            FitIni::from_bytes(b"FITini\n[General]\nf MaxVisualRange = 1\n".to_vec()).unwrap();
        let err = GameSystem::from_fit(&mut fit).unwrap_err();
        assert!(err.to_string().contains("WeaponRanges"), "{err}");
    }

    /// A `[Pathfinding]` block with the keys the reader needs, and `range` as its `SimplePathTileRange` line (none when `None`). The
    /// block is followed by another one because the reader does not find a key on the last line of the file.
    fn pathfinding_fit(range: Option<&str>) -> Vec<u8> {
        let range = range.map_or(String::new(), |v| format!("l SimplePathTileRange = {v}\n"));
        format!(
            "FITini\n[Pathfinding]\nf[2] MoveMarginOfError = 5.0, 10.0\n{range}f DelayedOrderTime = 2.5\nf GroupOrderGoalOffset = 1.0\n[Next]\ni Other = 0\n"
        )
        .into_bytes()
    }

    #[test]
    fn simple_move_path_range_is_read_from_the_data() {
        let mut fit = FitIni::from_bytes(pathfinding_fit(Some("30"))).unwrap();
        let mut r = R::new(&mut fit);
        let p = Pathfinding::read(&mut r).unwrap();
        assert_eq!(p.simple_move_path_range, 30);
    }

    #[test]
    fn simple_move_path_range_defaults_to_21_when_the_key_is_absent() {
        let mut fit = FitIni::from_bytes(pathfinding_fit(None)).unwrap();
        let mut r = R::new(&mut fit);
        let p = Pathfinding::read(&mut r).unwrap();
        assert_eq!(p.simple_move_path_range, 21);
    }

    #[test]
    fn short_array_declaration_fills_the_prefix() {
        let mut fit = FitIni::from_bytes(
            b"FITini\n[A]\nf[3] WeaponRange = 60, 140, 200\nl[2] X = 4, 5\n".to_vec(),
        )
        .unwrap();
        let mut r = R::new(&mut fit);
        r.seek("A").unwrap();
        assert_eq!(
            r.f32s::<4>("WeaponRange").unwrap(),
            [60.0, 140.0, 200.0, 0.0]
        );
        // A declaration larger than the capacity is an error (USER_ARRAY_TOO_SMALL).
        assert!(r.i32s::<1>("X").is_err());
        assert_eq!(r.opt_i32s::<3>("Y").unwrap(), None);
    }
}
