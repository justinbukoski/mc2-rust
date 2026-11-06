// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Capture order outcomes on the shipped mc2_17 mission.
//!
//! `Sim::step` is the entry point for both cases. The fixture changes only the tested mover
//! positions so the tests exercise the same capture order on a known unblocked/blocked setup.

use mc2_sim::combat::{loadout_of, HOME_COMMANDER};
use mc2_sim::command::{AttackMethod, OrderCode, OrderOrigin, PlayerCommand, TacticalOrder};
use mc2_sim::ids::{PartId, WatchId};
use mc2_sim::math::Vec3;
use mc2_sim::mission::init::{load_world, LoadSpec};
use mc2_sim::object::{flags, Kind};
use mc2_sim::rng::RngMode;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim};
use std::rc::Rc;

fn sim() -> Option<Sim> {
    sim_for("mc2_17")
}

fn sim_for(mission: &str) -> Option<Sim> {
    let root = std::env::var_os("MC2_DATA").map(std::path::PathBuf::from)?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount shipped game data");
    let loaded = load_world(
        &mut vfs,
        &LoadSpec::quickstart(mission, 1, RngMode::LegacyMsvc),
    )
    .expect("mission load");
    assert!(loaded.complete, "missing hooks: {:?}", loaded.missing_hooks);
    Some(Sim::new(loaded))
}

fn capture_order(part: i32, target: WatchId) -> PlayerCommand {
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::Capture, false, 0.0);
    order.target = target;
    order.attack_params.method = AttackMethod::Ramming;
    order.attack_params.pursue = true;
    PlayerCommand::Order {
        movers: vec![PartId(part)],
        order,
        queue: false,
    }
}

fn location_order(part: i32, code: OrderCode, location: Vec3) -> PlayerCommand {
    let mut order = TacticalOrder::new(OrderOrigin::Player, code, false, 0.0);
    order.move_params.way_path.points[0] = location;
    order.move_params.way_path.num_points = 1;
    PlayerCommand::Order {
        movers: vec![PartId(part)],
        order,
        queue: false,
    }
}

fn step(sim: &mut Sim, frame: u64, command: PlayerCommand) -> Vec<mc2_sim::SimEvent> {
    sim.step(FrameInput::fixed(
        30,
        frame,
        Rc::new(AllVisible),
        vec![command],
    ))
    .expect("complete mc2_17 step")
    .events
}

fn capture_pair(sim: &mut Sim) -> (mc2_sim::ObjectHandle, mc2_sim::ObjectHandle) {
    let targets = sim.world.objects.buildings.clone();
    let movers = sim.world.objects.movers.clone();
    for target in targets {
        let Some(target_obj) = sim.world.objects.get(target) else {
            continue;
        };
        if !target_obj.c.flag(flags::CAPTURABLE)
            || target_obj.c.is_destroyed()
            || !target_obj.c.awake()
            || target_obj.team_id() == sim.world.teams.home as i32
        {
            continue;
        }
        for &mover in &movers {
            let Some(mech) = sim.world.objects.get(mover) else {
                continue;
            };
            if !matches!(mech.kind, Kind::Mech(_)) || mech.commander_id() != HOME_COMMANDER {
                continue;
            }
            if mc2_sim::ai::capture::is_captureable(&sim.world, target, sim.world.teams.home as i32)
            {
                return (target, mover);
            }
        }
    }
    panic!("mc2_17 has no capturable building and player mech pair");
}

fn place(sim: &mut Sim, mover: mc2_sim::ObjectHandle, position: Vec3) {
    mc2_sim::mover::set_position(&mut sim.world, mover, position);
}

fn expected_adjacent_cell(
    sim: &Sim,
    building: mc2_sim::ObjectHandle,
    level: usize,
) -> Option<(i32, i32)> {
    let cells = sim
        .world
        .objects
        .get(building)?
        .structure()?
        .cells_covered
        .clone();
    let map = &sim.world.terrain.move_data().global[level];
    for (row, col) in cells {
        let (row, col) = (i32::from(row), i32::from(col));
        for (r, c) in [
            (row - 1, col),
            (row, col + 1),
            (row + 1, col),
            (row, col - 1),
        ] {
            if map.calc_area(r, c) >= 0 {
                return Some((r, c));
            }
        }
    }
    None
}

fn synthetic_global_map(
    width: i32,
    height: i32,
    blocked_cells: &[(i32, i32)],
) -> mc2_path::GlobalMap {
    let mut area_map = vec![0; (width * height) as usize];
    for &(row, col) in blocked_cells {
        if (0..height).contains(&row) && (0..width).contains(&col) {
            area_map[(row * width + col) as usize] = -1;
        }
    }
    mc2_path::GlobalMap::from_parts(mc2_path::GlobalMapParts {
        width,
        height,
        area_map,
        areas: vec![mc2_path::GlobalMapArea {
            sector_r: 0,
            sector_c: 0,
            area_type: mc2_path::AreaType::Normal,
            num_doors: 0,
            owner_wid: -1,
            team_id: -1,
            off_map: false,
            open: true,
        }],
        doors: vec![],
        links: vec![],
    })
    .expect("valid one-area test map")
}

#[test]
fn player_mech_captures_mc2_17_building_when_unblocked() {
    let Some(mut sim) = sim() else { return };
    let (target, capturer) = capture_pair(&mut sim);
    let target_obj = sim.world.objects.get(target).unwrap();
    let target_pos = target_obj.c.position;
    let target_team = target_obj.team_id();
    let part = sim.world.objects.get(capturer).unwrap().c.part_id.0;
    let target_wid = sim.world.objects.watch_id(target);
    place(&mut sim, capturer, target_pos);
    assert!(mc2_sim::ai::capture::blockers(&mut sim.world, target, capturer).is_empty());

    step(&mut sim, 1, capture_order(part, target_wid));

    assert_eq!(
        sim.world.objects.get(target).unwrap().team_id(),
        sim.world.teams.home as i32,
        "building team changes from {target_team} after capture"
    );
}

/// tacordr.cpp:1783: a capture order issued from afar completes in the status check once the
/// unit is within 30 of the target (the execute path only captures when already there).
#[test]
fn player_mech_capture_completes_when_the_unit_arrives_after_the_order() {
    let Some(mut sim) = sim() else { return };
    let (target, capturer) = capture_pair(&mut sim);
    let target_pos = sim.world.objects.get(target).unwrap().c.position;
    let part = sim.world.objects.get(capturer).unwrap().c.part_id.0;
    let target_wid = sim.world.objects.watch_id(target);
    let far = Vec3 {
        x: target_pos.x + 150.0,
        ..target_pos
    };
    place(&mut sim, capturer, far);
    assert!(mc2_sim::ai::capture::blockers(&mut sim.world, target, capturer).is_empty());

    step(&mut sim, 1, capture_order(part, target_wid));
    let home = sim.world.teams.home as i32;
    assert_ne!(sim.world.objects.get(target).unwrap().team_id(), home);

    for frame in 2..1500 {
        let out = sim
            .step(FrameInput::fixed(30, frame, Rc::new(AllVisible), vec![]))
            .expect("step");
        if sim.world.objects.get(target).unwrap().team_id() == home {
            let slot = mc2_sim::ai::warrior::slot_of(&sim.world, capturer).unwrap();
            assert_ne!(
                sim.world.pilots.get(slot).unwrap().cur_order.code,
                OrderCode::Capture,
                "the capture order completed after the mech arrived"
            );
            assert!(
                out.events.iter().any(|event| matches!(
                    event,
                    mc2_sim::SimEvent::Radio {
                        pilot,
                        message: mc2_sim::ai::orders::radio::CAPTURED_BUILDING,
                        ..
                    } if *pilot == u32::from(slot)
                )),
                "the completing approach emitted captured-building feedback"
            );
            return;
        }
    }
    let d = mc2_sim::ai::mover_ops::distance_from(
        &sim.world,
        capturer,
        &sim.world.objects.get(target).unwrap().c.position,
    );
    panic!("never captured; distance {d}");
}

#[test]
fn player_mech_capture_is_refused_when_an_enemy_mover_blocks_mc2_17_building() {
    let Some(mut sim) = sim() else { return };
    let (target, capturer) = capture_pair(&mut sim);
    let target_pos = sim.world.objects.get(target).unwrap().c.position;
    let target_team = sim.world.objects.get(target).unwrap().team_id();
    let part = sim.world.objects.get(capturer).unwrap().c.part_id.0;
    let target_wid = sim.world.objects.watch_id(target);
    place(
        &mut sim,
        capturer,
        Vec3 {
            x: target_pos.x + 150.0,
            ..target_pos
        },
    );
    assert!(mc2_sim::ai::capture::blockers(&mut sim.world, target, capturer).is_empty());

    step(&mut sim, 1, capture_order(part, target_wid));

    let blocker = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            h != capturer
                && sim.world.objects.get(h).is_some_and(|o| {
                    o.team_id() != sim.world.teams.home as i32
                        && !o.c.is_disabled()
                        && o.c.awake()
                        && loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
                })
        })
        .expect("mc2_17 armed enemy mover");
    place(&mut sim, blocker, target_pos);
    place(&mut sim, capturer, target_pos);
    assert!(!mc2_sim::ai::capture::blockers(&mut sim.world, target, capturer).is_empty());

    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, capturer).unwrap();
    let mut feedback = Vec::new();
    for frame in 2..8 {
        let out = sim
            .step(FrameInput::fixed(30, frame, Rc::new(AllVisible), vec![]))
            .expect("complete blocked-capture status step");
        feedback.extend(out.events);
    }

    assert_eq!(
        sim.world.objects.get(target).unwrap().team_id(),
        target_team
    );
    assert!(
        feedback.iter().any(|event| matches!(
            event,
            mc2_sim::SimEvent::Radio {
                pilot,
                message: mc2_sim::ai::orders::radio::CANNOT_CAPTURE,
                ..
            } if *pilot == u32::from(slot)
        )),
        "the blocked capture emitted cannot-capture feedback"
    );
}

#[test]
fn capture_natives_report_and_set_cpp_capture_state() {
    let Some(mut sim) = sim() else { return };
    let (target, _) = capture_pair(&mut sim);
    let part = sim.world.objects.get(target).unwrap().c.part_id.0;
    let home = sim.world.teams.home as i32;

    let mut bindings = mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world);
    assert!(mc2_sim::mission::natives_capture::is_capturable(
        &bindings, part, home
    ));
    mc2_sim::mission::natives_capture::set_capturable(&mut bindings, part, false);
    assert!(mc2_sim::mission::natives_capture::is_capturable(
        &bindings, part, home
    ));
    mc2_sim::mission::natives_capture::set_captured(&mut bindings, part);
    assert_eq!(
        mc2_sim::mission::natives_capture::is_captured(&bindings, part),
        1
    );
}

#[test]
fn invalidated_capture_order_completes_on_the_next_sim_step() {
    let Some(mut sim) = sim() else { return };
    let (target, capturer) = capture_pair(&mut sim);
    let target_pos = sim.world.objects.get(target).unwrap().c.position;
    let target_wid = sim.world.objects.watch_id(target);
    let part = sim.world.objects.get(capturer).unwrap().c.part_id.0;
    place(
        &mut sim,
        capturer,
        Vec3 {
            x: target_pos.x + 100.0,
            y: target_pos.y,
            z: target_pos.z,
        },
    );

    step(&mut sim, 1, capture_order(part, target_wid));
    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, capturer).unwrap();
    assert_eq!(
        sim.world.pilots.get(slot).unwrap().cur_order.code,
        OrderCode::Capture,
        "the distant target leaves a live capture order"
    );

    sim.world
        .objects
        .get_mut(target)
        .unwrap()
        .c
        .set_flag(flags::CAPTURABLE, false);
    sim.step(FrameInput::fixed(30, 2, Rc::new(AllVisible), vec![]))
        .expect("complete mc2_17 step after capture eligibility changes");

    assert_ne!(
        sim.world.pilots.get(slot).unwrap().cur_order.code,
        OrderCode::Capture,
        "the C++ executes the now-ineligible order successfully and finishes it"
    );
}

#[test]
fn team_targeting_native_uses_watch_ids_and_except_watch_id() {
    let Some(mut sim) = sim() else { return };
    let target = sim.world.objects.movers[0];
    let attacker = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            sim.world.objects.get(h).is_some_and(|o| {
                o.commander_id() == HOME_COMMANDER && matches!(&o.kind, Kind::Mech(_))
            })
        })
        .expect("mc2_17 has a player mech");
    let target_part = sim.world.objects.get(target).unwrap().c.part_id.0;
    let target_watch = sim.world.objects.watch_id(target);
    let attacker_watch = sim.world.objects.watch_id(attacker);
    assert_ne!(
        target_part as u32, target_watch.0,
        "fixture distinguishes part and watch IDs"
    );
    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, attacker).unwrap();
    mc2_sim::ai::warrior::set_last_target(&mut sim.world, slot, Some(target), true, false);
    let team = 500 + sim.world.teams.home as i32;
    let mut bindings = mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world);

    assert!(mc2_sim::mission::natives_capture::is_team_targeting(
        &mut bindings,
        team,
        target_watch.0 as i32,
        0
    ));
    assert!(!mc2_sim::mission::natives_capture::is_team_targeting(
        &mut bindings,
        team,
        target_watch.0 as i32,
        attacker_watch.0 as i32
    ));
}

#[test]
fn team_zero_targeting_native_resolves_object_handles_not_part_ids() {
    let Some(mut sim) = sim() else { return };
    let target = sim.world.objects.movers[0];
    let target_part = sim.world.objects.get(target).unwrap().c.part_id.0;
    assert_ne!(target.0, target_part as u32);
    for obj in sim.world.objects.list.iter_mut().flatten() {
        obj.c.num_attackers = 0;
    }
    sim.world.objects.get_mut(target).unwrap().c.num_attackers = 1;
    let handle_id = target.0 as i32;
    let mut bindings = mc2_sim::mission::bindings::SimBindings::mission(&mut sim.world);

    assert!(mc2_sim::mission::natives_capture::is_team_targeting(
        &mut bindings,
        0,
        handle_id,
        0
    ));
    assert!(!mc2_sim::mission::natives_capture::is_team_targeting(
        &mut bindings,
        0,
        target_part,
        0
    ));
}

#[test]
fn captured_resource_vehicle_awards_its_type_points_after_turn_ten() {
    let mut fixture = None;
    for mission in 1..=24 {
        let Some(mut candidate) = sim_for(&format!("mc2_{mission:02}")) else {
            return;
        };
        let home = candidate.world.teams.home as i32;
        let Some(capturer) = candidate.world.objects.movers.iter().copied().find(|&h| {
            candidate.world.objects.get(h).is_some_and(|o| {
                matches!(&o.kind, Kind::Mech(_)) && o.commander_id() == HOME_COMMANDER
            })
        }) else {
            continue;
        };
        let vehicles: Vec<_> = candidate
            .world
            .objects
            .vehicles
            .iter()
            .copied()
            .filter(|&h| {
                let Some(o) = candidate.world.objects.get(h) else {
                    return false;
                };
                candidate
                    .world
                    .objects
                    .types
                    .as_ref()
                    .and_then(|types| types.get(o.c.type_handle as u32))
                    .is_some_and(|ty| {
                        matches!(&ty.data, mc2_sim_data::objtype::ObjectTypeData::GroundVehicle(v) if v.resource_points > 0)
                    })
                    && o.team_id() != home
                    && o.c.flag(flags::CAPTURABLE)
                    && o.c.awake()
                    && !o.c.is_destroyed()
            })
            .collect();
        for target in vehicles {
            let position = candidate.world.objects.get(target).unwrap().c.position;
            place(&mut candidate, capturer, position);
            if !mc2_sim::ai::capture::blockers(&mut candidate.world, target, capturer).is_empty() {
                continue;
            }
            let type_handle = candidate.world.objects.get(target).unwrap().c.type_handle as u32;
            let reward = match &candidate
                .world
                .objects
                .types
                .as_ref()
                .unwrap()
                .get(type_handle)
                .unwrap()
                .data
            {
                mc2_sim_data::objtype::ObjectTypeData::GroundVehicle(v) => v.resource_points,
                _ => unreachable!(),
            };
            fixture = Some((candidate, target, capturer, reward));
            break;
        }
        if fixture.is_some() {
            break;
        }
    }
    let Some((mut sim, target, capturer, reward)) = fixture else {
        panic!("no shipped mission has a reachable capturable resource vehicle and a home mech");
    };
    let home = sim.world.teams.home as i32;
    let before = sim.world.objects.resource_points_gained;
    let balance_before = sim.world.mission.resource_points;
    sim.world.clock.turn = 10;
    let part = sim.world.objects.get(capturer).unwrap().c.part_id.0;
    let target_watch = sim.world.objects.watch_id(target);

    step(&mut sim, 11, capture_order(part, target_watch));

    assert_eq!(sim.world.objects.get(target).unwrap().team_id(), home);
    assert_eq!(sim.world.objects.resource_points_gained, before + reward);
    assert_eq!(sim.world.mission.resource_points, balance_before + reward);
}

#[test]
fn guard_order_persists_and_engages_a_nearby_enemy() {
    let Some(mut sim) = sim() else { return };
    let (_, guard) = capture_pair(&mut sim);
    let home = sim.world.teams.home as i32;
    let enemy = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            h != guard
                && sim.world.objects.get(h).is_some_and(|o| {
                    o.team_id() != home
                        && !o.c.is_disabled()
                        && o.c.awake()
                        && loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
                })
        })
        .expect("mc2_17 has an armed enemy mover");
    let enemy_position = sim.world.objects.get(enemy).unwrap().c.position;
    let guard_part = sim.world.objects.get(guard).unwrap().c.part_id.0;
    let enemy_watch = sim.world.objects.watch_id(enemy);
    place(&mut sim, guard, enemy_position);

    step(
        &mut sim,
        1,
        location_order(guard_part, OrderCode::Guard, enemy_position),
    );
    for frame in 2..=60 {
        sim.step(FrameInput::fixed(30, frame, Rc::new(AllVisible), vec![]))
            .expect("complete guard engagement step");
    }

    let slot = mc2_sim::ai::warrior::slot_of(&sim.world, guard).expect("player pilot slot");
    let pilot = sim.world.pilots.get(slot).unwrap();
    assert_eq!(pilot.cur_order.code, OrderCode::Guard);
    assert_eq!(pilot.last_target.0, enemy_watch);
}

#[test]
fn level_one_adjacent_goal_uses_the_movers_map() {
    let Some(mut sim) = sim() else { return };
    let mover = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&m| {
            sim.world
                .objects
                .get(m)
                .is_some_and(|object| object.move_level() == 1)
        })
        .expect("mc2_17 hover mover");
    let target = sim
        .world
        .objects
        .buildings
        .iter()
        .copied()
        .find(|&building| {
            sim.world
                .objects
                .get(building)
                .and_then(|object| object.structure())
                .is_some_and(|structure| !structure.cells_covered.is_empty())
        })
        .expect("mc2_17 building with map cells");
    let building_cells = sim
        .world
        .objects
        .get(target)
        .unwrap()
        .structure()
        .unwrap()
        .cells_covered
        .clone();
    let adjacent_cells: Vec<_> = building_cells
        .iter()
        .flat_map(|&(row, col)| {
            let (row, col) = (i32::from(row), i32::from(col));
            [
                (row - 1, col),
                (row, col + 1),
                (row + 1, col),
                (row, col - 1),
            ]
        })
        .collect();
    let first_goal = adjacent_cells[0];
    let second_goal = adjacent_cells
        .iter()
        .copied()
        .find(|&cell| cell != first_goal)
        .expect("building has a second adjacent map cell");
    let width = sim.world.terrain.move_data().global[0].width();
    let height = sim.world.terrain.move_data().global[0].height();
    // Shipped missions currently give hover and ground the same adjacent goal here. This
    // fixture changes only the upstream global-map cells to force the C++ move-level choice:
    // both maps retain one connected area, while only the selected neighbor remains open.
    let mut ground_blocked = adjacent_cells.clone();
    ground_blocked.extend(
        building_cells
            .iter()
            .map(|&(row, col)| (i32::from(row), i32::from(col))),
    );
    ground_blocked.retain(|&cell| cell != second_goal);
    let mut hover_blocked = adjacent_cells;
    hover_blocked.extend(
        building_cells
            .iter()
            .map(|&(row, col)| (i32::from(row), i32::from(col))),
    );
    hover_blocked.retain(|&cell| cell != first_goal);
    let ground = synthetic_global_map(width, height, &ground_blocked);
    let hover = synthetic_global_map(width, height, &hover_blocked);
    let data = sim.world.terrain.move_data_mut();
    data.global[0] = ground;
    data.global[1] = hover;
    assert_eq!(
        expected_adjacent_cell(&sim, target, 0),
        Some(second_goal),
        "ground uses the ground map's configured neighbor"
    );
    assert_eq!(expected_adjacent_cell(&sim, target, 1), Some(first_goal));
    let selected = mc2_sim::ai::goal::calc_adjacent_area_cell(&sim.world, target, mover, -1)
        .expect("the selector finds a level-one adjacent cell");
    assert_eq!(
        sim.world.terrain.world_to_cell(&selected),
        first_goal,
        "the selector follows the independently calculated level-one neighbor"
    );
}

#[test]
fn move_to_building_reaches_a_walkable_adjacent_cell_on_mc2_17() {
    let Some(mut sim) = sim() else { return };
    let mover = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&mover| {
            sim.world.objects.get(mover).is_some_and(|object| {
                matches!(object.kind, Kind::Mech(_)) && object.commander_id() == HOME_COMMANDER
            })
        })
        .expect("mc2_17 has a player mech");
    let target_and_goal = sim
        .world
        .objects
        .buildings
        .iter()
        .copied()
        .find_map(|target| {
            mc2_sim::ai::goal::calc_adjacent_area_cell(&sim.world, target, mover, -1)
                .map(|goal| (target, goal))
        });
    let (target, goal) = target_and_goal.expect("mc2_17 has an adjacent building goal");
    let (goal_row, goal_col) = sim.world.terrain.world_to_cell(&goal);
    let level = sim
        .world
        .objects
        .get(mover)
        .unwrap()
        .move_level()
        .clamp(0, 2) as usize;
    let goal_area = sim.world.terrain.move_data().global[level].calc_area(goal_row, goal_col);
    assert!(goal_area >= 0, "adjacent goal is in an open movement area");
    let start = (goal_row - 3..=goal_row + 3)
        .flat_map(|row| (goal_col - 3..=goal_col + 3).map(move |col| (row, col)))
        .find(|&(row, col)| {
            (row - goal_row).abs().max((col - goal_col).abs()) == 3
                && sim.world.terrain.move_data().global[level].calc_area(row, col) == goal_area
        })
        .map(|(row, col)| sim.world.terrain.cell_to_world(row, col))
        .expect("adjacent goal area has a nearby start cell");
    place(&mut sim, mover, start);
    let part = sim.world.objects.get(mover).unwrap().c.part_id.0;
    let target_wid = sim.world.objects.watch_id(target);
    let mut order = TacticalOrder::new(OrderOrigin::Player, OrderCode::MoveToObject, false, 0.0);
    order.target = target_wid;
    step(
        &mut sim,
        1,
        PlayerCommand::Order {
            movers: vec![PartId(part)],
            order,
            queue: false,
        },
    );
    for frame in 2..=301 {
        sim.step(FrameInput::fixed(30, frame, Rc::new(AllVisible), vec![]))
            .expect("complete mc2_17 object-move step");
    }
    let reached = sim.world.objects.get(mover).unwrap().c.cell;
    let (reached_row, reached_col) = (i32::from(reached.0), i32::from(reached.1));
    let reached_area =
        sim.world.terrain.move_data().global[level].calc_area(reached_row, reached_col);
    assert!(
        (reached_row - goal_row).abs().max((reached_col - goal_col).abs()) <= 1
            && reached_area == goal_area,
        "move-to-building reaches the selected adjacent cell: reached ({reached_row}, {reached_col}), selected goal ({goal_row}, {goal_col}), areas {reached_area}/{goal_area}"
    );
}
