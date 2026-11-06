// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The slice of `Stuff` (the 2001 math library) that gosFX uses.
//!
//! Conventions copied from `mclib/stuff`: row vectors (`v' = v * M`), matrices stored as four
//! rows (X, Y, Z axes, then the translation) of three columns, so `m(row, column)` is
//! `entries[(column << 2) + row]` in the file. Right-handed cross product, `Vector3D::Up` is
//! +Y and `Forward` is +Z. Everything is `f32`.

use std::ops::{Add, AddAssign, Mul, Neg, Sub};

/// `Stuff::SMALL` (`mclib/stuff/style.cpp`).
pub const SMALL: f32 = 1e-4;
pub const PI: f32 = std::f32::consts::PI;
pub const TWO_PI: f32 = std::f32::consts::TAU;
pub const PI_OVER_2: f32 = std::f32::consts::FRAC_PI_2;

pub const X_AXIS: usize = 0;
pub const Y_AXIS: usize = 1;
pub const Z_AXIS: usize = 2;

pub fn small_enough(x: f32) -> bool {
    x.abs() <= SMALL
}

/// `Stuff::Lerp(a, b, t)`.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// `Stuff::Truncate_Float_To_Byte`: the magic-number rounding trick, bit for bit.
pub fn truncate_float_to_byte(v: f32) -> u8 {
    let mut x = v;
    x -= 0.5;
    x += 12582912.0;
    x.to_bits() as u8
}

/// `Radian::Normalize(Scalar)`: wrap into `(-pi, pi]`.
pub fn radian_normalize(v: f32) -> f32 {
    let mut t = (v as f64 % TWO_PI as f64) as f32;
    if t > PI {
        t -= TWO_PI;
    } else if t <= -PI {
        t += TWO_PI;
    }
    t
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
    pub const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    /// `Vector3D::Cross` (right-handed).
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }
    /// `Vector3D::Normalize`. The C++ divides by the length unchecked (a `Verify` in debug
    /// builds); a zero vector stays zero here instead of becoming NaN.
    pub fn normalized(self) -> Vec3 {
        let l = self.length();
        if l > 1e-20 {
            self * (1.0 / l)
        } else {
            Vec3::ZERO
        }
    }
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        Vec3::new(
            self.x + t * (o.x - self.x),
            self.y + t * (o.y - self.y),
            self.z + t * (o.z - self.z),
        )
    }
    /// `AddScaled(a, b, t)`: `a + b * t`.
    pub fn add_scaled(self, b: Vec3, t: f32) -> Vec3 {
        Vec3::new(self.x + b.x * t, self.y + b.y * t, self.z + b.z * t)
    }
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
    /// `Vector3D(YawPitchRange)` (`vector3d.cpp`): y is `-range * sin(pitch)`.
    pub fn from_yaw_pitch_range(yaw: f32, pitch: f32, range: f32) -> Vec3 {
        let y = -range * pitch.sin();
        let len = range * pitch.cos();
        Vec3::new(len * yaw.sin(), y, len * yaw.cos())
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl AddAssign for Vec3 {
    fn add_assign(&mut self, o: Vec3) {
        *self = *self + o;
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
impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// `Stuff::RGBAColor`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const WHITE: Rgba = Rgba::new(1.0, 1.0, 1.0, 1.0);
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

/// `Stuff::LinearMatrix4D`: a rigid-body (rotation plus translation) 4x3 matrix.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Mat {
    /// `m[row][column]`: rows 0..3 are the X, Y, Z axes, row 3 the translation.
    pub m: [[f32; 3]; 4],
}

impl Mat {
    pub const IDENTITY: Mat = Mat {
        m: [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ],
    };

    /// From the file/memory layout: `entries[(column << 2) + row]`.
    pub fn from_entries(e: [f32; 12]) -> Mat {
        let mut m = [[0.0; 3]; 4];
        for (i, v) in e.iter().enumerate() {
            m[i & 3][i >> 2] = *v;
        }
        Mat { m }
    }

    pub fn get(&self, row: usize, col: usize) -> f32 {
        self.m[row][col]
    }

    pub fn translation(&self) -> Vec3 {
        Vec3::new(self.m[3][0], self.m[3][1], self.m[3][2])
    }

    /// Row `axis` as a vector (the local axis in parent space).
    pub fn axis(&self, axis: usize) -> Vec3 {
        Vec3::new(self.m[axis][0], self.m[axis][1], self.m[axis][2])
    }

    fn set_axis(&mut self, axis: usize, v: Vec3) {
        self.m[axis] = [v.x, v.y, v.z];
    }

    /// `AffineMatrix4D::Multiply(a, b)`: `a` first, then `b`.
    pub fn multiply(a: &Mat, b: &Mat) -> Mat {
        let mut out = [[0.0f32; 3]; 4];
        for (r, row) in out.iter_mut().enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                let mut v = a.m[r][0] * b.m[0][c] + a.m[r][1] * b.m[1][c] + a.m[r][2] * b.m[2][c];
                if r == 3 {
                    v += b.m[3][c];
                }
                *cell = v;
            }
        }
        Mat { m: out }
    }

    /// `LinearMatrix4D::Invert` (transpose of the rotation, negated offsets).
    pub fn invert(&self) -> Mat {
        let m = &self.m;
        let mut o = [[0.0f32; 3]; 4];
        for (r, row) in o.iter_mut().take(3).enumerate() {
            for (c, cell) in row.iter_mut().enumerate() {
                *cell = m[c][r];
            }
        }
        for (c, cell) in o[3].iter_mut().enumerate() {
            *cell = -m[3][0] * m[c][0] - m[3][1] * m[c][1] - m[3][2] * m[c][2];
        }
        Mat { m: o }
    }

    /// `Point3D::Multiply(p, m)`.
    pub fn point(&self, p: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            p.x * m[0][0] + p.y * m[1][0] + p.z * m[2][0] + m[3][0],
            p.x * m[0][1] + p.y * m[1][1] + p.z * m[2][1] + m[3][1],
            p.x * m[0][2] + p.y * m[1][2] + p.z * m[2][2] + m[3][2],
        )
    }

    /// `Vector3D::Multiply(v, m)`: rotation only.
    pub fn vector(&self, v: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            v.x * m[0][0] + v.y * m[1][0] + v.z * m[2][0],
            v.x * m[0][1] + v.y * m[1][1] + v.z * m[2][1],
            v.x * m[0][2] + v.y * m[1][2] + v.z * m[2][2],
        )
    }

    /// `Vector3D::MultiplyByInverse(v, m)`.
    pub fn vector_by_inverse(&self, v: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            v.x * m[0][0] + v.y * m[0][1] + v.z * m[0][2],
            v.x * m[1][0] + v.y * m[1][1] + v.z * m[1][2],
            v.x * m[2][0] + v.y * m[2][1] + v.z * m[2][2],
        )
    }

    /// `Point3D::MultiplyByInverse(p, m)`.
    pub fn point_by_inverse(&self, p: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            p.x * m[0][0] + p.y * m[0][1] + p.z * m[0][2]
                - m[3][0] * m[0][0]
                - m[3][1] * m[0][1]
                - m[3][2] * m[0][2],
            p.x * m[1][0] + p.y * m[1][1] + p.z * m[1][2]
                - m[3][0] * m[1][0]
                - m[3][1] * m[1][1]
                - m[3][2] * m[1][2],
            p.x * m[2][0] + p.y * m[2][1] + p.z * m[2][2]
                - m[3][0] * m[2][0]
                - m[3][1] * m[2][1]
                - m[3][2] * m[2][2],
        )
    }

    /// `BuildTranslation`: sets row 3 only.
    pub fn build_translation(&mut self, p: Vec3) {
        self.m[3] = [p.x, p.y, p.z];
    }

    /// `BuildRotation(UnitQuaternion)`: sets the 3x3 part only.
    pub fn build_rotation(&mut self, q: &Quat) {
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
        self.m[0][0] = g + h - i - j;
        self.m[1][0] = 2.0 * (a - f);
        self.m[2][0] = 2.0 * (c + e);
        self.m[0][1] = 2.0 * (f + a);
        self.m[1][1] = g - h + i - j;
        self.m[2][1] = 2.0 * (b - d);
        self.m[0][2] = 2.0 * (c - e);
        self.m[1][2] = 2.0 * (b + d);
        self.m[2][2] = g - h - i + j;
    }

    /// `LinearMatrix4D(UnitQuaternion)`: the rotation on an identity with no translation.
    pub fn from_quat(q: &Quat) -> Mat {
        let mut m = Mat::IDENTITY;
        m.build_rotation(q);
        m
    }

    /// `BuildRotation(EulerAngles(pitch, yaw, roll))`: sets the 3x3 part only.
    pub fn build_rotation_euler(&mut self, pitch: f32, yaw: f32, roll: f32) {
        let (xs, xc) = pitch.sin_cos();
        let (ys, yc) = yaw.sin_cos();
        let (zs, zc) = roll.sin_cos();
        self.m[0][0] = yc * zc;
        self.m[0][1] = yc * zs;
        self.m[0][2] = -ys;
        self.m[1][0] = xs * ys * zc - xc * zs;
        self.m[1][1] = xs * ys * zs + xc * zc;
        self.m[1][2] = xs * yc;
        self.m[2][0] = xc * ys * zc + xs * zs;
        self.m[2][1] = xc * ys * zs - xs * zc;
        self.m[2][2] = xc * yc;
    }

    /// `LinearMatrix4D(EulerAngles)`-style rotation built on an identity.
    pub fn from_euler(pitch: f32, yaw: f32, roll: f32) -> Mat {
        let mut m = Mat::IDENTITY;
        m.build_rotation_euler(pitch, yaw, roll);
        m
    }

    pub fn from_translation(p: Vec3) -> Mat {
        let mut m = Mat::IDENTITY;
        m.build_translation(p);
        m
    }

    /// `LinearMatrix4D::AlignLocalAxisToWorldVector` (`linearmatrix.cpp`).
    ///
    /// Turns the matrix so local axis `pointing` points along `world_target`, rotating about
    /// local axis `rotating`. With `minor == Some(axis)` the matrix is rotated about the minor
    /// axis to hit the target exactly; with `None` (the C++ `-1`) the rotating axis is frozen.
    pub fn align_local_axis_to_world_vector(
        &mut self,
        world_target: Vec3,
        pointing: usize,
        rotating: usize,
        minor: Option<usize>,
    ) {
        if small_enough(world_target.length_squared()) {
            return;
        }
        let mut rotation_vector = self.axis(rotating);
        let temp = rotation_vector.cross(world_target);
        let pointing_vector;
        let mut minor_vector;
        let minor_axis;
        match minor {
            None => {
                minor_axis = 3 - pointing - rotating;
                minor_vector = temp.normalized();
                if (rotating + 1) % 3 == pointing {
                    pointing_vector = minor_vector.cross(rotation_vector);
                } else {
                    minor_vector = -minor_vector;
                    pointing_vector = rotation_vector.cross(minor_vector);
                }
            }
            Some(ma) => {
                minor_axis = ma;
                if small_enough(temp.length_squared()) {
                    if world_target.dot(rotation_vector) > 0.0 {
                        pointing_vector = self.axis(rotating);
                        rotation_vector = -self.axis(pointing);
                    } else {
                        pointing_vector = -self.axis(rotating);
                        rotation_vector = self.axis(pointing);
                    }
                    minor_vector = self.axis(minor_axis);
                } else {
                    let pv = world_target.normalized();
                    minor_vector = temp.normalized();
                    if (rotating + 1) % 3 == pointing {
                        rotation_vector = pv.cross(minor_vector);
                    } else {
                        minor_vector = -minor_vector;
                        rotation_vector = minor_vector.cross(pv);
                    }
                    pointing_vector = pv;
                }
            }
        }
        self.set_axis(pointing, pointing_vector);
        self.set_axis(rotating, rotation_vector);
        self.set_axis(minor_axis, minor_vector);
    }
}

impl Default for Mat {
    fn default() -> Self {
        Mat::IDENTITY
    }
}

/// `Stuff::UnitQuaternion`.
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

    /// `UnitQuaternion::Normalize`: rebuilds `w` from the vector part.
    pub fn normalize(&mut self) {
        let t = self.x * self.x + self.y * self.y + self.z * self.z;
        if t <= 1.0 {
            let w = (1.0 - t).sqrt();
            if self.w < 0.0 {
                self.x = -self.x;
                self.y = -self.y;
                self.z = -self.z;
            }
            self.w = w;
        } else {
            let s = 1.0 / t.sqrt();
            self.x *= s;
            self.y *= s;
            self.z *= s;
            self.w = 0.0;
        }
    }

    /// `UnitQuaternion::operator=(LinearMatrix4D)`.
    pub fn from_matrix(m: &Mat) -> Quat {
        let mut q = Quat::IDENTITY;
        q.w = (1.0 + m.get(0, 0) + m.get(1, 1) + m.get(2, 2)) * 0.25;
        if q.w.abs() <= 1e-2 {
            if q.w < 0.0 {
                q.w = 0.0;
            }
            let mut temp = ((1.0 + m.get(0, 0)) * 0.5 - q.w).max(0.0);
            q.x = temp.sqrt();
            temp = ((1.0 + m.get(1, 1)) * 0.5 - q.w).max(0.0);
            q.y = temp.sqrt();
            temp = ((1.0 + m.get(2, 2)) * 0.5 - q.w).max(0.0);
            q.z = temp.sqrt();
            q.w = q.w.sqrt();
            if m.get(0, 1) < m.get(1, 0) {
                q.z = -q.z;
            }
            if m.get(2, 0) < m.get(0, 2) {
                q.y = -q.y;
            }
            if m.get(1, 2) < m.get(2, 1) {
                q.x = -q.x;
            }
        } else {
            q.w = q.w.sqrt();
            q.x = (m.get(1, 2) - m.get(2, 1)) * 0.25 / q.w;
            q.y = (m.get(2, 0) - m.get(0, 2)) * 0.25 / q.w;
            q.z = (m.get(0, 1) - m.get(1, 0)) * 0.25 / q.w;
        }
        q.normalize();
        q
    }

    /// `UnitQuaternion::operator=(EulerAngles)`: through the matrix, as the C++ does.
    pub fn from_euler(pitch: f32, yaw: f32, roll: f32) -> Quat {
        Quat::from_matrix(&Mat::from_euler(pitch, yaw, roll))
    }

    /// `UnitQuaternion::operator=(Vector3D)`: rotation vector (axis times angle).
    pub fn from_rotation_vector(v: Vec3) -> Quat {
        let rotation = v.length();
        if small_enough(rotation) {
            return Quat::IDENTITY;
        }
        let half = 0.5 * radian_normalize(rotation);
        let s = half.sin() / rotation;
        Quat {
            x: v.x * s,
            y: v.y * s,
            z: v.z * s,
            w: half.cos(),
        }
    }

    /// `UnitQuaternion::Multiply(q2, q1)`.
    pub fn multiply(q2: &Quat, q1: &Quat) -> Quat {
        Quat {
            x: q1.w * q2.x + q2.w * q1.x + q1.y * q2.z - q1.z * q2.y,
            y: q1.w * q2.y + q2.w * q1.y + q1.z * q2.x - q1.x * q2.z,
            z: q1.w * q2.z + q2.w * q1.z + q1.x * q2.y - q1.y * q2.x,
            w: q1.w * q2.w - q1.x * q2.x - q1.y * q2.y - q1.z * q2.z,
        }
    }
}

/// `Stuff::OBB`, as far as gosFX uses it: the bounds an effect reports to its caller.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Obb {
    pub local_to_parent: Mat,
    pub axis_extents: Vec3,
    pub sphere_radius: f32,
}

impl Obb {
    pub const IDENTITY: Obb = Obb {
        local_to_parent: Mat::IDENTITY,
        axis_extents: Vec3::ZERO,
        sphere_radius: 0.0,
    };

    /// `OBB::Multiply(obb, matrix)`.
    pub fn multiply(obb: &Obb, matrix: &Mat) -> Obb {
        Obb {
            local_to_parent: Mat::multiply(&obb.local_to_parent, matrix),
            axis_extents: obb.axis_extents,
            sphere_radius: obb.sphere_radius,
        }
    }

    /// `OBB::Union(first, second)`: the bounding sphere of both.
    pub fn union(first: &Obb, second: &Obb) -> Obb {
        let c1 = first.local_to_parent.translation();
        let c2 = second.local_to_parent.translation();
        let len = (c2 - c1).length();
        if len + first.sphere_radius <= second.sphere_radius || first.sphere_radius == 0.0 {
            return *second;
        }
        if len + second.sphere_radius <= first.sphere_radius || second.sphere_radius == 0.0 {
            return *first;
        }
        let c = c1.lerp(
            c2,
            (len + second.sphere_radius - first.sphere_radius) / (2.0 * len),
        );
        Obb {
            local_to_parent: Mat::from_translation(c),
            axis_extents: Vec3::ZERO,
            sphere_radius: 0.5 * (len + first.sphere_radius + second.sphere_radius),
        }
    }

    /// The box `ExtentBox` -> OBB step shared by every cloud's `Execute`.
    pub fn from_extent_box(min: Vec3, max: Vec3) -> Obb {
        let ext = Vec3::new(
            0.5 * (max.x - min.x),
            0.5 * (max.y - min.y),
            0.5 * (max.z - min.z),
        );
        let mut m = Mat::IDENTITY;
        m.build_translation(Vec3::new(min.x + ext.x, min.y + ext.y, min.z + ext.z));
        let mut radius = ext.length();
        if radius < SMALL {
            radius = 0.01;
        }
        Obb {
            local_to_parent: m,
            axis_extents: ext,
            sphere_radius: radius,
        }
    }
}
