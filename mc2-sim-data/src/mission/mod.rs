// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission definitions loaded from mission `.fit` files.
//!
//! Port of the data half of `Mission::init` in `code/mission.cpp` (the block
//! reads, not the world setup) and of `code/objective.cpp` for the objective
//! blocks. The loader returns the file's content as typed values. It does not
//! create objects, resolve object type numbers, chassis names or profiles, open
//! pilot files, or preprocess brain scripts. Those are the owning lanes' jobs.
//!
//! Submodules:
//! - [`settings`]: `[MissionSettings]`, `[TheSky]`, `[DropZone0]`, `[Script]`, `[Music]`.
//! - [`environment`]: `[Cameras]`, `[Water]`, `[Terrain]`, `[Weather]`, `[Artillery]`.
//! - [`parts`]: `[Parts]` and `[PartN]`.
//! - [`warriors`]: `[Warriors]`, `[WarriorN]`, brain cells and static variables.
//! - [`map`]: nav markers, forests and commander groups.
//! - [`objective`]: per-team objectives, boolean flags and nav-marker objectives.
//! - [`condition`] and [`action`]: the objective species and their parameters.
//!
//! Warnings collect the entries the C++ silently drops or defaults; they are
//! not errors. A missing required block or key is an error.

pub mod action;
pub mod condition;
pub mod environment;
pub mod map;
pub mod objective;
pub mod parts;
mod read;
pub mod settings;
pub mod warriors;

pub use action::{Action, ActionSpecies};
pub use condition::{Area, Condition, ConditionSpecies, ObjectRef};
pub use environment::{Artillery, Cameras, Rgb, Terrain, Water, Weather};
pub use map::{CommanderGroup, Forest, NavMarker};
pub use objective::{BooleanFlag, Objective, ObjectiveModel, ObjectiveOrigin, TeamObjectives};
pub use parts::{ControlDataType, ControlType, Part};
pub use settings::{DropZone, MissionSettings, Music, Script, TheSky, DEFAULT_SKY};
pub use warriors::{CellMemory, StaticVar, StaticVarKind, Warrior, WarriorCell};

use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;

/// One loaded mission file.
#[derive(Debug, Clone, PartialEq)]
pub struct Mission {
    /// The name the file was loaded under (for example `data/missions/mc2_01.fit`).
    pub name: String,
    /// `[MissionSettings]`.
    pub settings: MissionSettings,
    /// `[TheSky]` after the loader's range rule.
    pub sky: TheSky,
    /// `[DropZone0]` position, `(-1, -1)` when absent.
    pub drop_zone: DropZone,
    /// `[Script]`.
    pub script: Script,
    /// `[Music]`, when present. The loader does not use it.
    pub music: Option<Music>,
    pub cameras: Option<Cameras>,
    pub water: Option<Water>,
    pub terrain: Option<Terrain>,
    pub weather: Option<Weather>,
    pub artillery: Option<Artillery>,
    /// `[PartN]` in order, `index` running from 1.
    pub parts: Vec<Part>,
    /// `[WarriorN]` in order, `index` running from 1.
    pub warriors: Vec<Warrior>,
    /// `[NavMarkerN]` that were read. Each team also carries them as objectives.
    pub nav_markers: Vec<NavMarker>,
    /// `[ForestN]` in order.
    pub forests: Vec<Forest>,
    /// `[CommanderNGroup:G]` blocks.
    pub commander_groups: Vec<CommanderGroup>,
    /// One entry per team id from 0 to the highest team id among the parts
    /// (mission.cpp:2050-2064). Empty when the mission has no parts.
    pub teams: Vec<TeamObjectives>,
    /// Entries the C++ drops or defaults silently, in file order.
    pub warnings: Vec<String>,
}

impl Mission {
    /// Loads a mission from an open FIT file. `name` labels the errors.
    pub fn load(fit: &mut FitIni, name: &str) -> Result<Self> {
        let mut warnings = Vec::new();
        let settings = settings::load_settings(fit, name)?;
        let sky = settings::load_sky(fit, name)?;
        let drop_zone = settings::load_drop_zone(fit)?;
        let script = settings::load_script(fit, name)?;
        let music = settings::load_music(fit)?;
        let cameras = environment::load_cameras(fit)?;
        let water = environment::load_water(fit)?;
        let terrain = environment::load_terrain(fit)?;
        let weather = environment::load_weather(fit)?;
        let artillery = environment::load_artillery(fit)?;
        if !fit.has_block("Teams") {
            warnings.push(format!("{name}: [Teams] missing"));
        }
        let parts = parts::load_parts(fit, name)?;
        let warriors = warriors::load_warriors(fit, name, &mut warnings)?;
        let nav_markers = map::load_nav_markers(fit, &mut warnings)?;

        // Teams run from 0 to the highest team id among the parts (mission.cpp:2064).
        let max_team = parts
            .iter()
            .map(|p| i32::from(p.team_id))
            .max()
            .unwrap_or(-1);
        let mut teams = Vec::new();
        if let Ok(max_team) = u32::try_from(max_team) {
            for team in 0..=max_team {
                teams.push(objective::load_team(
                    fit,
                    name,
                    team,
                    &nav_markers,
                    &mut warnings,
                )?);
            }
        }

        let commander_groups = map::load_commander_groups(fit, name)?;
        let forests = map::load_forests(fit)?;

        Ok(Self {
            name: name.to_string(),
            settings,
            sky,
            drop_zone,
            script,
            music,
            cameras,
            water,
            terrain,
            weather,
            artillery,
            parts,
            warriors,
            nav_markers,
            forests,
            commander_groups,
            teams,
            warnings,
        })
    }

    /// Loads a mission by its data-root name, through the VFS lookup order.
    pub fn load_named(vfs: &mut Vfs, name: &str) -> Result<Self> {
        let mut fit = vfs
            .read_fit(name)?
            .ok_or_else(|| SimDataError::MissingFile {
                name: name.to_string(),
            })?;
        Self::load(&mut fit, name)
    }

    /// Whether a FIT file has the blocks a mission needs. `gamesys.fit` and the
    /// purchase and bonus files do not.
    pub fn is_mission_file(fit: &FitIni) -> bool {
        fit.has_block("Parts") && fit.has_block("MissionSettings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MISSION: &str = "FITini\n\
        [MissionSettings]\nuc scenarioTuneNum = 1\nl ResourcePoints = 100\nf TimeLimit = 600\n\
        [TheSky]\nl SkyNumber = 3\n\
        [Script]\nst ScenarioScript = \"mc2_test\"\n\
        [Parts]\nul NumParts = 1\n\
        [Part1]\nul ObjectNumber = 5\nul ControlType = 1\nul ControlDataType = 1\n\
        st ObjectProfile = \"PM1\"\nul Pilot = 1\nf PositionX = 1\nf PositionY = 2\n\
        f Rotation = 0\nc TeamID = 1\nc CommanderID = 0\nl Active = 1\nl Exists = 1\nc MyIcon = 0\n\
        ul squadNum = 0\n\
        [Warriors]\nul NumWarriors = 1\n[Warrior1]\nst Profile = \"P\"\nst Brain = \"B\"\n\
        l NumCells = 0\nl NumStaticVars = 0\n\
        [Teams]\nb AlliedTeam = TRUE\n\
        [Objectives Version]\nul Version = 3\n\
        [Team0Objectives]\nul NumObjectives = 0\n\
        [Team1Objectives]\nul NumObjectives = 0\n\
        [Commander0Group:0]\nl[12] Mates = 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0\n";

    #[test]
    fn mission_loads_from_synthetic_fit() {
        let mut f = FitIni::from_bytes(format!("{MISSION}\n").into_bytes()).unwrap();
        assert!(Mission::is_mission_file(&f));
        let m = Mission::load(&mut f, "synthetic.fit").unwrap();
        assert_eq!(m.settings.resource_points, 100);
        assert_eq!(m.settings.time_limit, 600.0);
        assert_eq!(m.sky.sky_number, 3);
        assert_eq!(m.script.scenario_script, "mc2_test");
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.parts[0].profile_name, "PM1");
        assert_eq!(m.warriors.len(), 1);
        assert_eq!(m.commander_groups.len(), 1);
        // Team ids run 0..=1 because the part is on team 1.
        assert_eq!(m.teams.len(), 2);
        assert_eq!(m.teams[1].team, 1);
        assert!(m.teams[1].objectives.is_empty());
        assert!(m.forests.is_empty());
        assert!(m.warnings.is_empty(), "{:?}", m.warnings);
    }

    #[test]
    fn missing_parts_block_is_not_a_mission() {
        let f = FitIni::from_bytes(b"FITini\n[MissionSettings]\nl x = 1\n".to_vec()).unwrap();
        assert!(!Mission::is_mission_file(&f));
    }

    #[test]
    fn missing_settings_block_is_an_error() {
        let mut f = FitIni::from_bytes(b"FITini\n[Parts]\nul NumParts = 0\n".to_vec()).unwrap();
        assert!(Mission::load(&mut f, "x.fit").is_err());
    }
}
