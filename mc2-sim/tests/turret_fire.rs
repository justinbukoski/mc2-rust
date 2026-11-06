// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Turret::fireWeapon` (turret.cpp:1356-1830): one hit roll, then a bolt.

mod common;

use mc2_sim::combat::turret::fire;
use mc2_sim::combat::turret_math::turret_attack_chance;
use mc2_sim::combat::weapon_bolt::bolt_state;
use mc2_sim::object::{Kind, ObjectClass};
use mc2_sim::rng::SimRng;
use mc2_sim_data::objtype::ObjectTypeData;

#[test]
fn turret_fire_consumes_one_hit_roll_and_creates_a_bolt() {
    // the first shipped mission that has a live turret with a weapon (mc2_01 has none)
    let mut found = None;
    for mission in ["mc2_05", "mc2_17", "mc2_03", "mc2_08", "mc2_10"] {
        let Some(fx) = common::fixture(mission, "msvc") else {
            return;
        };
        let has = fx.world.objects.turrets.iter().any(|&h| {
            fx.world.objects.get(h).is_some_and(|o| o.exists())
                && matches!(&fx.world.objects.type_of(h).map(|t| &t.data),
                    Some(ObjectTypeData::Turret(t)) if t.weapon_master_id[0] != -1)
        });
        if has {
            found = Some(fx);
            break;
        }
    }
    let mut fx = found.expect("a shipped mission with a turret that has a weapon");
    let w = &mut fx.world;
    // a turret with a weapon, and a mech to shoot at, east of it in the middle of the weapon's range band
    let (turret, weapon_master, pilot_skill) = w
        .objects
        .turrets
        .iter()
        .copied()
        .find_map(|h| {
            let o = w.objects.get(h)?;
            if !o.exists() {
                return None;
            }
            match &w.objects.type_of(h)?.data {
                ObjectTypeData::Turret(t) if t.weapon_master_id[0] != -1 => {
                    Some((h, t.weapon_master_id[0], t.pilot_skill as f32))
                }
                _ => None,
            }
        })
        .expect("mc2_01 has a turret with a weapon");
    let target = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| w.objects.get(h).unwrap().c.class == ObjectClass::BattleMech)
        .expect("a mech");
    let tpos = w.objects.get(turret).unwrap().c.position;
    // the middle of the weapon's range band, in world units
    let class = w
        .data
        .components
        .as_ref()
        .unwrap()
        .get(weapon_master)
        .and_then(|c| c.weapon())
        .and_then(|ws| ws.range)
        .map_or(0, |r| r.index());
    let band = w
        .data
        .gamesys
        .tables
        .as_ref()
        .unwrap()
        .general
        .weapon_ranges[class];
    // the nearest distance inside the weapon's band (in meters, then world units) from which the turret sees the mech
    let mut placed = false;
    // (any bearing: the line of sight is source-faithful now, so ridges and forest block some directions)
    'search: for bearing in 0..16 {
        let ang = bearing as f32 * std::f32::consts::TAU / 16.0;
        for step in 1..60 {
            let meters = band[0] + 0.5 + step as f32 * 2.0;
            if meters >= band[1] {
                break;
            }
            // the mech stands on the ground there, and its appearance follows it (as every mover update does), so that its line
            // of sight position is its own weapon node and not a stale one
            let reach = meters / mc2_sim::terrain::METERS_PER_WORLD_UNIT;
            let mut p = mc2_sim::math::Vec3 {
                x: tpos.x + reach * ang.cos(),
                y: tpos.y + reach * ang.sin(),
                z: 0.0,
            };
            p.z = w.terrain.elevation(&p);
            let o = w.objects.get_mut(target).unwrap();
            o.c.position = p;
            if let Kind::Mech(m) = &mut o.kind {
                m.m.velocity = mc2_sim::math::Vec3::default();
            }
            mc2_sim::object::hooks::set_object_parameters(w, target);
            mc2_sim::appearance::update(w, target, false);
            let r = mc2_sim::object::hooks::appear_radius(w, turret);
            w.objects.mover_los_table.iter_mut().for_each(|v| *v = -1);
            if mc2_sim::object::hooks::line_of_sight(w, turret, target, r) {
                placed = true;
                break 'search;
            }
        }
    }
    assert!(
        placed,
        "no distance inside the weapon's band with a line of sight"
    );
    let mode = w.rng.0.borrow().sim.mode();
    for want_hit in [true, false] {
        // the chance the turret has against a standing mech is skill + 50, against a moving one the skill alone (no specialist pilot):
        // the hit shot is fired at a standing target, the missed one at a moving target
        {
            let Kind::Mech(m) = &mut w.objects.get_mut(target).unwrap().kind else {
                panic!()
            };
            m.m.velocity = mc2_sim::math::Vec3 {
                x: if want_hit { 0.0 } else { 1.0 },
                y: 0.0,
                z: 0.0,
            };
        }
        let chance = if want_hit {
            pilot_skill + 50.0
        } else {
            pilot_skill
        } as i64;
        // find a seed whose first RandomNumber(100) lands on the wanted side
        let seed = (1..500u32)
            .find(|&s| {
                let r = i64::from(SimRng::new(mode, s).random_number(100));
                (r < chance) == want_hit
            })
            .expect("a seed");
        w.rng.0.borrow_mut().sim.seed(seed);
        let weapons_before = w
            .objects
            .weapons
            .iter()
            .filter(|&&b| w.objects.get(b).unwrap().exists())
            .count();
        let ready_before = match &w.objects.get(turret).unwrap().kind {
            Kind::Turret(t) => t.ready_time[0],
            _ => panic!(),
        };
        fire(w, turret, target, 0);
        let drawn = w.rng.0.borrow().sim.draws();
        let bolts: Vec<_> = w
            .objects
            .weapons
            .iter()
            .copied()
            .filter(|&b| w.objects.get(b).unwrap().exists())
            .collect();
        let ready_after = match &w.objects.get(turret).unwrap().kind {
            Kind::Turret(t) => t.ready_time[0],
            _ => panic!(),
        };
        if drawn == 0 {
            // the turret could not shoot (no weapon node in view, no line of sight): the test needs the fire path
            panic!("the turret did not fire: no weapon node or no line of sight in this fixture");
        }
        let recycle = w
            .data
            .components
            .as_ref()
            .unwrap()
            .get(weapon_master)
            .and_then(|c| c.weapon())
            .map(|ws| ws.recycle_time)
            .unwrap();
        assert_eq!(
            ready_after,
            w.clock.scenario_time + recycle,
            "the weapon recycle started"
        );
        let _ = ready_before;
        assert_eq!(bolts.len(), weapons_before + 1, "one bolt");
        let b = bolt_state(w, *bolts.last().unwrap()).unwrap();
        let shot = b.weapon_shot.expect("connected");
        assert_eq!(shot.master_id, weapon_master);
        if want_hit {
            // the hit roll, the hit location draws, then the connect dice (one for a mech target)
            assert!(drawn >= 2, "roll + connect dice at least, got {drawn}");
            assert!(shot.hit_location >= 0);
            assert!(!b.target.is_none());
        } else {
            assert_eq!(shot.hit_location, -1, "a miss carries no hit location");
        }
        let _ = turret_attack_chance;
    }
}
