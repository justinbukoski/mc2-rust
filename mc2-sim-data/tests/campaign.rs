//! Campaign scripts and purchase lists against the shipped data.
//!
//! Skipped unless `MC2_DATA` names the built game directory (the one with `system.cfg`).

use mc2_formats::vfs::Vfs;
use mc2_sim_data::campaign::{Campaign, PurchaseList};
use mc2_sim_data::pilot::WarriorProfile;

/// Purchase lists in `missions/` (`purchase*.fit`, 31 files), by stem.
const PURCHASE_FILES: &[&str] = &[
    "purchase",
    "purchase01",
    "purchase02",
    "purchase03",
    "purchase04",
    "purchase05",
    "purchase06",
    "purchase07",
    "purchase08",
    "purchase09",
    "purchase10",
    "purchase11",
    "purchase12",
    "purchase13",
    "purchase14",
    "purchase15",
    "purchase16",
    "purchase17",
    "purchase18",
    "purchase19",
    "purchase20",
    "purchase21",
    "purchase22",
    "purchase23",
    "purchase24",
    "purchase_all",
    "purchase_clan",
    "purchase_davion",
    "purchase_liao",
    "purchase_steiner",
    "purchaseall",
];

/// Bonus purchase lists in `missions/` (`purbonus*.fit`), by stem.
const BONUS_FILES: &[&str] = &[
    "purbonus08",
    "purbonus11",
    "purbonus13",
    "purbonus19",
    "purbonus20",
    "purbonus21",
];

fn data_vfs() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA")?;
    Some(Vfs::from_system_cfg(root).expect("MC2_DATA: system.cfg and FastFiles mount"))
}

#[test]
fn campaign_and_tutorial_load_with_all_mission_files_present() {
    let Some(mut vfs) = data_vfs() else {
        eprintln!("MC2_DATA unset: skipping campaign test");
        return;
    };

    let campaign = Campaign::load(&mut vfs, "campaign").expect("campaign.fit");
    assert!(campaign.missing_groups.is_empty());
    assert!(
        campaign.missing_mission_files.is_empty(),
        "missing missions: {:?}",
        campaign.missing_mission_files
    );
    assert_eq!(campaign.group_count, 13);
    assert_eq!(campaign.groups.len(), 13);
    assert_eq!(campaign.cbills, 80000);
    assert_eq!(campaign.name_id, Some(60021));
    assert_eq!(campaign.mission_total(), 24);
    eprintln!(
        "campaign: {} groups, {} missions, starting C-bills {}",
        campaign.groups.len(),
        campaign.mission_total(),
        campaign.cbills
    );
    for g in &campaign.groups {
        let names: Vec<&str> = g.missions.iter().map(|m| m.file_name.as_str()).collect();
        eprintln!(
            "  group {:>2}: need {:>2} of {:>1} {:?} tune {} op {}",
            g.index,
            g.number_to_complete,
            g.missions.len(),
            names,
            g.logistics_tune_id,
            g.operation_file
        );
    }
    let mandatory = campaign.missions().filter(|(_, m)| m.mandatory).count();
    let hidden = campaign.missions().filter(|(_, m)| m.hidden).count();
    eprintln!("mandatory missions: {mandatory}, hidden: {hidden}");

    let rp_known = campaign
        .missions()
        .filter(|(_, m)| {
            m.settings
                .as_ref()
                .is_some_and(|s| s.resource_points.is_some())
        })
        .count();
    let rp_total: i64 = campaign
        .missions()
        .filter_map(|(_, m)| m.settings.as_ref().and_then(|s| s.resource_points))
        .sum();
    eprintln!(
        "missions with a readable ResourcePoints ('l' tag): {rp_known} of {}; sum {rp_total}",
        campaign.mission_total()
    );

    let tutorial = Campaign::load(&mut vfs, "tutorial").expect("tutorial.fit");
    assert!(tutorial.missing_mission_files.is_empty());
    assert_eq!(tutorial.groups.len(), 1);
    assert_eq!(tutorial.mission_total(), 5);
    assert_eq!(tutorial.name_id, Some(60022));
    eprintln!(
        "tutorial: {} group(s), {} missions",
        tutorial.groups.len(),
        tutorial.mission_total()
    );
}

#[test]
fn every_purchase_and_bonus_list_loads() {
    let Some(mut vfs) = data_vfs() else {
        eprintln!("MC2_DATA unset: skipping purchase test");
        return;
    };
    assert_eq!(PURCHASE_FILES.len(), 31);

    let mut components = 0usize;
    let mut mechs = 0usize;
    let mut pilots = 0usize;
    let mut all = 0usize;
    let mut unresolved = Vec::new();
    for name in PURCHASE_FILES.iter().chain(BONUS_FILES.iter()) {
        let list = PurchaseList::load(&mut vfs, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        components += list.components.len();
        mechs += list.mechs.len();
        pilots += list.pilots.len();
        all += usize::from(list.all_components);
        for pilot in &list.pilots {
            let stem = pilot.to_ascii_lowercase();
            if WarriorProfile::load(&mut vfs, &stem).is_err() {
                unresolved.push(format!("{name}:{pilot}"));
            }
        }
    }
    eprintln!(
        "purchase lists: {} files; items: {components} components, {mechs} mechs, {pilots} pilots; AllComponents set in {all}",
        PURCHASE_FILES.len() + BONUS_FILES.len()
    );
    eprintln!(
        "pilot names in purchase lists with no warrior file: {}",
        unresolved.len()
    );
    for u in &unresolved {
        eprintln!("  {u}");
    }
}

#[test]
fn every_campaign_purchase_reference_resolves() {
    let Some(mut vfs) = data_vfs() else {
        eprintln!("MC2_DATA unset: skipping campaign purchase reference test");
        return;
    };
    let campaign = Campaign::load(&mut vfs, "campaign").expect("campaign.fit");
    let tutorial = Campaign::load(&mut vfs, "tutorial").expect("tutorial.fit");
    let mut seen = std::collections::BTreeSet::new();
    for c in [&campaign, &tutorial] {
        for (_, m) in c.missions() {
            if m.purchase_file.is_empty() {
                continue;
            }
            PurchaseList::load(&mut vfs, &m.purchase_file)
                .unwrap_or_else(|e| panic!("mission {}: {e}", m.file_name));
            seen.insert(m.purchase_file.to_ascii_lowercase());
        }
    }
    eprintln!(
        "distinct purchase files referenced by campaign.fit and tutorial.fit: {}",
        seen.len()
    );
}
