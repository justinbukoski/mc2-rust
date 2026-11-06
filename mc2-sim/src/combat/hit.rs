// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Hit location: `BattleMech::calcHitLocation` (mech.cpp:6638) and `GroundVehicle::calcHitLocation` (gvehicl.cpp:4038), plus
//! `BattleMech::transferHitLocation` (mech.cpp:6692).
//!
//! A mech's hit section follows the attack source (`MechHitSectionTable`), its arc the angle the attacker stands at relative to the
//! mech's facing; one `RandomNumber(100)` picks the armor location from the `HitLocationTable` row of that (section, arc). Without an
//! attacker the entry angle is itself random (`RandomNumber(360) - 180`, drawn first). A ground vehicle takes two `RandomNumber(100)`
//! draws for the height and hits the turret from 70 up, otherwise the front/left/right/rear side facing the attacker.

use crate::combat::AttackSource;
use crate::ids::ObjectHandle;
use crate::object::Kind;
use crate::World;

/// `MECH_HIT_ARC_*` (mech.h:67).
pub mod arc {
    /// Front.
    pub const FRONT: usize = 0;
    /// Rear.
    pub const REAR: usize = 1;
    /// Left.
    pub const LEFT: usize = 2;
    /// Right.
    pub const RIGHT: usize = 3;
}

/// `MECH_HIT_SECTION_*` (mech.h:73).
pub mod section {
    /// Top.
    pub const TOP: usize = 0;
    /// Middle.
    pub const MIDDLE: usize = 1;
    /// Bottom.
    pub const BOTTOM: usize = 2;
}

/// `NUM_MECH_ARMOR_LOCATIONS`.
const NUM_MECH_ARMOR_LOCATIONS: usize = 11;
/// `NUM_MECH_HIT_ARCS`.
const NUM_MECH_HIT_ARCS: usize = 4;

/// `MechHitSectionTable[NUM_ATTACKSOURCES]` (mech.cpp:209).
pub const MECH_HIT_SECTION_TABLE: [usize; 5] = [
    section::MIDDLE, // weapon fire
    section::MIDDLE, // collision
    section::TOP,    // death from above
    section::BOTTOM, // mine
    section::MIDDLE, // artillery
];

/// `GROUNDVEHICLE_LOCATION_*` (gvehicl.h:80).
pub mod vehicle_location {
    /// Front.
    pub const FRONT: i32 = 0;
    /// Left.
    pub const LEFT: i32 = 1;
    /// Right.
    pub const RIGHT: i32 = 2;
    /// Rear.
    pub const REAR: i32 = 3;
    /// Turret.
    pub const TURRET: i32 = 4;
}

/// `MECH_STATE_FALLEN_FORWARD` / `BACKWARD` (mech3d.h:94).
const MECH_STATE_FALLEN_FORWARD: i32 = 7;
const MECH_STATE_FALLEN_BACKWARD: i32 = 8;

/// The arc of an entry angle (degrees): the boundaries are inclusive for the front only.
fn arc_of(entry_angle: f32) -> usize {
    if (-45.0..=45.0).contains(&entry_angle) {
        arc::FRONT
    } else if entry_angle > -135.0 && entry_angle < -45.0 {
        arc::LEFT
    } else if entry_angle > 45.0 && entry_angle < 135.0 {
        arc::RIGHT
    } else {
        arc::REAR
    }
}

/// `calcHitLocation(attacker, weaponIndex, attackSource, attackType)` on a mech or ground vehicle: the armor location (mech) or
/// vehicle location hit. `attacker` is `ObjectHandle::NONE` for "no attacker" (mines, artillery). RNG: sim stream.
pub fn calc_hit_location(
    w: &mut World,
    target: ObjectHandle,
    attacker: ObjectHandle,
    _weapon_index: i32,
    source: AttackSource,
    _attack_type: i32,
) -> i32 {
    let attacker_position = w.objects.get(attacker).map(|o| o.c.position);
    let Some(t) = w.objects.get(target) else {
        return -1;
    };
    match &t.kind {
        Kind::Mech(_) => {
            let section = MECH_HIT_SECTION_TABLE[source as usize];
            // Find the arc...
            let entry_angle = match attacker_position {
                Some(p) => crate::ai::decide::rel_facing_to(w, target, &p, -1),
                None => {
                    let r = w.rng.0.clone();
                    let n = r.borrow_mut().sim.random_number(360);
                    (n - 180) as f32
                }
            };
            let mut arc = arc_of(entry_angle);
            let mut section = section;
            if source == AttackSource::Mine {
                // if this came from a mine
                let old = w.appearance.old_gesture_goal(target);
                if old == MECH_STATE_FALLEN_FORWARD {
                    arc = arc::FRONT;
                    section = section::MIDDLE;
                }
                if old == MECH_STATE_FALLEN_BACKWARD {
                    arc = arc::REAR;
                    section = section::MIDDLE;
                }
            }
            // Roll, and see where it hits...
            let mut location_roll = {
                let r = w.rng.0.clone();
                let n = r.borrow_mut().sim.random_number(100);
                n
            };
            let start_entry = section * (NUM_MECH_ARMOR_LOCATIONS * NUM_MECH_HIT_ARCS)
                + (arc * NUM_MECH_ARMOR_LOCATIONS);
            let table = w
                .data
                .gamesys
                .tables
                .as_ref()
                .map(|t| t.mech.hit_location_table.clone())
                .unwrap_or_default();
            let mut armor_location = 0;
            while armor_location < NUM_MECH_ARMOR_LOCATIONS {
                let cell = i32::from(
                    table
                        .get(start_entry + armor_location)
                        .copied()
                        .unwrap_or(0),
                );
                if location_roll < cell {
                    break;
                }
                location_roll -= cell;
                armor_location += 1;
            }
            armor_location as i32
        }
        Kind::Vehicle(_) => {
            // `attackSource == MECH_HIT_SECTION_BOTTOM`: the value 2, i.e. a death-from-above hit, not a mine (the C++ mixes the enums)
            if source as i32 == section::BOTTOM as i32 {
                return vehicle_location::FRONT;
            }
            // First, find the section: the height is the mean of two rolls
            let z_axis_pos = {
                let r = w.rng.0.clone();
                let mut r = r.borrow_mut();
                let a = r.sim.random_number(100);
                let b = r.sim.random_number(100);
                (a + b) / 2
            };
            if z_axis_pos >= 70 {
                return vehicle_location::TURRET;
            }
            let entry_angle = match attacker_position {
                Some(p) => {
                    let mut f = crate::ai::decide::rel_facing_to(w, target, &p, -1);
                    if f < -180.0 {
                        f += 360.0;
                    } else if f > 180.0 {
                        f -= 360.0;
                    }
                    f
                }
                None => 0.0,
            };
            if (-45.0..=45.0).contains(&entry_angle) {
                vehicle_location::FRONT
            } else if entry_angle > -135.0 && entry_angle < -45.0 {
                vehicle_location::LEFT
            } else if entry_angle > 45.0 && entry_angle < 135.0 {
                vehicle_location::RIGHT
            } else {
                vehicle_location::REAR
            }
        }
        _ => -1,
    }
}

/// `BattleMech::transferHitLocation(hitLocation)` (mech.cpp:6692): `MechTransferHitTable`, -1 for the head (nothing to transfer to).
pub fn transfer_hit_location(w: &World, body_location: usize) -> i32 {
    w.data
        .gamesys
        .tables
        .as_ref()
        .map_or(-1, |t| i32::from(t.mech.transfer_hit_table[body_location]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_arc_is_inclusive_and_the_sides_are_not() {
        assert_eq!(arc_of(0.0), arc::FRONT);
        assert_eq!(arc_of(45.0), arc::FRONT);
        assert_eq!(arc_of(-45.0), arc::FRONT);
        assert_eq!(arc_of(45.01), arc::RIGHT);
        assert_eq!(arc_of(-45.01), arc::LEFT);
        assert_eq!(arc_of(134.99), arc::RIGHT);
        assert_eq!(arc_of(135.0), arc::REAR);
        assert_eq!(arc_of(-135.0), arc::REAR);
        assert_eq!(arc_of(180.0), arc::REAR);
        assert_eq!(arc_of(-179.0), arc::REAR);
    }

    #[test]
    fn the_arc_and_section_numbers_are_the_c_plus_plus_ones() {
        assert_eq!([arc::FRONT, arc::REAR, arc::LEFT, arc::RIGHT], [0, 1, 2, 3]);
        assert_eq!(MECH_HIT_SECTION_TABLE, [1, 1, 0, 2, 1]);
        assert_eq!(AttackSource::from_code(3), Some(AttackSource::Mine));
        assert_eq!(AttackSource::from_code(5), None);
        assert_eq!(AttackSource::from_code(-1), None);
    }
}
