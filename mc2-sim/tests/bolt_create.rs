// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `GameObjectManager::createWeaponBolt` (objmgr.cpp:2734): pool slot, init, exists.

mod common;

use common::fire_dump::*;
use mc2_sim::combat::weapon_bolt::{bolt_state, create};
use mc2_sim::object::flags;

#[test]
fn create_bolt_takes_the_next_pool_slot() {
    let Some(mut fx) = mc2_01() else { return };
    let effect = *bolt_effect_ids(&fx).first().expect("a bolt effect");
    let weapons = fx.w.objects.weapons.clone();
    let n = weapons.len();
    assert!(n >= 3);
    // `weapon_index` pre-increments from 0: the first bolt is weapons[1], the second weapons[2]
    let (sim, vis) = draws(&fx);
    let first = create(&mut fx.w, effect).expect("a weapon bolt type");
    assert_eq!(first, weapons[1]);
    let o = fx.w.objects.get(first).unwrap();
    assert!(o.exists());
    assert!(o.c.flag(flags::JUSTCREATED));
    let b = bolt_state(&fx.w, first).unwrap();
    assert!(b.just_created);
    assert_eq!(b.effect_id, effect);
    assert_eq!(draws(&fx), (sim, vis), "creating a bolt draws nothing");
    let second = create(&mut fx.w, effect).unwrap();
    assert_eq!(second, weapons[2]);
    // the ring wraps: call number c (1-based) returns weapons[c % n]
    for c in 3..=(n + 2) {
        let h = create(&mut fx.w, effect).unwrap();
        assert_eq!(h, weapons[c % n], "call {c}");
    }
    // an effect whose object type is not a weapon bolt creates nothing (but consumes the slot)
    let non_bolt = (0..fx.w.objects.effect_objects.len() as i32)
        .find(|e| !bolt_effect_ids(&fx).contains(e))
        .expect("a non-bolt effect");
    assert!(create(&mut fx.w, non_bolt).is_none());
}
