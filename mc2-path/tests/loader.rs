//! Loader regression tests: malformed-but-sized packets must return `LoadError`, the special-area
//! count must be checked against its packet before anything is allocated, and the count can be
//! 4 or 8 bytes wide (sizeof(long) on Win32 and LP64 builds).

mod support;

use mc2_path::{GlobalMap, LoadError, MoveData, Mover, NoGates, Planner};
use support::world::*;

fn global_packets() -> Vec<Vec<u8>> {
    encode_global(&parts_for(four_area_doors()))
}

#[test]
fn encoded_world_loads_and_routes_like_the_in_memory_one() {
    let pk = move_packets(
        &open_mission(),
        &parts_for(four_area_doors()),
        vec![0; 4],
        vec![0; 4],
    );
    let data = MoveData::load(&pk, 0).expect("valid packets load");
    assert_eq!(data.packets_used, pk.len());
    let mut planner = Planner::new(data);
    let mut reference = Planner::new(move_data(open_mission(), parts_for(four_area_doors())));
    let mover = Mover::mech(0, 60.0);
    let a = planner
        .global_path(&mover, (5, 5), (50, 50), &NoGates)
        .unwrap();
    let b = reference
        .global_path(&mover, (5, 5), (50, 50), &NoGates)
        .unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 3);
}

// ----------------------------------------------------- malformed global maps

fn expect_bad(pk: &[Vec<u8>], what_contains: &str) {
    let owned = pk.to_vec();
    match GlobalMap::from_packets(&owned, 0) {
        Err(LoadError::BadValue { what, .. }) => {
            assert!(what.contains(what_contains), "got '{what}'")
        }
        other => panic!("expected BadValue ({what_contains}), got {other:?}"),
    }
}

#[test]
fn door_info_index_out_of_range_is_an_error() {
    let mut pk = global_packets();
    pk[12][0..2].copy_from_slice(&99i16.to_le_bytes()); // first area door info of area 0
    expect_bad(&pk, "area door index");
}

#[test]
fn door_info_side_out_of_range_is_an_error() {
    let mut pk = global_packets();
    pk[12][2] = 2;
    expect_bad(&pk, "area door side");
}

#[test]
fn door_info_naming_a_door_that_does_not_border_the_area_is_an_error() {
    let mut pk = global_packets();
    // Door 0 (V0) borders areas 0 and 1; flipping its side makes area 0 claim area 1's side.
    pk[12][2] = 1;
    expect_bad(&pk, "does not border");
}

#[test]
fn area_map_id_out_of_range_is_an_error() {
    let mut pk = global_packets();
    pk[10][0..2].copy_from_slice(&200i16.to_le_bytes());
    expect_bad(&pk, "areaMap id");
}

#[test]
fn link_index_out_of_range_is_an_error() {
    let mut pk = global_packets();
    // Packet 17 is door 0, side 0: its first link names door 99.
    pk[17][0..2].copy_from_slice(&99i16.to_le_bytes());
    expect_bad(&pk, "door link index");
}

#[test]
fn link_side_out_of_range_is_an_error() {
    let mut pk = global_packets();
    pk[17][2] = 2;
    expect_bad(&pk, "door link side");
}

#[test]
fn negative_door_length_is_an_error() {
    let mut pk = global_packets();
    pk[16][4] = 0xFF; // door 0 length = -1
    expect_bad(&pk, "door length");
}

#[test]
fn garbage_in_placeholder_link_slots_is_ignored() {
    // The start/goal placeholders carry stale link data in shipped files; the search overwrites
    // it, so the loader must not reject it. Packets 25..=28 are the placeholders' link packets.
    let mut pk = global_packets();
    for p in &mut pk[25..=28] {
        for b in p.iter_mut() {
            *b = 0xFF;
        }
    }
    let (map, used) = GlobalMap::from_packets(&pk, 0).expect("placeholder garbage is tolerated");
    assert_eq!(used, pk.len());
    assert_eq!(map.num_doors(), 4);
}

// ----------------------------------------------------- special-area count

fn with_specials(count: Vec<u8>, footprints: Vec<u8>) -> Result<MoveData, LoadError> {
    let pk = move_packets(
        &open_mission(),
        &parts_for(four_area_doors()),
        count,
        footprints,
    );
    MoveData::load(&pk, 0)
}

#[test]
fn eight_byte_count_with_one_footprint_loads() {
    let fp = footprint_bytes(5, 6, &[(5, 6), (5, 7)]);
    let data = with_specials(1i64.to_le_bytes().to_vec(), fp).expect("LP64 count width accepted");
    assert_eq!(data.special_areas.len(), 1);
    assert_eq!(data.special_areas[0].cell_position_row, 5);
    assert_eq!(data.special_areas[0].num_cells, 2);
    assert_eq!(data.special_areas[0].cells[1], [5, 7]);
}

#[test]
fn four_byte_count_with_one_footprint_loads() {
    let fp = footprint_bytes(1, 2, &[(1, 2)]);
    let data = with_specials(1i32.to_le_bytes().to_vec(), fp).expect("Win32 count width accepted");
    assert_eq!(data.special_areas.len(), 1);
}

#[test]
fn zero_count_with_eight_byte_filler_loads() {
    let data = with_specials(0i64.to_le_bytes().to_vec(), 0i64.to_le_bytes().to_vec()).unwrap();
    assert!(data.special_areas.is_empty());
}

#[test]
fn huge_four_byte_count_is_rejected_before_allocating() {
    // i32::MAX footprints would need ~560 GB. The footprint packet is 4 bytes, so the count must fail
    // on the length check, not on an allocation.
    let err = with_specials(i32::MAX.to_le_bytes().to_vec(), vec![0; 4]).unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::BadPacketSize {
                what: "special area footprints",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn eight_byte_count_overflowing_the_footprint_size_is_rejected() {
    let err =
        with_specials(i64::MAX.to_le_bytes().to_vec(), footprint_bytes(0, 0, &[])).unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::BadValue {
                what: "special area count",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn zero_filler_must_be_the_same_width_as_the_count() {
    let err = with_specials(0i64.to_le_bytes().to_vec(), vec![0; 4]).unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::BadPacketSize {
                what: "zero special-area filler",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn footprint_packet_must_match_the_count_exactly() {
    let err = with_specials(1i32.to_le_bytes().to_vec(), vec![0; 263]).unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::BadPacketSize {
                what: "special area footprints",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn count_packet_must_be_four_or_eight_bytes() {
    let err = with_specials(vec![0; 3], vec![0; 3]).unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::BadPacketSize {
                what: "special area count",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn negative_count_is_an_error() {
    let err = with_specials((-1i32).to_le_bytes().to_vec(), vec![0; 4]).unwrap_err();
    assert!(
        matches!(
            err,
            LoadError::BadValue {
                what: "special area count",
                ..
            }
        ),
        "{err:?}"
    );
}
