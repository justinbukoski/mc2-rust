// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Durable campaign slots around `mc2-campaign`'s versioned save format.
//!
//! `CampaignSaveService` is the UI host seam: construct it with a save directory, then call
//! `save_slot`, `save_autosave_if_eligible`, `load`, and `list`. It has no UI or global state.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mc2_campaign::results::AutosaveRequest;
use mc2_campaign::{GameData, Logistics, SaveError};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// One numbered or player-named save file, or the campaign autosave.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SaveSlot {
    /// Player-managed slot; numbering begins at 1.
    Numbered(u32),
    /// Results-screen campaign autosave.
    Autosave,
    /// Player-named campaign save file stem.
    Named(String),
}

/// Metadata used by the campaign load screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveMetadata {
    /// Stable slot identity and ordering.
    pub slot: SaveSlot,
    /// Campaign name shown by the original load dialog (`NameID` or `CampaignName`).
    pub campaign_name: String,
    /// String-table ID displayed by C++ when the campaign has a `NameID` instead of a literal
    /// `CampaignName`; the UI host can resolve it through its localization service.
    pub campaign_name_id: Option<i64>,
    /// Player name stored in the campaign.
    pub player_name: String,
    /// File stem of the mission currently selected in the campaign.
    pub current_mission_file: Option<String>,
    /// Literal or string-table ID shown for the currently selected mission.
    pub current_mission_name: Option<String>,
    /// String-table ID for `current_mission_name`, when the mission uses a resource string.
    pub current_mission_name_id: Option<u32>,
    /// Current campaign C-Bills balance.
    pub c_bills: i64,
    /// Last successful write time from the filesystem.
    pub modified: std::time::SystemTime,
}

/// A file recognized as a save slot but unusable by the current build or installed game data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSaveMetadata {
    /// Stable slot identity shown to the load screen.
    pub slot: SaveSlot,
    /// Why this row could not be loaded or decoded.
    pub error: String,
}

/// Load-screen rows. One damaged file does not hide usable saves in other slots.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveListing {
    /// Usable saves, sorted by numbered slot with autosave last.
    pub saves: Vec<SaveMetadata>,
    /// Recognized slots whose contents could not be loaded.
    pub invalid: Vec<InvalidSaveMetadata>,
}

/// Errors from resolving, reading, writing, or decoding a campaign save.
#[derive(Debug)]
pub enum CampaignSaveError {
    /// Filesystem operation failed.
    Io(io::Error),
    /// The campaign save was malformed or did not match the installed campaign data.
    Save(SaveError),
    /// The save referenced installed content that is no longer available.
    DegradedLoad(Vec<String>),
    /// Numbered slots start at 1.
    InvalidSlot,
}

impl fmt::Display for CampaignSaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "campaign save I/O failed: {error}"),
            Self::Save(error) => write!(f, "campaign save could not be loaded: {error}"),
            Self::DegradedLoad(warnings) => write!(
                f,
                "campaign save is missing installed content: {}",
                warnings.join("; ")
            ),
            Self::InvalidSlot => write!(f, "campaign save slots start at 1"),
        }
    }
}

impl std::error::Error for CampaignSaveError {}

impl From<io::Error> for CampaignSaveError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<SaveError> for CampaignSaveError {
    fn from(value: SaveError) -> Self {
        Self::Save(value)
    }
}

/// File-backed campaign save service. Each save is committed by renaming a fully synced sibling
/// temporary file over its destination, so an interrupted write leaves the old slot readable.
#[derive(Debug, Clone)]
pub struct CampaignSaveService {
    directory: PathBuf,
}

impl CampaignSaveService {
    /// Use the configured `MC2_SAVE_DIR`, or the per-user application data directory.
    pub fn new_default() -> Result<Self, CampaignSaveError> {
        let directory = if let Some(path) = std::env::var_os("MC2_SAVE_DIR") {
            PathBuf::from(path)
        } else {
            default_save_directory().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "no per-user application data directory",
                )
            })?
        };
        Ok(Self::new(directory))
    }

    /// Use an explicit directory (tests and callers may choose `$TMPDIR`).
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// Directory used for numbered slots and autosave.
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// Persist a numbered campaign slot. Slot numbering begins at 1.
    pub fn save_slot(&self, slot: u32, campaign: &Logistics) -> Result<(), CampaignSaveError> {
        if slot == 0 {
            return Err(CampaignSaveError::InvalidSlot);
        }
        self.write_campaign(SaveSlot::Numbered(slot), campaign)
    }

    /// Persist a player-named campaign save. Names are file stems, not paths.
    pub fn save_named(&self, name: &str, campaign: &Logistics) -> Result<(), CampaignSaveError> {
        let slot = SaveSlot::Named(name.to_string());
        self.path(slot.clone())?;
        self.write_campaign(slot, campaign)
    }

    /// Persist the results-screen autosave.
    fn save_autosave(&self, campaign: &Logistics) -> Result<(), CampaignSaveError> {
        self.write_campaign(SaveSlot::Autosave, campaign)
    }

    /// Persist the campaign after its final mission, matching `logistics.cpp`'s final-win write.
    pub fn save_campaign_completion(&self, campaign: &Logistics) -> Result<(), CampaignSaveError> {
        self.save_autosave(campaign)
    }

    /// Apply the original `MissionResults::end` eligibility rule after results are complete.
    /// `request` must come from `MissionResults::autosave`, which is `None` until salvage and
    /// pilot review have finished. `multiplayer` is supplied by the host because network mode is
    /// not campaign state.
    pub fn save_autosave_if_eligible(
        &self,
        campaign: &Logistics,
        request: Option<&AutosaveRequest>,
        multiplayer: bool,
    ) -> Result<bool, CampaignSaveError> {
        let Some(request) = request else {
            return Ok(false);
        };
        if multiplayer || request.mission() != campaign.last_mission() {
            return Ok(false);
        }
        self.save_autosave(campaign)?;
        Ok(true)
    }

    /// `DeleteFile` of the save dialog's DELETE (`logisticsdialog.cpp:765-783`): removes the file
    /// of `slot`. A file that is already gone is not an error (the C++ ignores the result).
    pub fn delete(&self, slot: SaveSlot) -> Result<(), CampaignSaveError> {
        match fs::remove_file(self.path(slot)?) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }

    /// Load a slot into a fresh logistics state using the supplied game data.
    pub fn load(
        &self,
        slot: SaveSlot,
        data: Arc<GameData>,
    ) -> Result<Logistics, CampaignSaveError> {
        let text = fs::read_to_string(self.path(slot)?)?;
        load_complete_campaign(data, &text)
    }

    /// List usable and invalid save rows in numbered-slot order, with autosave last. The visible
    /// campaign name follows its `NameID` or `CampaignName` fields; the filename is represented
    /// by `slot`. Canonical numbered files are recognized as numbered slots; other valid file
    /// stems, including `slot-notes` and `slot-1`, remain player-named saves.
    pub fn list(&self, data: Arc<GameData>) -> Result<SaveListing, CampaignSaveError> {
        let mut paths: Vec<(SaveSlot, PathBuf, std::time::SystemTime)> = Vec::new();
        if !self.directory.exists() {
            return Ok(SaveListing::default());
        }
        for entry in fs::read_dir(&self.directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let slot = if name == "autosave.ron" {
                Some(SaveSlot::Autosave)
            } else {
                let numbered = name
                    .strip_prefix("slot-")
                    .and_then(|s| s.strip_suffix(".ron"))
                    .and_then(|number| number.parse::<u32>().ok())
                    .filter(|number| *number > 0)
                    .filter(|number| name == format!("slot-{number:02}.ron"))
                    .map(SaveSlot::Numbered);
                numbered.or_else(|| {
                    name.strip_suffix(".ron")
                        .filter(|stem| valid_save_name(stem))
                        .map(|stem| SaveSlot::Named(stem.to_string()))
                })
            };
            if let Some(slot) = slot {
                paths.push((slot, entry.path(), entry.metadata()?.modified()?));
            }
        }
        paths.sort_by_key(|(slot, path, _)| match slot {
            SaveSlot::Numbered(number) => (0_u8, *number, String::new()),
            SaveSlot::Named(name) => (1, 0, name.to_ascii_lowercase()),
            SaveSlot::Autosave => (2, 0, path.to_string_lossy().into_owned()),
        });
        let mut listing = SaveListing::default();
        for (slot, path, modified) in paths {
            let row = (|| {
                let text = fs::read_to_string(path)?;
                let logistics = load_complete_campaign(Arc::clone(&data), &text)?;
                let info = logistics.mission_info();
                let campaign_name = info
                    .campaign_display_name
                    .clone()
                    .unwrap_or_else(|| info.campaign_name.clone());
                let current_entry = info.current_entry();
                let (current_mission_name, current_mission_name_id) = current_entry
                    .and_then(|entry| entry.descriptive_name.as_ref())
                    .map(|display| match display {
                        mc2_sim_data::campaign::DisplayText::Literal(name) => {
                            (Some(name.clone()), None)
                        }
                        mc2_sim_data::campaign::DisplayText::Resource(id) => (None, Some(*id)),
                    })
                    .unwrap_or((None, None));
                Ok::<_, CampaignSaveError>(SaveMetadata {
                    slot: slot.clone(),
                    campaign_name,
                    campaign_name_id: info.campaign_name_id,
                    player_name: info.player_name.clone(),
                    current_mission_file: current_entry.map(|entry| entry.file_name.clone()),
                    current_mission_name,
                    current_mission_name_id,
                    c_bills: info.c_bills,
                    modified,
                })
            })();
            match row {
                Ok(metadata) => listing.saves.push(metadata),
                Err(error) => listing.invalid.push(InvalidSaveMetadata {
                    slot,
                    error: error.to_string(),
                }),
            }
        }
        Ok(listing)
    }

    fn write_campaign(
        &self,
        slot: SaveSlot,
        campaign: &Logistics,
    ) -> Result<(), CampaignSaveError> {
        let text = campaign.save_to_string()?;
        atomic_write(&self.path(slot)?, text.as_bytes())?;
        Ok(())
    }

    fn path(&self, slot: SaveSlot) -> Result<PathBuf, CampaignSaveError> {
        match slot {
            SaveSlot::Autosave => Ok(self.directory.join("autosave.ron")),
            SaveSlot::Numbered(0) => Err(CampaignSaveError::InvalidSlot),
            SaveSlot::Numbered(number) => Ok(self.directory.join(format!("slot-{number:02}.ron"))),
            SaveSlot::Named(name) if valid_save_name(&name) => {
                Ok(self.directory.join(format!("{name}.ron")))
            }
            SaveSlot::Named(_) => Err(CampaignSaveError::InvalidSlot),
        }
    }
}

fn load_complete_campaign(data: Arc<GameData>, text: &str) -> Result<Logistics, CampaignSaveError> {
    let loaded = Logistics::load_from_str(data, text)?;
    if !loaded.warnings.is_empty() {
        return Err(CampaignSaveError::DegradedLoad(loaded.warnings));
    }
    Ok(loaded.logistics)
}

fn valid_save_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.eq_ignore_ascii_case("autosave")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || " ._-".contains(c))
}

/// Use XDG on Linux, Application Support on macOS, and APPDATA on Windows.
pub fn default_save_directory() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|p| p.join("Library/Application Support/MechCommander 2/Saves"));
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("MechCommander 2/Saves"));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(path) = std::env::var_os("XDG_DATA_HOME") {
            return Some(PathBuf::from(path).join("mc2/saves"));
        }
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|p| p.join(".local/share/mc2/saves"));
    }
    #[allow(unreachable_code)]
    None
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    atomic_write_with(path, |file| file.write_all(contents))
}

fn atomic_write_with(
    path: &Path,
    write_contents: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path.file_name().unwrap_or_default().to_string_lossy();
    let (temp_path, mut file) = loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            sequence
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        write_contents(&mut file)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp_path, path)?;
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_campaign::interface::{ObjectiveReport, ObjectiveStatus};
    use mc2_campaign::pilot::{PilotReport, WarriorStatus};
    use mc2_campaign::results::{ResultsFlow, ResultsStage};
    use mc2_campaign::{Logistics, Outcome};
    use mc2_formats::vfs::Vfs;
    use mc2_sim_data::campaign::MissionSettings;

    fn game_data() -> Option<Arc<GameData>> {
        let data_root = std::env::var_os("MC2_DATA")?;
        let mut vfs = Vfs::from_system_cfg(data_root).expect("MC2_DATA system.cfg");
        Some(Arc::new(
            GameData::load(&mut vfs).expect("campaign game data"),
        ))
    }

    fn test_directory(label: &str) -> PathBuf {
        std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .expect("TMPDIR")
            .join(format!(
                "mc2-save-{label}-{}-{}",
                std::process::id(),
                TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ))
    }

    fn complete_results(flow: &mut ResultsFlow, campaign: &mut Logistics) {
        while !flow.is_done() {
            match flow.stage() {
                ResultsStage::Salvage => {
                    flow.finish_salvage(campaign);
                }
                ResultsStage::PilotReview => {
                    if let Some(pilot) = flow.pending_specialty(campaign) {
                        let skill = campaign.pilots()[pilot]
                            .specialty_offers()
                            .first()
                            .copied()
                            .expect("pending specialty has an offer");
                        flow.choose_specialty(campaign, pilot, skill).unwrap();
                    }
                    flow.finish_pilot_review(campaign);
                }
                ResultsStage::Done => break,
            }
        }
    }

    #[test]
    fn failed_partial_write_keeps_previous_slot_intact() {
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .expect("TMPDIR")
            .join(format!("mc2-save-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let target = root.join("slot-01.ron");
        fs::write(&target, b"complete old save").unwrap();
        let result = atomic_write_with(&target, |file| {
            file.write_all(b"partial new save")?;
            Err(io::Error::other(
                "simulated process interruption before rename",
            ))
        });
        assert!(result.is_err());
        assert_eq!(fs::read(&target).unwrap(), b"complete old save");
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "temporary file is cleaned up"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fresh_instance_round_trip_and_list_metadata_are_exact_and_ordered() {
        let Some(data) = game_data() else {
            eprintln!("MC2_DATA unset: skipping campaign save integration test");
            return;
        };
        let campaign = Logistics::new(Arc::clone(&data)).expect("new campaign");
        let root = test_directory("roundtrip");
        let _ = fs::remove_dir_all(&root);
        let service = CampaignSaveService::new(&root);

        service.save_slot(2, &campaign).unwrap();
        service.save_named("Player Save", &campaign).unwrap();
        service.save_named("slot-notes", &campaign).unwrap();
        service.save_named("slot-1", &campaign).unwrap();
        service.save_autosave(&campaign).unwrap();
        service.save_slot(1, &campaign).unwrap();

        let restored = service
            .load(SaveSlot::Numbered(1), Arc::clone(&data))
            .unwrap();
        assert_eq!(restored.to_save(), campaign.to_save());
        assert_eq!(
            service
                .load(
                    SaveSlot::Named("Player Save".to_string()),
                    Arc::clone(&data)
                )
                .unwrap()
                .to_save(),
            campaign.to_save()
        );
        let saves = service.list(data).unwrap();
        assert_eq!(
            saves
                .saves
                .iter()
                .map(|save| save.slot.clone())
                .collect::<Vec<_>>(),
            vec![
                SaveSlot::Numbered(1),
                SaveSlot::Numbered(2),
                SaveSlot::Named("Player Save".to_string()),
                SaveSlot::Named("slot-1".to_string()),
                SaveSlot::Named("slot-notes".to_string()),
                SaveSlot::Autosave,
            ]
        );
        assert!(saves.saves.iter().all(|save| save.campaign_name
            == campaign
                .mission_info()
                .campaign_display_name
                .clone()
                .unwrap_or_else(|| campaign.mission_info().campaign_name.clone())));
        assert!(saves
            .saves
            .iter()
            .all(|save| save.player_name == campaign.mission_info().player_name));
        assert!(saves
            .saves
            .iter()
            .all(|save| save.c_bills == campaign.mission_info().c_bills));
        assert!(saves
            .saves
            .iter()
            .all(|save| save.current_mission_file.as_deref()
                == Some(campaign.mission_info().current_mission_name.as_str())));
        assert!(saves.invalid.is_empty());
        for name in ["slot-notes", "slot-1"] {
            let named = service
                .load(
                    SaveSlot::Named(name.to_string()),
                    Arc::clone(campaign.data()),
                )
                .unwrap();
            assert_eq!(named.to_save(), campaign.to_save());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progressed_save_lists_the_current_selected_mission_and_balance() {
        let Some(data) = game_data() else {
            eprintln!("MC2_DATA unset: skipping campaign save integration test");
            return;
        };
        let mut campaign = Logistics::new(Arc::clone(&data)).expect("new campaign");
        let first_mission = campaign.mission_info().current_mission_name.clone();
        let outcome = Outcome {
            objectives: vec![ObjectiveReport {
                status: ObjectiveStatus::Successful,
                resource_points: 1234,
            }],
            ..Outcome::won()
        };
        campaign.apply_outcome(&outcome).unwrap();
        assert_ne!(
            campaign.mission_info().last_mission_name,
            campaign.current_mission()
        );

        let root = test_directory("metadata");
        let service = CampaignSaveService::new(&root);
        service.save_slot(1, &campaign).unwrap();
        let metadata = service.list(data).unwrap().saves.remove(0);
        assert_eq!(
            metadata.current_mission_file.as_deref(),
            Some(campaign.current_mission())
        );
        assert_ne!(
            metadata.current_mission_file.as_deref(),
            Some(first_mission.as_str())
        );
        match campaign.mission_info().current_mission_friendly_name() {
            Some(mc2_sim_data::campaign::DisplayText::Literal(expected)) => {
                assert_eq!(
                    metadata.current_mission_name.as_deref(),
                    Some(expected.as_str())
                );
                assert_eq!(metadata.current_mission_name_id, None);
            }
            Some(mc2_sim_data::campaign::DisplayText::Resource(expected)) => {
                assert_eq!(metadata.current_mission_name, None);
                assert_eq!(metadata.current_mission_name_id, Some(*expected));
            }
            None => {
                assert_eq!(metadata.current_mission_name, None);
                assert_eq!(metadata.current_mission_name_id, None);
            }
        }
        assert_eq!(metadata.c_bills, campaign.mission_info().c_bills);
        assert_eq!(
            metadata.modified,
            fs::metadata(service.directory().join("slot-01.ron"))
                .unwrap()
                .modified()
                .unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn listing_keeps_good_slots_and_reports_bad_slots_and_named_slot_aliases() {
        let Some(data) = game_data() else {
            eprintln!("MC2_DATA unset: skipping campaign save integration test");
            return;
        };
        let campaign = Logistics::new(Arc::clone(&data)).expect("new campaign");
        let root = test_directory("listing-errors");
        let service = CampaignSaveService::new(&root);
        service.save_slot(1, &campaign).unwrap();
        fs::write(root.join("slot-02.ron"), b"corrupt save").unwrap();
        fs::write(root.join("slot-1.ron"), campaign.save_to_string().unwrap()).unwrap();

        let listing = service.list(Arc::clone(&data)).unwrap();
        assert_eq!(
            listing
                .saves
                .iter()
                .map(|row| row.slot.clone())
                .collect::<Vec<_>>(),
            vec![SaveSlot::Numbered(1), SaveSlot::Named("slot-1".to_string())]
        );
        assert_eq!(listing.invalid.len(), 1);
        assert_eq!(listing.invalid[0].slot, SaveSlot::Numbered(2));
        assert!(listing.invalid[0].error.contains("does not parse"));
        assert_eq!(
            service
                .load(SaveSlot::Numbered(1), Arc::clone(&data))
                .unwrap()
                .to_save(),
            campaign.to_save()
        );
        assert_eq!(
            service
                .load(SaveSlot::Named("slot-1".to_string()), data)
                .unwrap()
                .to_save(),
            campaign.to_save()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn results_autosave_matches_cpp_eligibility_and_restores_post_results_state() {
        let Some(data) = game_data() else {
            eprintln!("MC2_DATA unset: skipping campaign save integration test");
            return;
        };
        let root = test_directory("autosave");
        let service = CampaignSaveService::new(&root);

        let untouched = Logistics::new(Arc::clone(&data)).unwrap();
        assert!(!service
            .save_autosave_if_eligible(&untouched, None, false)
            .unwrap());
        assert!(!root.join("autosave.ron").exists());

        let mut single = Logistics::new(Arc::clone(&data)).unwrap();
        single.set_single_mission("mc2_01", &MissionSettings::default(), None);
        assert!(!service
            .save_autosave_if_eligible(&single, None, false)
            .unwrap());
        assert!(!root.join("autosave.ron").exists());

        let mut multiplayer = Logistics::new(Arc::clone(&data)).unwrap();
        let multiplayer_outcome = Outcome::won();
        multiplayer.apply_outcome(&multiplayer_outcome).unwrap();
        let mut multiplayer_flow = multiplayer.begin_results(&multiplayer_outcome);
        complete_results(&mut multiplayer_flow, &mut multiplayer);
        let multiplayer_request = multiplayer_flow.end(&multiplayer);
        assert!(!service
            .save_autosave_if_eligible(&multiplayer, multiplayer_request.as_ref(), true)
            .unwrap());
        assert!(!root.join("autosave.ron").exists());

        let mut completed = Logistics::new(Arc::clone(&data)).unwrap();
        completed.add_c_bills(1_000_000);
        let variant = *completed
            .purchasable_mechs()
            .first()
            .expect("campaign offers a mech");
        let variant_name = completed.variant(variant).unwrap().name.clone();
        let pilot_index = completed
            .first_available_pilot()
            .expect("campaign has a pilot");
        let pilot_file = completed.pilots()[pilot_index].file_name.clone();
        let mech = completed.purchase_mech(variant).unwrap();
        completed.set_mech_pilot(mech, Some(pilot_index));
        completed.add_mech_to_force_group(mech, 1).unwrap();
        let outcome = Outcome {
            movers: vec![mc2_campaign::interface::MoverReport {
                on_gui: true,
                is_battle_mech: true,
                home_commander: true,
                is_air: false,
                destroyed: false,
                disabled: true,
                variant_name: variant_name.clone(),
                pilot_file: Some(pilot_file),
                paint_scheme: [1, 2, 3],
                pilot: PilotReport {
                    status: WarriorStatus::Dead,
                    gunnery_rank: 40.0,
                    piloting_rank: 40.0,
                    ..PilotReport::default()
                },
            }],
            mechs: vec![mc2_campaign::interface::MechSummary::wreck(
                &variant_name,
                50.0,
            )],
            objectives: vec![ObjectiveReport {
                status: ObjectiveStatus::Successful,
                resource_points: 4321,
            }],
            ..Outcome::won()
        };
        completed.apply_outcome(&outcome).unwrap();
        assert!(!completed.mission_info().last_mission_name.is_empty());
        assert!(completed.pilots()[pilot_index].just_died);
        let mut results = completed.begin_results(&outcome);
        assert_eq!(results.stage(), ResultsStage::PilotReview);
        assert!(
            results.end(&completed).is_none(),
            "no request before results"
        );
        assert!(!service
            .save_autosave_if_eligible(&completed, results.end(&completed).as_ref(), false)
            .unwrap());
        assert!(!root.join("autosave.ron").exists());
        let inventory_before_salvage = completed.inventory().len();
        // This mission's C++ flags skip the salvage screen, but the completed campaign still
        // exercises the persistent salvage mutation before its pilot-review screen closes.
        completed.finish_salvage(&[variant]);
        assert_eq!(completed.inventory().len(), inventory_before_salvage + 1);
        assert_eq!(results.stage(), ResultsStage::PilotReview);
        assert!(results
            .pilot_review()
            .unwrap()
            .killed
            .contains(&pilot_index));
        assert!(
            results.end(&completed).is_none(),
            "no request during pilot review"
        );
        results.finish_pilot_review(&mut completed);
        assert!(!completed.pilots()[pilot_index].just_died);
        let request = results.end(&completed);
        assert!(
            request.is_some(),
            "request only after salvage and pilot review"
        );
        assert!(service
            .save_autosave_if_eligible(&completed, request.as_ref(), false)
            .unwrap());
        let restored = service.load(SaveSlot::Autosave, Arc::clone(&data)).unwrap();
        assert_eq!(restored.to_save(), completed.to_save());

        let mut over = Logistics::new(Arc::clone(&data)).unwrap();
        while !over.campaign_over() {
            let current = over.current_mission().to_owned();
            assert!(!current.is_empty(), "campaign has a selected mission");
            let outcome = Outcome::won();
            over.apply_outcome(&outcome).unwrap();
            if !over.campaign_over() {
                let choices = over.begin_mission_selection().unwrap();
                let next = choices
                    .iter()
                    .find(|choice| choice.available)
                    .expect("campaign has an available next mission")
                    .name
                    .clone();
                over.choose_mission(&next).unwrap();
            }
        }
        let over_request = None;
        assert!(!service
            .save_autosave_if_eligible(&over, over_request.as_ref(), false)
            .unwrap());
        service.save_campaign_completion(&over).unwrap();
        assert_eq!(
            service
                .load(SaveSlot::Autosave, Arc::clone(&data))
                .unwrap()
                .to_save(),
            over.to_save(),
            "the final completion write can be loaded as an over campaign"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_rejects_saves_that_drop_missing_dependencies() {
        let Some(data) = game_data() else {
            eprintln!("MC2_DATA unset: skipping campaign save integration test");
            return;
        };
        let campaign = Logistics::new(Arc::clone(&data)).unwrap();
        let save = campaign.to_save();
        let pilot = save.pilots.first().expect("campaign has saved pilots");
        let old = format!("file_name: {:?}", pilot.file_name);
        let text =
            campaign
                .save_to_string()
                .unwrap()
                .replacen(&old, "file_name: \"missing-pilot\"", 1);
        assert_ne!(text, campaign.save_to_string().unwrap());

        let root = test_directory("degraded");
        let service = CampaignSaveService::new(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("slot-01.ron"), text).unwrap();
        match service.load(SaveSlot::Numbered(1), data).unwrap_err() {
            CampaignSaveError::DegradedLoad(warnings) => {
                assert!(warnings
                    .iter()
                    .any(|warning| warning.contains("missing-pilot")));
            }
            other => panic!("expected a degraded-load error, got {other}"),
        }
        fs::remove_dir_all(root).unwrap();
    }
}
