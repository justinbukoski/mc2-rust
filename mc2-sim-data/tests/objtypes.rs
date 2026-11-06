// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Loads every packet of the shipped `data/objects/object2.pak`.
//!
//! Needs `MC2_DATA` set to the game data root (the directory with `system.cfg`).
//! Without it the test prints a note and returns.

use std::path::PathBuf;

use mc2_formats::pak::PakFile;
use mc2_sim_data::objtype::{
    load_object_table, ObjectTypeClass, ObjectTypeData, MAX_OBJECT_TYPES, OBJECT_PAK_PATH,
};

/// Packet 55 is the PUMA mech base file (`[Header]`, `[General]`, ...). It has no
/// `[MechProfile]` block, so the C++ `BattleMechType::init` returns an error and the
/// game would `Fatal` if it loaded the packet. It is the only expected failure.
const EXPECTED_ERRORS: &[u32] = &[55];

fn data_root() -> Option<PathBuf> {
    let root = std::env::var_os("MC2_DATA").map(PathBuf::from)?;
    let pak = root.join(OBJECT_PAK_PATH);
    if pak.exists() {
        Some(pak)
    } else {
        eprintln!("MC2_DATA set but {} is missing; skipping", pak.display());
        None
    }
}

#[test]
fn every_object_packet_loads() {
    let Some(pak_path) = data_root() else {
        eprintln!("MC2_DATA unset; skipping object type integration test");
        return;
    };

    let table = load_object_table(&pak_path).expect("open object2.pak");
    let errors = &table.failures;

    for e in errors {
        eprintln!("packet {}: {}", e.index, e.error);
    }
    let error_indices: Vec<u32> = errors.iter().map(|e| e.index).collect();
    assert_eq!(error_indices, EXPECTED_ERRORS, "unexpected packet failures");

    // Every non-empty packet is either loaded or one of the expected failures. Index 0
    // is never loaded (objtype.cpp:324).
    let pak = PakFile::open(&pak_path).expect("reopen object2.pak");
    let mut non_empty = 0usize;
    for i in 1..pak.packet_count() {
        if !pak.read_packet(i).expect("read packet").is_empty() {
            non_empty += 1;
        }
    }
    assert_eq!(table.len() + errors.len(), non_empty);

    eprintln!(
        "loaded {} object types from {} packets",
        table.len(),
        pak.packet_count()
    );
    for (class, count) in table.class_counts() {
        match class {
            Some(code) => eprintln!("  class {code:>3}: {count}"),
            None => eprintln!("  misc terrain hack handles: {count}"),
        }
    }

    // The five misc-terrain handles come from the table size, not from packets.
    for index in MAX_OBJECT_TYPES - 5..MAX_OBJECT_TYPES {
        let obj = table.get(index).expect("misc handle present");
        assert!(obj.packet_class.is_none());
        assert_eq!(obj.object_type_class, ObjectTypeClass::TerrainObject);
    }

    // Spot checks against the packets (see the scratchpad extract): a building, a tree,
    // a turret and a weapon bolt are typed.
    let typed = |pred: fn(&ObjectTypeData) -> bool| table.types.values().any(|t| pred(&t.data));
    assert!(typed(|d| matches!(d, ObjectTypeData::Building(_))));
    assert!(typed(|d| matches!(d, ObjectTypeData::TerrainObject(_))));
    assert!(typed(|d| matches!(d, ObjectTypeData::Turret(_))));
    assert!(typed(|d| matches!(d, ObjectTypeData::WeaponBolt(_))));
    assert!(typed(|d| matches!(d, ObjectTypeData::Raw)));
}
