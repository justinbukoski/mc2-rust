// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `gamesys.fit` from the shipped data. Skips cleanly when `MC2_DATA` is unset.

use mc2_formats::vfs::Vfs;
use mc2_sim_data::gamesys::{GameSystem, MECH_HIT_LOCATION_TABLE_LEN, VISUAL_RANGE_TABLE_LEN};

fn vfs() -> Option<Vfs> {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping shipped gamesys test");
        return None;
    };
    Some(Vfs::from_system_cfg(root).expect("system.cfg mounts"))
}

#[test]
fn shipped_gamesys_loads_with_the_values_the_file_holds() {
    let Some(mut vfs) = vfs() else { return };
    let g = GameSystem::load(&mut vfs).expect("gamesys.fit");

    let gen = &g.general;
    assert_eq!(
        gen.weapon_ranges,
        [[0.0, 60.0], [30.0, 120.0], [60.0, 180.0]]
    );
    assert_eq!(gen.optimal_range_points, [20.0, 45.0, 100.0, 100.0, 160.0]);
    assert_eq!(gen.max_visual_range, 225.0);
    #[allow(clippy::approx_constant)]
    let radius = (225.0f64 * 1.4142) as f32;
    assert_eq!(gen.max_visual_radius, radius);
    assert_eq!(gen.fire_visual_range, 45.0);
    assert_eq!(gen.max_tree_los_cell_block, 6);
    assert_eq!(gen.weapon_range, [60.0, 140.0, 200.0, 0.0]);
    assert_eq!(gen.default_attack_range, 70.0);
    assert_eq!(gen.base_sensor_range, 1.0);
    assert_eq!(gen.visual_range_table.len(), VISUAL_RANGE_TABLE_LEN);
    assert_eq!(gen.base_head_shot_elevation, 0.7);
    assert_eq!(gen.forest_move_cost, 50);
    assert_eq!(gen.max_extract_unit_distance, 1280.0);
    assert_eq!(gen.tonnage_divisor, 5);
    assert_eq!(gen.resources_per_ton_divided, 1000);

    assert_eq!(g.difficulty.player_skills, [125, 100, 75, 50]);
    assert_eq!(g.difficulty.enemy_weapons, [50, 100, 125, 150]);

    assert_eq!(g.pathfinding.simple_move_path_range, 30);
    assert_eq!(g.pathfinding.move_time_out, 60.0);
    assert_eq!(g.pathfinding.group_move_trail_len, [1, 3]);
    assert_eq!(g.pathfinding.margin_of_error, [3.0, 4.0]);

    assert_eq!(g.mover_general.block_capture_range, 75.0);
    assert_eq!(g.mover_general.refit_costs[2][1], 0.2);
    assert_eq!(
        &g.mover_general.indirect_fire_weapons[..5],
        &[120, 123, 130, 160, -1]
    );

    assert_eq!(g.mover_fire.fire_arc, [12.5, 12.5, 22.5]);
    assert_eq!(g.mover_fire.aimed_fire_abort, 60);
    assert_eq!(g.mover_fire.head_shot_modifier, 0.08);
    assert_eq!(g.mover_damage.hit_level, [10, 20]);
    assert_eq!(g.mover_damage.cluster_size_srm, 2);
    assert_eq!(g.mover_damage.cluster_size_lrm, 5);
    assert_eq!(g.mover_damage.anti_missile_system_stats, [[1, 2], [2, 2]]);

    assert_eq!(
        g.warrior.warrior_rank_scale,
        [45.0, 56.0, 66.0, 999.0, 999.0]
    );
    assert_eq!(g.warrior.attitude_effect[0], [50, 75, 5, -2, 0, 3]);
    assert_eq!(g.warrior.attitude_effect[5], [0, 0, 32, 2, 5, -128]);
    assert_eq!(g.warrior.skill_increase_cap, 3);

    assert_eq!(g.sensors.range_modifier[3], [1.0, -25.0]);
    assert_eq!(g.sensors.size_modifier[2], [999.0, 15.0]);
    assert_eq!(g.sensors.master_ids[8], -1);

    assert_eq!(g.skills.skill_try, [0.0; 4]);
    assert_eq!(g.skills.skill_success, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(g.skills.weapon_hit, 3.0);
    assert_eq!(g.skills.kill_skill, [45.0, 60.0, 70.0, 80.0, 20.0, 6.0]);
    assert_eq!(g.skills.sensor_skill, 20.0);

    // MaxMediumMech is never read; the medium and heavy slots both hold MaxHeavyMech.
    assert_eq!(g.mech.mech_class_weights, [0.0, 35.0, 75.0, 75.0, 100.0]);
    assert_eq!(g.mech.pilot_check_conditions, [10, 10]);
    assert_eq!(
        g.mech.attacker_move_modifier,
        [0, 0, 10, 20, 10, 5, 30, 0, 0, 0]
    );
    assert_eq!(g.mech.hit_location_table.len(), MECH_HIT_LOCATION_TABLE_LEN);
    assert_eq!(
        &g.mech.hit_location_table[..11],
        &[18, 16, 17, 17, 16, 16, 0, 0, 0, 0, 0]
    );
    let row_sums: Vec<i32> = g
        .mech
        .hit_location_table
        .chunks(11)
        .map(|r| r.iter().map(|&v| i32::from(v)).sum())
        .collect();
    assert_eq!(row_sums, [100; 12]);
    assert_eq!(
        g.mech.target_move_modifier_table,
        [[6, 0], [12, 1], [18, 2], [27, 3], [999, 4]]
    );
    assert_eq!(g.mech.critical_hit_table, [58, 83, 97, 100]);
    assert_eq!(g.mech.transfer_hit_table, [1, -1, 1, 1, 2, 3, 2, 3]);
    assert_eq!(g.mech.salvage_chance, 40);
    assert_eq!(g.mech.pivot_throttle, 75.0);

    assert_eq!(g.vehicle.attacker_move_modifier, [0, 10, 100]);
    assert_eq!(g.vehicle.critical_hit_table.iter().sum::<i32>(), 100);
    assert_eq!(g.vehicle.walk_speed, 9.0);
    assert_eq!(g.vehicle.hill_speed_factor, 1.5);

    assert_eq!(g.mine.mine_damage, 12.0);
    assert_eq!(g.mine.mine_explosion, 655);
    assert_eq!(g.mine.mine_lay_throttle, 100);
    assert_eq!(g.mine.strike_time_to_impact, 4.0);
    assert_eq!(g.mine.total_smoke_spheres, 2000);
    assert_eq!(g.mine.max_fires_burning, 3);
}
