// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The JUSTCREATED frame of `WeaponBolt::update` (weaponbolt.cpp:380-512).

mod common;

use common::fire_dump::*;
use mc2_sim::combat::bolt_flight::first_frame;
use mc2_sim::combat::weapon_bolt::{bolt_params, bolt_state, connect_point, create};
use mc2_sim::combat::WeaponShotInfo;
use mc2_sim::math::Vec3;
use mc2_sim::object::flags;

#[test]
fn first_frame_initialises_distance_and_goal_height() {
    let Some(mut fx) = mc2_01() else { return };
    let effect = bolt_effect_ids(&fx)[0];
    let owner = find(&fx, 709);
    let owner_wid = fx.w.objects.watch_id(owner);
    let hs = 1;
    let pos = fx.w.appearance.weapon_node_position(owner, hs);
    let point = Vec3 {
        x: pos.x + 100.0,
        y: pos.y,
        z: pos.z,
    };
    let bolt = create(&mut fx.w, effect).unwrap();
    let shot = WeaponShotInfo::init(&fx.w, owner_wid, 156, 1.0, -1, 0.0);
    connect_point(&mut fx.w, bolt, owner, point, Some(&shot), hs);
    let before = draws(&fx);
    assert!(first_frame(&mut fx.w, bolt));
    assert_eq!(draws(&fx), before, "no random number is drawn");
    let b = bolt_state(&fx.w, bolt).unwrap();
    assert_ne!(b.shot_id, 0, "created shots have a presentation identity");
    assert!(fx.w.events.iter().any(|event| matches!(
        event,
        mc2_sim::SimEvent::WeaponEffect {
            shot_id,
            impact: mc2_sim::WeaponImpact::Launch,
            ..
        } if *shot_id == b.shot_id
    )));
    let params = bolt_params(&fx.w, bolt).unwrap();
    // the first frame measures from the owner's hot spot to the point; z is ignored
    let (dx, dy) = (point.x - pos.x, point.y - pos.y);
    assert_eq!(b.distance_to_target, dx * dx + dy * dy);
    assert_eq!(b.half_distance_to_target, b.distance_to_target / 2.0);
    assert_eq!(b.goal_height, params.arc_height * 10.0);
    assert_eq!(b.laser_position, pos);
    assert!(!b.just_created && !b.hit_target);
    let o = fx.w.objects.get(bolt).unwrap();
    assert!(!o.c.flag(flags::JUSTCREATED) && !o.c.tangible());
    // a bolt that was never connected refuses the first frame
    let lone = create(&mut fx.w, effect).unwrap();
    assert!(!first_frame(&mut fx.w, lone));
}
