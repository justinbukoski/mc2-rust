// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `ArtilleryType::handleCollision`: the damage a boomed strike delivers (artlry.cpp:532-617).

mod common;

use common::fire_dump::*;
use mc2_sim::combat::artillery::{create_artillery, handle_collision};
use mc2_sim::combat::artillery_math::ARTILLERY_LARGE;
use mc2_sim::combat::loadout_of;
use mc2_sim::object::Kind;
use mc2_sim_data::objtype::ObjectTypeData;

#[test]
fn artillery_boom_damages_a_mech_in_range() {
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.difficulty = 0;
    let mech = find(&fx, 706);
    // thick armor everywhere: a hit that breaks through would transfer to the next location and count as a second handled hit
    for a in &mut mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(mech).unwrap())
        .unwrap()
        .armor
    {
        a.0 = 1000.0;
    }
    let at = fx.w.objects.get(mech).unwrap().c.position;
    let strike = create_artillery(&mut fx.w, ARTILLERY_LARGE, at).expect("a free strike slot");
    let hits = match &fx.w.objects.type_of(strike).unwrap().data {
        ObjectTypeData::Artillery(a) => {
            assert!(a.nominal_damage > 0.0);
            a.nominal_major_hits as i32 // trunc(major hits): the strike sits on the mech, so it is a major hit
        }
        _ => panic!(),
    };
    let handled = |fx: &Fixture| {
        loadout_of(fx.w.objects.get(mech).unwrap())
            .unwrap()
            .stats
            .num_weapon_hits_handled
    };
    // before the boom nothing happens and nothing is drawn
    let (armor, n, d) = (total_armor(&fx, mech), handled(&fx), draws(&fx));
    assert!(!handle_collision(&mut fx.w, strike, mech));
    assert_eq!(
        (total_armor(&fx, mech), handled(&fx), draws(&fx)),
        (armor, n, d)
    );
    // boomed: `hits` hits, each through handleWeaponHit
    let Kind::Artillery(a) = &mut fx.w.objects.get_mut(strike).unwrap().kind else {
        panic!()
    };
    a.boom = true;
    assert!(!handle_collision(&mut fx.w, strike, mech));
    assert_eq!(handled(&fx) - n, hits);
    assert!(total_armor(&fx, mech) < armor);
}

/// Per-location outcome of a boomed strike on a mech (artlry.cpp:578-607), derived independently of the port's damage code:
/// the hit count from the nominal major/minor range test, each hit's armor location from a replay of the C++ draw
/// (`calcHitLocation`: the `MechHitLocationTable` row of `MechHitSectionTable[source]` and the arc of `relFacingTo(strike)`,
/// one `RandomNumber(100)` per hit), and each hit's damage = `nominalDamage` (unscaled: the artillery class is not difficulty-scaled).
/// Thick armor keeps every hit on its own location, so the armor delta of each location is exactly (hits there) x damage.
/// Tolerance: exact on counts and locations; damage within 1e-3 (float summation order).
/// Covers the head-range path (DFA source), the major path and the minor path.
#[test]
fn artillery_boom_applies_per_location_damage_by_range_class() {
    use mc2_sim::combat::hit::MECH_HIT_SECTION_TABLE;
    use mc2_sim::combat::AttackSource;
    use mc2_sim::math::{rel_facing_to, Vec3};
    use mc2_sim::rng::SimRng;
    use mc2_sim::terrain::METERS_PER_WORLD_UNIT;

    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.difficulty = 0;
    let mech = find(&fx, 706);
    let probe = {
        let at = fx.w.objects.get(mech).unwrap().c.position;
        create_artillery(&mut fx.w, ARTILLERY_LARGE, at).expect("a free strike slot")
    };
    let (major_range, major_hits, minor_hits, damage, head_range) =
        match &fx.w.objects.type_of(probe).unwrap().data {
            ObjectTypeData::Artillery(a) => (
                a.nominal_major_range,
                a.nominal_major_hits as i32,
                a.nominal_minor_hits as i32,
                a.nominal_damage,
                a.explosions
                    .as_ref()
                    .map_or(5, |e| e.min_artillery_head_range) as f32,
            ),
            _ => panic!(),
        };
    let mut ranges = Vec::from([(0.0f32, major_hits, AttackSource::Dfa)]);
    if major_range > head_range + 1.0 {
        ranges.push((
            (head_range + major_range) / 2.0,
            major_hits,
            AttackSource::Artillery,
        ));
    }
    ranges.push((major_range + 1.0, minor_hits, AttackSource::Artillery));
    let section_cells = 11 * 4;
    for (meters, want_hits, source) in ranges {
        if want_hits == 0 {
            continue;
        }
        for a in &mut mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(mech).unwrap())
            .unwrap()
            .armor
        {
            a.0 = 1000.0;
        }
        let base = fx.w.objects.get(mech).unwrap().c.position;
        let at = Vec3 {
            x: base.x + meters / METERS_PER_WORLD_UNIT,
            ..base
        };
        let strike = create_artillery(&mut fx.w, ARTILLERY_LARGE, at).expect("a free strike slot");
        let Kind::Artillery(a) = &mut fx.w.objects.get_mut(strike).unwrap().kind else {
            panic!()
        };
        a.boom = true;
        let before: Vec<f32> = loadout_of(fx.w.objects.get(mech).unwrap())
            .unwrap()
            .armor
            .iter()
            .map(|a| a.0)
            .collect();
        // Independent replay of the location draws.
        let mode = fx.w.rng.0.borrow().sim.mode();
        let seed = 12345u32;
        fx.w.rng.0.borrow_mut().sim.seed(seed);
        let (pos, rot) = {
            let o = fx.w.objects.get(mech).unwrap();
            (o.c.position, o.c.rotation)
        };
        let entry = rel_facing_to(&pos, rot, &at);
        let arc = if (-45.0..=45.0).contains(&entry) {
            0
        } else if entry > -135.0 && entry < -45.0 {
            2
        } else if entry > 45.0 && entry < 135.0 {
            3
        } else {
            1
        };
        let table =
            fx.w.data
                .gamesys
                .tables
                .as_ref()
                .unwrap()
                .mech
                .hit_location_table
                .clone();
        let start = MECH_HIT_SECTION_TABLE[source as usize] * section_cells + arc * 11;
        let mut replay = SimRng::new(mode, seed);
        let mut want = [0.0f32; 11];
        // `WeaponShotInfo::init` (gameobj.cpp:157-170) scales only shots whose attacker is a non-player mech, vehicle or
        // turret (or any home-commander attacker); a strike of another commander is unscaled.
        assert_ne!(
            fx.w.objects.get(strike).unwrap().commander_id(),
            mc2_sim::combat::HOME_COMMANDER,
            "the strike is not the player's, so its damage is unscaled"
        );
        let per_hit = damage;
        for _ in 0..want_hits {
            let mut roll = replay.random_number(100);
            let mut loc = 0usize;
            while loc < 11 {
                let cell = i32::from(table[start + loc]);
                if roll < cell {
                    break;
                }
                roll -= cell;
                loc += 1;
            }
            want[loc] += per_hit;
        }
        assert!(!handle_collision(&mut fx.w, strike, mech));
        let after: Vec<f32> = loadout_of(fx.w.objects.get(mech).unwrap())
            .unwrap()
            .armor
            .iter()
            .map(|a| a.0)
            .collect();
        for l in 0..11 {
            assert!(
                ((before[l] - after[l]) - want[l]).abs() < 1e-3,
                "range {meters} m {source:?}: location {l} lost {} want {}",
                before[l] - after[l],
                want[l]
            );
        }
        // Armor held: no internal damage, the mech is still in the same status.
        assert_eq!(
            fx.w.objects.get(mech).unwrap().c.status,
            mc2_sim::object::ObjectStatus::Normal
        );
    }
}
