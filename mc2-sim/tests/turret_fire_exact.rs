// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Exact, hand-derived numbers for one `Turret::fireWeapon` shot of a non-missile weapon (turret.cpp:1356-1830): the sim
//! draws it consumes, the bolt it creates and the recycle it starts. `tests/turret_fire.rs` only checks loose bounds.
//! Every expected value here is derived from the C++ cited beside it; the fixture search and placement follow
//! `tests/turret_fire.rs`.
//!
//! The draw chain, from the C++:
//! * `hitRoll = RandomNumber(100)` (turret.cpp:1409) is the first draw, on both paths. `startWeaponRecycle` (turret.cpp:1421,
//!   body at 1088-1091) and `calcAttackChance` draw nothing.
//! * Hit (`hitRoll < attackChance`, turret.cpp:1423), non-missile branch (turret.cpp:1530-1594):
//!   - `calcHitLocation(this, ..., ATTACKSOURCE_WEAPONFIRE)` (turret.cpp:1538). The attacker is non-NULL, so the arc comes
//!     from `relFacingTo` with no draw (mech.cpp:6684-6687). `locationRoll = RandomNumber(100)` (mech.cpp:6715) is the
//!     second draw. `MechHitSectionTable[ATTACKSOURCE_WEAPONFIRE]` is MECH_HIT_SECTION_MIDDLE (mech.cpp:210, mech.h:75).
//!   - `WeaponShotInfo::init` (gameobj.cpp:139) draws nothing.
//!   - `weaponFX->connect(this, target, ...)` (turret.cpp:1594, weaponbolt.h:304-336): a mech target draws
//!     `RollDice(50)` (weaponbolt.h:315), the third draw. `weaponShot = *shotInfo` (weaponbolt.h:307) draws nothing.
//!   - `WeaponBolt::update` is not run by `fireWeapon`, so the bolt-impact dice (weaponbolt.cpp:754, 817) are not drawn here.
//!     Total: EXACTLY THREE draws.
//! * Miss (turret.cpp:1599 and the non-missile miss branch at 1724-1814): NO further draws. The miss point is found by a
//!   deterministic line-of-sight walk over four candidates (turret.cpp:1747-1770), with no RNG. The `RandomNumber(
//!   positionOffset*2)` jitter at turret.cpp:1963-1965 is in `Turret::handleWeaponFire` (turret.cpp:1828), not in
//!   `fireWeapon`, so it does not apply here. Total: EXACTLY ONE draw for a point connect; a miss with no
//!   candidate in LOS connects to the target mech and adds connect()'s `RollDice(50)` (weaponbolt.h:315): two.
//!
//! Damage: `WeaponShotInfo::init` (gameobj.cpp:139-169) scales the damage by `applyDifficultyWeapon(dmg, isPlayer)` for a
//! non-player TURRET attacker (mission.cpp:857, the enemy table). `operator=` (gameobj.h:99-101) calls `init` again on the
//! copy, so the connect copy (weaponbolt.h:307 for a target, :346 for a point) scales it a second time. A miss outside
//! LOS re-inits the shot with 0.0 (turret.cpp:1778-1782), so it stays 0.
//!
//! Not derivable from `fireWeapon`: `WeaponBolt::hitTarget` (weaponbolt.h, set at weaponbolt.cpp:611 and 873 in `update`)
//! stays false after the shot on both paths, because `connect` never sets it. The hit is recorded by the target watch id
//! and the hit location instead.

mod common;

use mc2_sim::combat::turret::fire;
use mc2_sim::combat::turret_math::LosOutcome;
use mc2_sim::combat::weapon_bolt::bolt_state;
use mc2_sim::ids::{ObjectHandle, WatchId};
use mc2_sim::math::{rel_facing_to, Vec3};
use mc2_sim::object::{hooks, Kind, ObjectClass};
use mc2_sim::rng::{RngMode, SimRng};
use mc2_sim::terrain::METERS_PER_WORLD_UNIT;
use mc2_sim::World;
use mc2_sim_data::component::ComponentForm;
use mc2_sim_data::objtype::ObjectTypeData;

/// `NUM_MECH_ARMOR_LOCATIONS` and `NUM_MECH_HIT_ARCS` (mech.h:173, mech.h:67).
const NUM_MECH_ARMOR_LOCATIONS: usize = 11;
const NUM_MECH_HIT_ARCS: usize = 4;
/// `MECH_HIT_SECTION_MIDDLE` (mech.h:75): `MechHitSectionTable[ATTACKSOURCE_WEAPONFIRE]` (mech.cpp:210).
const MECH_HIT_SECTION_MIDDLE: usize = 1;
/// `MECH_HIT_ARC_FRONT/REAR/LEFT/RIGHT` (mech.h:68-71).
const ARC_FRONT: usize = 0;
const ARC_REAR: usize = 1;
const ARC_LEFT: usize = 2;
const ARC_RIGHT: usize = 3;
/// `missRadius = target ? 25.0 : 5.0` (turret.cpp:1744); the target is always present here.
const MISS_RADIUS: f32 = 25.0;

/// The arc of `BattleMech::calcHitLocation` (mech.cpp:6689-6697), from the entry angle.
fn hit_arc(entry_angle: f32) -> usize {
    if (-45.0..=45.0).contains(&entry_angle) {
        ARC_FRONT
    } else if entry_angle > -135.0 && entry_angle < -45.0 {
        ARC_LEFT
    } else if entry_angle > 45.0 && entry_angle < 135.0 {
        ARC_RIGHT
    } else {
        ARC_REAR
    }
}

/// The `MechHitLocationTable` walk of `calcHitLocation` (mech.cpp:6716-6722) for one `locationRoll`.
fn table_location(table: &[i8], start_entry: usize, mut location_roll: i32) -> i32 {
    let mut armor_location = 0usize;
    while armor_location < NUM_MECH_ARMOR_LOCATIONS {
        let cell = i32::from(table[start_entry + armor_location]);
        if location_roll < cell {
            break;
        }
        location_roll -= cell;
        armor_location += 1;
    }
    armor_location as i32
}

/// `applyDifficultyWeapon(dmg, false)` (mission.cpp:857): the enemy table scales the damage by `table[d] / 100` (a
/// double, narrowed back to float) for difficulty `d` in 0..4; any other difficulty leaves it alone.
fn enemy_scaled(w: &World, dmg: f32) -> f32 {
    let t = w.data.gamesys.tables.as_ref().expect("tables");
    let d = w.mission.difficulty;
    if (0..4).contains(&d) {
        (f64::from(dmg) * (f64::from(t.difficulty.enemy_weapons[d as usize] as f32) / 100.0)) as f32
    } else {
        dmg
    }
}

/// The damage a non-missile turret bolt carries after `init` and the `operator=` copy in `connect`: scaled twice.
fn bolt_damage(w: &World, raw: f32) -> f32 {
    enemy_scaled(w, enemy_scaled(w, raw))
}

/// The weapon 0 master id of `turret` and the weapon's range band (meters): `WeaponRanges[getWeaponRange()]`, the weapon's own range class
/// (turret.cpp:1404 reads the band of the weapon, not of the target's object class).
fn weapon_band(w: &World, turret: ObjectHandle, _target: ObjectHandle) -> (i32, [f32; 2]) {
    let ObjectTypeData::Turret(t) = &w.objects.type_of(turret).unwrap().data else {
        unreachable!("the search only returns turrets")
    };
    let master = t.weapon_master_id[0];
    let comps = w.data.components.as_ref().expect("components");
    let class = comps
        .get(master)
        .and_then(|c| c.weapon())
        .and_then(|ws| ws.range)
        .map_or(0, |r| r.index());
    let tables = w.data.gamesys.tables.as_ref().expect("tables");
    (master, tables.general.weapon_ranges[class])
}

/// Put `target` at the nearest distance inside the weapon's band from which `turret` sees it, with the velocity the
/// attack chance reads (zero for a standing target, 1.0 along x for a moving one). Mirrors `tests/turret_fire.rs`.
fn place(w: &mut World, turret: ObjectHandle, target: ObjectHandle, moving: bool) -> bool {
    let (_, band) = weapon_band(w, turret, target);
    let tpos = w.objects.get(turret).unwrap().c.position;
    let radius = hooks::appear_radius(w, turret);
    for step in 1..60 {
        let meters = band[0] + 0.5 + step as f32 * 2.0;
        if meters >= band[1] {
            break;
        }
        {
            let o = w.objects.get_mut(target).unwrap();
            o.c.position = Vec3 {
                x: tpos.x + meters / METERS_PER_WORLD_UNIT,
                y: tpos.y,
                z: tpos.z,
            };
            if let Kind::Mech(m) = &mut o.kind {
                m.m.velocity = Vec3 {
                    x: if moving { 1.0 } else { 0.0 },
                    y: 0.0,
                    z: 0.0,
                };
            }
        }
        if hooks::line_of_sight(w, turret, target, radius) {
            return true;
        }
    }
    false
}

/// The search of `tests/turret_fire.rs`, restricted to a turret whose weapon 0 is NOT a missile: the single-bolt path
/// (turret.cpp:1530-1594 and 1724-1814) is the one this test derives.
fn find_scenario() -> Option<(World, ObjectHandle, ObjectHandle)> {
    for mission in ["mc2_05", "mc2_17", "mc2_03", "mc2_08", "mc2_10"] {
        let Some(fx) = common::fixture(mission, "msvc") else {
            continue;
        };
        let mut w = fx.world;
        let turrets: Vec<ObjectHandle> = w
            .objects
            .turrets
            .iter()
            .copied()
            .filter(|&h| {
                let Some(ObjectTypeData::Turret(t)) = w.objects.type_of(h).map(|t| &t.data) else {
                    return false;
                };
                let master = t.weapon_master_id[0];
                master != -1
                    && w.data
                        .components
                        .as_ref()
                        .and_then(|c| c.get(master))
                        .is_some_and(|m| m.form != Some(ComponentForm::WeaponMissile))
            })
            .collect();
        let targets: Vec<ObjectHandle> = w
            .objects
            .movers
            .iter()
            .copied()
            .filter(|&h| w.objects.get(h).unwrap().c.class == ObjectClass::BattleMech)
            .collect();
        for &turret in &turrets {
            for &target in &targets {
                if place(&mut w, turret, target, false) {
                    return Some((w, turret, target));
                }
            }
        }
    }
    None
}

/// `Turret::calcAttackChance` (turret.cpp:1141) as the seed search uses it: the pilot skill, +50 for a standing target,
/// clamped to >= 5 (the form `tests/turret_rng_exact.rs` uses). A mission mech carries no turret-relevant specialty, so
/// no specialty bonus applies.
fn attack_chance(w: &World, turret: ObjectHandle, moving: bool) -> i64 {
    let ObjectTypeData::Turret(t) = &w.objects.type_of(turret).unwrap().data else {
        unreachable!()
    };
    let mut chance = t.pilot_skill as f32;
    if !moving {
        chance += 50.0;
    }
    if chance < 5.0 {
        chance = 5.0;
    }
    chance as i64
}

/// The four miss candidates in the order `fireWeapon` tries them (turret.cpp:1747-1770), with the same float steps:
/// `positionOffset` starts at the target position, then `x += r`, `x -= 2r`, `x += r; y += r`, `y -= 2r`.
fn miss_candidates(base: Vec3) -> [(f32, f32); 4] {
    let r = MISS_RADIUS;
    let x1 = base.x + r;
    let x2 = x1 - r * 2.0;
    let x3 = x2 + r;
    let y3 = base.y + r;
    let y4 = y3 - r * 2.0;
    [(x1, base.y), (x2, base.y), (x3, y3), (x3, y4)]
}

#[test]
fn turret_fire_rng_and_bolt_match_cxx() {
    // Without the game data there is nothing to load (the repo's convention is to skip, see tests/common).
    if std::env::var_os("MC2_DATA")
        .filter(|v| !v.is_empty())
        .is_none()
    {
        eprintln!("MC2_DATA not set; skipping the turret fire derivation");
        return;
    }
    // With the data present, a missing turret/mech pair is a failure, not a silent pass.
    let (mut w, turret, target) = find_scenario()
        .expect("a shipped mission with a non-missile turret and a mech in its band with LOS");
    let mode = w.rng.0.borrow().sim.mode();
    assert_eq!(mode, RngMode::LegacyMsvc);
    let (weapon_master, _) = weapon_band(&w, turret, target);
    let (recycle, raw_damage, effect) = {
        let comps = w.data.components.as_ref().expect("components");
        let ws = comps
            .get(weapon_master)
            .and_then(|c| c.weapon())
            .expect("weapon stats");
        (
            ws.recycle_time,
            ws.damage,
            i32::from(ws.special_effect as u8),
        )
    };
    let target_wid = w.objects.watch_id(target);

    for want_hit in [true, false] {
        // The hit shot is fired at a standing target (chance skill + 50); the missed one at a moving target (chance skill).
        assert!(
            place(&mut w, turret, target, !want_hit),
            "the placement found by the search must hold"
        );
        let chance = attack_chance(&w, turret, !want_hit);

        // A seed whose FIRST RandomNumber(100) lands on the wanted side of the chance (hitRoll < attackChance, turret.cpp:1423).
        let seed = (1..500u32)
            .find(|&s| (i64::from(SimRng::new(mode, s).random_number(100)) < chance) == want_hit)
            .expect("a seed whose first roll hits or misses as wanted");

        w.rng.0.borrow_mut().sim.seed(seed);
        let draws_before = w.rng.0.borrow().sim.draws();
        let weapons_before = w
            .objects
            .weapons
            .iter()
            .filter(|&&b| w.objects.get(b).unwrap().exists())
            .count();
        let scenario_time = w.clock.scenario_time;

        let outcome = fire(&mut w, turret, target, 0);
        assert_eq!(
            outcome,
            LosOutcome::Clear,
            "the shot must clear the sight check"
        );

        // --- the recycle: startWeaponRecycle on both paths (turret.cpp:1421, body at 1088-1091) ---
        let ready = match &w.objects.get(turret).unwrap().kind {
            Kind::Turret(ts) => ts.ready_time[0],
            _ => unreachable!(),
        };
        assert_eq!(
            ready,
            scenario_time + recycle,
            "readyTime[0] = scenarioTime + getWeaponRecycleTime()"
        );

        // --- the bolt: exactly one more, carrying the shot ---
        let bolts: Vec<ObjectHandle> = w
            .objects
            .weapons
            .iter()
            .copied()
            .filter(|&b| w.objects.get(b).unwrap().exists())
            .collect();
        assert_eq!(
            bolts.len(),
            weapons_before + 1,
            "a non-missile shot spawns one bolt"
        );
        let bolt = *bolts.last().unwrap();
        let b = bolt_state(&w, bolt).expect("bolt state").clone();
        let shot = b.weapon_shot.expect("the bolt carries the shot");
        assert_eq!(shot.master_id, weapon_master);
        assert_eq!(b.effect_id, effect);
        // hitTarget is set by WeaponBolt::update only (weaponbolt.cpp:611, 873), which fireWeapon never runs.
        assert!(!b.hit_target, "connect() leaves hitTarget false");

        // --- the exact RNG stream, replayed from the seed in the C++ draw order ---
        let mut replay = SimRng::new(mode, seed);
        let hit_roll = replay.random_number(100);
        assert_eq!(i64::from(hit_roll) < chance, want_hit);
        let tpos = w.objects.get(turret).unwrap().c.position;
        let (tgt_pos, tgt_rot) = {
            let o = w.objects.get(target).unwrap();
            (o.c.position, o.c.rotation)
        };
        // fireWeapon's entryAngle is `target->relFacingTo(position)` (turret.cpp:1398), the shot's entry angle.
        let entry_angle = rel_facing_to(&tgt_pos, tgt_rot, &tpos);
        assert_eq!(
            shot.entry_angle, entry_angle,
            "entryAngle = target->relFacingTo(turret)"
        );

        if want_hit {
            // Draw 2: calcHitLocation's locationRoll (mech.cpp:6715); the arc needs no draw (mech.cpp:6684-6687).
            let location_roll = replay.random_number(100);
            let tables = w.data.gamesys.tables.as_ref().expect("tables");
            let start_entry = MECH_HIT_SECTION_MIDDLE
                * (NUM_MECH_ARMOR_LOCATIONS * NUM_MECH_HIT_ARCS)
                + hit_arc(entry_angle) * NUM_MECH_ARMOR_LOCATIONS;
            let want_location =
                table_location(&tables.mech.hit_location_table, start_entry, location_roll);
            assert_eq!(
                shot.hit_location, want_location,
                "hit location from locationRoll {location_roll}"
            );
            assert!(shot.hit_location >= 0);

            // Draw 3: connect()'s RollDice(50) picks the mech hot spot (weaponbolt.h:315).
            let hot = i32::from(replay.roll_dice(50));
            assert_eq!(b.hot_spots.1, hot, "targetHotSpot = RollDice(50)");

            // connect(dest) (weaponbolt.h:304) stores the target WID and no target point: the bolt tracks the mech.
            assert_eq!(b.target, target_wid, "a hit bolt keeps the target watch id");
            assert!(!b.has_target_position, "a hit bolt has no target point");

            assert_eq!(
                shot.damage,
                bolt_damage(&w, raw_damage),
                "damage = applyDifficultyWeapon applied twice to the weapon damage"
            );
        } else {
            // A miss: no calcHitLocation, so the hit location is -1 (turret.cpp:1727-1733, 1778-1782).
            assert_eq!(shot.hit_location, -1, "a miss carries no hit location");
            // The point connect (weaponbolt.h:345) clears the target WID and stores the target point; or, when no
            // candidate is in LOS, the target connect (turret.cpp:1811) keeps the WID with ZERO damage. Which candidate
            // is first in LOS is a visibility query (Team::lineOfSight) and is not re-derived here.
            if b.has_target_position {
                let candidates = miss_candidates(tgt_pos);
                assert!(
                    candidates
                        .iter()
                        .any(|&(x, y)| b.target_position.x == x && b.target_position.y == y),
                    "the miss point is one of the four derived candidates, got {:?}",
                    b.target_position
                );
                assert_eq!(b.target, WatchId::NONE, "a point connect has no target WID");
                assert_eq!(
                    shot.damage,
                    bolt_damage(&w, raw_damage),
                    "an in-LOS miss keeps the damage"
                );
            } else {
                assert_eq!(
                    b.target, target_wid,
                    "no candidate in LOS: the bolt connects to the target"
                );
                assert_eq!(
                    shot.damage, 0.0,
                    "no candidate in LOS: zero damage (turret.cpp:1778-1782)"
                );
            }
        }

        // --- the draw count and the stream position ---
        // hit = hitRoll + locationRoll + RollDice(50) = 3 draws; miss with a point connect = hitRoll only = 1 draw (no LOS-walk draws).
        // A miss with no candidate in LOS connects to the target mech (turret.cpp:1811), and connect() draws RollDice(50)
        // for the hot spot (weaponbolt.h:315): a second draw.
        let target_connect_miss = !want_hit && !b.has_target_position;
        if target_connect_miss {
            let hot = i32::from(replay.roll_dice(50));
            assert_eq!(b.hot_spots.1, hot, "targetHotSpot = RollDice(50)");
        }
        let expected_draws = if want_hit {
            3
        } else if target_connect_miss {
            2
        } else {
            1
        };
        assert_eq!(
            w.rng.0.borrow().sim.draws() - draws_before,
            expected_draws,
            "exact draws for a {} shot",
            if want_hit { "hit" } else { "miss" }
        );
        assert_eq!(w.rng.0.borrow().sim.draws(), replay.draws());
        assert_eq!(w.rng.0.borrow().sim.state_hash(), replay.state_hash());
    }
}
