// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! The in-mission pause menu through the production HUD and UI host, driven by pointer events on the
//! live simulation (headless, never a window; skipped without `MC2_DATA` or a GPU): SAVE and LOAD
//! (`pausewindow.cpp:383-393`, `mission.cpp:409`, `mechcmd2.cpp:2268`), OPTIONS (`bInvokeOptionsScreenFlag`),
//! RESTART (the abort prompt, `pausewindow.cpp:74-97`) and EXIT (the quit prompt).

use mc2_app::campaign_save::CampaignSaveService;
use mc2_app::game::MISSION_SAVE_FILE;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::hud::HudRuntime;
use mc2_app::live_sim::LiveSim;
use mc2_app::state::GameState;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_platform::keys;
use std::path::PathBuf;

fn start(tag: &str) -> Option<(HeadlessRun<LiveSim>, PathBuf)> {
    let Some(data) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA not set; skipping");
        return None;
    };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e) if e.contains("adapter") => {
            assert!(
                std::env::var_os("MC2_REQUIRE_GPU").is_none(),
                "MC2_REQUIRE_GPU is set but there is no adapter: {e}"
            );
            eprintln!("no GPU: {e}; skipping");
            return None;
        }
        Err(e) => panic!("{e}"),
    };
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("mc2-pause-menu-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    run.core
        .ui_host_mut()
        .expect("the production host")
        .set_campaign_save_service(CampaignSaveService::new(&dir));
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    // Normal mission GUI input is correctly gated during mc2_01's opening movie. Dismiss it
    // through the production Escape/ForceMovieEnd route before driving the pause panel.
    run.step(None).unwrap();
    dismiss_opening_movie(&mut run);
    while sim_frame(&run) < 90 {
        run.step(None).unwrap();
    }
    Some((run, dir))
}

fn sim_frame(run: &HeadlessRun<LiveSim>) -> u64 {
    run.core.mission.as_ref().unwrap().clock.frame()
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

fn click(run: &mut HeadlessRun<LiveSim>, at: (f32, f32)) {
    run.event(InputEvent::CursorMoved {
        x: f64::from(at.0),
        y: f64::from(at.1),
    });
    frames(run, 3);
    // a press that lasts a few simulation frames, as a hand makes one (the GUI's click edge needs the
    // button down at one frame and up at the next)
    for pressed in [true, false] {
        run.event(InputEvent::Button {
            button: MouseButton::Left,
            pressed,
        });
        frames(run, 4);
    }
}

fn frames(run: &mut HeadlessRun<LiveSim>, n: usize) {
    for _ in 0..n {
        run.step(None).unwrap();
    }
}

fn dismiss_opening_movie(run: &mut HeadlessRun<LiveSim>) {
    if !run
        .core
        .mission
        .as_ref()
        .is_some_and(|mission| mission.sim_presentation.movie_mode)
    {
        return;
    }
    run.event(InputEvent::Key {
        key: keys::KEY_ESCAPE,
        phys_id: keys::KEY_ESCAPE as u16,
        pressed: true,
        repeat: false,
        text: None,
    });
    run.step(None).unwrap();
    run.event(InputEvent::Key {
        key: keys::KEY_ESCAPE,
        phys_id: keys::KEY_ESCAPE as u16,
        pressed: false,
        repeat: false,
        text: None,
    });
    run.step(None).unwrap();
    for _ in 0..400 {
        if !run
            .core
            .mission
            .as_ref()
            .is_some_and(|mission| mission.sim_presentation.movie_mode)
        {
            run.step(None).unwrap();
            return;
        }
        run.step(None).unwrap();
    }
    panic!("mc2_01's Escape dismissal never ended the opening movie");
}

/// Where every mover stands and the scenario clock: what a save has to bring back.
fn state(run: &HeadlessRun<LiveSim>) -> (u32, Vec<(u32, u32)>) {
    let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
    let pos = w
        .objects
        .movers
        .iter()
        .filter_map(|&h| w.objects.get(h))
        .map(|o| (o.c.position.x.to_bits(), o.c.position.y.to_bits()))
        .collect();
    (w.clock.scenario_time.to_bits(), pos)
}

/// The control panel's pause button (`cxx_id` 11), then the panel's slide-in.
fn open_pause_menu(run: &mut HeadlessRun<LiveSim>) {
    let rect = hud(run)
        .control_panel()
        .buttons
        .iter()
        .find(|b| b.cxx_id == 11)
        .expect("the pause button")
        .rect;
    let at = hud(run).hud_to_game(((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2));
    click(run, at);
    assert!(hud(run).paused(), "the pause button paused the mission");
    frames(run, 90);
}

fn click_pause_button(run: &mut HeadlessRun<LiveSim>, id: i32) {
    let at = hud(run)
        .pause_button_point(id)
        .unwrap_or_else(|| panic!("pause button {id}"));
    click(run, at);
}

const SAVE: i32 = 1;
const LOAD: i32 = 2;
const OPTIONS: i32 = 3;
const RESTART: i32 = 4;
const EXIT: i32 = 5;
const RETURN: i32 = 6;

#[test]
fn pause_save_then_load_restores_the_mission_state() {
    let Some((mut run, dir)) = start("save") else {
        return;
    };
    open_pause_menu(&mut run);
    let saved = state(&run);
    click_pause_button(&mut run, SAVE);
    let file = dir.join(MISSION_SAVE_FILE);
    assert!(file.is_file(), "SAVE wrote {}", file.display());

    // play on: the clock and the movers move away from the save
    click_pause_button(&mut run, RETURN);
    assert!(!hud(&run).paused());
    frames(&mut run, 120);
    let later = state(&run);
    assert_ne!(later.0, saved.0, "the clock ran on");

    open_pause_menu(&mut run);
    click_pause_button(&mut run, LOAD);
    let restored = state(&run);
    assert_eq!(restored, saved, "LOAD brought the saved state back");
    assert!(run.core.mission.as_ref().unwrap().paused);

    // the mission goes on from the loaded state
    click_pause_button(&mut run, RETURN);
    frames(&mut run, 60);
    assert!(state(&run).0 != saved.0);
    assert_eq!(run.core.state(), &GameState::InMission);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_load_without_a_save_reports_it_and_keeps_playing() {
    let Some((mut run, dir)) = start("noload") else {
        return;
    };
    open_pause_menu(&mut run);
    let before = state(&run);
    click_pause_button(&mut run, LOAD);
    assert_eq!(state(&run), before);
    assert!(
        run.core.ui_host_mut().unwrap().banner().is_some(),
        "the missing save is reported to the player (notices {:?})",
        run.core.ui_host_mut().unwrap().notices().to_vec()
    );
    assert_eq!(run.core.state(), &GameState::InMission);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_load_of_a_corrupt_save_reports_it_and_keeps_the_mission() {
    let Some((mut run, dir)) = start("corrupt") else {
        return;
    };
    open_pause_menu(&mut run);
    click_pause_button(&mut run, SAVE);
    let file = dir.join(MISSION_SAVE_FILE);
    let mut bytes = std::fs::read(&file).expect("the save");
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&file, &bytes).expect("corrupt the save");
    click_pause_button(&mut run, RETURN);
    frames(&mut run, 90);
    open_pause_menu(&mut run);
    let before = state(&run);
    click_pause_button(&mut run, LOAD);
    assert_eq!(state(&run), before, "a corrupt save changes nothing");
    assert!(run.core.ui_host_mut().unwrap().banner().is_some());
    assert_eq!(run.core.state(), &GameState::InMission);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_restart_asks_then_relaunches_the_same_mission() {
    let Some((mut run, dir)) = start("abort") else {
        return;
    };
    open_pause_menu(&mut run);
    click_pause_button(&mut run, RESTART);
    frames(&mut run, 3);
    assert_eq!(run.core.state(), &GameState::InMission, "it asks first");
    let no = hud(&run).pause_dialog_point(0).expect("the prompt is open");
    click(&mut run, no);
    frames(&mut run, 3);
    assert!(
        hud(&run).pause_dialog_point(0).is_none(),
        "No closes the prompt"
    );
    assert_eq!(run.core.state(), &GameState::InMission);

    click_pause_button(&mut run, RESTART);
    frames(&mut run, 3);
    let yes = hud(&run).pause_dialog_point(1).expect("the prompt is open");
    click(&mut run, yes);
    frames(&mut run, 3);
    assert_eq!(
        run.core.state(),
        &GameState::InMission,
        "the mission relaunched"
    );
    let restarted = run.core.mission.as_ref().expect("restarted mission");
    assert_eq!(restarted.name, "mc2_01");
    assert!(restarted.clock.frame() < 30, "the mission started over");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_exit_asks_then_ends_the_game() {
    let Some((mut run, dir)) = start("exit") else {
        return;
    };
    open_pause_menu(&mut run);
    click_pause_button(&mut run, EXIT);
    frames(&mut run, 3);
    let no = hud(&run).pause_dialog_point(0).expect("the prompt is open");
    click(&mut run, no);
    frames(&mut run, 3);
    assert_eq!(run.core.state(), &GameState::InMission, "No keeps playing");

    click_pause_button(&mut run, EXIT);
    frames(&mut run, 3);
    let yes = hud(&run).pause_dialog_point(1).expect("the prompt is open");
    click(&mut run, yes);
    frames(&mut run, 3);
    assert_eq!(run.core.state(), &GameState::Exit);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pause_options_opens_the_options_screen_over_the_stopped_mission() {
    let Some((mut run, dir)) = start("options") else {
        return;
    };
    open_pause_menu(&mut run);
    let before = state(&run);
    click_pause_button(&mut run, OPTIONS);
    frames(&mut run, 5);
    assert!(
        !hud(&run).paused(),
        "OPTIONS resumes through togglePause, as RETURN does"
    );
    let host = run.core.ui_host_mut().unwrap();
    assert!(
        matches!(host.screen(), mc2_app::ui_host::ActiveScreen::Options),
        "the options screen is up: {:?}",
        host.screen()
    );
    let during = state(&run);
    frames(&mut run, 90);
    assert_eq!(state(&run), during, "the mission does not run under it");
    assert_eq!(during, before);
    let _ = std::fs::remove_dir_all(dir);
}

/// Async Mouse in a mission: the HUD's shipped cursor follows a pointer that moved between the update and the
/// render only with the option on.
#[test]
fn async_mouse_moves_the_mission_cursor_between_update_and_render() {
    let Some((mut run, dir)) = start("async") else {
        return;
    };
    run.core.handle_event(InputEvent::CursorEntered);
    run.core
        .handle_event(InputEvent::CursorMoved { x: 300.0, y: 300.0 });
    frames(&mut run, 3);
    let render = |run: &mut HeadlessRun<LiveSim>| {
        let gpu = run.core.scene.gpu_arc().clone();
        let target = run.target.target();
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("t") });
        run.core.render(&mut enc, &target).unwrap();
        let rb = target.encode_readback(&gpu, &mut enc).unwrap();
        gpu.queue.submit(Some(enc.finish()));
        rb.finish(&gpu).unwrap()
    };
    run.core.frame(std::time::Duration::from_millis(16));
    run.core.handle_event(InputEvent::CursorMoved {
        x: 1500.0,
        y: 900.0,
    });
    run.core.prefs.async_mouse = false;
    let off = render(&mut run);
    run.core.prefs.async_mouse = true;
    let on = render(&mut run);
    let w = run.target.size().0;
    let (mut old_spot, mut new_spot) = (0, 0);
    for (i, (a, b)) in on.chunks_exact(4).zip(off.chunks_exact(4)).enumerate() {
        if a != b {
            let (x, y) = (i as u32 % w, i as u32 / w);
            if x < 400 && y < 400 {
                old_spot += 1;
            } else if x > 1400 && y > 800 {
                new_spot += 1;
            }
        }
    }
    assert!(old_spot > 0, "off: drawn at the update's pointer");
    assert!(new_spot > 0, "on: drawn at the pointer that moved since");
    let _ = std::fs::remove_dir_all(dir);
}

/// Accepting a changed Hardware Rasterizer from the pause menu takes effect at once: the windowed loop's
/// decision (`AdapterState::apply`, with no mission-state exclusion) relaunches with the other adapter while
/// the mission is still the running state, and a failed relaunch is reported, not claimed.
#[test]
fn pause_options_hardware_rasterizer_relaunches_immediately_mid_mission() {
    use mc2_app::windowed::{AdapterOutcome, AdapterState};
    use mc2_ui::screens::{dialog, options};
    let Some((mut run, dir)) = start("rasterizer") else {
        return;
    };
    run.core
        .ui_host_mut()
        .unwrap()
        .set_config_directory(dir.join("config"));
    open_pause_menu(&mut run);
    click_pause_button(&mut run, OPTIONS);
    frames(&mut run, 5);
    let host = run.core.ui_host_mut().unwrap();
    let at = host
        .options_tab_control(options::MSG_HARDWARE_RASTERIZER)
        .expect("the Hardware Rasterizer control");
    // the options screen's points are 800 x 600 UI units, fitted to the 1920 x 1080 frame
    let layout =
        mc2_ui::render::UiLayout::new((800, 600), (1920, 1080), mc2_ui::render::ScaleMode::Fit);
    click(&mut run, layout.to_target(at.0, at.1));
    frames(&mut run, 2);
    let host = run.core.ui_host_mut().unwrap();
    let yes = host.options_button(dialog::YES).expect("accept button");
    click(&mut run, layout.to_target(yes.0, yes.1));
    frames(&mut run, 3);
    assert!(run.core.settings_applied());
    assert_ne!(
        run.core.prefs.renderer, 0,
        "the software adapter was chosen"
    );
    assert_eq!(
        run.core.state(),
        &GameState::InMission,
        "accepted mid-mission"
    );
    // hardware is running: the accepted choice relaunches now
    let mut relaunches = 0;
    let mut started = None;
    let (mut adapter, _) = AdapterState::new(false, false);
    // the relaunch callback IS the relaunched process's start-up: it reads the options.cfg the UiHost just wrote
    // through the production path (`apply_saved_adapter`) and builds its device from that, never from the
    // in-memory preference. It reports success only if that device exists.
    let outcome = adapter.apply(run.core.prefs.renderer, || {
        relaunches += 1;
        let config = dir.join("config");
        if !config.join("options.cfg").is_file() {
            return Err("UiHost did not write options.cfg".to_string());
        }
        let mut cfg = mc2_gfx::config::RenderConfig::default();
        mc2_app::windowed::apply_saved_adapter(&mut cfg, Some(&config), &dir);
        if !cfg.software_adapter {
            return Err("the saved choice did not reach start-up".to_string());
        }
        let gpu = mc2_render::gpu::Gpu::headless(&cfg).map_err(|e| format!("{e:#}"))?;
        started = Some((cfg, gpu));
        Ok(())
    });
    assert_eq!(outcome, AdapterOutcome::Relaunched);
    assert_eq!(relaunches, 1);
    let (cfg, gpu) = started.expect("the relaunch callback created the device");
    let actual = gpu.is_software_adapter();
    let (mut next, notice) = AdapterState::new(cfg.software_adapter, actual);
    assert_eq!(next.software, actual);
    // the start-up line names the adapter the device really is on
    let line = gpu.startup_line();
    assert!(
        line.starts_with("mc2: gpu adapter: ") && line.ends_with(&format!("software={actual})")),
        "{line}"
    );
    if mc2_render::gpu::Gpu::software_adapter_available() {
        // a software adapter exists: the saved choice must really create exactly that one, never the GPU
        assert!(actual, "the saved choice created the software adapter");
        assert_eq!(
            gpu.adapter.get_info().device_type,
            wgpu::DeviceType::Cpu,
            "{line}"
        );
        assert!(line.contains("Cpu"), "{line}");
        assert!(
            notice.is_none(),
            "the software adapter was created as asked"
        );
    } else {
        println!("SKIP software-adapter branch: no software adapter on this host");
        // no software adapter on this machine: the choice is reported unavailable, and no relaunch loop starts
        assert!(!actual);
        assert!(notice.is_some_and(|n| n.contains("no software adapter")));
    }
    let outcome = next.apply(run.core.prefs.renderer, || panic!("relaunched again"));
    assert_eq!(outcome, AdapterOutcome::Unchanged);
    // a relaunch that fails is reported to the player and not counted as applied
    let (mut adapter, _) = AdapterState::new(false, false);
    let outcome = adapter.apply(run.core.prefs.renderer, || Err("denied".into()));
    assert!(matches!(outcome, AdapterOutcome::Failed(m) if m.contains("denied")));
    let _ = std::fs::remove_dir_all(dir);
}

/// A support vehicle in the save is drawn after LOAD: the load re-binds the restored movers to their
/// models at once, so the first frame after it already has the vehicle's instance and pick box.
#[test]
fn pause_load_redraws_a_saved_support_vehicle() {
    use mc2_sim::command::PlayerCommand;
    let Some((mut run, dir)) = start("support") else {
        return;
    };
    let (before, at) = {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let h = w
            .objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.team_id() == w.teams.home as i32
                        && matches!(o.kind, mc2_sim::object::Kind::Mech(_))
                        && !o.c.is_disabled()
                })
            })
            .expect("a home mech");
        let p = w.objects.get(h).unwrap().c.position;
        let at = mc2_sim::math::Vec3 { x: p.x + 40.0, ..p };
        let z = w.terrain.elevation(&at);
        (w.objects.movers.len(), mc2_sim::math::Vec3 { z, ..at })
    };
    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .inject_commands(vec![PlayerCommand::BeginVtol {
            commander: 0,
            vehicle_id: 182,
            at,
            target: None,
        }]);
    for _ in 0..600 {
        frames(&mut run, 1);
        if run
            .core
            .mission
            .as_ref()
            .unwrap()
            .driver
            .sim()
            .world
            .objects
            .movers
            .len()
            > before
        {
            break;
        }
    }
    frames(&mut run, 5);
    open_pause_menu(&mut run);
    click_pause_button(&mut run, SAVE);
    click_pause_button(&mut run, RETURN);
    frames(&mut run, 120);
    open_pause_menu(&mut run);
    click_pause_button(&mut run, LOAD);
    let rt = run.core.mission.as_ref().unwrap();
    let w = &rt.driver.sim().world;
    assert!(
        w.objects.movers.len() > before,
        "the save holds the vehicle"
    );
    for &h in &w.objects.movers {
        assert!(
            rt.driver.mover_instance(h).is_some(),
            "mover {h:?} is drawn after LOAD"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}
