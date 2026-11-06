// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The gate state `mc2-path` asks about while searching (`IsGateDisabled` / `IsGateOpen`, mission.cpp:1605-1636), keyed by the
//! watch id of the object that owns a global-map area.
//!
//! Path queries run while the caller holds the terrain (and so the move data) mutably, so the oracle is a detached
//! [`GateSnapshot`] taken from the object store just before the search instead of a borrow of the world.

use std::collections::BTreeMap;

use mc2_path::GateOracle;

use super::{Kind, ObjectStore};

/// `(disabled, open)` per area-owner watch id, for every object that owns global-map areas (walls, bridges, gates).
#[derive(Clone, Debug, Default)]
pub struct GateSnapshot {
    states: BTreeMap<i32, (bool, bool)>,
}

impl GateSnapshot {
    /// Capture the state of every area-owning structure.
    ///
    /// `IsGateDisabled(wid)`: the object is disabled or destroyed, or it is a building-like object (building, turret, gate)
    /// whose parent is disabled or destroyed. `IsGateOpen(wid)`: the object is a gate and `opened`.
    pub fn from_store(store: &ObjectStore) -> GateSnapshot {
        let mut states = BTreeMap::new();
        for o in store.list.iter().skip(1).flatten() {
            let owns_areas = o
                .structure()
                .is_some_and(|s| !s.sub_areas[0].is_empty() || !s.sub_areas[1].is_empty());
            if !owns_areas || o.c.watch_id.is_none() {
                continue;
            }
            let mut disabled = o.c.is_disabled();
            if !disabled {
                let parent = match &o.kind {
                    Kind::Building(b) => Some(b.parent),
                    Kind::Gate(g) => Some(g.parent),
                    Kind::Turret(t) => Some(t.parent),
                    _ => None,
                };
                if let Some(p) = parent {
                    if let Some(po) = store.get_by_watch_id(p) {
                        disabled = po.c.is_disabled();
                    }
                }
            }
            let open = matches!(&o.kind, Kind::Gate(g) if g.opened);
            states.insert(o.c.watch_id.0 as i32, (disabled, open));
        }
        GateSnapshot { states }
    }
}

impl GateOracle for GateSnapshot {
    fn is_gate_disabled(&self, owner_wid: i32) -> bool {
        if owner_wid < 1 {
            return false;
        }
        self.states.get(&owner_wid).is_some_and(|s| s.0)
    }

    fn is_gate_open(&self, owner_wid: i32) -> bool {
        if owner_wid == 0 {
            return false;
        }
        self.states.get(&owner_wid).is_some_and(|s| s.1)
    }
}
