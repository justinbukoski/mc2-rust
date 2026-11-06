// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tutorials 3 and 4: the required captures made by PLAYER commands (`feat/rust-tutorials`, review 2 blocker 1).
//!
//! A right click on a capturable target goes through the production path: the HUD pick (`hud_data.rs`, C++ `doAttack`'s
//! `target->isCaptureable(Team::home)`, `missiongui.cpp:1811`), the order bridge (`orders.rs`, `AttackTargetKind::Captureable`),
//! the sim's `Capture` order and its completion (`ai/capture.rs`). The tests change positions (the lead mech beside the target, the
//! camera on it: the walk there is not the claim) and never a team; the objective statuses and the tutorial's next objective come
//! from the script and the sim.
//!
//! * `tutorial_3_*`: three structures and three ground vehicles, the objectives follow (all eight succeed). Without the
//!   vehicle pick fix the click on a convoy vehicle is an `AttackObject` order and this test fails.
//! * `tutorial_4_structure_*`: the structure, then the destroy-the-'Mech objective comes up.
//!
//! * `tutorial_4_enemy_mech_*`: the "Salvage Enemy 'Mech" objective is won with the recovery craft (HUD button, placement click,
//!   VTOL, trade to the home team), the C++ route (`BattleMech::isCaptureable` is not a click path, `mech.cpp:8923`); then the
//!   last objective, the sim's WIN_BIG and the campaign's advance to `tut_05` from that outcome.
//!
//! Headless, never a window; skipped without `MC2_DATA` or, with `MC2_SKIP_GPU_TESTS=1`, without a GPU adapter.

#[path = "common/mod.rs"]
mod common;

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::hud::HudRuntime;
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::mission::objective::ObjectiveStatus::Undetermined;
use mc2_sim::object::Kind;
use mc2_sim::PartId;
use std::path::PathBuf;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

fn start_mission(data: PathBuf, mission: &str, sim_frames: u64) -> Option<HeadlessRun<LiveSim>> {
    start_mission_with(data, mission, sim_frames, None)
}

fn start_mission_with(
    data: PathBuf,
    mission: &str,
    sim_frames: u64,
    launch: Option<mc2_campaign::LoadSpec>,
) -> Option<HeadlessRun<LiveSim>> {
    let mut opts = HeadlessOptions::new(&data, mission);
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    opts.game.campaign_launch = launch;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e) if common::skip_gpu_test_without_adapter(&e) => {
            return None;
        }
        Err(e) => panic!("{e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    // The mission's first script frame enters its opening movie.  Dismiss it before advancing
    // to the HUD case's requested frame: that exercises the same production ESC route without
    // changing a `start(5)` fixture into a several-hundred-frame post-intro world.
    run.step(None).unwrap();
    dismiss_opening_movie(&mut run);
    step_to(&mut run, sim_frames);
    Some(run)
}

/// The shipped `mc2_01` script opens in movie mode.  That correctly gates the HUD (including
/// Select Visible) until the player presses Escape, so HUD tests must take the same input route
/// before asserting ordinary player controls.
fn dismiss_opening_movie(run: &mut HeadlessRun<LiveSim>) {
    if !run
        .core
        .mission
        .as_ref()
        .is_some_and(|mission| mission.sim_presentation.movie_mode)
    {
        return;
    }
    key(run, "escape");
    for _ in 0..400 {
        if !run
            .core
            .mission
            .as_ref()
            .is_some_and(|mission| mission.sim_presentation.movie_mode)
        {
            // The GUI frame runs before the sim's script on the transition frame.
            run.step(None).unwrap();
            return;
        }
        run.step(None).unwrap();
    }
    panic!("mc2_01's ESC dismissal never ended the opening movie");
}

fn sim_frame(run: &HeadlessRun<LiveSim>) -> u64 {
    run.core.mission.as_ref().unwrap().clock.frame()
}

fn step_to(run: &mut HeadlessRun<LiveSim>, frame: u64) {
    while sim_frame(run) < frame {
        run.step(None).unwrap();
    }
}

fn hud(run: &HeadlessRun<LiveSim>) -> &HudRuntime {
    run.core
        .mission
        .as_ref()
        .unwrap()
        .hud
        .as_ref()
        .expect("the live simulation has a HUD")
}

fn world(run: &HeadlessRun<LiveSim>) -> &mc2_sim::World {
    &run.core.mission.as_ref().unwrap().driver.sim().world
}

/// The part ids of the home commander's movers on the GUI, straight from the simulation.
fn lance(w: &mc2_sim::World) -> Vec<i32> {
    w.objects
        .movers
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .filter(|o| o.exists())
        .filter(|o| match &o.kind {
            Kind::Mech(m) => i32::from(m.m.commander.0) == HOME_COMMANDER && m.m.flags.on_gui,
            Kind::Vehicle(v) => i32::from(v.m.commander.0) == HOME_COMMANDER && v.m.flags.on_gui,
            _ => false,
        })
        .map(|o| o.c.part_id.0)
        .collect()
}

fn mouse(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    run.event(InputEvent::CursorMoved { x, y });
}

fn key(run: &mut HeadlessRun<LiveSim>, name: &str) {
    let code = script::key_code(name).expect("key name");
    for e in script::key_events(code, true) {
        run.event(e);
    }
    run.step(None).unwrap();
    for e in script::key_events(code, false) {
        run.event(e);
    }
    run.step(None).unwrap();
}

fn player_order(run: &HeadlessRun<LiveSim>, part: i32) -> Option<mc2_sim::command::TacticalOrder> {
    let w = world(run);
    let o = w
        .objects
        .find_by_part_id(PartId(part))
        .and_then(|h| w.objects.get(h))?;
    let slot = match &o.kind {
        Kind::Mech(m) => m.m.pilot_slot,
        Kind::Vehicle(v) => v.m.pilot_slot,
        _ => return None,
    };
    let pilot = w.pilots.get(slot)?;
    [&pilot.cur_order, &pilot.tac_orders[1]]
        .into_iter()
        .find(|o| o.origin == mc2_sim::command::OrderOrigin::Player)
        .cloned()
}

/// The home objectives' statuses, in script order.
fn statuses(run: &HeadlessRun<LiveSim>) -> Vec<mc2_sim::mission::objective::ObjectiveStatus> {
    let w = world(run);
    w.mission.objectives[w.teams.home]
        .entries
        .iter()
        .map(|e| e.runtime.status)
        .collect()
}

/// A player's capture. The camera is centred on the target and the lance's lead mech is put beside it (positions only, never a
/// team: the walk there is not the claim). Then the lance is selected with the select-visible key and the capture is a right
/// click on the target: the HUD's `doAttack` classification (`hud_data.rs`), the order bridge (`orders.rs`), the sim's `Capture`
/// order and its completion. Returns once the target belongs to the home team.
fn capture_by_click(run: &mut HeadlessRun<LiveSim>, target: mc2_sim::ObjectHandle) {
    let target_is_building = matches!(
        world(run).objects.get(target).unwrap().kind,
        Kind::Building(_)
    );
    let mut found = None;
    for (dx, dy) in [
        (0.0, 0.0),
        (-70.0, 0.0),
        (70.0, 0.0),
        (0.0, 70.0),
        (0.0, -70.0),
        (-140.0, 0.0),
        (140.0, 0.0),
    ] {
        let at = world(run).objects.get(target).unwrap().c.position;
        {
            let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
            let lead = w.objects.find_by_part_id(PartId(512)).expect("lead mech");
            let z = w.objects.get(lead).unwrap().c.position.z;
            mc2_sim::mover::set_position(
                w,
                lead,
                mc2_sim::math::Vec3 {
                    x: at.x + dx,
                    y: at.y + dy,
                    z,
                },
            );
        }
        {
            let rt = run.core.mission.as_mut().unwrap();
            let ground = rt.view.ground();
            rt.rig.follow = None;
            rt.rig.set_position(
                mc2_render::math::Vec3::new(at.x, at.y, 0.0),
                false,
                &ground,
                1.0 / 30.0,
            );
        }
        // a structure far from every unit wakes and is drawn once the camera and a unit are there
        for _ in 0..30 {
            run.step(None).unwrap();
        }
        let click = if target_is_building {
            independent_click(run, target)
        } else {
            vehicle_click(run, target)
        };
        if let Some(c) = click.filter(|c| c.0 > 0.0 && c.0 < 1920.0 && c.1 > 0.0 && c.1 < 1080.0) {
            found = Some(c);
            break;
        }
    }
    let click = found.unwrap_or_else(|| panic!("no clear point on {target:?} in view"));
    let lance = select_lance(run);
    let watch = world(run).objects.watch_id_peek(target);
    mouse(run, f64::from(click.0), f64::from(click.1));
    run.step(None).unwrap();
    run.event(InputEvent::Button {
        button: MouseButton::Right,
        pressed: true,
    });
    run.step(None).unwrap();
    run.event(InputEvent::Button {
        button: MouseButton::Right,
        pressed: false,
    });
    run.step(None).unwrap();
    let ordered: Vec<_> = lance.iter().filter_map(|&p| player_order(run, p)).collect();
    assert!(
        !ordered.is_empty()
            && ordered
                .iter()
                .all(|o| o.code == mc2_sim::command::OrderCode::Capture && o.target == watch),
        "the click did not order the lance to capture: {ordered:?}"
    );
    let home = world(run).teams.home as i32;
    for _ in 0..4000 {
        if world(run).objects.get(target).unwrap().team_id() == home {
            return;
        }
        run.step(None).unwrap();
    }
    let w = world(run);
    let t = w.objects.get(target).unwrap();
    panic!(
        "the capture order was given but {target:?} (team {}) was not captured; mech at {:?}, target at {:?}, orders {:?}",
        t.team_id(),
        w.objects.get(w.objects.find_by_part_id(PartId(512)).unwrap()).unwrap().c.position,
        t.c.position,
        lance.iter().filter_map(|&p| player_order(run, p)).collect::<Vec<_>>()
    );
}

fn triangle_holds(t: &[f32; 6], p: (f32, f32)) -> bool {
    let e = |ax: f32, ay: f32, bx: f32, by: f32| (bx - ax) * (p.1 - ay) - (by - ay) * (p.0 - ax);
    let d = [
        e(t[0], t[1], t[2], t[3]),
        e(t[2], t[3], t[4], t[5]),
        e(t[4], t[5], t[0], t[1]),
    ];
    d.iter().all(|v| *v >= 0.0) || d.iter().all(|v| *v <= 0.0)
}

/// A point on a drawn face of the building that no mover's drawn face covers.
fn independent_click(run: &HeadlessRun<LiveSim>, h: mc2_sim::ObjectHandle) -> Option<(f32, f32)> {
    let boxes = run.core.mission.as_ref().unwrap().driver.pick_boxes();
    boxes
        .structure_triangles
        .get(&h.0)?
        .iter()
        .map(|t| ((t[0] + t[2] + t[4]) / 3.0, (t[1] + t[3] + t[5]) / 3.0))
        .find(|&c| {
            !boxes
                .movers
                .values()
                .any(|b| c.0 >= b[0] && c.0 <= b[2] && c.1 >= b[1] && c.1 <= b[3])
                && !boxes
                    .mover_triangles
                    .values()
                    .any(|ts| ts.iter().any(|t| triangle_holds(t, c)))
        })
}

/// The centre of a vehicle's box when no other mover's box covers it.
fn vehicle_click(run: &HeadlessRun<LiveSim>, h: mc2_sim::ObjectHandle) -> Option<(f32, f32)> {
    let boxes = run.core.mission.as_ref().unwrap().driver.pick_boxes();
    let part = world(run).objects.get(h)?.c.part_id.0;
    let b = *boxes.movers.get(&part)?;
    Some(((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0))
}

/// The select-visible key: the lance's mechs in view are selected (the camera shows the lead mech beside the target).
fn select_lance(run: &mut HeadlessRun<LiveSim>) -> Vec<i32> {
    key(run, "E");
    let got = hud(run).selection.selected.clone();
    let all = lance(world(run));
    assert!(
        !got.is_empty() && got.iter().all(|p| all.contains(p)),
        "the select-visible key selected {got:?} of the lance {all:?}"
    );
    got
}

fn watch_target(run: &HeadlessRun<LiveSim>, wid: u32) -> mc2_sim::ObjectHandle {
    world(run)
        .objects
        .by_watch_id(mc2_sim::WatchId(wid))
        .expect("the objective's target")
}

#[test]
fn tutorial_4_structure_is_captured_by_a_player_click() {
    let Some(data) = data() else { return };
    let Some(mut run) = start_mission(data, "tut_04", 5) else {
        return;
    };
    let before = statuses(&run);
    let building = watch_target(&run, 5);
    assert_ne!(world(&run).objects.get(building).unwrap().team_id(), 0);
    capture_by_click(&mut run, building);
    for _ in 0..60 {
        run.step(None).unwrap();
    }
    let after = statuses(&run);
    eprintln!("tut_04 statuses {before:?} -> {after:?}");
    assert_eq!(
        after[0],
        mc2_sim::mission::objective::ObjectiveStatus::Successful
    );
    // the tutorial moves on: the next objective ("shoot the 'Mech down") is now up
    let w = world(&run);
    assert!(
        w.mission.objectives[w.teams.home].entries[1].runtime.active,
        "the destroy-the-'Mech objective follows the capture"
    );
}

/// Tutorial 3 asks for three structures and three vehicles (`CaptureSpecificStructure`, `CaptureSpecificUnit`): each is
/// captured by a player right click, the objectives report it.
#[test]
fn tutorial_3_structures_and_vehicles_are_captured_by_player_clicks() {
    let Some(data) = data() else { return };
    let Some(mut run) = start_mission(data, "tut_03", 5) else {
        return;
    };
    let home = world(&run).teams.home as i32;
    let mut remaining: Vec<u32> = vec![25, 26, 27, 7, 8, 9];
    for _round in 0..40 {
        let done = statuses(&run)[2..=5]
            .iter()
            .all(|s| *s == mc2_sim::mission::objective::ObjectiveStatus::Successful);
        if remaining.is_empty() && done {
            break;
        }
        // The script wakes a structure when its objective comes up: capture what the player can capture now.
        let ready: Vec<u32> = remaining
            .iter()
            .copied()
            .filter(|&wid| {
                let w = world(&run);
                let t = watch_target(&run, wid);
                mc2_sim::ai::capture::is_captureable(w, t, home)
            })
            .collect();
        for wid in ready {
            let target = watch_target(&run, wid);
            capture_by_click(&mut run, target);
            remaining.retain(|&r| r != wid);
        }
        // A move objective that is up: the lead mech is put in its area (a position, as the walk is not the claim here).
        let areas: Vec<(f32, f32)> = {
            let w = world(&run);
            w.mission.objectives[w.teams.home]
                .entries
                .iter()
                .filter(|e| e.runtime.active && e.runtime.status == Undetermined)
                .flat_map(|e| e.conditions.iter())
                .filter_map(|c| match &c.condition {
                    mc2_sim_data::mission::Condition::MoveAnyUnitToArea { area } => {
                        Some((area.x, area.y))
                    }
                    _ => None,
                })
                .collect()
        };
        for (x, y) in areas {
            let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
            let lead = w.objects.find_by_part_id(PartId(512)).expect("lead mech");
            let z = w.objects.get(lead).unwrap().c.position.z;
            mc2_sim::mover::set_position(w, lead, mc2_sim::math::Vec3 { x, y, z });
        }
        for _ in 0..60 {
            run.step(None).unwrap();
        }
    }
    assert!(remaining.is_empty(), "never capturable: {remaining:?}");
    for _ in 0..120 {
        run.step(None).unwrap();
    }
    let after = statuses(&run);
    eprintln!("tut_03 statuses {after:?}");
    // the three vehicles, the three structures, and the move objectives that frame them: the tutorial is complete
    assert!(
        after
            .iter()
            .all(|s| *s == mc2_sim::mission::objective::ObjectiveStatus::Successful),
        "tutorial 3 objectives: {after:?}"
    );
}

fn left_click(run: &mut HeadlessRun<LiveSim>, x: f32, y: f32) {
    mouse(run, f64::from(x), f64::from(y));
    run.step(None).unwrap();
    for pressed in [true, false] {
        run.event(InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        });
        run.step(None).unwrap();
    }
}

fn right_click(run: &mut HeadlessRun<LiveSim>, x: f32, y: f32) {
    mouse(run, f64::from(x), f64::from(y));
    run.step(None).unwrap();
    for pressed in [true, false] {
        run.event(InputEvent::Button {
            button: MouseButton::Right,
            pressed,
        });
        run.step(None).unwrap();
    }
}

/// Tutorial 4's last objective, "Salvage Enemy 'Mech" (`CaptureSpecificUnit`): the C++ player route is the recovery craft
/// (`canSalvage`/`doSalvage`, `canRecover`, `beginVtol`, missiongui.cpp:1324-1331, 4461-4483), not a capture click. The campaign
/// launch gives the roster's recovery pilots. The mech is shot down by the test (disabled: the objective before it asks for it; the
/// shooting is not the claim); the recovery craft is bought with the HUD button, placed with a right click (the AOE profile's command click) on the mech, flies in,
/// recovers it and trades it to the home team; no team is assigned by the test.
#[test]
fn tutorial_4_enemy_mech_is_salvaged_by_the_players_recovery_craft() {
    use mc2_ui::control::{ControlId, VehicleKind};
    let Some(data) = data() else { return };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&data).expect("mount");
    let game = std::sync::Arc::new(mc2_campaign::GameData::load(&mut vfs).expect("game data"));
    let mut logistics = mc2_campaign::Logistics::with_campaign(game, "tutorial").unwrap();
    for _ in 0..3 {
        logistics
            .apply_outcome(&mc2_campaign::interface::Outcome::won())
            .unwrap();
    }
    assert_eq!(logistics.current_mission(), "tut_04");
    let spec = logistics.load_spec(mc2_campaign::interface::MissionOptions::default());
    assert!(
        !spec.roster.is_empty(),
        "the tutorial has the campaign roster for its recovery pilots"
    );
    let Some(mut run) = start_mission_with(data, "tut_04", 5, Some(spec)) else {
        return;
    };
    let building = watch_target(&run, 5);
    capture_by_click(&mut run, building);
    for _ in 0..60 {
        run.step(None).unwrap();
    }
    // the second objective's enemy 'Mech is shot down
    let mech = {
        let w = world(&run);
        w.mission.objectives[w.teams.home].entries[2]
            .conditions
            .iter()
            .find_map(|c| match &c.condition {
                mc2_sim_data::mission::Condition::CaptureUnit { .. } => Some(c.wid),
                _ => None,
            })
            .expect("the salvage objective's unit")
    };
    let mech = watch_target(&run, mech.0);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        mc2_sim::test_hooks::disable_mover(w, mech);
        w.mission.resource_points = 100_000;
    }
    for _ in 0..30 {
        run.step(None).unwrap();
    }
    let enemy_team = world(&run).objects.get(mech).unwrap().team_id();
    assert_ne!(enemy_team, world(&run).teams.home as i32);
    let snap = hud(&run).snapshot();
    eprintln!(
        "pilots_remaining={} points={}",
        snap.pilots_remaining,
        world(&run).mission.resource_points
    );
    // bring the lead mech and the camera to the 'Mech so it is in view and seen
    let at = world(&run).objects.get(mech).unwrap().c.position;
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let lead = w.objects.find_by_part_id(PartId(512)).expect("lead mech");
        let z = w.objects.get(lead).unwrap().c.position.z;
        mc2_sim::mover::set_position(
            w,
            lead,
            mc2_sim::math::Vec3 {
                x: at.x - 70.0,
                y: at.y,
                z,
            },
        );
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.follow = None;
        rt.rig.set_position(
            mc2_render::math::Vec3::new(at.x, at.y, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..30 {
        run.step(None).unwrap();
    }
    select_lance(&mut run);
    let tab = hud(&run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.id == Some(ControlId::VehicleTab))
        .map(|b| b.rect)
        .expect("vehicle tab");
    let centre = |run: &HeadlessRun<LiveSim>, r: mc2_ui::control::ControlRect| {
        hud(run).hud_to_game(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
    };
    let (x, y) = centre(&run, tab);
    left_click(&mut run, x, y);
    let button = hud(&run)
        .control_panel()
        .vehicles
        .iter()
        .find(|b| b.kind == Some(VehicleKind::RecoveryTeam))
        .map(|b| (b.rect, b.state))
        .expect("recovery button");
    eprintln!("recovery button {:?}", button.1);
    let (x, y) = centre(&run, button.0);
    left_click(&mut run, x, y);
    assert_eq!(
        hud(&run).control_panel().selected_vehicle,
        Some(VehicleKind::RecoveryTeam),
        "the recovery button arms"
    );
    let (cx, cy) = vehicle_click(&run, mech).expect("the 'Mech is in view");
    right_click(&mut run, cx, cy);
    assert!(
        world(&run).vtol.in_flight(HOME_COMMANDER),
        "the click on the disabled 'Mech sends the recovery craft"
    );
    let home = world(&run).teams.home as i32;
    for _ in 0..3000 {
        if world(&run).objects.get(mech).unwrap().team_id() == home {
            break;
        }
        run.step(None).unwrap();
    }
    assert_eq!(world(&run).objects.get(mech).unwrap().team_id(), home);
    for _ in 0..60 {
        run.step(None).unwrap();
    }
    let after = statuses(&run);
    eprintln!("tut_04 statuses {after:?}");
    assert_eq!(
        after[2],
        mc2_sim::mission::objective::ObjectiveStatus::Successful
    );

    // The last objective, "MoveAllSurvivingMechsToArea": the lance, the salvaged 'Mech among it, reaches the area (the walk
    // is placed, as in the capture tests: it is not the claim). The mission is then won by the sim's own termination and the
    // campaign moves on to tut_05 from the outcome the production host would hand it.
    let area = {
        let w = world(&run);
        w.mission.objectives[w.teams.home].entries[3]
            .conditions
            .iter()
            .find_map(|c| match &c.condition {
                mc2_sim_data::mission::Condition::MoveAllSurvivingMechsToArea { area } => {
                    Some((area.x, area.y))
                }
                _ => None,
            })
            .expect("the last objective's area")
    };
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let home = w.teams.home as i32;
        let movers: Vec<_> = w.objects.movers.clone();
        for h in movers {
            if matches!(w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(_)))
                && w.objects.get(h).unwrap().team_id() == home
            {
                mc2_sim::test_hooks::move_mover(w, h, area.0, area.1);
            }
        }
    }
    for _ in 0..1200 {
        if world(&run).mission.term.scenario_result != 0 {
            break;
        }
        run.step(None).unwrap();
    }
    let after = statuses(&run);
    eprintln!("tut_04 statuses at the end {after:?}");
    assert!(
        after
            .iter()
            .all(|s| *s == mc2_sim::mission::objective::ObjectiveStatus::Successful),
        "every tut_04 objective is won: {after:?}"
    );
    assert_eq!(
        world(&run).mission.term.scenario_result,
        5,
        "mis_PLAYER_WIN_BIG"
    );
    let outcome = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        mc2_app::outcome::to_campaign(mc2_sim::mission::outcome::build(w))
    };
    logistics.apply_outcome(&outcome).unwrap();
    assert_eq!(logistics.current_mission(), "tut_05");
}
