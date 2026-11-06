// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Same-pose comparison of the instrumented C++ vertex dump with the port's (rc-vqa-vertexdump).
//!
//! The C++ preview draws its mech animated (gesture 2) and the port draws the bind pose, so their dumps cannot be
//! compared vertex by vertex as they are. This puts both lightings on the C++ pose:
//!
//! - [`relight_cpp_normals`]: every C++ vertex's world normal and `aRGBLight`, lit by the port's inputs and
//!   formula ([`shape_lit`]), against the C++ final argb;
//! - [`port_mesh_in_cpp_pose`]: the port mesh's own local normals, posed by the C++ `shapeToWorld` of the same
//!   node, lit the same way, against the C++ vertex with the same node, local position and local normal;
//! - [`port_bind_pose`]: the port's dumped argb (bind pose, as drawn) against the same C++ vertices, for scale.
//!
//! [`compare_inputs`] lists the per-shape inputs first (light 0 colour and direction, ambient, highlight,
//! `BaseVertexColor`, fog, night), so the report names the first field that differs.

use std::collections::HashMap;

use crate::math::{stuff_to_world, world_to_stuff, Vec3};
use crate::objects::GlowColors;
use crate::vertex_light::{lit_argb, shape_lit, VertexLightInputs};

/// One `V` line of the C++ dump (tgl.cpp `MultiTransformShape`, cpp-vertex-dump.patch).
#[derive(Clone, Debug, PartialEq)]
pub struct CppVertex {
    pub shape: String,
    pub index: usize,
    pub pos: Vec3,
    pub nrm: Vec3,
    /// `nrm` times the shape's `shapeToWorld` (Stuff frame, not normalised).
    pub wnrm: Vec3,
    pub vlight: u32,
    pub argb: u32,
}

/// One `S` line of the C++ dump and its light 0 / light 1 lines.
#[derive(Clone, Debug, PartialEq)]
pub struct CppShape {
    pub name: String,
    pub light0_argb: u32,
    pub light0_intensity: f32,
    /// Light 0's travel direction, Stuff world frame.
    pub light0_world: Vec3,
    pub ambient_argb: u32,
    pub highlight: u32,
    pub base: u32,
    pub fog: u32,
    pub is_night: bool,
    pub night_factor: f32,
}

/// The mech section (the first `M` block) of a C++ dump.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CppDump {
    pub shapes: Vec<CppShape>,
    pub vertices: Vec<CppVertex>,
    /// `shapeToWorld` rotation per node, `m(row, col)`: Stuff row vectors, `w = n * m`.
    pub nodes: HashMap<String, [[f32; 3]; 3]>,
}

/// One `V` line of the port dump ([`crate::vertex_light::write_dump`]).
#[derive(Clone, Debug, PartialEq)]
pub struct PortVertex {
    pub node: String,
    pub pos: Vec3,
    pub nrm: Vec3,
    pub vlight: u32,
    pub argb: u32,
}

/// A port dump: its inputs (the `R` line) and vertices.
#[derive(Clone, Debug, PartialEq)]
pub struct PortDump {
    pub shape: String,
    pub inputs: VertexLightInputs,
    pub vertices: Vec<PortVertex>,
}

fn field<'a>(t: &'a [&'a str], key: &str, n: usize) -> Result<&'a [&'a str], String> {
    let i = t
        .iter()
        .position(|s| *s == key)
        .ok_or_else(|| format!("no `{key}` in `{}`", t.join(" ")))?;
    t.get(i + 1..i + 1 + n)
        .ok_or_else(|| format!("`{key}` short in `{}`", t.join(" ")))
}

fn f(s: &str) -> Result<f32, String> {
    s.parse().map_err(|_| format!("bad number `{s}`"))
}

fn hex(s: &str) -> Result<u32, String> {
    u32::from_str_radix(s, 16).map_err(|_| format!("bad hex `{s}`"))
}

fn v3(t: &[&str], key: &str) -> Result<Vec3, String> {
    let v = field(t, key, 3)?;
    Ok(Vec3::new(f(v[0])?, f(v[1])?, f(v[2])?))
}

/// Parses the mech section of a C++ dump: everything before the second `M` line (the contact-marker shapes
/// after it are not the mech).
pub fn parse_cpp(text: &str) -> Result<CppDump, String> {
    let mut d = CppDump::default();
    let mut models = 0;
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        match t.first().copied() {
            Some("M") => {
                models += 1;
                if models > 1 {
                    break;
                }
            }
            Some("N") => {
                let m = field(&t, "m", 9)?;
                let mut r = [[0.0; 3]; 3];
                for (k, s) in m.iter().enumerate() {
                    r[k / 3][k % 3] = f(s)?;
                }
                d.nodes.insert(t[1].to_string(), r);
            }
            Some("S") => d.shapes.push(CppShape {
                name: t[1].to_string(),
                light0_argb: 0,
                light0_intensity: 0.0,
                light0_world: Vec3::ZERO,
                ambient_argb: 0,
                highlight: hex(field(&t, "highlight", 1)?[0])?,
                base: hex(field(&t, "base", 1)?[0])?,
                fog: hex(field(&t, "fog", 1)?[0])?,
                is_night: field(&t, "isNight", 1)?[0] != "0",
                night_factor: f(field(&t, "nightFactor", 1)?[0])?,
            }),
            Some("L") if t.get(2) != Some(&"null") => {
                let s = d.shapes.last_mut().ok_or("L line before any S line")?;
                let argb = hex(field(&t, "argb", 1)?[0])?;
                match t[1] {
                    "0" => {
                        s.light0_argb = argb;
                        s.light0_intensity = f(field(&t, "intensity", 1)?[0])?;
                        s.light0_world = v3(&t, "world")?;
                    }
                    "1" => s.ambient_argb = argb,
                    _ => {}
                }
            }
            Some("V") => d.vertices.push(CppVertex {
                shape: t[1].to_string(),
                index: t[2].parse().map_err(|_| format!("bad index in `{line}`"))?,
                pos: v3(&t, "pos")?,
                nrm: v3(&t, "nrm")?,
                wnrm: v3(&t, "wnrm")?,
                vlight: hex(field(&t, "vlight", 1)?[0])?,
                argb: hex(field(&t, "argb", 1)?[0])?,
            }),
            _ => {}
        }
    }
    Ok(d)
}

/// Parses a port dump. The marker glow colours are not in the dump; the defaults are used (a mech has none).
pub fn parse_port(text: &str) -> Result<PortDump, String> {
    let mut lines = text.lines();
    let head = lines.next().ok_or("empty port dump")?;
    let t: Vec<&str> = head.split_whitespace().collect();
    if t.first() != Some(&"R") {
        return Err(format!("port dump starts `{head}`, not `R`"));
    }
    let rgb = |key| -> Result<[f32; 3], String> {
        let v = field(&t, key, 3)?;
        Ok([f(v[0])?, f(v[1])?, f(v[2])?])
    };
    let night = field(&t, "night", 2)?;
    // the dump writes the travel direction in the Stuff frame; `shape_dir` points at the sun, world frame
    let travel = v3(&t, "lightdir_stuff")?;
    let inputs = VertexLightInputs {
        shape_dir: -stuff_to_world().transform_vector(travel),
        infinite: rgb("infinite")?,
        ambient: rgb("ambient")?,
        highlight: hex(field(&t, "highlight", 1)?[0])?,
        night_factor: f(night[1])?,
        is_night: night[0] != "0",
        glow: GlowColors::default(),
    };
    let mut vertices = Vec::new();
    for line in lines {
        let t: Vec<&str> = line.split_whitespace().collect();
        if t.first() == Some(&"V") {
            vertices.push(PortVertex {
                node: t[1].to_string(),
                pos: v3(&t, "pos")?,
                nrm: v3(&t, "nrm")?,
                vlight: hex(field(&t, "vlight", 1)?[0])?,
                argb: hex(field(&t, "argb", 1)?[0])?,
            });
        }
    }
    Ok(PortDump {
        shape: field(&t, "shape", 1)?[0].to_string(),
        inputs,
        vertices,
    })
}

/// One input field, C++ against port.
#[derive(Clone, Debug, PartialEq)]
pub struct InputCheck {
    pub field: &'static str,
    pub cpp: String,
    pub port: String,
    pub equal: bool,
}

fn bytes(c: [f32; 3]) -> [u32; 3] {
    c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32)
}

/// One field of a C++ shape: its value as written and whether it equals the port's.
type ShapeCheck<'a> = Box<dyn Fn(&CppShape) -> (String, bool) + 'a>;

fn rgb_bytes(argb: u32) -> [u32; 3] {
    [(argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff]
}

/// The inputs of every C++ shape against the port's (one row per field; a field is equal only when it is equal
/// on every shape, and the row shows the first shape that differs).
pub fn compare_inputs(cpp: &CppDump, port: &PortDump) -> Vec<InputCheck> {
    let p = &port.inputs;
    let port_dir = world_to_stuff().transform_vector(-p.shape_dir);
    let rows: [(&'static str, String, ShapeCheck); 8] = [
        (
            "light 0 colour (sun to shapes, bytes)",
            format!("{:?}", bytes(p.infinite)),
            Box::new(|s| {
                let c = rgb_bytes(s.light0_argb)
                    .map(|v| (v as f32 * s.light0_intensity).round() as u32);
                (format!("{c:?}"), c == bytes(p.infinite))
            }),
        ),
        (
            "light 0 direction (travel, Stuff world)",
            format!("{:.6} {:.6} {:.6}", port_dir.x, port_dir.y, port_dir.z),
            Box::new(|s| {
                let d = s.light0_world - port_dir;
                (
                    format!(
                        "{:.6} {:.6} {:.6}",
                        s.light0_world.x, s.light0_world.y, s.light0_world.z
                    ),
                    d.dot(d).sqrt() < 1.0e-4,
                )
            }),
        ),
        (
            "ambient (bytes)",
            format!("{:?}", bytes(p.ambient)),
            Box::new(|s| {
                let c = rgb_bytes(s.ambient_argb);
                (format!("{c:?}"), c == bytes(p.ambient))
            }),
        ),
        (
            "highlight",
            format!("{:08x}", p.highlight),
            Box::new(|s| (format!("{:08x}", s.highlight), s.highlight == p.highlight)),
        ),
        // the port's shapes have no BaseVertexColor (the preview's appearance never sets one)
        (
            "BaseVertexColor",
            "00000000".into(),
            Box::new(|s| (format!("{:08x}", s.base), s.base == 0)),
        ),
        // the preview draws without fog (`SimpleCamera::render` sets `useFog = 0`); 0xffffffff is no fog
        (
            "fog",
            "ffffffff (off)".into(),
            Box::new(|s| (format!("{:08x}", s.fog), s.fog == 0xffff_ffff)),
        ),
        (
            "isNight",
            format!("{}", p.is_night as u8),
            Box::new(|s| (format!("{}", s.is_night as u8), s.is_night == p.is_night)),
        ),
        (
            "nightFactor",
            format!("{:.6}", p.night_factor),
            Box::new(|s| {
                (
                    format!("{:.6}", s.night_factor),
                    (s.night_factor - p.night_factor).abs() < 1.0e-5,
                )
            }),
        ),
    ];
    rows.into_iter()
        .map(|(field, port, check)| {
            let all: Vec<(String, bool)> = cpp.shapes.iter().map(&check).collect();
            let shown = all
                .iter()
                .find(|(_, eq)| !eq)
                .or(all.first())
                .map_or_else(|| "(no shapes)".to_string(), |(v, _)| v.clone());
            InputCheck {
                field,
                cpp: shown,
                port,
                equal: !all.is_empty() && all.iter().all(|(_, eq)| *eq),
            }
        })
        .collect()
}

/// The vertex comparison of one method.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VertexCheck {
    pub compared: usize,
    pub within: usize,
    /// Largest per-channel byte difference.
    pub max_diff: u32,
    /// Mean lit bytes: C++ and port.
    pub cpp_mean: [f64; 3],
    pub port_mean: [f64; 3],
    /// The first vertex (C++ order) outside the tolerance.
    pub first: Option<String>,
}

fn diff(a: u32, b: u32) -> u32 {
    let (a, b) = (rgb_bytes(a), rgb_bytes(b));
    (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0)
}

#[derive(Default)]
struct Acc {
    check: VertexCheck,
    cpp_sum: [u64; 3],
    port_sum: [u64; 3],
}

impl Acc {
    fn add(&mut self, cv: &CppVertex, port_argb: u32, tol: u32, note: impl FnOnce() -> String) {
        let d = diff(cv.argb, port_argb);
        let c = &mut self.check;
        c.compared += 1;
        c.max_diff = c.max_diff.max(d);
        if d <= tol {
            c.within += 1;
        } else if c.first.is_none() {
            c.first = Some(format!(
                "{} {}: C++ argb {:08x}, port {:08x} ({})",
                cv.shape,
                cv.index,
                cv.argb,
                port_argb,
                note()
            ));
        }
        for (k, (a, b)) in rgb_bytes(cv.argb)
            .into_iter()
            .zip(rgb_bytes(port_argb))
            .enumerate()
        {
            self.cpp_sum[k] += a as u64;
            self.port_sum[k] += b as u64;
        }
    }

    fn done(mut self) -> VertexCheck {
        let n = self.check.compared.max(1) as f64;
        self.check.cpp_mean = self.cpp_sum.map(|s| s as f64 / n);
        self.check.port_mean = self.port_sum.map(|s| s as f64 / n);
        self.check
    }
}

/// Every C++ vertex, lit by the port's inputs and formula at the C++ world normal (the C++ pose).
pub fn relight_cpp_normals(cpp: &CppDump, inputs: &VertexLightInputs, tol: u32) -> VertexCheck {
    let to_world = stuff_to_world();
    let mut acc = Acc::default();
    for cv in &cpp.vertices {
        let lit = shape_lit(to_world.transform_vector(cv.wnrm), cv.vlight, inputs);
        acc.add(cv, lit_argb(lit), tol, || {
            let n = cv.wnrm.normalized();
            format!("wnrm {:.6} {:.6} {:.6}", n.x, n.y, n.z)
        });
    }
    acc.done()
}

fn key(node: &str, pos: Vec3, nrm: Vec3) -> String {
    format!(
        "{node} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3}",
        pos.x, pos.y, pos.z, nrm.x, nrm.y, nrm.z
    )
}

/// `n * m` with Stuff's row vectors (`Vector3D::Multiply(v, LinearMatrix4D)`).
pub fn row_times(n: Vec3, m: &[[f32; 3]; 3]) -> Vec3 {
    Vec3::new(
        n.x * m[0][0] + n.y * m[1][0] + n.z * m[2][0],
        n.x * m[0][1] + n.y * m[1][1] + n.z * m[2][1],
        n.x * m[0][2] + n.y * m[1][2] + n.z * m[2][2],
    )
}

/// The first port vertex for each node + local position + local normal (the port mesh is per triangle corner).
fn port_by_key(port: &PortDump) -> HashMap<String, &PortVertex> {
    let mut m = HashMap::new();
    for pv in &port.vertices {
        m.entry(key(&pv.node, pv.pos, pv.nrm)).or_insert(pv);
    }
    m
}

/// The port mesh's local normals posed by the C++ node matrices, lit by the port, against the C++ vertices with
/// the same node, local position and local normal. Also returns the largest gap between the port's posed normal
/// and the C++ world normal (the pose check).
pub fn port_mesh_in_cpp_pose(cpp: &CppDump, port: &PortDump, tol: u32) -> (VertexCheck, f32) {
    let to_world = stuff_to_world();
    let by_key = port_by_key(port);
    let mut acc = Acc::default();
    let mut pose_gap: f32 = 0.0;
    for cv in &cpp.vertices {
        let (Some(pv), Some(m)) = (
            by_key.get(&key(&cv.shape, cv.pos, cv.nrm)),
            cpp.nodes.get(&cv.shape),
        ) else {
            continue;
        };
        let wn = row_times(pv.nrm, m);
        let g = wn - cv.wnrm;
        pose_gap = pose_gap.max(g.dot(g).sqrt());
        let lit = shape_lit(to_world.transform_vector(wn), pv.vlight, &port.inputs);
        acc.add(cv, lit_argb(lit), tol, || {
            format!("vlight C++ {:08x} port {:08x}", cv.vlight, pv.vlight)
        });
    }
    (acc.done(), pose_gap)
}

/// The port's argb as dumped (its bind pose) against the same shared C++ vertices.
pub fn port_bind_pose(cpp: &CppDump, port: &PortDump, tol: u32) -> VertexCheck {
    let by_key = port_by_key(port);
    let mut acc = Acc::default();
    for cv in &cpp.vertices {
        if let Some(pv) = by_key.get(&key(&cv.shape, cv.pos, cv.nrm)) {
            acc.add(cv, pv.argb, tol, || "bind pose".into());
        }
    }
    acc.done()
}

/// The report the `vertex_light_compare` example prints.
pub fn report(cpp: &CppDump, port: &PortDump, tol: u32) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "port shape {}; C++ mech section: {} shapes, {} vertices; tolerance {tol} per channel byte",
        port.shape,
        cpp.shapes.len(),
        cpp.vertices.len()
    );
    let inputs = compare_inputs(cpp, port);
    for r in &inputs {
        let _ = writeln!(
            s,
            "input {:<40} C++ {:<28} port {:<28} {}",
            r.field,
            r.cpp,
            r.port,
            if r.equal { "equal" } else { "DIFFERS" }
        );
    }
    match inputs.iter().find(|r| !r.equal) {
        Some(r) => {
            let _ = writeln!(s, "first differing input: {}", r.field);
        }
        None => {
            let _ = writeln!(s, "first differing input: none");
        }
    }
    let mut row = |label: &str, c: &VertexCheck| {
        let _ = writeln!(
            s,
            "{label}: {}/{} within, max diff {}, mean C++ {:.1}/{:.1}/{:.1} port {:.1}/{:.1}/{:.1}; first outside: {}",
            c.within,
            c.compared,
            c.max_diff,
            c.cpp_mean[0],
            c.cpp_mean[1],
            c.cpp_mean[2],
            c.port_mean[0],
            c.port_mean[1],
            c.port_mean[2],
            c.first.as_deref().unwrap_or("none")
        );
    };
    row(
        "C++ pose, C++ normals, port light",
        &relight_cpp_normals(cpp, &port.inputs, tol),
    );
    let (posed, gap) = port_mesh_in_cpp_pose(cpp, port, tol);
    row("C++ pose, port mesh normals, port light", &posed);
    row(
        "port bind pose as dumped (shared vertices)",
        &port_bind_pose(cpp, port, tol),
    );
    let _ = writeln!(
        s,
        "pose check: largest |port normal * C++ shapeToWorld - C++ wnrm| = {gap:.6}"
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // A C++ shape posed by a quarter turn about Stuff y (x -> -z), lit warm from above: its vertex normal (1,0,0)
    // lands on (0,0,-1) and the light travels along -y with a z part, so cos is 0.6.
    const CPP: &str = "\
C turn 1\n\
M pos 0 0 0 rot 0 0 0 1 shapes 1 textures 1\n\
N joint_root processMe 1 m 1 0 0 0 1 0 0 0 1 0 0 0\n\
N s1 processMe 1 m 0 0 -1 0 1 0 1 0 0 0 0 0\n\
S s1 nv 1 nt 1 scalar 0 useVertexLighting 1 renderer 0 isNight 0 nightFactor 0.166667 lightsOut 0 isSpotlight 0 isWindow 0 drawnByShader 0 alpha 255 highlight 00000000 base 00000000 fog ffffffff\n\
L 0 type 1 active 1 argb 00ffbeab intensity 1.000000 local 0 0 0 world 0.000000 -0.800000 0.600000\n\
L 1 type 0 active 1 argb 001f1f1f intensity 1.000000 local 0 0 0 world 0 0 0\n\
V s1 0 pos 1.0 2.0 3.0 nrm 1.0 0.0 0.0 wpos 0 0 0 wnrm 0.000000 0.000000 -1.000000 vlight ff000000 amb 31 31 31 argb ffb89186 frgb ff000000 scr 0 0 0\n\
M pos 0 0 0 rot 0 0 0 1 shapes 1 textures 1\n\
S marker nv 1 nt 1 scalar 0 useVertexLighting 1 renderer 0 isNight 0 nightFactor 0 lightsOut 0 isSpotlight 0 isWindow 0 drawnByShader 0 alpha 255 highlight 00000000 base 00000000 fog ffffffff\n\
V marker 0 pos 0 0 0 nrm 0 1 0 wpos 0 0 0 wnrm 0 1 0 vlight ff000000 amb 31 31 31 argb ff000000 frgb ff000000 scr 0 0 0\n";

    fn port(infinite: &str) -> String {
        format!(
            "R shape t nodes 2 vertices 1 lightdir_stuff 0.000000 -0.800000 0.600000 infinite {infinite} ambient 0.121569 0.121569 0.121569 highlight 00000000 night 0 0.166667\n\
             V s1 0 pos 1.000000 2.000000 3.000000 nrm 1.000000 0.000000 0.000000 wpos 0 0 0 wnrm 1 0 0 vlight ff000000 lit 0 0 0 argb ff9e9e9e uv 0 0\n"
        )
    }

    #[test]
    fn parses_the_mech_section_only() {
        let d = parse_cpp(CPP).unwrap();
        assert_eq!(d.shapes.len(), 1);
        assert_eq!(d.vertices.len(), 1);
        assert_eq!(d.shapes[0].light0_argb, 0x00ff_beab);
        assert_eq!(d.shapes[0].ambient_argb, 0x001f_1f1f);
        assert_eq!(d.nodes["s1"][0], [0.0, 0.0, -1.0]);
    }

    /// The pose convention is Stuff's row vectors: the C++ dump's own `wnrm` is `nrm * shapeToWorld`.
    #[test]
    fn row_vectors_reproduce_the_cpp_world_normal() {
        let d = parse_cpp(CPP).unwrap();
        let v = &d.vertices[0];
        let w = row_times(v.nrm, &d.nodes["s1"]);
        assert!((w - v.wnrm).dot(w - v.wnrm) < 1e-12, "{w:?}");
    }

    /// The port dump's light direction reads back as the `shape_dir` that wrote it.
    #[test]
    fn the_port_light_direction_round_trips() {
        let p = parse_port(&port("1.000000 0.745098 0.670588")).unwrap();
        let back = crate::math::world_to_stuff().transform_vector(-p.inputs.shape_dir);
        assert!((back - Vec3::new(0.0, -0.8, 0.6)).dot(back - Vec3::new(0.0, -0.8, 0.6)) < 1e-12);
    }

    /// Warm light 255/190/171 at cos 0.6 plus 31 is 184/145/134 (0xb89186) on both sides; white is 184/184/184.
    #[test]
    fn the_same_light_matches_in_the_cpp_pose_and_white_does_not() {
        let cpp = parse_cpp(CPP).unwrap();
        let warm = parse_port(&port("1.000000 0.745098 0.670588")).unwrap();
        assert!(compare_inputs(&cpp, &warm).iter().all(|r| r.equal));
        let c = relight_cpp_normals(&cpp, &warm.inputs, 1);
        assert_eq!((c.compared, c.within), (1, 1), "{c:?}");
        let (posed, gap) = port_mesh_in_cpp_pose(&cpp, &warm, 1);
        assert_eq!((posed.compared, posed.within), (1, 1), "{posed:?}");
        assert!(gap < 1e-6);
        // the bind pose (ff9e9e9e) is not the C++ pose
        assert_eq!(port_bind_pose(&cpp, &warm, 1).within, 0);

        let white = parse_port(&port("1.000000 1.000000 1.000000")).unwrap();
        let rows = compare_inputs(&cpp, &white);
        let first = rows.iter().find(|r| !r.equal).unwrap();
        assert_eq!(first.field, "light 0 colour (sun to shapes, bytes)");
        assert_eq!(first.cpp, "[255, 190, 171]");
        let c = relight_cpp_normals(&cpp, &white.inputs, 1);
        assert_eq!(c.within, 0);
        assert!(c
            .first
            .unwrap()
            .starts_with("s1 0: C++ argb ffb89186, port ffb8b8b8"));
        assert!(report(&cpp, &white, 1).contains("first differing input: light 0 colour"));
    }
}
