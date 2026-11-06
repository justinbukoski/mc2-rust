// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `BaseSensorRange` and the `WeaponRanges` block of `data/missions/gamesys.fit`.
//!
//! Port of the reads in `code/mission.cpp:1726-1780`. `BaseSensorRange` is in
//! `[General]` (mission.cpp:1780). The range spans are in `[WeaponRanges]`
//! (mission.cpp:1729-1746). The C++ also computes `OptimalRangePointInRange`
//! from them, which is not ported.

use super::record::WeaponRange;
use super::GAMESYS_FILE;
use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;

/// Reads `BaseSensorRange` from the `[General]` block (mission.cpp:1780).
pub fn base_sensor_range(gamesys: &mut FitIni) -> Result<f32> {
    gamesys.seek_block("General")?;
    Ok(gamesys.read_f32("BaseSensorRange")?)
}

/// Converts an array read to a fixed size, or reports the mismatch.
fn fixed<const N: usize>(name: &str, values: Vec<f32>) -> Result<[f32; N]> {
    <[f32; N]>::try_from(values).map_err(|found| {
        SimDataError::schema(
            GAMESYS_FILE,
            format!("{name} has {} values, expected {N}", found.len()),
        )
    })
}

/// The `WeaponRanges` block: the meters each range band covers, and the points
/// the AI treats as optimal.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponRangeSpans {
    /// `[min, max]` for short, medium and long, in that order (`Short`, `Medium`, `Long`).
    spans: [[f32; 2]; 3],
    /// `OptimalRangePoints`, five values.
    pub optimal_points: [f32; 5],
}

impl WeaponRangeSpans {
    /// Reads the `[WeaponRanges]` block from `gamesys.fit`.
    pub fn from_gamesys(gamesys: &mut FitIni) -> Result<Self> {
        gamesys.seek_block("WeaponRanges")?;
        let short = fixed::<2>("Short", gamesys.read_array::<f32>("Short", 2)?)?;
        let medium = fixed::<2>("Medium", gamesys.read_array::<f32>("Medium", 2)?)?;
        let long = fixed::<2>("Long", gamesys.read_array::<f32>("Long", 2)?)?;
        let optimal_points = fixed::<5>(
            "OptimalRangePoints",
            gamesys.read_array::<f32>("OptimalRangePoints", 5)?,
        )?;
        Ok(Self {
            spans: [short, medium, long],
            optimal_points,
        })
    }

    /// `[min, max]` meters for a range band (`WeaponRanges[range]`).
    pub fn span(&self, range: WeaponRange) -> [f32; 2] {
        self.spans[range.index()]
    }
}
