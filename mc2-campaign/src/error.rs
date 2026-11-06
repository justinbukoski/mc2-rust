// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Error codes of the logistics layer.
//!
//! Port of `code/logisticserrors.h`. The C++ functions return `0` for success and one of these
//! numbers (or `-1`) otherwise. [`LogisticsError::code`] gives the original number so callers that
//! want to compare against the C++ behaviour can.

use std::fmt;

/// A failed logistics operation. One variant per code in `logisticserrors.h`, plus [`Failed`]
/// for the bare `-1` the C++ returns when an argument is null or an item is not found.
///
/// [`Failed`]: LogisticsError::Failed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogisticsError {
    /// `-1`: null argument, item not found, or nothing to do.
    Failed,
    /// `NOT_YET_IMPLEMENTED` (1000).
    NotYetImplemented,
    /// `INVALID_COMPONENT` / `INVALID_PILOT` / `INVALID_ID` / `INVALID_MISSION` (all 1001).
    InvalidId,
    /// `NEED_BIGGER_ARRAY` (1003). Kept for completeness; the Rust API returns vectors.
    NeedBiggerArray,
    /// `INVALID_SKILL` (1004).
    InvalidSkill,
    /// `WRONG_PARAMS` (1005).
    WrongParams,
    /// `NOT_ENOUGH_RESOURCE_POINTS` (1006): not enough C-Bills.
    NotEnoughResourcePoints,
    /// `COMPONENT_TOO_HEAVY` (1007). The C++ check is commented out ("weight no longer matters").
    ComponentTooHeavy,
    /// `COMPONENT_SLOT_FULL` (1008).
    ComponentSlotFull,
    /// `SENSORS_NOT_ALLOWED` (1009).
    SensorsNotAllowed,
    /// `ECM_NOT_ALLOWED` (1010).
    EcmNotAllowed,
    /// `COMPONENT_TOO_HOT` (1011).
    ComponentTooHot,
    /// `JUMPJETS_NOT_ALLOWED` (1012).
    JumpJetsNotAllowed,
    /// `ONLY_ONE_SENSOR_ALLOWED` (1013).
    OnlyOneSensorAllowed,
    /// `ONLY_ONE_ECM_ALLOWED` (1014).
    OnlyOneEcmAllowed,
    /// `ONLY_ONE_JUMPJET_ALLOWED` (1015).
    OnlyOneJumpJetAllowed,
    /// `INVALID_LOCATION` (1016).
    InvalidLocation,
    /// `ADD_COMPONENT_FAILED` (1017).
    AddComponentFailed,
    /// `SAVE_NAME_IN_USE` (1018).
    SaveNameInUse,
    /// `NAME_IN_INVENTORY` (1019).
    NameInInventory,
    /// `DESIGNER_MECH` (1020).
    DesignerMech,
    /// `PILOT_ALREADY_IN_USE` (1021).
    PilotAlreadyInUse,
    /// `MISSION_ALREADY_DONE` (1023).
    MissionAlreadyDone,
    /// `INVALID_FILE_NAME` (1024).
    InvalidFileName,
    /// `NO_PURCHASE_FILE` (1025).
    NoPurchaseFile,
    /// `COMPONENT_NOT_FOUND` (1026).
    ComponentNotFound,
    /// `INVALID_VARIANT` (1027).
    InvalidVariant,
    /// `VARIANT_IN_USE` (1028).
    VariantInUse,
    /// `NO_MORE_ARMOR` (1029).
    NoMoreArmor,
    /// `INSUFFICIENT_HEAT` (1030).
    InsufficientHeat,
}

impl LogisticsError {
    /// The number the C++ returns for this error.
    pub fn code(self) -> i32 {
        match self {
            Self::Failed => -1,
            Self::NotYetImplemented => 1000,
            Self::InvalidId => 1001,
            Self::NeedBiggerArray => 1003,
            Self::InvalidSkill => 1004,
            Self::WrongParams => 1005,
            Self::NotEnoughResourcePoints => 1006,
            Self::ComponentTooHeavy => 1007,
            Self::ComponentSlotFull => 1008,
            Self::SensorsNotAllowed => 1009,
            Self::EcmNotAllowed => 1010,
            Self::ComponentTooHot => 1011,
            Self::JumpJetsNotAllowed => 1012,
            Self::OnlyOneSensorAllowed => 1013,
            Self::OnlyOneEcmAllowed => 1014,
            Self::OnlyOneJumpJetAllowed => 1015,
            Self::InvalidLocation => 1016,
            Self::AddComponentFailed => 1017,
            Self::SaveNameInUse => 1018,
            Self::NameInInventory => 1019,
            Self::DesignerMech => 1020,
            Self::PilotAlreadyInUse => 1021,
            Self::MissionAlreadyDone => 1023,
            Self::InvalidFileName => 1024,
            Self::NoPurchaseFile => 1025,
            Self::ComponentNotFound => 1026,
            Self::InvalidVariant => 1027,
            Self::VariantInUse => 1028,
            Self::NoMoreArmor => 1029,
            Self::InsufficientHeat => 1030,
        }
    }

    /// Maps a C++ return code back to an error. `0` is success and gives `None`.
    pub fn from_code(code: i32) -> Option<Self> {
        Some(match code {
            0 => return None,
            1000 => Self::NotYetImplemented,
            1001 => Self::InvalidId,
            1003 => Self::NeedBiggerArray,
            1004 => Self::InvalidSkill,
            1005 => Self::WrongParams,
            1006 => Self::NotEnoughResourcePoints,
            1007 => Self::ComponentTooHeavy,
            1008 => Self::ComponentSlotFull,
            1009 => Self::SensorsNotAllowed,
            1010 => Self::EcmNotAllowed,
            1011 => Self::ComponentTooHot,
            1012 => Self::JumpJetsNotAllowed,
            1013 => Self::OnlyOneSensorAllowed,
            1014 => Self::OnlyOneEcmAllowed,
            1015 => Self::OnlyOneJumpJetAllowed,
            1016 => Self::InvalidLocation,
            1017 => Self::AddComponentFailed,
            1018 => Self::SaveNameInUse,
            1019 => Self::NameInInventory,
            1020 => Self::DesignerMech,
            1021 => Self::PilotAlreadyInUse,
            1023 => Self::MissionAlreadyDone,
            1024 => Self::InvalidFileName,
            1025 => Self::NoPurchaseFile,
            1026 => Self::ComponentNotFound,
            1027 => Self::InvalidVariant,
            1028 => Self::VariantInUse,
            1029 => Self::NoMoreArmor,
            1030 => Self::InsufficientHeat,
            _ => Self::Failed,
        })
    }
}

impl fmt::Display for LogisticsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::Failed => "operation failed",
            Self::NotYetImplemented => "not yet implemented",
            Self::InvalidId => "invalid id",
            Self::NeedBiggerArray => "array too small",
            Self::InvalidSkill => "invalid skill",
            Self::WrongParams => "wrong parameters",
            Self::NotEnoughResourcePoints => "not enough C-Bills",
            Self::ComponentTooHeavy => "component too heavy",
            Self::ComponentSlotFull => "component slot full",
            Self::SensorsNotAllowed => "sensors not allowed on this chassis",
            Self::EcmNotAllowed => "ECM not allowed on this chassis",
            Self::ComponentTooHot => "component adds more heat than the chassis can shed",
            Self::JumpJetsNotAllowed => "jump jets not allowed on this chassis",
            Self::OnlyOneSensorAllowed => "only one sensor allowed",
            Self::OnlyOneEcmAllowed => "only one ECM allowed",
            Self::OnlyOneJumpJetAllowed => "only one set of jump jets allowed",
            Self::InvalidLocation => "invalid location",
            Self::AddComponentFailed => "could not add the component",
            Self::SaveNameInUse => "variant name already in use",
            Self::NameInInventory => "variant name belongs to a mech in the inventory",
            Self::DesignerMech => "designer mech",
            Self::PilotAlreadyInUse => "pilot already in use",
            Self::MissionAlreadyDone => "mission already completed",
            Self::InvalidFileName => "invalid file name",
            Self::NoPurchaseFile => "no purchase file",
            Self::ComponentNotFound => "component not found",
            Self::InvalidVariant => "invalid variant",
            Self::VariantInUse => "variant is used by a mech in the inventory",
            Self::NoMoreArmor => "no more armor allowed",
            Self::InsufficientHeat => "removing the heat sink would leave the mech overheated",
        };
        write!(f, "{text} (code {})", self.code())
    }
}

impl std::error::Error for LogisticsError {}

/// Result of a logistics operation.
pub type LogisticsResult<T> = std::result::Result<T, LogisticsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for code in 1000..=1030 {
            if let Some(e) = LogisticsError::from_code(code) {
                if e != LogisticsError::Failed {
                    assert_eq!(e.code(), code);
                }
            }
        }
        assert_eq!(LogisticsError::from_code(0), None);
        assert_eq!(LogisticsError::ComponentTooHot.code(), 1011);
        assert_eq!(LogisticsError::InsufficientHeat.code(), 1030);
        assert_eq!(LogisticsError::Failed.code(), -1);
    }
}
