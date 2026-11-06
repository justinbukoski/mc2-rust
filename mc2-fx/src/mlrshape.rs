// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MidLevelRenderer::MLRShape` as embedded in `Shape`, `ShapeCloud` and `DebrisCloud`
//! specifications (`mclib/mlr/mlrshape.cpp`, `mlrprimitivebase.cpp`,
//! `mlrindexedprimitivebase.cpp`, `mlr_i_*mesh.cpp`).
//!
//! A shape is a count and that many primitives, each a class id followed by that class's
//! fields. Only the polygon and triangle mesh classes appear in effect data. They all share
//! the primitive base fields (coordinates, texture coordinates, strip lengths, draw mode,
//! state) and the indexed base (u16 index list); the variants add, in order, a second state
//! (the `DT`/`DeT` detail-texture meshes, `DeT` also with offset/scale/fade floats; the load
//! constructors then turn z-writes off for that second pass and, for `DT` and `DeT_PMesh`, reset
//! the priorities), vertex colours (`C`) and vertex normals (`L`).

use crate::error::{FxError, Result};
use crate::math::{Rgba, Vec3};
use crate::state::{MlrState, FIRST_MLR_CLASS_ID};
use crate::stream::Stream;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MeshKind {
    /// `MLR_I_PMesh` family: polygons, with per-polygon vertex counts in `lengths`.
    Poly,
    /// `MLR_I_TMesh` family: triangles from the index list.
    Tri,
}

#[derive(Clone, Debug)]
pub struct Primitive {
    pub class_id: i32,
    pub kind: MeshKind,
    pub coords: Vec<Vec3>,
    pub tex_coords: Vec<[f32; 2]>,
    pub lengths: Vec<u8>,
    pub draw_mode: i32,
    pub state: MlrState,
    pub index: Vec<u16>,
    /// Second-pass state of the detail-texture variants.
    pub state2: Option<MlrState>,
    /// `DeT` meshes: x offset, y offset, x scale, y scale.
    pub detail: Option<[f32; 4]>,
    /// `DeT_TMesh`: the distances at which the detail pass starts and ends fading
    /// (`fadeDetailStart`, `fadeDetailEnd`).
    pub detail_fade: Option<[f32; 2]>,
    pub colors: Vec<Rgba>,
    pub normals: Vec<Vec3>,
}

#[derive(Clone, Debug, Default)]
pub struct MlrShape {
    pub primitives: Vec<Primitive>,
}

/// What a mesh class reads after the indexed base.
struct Layout {
    kind: MeshKind,
    /// Reads a second state (the detail-texture meshes).
    state2: bool,
    detail: Detail,
    colors: bool,
    normals: bool,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Detail {
    None,
    /// `DT`: second state only; both priorities are forced to the default (0).
    Dt,
    /// `DeT_PMesh`: second state and four offset/scale floats; priorities forced to 0.
    DetP,
    /// `DeT_TMesh`: second state, four floats and two fade floats; priorities untouched.
    DetT,
}

fn mesh_layout(class_index: i32) -> Option<Layout> {
    // index into the MLR class-id enum (mclib/mlr/mlr.hpp), relative to FirstMLRClassID
    use Detail::{DetP, DetT, Dt};
    use MeshKind::{Poly, Tri};
    let l = |kind, detail, colors, normals| Layout {
        kind,
        state2: detail != Detail::None,
        detail,
        colors,
        normals,
    };
    Some(match class_index {
        23 => l(Poly, Detail::None, false, false), // MLR_I_PMesh
        24 => l(Poly, Detail::None, true, false),  // MLR_I_C_PMesh
        25 => l(Poly, Detail::None, true, true),   // MLR_I_L_PMesh
        26 => l(Poly, Dt, false, false),           // MLR_I_DT_PMesh
        27 => l(Poly, Dt, true, false),            // MLR_I_C_DT_PMesh
        28 => l(Poly, Dt, true, true),             // MLR_I_L_DT_PMesh
        32 => l(Poly, DetP, false, false),         // MLR_I_DeT_PMesh
        33 => l(Poly, DetP, true, false),          // MLR_I_C_DeT_PMesh
        34 => l(Poly, DetP, true, true),           // MLR_I_L_DeT_PMesh
        35 => l(Tri, Detail::None, false, false),  // MLR_I_TMesh
        36 => l(Tri, DetT, false, false),          // MLR_I_DeT_TMesh
        37 => l(Tri, Detail::None, true, false),   // MLR_I_C_TMesh
        38 => l(Tri, Detail::None, true, true),    // MLR_I_L_TMesh
        43 => l(Tri, Dt, false, false),            // MLR_I_DT_TMesh
        44 => l(Tri, Dt, true, false),             // MLR_I_C_DT_TMesh
        45 => l(Tri, Dt, true, true),              // MLR_I_L_DT_TMesh
        46 => l(Tri, DetT, true, false),           // MLR_I_C_DeT_TMesh
        47 => l(Tri, DetT, true, true),            // MLR_I_L_DeT_TMesh
        _ => return None,
    })
}

fn read_vec3_array(s: &mut Stream) -> Result<Vec<Vec3>> {
    let n = s.count(12)?;
    (0..n).map(|_| s.vec3()).collect()
}

/// A `DWORD` colour array, converted the way `MLR_I_C_*Mesh` does (`* One_Over_256`).
fn read_colors(s: &mut Stream) -> Result<Vec<Rgba>> {
    let n = s.count(4)?;
    let k = 1.0f32 / 256.0;
    (0..n)
        .map(|_| {
            let c = s.u32()?;
            Ok(Rgba::new(
                ((c >> 16) & 0xff) as f32 * k,
                ((c >> 8) & 0xff) as f32 * k,
                (c & 0xff) as f32 * k,
                ((c >> 24) & 0xff) as f32 * k,
            ))
        })
        .collect()
}

impl Primitive {
    fn load(s: &mut Stream, version: i32) -> Result<Primitive> {
        let class_id = s.i32()?;
        let layout = mesh_layout(class_id - FIRST_MLR_CLASS_ID).ok_or_else(|| {
            FxError::new(format!(
                "unsupported MLR primitive class {class_id} (index {})",
                class_id - FIRST_MLR_CLASS_ID
            ))
        })?;
        if version < 3 {
            return Err(FxError::new("MLR data before version 3 is not supported"));
        }
        // MLRPrimitiveBase
        let coords = read_vec3_array(s)?;
        let n = s.count(8)?;
        let mut tex_coords = Vec::with_capacity(n);
        for _ in 0..n {
            tex_coords.push([s.f32()?, s.f32()?]);
        }
        let n = s.count(1)?;
        let mut lengths = Vec::with_capacity(n);
        for _ in 0..n {
            lengths.push(s.u8()?);
        }
        let draw_mode = s.i32()?;
        let state = MlrState::load(s, version)?;
        // MLRIndexedPrimitiveBase
        let n = s.count(2)?;
        let mut index = Vec::with_capacity(n);
        for _ in 0..n {
            let b0 = s.u8()? as u16;
            let b1 = s.u8()? as u16;
            index.push(b0 | (b1 << 8));
        }
        let mut state = state;
        let mut state2 = None;
        let mut detail_vals = None;
        let mut detail_fade = None;
        if layout.state2 {
            let mut st2 = MlrState::load(s, version)?;
            match layout.detail {
                Detail::DetP => {
                    detail_vals = Some([s.f32()?, s.f32()?, s.f32()?, s.f32()?]);
                }
                Detail::DetT => {
                    let v = [s.f32()?, s.f32()?, s.f32()?, s.f32()?];
                    detail_fade = Some([s.f32()?, s.f32()?]);
                    detail_vals = Some(v);
                }
                _ => {}
            }
            // The constructors: the second pass never writes z, and the DT and DeT_P meshes
            // reset both priorities to the default.
            st2.set_z_write(false);
            if matches!(layout.detail, Detail::Dt | Detail::DetP) {
                state.set_priority(0);
                st2.set_priority(0);
            }
            state2 = Some(st2);
        }
        let colors = if layout.colors {
            read_colors(s)?
        } else {
            Vec::new()
        };
        let normals = if layout.normals {
            read_vec3_array(s)?
        } else {
            Vec::new()
        };
        let kind = layout.kind;
        Ok(Primitive {
            class_id,
            kind,
            coords,
            tex_coords,
            lengths,
            draw_mode,
            state,
            index,
            state2,
            detail: detail_vals,
            detail_fade,
            colors,
            normals,
        })
    }

    /// Triangles of this primitive as index triples into `coords`.
    pub fn triangles(&self) -> Vec<[u16; 3]> {
        let mut out = Vec::new();
        match self.kind {
            MeshKind::Tri => {
                for t in self.index.chunks(3) {
                    if let [a, b, c] = *t {
                        out.push([a, b, c]);
                    }
                }
            }
            MeshKind::Poly => {
                // Each polygon takes `lengths[i]` consecutive indices; fan it from the first.
                let mut at = 0usize;
                for &len in &self.lengths {
                    let len = len as usize;
                    if at + len > self.index.len() {
                        break;
                    }
                    for k in 1..len.saturating_sub(1) {
                        out.push([self.index[at], self.index[at + k], self.index[at + k + 1]]);
                    }
                    at += len;
                }
            }
        }
        out
    }
}

impl MlrShape {
    /// `MLRShape::MLRShape(stream, version)`.
    pub fn load(s: &mut Stream, version: i32) -> Result<MlrShape> {
        let n = s.i32()?;
        if !(0..=4096).contains(&n) {
            return Err(FxError::new(format!("MLR shape with {n} primitives")));
        }
        let mut primitives = Vec::with_capacity(n as usize);
        for _ in 0..n {
            primitives.push(Primitive::load(s, version)?);
        }
        Ok(MlrShape { primitives })
    }

    /// `ShapeCloud/Shape__Specification::SetShape`: the largest distance of any vertex from
    /// the origin.
    pub fn radius(&self) -> f32 {
        let mut r = 0.0f32;
        for p in &self.primitives {
            for c in &p.coords {
                r = r.max(c.length_squared());
            }
        }
        r.sqrt()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{FIRST_MLR_CLASS_ID, ZBUFFER_WRITE_MASK};

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A state with a priority-2 alpha render state, z-write on, no texture.
    fn state_bytes() -> Vec<u8> {
        words(&[ZBUFFER_WRITE_MASK, 0, 0xffff_ffff, 2, 0xf, 0xffff_ffff])
    }

    fn triangle_primitive(class_index: i32, with_second_state: bool) -> Vec<u8> {
        let mut b = words(&[(FIRST_MLR_CLASS_ID + class_index) as u32]);
        // coords: 3 vertices
        b.extend(3u32.to_le_bytes());
        for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            b.extend(v.iter().flat_map(|f| f.to_le_bytes()));
        }
        b.extend(0u32.to_le_bytes()); // tex coords
        b.extend(0u32.to_le_bytes()); // lengths
        b.extend(0i32.to_le_bytes()); // draw mode
        b.extend(state_bytes());
        // index: 0 1 2
        b.extend(3u32.to_le_bytes());
        for i in [0u16, 1, 2] {
            b.extend(i.to_le_bytes());
        }
        if with_second_state {
            b.extend(state_bytes());
        }
        b
    }

    #[test]
    fn plain_triangle_mesh() {
        let mut b = 1i32.to_le_bytes().to_vec();
        b.extend(triangle_primitive(35, false));
        let shape = MlrShape::load(&mut Stream::new(&b), 9).unwrap();
        assert_eq!(shape.primitives.len(), 1);
        assert_eq!(shape.primitives[0].triangles(), vec![[0, 1, 2]]);
        assert!((shape.radius() - 1.0).abs() < 1e-6);
        // the plain mesh keeps its priority
        assert_eq!(shape.primitives[0].state.priority(), 2);
    }

    #[test]
    fn dt_mesh_resets_priorities_and_second_pass_z_write() {
        let mut b = 1i32.to_le_bytes().to_vec();
        b.extend(triangle_primitive(43, true)); // MLR_I_DT_TMesh
        let shape = MlrShape::load(&mut Stream::new(&b), 9).unwrap();
        let p = &shape.primitives[0];
        assert_eq!(p.state.priority(), 0);
        let s2 = p.state2.as_ref().unwrap();
        assert_eq!(s2.priority(), 0);
        assert!(!s2.z_write());
        assert!(p.state.z_write());
    }

    #[test]
    fn polygon_strips_fan_into_triangles() {
        let p = Primitive {
            class_id: 0,
            kind: MeshKind::Poly,
            coords: Vec::new(),
            tex_coords: Vec::new(),
            lengths: vec![4, 3],
            draw_mode: 0,
            state: MlrState::default(),
            index: vec![0, 1, 2, 3, 4, 5, 6],
            state2: None,
            detail: None,
            detail_fade: None,
            colors: Vec::new(),
            normals: Vec::new(),
        };
        assert_eq!(p.triangles(), vec![[0, 1, 2], [0, 2, 3], [4, 5, 6]]);
    }

    #[test]
    fn unknown_class_is_an_error() {
        let mut b = 1i32.to_le_bytes().to_vec();
        b.extend(((FIRST_MLR_CLASS_ID + 3) as u32).to_le_bytes());
        assert!(MlrShape::load(&mut Stream::new(&b), 9).is_err());
    }
}

#[cfg(test)]
mod fade_tests {
    use super::*;

    #[test]
    fn det_tmesh_keeps_offsets_scales_and_fade() {
        let w = |v: &[u32]| -> Vec<u8> { v.iter().flat_map(|x| x.to_le_bytes()).collect() };
        let state = w(&[0, 0, 0xffff_ffff, 0, 0, 0xffff_ffff]);
        let mut b = 1i32.to_le_bytes().to_vec();
        b.extend(w(&[(FIRST_MLR_CLASS_ID + 36) as u32])); // MLR_I_DeT_TMesh
        b.extend(0u32.to_le_bytes()); // coords
        b.extend(0u32.to_le_bytes()); // tex coords
        b.extend(0u32.to_le_bytes()); // lengths
        b.extend(0i32.to_le_bytes()); // draw mode
        b.extend(&state);
        b.extend(0u32.to_le_bytes()); // index
        b.extend(&state); // second state
        for f in [0.5f32, 0.25, 8.0, 9.0, 300.0, 100.0] {
            b.extend(f.to_le_bytes());
        }
        let shape = MlrShape::load(&mut Stream::new(&b), 9).unwrap();
        let p = &shape.primitives[0];
        assert_eq!(p.detail, Some([0.5, 0.25, 8.0, 9.0]));
        assert_eq!(p.detail_fade, Some([300.0, 100.0]));
    }
}
