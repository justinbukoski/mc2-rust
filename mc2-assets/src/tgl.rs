// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
use crate::{AssetError, Reader};

pub const CURRENT_SHAPE_VERSION: u32 = 0xBAFD_ECAF;
pub const CURRENT_ANIM_VERSION: u32 = 0xBADD_ECAF;
const NODE_ID_LEN: usize = 25;
const MAX_ITEMS: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub argb_light: u32,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Triangle {
    pub vertices: [u32; 3],
    pub texture: u32,
    pub render_flags: u32,
    pub uv: [[f32; 2]; 3],
    pub face_normal: [f32; 3],
}
#[derive(Debug, Clone, PartialEq)]
pub struct Texture {
    pub node_index: u32,
    pub gos_handle: u32,
    pub alpha: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TypeNode {
    pub node_id: String,
    pub parent_id: String,
    pub node_center: [f32; 3],
    pub relative_node_center: [f32; 3],
    pub shape: Option<TypeShape>,
    pub parent_index: Option<usize>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TypeShape {
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<Triangle>,
    pub textures: Vec<Texture>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct MultiShape {
    pub textures: Vec<NamedTexture>,
    pub nodes: Vec<TypeNode>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct NamedTexture {
    pub name: String,
    pub node_index: u32,
    pub gos_handle: u32,
    pub alpha: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub node_id: String,
    pub shape_id: i32,
    pub frame_rate: f32,
    pub tick_rate: f32,
    pub quaternions: Vec<[f32; 4]>,
    pub positions: Vec<[f32; 3]>,
    /// `numFrames` as stored (also when neither keyframe array is present).
    pub frames: u32,
}

fn count(r: &mut Reader<'_>) -> crate::reader::Result<usize> {
    let n = r.u32()? as usize;
    if n > MAX_ITEMS {
        return Err(AssetError(format!("unreasonable item count {n}")));
    }
    Ok(n)
}
fn bounded_count(r: &mut Reader<'_>, minimum_bytes: usize) -> crate::reader::Result<usize> {
    let n = count(r)?;
    if minimum_bytes != 0 && n > r.remaining() / minimum_bytes {
        return Err(AssetError(format!(
            "item count {n} exceeds remaining input"
        )));
    }
    Ok(n)
}
fn vec3(r: &mut Reader<'_>) -> crate::reader::Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}
fn fixed_id(r: &mut Reader<'_>) -> crate::reader::Result<String> {
    r.fixed_string(NODE_ID_LEN)
}
fn tiny_texture(r: &mut Reader<'_>) -> crate::reader::Result<Texture> {
    let node_index = r.u32()?;
    let gos_handle = r.u32()?;
    let alpha = r.u8()? != 0;
    r.take(3)?;
    Ok(Texture {
        node_index,
        gos_handle,
        alpha,
    })
}

pub fn decode_shape(bytes: &[u8]) -> crate::reader::Result<MultiShape> {
    let mut r = Reader::new(bytes);
    let version = r.u32()?;
    if version != CURRENT_SHAPE_VERSION {
        return Err(AssetError(format!(
            "unsupported TGL version {version:#010x}"
        )));
    }
    let node_count = bounded_count(&mut r, 78)?;
    let texture_count = bounded_count(&mut r, 268)?;
    let mut textures = Vec::new();
    textures
        .try_reserve_exact(texture_count)
        .map_err(|_| AssetError("TGL texture allocation failed".into()))?;
    for _ in 0..texture_count {
        let name = r.fixed_string(256)?;
        let node_index = r.u32()?;
        let gos_handle = r.u32()?;
        let alpha = r.u8()? != 0;
        r.take(3)?;
        textures.push(NamedTexture {
            name,
            node_index,
            gos_handle,
            alpha,
        });
    }
    let mut nodes = Vec::new();
    nodes
        .try_reserve_exact(node_count)
        .map_err(|_| AssetError("TGL node allocation failed".into()))?;
    for _ in 0..node_count {
        let kind = r.u32()?;
        let shape = if kind == 1 {
            let nv = bounded_count(&mut r, 28)?;
            let mut vertices = Vec::new();
            vertices
                .try_reserve_exact(nv)
                .map_err(|_| AssetError("TGL vertex allocation failed".into()))?;
            for _ in 0..nv {
                vertices.push(Vertex {
                    position: vec3(&mut r)?,
                    normal: vec3(&mut r)?,
                    argb_light: r.u32()?,
                });
            }
            let nt = bounded_count(&mut r, 56)?;
            let mut triangles = Vec::new();
            triangles
                .try_reserve_exact(nt)
                .map_err(|_| AssetError("TGL triangle allocation failed".into()))?;
            for _ in 0..nt {
                let vertices = [r.u32()?, r.u32()?, r.u32()?];
                if vertices.iter().any(|&index| index >= nv as u32) {
                    return Err(AssetError("TGL triangle vertex index out of range".into()));
                }
                let texture = r.u32()?;
                let render_flags = r.u32()?;
                let uv = [
                    [r.f32()?, r.f32()?],
                    [r.f32()?, r.f32()?],
                    [r.f32()?, r.f32()?],
                ];
                let face_normal = vec3(&mut r)?;
                triangles.push(Triangle {
                    vertices,
                    texture,
                    render_flags,
                    uv,
                    face_normal,
                });
            }
            let ntex = bounded_count(&mut r, 12)?;
            let mut tx = Vec::new();
            tx.try_reserve_exact(ntex)
                .map_err(|_| AssetError("TGL shape texture allocation failed".into()))?;
            for _ in 0..ntex {
                tx.push(tiny_texture(&mut r)?);
            }
            Some(TypeShape {
                vertices,
                triangles,
                textures: tx,
            })
        } else if kind == 0 {
            None
        } else {
            return Err(AssetError(format!("unknown TGL node kind {kind}")));
        };
        let node_center = vec3(&mut r)?;
        let relative_node_center = vec3(&mut r)?;
        let node_id = fixed_id(&mut r)?;
        let parent_id = fixed_id(&mut r)?;
        nodes.push(TypeNode {
            node_id,
            parent_id,
            node_center,
            relative_node_center,
            shape,
            parent_index: None,
        });
    }
    for i in 0..nodes.len() {
        if !nodes[i].parent_id.is_empty() {
            nodes[i].parent_index = nodes
                .iter()
                .position(|n| n.node_id.eq_ignore_ascii_case(&nodes[i].parent_id));
        }
    }
    if r.remaining() != 0 {
        return Err(AssetError(format!(
            "{} trailing bytes in TGL",
            r.remaining()
        )));
    }
    Ok(MultiShape { textures, nodes })
}

pub fn decode_animation(bytes: &[u8]) -> crate::reader::Result<Vec<Animation>> {
    let mut r = Reader::new(bytes);
    let version = r.u32()?;
    if version != CURRENT_ANIM_VERSION {
        return Err(AssetError(format!(
            "unsupported AGL version {version:#010x}"
        )));
    }
    let n = bounded_count(&mut r, 49)?;
    let mut out = Vec::new();
    out.try_reserve_exact(n)
        .map_err(|_| AssetError("AGL animation allocation failed".into()))?;
    for _ in 0..n {
        let node_id = fixed_id(&mut r)?;
        let shape_id = r.i32()?;
        let frames = count(&mut r)?;
        let frame_rate = r.f32()?;
        let tick_rate = r.f32()?;
        let quat_bytes = r.i32()?;
        let pos_bytes = r.i32()?;
        let expected_q = frames
            .checked_mul(16)
            .ok_or_else(|| AssetError("animation size overflow".into()))?;
        let expected_p = frames
            .checked_mul(12)
            .ok_or_else(|| AssetError("animation size overflow".into()))?;
        if quat_bytes < 0
            || pos_bytes < 0
            || (quat_bytes as usize != 0 && quat_bytes as usize != expected_q)
            || (pos_bytes as usize != 0 && pos_bytes as usize != expected_p)
        {
            return Err(AssetError("invalid AGL keyframe byte lengths".into()));
        }
        let payload_bytes = usize::try_from(quat_bytes)
            .unwrap_or(0)
            .checked_add(usize::try_from(pos_bytes).unwrap_or(0))
            .ok_or_else(|| AssetError("animation size overflow".into()))?;
        if payload_bytes > r.remaining() {
            return Err(AssetError(
                "AGL keyframe data exceeds remaining input".into(),
            ));
        }
        let mut quaternions = Vec::new();
        if quat_bytes != 0 {
            quaternions
                .try_reserve_exact(frames)
                .map_err(|_| AssetError("AGL quaternion allocation failed".into()))?;
            for _ in 0..frames {
                quaternions.push([r.f32()?, r.f32()?, r.f32()?, r.f32()?]);
            }
        }
        let mut positions = Vec::new();
        if pos_bytes != 0 {
            positions
                .try_reserve_exact(frames)
                .map_err(|_| AssetError("AGL position allocation failed".into()))?;
            for _ in 0..frames {
                positions.push(vec3(&mut r)?);
            }
        }
        out.push(Animation {
            node_id,
            shape_id,
            frame_rate,
            tick_rate,
            quaternions,
            positions,
            frames: frames as u32,
        });
    }
    if r.remaining() != 0 {
        return Err(AssetError(format!(
            "{} trailing bytes in AGL",
            r.remaining()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape_nodes_and_explicit_padding() {
        let mut b = Vec::new();
        b.extend(CURRENT_SHAPE_VERSION.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend(0u32.to_le_bytes());
        b.extend([0; 24]);
        let mut id = [0; 25];
        id[..4].copy_from_slice(b"root");
        b.extend(id);
        b.extend([0; 25]);
        let s = decode_shape(&b).unwrap();
        assert_eq!(s.nodes[0].node_id, "root");
    }

    #[test]
    fn one_named_texture_without_nodes_fits_268_byte_record() {
        let mut b = CURRENT_SHAPE_VERSION.to_le_bytes().to_vec();
        b.extend(0u32.to_le_bytes()); // nodes
        b.extend(1u32.to_le_bytes()); // named textures
        let mut name = [0; 256];
        name[..6].copy_from_slice(b"stone\0");
        b.extend(name);
        b.extend(7u32.to_le_bytes()); // node index
        b.extend(11u32.to_le_bytes()); // gos handle
        b.push(1); // alpha
        b.extend([0; 3]); // bool tail padding

        let shape = decode_shape(&b).unwrap();
        assert!(shape.nodes.is_empty());
        assert_eq!(shape.textures.len(), 1);
        assert_eq!(shape.textures[0].name, "stone");
        assert_eq!(shape.textures[0].node_index, 7);
        assert_eq!(shape.textures[0].gos_handle, 11);
        assert!(shape.textures[0].alpha);
    }
    #[test]
    fn animation_reads_frame_arrays() {
        let mut b = Vec::new();
        b.extend(CURRENT_ANIM_VERSION.to_le_bytes());
        b.extend(1u32.to_le_bytes());
        let mut id = [0; 25];
        id[..4].copy_from_slice(b"root");
        b.extend(id);
        b.extend((-1i32).to_le_bytes());
        b.extend(1u32.to_le_bytes());
        b.extend(30f32.to_le_bytes());
        b.extend(60f32.to_le_bytes());
        b.extend(16i32.to_le_bytes());
        b.extend(12i32.to_le_bytes());
        for v in [0f32, 0., 0., 1., 2., 3., 4.] {
            b.extend(v.to_le_bytes());
        }
        let a = decode_animation(&b).unwrap();
        assert_eq!(a[0].positions[0], [2., 3., 4.]);
    }
    #[test]
    fn rejects_declared_texture_count_before_allocating() {
        let mut b = CURRENT_SHAPE_VERSION.to_le_bytes().to_vec();
        b.extend(0u32.to_le_bytes());
        b.extend(2_000_000u32.to_le_bytes());
        assert!(decode_shape(&b).is_err());
    }

    #[test]
    fn rejects_triangle_vertex_indices_outside_vertex_array() {
        let mut b = Vec::new();
        b.extend(CURRENT_SHAPE_VERSION.to_le_bytes());
        b.extend(1u32.to_le_bytes()); // nodes
        b.extend(0u32.to_le_bytes()); // named textures
        b.extend(1u32.to_le_bytes()); // node kind: shape
        b.extend(3u32.to_le_bytes());
        for _ in 0..3 {
            for value in [0f32; 6] {
                b.extend(value.to_le_bytes());
            }
            b.extend(0u32.to_le_bytes());
        }
        b.extend(1u32.to_le_bytes()); // triangles
        b.extend([0u32, 1, 3].into_iter().flat_map(u32::to_le_bytes));
        b.extend(0u32.to_le_bytes()); // texture
        b.extend(0u32.to_le_bytes()); // flags
        for _ in 0..9 {
            b.extend(0f32.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes()); // shape textures
        b.extend([0; 24]); // centers
        b.extend([0; 25]); // node id
        b.extend([0; 25]); // parent id
        assert!(decode_shape(&b).is_err());
    }
}
