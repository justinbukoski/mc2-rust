// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Master component and weapon list (`compbas.csv`).
//!
//! Port of the component loader in `mclib/cmponent.{h,cpp}`. The game reads
//! `data/objects/compbas.csv` from `mission.fst` with 255 data rows, after one
//! header line (`code/mission.cpp:1802-1806`, `code/saveload.cpp:793-797`). The
//! row id in column 0 is the index into `MasterComponent::masterList`.
//!
//! Also here:
//! - [`WeaponRangeSpans`]: the `WeaponRanges` block of `gamesys.fit`, which gives the
//!   meters for each range band, and `BaseSensorRange` used by sensor rows.
//!
//! - [`logistics`]: the catalogue the logistics screens read from the same file
//!   (`LogisticsComponent`), added for `mc2-campaign`.
//!
//! Not ported: the effect table (`effects.csv`, `mclib/weaponfx.cpp`) that
//! [`WeaponStats::special_effect`] indexes, the logistics UI, and the master
//! writer (`saveMasterList`), which writes a layout the loader cannot read back.
//! Provenance and quirks are in `rust/docs/porting-log.md`.

mod cparse;
pub mod logistics;
mod ranges;
mod record;

use crate::error::{Result, SimDataError};
use cparse::split_lines;
use mc2_formats::vfs::Vfs;
pub use ranges::{base_sensor_range, WeaponRangeSpans};
use record::parse_row;
pub use record::{
    ComponentForm, ComponentStats, LogisticsEntry, MasterComponent, WeaponAmmoType, WeaponRange,
    WeaponStats, NUM_BODY_LOCATIONS, WEAPON_FLAG_ARTILLERY, WEAPON_FLAG_INFERNO, WEAPON_FLAG_LBX,
    WEAPON_FLAG_STREAK,
};

#[cfg(test)]
mod tests;

/// Path of the master list inside the data root (`objects/compbas` + `.csv`).
pub const COMPBAS_FILE: &str = "data/objects/compbas.csv";

/// Path of the game-system FIT file that holds `BaseSensorRange` and `WeaponRanges`.
pub const GAMESYS_FILE: &str = "data/missions/gamesys.fit";

/// Data rows the game loads (`loadMasterList(compFileName, 255, ...)`, mission.cpp:1806).
pub const MASTER_LIST_COUNT: usize = 255;

/// Longest line the loader reads (`componentFile.readLine(dataLine, 511)`, cmponent.cpp:668).
const LINE_LIMIT: usize = 511;

/// Component ids the game looks up by value. The loader resets them to -1 and never
/// reads the file's `Master...ID` lines (cmponent.cpp:57-60 defaults, 644-647 reset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecialComponentIds {
    /// `MasterComponent::armActuatorID`. Used by mech.cpp:1801 and 2586.
    pub arm_actuator: i32,
    /// `MasterComponent::legActuatorID`. Used by mech.cpp:1808 and 2593.
    pub leg_actuator: i32,
    /// `MasterComponent::clanAntiMissileSystemID`.
    pub clan_anti_missile: i32,
    /// `MasterComponent::innerSphereAntiMissileSystemID`.
    pub inner_sphere_anti_missile: i32,
}

impl SpecialComponentIds {
    /// The values the C++ holds after `loadMasterList` returns: all -1.
    pub const AFTER_LOAD: Self = Self {
        arm_actuator: -1,
        leg_actuator: -1,
        clan_anti_missile: -1,
        inner_sphere_anti_missile: -1,
    };
}

/// The master component list (`MasterComponent::masterList`, `numComponents`).
#[derive(Debug, Clone, PartialEq)]
pub struct MasterComponentList {
    components: Vec<MasterComponent>,
    special: SpecialComponentIds,
}

impl MasterComponentList {
    /// Loads the list from the data root and the mounted archives, as the game does
    /// at mission start. Reads `BaseSensorRange` from [`GAMESYS_FILE`].
    pub fn load(vfs: &mut Vfs) -> Result<Self> {
        let compbas = vfs
            .read(COMPBAS_FILE)?
            .ok_or_else(|| SimDataError::MissingFile {
                name: COMPBAS_FILE.to_string(),
            })?;
        let mut gamesys = vfs
            .read_fit(GAMESYS_FILE)?
            .ok_or_else(|| SimDataError::MissingFile {
                name: GAMESYS_FILE.to_string(),
            })?;
        let base = base_sensor_range(&mut gamesys)?;
        Self::from_compbas_bytes(&compbas, MASTER_LIST_COUNT, base)
    }

    /// Parses `compbas.csv` bytes: one header line, then `count` data rows.
    ///
    /// Row `n` must carry id `n` in column 0, because the game indexes the list by
    /// id. A row that the C++ rejects (form `undefined`, `Removed`, `Optics`) stays in
    /// the list with `form == None`, as it does in `masterList`.
    pub fn from_compbas_bytes(data: &[u8], count: usize, base_sensor_range: f32) -> Result<Self> {
        let lines = split_lines(data);
        let available = lines.len().saturating_sub(1);
        if available < count {
            return Err(SimDataError::schema(
                COMPBAS_FILE,
                format!("{count} data rows expected after the header, found {available}"),
            ));
        }
        let mut components = Vec::with_capacity(count);
        for (index, raw) in lines.iter().skip(1).take(count).enumerate() {
            let row = index + 2;
            if raw.len() > LINE_LIMIT {
                return Err(SimDataError::schema(
                    COMPBAS_FILE,
                    format!("row {row} is longer than {LINE_LIMIT} bytes"),
                ));
            }
            let text = String::from_utf8_lossy(raw);
            let record = parse_row(&text, row, COMPBAS_FILE, base_sensor_range)?;
            if i64::from(record.master_id) != index as i64 {
                return Err(SimDataError::schema(
                    COMPBAS_FILE,
                    format!(
                        "row {row} has id {} where {index} is expected",
                        record.master_id
                    ),
                ));
            }
            components.push(record);
        }
        Ok(Self {
            components,
            special: SpecialComponentIds::AFTER_LOAD,
        })
    }

    /// Number of records, including rows the C++ rejects.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// `true` when the list has no records.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// The record for `master_id` (`masterList[id]`), or `None` when the id is out of range.
    pub fn get(&self, master_id: i32) -> Option<&MasterComponent> {
        usize::try_from(master_id)
            .ok()
            .and_then(|index| self.components.get(index))
    }

    /// Every record in id order.
    pub fn iter(&self) -> impl Iterator<Item = &MasterComponent> + '_ {
        self.components.iter()
    }

    /// Records the logistics purchase list shows (see [`MasterComponent::is_logistics_listed`]).
    pub fn logistics_listed(&self) -> impl Iterator<Item = &MasterComponent> + '_ {
        self.components
            .iter()
            .filter(|component| component.is_logistics_listed())
    }

    /// The special ids the game uses, in their post-load state.
    pub fn special(&self) -> SpecialComponentIds {
        self.special
    }
}
