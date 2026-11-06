//! Draw data: what an effect hands to the renderer.
//!
//! The C++ effects write vertices into MLR primitives (`MLRCardCloud`, `MLRTriangleCloud`,
//! `MLRNGonCloud`, `MLRPointCloud`, `MLRIndexedTriangleCloud`) and queue them with the
//! clipper. Here `Effect::draw` appends the same geometry, already transformed into world space,
//! to a [`DrawList`]; a renderer sorts and draws it. Nothing in this module touches the GPU.

use crate::math::{Mat, Vec3};
use crate::state::{BlendMode, MlrState, TextureRef};

/// The render state of one batch, resolved from the `MLRState::Combine` chain.
///
/// The MLR back-face bit is deliberately absent: `MLRClipper::DrawScalableShape` and
/// `DrawEffect` transform primitives without ever calling `FindBackFace` (only the plain
/// `DrawShape` path does), so effects are two-sided.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawState {
    pub texture: Option<TextureRef>,
    pub blend: BlendMode,
    pub z_write: bool,
    pub z_compare: bool,
    /// `MLRState` priority: the renderer draws lower priorities first (alpha effects use 2+).
    pub priority: u32,
    /// Texture clamp (`TextureClamp`) rather than wrap.
    pub clamp: bool,
}

impl DrawState {
    pub fn from_mlr(s: &MlrState) -> DrawState {
        DrawState {
            texture: s.texture.clone(),
            blend: s.blend(),
            z_write: s.z_write(),
            z_compare: s.z_compare(),
            priority: s.priority(),
            clamp: s.clamp_texture(),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct DrawVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// Straight (non-premultiplied) RGBA, 0..1 and possibly above 1 for overbright effects.
    pub color: [f32; 4],
}

/// Triangles sharing one state.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawBatch {
    pub state: DrawState,
    /// The batch belongs to an explosion that should soften its intersection with terrain.
    pub ground_fade: bool,
    pub vertices: Vec<DrawVertex>,
    /// Triangle list indices into `vertices`.
    pub indices: Vec<u32>,
}

/// A light the original code computed and then discarded (`LightManager::ChangeLight` is empty
/// in the MechCommander 2 build). Exposed so a modern renderer can use it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LightDraw {
    pub position: Vec3,
    pub color: [f32; 3],
    pub intensity: f32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub two_sided: bool,
}

/// Everything one frame of effects wants drawn.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    pub batches: Vec<DrawBatch>,
    pub lights: Vec<LightDraw>,
    pub(crate) ground_fade: bool,
}

impl DrawList {
    pub fn clear(&mut self) {
        self.batches.clear();
        self.lights.clear();
        self.ground_fade = false;
    }

    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }

    /// Appends triangles, merging into the last batch when the state is the same (effects emit
    /// many small primitives with one state).
    pub fn push(&mut self, state: &DrawState, vertices: &[DrawVertex], indices: &[u32]) {
        if vertices.is_empty() || indices.is_empty() {
            return;
        }
        let merge = matches!(self.batches.last(), Some(b) if b.state == *state && b.ground_fade == self.ground_fade);
        if !merge {
            self.batches.push(DrawBatch {
                state: state.clone(),
                ground_fade: self.ground_fade,
                vertices: Vec::new(),
                indices: Vec::new(),
            });
        }
        let batch = self.batches.last_mut().expect("batch just ensured");
        let base = batch.vertices.len() as u32;
        batch.vertices.extend_from_slice(vertices);
        batch.indices.extend(indices.iter().map(|i| base + i));
    }

    /// Batches in drawing order: stable by priority, so opaque (priority 0) first and alpha
    /// effects after, each group in emission order.
    pub fn sorted(&self) -> Vec<&DrawBatch> {
        let mut v: Vec<&DrawBatch> = self.batches.iter().collect();
        v.sort_by_key(|b| b.state.priority);
        v
    }
}

/// What the caller supplies to `Effect::draw` (`Effect::DrawInfo`).
#[derive(Clone, Debug)]
pub struct DrawInfo {
    pub parent_to_world: Mat,
    pub state: MlrState,
    /// `MLRClipper::GetCameraToWorldMatrix`: camera-facing cards and ribbons use it.
    pub camera_to_world: Mat,
}

impl DrawInfo {
    pub fn new(parent_to_world: Mat, camera_to_world: Mat) -> DrawInfo {
        DrawInfo {
            parent_to_world,
            state: MlrState::game_draw_state(),
            camera_to_world,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(priority: u32) -> DrawState {
        DrawState {
            texture: None,
            blend: BlendMode::OneOne,
            z_write: false,
            z_compare: true,
            priority,
            clamp: false,
        }
    }
    fn v(x: f32) -> DrawVertex {
        DrawVertex {
            pos: [x, 0.0, 0.0],
            uv: [0.0, 0.0],
            color: [1.0; 4],
        }
    }

    #[test]
    fn push_merges_equal_states_and_rebases_indices() {
        let mut l = DrawList::default();
        l.push(&st(2), &[v(0.0), v(1.0), v(2.0)], &[0, 1, 2]);
        l.push(&st(2), &[v(3.0), v(4.0), v(5.0)], &[0, 1, 2]);
        assert_eq!(l.batches.len(), 1);
        assert_eq!(l.batches[0].indices, vec![0, 1, 2, 3, 4, 5]);
        l.push(&st(0), &[v(6.0), v(7.0), v(8.0)], &[0, 1, 2]);
        assert_eq!(l.batches.len(), 2);
        assert_eq!(l.triangle_count(), 3);
        assert_eq!(l.sorted()[0].state.priority, 0);
    }

    #[test]
    fn ground_fade_batches_do_not_merge_with_regular_effects() {
        let mut list = DrawList::default();
        let state = DrawState {
            texture: None,
            blend: BlendMode::AlphaInvAlpha,
            z_write: false,
            z_compare: true,
            priority: 2,
            clamp: false,
        };
        let vertices = [
            DrawVertex {
                pos: [0.0; 3],
                uv: [0.0; 2],
                color: [1.0; 4],
            },
            DrawVertex {
                pos: [1.0; 3],
                uv: [0.0; 2],
                color: [1.0; 4],
            },
            DrawVertex {
                pos: [2.0; 3],
                uv: [0.0; 2],
                color: [1.0; 4],
            },
        ];
        list.ground_fade = true;
        list.push(&state, &vertices, &[0, 1, 2]);
        list.ground_fade = false;
        list.push(&state, &vertices, &[0, 1, 2]);

        assert_eq!(list.batches.len(), 2);
        assert!(list.batches[0].ground_fade);
        assert!(!list.batches[1].ground_fade);
    }
}
