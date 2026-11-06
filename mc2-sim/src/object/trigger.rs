// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `TriggerAreaManager` (trigger.cpp): 16 areas, handles 1..15, linear scan from `Mover::setPosition`.
//! `remove()` tests `param == TRIGGER_AREA_NONE` instead of `type` (C++ bug; keep, see DESIGN.md).
//!
//! The manager lives in [`MissionState::triggers`](crate::mission::MissionState); the ABL natives (`addtriggerarea`,
//! `istriggerareahit`, `resettriggerarea`, `removetriggerarea`) are lane E's thin wrappers.

use crate::ids::ObjectHandle;
use crate::object::Kind;
use crate::World;

/// `MAX_MAP_CELL_WIDTH / 3` (the tile ownership map side).
const MAX_MAP_CELL_WIDTH_DIV3: usize = 240;
/// `MAX_TRIGGER_AREAS`.
pub const MAX_TRIGGER_AREAS: usize = 16;
/// `TRIGGER_AREA_NONE`.
pub const TRIGGER_AREA_NONE: i8 = 0;
/// `TRIGGER_AREA_MOVER`.
pub const TRIGGER_AREA_MOVER: i8 = 1;
/// `TRIGGER_AREA_TEAM`.
pub const TRIGGER_AREA_TEAM: i8 = 2;
/// `TRIGGER_AREA_GROUP`.
pub const TRIGGER_AREA_GROUP: i8 = 3;
/// `TRIGGER_AREA_COMMANDER`.
pub const TRIGGER_AREA_COMMANDER: i8 = 4;

/// One area (`TriggerArea`).
#[derive(Clone, Debug, Default)]
pub struct TriggerArea {
    /// `type` (`TRIGGER_AREA_*`).
    pub kind: i8,
    /// `param`: watch id, team, group or commander depending on the kind.
    pub param: i32,
    /// `dim`: upper-left row, upper-left column, lower-right row, lower-right column, in map cells.
    pub dim: [i32; 4],
    /// `hit`: latched until `reset`.
    pub hit: bool,
}

/// The manager (`TriggerAreaManager`).
#[derive(Clone, Debug)]
pub struct TriggerAreas {
    /// `triggerAreas[16]` (slot 0 is never handed out).
    pub areas: [TriggerArea; MAX_TRIGGER_AREAS],
    /// `map[240][240]` tile ownership (written by `add`/`remove`, never read by the shipped code).
    pub map: Vec<u8>,
}

impl Default for TriggerAreas {
    fn default() -> Self {
        TriggerAreas {
            areas: Default::default(),
            map: vec![0; MAX_MAP_CELL_WIDTH_DIV3 * MAX_MAP_CELL_WIDTH_DIV3],
        }
    }
}

impl TriggerAreas {
    /// `TriggerAreaManager::init`: all areas free, map cleared.
    pub fn init(&mut self) {
        *self = TriggerAreas::default();
    }

    fn map_index(r: i32, c: i32) -> Option<usize> {
        if r >= 0
            && c >= 0
            && (r as usize) < MAX_MAP_CELL_WIDTH_DIV3
            && (c as usize) < MAX_MAP_CELL_WIDTH_DIV3
        {
            Some(r as usize * MAX_MAP_CELL_WIDTH_DIV3 + c as usize)
        } else {
            None
        }
    }

    /// `add(ULrow, ULcol, LRrow, LRcol, type, param)`: the first free slot from 1, or 0 when all are in use.
    pub fn add(
        &mut self,
        ul_row: i32,
        ul_col: i32,
        lr_row: i32,
        lr_col: i32,
        kind: i32,
        param: i32,
    ) -> i32 {
        let Some(i) = (1..MAX_TRIGGER_AREAS).find(|&i| self.areas[i].kind == TRIGGER_AREA_NONE)
        else {
            return 0;
        };
        let a = &mut self.areas[i];
        a.dim = [ul_row, ul_col, lr_row, lr_col];
        a.kind = kind as i8;
        a.param = param;
        a.hit = false;
        let tile_dims = [ul_row / 3, ul_col / 3 + 1, lr_row / 3, lr_col / 3 + 1];
        for r in tile_dims[0]..tile_dims[2] {
            for c in tile_dims[1]..tile_dims[3] {
                if let Some(ix) = Self::map_index(r, c) {
                    self.map[ix] = i as u8;
                }
            }
        }
        i as i32
    }

    /// `remove(handle)`. The C++ tests `param == TRIGGER_AREA_NONE` (0) instead of `type`: an area whose param is 0 is never
    /// removed, and an area with a nonzero param is.
    pub fn remove(&mut self, handle: i32) {
        let Some(a) = self.areas.get(handle as usize) else {
            return;
        };
        if a.param == i32::from(TRIGGER_AREA_NONE) {
            return;
        }
        let tile_dims = [
            a.dim[0] / 3,
            a.dim[1] / 3 + 1,
            a.dim[2] / 3,
            a.dim[3] / 3 + 1,
        ];
        for r in tile_dims[0]..tile_dims[2] {
            for c in tile_dims[1]..tile_dims[3] {
                if let Some(ix) = Self::map_index(r, c) {
                    if i32::from(self.map[ix]) == handle {
                        self.map[ix] = 0;
                    }
                }
            }
        }
        let a = &mut self.areas[handle as usize];
        a.kind = TRIGGER_AREA_NONE;
        a.hit = false;
    }

    /// `reset(handle)`.
    pub fn reset(&mut self, handle: i32) {
        if let Some(a) = self.areas.get_mut(handle as usize) {
            a.hit = false;
        }
    }

    /// `isHit(handle)`.
    pub fn is_hit(&self, handle: i32) -> bool {
        match self.areas.get(handle as usize) {
            Some(a) if a.kind != TRIGGER_AREA_NONE => a.hit,
            _ => false,
        }
    }
}

/// `TriggerAreaManager::setHit(mover)` (trigger.cpp:98), called from `Mover::setPosition`: latch every area the mover's cell
/// is inside whose filter matches. `getWatchID()` is only evaluated (and may assign an id) for a mover-specific area the
/// mover is inside.
pub fn set_hit(world: &mut World, mover: ObjectHandle) {
    let (row, col, team, group, commander) = match world.objects.get(mover) {
        Some(o) => {
            let group = match &o.kind {
                Kind::Mech(m) => i32::from(m.m.group_id),
                Kind::Vehicle(v) => i32::from(v.m.group_id),
                _ => -1,
            };
            (
                i32::from(o.c.cell.0),
                i32::from(o.c.cell.1),
                o.team_id(),
                group,
                o.commander_id(),
            )
        }
        None => return,
    };
    for i in 0..MAX_TRIGGER_AREAS {
        let a = world.mission.triggers.areas[i].clone();
        if a.kind == TRIGGER_AREA_NONE {
            continue;
        }
        if row >= a.dim[0] && row < a.dim[2] && col >= a.dim[1] && col < a.dim[3] {
            let hit = match a.kind {
                TRIGGER_AREA_MOVER => {
                    if a.param > 0 {
                        world.objects.watch_id(mover).0 as i32 == a.param
                    } else {
                        true
                    }
                }
                TRIGGER_AREA_TEAM => team == a.param,
                TRIGGER_AREA_GROUP => group == a.param,
                TRIGGER_AREA_COMMANDER => commander == a.param,
                _ => false,
            };
            if hit {
                world.mission.triggers.areas[i].hit = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_uses_slots_from_one_and_remove_ignores_param_zero() {
        let mut t = TriggerAreas::default();
        let h1 = t.add(3, 3, 30, 30, i32::from(TRIGGER_AREA_TEAM), 0);
        assert_eq!(h1, 1);
        // param 0: remove() returns early (the C++ tests param, not type) and the area stays
        t.remove(h1);
        assert_eq!(t.areas[1].kind, TRIGGER_AREA_TEAM);
        let h2 = t.add(3, 3, 30, 30, i32::from(TRIGGER_AREA_TEAM), 1);
        assert_eq!(h2, 2);
        t.remove(h2);
        assert_eq!(t.areas[2].kind, TRIGGER_AREA_NONE);
        // the freed slot is reused
        assert_eq!(t.add(0, 0, 3, 3, i32::from(TRIGGER_AREA_GROUP), 2), 2);
    }

    #[test]
    fn exhaustion_returns_zero_and_is_hit_needs_a_live_area() {
        let mut t = TriggerAreas::default();
        for _ in 1..MAX_TRIGGER_AREAS {
            assert!(t.add(0, 0, 3, 3, 1, 7) > 0);
        }
        assert_eq!(t.add(0, 0, 3, 3, 1, 7), 0);
        assert!(!t.is_hit(1));
        t.areas[1].hit = true;
        assert!(t.is_hit(1));
        t.reset(1);
        assert!(!t.is_hit(1));
        assert!(!t.is_hit(0));
    }
}

// ==================================================================================================================
// Save/load (DESIGN.md section 9). Appended by the lane A save work. The areas are mission state (lane E holds the manager).
// ==================================================================================================================

crate::snapshot_struct!(TriggerArea {
    kind,
    param,
    dim,
    hit,
});

crate::snapshot_struct!(TriggerAreas { areas, map });
