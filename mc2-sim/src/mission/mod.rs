// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! LANE E - mission loop, objectives, scripting bindings, logistics hooks, save/load.
//!
//! Port of `code/mission.{h,cpp}` (`Mission::init`, `update`, `addMover`, `removeMover`, `tradeMover`), `code/objective.{h,cpp}`,
//! `code/ablmc2.cpp` (the 222 ABL natives, over lanes A-D), the simulation parts of `DoGameLogic`, and the logistics
//! interface (`LogisticsData` for mech/pilot hand-over at start and the outcome report at the end). `weather`, camera,
//! sound and GUI are not ported (their natives are no-ops that push [`SimEvent`](crate::SimEvent)s).
//!
//! The exact `Mission::update` order is implemented by [`Sim::step`](crate::Sim::step); this module supplies the pieces.

pub mod abl_load;
pub mod bindings;
pub mod init;
pub mod loadout;
pub mod natives_audio;
pub mod natives_capture;
pub mod natives_object;
pub mod natives_presentation;
pub mod objective;
pub mod objective_eval;
pub mod outcome;
pub mod pilots;
pub mod plan;
pub mod spawn;
pub mod trade;
pub mod vtol;

pub use init::{LoadCtx, LoadSpec, LoadType};

use crate::time::TimerManager;
use std::collections::BTreeMap;

/// `MAX_GLOBAL_MISSION_VALUES` (mission.h:384): the size of `globalMissionValues`; the ABL natives ignore other indices.
pub const MAX_GLOBAL_MISSION_VALUES: usize = 50;

/// `mis_*` result codes (mission.h:41-46).
pub mod result {
    /// `mis_PLAYING`.
    pub const PLAYING: u32 = 0;
    /// `mis_PLAYER_LOST_BIG`.
    pub const LOST_BIG: u32 = 1;
    /// `mis_PLAYER_LOST_SMALL`.
    pub const LOST_SMALL: u32 = 2;
    /// `mis_PLAYER_DRAW`.
    pub const DRAW: u32 = 3;
    /// `mis_PLAYER_WIN_SMALL`.
    pub const WIN_SMALL: u32 = 4;
    /// `mis_PLAYER_WIN_BIG`.
    pub const WIN_BIG: u32 = 5;
}

/// Termination state machine (mission.cpp:545-663).
#[derive(Clone, Debug, Default)]
pub struct Termination {
    /// `terminationResult`: the mission script return value overwrites it every frame; -1 means "do not evaluate objectives".
    pub result: i32,
    /// `terminationCounterStarted`.
    pub counter_started: bool,
    /// `missionTerminationTime` (scenario seconds).
    pub end_time: f64,
    /// `scenarioResult` once the 5 s timer elapsed and the results screen finished.
    pub scenario_result: u32,
    /// `neverEndingStory` (cheat).
    pub never_ending_story: bool,
    /// `ControlGui::resultsDone()` (controlgui.cpp:2901). The GUI is not ported: the host sets it through
    /// [`Sim::results_screen_done`](crate::Sim::results_screen_done) when its results screen has finished.
    pub results_done: bool,
    /// The timer ran out while `results_done` was false, so the mission waits on its results screen (mission.cpp:566-569). Set
    /// on that frame and kept: the force-death path re-arms the timer every 5 s (mission.cpp:587-600), so the timer is not
    /// seen to run out again on the next frame. Cleared when the results screen is done.
    pub awaiting_results: bool,
    /// The termination counter was armed because the player's whole force is dead or disabled (mission.cpp:547-563, 587-600). The
    /// mission script overwrites `result` every frame, so the end of the counter needs this to stay a loss (see
    /// `update_termination`).
    pub force_lost: bool,
}

/// Everything lane E owns at run time (reset in `init`, saved in full; the original never reset several of these between
/// missions in one process, which is a bug the port does not keep).
#[derive(Clone, Debug)]
pub struct MissionState {
    /// `actualTime` (= `scenarioTime`).
    pub actual_time: f32,
    /// Termination.
    pub term: Termination,
    /// `globalMissionValues[MAX_GLOBAL_MISSION_VALUES]`.
    pub global_values: [f32; MAX_GLOBAL_MISSION_VALUES],
    /// Script timers (`timerManager`, 32 `Timer`s ticked by `Sim::step` before `Mission::update`, paused or not).
    pub timers: TimerManager,
    /// `GeneralAlarm`.
    pub general_alarm: bool,
    /// `friendlyDestroyed` / `enemyDestroyed` latches (read-and-clear by natives).
    pub destroyed_latches: (bool, bool),
    /// `invulnerableON`.
    pub invulnerable: bool,
    /// `m_timeLimit` (-1 = none).
    pub time_limit: f32,
    /// Mission brain (ABL instance).
    pub brain: Option<mc2_abl::InstId>,
    /// Paused (single player pause freezes scenario time, sensors, collisions and the object update).
    pub paused: bool,
    /// Trigger areas.
    pub triggers: crate::object::trigger::TriggerAreas,
    // ---- appended by the load half of lane E (`init_mission`) ----
    /// The mission file data (parts, warriors, objectives as written).
    pub file: Option<std::rc::Rc<mc2_sim_data::mission::Mission>>,
    /// `parts[0..=numParts]` after the squad alternatives (`Mission::parts`); entry 0 is unused.
    pub parts: Vec<plan::PartRec>,
    /// Squad bookkeeping of the load (`alternateChoice`, `randomAlternative[]`, ...).
    pub squads: plan::SquadChoice,
    /// Per pilot slot: the profile data lane D's `Pilot` has no field for.
    pub pilot_loads: Vec<pilots::PilotLoad>,
    /// Per team: loaded objectives (`Team::objectives`).
    pub objectives: Vec<objective::TeamObjectiveSet>,
    /// Commander group membership at start (`Commander{c}Group:{g}` blocks): the groups lane D builds its `MoverGroup`s from.
    pub groups: Vec<plan::GroupMember>,
    /// `missionFileName` (`mc2_01`).
    pub name: String,
    /// `missionScriptName`.
    pub script_name: String,
    /// `GameDifficulty` the mission was loaded with.
    pub difficulty: i32,
    /// `LogisticsData::setResourcePoints(numRPoints)` (`MaxResourcePoints` overrides the file).
    pub resource_points: i32,
    /// `missionTuneNum`.
    pub tune_num: u8,
    /// `theSkyNumber`.
    pub sky_number: i32,
    /// `dropZone` (x, y), `(-1, -1)` when absent.
    pub drop_zone: (f32, f32),
    /// `missionBrainCallback` (`findFunction("handlemessage", TRUE)`).
    pub brain_callback: Option<mc2_abl::SymId>,
    /// `maxTeamID` / `maxCommanderID` of the pre-scan (teams `0..=max_team_id` exist).
    pub max_team_id: i32,
    /// See `max_team_id`.
    pub max_commander_id: i32,
    /// The load type of the current mission.
    pub load_type: Option<LoadType>,
    /// `dropZoneList` for team 0: the positions of the loaded team-0 parts in file order (`add_force` uses the first).
    pub drop_zone_positions: Vec<crate::math::Vec3>,
    /// `useUnlimitedAmmo` (the load request; lane C reads it).
    pub unlimited_ammo: bool,
    /// Purchase files enabled by completed objective actions for the campaign handoff.
    pub unlocked_purchase_files: Vec<String>,
    /// `CurMultiplayCode` and `CurMultiplayParam` read by `getmessage`.
    pub current_message: (i32, i32),
    /// Paint scheme (`base, highlight1, highlight2`) of each mover, by watch id, as it was created (`Mech3DAppearance::getPaintScheme`
    /// in the campaign outcome). Written by `spawn_into_slot`.
    pub paint_schemes: Vec<(crate::ids::WatchId, [u32; 3])>,
    /// `Mover::unitGroup`, stored by mission part id rather than in the mover lane. A mover is in at
    /// most one Ctrl+digit group (`mover.cpp:7163-7176`); this mission-scoped, saved map preserves
    /// the same player-visible and save behavior without coupling the force-group slice to mover work.
    pub unit_groups: BTreeMap<crate::ids::PartId, u8>,
    /// Every pilot `add_mover` created, in creation order (`spawn::SpawnRecord`). `save::save_world` writes it ahead of the state and
    /// `save::load_world` replays the records the base world lacks (`spawn::replay_spawn`), so the base has the same ABL instances.
    pub spawn_log: Vec<spawn::SpawnRecord>,
}

impl Default for MissionState {
    fn default() -> Self {
        MissionState {
            actual_time: 0.0,
            term: Termination::default(),
            global_values: [0.0; MAX_GLOBAL_MISSION_VALUES],
            timers: TimerManager::default(),
            general_alarm: false,
            destroyed_latches: (false, false),
            invulnerable: false,
            time_limit: -1.0,
            brain: None,
            paused: false,
            triggers: Default::default(),
            file: None,
            parts: Vec::new(),
            squads: Default::default(),
            pilot_loads: Vec::new(),
            objectives: Vec::new(),
            groups: Vec::new(),
            name: String::new(),
            script_name: String::new(),
            difficulty: 1,
            resource_points: 0,
            tune_num: 0,
            sky_number: 1,
            drop_zone: (-1.0, -1.0),
            brain_callback: None,
            max_team_id: -1,
            max_commander_id: -1,
            load_type: None,
            drop_zone_positions: Vec::new(),
            unlimited_ammo: true,
            unlocked_purchase_files: Vec::new(),
            current_message: (0, 0),
            paint_schemes: Vec::new(),
            unit_groups: BTreeMap::new(),
            spawn_log: Vec::new(),
        }
    }
}

pub use spawn::add_mover;

/// `Mission::removeMover` (mission.cpp:1452-1462): the GUI hook and the MP roster updates are presentation or network only, so
/// the mover is just freed (`ObjectManager->freeMover`).
pub fn remove_mover(w: &mut crate::World, h: crate::ids::ObjectHandle) {
    crate::object::ObjectStore::free_mover(w, h);
}

/// Sets the `friendlyDestroyed` / `enemyDestroyed` latch (gamesound.cpp:31-32) for a destroyed mover: `friendly` is
/// `Team::home->isFriendly(team)` at the time of the destruction (mover.cpp:6526-6530). The ABL natives read and clear the latches
/// (ablmc2.cpp:2775-2783, [`SimBindings`](bindings::SimBindings)).
///
/// TODO(lane C): call this from `Mover::destroy` (mover.cpp:6517-6530) and the other C++ sites that set the latches
/// (mover.cpp:7192-7195, mech.cpp:1058-1061, 6791-6794, 7488-7491). Until then the latches are never set.
pub fn note_destroyed(w: &mut crate::World, friendly: bool) {
    if friendly {
        w.mission.destroyed_latches.0 = true;
    } else {
        w.mission.destroyed_latches.1 = true;
    }
}

/// `ControlGui::resultsDone()` (controlgui.cpp:2901) becomes true: the host's results screen has finished. Nothing starts a timer here.
/// The mission no longer waits on the results screen. `scenarioResult` is set on the next frame the termination timer has run out
/// (mission.cpp:566-594); with all the player's dudes dead the timer was re-armed at the last check (mission.cpp:587-600), so that is
/// the next expiry 5 s later.
///
/// TODO(lane A, app call; see porting-log): the mc2-app results screen must call `Sim::results_screen_done` when it finishes.
pub fn results_screen_done(w: &mut crate::World) {
    w.mission.term.results_done = true;
    w.mission.term.awaiting_results = false;
}

/// Termination logic of `Mission::update` (mission.cpp:545-663). Runs every frame **including while paused** (only the object
/// update, sensors, collisions and the mission script are skipped when paused); it reads `term.result`, which the script
/// overwrites each unpaused frame. Returns `scenarioResult`.
pub fn update_termination(w: &mut crate::World) -> u32 {
    use crate::object::Kind;

    fn player_force_all_dead(w: &crate::World) -> bool {
        w.objects.movers.iter().all(|&h| {
            let Some(o) = w.objects.get(h) else {
                return true;
            };
            let (commander, on_gui) = match &o.kind {
                Kind::Mech(m) => (m.m.commander.0, m.m.flags.on_gui),
                Kind::Vehicle(v) => (v.m.commander.0, v.m.flags.on_gui),
                _ => return true,
            };
            commander != 0 || !on_gui || o.c.is_disabled() || o.c.is_destroyed()
        })
    }

    let actual_time = w.mission.actual_time;
    let never_ending = w.mission.term.never_ending_story;
    let counter_started = w.mission.term.counter_started;

    if !never_ending && !counter_started && player_force_all_dead(w) {
        w.mission.term.result = result::LOST_BIG as i32;
        w.mission.term.counter_started = true;
        w.mission.term.force_lost = true;
        w.mission.term.end_time = f64::from(actual_time) + 5.0;
    }

    if !never_ending && w.mission.term.counter_started {
        if w.mission.term.end_time <= f64::from(actual_time) {
            if w.mission.term.results_done {
                w.mission.term.awaiting_results = false;
                let was_playing = w.mission.term.scenario_result == result::PLAYING;
                w.mission.term.scenario_result = w.mission.term.result as u32;
                if w.mission.term.scenario_result == result::PLAYING && w.mission.term.force_lost {
                    // The script trashed the LOST_BIG that the dead force armed the counter with (mission.cpp:559, 595). The
                    // C++ then asks the objectives, and an objective that holds for an empty force (`MoveAllSurvivingMechsToArea`
                    // with no mech left) reports a win for a mission whose force was wiped out: a logic error, fixed (D-80).
                    w.mission.term.scenario_result = result::LOST_BIG;
                } else if w.mission.term.scenario_result == result::PLAYING {
                    let status = if w.teams.home < w.mission.objectives.len() {
                        crate::mission::objective::team_status(w, w.teams.home)
                    } else {
                        crate::mission::objective::ObjectiveStatus::Undetermined
                    };
                    w.mission.term.scenario_result =
                        if status == crate::mission::objective::ObjectiveStatus::Successful {
                            result::WIN_BIG
                        } else {
                            result::LOST_BIG
                        };
                }
                if was_playing && w.mission.term.scenario_result != 0 {
                    w.events.push(crate::events::SimEvent::MissionResult(
                        w.mission.term.scenario_result,
                    ));
                }
            } else {
                // The results screen has not finished: the mission waits for it (mission.cpp:566-569).
                w.mission.term.awaiting_results = true;
                // if all the player's dudes are dead then end it (mission.cpp:587-600)
                if player_force_all_dead(w) {
                    w.mission.term.result = result::LOST_BIG as i32;
                    w.mission.term.force_lost = true;
                    w.mission.term.end_time = f64::from(actual_time) + 5.0;
                }
            }
        }
    } else if w.mission.term.result != -1
        && w.teams.home < w.mission.objectives.len()
        && !w.mission.objectives[w.teams.home].entries.is_empty()
    {
        let has_primary = w.mission.objectives[w.teams.home].num_primary > 0;
        let status = crate::mission::objective::team_status(w, w.teams.home);
        if has_primary
            && status != crate::mission::objective::ObjectiveStatus::Undetermined
            && !never_ending
        {
            w.mission.term.result =
                if status == crate::mission::objective::ObjectiveStatus::Successful {
                    result::WIN_BIG as i32
                } else {
                    result::LOST_BIG as i32
                };
            w.mission.term.counter_started = true;
            w.mission.term.end_time = f64::from(actual_time) + 5.0;
        }
    } else if w.mission.term.result == -1
        && w.teams.home < w.mission.objectives.len()
        && !w.mission.objectives[w.teams.home].entries.is_empty()
    {
        // Tutorial 5 returns -1 while its voice-over chain is incomplete; tutorials 1-4 return ScenarioResult. In the -1 case,
        // the objectives still run: `ControlGui::render` calls `Team::home->objectives.Status()` every frame (`controlgui.cpp:430`),
        // which activates objectives that wait on a flag and resets ones that ask for it. The result is not used here.
        let _ = crate::mission::objective::team_status(w, w.teams.home);
    }

    if !never_ending
        && !w.mission.term.counter_started
        && w.mission.time_limit != -1.0
        && actual_time > w.mission.time_limit
    {
        w.mission.term.result = result::LOST_BIG as i32;
        w.mission.term.counter_started = true;
        w.mission.term.end_time = f64::from(actual_time) + 5.0;
    }

    w.mission.term.scenario_result
}

/// `mc2-campaign::interface::Outcome`, kept as a lane E adapter so the sim crate does not depend on campaign orchestration.
pub use outcome::Outcome;

/// Build the end-of-mission report (`LogisticsData::setMissionCompleted` inputs).
pub fn outcome(w: &mut crate::World) -> Outcome {
    outcome::build(w)
}

/// Digest lines for lane E objective runtime state, matching the read-only `OE` records in the C++ oracle.
pub fn digest(w: &crate::World, _level: u32, out: &mut Vec<String>) {
    for team in &w.mission.objectives {
        for (index, objective) in team.entries.iter().enumerate() {
            out.push(format!(
                "OE {} {} {} {} {}",
                team.team,
                index,
                i32::from(objective.runtime.active),
                i32::from(objective.runtime.resolved),
                objective_eval::status_code(objective.runtime.status),
            ));
        }
    }
}

#[cfg(test)]
mod termination_tests {
    use super::*;

    #[test]
    fn force_lost_save_round_trip_continues_to_a_loss() {
        use crate::save::{Reader, Snapshot, Writer};
        let term = Termination {
            result: result::PLAYING as i32,
            force_lost: true,
            counter_started: true,
            results_done: true,
            end_time: 15.0,
            ..Termination::default()
        };
        let mut w = Writer::new();
        term.save(&mut w);
        let bytes = w.into_bytes();
        let mut r = Reader::new(&bytes);
        let back = Termination::load(&mut r).unwrap();
        r.finish().unwrap();
        assert!(back.force_lost && back.counter_started && back.results_done);

        let mut restored = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        restored.mission.actual_time = 15.0;
        restored.mission.term = back;
        assert_eq!(update_termination(&mut restored), result::LOST_BIG);
        assert_eq!(
            restored.mission.term.scenario_result,
            result::LOST_BIG,
            "the restored dead force remains a loss after the script's PLAYING result"
        );
    }

    fn add_live_home_mech(w: &mut crate::World) {
        let handle = crate::ids::ObjectHandle(1);
        let mut common = crate::object::Common::new(crate::object::ObjectClass::BattleMech, handle);
        common.flags |= crate::object::flags::EXISTS;
        let mut mech = crate::mover::MechState::default();
        mech.m.commander = crate::ids::CommanderId(0);
        mech.m.flags.on_gui = true;
        w.objects.list = vec![
            None,
            Some(crate::object::Obj {
                c: common,
                kind: crate::object::Kind::Mech(Box::new(mech)),
            }),
        ];
        w.objects.movers.push(handle);
    }

    #[test]
    fn empty_home_force_starts_five_second_loss_timer() {
        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        w.mission.actual_time = 10.0;
        assert_eq!(update_termination(&mut w), result::PLAYING);
        assert!(w.mission.term.counter_started);
        assert_eq!(w.mission.term.result, result::LOST_BIG as i32);
        assert_eq!(w.mission.term.end_time, 15.0);
    }

    #[test]
    fn results_done_sets_scenario_result_after_timer() {
        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        w.mission.term.counter_started = true;
        w.mission.term.end_time = 5.0;
        w.mission.term.result = result::WIN_SMALL as i32;
        w.mission.term.results_done = true;
        w.mission.actual_time = 5.0;
        assert_eq!(update_termination(&mut w), result::WIN_SMALL);
        assert_eq!(w.events.len(), 1);
        assert!(matches!(
            w.events[0],
            crate::events::SimEvent::MissionResult(4)
        ));
    }

    #[test]
    fn results_screen_input_is_needed_before_the_result_is_emitted() {
        // mission.cpp:566-594: after the timer, scenarioResult waits for ControlGui::resultsDone() (controlgui.cpp:2901).
        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        add_live_home_mech(&mut w);
        w.mission.term.counter_started = true;
        w.mission.term.end_time = 5.0;
        w.mission.term.result = result::LOST_BIG as i32;
        w.mission.actual_time = 6.0;
        assert_eq!(update_termination(&mut w), result::PLAYING);
        assert!(w.events.is_empty());
        assert!(
            w.mission.term.awaiting_results,
            "mission.cpp:566-569: the timer ran out and the results screen is not done"
        );

        results_screen_done(&mut w);
        assert!(!w.mission.term.awaiting_results);
        assert_eq!(update_termination(&mut w), result::LOST_BIG);
        assert!(matches!(
            w.events[..],
            [crate::events::SimEvent::MissionResult(1)]
        ));
    }

    #[test]
    fn time_limit_uses_strict_greater_than_and_respects_never_ending() {
        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        add_live_home_mech(&mut w);
        w.mission.time_limit = 2.0;
        w.mission.actual_time = 2.0;
        update_termination(&mut w);
        assert!(!w.mission.term.counter_started);
        w.mission.actual_time = 2.01;
        update_termination(&mut w);
        assert!(w.mission.term.counter_started);

        let mut w = crate::World::new(crate::rng::RngMode::Pcg32, 1);
        w.mission.time_limit = 1.0;
        w.mission.actual_time = 2.0;
        w.mission.term.never_ending_story = true;
        update_termination(&mut w);
        assert!(!w.mission.term.counter_started);
    }
}

// ---- save/load (DESIGN.md section 9): the mission's dynamic state. `file` (the parsed mission file) and the profile of each
// pilot load are static game data: `load_into` keeps them from the base world.

crate::snapshot_struct!(Termination {
    result,
    counter_started,
    end_time,
    scenario_result,
    never_ending_story,
    results_done,
    awaiting_results,
    force_lost,
});

impl crate::save::Snapshot for mc2_abl::InstId {
    fn save(&self, w: &mut crate::save::Writer) {
        w.u32(self.0);
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        Ok(mc2_abl::InstId(r.u32()?))
    }
}

impl crate::save::Snapshot for mc2_abl::SymId {
    fn save(&self, w: &mut crate::save::Writer) {
        w.u32(self.0);
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        Ok(mc2_abl::SymId(r.u32()?))
    }
}

impl crate::save::Snapshot for MissionState {
    fn save(&self, w: &mut crate::save::Writer) {
        self.actual_time.save(w);
        self.term.save(w);
        self.global_values.save(w);
        self.timers.save(w);
        self.general_alarm.save(w);
        self.destroyed_latches.save(w);
        self.invulnerable.save(w);
        self.time_limit.save(w);
        self.brain.save(w);
        self.paused.save(w);
        self.triggers.save(w);
        self.parts.save(w);
        self.squads.save(w);
        self.pilot_loads.save(w);
        w.len(self.objectives.len());
        for set in &self.objectives {
            set.save(w);
        }
        self.groups.save(w);
        self.name.save(w);
        self.script_name.save(w);
        self.difficulty.save(w);
        self.resource_points.save(w);
        self.tune_num.save(w);
        self.sky_number.save(w);
        self.drop_zone.save(w);
        self.brain_callback.save(w);
        self.max_team_id.save(w);
        self.max_commander_id.save(w);
        self.load_type.save(w);
        self.drop_zone_positions.save(w);
        self.unlimited_ammo.save(w);
        self.unlocked_purchase_files.save(w);
        self.current_message.save(w);
        self.paint_schemes.save(w);
        self.unit_groups.save(w);
        // `spawn_log` is not written here: `save::save_world` writes it before the clock, and the base world rebuilds it by replay.
    }

    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        let mut state = MissionState::default();
        state.load_into(r)?;
        Ok(state)
    }

    fn load_into(&mut self, r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<()> {
        use crate::save::{SaveError, Snapshot};
        self.actual_time = f32::load(r)?;
        self.term = Termination::load(r)?;
        self.global_values = Snapshot::load(r)?;
        self.timers = Snapshot::load(r)?;
        self.general_alarm = bool::load(r)?;
        self.destroyed_latches = Snapshot::load(r)?;
        self.invulnerable = bool::load(r)?;
        self.time_limit = f32::load(r)?;
        self.brain = Option::load(r)?;
        self.paused = bool::load(r)?;
        self.triggers = Snapshot::load(r)?;
        self.parts = Vec::load(r)?;
        self.squads = Snapshot::load(r)?;
        // Overlay per slot: the base slot's profile is kept only where the saved slot had one (`PilotLoad::load_into`).
        pilots::load_pilot_loads_into(&mut self.pilot_loads, r)?;
        let teams = r.length()?;
        if teams != self.objectives.len() {
            return Err(SaveError(format!(
                "save: {teams} objective teams, the base world {}",
                self.objectives.len()
            )));
        }
        for set in &mut self.objectives {
            set.load_into(r)?;
        }
        self.groups = Vec::load(r)?;
        // `name`, `script_name` and the other mission-file constants are saved too: they are equal to the base's, and a check
        // on the mission name is made by `save::load_world`.
        self.name = String::load(r)?;
        self.script_name = String::load(r)?;
        self.difficulty = i32::load(r)?;
        self.resource_points = i32::load(r)?;
        self.tune_num = u8::load(r)?;
        self.sky_number = i32::load(r)?;
        self.drop_zone = Snapshot::load(r)?;
        self.brain_callback = Option::load(r)?;
        self.max_team_id = i32::load(r)?;
        self.max_commander_id = i32::load(r)?;
        self.load_type = Option::load(r)?;
        self.drop_zone_positions = Vec::load(r)?;
        self.unlimited_ammo = bool::load(r)?;
        self.unlocked_purchase_files = Vec::load(r)?;
        self.current_message = Snapshot::load(r)?;
        self.paint_schemes = Vec::load(r)?;
        self.unit_groups = BTreeMap::load(r)?;
        // `spawn_log` is kept from the base (the replay of `save::load_world` made it equal to the saved log).
        Ok(())
    }
}
