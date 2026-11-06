//! The terrain query dump: the text format the C++ oracle writes with `--dump-terrain` (`oracle/oracle_main.cpp`,
//! `dumpTerrain`) and the Rust port reproduces line by line, so every query can be diffed bit-exactly (`%.9g`).
//!
//! # File layout (the "golden" file)
//!
//! ```text
//! TH <mission> <side> <blocks> <half> <wums> <tlx> <tly> <tlz> <waterElevation> <shallowDepth> <cellW> <cellH>
//! TE <fnv64 of every vertex (elevation bits, terrainType), row-major> <fnv64 of every cell's data word>
//! HQ <step> <count> <fnv64>      full-map Q lines (whole map, 2 steps of margin)    } hashes of the lines of the
//! HC <count> <fnv64>             full-map C lines (every cell, 1 cell of margin)    } exhaustive dump, which
//! HT <count> <fnv64>             full-map T lines (every tile x 3x3 cells + 4 bad)  } `--terrain-full` writes out
//! WIN <r0> <c0> <nr> <nc> <step>             one section per window of vertices; its V, Q, C and T lines follow
//! V <row> <col> <elevation> <terrainType>
//! Q <x> <y> <elev> <nx> <ny> <nz> <angle> <anx> <any> <anz> f<valid><game><editor> <tR> <tC> <cR> <cC> <ttR> <ttC> <tcR> <tcC> <water>
//! C <row> <col> <data hex|-> <cwx> <cwy> <cpx> <cpy> <cpz> <wR> <wC> <water>
//! T <tileR> <tileC> <cellR> <cellC> <x> <y>|-
//! ```
//!
//! The cell data word is the one stored in the mission pak (the oracle re-reads packets 4..7 instead of looking at
//! `GameMap`, which `Mission::init` has already changed: movers set the mover bit, buildings clear the passable bit),
//! printed and hashed with the mover bit (`0x40`) cleared, as `MissionMap::init` leaves it.
//!
//! `Q`: elevation, normal, angle (and the normal `getTerrainAngle` writes, `- - -` when it writes none), position
//! flags, `worldToTile`, `worldToCell`, `worldToTileCell` and `getWater` of a world position. `C`: cell data word,
//! `cellToWorld`, `getCellPos`, `worldToCell` of the cell centre and the water level there. `T`: `tileCellToWorld`
//! (`-` when it leaves its output untouched).
//!
//! Positions are generated, not stored: a lattice of `step` world units in x and y over a window, then the same lattice
//! shifted by a pseudo random offset in `[0, step)` per axis (x to the right of, y below, the lattice point), from
//! `xorshift32` seeded with 2463534242 at the start of each Q sequence, two draws (x then y) per lattice point in
//! row-major order top to bottom. Window sides that touch the map edge extend two steps beyond the map; other sides stop
//! inside the window so no query reads a vertex outside it.

use super::{Terrain, MAPCELL_DIM};
use crate::math::Vec3;
use std::fmt::Write as _;

/// Step of the lattice in the golden window sections, and the default step of the full-map hashes.
pub const GOLDEN_STEP: i32 = 64;

/// A rectangle of vertices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First vertex row.
    pub r0: i32,
    /// First vertex column.
    pub c0: i32,
    /// Vertex rows.
    pub nr: i32,
    /// Vertex columns.
    pub nc: i32,
}

/// The windows of the golden file for a map `side` vertices across: a small block in the middle and one in the
/// bottom-right corner (the last row and column of quads read nothing and return the out-of-range values).
pub fn golden_windows(side: i32) -> [Window; 2] {
    [
        Window {
            r0: side / 2 - 2,
            c0: side / 2 - 2,
            nr: 5,
            nc: 5,
        },
        Window {
            r0: side - 3,
            c0: side - 3,
            nr: 3,
            nc: 3,
        },
    ]
}

/// The whole map as a window (all four sides touch the edge).
pub fn full_window(side: i32) -> Window {
    Window {
        r0: 0,
        c0: 0,
        nr: side,
        nc: side,
    }
}

/// `%.9g` like the oracle's `F()`: `0` for +-0, `nan` for NaN of either sign (the macOS printf prints no sign).
pub fn f(v: f32) -> String {
    crate::digest::fmt_f32(v)
}

/// FNV-1a 64 (offset basis 1469598103934665603, prime 1099511628211), the hash of the oracle dump.
#[derive(Clone, Copy, Debug)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(1_469_598_103_934_665_603)
    }
}

impl Fnv {
    /// Mix in bytes.
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(1_099_511_628_211);
        }
    }
    /// Mix in a line plus its newline.
    pub fn line(&mut self, s: &str) {
        self.bytes(s.as_bytes());
        self.bytes(b"\n");
    }
}

/// Marsaglia xorshift32.
struct XorShift(u32);

impl XorShift {
    fn new() -> XorShift {
        XorShift(2_463_534_242)
    }
    fn next(&mut self) -> u32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s
    }
    /// Offset in `[0, step)`: `(draw & 0xFFFF) / 65536 * step`, exact in `f32`.
    fn offset(&mut self, step: i32) -> f32 {
        ((self.next() & 0xFFFF) as f32) / 65536.0f32 * (step as f32)
    }
}

/// The `Q` line for one world position.
pub fn q_line(t: &Terrain, p: &Vec3) -> String {
    q_line_with(t, p, &f)
}

/// IEEE bit pattern of an `f32` (8 hex digits): distinguishes `+0`/`-0` and NaN payloads, unlike [`f`].
pub fn fb(v: f32) -> String {
    format!("{:08x}", v.to_bits())
}

/// [`q_line`] with a chosen float formatter.
pub fn q_line_with(t: &Terrain, p: &Vec3, f: &dyn Fn(f32) -> String) -> String {
    let e = t.elevation(p);
    let n = super::mesh::normal(t, p);
    let (a, an) = t.angle_and_normal(p);
    let (tr, tc, _) = t.world_to_tile(p);
    let (cr, cc) = t.world_to_cell(p);
    let (ttr, ttc, tcr, tcc) = t.world_to_tile_cell(p);
    let mut s = String::with_capacity(220);
    let _ = write!(
        s,
        "Q {} {} {} {} {} {} {}",
        f(p.x),
        f(p.y),
        f(e),
        f(n.x),
        f(n.y),
        f(n.z),
        f(a)
    );
    match an {
        Some(v) => {
            let _ = write!(s, " {} {} {}", f(v.x), f(v.y), f(v.z));
        }
        None => s.push_str(" - - -"),
    }
    let _ = write!(
        s,
        " f{}{}{} {tr} {tc} {cr} {cc} {ttr} {ttc} {tcr} {tcc} {}",
        u8::from(t.is_valid_position(p)),
        u8::from(t.is_game_select_position(p)),
        u8::from(t.is_editor_select_position(p)),
        t.get_water(p)
    );
    s
}

/// The `C` line of one cell.
pub fn c_line(t: &Terrain, row: i32, col: i32) -> String {
    c_line_with(t, row, col, &f)
}

/// [`c_line`] with a chosen float formatter.
pub fn c_line_with(t: &Terrain, row: i32, col: i32, f: &dyn Fn(f32) -> String) -> String {
    let data = match t.cell(row, col) {
        Some(c) => format!("{:x}", c.0 & !mc2_path::mission::MAPCELL_MOVER_MASK),
        None => "-".to_string(),
    };
    let cw = t.cell_to_world(row, col);
    let cp = t.get_cell_pos(row, col);
    let (wr, wc) = t.world_to_cell(&cw);
    format!(
        "C {row} {col} {data} {} {} {} {} {} {wr} {wc} {}",
        f(cw.x),
        f(cw.y),
        f(cp.x),
        f(cp.y),
        f(cp.z),
        t.get_water(&cp)
    )
}

/// The `T` line of one tile/cell.
pub fn t_line(t: &Terrain, tr: i32, tc: i32, cr: i32, cc: i32) -> String {
    t_line_with(t, tr, tc, cr, cc, &f)
}

/// [`t_line`] with a chosen float formatter.
pub fn t_line_with(
    t: &Terrain,
    tr: i32,
    tc: i32,
    cr: i32,
    cc: i32,
    f: &dyn Fn(f32) -> String,
) -> String {
    match t.tile_cell_to_world(tr, tc, cr, cc) {
        Some(w) => format!("T {tr} {tc} {cr} {cc} {} {}", f(w.x), f(w.y)),
        None => format!("T {tr} {tc} {cr} {cc} -"),
    }
}

/// Lattice bounds `(x_min, x_max, y_max, y_min)` (inclusive) of a window, in world units.
fn lattice(t: &Terrain, w: &Window, step: i32) -> (i32, i32, i32, i32) {
    let side = t.vertices_per_side;
    let half = side * 64; // worldUnitsMapSide / 2 for 128 units per vertex
    let tile_col = |c: i32| c * 128 - half;
    let tile_row = |r: i32| half - r * 128;
    let x_min = if w.c0 == 0 {
        -half - 2 * step
    } else {
        tile_col(w.c0)
    };
    let x_max = if w.c0 + w.nc == side {
        half + 2 * step
    } else {
        tile_col(w.c0 + w.nc - 1) - step
    };
    let y_max = if w.r0 == 0 {
        half + 2 * step
    } else {
        tile_row(w.r0)
    };
    let y_min = if w.r0 + w.nr == side {
        -half - 2 * step
    } else {
        tile_row(w.r0 + w.nr - 1) + step
    };
    (x_min, x_max, y_max, y_min)
}

/// Every `Q` line of a window: the lattice, then the jittered lattice.
pub fn q_lines(t: &Terrain, w: &Window, step: i32, out: &mut dyn FnMut(&str)) {
    q_lines_with(t, w, step, &f, out)
}

/// [`q_lines`] with a chosen float formatter.
pub fn q_lines_with(
    t: &Terrain,
    w: &Window,
    step: i32,
    f: &dyn Fn(f32) -> String,
    out: &mut dyn FnMut(&str),
) {
    let (x_min, x_max, y_max, y_min) = lattice(t, w, step);
    let nx = (x_max - x_min) / step + 1;
    let ny = (y_max - y_min) / step + 1;
    for pass in 0..2 {
        let mut rng = XorShift::new();
        for j in 0..ny {
            for i in 0..nx {
                let mut x = (x_min + i * step) as f32;
                let mut y = (y_max - j * step) as f32;
                let ox = rng.offset(step);
                let oy = rng.offset(step);
                if pass == 1 {
                    x += ox;
                    y -= oy;
                }
                out(&q_line_with(t, &Vec3 { x, y, z: 0.0 }, f));
            }
        }
    }
}

/// Cell ranges `(row_lo, row_hi, col_lo, col_hi)` (inclusive) a window's `C` lines cover.
fn cell_range(t: &Terrain, w: &Window) -> (i32, i32, i32, i32) {
    let side = t.vertices_per_side;
    let lo = |v0: i32| if v0 == 0 { -1 } else { v0 * MAPCELL_DIM };
    let hi = |v0: i32, n: i32| {
        if v0 + n == side {
            side * MAPCELL_DIM
        } else {
            (v0 + n - 1) * MAPCELL_DIM - 1
        }
    };
    (lo(w.r0), hi(w.r0, w.nr), lo(w.c0), hi(w.c0, w.nc))
}

/// Every `C` line of a window.
pub fn c_lines(t: &Terrain, w: &Window, out: &mut dyn FnMut(&str)) {
    c_lines_with(t, w, &f, out)
}

/// [`c_lines`] with a chosen float formatter.
pub fn c_lines_with(t: &Terrain, w: &Window, f: &dyn Fn(f32) -> String, out: &mut dyn FnMut(&str)) {
    let (r_lo, r_hi, c_lo, c_hi) = cell_range(t, w);
    for r in r_lo..=r_hi {
        for c in c_lo..=c_hi {
            out(&c_line_with(t, r, c, f));
        }
    }
}

/// Every `T` line of a window: all nine cells of every tile the window holds, then four out-of-range requests.
pub fn t_lines(t: &Terrain, w: &Window, out: &mut dyn FnMut(&str)) {
    t_lines_with(t, w, &f, out)
}

/// [`t_lines`] with a chosen float formatter.
pub fn t_lines_with(t: &Terrain, w: &Window, f: &dyn Fn(f32) -> String, out: &mut dyn FnMut(&str)) {
    let side = t.vertices_per_side;
    let tile_hi = |v0: i32, n: i32| if v0 + n == side { side - 1 } else { v0 + n - 2 };
    for tr in w.r0..=tile_hi(w.r0, w.nr) {
        for tc in w.c0..=tile_hi(w.c0, w.nc) {
            for cr in 0..MAPCELL_DIM {
                for cc in 0..MAPCELL_DIM {
                    out(&t_line_with(t, tr, tc, cr, cc, f));
                }
            }
        }
    }
    for (tr, tc, cr, cc) in [
        (w.r0 - 1, w.c0, 0, 0),
        (w.r0, side, 0, 0),
        (w.r0, w.c0, MAPCELL_DIM, 0),
        (w.r0, w.c0, 0, -1),
    ] {
        out(&t_line_with(t, tr, tc, cr, cc, f));
    }
}

/// `(count, hash)` of the lines `gen` emits.
fn hashed(gen: impl FnOnce(&mut dyn FnMut(&str))) -> (u64, u64) {
    let mut h = Fnv::default();
    let mut n = 0u64;
    gen(&mut |l: &str| {
        h.line(l);
        n += 1;
    });
    (n, h.0)
}

/// FNV of all vertices (elevation bits, terrain type) and of all cell data words.
pub fn data_hashes(t: &Terrain) -> (u64, u64) {
    let mut v = Fnv::default();
    for x in t.vertices() {
        v.bytes(&x.elevation.to_bits().to_le_bytes());
        v.bytes(&x.terrain_type.to_le_bytes());
    }
    let mut c = Fnv::default();
    let m = t.map();
    for r in 0..m.height() {
        for col in 0..m.width() {
            c.bytes(&(m.cell(r, col).0 & !mc2_path::mission::MAPCELL_MOVER_MASK).to_le_bytes());
        }
    }
    (v.0, c.0)
}

/// The `TH` line.
pub fn th_line(t: &Terrain, mission: &str) -> String {
    format!(
        "TH {mission} {} {} {} {} {} {} {} {} {} {} {}",
        t.vertices_per_side,
        t.blocks_per_side,
        t.half_vertices_per_side,
        f(t.world_units_map_side),
        f(t.map_top_left.x),
        f(t.map_top_left.y),
        f(t.map_top_left.z),
        f(t.water_elevation),
        f(t.shallow_depth),
        t.map().width(),
        t.map().height()
    )
}

/// One window section: `WIN`, its `V` lines, then `Q`, `C` and `T` lines.
pub fn window_section(t: &Terrain, w: &Window, step: i32) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "WIN {} {} {} {} {step}", w.r0, w.c0, w.nr, w.nc);
    for r in w.r0..w.r0 + w.nr {
        for c in w.c0..w.c0 + w.nc {
            let v = t.vertex(r, c).expect("window inside the map");
            let _ = writeln!(s, "V {r} {c} {} {}", f(v.elevation), v.terrain_type);
        }
    }
    let mut push = |l: &str| {
        s.push_str(l);
        s.push('\n');
    };
    q_lines(t, w, step, &mut push);
    c_lines(t, w, &mut push);
    t_lines(t, w, &mut push);
    s
}

/// `HB` line: `(count, fnv)` of the exhaustive `Q`, `C` and `T` lines with every float written as IEEE bits, so the hash
/// tells `+0` from `-0` and keeps NaN payloads (the `HQ`/`HC`/`HT` hashes cover the oracle's `%.9g` text, which cannot).
/// Only the Rust-own golden carries it; the oracle golden has no such line.
pub fn bits_line(t: &Terrain, full_step: i32) -> String {
    let full = full_window(t.vertices_per_side);
    let (qn, qh) = hashed(|o| q_lines_with(t, &full, full_step, &fb, o));
    let (cn, ch) = hashed(|o| c_lines_with(t, &full, &fb, o));
    let (tn, th) = hashed(|o| t_lines_with(t, &full, &fb, o));
    format!("HB {full_step} {qn} {qh:x} {cn} {ch:x} {tn} {th:x}\n")
}

/// The whole golden file for a loaded mission (what `--dump-terrain` writes).
pub fn golden(t: &Terrain, mission: &str, full_step: i32) -> String {
    let side = t.vertices_per_side;
    let full = full_window(side);
    let (ve, vc) = data_hashes(t);
    let (qn, qh) = hashed(|o| q_lines(t, &full, full_step, o));
    let (cn, ch) = hashed(|o| c_lines(t, &full, o));
    let (tn, th) = hashed(|o| t_lines(t, &full, o));
    let mut s = String::new();
    let _ = writeln!(s, "{}", th_line(t, mission));
    let _ = writeln!(s, "TE {ve:x} {vc:x}");
    let _ = writeln!(s, "HQ {full_step} {qn} {qh:x}");
    let _ = writeln!(s, "HC {cn} {ch:x}");
    let _ = writeln!(s, "HT {tn} {th:x}");
    for w in golden_windows(side) {
        s.push_str(&window_section(t, &w, GOLDEN_STEP));
    }
    s
}

/// Write every full-map `Q`, then `C`, then `T` line (what `--terrain-full` writes next to the golden file).
pub fn write_full(
    t: &Terrain,
    full_step: i32,
    out: &mut dyn std::io::Write,
) -> std::io::Result<()> {
    let full = full_window(t.vertices_per_side);
    let mut err = Ok(());
    let mut emit = |l: &str| {
        if err.is_ok() {
            err = out
                .write_all(l.as_bytes())
                .and_then(|_| out.write_all(b"\n"));
        }
    };
    q_lines(t, &full, full_step, &mut emit);
    c_lines(t, &full, &mut emit);
    t_lines(t, &full, &mut emit);
    err
}

/// Parse the window sections of a golden file back into a terrain holding just those windows' vertices and cells (all
/// else zero), with the header's map size and water. Returns the terrain and the windows with their steps.
pub fn from_golden(text: &str) -> Result<(Terrain, Vec<(Window, i32)>), String> {
    use mc2_path::{MapCell, MissionMap};
    let mut lines = text.lines();
    let th: Vec<&str> = lines
        .next()
        .ok_or("empty golden")?
        .split_whitespace()
        .collect();
    if th.first() != Some(&"TH") || th.len() < 13 {
        return Err("missing TH line".into());
    }
    let side: i32 = th[2].parse().map_err(|e| format!("side: {e}"))?;
    let num = |s: &str| s.parse::<f32>().map_err(|e| format!("{s}: {e}"));
    let n = (side * side) as usize;
    let mut elev = vec![0.0f32; n];
    let mut ttype = vec![0u32; n];
    let cells_side = side * MAPCELL_DIM;
    let mut cells = vec![MapCell(0); (cells_side * cells_side) as usize];
    let mut windows = Vec::new();
    for line in lines {
        let tok: Vec<&str> = line.split_whitespace().collect();
        match tok.first().copied() {
            Some("WIN") if tok.len() == 6 => {
                let p = |i: usize| tok[i].parse::<i32>().map_err(|e| format!("WIN: {e}"));
                windows.push((
                    Window {
                        r0: p(1)?,
                        c0: p(2)?,
                        nr: p(3)?,
                        nc: p(4)?,
                    },
                    p(5)?,
                ));
            }
            Some("V") if tok.len() == 5 => {
                let r: i32 = tok[1].parse().map_err(|e| format!("V row: {e}"))?;
                let c: i32 = tok[2].parse().map_err(|e| format!("V col: {e}"))?;
                let i = (r * side + c) as usize;
                elev[i] = num(tok[3])?;
                ttype[i] = tok[4].parse().map_err(|e| format!("V type: {e}"))?;
            }
            Some("C") if tok.len() == 12 && tok[3] != "-" => {
                let r: i32 = tok[1].parse().map_err(|e| format!("C row: {e}"))?;
                let c: i32 = tok[2].parse().map_err(|e| format!("C col: {e}"))?;
                let d = u32::from_str_radix(tok[3], 16).map_err(|e| format!("C data: {e}"))?;
                cells[(r * cells_side + c) as usize] = MapCell(d);
            }
            _ => {}
        }
    }
    let vertices = elev
        .iter()
        .zip(&ttype)
        .map(|(&e, &tt)| {
            let mut v = super::blank_vertex(e);
            v.terrain_type = tt;
            v
        })
        .collect();
    let mut md = super::empty_move_data(cells_side);
    md.mission =
        MissionMap::from_cells(cells_side, cells_side, cells).map_err(|e| e.to_string())?;
    let mut t = Terrain::from_vertices(side, vertices, Some(md));
    t.water_elevation = num(th[9])?;
    t.shallow_depth = num(th[10])?;
    t.map_top_left.z = num(th[8])?;
    Ok((t, windows))
}
