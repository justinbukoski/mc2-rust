// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A SENSOR artillery strike on the shipped `mc2_01` mission, driven through `Sim::step` only.
//!
//! What the C++ does (`Artillery::setJustCreated` artlry.cpp:760-838 and `Artillery::update` artlry.cpp:1097-1128): a strike whose
//! type has `NominalSensorRange != 0` takes a sensor from the pool (`SensorManager->newSensor`, contact.cpp:1221), joins it to the
//! striking team with `Artillery::setSensorData` (artlry.cpp:1302-1317: owner, range, `hasLOSCapability = false`), starts the
//! `nominalSensorTime` countdown on the first update, decays the sensor range linearly with the remaining time, and when the
//! countdown ends removes the sensor from the team and returns it to the pool. The merged sensor scan (`SensorSystem::updateScan`,
//! contact.cpp:510) then scans with it, so an enemy that was not a contact becomes one for the striking team.
//!
//! EVIDENCE RULE: the world is the production `load_world` of `mc2_01`; nothing is installed. The duration and range the strike must
//! honour are read from the shipped `object2.pak` bytes in this test (the packet of `SENSOR_ARTLRY` 250, parsed here from the raw
//! `ArtilleryType` record fields, not through the loader under test), and the enemy's identity/relation from the loaded world.
//!
//! TOLERANCE (fidelity rule): the reveal frame is bounded, not pinned (the team scan is round robin: a team of `n` scans every `n`
//! frames, so the first scan with the new sensor lands within `numTeams + 1` frames of the strike; the test allows 8 frames); the
//! expiry frame is pinned to the frame the C++ arithmetic produces on this data (the countdown is pure float subtraction of
//! `frameLength` from `nominalSensorTime`, deterministic in Rust; the C++ reaches the same frame up to float last bits, and the
//! observable outcome — the sensor is gone after `nominalSensorTime` seconds — is exact).

mod common;

use mc2_sim::combat::artillery_math::ARTILLERY_SENSOR;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::{Vec3, Vec3Ext};
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::sim::FrameInput;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{PlayerCommand, Sim, World};
use std::rc::Rc;

fn load(mission: &str) -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let outcome = load_world(&mut vfs, &LoadSpec::quickstart(mission, 7, RngMode::Pcg32))
        .unwrap_or_else(|e| panic!("{mission}: {e}"));
    assert!(
        outcome.complete && outcome.missing_hooks.is_empty(),
        "{mission}: {:?}",
        outcome.missing_hooks
    );
    Some(Sim::new(outcome))
}

fn step(sim: &mut Sim, frame: u64) {
    sim.step(FrameInput::fixed(
        30,
        frame,
        Rc::new(AllVisible),
        Vec::new(),
    ))
    .expect("step");
}

/// Drive the production player path: `Mission::update` applies the command,
/// then the same `Sim::step` updates the new artillery object and scans the
/// sensor pool (`mission.cpp:475, 510-524`).
fn call_sensor_strike(sim: &mut Sim, frame: u64, at: Vec3) -> mc2_sim::FrameOutput {
    sim.step(FrameInput::fixed(
        30,
        frame,
        Rc::new(AllVisible),
        vec![PlayerCommand::CallStrike {
            strike: ARTILLERY_SENSOR,
            at,
        }],
    ))
    .expect("step")
}

fn mover(w: &World, part: i32) -> ObjectHandle {
    *w.objects
        .movers
        .iter()
        .find(|h| w.objects.get(**h).unwrap().c.part_id.0 == part)
        .unwrap_or_else(|| panic!("no mover with part id {part}"))
}

/// Find the nearest eligible home mover to the known shipped enemy probe
/// target. Calling a probe at their midpoint keeps both candidates inside its
/// shipped radius, while leaving their identities, teams, and initial
/// positions loaded from the mission.
fn nearest_friendly_enemy_pair(w: &World) -> (ObjectHandle, ObjectHandle, Vec3) {
    let home_team = w.teams.home as i32;
    let enemy = mover(w, 706);
    let enemy_obj = w.objects.get(enemy).expect("shipped mover 706");
    assert_eq!(
        mc2_sim::object::hooks::team_relation(w, home_team, enemy_obj.team_id()),
        Some(mc2_sim::ai::Relation::Enemy)
    );
    assert_ne!(
        enemy_obj.c.flags & mc2_sim::object::flags::SENSOR,
        0,
        "the established shipped probe target is sensor-detectable"
    );
    let mut nearest = None;
    for friendly in w.objects.movers.iter().copied() {
        let Some(friendly_obj) = w.objects.get(friendly) else {
            continue;
        };
        if friendly_obj.team_id() != home_team
            || friendly_obj.c.flags & mc2_sim::object::flags::SENSOR == 0
            || friendly_obj.c.is_disabled()
        {
            continue;
        }
        let distance = friendly_obj.c.position.distance_xy(&enemy_obj.c.position);
        match nearest {
            Some((_, best)) if distance >= best => {}
            _ => nearest = Some((friendly, distance)),
        }
    }
    let (friendly, _) = nearest.expect("a sensor-detectable friendly mover in mc2_01");
    let a = w.objects.get(friendly).unwrap().c.position;
    let b = w.objects.get(enemy).unwrap().c.position;
    (
        friendly,
        enemy,
        Vec3 {
            x: (a.x + b.x) * 0.5,
            y: (a.y + b.y) * 0.5,
            z: (a.z + b.z) * 0.5,
        },
    )
}

/// Remove the mission's normal sensors from this test's scan schedule. The
/// production strike then remains the only source that can publish either
/// contact; this is setup, not an oracle-fed result.
fn isolate_sensor_probe(w: &mut World) {
    for sensor in &mut w.sensors.pool {
        sensor.range = -1.0;
        sensor.contacts.clear();
    }
    for team_sensors in &mut w.sensors.teams {
        team_sensors.clear();
    }
    for contacts in &mut w.sensors.team_contact_order {
        contacts.clear();
    }
    w.sensors.team_contact_status.clear();
    w.sensors.enemy_in_los = false;
}

/// `NominalSensorTime` and `NominalSensorRange` of the shipped `SENSOR_ARTLRY` (object number 250, dobjnum.h:223), read from
/// the decompressed packet 250 of the shipped `data/objects/object2.pak` with the production FIT reader (`mc2_formats::ini::FitIni`
/// seeking the `[Artillery]` block, the same keys `ArtilleryType::read` walks at artlry.cpp:416-448). The values are the shipped
/// data the strike must honour; they are never taken from the code under test.
fn shipped_sensor_strike_data() -> Option<(f32, f32)> {
    let root = common::data_root()?;
    let path = root.join("data/objects/object2.pak");
    let pak = mc2_formats::pak::PakFile::open(&path).expect("open object2.pak");
    // The packet index is positional: packet 250 is `SENSOR_ARTLRY` (objmgr.cpp:2763 installs it as strikeObjectId[SENSOR]).
    let packet = pak.read_packet(250).expect("packet 250");
    let mut fit = mc2_formats::ini::FitIni::from_bytes(packet).expect("FIT header of packet 250");
    fit.seek_block("Artillery").expect("[Artillery] block");
    // A sensor round is identified exactly as the port does it: `nominalDamage == 0` plus a positive sensor range
    // (artlry.cpp:775). If packet 250 ever stopped being the sensor type, this assert fails instead of the test silently
    // probing the wrong record.
    assert_eq!(fit.read_f32("NominalDamage").expect("NominalDamage"), 0.0);
    let time = fit
        .read_f32("NominalSensorTime")
        .expect("NominalSensorTime");
    let range = fit
        .read_f32("NominalSensorRange")
        .expect("NominalSensorRange");
    (time > 0.0 && range > 0.0).then_some((time, range))
}

/// The strike's sensor reveals an enemy that was not a contact, and expires on the shipped schedule.
#[test]
fn sensor_strike_reveals_an_enemy_and_expires() {
    let Some(mut sim) = load("mc2_01") else {
        return;
    };
    let (sensor_time, sensor_range_m) = shipped_sensor_strike_data().expect(
        "a shipped sensor artillery record (damage 0, positive NominalSensorTime/NominalSensorRange) in object2.pak",
    );
    // The shipped sensor probe: 120 s of scan at 800 m. These are the values the strike must honour.
    assert_eq!(sensor_time, 120.0);
    assert_eq!(sensor_range_m, 800.0);

    let w = &mut sim.world;
    // Isolate a real probe against a real friendly/enemy pair. The initial
    // sensor schedule is not the result under test, so it is disabled before
    // the player command creates the new sensor.
    isolate_sensor_probe(w);
    let (friendly, enemy, at) = nearest_friendly_enemy_pair(w);
    let friendly_pos = w.objects.get(friendly).unwrap().c.position;
    let enemy_pos = w.objects.get(enemy).unwrap().c.position;
    assert!(
        friendly_pos.distance_xy(&enemy_pos) * 0.5 * mc2_sim::terrain::METERS_PER_WORLD_UNIT
            < sensor_range_m,
        "the shipped nearest friendly/enemy pair fits inside the probe radius"
    );
    assert_eq!(
        mc2_sim::object::hooks::team_relation(
            w,
            w.teams.home as i32,
            w.objects.get(enemy).unwrap().team_id()
        ),
        Some(mc2_sim::ai::Relation::Enemy)
    );
    assert!(w
        .sensors
        .team_contact_status
        .get(&enemy)
        .is_none_or(|s| s.iter().all(|x| *x == 0)));
    assert!(w
        .sensors
        .team_contact_status
        .get(&friendly)
        .is_none_or(|s| s.iter().all(|x| *x == 0)));

    // The player calls a sensor strike on the enemy's position through
    // `IfaceCallStrike`/`Sim::step` (artlry.cpp:159-232, then CallArtillery at
    // :99). This is deliberately not a direct helper call.
    let first_output = call_sensor_strike(&mut sim, 1, at);
    assert!(
        first_output
            .unavailable
            .iter()
            .all(|u| !format!("{u:?}").contains("sensor")),
        "the production sensor-strike path is fully ported: {u:?}",
        u = first_output.unavailable
    );
    let w = &mut sim.world;
    let strike = *w
        .objects
        .artillery
        .iter()
        .find(|h| w.objects.get(**h).is_some_and(|o| o.exists()))
        .expect("the strike object exists");
    let sensor = usize::try_from(mc2_sim::combat::artillery::sensor_system_index(w, strike))
        .expect("the strike owns a sensor slot");
    // `setSensorData` (artlry.cpp:1302-1317) gives the probe the shipped
    // range, then this same `Sim::step` reaches `Artillery::update` and makes
    // its first countdown tick (artlry.cpp:1097-1106). Observe the resulting
    // C++ range rather than a helper-only pre-update intermediate.
    assert_eq!(w.sensors.pool[sensor].owner, strike);
    assert_eq!(
        w.objects.get(strike).unwrap().team_id(),
        w.teams.home as i32,
        "the artillery owner's virtual team is the striking team (artlry.h:304-307)"
    );
    let first_tick_range = ((sensor_time - 1.0 / 30.0) / sensor_time) * sensor_range_m * 1.002;
    assert!(
        (w.sensors.pool[sensor].range - first_tick_range).abs() < 1e-3,
        "the first production frame applies the C++ sensor countdown"
    );
    assert!(!w.sensors.pool[sensor].has_los_capability);
    assert!(w.sensors.teams[0].contains(&sensor));
    let team_sensors_before = w.sensors.teams[0].len();
    let _ = w;

    // (a) the reveal: within a few frames the enemy is a team contact for the striking team. The scan cadence is round robin, so
    // the exact frame is a timing intermediate (tolerance: 8 frames, stated above); the outcome (a sensor-quality contact, not
    // visual — a probe has no LOS capability) is a rule and is exact.
    let mut reveal = sim
        .world
        .sensors
        .team_contact_status
        .get(&enemy)
        .and_then(|s| (s[0] != 0).then_some((1, s[0])));
    for f in 2..=8u64 {
        if reveal.is_some() {
            break;
        }
        step(&mut sim, f);
        if let Some(s) = sim.world.sensors.team_contact_status.get(&enemy) {
            if s[0] != 0 {
                reveal = Some((f, s[0]));
                break;
            }
        }
    }
    let (reveal_frame, status) =
        reveal.expect("the sensor strike makes the enemy a contact within 8 frames");
    assert!(
        (1..=4).contains(&status),
        "a probe contact is sensor quality 1..4, got {status}"
    );
    assert!(reveal_frame <= 8);
    assert!(
        !sim.world.sensors.pool[sensor]
            .contacts
            .iter()
            .any(|(contact, _)| *contact == friendly),
        "scanBattlefield excludes the friendly mover by the artillery owner's team (contact.cpp:591)"
    );
    assert!(
        sim.world.sensors.enemy_in_los,
        "the probe's enemy contact raises the home team's enemyInLOS flag"
    );

    // The range decays with the countdown (artlry.cpp:1101-1106): strictly decreasing while the sensor runs. The countdown
    // recomputes the sensor range as `timeToBlind / nominalSensorTime * nominalSensorRange * worldUnitsPerMeter * metersPerWorldUnit`
    // (artlry.cpp:1101-1106) and the shipped constants (mclib/terrain.cpp:63-64, 5.01f and 0.2f) are not exact inverses, so the
    // countdown starts at `sensor_range_m * 1.002` meters — the value the C++ puts in `sensor->setRange` on the first countdown
    // frame. The bound below is that shipped-data product, not the raw `NominalSensorRange`.
    let r_after_reveal = sim.world.sensors.pool[sensor].range;
    assert!(r_after_reveal <= sensor_range_m * 1.002 + 1e-3);

    // (b) the expiry: the sensor runs for the shipped `nominalSensorTime` seconds. The countdown starts on the creation update
    // (the `else if (type->nominalSensorTime > 0)` branch, artlry.cpp:1123-1127) and each update subtracts `frameLength`
    // (artlry.cpp:1100). At the fixed 30 fps step the float subtraction `120 - n/30` crosses zero at n = 3600 (0.0014863834 at
    // n = 3599) and the next update frees the sensor and ends the strike (artlry.cpp:1113-1121): the strike dies on its 3601st
    // update, deterministic in Rust; the C++ reaches the same frame up to float last bits. The strike's first update ran on the
    // frame it was created, before this test's stepping, so at step frame `f` it has run `f + 1` updates and the death lands on
    // step frame 3602 (3600 updates of the countdown, then the update that frees the sensor, then the update that ends the
    // strike). The reveal loop above stops early (round robin, tolerance); the expiry loop simply steps to the death.
    let death_frame = (sensor_time * 30.0) as u64 + 2; // the 3601st update of the strike, at step frame 3602
    let mut expiry = None;
    for f in (reveal_frame + 1)..=(death_frame + 10) {
        step(&mut sim, f);
        if !sim.world.objects.get(strike).is_some_and(|o| o.exists()) {
            expiry = Some(f);
            break;
        }
    }
    let expiry_frame = expiry.expect("the sensor strike ends within nominalSensorTime + 10 frames");
    assert_eq!(
        expiry_frame, death_frame,
        "the strike dies on its 3601st update, nominalSensorTime seconds after the countdown started"
    );
    // The reveal happened long before the expiry and the range really decayed.
    assert!(r_after_reveal > sim.world.sensors.pool[sensor].range);

    let s = &sim.world.sensors.pool[sensor];
    assert_eq!(
        s.owner,
        ObjectHandle::NONE,
        "the sensor returns to the pool (freeSensor, artlry.cpp:1119)"
    );
    assert_eq!(s.team_index, -1);
    assert!(!sim.world.sensors.teams[0].contains(&sensor));
    assert_eq!(sim.world.sensors.teams[0].len(), team_sensors_before - 1);
    assert!(
        sim.world
            .sensors
            .team_contact_status
            .get(&enemy)
            .is_none_or(|st| st.iter().all(|x| *x == 0)),
        "the contact goes away with the sensor"
    );
}

/// The sensor strike reports nothing unavailable: the lane D sensor pool hook is gone (the strike is ported end to end).
#[test]
fn sensor_strike_reports_no_unavailable() {
    let Some(mut sim) = load("mc2_01") else {
        return;
    };
    let w = &mut sim.world;
    let enemy = mover(w, 706);
    let at = w.objects.get(enemy).unwrap().c.position;
    let out = call_sensor_strike(&mut sim, 1, at);
    assert!(
        out.unavailable
            .iter()
            .all(|u| !format!("{u:?}").contains("sensor")),
        "no sensor hook: {u:?}",
        u = out.unavailable
    );
}

/// Expiry must remove only the probe while the mission's ordinary sensors remain scheduled.
#[test]
fn probe_expires_with_normal_team_sensors() {
    let Some(mut sim) = load("mc2_01") else {
        return;
    };
    let (time, _) = shipped_sensor_strike_data().unwrap();
    let team = sim.world.teams.home;
    let normal = sim.world.sensors.teams[team].clone();
    assert!(!normal.is_empty(), "mission has normal home sensors");
    let (_, _, at) = nearest_friendly_enemy_pair(&sim.world);
    call_sensor_strike(&mut sim, 1, at);
    let strike = *sim
        .world
        .objects
        .artillery
        .iter()
        .find(|h| sim.world.objects.get(**h).is_some_and(|o| o.exists()))
        .unwrap();
    let sensor = usize::try_from(mc2_sim::combat::artillery::sensor_system_index(
        &sim.world, strike,
    ))
    .unwrap();
    assert!(!normal.contains(&sensor));
    assert!(normal
        .iter()
        .all(|s| sim.world.sensors.teams[team].contains(s)));
    for frame in 2..=((time * 30.0) as u64 + 12) {
        step(&mut sim, frame);
        if !sim.world.objects.get(strike).is_some_and(|o| o.exists()) {
            break;
        }
    }
    assert!(
        !sim.world.objects.get(strike).is_some_and(|o| o.exists()),
        "probe expires within nominal duration plus 12 frames"
    );
    assert_eq!(sim.world.sensors.pool[sensor].owner, ObjectHandle::NONE);
    assert!(!sim.world.sensors.teams[team].contains(&sensor));
    assert!(
        normal
            .iter()
            .any(|s| sim.world.sensors.teams[team].contains(s)),
        "normal sensors remain after probe expiry"
    );
}
