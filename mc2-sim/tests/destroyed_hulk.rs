// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Death events through the mission's production frame loop.
mod common;

use mc2_sim::combat::damage::handle_destruction;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::Kind;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim, SimEvent};
use std::rc::Rc;

#[test]
fn incapacitated_walking_mechs_continue_to_move() {
    let Some(root) = common::data_root() else {
        return;
    };
    for destroyed in [false, true] {
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
        let (mode, seed) = common::rng_by_name("pcg");
        let outcome =
            common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode)).unwrap();
        let mut sim = Sim::new(outcome);
        let vis = Rc::new(AllVisible);
        sim.step(FrameInput::fixed(30, 1, vis.clone(), vec![]))
            .unwrap();
        let mech = *sim
            .world
            .objects
            .movers
            .iter()
            .find(|&&h| matches!(sim.world.objects.get(h).unwrap().kind, Kind::Mech(_)))
            .unwrap();
        if destroyed {
            handle_destruction(&mut sim.world, mech, None);
        } else {
            mc2_sim::combat::damage::disable(&mut sim.world, mech, 0);
        }
        let (appearance, _) = sim.world.appearance.mech_mut(mech).unwrap();
        appearance.current_gesture = mc2_sim::appearance::tables::gesture::WALK;
        let speed = sim.world.appearance.velocity_magnitude(mech);
        assert!(speed > 0.0, "fixture must be walking");
        let before = sim.world.objects.get(mech).unwrap().c.position;
        let rotation = sim.world.objects.get(mech).unwrap().c.rotation;
        let direction = mc2_sim::math::rotation_vector(rotation);
        let distance = speed * mc2_sim::terrain::WORLD_UNITS_PER_METER / 30.0;
        sim.step(FrameInput::fixed(30, 2, vis, vec![])).unwrap();
        let after = sim.world.objects.get(mech).unwrap().c.position;
        // C++ mech.cpp:6104-6117 and 6170-6183; one world unit is the owner tolerance.
        assert!((after.x - before.x - direction.x * distance).abs() < 1.0);
        assert!((after.y - before.y - direction.y * distance).abs() < 1.0);
    }
}

#[test]
fn disabled_mech_and_destroyed_vehicle_release_path_locks() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome =
        common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode)).unwrap();
    let mut sim = Sim::new(outcome);
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis.clone(), vec![]))
        .unwrap();
    let movers = sim.world.objects.movers.clone();
    let mech = *movers
        .iter()
        .find(|&&h| matches!(sim.world.objects.get(h).unwrap().kind, Kind::Mech(_)))
        .unwrap();
    let vehicle = *movers
        .iter()
        .find(|&&h| matches!(sim.world.objects.get(h).unwrap().kind, Kind::Vehicle(_)))
        .unwrap();
    for h in [mech, vehicle] {
        match &mut sim.world.objects.get_mut(h).unwrap().kind {
            Kind::Mech(m) => m.m.path_locks_enabled = true,
            Kind::Vehicle(v) => v.m.path_locks_enabled = true,
            _ => unreachable!(),
        }
        mc2_sim::mover::mech::update_path_lock(&mut sim.world, h, true);
    }
    let cells: Vec<_> = [mech, vehicle]
        .into_iter()
        .map(|h| {
            let o = sim.world.objects.get(h).unwrap();
            let level = match &o.kind {
                Kind::Mech(m) => m.m.move_level,
                Kind::Vehicle(v) => v.m.move_level,
                _ => unreachable!(),
            };
            (
                u32::from(level == 2),
                i32::from(o.c.cell.0),
                i32::from(o.c.cell.1),
            )
        })
        .collect();
    for &(level, row, col) in &cells {
        assert!(sim.world.terrain.get_pathlock(level, row, col));
    }
    mc2_sim::combat::damage::disable(&mut sim.world, mech, 0);
    handle_destruction(&mut sim.world, vehicle, None);
    sim.step(FrameInput::fixed(30, 2, vis, vec![])).unwrap();
    for &(level, row, col) in &cells {
        assert!(!sim.world.terrain.get_pathlock(level, row, col));
    }
}

#[test]
fn destroyed_movers_explode_and_leave_craters() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome =
        common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode)).unwrap();
    let mut sim = Sim::new(outcome);
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis.clone(), vec![]))
        .unwrap();
    let mech = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| matches!(sim.world.objects.get(h).unwrap().kind, Kind::Mech(_)))
        .unwrap();
    let vehicle = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            let o = sim.world.objects.get(h).unwrap();
            matches!(o.kind, Kind::Vehicle(_)) && o.c.type_handle == 201
        })
        .unwrap();
    handle_destruction(&mut sim.world, mech, None);
    handle_destruction(&mut sim.world, vehicle, None);
    let saved = mc2_sim::save::save_world(&sim.world);
    let base = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode)).unwrap();
    let loaded = mc2_sim::save::load_world(base.world, Some(&mut vfs), &saved).unwrap();
    let mut resumed = Sim::new(mc2_sim::mission::init::LoadOutcome {
        complete: loaded.load_is_complete(),
        missing_hooks: loaded.unported_load_hooks.clone(),
        world: loaded,
    });
    let mut explosions = Vec::new();
    let mut craters = 0;
    let mut fallen_at = None;
    for frame in 2..250 {
        let out = sim
            .step(FrameInput::fixed(30, frame, vis.clone(), vec![]))
            .unwrap();
        let resumed_out = resumed
            .step(FrameInput::fixed(30, frame, vis.clone(), vec![]))
            .unwrap();
        let death_events = |events: &[SimEvent]| {
            events
                .iter()
                .filter(|event| {
                    matches!(
                        event,
                        SimEvent::Explosion {
                            effect_id: 39 | 40 | 46,
                            ..
                        } | SimEvent::Crater { kind: 67, .. }
                    )
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(
            death_events(&out.events),
            death_events(&resumed_out.events),
            "save continuation frame {frame}"
        );
        if let Kind::Mech(m) = &sim.world.objects.get(mech).unwrap().kind {
            if m.fallen && fallen_at.is_none() {
                fallen_at = Some(frame);
            }
        }
        for event in out.events {
            match event {
                SimEvent::Explosion { effect_id, .. } => explosions.push((frame, effect_id)),
                SimEvent::Crater { kind: 67, .. } => craters += 1,
                _ => {}
            }
        }
    }
    assert!(explosions.iter().any(|&(_, id)| id == 39), "{explosions:?}");
    let mech_frame = explosions.iter().find(|&&(_, id)| id == 39).unwrap().0;
    // C++ decrements a one-second timer as soon as the fall completes and explodes below 0.4 s.
    // At 30 fps allow two frames for the strict threshold and animation update order.
    assert!((16..=20).contains(&(mech_frame - fallen_at.unwrap())));
    assert!(
        explosions.iter().any(|&(_, id)| id == 40 || id == 46),
        "{explosions:?}"
    );
    assert!(explosions
        .iter()
        .any(|&(frame, id)| frame == 2 && (id == 40 || id == 46)));
    assert!(craters >= 2, "{craters}");
    let Kind::Mech(m) = &sim.world.objects.get(mech).unwrap().kind else {
        panic!()
    };
    assert!(m.mech_removed);
    assert!(sim.world.objects.get(vehicle).unwrap().exists());
}

/// `BattleMech::update` hands the mech's status to its appearance only while it is not destroyed (mech.cpp:6282-6283) and, once
/// destroyed, from the frame after `mechRemoved` (mech.cpp:6229-6233). That hand-over is what the fall roll reads
/// (mech3d.cpp:3952-3967); D-52 gives the appearance DESTROYED at the kill for the damage shape only.
#[test]
fn a_destroyed_mech_hands_its_status_to_the_appearance_the_frame_after_removal() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome =
        common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode)).unwrap();
    let mut sim = Sim::new(outcome);
    let vis = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, vis.clone(), vec![]))
        .unwrap();
    let mech = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| matches!(sim.world.objects.get(h).unwrap().kind, Kind::Mech(_)))
        .unwrap();
    let statuses = |sim: &mut Sim| {
        let (a, _) = sim.world.appearance.mech_mut(mech).unwrap();
        (a.status, a.cpp_status)
    };
    assert_eq!(statuses(&mut sim), (0, 0), "a live mech is handed NORMAL");
    handle_destruction(&mut sim.world, mech, None);
    let mut removed_at = None;
    for frame in 2..400 {
        sim.step(FrameInput::fixed(30, frame, vis.clone(), vec![]))
            .unwrap();
        let removed =
            matches!(&sim.world.objects.get(mech).unwrap().kind, Kind::Mech(m) if m.mech_removed);
        if removed && removed_at.is_none() {
            removed_at = Some(frame);
        }
        let want = match removed_at {
            Some(r) if frame > r => 2,
            _ => 0,
        };
        assert_eq!(
            statuses(&mut sim),
            (2, want),
            "frame {frame} (removed at {removed_at:?})"
        );
    }
    assert!(
        removed_at.is_some(),
        "the wreck is removed within 400 frames"
    );
}
