//! Original art pack: glTF 2.0 (`.glb`) replacements for TGL shapes.
//!
//! A pack is a folder holding `<ShapeName>.glb`.  [`load_pack_model`] turns one into a
//! [`ShapeModel`] that keeps the *original* shape's node list (names, hierarchy, pivots), bounds and
//! extents, so animation clips, hardpoints, picking and selection are unchanged; only the geometry and
//! the paint-mask texture come from the pack.  Every node of the original shape must exist in the glb
//! (matched by name, case-insensitively); a mesh on a glb node is attached to the same-named original
//! node, in that node's local frame (the model-space position minus the node's absolute pivot).
//! See `rust/docs/ART-PACK.md`.

use crate::shape::{Batch, NodeKind, ShapeModel, ShapeVertex};
use crate::{RenderError, Result};
use std::path::{Path, PathBuf};

/// An RGBA8 texture embedded in a pack model.
#[derive(Clone, Debug)]
pub struct PackTexture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Path of the pack file for `shape` inside `dir`.
pub fn pack_path(dir: &Path, shape: &str) -> PathBuf {
    dir.join(format!("{shape}.glb"))
}

fn err(shape: &str, what: impl std::fmt::Display) -> RenderError {
    RenderError::Other(format!("art pack {shape}.glb: {what}"))
}

/// Loads `<dir>/<shape>.glb` over `original`; `Ok(None)` when the pack has no model for the shape.
/// A file that exists but breaks the node contract is an error, never a silent fallback.
pub fn load_pack_model(
    dir: &Path,
    shape: &str,
    original: &ShapeModel,
) -> Result<Option<(ShapeModel, Vec<PackTexture>)>> {
    let path = pack_path(dir, shape);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(err(shape, e)),
    };
    pack_model_from_glb(shape, &bytes, original).map(Some)
}

/// The pure form of [`load_pack_model`], over the bytes of a glb.
pub fn pack_model_from_glb(
    shape: &str,
    bytes: &[u8],
    original: &ShapeModel,
) -> Result<(ShapeModel, Vec<PackTexture>)> {
    let glb = gltf::Gltf::from_slice(bytes).map_err(|e| err(shape, e))?;
    let blob = glb
        .blob
        .as_deref()
        .ok_or_else(|| err(shape, "no BIN chunk"))?;

    // Node contract: every original node exists in the pack.
    let index_of = |name: &str| original.node_index(name);
    let mut present = vec![false; original.nodes.len()];
    for n in glb.nodes() {
        if let Some(i) = n.name().and_then(index_of) {
            present[i] = true;
        }
    }
    if let Some(i) = present.iter().position(|p| !p) {
        return Err(err(shape, format!("missing node {}", original.nodes[i].id)));
    }

    let mut vertices: Vec<ShapeVertex> = Vec::new();
    for n in glb.nodes() {
        let (Some(mesh), Some(ni)) = (n.mesh(), n.name().and_then(index_of)) else {
            continue;
        };
        for prim in mesh.primitives() {
            if prim.mode() != gltf::mesh::Mode::Triangles {
                return Err(err(shape, "only triangle lists are supported"));
            }
            let reader = prim.reader(|b| match b.source() {
                gltf::buffer::Source::Bin => Some(blob),
                _ => None,
            });
            let pos: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| err(shape, "primitive without POSITION"))?
                .collect();
            let nrm: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|i| i.collect())
                .unwrap_or_else(|| vec![[0.0, 1.0, 0.0]; pos.len()]);
            let uv: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|i| i.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; pos.len()]);
            if nrm.len() != pos.len() || uv.len() != pos.len() {
                return Err(err(shape, "attribute counts differ"));
            }
            let indices: Vec<u32> = match reader.read_indices() {
                Some(i) => i.into_u32().collect(),
                None => (0..pos.len() as u32).collect(),
            };
            if !indices.len().is_multiple_of(3) || indices.iter().any(|&i| i as usize >= pos.len())
            {
                return Err(err(shape, "bad index list"));
            }
            for &i in &indices {
                let i = i as usize;
                vertices.push(ShapeVertex {
                    position: pos[i],
                    node: ni as u32,
                    normal: nrm[i],
                    argb: 0xffff_ffff,
                    uv: uv[i],
                });
            }
        }
    }
    if vertices.is_empty() {
        return Err(err(shape, "no geometry"));
    }

    // One embedded texture (the paint mask): the first image of the file.
    let image = glb.images().next().ok_or_else(|| err(shape, "no image"))?;
    let png_bytes = match image.source() {
        gltf::image::Source::View { view, .. } => {
            let start = view.offset();
            blob.get(start..start + view.length())
                .ok_or_else(|| err(shape, "image view out of range"))?
        }
        gltf::image::Source::Uri { .. } => return Err(err(shape, "external images unsupported")),
    };
    let texture = decode_png(shape, png_bytes)?;

    let mut model = original.clone();
    model.texture_names = vec![format!("artpack/{}", shape.to_ascii_lowercase())];
    model.texture_alpha = vec![false];
    model.batches = vec![Batch {
        texture: 0,
        alpha: false,
        kind: NodeKind::Normal,
        first_vertex: 0,
        vertex_count: vertices.len() as u32,
    }];
    model.vertices = vertices;
    // bounds, cone_extent, building_bounds and gestures stay the original's: picking, selection and
    // animation binding are gameplay, not art.
    Ok((model, vec![texture]))
}

fn decode_png(shape: &str, bytes: &[u8]) -> Result<PackTexture> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().map_err(|e| err(shape, e))?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| err(shape, "png size"))?
    ];
    let info = reader.next_frame(&mut buf).map_err(|e| err(shape, e))?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => return Err(err(shape, format!("unsupported png colour {other:?}"))),
    };
    Ok(PackTexture {
        width: info.width,
        height: info.height,
        rgba,
    })
}
