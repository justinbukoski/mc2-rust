// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `Singleton` (`mclib/gosfx/singleton.cpp`) and `Shape` (`shape.cpp`): one animated colour and
//! scale on a single drawable, here an MLR shape. `Card` builds on the same base (`card.rs`).

use crate::draw::{DrawInfo, DrawList, DrawState, DrawVertex};
use crate::effect::{Ctx, Data, Effect, ExecuteInfo};
use crate::fcurve::FCurve;
use crate::math::{Mat, Obb, Rgba, Vec3, SMALL, X_AXIS, Y_AXIS, Z_AXIS};
use crate::mlrshape::MlrShape;
use crate::spec::{SingletonSpec, SpecKind};
use crate::state::MlrState;

/// The members `Singleton` adds to an effect.
#[derive(Clone, Copy, Debug, Default)]
pub struct SingletonState {
    pub color: Rgba,
    pub radius: f32,
    pub scale: f32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShapeState {
    pub s: SingletonState,
}

impl ShapeState {
    pub(crate) fn new(radius: f32) -> ShapeState {
        ShapeState {
            s: SingletonState {
                color: Rgba::WHITE,
                radius,
                scale: 1.0,
            },
        }
    }
}

pub(crate) fn singleton_spec(spec: &crate::spec::EffectSpec) -> Option<&SingletonSpec> {
    match &spec.kind {
        SpecKind::Card(c) => Some(&c.singleton),
        SpecKind::Shape(s) => Some(&s.singleton),
        _ => None,
    }
}

/// `Singleton::Execute` after the base `Effect::Execute`: animate colour and scale, add the
/// sphere to the bounds.
pub(crate) fn singleton_animate(
    spec: &SingletonSpec,
    age: f32,
    seed: f32,
    state: &mut SingletonState,
    local_to_parent: &Mat,
    bounds: &mut Obb,
) {
    state.color = Rgba::new(
        spec.red.compute(age, seed),
        spec.green.compute(age, seed),
        spec.blue.compute(age, seed),
        spec.alpha.compute(age, seed),
    );
    state.scale = spec.scale.compute(age, seed);

    let mut radius = state.radius * state.scale;
    if radius < SMALL {
        radius = 0.01;
    }
    let sphere = Obb {
        local_to_parent: *local_to_parent,
        axis_extents: Vec3::ZERO,
        sphere_radius: radius,
    };
    *bounds = Obb::union(bounds, &sphere);
}

/// The camera alignment `Card::Draw` and `Shape::Draw` apply to the effect-to-world matrix.
pub(crate) fn align_to_camera(
    spec: &SingletonSpec,
    local_to_world: &mut Mat,
    camera_to_world: &Mat,
) {
    if spec.align_z_using_x {
        let look_at = camera_to_world.translation() - local_to_world.translation();
        if spec.align_z_using_y {
            local_to_world.align_local_axis_to_world_vector(look_at, Z_AXIS, Y_AXIS, Some(X_AXIS));
        } else {
            local_to_world.align_local_axis_to_world_vector(look_at, Z_AXIS, X_AXIS, None);
        }
    } else if spec.align_z_using_y {
        let look_at = camera_to_world.translation() - local_to_world.translation();
        local_to_world.align_local_axis_to_world_vector(look_at, Z_AXIS, Y_AXIS, None);
    }
}

/// `MLRClipper::DrawScalableShape`, flattened to triangles in world space: the shape is scaled
/// (`scale * shapeToWorld`), every primitive takes the combined state of the effect and its own
/// reference state, and primitives that carry vertex colours are painted with `paint`
/// (`PaintMe`). `DT`/`DeT` meshes get their second pass (`GetNumPasses() == 2`): the detail state,
/// for `DeT` meshes the texture coordinates scaled and offset by the mesh's detail floats, and for
/// `DeT_TMesh` an alpha that fades with the distance from `camera` (`rhw` against
/// `fadeDetailStart`/`End`, mlrtriangleclipping.hpp:315-366; `rhw` is taken as one over the
/// distance to the camera instead of one over the view depth).
pub fn draw_shape(
    shape: &MlrShape,
    shape_to_world: &Mat,
    camera: Vec3,
    scale: Vec3,
    paint: Rgba,
    state: &MlrState,
    out: &mut DrawList,
) {
    for prim in &shape.primitives {
        let ds: DrawState = DrawState::from_mlr(&MlrState::combine(state, &prim.state));
        let color = if prim.colors.is_empty() {
            Rgba::WHITE
        } else {
            paint
        };
        let verts: Vec<DrawVertex> = prim
            .coords
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let scaled = Vec3::new(c.x * scale.x, c.y * scale.y, c.z * scale.z);
                DrawVertex {
                    pos: shape_to_world.point(scaled).to_array(),
                    uv: prim.tex_coords.get(i).copied().unwrap_or([0.0, 0.0]),
                    color: color.to_array(),
                }
            })
            .collect();
        let mut indices = Vec::new();
        for t in prim.triangles() {
            if t.iter().all(|&i| (i as usize) < verts.len()) {
                indices.extend(t.iter().map(|&i| i as u32));
            }
        }
        out.push(&ds, &verts, &indices);
        if let Some(state2) = &prim.state2 {
            let ds2 = DrawState::from_mlr(&MlrState::combine(state, state2));
            let mut verts2 = verts.clone();
            for (i, v) in verts2.iter_mut().enumerate() {
                if let Some(d) = prim.detail {
                    v.uv = [v.uv[0] * d[2] + d[0], v.uv[1] * d[3] + d[1]];
                } else if let Some(uv) = prim.tex_coords.get(verts.len() + i) {
                    // `DT` meshes carry a second set of texture coordinates after the first
                    v.uv = *uv;
                }
                if let Some([start, end]) = prim.detail_fade {
                    let p = Vec3::new(v.pos[0], v.pos[1], v.pos[2]);
                    let rhw = 1.0 / (p - camera).length().max(1.0e-6);
                    if rhw < end {
                        v.color[3] = 0.0;
                    } else if rhw < start {
                        let a = (((rhw - end) * 255.9 / (start - end)) as u32).min(255) as f32;
                        v.color[3] = v.color[3].min(a / 255.0);
                    }
                }
            }
            // `detTextureVisible`: nothing is drawn when every vertex has faded out
            if prim.detail_fade.is_none() || verts2.iter().any(|v| v.color[3] > 0.0) {
                out.push(&ds2, &verts2, &indices);
            }
        }
    }
}

impl Effect {
    /// `Singleton::Execute` for `Shape` (which adds nothing of its own).
    pub(crate) fn shape_execute(
        &mut self,
        ctx: &mut Ctx,
        info: &ExecuteInfo,
        bounds: &mut Obb,
    ) -> bool {
        if !self.is_executed() {
            return false;
        }
        if !self.effect_execute(ctx, info, bounds) {
            return false;
        }
        let Effect { spec, b, data } = self;
        if let (Some(ss), Data::Shape(st)) = (singleton_spec(spec), data) {
            singleton_animate(ss, b.age, b.seed, &mut st.s, &b.local_to_parent, bounds);
        }
        true
    }

    /// `Shape::Draw`.
    pub(crate) fn shape_draw(&self, info: &DrawInfo, out: &mut DrawList) {
        let (SpecKind::Shape(spec), Data::Shape(st)) = (&self.spec.kind, &self.data) else {
            return;
        };
        let combined = MlrState::combine(&info.state, &self.spec.base.state);
        let mut local_to_world = Mat::multiply(&self.b.local_to_parent, &info.parent_to_world);
        align_to_camera(&spec.singleton, &mut local_to_world, &info.camera_to_world);
        let s = st.s.scale;
        draw_shape(
            &spec.shape,
            &local_to_world,
            info.camera_to_world.translation(),
            Vec3::new(s, s, s),
            st.s.color,
            &combined,
            out,
        );
    }
}

#[cfg(test)]
mod detail_pass_tests {
    use super::*;
    use crate::mlrshape::{MeshKind, Primitive};

    fn tri(detail: Option<[f32; 4]>, fade: Option<[f32; 2]>, uv: Vec<[f32; 2]>) -> MlrShape {
        let mut state2 = MlrState::default();
        state2.set_z_write(false);
        MlrShape {
            primitives: vec![Primitive {
                class_id: 0,
                kind: MeshKind::Tri,
                coords: vec![
                    Vec3::new(0.0, 0.0, 0.0),
                    Vec3::new(1.0, 0.0, 0.0),
                    Vec3::new(0.0, 1.0, 0.0),
                ],
                tex_coords: uv,
                lengths: Vec::new(),
                draw_mode: 0,
                state: MlrState::default(),
                index: vec![0, 1, 2],
                state2: Some(state2),
                detail,
                detail_fade: fade,
                colors: Vec::new(),
                normals: Vec::new(),
            }],
        }
    }

    fn draw(shape: &MlrShape, camera: Vec3) -> DrawList {
        let mut out = DrawList::default();
        let mut master = MlrState::game_draw_state();
        master.set_z_write(true);
        draw_shape(
            shape,
            &Mat::IDENTITY,
            camera,
            Vec3::new(1.0, 1.0, 1.0),
            Rgba::WHITE,
            &master,
            &mut out,
        );
        out
    }

    const UV: [[f32; 2]; 3] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];

    #[test]
    fn det_mesh_draws_a_second_pass_with_scaled_offset_uvs() {
        let out = draw(
            &tri(Some([0.5, 0.25, 8.0, 9.0]), None, UV.to_vec()),
            Vec3::new(0.0, 0.0, 10.0),
        );
        assert_eq!(out.batches.len(), 2);
        let (a, b) = (&out.batches[0], &out.batches[1]);
        assert!(a.state.z_write && !b.state.z_write);
        assert_eq!(a.vertices[1].uv, [1.0, 0.0]);
        assert_eq!(b.vertices[1].uv, [8.5, 0.25]);
        assert_eq!(b.vertices[2].uv, [0.5, 9.25]);
    }

    #[test]
    fn dt_mesh_second_pass_reads_the_second_uv_set() {
        let mut uv = UV.to_vec();
        uv.extend([[0.2, 0.2], [0.4, 0.4], [0.6, 0.6]]);
        let out = draw(&tri(None, None, uv), Vec3::ZERO);
        assert_eq!(out.batches.len(), 2);
        assert_eq!(out.batches[1].vertices[1].uv, [0.4, 0.4]);
    }

    #[test]
    fn det_tmesh_detail_fades_with_distance_and_is_skipped_when_gone() {
        // reciprocal distances: fully visible nearer than 1/0.02 = 50, gone beyond 1/0.01 = 100
        let shape = tri(Some([0.0, 0.0, 2.0, 2.0]), Some([0.02, 0.01]), UV.to_vec());
        let near = draw(&shape, Vec3::new(0.0, 0.0, 40.0));
        assert_eq!(near.batches[1].vertices[0].color[3], 1.0);
        let mid = draw(&shape, Vec3::new(0.0, 0.0, 70.0));
        let a = mid.batches[1].vertices[0].color[3];
        assert!(a > 0.1 && a < 0.9, "{a}");
        let far = draw(&shape, Vec3::new(0.0, 0.0, 200.0));
        assert_eq!(far.batches.len(), 1);
    }
}
