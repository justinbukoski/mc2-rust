// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Acceptance seam for P3: a real-input replay of the first campaign mission.
//!
//! This deliberately does not offer a back door into `mc2_sim`: the checked-in
//! script consists only of the same keyboard and pointer events a player can
//! produce.  It runs through the production live simulation (`LiveSim`), so an
//! orchestrator gets a precise first missing seam after every merge.

use crate::headless::{self, HeadlessOptions, HeadlessReport};
use crate::script::{Action, InputScript};
use crate::{hotkeys, hotkeys::Command};
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_platform::keys;
use mc2_ui::draw::GuiRect;
use mc2_ui::tacmap::{TacMap, TerrainBounds};

/// The mc2_01 player replay, in [`InputScript`] format.
pub const SCRIPT: &str = include_str!("../tests/scripts/p3_mc2_01_win.input");

/// The first production-path prerequisite that is not present in this branch.
///
/// Keep this message specific.  The acceptance test and the standalone
/// runner both surface it verbatim, rather than pretending a P1 `NullSim` view
/// can play a mission.
pub const LIVE_SIM_UNAVAILABLE: &str = "P3 acceptance unavailable: mc2-app's production headless path still constructs NullSim; a live mc2_sim::Sim adapter is required before player HUD/map input can reach the mission.";

/// The second production-path prerequisite for P3.  This remains distinct
/// from the state machine: a `Results` value without a rendered Results UI is
/// not a player-visible screen.
pub const RESULTS_SCREEN_UNAVAILABLE: &str = "P3 acceptance unavailable: mc2-app's production UI still has no Results screen; the replay must observe a real Results UI frame, not only GameState::Results.";

/// The required P3 route between physical input and the live simulation.
pub const ORDER_PATH_UNAVAILABLE: &str = "P3 acceptance unavailable: mc2-app's production path has no order route to the simulation; physical player input must reach selection and emitted sim commands (a3-orders).";

/// 600 simulated seconds at the production 30 Hz.
pub const FRAME_BUDGET: u64 = 18_000;

/// A checkpoint the runner reports after its named player action completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProgressStep {
    /// The input-script frame containing the action named by [`Self::label`].
    pub frame: u64,
    pub label: &'static str,
}

/// A progress claim is eligible only after the input frame that performs its
/// action has completed.  `HeadlessRun::frame` is the count after `step`, so
/// the action at frame `N` is complete at `N + 1`.
pub fn progress_ready_after(step: ProgressStep, completed_frames: u64) -> bool {
    completed_frames > step.frame
}

/// Completed player-input gestures of the checked-in one-pass script (`p3_mc2_01_win.input`), kept
/// to validate that script; the replay itself is the outcome-driven `PlayerDriver`, whose
/// checkpoints are its own log lines.  These are intentionally
/// tied to script frames, not simulation outcomes: issuing a tactical order
/// does not prove that a unit has travelled, fired, or destroyed its target.
pub const PROGRESS: &[ProgressStep] = &[
    ProgressStep {
        frame: 2_730,
        label: "completed the HUD Select Visible hotkey gesture for the three-player lance",
    },
    ProgressStep {
        frame: 2_802,
        label: "completed the tactical-map move gesture for the abandoned airfield marker",
    },
    ProgressStep {
        frame: 8_633,
        label: "completed the Default Attack right-click gesture at the airfield patrol",
    },
    ProgressStep {
        frame: 13_133,
        label: "completed the Default Attack right-click gesture at the northern bandits",
    },
    ProgressStep {
        frame: 14_002,
        label: "completed the tactical-map move gesture for the extraction marker",
    },
];

/// The only pointer regions the replay is allowed to use.  The P3 replay is
/// pinned to the shipped 800x600 HUD so these are the actual `TacMap` region
/// and the camera-centred world click, rather than synthetic command syntax.
const TAC_MAP: (f64, f64, f64, f64) = (4.0, 435.0, 165.0, 596.0);
const WORLD_CENTRE: (f64, f64) = (400.0, 300.0);

/// The map targets, in the same order as the map gestures in [`SCRIPT`].  These
/// are the actual player-lance, objective and enemy positions from
/// `data/missions/mc2_01.fit`; the parser checks that each HUD coordinate maps
/// to its intended real-world target rather than merely falling inside the map.
const MAP_TARGETS: &[(&str, (f32, f32))] = &[
    ("player lance", (2496.0, -1941.0)),
    ("abandoned airfield", (3136.0, -789.0)),
    ("hangar", (3221.3, -277.3)),
    ("bandit patrol 1", (3264.0, 1984.0)),
    ("bandit patrol 2", (3434.7, 1984.0)),
    ("bandit patrol 3", (2837.3, 2112.0)),
    ("bandit patrol 4", (2880.0, 2112.0)),
    ("bandit patrol 5", (2922.7, 2112.0)),
    ("bandit patrol 6", (3349.3, 1984.0)),
    ("bandit patrol 7", (3562.7, 1984.0)),
    ("bandit patrol 8", (2922.7, 2197.3)),
    ("north-island repair point", (832.0, 2752.0)),
    ("north bandit 1", (-1045.3, 3093.3)),
    ("north bandit 2", (-1002.7, 3093.3)),
    ("north bandit 3", (-1173.3, 3050.7)),
    ("north bandit 4", (-1216.0, 3050.7)),
    ("extraction", (-4117.3, 513.3)),
];

/// A map pixel spans roughly 80 world units at 800x600; the map conversion
/// itself quantizes another 43 world units.  This bound accepts the target
/// click cells while rejecting a different objective or arbitrary map point.
const MAP_TARGET_TOLERANCE: f32 = 160.0;

fn in_tac_map((x, y): (f64, f64)) -> bool {
    TAC_MAP.0 < x && x < TAC_MAP.2 && TAC_MAP.1 < y && y < TAC_MAP.3
}

fn is_world_centre((x, y): (f64, f64)) -> bool {
    (x.to_bits(), y.to_bits()) == (WORLD_CENTRE.0.to_bits(), WORLD_CENTRE.1.to_bits())
}

/// The P3 HUD's real `buttonlayout800.fit` tactical-map widget.  This is the
/// same 100-vertex mc2_01 map and 800x600 rect used by the merged `mc2-ui`
/// tac-map example; keeping the conversion here makes a script coordinate a
/// checked player gesture rather than an opaque magic number.
fn tac_map_world((x, y): (f64, f64)) -> Option<(f32, f32)> {
    let mut map = TacMap::new(TerrainBounds::from_vertices(100));
    map.set_pos(GuiRect::new(
        TAC_MAP.0 as i32,
        TAC_MAP.1 as i32,
        TAC_MAP.2 as i32,
        TAC_MAP.3 as i32,
    ));
    map.tac_map_to_world(x as i32 - TAC_MAP.0 as i32, y as i32 - TAC_MAP.1 as i32)
}

fn is_hotkey(key: u32, command: Command) -> bool {
    hotkeys::COMMANDS
        .iter()
        .any(|binding| binding.key & 0xff == key && binding.function == command)
}

/// Parses the replay and verifies that every action is a supported player
/// gesture.  `InputScript` itself already verifies the textual grammar; this
/// adds the acceptance-specific HUD/map contract.
pub fn parse_player_script() -> Result<InputScript, String> {
    let script = InputScript::parse(SCRIPT)?;
    let mut select_visible = false;
    let mut default_attack_selected = false;
    let mut map_focused = false;
    let mut pointer = None;
    let mut tactical_moves = 0usize;
    let mut tactical_focuses = 0usize;
    let mut target_attacks = 0usize;
    let mut map_target = 0usize;
    let mut right_down = None;

    for (_, action) in &script.actions {
        match action {
            Action::Event(InputEvent::Key {
                key, pressed: true, ..
            }) if *key == keys::KEY_E && is_hotkey(*key, Command::SelectVisible) => {
                select_visible = true
            }
            Action::Event(InputEvent::Key {
                key, pressed: true, ..
            }) if *key == keys::KEY_D && is_hotkey(*key, Command::DefaultAttack) => {
                default_attack_selected = true
            }
            Action::Event(InputEvent::Key {
                key, pressed: true, ..
            }) => {
                return Err(format!(
                    "P3 acceptance key {key} is not the real Select Visible (E) or Default Attack (D) HUD hotkey"
                ));
            }
            Action::Event(InputEvent::CursorMoved { x, y }) => {
                let point = (*x, *y);
                if !in_tac_map(point) && !is_world_centre(point) {
                    return Err(format!(
                        "P3 acceptance pointer {point:?} is neither the shipped tactical map nor the camera-centred world target"
                    ));
                }
                if in_tac_map(point) && tac_map_world(point).is_none() {
                    return Err(format!(
                        "P3 acceptance tactical-map pointer {point:?} falls outside mc2_01's real map cells"
                    ));
                }
                if in_tac_map(point) {
                    let Some((label, target)) = MAP_TARGETS.get(map_target).copied() else {
                        return Err("P3 acceptance has more tactical-map targets than mc2_01's player route".into());
                    };
                    let mapped = tac_map_world(point).expect("checked tactical-map point");
                    let dx = mapped.0 - target.0;
                    let dy = mapped.1 - target.1;
                    if dx.hypot(dy) > MAP_TARGET_TOLERANCE {
                        return Err(format!(
                            "P3 acceptance tactical-map point {point:?} maps to {mapped:?}, not the {label} target {target:?}"
                        ));
                    }
                    map_target += 1;
                }
                pointer = Some(point);
            }
            // In the Left/Right (AOE) profile, `missiongui.cpp` sets
            // `commandClicked` from `rightMouseReleased`, not from a press
            // (code/missiongui.cpp:760-761, 847-853, 1482-1486).  Keep the
            // complete gesture explicit so a left selection click can never
            // be mistaken for an attack order.
            Action::Event(InputEvent::Button {
                button: MouseButton::Right,
                pressed: true,
            }) => right_down = pointer,
            Action::Event(InputEvent::Button {
                button: MouseButton::Right,
                pressed: false,
            }) => {
                let Some(down) = right_down.take() else {
                    return Err("P3 command right-click released without its press".into());
                };
                let Some(point) = pointer else {
                    return Err("P3 command right-click has no pointer position".into());
                };
                if down != point {
                    return Err("P3 command right-click moved before release; drag orders are not valid replay commands".into());
                }
                if in_tac_map(point) {
                    if !select_visible {
                        return Err("P3 move must follow the Select Visible HUD hotkey".into());
                    }
                    tactical_moves += 1;
                } else if is_world_centre(point)
                    && select_visible
                    && default_attack_selected
                    && map_focused
                {
                    target_attacks += 1;
                    default_attack_selected = false;
                    map_focused = false;
                } else if is_world_centre(point) {
                    return Err(
                        "P3 attack must be a right-click release after Select Visible, a tactical-map camera focus, and Default Attack (D)".into(),
                    );
                } else {
                    return Err(
                        "P3 command must be a right-click release in the shipped tactical-map region or at the camera-centred world target".into(),
                    );
                }
            }
            Action::Event(InputEvent::Button {
                button: MouseButton::Left,
                pressed: true,
            }) => match pointer {
                Some(point) if in_tac_map(point) => {
                    tactical_focuses += 1;
                    map_focused = true;
                }
                Some(point) if is_world_centre(point) => {
                    return Err(
                        "P3 attack must be a right-click release; a left click is selection in the Left/Right mouse profile".into(),
                    );
                }
                _ => return Err(
                    "P3 left click must focus the tactical map; commands use right-click release"
                        .into(),
                ),
            },
            Action::Event(InputEvent::Key { .. }) | Action::Event(InputEvent::Button { .. }) => {}
            Action::Event(other) => {
                return Err(format!(
                    "P3 acceptance script has input outside the HUD/map contract: {other:?}"
                ));
            }
            Action::Resize(..) | Action::Screenshot(..) => {
                return Err(
                    "P3 acceptance script must contain only player input, not test directives"
                        .into(),
                )
            }
        }
    }

    if !select_visible {
        return Err("P3 acceptance script is missing the Select Visible HUD hotkey (E)".into());
    }
    if right_down.is_some() {
        return Err("P3 command right-click is missing its release".into());
    }
    if tactical_moves != 3 || tactical_focuses != 14 || target_attacks != 13 {
        return Err(format!(
            "P3 acceptance expected 3 tactical moves, 14 tactical focuses, and 13 world attacks; got {tactical_moves}, {tactical_focuses}, and {target_attacks}"
        ));
    }
    if map_target != MAP_TARGETS.len() {
        return Err(format!(
            "P3 acceptance used {map_target} of {} real mc2_01 map targets",
            MAP_TARGETS.len()
        ));
    }
    Ok(script)
}

/// Returns the first production-path blocker, after validating the script.
/// This reads the same driver and UI selections used by `mc2 --headless`, so
/// it advances when the P2 driver or P3 Results host lands instead of being a
/// stale constant; it intentionally needs no GPU merely to report a blocker.
pub fn production_ready(_options: &HeadlessOptions) -> Result<(), String> {
    let _ = parse_player_script()?;
    readiness_for(headless::production_capabilities())
}

/// Checks the concrete production selections.  It is public so P3 can test
/// each unavailable seam without pretending a stand-in is a game session.
pub fn readiness_for(capabilities: headless::ProductionCapabilities) -> Result<(), String> {
    if !capabilities.live_sim {
        return Err(LIVE_SIM_UNAVAILABLE.into());
    }
    if !capabilities.orders_reach_sim_commands {
        return Err(ORDER_PATH_UNAVAILABLE.into());
    }
    if !capabilities.results_screen {
        return Err(RESULTS_SCREEN_UNAVAILABLE.into());
    }
    Ok(())
}

/// The 13 objects the six primary objectives require destroyed, by watch id (hangar, the eight
/// "Destroy Bandit Patrol" movers, the four "Locate and Destroy Bandits" movers).  Only the
/// acceptance *report* reads them; the player driver never does.
pub const TARGET_WATCH_IDS: [u32; 13] = [30, 10, 11, 5, 4, 12, 27, 29, 28, 6, 13, 7, 8];

/// Plays the first mission through the production app route and returns what the run reports.
///
/// The player is [`crate::p3_driver::PlayerDriver`]: physical HUD/map input chosen from what the
/// screen shows, repeated until the objectives panel says each objective is done.  The checked-in
/// `p3_mc2_01_win.input` stays the validated one-pass script ([`parse_player_script`]); it cannot
/// say "repeat until destroyed", so it is not what is replayed here.
pub fn replay(options: &HeadlessOptions) -> Result<HeadlessReport, String> {
    replay_logged(options, |_| {})
}

/// [`replay`], also reporting (through `log`) each primary-objective change, each target the first
/// time it is found destroyed, and the final state.
pub fn replay_logged(
    options: &HeadlessOptions,
    mut log: impl FnMut(&str),
) -> Result<HeadlessReport, String> {
    production_ready(options)?;
    let mut options = options.clone();
    options.script = InputScript::default();
    let mut driver = crate::p3_driver::PlayerDriver::new();
    let mut logged_notes = 0usize;
    let mut destroyed = [false; 13];
    let mut disabled = [false; 13];
    let mut last_seen: [String; 13] = std::array::from_fn(|_| String::from("never seen"));
    let report = headless::run_driven(
        &options,
        |run| {
            driver.tick(run);
            for note in &driver.notes[logged_notes..] {
                log(&format!("P3 frame {}: {}", note.turn, note.text));
            }
            logged_notes = driver.notes.len();
            let Some(rt) = run.core.mission.as_ref() else {
                return;
            };
            let objects = &rt.driver.sim().world.objects;
            if rt.turn == 12_100 || rt.turn == 7_000 {
                for pilot in &rt.driver.sim().world.pilots.slots {
                    let Some(o) = objects.get_by_watch_id(pilot.mover) else {
                        continue;
                    };
                    if (512..=514).contains(&o.c.part_id.0) {
                        let mv = match &o.kind {
                            mc2_sim::object::Kind::Mech(m) => Some(&m.m),
                            _ => None,
                        };
                        log(&format!(
                            "P3 frame {}: part {} order {:?} state {:?} vel {:?} crash {:?}",
                            rt.turn,
                            o.c.part_id.0,
                            pilot.cur_order.code,
                            pilot.order_state,
                            mv.map(|m| m.velocity),
                            mv.map(|m| m.crash)
                        ));
                    }
                }
            }
            if rt.turn % 3000 == 0 {
                for (i, id) in TARGET_WATCH_IDS.iter().enumerate() {
                    if let (false, Some(o)) =
                        (destroyed[i], objects.get_by_watch_id(mc2_sim::WatchId(*id)))
                    {
                        log(&format!(
                            "P3 frame {}: target watch {id} alive at ({:.0}, {:.0}) disabled {}",
                            rt.turn,
                            o.c.position.x,
                            o.c.position.y,
                            o.c.is_disabled()
                        ));
                    }
                }
            }
            for (i, id) in TARGET_WATCH_IDS.iter().enumerate() {
                last_seen[i] = match objects.get_by_watch_id(mc2_sim::WatchId(*id)) {
                    None => format!("absent from the object table at frame {}", rt.turn),
                    Some(o) => format!(
                        "present at frame {}, destroyed {}, disabled {}",
                        rt.turn,
                        o.c.is_destroyed(),
                        o.c.is_disabled()
                    ),
                };
                if objects
                    .get_by_watch_id(mc2_sim::WatchId(*id))
                    .is_some_and(|o| o.c.is_disabled())
                {
                    disabled[i] = true;
                }
                if !destroyed[i]
                    && objects
                        .get_by_watch_id(mc2_sim::WatchId(*id))
                        .is_some_and(|o| o.c.is_destroyed())
                {
                    destroyed[i] = true;
                    log(&format!(
                        "P3 frame {}: target watch {id} destroyed",
                        rt.turn
                    ));
                }
            }
        },
        true,
        |_| {},
    )?;
    for (i, id) in TARGET_WATCH_IDS.iter().enumerate() {
        if !destroyed[i] {
            log(&format!(
                "P3 target watch {id} never reported destroyed: {}",
                last_seen[i]
            ));
        }
    }
    let unresolved: Vec<u32> = TARGET_WATCH_IDS
        .iter()
        .zip(destroyed.iter().zip(&disabled))
        .filter(|(_, (d, x))| !**d && !**x)
        .map(|(id, _)| *id)
        .collect();
    let missing: Vec<u32> = TARGET_WATCH_IDS
        .iter()
        .zip(destroyed)
        .filter(|(_, d)| !d)
        .map(|(id, _)| *id)
        .collect();
    let outcome = match report.result {
        Some(5) => format!("mission won at sim frame {}", report.sim_frames),
        other => format!(
            "no win (mission result {other:?}, state {}) after {} sim frames",
            report.state, report.sim_frames
        ),
    };
    log(&format!(
        "P3 final: {} of 13 targets destroyed (not destroyed: {missing:?}), {outcome}",
        13 - missing.len()
    ));
    // C++ objective.cpp:460 counts a disabled unit as done, so the win may leave one disabled;
    // every one of the 13 must still end Destroyed or Disabled when the mission is won.
    if report.result == Some(5) && !unresolved.is_empty() {
        return Err(format!(
            "P3 won with targets neither destroyed nor disabled: {unresolved:?}"
        ));
    }
    for line in &report.sim_unavailable {
        log(&format!("P3 sim unavailable: {line}"));
    }
    Ok(report)
}

/// Validates the exact player-visible P3 outcome.  Both the
/// acceptance test and the standalone runner call this one function so the
/// runner can never claim success on weaker evidence.
pub fn validate_report(report: &HeadlessReport) -> Result<(), String> {
    if report.result != Some(5) {
        return Err(format!(
            "P3 expected mission result = win (mis_PLAYER_WIN_BIG (5)), got {:?} while state was {}",
            report.result, report.state
        ));
    }
    if report.state != "results" || !report.results_screen_drawn {
        return Err(format!(
            "P3 expected Results screen reached, got state {} with results UI drawn {}",
            report.state, report.results_screen_drawn
        ));
    }
    if report.sim_frames > FRAME_BUDGET {
        return Err(format!(
            "P3 exceeded {FRAME_BUDGET}-frame budget: {}",
            report.sim_frames
        ));
    }
    Ok(())
}

/// Reports a completed replay using the same assertions as the acceptance
/// test.  Kept separate from launch/preflight so failure tests need no GPU.
pub fn report_completion(report: &HeadlessReport, mut write: impl FnMut(&str)) -> bool {
    match validate_report(report) {
        Ok(()) => {
            write(&format!(
                "P3 acceptance complete: mission result = win, Results screen reached, {} simulation frames",
                report.sim_frames
            ));
            true
        }
        Err(error) => {
            write(&format!("P3 first failure: {error}"));
            false
        }
    }
}

/// Prints progress in a stable, line-oriented form for the merge orchestrator.
/// The function intentionally has no GPU or window interaction.
pub fn progress_report(mut write: impl FnMut(&str)) -> bool {
    match parse_player_script() {
        Ok(script) => {
            write(&format!(
                "P3 script parsed: {} player-input actions through frame {}",
                script.actions.len(),
                script.last_frame().unwrap_or(0)
            ));
            match std::env::var_os("MC2_DATA") {
                Some(data) => {
                    let mut options = HeadlessOptions::new(data, "mc2_01");
                    options.size = (800, 600);
                    options.frames = FRAME_BUDGET;
                    options.script = script;
                    // Checkpoints are the driver's own: each primary-objective change, each target
                    // found destroyed, each order it gives, all from what the HUD shows.
                    let result = replay_logged(&options, |line| write(line));
                    match result {
                        Ok(report) => report_completion(&report, &mut write),
                        Err(error) => {
                            write(&format!("P3 first failure: {error}"));
                            false
                        }
                    }
                }
                None => {
                    write("P3 first failure: MC2_DATA not set; P3 acceptance needs the shipped mission data");
                    false
                }
            }
        }
        Err(error) => {
            write(&format!("P3 first failure: {error}"));
            false
        }
    }
}
