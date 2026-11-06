// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The GUI cursor query (`MissionInterfaceManager::update`, missiongui.cpp:742-754) as a sim input: `PlayerCommand::CursorLineOfSight`.
//!
//! The C++ asks `Team::home->teamLineOfSight(wPos, 0.0f)` for the terrain point under the mouse every frame; each home mover in visual
//! range of that point runs `getLOSPosition`, which caches its lowest weapon node in turns < 6 (mover.cpp:3471-3490). In mc2_07 that is
//! the only query that reaches mech 512 in turns 1-5 (rc-mc2-01b evidence). Expected values: the oracle's `--cursor-trace` and
//! `--los-trace` of mc2_07 seed 6 (`L 1..3 0 0 0 0`, `L 4.. 448 -3178.6665 318.518555 1`; `Z 4 512 1 27.3545532`, 513/514 still -2 at turn 6).
mod common;

use mc2_sim::command::PlayerCommand;
use mc2_sim::ids::PartId;
use mc2_sim::math::Vec3;
use mc2_sim::mission::init::LoadSpec;
use mc2_sim::object::Kind;
use mc2_sim::visibility::AllVisible;
use mc2_sim::{FrameInput, Sim};
use std::rc::Rc;

fn cached_node(sim: &Sim, part: i32) -> Option<(i32, f32)> {
    let h = sim.world.objects.find_by_part_id(PartId(part)).unwrap();
    match &sim.world.objects.get(h).unwrap().kind {
        Kind::Mech(m) => m.m.cached_los_weapon_node,
        _ => panic!("part {part} is not a mech"),
    }
}

/// The cached `(lowestWeaponNodeID, lowestWeaponNodeZ)` of 512, 513, 514.
type Nodes = [Option<(i32, f32)>; 3];

/// Six frames of mc2_07 (msvc seed 6), with or without the oracle's cursor points; the cached node of 512, 513, 514 after each frame.
fn first_six_frames(cursor: bool) -> Option<Vec<Nodes>> {
    let root = common::data_root()?;
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let (mode, _) = common::rng_by_name("msvc");
    let outcome = common::load_blocker(&mut vfs, &LoadSpec::quickstart("mc2_07", 6, mode)).unwrap();
    let mut sim = Sim::new(outcome);
    let vis = Rc::new(AllVisible);
    let mut out = Vec::new();
    for frame in 1..=6u64 {
        let at = if frame < 4 {
            Vec3 {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            }
        } else {
            Vec3 {
                x: 448.0,
                y: -3178.6665,
                z: 318.518_55,
            }
        };
        let commands = if cursor {
            vec![PlayerCommand::CursorLineOfSight { at }]
        } else {
            vec![]
        };
        sim.step(FrameInput::fixed(30, frame, vis.clone(), commands))
            .unwrap();
        out.push([512, 513, 514].map(|p| cached_node(&sim, p)));
    }
    Some(out)
}

#[test]
fn the_cursor_query_caches_512s_eye_height_on_turn_4_and_no_other() {
    let Some(with) = first_six_frames(true) else {
        return;
    };
    for (i, nodes) in with.iter().enumerate() {
        let turn = i + 1;
        if turn < 4 {
            assert_eq!(
                nodes[0], None,
                "turn {turn}: the origin is out of 512's range"
            );
        } else {
            let (node, z) = nodes[0].unwrap_or_else(|| panic!("turn {turn}: 512 not cached"));
            assert_eq!(node, 1, "turn {turn}");
            // the oracle's `Z 4 512 1 27.3545532`; head (blanket priming) read 27.355 on turn 1, rc-mc2-01 alone 26.179 on turn 40
            assert!((z - 27.354_553).abs() < 1e-3, "turn {turn}: 512 at {z}");
        }
        assert_eq!(
            nodes[1], None,
            "turn {turn}: 513 is never asked before turn 46"
        );
        assert_eq!(
            nodes[2], None,
            "turn {turn}: 514 is never asked before turn 51"
        );
    }
    // without the cursor input (rc-mc2-01's tip) nothing asks 512 in these turns
    let without = first_six_frames(false).unwrap();
    assert!(without.iter().all(|n| n[0].is_none()), "{without:?}");
}
