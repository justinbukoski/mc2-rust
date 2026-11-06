// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The frame loop: [`Sim::step`] runs one `Mission::update` + the simulation-relevant parts of `DoGameLogic`.

use std::rc::Rc;

use crate::command::PlayerCommand;
use crate::digest::LineBuilder;
use crate::events::SimEvent;
use crate::time::FixedStep;
use crate::visibility::Visibility;
use crate::World;

/// Inputs of one frame.
pub struct FrameInput {
    /// Measured frame rate (`frameRate`; sets `frameLength = 1/frameRate`, capped at 0.25 s). With [`FixedStep`] this is the fixed rate.
    pub frame_rate: f32,
    /// Virtual `timeGetTime()` in ms (feeds `scenarioTime`). [`FixedStep::now_ms`] produces the values the oracle uses.
    pub now_ms: u32,
    /// Camera-derived inputs (see [`crate::visibility`]).
    pub visibility: Rc<dyn Visibility>,
    /// Player commands arriving this frame.
    pub commands: Vec<PlayerCommand>,
}

impl FrameInput {
    /// Input for frame `frame` of a fixed-rate run.
    pub fn fixed(
        fps: u32,
        frame: u64,
        visibility: Rc<dyn Visibility>,
        commands: Vec<PlayerCommand>,
    ) -> FrameInput {
        FrameInput {
            frame_rate: fps as f32,
            now_ms: FixedStep { fps }.now_ms(frame),
            visibility,
            commands,
        }
    }
}

/// What a frame produced.
#[derive(Debug, Default)]
pub struct FrameOutput {
    /// Presentation events (drained from `World::events`).
    pub events: Vec<SimEvent>,
    /// `scenarioResult` (`mis_PLAYING` while the mission runs).
    pub result: u32,
    /// The termination timer has elapsed and the host has not yet called [`Sim::results_screen_done`]: the mission is waiting on
    /// its results screen (`mission.cpp:568-569`). The host should show the results and then call `results_screen_done`.
    pub awaiting_results_screen: bool,
    /// Behaviour that is not ported and was reached in THIS frame (every frame, not once): a frame with entries here is not a
    /// source-faithful C++ frame for the named pilots / sensors, whatever `Ok` says.
    pub unavailable: Vec<FrameUnavailable>,
}

/// One unported behaviour reached during a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameUnavailable {
    /// A pilot reached unported AI (e.g. the decision trees) this frame.
    Ai {
        /// Mission part id of the mover the pilot flies.
        part_id: i32,
        /// What was reached.
        what: crate::ai::AiUnavailable,
    },
    /// The visibility producer marked object `inView` as unavailable
    /// ([`Visibility::object_in_view_available`](crate::visibility::Visibility::object_in_view_available)): mover and structure
    /// `inView` are an approximation this frame, so every digest field and update gate that reads it is not a parity result.
    ObjectInView,
    /// No longer produced: sensor visual line of sight is fully ported (`GameObject::lineOfSight`, gameobj.cpp:1846). Kept so
    /// exhaustive matches in callers keep compiling.
    VisualLos {
        /// Mission part id of the sensor's mover.
        sensor_part_id: i32,
        /// Mission part id of the target.
        target_part_id: i32,
    },
    /// A combat behaviour whose lane is not ported was reached this frame.
    Combat {
        /// The unported combat hook that was reached.
        hook: &'static str,
    },
}

/// A running simulation.
pub struct Sim {
    /// All state.
    pub world: World,
    allow_incomplete: bool,
}

/// Why a frame was not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    /// The world was loaded with unported lane B/C/D hooks (the names, as in `World::unported_load_hooks`); simulating it
    /// would silently run without their state.
    Unavailable { missing_hooks: Vec<&'static str> },
}

impl std::fmt::Display for SimError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SimError::Unavailable { missing_hooks } => write!(
                f,
                "simulation unavailable: incomplete mission load, unported hooks: {}",
                missing_hooks.join("; ")
            ),
        }
    }
}

impl std::error::Error for SimError {}

/// The cursor query of `MissionInterfaceManager::update` (missiongui.cpp:747-752): on game-select terrain,
/// `Team::home->teamLineOfSight(wPos, 0.0f)`. Only its side effect on the home movers' cached LOS nodes matters to the sim.
fn cursor_line_of_sight(w: &mut World, at: crate::math::Vec3) {
    if w.terrain.is_game_select_position(&at) {
        let home = w.teams.home as i32;
        crate::object::hooks::team_line_of_sight(w, home, at, 0.0);
    }
}

/// The mission-script block of an unpaused frame (`if (missionBrain)`, mission.cpp:530-541). Returns `Some(9999)` when the script
/// ends the run at once (mission.cpp:536-537; the caller returns before the termination logic). Otherwise the script's result
/// overwrites `terminationResult` (mission.cpp:539). Without a brain the block does not run, so `terminationResult` keeps what the
/// termination logic set last (the force-death `LOST_BIG`, mission.cpp:558-563).
pub fn run_script_phase(w: &mut World) -> Option<u32> {
    let script_result = crate::mission::bindings::run_mission_brain(w)?;
    if script_result == 9999 {
        w.mission.term.result = 9999;
        w.mission.term.scenario_result = 9999;
        return Some(9999);
    }
    w.mission.term.result = script_result;
    None
}

impl Sim {
    /// Wrap a loaded mission. An incomplete load (see [`LoadOutcome`](crate::mission::init::LoadOutcome)) can be digested but
    /// [`Sim::step`] refuses it unless [`Sim::allow_incomplete`] is set.
    pub fn new(loaded: crate::mission::init::LoadOutcome) -> Sim {
        Sim {
            world: loaded.world,
            allow_incomplete: false,
        }
    }

    /// Opt in to stepping a world with unported load hooks. Only the frame-0 baseline tests use this.
    pub fn allow_incomplete(mut self) -> Sim {
        self.allow_incomplete = true;
        self
    }

    /// Replay one sensor frame when mover state has already been supplied by an oracle trace.
    /// This advances the shared clock, resets the per-frame LOS cache, and updates sensors while
    /// leaving the unported mover and combat frame hooks out of the replay.
    pub fn step_sensors_with_replayed_movers(
        &mut self,
        frame_rate: f32,
        now_ms: u32,
        visibility: Rc<dyn Visibility>,
    ) {
        let w = &mut self.world;
        w.visibility = visibility;
        w.clock.begin_frame(frame_rate, now_ms, w.mission.paused);
        w.mission.actual_time = w.clock.scenario_time;
        w.objects.mover_los_table.iter_mut().for_each(|b| *b = -1);
        if !w.mission.paused {
            crate::ai::sensors::update(w);
        }
    }

    /// One frame, in the order of `Mission::update` (mission.cpp:318-663) preceded by the `DoGameLogic` frame-length computation.
    /// Skipped because they have no simulation effect: debug hotkeys, in-mission save, texture manager, camera update,
    /// weather, craters (visual), `terrainTextures`, GUI update except player orders and the script-visible callout state.
    pub fn step(&mut self, input: FrameInput) -> Result<FrameOutput, SimError> {
        if !self.allow_incomplete && !self.world.load_is_complete() {
            return Err(SimError::Unavailable {
                missing_hooks: self.world.unported_load_hooks.clone(),
            });
        }
        let w = &mut self.world;
        w.ai_unavailable_frame.clear();
        w.ai_native_calls_frame.clear();
        w.combat_unavailable_frame.clear();
        w.visibility = input.visibility;
        // DoGameLogic: frameLength from frameRate; Mission::update: turn++, scenarioTime from timeGetTime deltas (not while paused).
        let paused = w.mission.paused;
        w.clock.begin_frame(input.frame_rate, input.now_ms, paused);
        w.rng.0.borrow_mut().sim.set_trace_frame(w.clock.turn);
        w.mission.actual_time = w.clock.scenario_time;
        // memset(moverLineOfSightTable, -1)
        w.objects.mover_los_table.iter_mut().for_each(|b| *b = -1);

        // DoGameLogic: timerManager->update() runs before Mission::update (mechcmd2.cpp:2112), paused or not; it only reads
        // frameLength, so its position relative to the turn++/scenarioTime lines of begin_frame does not matter.
        w.mission.timers.update(w.clock.frame_length);

        // missionInterface->update(): player orders, issued after the scenarioTime advance and the LOS cache reset
        // (Mover::handleTacticalOrder reads scenarioTime; the oracle issues scripted orders at this exact point), and the
        // `updateVTol` support-vehicle arrival, a sim effect.
        for cmd in &input.commands {
            // CROSS-CRATE (mc2-app HUD): the pause toggle of `MissionInterfaceManager::togglePause`; `Mission::update` re-reads
            // `isPaused()` after the player orders, below.
            if let PlayerCommand::SetPaused(paused) = cmd {
                w.mission.paused = *paused;
                continue;
            }
            if let PlayerCommand::ForceMovieEnd = cmd {
                crate::mission::natives_presentation::player_force_movie_end(w);
                continue;
            }
            if let PlayerCommand::SetAoeStyle(aoe) = cmd {
                w.presentation.aoe_style = *aoe;
                continue;
            }
            if let PlayerCommand::CursorLineOfSight { .. } = cmd {
                continue;
            }
            if let PlayerCommand::ScriptedHit { part, damage } = cmd {
                let target = w.objects.find_by_part_id(*part);
                if let Some(target) = target {
                    let shot = crate::combat::WeaponShotInfo {
                        attacker: crate::ids::WatchId::NONE,
                        master_id: 0,
                        damage: *damage,
                        hit_location: 0,
                        entry_angle: 0.0,
                    };
                    crate::object::hooks::weapon_hit(w, target, &shot);
                }
                continue;
            }
            crate::ai::issue_player_command(w, cmd);
        }
        // `MissionInterfaceManager::update` (`missiongui.cpp:455-530`): a live callout advances before the mission
        // script runs, allowing its `InCallout` wait to complete. Movie mode returns from that GUI update before this block.
        crate::mission::natives_presentation::advance_callout(w);
        // missiongui.cpp:742-754: the cursor query, after the callout block of the same GUI update
        for cmd in &input.commands {
            if let PlayerCommand::CursorLineOfSight { at } = cmd {
                cursor_line_of_sight(w, *at);
            }
        }
        // missionInterface->update(): updateVTol
        crate::mission::vtol::update(w);
        // PathManager->update(): up to 6 calcMovePath (runs even while paused)
        crate::mover::PathManager::update(w);
        // land->geometry(): the camera marks object blocks/vertices active -> comes from `w.visibility`.
        // The pause flag is re-read here: a SetPaused command changes `isPaused()` after the scenarioTime check above.
        if !w.mission.paused {
            // ObjectManager->update(true,true,true) (paused: updateAppearancesOnly, a lane F entry point)
            crate::object::ObjectStore::update(w);
            // SensorManager->update()
            crate::ai::sensors::update(w);
            // ObjectManager->updateCollisions()
            crate::object::collision::update_collisions(w);
            if let Some(result) = run_script_phase(w) {
                // The script asked for the end of the game at once (mission.cpp:536-537, 9999). The rest of Mission::update
                // is not simulated, so the camera's end of the frame (updateLetterboxAndFade) does not run either; with the
                // mission over, nothing is presented.
                return Ok(FrameOutput {
                    events: std::mem::take(&mut w.events),
                    result,
                    awaiting_results_screen: false,
                    unavailable: crate::ai::frame_unavailable(w),
                });
            }
        } else {
            crate::object::update_appearances_only(w);
        }
        // eye->update() (`mission.cpp:478`) runs after `missionInterface->update()` and so after the whole rest of the
        // update. Its `updateLetterboxAndFade` (`mclib/camera.cpp:1440-1490`, called from `Camera::update`, `:1601`) steps the
        // letterbox and fade the script just changed, so a movie mode started by this frame's script has its first letterbox
        // step in this frame, and one the script closed this frame closes now. The callout block of the GUI update
        // (`missiongui.cpp:455-530`) belongs to next frame's GUI update, so it does not advance here.
        crate::mission::natives_presentation::advance_camera(w);
        // Termination, objectives and the time limit run every frame, paused or not (mission.cpp:545-663).
        crate::mission::update_termination(w);
        Ok(FrameOutput {
            events: std::mem::take(&mut w.events),
            result: w.mission.term.scenario_result,
            awaiting_results_screen: w.mission.term.awaiting_results,
            unavailable: crate::ai::frame_unavailable(w),
        })
    }

    /// The results screen has finished: `ControlGui::resultsDone()` (controlgui.cpp:2901) returns true from now on. The GUI is not
    /// ported, so the host calls this when its results screen is done. Until then the mission stays `mis_PLAYING` after the 5 s
    /// timer (mission.cpp:566-594), reports [`FrameOutput::awaiting_results_screen`], and emits no `MissionResult` event.
    ///
    /// TODO(lane A, app call; see porting-log): the mc2-app results screen must call this. Nothing calls it yet.
    pub fn results_screen_done(&mut self) {
        crate::mission::results_screen_done(&mut self.world);
    }

    /// The `H` header line of a digest (mission, fps, seed, RNG mode code as in the oracle: 0 msvc, 1 glibc, 2 pcg).
    pub fn digest_header(mission: &str, fps: u32, seed: u32, mode: crate::rng::RngMode) -> String {
        format!("H {mission} {fps} {seed} {}\n", mode.oracle_code())
    }

    /// The digest record of the current state (`F` ... `Z`, see oracle/DIGEST.md). `frame` is the frame number to print.
    pub fn digest(&self, frame: i64, level: u32) -> String {
        self.digest_rows(frame, level, false)
    }

    /// [`Sim::digest`] plus the lane D `P`/`KM` rows the oracle writes with `--lane-d-rows` (before the `Z` line, as it does).
    pub fn digest_with_pilots(&self, frame: i64, level: u32) -> String {
        self.digest_rows(frame, level, true)
    }

    fn digest_rows(&self, frame: i64, level: u32, pilots: bool) -> String {
        let w = &self.world;
        let rng = w.rng.0.borrow();
        let mut lines = Vec::new();
        let mut f = LineBuilder::new("F");
        f.i(frame)
            .i(w.clock.turn)
            .f(w.clock.scenario_time)
            .f(w.clock.frame_length)
            .i(rng.sim.draws() as i64)
            .x(rng.sim.state_hash())
            .i(rng.visual.draws() as i64)
            .x(rng.visual.state_hash());
        lines.push(f.build());
        // VA line: lane A computes it from the visibility the frame was given.
        crate::object::digest(w, level, &mut lines);
        // `P`/`KM` rows are opt-in (the oracle writes them only with `--lane-d-rows`), so the default digest stays the shared one.
        if pilots {
            crate::ai::digest_pilots(w, level, &mut lines);
            crate::mission::digest(w, level, &mut lines);
        }
        // Z is signed, like the oracle: -1 means the script told the engine not to evaluate objectives.
        lines.push(format!("Z {}", w.mission.term.result));
        let mut s = lines.join("\n");
        s.push('\n');
        s
    }
}
