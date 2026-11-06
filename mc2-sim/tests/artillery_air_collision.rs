// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `ArtilleryType::handleCollision`: airborne mechs are immune to boomed strikes (artlry.cpp:586-592).
//!
//! The mech's type is swapped for a copy with `MoveType` = MOVETYPE_AIR (the table is shared and immutable, so the copy is installed
//! as a whole). An active airborne mech takes no hits and no RNG is drawn; the same mech shut down, and the ground control, are hit
//! `trunc(nominal_major_hits)` times.

mod common;

use std::rc::Rc;

use common::fire_dump::*;
use mc2_sim::combat::artillery::{create_artillery, handle_collision};
use mc2_sim::combat::artillery_math::{ARTILLERY_LARGE, MOVETYPE_AIR};
use mc2_sim::combat::loadout_of;
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim_data::objtype::{ObjectTable, ObjectTypeData};

#[test]
fn artillery_spares_airborne_active_mech_but_hits_shutdown_one() {
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.difficulty = 0;
    let mech = find(&fx, 706);
    for a in &mut mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(mech).unwrap())
        .unwrap()
        .armor
    {
        a.0 = 1000.0;
    }
    let at = fx.w.objects.get(mech).unwrap().c.position;
    let strike = create_artillery(&mut fx.w, ARTILLERY_LARGE, at).expect("a free strike slot");
    let Kind::Artillery(a) = &mut fx.w.objects.get_mut(strike).unwrap().kind else {
        panic!()
    };
    a.boom = true;
    let handled = |fx: &Fixture| {
        loadout_of(fx.w.objects.get(mech).unwrap())
            .unwrap()
            .stats
            .num_weapon_hits_handled
    };

    // (c) control: ground mech is hit
    let (armor, n, d) = (total_armor(&fx, mech), handled(&fx), draws(&fx));
    assert!(!handle_collision(&mut fx.w, strike, mech));
    assert!(total_armor(&fx, mech) < armor);
    assert!(handled(&fx) > n);
    assert!(draws(&fx) > d);

    // Swap in an airborne copy of the mech's type.
    let num = fx.w.objects.get(mech).unwrap().c.type_handle;
    let mut table = ObjectTable::default();
    for (k, t) in &fx.w.objects.types.as_ref().unwrap().types {
        table.types.insert(*k, t.clone());
    }
    let Some(ObjectTypeData::BattleMech(m)) =
        table.types.get_mut(&(num as u32)).map(|t| &mut t.data)
    else {
        panic!("mech type")
    };
    m.move_type = MOVETYPE_AIR;
    fx.w.objects.types = Some(Rc::new(table));

    // (a) airborne and active: immune, nothing changes
    let (armor, n, d) = (total_armor(&fx, mech), handled(&fx), draws(&fx));
    assert!(!handle_collision(&mut fx.w, strike, mech));
    assert_eq!(
        (total_armor(&fx, mech), handled(&fx), draws(&fx)),
        (armor, n, d)
    );

    // (b) airborne but shut down: hit
    fx.w.objects.get_mut(mech).unwrap().c.status = ObjectStatus::ShutDown;
    assert!(!handle_collision(&mut fx.w, strike, mech));
    assert!(total_armor(&fx, mech) < armor);
    assert!(handled(&fx) > n);
}
