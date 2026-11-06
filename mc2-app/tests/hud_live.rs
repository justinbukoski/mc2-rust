// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! The mission HUD on the live simulation (headless, never a window; skipped without `MC2_DATA`
//! or when the GPU adapter is missing and `MC2_SKIP_GPU_TESTS=1`). Every test runs `mc2_01` at
//! 1920x1080 through `GameCore` with `LiveSim`, drives it with real mouse and keyboard events,
//! and reads the result back from the app's state.

#[path = "common/mod.rs"]
mod common;

use mc2_app::audio::Cue;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::hud::HudRuntime;
use mc2_app::hud_data::HudText;
use mc2_app::layers::{OrderContext, OrderHook};
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_audio::ids::sfx;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_render::math::Vec3;
use mc2_sim::combat::HOME_COMMANDER;
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim::{PartId, PlayerCommand};
use mc2_ui::mission_hud::{HitTarget, HudIntent};
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

/// A started `mc2_01` with the live simulation at 1920x1080, at least `sim_frames` frames in
/// and with its opening movie dismissed through the production ESC path.
fn start(sim_frames: u64) -> Option<HeadlessRun<LiveSim>> {
    start_in(data()?, sim_frames)
}

fn start_in(data: PathBuf, sim_frames: u64) -> Option<HeadlessRun<LiveSim>> {
    start_mission(data, "mc2_01", sim_frames)
}

fn start_mission(data: PathBuf, mission: &str, sim_frames: u64) -> Option<HeadlessRun<LiveSim>> {
    let mut opts = HeadlessOptions::new(&data, mission);
    opts.size = (1920, 1080);
    opts.game.cursor = false;
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

fn button(run: &mut HeadlessRun<LiveSim>, pressed: bool) {
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed,
    });
}

/// A left click: move, press, release, each on its own game frame.
fn click(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    mouse(run, x, y);
    run.step(None).unwrap();
    button(run, true);
    run.step(None).unwrap();
    button(run, false);
    run.step(None).unwrap();
}

/// An AOE-profile command click: move, press, release, each on its own game frame.
fn right_click(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    mouse(run, x, y);
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

/// Hold a command hotkey through the mouse release that completes a world command.
/// `doEject` reads both inputs in that one GUI frame (`missiongui.cpp:2311-2323`).
fn command_click_with_key(
    run: &mut HeadlessRun<LiveSim>,
    name: &str,
    x: f64,
    y: f64,
    before_release: impl FnOnce(&mut HeadlessRun<LiveSim>),
) {
    mouse(run, x, y);
    run.step(None).unwrap();
    run.event(InputEvent::Button {
        button: MouseButton::Right,
        pressed: true,
    });
    run.step(None).unwrap();
    before_release(run);
    let code = script::key_code(name).expect("key name");
    for event in script::key_events(code, true) {
        run.event(event);
    }
    run.event(InputEvent::Button {
        button: MouseButton::Right,
        pressed: false,
    });
    run.step(None).unwrap();
    for event in script::key_events(code, false) {
        run.event(event);
    }
    run.step(None).unwrap();
}

/// Where the camera shows a unit, in game pixels.
fn screen_of(run: &HeadlessRun<LiveSim>, part: i32) -> (f32, f32) {
    let rt = run.core.mission.as_ref().unwrap();
    let u = hud(run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.part_id == part)
        .unwrap();
    let (s, _) = rt
        .rig
        .view()
        .project_z(Vec3::new(u.position.0, u.position.1, u.position.2));
    (s[0], s[1])
}

#[test]
fn the_unit_bar_shows_the_real_player_lance_with_the_sims_damage_and_pilots() {
    struct HitOnce {
        part: i32,
        frame: u64,
        done: bool,
    }
    impl OrderHook<PlayerCommand> for HitOnce {
        fn update(&mut self, ctx: &OrderContext<'_>, out: &mut Vec<PlayerCommand>) {
            if !self.done && ctx.frame >= self.frame {
                self.done = true;
                out.push(PlayerCommand::ScriptedHit {
                    part: PartId(self.part),
                    damage: 6.0,
                });
            }
        }
    }
    let Some(mut run) = start(5) else { return };
    let expected = lance(world(&run));
    assert!(
        expected.len() >= 3,
        "mc2_01 starts with a lance: {expected:?}"
    );
    let target = expected[0];
    run.core.orders = Box::new(HitOnce {
        part: target,
        // The live fixture dismisses mc2_01's opening movie before HUD input is legal, so its
        // absolute mission frame is no longer a small fixed number.  Schedule the test hook a
        // few production frames from the playable fixture instead.
        frame: sim_frame(&run) + 3,
        done: false,
    });
    let before = hud(&run).snapshot().clone();
    let mut shown: Vec<i32> = before.units.iter().map(|u| u.part_id).collect();
    shown.sort_unstable();
    let mut want = expected.clone();
    want.sort_unstable();
    assert_eq!(shown, want, "the HUD lists the sim's player movers");
    assert_eq!(
        hud(&run).unit_bar().icon_count(),
        expected.len(),
        "the unit bar has an icon per player unit"
    );
    for u in &before.units {
        assert!(
            hud(&run).unit_bar().slot_of(u.part_id as u32).is_some(),
            "{} has an icon",
            u.part_id
        );
    }

    // pilots: the name string and the portrait come from the pilot's profile in the sim
    for u in &before.units {
        let o = world(&run)
            .objects
            .find_by_part_id(PartId(u.part_id))
            .and_then(|h| world(&run).objects.get(h))
            .unwrap();
        let slot = match &o.kind {
            Kind::Mech(m) => m.m.pilot_slot,
            Kind::Vehicle(v) => v.m.pilot_slot,
            _ => unreachable!(),
        };
        let load = &world(&run).mission.pilot_loads[usize::from(slot)];
        let g = &load.profile.as_ref().expect("pilot profile").general;
        assert_eq!(u.pilot_name, HudText::Resource(g.desc_index as u32));
        assert_eq!(u.bar.pilot.photo, g.picture_index.max(0) as u32);
        assert!(!u.bar.destroyed && u.bar.bar_status > 0.0 && u.bar.bar_status <= 1.0);
    }

    // damage: every armour location the bar shows equals the sim's
    for u in &before.units {
        let o = world(&run)
            .objects
            .find_by_part_id(PartId(u.part_id))
            .and_then(|h| world(&run).objects.get(h))
            .unwrap();
        let l = mc2_sim::combat::loadout_of(o).unwrap();
        if let mc2_ui::unitbar::Armor::Mech(a) = u.bar.armor {
            for (i, loc) in [
                a.head, a.ctorso, a.ltorso, a.rtorso, a.larm, a.rarm, a.lleg, a.rleg, a.rctorso,
                a.rltorso, a.rrtorso,
            ]
            .iter()
            .enumerate()
            {
                assert_eq!(
                    (loc.cur, loc.max),
                    (l.armor[i].0, f32::from(l.armor[i].1)),
                    "{i}"
                );
            }
        }
    }
    let armor_of = |snap: &mc2_app::hud_data::HudSnapshot| match snap
        .units
        .iter()
        .find(|u| u.part_id == target)
        .unwrap()
        .bar
        .armor
    {
        mc2_ui::unitbar::Armor::Mech(a) => a.head.cur,
        mc2_ui::unitbar::Armor::Vehicle(v) => v.front.cur,
    };
    let bar_before = before
        .units
        .iter()
        .find(|u| u.part_id == target)
        .unwrap()
        .bar
        .bar_status;
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    let after = hud(&run).snapshot();
    let head_after = armor_of(after);
    assert!(
        head_after < armor_of(&before),
        "the scripted hit shows in the bar: {} -> {head_after}",
        armor_of(&before)
    );
    let o = world(&run)
        .objects
        .find_by_part_id(PartId(target))
        .and_then(|h| world(&run).objects.get(h))
        .unwrap();
    assert_eq!(
        head_after,
        mc2_sim::combat::loadout_of(o).unwrap().armor[0].0,
        "and equals the sim's armour"
    );
    let bar_after = after
        .units
        .iter()
        .find(|u| u.part_id == target)
        .unwrap()
        .bar
        .bar_status;
    assert!(
        bar_after < bar_before,
        "the health bar follows: {bar_before} -> {bar_after}"
    );
}

/// The live snapshot is the only bridge into the production HUD.  It carries the unit-bar
/// recovery/death fields and applies `GameTacMap::render`'s contact filters before the widgets see
/// the data (`forcegroupbar.cpp:173-180`, `gametacmap.cpp:282-410`).
#[test]
fn the_production_hud_snapshot_keeps_recovery_state_and_filters_tac_contacts() {
    let Some(mut run) = start(130) else { return };
    let (home, enemy, sensor_owner, home_part, enemy_pos, sensor_pos) = {
        let w = world(&run);
        let home = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| match &o.kind {
                    Kind::Mech(m) => i32::from(m.m.commander.0) == HOME_COMMANDER,
                    Kind::Vehicle(v) => i32::from(v.m.commander.0) == HOME_COMMANDER,
                    _ => false,
                })
            })
            .expect("mc2_01 has a player mover");
        let enemy = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| match &o.kind {
                    Kind::Mech(m) => i32::from(m.m.commander.0) != HOME_COMMANDER,
                    Kind::Vehicle(v) => i32::from(v.m.commander.0) != HOME_COMMANDER,
                    _ => false,
                })
            })
            .expect("mc2_01 has an enemy mover");
        let sensor_owner = w
            .objects
            .buildings
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.exists() && !o.c.is_disabled() && !o.c.is_destroyed() && o.team_id() >= 0
                })
            })
            .expect("mc2_01 has a live building for the sensor fixture");
        let home_part = w.objects.get(home).unwrap().c.part_id.0;
        let enemy_pos = w.objects.get(enemy).unwrap().c.position;
        let sensor_pos = w.objects.get(sensor_owner).unwrap().c.position;
        (
            home,
            enemy,
            sensor_owner,
            home_part,
            (enemy_pos.x, enemy_pos.y),
            (sensor_pos.x, sensor_pos.y),
        )
    };
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let recover_buddy = w.objects.watch_id(home);
        let home_obj = w.objects.get_mut(home).unwrap();
        match &mut home_obj.kind {
            Kind::Mech(m) => m.m.recover_buddy = recover_buddy,
            Kind::Vehicle(v) => v.m.recover_buddy = recover_buddy,
            _ => unreachable!("a mover is mech or vehicle"),
        }
        mc2_sim::combat::loadout_of_mut(home_obj)
            .unwrap()
            .stats
            .cause_of_death = mc2_sim::combat::fx::death::POWER_USED_UP as i32;
        let home_team = w.teams.home;
        w.sensors.team_contact_status.entry(enemy).or_default()[home_team] = 1;
        w.objects.get_mut(enemy).unwrap().c.status = ObjectStatus::ShutDown;
        w.sensors.pool.push(mc2_sim::ai::sensors::Sensor {
            owner: sensor_owner,
            team_index: 0,
            range: 120.0,
            has_los_capability: true,
            not_shutdown: true,
            ..Default::default()
        });
        // `GameTacMap::render` draws only the sensors in each team's list: register this one.
        let id = w.sensors.pool.len() - 1;
        let owner_team = w.objects.get(sensor_owner).unwrap().team_id() as usize;
        w.sensors.teams[owner_team].push(id);
    }

    // `mission_tick` updates the HUD before the sim's later sensor scan, so this frame observes
    // precisely the production snapshot assembled from the state above.
    run.step(None).unwrap();
    let snap = hud(&run).snapshot();
    let home = snap
        .units
        .iter()
        .find(|unit| unit.part_id == home_part)
        .expect("home mover remains on the HUD");
    assert!(home.bar.recover_pending);
    assert!(home.bar.power_used_up);
    assert!(
        !snap.contacts.iter().any(|unit| unit.pos == enemy_pos),
        "a contacted shutdown mover is omitted from the tactical map"
    );
    assert!(snap.contacts.iter().any(|unit| {
        unit.sensor_entry
            && unit.pos == sensor_pos
            && (unit.sensor_range_m - 120.0).abs() < f32::EPSILON
    }));

    // The same sensor leaves its team's list: it is still in the pool but no longer drawn.
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        for list in w.sensors.teams.iter_mut() {
            list.clear();
        }
    }
    let w = world(&run);
    let snap = mc2_app::hud_data::snapshot(w, &Default::default());
    assert!(
        !snap
            .contacts
            .iter()
            .any(|unit| unit.sensor_entry && unit.pos == sensor_pos),
        "an unregistered building sensor is not drawn"
    );

    // Restore the mover, then give it an active ECM component. This is a separate production
    // frame so neither omission can accidentally mask the other.
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let enemy_obj = w.objects.get_mut(enemy).unwrap();
        enemy_obj.c.status = ObjectStatus::Normal;
        mc2_sim::combat::loadout_of_mut(enemy_obj).unwrap().critical
            [mc2_sim::mission::loadout::crit::ECM] = 0;
    }
    run.step(None).unwrap();
    assert!(
        !hud(&run)
            .snapshot()
            .contacts
            .iter()
            .any(|unit| unit.pos == enemy_pos),
        "a contacted ECM mover is omitted from the tactical map"
    );
}

/// The completed S9a capabilities must not be advertised as unavailable in a real mission run.
/// This is deliberately a 2,000-frame production HUD run, not an inspection of the static notice
/// list: every frame exercises the snapshot, objective, chat and draw paths that own the notices.
#[test]
fn mc2_01_two_thousand_frames_has_no_completed_hud_residue_notices() {
    let Some(run) = start(2_000) else { return };
    assert!(sim_frame(&run) >= 2_000);
    let unavailable = hud(&run).unavailable_summary();
    for residue in [
        "recoverBuddyWID",
        "message-playing",
        "ECM / shutdown",
        "activation time",
        "radio message text",
        "selection brackets and the order cursor",
        "sliding status frame",
        "objectives are all decided",
        "every other support-vehicle button reads disabled",
        "mine laying, repair, salvage, guard tower and queued-order",
    ] {
        assert!(
            unavailable.iter().all(|note| !note.contains(residue)),
            "completed HUD residue {residue:?} remained in a 2,000-frame mc2_01 run: {unavailable:?}"
        );
    }
}

#[test]
fn a_drag_select_with_the_real_mouse_selects_the_lance_in_app_state() {
    let Some(mut run) = start(130) else { return };
    let ids = lance(world(&run));
    assert!(
        hud(&run).selection.selected.is_empty(),
        "nothing selected at the start"
    );
    let pts: Vec<(f32, f32)> = ids.iter().map(|&p| screen_of(&run, p)).collect();
    let (x0, x1) = (
        pts.iter().map(|p| p.0).fold(f32::MAX, f32::min) - 50.0,
        pts.iter().map(|p| p.0).fold(f32::MIN, f32::max) + 50.0,
    );
    let (y0, y1) = (
        pts.iter().map(|p| p.1).fold(f32::MAX, f32::min) - 50.0,
        pts.iter().map(|p| p.1).fold(f32::MIN, f32::max) + 50.0,
    );
    assert!(
        x0 > 0.0 && y0 > 0.0 && x1 < 1920.0,
        "the lance is on screen: {pts:?}"
    );
    assert_eq!(
        hud(&run).hit_test((x0 as i32, y0 as i32)),
        HitTarget::World,
        "the drag starts over the world view"
    );
    mouse(&mut run, f64::from(x0), f64::from(y0));
    run.step(None).unwrap();
    button(&mut run, true);
    run.step(None).unwrap();
    mouse(
        &mut run,
        f64::from((x0 + x1) / 2.0),
        f64::from((y0 + y1) / 2.0),
    );
    run.step(None).unwrap();
    mouse(&mut run, f64::from(x1), f64::from(y1.min(1000.0)));
    run.step(None).unwrap();
    button(&mut run, false);
    run.step(None).unwrap();
    run.step(None).unwrap();
    let mut got = hud(&run).selection.selected.clone();
    got.sort_unstable();
    let mut want = ids.clone();
    want.sort_unstable();
    assert_eq!(got, want, "the drag box selected exactly the lance");
    // and the unit bar shows them selected
    let bar_selected = hud(&run)
        .snapshot()
        .units
        .iter()
        .filter(|u| hud(&run).selection.is_selected(u.part_id))
        .count();
    assert_eq!(bar_selected, want.len());

    // a click on empty ground clears the selection
    click(&mut run, 60.0, 300.0);
    assert!(
        hud(&run).selection.selected.is_empty(),
        "a click on empty ground deselects"
    );
}

#[test]
fn the_select_visible_hot_key_selects_the_lance() {
    let Some(mut run) = start(130) else { return };
    key(&mut run, "E");
    let mut got = hud(&run).selection.selected.clone();
    got.sort_unstable();
    let mut want = lance(world(&run));
    want.sort_unstable();
    assert_eq!(got, want);
}

#[test]
fn select_visible_hot_key_ignores_movers_left_behind_by_a_camera_pan() {
    let Some(mut run) = start(130) else { return };
    let parts = lance(world(&run));
    assert!(parts.len() >= 2, "mc2_01 has multiple home movers");
    key(&mut run, "E");
    let mut selected_before_pan = hud(&run).selection.selected.clone();
    selected_before_pan.sort_unstable();
    let mut expected_before_pan = parts.clone();
    expected_before_pan.sort_unstable();
    assert_eq!(selected_before_pan, expected_before_pan);
    let focus = world(&run)
        .objects
        .find_by_part_id(PartId(parts[0]))
        .and_then(|h| world(&run).objects.get(h))
        .expect("focus mover")
        .c
        .position;
    {
        let mission = run.core.mission.as_mut().expect("mission");
        mission.rig.follow = None;
        mission.rig.look_target = -1;
        // Pan to the opposite map edge. Re-centering on the first lance mover can be a no-op
        // when the mission camera already follows it. Clear the scripted look target too, since
        // the next camera update would otherwise restore that follow and undo the pan.
        mission.rig.set_position(
            Vec3::new(
                if focus.x >= 0.0 { -20_000.0 } else { 20_000.0 },
                if focus.y >= 0.0 { -20_000.0 } else { 20_000.0 },
                focus.z,
            ),
            false,
            &mission.view.ground(),
            1.0 / 30.0,
        );
        mission
            .rig
            .zoom_value(mission.rig.settings.altitude_minimum);
    }
    run.step(None).unwrap();
    // The HUD consumes the visibility computed at the end of the previous frame.
    run.step(None).unwrap();
    let currently_visible: Vec<_> = hud(&run)
        .snapshot()
        .units
        .iter()
        .filter(|u| u.in_view)
        .map(|u| u.part_id)
        .collect();
    let previously_visible_offscreen: Vec<_> = parts
        .iter()
        .copied()
        .filter(|part| !currently_visible.contains(part))
        .collect();
    assert!(
        !previously_visible_offscreen.is_empty(),
        "the camera pan moves at least one previously selected lance member offscreen"
    );

    key(&mut run, "E");
    let mut selected = hud(&run).selection.selected.clone();
    let mut expected = currently_visible;
    selected.sort_unstable();
    expected.sort_unstable();
    assert_eq!(
        selected, expected,
        "the production select-visible hot key uses current viewport visibility"
    );
}

#[test]
fn the_pause_button_stops_the_simulation_and_a_second_click_resumes_it() {
    let Some(mut run) = start(130) else { return };
    let rect = hud(&run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.cxx_id == 11)
        .expect("the pause button")
        .rect;
    let (cx, cy) =
        hud(&run).hud_to_game(((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2));
    let (cx, cy) = (f64::from(cx), f64::from(cy));
    let snapshot = |run: &HeadlessRun<LiveSim>| {
        let w = world(run);
        let pos: Vec<_> = w
            .objects
            .movers
            .iter()
            .filter_map(|&h| w.objects.get(h))
            .map(|o| (o.c.position.x.to_bits(), o.c.position.y.to_bits()))
            .collect();
        (w.clock.scenario_time, pos)
    };
    let running_a = snapshot(&run);
    step_to(&mut run, 160);
    let running_b = snapshot(&run);
    assert!(
        running_b.0 > running_a.0,
        "the clock runs: {} -> {}",
        running_a.0,
        running_b.0
    );
    click(&mut run, cx, cy);
    assert!(hud(&run).paused(), "the pause button paused the HUD");
    assert!(run.core.mission.as_ref().unwrap().paused);
    let paused_a = snapshot(&run);
    let turn_a = world(&run).clock.turn;
    for _ in 0..30 {
        run.step(None).unwrap();
    }
    let paused_b = snapshot(&run);
    assert_eq!(
        paused_a, paused_b,
        "while paused the scenario clock and every mover stand still"
    );
    assert!(
        world(&run).clock.turn >= turn_a + 30,
        "the turn counter still runs (Mission::update does), only the objects stop"
    );
    // a real P key resumes (the same toggle as the button)
    key(&mut run, "P");
    assert!(!hud(&run).paused());
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    let resumed = snapshot(&run);
    assert!(
        resumed.0 > paused_b.0,
        "the clock runs again after resuming"
    );
}

#[test]
fn a_resize_keeps_a_paused_mission_paused() {
    let Some(mut run) = start(130) else { return };
    key(&mut run, "P");
    assert!(hud(&run).paused() && run.core.mission.as_ref().unwrap().paused);
    let frozen = |run: &HeadlessRun<LiveSim>| {
        let w = world(run);
        let pos: Vec<_> = w
            .objects
            .movers
            .iter()
            .filter_map(|&h| w.objects.get(h))
            .map(|o| (o.c.position.x.to_bits(), o.c.position.y.to_bits()))
            .collect();
        (w.clock.scenario_time, pos)
    };
    let before = frozen(&run);
    let res_before = hud(&run).resolution();
    // 1920x1200 loads a different shipped layout than 1920x1080
    run.resize(1920, 1200);
    for _ in 0..30 {
        run.step(None).unwrap();
    }
    assert_ne!(hud(&run).resolution(), res_before, "the layout was rebuilt");
    assert!(hud(&run).paused(), "the rebuilt HUD is still paused");
    assert!(run.core.mission.as_ref().unwrap().paused);
    assert_eq!(before, frozen(&run), "movers and the clock stay frozen");
}

fn hold(run: &mut HeadlessRun<LiveSim>, names: &[&str], pressed: bool) {
    for m in names {
        for e in script::key_events(script::key_code(m).unwrap(), pressed) {
            run.event(e);
        }
    }
}

fn select_lance(run: &mut HeadlessRun<LiveSim>) -> Vec<i32> {
    key(run, "E");
    let mut got = hud(run).selection.selected.clone();
    got.sort_unstable();
    let mut all = lance(world(run));
    all.sort_unstable();
    assert_eq!(got, all, "the select-visible key selected the lance");
    all
}

fn selected(run: &HeadlessRun<LiveSim>) -> Vec<i32> {
    hud(run).selection.selected.clone()
}

fn force_group(run: &HeadlessRun<LiveSim>, parts: &[i32]) -> Vec<Option<u8>> {
    let w = world(run);
    parts
        .iter()
        .map(|part| w.mission.unit_groups.get(&PartId(*part)).copied())
        .collect()
}

#[test]
fn initial_group_one_uses_final_part_ids_through_the_live_hud() {
    let Some(mut run) = start(0) else { return };
    let initial_lance = lance(world(&run));
    assert!(
        !initial_lance.is_empty(),
        "mc2_01 supplies player movers with the Mover::init group-1 default"
    );
    assert_eq!(
        force_group(&run, &initial_lance),
        vec![Some(1); initial_lance.len()],
        "the saved mission map keys group 1 by the final mission part ids"
    );
    for part_id in &initial_lance {
        let unit = hud(&run)
            .snapshot()
            .units
            .iter()
            .find(|unit| unit.part_id == *part_id)
            .expect("every live player mover has a HUD unit");
        assert_eq!(
            unit.bar.groups,
            1 << 1,
            "HUD group bit for final part id {part_id}"
        );
    }

    // A real plain-1 event goes through GameCore -> HUD -> selection.  C++ constructs movers
    // in group 1 (`mover.cpp:2217,2278`) before mission loading assigns their final ids
    // (`mission.cpp:2875-2876`), so this proves the Rust state moved that membership too.
    key(&mut run, "1");
    let mut selected_group = selected(&run);
    selected_group.sort_unstable();
    let mut expected = initial_lance;
    expected.sort_unstable();
    assert_eq!(
        selected_group, expected,
        "plain 1 selects the initial group"
    );
}

fn queue_executing(run: &HeadlessRun<LiveSim>, part: i32) -> bool {
    let w = world(run);
    let h = w.objects.find_by_part_id(PartId(part)).unwrap();
    let slot = match &w.objects.get(h).unwrap().kind {
        Kind::Mech(mech) => mech.m.pilot_slot,
        Kind::Vehicle(vehicle) => vehicle.m.pilot_slot,
        _ => panic!("force-group member is not a mover"),
    };
    w.pilots.get(slot).unwrap().queue_executing
}

fn refused(run: &HeadlessRun<LiveSim>, action: &str) -> bool {
    hud(run)
        .unavailable_summary()
        .iter()
        .any(|s| s.contains(&format!("order {action}:")))
}

/// The player order (`MoveToPoint`, `Stop`, ...) a mover's pilot holds, from the sim.
fn player_order_code(run: &HeadlessRun<LiveSim>, part: i32) -> Option<mc2_sim::command::OrderCode> {
    player_order(run, part).map(|o| o.code)
}

/// The player-origin order a mover's pilot holds, from the sim.
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

/// The ABL `setInvulnerable` native writes the mission flag (covered by the
/// script-binding test in `mc2-sim`). Through `GameCore`, that live flag must
/// reach `BridgeContext` so the C++ ejection guard is visible to a player.
#[test]
fn invulnerable_home_commander_cannot_eject_until_the_script_flag_is_cleared() {
    let Some(mut run) = start(130) else { return };
    let part = lance(world(&run))[0];
    let (x, y) = screen_of(&run, part);

    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .record_inputs(true);
    command_click_with_key(&mut run, "END", f64::from(x), f64::from(y), |run| {
        run.core
            .mission
            .as_mut()
            .unwrap()
            .driver
            .sim_mut()
            .world
            .mission
            .invulnerable = true;
    });
    assert!(refused(&run, "eject"));
    assert_ne!(
        player_order_code(&run, part),
        Some(mc2_sim::command::OrderCode::Eject),
        "the refused command never reaches the live simulation"
    );
    let rejected_frames = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .driver
        .recorded_frames()
        .len();
    let (x, y) = screen_of(&run, part);

    command_click_with_key(&mut run, "END", f64::from(x), f64::from(y), |run| {
        run.core
            .mission
            .as_mut()
            .unwrap()
            .driver
            .sim_mut()
            .world
            .mission
            .invulnerable = false;
    });
    assert!(
        run.core
            .mission
            .as_ref()
            .unwrap()
            .driver
            .recorded_frames()
            .iter()
            .skip(rejected_frames)
            .flat_map(|frame| &frame.commands)
            .any(|command| matches!(
                command,
                PlayerCommand::Order { movers, order, .. }
                    if movers == &vec![PartId(part)]
                        && order.code == mc2_sim::command::OrderCode::Eject
            )),
        "clearing the script-controlled flag emits the live eject order"
    );
}

#[test]
fn a_player_move_click_reaches_the_selected_mechs_in_the_same_frame() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    for &p in &lance {
        assert_eq!(player_order_code(&run, p), None, "no player order yet");
    }
    // AOE profile (the default): the right click on the ground is the move order
    mouse(&mut run, 960.0, 400.0);
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
    for &p in &lance {
        assert_eq!(
            player_order_code(&run, p),
            Some(mc2_sim::command::OrderCode::MoveToPoint),
            "part {p} was ordered to move"
        );
    }
    assert_eq!(
        selected(&run).len(),
        lance.len(),
        "the order keeps the selection"
    );
}

/// Moves an enemy building onto the lance's first unit (a declared input: the building's own place
/// is off screen) and lets the production visibility pass run on it for a few frames, so its
/// `windowsVisible`, `inView`, selection box and visible screen triangles are the real ones.
/// Returns its watch id, handle, selection box and the visible screen triangles.
fn real_structure(
    run: &mut HeadlessRun<LiveSim>,
    find: impl Fn(&mc2_sim::World) -> Option<mc2_sim::ObjectHandle>,
) -> (
    mc2_sim::WatchId,
    mc2_sim::ObjectHandle,
    [f32; 4],
    Vec<[f32; 6]>,
) {
    let at = hud(run).snapshot().units[0].position;
    let (watch, h) = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let h = find(w).expect("a matching structure");
        let o = w.objects.get_mut(h).unwrap();
        o.c.position.x = at.0;
        o.c.position.y = at.1;
        o.c.position.z = at.2;
        (w.objects.watch_id(h), h)
    };
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    let boxes = run.core.mission.as_ref().unwrap().driver.pick_boxes();
    let b = *boxes.structures.get(&h.0).expect("a projected box");
    let tris = boxes
        .structure_triangles
        .get(&h.0)
        .expect("triangles")
        .to_vec();
    assert!(!tris.is_empty(), "the building shows visible faces");
    (watch, h, b, tris)
}

fn real_enemy_building(
    run: &mut HeadlessRun<LiveSim>,
) -> (
    mc2_sim::WatchId,
    mc2_sim::ObjectHandle,
    [f32; 4],
    Vec<[f32; 6]>,
) {
    real_structure(run, |w| {
        w.objects.buildings.iter().copied().find(|&h| {
            w.objects.get(h).is_some_and(|o| {
                o.exists() && !o.c.is_disabled() && o.team_id() != w.teams.home as i32
            })
        })
    })
}

fn shipped_captureable_building(
    run: &mut HeadlessRun<LiveSim>,
) -> (mc2_sim::WatchId, mc2_sim::ObjectHandle) {
    let home = world(run).teams.home as i32;
    let w = world(run);
    let h = w
        .objects
        .buildings
        .iter()
        .copied()
        .find(|&h| mc2_sim::ai::capture::is_captureable(w, h, home))
        .expect("mc2_23 has a shipped capturable building");
    let (watch, position) = {
        let w = world(run);
        (
            w.objects.watch_id_peek(h),
            w.objects.get(h).unwrap().c.position,
        )
    };
    assert!(
        world(run)
            .objects
            .get(h)
            .unwrap()
            .c
            .flag(mc2_sim::object::flags::CAPTURABLE),
        "the shipped building keeps its capturable data flag"
    );
    // Keep the shipped structure transform and data untouched; frame it by moving only the camera.
    {
        let mission = run.core.mission.as_mut().unwrap();
        mission.rig.follow = None;
        mission.rig.set_position(
            Vec3::new(position.x, position.y, position.z),
            false,
            &mission.view.ground(),
            1.0 / 30.0,
        );
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    (watch, h)
}

fn select_one_lance_mech(run: &mut HeadlessRun<LiveSim>) -> i32 {
    let part = lance(world(run))
        .into_iter()
        .find(|&part| {
            world(run)
                .objects
                .find_by_part_id(PartId(part))
                .and_then(|h| world(run).objects.get(h))
                .is_some_and(|o| matches!(o.kind, Kind::Mech(_)))
        })
        .expect("a player lance mech");
    let (x, y) = screen_of(run, part);
    click(run, f64::from(x), f64::from(y));
    assert_eq!(
        selected(run),
        [part],
        "the production click selects one mech"
    );
    part
}

fn clear_mover_pick_click(run: &HeadlessRun<LiveSim>, watch: mc2_sim::WatchId) -> (f32, f32) {
    let snap = hud(run).snapshot();
    let pick = snap
        .picks
        .iter()
        .find(|pick| pick.watch_id == watch.0)
        .expect("the shipped vehicle is visible in the HUD picker");
    let triangles = &pick.triangles;
    let own = pick.watch_id;
    // A mover is picked from its projected box, but use a point on its visible geometry so the
    // integration case remains a real object click even if the sprite has a sparse silhouette.
    for triangle in triangles.iter() {
        let point = (
            (triangle[0] + triangle[2] + triangle[4]) / 3.0,
            (triangle[1] + triangle[3] + triangle[5]) / 3.0,
        );
        if !snap.picks.iter().any(|other| {
            other.watch_id != own
                && point.0 >= other.screen_box[0]
                && point.0 <= other.screen_box[2]
                && point.1 >= other.screen_box[1]
                && point.1 <= other.screen_box[3]
        }) {
            return point;
        }
    }
    panic!("the shipped vehicle has no clear point on its visible triangles")
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

/// A right click at game pixel `point`; `hide` marks `h` out of view just before the release (the
/// HUD reads the state at the start of that frame).  Returns the lance's player orders afterwards.
fn right_click_at(
    run: &mut HeadlessRun<LiveSim>,
    lance: &[i32],
    point: (f32, f32),
    hide: Option<mc2_sim::ObjectHandle>,
) -> Vec<mc2_sim::command::TacticalOrder> {
    right_click_with(run, lance, point, |w| {
        if let Some(h) = hide {
            w.appearance.set_in_view(h, false);
        }
    })
}

/// [`right_click_at`] with `before_release` run on the world just before the button release.
fn right_click_with(
    run: &mut HeadlessRun<LiveSim>,
    lance: &[i32],
    point: (f32, f32),
    before_release: impl FnOnce(&mut mc2_sim::World),
) -> Vec<mc2_sim::command::TacticalOrder> {
    mouse(run, f64::from(point.0), f64::from(point.1));
    run.step(None).unwrap();
    run.event(InputEvent::Button {
        button: MouseButton::Right,
        pressed: true,
    });
    run.step(None).unwrap();
    before_release(&mut run.core.mission.as_mut().unwrap().driver.sim_mut().world);
    run.event(InputEvent::Button {
        button: MouseButton::Right,
        pressed: false,
    });
    run.step(None).unwrap();
    lance.iter().filter_map(|&p| player_order(run, p)).collect()
}

/// A click on the object that comes from the camera and the object's own world position, not from the
/// picker's triangles: `Camera::projectZ` of the position raised by `dz` (the CPU projection, a different
/// path from the picker's GPU matrix), the first such point inside the object's box and under no mover's box.
fn independent_face_click(run: &HeadlessRun<LiveSim>, h: mc2_sim::ObjectHandle) -> (f32, f32) {
    independent_clicks(run, h)
        .first()
        .copied()
        .expect("a raised position under the box and clear of every other box")
}

/// Every such candidate click (the object's position raised in steps, then its box centre).
fn independent_clicks(run: &HeadlessRun<LiveSim>, h: mc2_sim::ObjectHandle) -> Vec<(f32, f32)> {
    let rt = run.core.mission.as_ref().unwrap();
    let w = &rt.driver.sim().world;
    let boxes = rt.driver.pick_boxes();
    let Some(&b) = boxes.structures.get(&h.0) else {
        return Vec::new();
    };
    let own = w.objects.watch_id_peek(h).0;
    let snap = mc2_app::hud_data::snapshot(w, boxes);
    let p = w.objects.get(h).unwrap().c.position;
    let inside =
        |b: &[f32; 4], c: (f32, f32)| c.0 >= b[0] && c.0 <= b[2] && c.1 >= b[1] && c.1 <= b[3];
    (0..60)
        .map(|i| {
            let (s, _) = rt
                .rig
                .view()
                .project_z(Vec3::new(p.x, p.y, p.z + i as f32 * 2.0));
            (s[0], s[1])
        })
        .chain([((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0)])
        .filter(|&c| {
            inside(&b, c)
                && !boxes.movers.values().any(|m| inside(m, c))
                && !snap
                    .picks
                    .iter()
                    .any(|q| q.watch_id != own && inside(&q.screen_box, c))
        })
        .collect()
}

fn centroid(t: &[f32; 6]) -> (f32, f32) {
    ((t[0] + t[2] + t[4]) / 3.0, (t[1] + t[3] + t[5]) / 3.0)
}

fn attacks(orders: &[mc2_sim::command::TacticalOrder], watch: mc2_sim::WatchId) -> bool {
    orders
        .iter()
        .any(|o| o.code == mc2_sim::command::OrderCode::AttackObject && o.target == watch)
}

/// A right click on an enemy building's visible face (`findObjectByMouse`, then `doAttack`) orders the
/// selected mechs to attack that object, not to move to the ground under it.
#[test]
fn a_player_attack_click_on_an_enemy_building_orders_the_lance_to_attack_it() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    let orders = right_click_at(&mut run, &lance, click, None);
    assert_eq!(orders.len(), lance.len(), "every selected mech was ordered");
    for order in orders {
        assert_eq!(order.code, mc2_sim::command::OrderCode::AttackObject);
        assert_eq!(order.target, watch, "the building is the target");
    }
}

/// The `MC2_P3_TRACE` command-click trace, read through `HudRuntime::update`: a right click on an enemy
/// building is recorded with its watch id (and the attack order is issued), a completed right click that
/// picks nothing is recorded as `picked watch none`, distinct from the attack.
#[test]
fn the_command_click_trace_records_a_hit_and_a_no_pick_click() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, b, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .unwrap()
        .capture_trace = true;
    let orders = right_click_at(&mut run, &lance, click, None);
    assert_eq!(
        orders.len(),
        lance.len(),
        "an attack was issued to every selected mech"
    );
    assert!(orders
        .iter()
        .all(|o| o.code == mc2_sim::command::OrderCode::AttackObject && o.target == watch));
    // a point in the world view well clear of the building's box and of every unit
    let miss = (60.0, 140.0);
    assert!(
        miss.0 < b[0] || miss.0 > b[2] || miss.1 < b[1] || miss.1 > b[3],
        "the miss point is outside the building's box"
    );
    right_click_at(&mut run, &lance, miss, None);
    let log = hud(&run).trace_log.clone();
    assert_eq!(
        log.len(),
        2,
        "one trace line per completed command click: {log:?}"
    );
    assert!(
        log[0].contains(&format!("picked watch {}", watch.0)) && log[0].contains("selected ["),
        "{}",
        log[0]
    );
    assert!(log[1].contains("picked watch none"), "{}", log[1]);
    eprintln!("trace hit : {}\ntrace miss: {}", log[0], log[1]);
}

/// The P3 player driver keeps giving physical orders when a lance member is disabled mid-replay:
/// a real HUD snapshot (units, picks from the live sim), one unit then marked disabled on the unit bar,
/// driven through `PlayerDriver::step` frame by frame.  It must not wait for the disabled unit
/// (it would keep re-selecting forever) and must emit an attack click once the able ones are selected.
#[test]
fn the_p3_driver_keeps_ordering_with_a_disabled_lance_member() {
    use mc2_app::p3_driver::PlayerDriver;
    use mc2_ui::objectives::{ObjectiveStatus, ObjectiveView};
    use mc2_ui::tacmap::TacTeam;

    let Some(mut run) = start(130) else { return };
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let mut snap = hud(&run).snapshot().clone();
    assert!(snap.units.len() >= 3, "the lance has three mechs");
    // The objectives panel: primaries 1 and 2 done, "destroy the patrol" (a stage of movers) open.
    snap.objectives = (0..6)
        .map(|i| ObjectiveView {
            status: if i < 2 {
                ObjectiveStatus::Successful
            } else {
                ObjectiveStatus::Undetermined
            },
            ..ObjectiveView::default()
        })
        .collect();
    // A hostile mover on screen near the lance (the pick the driver clicks).
    let mut pick = snap
        .picks
        .iter()
        .find(|p| p.watch_id == watch.0)
        .cloned()
        .expect("the moved enemy building is a pick");
    pick.mover = true;
    pick.team = TacTeam::Enemy;
    pick.disabled = false;
    pick.position = snap.units[0].position;
    pick.screen_box = [380.0, 150.0, 420.0, 190.0];
    pick.triangles = Default::default();
    snap.picks = vec![pick];
    // The third mech is disabled: greyed on the bar, skipped by Select Visible.
    snap.units[2].bar.destroyed = true;
    let able: Vec<i32> = snap.units[..2].iter().map(|u| u.part_id).collect();
    let disabled = snap.units[2].part_id;

    let mut driver = PlayerDriver::new();
    let turn0 = 2_700;
    // Only the two able units are selected (what Select Visible leaves): the driver must go on to
    // attack instead of selecting again.
    let mut emitted = Vec::new();
    for turn in turn0..turn0 + 90 {
        emitted.extend(driver.step(turn, || Some((snap.clone(), able.clone()))));
    }
    let buttons = |ev: &[InputEvent], b: MouseButton| {
        ev.iter()
            .filter(|e| matches!(e, InputEvent::Button { button, pressed: true } if *button == b))
            .count()
    };
    assert_eq!(
        buttons(&emitted, MouseButton::Right),
        1,
        "one attack click on the visible bandit: {emitted:?}"
    );
    assert_eq!(buttons(&emitted, MouseButton::Left), 0, "no re-selecting");
    assert!(
        emitted
            .iter()
            .any(|e| matches!(e, InputEvent::CursorMoved { x, y } if (380.0..=420.0).contains(x) && (150.0..=190.0).contains(y))),
        "the click lands in the bandit's box: {emitted:?}"
    );
    // Later a second unit goes down: a fresh look at the one remaining unit (selected) still orders.
    snap.units[1].bar.destroyed = true;
    let mut driver = PlayerDriver::new();
    let mut later = Vec::new();
    for turn in turn0..turn0 + 90 {
        later.extend(driver.step(turn, || Some((snap.clone(), vec![able[0]]))));
    }
    assert_eq!(
        buttons(&later, MouseButton::Left),
        0,
        "no re-selecting: {later:?}"
    );
    assert_eq!(
        buttons(&later, MouseButton::Right),
        1,
        "the remaining unit is ordered to attack: {later:?}"
    );
    // With nobody selected the driver does select (Select Visible, `E`), so the check above is not vacuous.
    let mut driver = PlayerDriver::new();
    let mut none = Vec::new();
    for turn in turn0..turn0 + 90 {
        none.extend(driver.step(turn, || Some((snap.clone(), Vec::new()))));
    }
    assert!(
        buttons(&none, MouseButton::Left) >= 1,
        "refocus click: {none:?}"
    );
    assert_eq!(
        buttons(&none, MouseButton::Right),
        0,
        "no order before selecting"
    );
    // The disabled unit is never waited for; were it expected, `E` would be tapped.
    assert!(!able.contains(&disabled));
    let _ = h;
}

/// A hit and a miss click for structure `h`, derived from the shape's own vertices (world space, via the
/// node matrices) projected with the CPU camera path (`project_z`), not from the picker's screen
/// triangles: the hit is the centroid of the largest projected face that no mover or other pick covers,
/// the miss a point inside the selection box that no projected face of the shape (of either facing) covers.
fn silhouette_clicks(
    run: &HeadlessRun<LiveSim>,
    h: mc2_sim::ObjectHandle,
) -> Option<((f32, f32), (f32, f32))> {
    let rt = run.core.mission.as_ref().unwrap();
    let boxes = rt.driver.pick_boxes();
    let b = *boxes.structures.get(&h.0)?;
    let own = rt.driver.sim().world.objects.watch_id_peek(h).0;
    let snap = mc2_app::hud_data::snapshot(&rt.driver.sim().world, boxes);
    let tris: Vec<[(f32, f32); 3]> = rt
        .driver
        .structure_world_triangles(&run.core.scene, h)
        .iter()
        .map(|t| {
            t.map(|p| {
                let (s, _) = rt.rig.view().project_z(p);
                (s[0], s[1])
            })
        })
        .collect();
    let inside =
        |b: &[f32; 4], c: (f32, f32)| c.0 >= b[0] && c.0 <= b[2] && c.1 >= b[1] && c.1 <= b[3];
    let covered = |p: (f32, f32)| {
        tris.iter()
            .any(|t| triangle_holds(&[t[0].0, t[0].1, t[1].0, t[1].1, t[2].0, t[2].1], p))
    };
    let clear = |c: (f32, f32)| {
        inside(&b, c)
            && !boxes.movers.values().any(|m| inside(m, c))
            && !snap
                .picks
                .iter()
                .any(|q| q.watch_id != own && inside(&q.screen_box, c))
    };
    let area = |t: &[(f32, f32); 3]| {
        ((t[1].0 - t[0].0) * (t[2].1 - t[0].1) - (t[2].0 - t[0].0) * (t[1].1 - t[0].1)).abs()
    };
    let mut faces: Vec<&[(f32, f32); 3]> = tris.iter().collect();
    faces.sort_by(|x, y| area(y).total_cmp(&area(x)));
    let hit = faces
        .iter()
        .map(|t| {
            (
                (t[0].0 + t[1].0 + t[2].0) / 3.0,
                (t[0].1 + t[1].1 + t[2].1) / 3.0,
            )
        })
        .find(|&c| clear(c) && covered(c))?;
    let miss = (0..40)
        .flat_map(|i| (0..40).map(move |j| (i, j)))
        .map(|(i, j)| {
            (
                b[0] + (b[2] - b[0]) * (i as f32 + 0.5) / 40.0,
                b[1] + (b[3] - b[1]) * (j as f32 + 0.5) / 40.0,
            )
        })
        .find(|&c| clear(c) && !covered(c))?;
    Some((hit, miss))
}

/// `findObjectByMouse` ends in `PerPolySelect` (objmgr.cpp:2186, tgl.cpp:2763): a click inside the
/// projected box but on no visible face of the shape picks nothing, so it is no attack order; a click on
/// a face does.  Both clicks come from the shape's own vertices transformed with the camera in the test
/// (`silhouette_clicks`), not from the picker's triangles.
#[test]
fn a_click_inside_a_structures_box_but_off_its_silhouette_does_not_pick_it() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let (hit, miss) = silhouette_clicks(&run, h).expect("a face and a gap clear of other picks");
    let orders = right_click_at(&mut run, &lance, miss, None);
    assert!(
        !attacks(&orders, watch),
        "a click off every face of the shape in its box picked the structure: {orders:?}"
    );
    let orders = right_click_at(&mut run, &lance, hit, None);
    assert!(
        attacks(&orders, watch),
        "a click on a face of the shape did not pick the structure: {orders:?}"
    );
}

/// `findObjectByMouse` needs the object to have been drawn last turn (`windowsVisible`): a structure the
/// camera did not show is not a pick, so a click on it never becomes an attack order.
#[test]
fn a_click_on_a_structure_that_was_not_drawn_is_not_an_attack_order() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let world = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    world.objects.get_mut(h).unwrap().c.windows_visible = -100; // never drawn
    let driver = &run.core.mission.as_mut().unwrap().driver;
    let snapshot = mc2_app::hud_data::snapshot(&driver.sim().world, driver.pick_boxes());
    assert!(
        !snapshot.picks.iter().any(|p| p.watch_id == watch.0),
        "a hidden structure is not pickable"
    );
}

/// `canBeSeen()` is read now (objmgr.cpp:2172): an object drawn on the previous turn that is out of
/// view this turn (`inView` false) cannot become an `AttackObject` target.
#[test]
fn a_structure_drawn_last_turn_but_out_of_view_now_is_not_an_attack_target() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    let orders = right_click_at(&mut run, &lance, click, Some(h));
    assert!(
        !attacks(&orders, watch),
        "an out-of-view structure was targeted: {orders:?}"
    );
}

/// `findObjectByMouse` searches the movers of other commanders first, then all movers (objmgr.cpp:2347): a command click
/// on one of the lance's mechs is an object click (`doAttack` declines a friendly), never the terrain
/// move under it.
#[test]
fn a_command_click_on_a_home_mech_is_an_object_click_not_a_terrain_move() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let boxes = run.core.mission.as_ref().unwrap().driver.pick_boxes();
    let b = *boxes.movers.get(&lance[0]).expect("the first mech's box");
    let tris = boxes
        .mover_triangles
        .get(&lance[0])
        .expect("the lance's first mech is drawn")
        .to_vec();
    // the centre of the largest visible face (the mech may step a pixel or two between this read and
    // the click, and both the box and a face must hold the click)
    let area = |t: &[f32; 6]| ((t[2] - t[0]) * (t[5] - t[1]) - (t[4] - t[0]) * (t[3] - t[1])).abs();
    let point = centroid(
        tris.iter()
            .max_by(|a, b| area(a).total_cmp(&area(b)))
            .expect("a visible face"),
    );
    assert!(point.0 >= b[0] && point.0 <= b[2] && point.1 >= b[1] && point.1 <= b[3]);
    assert!(
        hud(&run)
            .snapshot()
            .picks
            .iter()
            .any(|p| p.team == mc2_ui::tacmap::TacTeam::PlayerAlt
                && p.triangles.iter().any(|t| triangle_holds(t, point))),
        "the home mech is a pick"
    );
    let orders = right_click_at(&mut run, &lance, point, None);
    assert!(
        orders
            .iter()
            .all(|o| o.code != mc2_sim::command::OrderCode::MoveToPoint),
        "the click fell through to a terrain move: {orders:?}"
    );
}

/// Moves an enemy mover onto the lance's first mech (a declared input: the enemy's own place is off
/// screen) and lets the production visibility pass project it.  Returns its watch id, handle and the
/// projected box; the lance's first mech's box must overlap it.
fn real_enemy_mover(
    run: &mut HeadlessRun<LiveSim>,
    lance: &[i32],
) -> (mc2_sim::WatchId, mc2_sim::ObjectHandle, [f32; 4]) {
    use mc2_sim::object::Kind;
    let at = hud(run).snapshot().units[0].position;
    let (watch, h, part) = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let h = *w
            .objects
            .movers
            .iter()
            .find(|&&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.exists()
                        && !o.c.is_destroyed()
                        && match &o.kind {
                            Kind::Mech(m) => i32::from(m.m.commander.0) != 0,
                            Kind::Vehicle(v) => i32::from(v.m.commander.0) != 0,
                            _ => false,
                        }
                })
            })
            .expect("an enemy mover");
        let o = w.objects.get_mut(h).unwrap();
        o.c.position.x = at.0;
        o.c.position.y = at.1;
        o.c.position.z = at.2;
        let part = o.c.part_id.0;
        (w.objects.watch_id(h), h, part)
    };
    for _ in 0..4 {
        contact(run, h);
        run.step(None).unwrap();
    }
    let boxes = run.core.mission.as_ref().unwrap().driver.pick_boxes();
    let b = *boxes.movers.get(&part).expect("the enemy's projected box");
    let home = *boxes.movers.get(&lance[0]).expect("the home mech's box");
    assert!(
        b[0] < home[2] && home[0] < b[2] && b[1] < home[3] && home[1] < b[3],
        "the two movers' boxes overlap: {b:?} {home:?}"
    );
    (watch, h, b)
}

/// The home team has `h` in sensor contact (a declared input for the pick, which needs contact).
fn contact(run: &mut HeadlessRun<LiveSim>, h: mc2_sim::ObjectHandle) {
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    let home = w.teams.home;
    w.sensors.team_contact_status.entry(h).or_default()[home] = 1;
}

/// `findObjectByMouse` takes a mover on its projected box (objmgr.cpp:2253, "Movers are NOT per poly"):
/// a click at the box's corner, which no hull covers, still picks a contacted enemy mover, so it is
/// an attack order and not a ground move.
#[test]
fn a_click_in_an_enemy_movers_box_but_off_its_silhouette_attacks_it() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, b) = real_enemy_mover(&mut run, &lance);
    // a point just inside the box's top-left corner: a hull's silhouette never reaches it
    let corner = (b[0] + 5.0, b[1] + 5.0);
    let orders = right_click_with(&mut run, &lance, corner, |w| {
        let home = w.teams.home;
        w.sensors.team_contact_status.entry(h).or_default()[home] = 1;
    });
    assert!(
        attacks(&orders, watch),
        "no attack on the enemy: {orders:?} box {b:?} corner {corner:?}"
    );
}

/// `findObjectByMouse` searches the movers whose commander is not home first (objmgr.cpp:2278): where
/// a contacted enemy overlaps a home mech, a click on both is an attack on the enemy.
#[test]
fn a_click_where_an_enemy_and_a_home_mover_overlap_attacks_the_enemy() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, b) = real_enemy_mover(&mut run, &lance);
    let home = *run
        .core
        .mission
        .as_ref()
        .unwrap()
        .driver
        .pick_boxes()
        .movers
        .get(&lance[0])
        .unwrap();
    // the middle of the overlap of the two boxes: inside both
    let point = (
        (b[0].max(home[0]) + b[2].min(home[2])) / 2.0,
        (b[1].max(home[1]) + b[3].min(home[3])) / 2.0,
    );
    let orders = right_click_with(&mut run, &lance, point, |w| {
        let home = w.teams.home;
        w.sensors.team_contact_status.entry(h).or_default()[home] = 1;
    });
    assert!(
        attacks(&orders, watch),
        "the home mech won the pick: {orders:?}"
    );
}

#[test]
fn the_stop_button_orders_the_selected_mechs_to_stop() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let stop = hud(&run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.id == Some(mc2_ui::control::ControlId::Stop))
        .copied()
        .expect("the stop button");
    let (cx, cy) = hud(&run).hud_to_game((
        (stop.rect.left + stop.rect.right) / 2,
        (stop.rect.top + stop.rect.bottom) / 2,
    ));
    click(&mut run, f64::from(cx), f64::from(cy));
    assert!(hud(&run)
        .order_log
        .iter()
        .any(|i| matches!(i, HudIntent::Control(mc2_ui::control::ControlIntent::Stop))));
    for &p in &lance {
        assert_eq!(
            player_order_code(&run, p),
            Some(mc2_sim::command::OrderCode::Stop),
            "part {p}"
        );
    }
}

#[test]
fn force_group_hotkeys_make_select_and_toggle_through_game_core() {
    let Some(mut run) = start(130) else { return };
    let all = select_lance(&mut run);
    // Ctrl+2 -> `makeForceGroup` (`missiongui.cpp:1741-1752,3284-3307`).
    hold(&mut run, &["ctrl"], true);
    key(&mut run, "2");
    hold(&mut run, &["ctrl"], false);
    run.step(None).unwrap();
    assert_eq!(selected(&run).len(), all.len(), "make leaves the selection");
    assert_eq!(force_group(&run, &all), vec![Some(2); all.len()]);

    // Click elsewhere, then plain 2 restores exactly the C++ group selection
    // (`selectForceGroup`, missiongui.cpp:3249-3282).
    click(&mut run, 60.0, 300.0);
    assert!(
        selected(&run).is_empty(),
        "the empty-ground click deselects"
    );
    key(&mut run, "2");
    let mut restored = selected(&run);
    restored.sort_unstable();
    assert_eq!(restored, all, "plain 2 restores the saved group exactly");

    // Shift+2 toggles an already completely-selected group off.
    hold(&mut run, &["shift"], true);
    key(&mut run, "2");
    hold(&mut run, &["shift"], false);
    run.step(None).unwrap();
    assert!(selected(&run).is_empty(), "Shift+2 toggles the group");
}

#[test]
fn waypoint_release_before_a_digit_keeps_selection_and_executes_its_queue() {
    let Some(mut run) = start(130) else { return };
    let all = select_lance(&mut run);
    // Save the complete lance in group 2, then select one member through the production picker.
    hold(&mut run, &["ctrl"], true);
    key(&mut run, "2");
    hold(&mut run, &["ctrl"], false);
    run.step(None).unwrap();
    let member = all[0];
    let (x, y) = screen_of(&run, member);
    click(&mut run, f64::from(x), f64::from(y));
    assert_eq!(
        selected(&run),
        [member],
        "the picker selected one group member"
    );

    // Ctrl+right-click queues a normal move instead of executing it. The headless harness uses
    // the shipped AOE mouse profile, where left click selects and right click commands
    // (`MissionInterfaceManager::updateAOEStyle`, missiongui.cpp:1462-1665).
    // (`MissionInterfaceManager::handleOrders`, missiongui.cpp:2462-2555).
    hold(&mut run, &["ctrl"], true);
    right_click(&mut run, 60.0, 300.0);
    assert!(
        !queue_executing(&run, member),
        "the waypoint move remains queued until Ctrl is released"
    );

    // C++ handles this release first and sets bRetVal, so the same-frame 2 must not expand the
    // one-member selection to group 2 (`missiongui.cpp:1708-1754`). It also executes the queue.
    hold(&mut run, &["ctrl"], false);
    hold(&mut run, &["2"], true);
    run.step(None).unwrap();
    assert_eq!(
        selected(&run),
        [member],
        "waypoint release suppresses group selection"
    );
    assert!(
        queue_executing(&run, member),
        "waypoint release reaches the live simulation queue transition"
    );
    hold(&mut run, &["2"], false);
    run.step(None).unwrap();
}

#[test]
fn unit_bar_group_double_click_selects_the_live_sim_group() {
    let Some(mut run) = start(130) else { return };
    let all = select_lance(&mut run);
    hold(&mut run, &["ctrl"], true);
    key(&mut run, "3");
    hold(&mut run, &["ctrl"], false);
    run.step(None).unwrap();
    assert_eq!(force_group(&run, &all), vec![Some(3); all.len()]);
    // A real group-bar double click, rather than the prior selection, must restore the group.
    click(&mut run, 60.0, 300.0);
    assert!(
        selected(&run).is_empty(),
        "the world click deselected the lance"
    );
    // A world click just released the left button.  Let its double-click window expire before
    // starting the bar's first click, so the second bar press—not the first—takes the C++
    // `leftDoubleClick` path (`userinput.cpp:235-242`).
    for _ in 0..7 {
        run.step(None).unwrap();
    }
    let probes: Vec<(i32, i32)> = (0..1080 / 12)
        .flat_map(|r| (0..1920 / 12).map(move |c| (c * 12, r * 12)))
        .filter(|p| hud(&run).hit_test(*p) == HitTarget::UnitBar)
        .collect();
    assert!(!probes.is_empty(), "the unit bar is on screen");
    let mut found = false;
    for (x, y) in probes {
        mouse(&mut run, f64::from(x), f64::from(y));
        run.step(None).unwrap();
        for pressed in [true, false, true, false] {
            button(&mut run, pressed);
            run.step(None).unwrap();
        }
        let mut selected = selected(&run);
        selected.sort_unstable();
        if selected == all {
            found = true;
            break;
        }
    }
    assert!(found, "the double-click did not select its force group");
}

#[test]
fn a_destroyed_force_group_member_leaves_after_its_bar_animation() {
    let Some(mut run) = start(130) else { return };
    let all = select_lance(&mut run);
    hold(&mut run, &["ctrl"], true);
    key(&mut run, "4");
    hold(&mut run, &["ctrl"], false);
    run.step(None).unwrap();
    let dead = all[0];
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let h = w.objects.find_by_part_id(PartId(dead)).unwrap();
        w.objects.get_mut(h).unwrap().c.status = ObjectStatus::Destroyed;
        // HUD render drives the C++ death animation; pause keeps this fixture's object alive
        // while `ClearForceGroup` is accepted at the mission-interface command point.
        w.mission.paused = true;
    }
    // The C++ table's final death-animation stage ends at 6.5 seconds
    // (`mechicon.cpp:76-85`); the headless test runs at 30 Hz.
    for _ in 0..210 {
        run.step(None).unwrap();
        if force_group(&run, &[dead]) == [None] {
            break;
        }
    }
    assert_eq!(
        force_group(&run, &[dead]),
        [None],
        "ForceGroupIcon::drawDeathEffect clears unitGroup (mechicon.cpp:1493-1500)"
    );
}

#[test]
fn a_resize_keeps_messages_and_the_unavailable_report() {
    let Some(mut run) = start(130) else { return };
    let part = lance(world(&run))[0];
    let (x, y) = screen_of(&run, part);
    // The eject is refused while the home commander is invulnerable (`doEject`): an ORDERS chat
    // line and an unavailable report. The vehicle key (V) is no longer refused, so it is not used.
    command_click_with_key(&mut run, "END", f64::from(x), f64::from(y), |run| {
        run.core
            .mission
            .as_mut()
            .unwrap()
            .driver
            .sim_mut()
            .world
            .mission
            .invulnerable = true;
    });
    assert!(refused(&run, "eject"));
    assert!(hud(&run).chat_text().iter().any(|l| l.contains("ORDERS")));
    let res = hud(&run).resolution();
    run.resize(1920, 1200);
    run.step(None).unwrap();
    assert_ne!(hud(&run).resolution(), res, "the layout was rebuilt");
    assert!(hud(&run).chat_text().iter().any(|l| l.contains("ORDERS")));
    assert!(refused(&run, "eject"));
}

#[test]
fn a_player_vehicle_bar_follows_turret_damage_and_its_last_mine() {
    let Some(mut run) = start(10) else { return };
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    // a vehicle WITHOUT refit points, so the extra turret term depends on the mine count alone
    let h = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            w.objects.get(h).is_some_and(|o| {
                o.exists()
                    && matches!(o.kind, Kind::Vehicle(_))
                    && w.data
                        .vehicle_objects
                        .get(&(o.c.type_handle as u32))
                        .is_some_and(|v| v.refit_points == 0)
            })
        })
        .expect("mc2_01 has a non-refit ground vehicle to hand to the player");
    // fixture: the vehicle becomes a player unit on the GUI, undamaged, healthy pilot
    let slot = {
        let o = w.objects.get_mut(h).unwrap();
        let Kind::Vehicle(v) = &mut o.kind else {
            unreachable!()
        };
        v.m.commander = mc2_sim::ids::CommanderId(HOME_COMMANDER as _);
        v.m.flags.on_gui = true;
        v.m.pilot_slot
    };
    if let Some(p) = w.pilots.get_mut(slot) {
        p.wounds = 0.0;
    }
    let part = w.objects.get(h).unwrap().c.part_id.0;
    // independent C++ expectation (gvehicl.cpp:5406-5460) from the armour (cur, max) per location
    // front,left,right,rear,turret, weapon effectiveness 1, pilot factor 1
    let expect = |arm: [(f32, f32); 5], extra_turret: bool| -> f32 {
        let side = |(c, m): (f32, f32)| c / m * 0.6 + 0.4;
        let turret = if arm[4].1 != 0.0 { side(arm[4]) } else { 1.0 };
        let extra = if extra_turret && arm[4].1 != 0.0 {
            arm[4].0 / arm[4].1
        } else {
            1.0
        };
        side(arm[0]) * side(arm[1]) * side(arm[2]) * side(arm[3]) * turret * extra
    };
    // the bar as the HUD shows it after a real GameCore frame
    let bar = |run: &HeadlessRun<LiveSim>| {
        hud(run)
            .snapshot()
            .units
            .iter()
            .find(|u| u.part_id == part)
            .expect("the vehicle is on the unit bar")
            .bar
            .bar_status
    };
    {
        let l = mc2_sim::combat::loadout_of_mut(w.objects.get_mut(h).unwrap()).unwrap();
        l.stats.weapon_effectiveness = l.stats.max_weapon_effectiveness;
        for a in l.armor.iter_mut().take(5) {
            a.1 = 40;
            a.0 = 40.0;
        }
        // damaged: front 20/40, turret 10/40
        l.armor[0].0 = 20.0;
        l.armor[4].0 = 10.0;
        l.stats.mine_layer = 3;
    }
    let arm = [
        (20.0, 40.0),
        (40.0, 40.0),
        (40.0, 40.0),
        (40.0, 40.0),
        (10.0, 40.0),
    ];
    let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
    let (with_mine, spent) = (expect(arm, true), expect(arm, false));
    assert!(
        (with_mine - spent).abs() > 1e-3,
        "the two states differ: {with_mine} vs {spent}"
    );
    run.step(None).unwrap();
    let got = bar(&run);
    assert!(close(got, with_mine), "mines left: {got} vs {with_mine}");
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    mc2_sim::combat::loadout_of_mut(w.objects.get_mut(h).unwrap())
        .unwrap()
        .stats
        .mine_layer = 0;
    run.step(None).unwrap();
    let got = bar(&run);
    assert!(close(got, spent), "last mine laid: {got} vs {spent}");
}

#[test]
fn a_tactical_map_click_moves_the_camera_there() {
    let Some(mut run) = start(130) else { return };
    // the mission script pans the camera at the start; wait for its goal to end so the click is
    // the only thing moving the camera
    while run.core.mission.as_ref().unwrap().rig.goal_pos_time > 0.0 && sim_frame(&run) < 1800 {
        run.step(None).unwrap();
    }
    assert!(
        run.core.mission.as_ref().unwrap().rig.goal_pos_time <= 0.0,
        "the opening camera script ended"
    );
    let map = hud(&run).tac_map().rect();
    let before = run.core.mission.as_ref().unwrap().rig.position;
    // a point in the map's north-west part, well away from the camera's start
    let (px, py) = (
        map.left + (map.right - map.left) / 5,
        map.top + (map.bottom - map.top) / 5,
    );
    let (gx, gy) = hud(&run).hud_to_game((px, py));
    assert_eq!(
        hud(&run).hit_test((gx as i32, gy as i32)),
        HitTarget::TacMap
    );
    let (wx, wy) = hud(&run)
        .tac_map()
        .tac_map_to_world(px - map.left, py - map.top)
        .expect("the click is on the map");
    click(&mut run, f64::from(gx), f64::from(gy));
    for _ in 0..3 {
        run.step(None).unwrap();
    }
    let after = run.core.mission.as_ref().unwrap().rig.position;
    assert!(
        (before.x - after.x).hypot(before.y - after.y) > 500.0,
        "the camera moved: {before:?} -> {after:?}"
    );
    assert!(
        (after.x - wx).hypot(after.y - wy) < 2.0,
        "to the clicked map point ({wx}, {wy}): {after:?}"
    );
}

/// Median-cut palette of at most `colors` entries and the index of every pixel (the same helper
/// as `sim_replay`'s screenshots).
fn quantize(rgba: &[u8], colors: usize) -> (Vec<[u8; 3]>, Vec<u8>) {
    let mut hist: std::collections::HashMap<u16, u32> = Default::default();
    for p in rgba.chunks_exact(4) {
        let k = ((p[0] >> 3) as u16) << 10 | ((p[1] >> 3) as u16) << 5 | (p[2] >> 3) as u16;
        *hist.entry(k).or_default() += 1;
    }
    let mut boxes: Vec<Vec<(u16, u32)>> = vec![hist.into_iter().collect()];
    let chan = |k: u16, c: usize| ((k >> (10 - 5 * c as u16)) & 31) as i32;
    while boxes.len() < colors {
        let (bi, _) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let range = (0..3)
                    .map(|c| {
                        let lo = b.iter().map(|e| chan(e.0, c)).min().unwrap();
                        let hi = b.iter().map(|e| chan(e.0, c)).max().unwrap();
                        hi - lo
                    })
                    .max()
                    .unwrap() as u64;
                (i, range * b.iter().map(|e| e.1 as u64).sum::<u64>())
            })
            .max_by_key(|x| x.1)
            .unwrap_or((usize::MAX, 0));
        if bi == usize::MAX {
            break;
        }
        let mut b = boxes.swap_remove(bi);
        let c = (0..3)
            .max_by_key(|&c| {
                b.iter().map(|e| chan(e.0, c)).max().unwrap()
                    - b.iter().map(|e| chan(e.0, c)).min().unwrap()
            })
            .unwrap();
        b.sort_by_key(|e| chan(e.0, c));
        let total: u64 = b.iter().map(|e| e.1 as u64).sum();
        let mut acc = 0;
        let mut cut = 1;
        for (i, e) in b.iter().enumerate() {
            acc += e.1 as u64;
            if acc * 2 >= total {
                cut = (i + 1).clamp(1, b.len() - 1);
                break;
            }
        }
        let hi = b.split_off(cut);
        boxes.push(b);
        boxes.push(hi);
    }
    let palette: Vec<[u8; 3]> = boxes
        .iter()
        .map(|b| {
            let n: u64 = b.iter().map(|e| e.1 as u64).sum::<u64>().max(1);
            let m = |c: usize| {
                let sum: u64 = b.iter().map(|e| chan(e.0, c) as u64 * e.1 as u64).sum();
                (((sum / n) as u8) << 3) | 4
            };
            [m(0), m(1), m(2)]
        })
        .collect();
    let mut lookup: std::collections::HashMap<u16, u8> = Default::default();
    for (i, b) in boxes.iter().enumerate() {
        for e in b {
            lookup.insert(e.0, i as u8);
        }
    }
    let idx = rgba
        .chunks_exact(4)
        .map(|p| {
            lookup[&(((p[0] >> 3) as u16) << 10 | ((p[1] >> 3) as u16) << 5 | (p[2] >> 3) as u16)]
        })
        .collect();
    (palette, idx)
}

fn write_indexed(path: &std::path::Path, w: u32, h: u32, palette: &[[u8; 3]], idx: &[u8]) -> u64 {
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(path).unwrap()),
        w,
        h,
    );
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_palette(palette.iter().flatten().copied().collect::<Vec<u8>>());
    enc.set_compression(png::Compression::High);
    enc.set_filter(png::Filter::Adaptive);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(idx).unwrap();
    wr.finish().unwrap();
    std::fs::metadata(path).unwrap().len()
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

const HUD_PNG: &str = "mc2_01_live_hud_1920x1080.png";

/// Writes `mc2_01_live_hud_1920x1080.png` under the cargo target tmp dir: `mc2_01` at 1920x1080 with the live
/// HUD, the lance drag-selected with the mouse, palette-quantised until it is under 400 KB.
/// `MC2_DATA=... cargo test -p mc2-app --test hud_live -- --ignored write_hud_screenshot`
#[test]
#[ignore = "writes target/tmp/mc2_01_live_hud_1920x1080.png"]
fn write_hud_screenshot() {
    let Some(mut run) = start(150) else { return };
    let ids = lance(world(&run));
    let pts: Vec<(f32, f32)> = ids.iter().map(|&p| screen_of(&run, p)).collect();
    let (x0, y0) = (
        pts.iter().map(|p| p.0).fold(f32::MAX, f32::min) - 60.0,
        pts.iter().map(|p| p.1).fold(f32::MAX, f32::min) - 60.0,
    );
    let (x1, y1) = (
        pts.iter().map(|p| p.0).fold(f32::MIN, f32::max) + 60.0,
        pts.iter().map(|p| p.1).fold(f32::MIN, f32::max) + 60.0,
    );
    mouse(&mut run, f64::from(x0), f64::from(y0));
    run.step(None).unwrap();
    button(&mut run, true);
    run.step(None).unwrap();
    mouse(&mut run, f64::from(x1), f64::from(y1));
    run.step(None).unwrap();
    button(&mut run, false);
    for _ in 0..40 {
        run.step(None).unwrap();
    }
    assert_eq!(hud(&run).selection.selected.len(), ids.len());
    let full = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hud_full.png");
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    run.step(Some(&full)).unwrap();
    let (w, h, rgba) = read_png(&full);
    assert_eq!((w, h), (1920, 1080));
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(HUD_PNG);
    for colors in [256usize, 128, 96, 64, 32] {
        let (palette, idx) = quantize(&rgba, colors);
        let size = write_indexed(&out, w, h, &palette, &idx);
        if size < 400_000 {
            println!("wrote {} ({colors} colours, {size} bytes)", out.display());
            return;
        }
    }
    panic!("no palette size fits 400 KB");
}

/// Empties the selection like the player's Escape-free route: a classic left click on open ground deselects.
fn hud_deselect(run: &mut HeadlessRun<LiveSim>) {
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .unwrap()
        .selection
        .selected
        .clear();
    run.step(None).unwrap();
}

/// A completed right click on open ground (the AOE profile's command click), keys as already held.
fn right_click_ground(run: &mut HeadlessRun<LiveSim>) {
    mouse(run, 960.0, 400.0);
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
}

#[test]
fn releasing_space_clears_the_run_mode_before_the_next_move() {
    use mc2_sim::command::TravelMode;
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    // held Space: the move is a run
    hold(&mut run, &["space"], true);
    run.step(None).unwrap();
    right_click_ground(&mut run);
    let order = player_order(&run, lance[0]).expect("a move order");
    assert_eq!(order.move_params.way_path.mode[0], TravelMode::Fast);
    hold(&mut run, &["space"], false);
    run.step(None).unwrap();
    // released: the next move is slow again
    right_click_ground(&mut run);
    for &p in &lance {
        let order = player_order(&run, p).expect("a move order");
        assert_eq!(order.code, mc2_sim::command::OrderCode::MoveToPoint);
        assert_eq!(
            order.move_params.way_path.mode[0],
            TravelMode::Slow,
            "part {p}: the Space release cleared run mode"
        );
    }
}

#[test]
fn a_forced_shot_with_a_held_energy_key_conserves_ammo() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    // F alone: attack to destroy
    hold(&mut run, &["f"], true);
    run.step(None).unwrap();
    right_click_ground(&mut run);
    hold(&mut run, &["f"], false);
    run.step(None).unwrap();
    let plain = player_order(&run, lance[0]).expect("a forced shot");
    assert_eq!(plain.code, mc2_sim::command::OrderCode::AttackPoint);
    assert_eq!(plain.attack_params.attack_type, 1, "ATTACK_TO_DESTROY");
    // A held with F: attack conserving ammo
    hold(&mut run, &["a", "f"], true);
    run.step(None).unwrap();
    right_click_ground(&mut run);
    let held = player_order(&run, lance[0]).expect("a forced shot");
    assert_eq!(held.code, mc2_sim::command::OrderCode::AttackPoint);
    assert_eq!(held.attack_params.attack_type, 3, "ATTACK_CONSERVING_AMMO");
}

/// A data root that is `data` with `mc2_01.fit` replaced by a loose copy whose strike flags are
/// the given ones (no shipped mission disables a strike).  Built under the temp dir.
fn data_with_strikes(data: &std::path::Path, air: bool, sensor: bool, tag: &str) -> PathBuf {
    let b = |v: bool| if v { "TRUE" } else { "FALSE" };
    data_with_settings(
        data,
        &format!(
            "b AirStrikesEnabledDefault = {}\r\nb SensorProbesEnabledDefault = {}\r\n",
            b(air),
            b(sensor)
        ),
        tag,
    )
}

/// A data root whose `mc2_01.fit` has `lines` (CRLF `[MissionSettings]` keys) prepended to its settings block.
fn data_with_settings(data: &std::path::Path, lines: &str, tag: &str) -> PathBuf {
    let mut game = mc2_render::game::GameData::open(data).unwrap();
    let fit = game.vfs.read("data/missions/mc2_01.fit").unwrap().unwrap();
    let text = String::from_utf8_lossy(&fit).into_owned();
    let flags = format!("[MissionSettings]\r\n{lines}");
    let edited = text.replacen("[MissionSettings]\r\n", &flags, 1);
    assert_ne!(
        edited, text,
        "the mission file has a [MissionSettings] block"
    );
    let root = std::env::temp_dir().join(format!("mc2-hud-live-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let missions = root.join("data").join("missions");
    std::fs::create_dir_all(&missions).unwrap();
    link_tree(data, &root);
    // The link tree may hold a hard link to a loose source file: unlink it so the write makes a
    // fresh file instead of editing the shared game data.
    let target = missions.join("mc2_01.fit");
    let source = data.join("data").join("missions").join("mc2_01.fit");
    let before = std::fs::read(&source).ok();
    write_unlinked(&target, edited.as_bytes());
    assert_eq!(
        std::fs::read(&source).ok(),
        before,
        "the shared game data is untouched"
    );
    root
}

/// Writes `bytes` to `target` as a fresh file: a hard link there (to a loose source file) is
/// removed first so the shared file is never edited through it.
fn write_unlinked(target: &std::path::Path, bytes: &[u8]) {
    let _ = std::fs::remove_file(target);
    std::fs::write(target, bytes).unwrap();
}

/// The loose-source case the shipped data root lacks: a fixture hard-linked to a source file on
/// the same volume must leave the source's bytes unchanged when it is edited.
#[test]
fn editing_a_hard_linked_fixture_leaves_the_loose_source_untouched() {
    let dir = std::env::temp_dir().join(format!("mc2-hud-live-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("source.fit");
    let target = dir.join("target.fit");
    std::fs::write(&source, b"original").unwrap();
    std::fs::hard_link(&source, &target).expect("same volume hard link");
    // The shared-inode check is Unix-only; elsewhere isolation is asserted by content below.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(&source).unwrap().ino(),
            std::fs::metadata(&target).unwrap().ino(),
            "the hard-link case is reached"
        );
    }
    let hash = |p: &std::path::Path| {
        std::fs::read(p)
            .unwrap()
            .iter()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3)
            })
    };
    let before = hash(&source);
    write_unlinked(&target, b"edited fixture");
    assert_eq!(hash(&source), before, "the source hash is unchanged");
    assert_eq!(std::fs::read(&source).unwrap(), b"original");
    assert_eq!(std::fs::read(&target).unwrap(), b"edited fixture");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mirrors `from` under `to`: directories are made, files are hard-linked (a loader refuses
/// symlinks that leave the data root) or copied when the volumes differ.
fn link_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let dest = to.join(e.file_name());
        let path = std::fs::canonicalize(e.path()).unwrap();
        if path.is_dir() {
            link_tree(&path, &dest);
        } else if std::fs::hard_link(&path, &dest).is_err() {
            std::fs::copy(&path, &dest).unwrap();
        }
    }
}

/// Opens the vehicle tab and returns the strike button's rect and state.
fn strike_button(
    run: &mut HeadlessRun<LiveSim>,
    kind: mc2_ui::control::VehicleKind,
) -> (
    mc2_ui::control::ControlRect,
    mc2_ui::control::ButtonVisualState,
) {
    let tab = hud(run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.id == Some(mc2_ui::control::ControlId::VehicleTab))
        .map(|b| b.rect)
        .expect("the vehicle tab button");
    let (x, y) = centre(run, tab);
    click(run, f64::from(x), f64::from(y));
    hud(run)
        .control_panel()
        .vehicles
        .iter()
        .find(|b| b.kind == Some(kind))
        .map(|b| (b.rect, b.state))
        .expect("the strike button")
}

fn centre(run: &HeadlessRun<LiveSim>, r: mc2_ui::control::ControlRect) -> (f32, f32) {
    hud(run).hud_to_game(((r.left + r.right) / 2, (r.top + r.bottom) / 2))
}

/// Gives the sim and the panel enough resource points for any strike.
fn fund(run: &mut HeadlessRun<LiveSim>, points: i32) {
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .sim_mut()
        .world
        .mission
        .resource_points = points;
    run.step(None).unwrap();
    run.step(None).unwrap();
}

fn sim_points(run: &HeadlessRun<LiveSim>) -> i32 {
    world(run).mission.resource_points
}

/// The live strikes in the sim's artillery pool, as their object type handles.
fn live_strikes(run: &HeadlessRun<LiveSim>) -> Vec<i32> {
    let w = world(run);
    w.objects
        .artillery
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .filter(|o| o.exists())
        .map(|o| o.c.type_handle)
        .collect()
}

/// The shipped object type of a strike, for either line-of-sight outcome (`IfaceCallStrike` remaps a LARGE strike to SMALL when
/// the target is out of sight; the sim decides which).
fn strike_types(strike: i32) -> Vec<i32> {
    use mc2_sim::combat::artillery::STRIKE_OBJECT_IDS;
    use mc2_sim::combat::artillery_math::normalize_strike;
    [false, true]
        .iter()
        .filter_map(|&off| normalize_strike(strike, off))
        .map(|n| STRIKE_OBJECT_IDS[n as usize])
        .collect()
}

/// Clicks a vehicle-tab button (opening the tab first); returns whether the panel then holds it pressed.
fn press_vehicle_button(run: &mut HeadlessRun<LiveSim>, kind: mc2_ui::control::VehicleKind) {
    let (rect, _) = strike_button(run, kind);
    let (x, y) = centre(run, rect);
    click(run, f64::from(x), f64::from(y));
}

fn click_vehicle_stop(run: &mut HeadlessRun<LiveSim>) {
    let rect = hud(run)
        .control_panel()
        .vehicles
        .iter()
        .find(|b| b.kind.is_none())
        .map(|b| b.rect)
        .expect("the vehicle stop button");
    let (x, y) = centre(run, rect);
    click(run, f64::from(x), f64::from(y));
}

#[test]
fn a_mission_that_disables_a_strike_keeps_its_button_disabled() {
    use mc2_ui::control::{ButtonVisualState as S, VehicleKind as K};
    let Some(data) = data() else { return };
    let root = data_with_strikes(&data, false, true, "air-off");
    let Some(mut run) = start_in(root.clone(), 130) else {
        return;
    };
    select_lance(&mut run);
    fund(&mut run, 100_000);
    let (_, air) = strike_button(&mut run, K::Airstrike);
    let (_, sensor) = strike_button(&mut run, K::SensorProbe);
    assert_eq!(air, S::Disabled, "the mission disables the airstrike");
    assert_eq!(sensor, S::Enabled, "the mission allows the sensor probe");
    // pressing the disabled button, by click or by hot key, arms nothing and spends nothing
    press_vehicle_button(&mut run, K::Airstrike);
    key(&mut run, "multiply");
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert_eq!(sim_points(&run), 100_000);
    // the allowed one arms
    key(&mut run, "divide");
    assert_eq!(
        hud(&run).control_panel().selected_vehicle,
        Some(K::SensorProbe)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_strike_the_resource_points_cannot_pay_stays_disabled() {
    use mc2_ui::control::{ButtonVisualState as S, VehicleKind as K};
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    fund(&mut run, 1_000);
    let (_, air) = strike_button(&mut run, K::Airstrike);
    assert_eq!(air, S::Disabled, "7000 points are needed");
    press_vehicle_button(&mut run, K::Airstrike);
    key(&mut run, "multiply");
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert_eq!(sim_points(&run), 1_000, "nothing was spent");
    right_click_ground(&mut run);
    assert!(live_strikes(&run).is_empty());
}

/// The acceptance path: the panel button arms the strike (the cost is spent by the press, as `handleVehicleClick` does), the
/// next ground click issues exactly one `CallStrike`, the sim schedules it, and the button is released without a refund.
#[test]
fn a_strike_button_and_a_ground_click_call_one_strike_and_charge_once() {
    use mc2_sim::combat::artillery_math::ARTILLERY_LARGE;
    use mc2_ui::control::VehicleKind as K;
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    fund(&mut run, 100_000);
    press_vehicle_button(&mut run, K::Airstrike);
    assert_eq!(
        hud(&run).control_panel().selected_vehicle,
        Some(K::Airstrike)
    );
    assert_eq!(
        sim_points(&run),
        100_000 - 7_000,
        "the press spends the cost"
    );
    assert!(
        live_strikes(&run).is_empty(),
        "nothing is called by the press"
    );
    right_click_ground(&mut run);
    let strikes = live_strikes(&run);
    assert_eq!(strikes.len(), 1, "exactly one strike is scheduled");
    assert!(strike_types(ARTILLERY_LARGE).contains(&strikes[0]));
    assert_eq!(sim_points(&run), 100_000 - 7_000, "charged once, no refund");
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert!(!refused(&run, "support strike"));
    for &p in &lance {
        assert_eq!(player_order_code(&run, p), None, "the click was not a move");
    }
    // a second ground click is an ordinary move again
    right_click_ground(&mut run);
    assert_eq!(live_strikes(&run).len(), 1);
    assert_eq!(
        player_order_code(&run, lance[0]),
        Some(mc2_sim::command::OrderCode::MoveToPoint)
    );
}

#[test]
fn cancelling_an_armed_strike_refunds_it_and_calls_nothing() {
    use mc2_ui::control::VehicleKind as K;
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    fund(&mut run, 100_000);
    press_vehicle_button(&mut run, K::SensorProbe);
    assert_eq!(sim_points(&run), 100_000 - 2_000);
    click_vehicle_stop(&mut run);
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert_eq!(sim_points(&run), 100_000, "the cancel refunds the cost");
    right_click_ground(&mut run);
    assert!(live_strikes(&run).is_empty(), "no strike was called");
    // pressing the armed button again does nothing: arm, then click it again
    press_vehicle_button(&mut run, K::SensorProbe);
    press_vehicle_button(&mut run, K::SensorProbe);
    assert_eq!(sim_points(&run), 100_000 - 2_000, "the cost is spent once");
    // the main Stop button also cancels
    let stop = hud(&run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.id == Some(mc2_ui::control::ControlId::Stop))
        .map(|b| b.rect)
        .expect("the stop button");
    let (x, y) = centre(&run, stop);
    click(&mut run, f64::from(x), f64::from(y));
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert_eq!(sim_points(&run), 100_000);
}

/// The hot keys press the same buttons (`sendAirstrike` / `sendSensorStrike`), so the same click calls the same strike and
/// spends the same cost as the panel path.
#[test]
fn the_strike_hot_keys_issue_the_same_strikes_as_the_panel_buttons() {
    use mc2_ui::control::VehicleKind as K;
    for (kind, name) in [(K::Airstrike, "multiply"), (K::SensorProbe, "divide")] {
        let Some(mut panel) = start(130) else { return };
        select_lance(&mut panel);
        fund(&mut panel, 100_000);
        press_vehicle_button(&mut panel, kind);
        right_click_ground(&mut panel);

        let Some(mut hot) = start(130) else { return };
        select_lance(&mut hot);
        fund(&mut hot, 100_000);
        key(&mut hot, name);
        assert_eq!(hud(&hot).control_panel().selected_vehicle, Some(kind));
        right_click_ground(&mut hot);

        assert_eq!(live_strikes(&hot).len(), 1, "{kind:?}");
        assert_eq!(live_strikes(&hot), live_strikes(&panel), "{kind:?}");
        assert_eq!(sim_points(&hot), sim_points(&panel), "{kind:?}");
        assert_eq!(hud(&hot).control_panel().selected_vehicle, None);
    }
}

/// The sensor probe also places on the TacMap (`GameTacMap::update` -> `doMove` -> `addAirstrike`); the airstrike does not (its
/// map click still pans the camera).
#[test]
fn a_tacmap_click_places_the_sensor_probe_like_the_panel_path() {
    use mc2_ui::control::VehicleKind as K;
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    fund(&mut run, 100_000);
    press_vehicle_button(&mut run, K::SensorProbe);
    // the map is clickable once its tab is the pressed one (`ControlGui::update`, controlgui.cpp:1200)
    let tab = hud(&run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.cxx_id == 12)
        .map(|b| b.rect)
        .expect("the map tab button");
    let (x, y) = centre(&run, tab);
    click(&mut run, f64::from(x), f64::from(y));
    assert_eq!(
        hud(&run).control_panel().selected_vehicle,
        Some(K::SensorProbe),
        "switching tabs keeps the armed strike"
    );
    // past the double-click window of the tab click
    for _ in 0..20 {
        run.step(None).unwrap();
    }
    let map = hud(&run).tac_map().rect();
    let (px, py) = (
        map.left + (map.right - map.left) / 5,
        map.top + (map.bottom - map.top) / 5,
    );
    let (gx, gy) = hud(&run).hud_to_game((px, py));
    click(&mut run, f64::from(gx), f64::from(gy));
    for _ in 0..2 {
        run.step(None).unwrap();
    }
    let strikes = live_strikes(&run);
    assert_eq!(strikes.len(), 1, "one probe is scheduled");
    assert_eq!(
        strikes,
        strike_types(mc2_sim::combat::artillery_math::ARTILLERY_SENSOR)
            .into_iter()
            .take(1)
            .collect::<Vec<_>>()
    );
    assert_eq!(sim_points(&run), 100_000 - 2_000);
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
}

/// The rollover help of a pressed strike (`updateVehicleTab`, controlgui.cpp:1763-1772): the no-line-of-sight text, or the
/// airstrike's own text while the pointer is in sight.
#[test]
fn a_pressed_strike_shows_its_rollover_help() {
    use mc2_ui::control::VehicleKind as K;
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    fund(&mut run, 100_000);
    press_vehicle_button(&mut run, K::Airstrike);
    mouse(&mut run, 960.0, 400.0);
    run.step(None).unwrap();
    let los = hud(&run).cursor_line_of_sight();
    // IDS_AIRSTRIKE_HELP / _NO_LOS; the left/right profile (the default) shows the "Click"/"Right-click" twin, id + 1
    // (`setRolloverHelpText`, controlgui.cpp:2919; strings.res.h 45104..45107)
    assert!(run.core.prefs.left_right_mouse_profile);
    assert_eq!(
        hud(&run).control_panel().help_text,
        if los { 45105 } else { 45107 },
        "left/right profile, line of sight {los}"
    );
    run.core.prefs.left_right_mouse_profile = false;
    mouse(&mut run, 961.0, 401.0);
    run.step(None).unwrap();
    run.step(None).unwrap();
    assert_eq!(
        hud(&run).control_panel().help_text,
        if los { 45104 } else { 45106 },
        "classic profile, line of sight {los}"
    );
}

/// Classic profile: `addAirstrike` runs before target selection (missiongui.cpp:1340), so a left click calls the strike with
/// nothing selected and on top of one of our own units.
#[test]
fn the_classic_profile_places_a_strike_with_no_selection_and_on_a_unit() {
    use mc2_sim::combat::artillery_math::ARTILLERY_LARGE;
    use mc2_ui::control::VehicleKind as K;
    for on_unit in [false, true] {
        let Some(mut run) = start(130) else { return };
        run.core.prefs.left_right_mouse_profile = false;
        let lance = select_lance(&mut run);
        fund(&mut run, 100_000);
        let target = if on_unit {
            screen_of(&run, lance[0])
        } else {
            (960.0, 400.0)
        };
        if !on_unit {
            hud_deselect(&mut run);
            assert!(selected(&run).is_empty());
        }
        press_vehicle_button(&mut run, K::Airstrike);
        assert_eq!(sim_points(&run), 100_000 - 7_000);
        click(&mut run, f64::from(target.0), f64::from(target.1));
        let strikes = live_strikes(&run);
        assert_eq!(strikes.len(), 1, "on_unit {on_unit}: one strike");
        assert!(strike_types(ARTILLERY_LARGE).contains(&strikes[0]));
        assert_eq!(sim_points(&run), 100_000 - 7_000);
        assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    }
}

/// `findObjectByMouse` also searches gates, terrain objects and special buildings (objmgr.cpp:2371,
/// every non-mover in `objList`, trees excluded): a visible gate is an attack target on a face and not
/// on the fixed off-face corner of its box.
#[test]
fn a_click_on_a_gate_is_an_attack_order_and_an_off_face_click_is_not() {
    // mc2_01 has no gate; mc2_02 has three.
    let Some(mut run) = data().and_then(|d| start_mission(d, "mc2_02", 130)) else {
        return;
    };
    let lance = select_lance(&mut run);
    let (watch, h, b, _) = real_structure(&mut run, |w| {
        w.objects.gates.iter().copied().find(|&h| {
            w.objects.get(h).is_some_and(|o| {
                o.exists() && !o.c.is_disabled() && o.team_id() != w.teams.home as i32
            })
        })
    });
    let _ = b;
    let (on, off) = silhouette_clicks(&run, h).expect("a face and a gap clear of other picks");
    let orders = right_click_at(&mut run, &lance, off, None);
    assert!(
        !attacks(&orders, watch),
        "off-face click picked the gate: {orders:?}"
    );
    let orders = right_click_at(&mut run, &lance, on, None);
    assert!(attacks(&orders, watch), "no attack on the gate: {orders:?}");
}

/// The same for a (non-tree) terrain object; the off-face side is covered by the building and gate tests.
#[test]
fn a_click_on_a_terrain_object_is_an_attack_order() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    // The dismissed intro has left the stock camera looking away from the map's terrain objects.
    // Pan the production camera over one object; unlike relocating the object, this preserves its
    // map block, vertex and collision/picking state.
    let focus = {
        let w = world(&run);
        let h = w
            .objects
            .terrain_objects
            .iter()
            .copied()
            .find(|&h| {
                w.objects
                    .get(h)
                    .is_some_and(|o| o.exists() && !o.c.is_disabled())
            })
            .expect("a shipped terrain object");
        w.objects.get(h).unwrap().c.position
    };
    {
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_camera_rotation(0.0, 0.0);
        rt.rig.projection_angle = 20.0;
        rt.rig.zoom_value(560.0);
        rt.rig.set_position(
            mc2_render::math::Vec3::new(focus.x, focus.y, focus.z),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    // A terrain object the camera naturally shows (its block and vertex are active; moving one
    // would leave its vertex stale), with a face no mover's box covers.
    let mut found = {
        let d = &run.core.mission.as_ref().unwrap().driver;
        let w = &d.sim().world;
        let boxes = d.pick_boxes();
        let snap = mc2_app::hud_data::snapshot(w, boxes);
        w.objects
            .terrain_objects
            .iter()
            .copied()
            .filter(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.exists()
                        && o.c.class == mc2_sim::object::ObjectClass::TerrainObject
                        && !o.c.is_disabled()
                })
            })
            .filter_map(|h| {
                let b = *boxes.structures.get(&h.0)?;
                let watch = w.objects.watch_id_peek(h);
                if watch.is_none() || !snap.picks.iter().any(|p| p.watch_id == watch.0) {
                    return None;
                }
                // Solid objects only (a 2-triangle ground decal has no volume above its position).
                if boxes
                    .structure_triangles
                    .get(&h.0)
                    .is_none_or(|t| t.len() < 6)
                {
                    return None;
                }
                let on = independent_clicks(&run, h);
                if on.is_empty() {
                    return None;
                }
                Some((watch, b, on))
            })
            .collect::<Vec<_>>()
    };
    // The biggest on screen first: a small or edge object can leave view between clicks.
    let area = |b: &[f32; 4]| (b[2] - b[0]) * (b[3] - b[1]);
    found.sort_by(|x, y| area(&y.1).total_cmp(&area(&x.1)));
    assert!(
        !found.is_empty(),
        "a visible terrain object in the first frame"
    );
    // The click is a face of the object's own shape transformed with the camera in the test.
    let mut hit = false;
    for (watch, _, _) in found {
        let h = {
            let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
            w.objects
                .terrain_objects
                .iter()
                .copied()
                .find(|&h| w.objects.watch_id_peek(h) == watch)
                .unwrap()
        };
        let Some((click, _)) = silhouette_clicks(&run, h) else {
            continue;
        };
        if attacks(&right_click_at(&mut run, &lance, click, None), watch) {
            hit = true;
            break;
        }
    }
    assert!(hit, "no click on the object attacked it");
}

/// `doAttack` refuses only a friendly target (`!Team::home->isFriendly`, missiongui.cpp:1817): a
/// building of a neutral team is attacked.
#[test]
fn a_click_on_a_neutral_building_attacks_it() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let team = w.objects.get(h).unwrap().team_id() as usize;
        let home = w.teams.home;
        w.teams.relations[home][team] = mc2_sim::ai::team::Relation::Neutral;
    }
    let orders = right_click_at(&mut run, &lance, click, None);
    assert!(
        attacks(&orders, watch),
        "no attack on a neutral building: {orders:?}"
    );
}

/// `doAttack` declines a friendly target (missiongui.cpp:1817) with no order; the bridge reports no
/// unavailable notice for it (a faithful C++ refusal, not a missing feature).
#[test]
fn a_click_on_a_friendly_building_orders_nothing() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let team = w.objects.get(h).unwrap().team_id() as usize;
        let home = w.teams.home;
        w.teams.relations[home][team] = mc2_sim::ai::team::Relation::Friendly;
    }
    let orders = right_click_at(&mut run, &lance, click, None);
    assert!(
        !attacks(&orders, watch) && orders.iter().all(|o| o.target != watch),
        "a friendly building was ordered against: {orders:?}"
    );
    let notes = hud(&run).unavailable_summary();
    assert!(
        notes.iter().all(|n| !n.contains("friendly target")
            && !n.contains("capture eligibility")
            && !n.contains("bridge context")),
        "attack/capture residue notice remained: {notes:?}"
    );
}

/// A capturable building takes the capture branch of `doAttack` (missiongui.cpp:1811), ordering
/// the selected mech lance to capture that building.
#[test]
fn a_click_on_a_captureable_building_orders_capture_for_the_lance() {
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        w.objects
            .get_mut(h)
            .unwrap()
            .c
            .set_flag(mc2_sim::object::flags::CAPTURABLE, true);
    }
    let orders = right_click_at(&mut run, &lance, click, None);
    assert!(
        !attacks(&orders, watch),
        "a capturable building was attacked: {orders:?}"
    );
    assert_eq!(
        orders.len(),
        lance.len(),
        "capture was ordered for each selected mech"
    );
    assert!(
        orders
            .iter()
            .all(|o| { o.code == mc2_sim::command::OrderCode::Capture && o.target == watch }),
        "the lance did not receive capture orders for the picked building: {orders:?}"
    );
    let notes = hud(&run).unavailable_summary();
    assert!(
        notes.iter().all(|n| !n.contains("friendly target")
            && !n.contains("capture eligibility")
            && !n.contains("bridge context")),
        "attack/capture residue notice remained: {notes:?}"
    );
}

/// A `SpinMe` building (mc2_01's `Nav1` marker) is not selectable (`isSelectable`, bdactor.h:346-349) and
/// `missiongui.cpp:1245-1248` clears it as a click target: a visible click on it is no attack order.
#[test]
fn a_spinning_marker_building_is_not_a_pick() {
    let Some(mut run) = start(130) else { return };
    let spin = |w: &mc2_sim::World, h: mc2_sim::ObjectHandle| {
        matches!(
            w.appearance.get(h),
            Some(mc2_sim::appearance::Appearance::Building(b))
                if w.appearance.building_types[b.type_index].spin_me
        )
    };
    let h = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .driver
        .sim()
        .world
        .objects
        .buildings
        .iter()
        .copied()
        .find(|&h| {
            let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
            spin(w, h) && w.objects.get(h).is_some_and(|o| o.exists())
        })
        .expect("mc2_01 has a SpinMe building");
    // Declared input: the marker is far from the camera, so give it a drawn state, a box and faces
    // (the picker's own inputs), leaving the selectability rule the only reason it is not a pick.
    {
        let rt = run.core.mission.as_mut().unwrap();
        let turn = rt.driver.sim().world.clock.turn as i32;
        let w = &mut rt.driver.sim_mut().world;
        // the script enables the marker in play; here it is a declared input
        w.objects.get_mut(h).unwrap().c.status = mc2_sim::object::ObjectStatus::Normal;
        w.objects.get_mut(h).unwrap().c.windows_visible = turn;
        if let Some(mc2_sim::appearance::Appearance::Building(b)) =
            w.appearance.by_handle[h.0 as usize].as_mut()
        {
            b.in_view = true;
        }
        w.objects.watch_id(h);
        let boxes = rt.driver.pick_boxes_mut();
        boxes.structures.insert(h.0, [100.0, 100.0, 140.0, 140.0]);
        boxes
            .structure_triangles
            .insert(h.0, vec![[100.0, 100.0, 140.0, 100.0, 100.0, 140.0]].into());
    }
    let picked = |run: &HeadlessRun<LiveSim>| {
        let d = &run.core.mission.as_ref().unwrap().driver;
        let w = &d.sim().world;
        let watch = w.objects.watch_id_peek(h);
        mc2_app::hud_data::snapshot(w, d.pick_boxes())
            .picks
            .iter()
            .any(|p| p.watch_id == watch.0)
    };
    assert!(!picked(&run), "a spinning marker is a pick");
    // the same input without `SpinMe` is a pick, so the refusal above is the rule's
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let ty = match w.appearance.get(h) {
            Some(mc2_sim::appearance::Appearance::Building(b)) => b.type_index,
            _ => unreachable!(),
        };
        w.appearance.building_types[ty].spin_me = false;
    }
    assert!(picked(&run), "without SpinMe the same building is a pick");
}

/// The watch ids of the snapshot's picks.
fn pick_ids(run: &HeadlessRun<LiveSim>) -> Vec<u32> {
    let d = &run.core.mission.as_ref().unwrap().driver;
    mc2_app::hud_data::snapshot(&d.sim().world, d.pick_boxes())
        .picks
        .iter()
        .map(|p| p.watch_id)
        .collect()
}

/// `findObjectByMouse` takes a mover only when `isOnGUI()` and `IsGameSelectTerrainPosition` hold
/// (objmgr.cpp:2263): a mover that is off the GUI or on the map border is no pick.
#[test]
fn a_mover_off_the_gui_or_on_the_map_border_is_not_a_pick() {
    let Some(mut run) = start(130) else { return };
    let (h, watch) = {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let ids = pick_ids(&run);
        w.objects
            .movers
            .iter()
            .copied()
            .map(|h| (h, w.objects.watch_id_peek(h)))
            .find(|(_, wid)| !wid.is_none() && ids.contains(&wid.0))
            .expect("a mover that is a pick")
    };
    let set_gui = |run: &mut HeadlessRun<LiveSim>, on: bool| {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        match &mut w.objects.get_mut(h).unwrap().kind {
            mc2_sim::object::Kind::Mech(m) => m.m.flags.on_gui = on,
            mc2_sim::object::Kind::Vehicle(v) => v.m.flags.on_gui = on,
            _ => unreachable!(),
        }
    };
    set_gui(&mut run, false);
    assert!(
        !pick_ids(&run).contains(&watch.0),
        "an off-GUI mover is a pick"
    );
    set_gui(&mut run, true);
    assert!(
        pick_ids(&run).contains(&watch.0),
        "the eligible mover is a pick"
    );
    // the border: within two vertices of the map edge
    let saved = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let side = w.terrain.vertices_per_side as f32 * mc2_sim::terrain::WORLD_UNITS_PER_VERTEX;
        let o = w.objects.get_mut(h).unwrap();
        let saved = o.c.position;
        o.c.position.x = side / 2.0 - mc2_sim::terrain::WORLD_UNITS_PER_VERTEX;
        saved
    };
    assert!(
        !pick_ids(&run).contains(&watch.0),
        "a border mover is a pick"
    );
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    w.objects.get_mut(h).unwrap().c.position = saved;
    assert!(pick_ids(&run).contains(&watch.0));
}

/// A terrain object whose type has damage level 36000000 is a rock clump and never a target
/// (objmgr.cpp:2199).  No shipped terrain type carries that level, so the type table is a declared
/// input: the same visible terrain object is a pick, then not once its type is a rock clump.
#[test]
fn a_rock_clump_is_not_a_pick() {
    let Some(mut run) = start(130) else { return };
    let (watch, num) = {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let ids = pick_ids(&run);
        w.objects
            .terrain_objects
            .iter()
            .copied()
            .map(|h| {
                (
                    w.objects.watch_id_peek(h),
                    w.objects.get(h).unwrap().c.type_handle,
                )
            })
            .find(|(wid, _)| !wid.is_none() && ids.contains(&wid.0))
            .expect("a terrain object that is a pick")
    };
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    let mut types = w.objects.types.as_ref().unwrap().types.clone();
    match &mut types.get_mut(&(num as u32)).unwrap().data {
        mc2_sim_data::objtype::ObjectTypeData::TerrainObject(t) => t.damage_level = 36_000_000.0,
        _ => unreachable!(),
    }
    w.objects.types = Some(std::rc::Rc::new(mc2_sim_data::objtype::ObjectTable {
        types,
        failures: Vec::new(),
    }));
    assert!(!pick_ids(&run).contains(&watch.0), "a rock clump is a pick");
}

/// `PlayerDriver::tick` on the live simulation across a lance member being disabled: the driver
/// selects the lance, one mech is then disabled in the sim (the unit bar greys it), and the driver
/// goes on issuing physical orders (Select Visible, then a tactical-map move) that reach the two
/// remaining mechs through the input path; the disabled one gets none.
#[test]
fn the_p3_driver_tick_keeps_ordering_across_a_member_being_disabled() {
    use mc2_app::p3_driver::PlayerDriver;
    let Some(mut run) = start(1) else { return };
    let all = lance(world(&run));
    assert!(all.len() >= 3, "the lance has three mechs");
    let victim = all[2];
    let mut driver = PlayerDriver::new();
    let mut disabled_at = None;
    let mut ordered = Vec::new();
    for _ in 0..6_000 {
        driver.tick(&mut run);
        run.step(None).unwrap();
        let turn = run.core.mission.as_ref().unwrap().turn;
        if disabled_at.is_none() && hud(&run).selection.selected.len() == all.len() {
            // the whole lance is selected: now one member is disabled
            let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
            let h = w.objects.find_by_part_id(PartId(victim)).unwrap();
            w.objects.get_mut(h).unwrap().c.status = mc2_sim::object::ObjectStatus::Disabled;
            disabled_at = Some(turn);
        }
        if disabled_at.is_some() {
            ordered = all
                .iter()
                .filter(|p| player_order_code(&run, **p).is_some())
                .copied()
                .collect();
            if !ordered.is_empty() {
                break;
            }
        }
    }
    assert!(disabled_at.is_some(), "the driver selected the whole lance");
    let mut expect: Vec<i32> = all.iter().copied().filter(|p| *p != victim).collect();
    expect.sort_unstable();
    ordered.sort_unstable();
    assert_eq!(ordered, expect, "the remaining members were ordered");
    assert!(
        player_order_code(&run, victim).is_none(),
        "the disabled member got no order"
    );
    assert!(
        !hud(&run).selection.selected.contains(&victim),
        "the disabled member left the selection"
    );
}

/// Child half of [`the_command_click_trace_env_path_records_a_no_pick`]: runs only when spawned with
/// `MC2_P3_TRACE=1`, so the real environment-gated path (not `capture_trace`) prints its lines.
#[test]
fn trace_env_child() {
    if std::env::var_os("MC2_P3_TRACE_CHILD").is_none() {
        return;
    }
    let Some(mut run) = start(130) else { return };
    let lance = select_lance(&mut run);
    let (watch, h, _, _) = real_enemy_building(&mut run);
    let click = independent_face_click(&run, h);
    let orders = right_click_at(&mut run, &lance, click, None);
    assert_eq!(orders.len(), lance.len());
    assert!(orders.iter().all(|o| o.target == watch));
    right_click_at(&mut run, &lance, (60.0, 140.0), None);
}

/// With `MC2_P3_TRACE=1` in the environment, `HudRuntime::update` prints the click trace: the attack
/// click with its watch id and the empty click as `picked watch none`.  Runs the child test above in a
/// subprocess so the environment is real and its stderr can be read.
#[test]
fn the_command_click_trace_env_path_records_a_no_pick() {
    if data().is_none() {
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "trace_env_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("MC2_P3_TRACE", "1")
        .env("MC2_P3_TRACE_CHILD", "1")
        .output()
        .expect("spawn the child test");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child failed: {err}");
    if err.contains("no GPU") {
        return;
    }
    let lines: Vec<&str> = err
        .lines()
        .filter(|l| l.starts_with("p3-trace click"))
        .collect();
    assert_eq!(lines.len(), 2, "one line per completed click: {err}");
    assert!(
        lines[0].contains("picked watch ") && !lines[0].contains("picked watch none"),
        "{}",
        lines[0]
    );
    assert!(lines[1].contains("picked watch none"), "{}", lines[1]);
}

/// The mission-script effect `win_the_objectives` has in `p3_integration`: every home objective
/// is decided (no activate-on-flag primary waits).
fn decide_home_objectives(run: &mut HeadlessRun<LiveSim>) {
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    let home = w.teams.home;
    for set in w.mission.objectives.iter_mut().filter(|s| s.team == home) {
        for e in &mut set.entries {
            e.runtime.active = false;
            e.def.activate_on_flag = false;
        }
    }
}

/// The shipped key table has no `toggleGUI` (debug-only in the C++), so the hidden GUI is set
/// directly on the production HUD.
fn set_gui(run: &mut HeadlessRun<LiveSim>, visible: bool) {
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .unwrap()
        .set_gui_visible(visible);
}

/// `controlgui.cpp:374,431-436,514`: the results run only while the GUI is drawn, and they clear
/// the info window's unit.
#[test]
fn results_wait_for_the_gui_and_clear_the_info_window() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    run.step(None).unwrap();
    assert!(
        hud(&run).info_window().unit().is_some(),
        "a selected unit fills the info window before the results"
    );

    set_gui(&mut run, false);
    decide_home_objectives(&mut run);
    for _ in 0..30 {
        run.step(None).unwrap();
        assert!(
            !hud(&run).results_status_frame_drawn(),
            "no status frame is drawn while the GUI is hidden"
        );
    }
    assert_eq!(hud(&run).objectives_view().results_time, 0.0);
    assert!(
        hud(&run).snapshot().results_started,
        "the mission is decided"
    );

    set_gui(&mut run, true);
    let mut drawn = false;
    for _ in 0..30 {
        run.step(None).unwrap();
        if hud(&run).results_status_frame_drawn() {
            drawn = true;
            break;
        }
    }
    assert!(drawn, "showing the GUI starts the results presentation");
    assert!(hud(&run).objectives_view().results_time > 0.0);
    assert!(
        hud(&run).info_window().unit().is_none(),
        "renderResults empties the info window"
    );
}

/// `gametacmap.cpp:373-376`: a shut-down home unit keeps its blip, its ring is gone.
#[test]
fn a_shut_down_home_unit_keeps_its_blip_and_loses_its_ring() {
    let Some(mut run) = start(60) else { return };
    let part = lance(world(&run))[0];
    run.step(None).unwrap();
    let pos = {
        let w = world(&run);
        let o = w
            .objects
            .movers
            .iter()
            .filter_map(|&h| w.objects.get(h))
            .find(|o| o.c.part_id.0 == part)
            .unwrap();
        (o.c.position.x, o.c.position.y)
    };
    let ring_of = |run: &HeadlessRun<LiveSim>| {
        hud(run)
            .tac_map_input()
            .units
            .iter()
            .find(|u| u.pos == pos && u.team == mc2_ui::tacmap::TacTeam::Ours)
            .map(|u| u.sensor_range_m)
    };
    let live = ring_of(&run).expect("the home unit's blip");
    assert!(live > 0.0, "an active unit has a sensor ring");
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let h = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| w.objects.get(h).is_some_and(|o| o.c.part_id.0 == part))
            .unwrap();
        w.objects.get_mut(h).unwrap().c.status = ObjectStatus::ShutDown;
    }
    run.step(None).unwrap();
    // the unit has not moved this frame, so its blip is still found at the same place
    assert_eq!(ring_of(&run), Some(0.0), "blip kept, ring removed");
}

/// `MechWarrior::messagePlaying` reaches the unit bar from the audio callback and leaves again.
#[test]
fn the_unit_bar_shows_a_pilot_talking_while_the_radio_plays() {
    use mc2_audio::rng::SeededRng;
    use mc2_audio::{AudioEvent, PilotId, RadioMessageType};
    // the production session: it alone registers the battle sound sink that registers the pilots
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::production(&opts) {
        Ok(r) => r,
        Err(e) if common::skip_gpu_test_without_adapter(&e) => return,
        Err(e) => panic!("the production headless session needs a GPU and the game data: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    // This production session enters mc2_01's opening movie on its first
    // mission frame. Dismiss it through the actual player route before
    // asserting ordinary HUD feedback.
    run.step(None).unwrap();
    dismiss_opening_movie(&mut run);
    step_to(&mut run, 60);
    let (part, slot) = {
        let unit = &hud(&run).snapshot().units[0];
        (unit.part_id as u32, unit.pilot_slot)
    };
    assert_eq!(hud(&run).unit_bar().message_playing(part), Some(false));
    let mut rng = SeededRng::new(7);
    run.core
        .audio
        .as_mut()
        .expect("headless game sound")
        .game_mut()
        .handle(
            &AudioEvent::Radio {
                pilot: PilotId(slot),
                message: RadioMessageType::Ack.index() as i64,
                under_home_command: true,
            },
            &mut rng,
        );
    let (mut talked, mut finished) = (false, false);
    for _ in 0..900 {
        run.step(None).unwrap();
        match hud(&run).unit_bar().message_playing(part) {
            Some(true) => talked = true,
            Some(false) if talked => {
                finished = true;
                break;
            }
            _ => {}
        }
    }
    assert!(talked, "the audio callback marks the pilot talking");
    assert!(finished, "and the end of the message clears it");
}

/// `objective.cpp:2203-2218`: an objective switched on by its flag during play reaches the HUD
/// with the time it was switched on.
#[test]
fn a_flag_activated_objective_reaches_the_hud_with_its_runtime_activation_time() {
    let Some(mut run) = start(60) else { return };
    let (index, flag) = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let home = w.teams.home;
        let set = w
            .mission
            .objectives
            .iter_mut()
            .find(|s| s.team == home)
            .unwrap();
        let (i, e) = set
            .entries
            .iter_mut()
            .enumerate()
            .next()
            .expect("mc2_01 has objectives");
        e.runtime.active = false;
        e.def.activate_on_flag = true;
        e.def.activation_time = -1.0;
        e.def.activate_flag_id = "S9aTestFlag".to_string();
        (i, e.def.activate_flag_id.clone())
    };
    run.step(None).unwrap();
    assert!(!hud(&run).snapshot().objectives[index].is_active);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let home = w.teams.home;
        let set = w
            .mission
            .objectives
            .iter_mut()
            .find(|s| s.team == home)
            .unwrap();
        mc2_sim::mission::objective_eval::set_flag(&mut set.boolean_flags, &flag, true);
    }
    for _ in 0..5 {
        run.step(None).unwrap();
    }
    let view = &hud(&run).snapshot().objectives[index];
    assert!(view.is_active, "the flag switched the objective on");
    assert!(
        view.activation_time > 0.0,
        "the sim recorded when: {}",
        view.activation_time
    );
}

/// `controlgui.cpp:430`: the results start from `CObjectives::Status()`, which honours a reset
/// flag. A successful primary whose reset flag is set and whose condition is no longer met is
/// undetermined, so no results start; they start again once it is decided.
#[test]
fn a_reset_objective_does_not_start_the_results_until_decided_again() {
    use mc2_sim::mission::objective::ObjectiveStatus;
    use mc2_sim_data::mission::{BooleanFlag, Condition};
    let Some(mut run) = start(60) else { return };
    run.step(None).unwrap();
    decide_home_objectives(&mut run);
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    let home = w.teams.home;
    let set = w
        .mission
        .objectives
        .iter_mut()
        .find(|s| s.team == home)
        .unwrap();
    set.boolean_flags = vec![
        BooleanFlag {
            flag_id: "rst".into(),
            value: true,
        },
        BooleanFlag {
            flag_id: "ok".into(),
            value: false,
        },
    ];
    let e = &mut set.entries[0];
    e.def.priority = 1;
    e.def.reset_status_on_flag = true;
    e.def.reset_status_flag_id = "rst".into();
    e.conditions = vec![mc2_sim::mission::objective::ResolvedCondition {
        condition: Condition::BooleanFlagIsSet {
            flag_id: "ok".into(),
            value: true,
        },
        wid: Default::default(),
    }];
    e.runtime.active = true;
    e.runtime.resolved = true;
    e.runtime.status = ObjectiveStatus::Successful;
    let boxes = Default::default();

    // The cached status is successful, but `Status()` resets it and the condition is unmet.
    let snap = mc2_app::hud_data::snapshot(w, &boxes);
    assert!(!snap.results_started, "a reset, unmet objective is open");
    assert!(!snap.home_successful);

    // Once the condition holds the objective is decided again.
    w.mission
        .objectives
        .iter_mut()
        .find(|s| s.team == home)
        .unwrap()
        .boolean_flags[1]
        .value = true;
    let snap = mc2_app::hud_data::snapshot(w, &boxes);
    assert!(snap.results_started);
    assert!(snap.home_successful);
}

/// Steps until the settled results are drawn (the status frame has arrived); false on timeout.
fn step_to_settled_results(run: &mut HeadlessRun<LiveSim>) -> bool {
    for _ in 0..120 {
        run.step(None).unwrap();
        if hud(run).results_status_frame_drawn() && hud(run).objectives_view().results_time >= 0.3 {
            return true;
        }
    }
    false
}

/// `controlgui.cpp:540`: the grey-out covers `Environment.screenWidth x screenHeight`, not the
/// fitted HUD rectangle, so a 1920x1080 window is dimmed edge to edge.
#[test]
fn results_grey_out_covers_the_whole_wide_screen() {
    let Some(mut run) = start(60) else { return };
    run.step(None).unwrap();
    decide_home_objectives(&mut run);
    assert!(step_to_settled_results(&mut run));
    let grey = hud(&run).results_overlay(1920, 1080, true);
    let (l, t, r, b) = grey.bounds().expect("the settled results grey the screen");
    assert_eq!((l, t, r, b), (0.0, 0.0, 1920.0, 1080.0));
}

/// `controlgui.cpp:589-596`: a normal mission fades to black in the last second of the results;
/// `neverEndingStory` never does.
#[test]
fn results_fade_to_black_only_in_a_normal_mission() {
    for never_ending in [false, true] {
        let Some(mut run) = start(60) else { return };
        run.step(None).unwrap();
        run.core
            .mission
            .as_mut()
            .unwrap()
            .driver
            .sim_mut()
            .world
            .mission
            .term
            .never_ending_story = never_ending;
        decide_home_objectives(&mut run);
        assert!(step_to_settled_results(&mut run));
        assert!(hud(&run)
            .results_overlay(800, 600, false)
            .bounds()
            .is_none());
        let mut faded = false;
        for _ in 0..3000 {
            run.step(None).unwrap();
            if hud(&run)
                .results_overlay(800, 600, false)
                .bounds()
                .is_some()
            {
                faded = true;
                break;
            }
        }
        assert_eq!(faded, !never_ending, "never_ending_story = {never_ending}");
    }
}

/// `controlgui.cpp:430,512`: `renderStatusInfo` is never cleared, so the presentation keeps
/// running after a reset flag makes the objectives undetermined again.
#[test]
fn results_continue_after_an_objective_resets() {
    let Some(mut run) = start(60) else { return };
    run.step(None).unwrap();
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .sim_mut()
        .world
        .mission
        .term
        .never_ending_story = true;
    decide_home_objectives(&mut run);
    assert!(step_to_settled_results(&mut run));
    let before = hud(&run).objectives_view().results_time;
    // Make the home objectives undetermined: an active, unresolved primary.
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let home = w.teams.home;
        for set in w.mission.objectives.iter_mut().filter(|s| s.team == home) {
            for e in &mut set.entries {
                e.runtime.active = true;
                e.runtime.resolved = false;
                e.runtime.status = mc2_sim::mission::objective::ObjectiveStatus::Undetermined;
            }
        }
    }
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    assert!(
        !hud(&run).snapshot().results_started,
        "the list is open again"
    );
    assert!(hud(&run).objectives_view().results_time > before);
    assert!(hud(&run).results_status_frame_drawn());
}

// ---- support-vehicle buttons and selection capabilities (control panel, controlgui.cpp:1790-1890) -------------------------

/// `(button, cost, vehicleIDs[])` of the five support-vehicle buttons (controlgui.cpp:85-120).
fn support_buttons() -> [(mc2_ui::control::VehicleKind, i32, i32); 5] {
    use mc2_ui::control::VehicleKind as K;
    [
        (K::GuardTower, 8000, 415),
        (K::RepairTruck, 6000, 182),
        (K::Scout, 2000, 393),
        (K::Minelayer, 4000, 120),
        (K::RecoveryTeam, 10000, 147),
    ]
}

fn button_states(
    run: &mut HeadlessRun<LiveSim>,
) -> Vec<(
    mc2_ui::control::VehicleKind,
    mc2_ui::control::ButtonVisualState,
)> {
    support_buttons()
        .iter()
        .map(|&(kind, ..)| (kind, strike_button(run, kind).1))
        .collect()
}

/// Per-button enable rules: a mission flag that is off, a cost the resource points cannot pay, and a full unit bar each disable
/// a button (`updateVehicleTab`); no restriction leaves it enabled. Pressing a disabled button spends nothing.
#[test]
fn each_support_button_follows_the_mission_flag_and_the_resource_points() {
    use mc2_ui::control::{ButtonVisualState as S, VehicleKind as K};
    let Some(data) = data() else { return };
    // Repair, mine layer and salvage craft are off; scout and guard tower on (the guard tower defaults off).
    let root = data_with_settings(
        &data,
        "b RepairVehicleEnabledDefault = FALSE\r\nb MineLayersEnabledDefault = FALSE\r\nb SalvageCraftEnabledDefault = FALSE\r\n\
         b ScoutCoptersEnabledDefault = TRUE\r\nb ArtilleryPieceEnabledDefault = TRUE\r\n",
        "support-flags",
    );
    let Some(mut run) = start_in(root.clone(), 130) else {
        return;
    };
    select_lance(&mut run);
    fund(&mut run, 100_000);
    let states = button_states(&mut run);
    let want = |kind: K| match kind {
        K::Scout | K::GuardTower => S::Enabled,
        _ => S::Disabled,
    };
    for (kind, state) in &states {
        assert_eq!(*state, want(*kind), "{kind:?} with plenty of points");
    }
    // 3000 points pay only the scout (2000); the guard tower (8000) is now too dear
    fund(&mut run, 3_000);
    for (kind, state) in button_states(&mut run) {
        let want = if kind == K::Scout {
            S::Enabled
        } else {
            S::Disabled
        };
        assert_eq!(state, want, "{kind:?} with 3000 points");
    }
    // pressing the ones that read disabled arms nothing and spends nothing
    press_vehicle_button(&mut run, K::GuardTower);
    press_vehicle_button(&mut run, K::Minelayer);
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert_eq!(sim_points(&run), 3_000);
    // the enabled one arms and, once pressed, every other button is locked out
    press_vehicle_button(&mut run, K::Scout);
    assert_eq!(hud(&run).control_panel().selected_vehicle, Some(K::Scout));
    fund(&mut run, 100_000);
    for (kind, state) in button_states(&mut run) {
        let want = if kind == K::Scout {
            S::Pressed
        } else {
            S::Disabled
        };
        assert_eq!(state, want, "{kind:?} while the scout is armed");
    }
    let _ = std::fs::remove_dir_all(root);
}

/// With every flag on, no points disable all five and 100,000 enable all five.
#[test]
fn all_support_buttons_are_enabled_with_the_flags_on_and_disabled_without_points() {
    use mc2_ui::control::ButtonVisualState as S;
    let Some(data) = data() else { return };
    let root = data_with_settings(
        &data,
        "b ArtilleryPieceEnabledDefault = TRUE\r\n",
        "support-all",
    );
    let Some(mut run) = start_in(root.clone(), 130) else {
        return;
    };
    select_lance(&mut run);
    fund(&mut run, 0);
    assert!(button_states(&mut run)
        .iter()
        .all(|(_, s)| *s == S::Disabled));
    fund(&mut run, 100_000);
    // the recovery craft also needs a pilot left in the roster (`gotPilotsLeft`)
    let pilots = hud(&run).snapshot().pilots_remaining;
    for (kind, state) in button_states(&mut run) {
        let want = if kind == mc2_ui::control::VehicleKind::RecoveryTeam && !pilots {
            S::Disabled
        } else {
            S::Enabled
        };
        assert_eq!(state, want, "{kind:?}");
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Pressing an enabled support button and clicking the ground issues one `BeginVtol` to the sim, which charges the cost once and
/// starts the VTOL flight (`paintingVtol`); the headless run goes press -> click -> sim step.
#[test]
fn a_support_button_and_a_ground_click_start_one_vtol_flight_in_the_sim() {
    use mc2_ui::control::VehicleKind as K;
    let Some(data) = data() else { return };
    let root = data_with_settings(
        &data,
        "b ArtilleryPieceEnabledDefault = TRUE\r\n",
        "support-press",
    );
    let Some(mut run) = start_in(root.clone(), 130) else {
        return;
    };
    select_lance(&mut run);
    fund(&mut run, 100_000);
    press_vehicle_button(&mut run, K::RepairTruck);
    assert_eq!(
        hud(&run).control_panel().selected_vehicle,
        Some(K::RepairTruck)
    );
    assert_eq!(
        sim_points(&run),
        100_000 - 6_000,
        "the press spends the cost"
    );
    assert!(!world(&run).vtol.in_flight(HOME_COMMANDER));
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .record_inputs(true);
    right_click_ground(&mut run);
    assert!(
        !refused(&run, "support vehicle"),
        "{:?}",
        hud(&run).unavailable_summary()
    );
    assert!(
        world(&run).vtol.in_flight(HOME_COMMANDER),
        "the click called the VTOL in"
    );
    assert_eq!(sim_points(&run), 100_000 - 6_000, "charged once");
    // the button stays pressed while the VTOL flies (`paintingVtol`) and is released, without a refund, when the flight ends
    assert_eq!(
        hud(&run).control_panel().selected_vehicle,
        Some(K::RepairTruck)
    );
    let movers = world(&run).objects.movers.len();
    for _ in 0..600 {
        run.step(None).unwrap();
        if !world(&run).vtol.in_flight(HOME_COMMANDER) {
            break;
        }
    }
    run.step(None).unwrap();
    assert!(!world(&run).vtol.in_flight(HOME_COMMANDER));
    assert_eq!(
        world(&run).objects.movers.len(),
        movers + 1,
        "the truck was dropped"
    );
    assert_eq!(hud(&run).control_panel().selected_vehicle, None);
    assert_eq!(sim_points(&run), 100_000 - 6_000, "no refund");
    let _ = std::fs::remove_dir_all(root);
}

fn panel_state(
    run: &HeadlessRun<LiveSim>,
    id: mc2_ui::control::ControlId,
) -> mc2_ui::control::ButtonVisualState {
    hud(run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.id == Some(id))
        .map(|b| b.state)
        .expect("the panel button")
}

/// Selecting the support vehicles the sim dropped, one at a time through the production picker, drives the control panel: the
/// mine layer shows the lay-mines button, the repair truck the repair button, the guard tower its button, a vehicle whose type
/// definition carries `RecoverPoints` the salvage button (spawn computes the flag from that upstream definition), and a scout
/// or a mech none. A selected unit with a queued order reports it from the sim's pilot queue.
#[test]
fn selecting_dropped_support_vehicles_drives_the_control_panel_buttons() {
    use mc2_sim::command::PlayerCommand;
    use mc2_ui::control::{ButtonVisualState as S, ControlId as C};
    let Some(mut run) = start(130) else { return };
    let specialty = [C::LayMines, C::Repair, C::Salvage, C::GuardTower];
    let panel = |run: &HeadlessRun<LiveSim>| specialty.map(|id| panel_state(run, id));
    // a selected mech of the lance has no support button
    let all = select_lance(&mut run);
    assert_eq!(
        panel(&run),
        [S::Hidden; 4],
        "the lance shows no support button"
    );
    let origin = {
        let w = world(&run);
        let h = w
            .objects
            .find_by_part_id(PartId(all[0]))
            .expect("a lance member");
        w.objects.get(h).unwrap().c.position
    };
    // the recovery capability enters as upstream data: the repair truck's vehicle definition gets RecoverPoints, and the drop's spawn
    // derives `can_recover` from it (spawn.rs `recover_points > 0`); the unmodified truck is dropped first
    let drops: [(i32, &str); 5] = [
        (182, "repair"),
        (120, "mine"),
        (415, "tower"),
        (393, "scout"),
        (182, "salvage"),
    ];
    let mut parts = std::collections::BTreeMap::new();
    for (i, (vehicle_id, name)) in drops.into_iter().enumerate() {
        if name == "salvage" {
            // the plain repair truck's drop has loaded its packet; give the definition RecoverPoints for the next 182 drop (salvage outranks repair on the panel)
            let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
            let mut def = (**w.data.vehicle_objects.get(&182).unwrap()).clone();
            assert_eq!(
                def.recover_points, 0,
                "the shipped repair truck recovers nothing"
            );
            def.recover_points = 100;
            w.data.vehicle_objects.insert(182, std::rc::Rc::new(def));
        }
        let at = {
            let w = world(&run);
            let at = mc2_sim::math::Vec3 {
                x: origin.x + 60.0 + 30.0 * i as f32,
                y: origin.y + 70.0 * i as f32 - 150.0,
                ..origin
            };
            mc2_sim::math::Vec3 {
                z: w.terrain.elevation(&at),
                ..at
            }
        };
        let before = world(&run).objects.movers.len();
        run.core
            .mission
            .as_mut()
            .unwrap()
            .driver
            .inject_commands(vec![PlayerCommand::BeginVtol {
                commander: 0,
                vehicle_id,
                at,
                target: None,
            }]);
        for _ in 0..600 {
            run.step(None).unwrap();
            if world(&run).objects.movers.len() > before {
                break;
            }
        }
        let part = world(&run).vtol.last_dropped[0].expect("the dropped mover");
        parts.insert(name, part.0);
        // let the flight finish so the next drop may start
        for _ in 0..400 {
            run.step(None).unwrap();
        }
    }
    let pick = |run: &mut HeadlessRun<LiveSim>, name: &str| {
        let part = parts[name];
        let (x, y) = screen_of(run, part);
        click(run, f64::from(x), f64::from(y));
        assert_eq!(selected(run), [part], "{name} is the one selected unit");
        panel(run)
    };
    assert_eq!(
        pick(&mut run, "mine"),
        [S::Enabled, S::Hidden, S::Hidden, S::Hidden]
    );
    assert_eq!(
        pick(&mut run, "repair"),
        [S::Hidden, S::Pressed, S::Hidden, S::Hidden]
    );
    assert_eq!(
        pick(&mut run, "salvage"),
        [S::Hidden, S::Hidden, S::Pressed, S::Hidden]
    );
    assert_eq!(
        pick(&mut run, "tower"),
        [S::Hidden, S::Hidden, S::Hidden, S::Enabled]
    );
    assert_eq!(pick(&mut run, "scout"), [S::Hidden; 4]);
    {
        let w = world(&run);
        let flag = |name: &str| {
            let h = w.objects.find_by_part_id(PartId(parts[name])).unwrap();
            mc2_sim::mover::refit::can_recover(w, h)
        };
        assert!(
            flag("salvage") && !flag("repair"),
            "spawn computed can_recover from the definition"
        );
    }
    // a unit with a queued order: Ctrl+right-click queues a move on the selected mech, and the sim's pilot queue reports it
    let member = all[0];
    let (x, y) = screen_of(&run, member);
    click(&mut run, f64::from(x), f64::from(y));
    assert_eq!(selected(&run), [member]);
    let queued = |run: &HeadlessRun<LiveSim>| {
        hud(run)
            .snapshot()
            .units
            .iter()
            .find(|u| u.part_id == member)
            .unwrap()
            .caps
            .has_queued_orders
    };
    assert!(!queued(&run), "nothing is queued yet");
    hold(&mut run, &["ctrl"], true);
    right_click(&mut run, 60.0, 300.0);
    run.step(None).unwrap();
    assert!(queued(&run), "the queued move shows in the capability flag");
    hold(&mut run, &["ctrl"], false);
    run.step(None).unwrap();
}

/// A shipped capturable resource vehicle is picked by the production HUD, becomes a Capture
/// command, then reports the pilot's blocked-capture radio notice when an armed enemy moves into
/// capture range before the selected mech arrives.
#[test]
fn a_shipped_captureable_vehicle_click_orders_capture_and_reports_a_blocker() {
    let Some(data) = data() else { return };
    let Some(mut run) = start_mission(data, "mc2_23", 130) else {
        return;
    };
    let part = select_one_lance_mech(&mut run);
    let (vehicle, watch, original_team, target_pos, capturer) = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let home = w.teams.home as i32;
        let h = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    matches!(o.kind, Kind::Vehicle(_))
                        && mc2_sim::ai::capture::is_captureable(w, h, home)
                })
            })
            .expect("mc2_23 has a shipped capturable ground vehicle");
        let capturer = w.objects.find_by_part_id(PartId(part)).unwrap();
        let origin = w.objects.get(capturer).unwrap().c.position;
        let target_pos = mc2_sim::math::Vec3 {
            x: origin.x + 200.0,
            ..origin
        };
        let o = w.objects.get_mut(h).unwrap();
        let original_team = o.team_id();
        o.c.position = target_pos;
        let watch = w.objects.watch_id(h);
        (h, watch, original_team, target_pos, capturer)
    };
    assert!(
        world(&run)
            .objects
            .get(vehicle)
            .unwrap()
            .c
            .flag(mc2_sim::object::flags::CAPTURABLE),
        "the shipped vehicle data supplies captureability"
    );
    {
        let mission = run.core.mission.as_mut().unwrap();
        mission.rig.follow = None;
        mission.rig.set_position(
            Vec3::new(target_pos.x, target_pos.y, target_pos.z),
            false,
            &mission.view.ground(),
            1.0 / 30.0,
        );
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    let click = clear_mover_pick_click(&run, watch);
    let orders = right_click_at(&mut run, &[part], click, None);
    assert_eq!(orders.len(), 1, "the click issued one player order");
    assert_eq!(orders[0].code, mc2_sim::command::OrderCode::Capture);
    assert_eq!(orders[0].target, watch);
    assert_eq!(
        world(&run).objects.get(vehicle).unwrap().team_id(),
        original_team
    );
    assert!(
        hud(&run).unavailable_summary().is_empty(),
        "the production HUD reported no missing capture context"
    );

    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let h = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                h != vehicle
                    && h != capturer
                    && w.objects.get(h).is_some_and(|o| {
                        o.team_id() != w.teams.home as i32
                            && !o.c.is_disabled()
                            && o.c.awake()
                            && mc2_sim::combat::loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
                    })
            })
            .expect("mc2_23 has an active armed enemy mover");
        mc2_sim::mover::set_position(
            w,
            h,
            mc2_sim::math::Vec3 {
                x: target_pos.x + 8.0,
                ..target_pos
            },
        );
        mc2_sim::mover::set_position(
            w,
            capturer,
            mc2_sim::math::Vec3 {
                x: target_pos.x - 8.0,
                ..target_pos
            },
        );
    }
    let mut blocked = false;
    let mut notice_seen = false;
    let mut radio_events = Vec::new();
    for _ in 0..60 {
        run.step(None).unwrap();
        let (team, slot, order) = {
            let w = world(&run);
            let slot = match &w.objects.get(capturer).unwrap().kind {
                Kind::Mech(mech) => mech.m.pilot_slot,
                _ => unreachable!(),
            };
            (
                w.objects.get(vehicle).unwrap().team_id(),
                slot,
                w.pilots.get(slot).unwrap().cur_order.code,
            )
        };
        let last_radio_events = run
            .core
            .mission
            .as_ref()
            .unwrap()
            .driver
            .last_radio_events();
        radio_events.extend_from_slice(last_radio_events);
        notice_seen |= last_radio_events
            .contains(&(u32::from(slot), mc2_sim::ai::orders::radio::CANNOT_CAPTURE));
        blocked =
            team == original_team && order != mc2_sim::command::OrderCode::Capture && notice_seen;
        if blocked {
            break;
        }
    }
    assert!(
        blocked,
        "the target stays enemy-owned, the capture order completes as refused, and the pilot reports CANNOT_CAPTURE (notice seen: {notice_seen}, radio events: {radio_events:?})"
    );
}

/// An unmodified shipped capturable building in mc2_23 shows the blocked capture cursor and the production
/// HUD refusal sound when an armed enemy prevents the selected mech from capturing it.
#[test]
fn shipped_captureable_building_shows_x_capture_and_refuses_a_blocked_click() {
    let Some(data) = data() else { return };
    let Some(mut run) = start_mission(data, "mc2_23", 130) else {
        return;
    };
    let part = select_one_lance_mech(&mut run);
    let (watch, building) = shipped_captureable_building(&mut run);
    let capturer = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let capturer = w.objects.find_by_part_id(PartId(part)).unwrap();
        let target_pos = w.objects.get(building).unwrap().c.position;
        let blocker = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                h != capturer
                    && w.objects.get(h).is_some_and(|o| {
                        o.team_id() != w.teams.home as i32
                            && !o.c.is_disabled()
                            && o.c.awake()
                            && mc2_sim::combat::loadout_of(o)
                                .is_some_and(|loadout| loadout.num_weapons() > 0)
                    })
            })
            .expect("mc2_23 has an active armed enemy mover");
        mc2_sim::mover::set_position(
            w,
            capturer,
            mc2_sim::math::Vec3 {
                x: target_pos.x - 10.0,
                ..target_pos
            },
        );
        mc2_sim::mover::set_position(
            w,
            blocker,
            mc2_sim::math::Vec3 {
                x: target_pos.x + 10.0,
                ..target_pos
            },
        );
        capturer
    };
    assert!(
        world(&run)
            .objects
            .get(building)
            .unwrap()
            .c
            .flag(mc2_sim::object::flags::CAPTURABLE),
        "the HUD fixture retains the shipped capturable-building flag"
    );
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .unwrap()
        .capture_sounds = true;
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .unwrap()
        .sound_log
        .clear();
    let (click, _) = silhouette_clicks(&run, building)
        .expect("an unmodified shipped building face clear of other picks");
    mouse(&mut run, f64::from(click.0), f64::from(click.1));
    run.step(None).unwrap();
    assert_eq!(
        hud(&run).order_cursor(),
        mc2_ui::mission_keys::state::XCAPTURE,
        "the production cursor reports a blocked capture"
    );
    let team = world(&run).objects.get(building).unwrap().team_id();
    let orders = right_click_at(&mut run, &[part], click, None);
    assert!(
        orders.iter().all(|order| {
            order.code != mc2_sim::command::OrderCode::Capture || order.target != watch
        }),
        "the blocked click issued no capture order: {orders:?}"
    );
    assert_eq!(
        world(&run).objects.get(building).unwrap().team_id(),
        team,
        "the shipped target remains on its original team"
    );
    assert_eq!(
        hud(&run)
            .sound_log
            .iter()
            .filter(|cue| **cue == Cue::Sound(sfx::INVALID_GUI as i32))
            .count(),
        1,
        "the production refusal emits exactly one INVALID_GUI cue"
    );
    assert!(
        !mc2_sim::ai::capture::blockers(
            &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world,
            building,
            capturer,
        )
        .is_empty(),
        "the shipped HUD fixture has a real enemy blocker"
    );
}
