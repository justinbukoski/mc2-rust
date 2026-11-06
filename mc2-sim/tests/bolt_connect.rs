// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `WeaponBolt::connect` (weaponbolt.h:304-363): target and point versions.

mod common;

use common::fire_dump::*;
use mc2_sim::combat::shot::apply_difficulty_weapon;
use mc2_sim::combat::weapon_bolt::{
    bolt_params, bolt_state, connect_point, connect_target, create,
};
use mc2_sim::combat::WeaponShotInfo;
use mc2_sim::math::Vec3;
use mc2_sim::object::{Kind, ObjectClass};

#[test]
fn connect_draws_one_dice_and_copies_the_shot_with_scaling() {
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.difficulty = 0;
    let effect = bolt_effect_ids(&fx)[0];
    let owner = find(&fx, 709);
    let owner_wid = fx.w.objects.watch_id(owner);
    let is_player = fx.w.objects.get(owner).unwrap().commander_id() == 0;
    let mech_target = find(&fx, 706);
    let vehicle_target = fx
        .movers
        .iter()
        .copied()
        .find(|&h| fx.w.objects.get(h).unwrap().c.class == ObjectClass::GroundVehicle)
        .expect("mc2_01 has a vehicle");
    let d = 10.0f32;
    let once = apply_difficulty_weapon(&fx.w, d, is_player);
    let twice = apply_difficulty_weapon(&fx.w, once, is_player);
    for (target, is_mech) in [(mech_target, true), (vehicle_target, false)] {
        let bolt = create(&mut fx.w, effect).unwrap();
        let shot = WeaponShotInfo::init(&fx.w, owner_wid, 156, d, 1, 0.0);
        assert_eq!(shot.damage, once);
        let (sim, _) = draws(&fx);
        connect_target(&mut fx.w, bolt, owner, target, Some(&shot), 1, 0);
        let (sim_after, _) = draws(&fx);
        let b = bolt_state(&fx.w, bolt).unwrap();
        if is_mech {
            assert_eq!(sim_after, sim + 1, "RollDice(50) for a mech target");
            assert!(b.hot_spots.1 == 0 || b.hot_spots.1 == 1);
        } else {
            assert_eq!(sim_after, sim, "no dice for a vehicle");
            assert_eq!(b.hot_spots.1, -1);
        }
        assert_eq!(b.hot_spots.0, 1);
        // `weaponShot = *shotInfo` is a copy assignment: init runs again, so the difficulty scaling is applied twice
        assert_eq!(b.weapon_shot.unwrap().damage, twice);
        let p = bolt_params(&fx.w, bolt).unwrap();
        let recycle = fx.w.appearance.weapon_node_recycle(owner, 1);
        assert_eq!(recycle, if p.is_beam { p.beam_duration } else { 0.25 });
    }
    // the point version: no target, the point is stored
    let bolt = create(&mut fx.w, effect).unwrap();
    let shot = WeaponShotInfo::init(&fx.w, owner_wid, 156, d, 1, 0.0);
    let (sim, _) = draws(&fx);
    let at = Vec3 {
        x: 10.0,
        y: 20.0,
        z: 30.0,
    };
    connect_point(&mut fx.w, bolt, owner, at, Some(&shot), 4);
    assert_eq!(draws(&fx).0, sim);
    let Kind::WeaponBolt(b) = &fx.w.objects.get(bolt).unwrap().kind else {
        panic!()
    };
    assert!(b.target.is_none() && b.has_target_position);
    assert_eq!(b.target_position, at);
    assert_eq!(b.hot_spots.0, 4);
    assert_eq!(b.weapon_shot.unwrap().damage, twice);
}
