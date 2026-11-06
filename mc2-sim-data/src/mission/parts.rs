// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission parts: `[Parts]` (count) and `[PartN]` (one block per object).
//!
//! Port of the parts read in `Mission::init`, `code/mission.cpp:2410-2690`.
//! Parts are numbered from 1; part 0 is reserved as "no object"
//! (mission.cpp:2416-2422). Object types, chassis and profiles are stored as
//! the numbers and names the file carries; nothing here resolves them.

use super::read::{opt, req, select};
use crate::bounds::{required_indexed, reserve_entries};
use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;

/// `ControlType` (mover.h:280-285). The raw value is kept in [`Part`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlType {
    Base = 0,
    Player = 1,
    Ai = 2,
    Net = 3,
}

impl ControlType {
    /// The enum for a raw value, or `None` for a value the game does not define.
    pub fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Base),
            1 => Some(Self::Player),
            2 => Some(Self::Ai),
            3 => Some(Self::Net),
            _ => None,
        }
    }
}

/// `ControlDataType` (mover.h:287-293).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlDataType {
    Base = 0,
    Mech = 1,
    GroundVehicle = 2,
    Elemental = 3,
}

impl ControlDataType {
    /// The enum for a raw value, or `None` for a value the game does not define.
    pub fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Base),
            1 => Some(Self::Mech),
            2 => Some(Self::GroundVehicle),
            3 => Some(Self::Elemental),
            _ => None,
        }
    }
}

/// One `[PartN]` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// The N in `PartN`, starting at 1.
    pub index: u32,
    /// `ul ObjectNumber`: the object type packet index (mission.cpp:2566).
    pub object_number: u32,
    /// `ul ControlType` (mission.cpp:2571). Raw value; see [`ControlType::from_raw`].
    pub control_type: u32,
    /// `ul ControlDataType` (mission.cpp:2574). Raw value.
    pub control_data_type: u32,
    /// `st ObjectProfile`, read into a 9 byte buffer (mission.cpp:2577).
    pub profile_name: String,
    /// `st CSVFile`: the chassis CSV name. Not read by `Mission::init`.
    pub csv_file: Option<String>,
    /// `ul VariantNumber`; `0` when absent (mission.cpp:2580-2585).
    pub variant_number: u32,
    /// `ul Pilot` (mission.cpp:2592). The pilot file reference, unresolved.
    pub pilot: u32,
    /// `f PositionX`, `f PositionY` (mission.cpp:2598-2601). Z is always `-1.0`.
    pub position: (f32, f32),
    /// `f Rotation` (mission.cpp:2606).
    pub rotation: f32,
    /// `c TeamID` (mission.cpp:2609). Signed, as C `char`.
    pub team_id: i8,
    /// `c CommanderID`, falling back to `l CommanderId` (mission.cpp:2620-2626).
    pub commander_id: i32,
    /// `ul BaseColor`, `ul HighlightColor1`, `ul HighlightColor2`
    /// (mission.cpp:2647-2656). `None` when absent; the C++ then uses prefs.
    pub base_color: Option<u32>,
    pub highlight_color1: Option<u32>,
    pub highlight_color2: Option<u32>,
    /// `l Active`, `l Exists` (mission.cpp:2661-2664).
    pub active: i32,
    pub exists: i32,
    /// `f Damage`, default `0.0` (mission.cpp:2668). `destroyed` is set when it
    /// is `>= 1.0`.
    pub damage: f32,
    pub destroyed: bool,
    /// `c MyIcon` (mission.cpp:2675).
    pub my_icon: i8,
    /// `b Captureable`; `false` when absent (mission.cpp:2681).
    pub captureable: bool,
    /// `b PlayerPart` (missionbriefingscreen.cpp:491). Not read by `Mission::init`.
    pub player_part: bool,
    /// `ul squadNum` (mission.cpp:2463); `0` when absent.
    pub squad_num: u32,
    /// `ul SelfRepairBehavior`. Not read by `Mission::init`.
    pub self_repair_behavior: Option<u32>,
    /// `ul NumAlternatives`, `ul AlternativeStartIndex`. Not read by the C++
    /// loader, which derives the alternative count from the index list.
    pub num_alternatives: Option<u32>,
    pub alternative_start_index: Option<u32>,
    /// `l[15] IndicesOfAlternatives` (mission.cpp:2472), as written, including
    /// the `-1` padding. Empty when the key is absent.
    pub indices_of_alternatives: Vec<i32>,
}

impl Part {
    /// `control_type` as an enum, if the value is defined.
    pub fn control(&self) -> Option<ControlType> {
        ControlType::from_raw(self.control_type)
    }

    /// `control_data_type` as an enum, if the value is defined.
    pub fn control_data(&self) -> Option<ControlDataType> {
        ControlDataType::from_raw(self.control_data_type)
    }
}

/// Maximum alternatives per part (mission.cpp:2423).
pub const MAX_ALTERNATIVES: usize = 15;

/// Reads `[Parts]` and every `[PartN]`. Fails when `[Parts]`, `NumParts`, a part
/// block or a required part key is missing (the C++ asserts on each).
pub(crate) fn load_parts(fit: &mut FitIni, file: &str) -> Result<Vec<Part>> {
    if !select(fit, "Parts")? {
        return Err(SimDataError::schema(file, "missing block [Parts]"));
    }
    let count = req(fit.read_u32("NumParts"), file, "NumParts")?;
    let count = required_indexed(fit, file, "NumParts", u64::from(count), "Part", "", 1)?;
    let mut parts = reserve_entries(file, count)?;
    for index in 1..=count as u32 {
        let block = format!("Part{index}");
        if !select(fit, &block)? {
            return Err(SimDataError::schema(
                file,
                format!("missing block [{block}]"),
            ));
        }
        parts.push(read_part(fit, file, index)?);
    }
    Ok(parts)
}

fn read_part(fit: &mut FitIni, file: &str, index: u32) -> Result<Part> {
    let object_number = req(fit.read_u32("ObjectNumber"), file, "ObjectNumber")?;
    let control_type = req(fit.read_u32("ControlType"), file, "ControlType")?;
    let control_data_type = req(fit.read_u32("ControlDataType"), file, "ControlDataType")?;
    let profile_name = req(fit.read_string("ObjectProfile", 9), file, "ObjectProfile")?;
    let csv_file = opt(fit.read_string("CSVFile", 256))?;
    let variant_number = opt(fit.read_u32("VariantNumber"))?.unwrap_or(0);
    let pilot = req(fit.read_u32("Pilot"), file, "Pilot")?;
    let position = (
        req(fit.read_f32("PositionX"), file, "PositionX")?,
        req(fit.read_f32("PositionY"), file, "PositionY")?,
    );
    let rotation = req(fit.read_f32("Rotation"), file, "Rotation")?;
    let team_id = req(fit.read_i8("TeamId"), file, "TeamId")?;
    let commander_id = match opt(fit.read_i8("CommanderId"))? {
        Some(c) => i32::from(c),
        None => req(fit.read_i32("CommanderId"), file, "CommanderId")?,
    };
    let base_color = opt(fit.read_u32("BaseColor"))?;
    let highlight_color1 = opt(fit.read_u32("HighlightColor1"))?;
    let highlight_color2 = opt(fit.read_u32("HighlightColor2"))?;
    let active = req(fit.read_i32("Active"), file, "Active")?;
    let exists = req(fit.read_i32("Exists"), file, "Exists")?;
    let damage = opt(fit.read_f32("Damage"))?.unwrap_or(0.0);
    let my_icon = req(fit.read_i8("MyIcon"), file, "MyIcon")?;
    let captureable = opt(fit.read_bool("Captureable"))?.unwrap_or(false);
    let player_part = opt(fit.read_bool("PlayerPart"))?.unwrap_or(false);
    let squad_num = opt(fit.read_u32("SquadNum"))?.unwrap_or(0);
    let self_repair_behavior = opt(fit.read_u32("SelfRepairBehavior"))?;
    let num_alternatives = opt(fit.read_u32("NumAlternatives"))?;
    let alternative_start_index = opt(fit.read_u32("AlternativeStartIndex"))?;
    let indices_of_alternatives =
        opt(fit.read_array::<i32>("IndicesOfAlternatives", MAX_ALTERNATIVES))?.unwrap_or_default();
    Ok(Part {
        index,
        object_number,
        control_type,
        control_data_type,
        profile_name,
        csv_file,
        variant_number,
        pilot,
        position,
        rotation,
        team_id,
        commander_id,
        base_color,
        highlight_color1,
        highlight_color2,
        active,
        exists,
        damage,
        destroyed: damage >= 1.0,
        my_icon,
        captureable,
        player_part,
        squad_num,
        self_repair_behavior,
        num_alternatives,
        alternative_start_index,
        indices_of_alternatives,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PART: &str = "[Part1]\nul ObjectNumber = 12\nul ControlType = 1\n\
        ul ControlDataType = 1\nst ObjectProfile = \"PM207300\"\nul Pilot = 3\n\
        f PositionX = 10.5\nf PositionY = -4\nf Rotation = 1.25\nc TeamID = 2\n\
        c CommanderID = 1\nl Active = 1\nl Exists = 1\nf Damage = 1\nc MyIcon = 0\n\
        ul squadNum = 9\nl[15] IndicesOfAlternatives = 2, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0\n";

    #[test]
    fn parts_read_required_and_defaulted_keys() {
        let text = format!("FITini\n[Parts]\nul NumParts = 1\n{PART}");
        let mut f = FitIni::from_bytes(format!("{text}\n").into_bytes()).unwrap();
        let parts = load_parts(&mut f, "t.fit").unwrap();
        assert_eq!(parts.len(), 1);
        let p = &parts[0];
        assert_eq!(p.index, 1);
        assert_eq!(p.object_number, 12);
        assert_eq!(p.profile_name, "PM207300");
        assert_eq!(p.control(), Some(ControlType::Player));
        assert_eq!(p.control_data(), Some(ControlDataType::Mech));
        assert_eq!(p.position, (10.5, -4.0));
        assert_eq!(p.team_id, 2);
        assert_eq!(p.commander_id, 1);
        assert!(p.destroyed);
        assert_eq!(p.variant_number, 0);
        assert_eq!(p.squad_num, 9);
        assert_eq!(p.indices_of_alternatives.len(), 15);
        assert_eq!(p.indices_of_alternatives[0], 2);
        assert_eq!(p.base_color, None);
        assert!(!p.captureable);
    }

    #[test]
    fn missing_part_block_is_an_error() {
        let mut f = FitIni::from_bytes(b"FITini\n[Parts]\nul NumParts = 1\n".to_vec()).unwrap();
        assert!(load_parts(&mut f, "t.fit").is_err());
    }

    #[test]
    fn control_enums_reject_undefined_values() {
        assert_eq!(ControlType::from_raw(3), Some(ControlType::Net));
        assert_eq!(ControlType::from_raw(4), None);
        assert_eq!(
            ControlDataType::from_raw(3),
            Some(ControlDataType::Elemental)
        );
        assert_eq!(ControlDataType::from_raw(9), None);
    }
}
