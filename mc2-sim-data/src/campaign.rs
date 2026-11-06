// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Campaign scripts, mission settings and purchase lists.
//!
//! Ports the campaign load path:
//! - `code/logisticsmissioninfo.cpp`, `LogisticsMissionInfo::init` (lines 77-261) reads
//!   `data/campaign/<name>.fit`. Each mission's `Group<i>Mission<j>` entry is joined to its
//!   mission file, and `readMissionInfo` (lines 263-340) reads that file's `[MissionSettings]`.
//! - `code/logisticsdata.cpp`, `LogisticsData::init` (lines 92-120) and
//!   `LogisticsData::updateAvailability` (lines 1483-1676) reads the purchase lists:
//!   `[Components]`, `[Mechs]` and `[Pilots]`. `appendAvailability` (lines 1678-1740) reads
//!   the bonus lists the same way.
//! - `mclib/paths.cpp:44` sets `campaignPath` to `data/campaign/`.

use crate::bounds::capped_count;
use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use mc2_formats::FormatError;

/// Campaign script directory (`campaignPath`, `mclib/paths.cpp:44`).
pub const CAMPAIGN_DIR: &str = "data/campaign/";
/// Mission and purchase directory (`missionPath` in the shipped `system.cfg`).
pub const MISSION_DIR: &str = "data/missions/";
/// Purchase file used when a mission names none (`logisticsmissioninfo.cpp:227`).
pub const DEFAULT_PURCHASE_FILE: &str = "purchase_All";
/// Operation file used when a group names none (`logisticsmissioninfo.cpp:171`).
pub const DEFAULT_OPERATION_FILE: &str = "mcl_cm_opplayer.fit";
/// Drop weight limit used when the mission has none (`logisticsmissioninfo.cpp:314`).
pub const DEFAULT_DROP_WEIGHT: i64 = 300;
/// Most entries a purchase list block may hold (`for (i < 255)` in the C++ readers).
const MAX_LIST_ENTRIES: usize = 255;

/// Maps a `mc2_formats` lookup to `Ok(None)` when the key is absent.
fn opt<T>(r: mc2_formats::Result<T>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(FormatError::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Selects `block` if it exists. The current block is left alone when it does not.
fn enter(fit: &mut FitIni, block: &str) -> bool {
    fit.has_block(block) && fit.seek_block(block).is_ok()
}

/// `[MissionSettings]` of a mission file, as `LogisticsMissionInfo::readMissionInfo` reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionSettings {
    /// `st MissionName`. The display name when `MissionNameUseResourceString` is false.
    pub mission_name: Option<String>,
    /// `ul MissionNameResourceStringID`. The string-table id when the resource flag is set.
    pub mission_name_resource_id: Option<u32>,
    /// `b MissionNameUseResourceString` (logisticsmissioninfo.cpp:273). When true the name
    /// is the string-table resource; when false it is the literal `MissionName`. Default false.
    pub mission_name_use_resource_string: bool,
    /// `st Blurb2`, the mission description.
    pub blurb2: Option<String>,
    /// `ul Blurb2ResourceStringID`.
    pub blurb2_resource_id: Option<u32>,
    /// `b Blurb2UseResourceString` (logisticsmissioninfo.cpp:296). When true and the id is
    /// read, the description is the string-table resource. Default false.
    pub blurb2_use_resource_string: bool,
    /// `l ResourcePoints`. The C++ reads this with `readIdLong`, which matches only the `l`
    /// tag. Missions written as `ul ResourcePoints` give `None`, and the C++ leaves the field
    /// unset there.
    pub resource_points: Option<i64>,
    /// `l AdditionalCBills`. Read with `readIdLong` (the `l` tag only).
    pub additional_cbills: Option<i64>,
    /// `f DropWeightLimit`, truncated to a whole number. Default [`DEFAULT_DROP_WEIGHT`].
    pub drop_weight_limit: i64,
    /// `AirStrikesEnabledDefault`, default true.
    pub enable_air_strike: bool,
    /// `MineLayersEnabledDefault`, default true.
    pub enable_mine_layer: bool,
    /// `ScoutCoptersEnabledDefault`, default true.
    pub enable_scout_copter: bool,
    /// `SalvageCraftEnabledDefault`, default true.
    pub enable_salvage_craft: bool,
    /// `SensorProbesEnabledDefault`, default true.
    pub enable_sensor_strike: bool,
    /// `RepairVehicleEnabledDefault`, default true.
    pub enable_repair_truck: bool,
    /// `ArtilleryPieceEnabledDefault`, default false.
    pub enable_artillery_piece: bool,
}

/// Mission text the campaign screens show: a literal from the file, or a string-table
/// resource that the UI resolves (`cLoadString`). The string table is not in the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayText {
    /// The literal text from the mission file.
    Literal(String),
    /// A string-table id.
    Resource(u32),
}

impl MissionSettings {
    /// The mission name the C++ displays (logisticsmissioninfo.cpp:272-288). With
    /// `MissionNameUseResourceString` set, the string-table id is used (the C++ asserts when
    /// the id is missing, and the name stays empty, so this returns `None`). Otherwise the
    /// literal `MissionName` is used.
    pub fn display_name(&self) -> Option<DisplayText> {
        if self.mission_name_use_resource_string {
            self.mission_name_resource_id.map(DisplayText::Resource)
        } else {
            self.mission_name.clone().map(DisplayText::Literal)
        }
    }

    /// The mission description the C++ displays (logisticsmissioninfo.cpp:292-306). `Blurb2`
    /// is the literal; `Blurb2UseResourceString` with a readable `Blurb2ResourceStringID`
    /// replaces it with the string-table resource.
    pub fn display_blurb(&self) -> Option<DisplayText> {
        if self.blurb2_use_resource_string {
            if let Some(id) = self.blurb2_resource_id {
                return Some(DisplayText::Resource(id));
            }
        }
        self.blurb2.clone().map(DisplayText::Literal)
    }
}

impl Default for MissionSettings {
    fn default() -> Self {
        MissionSettings {
            mission_name: None,
            mission_name_resource_id: None,
            mission_name_use_resource_string: false,
            blurb2: None,
            blurb2_resource_id: None,
            blurb2_use_resource_string: false,
            resource_points: None,
            additional_cbills: None,
            drop_weight_limit: DEFAULT_DROP_WEIGHT,
            enable_air_strike: true,
            enable_mine_layer: true,
            enable_scout_copter: true,
            enable_salvage_craft: true,
            enable_sensor_strike: true,
            enable_repair_truck: true,
            enable_artillery_piece: false,
        }
    }
}

impl MissionSettings {
    /// Reads `[MissionSettings]` from a mission file. A missing block gives the defaults.
    pub fn from_fit(fit: &mut FitIni) -> Result<Self> {
        let mut s = MissionSettings::default();
        if !enter(fit, "MissionSettings") {
            return Ok(s);
        }
        s.mission_name_use_resource_string =
            opt(fit.read_bool("MissionNameUseResourceString"))?.unwrap_or(false);
        s.mission_name = opt(fit.read_string("MissionName", 255))?;
        s.mission_name_resource_id = opt(fit.read_u32("MissionNameResourceStringID"))?;
        s.blurb2 = opt(fit.read_string("Blurb2", 4095))?;
        s.blurb2_use_resource_string =
            opt(fit.read_bool("Blurb2UseResourceString"))?.unwrap_or(false);
        s.blurb2_resource_id = opt(fit.read_u32("Blurb2ResourceStringID"))?;
        s.resource_points = opt(fit.read_i64("ResourcePoints"))?;
        s.additional_cbills = opt(fit.read_i64("AdditionalCBills"))?;
        s.drop_weight_limit = match opt(fit.read_f32("DropWeightLimit"))? {
            Some(f) => f as i64,
            None => DEFAULT_DROP_WEIGHT,
        };
        s.enable_air_strike = opt(fit.read_bool("AirStrikesEnabledDefault"))?.unwrap_or(true);
        s.enable_mine_layer = opt(fit.read_bool("MineLayersEnabledDefault"))?.unwrap_or(true);
        s.enable_scout_copter = opt(fit.read_bool("ScoutCoptersEnabledDefault"))?.unwrap_or(true);
        s.enable_salvage_craft = opt(fit.read_bool("SalvageCraftEnabledDefault"))?.unwrap_or(true);
        s.enable_sensor_strike = opt(fit.read_bool("SensorProbesEnabledDefault"))?.unwrap_or(true);
        s.enable_repair_truck = opt(fit.read_bool("RepairVehicleEnabledDefault"))?.unwrap_or(true);
        s.enable_artillery_piece =
            opt(fit.read_bool("ArtilleryPieceEnabledDefault"))?.unwrap_or(false);
        Ok(s)
    }
}

/// One mission entry of a campaign group (`Group<i>Mission<j>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignMission {
    /// Position within its group (`j`).
    pub index: usize,
    /// Mission file stem (`FileName`), without `.fit`. The file lives under `MISSION_DIR`.
    pub file_name: String,
    /// `b Mandatory`, default false.
    pub mandatory: bool,
    /// `b CompletePrevious`, default false.
    pub complete_previous: bool,
    /// `b Hidden`, default false.
    pub hidden: bool,
    /// `st VideoOverride`, replaces the group video for this mission.
    pub video_override: Option<String>,
    /// Purchase file stem (`PurchaseFile`). Defaults to [`DEFAULT_PURCHASE_FILE`].
    /// Empty when the mission file was missing (the C++ skips the rest of the entry then).
    pub purchase_file: String,
    /// `b PlayLogistics`, default true.
    pub play_logistics: bool,
    /// `b PlaySalvage`, default true.
    pub play_salvage: bool,
    /// `b PlayPilotPromotion`, default true.
    pub play_pilot_promotion: bool,
    /// `b PlayPurchasing`, default true.
    pub play_purchasing: bool,
    /// `b PlaySelection`, default false.
    pub play_mission_selection: bool,
    /// Settings from the mission file. `None` when that file is missing.
    pub settings: Option<MissionSettings>,
}

/// One campaign group (`Group<i>`): a set of missions, `NumberToComplete` of which are needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignGroup {
    /// Group index (`i`).
    pub index: usize,
    /// `l NumberToComplete`, default -1 (`logisticsmissioninfo.cpp:130`).
    pub number_to_complete: i64,
    /// `l Tune`, the music id for the logistics screens. Default 0.
    pub logistics_tune_id: i64,
    /// `st Video`, the group's operation video.
    pub video: Option<String>,
    /// `st PreVideo`, the video shown before the group.
    pub pre_video: Option<String>,
    /// `st OperationFile`. Defaults to [`DEFAULT_OPERATION_FILE`] when absent or empty.
    pub operation_file: String,
    /// `st ABLScript`, the ABL brain name. `"NONE"` when absent (`logisticsmissioninfo.cpp:145`).
    pub abl_script: String,
    /// `l MissionCount`, as written. The entries that exist are in `missions`.
    pub mission_count: i64,
    /// Mission entries in file order. Entries whose block or `FileName` is missing are dropped.
    pub missions: Vec<CampaignMission>,
}

/// A campaign script (`campaign.fit`, `tutorial.fit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Campaign {
    /// File stem, for example `campaign` or `tutorial`.
    pub file_name: String,
    /// `l NameID`, a string-table id for the display name. The string table is not in the data
    /// files, so the name is left unresolved.
    pub name_id: Option<i64>,
    /// `st CampaignName`. Read only when `NameID` is absent (`logisticsmissioninfo.cpp:91-98`).
    pub campaign_name: Option<String>,
    /// `l GroupCount`, as written.
    pub group_count: i64,
    /// `l CBills`, default 0.
    pub cbills: i64,
    /// `st FinalVideo`.
    pub final_video: Option<String>,
    /// Groups that are present, in file order. The index is in each group.
    pub groups: Vec<CampaignGroup>,
    /// Indices of groups whose `Group<i>` block is missing. The C++ asserts on these and goes on.
    pub missing_groups: Vec<usize>,
    /// Mission file stems that are not in the VFS.
    pub missing_mission_files: Vec<String>,
    /// Bonus purchase files added at run time (`addBonusPurchaseFile`, `logisticsmissioninfo.cpp:1085`).
    pub bonus_purchase_files: Vec<String>,
}

impl Campaign {
    /// Parses a campaign script. `mission_fit` opens a mission file by stem and returns
    /// `Ok(None)` when it is absent.
    ///
    /// Missing mission files follow the C++: the entry keeps its name, the rest of the entry
    /// is skipped, and the name is recorded in `missing_mission_files`.
    pub fn from_fit(
        file_name: &str,
        fit: &mut FitIni,
        mut mission_fit: impl FnMut(&str) -> Result<Option<FitIni>>,
    ) -> Result<Self> {
        if !enter(fit, "Campaign") {
            return Err(SimDataError::schema(file_name, "missing [Campaign] block"));
        }
        let name_id = opt(fit.read_i64("NameID"))?;
        let campaign_name = match name_id {
            Some(_) => None,
            None => opt(fit.read_string("CampaignName", 255))?,
        };
        let group_count = opt(fit.read_i64("GroupCount"))?
            .ok_or_else(|| SimDataError::schema(file_name, "no GroupCount in [Campaign]"))?;
        let cbills = opt(fit.read_i64("CBills"))?.unwrap_or(0);
        let final_video = opt(fit.read_string("FinalVideo", 255))?;

        let mut campaign = Campaign {
            file_name: file_name.to_string(),
            name_id,
            campaign_name,
            group_count,
            cbills,
            final_video,
            groups: Vec::new(),
            missing_groups: Vec::new(),
            missing_mission_files: Vec::new(),
            bonus_purchase_files: Vec::new(),
        };

        let group_count = capped_count(file_name, "GroupCount", group_count)?;
        for i in 0..group_count {
            if !enter(fit, &format!("Group{i}")) {
                campaign.missing_groups.push(i);
                continue;
            }
            let number_to_complete = opt(fit.read_i64("NumberToComplete"))?.unwrap_or(-1);
            let logistics_tune_id = opt(fit.read_i64("Tune"))?.unwrap_or(0);
            let video = opt(fit.read_string("Video", 255))?;
            let pre_video = opt(fit.read_string("PreVideo", 255))?;
            let operation_file = match opt(fit.read_string("OperationFile", 255))? {
                Some(s) if !s.is_empty() => s,
                _ => DEFAULT_OPERATION_FILE.to_string(),
            };
            let abl_script =
                opt(fit.read_string("ABLScript", 255))?.unwrap_or_else(|| "NONE".to_string());
            let mission_count = opt(fit.read_i64("MissionCount"))?.unwrap_or(0);
            let mission_loops = capped_count(file_name, "MissionCount", mission_count)?;

            let mut missions = Vec::new();
            for j in 0..mission_loops {
                let block = format!("Group{i}Mission{j}");
                if !enter(fit, &block) {
                    continue;
                }
                let Some(mission_name) = opt(fit.read_string("FileName", 1023))? else {
                    continue;
                };
                let mut entry = CampaignMission {
                    index: j,
                    file_name: mission_name.clone(),
                    mandatory: false,
                    complete_previous: false,
                    hidden: false,
                    video_override: None,
                    purchase_file: String::new(),
                    play_logistics: true,
                    play_salvage: true,
                    play_pilot_promotion: true,
                    play_purchasing: true,
                    play_mission_selection: false,
                    settings: None,
                };
                let Some(mut mfit) = mission_fit(&mission_name)? else {
                    campaign.missing_mission_files.push(mission_name);
                    missions.push(entry);
                    continue;
                };
                entry.settings = Some(MissionSettings::from_fit(&mut mfit)?);

                entry.video_override = opt(fit.read_string("VideoOverride", 255))?;
                entry.mandatory = opt(fit.read_bool("Mandatory"))?.unwrap_or(false);
                entry.complete_previous = opt(fit.read_bool("CompletePrevious"))?.unwrap_or(false);
                entry.purchase_file = match opt(fit.read_string("PurchaseFile", 1023))? {
                    Some(s) if !s.is_empty() => s,
                    _ => DEFAULT_PURCHASE_FILE.to_string(),
                };
                entry.hidden = opt(fit.read_bool("Hidden"))?.unwrap_or(false);
                entry.play_logistics = opt(fit.read_bool("PlayLogistics"))?.unwrap_or(true);
                entry.play_salvage = opt(fit.read_bool("PlaySalvage"))?.unwrap_or(true);
                entry.play_pilot_promotion =
                    opt(fit.read_bool("PlayPilotPromotion"))?.unwrap_or(true);
                entry.play_purchasing = opt(fit.read_bool("PlayPurchasing"))?.unwrap_or(true);
                entry.play_mission_selection =
                    opt(fit.read_bool("PlaySelection"))?.unwrap_or(false);
                missions.push(entry);
            }

            campaign.groups.push(CampaignGroup {
                index: i,
                number_to_complete,
                logistics_tune_id,
                video,
                pre_video,
                operation_file,
                abl_script,
                mission_count,
                missions,
            });
        }
        Ok(campaign)
    }

    /// Loads `CAMPAIGN_DIR/<name>.fit` and the mission files it names, through the VFS.
    pub fn load(vfs: &mut Vfs, name: &str) -> Result<Self> {
        let path = format!("{CAMPAIGN_DIR}{name}.fit");
        let mut fit = vfs
            .read_fit(&path)?
            .ok_or_else(|| SimDataError::MissingFile { name: path.clone() })?;
        Self::from_fit(name, &mut fit, |mission| {
            let path = format!("{MISSION_DIR}{mission}.fit");
            Ok(vfs.read_fit(&path)?)
        })
    }

    /// Every mission in campaign order, with its group index.
    pub fn missions(&self) -> impl Iterator<Item = (usize, &CampaignMission)> + '_ {
        self.groups
            .iter()
            .flat_map(|g| g.missions.iter().map(move |m| (g.index, m)))
    }

    /// Number of mission entries across all groups.
    pub fn mission_total(&self) -> usize {
        self.groups.iter().map(|g| g.missions.len()).sum()
    }

    /// Adds a bonus purchase file (`LogisticsMissionInfo::addBonusPurchaseFile`).
    pub fn add_bonus_purchase_file(&mut self, file_name: &str) {
        self.bonus_purchase_files.push(file_name.to_string());
    }
}

/// A purchase list (`purchase*.fit`, `purbonus*.fit`).
///
/// `[Components]` holds the component ids that may be bought, and `[Mechs]` and `[Pilots]` hold
/// chassis and warrior names. Components are only read with `AllComponents` off.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PurchaseList {
    /// File stem, as the campaign names it.
    pub file_name: String,
    /// `b AllComponents`, default false. When true, every component is available.
    pub all_components: bool,
    /// `l Component<i>` values, read until the first missing index.
    pub components: Vec<i64>,
    /// `st Mech<i>` chassis names, read until the first missing index.
    pub mechs: Vec<String>,
    /// `st Pilot<i>` warrior names, read until the first missing index. Matched ignoring ASCII case.
    pub pilots: Vec<String>,
}

impl PurchaseList {
    /// Parses a purchase list from an open FIT file.
    pub fn from_fit(file_name: &str, fit: &mut FitIni) -> Result<Self> {
        let mut list = PurchaseList {
            file_name: file_name.to_string(),
            ..PurchaseList::default()
        };
        if enter(fit, "Components") {
            list.all_components = opt(fit.read_bool("AllComponents"))?.unwrap_or(false);
            if !list.all_components {
                for i in 0..MAX_LIST_ENTRIES {
                    match opt(fit.read_i64(&format!("Component{i}")))? {
                        Some(v) => list.components.push(v),
                        None => break,
                    }
                }
            }
        }
        if enter(fit, "Mechs") {
            for i in 0..MAX_LIST_ENTRIES {
                match opt(fit.read_string(&format!("Mech{i}"), 254))? {
                    Some(s) => list.mechs.push(s),
                    None => break,
                }
            }
        }
        if enter(fit, "Pilots") {
            for i in 0..MAX_LIST_ENTRIES {
                match opt(fit.read_string(&format!("Pilot{i}"), 254))? {
                    Some(s) => list.pilots.push(s),
                    None => break,
                }
            }
        }
        Ok(list)
    }

    /// Loads `MISSION_DIR/<file_name>.fit` through the VFS.
    pub fn load(vfs: &mut Vfs, file_name: &str) -> Result<Self> {
        let path = format!("{MISSION_DIR}{file_name}.fit");
        let mut fit = vfs
            .read_fit(&path)?
            .ok_or_else(|| SimDataError::MissingFile { name: path.clone() })?;
        Self::from_fit(file_name, &mut fit)
    }

    /// Whether component `id` may be bought from this list.
    pub fn lists_component(&self, id: i64) -> bool {
        self.all_components || self.components.contains(&id)
    }

    /// Whether the warrior file stem `pilot` is listed, ignoring ASCII case.
    pub fn lists_pilot(&self, pilot: &str) -> bool {
        self.pilots.iter().any(|p| p.eq_ignore_ascii_case(pilot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(text.as_bytes().to_vec()).expect("FIT header")
    }

    const CAMPAIGN: &str = "FITini\n\n[Campaign]\nl NameID = 60022\nl GroupCount = 3\nl CBills = 500\n\
st FinalVideo = \"\"\n\
[Group0]\nl NumberToComplete = 1\nl MissionCount = 2\nst OperationFile = \"MCL_op\"\nst Video = \"NODE_1\"\nl Tune = 22\n\
[Group0Mission0]\nst FileName = \"mc2_01\"\nb Mandatory = TRUE\nst PurchaseFile = \"Purchase01\"\nb PlaySelection = 1\n\
[Group0Mission1]\nst FileName = \"missing_one\"\nb Mandatory = FALSE\nst PurchaseFile = \"Purchase02\"\n\
[Group2]\nl NumberToComplete = 2\nl MissionCount = 1\nst Video = \"CHOICE\"\nst ABLScript = \"\"\n\
[Group2Mission0]\nst FileName = \"mc2_09\"\nst VideoOverride = \"NODE_09.bik\"\nb Hidden = TRUE\n\
FitEnd";

    const MISSION: &str = "FITini\n[MissionSettings]\nst MissionName = \"mc2_01\"\nb MissionNameUseResourceString = TRUE\n\
ul MissionNameResourceStringID = 57001\nf DropWeightLimit = 190.5\nl ResourcePoints = 22000\nl AdditionalCBills = 1000\n\
b RepairVehicleEnabledDefault = FALSE\nFitEnd";

    #[test]
    fn parses_groups_missions_and_defaults() {
        let mut f = fit(CAMPAIGN);
        let c = Campaign::from_fit("campaign", &mut f, |name| {
            if name == "missing_one" {
                Ok(None)
            } else {
                Ok(Some(fit(MISSION)))
            }
        })
        .unwrap();
        assert_eq!(c.name_id, Some(60022));
        assert_eq!(c.campaign_name, None);
        assert_eq!(c.cbills, 500);
        assert_eq!(c.final_video.as_deref(), Some(""));
        assert_eq!(c.missing_groups, vec![1]);
        assert_eq!(c.groups.len(), 2);
        assert_eq!(c.groups[0].number_to_complete, 1);
        assert_eq!(c.groups[0].operation_file, "MCL_op");
        assert_eq!(c.groups[0].abl_script, "NONE");
        assert_eq!(c.groups[1].index, 2);
        assert_eq!(c.groups[1].abl_script, "");
        assert_eq!(c.groups[1].operation_file, DEFAULT_OPERATION_FILE);

        let m0 = &c.groups[0].missions[0];
        assert_eq!(m0.file_name, "mc2_01");
        assert!(m0.mandatory);
        assert!(m0.play_logistics);
        assert!(m0.play_mission_selection);
        assert_eq!(m0.purchase_file, "Purchase01");
        let s = m0.settings.as_ref().unwrap();
        assert_eq!(s.mission_name.as_deref(), Some("mc2_01"));
        assert_eq!(s.mission_name_resource_id, Some(57001));
        assert_eq!(s.resource_points, Some(22000));
        assert_eq!(s.additional_cbills, Some(1000));
        assert_eq!(s.drop_weight_limit, 190);
        assert!(!s.enable_repair_truck);
        assert!(s.enable_air_strike);

        // The missing mission keeps its name, drops the campaign keys, and is recorded.
        let m1 = &c.groups[0].missions[1];
        assert_eq!(m1.file_name, "missing_one");
        assert!(m1.settings.is_none());
        assert_eq!(m1.purchase_file, "");
        assert_eq!(c.missing_mission_files, vec!["missing_one".to_string()]);

        let m2 = &c.groups[1].missions[0];
        assert_eq!(m2.video_override.as_deref(), Some("NODE_09.bik"));
        assert!(m2.hidden);
        assert_eq!(m2.purchase_file, DEFAULT_PURCHASE_FILE);
        assert!(m2.settings.is_some());
        assert_eq!(c.mission_total(), 3);
    }

    #[test]
    fn missing_campaign_block_or_group_count_is_an_error() {
        let mut f = fit("FITini\n[Group0]\nl MissionCount = 0\nFitEnd\n");
        assert!(Campaign::from_fit("x", &mut f, |_| Ok(None)).is_err());
        let mut g = fit("FITini\n[Campaign]\nl CBills = 1\nFitEnd\n");
        assert!(Campaign::from_fit("x", &mut g, |_| Ok(None)).is_err());
    }

    #[test]
    fn ul_resource_points_are_not_read_by_the_l_tag_reader() {
        let text = MISSION.replace("l ResourcePoints", "ul ResourcePoints");
        let mut f = fit(&text);
        let s = MissionSettings::from_fit(&mut f).unwrap();
        assert_eq!(s.resource_points, None);
    }

    #[test]
    fn purchase_list_reads_components_mechs_and_pilots() {
        let text = "FITini\n[Components]\nb AllComponents = False\nl Component0 = 7\t\t//Heat Sink\n\
l Component1 = 10\t\t//Jump Jets\n[Mechs]\nst Mech0 = \"werewolf\"\n[Pilots]\nst Pilot0 = \"PMWChopper\"\n\
st Pilot1 = \"PMWCreep\"\nFITend";
        let mut f = fit(text);
        let p = PurchaseList::from_fit("purchase01", &mut f).unwrap();
        assert!(!p.all_components);
        assert_eq!(p.components, vec![7, 10]);
        assert!(p.lists_component(10));
        assert!(!p.lists_component(11));
        assert_eq!(p.mechs, vec!["werewolf".to_string()]);
        assert!(p.lists_pilot("pmwchopper"));
        assert!(!p.lists_pilot("pmwflash"));
    }

    #[test]
    fn all_components_flag_skips_the_list() {
        let mut f = fit("FITini\n[Components]\nb AllComponents = TRUE\nl Component0 = 7\nFITend");
        let p = PurchaseList::from_fit("all", &mut f).unwrap();
        assert!(p.all_components);
        assert!(p.components.is_empty());
        assert!(p.lists_component(999));
    }

    #[test]
    fn bonus_purchase_file_is_appended_at_runtime() {
        let mut f = fit(CAMPAIGN);
        let mut c = Campaign::from_fit("campaign", &mut f, |_| Ok(None)).unwrap();
        c.add_bonus_purchase_file("purbonus08");
        assert_eq!(c.bonus_purchase_files, vec!["purbonus08".to_string()]);
    }
}
