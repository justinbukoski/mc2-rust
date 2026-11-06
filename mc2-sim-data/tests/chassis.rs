//! Integration tests over the shipped mech and vehicle data.
//!
//! Set `MC2_DATA` to the data root (the directory with `system.cfg` and the `.fst`
//! files). Without it every test returns early. Run with `-- --nocapture` for counts.

use mc2_formats::fst::FastFile;
use mc2_formats::ini::FitIni;
use mc2_formats::pak::PakFile;
use mc2_formats::vfs::{normalize_name, Vfs};
use mc2_sim_data::mech::{
    load_chassis, load_mech_profile, load_variant_list, load_vehicle_profile, OBJECT_PATH,
    PROFILE_PATH,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn data_root() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(v) => Some(PathBuf::from(v)),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped mech data test");
            None
        }
    }
}

fn vfs(root: &Path) -> Vfs {
    Vfs::from_system_cfg(root).expect("system.cfg")
}

/// Chassis names from `[MechProfile] ProfileName` in every packet of `object2.pak`.
fn chassis_names_from_packets(vfs: &mut Vfs) -> (BTreeSet<String>, usize) {
    let object_pak = format!("{OBJECT_PATH}object2.pak");
    let bytes = vfs
        .read(&object_pak)
        .expect("read object2.pak")
        .expect("object2.pak present");
    let pak = PakFile::from_bytes(bytes).expect("parse object2.pak");
    let mut names = BTreeSet::new();
    let mut mech_packets = 0usize;
    for i in 0..pak.packet_count() {
        let Ok(packet) = pak.read_packet(i) else {
            continue;
        };
        let Ok(mut fit) = FitIni::from_bytes(packet) else {
            continue;
        };
        if fit.seek_block("MechProfile").is_err() {
            continue;
        }
        mech_packets += 1;
        let profile_name = fit
            .read_string("ProfileName", 511)
            .expect("ProfileName in [MechProfile]");
        names.insert(profile_name);
    }
    (names, mech_packets)
}

/// Lower-cased stems of the files under `dir` ending in `ext`, from the loose
/// directory and every mounted archive.
fn stems_under(vfs: &Vfs, root: &Path, dir: &str, ext: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Ok(entries) = std::fs::read_dir(root.join(dir)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if let Some(stem) = name.strip_suffix(ext) {
                out.insert(stem.to_string());
            }
        }
    }
    let prefix = normalize_name(dir);
    for path in vfs.archive_paths() {
        let fst = FastFile::open(path).expect("mounted archive");
        for entry in fst.entries() {
            let name = normalize_name(&entry.name);
            if let Some(rest) = name.strip_prefix(&prefix) {
                if let Some(stem) = rest.strip_suffix(ext) {
                    out.insert(stem.to_string());
                }
            }
        }
    }
    out
}

fn profile_names(vfs: &Vfs, root: &Path) -> BTreeSet<String> {
    stems_under(vfs, root, PROFILE_PATH, ".fit")
}

/// Chassis CSVs in `data/objects/` whose first line is `BaseChassisInfo`.
fn base_chassis_csvs(root: &Path, vfs: &mut Vfs) -> BTreeSet<String> {
    stems_under(vfs, root, OBJECT_PATH, ".csv")
        .into_iter()
        .filter(|stem| {
            vfs.read(&format!("{OBJECT_PATH}{stem}.csv"))
                .ok()
                .flatten()
                .is_some_and(|bytes| bytes.starts_with(b"BaseChassisInfo"))
        })
        .collect()
}

#[test]
fn every_base_chassis_csv_loads() {
    let Some(root) = data_root() else { return };
    let mut vfs = vfs(&root);
    let names = base_chassis_csvs(&root, &mut vfs);
    let mut errors = Vec::new();
    for name in &names {
        if let Err(e) = load_chassis(&mut vfs, name) {
            errors.push(format!("{name}: {e}"));
        }
    }
    eprintln!("BaseChassisInfo CSVs: {} files", names.len());
    let (packet_names, _) = chassis_names_from_packets(&mut vfs);
    for name in names
        .iter()
        .filter(|n| !packet_names.iter().any(|p| p.eq_ignore_ascii_case(n)))
    {
        eprintln!("  BaseChassisInfo CSV not named by any MechProfile packet: {name}");
    }
    assert!(
        errors.is_empty(),
        "chassis CSV errors:\n{}",
        errors.join("\n")
    );
}

#[test]
fn every_packet_chassis_loads() {
    let Some(root) = data_root() else { return };
    let mut vfs = vfs(&root);
    let (names, mech_packets) = chassis_names_from_packets(&mut vfs);
    eprintln!(
        "object2.pak: {mech_packets} MechProfile packets, {} chassis names",
        names.len()
    );

    let mut errors = Vec::new();
    let mut variants = 0usize;
    let mut items = 0usize;
    let mut installed = 0usize;
    for name in &names {
        match load_chassis(&mut vfs, name) {
            Ok(c) => {
                variants += c.variants.len();
                for v in &c.variants {
                    items += v.items.len();
                    installed += v.installed_items().count();
                }
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    eprintln!(
        "chassis: {} loaded, {} variants, {} item rows, {} installed items",
        names.len() - errors.len(),
        variants,
        items,
        installed
    );
    assert!(errors.is_empty(), "chassis errors:\n{}", errors.join("\n"));
}

#[test]
fn every_profile_fit_loads() {
    let Some(root) = data_root() else { return };
    let mut vfs = vfs(&root);
    let names = profile_names(&vfs, &root);
    let mut mech = 0usize;
    let mut vehicle = 0usize;
    let mut other = Vec::new();
    let mut errors = Vec::new();
    for name in &names {
        let path = format!("{PROFILE_PATH}{name}.fit");
        let Some(mut fit) = vfs.read_fit(&path).expect("read profile") else {
            errors.push(format!("{name}: not found"));
            continue;
        };
        if fit.seek_block("Header").is_err() {
            errors.push(format!("{name}: no [Header]"));
            continue;
        }
        let file_type = fit.read_string("FileType", 127).unwrap_or_default();
        match file_type.as_str() {
            "MechProfile" => match load_mech_profile(&mut vfs, name) {
                Ok(_) => mech += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            },
            "GroundVehicleProfile" => match load_vehicle_profile(&mut vfs, name) {
                Ok(_) => vehicle += 1,
                Err(e) => errors.push(format!("{name}: {e}")),
            },
            other_type => other.push(format!("{name}: {other_type}")),
        }
    }
    eprintln!(
        "profiles: {} files, {mech} mech, {vehicle} vehicle, {} other",
        names.len(),
        other.len()
    );
    for line in &other {
        eprintln!("  not a mech or vehicle profile: {line}");
    }
    assert!(errors.is_empty(), "profile errors:\n{}", errors.join("\n"));
}

#[test]
fn variant_list_loads() {
    let Some(root) = data_root() else { return };
    let mut vfs = vfs(&root);
    let names = load_variant_list(&mut vfs).expect("variants.csv");
    eprintln!("variants.csv: {} names", names.len());
    assert!(!names.is_empty());
}
