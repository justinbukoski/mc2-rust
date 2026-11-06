// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Objective evaluation, single player: a port of the runtime half of `code/objective.cpp`.
//!
//! The data model and loaders live in `mc2_sim_data::mission::objective`. This module evaluates them every frame:
//! the 24 condition `Status()` functions, `CObjective::Status()` (sticky result, one-shot actions, Betty sample),
//! `CObjective::Status(CObjectives&)`, the status setter, `StatusChangedSuccess/Failed`, `CObjectives::Status()`
//! (reset flags, activation, previous-primary qualifiers), and the team boolean flags (`CBooleanArray`).
//!
//! The simulation is reached only through [`ObjectiveWorld`], so this module does not depend on `World`. Multiplayer
//! branches (`if (MPlayer)`) are out of scope; only the single-player branch is ported.
//!
//! Deliberate deviation (BUG-SWEEP `units-combat-054`..`-059`): where the C++ dereferences a watch-id lookup without
//! checking it, a missing object here gives [`ObjectiveStatus::Undetermined`] instead of a crash. `CCaptureUnit`
//! keeps its explicit C++ null branch (`Failed`).

use mc2_sim_data::mission::{Action, BooleanFlag, Condition, ObjectRef, Objective, TeamObjectives};

pub use super::objective::ObjectiveStatus;

/// `WARRIOR_STATUS_*` values the objective code distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotStatus {
    /// Any status not listed below.
    Normal,
    /// `WARRIOR_STATUS_DEAD`.
    Dead,
    /// `WARRIOR_STATUS_EJECTED`.
    Ejected,
    /// `WARRIOR_STATUS_WITHDRAWN`.
    Withdrawn,
}

/// What the objective code reads from one mover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverView {
    /// `getTeamId()`.
    pub team_id: i32,
    /// `getCommanderId()`.
    pub commander_id: i32,
    /// `getPosition().x`.
    pub x: f32,
    /// `getPosition().y`.
    pub y: f32,
    /// `isDestroyed()`.
    pub destroyed: bool,
    /// `isDisabled()`.
    pub disabled: bool,
    /// `getPilot()->getStatus()`.
    pub pilot_status: PilotStatus,
    /// `isOnGUI()`.
    pub on_gui: bool,
    /// `isRecover()`.
    pub recover: bool,
    /// `isMech()`.
    pub is_mech: bool,
    /// `getMoveType() == MOVETYPE_AIR`.
    pub air: bool,
}

/// What the objective code reads from one building.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildingView {
    /// `getTeamId()`.
    pub team_id: i32,
    /// `isDestroyed()`.
    pub destroyed: bool,
    /// `getCaptured()`.
    pub captured: bool,
}

/// Betty voice samples played when a visible objective resolves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Betty {
    /// `BETTY_OBJECTIVE_COMPLETE`.
    ObjectiveComplete,
    /// `BETTY_CANNOT_COMP_OBJ`.
    CannotCompleteObjective,
}

/// The simulation as the objective code sees it.
pub trait ObjectiveWorld {
    /// `ObjectManager->getNumMovers()`.
    fn num_movers(&self) -> usize;
    /// `ObjectManager->getMover(i)`, `i < num_movers()`.
    fn mover(&self, index: usize) -> MoverView;
    /// `ObjectManager->getByWatchID(...)` for a mover reference; `None` when nothing matches.
    fn mover_by_ref(&self, r: &ObjectRef) -> Option<MoverView>;
    /// `ObjectManager->getByWatchID(...)` for a building reference; `None` when nothing matches.
    fn building_by_ref(&self, r: &ObjectRef) -> Option<BuildingView>;
    /// `Commander::home->getId()`.
    fn home_commander_id(&self) -> i32;
    /// `mission->actualTime`.
    fn actual_time(&self) -> f32;
    /// `mission->m_timeLimit` (`-1.0` when there is none).
    fn time_limit(&self) -> f32;
    /// `MaxExtractUnitDistance` (gamesys `MaxUnitExtractDistance`, default 1280).
    fn max_extract_unit_distance(&self) -> f32;
    /// `soundSystem->playBettySample(...)`.
    fn play_betty(&mut self, sample: Betty);
    /// `mission->missionInterface->showObjectives(true)`.
    fn show_objectives(&mut self);
    /// Executes one action for `team`. Called for every action except [`Action::SetBooleanFlag`], which this
    /// module applies to the team's own flags itself (objective.cpp:1297-1302) without calling this.
    fn execute_action(&mut self, team: i32, action: &Action);
}

/// The status as stored in `Objective::resolved_status` (`objective_status_type`: 0, 1, 2; objective.h:78-80).
/// Any other value reads as `Undetermined`.
pub fn status_from_code(code: i32) -> ObjectiveStatus {
    match code {
        1 => ObjectiveStatus::Successful,
        2 => ObjectiveStatus::Failed,
        _ => ObjectiveStatus::Undetermined,
    }
}

/// The inverse of [`status_from_code`].
pub fn status_code(status: ObjectiveStatus) -> i32 {
    match status {
        ObjectiveStatus::Undetermined => 0,
        ObjectiveStatus::Successful => 1,
        ObjectiveStatus::Failed => 2,
    }
}

/// `CBooleanArray::getElementValue` (objective.h:123-131): case-sensitive match, `false` when absent.
pub fn get_flag(flags: &[BooleanFlag], flag_id: &str) -> bool {
    for flag in flags {
        if flag.flag_id == flag_id {
            return flag.value;
        }
    }
    false
}

/// `CBooleanArray::setElementValue` (objective.h:114-122): replace in place, or append when absent.
pub fn set_flag(flags: &mut Vec<BooleanFlag>, flag_id: &str, value: bool) {
    for flag in flags.iter_mut() {
        if flag.flag_id == flag_id {
            flag.value = value;
            return;
        }
    }
    flags.push(BooleanFlag {
        flag_id: flag_id.to_string(),
        value,
    });
}

/// One condition's `Status()` for the team `alignment` (objective.cpp:120-1155, single-player branches).
pub fn condition_status<W: ObjectiveWorld + ?Sized>(
    world: &W,
    flags: &[BooleanFlag],
    alignment: i32,
    condition: &Condition,
) -> ObjectiveStatus {
    match condition {
        Condition::DestroyAllEnemyUnits => {
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.team_id != alignment
                    && !mover_destroyed_or_disabled_or_captured(world, &mover, alignment)
                {
                    return ObjectiveStatus::Undetermined;
                }
            }
            ObjectiveStatus::Successful
        }
        Condition::DestroyNumberOfEnemyUnits { num } => {
            let mut num_destroyed = 0;
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.team_id != alignment
                    && mover_destroyed_or_disabled_or_captured(world, &mover, alignment)
                {
                    num_destroyed += 1;
                }
            }
            if num_destroyed >= *num as i32 {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::DestroySpecificEnemyUnit { unit } => {
            if let Some(mover) = world.mover_by_ref(unit) {
                if mover_destroyed_or_disabled_or_captured(world, &mover, alignment) {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                // The spec says Undetermined for missing objects (BUG-SWEEP)
                ObjectiveStatus::Undetermined
            }
        }
        Condition::DestroySpecificStructure { structure } => {
            if let Some(building) = world.building_by_ref(structure) {
                if building.destroyed {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::CaptureOrDestroyAllEnemyUnits => {
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.team_id != alignment
                    && !mover_destroyed_or_disabled(world, &mover)
                    && !mover_captured(world, &mover, alignment)
                {
                    return ObjectiveStatus::Undetermined;
                }
            }
            ObjectiveStatus::Successful
        }
        Condition::CaptureOrDestroyNumberOfEnemyUnits { num } => {
            let mut num_captured_or_destroyed = 0;
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.team_id != alignment
                    && (mover_destroyed_or_disabled(world, &mover)
                        || mover_captured(world, &mover, alignment))
                {
                    num_captured_or_destroyed += 1;
                }
            }
            if num_captured_or_destroyed >= *num as i32 {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::CaptureOrDestroySpecificEnemyUnit { unit } => {
            if let Some(mover) = world.mover_by_ref(unit) {
                if mover_destroyed_or_disabled(world, &mover)
                    || mover_captured(world, &mover, alignment)
                {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::CaptureOrDestroySpecificStructure { structure } => {
            // objective.cpp:636-651: destroyed, or already owned by this objective's alignment. A captured structure of another
            // team does not count (the `captured` flag alone is not enough).
            if let Some(building) = world.building_by_ref(structure) {
                if building.destroyed || building.team_id == alignment {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::DeadOrFledAllEnemyUnits => {
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if !mover_dead_or_fled(world, &mover) {
                    return ObjectiveStatus::Undetermined;
                }
            }
            ObjectiveStatus::Successful
        }
        Condition::DeadOrFledNumberOfEnemyUnits { num } => {
            let mut num_dead_or_fled = 0;
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover_dead_or_fled(world, &mover) {
                    num_dead_or_fled += 1;
                }
            }
            if num_dead_or_fled >= *num as i32 {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::DeadOrFledSpecificEnemyUnit { unit } => {
            if let Some(mover) = world.mover_by_ref(unit) {
                if mover_dead_or_fled(world, &mover) {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::CaptureUnit { unit } => {
            if let Some(mover) = world.mover_by_ref(unit) {
                if mover.destroyed {
                    ObjectiveStatus::Failed
                } else if mover.team_id == alignment {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                // The C++ null branch returns Failed (objective.cpp:734)
                ObjectiveStatus::Failed
            }
        }
        Condition::CaptureStructure { structure } => {
            if let Some(building) = world.building_by_ref(structure) {
                if building.destroyed {
                    ObjectiveStatus::Failed
                } else if building.team_id == alignment {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::GuardSpecificUnit { unit } => {
            if let Some(mover) = world.mover_by_ref(unit) {
                if mover.destroyed || mover.team_id != alignment {
                    ObjectiveStatus::Failed
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::GuardSpecificStructure { structure } => {
            if let Some(building) = world.building_by_ref(structure) {
                if building.destroyed || (building.captured && building.team_id != alignment) {
                    ObjectiveStatus::Failed
                } else if world.time_limit() != -1.0 && world.actual_time() > world.time_limit() {
                    ObjectiveStatus::Successful
                } else {
                    ObjectiveStatus::Undetermined
                }
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::MoveAnyUnitToArea { area } => {
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.on_gui && mover.commander_id == world.home_commander_id() {
                    let dx = mover.x - area.x;
                    let dy = mover.y - area.y;
                    let dist_sq = dx * dx + dy * dy;
                    let radius_sq = area.radius * area.radius;
                    if dist_sq < radius_sq {
                        return ObjectiveStatus::Successful;
                    }
                }
            }
            ObjectiveStatus::Undetermined
        }
        Condition::MoveAllUnitsToArea { area } => {
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.recover {
                    continue;
                }
                if mover.on_gui && mover.commander_id == world.home_commander_id() {
                    let dx = mover.x - area.x;
                    let dy = mover.y - area.y;
                    let dist_sq = dx * dx + dy * dy;
                    let radius_sq = area.radius * area.radius;
                    if dist_sq > radius_sq {
                        return ObjectiveStatus::Undetermined;
                    }
                }
            }
            ObjectiveStatus::Successful
        }
        Condition::MoveAllSurvivingUnitsToArea { area } => {
            let num_movers = world.num_movers();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.recover {
                    continue;
                }
                if mover.on_gui
                    && mover.commander_id == world.home_commander_id()
                    && !mover_dead_or_fled(world, &mover)
                {
                    let dx = mover.x - area.x;
                    let dy = mover.y - area.y;
                    let dist_sq = dx * dx + dy * dy;
                    let radius_sq = area.radius * area.radius;
                    if dist_sq > radius_sq {
                        return ObjectiveStatus::Undetermined;
                    }
                }
            }
            ObjectiveStatus::Successful
        }
        Condition::MoveAllSurvivingMechsToArea { area } => {
            let num_movers = world.num_movers();
            // Check all our mechs are in the area
            for i in 0..num_movers {
                let mover = world.mover(i);
                // Only mechs (not air) with home commander
                if !mover.is_mech || mover.air {
                    continue;
                }
                if mover.on_gui
                    && mover.commander_id == world.home_commander_id()
                    && !mover_dead_or_fled(world, &mover)
                {
                    let dx = mover.x - area.x;
                    let dy = mover.y - area.y;
                    let dist_sq = dx * dx + dy * dy;
                    let radius_sq = area.radius * area.radius;
                    if dist_sq > radius_sq {
                        return ObjectiveStatus::Undetermined;
                    }
                }
            }

            // Check no enemy units within MaxExtractUnitDistance
            let max_extract_sq =
                world.max_extract_unit_distance() * world.max_extract_unit_distance();
            for i in 0..num_movers {
                let mover = world.mover(i);
                if mover.commander_id != world.home_commander_id()
                    && !mover_dead_or_fled(world, &mover)
                {
                    let dx = mover.x - area.x;
                    let dy = mover.y - area.y;
                    let dist_sq = dx * dx + dy * dy;
                    if dist_sq < max_extract_sq {
                        return ObjectiveStatus::Undetermined;
                    }
                }
            }

            ObjectiveStatus::Successful
        }
        Condition::BooleanFlagIsSet { flag_id, value } => {
            let flag_value = get_flag(flags, flag_id);
            if flag_value == *value {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Undetermined
            }
        }
        Condition::ElapsedMissionTime { time } => {
            if world.actual_time() >= *time {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Undetermined
            }
        }
    }
}

// Helper functions for mover status checks

fn mover_destroyed_or_disabled<W: ObjectiveWorld + ?Sized>(_world: &W, mover: &MoverView) -> bool {
    if mover.destroyed || mover.disabled {
        return true;
    }
    matches!(mover.pilot_status, PilotStatus::Dead | PilotStatus::Ejected)
}

fn mover_destroyed_or_disabled_or_captured<W: ObjectiveWorld + ?Sized>(
    world: &W,
    mover: &MoverView,
    alignment: i32,
) -> bool {
    if mover_destroyed_or_disabled(world, mover) {
        return true;
    }
    mover_captured(world, mover, alignment)
}

fn mover_captured<W: ObjectiveWorld + ?Sized>(
    _world: &W,
    mover: &MoverView,
    alignment: i32,
) -> bool {
    // MoverIsCaptured: not destroyed and on our team
    !mover.destroyed && mover.team_id == alignment
}

fn mover_dead_or_fled<W: ObjectiveWorld + ?Sized>(_world: &W, mover: &MoverView) -> bool {
    if mover.destroyed || mover.disabled {
        return true;
    }
    matches!(
        mover.pilot_status,
        PilotStatus::Dead | PilotStatus::Ejected | PilotStatus::Withdrawn
    )
}

/// `CObjective::Status()` for `team.objectives[index]` (objective.cpp:1869-1942).
pub fn objective_status<W: ObjectiveWorld + ?Sized>(
    world: &mut W,
    team: &mut TeamObjectives,
    index: usize,
) -> ObjectiveStatus {
    let objective = &mut team.objectives[index];

    if !objective.is_active {
        return ObjectiveStatus::Undetermined;
    }

    if objective.resolved {
        return status_from_code(objective.resolved_status);
    }

    // Evaluate success conditions
    let mut objective_status = ObjectiveStatus::Successful;
    for condition in &objective.conditions {
        let condition_status =
            condition_status(world, &team.boolean_flags, team.team as i32, condition);
        if condition_status == ObjectiveStatus::Failed {
            objective_status = ObjectiveStatus::Failed;
            break;
        } else if condition_status == ObjectiveStatus::Undetermined {
            objective_status = ObjectiveStatus::Undetermined;
        }
    }

    // Check failure conditions if success is not already failed
    if objective_status != ObjectiveStatus::Failed && !objective.failure_conditions.is_empty() {
        let mut failure_status = ObjectiveStatus::Successful;
        for condition in &objective.failure_conditions {
            let condition_status =
                condition_status(world, &team.boolean_flags, team.team as i32, condition);
            if condition_status == ObjectiveStatus::Failed {
                failure_status = ObjectiveStatus::Failed;
                break;
            } else if condition_status == ObjectiveStatus::Undetermined {
                failure_status = ObjectiveStatus::Undetermined;
                break;
            }
        }

        if failure_status == ObjectiveStatus::Successful {
            // Failure conditions are satisfied
            objective_status = ObjectiveStatus::Failed;
        }
    }

    if objective_status == ObjectiveStatus::Undetermined {
        return ObjectiveStatus::Undetermined;
    }

    // Resolve the objective
    set_objective_status(objective, objective_status);

    if objective_status == ObjectiveStatus::Failed {
        if !objective.is_hidden_trigger {
            world.play_betty(Betty::CannotCompleteObjective);
        }
        for action in &objective.failure_actions {
            if let Action::SetBooleanFlag { flag_id, value } = action {
                set_flag(&mut team.boolean_flags, flag_id, *value);
            } else {
                world.execute_action(team.team as i32, action);
            }
        }
    } else {
        if !objective.is_hidden_trigger {
            world.play_betty(Betty::ObjectiveComplete);
        }
        for action in &objective.actions {
            if let Action::SetBooleanFlag { flag_id, value } = action {
                set_flag(&mut team.boolean_flags, flag_id, *value);
            } else {
                world.execute_action(team.team as i32, action);
            }
        }
    }

    objective_status
}

/// `CObjective::Status(CObjectives&)` for `team.objectives[index]` (objective.cpp:1944-1989).
pub fn objective_status_in_list<W: ObjectiveWorld + ?Sized>(
    world: &mut W,
    team: &mut TeamObjectives,
    index: usize,
) -> ObjectiveStatus {
    let Some(objective) = team.objectives.get(index) else {
        return ObjectiveStatus::Undetermined;
    };
    if !objective.is_active {
        return ObjectiveStatus::Undetermined;
    }

    // Check if this objective has qualifiers
    let needs_qualifier_check = objective.previous_primary_objective_must_be_complete
        || objective.all_previous_primary_objectives_must_be_complete;

    if !needs_qualifier_check {
        // No qualifiers, just return plain status
        return objective_status(world, team, index);
    }

    let mut retval = ObjectiveStatus::Successful;
    let mut previous_primary = ObjectiveStatus::Successful;
    let mut all_previous_complete = true;
    for i in 0..team.objectives.len() {
        let (all_prev, prev_must_complete, priority) = {
            let prior = &team.objectives[i];
            (
                prior.all_previous_primary_objectives_must_be_complete,
                prior.previous_primary_objective_must_be_complete,
                prior.priority,
            )
        };
        let status = if all_prev && !all_previous_complete {
            ObjectiveStatus::Undetermined
        } else if prev_must_complete && previous_primary != ObjectiveStatus::Successful {
            previous_primary
        } else {
            objective_status(world, team, i)
        };
        if i == index {
            return status;
        }
        if priority == 1 {
            if status == ObjectiveStatus::Failed {
                retval = ObjectiveStatus::Failed;
                break;
            }
            if status == ObjectiveStatus::Undetermined {
                all_previous_complete = false;
                retval = ObjectiveStatus::Undetermined;
            }
            previous_primary = status;
        }
    }
    retval
}

/// `CObjective::Status(objective_status_type)` (objective.cpp:1991-1999).
pub fn set_objective_status(objective: &mut Objective, status: ObjectiveStatus) {
    match status {
        ObjectiveStatus::Undetermined => {
            objective.resolved = false;
        }
        _ => {
            objective.resolved = true;
        }
    }
    objective.resolved_status = status_code(status);
    objective.changed_status = true;
}

/// `CObjective::StatusChangedSuccess()` (objective.cpp:2001-2018).
pub fn status_changed_success(objective: &mut Objective) -> bool {
    if !objective.is_active {
        return false;
    }

    if objective.changed_status {
        objective.changed_status = false;
        if objective.resolved_status == 1 {
            return true;
        }
    }

    false
}

/// `CObjective::StatusChangedFailed()` (objective.cpp:2020-2035).
pub fn status_changed_failed(objective: &mut Objective) -> bool {
    if !objective.is_active {
        return false;
    }

    if objective.changed_status {
        objective.changed_status = false;
        if objective.resolved_status == 2 {
            return true;
        }
    }

    false
}

/// `CObjectives::Status()` for one team (objective.cpp:2165-2255).
pub fn team_status<W: ObjectiveWorld + ?Sized>(
    world: &mut W,
    team: &mut TeamObjectives,
) -> ObjectiveStatus {
    let mut retval = ObjectiveStatus::Successful;
    let mut status_of_previous_primary_objective = ObjectiveStatus::Successful;
    let mut all_previous_primary_objectives_have_been_completed = true;

    for i in 0..team.objectives.len() {
        // Extract properties before calling objective_status which mutates team
        let objective = &team.objectives[i];
        let is_active = objective.is_active;
        let priority = objective.priority;
        let reset_status_on_flag = objective.reset_status_on_flag;
        let reset_status_flag_id = objective.reset_status_flag_id.clone();
        let all_prev = objective.all_previous_primary_objectives_must_be_complete;
        let prev_must_complete = objective.previous_primary_objective_must_be_complete;

        if is_active {
            // Check if objective status needs to be reset
            if reset_status_on_flag && get_flag(&team.boolean_flags, &reset_status_flag_id) {
                set_flag(&mut team.boolean_flags, &reset_status_flag_id, false);
                set_objective_status(&mut team.objectives[i], ObjectiveStatus::Undetermined);
            }

            // Determine status considering qualifiers
            let status = if all_prev && !all_previous_primary_objectives_have_been_completed {
                ObjectiveStatus::Undetermined
            } else if prev_must_complete
                && status_of_previous_primary_objective != ObjectiveStatus::Successful
            {
                // Previous primary objective was not successful
                status_of_previous_primary_objective
            } else {
                // Evaluate this objective (only if qualifiers are met)
                objective_status(world, team, i)
            };

            if priority == 1 {
                if status == ObjectiveStatus::Failed {
                    all_previous_primary_objectives_have_been_completed = false;
                    retval = ObjectiveStatus::Failed;
                    // If one primary objective is failed, it's over
                    break;
                } else if status == ObjectiveStatus::Undetermined {
                    all_previous_primary_objectives_have_been_completed = false; // assignment consumed in next loop iteration
                    retval = ObjectiveStatus::Undetermined;
                }
                status_of_previous_primary_objective = status;
            }
        } else {
            // Check if inactive objective needs to become active (objective.cpp:2203-2218). The flag is read for every
            // inactive objective; only the rule that keeps the mission open is limited to primaries.
            if objective.activate_on_flag {
                if priority == 1 {
                    // A primary objective still inactive - mission is not over
                    retval = ObjectiveStatus::Undetermined;
                }

                if get_flag(&team.boolean_flags, &objective.activate_flag_id) {
                    team.objectives[i].is_active = true;
                    team.objectives[i].activation_time = world.actual_time();
                    world.show_objectives();
                }
            }
        }
    }

    // Use the variable to avoid unused assignment warning
    let _ = all_previous_primary_objectives_have_been_completed;

    retval
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_get_missing_is_false_set_appends_then_replaces() {
        let mut f = Vec::new();
        assert!(!get_flag(&f, "a"));
        set_flag(&mut f, "a", true);
        set_flag(&mut f, "b", false);
        set_flag(&mut f, "a", false);
        assert_eq!(
            f,
            vec![
                BooleanFlag {
                    flag_id: "a".into(),
                    value: false
                },
                BooleanFlag {
                    flag_id: "b".into(),
                    value: false
                }
            ]
        );
        set_flag(&mut f, "b", true);
        assert!(get_flag(&f, "b"));
    }

    #[test]
    fn flags_are_case_sensitive() {
        let mut f = Vec::new();
        set_flag(&mut f, "Gate", true);
        assert!(!get_flag(&f, "gate"));
        set_flag(&mut f, "gate", false);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn status_codes_round_trip() {
        for s in [
            ObjectiveStatus::Undetermined,
            ObjectiveStatus::Successful,
            ObjectiveStatus::Failed,
        ] {
            assert_eq!(status_from_code(status_code(s)), s);
        }
        assert_eq!(status_code(ObjectiveStatus::Failed), 2);
        assert_eq!(status_from_code(7), ObjectiveStatus::Undetermined);
    }
}
