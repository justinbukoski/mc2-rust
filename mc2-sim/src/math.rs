// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Small math helpers shared by all lanes.
//!
//! Vectors are `mc2_path::Vec3` (`Stuff::Vector3D`, three `f32`). The C++ mixes `float` and `double` arithmetic
//! (`cos`, `acos`, `sqrt` are the C library double versions; results are stored back into `float`): port such
//! expressions with the same intermediate types. Do not enable fused multiply-add anywhere in this crate: the oracle is
//! built with `-ffp-contract=off` and Rust never fuses on its own.

pub use mc2_path::Vec3;

/// `DEG2RAD` as used by the sources (`Stuff::Pi / 180` in float).
pub const DEG2RAD: f32 = std::f32::consts::PI / 180.0;

/// Operations the C++ `Stuff::Vector3D` offers.
pub trait Vec3Ext {
    /// `(0,0,0)`.
    fn zero() -> Vec3;
    /// Construct.
    fn from_xyz(x: f32, y: f32, z: f32) -> Vec3;
    /// `GetLength()`: `sqrt(x*x + y*y + z*z)` (float math through double like `Stuff`).
    fn length(&self) -> f32;
    /// 2-D distance in the ground plane (x, y).
    fn distance_xy(&self, other: &Vec3) -> f32;
}

impl Vec3Ext for Vec3 {
    fn zero() -> Vec3 {
        Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        }
    }
    fn from_xyz(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }
    fn length(&self) -> f32 {
        ((self.x * self.x + self.y * self.y + self.z * self.z) as f64).sqrt() as f32
    }
    fn distance_xy(&self, other: &Vec3) -> f32 {
        let (dx, dy) = (self.x - other.x, self.y - other.y);
        ((dx * dx + dy * dy) as f64).sqrt() as f32
    }
}

// ------------------------------------------------------------------------------------------------------------------
// mclib/mathfunc.cpp helpers (added by lane A; used by turrets, trees and anything that turns a facing into an angle).
//
// NOTE on libm: the oracle compiles `sin(float)`, `cos(float)` and `acos(float)` to the *float* libm functions
// (`__sincosf_stret`, `acosf`; checked with `nm` on the oracle's mathfunc.o), not to the double versions rounded to float.
// A `sin`/`cos` pair on the same argument is one merged call (`mc2_assets::stuff::sin_cos` reproduces it); `acos` is plain
// `acosf`, which Rust's `f32::acos` calls.
// ------------------------------------------------------------------------------------------------------------------

/// `DEGREES_TO_RADS` (mclib/dstd.h:35).
pub const DEGREES_TO_RADS: f32 = 0.017_453_292;
/// `RADS_TO_DEGREES` (mclib/dstd.h:34).
pub const RADS_TO_DEGREES: f32 = 57.295_78;

/// `Rotate(Vector3D&, angle)` (mathfunc.cpp:30): rotates the x/y components by `angle` degrees.
pub fn rotate_xy(v: &mut Vec3, angle: f32) {
    let tx = v.x;
    // sin() and cos() of the same argument: clang merges the pair into one __sincosf_stret call, see
    // mc2_assets::stuff::sin_cos.
    let (sine, cosine) = mc2_assets::stuff::sin_cos(angle * DEGREES_TO_RADS);
    v.x *= cosine;
    v.x += v.y * sine;
    v.y *= cosine;
    v.y -= tx * sine;
}

/// `my_acos(val)` (mathfunc.cpp:84): clamps to [-1, 1] and returns `acosf`.
pub fn my_acos(val: f32) -> f32 {
    // clamp (NaN passes through, as with the C++ comparisons)
    mc2_libm::acosf(val.clamp(-1.0, 1.0))
}

/// `angle_from(Vector3D&, Vector3D&)` (mathfunc.cpp:122): unsigned angle in degrees between two vectors (0 when either is
/// zero length).
pub fn angle_from(v1: &Vec3, v2: &Vec3) -> f32 {
    let mag_product = v1.length() * v2.length();
    if mag_product == 0.0 {
        mag_product
    } else {
        let dot = v1.x * v2.x + v1.y * v2.y + v1.z * v2.z;
        my_acos(dot / mag_product) * RADS_TO_DEGREES
    }
}

/// `GameObject::getRotationVector()`: the unit vector `(0, -1, 0)` rotated by `-rotation` degrees.
pub fn rotation_vector(rotation: f32) -> Vec3 {
    let mut v = Vec3 {
        x: 0.0,
        y: -1.0,
        z: 0.0,
    };
    rotate_xy(&mut v, -rotation);
    v
}

/// `GameObject::relFacingTo(goal)` for an object at `position` facing `rotation` degrees (gameobj.cpp:1605): signed angle
/// to the goal, negative to the left of the facing.
pub fn rel_facing_to(position: &Vec3, rotation: f32, goal: &Vec3) -> f32 {
    let facing = rotation_vector(rotation);
    let goal_vec = Vec3 {
        x: goal.x - position.x,
        y: goal.y - position.y,
        z: goal.z - position.z,
    };
    let mut angle = angle_from(&facing, &goal_vec);
    let z = (facing.x * goal_vec.y) - (facing.y * goal_vec.x);
    if z > 0.0 {
        angle = -angle;
    }
    angle
}

/// `Vector3D::GetApproximateLength()`: `sqrt(x*x + y*y + z*z)` (the assembler variant is off in the oracle build).
pub fn approximate_length(v: &Vec3) -> f32 {
    ((v.x * v.x + v.y * v.y + v.z * v.z) as f64).sqrt() as f32
}

/// Portable `atan2(y, x)` in radians for the head-shot roll: only IEEE-exact `+ - * /` and `sqrt` on `f64`, so every platform gets the
/// same bits (a libm `atan2` may differ in the last ulp, which can move an integer percent at a truncation boundary).
/// Two half-angle reductions bring the argument below 0.2, then the Maclaurin series (error far below one `f32` ulp).
pub fn portable_atan2(y: f32, x: f32) -> f32 {
    let (y, x) = (f64::from(y), f64::from(x));
    const PI: f64 = std::f64::consts::PI;
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let (ay, ax) = (y.abs(), x.abs());
    // atan of the ratio in [0, 1], folding the larger operand into the denominator.
    let swap = ay > ax;
    let t = if swap { ax / ay } else { ay / ax };
    let mut u = t;
    for _ in 0..2 {
        u /= 1.0 + (1.0 + u * u).sqrt();
    }
    let u2 = u * u;
    let mut term = u;
    let mut sum = u;
    for k in 1..12 {
        term *= -u2;
        sum += term / f64::from(2 * k + 1);
    }
    let mut a = sum * 4.0;
    if swap {
        a = PI / 2.0 - a;
    }
    if x < 0.0 {
        a = PI - a;
    }
    if y < 0.0 {
        a = -a;
    }
    a as f32
}
