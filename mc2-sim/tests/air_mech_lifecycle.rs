// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Air movers (`MoveType` AIR, a BattleMech with a helicopter profile) through the movement and incapacitated branches of
//! `BattleMech::update`, on mc2_16's hostile 707 (rc-ai-mc2-16).

mod common;

use mc2_sim::ids::ObjectHandle;
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim::World;
use mc2_sim_data::objtype::ObjectTypeData;

fn is_air(w: &World, h: ObjectHandle) -> bool {
    matches!(
        w.objects.type_of(h).map(|t| &t.data),
        Some(ObjectTypeData::BattleMech(t)) if t.move_type == 1
    )
}

fn mech(w: &mut World, h: ObjectHandle) -> &mut mc2_sim::mover::MechState {
    match &mut w.objects.get_mut(h).unwrap().kind {
        Kind::Mech(m) => m,
        _ => unreachable!(),
    }
}

/// mc2_16's hostile 707 (an air mech) and player mech 512 (a ground mech).
fn load() -> Option<(common::Fixture, ObjectHandle, ObjectHandle)> {
    let fixture = common::fixture("mc2_16", "pcg")?;
    let air = fixture.mover_handles[&707];
    let ground = fixture.mover_handles[&512];
    assert!(is_air(&fixture.world, air), "mc2_16 707 is an air mover");
    assert!(
        !is_air(&fixture.world, ground),
        "mc2_16 512 is a ground mover"
    );
    Some((fixture, air, ground))
}

/// `if (failedPilotingCheck && (getMoveType() != MOVETYPE_AIR))` (mech.cpp:4871): an air mech never takes the fall branch,
/// so it draws no `RandomNumber(2)` fall gesture and its failed check stays set. Oracle mc2_16 `ai` seed 1: 707 loses its
/// left leg at frame 668 (the check fails, roll 135 vs skill 5) and keeps moving with the flag set; Rust drew the fall
/// at frame 669 and stopped it.
#[test]
fn air_mech_failed_piloting_check_skips_the_fall_branch() {
    let Some((mut fixture, air, ground)) = load() else {
        return;
    };
    let w = &mut fixture.world;
    let draws = |w: &World| w.rng.0.borrow().sim.draws();

    // The ground mech takes the branch: one fall roll, the check is consumed.
    mech(w, ground).failed_piloting_check = true;
    let before = draws(w);
    mc2_sim::mover::mech::update_movement(w, ground).unwrap();
    assert_eq!(
        draws(w),
        before + 1,
        "RandomNumber(2) for the fall gesture (mech.cpp:4872)"
    );
    assert!(!mech(w, ground).failed_piloting_check);

    // The air mech passes it by: no fall roll, the flag stays set, no fall goal reaches its appearance.
    mech(w, air).failed_piloting_check = true;
    let before = draws(w);
    mc2_sim::mover::mech::update_movement(w, air).unwrap();
    assert_eq!(draws(w), before, "no fall roll for an air mover");
    assert!(mech(w, air).failed_piloting_check);
    let goal = w.appearance.mech(air).unwrap().current_state_goal;
    assert!(
        goal < mc2_sim::appearance::tables::state::FALLEN_FORWARD,
        "no fall goal, got {goal}"
    );
}

/// The disabled branch of `BattleMech::update` (mech.cpp:6091-6159) slides the mech and counts `timeLeft` down, and only
/// then turns an air mover DESTROYED with `timeLeft = 0`. A destroyed mech's status reaches the appearance only after it is
/// removed (`if (!mechRemoved && !isDestroyed()) appearance->setObjStatus(...)`, :6282; `else if (mechRemoved)
/// appearance->setObjStatus(...)`, :6231). Oracle mc2_16 `ai` seed 2: 707 is disabled at frame 670 and destroyed at 671
/// with timeLeft 0, and slides one more step at 672; Rust turned it DESTROYED before the disabled branch.
#[test]
fn disabled_air_mech_turns_destroyed_after_the_disabled_branch() {
    let Some((mut fixture, air, _)) = load() else {
        return;
    };
    let w = &mut fixture.world;
    mc2_sim::combat::damage::disable(w, air, 2);
    let appearance_status = w.appearance.mech(air).unwrap().status;
    assert_ne!(appearance_status, ObjectStatus::Destroyed as i32);
    mech(w, air).m.time_left = 5.0;

    mc2_sim::mover::update_mech(w, air).unwrap();
    assert_eq!(
        w.objects.get(air).unwrap().c.status,
        ObjectStatus::Destroyed
    );
    assert_eq!(
        mech(w, air).m.time_left,
        0.0,
        "timeLeft = 0.0 (mech.cpp:6158)"
    );
    assert_eq!(
        w.appearance.mech(air).unwrap().status,
        appearance_status,
        "no status push for a destroyed mech (mech.cpp:6282)"
    );

    // Destroyed, not yet removed: still no push.
    mech(w, air).fallen = true;
    mc2_sim::mover::update_mech(w, air).unwrap();
    assert_eq!(w.appearance.mech(air).unwrap().status, appearance_status);

    // Removed: the destroyed branch pushes the status (mech.cpp:6231-6232).
    mech(w, air).mech_removed = true;
    mech(w, air).m.time_left = -5.0;
    mc2_sim::mover::update_mech(w, air).unwrap();
    assert_eq!(
        w.appearance.mech(air).unwrap().status,
        ObjectStatus::Destroyed as i32
    );
}
