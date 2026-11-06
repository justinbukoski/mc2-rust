// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The weapon/effect equivalence table (`mclib/weaponfx.{h,cpp}`).
//!
//! `data/objects/effects.csv` maps an effect id (the row, from 0) to gosFX effect *names*:
//! the in-flight effect, the muzzle flash, the hit and the miss. The game looks the names up
//! in the library with `EffectLibrary::Find` each time it creates a bolt or an explosion.

use crate::error::{FxError, Result};
use mc2_formats::csv::CsvFile;

/// Where the table lives inside the data root.
pub const EFFECTS_CSV_PATH: &str = "data/objects/effects.csv";

/// `mc2_word_none`.
pub const NONE: &str = "NONE";

// Effect ids the game hard-codes (`weaponfx.h`).
pub const BUILDING_EXPLOSION_ID: usize = 45;
pub const VEHICLE_EXPLOSION_ID: usize = 40;
pub const SPLASH_VEHICLE_EXPLOSION_ID: usize = 46;
pub const DFA_BOOM_EFFECT: usize = 47;
pub const MECH_EXPLOSION_ID: usize = 39;
pub const MECH_CRITICAL_HIT_ID: usize = 35;
pub const MECH_SMOKE_ID: usize = 48;
pub const TURRET_EXPLOSION_ID: usize = 44;
pub const MECH_DISABLE_EXPLOSION_ID: usize = 49;
pub const LARGE_AIRSTRIKE_ID: usize = 50;
pub const SMALL_AIRSTRIKE_ID: usize = 51;
pub const SENSOR_AIRSTRIKE_ID: usize = 72;
pub const MINE_EXPLOSION_ID: usize = 53;
pub const EMPTY_EXPLOSION_ID: usize = 54;
pub const JET_CONTRAIL_ID: usize = 58;
pub const ARM_FLYING_ID: usize = 61;
pub const SHOULDER_POP_ID: usize = 62;
pub const CRITICAL_SMOKE_ID: usize = 65;
pub const MECH_WATER_WAKE: usize = 34;
pub const VEHICLE_WATER_WAKE: usize = 66;
pub const VEHICLE_DUST_CLOUD: usize = 70;
pub const VTOL_DUST_CLOUD: usize = 56;
pub const KARNOV_DUST_CLOUD: usize = 73;
pub const KARNOV_RECOVERY_BEAM: usize = 69;
pub const MINELAYER_EFFECT_ID: usize = 68;
pub const REPAIR_VEHICLE_EFFECT_ID: usize = 57;
pub const HELICOPTER_DUST_CLOUD: usize = 67;
pub const TURRET_POWER_DOWN_EFFECT: usize = 75;
pub const WATER_MISS_FX: usize = 76;
pub const INFANTRY_BOOM_EFFECT: usize = 55;
pub const INFANTRY_BLOOD_EFFECT: usize = 82;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectData {
    pub effect_name: String,
    pub muzzle_flash_name: String,
    pub hit_effect_name: String,
    pub miss_effect_name: String,
    pub effect_obj_num: i32,
}

#[derive(Clone, Debug, Default)]
pub struct WeaponEffects {
    effects: Vec<EffectData>,
}

impl WeaponEffects {
    /// `WeaponEffects::init`. The count is the number of newline bytes minus one for the
    /// header (`File::getNumLines`). Entry `i` is CSV row `i + 2`: effect name in column 2,
    /// muzzle flash 3, hit 4, object number 5, miss 6. A cell the CSV reader cannot reach (the
    /// C++ leaves such a name as uninitialised memory) reads as `NONE`.
    pub fn from_csv_bytes(bytes: &[u8]) -> Result<WeaponEffects> {
        let lines = bytes.iter().filter(|&&b| b == b'\n').count();
        let n = lines.saturating_sub(1);
        let csv = CsvFile::from_bytes(bytes.to_vec())
            .map_err(|e| FxError::new(format!("effects.csv: {e}")))?;
        let name = |row: u32, col: u32| -> String {
            match csv.read_string(row, col, 49) {
                Ok(Some(s)) if !s.is_empty() => s,
                _ => NONE.to_string(),
            }
        };
        let mut effects = Vec::with_capacity(n);
        for i in 0..n as u32 {
            let row = i + 2;
            effects.push(EffectData {
                effect_name: name(row, 2),
                muzzle_flash_name: name(row, 3),
                hit_effect_name: name(row, 4),
                miss_effect_name: name(row, 6),
                effect_obj_num: csv.read_i32(row, 5).unwrap_or(0),
            });
        }
        Ok(WeaponEffects { effects })
    }

    pub fn len(&self) -> usize {
        self.effects.len()
    }
    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }
    pub fn get(&self, id: usize) -> Option<&EffectData> {
        self.effects.get(id)
    }
    /// `GetEffectName`: `"NONE"` when the id is out of range.
    pub fn effect_name(&self, id: i64) -> &str {
        self.entry(id).map_or(NONE, |e| e.effect_name.as_str())
    }
    pub fn muzzle_flash_name(&self, id: i64) -> &str {
        self.entry(id)
            .map_or(NONE, |e| e.muzzle_flash_name.as_str())
    }
    pub fn hit_effect_name(&self, id: i64) -> &str {
        self.entry(id).map_or(NONE, |e| e.hit_effect_name.as_str())
    }
    pub fn miss_effect_name(&self, id: i64) -> &str {
        self.entry(id).map_or(NONE, |e| e.miss_effect_name.as_str())
    }
    /// `GetEffectObjNum`: -1 when the id is out of range.
    pub fn effect_obj_num(&self, id: i64) -> i32 {
        self.entry(id).map_or(-1, |e| e.effect_obj_num)
    }
    fn entry(&self, id: i64) -> Option<&EffectData> {
        usize::try_from(id).ok().and_then(|i| self.effects.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV: &str = "3,Effect,Muzzle,Hit,Obj,Miss,Weapon\r\n\
        1,NONE,NONE,NONE,0,NONE,No weapon\r\n\
        2,ppc_trail,PPC_flare,ppc_hit,467,PPC_Miss,PPC\r\n\
        3,Mech_Explosion,NONE,NONE,NA,NONE,Boom\r\n";

    #[test]
    fn rows_map_to_ids() {
        let w = WeaponEffects::from_csv_bytes(CSV.as_bytes()).unwrap();
        // 4 newlines - 1 header = 3 entries
        assert_eq!(w.len(), 3);
        assert_eq!(w.effect_name(0), "NONE");
        assert_eq!(w.effect_name(1), "ppc_trail");
        assert_eq!(w.muzzle_flash_name(1), "PPC_flare");
        assert_eq!(w.hit_effect_name(1), "ppc_hit");
        assert_eq!(w.miss_effect_name(1), "PPC_Miss");
        assert_eq!(w.effect_obj_num(1), 467);
    }

    #[test]
    fn out_of_range_is_none() {
        let w = WeaponEffects::from_csv_bytes(CSV.as_bytes()).unwrap();
        assert_eq!(w.effect_name(-1), NONE);
        assert_eq!(w.effect_name(99), NONE);
        assert_eq!(w.effect_obj_num(99), -1);
    }
}
