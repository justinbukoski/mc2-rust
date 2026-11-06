// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Objective actions: the 7 `action_species_type` values and the typed
//! parameters each species reads.
//!
//! Enum and names: code/objective.h:478-487 and code/objective.cpp:61-69.
//! Construction: `CObjective::new_CObjectiveAction` (code/objective.cpp:1433-1451).
//! Parameter reads: the `Read` methods in code/objective.cpp (line refs on each
//! variant).

use super::condition::{object_ref, ObjectRef};
use super::read::{objective_text, opt, Read};
use crate::error::Result;
use mc2_formats::ini::FitIni;

/// `action_species_type` (objective.h:478-487). Discriminants follow the C++
/// enum order, which is also the index into the name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionSpecies {
    PlayBik = 0,
    PlayWav = 1,
    DisplayTextMessage = 2,
    DisplayResourceTextMessage = 3,
    SetBooleanFlag = 4,
    MakeNewTechnologyAvailable = 5,
    RemoveStructure = 6,
}

/// Number of action species (objective.h:487, `NUM_ACTION_SPECIES`).
pub const NUM_ACTION_SPECIES: usize = 7;

impl ActionSpecies {
    /// Every species in enum order.
    pub const ALL: [ActionSpecies; NUM_ACTION_SPECIES] = [
        Self::PlayBik,
        Self::PlayWav,
        Self::DisplayTextMessage,
        Self::DisplayResourceTextMessage,
        Self::SetBooleanFlag,
        Self::MakeNewTechnologyAvailable,
        Self::RemoveStructure,
    ];

    /// The name the data stores in `ActionSpeciesString` (objective.cpp:61-69).
    pub fn name(self) -> &'static str {
        match self {
            Self::PlayBik => "PlayBIK",
            Self::PlayWav => "PlayWAV",
            Self::DisplayTextMessage => "DisplayTextMessage",
            Self::DisplayResourceTextMessage => "DisplayResourceTextMessage",
            Self::SetBooleanFlag => "SetBooleanFlag",
            Self::MakeNewTechnologyAvailable => "MakeNewTechnologyAvailable",
            Self::RemoveStructure => "_RemoveStructure",
        }
    }

    /// The species with this name, or `None` for an unknown name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }
}

/// An action with its parameters.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `CPlayBIK`, `AVIFileName` (objective.cpp:1163-1175). The C++ stores
    /// `"./" + name`; the name is kept as written.
    PlayBik { file_name: String },
    /// `CPlayWAV`, `WAVFileName` (objective.cpp:1189-1201). Stored as written.
    PlayWav { file_name: String },
    /// `CDisplayTextMessage`, `Message` (objective.cpp:1215-1223).
    DisplayTextMessage { message: String },
    /// `CDisplayResourceTextMessage`, `ResourceStringID` (objective.cpp:1237-1243).
    DisplayResourceTextMessage { resource_string_id: u32 },
    /// `CSetBooleanFlag`: `FlagID` (or `FlagIndex`), `Value` (objective.cpp:1273-1290).
    /// A missing `Value` keeps the constructor default `true` (objective.h:586).
    SetBooleanFlag { flag_id: String, value: bool },
    /// `CMakeNewTechnologyAvailable`, `PurchaseFileName` (objective.cpp:1304-1313).
    MakeNewTechnologyAvailable { purchase_file_name: String },
    /// `C_RemoveStructure`, `BuildingWID` or position (objective.cpp:1349-1390).
    /// Also the action every nav marker carries (objective.cpp:2347-2351).
    RemoveStructure { structure: ObjectRef },
}

impl Action {
    /// The species of this action.
    pub fn species(&self) -> ActionSpecies {
        match self {
            Self::PlayBik { .. } => ActionSpecies::PlayBik,
            Self::PlayWav { .. } => ActionSpecies::PlayWav,
            Self::DisplayTextMessage { .. } => ActionSpecies::DisplayTextMessage,
            Self::DisplayResourceTextMessage { .. } => ActionSpecies::DisplayResourceTextMessage,
            Self::SetBooleanFlag { .. } => ActionSpecies::SetBooleanFlag,
            Self::MakeNewTechnologyAvailable { .. } => ActionSpecies::MakeNewTechnologyAvailable,
            Self::RemoveStructure { .. } => ActionSpecies::RemoveStructure,
        }
    }
}

/// Reads the parameters for `species` from the selected action block. `Dropped`
/// when a required key is absent (the C++ `Read` returns false).
pub(crate) fn read(fit: &mut FitIni, species: ActionSpecies) -> Result<Read<Action>> {
    let action = match species {
        ActionSpecies::PlayBik => match objective_text(fit, "AVIFileName")? {
            Some(file_name) => Action::PlayBik { file_name },
            None => {
                return Ok(Read::Dropped(
                    "AVIFileName missing (objective.cpp:1167-1168)".into(),
                ))
            }
        },
        ActionSpecies::PlayWav => match objective_text(fit, "WAVFileName")? {
            Some(file_name) => Action::PlayWav { file_name },
            None => {
                return Ok(Read::Dropped(
                    "WAVFileName missing (objective.cpp:1193-1194)".into(),
                ))
            }
        },
        ActionSpecies::DisplayTextMessage => match objective_text(fit, "Message")? {
            Some(message) => Action::DisplayTextMessage { message },
            None => {
                return Ok(Read::Dropped(
                    "Message missing (objective.cpp:1219-1220)".into(),
                ))
            }
        },
        ActionSpecies::DisplayResourceTextMessage => match opt(fit.read_u32("ResourceStringID"))? {
            Some(resource_string_id) => Action::DisplayResourceTextMessage { resource_string_id },
            None => {
                return Ok(Read::Dropped(
                    "ResourceStringID missing (objective.cpp:1240-1241)".into(),
                ))
            }
        },
        ActionSpecies::SetBooleanFlag => {
            let Some(flag_id) = super::condition::flag_id(fit)? else {
                return Ok(Read::Dropped(
                    "FlagID and FlagIndex missing (objective.cpp:1277-1282)".into(),
                ));
            };
            let value = opt(fit.read_bool("Value"))?.unwrap_or(true);
            Action::SetBooleanFlag { flag_id, value }
        }
        ActionSpecies::MakeNewTechnologyAvailable => {
            match objective_text(fit, "PurchaseFileName")? {
                Some(purchase_file_name) => {
                    Action::MakeNewTechnologyAvailable { purchase_file_name }
                }
                None => {
                    return Ok(Read::Dropped(
                        "PurchaseFileName missing (objective.cpp:1308-1309)".into(),
                    ))
                }
            }
        }
        ActionSpecies::RemoveStructure => match object_ref(fit, "BuildingWID")? {
            Some(structure) => Action::RemoveStructure { structure },
            None => {
                return Ok(Read::Dropped(
                    "BuildingWID and PositionX/PositionY both absent (objective.cpp:1352-1373)"
                        .into(),
                ))
            }
        },
    };
    Ok(Read::Loaded(action))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("{text}\n").into_bytes()).expect("fit")
    }

    #[test]
    fn species_names_round_trip() {
        assert_eq!(ActionSpecies::ALL.len(), NUM_ACTION_SPECIES);
        for s in ActionSpecies::ALL {
            assert_eq!(ActionSpecies::from_name(s.name()), Some(s));
        }
        assert_eq!(
            ActionSpecies::from_name("_RemoveStructure"),
            Some(ActionSpecies::RemoveStructure)
        );
        assert_eq!(ActionSpecies::from_name("Bogus"), None);
    }

    #[test]
    fn set_flag_reads_value_and_defaults_true() {
        let mut f = fit("FITini\n[B]\nst FlagID = \"door\"\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ActionSpecies::SetBooleanFlag).unwrap(),
            Read::Loaded(Action::SetBooleanFlag {
                flag_id: "door".into(),
                value: true
            })
        );
    }

    #[test]
    fn text_actions_drop_without_their_key() {
        let mut f = fit("FITini\n[B]\nst Message = \"hi\"\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ActionSpecies::DisplayTextMessage).unwrap(),
            Read::Loaded(Action::DisplayTextMessage {
                message: "hi".into()
            })
        );
        assert!(matches!(
            read(&mut f, ActionSpecies::PlayWav).unwrap(),
            Read::Dropped(_)
        ));
    }

    #[test]
    fn remove_structure_reads_position_fallback() {
        let mut f = fit("FITini\n[B]\nf PositionX = 3\nf PositionY = 4\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ActionSpecies::RemoveStructure).unwrap(),
            Read::Loaded(Action::RemoveStructure {
                structure: ObjectRef::Position { x: 3.0, y: 4.0 }
            })
        );
    }
}
