// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `WeaponEffects` (mclib/weaponfx.cpp:36-60): the weapon effect table of `data/objects/effects.csv`.
//!
//! Row 1 is the column header; effect `i` is row `i + 2`. Columns 2, 3, 4 and 6 are the effect, muzzle flash, hit and miss effect
//! names (`readString(.., 49)`), column 5 is the object type number the effect creates (`readLong`). `numEffects` is
//! `File::getNumLines() - 1`, and `getNumLines` counts the `\n` bytes of the file (mclib/file.cpp:1748). The CSV reader cannot reach
//! the last line (`countRows`), so its cells keep the unset heap value: 0 in practice (the oracle prints `EF <last> 0`).

use mc2_formats::csv::CsvFile;

/// One `EffectData` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeaponEffectRow {
    /// `effectName`, `muzzleFlashName`, `hitEffectName`, `missEffectName` (columns 2, 3, 4, 6).
    pub names: [String; 4],
    /// `effectObjNum` (column 5). A cell the CSV leaves empty (or cannot reach) keeps the unset value of the C++ `Malloc`: 0.
    pub obj_num: i32,
}

/// `WeaponEffects::init`: the rows of an effects CSV image. A missing cell reads as the empty string (the C++ `readString` leaves its
/// buffer unset); a value longer than 49 bytes is an error (the C++ loader would STOP), returned as such rather than loaded as data.
pub fn parse_weapon_effects(csv: &str) -> Result<Vec<WeaponEffectRow>, String> {
    let Ok(file) = CsvFile::from_bytes(csv.as_bytes().to_vec()) else {
        return Ok(Vec::new());
    };
    let num_effects = (csv.bytes().filter(|&b| b == b'\n').count() as u32).saturating_sub(1);
    let mut rows = Vec::with_capacity(num_effects as usize);
    for i in 0..num_effects {
        let name = |col: u32| -> Result<String, String> {
            file.read_string(i + 2, col, 49)
                .map(Option::unwrap_or_default)
                .map_err(|e| format!("effects row {} column {col}: {e:?}", i + 2))
        };
        rows.push(WeaponEffectRow {
            names: [name(2)?, name(3)?, name(4)?, name(6)?],
            obj_num: file.read_i32(i + 2, 5).unwrap_or(0),
        });
    }
    Ok(rows)
}

/// `GetEffectObjNum(i)` for every row.
pub fn effect_obj_nums(rows: &[WeaponEffectRow]) -> Vec<i32> {
    rows.iter().map(|r| r.obj_num).collect()
}
