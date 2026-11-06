// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A destroyed mech falls through the mission's production frame loop (no GPU).
//!
//! `BattleMech::update` hands the appearance DESTROYED (`cpp_status`) from the frame after `mechRemoved`
//! (mech.cpp:6229-6233, 6282-6283); the appearance then rolls its fall (mech3d.cpp:3952-3967), reaches
//! FALLEN_FORWARD/FALLEN_BACKWARD and latches the matching damage shape (DIFFERENCES.md D-52).
mod common;

use mc2_sim::appearance::mech::ShapeKind;
use mc2_sim::appearance::tables::gesture;
use mc2_sim::combat::damage::handle_destruction;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::Kind;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim};
use std::rc::Rc;

const DESTROYED: i32 = mc2_sim::object::ObjectStatus::Destroyed as i32;

#[test]
fn a_destroyed_mech_falls_and_holds_the_wreck_in_a_mission() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mission = "mc2_02";
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome =
        common::load_blocker(&mut vfs, &LoadSpec::quickstart(mission, seed, mode)).unwrap();
    let mut sim = Sim::new(outcome);
    let vis = Rc::new(AllVisible);
    let mut turn = 1;
    sim.step(FrameInput::fixed(30, turn, vis.clone(), vec![]))
        .unwrap();
    let mech = *sim
        .world
        .objects
        .movers
        .iter()
        .find(|&&h| matches!(sim.world.objects.get(h).unwrap().kind, Kind::Mech(_)))
        .unwrap();
    let before = sim.world.appearance.mech(mech).unwrap();
    assert_ne!(
        before.cpp_status, DESTROYED,
        "the fixture mech starts alive"
    );

    handle_destruction(&mut sim.world, mech, None);
    let death_frame = turn;
    let (cpp, status) = {
        let a = sim.world.appearance.mech(mech).unwrap();
        (a.cpp_status, a.status)
    };
    eprintln!(
        "mech_fall: mission={mission} seed={seed} mech={mech:?} death_frame={death_frame} \
         cpp_status={cpp} status={status} at death"
    );

    let mut handed_over = None;
    let mut fallen_gesture = None;
    let mut latched = None;
    for _ in 0..900 {
        turn += 1;
        sim.step(FrameInput::fixed(30, turn, vis.clone(), vec![]))
            .unwrap();
        let a = sim.world.appearance.mech(mech).unwrap();
        if handed_over.is_none() && a.cpp_status == DESTROYED {
            handed_over = Some(turn);
            eprintln!(
                "mech_fall: cpp_status DESTROYED handed over at frame {turn} (status={}, gesture={})",
                a.status, a.current_gesture
            );
        }
        if fallen_gesture.is_none()
            && matches!(
                a.current_gesture,
                gesture::FALLEN_FORWARD | gesture::FALLEN_BACKWARD
            )
        {
            fallen_gesture = Some((turn, a.current_gesture));
        }
        if latched.is_none()
            && matches!(
                a.shape_kind,
                ShapeKind::ForwardDamage | ShapeKind::BackwardDamage
            )
        {
            latched = Some((turn, a.shape_kind));
            eprintln!(
                "mech_fall: fallen shape {:?} latched at frame {turn} (cpp_status={}, status={}, gesture={})",
                a.shape_kind, a.cpp_status, a.status, a.current_gesture
            );
        }
        if handed_over.is_some() && latched.is_some() {
            break;
        }
    }
    let handed_over =
        handed_over.expect("BattleMech::update hands the appearance cpp_status DESTROYED");
    let (gesture_frame, g) = fallen_gesture.expect("the destroyed mech reaches a fallen gesture");
    let (latch_frame, kind) = latched.expect("the destroyed mech latches a damage shape");
    eprintln!("mech_fall: fallen gesture {g} at frame {gesture_frame}");
    assert!(
        handed_over > death_frame,
        "the hand-over follows the removal, not the kill"
    );
    // The kill itself sets the fall goal (mover/mech.rs, update_incapacitated_mech), so the fall precedes the hand-over.
    assert!(gesture_frame <= latch_frame);
    // The fallen gesture and the latched damage shape agree on direction.
    assert_eq!(
        g == gesture::FALLEN_FORWARD,
        kind == ShapeKind::ForwardDamage,
        "fallen gesture {g} matches latched shape {kind:?}"
    );

    // The wreck holds: the shape and gesture stay latched.
    for _ in 0..60 {
        turn += 1;
        sim.step(FrameInput::fixed(30, turn, vis.clone(), vec![]))
            .unwrap();
        let a = sim.world.appearance.mech(mech).unwrap();
        assert_eq!(
            a.shape_kind, kind,
            "the wreck shape holds (latched at {latch_frame})"
        );
        assert_eq!(a.current_gesture, g, "the fallen gesture holds");
        assert_eq!(a.cpp_status, DESTROYED);
    }
}
