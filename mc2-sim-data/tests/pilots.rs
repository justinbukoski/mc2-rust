//! Warrior and pilot roster loading against the shipped data.
//!
//! Skipped unless `MC2_DATA` names the built game directory (the one with `system.cfg`).

use mc2_formats::vfs::Vfs;
use mc2_sim_data::pilot::{load_roster, PilotRecord, SpecialtySkill, WarriorProfile, WarriorRank};

/// Every file in `missions/warriors/` (76 `.fit` files), by stem.
const WARRIORS: &[&str] = &[
    "pmp_bubba",
    "pmp_chopper",
    "pmp_claymore",
    "pmp_cobra",
    "pmp_creep",
    "pmp_dagger",
    "pmp_flash",
    "pmp_ghost",
    "pmp_hacksaw",
    "pmp_hammer",
    "pmp_jinx",
    "pmp_longshot",
    "pmp_meat",
    "pmp_mother",
    "pmp_nuke",
    "pmp_palerider",
    "pmp_payback",
    "pmp_psycho",
    "pmp_rooster",
    "pmp_scooter",
    "pmp_shadow",
    "pmp_steel",
    "pmp_twitch",
    "pmp_venom",
    "pmp_wicked",
    "pmp_worm",
    "pmwbubba",
    "pmwchopper",
    "pmwclaymore",
    "pmwcobra",
    "pmwcreep",
    "pmwdagger",
    "pmwelite",
    "pmwflash",
    "pmwghost",
    "pmwgreen",
    "pmwguardtower",
    "pmwhacksaw",
    "pmwhammer",
    "pmwjinx",
    "pmwlongshot",
    "pmwmeat",
    "pmwminelayer",
    "pmwmother",
    "pmwnuke",
    "pmwpalerider",
    "pmwpayback",
    "pmwpsycho",
    "pmwrecover",
    "pmwregular",
    "pmwrepair",
    "pmwrooster",
    "pmwscooter",
    "pmwscout",
    "pmwshadow",
    "pmwsparky",
    "pmwsteel",
    "pmwtool",
    "pmwtwitch",
    "pmwvenom",
    "pmwveteran",
    "pmwwicked",
    "pmwworm",
    "war001",
    "war002",
    "war002a",
    "war003",
    "war003a",
    "war004",
    "war005",
    "war006",
    "war007",
    "war008",
    "war009",
    "war010",
    "war011",
];

fn data_vfs() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA")?;
    Some(Vfs::from_system_cfg(root).expect("MC2_DATA: system.cfg and FastFiles mount"))
}

#[test]
fn every_warrior_file_loads_with_zero_errors() {
    let Some(mut vfs) = data_vfs() else {
        eprintln!("MC2_DATA unset: skipping warrior load test");
        return;
    };
    assert_eq!(WARRIORS.len(), 76);

    let mut profiles = Vec::new();
    for name in WARRIORS {
        let profile =
            WarriorProfile::load(&mut vfs, name).unwrap_or_else(|e| panic!("warrior {name}: {e}"));
        profiles.push(profile);
    }

    let not_mission_loadable: Vec<&str> = profiles
        .iter()
        .filter(|p| !p.mission_init_gaps().is_empty())
        .map(|p| p.file_name.as_str())
        .collect();
    for name in &not_mission_loadable {
        assert!(
            name.starts_with("war"),
            "unexpected mission-init gap in {name}"
        );
    }
    eprintln!(
        "warriors loaded: {} of {}; not mission-loadable (no Skills.Gunnery): {}",
        profiles.len(),
        WARRIORS.len(),
        not_mission_loadable.len()
    );

    let mut counts = [0usize; 23];
    for p in &profiles {
        for s in SpecialtySkill::ALL {
            if p.specialty[s.index()] {
                counts[s.index()] += 1;
            }
        }
    }
    for s in SpecialtySkill::ALL {
        eprintln!(
            "  specialty {:<26} {:>3} warriors",
            s.key(),
            counts[s.index()]
        );
    }
    let with_unmapped = profiles
        .iter()
        .filter(|p| !p.specialty_extra.is_empty())
        .count();
    eprintln!("warriors with keys no C++ table names: {with_unmapped}");
    let toughness = profiles
        .iter()
        .filter(|p| p.is_toughness_specialist())
        .count();
    eprintln!("toughness specialists (ejection bonus): {toughness}");
}

#[test]
fn pilot_roster_loads_every_listed_warrior() {
    let Some(mut vfs) = data_vfs() else {
        eprintln!("MC2_DATA unset: skipping pilot roster test");
        return;
    };
    let roster = load_roster(&mut vfs).expect("pilots.csv and its warrior files");
    assert!(roster.skipped.is_empty(), "skipped: {:?}", roster.skipped);
    assert_eq!(roster.pilots.len(), 52, "pilots.csv lists 52 warriors");
    assert_eq!(roster.pilots[0].id, 1);
    assert_eq!(roster.pilots[0].file_name, "pmwbubba");
    assert_eq!(roster.pilots[51].id, 52);
    assert!(roster.find("PMP_FLASH").is_some());

    let pmw = roster
        .pilots
        .iter()
        .filter(|p| p.file_name.starts_with("pmw"))
        .count();
    eprintln!(
        "roster: {} pilots ({} pmw, {} pmp)",
        roster.pilots.len(),
        pmw,
        roster.pilots.len() - pmw
    );

    let mut by_rank = [0usize; 5];
    for p in &roster.pilots {
        by_rank[p.rank as usize] += 1;
    }
    eprintln!(
        "roster ranks: green {} regular {} veteran {} elite {} ace {}",
        by_rank[WarriorRank::Green as usize],
        by_rank[WarriorRank::Regular as usize],
        by_rank[WarriorRank::Veteran as usize],
        by_rank[WarriorRank::Elite as usize],
        by_rank[WarriorRank::Ace as usize]
    );

    let fallback_icons = roster
        .pilots
        .iter()
        .filter(|p: &&PilotRecord| p.icon_file.ends_with(p.video.as_deref().unwrap_or("\u{0}")))
        .count();
    eprintln!("pilots whose icon comes from the video name: {fallback_icons}");

    for s in SpecialtySkill::ALL {
        let n = roster
            .pilots
            .iter()
            .filter(|p| p.specialty[s.index()])
            .count();
        eprintln!("  roster specialty {:<26} {:>3}", s.key(), n);
    }
}
