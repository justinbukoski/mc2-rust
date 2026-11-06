//! Shipped mission data: every mission `.fit` in `mission.fst` loads with no
//! error, and the counts of what it holds are printed.
//!
//! Set `MC2_DATA` to the data root (the directory holding `system.cfg` and the
//! `.fst` files). Without it the tests print a note and return.

use mc2_formats::fst::FastFile;
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use mc2_sim_data::mission::{Action, Condition, Mission, ObjectiveOrigin, TeamObjectives};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn data_root() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(p) => Some(PathBuf::from(p)),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped mission test");
            None
        }
    }
}

/// Every `.fit` entry in `mission.fst` whose name is under `missions/`, with its
/// bytes, in archive order.
fn mission_entries(root: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut fst = FastFile::open(root.join("mission.fst")).expect("open mission.fst");
    let names: Vec<(usize, String)> = fst
        .entries()
        .iter()
        .enumerate()
        .map(|(i, e)| (i, e.name.clone()))
        .collect();
    let mut out = Vec::new();
    for (index, name) in names {
        let lower = name.to_ascii_lowercase().replace('\\', "/");
        if lower.ends_with(".fit")
            && lower.contains("missions/")
            && !lower.contains("/warriors/")
            && !lower.contains("/profiles/")
        {
            let bytes = fst.read_index(index).expect("read mission entry");
            out.push((name, bytes));
        }
    }
    out
}

#[test]
fn shipped_missions_load_with_zero_errors() {
    let Some(root) = data_root() else { return };
    let _vfs = Vfs::from_system_cfg(&root).expect("system.cfg mounts");

    let entries = mission_entries(&root);
    assert!(
        !entries.is_empty(),
        "no mission .fit entries in mission.fst"
    );

    let mut missions = Vec::new();
    let mut not_missions = Vec::new();
    for (name, bytes) in entries {
        let mut fit = FitIni::from_bytes(bytes).expect("FIT header");
        if Mission::is_mission_file(&fit) {
            let m = Mission::load(&mut fit, &name)
                .unwrap_or_else(|e| panic!("{name}: load failed: {e}"));
            missions.push(m);
        } else {
            not_missions.push(name);
        }
    }
    println!("non-mission .fit files (no [Parts] and [MissionSettings]): {not_missions:?}");

    let mut parts = 0usize;
    let mut warriors = 0usize;
    let mut objectives = 0usize;
    let mut nav_objectives = 0usize;
    let mut conditions: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut failure_conditions = 0usize;
    let mut actions: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut failure_actions = 0usize;
    let mut warning_kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut warnings_total = 0usize;
    let mut forests = 0usize;
    let mut groups = 0usize;
    let mut nav_markers = 0usize;
    let mut cells = 0usize;
    let mut statics = 0usize;
    let mut teams_total = 0usize;
    let mut boolean_flags = 0usize;

    for m in &missions {
        parts += m.parts.len();
        warriors += m.warriors.len();
        forests += m.forests.len();
        groups += m.commander_groups.len();
        nav_markers += m.nav_markers.len();
        teams_total += m.teams.len();
        for w in &m.warriors {
            cells += w.cells.len();
            statics += w.static_vars.len();
        }
        for t in &m.teams {
            boolean_flags += t.boolean_flags.len();
            count_team(
                t,
                &mut objectives,
                &mut nav_objectives,
                &mut conditions,
                &mut failure_conditions,
                &mut actions,
                &mut failure_actions,
            );
        }
        for w in &m.warnings {
            warnings_total += 1;
            // Group by reason: drop the block name and the digits.
            let reason = w.split("] ").nth(1).unwrap_or(w);
            let kind: String = reason
                .split(" (")
                .next()
                .unwrap_or(reason)
                .chars()
                .filter(|c| !c.is_ascii_digit())
                .collect();
            *warning_kinds.entry(kind).or_default() += 1;
        }
    }

    println!("missions: {}", missions.len());
    println!("parts: {parts}");
    println!("warriors: {warriors} (brain cells {cells}, static vars {statics})");
    println!("team lists: {teams_total}, boolean flags: {boolean_flags}");
    println!("objectives (blocks): {objectives}, nav-marker objectives: {nav_objectives}");
    println!("nav markers: {nav_markers}, forests: {forests}, commander groups: {groups}");
    println!("conditions by species:");
    for (species, n) in &conditions {
        println!("  {species}: {n}");
    }
    println!("failure conditions: {failure_conditions}");
    println!("actions by species:");
    for (species, n) in &actions {
        println!("  {species}: {n}");
    }
    println!("failure actions: {failure_actions}");
    println!("warnings: {warnings_total}");
    for (kind, n) in &warning_kinds {
        println!("  {n} x {kind}");
    }

    assert_eq!(missions.len(), 44, "the shipped set has 44 mission files");
    assert!(parts > 0 && warriors > 0 && objectives > 0);
}

fn count_team(
    t: &TeamObjectives,
    objectives: &mut usize,
    nav_objectives: &mut usize,
    conditions: &mut BTreeMap<&'static str, usize>,
    failure_conditions: &mut usize,
    actions: &mut BTreeMap<&'static str, usize>,
    failure_actions: &mut usize,
) {
    for o in &t.objectives {
        match o.origin {
            ObjectiveOrigin::Block { .. } => *objectives += 1,
            ObjectiveOrigin::NavMarker { .. } => *nav_objectives += 1,
        }
        for c in &o.conditions {
            *conditions.entry(condition_name(c)).or_default() += 1;
        }
        *failure_conditions += o.failure_conditions.len();
        for a in &o.actions {
            *actions.entry(action_name(a)).or_default() += 1;
        }
        *failure_actions += o.failure_actions.len();
    }
}

fn condition_name(c: &Condition) -> &'static str {
    c.species().name()
}

fn action_name(a: &Action) -> &'static str {
    a.species().name()
}
