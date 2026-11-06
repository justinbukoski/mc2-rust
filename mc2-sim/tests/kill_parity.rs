// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! mc2_24 kill parity (ai-replay sweep, `rust/docs/evidence/kill-parity-mc2_24.md`): the lockstep trace against the C++ oracle found
//! four places where the Rust sim differed from the original on the way to a kill. Each is pinned here on the shipped mc2_01 lance.
//!
//! 1. `BattleMech::updateMovement` draws `RandomNumber(2)` for the fall direction BEFORE the hit-direction override, once per failed
//!    piloting check (mech.cpp:4872-4879), and clears the check whether or not the appearance took the goal.
//! 2. `Mover::relFacingTo` measures the entry angle in the plane (mover.cpp:2909); the 3-D `GameObject` formula moved shots across hit arcs.
//! 3. `Mover::getLOSPosition` of a mech in the jump gesture starts at the live lowest weapon node (mover.cpp:3492-3515).
//! 5. (landing-turn lane) the point sight of a missed shot shares the target sight's lowest-weapon-node cache.
//!
//! Needs `MC2_DATA`.

use mc2_sim::command::{OrderCode, OrderOrigin, PlayerCommand, TacticalOrder};
use mc2_sim::ids::PartId;
use mc2_sim::math::{Vec3, Vec3Ext};
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim, World};
use std::rc::Rc;

mod common;

fn load() -> Option<Sim> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("system.cfg");
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart("mc2_01", 1, RngMode::LegacyMsvc),
    )
    .expect("mc2_01 loads");
    assert!(loaded.complete, "missing hooks: {:?}", loaded.missing_hooks);
    Some(Sim::new(loaded))
}

fn draws(w: &World) -> u64 {
    w.rng.0.borrow().sim.draws()
}

#[test]
fn a_failed_piloting_check_always_draws_the_fall_direction_once() {
    let Some(mut sim) = load() else { return };
    let h = sim.world.objects.find_by_part_id(PartId(512)).unwrap();
    for (front, behind) in [(false, false), (true, false), (false, true)] {
        if let mc2_sim::object::Kind::Mech(m) = &mut sim.world.objects.get_mut(h).unwrap().kind {
            m.failed_piloting_check = true;
            m.hit_from_front = front;
            m.hit_from_behind = behind;
        }
        let before = draws(&sim.world);
        mc2_sim::mover::mech::update_movement(&mut sim.world, h).ok();
        let after = draws(&sim.world);
        assert_eq!(
            after - before,
            1,
            "front={front} behind={behind}: the C++ draws RandomNumber(2) first and lets the hit direction override it"
        );
        let still_pending = matches!(
            &sim.world.objects.get(h).unwrap().kind,
            mc2_sim::object::Kind::Mech(m) if m.failed_piloting_check
        );
        assert!(
            !still_pending,
            "front={front} behind={behind}: one attempt consumes the check (mech.cpp:4874-4879)"
        );
        // the next frame must not draw again
        let before = draws(&sim.world);
        mc2_sim::mover::mech::update_movement(&mut sim.world, h).ok();
        assert_eq!(
            draws(&sim.world) - before,
            0,
            "a consumed check draws nothing on the next frame"
        );
    }
}

#[test]
fn the_hit_arc_of_a_mover_ignores_the_height_difference() {
    let Some(mut sim) = load() else { return };
    let target = sim.world.objects.find_by_part_id(PartId(512)).unwrap();
    let attacker = sim.world.objects.find_by_part_id(PartId(513)).unwrap();
    // the target at the origin faces 0 degrees (its facing vector is (0,-1,0)); the attacker stands 44 degrees off the facing in
    // the plane, far above it: planar 44 degrees is still the front arc, the 3-D angle would be well past 45
    mc2_sim::mover::set_position(&mut sim.world, target, Vec3::from_xyz(0.0, 0.0, 0.0));
    if let Some(o) = sim.world.objects.get_mut(target) {
        o.c.rotation = 0.0;
        o.c.position.z = 0.0;
    }
    let (x, y) = (
        (44.0f32).to_radians().sin() * 400.0,
        -(44.0f32).to_radians().cos() * 400.0,
    );
    mc2_sim::mover::set_position(&mut sim.world, attacker, Vec3::from_xyz(x, y, 0.0));
    let planar = mc2_sim::ai::decide::rel_facing_to(
        &sim.world,
        target,
        &sim.world.objects.get(attacker).unwrap().c.position,
        -1,
    );
    assert!((planar.abs() - 44.0).abs() < 0.5, "planar angle {planar}");
    let saved = sim.world.rng.0.borrow().clone();
    let mut flat = Vec::new();
    let mut high = Vec::new();
    for lift in [0.0f32, 600.0] {
        sim.world.objects.get_mut(attacker).unwrap().c.position.z = lift;
        for round in 0..64 {
            *sim.world.rng.0.borrow_mut() = saved.clone();
            for _ in 0..round {
                sim.world.rng.0.borrow_mut().sim.random_number(100);
            }
            let loc = mc2_sim::combat::hit::calc_hit_location(
                &mut sim.world,
                target,
                attacker,
                0,
                mc2_sim::combat::AttackSource::WeaponFire,
                0,
            );
            if lift == 0.0 {
                flat.push(loc);
            } else {
                high.push(loc);
            }
        }
    }
    assert_eq!(
        flat, high,
        "the same roll must pick the same location whatever the height difference"
    );
}

/// A mech in the jump gesture sights from the lowest weapon node's live height above the terrain under it (mover.cpp:3492-3515), not from
/// the standing height cached at the first refresh.
#[test]
fn a_jumping_mech_sights_from_its_live_lowest_weapon_node() {
    let Some(mut sim) = load() else { return };
    let h = sim.world.objects.find_by_part_id(PartId(512)).unwrap();
    mc2_sim::mover::set_position(&mut sim.world, h, Vec3::from_xyz(2981.0, -744.0, 0.0));
    let goal = Vec3::from_xyz(2981.0, -744.0 + 300.0, 0.0);
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, 0.0);
    order.move_params.way_path.points[0] = goal;
    order.move_params.jump = true;
    let command = PlayerCommand::Order {
        movers: vec![PartId(512)],
        order,
        queue: false,
    };
    // the sight cache is filled by the first sighting, while the mech stands
    mc2_sim::object::hooks::los_position(&mut sim.world, h);
    let mut worst = 0.0f32;
    let mut checked = 0;
    let mut seen_jump = false;
    for frame in 1..=400 {
        let commands = if frame == 2 {
            vec![command.clone()]
        } else {
            Vec::new()
        };
        sim.step(FrameInput::fixed(30, frame, Rc::new(AllVisible), commands))
            .expect("steps");
        if sim.world.appearance.current_gesture_id(h) == 20 {
            seen_jump = true;
            // poison the cached standing height: a jumping mech must not use it
            let node = match &mut sim.world.objects.get_mut(h).unwrap().kind {
                mc2_sim::object::Kind::Mech(m) => {
                    let node = m.m.cached_los_weapon_node.map_or(-2, |c| c.0);
                    m.m.cached_los_weapon_node = Some((node, 777.0));
                    node
                }
                _ => -2,
            };
            let pos = sim.world.objects.get(h).unwrap().c.position;
            let old = sim.world.appearance.in_view(h);
            sim.world.appearance.set_in_view(h, true);
            let node_pos = sim.world.appearance.weapon_node_position(h, node);
            sim.world.appearance.set_in_view(h, old);
            let live = node_pos.z - sim.world.terrain.elevation(&node_pos);
            let los = mc2_sim::object::hooks::los_position(&mut sim.world, h);
            if live > 0.0 {
                checked += 1;
                worst = worst.max((los.z - (pos.z + live)).abs());
            }
        }
    }
    assert!(seen_jump, "the mech never reached the jump gesture");
    assert!(checked > 0, "no jump frame had a node above the terrain");
    assert!(
        worst < 0.01,
        "while jumping the sight height must be position.z + the live node height (worst error {worst})"
    );
}

/// `lowestWeaponNodeID/Z` is one cache in the C++ (mover.cpp:3447-3458, 3470-3515): the point sight of a missed shot (`lineOfSight(point)`)
/// reads the same height as the target sight, so a mech whose cache was filled in the first turns does not re-read it at the first miss.
#[test]
fn the_miss_point_sight_reads_the_shared_lowest_weapon_node_cache() {
    let Some(mut sim) = load() else { return };
    let h = sim.world.objects.find_by_part_id(PartId(512)).unwrap();
    for frame in 1..=8 {
        sim.step(FrameInput::fixed(
            30,
            frame,
            Rc::new(AllVisible),
            Vec::new(),
        ))
        .expect("steps");
    }
    assert!(
        sim.world.clock.turn >= 6,
        "the cache only refreshes before turn 6"
    );
    // a standing mech past turn 6 keeps its cached height; poison it and both sights must show it
    if let mc2_sim::object::Kind::Mech(m) = &mut sim.world.objects.get_mut(h).unwrap().kind {
        let node = m.m.cached_los_weapon_node.map_or(1, |c| c.0);
        m.m.cached_los_weapon_node = Some((node, 77.0));
    }
    let pos = sim.world.objects.get(h).unwrap().c.position;
    let by_target = mc2_sim::object::hooks::los_position(&mut sim.world, h);
    let by_point = mc2_sim::combat::los::mover_los_position(&mut sim.world, h);
    assert_eq!(by_target.z, pos.z + 77.0);
    assert_eq!(by_point.z, by_target.z, "one cache serves both sights");
}
