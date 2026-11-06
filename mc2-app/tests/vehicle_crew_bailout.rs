// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A destroyed ground vehicle's bailed-out crew on the live render path (headless, never a window; skipped without `MC2_DATA` or,
//! with `MC2_SKIP_GPU_TESTS=1`, without a GPU adapter): `createVehiclePilot` (gvehicl.cpp:2710-2807) spawns object 638 as a mover,
//! the app binds a model to it painted `0x00ffffff/0x00ffffff/0x00ff8000`, it is drawn and its animation advances.

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::live_sim::LiveSim;
use mc2_app::sim::SimDriver;
use mc2_render::scene::ShapeInstance;
use mc2_sim::combat::damage::handle_destruction;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::object::Kind;
use std::path::PathBuf;

fn start() -> Option<HeadlessRun<LiveSim>> {
    let Some(data) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA not set; skipping");
        return None;
    };
    let mut opts = HeadlessOptions::new(data, "mc2_03");
    opts.size = (800, 600);
    opts.game.cursor = false;
    match HeadlessRun::live_bare(&opts) {
        Ok(r) => Some(r),
        Err(e)
            if e.contains("no suitable GPU adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1") =>
        {
            eprintln!("no GPU adapter; skipping: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn crew(run: &HeadlessRun<LiveSim>) -> Option<ObjectHandle> {
    let w = &run.core.mission.as_ref()?.driver.sim().world;
    (0..w.objects.max_vehicles)
        .filter_map(|i| w.objects.vehicles.get(i).copied())
        .find(|&h| {
            w.objects
                .get(h)
                .is_some_and(|o| matches!(&o.kind, Kind::Vehicle(v) if v.is_vehicle_pilot))
        })
}

fn drawn(run: &mut HeadlessRun<LiveSim>, h: ObjectHandle) -> Option<ShapeInstance> {
    let core = &mut run.core;
    let rt = core.mission.as_mut().unwrap();
    let mut out: Vec<ShapeInstance> = Vec::new();
    rt.driver
        .append_instances(&mut core.scene, &rt.view, 0.0, &mut out);
    let at = rt.driver.sim().world.objects.get(h)?.c.position;
    out.into_iter()
        .filter(|i| (i.position.x - at.x).hypot(i.position.y - at.y) < 1.0)
        .min_by(|a, b| a.pose.len().cmp(&b.pose.len()))
}

#[test]
fn bailed_out_crew_is_bound_painted_drawn_and_animated() {
    let Some(mut run) = start() else { return };
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    for _ in 0..10 {
        run.step(None).unwrap();
    }
    // A land vehicle, moved to the player's first mech so the camera sees the wreck.
    let victim = {
        let sim = run.core.mission.as_mut().unwrap().driver.sim_mut();
        let w = &mut sim.world;
        let near = w
            .objects
            .movers
            .iter()
            .filter_map(|&h| w.objects.get(h))
            .find(|o| matches!(o.kind, Kind::Mech(_)) && o.team_id() == w.teams.home as i32)
            .map(|o| o.c.position)
            .expect("a home mech");
        let victim = *w
            .objects
            .movers
            .iter()
            .find(|&&h| {
                let o = w.objects.get(h).unwrap();
                matches!(&o.kind, Kind::Vehicle(v) if v.m.path_locks_enabled)
            })
            .expect("a vehicle");
        let o = w.objects.get_mut(victim).unwrap();
        o.c.position.x = near.x + 40.0;
        o.c.position.y = near.y;
        handle_destruction(w, victim, None);
        victim
    };
    let _ = victim;
    let mut found = None;
    for _ in 0..400 {
        run.step(None).unwrap();
        if let Some(h) = crew(&run) {
            found = Some(h);
            break;
        }
    }
    let h = found.expect("the destroyed vehicle bailed out its crew");
    // bound to a model of type 638, with the crew's paint (the template painted at load)
    // the binding happens before the next step's visibility pass
    run.step(None).unwrap();
    {
        let rt = run.core.mission.as_ref().unwrap();
        let o = rt.driver.sim().world.objects.get(h).unwrap();
        assert!(o.exists(), "the crew stays while the camera sees it");
        assert!(rt.driver.mover_instance(h).is_some(), "a model is bound");
    }
    let first = drawn(&mut run, h).expect("the crew is drawn (bound, in the mover list, existing)");
    let rt = run.core.mission.as_ref().unwrap();
    assert!(rt.driver.load_unbound_movers().iter().all(|&u| u != h));
    assert_eq!(
        rt.driver.sim().world.objects.get(h).unwrap().c.type_handle,
        638
    );
    assert!(rt.driver.mover_instance(h).is_some());
    // it keeps moving: the pose changes between frames while it is drawn
    let mut changed = false;
    for _ in 0..30 {
        run.step(None).unwrap();
        if let Some(i) = drawn(&mut run, h) {
            if i.pose != first.pose {
                changed = true;
                break;
            }
        }
    }
    assert!(changed, "the crew's animation advances");
    // and the pixels of its projected box are not flat scenery
    let rt = run.core.mission.as_ref().unwrap();
    let b = rt
        .driver
        .mover_screen_box(h)
        .expect("the crew has a screen box");
    let gpu = run.core.scene.gpu_arc().clone();
    let target = run.target.target();
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("crew"),
        });
    run.core.render(&mut enc, &target).unwrap();
    let rb = target.encode_readback(&gpu, &mut enc).unwrap();
    gpu.queue.submit(Some(enc.finish()));
    let rgba = rb.finish(&gpu).unwrap();
    let (w, h_px) = (800usize, 600usize);
    let mut colours = std::collections::BTreeSet::new();
    for y in (b[1].max(0.0) as usize)..(b[3].max(0.0) as usize).min(h_px) {
        for x in (b[0].max(0.0) as usize)..(b[2].max(0.0) as usize).min(w) {
            let i = (y * w + x) * 4;
            colours.insert([rgba[i], rgba[i + 1], rgba[i + 2]]);
        }
    }
    assert!(colours.len() > 8, "the crew's box shows a model: {b:?}");
}
