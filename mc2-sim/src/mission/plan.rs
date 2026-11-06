// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The deterministic skeleton of `Mission::init` (mission.cpp:2412-2742, 2745-2780, 2855-2895): which mission parts become
//! movers, in which order, with which pilot slot, and the part ids the commander groups give them.
//!
//! Everything here is a pure function of the parsed mission file, the object type table and one draw from the sim RNG,
//! so it can be checked against the oracle without any terrain or object storage (`tests/init_plan.rs`).
//!
//! Steps, in C++ order:
//! 1. the squad pre-pass (mission.cpp:2430-2488): `squadMap`, per-squad `maxAlternatives`, `bigAlternatives`;
//! 2. ONE sim draw `RandomNumber(bigAlternatives + 1)` (mission.cpp:2489), made even when no part has alternatives;
//! 3. every squad takes that choice, minus one (floor 1) from difficulty 2 up (mission.cpp:2490-2497); the optional
//!    `[Squads]` block overrides single squads (mission.cpp:2500-2510);
//! 4. each part reads its fields from the alternative it picked, except `Pilot` and `squadNum`, which stay those of the
//!    original part (mission.cpp:2512-2690); an alternative index of `-1` leaves a zeroed record that never matches an
//!    object type (the C++ `continue`s past a `memset` record);
//! 5. the single-player hack `teamId == 2 -> 0` (mission.cpp:2640);
//! 6. mover counts, then creation order: team 0..7 outermost, parts in file order inside (mission.cpp:2745-2780).

use crate::math::Vec3;
use mc2_sim_data::mission::{Mission, Part};
use mc2_sim_data::objtype::{ObjectTable, ObjectTypeClass};

use crate::ids::WatchId;
use crate::rng::SimRng;

/// `MAX_SQUADS` (mission.cpp:2425).
pub const MAX_SQUADS: usize = 256;
/// `MAX_ALTERNATIVES` (mission.cpp:2426).
pub const MAX_ALTERNATIVES: usize = 15;
/// `MAX_MOVERGROUP_COUNT_START` (unitdesg.h:83).
pub const MAX_MOVERGROUP_COUNT_START: usize = 12;
/// `MAX_MOVERGROUPS` (unitdesg.h:82).
pub const MAX_MOVERGROUPS: usize = 16;
/// `MIN_MOVER_PART_ID` (objmgr.h).
pub const MIN_MOVER_PART_ID: i32 = 512;
/// `MAX_TEAMS`.
pub const MAX_TEAMS: usize = 8;
/// `MAX_MC_PLAYERS` / `MAX_COMMANDERS`.
pub const MAX_COMMANDERS: usize = 8;

/// `Part` (mission.h) after the alternative was applied. Index 0 of the part list is the unused "null" part.
#[derive(Clone, Debug, Default)]
pub struct PartRec {
    /// False for a part whose chosen alternative was `-1`: the C++ record stays all zero.
    pub loaded: bool,
    /// `objNumber`.
    pub obj_number: u32,
    /// `controlType` (`CONTROL_PLAYER` = 1).
    pub control_type: u32,
    /// `controlDataType`.
    pub control_data_type: u32,
    /// `profileName`.
    pub profile_name: String,
    /// `variantNum`.
    pub variant_num: u32,
    /// `pilot`: index of the pilot pool slot (`[WarriorN]` is slot N).
    pub pilot: u32,
    /// `position` (z is -1).
    pub position: Vec3,
    /// `rotation` (degrees).
    pub rotation: f32,
    /// `teamId` after the single-player `2 -> 0` hack.
    pub team_id: i8,
    /// `commanderID`.
    pub commander_id: i32,
    /// `squadId` (the squad number of the original part; narrowed to `char` by `setSquadId`).
    pub squad_id: u32,
    /// `baseColor`, `highlightColor1`, `highlightColor2` when the file has them.
    pub base_color: Option<u32>,
    /// See `base_color`.
    pub highlight_color1: Option<u32>,
    /// See `base_color`.
    pub highlight_color2: Option<u32>,
    /// `active`.
    pub active: i32,
    /// `exists`.
    pub exists: i32,
    /// `destroyed` (`Damage >= 1.0`).
    pub destroyed: bool,
    /// `myIcon`.
    pub my_icon: i8,
    /// `captureable`.
    pub captureable: bool,
    /// `objectWID`: filled when the mover is created.
    pub object_wid: WatchId,
}

/// Squad bookkeeping of the pre-pass.
#[derive(Clone, Debug, Default)]
pub struct SquadChoice {
    /// `squadMap`: squad numbers in order of first appearance.
    pub squad_map: Vec<u32>,
    /// `maxAlternatives[squadIndex]`.
    pub max_alternatives: Vec<usize>,
    /// `randomAlternative[squadIndex]` after the difficulty adjustment and `[Squads]` overrides.
    pub random_alternative: Vec<i32>,
    /// `bigAlternatives`.
    pub big_alternatives: i32,
    /// `alternateChoice`: the value the single draw returned.
    pub alternate_choice: i32,
}

/// What kind of mover a part creates (`BATTLEMECH_TYPE` / `VEHICLE_TYPE`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoverClass {
    /// `BattleMech`, taken from `ObjectManager::mechs[]`.
    Mech,
    /// `GroundVehicle`, taken from `ObjectManager::vehicles[]`.
    Vehicle,
}

/// One mover to create, in creation order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoverPlan {
    /// Part index (1-based).
    pub part: usize,
    /// Mech or vehicle.
    pub class: MoverClass,
    /// `curMech` / `curVehicle` at creation: the index into `mechs[]` / `vehicles[]`.
    pub slot: usize,
}

/// A group slot filled from `Commander{c}Group:{g}` (mission.cpp:2855-2895).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroupMember {
    /// Part index (1-based).
    pub part: usize,
    /// `commandersToLoad[c][0]`: the commander the part id and the group belong to.
    pub commander: usize,
    /// Group number.
    pub group: usize,
    /// Mate index inside the group (0..12).
    pub mate: usize,
}

/// `Mission::init` as far as it can be decided without objects.
#[derive(Clone, Debug, Default)]
pub struct LoadPlan {
    /// `parts[0..=numParts]`; entry 0 is unused.
    pub parts: Vec<PartRec>,
    /// Squad choice.
    pub squads: SquadChoice,
    /// `numMechs`.
    pub num_mechs: usize,
    /// `numVehicles`.
    pub num_vehicles: usize,
    /// Movers in the order `createPartObject` runs.
    pub order: Vec<MoverPlan>,
    /// Pilot slots released because their part is not loaded (`MechWarrior::freeWarrior`), in order.
    pub freed_pilots: Vec<u32>,
    /// `maxTeamID` of the raw part team ids (before the `2 -> 0` hack).
    pub max_team_id: i32,
    /// `maxCommanderID`.
    pub max_commander_id: i32,
}

/// The three `LoadType`-dependent inputs of the plan.
#[derive(Clone, Copy, Debug)]
pub struct PlanInputs<'a> {
    /// `GameDifficulty`.
    pub difficulty: i32,
    /// `[Squads] SquadN` overrides (by squad index), `None` if the block or key is absent.
    pub squad_overrides: &'a [Option<u32>],
    /// `loadType == MISSION_LOAD_SP_LOGISTICS`.
    pub logistics: bool,
}

/// `commandersToLoad[8][3]` for single player: `-mission` (quickstart) or logistics (commander 0 comes from the player's force).
pub fn commanders_to_load(logistics: bool) -> [[i8; 3]; MAX_COMMANDERS] {
    if logistics {
        // logistics.cpp:456 with commandersToLoad[0] = -1 (logistics.cpp:556)
        [
            [-1, -1, -1],
            [1, 1, 1],
            [2, 2, 2],
            [3, 3, 3],
            [4, 4, 4],
            [5, 5, 5],
            [6, 6, 6],
            [7, 7, 7],
        ]
    } else {
        // mechcmd2.cpp:1675: commander 2 is an ally on team 0
        [
            [0, 0, 0],
            [1, 1, 1],
            [2, 0, 2],
            [3, 3, 3],
            [4, 4, 4],
            [5, 5, 5],
            [6, 6, 6],
            [7, 7, 7],
        ]
    }
}

/// The team and commander id ranges of the pre-scan (mission.cpp:2020-2060): the raw ids of all parts.
pub fn max_ids(parts: &[Part]) -> (i32, i32) {
    let mut max_team = -1;
    let mut max_cmdr = -1;
    for p in parts {
        let cmdr = i32::from(p.commander_id as i8);
        if cmdr > max_cmdr {
            max_cmdr = cmdr;
        }
        let team = i32::from(p.team_id);
        if team > max_team {
            max_team = team;
        }
    }
    (max_team, max_cmdr)
}

fn count_alternatives(p: &Part) -> usize {
    // mission.cpp:2467-2474: count entries up to the first -1 (the array always has 15 entries in the data).
    let mut n = 0;
    while n < MAX_ALTERNATIVES {
        match p.indices_of_alternatives.get(n) {
            Some(&v) if v != -1 => n += 1,
            _ => break,
        }
    }
    n
}

/// Index of `squad` in the squad map, or the current length when it is new (the C++ appends it).
fn squad_index(map: &mut Vec<u32>, squad: u32) -> usize {
    match map.iter().position(|&s| s == squad) {
        Some(i) => i,
        None => {
            map.push(squad);
            map.len() - 1
        }
    }
}

/// The optional `[Squads]` test block (mission.cpp:2500-2510): `ul Squad<i>` forces the alternative of squad index `i` (squads in
/// order of first appearance). Returns one entry per squad; the whole block is absent in the shipped missions.
pub fn read_squad_overrides(
    fit: &mut mc2_formats::ini::FitIni,
    parts: &[Part],
) -> Vec<Option<u32>> {
    let mut map: Vec<u32> = Vec::new();
    for p in parts {
        squad_index(&mut map, p.squad_num);
    }
    let mut out = vec![None; map.len()];
    if fit.seek_block("Squads").is_ok() {
        for (i, slot) in out.iter_mut().enumerate() {
            if let Ok(v) = fit.read_u32(&format!("Squad{i}")) {
                *slot = Some(v);
            }
        }
    }
    out
}

/// Steps 1-3: squad pre-pass, the one random draw and the per-squad choice.
pub fn choose_alternatives(
    mission_parts: &[Part],
    inputs: &PlanInputs<'_>,
    rng: &mut SimRng,
) -> SquadChoice {
    let mut squad_map: Vec<u32> = Vec::new();
    let mut max_alternatives: Vec<usize> = Vec::new();
    let mut big = 0usize;
    for p in mission_parts {
        let si = squad_index(&mut squad_map, p.squad_num);
        if max_alternatives.len() <= si {
            max_alternatives.resize(si + 1, 0);
        }
        let n = count_alternatives(p);
        if max_alternatives[si] < n {
            max_alternatives[si] = n;
        }
        if n > big {
            big = n;
        }
    }
    let big_alternatives = big as i32;
    let alternate_choice = rng.random_number(big_alternatives + 1);
    let mut random_alternative = vec![-1; squad_map.len()];
    for ra in &mut random_alternative {
        *ra = alternate_choice;
        if inputs.difficulty >= 2 {
            *ra -= 1;
            if *ra < 1 {
                *ra = 1;
            }
        }
    }
    for (i, ra) in random_alternative.iter_mut().enumerate() {
        if let Some(Some(v)) = inputs.squad_overrides.get(i) {
            *ra = *v as i32;
        }
    }
    SquadChoice {
        squad_map,
        max_alternatives,
        random_alternative,
        big_alternatives,
        alternate_choice,
    }
}

fn rec_from(p: &Part, original: &Part) -> PartRec {
    PartRec {
        loaded: true,
        obj_number: p.object_number,
        control_type: p.control_type,
        control_data_type: p.control_data_type,
        profile_name: p.profile_name.clone(),
        variant_num: p.variant_number,
        // `Pilot` is read from the ORIGINAL part when an alternative is used (mission.cpp:2569-2576).
        pilot: original.pilot,
        position: Vec3 {
            x: p.position.0,
            y: p.position.1,
            z: -1.0,
        },
        rotation: p.rotation,
        // single-player hack for allies (mission.cpp:2640)
        team_id: if p.team_id == 2 { 0 } else { p.team_id },
        commander_id: p.commander_id,
        squad_id: original.squad_num,
        base_color: p.base_color,
        highlight_color1: p.highlight_color1,
        highlight_color2: p.highlight_color2,
        active: p.active,
        exists: p.exists,
        destroyed: p.destroyed,
        my_icon: p.my_icon,
        captureable: p.captureable,
        object_wid: WatchId::NONE,
    }
}

/// Steps 4-6: build the part records and the creation order.
pub fn plan_parts(
    mission: &Mission,
    objects: &ObjectTable,
    inputs: &PlanInputs<'_>,
    rng: &mut SimRng,
) -> Result<LoadPlan, String> {
    let parts_in = &mission.parts;
    let (max_team_id, max_commander_id) = max_ids(parts_in);
    let squads = choose_alternatives(parts_in, inputs, rng);

    let mut parts: Vec<PartRec> = vec![PartRec::default(); parts_in.len() + 1];
    for (k, orig) in parts_in.iter().enumerate() {
        let i = k + 1;
        let si = squads
            .squad_map
            .iter()
            .position(|&s| s == orig.squad_num)
            .ok_or_else(|| {
                format!(
                    "part {i}: squad {} missing from the squad map",
                    orig.squad_num
                )
            })?;
        let mut source = orig;
        if squads.max_alternatives[si] > 0 {
            let mut part_id = i as i32;
            if squads.random_alternative[si] > 0 {
                let which = (squads.random_alternative[si] - 1) as usize;
                // alternatives[] has 15 entries in the data; an index past the declared list reads -1 padding
                part_id = orig
                    .indices_of_alternatives
                    .get(which)
                    .copied()
                    .unwrap_or(-1);
            }
            if part_id == -1 {
                // The record stays zeroed, but `parts[i].squadId = squadNum` ran before the `continue` (mission.cpp:2516).
                parts[i].squad_id = orig.squad_num;
                continue;
            }
            if part_id <= 0 {
                return Err(format!("part {i}: bad alternate {part_id}"));
            }
            source = parts_in
                .get(part_id as usize - 1)
                .ok_or_else(|| format!("part {i}: alternate {part_id} is not a part"))?;
        }
        parts[i] = rec_from(source, orig);
    }

    // mission.cpp:2698-2715: count mechs and vehicles by object type class.
    let class_of = |obj: u32| -> Option<MoverClass> {
        objects.get(obj).and_then(|t| match t.object_type_class {
            ObjectTypeClass::BattleMech => Some(MoverClass::Mech),
            ObjectTypeClass::Vehicle => Some(MoverClass::Vehicle),
            _ => None,
        })
    };
    let mut num_mechs = 0usize;
    let mut num_vehicles = 0usize;
    for rec in parts.iter().skip(1) {
        match class_of(rec.obj_number) {
            Some(MoverClass::Mech) => num_mechs += 1,
            Some(MoverClass::Vehicle) => num_vehicles += 1,
            None => {}
        }
    }

    // mission.cpp:2745-2780
    let mut order = Vec::new();
    let mut freed_pilots = Vec::new();
    let (mut cur_mech, mut cur_vehicle) = (0usize, 0usize);
    for t in 0..MAX_TEAMS as i32 {
        for (i, rec) in parts.iter().enumerate().skip(1) {
            let mut load_em = true;
            if inputs.logistics && rec.commander_id == 0 && !rec.destroyed {
                load_em = false;
            }
            if load_em {
                match class_of(rec.obj_number) {
                    Some(MoverClass::Mech) if i32::from(rec.team_id) == t => {
                        order.push(MoverPlan {
                            part: i,
                            class: MoverClass::Mech,
                            slot: cur_mech,
                        });
                        cur_mech += 1;
                    }
                    Some(MoverClass::Vehicle) if i32::from(rec.team_id) == t => {
                        order.push(MoverPlan {
                            part: i,
                            class: MoverClass::Vehicle,
                            slot: cur_vehicle,
                        });
                        cur_vehicle += 1;
                    }
                    _ => {}
                }
            } else {
                freed_pilots.push(rec.pilot);
            }
        }
    }

    Ok(LoadPlan {
        parts,
        squads,
        num_mechs,
        num_vehicles,
        order,
        freed_pilots,
        max_team_id,
        max_commander_id,
    })
}

/// Which team each commander belongs to after `Mission::init` creates teams and commanders (mission.cpp:2020-2090):
/// `result[c]` is `None` when commander `c` does not exist, `Some(None)` when it exists with a NULL team and
/// `Some(Some(t))` for team `t`. Teams `0..=max_team_id` and commanders `0..=max_commander_id` are created; every commander
/// `i <= max_commander_id` gets `Team::teams[commandersToLoad[i][1]]` (NULL when that team was not created) set on commander
/// `commandersToLoad[i][0]`, and commander 0 finally joins the home team (team 0).
pub fn commander_teams(
    max_team_id: i32,
    max_commander_id: i32,
    commanders: &[[i8; 3]; MAX_COMMANDERS],
) -> [Option<Option<i8>>; MAX_COMMANDERS] {
    let mut out: [Option<Option<i8>>; MAX_COMMANDERS] = [None; MAX_COMMANDERS];
    for slot in out.iter_mut().take((max_commander_id + 1).max(0) as usize) {
        *slot = Some(None);
    }
    let team_exists = |t: i8| t >= 0 && i32::from(t) <= max_team_id;
    for c in commanders
        .iter()
        .take((max_commander_id + 1).max(0) as usize)
    {
        if c[0] > -1 {
            let target = c[0] as usize;
            if out[target].is_some() {
                out[target] = Some(if team_exists(c[1]) { Some(c[1]) } else { None });
            }
        }
    }
    if let Some(slot) = out.get_mut(0) {
        if slot.is_some() && max_team_id >= 0 {
            *slot = Some(Some(0));
        }
    }
    out
}

/// `MIN_MOVER_PART_ID + commander * 192 + group * 12 + mate` (`GameObjectManager::calcPartId`).
pub fn calc_mover_part_id(commander: usize, group: usize, mate: usize) -> i32 {
    MIN_MOVER_PART_ID
        + (commander * MAX_MOVERGROUPS * MAX_MOVERGROUP_COUNT_START
            + group * MAX_MOVERGROUP_COUNT_START
            + mate) as i32
}

/// Commander group membership (mission.cpp:2855-2895): for each commander slot with a commander to load, each
/// `Commander{c}Group:{g}` block lists up to 12 part numbers; a part counts when it was created (`objectWID != 0`).
/// `created[i]` says whether part `i` has an object.
pub fn group_members(
    mission: &Mission,
    commanders: &[[i8; 3]; MAX_COMMANDERS],
    created: &dyn Fn(usize) -> bool,
) -> Vec<GroupMember> {
    let mut out = Vec::new();
    for (cid, slot) in commanders.iter().enumerate() {
        if slot[0] == -1 {
            continue;
        }
        let commander = slot[0] as usize;
        let mut group = 0u32;
        while let Some(g) = mission
            .commander_groups
            .iter()
            .find(|g| g.commander_id as usize == cid && g.group_index == group)
        {
            for (mate, &part) in g.mates.iter().enumerate().take(MAX_MOVERGROUP_COUNT_START) {
                if part > 0 && created(part as usize) {
                    out.push(GroupMember {
                        part: part as usize,
                        commander,
                        group: group as usize,
                        mate,
                    });
                }
            }
            group += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{RngMode, SimRng};

    fn part(index: u32, squad: u32, alts: &[i32], team: i8, obj: u32) -> Part {
        let mut a = alts.to_vec();
        a.resize(MAX_ALTERNATIVES, -1);
        Part {
            index,
            object_number: obj,
            control_type: 2,
            control_data_type: 1,
            profile_name: format!("P{index}"),
            csv_file: None,
            variant_number: 0,
            pilot: index,
            position: (index as f32, -(index as f32)),
            rotation: 0.0,
            team_id: team,
            commander_id: 1,
            base_color: None,
            highlight_color1: None,
            highlight_color2: None,
            active: 1,
            exists: 1,
            damage: 0.0,
            destroyed: false,
            my_icon: 0,
            captureable: false,
            player_part: false,
            squad_num: squad,
            self_repair_behavior: None,
            num_alternatives: None,
            alternative_start_index: None,
            indices_of_alternatives: a,
        }
    }

    #[test]
    fn one_draw_even_without_alternatives() {
        let parts = vec![part(1, 0, &[], 0, 5), part(2, 0, &[], 0, 5)];
        let mut rng = SimRng::new(RngMode::LegacyMsvc, 1);
        let c = choose_alternatives(
            &parts,
            &PlanInputs {
                difficulty: 1,
                squad_overrides: &[],
                logistics: false,
            },
            &mut rng,
        );
        assert_eq!(rng.draws(), 1);
        assert_eq!(c.big_alternatives, 0);
        assert_eq!(c.alternate_choice, 0);
        assert_eq!(c.max_alternatives, vec![0]);
    }

    #[test]
    fn choice_is_shared_by_all_squads_and_difficulty_lowers_it() {
        let parts = vec![
            part(1, 7, &[3, 4], 0, 5),
            part(2, 9, &[], 0, 5),
            part(3, 7, &[1], 0, 5),
        ];
        // MSVC seed 1: first next15() is 41; 41 % 3 = 2
        let mut rng = SimRng::new(RngMode::LegacyMsvc, 1);
        let c = choose_alternatives(
            &parts,
            &PlanInputs {
                difficulty: 1,
                squad_overrides: &[],
                logistics: false,
            },
            &mut rng,
        );
        assert_eq!(c.squad_map, vec![7, 9]);
        assert_eq!(c.max_alternatives, vec![2, 0]);
        assert_eq!(c.big_alternatives, 2);
        assert_eq!(c.alternate_choice, 2);
        assert_eq!(c.random_alternative, vec![2, 2]);

        let mut rng = SimRng::new(RngMode::LegacyMsvc, 1);
        let c = choose_alternatives(
            &parts,
            &PlanInputs {
                difficulty: 2,
                squad_overrides: &[None, Some(5)],
                logistics: false,
            },
            &mut rng,
        );
        assert_eq!(c.random_alternative, vec![1, 5]);
        // a choice of 0 at difficulty 3 is raised to 1, not left at -1
        let mut rng = SimRng::new(RngMode::LegacyMsvc, 1);
        let none = vec![part(1, 0, &[], 0, 5)];
        let c = choose_alternatives(
            &none,
            &PlanInputs {
                difficulty: 3,
                squad_overrides: &[],
                logistics: false,
            },
            &mut rng,
        );
        assert_eq!(c.random_alternative, vec![1]);
    }

    #[test]
    fn part_ids_follow_the_commander_group_formula() {
        assert_eq!(calc_mover_part_id(0, 0, 0), 512);
        assert_eq!(calc_mover_part_id(1, 0, 2), 706);
        assert_eq!(calc_mover_part_id(1, 1, 3), 512 + 192 + 12 + 3);
    }

    #[test]
    fn commanders_join_teams_by_the_session_table() {
        let q = commanders_to_load(false);
        // mc2_01: commanders 0 and 1, teams 0 and 1
        let t = commander_teams(1, 1, &q);
        assert_eq!(t[0], Some(Some(0)));
        assert_eq!(t[1], Some(Some(1)));
        assert_eq!(t[2], None);
        // an ally commander 2 sits on team 0 (commandersToLoad[2] = {2, 0, 2}); commander 3 on a missing team has no team
        let t = commander_teams(1, 3, &q);
        assert_eq!(t[2], Some(Some(0)));
        assert_eq!(t[3], Some(None));
    }

    #[test]
    fn commanders_to_load_tables() {
        assert_eq!(commanders_to_load(false)[2], [2, 0, 2]);
        assert_eq!(commanders_to_load(true)[0], [-1, -1, -1]);
    }
}

// ---- save/load (lane E). All dynamic in practice: the records are the load-time decisions (squad alternatives, group membership)
// and `object_wid` is filled when the mover is created. Saved in full rather than re-derived from the file, so a load never depends
// on the squad draw being replayed. ----

crate::snapshot_struct!(PartRec {
    loaded,
    obj_number,
    control_type,
    control_data_type,
    profile_name,
    variant_num,
    pilot,
    position,
    rotation,
    team_id,
    commander_id,
    squad_id,
    base_color,
    highlight_color1,
    highlight_color2,
    active,
    exists,
    destroyed,
    my_icon,
    captureable,
    object_wid,
});

crate::snapshot_struct!(SquadChoice {
    squad_map,
    max_alternatives,
    random_alternative,
    big_alternatives,
    alternate_choice,
});

crate::snapshot_struct!(GroupMember {
    part,
    commander,
    group,
    mate,
});
