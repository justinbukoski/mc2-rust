// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Ground vehicle fire through the production mission, appearance, and `Sim::step` paths.

mod common;

use mc2_sim::combat::fire::fire_weapon;
use mc2_sim::command::{PlayerCommand, ScriptedOrder};
use mc2_sim::ids::{ObjectHandle, WatchId};
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim, SimEvent, WeaponImpact};
use std::collections::BTreeSet;
use std::rc::Rc;

fn health(sim: &Sim, h: ObjectHandle) -> f32 {
    let loadout = mc2_sim::combat::loadout_of(sim.world.objects.get(h).unwrap()).unwrap();
    loadout.armor.iter().map(|a| a.0).sum::<f32>()
        + loadout
            .body
            .iter()
            .map(|b| b.cur_internal_structure)
            .sum::<f32>()
}

fn bolt_ids(sim: &Sim, owner: WatchId) -> BTreeSet<i32> {
    sim.world
        .objects
        .weapons
        .iter()
        .filter_map(|&h| {
            let object = sim.world.objects.get(h)?;
            match &object.kind {
                Kind::WeaponBolt(bolt) if bolt.owner == owner => {
                    Some(bolt.weapon_shot.as_ref()?.master_id)
                }
                _ => None,
            }
        })
        .collect()
}

fn target_bolts(sim: &Sim, owner: WatchId, target: WatchId) -> BTreeSet<(i32, i32, bool)> {
    sim.world
        .objects
        .weapons
        .iter()
        .filter_map(|&h| {
            let object = sim.world.objects.get(h)?;
            match &object.kind {
                Kind::WeaponBolt(bolt) if bolt.owner == owner && bolt.target == target => Some((
                    bolt.effect_id,
                    bolt.weapon_shot.as_ref()?.master_id,
                    bolt.hit_target,
                )),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn mc2_01_common_vehicle_weapon_data_matches_cpp_tables() {
    let Some(mut fixture) = common::fixture("mc2_01", "pcg") else {
        return;
    };

    // Mission records: the two largest enemy ground-vehicle groups are Infantry (type 241, 8
    // parts) and LRMC (type 201, 7 parts). Read each type's inventory packet independently from
    // object2.pak, as GroundVehicle::init does, rather than inferring its weapons from fire events.
    let mission = fixture.world.mission.file.as_ref().unwrap();
    for (object_number, expected) in [(241, 8), (201, 7)] {
        assert_eq!(
            mission
                .parts
                .iter()
                .filter(|part| {
                    part.team_id != 0
                        && part.control_data_type == 2
                        && part.object_number == object_number
                })
                .count(),
            expected,
            "mc2_01 mission GroundVehicle parts for object type {object_number}"
        );
    }
    let mut counts = std::collections::BTreeMap::<u32, usize>::new();
    for &handle in fixture.world.objects.movers.iter() {
        let Some(object) = fixture.world.objects.get(handle) else {
            continue;
        };
        if matches!(object.kind, Kind::Vehicle(_)) && object.team_id() != 0 {
            *counts.entry(object.c.type_handle as u32).or_default() += 1;
        }
    }
    assert_eq!(counts.get(&241), Some(&8), "mc2_01 Infantry parts");
    assert_eq!(counts.get(&201), Some(&7), "mc2_01 LRMC parts");

    let packet_bytes = fixture
        .vfs
        .read(mc2_sim_data::objtype::OBJECT_PAK_PATH)
        .unwrap()
        .expect("shipped object2.pak");
    let pak = mc2_formats::pak::PakFile::from_bytes(packet_bytes).unwrap();
    let packet_items = |object_type: usize| {
        let bytes = pak.read_packet(object_type).unwrap();
        let mut fit = mc2_formats::ini::FitIni::from_bytes(bytes).unwrap();
        fit.seek_block("InventoryInfo").unwrap();
        let other = fit.read_u8("NumOther").unwrap();
        let weapons = fit.read_u8("NumWeapons").unwrap();
        let ids = (0..usize::from(other + weapons))
            .map(|index| {
                fit.seek_block(&format!("Item:{index}")).unwrap();
                fit.read_u8("MasterID").unwrap()
            })
            .collect::<Vec<_>>();
        (ids, usize::from(other), usize::from(weapons))
    };
    let (infantry, infantry_other, infantry_weapons) = packet_items(241);
    let (lrmc, lrmc_other, lrmc_weapons) = packet_items(201);
    assert_eq!(
        infantry[infantry_other..infantry_other + infantry_weapons].to_vec(),
        [114]
    );
    assert_eq!(
        lrmc[lrmc_other..lrmc_other + lrmc_weapons].to_vec(),
        [120, 120, 120, 120, 120]
    );

    // C++ component loader inputs from compbas.csv (cmponent.cpp:123-153, 228-244), and the
    // mission's [WeaponRanges] table (mission.cpp:1729-1746). These values are the oracle, not
    // values sampled from a Rust firing result.
    let components = fixture.world.data.components.as_ref().unwrap();
    let mg = components.get(114).unwrap().weapon().unwrap();
    assert_eq!(
        (mg.damage, mg.recycle_time, mg.range),
        (1.0, 2.5, Some(mc2_sim_data::component::WeaponRange::Short))
    );
    assert_eq!(mg.ammo_amount, 1);
    let lrm5 = components.get(120).unwrap().weapon().unwrap();
    assert_eq!(
        (lrm5.damage, lrm5.recycle_time, lrm5.range),
        (3.0, 9.5, Some(mc2_sim_data::component::WeaponRange::Long))
    );
    assert_eq!((lrm5.ammo_amount, lrm5.ammo_master_id), (1, 220));
    assert_eq!(
        fixture
            .world
            .data
            .gamesys
            .tables
            .as_ref()
            .unwrap()
            .general
            .weapon_ranges,
        [[0.0, 60.0], [30.0, 120.0], [60.0, 180.0]]
    );

    // C++'s no-mover/point-target path has a 100% hit chance once the weapon's range gate passes
    // (Mover::calcAttackChance, mover.cpp:6281-6300). At 100 meters the LRM5 is in its long band;
    // the MG's short band rejects the same point. This pins both the range input and chance input.
    let lrmc_handle = fixture
        .mover_handles
        .values()
        .copied()
        .find(|&h| {
            fixture
                .world
                .objects
                .get(h)
                .is_some_and(|o| o.c.type_handle == 201)
        })
        .unwrap();
    let lrmc_object = fixture.world.objects.get(lrmc_handle).unwrap();
    let target_point = mc2_sim::math::Vec3 {
        x: lrmc_object.c.position.x + 100.0 * mc2_sim::terrain::WORLD_UNITS_PER_METER,
        y: lrmc_object.c.position.y,
        z: lrmc_object.c.position.z,
    };
    let lrmc_loadout = mc2_sim::combat::loadout_of(lrmc_object).unwrap();
    let lrm_index = (0..lrmc_loadout.num_weapons())
        .find(|index| lrmc_loadout.inventory[lrmc_loadout.num_other() + index].master_id == 120)
        .unwrap() as i32
        + lrmc_loadout.num_other() as i32;
    let lrm_chance = mc2_sim::combat::calc_attack_chance(
        &fixture.world,
        lrmc_handle,
        None,
        -1,
        lrm_index,
        Some(target_point),
    );
    assert_eq!((lrm_chance.range, lrm_chance.chance), (Some(1), 100.0));

    let infantry_handle = fixture
        .mover_handles
        .values()
        .copied()
        .find(|&h| {
            fixture
                .world
                .objects
                .get(h)
                .is_some_and(|o| o.c.type_handle == 241)
        })
        .unwrap();
    let infantry_object = fixture.world.objects.get(infantry_handle).unwrap();
    let infantry_loadout = mc2_sim::combat::loadout_of(infantry_object).unwrap();
    let target_point = mc2_sim::math::Vec3 {
        x: infantry_object.c.position.x + 100.0 * mc2_sim::terrain::WORLD_UNITS_PER_METER,
        y: infantry_object.c.position.y,
        z: infantry_object.c.position.z,
    };
    let mg_index = (0..infantry_loadout.num_weapons())
        .find(|index| {
            infantry_loadout.inventory[infantry_loadout.num_other() + index].master_id == 114
        })
        .unwrap() as i32
        + infantry_loadout.num_other() as i32;
    let mg_chance = mc2_sim::combat::calc_attack_chance(
        &fixture.world,
        infantry_handle,
        None,
        -1,
        mg_index,
        Some(target_point),
    );
    assert_eq!((mg_chance.range, mg_chance.chance), (Some(1), -1.0));

    // Ammunition is one unit per weapon shot, deducted only on a hit in C++ (gvehicl.cpp:4633-4639).
    let ammo = lrmc_loadout
        .inventory
        .iter()
        .filter(|item| item.master_id == 220)
        .map(|item| item.amount)
        .sum::<i16>();
    assert_eq!(ammo, 120, "the shipped LRMC loadout carries 120 LRM rounds");
}

#[test]
fn ground_vehicle_attack_fires_from_a_real_vehicle_hot_spot() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode))
        .expect("mc2_02 loads");
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    let mut sim = Sim::new(outcome);

    // One normal production frame loads the mission's real vehicle shapes and their node transforms.
    let visibility = Rc::new(AllVisible);
    sim.step(FrameInput::fixed(30, 1, visibility.clone(), Vec::new()))
        .unwrap();
    let vehicle = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            let Some(o) = sim.world.objects.get(h) else {
                return false;
            };
            if !matches!(o.kind, Kind::Vehicle(_))
                || o.c.status != ObjectStatus::Normal
                || !mc2_sim::combat::loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
            {
                return false;
            }
            let Some(appearance) = sim.world.appearance.vehicle(h) else {
                return false;
            };
            if appearance.type_index == usize::MAX {
                return false;
            }
            let ty = &sim.world.appearance.vehicle_types[appearance.type_index];
            let shape = ty.shape(mc2_sim::appearance::vehicle::GvShapeKind::Lod(0));
            [4usize, 5]
                .into_iter()
                .all(|i| shape.node_name_id(&ty.node_data[i].node_id) != -1)
        })
        .expect("mc2_02 has an armed vehicle with real Weapon_03 and Weapon_04 nodes");
    // The vehicle stays on its shipped (enemy) team: enemy ground-vehicle fire is enabled on the production path.
    let team = sim.world.objects.get(vehicle).unwrap().team_id();
    let target = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            sim.world.objects.get(h).is_some_and(|o| {
                o.team_id() != team
                    && o.c.status == ObjectStatus::Normal
                    && matches!(o.kind, Kind::Mech(_) | Kind::Vehicle(_))
            })
        })
        .expect("mc2_01 has a live opposing mover");
    let vehicle_watch = sim.world.objects.watch_id_peek(vehicle);
    let vehicle_part = sim.world.objects.get(vehicle).unwrap().c.part_id;
    let target_part = sim.world.objects.get(target).unwrap().c.part_id;
    // Read the shipped object2.pak packet separately from the production object-table loader and runtime Loadout.
    let vehicle_type = sim.world.objects.get(vehicle).unwrap().c.type_handle as u32;
    let object_bytes = vfs
        .read(mc2_sim_data::objtype::OBJECT_PAK_PATH)
        .unwrap()
        .expect("shipped object2.pak");
    let object_pak = mc2_formats::pak::PakFile::from_bytes(object_bytes).unwrap();
    let packet = object_pak.read_packet(vehicle_type as usize).unwrap();
    let mut packet_fit = mc2_formats::ini::FitIni::from_bytes(packet).unwrap();
    packet_fit.seek_block("InventoryInfo").unwrap();
    let num_other = packet_fit.read_u8("NumOther").unwrap();
    let num_weapons = packet_fit.read_u8("NumWeapons").unwrap();
    let mut expected_weapon_ids = BTreeSet::new();
    for item_index in usize::from(num_other)..usize::from(num_other + num_weapons) {
        packet_fit
            .seek_block(&format!("Item:{item_index}"))
            .unwrap();
        expected_weapon_ids.insert(i32::from(packet_fit.read_u8("MasterID").unwrap()));
    }
    let first_weapon = usize::from(num_other);

    assert!(sim
        .world
        .appearance
        .vehicle(vehicle)
        .is_some_and(|a| a.type_index != usize::MAX));
    let appearance_type = sim.world.appearance.vehicle(vehicle).unwrap().type_index;
    let weapon_nodes: Vec<(usize, String)> = [4usize, 5]
        .into_iter()
        .map(|index| {
            let name = sim.world.appearance.vehicle_types[appearance_type].node_data[index]
                .node_id
                .clone();
            let shape = sim.world.appearance.vehicle_types[appearance_type]
                .shape(mc2_sim::appearance::vehicle::GvShapeKind::Lod(0));
            assert_ne!(
                shape.node_name_id(&name),
                -1,
                "shipped vehicle shape has {name}"
            );
            (index, name)
        })
        .collect();
    // Make node indices 4 and 5 the least-used valid Weapon_03/04 nodes so this test pins the upper half of the C++ node range.
    {
        let appearance = sim.world.appearance.vehicle_mut(vehicle).unwrap();
        appearance.0.node_used[0] = 100;
        appearance.0.node_used[1] = 100;
        appearance.0.node_used[2] = 0;
        appearance.0.node_used[3] = 0;
    }

    // Declared scenario placement: freeze a live opposing mover 500 units in front of the vehicle. Its laser is in range, direct
    // line of sight and inside the vehicle's `FireArc[1]`.
    let mut vehicle_pos = sim.world.objects.get(vehicle).unwrap().c.position;
    vehicle_pos.z = sim.world.terrain.elevation(&vehicle_pos);
    let mut target_pos = vehicle_pos;
    target_pos.y -= 500.0;
    target_pos.z = vehicle_pos.z;
    mc2_sim::object::set_position(&mut sim.world, vehicle, vehicle_pos, true);
    mc2_sim::object::set_position(&mut sim.world, target, target_pos, true);
    sim.world
        .objects
        .get_mut(target)
        .unwrap()
        .c
        .set_awake(false);
    {
        let object = sim.world.objects.get_mut(vehicle).unwrap();
        object.c.rotation = 0.0;
        if let Kind::Vehicle(state) = &mut object.kind {
            state.turret_rotation = 0.0;
        }
    }
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
    mc2_sim::object::hooks::set_turret_parameters(&mut sim.world, vehicle, 0.0, 0.0);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, target);

    let fire_arc = mc2_sim::ai::decide::fire_arc(
        &sim.world,
        vehicle,
        sim.world.data.gamesys.tables.as_ref().unwrap(),
    );
    let pilot_slot = match &sim.world.objects.get(vehicle).unwrap().kind {
        Kind::Vehicle(v) => v.m.pilot_slot,
        _ => unreachable!(),
    };

    // A target behind the vehicle is outside FireArc[1]. Keep its body and turret headings fixed while a full attack-status
    // decision interval runs, so the test observes the rear target remaining outside the arc rather than just order submission.
    let behind = mc2_sim::math::Vec3 {
        x: vehicle_pos.x,
        y: vehicle_pos.y + 500.0,
        z: vehicle_pos.z,
    };
    mc2_sim::object::set_position(&mut sim.world, target, behind, true);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, target);
    let behind_distance = mc2_sim::ai::mover_ops::distance_from(&sim.world, vehicle, &behind);
    let stats = &mc2_sim::combat::loadout_of(sim.world.objects.get(vehicle).unwrap())
        .unwrap()
        .stats;
    let behind_angle = mc2_sim::ai::decide::rel_facing_to(&sim.world, vehicle, &behind, 0);
    assert!(behind_distance > stats.min_range && behind_distance < stats.max_range);
    assert!(behind_angle.abs() > fire_arc);
    // Other mission movers cannot account for damage in this vehicle-only outcome check.
    for h in sim.world.objects.movers.clone() {
        if h != vehicle {
            sim.world.objects.get_mut(h).unwrap().c.set_awake(false);
        }
    }
    sim.world
        .objects
        .get_mut(target)
        .unwrap()
        .c
        .set_awake(false);
    let mut saw_rear_arc_rejection = false;
    for frame in 2..=121u64 {
        {
            let object = sim.world.objects.get_mut(vehicle).unwrap();
            object.c.rotation = 0.0;
            if let Kind::Vehicle(state) = &mut object.kind {
                state.turret_rotation = 0.0;
            }
        }
        mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
        mc2_sim::object::hooks::set_turret_parameters(&mut sim.world, vehicle, 0.0, 0.0);
        let commands = if frame == 2 {
            vec![PlayerCommand::Scripted(ScriptedOrder::Attack {
                part: vehicle_part,
                target: target_part,
            })]
        } else {
            Vec::new()
        };
        let rear = sim
            .step(FrameInput::fixed(30, frame, visibility.clone(), commands))
            .unwrap();
        if sim.world.pilots.slots[usize::from(pilot_slot)]
            .weapons_status
            .contains(&mc2_sim::ai::pilot::WEAPON_STATUS_NOT_LOCKED)
        {
            saw_rear_arc_rejection = true;
        }
        assert!(
            !rear.events.iter().any(|event| matches!(event,
                SimEvent::WeaponEffect { owner, impact: WeaponImpact::Launch, .. } if *owner == vehicle_watch)),
            "vehicle launched during the 120-frame rear-arc decision budget at frame {frame}"
        );
    }
    assert!(
        saw_rear_arc_rejection,
        "rear-arc weapon status was never evaluated as not locked"
    );

    mc2_sim::ai::warrior::clear_cur_tac_order(&mut sim.world, pilot_slot, false);
    mc2_sim::ai::warrior::set_last_target(&mut sim.world, pilot_slot, None, false, false);
    if let Kind::Vehicle(v) = &mut sim.world.objects.get_mut(vehicle).unwrap().kind {
        v.m.suppression_fire = 0;
    }
    sim.world.objects.get_mut(target).unwrap().c.set_awake(true);
    // Restore the controlled front-facing setup after the point-fire sequence, which runs normal vehicle movement each frame.
    mc2_sim::object::set_position(&mut sim.world, vehicle, vehicle_pos, true);
    {
        let object = sim.world.objects.get_mut(vehicle).unwrap();
        object.c.rotation = 0.0;
        if let Kind::Vehicle(state) = &mut object.kind {
            state.turret_rotation = 0.0;
        }
    }
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
    mc2_sim::object::hooks::set_turret_parameters(&mut sim.world, vehicle, 0.0, 0.0);
    mc2_sim::object::set_position(&mut sim.world, target, target_pos, true);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, target);
    // This reposition happens between frames; bypass the previous frame's mover-pair LOS cache for the geometry assertion.
    let use_mover_los_table = std::mem::replace(&mut sim.world.objects.use_mover_los_table, false);
    let front_has_los = mc2_sim::object::line_of_sight(&mut sim.world, vehicle, target, 0.0, true);
    sim.world.objects.use_mover_los_table = use_mover_los_table;
    assert!(front_has_los);
    let target_watch = sim.world.objects.watch_id_peek(target);
    // Missions default to unlimited ammunition (`useUnlimitedAmmo`), which never spends a round; make this scenario count them.
    sim.world.mission.unlimited_ammo = false;
    {
        let components = std::rc::Rc::clone(
            sim.world
                .data
                .components
                .as_ref()
                .expect("loaded components"),
        );
        mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap())
            .unwrap()
            .calc_ammo_totals(&components, false)
            .expect("ammo totals");
    }
    let ammo_before_attack = mc2_sim::combat::loadout_of(sim.world.objects.get(vehicle).unwrap())
        .unwrap()
        .inventory
        .iter()
        .map(|item| i32::from(item.amount))
        .sum::<i32>();
    // Keep the outcome evidence attributable to this vehicle. The production mission can have several
    // autonomous movers attack the same target, so disable every other mover's weapon inventory for
    // this scenario (including the target's weapons).
    let mut disabled_for_isolation = Vec::new();
    for other in sim.world.objects.movers.clone() {
        if other == vehicle {
            continue;
        }
        if let Some(loadout) = mc2_sim::combat::loadout_of_mut(
            sim.world
                .objects
                .get_mut(other)
                .expect("mover handle remains live"),
        ) {
            for weapon in loadout.num_other()..loadout.num_other() + loadout.num_weapons() {
                disabled_for_isolation.push((other, weapon, loadout.inventory[weapon].disabled));
                loadout.inventory[weapon].disabled = true;
            }
        }
    }
    // Drain any projectiles launched before isolation so no earlier mission attack can damage this target during the evidence window.
    for bolt_handle in sim.world.objects.weapons.clone() {
        if let Some(Kind::WeaponBolt(bolt)) = sim
            .world
            .objects
            .get_mut(bolt_handle)
            .map(|object| &mut object.kind)
        {
            if bolt.target == target_watch && bolt.owner != vehicle_watch {
                bolt.target = WatchId::NONE;
                bolt.hit_target = true;
            }
        }
    }
    let mut launch_count = 0;
    let mut hit_impact_count = 0;
    let mut launch_error = 0.0f32;
    let mut fired_hotspot_indices = BTreeSet::new();
    let mut launched_effect_ids = BTreeSet::new();
    let mut target_weapon_ids = BTreeSet::new();
    let mut vehicle_caused_damage = false;
    let attack_start_frame = 122u64;

    // Reissue the player's attack order after placing the target inside FireArc[1]. The vehicle pilot, weapon readiness,
    // shared firing path and bolt lifecycle all run through the normal Sim::step decision path.
    for frame in attack_start_frame..=attack_start_frame + 899 {
        let health_before_frame = health(&sim, target);
        let commands = if frame == attack_start_frame {
            vec![PlayerCommand::Scripted(ScriptedOrder::Attack {
                part: vehicle_part,
                target: target_part,
            })]
        } else {
            Vec::new()
        };
        let out = sim
            .step(FrameInput::fixed(30, frame, visibility.clone(), commands))
            .unwrap();
        let health_after_frame = health(&sim, target);
        for event in &out.events {
            if let SimEvent::WeaponEffect {
                owner,
                from,
                effect_id,
                impact,
                ..
            } = event
            {
                if *owner != vehicle_watch {
                    continue;
                }
                if *impact == WeaponImpact::Launch {
                    launch_count += 1;
                    launched_effect_ids.insert(*effect_id);
                    // Resolve the actual named shape nodes 4 and 5 after asserting they exist; this cannot pass via the base-position
                    // fallback for a missing node.
                    let (nearest_index, nearest) = weapon_nodes
                        .iter()
                        .map(|(index, name)| {
                            let p = sim.world.appearance.node_name_position(vehicle, name);
                            (
                                *index,
                                ((from.x - p.x).powi(2)
                                    + (from.y - p.y).powi(2)
                                    + (from.z - p.z).powi(2))
                                .sqrt(),
                            )
                        })
                        .min_by(|a, b| a.1.total_cmp(&b.1))
                        .unwrap();
                    fired_hotspot_indices.insert(nearest_index);
                    launch_error = launch_error.max(nearest);
                } else if *impact == WeaponImpact::Hit
                    && target_bolts(&sim, vehicle_watch, target_watch)
                        .iter()
                        .any(|(effect, _, hit)| effect == effect_id && *hit)
                {
                    hit_impact_count += 1;
                    vehicle_caused_damage |= health_after_frame < health_before_frame;
                }
            }
        }
        for (effect_id, weapon_id, _) in target_bolts(&sim, vehicle_watch, target_watch) {
            if launched_effect_ids.contains(&effect_id) {
                target_weapon_ids.insert(weapon_id);
            }
        }
        if launch_count >= 2
            && hit_impact_count > 0
            && vehicle_caused_damage
            && fired_hotspot_indices.contains(&4)
            && fired_hotspot_indices.contains(&5)
        {
            break;
        }
    }

    assert!(
        launch_count > 0,
        "vehicle emitted no WeaponEffect launch event"
    );
    assert!(
        hit_impact_count > 0,
        "vehicle emitted no WeaponEffect Hit tied to a bolt targeting the selected target"
    );
    assert!(
        vehicle_caused_damage,
        "a vehicle-owned target Hit did not damage its target"
    );
    assert!(
        launch_error <= 1.0,
        "launch missed the vehicle's weapon hot spots by {launch_error} world units"
    );
    assert_eq!(fired_hotspot_indices, BTreeSet::from([4usize, 5usize]));
    assert_eq!(
        target_weapon_ids, expected_weapon_ids,
        "weapon IDs on vehicle bolts launched at the selected target must equal the independent shipped vehicle type packet inventory"
    );
    let components = std::rc::Rc::clone(
        sim.world
            .data
            .components
            .as_ref()
            .expect("loaded components"),
    );
    let has_ammo_weapon = expected_weapon_ids.iter().any(|id| {
        components
            .get(*id)
            .and_then(|component| component.weapon())
            .is_some_and(|weapon| weapon.ammo_master_id != 0)
    });
    if has_ammo_weapon {
        let ammo_after_attack =
            mc2_sim::combat::loadout_of(sim.world.objects.get(vehicle).unwrap())
                .unwrap()
                .inventory
                .iter()
                .map(|item| i32::from(item.amount))
                .sum::<i32>();
        assert!(
            ammo_after_attack < ammo_before_attack,
            "a shot with an ammunition weapon did not spend its C++ one-unit ammo amount"
        );
    }
    for (other, weapon, was_disabled) in disabled_for_isolation {
        mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(other).unwrap())
            .unwrap()
            .inventory[weapon]
            .disabled = was_disabled;
    }

    // Vehicle-specific early returns: StartingUp is rejected with C++ code 1; aerospace spotters with code 4; neither creates a bolt.
    let existing_bolts = bolt_ids(&sim, vehicle_watch);
    sim.world.objects.get_mut(vehicle).unwrap().c.status = ObjectStatus::StartingUp;
    mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap())
        .unwrap()
        .inventory[first_weapon]
        .ready_time = 0.0;
    sim.world
        .appearance
        .vehicle_mut(vehicle)
        .unwrap()
        .0
        .node_recycle
        .fill(0.0);
    let starting = fire_weapon(
        &mut sim.world,
        vehicle,
        first_weapon as i32,
        Some(target),
        -1,
        None,
        0,
    );
    assert_eq!(
        starting.code, 1,
        "GroundVehicle::fireWeapon rejects non-normal status"
    );
    sim.world.objects.get_mut(vehicle).unwrap().c.status = ObjectStatus::Normal;
    mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap())
        .unwrap()
        .inventory[first_weapon]
        .ready_time = 0.0;
    sim.world
        .appearance
        .vehicle_mut(vehicle)
        .unwrap()
        .0
        .node_recycle
        .fill(0.0);
    let vehicle_packet = sim
        .world
        .data
        .vehicle_objects
        .get_mut(&vehicle_type)
        .unwrap();
    std::rc::Rc::make_mut(vehicle_packet).aerospace_spotter = true;
    let spotter = fire_weapon(
        &mut sim.world,
        vehicle,
        first_weapon as i32,
        Some(target),
        -1,
        None,
        0,
    );
    assert_eq!(spotter.code, 4, "aerospace spotters cannot fire");
    std::rc::Rc::make_mut(
        sim.world
            .data
            .vehicle_objects
            .get_mut(&vehicle_type)
            .unwrap(),
    )
    .aerospace_spotter = false;
    assert_eq!(
        bolt_ids(&sim, vehicle_watch),
        existing_bolts,
        "blocked vehicle shots create no bolts"
    );
    assert_eq!(
        fire_weapon(&mut sim.world, vehicle, -1, Some(target), -1, None, 0).code,
        2,
        "GroundVehicle::fireWeapon returns 2 for an invalid weapon index"
    );

    // Vehicle weapon readiness ignores component health when the item is enabled and recycled (Mover::isWeaponReady).
    {
        let loadout =
            mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap()).unwrap();
        loadout.inventory[first_weapon].health = 0;
        loadout.inventory[first_weapon].disabled = false;
        loadout.inventory[first_weapon].ready_time = 0.0;
    }
    sim.world
        .appearance
        .vehicle_mut(vehicle)
        .unwrap()
        .0
        .node_recycle
        .fill(0.0);
    let healthless = fire_weapon(
        &mut sim.world,
        vehicle,
        first_weapon as i32,
        Some(target),
        -1,
        None,
        0,
    );
    assert_eq!(
        healthless.code, 0,
        "enabled, recycled vehicle weapons remain ready at zero component health"
    );

    // C++ applies a hit directly when createWeaponBolt returns null. Remove every effect-object mapping to force that fallback,
    // then fire through the shared vehicle path until the deterministic scenario rolls a hit.
    sim.world.objects.effect_objects.clear();
    let before_fallback = health(&sim, target);
    let mut fallback_hit = None;
    for _ in 0..500 {
        sim.world.clock.scenario_time += 1.0;
        mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap())
            .unwrap()
            .inventory[first_weapon]
            .ready_time = 0.0;
        sim.world
            .appearance
            .vehicle_mut(vehicle)
            .unwrap()
            .0
            .node_recycle
            .fill(0.0);
        let result = fire_weapon(
            &mut sim.world,
            vehicle,
            first_weapon as i32,
            Some(target),
            -1,
            None,
            0,
        );
        if health(&sim, target) < before_fallback {
            fallback_hit = Some(result);
            break;
        }
    }
    let fallback_hit = fallback_hit.expect("missing bolt effect still applies a vehicle hit");
    assert_eq!(fallback_hit.code, 0, "a fallback hit is a successful shot");
    assert!(fallback_hit.dmg_done > 0.0, "fallback hit reports damage");
    assert!(
        health(&sim, target) < before_fallback,
        "fallback hit damages the target"
    );

    // Mech.FireWeapon has no synchronous no-bolt fallback (mech.cpp:8195): a hit roll without a bolt must not damage.
    let mech = sim
        .world
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            sim.world.objects.get(h).is_some_and(|o| {
                matches!(o.kind, Kind::Mech(_))
                    && mc2_sim::combat::loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
            })
        })
        .expect("mission contains a weapon-equipped mech");
    let mech_index = mc2_sim::combat::loadout_of(sim.world.objects.get(mech).unwrap())
        .unwrap()
        .num_other();
    let mut mech_pos = vehicle_pos;
    mech_pos.x += 50.0;
    mc2_sim::object::set_position(&mut sim.world, mech, mech_pos, true);
    mc2_sim::object::set_position(&mut sim.world, vehicle, vehicle_pos, true);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, mech);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
    sim.world.mission.unlimited_ammo = true;
    let before_mech_no_bolt = health(&sim, vehicle);
    let mut mech_hit_without_bolt = None;
    for _ in 0..500 {
        sim.world.clock.scenario_time += 1.0;
        mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(mech).unwrap())
            .unwrap()
            .inventory[mech_index]
            .ready_time = 0.0;
        if let Some((appearance, _)) = sim.world.appearance.mech_mut(mech) {
            appearance.node_recycle.fill(0.0);
        }
        let result = fire_weapon(
            &mut sim.world,
            mech,
            mech_index as i32,
            Some(vehicle),
            -1,
            None,
            0,
        );
        if result.dmg_done > 0.0 {
            mech_hit_without_bolt = Some(result);
            break;
        }
    }
    let mech_hit_without_bolt =
        mech_hit_without_bolt.expect("deterministic mission RNG rolls a mech hit");
    assert_eq!(mech_hit_without_bolt.code, 0);
    assert!(mech_hit_without_bolt.dmg_done > 0.0);
    assert_eq!(health(&sim, vehicle), before_mech_no_bolt);
}

/// A grouped mech shot runs the implemented group mate alarm without reporting it unavailable.
#[test]
fn grouped_mech_shot_handles_mate_fired_alarm() {
    use common::fire_dump::{find, mc2_01, recycle};
    use mc2_sim::ai::frame_unavailable;
    use mc2_sim::combat::fire::{fire_weapon as full_fire, MATE_FIRED_HOOK};
    let Some(mut fx) = mc2_01() else { return };
    fx.w.mission.unlimited_ammo = true;
    fx.w.clock.scenario_time = 1000.0;
    fx.w.clock.frame_length = 1.0 / 30.0;
    let (att, tgt) = (find(&fx, 709), find(&fx, 706));
    // The compact fire fixture does not rebuild the global mover list.
    if !fx.w.objects.movers.contains(&att) {
        fx.w.objects.movers.push(att);
    }
    for (h, x, y, rot) in [(att, 0.0, 0.0, 90.0), (tgt, 100.0, 0.0, 270.0)] {
        let o = fx.w.objects.get_mut(h).unwrap();
        o.c.position.x = x;
        o.c.position.y = y;
        o.c.rotation = rot;
    }
    for node in ["1", "4", "5"] {
        recycle(&mut fx, &["recycle", "709", node, "0"]);
    }
    for group in [-1i8, 3] {
        fx.w.combat_unavailable_frame.clear();
        mc2_sim::combat::loadout_of_mut(fx.w.objects.get_mut(att).unwrap())
            .unwrap()
            .inventory[14]
            .ready_time = 0.0;
        if let Some(Kind::Mech(m)) = fx.w.objects.get_mut(att).map(|o| &mut o.kind) {
            m.m.group_id = group;
        }
        let r = full_fire(&mut fx.w, att, 14, Some(tgt), -1, None, 0);
        assert_eq!(r.code, 0, "the shot must proceed (group {group})");
        assert!(
            !frame_unavailable(&fx.w).iter().any(
                |u| matches!(u, mc2_sim::FrameUnavailable::Combat { hook } if *hook == MATE_FIRED_HOOK),
            ),
            "group {group}"
        );
        fx.w.clock.scenario_time += 1000.0;
        for node in ["1", "4", "5"] {
            recycle(&mut fx, &["recycle", "709", node, "0"]);
        }
    }
}

/// `GroundVehicle::fireWeapon` order (gvehicl.cpp:4502-4664): the target pilot is told (4586-4590) and the hit roll drawn (4599-4600)
/// before the weapon node check (4636-4651). A node still recycling returns 3 with the roll spent, no node used on either side
/// (`setWeaponNodeUsed` 4653, the target's `getWeaponNode` 4655-4660, which on a mech bumps that node) and no weapon recycle (4664).
/// A green node fires: one node used on the vehicle, one on the mech target, and the weapon recycles.
#[test]
fn vehicle_fire_order_is_pilot_roll_node_target_recycle() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode))
        .expect("mc2_02 loads");
    let mut sim = Sim::new(outcome);
    sim.step(FrameInput::fixed(30, 1, Rc::new(AllVisible), Vec::new()))
        .unwrap();
    let w = &sim.world;
    let vehicle = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            w.objects.get(h).is_some_and(|o| {
                matches!(o.kind, Kind::Vehicle(_))
                    && o.c.status == ObjectStatus::Normal
                    && mc2_sim::combat::loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
            }) && w
                .appearance
                .vehicle(h)
                .is_some_and(|a| a.type_index != usize::MAX)
        })
        .expect("mc2_02 has an armed vehicle");
    let team = w.objects.get(vehicle).unwrap().team_id();
    let target = w
        .objects
        .movers
        .iter()
        .copied()
        .find(|&h| {
            w.objects.get(h).is_some_and(|o| {
                o.team_id() != team
                    && o.c.status == ObjectStatus::Normal
                    && matches!(o.kind, Kind::Mech(_))
            })
        })
        .expect("mc2_02 has an opposing mech");
    let weapon = mc2_sim::combat::loadout_of(w.objects.get(vehicle).unwrap())
        .unwrap()
        .num_other();
    let vehicle_watch = sim.world.objects.watch_id(vehicle);
    let mut pos = sim.world.objects.get(vehicle).unwrap().c.position;
    pos.z = sim.world.terrain.elevation(&pos);
    let mut target_pos = pos;
    target_pos.y -= 300.0;
    mc2_sim::object::set_position(&mut sim.world, vehicle, pos, true);
    mc2_sim::object::set_position(&mut sim.world, target, target_pos, true);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, target);
    let pilot_slot = match &sim.world.objects.get(target).unwrap().kind {
        Kind::Mech(m) => m.m.pilot_slot,
        _ => unreachable!(),
    };

    let draws = |sim: &Sim| sim.world.rng.0.borrow().sim.draws();
    let vehicle_used = |sim: &Sim| -> i64 {
        let v = sim.world.appearance.vehicle(vehicle).unwrap();
        v.node_used.iter().map(|n| i64::from(*n)).sum()
    };
    let mech_used = |sim: &mut Sim| -> i64 {
        let (m, _) = sim.world.appearance.mech_mut(target).unwrap();
        m.node_used.iter().map(|n| i64::from(*n)).sum()
    };
    let ready_time = |sim: &Sim| {
        mc2_sim::combat::loadout_of(sim.world.objects.get(vehicle).unwrap())
            .unwrap()
            .inventory[weapon]
            .ready_time
    };
    let make_ready = |sim: &mut Sim, recycle: f32| {
        sim.world.clock.scenario_time += 1.0;
        mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(vehicle).unwrap())
            .unwrap()
            .inventory[weapon]
            .ready_time = 0.0;
        sim.world
            .appearance
            .vehicle_mut(vehicle)
            .unwrap()
            .0
            .node_recycle
            .fill(recycle);
    };

    // A recycling node: rejected with 3 after the roll and the pilot call, nothing else touched.
    make_ready(&mut sim, 0.5);
    if let Some(p) = sim.world.pilots.get_mut(pilot_slot) {
        p.attackers.clear();
    }
    let (d0, v0, m0, r0) = (
        draws(&sim),
        vehicle_used(&sim),
        mech_used(&mut sim),
        ready_time(&sim),
    );
    let rejected = fire_weapon(
        &mut sim.world,
        vehicle,
        weapon as i32,
        Some(target),
        -1,
        None,
        0,
    );
    assert_eq!(rejected.code, 3, "a recycling weapon node is not ready");
    assert_eq!(
        draws(&sim),
        d0 + 1,
        "the hit roll is drawn before the node check"
    );
    assert!(
        sim.world
            .pilots
            .get(pilot_slot)
            .unwrap()
            .attackers
            .iter()
            .any(|(wid, _)| *wid == vehicle_watch),
        "the target pilot is told before the node check"
    );
    assert_eq!(
        vehicle_used(&sim),
        v0,
        "no source node used on a rejected shot"
    );
    assert_eq!(
        mech_used(&mut sim),
        m0,
        "the target hot spot is picked only after the node check"
    );
    assert_eq!(
        ready_time(&sim),
        r0,
        "a rejected shot does not recycle the weapon"
    );

    // A green node: the shot goes out.
    make_ready(&mut sim, 0.0);
    let (d0, v0, m0) = (draws(&sim), vehicle_used(&sim), mech_used(&mut sim));
    let fired = fire_weapon(
        &mut sim.world,
        vehicle,
        weapon as i32,
        Some(target),
        -1,
        None,
        0,
    );
    assert_eq!(fired.code, 0);
    assert!(draws(&sim) > d0, "the hit roll is drawn");
    assert_eq!(
        vehicle_used(&sim),
        v0 + 1,
        "setWeaponNodeUsed(sourceHotSpot)"
    );
    assert_eq!(
        mech_used(&mut sim),
        m0 + 1,
        "the mech target's getWeaponNode(ANY) uses one of its nodes"
    );
    assert!(
        ready_time(&sim) > sim.world.clock.scenario_time,
        "startWeaponRecycle"
    );
}

/// `BattleMech::fireWeapon` returns 3 for a recycling weapon node (mech.cpp:8034-8036) before its hit roll (:8056), so a mech
/// refused for its node draws nothing. The vehicle side (node test after the roll, gvehicl.cpp:4600 / :4643-4651) is pinned by
/// `vehicle_node_check_follows_the_hit_roll` (tests/vehicle_fire_node_order.rs).
#[test]
fn mech_shot_refused_by_a_recycling_node_draws_no_hit_roll() {
    let Some(root) = common::data_root() else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("mount archives");
    let (mode, seed) = common::rng_by_name("pcg");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_02", seed, mode))
        .expect("mc2_02 loads");
    assert!(outcome.complete, "{:?}", outcome.missing_hooks);
    let mut sim = Sim::new(outcome);
    sim.step(FrameInput::fixed(30, 1, Rc::new(AllVisible), Vec::new()))
        .unwrap();
    let armed = |sim: &Sim, h: ObjectHandle, mech: bool| {
        sim.world.objects.get(h).is_some_and(|o| {
            matches!(o.kind, Kind::Mech(_)) == mech
                && matches!(o.kind, Kind::Mech(_) | Kind::Vehicle(_))
                && o.c.status == ObjectStatus::Normal
                && mc2_sim::combat::loadout_of(o).is_some_and(|l| l.num_weapons() > 0)
        })
    };
    let movers = sim.world.objects.movers.clone();
    let vehicle = *movers
        .iter()
        .find(|&&h| armed(&sim, h, false))
        .expect("mc2_02 has an armed vehicle");
    let team = sim.world.objects.get(vehicle).unwrap().team_id();
    let mech = *movers
        .iter()
        .find(|&&h| armed(&sim, h, true) && sim.world.objects.get(h).unwrap().team_id() != team)
        .expect("mc2_02 has an armed opposing mech");
    for &h in &movers {
        sim.world.objects.get_mut(h).unwrap().c.set_awake(false);
    }
    // the two face each other 100 units apart, inside every weapon's range
    let mut vehicle_pos = sim.world.objects.get(vehicle).unwrap().c.position;
    vehicle_pos.z = sim.world.terrain.elevation(&vehicle_pos);
    let mut mech_pos = vehicle_pos;
    mech_pos.y -= 100.0;
    mc2_sim::object::set_position(&mut sim.world, vehicle, vehicle_pos, true);
    mc2_sim::object::set_position(&mut sim.world, mech, mech_pos, true);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, vehicle);
    mc2_sim::object::hooks::set_object_parameters(&mut sim.world, mech);
    sim.world.mission.unlimited_ammo = true;
    let mech_weapon = mc2_sim::combat::loadout_of(sim.world.objects.get(mech).unwrap())
        .unwrap()
        .num_other();
    let draws = |sim: &Sim| sim.world.rng.0.borrow().sim.draws();

    // Mech, its weapon node recycling: code 3 and no draw (mech.cpp:8034-8036 precede the roll at :8056).
    sim.world.clock.scenario_time += 1.0;
    mc2_sim::combat::loadout_of_mut(sim.world.objects.get_mut(mech).unwrap())
        .unwrap()
        .inventory[mech_weapon]
        .ready_time = 0.0;
    sim.world
        .appearance
        .mech_mut(mech)
        .unwrap()
        .0
        .node_recycle
        .fill(1.0);
    let before = draws(&sim);
    let mech_refused = fire_weapon(
        &mut sim.world,
        mech,
        mech_weapon as i32,
        Some(vehicle),
        -1,
        None,
        0,
    );
    assert_eq!(mech_refused.code, 3);
    assert_eq!(
        draws(&sim),
        before,
        "BattleMech::fireWeapon refuses before its roll"
    );
}
