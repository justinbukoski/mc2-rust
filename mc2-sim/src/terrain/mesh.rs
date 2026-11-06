// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Ground shape queries: elevation, normal and angle of the terrain mesh.
//!
//! Ports of `MapData::terrainElevation` (mapdata.cpp:1584-1806), `MapData::terrainNormal` (1383-1581) and
//! `MapData::terrainAngle` (1070-1290). All three find the quad of four vertices under the position, split it into two
//! triangles along a diagonal that alternates in a checkerboard (keyed by the parity of the vertex's mesh coordinates:
//! `topLeftVertex` cancels out of the parity test (`(x - tl & 1) ^ (tl & 1) == x & 1`), so the quad is split `BOTTOMRIGHT` when `(x & 1) == (y & 1)` and `BOTTOMLEFT`
//! otherwise), and work with the plane of the triangle the position falls in.
//!
//! Float rules: everything is `f32` (`Stuff::Scalar`), exactly as the C++ expressions are ordered; there is no fused
//! multiply-add. `Stuff::Vector3D::Normalize` multiplies by `1.0f / length`, not divides. There is no double
//! intermediate: `acos` is the float overload (`acosf`).

use super::{
    Terrain, ONE_OVER_WORLD_UNITS_PER_VERTEX, VERTICES_BLOCK_SIDE, WORLD_UNITS_PER_VERTEX,
};
use crate::math::Vec3;

/// `RADS_TO_DEGREES` (dstd.h:34).
pub const RADS_TO_DEGREES: f32 = 57.295_78;

const WUPV: f32 = WORLD_UNITS_PER_VERTEX;

fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

/// `Vector3D::Subtract(a, b)`.
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    v3(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// `Vector3D::Negate`.
fn negate(v: Vec3) -> Vec3 {
    v3(-v.x, -v.y, -v.z)
}

/// `Vector3D::Normalize`: `len = GetLength(); len = 1.0f / len; v * len` (`Stuff::Sqrt` is the float of the C `sqrt`).
fn normalize(v: Vec3) -> Vec3 {
    let len_sq = v.x * v.x + v.y * v.y + v.z * v.z;
    let len = len_sq.sqrt();
    let len = 1.0f32 / len;
    v3(v.x * len, v.y * len, v.z * len)
}

/// `Vector3D::Cross` (right handed: `LEFT_HANDED_COORDINATES` is not defined).
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    v3(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

/// The lattice point at or above-left of a position: `upperLeft` (world units) and `_upperLeft` (in vertices), the
/// first lines shared by all three functions (e.g. mapdata.cpp:1608-1619).
fn upper_left(p: &Vec3) -> ((f32, f32), (f32, f32)) {
    let mut ulx = (p.x * ONE_OVER_WORLD_UNITS_PER_VERTEX).floor();
    ulx *= WUPV;
    let qy = p.y * ONE_OVER_WORLD_UNITS_PER_VERTEX;
    let mut uly = qy.floor();
    if qy != uly {
        uly += 1.0;
    }
    uly *= WUPV;
    // _upperLeft.Multiply(upperLeft, float(oneOverWorldUnitsPerVertex))
    let uls = (
        ulx * ONE_OVER_WORLD_UNITS_PER_VERTEX,
        uly * ONE_OVER_WORLD_UNITS_PER_VERTEX,
    );
    ((ulx, uly), uls)
}

/// The two triangle edge vectors, the base height and the offsets `(deltaX, deltaY)` from the triangle's first vertex.
struct Triangle {
    v0: Vec3,
    v1: Vec3,
    z0: f32,
    delta_x: f32,
    delta_y: f32,
}

/// Choose the triangle under `p` (the `uvMode` block, e.g. mapdata.cpp:1675-1790). `e` are the elevations of the quad's
/// vertices 1..4 (upper left, upper right, lower right, lower left); `mesh` the quad's vertex column and row.
fn triangle(p: &Vec3, ul: (f32, f32), uls: (f32, f32), mesh: (i32, i32), e: [f32; 4]) -> Triangle {
    let tri_pos = v3(WUPV * uls.0.floor(), WUPV * uls.1.floor(), e[0]);
    // tlx = tly = 0 (`topLeftVertex` cancels out of the parity): x = meshOffset.x, y = meshOffset.y
    let bottom_right = (mesh.0 & 1) == (mesh.1 & 1);
    let (tv0, tv1, z0, delta_x, delta_y);
    if bottom_right {
        let dx = (p.x - ul.0).abs();
        let dy = (ul.1 - p.y).abs();
        if dx > dy {
            // top triangle
            let t0 = tri_pos;
            let t1 = v3(t0.x + WUPV, t0.y, e[1]);
            let t2 = v3(t1.x, t0.y - WUPV, e[2]);
            tv0 = sub(t1, t0);
            tv1 = sub(t2, t0);
            z0 = t0.z;
        } else {
            // bottom triangle
            let t0 = tri_pos;
            let t1 = v3(t0.x + WUPV, t0.y - WUPV, e[2]);
            let t2 = v3(t0.x, t1.y, e[3]);
            tv0 = sub(t2, t0);
            tv1 = sub(t1, t0);
            z0 = t0.z;
        }
        delta_x = dx;
        delta_y = dy;
    } else {
        let dx = ((ul.0 + WUPV) - p.x).abs();
        let dy = (ul.1 - p.y).abs();
        if dx > dy {
            let t1 = tri_pos;
            let t0 = v3(tri_pos.x + WUPV, tri_pos.y, e[1]);
            let t2 = v3(t1.x, t1.y - WUPV, e[3]);
            tv0 = sub(t1, t0);
            tv1 = sub(t2, t0);
            z0 = t0.z;
        } else {
            let t0 = v3(tri_pos.x + WUPV, tri_pos.y, e[1]);
            let t1 = v3(tri_pos.x, t0.y - WUPV, e[3]);
            let t2 = v3(t0.x, t1.y, e[2]);
            tv0 = sub(t2, t0);
            tv1 = sub(t1, t0);
            z0 = t0.z;
        }
        delta_x = -dx; // "Must reverse for Bottom_left triangles"
        delta_y = dy;
    }
    Triangle {
        v0: tv0,
        v1: tv1,
        z0,
        delta_x,
        delta_y,
    }
}

/// The four quad elevations at vertex column `mx`, row `my`; `None` if any index lies outside the vertex array (the C++
/// reads garbage there; callers return their out-of-range value instead).
fn quad(t: &Terrain, mx: i32, my: i32) -> Option<[f32; 4]> {
    let side = t.vertices_per_side;
    let at = |c: i32, r: i32| -> Option<f32> {
        if c < 0 || r < 0 {
            return None;
        }
        t.vertices()
            .get((c + r * side) as usize)
            .map(|v| v.elevation)
    };
    Some([
        at(mx, my)?,
        at(mx + 1, my)?,
        at(mx + 1, my + 1)?,
        at(mx, my + 1)?,
    ])
}

/// Unit normal of the triangle's plane pointing up (`Normalize`, `Cross`, flip when `z < 0`), before the final
/// normalisation.
fn plane_normal(tri: &Triangle) -> Vec3 {
    let a = normalize(tri.v0);
    let b = normalize(tri.v1);
    cross(a, b)
}

/// `getTerrainElevation(position)`: `MapData::terrainElevation`, mapdata.cpp:1584. Zero outside the map and for the last
/// row/column of vertices (`meshOffset >= verticesMapSide - 1`).
pub fn elevation(t: &Terrain, p: &Vec3) -> f32 {
    if !t.is_valid_position(p) {
        return 0.0;
    }
    let (ul, uls) = upper_left(p);
    let vertices_map_side = VERTICES_BLOCK_SIDE * t.blocks_per_side;
    let mx = (uls.0.floor() as i32) + (vertices_map_side >> 1);
    let my = (vertices_map_side >> 1) - (uls.1.floor() as i32);
    if mx >= vertices_map_side - 1 {
        return 0.0;
    }
    if my >= vertices_map_side - 1 {
        return 0.0;
    }
    let Some(e) = quad(t, mx, my) else {
        return 0.0;
    };
    let tri = triangle(p, ul, uls, (mx, my), e);
    let mut perp = plane_normal(&tri);
    // Calculate terrain elevation
    if perp.z != 0.0 {
        if perp.z < 0.0 {
            perp = negate(perp);
        }
        let perpend_x = perp.x / perp.z;
        let perpend_y = perp.y / perp.z;
        let mut result = tri.delta_x * perpend_x;
        result += (-tri.delta_y) * perpend_y;
        result = -result;
        result += tri.z0;
        result
    } else {
        0.0
    }
}

/// Shared prologue of `terrainNormal` and `terrainAngle`: the triangle under `p`, or `None` when the position is
/// invalid or its quad is not fully inside the vertex grid.
fn mesh_triangle(t: &Terrain, p: &Vec3) -> Option<Triangle> {
    if !t.is_valid_position(p) {
        return None;
    }
    let (ul, uls) = upper_left(p);
    // meshOffset = float2long(_upperLeft); `float2long` is a plain truncating cast
    let vertices_map_side = VERTICES_BLOCK_SIDE * t.blocks_per_side;
    let mx = (uls.0 as i32) + (vertices_map_side >> 1);
    let my = (vertices_map_side >> 1) - (uls.1 as i32);
    if (mx + 1) >= t.vertices_per_side || (my + 1) >= t.vertices_per_side || mx < 0 || my < 0 {
        return None;
    }
    let e = quad(t, mx, my)?;
    Some(triangle(p, ul, uls, (mx, my), e))
}

/// The normal `MapData::terrainNormal` returns (`getTerrainNormal`, mapdata.cpp:1383), which mech and vehicle state and
/// hill factors read. `(0, 0, 1)` when the position is invalid or its quad is outside the vertex grid.
pub fn normal(t: &Terrain, p: &Vec3) -> Vec3 {
    let up = v3(0.0, 0.0, 1.0);
    let Some(tri) = mesh_triangle(t, p) else {
        return up;
    };
    let mut perp = plane_normal(&tri);
    if perp.z < 0.0 {
        perp = negate(perp);
    }
    normalize(perp)
}

/// The normal `MapData::terrainAngle` writes through its out parameter, including its second normalization, as the
/// caller's `Stuff::Vector3D` holds it: `(0, 0, 0)` where the C++ returns before writing anything (a valid position in
/// the last row or column of quads; the caller's vector keeps its default-constructed zero, gvactor.cpp:2297-2298,
/// vector3d.hpp:126). Vehicle appearance pitch/roll reads this one.
pub fn angle_normal(t: &Terrain, p: &Vec3) -> Vec3 {
    angle(t, p).1.unwrap_or(v3(0.0, 0.0, 0.0))
}

/// `getTerrainAngle(position, &normal)`: `MapData::terrainAngle`, mapdata.cpp:1070. Returns the pitch of the ground in
/// degrees and the normal the C++ writes through its pointer (`None` when it does not write one: position valid but its
/// quad is outside the vertex grid; invalid positions write `(0, 0, 1)` and return 0).
pub fn angle(t: &Terrain, p: &Vec3) -> (f32, Option<Vec3>) {
    if !t.is_valid_position(p) {
        return (0.0, Some(v3(0.0, 0.0, 1.0)));
    }
    let Some(tri) = mesh_triangle(t, p) else {
        return (0.0, None);
    };
    let mut perp = plane_normal(&tri);
    if perp.z < 0.0 {
        perp = negate(perp);
    }
    perp = normalize(perp);
    // normal->Normalize(perpendicularVec): a second normalisation of the already unit vector
    let normal = normalize(perp);
    // "We ONLY want PITCH! No roll should be included!"
    perp.y = 0.0;
    // result = perpendicularVec * worldK  (x*0 + y*0 + z*1 in float)
    let dot = perp.x * 0.0f32 + perp.y * 0.0f32 + perp.z * 1.0f32;
    // `acos(float)` resolves to the float overload (`acosf`) in the oracle build (libc++), so the product with the float
    // constant is a float multiply. MSVC's overload computed `(float)acos((double)x)`, which differs in the last bit for
    // about 2% of inputs; the oracle's libm is the reference here (see rust/docs/porting-log.md, lane T).
    let result = mc2_libm::acosf(dot) * RADS_TO_DEGREES;
    (result, Some(normal))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 60x60 terrain with `h(row, col)` elevations.
    fn terrain(h: impl Fn(i32, i32) -> f32) -> Terrain {
        let side = 60;
        let mut e = Vec::new();
        for r in 0..side {
            for c in 0..side {
                e.push(h(r, c));
            }
        }
        Terrain::from_elevations(side, &e)
    }

    fn at(x: f32, y: f32) -> Vec3 {
        Vec3 { x, y, z: 0.0 }
    }

    #[test]
    fn flat_ground_is_flat() {
        let t = terrain(|_, _| 7.0);
        for (x, y) in [
            (0.5, 0.5),
            (-1000.25, 2000.75),
            (63.9, -63.9),
            (128.0, 128.0),
        ] {
            assert_eq!(t.elevation(&at(x, y)), 7.0);
            // The C++ normalises two edge vectors and crosses them: a flat triangle gives z = 0.99999994 from
            // `terrainNormal`; `terrainAngle` normalizes once more and writes z = 1.
            let n = t.normal(&at(x, y));
            assert_eq!((n.x, n.y, n.z), (0.0, 0.0, 0.999_999_94));
            assert_eq!(t.angle_normal(&at(x, y)).z, 1.0);
            let (a, an) = t.angle_and_normal(&at(x, y));
            assert_eq!(a, 0.019_782_34);
            assert_eq!(an.unwrap().z, 1.0);
        }
    }

    #[test]
    fn elevation_interpolates_planes_per_triangle() {
        // h = 2 * col (ramp rising to the east): a plane, so both triangle shapes give the same answer
        let t = terrain(|_, c| 2.0 * c as f32);
        // x = tileCol*128 - 3840; elevation(x) = 2 * (x + 3840) / 128 = (x + 3840) / 64
        for x in [-3000.0f32, -1234.5, 0.0, 77.0, 1000.0] {
            for y in [-2000.0f32, -17.5, 0.25, 1500.0] {
                let got = t.elevation(&at(x, y));
                let want = (x + 3840.0) / 64.0;
                assert!((got - want).abs() < 1e-3, "{x},{y}: {got} vs {want}");
            }
        }
    }

    #[test]
    fn diagonal_flips_with_vertex_parity() {
        // Vertex (30,30) at world (0,0) and its east neighbour (30,31) at (128,0) are raised to 100, the rest is 0.
        let t = terrain(|r, c| {
            if (r, c) == (30, 30) || (r, c) == (30, 31) {
                100.0
            } else {
                0.0
            }
        });
        // Quad (30,30) has even/even parity: its diagonal runs from the upper left (0,0) to the lower right (128,-128).
        // (96,-32) lies right of it, in the top triangle UL(100) UR(100) LR(0): the plane z = 100 + 100 y / 128.
        let top = t.elevation(&at(96.0, -32.0));
        assert!((top - 75.0).abs() < 1e-3, "{top}");
        // (32,-96) lies left of it, in the bottom triangle UL(100) LR(0) LL(0): the plane z = 100 + 100 y / 128.
        let bottom = t.elevation(&at(32.0, -96.0));
        assert!((bottom - 25.0).abs() < 1e-3, "{bottom}");
        // Quad (30,31) has even/odd parity: the diagonal runs from the upper right (256,0) to the lower left (128,-128).
        // The point (160,-32) is above that diagonal: top triangle UL(100) UR(0) LL(0) -> 100 - 25 - 25 = 50.
        let e = t.elevation(&at(160.0, -32.0));
        assert!((e - 50.0).abs() < 1e-3, "{e}");
        // The mirror position (224,-96) is below it: bottom triangle UR(0) LL(0) LR(0): flat zero.
        let e = t.elevation(&at(224.0, -96.0));
        assert!(e.abs() < 1e-3, "{e}");
    }

    #[test]
    fn edges_return_zero() {
        let t = terrain(|_, _| 3.0);
        assert_eq!(t.elevation(&at(3840.0, 0.0)), 0.0); // not a valid position
        assert_eq!(t.elevation(&at(3839.0, 0.0)), 0.0); // last column of quads: meshOffset.x >= side - 1
        assert_eq!(t.elevation(&at(0.0, -3839.0)), 0.0);
        assert_eq!(t.elevation(&at(-3839.0, 3839.0)), 3.0);
        assert_eq!(t.normal(&at(5000.0, 0.0)).z, 1.0);
        assert_eq!(
            t.angle_and_normal(&at(5000.0, 0.0)),
            (0.0, Some(v3(0.0, 0.0, 1.0)))
        );
        // valid but in the last column of quads: terrainAngle returns before it writes the normal (mapdata.cpp:1115-1120)
        assert_eq!(t.angle_and_normal(&at(3839.0, 0.0)), (0.0, None));
        assert_eq!(t.angle_normal(&at(3839.0, 0.0)), v3(0.0, 0.0, 0.0));
        // `terrainNormal` returns up there instead
        assert_eq!(t.normal(&at(3839.0, 0.0)), v3(0.0, 0.0, 1.0));
    }

    #[test]
    fn angle_is_acos_of_the_normal_z() {
        // a plane rising 128 over 128 to the east is a 45 degree slope
        let t = terrain(|_, c| 128.0 * c as f32);
        let a = t.angle_deg(&at(10.0, 10.0));
        assert!((a - 45.0).abs() < 1e-3, "{a}");
        // "We ONLY want PITCH" zeroes the y component after the dot product has been fixed to z, so a ramp rising to
        // the north reports the same angle (the zeroing has no effect on the result).
        let t = terrain(|r, _| -128.0 * r as f32);
        let a = t.angle_deg(&at(10.0, 10.0));
        assert!((a - 45.0).abs() < 1e-3, "{a}");
        let n = t.normal(&at(10.0, 10.0));
        assert!(n.y.abs() > 0.5, "normal leans north: {n:?}");
    }
}
