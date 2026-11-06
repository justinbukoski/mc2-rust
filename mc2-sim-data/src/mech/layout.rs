// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Body and armor location tables shared by the chassis CSV and the profile FIT.
//!
//! Ports `MechBodyLocationType` and `MechArmorLocationType` (`code/mech.h:147-173`),
//! `NumLocationCriticalSpaces` (`code/mech.cpp:198-208`), `stupidNewSageColumns`
//! (`code/mech.cpp:279-280`), the `NUM_CRITICAL_SPACES_*` counts
//! (`code/mech.h:82-90`) and `MAX_MOVER_INVENTORY_ITEMS` (`code/mover.h:588`).

/// Mech body locations with critical spaces (`NUM_MECH_BODY_LOCATIONS`).
pub const NUM_BODY_LOCATIONS: usize = 8;
/// Armor locations, including the three rear sections (`NUM_MECH_ARMOR_LOCATIONS`).
pub const NUM_ARMOR_LOCATIONS: usize = 11;
/// Inventory rows per variant in the chassis CSV and per mech in the profile.
pub const MAX_INVENTORY_ITEMS: usize = 72;

/// A mech body location (`MECH_BODY_LOCATION_*`, `code/mech.h:148-157`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyLocation {
    Head,
    CenterTorso,
    LeftTorso,
    RightTorso,
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
}

impl BodyLocation {
    /// All eight locations in enum order.
    pub const ALL: [BodyLocation; NUM_BODY_LOCATIONS] = [
        BodyLocation::Head,
        BodyLocation::CenterTorso,
        BodyLocation::LeftTorso,
        BodyLocation::RightTorso,
        BodyLocation::LeftArm,
        BodyLocation::RightArm,
        BodyLocation::LeftLeg,
        BodyLocation::RightLeg,
    ];

    /// Position in enum order, used to index the per-location arrays.
    pub fn index(self) -> usize {
        self as usize
    }

    /// CSV column (1-based) of this location in the internal structure, armor and
    /// slot rows. `stupidNewSageColumns` (`code/mech.cpp:279-280`).
    pub fn csv_column(self) -> u32 {
        [2, 7, 5, 6, 3, 4, 8, 9][self.index()]
    }

    /// Number of critical spaces (`NumLocationCriticalSpaces`, `code/mech.cpp:198-208`).
    pub fn critical_spaces(self) -> usize {
        [6, 12, 12, 12, 12, 12, 6, 6][self.index()]
    }

    /// Profile FIT block name (`BodyLocationBlockString`, `code/mech.cpp:2694-2703`).
    pub fn profile_block(self) -> &'static str {
        [
            "Head",
            "CenterTorso",
            "LeftTorso",
            "RightTorso",
            "LeftArm",
            "RightArm",
            "LeftLeg",
            "RightLeg",
        ][self.index()]
    }
}

/// A mech armor location (`MECH_ARMOR_LOCATION_*`, `code/mech.h:161-173`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArmorLocation {
    Head,
    CenterTorso,
    LeftTorso,
    RightTorso,
    LeftArm,
    RightArm,
    LeftLeg,
    RightLeg,
    RearCenterTorso,
    RearLeftTorso,
    RearRightTorso,
}

impl ArmorLocation {
    /// All eleven armor locations in enum order.
    pub const ALL: [ArmorLocation; NUM_ARMOR_LOCATIONS] = [
        ArmorLocation::Head,
        ArmorLocation::CenterTorso,
        ArmorLocation::LeftTorso,
        ArmorLocation::RightTorso,
        ArmorLocation::LeftArm,
        ArmorLocation::RightArm,
        ArmorLocation::LeftLeg,
        ArmorLocation::RightLeg,
        ArmorLocation::RearCenterTorso,
        ArmorLocation::RearLeftTorso,
        ArmorLocation::RearRightTorso,
    ];

    /// Position in enum order.
    pub fn index(self) -> usize {
        self as usize
    }

    /// CSV column (1-based) of this armor value in rows 18 (max) and 115 (current).
    /// The rear sections are columns 12, 10 and 11 (`code/mech.cpp:1435-1455`).
    pub fn csv_column(self) -> u32 {
        [2, 7, 5, 6, 3, 4, 8, 9, 12, 10, 11][self.index()]
    }

    /// Profile FIT key under `[MaxArmorPoints]` and `[CurArmorPoints]`
    /// (`code/mech.cpp:2823-2870` and `:2871-2943`).
    pub fn profile_key(self) -> &'static str {
        [
            "Head",
            "CenterTorso",
            "LeftTorso",
            "RightTorso",
            "LeftArm",
            "RightArm",
            "LeftLeg",
            "RightLeg",
            "RearCenterTorso",
            "RearLeftTorso",
            "RearRightTorso",
        ][self.index()]
    }
}
