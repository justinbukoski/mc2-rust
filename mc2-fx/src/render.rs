// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The glue between effect draw data and `mc2-render`'s drawing hook.
//!
//! `mc2-fx` produces [`DrawList`]s in the `Stuff` frame with textures named by file. This module
//! converts them to [`FxBatch`]es (game world coordinates, texture ids) and loads the effect
//! textures from the game data. It holds no GPU state of its own.

use crate::draw::{DrawBatch, DrawList};
use crate::math::{Mat, Vec3};
use crate::state::{BlendMode, TextureRef};
use mc2_formats::vfs::Vfs;
use mc2_render::camera::GameCamera;
use mc2_render::fx::{FxBatch, FxBlend, FxTextureId, FxVertex};
use mc2_render::scene::SceneRenderer;
use std::collections::HashMap;

/// Where the game keeps effect textures (`TGAFilePool("data\\tgl\\128\\")` in `mechcmd2.cpp`).
pub const TEXTURE_DIR: &str = "data/tgl/128/";

/// `Stuff` frame -> game world: `x = -x_s`, `y = z_s`, `z = y_s` (the swap the C++ writes out at
/// every call site; its own inverse).
pub fn stuff_to_world(p: [f32; 3]) -> [f32; 3] {
    [-p[0], p[2], p[1]]
}

/// The camera-to-world matrix gosFX wants (`GetCameraToWorldMatrix`), in the `Stuff` frame.
pub fn camera_to_world(camera: &GameCamera) -> Mat {
    let rows = camera.camera_to_world_stuff();
    let mut m = Mat::IDENTITY;
    m.m = rows;
    m
}

pub fn blend_of(b: BlendMode) -> FxBlend {
    match b {
        BlendMode::OneZero => FxBlend::Opaque,
        BlendMode::OneOne => FxBlend::Additive,
        BlendMode::AlphaOne => FxBlend::AlphaAdditive,
        BlendMode::OneAlpha => FxBlend::OneAlpha,
        BlendMode::AlphaInvAlpha => FxBlend::Alpha,
        BlendMode::OneInvAlpha => FxBlend::Premultiplied,
    }
}

/// Uploads one RGBA8 texture and returns its id (a `SceneRenderer::add_fx_texture` or
/// `FxRenderer::add_texture` call).
pub type Upload<'a> = &'a mut dyn FnMut(&str, u32, u32, &[u8]) -> FxTextureId;

/// Caches the texture ids of effect textures loaded from the game data.
#[derive(Default)]
pub struct FxTextures {
    ids: HashMap<String, Option<FxTextureId>>,
}

impl FxTextures {
    /// The id for `tex`, loading and uploading `data/tgl/128/<name>.tga` the first time. `None`
    /// (drawn untextured) when the file is missing or unreadable.
    pub fn resolve(
        &mut self,
        vfs: &mut Vfs,
        tex: &TextureRef,
        upload: Upload,
    ) -> Option<FxTextureId> {
        if let Some(id) = self.ids.get(&tex.name) {
            return *id;
        }
        let id = load_texture(vfs, &tex.name, upload);
        self.ids.insert(tex.name.clone(), id);
        id
    }
}

fn load_texture(vfs: &mut Vfs, name: &str, upload: Upload) -> Option<FxTextureId> {
    let path = format!("{TEXTURE_DIR}{name}.tga");
    let bytes = vfs.read(&path).ok()??;
    let img = mc2_assets::tga::decode(&bytes).ok()?;
    Some(upload(
        &path,
        img.width as u32,
        img.height as u32,
        &img.pixels,
    ))
}

/// Converts a draw list (Stuff frame) to renderer batches (game world). `texture` maps an
/// effect texture to its registered id.
pub fn to_fx_batches(
    list: &DrawList,
    mut texture: impl FnMut(&TextureRef) -> Option<FxTextureId>,
) -> Vec<FxBatch> {
    list.batches
        .iter()
        .map(|b: &DrawBatch| {
            let blend = blend_of(b.state.blend);
            FxBatch {
                texture: b.state.texture.as_ref().and_then(&mut texture),
                blend,
                z_write: b.state.z_write,
                z_compare: b.state.z_compare,
                clamp: b.state.clamp,
                ground_fade: b.ground_fade,
                priority: b.state.priority,
                vertices: b
                    .vertices
                    .iter()
                    .map(|v| FxVertex {
                        pos: stuff_to_world(v.pos),
                        uv: v.uv,
                        color: v.color,
                    })
                    .collect(),
                indices: b.indices.clone(),
            }
        })
        .collect()
}

/// Draw data for the renderer, loading any new textures on the way.
pub fn batches(
    list: &DrawList,
    vfs: &mut Vfs,
    textures: &mut FxTextures,
    upload: Upload,
) -> Vec<FxBatch> {
    let mut ids: HashMap<TextureRef, Option<FxTextureId>> = HashMap::new();
    for b in &list.batches {
        if let Some(t) = &b.state.texture {
            if !ids.contains_key(t) {
                let id = textures.resolve(vfs, t, upload);
                ids.insert(t.clone(), id);
            }
        }
    }
    to_fx_batches(list, |t| ids.get(t).copied().flatten())
}

/// [`batches`] for a [`SceneRenderer`].
pub fn batches_for_scene(
    list: &DrawList,
    scene: &mut SceneRenderer,
    vfs: &mut Vfs,
    textures: &mut FxTextures,
) -> Vec<FxBatch> {
    batches(list, vfs, textures, &mut |n, w, h, px| {
        scene.add_fx_texture(n, w, h, px)
    })
}

/// A point in the `Stuff` frame from a game world position.
pub fn world_to_stuff(x: f32, y: f32, z: f32) -> Vec3 {
    crate::system::game_to_fx(x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw::{DrawState, DrawVertex};

    #[test]
    fn frame_swap_is_its_own_inverse() {
        let p = [1.0, 2.0, 3.0];
        assert_eq!(stuff_to_world(stuff_to_world(p)), p);
        assert_eq!(stuff_to_world([1.0, 2.0, 3.0]), [-1.0, 3.0, 2.0]);
    }

    #[test]
    fn batches_carry_state_and_world_positions() {
        let mut list = DrawList::default();
        let st = DrawState {
            texture: None,
            blend: BlendMode::OneOne,
            z_write: false,
            z_compare: true,
            priority: 7,
            clamp: true,
        };
        let v = |x| DrawVertex {
            pos: [x, 1.0, 2.0],
            uv: [0.5, 0.5],
            color: [1.0, 0.5, 0.25, 1.0],
        };
        list.push(&st, &[v(0.0), v(1.0), v(2.0)], &[0, 1, 2]);
        let out = to_fx_batches(&list, |_| None);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].blend, FxBlend::Additive);
        assert_eq!(out[0].priority, 7);
        assert!(!out[0].ground_fade);
        assert!(out[0].clamp && !out[0].z_write && out[0].z_compare);
        // (x, y, z) in Stuff -> (-x, z, y) in world
        assert_eq!(out[0].vertices[1].pos, [-1.0, 2.0, 1.0]);
        assert_eq!(out[0].indices, vec![0, 1, 2]);
    }
}
