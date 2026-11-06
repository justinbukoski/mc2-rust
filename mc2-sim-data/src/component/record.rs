// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The typed master component record, one per `compbas.csv` row.
//!
//! Port of `MasterComponent::initEXCEL` (mclib/cmponent.cpp:87-264) and of the
//! logistics reader `LogisticsComponent::init` (code/logisticscomponent.cpp:79-180),
//! which both read the same row. The master reader splits the row with `strtok`
//! and drops empty cells. The logistics reader splits it by position and keeps
//! them. Both views are read here, each the way its C++ reader does.

use super::cparse::{atof, atoi, strtok_fields};
use crate::error::{Result, SimDataError};

/// `WEAPON_FLAG_STREAK` (cmponent.h:103).
pub const WEAPON_FLAG_STREAK: u8 = 1;
/// `WEAPON_FLAG_INFERNO` (cmponent.h:104). Named in the header, never set by the loader.
pub const WEAPON_FLAG_INFERNO: u8 = 2;
/// `WEAPON_FLAG_LBX` (cmponent.h:105). Named in the header, never set by the loader.
pub const WEAPON_FLAG_LBX: u8 = 4;
/// `WEAPON_FLAG_ARTILLERY` (cmponent.h:106). Named in the header, never set by the loader.
pub const WEAPON_FLAG_ARTILLERY: u8 = 8;

/// Number of body locations in the critical-space columns (`NUM_BODY_LOCATIONS`).
pub const NUM_BODY_LOCATIONS: usize = 8;

/// `ComponentFormType` (cmponent.h:52-74). The file's form column must match
/// one of these spellings exactly (`strcmp`, cmponent.cpp:105-106).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ComponentForm {
    /// `COMPONENT_FORM_SIMPLE`, file text "Simple".
    Simple,
    /// `COMPONENT_FORM_COCKPIT`, "Cockpit".
    Cockpit,
    /// `COMPONENT_FORM_SENSOR`, "Sensors" (plural in the file).
    Sensor,
    /// `COMPONENT_FORM_ACTUATOR`, "Actuator".
    Actuator,
    /// `COMPONENT_FORM_ENGINE`, "Engine".
    Engine,
    /// `COMPONENT_FORM_HEATSINK`, "HeatSink".
    HeatSink,
    /// `COMPONENT_FORM_WEAPON`, "Weapon". The loader has no case for it and returns an error.
    Weapon,
    /// `COMPONENT_FORM_WEAPON_ENERGY`, "EnergyWeapon".
    WeaponEnergy,
    /// `COMPONENT_FORM_WEAPON_BALLISTIC`, "BallisticWeapon".
    WeaponBallistic,
    /// `COMPONENT_FORM_WEAPON_MISSILE`, "MissileWeapon".
    WeaponMissile,
    /// `COMPONENT_FORM_AMMO`, "Ammo".
    Ammo,
    /// `COMPONENT_FORM_JUMPJET`, "JumpJet".
    JumpJet,
    /// `COMPONENT_FORM_CASE`, "Case".
    Case,
    /// `COMPONENT_FORM_LIFESUPPORT`, "LifeSupport".
    LifeSupport,
    /// `COMPONENT_FORM_GYROSCOPE`, "Gyroscope".
    Gyroscope,
    /// `COMPONENT_FORM_POWER_AMPLIFIER`, "PowerAmplifier".
    PowerAmplifier,
    /// `COMPONENT_FORM_ECM`, "ECM".
    Ecm,
    /// `COMPONENT_FORM_PROBE`, "Probe".
    Probe,
    /// `COMPONENT_FORM_JAMMER`, "Jammer".
    Jammer,
    /// `COMPONENT_FORM_BULK`, "Bulk".
    Bulk,
}

impl ComponentForm {
    /// Looks up a form by its exact file spelling.
    pub fn from_text(text: &str) -> Option<Self> {
        Some(match text {
            "Simple" => Self::Simple,
            "Cockpit" => Self::Cockpit,
            "Sensors" => Self::Sensor,
            "Actuator" => Self::Actuator,
            "Engine" => Self::Engine,
            "HeatSink" => Self::HeatSink,
            "Weapon" => Self::Weapon,
            "EnergyWeapon" => Self::WeaponEnergy,
            "BallisticWeapon" => Self::WeaponBallistic,
            "MissileWeapon" => Self::WeaponMissile,
            "Ammo" => Self::Ammo,
            "JumpJet" => Self::JumpJet,
            "Case" => Self::Case,
            "LifeSupport" => Self::LifeSupport,
            "Gyroscope" => Self::Gyroscope,
            "PowerAmplifier" => Self::PowerAmplifier,
            "ECM" => Self::Ecm,
            "Probe" => Self::Probe,
            "Jammer" => Self::Jammer,
            "Bulk" => Self::Bulk,
            _ => return None,
        })
    }

    /// The file spelling of this form.
    pub fn text(self) -> &'static str {
        match self {
            Self::Simple => "Simple",
            Self::Cockpit => "Cockpit",
            Self::Sensor => "Sensors",
            Self::Actuator => "Actuator",
            Self::Engine => "Engine",
            Self::HeatSink => "HeatSink",
            Self::Weapon => "Weapon",
            Self::WeaponEnergy => "EnergyWeapon",
            Self::WeaponBallistic => "BallisticWeapon",
            Self::WeaponMissile => "MissileWeapon",
            Self::Ammo => "Ammo",
            Self::JumpJet => "JumpJet",
            Self::Case => "Case",
            Self::LifeSupport => "LifeSupport",
            Self::Gyroscope => "Gyroscope",
            Self::PowerAmplifier => "PowerAmplifier",
            Self::Ecm => "ECM",
            Self::Probe => "Probe",
            Self::Jammer => "Jammer",
            Self::Bulk => "Bulk",
        }
    }

    /// `LogisticsComponent::isWeapon` (logisticscomponent.cpp:262-269).
    pub fn is_weapon(self) -> bool {
        matches!(
            self,
            Self::Weapon | Self::WeaponEnergy | Self::WeaponBallistic | Self::WeaponMissile
        )
    }
}

/// `WeaponAmmoType` (cmponent.h:76-83), from the file's missile-type column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponAmmoType {
    /// `WEAPON_AMMO_NONE`: energy weapons, and any text other than the codes below.
    Unlimited,
    /// `WEAPON_AMMO_SRM`, text "SRM".
    Srm,
    /// `WEAPON_AMMO_LRM`, text "LRM".
    Lrm,
    /// `WEAPON_AMMO_ST` (Streak), text "ST". Also sets the streak flag.
    St,
    /// `WEAPON_AMMO_LIMITED`, text "1" (ballistic weapons).
    Limited,
}

impl WeaponAmmoType {
    fn from_field(text: &str) -> Self {
        match text {
            "1" => Self::Limited,
            "SRM" => Self::Srm,
            "LRM" => Self::Lrm,
            "ST" => Self::St,
            _ => Self::Unlimited,
        }
    }
}

/// `WeaponRangeType` (cmponent.h:92-99) as the file can express it. The C++
/// enum also has `SHORT_MEDIUM` and `MEDIUM_LONG`, which the loader never produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WeaponRange {
    /// `WEAPON_RANGE_SHORT`, text "short".
    Short,
    /// `WEAPON_RANGE_MEDIUM`, text "medium".
    Medium,
    /// `WEAPON_RANGE_LONG`, text "long".
    Long,
}

impl WeaponRange {
    /// Index into the `WeaponRanges` table (`WEAPON_RANGE_*` values 0, 1, 2).
    pub fn index(self) -> usize {
        match self {
            Self::Short => 0,
            Self::Medium => 1,
            Self::Long => 2,
        }
    }

    fn from_lower(text: &str) -> Option<Self> {
        match text {
            "short" => Some(Self::Short),
            "medium" => Some(Self::Medium),
            "long" => Some(Self::Long),
            _ => None,
        }
    }
}

/// The weapon half of `ComponentStats.weapon` (cmponent.h:128-139).
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponStats {
    /// Heat per shot, truncated to a whole number as the loader does (cmponent.cpp:127).
    /// Loaded but never used: `USEHEAT` is not defined in this build, so every
    /// heat read sits in an `#ifdef USEHEAT` block.
    pub heat: f32,
    /// One-hit damage (column "Damage", cmponent.cpp:133, 231).
    pub damage: f32,
    /// Recycle time in seconds (column "Recycle", cmponent.cpp:124, 232).
    pub recycle_time: f32,
    /// Shots per ammo load. The loader always writes 1 (cmponent.cpp:234, 240).
    pub ammo_amount: i32,
    /// Ammo class from the missile-type column.
    pub ammo_type: WeaponAmmoType,
    /// Band from the range column. `None` where the file says `0` (cmponent.cpp:142-153).
    pub range: Option<WeaponRange>,
    /// Streak flag only: set when `ammo_type` is `St` (cmponent.cpp:239). The
    /// C++ never sets the other flags from the file. See `declared_flags`.
    pub flags: u8,
    /// Special effect id, an index into `effects.csv` (`WeaponEffects::init`).
    /// Read as a C `char` (cmponent.cpp:192), so it wraps like a signed byte.
    pub special_effect: i8,
    /// Ammo component id this weapon fires (cmponent.cpp:197). Read as a C `char`
    /// and stored unsigned.
    pub ammo_master_id: u8,
    /// Value of the "Fields" column (1 streak, 2 inferno, 4 LBX, 8 artillery).
    /// The C++ reads it into a local and never uses it (cmponent.cpp:184-187),
    /// so the flags it names are not set in the game.
    pub declared_flags: i32,
}

impl WeaponStats {
    /// Damage over ten seconds, the effectiveness basis of the inventory (`gvehicl.cpp:1136`):
    /// `damage * 10 / recycle`, in double precision as the C++ computes it. The file has
    /// no such column (the chassis CSVs do, as "Dam/10Sec").
    pub fn damage_per_10_seconds(&self) -> f64 {
        f64::from(self.damage) * 10.0 / f64::from(self.recycle_time)
    }
}

/// `ComponentStats`, the form-specific part of the record (cmponent.h:108-147).
#[derive(Debug, Clone, PartialEq)]
pub enum ComponentStats {
    /// No form-specific data. Covers Simple, Cockpit, Actuator, Engine (rating forced
    /// to 0, cmponent.cpp:223), Case, LifeSupport, Gyroscope, PowerAmplifier and Bulk.
    /// Also the generic `Weapon` form, whose loader case is missing (cmponent.cpp:259-260).
    None,
    /// Sensors. Range is the file's damage column times `BaseSensorRange` (cmponent.cpp:207).
    Sensor {
        /// Detection range in meters.
        range: f32,
    },
    /// ECM suite (cmponent.cpp:210-212).
    Ecm {
        /// Effect, from the damage column.
        effect: f32,
        /// Range, from the recycle column.
        range: f32,
    },
    /// Jammer (cmponent.cpp:215).
    Jammer {
        /// Effect, from the damage column.
        effect: f32,
    },
    /// Probe (cmponent.cpp:218).
    Probe {
        /// Effect, from the damage column.
        effect: f32,
    },
    /// Heat sink dissipation, from the damage column as a C `short` (cmponent.cpp:226).
    HeatSink {
        /// Dissipation per turn.
        dissipation: i16,
    },
    /// Energy, ballistic or missile weapon.
    Weapon(WeaponStats),
    /// Ammo (cmponent.cpp:243-244).
    Ammo {
        /// Shots per ton, from the recycle column as a C `long`.
        ammo_per_ton: i32,
        /// Explosive damage per missile, from the damage column.
        explosive_damage: f32,
    },
    /// Jump jet range modifier, from the damage column as a C `long` (cmponent.cpp:247).
    JumpJet {
        /// Range modifier, truncated to an integer.
        range_mod: i32,
    },
}

/// The logistics columns (LogisticsComponent::init, logisticscomponent.cpp:151-177).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogisticsEntry {
    /// Icon file name (column "Logistics Icon 1"). `None` where empty or starting
    /// with `0`. `LogisticsComponent::init` rejects such rows, so the component is
    /// not in the purchase list.
    pub icon: Option<String>,
    /// Picture file name (column "Logistics Icon 2"). `None` where empty.
    pub picture: Option<String>,
    /// String table index (column "String Index"). The logistics reader loads the name from it.
    pub string_index: i32,
    /// Help text string index (column "Mechlipidia String").
    pub help_string_index: i32,
    /// Icon X position, or `-1` when the cell is absent.
    pub icon_x: i32,
    /// Icon Y position, or `-1` when the cell is absent.
    pub icon_y: i32,
}

/// One master component record (`MasterComponent`, cmponent.h:154-172).
#[derive(Debug, Clone, PartialEq)]
pub struct MasterComponent {
    /// Row id (column 0). The loader indexes the master list by this value.
    pub master_id: i32,
    /// Form text as written in the file, kept even when it is not a known form.
    pub form_text: String,
    /// Known form, or `None` for rows the C++ rejects (`undefined`, `Removed`, `Optics`
    /// and any other text not in `ComponentFormString`, cmponent.cpp:108-109).
    pub form: Option<ComponentForm>,
    /// Name from column 2. The C++ loads the name from string table `32000 + id`
    /// instead (cmponent.cpp:100). Column 2 carries the same English text.
    pub name: String,
    /// Critical slots used, from column "crit hits" (C++ `size`, cmponent.cpp:121).
    pub size: i8,
    /// Hits before destruction. The loader sets 1 for every row (cmponent.cpp:96).
    pub health: u8,
    /// Weight in tons (column "Weight", cmponent.cpp:130).
    pub tonnage: f32,
    /// Battle rating, the C++ `CV` (column "BR", cmponent.cpp:136).
    pub battle_rating: f32,
    /// Resource cost in C-Bills (column "RP", cmponent.cpp:139). The logistics
    /// reader calls the same column `cost`.
    pub resource_points: i32,
    /// Critical slots needed in each location, in the order head, CT, LT, RT, LA,
    /// RA, LL, RL. `-1` means the file says `No`, `0` means `Yes`, and any other
    /// value is a count (cmponent.cpp:156-169).
    pub critical_spaces: [i8; NUM_BODY_LOCATIONS],
    /// Form-specific stats.
    pub stats: ComponentStats,
    /// Logistics columns.
    pub logistics: LogisticsEntry,
}

impl MasterComponent {
    /// `true` when the logistics reader would list this component: a known form
    /// and an icon (`LogisticsComponent::init` returns -1 otherwise).
    pub fn is_logistics_listed(&self) -> bool {
        self.form.is_some() && self.logistics.icon.is_some()
    }

    /// Critical slots needed at `location` (0 = head ... 7 = RL). Returns `-1`
    /// outside 0..8. The C++ check is `location > NUM_BODY_LOCATIONS`, which
    /// lets index 8 read past the array (cmponent.h:243).
    pub fn critical_space_at(&self, location: usize) -> i8 {
        self.critical_spaces.get(location).copied().unwrap_or(-1)
    }

    /// The weapon stats, when this is a weapon with stats.
    pub fn weapon(&self) -> Option<&WeaponStats> {
        match &self.stats {
            ComponentStats::Weapon(weapon) => Some(weapon),
            _ => None,
        }
    }
}

/// Record for a row whose form is not known. Id, name and the logistics columns are
/// set, and everything else keeps the defaults.
fn unset_record(
    master_id: i32,
    form_text: &str,
    name: String,
    logistics: LogisticsEntry,
) -> MasterComponent {
    MasterComponent {
        master_id,
        form_text: form_text.to_string(),
        form: None,
        name,
        size: 0,
        health: 1,
        tonnage: 0.0,
        battle_rating: 0.0,
        resource_points: 0,
        critical_spaces: [-1; NUM_BODY_LOCATIONS],
        stats: ComponentStats::None,
        logistics,
    }
}

/// Reads a logistics integer cell the way `extractInt` does. An empty or missing
/// cell gives `-1`.
fn raw_int(raw: &[&str], index: usize) -> i32 {
    match raw.get(index) {
        Some(cell) if !cell.is_empty() => atoi(cell) as i32,
        _ => -1,
    }
}

/// Reads the logistics columns by position, keeping empty cells.
fn parse_logistics(raw: &[&str]) -> LogisticsEntry {
    let cell = |index: usize| raw.get(index).copied().unwrap_or("");
    let icon = cell(23);
    let picture = cell(24);
    LogisticsEntry {
        icon: (!icon.is_empty() && !icon.starts_with('0')).then(|| icon.to_string()),
        picture: (!picture.is_empty()).then(|| picture.to_string()),
        string_index: raw_int(raw, 25),
        help_string_index: raw_int(raw, 26),
        icon_x: raw_int(raw, 27),
        icon_y: raw_int(raw, 28),
    }
}

/// Parses one data row of `compbas.csv`.
///
/// `row` is the 1-based line number, for error messages. `base_sensor_range` is
/// `BaseSensorRange` from `gamesys.fit`.
pub(crate) fn parse_row(
    line: &str,
    row: usize,
    file: &str,
    base_sensor_range: f32,
) -> Result<MasterComponent> {
    let raw: Vec<&str> = line.split(',').collect();
    let tokens = strtok_fields(line);
    let need = |index: usize| -> Result<&str> {
        tokens.get(index).copied().ok_or_else(|| {
            SimDataError::schema(file, format!("row {row}: cell {index} is missing"))
        })
    };
    let optional = |index: usize| tokens.get(index).copied();

    let master_id = atoi(need(0)?) as i32;
    let form_text = need(1)?;
    let name = need(2)?.to_string();
    let logistics = parse_logistics(&raw);
    let Some(form) = ComponentForm::from_text(form_text) else {
        return Ok(unset_record(master_id, form_text, name, logistics));
    };

    let size = atof(need(3)?) as i8;
    let recycle = atof(need(4)?) as f32;
    let heat = (atof(need(5)?) as u64) as f32;
    let tonnage = atof(need(6)?) as f32;
    let damage = atof(need(7)?) as f32;
    let battle_rating = atof(need(8)?) as f32;
    let resource_points = atoi(need(9)?) as i32;

    let range_text = need(10)?.to_ascii_lowercase();
    let range = if range_text == "0" {
        None
    } else {
        let parsed = WeaponRange::from_lower(&range_text).ok_or_else(|| {
            SimDataError::out_of_range(file, format!("row {row}: bad weapon range {range_text:?}"))
        })?;
        Some(parsed)
    };

    let mut critical_spaces = [-1i8; NUM_BODY_LOCATIONS];
    for (location, slot) in critical_spaces.iter_mut().enumerate() {
        let cell = need(11 + location)?;
        *slot = match cell {
            "No" => -1,
            "Yes" => 0,
            other => atoi(other) as i8,
        };
    }

    let ammo_type = optional(19).map_or(WeaponAmmoType::Unlimited, WeaponAmmoType::from_field);
    let declared_flags = optional(20).map_or(0, |cell| atoi(cell) as i32);
    let special_effect = optional(21).map_or(0, |cell| atoi(cell) as i8);
    let ammo_master_id = optional(22).map_or(0, |cell| (atoi(cell) as i8) as u8);

    let stats = match form {
        ComponentForm::Simple
        | ComponentForm::Cockpit
        | ComponentForm::Actuator
        | ComponentForm::Engine
        | ComponentForm::Weapon
        | ComponentForm::Case
        | ComponentForm::LifeSupport
        | ComponentForm::Gyroscope
        | ComponentForm::PowerAmplifier
        | ComponentForm::Bulk => ComponentStats::None,
        ComponentForm::Sensor => ComponentStats::Sensor {
            range: damage * base_sensor_range,
        },
        ComponentForm::Ecm => ComponentStats::Ecm {
            effect: damage,
            range: recycle,
        },
        ComponentForm::Jammer => ComponentStats::Jammer { effect: damage },
        ComponentForm::Probe => ComponentStats::Probe { effect: damage },
        ComponentForm::HeatSink => ComponentStats::HeatSink {
            dissipation: damage as i16,
        },
        ComponentForm::WeaponEnergy
        | ComponentForm::WeaponBallistic
        | ComponentForm::WeaponMissile => ComponentStats::Weapon(WeaponStats {
            heat,
            damage,
            recycle_time: recycle,
            ammo_amount: 1,
            ammo_type,
            range,
            flags: if ammo_type == WeaponAmmoType::St {
                WEAPON_FLAG_STREAK
            } else {
                0
            },
            special_effect,
            ammo_master_id,
            declared_flags,
        }),
        ComponentForm::Ammo => ComponentStats::Ammo {
            ammo_per_ton: recycle as i32,
            explosive_damage: damage,
        },
        ComponentForm::JumpJet => ComponentStats::JumpJet {
            range_mod: damage as i32,
        },
    };

    Ok(MasterComponent {
        master_id,
        form_text: form_text.to_string(),
        form: Some(form),
        name,
        size,
        health: 1,
        tonnage,
        battle_rating,
        resource_points,
        critical_spaces,
        stats,
        logistics,
    })
}
