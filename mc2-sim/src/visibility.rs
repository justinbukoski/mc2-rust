// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The camera/visibility input of the simulation.
//!
//! In the original, what the *camera sees* changes simulation results. `Mission::update` runs `land->geometry()`
//! (camera projection of terrain vertices) before `ObjectManager::update`, and:
//! * terrain-object blocks that are not on screen (`Terrain::objBlockInfo[b].active`, `Terrain::objVertexActive[v]`) are
//!   not updated after turn 3 (buildings, turrets, terrain objects) - see `GameObjectManager::update`;
//! * movers whose appearance is not `inView` skip `updateGeometry()` (weapon node positions stay stale), withdrawing
//!   units are removed once out of view, and vehicle-pilot units are deleted when out of view (`BattleMech::update`,
//!   `GroundVehicle::update`, `Mech3DAppearance::update`, `GVAppearance::update`).
//!
//! The Rust simulation therefore never looks at a camera. The caller (the application, a test, a replay) provides a
//! [`Visibility`] for each frame; the oracle writes the sets it computed with the real camera to a trace file
//! (`--vis-trace`) which [`TraceVisibility`] replays.

use crate::ids::PartId;
use std::collections::{BTreeMap, BTreeSet};

/// Per-frame camera-derived inputs.
pub trait Visibility {
    /// `Terrain::objBlockInfo[block].active`.
    fn block_active(&self, block: u32) -> bool;
    /// `Terrain::objVertexActive[vertex]`.
    fn vertex_active(&self, vertex: u32) -> bool;
    /// `appearance->inView` for the mover with this part id.
    fn mover_in_view(&self, part: PartId) -> bool;
    /// `false` when the producer could not compute `appearance->inView` the way the original does (it needs the appearances' screen
    /// bounds: lane R1's `select_shapes` rough rect / OBB / type box). The `mover_in_view` / `structure_in_view` answers are then an
    /// approximation, and every [`Sim::step`](crate::Sim::step) reports [`FrameUnavailable::ObjectInView`](crate::FrameUnavailable)
    /// so no result that depends on them is taken for parity. Terrain activity is unaffected. A DECLARED UPSTREAM INPUT; the
    /// default says the answers are complete.
    fn object_in_view_available(&self) -> bool {
        true
    }
    /// Camera-selected mech LOD (`Mech3DAppearance::recalcBounds`).
    fn mover_lod(&self, _part: PartId) -> usize {
        0
    }
    /// `appearance->inView` (the `recalcBounds()` result) for the building, turret or gate / terrain object with this handle.
    /// Added by lane A; defaults to "in view".
    fn structure_in_view(&self, _handle: crate::ids::ObjectHandle) -> bool {
        true
    }
    /// The level of detail `recalcBounds` selects for the building or turret with this handle (bdactor.cpp:1305-1325:
    /// the camera distance against `lodDistance`, and the high-detail option). Added by lane A.
    ///
    /// A DECLARED UPSTREAM INPUT: the app's visibility producer supplies it (the camera is not part of the simulation).
    /// `None` means the producer did not supply it: the LOD is unavailable, and `hooks::recalc_bounds` then reports no
    /// bounds result rather than a normal one at LOD 0. No oracle trace records it yet, so [`TraceVisibility`] answers `None`.
    fn structure_lod(&self, _handle: crate::ids::ObjectHandle) -> Option<usize> {
        None
    }
}

/// Everything visible (a good default for headless simulation: all terrain objects update every frame).
#[derive(Clone, Copy, Debug, Default)]
pub struct AllVisible;

impl Visibility for AllVisible {
    fn block_active(&self, _block: u32) -> bool {
        true
    }
    fn vertex_active(&self, _vertex: u32) -> bool {
        true
    }
    fn mover_in_view(&self, _part: PartId) -> bool {
        true
    }
}

/// Nothing visible (terrain objects only update during the first 3 turns).
#[derive(Clone, Copy, Debug, Default)]
pub struct NoneVisible;

impl Visibility for NoneVisible {
    fn structure_in_view(&self, _handle: crate::ids::ObjectHandle) -> bool {
        false
    }
    fn block_active(&self, _block: u32) -> bool {
        false
    }
    fn vertex_active(&self, _vertex: u32) -> bool {
        false
    }
    fn mover_in_view(&self, _part: PartId) -> bool {
        false
    }
}

/// One change record of an oracle trace: the state that holds from `frame` until the next record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VisRecord {
    /// First frame the record applies to.
    pub frame: i64,
    /// Part ids of movers in view.
    pub movers: BTreeSet<i32>,
    /// Handles of structures (buildings, turrets, gates, terrain objects) whose appearance is in view (`S` record).
    pub structures: BTreeSet<u32>,
    /// Camera-selected LOD by mission part id (0 is the base shape).
    pub mover_lods: BTreeMap<i32, usize>,
    /// Camera-selected LOD by object handle (1-based).
    pub structure_lods: BTreeMap<u32, usize>,
    /// Active object blocks.
    pub blocks: BTreeSet<u32>,
    /// Active vertex ranges (inclusive), ascending.
    pub vertex_ranges: Vec<(u32, u32)>,
}

/// A trace written by `sim_oracle --vis-trace`, replayed frame by frame.
#[derive(Clone, Debug, Default)]
pub struct TraceVisibility {
    records: Vec<VisRecord>,
    current: usize,
}

impl TraceVisibility {
    /// Parse the oracle trace (`T`, `I`, `S`, `LM`, `LS`, `B`, and `V` records).
    pub fn parse(text: &str) -> Result<TraceVisibility, String> {
        let mut records: Vec<VisRecord> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let mut it = line.split_whitespace();
            let Some(tag) = it.next() else { continue };
            let err = |what: &str| format!("line {}: {what}", n + 1);
            match tag {
                "T" => {
                    let frame = it
                        .next()
                        .and_then(|t| t.parse().ok())
                        .ok_or_else(|| err("bad T"))?;
                    records.push(VisRecord {
                        frame,
                        ..VisRecord::default()
                    });
                }
                "I" | "S" | "LM" | "LS" | "B" | "V" => {
                    let r = records.last_mut().ok_or_else(|| err("record before T"))?;
                    for tok in it {
                        match tag {
                            "I" => {
                                r.movers
                                    .insert(tok.parse().map_err(|_| err("bad part id"))?);
                            }
                            "S" => {
                                r.structures
                                    .insert(tok.parse().map_err(|_| err("bad handle"))?);
                            }
                            "LM" | "LS" => {
                                let (id, lod) =
                                    tok.split_once(':').ok_or_else(|| err("bad LOD"))?;
                                let lod = lod.parse().map_err(|_| err("bad LOD value"))?;
                                if tag == "LM" {
                                    r.mover_lods
                                        .insert(id.parse().map_err(|_| err("bad LOD id"))?, lod);
                                } else {
                                    r.structure_lods
                                        .insert(id.parse().map_err(|_| err("bad LOD id"))?, lod);
                                }
                            }
                            "B" => {
                                r.blocks.insert(tok.parse().map_err(|_| err("bad block"))?);
                            }
                            _ => {
                                let (a, b) = tok.split_once('-').ok_or_else(|| err("bad range"))?;
                                r.vertex_ranges.push((
                                    a.parse().map_err(|_| err("bad range"))?,
                                    b.parse().map_err(|_| err("bad range"))?,
                                ));
                            }
                        }
                    }
                }
                _ => return Err(err("unknown tag")),
            }
        }
        Ok(TraceVisibility {
            records,
            current: 0,
        })
    }

    /// Select the record in force at `frame`.
    pub fn set_frame(&mut self, frame: i64) {
        let idx = self.records.partition_point(|r| r.frame <= frame);
        self.current = idx.saturating_sub(1);
    }

    fn rec(&self) -> Option<&VisRecord> {
        self.records.get(self.current)
    }
}

impl VisRecord {
    fn vertex_in_ranges(&self, vertex: u32) -> bool {
        let i = self.vertex_ranges.partition_point(|&(a, _)| a <= vertex);
        i > 0 && self.vertex_ranges[i - 1].1 >= vertex
    }
}

/// A single record is itself a [`Visibility`]: the camera state that holds for the frames it covers.
impl Visibility for VisRecord {
    fn block_active(&self, block: u32) -> bool {
        self.blocks.contains(&block)
    }
    fn vertex_active(&self, vertex: u32) -> bool {
        self.vertex_in_ranges(vertex)
    }
    fn mover_in_view(&self, part: PartId) -> bool {
        self.movers.contains(&part.0)
    }
    fn structure_in_view(&self, handle: crate::ids::ObjectHandle) -> bool {
        self.structures.contains(&handle.0)
    }
    fn mover_lod(&self, part: PartId) -> usize {
        self.mover_lods.get(&part.0).copied().unwrap_or(0)
    }
    fn structure_lod(&self, handle: crate::ids::ObjectHandle) -> Option<usize> {
        self.structure_lods.get(&handle.0).copied()
    }
}

impl TraceVisibility {
    /// The record in force at `frame` (a copy-free borrow; clone it to keep it past the next frame).
    pub fn record_at(&self, frame: i64) -> Option<&VisRecord> {
        let idx = self.records.partition_point(|r| r.frame <= frame);
        self.records.get(idx.saturating_sub(1))
    }
}

impl Visibility for TraceVisibility {
    fn block_active(&self, block: u32) -> bool {
        self.rec().is_some_and(|r| r.blocks.contains(&block))
    }
    fn vertex_active(&self, vertex: u32) -> bool {
        self.rec().is_some_and(|r| {
            let i = r.vertex_ranges.partition_point(|&(a, _)| a <= vertex);
            i > 0 && r.vertex_ranges[i - 1].1 >= vertex
        })
    }
    fn mover_in_view(&self, part: PartId) -> bool {
        self.rec().is_some_and(|r| r.movers.contains(&part.0))
    }
    fn structure_in_view(&self, handle: crate::ids::ObjectHandle) -> bool {
        self.rec().is_some_and(|r| r.structures.contains(&handle.0))
    }
    fn mover_lod(&self, part: PartId) -> usize {
        self.rec()
            .and_then(|r| r.mover_lods.get(&part.0))
            .copied()
            .unwrap_or(0)
    }
    fn structure_lod(&self, handle: crate::ids::ObjectHandle) -> Option<usize> {
        self.rec()
            .and_then(|r| r.structure_lods.get(&handle.0))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_replay() {
        let text = "T 0\nI\nS\nLM\nLS\nB\nV\nT 2\nI 704 705\nS 42\nLM 704:1 705:2\nLS 42:1\nB 13 14\nV 4273-4274 4372-4377\n";
        let mut t = TraceVisibility::parse(text).unwrap();
        t.set_frame(1);
        assert!(!t.block_active(13));
        t.set_frame(2);
        assert!(t.block_active(13));
        assert!(!t.block_active(12));
        assert!(t.vertex_active(4273) && t.vertex_active(4377) && !t.vertex_active(4275));
        assert!(t.mover_in_view(PartId(704)) && !t.mover_in_view(PartId(706)));
        assert_eq!(t.mover_lod(PartId(704)), 1);
        assert_eq!(t.mover_lod(PartId(705)), 2);
        assert_eq!(t.structure_lod(crate::ids::ObjectHandle(42)), Some(1));
        assert!(t.structure_in_view(crate::ids::ObjectHandle(42)));
        t.set_frame(1);
        assert!(!t.mover_in_view(PartId(704)));
        assert_eq!(t.mover_lod(PartId(704)), 0);
    }
}
