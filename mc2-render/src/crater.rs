// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Craters and footprints (`CraterManager::render`, `mclib/crater.cpp`): terrain-projected decals.
//!
//! The simulation raises `SimEvent::Crater`; a `mc2_sim::crater::CraterPool` keeps the decals exactly like the C++ list (a
//! ring of 1000, the oldest replaced; no lifetime or fade). Here the pool becomes two [`FxBatch`]es: bomb craters from
//! `defaults/feet0000.tga` and footprints from `defaults/feet0001.tga`, each quad two triangles over the four ground corners with
//! the C++ texture coordinates (`craterUVTable`, extent 0.5 for a crater, 0.125 for a footprint) and the scene's ambient light as
//! the vertex colour (`lightRGB`). They are drawn in the scene pass with alpha blending, depth tested against the terrain and not
//! writing depth. Distance haze and fog are the scene's own.

use crate::fx::{FxBatch, FxBlend, FxTextureId, FxVertex};

/// `data/textures/defaults/feet0000.tga` (the crater texture, `gos_Texture_Keyed`).
pub const CRATER_TEXTURE: &str = "data/textures/defaults/feet0000.tga";
/// `data/textures/defaults/feet0001.tga` (the footprint texture, `gos_Texture_Alpha`).
pub const FOOTPRINT_TEXTURE: &str = "data/textures/defaults/feet0001.tga";

/// How far above the sampled ground the decal corners are drawn, in world units. The C++ projects the corners at the terrain
/// elevation and lets its terrain flag bias the depth; the decal quad spans a coarser surface than the terrain mesh, and this
/// lift keeps it from sinking into the slope between its corners.
pub const GROUND_LIFT: f32 = 1.0;

/// One decal as the renderer needs it (the app builds it from `mc2_sim::crater::Crater`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Decal {
    /// A bomb crater (`feet0000`) rather than a footprint (`feet0001`).
    pub crater: bool,
    /// The four ground corners in game world coordinates, `z` on the terrain.
    pub corners: [[f32; 3]; 4],
    /// Lower left texture coordinate of the shape.
    pub uv: [f32; 2],
    /// Width and height of the shape in the texture (0.5 for a crater, 0.125 for a footprint).
    pub extent: f32,
}

/// The uploaded decal textures; `None` draws that kind untextured (an opaque dark quad is never wanted, so such decals are
/// simply left out).
#[derive(Copy, Clone, Debug, Default)]
pub struct CraterTextures {
    /// `feet0000`.
    pub crater: Option<FxTextureId>,
    /// `feet0001`.
    pub footprint: Option<FxTextureId>,
}

/// Draw data for `decals`, lit by `ambient` (0..1 per channel). Empty when no decal has a texture.
pub fn crater_batches<'a>(
    decals: impl Iterator<Item = &'a Decal>,
    textures: CraterTextures,
    ambient: [f32; 3],
) -> Vec<FxBatch> {
    let mut craters = Vec::new();
    let mut prints = Vec::new();
    for d in decals {
        if d.crater {
            craters.push(d);
        } else {
            prints.push(d);
        }
    }
    [(craters, textures.crater), (prints, textures.footprint)]
        .into_iter()
        .filter_map(|(list, texture)| {
            let texture = texture?;
            if list.is_empty() {
                return None;
            }
            Some(batch(&list, texture, ambient))
        })
        .collect()
}

fn batch(list: &[&Decal], texture: FxTextureId, ambient: [f32; 3]) -> FxBatch {
    let color = [ambient[0], ambient[1], ambient[2], 1.0];
    let mut vertices = Vec::with_capacity(list.len() * 4);
    let mut indices = Vec::with_capacity(list.len() * 6);
    for d in list {
        let ([u, v], add) = (d.uv, d.extent);
        // gVertex[0..2] / sVertex: corner 0 at (u, v), 1 at (u + add, v), 2 at (u + add, v + add), 3 at (u, v + add)
        let uvs = [[u, v], [u + add, v], [u + add, v + add], [u, v + add]];
        let base = vertices.len() as u32;
        for (c, uv) in d.corners.iter().zip(uvs) {
            vertices.push(FxVertex {
                pos: [c[0], c[1], c[2] + GROUND_LIFT],
                uv,
                color,
            });
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    FxBatch {
        texture: Some(texture),
        blend: FxBlend::Alpha,
        z_write: false,
        z_compare: true,
        clamp: true,
        ground_fade: false,
        // under the effects (smoke and fire draw over a scorch)
        priority: 0,
        vertices,
        indices,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decal(crater: bool) -> Decal {
        let (s, uv, extent) = if crater {
            (32.0, [0.0, 0.5], 0.5)
        } else {
            (16.0, [0.125, 0.125], 0.125)
        };
        Decal {
            crater,
            corners: [
                [10.0 - s, 20.0 - s, 5.0],
                [10.0 + s, 20.0 - s, 5.0],
                [10.0 + s, 20.0 + s, 5.0],
                [10.0 - s, 20.0 + s, 5.0],
            ],
            uv,
            extent,
        }
    }

    #[test]
    fn a_decal_is_a_quad_with_the_table_coordinates() {
        let list = [decal(true), decal(false)];
        let t = CraterTextures {
            crater: Some(FxTextureId(1)),
            footprint: Some(FxTextureId(2)),
        };
        let b = crater_batches(list.iter(), t, [0.5, 0.5, 0.5]);
        assert_eq!(b.len(), 2);
        let (c, f) = (&b[0], &b[1]);
        assert_eq!(c.texture, Some(FxTextureId(1)));
        assert_eq!(c.indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(c.vertices[0].uv, [0.0, 0.5]);
        assert_eq!(c.vertices[2].uv, [0.5, 1.0]);
        assert_eq!(
            c.vertices[0].pos,
            [10.0 - 32.0, 20.0 - 32.0, 5.0 + GROUND_LIFT]
        );
        assert_eq!(f.texture, Some(FxTextureId(2)));
        assert_eq!(f.vertices[0].uv, [0.125, 0.125]);
        assert_eq!(f.vertices[1].uv, [0.25, 0.125]);
        assert!(crater_batches(list.iter(), CraterTextures::default(), [1.0; 3]).is_empty());
    }
}
