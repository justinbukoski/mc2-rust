// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Effects attached to appearances (`Mech3DAppearance`, `GVAppearance`, `BldgAppearance`): the sim raises
//! `SimEvent::AppearanceFx` at the C++ condition, for the C++ member (`FxSlot`), at the C++ node. Needs `MC2_DATA`
//! (the shipped mission `mc2_01`); without it the data tests return early.

mod common;

use common::fire_dump::*;
use mc2_sim::appearance::tables::gesture;
use mc2_sim::ids::ObjectHandle;
use mc2_sim::math::Vec3;
use mc2_sim::object::Kind;
use mc2_sim::{fx_id, FxAction, FxBase, FxEffect, FxSlot, SimEvent};

type Call = (ObjectHandle, FxSlot, FxEffect, FxAction);

fn calls(events: &[SimEvent]) -> Vec<Call> {
    events
        .iter()
        .filter_map(|e| match e {
            SimEvent::AppearanceFx {
                object,
                slot,
                effect,
                action,
            } => Some((*object, *slot, effect.clone(), *action)),
            _ => None,
        })
        .collect()
}

fn first_mech(fx: &Fixture) -> ObjectHandle {
    (0..2000)
        .map(ObjectHandle)
        .find(|&h| {
            matches!(fx.w.objects.get(h).map(|o| &o.kind), Some(Kind::Mech(_)))
                && fx.w.appearance.mech(h).is_some()
        })
        .expect("a mech")
}

fn mech_state_walk(fx: &mut Fixture, mech: ObjectHandle) -> [i32; 4] {
    fx.w.objects.use_non_weapon_effects = true;
    mc2_sim::appearance::recalc_bounds(&mut fx.w, mech, true, 0);
    let (m, t) = fx.w.appearance.mech_mut(mech).unwrap();
    m.rotation = 37.0;
    let walk = gesture::WALK;
    t.gestures[walk as usize].foot_down = [3, 15, 9, 24];
    m.current_gesture = walk;
    m.current_state_goal = -1;
    m.current_frame = 0.0;
    t.gestures[walk as usize].foot_down
}

/// A walking mech kicks up a dust poof (effect id `FX_POOF_ID`, 32) at the foot node of each foot-down window, with the right
/// foot's poofs on `rightDustPoofEffect[i]` and the left foot's on `leftDustPoofEffect[i]`, three effects each in turn
/// (mech3d.cpp:3193-3207, `MAX_DUST_POOFS`), at the position the footprint of the same step is printed at.
#[test]
fn a_walking_mech_starts_a_dust_poof_at_each_foot_down_window() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    let down = mech_state_walk(&mut fx, mech);
    let mut poofs: Vec<(FxSlot, Vec3)> = Vec::new();
    let mut prints: Vec<Vec3> = Vec::new();
    let mut expected = Vec::new();
    let mut prev_in = [false; 4];
    for _ in 0..400 {
        fx.w.events.clear();
        mc2_sim::appearance::update(&mut fx.w, mech, true);
        let f = fx.w.appearance.mech(mech).unwrap().current_frame;
        for (i, d) in down.iter().enumerate() {
            let inside = f >= (*d - 2) as f32 && f <= (*d + 2) as f32;
            if inside && !prev_in[i] {
                expected.push(i < 2);
            }
            prev_in[i] = inside;
        }
        for (obj, slot, effect, action) in calls(&fx.w.events) {
            assert_eq!(obj, mech);
            assert_eq!(effect, FxEffect::Id(fx_id::FX_POOF_ID));
            let FxAction::Start { place, looping } = action else {
                panic!("a poof only starts: {action:?}");
            };
            assert!(!looping, "the poof runs once");
            assert_eq!(place.base, FxBase::Plain, "shapeOrigin alone");
            poofs.push((slot, place.at));
        }
        for e in &fx.w.events {
            if let SimEvent::Crater { at, .. } = e {
                prints.push(*at);
            }
        }
    }
    assert!(expected.len() >= 6, "{} windows entered", expected.len());
    assert_eq!(poofs.len(), expected.len(), "one poof per window");
    let (mut right_i, mut left_i) = (0u8, 0u8);
    for ((slot, _), right) in poofs.iter().zip(&expected) {
        let want = if *right {
            let s = FxSlot::RightFootPoof(right_i);
            right_i = (right_i + 1) % 3;
            s
        } else {
            let s = FxSlot::LeftFootPoof(left_i);
            left_i = (left_i + 1) % 3;
            s
        };
        assert_eq!(*slot, want);
    }
    let at: Vec<Vec3> = poofs.iter().map(|p| p.1).collect();
    assert_eq!(at, prints, "the poof and the footprint share the foot node");
}

/// The idle gesture (13) raises no poof (mech3d.cpp:3193 `currentGestureId != 13`) and a wading mech prints no footprint
/// (`!isWaking`, mech3d.cpp:3216) while it still kicks up the poof of the C++ code.
#[test]
fn idle_makes_no_poof_and_wading_makes_no_footprint() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    let down = mech_state_walk(&mut fx, mech);
    // wading: the footprints go, the poofs stay
    fx.w.appearance.mech_mut(mech).unwrap().0.fx.waking = true;
    let (mut poofs, mut prints) = (0, 0);
    for _ in 0..200 {
        fx.w.events.clear();
        mc2_sim::appearance::update(&mut fx.w, mech, true);
        poofs += calls(&fx.w.events)
            .iter()
            .filter(|c| matches!(c.1, FxSlot::RightFootPoof(_) | FxSlot::LeftFootPoof(_)))
            .count();
        prints +=
            fx.w.events
                .iter()
                .filter(|e| matches!(e, SimEvent::Crater { .. }))
                .count();
    }
    assert!(poofs >= 3, "{poofs} poofs while wading ({down:?})");
    assert_eq!(prints, 0, "no footprints while wading");
    // the preference off: neither
    fx.w.objects.use_non_weapon_effects = false;
    fx.w.appearance.mech_mut(mech).unwrap().0.fx.waking = false;
    fx.w.events.clear();
    for _ in 0..200 {
        mc2_sim::appearance::update(&mut fx.w, mech, true);
    }
    // (the foot dust is shown again now that the mech is out of the water: a draw call, no effect starts)
    assert!(calls(&fx.w.events)
        .iter()
        .all(|c| matches!(c.3, FxAction::Draw(true))));
}

/// The jump jet exhaust starts once when the jump begins (looping, `Against_Motion`, pitched 90 degrees, at the jump node
/// `numSmokeNodes + numWeaponNodes`), follows that node while the mech is in its jump, and is stopped when it lands.
#[test]
fn a_jumping_mech_runs_the_jump_jet_exhaust_from_takeoff_to_landing() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    fx.w.objects.use_non_weapon_effects = true;
    mc2_sim::appearance::recalc_bounds(&mut fx.w, mech, true, 0);
    {
        let (m, _) = fx.w.appearance.mech_mut(mech).unwrap();
        m.rotation = 20.0;
        m.current_gesture = gesture::STAND;
        m.current_frame = 0.0;
        m.current_state_goal = -1;
        let p = m.position;
        assert!(m.set_jump_parameters(Vec3 {
            x: p.x + 80.0,
            y: p.y,
            z: p.z,
        }));
        m.set_gesture_goal(mc2_sim::appearance::tables::state::JUMPING);
    }
    let (mut starts, mut moves, mut stops) = (Vec::new(), 0, 0);
    let mut stop_at_land = None;
    for _ in 0..600 {
        fx.w.events.clear();
        mc2_sim::appearance::update(&mut fx.w, mech, true);
        for (obj, slot, effect, action) in calls(&fx.w.events) {
            if slot != FxSlot::JumpJet {
                continue;
            }
            assert_eq!(obj, mech);
            assert_eq!(effect, FxEffect::Id(fx_id::FX_JUMP_ID));
            match action {
                FxAction::Start { place, looping } => {
                    assert!(looping, "LoopFlag until stopped");
                    assert_eq!(place.pitch, 90.0, "JUMP_PITCH");
                    assert_eq!(place.base, FxBase::AgainstMotion);
                    starts.push(place);
                }
                FxAction::Move { place, .. } => {
                    moves += 1;
                    assert_eq!(place.pitch, 90.0);
                }
                FxAction::Stop => {
                    stops += 1;
                    let m = fx.w.appearance.mech(mech).unwrap();
                    stop_at_land.get_or_insert(m.current_frame);
                }
                FxAction::Kill => panic!("the jet is stopped, not killed"),
                FxAction::Draw(_) => {}
            }
        }
    }
    assert_eq!(starts.len(), 1, "started once per jump");
    assert_eq!(starts[0].yaw, 20.0, "the mech's facing");
    assert!(moves > 10, "{moves} follow-ups");
    assert!(stops >= 1, "stopped when the mech lands");
    let (_, t) = fx.w.appearance.mech_mut(mech).unwrap();
    let land = t.jump_land_frame() as f32;
    assert!(
        stop_at_land.unwrap() >= land,
        "not before the landing frame"
    );
}

/// A mech standing over a water cell starts its wake on `MECH_WATER_WAKE` once, at the water level, lying flat
/// (`Euler(90, rotation)`), and puts it out when it leaves the water (mech.cpp:6290-6301, mech3d.cpp:4691-4760).
#[test]
fn a_mech_in_water_starts_the_wake_once_and_kills_it_on_land() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    fx.w.objects.use_non_weapon_effects = true;
    let pos = fx.w.objects.get(mech).unwrap().c.position;
    let (r, c) = fx.w.terrain.world_to_cell(&pos);
    fx.w.terrain.set_deep_water(r, c, true);
    fx.w.terrain.water_elevation = -12.0;
    fx.w.objects.get_mut(mech).unwrap().c.rotation = 15.0;
    fx.w.events.clear();
    for _ in 0..3 {
        mc2_sim::mover::update_mech(&mut fx.w, mech).unwrap();
    }
    let wake: Vec<Call> = calls(&fx.w.events)
        .into_iter()
        .filter(|c| c.1 == FxSlot::WaterWake)
        .collect();
    let starts: Vec<_> = wake
        .iter()
        .filter(|c| matches!(c.3, FxAction::Start { .. }))
        .collect();
    assert_eq!(starts.len(), 1, "started once: {wake:?}");
    assert_eq!(starts[0].2, FxEffect::Id(fx_id::MECH_WATER_WAKE));
    let FxAction::Start { place, looping } = starts[0].3 else {
        unreachable!()
    };
    assert!(looping);
    assert_eq!((place.at.x, place.at.y), (pos.x, pos.y));
    assert_eq!(place.at.z, -12.0, "wake is at water level");
    assert_eq!(place.pitch, 90.0);
    assert!(wake.iter().any(|c| matches!(c.3, FxAction::Move { .. })));
    // out of the water
    fx.w.terrain.set_deep_water(r, c, false);
    fx.w.terrain.set_shallow_water(r, c, false);
    fx.w.events.clear();
    mc2_sim::mover::update_mech(&mut fx.w, mech).unwrap();
    assert!(calls(&fx.w.events)
        .iter()
        .any(|c| c.1 == FxSlot::WaterWake && c.3 == FxAction::Kill));
}

/// A destroyed arm pops off: the shoulder boom (`SHOULDER_POP_ID`) and the arm smoke (`ARM_FLYING_ID`) start at the arm node
/// with `Against_Motion` and the mech's yaw (mech3d.cpp:4350-4470); the C++ asks for the other arm
/// (mech.cpp:5815-5822), the arm smoke follows the flying arm and the shoulder boom the shoulder node.
#[test]
fn a_blown_arm_starts_the_shoulder_pop_and_the_arm_smoke() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    fx.w.objects.use_non_weapon_effects = true;
    mc2_sim::appearance::recalc_bounds(&mut fx.w, mech, true, 0);
    if let Some(Kind::Mech(m)) = fx.w.objects.get_mut(mech).map(|o| &mut o.kind) {
        m.left_arm_blown = true;
    }
    fx.w.events.clear();
    mc2_sim::mover::update_mech(&mut fx.w, mech).unwrap();
    // `leftArmBlownThisFrame` makes the C++ blow the right arm
    let c = calls(&fx.w.events);
    let start = |slot| {
        c.iter().find_map(|x| match (x.1 == slot, x.3) {
            (true, FxAction::Start { place, looping }) => Some((x.2.clone(), place, looping)),
            _ => None,
        })
    };
    let (pop, pop_place, pop_loop) = start(FxSlot::ShoulderBoom(false)).expect("shoulder pop");
    let (smoke, smoke_place, _) = start(FxSlot::ArmSmoke(false)).expect("arm smoke");
    assert_eq!(pop, FxEffect::Id(fx_id::SHOULDER_POP_ID));
    assert_eq!(smoke, FxEffect::Id(fx_id::ARM_FLYING_ID));
    assert!(!pop_loop);
    assert_eq!(pop_place.base, FxBase::AgainstMotion);
    assert_eq!(pop_place.at, smoke_place.at, "both at the shoulder node");
    assert!(fx.w.appearance.arm_off(mech, false), "the right arm is off");
    assert!(!fx.w.appearance.arm_off(mech, true));
    assert!(c
        .iter()
        .any(|x| x.1 == FxSlot::ArmSmoke(false) && matches!(x.3, FxAction::Move { .. })));
    // not blown again
    fx.w.events.clear();
    mc2_sim::mover::update_mech(&mut fx.w, mech).unwrap();
    assert!(!calls(&fx.w.events)
        .iter()
        .any(|x| matches!(x.3, FxAction::Start { .. })));
}

/// A critical hit starts the critical smoke (`CRITICAL_SMOKE_ID`, looping) at smoke node 0, and a finished repair puts it out
/// (mech.cpp:7009, mover.cpp:6768, mech3d.cpp:4602-4688).
#[test]
fn critical_smoke_starts_at_node_zero_and_is_put_out() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    fx.w.objects.use_non_weapon_effects = true;
    mc2_sim::appearance::recalc_bounds(&mut fx.w, mech, true, 0);
    fx.w.events.clear();
    mc2_sim::appearance::fx::start_smoking(&mut fx.w, mech, 1);
    let c = calls(&fx.w.events);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].1, FxSlot::CriticalSmoke);
    assert_eq!(c[0].2, FxEffect::Id(fx_id::CRITICAL_SMOKE_ID));
    let node0 = mc2_sim::appearance::node_position(&mut fx.w, mech, 0);
    match c[0].3 {
        FxAction::Start { place, looping } => {
            assert!(looping);
            assert_eq!(place.at, node0);
        }
        a => panic!("{a:?}"),
    }
    // already smoking at this level: nothing new
    fx.w.events.clear();
    mc2_sim::appearance::fx::start_smoking(&mut fx.w, mech, 1);
    assert!(calls(&fx.w.events).is_empty());
    mc2_sim::appearance::fx::start_smoking(&mut fx.w, mech, -1);
    assert!(calls(&fx.w.events)
        .iter()
        .any(|x| x.1 == FxSlot::CriticalSmoke && x.3 == FxAction::Kill));
}

/// The whole mission, terrain objects and typed vehicle appearances included (the movers-only fixture has neither).
fn full_mission(name: &str) -> Option<mc2_sim::Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, seed) = common::rng_by_name("pcg");
    let loaded = common::load_blocker(
        &mut vfs,
        &mc2_sim::mission::init::LoadSpec::quickstart(name, seed, mode),
    )
    .unwrap_or_else(|e| panic!("{name} loads: {e:?}"));
    Some(mc2_sim::Sim::new(loaded))
}

fn first_vehicle(fx: &mut mc2_sim::Sim) -> ObjectHandle {
    (0..2000)
        .map(ObjectHandle)
        .find(|&h| {
            matches!(
                fx.world.objects.get(h).map(|o| &o.kind),
                Some(Kind::Vehicle(_))
            ) && fx.world.appearance.vehicle_mut(h).is_some()
        })
        .expect("a vehicle")
}

/// A ground vehicle's dust cloud (`VEHICLE_DUST_CLOUD`) starts once at the `dust_body` node, `Against_Motion` with the vehicle's
/// yaw, loops while the vehicle moves and runs out when it stands (gvactor.cpp:2473-2541); no dust over water, where the wake
/// (`VEHICLE_WATER_WAKE`, lying flat at the water level) takes its place (gvactor.cpp:2449, 2755-2817).
#[test]
fn a_ground_vehicle_raises_dust_while_it_moves_and_a_wake_in_water() {
    let Some(mut fx) = full_mission("mc2_01") else {
        return;
    };
    let v = first_vehicle(&mut fx);
    fx.world.objects.use_non_weapon_effects = true;
    let pos = fx.world.objects.get(v).unwrap().c.position;
    fx.world.appearance.set_object_parameters(v, pos, 10.0);
    mc2_sim::appearance::recalc_bounds(&mut fx.world, v, true, 0);
    fx.world.events.clear();
    mc2_sim::appearance::update(&mut fx.world, v, true);
    let c = calls(&fx.world.events);
    let start = c
        .iter()
        .find(|x| x.1 == FxSlot::DustCloud && matches!(x.3, FxAction::Start { .. }))
        .expect("dust starts");
    assert_eq!(start.2, FxEffect::Id(fx_id::VEHICLE_DUST_CLOUD));
    let FxAction::Start { place, looping } = start.3 else {
        unreachable!()
    };
    assert_eq!(place.base, FxBase::AgainstMotion);
    assert_eq!(place.yaw, 10.0);
    assert!(looping, "the vehicle turned this frame: it moved");
    // standing still: the dust is told to run out
    fx.world.appearance.set_object_parameters(v, pos, 10.0);
    fx.world.events.clear();
    mc2_sim::appearance::update(&mut fx.world, v, true);
    assert!(calls(&fx.world.events)
        .iter()
        .any(|x| x.1 == FxSlot::DustCloud
            && matches!(
                x.3,
                FxAction::Move {
                    looping: Some(false),
                    ..
                }
            )));
    // water: the wake instead, once
    let (r, col) = fx.world.terrain.world_to_cell(&pos);
    fx.world.terrain.set_deep_water(r, col, true);
    fx.world.terrain.water_elevation = -7.0;
    fx.world.events.clear();
    for _ in 0..3 {
        mc2_sim::mover::update_vehicle(&mut fx.world, v).unwrap();
    }
    let c = calls(&fx.world.events);
    let wake_starts: Vec<_> = c
        .iter()
        .filter(|x| x.1 == FxSlot::WaterWake && matches!(x.3, FxAction::Start { .. }))
        .collect();
    assert_eq!(wake_starts.len(), 1);
    assert_eq!(wake_starts[0].2, FxEffect::Id(fx_id::VEHICLE_WATER_WAKE));
    let FxAction::Start { place, .. } = wake_starts[0].3 else {
        unreachable!()
    };
    assert_eq!((place.at.z, place.pitch), (-7.0, 90.0));
}

/// A mine layer runs its effect (`MINELAYER_EFFECT_ID`) at `activity_node` while its order is laying mines, and stops it after
/// (gvehicl.cpp:3744-3760).
#[test]
fn a_mine_layer_runs_its_activity_effect_while_laying_mines() {
    let Some(mut fx) = full_mission("mc2_01") else {
        return;
    };
    let v = first_vehicle(&mut fx);
    fx.world.objects.use_non_weapon_effects = true;
    let slot = mc2_sim::combat::fx::pilot_slot(&fx.world, v).expect("pilot");
    fx.world
        .pilots
        .get_mut(slot)
        .unwrap()
        .cur_order
        .move_params
        .mine_laying = true;
    fx.world.events.clear();
    mc2_sim::mover::update_vehicle(&mut fx.world, v).unwrap();
    let c = calls(&fx.world.events);
    let a = c
        .iter()
        .find(|x| x.1 == FxSlot::Activity && matches!(x.3, FxAction::Start { .. }))
        .expect("the effect starts");
    assert_eq!(a.2, FxEffect::Id(fx_id::MINELAYER_EFFECT_ID));
    assert!(matches!(a.3, FxAction::Start { looping: true, .. }));
    fx.world
        .pilots
        .get_mut(slot)
        .unwrap()
        .cur_order
        .move_params
        .mine_laying = false;
    fx.world.events.clear();
    mc2_sim::mover::update_vehicle(&mut fx.world, v).unwrap();
    assert!(calls(&fx.world.events)
        .iter()
        .any(|x| x.1 == FxSlot::Activity && x.3 == FxAction::Kill));
}

/// A turret whose generator is gone powers down: `TURRET_POWER_DOWN_EFFECT` plays once, not looping, at the turret
/// (turret.cpp:630-638).
#[test]
fn a_turret_without_its_generator_plays_the_power_down_effect_once() {
    // the movers-only fixture has no terrain objects: load the whole mission (mc2_02 has 21 turrets)
    let Some(mut fx) = full_mission("mc2_02") else {
        return;
    };
    let turret = fx.world.objects.turrets.clone().into_iter().find(|&h| {
        matches!(
            fx.world.objects.get(h).map(|o| &o.kind),
            Some(Kind::Turret(_))
        ) && fx.world.appearance.building_mut(h).is_some()
    });
    let Some(turret) = turret else {
        panic!("mc2_02 has a turret with an appearance");
    };
    let gen = fx
        .world
        .objects
        .buildings
        .clone()
        .into_iter()
        .find(|&h| {
            h != turret
                && matches!(
                    fx.world.objects.get(h).map(|o| &o.kind),
                    Some(Kind::Building(_))
                )
        })
        .expect("a building");
    fx.world.objects.use_non_weapon_effects = true;
    // the first update runs the turret's setup, which links it to its mission generator: do it before relinking
    mc2_sim::object::structure::update_turret(&mut fx.world, turret);
    let gen_wid = fx.world.objects.watch_id(gen);
    if let Some(Kind::Turret(ts)) = fx.world.objects.get_mut(turret).map(|o| &mut o.kind) {
        ts.parent = gen_wid;
    }
    fx.world
        .objects
        .get_mut(gen)
        .unwrap()
        .c
        .set_status(mc2_sim::object::ObjectStatus::Destroyed, false);
    fx.world.objects.get_mut(turret).unwrap().c.set_awake(true);
    fx.world.events.clear();
    for _ in 0..3 {
        mc2_sim::object::structure::update_turret(&mut fx.world, turret);
    }
    let c = calls(&fx.world.events);
    let starts: Vec<_> = c
        .iter()
        .filter(|x| matches!(x.3, FxAction::Start { .. }))
        .collect();
    assert_eq!(starts.len(), 1, "{c:?}");
    assert_eq!(starts[0].0, turret);
    assert_eq!(starts[0].2, FxEffect::Id(fx_id::TURRET_POWER_DOWN_EFFECT));
    assert!(matches!(
        starts[0].3,
        FxAction::Start { looping: false, .. }
    ));
}

/// The first building of the mission whose appearance type satisfies `pick`.
fn building_where(
    fx: &mut mc2_sim::Sim,
    pick: impl Fn(&mc2_sim::World, ObjectHandle) -> bool,
) -> Option<ObjectHandle> {
    (0..fx.world.objects.max_objects() as u32)
        .map(ObjectHandle)
        .find(|&h| {
            matches!(
                fx.world.objects.get(h).map(|o| &o.kind),
                Some(Kind::Building(_))
            ) && fx.world.appearance.building_mut(h).is_some()
                && pick(&fx.world, h)
        })
}

/// A building whose type names a `[TGLDestructEffect]` plays it, not looping, at the building turned half a circle, when a hit
/// destroys it (bdactor.cpp:1449, bldng.cpp:1456); the generic explosion is left out.
#[test]
fn a_destroyed_building_plays_its_destruct_effect_where_it_stands() {
    let Some(mut fx) = full_mission("mc2_02") else {
        return;
    };
    let h = (0..fx.world.objects.max_objects() as u32)
        .map(ObjectHandle)
        .find(|&h| {
            matches!(
                fx.world.objects.get(h).map(|o| &o.kind),
                Some(Kind::Building(_))
            ) && fx
                .world
                .appearance
                .building_mut(h)
                .is_some_and(|(_, t)| !t.destruct_effect.is_empty())
        })
        .expect("mc2_02 has a building with a destruct effect");
    let name = fx
        .world
        .appearance
        .building_mut(h)
        .unwrap()
        .1
        .destruct_effect
        .clone();
    let (pos, rot) = {
        let o = fx.world.objects.get(h).unwrap();
        (o.c.position, o.c.rotation)
    };
    fx.world
        .clock
        .begin_frame(30.0, mc2_sim::time::FixedStep { fps: 30 }.now_ms(10), false);
    fx.world.events.clear();
    let shot = mc2_sim::combat::WeaponShotInfo {
        attacker: mc2_sim::ids::WatchId::NONE,
        master_id: -3,
        damage: 1.0e9,
        hit_location: 0,
        entry_angle: 0.0,
    };
    mc2_sim::object::structure::handle_weapon_hit(&mut fx.world, h, &shot);
    assert!(fx.world.objects.get(h).unwrap().c.is_destroyed());
    let c = calls(&fx.world.events);
    let starts: Vec<_> = c
        .iter()
        .filter(|x| x.0 == h && x.1 == FxSlot::Destruct)
        .collect();
    assert_eq!(starts.len(), 1, "{c:?}");
    assert_eq!(starts[0].2, FxEffect::Named(name));
    let FxAction::Start { place, looping } = starts[0].3 else {
        panic!("{:?}", starts[0].3)
    };
    assert!(!looping);
    assert_eq!((place.at.x, place.at.y), (pos.x, pos.y));
    assert_eq!(place.yaw, 180.0 + rot);
}

/// A building with an `ActivityEffectID` that is not a mech bay runs it, looping, at its `activity_node` while it stands, and the
/// destruction puts it out (bldng.cpp:811-818, 1452).
#[test]
fn a_standing_building_runs_its_activity_effect_until_it_is_destroyed() {
    use mc2_sim_data::objtype::ObjectTypeData;
    let Some(mut fx) = full_mission("mc2_02") else {
        return;
    };
    let h = building_where(&mut fx, |w, h| {
        matches!(
            w.objects.type_of(h).map(|t| &t.data),
            Some(ObjectTypeData::Building(b)) if b.activity_effect_id != 0xffff_ffff && !b.mech_bay
        )
    })
    .expect("mc2_02 has a building with an activity effect");
    let want = match fx.world.objects.type_of(h).map(|t| &t.data) {
        Some(ObjectTypeData::Building(b)) => b.activity_effect_id as i32,
        _ => unreachable!(),
    };
    fx.world.objects.use_non_weapon_effects = true;
    struct Lod0;
    impl mc2_sim::visibility::Visibility for Lod0 {
        fn block_active(&self, _: u32) -> bool {
            true
        }
        fn vertex_active(&self, _: u32) -> bool {
            true
        }
        fn mover_in_view(&self, _: mc2_sim::ids::PartId) -> bool {
            true
        }
        fn structure_lod(&self, _: ObjectHandle) -> Option<usize> {
            Some(0)
        }
    }
    fx.world.visibility = std::rc::Rc::new(Lod0);
    fx.world
        .clock
        .begin_frame(30.0, mc2_sim::time::FixedStep { fps: 30 }.now_ms(10), false);
    fx.world.events.clear();
    for frame in 11..14 {
        fx.world.clock.begin_frame(
            30.0,
            mc2_sim::time::FixedStep { fps: 30 }.now_ms(frame),
            false,
        );
        mc2_sim::object::structure::update_building(&mut fx.world, h);
    }
    let c = calls(&fx.world.events);
    let starts: Vec<_> = c
        .iter()
        .filter(|x| x.0 == h && x.1 == FxSlot::Activity && matches!(x.3, FxAction::Start { .. }))
        .collect();
    assert_eq!(starts.len(), 1, "{c:?}");
    assert_eq!(starts[0].2, FxEffect::Id(want));
    let FxAction::Start { place, looping } = starts[0].3 else {
        unreachable!()
    };
    assert!(looping);
    let pos = fx.world.objects.get(h).unwrap().c.position;
    let d = ((place.at.x - pos.x).powi(2) + (place.at.y - pos.y).powi(2)).sqrt();
    assert!(d < 200.0, "at the building's activity node: {d}");
    assert!(
        c.iter()
            .any(|x| x.0 == h && x.1 == FxSlot::Activity && matches!(x.3, FxAction::Move { .. })),
        "the effect follows its node: {c:?} in view {}",
        fx.world.appearance.in_view(h)
    );
    fx.world.events.clear();
    let shot = mc2_sim::combat::WeaponShotInfo {
        attacker: mc2_sim::ids::WatchId::NONE,
        master_id: -3,
        damage: 1.0e9,
        hit_location: 0,
        entry_angle: 0.0,
    };
    mc2_sim::object::structure::handle_weapon_hit(&mut fx.world, h, &shot);
    assert!(calls(&fx.world.events)
        .iter()
        .any(|x| x.0 == h && x.1 == FxSlot::Activity && x.3 == FxAction::Kill));
}

/// The draw conditions the C++ `render` applies to attached effects, raised as `Draw` calls: no foot dust while the mech wades
/// (`!isWaking`, mech3d.cpp:2432), no wake while it jumps (`currentGestureId != 20`, mech3d.cpp:2579), and a vehicle in the water
/// draws its wake instead of its dust cloud (gvactor.cpp:2131-2168).
#[test]
fn effects_are_hidden_where_the_cpp_does_not_draw_them() {
    let Some(mut fx) = mc2_01() else { return };
    let mech = first_mech(&fx);
    fx.w.objects.use_non_weapon_effects = true;
    mc2_sim::appearance::recalc_bounds(&mut fx.w, mech, true, 0);
    let draws = |events: &[SimEvent], slot: FxSlot| -> Vec<bool> {
        calls(events)
            .iter()
            .filter(|c| c.1 == slot)
            .filter_map(|c| match c.3 {
                FxAction::Draw(on) => Some(on),
                _ => None,
            })
            .collect()
    };
    // wading: the six foot dust members are hidden, and shown again on land
    fx.w.events.clear();
    mc2_sim::appearance::fx::update_water_wake(&mut fx.w, mech, true);
    mc2_sim::appearance::update(&mut fx.w, mech, true);
    for i in 0..3 {
        assert_eq!(draws(&fx.w.events, FxSlot::RightFootPoof(i)), [false]);
        assert_eq!(draws(&fx.w.events, FxSlot::LeftFootPoof(i)), [false]);
    }
    assert!(draws(&fx.w.events, FxSlot::WaterWake).is_empty());
    // jumping while the wake runs: the wake is hidden
    fx.w.appearance.mech_mut(mech).unwrap().0.current_gesture = gesture::JUMP;
    fx.w.events.clear();
    mc2_sim::appearance::update(&mut fx.w, mech, true);
    assert_eq!(draws(&fx.w.events, FxSlot::WaterWake), [false]);
    fx.w.appearance.mech_mut(mech).unwrap().0.current_gesture = gesture::STAND;
    fx.w.events.clear();
    mc2_sim::appearance::update(&mut fx.w, mech, true);
    assert_eq!(draws(&fx.w.events, FxSlot::WaterWake), [true]);
    mc2_sim::appearance::fx::update_water_wake(&mut fx.w, mech, false);
    fx.w.events.clear();
    mc2_sim::appearance::update(&mut fx.w, mech, true);
    assert_eq!(draws(&fx.w.events, FxSlot::RightFootPoof(0)), [true]);

    // a vehicle that enters the water stops drawing its dust cloud
    let Some(mut sim) = full_mission("mc2_01") else {
        return;
    };
    let v = first_vehicle(&mut sim);
    sim.world.objects.use_non_weapon_effects = true;
    let pos = sim.world.objects.get(v).unwrap().c.position;
    sim.world.appearance.set_object_parameters(v, pos, 0.0);
    mc2_sim::appearance::recalc_bounds(&mut sim.world, v, true, 0);
    mc2_sim::appearance::update(&mut sim.world, v, true);
    let (r, c) = sim.world.terrain.world_to_cell(&pos);
    sim.world.terrain.set_deep_water(r, c, true);
    sim.world.terrain.water_elevation = -7.0;
    sim.world.events.clear();
    for _ in 0..3 {
        mc2_sim::mover::update_vehicle(&mut sim.world, v).unwrap();
    }
    assert_eq!(draws(&sim.world.events, FxSlot::DustCloud), [false]);
    sim.world.terrain.set_deep_water(r, c, false);
    sim.world.terrain.set_shallow_water(r, c, false);
    sim.world.events.clear();
    for _ in 0..3 {
        mc2_sim::mover::update_vehicle(&mut sim.world, v).unwrap();
    }
    assert_eq!(draws(&sim.world.events, FxSlot::DustCloud), [true]);
}

/// A mission loaded from a save starts the effects the saved appearances were running: a mech that was smoking smokes again
/// (mech3d.cpp:5008), and a building that was running its activity effect starts it again.
#[test]
fn a_loaded_save_resumes_smoke_and_building_activity() {
    use mc2_sim_data::objtype::ObjectTypeData;
    let Some(mut fx) = full_mission("mc2_02") else {
        return;
    };
    let Some(base) = full_mission("mc2_02") else {
        return;
    };
    fx.world.objects.use_non_weapon_effects = true;
    let mech = (0..2000)
        .map(ObjectHandle)
        .find(|&h| {
            matches!(
                fx.world.objects.get(h).map(|o| &o.kind),
                Some(Kind::Mech(_))
            ) && fx.world.appearance.mech(h).is_some()
        })
        .expect("a mech");
    mc2_sim::appearance::recalc_bounds(&mut fx.world, mech, true, 0);
    mc2_sim::appearance::fx::start_smoking(&mut fx.world, mech, 1);
    // the destroyed-hulk state (another lane's saved field) rides in the same mech record as the smoke intent
    let Some(Kind::Mech(m)) = fx.world.objects.get_mut(mech).map(|o| &mut o.kind) else {
        unreachable!()
    };
    m.mech_removed = true;
    let bld = building_where(&mut fx, |w, h| {
        matches!(
            w.objects.type_of(h).map(|t| &t.data),
            Some(ObjectTypeData::Building(b)) if b.activity_effect_id != 0xffff_ffff && !b.mech_bay
        )
    })
    .expect("a building with an activity effect");
    mc2_sim::appearance::fx::set_activity(&mut fx.world, bld, Some(0x4f), true);
    assert!(
        fx.world
            .appearance
            .building_mut(bld)
            .unwrap()
            .0
            .is_activitying
    );
    let bytes = mc2_sim::save::save_world(&fx.world);

    let mut base = base.world;
    base.objects.use_non_weapon_effects = true; // the player's preference, which the host sets (it is not saved)
    let mut loaded = mc2_sim::save::load_world(base, None, &bytes).expect("load");
    let c = calls(&loaded.events);
    let smoke = c
        .iter()
        .find(|x| x.0 == mech && x.1 == FxSlot::CriticalSmoke)
        .unwrap_or_else(|| panic!("the critical smoke starts again: {c:?}"));
    assert!(
        matches!(loaded.objects.get(mech).map(|o| &o.kind), Some(Kind::Mech(m)) if m.mech_removed),
        "the hulk state survives the round trip"
    );
    assert_eq!(loaded.appearance.mech(mech).unwrap().is_smoking, 1);
    let node0 = mc2_sim::appearance::node_position(&mut loaded, mech, 0);
    let FxAction::Start { place, looping } = smoke.3 else {
        panic!("{:?}", smoke.3)
    };
    assert!(looping);
    assert_eq!(place.at, node0, "at the smoke node, which it then follows");
    // the building's activity flag is cleared by the load: its own update starts the effect again
    assert!(
        !loaded
            .appearance
            .building_mut(bld)
            .unwrap()
            .0
            .is_activitying
    );
    loaded.events.clear();
    mc2_sim::appearance::fx::set_activity(&mut loaded, bld, Some(0x4f), true);
    assert!(calls(&loaded.events)
        .iter()
        .any(|x| x.0 == bld && x.1 == FxSlot::Activity && matches!(x.3, FxAction::Start { .. })));
}
