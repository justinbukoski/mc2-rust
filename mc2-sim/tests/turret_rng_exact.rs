// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The exact RNG draw order of `Turret::fireWeapon` (turret.cpp:1392-1470), compared against an independent replay.
//!
//! `tests/turret_fire.rs` asserts the *selected* behaviour (a bolt exists, the recycle moved). This test pins the
//! *exact* numbers: the seed is searched with an independent `SimRng` replay, the fire is then driven at that seed,
//! and every sim draw the shot consumed is replayed by hand from the C++ order:
//!
//! * `calcAttackChance` (turret.cpp:1404) consumes no RNG.
//! * `hitRoll = RandomNumber(100)` (turret.cpp:1408) is the first draw.
//! * `startWeaponRecycle(weaponId)` (turret.cpp:1419, body at 1088-1091) runs on BOTH paths and consumes no RNG:
//!   `readyTime[weaponId] = scenarioTime + getWeaponRecycleTime()`.
//! * A miss consumes nothing else: `recordWeaponFireTime` (turret.cpp:1081) only stores `lastFireTime` on the hit
//!   path, the miss branch rolls no dice, and `WeaponBolt::connect(source, targetLoc, ...)` (weaponbolt.h:344, the
//!   point version used for a miss inside LOS) draws nothing. So the miss shot consumes EXACTLY ONE sim draw.
//! * A hit draws `BattleMech::calcHitLocation` (mech.cpp:6669, the ATTACKSOURCE_WEAPONFIRE branch: the section is
//!   `MechHitSectionTable[0]` = MIDDLE, the arc comes from `relFacingTo(attacker->getPosition())` with no draw, then
//!   one `RandomNumber(100)` walks the `MechHitLocationTable` row) and then `connect(source, dest, ...)`
//!   (weaponbolt.h:304) draws `RollDice(50)` for a mech target. `WeaponShotInfo::init` (gameobj.cpp:139) and the
//!   connect tail draw nothing, and the bolt-connect *impact* dice (`weaponbolt.cpp:754` head shot, 817 crater)
//!   belong to `WeaponBolt::update`, which the fire call never runs — so a mech-target hit consumes EXACTLY TWO
//!   sim draws after the hit roll, and the replay reproduces the hit location and the target hot spot from them.

mod common;

use mc2_sim::ai::sensors;
use mc2_sim::combat::attack::distance_from;
use mc2_sim::combat::fire_math::{miss_offset_points, miss_radius};
use mc2_sim::combat::hit::MECH_HIT_SECTION_TABLE;
use mc2_sim::combat::shot::apply_difficulty_weapon;
use mc2_sim::combat::turret::fire;
use mc2_sim::combat::turret_math::LosOutcome;
use mc2_sim::combat::weapon_bolt::{bolt_params, bolt_state};
use mc2_sim::combat::AttackSource;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::{rel_facing_to, Vec3};
use mc2_sim::object::hooks;
use mc2_sim::object::{Kind, ObjectClass};
use mc2_sim::rng::{RngMode, SimRng};
use mc2_sim::terrain::METERS_PER_WORLD_UNIT;
use mc2_sim::World;
use mc2_sim_data::component::ComponentForm;
use mc2_sim_data::objtype::ObjectTypeData;

/// `NUM_MECH_ARMOR_LOCATIONS` and `NUM_MECH_HIT_ARCS` (mech.h:67, mech.h:173).
const NUM_MECH_ARMOR_LOCATIONS: usize = 11;
const NUM_MECH_HIT_ARCS: usize = 4;

/// `MECH_HIT_ARC_*` (mech.h:68-71).
mod arc {
    pub const FRONT: usize = 0;
    pub const REAR: usize = 1;
    pub const LEFT: usize = 2;
    pub const RIGHT: usize = 3;
}

/// The arc of an entry angle, re-derived from `BattleMech::calcHitLocation` (mech.cpp:6689-6697): the boundaries are
/// inclusive for the front only. Cross-checked against the crate's own table below.
fn hit_arc(entry_angle: f32) -> usize {
    if (-45.0..=45.0).contains(&entry_angle) {
        arc::FRONT
    } else if entry_angle > -135.0 && entry_angle < -45.0 {
        arc::LEFT
    } else if entry_angle > 45.0 && entry_angle < 135.0 {
        arc::RIGHT
    } else {
        arc::REAR
    }
}

/// The `MechHitLocationTable` walk of `calcHitLocation` (mech.cpp:6716-6722) for one `locationRoll`.
fn table_location(table: &[i8], start_entry: usize, mut location_roll: i32) -> i32 {
    let mut armor_location = 0usize;
    while armor_location < NUM_MECH_ARMOR_LOCATIONS {
        let cell = i32::from(
            table
                .get(start_entry + armor_location)
                .copied()
                .unwrap_or(0),
        );
        if location_roll < cell {
            break;
        }
        location_roll -= cell;
        armor_location += 1;
    }
    armor_location as i32
}

/// Everything the test reads off the loaded world about the turret's weapon 0 and its range bands.
struct Scenario {
    mission: &'static str,
    turret: ObjectHandle,
    target: ObjectHandle,
    weapon_master: i32,
    pilot_skill: f32,
    /// `recycle_time` of the weapon master component (`getWeaponRecycleTime`).
    recycle: f32,
    /// Raw weapon damage (`getWeaponDamage`), before `WeaponShotInfo::init` scaling.
    damage: f32,
    /// `special_effect` of the master component (`getWeaponSpecialEffect`).
    effect: i32,
    /// The weapon's range band in meters (`WeaponRanges[class]`).
    band: (f32, f32),
}

/// The turret/target pair from the mission search pattern of `tests/turret_fire.rs`: the first mission, turret and
/// BattleMech where the turret has a non-missile weapon 0 (the single-bolt assertions below are about the "Non-missile
/// weapon, so just one weapon hit spawn" path, turret.cpp:1533) and the mech can be placed inside the weapon's band
/// with line of sight (the mech is parked far away first, so the placement loop starts from a known state).
fn find_scenario() -> Option<(World, Scenario)> {
    for mission in ["mc2_05", "mc2_17", "mc2_03", "mc2_08", "mc2_10"] {
        let Some(mut fx) = common::fixture(mission, "msvc") else {
            continue;
        };
        let turrets: Vec<ObjectHandle> = fx
            .world
            .objects
            .turrets
            .iter()
            .copied()
            .filter(|&h| {
                let Some(ObjectTypeData::Turret(t)) = fx.world.objects.type_of(h).map(|t| &t.data)
                else {
                    return false;
                };
                let master = t.weapon_master_id[0];
                if master == -1 {
                    return false;
                }
                fx.world
                    .data
                    .components
                    .as_ref()
                    .and_then(|c| c.get(master))
                    .is_some_and(|m| m.form != Some(ComponentForm::WeaponMissile))
            })
            .collect();
        let targets: Vec<ObjectHandle> = fx
            .world
            .objects
            .movers
            .iter()
            .copied()
            .filter(|&h| fx.world.objects.get(h).unwrap().c.class == ObjectClass::BattleMech)
            .collect();
        for turret in &turrets {
            for target in &targets {
                let tpos = fx.world.objects.get(*turret).unwrap().c.position;
                mc2_sim::object::set_position(
                    &mut fx.world,
                    *target,
                    Vec3 {
                        x: tpos.x + 4000.0,
                        y: tpos.y,
                        z: tpos.z,
                    },
                    true,
                );
                let sc = scenario(mission, &fx.world, *turret, *target);
                // The standing placement: the hit case needs LOS, so a pair without it is skipped.
                if place(&mut fx.world, &sc, false).is_some() {
                    return Some((fx.world, sc));
                }
            }
        }
    }
    None
}

fn scenario(
    mission: &'static str,
    w: &World,
    turret: ObjectHandle,
    target: ObjectHandle,
) -> Scenario {
    let tt = match &w.objects.type_of(turret).unwrap().data {
        ObjectTypeData::Turret(t) => t,
        _ => unreachable!("the search only returns turrets"),
    };
    let weapon_master = tt.weapon_master_id[0];
    let comps = w.data.components.as_ref().expect("components");
    let master = comps.get(weapon_master).expect("master component");
    let ws = master.weapon().expect("weapon stats");
    let class = w.objects.get(target).unwrap().c.class as usize;
    let tables = w.data.gamesys.tables.as_ref().expect("tables");
    let band = tables.general.weapon_ranges[class];
    Scenario {
        mission,
        turret,
        target,
        weapon_master,
        pilot_skill: tt.pilot_skill as f32,
        recycle: ws.recycle_time,
        damage: ws.damage,
        effect: i32::from(ws.special_effect as u8),
        band: (band[0], band[1]),
    }
}

/// Place `target` at a distance inside the band where the turret sees it; `moving` sets the velocity the attack
/// chance reads. Returns the placement distance in meters.
fn place(w: &mut World, sc: &Scenario, moving: bool) -> Option<f32> {
    let tpos = w.objects.get(sc.turret).unwrap().c.position;
    let radius = hooks::appear_radius(w, sc.turret);
    // Any bearing: the line of sight is source-faithful now, so ridges and forest block some directions. The mech stands on the
    // ground and its appearance follows it (as every mover update does), so its line of sight position is its own weapon node.
    for bearing in 0..16 {
        let ang = bearing as f32 * std::f32::consts::TAU / 16.0;
        for step in 1..80 {
            let meters = sc.band.0 + 0.5 + step as f32 * 2.0;
            if meters >= sc.band.1 {
                break;
            }
            let reach = meters / METERS_PER_WORLD_UNIT;
            let mut position = Vec3 {
                x: tpos.x + reach * ang.cos(),
                y: tpos.y + reach * ang.sin(),
                z: 0.0,
            };
            position.z = w.terrain.elevation(&position);
            mc2_sim::object::set_position(w, sc.target, position, true);
            {
                let o = w.objects.get_mut(sc.target).unwrap();
                if let Kind::Mech(m) = &mut o.kind {
                    m.m.velocity = if moving {
                        Vec3 {
                            x: 1.0,
                            y: 0.0,
                            z: 0.0,
                        }
                    } else {
                        Vec3::default()
                    };
                }
            }
            hooks::set_object_parameters(w, sc.target);
            mc2_sim::appearance::update(w, sc.target, false);
            w.objects.mover_los_table.iter_mut().for_each(|v| *v = -1);
            if hooks::line_of_sight(w, sc.turret, sc.target, radius) {
                return Some(meters);
            }
        }
    }
    None
}

/// The chance `Turret::calcAttackChance` (turret.cpp:1141) yields for the placed target: the pilot skill, +50 for a
/// standing target, then the mech clamp to >= 5. A mission-spawned mech has no turret-relevant specialty, so no
/// specialty branch fires and only the clamp can bite.
fn expected_chance(w: &World, sc: &Scenario, moving: bool) -> i64 {
    let tpos = w.objects.get(sc.turret).unwrap().c.position;
    let target = w.objects.get(sc.target).unwrap();
    let distance = distance_from(&tpos, &target.c.position);
    assert!(
        distance > sc.band.0 && distance <= sc.band.1,
        "{}: placed distance {distance} outside the weapon band {:?}",
        sc.mission,
        sc.band
    );
    let mut chance = sc.pilot_skill;
    if !moving {
        chance += 50.0;
    }
    if chance < 5.0 {
        chance = 5.0;
    }
    chance as i64
}

/// `turret_shot_rng_order_matches_fire_weapon`: the exact draw order, recycle value and bolt state of one turret
/// shot, replayed draw by draw from an independent `SimRng`.
#[test]
fn turret_shot_rng_order_matches_fire_weapon() {
    let Some((mut w, sc)) = find_scenario() else {
        return;
    };
    let mode = w.rng.0.borrow().sim.mode();
    assert_eq!(mode, RngMode::LegacyMsvc);

    // The replay's tables must be the crate's own, not a drifted copy.
    assert_eq!(MECH_HIT_SECTION_TABLE[AttackSource::WeaponFire as usize], 1);
    assert_eq!(hit_arc(0.0), arc::FRONT);
    assert_eq!(hit_arc(45.0), arc::FRONT);
    assert_eq!(hit_arc(45.01), arc::RIGHT);
    assert_eq!(hit_arc(-135.0), arc::REAR);

    for want_hit in [false, true] {
        // A moving target drops the +50 standing bonus; a standing target keeps it.
        // The search already found LOS for the standing case; the moving placement uses the same
        // positions, so a failure here is a fixture problem, not a skip.
        place(&mut w, &sc, !want_hit).expect("the placement found by the search must not vanish");
        let chance = expected_chance(&w, &sc, !want_hit);

        // A seed whose FIRST RandomNumber(100) lands on the wanted side of the chance.
        let seed = (1..500u32)
            .find(|&s| (i64::from(SimRng::new(mode, s).random_number(100)) < chance) == want_hit)
            .expect("a seed whose first roll hits or misses as wanted");
        let replay_roll = SimRng::new(mode, seed).random_number(100);
        assert_eq!(i64::from(replay_roll) < chance, want_hit);

        w.rng.0.borrow_mut().sim.seed(seed);
        let draws_before = w.rng.0.borrow().sim.draws();
        let weapons_before = w
            .objects
            .weapons
            .iter()
            .filter(|&&b| w.objects.get(b).unwrap().exists())
            .count();
        let scenario_time = w.clock.scenario_time;

        let outcome = fire(&mut w, sc.turret, sc.target, 0);
        assert_eq!(
            outcome,
            LosOutcome::Clear,
            "{}: the shot must proceed past the sight check",
            sc.mission
        );

        // --- recycle: startWeaponRecycle on BOTH paths, ready_time[0] = scenario time + recycle_time ---
        let Kind::Turret(ts) = &w.objects.get(sc.turret).unwrap().kind else {
            unreachable!()
        };
        assert_eq!(
            ts.ready_time[0],
            scenario_time + sc.recycle,
            "{}: ready_time[0] must be scenario_time + recycle_time ({})",
            sc.mission,
            sc.recycle
        );
        assert_eq!(
            ts.last_fire_time[0],
            if want_hit { scenario_time } else { 0.0 },
            "{}: recordWeaponFireTime stores scenarioTime on a hit only",
            sc.mission
        );

        // --- the bolt: exactly one more exists, and it carries the shot ---
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
            "{}: a non-missile shot spawns exactly one bolt",
            sc.mission
        );
        let bolt = *bolts.last().unwrap();
        let (shot, effect_id, target_wid, hot_spots, has_target_position, target_position) = {
            let b = bolt_state(&w, bolt).expect("bolt state");
            (
                b.weapon_shot.expect("the bolt carries the shot"),
                b.effect_id,
                b.target,
                b.hot_spots,
                b.has_target_position,
                b.target_position,
            )
        };
        assert_eq!(shot.master_id, sc.weapon_master);
        assert_eq!(effect_id, sc.effect);
        let _ = bolt_params(&w, bolt).expect("bolt params");

        // The C++ scales the damage twice: `shotInfo.init(...)` and then the copy assignment inside `connect`
        // (`weaponShot = *shotInfo` runs init again, gameobj.cpp:139). The turret is not the player.
        let scaled_once = apply_difficulty_weapon(&w, sc.damage, false);
        let scaled_twice = apply_difficulty_weapon(&w, scaled_once, false);

        // --- the exact RNG state: replay the C++ draw order from the seed ---
        let mut replay = SimRng::new(mode, seed);
        let hit_roll = replay.random_number(100);
        assert_eq!(i64::from(hit_roll), i64::from(replay_roll));

        // The entry angle the C++ reads twice: fireWeapon's `target->relFacingTo(position)` (the shot's entryAngle)
        // and calcHitLocation's `relFacingTo(attacker->getPosition())` (the arc). Both use the turret POSITION.
        let tpos = w.objects.get(sc.turret).unwrap().c.position;
        let t = w.objects.get(sc.target).unwrap();
        // A mover target measures it in the plane (`Mover::relFacingTo`, mover.cpp:2909; kill-parity fix 3).
        let entry_angle = if t.is_mover() {
            mc2_sim::ai::decide::rel_facing_to(&w, sc.target, &tpos, -1)
        } else {
            rel_facing_to(&t.c.position, t.c.rotation, &tpos)
        };
        assert_eq!(shot.entry_angle, entry_angle);

        if want_hit {
            // Draw 2: calcHitLocation's locationRoll (no draw for the arc: the attacker exists).
            let location_roll = replay.random_number(100);
            let section = MECH_HIT_SECTION_TABLE[AttackSource::WeaponFire as usize];
            let start_entry = section * (NUM_MECH_ARMOR_LOCATIONS * NUM_MECH_HIT_ARCS)
                + (hit_arc(entry_angle) * NUM_MECH_ARMOR_LOCATIONS);
            let table = w
                .data
                .gamesys
                .tables
                .as_ref()
                .expect("tables")
                .mech
                .hit_location_table
                .clone();
            let want_location = table_location(&table, start_entry, location_roll);
            assert_eq!(
                shot.hit_location, want_location,
                "{}: hit location from locationRoll {location_roll} at start entry {start_entry}",
                sc.mission
            );
            assert!(shot.hit_location >= 0);
            assert!(
                !target_wid.is_none(),
                "a hit bolt keeps the target watch id"
            );
            assert_eq!(shot.damage, scaled_twice);

            // Draw 3: connect()'s RollDice(50) picks the mech target hot spot 0 or 1.
            let hot = replay.roll_dice(50);
            assert_eq!(hot_spots.1, i32::from(hot));
            let Kind::Turret(ts) = &w.objects.get(sc.turret).unwrap().kind else {
                unreachable!()
            };
            assert_eq!(hot_spots.0, ts.current_weapon_node);

            // The sim stream must now sit exactly where the replay sits: same draws, same state.
            assert_eq!(
                w.rng.0.borrow().sim.draws() - draws_before,
                3,
                "a hit consumes the roll, the location roll and the connect dice"
            );
            assert_eq!(w.rng.0.borrow().sim.draws(), replay.draws());
            assert_eq!(w.rng.0.borrow().sim.state_hash(), replay.state_hash());
        } else {
            // A miss: no calcHitLocation. The point connect draws nothing; with no candidate in LOS the target connect
            // (turret.cpp:1811) draws connect()'s RollDice(50) for the mech hot spot (weaponbolt.h:315).
            assert_eq!(shot.hit_location, -1, "a miss carries no hit location");

            // The miss point: replay the C++ candidate walk (turret.cpp:1655-1700) and check the bolt aims at the
            // first candidate in LOS, or at the target with zero damage when none is.
            let radius = miss_radius(true);
            let base = w.objects.get(sc.target).unwrap().c.position;
            let mut offset = base;
            let mut can_see = false;
            for (x, y) in miss_offset_points(base, radius) {
                offset.x = x;
                offset.y = y;
                offset.z = w.terrain.elevation(&offset);
                can_see = {
                    let from = hooks::los_position(&mut w, sc.turret);
                    let team = w.objects.get(sc.turret).map(|o| o.team_id()).unwrap_or(-1);
                    w.terrain.is_game_select_position(&offset)
                        && sensors::line_of_sight_ray(&w, from, offset, team, 15.0, 1.0)
                };
                if can_see {
                    break;
                }
            }
            if can_see {
                assert!(target_wid.is_none(), "the point connect clears the target");
                assert_eq!(
                    w.rng.0.borrow().sim.draws() - draws_before,
                    1,
                    "a point-connect miss consumes EXACTLY the one hit roll"
                );
                assert_eq!(w.rng.0.borrow().sim.draws(), replay.draws());
                assert_eq!(w.rng.0.borrow().sim.state_hash(), replay.state_hash());
                assert!(has_target_position);
                assert_eq!(target_position, offset);
                assert_eq!(shot.damage, scaled_twice);
            } else {
                // Miss location NOT in LOS: the shot connects to the target with ZERO damage.
                assert_eq!(shot.damage, 0.0);
                assert!(!target_wid.is_none());
                let hot = replay.roll_dice(50);
                assert_eq!(hot_spots.1, i32::from(hot));
                assert_eq!(
                    w.rng.0.borrow().sim.draws() - draws_before,
                    2,
                    "a target-connect miss consumes the hit roll and the connect dice"
                );
                assert_eq!(w.rng.0.borrow().sim.draws(), replay.draws());
                assert_eq!(w.rng.0.borrow().sim.state_hash(), replay.state_hash());
            }
        }
    }
}
