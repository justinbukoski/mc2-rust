// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The simulation's random number streams.
//!
//! The C++ game has one global stream (`gos_rand`, i.e. the host libc `rand()` masked to 15 bits) used by gameplay *and*
//! by effects, craters, weather, animation start frames and radio chatter. A renderer-less simulation cannot reproduce
//! the visual draws, so the port has two independent streams: [`RngStreams::sim`] (every call site that can change game
//! state) and [`RngStreams::visual`] (everything else). The visual call sites are listed in
//! `oracle/visual_rng_sites.txt` and the oracle rewrites exactly those, so a Rust run and an oracle run draw the same
//! sim numbers.
//!
//! Three algorithms, all returning 15-bit values like `gos_rand`:
//! * [`RngMode::Pcg32`] - the default: PCG32 (O'Neill), top 15 bits. Seeded with `pcg32_srandom(seed, 54)`.
//! * [`RngMode::LegacyMsvc`] - the MSVC `rand()` LCG, what the shipped Windows game used.
//! * [`RngMode::LegacyGlibc`] - glibc `random()` TYPE_3, what alariq's Linux build uses.
//!
//! `tests` pin known output vectors, and the same numbers come out of `oracle/stubs/rng.cpp`.

use std::cell::RefCell;
use std::rc::Rc;

/// Which algorithm backs a stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RngMode {
    /// PCG32, top 15 bits.
    #[default]
    Pcg32,
    /// MSVC `rand()`: `s = s * 214013 + 2531011; (s >> 16) & 0x7fff`.
    LegacyMsvc,
    /// glibc `random()` additive feedback generator; `rand() % 32768` like the original `gos_rand`.
    LegacyGlibc,
}

impl RngMode {
    /// The code the oracle prints in the digest header (`--rng msvc|glibc|pcg` -> 0, 1, 2).
    pub fn oracle_code(self) -> u8 {
        match self {
            RngMode::LegacyMsvc => 0,
            RngMode::LegacyGlibc => 1,
            RngMode::Pcg32 => 2,
        }
    }
}

const GLIBC_RING: usize = 34;

/// One deterministic stream.
#[derive(Clone, Debug)]
pub struct SimRng {
    mode: RngMode,
    msvc: u32,
    /// glibc: element k of the sequence lives at `ring[k % 34]`; `glibc_k` is the next k to generate.
    ring: [i32; GLIBC_RING],
    glibc_k: usize,
    pcg_state: u64,
    pcg_inc: u64,
    draws: u64,
    /// Diagnostic draw trace (`MC2_RNG_TRACE`): only the sim stream is traced; `trace_frame` is the sim turn, set by `Sim::step`.
    traced: bool,
    trace_frame: i64,
}

impl SimRng {
    /// New stream in `mode`, seeded with `seed` (the argument of `gos_srand`).
    pub fn new(mode: RngMode, seed: u32) -> SimRng {
        let mut r = SimRng {
            mode,
            msvc: 0,
            ring: [0; GLIBC_RING],
            glibc_k: 0,
            pcg_state: 0,
            pcg_inc: 0,
            draws: 0,
            traced: false,
            trace_frame: 0,
        };
        r.seed(seed);
        r
    }

    /// `gos_srand(seed)`: reseed (resets the draw counter).
    pub fn seed(&mut self, seed: u32) {
        self.msvc = seed;
        // glibc srandom_r: r[0] = seed (0 -> 1), r[i] = 16807 * r[i-1] % 2147483647 via Schrage, then 310 discards.
        let s = if seed == 0 { 1 } else { seed };
        let mut r = vec![0i32; GLIBC_RING + 310];
        r[0] = s as i32;
        for i in 1..31 {
            let prev = r[i - 1] as i64;
            let (hi, lo) = (prev / 127773, prev % 127773);
            let mut word = 16807 * lo - 2836 * hi;
            if word < 0 {
                word += 2147483647;
            }
            r[i] = word as i32;
        }
        for i in 31..34 {
            r[i] = r[i - 31];
        }
        for i in 34..(34 + 310) {
            r[i] = (r[i - 31] as u32).wrapping_add(r[i - 3] as u32) as i32;
        }
        let total = GLIBC_RING + 310;
        for (k, v) in r.iter().enumerate().skip(total - GLIBC_RING) {
            self.ring[k % GLIBC_RING] = *v;
        }
        self.glibc_k = total;
        // PCG32 reference seeding.
        self.pcg_state = 0;
        self.pcg_inc = (54u64 << 1) | 1;
        self.pcg_step();
        self.pcg_state = self.pcg_state.wrapping_add(seed as u64);
        self.pcg_step();
        self.draws = 0;
    }

    fn pcg_step(&mut self) -> u32 {
        let old = self.pcg_state;
        self.pcg_state = old
            .wrapping_mul(6364136223846793005)
            .wrapping_add(self.pcg_inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// `gos_rand()`: a value in `0..32768`.
    pub fn next15(&mut self) -> i32 {
        self.draws += 1;
        let v = match self.mode {
            RngMode::LegacyMsvc => {
                self.msvc = self.msvc.wrapping_mul(214013).wrapping_add(2531011);
                ((self.msvc >> 16) & 0x7fff) as i32
            }
            RngMode::LegacyGlibc => {
                let k = self.glibc_k;
                let o = (self.ring[(k - 31) % GLIBC_RING] as u32)
                    .wrapping_add(self.ring[(k - 3) % GLIBC_RING] as u32);
                self.ring[k % GLIBC_RING] = o as i32;
                self.glibc_k += 1;
                ((o >> 1) % 32768) as i32
            }
            RngMode::Pcg32 => (self.pcg_step() >> 17) as i32,
        };
        if self.traced {
            trace_draw(self.draws, self.trace_frame, v);
        }
        v
    }

    /// Set the turn that `MC2_RNG_TRACE` rows carry (diagnostic only).
    pub fn set_trace_frame(&mut self, frame: i64) {
        self.trace_frame = frame;
    }

    /// `RandomNumber(range)` (`mclib/routines.cpp`): `gos_rand() % range`. The C++ divides by zero for `range == 0`
    /// (see `Mission::calcComplexDropZones`); here that returns 0 (a draw is still consumed). A negative range behaves
    /// like C's `%`: the result is non-negative.
    pub fn random_number(&mut self, range: i32) -> i32 {
        let r = self.next15();
        if range == 0 {
            0
        } else {
            (r as i64 % (range as i64).abs()) as i32
        }
    }

    /// `RollDice(percent)`: `((gos_rand() * 100) >> 15) < percent`.
    pub fn roll_dice(&mut self, percent: i32) -> bool {
        ((self.next15() * 100) >> 15) < percent
    }

    /// Number of draws since the last seed (appears in digests).
    pub fn draws(&self) -> u64 {
        self.draws
    }

    /// Cheap fingerprint of the generator state (appears in digests; same definition as the oracle).
    pub fn state_hash(&self) -> u64 {
        match self.mode {
            RngMode::LegacyMsvc => self.msvc as u64,
            RngMode::LegacyGlibc => (self.ring[(self.glibc_k - 1) % GLIBC_RING] as u32) as u64,
            RngMode::Pcg32 => self.pcg_state,
        }
    }

    /// The algorithm in use.
    pub fn mode(&self) -> RngMode {
        self.mode
    }
}

/// Seed offset of the visual stream relative to the sim stream (same constant as `oracle_seed_visual`).
pub const VISUAL_SEED_XOR: u32 = 0xA5A5_A5A5;

/// The two streams of a [`World`](crate::World).
#[derive(Clone, Debug)]
pub struct RngStreams {
    /// Gameplay stream: hit rolls, criticals, AI dice, turret idle moves, squad alternatives...
    pub sim: SimRng,
    /// Presentation-only stream: effects, craters, debris, weather, animation start frames, radio chatter.
    pub visual: SimRng,
}

impl RngStreams {
    /// Seed both streams from the mission seed.
    pub fn new(mode: RngMode, seed: u32) -> RngStreams {
        RngStreams {
            sim: SimRng {
                traced: true,
                ..SimRng::new(mode, seed)
            },
            visual: SimRng::new(mode, seed ^ VISUAL_SEED_XOR),
        }
    }
}

/// A handle to the sim stream that can also be given to the ABL environment, whose `random(n)` / `seedRandom(n)` natives
/// draw from `RandomNumber` / `gos_srand` in the C++ (`code/ablmc2.cpp`).
#[derive(Clone, Debug)]
pub struct SharedSimRng(pub Rc<RefCell<RngStreams>>);

impl SharedSimRng {
    /// Wrap the streams.
    pub fn new(streams: RngStreams) -> SharedSimRng {
        SharedSimRng(Rc::new(RefCell::new(streams)))
    }
}

impl mc2_abl::AblRng for SharedSimRng {
    fn random(&mut self, range: i32) -> i32 {
        // The C++ ABL callback is `RandomNumber(range)` with no guard (ablxstd.cpp execStdRandom): range 0 is the pinned
        // "consume one draw, return 0" case, a negative range behaves like C `%`.
        self.0.borrow_mut().sim.random_number(range)
    }
    fn seed(&mut self, seed: u32) {
        self.0.borrow_mut().sim.seed(seed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(mode: RngMode, seed: u32, n: usize) -> Vec<i32> {
        let mut r = SimRng::new(mode, seed);
        (0..n).map(|_| r.next15()).collect()
    }

    #[test]
    fn msvc_matches_known_sequence() {
        // MSVC srand(1); rand() -> 41, 18467, 6334, 26500
        assert_eq!(
            take(RngMode::LegacyMsvc, 1, 4),
            vec![41, 18467, 6334, 26500]
        );
    }

    #[test]
    fn glibc_matches_known_sequence() {
        // glibc srand(1); rand() -> 1804289383, 846930886, 1681692777, 1714636915 (then % 32768)
        let want: Vec<i32> = [1804289383u32, 846930886, 1681692777, 1714636915]
            .iter()
            .map(|v| (v % 32768) as i32)
            .collect();
        assert_eq!(take(RngMode::LegacyGlibc, 1, 4), want);
        // long run crosses the ring many times; compare against the closed form
        let mut r = SimRng::new(RngMode::LegacyGlibc, 12345);
        // reference implementation with a flat vector
        let mut flat = vec![0i32; 344];
        flat[0] = 12345;
        for i in 1..31 {
            let prev = flat[i - 1] as i64;
            let mut w = 16807 * (prev % 127773) - 2836 * (prev / 127773);
            if w < 0 {
                w += 2147483647;
            }
            flat[i] = w as i32;
        }
        for i in 31..34 {
            flat[i] = flat[i - 31];
        }
        for i in 34..344 {
            flat[i] = (flat[i - 31] as u32).wrapping_add(flat[i - 3] as u32) as i32;
        }
        for _ in 0..1000 {
            let i = flat.len();
            let o = (flat[i - 31] as u32).wrapping_add(flat[i - 3] as u32);
            flat.push(o as i32);
            assert_eq!(r.next15(), ((o >> 1) % 32768) as i32);
        }
    }

    #[test]
    fn pcg_matches_reference_vector() {
        // pcg32_srandom_r(&rng, 42u, 54u): 0xa15c02b7 0x7b47f409 0xba1d3330 0x83d2f293 (top 15 bits here)
        let want: Vec<i32> = [0xa15c02b7u32, 0x7b47f409, 0xba1d3330, 0x83d2f293]
            .iter()
            .map(|v| (v >> 17) as i32)
            .collect();
        assert_eq!(take(RngMode::Pcg32, 42, 4), want);
    }

    #[test]
    fn random_number_and_dice() {
        let mut a = SimRng::new(RngMode::LegacyMsvc, 1);
        assert_eq!(a.random_number(100), 41);
        assert_eq!(a.random_number(0), 0); // C++ would trap; we consume a draw and return 0
        assert_eq!(a.draws(), 2);
        let mut b = SimRng::new(RngMode::LegacyMsvc, 1);
        // 41 * 100 >> 15 == 0 < 1
        assert!(b.roll_dice(1));
    }

    #[test]
    fn streams_are_independent() {
        let mut s = RngStreams::new(RngMode::Pcg32, 7);
        let a = s.sim.next15();
        let _ = s.visual.next15();
        let mut t = RngStreams::new(RngMode::Pcg32, 7);
        assert_eq!(a, t.sim.next15());
        assert_eq!(s.sim.draws(), 1);
    }
}

impl SimRng {
    /// Rebuilds a stream from its [`SimRng::state_hash`] where the hash is the whole generator state (MSVC LCG: the seed
    /// word; PCG32: the state, with the fixed stream increment of [`SimRng::seed`]), `None` for glibc (34-word ring).
    /// The draw counter is set to `draws`. Used to replay oracle traces from the middle of a run (lane F).
    pub fn from_state_hash(mode: RngMode, hash: u64, draws: u64) -> Option<SimRng> {
        let mut r = SimRng::new(mode, 0);
        match mode {
            RngMode::LegacyMsvc => r.msvc = hash as u32,
            RngMode::Pcg32 => r.pcg_state = hash,
            RngMode::LegacyGlibc => return None,
        }
        r.draws = draws;
        Some(r)
    }
}

#[cfg(test)]
mod from_state_tests {
    use super::*;

    #[test]
    fn rebuilt_streams_continue_identically() {
        for mode in [RngMode::LegacyMsvc, RngMode::Pcg32] {
            let mut a = SimRng::new(mode, 1234);
            for _ in 0..17 {
                a.next15();
            }
            let mut b = SimRng::from_state_hash(mode, a.state_hash(), a.draws()).unwrap();
            for _ in 0..50 {
                assert_eq!(a.next15(), b.next15());
            }
            assert_eq!(a.draws(), b.draws());
        }
        assert!(SimRng::from_state_hash(RngMode::LegacyGlibc, 0, 0).is_none());
    }
}

impl SimRng {
    /// Rebuilds a glibc stream from its last 34 outputs, oldest first (`oracle_rng_glibc_ring`, the generator's whole
    /// state). The draw counter is set to `draws`.
    pub fn from_glibc_ring(ring: &[i32; GLIBC_RING], draws: u64) -> SimRng {
        let mut r = SimRng::new(RngMode::LegacyGlibc, 0);
        r.ring = *ring;
        r.glibc_k = 2 * GLIBC_RING;
        r.draws = draws;
        r
    }
}

#[cfg(test)]
mod from_glibc_ring_tests {
    use super::*;

    #[test]
    fn rebuilt_glibc_stream_continues_identically() {
        let mut a = SimRng::new(RngMode::LegacyGlibc, 99);
        let mut last = Vec::new();
        for _ in 0..40 {
            a.next15();
            last.push(a.ring[(a.glibc_k - 1) % GLIBC_RING]);
        }
        let ring: [i32; GLIBC_RING] = last[last.len() - GLIBC_RING..].try_into().unwrap();
        let mut b = SimRng::from_glibc_ring(&ring, a.draws());
        for _ in 0..50 {
            assert_eq!(a.next15(), b.next15());
        }
    }
}

// ---- save/load (DESIGN.md section 9): both streams are dynamic; the algorithm is part of the state ----

impl crate::save::Snapshot for RngMode {
    fn save(&self, w: &mut crate::save::Writer) {
        w.u8(match self {
            RngMode::Pcg32 => 0,
            RngMode::LegacyMsvc => 1,
            RngMode::LegacyGlibc => 2,
        });
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        match r.u8()? {
            0 => Ok(RngMode::Pcg32),
            1 => Ok(RngMode::LegacyMsvc),
            2 => Ok(RngMode::LegacyGlibc),
            t => Err(crate::save::SaveError(format!(
                "save: unknown RNG mode {t}"
            ))),
        }
    }
}

crate::snapshot_struct!(SimRng {
    mode,
    msvc,
    ring,
    glibc_k,
    pcg_state,
    pcg_inc,
    draws,
    traced => false,
    trace_frame => 0,
});

crate::snapshot_struct!(RngStreams { sim, visual });

impl crate::save::Snapshot for SharedSimRng {
    fn save(&self, w: &mut crate::save::Writer) {
        self.0.borrow().save(w);
    }
    fn load(r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<Self> {
        Ok(SharedSimRng::new(RngStreams::load(r)?))
    }
    /// Overlays the streams in place, so the ABL environment that shares this `Rc` keeps seeing the same stream.
    fn load_into(&mut self, r: &mut crate::save::Reader<'_>) -> crate::save::SaveResult<()> {
        self.0.borrow_mut().load_into(r)
    }
}

/// `MC2_RNG_TRACE=<path>` (env, off by default): one row per sim-stream draw, `D <draw#> t<turn> v<value> <Rust call sites>`, the
/// twin of the scratch oracle's `ORACLE_DRAW_TRACE` rows. `MC2_RNG_TRACE_FROM=<turn>` skips the (slow) call-site capture before that
/// turn. Read-only diagnostic: it never changes a draw.
fn trace_draw(idx: u64, frame: i64, value: i32) {
    use std::io::Write;
    type Sink = (std::sync::Mutex<std::fs::File>, i64);
    static OUT: std::sync::OnceLock<Option<Sink>> = std::sync::OnceLock::new();
    let out = OUT.get_or_init(|| {
        let from = std::env::var("MC2_RNG_TRACE_FROM")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        std::env::var("MC2_RNG_TRACE")
            .ok()
            .and_then(|p| std::fs::File::create(p).ok())
            .map(|f| (std::sync::Mutex::new(f), from))
    });
    let Some((f, from)) = out else { return };
    let mut sites = Vec::new();
    if frame >= *from {
        let bt = std::backtrace::Backtrace::force_capture().to_string();
        // frames print as `N: symbol` then (with debug info) `at path:line`; keep the mc2_sim symbols, with the location when present
        let mut keep = false;
        for l in bt.lines() {
            let l = l.trim();
            if let Some((n, sym)) = l.split_once(": ") {
                if n.bytes().all(|b| b.is_ascii_digit()) {
                    keep = sym.starts_with("mc2_sim::") && !sym.starts_with("mc2_sim::rng::");
                    if keep {
                        let base = match sym.rfind("::h") {
                            Some(i) if sym.len() - i == 19 => &sym[..i],
                            _ => sym,
                        };
                        sites.push(base.to_string());
                    }
                    continue;
                }
            }
            if let (true, Some(at), Some(last)) = (keep, l.strip_prefix("at "), sites.last_mut()) {
                last.push_str(&format!(
                    " @{}",
                    at.rsplit("mc2-sim/src/").next().unwrap_or(at)
                ));
                keep = false;
            }
            if sites.len() >= 5 {
                break;
            }
        }
    }
    let _ = writeln!(
        f.lock().unwrap(),
        "D {idx} t{frame} v{value} {}",
        sites.join(" < ")
    );
}
