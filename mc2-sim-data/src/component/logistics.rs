// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The logistics view of the component table (`compbas.csv`).
//!
//! Port of `code/logisticscomponent.cpp` (`LogisticsComponent::init` and its `extract*`
//! helpers) and of `LogisticsData::initComponents` (`code/logisticsdata.cpp:96-163`). The
//! master list ([`super::MasterComponentList`]) and this catalogue read the same rows with
//! different rules: this reader splits by position and keeps empty cells, matches the form name
//! ignoring case, keeps heat as a float, and resolves the ammo column through the rows it
//! rejected.

use super::cparse::{atof, atoi, split_lines};
use super::{ComponentForm, COMPBAS_FILE};
use crate::error::{Result, SimDataError};
use mc2_formats::vfs::Vfs;

/// `ComponentFormString` order (`mclib/cmponent.cpp:25-47`); the index is the C++ form number.
pub const COMPONENT_FORMS: [ComponentForm; 20] = [
    ComponentForm::Simple,
    ComponentForm::Cockpit,
    ComponentForm::Sensor,
    ComponentForm::Actuator,
    ComponentForm::Engine,
    ComponentForm::HeatSink,
    ComponentForm::Weapon,
    ComponentForm::WeaponEnergy,
    ComponentForm::WeaponBallistic,
    ComponentForm::WeaponMissile,
    ComponentForm::Ammo,
    ComponentForm::JumpJet,
    ComponentForm::Case,
    ComponentForm::LifeSupport,
    ComponentForm::Gyroscope,
    ComponentForm::PowerAmplifier,
    ComponentForm::Ecm,
    ComponentForm::Probe,
    ComponentForm::Jammer,
    ComponentForm::Bulk,
];

/// The scratch array the C++ fills with the recycle time of rejected rows (`int Ammo[512]`).
const AMMO_TABLE_LEN: usize = 512;

/// `LogisticsComponent::WEAPON_RANGE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WeaponRangeType {
    /// `SHORT`.
    Short = 0,
    /// `MEDIUM`.
    Medium = 1,
    /// `LONG`.
    Long = 2,
    /// `NO_RANGE`: not a weapon.
    None = 3,
}

/// One entry of the logistics component catalogue (`LogisticsComponent`).
#[derive(Debug, Clone, PartialEq)]
pub struct LogisticsComponent {
    /// `ID`, column 0 (the index into the CSV). Looked up with the low 8 bits only.
    pub id: i32,
    /// `Type`, column 1, matched ignoring case.
    pub form: ComponentForm,
    /// Column 4.
    pub recycle_time: f32,
    /// Column 5, kept as a float (`LogisticsComponent::heat`).
    pub heat: f32,
    /// Column 6, tons.
    pub weight: f32,
    /// Column 7. For heat sinks this is the heat they remove.
    pub damage: f32,
    /// Column 9, C-Bills.
    pub cost: i32,
    /// Column 10 for weapons (`"long"`, `"medium"`, anything else is short).
    pub range_type: WeaponRangeType,
    /// Column 11 (`bHead`). Never read after parsing.
    pub head: bool,
    /// Column 12 (`bTorso`). Never read after parsing.
    pub torso: bool,
    /// Column 17 (`bLegs`). Never read after parsing.
    pub legs: bool,
    /// Column 22 after the ammo fix-up: rounds carried, or `0`.
    pub ammo: i32,
    /// Column 23: icon file. Rows without one are not in the catalogue.
    pub icon_file: String,
    /// Column 24: picture file, when present.
    pub picture_file: Option<String>,
    /// Column 25: string-table id of the name.
    pub string_id: i32,
    /// Column 26: string-table id of the help text.
    pub help_string_id: i32,
    /// Column 27: icon width in lab cells (`iconX`).
    pub width: i32,
    /// Column 28: icon height in lab cells (`iconY`).
    pub height: i32,
}

impl LogisticsComponent {
    /// `LogisticsComponent::isWeapon`.
    pub fn is_weapon(&self) -> bool {
        self.form.is_weapon()
    }

    /// Whether this is a heat sink.
    pub fn is_heat_sink(&self) -> bool {
        self.form == ComponentForm::HeatSink
    }
}

/// Splits like `LogisticsComponent::extractString`: up to `,`, line end or end of text. Returns
/// the field and moves `rest` past the terminator. `None` means the 512-byte scan limit was
/// reached (the C++ returns 0 and leaves the cursor in place).
fn extract_string<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while i < 512 {
        match bytes.get(i) {
            None | Some(b'\n') | Some(b',') => break,
            _ => i += 1,
        }
    }
    if i == 512 {
        return None;
    }
    let field = &rest[..i];
    *rest = rest.get(i + 1..).unwrap_or("");
    Some(field)
}

/// `extractInt`: `-1` for an empty field, `atoi` otherwise.
fn extract_int(rest: &mut &str) -> i32 {
    match extract_string(rest) {
        Some(f) if !f.is_empty() => atoi(f).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        _ => -1,
    }
}

/// `extractFloat`: `-1.0` for an empty field, `atof` otherwise.
fn extract_float(rest: &mut &str) -> f32 {
    match extract_string(rest) {
        Some(f) if !f.is_empty() => atof(f) as f32,
        _ => -1.0,
    }
}

/// What parsing a `compbas.csv` row produced.
#[derive(Debug, Clone, PartialEq)]
pub enum ParsedRow {
    /// A catalogue entry.
    Component(LogisticsComponent),
    /// `init` returned -1. The row's recycle time is what the C++ stores in the ammo table
    /// (`Ammo[counter] = (long)tmp.getRecycleTime()`); it is `0` for rows rejected before the
    /// recycle column is read.
    Rejected { recycle_time: f32 },
}

/// `LogisticsComponent::init(dataLine)`.
///
/// The ammo column is left as the raw table index; [`LogisticsCatalog::from_compbas_bytes`]
/// resolves it afterwards, as `initComponents` does.
pub fn parse_row(line: &str) -> ParsedRow {
    let mut rest = line;

    let id = extract_int(&mut rest);

    // The type, matched ignoring case against ComponentFormString.
    let type_text = extract_string(&mut rest).unwrap_or("");
    let Some(form) = COMPONENT_FORMS
        .iter()
        .copied()
        .find(|f| f.text().eq_ignore_ascii_case(type_text))
    else {
        return ParsedRow::Rejected { recycle_time: 0.0 };
    };

    let _name = extract_string(&mut rest);
    let _crit_hits = extract_string(&mut rest);

    let recycle_time = extract_float(&mut rest);
    let heat = extract_float(&mut rest);
    let weight = extract_float(&mut rest);
    let damage = extract_float(&mut rest);
    let _battle_rating = extract_string(&mut rest);
    let cost = extract_int(&mut rest);

    let range_text = extract_string(&mut rest).unwrap_or("");
    let range_type = if !form.is_weapon() {
        WeaponRangeType::None
    } else if range_text == "long" {
        WeaponRangeType::Long
    } else if range_text == "medium" {
        WeaponRangeType::Medium
    } else {
        WeaponRangeType::Short
    };

    let yes = |s: Option<&str>| s.is_some_and(|s| s.eq_ignore_ascii_case("Yes"));
    let head = yes(extract_string(&mut rest));
    let torso = yes(extract_string(&mut rest));
    for _ in 0..4 {
        let _ = extract_string(&mut rest);
    }
    let legs = yes(extract_string(&mut rest));
    for _ in 0..4 {
        let _ = extract_string(&mut rest);
    }

    let ammo = extract_int(&mut rest);

    let icon = extract_string(&mut rest).unwrap_or("");
    if icon.is_empty() || icon.starts_with('0') {
        return ParsedRow::Rejected { recycle_time };
    }
    let icon_file = icon.to_string();
    let picture = extract_string(&mut rest).unwrap_or("");
    let picture_file = (!picture.is_empty()).then(|| picture.to_string());

    let string_id = extract_int(&mut rest);
    let help_string_id = extract_int(&mut rest);
    let width = extract_int(&mut rest);
    let height = extract_int(&mut rest);

    // `init` returns `ID`; the loader treats -1 as a failure.
    if id == -1 {
        return ParsedRow::Rejected { recycle_time };
    }

    ParsedRow::Component(LogisticsComponent {
        id,
        form,
        recycle_time,
        heat,
        weight,
        damage,
        cost,
        range_type,
        head,
        torso,
        legs,
        ammo,
        icon_file,
        picture_file,
        string_id,
        help_string_id,
        width,
        height,
    })
}

/// The components the lab and the purchase list know about (`LogisticsData::components`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LogisticsCatalog {
    components: Vec<LogisticsComponent>,
}

impl LogisticsCatalog {
    /// Builds a catalogue from already parsed components (tests, tools).
    pub fn from_components(components: Vec<LogisticsComponent>) -> Self {
        Self { components }
    }

    /// `LogisticsData::initComponents`: one header line, then rows until the first empty line.
    /// Rows `init` rejects are dropped; their recycle time feeds the ammo table, and each kept
    /// component with a non-zero ammo column takes `Ammo[column]`.
    pub fn from_compbas_bytes(data: &[u8]) -> Self {
        let mut ammo_table = [0i32; AMMO_TABLE_LEN];
        let mut components = Vec::new();

        let mut lines = split_lines(data).into_iter();
        let _header = lines.next();
        for (counter, raw) in lines.enumerate() {
            // `result < 2` in the C++: an empty line ends the table.
            if raw.is_empty() || raw.len() >= 1024 {
                break;
            }
            let text = String::from_utf8_lossy(raw);
            match parse_row(&text) {
                ParsedRow::Component(c) => components.push(c),
                ParsedRow::Rejected { recycle_time } => {
                    if let Some(slot) = ammo_table.get_mut(counter) {
                        *slot = recycle_time as i32;
                    }
                }
            }
        }

        for c in &mut components {
            if c.ammo != 0 {
                c.ammo = usize::try_from(c.ammo)
                    .ok()
                    .and_then(|i| ammo_table.get(i).copied())
                    .unwrap_or(0);
            }
        }
        Self { components }
    }

    /// Reads `data/objects/compbas.csv` through the VFS.
    pub fn load(vfs: &mut Vfs) -> Result<Self> {
        let bytes = vfs
            .read(COMPBAS_FILE)?
            .ok_or_else(|| SimDataError::MissingFile {
                name: COMPBAS_FILE.to_string(),
            })?;
        Ok(Self::from_compbas_bytes(&bytes))
    }

    /// Number of components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// `true` when there are none.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// Every component in file order.
    pub fn iter(&self) -> impl Iterator<Item = &LogisticsComponent> + '_ {
        self.components.iter()
    }

    /// The component at catalogue index `index`.
    pub fn get(&self, index: usize) -> Option<&LogisticsComponent> {
        self.components.get(index)
    }

    /// `LogisticsData::getComponent`: the first component whose low 8 id bits match, as a
    /// catalogue index.
    pub fn find(&self, component_id: i32) -> Option<usize> {
        self.components
            .iter()
            .position(|c| (c.id & 0xff) == (component_id & 0xff))
    }

    /// Like [`find`](Self::find) but returns the component.
    pub fn by_id(&self, component_id: i32) -> Option<&LogisticsComponent> {
        self.find(component_id).and_then(|i| self.get(i))
    }
}

/// Convenience for tests: a component with the given form, `(weight, heat, damage)`, cost and
/// `(width, height)`.
#[cfg(test)]
pub(crate) fn test_component(
    id: i32,
    form: ComponentForm,
    (weight, heat, damage): (f32, f32, f32),
    cost: i32,
    (width, height): (i32, i32),
) -> LogisticsComponent {
    LogisticsComponent {
        id,
        form,
        recycle_time: 1.0,
        heat,
        weight,
        damage,
        cost,
        range_type: if form.is_weapon() {
            WeaponRangeType::Short
        } else {
            WeaponRangeType::None
        },
        head: false,
        torso: false,
        legs: false,
        ammo: 0,
        icon_file: "icon.tga".into(),
        picture_file: None,
        string_id: 0,
        help_string_id: 0,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // id,type,name,crit,recycle,heat,weight,damage,BR,cost,range,head,torso,lt,rt,la,ra,legs,rl,x,x,x,x? ...
    fn row(id: i32, ty: &str, icon: &str, extra_ammo: i32) -> String {
        format!(
            "{id},{ty},Name,1,2.5,3,4.5,5,6,7000,long,Yes,No,No,No,No,No,Yes,No,No,No,No,{extra_ammo},{icon},pic.tga,100,200,2,3"
        )
    }

    #[test]
    fn row_columns_are_read_by_position() {
        let ParsedRow::Component(c) = parse_row(&row(12, "energyweapon", "ic.tga", 0)) else {
            panic!("rejected")
        };
        assert_eq!(c.id, 12);
        assert_eq!(c.form, ComponentForm::WeaponEnergy);
        assert_eq!(c.recycle_time, 2.5);
        assert_eq!(c.heat, 3.0);
        assert_eq!(c.weight, 4.5);
        assert_eq!(c.damage, 5.0);
        assert_eq!(c.cost, 7000);
        assert_eq!(c.range_type, WeaponRangeType::Long);
        assert!(c.head && !c.torso && c.legs);
        assert_eq!(c.icon_file, "ic.tga");
        assert_eq!(c.picture_file.as_deref(), Some("pic.tga"));
        assert_eq!((c.string_id, c.help_string_id), (100, 200));
        assert_eq!((c.width, c.height), (2, 3));
    }

    #[test]
    fn non_weapons_have_no_range_and_icons_starting_with_zero_are_rejected() {
        let ParsedRow::Component(c) = parse_row(&row(7, "HeatSink", "ic.tga", 0)) else {
            panic!()
        };
        assert_eq!(c.range_type, WeaponRangeType::None);
        assert!(matches!(
            parse_row(&row(7, "HeatSink", "0", 0)),
            ParsedRow::Rejected { recycle_time } if recycle_time == 2.5
        ));
        // Unknown type: rejected before the recycle column is read.
        assert!(matches!(
            parse_row(&row(7, "Optics", "ic.tga", 0)),
            ParsedRow::Rejected { recycle_time } if recycle_time == 0.0
        ));
    }

    #[test]
    fn ammo_fixup_reads_the_recycle_time_of_the_rejected_row() {
        let ammo_row =
            "5,Ammo,AC ammo,1,20,0,1,0,0,100,0,No,No,No,No,No,No,No,No,No,No,No,0,0,,0,0,1,1";
        let mut text = String::from("header\n");
        text.push_str(&row(1, "EnergyWeapon", "ic.tga", 0)); // data row 0
        text.push('\n');
        text.push_str(ammo_row); // data row 1: rejected, recycle 20
        text.push('\n');
        text.push_str(&row(3, "BallisticWeapon", "ic.tga", 1)); // ammo column 1 -> Ammo[1] = 20
        text.push('\n');
        let cat = LogisticsCatalog::from_compbas_bytes(text.as_bytes());
        assert_eq!(cat.by_id(3).unwrap().ammo, 20);
        assert_eq!(cat.by_id(1).unwrap().ammo, 0);
    }

    #[test]
    fn find_uses_the_low_eight_bits() {
        let cat = LogisticsCatalog::from_components(vec![test_component(
            7,
            ComponentForm::HeatSink,
            (1.0, 0.0, 1.0),
            100,
            (1, 1),
        )]);
        assert_eq!(cat.find(7), Some(0));
        assert_eq!(cat.find(0x107), Some(0));
        assert_eq!(cat.find(8), None);
    }
}
