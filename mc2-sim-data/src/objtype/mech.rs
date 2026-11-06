// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `BattleMechType` class fields from `[MechProfile]` (mech.cpp:666-731).
//!
//! The chassis CSV named by `ProfileName` (mech.cpp:690-720: `ProfileName.csv`,
//! max internal structure, dynamics) belongs to the mech lane and is not read here.
//! The `[ObjectType]` block is read by the caller after this one (mech.cpp:731).

use mc2_formats::ini::FitIni;

use crate::error::Result;

/// `MOVETYPE_GROUND`, the first value of `enum MoveType` (mover.h:744-748).
pub const MOVETYPE_GROUND: i32 = 0;

/// Class-level fields of a battle mech type.
#[derive(Debug, Clone, PartialEq)]
pub struct BattleMechType {
    /// `ProfileName` (mech.cpp:671). Required. The mech lane opens
    /// `data/objects/<ProfileName>.csv` with it.
    pub profile_name: String,
    /// `MoveType` (mech.cpp:675). Default [`MOVETYPE_GROUND`].
    pub move_type: i32,
    /// `LOSFactor` (mech.cpp:679). Default 1.0.
    pub los_factor: f32,
    /// `DestructDamage` (mech.cpp:683). Default 20.0.
    pub destruct_damage: f32,
    /// `ExplosionDamage` (mech.cpp:687). Default 10.0.
    pub explosion_damage: f32,
    /// `ExplosionRadius` (mech.cpp:691). Default 25.0.
    pub explosion_radius: f32,
}

impl BattleMechType {
    /// Reads `[MechProfile]`. Moves the file's current block to `MechProfile`.
    pub(crate) fn read(fit: &mut FitIni) -> Result<Self> {
        fit.seek_block("MechProfile")?;
        let profile_name = fit.read_string("ProfileName", 512)?;
        let move_type = fit.read_i32("MoveType").unwrap_or(MOVETYPE_GROUND);
        let los_factor = fit.read_f32("LOSFactor").unwrap_or(1.0);
        let destruct_damage = fit.read_f32("DestructDamage").unwrap_or(20.0);
        let explosion_damage = fit.read_f32("ExplosionDamage").unwrap_or(10.0);
        let explosion_radius = fit.read_f32("ExplosionRadius").unwrap_or(25.0);
        Ok(Self {
            profile_name,
            move_type,
            los_factor,
            destruct_damage,
            explosion_damage,
            explosion_radius,
        })
    }
}
