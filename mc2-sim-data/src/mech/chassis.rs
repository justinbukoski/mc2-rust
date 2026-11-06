// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Chassis CSV (`data/objects/<name>.csv`): mech chassis, variants and loadouts.
//!
//! Port of `BattleMech::init(DWORD variantNum)` (`code/mech.cpp:1376-1748`), the
//! chassis reads in `LogisticsChassis::init` and `LogisticsVariant::init`
//! (`code/logisticsvariant.cpp:42-130`, `:323-370`), and the CSV half of
//! `MoverDynamics::init(CSVFilePtr)` (`code/mover.cpp:1581-1600`).
//!
//! Numbering: rows and columns are 1-based as in `CSVFile` (see
//! `mc2_formats::csv`). Column 2 holds single values, column 5 the chassis
//! statistics, column 8 the AllowComponents flags. A variant `v` (0-based) is a
//! block of 97 rows whose name row is `23 + 97 * v`.
//!
//! Heat values (`Heat Index`, the item `Heat` column, the `Heat` total) are read
//! but unused, as `USEHEAT` is undefined in this build.
//!
//! Errors: the C++ leaves a value uninitialised when a read fails and carries on
//! for most optional cells. This port keeps those as `Option` and reports a
//! schema error only where the C++ returns an error (`code/mech.cpp:716` for the
//! base internal structure, `:1399-1408` and `:1419-1428` for the required header
//! and stats). The armor reads (`:1435-1488`) and slot reads (`:1732`) are unchecked.

use super::layout::{
    ArmorLocation, BodyLocation, MAX_INVENTORY_ITEMS, NUM_ARMOR_LOCATIONS, NUM_BODY_LOCATIONS,
};
use crate::error::{Result, SimDataError};
use mc2_formats::csv::CsvFile;

/// Name row of variant 0 (`23 + 97 * variant` in `code/mech.cpp:1422`).
pub const VARIANT_BASE_ROW: u32 = 23;
/// Rows per variant block (`97 * variant` in `code/mech.cpp:1422`).
pub const VARIANT_STRIDE: u32 = 97;
/// First item row, relative to the name row (row 26, `code/mech.cpp:1516`).
const ITEM_OFFSET: u32 = 3;
/// Totals row, relative to the name row (row 99).
const TOTALS_OFFSET: u32 = 76;
/// First critical slot row, relative to the name row (row 102, `code/mech.cpp:1732`).
const SLOT_OFFSET: u32 = 79;
/// Current internal structure row, relative to the name row (row 114, `code/mech.cpp:1711`).
const CUR_INTERNAL_OFFSET: u32 = 91;
/// Current armor row, relative to the name row (row 115, `code/mech.cpp:1458`).
const CUR_ARMOR_OFFSET: u32 = 92;

/// Chassis block of single-value cells (column 2 unless noted).
const ROW_MECH_NAME: u32 = 3;
const ROW_HOUSE_ID: u32 = 4;
const ROW_DESC_INDEX: u32 = 5;
const ROW_ABBR_INDEX: u32 = 6;
const ROW_ENCYCLOPEDIA_ID: u32 = 7;
const ROW_HELP_ID: u32 = 8;
const ROW_ICON_FIRST: u32 = 9;
const ROW_MECH_PARTS: u32 = 12;
const ROW_LOGISTICS_X: u32 = 13;
const ROW_LOGISTICS_Y: u32 = 14;
const ROW_BASE_INTERNAL: u32 = 17;
const ROW_BASE_ARMOR: u32 = 18;
/// Column of single values.
const COL_VALUE: u32 = 2;
/// Column of chassis statistics.
const COL_STAT: u32 = 5;
/// Column of the AllowComponents flags (rows 3 to 9).
const COL_ALLOW: u32 = 8;

/// Flags that decide which optional components a chassis may carry (`Yes`/`No` in
/// the CSV, read with `readBoolean`). An absent cell is false.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllowComponents {
    /// Row 3, `Advanced Sensor`.
    pub advanced_sensor: bool,
    /// Row 4, `Optics`.
    pub optics: bool,
    /// Row 5, `GlobalSensor`.
    pub global_sensor: bool,
    /// Row 6, `ECM`.
    pub ecm: bool,
    /// Row 7, `ActiveProbe`.
    pub active_probe: bool,
    /// Row 8, `Jump Jets`.
    pub jump_jets: bool,
    /// Row 9, `Extended Sensor`.
    pub extended_sensor: bool,
}

/// Movement limits read from the chassis CSV (`MoverDynamics::init`, `code/mover.cpp:1584-1591`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChassisDynamics {
    /// `TorsoYawRate` (row 8, column 5): `max.mech.torsoYawRate`.
    pub torso_yaw_rate: i32,
    /// `MaxTorsoYaw` (row 9, column 5): `max.mech.torsoYaw`.
    pub max_torso_yaw: i32,
}

/// One inventory row of a variant (`Item0MasterId` ... `Item71MasterId`).
///
/// Columns follow the CSV header on row 25: 2 `Load#`, 3 `MechBay X`, 4 `MechBay Y`,
/// 5 `CompBas#`, 6 `Name`, 7 `Range`, 8 `Heat`, 9 `Weight`, 10 `1Hit Damage`,
/// 11 `Dam/10Sec`, 12 `BR`, 13 `C-Bills`, 14 `Comment`. The display columns are
/// kept as the cell text (for example `NA`); the game does not read them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemRow {
    /// 1-based CSV row of this item.
    pub row: u32,
    /// Column 2, raw text. The header says `Load#`; the value does not index the
    /// component table (Fusion Engine has 91 here, and `compbas` id 91 is empty).
    pub load_number: Option<String>,
    /// Column 3 `MechBay X`, read by `LogisticsVariant::init` (`code/logisticsvariant.cpp:360`).
    pub mechbay_x: Option<i64>,
    /// Column 4 `MechBay Y`, read by `LogisticsVariant::init` (`code/logisticsvariant.cpp:359`).
    pub mechbay_y: Option<i64>,
    /// Column 5 `CompBas#`. The C++ stores this as the inventory `masterID`
    /// (`code/mech.cpp:1516`) and indexes the master component list with it. 0 and
    /// 255 are empty (`code/mech.cpp:1516-1518`). Also the component id that
    /// `LogisticsVariant::init` reads (`code/logisticsvariant.cpp:354`).
    pub comp_bas: Option<u8>,
    /// Column 6 `Name`, cell text.
    pub name: Option<String>,
    /// Column 7 `Range`, cell text.
    pub range: Option<String>,
    /// Column 8 `Heat`, cell text. Unused (USEHEAT).
    pub heat: Option<String>,
    /// Column 9 `Weight`, cell text.
    pub weight: Option<String>,
    /// Column 10 `1Hit Damage`, cell text.
    pub hit_damage: Option<String>,
    /// Column 11 `Dam/10Sec`, cell text.
    pub damage_per_10s: Option<String>,
    /// Column 12 `BR`, cell text.
    pub br: Option<String>,
    /// Column 13 `C-Bills`, cell text.
    pub cbills: Option<String>,
    /// Column 14 `Comment`, cell text.
    pub comment: Option<String>,
}

impl ItemRow {
    /// Whether the C++ loaders install this row (non-zero, not 255 `comp_bas`).
    pub fn is_installed(&self) -> bool {
        matches!(self.comp_bas, Some(v) if v != 0 && v != 255)
    }
}

/// Totals row (row 99), cell text as the designer tool wrote it. Not read by the game.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ItemTotals {
    /// Column 8 `Heat` (unused).
    pub heat: Option<String>,
    /// Column 9 `Weight`.
    pub weight: Option<String>,
    /// Column 10 `1Hit Damage`.
    pub hit_damage: Option<String>,
    /// Column 11 `Dam/10Sec`.
    pub damage_per_10s: Option<String>,
    /// Column 12 `BR`.
    pub br: Option<String>,
    /// Column 13 `C-Bills`.
    pub cbills: Option<String>,
}

/// One loadout variant of a chassis (`VariantInfo` block, rows `23 + 97 * v` on).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChassisVariant {
    /// 0-based variant number, as `variantNum` in `BattleMech::init`.
    pub index: u32,
    /// `VariantName`, row `23 + 97v` column 2 (read with a 256-byte buffer as in `LogisticsVariant::init`).
    pub name: String,
    /// `VariantID` cell text, row `22 + 97v` column 2 (`0000`). Not read by the C++.
    pub variant_id: Option<String>,
    /// `bHidden`, row `21 + 97v` column 4, from `LogisticsVariant::init`. Absent is false.
    pub hidden: bool,
    /// The 72 inventory rows, in row order (`Item0` at row `26 + 97v`).
    pub items: Vec<ItemRow>,
    /// Totals row cell text.
    pub totals: ItemTotals,
    /// Critical slots per body location, in slot order. Each value is an index into
    /// `items` (the C++ maps it through `ItemLocationToInvLocation`, `code/mech.cpp:1732-1735`),
    /// and 255 marks an empty slot. `None` is an absent cell; the C++ leaves that
    /// slot uninitialised. Length is `BodyLocation::critical_spaces`.
    pub critical_slots: [Vec<Option<u8>>; NUM_BODY_LOCATIONS],
    /// Current internal structure per body location (row `114 + 97v`).
    pub current_internal: [Option<u8>; NUM_BODY_LOCATIONS],
    /// Current armor per armor location (row `115 + 97v`).
    pub current_armor: [Option<u8>; NUM_ARMOR_LOCATIONS],
}

impl ChassisVariant {
    /// Items the C++ loaders install, with their index in `items`.
    pub fn installed_items(&self) -> impl Iterator<Item = (usize, &ItemRow)> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.is_installed())
    }

    /// Critical slots of one body location.
    pub fn slots(&self, location: BodyLocation) -> &[Option<u8>] {
        &self.critical_slots[location.index()]
    }
}

/// A chassis CSV (`BaseChassisInfo`) with its variants.
#[derive(Debug, Clone, PartialEq)]
pub struct Chassis {
    /// Lower-case file stem, as `LogisticsChassis::fileName`.
    pub file_name: String,
    /// `MechName`, row 3 column 2 (`code/mech.cpp:1399`).
    pub name: String,
    /// `Tonnage`, row 3 column 5 (`mech.cpp:1405`; `maxWeight`, `logisticsvariant.cpp:56`).
    pub tonnage: f32,
    /// `Heat Index`, row 4 column 5 (`maxHeat`, `logisticsvariant.cpp:109`). Unused by the sim.
    pub heat_index: Option<i64>,
    /// `Load Index`, row 5 column 5. Not read by the C++ loaders.
    pub load_index: Option<i64>,
    /// `Total Armor`, row 6 column 5 (`baseArmor`, `logisticsvariant.cpp:112`).
    pub total_armor: Option<i64>,
    /// `MaxRunSpeed`, row 7 column 5 (`mech.cpp:1419`, read as `uc`).
    pub max_run_speed: u8,
    /// Torso yaw limits from rows 8 and 9.
    pub dynamics: ChassisDynamics,
    /// `ChassisBR`, row 10 column 5 (`mech.cpp:1403`).
    pub chassis_br: Option<i32>,
    /// `ChassisC-Bills`, row 11 column 5 (`baseCost`, `logisticsvariant.cpp:50`).
    pub base_cost: Option<i64>,
    /// `MAX Armor`, row 12 column 5 (`maxArmor`, `logisticsvariant.cpp:59`).
    pub max_armor: Option<i64>,
    /// `HouseID`, row 4 column 2 (`logisticsvariant.cpp:73`).
    pub house_id: Option<i64>,
    /// `DescIndex`, row 5 column 2 (`mech.cpp:1408`).
    pub descriptor_id: Option<i32>,
    /// Row 6 column 2, labelled `AbbrIndex`. `LogisticsChassis::init` reads it as
    /// `encyclopediaID` (`logisticsvariant.cpp:65`), so the label and the use differ.
    pub abbr_index: Option<i64>,
    /// Row 7 column 2, labelled `EncyclopediaID`. Not read by the C++.
    pub encyclopedia_id: Option<i64>,
    /// `HelpId`, row 8 column 2 (`logisticsvariant.cpp:62`).
    pub help_id: Option<i64>,
    /// `MechParts` value, row 12 column 2 (`iconPictureIndex`, `mech.cpp:1416`).
    pub icon_picture_index: Option<i32>,
    /// Icon and animation names: `LittleIcon`, `Animation`, `BigIcon` (rows 9 to 11).
    pub icon_files: [Option<String>; 3],
    /// `LogisticsX`, row 13 column 2 (`componentAreaWidth`).
    pub logistics_width: Option<i64>,
    /// `LogisticsY`, row 14 column 2 (`componentAreaHeight`).
    pub logistics_height: Option<i64>,
    /// AllowComponents flags.
    pub allow: AllowComponents,
    /// Base internal structure per body location (row 17, `mech.cpp:716`).
    pub base_internal: [u8; NUM_BODY_LOCATIONS],
    /// Base armor per armor location (row 18). `mech.cpp:1435-1455` does not check these.
    pub base_armor: [Option<u8>; NUM_ARMOR_LOCATIONS],
    /// Variants in file order, stopping at the first variant with no name, as the
    /// `LogisticsData::initVariants` loop does (`code/logisticsdata.cpp:322-331`).
    pub variants: Vec<ChassisVariant>,
}

fn schema(file: &str, detail: impl Into<String>) -> SimDataError {
    SimDataError::schema(file, detail)
}

impl Chassis {
    /// Reads a chassis from its CSV image. `file_name` is the lower-case stem.
    pub fn from_csv(file_name: &str, csv: &CsvFile) -> Result<Chassis> {
        let file = format!("{file_name}.csv");
        let name = csv
            .read_string(ROW_MECH_NAME, COL_VALUE, 127)?
            .ok_or_else(|| schema(&file, "MechName (row 3 col 2) is empty"))?;
        let tonnage = csv
            .read_f32(ROW_MECH_NAME, COL_STAT)
            .ok_or_else(|| schema(&file, "Tonnage (row 3 col 5) is missing"))?;
        let max_run_speed = csv
            .read_u8(7, COL_STAT)
            .ok_or_else(|| schema(&file, "MaxRunSpeed (row 7 col 5) is missing"))?;
        let torso_yaw_rate = csv
            .read_i32(8, COL_STAT)
            .ok_or_else(|| schema(&file, "TorsoYawRate (row 8 col 5) is missing"))?;
        let max_torso_yaw = csv
            .read_i32(9, COL_STAT)
            .ok_or_else(|| schema(&file, "MaxTorsoYaw (row 9 col 5) is missing"))?;

        let mut base_internal = [0u8; NUM_BODY_LOCATIONS];
        for loc in BodyLocation::ALL {
            base_internal[loc.index()] = csv
                .read_u8(ROW_BASE_INTERNAL, loc.csv_column())
                .ok_or_else(|| {
                    schema(
                        &file,
                        format!("BaseInternalStructure missing for {loc:?} (row 17)"),
                    )
                })?;
        }
        let base_armor = std::array::from_fn(|i| {
            csv.read_u8(ROW_BASE_ARMOR, ArmorLocation::ALL[i].csv_column())
        });

        let mut icon_files: [Option<String>; 3] = [None, None, None];
        for (i, slot) in icon_files.iter_mut().enumerate() {
            *slot = csv.cell(ROW_ICON_FIRST + i as u32, COL_VALUE);
        }

        let allow = AllowComponents {
            advanced_sensor: csv.read_bool(3, COL_ALLOW).unwrap_or(false),
            optics: csv.read_bool(4, COL_ALLOW).unwrap_or(false),
            global_sensor: csv.read_bool(5, COL_ALLOW).unwrap_or(false),
            ecm: csv.read_bool(6, COL_ALLOW).unwrap_or(false),
            active_probe: csv.read_bool(7, COL_ALLOW).unwrap_or(false),
            jump_jets: csv.read_bool(8, COL_ALLOW).unwrap_or(false),
            extended_sensor: csv.read_bool(9, COL_ALLOW).unwrap_or(false),
        };

        let mut variants = Vec::new();
        for index in 0u32.. {
            let name_row = VARIANT_BASE_ROW + VARIANT_STRIDE * index;
            if name_row > csv.rows() {
                break;
            }
            if csv.read_string(name_row, COL_VALUE, 256)?.is_none() {
                break;
            }
            variants.push(ChassisVariant::from_csv(csv, &file, index)?);
        }

        Ok(Chassis {
            file_name: file_name.to_string(),
            name,
            tonnage,
            heat_index: csv.read_i64(4, COL_STAT),
            load_index: csv.read_i64(5, COL_STAT),
            total_armor: csv.read_i64(6, COL_STAT),
            max_run_speed,
            dynamics: ChassisDynamics {
                torso_yaw_rate,
                max_torso_yaw,
            },
            chassis_br: csv.read_i32(10, COL_STAT),
            base_cost: csv.read_i64(11, COL_STAT),
            max_armor: csv.read_i64(ROW_MECH_PARTS, COL_STAT),
            house_id: csv.read_i64(ROW_HOUSE_ID, COL_VALUE),
            descriptor_id: csv.read_i32(ROW_DESC_INDEX, COL_VALUE),
            abbr_index: csv.read_i64(ROW_ABBR_INDEX, COL_VALUE),
            encyclopedia_id: csv.read_i64(ROW_ENCYCLOPEDIA_ID, COL_VALUE),
            help_id: csv.read_i64(ROW_HELP_ID, COL_VALUE),
            icon_picture_index: csv.read_i32(ROW_MECH_PARTS, COL_VALUE),
            icon_files,
            logistics_width: csv.read_i64(ROW_LOGISTICS_X, COL_VALUE),
            logistics_height: csv.read_i64(ROW_LOGISTICS_Y, COL_VALUE),
            allow,
            base_internal,
            base_armor,
            variants,
        })
    }
}

impl ChassisVariant {
    /// Reads variant `index`, whose name row must exist.
    pub fn from_csv(csv: &CsvFile, file: &str, index: u32) -> Result<ChassisVariant> {
        let base = VARIANT_BASE_ROW + VARIANT_STRIDE * index;
        let name = csv
            .read_string(base, COL_VALUE, 256)?
            .ok_or_else(|| schema(file, format!("variant {index} has no name at row {base}")))?;

        let mut items = Vec::with_capacity(MAX_INVENTORY_ITEMS);
        for i in 0..MAX_INVENTORY_ITEMS as u32 {
            items.push(read_item(csv, base + ITEM_OFFSET + i));
        }

        let totals_row = base + TOTALS_OFFSET;
        let totals = ItemTotals {
            heat: csv.cell(totals_row, 8),
            weight: csv.cell(totals_row, 9),
            hit_damage: csv.cell(totals_row, 10),
            damage_per_10s: csv.cell(totals_row, 11),
            br: csv.cell(totals_row, 12),
            cbills: csv.cell(totals_row, 13),
        };

        let critical_slots = std::array::from_fn(|i| {
            let loc = BodyLocation::ALL[i];
            (0..loc.critical_spaces() as u32)
                .map(|s| csv.read_u8(base + SLOT_OFFSET + s, loc.csv_column()))
                .collect()
        });

        let current_internal = std::array::from_fn(|i| {
            let loc = BodyLocation::ALL[i];
            csv.read_u8(base + CUR_INTERNAL_OFFSET, loc.csv_column())
        });
        let current_armor = std::array::from_fn(|i| {
            let loc = ArmorLocation::ALL[i];
            csv.read_u8(base + CUR_ARMOR_OFFSET, loc.csv_column())
        });

        Ok(ChassisVariant {
            index,
            name,
            variant_id: csv.cell(base - 1, COL_VALUE),
            hidden: csv.read_bool(base - 2, 4).unwrap_or(false),
            items,
            totals,
            critical_slots,
            current_internal,
            current_armor,
        })
    }
}

fn read_item(csv: &CsvFile, row: u32) -> ItemRow {
    ItemRow {
        row,
        load_number: csv.cell(row, 2),
        mechbay_x: csv.read_i64(row, 3),
        mechbay_y: csv.read_i64(row, 4),
        comp_bas: csv.read_u8(row, 5),
        name: csv.cell(row, 6),
        range: csv.cell(row, 7),
        heat: csv.cell(row, 8),
        weight: csv.cell(row, 9),
        hit_damage: csv.cell(row, 10),
        damage_per_10s: csv.cell(row, 11),
        br: csv.cell(row, 12),
        cbills: csv.cell(row, 13),
        comment: csv.cell(row, 14),
    }
}

/// Reads the chassis names in `variants.csv` (one column), from row 1 down.
///
/// The game does not read this file (`code/` has no reference to it). It is the
/// designer's list of chassis names, one per row from row 1.
pub fn variant_list_names(csv: &CsvFile) -> Vec<String> {
    // The file has no comma, so column 1 is past the column count and unreachable.
    // Column 0 aliases the first field (`seekRowCol`), which is what reads it.
    (1..=csv.rows()).filter_map(|r| csv.cell(r, 0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a CSV image from `(row, col, text)` cells, 1-based as the game counts.
    /// Line 1 gets one extra field so column `max_col` is addressable (the game
    /// counts columns as commas on line 1), and a blank last line makes the last
    /// data row addressable (`countRows` leaves the final line out).
    pub(crate) fn csv_from(cells: &[(u32, u32, &str)]) -> CsvFile {
        let rows = cells.iter().map(|c| c.0).max().unwrap_or(1);
        let cols = cells.iter().map(|c| c.1).max().unwrap_or(1) + 1;
        let mut grid = vec![vec![String::new(); cols as usize]; rows as usize];
        for &(r, c, v) in cells {
            grid[(r - 1) as usize][(c - 1) as usize] = v.to_string();
        }
        let mut out = String::new();
        for line in &grid {
            out.push_str(&line.join(","));
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        CsvFile::from_bytes(out.into_bytes()).unwrap()
    }

    /// Cells every chassis needs: the header block and the required stats.
    fn base_cells() -> Vec<(u32, u32, &'static str)> {
        vec![
            (3, 2, "Testmech"),
            (3, 5, "35"),
            (4, 2, "1"),
            (4, 5, "22"),
            (5, 2, "28912"),
            (5, 5, "16"),
            (6, 2, "28946"),
            (6, 5, "80"),
            (7, 2, "-1"),
            (7, 5, "27"),
            (8, 2, "-1"),
            (8, 5, "360"),
            (9, 2, "MCL_ICON"),
            (9, 5, "120"),
            (10, 5, "57"),
            (11, 5, "22250"),
            (12, 2, "23"),
            (12, 5, "144"),
            (13, 2, "4"),
            (14, 2, "2"),
            (3, 8, "No"),
            (4, 8, "No"),
            (5, 8, "No"),
            (6, 8, "No"),
            (7, 8, "No"),
            (8, 8, "yes"),
            (9, 8, "yes"),
            // Base internal structure (row 17) and armor (row 18).
            (17, 2, "3"),
            (17, 3, "5"),
            (17, 4, "5"),
            (17, 5, "7"),
            (17, 6, "7"),
            (17, 7, "10"),
            (17, 8, "7"),
            (17, 9, "7"),
            (18, 2, "9"),
            (18, 3, "8"),
            (18, 4, "8"),
            (18, 5, "9"),
            (18, 6, "9"),
            (18, 7, "11"),
            (18, 8, "9"),
            (18, 9, "9"),
            (18, 10, "2"),
            (18, 11, "2"),
            (18, 12, "4"),
        ]
    }

    /// Adds variant `v` with one installed item, a slot, and current values.
    fn variant_cells(v: u32, name: &str) -> Vec<(u32, u32, String)> {
        let b = VARIANT_BASE_ROW + VARIANT_STRIDE * v;
        vec![
            (b, 2, name.to_string()),
            (b - 1, 2, "0000".to_string()),
            (b - 2, 4, "TRUE".to_string()),
            (b + 3, 2, "91".to_string()),
            (b + 3, 3, "-1".to_string()),
            (b + 3, 4, "-1".to_string()),
            (b + 3, 5, "1".to_string()),
            (b + 3, 6, "Fusion Engine".to_string()),
            (b + 3, 9, "0.00".to_string()),
            (b + 4, 5, "255".to_string()),
            (b + 5, 5, "0".to_string()),
            (b + 76, 9, "0.00".to_string()),
            (b + 76, 10, "407.50".to_string()),
            (b + 76, 13, "29450".to_string()),
            // Critical slots: Head slot 1 = item 6, LeftTorso slot 1 = item 38, LeftLeg slot 1 = item 11.
            (b + 79, 2, "6".to_string()),
            (b + 79, 5, "38".to_string()),
            (b + 79, 8, "11".to_string()),
            (b + 91, 2, "3".to_string()),
            (b + 91, 7, "10".to_string()),
            (b + 92, 2, "9".to_string()),
            (b + 92, 12, "4".to_string()),
        ]
    }

    fn csv_with(extra: &[(u32, u32, String)]) -> CsvFile {
        let mut cells: Vec<(u32, u32, &str)> = base_cells();
        let owned: Vec<(u32, u32, &str)> =
            extra.iter().map(|(r, c, s)| (*r, *c, s.as_str())).collect();
        cells.extend(owned.iter().copied());
        csv_from(&cells)
    }

    #[test]
    fn reads_chassis_block_and_one_variant() {
        let extra = variant_cells(0, "Razorback Prime");
        let csv = csv_with(&extra);
        let chassis = Chassis::from_csv("testmech", &csv).unwrap();

        assert_eq!(chassis.name, "Testmech");
        assert_eq!(chassis.tonnage, 35.0);
        assert_eq!(chassis.max_run_speed, 27);
        assert_eq!(
            chassis.dynamics,
            ChassisDynamics {
                torso_yaw_rate: 360,
                max_torso_yaw: 120
            }
        );
        assert_eq!(chassis.chassis_br, Some(57));
        assert_eq!(chassis.base_cost, Some(22250));
        assert_eq!(chassis.max_armor, Some(144));
        assert_eq!(chassis.total_armor, Some(80));
        assert_eq!(chassis.heat_index, Some(22));
        assert_eq!(chassis.load_index, Some(16));
        assert_eq!(chassis.house_id, Some(1));
        assert_eq!(chassis.descriptor_id, Some(28912));
        assert_eq!(chassis.abbr_index, Some(28946));
        assert_eq!(chassis.help_id, Some(-1));
        assert_eq!(chassis.icon_picture_index, Some(23));
        assert_eq!(chassis.icon_files[0].as_deref(), Some("MCL_ICON"));
        assert_eq!(chassis.logistics_width, Some(4));
        assert_eq!(chassis.logistics_height, Some(2));
        assert_eq!(
            chassis.allow,
            AllowComponents {
                jump_jets: true,
                extended_sensor: true,
                ..AllowComponents::default()
            }
        );
        assert_eq!(chassis.base_internal, [3, 10, 7, 7, 5, 5, 7, 7]);
        assert_eq!(
            chassis.base_armor[ArmorLocation::RearCenterTorso.index()],
            Some(4)
        );
        assert_eq!(
            chassis.base_armor[ArmorLocation::RearLeftTorso.index()],
            Some(2)
        );

        assert_eq!(chassis.variants.len(), 1);
        let v = &chassis.variants[0];
        assert_eq!(v.name, "Razorback Prime");
        assert_eq!(v.variant_id.as_deref(), Some("0000"));
        assert!(v.hidden);
        assert_eq!(v.items.len(), MAX_INVENTORY_ITEMS);
        assert_eq!(v.items[0].row, 26);
        assert_eq!(v.items[0].comp_bas, Some(1));
        assert_eq!(v.items[0].name.as_deref(), Some("Fusion Engine"));
        assert_eq!(v.items[0].load_number.as_deref(), Some("91"));
        assert_eq!(v.items[0].mechbay_x, Some(-1));
        assert_eq!(v.items[1].comp_bas, Some(255));
        assert_eq!(v.items[2].comp_bas, Some(0));
        assert_eq!(v.installed_items().count(), 1);
        assert_eq!(v.totals.cbills.as_deref(), Some("29450"));
        assert_eq!(v.totals.hit_damage.as_deref(), Some("407.50"));

        assert_eq!(v.critical_slots[BodyLocation::Head.index()].len(), 6);
        assert_eq!(v.slots(BodyLocation::Head)[0], Some(6));
        assert_eq!(v.slots(BodyLocation::LeftTorso)[0], Some(38));
        assert_eq!(v.slots(BodyLocation::LeftLeg).len(), 6);
        assert_eq!(v.slots(BodyLocation::LeftLeg)[0], Some(11));
        assert_eq!(v.slots(BodyLocation::RightLeg)[0], None);
        assert_eq!(v.current_internal[BodyLocation::Head.index()], Some(3));
        assert_eq!(
            v.current_internal[BodyLocation::CenterTorso.index()],
            Some(10)
        );
        assert_eq!(v.current_armor[ArmorLocation::Head.index()], Some(9));
        assert_eq!(
            v.current_armor[ArmorLocation::RearCenterTorso.index()],
            Some(4)
        );
    }

    #[test]
    fn variant_stride_is_97_rows_and_list_stops_at_first_gap() {
        let mut extra = variant_cells(0, "Base");
        extra.extend(variant_cells(1, "Second"));
        // A third variant after a gap is not reached.
        extra.extend(variant_cells(3, "Unreached"));
        let csv = csv_with(&extra);
        let chassis = Chassis::from_csv("testmech", &csv).unwrap();
        let names: Vec<&str> = chassis.variants.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Base", "Second"]);
        assert_eq!(chassis.variants[1].index, 1);
        // Variant 1 reads its own name row (120), item row (123) and totals (196).
        assert_eq!(chassis.variants[1].items[0].row, 123);
        assert_eq!(chassis.variants[1].totals.cbills.as_deref(), Some("29450"));
    }

    #[test]
    fn missing_tonnage_is_a_schema_error() {
        let csv = csv_from(&[(3, 2, "Testmech"), (7, 5, "27")]);
        let err = Chassis::from_csv("broken", &csv).unwrap_err();
        assert!(matches!(err, SimDataError::Schema { .. }), "{err}");
    }

    #[test]
    fn variant_list_reads_column_one() {
        let csv = csv_from(&[(1, 1, "Madcat"), (2, 1, "Raven"), (4, 1, "Atlas")]);
        assert_eq!(variant_list_names(&csv), ["Madcat", "Raven", "Atlas"]);
    }
}
