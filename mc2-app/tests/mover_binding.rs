// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Every simulation mover and structure is bound to a placed model at load, in all 24 campaign
//! missions (the production logistics launch of each; see `campaign_playthrough.rs`).

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_campaign::interface::{LoadSpec, MissionOptions, Outcome};
use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn campaign_missions(root: &Path) -> Result<Vec<String>, String> {
    let mut vfs = Vfs::from_system_cfg(root).map_err(|error| error.to_string())?;
    let data = GameData::load(&mut vfs).map_err(|error| error.to_string())?;
    let campaign = data
        .campaigns
        .get("campaign")
        .ok_or("campaign.fit did not load")?;
    Ok(campaign
        .missions()
        .map(|(_, mission)| mission.file_name.clone())
        .collect())
}

/// Builds the same `LoadSpec` the campaign's logistics screen hands to the production mission
/// loader.  `campaign.fit` chooses quickstart versus logistics; for the latter there is no shipped
/// canonical player lance, so this creates the smallest legal deterministic player choice: the
/// first purchasable chassis that fits the mission's drop limit with the first free pilot. The
/// builder advances the logistics state in campaign order with a synthetic completed outcome only
/// to reach the next campaign screen; it never uses a simulated run to choose a later force. That
/// force is campaign data, not a mission/simulation inspection, and `Logistics::load_spec` is the
/// production `Logistics::beginMission` handoff (`code/logistics.cpp:457, 759`).
fn campaign_launches(
    root: &Path,
    missions: &[String],
) -> Result<BTreeMap<String, LoadSpec>, String> {
    let mut vfs = Vfs::from_system_cfg(root).map_err(|error| error.to_string())?;
    let data = Arc::new(GameData::load(&mut vfs).map_err(|error| error.to_string())?);
    let mut logistics = Logistics::new(Arc::clone(&data)).map_err(|error| error.to_string())?;
    let mut launches = BTreeMap::new();
    for mission in missions {
        if logistics.current_mission() != mission {
            return Err(format!(
                "campaign order expected {mission}, logistics selected {}",
                logistics.current_mission()
            ));
        }
        if !logistics.skip_logistics() {
            logistics.add_c_bills(10_000_000);
            let pilot = logistics
                .first_available_pilot()
                .ok_or_else(|| format!("{mission}: campaign has no available pilot"))?;
            let candidate = logistics
                .purchasable_mechs()
                .into_iter()
                .find_map(|variant| {
                    let mech = logistics.purchase_mech(variant).ok()?;
                    logistics.can_add_mech_to_force_group(mech).then_some(mech)
                })
                .ok_or_else(|| format!("{mission}: no campaign chassis fits its drop limit"))?;
            logistics.set_mech_pilot(candidate, Some(pilot));
            logistics
                .add_mech_to_force_group(candidate, 1)
                .map_err(|error| format!("{mission}: add selected campaign mech: {error}"))?;
        }
        let spec = logistics.load_spec(MissionOptions::default());
        if spec.mission != *mission {
            return Err(format!(
                "{mission}: logistics handed off {}, not the selected mission",
                spec.mission
            ));
        }
        launches.insert(mission.clone(), spec);
        logistics
            .apply_outcome(&Outcome::won())
            .map_err(|error| format!("{mission}: advance campaign: {error}"))?;
    }
    Ok(launches)
}

/// `(type handle, x, y)` of each unbound mover, and of each unmatched structure.
type Gaps = (Vec<(i32, f32, f32)>, Vec<(i32, f32, f32)>);

/// Load-time binding failures `(unbound movers, unmatched structures)` of `launch`'s mission, as
/// `(type handle, x, y)` lists; `None` when no GPU adapter exists.
fn binding_gaps(root: &Path, launch: &LoadSpec) -> Option<Gaps> {
    let mut options = HeadlessOptions::new(root, &launch.mission);
    options.size = (800, 600);
    options.game.cursor = false;
    options.game.campaign_launch = Some(launch.clone());
    let mut run = match HeadlessRun::live_bare(&options) {
        Ok(run) => run,
        Err(e) if e.contains("no suitable GPU adapter") => {
            eprintln!("no GPU adapter; skipping: {e}");
            return None;
        }
        Err(e) => panic!("{}: create live run: {e}", launch.mission),
    };
    let mut guard = 0;
    while run.core.mission.is_none() {
        run.step(None).expect("load mission");
        guard += 1;
        assert!(guard < 1000, "{}: mission never loaded", launch.mission);
    }
    let driver = &run.core.mission.as_ref().expect("mission").driver;
    let world = &driver.sim().world;
    let describe = |hs: &[mc2_sim::ObjectHandle]| {
        hs.iter()
            .filter_map(|&h| world.objects.get(h))
            .map(|o| (o.c.type_handle, o.c.position.x, o.c.position.y))
            .collect::<Vec<_>>()
    };
    Some((
        describe(driver.load_unbound_movers()),
        describe(driver.load_unmatched_structures()),
    ))
}

#[test]
fn every_campaign_mission_binds_all_movers_and_structures_at_load() {
    let Some(root) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let missions = campaign_missions(&root).expect("campaign missions");
    assert_eq!(missions.len(), 24);
    let launches = campaign_launches(&root, &missions).expect("launches");
    let mut gaps = BTreeMap::new();
    for mission in &missions {
        if let Ok(only) = std::env::var("MC2_BIND_ONLY") {
            if &only != mission {
                continue;
            }
        }
        match binding_gaps(&root, &launches[mission]) {
            None => return,
            Some((m, s)) if !m.is_empty() || !s.is_empty() => {
                gaps.insert(mission.clone(), (m, s));
            }
            Some(_) => {}
        }
    }
    assert!(gaps.is_empty(), "unbound (movers, structures): {gaps:#?}");
}
