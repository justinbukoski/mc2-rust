//! Cross-reference check over the shipped data (skipped unless `MC2_DATA` is set).
//!
//! Every name that one file uses to point at another must resolve:
//! - object packets: the only packet allowed to fail is 55 (PUMA base, no `[MechProfile]`);
//! - every mech packet's `ProfileName` loads as a chassis CSV;
//! - the component master list loads;
//! - every mission `.fit` loads, and every `Part` object number resolves to a loadable
//!   object type, every `CSVFile` loads as a chassis, every `ObjectProfile` loads as a mech
//!   or vehicle profile, and every warrior `Profile` loads;
//! - the campaign and tutorial load, and each purchase list they name loads.

use std::collections::BTreeSet;
use std::path::PathBuf;

use mc2_formats::fst::FastFile;
use mc2_formats::vfs::Vfs;
use mc2_sim_data::campaign::{Campaign, PurchaseList};
use mc2_sim_data::component::MasterComponentList;
use mc2_sim_data::mech::{load_chassis, load_mech_profile, load_vehicle_profile};
use mc2_sim_data::mission::Mission;
use mc2_sim_data::objtype::{load_object_table, ObjectTypeData, OBJECT_PAK_PATH};
use mc2_sim_data::pilot::WarriorProfile;

/// The one packet the C++ cannot load either (no `[MechProfile]`).
const EXPECTED_PACKET_FAILURES: [u32; 1] = [55];
const MISSION_PREFIX: &str = "data/missions/";

fn data_root() -> Option<PathBuf> {
    std::env::var_os("MC2_DATA").map(PathBuf::from)
}

/// Mission files directly under `data/missions/` in `mission.fst`.
fn mission_paths(root: &std::path::Path) -> Vec<String> {
    let fst = FastFile::open(root.join("mission.fst")).expect("open mission.fst");
    fst.entries()
        .iter()
        .map(|e| e.name.clone())
        .filter(|n| {
            n.strip_prefix(MISSION_PREFIX)
                .is_some_and(|rest| !rest.contains('/') && rest.ends_with(".fit"))
        })
        .collect()
}

#[test]
fn shipped_references_resolve() {
    let Some(root) = data_root() else {
        eprintln!("skipping: MC2_DATA is not set");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(&root).expect("system.cfg mounts the archives");
    let mut problems: Vec<String> = Vec::new();

    // Object packets: only packet 55 may fail, and every mech packet names a chassis.
    let table = load_object_table(root.join(OBJECT_PAK_PATH)).expect("object2.pak opens");
    let failures = &table.failures;
    let failed: BTreeSet<u32> = failures.iter().map(|f| f.index).collect();
    let expected: BTreeSet<u32> = EXPECTED_PACKET_FAILURES.into_iter().collect();
    assert_eq!(failed, expected, "unexpected object packet failures");

    let mut mech_names: BTreeSet<String> = BTreeSet::new();
    for obj in table.types.values() {
        if let ObjectTypeData::BattleMech(m) = &obj.data {
            mech_names.insert(m.profile_name.clone());
        }
    }
    let mut chassis_ok = 0usize;
    for name in &mech_names {
        match load_chassis(&mut vfs, name) {
            Ok(_) => chassis_ok += 1,
            Err(e) => problems.push(format!("chassis {name}: {e}")),
        }
    }
    eprintln!(
        "object packets: {} loaded, {} expected failure(s); mech ProfileNames {} -> chassis {} loaded",
        table.len(),
        failures.len(),
        mech_names.len(),
        chassis_ok
    );

    // Component master list.
    let master = MasterComponentList::load(&mut vfs).expect("compbas.csv loads");
    eprintln!("component master list: {} records", master.len());

    // Missions: every mission file, and every reference out of its parts and warriors.
    let mut mission_count = 0usize;
    let mut skipped_non_mission = 0usize;
    let mut part_count = 0usize;
    let mut object_refs: BTreeSet<u32> = BTreeSet::new();
    let mut csv_refs: BTreeSet<String> = BTreeSet::new();
    let mut unread_csv_refs: BTreeSet<String> = BTreeSet::new();
    let mut profile_refs: BTreeSet<String> = BTreeSet::new();
    let mut warrior_refs: BTreeSet<String> = BTreeSet::new();
    let mut vehicle_profiles = 0usize;
    let mut mech_profiles = 0usize;

    for path in mission_paths(&root) {
        let mut fit = match vfs.read_fit(&path) {
            Ok(Some(fit)) => fit,
            Ok(None) => {
                problems.push(format!("{path}: listed in mission.fst but unreadable"));
                continue;
            }
            Err(e) => {
                problems.push(format!("{path}: {e}"));
                continue;
            }
        };
        if !Mission::is_mission_file(&fit) {
            skipped_non_mission += 1;
            continue;
        }
        let stem = path
            .strip_prefix(MISSION_PREFIX)
            .and_then(|s| s.strip_suffix(".fit"))
            .unwrap_or(&path)
            .to_string();
        let mission = match Mission::load(&mut fit, &stem) {
            Ok(m) => m,
            Err(e) => {
                problems.push(format!("mission {stem}: {e}"));
                continue;
            }
        };
        mission_count += 1;

        for part in &mission.parts {
            part_count += 1;
            object_refs.insert(part.object_number);
            match table.get(part.object_number) {
                Some(obj) if obj.game_loadable() => {}
                Some(_) => problems.push(format!(
                    "mission {stem} part {}: object {} is not game-loadable",
                    part.index, part.object_number
                )),
                None => problems.push(format!(
                    "mission {stem} part {}: object {} is not in object2.pak",
                    part.index, part.object_number
                )),
            }
            // Only mech parts load their CSVFile (BattleMech::init); a vehicle part's
            // CSVFile names no chassis file and is not read by the C++ loaders.
            let is_mech = matches!(
                table.get(part.object_number).map(|o| &o.data),
                Some(ObjectTypeData::BattleMech(_))
            );
            if let Some(csv) = &part.csv_file {
                if is_mech {
                    csv_refs.insert(csv.clone());
                } else {
                    unread_csv_refs.insert(csv.clone());
                }
            }
            if !part.profile_name.is_empty() {
                profile_refs.insert(part.profile_name.clone());
            }
        }
        for warrior in &mission.warriors {
            warrior_refs.insert(warrior.profile.clone());
        }
    }

    for name in &csv_refs {
        if let Err(e) = load_chassis(&mut vfs, name) {
            problems.push(format!("part CSVFile {name}: {e}"));
        }
    }
    for name in &profile_refs {
        match load_mech_profile(&mut vfs, name) {
            Ok(_) => mech_profiles += 1,
            Err(mech_err) => match load_vehicle_profile(&mut vfs, name) {
                Ok(_) => vehicle_profiles += 1,
                Err(vehicle_err) => problems.push(format!(
                    "part ObjectProfile {name}: not a mech ({mech_err}) nor a vehicle ({vehicle_err})"
                )),
            },
        }
    }
    let mut warriors_ok = 0usize;
    for name in &warrior_refs {
        match WarriorProfile::load(&mut vfs, name) {
            Ok(_) => warriors_ok += 1,
            Err(e) => problems.push(format!("warrior profile {name}: {e}")),
        }
    }
    eprintln!(
        "missions: {mission_count} loaded, {skipped_non_mission} non-mission .fit skipped; \
         parts {part_count}, distinct object types {}, mech chassis CSVs {} \
         (vehicle CSVFile names not read by the loaders: {}), profiles mech {} / vehicle {}, \
         warrior profiles {warriors_ok}/{}",
        object_refs.len(),
        csv_refs.len(),
        unread_csv_refs.len(),
        mech_profiles,
        vehicle_profiles,
        warrior_refs.len()
    );

    // Campaign and tutorial, and the purchase lists they name.
    let mut purchase_files: BTreeSet<String> = BTreeSet::new();
    for name in ["campaign", "tutorial"] {
        match Campaign::load(&mut vfs, name) {
            Ok(campaign) => {
                let missions: usize = campaign.groups.iter().map(|g| g.missions.len()).sum();
                eprintln!(
                    "{name}: {} groups, {missions} missions, {} mission file(s) missing",
                    campaign.groups.len(),
                    campaign.missing_mission_files.len()
                );
                for group in &campaign.groups {
                    for mission in &group.missions {
                        if !mission.purchase_file.is_empty() {
                            purchase_files.insert(mission.purchase_file.clone());
                        }
                    }
                }
                for file in &campaign.missing_mission_files {
                    problems.push(format!("campaign {name}: missing mission file {file}"));
                }
            }
            Err(e) => problems.push(format!("campaign {name}: {e}")),
        }
    }
    let mut purchase_ok = 0usize;
    for file in &purchase_files {
        match PurchaseList::load(&mut vfs, file) {
            Ok(_) => purchase_ok += 1,
            Err(e) => problems.push(format!("purchase list {file}: {e}")),
        }
    }
    eprintln!(
        "purchase lists named by campaign and tutorial: {purchase_ok}/{}",
        purchase_files.len()
    );

    assert!(
        problems.is_empty(),
        "{} unresolved reference(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}
