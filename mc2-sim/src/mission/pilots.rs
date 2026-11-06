// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Pilot loading for `Mission::init` (mission.cpp:2310-2380): `MechWarrior::setup`, `newWarrior`, `MechWarrior::init(FitIniFile*)`,
//! `setBrain`, `loadBrainParameters`.
//!
//! The pool is the 120 slots of lane D ([`PilotPool`]). Slot 0 is never handed out; `[WarriorN]` of the mission file gets slot N
//! because `newWarrior` returns the lowest unused slot and the loop runs N = 1.. in order. A part's `Pilot` key is that slot.
//!
//! RNG: `MechWarrior::init` draws ONE `RandomNumber(100)` per pilot after reading the profile (warrior.cpp:1203), whether or not
//! the pilot is a toughness specialist, in slot order. The result decides `escapesThruEjection`:
//! `roll <= piloting + 30 && roll < 95`, or always true for a toughness specialist (and in multiplayer, not ported).
//!
//! The profile data lane E reads but lane D's [`Pilot`] has no field for (rank, skill points, personality, brain name, ...) is kept
//! in [`PilotLoad`] next to the pool, indexed by slot.

use std::rc::Rc;

use crate::ai::pilot::MAX_WARRIORS;
use crate::ai::{Pilot, PilotPool};
use crate::command::TacticalOrder;
use crate::ids::{ObjectHandle, WatchId};
use crate::rng::SimRng;
use mc2_sim_data::mission::{CellMemory, Warrior};
use mc2_sim_data::pilot::{Skill, WarriorProfile, WarriorRank};

/// `NUM_MEMORY_CELLS` (warrior.h:627).
pub const NUM_MEMORY_CELLS: usize = 60;

/// `DefaultAttackRadius` (warrior.cpp:293 = 275.0): what a pilot gets when gamesys.fit is absent or lacks the key.
pub const DEFAULT_ATTACK_RADIUS: f32 = 275.0;

/// What `MechWarrior::init(FitIniFile*)` and `Mission::init` store about a pilot that [`Pilot`] has no field for.
#[derive(Clone, Debug)]
pub struct PilotLoad {
    /// `used`: the slot was handed out by `newWarrior` and not freed.
    pub used: bool,
    /// The profile the pilot was read from.
    pub profile: Option<Rc<WarriorProfile>>,
    /// `brainStr`: the `Brain` key of `[WarriorN]` (the module name, case as written).
    pub brain_name: String,
    /// `rank` (`calcRank`).
    pub rank: Option<WarriorRank>,
    /// `skillRank[]` (floats, start equal to the skills) by [`Skill::index`].
    pub skill_rank: [f32; 3],
    /// `skillPoints[]`.
    pub skill_points: [f32; 3],
    /// `originalSkills[]`.
    pub original_skills: [i8; 3],
    /// `startingSkills[]`.
    pub starting_skills: [i8; 3],
    /// `professionalism, decorum, aggressiveness, courage` (`baseCourage` equals `courage`).
    pub personality: [i8; 4],
    /// `teamId` (`MechWarrior::teamId`, set by `Mover::setTeamId`).
    pub team: i32,
    /// `paintScheme`.
    pub paint_scheme: i64,
    /// `notMineYet`.
    pub not_mine_yet: bool,
    /// `descID`, `nameIndex`, `photoIndex`.
    pub ids: [i64; 3],
}

impl Default for PilotLoad {
    /// `MechWarrior::init(bool)` / `clear()` (warrior.cpp:854-876, 1377-1397): green rank, every personality trait 40, no profile.
    fn default() -> Self {
        PilotLoad {
            used: false,
            profile: None,
            brain_name: String::new(),
            rank: Some(WarriorRank::Green),
            skill_rank: [0.0; 3],
            skill_points: [0.0; 3],
            original_skills: [0; 3],
            starting_skills: [0; 3],
            personality: [40; 4],
            team: 0,
            paint_scheme: 0,
            not_mine_yet: false,
            ids: [0; 3],
        }
    }
}

/// `MechWarrior::freeWarrior` -> `clear()` (warrior.cpp:7539, 1359-1450): a pilot returned to the pool loses its profile, skills, rank,
/// personality, wounds, specialties, memory cells and mover link. The start-up stagger timers, the compiled brain and the pool index
/// stay (`brainUpdate` & co. are commented out of `clear`; `brain->init(-1)` re-initialises the module, which this port does not model
/// beyond keeping the instance).
pub fn clear_slot(
    pool: &mut PilotPool,
    loads: &mut [PilotLoad],
    slot: usize,
    default_attack_radius: f32,
) {
    loads[slot] = PilotLoad::default();
    let p = &mut pool.slots[slot];
    p.mover = WatchId::NONE;
    p.wounds = 0.0;
    // `skills[]` itself is not touched by clear() (skillRank, skillPoints, originalSkills and startingSkills are)
    p.specialty = [false; 23];
    p.escapes_thru_ejection = false;
    p.attack_radius = default_attack_radius;
    p.memory = vec![0; NUM_MEMORY_CELLS];
    // clear(): tacOrder[3].init(), curTacOrder.init() (warrior.cpp:1460-1464)
    p.tac_orders = std::array::from_fn(|_| TacticalOrder::init());
    p.cur_order = TacticalOrder::init();
}

/// `MechWarrior::setup` (warrior.cpp:7512): 120 pilots, `index = i`, not used, with the start-up stagger of the three update
/// timers. `MechWarrior::init(bool)` computes them from the running count `numWarriors`, which is the pool index while the pool is
/// being built (`(float)(n % 30) * 0.2` is a double product narrowed to float).
pub fn setup_pool(pool: &mut PilotPool, default_attack_radius: f32) {
    pool.slots = (0..MAX_WARRIORS)
        .map(|i| {
            let n = i as i32;
            Pilot::fresh(
                i as u16,
                default_attack_radius,
                [
                    ((n % 30) as f32 as f64 * 0.2) as f32,
                    ((n % 15) as f32 as f64 * 0.1) as f32,
                    ((n % 15) as f32 as f64 * 0.2) as f32,
                ],
            )
        })
        .collect();
    pool.brains_enabled = [true; crate::ids::MAX_TEAMS];
}

/// `MechWarrior::newWarrior`: the lowest unused slot from 1.
pub fn new_warrior(loads: &mut [PilotLoad]) -> Option<usize> {
    for (i, l) in loads.iter_mut().enumerate().skip(1) {
        if !l.used {
            l.used = true;
            return Some(i);
        }
    }
    None
}

/// `escapesThruEjection` (warrior.cpp:1203-1206) for a roll already drawn.
pub fn escapes_thru_ejection(roll: i32, piloting: i32, toughness_specialist: bool) -> bool {
    let mut escapes = roll <= piloting + 30 && roll < 95;
    if toughness_specialist {
        escapes = true;
    }
    escapes
}

/// `MAX_MOVERS` (mclib/dmovemgr.h:16); `MechWarrior::killed[]` holds `MAX_MOVERS / 3` watch ids.
pub const MAX_KILLED: usize = 255 / 3;

/// The `VehicleClass` indices of `numMechKills` (mechclass.h: `VEHICLE_CLASS_LIGHTMECH` = 1 ... `VEHICLE_CLASS_GROUND` = 5,
/// `VEHICLE_CLASS_ELEMENTAL` = 6), the order of [`Pilot::num_mech_kills`](crate::ai::Pilot::num_mech_kills).
const VEHICLE_CLASS_GROUND: usize = 5;

/// The `gamesys.fit` values a kill reads: `MechClassWeights[]` (mech.cpp:512-518) and `KillSkill[]` (mover.cpp:2154).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct KillValues {
    /// `MechClassWeights[NUM_MECH_CLASSES]`, indexed by `MECH_CLASS_*` (light 1, medium 2, heavy 3).
    pub mech_class_weights: [f32; 5],
    /// `KillSkill[6]`, indexed by the C++ `vehicleClass` (a mech's weight class, 4 for ground vehicles).
    pub kill_skill: [f32; 6],
}

impl KillValues {
    /// From the loaded `gamesys.fit` tables.
    pub fn from_gamesys(g: &mc2_sim_data::gamesys::GameSystem) -> KillValues {
        KillValues {
            mech_class_weights: g.mech.mech_class_weights,
            kill_skill: g.skills.kill_skill,
        }
    }
}

/// `MechWarrior::handleKilledTarget` (warrior.cpp:7060-7120): the kill counters of the pilot of `attacker` (its mission counters and
/// the `killed[]` list) and the gunnery skill points of `KillSkill[]` (mover.cpp:2154, in `values`) that the kill earns.
///
/// The C++ `switch` on the target's class: a battle mech that is not an air mech counts under its weight class (`getMechClass`,
/// gameobj.cpp:2049) and earns `KillSkill[vehicleClass]`; an air mech counts as a ground vehicle and earns `KillSkill[4]`; a ground
/// vehicle counts only when it has path locks (infantry and powered armour, `gv->pathLocks`), as a ground vehicle with `KillSkill[4]`.
/// Turrets, gates, buildings and other objects earn nothing.
///
/// TODO(lane A, lane C, lane D): the C++ runs this from the pilot's `PILOT_ALARM_KILLED_TARGET` alarm. Call it from
/// `object::hooks::attacker_killed_target` (object/hooks.rs, lane A's hook where a shot kills its target, structure.rs:2608) and from the
/// destroy paths of lane C's `Mover::destroy`/`applyDamage`. Until then the kill counters stay zero in every run.
pub fn credit_kill(
    w: &mut crate::World,
    attacker: WatchId,
    target: ObjectHandle,
    values: &KillValues,
) {
    use crate::object::Kind;

    let Some(pilot_slot) = w
        .objects
        .by_watch_id(attacker)
        .and_then(|h| w.objects.get(h))
        .and_then(|o| match &o.kind {
            Kind::Mech(m) => Some(m.m.pilot_slot),
            Kind::Vehicle(v) => Some(v.m.pilot_slot),
            _ => None,
        })
    else {
        return;
    };
    let Some(object) = w.objects.get(target) else {
        return;
    };
    let target_wid = object.c.watch_id;
    // (numMechKills index, KillSkill index)
    let credit = match &object.kind {
        Kind::Mech(m) if m.m.move_level != 2 => {
            let weights = &values.mech_class_weights;
            let tonnage = object.c.tonnage;
            // getMechClass: LIGHT (1) under weights[1], MEDIUM (2) under weights[2], HEAVY (3) under weights[3], else ASSAULT (4)
            let class = if tonnage < weights[1] {
                1
            } else if tonnage < weights[2] {
                2
            } else if tonnage < weights[3] {
                3
            } else {
                4
            };
            Some((class, class))
        }
        Kind::Mech(_) => Some((VEHICLE_CLASS_GROUND, 4)),
        Kind::Vehicle(v) if v.m.path_locks_enabled => Some((VEHICLE_CLASS_GROUND, 4)),
        _ => None,
    };
    let Some((class, skill_index)) = credit else {
        return;
    };
    let skill_increment = values.kill_skill[skill_index];
    if let Some(pilot) = w.pilots.get_mut(pilot_slot) {
        pilot.num_mech_kills[class][1] += 1;
        if pilot.killed.len() < MAX_KILLED {
            pilot.killed.push(target_wid);
        }
    }
    if let Some(load) = w.mission.pilot_loads.get_mut(pilot_slot as usize) {
        load.skill_points[Skill::Gunnery.index()] += skill_increment;
    }
}

/// How far `MechWarrior::init(FitIniFile*)` got. The C++ returns an error code at the first missing mandatory key and every caller
/// ignores it (`gosASSERT` is compiled out), so a pilot whose file is missing or incomplete keeps whatever was read before that key
/// and the defaults of `init(bool)` for the rest. No ejection roll is drawn in that case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InitProgress {
    /// Everything was read and the ejection roll was drawn.
    Complete,
    /// The read stopped at this key or block (nothing after it was read, no sim draw).
    StoppedAt(&'static str),
}

/// `MechWarrior::init(FitIniFile*)` for the pilot in `slot` (warrior.cpp:1020-1215), including the one sim draw. `profile` is `None`
/// when the file does not exist (`pmw00080`, the infantry pilot of `create_infantry`, is not in the shipped data): the C++ then
/// calls `init` on a file that never opened, which fails at the first `seekBlock`.
///
/// Order of the reads (and so of the early returns): `[General]` (always optional keys), `[PersonalityTraits]` professionalism,
/// decorum, aggressiveness, courage; `[Skills]` piloting, sensors, gunnery (each also copied to `skillRank`); the optional blocks;
/// `calcRank`; `[Status] Wounds`; then the ejection roll.
pub fn init_pilot(
    pool: &mut PilotPool,
    loads: &mut [PilotLoad],
    slot: usize,
    profile: Option<Rc<WarriorProfile>>,
    brain_name: &str,
    rng: &mut SimRng,
) -> InitProgress {
    let p = &mut pool.slots[slot];
    let l = &mut loads[slot];
    l.brain_name = brain_name.to_string();
    l.team = 0;
    let Some(profile) = profile else {
        return InitProgress::StoppedAt("General");
    };
    l.profile = Some(profile.clone());
    l.paint_scheme = profile.general.paint_scheme;
    l.not_mine_yet = profile.general.not_mine_yet;
    l.ids = [
        profile.general.desc_index,
        profile.general.name_index,
        profile.general.picture_index,
    ];

    let Some(personality) = profile.personality else {
        return InitProgress::StoppedAt("PersonalityTraits");
    };
    let traits = [
        ("Professionalism", personality.professionalism),
        ("Decorum", personality.decorum),
        ("Aggressiveness", personality.aggressiveness),
        ("Courage", personality.courage),
    ];
    for (i, (name, v)) in traits.iter().enumerate() {
        let Some(v) = v else {
            return InitProgress::StoppedAt(name);
        };
        l.personality[i] = *v;
    }

    for s in Skill::ALL {
        let i = s.index();
        let Some(v) = profile.skills.get(s) else {
            return InitProgress::StoppedAt(s.key());
        };
        p.skills[i] = i32::from(v);
        l.skill_rank[i] = f32::from(v);
    }
    p.specialty = profile.specialty;
    for s in Skill::ALL {
        let i = s.index();
        let v = i32::from(profile.skills.get(s).unwrap_or(0)) as i8;
        l.original_skills[i] = profile.original_skills.get(s).unwrap_or(v);
        l.starting_skills[i] = profile.starting_skills.get(s).unwrap_or(v);
        l.skill_points[i] = profile.skill_points[i];
    }
    l.rank = profile.calc_rank();

    let Some(wounds) = profile.wounds else {
        return InitProgress::StoppedAt("Status.Wounds");
    };
    p.wounds = f32::from(wounds);

    // warrior.cpp:1203
    let roll = rng.random_number(100);
    p.escapes_thru_ejection = escapes_thru_ejection(
        roll,
        p.skills[Skill::Piloting.index()],
        profile.is_toughness_specialist(),
    );
    InitProgress::Complete
}

/// The memory-cell half of `MechWarrior::loadBrainParameters` (warrior.cpp:7288-7333), applied to values already parsed by
/// `mc2_sim_data::mission::warriors` (which stops at the first bad cell like the C++).
pub fn apply_brain_cells(pilot: &mut Pilot, warrior: &Warrior) -> Result<(), String> {
    for c in &warrior.cells {
        let idx = usize::try_from(c.cell)
            .ok()
            .filter(|&i| i < NUM_MEMORY_CELLS)
            .ok_or_else(|| {
                format!(
                    "Warrior{}: memory cell {} out of range",
                    warrior.index, c.cell
                )
            })?;
        // MemoryCell is a union of int and float: a real is stored as its bit pattern.
        pilot.memory[idx] = match c.memory {
            CellMemory::Integer(v) => v,
            CellMemory::Real(v) => v.to_bits() as i32,
        };
    }
    Ok(())
}

/// Reads back a real from a memory cell (`getRealMemory`).
pub fn memory_real(pilot: &Pilot, cell: usize) -> f32 {
    f32::from_bits(pilot.memory[cell] as u32)
}

/// A pilot with no vehicle yet: `vehicleWID` zero.
pub const NO_VEHICLE: WatchId = WatchId::NONE;

// ---- save/load (lane E, for the pilot slots). `profile` is STATIC (an Rc into the game data): `load` sets it to None and
// `load_into` keeps the base world's profile for the same slot, which is why the save records only whether a profile was present. ----

use crate::save::{Reader, SaveError, SaveResult, Snapshot, Writer};

/// `Option<WarriorRank>` tag: 0 none, 1 Green, 2 Regular, 3 Veteran, 4 Elite, 5 Ace. `WarriorRank` is mc2-sim-data's, so it is
/// encoded here rather than given a trait impl.
fn save_rank(rank: Option<WarriorRank>, w: &mut Writer) {
    w.u8(match rank {
        None => 0,
        Some(WarriorRank::Green) => 1,
        Some(WarriorRank::Regular) => 2,
        Some(WarriorRank::Veteran) => 3,
        Some(WarriorRank::Elite) => 4,
        Some(WarriorRank::Ace) => 5,
    });
}

fn load_rank(r: &mut Reader<'_>) -> SaveResult<Option<WarriorRank>> {
    Ok(match r.u8()? {
        0 => None,
        1 => Some(WarriorRank::Green),
        2 => Some(WarriorRank::Regular),
        3 => Some(WarriorRank::Veteran),
        4 => Some(WarriorRank::Elite),
        5 => Some(WarriorRank::Ace),
        other => return Err(SaveError(format!("save: bad rank tag {other}"))),
    })
}

impl PilotLoad {
    /// Reads one record. The flag is whether the saved slot had a profile (the record itself has `profile: None`).
    fn read_saved(r: &mut Reader<'_>) -> SaveResult<(PilotLoad, bool)> {
        let used = bool::load(r)?;
        let has_profile = bool::load(r)?;
        let brain_name = String::load(r)?;
        let rank = load_rank(r)?;
        let skill_rank = <[f32; 3]>::load(r)?;
        let skill_points = <[f32; 3]>::load(r)?;
        let original_skills = <[i8; 3]>::load(r)?;
        let starting_skills = <[i8; 3]>::load(r)?;
        let personality = <[i8; 4]>::load(r)?;
        let team = i32::load(r)?;
        let paint_scheme = i64::load(r)?;
        let not_mine_yet = bool::load(r)?;
        let ids = <[i64; 3]>::load(r)?;
        let loaded = PilotLoad {
            used,
            profile: None,
            brain_name,
            rank,
            skill_rank,
            skill_points,
            original_skills,
            starting_skills,
            personality,
            team,
            paint_scheme,
            not_mine_yet,
            ids,
        };
        Ok((loaded, has_profile))
    }
}

impl Snapshot for PilotLoad {
    fn save(&self, w: &mut Writer) {
        self.used.save(w);
        self.profile.is_some().save(w);
        self.brain_name.save(w);
        save_rank(self.rank, w);
        self.skill_rank.save(w);
        self.skill_points.save(w);
        self.original_skills.save(w);
        self.starting_skills.save(w);
        self.personality.save(w);
        self.team.save(w);
        self.paint_scheme.save(w);
        self.not_mine_yet.save(w);
        self.ids.save(w);
    }

    /// `profile` is `None`: a load without a base world cannot recover the Rc. Use [`Snapshot::load_into`] on the base world.
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(PilotLoad::read_saved(r)?.0)
    }

    /// Overlay: the profile of the base slot is kept when the saved slot had one. Failing that (the saved slot had none, or the base
    /// has none while the save has one) the profile is `None` or an error respectively.
    fn load_into(&mut self, r: &mut Reader<'_>) -> SaveResult<()> {
        let (mut saved, has_profile) = PilotLoad::read_saved(r)?;
        saved.profile = match (has_profile, self.profile.clone()) {
            (false, _) => None,
            (true, Some(base)) => Some(base),
            (true, None) => {
                return Err(SaveError(
                    "save: a pilot slot has a profile the base world does not have".into(),
                ))
            }
        };
        *self = saved;
        Ok(())
    }
}

/// Overlay for the whole `PilotLoad` table (`MissionState::pilot_loads`). A plain `Vec<PilotLoad>::load_into` replaces every record
/// and so drops the static profiles; call this instead. Writes with `Vec<PilotLoad>::save`. The base table must have the same length
/// (the pool size, `MAX_WARRIORS`), and each slot keeps its base profile through [`PilotLoad::load_into`].
pub fn load_pilot_loads_into(base: &mut [PilotLoad], r: &mut Reader<'_>) -> SaveResult<()> {
    let n = r.length()?;
    if n != base.len() {
        return Err(SaveError(format!(
            "save: {n} pilot load slots, base world has {}",
            base.len()
        )));
    }
    for slot in base.iter_mut() {
        slot.load_into(r)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{RngMode, SimRng};

    #[test]
    fn default_attack_radius_matches_cpp() {
        // code/warrior.cpp:293 `float DefaultAttackRadius = 275.0;`
        assert_eq!(DEFAULT_ATTACK_RADIUS, 275.0);
    }

    #[test]
    fn fresh_pilots_hold_initialised_tactical_orders() {
        // TacticalOrder::init(): stage 1 (the `g1` of every frame-0 digest line), commander origin, times -1 (tacordr.cpp:169-206)
        let mut pool = PilotPool::default();
        setup_pool(&mut pool, 275.0);
        for p in [&pool.slots[1], &pool.slots[119]] {
            for o in p.tac_orders.iter().chain([&p.cur_order]) {
                assert_eq!(o.stage, 1);
                assert_eq!(o.origin, crate::command::OrderOrigin::Commander);
                assert_eq!((o.time, o.delayed_time, o.last_time), (-1.0, -1.0, -1.0));
                assert_eq!(o.selection_index, -1);
                assert!(o.move_params.keep_moving && o.attack_params.pursue);
            }
        }
        let mut loads = vec![PilotLoad::default(); MAX_WARRIORS];
        pool.slots[3].cur_order.stage = 3;
        clear_slot(&mut pool, &mut loads, 3, 275.0);
        assert_eq!(pool.slots[3].cur_order.stage, 1);
    }

    #[test]
    fn pool_has_120_slots_with_the_slot_based_stagger() {
        let mut pool = PilotPool::default();
        setup_pool(&mut pool, 275.0);
        assert_eq!(pool.slots.len(), MAX_WARRIORS);
        assert_eq!(pool.slots[5].timers, [1.0, 0.5, 1.0]);
        assert_eq!(pool.slots[31].slot, 31);
        // slot 31: 31 % 30 = 1, 31 % 15 = 1
        assert_eq!(pool.slots[31].timers, [0.2, 0.1, 0.2]);
        assert_eq!(pool.slots[7].timers[0], 1.4);
        assert!(pool
            .slots
            .iter()
            .all(|p| p.memory.len() == NUM_MEMORY_CELLS));
    }

    #[test]
    fn new_warrior_skips_slot_zero_and_reuses_freed_slots() {
        let mut loads = vec![PilotLoad::default(); MAX_WARRIORS];
        assert_eq!(new_warrior(&mut loads), Some(1));
        assert_eq!(new_warrior(&mut loads), Some(2));
        loads[1].used = false;
        assert_eq!(new_warrior(&mut loads), Some(1));
    }

    #[test]
    fn ejection_roll_rules() {
        // piloting 40: rolls up to 70 escape, never 95 and above
        assert!(escapes_thru_ejection(70, 40, false));
        assert!(!escapes_thru_ejection(71, 40, false));
        assert!(!escapes_thru_ejection(95, 90, false));
        assert!(escapes_thru_ejection(99, 10, true));
    }

    #[test]
    fn real_memory_round_trips_through_the_union() {
        let mut p = Pilot {
            memory: vec![0; NUM_MEMORY_CELLS],
            ..Pilot::default()
        };
        let w = Warrior {
            index: 1,
            profile: String::new(),
            brain: String::new(),
            num_cells: 2,
            num_static_vars: 0,
            cells: vec![
                mc2_sim_data::mission::WarriorCell {
                    cell: 3,
                    memory: CellMemory::Integer(9),
                },
                mc2_sim_data::mission::WarriorCell {
                    cell: 4,
                    memory: CellMemory::Real(0.5),
                },
            ],
            static_vars: vec![],
        };
        apply_brain_cells(&mut p, &w).unwrap();
        assert_eq!(p.memory[3], 9);
        assert_eq!(memory_real(&p, 4), 0.5);
        let mut rng = SimRng::new(RngMode::LegacyMsvc, 1);
        let _ = rng.random_number(100);
    }

    #[test]
    fn a_fresh_or_cleared_pilot_has_the_c_plus_plus_defaults() {
        let l = PilotLoad::default();
        assert_eq!(l.personality, [40; 4]);
        assert_eq!(l.rank, Some(WarriorRank::Green));
        // a freed slot loses its state but keeps its stagger timers
        let mut pool = PilotPool::default();
        setup_pool(&mut pool, 50.0);
        let mut loads = vec![PilotLoad::default(); MAX_WARRIORS];
        let timers = pool.slots[7].timers;
        loads[7].used = true;
        loads[7].personality = [1, 2, 3, 4];
        loads[7].brain_name = "pbrain".into();
        pool.slots[7].memory[5] = 9;
        pool.slots[7].skills = [10, 20, 30, 0];
        pool.slots[7].wounds = 2.0;
        clear_slot(&mut pool, &mut loads, 7, 50.0);
        assert!(!loads[7].used);
        assert_eq!(loads[7].personality, [40; 4]);
        assert!(loads[7].brain_name.is_empty());
        assert_eq!(pool.slots[7].memory[5], 0);
        assert_eq!(pool.slots[7].skills, [10, 20, 30, 0]);
        assert_eq!(pool.slots[7].wounds, 0.0);
        assert_eq!(pool.slots[7].timers, timers);
    }
}
