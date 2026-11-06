// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Debug dump of a shape's per-vertex lit colour (rc-vqa-vertexdump).
//!
//! The C++ lights shapes per vertex on the CPU (`TG_Shape::MultiTransformShape`, mclib/tgl.cpp:1746-2230) and
//! the GPU interpolates that colour; the port evaluates the same terms in `fs_shape` (shaders/scene.wgsl). To
//! compare the two, [`shape_lit`] evaluates the port's lighting at a vertex normal, exactly as `fs_shape` does
//! with the shadow factor 1 (a preview casts no shadows) and the enhanced-lighting switch off, and
//! [`write_dump`] writes it for every vertex of a posed model in the same fields the instrumented C++ writes.
//!
//! Off by default and debug-only: [`dump_dir`] reads `MC2_DUMP_VERTEX_LIGHT` once in a debug build (never in a
//! release build), and nothing else runs unless it is set.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::OnceLock;

use crate::light::argb_to_rgb;
use crate::math::{world_to_stuff, Mat4, Vec3};
use crate::objects::GlowColors;
use crate::shape::ShapeModel;

/// Environment variable naming the directory the dump is written to.
pub const DUMP_ENV: &str = "MC2_DUMP_VERTEX_LIGHT";

/// The dump directory, or `None` (the default) when [`DUMP_ENV`] is unset or empty. Read once per process, and
/// only in a debug build: a release build always returns `None` without looking at the environment.
pub fn dump_dir() -> Option<&'static PathBuf> {
    if !cfg!(debug_assertions) {
        return None;
    }
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::var_os(DUMP_ENV)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    })
    .as_ref()
}

/// What `fs_shape` receives for one instance, besides the vertex.
#[derive(Clone, Debug, PartialEq)]
pub struct VertexLightInputs {
    /// `g.shape_dir`: towards the sun, world frame (`LightModel::shape_light_direction`).
    pub shape_dir: Vec3,
    /// `ShapeInstance::infinite_light`: the sun colour fed to the shape (`setLightColor(0, ..)`).
    pub infinite: [f32; 3],
    /// `g.ambient.rgb` (or the instance's ambient override).
    pub ambient: [f32; 3],
    /// `ShapeInstance::highlight` (`SetARGBHighLight`).
    pub highlight: u32,
    /// `g.sun.w`.
    pub night_factor: f32,
    /// `g.ambient.w > 0.5`.
    pub is_night: bool,
    /// `ModelParams` pink / yellow / green.
    pub glow: GlowColors,
}

/// `vertex_emissive` of scene.wgsl: the vertex's `aRGBLight` term (tgl.cpp:1746-1834).
pub fn vertex_emissive(argb: u32, inp: &VertexLightInputs) -> [f32; 3] {
    let pink = argb_to_rgb(inp.glow.pink);
    let yellow = argb_to_rgb(inp.glow.yellow);
    let green = argb_to_rgb(inp.glow.green);
    let scale = |c: [f32; 3], k: f32| c.map(|v| v * k);
    let nf = inp.night_factor;
    match argb {
        0xffff_00ff => {
            if inp.is_night {
                pink
            } else if nf > 1.0e-4 {
                scale(pink, nf)
            } else {
                [47.0 / 255.0; 3]
            }
        }
        0xffff_ff00 => {
            if nf >= 0.75 {
                if inp.is_night {
                    yellow
                } else {
                    scale(yellow, nf)
                }
            } else {
                [0.0; 3]
            }
        }
        0xff00_ff00 => {
            if inp.is_night {
                green
            } else if nf > 1.0e-4 {
                scale(green, nf)
            } else {
                [0.0; 3]
            }
        }
        0xffff_0000 | 0xff00_00ff => [0.0; 3],
        _ if argb & 0x00ff_ffff != 0 => argb_to_rgb(argb),
        _ => [0.0; 3],
    }
}

/// `fs_shape`'s light multiplier at a vertex: `min(min(emissive + infinite * ndl * shade + ambient, 1) +
/// highlight, 1)` with `ndl = max(dot(normalize(n), shape_dir), 0)` and `shade = 1`.
pub fn shape_lit(normal_world: Vec3, argb: u32, inp: &VertexLightInputs) -> [f32; 3] {
    let n = normal_world.normalized();
    let ndl = n.dot(inp.shape_dir).max(0.0);
    let e = vertex_emissive(argb, inp);
    let h = argb_to_rgb(inp.highlight);
    std::array::from_fn(|c| {
        ((e[c] + inp.infinite[c] * ndl + inp.ambient[c]).min(1.0) + h[c]).min(1.0)
    })
}

/// `lit` as the C++ packs it: 0xffRRGGBB, each channel rounded to a byte.
pub fn lit_argb(lit: [f32; 3]) -> u32 {
    let b = lit.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u32);
    0xff00_0000 | (b[0] << 16) | (b[1] << 8) | b[2]
}

/// Writes one line per vertex of `model` (per triangle corner: the port's mesh has no shared vertices) posed by
/// `node_world` (the node matrices `fs_shape` sees, world frame, from `instance_node_matrices`). Positions,
/// normals and the light direction are written in the Stuff frame, as the C++ dump writes them; the C++ light
/// direction is the light's travel direction, so `-shape_dir` is written next to it for comparison.
pub fn write_dump(
    out: &mut impl Write,
    model: &ShapeModel,
    node_world: &[Mat4],
    inp: &VertexLightInputs,
) -> io::Result<()> {
    let to_stuff = world_to_stuff();
    // the travel direction; `+ 0.0` writes a negated zero as 0.000000
    let l = -to_stuff.transform_vector(inp.shape_dir) + Vec3::ZERO;
    writeln!(
        out,
        "R shape {} nodes {} vertices {} lightdir_stuff {:.6} {:.6} {:.6} infinite {:.6} {:.6} {:.6} ambient {:.6} {:.6} {:.6} highlight {:08x} night {} {:.6}",
        model.name,
        model.nodes.len(),
        model.vertices.len(),
        l.x,
        l.y,
        l.z,
        inp.infinite[0],
        inp.infinite[1],
        inp.infinite[2],
        inp.ambient[0],
        inp.ambient[1],
        inp.ambient[2],
        inp.highlight,
        inp.is_night as u8,
        inp.night_factor,
    )?;
    for (i, v) in model.vertices.iter().enumerate() {
        let node = v.node as usize;
        let m = node_world.get(node).copied().unwrap_or(Mat4::IDENTITY);
        let p = Vec3::from_array(v.position);
        let n = Vec3::from_array(v.normal);
        let nw = m.transform_vector(n);
        let wp = to_stuff.transform_point(m.transform_point(p));
        let wn = to_stuff.transform_vector(nw).normalized();
        let lit = shape_lit(nw, v.argb, inp);
        let name = model.nodes.get(node).map_or("?", |n| n.id.as_str());
        writeln!(
            out,
            "V {name} {i} pos {:.6} {:.6} {:.6} nrm {:.6} {:.6} {:.6} wpos {:.4} {:.4} {:.4} wnrm {:.6} {:.6} {:.6} vlight {:08x} lit {:.6} {:.6} {:.6} argb {:08x} uv {:.6} {:.6}",
            p.x,
            p.y,
            p.z,
            n.x,
            n.y,
            n.z,
            wp.x,
            wp.y,
            wp.z,
            wn.x,
            wn.y,
            wn.z,
            v.argb,
            lit[0],
            lit[1],
            lit[2],
            lit_argb(lit),
            v.uv[0],
            v.uv[1],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE_WGSL: &str = include_str!("shaders/scene.wgsl");

    fn preview_inputs() -> VertexLightInputs {
        VertexLightInputs {
            shape_dir: Vec3::new(0.0, 0.6, 0.8),
            infinite: [1.0; 3],
            ambient: [31.0 / 255.0; 3],
            highlight: 0,
            night_factor: 0.0,
            is_night: false,
            glow: GlowColors::default(),
        }
    }

    /// The dump's formula is the shader's: the lines of `fs_shape` and `vertex_emissive` that [`shape_lit`] and
    /// [`vertex_emissive`] mirror are pinned here, so a change to the shader lighting fails this test until the
    /// dump follows it.
    #[test]
    fn the_dump_formula_is_the_fs_shape_lighting() {
        for line in [
            "let n = normalize(i.normal);",
            "var ndl = max(dot(n, g.shape_dir.xyz), 0.0);",
            "let lit = min(min(i.emissive + i.infinite * ndl * shade + i.ambient, vec3<f32>(1.0)) + i.highlight, vec3<f32>(1.0));",
            "let rgb = apply_fog(min(tex.rgb * lit + pool, vec3<f32>(1.0)), i.world, 0.0);",
            "o.emissive = vertex_emissive(v.argb);",
            "o.infinite = v.infinite.rgb;",
            "o.highlight = vec3<f32>(byte_of(v.highlight, 16u), byte_of(v.highlight, 8u), byte_of(v.highlight, 0u));",
            "if (argb == 0xffff00ffu) {",
            "return vec3<f32>(47.0 / 255.0);",
            "if (night_factor >= 0.75) {",
            "if (argb == 0xffff0000u || argb == 0xff0000ffu) {",
            "if (rgb != 0u) {",
        ] {
            assert!(
                SCENE_WGSL.contains(line),
                "scene.wgsl no longer contains `{line}`: update vertex_light.rs to match"
            );
        }
    }

    /// At the vertices the shader's lit colour is the C++ per-vertex sum (tgl.cpp:1862-2140): emissive + sun *
    /// max(cos, 0) + ambient, clamped, then the highlight added and clamped.
    #[test]
    fn shape_lit_is_the_tgl_vertex_sum() {
        let inp = preview_inputs();
        // facing the sun: 255 + 31 saturates
        assert_eq!(shape_lit(inp.shape_dir, 0xff00_0000, &inp), [1.0; 3]);
        // facing away: ambient only
        let back = Vec3::new(0.0, -0.6, -0.8);
        assert_eq!(shape_lit(back, 0xff00_0000, &inp), [31.0 / 255.0; 3]);
        // cos 0.6 off the sun: 153 + 31 (whole bytes, clear of a rounding tie); the length of the normal does not matter
        let n = (Vec3::new(1.0, 0.0, 0.0) * 0.8 + inp.shape_dir * 0.6) * 3.0;
        let lit = shape_lit(n, 0xff00_0000, &inp);
        for c in lit {
            assert!((c - (0.6 + 31.0 / 255.0)).abs() < 1e-5, "{c}");
        }
        assert_eq!(lit_argb(lit), 0xffb8_b8b8);
        // an ordinary vertex light is added; the highlight goes on after the first clamp
        let lit = shape_lit(
            back,
            0xff10_2030,
            &VertexLightInputs {
                highlight: 0x0040_0000,
                ..inp.clone()
            },
        );
        assert!((lit[0] - (0x10 as f32 + 31.0 + 0x40 as f32) / 255.0).abs() < 1e-5);
        assert!((lit[1] - (0x20 as f32 + 31.0) / 255.0).abs() < 1e-5);
        assert!((lit[2] - (0x30 as f32 + 31.0) / 255.0).abs() < 1e-5);
        // a lit-window marker is dark grey by day and takes no sun in the vertex sum's emissive term
        assert_eq!(vertex_emissive(0xffff_00ff, &inp), [47.0 / 255.0; 3]);
        assert_eq!(vertex_emissive(0xffff_0000, &inp), [0.0; 3]);
    }

    #[test]
    fn the_dump_writes_one_line_per_vertex_in_the_stuff_frame() {
        use crate::shape::{NodeInfo, NodeKind, ShapeVertex};
        let model = ShapeModel {
            name: "t".into(),
            nodes: vec![NodeInfo {
                id: "joint_root".into(),
                parent: None,
                center: Vec3::ZERO,
                relative_center: Vec3::ZERO,
                kind: NodeKind::Normal,
            }],
            texture_names: vec![],
            texture_alpha: vec![],
            vertices: vec![ShapeVertex {
                position: [1.0, 2.0, 3.0],
                node: 0,
                normal: [0.0, 1.0, 0.0],
                argb: 0xff00_0000,
                uv: [0.25, 0.5],
            }],
            batches: vec![],
            bounds: (Vec3::ZERO, Vec3::ZERO),
            cone_extent: 0.0,
            building_bounds: (Vec3::ZERO, Vec3::ZERO),
            gestures: vec![],
        };
        // the scene's node matrix for an unposed node is stuff_to_world
        let mats = crate::shape::instance_node_matrices(&[Mat4::IDENTITY], Vec3::ZERO, 0.0);
        let mut out = Vec::new();
        write_dump(&mut out, &model, &mats, &preview_inputs()).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("R shape t nodes 1 vertices 1 "));
        // Stuff up (+y) is world +z: 0.8 towards the sun, so the light travels along Stuff -y by 0.8
        assert!(
            lines[0].contains("lightdir_stuff 0.000000 -0.800000 -0.600000"),
            "{}",
            lines[0]
        );
        assert!(lines[1].starts_with("V joint_root 0 pos 1.000000 2.000000 3.000000 nrm 0.000000 1.000000 0.000000 wpos 1.0000 2.0000 3.0000 wnrm 0.000000 1.000000 0.000000"), "{}", lines[1]);
        // ndl 0.8: 204 + 31 = 235
        assert!(lines[1].contains("argb ffebebeb"), "{}", lines[1]);
    }
}
