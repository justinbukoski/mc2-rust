// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Artillery::update` timers and boom (artlry.cpp:760-790, 852-1075).

mod common;

use common::fire_dump::*;
use mc2_sim::combat::artillery::{call_strike_seconds, update};
use mc2_sim::combat::artillery_math::ARTILLERY_LARGE;
use mc2_sim::math::Vec3;
use mc2_sim::object::Kind;
use mc2_sim_data::objtype::ObjectTypeData;

fn state(fx: &Fixture, h: mc2_sim::ids::ObjectHandle) -> (f32, bool, bool, bool) {
    let o = fx.w.objects.get(h).unwrap();
    let Kind::Artillery(a) = &o.kind else {
        panic!("not an artillery strike")
    };
    (a.time_to_impact, a.whoosh, a.boom, o.c.tangible())
}

#[test]
fn artillery_counts_down_and_booms() {
    let Some(mut fx) = mc2_01() else { return };
    fx.w.clock.frame_length = 0.5;
    let at = Vec3 {
        x: 10.0,
        y: 20.0,
        z: 0.0,
    };
    let pool = fx.w.objects.artillery.clone();
    assert!(call_strike_seconds(&mut fx.w, 0, ARTILLERY_LARGE, at, 4));
    let h = pool[1];
    // the creation update is the JUSTCREATED frame: no decrement; the whoosh check runs in it too (4.0 <= 5.5)
    let (tti, whoosh, boom, tangible) = state(&fx, h);
    assert_eq!(tti, 4.0);
    assert!(whoosh && !boom && !tangible);
    for k in 1..=7 {
        assert!(update(&mut fx.w, h));
        let (tti, _, boom, _) = state(&fx, h);
        assert_eq!(tti, 4.0 - 0.5 * k as f32);
        assert!(!boom, "no boom at t={tti}");
    }
    // the 8th decrement brings 4.0 to 0.0: boom, and the strike is tangible for the collision pass of this frame
    assert!(update(&mut fx.w, h));
    let (tti, _, boom, tangible) = state(&fx, h);
    assert_eq!(tti, 0.0);
    assert!(boom && tangible);
    // DESIGN pin (headless: no lingering effects): the strike lives exactly one more frame
    assert!(!update(&mut fx.w, h));
    assert!(!fx.w.objects.get(h).unwrap().exists());
    // 0 seconds means the type's full duration, read on the creation update
    assert!(call_strike_seconds(&mut fx.w, 0, ARTILLERY_LARGE, at, 0));
    let h2 = pool[2];
    let nominal = match &fx.w.objects.type_of(h2).unwrap().data {
        ObjectTypeData::Artillery(a) => a.nominal_time_to_impact,
        _ => panic!(),
    };
    assert_eq!(state(&fx, h2).0, nominal);
}
