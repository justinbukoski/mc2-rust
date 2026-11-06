// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Hooks that make an objective's WORLD CONDITION true directly in sim state (cargo feature `test-hooks`; compiled into the
//! app for the packaged `--selftest-play` objective check). Used by the objective harness (`tests/objective_harness.rs`) and that check.
//!
//! The rule of the harness: only WORLD FACTS are set here (a unit or building destroyed, a unit or building captured, the home
//! lance inside an area, mission time advanced). An objective's status, a mission-script variable and a boolean flag an
//! objective reads are never written: the mission's own ABL script and the objective system react through [`crate::Sim::step`].
//!
//! The condition species the C++ supports are the 24 of `code/objective.h:33-65` (`condition_species_type`), evaluated by
//! `code/objective.cpp`: [`satisfy_condition`] has one arm for each (the three `...EnemyUnitGroup` species are never constructed
//! by `CObjective::new_CObjectiveCondition`, objective.cpp:1391-1421, so a mission cannot use them).

use std::rc::Rc;

use crate::ids::ObjectHandle;
use crate::mission::init::{load_world, LoadSpec};
use crate::mission::objective::{ObjectiveStatus, ResolvedCondition};
use crate::object::{flags, Kind, ObjectStatus};
use crate::visibility::NoneVisible;
use crate::{FrameInput, FrameOutput, FrameUnavailable, Sim, World};
use mc2_formats::vfs::Vfs;
use mc2_sim_data::mission::{Condition, ObjectRef};

/// Fixed frame rate of the harness runs.
pub const FPS: u32 = 30;

/// Destroys a mover (`status = DESTROYED`, what `isDestroyed()` reads).
pub fn destroy_mover(w: &mut World, h: ObjectHandle) {
    if let Some(o) = w.objects.get_mut(h) {
        o.c.set_status(ObjectStatus::Destroyed, true);
    }
}

/// Disables a mover (`status = DISABLED`): what `isDisabled()` reads, and what a player does to a mech that is to be captured
/// afterwards (a destroyed unit cannot be captured, `CCaptureUnit::Status`, objective.cpp:733).
pub fn disable_mover(w: &mut World, h: ObjectHandle) {
    if let Some(o) = w.objects.get_mut(h) {
        o.c.set_status(ObjectStatus::Disabled, true);
    }
}

/// Destroys a building (`status = DESTROYED`).
pub fn destroy_building(w: &mut World, h: ObjectHandle) {
    if let Some(o) = w.objects.get_mut(h) {
        o.c.set_status(ObjectStatus::Destroyed, true);
    }
}

/// `Mover::setTeamId(home, true)`: the mover now belongs to the objective's team (a captured unit).
pub fn capture_mover(w: &mut World, h: ObjectHandle, team: i32) {
    let slot = match w.objects.get(h).map(|o| &o.kind) {
        Some(Kind::Mech(m)) => m.m.pilot_slot,
        Some(Kind::Vehicle(v)) => v.m.pilot_slot,
        _ => return,
    };
    let _ = crate::mission::spawn::set_team(w, h, team as i8, usize::from(slot));
}

/// `Building::setTeamId(team)` plus the `CAPTURED` flag (a captured structure).
pub fn capture_building(w: &mut World, h: ObjectHandle, team: i32) {
    crate::object::structure::set_team_id(w, h, team, false);
    if let Some(o) = w.objects.get_mut(h) {
        o.c.flags |= flags::CAPTURED;
    }
}

/// Moves a mover to `(x, y)` (keeps its height) with the engine's own `setPosition`.
pub fn move_mover(w: &mut World, h: ObjectHandle, x: f32, y: f32) {
    let Some(o) = w.objects.get(h) else { return };
    let mut p = o.c.position;
    p.x = x;
    p.y = y;
    crate::mover::set_position(w, h, p);
}

/// Advances mission time (`scenarioTime`) to at least `seconds`; the next frame reads it into `actualTime`.
pub fn advance_time_to(w: &mut World, seconds: f32) {
    if w.clock.scenario_time < seconds {
        w.clock.scenario_time = seconds;
    }
}

/// The movers of the home team's lance that are on the GUI (`getCommanderId() == home`, `isOnGUI()`).
pub fn home_lance(w: &World) -> Vec<ObjectHandle> {
    w.objects
        .movers
        .iter()
        .copied()
        .filter(|&h| {
            w.objects.get(h).is_some_and(|o| match &o.kind {
                Kind::Mech(m) => {
                    m.m.flags.on_gui && i32::from(m.m.commander.0) == w.teams.home as i32
                }
                Kind::Vehicle(v) => {
                    v.m.flags.on_gui && i32::from(v.m.commander.0) == w.teams.home as i32
                }
                _ => false,
            })
        })
        .collect()
}

fn mover_team(w: &World, h: ObjectHandle) -> Option<i32> {
    match &w.objects.get(h)?.kind {
        Kind::Mech(m) => Some(i32::from(m.m.team.0)),
        Kind::Vehicle(v) => Some(i32::from(v.m.team.0)),
        _ => None,
    }
}

pub fn mover_down(w: &World, h: ObjectHandle) -> bool {
    w.objects.get(h).is_some_and(|o| o.c.is_disabled())
}

/// Enemy movers of `team` (any other team), in object-manager order.
pub fn enemies(w: &World, team: i32) -> Vec<ObjectHandle> {
    w.objects
        .movers
        .iter()
        .copied()
        .filter(|&h| mover_team(w, h).is_some_and(|t| t != team))
        .collect()
}

fn wid_handle(w: &World, c: &ResolvedCondition) -> Option<ObjectHandle> {
    w.objects.by_watch_id(c.wid)
}

/// What [`satisfy_condition`] did, for the harness log.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Satisfied {
    /// World facts were changed.
    Done(String),
    /// The condition needs no hook: it only fails (a guard) and stays undetermined while its subject lives.
    GuardOnly,
    /// The condition reads a boolean flag only an action of another objective sets; the harness never writes it.
    FlagFromAction(String),
}

/// Makes `c` (a condition of an objective of `team`) true by setting world facts, and nothing else.
pub fn satisfy_condition(w: &mut World, team: i32, c: &ResolvedCondition) -> Satisfied {
    match &c.condition {
        Condition::DestroyAllEnemyUnits
        | Condition::CaptureOrDestroyAllEnemyUnits
        | Condition::DeadOrFledAllEnemyUnits => {
            // `DeadOrFledAllEnemyUnits` reads every mover, the player's included (objective.cpp:~860: no team test).
            let all = matches!(c.condition, Condition::DeadOrFledAllEnemyUnits);
            let list: Vec<_> = if all {
                w.objects.movers.clone()
            } else {
                enemies(w, team)
            };
            for &h in &list {
                destroy_mover(w, h);
            }
            Satisfied::Done(format!("destroyed {} movers", list.len()))
        }
        Condition::DestroyNumberOfEnemyUnits { num }
        | Condition::CaptureOrDestroyNumberOfEnemyUnits { num }
        | Condition::DeadOrFledNumberOfEnemyUnits { num } => {
            let dead_or_fled =
                matches!(c.condition, Condition::DeadOrFledNumberOfEnemyUnits { .. });
            let already = if dead_or_fled {
                w.objects
                    .movers
                    .iter()
                    .filter(|&&h| mover_down(w, h))
                    .count()
            } else {
                enemies(w, team)
                    .iter()
                    .filter(|&&h| mover_down(w, h))
                    .count()
            };
            let need = (*num as usize).saturating_sub(already);
            let list: Vec<_> = if dead_or_fled {
                w.objects.movers.clone()
            } else {
                enemies(w, team)
            };
            let alive: Vec<_> = list
                .into_iter()
                .filter(|&h| !mover_down(w, h))
                .take(need)
                .collect();
            let n = alive.len();
            for h in alive {
                destroy_mover(w, h);
            }
            Satisfied::Done(format!("destroyed {n} more movers ({num} wanted)"))
        }
        Condition::DestroySpecificEnemyUnit { .. }
        | Condition::CaptureOrDestroySpecificEnemyUnit { .. }
        | Condition::DeadOrFledSpecificEnemyUnit { .. } => match wid_handle(w, c) {
            Some(h) => {
                // `MoverIsDeadOrDisabled` is enough for these conditions. A unit another objective of the team captures (tutorial 4:
                // shoot the 'Mech down, then salvage it) is disabled, not destroyed: a destroyed unit fails its capture objective.
                let captured_later = w.mission.objectives.get(team as usize).is_some_and(|t| {
                    t.entries.iter().any(|e| {
                        e.conditions.iter().any(|o| {
                            matches!(o.condition, Condition::CaptureUnit { .. }) && o.wid == c.wid
                        })
                    })
                });
                if captured_later {
                    disable_mover(w, h);
                    Satisfied::Done(format!("disabled mover {}", h.0))
                } else {
                    destroy_mover(w, h);
                    Satisfied::Done(format!("destroyed mover {}", h.0))
                }
            }
            None => Satisfied::Done("no such unit".into()),
        },
        Condition::DestroySpecificStructure { .. } => match wid_handle(w, c) {
            Some(h) => {
                destroy_building(w, h);
                Satisfied::Done(format!("destroyed building {}", h.0))
            }
            None => Satisfied::Done("no such structure".into()),
        },
        Condition::CaptureOrDestroySpecificStructure { .. }
        | Condition::CaptureStructure { .. } => match wid_handle(w, c) {
            Some(h) => {
                capture_building(w, h, team);
                Satisfied::Done(format!("captured building {}", h.0))
            }
            None => Satisfied::Done("no such structure".into()),
        },
        Condition::CaptureUnit { .. } => match wid_handle(w, c) {
            Some(h) => {
                capture_mover(w, h, team);
                Satisfied::Done(format!("captured mover {}", h.0))
            }
            None => Satisfied::Done("no such unit".into()),
        },
        Condition::GuardSpecificUnit { .. } => Satisfied::GuardOnly,
        Condition::GuardSpecificStructure { .. } => {
            // Succeeds when the structure survives to the time limit (objective.cpp:1060).
            if w.mission.time_limit != -1.0 {
                let t = w.mission.time_limit + 1.0;
                advance_time_to(w, t);
                Satisfied::Done(format!(
                    "advanced time past the limit {}",
                    w.mission.time_limit
                ))
            } else {
                Satisfied::GuardOnly
            }
        }
        Condition::MoveAnyUnitToArea { area }
        | Condition::MoveAllUnitsToArea { area }
        | Condition::MoveAllSurvivingUnitsToArea { area }
        | Condition::MoveAllSurvivingMechsToArea { area } => {
            let lance = home_lance(w);
            for &h in &lance {
                move_mover(w, h, area.x, area.y);
            }
            if matches!(c.condition, Condition::MoveAllSurvivingMechsToArea { .. }) {
                // objective.cpp:~1100: living units of other commanders within `MaxExtractUnitDistance` of the area keep it from
                // succeeding. Having them elsewhere is a world fact too.
                let reach = <World as crate::mission::objective_eval::ObjectiveWorld>::max_extract_unit_distance(w);
                let home = w.teams.home as i32;
                let near: Vec<_> = w
                    .objects
                    .movers
                    .iter()
                    .copied()
                    .filter(|&h| {
                        let Some(o) = w.objects.get(h) else {
                            return false;
                        };
                        let commander = match &o.kind {
                            Kind::Mech(m) => i32::from(m.m.commander.0),
                            Kind::Vehicle(v) => i32::from(v.m.commander.0),
                            _ => return false,
                        };
                        let (dx, dy) = (o.c.position.x - area.x, o.c.position.y - area.y);
                        commander != home && dx * dx + dy * dy < reach * reach
                    })
                    .collect();
                // They are moved out of reach rather than destroyed: an allied convoy parked there must stay alive (mc2_10
                // fails its "protect the convoy" objective if it dies).
                for h in near {
                    move_mover(w, h, area.x + reach + 256.0, area.y);
                }
            }
            Satisfied::Done(format!(
                "moved {} movers to ({}, {})",
                lance.len(),
                area.x,
                area.y
            ))
        }
        Condition::BooleanFlagIsSet { flag_id, .. } => Satisfied::FlagFromAction(flag_id.clone()),
        Condition::ElapsedMissionTime { time } => {
            advance_time_to(w, *time + 0.5);
            Satisfied::Done(format!("advanced time to {time}"))
        }
    }
}

/// `ObjectRef` of a condition, when it names an object (for the harness table).
pub fn condition_ref(c: &Condition) -> Option<ObjectRef> {
    match c {
        Condition::DestroySpecificEnemyUnit { unit }
        | Condition::CaptureOrDestroySpecificEnemyUnit { unit }
        | Condition::DeadOrFledSpecificEnemyUnit { unit }
        | Condition::CaptureUnit { unit }
        | Condition::GuardSpecificUnit { unit } => Some(*unit),
        Condition::DestroySpecificStructure { structure }
        | Condition::CaptureOrDestroySpecificStructure { structure }
        | Condition::CaptureStructure { structure }
        | Condition::GuardSpecificStructure { structure } => Some(*structure),
        _ => None,
    }
}

/// Something that runs a mission frame by frame and lets the harness reach its world: the [`Harness`] (a bare [`Sim`]) or the
/// application's live simulation (the headless app tests). Every provided method works through the world only, by world facts.
pub trait Player {
    /// The world.
    fn world(&self) -> &World;
    /// The world, mutable (for the hooks).
    fn world_mut(&mut self) -> &mut World;
    /// One frame of the mission.
    fn step_frame(&mut self) -> Result<(), String>;
    /// The mission is over, or waits on its results screen: objectives no longer matter.
    fn ended(&self) -> bool;
    /// Records what a hook did.
    fn note(&mut self, line: String);

    /// `n` frames.
    fn step_n(&mut self, n: u64) -> Result<(), String> {
        for _ in 0..n {
            self.step_frame()?;
        }
        Ok(())
    }

    /// Number of objectives of the home team.
    fn objective_count(&self) -> usize {
        let w = self.world();
        w.mission
            .objectives
            .get(w.teams.home)
            .map_or(0, |t| t.entries.len())
    }

    /// Runtime status of home objective `i`.
    fn status(&self, i: usize) -> ObjectiveStatus {
        let w = self.world();
        w.mission.objectives[w.teams.home].entries[i].runtime.status
    }

    /// Whether home objective `i` is active.
    fn active(&self, i: usize) -> bool {
        let w = self.world();
        w.mission.objectives[w.teams.home].entries[i].runtime.active
    }

    /// Satisfies every condition of home objective `i` with world facts only. Returns what was done.
    fn satisfy(&mut self, i: usize) -> Vec<Satisfied> {
        let team = self.world().teams.home;
        let conds = self.world().mission.objectives[team].entries[i]
            .conditions
            .clone();
        let mut out = Vec::new();
        for c in &conds {
            let s = satisfy_condition(self.world_mut(), team as i32, c);
            self.note(format!("objective {i}: {:?} -> {s:?}", c.condition));
            out.push(s);
        }
        out
    }

    /// Whether home objective `i` is one the player is told about (not a hidden trigger, not a nav marker).
    fn visible(&self, i: usize) -> bool {
        let w = self.world();
        let e = &w.mission.objectives[w.teams.home].entries[i];
        !e.def.is_hidden_trigger
            && matches!(
                e.def.origin,
                mc2_sim_data::mission::ObjectiveOrigin::Block { .. }
            )
    }

    /// Scheduling class of a visible objective: secondary ones first (they are evaluated every frame only until the primary ones
    /// are done), then the primary ones in file order, and last those that need every previous primary objective done
    /// (`AllPreviousPrimaryObjectivesMustBeComplete`, the extraction).
    fn class(&self, i: usize) -> u8 {
        let w = self.world();
        let def = &w.mission.objectives[w.teams.home].entries[i].def;
        if def.all_previous_primary_objectives_must_be_complete {
            2
        } else if def.priority == 1 {
            1
        } else {
            0
        }
    }

    /// Plays the mission to its end by world facts only: repeatedly takes the next visible objective that the sim has activated and
    /// not yet resolved (secondary first, then primary in file order, the extraction last), satisfies it and steps until the sim
    /// resolves it. Objectives that the mission script activates later are waited for. Returns the order the objectives were
    /// played in; the mission ends by itself 5 s after the last primary objective resolves.
    fn play_to_win(&mut self) -> Result<Vec<usize>, String> {
        let mut played: Vec<usize> = Vec::new();
        let mut idle = 0;
        while !self.ended() && idle < 40 {
            let next = (0..self.objective_count())
                .filter(|&i| {
                    self.visible(i)
                        && self.active(i)
                        && self.status(i) == ObjectiveStatus::Undetermined
                        && !played.contains(&i)
                })
                .min_by_key(|&i| (self.class(i), i));
            match next {
                Some(i) => {
                    idle = 0;
                    played.push(i);
                    self.play_objective(i, 30)?;
                }
                None => {
                    idle += 1;
                    // Nothing visible is active and the script has had time to activate more: an objective that waits on a flag
                    // (`ActivateOnFlag`) is waiting for what the player does to set it (tutorial 5: destroy the Hunchback, a hidden
                    // trigger). Play the objectives whose actions set that flag, by world facts as any other.
                    if idle >= 3 && self.play_flag_activators(&mut played)? {
                        idle = 0;
                        continue;
                    }
                    self.step_n(10)?;
                }
            }
        }
        Ok(played)
    }

    /// Plays the objectives that set the activation flag of a visible, inactive, undetermined objective whose flag is not set yet.
    /// Returns whether any was played.
    fn play_flag_activators(&mut self, played: &mut Vec<usize>) -> Result<bool, String> {
        let home = self.world().teams.home;
        let mut setters: Vec<usize> = Vec::new();
        for i in 0..self.objective_count() {
            let def = &self.world().mission.objectives[home].entries[i].def;
            if !(self.visible(i)
                && !self.active(i)
                && self.status(i) == ObjectiveStatus::Undetermined
                && def.activate_on_flag)
            {
                continue;
            }
            let flag = def.activate_flag_id.clone();
            for j in self.flag_setters(i, &flag) {
                if !played.contains(&j)
                    && self.status(j) == ObjectiveStatus::Undetermined
                    && !setters.contains(&j)
                {
                    setters.push(j);
                }
            }
        }
        for &j in &setters {
            played.push(j);
            self.play_objective(j, 30)?;
        }
        Ok(!setters.is_empty())
    }

    /// The objectives (other than `i`) whose actions set boolean flag `flag` to true.
    fn flag_setters(&self, i: usize, flag: &str) -> Vec<usize> {
        let w = self.world();
        let entries = &w.mission.objectives[w.teams.home].entries;
        (0..entries.len())
            .filter(|&j| {
                j != i
                    && entries[j].actions.iter().any(|a| {
                        matches!(&a.action, mc2_sim_data::mission::Action::SetBooleanFlag { flag_id, value: true } if flag_id == flag)
                    })
            })
            .collect()
    }

    /// Satisfies objective `i` by world facts (first the objectives whose actions set a flag it reads, then its own conditions) and
    /// steps until the sim has resolved it, at most `patience` frames. Returns its status.
    fn play_objective(&mut self, i: usize, patience: u64) -> Result<ObjectiveStatus, String> {
        if self.status(i) != ObjectiveStatus::Undetermined || self.ended() {
            return Ok(self.status(i));
        }
        let home = self.world().teams.home;
        let conditions = self.world().mission.objectives[home].entries[i]
            .conditions
            .clone();
        for c in &conditions {
            if let Condition::BooleanFlagIsSet { flag_id, .. } = &c.condition {
                for j in self.flag_setters(i, flag_id) {
                    self.play_objective(j, patience)?;
                }
            }
        }
        let done = self.satisfy(i);
        debug_assert_eq!(done.len(), conditions.len());
        for _ in 0..patience {
            self.step_frame()?;
            if self.status(i) != ObjectiveStatus::Undetermined || self.ended() {
                break;
            }
        }
        Ok(self.status(i))
    }
}

/// One loaded mission being driven through [`Sim::step`] at a fixed rate.
pub struct Harness {
    /// The simulation under test.
    pub sim: Sim,
    /// Mission name.
    pub mission: String,
    frame: u64,
    /// Distinct `unavailable` notices seen in any frame so far.
    pub unavailable: Vec<FrameUnavailable>,
    /// The `SimEvent::MissionResult` code, once the mission ended.
    pub result: Option<u32>,
    /// Log of what each satisfied condition did.
    pub log: Vec<String>,
}

impl Player for Harness {
    fn world(&self) -> &World {
        &self.sim.world
    }
    fn world_mut(&mut self) -> &mut World {
        &mut self.sim.world
    }
    fn step_frame(&mut self) -> Result<(), String> {
        self.step().map(|_| ())
    }
    fn ended(&self) -> bool {
        self.result.is_some() || self.sim.world.mission.term.awaiting_results
    }
    fn note(&mut self, line: String) {
        self.log.push(line);
    }
}

impl Harness {
    /// Loads `mission` with the production quickstart load (`LoadSpec::quickstart`).
    pub fn load(vfs: &mut Vfs, mission: &str) -> Result<Harness, String> {
        let spec = LoadSpec::quickstart(mission, 1, crate::rng::RngMode::Pcg32);
        let outcome = load_world(vfs, &spec)?;
        Ok(Harness {
            sim: Sim::new(outcome),
            mission: mission.to_string(),
            frame: 0,
            unavailable: Vec::new(),
            result: None,
            log: Vec::new(),
        })
    }

    /// Frames stepped so far.
    pub fn frames(&self) -> u64 {
        self.frame
    }

    /// One frame.
    pub fn step(&mut self) -> Result<FrameOutput, String> {
        let input = FrameInput::fixed(FPS, self.frame, Rc::new(NoneVisible), Vec::new());
        self.frame += 1;
        let out = self.sim.step(input).map_err(|e| e.to_string())?;
        for u in &out.unavailable {
            if !self.unavailable.contains(u) {
                self.unavailable.push(u.clone());
            }
        }
        if out.result != 0 {
            self.result = Some(out.result);
        }
        Ok(out)
    }

    /// After the objectives are done: lets the termination timer run out, plays the results screen and returns the final
    /// `scenarioResult` (`0` when it never ended within `max_frames`).
    pub fn finish(&mut self, max_frames: u64) -> Result<u32, String> {
        for _ in 0..max_frames {
            let out = self.step()?;
            if out.awaiting_results_screen {
                self.sim.results_screen_done();
            }
            if out.result != 0 {
                return Ok(out.result);
            }
        }
        Ok(0)
    }
}

/// Names of the ABL natives that only present (camera, music, speech, video, movie mode). A mission script that reaches one of
/// them unported loses presentation, never a rule; the app host owns these.
const PRESENTATION_NATIVES: [&str; 14] = [
    "getcameraposition",
    "setcameraposition",
    "getcamerarotation",
    "setcamerarotation",
    "setcameragoalposition",
    "setcameragoalrotation",
    "setmoviemode",
    "forcemovieend",
    "playdigitalmusic",
    "playwave",
    "playvideo",
    "getcurrentmusicid",
    "stopvoiceover",
    "isplayingvoiceover",
];

/// More presentation-only natives: speech call-outs, the logistics clock and the music "player in combat" flag.
const PRESENTATION_NATIVES_2: [&str; 3] = ["incallout", "getlogisticstime", "getplayerincombat"];

/// Name of the ABL native `id` (`mc2_abl::NATIVES`).
pub fn native_name(id: u16) -> &'static str {
    mc2_abl::NATIVES
        .get(usize::from(id))
        .map_or("?", |n| n.name)
}

/// A readable label of an unavailable notice: `script:getcameraposition`, `pilot 741:AdjacentAreaCell`, `combat:<hook>`.
pub fn notice_label(u: &FrameUnavailable) -> String {
    match u {
        FrameUnavailable::Ai { part_id, what } => {
            let who = if *part_id == 0 {
                "script".to_string()
            } else {
                format!("pilot {part_id}")
            };
            match what {
                crate::ai::AiUnavailable::Native { id } => format!("{who}:{}", native_name(*id)),
                other => format!("{who}:{other:?}"),
            }
        }
        FrameUnavailable::Combat { hook } => format!("combat:{hook}"),
        other => format!("{other:?}"),
    }
}

/// Whether the notice is a presentation native (see [`PRESENTATION_NATIVES`]); everything else can change a rule or an outcome.
pub fn is_presentation(u: &FrameUnavailable) -> bool {
    match u {
        FrameUnavailable::Ai {
            what: crate::ai::AiUnavailable::Native { id },
            ..
        } => {
            let n = native_name(*id);
            PRESENTATION_NATIVES.contains(&n) || PRESENTATION_NATIVES_2.contains(&n)
        }
        _ => false,
    }
}

/// One objective the player sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleObjective {
    /// Index into the home team's objectives.
    pub index: usize,
    /// `Priority` (1 primary, 2 secondary).
    pub priority: u32,
    /// Condition species with counts (`DestroySpecificEnemyUnitx8`), joined by `+`; failure condition species after ` | fail: `.
    pub spec: String,
    /// Title, trimmed.
    pub title: String,
}

/// Everything one played mission produced.
#[derive(Clone, Debug)]
pub struct MissionRun {
    /// Mission name.
    pub mission: String,
    /// The objective order the harness played (indexes into the home team's objectives).
    pub order: Vec<usize>,
    /// The objectives the player is told about.
    pub visible: Vec<VisibleObjective>,
    /// Every condition species (data name) any objective of the mission reads, success or failure, hidden or not.
    pub species_used: Vec<String>,
    /// `scenarioResult` at the end (`5` = `mis_PLAYER_WIN_BIG`).
    pub result: u32,
    /// Frames stepped.
    pub frames: u64,
    /// Runtime status of every objective just before the results screen finished.
    pub runtime: Vec<ObjectiveStatus>,
    /// The end-of-mission report built from the world (`LogisticsData::setMissionCompleted` inputs).
    pub outcome: crate::mission::outcome::Outcome,
    /// Every distinct unavailable notice of the run.
    pub unavailable: Vec<FrameUnavailable>,
    /// What the hooks did, in order.
    pub log: Vec<String>,
}

impl MissionRun {
    /// The notices that are not presentation-only, as unique labels without pilot ids (`script:convertcoords`,
    /// `pilot:getrelativepositiontoobject`, `combat:<hook>`), sorted. `getsensorsactive` is left out: `SimBindings` implements it
    /// (`mission/bindings.rs`, `get_sensors_active`) but `ai::natives::IMPLEMENTED` does not list it, so the notice is a
    /// bookkeeping false alarm, not an unported behaviour (recorded in `docs/OBJECTIVE-HARNESS.md`).
    pub fn gameplay_unavailable(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .unavailable
            .iter()
            .filter(|u| !is_presentation(u))
            .map(|u| {
                let l = notice_label(u);
                match l.strip_prefix("pilot ") {
                    Some(rest) => format!("pilot:{}", rest.split_once(':').map_or(rest, |x| x.1)),
                    None => l,
                }
            })
            .filter(|l| l != "script:getsensorsactive" && l != "pilot:getsensorsactive")
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

impl Harness {
    /// Loads `mission`, plays it to its win by world facts only ([`Harness::play_to_win`]), lets the termination timer and the results
    /// screen finish, and builds the end-of-mission report.
    pub fn play_mission(vfs: &mut Vfs, mission: &str) -> Result<MissionRun, String> {
        let mut h = Harness::load(vfs, mission)?;
        h.step_n(5)?;
        let order = h.play_to_win()?;
        let runtime: Vec<ObjectiveStatus> = (0..h.objective_count()).map(|i| h.status(i)).collect();
        let result = h.finish(600)?;
        let outcome = crate::mission::outcome::build(&mut h.sim.world);
        let w = h.world();
        let entries = &w.mission.objectives[w.teams.home].entries;
        let mut species_used: Vec<String> = Vec::new();
        for e in entries {
            for c in e.conditions.iter().chain(&e.failure_conditions) {
                let name = c.condition.species().name().to_string();
                if !species_used.contains(&name) {
                    species_used.push(name);
                }
            }
        }
        let visible = (0..h.objective_count())
            .filter(|&i| h.visible(i))
            .map(|i| {
                let e = &entries[i];
                let mut counts: Vec<(&'static str, usize)> = Vec::new();
                for c in &e.conditions {
                    let n = c.condition.species().name();
                    match counts.iter_mut().find(|x| x.0 == n) {
                        Some(x) => x.1 += 1,
                        None => counts.push((n, 1)),
                    }
                }
                let spec: Vec<String> = counts
                    .iter()
                    .map(|(n, c)| {
                        if *c > 1 {
                            format!("{n}x{c}")
                        } else {
                            (*n).to_string()
                        }
                    })
                    .collect();
                let mut spec = spec.join("+");
                if !e.failure_conditions.is_empty() {
                    let f: Vec<&str> = e
                        .failure_conditions
                        .iter()
                        .map(|c| c.condition.species().name())
                        .collect();
                    spec += &format!(" | fail: {}", f.join("+"));
                }
                VisibleObjective {
                    index: i,
                    priority: e.def.priority,
                    spec,
                    title: e.def.title.trim().to_string(),
                }
            })
            .collect();
        Ok(MissionRun {
            mission: mission.to_string(),
            order,
            visible,
            species_used,
            result,
            frames: h.frames(),
            runtime,
            outcome,
            unavailable: h.unavailable.clone(),
            log: h.log.clone(),
        })
    }
}
