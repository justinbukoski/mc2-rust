// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Team objectives: `[Objectives Version]`, `[TeamNObjectives]`,
//! `[TeamNObjectiveM]`, its condition, action and failure blocks, and the
//! `[TeamNBooleanFlags]` table.
//!
//! Port of `CObjectives::Read` (code/objective.cpp:2095-2165),
//! `CObjective::Read` (code/objective.cpp:1675-1866), `CBooleanArray::load`
//! (code/objective.cpp:332-362), `ReadNavMarkers` (code/objective.cpp:2311-2356)
//! and `Team::loadObjectives` (code/team.cpp:160-176), which calls
//! `ReadNavMarkers` for each team.
//!
//! The marker text `CObjective::Read` formats (objective.cpp:1705-1716) is read into
//! `Objective::marker_text`; the live status and the `Status` functions are not
//! here. The `IsActive`, `ActivationTime`, `IsResolved`, `ResolvedStatus` and
//! `ChangedStatus` keys are read when present; the shipped missions do not
//! carry them.

use super::action::{self, Action, ActionSpecies};
use super::condition::{self, Area, Condition, ConditionSpecies, ObjectRef};
use super::map::NavMarker;
use super::read::{objective_text, opt, select, Read};
use crate::error::Result;
use mc2_formats::ini::FitIni;

/// Where an objective came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveOrigin {
    /// An `[TeamNObjectiveM]` block; `index` is M.
    Block { index: u32 },
    /// A `[NavMarkerN]` block turned into a hidden trigger; `index` is N.
    NavMarker { index: u32 },
}

/// The 3D model shown for an objective (`ObjectiveModel`, `ModelType`,
/// `BaseColor`, `HighlightColor`, `HighlightColor2`, `ModelScale`).
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveModel {
    pub name: String,
    pub model_type: Option<i32>,
    pub base_color: Option<i32>,
    pub highlight_color: Option<i32>,
    pub highlight_color2: Option<i32>,
    /// `f ModelScale`; `1.0` when absent (objective.cpp:1488).
    pub scale: f32,
}

impl Default for ObjectiveModel {
    fn default() -> Self {
        Self {
            name: String::new(),
            model_type: None,
            base_color: None,
            highlight_color: None,
            highlight_color2: None,
            scale: 1.0,
        }
    }
}

/// One objective with its conditions, actions and failure lists.
#[derive(Debug, Clone, PartialEq)]
pub struct Objective {
    pub origin: ObjectiveOrigin,
    /// `st Title` (objective.cpp:1675-1690), with `\n` written as `\r\n`.
    pub title: String,
    /// `b TitleUseResourceString`, `ul TitleResourceStringID`.
    pub title_use_resource_string: bool,
    pub title_resource_string_id: u32,
    /// `st Description`, and its resource flags.
    pub description: String,
    pub description_use_resource_string: bool,
    pub description_resource_string_id: u32,
    /// `ul Priority`; `1` is a primary objective, `2` secondary (default `1`).
    pub priority: u32,
    /// `ul ResourcePoints` (default `0`).
    pub resource_points: u32,
    /// `b PreviousPrimaryObjectiveMustBeComplete`, falling back to
    /// `PreviousObjectiveMustBeComplete` (objective.cpp:1697-1702).
    pub previous_primary_objective_must_be_complete: bool,
    /// `b AllPreviousPrimaryObjectivesMustBeComplete`.
    pub all_previous_primary_objectives_must_be_complete: bool,
    /// `b DisplayMarker`.
    pub display_marker: bool,
    /// `f MarkerX`, `f MarkerY` (default `0.0`).
    pub marker_x: f32,
    pub marker_y: f32,
    /// `m_markerText` (objective.cpp:1705-1716): the primary number or the secondary letter
    /// printed at the marker on the minimap; empty without `DisplayMarker`.
    pub marker_text: String,
    /// `b HiddenTrigger`.
    pub is_hidden_trigger: bool,
    /// `b ActivateOnFlag`, `st ActivateFlagID` (or `ul ActivateFlagIndex`).
    pub activate_on_flag: bool,
    pub activate_flag_id: String,
    /// `b ResetStatusOnFlag`, `st ResetStatusFlagID`.
    pub reset_status_on_flag: bool,
    pub reset_status_flag_id: String,
    pub model: ObjectiveModel,
    /// Saved-game state. `is_active` starts `true` and is `false` when
    /// `activate_on_flag` is set, before `IsActive` is read (objective.cpp:1697-1722).
    pub is_active: bool,
    /// `f ActivationTime`, default `-1.0`.
    pub activation_time: f32,
    /// `b IsResolved`, default `false`.
    pub resolved: bool,
    /// `l ResolvedStatus` as an `objective_status_type` (`0` undetermined,
    /// `1` successful, `2` failed), default `0`.
    pub resolved_status: i32,
    /// `b ChangedStatus`, default `false`.
    pub changed_status: bool,
    /// `Team{t}Objective{i}Condition{k}` blocks, in file order.
    pub conditions: Vec<Condition>,
    /// `Team{t}Objective{i}Action{k}` blocks.
    pub actions: Vec<Action>,
    /// `Team{t}Objective{i}FailureCondition{k}` blocks.
    pub failure_conditions: Vec<Condition>,
    /// `Team{t}Objective{i}FailureAction{k}` blocks.
    pub failure_actions: Vec<Action>,
}

impl Objective {
    /// An objective with the constructor defaults (objective.cpp:1463-1488).
    fn new(origin: ObjectiveOrigin) -> Self {
        Self {
            origin,
            title: String::new(),
            title_use_resource_string: false,
            title_resource_string_id: 0,
            description: String::new(),
            description_use_resource_string: false,
            description_resource_string_id: 0,
            priority: 1,
            resource_points: 0,
            previous_primary_objective_must_be_complete: false,
            all_previous_primary_objectives_must_be_complete: false,
            display_marker: false,
            marker_x: 0.0,
            marker_y: 0.0,
            marker_text: String::new(),
            is_hidden_trigger: false,
            activate_on_flag: false,
            activate_flag_id: String::new(),
            reset_status_on_flag: false,
            reset_status_flag_id: String::new(),
            model: ObjectiveModel::default(),
            is_active: true,
            activation_time: -1.0,
            resolved: false,
            resolved_status: 0,
            changed_status: false,
            conditions: Vec::new(),
            actions: Vec::new(),
            failure_conditions: Vec::new(),
            failure_actions: Vec::new(),
        }
    }
}

/// A `Team{t}BooleanFlags` entry: a named flag and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BooleanFlag {
    pub flag_id: String,
    pub value: bool,
}

/// One team's objective list. Nav-marker objectives are appended after the
/// block objectives, as `Team::loadObjectives` does (team.cpp:168-172).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TeamObjectives {
    /// The team (alignment) the list belongs to.
    pub team: u32,
    /// `Team{t}BooleanFlags`, in file order. Empty in every shipped mission.
    pub boolean_flags: Vec<BooleanFlag>,
    pub objectives: Vec<Objective>,
}

/// The objective that a nav marker becomes (objective.cpp:2325-2351): a hidden
/// trigger, priority 2, with a move-to-area condition and a remove-structure action.
pub(crate) fn nav_marker_objective(marker: &NavMarker) -> Objective {
    let mut o = Objective::new(ObjectiveOrigin::NavMarker {
        index: marker.index,
    });
    o.title = format!("Nav Marker {}", marker.index);
    o.priority = 2;
    o.is_hidden_trigger = true;
    o.display_marker = false;
    o.marker_x = marker.x;
    o.marker_y = marker.y;
    let area = Area {
        x: marker.x,
        y: marker.y,
        radius: marker.radius,
    };
    o.conditions.push(Condition::MoveAnyUnitToArea { area });
    o.actions.push(Action::RemoveStructure {
        structure: ObjectRef::Position {
            x: marker.x,
            y: marker.y,
        },
    });
    o
}

/// `Team{t}`'s objectives: the block objectives, then the nav markers.
///
/// Missing blocks are not errors, as in the C++: a team without a
/// `Team{t}Objectives` block gets no block objectives. Entries the C++ would
/// delete are dropped and recorded in `warnings`.
pub(crate) fn load_team(
    fit: &mut FitIni,
    file: &str,
    team: u32,
    nav_markers: &[NavMarker],
    warnings: &mut Vec<String>,
) -> Result<TeamObjectives> {
    let mut out = read_block_objectives(fit, file, team, warnings)?;
    for marker in nav_markers {
        out.objectives.push(nav_marker_objective(marker));
    }
    Ok(out)
}

/// The `markerI` and `secondaryMarkers` counters of `CObjectives::Read` (objective.cpp:2139-2157).
struct MarkerCounters {
    primary: u32,
    secondary: char,
}

fn read_block_objectives(
    fit: &mut FitIni,
    file: &str,
    team: u32,
    warnings: &mut Vec<String>,
) -> Result<TeamObjectives> {
    let mut out = TeamObjectives {
        team,
        ..TeamObjectives::default()
    };
    if !select(fit, "Objectives Version")? {
        warnings.push(format!(
            "{file}: [Objectives Version] missing, no objectives"
        ));
        return Ok(out);
    }
    let Some(version) = opt(fit.read_u32("Version"))? else {
        return Ok(out);
    };
    // Version 2 lists live in one block and belong to team 0 only (objective.cpp:2107-2112).
    if version == 2 && team != 0 {
        return Ok(out);
    }
    let list_block = if version == 2 {
        "Objectives".to_string()
    } else {
        format!("Team{team}Objectives")
    };
    if !select(fit, &list_block)? {
        warnings.push(format!("{file}: [{list_block}] missing, no objectives"));
        return Ok(out);
    }
    let Some(count) = opt(fit.read_u32("NumObjectives"))? else {
        return Ok(out);
    };
    out.boolean_flags = load_boolean_flags(fit, team)?;
    let mut markers = MarkerCounters {
        primary: 1,
        secondary: 'a',
    };

    for index in 0..count {
        let block = if version == 2 {
            format!("Objective{index}")
        } else {
            format!("Team{team}Objective{index}")
        };
        if !select(fit, &block)? {
            warnings.push(format!("{file}: [{block}] missing, objective skipped"));
            continue;
        }
        match read_objective(fit, file, team, version, index, &mut markers, warnings)? {
            Read::Loaded(o) => out.objectives.push(o),
            Read::Dropped(why) => warnings.push(format!("{file}: [{block}] dropped: {why}")),
        }
    }
    Ok(out)
}

/// `CBooleanArray::load` (objective.cpp:332-362). Reads `BooleanStringN` and
/// `BooleanValueN` pairs until a `BooleanStringN` is missing.
fn load_boolean_flags(fit: &mut FitIni, team: u32) -> Result<Vec<BooleanFlag>> {
    if !select(fit, &format!("Team{team}BooleanFlags"))? {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut n = 0u32;
    while let Some(flag_id) = objective_text(fit, &format!("BooleanString{n}"))? {
        let value = opt(fit.read_bool(&format!("BooleanValue{n}")))?.unwrap_or(false);
        out.push(BooleanFlag { flag_id, value });
        n += 1;
    }
    Ok(out)
}

/// `CObjective::Read` for the block currently selected. Returns `Dropped` when
/// `NumConditions` is missing (objective.cpp:1760).
fn read_objective(
    fit: &mut FitIni,
    file: &str,
    team: u32,
    version: u32,
    index: u32,
    markers: &mut MarkerCounters,
    warnings: &mut Vec<String>,
) -> Result<Read<Objective>> {
    let mut o = Objective::new(ObjectiveOrigin::Block { index });
    o.title = objective_text(fit, "Title")?.unwrap_or_default();
    o.title_use_resource_string = opt(fit.read_bool("TitleUseResourceString"))?.unwrap_or(false);
    o.title_resource_string_id = opt(fit.read_u32("TitleResourceStringID"))?.unwrap_or(0);
    o.description = objective_text(fit, "Description")?.unwrap_or_default();
    o.description_use_resource_string =
        opt(fit.read_bool("DescriptionUseResourceString"))?.unwrap_or(false);
    o.description_resource_string_id =
        opt(fit.read_u32("DescriptionResourceStringID"))?.unwrap_or(0);
    o.priority = opt(fit.read_u32("Priority"))?.unwrap_or(1);
    o.resource_points = opt(fit.read_u32("ResourcePoints"))?.unwrap_or(0);
    o.previous_primary_objective_must_be_complete =
        match opt(fit.read_bool("PreviousPrimaryObjectiveMustBeComplete"))? {
            Some(v) => v,
            None => opt(fit.read_bool("PreviousObjectiveMustBeComplete"))?.unwrap_or(false),
        };
    o.all_previous_primary_objectives_must_be_complete =
        opt(fit.read_bool("AllPreviousPrimaryObjectivesMustBeComplete"))?.unwrap_or(false);
    // The counter advances for every objective by priority, shown or not, and for objectives
    // dropped later (objective.cpp:2151-2157); `Read` only formats the text (lines 1705-1716).
    let (number, letter) = (markers.primary, markers.secondary);
    if o.priority == 1 {
        markers.primary += 1;
    } else {
        markers.secondary = char::from_u32(u32::from(letter) + 1).unwrap_or(letter);
    }
    o.display_marker = opt(fit.read_bool("DisplayMarker"))?.unwrap_or(false);
    if o.display_marker {
        o.marker_text = if o.priority == 1 {
            number.to_string()
        } else {
            letter.to_string()
        };
    }
    o.marker_x = opt(fit.read_f32("MarkerX"))?.unwrap_or(0.0);
    o.marker_y = opt(fit.read_f32("MarkerY"))?.unwrap_or(0.0);
    o.is_hidden_trigger = opt(fit.read_bool("HiddenTrigger"))?.unwrap_or(false);
    o.activate_on_flag = opt(fit.read_bool("ActivateOnFlag"))?.unwrap_or(false);
    o.activate_flag_id = match objective_text(fit, "ActivateFlagID")? {
        Some(id) => id,
        None => match opt(fit.read_u32("ActivateFlagIndex"))? {
            Some(i) => i.to_string(),
            None => String::new(),
        },
    };
    o.reset_status_on_flag = opt(fit.read_bool("ResetStatusOnFlag"))?.unwrap_or(false);
    o.reset_status_flag_id = objective_text(fit, "ResetStatusFlagID")?.unwrap_or_default();

    o.model = ObjectiveModel {
        name: objective_text(fit, "ObjectiveModel")?.unwrap_or_default(),
        model_type: opt(fit.read_i32("ModelType"))?,
        base_color: opt(fit.read_i32("BaseColor"))?,
        highlight_color: opt(fit.read_i32("HighlightColor"))?,
        highlight_color2: opt(fit.read_i32("HighlightColor2"))?,
        scale: opt(fit.read_f32("ModelScale"))?.unwrap_or(1.0),
    };

    o.is_active = !o.activate_on_flag;
    if let Some(v) = opt(fit.read_bool("IsActive"))? {
        o.is_active = v;
    }
    o.activation_time = opt(fit.read_f32("ActivationTime"))?.unwrap_or(-1.0);
    o.resolved = opt(fit.read_bool("IsResolved"))?.unwrap_or(false);
    o.resolved_status = opt(fit.read_i32("ResolvedStatus"))?.unwrap_or(0);
    o.changed_status = opt(fit.read_bool("ChangedStatus"))?.unwrap_or(false);

    let Some(num_conditions) = opt(fit.read_u32("NumConditions"))? else {
        return Ok(Read::Dropped(
            "NumConditions missing (objective.cpp:1766-1768)".into(),
        ));
    };
    let num_actions = opt(fit.read_u32("NumActions"))?.unwrap_or(0);
    let num_failure_conditions = opt(fit.read_u32("NumFailureConditions"))?.unwrap_or(0);
    let num_failure_actions = opt(fit.read_u32("NumFailureActions"))?.unwrap_or(0);

    let label = format!("{file}: Team{team}Objective{index}");

    for k in 0..num_conditions {
        let block = if version == 2 {
            format!("Objective{index}Condition{k}")
        } else {
            format!("Team{team}Objective{index}Condition{k}")
        };
        if !select(fit, &block)? {
            warnings.push(format!("{file}: [{block}] missing, condition skipped"));
            continue;
        }
        let species = match condition_species(fit, false, &label, warnings)? {
            Read::Loaded(s) => s,
            Read::Dropped(why) => {
                warnings.push(format!("{file}: [{block}] dropped: {why}"));
                continue;
            }
        };
        match condition::read(fit, species)? {
            Read::Loaded(c) => o.conditions.push(c),
            Read::Dropped(why) => warnings.push(format!("{file}: [{block}] dropped: {why}")),
        }
    }

    for k in 0..num_actions {
        let block = format!("Team{team}Objective{index}Action{k}");
        if let Some(a) = read_action_block(fit, file, &block, &label, false, warnings)? {
            o.actions.push(a);
        }
    }

    for k in 0..num_failure_conditions {
        let block = format!("Team{team}Objective{index}FailureCondition{k}");
        if !select(fit, &block)? {
            warnings.push(format!(
                "{file}: [{block}] missing, failure condition skipped"
            ));
            continue;
        }
        let species = match condition_species(fit, true, &label, warnings)? {
            Read::Loaded(s) => s,
            Read::Dropped(why) => {
                warnings.push(format!("{file}: [{block}] dropped: {why}"));
                continue;
            }
        };
        match condition::read(fit, species)? {
            Read::Loaded(c) => o.failure_conditions.push(c),
            Read::Dropped(why) => warnings.push(format!("{file}: [{block}] dropped: {why}")),
        }
    }

    for k in 0..num_failure_actions {
        let block = format!("Team{team}Objective{index}FailureAction{k}");
        if let Some(a) = read_action_block(fit, file, &block, &label, true, warnings)? {
            o.failure_actions.push(a);
        }
    }

    Ok(Read::Loaded(o))
}

/// Species for a condition block (objective.cpp:1788-1800, 1831-1841). An
/// unknown name is read as `DestroyAllEnemyUnits`, which the C++ does too
/// (objective.cpp:1649-1659), and is reported here.
fn condition_species(
    fit: &FitIni,
    failure: bool,
    label: &str,
    warnings: &mut Vec<String>,
) -> Result<Read<ConditionSpecies>> {
    let key = if failure {
        "FailureConditionSpeciesString"
    } else {
        "ConditionSpeciesString"
    };
    if let Some(name) = objective_text(fit, key)? {
        return Ok(Read::Loaded(match ConditionSpecies::from_name(&name) {
            Some(s) => s,
            None => {
                warnings.push(format!(
                    "{label}: unknown condition species \"{name}\" read as DestroyAllEnemyUnits (objective.cpp:1649-1659)"
                ));
                ConditionSpecies::DestroyAllEnemyUnits
            }
        }));
    }
    if failure {
        return Ok(Read::Dropped(
            "FailureConditionSpeciesString missing (objective.cpp:1835-1837)".into(),
        ));
    }
    match opt(fit.read_u32("ConditionSpecies"))? {
        Some(i) => Ok(match ConditionSpecies::from_index(i) {
            Some(s) => Read::Loaded(s),
            None => Read::Dropped(format!(
                "ConditionSpecies {i} out of range (objective.cpp:1415-1418)"
            )),
        }),
        None => Ok(Read::Dropped(
            "no species key (objective.cpp:1787-1800)".into(),
        )),
    }
}

/// Reads one action or failure-action block; `None` when it is absent or dropped.
fn read_action_block(
    fit: &mut FitIni,
    file: &str,
    block: &str,
    label: &str,
    failure: bool,
    warnings: &mut Vec<String>,
) -> Result<Option<Action>> {
    if !select(fit, block)? {
        warnings.push(format!("{file}: [{block}] missing, action skipped"));
        return Ok(None);
    }
    let key = if failure {
        "FailureActionSpeciesString"
    } else {
        "ActionSpeciesString"
    };
    let species = match objective_text(fit, key)? {
        Some(name) => match ActionSpecies::from_name(&name) {
            Some(s) => s,
            None => {
                warnings.push(format!(
                    "{label}: unknown action species \"{name}\" read as PlayBIK (objective.cpp:1662-1672)"
                ));
                ActionSpecies::PlayBik
            }
        },
        None => {
            warnings.push(format!(
                "{file}: [{block}] dropped: {key} missing (objective.cpp:1815-1817)"
            ));
            return Ok(None);
        }
    };
    match action::read(fit, species)? {
        Read::Loaded(a) => Ok(Some(a)),
        Read::Dropped(why) => {
            warnings.push(format!("{file}: [{block}] dropped: {why}"));
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("{text}\n").into_bytes()).expect("fit")
    }

    const SAMPLE: &str = "FITini\n[Objectives Version]\nul Version = 3\n\
        [Team0Objectives]\nul NumObjectives = 1\n\
        [Team0Objective0]\nst Title = \"Kill them\"\nul Priority = 1\nul ResourcePoints = 50\n\
        b HiddenTrigger = FALSE\nul NumConditions = 2\nul NumActions = 1\nul NumFailureConditions = 1\n\
        ul NumFailureActions = 0\n\
        [Team0Objective0Condition0]\nst ConditionSpeciesString = \"DestroyNumberOfEnemyUnits\"\nul Num = 2\n\
        [Team0Objective0Condition1]\nst ConditionSpeciesString = \"ElapsedMissionTime\"\nf Time = 30\n\
        [Team0Objective0Action0]\nst ActionSpeciesString = \"SetBooleanFlag\"\nst FlagID = \"done\"\nb Value = TRUE\n\
        [Team0Objective0FailureCondition0]\nst FailureConditionSpeciesString = \"BooleanFlagIsSet\"\nst FlagID = \"lost\"\n";

    #[test]
    fn team_objective_block_loads_all_lists() {
        let mut f = fit(SAMPLE);
        let mut warnings = Vec::new();
        let t = load_team(&mut f, "t.fit", 0, &[], &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(t.objectives.len(), 1);
        let o = &t.objectives[0];
        assert_eq!(o.origin, ObjectiveOrigin::Block { index: 0 });
        assert_eq!(o.title, "Kill them");
        assert_eq!(o.resource_points, 50);
        assert!(o.is_active);
        assert_eq!(o.activation_time, -1.0);
        assert_eq!(o.conditions.len(), 2);
        assert_eq!(
            o.conditions[0],
            Condition::DestroyNumberOfEnemyUnits { num: 2 }
        );
        assert_eq!(
            o.conditions[1],
            Condition::ElapsedMissionTime { time: 30.0 }
        );
        assert_eq!(
            o.actions[0],
            Action::SetBooleanFlag {
                flag_id: "done".into(),
                value: true
            }
        );
        assert_eq!(o.failure_conditions.len(), 1);
        assert_eq!(
            o.failure_conditions[0],
            Condition::BooleanFlagIsSet {
                flag_id: "lost".into(),
                value: true
            }
        );
    }

    /// objective.cpp:1705-1716, 2139-2157: primaries are numbered from 1 and secondaries
    /// lettered from `a`, both counted whether or not the marker is shown; no marker, no text.
    #[test]
    fn marker_text_numbers_primaries_and_letters_secondaries() {
        let obj = |i: u32, priority: u32, marker: bool| {
            format!(
                "[Team0Objective{i}]\nst Title = \"o{i}\"\nul Priority = {priority}\nb DisplayMarker = {}\n\
                 f MarkerX = 1\nf MarkerY = 2\nul NumConditions = 0\nul NumActions = 0\n\
                 ul NumFailureConditions = 0\nul NumFailureActions = 0\n",
                if marker { "TRUE" } else { "FALSE" }
            )
        };
        let text = format!(
            "FITini\n[Objectives Version]\nul Version = 3\n[Team0Objectives]\nul NumObjectives = 5\n{}{}{}{}{}",
            obj(0, 1, true),
            obj(1, 2, true),
            obj(2, 1, false),
            obj(3, 1, true),
            obj(4, 2, true),
        );
        let mut f = fit(&text);
        let t = load_team(&mut f, "t.fit", 0, &[], &mut Vec::new()).unwrap();
        let marks: Vec<&str> = t
            .objectives
            .iter()
            .map(|o| o.marker_text.as_str())
            .collect();
        assert_eq!(marks, ["1", "a", "", "3", "b"]);
    }

    #[test]
    fn nav_markers_append_as_hidden_triggers() {
        let mut f = fit("FITini\n[Objectives Version]\nul Version = 3\n");
        let markers = [NavMarker {
            index: 4,
            x: 10.0,
            y: 20.0,
            radius: 5.0,
        }];
        let t = load_team(&mut f, "t.fit", 1, &markers, &mut Vec::new()).unwrap();
        assert_eq!(t.team, 1);
        assert_eq!(t.objectives.len(), 1);
        let o = &t.objectives[0];
        assert_eq!(o.origin, ObjectiveOrigin::NavMarker { index: 4 });
        assert_eq!(o.title, "Nav Marker 4");
        assert_eq!(o.priority, 2);
        assert!(o.is_hidden_trigger);
        assert_eq!(
            o.conditions[0],
            Condition::MoveAnyUnitToArea {
                area: Area {
                    x: 10.0,
                    y: 20.0,
                    radius: 5.0
                }
            }
        );
    }

    #[test]
    fn unknown_species_name_falls_back_with_warning() {
        let text = SAMPLE.replace("DestroyNumberOfEnemyUnits", "NoSuchSpecies");
        let mut f = fit(&text);
        let mut warnings = Vec::new();
        let t = load_team(&mut f, "t.fit", 0, &[], &mut warnings).unwrap();
        assert_eq!(
            t.objectives[0].conditions[0],
            Condition::DestroyAllEnemyUnits
        );
        assert!(warnings.iter().any(|w| w.contains("NoSuchSpecies")));
    }

    #[test]
    fn missing_num_conditions_drops_objective() {
        let text = "FITini\n[Objectives Version]\nul Version = 3\n[Team0Objectives]\nul NumObjectives = 1\n\
            [Team0Objective0]\nst Title = \"x\"\n";
        let mut f = fit(text);
        let mut warnings = Vec::new();
        let t = load_team(&mut f, "t.fit", 0, &[], &mut warnings).unwrap();
        assert!(t.objectives.is_empty());
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn boolean_flags_read_until_first_missing_string() {
        let text = "FITini\n[Objectives Version]\nul Version = 3\n[Team2Objectives]\nul NumObjectives = 0\n[Team2BooleanFlags]\n\
            st BooleanString0 = \"a\"\nb BooleanValue0 = TRUE\nst BooleanString1 = \"b\"\n";
        let mut f = fit(text);
        let t = load_team(&mut f, "t.fit", 2, &[], &mut Vec::new()).unwrap();
        assert_eq!(
            t.boolean_flags,
            vec![
                BooleanFlag {
                    flag_id: "a".into(),
                    value: true
                },
                BooleanFlag {
                    flag_id: "b".into(),
                    value: false
                },
            ]
        );
    }
}
