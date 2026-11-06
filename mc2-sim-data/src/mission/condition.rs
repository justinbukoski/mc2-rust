// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Objective conditions: the 24 `condition_species_type` values and the typed
//! parameters each species reads.
//!
//! Enum and names: code/objective.h:33-65 and code/objective.cpp:71-101.
//! Construction: `CObjective::new_CObjectiveCondition` (code/objective.cpp:1391-1421).
//! Parameter reads: the `Read` methods in code/objective.cpp (line refs on each
//! variant).

use super::read::{objective_text, opt, Read};
use crate::error::Result;
use mc2_formats::ini::FitIni;

/// `condition_species_type` (objective.h:33-65). Discriminants follow the C++
/// enum order, which is also the index into the name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConditionSpecies {
    DestroyAllEnemyUnits = 0,
    DestroyNumberOfEnemyUnits = 1,
    DestroyEnemyUnitGroup = 2,
    DestroySpecificEnemyUnit = 3,
    DestroySpecificStructure = 4,
    CaptureOrDestroyAllEnemyUnits = 5,
    CaptureOrDestroyNumberOfEnemyUnits = 6,
    CaptureOrDestroyEnemyUnitGroup = 7,
    CaptureOrDestroySpecificEnemyUnit = 8,
    CaptureOrDestroySpecificStructure = 9,
    DeadOrFledAllEnemyUnits = 10,
    DeadOrFledNumberOfEnemyUnits = 11,
    DeadOrFledEnemyUnitGroup = 12,
    DeadOrFledSpecificEnemyUnit = 13,
    CaptureUnit = 14,
    CaptureStructure = 15,
    GuardSpecificUnit = 16,
    GuardSpecificStructure = 17,
    MoveAnyUnitToArea = 18,
    MoveAllUnitsToArea = 19,
    MoveAllSurvivingUnitsToArea = 20,
    MoveAllSurvivingMechsToArea = 21,
    BooleanFlagIsSet = 22,
    ElapsedMissionTime = 23,
}

/// Number of condition species (objective.h:65, `NUM_CONDITION_SPECIES`).
pub const NUM_CONDITION_SPECIES: usize = 24;

impl ConditionSpecies {
    /// Every species in enum order.
    pub const ALL: [ConditionSpecies; NUM_CONDITION_SPECIES] = [
        Self::DestroyAllEnemyUnits,
        Self::DestroyNumberOfEnemyUnits,
        Self::DestroyEnemyUnitGroup,
        Self::DestroySpecificEnemyUnit,
        Self::DestroySpecificStructure,
        Self::CaptureOrDestroyAllEnemyUnits,
        Self::CaptureOrDestroyNumberOfEnemyUnits,
        Self::CaptureOrDestroyEnemyUnitGroup,
        Self::CaptureOrDestroySpecificEnemyUnit,
        Self::CaptureOrDestroySpecificStructure,
        Self::DeadOrFledAllEnemyUnits,
        Self::DeadOrFledNumberOfEnemyUnits,
        Self::DeadOrFledEnemyUnitGroup,
        Self::DeadOrFledSpecificEnemyUnit,
        Self::CaptureUnit,
        Self::CaptureStructure,
        Self::GuardSpecificUnit,
        Self::GuardSpecificStructure,
        Self::MoveAnyUnitToArea,
        Self::MoveAllUnitsToArea,
        Self::MoveAllSurvivingUnitsToArea,
        Self::MoveAllSurvivingMechsToArea,
        Self::BooleanFlagIsSet,
        Self::ElapsedMissionTime,
    ];

    /// The name the data stores in `ConditionSpeciesString` (objective.cpp:71-101).
    pub fn name(self) -> &'static str {
        match self {
            Self::DestroyAllEnemyUnits => "DestroyAllEnemyUnits",
            Self::DestroyNumberOfEnemyUnits => "DestroyNumberOfEnemyUnits",
            Self::DestroyEnemyUnitGroup => "DestroyEnemyUnitGroup",
            Self::DestroySpecificEnemyUnit => "DestroySpecificEnemyUnit",
            Self::DestroySpecificStructure => "DestroySpecificStructure",
            Self::CaptureOrDestroyAllEnemyUnits => "CaptureOrDestroyAllEnemyUnits",
            Self::CaptureOrDestroyNumberOfEnemyUnits => "CaptureOrDestroyNumberOfEnemyUnits",
            Self::CaptureOrDestroyEnemyUnitGroup => "CaptureOrDestroyEnemyUnitGroup",
            Self::CaptureOrDestroySpecificEnemyUnit => "CaptureOrDestroySpecificEnemyUnit",
            Self::CaptureOrDestroySpecificStructure => "CaptureOrDestroySpecificStructure",
            Self::DeadOrFledAllEnemyUnits => "DeadOrFledAllEnemyUnits",
            Self::DeadOrFledNumberOfEnemyUnits => "DeadOrFledNumberOfEnemyUnits",
            Self::DeadOrFledEnemyUnitGroup => "DeadOrFledEnemyUnitGroup",
            Self::DeadOrFledSpecificEnemyUnit => "DeadOrFledSpecificEnemyUnit",
            Self::CaptureUnit => "CaptureSpecificUnit",
            Self::CaptureStructure => "CaptureSpecificStructure",
            Self::GuardSpecificUnit => "GuardSpecificUnit",
            Self::GuardSpecificStructure => "GuardSpecificStructure",
            Self::MoveAnyUnitToArea => "MoveAnyUnitToArea",
            Self::MoveAllUnitsToArea => "MoveAllUnitsToArea",
            Self::MoveAllSurvivingUnitsToArea => "MoveAllSurvivingUnitsToArea",
            Self::MoveAllSurvivingMechsToArea => "MoveAllSurvivingMechsToArea",
            Self::BooleanFlagIsSet => "BooleanFlagIsSet",
            Self::ElapsedMissionTime => "ElapsedMissionTime",
        }
    }

    /// The species with this name, or `None` for an unknown name.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.name() == name)
    }

    /// The species for a raw enum value (the deprecated `ConditionSpecies` key).
    pub fn from_index(index: u32) -> Option<Self> {
        Self::ALL.get(index as usize).copied()
    }
}

/// A position or a watch ID that names a unit or structure.
///
/// The C++ stores a runtime watch ID (`MoverWID`, `BuildingWID`) and, when the
/// key is absent, resolves a saved position against the live object list
/// (objective.cpp:422-476). The watch ID is only meaningful in a running game,
/// so this model keeps the value as written. Resolving a `Position` belongs to
/// the simulation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ObjectRef {
    /// `l` watch ID from the file.
    Wid(i32),
    /// `f PositionX`, `f PositionY`.
    Position { x: f32, y: f32 },
}

/// A circular target area (`CAreaObjectiveCondition`, objective.cpp:868-878).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
}

/// A condition with its parameters.
///
/// Variants without a parameter list read nothing beyond the species key.
#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    /// `CDestroyAllEnemyUnits` (no `Read` override; objective.h:160).
    DestroyAllEnemyUnits,
    /// `CDestroyNumberOfEnemyUnits`, `Num` (objective.cpp:379-385).
    DestroyNumberOfEnemyUnits { num: u32 },
    /// `CDestroySpecificEnemyUnit`, `MoverWID` or position (objective.cpp:422-455).
    DestroySpecificEnemyUnit { unit: ObjectRef },
    /// `CDestroySpecificStructure`, `BuildingWID` or position (objective.cpp:478-511).
    DestroySpecificStructure { structure: ObjectRef },
    /// `CCaptureOrDestroyAllEnemyUnits` (no `Read` override).
    CaptureOrDestroyAllEnemyUnits,
    /// `CCaptureOrDestroyNumberOfEnemyUnits`, `Num`.
    CaptureOrDestroyNumberOfEnemyUnits { num: u32 },
    /// `CCaptureOrDestroySpecificEnemyUnit` (objective.cpp:557-589).
    CaptureOrDestroySpecificEnemyUnit { unit: ObjectRef },
    /// `CCaptureOrDestroySpecificStructure` (objective.cpp:602-634).
    CaptureOrDestroySpecificStructure { structure: ObjectRef },
    /// `CDeadOrFledAllEnemyUnits` (no `Read` override).
    DeadOrFledAllEnemyUnits,
    /// `CDeadOrFledNumberOfEnemyUnits`, `Num`.
    DeadOrFledNumberOfEnemyUnits { num: u32 },
    /// `CDeadOrFledSpecificEnemyUnit`; inherits the `MoverWID` reader (objective.cpp:422).
    DeadOrFledSpecificEnemyUnit { unit: ObjectRef },
    /// `CCaptureUnit` (objective.cpp:696-728).
    CaptureUnit { unit: ObjectRef },
    /// `CCaptureStructure` (objective.cpp:748-782).
    CaptureStructure { structure: ObjectRef },
    /// `CGuardSpecificUnit` (objective.cpp:801-832).
    GuardSpecificUnit { unit: ObjectRef },
    /// `CGuardSpecificStructure`; inherits the structure reader (objective.cpp:478).
    GuardSpecificStructure { structure: ObjectRef },
    /// `CMoveAnyUnitToArea` (objective.cpp:868-878).
    MoveAnyUnitToArea { area: Area },
    /// `CMoveAllUnitsToArea` (objective.cpp:868-878).
    MoveAllUnitsToArea { area: Area },
    /// `CMoveAllSurvivingUnitsToArea` (objective.cpp:868-878).
    MoveAllSurvivingUnitsToArea { area: Area },
    /// `CMoveAllSurvivingMechsToArea` (objective.cpp:868-878).
    MoveAllSurvivingMechsToArea { area: Area },
    /// `CBooleanFlagIsSet`: `FlagID` (or `FlagIndex`), `Value` (objective.cpp:1109-1125).
    /// A missing `Value` keeps the constructor default `true` (objective.h:446).
    BooleanFlagIsSet { flag_id: String, value: bool },
    /// `CElapsedMissionTime`, `Time` (objective.cpp:1142-1148).
    ElapsedMissionTime { time: f32 },
}

impl Condition {
    /// The species of this condition.
    pub fn species(&self) -> ConditionSpecies {
        match self {
            Self::DestroyAllEnemyUnits => ConditionSpecies::DestroyAllEnemyUnits,
            Self::DestroyNumberOfEnemyUnits { .. } => ConditionSpecies::DestroyNumberOfEnemyUnits,
            Self::DestroySpecificEnemyUnit { .. } => ConditionSpecies::DestroySpecificEnemyUnit,
            Self::DestroySpecificStructure { .. } => ConditionSpecies::DestroySpecificStructure,
            Self::CaptureOrDestroyAllEnemyUnits => ConditionSpecies::CaptureOrDestroyAllEnemyUnits,
            Self::CaptureOrDestroyNumberOfEnemyUnits { .. } => {
                ConditionSpecies::CaptureOrDestroyNumberOfEnemyUnits
            }
            Self::CaptureOrDestroySpecificEnemyUnit { .. } => {
                ConditionSpecies::CaptureOrDestroySpecificEnemyUnit
            }
            Self::CaptureOrDestroySpecificStructure { .. } => {
                ConditionSpecies::CaptureOrDestroySpecificStructure
            }
            Self::DeadOrFledAllEnemyUnits => ConditionSpecies::DeadOrFledAllEnemyUnits,
            Self::DeadOrFledNumberOfEnemyUnits { .. } => {
                ConditionSpecies::DeadOrFledNumberOfEnemyUnits
            }
            Self::DeadOrFledSpecificEnemyUnit { .. } => {
                ConditionSpecies::DeadOrFledSpecificEnemyUnit
            }
            Self::CaptureUnit { .. } => ConditionSpecies::CaptureUnit,
            Self::CaptureStructure { .. } => ConditionSpecies::CaptureStructure,
            Self::GuardSpecificUnit { .. } => ConditionSpecies::GuardSpecificUnit,
            Self::GuardSpecificStructure { .. } => ConditionSpecies::GuardSpecificStructure,
            Self::MoveAnyUnitToArea { .. } => ConditionSpecies::MoveAnyUnitToArea,
            Self::MoveAllUnitsToArea { .. } => ConditionSpecies::MoveAllUnitsToArea,
            Self::MoveAllSurvivingUnitsToArea { .. } => {
                ConditionSpecies::MoveAllSurvivingUnitsToArea
            }
            Self::MoveAllSurvivingMechsToArea { .. } => {
                ConditionSpecies::MoveAllSurvivingMechsToArea
            }
            Self::BooleanFlagIsSet { .. } => ConditionSpecies::BooleanFlagIsSet,
            Self::ElapsedMissionTime { .. } => ConditionSpecies::ElapsedMissionTime,
        }
    }
}

/// Reads an `ObjectRef` from `wid_key`, else from `PositionX`/`PositionY`.
/// `None` when neither form is present (the C++ returns false).
pub(crate) fn object_ref(fit: &FitIni, wid_key: &str) -> Result<Option<ObjectRef>> {
    if let Some(wid) = opt(fit.read_i32(wid_key))? {
        return Ok(Some(ObjectRef::Wid(wid)));
    }
    let x = opt(fit.read_f32("PositionX"))?;
    let y = opt(fit.read_f32("PositionY"))?;
    Ok(match (x, y) {
        (Some(x), Some(y)) => Some(ObjectRef::Position { x, y }),
        _ => None,
    })
}

/// Reads the parameters for `species` from the selected condition block.
/// `Dropped` when a required key is absent, as the C++ `Read` returns false.
pub(crate) fn read(fit: &mut FitIni, species: ConditionSpecies) -> Result<Read<Condition>> {
    let dropped = |why: &str| Ok(Read::Dropped(why.to_string()));
    let condition = match species {
        ConditionSpecies::DestroyAllEnemyUnits => Condition::DestroyAllEnemyUnits,
        ConditionSpecies::CaptureOrDestroyAllEnemyUnits => Condition::CaptureOrDestroyAllEnemyUnits,
        ConditionSpecies::DeadOrFledAllEnemyUnits => Condition::DeadOrFledAllEnemyUnits,
        ConditionSpecies::DestroyNumberOfEnemyUnits => {
            let Some(num) = opt(fit.read_u32("Num"))? else {
                return dropped("Num missing (objective.cpp:381)");
            };
            Condition::DestroyNumberOfEnemyUnits { num }
        }
        ConditionSpecies::CaptureOrDestroyNumberOfEnemyUnits => {
            let Some(num) = opt(fit.read_u32("Num"))? else {
                return dropped("Num missing (objective.cpp:381)");
            };
            Condition::CaptureOrDestroyNumberOfEnemyUnits { num }
        }
        ConditionSpecies::DeadOrFledNumberOfEnemyUnits => {
            let Some(num) = opt(fit.read_u32("Num"))? else {
                return dropped("Num missing (objective.cpp:381)");
            };
            Condition::DeadOrFledNumberOfEnemyUnits { num }
        }
        ConditionSpecies::DestroySpecificEnemyUnit
        | ConditionSpecies::CaptureOrDestroySpecificEnemyUnit
        | ConditionSpecies::DeadOrFledSpecificEnemyUnit
        | ConditionSpecies::CaptureUnit
        | ConditionSpecies::GuardSpecificUnit => {
            let Some(unit) = object_ref(fit, "MoverWID")? else {
                return dropped("MoverWID and PositionX/PositionY both absent");
            };
            match species {
                ConditionSpecies::DestroySpecificEnemyUnit => {
                    Condition::DestroySpecificEnemyUnit { unit }
                }
                ConditionSpecies::CaptureOrDestroySpecificEnemyUnit => {
                    Condition::CaptureOrDestroySpecificEnemyUnit { unit }
                }
                ConditionSpecies::DeadOrFledSpecificEnemyUnit => {
                    Condition::DeadOrFledSpecificEnemyUnit { unit }
                }
                ConditionSpecies::CaptureUnit => Condition::CaptureUnit { unit },
                _ => Condition::GuardSpecificUnit { unit },
            }
        }
        ConditionSpecies::DestroySpecificStructure
        | ConditionSpecies::CaptureOrDestroySpecificStructure
        | ConditionSpecies::CaptureStructure
        | ConditionSpecies::GuardSpecificStructure => {
            let Some(structure) = object_ref(fit, "BuildingWID")? else {
                return dropped("BuildingWID and PositionX/PositionY both absent");
            };
            match species {
                ConditionSpecies::DestroySpecificStructure => {
                    Condition::DestroySpecificStructure { structure }
                }
                ConditionSpecies::CaptureOrDestroySpecificStructure => {
                    Condition::CaptureOrDestroySpecificStructure { structure }
                }
                ConditionSpecies::CaptureStructure => Condition::CaptureStructure { structure },
                _ => Condition::GuardSpecificStructure { structure },
            }
        }
        // No class in objective.h defines these; new_CObjectiveCondition hits its
        // default branch, asserts, and returns null (objective.cpp:1415-1418).
        ConditionSpecies::DestroyEnemyUnitGroup
        | ConditionSpecies::CaptureOrDestroyEnemyUnitGroup
        | ConditionSpecies::DeadOrFledEnemyUnitGroup => {
            return dropped("no condition class for this species (objective.cpp:1415-1418)");
        }
        ConditionSpecies::MoveAnyUnitToArea
        | ConditionSpecies::MoveAllUnitsToArea
        | ConditionSpecies::MoveAllSurvivingUnitsToArea
        | ConditionSpecies::MoveAllSurvivingMechsToArea => {
            let (Some(x), Some(y), Some(radius)) = (
                opt(fit.read_f32("TargetCenterX"))?,
                opt(fit.read_f32("TargetCenterY"))?,
                opt(fit.read_f32("TargetRadius"))?,
            ) else {
                return dropped("TargetCenterX/Y or TargetRadius missing (objective.cpp:868-876)");
            };
            let area = Area { x, y, radius };
            match species {
                ConditionSpecies::MoveAnyUnitToArea => Condition::MoveAnyUnitToArea { area },
                ConditionSpecies::MoveAllUnitsToArea => Condition::MoveAllUnitsToArea { area },
                ConditionSpecies::MoveAllSurvivingUnitsToArea => {
                    Condition::MoveAllSurvivingUnitsToArea { area }
                }
                _ => Condition::MoveAllSurvivingMechsToArea { area },
            }
        }
        ConditionSpecies::BooleanFlagIsSet => {
            let Some(flag_id) = flag_id(fit)? else {
                return dropped("FlagID and FlagIndex missing (objective.cpp:1113-1118)");
            };
            let value = opt(fit.read_bool("Value"))?.unwrap_or(true);
            Condition::BooleanFlagIsSet { flag_id, value }
        }
        ConditionSpecies::ElapsedMissionTime => {
            let Some(time) = opt(fit.read_f32("Time"))? else {
                return dropped("Time missing (objective.cpp:1145-1146)");
            };
            Condition::ElapsedMissionTime { time }
        }
    };
    Ok(Read::Loaded(condition))
}

/// `FlagID`, or the older `FlagIndex` formatted as `%d` (objective.cpp:1113-1122).
pub(crate) fn flag_id(fit: &FitIni) -> Result<Option<String>> {
    if let Some(id) = objective_text(fit, "FlagID")? {
        return Ok(Some(id));
    }
    Ok(opt(fit.read_u32("FlagIndex"))?.map(|i| i.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("{text}\n").into_bytes()).expect("fit")
    }

    #[test]
    fn species_names_and_indices_round_trip() {
        assert_eq!(ConditionSpecies::ALL.len(), NUM_CONDITION_SPECIES);
        for (i, s) in ConditionSpecies::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
            assert_eq!(ConditionSpecies::from_name(s.name()), Some(*s));
            assert_eq!(ConditionSpecies::from_index(i as u32), Some(*s));
        }
        assert_eq!(
            ConditionSpecies::from_name("CaptureSpecificUnit"),
            Some(ConditionSpecies::CaptureUnit)
        );
        assert_eq!(ConditionSpecies::from_name("Nope"), None);
        assert_eq!(ConditionSpecies::from_index(24), None);
    }

    #[test]
    fn number_and_time_conditions_read_their_keys() {
        let mut f = fit("FITini\n[B]\nul Num = 3\nf Time = 12.5\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ConditionSpecies::DestroyNumberOfEnemyUnits).unwrap(),
            Read::Loaded(Condition::DestroyNumberOfEnemyUnits { num: 3 })
        );
        assert_eq!(
            read(&mut f, ConditionSpecies::ElapsedMissionTime).unwrap(),
            Read::Loaded(Condition::ElapsedMissionTime { time: 12.5 })
        );
    }

    #[test]
    fn unit_reference_prefers_watch_id_then_position() {
        let mut f = fit("FITini\n[B]\nl MoverWID = 7\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ConditionSpecies::DestroySpecificEnemyUnit).unwrap(),
            Read::Loaded(Condition::DestroySpecificEnemyUnit {
                unit: ObjectRef::Wid(7)
            })
        );
        let mut f = fit("FITini\n[B]\nf PositionX = 1\nf PositionY = 2\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ConditionSpecies::CaptureUnit).unwrap(),
            Read::Loaded(Condition::CaptureUnit {
                unit: ObjectRef::Position { x: 1.0, y: 2.0 }
            })
        );
    }

    #[test]
    fn group_species_and_missing_keys_are_dropped() {
        let mut f = fit("FITini\n[B]\nf Time = 1\n");
        f.seek_block("B").unwrap();
        assert!(matches!(
            read(&mut f, ConditionSpecies::DestroyEnemyUnitGroup).unwrap(),
            Read::Dropped(_)
        ));
        assert!(matches!(
            read(&mut f, ConditionSpecies::MoveAnyUnitToArea).unwrap(),
            Read::Dropped(_)
        ));
    }

    #[test]
    fn boolean_flag_defaults_value_to_true() {
        let mut f = fit("FITini\n[B]\nst FlagID = \"gate\"\n");
        f.seek_block("B").unwrap();
        assert_eq!(
            read(&mut f, ConditionSpecies::BooleanFlagIsSet).unwrap(),
            Read::Loaded(Condition::BooleanFlagIsSet {
                flag_id: "gate".to_string(),
                value: true
            })
        );
    }
}
