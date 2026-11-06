// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
use crate::{tga::RgbaImage, AssetError, Reader};

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainVertex {
    pub normal: [f32; 3],
    pub elevation: f32,
    pub texture_data: u32,
    pub local_rgb_light: u32,
    pub terrain_type: u32,
    pub selected: u8,
    pub water: u8,
    pub shadow: u8,
    pub highlighted: u8,
}
#[derive(Debug, Clone, PartialEq)]
pub struct TerrainObject {
    pub object_type: i32,
    pub position: [f32; 3],
    pub rotation: f32,
    pub damage: i32,
    pub team_id: i32,
    pub parent_id: i32,
    pub padding: [i32; 2],
}
#[derive(Debug, Clone, PartialEq)]
pub struct TacMapBuilding {
    pub x: i32,
    pub y: i32,
}
#[derive(Debug, Clone, PartialEq)]
pub enum MissionPacket {
    Terrain {
        side: usize,
        vertices: Vec<TerrainVertex>,
    },
    Objects(Vec<TerrainObject>),
    TacMapBuildings(Vec<TacMapBuilding>),
    TacMapImage(RgbaImage),
}
fn count(r: &mut Reader<'_>, limit: usize) -> crate::reader::Result<usize> {
    let n = r.u32()? as usize;
    if n > limit {
        return Err(AssetError(format!("unreasonable packet count {n}")));
    }
    Ok(n)
}
fn vec3(r: &mut Reader<'_>) -> crate::reader::Result<[f32; 3]> {
    Ok([r.f32()?, r.f32()?, r.f32()?])
}

pub fn decode_packet(index: u32, bytes: &[u8]) -> crate::reader::Result<MissionPacket> {
    match index {
        0 => {
            if !bytes.len().is_multiple_of(32) {
                return Err(AssetError(
                    "terrain packet length is not a multiple of 32".into(),
                ));
            }
            let n = bytes.len() / 32;
            let side = (n as f64).sqrt() as usize;
            if side * side != n {
                return Err(AssetError(
                    "terrain vertex count is not a square grid".into(),
                ));
            }
            let mut r = Reader::new(bytes);
            let mut vertices = Vec::with_capacity(n);
            for _ in 0..n {
                vertices.push(TerrainVertex {
                    normal: vec3(&mut r)?,
                    elevation: r.f32()?,
                    texture_data: r.u32()?,
                    local_rgb_light: r.u32()?,
                    terrain_type: r.u32()?,
                    selected: r.u8()?,
                    water: r.u8()?,
                    shadow: r.u8()?,
                    highlighted: r.u8()?,
                });
            }
            Ok(MissionPacket::Terrain { side, vertices })
        }
        1 => {
            let mut r = Reader::new(bytes);
            let n = count(&mut r, 100_000)?;
            if r.remaining() != n * 40 {
                return Err(AssetError(format!(
                    "object packet size mismatch: {n} records, {} bytes",
                    r.remaining()
                )));
            }
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                out.push(TerrainObject {
                    object_type: r.i32()?,
                    position: vec3(&mut r)?,
                    rotation: r.f32()?,
                    damage: r.i32()?,
                    team_id: r.i32()?,
                    parent_id: r.i32()?,
                    padding: [r.i32()?, r.i32()?],
                });
            }
            Ok(MissionPacket::Objects(out))
        }
        2 => {
            let mut r = Reader::new(bytes);
            let n = count(&mut r, 100_000)?;
            if r.remaining() != n * 8 {
                return Err(AssetError("tac-map building packet size mismatch".into()));
            }
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                out.push(TacMapBuilding {
                    x: r.i32()?,
                    y: r.i32()?,
                });
            }
            Ok(MissionPacket::TacMapBuildings(out))
        }
        3 => Ok(MissionPacket::TacMapImage(crate::tga::decode(bytes)?)),
        _ => Err(AssetError(format!(
            "packet {index} is owned by mc2-path or a later system"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_terrain_and_typed_objects() {
        let mut b = Vec::new();
        for elevation in [4f32, 4., 4., 4.] {
            for f in [1f32, 2., 3., elevation] {
                b.extend(f.to_le_bytes());
            }
            for n in [5u32, 6, 7] {
                b.extend(n.to_le_bytes());
            }
            b.extend([1, 2, 3, 4]);
        }
        if let MissionPacket::Terrain { side, vertices } = decode_packet(0, &b).unwrap() {
            assert_eq!(side, 2);
            assert_eq!(vertices[1].elevation, 4.)
        } else {
            panic!()
        }
    }
    #[test]
    fn objects_are_40_byte_records() {
        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend(7i32.to_le_bytes());
        for f in [1f32, 2., 3., 0.5] {
            b.extend(f.to_le_bytes());
        }
        for n in [4i32, 5, 6, 0, 0] {
            b.extend(n.to_le_bytes());
        }
        assert!(matches!(decode_packet(1,&b).unwrap(),MissionPacket::Objects(v) if v.len()==1));
    }
}
