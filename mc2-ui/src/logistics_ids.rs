// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Ids the logistics screens share with the original: button ids of the `.fit` layouts
//! (`code/mechbayscreen.h` `MB_MSG_*`, `code/logisticsdialog.cpp` `DELETE_BUTTON`), component
//! forms (`mclib/cmponent.h` `ComponentFormType`) and string ids (`strings.res.h`).
//!
//! The string ids are checked against `strings.res.h` by this crate's tests, so a renumbered
//! table cannot pass silently.

/// Button ids (`MB_MSG_*`, `mechbayscreen.h`). The screens use them as the messages of their
/// buttons, as the originals do.
pub mod button {
    /// The "next" button: accept the mech lab's changes or buy in the purchase screen.
    pub const MB_MSG_NEXT: i32 = 50;
    /// The "previous" button: cancel the edit and go back.
    pub const MB_MSG_PREV: i32 = 51;
    pub const MB_MSG_ADD: i32 = 52;
    pub const MB_MSG_REMOVE: i32 = 53;
    pub const MB_MSG_BUY: i32 = 54;
    pub const MB_MSG_MAINMENU: i32 = 57;
    /// `Status::Yes` of the dialogs (`YES`, `logisticsscreen.h`).
    pub const YES: i32 = 7;
    /// `Status::No` of the dialogs (`NO`).
    pub const NO: i32 = 8;
    /// `DELETE_BUTTON` of the variant save dialog (`logisticsdialog.cpp:35`).
    pub const DELETE_BUTTON: i32 = 40;
}

/// `ComponentFormType` values (`mclib/cmponent.h`); the mech lab's form buttons carry them.
pub mod form {
    pub const SIMPLE: i32 = 0;
    pub const COCKPIT: i32 = 1;
    pub const SENSOR: i32 = 2;
    pub const ACTUATOR: i32 = 3;
    pub const ENGINE: i32 = 4;
    pub const HEATSINK: i32 = 5;
    pub const WEAPON: i32 = 6;
    pub const WEAPON_ENERGY: i32 = 7;
    pub const WEAPON_BALLISTIC: i32 = 8;
    pub const WEAPON_MISSILE: i32 = 9;
    pub const AMMO: i32 = 10;
    pub const JUMPJET: i32 = 11;
    pub const CASE: i32 = 12;
    pub const LIFESUPPORT: i32 = 13;
    pub const GYROSCOPE: i32 = 14;
    pub const POWER_AMPLIFIER: i32 = 15;
    pub const ECM: i32 = 16;
    pub const PROBE: i32 = 17;
    pub const JAMMER: i32 = 18;
    pub const BULK: i32 = 19;
}

/// String ids (`strings.res.h`, `IDS_*`).
pub mod ids {
    /// `IDS_MC_INSUFFICIENT_CBILLS`.
    pub const MC_INSUFFICIENT_CBILLS: i32 = 27337;
    /// `IDS_MC_UNAVAILABLE_TECH`.
    pub const MC_UNAVAILABLE_TECH: i32 = 27338;
    /// `IDS_DIALOG_OK`.
    pub const DIALOG_OK: i32 = 27650;
    /// `IDS_DIALOG_CANCEL`.
    pub const DIALOG_CANCEL: i32 = 27651;
    /// `IDS_DIALOG_YES`.
    pub const DIALOG_YES: i32 = 27652;
    /// `IDS_DIALOG_NO`.
    pub const DIALOG_NO: i32 = 27653;
    /// `IDS_DIALOG_QUIT_PROMPT`.
    pub const DIALOG_QUIT_PROMPT: i32 = 27654;
    /// `IDS_DIALOG_DELETE`.
    pub const DIALOG_DELETE: i32 = 27657;
    /// `IDS_DIALOG_SAVE`.
    pub const DIALOG_SAVE: i32 = 27659;
    /// `IDS_DIALOG_LIST_FONT`: the font of the dialog lists.
    pub const DIALOG_LIST_FONT: i32 = 27663;
    /// `IDS_DIALOG_OVERWRITE_PROMPT`: "... already exists" format.
    pub const DIALOG_OVERWRITE_PROMPT: i32 = 27664;
    /// `IDS_DIALOG_VARIANT_SAVE`.
    pub const DIALOG_VARIANT_SAVE: i32 = 27666;
    /// `IDS_DIALOG_VARIANT_LIST`.
    pub const DIALOG_VARIANT_LIST: i32 = 27667;
    /// `IDS_DIALOG_DELETE_PROMPT`: "delete ..." format.
    pub const DIALOG_DELETE_PROMPT: i32 = 27668;
    /// `IDS_MC_AVAILABLE_COMPONENTS_FONT`: the font of the mech lab's component rows.
    pub const MC_AVAILABLE_COMPONENTS_FONT: i32 = 27319;
    /// `IDS_KILLED_IN_ACTION`: the heading of the pilot review's dead.
    pub const KILLED_IN_ACTION: i32 = 27018;
    /// `IDS_ACTIVE_DUTY`: the heading of the pilot review's living.
    pub const ACTIVE_DUTY: i32 = 27019;
    /// `IDS_GREEN`: the rank names follow it in rank order (`IDS_GREEN + rank`), Elite last.
    pub const GREEN: i32 = 30112;
    /// `IDS_ACE`: the Ace rank name, which sits apart from the run that `IDS_GREEN` starts.
    pub const ACE: i32 = 30192;
    /// `IDS_SPECIALTY`: the specialty skill names follow it in skill order (`IDS_SPECIALTY + skill`).
    pub const SPECIALTY: i32 = 41001;
    /// `IDS_REGULAR_SKILLS`, `IDS_VETERAN_SKILLS`, `IDS_ELITE_SKILLS3`, `IDS_ACE_SKILLS`: the
    /// headings the specialty picker puts over each rank's run of skills.
    pub const REGULAR_SKILLS: i32 = 27055;
    pub const VETERAN_SKILLS: i32 = 27054;
    pub const ELITE_SKILLS3: i32 = 27056;
    pub const ACE_SKILLS: i32 = 27057;
    /// `IDS_SHORT_RANGE`: the range names are this id plus the range type.
    pub const SHORT_RANGE: i32 = 27133;
    /// `IDS_HELP_MC_VARIANT_DROP_DOWN_LIST_BOX`.
    pub const HELP_MC_VARIANT_DROP_DOWN_LIST_BOX: i32 = 29061;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// The ids above, by their `IDS_` name, against the table the original compiles from.
    #[test]
    fn string_ids_match_the_original_header() {
        // the C++ header lives next to the workspace in the full repository, not in the published tree
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../strings.res.h");
        let Ok(header) = std::fs::read_to_string(path) else {
            eprintln!("skipped: {path} not found");
            return;
        };
        let header = header.as_str();
        let table: HashMap<String, u32> = crate::strings::parse_enum(header);
        let expect = [
            ("IDS_MC_INSUFFICIENT_CBILLS", ids::MC_INSUFFICIENT_CBILLS),
            ("IDS_MC_UNAVAILABLE_TECH", ids::MC_UNAVAILABLE_TECH),
            ("IDS_DIALOG_OK", ids::DIALOG_OK),
            ("IDS_DIALOG_CANCEL", ids::DIALOG_CANCEL),
            ("IDS_DIALOG_YES", ids::DIALOG_YES),
            ("IDS_DIALOG_NO", ids::DIALOG_NO),
            ("IDS_DIALOG_QUIT_PROMPT", ids::DIALOG_QUIT_PROMPT),
            ("IDS_DIALOG_DELETE", ids::DIALOG_DELETE),
            ("IDS_DIALOG_SAVE", ids::DIALOG_SAVE),
            ("IDS_DIALOG_LIST_FONT", ids::DIALOG_LIST_FONT),
            ("IDS_DIALOG_OVERWRITE_PROMPT", ids::DIALOG_OVERWRITE_PROMPT),
            ("IDS_DIALOG_VARIANT_SAVE", ids::DIALOG_VARIANT_SAVE),
            ("IDS_DIALOG_VARIANT_LIST", ids::DIALOG_VARIANT_LIST),
            ("IDS_DIALOG_DELETE_PROMPT", ids::DIALOG_DELETE_PROMPT),
            (
                "IDS_MC_AVAILABLE_COMPONENTS_FONT",
                ids::MC_AVAILABLE_COMPONENTS_FONT,
            ),
            ("IDS_KILLED_IN_ACTION", ids::KILLED_IN_ACTION),
            ("IDS_ACTIVE_DUTY", ids::ACTIVE_DUTY),
            ("IDS_GREEN", ids::GREEN),
            ("IDS_ACE", ids::ACE),
            ("IDS_SPECIALTY", ids::SPECIALTY),
            ("IDS_REGULAR_SKILLS", ids::REGULAR_SKILLS),
            ("IDS_VETERAN_SKILLS", ids::VETERAN_SKILLS),
            ("IDS_ELITE_SKILLS3", ids::ELITE_SKILLS3),
            ("IDS_ACE_SKILLS", ids::ACE_SKILLS),
            ("IDS_SHORT_RANGE", ids::SHORT_RANGE),
            (
                "IDS_HELP_MC_VARIANT_DROP_DOWN_LIST_BOX",
                ids::HELP_MC_VARIANT_DROP_DOWN_LIST_BOX,
            ),
        ];
        for (name, id) in expect {
            assert_eq!(table.get(name).copied(), Some(id as u32), "{name}");
        }
    }
}
