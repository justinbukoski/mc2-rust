//! Test: artillery immunity predicate for airborne mechs.

use mc2_sim::combat::artillery_math::{airborne_mech_immune, MOVETYPE_AIR};
use mc2_sim::object::ObjectStatus;

#[test]
fn airborne_mech_immunity_predicate() {
    // Immune iff move_type == MOVETYPE_AIR && status != ObjectStatus::ShutDown

    // Airborne mechs (move_type == 1) should be immune unless ShutDown
    assert!(airborne_mech_immune(MOVETYPE_AIR, ObjectStatus::Normal));
    assert!(airborne_mech_immune(MOVETYPE_AIR, ObjectStatus::Disabled));
    assert!(airborne_mech_immune(
        MOVETYPE_AIR,
        ObjectStatus::ShuttingDown
    ));
    assert!(airborne_mech_immune(MOVETYPE_AIR, ObjectStatus::StartingUp));
    assert!(!airborne_mech_immune(MOVETYPE_AIR, ObjectStatus::ShutDown));

    // Non-airborne mechs (move_type == 0) should never be immune
    assert!(!airborne_mech_immune(0, ObjectStatus::Normal));
    assert!(!airborne_mech_immune(0, ObjectStatus::ShutDown));
}
