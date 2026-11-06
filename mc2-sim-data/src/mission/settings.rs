// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission-level scalar blocks: `[MissionSettings]`, `[TheSky]`, `[DropZone0]`,
//! `[Script]` and `[Music]`.
//!
//! `code/mission.cpp` reads `MissionSettings` in `Mission::init` (lines
//! 2126-2147 and 2803-2812). Most keys in the block are read by the multiplayer
//! and logistics screens instead; each field notes its reader.

use super::read::{opt, opt_text, req, select};
use crate::error::Result;
use mc2_formats::ini::FitIni;

/// Sky number used when `[TheSky]` is absent or out of range.
pub const DEFAULT_SKY: i32 = 1; // code/mission.cpp:315
/// Highest sky number the loader accepts.
pub const MAX_SKY: i32 = 21; // code/mission.cpp:1984-1988

/// `[MissionSettings]`. Required keys are the ones `Mission::init` asserts on.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionSettings {
    /// `uc scenarioTuneNum` (mission.cpp:2129).
    pub scenario_tune_num: u8,
    /// `l ResourcePoints` (mission.cpp:2133). The C++ reads it unchecked, so a
    /// missing key leaves an uninitialised value; the data always has it.
    pub resource_points: i32,
    /// `f TimeLimit`, seconds; `-1.0` means the mission never times out
    /// (mission.cpp:2803-2812).
    pub time_limit: f32,
    /// `ul NumRandomRPbuildings` (mission.cpp:2147, multiplayer only).
    pub num_random_rp_buildings: Option<u32>,
    /// `st MissionName` (mploadmap.cpp:555, mpparameterscreen.cpp:630).
    pub mission_name: Option<String>,
    pub mission_name_use_resource_string: Option<bool>,
    pub mission_name_resource_string_id: Option<u32>,
    /// `st Author`. Not read by the shipped C++ readers.
    pub author: Option<String>,
    /// `st Blurb` (missionbriefingscreen.cpp).
    pub blurb: Option<String>,
    pub blurb_use_resource_string: Option<bool>,
    pub blurb_resource_string_id: Option<u32>,
    /// `st Blurb2` (logisticsmissioninfo.cpp:292, mploadmap.cpp:594).
    pub blurb2: Option<String>,
    pub blurb2_use_resource_string: Option<bool>,
    pub blurb2_resource_string_id: Option<u32>,
    /// `st DownloadURL` (mpparameterscreen.cpp:664).
    pub download_url: Option<String>,
    /// `st AVIFilename`. Not read by the shipped C++ readers.
    pub avi_filename: Option<String>,
    /// `ul MissionType` (logisticsdialog.cpp:1820, mploadmap.cpp:570).
    pub mission_type: Option<u32>,
    /// `ul IsSinglePlayer` (mploadmap.cpp:175).
    pub is_single_player: Option<u32>,
    /// `ul MaximumNumberOfPlayers` (logisticsdialog.cpp:1832, mploadmap.cpp:580).
    pub maximum_number_of_players: Option<u32>,
    /// `ul MaximumNumberOfTeams` (mpparameterscreen.cpp:666).
    pub maximum_number_of_teams: Option<u32>,
    /// `l AdditionalCBills` (logisticsmissioninfo.cpp:310).
    pub additional_c_bills: Option<i32>,
    /// `f DropWeightLimit` (logisticsmissioninfo.cpp:312).
    pub drop_weight_limit: Option<f32>,
    /// The `*EnabledDefault` flags (logisticsmissioninfo.cpp:319-338,
    /// mpparameterscreen.cpp:650-663).
    pub air_strikes_enabled_default: Option<bool>,
    pub all_tech_enabled_default: Option<bool>,
    pub artillery_piece_enabled_default: Option<bool>,
    pub mine_layers_enabled_default: Option<bool>,
    pub no_variants_enabled_default: Option<bool>,
    pub rp_for_mechs_enabled_default: Option<bool>,
    pub repair_vehicle_enabled_default: Option<bool>,
    pub resource_buildings_enabled_default: Option<bool>,
    pub salvage_craft_enabled_default: Option<bool>,
    pub scout_copters_enabled_default: Option<bool>,
    pub sensor_probes_enabled_default: Option<bool>,
    pub unlimited_ammo_enabled_default: Option<bool>,
}

/// Text buffer used for the mission name, author and URL readers.
const NAME_BUFFER: usize = 256;
/// Text buffer used for the blurbs. The C++ readers use 1024 and 4096; the
/// larger limit is kept so the typed model never refuses a shipped string.
const BLURB_BUFFER: usize = 4096;

pub(crate) fn load_settings(fit: &mut FitIni, file: &str) -> Result<MissionSettings> {
    if !select(fit, "MissionSettings")? {
        return Err(crate::error::SimDataError::schema(
            file,
            "missing block [MissionSettings]",
        ));
    }
    Ok(MissionSettings {
        scenario_tune_num: req(fit.read_u8("scenarioTuneNum"), file, "scenarioTuneNum")?,
        resource_points: req(fit.read_i32("ResourcePoints"), file, "ResourcePoints")?,
        time_limit: opt(fit.read_f32("TimeLimit"))?.unwrap_or(-1.0),
        num_random_rp_buildings: opt(fit.read_u32("NumRandomRPbuildings"))?,
        mission_name: opt_text(fit, "MissionName", NAME_BUFFER)?,
        mission_name_use_resource_string: opt(fit.read_bool("MissionNameUseResourceString"))?,
        mission_name_resource_string_id: opt(fit.read_u32("MissionNameResourceStringID"))?,
        author: opt_text(fit, "Author", NAME_BUFFER)?,
        blurb: opt_text(fit, "Blurb", BLURB_BUFFER)?,
        blurb_use_resource_string: opt(fit.read_bool("BlurbUseResourceString"))?,
        blurb_resource_string_id: opt(fit.read_u32("BlurbResourceStringID"))?,
        blurb2: opt_text(fit, "Blurb2", BLURB_BUFFER)?,
        blurb2_use_resource_string: opt(fit.read_bool("Blurb2UseResourceString"))?,
        blurb2_resource_string_id: opt(fit.read_u32("Blurb2ResourceStringID"))?,
        download_url: opt_text(fit, "DownloadURL", NAME_BUFFER)?,
        avi_filename: opt_text(fit, "AVIFilename", NAME_BUFFER)?,
        mission_type: opt(fit.read_u32("MissionType"))?,
        is_single_player: opt(fit.read_u32("IsSinglePlayer"))?,
        maximum_number_of_players: opt(fit.read_u32("MaximumNumberOfPlayers"))?,
        maximum_number_of_teams: opt(fit.read_u32("MaximumNumberOfTeams"))?,
        additional_c_bills: opt(fit.read_i32("AdditionalCBills"))?,
        drop_weight_limit: opt(fit.read_f32("DropWeightLimit"))?,
        air_strikes_enabled_default: opt(fit.read_bool("AirStrikesEnabledDefault"))?,
        all_tech_enabled_default: opt(fit.read_bool("AllTechEnabledDefault"))?,
        artillery_piece_enabled_default: opt(fit.read_bool("ArtilleryPieceEnabledDefault"))?,
        mine_layers_enabled_default: opt(fit.read_bool("MineLayersEnabledDefault"))?,
        no_variants_enabled_default: opt(fit.read_bool("NoVariantsEnabledDefault"))?,
        rp_for_mechs_enabled_default: opt(fit.read_bool("RPsForMechsEnabledDefault"))?,
        repair_vehicle_enabled_default: opt(fit.read_bool("RepairVehicleEnabledDefault"))?,
        resource_buildings_enabled_default: opt(fit.read_bool("ResourceBuildingsEnabledDefault"))?,
        salvage_craft_enabled_default: opt(fit.read_bool("SalvageCraftEnabledDefault"))?,
        scout_copters_enabled_default: opt(fit.read_bool("ScoutCoptersEnabledDefault"))?,
        sensor_probes_enabled_default: opt(fit.read_bool("SensorProbesEnabledDefault"))?,
        unlimited_ammo_enabled_default: opt(fit.read_bool("UnlimitedAmmoEnabledDefault"))?,
    })
}

/// `[TheSky]` `l SkyNumber` after the loader's fallback rules
/// (mission.cpp:1974-1988): an absent block or key, or a value outside 1..=21,
/// becomes [`DEFAULT_SKY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TheSky {
    pub sky_number: i32,
}

pub(crate) fn load_sky(fit: &mut FitIni, file: &str) -> Result<TheSky> {
    let _ = file;
    if !select(fit, "TheSky")? {
        return Ok(TheSky {
            sky_number: DEFAULT_SKY,
        });
    }
    let raw = opt(fit.read_i32("SkyNumber"))?.unwrap_or(DEFAULT_SKY);
    let sky_number = if (1..=MAX_SKY).contains(&raw) {
        raw
    } else {
        DEFAULT_SKY
    };
    Ok(TheSky { sky_number })
}

/// `[Script]` `st ScenarioScript` (mission.cpp:2272-2275). The C++ buffer is
/// 80 bytes, so the value must be shorter than 79.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Script {
    pub scenario_script: String,
}

pub(crate) fn load_script(fit: &mut FitIni, file: &str) -> Result<Script> {
    if !select(fit, "Script")? {
        return Err(crate::error::SimDataError::schema(
            file,
            "missing block [Script]",
        ));
    }
    let scenario_script = req(
        fit.read_string("ScenarioScript", 79),
        file,
        "ScenarioScript",
    )?;
    Ok(Script { scenario_script })
}

/// `[Music]` `uc scenarioTuneNum`. The C++ seek is commented out at
/// mission.cpp:2125 and the tune number comes from `MissionSettings`, so this
/// block is decoded but not used by the loader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Music {
    pub scenario_tune_num: Option<u8>,
}

pub(crate) fn load_music(fit: &mut FitIni) -> Result<Option<Music>> {
    if !select(fit, "Music")? {
        return Ok(None);
    }
    Ok(Some(Music {
        scenario_tune_num: opt(fit.read_u8("scenarioTuneNum"))?,
    }))
}

/// `[DropZone0]` `f PositionX`, `f PositionY` (mission.cpp:2106-2117). Absent
/// values are `-1.0`, as in the C++.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropZone {
    pub x: f32,
    pub y: f32,
}

pub(crate) fn load_drop_zone(fit: &mut FitIni) -> Result<DropZone> {
    if !select(fit, "DropZone0")? {
        return Ok(DropZone { x: -1.0, y: -1.0 });
    }
    Ok(DropZone {
        x: opt(fit.read_f32("PositionX"))?.unwrap_or(-1.0),
        y: opt(fit.read_f32("PositionY"))?.unwrap_or(-1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("{text}\n").into_bytes()).expect("fit")
    }

    #[test]
    fn settings_read_required_and_optional_keys() {
        let mut f = fit(
            "FITini\n[MissionSettings]\nuc scenarioTuneNum = 3\nl ResourcePoints = 500\n\
             st MissionName = \"Test\"\nb AirStrikesEnabledDefault = TRUE\n",
        );
        let s = load_settings(&mut f, "t.fit").unwrap();
        assert_eq!(s.scenario_tune_num, 3);
        assert_eq!(s.resource_points, 500);
        assert_eq!(s.time_limit, -1.0);
        assert_eq!(s.mission_name.as_deref(), Some("Test"));
        assert_eq!(s.air_strikes_enabled_default, Some(true));
        assert_eq!(s.download_url, None);
    }

    #[test]
    fn settings_require_resource_points() {
        let mut f = fit("FITini\n[MissionSettings]\nuc scenarioTuneNum = 3\n");
        assert!(load_settings(&mut f, "t.fit").is_err());
    }

    #[test]
    fn sky_falls_back_outside_range_and_when_absent() {
        let mut f = fit("FITini\n[TheSky]\nl SkyNumber = 22\n[Other]\nl x = 1\n");
        assert_eq!(load_sky(&mut f, "t").unwrap().sky_number, DEFAULT_SKY);
        let mut f = fit("FITini\n[TheSky]\nl SkyNumber = 7\n");
        assert_eq!(load_sky(&mut f, "t").unwrap().sky_number, 7);
        let mut f = fit("FITini\n[Other]\nl x = 1\n");
        assert_eq!(load_sky(&mut f, "t").unwrap().sky_number, DEFAULT_SKY);
    }

    #[test]
    fn drop_zone_defaults_to_minus_one() {
        let mut f = fit("FITini\n[Other]\nl x = 1\n");
        assert_eq!(
            load_drop_zone(&mut f).unwrap(),
            DropZone { x: -1.0, y: -1.0 }
        );
        let mut f = fit("FITini\n[DropZone0]\nf PositionX = 2.5\nf PositionY = 4\n");
        assert_eq!(load_drop_zone(&mut f).unwrap(), DropZone { x: 2.5, y: 4.0 });
    }

    #[test]
    fn script_reads_name_and_music_is_optional() {
        let mut f = fit("FITini\n[Script]\nst ScenarioScript = \"mc2_01\"\n");
        assert_eq!(load_script(&mut f, "t").unwrap().scenario_script, "mc2_01");
        let mut f = fit("FITini\n[Other]\nl x = 1\n");
        assert_eq!(load_music(&mut f).unwrap(), None);
    }
}
