// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The part of the Stuff math library (`mclib/stuff`) that TGL node transforms use, ported operation for operation.
//!
//! Every function keeps the C++ evaluation order and `f32` arithmetic (`-ffp-contract=off`, no fused multiply-add), so
//! the results are bit-identical to the C++ build of the same platform. `sin`/`cos` are the platform `sinf`/`cosf`
//! (`SinCosPair::operator=(const Radian&)`, angle.hpp:282-303, `USE_ASSEMBLER_CODE == 0`); `Sqrt` is `sqrt` of the
//! float argument, which rounds the same as `f32::sqrt` (scalar.hpp:156).
//!
//! Matrices are `AffineMatrix4D`/`LinearMatrix4D`: 12 entries, `m(row, column) = entries[(column << 2) + row]`
//! (affinematrix.hpp:136), rows 0-2 the rotation, row 3 (`W_Axis`) the translation.

/// `Stuff::UnitQuaternion` (x, y, z, w).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    /// `UnitQuaternion::Identity` (0, 0, 0, 1).
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// `baseRotation.w == 1 && x == 0 && y == 0 && z == 0` (msl.cpp:1384).
    pub fn is_identity(&self) -> bool {
        self.w == 1.0 && self.x == 0.0 && self.y == 0.0 && self.z == 0.0
    }
}

/// `Stuff::LinearMatrix4D` entries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat(pub [f32; 12]);

impl Default for Mat {
    fn default() -> Self {
        Mat::IDENTITY
    }
}

impl Mat {
    /// `LinearMatrix4D::Identity`.
    pub const IDENTITY: Mat = Mat([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);

    /// `(*this)(row, column)`.
    #[inline]
    pub fn at(&self, row: usize, column: usize) -> f32 {
        self.0[(column << 2) + row]
    }

    #[inline]
    fn set(&mut self, row: usize, column: usize, v: f32) {
        self.0[(column << 2) + row] = v;
    }

    /// `AffineMatrix4D::BuildRotation(const EulerAngles&)` (affinematrix.cpp:162): sets the 3x3 part only.
    pub fn build_rotation_euler(&mut self, pitch: f32, yaw: f32, roll: f32) {
        let (xs, xc) = sin_cos(pitch);
        let (ys, yc) = sin_cos(yaw);
        let (zs, zc) = sin_cos(roll);
        self.set(0, 0, yc * zc);
        self.set(0, 1, yc * zs);
        self.set(0, 2, -ys);
        self.set(1, 0, xs * ys * zc - xc * zs);
        self.set(1, 1, xs * ys * zs + xc * zc);
        self.set(1, 2, xs * yc);
        self.set(2, 0, xc * ys * zc + xs * zs);
        self.set(2, 1, xc * ys * zs - xs * zc);
        self.set(2, 2, xc * yc);
    }

    /// `AffineMatrix4D::BuildRotation(const UnitQuaternion&)` (affinematrix.cpp:242): sets the 3x3 part only.
    pub fn build_rotation_quat(&mut self, q: &Quat) {
        let a = q.x * q.y;
        let b = q.y * q.z;
        let c = q.z * q.x;
        let d = q.w * q.x;
        let e = q.w * q.y;
        let f = q.w * q.z;
        let g = q.w * q.w;
        let h = q.x * q.x;
        let i = q.y * q.y;
        let j = q.z * q.z;
        self.set(0, 0, g + h - i - j);
        self.set(1, 0, 2.0 * (a - f));
        self.set(2, 0, 2.0 * (c + e));
        self.set(0, 1, 2.0 * (f + a));
        self.set(1, 1, g - h + i - j);
        self.set(2, 1, 2.0 * (b - d));
        self.set(0, 2, 2.0 * (c - e));
        self.set(1, 2, 2.0 * (b + d));
        self.set(2, 2, g - h - i + j);
    }

    /// `AffineMatrix4D::BuildTranslation(const Point3D&)` (affinematrix.hpp:102): sets row 3 only.
    pub fn build_translation(&mut self, p: [f32; 3]) {
        self.set(3, 0, p[0]);
        self.set(3, 1, p[1]);
        self.set(3, 2, p[2]);
    }

    /// `AffineMatrix4D::Multiply(Source1, Source2)` (affinematrix.cpp:354, the C branch): `this = s1 * s2`.
    pub fn multiply(s1: &Mat, s2: &Mat) -> Mat {
        let mut r = Mat([0.0; 12]);
        for c in 0..3 {
            for row in 0..3 {
                r.set(
                    row,
                    c,
                    s1.at(row, 0) * s2.at(0, c)
                        + s1.at(row, 1) * s2.at(1, c)
                        + s1.at(row, 2) * s2.at(2, c),
                );
            }
            r.set(
                3,
                c,
                s1.at(3, 0) * s2.at(0, c)
                    + s1.at(3, 1) * s2.at(1, c)
                    + s1.at(3, 2) * s2.at(2, c)
                    + s2.at(3, c),
            );
        }
        r
    }
}

/// `SinCosPair = Radian` (angle.hpp:300): `cosine = cosf(r); sine = sinf(r)`.
///
/// Uses the portable `mc2_libm` functions (see `mc2-libm`; the C++ oracle keeps the platform libm and is compared with tolerances): neither the platform's separate
/// `sinf`/`cosf` nor clang's merged `__sincosf_stret` is portable, and the two disagree in the last bit for ~2.6% of
/// arguments on macOS.
pub fn sin_cos(r: f32) -> (f32, f32) {
    mc2_libm::sin_cosf(r)
}

/// `Rotate(Vector3D&, angle)` (mclib/mathfunc.cpp:34) on an `[x, y, z]`: turns x/y by `angle` degrees
/// (`DEGREES_TO_RADS` = 0.0174532925199f; float `sin`/`cos` merged like [`sin_cos`]).
pub fn rotate_xy(v: &mut [f32; 3], angle: f32) {
    #[allow(clippy::excessive_precision)]
    const DEGREES_TO_RADS: f32 = 0.017_453_292_519_9;
    let tx = v[0];
    let (sine, cosine) = sin_cos(angle * DEGREES_TO_RADS);
    v[0] *= cosine;
    v[0] += v[1] * sine;
    v[1] *= cosine;
    v[1] -= tx * sine;
}

/// `UnitQuaternion::operator=(const LinearMatrix4D&)` (rotation.cpp:588), including the final `Normalize()`.
pub fn quat_from_matrix(m: &Mat) -> Quat {
    let mut w = (1.0f32 + m.at(0, 0) + m.at(1, 1) + m.at(2, 2)) * 0.25;
    let (mut x, mut y, mut z);
    if w.abs() <= 1e-2 {
        if w < 0.0 {
            w = 0.0;
        }
        let mut temp = (1.0f32 + m.at(0, 0)) * 0.5 - w;
        if temp < 0.0 {
            temp = 0.0;
        }
        x = temp.sqrt();
        temp = (1.0f32 + m.at(1, 1)) * 0.5 - w;
        if temp < 0.0 {
            temp = 0.0;
        }
        y = temp.sqrt();
        temp = (1.0f32 + m.at(2, 2)) * 0.5 - w;
        if temp < 0.0 {
            temp = 0.0;
        }
        z = temp.sqrt();
        w = w.sqrt();
        if m.at(0, 1) < m.at(1, 0) {
            z = -z;
        }
        if m.at(2, 0) < m.at(0, 2) {
            y = -y;
        }
        if m.at(1, 2) < m.at(2, 1) {
            x = -x;
        }
    } else {
        w = w.sqrt();
        x = (m.at(1, 2) - m.at(2, 1)) * 0.25 / w;
        y = (m.at(2, 0) - m.at(0, 2)) * 0.25 / w;
        z = (m.at(0, 1) - m.at(1, 0)) * 0.25 / w;
    }
    let mut q = Quat { x, y, z, w };
    quat_normalize(&mut q);
    q
}

/// `UnitQuaternion = EulerAngles(pitch, yaw, roll)` (rotation.cpp:560): through a rotation matrix.
pub fn quat_from_euler(pitch: f32, yaw: f32, roll: f32) -> Quat {
    let mut m = Mat::IDENTITY;
    m.build_rotation_euler(pitch, yaw, roll);
    quat_from_matrix(&m)
}

/// `UnitQuaternion::Multiply(q2, q1)` (rotation.cpp:773).
pub fn quat_multiply(q2: &Quat, q1: &Quat) -> Quat {
    Quat {
        x: q1.w * q2.x + q2.w * q1.x + q1.y * q2.z - q1.z * q2.y,
        y: q1.w * q2.y + q2.w * q1.y + q1.z * q2.x - q1.x * q2.z,
        z: q1.w * q2.z + q2.w * q1.z + q1.x * q2.y - q1.y * q2.x,
        w: q1.w * q2.w - q1.x * q2.x - q1.y * q2.y - q1.z * q2.z,
    }
}

/// `UnitQuaternion::Normalize()` (rotation.cpp:886): rebuilds `w` from x, y, z.
pub fn quat_normalize(q: &mut Quat) {
    let mut t = q.x * q.x + q.y * q.y + q.z * q.z;
    if t <= 1.0 {
        t = (1.0f32 - t).sqrt();
        if q.w < 0.0 {
            q.x = -q.x;
            q.y = -q.y;
            q.z = -q.z;
        }
        q.w = t;
    } else {
        t = t.sqrt();
        t = 1.0 / t;
        q.x *= t;
        q.y *= t;
        q.z *= t;
        q.w = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_quaternion_round_trip() {
        let q = quat_from_euler(0.0, 0.5, 0.0);
        let mut m = Mat::IDENTITY;
        m.build_rotation_quat(&q);
        let mut e = Mat::IDENTITY;
        e.build_rotation_euler(0.0, 0.5, 0.0);
        for i in 0..12 {
            assert!((m.0[i] - e.0[i]).abs() < 1e-6, "{i}: {} {}", m.0[i], e.0[i]);
        }
    }

    #[test]
    fn sin_cos_is_the_portable_pair() {
        for i in -2000..2000 {
            let x = i as f32 * 0.0137;
            assert_eq!(sin_cos(x), (mc2_libm::sinf(x), mc2_libm::cosf(x)));
        }
    }

    #[test]
    fn identity_and_multiply() {
        assert_eq!(quat_from_euler(0.0, 0.0, 0.0), Quat::IDENTITY);
        let mut a = Mat::IDENTITY;
        a.build_translation([1.0, 2.0, 3.0]);
        let mut b = Mat::IDENTITY;
        b.build_translation([10.0, 20.0, 30.0]);
        let c = Mat::multiply(&a, &b);
        assert_eq!([c.at(3, 0), c.at(3, 1), c.at(3, 2)], [11.0, 22.0, 33.0]);
    }

    #[test]
    fn normalize_rebuilds_w_and_keeps_hemisphere() {
        let mut q = Quat {
            x: 0.6,
            y: 0.0,
            z: 0.0,
            w: -0.8,
        };
        quat_normalize(&mut q);
        assert_eq!(q.x, -0.6);
        assert!(q.w > 0.0);
    }
}
