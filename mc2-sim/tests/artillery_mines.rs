// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Artillery::handleStaticCollision`: clears mines in a 9x9 cell area (artlry.cpp:670-720).

mod common;

use common::fire_dump::*;
use mc2_sim::combat::artillery::create_artillery;
use mc2_sim::combat::artillery_math::ARTILLERY_LARGE;
use mc2_sim::object::flags;
use mc2_sim::object::hooks::artillery_handle_static_collision;
use mc2_sim::object::terr::{cell_to_world, in_bounds, world_to_cell};

/// The test that a tangible artillery strike clears mines in a 9x9 area around its position.
#[test]
fn boomed_strike_clears_mines_in_nine_by_nine() {
    let Some(mut fx) = mc2_01() else { return };

    // Set mine_splash to non-zero (explicit input, not a compared output).
    fx.w.objects.mine_splash = (50.0, 10.0);

    // Find a mech to use as a position reference.
    let mech = find(&fx, 706);
    let pos = fx.w.objects.get(mech).unwrap().c.position;

    // Create an artillery strike at the mech's position.
    let strike = create_artillery(&mut fx.w, ARTILLERY_LARGE, pos).expect("a free strike slot");

    // Set the TANGIBLE flag on the strike.
    let o = fx.w.objects.get_mut(strike).unwrap();
    o.c.set_flag(flags::TANGIBLE, true);

    // Get the cell position of the strike.
    let (r, c) = world_to_cell(&fx.w.terrain, &pos);

    // Set mines at specific cells:
    // - (r+4, c-4): should be cleared (inside 9x9 area)
    // - (r, c): should be cleared (inside 9x9 area)
    // - (r+5, c): should NOT be cleared (outside 9x9 area)
    assert!(
        in_bounds(&fx.w.terrain, r + 4, c - 4),
        "cell r+4,c-4 in bounds"
    );
    assert!(in_bounds(&fx.w.terrain, r, c), "cell r,c in bounds");
    assert!(in_bounds(&fx.w.terrain, r + 5, c), "cell r+5,c in bounds");

    fx.w.terrain.set_mine(r + 4, c - 4, 1);
    fx.w.terrain.set_mine(r, c, 1);
    fx.w.terrain.set_mine(r + 5, c, 1);

    // Count existing carnage objects (explosions or fires) before the call.
    let carnage_before: Vec<_> =
        fx.w.objects
            .carnage
            .iter()
            .filter(|&&h| {
                fx.w.objects.get(h).is_some_and(|obj| {
                    obj.exists() && matches!(obj.kind, mc2_sim::object::Kind::Carnage(_))
                })
            })
            .collect();
    let count_before = carnage_before.len();

    // Call the hook (this is what we're testing).
    artillery_handle_static_collision(&mut fx.w, strike);

    // After the hook:
    // - (r+4, c-4) should be cleared (mine 2)
    // - (r, c) should be cleared (mine 2)
    // - (r+5, c) should NOT be cleared (mine 1)
    assert_eq!(
        fx.w.terrain.get_mine(r + 4, c - 4),
        2,
        "mine at r+4,c-4 should be cleared (2)"
    );
    assert_eq!(
        fx.w.terrain.get_mine(r, c),
        2,
        "mine at r,c should be cleared (2)"
    );
    assert_eq!(
        fx.w.terrain.get_mine(r + 5, c),
        1,
        "mine at r+5,c should NOT be cleared (1)"
    );

    // Count explosion events - there should be exactly 2 (for the two cleared mines).
    let explosion_events: Vec<_> = fx.w.events.iter().filter(|e| {
        matches!(e, mc2_sim::events::SimEvent::Explosion { effect_id, .. } if *effect_id == 53)
    }).collect();
    assert_eq!(
        explosion_events.len(),
        2,
        "should have exactly 2 mine explosion events"
    );

    // Verify the explosion positions match the cleared mine cells.
    let pos_r4_c4 = cell_to_world(&fx.w.terrain, r + 4, c - 4);
    let pos_r_c = cell_to_world(&fx.w.terrain, r, c);

    let found_r4_c4 = explosion_events
        .iter()
        .any(|e| matches!(e, mc2_sim::events::SimEvent::Explosion { at, .. } if *at == pos_r4_c4));
    let found_r_c = explosion_events
        .iter()
        .any(|e| matches!(e, mc2_sim::events::SimEvent::Explosion { at, .. } if *at == pos_r_c));

    assert!(found_r4_c4, "explosion at r+4,c-4 not found");
    assert!(found_r_c, "explosion at r,c not found");

    // Also verify the carnage pool was updated correctly.
    let carnage_after: Vec<_> =
        fx.w.objects
            .carnage
            .iter()
            .filter(|&&h| {
                fx.w.objects.get(h).is_some_and(|obj| {
                    obj.exists() && matches!(obj.kind, mc2_sim::object::Kind::Carnage(_))
                })
            })
            .collect();
    let count_after = carnage_after.len();
    assert_eq!(
        count_after,
        count_before + 2,
        "carnage count should increase by 2 (from {} to {})",
        count_before,
        count_after
    );
}

/// The test that an intangible strike does NOT clear mines.
#[test]
fn intangible_strike_does_not_clear_mines() {
    let Some(mut fx) = mc2_01() else { return };

    // Set mine_splash to non-zero.
    fx.w.objects.mine_splash = (50.0, 10.0);

    // Find a mech to use as a position reference.
    let mech = find(&fx, 706);
    let pos = fx.w.objects.get(mech).unwrap().c.position;

    // Create an artillery strike at the mech's position.
    let strike = create_artillery(&mut fx.w, ARTILLERY_LARGE, pos).expect("a free strike slot");

    // DO NOT set the TANGIBLE flag (intangible strike).
    // The strike should already have TANGIBLE = false from create_artillery.

    // Get the cell position of the strike.
    let (r, c) = world_to_cell(&fx.w.terrain, &pos);

    // Set a mine at (r, c).
    assert!(in_bounds(&fx.w.terrain, r, c), "cell r,c in bounds");
    fx.w.terrain.set_mine(r, c, 1);

    // Call the hook (should not clear mines because strike is intangible).
    artillery_handle_static_collision(&mut fx.w, strike);

    // The mine should NOT be cleared (still 1, not 2).
    assert_eq!(
        fx.w.terrain.get_mine(r, c),
        1,
        "mine at r,c should NOT be cleared for intangible strike"
    );
}

/// A tangible strike standing on a static object of its terrain block is tested against it (artlry.cpp:724-749): the pair
/// reaches `checkExtents`. An intangible strike tests nothing.
#[test]
fn tangible_strike_tests_static_objects_of_its_block() {
    let Some(mut fx) = mc2_01() else { return };
    // The mc2_01 fixture loads no terrain objects, so a live mech stands in for the static object: it is listed in the strike's
    // own terrain block (a declared placement), which is all `handleStaticCollision` reads from the block table.
    let target = find(&fx, 706);
    let pos = fx.w.objects.get(target).unwrap().c.position;
    let probe = create_artillery(&mut fx.w, ARTILLERY_LARGE, pos).expect("a free strike slot");
    let (block, _) = mc2_sim::object::carnage::block_and_vertex_number(&fx.w, probe);
    let blocks = &mut fx.w.objects.blocks;
    if blocks.len() <= block as usize {
        blocks.resize(block as usize + 1, Default::default());
    }
    blocks[block as usize] = mc2_sim::object::store::ObjBlock {
        first_handle: target.0,
        num_objects: 1,
        num_collidable: 0,
    };
    fx.w.objects.collision.log_enabled = true;
    let strike = probe;

    // intangible: nothing is tested
    artillery_handle_static_collision(&mut fx.w, strike);
    assert!(fx.w.objects.collision.log.is_empty());

    fx.w.objects
        .get_mut(strike)
        .unwrap()
        .c
        .set_flag(flags::TANGIBLE, true);
    artillery_handle_static_collision(&mut fx.w, strike);
    assert!(
        fx.w.objects
            .collision
            .log
            .iter()
            .any(|&(a, b)| a == strike.0 && b == target.0),
        "the strike must reach checkExtents with the static object under it"
    );
}
