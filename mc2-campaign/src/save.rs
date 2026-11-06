//! Campaign saves.
//!
//! A new, versioned format; it does not read the original Windows saves (`SaveGameVersionNumber`
//! 10004 FIT files). Decided in the bug sweep: the old format stores a half-reconstructed state
//! (the mission list is re-derived, mech force groups are guessed from pilots, and loading
//! re-applies `setNextMission`, which shifts the C-Bills), so the Rust port saves its state
//! explicitly instead.
//!
//! # Encoding
//!
//! [RON](https://github.com/ron-rs/ron) text through `serde` (pinned: `serde =1.0.229`,
//! `ron =0.12.2`). Reasons: the data model is enums and nested records, which RON writes as
//! readable, diffable text with the type names in it (a save can be read and fixed by hand, and a
//! golden file in the repository shows what changed); unlike JSON it has real enums, tuples and
//! `Option`s without conventions; unlike a binary format it survives missing or extra fields.
//! The saves are small (tens of kilobytes), so size is not a reason to prefer binary.
//!
//! # Schema and versions
//!
//! The file is one [`SaveFile`] with `magic` and `version` first. [`load_str`] reads just that
//! header before anything else (unknown fields are ignored), so a file from a newer build is
//! refused with [`SaveError::TooNew`] instead of a parse error halfway through, and a file from an
//! older build is routed through an upgrade step. Version 1 is the first and only one; the
//! upgrade table in [`upgrade_to_current`] is where a version 2 reader would convert a v1 file.
//! Within a version, every field that could grow has `#[serde(default)]`, and unknown fields are
//! ignored, so a build can read a save from a slightly newer minor revision of the same version.
//!
//! The save holds state, not data: designer variants are referenced by chassis and number,
//! pilots by profile file, components by id. Loading needs the same [`GameData`]; names that no
//! longer exist are dropped with a warning when that loses nothing important, and refused when
//! it would (a mech whose variant is gone).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::data::GameData;
use crate::logistics::{Logistics, Mech, VariantKey};
use crate::mission_info::MissionInfo;
use crate::pilot::{Pilot, MAX_MEDAL, MAX_MISSIONS};
use crate::variant::{Location, PlacedComponent, Variant};
use mc2_sim_data::pilot::NUM_SPECIALTY_SKILLS;

/// Identifies the file type.
pub const SAVE_MAGIC: &str = "mc2-campaign-save";
/// The version this build writes.
pub const SAVE_VERSION: u32 = 1;
/// The oldest version this build can still read.
pub const MIN_SUPPORTED_VERSION: u32 = 1;

/// Why a save could not be written or read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    /// The text is not valid RON for the expected shape.
    Parse(String),
    /// The text could not be produced.
    Serialize(String),
    /// The header is not this program's save header.
    BadMagic(String),
    /// Written by a newer build than this one.
    TooNew {
        /// Version in the file.
        found: u32,
        /// Newest version this build reads.
        supported: u32,
    },
    /// Written by a build older than the oldest this one still reads.
    TooOld {
        /// Version in the file.
        found: u32,
        /// Oldest version this build reads.
        supported: u32,
    },
    /// The save does not fit the game data (unknown campaign, mismatching mission list, a mech
    /// whose variant no longer exists, and similar).
    Invalid(String),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Parse(d) => write!(f, "save file does not parse: {d}"),
            SaveError::Serialize(d) => write!(f, "save file could not be written: {d}"),
            SaveError::BadMagic(m) => write!(f, "not a campaign save (magic {m:?})"),
            SaveError::TooNew { found, supported } => write!(
                f,
                "save is version {found}; this build reads up to version {supported}"
            ),
            SaveError::TooOld { found, supported } => write!(
                f,
                "save is version {found}; this build reads from version {supported}"
            ),
            SaveError::Invalid(d) => write!(f, "save does not match the game data: {d}"),
        }
    }
}

impl std::error::Error for SaveError {}

/// The part of a save that is read first. Everything else in the file is ignored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveHeader {
    /// [`SAVE_MAGIC`].
    pub magic: String,
    /// Format version.
    pub version: u32,
}

/// A variant reference in a mech.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VariantRefSave {
    /// Index into [`SaveFile::variants`].
    List(usize),
    /// The variant being edited in the mech lab ([`SaveFile::editing`]).
    Editing,
}

/// One variant of the list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum VariantSave {
    /// A designer variant: the loadout comes from the game data.
    Designer {
        /// Chassis CSV stem.
        chassis: String,
        /// Variant number inside the chassis CSV.
        file_id: i32,
        /// Variant id (carries the chassis build-up count).
        id: u32,
        /// `availableToUser`.
        available: bool,
    },
    /// A variant the player made.
    Custom(CustomVariantSave),
}

/// A player-made variant in full.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomVariantSave {
    /// Chassis CSV stem.
    pub chassis: String,
    /// Variant id.
    pub id: u32,
    /// `fileID`.
    #[serde(default)]
    pub file_id: i32,
    /// Variant name.
    pub name: String,
    /// `availableToUser`.
    pub available: bool,
    /// `bHidden`.
    #[serde(default)]
    pub hidden: bool,
    /// Components in the order they were added.
    pub components: Vec<PlacedSave>,
}

/// A placed component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedSave {
    /// Component id (`LogisticsComponent::getID`).
    pub component: i32,
    /// Lab grid column.
    pub x: i64,
    /// Lab grid row.
    pub y: i64,
    /// Location.
    pub location: Location,
}

/// A pilot's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PilotSave {
    /// Profile stem.
    pub file_name: String,
    /// `rank`.
    pub rank: i32,
    /// `gunnery`.
    pub gunnery: f32,
    /// `piloting`.
    pub piloting: f32,
    /// `mechKills`.
    #[serde(default)]
    pub mech_kills: i32,
    /// `vehicleKills`.
    #[serde(default)]
    pub vehicle_kills: i32,
    /// `infantryKills`.
    #[serde(default)]
    pub infantry_kills: i32,
    /// `missionsCompleted`.
    #[serde(default)]
    pub missions_completed: i32,
    /// `missionsPlayed`, [`MAX_MISSIONS`] entries.
    #[serde(default)]
    pub missions_played: Vec<u8>,
    /// `bIsUsed`.
    #[serde(default)]
    pub used: bool,
    /// `bDead`.
    #[serde(default)]
    pub dead: bool,
    /// `bAvailable`.
    #[serde(default)]
    pub available: bool,
    /// `bJustDied`.
    #[serde(default)]
    pub just_died: bool,
    /// `newPiloting`.
    #[serde(default)]
    pub new_piloting: f32,
    /// `newGunnery`.
    #[serde(default)]
    pub new_gunnery: f32,
    /// `specialtySkills`.
    #[serde(default)]
    pub specialty_skills: Vec<bool>,
    /// `medals`.
    #[serde(default)]
    pub medals: Vec<bool>,
    /// `medalsLastMission`.
    #[serde(default)]
    pub medals_last_mission: Vec<bool>,
}

/// A mech in the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MechSave {
    /// Its variant.
    pub variant: VariantRefSave,
    /// `ID`.
    pub id: i32,
    /// Pilot profile stem.
    #[serde(default)]
    pub pilot: Option<String>,
    /// `forceGroup`.
    #[serde(default)]
    pub force_group: i32,
    /// `baseColor`, `highlightColor1`, `highlightColor2`.
    #[serde(default)]
    pub colors: [u32; 3],
}

/// The state of one mission in its group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionFlagsSave {
    /// `completed`.
    pub completed: bool,
    /// `hidden`.
    pub hidden: bool,
}

/// The state of one group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupSave {
    /// `videoFileName` (changes when a hidden mission is revealed).
    #[serde(default)]
    pub video_file_name: String,
    /// `bigVideoShown`.
    #[serde(default)]
    pub big_video_shown: bool,
    /// `videoShown`.
    #[serde(default)]
    pub video_shown: bool,
    /// Per mission, in order.
    pub missions: Vec<MissionFlagsSave>,
}

/// Chassis reference count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChassisRefSave {
    /// Chassis CSV stem.
    pub chassis: String,
    /// `refCount`.
    pub refs: u32,
}

/// An edit in progress in the mech lab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditSave {
    /// Index into [`SaveFile::inventory`] of the mech being edited.
    pub mech: usize,
    /// Index into [`SaveFile::variants`] of the variant the edit started from.
    pub original: Option<usize>,
    /// The variant being edited.
    pub variant: CustomVariantSave,
}

/// Text that is a literal or a string-table id (`mc2_sim_data::campaign::DisplayText`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextSave {
    /// Literal text from the mission file.
    Literal(String),
    /// A string-table id.
    Resource(u32),
}

/// The mission of a single-mission game (`LogisticsData::setSingleMission`); there is no
/// campaign script to take it from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingleSave {
    /// Mission file stem.
    pub name: String,
    /// `ResourcePoints`.
    pub resource_points: i64,
    /// The C-Bills the mission paid (`AdditionalCBills`).
    pub additional_cbills: i64,
    /// Drop weight limit, tons.
    pub drop_weight: i64,
    /// Mission name text.
    #[serde(default)]
    pub descriptive_name: Option<TextSave>,
    /// Briefing text.
    #[serde(default)]
    pub description: Option<TextSave>,
    /// `enableSalavageCraft`, `RepairTruck`, `ScoutCopter`, `ArtilleryPiece`, `AirStrike`,
    /// `SensorStrike`, `MineLayer`.
    pub enables: [bool; 7],
}

/// A whole saved campaign (format version 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SaveFile {
    /// [`SAVE_MAGIC`].
    pub magic: String,
    /// [`SAVE_VERSION`].
    pub version: u32,
    /// Free text: which build wrote the file.
    #[serde(default)]
    pub generator: String,
    /// Campaign script name; empty for a single-mission game.
    pub campaign: String,
    /// The mission of a single-mission game.
    #[serde(default)]
    pub single: Option<SingleSave>,
    /// `playerName`.
    #[serde(default)]
    pub player_name: String,
    /// `CBills`.
    pub c_bills: i64,
    /// `resourcePoints`.
    #[serde(default)]
    pub resource_points: i64,
    /// `rpJustAdded`.
    #[serde(default)]
    pub rp_just_added: bool,
    /// `bNewMechs`.
    #[serde(default)]
    pub new_mechs: bool,
    /// `bNewPilots`.
    #[serde(default)]
    pub new_pilots: bool,
    /// `bNewWeapons`.
    #[serde(default)]
    pub new_weapons: bool,
    /// `MissionResults::FirstTimeResults`.
    #[serde(default)]
    pub first_time_results: bool,
    /// `MissionBegin::FirstTimePurchase` (true for a save that predates it).
    #[serde(default = "first_time_flag")]
    pub first_time_purchase: bool,
    /// `MissionBegin::FirstTimeMechLab` (true for a save that predates it).
    #[serde(default = "first_time_flag")]
    pub first_time_mech_lab: bool,
    /// `currentStage`.
    pub current_stage: i64,
    /// `currentMission`.
    pub current_mission: i64,
    /// `currentMissionName`.
    #[serde(default)]
    pub current_mission_name: String,
    /// `lastStage`.
    #[serde(default)]
    pub last_stage: i64,
    /// `lastMission`.
    #[serde(default)]
    pub last_mission: i64,
    /// `lastMissionName`.
    #[serde(default)]
    pub last_mission_name: String,
    /// Group and mission flags.
    pub groups: Vec<GroupSave>,
    /// Bonus purchase lists unlocked, in order, duplicates kept.
    #[serde(default)]
    pub additional_purchase_files: Vec<String>,
    /// Ids of the available components.
    #[serde(default)]
    pub available_components: Vec<i32>,
    /// Chassis reference counts, which decide the id of the next variant.
    #[serde(default)]
    pub chassis_refs: Vec<ChassisRefSave>,
    /// The variant list.
    pub variants: Vec<VariantSave>,
    /// The pilots.
    pub pilots: Vec<PilotSave>,
    /// The inventory.
    pub inventory: Vec<MechSave>,
    /// The mech lab edit in progress, if any.
    #[serde(default)]
    pub editing: Option<EditSave>,
}

/// A loaded campaign and what was dropped on the way.
#[derive(Debug)]
pub struct Loaded {
    /// The campaign state.
    pub logistics: Logistics,
    /// Things in the save that the game data no longer has and that were dropped.
    pub warnings: Vec<String>,
}

/// The serde default of the tutorial's first-time flags: a fresh campaign has not left the screens.
fn first_time_flag() -> bool {
    true
}

fn invalid<T>(detail: impl Into<String>) -> Result<T, SaveError> {
    Err(SaveError::Invalid(detail.into()))
}

fn ron_options() -> ron::Options {
    ron::Options::default()
}

fn pretty() -> ron::ser::PrettyConfig {
    ron::ser::PrettyConfig::default()
        .indentor("  ".to_string())
        .struct_names(false)
        .new_line("\n".to_string())
}

/// Reads the header of a save text.
pub fn read_header(text: &str) -> Result<SaveHeader, SaveError> {
    let header: SaveHeader = ron_options()
        .from_str(text)
        .map_err(|e| SaveError::Parse(e.to_string()))?;
    if header.magic != SAVE_MAGIC {
        return Err(SaveError::BadMagic(header.magic));
    }
    Ok(header)
}

/// Parses a save text into the current [`SaveFile`], upgrading older versions.
pub fn parse_str(text: &str) -> Result<SaveFile, SaveError> {
    let header = read_header(text)?;
    if header.version > SAVE_VERSION {
        return Err(SaveError::TooNew {
            found: header.version,
            supported: SAVE_VERSION,
        });
    }
    if header.version < MIN_SUPPORTED_VERSION {
        return Err(SaveError::TooOld {
            found: header.version,
            supported: MIN_SUPPORTED_VERSION,
        });
    }
    upgrade_to_current(header.version, text)
}

/// The upgrade table: reads a file of `version` and returns it as the current [`SaveFile`].
/// Version 1 is current, so it parses directly. A version 2 build adds an arm that reads
/// `SaveFileV1` and converts it.
fn upgrade_to_current(version: u32, text: &str) -> Result<SaveFile, SaveError> {
    match version {
        1 => ron_options()
            .from_str::<SaveFile>(text)
            .map_err(|e| SaveError::Parse(e.to_string())),
        other => Err(SaveError::TooOld {
            found: other,
            supported: MIN_SUPPORTED_VERSION,
        }),
    }
}

/// Writes a save as RON text.
pub fn to_string(save: &SaveFile) -> Result<String, SaveError> {
    ron_options()
        .to_string_pretty(save, pretty())
        .map_err(|e| SaveError::Serialize(e.to_string()))
}

fn variant_chassis_stem(data: &GameData, v: &Variant) -> String {
    data.chassis[v.chassis].stem().to_string()
}

fn custom_save(data: &GameData, v: &Variant) -> CustomVariantSave {
    CustomVariantSave {
        chassis: variant_chassis_stem(data, v),
        id: v.id,
        file_id: v.file_id,
        name: v.name.clone(),
        available: v.available_to_user,
        hidden: v.hidden,
        components: v
            .components
            .iter()
            .map(|p| PlacedSave {
                component: data.catalog.get(p.component).map_or(-1, |c| c.id),
                x: p.x,
                y: p.y,
                location: p.location,
            })
            .collect(),
    }
}

impl Logistics {
    /// Captures the campaign state as a [`SaveFile`].
    pub fn to_save(&self) -> SaveFile {
        let data = &self.data;
        let m = &self.mission;

        // variant list; keys map to list positions for the mechs
        let mut position: BTreeMap<VariantKey, usize> = BTreeMap::new();
        let mut variants = Vec::new();
        for (i, key) in self.variant_list.iter().enumerate() {
            let v = &self.variants[key];
            position.insert(*key, i);
            variants.push(if v.designer {
                VariantSave::Designer {
                    chassis: variant_chassis_stem(data, v),
                    file_id: v.file_id,
                    id: v.id,
                    available: v.available_to_user,
                }
            } else {
                VariantSave::Custom(custom_save(data, v))
            });
        }

        let editing_variant = self
            .currently_modified
            .and_then(|k| self.mech(k))
            .map(|mech| mech.variant);

        let inventory: Vec<MechSave> = self
            .inventory
            .iter()
            .map(|mech: &Mech| MechSave {
                variant: if Some(mech.variant) == editing_variant
                    && Some(mech.key) == self.currently_modified
                {
                    VariantRefSave::Editing
                } else {
                    VariantRefSave::List(position[&mech.variant])
                },
                id: mech.id,
                pilot: mech.pilot.map(|p| self.pilots[p].file_name.clone()),
                force_group: mech.force_group,
                colors: mech.colors,
            })
            .collect();

        let editing = self.currently_modified.and_then(|key| {
            let mech_index = self.inventory.iter().position(|mm| mm.key == key)?;
            let variant = self.variants.get(&self.inventory[mech_index].variant)?;
            Some(EditSave {
                mech: mech_index,
                original: self.old_variant.and_then(|k| position.get(&k).copied()),
                variant: custom_save(data, variant),
            })
        });

        SaveFile {
            magic: SAVE_MAGIC.to_string(),
            version: SAVE_VERSION,
            generator: format!("mc2-campaign {}", env!("CARGO_PKG_VERSION")),
            campaign: m.campaign_name.clone(),
            single: m.single_mission.then(|| single_save(m)).flatten(),
            player_name: m.player_name.clone(),
            c_bills: m.c_bills,
            resource_points: self.resource_points,
            rp_just_added: self.rp_just_added,
            new_mechs: self.new_mechs,
            new_pilots: self.new_pilots,
            new_weapons: self.new_weapons,
            first_time_results: self.first_time_results,
            first_time_purchase: self.first_time_purchase,
            first_time_mech_lab: self.first_time_mech_lab,
            current_stage: m.current_stage,
            current_mission: m.current_mission,
            current_mission_name: m.current_mission_name.clone(),
            last_stage: m.last_stage,
            last_mission: m.last_mission,
            last_mission_name: m.last_mission_name.clone(),
            groups: m
                .groups
                .iter()
                .map(|g| GroupSave {
                    video_file_name: g.video_file_name.clone(),
                    big_video_shown: g.big_video_shown,
                    video_shown: g.video_shown,
                    missions: g
                        .missions
                        .iter()
                        .map(|x| MissionFlagsSave {
                            completed: x.completed,
                            hidden: x.hidden,
                        })
                        .collect(),
                })
                .collect(),
            additional_purchase_files: m.additional_purchase_files.clone(),
            available_components: data
                .catalog
                .iter()
                .enumerate()
                .filter(|(i, _)| self.component_available[*i])
                .map(|(_, c)| c.id)
                .collect(),
            chassis_refs: data
                .chassis
                .iter()
                .zip(&self.chassis_refs)
                .map(|(c, refs)| ChassisRefSave {
                    chassis: c.stem().to_string(),
                    refs: *refs,
                })
                .collect(),
            variants,
            pilots: self.pilots.iter().map(pilot_save).collect(),
            inventory,
            editing,
        }
    }

    /// Writes the campaign state as RON text.
    pub fn save_to_string(&self) -> Result<String, SaveError> {
        to_string(&self.to_save())
    }

    /// Restores a campaign from RON text.
    pub fn load_from_str(data: Arc<GameData>, text: &str) -> Result<Loaded, SaveError> {
        Self::from_save(data, &parse_str(text)?)
    }

    /// Restores a campaign from a [`SaveFile`], checking it against the game data.
    pub fn from_save(data: Arc<GameData>, save: &SaveFile) -> Result<Loaded, SaveError> {
        if save.magic != SAVE_MAGIC {
            return Err(SaveError::BadMagic(save.magic.clone()));
        }
        if save.version > SAVE_VERSION {
            return Err(SaveError::TooNew {
                found: save.version,
                supported: SAVE_VERSION,
            });
        }
        let mut warnings = Vec::new();

        // campaign position
        let mut mission = match &save.single {
            Some(single) => single_from_save(single),
            None => {
                let campaign = data
                    .campaigns
                    .get(&save.campaign.to_ascii_lowercase())
                    .cloned()
                    .ok_or_else(|| {
                        SaveError::Invalid(format!("unknown campaign {:?}", save.campaign))
                    })?;
                MissionInfo::new(&save.campaign.to_ascii_lowercase(), &campaign)
            }
        };
        if save.groups.len() != mission.groups.len() {
            return invalid(format!(
                "{} groups saved, the campaign has {}",
                save.groups.len(),
                mission.groups.len()
            ));
        }
        for (gi, (saved, group)) in save.groups.iter().zip(&mut mission.groups).enumerate() {
            if saved.missions.len() != group.missions.len() {
                return invalid(format!(
                    "group {gi}: {} missions saved, the campaign has {}",
                    saved.missions.len(),
                    group.missions.len()
                ));
            }
            group.video_file_name = saved.video_file_name.clone();
            group.big_video_shown = saved.big_video_shown;
            group.video_shown = saved.video_shown;
            for (flags, m) in saved.missions.iter().zip(&mut group.missions) {
                m.completed = flags.completed;
                m.hidden = flags.hidden;
            }
        }
        let group_count = mission.groups.len() as i64;
        if save.current_stage < 0 || save.current_stage > group_count {
            return invalid(format!("stage {} out of range", save.current_stage));
        }
        if save.current_stage < group_count {
            let len = mission.groups[save.current_stage as usize].missions.len() as i64;
            if save.current_mission < -1 || save.current_mission >= len.max(1) {
                return invalid(format!("mission {} out of range", save.current_mission));
            }
        }
        mission.c_bills = save.c_bills;
        mission.current_stage = save.current_stage;
        mission.current_mission = save.current_mission;
        mission.current_mission_name = save.current_mission_name.clone();
        mission.last_stage = save.last_stage;
        mission.last_mission = save.last_mission;
        mission.last_mission_name = save.last_mission_name.clone();
        mission.player_name = save.player_name.clone();
        mission.additional_purchase_files = save.additional_purchase_files.clone();

        // fresh state to fill
        let mut l = Logistics {
            variants: BTreeMap::new(),
            variant_list: Vec::new(),
            next_variant_key: 1,
            chassis_refs: vec![0; data.chassis.len()],
            inventory: Vec::new(),
            next_mech_key: 1,
            pilots: Vec::new(),
            component_available: vec![false; data.catalog.len()],
            mission,
            resource_points: save.resource_points,
            rp_just_added: save.rp_just_added,
            new_mechs: save.new_mechs,
            new_pilots: save.new_pilots,
            new_weapons: save.new_weapons,
            currently_modified: None,
            old_variant: None,
            first_time_results: save.first_time_results,
            first_time_purchase: save.first_time_purchase,
            first_time_mech_lab: save.first_time_mech_lab,
            data: Arc::clone(&data),
        };

        // components
        for id in &save.available_components {
            match data.catalog.find(*id) {
                Some(_) => {
                    for (i, c) in data.catalog.iter().enumerate() {
                        if c.id == *id {
                            l.component_available[i] = true;
                        }
                    }
                }
                None => warnings.push(format!("available component {id} no longer exists")),
            }
        }

        // chassis reference counts
        for r in &save.chassis_refs {
            match data.chassis_by_stem(&r.chassis) {
                Some(i) => l.chassis_refs[i] = r.refs,
                None => warnings.push(format!("chassis {:?} no longer exists", r.chassis)),
            }
        }

        // pilots: the roster as shipped, with the saved state laid over it
        l.pilots = data.pilots.iter().map(Pilot::from_template).collect();
        for saved in &save.pilots {
            match l.pilots.iter_mut().find(|p| p.file_name == saved.file_name) {
                Some(p) => apply_pilot_save(p, saved),
                None => warnings.push(format!("pilot {:?} no longer exists", saved.file_name)),
            }
        }

        // variants
        let mut keys: Vec<VariantKey> = Vec::new();
        for (i, v) in save.variants.iter().enumerate() {
            let variant = match v {
                VariantSave::Designer {
                    chassis,
                    file_id,
                    id,
                    available,
                } => {
                    let Some(ci) = data.chassis_by_stem(chassis) else {
                        return invalid(format!("variant {i}: chassis {chassis:?} not found"));
                    };
                    let Some((_, spec)) = data
                        .designer_variants
                        .iter()
                        .find(|(c, s)| *c == ci && s.file_id == *file_id)
                    else {
                        return invalid(format!(
                            "variant {i}: chassis {chassis:?} has no designer variant {file_id}"
                        ));
                    };
                    let ctx = crate::variant::Ctx {
                        catalog: &data.catalog,
                        chassis: &data.chassis[ci],
                    };
                    let mut v = Variant::from_spec(&ctx, spec, *id);
                    v.available_to_user = *available;
                    v
                }
                VariantSave::Custom(c) => custom_from_save(&data, c)
                    .map_err(|d| SaveError::Invalid(format!("variant {i}: {d}")))?,
            };
            let key = l.next_variant_key;
            l.next_variant_key += 1;
            l.variants.insert(key, variant);
            l.variant_list.push(key);
            keys.push(key);
        }

        // the edit in progress, if any
        let mut editing_key = None;
        if let Some(e) = &save.editing {
            let variant = custom_from_save(&data, &e.variant)
                .map_err(|d| SaveError::Invalid(format!("edited variant: {d}")))?;
            let key = l.next_variant_key;
            l.next_variant_key += 1;
            l.variants.insert(key, variant);
            editing_key = Some((key, e));
        }

        // inventory
        for (i, m) in save.inventory.iter().enumerate() {
            let variant = match &m.variant {
                VariantRefSave::List(index) => match keys.get(*index) {
                    Some(k) => *k,
                    None => return invalid(format!("mech {i}: variant {index} out of range")),
                },
                VariantRefSave::Editing => match editing_key {
                    Some((k, e)) if e.mech == i => k,
                    _ => return invalid(format!("mech {i}: refers to an edit that is not saved")),
                },
            };
            let pilot = match &m.pilot {
                Some(name) => match l.pilots.iter().position(|p| p.file_name == *name) {
                    Some(p) => Some(p),
                    None => {
                        warnings.push(format!("mech {i}: pilot {name:?} no longer exists"));
                        None
                    }
                },
                None => None,
            };
            let mech_key = l.next_mech_key;
            l.next_mech_key += 1;
            l.inventory.push(Mech {
                key: mech_key,
                id: m.id,
                variant,
                pilot,
                force_group: m.force_group,
                colors: m.colors,
            });
        }
        if let Some((_, e)) = editing_key {
            let Some(mech) = l.inventory.get(e.mech) else {
                return invalid("edit refers to a mech that is not in the inventory");
            };
            l.currently_modified = Some(mech.key);
            l.old_variant = match e.original {
                Some(i) => Some(*keys.get(i).ok_or_else(|| {
                    SaveError::Invalid(format!("edit: original variant {i} out of range"))
                })?),
                None => None,
            };
        }

        Ok(Loaded {
            logistics: l,
            warnings,
        })
    }
}

fn text_save(t: &Option<mc2_sim_data::campaign::DisplayText>) -> Option<TextSave> {
    use mc2_sim_data::campaign::DisplayText;
    t.as_ref().map(|t| match t {
        DisplayText::Literal(s) => TextSave::Literal(s.clone()),
        DisplayText::Resource(id) => TextSave::Resource(*id),
    })
}

fn text_load(t: &Option<TextSave>) -> Option<mc2_sim_data::campaign::DisplayText> {
    use mc2_sim_data::campaign::DisplayText;
    t.as_ref().map(|t| match t {
        TextSave::Literal(s) => DisplayText::Literal(s.clone()),
        TextSave::Resource(id) => DisplayText::Resource(*id),
    })
}

fn single_save(m: &MissionInfo) -> Option<SingleSave> {
    let e = m.groups.first()?.missions.first()?;
    Some(SingleSave {
        name: e.file_name.clone(),
        resource_points: e.resource_points,
        additional_cbills: e.additional_cbills,
        drop_weight: e.drop_weight,
        descriptive_name: text_save(&e.descriptive_name),
        description: text_save(&e.description),
        enables: [
            e.enable_salvage_craft,
            e.enable_repair_truck,
            e.enable_scout_copter,
            e.enable_artillery_piece,
            e.enable_air_strike,
            e.enable_sensor_strike,
            e.enable_mine_layer,
        ],
    })
}

fn single_from_save(s: &SingleSave) -> MissionInfo {
    let mut m = MissionInfo::new_single(&s.name, &Default::default(), None);
    let e = &mut m.groups[0].missions[0];
    e.resource_points = s.resource_points;
    e.additional_cbills = s.additional_cbills;
    e.drop_weight = s.drop_weight;
    e.descriptive_name = text_load(&s.descriptive_name);
    e.description = text_load(&s.description);
    [
        e.enable_salvage_craft,
        e.enable_repair_truck,
        e.enable_scout_copter,
        e.enable_artillery_piece,
        e.enable_air_strike,
        e.enable_sensor_strike,
        e.enable_mine_layer,
    ] = s.enables;
    m
}

fn custom_from_save(data: &GameData, c: &CustomVariantSave) -> Result<Variant, String> {
    let ci = data
        .chassis_by_stem(&c.chassis)
        .ok_or_else(|| format!("chassis {:?} not found", c.chassis))?;
    let mut components = Vec::new();
    for p in &c.components {
        let index = data
            .catalog
            .find(p.component)
            .ok_or_else(|| format!("component {} not found", p.component))?;
        components.push(PlacedComponent {
            component: index,
            x: p.x,
            y: p.y,
            location: p.location,
        });
    }
    Ok(Variant {
        id: c.id,
        file_id: c.file_id,
        chassis: ci,
        name: c.name.clone(),
        components,
        available_to_user: c.available,
        designer: false,
        hidden: c.hidden,
    })
}

fn pilot_save(p: &Pilot) -> PilotSave {
    PilotSave {
        file_name: p.file_name.clone(),
        rank: p.rank,
        gunnery: p.gunnery,
        piloting: p.piloting,
        mech_kills: p.mech_kills,
        vehicle_kills: p.vehicle_kills,
        infantry_kills: p.infantry_kills,
        missions_completed: p.missions_completed,
        missions_played: p.missions_played.to_vec(),
        used: p.used,
        dead: p.dead,
        available: p.available,
        just_died: p.just_died,
        new_piloting: p.new_piloting,
        new_gunnery: p.new_gunnery,
        specialty_skills: p.specialty_skills.to_vec(),
        medals: p.medals.to_vec(),
        medals_last_mission: p.medals_last_mission.to_vec(),
    }
}

/// Copies a saved array over a fixed-size one; short arrays leave the tail as it was, long ones
/// are cut.
fn copy_into<T: Copy>(dst: &mut [T], src: &[T]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = *s;
    }
}

fn apply_pilot_save(p: &mut Pilot, s: &PilotSave) {
    p.rank = s.rank;
    p.gunnery = s.gunnery;
    p.piloting = s.piloting;
    p.mech_kills = s.mech_kills;
    p.vehicle_kills = s.vehicle_kills;
    p.infantry_kills = s.infantry_kills;
    p.missions_completed = s.missions_completed;
    copy_into::<u8>(&mut p.missions_played, &s.missions_played);
    p.used = s.used;
    p.dead = s.dead;
    p.available = s.available;
    p.just_died = s.just_died;
    p.new_piloting = s.new_piloting;
    p.new_gunnery = s.new_gunnery;
    copy_into::<bool>(&mut p.specialty_skills, &s.specialty_skills);
    copy_into::<bool>(&mut p.medals, &s.medals);
    copy_into::<bool>(&mut p.medals_last_mission, &s.medals_last_mission);
}

// Keep the array sizes in the schema honest.
const _: () = {
    assert!(MAX_MISSIONS == 50);
    assert!(MAX_MEDAL == 16);
    assert!(NUM_SPECIALTY_SKILLS == 23);
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interface::Outcome;
    use crate::testdata;

    fn busy() -> Logistics {
        let mut l = Logistics::new(testdata::data()).unwrap();
        l.add_c_bills(1_000_000);
        let prime = l.variant_by_name("Alpha Prime").unwrap();
        let a = l.purchase_mech(prime).unwrap();
        let b = l.purchase_mech(prime).unwrap();
        l.set_mech_pilot(a, Some(1));
        l.add_mech_to_force_group(a, 1).unwrap();
        l.set_mech_to_modify(b).unwrap();
        let hs = l.component_index(12).unwrap();
        let (mut x, mut y) = (-1, -1);
        l.lab_add_component(hs, &mut x, &mut y).unwrap();
        l.accept_mech_modifications("Custom").unwrap();
        l.add_new_bonus_purchase_file("bonus");
        l.update_availability().unwrap();
        l
    }

    #[test]
    fn a_save_round_trips_exactly() {
        let l = busy();
        let text = l.save_to_string().unwrap();
        let loaded = Logistics::load_from_str(testdata::data(), &text).unwrap();
        assert!(loaded.warnings.is_empty());
        let m = loaded.logistics;
        assert_eq!(m.to_save(), l.to_save());
        assert_eq!(m.save_to_string().unwrap(), text);
        assert_eq!(m.c_bills(), l.c_bills());
        assert_eq!(m.inventory().len(), 2);
        assert_eq!(m.player_variant_names(), vec!["Custom".to_string()]);
        assert_eq!(m.chassis_refs, l.chassis_refs);
        // the next variant id continues the numbering
        let mut a = l.clone();
        let mut b = m;
        for x in [&mut a, &mut b] {
            let mech = x.inventory()[0].key;
            x.set_mech_to_modify(mech).unwrap();
            x.accept_mech_modifications("Next").unwrap();
        }
        assert_eq!(a.to_save(), b.to_save());
    }

    #[test]
    fn an_edit_in_progress_survives_a_save() {
        let mut l = busy();
        let mech = l.inventory()[1].key;
        l.set_mech_to_modify(mech).unwrap();
        let jj = l.component_index(13).unwrap();
        l.lab_remove_component(-2, -2).unwrap();
        let (mut x, mut y) = (-1, -1);
        l.lab_add_component(jj, &mut x, &mut y).unwrap();
        let text = l.save_to_string().unwrap();
        let mut m = Logistics::load_from_str(testdata::data(), &text)
            .unwrap()
            .logistics;
        assert_eq!(m.to_save(), l.to_save());
        assert!(m.mech_to_modify().is_some());
        // and it can still be cancelled or accepted
        let mut c = m.clone();
        c.cancel_mech_modifications().unwrap();
        l.cancel_mech_modifications().unwrap();
        assert_eq!(c.to_save(), l.to_save());
        m.accept_mech_modifications("After Load").unwrap();
        assert_eq!(m.player_variant_names().len(), 2);
    }

    #[test]
    fn the_header_is_checked_first() {
        let l = busy();
        let text = l.save_to_string().unwrap();
        assert_eq!(read_header(&text).unwrap().version, SAVE_VERSION);

        let newer = text.replacen(&format!("version: {SAVE_VERSION}"), "version: 2", 1);
        assert_eq!(
            Logistics::load_from_str(testdata::data(), &newer).unwrap_err(),
            SaveError::TooNew {
                found: 2,
                supported: SAVE_VERSION
            }
        );
        let older = text.replacen(&format!("version: {SAVE_VERSION}"), "version: 0", 1);
        assert_eq!(
            Logistics::load_from_str(testdata::data(), &older).unwrap_err(),
            SaveError::TooOld {
                found: 0,
                supported: MIN_SUPPORTED_VERSION
            }
        );
        let alien = text.replacen(SAVE_MAGIC, "something-else", 1);
        assert!(matches!(
            Logistics::load_from_str(testdata::data(), &alien),
            Err(SaveError::BadMagic(m)) if m == "something-else"
        ));
        assert!(matches!(
            Logistics::load_from_str(testdata::data(), "not a save"),
            Err(SaveError::Parse(_))
        ));
    }

    #[test]
    fn a_newer_file_with_extra_fields_and_a_newer_version_is_refused_cleanly() {
        // The header parses even when the rest has a shape this build does not know.
        let text = "(magic: \"mc2-campaign-save\", version: 7, brand_new: [1, 2, 3])";
        assert_eq!(
            parse_str(text).unwrap_err(),
            SaveError::TooNew {
                found: 7,
                supported: SAVE_VERSION
            }
        );
    }

    #[test]
    fn unknown_fields_are_ignored_and_optional_ones_default() {
        let l = busy();
        let text = l.save_to_string().unwrap();
        // add a field this build does not know to the top level
        let extended = text.replacen("campaign:", "future_field: [1, 2],\n  campaign:", 1);
        let loaded = Logistics::load_from_str(testdata::data(), &extended).unwrap();
        assert_eq!(loaded.logistics.to_save(), l.to_save());

        // drop optional fields: they take their defaults
        let mut save = l.to_save();
        save.player_name.clear();
        let mut stripped = to_string(&save).unwrap();
        for key in [
            "player_name",
            "generator",
            "new_mechs",
            "new_pilots",
            "new_weapons",
            "editing",
        ] {
            let start = stripped.find(&format!("  {key}:")).unwrap();
            let end = start + stripped[start..].find('\n').unwrap() + 1;
            stripped.replace_range(start..end, "");
        }
        let loaded = Logistics::load_from_str(testdata::data(), &stripped)
            .unwrap()
            .logistics;
        assert!(!loaded.new_mechs_available());
        assert_eq!(loaded.c_bills(), l.c_bills());
    }

    #[test]
    fn a_save_that_does_not_fit_the_data_is_refused_or_warned_about() {
        let l = busy();
        let good = l.to_save();
        let data = || testdata::data();

        let mut s = good.clone();
        s.campaign = "elsewhere".into();
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        let mut s = good.clone();
        s.groups.pop();
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        let mut s = good.clone();
        s.groups[0].missions.pop();
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        let mut s = good.clone();
        s.current_stage = 99;
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        let mut s = good.clone();
        s.inventory[0].variant = VariantRefSave::List(99);
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        let mut s = good.clone();
        s.inventory[0].variant = VariantRefSave::Editing;
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        let mut s = good.clone();
        if let Some(VariantSave::Designer { chassis, .. }) = s.variants.first_mut() {
            *chassis = "gamma".into();
        }
        assert!(matches!(
            Logistics::from_save(data(), &s),
            Err(SaveError::Invalid(_))
        ));

        // things that no longer exist but lose nothing important only warn
        let mut s = good;
        s.pilots[0].file_name = "retired".into();
        s.inventory[0].pilot = Some("also retired".into());
        s.available_components.push(999);
        let loaded = Logistics::from_save(data(), &s).unwrap();
        assert_eq!(loaded.warnings.len(), 3, "{:?}", loaded.warnings);
    }

    #[test]
    fn the_text_is_plain_ron_with_the_header_first() {
        let l = Logistics::new(testdata::data()).unwrap();
        let text = l.save_to_string().unwrap();
        assert!(
            text.starts_with("(\n  magic: \"mc2-campaign-save\",\n  version: 1,"),
            "{}",
            &text[..80]
        );
        let again = Logistics::load_from_str(testdata::data(), &text)
            .unwrap()
            .logistics;
        assert_eq!(again.to_save(), l.to_save());
    }

    #[test]
    fn progress_survives_a_completed_mission() {
        let mut l = busy();
        l.apply_outcome(&Outcome::won()).unwrap();
        let text = l.save_to_string().unwrap();
        let m = Logistics::load_from_str(testdata::data(), &text)
            .unwrap()
            .logistics;
        assert_eq!(m.current_mission(), "c");
        assert_eq!(m.last_mission(), "a");
        assert_eq!(m.mission_info().current_stage, 1);
        assert!(m.mission_info().groups[0].missions[0].completed);
    }
}
