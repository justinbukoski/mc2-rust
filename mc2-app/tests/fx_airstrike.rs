// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! Airstrike presentation (artlry.cpp:938-962): the bomber flyover about 2.5 s before impact and its jet contrail reach the app's
//! effect sink and are drawn, for the ABL-called strike (`callstrikeex`) and the player-called strike (`PlayerCommand::CallStrike`,
//! what the HUD strike buttons issue). Both run the one production `Artillery::update` path.
//!
//! The frame written mid-flyover goes under `CARGO_TARGET_TMPDIR`.

use mc2_app::fx_sink::{FxHub, FxRole};
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::layers::{OrderContext, OrderHook};
use mc2_app::live_sim::LiveSim;
use mc2_app::sim::{FrameContext, SimEventSink};
use mc2_fx::system::FxSystem;
use mc2_sim::{PlayerCommand, SimEvent};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Flyover,
    Contrail,
    Impact,
}

fn classify(e: &SimEvent) -> Option<Step> {
    match e {
        SimEvent::AirstrikeFlyover { .. } => Some(Step::Flyover),
        SimEvent::AirstrikeContrail { .. } => Some(Step::Contrail),
        SimEvent::Explosion {
            effect_id: 50 | 51, ..
        } => Some(Step::Impact),
        _ => None,
    }
}

#[test]
fn an_abl_strike_flyover_contrail_sink_and_impact_in_order() {
    use mc2_abl::{Config, DefaultHost, Env, MemorySource};
    use mc2_sim::mission::init::{load_world, LoadSpec};
    use mc2_sim::rng::RngMode;
    use mc2_sim::sim::FrameInput;
    use mc2_sim::visibility::AllVisible;
    let Some(root) = data() else { return };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let mut outcome =
        load_world(&mut vfs, &LoadSpec::quickstart("mc2_01", 7, RngMode::Pcg32)).expect("load");
    let mut src = MemorySource::new();
    src.insert(
        "data/missions/strike.abl",
        "module strike : integer;\nvar\nstatic integer done;\ncode\nif (done == 0) then done = 1; callstrikeex(1, 0, 1.0, 2.0, 3.0, false, 5.0); endif;\nreturn(0);\nendmodule.\n",
    );
    let mut env = Env::new(Config::default(), src, Box::new(DefaultHost));
    let handle = env
        .pre_process("data/missions/strike.abl")
        .expect("compile");
    outcome.world.mission.brain = Some(env.new_instance(handle).expect("instance"));
    for pilot in &mut outcome.world.pilots.slots {
        pilot.brain = None;
    }
    outcome.world.abl = Some(env);
    let mut sim = mc2_sim::Sim::new(outcome);
    let fx = FxSystem::from_vfs(&mut vfs, 7).expect("effect library");
    let hub = FxHub::with_system(fx, HashMap::new(), true);

    // (frame, step) of the sim events; the frame the hub first holds the contrail effect
    let mut seen: Vec<(u64, Step)> = Vec::new();
    let mut sink_frame = None;
    for frame in 1..200u64 {
        let out = sim
            .step(FrameInput::fixed(
                30,
                frame,
                Rc::new(AllVisible),
                Vec::new(),
            ))
            .expect("step");
        hub.handle_events(Some(&sim.world), &out.events);
        hub.advance_to(frame as f64 / 30.0);
        for e in &out.events {
            if let Some(s) = classify(e) {
                seen.push((frame, s));
            }
        }
        if sink_frame.is_none()
            && hub
                .spawned()
                .iter()
                .any(|s| s.role == FxRole::Contrail && (s.at.x - 1.0).abs() < 1e-3)
        {
            sink_frame = Some(frame);
        }
    }
    let steps: Vec<Step> = seen.iter().map(|&(_, s)| s).collect();
    assert_eq!(
        steps,
        [Step::Flyover, Step::Contrail, Step::Impact],
        "{seen:?}"
    );
    let (fly, contrail, impact) = (seen[0].0, seen[1].0, seen[2].0);
    let sink = sink_frame.expect("the app sink started the contrail effect");
    assert!(contrail <= sink && sink < impact, "{seen:?} sink {sink}");
    // the flyover starts 2.5 s (75 frames) before impact, within a few frames
    assert!(
        (impact - fly).abs_diff(75) <= 3,
        "flyover at {fly}, impact at {impact}"
    );
    assert_eq!(fly, contrail);
}

#[derive(Clone, Default)]
struct Seen(Rc<RefCell<Vec<(u64, SimEvent)>>>);

impl SimEventSink<LiveSim> for Seen {
    fn on_frame(&mut self, ctx: &FrameContext<'_, LiveSim>, events: &[SimEvent]) {
        let frame = ctx.world.clock.turn as u64;
        self.0.borrow_mut().extend(
            events
                .iter()
                .filter(|e| classify(e).is_some())
                .map(|e| (frame, e.clone())),
        );
    }
}

struct Queue(Rc<RefCell<Vec<PlayerCommand>>>);

impl OrderHook<PlayerCommand> for Queue {
    fn emits_sim_commands(&self) -> bool {
        true
    }
    fn update(&mut self, _ctx: &OrderContext<'_>, out: &mut Vec<PlayerCommand>) {
        out.append(&mut self.0.borrow_mut());
    }
}

fn render_rgba(run: &mut HeadlessRun<LiveSim>) -> Vec<u8> {
    let gpu = run.core.scene.gpu_arc().clone();
    let target = run.target.target();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("fx") });
    run.core.render(&mut enc, &target).unwrap();
    let rb = target.encode_readback(&gpu, &mut enc).unwrap();
    gpu.queue.submit(Some(enc.finish()));
    rb.finish(&gpu).unwrap()
}

#[test]
fn a_player_strike_reaches_the_same_path_and_draws_the_bomber_mid_flyover() {
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e)
            if e.contains("no suitable GPU adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1") =>
        {
            eprintln!("needs a GPU adapter ({e}); MC2_SKIP_GPU_TESTS=1, skipping");
            return;
        }
        Err(e) => panic!("setup failed: {e}"),
    };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    let hub = FxHub::new(&mut run.core.scene, &mut run.core.game.vfs, 7, true)
        .expect("effect library and textures");
    let rig = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .view
        .bomber
        .clone()
        .expect("the Shilone bomber loads from the shipped data");
    hub.set_bomber(Some(rig.clone()));
    let seen = Seen::default();
    let queue = Rc::new(RefCell::new(Vec::new()));
    run.core.sinks.push(Box::new(seen.clone()));
    run.core.sinks.push(Box::new(hub.sink()));
    run.core.effects = Some(Box::new(hub.scene()));
    run.core.orders = Box::new(Queue(queue.clone()));
    for _ in 0..5 {
        run.step(None).unwrap();
    }
    // the strike point: on the first mover, which the camera then looks at
    let at = {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let h = w.objects.movers[0];
        w.objects.get(h).unwrap().c.position
    };
    {
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(
            mc2_render::math::Vec3::new(at.x, at.y, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    queue
        .borrow_mut()
        .push(PlayerCommand::CallStrike { strike: 1, at });
    let mut waited = 0;
    while !seen
        .0
        .borrow()
        .iter()
        .any(|(_, e)| matches!(e, SimEvent::AirstrikeContrail { .. }))
    {
        run.step(None).unwrap();
        waited += 1;
        assert!(waited < 400, "no flyover within 400 frames of the strike");
    }
    // fly the bomb run until the bomber's contrail node is on screen (it starts about a thousand units out)
    let fly_turn = seen
        .0
        .borrow()
        .iter()
        .find_map(|(f, e)| matches!(e, SimEvent::AirstrikeFlyover { .. }).then_some(*f))
        .unwrap() as i64;
    let node_screen = |run: &HeadlessRun<LiveSim>| {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let node =
            rig.activity_offset(rig.frame_after(w.clock.turn - fly_turn, w.clock.frame_length));
        let cam = run
            .core
            .mission
            .as_ref()
            .unwrap()
            .render_camera((1920.0, 1080.0), run.core.config.interpolate_camera);
        let (p, _) = cam.project_z(mc2_render::math::Vec3::new(
            at.x + node.x,
            at.y + node.y,
            at.z + node.z,
        ));
        (p, node)
    };
    // two thirds into the 2.5 s run the bomber is over its approach; the camera looks at it (a player scrolling to the strike)
    for _ in 0..44 {
        run.step(None).unwrap();
    }
    {
        // the node a few frames ahead: the camera needs them to settle
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        let node =
            rig.activity_offset(rig.frame_after(w.clock.turn + 6 - fly_turn, w.clock.frame_length));
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(
            mc2_render::math::Vec3::new(at.x + node.x, at.y + node.y, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..6 {
        run.step(None).unwrap();
    }
    {
        // the bomber is high above its ground point, so centring the ground point leaves it up the screen: move the camera
        // by the pixel gap, using the screen directions of the two ground axes
        let (p, node) = node_screen(&run);
        let cam = run
            .core
            .mission
            .as_ref()
            .unwrap()
            .render_camera((1920.0, 1080.0), run.core.config.interpolate_camera);
        let g = (at.x + node.x, at.y + node.y);
        let pg = |x: f32, y: f32| cam.project_z(mc2_render::math::Vec3::new(x, y, at.z)).0;
        let o = pg(g.0, g.1);
        let (a, b) = (pg(g.0, g.1 + 50.0), pg(g.0 + 50.0, g.1));
        let (a, b) = ([a[0] - o[0], a[1] - o[1]], [b[0] - o[0], b[1] - o[1]]);
        let d = [p[0] - 960.0, p[1] - 540.0];
        let det = a[0] * b[1] - a[1] * b[0];
        let sy = (d[0] * b[1] - d[1] * b[0]) / det;
        let sx = (a[0] * d[1] - a[1] * d[0]) / det;
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(
            mc2_render::math::Vec3::new(g.0 + sx * 50.0, g.1 + sy * 50.0, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
    }
    for _ in 0..6 {
        run.step(None).unwrap();
    }
    let events = seen.0.borrow().clone();
    let steps: Vec<Step> = events.iter().filter_map(|(_, e)| classify(e)).collect();
    assert_eq!(&steps[..2], [Step::Flyover, Step::Contrail], "{steps:?}");
    assert!(
        !steps.contains(&Step::Impact),
        "still mid-flyover, not yet impact"
    );
    let bomber = events
        .iter()
        .find_map(|(_, e)| match e {
            SimEvent::AirstrikeFlyover { at, .. } => Some(*at),
            _ => None,
        })
        .unwrap();
    assert!((bomber.x - at.x).abs() < 1.0 && (bomber.y - at.y).abs() < 1.0);
    assert!(
        hub.spawned().iter().any(|s| s.role == FxRole::Contrail),
        "the sink started the contrail"
    );
    let (screen, node) = node_screen(&run);
    let start = rig.activity_offset(rig.frame_after(0, 1.0 / 30.0));
    let moved =
        ((node.x - start.x).powi(2) + (node.y - start.y).powi(2) + (node.z - start.z).powi(2))
            .sqrt();
    assert!(
        moved > 1.0,
        "the bomb-run animation moves the bomber ({moved})"
    );
    let with = render_rgba(&mut run);
    // the same frame and effects without the bomber mesh: what differs is the bomber
    let view_bomber = run.core.mission.as_mut().unwrap().view.bomber.take();
    let without = render_rgba(&mut run);
    run.core.mission.as_mut().unwrap().view.bomber = view_bomber;
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("airstrike_mid_flyover.png");
    write_png(&out, &with, 1920, 1080);
    let tol = 250.0;
    let (mut inside, mut total) = (0usize, 0usize);
    for y in 0..1080usize {
        for x in 0..1920usize {
            let i = (y * 1920 + x) * 4;
            if with[i..i + 4] != without[i..i + 4] {
                total += 1;
                if (x as f32 - screen[0]).abs() < tol && (y as f32 - screen[1]).abs() < tol {
                    inside += 1;
                }
            }
        }
    }
    eprintln!("{inside} bomber pixels near {screen:?} ({total} in the frame); frame {out:?}");
    assert!(inside > 100, "only {inside} bomber pixels near {screen:?}");

    // Through impact and departure: the sim strike object is gone a frame after the impact, but the bomber keeps flying and
    // its contrail keeps following until the bomb-run animation has finished (`artlry.cpp:875-923`), then both are cleaned up.
    let contrail = hub
        .spawned()
        .into_iter()
        .find(|s| s.role == FxRole::Contrail)
        .expect("contrail")
        .handle;
    let run_steps = |run: &HeadlessRun<LiveSim>| {
        let w = &run.core.mission.as_ref().unwrap().driver.sim().world;
        w.clock.turn - fly_turn
    };
    let impacted = |seen: &Seen| {
        seen.0
            .borrow()
            .iter()
            .any(|(_, e)| matches!(classify(e), Some(Step::Impact)))
    };
    let mut guard = 0;
    while !impacted(&seen) {
        run.step(None).unwrap();
        guard += 1;
        assert!(guard < 400, "the strike never impacted");
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    assert!(
        !rig.run_finished(run_steps(&run), 1.0 / 30.0),
        "the animation outlasts the impact"
    );
    assert!(hub.is_alive(contrail), "the contrail survives the impact");
    let (screen, _) = node_screen(&run);
    let with = render_rgba(&mut run);
    let view_bomber = run.core.mission.as_mut().unwrap().view.bomber.take();
    let without = render_rgba(&mut run);
    run.core.mission.as_mut().unwrap().view.bomber = view_bomber;
    let near = |a: &[u8], b: &[u8]| {
        (0..1080usize)
            .flat_map(|y| (0..1920usize).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = (y * 1920 + x) * 4;
                a[i..i + 4] != b[i..i + 4]
                    && (x as f32 - screen[0]).abs() < 400.0
                    && (y as f32 - screen[1]).abs() < 400.0
            })
            .count()
    };
    let after_impact = near(&with, &without);
    eprintln!("{after_impact} bomber pixels after impact");
    assert!(after_impact > 0, "the bomber is still drawn after impact");
    let mut guard = 0;
    while !rig.run_finished(run_steps(&run), 1.0 / 30.0) {
        run.step(None).unwrap();
        guard += 1;
        assert!(guard < 400, "the bomb run never finished");
    }
    run.step(None).unwrap();
    assert!(!hub.is_alive(contrail), "the contrail is cleaned up");
    let with = render_rgba(&mut run);
    let view_bomber = run.core.mission.as_mut().unwrap().view.bomber.take();
    let without = render_rgba(&mut run);
    run.core.mission.as_mut().unwrap().view.bomber = view_bomber;
    assert_eq!(
        near(&with, &without),
        0,
        "the bomber is gone after departure"
    );
}

fn write_png(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) {
    let file = std::fs::File::create(path).expect("png file");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wr| wr.write_image_data(rgba))
        .expect("write png");
}
