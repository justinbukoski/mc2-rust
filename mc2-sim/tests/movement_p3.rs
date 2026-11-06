// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! P3 movement gaps (mc2_01): the lance reaching the west of the map and the retreating bandit patrol (watch ids 4 and 5).
//!
//! Every test loads the shipped mc2_01 with the Rust mission load and drives orders through `Sim::step`, as the application
//! does. Targeted scenarios place mission movers with `mover::set_position`; Visibility is `AllVisible`.
//!
//! What stopped the lance (found by running these scenarios, see `rust/docs/porting-log.md`): an attack order on a target that no
//! reachable cell can see (the patrol's hovercraft sit on open water, which a walking mech cannot enter) ended in
//! `MovementUnavailable(AttackGoalScoring)` because the no-line-of-fire branch of `Mover::calcMoveGoal` (`calcBestGoalFromTarget`) was not
//! ported, and the pilots then sat in `AttackObject`/`Alarm` with zero velocity. A move order whose goal is water cannot succeed in
//! the C++ either (the global map has no area there); it ends in the escape-tile alarm, which was unported (`EscapePath`).
//!
//! Tolerances: positions within 128 world units (three cells; the goal is snapped to a cell centre and a pilot stops inside the
//! margin of error), frame budgets are the measured arrival frame plus about a third. Needs `MC2_DATA`.

use mc2_sim::command::{
    AttackMethod, OrderCode, OrderOrigin, PlayerCommand, TacticalOrder, FIRERANGE_OPTIMAL,
};
use mc2_sim::ids::{PartId, WatchId};
use mc2_sim::math::{Vec3, Vec3Ext};
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::mover::MoverDecisionUnavailable;
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim, SimEvent};
use std::rc::Rc;

mod common;

/// The lance's first mech.
const MECH: i32 = 512;
/// The airfield of objective 1 (the position of the P3 report).
const AIRFIELD: (f32, f32) = (2981.0, -744.0);

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

fn place(sim: &mut Sim, part: i32, x: f32, y: f32) {
    let h = sim.world.objects.find_by_part_id(PartId(part)).unwrap();
    mc2_sim::mover::set_position(&mut sim.world, h, Vec3::from_xyz(x, y, 0.0));
}

/// Disable every weapon of every mover except `keep` (the movement scenarios are not about combat).
fn disarm_other_movers(sim: &mut Sim, keep: i32) {
    let keep = sim.world.objects.find_by_part_id(PartId(keep)).unwrap();
    for other in sim.world.objects.movers.clone() {
        if other == keep {
            continue;
        }
        if let Some(loadout) =
            mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(other).unwrap())
        {
            for weapon in loadout.num_other()..loadout.num_other() + loadout.num_weapons() {
                loadout.inventory[weapon].disabled = true;
            }
        }
    }
}

fn pos(sim: &Sim, part: i32) -> Vec3 {
    let h = sim.world.objects.find_by_part_id(PartId(part)).unwrap();
    sim.world.objects.get(h).unwrap().c.position
}

fn watch_pos(sim: &Sim, watch: u32) -> Vec3 {
    let h = sim.world.objects.by_watch_id(WatchId(watch)).unwrap();
    sim.world.objects.get(h).unwrap().c.position
}

fn dist(a: Vec3, b: Vec3) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

fn move_order(part: i32, x: f32, y: f32) -> PlayerCommand {
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::MoveToPoint, false, 0.0);
    order.move_params.way_path.points[0] = Vec3::from_xyz(x, y, 0.0);
    PlayerCommand::Order {
        movers: vec![PartId(part)],
        order,
        queue: false,
    }
}

/// `MissionInterfaceManager::doAttack` for one mover: a ranged attack that pursues at the optimal range.
fn attack_order(part: i32, target: u32) -> PlayerCommand {
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::AttackObject, false, 0.0);
    order.target = WatchId(target);
    order.attack_params.method = AttackMethod::Ranged;
    order.attack_params.range = FIRERANGE_OPTIMAL;
    order.attack_params.pursue = true;
    order.attack_params.attack_type = 1;
    PlayerCommand::Order {
        movers: vec![PartId(part)],
        order,
        queue: false,
    }
}

/// Step `frames` frames (frame numbers from 1; `command` is issued on frame 2), calling `observe(frame, sim, output)` after each.
fn run(
    sim: &mut Sim,
    command: PlayerCommand,
    frames: u64,
    mut observe: impl FnMut(u64, &Sim, &mc2_sim::FrameOutput) -> bool,
) {
    for f in 1..=frames {
        let commands = if f == 2 {
            vec![command.clone()]
        } else {
            Vec::new()
        };
        let out = sim
            .step(FrameInput::fixed(30, f, Rc::new(AllVisible), commands))
            .expect("complete load steps");
        if !observe(f, sim, &out) {
            return;
        }
    }
}

fn movement_unavailable(out: &mc2_sim::FrameOutput, part: i32) -> Vec<MoverDecisionUnavailable> {
    out.events
        .iter()
        .filter_map(|e| match e {
            SimEvent::MovementUnavailable { part_id, decision } if *part_id == part => {
                Some(*decision)
            }
            _ => None,
        })
        .collect()
}

/// A mech at the airfield (2981, -744) ordered to a point west of the map centre arrives.
///
/// The goal is (64, 1258.67): a dry ground cell on the west side of the map. The map boundary has no standable cell at x < 0,
/// so goals beyond it can leave the final global door outside the mover's reachable bounds. The route runs around the lake through
/// the global map. Arrival within 128 units inside 4900 frames.
#[test]
fn mech_from_the_airfield_walks_to_a_point_west_of_the_map_centre() {
    let Some(mut sim) = load() else { return };
    let goal = (64.0_f32, 1258.6666_f32);
    {
        let t = &sim.world.terrain;
        let (r, c) = t.world_to_cell(&Vec3::from_xyz(goal.0, goal.1, 0.0));
        assert!(
            t.move_data().global[0].calc_area(r, c) >= 0,
            "goal is on the ground map"
        );
        assert!(!t.get_deep_water(r, c) && !t.get_shallow_water(r, c));
    }
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    // This is a movement scenario: enemy vehicles now fire (part B), and a lone mech walking through their base is shot down
    // before it arrives. Disable every other mover's weapons so the walk is judged on movement alone.
    disarm_other_movers(&mut sim, MECH);
    let mut arrived = None;
    let mut unavailable = Vec::new();
    run(
        &mut sim,
        move_order(MECH, goal.0, goal.1),
        4900,
        |f, sim, out| {
            unavailable.extend(movement_unavailable(out, MECH));
            let p = pos(sim, MECH);
            if dist(p, Vec3::from_xyz(goal.0, goal.1, 0.0)) <= 128.0 {
                arrived = Some(f);
                return false;
            }
            true
        },
    );
    let frame = arrived.expect("the mech reaches the goal inside the budget");
    eprintln!("arrived at frame {frame}");
    assert!(unavailable.is_empty(), "{unavailable:?}");
}

/// A hit sets `hitFromFrontThisFrame` / `hitFromBehindThisFrame` on every mech hit (mech.cpp:7555-7560), and a live mech's update
/// reads neither (they only pick the direction of a fall, mech.cpp:4787-4801, 4873-4875). A mech that has been hit must keep
/// walking, with no unported-decision marker.
/// Regression for the P3 stall: after vehicles began to fire, the first hit froze the lance's mechs for the rest of the mission.
#[test]
fn a_mech_that_was_hit_keeps_walking() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
    if let mc2_sim::object::Kind::Mech(m) = &mut sim.world.objects.get_mut(h).unwrap().kind {
        m.hit_from_front = true;
        m.hit_from_behind = true;
    }
    let start = pos(&sim, MECH);
    let mut unavailable = Vec::new();
    run(
        &mut sim,
        move_order(MECH, -320.0, 917.3333),
        300,
        |_, _, out| {
            unavailable.extend(movement_unavailable(out, MECH));
            true
        },
    );
    assert!(unavailable.is_empty(), "{unavailable:?}");
    assert!(
        dist(pos(&sim, MECH), start) > 50.0,
        "a hit mech walks away from {start:?}, now {:?}",
        pos(&sim, MECH)
    );
}

/// A goal on open water has no global area: the C++ raises the escape alarm and the escape search finds no cell with a route to
/// the goal's (missing) area, so the pilot gives up. The order must end without a panic and without the unported-escape marker,
/// and the mech must not wander off.
#[test]
fn move_order_onto_open_water_ends_without_unported_escape() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let mut unavailable = Vec::new();
    run(&mut sim, move_order(MECH, 0.0, 0.0), 600, |_, _, out| {
        unavailable.extend(movement_unavailable(out, MECH));
        true
    });
    assert!(unavailable.is_empty(), "{unavailable:?}");
    let p = pos(&sim, MECH);
    assert!(
        dist(p, Vec3::from_xyz(AIRFIELD.0, AIRFIELD.1, 0.0)) < 128.0,
        "{p:?}"
    );
}

/// `Mover::calcEscapePath`: a mech standing in deep water next to the shore, ordered to a far goal, gets an escape alarm
/// (`startArea == -1` on the long-range search) and leaves the water by the escape path; it then walks the long route.
/// The start is the deep cell (122, 219), one cell west of shallow water of area 31.
#[test]
fn mech_in_deep_water_escapes_to_the_shore_and_goes_on() {
    let Some(mut sim) = load() else { return };
    let start = sim.world.terrain.cell_to_world(122, 219);
    {
        let t = &sim.world.terrain;
        assert!(t.get_deep_water(122, 219));
        assert_eq!(t.move_data().global[0].calc_area(122, 219), -1);
        assert_eq!(t.move_data().global[0].calc_area(122, 220), 31);
    }
    place(&mut sim, MECH, start.x, start.y);
    let mut unavailable = Vec::new();
    let mut shore_frame = None;
    run(
        &mut sim,
        move_order(MECH, -320.0, 917.3333),
        600,
        |f, sim, out| {
            unavailable.extend(movement_unavailable(out, MECH));
            let t = &sim.world.terrain;
            let (r, c) = t.world_to_cell(&pos(sim, MECH));
            if shore_frame.is_none() && t.move_data().global[0].calc_area(r, c) >= 0 {
                shore_frame = Some(f);
            }
            true
        },
    );
    assert!(unavailable.is_empty(), "{unavailable:?}");
    let frame = shore_frame.expect("the mech leaves the water");
    eprintln!("on the ground map at frame {frame}");
    assert!(frame <= 60, "escape step is one cell: {frame}");
    // 600 frames in it is well on its way south-east around the lake, far from the start cell.
    assert!(
        dist(pos(&sim, MECH), start) > 600.0,
        "{:?}",
        pos(&sim, MECH)
    );
}

/// The escape search itself (`MoveMap::calcEscapePath` + `markEscapeGoals`): from a deep-water cell with shore beside it the first
/// cell is a ground-area cell one step away; from the middle of the lake every neighbour is blocked and there is no escape.
#[test]
fn escape_search_finds_the_shore_and_fails_in_open_water() {
    let Some(mut sim) = load() else { return };
    let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
    let goal = Vec3::from_xyz(-320.0, 917.3333, 0.0);
    let shore_start = sim.world.terrain.cell_to_world(122, 219);
    let found = mc2_sim::mover::planning::escape_search(&mut sim.world, h, shore_start, goal, 0)
        .expect("escape search");
    assert!(found.path.num_steps >= 1, "{:?}", found.path.num_steps);
    let last = found.path.steps[found.path.num_steps - 1].cell;
    let area = sim.world.terrain.move_data().global[0].calc_area(last[0], last[1]);
    assert!(
        area >= 0,
        "the escape ends on a cell with a ground area: {last:?}"
    );
    assert!((last[1] - 219).abs() <= 3 && (last[0] - 122).abs() <= 3);

    let lake_start = sim.world.terrain.cell_to_world(128, 200);
    assert_eq!(
        sim.world.terrain.move_data().global[0].calc_area(128, 200),
        -1
    );
    let none = mc2_sim::mover::planning::escape_search(&mut sim.world, h, lake_start, goal, 0)
        .expect("escape search");
    assert_eq!(none.path.num_steps, 0);
}

/// The same attack closes in on its target and fires: a mech 1000 units from the patrol's hovercraft (watch 4, then 5), ordered to
/// attack it, walks to weapon range (the mech's `max_range` 120 m = 601 world units, +50 for the stop margin) and its weapons fire
/// (`WeaponEffect` owned by the mech) before frame 600.
#[test]
fn attack_order_closes_to_weapon_range_and_fires_on_the_patrol() {
    for target in [4_u32, 5] {
        let Some(mut sim) = load() else { return };
        // On the dry ground east of the shore, south of the patrol's start (2880, 2112), 1000 units away.
        place(&mut sim, MECH, 3150.0, 900.0);
        let owner = {
            let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
            sim.world.objects.watch_id(h)
        };
        let start_distance = dist(pos(&sim, MECH), watch_pos(&sim, target));
        assert!(start_distance > 900.0, "{start_distance}");
        let mut first_shot = None;
        let mut unavailable = Vec::new();
        run(&mut sim, attack_order(MECH, target), 600, |f, sim, out| {
            unavailable.extend(movement_unavailable(out, MECH));
            let fired = out
                .events
                .iter()
                .any(|e| matches!(e, SimEvent::WeaponEffect { owner: o, .. } if *o == owner));
            if fired && first_shot.is_none() {
                first_shot = Some((f, dist(pos(sim, MECH), watch_pos(sim, target))));
                return false;
            }
            true
        });
        let (frame, range) =
            first_shot.unwrap_or_else(|| panic!("watch {target}: the mech never fired"));
        eprintln!("watch {target}: first shot at frame {frame}, range {range:.0}");
        assert!(range <= 601.0 + 50.0, "watch {target}: fired from {range}");
        assert!(unavailable.is_empty(), "{unavailable:?}");
    }
}

/// Attack order on a hovercraft that has run to the middle of the lake (watch 4 at about (40, -5)): the order no longer stalls in
/// `AttackGoalScoring`. The mech walks the long route round to the west shore (reaching the shore, NOT weapon range: the minimum
/// range over the run is recorded and no shot is possible, see `settled_patrol_is_out_of_reach_of_any_ground_cell`).
#[test]
fn attack_order_on_a_target_on_open_water_walks_to_the_shore() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let start_distance = dist(pos(&sim, MECH), watch_pos(&sim, 4));
    let mut unavailable = Vec::new();
    let owner = {
        let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
        sim.world.objects.watch_id(h)
    };
    let mut min_range = f32::MAX;
    let mut shots = 0_u32;
    let mut last_log = 0;
    run(&mut sim, attack_order(MECH, 4), 4500, |f, sim, out| {
        unavailable.extend(movement_unavailable(out, MECH));
        let r = dist(pos(sim, MECH), watch_pos(sim, 4));
        min_range = min_range.min(r);
        shots += out
            .events
            .iter()
            .filter(|e| matches!(e, SimEvent::WeaponEffect { owner: o, .. } if *o == owner))
            .count() as u32;
        if f >= last_log + 250 {
            last_log = f;
            eprintln!(
                "frame {f}: range {r:.0} mech {:?} target {:?}",
                pos(sim, MECH),
                watch_pos(sim, 4)
            );
        }
        true
    });
    assert!(unavailable.is_empty(), "{unavailable:?}");
    let end = dist(pos(&sim, MECH), watch_pos(&sim, 4));
    eprintln!("distance {start_distance:.0} -> {end:.0}, min {min_range:.0}, shots {shots}");
    assert!(min_range < 1000.0, "the chase closes in: {min_range}");
    // Scoring now centres on the pilot's goal as in the original (`calcMoveGoal`'s `moveGoal`), not on the target's current position;
    // against a patrolling target the chase closes in by about 550 over the run instead of 1000 (the 72 minimum range still shows it
    // reaching the target).
    assert!(end < start_distance - 500.0, "{start_distance} -> {end}");
}

/// A hovercraft at the lake centre is outside weapon range from every passable ground cell. This pins the geometry used by
/// `Mover::calcMoveGoal` (`mover.cpp:4567-4650`), without depending on the mission AI's retreat trigger.
#[test]
fn settled_patrol_is_out_of_reach_of_any_ground_cell() {
    let Some(mut sim) = load() else { return };
    let lake_centre = Vec3::from_xyz(0.0, 0.0, 0.0);
    for watch in [4_u32, 5] {
        let h = sim.world.objects.by_watch_id(WatchId(watch)).unwrap();
        mc2_sim::mover::set_position(&mut sim.world, h, lake_centre);
    }
    for target in [4_u32, 5] {
        let t = watch_pos(&sim, target);
        let mut nearest = f32::MAX;
        for r in 0..512_i32 {
            for c in 0..512_i32 {
                if !sim.world.terrain.in_bounds(r, c)
                    || sim.world.terrain.move_data().global[0].calc_area(r, c) < 0
                    || !sim.world.terrain.map().cell(r, c).passable()
                {
                    continue;
                }
                nearest = nearest.min(dist(sim.world.terrain.cell_to_world(r, c), t));
            }
        }
        eprintln!("watch {target} at {t:?}: nearest standable cell {nearest:.0}");
        assert!(nearest > 601.0, "watch {target}: {nearest}");
    }
}

/// Enemy mechs with jump jets (parts 706 and 713 have five) path like walkers whenever the walking route costs less than a jump step
/// (`DefaultMechJumpCost`), which is every route a patrol takes here: the searches that were `MovementUnavailable(JumpMovement)` for
/// them (one per frame while the lance was near) now run.
#[test]
fn jump_capable_enemy_mechs_path_when_walking_is_cheaper_than_a_jump() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let before = (pos(&sim, 706), pos(&sim, 713));
    let mut unavailable = Vec::new();
    run(&mut sim, attack_order(MECH, 4), 3000, |_, _, out| {
        unavailable.extend(movement_unavailable(out, 706));
        unavailable.extend(movement_unavailable(out, 713));
        true
    });
    assert!(unavailable.is_empty(), "{unavailable:?}");
    assert!(dist(pos(&sim, 706), before.0) > 100.0 || dist(pos(&sim, 713), before.1) > 100.0);
}

/// `JumpToPoint` flies and lands at its requested cell through the same step/order path the app uses.
#[test]
fn jump_order_leaves_ground_and_lands_at_its_goal() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let goal = Vec3::from_xyz(AIRFIELD.0, AIRFIELD.1 + 300.0, 0.0);
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, 0.0);
    order.move_params.way_path.points[0] = goal;
    order.move_params.jump = true;
    let command = PlayerCommand::Order {
        movers: vec![PartId(MECH)],
        order,
        queue: false,
    };
    let mut airborne_frames = 0_u64;
    let mut max_z_error = 0.0_f32;
    let mut jump_sound_count = 0;
    let mut unavailable = Vec::new();
    run(&mut sim, command, 240, |_, sim, out| {
        jump_sound_count += out.events.iter().filter(|event| matches!(event, SimEvent::Sound { id, at: Some(_), .. } if *id == mc2_sim::events::sfx::JUMPJETS)).count();
        let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
        airborne_frames += u64::from(matches!(
            sim.world.objects.get(h).map(|o| &o.kind),
            Some(mc2_sim::object::Kind::Mech(m)) if m.in_jump
        ));
        unavailable.extend(movement_unavailable(out, MECH));
        // C++ mech.cpp:5992: world z follows the terrain every frame, mid-flight included.
        let p = pos(sim, MECH);
        let ground = sim.world.terrain.elevation(&p);
        max_z_error = max_z_error.max((p.z - ground).abs());
        true
    });
    let landed = pos(&sim, MECH);
    assert!(
        max_z_error <= 1.0,
        "mid-flight z left the terrain by {max_z_error}"
    );
    assert!(unavailable.is_empty(), "{unavailable:?}");
    assert_eq!(
        jump_sound_count, 1,
        "jump emits one positional JUMPJETS sound"
    );
    assert!(
        (164..=170).contains(&airborne_frames),
        "jump flight lasts {airborne_frames} frames; mission mech 512's shipped jump gesture gives a 167-frame flight window at 30 fps (±3 frames for transition ownership)"
    );
    assert!(
        dist(landed, goal) <= 1.0,
        "landed at {landed:?}, goal {goal:?}"
    );
}

/// A jump to the mech's own cell stays finite (the C++ divides by the zero-length vector).
#[test]
fn zero_distance_jump_keeps_a_finite_position() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let start = pos(&sim, MECH);
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, 0.0);
    order.move_params.way_path.points[0] = start;
    order.move_params.jump = true;
    let command = PlayerCommand::Order {
        movers: vec![PartId(MECH)],
        order,
        queue: false,
    };
    run(&mut sim, command, 240, |_, sim, _| {
        let p = pos(sim, MECH);
        assert!(
            p.x.is_finite() && p.y.is_finite() && p.z.is_finite(),
            "{p:?}"
        );
        true
    });
    assert!(dist(pos(&sim, MECH), start) <= 1.0);
}

/// An out-of-range point is rejected before takeoff; it never becomes a flying mover.
#[test]
fn jump_order_out_of_range_does_not_leave_the_ground() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let start = pos(&sim, MECH);
    let goal = Vec3::from_xyz(AIRFIELD.0, AIRFIELD.1 + 5000.0, 0.0);
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, 0.0);
    order.move_params.way_path.points[0] = goal;
    order.move_params.jump = true;
    run(
        &mut sim,
        PlayerCommand::Order {
            movers: vec![PartId(MECH)],
            order,
            queue: false,
        },
        120,
        |_, sim, _| {
            let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
            assert!(!matches!(
                sim.world.objects.get(h).map(|o| &o.kind),
                Some(mc2_sim::object::Kind::Mech(m)) if m.in_jump
            ));
            true
        },
    );
    assert!(dist(pos(&sim, MECH), start) <= 128.0);
}

/// A visible shut-down home mech powers up and rejoins the home roster on its next sim update.
#[test]
fn visible_shutdown_mech_powers_up() {
    let Some(mut sim) = load() else { return };
    let h = sim
        .world
        .objects
        .find_by_part_id(PartId(706))
        .expect("shipped enemy mech");
    let home_position = pos(&sim, MECH);
    mc2_sim::mover::set_position(&mut sim.world, h, home_position);
    {
        let obj = sim.world.objects.get_mut(h).unwrap();
        obj.c.status = mc2_sim::object::ObjectStatus::ShutDown;
        if let mc2_sim::object::Kind::Mech(m) = &mut obj.kind {
            m.m.flags.on_gui = false;
        }
    }
    sim.step(FrameInput::fixed(30, 1, Rc::new(AllVisible), Vec::new()))
        .expect("complete load steps");
    let obj = sim.world.objects.get(h).unwrap();
    assert_eq!(obj.c.status, mc2_sim::object::ObjectStatus::Normal);
    assert!(matches!(&obj.kind, mc2_sim::object::Kind::Mech(m) if m.m.flags.on_gui));
}

/// `BattleMech::updateMovement` (mech.cpp:4866-4882) clears `failedPilotingCheck` after one `setGestureGoal(FALLEN_*)`; the
/// appearance refuses it while the jump goal is current, so a failed landing check drops the knockdown (oracle, mc2_24 frame 2035).
/// The check is consumed on the first attempt, the mech never falls and keeps standing/walking.
#[test]
fn failed_landing_piloting_check_is_consumed_without_a_knockdown() {
    let Some(mut sim) = load() else { return };
    place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
    let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
    if let mc2_sim::object::Kind::Mech(m) = &mut sim.world.objects.get_mut(h).unwrap().kind {
        m.loadout.body[6].cur_internal_structure = 0.0;
    }
    let goal = Vec3::from_xyz(AIRFIELD.0, AIRFIELD.1 + 300.0, 0.0);
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, 0.0);
    order.move_params.way_path.points[0] = goal;
    order.move_params.jump = true;
    let command = PlayerCommand::Order {
        movers: vec![PartId(MECH)],
        order,
        queue: false,
    };
    let mut saw_fall = false;
    let mut saw_check = false;
    let mut saw_get_up = false;
    for frame in 1..=600 {
        let commands = if frame == 2 {
            vec![command.clone()]
        } else {
            Vec::new()
        };
        sim.step(FrameInput::fixed(30, frame, Rc::new(AllVisible), commands))
            .expect("complete load steps");
        let gesture = sim.world.appearance.current_gesture_id(h);
        saw_fall |= matches!(
            gesture,
            mc2_sim::appearance::tables::gesture::FALLEN_FORWARD
                | mc2_sim::appearance::tables::gesture::FALLEN_BACKWARD
        );
        saw_get_up |= gesture == mc2_sim::appearance::tables::gesture::GET_UP;
        saw_check |= matches!(&sim.world.objects.get(h).unwrap().kind, mc2_sim::object::Kind::Mech(m) if m.failed_piloting_check);
    }
    assert!(
        saw_check,
        "the destroyed-leg landing never failed its piloting check"
    );
    assert!(
        !saw_fall,
        "the oracle drops the knockdown of a failed landing check (mech.cpp:4866-4882)"
    );
    assert!(!saw_get_up);
    assert!(
        !matches!(&sim.world.objects.get(h).unwrap().kind, mc2_sim::object::Kind::Mech(m) if m.failed_piloting_check),
        "one attempt consumes the check"
    );
}

/// A jump that lands on another mover through the production collision path: against an enemy, death from above is applied
/// exactly once to each side (one hit on the target by the jumper and one on the jumper by the target, each
/// `tonnage / 10 + 0.5` of the target); against a friendly mover nobody is hit. The jumper is displaced off the goal either way.
/// No contact is reported as unavailable any more.
#[test]
fn jump_landing_on_another_mover_applies_death_from_above_once() {
    for want_enemy in [true, false] {
        let Some(mut sim) = load() else { return };
        let my_team = {
            let h = sim.world.objects.find_by_part_id(PartId(MECH)).unwrap();
            sim.world.objects.get(h).unwrap().team_id()
        };
        let other = (1..2000).find(|&p| {
            p != MECH
                && sim
                    .world
                    .objects
                    .find_by_part_id(PartId(p))
                    .and_then(|h| sim.world.objects.get(h))
                    .is_some_and(|o| {
                        matches!(o.kind, mc2_sim::object::Kind::Mech(_))
                            && (o.team_id() != my_team) == want_enemy
                    })
        });
        let other = other.expect("a mover of the wanted alignment exists");
        let (mech_h, other_h) = (
            sim.world.objects.find_by_part_id(PartId(MECH)).unwrap(),
            sim.world.objects.find_by_part_id(PartId(other)).unwrap(),
        );
        let (mech_wid, other_wid) = (
            sim.world.objects.watch_id(mech_h),
            sim.world.objects.watch_id(other_h),
        );
        sim.world.objects.hit_sink = Some(Vec::new());
        place(&mut sim, MECH, AIRFIELD.0, AIRFIELD.1);
        let goal = Vec3::from_xyz(AIRFIELD.0, AIRFIELD.1 + 300.0, 0.0);
        let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::JumpToPoint, false, 0.0);
        order.move_params.way_path.points[0] = goal;
        order.move_params.jump = true;
        let command = PlayerCommand::Order {
            movers: vec![PartId(MECH)],
            order,
            queue: false,
        };
        let other_handle = sim.world.objects.find_by_part_id(PartId(other)).unwrap();
        let other_team = sim.world.objects.get(other_handle).unwrap().team_id();
        if other_team >= 0 && other_team != my_team {
            sim.world.pilots.brains_enabled[other_team as usize] = false;
        }
        let scenario_time = sim.world.clock.scenario_time;
        mc2_sim::ai::handle_tactical_order(
            &mut sim.world,
            other_handle,
            TacticalOrder::new(OrderOrigin::Player, OrderCode::Stop, false, scenario_time),
            0,
            false,
        );
        place(&mut sim, other, goal.x, goal.y);
        let mut unavailable = false;
        run(&mut sim, command, 240, |_, _, out| {
            unavailable |= !movement_unavailable(out, MECH).is_empty()
                || !movement_unavailable(out, other).is_empty();
            true
        });
        assert!(!unavailable, "a jump contact was reported unavailable");
        let tonnage = sim.world.objects.get(other_h).unwrap().c.tonnage;
        let want = mc2_sim::combat::WeaponShotInfo::init(
            &sim.world,
            other_wid,
            -1,
            tonnage / 10.0 + 0.5,
            0,
            0.0,
        )
        .damage;
        let want_on_target = mc2_sim::combat::WeaponShotInfo::init(
            &sim.world,
            mech_wid,
            -1,
            tonnage / 10.0 + 0.5,
            0,
            0.0,
        )
        .damage;
        let hits = sim.world.objects.hit_sink.take().unwrap();
        let count = |target: mc2_sim::ids::ObjectHandle, attacker: WatchId, damage: f32| {
            hits.iter()
                .filter(|h| h.target == target && h.attacker == attacker && h.damage == damage)
                .count()
        };
        let (on_target, on_jumper) = (
            count(other_h, mech_wid, want_on_target),
            count(mech_h, other_wid, want),
        );
        if want_enemy {
            assert_eq!(on_target, 1, "death from above on the target: once");
            assert_eq!(on_jumper, 1, "the jumper's own damage: once");
        } else {
            assert_eq!((on_target, on_jumper), (0, 0), "a friendly landing hurts");
        }
        // the landing is displaced off the goal (bounced to a neighbouring cell), never left on top of the other mover
        let d = dist(pos(&sim, MECH), goal);
        assert!(d > 1.0 && d < 200.0, "jumper not displaced: {d}");
    }
}
