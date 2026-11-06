//! Slice-2 state: the bolt/artillery state fields exist with C++-matching
//! defaults.

use mc2_sim::combat::{ArtilleryState, BoltState};

#[test]
fn fire_scaffold_links() {
    let bolt = BoltState::default();
    assert!(bolt.weapon_shot.is_none());
    assert!(!bolt.just_created);
    assert_eq!(bolt.goal_height, 0.0);
    assert!(!bolt.has_target_position);

    let artillery = ArtilleryState::default();
    assert!(!artillery.boom);
    assert!(!artillery.whoosh);
    assert!(!artillery.just_created);
    assert_eq!(artillery.time_to_launch, 0.0);
    assert_eq!(artillery.time_to_blind, 0.0);
    assert_eq!(artillery.sensor_range, 0.0);
}
