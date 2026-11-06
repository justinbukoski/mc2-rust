// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Minimal linear algebra for the render front end.
//!
//! Matrices are **column-major** (`m[col * 4 + row]`) with column-vector semantics
//! (`p' = M * p`), the layout WGSL `mat4x4<f32>` expects.  The 2001 code used row-vector
//! `Stuff::Matrix4D`; ports below transpose as they go and say so.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn from_array(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }
    /// Unit vector, or `self` unchanged when the length is (nearly) zero.
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 1e-12 {
            self * (1.0 / l)
        } else {
            self
        }
    }
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        self + (o - self) * t
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
/// Hamilton product `a * b` (apply `b` first, then `a`).
impl Mul for Quat {
    type Output = Quat;
    fn mul(self, o: Quat) -> Quat {
        Quat {
            w: self.w * o.w - self.x * o.x - self.y * o.y - self.z * o.z,
            x: self.w * o.x + self.x * o.w + self.y * o.z - self.z * o.y,
            y: self.w * o.y - self.x * o.z + self.y * o.w + self.z * o.x,
            z: self.w * o.z + self.x * o.y - self.y * o.x + self.z * o.w,
        }
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// Unit quaternion `(x, y, z, w)`; `rotate` is the standard `q v q*`.
///
/// `Stuff::UnitQuaternion::BuildRotation` (affinematrix.cpp) builds the transpose of the
/// textbook matrix for a row-vector convention, so `v * M` equals the textbook `R(q) * v`.
/// The quaternions stored in AGL files therefore rotate exactly like this type does.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };
    pub fn from_array(a: [f32; 4]) -> Self {
        Self {
            x: a[0],
            y: a[1],
            z: a[2],
            w: a[3],
        }
    }
    pub fn from_axis_angle(axis: Vec3, radians: f32) -> Self {
        let a = axis.normalized();
        let (s, c) = (radians * 0.5).sin_cos();
        Self {
            x: a.x * s,
            y: a.y * s,
            z: a.z * s,
            w: c,
        }
    }
    pub fn normalized(self) -> Self {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l > 1e-12 {
            Self {
                x: self.x / l,
                y: self.y / l,
                z: self.z / l,
                w: self.w / l,
            }
        } else {
            Quat::IDENTITY
        }
    }
    pub fn dot(self, o: Quat) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }
    /// Shortest-arc spherical interpolation.
    pub fn slerp(self, mut o: Quat, t: f32) -> Quat {
        let mut d = self.dot(o);
        if d < 0.0 {
            d = -d;
            o = Quat {
                x: -o.x,
                y: -o.y,
                z: -o.z,
                w: -o.w,
            };
        }
        if d > 0.9995 {
            return Quat {
                x: self.x + (o.x - self.x) * t,
                y: self.y + (o.y - self.y) * t,
                z: self.z + (o.z - self.z) * t,
                w: self.w + (o.w - self.w) * t,
            }
            .normalized();
        }
        let theta = d.clamp(-1.0, 1.0).acos();
        let s = theta.sin();
        let a = ((1.0 - t) * theta).sin() / s;
        let b = (t * theta).sin() / s;
        Quat {
            x: self.x * a + o.x * b,
            y: self.y * a + o.y * b,
            z: self.z * a + o.z * b,
            w: self.w * a + o.w * b,
        }
    }
    pub fn rotate(self, v: Vec3) -> Vec3 {
        Mat4::from_quat(self).transform_vector(v)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]);

    /// Element at `(row, col)`.
    pub fn at(&self, row: usize, col: usize) -> f32 {
        self.0[col * 4 + row]
    }
    fn set(&mut self, row: usize, col: usize, v: f32) {
        self.0[col * 4 + row] = v;
    }
    pub fn from_rows(rows: [[f32; 4]; 4]) -> Mat4 {
        let mut m = Mat4([0.0; 16]);
        for (r, row) in rows.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                m.set(r, c, *v);
            }
        }
        m
    }
    pub fn translation(t: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.set(0, 3, t.x);
        m.set(1, 3, t.y);
        m.set(2, 3, t.z);
        m
    }
    pub fn scale(s: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        m.set(0, 0, s.x);
        m.set(1, 1, s.y);
        m.set(2, 2, s.z);
        m
    }
    /// Rotation by `radians` about +Z (counter-clockwise seen from above in a z-up world).
    pub fn rotation_z(radians: f32) -> Mat4 {
        let (s, c) = radians.sin_cos();
        Mat4::from_rows([
            [c, -s, 0.0, 0.0],
            [s, c, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
    /// Rotation by `radians` about +Y.
    pub fn rotation_y(radians: f32) -> Mat4 {
        let (s, c) = radians.sin_cos();
        Mat4::from_rows([
            [c, 0.0, s, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [-s, 0.0, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
    /// Rotation by `radians` about +X.
    pub fn rotation_x(radians: f32) -> Mat4 {
        let (s, c) = radians.sin_cos();
        Mat4::from_rows([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, c, -s, 0.0],
            [0.0, s, c, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
    /// Textbook column-vector rotation matrix of a unit quaternion.
    pub fn from_quat(q: Quat) -> Mat4 {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        Mat4::from_rows([
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - w * z),
                2.0 * (x * z + w * y),
                0.0,
            ],
            [
                2.0 * (x * y + w * z),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - w * x),
                0.0,
            ],
            [
                2.0 * (x * z - w * y),
                2.0 * (y * z + w * x),
                1.0 - 2.0 * (x * x + y * y),
                0.0,
            ],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }
    pub fn mul(&self, o: &Mat4) -> Mat4 {
        let mut out = Mat4([0.0; 16]);
        for c in 0..4 {
            for r in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += self.at(r, k) * o.at(k, c);
                }
                out.set(r, c, s);
            }
        }
        out
    }
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        let x = self.at(0, 0) * p.x + self.at(0, 1) * p.y + self.at(0, 2) * p.z + self.at(0, 3);
        let y = self.at(1, 0) * p.x + self.at(1, 1) * p.y + self.at(1, 2) * p.z + self.at(1, 3);
        let z = self.at(2, 0) * p.x + self.at(2, 1) * p.y + self.at(2, 2) * p.z + self.at(2, 3);
        Vec3::new(x, y, z)
    }
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.at(0, 0) * v.x + self.at(0, 1) * v.y + self.at(0, 2) * v.z,
            self.at(1, 0) * v.x + self.at(1, 1) * v.y + self.at(1, 2) * v.z,
            self.at(2, 0) * v.x + self.at(2, 1) * v.y + self.at(2, 2) * v.z,
        )
    }
    /// Full 4D transform returning `(x, y, z, w)`.
    pub fn transform4(&self, p: [f32; 4]) -> [f32; 4] {
        let mut o = [0.0; 4];
        for (r, out) in o.iter_mut().enumerate() {
            *out = (0..4).map(|c| self.at(r, c) * p[c]).sum();
        }
        o
    }
    pub fn transpose(&self) -> Mat4 {
        let mut o = Mat4([0.0; 16]);
        for r in 0..4 {
            for c in 0..4 {
                o.set(c, r, self.at(r, c));
            }
        }
        o
    }
    /// General inverse (cofactor expansion).  Returns `None` for singular matrices.
    pub fn inverse(&self) -> Option<Mat4> {
        let m = &self.0;
        let mut inv = [0.0f32; 16];
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
            + m[9] * m[7] * m[14]
            + m[13] * m[6] * m[11]
            - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
            - m[8] * m[7] * m[14]
            - m[12] * m[6] * m[11]
            + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
            + m[8] * m[7] * m[13]
            + m[12] * m[5] * m[11]
            - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
            - m[8] * m[6] * m[13]
            - m[12] * m[5] * m[10]
            + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
            - m[9] * m[3] * m[14]
            - m[13] * m[2] * m[11]
            + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
            + m[8] * m[3] * m[14]
            + m[12] * m[2] * m[11]
            - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
            - m[8] * m[3] * m[13]
            - m[12] * m[1] * m[11]
            + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
            + m[8] * m[2] * m[13]
            + m[12] * m[1] * m[10]
            - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
            + m[5] * m[3] * m[14]
            + m[13] * m[2] * m[7]
            - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
            - m[4] * m[3] * m[14]
            - m[12] * m[2] * m[7]
            + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
            + m[4] * m[3] * m[13]
            + m[12] * m[1] * m[7]
            - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
            - m[4] * m[2] * m[13]
            - m[12] * m[1] * m[6]
            + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
            - m[5] * m[3] * m[10]
            - m[9] * m[2] * m[7]
            + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
            + m[4] * m[3] * m[10]
            + m[8] * m[2] * m[7]
            - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
            - m[4] * m[3] * m[9]
            - m[8] * m[1] * m[7]
            + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
            + m[4] * m[2] * m[9]
            + m[8] * m[1] * m[6]
            - m[8] * m[2] * m[5];
        let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        if det.abs() < 1e-20 {
            return None;
        }
        let id = 1.0 / det;
        for v in &mut inv {
            *v *= id;
        }
        Some(Mat4(inv))
    }
}

impl Default for Mat4 {
    fn default() -> Self {
        Mat4::IDENTITY
    }
}

pub fn deg_to_rad(d: f32) -> f32 {
    d * std::f32::consts::PI / 180.0
}

/// The fixed change of basis between the game's world (x east, y north, z up) and the
/// frame the shape/camera code works in (`Stuff`: x = -world.x, y = world.z, z = world.y).
///
/// The game spells this out at every call site (`xlatPosition.x = -position.x; .y = position.z;
/// .z = position.y` in `bdactor.cpp`/`mech3d.cpp`, `coords.x = -point.x ...` in `Camera::projectZ`).
/// Both frames are right-handed (determinant +1), so triangle winding is preserved.
pub fn stuff_to_world() -> Mat4 {
    Mat4::from_rows([
        [-1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

/// World -> Stuff frame (the change of basis is its own inverse up to the axis swap).
pub fn world_to_stuff() -> Mat4 {
    // x_s = -x_w, y_s = z_w, z_s = y_w
    Mat4::from_rows([
        [-1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn inverse_round_trips() {
        let m = Mat4::translation(Vec3::new(1.0, 2.0, 3.0))
            .mul(&Mat4::rotation_y(0.7))
            .mul(&Mat4::scale(Vec3::new(2.0, 1.0, 0.5)));
        let id = m.mul(&m.inverse().unwrap());
        for (i, v) in id.0.iter().enumerate() {
            let want = if i % 5 == 0 { 1.0 } else { 0.0 };
            assert!(close(*v, want), "{i}: {v}");
        }
    }

    #[test]
    fn quat_matches_axis_rotation() {
        let q = Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), 0.9);
        let a = q.rotate(Vec3::new(1.0, 0.0, 0.0));
        let b = Mat4::rotation_y(0.9).transform_vector(Vec3::new(1.0, 0.0, 0.0));
        assert!(close(a.x, b.x) && close(a.y, b.y) && close(a.z, b.z));
    }

    #[test]
    fn slerp_endpoints_and_midpoint() {
        let a = Quat::IDENTITY;
        let b = Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), 1.0);
        let m = a.slerp(b, 0.5);
        let want = Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), 0.5);
        assert!(close(m.z, want.z) && close(m.w, want.w));
        assert!(close(a.slerp(b, 0.0).w, 1.0));
        assert!(close(a.slerp(b, 1.0).z, b.z));
    }

    #[test]
    fn stuff_basis_is_proper_rotation() {
        let m = stuff_to_world();
        // east (1,0,0) in the world is -x in the Stuff frame
        let s = world_to_stuff().transform_vector(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(s, Vec3::new(-1.0, 0.0, 0.0));
        // round trip
        let p = Vec3::new(3.0, -4.0, 5.0);
        let q = m.transform_point(world_to_stuff().transform_point(p));
        assert!(close(q.x, p.x) && close(q.y, p.y) && close(q.z, p.z));
        // determinant +1: x cross y = z in both frames
        let x = m.transform_vector(Vec3::new(1.0, 0.0, 0.0));
        let y = m.transform_vector(Vec3::new(0.0, 1.0, 0.0));
        let z = m.transform_vector(Vec3::new(0.0, 0.0, 1.0));
        let c = x.cross(y);
        assert!(close(c.x, z.x) && close(c.y, z.y) && close(c.z, z.z));
    }
}
