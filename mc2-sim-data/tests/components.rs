//! Master component list against the shipped data.
//!
//! Needs `MC2_DATA` set to the built game data root (the folder with `system.cfg`).
//! Without it every test returns early and prints why.

use mc2_formats::csv::CsvFile;
use mc2_formats::fst::FastFile;
use mc2_formats::vfs::Vfs;
use mc2_sim_data::component::{
    base_sensor_range, ComponentForm, ComponentStats, MasterComponentList, WeaponAmmoType,
    WeaponRange, WeaponRangeSpans, COMPBAS_FILE, GAMESYS_FILE, MASTER_LIST_COUNT,
    WEAPON_FLAG_STREAK,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// `Load#` of the "EMPTY!" slot in every chassis file. It has no compbas row.
const EMPTY_LOAD: i64 = 94;
/// `CompBas#` written for the "EMPTY!" slot.
const EMPTY_COMPBAS: i64 = 255;

fn data_vfs() -> Option<Vfs> {
    let Some(root) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA is not set; skipping component data tests");
        return None;
    };
    if !root.join("system.cfg").is_file() {
        eprintln!(
            "MC2_DATA={} has no system.cfg; skipping component data tests",
            root.display()
        );
        return None;
    }
    Some(Vfs::from_system_cfg(&root).expect("open the data root"))
}

/// One `Item` row of a chassis CSV: `(file, Load#, CompBas#, name)`.
#[derive(Debug)]
struct ItemRow {
    file: String,
    load: i64,
    compbas: i64,
    name: String,
}

fn is_chassis(text: &str) -> bool {
    text.lines().any(|line| {
        let first = line.split(',').next().unwrap_or("");
        first.starts_with("Item") && first.ends_with("MasterId")
    })
}

/// Chassis CSVs, loose files first, then archive entries (first name wins).
fn chassis_csvs(vfs: &Vfs) -> BTreeMap<String, String> {
    let mut found: BTreeMap<String, String> = BTreeMap::new();
    if let Ok(entries) = std::fs::read_dir(vfs.root().join("data/objects")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if !name.ends_with(".csv") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path()) {
                found
                    .entry(name)
                    .or_insert_with(|| String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }
    for archive in vfs.archive_paths() {
        let Ok(mut fst) = FastFile::open(archive) else {
            continue;
        };
        let wanted: Vec<(usize, String)> = fst
            .entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let lower = entry.name.to_ascii_lowercase();
                let base = lower.rsplit(['/', '\\']).next().unwrap_or("").to_string();
                (base.ends_with(".csv") && !found.contains_key(&base)).then_some((index, base))
            })
            .collect();
        for (index, base) in wanted {
            if let Ok(bytes) = fst.read_index(index) {
                let text = String::from_utf8_lossy(&bytes).into_owned();
                if is_chassis(&text) {
                    found.insert(base, text);
                }
            }
        }
    }
    found.retain(|_, text| is_chassis(text));
    found
}

fn item_rows(chassis: &BTreeMap<String, String>) -> Vec<ItemRow> {
    let mut rows = Vec::new();
    for (file, text) in chassis {
        for line in text.lines() {
            let cells: Vec<&str> = line.split(',').collect();
            let label = cells.first().copied().unwrap_or("");
            if !(label.starts_with("Item") && label.ends_with("MasterId")) {
                continue;
            }
            let number = |index: usize| cells.get(index).and_then(|c| c.trim().parse::<i64>().ok());
            let (Some(load), Some(compbas)) = (number(1), number(4)) else {
                panic!("{file}: {label} has no Load# or CompBas# number");
            };
            rows.push(ItemRow {
                file: file.clone(),
                load,
                compbas,
                name: cells.get(5).copied().unwrap_or("").to_string(),
            });
        }
    }
    rows
}

#[test]
fn master_list_loads_with_no_errors() {
    let Some(mut vfs) = data_vfs() else {
        return;
    };
    let list = MasterComponentList::load(&mut vfs).expect("master list loads");
    assert_eq!(list.len(), MASTER_LIST_COUNT);

    let mut by_form: BTreeMap<String, usize> = BTreeMap::new();
    let mut weapons = 0usize;
    let mut ammo_types: BTreeMap<String, usize> = BTreeMap::new();
    let mut ranges: BTreeMap<String, usize> = BTreeMap::new();
    let mut sensors = 0usize;
    for record in list.iter() {
        let form = record.form.map_or("(rejected)", ComponentForm::text);
        *by_form
            .entry(format!("{form} [{}]", record.form_text))
            .or_default() += 1;
        if let Some(weapon) = record.weapon() {
            weapons += 1;
            let key = format!("{:?}", weapon.ammo_type);
            *ammo_types.entry(key).or_default() += 1;
            let band = weapon
                .range
                .map_or("none".to_string(), |r| format!("{r:?}"));
            *ranges.entry(band).or_default() += 1;
        }
        if matches!(record.stats, ComponentStats::Sensor { .. }) {
            sensors += 1;
        }
    }
    let ammo = list
        .iter()
        .filter(|r| matches!(r.stats, ComponentStats::Ammo { .. }))
        .count();
    eprintln!("master list records: {}", list.len());
    for (form, count) in &by_form {
        eprintln!("  form {form}: {count}");
    }
    eprintln!("weapons with stats: {weapons}, ammo: {ammo}, sensors: {sensors}");
    eprintln!("weapon ammo types: {ammo_types:?}");
    eprintln!("weapon ranges: {ranges:?}");
    eprintln!(
        "logistics purchase list entries: {}",
        list.logistics_listed().count()
    );

    let special = list.special();
    eprintln!("special ids after load: {special:?}");

    let jump: Vec<i32> = list
        .iter()
        .filter_map(|r| match r.stats {
            ComponentStats::JumpJet { range_mod } => Some(range_mod),
            _ => None,
        })
        .collect();
    eprintln!("jump jet range modifiers: {jump:?}");
    let max_effect = list
        .iter()
        .filter_map(|r| r.weapon().map(|w| w.special_effect))
        .max();
    eprintln!("largest weapon special effect id: {max_effect:?}");
    let mut dangling = Vec::new();
    for record in list.iter() {
        if let Some(weapon) = record.weapon() {
            let ammo = i32::from(weapon.ammo_master_id);
            let is_ammo = list
                .get(ammo)
                .is_some_and(|a| matches!(a.stats, ComponentStats::Ammo { .. }));
            if weapon.ammo_master_id != 0 && !is_ammo {
                dangling.push((record.master_id, ammo));
            }
        }
    }
    eprintln!("weapons whose ammo id is not an Ammo record: {dangling:?}");
    let csv = CsvFile::from_bytes(
        vfs.read(COMPBAS_FILE)
            .expect("read compbas")
            .expect("compbas present"),
    )
    .expect("parse compbas");
    // Data rows run from file line 2 to line MASTER_LIST_COUNT + 1. CsvFile::rows()
    // stops one short of that, so cell(row, col) cannot reach the last record.
    // The loader reads lines in order and does not depend on it.
    eprintln!(
        "CsvFile::rows() for compbas: {} (last data line is {}, id {})",
        csv.rows(),
        MASTER_LIST_COUNT + 1,
        MASTER_LIST_COUNT - 1
    );
}

#[test]
fn known_records_match_the_shipped_file() {
    let Some(mut vfs) = data_vfs() else {
        return;
    };
    let list = MasterComponentList::load(&mut vfs).expect("master list loads");

    let engine = list.get(1).expect("id 1");
    assert_eq!(engine.name, "Fusion Engine");
    assert_eq!(engine.form, Some(ComponentForm::Engine));
    assert_eq!(engine.critical_space_at(1), 6);

    let large = list.get(140).expect("id 140");
    assert_eq!(large.name, "Large Laser");
    let weapon = large.weapon().expect("large laser has weapon stats");
    assert_eq!(weapon.damage, 4.0);
    assert_eq!(weapon.recycle_time, 5.0);
    assert_eq!(weapon.range, Some(WeaponRange::Medium));
    assert_eq!(weapon.ammo_type, WeaponAmmoType::Unlimited);
    assert_eq!(large.resource_points, 3800);

    let streak = list.get(125).expect("id 125");
    let weapon = streak.weapon().expect("streak pack has weapon stats");
    assert_eq!(weapon.ammo_type, WeaponAmmoType::St);
    assert_eq!(weapon.flags, WEAPON_FLAG_STREAK);
    assert_eq!(weapon.ammo_master_id, 225);
    assert_eq!(list.get(225).expect("id 225").name, "Streak SRM2 Pack Ammo");

    let mut gamesys = vfs
        .read_fit(GAMESYS_FILE)
        .expect("read gamesys.fit")
        .expect("gamesys.fit present");
    let base = base_sensor_range(&mut gamesys).expect("BaseSensorRange");
    assert_eq!(base, 1.0);
    let extended = list.get(14).expect("id 14");
    assert_eq!(
        extended.stats,
        ComponentStats::Sensor {
            range: 400.0 * base
        }
    );

    let spans = WeaponRangeSpans::from_gamesys(&mut gamesys).expect("WeaponRanges");
    assert_eq!(spans.span(WeaponRange::Short), [0.0, 60.0]);
    assert_eq!(spans.span(WeaponRange::Medium), [30.0, 120.0]);
    assert_eq!(spans.span(WeaponRange::Long), [60.0, 180.0]);
    assert_eq!(spans.optimal_points, [20.0, 45.0, 100.0, 100.0, 160.0]);
}

#[test]
fn chassis_item_rows_resolve_to_the_master_list() {
    let Some(mut vfs) = data_vfs() else {
        return;
    };
    let list = MasterComponentList::load(&mut vfs).expect("master list loads");
    let chassis = chassis_csvs(&vfs);
    assert!(!chassis.is_empty(), "no chassis CSVs with Item rows found");
    let rows = item_rows(&chassis);
    assert!(!rows.is_empty());

    let mut errors: Vec<String> = Vec::new();
    let mut load_to_compbas: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    let mut load_to_name: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    let mut empty_rows = 0usize;
    for row in &rows {
        load_to_compbas
            .entry(row.load)
            .or_default()
            .insert(row.compbas);
        load_to_name
            .entry(row.load)
            .or_default()
            .insert(row.name.clone());
        if row.load == EMPTY_LOAD {
            empty_rows += 1;
            if row.compbas != EMPTY_COMPBAS || row.name != "EMPTY!" {
                errors.push(format!("{}: EMPTY slot is {row:?}", row.file));
            }
            continue;
        }
        let Some(record) = i32::try_from(row.compbas).ok().and_then(|id| list.get(id)) else {
            errors.push(format!(
                "{}: CompBas# {} ({}) is not in the master list",
                row.file, row.compbas, row.name
            ));
            continue;
        };
        if record.form.is_none() {
            errors.push(format!(
                "{}: CompBas# {} is a rejected row ({})",
                row.file, row.compbas, record.form_text
            ));
        }
        if record.name != row.name {
            errors.push(format!(
                "{}: CompBas# {} is {:?} in compbas but {:?} in the chassis",
                row.file, row.compbas, record.name, row.name
            ));
        }
    }
    for (load, ids) in &load_to_compbas {
        if ids.len() != 1 {
            errors.push(format!("Load# {load} maps to several CompBas# {ids:?}"));
        }
    }
    for (load, names) in &load_to_name {
        if names.len() != 1 {
            errors.push(format!("Load# {load} has several names {names:?}"));
        }
    }

    eprintln!("chassis CSVs: {}", chassis.len());
    eprintln!(
        "chassis Item rows: {} (EMPTY slots: {empty_rows})",
        rows.len()
    );
    eprintln!(
        "distinct Load# (chassis MasterComponentID): {}, distinct CompBas#: {}",
        load_to_compbas.len(),
        load_to_compbas
            .values()
            .flatten()
            .collect::<BTreeSet<_>>()
            .len()
    );
    assert!(
        errors.is_empty(),
        "{} chassis reference errors, first ten: {:#?}",
        errors.len(),
        errors.iter().take(10).collect::<Vec<_>>()
    );
}
