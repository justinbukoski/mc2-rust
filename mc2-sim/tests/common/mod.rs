//! Shared fixtures of the oracle tests.
//!
//! EVIDENCE RULE (sim base 0): oracle data may replace only DECLARED UPSTREAM inputs. A world is built here by the Rust mission
//! load alone ([`mc2_sim::mission::init::load_world`]): RNG draw counts and states, object handles / part ids / watch ids, mover
//! teams and flags, capturable flags and every other thing a test later compares are computed by Rust. The previous helper
//! installed them from the oracle digest (and advanced the RNG to the oracle's draw count), which made every comparison against
//! that digest circular; it is gone.
//!
//! What a test may still take from the oracle, and must say so where it does: the per-frame state of movers (position, rotation,
//! status, flags: lanes B/C/D output, not yet ported), the camera visibility trace, the hit script, and the RNG draw counts that
//! other lanes make between the turrets' updates (installed before a turret update, never asserted as a result).
//!
//! A load that still reports unported hooks ([`load_blocker`]) makes [`fixture`] panic with that message rather than silently
//! using other data; the missions of this slice load completely, so the hooks are no longer `todo!()`.

#![allow(dead_code)]

pub mod fire_dump;

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use mc2_formats::vfs::Vfs;
use mc2_sim::digest::{DigestLine, FrameDigest};
use mc2_sim::ids::ObjectHandle;
use mc2_sim::mission::init::{load_world, LoadOutcome, LoadSpec};
use mc2_sim::object::{Kind, ObjectStatus};
use mc2_sim::rng::RngMode;
use mc2_sim::World;

pub fn golden(name: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

pub fn data_root() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) {
        Some(p) => Some(PathBuf::from(p)),
        None if std::env::var_os("MC2_ALLOW_SKIP").is_some() => {
            eprintln!(
                "MC2_DATA not set and MC2_ALLOW_SKIP set; skipping the test against the game data"
            );
            None
        }
        None => panic!(
            "MC2_DATA is not set: the game-data tests would not run (set MC2_ALLOW_SKIP=1 to skip)"
        ),
    }
}

pub fn tokens(l: &str) -> Vec<String> {
    l.split_whitespace().map(str::to_string).collect()
}

/// The three RNG modes of the oracle with the seed each golden was made with (`oracle/gen_frame0_goldens.sh`).
pub const RNG_MODES: [(&str, RngMode, u32); 3] = [
    ("msvc", RngMode::LegacyMsvc, 1),
    ("glibc", RngMode::LegacyGlibc, 7),
    ("pcg", RngMode::Pcg32, 7),
];

/// Mode and seed of an oracle RNG name (`msvc`, `glibc`, `pcg`).
pub fn rng_by_name(name: &str) -> (RngMode, u32) {
    RNG_MODES
        .iter()
        .find(|m| m.0 == name)
        .map(|m| (m.1, m.2))
        .unwrap_or_else(|| panic!("unknown RNG mode {name}"))
}

/// Name of the golden `tests/golden/frame0/<mission>_<rng>.digest`.
pub fn frame0_golden(mission: &str, rng: &str) -> String {
    golden(&format!("frame0/{mission}_{rng}.digest"))
}

/// Load a mission with the Rust code only. `Err` carries the panic message of a load that reached
/// an unported function, `Ok` the load outcome with its completeness report.
pub fn load_blocker(vfs: &mut Vfs, spec: &LoadSpec) -> Result<LoadOutcome, String> {
    match catch_unwind(AssertUnwindSafe(|| load_world(vfs, spec))) {
        Ok(Ok(w)) => Ok(w),
        Ok(Err(e)) => panic!("{}: mission load failed: {e}", spec.mission),
        Err(p) => Err(p
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_else(|| "non-string panic".to_string())),
    }
}

/// A mission world made by the Rust load.
pub struct Fixture {
    pub world: World,
    pub mission: String,
    pub vfs: Vfs,
    pub fps: u32,
    /// part id -> handle of every mover (read from the loaded world).
    pub mover_handles: BTreeMap<i32, ObjectHandle>,
}

/// The mission `mission` loaded by Rust under the RNG `rng` (`msvc`, `glibc`, `pcg`) with the seed of the goldens. `None` without
/// `MC2_DATA`. Panics with the unported hook while lanes B, C and D have not landed (see the module comment).
pub fn fixture(mission: &str, rng: &str) -> Option<Fixture> {
    fixture_seeded(mission, rng, rng_by_name(rng).1)
}

/// [`fixture`] with an explicit seed (the scenario traces were recorded with seed 1 in every RNG mode).
pub fn fixture_seeded(mission: &str, rng: &str, seed: u32) -> Option<Fixture> {
    let root = data_root()?;
    let (mode, _) = rng_by_name(rng);
    let mut vfs = Vfs::from_system_cfg(&root).expect("mount archives");
    let spec = LoadSpec::quickstart(mission, seed, mode);
    // The object tests read only structure/mover state the digest shows; they never `Sim::step`, so an incomplete outcome is fine.
    let world = load_blocker(&mut vfs, &spec)
        .map(|o| o.world)
        .unwrap_or_else(|e| panic!("{mission} {rng}: the Rust mission load is blocked by `{e}`"));
    let mover_handles = world
        .objects
        .movers
        .iter()
        .map(|&h| (world.objects.get(h).unwrap().c.part_id.0, h))
        .collect();
    Some(Fixture {
        world,
        mission: mission.to_string(),
        vfs,
        fps: 30,
        mover_handles,
    })
}

/// One mover's state from an `M`/`V` digest line: `M part class x y z rot status flags iv t<team> c<cmdr> ...`. This is a declared
/// upstream input of the structure replays (movers are lanes B/C/D); identity, team and commander stay what the Rust load made.
#[derive(Clone, Debug)]
pub struct MoverState {
    pub part: i32,
    pub pos: mc2_sim::math::Vec3,
    pub rot: f32,
    pub status: u8,
    pub flags: u32,
}

impl MoverState {
    pub fn parse(t: &[String]) -> MoverState {
        let f = |i: usize| t[i].parse::<f32>().unwrap();
        MoverState {
            part: t[1].parse().unwrap(),
            pos: mc2_sim::math::Vec3 {
                x: f(3),
                y: f(4),
                z: f(5),
            },
            rot: f(6),
            status: t[7].parse().unwrap(),
            flags: t[8].parse().unwrap(),
        }
    }
}

/// Apply a mover's replayed state to its object (position through `GameObject::setPosition`).
pub fn apply_mover_state(world: &mut World, h: ObjectHandle, s: &MoverState) {
    mc2_sim::object::set_position(world, h, s.pos, true);
    let o = world.objects.get_mut(h).unwrap();
    o.c.rotation = s.rot;
    o.c.status = match s.status {
        0 => ObjectStatus::Normal,
        1 => ObjectStatus::Disabled,
        2 => ObjectStatus::Destroyed,
        3 => ObjectStatus::StartingUp,
        4 => ObjectStatus::ShuttingDown,
        _ => ObjectStatus::ShutDown,
    };
    o.c.flags = s.flags;
    assert!(
        matches!(o.kind, Kind::Mech(_) | Kind::Vehicle(_)),
        "handle {} is not a mover",
        h.0
    );
}

/// `status` and `flags` of every mover of a loaded world, by part id (the state before the first replayed frame).
pub fn mover_status_flags(world: &World) -> BTreeMap<i32, (u8, u32)> {
    world
        .objects
        .movers
        .iter()
        .map(|&h| {
            let o = world.objects.get(h).unwrap();
            (o.c.part_id.0, (o.c.status as u8, o.c.flags))
        })
        .collect()
}

/// The object lines of a loaded world at digest level 4.
pub fn structure_lines(world: &World) -> Vec<String> {
    let mut out = Vec::new();
    mc2_sim::object::digest::structure_lines(world, 4, &mut out);
    out
}

/// Whether the first token of `line` is one of `tags`.
fn line_has_tag(line: &str, tags: &[&str]) -> bool {
    let tag = line.split_whitespace().next().unwrap_or("");
    for t in tags {
        if *t == tag {
            return true;
        }
    }
    false
}

/// Compare the lines of the given tags against the golden, reporting the first differences.
pub fn compare(name: &str, got: &[String], golden_text: &str, tags: &[&str]) {
    let want: Vec<&str> = golden_text
        .lines()
        .filter(|l| line_has_tag(l, tags))
        .collect();
    let got_f: Vec<&String> = got.iter().filter(|l| line_has_tag(l, tags)).collect();
    let mut diffs = Vec::new();
    for (i, (a, b)) in want.iter().zip(got_f.iter()).enumerate() {
        if *a != b.as_str() {
            diffs.push(format!("{name}: line {i} differs\n  want {a}\n  got  {b}"));
        }
    }
    if want.len() != got_f.len() {
        diffs.push(format!(
            "{name}: line count want {} got {}",
            want.len(),
            got_f.len()
        ));
    }
    assert!(
        diffs.is_empty(),
        "{} differences, first ones:\n{}",
        diffs.len(),
        diffs.iter().take(8).cloned().collect::<Vec<_>>().join("\n")
    );
}

// ---------------------------------------------------------------------------------------------------------------------
// Digest divergence reports
// ---------------------------------------------------------------------------------------------------------------------

/// Field names of the `F` record (oracle/DIGEST.md).
const F_FIELDS: [&str; 8] = [
    "frame",
    "turn",
    "scenarioTime",
    "frameLength",
    "simDraws",
    "simState",
    "visDraws",
    "visState",
];

/// The name of token `i` of a digest line, for the report of a divergence: `M`/`V` movers (`<part> <class> <x y z> <rot> <status>
/// <flags> iv t c <vx vy vz> <timeLeft>` then the `A`rmor, `S`tructure, `W`arrior and `I`nventory groups), the lane D `P` and `KM`
/// rows, and the `F` record. Other tags are reported by token position.
pub fn field_name(tag: &str, tokens: &[String], i: usize) -> String {
    const M_FIELDS: [&str; 16] = [
        "part",
        "class",
        "x",
        "y",
        "z",
        "rotation",
        "status",
        "flags",
        "inView",
        "team",
        "commander",
        "velocityX",
        "velocityY",
        "velocityZ",
        "timeLeft",
        "",
    ];
    const W_FIELDS: [&str; 5] = ["wounds", "order", "stage", "targetWid", "lastTargetWid"];
    const P_FIELDS: [&str; 11] = [
        "part",
        "wounds",
        "order",
        "stage",
        "targetWid",
        "lastTargetWid",
        "brainUpdate",
        "combatUpdate",
        "movementUpdate",
        "alarmTriggers",
        "queuedOrders",
    ];
    // `PD` (oracle/DIGEST.md): glued prefix+value tokens, named by their prefix
    const PD_FIELDS: [&str; 41] = [
        "orderState",
        "newOrderReceived",
        "alarmPriority",
        "generalOrder",
        "playerOrder",
        "alarmOrder",
        "lastOrder",
        "weaponsStatus",
        "lastTargetTime",
        "lastTargetFlags",
        "moveGoalType",
        "moveGoalObject",
        "moveGoalX",
        "moveGoalY",
        "moveGoalZ",
        "pathType",
        "moveState",
        "moveStateGoal",
        "run",
        "twisting",
        "yieldTime",
        "yieldState",
        "waitForPointTime",
        "timeOfLastStep",
        "path0",
        "path1",
        "globalSteps",
        "originalGoalX",
        "originalGoalY",
        "brainState",
        "timeOfLastOrders",
        "willHelp",
        "keepMoving",
        "useGoalPlan",
        "mainGoal",
        "coreScanTarget",
        "queueExecuting",
        "queueIndex",
        "attackOrders",
        "attackRadius",
        "token 41",
    ];
    match tag {
        "F" => F_FIELDS.get(i).map(|n| n.to_string()),
        "P" => P_FIELDS.get(i).map(|n| n.to_string()),
        "PD" if i >= 1 => PD_FIELDS.get(i - 1).map(|n| format!("PD.{n}")),
        "KM" if i >= 1 => Some(format!("contactStatus[team {}]", i - 1)),
        "KM" => Some("part".to_string()),
        "M" | "V" => {
            // tokens: 0 part, 1 class, 2..4 position, 5 rotation, 6 status, 7 flags, 8 iv, 9 t, 10 c, 11..13 velocity, 14 timeLeft, 15 'A'
            if i < 15 {
                Some(M_FIELDS[i].to_string())
            } else {
                let mut group = "";
                let mut start = 0;
                for (k, t) in tokens.iter().enumerate().take(i + 1).skip(15) {
                    if matches!(t.as_str(), "A" | "S" | "W" | "I") {
                        group = match t.as_str() {
                            "A" => "armor",
                            "S" => "structure",
                            "W" => "warrior",
                            _ => "inventory",
                        };
                        start = k + 1;
                    }
                }
                let n = i.saturating_sub(start);
                match group {
                    "warrior" => Some(format!(
                        "warrior.{}",
                        W_FIELDS.get(n).copied().unwrap_or("extra")
                    )),
                    "" => None,
                    g if start == i + 1 => Some(format!("{g} marker")),
                    g => Some(format!("{g}[{n}]")),
                }
            }
        }
        _ => None,
    }
    .unwrap_or_else(|| format!("token {i}"))
}

/// Where two lines of one tag first differ: `<tag> <object> field <name>: oracle .. rust ..`.
pub fn line_divergence(want: &DigestLine, got: &DigestLine) -> Option<String> {
    if want == got {
        return None;
    }
    let object = match want.tag.as_str() {
        "M" | "V" | "B" | "T" | "G" | "OH" | "KD" | "P" | "PD" | "KM" => {
            format!(" object {}", want.tokens.first()?)
        }
        _ => String::new(),
    };
    let n = want.tokens.len().max(got.tokens.len());
    for i in 0..n {
        let (a, b) = (want.tokens.get(i), got.tokens.get(i));
        if a != b {
            let field = field_name(&want.tag, &want.tokens, i);
            return Some(format!(
                "{}{object} field {field}: oracle {} rust {}",
                want.tag,
                a.map_or("<none>", String::as_str),
                b.map_or("<none>", String::as_str)
            ));
        }
    }
    None
}

/// The first divergence of every tag, in oracle line order (so the first entry is the first divergent line of the frame). A tag the
/// oracle writes and Rust does not is reported as `missing` with the oracle's line count.
pub fn divergences(want: &FrameDigest, got: &FrameDigest) -> Vec<String> {
    let mut tags: Vec<&str> = Vec::new();
    for l in want.lines.iter().chain(&got.lines) {
        if !tags.contains(&l.tag.as_str()) {
            tags.push(&l.tag);
        }
    }
    let mut found: Vec<(usize, String)> = Vec::new();
    for tag in tags {
        let w: Vec<(usize, &DigestLine)> = want
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.tag == tag)
            .collect();
        let g: Vec<&DigestLine> = got.lines.iter().filter(|l| l.tag == tag).collect();
        let order = w.first().map_or(usize::MAX, |x| x.0);
        if g.is_empty() {
            found.push((
                order,
                format!("{tag}: missing from rust ({} oracle lines)", w.len()),
            ));
            continue;
        }
        if w.is_empty() {
            found.push((
                order,
                format!("{tag}: not in the oracle ({} rust lines)", g.len()),
            ));
            continue;
        }
        let first = w
            .iter()
            .zip(&g)
            .find_map(|((i, a), b)| line_divergence(a, b).map(|d| (*i, d)));
        if let Some((i, d)) = first {
            found.push((i, d));
        } else if w.len() != g.len() {
            found.push((
                order,
                format!("{tag}: line count oracle {} rust {}", w.len(), g.len()),
            ));
        }
    }
    found.sort();
    found.into_iter().map(|(_, d)| d).collect()
}

/// Every differing field of every line of a frame: `(tag, field name, first object, oracle token, rust token)`, grouped by tag + field
/// in oracle line order, with the first object (and values) the field differs for. Lines are matched by tag and position within the
/// tag, as in [`divergences`].
pub fn field_divergences(
    want: &FrameDigest,
    got: &FrameDigest,
) -> Vec<(String, String, String, String, String)> {
    let mut out: Vec<(String, String, String, String, String)> = Vec::new();
    let mut tags: Vec<&str> = Vec::new();
    for l in want.lines.iter().chain(&got.lines) {
        if !tags.contains(&l.tag.as_str()) {
            tags.push(&l.tag);
        }
    }
    for tag in tags {
        let w: Vec<&DigestLine> = want.lines.iter().filter(|l| l.tag == tag).collect();
        let g: Vec<&DigestLine> = got.lines.iter().filter(|l| l.tag == tag).collect();
        if g.is_empty() || w.is_empty() || w.len() != g.len() {
            out.push((
                tag.to_string(),
                "line count".to_string(),
                String::new(),
                w.len().to_string(),
                g.len().to_string(),
            ));
            continue;
        }
        for (a, b) in w.iter().zip(&g) {
            for i in 0..a.tokens.len().max(b.tokens.len()) {
                let (x, y) = (a.tokens.get(i), b.tokens.get(i));
                if x == y {
                    continue;
                }
                let field = field_name(&a.tag, &a.tokens, i);
                // strip the digits of a mover-indexed field (`armor[3]` -> `armor[]`) so the group is per field kind
                let kind = field.split('[').next().unwrap_or(&field).to_string();
                let object = a.tokens.first().cloned().unwrap_or_default();
                if !out.iter().any(|o| o.0 == tag && o.1 == kind) {
                    out.push((
                        tag.to_string(),
                        kind,
                        object,
                        x.cloned().unwrap_or_default(),
                        y.cloned().unwrap_or_default(),
                    ));
                }
            }
        }
    }
    out
}

/// A recorded camera visibility ([`mc2_sim::visibility::VisRecord`]) plus the structure LOD, which no oracle trace records
/// (`Visibility::structure_lod` is unavailable in the traces). The replays that need a structure's appearance geometry to be
/// computed (turret and building updates are otherwise explicitly unavailable in view) supply LOD 0, the appearance's base
/// shape, as a TEST INPUT: this is not an oracle value, and tests that care about the LOD pass their own `Visibility`.
pub struct WithBaseLod(pub mc2_sim::visibility::VisRecord);

impl mc2_sim::visibility::Visibility for WithBaseLod {
    fn block_active(&self, b: u32) -> bool {
        self.0.block_active(b)
    }
    fn vertex_active(&self, v: u32) -> bool {
        self.0.vertex_active(v)
    }
    fn mover_in_view(&self, p: mc2_sim::ids::PartId) -> bool {
        self.0.mover_in_view(p)
    }
    fn mover_lod(&self, p: mc2_sim::ids::PartId) -> usize {
        self.0.mover_lod(p)
    }
    fn structure_in_view(&self, h: ObjectHandle) -> bool {
        self.0.structure_in_view(h)
    }
    fn structure_lod(&self, _h: ObjectHandle) -> Option<usize> {
        Some(0)
    }
}
