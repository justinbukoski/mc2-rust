// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The hand-over between the campaign and the mission simulation.
//!
//! [`LoadSpec`] is what logistics gives a mission when it starts (what `LogisticsData`, `prefs`
//! and `Logistics::beginMission` supplied through globals in the original). [`Outcome`] is what
//! the mission gives back (what `LogisticsData::setMissionCompleted` and
//! `LogisticsPilot::update` read from `Team::home`, the movers and their `MechWarrior`s).
//!
//! These match "lane E - Logistics interface" in `rust/mc2-sim/DESIGN.md`. The sim's own
//! `LoadSpec`/`Outcome` were still stubs when this crate was written, so the types live here and
//! use the same field names where the sim already names them (`mission`, `load_type`,
//! `difficulty`, `unlimited_ammo`, `force`, `pilot_file`, `result`, `objectives`). The sim can
//! adopt them by depending on `mc2-campaign`, or copy the fields it needs. Every field cites the
//! C++ it comes from; this is the proposal the sim's lane E adopts for its `Outcome` (mover and
//! pilot reports, salvage flag, objective points, mission time).
//!
//! What the sim types need to gain to adopt it: on the force mech the pilot skills, specialty
//! skills and the paint colors ([`ForceMech`]); on `Outcome` the mover reports, pilot reports,
//! mech list, unlocked purchase lists and mission time ([`Outcome`]). That change belongs to
//! lane E-rest; this crate has no adapter because the sim's types are not in a merged branch yet.

use mc2_sim_data::pilot::NUM_SPECIALTY_SKILLS;

use crate::pilot::PilotReport;

/// `mis_*` mission results (`code/mission.h:42-46`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MissionResult {
    /// `mis_PLAYING`.
    Playing = 0,
    /// `mis_PLAYER_LOST_BIG`: the mission is lost; nothing changes and it can be replayed.
    PlayerLostBig = 1,
    /// `mis_PLAYER_LOST_SMALL`.
    PlayerLostSmall = 2,
    /// `mis_PLAYER_DRAW`.
    PlayerDraw = 3,
    /// `mis_PLAYER_WIN_SMALL`.
    PlayerWinSmall = 4,
    /// `mis_PLAYER_WIN_BIG`: the only result that completes a campaign mission.
    PlayerWinBig = 5,
}

impl MissionResult {
    /// The `mis_*` number.
    pub fn code(self) -> u32 {
        self as u32
    }

    /// From the `mis_*` number.
    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            0 => Self::Playing,
            1 => Self::PlayerLostBig,
            2 => Self::PlayerLostSmall,
            3 => Self::PlayerDraw,
            4 => Self::PlayerWinSmall,
            5 => Self::PlayerWinBig,
            _ => return None,
        })
    }
}

/// `objective_status_type` (`code/objective.h:77`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ObjectiveStatus {
    /// `OS_UNDETERMINED`.
    Undetermined = 0,
    /// `OS_SUCCESSFUL`.
    Successful = 1,
    /// `OS_FAILED`.
    Failed = 2,
}

/// How the mission's force is chosen (`MISSION_LOAD_SP_QUICKSTART` / `MISSION_LOAD_SP_LOGISTICS`,
/// `Logistics::beginMission`, `logistics.cpp:457`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LoadType {
    /// The mission file defines the player's units (`skipLogistics()` is true).
    SpQuickstart,
    /// The player's force from the mech bay is used.
    SpLogistics,
}

/// One component of a custom loadout, for missions that build the mech from components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadoutComponent {
    /// Master component id (`LogisticsComponent::getID`).
    pub component_id: i32,
    /// Lab grid column (`-2` jump jets, `-3` ECM and sensors).
    pub x: i64,
    /// Lab grid row.
    pub y: i64,
}

/// One mech of the player's force (what `Logistics::beginMission` puts into `MoverInitData` for
/// every force-group mech that has a pilot).
#[derive(Debug, Clone, PartialEq)]
pub struct ForceMech {
    /// Variant name (`BattleMech::variantName`); the mission reports it back in the outcome.
    pub variant_name: String,
    /// Chassis CSV path (`data.csvFileName`).
    pub chassis_file: String,
    /// Object type number (`data.objNumber` = chassis `fitID`).
    pub object_number: i64,
    /// Designer variant number inside the chassis CSV (`data.variant`); `0` for a player variant,
    /// whose loadout is then in [`components`](Self::components).
    pub variant_id: i32,
    /// `isDesignerMech()`.
    pub designer: bool,
    /// Components of a player-made variant (empty for designer variants).
    pub components: Vec<LoadoutComponent>,
    /// Pilot profile stem (`data.pilotFileName`).
    pub pilot_file: String,
    /// Pilot gunnery (`gunnerySkill`; `overrideLoadedPilot` is set).
    pub gunnery: f32,
    /// Pilot piloting (`pilotingSkill`).
    pub piloting: f32,
    /// Pilot specialty skills.
    pub specialty_skills: [bool; NUM_SPECIALTY_SKILLS],
    /// Slot in the force group (1-based).
    pub force_group: i32,
    /// `base, highlight1, highlight2` paint colors the mission paints the mech with: the
    /// player's preference colors for every deployed mech (`logistics.cpp:741-744`:
    /// `data.baseColor = prefs.baseColor`, both highlights `prefs.highlightColor`; the colors
    /// stored with the bay mech are not used at launch).
    pub colors: [u32; 3],
}

/// Which support options the mission allows (`canHave*` on `LogisticsData`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupportEnables {
    /// `canHaveSalavageCraft`.
    pub salvage_craft: bool,
    /// `canHaveRepairTruck`.
    pub repair_truck: bool,
    /// `canHaveScoutCopter`.
    pub scout_copter: bool,
    /// `canHaveArtilleryPiece`.
    pub artillery_piece: bool,
    /// `canHaveAirStrike`.
    pub air_strike: bool,
    /// `canHaveSensorStrike`.
    pub sensor_strike: bool,
    /// `canHaveMineLayer`.
    pub mine_layer: bool,
}

/// Options that come from the player's preferences, not from the campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionOptions {
    /// `GameDifficulty`.
    pub difficulty: i32,
    /// `useUnlimitedAmmo`.
    pub unlimited_ammo: bool,
    /// `prefs.baseColor` (default `0xffff7e00`, `code/prefs.cpp:73`).
    pub base_color: u32,
    /// `prefs.highlightColor` (default `0xffff7e00`, `code/prefs.cpp:74`).
    pub highlight_color: u32,
}

impl Default for MissionOptions {
    fn default() -> Self {
        MissionOptions {
            difficulty: 1,
            unlimited_ammo: true,
            base_color: 0xffff_7e00,
            highlight_color: 0xffff_7e00,
        }
    }
}

/// Everything a mission needs from logistics at load time.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadSpec {
    /// Mission file stem (`getCurrentMission()`).
    pub mission: String,
    /// Quickstart when the mission skips logistics, else logistics.
    pub load_type: LoadType,
    /// `GameDifficulty`.
    pub difficulty: i32,
    /// `useUnlimitedAmmo`.
    pub unlimited_ammo: bool,
    /// `LogisticsData::getResourcePoints()`: the in-mission support budget (not C-Bills).
    pub resource_points: i64,
    /// Drop weight limit in tons, for the HUD.
    pub drop_weight_limit: i64,
    /// The player's force in inventory order (`getForceGroup`), only mechs with a pilot, at most
    /// twelve. Empty for [`LoadType::SpQuickstart`].
    pub force: Vec<ForceMech>,
    /// The group's ABL brain script (`getCurrentABLScript`), `NONE` for no script.
    pub abl_script: String,
    /// Which support options may be bought in the mission.
    pub support: SupportEnables,
    /// The campaign's pilot roster (`LogisticsData::pilots`), for `getBestPilot` when a recovered mech needs a pilot.
    /// Empty for a quickstart: the mission then has no pilot to give and a recovery is refused (never faked).
    pub roster: Vec<RosterPilot>,
}

/// One roster pilot as `LogisticsData::getBestPilot` reads it (`logisticsdata.cpp:913`, `comparePilots` `:1000`).
#[derive(Debug, Clone, PartialEq)]
pub struct RosterPilot {
    /// Warrior profile stem.
    pub file_name: String,
    /// `isAvailable()`: listed in the purchase files and not dead.
    pub available: bool,
    /// `isUsed()`.
    pub used: bool,
    /// Some inventory mech carries this pilot (`comparePilots` ranks those last).
    pub in_inventory: bool,
    /// `getRank()`.
    pub rank: i32,
    /// `getGunnery()`.
    pub gunnery: f32,
    /// `getPiloting()`.
    pub piloting: f32,
    /// `specialtySkills[]`.
    pub specialty_skills: [bool; NUM_SPECIALTY_SKILLS],
}

/// Who a mover is, as far as the campaign cares, and how it ended
/// (the movers `LogisticsData::setMissionCompleted` walks through).
#[derive(Debug, Clone, PartialEq)]
pub struct MoverReport {
    /// `pMover->isOnGUI()`: the player's control group ever linked up with it.
    pub on_gui: bool,
    /// `getObjectTypeClass() == BATTLEMECH_TYPE`.
    pub is_battle_mech: bool,
    /// `getCommanderId() == Commander::home->getId()`.
    pub home_commander: bool,
    /// `getMoveType() == MOVETYPE_AIR` (helicopters).
    pub is_air: bool,
    /// `isDestroyed()`.
    pub destroyed: bool,
    /// `isDisabled()` (`gameobj.h:603`): true for a destroyed mover too.
    pub disabled: bool,
    /// `BattleMech::variantName`.
    pub variant_name: String,
    /// The pilot's profile stem; `None` for a mech without a warrior record in the roster.
    pub pilot_file: Option<String>,
    /// `getPaintScheme(red, green, blue)` as returned.
    pub paint_scheme: [u32; 3],
    /// What the pilot did.
    pub pilot: PilotReport,
}

/// An objective of the home team at the end of the mission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectiveReport {
    /// `Status(objectives)`.
    pub status: ObjectiveStatus,
    /// `ResourcePoints()`: the C-Bills it pays when it succeeds.
    pub resource_points: i32,
}

/// A mech the simulation had at the end of the mission, whoever's it was
/// (`ObjectManager->getMech(i)` for `i < numMechs`, as `SalvageMechScreen::init` walks them).
#[derive(Debug, Clone, PartialEq)]
pub struct MechSummary {
    /// `BattleMech::variantName`.
    pub variant_name: String,
    /// `pMech->tonnage`, shown in the salvage list.
    pub tonnage: f32,
    /// `isDestroyed()`.
    pub destroyed: bool,
    /// `isDisabled()`.
    pub disabled: bool,
    /// `moveLevel` (`mover.h:608`); 2 is a helicopter.
    pub move_level: i32,
    /// `getPaintScheme`: the mech's paint, which the salvage screen's 3D view wears.
    pub paint_scheme: [u32; 3],
    /// `(current armour, maximum armour, body location destroyed)` per armour location, as the damage colours of the wreck's icon read them.
    pub armor: [(f32, f32, bool); 11],
}

impl MechSummary {
    /// The salvage test of `SalvageMechScreen::init` (`salvagemecharea.cpp:155`):
    /// `isDisabled() && !isDestroyed() && moveLevel != 2`.
    pub fn is_salvage_candidate(&self) -> bool {
        self.disabled && !self.destroyed && self.move_level != 2
    }

    /// A disabled, intact ground mech (a salvage candidate).
    pub fn wreck(variant_name: &str, tonnage: f32) -> MechSummary {
        MechSummary {
            variant_name: variant_name.to_string(),
            tonnage,
            destroyed: false,
            disabled: true,
            move_level: 0,
            paint_scheme: [0; 3],
            armor: [(0.0, 0.0, false); 11],
        }
    }
}

/// A mech a salvage crew could pick up (the mechs that pass
/// [`MechSummary::is_salvage_candidate`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SalvageCandidate {
    /// `BattleMech::variantName`.
    pub variant_name: String,
    /// The mech's tonnage (`pMech->tonnage`), shown in the list.
    pub tonnage: f32,
    /// The mech's paint scheme.
    pub paint_scheme: [u32; 3],
    /// `(current armour, maximum armour, body location destroyed)` per armour location, as the damage colours of the wreck's icon read them.
    pub armor: [(f32, f32, bool); 11],
}

/// What a finished mission reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// `scenarioResult`.
    pub result: MissionResult,
    /// The home team's objectives (`Team::home->objectives`).
    pub objectives: Vec<ObjectiveReport>,
    /// Bonus purchase lists unlocked by objective actions during the mission
    /// (`CMakeNewTechnologyAvailable::Execute`), paths as the mission file spells them.
    pub unlocked_purchase_files: Vec<String>,
    /// The roster of the home team, in roster order (`Team::home->getMover(i)`, `i = 0..size`).
    pub movers: Vec<MoverReport>,
    /// Every mech in the mission (`ObjectManager` mech list); the salvage candidates are among
    /// them ([`Outcome::salvage_candidates`]).
    pub mechs: Vec<MechSummary>,
    /// Mission time in seconds (`Mission::actualTime`, `mission.h:190`, which
    /// `Mission::update` sets from `scenarioTime`, `mission.cpp:461`).
    pub mission_time: f32,
}

impl Outcome {
    /// A won mission with no objectives, movers or salvage.
    pub fn won() -> Outcome {
        Outcome {
            result: MissionResult::PlayerWinBig,
            objectives: Vec::new(),
            unlocked_purchase_files: Vec::new(),
            movers: Vec::new(),
            mechs: Vec::new(),
            mission_time: 0.0,
        }
    }

    /// The mechs a salvage crew could recover, in mech list order.
    pub fn salvage_candidates(&self) -> Vec<SalvageCandidate> {
        self.mechs
            .iter()
            .filter(|m| m.is_salvage_candidate())
            .map(|m| SalvageCandidate {
                variant_name: m.variant_name.clone(),
                tonnage: m.tonnage,
                paint_scheme: m.paint_scheme,
                armor: m.armor,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mission_result_codes_match_mission_h() {
        for code in 0..=5 {
            assert_eq!(MissionResult::from_code(code).unwrap().code(), code);
        }
        assert_eq!(MissionResult::from_code(6), None);
        assert_eq!(MissionResult::PlayerWinBig.code(), 5);
    }
}
