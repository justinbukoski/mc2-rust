// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Integration test over the shipped mission move data (skipped unless `MC2_DATA` is set).
//!
//! For every `data/missions/*.pak` inside `mission.fst`: load the move packets (from packet 4,
//! as `mission.cpp` does) and run random queries, checking the invariants of each path.

mod support;

use mc2_path::{LoadError, MoveData, Mover, NoGates, Planner};
use support::{data_dir, Fst, LazyPak};

/// xorshift64*: deterministic, no external rand crate.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

const QUERIES_PER_MAP: usize = 50;
const DIRS: [(i32, i32); 8] = [
    (-1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
    (1, 0),
    (1, -1),
    (0, -1),
    (-1, -1),
];

/// Whether the search may stand on (r, c) for a ground mover that wades shallow water.
/// Gate cells are left to the gate rules (friendly or unowned gates cost clear regardless
/// of passability), so they count as open here.
fn ground_open(data: &MoveData, r: i32, c: i32) -> bool {
    let m = &data.mission;
    if !m.in_bounds(r, c) {
        return false;
    }
    let cell = m.cell(r, c);
    if cell.gate() != 0 {
        return true;
    }
    cell.passable() && !cell.off_map() && !cell.deep_water()
}

fn check_steps(
    data: &MoveData,
    from: (i32, i32),
    to: (i32, i32),
    steps: &[mc2_path::PathStep],
) -> Result<(), String> {
    if steps.is_empty() {
        return Err("empty path between distinct cells".into());
    }
    let mut prev = from;
    for (i, s) in steps.iter().enumerate() {
        let cell = (s.cell[0], s.cell[1]);
        let dr = cell.0 - prev.0;
        let dc = cell.1 - prev.1;
        let dir = DIRS
            .iter()
            .position(|&d| d == (dr, dc))
            .ok_or(format!("step {i}: {prev:?} -> {cell:?} not adjacent"))?;
        if dir % 2 == 1 {
            // Diagonal: at least one orthogonal neighbour must be open (corner rule).
            let a = ground_open(data, prev.0 + dr, prev.1);
            let b = ground_open(data, prev.0, prev.1 + dc);
            if !a && !b {
                return Err(format!(
                    "step {i}: diagonal {prev:?} -> {cell:?} cuts a blocked corner"
                ));
            }
        }
        if !ground_open(data, cell.0, cell.1) {
            return Err(format!("step {i}: cell {cell:?} is not open ground"));
        }
        prev = cell;
    }
    if prev != to {
        return Err(format!("path ends at {prev:?}, goal was {to:?}"));
    }
    Ok(())
}

fn check_global(data: &MoveData, route: &[mc2_path::GlobalPathStep]) -> Result<(), String> {
    let g = &data.global[0];
    let nd = g.num_doors() as i32;
    let last = route.last().ok_or("empty route")?;
    if last.goal_door != nd + 1 {
        return Err(format!(
            "last entry goal_door {} is not the goal placeholder {}",
            last.goal_door,
            nd + 1
        ));
    }
    for w in route.windows(2) {
        let door = g.door(w[0].goal_door as usize);
        let a = [door.area[0] as i32, door.area[1] as i32];
        if !a.contains(&w[0].thru_area) {
            return Err("route thru-area is not a side of its exit door".into());
        }
        let other = if a[0] == w[0].thru_area { a[1] } else { a[0] };
        if w[1].thru_area != other {
            return Err("next entry does not enter the other side of the door".into());
        }
    }
    Ok(())
}

#[test]
fn every_mission_move_map_loads_and_paths_are_valid() {
    let Some(dir) = data_dir() else {
        eprintln!("MC2_DATA not set; skipping mission move data test");
        return;
    };
    let fst = Fst::open(&dir.join("mission.fst"));
    let mut maps = 0usize;
    let mut no_move = Vec::new();
    let mut old_format: Vec<String> = Vec::new();
    let mut stats = (0usize, 0usize, 0usize, 0usize, 0usize); // planned, unreachable, simple, global-only, too long
    let mut failures = Vec::new();

    for e in fst
        .entries
        .iter()
        .filter(|e| e.name.starts_with("data/missions/") && e.name.ends_with(".pak"))
    {
        let bytes = fst.read(e);
        let packets = LazyPak::new(bytes);
        if packets.len() <= 4 || packets.kind(4) == 7 {
            no_move.push(e.name.clone());
            continue;
        }
        let data = match MoveData::load(&packets, 4) {
            Ok(d) => d,
            // mission.cpp refuses these ("old version of move data"); they are not loadable.
            Err(LoadError::BadVersion { found }) => {
                old_format.push(format!("{} (0x{found:06X})", e.name));
                continue;
            }
            Err(err) => {
                failures.push(format!("{}: load failed: {err}", e.name));
                continue;
            }
        };
        assert!(
            4 + data.packets_used <= packets.len(),
            "{}: consumed past end of pak",
            e.name
        );
        maps += 1;
        let w = data.mission.width();
        let h = data.mission.height();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15
            ^ (e.name.len() as u64)
            ^ (w as u64) << 16
            ^ (h as u64) << 32);
        let mut planner = Planner::new(data);
        let mover = Mover::mech(0, 60.0);
        let mut found = 0;
        for _ in 0..QUERIES_PER_MAP {
            let pick = |rng: &mut Rng, planner: &Planner| -> (i32, i32) {
                loop {
                    let r = rng.below(h as u64) as i32;
                    let c = rng.below(w as u64) as i32;
                    if ground_open(&planner.data, r, c) {
                        return (r, c);
                    }
                }
            };
            let a = pick(&mut rng, &planner);
            let b = pick(&mut rng, &planner);
            if a == b {
                continue;
            }
            // Area-level route.
            match planner.global_path(&mover, a, b, &NoGates) {
                Err(mc2_path::PathError::PathTooLong { .. }) => stats.4 += 1,
                Ok(route) if !route.is_empty() => {
                    if let Err(msg) = check_global(&planner.data, &route) {
                        failures.push(format!("{}: global {a:?}->{b:?}: {msg}", e.name));
                    }
                    stats.3 += 1;
                }
                Ok(_) => stats.1 += 1,
                Err(err) => failures.push(format!("{}: global {a:?}->{b:?}: {err}", e.name)),
            }
            // Full route.
            match planner.plan(&mover, a, b, &NoGates) {
                Ok(p) => {
                    found += 1;
                    stats.0 += 1;
                    if let Err(msg) = check_steps(&planner.data, a, b, &p.steps) {
                        failures.push(format!("{}: plan {a:?}->{b:?}: {msg}", e.name));
                    }
                }
                Err(err) => {
                    if matches!(err, mc2_path::PathError::PathTooLong { .. }) {
                        stats.4 += 1;
                    } else if !matches!(err, mc2_path::PathError::NoRoute) {
                        failures.push(format!("{}: plan {a:?}->{b:?}: {err}", e.name));
                    }
                    stats.1 += 1;
                }
            }
            // Short-range window path.
            let close = (a.0 - b.0).abs() <= 21 && (a.1 - b.1).abs() <= 21;
            if close {
                match planner.simple_path(&mover, a, b, &NoGates) {
                    Ok(res) if res.path.num_steps > 0 => {
                        stats.2 += 1;
                        if let Err(msg) = check_steps(&planner.data, a, b, res.path.path_steps()) {
                            failures.push(format!("{}: simple {a:?}->{b:?}: {msg}", e.name));
                        }
                    }
                    Ok(_) => {}
                    Err(err) => failures.push(format!("{}: simple {a:?}->{b:?}: {err}", e.name)),
                }
            }
        }
        let _ = found;
    }

    eprintln!(
        "move maps loaded: {maps}; without move packets: {:?}; old format: {:?}",
        no_move, old_format
    );
    eprintln!(
        "plan ok/unreachable: {}/{}, simple ok: {}, global ok: {}, over MAX_GLOBAL_PATH: {}",
        stats.0, stats.1, stats.2, stats.3, stats.4
    );
    assert!(
        failures.is_empty(),
        "{} failures, first few:\n{}",
        failures.len(),
        failures
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
