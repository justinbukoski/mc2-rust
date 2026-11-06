// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission objective harness for the 24 campaign missions (`feat/rust-objective-harness`) and the 5 tutorial missions
//! (`feat/rust-tutorials`, `tutorial.fit`: `tut_01`..`tut_05`).
//!
//! RC proof per mission: the mission script and objective logic work, and meeting the objectives fires the win. ONE generic,
//! test-only harness does it (`mc2_sim::test_hooks`, cargo feature `test-hooks`, never in the shipped app): it makes an objective's
//! WORLD CONDITION true (destroy a unit or building, capture one, move the home lance into an area, advance mission time) and then
//! lets the mission's own ABL script and the objective system react through `Sim::step`. It never writes an objective status, a
//! script variable or a flag an objective reads.
//!
//! * `mc2_NN_objectives_win`: the mission's visible objectives, read from the mission data, equal the list below; playing them by
//!   hooks in a valid order (the sim activates later objectives, the extraction comes last) leaves every one marked complete by the
//!   sim, and the mission result is `mis_PLAYER_WIN_BIG`.
//! * `mc2_NN_script_path_has_no_unported_gameplay`: no "unavailable" notice that can change a rule fires on the path (presentation
//!   natives such as camera, music and speech are the app's and are not counted). `#[ignore = "port gap: ..."]` names each gap;
//!   `gap_table_is_current` keeps those names true.
//! * `negative_*`: one per condition species the shipped missions use: the condition not met leaves the objective incomplete, and a
//!   positive control right after shows the objective completes once it is met.
//!
//! `OBJECTIVE_HARNESS_REGEN=1` (test `regenerate`, `--ignored`) rewrites the report block and the `GAPS` const in place.
//! `OBJECTIVE_HARNESS_REPORT=<file>` writes the table of `rust/docs/OBJECTIVE-HARNESS.md` from this run.
//!
//! Everything needs `MC2_DATA` (see `common::data_root`).

mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use mc2_formats::vfs::Vfs;
use mc2_sim::mission::objective::ObjectiveStatus;
use mc2_sim::mission::outcome::MissionResult;
use mc2_sim::test_hooks::{self as hooks, Harness, MissionRun, Player};
use mc2_sim_data::mission::{Condition, ConditionSpecies};

const S: ObjectiveStatus = ObjectiveStatus::Successful;
const U: ObjectiveStatus = ObjectiveStatus::Undetermined;

/// `mis_PLAYER_WIN_BIG` (`code/mission.h:41-46`).
const WIN_BIG: u32 = 5;

/// The visible objectives of each campaign mission as the mission data lists them: (index, priority, condition species, title).
/// `x8` is eight conditions of that species; ` | fail:` names the failure conditions. Hidden triggers and nav markers are not
/// listed (the harness does not play them on their own). Generated from the data; `mc2_NN_objectives_win` checks it.
#[allow(clippy::type_complexity)]
const MISSIONS: &[(&str, &[(usize, u32, &str, &str)])] = &[
    (
        "mc2_01",
        &[
            (
                0,
                1,
                "MoveAnyUnitToArea",
                "1. Investigate Abandoned Airfield",
            ),
            (1, 1, "DestroySpecificStructure", "2. Destroy Hangar"),
            (
                2,
                1,
                "DestroySpecificEnemyUnitx8",
                "3. Destroy Bandit Patrol",
            ),
            (
                3,
                1,
                "MoveAnyUnitToArea",
                "4. Repair and Proceed to North Island",
            ),
            (
                4,
                1,
                "DestroySpecificEnemyUnitx4",
                "5. Locate and Destroy Bandits",
            ),
            (
                5,
                1,
                "MoveAllSurvivingMechsToArea",
                "6. Move All 'Mechs to Extraction Point",
            ),
        ],
    ),
    (
        "mc2_02",
        &[
            (
                0,
                1,
                "CaptureSpecificStructure",
                "1. Capture Resource Building",
            ),
            (
                1,
                1,
                "DestroySpecificEnemyUnitx8",
                "2. Destroy All Bandits at Base Gemini South",
            ),
            (
                2,
                1,
                "CaptureSpecificStructure",
                "3. Capture Steiner Sensor Control",
            ),
            (
                3,
                1,
                "DestroySpecificEnemyUnitx10",
                "4. Destroy All Bandits at Base Gemini North",
            ),
            (
                4,
                1,
                "CaptureSpecificStructure",
                "5. Capture Steiner Headquarters",
            ),
            (
                5,
                1,
                "BooleanFlagIsSetx2",
                "Destroy All Bandit Forces Occupying Base Gemini",
            ),
        ],
    ),
    (
        "mc2_03",
        &[
            (
                0,
                1,
                "DestroySpecificStructure",
                "1. Destroy Turret Generators at Checkpoint",
            ),
            (
                1,
                1,
                "DestroySpecificEnemyUnitx6",
                "2. Destroy Enemies at Checkpoint",
            ),
            (2, 1, "CaptureSpecificUnit", "3. Capture HQ Vehicle"),
            (
                3,
                1,
                "CaptureSpecificStructure",
                "4. Capture Weapon Facility",
            ),
            (
                4,
                1,
                "DestroySpecificEnemyUnitx8",
                "5. Defeat Bandit Counterattack",
            ),
        ],
    ),
    (
        "mc2_04",
        &[
            (
                0,
                1,
                "MoveAnyUnitToArea",
                "1 - Move to Bandit Convoy's Last Known Location",
            ),
            (
                1,
                1,
                "DestroySpecificEnemyUnitx7",
                "2 - Destroy Bandit Supply Convoy",
            ),
            (
                2,
                1,
                "CaptureSpecificStructure",
                "3 - Capture Weapon's Facility",
            ),
            (
                3,
                1,
                "MoveAllSurvivingMechsToArea",
                "4 - Extract all surviving mechs",
            ),
        ],
    ),
    (
        "mc2_05",
        &[
            (0, 1, "DestroySpecificStructure", "1. Destroy Fuel Tanks"),
            (
                1,
                1,
                "CaptureSpecificStructure",
                "2. Capture Weapons Facility",
            ),
            (2, 1, "CaptureSpecificStructure", "3. Capture Repair Bay"),
            (
                3,
                1,
                "CaptureSpecificStructure",
                "4. Capture Prison Complex",
            ),
            (
                4,
                1,
                "MoveAllSurvivingMechsToArea",
                "5. Move to Extraction Point",
            ),
            (5, 2, "CaptureSpecificUnitx2", "a. Capture Resource Trucks"),
        ],
    ),
    (
        "mc2_06",
        &[
            (
                0,
                1,
                "CaptureSpecificStructure+DestroyAllEnemyUnits",
                "1. Protect the Base Headquarters",
            ),
            (
                1,
                1,
                "DestroyAllEnemyUnits",
                "2. Destroy all attacking Liao forces.",
            ),
            (
                2,
                2,
                "CaptureSpecificStructure",
                "a - Capture Steiner Sensor Control.",
            ),
            (
                3,
                2,
                "CaptureSpecificStructure",
                "b - Capture the Turret Control at Road Checkpoint.",
            ),
        ],
    ),
    (
        "mc2_07",
        &[
            (
                0,
                1,
                "CaptureSpecificStructure",
                "1. Capture Liao Headquarters",
            ),
            (
                1,
                1,
                "DestroySpecificStructure",
                "2. Destroy Processing Plant",
            ),
            (
                2,
                1,
                "MoveAllSurvivingMechsToArea",
                "3. Extract All Surviving 'Mechs",
            ),
            (3, 2, "DestroyAllEnemyUnits", "Destroy All Enemy Units"),
        ],
    ),
    (
        "mc2_08",
        &[
            (
                0,
                1,
                "DestroySpecificStructure",
                "1 - Destroy Air Traffic Control Tower",
            ),
            (1, 1, "MoveAnyUnitToArea", "2 - Link up with Atlas"),
            (
                2,
                1,
                "DestroySpecificStructure",
                "3 - Destroy Air Traffic Control Tower BETA",
            ),
            (
                3,
                1,
                "MoveAllSurvivingMechsToArea",
                "4 - Evacuate all 'Mechs to extraction point",
            ),
        ],
    ),
    (
        "mc2_09",
        &[
            (
                0,
                1,
                "CaptureSpecificStructure",
                "1 - Capture Weapons Facility",
            ),
            (
                1,
                1,
                "DestroySpecificStructurex4",
                "2 - Destroy Fuel Tanks at Refinery",
            ),
            (
                2,
                1,
                "DestroySpecificStructure",
                "3 - Destroy Bandit HQ in Downed Spacecraft Nosecone",
            ),
            (
                3,
                1,
                "CaptureOrDestroySpecificEnemyUnit",
                "Eliminate Bandit Leader at Bandit HQ",
            ),
            (
                4,
                1,
                "MoveAllSurvivingMechsToArea",
                "Move all Surviving Mechs to Extraction Point",
            ),
        ],
    ),
    (
        "mc2_10",
        &[
            (
                0,
                1,
                "DestroySpecificEnemyUnitx9",
                "Destroy all Gulag Defenders",
            ),
            (
                1,
                1,
                "DestroySpecificEnemyUnitx7",
                "Destroy Steiner Reinforcements",
            ),
            (
                2,
                1,
                "DestroySpecificEnemyUnitx7",
                "Destroy All Airfield Defenders",
            ),
            (
                3,
                1,
                "MoveAllSurvivingMechsToArea",
                "Move All Surviving Mechs to Extraction Point",
            ),
            (
                4,
                2,
                "CaptureSpecificStructure",
                "Sec:  Capture Steiner Headquarters",
            ),
        ],
    ),
    (
        "mc2_11",
        &[
            (
                0,
                2,
                "CaptureSpecificStructurex5",
                "a - Capture RP Buildings at Davion Installation",
            ),
            (
                1,
                2,
                "BooleanFlagIsSetx5",
                "b - Salvage Davion 'Mechs at salvage yard",
            ),
            (
                2,
                1,
                "CaptureSpecificStructure",
                "1 - Capture Davion Weapons Facility",
            ),
            (
                3,
                1,
                "CaptureSpecificStructure",
                "2 - Capture Steiner Weapons Facility",
            ),
            (
                4,
                1,
                "CaptureOrDestroySpecificStructure",
                "3 - Capture Steiner HQ Building",
            ),
            (
                5,
                1,
                "MoveAllSurvivingMechsToArea",
                "4 - Extract at Steiner Drop Ship Pad",
            ),
        ],
    ),
    (
        "mc2_12",
        &[
            (
                0,
                1,
                "CaptureOrDestroySpecificEnemyUnitx2+DestroySpecificEnemyUnitx11",
                "1. Intercept and destroy all lead Convoy elements",
            ),
            (
                1,
                1,
                "CaptureOrDestroySpecificEnemyUnitx4+DestroySpecificEnemyUnitx9",
                "2. Intercept and destroy all units in second Convoy group",
            ),
            (
                2,
                1,
                "CaptureOrDestroySpecificEnemyUnitx4+DestroySpecificEnemyUnitx9",
                "3. Intercept and destroy all units in third Convoy group",
            ),
            (
                3,
                1,
                "CaptureOrDestroySpecificEnemyUnit+DestroySpecificEnemyUnitx9",
                "4. Intercept and destroy all rearguard Convoy elements",
            ),
            (
                4,
                1,
                "MoveAllSurvivingMechsToArea",
                "5. Move all surviving 'Mechs to extraction point",
            ),
        ],
    ),
    (
        "mc2_13",
        &[
            (
                0,
                1,
                "DestroySpecificStructurex2",
                "Destroy Both HPG Station",
            ),
            (
                1,
                1,
                "CaptureSpecificStructure",
                "Capture Weapons Facility #1",
            ),
            (
                2,
                1,
                "CaptureSpecificStructure",
                "Capture Weapons Facility #2",
            ),
            (
                3,
                1,
                "CaptureSpecificStructure",
                "Capture Weapons Facility #3",
            ),
            (4, 1, "DestroySpecificStructure", "Destroy Davion Dropship"),
            (
                5,
                1,
                "MoveAllSurvivingMechsToArea",
                "Move all Surviving 'Mechs to Extraction Point",
            ),
        ],
    ),
    (
        "mc2_14",
        &[
            (
                0,
                1,
                "CaptureSpecificStructure",
                "Contact the Liao Operative",
            ),
            (
                1,
                1,
                "DestroySpecificStructure",
                "Destroy Orbital Gun Alpha",
            ),
            (2, 1, "DestroySpecificStructure", "Destroy Orbital Gun Beta"),
            (
                3,
                1,
                "DestroySpecificStructure",
                "Destroy Orbital Gun Gamma",
            ),
            (
                4,
                1,
                "DestroySpecificEnemyUnit",
                "Destroy Mobile Orbital Gun Omega",
            ),
            (
                5,
                1,
                "MoveAllSurvivingMechsToArea",
                "Move to the Extraction Point",
            ),
            (
                6,
                2,
                "CaptureSpecificStructure",
                "Capture Airport Sensor Control",
            ),
            (
                7,
                2,
                "DestroySpecificStructure",
                "Destroy Fuel Supplies at Resupply Depot",
            ),
            (
                8,
                2,
                "CaptureSpecificStructure",
                "Capture Communications Array",
            ),
            (9, 2, "DestroyAllEnemyUnits", "Destroy all Enemy Units"),
        ],
    ),
    (
        "mc2_15",
        &[
            (
                0,
                1,
                "DestroySpecificEnemyUnitx9",
                "1. Destroy all Legion Tanks",
            ),
            (
                1,
                1,
                "DestroySpecificStructurex3",
                "2. Destroy the Three Industrial Complexes",
            ),
            (
                2,
                1,
                "MoveAllSurvivingMechsToArea",
                "3. Move All Surviving Mech to Extraction Zone",
            ),
            (
                3,
                2,
                "DestroySpecificStructure",
                "a. Destroy Relay Stations' Power Generator",
            ),
            (
                4,
                2,
                "DestroySpecificStructurex2",
                "b. Destroy the Two Medium Factories",
            ),
            (
                5,
                2,
                "MoveAnyUnitToArea",
                "c. Rendezvous with the Liao Operative near the Meeting Hall",
            ),
        ],
    ),
    (
        "mc2_16",
        &[
            (
                0,
                1,
                "CaptureSpecificStructure",
                "1. Capture Sensor Control",
            ),
            (
                1,
                1,
                "DestroySpecificEnemyUnitx14",
                "2. Destroy All Base Defenders",
            ),
            (
                2,
                1,
                "DestroySpecificEnemyUnitx25",
                "3. Destroy All Reinforcements",
            ),
            (
                3,
                1,
                "CaptureSpecificStructure",
                "4. Capture Davion Base HQ",
            ),
            (
                4,
                1,
                "MoveAllSurvivingMechsToArea",
                "5. Move All Surviving Mechs to Extraction Point",
            ),
            (
                5,
                2,
                "CaptureSpecificStructurex3",
                "a. Capture All Main Base Davion Turret Controls",
            ),
            (
                6,
                2,
                "CaptureSpecificStructurex2",
                "b. Capture All Main Base Davion Gate Controls",
            ),
        ],
    ),
    (
        "mc2_17",
        &[
            (
                0,
                1,
                "ElapsedMissionTime",
                "Escort Jason Cho to Rendezvous Location",
            ),
            (1, 1, "DestroyAllEnemyUnits", "Kill all Enemy Forces"),
            (
                2,
                2,
                "MoveAnyUnitToArea",
                "Sec: Take Control of Steiner Base",
            ),
        ],
    ),
    (
        "mc2_18",
        &[
            (
                0,
                1,
                "DestroyAllEnemyUnits",
                "1. Defend City from Liao and Steiner Forces",
            ),
            (
                1,
                1,
                "DestroyAllEnemyUnits",
                "2. Destroy all Liao/Steiner Forces",
            ),
            (
                2,
                2,
                "CaptureSpecificStructure",
                "a. Recapture Davion Headquarters",
            ),
            (
                3,
                2,
                "CaptureSpecificStructure",
                "b. Recapture the City's Power Generator",
            ),
            (
                4,
                2,
                "CaptureSpecificStructure",
                "c. Recapture Davion Sensor Control",
            ),
        ],
    ),
    (
        "mc2_19",
        &[
            (
                0,
                1,
                "DestroyNumberOfEnemyUnits",
                "1. Destroy 1/3 of All Enemy Forces in the Sector",
            ),
            (
                1,
                1,
                "MoveAllSurvivingMechsToArea",
                "2. Move All Surviving Mech to Extraction Point",
            ),
            (
                2,
                2,
                "DestroyNumberOfEnemyUnits",
                "a. Destroy 2/3 of All Enemy Forces in the Sector",
            ),
            (
                3,
                2,
                "DestroyAllEnemyUnits",
                "b. Destroy  All Enemy Forces in the Sector",
            ),
            (
                4,
                2,
                "CaptureSpecificStructure",
                "c. Captrue Liao Headquarters",
            ),
            (
                5,
                2,
                "DestroySpecificStructurex3",
                "d. Destroy All 3 Power Generators at the Liao Base",
            ),
            (
                6,
                2,
                "DestroySpecificStructurex2",
                "e. Destroy Both Industrial Complex at Liao Industrial Area",
            ),
            (
                7,
                2,
                "DestroySpecificStructurex5",
                "f. Destroy All 5 Slayer Bombers at  Liao Airstripe",
            ),
            (
                8,
                2,
                "CaptureSpecificStructure",
                "g. Capture Weapon Facility",
            ),
        ],
    ),
    (
        "mc2_20",
        &[
            (0, 1, "MoveAnyUnitToArea", "1. Goto Davion Dropship"),
            (
                1,
                1,
                "CaptureSpecificStructure",
                "2. Rescue Drop Ship Crew at Prison",
            ),
            (
                2,
                1,
                "DestroySpecificStructure",
                "3. Destroy Orbital Turret",
            ),
            (
                3,
                1,
                "DestroySpecificEnemyUnit",
                "4. Destroy Mobile Orbital Gun#1",
            ),
            (
                4,
                1,
                "DestroySpecificEnemyUnit",
                "5. Destroy Moblie Orbital Gun #2",
            ),
            (
                5,
                1,
                "MoveAllSurvivingMechsToArea",
                "6. Go To Davion Dropship for Extraction",
            ),
            (6, 2, "DestroyAllEnemyUnits", "a. Destroy All Enemy Forces"),
        ],
    ),
    (
        "mc2_21",
        &[
            (
                0,
                1,
                "MoveAllSurvivingMechsToArea",
                "1 - Move all units to Extraction Point",
            ),
            (
                1,
                2,
                "DestroySpecificStructurex2",
                "a - Destroy Ammo Dump ALPHA",
            ),
            (
                2,
                2,
                "DestroySpecificStructurex3",
                "b - Destroy Ammo Dump BETA",
            ),
            (
                3,
                2,
                "DestroySpecificStructure",
                "c - Destroy Power Generator ALPHA",
            ),
            (
                4,
                2,
                "DestroySpecificStructure",
                "d - Destroy Power Generator BETA",
            ),
            (
                5,
                2,
                "CaptureSpecificStructure",
                "a. Capture the Weapons Facility",
            ),
            (
                6,
                2,
                "DestroySpecificStructure",
                "f. Destroy Industrial Complex",
            ),
        ],
    ),
    (
        "mc2_22",
        &[
            (
                0,
                1,
                "DestroySpecificEnemyUnitx7",
                "Destroy the Liao Elite Guard",
            ),
            (
                1,
                1,
                "DestroySpecificStructurex3",
                "Destroy the Liao Palace",
            ),
            (
                2,
                1,
                "MoveAllSurvivingMechsToArea",
                "Move all surviving Mechs to Extraction Point",
            ),
        ],
    ),
    (
        "mc2_23",
        &[
            (
                0,
                1,
                "DestroySpecificEnemyUnitx16",
                "1. Clear Out Steiner Forces at Starport",
            ),
            (
                1,
                1,
                "DestroySpecificEnemyUnitx10",
                "2. Eliminate Steiner Reinforcements",
            ),
            (
                2,
                2,
                "CaptureSpecificStructure",
                "a. Capture Sensor Control",
            ),
            (
                3,
                2,
                "DestroySpecificEnemyUnitx5",
                "b. Clear Out Patrols Around Town to Free Rebels",
            ),
            (
                4,
                2,
                "DestroySpecificStructurex3",
                "c. Destroy Steiner Dropships",
            ),
            (
                5,
                2,
                "CaptureSpecificUnit",
                "d. Capture Steiner Mobile HQ Vehicle",
            ),
        ],
    ),
    (
        "mc2_24",
        &[
            (
                0,
                1,
                "DestroySpecificStructurex3",
                "Destroy Steiner High Command",
            ),
            (1, 1, "DestroySpecificEnemyUnit", "Eliminate Renard"),
            (
                2,
                1,
                "MoveAllSurvivingMechsToArea",
                "Extract All Surviving Mechs",
            ),
        ],
    ),
    (
        "tut_01",
        &[
            (0, 1, "MoveAnyUnitToArea", "Move All Units to Nav Point 1"),
            (1, 1, "MoveAnyUnitToArea", "Move All Units to Nav Point 2"),
            (2, 1, "MoveAnyUnitToArea", "Move All Units to Nav Point 3"),
            (3, 1, "MoveAnyUnitToArea", "Move Any Unit to Nav Point 4"),
        ],
    ),
    (
        "tut_02",
        &[
            (0, 1, "MoveAnyUnitToArea", "1. Move to Ridge"),
            (1, 1, "MoveAnyUnitToArea", "Move Specific Unit to Area"),
            (
                2,
                1,
                "DestroySpecificEnemyUnitx3",
                "Destroy Specific Units (Cargo Trucks)",
            ),
            (3, 1, "MoveAnyUnitToArea", "Move All Units To Area"),
            (
                4,
                1,
                "DestroySpecificEnemyUnitx3",
                "Destroy All Enemy Units",
            ),
            (5, 1, "DestroySpecificEnemyUnit", "Destroy 1st Cargo Truck"),
            (
                6,
                1,
                "DestroySpecificStructure",
                "Hidden Trigger - For VO to play when building destroyed",
            ),
        ],
    ),
    (
        "tut_03",
        &[
            (0, 1, "MoveAnyUnitToArea", "Move All Units To Area"),
            (1, 1, "MoveAnyUnitToArea", "Move All Units To Area"),
            (
                2,
                1,
                "CaptureSpecificUnitx3",
                "Capture Specific Units (3 HQ Vehicles)",
            ),
            (
                3,
                1,
                "CaptureSpecificStructure",
                "Hidden Trigger - Capture Gate Control",
            ),
            (
                4,
                1,
                "CaptureSpecificStructure",
                "Hidden Trigger - Capture Turret Control",
            ),
            (
                5,
                1,
                "CaptureSpecificStructure",
                "Hidden Trigger - Capture Sensor Control",
            ),
        ],
    ),
    (
        "tut_04",
        &[
            (0, 1, "CaptureSpecificStructure", "Capture RP Building"),
            (
                1,
                1,
                "DestroySpecificEnemyUnit",
                "Destroy Specific Enemy Unit",
            ),
            (2, 1, "CaptureSpecificUnit", "Salvage Enemy 'Mech"),
            (
                3,
                1,
                "MoveAllSurvivingMechsToArea",
                "Move All Units To Area",
            ),
        ],
    ),
    (
        "tut_05",
        &[
            (0, 1, "MoveAnyUnitToArea", "1. Move Unit To Area"),
            (1, 1, "MoveAnyUnitToArea", "2. Move Minelayer to Nav Marker"),
            (
                2,
                1,
                "DestroySpecificEnemyUnitx3",
                "3. Destroy Approaching Enemy Units",
            ),
            (3, 1, "MoveAnyUnitToArea", "4. Move Scout Copter to Island"),
            (4, 1, "DestroySpecificEnemyUnitx3", "5. Destroy Convoy"),
        ],
    ),
];

/// Gameplay-relevant unavailable notices each mission's script path reaches today (everything not presentation-only), one entry
/// per mission that has any: the cause of the `#[ignore = "port gap"]` on its `script_path` test. `gap_table_is_current` checks it
/// against the run, so it can neither hide a new gap nor outlive a fixed one.
// BEGIN GENERATED GAPS (objective_harness regenerate)
const GAPS: &[(&str, &[&str])] = &[];
// END GENERATED GAPS

fn vfs() -> Option<Vfs> {
    let root = common::data_root()?;
    Some(Vfs::from_system_cfg(&root).expect("mount the game archives"))
}

fn index_of(mission: &str) -> usize {
    MISSIONS
        .iter()
        .position(|m| m.0 == mission)
        .unwrap_or_else(|| panic!("{mission} is not a campaign or tutorial mission"))
}

/// The played mission, once per mission per test binary (the tests share it). `None` without `MC2_DATA`.
fn run(mission: &str) -> Option<Arc<MissionRun>> {
    static RUNS: [OnceLock<Result<Arc<MissionRun>, String>>; 29] = [const { OnceLock::new() }; 29];
    let mut v = vfs()?;
    RUNS[index_of(mission)]
        .get_or_init(|| Harness::play_mission(&mut v, mission).map(Arc::new))
        .clone()
        .map(Some)
        .unwrap_or_else(|e| panic!("{mission}: {e}"))
}

fn all_runs() -> Option<Vec<Arc<MissionRun>>> {
    MISSIONS.iter().map(|m| run(m.0)).collect()
}

fn check_win(mission: &str) {
    let Some(run) = run(mission) else { return };
    let want = MISSIONS[index_of(mission)].1;

    // The objectives are the mission data's.
    let got: Vec<(usize, u32, String, String)> = run
        .visible
        .iter()
        .map(|o| (o.index, o.priority, o.spec.clone(), o.title.clone()))
        .collect();
    let want_owned: Vec<(usize, u32, String, String)> = want
        .iter()
        .map(|&(i, p, s, t)| (i, p, s.to_string(), t.to_string()))
        .collect();
    assert_eq!(got, want_owned, "{mission}: the mission data changed");

    // Every visible objective is marked complete by the sim; the primary ones while the mission still ran.
    for &(i, priority, _, title) in want {
        assert_eq!(
            run.outcome.objectives[i].status,
            S,
            "{mission}: objective {i} {title:?} is not complete in the end-of-mission report\n{}",
            run.log.join("\n")
        );
        if priority == 1 {
            assert_eq!(
                run.runtime[i], S,
                "{mission}: primary objective {i} {title:?} was not resolved by the sim during the mission"
            );
        }
    }
    // Nothing the player sees failed. (mc2_10's hidden "Protect the Convoy" trigger reads Failed in the C++ too: its failure
    // condition `DestroySpecificEnemyUnit` counts a living unit of the objective's own team as captured, objective.cpp:160-175, 460.)
    for &(i, ..) in want {
        assert_ne!(
            run.outcome.objectives[i].status,
            ObjectiveStatus::Failed,
            "{mission}: objective {i} failed on the winning path"
        );
    }
    // The mission result is a win, through the 5 s termination timer and the results screen.
    assert_eq!(run.result, WIN_BIG, "{mission}: scenario result");
    assert_eq!(run.outcome.result, MissionResult::PlayerWinBig);
}

fn check_script_path(mission: &str) {
    let Some(run) = run(mission) else { return };
    let gameplay = run.gameplay_unavailable();
    assert!(
        gameplay.is_empty(),
        "{mission}: unavailable notices on the script path: {gameplay:?}"
    );
}

macro_rules! mission_tests {
    ($($win:ident, $path:ident, $m:literal $(, gap = $gap:literal)?;)*) => {$(
        #[test]
        fn $win() {
            check_win($m);
        }
        #[test]
        $(#[ignore = $gap])?
        fn $path() {
            check_script_path($m);
        }
    )*};
}

mission_tests! {
    mc2_01_objectives_win, mc2_01_script_path_has_no_unported_gameplay, "mc2_01";
    mc2_02_objectives_win, mc2_02_script_path_has_no_unported_gameplay, "mc2_02";
    mc2_03_objectives_win, mc2_03_script_path_has_no_unported_gameplay, "mc2_03";
    mc2_04_objectives_win, mc2_04_script_path_has_no_unported_gameplay, "mc2_04";
    mc2_05_objectives_win, mc2_05_script_path_has_no_unported_gameplay, "mc2_05";
    mc2_06_objectives_win, mc2_06_script_path_has_no_unported_gameplay, "mc2_06";
    mc2_07_objectives_win, mc2_07_script_path_has_no_unported_gameplay, "mc2_07";
    mc2_08_objectives_win, mc2_08_script_path_has_no_unported_gameplay, "mc2_08";
    mc2_09_objectives_win, mc2_09_script_path_has_no_unported_gameplay, "mc2_09";
    mc2_10_objectives_win, mc2_10_script_path_has_no_unported_gameplay, "mc2_10";
    mc2_11_objectives_win, mc2_11_script_path_has_no_unported_gameplay, "mc2_11";
    mc2_12_objectives_win, mc2_12_script_path_has_no_unported_gameplay, "mc2_12";
    mc2_13_objectives_win, mc2_13_script_path_has_no_unported_gameplay, "mc2_13";
    mc2_14_objectives_win, mc2_14_script_path_has_no_unported_gameplay, "mc2_14";
    mc2_15_objectives_win, mc2_15_script_path_has_no_unported_gameplay, "mc2_15";
    mc2_16_objectives_win, mc2_16_script_path_has_no_unported_gameplay, "mc2_16";
    mc2_17_objectives_win, mc2_17_script_path_has_no_unported_gameplay, "mc2_17";
    mc2_18_objectives_win, mc2_18_script_path_has_no_unported_gameplay, "mc2_18";
    mc2_19_objectives_win, mc2_19_script_path_has_no_unported_gameplay, "mc2_19";
    mc2_20_objectives_win, mc2_20_script_path_has_no_unported_gameplay, "mc2_20";
    mc2_21_objectives_win, mc2_21_script_path_has_no_unported_gameplay, "mc2_21";
    mc2_22_objectives_win, mc2_22_script_path_has_no_unported_gameplay, "mc2_22";
    mc2_23_objectives_win, mc2_23_script_path_has_no_unported_gameplay, "mc2_23";
    mc2_24_objectives_win, mc2_24_script_path_has_no_unported_gameplay, "mc2_24";
    tut_01_objectives_win, tut_01_script_path_has_no_unported_gameplay, "tut_01";
    tut_02_objectives_win, tut_02_script_path_has_no_unported_gameplay, "tut_02";
    tut_03_objectives_win, tut_03_script_path_has_no_unported_gameplay, "tut_03";
    tut_04_objectives_win, tut_04_script_path_has_no_unported_gameplay, "tut_04";
    tut_05_objectives_win, tut_05_script_path_has_no_unported_gameplay, "tut_05";
}

/// The gap table (and so every `#[ignore = "port gap"]` above) is what the runs show.
#[test]
#[ignore = "freshness check: run by the report job, not the merge gate (see OBJECTIVE-HARNESS.md)"]
fn gap_table_is_current() {
    let Some(runs) = all_runs() else { return };
    let mut seen: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in &runs {
        let mut g = r.gameplay_unavailable();
        g.sort();
        if !g.is_empty() {
            seen.insert(r.mission.clone(), g);
        }
    }
    let table: BTreeMap<String, Vec<String>> = GAPS
        .iter()
        .map(|(m, g)| {
            (
                (*m).to_string(),
                g.iter().map(|s| (*s).to_string()).collect(),
            )
        })
        .collect();
    assert_eq!(
        seen, table,
        "GAPS is out of date; update it and the #[ignore] reasons"
    );
}

/// The species the shipped missions use, and the ones no mission uses.
#[test]
fn condition_species_coverage() {
    let Some(runs) = all_runs() else { return };
    let used: std::collections::BTreeSet<String> = runs
        .iter()
        .flat_map(|r| r.species_used.iter().cloned())
        .collect();
    let expected = [
        "BooleanFlagIsSet",
        "CaptureOrDestroySpecificEnemyUnit",
        "CaptureOrDestroySpecificStructure",
        "CaptureSpecificStructure",
        "CaptureSpecificUnit",
        "DestroyAllEnemyUnits",
        "DestroyNumberOfEnemyUnits",
        "DestroySpecificEnemyUnit",
        "DestroySpecificStructure",
        "ElapsedMissionTime",
        "MoveAllSurvivingMechsToArea",
        "MoveAnyUnitToArea",
    ];
    assert_eq!(
        used.iter().map(String::as_str).collect::<Vec<_>>(),
        expected,
        "the species the 24 campaign and 5 tutorial missions use changed; add a negative test for each new one"
    );
    assert_eq!(ConditionSpecies::ALL.len(), 24);
}

// ---------------------------------------------------------------------------------------------------------------------------
// Negatives: one per condition species the shipped missions use.
// ---------------------------------------------------------------------------------------------------------------------------

fn load(mission: &str) -> Option<Harness> {
    let mut v = vfs()?;
    let mut h = Harness::load(&mut v, mission).expect("load");
    h.step_n(5).unwrap();
    Some(h)
}

fn conditions(h: &Harness, i: usize) -> Vec<mc2_sim::mission::objective::ResolvedCondition> {
    let w = h.world();
    w.mission.objectives[w.teams.home].entries[i]
        .conditions
        .clone()
}

/// Steps `n` frames and returns objective `i`'s status.
fn after(h: &mut Harness, n: u64, i: usize) -> ObjectiveStatus {
    h.step_n(n).unwrap();
    h.status(i)
}

fn handle_of(
    h: &Harness,
    c: &mc2_sim::mission::objective::ResolvedCondition,
) -> mc2_sim::ids::ObjectHandle {
    h.world()
        .objects
        .by_watch_id(c.wid)
        .expect("the condition's object exists")
}

fn area_of(c: &Condition) -> (f32, f32, f32) {
    match c {
        Condition::MoveAnyUnitToArea { area } | Condition::MoveAllSurvivingMechsToArea { area } => {
            (area.x, area.y, area.radius)
        }
        other => panic!("not an area condition: {other:?}"),
    }
}

/// `MoveAnyUnitToArea` (objective.cpp:886): a lance just outside the area leaves it incomplete; inside completes it.
#[test]
fn negative_move_any_unit_to_area() {
    let Some(mut h) = load("mc2_01") else { return };
    assert_eq!(
        after(&mut h, 30, 0),
        U,
        "nobody is in the area at the start"
    );
    let (x, y, r) = area_of(&conditions(&h, 0)[0].condition);
    for m in hooks::home_lance(h.world()) {
        hooks::move_mover(&mut h.sim.world, m, x + r + 8.0, y);
    }
    assert_eq!(
        after(&mut h, 10, 0),
        U,
        "the lance is just outside the area"
    );
    for m in hooks::home_lance(h.world()) {
        hooks::move_mover(&mut h.sim.world, m, x + r - 8.0, y);
    }
    assert_eq!(
        after(&mut h, 10, 0),
        S,
        "control: inside the area completes it"
    );
}

/// `MoveAllSurvivingMechsToArea` (objective.cpp:1031): one mech left behind keeps the extraction incomplete; so does a live enemy
/// within `MaxExtractUnitDistance` of the area.
#[test]
fn negative_move_all_surviving_mechs_to_area() {
    let Some(mut h) = load("mc2_01") else { return };
    for i in 0..5 {
        assert_eq!(
            h.play_objective(i, 30).unwrap(),
            S,
            "objective {i} unlocks the extraction"
        );
    }
    assert_eq!(h.status(5), U);
    let (x, y, _) = area_of(&conditions(&h, 5)[0].condition);
    let lance = hooks::home_lance(h.world());
    assert!(lance.len() >= 2, "the lance has several units");
    let (last, rest) = lance.split_last().unwrap();
    for &m in rest {
        hooks::move_mover(&mut h.sim.world, m, x, y);
    }
    assert_eq!(after(&mut h, 20, 5), U, "one mech is still outside");
    // Control: bring it in; an enemy just beside the area blocks the extraction until it is gone.
    let bandit = h
        .world()
        .objects
        .movers
        .iter()
        .copied()
        .find(|&m| {
            hooks::enemies(h.world(), h.world().teams.home as i32).contains(&m)
                && !hooks::mover_down(h.world(), m)
        })
        .expect("a living enemy");
    hooks::move_mover(&mut h.sim.world, bandit, x + 40.0, y);
    hooks::move_mover(&mut h.sim.world, *last, x, y);
    assert_eq!(
        after(&mut h, 20, 5),
        U,
        "a living enemy is within the extraction distance"
    );
    // Control: the enemy leaves (and so does every other unit of another commander within the distance, as the extraction hook
    // does it), everyone is in: complete.
    let cond = conditions(&h, 5)[0].clone();
    hooks::satisfy_condition(&mut h.sim.world, 0, &cond);
    assert_eq!(after(&mut h, 20, 5), S, "control: everyone in, area clear");
}

/// `DestroySpecificStructure` (objective.cpp:511).
#[test]
fn negative_destroy_specific_structure() {
    let Some(mut h) = load("mc2_01") else { return };
    assert_eq!(h.play_objective(0, 30).unwrap(), S);
    assert_eq!(after(&mut h, 60, 1), U, "the hangar stands");
    let c = conditions(&h, 1);
    let b = handle_of(&h, &c[0]);
    hooks::destroy_building(&mut h.sim.world, b);
    assert_eq!(after(&mut h, 5, 1), S, "control: destroyed");
}

/// `DestroySpecificEnemyUnit` (objective.cpp:460): seven of eight is not enough.
#[test]
fn negative_destroy_specific_enemy_unit() {
    let Some(mut h) = load("mc2_01") else { return };
    assert_eq!(h.play_objective(0, 30).unwrap(), S);
    assert_eq!(h.play_objective(1, 30).unwrap(), S);
    let c = conditions(&h, 2);
    assert_eq!(c.len(), 8);
    for cond in &c[..7] {
        let m = handle_of(&h, cond);
        hooks::destroy_mover(&mut h.sim.world, m);
    }
    assert_eq!(after(&mut h, 30, 2), U, "one bandit is left");
    let m = handle_of(&h, &c[7]);
    hooks::destroy_mover(&mut h.sim.world, m);
    assert_eq!(after(&mut h, 5, 2), S, "control: all eight");
}

/// `CaptureStructure` (objective.cpp:784): an untouched building leaves it incomplete.
#[test]
fn negative_capture_structure() {
    let Some(mut h) = load("mc2_02") else { return };
    assert_eq!(
        after(&mut h, 60, 0),
        U,
        "the resource building is the enemy's"
    );
    let c = conditions(&h, 0);
    let b = handle_of(&h, &c[0]);
    let team = h.world().teams.home as i32;
    hooks::capture_building(&mut h.sim.world, b, team);
    assert_eq!(after(&mut h, 5, 0), S, "control: captured");
}

/// `CaptureUnit` (objective.cpp:730).
#[test]
fn negative_capture_unit() {
    let Some(mut h) = load("mc2_03") else { return };
    assert_eq!(after(&mut h, 60, 2), U, "the HQ vehicle is the enemy's");
    let c = conditions(&h, 2);
    let m = handle_of(&h, &c[0]);
    let team = h.world().teams.home as i32;
    hooks::capture_mover(&mut h.sim.world, m, team);
    assert_eq!(after(&mut h, 5, 2), S, "control: captured");
}

/// `CaptureOrDestroySpecificEnemyUnit` (objective.cpp:591): all but one of the convoy elements leaves it incomplete.
#[test]
fn negative_capture_or_destroy_specific_enemy_unit() {
    let Some(mut h) = load("mc2_12") else { return };
    let c = conditions(&h, 0);
    assert!(c.len() > 2);
    for cond in &c[..c.len() - 1] {
        let m = handle_of(&h, cond);
        hooks::destroy_mover(&mut h.sim.world, m);
    }
    assert_eq!(after(&mut h, 30, 0), U, "one convoy element is left");
    let m = handle_of(&h, c.last().unwrap());
    hooks::destroy_mover(&mut h.sim.world, m);
    assert_eq!(after(&mut h, 5, 0), S, "control: the last one is down");
}

/// `CaptureOrDestroySpecificStructure` (objective.cpp:636).
#[test]
fn negative_capture_or_destroy_specific_structure() {
    let Some(mut h) = load("mc2_11") else { return };
    assert_eq!(after(&mut h, 60, 4), U, "the HQ is the enemy's");
    let c = conditions(&h, 4);
    let b = handle_of(&h, &c[0]);
    let team = h.world().teams.home as i32;
    hooks::capture_building(&mut h.sim.world, b, team);
    assert_eq!(after(&mut h, 5, 4), S, "control: captured");
}

/// `DestroyAllEnemyUnits` (objective.cpp:365): the last enemy decides it.
#[test]
fn negative_destroy_all_enemy_units() {
    let Some(mut h) = load("mc2_07") else { return };
    let team = h.world().teams.home as i32;
    let enemies = hooks::enemies(h.world(), team);
    assert!(enemies.len() > 1);
    for &m in &enemies[..enemies.len() - 1] {
        hooks::destroy_mover(&mut h.sim.world, m);
    }
    assert_eq!(after(&mut h, 30, 3), U, "one enemy is left");
    hooks::destroy_mover(&mut h.sim.world, *enemies.last().unwrap());
    assert_eq!(after(&mut h, 5, 3), S, "control: no enemy left");
}

/// `DestroyNumberOfEnemyUnits` (objective.cpp:394): 24 of 25 is not enough.
#[test]
fn negative_destroy_number_of_enemy_units() {
    let Some(mut h) = load("mc2_19") else { return };
    let team = h.world().teams.home as i32;
    let enemies = hooks::enemies(h.world(), team);
    assert!(enemies.len() >= 25);
    for &m in &enemies[..24] {
        hooks::destroy_mover(&mut h.sim.world, m);
    }
    assert_eq!(after(&mut h, 30, 0), U, "24 of 25");
    hooks::destroy_mover(&mut h.sim.world, enemies[24]);
    assert_eq!(after(&mut h, 5, 0), S, "control: 25");
}

/// `CBooleanFlagIsSet` (objective.cpp:1127): the flags are set by other objectives' actions, never by the harness; one of two is
/// not enough.
#[test]
fn negative_boolean_flag_is_set() {
    let Some(mut h) = load("mc2_02") else { return };
    assert_eq!(h.play_objective(0, 30).unwrap(), S);
    assert_eq!(h.status(5), U);
    assert_eq!(h.play_objective(1, 30).unwrap(), S, "sets flag 3");
    assert_eq!(after(&mut h, 30, 5), U, "flag 4 is still unset");
    assert_eq!(h.play_objective(2, 30).unwrap(), S);
    assert_eq!(h.play_objective(3, 30).unwrap(), S, "sets flag 4");
    assert_eq!(after(&mut h, 5, 5), S, "control: both flags set by the sim");
}

/// `CElapsedMissionTime` (objective.cpp:1150).
#[test]
fn negative_elapsed_mission_time() {
    let Some(mut h) = load("mc2_17") else { return };
    let Condition::ElapsedMissionTime { time } = conditions(&h, 0)[0].condition else {
        panic!("mc2_17 objective 0 is the timer");
    };
    assert_eq!(time, 3.0);
    assert_eq!(after(&mut h, 30, 0), U, "one second in");
    hooks::advance_time_to(&mut h.sim.world, time - 0.1);
    assert_eq!(after(&mut h, 2, 0), U, "just before the time");
    hooks::advance_time_to(&mut h.sim.world, time + 0.5);
    assert_eq!(after(&mut h, 2, 0), S, "control: past the time");
}

/// Left alone, no mission ends or completes a primary objective that needs the player (nothing is won by itself).
#[test]
fn untouched_missions_do_not_win() {
    let Some(mut v) = vfs() else { return };
    for &(mission, _) in MISSIONS {
        let mut h = Harness::load(&mut v, mission).expect("load");
        h.step_n(300).unwrap();
        assert_eq!(h.result, None, "{mission}: ended without the player");
        let w = h.world();
        let t = &w.mission.objectives[w.teams.home];
        let done_primaries = t
            .entries
            .iter()
            .filter(|e| e.def.priority == 1 && e.runtime.status == S)
            .count();
        let primaries = t.entries.iter().filter(|e| e.def.priority == 1).count();
        assert!(
            done_primaries < primaries,
            "{mission}: every primary objective completed by itself"
        );
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// The report.
// ---------------------------------------------------------------------------------------------------------------------------

/// The lane that ports a gap, from the unavailable notice (the same names the `#[ignore]` reasons carry).
fn owner_lane(notice: &str) -> &'static str {
    let n = notice.to_ascii_lowercase();
    if n.contains("iscaptur")
        || n.contains("isteamtargeting")
        || n.contains("capture orders")
        || n.ends_with("pilot:capture")
    {
        "sim-capture-guard"
    } else if n.starts_with("combat:") {
        "lane D (combat)"
    } else {
        "abl-natives-object"
    }
}

/// The generated part of `docs/OBJECTIVE-HARNESS.md`: one row per mission plus the presentation-only notices.
fn render_report(runs: &[Arc<MissionRun>]) -> String {
    let mut s = String::new();
    s += "| Mission | Objectives (index: condition species, priority) | Played order | Win test | No-unavailable test (natives reached) | Owner lane | Frames |\n|---|---|---|---|---|---|---|\n";
    for r in runs {
        let objs: Vec<String> = r
            .visible
            .iter()
            .map(|o| format!("{}: {} (P{})", o.index, o.spec, o.priority))
            .collect();
        let mut gaps = r.gameplay_unavailable();
        gaps.sort();
        let mut lanes: Vec<&str> = gaps.iter().map(|g| owner_lane(g)).collect();
        lanes.sort();
        lanes.dedup();
        let (path, lane) = if gaps.is_empty() {
            ("pass".to_string(), "-".to_string())
        } else {
            (format!("ignored: {}", gaps.join(", ")), lanes.join(" + "))
        };
        s += &format!(
            "| {} | {} | {:?} | {} | {} | {} | {} |\n",
            r.mission,
            objs.join("<br>"),
            r.order,
            if r.result == WIN_BIG {
                "pass (WIN, 5)"
            } else {
                "FAIL (not won)"
            },
            path,
            lane,
            r.frames,
        );
    }
    s += "\nPresentation-only notices (camera, music, speech, video, movie mode) seen per mission:\n\n";
    for r in runs {
        let p: Vec<String> = r
            .unavailable
            .iter()
            .filter(|u| hooks::is_presentation(u))
            .map(hooks::notice_label)
            .collect();
        s += &format!(
            "* {}: {}\n",
            r.mission,
            if p.is_empty() {
                "none".into()
            } else {
                p.join(", ")
            }
        );
    }
    s
}

/// Writes the generated part of `docs/OBJECTIVE-HARNESS.md` when `OBJECTIVE_HARNESS_REPORT` names a file.
#[test]
fn write_report() {
    let Ok(path) = std::env::var("OBJECTIVE_HARNESS_REPORT") else {
        return;
    };
    let Some(runs) = all_runs() else { return };
    std::fs::write(path, render_report(&runs)).unwrap();
}

/// The committed table is what this run produces: fails when the doc and the sim disagree.
#[test]
#[ignore = "freshness check: run by the report job, not the merge gate (see OBJECTIVE-HARNESS.md)"]
fn report_is_current() {
    let Some(runs) = all_runs() else { return };
    let doc = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../docs/OBJECTIVE-HARNESS.md"
    ))
    .unwrap();
    let (a, b) = marker_span(&doc, REPORT_BEGIN, REPORT_END);
    let committed = doc[a..b].trim_end();
    let fresh = render_report(&runs);
    assert_eq!(
        committed,
        fresh.trim_end(),
        "docs/OBJECTIVE-HARNESS.md is stale; regenerate with OBJECTIVE_HARNESS_REGEN=1 cargo test -p mc2-sim --test objective_harness regenerate -- --ignored"
    );
}

const REPORT_BEGIN: &str = "<!-- BEGIN GENERATED (objective_harness write_report) -->\n";
const REPORT_END: &str = "<!-- END GENERATED -->";
const GAPS_BEGIN: &str = "// BEGIN GENERATED GAPS (objective_harness regenerate)\n";
const GAPS_END: &str = "// END GENERATED GAPS";

/// Byte range of the text between the `begin` and `end` markers.
fn marker_span(text: &str, begin: &str, end: &str) -> (usize, usize) {
    let a = text
        .find(begin)
        .unwrap_or_else(|| panic!("marker {begin:?}"))
        + begin.len();
    let b = a + text[a..]
        .find(end)
        .unwrap_or_else(|| panic!("marker {end:?}"));
    (a, b)
}

/// `text` with the region between the markers replaced by `body` (one trailing newline).
fn splice(text: &str, begin: &str, end: &str, body: &str) -> String {
    let (a, b) = marker_span(text, begin, end);
    format!("{}{}\n{}", &text[..a], body.trim_end(), &text[b..])
}

/// Mission to its sorted gameplay-unavailable notices, for the missions that have any.
fn gap_map(runs: &[Arc<MissionRun>]) -> BTreeMap<String, Vec<String>> {
    let mut seen = BTreeMap::new();
    for r in runs {
        let mut g = r.gameplay_unavailable();
        g.sort();
        if !g.is_empty() {
            seen.insert(r.mission.clone(), g);
        }
    }
    seen
}

/// The `GAPS` const for `gaps` (missions sorted), laid out the way rustfmt does with the default widths: a tuple stays on
/// one line when it is at most 60 columns (`fn_call_width`), else its mission and list go on their own lines; the list
/// stays on one line when its items are at most 60 columns (`array_width`), or it is a single item that fits in 100
/// columns, else one notice per line. `render_gaps_is_rustfmt_stable` checks that rustfmt leaves this layout alone.
fn render_gaps(gaps: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::from("const GAPS: &[(&str, &[&str])] = &[\n");
    for (m, g) in gaps {
        let list: Vec<String> = g.iter().map(|n| format!("{n:?}")).collect();
        let items = list.join(", ");
        let one = format!("({m:?}, &[{items}])");
        if one.len() <= 60 {
            out += &format!("    {one},\n");
            continue;
        }
        out += &format!("    (\n        {m:?},\n");
        if items.len() <= 60 || (list.len() == 1 && 8 + items.len() + 4 <= 100) {
            out += &format!("        &[{items}],\n");
        } else {
            out += "        &[\n";
            for n in &list {
                out += &format!("            {n},\n");
            }
            out += "        ],\n";
        }
        out += "    ),\n";
    }
    out += "];\n";
    out
}

/// Rewrites the report block of `docs/OBJECTIVE-HARNESS.md` and the `GAPS` const of this file in place
/// (`OBJECTIVE_HARNESS_REGEN=1`; does nothing otherwise).
#[test]
#[ignore = "regeneration: run by the report job (see OBJECTIVE-HARNESS.md)"]
fn regenerate() {
    if std::env::var("OBJECTIVE_HARNESS_REGEN").as_deref() != Ok("1") {
        return;
    }
    let Some(runs) = all_runs() else {
        panic!("OBJECTIVE_HARNESS_REGEN=1 needs the game data (MC2_DATA); nothing was regenerated");
    };
    let doc_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/OBJECTIVE-HARNESS.md");
    let doc = std::fs::read_to_string(doc_path).unwrap();
    let doc = splice(&doc, REPORT_BEGIN, REPORT_END, &render_report(&runs));
    std::fs::write(doc_path, doc).unwrap();

    let src_path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/objective_harness.rs");
    let src = std::fs::read_to_string(src_path).unwrap();
    let src = splice(&src, GAPS_BEGIN, GAPS_END, &render_gaps(&gap_map(&runs)));
    std::fs::write(src_path, src).unwrap();
}

/// The GAPS renderer on a sample run set: sorted, deterministic, and byte-equal to what rustfmt makes of it.
#[test]
fn render_gaps_is_rustfmt_stable() {
    use std::io::Write;
    let mut gaps = BTreeMap::new();
    gaps.insert(
        "mc2_24".to_string(),
        vec!["script:b".into(), "x \"quoted\"".into()],
    );
    gaps.insert("mc2_05".to_string(), vec!["script:convertcoords".into()]);
    gaps.insert(
        "mc2_17".to_string(),
        vec![
            "pilot:iscapturable".into(),
            "pilot:iscaptured".into(),
            "pilot:isteamtargeting".into(),
        ],
    );
    gaps.insert(
        "mc2_20".to_string(),
        vec!["combat:lane D: Team::teamLineOfSight for indirect turret fire".into()],
    );
    gaps.insert(
        "mc2_13".to_string(),
        vec![
            "pilot:getrelativepositiontoobject".into(),
            "script:convertcoords".into(),
        ],
    );
    gaps.insert(
        "mc2_22".to_string(),
        vec![
            "combat:lane D: Team::teamLineOfSight for indirect turret fire".into(),
            "combat:lane D: MoverGroup::handleMateFiredWeapon (PILOT_ALARM_MATE_FIRED_WEAPON to group pilots)".into(),
            "pilot:convertcoords".into(),
        ],
    );
    let block = render_gaps(&gaps);
    assert_eq!(block, render_gaps(&gaps), "deterministic");
    assert!(block.find("mc2_05").unwrap() < block.find("mc2_22").unwrap());
    assert!(block.find("mc2_22").unwrap() < block.find("mc2_24").unwrap());
    let mut c = std::process::Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("rustfmt is installed");
    c.stdin.take().unwrap().write_all(block.as_bytes()).unwrap();
    let out = c.wait_with_output().unwrap();
    assert!(out.status.success(), "rustfmt rejected the block");
    assert_eq!(block, String::from_utf8(out.stdout).unwrap());
    // The markers splice idempotently.
    let file = format!("a\n{GAPS_BEGIN}old\n{GAPS_END}\nz\n");
    let once = splice(&file, GAPS_BEGIN, GAPS_END, &block);
    assert_eq!(once, splice(&once, GAPS_BEGIN, GAPS_END, &block));
}
