// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Support units end to end through `Sim::step` on the shipped missions (RC slice S8; OWNER FIDELITY RULE: outcome-level).
//!
//! * A repair truck (`mc2_16` part 733, vehicle type 182) ordered to a damaged mech refits it at the C++ rate: one pass of
//!   `RefitAmount / 3` points at the armour cost every `RefitTime` seconds (`Mover::refit`, mover.cpp:6636;
//!   `TacticalOrder::status` Refit stage 3, tacordr.cpp:1521-1568), and burns the points it spent.
//! * A damaged mech ordered into a repair bay (`mc2_07` part 69846) is repaired by the bay (`GetFixed`, tacordr.cpp:1612-1700).
//! * A recovery vehicle (`mc2_03` part 710, type 147) recovers a disabled mech: the mech joins the home team with a pilot from the
//!   campaign's roster, is repaired and powered up, and the vehicle is spent (`Recover`, tacordr.cpp:1822-1900).
//! * A mine layer (`mc2_07` part 754, type 120) on a mine-laying order lays mines; a heavy mech that enters one takes the mine's
//!   damage; a mine sweeper sets it off at the price of one armour point (`GroundVehicle::update`, `mineCheck`, gvehicl.cpp).
//!
//! Declared test inputs: the shipped missions field every support unit for the ENEMY (the player gets them from the support
//! VTOL), so a unit is traded to the home team/commander first, as `Mission::tradeMover` does; units and the damaged mech are
//! moved next to each other (the walks across a map are covered by the movement tests); a mech is damaged or disabled by writing
//! its loadout; no shipped vehicle type is a mine sweeper, so the flag is set on a shipped vehicle. The campaign pilot roster
//! (`LogisticsData::getBestPilot`) is the host's input and is a stub that names a shipped pilot file. Everything else is the sim.
//!
//! Tolerance: times within 0.25 s (a few frames) of the C++ constants; points within a quarter point per pass (the C++ rounds the
//! points a pass burns to quarter points, mover.cpp:6770).

mod common;

use std::rc::Rc;

use mc2_sim::combat::{loadout_of, loadout_of_mut, HOME_COMMANDER};
use mc2_sim::command::{OrderCode, OrderOrigin, PlayerCommand, TacticalOrder, TravelMode};
use mc2_sim::events::{fx_id, FxAction, FxEffect, FxSlot, SimEvent};
use mc2_sim::ids::{CommanderId, ObjectHandle, PartId};
use mc2_sim::math::Vec3;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::mission::trade::{RecoveryPilot, RecoveryPilots};
use mc2_sim::mover::refit;
use mc2_sim::object::{flags, terr, Kind, ObjectStatus};
use mc2_sim::visibility::{AllVisible, Visibility};
use mc2_sim::{FrameInput, Sim};

const FPS: u32 = 30;

fn load(mission: &str) -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart(mission, seed, mode))
        .unwrap_or_else(|e| panic!("{mission} loads: {e:?}"));
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    let mut sim = Sim::new(outcome);
    sim.world.objects.use_non_weapon_effects = true;
    Some(sim)
}

/// The attached-effect calls of a run, oldest first: (frame, owner, member, effect, call).
type FxLog = Vec<(u64, ObjectHandle, FxSlot, FxEffect, FxAction)>;

fn log_fx(log: &mut FxLog, frame: u64, events: &[SimEvent]) {
    for e in events {
        if let SimEvent::AppearanceFx {
            object,
            slot,
            effect,
            action,
        } = e
        {
            log.push((frame, *object, *slot, effect.clone(), *action));
        }
    }
}

fn handle(sim: &Sim, part: i32) -> ObjectHandle {
    sim.world
        .objects
        .find_by_part_id(PartId(part))
        .unwrap_or_else(|| panic!("part {part}"))
}

/// `Mission::tradeMover` of a shipped enemy support unit to the home team, keeping its mission pilot (declared test input).
fn make_home(sim: &mut Sim, h: ObjectHandle) {
    let home = sim.world.teams.home as i32;
    mc2_sim::mission::trade::trade_mover(&mut sim.world, h, home, -1, None).expect("trade to home");
    match &mut sim.world.objects.get_mut(h).unwrap().kind {
        Kind::Mech(m) => m.m.commander = CommanderId(HOME_COMMANDER as i8),
        Kind::Vehicle(v) => v.m.commander = CommanderId(HOME_COMMANDER as i8),
        _ => panic!("not a mover"),
    }
}

fn place(sim: &mut Sim, h: ObjectHandle, at: Vec3) {
    let z = sim.world.terrain.elevation(&at);
    let o = sim.world.objects.get_mut(h).unwrap();
    o.c.position = Vec3 { z, ..at };
    o.c.set_awake(true);
}

fn pos(sim: &Sim, h: ObjectHandle) -> Vec3 {
    sim.world.objects.get(h).unwrap().c.position
}

fn player_order(sim: &Sim, code: OrderCode) -> TacticalOrder {
    TacticalOrder::new(
        OrderOrigin::Player,
        code,
        false,
        sim.world.clock.scenario_time,
    )
}

fn step(sim: &mut Sim, frame: u64, commands: Vec<PlayerCommand>) -> Vec<SimEvent> {
    sim.step(FrameInput::fixed(FPS, frame, Rc::new(AllVisible), commands))
        .expect("step")
        .events
}

/// Everything in view at the nearest structure level of detail: the camera input the app's visibility producer supplies, without
/// which a building's appearance does not animate (declared test input).
struct InViewAtLod0;

impl Visibility for InViewAtLod0 {
    fn block_active(&self, _block: u32) -> bool {
        true
    }
    fn vertex_active(&self, _vertex: u32) -> bool {
        true
    }
    fn mover_in_view(&self, _part: PartId) -> bool {
        true
    }
    fn structure_lod(&self, _handle: ObjectHandle) -> Option<usize> {
        Some(0)
    }
}

fn step_in_view(sim: &mut Sim, frame: u64, commands: Vec<PlayerCommand>) -> Vec<SimEvent> {
    sim.step(FrameInput::fixed(
        FPS,
        frame,
        Rc::new(InViewAtLod0),
        commands,
    ))
    .expect("step")
    .events
}

fn command(part: i32, order: TacticalOrder) -> PlayerCommand {
    PlayerCommand::Order {
        movers: vec![PartId(part)],
        order,
        queue: false,
    }
}

/// The location with the most armour and its armour (current, max).
fn biggest_armor(sim: &Sim, h: ObjectHandle) -> (usize, f32, f32) {
    let l = loadout_of(sim.world.objects.get(h).unwrap()).unwrap();
    let (i, a) = l.armor.iter().enumerate().max_by_key(|(_, a)| a.1).unwrap();
    (i, a.0, f32::from(a.1))
}

fn armor_at(sim: &Sim, h: ObjectHandle, loc: usize) -> f32 {
    loadout_of(sim.world.objects.get(h).unwrap()).unwrap().armor[loc].0
}

fn health(sim: &Sim, h: ObjectHandle) -> f32 {
    let l = loadout_of(sim.world.objects.get(h).unwrap()).unwrap();
    l.armor.iter().map(|a| a.0).sum::<f32>()
        + l.body.iter().map(|b| b.cur_internal_structure).sum::<f32>()
}

#[test]
fn repair_truck_refits_a_damaged_mech_at_the_cpp_rate() {
    let Some(mut sim) = load("mc2_16") else {
        return;
    };
    let truck = handle(&sim, 733);
    let mech = handle(&sim, 512);
    let repair_truck_animation = sim
        .world
        .appearance
        .vehicle_types
        .iter()
        .find(|ty| ty.name.eq_ignore_ascii_case("repairtruck"))
        .expect("the repair truck's shipped appearance is loaded");
    assert_eq!(repair_truck_animation.num_frames(3), 101);
    assert_eq!(repair_truck_animation.frame_rate(3), 30.0);
    make_home(&mut sim, truck);
    let general = sim
        .world
        .data
        .gamesys
        .tables
        .as_ref()
        .unwrap()
        .mover_general
        .clone();
    let armor_cost = general.refit_costs[0][0];
    let per_pass = general.refit_amount / 3.0 * armor_cost;
    // damage one location by two and a half passes (or all of it)
    let (loc, cur, max) = biggest_armor(&sim, mech);
    assert_eq!(cur, max, "a fresh mech");
    let deficit = (per_pass * 2.5).min(max);
    loadout_of_mut(sim.world.objects.get_mut(mech).unwrap())
        .unwrap()
        .armor[loc]
        .0 = max - deficit;
    assert!(refit::needs_refit(&sim.world, mech));
    let points_before = refit::vehicle_refit_points(&sim.world, truck);
    assert!(
        points_before > 0.0,
        "a repair truck carries refit points (its turret armour)"
    );
    let mp = pos(&sim, mech);
    place(
        &mut sim,
        truck,
        Vec3 {
            x: mp.x + 250.0,
            y: mp.y,
            z: 0.0,
        },
    );
    let start_distance = mc2_sim::math::approximate_length(&Vec3 {
        x: pos(&sim, truck).x - mp.x,
        y: pos(&sim, truck).y - mp.y,
        z: pos(&sim, truck).z - mp.z,
    });
    assert!(
        start_distance > 128.0,
        "the test includes a real approach: {start_distance} wu"
    );

    let mut order = player_order(&sim, OrderCode::Refit);
    order.target = sim.world.objects.watch_id(mech);
    order.move_params.way_path.mode[0] = TravelMode::Fast;
    order.move_params.face_object = true;
    order.move_params.from_area = -1;
    order.selection_index = -1;
    let issued_at = sim.world.clock.scenario_time;
    let mut changes: Vec<(f32, f32)> = Vec::new(); // (scenario time, armour)
    let mut last = max - deficit;
    let mut shut_down_seen = false;
    let mut repair_hum_seen = false;
    let mut arrived_within_range_at = None;
    let mut deploy_started_at = None;
    let mut gesture_four_at = None;
    let mut first_repair_sample_at = None;
    let mut repair_samples_outside_gesture_four = 0;
    let mut done_at = None;
    let mut fx_log = FxLog::new();
    for f in 1..=(FPS as u64 * 150) {
        let cmds = if f == 1 {
            vec![command(733, order.clone())]
        } else {
            vec![]
        };
        let events = step(&mut sim, f, cmds);
        let repair_sample = events
            .iter()
            .any(|event| matches!(event, mc2_sim::SimEvent::Sound { id: 101, .. }));
        let repair_gesture = match &sim.world.objects.get(truck).unwrap().kind {
            Kind::Vehicle(v) => v.refit_gesture,
            _ => unreachable!(),
        };
        let truck_position = pos(&sim, truck);
        let mech_position = pos(&sim, mech);
        let distance = mc2_sim::math::approximate_length(&Vec3 {
            x: truck_position.x - mech_position.x,
            y: truck_position.y - mech_position.y,
            z: truck_position.z - mech_position.z,
        });
        if distance <= 128.0 && arrived_within_range_at.is_none() {
            arrived_within_range_at = Some(f);
        }
        if repair_gesture == 3 && deploy_started_at.is_none() {
            deploy_started_at = Some(f);
        }
        if repair_gesture == 4 && gesture_four_at.is_none() {
            gesture_four_at = Some(f);
        }
        if repair_sample {
            repair_hum_seen = true;
            if repair_gesture == 4 {
                first_repair_sample_at.get_or_insert(f);
            } else {
                repair_samples_outside_gesture_four += 1;
            }
        }
        log_fx(&mut fx_log, f, &events);
        let a = armor_at(&sim, mech, loc);
        if a != last {
            changes.push((sim.world.clock.scenario_time, a));
            last = a;
        }
        shut_down_seen |= sim.world.objects.get(mech).unwrap().c.status == ObjectStatus::ShutDown;
        if a >= max && !refit::needs_refit(&sim.world, mech) {
            done_at = Some(sim.world.clock.scenario_time);
            break;
        }
    }
    let done_at = done_at.expect("the truck repaired the mech within 150 s");
    // one pass of RefitAmount/3 armour points per RefitTime, the last one only what was missing
    let passes = (deficit / per_pass).ceil() as usize;
    assert_eq!(changes.len(), passes, "{changes:?}");
    let mut expected = max - deficit;
    for (i, (_, a)) in changes.iter().enumerate() {
        expected = (expected + per_pass).min(max);
        assert!((a - expected).abs() < 0.01, "pass {i}: {a} vs {expected}");
    }
    for pair in changes.windows(2) {
        let dt = pair[1].0 - pair[0].0;
        assert!(
            (dt - general.refit_time).abs() < 0.25,
            "passes {dt} s apart, RefitTime {}",
            general.refit_time
        );
    }
    // the truck spent what it repaired (a quarter point per pass of rounding)
    let spent = points_before - refit::vehicle_refit_points(&sim.world, truck);
    assert!(
        (spent - deficit / armor_cost).abs() <= 0.25 * passes as f32 + 0.01,
        "spent {spent} for {} armour at cost {armor_cost}",
        deficit
    );
    assert!(
        shut_down_seen,
        "the mech is powered down while it is repaired"
    );
    assert!(
        repair_hum_seen,
        "the repair hum starts when the repair truck reaches deployed gesture 4"
    );
    assert!(
        repair_samples_outside_gesture_four == 0,
        "the repair hum is requested only during gesture 4"
    );
    let deploy_started_at =
        deploy_started_at.expect("the truck enters its deploy gesture at the buddy");
    let arrived_within_range_at =
        arrived_within_range_at.expect("the truck approaches to within 128 world units");
    assert!(
        (0..=1).contains(&(deploy_started_at as i64 - arrived_within_range_at as i64)),
        "deployment starts at arrival within one sim frame: arrived {arrived_within_range_at}, deployed {deploy_started_at}"
    );
    let gesture_four_at =
        gesture_four_at.expect("the loaded deploy animation completes into gesture 4");
    assert!(
        (98..=103).contains(&(gesture_four_at - deploy_started_at)),
        "the 101-frame, 30-fps deploy animation takes 100 frames ±3: started {deploy_started_at}, gesture 4 {gesture_four_at}"
    );
    assert_eq!(
        first_repair_sample_at,
        Some(gesture_four_at),
        "sample 101 starts on the gesture-4 transition"
    );
    // it is powered up again and free of the handshake
    let mut close_gesture_seen = false;
    for f in 0..(FPS * 10) {
        let events = step(&mut sim, 5000 + u64::from(f), vec![]);
        log_fx(&mut fx_log, 5000 + u64::from(f), &events);
        close_gesture_seen |= matches!(sim.world.objects.get(truck).map(|o| &o.kind), Some(Kind::Vehicle(v)) if v.refit_gesture == 5);
    }
    // the truck's own repair effect (gvehicl.cpp:3750): it opens up (gestures 3, 4) on the way, then `REPAIR_VEHICLE_EFFECT_ID` starts
    // once, looping, at `activity_node` of the truck; closing up (gesture 5) puts it out
    let activity: Vec<_> = fx_log
        .iter()
        .filter(|c| c.1 == truck && c.2 == FxSlot::Activity)
        .collect();
    let starts: Vec<_> = activity
        .iter()
        .filter(|c| matches!(c.4, FxAction::Start { .. }))
        .collect();
    assert_eq!(starts.len(), 1, "{activity:?}");
    assert_eq!(starts[0].3, FxEffect::Id(fx_id::REPAIR_VEHICLE_EFFECT_ID));
    let FxAction::Start { place, looping } = starts[0].4 else {
        unreachable!()
    };
    assert!(looping);
    let tp = pos(&sim, truck);
    let d = ((place.at.x - tp.x).powi(2) + (place.at.y - tp.y).powi(2)).sqrt();
    assert!(
        d < 40.0,
        "the effect stands at the truck's activity node: {d}"
    );
    assert!(
        starts[0].0 > 30,
        "not before the truck has driven to the mech and opened up: frame {}",
        starts[0].0
    );
    let kill = activity
        .iter()
        .find(|c| c.4 == FxAction::Kill)
        .expect("the effect is put out");
    assert!(kill.0 >= starts[0].0);
    let o = sim.world.objects.get(mech).unwrap();
    assert_ne!(
        o.c.status,
        ObjectStatus::ShutDown,
        "powered up after the repair (done at {done_at}, ordered at {issued_at})"
    );
    assert!(
        close_gesture_seen,
        "the repair truck closes after its refit buddy is cleared"
    );
    match &o.kind {
        Kind::Mech(m) => assert_eq!(m.m.refit_buddy, mc2_sim::ids::WatchId::NONE),
        _ => unreachable!(),
    }
}

#[test]
fn disabled_repair_truck_stops_deploying_without_starting_the_repair_hum() {
    let Some(mut sim) = load("mc2_16") else {
        return;
    };
    let truck = handle(&sim, 733);
    let mech = handle(&sim, 512);
    make_home(&mut sim, truck);
    let (loc, current, _) = biggest_armor(&sim, mech);
    loadout_of_mut(sim.world.objects.get_mut(mech).unwrap())
        .unwrap()
        .armor[loc]
        .0 = current - 1.0;
    let mech_position = pos(&sim, mech);
    place(
        &mut sim,
        truck,
        Vec3 {
            x: mech_position.x + 64.0,
            y: mech_position.y,
            z: 0.0,
        },
    );

    let mut order = player_order(&sim, OrderCode::Refit);
    order.target = sim.world.objects.watch_id(mech);
    order.move_params.way_path.mode[0] = TravelMode::Fast;
    order.move_params.face_object = true;
    order.move_params.from_area = -1;
    order.selection_index = -1;
    let mut deploy_started = false;
    for frame in 1..=10 {
        let commands = if frame == 1 {
            vec![command(733, order.clone())]
        } else {
            vec![]
        };
        step(&mut sim, frame, commands);
        deploy_started |= matches!(
            sim.world.objects.get(truck).map(|o| &o.kind),
            Some(Kind::Vehicle(v)) if v.refit_gesture == 3
        );
        if deploy_started {
            break;
        }
    }
    assert!(deploy_started, "the repair truck began its deploy gesture");

    sim.world.objects.get_mut(truck).unwrap().c.status = ObjectStatus::Disabled;
    for frame in 11..=140 {
        let events = step(&mut sim, frame, vec![]);
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, mc2_sim::SimEvent::Sound { id: 101, .. })),
            "sample 101 must stay silent after the truck is disabled at frame {frame}"
        );
        assert!(
            matches!(
                sim.world.objects.get(truck).map(|o| &o.kind),
                Some(Kind::Vehicle(v)) if v.refit_gesture == 0
            ),
            "a disabled truck returns to idle instead of completing deployment"
        );
    }
}

#[test]
fn damaged_mech_ordered_into_a_repair_bay_is_repaired() {
    let Some(mut sim) = load("mc2_07") else {
        return;
    };
    let bay = handle(&sim, 69846);
    let mech = handle(&sim, 512);
    // the shipped bay belongs to the enemy team; the player's team repairs at its own (declared test input)
    {
        let home = sim.world.teams.home as i8;
        match &mut sim.world.objects.get_mut(bay).unwrap().kind {
            Kind::Building(b) => b.team_id = home,
            _ => panic!("bay is a building"),
        }
    }
    assert!(sim.world.objects.get(bay).unwrap().c.flag(flags::MECHBAY));
    let bay_points = refit::building_refit_points(&sim.world, bay);
    assert!(bay_points > 0.0);
    let (loc, _, max) = biggest_armor(&sim, mech);
    let general = sim
        .world
        .data
        .gamesys
        .tables
        .as_ref()
        .unwrap()
        .mover_general
        .clone();
    let deficit = (general.refit_amount / 3.0 * general.refit_costs[0][0] * 1.5).min(max);
    loadout_of_mut(sim.world.objects.get_mut(mech).unwrap())
        .unwrap()
        .armor[loc]
        .0 = max - deficit;
    let bp = pos(&sim, bay);
    place(
        &mut sim,
        mech,
        Vec3 {
            x: bp.x - 300.0,
            y: bp.y - 300.0,
            z: 0.0,
        },
    );

    let mut order = player_order(&sim, OrderCode::GetFixed);
    order.target = sim.world.objects.watch_id(bay);
    order.move_params.way_path.mode[0] = TravelMode::Fast;
    order.move_params.face_object = true;
    order.move_params.from_area = -1;
    order.selection_index = -1;
    let mut repaired_at = None;
    let mut repair_bay_sound_count = 0;
    let bay_position = pos(&sim, bay);
    let mut fx_log = FxLog::new();
    for f in 1..=(FPS as u64 * 150) {
        let cmds = if f == 1 {
            vec![command(512, order.clone())]
        } else {
            vec![]
        };
        let events = step_in_view(&mut sim, f, cmds);
        log_fx(&mut fx_log, f, &events);
        repair_bay_sound_count += events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    mc2_sim::SimEvent::Sound { id: 101, at: Some(at), .. } if *at == bay_position
                )
            })
            .count();
        if armor_at(&sim, mech, loc) >= max {
            repaired_at = Some(sim.world.clock.scenario_time);
            break;
        }
    }
    assert!(
        repaired_at.is_some(),
        "the bay repaired the mech within 150 s"
    );
    assert_eq!(
        repair_bay_sound_count, 1,
        "the bay sound starts once at the bay after shutdown wait"
    );
    assert!(
        refit::building_refit_points(&sim.world, bay) < bay_points,
        "the bay spent refit points"
    );
    // the mech walks off the bay and the handshake is over
    for f in 0..(FPS as u64 * 40) {
        let events = step_in_view(&mut sim, 1000 + f, vec![]);
        log_fx(&mut fx_log, 1000 + f, &events);
    }
    // the bay's activity effect runs while the mech in it is shut down (bldng.cpp:623) and is put out after
    let bay_fx: Vec<_> = fx_log.iter().filter(|c| c.1 == bay).collect();
    let start = bay_fx
        .iter()
        .find(|c| matches!(c.4, FxAction::Start { .. }))
        .unwrap_or_else(|| panic!("the bay's activity effect starts: {bay_fx:?}"));
    assert_eq!(start.2, FxSlot::Activity);
    let bp = pos(&sim, bay);
    let FxAction::Start { place, looping } = start.4 else {
        unreachable!()
    };
    assert!(looping);
    let d = ((place.at.x - bp.x).powi(2) + (place.at.y - bp.y).powi(2)).sqrt();
    assert!(d < 200.0, "at the bay's activity node: {d}");
    assert!(
        bay_fx
            .iter()
            .any(|c| c.4 == FxAction::Kill && c.0 >= start.0),
        "{bay_fx:?}"
    );
    let Kind::Mech(m) = &sim.world.objects.get(mech).unwrap().kind else {
        unreachable!()
    };
    assert_eq!(m.m.refit_buddy, mc2_sim::ids::WatchId::NONE);
    assert_ne!(
        sim.world.objects.get(mech).unwrap().c.status,
        ObjectStatus::ShutDown
    );
}

/// The campaign roster stub: the host's declared input (`LogisticsData::getBestPilot`). It names a pilot file the mission loaded.
struct Roster(String);

impl RecoveryPilots for Roster {
    fn best_pilot(&self, _tonnage: i64, _in_use: &dyn Fn(&str) -> bool) -> Option<RecoveryPilot> {
        Some(RecoveryPilot {
            file: self.0.clone(),
            skills: None,
        })
    }

    fn has_pilot(&self, _in_use: &dyn Fn(&str) -> bool) -> bool {
        true
    }
}

#[test]
fn recovery_vehicle_recovers_a_disabled_mech_for_the_home_team() {
    let Some(mut sim) = load("mc2_03") else {
        return;
    };
    let file = sim
        .world
        .data
        .pilot_profiles
        .keys()
        .next()
        .expect("a loaded pilot file")
        .clone();
    sim.world.recovery_pilots = Some(Rc::new(Roster(file)));
    let vehicle = handle(&sim, 710);
    let mech = handle(&sim, 704);
    make_home(&mut sim, vehicle);
    assert!(refit::vehicle_recover_points(&sim.world, vehicle) > 0.0);
    let vp = pos(&sim, vehicle);
    place(
        &mut sim,
        mech,
        Vec3 {
            x: vp.x + 220.0,
            y: vp.y + 120.0,
            z: 0.0,
        },
    );
    // the enemy mech is shot down: disabled with its structure damaged
    {
        let l = loadout_of_mut(sim.world.objects.get_mut(mech).unwrap()).unwrap();
        for a in &mut l.armor {
            a.0 = 0.0;
        }
        for b in &mut l.body {
            b.cur_internal_structure = (b.cur_internal_structure * 0.4).floor().max(1.0);
        }
    }
    mc2_sim::combat::damage::disable(
        &mut sim.world,
        mech,
        mc2_sim::combat::fx::death::POWER_USED_UP,
    );
    assert!(sim.world.objects.get(mech).unwrap().c.is_disabled());
    let damaged = health(&sim, mech);
    let enemy_team = sim.world.objects.get(mech).unwrap().team_id();
    assert_ne!(enemy_team, sim.world.teams.home as i32);

    let mut order = player_order(&sim, OrderCode::Recover);
    order.target = sim.world.objects.watch_id(mech);
    order.attack_params.attack_type = 0;
    order.attack_params.method = mc2_sim::command::AttackMethod::Ramming;
    order.attack_params.pursue = true;
    order.move_params.way_path.mode[0] = TravelMode::Fast;
    let mut traded_at = None;
    for f in 1..=(FPS as u64 * 150) {
        let cmds = if f == 1 {
            vec![command(710, order.clone())]
        } else {
            vec![]
        };
        step(&mut sim, f, cmds);
        if sim.world.objects.get(mech).unwrap().team_id() == sim.world.teams.home as i32 {
            traded_at = Some(f);
            break;
        }
    }
    assert!(
        traded_at.is_some(),
        "the mech joined the home team within 150 s"
    );
    // let the recovery finish (power-down, recover time, power-up)
    for f in 0..(FPS as u64 * 60) {
        step(&mut sim, 2000 + f, vec![]);
    }
    let o = sim.world.objects.get(mech).unwrap();
    assert_eq!(
        o.commander_id(),
        HOME_COMMANDER,
        "recovered mechs are the commander's"
    );
    assert!(!o.c.is_disabled(), "the recovered mech works again");
    assert_ne!(o.c.status, ObjectStatus::ShutDown, "and is powered up");
    assert!(health(&sim, mech) > damaged, "its structure was put right");
    let v = sim.world.objects.get(vehicle).unwrap();
    assert!(v.c.is_disabled(), "the recovery vehicle is used up");
    let Kind::Vehicle(vv) = &v.kind else {
        unreachable!()
    };
    assert!(!vv.recovering);
    assert_eq!(vv.m.recover_buddy, mc2_sim::ids::WatchId::NONE);
}

#[test]
fn recovery_without_a_campaign_pilot_is_refused_and_changes_nothing() {
    let Some(mut sim) = load("mc2_03") else {
        return;
    };
    let vehicle = handle(&sim, 710);
    let mech = handle(&sim, 704);
    make_home(&mut sim, vehicle);
    let vp = pos(&sim, vehicle);
    place(
        &mut sim,
        mech,
        Vec3 {
            x: vp.x + 100.0,
            y: vp.y,
            z: 0.0,
        },
    );
    mc2_sim::combat::damage::disable(
        &mut sim.world,
        mech,
        mc2_sim::combat::fx::death::POWER_USED_UP,
    );
    let mut order = player_order(&sim, OrderCode::Recover);
    order.target = sim.world.objects.watch_id(mech);
    order.move_params.way_path.mode[0] = TravelMode::Fast;
    let mut refused = false;
    for f in 1..=(FPS as u64 * 60) {
        let cmds = if f == 1 {
            vec![command(710, order.clone())]
        } else {
            vec![]
        };
        let out = sim
            .step(FrameInput::fixed(FPS, f, Rc::new(AllVisible), cmds))
            .expect("step");
        refused |= out.unavailable.iter().any(|u| {
            matches!(u, mc2_sim::FrameUnavailable::Combat { hook } if hook.starts_with("recovery:"))
        });
    }
    assert!(
        refused,
        "the refusal is reported as unavailable, not as success"
    );
    let o = sim.world.objects.get(mech).unwrap();
    assert_ne!(
        o.team_id(),
        sim.world.teams.home as i32,
        "no roster, no recovery"
    );
    assert!(o.c.is_disabled());
}

/// Cells holding a live mine (`mine == 1`) on the whole map.
fn mined_cells(sim: &Sim) -> Vec<(i32, i32)> {
    let t = &sim.world.terrain;
    let side = t.cells_per_side();
    let mut out = Vec::new();
    for r in 0..side {
        for c in 0..side {
            if terr::get_mine(t, r, c) == 1 {
                out.push((r, c));
            }
        }
    }
    out
}

#[test]
fn mine_layer_lays_a_minefield_that_hurts_an_enemy_and_a_sweeper_clears_it() {
    let Some(mut sim) = load("mc2_07") else {
        return;
    };
    let layer = handle(&sim, 754);
    make_home(&mut sim, layer);
    let (turret, turret_before, _) = {
        let l = loadout_of(sim.world.objects.get(layer).unwrap()).unwrap();
        (4usize, l.armor[4].0, l.armor[4].1)
    };
    // the mission seeds some mines of its own
    let seeded = mined_cells(&sim);
    let start = pos(&sim, layer);
    let goal = Vec3 {
        x: start.x + 700.0,
        y: start.y,
        z: start.z,
    };
    let mut order = player_order(&sim, OrderCode::MoveToPoint);
    order.move_params.way_path.points[0] = goal;
    order.move_params.way_path.mode[0] = TravelMode::Slow;
    order.move_params.mine_laying = true;
    // the mine toggle: queued and released
    let cmd = PlayerCommand::OrderQueueExecute {
        movers: vec![PartId(754)],
        order,
    };
    for f in 1..=(FPS as u64 * 90) {
        let cmds = if f == 1 { vec![cmd.clone()] } else { vec![] };
        step(&mut sim, f, cmds);
        if mined_cells(&sim).len() >= seeded.len() + 5 {
            break;
        }
    }
    let mines: Vec<_> = mined_cells(&sim)
        .into_iter()
        .filter(|m| !seeded.contains(m))
        .collect();
    assert!(mines.len() >= 5, "the layer laid {} mines", mines.len());
    // the layer pays one turret armour point per mine and is not hurt by its own field
    let turret_after = armor_at(&sim, layer, turret);
    assert!(
        (turret_before - turret_after - mines.len() as f32).abs() < 0.01,
        "turret {turret_before} -> {turret_after} for {} mines",
        mines.len()
    );
    assert!(!sim.world.objects.get(layer).unwrap().c.is_disabled());
    let layer_cell = {
        let p = pos(&sim, layer);
        terr::world_to_cell(&sim.world.terrain, &p)
    };
    // stop the layer so it does not lay more
    let stop = command(754, player_order(&sim, OrderCode::Stop));
    step(&mut sim, 5000, vec![stop]);

    // an enemy heavy mech (65 t, over the 35 t that sets a mine off) walks into the field
    let enemy = handle(&sim, 726);
    let mine = *mines.iter().find(|&&m| m != layer_cell).unwrap();
    let before = health(&sim, enemy);
    let at = terr::cell_to_world(&sim.world.terrain, mine.0, mine.1);
    place(&mut sim, enemy, at);
    let mine_damage = sim
        .world
        .data
        .gamesys
        .tables
        .as_ref()
        .unwrap()
        .mine
        .mine_damage;
    for f in 0..10 {
        step(&mut sim, 6000 + f, vec![]);
    }
    assert_eq!(
        terr::get_mine(&sim.world.terrain, mine.0, mine.1),
        2,
        "the mine went off"
    );
    let lost = before - health(&sim, enemy);
    assert!(
        lost >= mine_damage - 0.01,
        "the mech lost {lost}, a mine does {mine_damage}"
    );

    // a sweeper (a shipped vehicle flagged as one: no shipped type is) clears the next mine for one armour point
    let sweeper = handle(&sim, 755);
    make_home(&mut sim, sweeper);
    if let Kind::Vehicle(v) = &mut sim.world.objects.get_mut(sweeper).unwrap().kind {
        v.loadout.stats.mine_layer = 0;
        v.loadout.stats.mine_sweeper = true;
    }
    let mine2 = *mines
        .iter()
        .find(|&&m| m != layer_cell && m != mine)
        .unwrap();
    let front = armor_at(&sim, sweeper, 0);
    let structure = health(&sim, sweeper) - sim_armor_total(&sim, sweeper);
    let at = terr::cell_to_world(&sim.world.terrain, mine2.0, mine2.1);
    place(&mut sim, sweeper, at);
    for f in 0..10 {
        step(&mut sim, 7000 + f, vec![]);
    }
    assert_eq!(
        terr::get_mine(&sim.world.terrain, mine2.0, mine2.1),
        2,
        "the sweeper cleared the mine"
    );
    assert!(
        (front - armor_at(&sim, sweeper, 0) - 1.0).abs() < 0.01,
        "it paid one front armour point"
    );
    assert!(
        (health(&sim, sweeper) - sim_armor_total(&sim, sweeper) - structure).abs() < 0.01,
        "and took no structure damage"
    );
}

fn sim_armor_total(sim: &Sim, h: ObjectHandle) -> f32 {
    loadout_of(sim.world.objects.get(h).unwrap())
        .unwrap()
        .armor
        .iter()
        .map(|a| a.0)
        .sum()
}
