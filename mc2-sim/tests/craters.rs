// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Persistent craters and footprints (`mclib/crater.cpp`, weaponbolt.cpp:808, mech3d.cpp:3175).
//! Needs `MC2_DATA` (the shipped mission `mc2_01`); without it the data tests return early.

mod common;

use common::fire_dump::*;
use mc2_sim::combat::bolt_impact::on_hit;
use mc2_sim::combat::weapon_bolt::{bolt_state_mut, connect_point, create};
use mc2_sim::combat::WeaponShotInfo;
use mc2_sim::crater::{CraterPool, CRATER_1, MAX_CRATERS};
use mc2_sim::math::Vec3;
use mc2_sim::object::Kind;
use mc2_sim::SimEvent;

fn craters(events: &[SimEvent]) -> Vec<(i32, Vec3, f32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::Crater { kind, at, rotation } => Some((*kind, *at, *rotation)),
            _ => None,
        })
        .collect()
}

/// A bolt that missed and hit land adds `CRATER_1 + level` at the impact point, the level by the shot's damage
/// (weaponbolt.cpp:808-817), turned by the visual stream's `RandomNumber(180)`; it adds none over water or without the
/// non-weapon effects.
#[test]
fn a_ground_impact_adds_a_crater_of_the_damage_class_at_the_impact_point() {
    let Some(mut fx) = mc2_01() else { return };
    let owner = find(&fx, 709);
    let owner_wid = fx.w.objects.watch_id(owner);
    let at = fx.w.objects.get(find(&fx, 706)).unwrap().c.position;
    let effect = bolt_effect_ids(&fx)[0];
    // the row names a miss effect, as the shipped game's weapon effects do
    fx.w.objects.effect_rows[effect as usize].names[3] = "ground_hit".into();
    fx.w.objects.use_non_weapon_effects = true;
    let shoot = |fx: &mut Fixture, damage: f32| {
        let bolt = create(&mut fx.w, effect).unwrap();
        let shot = WeaponShotInfo::init(&fx.w, owner_wid, 156, damage, -1, 0.0);
        connect_point(&mut fx.w, bolt, owner, at, Some(&shot), 1);
        bolt_state_mut(&mut fx.w, bolt).unwrap().hit_target = true;
        fx.w.events.clear();
        let (sim, vis) = draws(fx);
        on_hit(&mut fx.w, bolt);
        let (sim2, vis2) = draws(fx);
        assert_eq!((sim2, vis2), (sim, vis + 1), "one visual draw, no sim draw");
        craters(&fx.w.events)
    };
    for (damage, kind) in [(2.0, 64), (3.5, 65), (7.0, 66), (12.0, 67)] {
        let c = shoot(&mut fx, damage);
        assert_eq!(c.len(), 1, "damage {damage}");
        assert_eq!(c[0].0, kind, "damage {damage}");
        assert_eq!(c[0].0 - CRATER_1, kind - 64);
        assert_eq!((c[0].1.x, c[0].1.y), (at.x, at.y));
        assert!((0.0..180.0).contains(&c[0].2), "rotation {}", c[0].2);
    }
    // the preference off: no crater, no visual draw
    fx.w.objects.use_non_weapon_effects = false;
    let bolt = create(&mut fx.w, effect).unwrap();
    let shot = WeaponShotInfo::init(&fx.w, owner_wid, 156, 12.0, -1, 0.0);
    connect_point(&mut fx.w, bolt, owner, at, Some(&shot), 1);
    bolt_state_mut(&mut fx.w, bolt).unwrap().hit_target = true;
    fx.w.events.clear();
    on_hit(&mut fx.w, bolt);
    assert!(craters(&fx.w.events).is_empty());
}

/// A walking mech prints each foot when its animation frame comes within two frames of a foot-down frame of the gesture,
/// once per visit of that window, at the foot node, with the mech's facing and the type's footprint id.
#[test]
fn a_walking_mech_leaves_footprints_at_the_foot_down_frames() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = (0..2000)
        .map(mc2_sim::ids::ObjectHandle)
        .find(|&h| {
            matches!(fx.w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(_)))
                && fx.w.appearance.mech(h).is_some()
        })
        .expect("a mech");
    fx.w.objects.use_non_weapon_effects = true;
    // the shipped `[FootPrint]` blocks: valid ids, and not every mech wears the same boot
    let types = &fx.w.appearance.mech_types;
    assert!(types
        .iter()
        .all(|t| (0..=63).contains(&t.left_footprint_type)
            && (0..=63).contains(&t.right_footprint_type)));
    assert!(
        types.iter().any(|t| t.right_footprint_type != 0),
        "some mech has a footprint id"
    );
    mc2_sim::appearance::recalc_bounds(&mut fx.w, mech, true, 0);
    let (walk, down, kinds) = {
        let (m, t) = fx.w.appearance.mech_mut(mech).unwrap();
        m.rotation = 37.0;
        let walk = mc2_sim::appearance::tables::gesture::WALK;
        t.gestures[walk as usize].foot_down = [3, 15, 9, 24];
        t.right_footprint_type = 3;
        t.left_footprint_type = 5;
        m.current_gesture = walk;
        m.current_state_goal = -1;
        m.current_frame = 0.0;
        (walk, t.gestures[walk as usize].foot_down, (3, 5))
    };
    let frames = fx.w.appearance.mech_types[fx.w.appearance.mech(mech).unwrap().type_index]
        .num_frames(walk) as f32;
    assert!(frames > 27.0, "walk has {frames} frames");

    // two full cycles of the walk, collecting what the sim raised and the frame it did so in
    let mut seen = Vec::new();
    let mut prev_in: [bool; 4] = [false; 4];
    let mut expected = Vec::new();
    for _ in 0..400 {
        fx.w.events.clear();
        mc2_sim::appearance::update(&mut fx.w, mech, true);
        let m = fx.w.appearance.mech(mech).unwrap();
        let f = m.current_frame;
        for (i, d) in down.iter().enumerate() {
            let inside = f >= (*d - 2) as f32 && f <= (*d + 2) as f32;
            if inside && !prev_in[i] {
                expected.push(if i < 2 { kinds.0 } else { kinds.1 });
            }
            prev_in[i] = inside;
        }
        for c in craters(&fx.w.events) {
            seen.push(c.0);
            assert_eq!(c.2, 37.0, "the mech's facing");
        }
    }
    assert!(expected.len() >= 6, "{} windows entered", expected.len());
    assert_eq!(
        seen, expected,
        "one print per visit of a foot-down window, right foot type 3, left foot type 5"
    );
}

/// The pool is a ring of `MAX_CRATERS` slots written in turn: the 1001st decal replaces the first.
#[test]
fn the_pool_replaces_the_oldest_decal_after_a_thousand() {
    let mut p = CraterPool::default();
    assert_eq!(p.capacity(), 1000);
    assert_eq!(MAX_CRATERS, 1000);
    let flat = |_: &Vec3| 0.0;
    for i in 0..1000 {
        let at = Vec3 {
            x: i as f32,
            y: 0.0,
            z: 0.0,
        };
        p.add(CRATER_1 + 1, &at, 0.0, &flat);
    }
    assert_eq!(p.len(), 1000);
    assert_eq!(p.next_slot(), 0);
    let at = Vec3 {
        x: 5000.0,
        y: 0.0,
        z: 0.0,
    };
    p.add(0, &at, 0.0, &flat);
    assert_eq!(p.len(), 1000, "still a thousand");
    assert_eq!(p.next_slot(), 1);
    // slot 0 now holds the new footprint, slot 1 the second old crater
    assert_eq!(p.slot(0).unwrap().shape_id, 0);
    assert_eq!(p.slot(0).unwrap().corners[0].x, 5000.0 - 16.0);
    assert_eq!(p.slot(1).unwrap().shape_id, CRATER_1 + 1);
    assert_eq!(p.slot(1).unwrap().corners[0].x, 1.0 - 32.0);
}
