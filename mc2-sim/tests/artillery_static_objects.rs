// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Artillery::handleStaticCollision` against real static objects (artlry.cpp:724-749): the production hook
//! (`hooks::artillery_handle_static_collision`) with the buildings and turrets of a loaded mission, in the strike's own terrain block
//! and in a neighbouring one, plus the mines of the 9x9 area in the same pass.
//!
//! Hand-derived expectations: the ARTILLERY_LARGE type has `nominalDamage` 15, `nominalMajorHits` 3, `nominalMinorHits` 2 and
//! `nominalMajorRange` 25 m. A strike standing on the object is a major hit (range 0), so three shots of 15 reach it (difficulty 0
//! leaves the damage unscaled): 45 damage, capped at the building's damage level (`Building::handleWeaponHit`), added without cap to a
//! turret (`Turret::handleWeaponHit`, destroyed once it reaches its level).

mod common;

use common::fixture;
use mc2_sim::combat::artillery::create_artillery;
use mc2_sim::combat::artillery_math::ARTILLERY_LARGE;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::Vec3;
use mc2_sim::object::carnage::block_and_vertex_number;
use mc2_sim::object::hooks::artillery_handle_static_collision;
use mc2_sim::object::terr::{get_mine, set_mine, world_to_cell};
use mc2_sim::object::{flags, structure, Kind};
use mc2_sim::World;

const HITS: f32 = 3.0;
const SHOT: f32 = 15.0;
/// 20 vertices of 128 world units: one terrain block along x.
const BLOCK_X: f32 = 2560.0;

fn find_static(w: &World, building: bool, min_level: f32) -> Option<ObjectHandle> {
    (0..w.objects.max_objects() as u32)
        .map(ObjectHandle)
        .find(|&h| {
            let Some(o) = w.objects.get(h) else {
                return false;
            };
            let kind_ok = if building {
                matches!(o.kind, Kind::Building(_))
            } else {
                matches!(o.kind, Kind::Turret(_))
            };
            kind_ok
                && o.exists()
                && !o.c.is_destroyed()
                && o.structure().is_some_and(|s| s.damage == 0.0)
                && structure::damage_level(w, h) > min_level
        })
}

fn boomed_strike(w: &mut World, at: Vec3) -> ObjectHandle {
    let s = create_artillery(w, ARTILLERY_LARGE, at).expect("a free strike slot");
    if let Kind::Artillery(a) = &mut w.objects.get_mut(s).unwrap().kind {
        a.boom = true;
    }
    w.objects
        .get_mut(s)
        .unwrap()
        .c
        .set_flag(flags::TANGIBLE, true);
    s
}

fn damage(w: &World, h: ObjectHandle) -> f32 {
    w.objects.get(h).unwrap().structure().unwrap().damage
}

fn carnage_count(w: &World) -> usize {
    w.objects
        .carnage
        .iter()
        .filter(|&&h| w.objects.get(h).is_some_and(|o| o.exists()))
        .count()
}

/// A building under the strike (its own block) takes three shots of 15; a second strike finishes it: capped at its level, destroyed,
/// intangible. The mine under the strike goes off in the same pass.
#[test]
fn strike_damages_and_destroys_a_building_in_its_own_block() {
    let Some(mut fx) = fixture("mc2_17", "msvc") else {
        return;
    };
    let w = &mut fx.world;
    w.mission.difficulty = 0;
    w.objects.mine_splash = (50.0, 10.0);
    let b = find_static(w, true, HITS * SHOT).expect("a building that survives one strike");
    let level = structure::damage_level(w, b);
    assert!(
        level < 2.0 * HITS * SHOT,
        "two strikes finish it (level {level})"
    );
    let at = w.objects.get(b).unwrap().c.position;
    let s = boomed_strike(w, at);
    assert_eq!(
        block_and_vertex_number(w, s).0,
        block_and_vertex_number(w, b).0,
        "the strike stands in the building's block"
    );
    let (r, c) = world_to_cell(&w.terrain, &at);
    set_mine(&mut w.terrain, r, c, 1);
    let carnage_before = carnage_count(w);

    artillery_handle_static_collision(w, s);
    assert_eq!(damage(w, b), HITS * SHOT, "three shots of 15");
    assert!(!w.objects.get(b).unwrap().c.is_destroyed());
    assert_eq!(
        get_mine(&w.terrain, r, c),
        2,
        "the mine under the strike is spent"
    );
    assert!(carnage_count(w) > carnage_before, "the mine exploded");

    artillery_handle_static_collision(w, s);
    let o = w.objects.get(b).unwrap();
    assert_eq!(
        damage(w, b),
        level.min(2.0 * HITS * SHOT),
        "capped at the damage level"
    );
    assert!(o.c.is_destroyed());
    assert!(!o.c.tangible());
}

/// The scan covers the eight neighbouring blocks: the strike one block east of the building's block still reaches it (the building
/// is placed under the strike, a declared placement; the block table is the loaded one). Two blocks away it does not.
#[test]
fn strike_reaches_a_building_in_a_neighbouring_block_only() {
    let Some(mut fx) = fixture("mc2_17", "msvc") else {
        return;
    };
    let w = &mut fx.world;
    w.mission.difficulty = 0;
    let b = find_static(w, true, HITS * SHOT).expect("a building");
    let home = w.objects.get(b).unwrap().c.position;
    let home_block = block_and_vertex_number(w, b).0;

    // one block west (x - 2560 is block - 1 for a building not in the first block column)
    let near = Vec3 {
        x: home.x - BLOCK_X,
        ..home
    };
    let s = boomed_strike(w, near);
    assert_eq!(
        block_and_vertex_number(w, s).0,
        home_block - 1,
        "a neighbouring block"
    );
    w.objects.get_mut(b).unwrap().c.position = near;
    artillery_handle_static_collision(w, s);
    assert_eq!(
        damage(w, b),
        HITS * SHOT,
        "the neighbour block's building is hit"
    );

    // two blocks west: outside the 3x3 scan, so a building standing right under the strike is not tested
    let far = Vec3 {
        x: home.x - 2.0 * BLOCK_X,
        ..home
    };
    let s2 = boomed_strike(w, far);
    assert_eq!(block_and_vertex_number(w, s2).0, home_block - 2);
    w.objects.get_mut(b).unwrap().c.position = far;
    artillery_handle_static_collision(w, s2);
    assert_eq!(
        damage(w, b),
        HITS * SHOT,
        "unchanged: two blocks away is not scanned"
    );
}

/// A turret under the strike (range 0, inside its little extent) takes the three shots; it is destroyed when they reach its level.
#[test]
fn strike_damages_a_turret_under_it() {
    for mission in ["mc2_01", "mc2_03", "mc2_05", "mc2_17"] {
        let Some(mut fx) = fixture(mission, "msvc") else {
            return;
        };
        let w = &mut fx.world;
        w.mission.difficulty = 0;
        let Some(t) = find_static(w, false, 0.0) else {
            continue;
        };
        let level = structure::damage_level(w, t);
        let at = w.objects.get(t).unwrap().c.position;
        let s = boomed_strike(w, at);
        artillery_handle_static_collision(w, s);
        assert_eq!(damage(w, t), HITS * SHOT, "{mission}: three shots of 15");
        assert_eq!(
            w.objects.get(t).unwrap().c.is_destroyed(),
            HITS * SHOT >= level,
            "{mission}: destroyed exactly when 45 reaches the level {level}"
        );
        return;
    }
    panic!("no shipped mission has an intact turret");
}
