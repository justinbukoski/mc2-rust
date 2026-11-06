//! Shared helpers for the data-driven tests. All of them skip cleanly without `MC2_DATA`.
#![allow(dead_code)]

pub mod bay;

use std::sync::{Arc, OnceLock};

use mc2_campaign::GameData;
use mc2_formats::vfs::Vfs;
use mc2_sim_data::mission::{Action, Mission};

/// A VFS over the built game data, or `None` when `MC2_DATA` is unset.
pub fn vfs() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA")?;
    Some(Vfs::from_system_cfg(root).expect("MC2_DATA: system.cfg and FastFiles mount"))
}

/// The game data, loaded once per test binary.
pub fn game() -> Option<Arc<GameData>> {
    static DATA: OnceLock<Option<Arc<GameData>>> = OnceLock::new();
    DATA.get_or_init(|| {
        let mut vfs = vfs()?;
        Some(Arc::new(GameData::load(&mut vfs).expect("GameData::load")))
    })
    .clone()
}

/// Skips the test when there is no data directory.
#[macro_export]
macro_rules! require_data {
    () => {
        match $crate::common::game() {
            Some(d) => d,
            None => {
                eprintln!("MC2_DATA unset: skipping");
                return;
            }
        }
    };
}

/// What a mission file says about the home team's objectives.
pub struct Obj {
    pub priority: u32,
    pub rp: i32,
    pub on_success: Vec<String>,
    pub on_failure: Vec<String>,
}

pub fn objectives_of(vfs: &mut Vfs, mission: &str) -> Vec<Obj> {
    let m = Mission::load_named(vfs, &format!("data/missions/{mission}.fit")).unwrap();
    let Some(team) = m.teams.iter().find(|t| t.team == 0) else {
        return Vec::new();
    };
    let files = |actions: &[Action]| {
        actions
            .iter()
            .filter_map(|a| match a {
                Action::MakeNewTechnologyAvailable { purchase_file_name } => {
                    Some(purchase_file_name.clone())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    team.objectives
        .iter()
        .map(|o| Obj {
            priority: o.priority,
            rp: o.resource_points as i32,
            on_success: files(&o.actions),
            on_failure: files(&o.failure_actions),
        })
        .collect()
}
