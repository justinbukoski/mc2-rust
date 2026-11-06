// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MovePathManager` (code/movemgr.cpp): FIFO queue of pilots waiting for `calcMovePath`, at most 6 served per frame.
//! Runs *before* the object update (`Mission::update` step), so a rethink leaves the mover standing still for at least one
//! frame (`pausePath` sets `numSteps = 0`).

use std::collections::VecDeque;

/// Paths computed per frame (`MAX_PATHS_PER_FRAME`-like constant, movemgr.cpp:176).
pub const PATHS_PER_FRAME: usize = 6;

/// One queued request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathRequest {
    /// Pilot slot.
    pub pilot: u16,
    /// Selector (`selectionIndex` argument of `calcMovePath`).
    pub selector: i32,
    /// `MOVEPARAM_*` bits.
    pub params: u32,
    /// Source code (which caller asked).
    pub source: i32,
}

/// The queue.
#[derive(Clone, Debug, Default)]
pub struct PathManager {
    /// Pending requests, oldest first.
    pub queue: VecDeque<PathRequest>,
}

impl PathManager {
    /// `request`: removes the pilot's existing entry, appends at the tail.
    pub fn request(&mut self, req: PathRequest) {
        self.remove(req.pilot);
        self.queue.push_back(req);
    }
    /// `remove(pilot)`.
    pub fn remove(&mut self, pilot: u16) {
        self.queue.retain(|request| request.pilot != pilot);
    }
    /// `update`: pop up to 6 and run `Mover::calcMovePath` for each (dead movers still use a slot).
    pub fn update(w: &mut crate::World) {
        for _ in 0..PATHS_PER_FRAME {
            let Some(request) = w.paths.queue.pop_front() else {
                break;
            };
            // `calcPath`: the pilot's vehicle may be gone (it still used a slot of the six), otherwise `pilot->calcMovePath`.
            if w.pilots.get(request.pilot).is_none() {
                continue;
            }
            if let Err(decision) = crate::ai::calcpath::calc_move_path(
                w,
                request.pilot,
                request.selector,
                request.params,
            ) {
                let part_id = crate::ai::warrior::vehicle(w, request.pilot)
                    .and_then(|h| w.objects.get(h))
                    .map_or(0, |obj| obj.c.part_id.0);
                w.events
                    .push(crate::SimEvent::MovementUnavailable { part_id, decision });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_request_moves_pilot_to_fifo_tail_and_removal_is_idempotent() {
        let mut paths = PathManager::default();
        let req = |pilot, selector| PathRequest {
            pilot,
            selector,
            params: 0,
            source: 1,
        };
        paths.request(req(1, 10));
        paths.request(req(2, 20));
        paths.request(req(1, 11));
        assert_eq!(
            paths.queue.iter().copied().collect::<Vec<_>>(),
            vec![req(2, 20), req(1, 11)]
        );
        paths.remove(1);
        paths.remove(1);
        assert_eq!(
            paths.queue.iter().copied().collect::<Vec<_>>(),
            vec![req(2, 20)]
        );
    }
}

// ---- save/load (lane B). The queue is order-significant (FIFO), so it is saved oldest first. ----

use crate::save::{Reader, SaveResult, Snapshot, Writer};

crate::snapshot_struct!(PathRequest {
    pilot,
    selector,
    params,
    source,
});

impl Snapshot for PathManager {
    fn save(&self, w: &mut Writer) {
        w.len(self.queue.len());
        for req in &self.queue {
            req.save(w);
        }
    }
    fn load(r: &mut Reader<'_>) -> SaveResult<Self> {
        Ok(PathManager {
            queue: Vec::<PathRequest>::load(r)?.into(),
        })
    }
}
