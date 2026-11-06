// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Objectives (`code/objective.{h,cpp}`): 24 condition species, 7 action species, sticky status with one-shot actions
//! (evaluation has side effects: Betty sample, action list). Only the home team's objectives are evaluated. Data model and
//! loaders already exist in `mc2_sim_data::mission::objective`.

/// `ObjectiveStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ObjectiveStatus {
    /// Not decided.
    #[default]
    Undetermined,
    /// Succeeded.
    Successful,
    /// Failed.
    Failed,
}

/// Runtime state of one objective.
#[derive(Clone, Debug, Default)]
pub struct ObjectiveRuntime {
    /// Active (activated by flag or from the start).
    pub active: bool,
    /// Resolved (sticky).
    pub resolved: bool,
    /// Cached resolved status.
    pub status: ObjectiveStatus,
    /// `m_changedStatus` (consumed by the first query of either kind).
    pub changed: bool,
}

// ------------------------------------------------------------------------------------------------------------------------
// Loading (`Team::loadObjectives`, objective.cpp:1675-1866, 2095-2165): the data model plus watch id resolution.
// ------------------------------------------------------------------------------------------------------------------------

use super::objective_eval;
use super::outcome::WarriorStatus;
use crate::ids::WatchId;
use crate::object::{flags, Kind, Obj};
use mc2_sim_data::mission::{Action, Condition, ObjectRef, Objective, TeamObjectives};

/// A condition with the watch id its position or `MoverWID`/`BuildingWID` resolved to (0 when it names no object).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedCondition {
    /// The parsed condition.
    pub condition: Condition,
    /// `m_pUnitWID` / `m_pBuildingWID`.
    pub wid: WatchId,
}

/// An action with its resolved watch id (only `_RemoveStructure` names an object).
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedAction {
    /// The parsed action.
    pub action: Action,
    /// `C_RemoveStructure::m_pBuildingWID`.
    pub wid: WatchId,
}

/// One loaded objective (`CObjective`): definition, resolved lists and the runtime state.
#[derive(Clone, Debug)]
pub struct ObjectiveEntry {
    /// The definition as read from the mission file.
    pub def: Objective,
    /// Conditions in file order.
    pub conditions: Vec<ResolvedCondition>,
    /// Actions.
    pub actions: Vec<ResolvedAction>,
    /// Failure conditions.
    pub failure_conditions: Vec<ResolvedCondition>,
    /// Failure actions.
    pub failure_actions: Vec<ResolvedAction>,
    /// Runtime state, initialised from the definition (`m_isActive`, `m_resolved`, `m_resolvedStatus`, `m_changedStatus`).
    pub runtime: ObjectiveRuntime,
}

/// One team's objectives (`Team::objectives`) and its boolean flags.
#[derive(Clone, Debug, Default)]
pub struct TeamObjectiveSet {
    /// Team id (`Alignment()`).
    pub team: usize,
    /// `CBooleanArray` entries.
    pub boolean_flags: Vec<mc2_sim_data::mission::BooleanFlag>,
    /// Block objectives, then the nav marker objectives.
    pub entries: Vec<ObjectiveEntry>,
    /// `numPrimaryObjectives`: objectives with priority 1 (`Team::loadObjectives`).
    pub num_primary: i32,
}

/// `areCloseEnough` (objective.cpp:292): relative difference below `10 * 10^-FLT_DIG`.
pub fn are_close_enough(f1: f32, f2: f32) -> bool {
    let diff = (f1 - f2).abs();
    let mut bigger = f1.abs();
    if f2.abs() > bigger {
        bigger = f2.abs();
    }
    let mut proportion_diff = 0.0f32;
    if bigger != 0.0 {
        proportion_diff = diff / bigger;
    }
    (mc2_libm::pow(10.0, -6.0) * 10.0) > f64::from(proportion_diff)
}

/// Which mover or structure search a condition species uses (objective.cpp:420-860).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Search {
    /// Movers, any team, no side effect (`CSpecificEnemyUnitObjectiveCondition::Read`, shared by destroy and dead-or-fled).
    UnitAnyTeam,
    /// Movers of another team than the objective's, marks the mover capturable (capture-or-destroy, capture unit).
    UnitOtherTeamCapturable,
    /// Movers of the objective's own team (`CGuardSpecificUnit`).
    UnitOwnTeam,
    /// Buildings, epsilon match, no side effect (destroy and guard structure).
    Structure,
    /// Buildings, epsilon match, marks the building capturable (capture-or-destroy and capture structure).
    StructureCapturable,
}

fn search_of(c: &Condition) -> Option<(Search, ObjectRef)> {
    Some(match c {
        Condition::DestroySpecificEnemyUnit { unit }
        | Condition::DeadOrFledSpecificEnemyUnit { unit } => (Search::UnitAnyTeam, *unit),
        Condition::CaptureOrDestroySpecificEnemyUnit { unit } | Condition::CaptureUnit { unit } => {
            (Search::UnitOtherTeamCapturable, *unit)
        }
        Condition::GuardSpecificUnit { unit } => (Search::UnitOwnTeam, *unit),
        Condition::DestroySpecificStructure { structure }
        | Condition::GuardSpecificStructure { structure } => (Search::Structure, *structure),
        Condition::CaptureOrDestroySpecificStructure { structure }
        | Condition::CaptureStructure { structure } => (Search::StructureCapturable, *structure),
        _ => return None,
    })
}

/// An object a position can resolve to: its list position, handle, ground position and (for movers) team.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Handle in the object store.
    pub handle: crate::ids::ObjectHandle,
    /// `position.x`.
    pub x: f32,
    /// `position.y`.
    pub y: f32,
    /// `teamId` (movers; -1 for structures).
    pub team: i32,
}

/// What objective loading needs from the world: the mover list and the building list in `ObjectManager` order, watch ids (assigned
/// lazily, so the order of requests decides the numbers) and the capturable flag.
pub trait ObjectIndex {
    /// `ObjectManager::getMover(0..getNumMovers())`.
    fn movers(&self) -> Vec<Candidate>;
    /// `ObjectManager::getBuilding(0..)` up to the first NULL.
    fn buildings(&self) -> Vec<Candidate>;
    /// `getWatchID()`.
    fn watch_id(&mut self, h: crate::ids::ObjectHandle) -> WatchId;
    /// `setFlag(OBJECT_FLAG_CAPTURABLE, true)`.
    fn set_capturable(&mut self, h: crate::ids::ObjectHandle);
}

fn mover_team(o: &Obj) -> Option<i32> {
    match &o.kind {
        Kind::Mech(m) => Some(i32::from(m.m.team.0)),
        Kind::Vehicle(v) => Some(i32::from(v.m.team.0)),
        _ => None,
    }
}

impl ObjectIndex for crate::World {
    fn movers(&self) -> Vec<Candidate> {
        self.objects
            .movers
            .iter()
            .filter_map(|&h| {
                let o = self.objects.get(h)?;
                Some(Candidate {
                    handle: h,
                    x: o.c.position.x,
                    y: o.c.position.y,
                    team: mover_team(o).unwrap_or(-1),
                })
            })
            .collect()
    }
    fn buildings(&self) -> Vec<Candidate> {
        self.objects
            .buildings
            .iter()
            .filter_map(|&h| {
                let o = self.objects.get(h)?;
                Some(Candidate {
                    handle: h,
                    x: o.c.position.x,
                    y: o.c.position.y,
                    team: -1,
                })
            })
            .collect()
    }
    fn watch_id(&mut self, h: crate::ids::ObjectHandle) -> WatchId {
        self.objects.watch_id(h)
    }
    fn set_capturable(&mut self, h: crate::ids::ObjectHandle) {
        if let Some(o) = self.objects.get_mut(h) {
            o.c.flags |= flags::CAPTURABLE;
        }
    }
}

/// The resolver: finds objects by position the way the `Read` methods do.
pub struct Resolver<'a, W: ObjectIndex> {
    /// The world whose objects are searched.
    pub world: &'a mut W,
    /// The team the objectives belong to (`Alignment()`).
    pub team: i32,
}

impl<W: ObjectIndex> Resolver<'_, W> {
    fn find_mover(
        &mut self,
        x: f32,
        y: f32,
        team_filter: Option<bool>,
    ) -> Option<crate::ids::ObjectHandle> {
        for c in self.world.movers() {
            let team_ok = match team_filter {
                None => true,
                // Some(true): another team than the objective's; Some(false): the objective's own team
                Some(other) => (c.team != self.team) == other,
            };
            if team_ok && are_close_enough(c.x, x) && are_close_enough(c.y, y) {
                return Some(c.handle);
            }
        }
        None
    }

    /// `getBuilding(0)`, then `for (i = 0; pBuilding; i++) { pBuilding = getBuilding(i); if (match) break; }`.
    fn find_building(&mut self, x: f32, y: f32, exact: bool) -> Option<crate::ids::ObjectHandle> {
        for c in self.world.buildings() {
            let hit = if exact {
                c.x == x && c.y == y
            } else {
                are_close_enough(c.x, x) && are_close_enough(c.y, y)
            };
            if hit {
                return Some(c.handle);
            }
        }
        None
    }

    /// `Read` of the condition: `None` drops it (the C++ `assert`s and returns false).
    fn resolve_condition(&mut self, c: &Condition) -> Option<ResolvedCondition> {
        let Some((search, reference)) = search_of(c) else {
            return Some(ResolvedCondition {
                condition: c.clone(),
                wid: WatchId::NONE,
            });
        };
        let wid = match reference {
            ObjectRef::Wid(w) => WatchId(w as u32),
            ObjectRef::Position { x, y } => {
                let h = match search {
                    Search::UnitAnyTeam => self.find_mover(x, y, None)?,
                    Search::UnitOtherTeamCapturable => self.find_mover(x, y, Some(true))?,
                    Search::UnitOwnTeam => self.find_mover(x, y, Some(false))?,
                    Search::Structure | Search::StructureCapturable => {
                        self.find_building(x, y, false)?
                    }
                };
                if matches!(
                    search,
                    Search::UnitOtherTeamCapturable | Search::StructureCapturable
                ) {
                    self.world.set_capturable(h);
                }
                self.world.watch_id(h)
            }
        };
        Some(ResolvedCondition {
            condition: c.clone(),
            wid,
        })
    }

    /// `C_RemoveStructure::Read` / `SetParams`: exact position match, no side effect.
    fn resolve_action(&mut self, a: &Action) -> Option<ResolvedAction> {
        let wid = match a {
            Action::RemoveStructure { structure } => match *structure {
                ObjectRef::Wid(w) => WatchId(w as u32),
                ObjectRef::Position { x, y } => {
                    let h = self.find_building(x, y, true)?;
                    self.world.watch_id(h)
                }
            },
            _ => WatchId::NONE,
        };
        Some(ResolvedAction {
            action: a.clone(),
            wid,
        })
    }
}

/// `Team::loadObjectives` after the objects exist: resolves every condition and action of the team in the order
/// `CObjective::Read` visits them (conditions, actions, failure conditions, failure actions; objectives in file order,
/// nav markers last), assigning watch ids to the buildings it finds in that order.
pub fn load_team_objectives<W: ObjectIndex>(
    world: &mut W,
    team: usize,
    data: &TeamObjectives,
) -> TeamObjectiveSet {
    let mut r = Resolver {
        world,
        team: team as i32,
    };
    let mut entries = Vec::new();
    for def in &data.objectives {
        let conditions = def
            .conditions
            .iter()
            .filter_map(|c| r.resolve_condition(c))
            .collect();
        let actions = def
            .actions
            .iter()
            .filter_map(|a| r.resolve_action(a))
            .collect();
        let failure_conditions = def
            .failure_conditions
            .iter()
            .filter_map(|c| r.resolve_condition(c))
            .collect();
        let failure_actions = def
            .failure_actions
            .iter()
            .filter_map(|a| r.resolve_action(a))
            .collect();
        let runtime = ObjectiveRuntime {
            active: def.is_active,
            resolved: def.resolved,
            status: match def.resolved_status {
                1 => ObjectiveStatus::Successful,
                2 => ObjectiveStatus::Failed,
                _ => ObjectiveStatus::Undetermined,
            },
            changed: def.changed_status,
        };
        entries.push(ObjectiveEntry {
            def: def.clone(),
            conditions,
            actions,
            failure_conditions,
            failure_actions,
            runtime,
        });
    }
    let num_primary = entries.iter().filter(|e| e.def.priority == 1).count() as i32;
    TeamObjectiveSet {
        team,
        boolean_flags: data.boolean_flags.clone(),
        entries,
        num_primary,
    }
}

/// `CObjectives::Status()` for a team (objective.cpp:2165): handles reset flags, activation, previous-primary qualifiers.
pub fn team_status(w: &mut crate::World, team: usize) -> ObjectiveStatus {
    with_team_model(w, team, objective_eval::team_status).unwrap_or(ObjectiveStatus::Successful)
}

/// `CObjective::Status(CObjectives&)` for objective `index` of a team (objective.cpp:1944-1989), which is what
/// `Mission::checkObjectiveStatus` returns (mission.cpp:3498-3513). Only objectives up to `index` are evaluated, and their
/// actions run as they do in the original; the result is this call's, never a cached value.
pub fn objective_status_at(w: &mut crate::World, team: usize, index: usize) -> ObjectiveStatus {
    with_team_model(w, team, |w, model| {
        objective_eval::objective_status_in_list(w, model, index)
    })
    .unwrap_or(ObjectiveStatus::Undetermined)
}

/// The evaluator's `TeamObjectives` view of a team's loaded objectives (watch ids restored, runtime flags as
/// `CObjective` holds them). The HUD builds the same view over a read-only world to read `CObjectives::Status`.
pub fn team_model_objectives(entries: &[ObjectiveEntry]) -> Vec<Objective> {
    entries
        .iter()
        .map(|entry| {
            let mut def = entry.def.clone();
            def.conditions = entry.conditions.iter().map(with_condition_wid).collect();
            def.failure_conditions = entry
                .failure_conditions
                .iter()
                .map(with_condition_wid)
                .collect();
            def.actions = entry.actions.iter().map(with_action_wid).collect();
            def.failure_actions = entry.failure_actions.iter().map(with_action_wid).collect();
            def.is_active = entry.runtime.active;
            def.resolved = entry.runtime.resolved;
            def.resolved_status = objective_eval::status_code(entry.runtime.status);
            def.changed_status = entry.runtime.changed;
            def
        })
        .collect()
}

/// Runs `f` on a `TeamObjectives` model of `team` (the evaluators work on that type), then writes the runtime state back.
/// `None` when the team has no objective set.
fn with_team_model<R>(
    w: &mut crate::World,
    team: usize,
    f: impl FnOnce(&mut crate::World, &mut TeamObjectives) -> R,
) -> Option<R> {
    let set = w.mission.objectives.get(team)?;
    let mut model = mc2_sim_data::mission::TeamObjectives {
        team: set.team as u32,
        boolean_flags: set.boolean_flags.clone(),
        objectives: team_model_objectives(&set.entries),
    };
    let result = f(w, &mut model);
    let set = &mut w.mission.objectives[team];
    set.boolean_flags = model.boolean_flags;
    for (entry, def) in set.entries.iter_mut().zip(model.objectives) {
        entry.runtime.active = def.is_active;
        entry.runtime.resolved = def.resolved;
        entry.runtime.status = objective_eval::status_from_code(def.resolved_status);
        entry.runtime.changed = def.changed_status;
        entry.def.activation_time = def.activation_time;
    }
    Some(result)
}

fn with_condition_wid(condition: &ResolvedCondition) -> Condition {
    let wid = ObjectRef::Wid(condition.wid.0 as i32);
    let mut condition = condition.condition.clone();
    match &mut condition {
        Condition::DestroySpecificEnemyUnit { unit }
        | Condition::CaptureOrDestroySpecificEnemyUnit { unit }
        | Condition::DeadOrFledSpecificEnemyUnit { unit }
        | Condition::CaptureUnit { unit }
        | Condition::GuardSpecificUnit { unit } => *unit = wid,
        Condition::DestroySpecificStructure { structure }
        | Condition::CaptureOrDestroySpecificStructure { structure }
        | Condition::CaptureStructure { structure }
        | Condition::GuardSpecificStructure { structure } => *structure = wid,
        _ => {}
    }
    condition
}

fn with_action_wid(action: &ResolvedAction) -> Action {
    let wid = ObjectRef::Wid(action.wid.0 as i32);
    let mut action = action.action.clone();
    if let Action::RemoveStructure { structure } = &mut action {
        *structure = wid;
    }
    action
}

impl objective_eval::ObjectiveWorld for crate::World {
    fn num_movers(&self) -> usize {
        self.objects.movers.len()
    }

    fn mover(&self, index: usize) -> objective_eval::MoverView {
        let h = self.objects.movers[index];
        self.objective_mover(h)
    }

    fn mover_by_ref(&self, reference: &ObjectRef) -> Option<objective_eval::MoverView> {
        let ObjectRef::Wid(wid) = reference else {
            return None;
        };
        let h = self.objects.by_watch_id(WatchId(*wid as u32))?;
        let o = self.objects.get(h)?;
        matches!(o.kind, Kind::Mech(_) | Kind::Vehicle(_)).then(|| self.objective_mover(h))
    }

    fn building_by_ref(&self, reference: &ObjectRef) -> Option<objective_eval::BuildingView> {
        let ObjectRef::Wid(wid) = reference else {
            return None;
        };
        let h = self.objects.by_watch_id(WatchId(*wid as u32))?;
        let o = self.objects.get(h)?;
        let Kind::Building(building) = &o.kind else {
            return None;
        };
        Some(objective_eval::BuildingView {
            team_id: i32::from(building.team_id),
            destroyed: o.c.is_destroyed(),
            captured: o.c.flag(flags::CAPTURED),
        })
    }

    fn home_commander_id(&self) -> i32 {
        self.teams.home as i32
    }

    fn actual_time(&self) -> f32 {
        self.mission.actual_time
    }

    fn time_limit(&self) -> f32 {
        self.mission.time_limit
    }

    fn max_extract_unit_distance(&self) -> f32 {
        // `MaxExtractUnitDistance`, read from gamesys.fit (mission.cpp:1794-1796). The 1280 default is the C++ fallback when the
        // key is missing and the value when the tables are not loaded.
        self.data
            .gamesys
            .tables
            .as_ref()
            .map_or(1280.0, |tables| tables.general.max_extract_unit_distance)
    }

    fn play_betty(&mut self, sample: objective_eval::Betty) {
        let id = match sample {
            objective_eval::Betty::ObjectiveComplete => 4,
            objective_eval::Betty::CannotCompleteObjective => 3,
        };
        self.events.push(crate::events::SimEvent::Betty(id as u32));
    }

    fn show_objectives(&mut self) {}

    fn execute_action(&mut self, team: i32, action: &Action) {
        if let Action::MakeNewTechnologyAvailable { purchase_file_name } = action {
            if !self
                .mission
                .unlocked_purchase_files
                .contains(purchase_file_name)
            {
                self.mission
                    .unlocked_purchase_files
                    .push(purchase_file_name.clone());
            }
        }
        if let Action::RemoveStructure {
            structure: ObjectRef::Wid(wid),
        } = action
        {
            if let Some(h) = self.objects.by_watch_id(WatchId(*wid as u32)) {
                if let Some(o) = self.objects.get_mut(h) {
                    o.c.position = crate::math::Vec3 {
                        x: 100_000.0,
                        y: 100_000.0,
                        z: 100_000.0,
                    };
                }
                crate::object::structure::update_building(self, h);
            }
            return;
        }
        self.events.push(crate::events::SimEvent::ObjectiveAction {
            team,
            action: action.clone(),
        });
    }
}

impl crate::World {
    fn objective_mover(&self, h: crate::ids::ObjectHandle) -> objective_eval::MoverView {
        let o = self.objects.get(h).expect("mover list handle");
        let (team_id, commander_id, on_gui, recover, is_mech, air, pilot_slot) = match &o.kind {
            Kind::Mech(m) => (
                i32::from(m.m.team.0),
                i32::from(m.m.commander.0),
                m.m.flags.on_gui,
                m.m.flags.salvage_vehicle,
                true,
                m.m.move_level == 2,
                m.m.pilot_slot,
            ),
            Kind::Vehicle(v) => (
                i32::from(v.m.team.0),
                i32::from(v.m.commander.0),
                v.m.flags.on_gui,
                v.can_recover,
                false,
                v.m.move_level == 2,
                v.m.pilot_slot,
            ),
            _ => unreachable!("mover list contains a non-mover"),
        };
        // `getPilot()->getStatus()` (objective.cpp:141-158). The mover's `withdrawing` flag is not the pilot's status: a unit
        // still withdrawing is not withdrawn until its pilot reaches WARRIOR_STATUS_WITHDRAWN (mech.cpp:6026). Six wounds are not
        // death by themselves: `injure` sets DEAD on its death paths only (warrior.cpp:1951-1982).
        // TODO(lane D): the withdrawal alarm that sets WARRIOR_STATUS_WITHDRAWN on the live pilot (warrior.cpp:7025) is not
        // ported, so no live unit reads as withdrawn yet. This read side is already the C++ rule.
        let pilot = self.pilots.get(pilot_slot);
        let pilot_status = match pilot {
            Some(p) => match p.status {
                WarriorStatus::Dead => objective_eval::PilotStatus::Dead,
                WarriorStatus::Ejected => objective_eval::PilotStatus::Ejected,
                WarriorStatus::Withdrawn => objective_eval::PilotStatus::Withdrawn,
                _ => objective_eval::PilotStatus::Normal,
            },
            None => objective_eval::PilotStatus::Normal,
        };
        objective_eval::MoverView {
            team_id,
            commander_id,
            x: o.c.position.x,
            y: o.c.position.y,
            destroyed: o.c.is_destroyed(),
            disabled: o.c.is_disabled(),
            pilot_status,
            on_gui,
            recover,
            is_mech,
            air,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::pilot::Pilot;
    use crate::ids::{CommanderId, ObjectHandle};
    use crate::object::{flags, Common, Kind, Obj, ObjectClass};

    /// objective.cpp:141-158 reads `getPilot()->getStatus()` for dead-or-fled. A mover that is still withdrawing (its `withdrawing`
    /// flag set, mech.cpp:6026) is not withdrawn until its pilot reaches WARRIOR_STATUS_WITHDRAWN.
    #[test]
    fn withdrawing_mover_is_not_withdrawn_until_its_pilot_is() {
        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        let handle = ObjectHandle(1);
        let mut common = Common::new(ObjectClass::BattleMech, handle);
        common.flags |= flags::EXISTS;
        let mut mech = crate::mover::MechState::default();
        mech.m.commander = CommanderId(0);
        mech.m.flags.on_gui = true;
        mech.m.flags.withdrawing = true;
        mech.m.pilot_slot = 1;
        w.objects.list = vec![
            None,
            Some(Obj {
                c: common,
                kind: Kind::Mech(Box::new(mech)),
            }),
        ];
        w.pilots.slots = vec![Pilot::default(), Pilot::default()];
        assert_eq!(
            w.objective_mover(handle).pilot_status,
            objective_eval::PilotStatus::Normal
        );
        w.pilots.slots[1].status = WarriorStatus::Withdrawn;
        assert_eq!(
            w.objective_mover(handle).pilot_status,
            objective_eval::PilotStatus::Withdrawn
        );
    }

    /// `MechWarrior::injure` sets DEAD only on its death paths (warrior.cpp:1976-1982): six wounds with an escape leave the status
    /// `Normal`, and the dead-or-fled checks read the status (objective.cpp:141-158), not the wound count.
    #[test]
    fn six_wounds_do_not_make_a_pilot_dead_by_themselves() {
        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        let handle = ObjectHandle(1);
        let mut common = Common::new(ObjectClass::BattleMech, handle);
        common.flags |= flags::EXISTS;
        let mut mech = crate::mover::MechState::default();
        mech.m.commander = CommanderId(0);
        mech.m.pilot_slot = 1;
        w.objects.list = vec![
            None,
            Some(Obj {
                c: common,
                kind: Kind::Mech(Box::new(mech)),
            }),
        ];
        w.pilots.slots = vec![Pilot::default(), Pilot::default()];
        w.pilots.slots[1].wounds = 6.0;
        assert_eq!(
            w.objective_mover(handle).pilot_status,
            objective_eval::PilotStatus::Normal
        );
        w.pilots.slots[1].status = WarriorStatus::Dead;
        assert_eq!(
            w.objective_mover(handle).pilot_status,
            objective_eval::PilotStatus::Dead
        );
    }
}

// ---- save/load (DESIGN.md section 9): the runtime state of the objectives. The definitions (conditions, actions, titles) and the
// team's primary count come from the mission file and stay as the base world has them; `load_into` overlays the rest.

impl crate::save::Snapshot for ObjectiveStatus {
    fn save(&self, w: &mut crate::save::Writer) {
        w.u8(match self {
            ObjectiveStatus::Undetermined => 0,
            ObjectiveStatus::Successful => 1,
            ObjectiveStatus::Failed => 2,
        });
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        match r.u8()? {
            0 => Ok(ObjectiveStatus::Undetermined),
            1 => Ok(ObjectiveStatus::Successful),
            2 => Ok(ObjectiveStatus::Failed),
            t => Err(crate::save::SaveError(format!(
                "save: unknown objective status {t}"
            ))),
        }
    }
}

crate::snapshot_struct!(ObjectiveRuntime {
    active,
    resolved,
    status,
    changed,
});

crate::snapshot_struct!(mc2_sim_data::mission::BooleanFlag { flag_id, value });

impl crate::save::Snapshot for ObjectiveEntry {
    fn save(&self, w: &mut crate::save::Writer) {
        self.runtime.save(w);
        // `team_status` writes the activation time back into the definition.
        self.def.activation_time.save(w);
    }
    fn load(_r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        Err(crate::save::SaveError(
            "objective state is restored over the base world (load_into)".into(),
        ))
    }
    fn load_into(&mut self, r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<()> {
        self.runtime = ObjectiveRuntime::load(r)?;
        self.def.activation_time = f32::load(r)?;
        Ok(())
    }
}

impl crate::save::Snapshot for TeamObjectiveSet {
    fn save(&self, w: &mut crate::save::Writer) {
        self.boolean_flags.save(w);
        w.len(self.entries.len());
        for entry in &self.entries {
            entry.save(w);
        }
    }
    fn load(_r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        Err(crate::save::SaveError(
            "objective sets are restored over the base world (load_into)".into(),
        ))
    }
    fn load_into(&mut self, r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<()> {
        use crate::save::SaveError;
        self.boolean_flags = Vec::load(r)?;
        let n = r.length()?;
        if n != self.entries.len() {
            return Err(SaveError(format!(
                "save: team has {} objectives, the base world {}",
                n,
                self.entries.len()
            )));
        }
        for entry in &mut self.entries {
            entry.load_into(r)?;
        }
        Ok(())
    }
}
