// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Distance attenuation and stereo panning from the camera.
//!
//! Port of the positional part of `SoundSystem::playDigitalSample` (`mclib/soundsys.cpp`) and `OppRotate`
//! (`mclib/mathfunc.cpp`). All maths is `f32`, as in the C++.
//!
//! - `dist = |eye.position - pos|` (3D, `GetApproximateLength`, which is an exact `sqrt` in this build
//!   because `USE_ASSEMBLER_CODE` is off).
//! - `dist >= FALLOFF_DISTANCE` (1500 world units): the sound is not played at all.
//! - otherwise the distance volume is `(1500 - dist) / 1500`.
//! - the offset `eye.position - pos` is turned by `OppRotate(.., eye.rotation.y)` (degrees, about the vertical
//!   axis of the world's x/y plane) and `pan = clamp(x / 750, -1, 1)`.
//!
//! `maxSoundDistance` (2800) from `sound.snd` is read by the C++ but never used; the falloff is the literal
//! constant. World coordinates are the game's (x/y ground plane, z up): the caller passes the sim's values.

/// `FALLOFF_DISTANCE` in `soundsys.cpp`.
pub const FALLOFF_DISTANCE: f32 = 1500.0;
/// `DEGREES_TO_RADS` in `dstd.h`.
pub const DEGREES_TO_RADS: f32 = 0.017_453_292;
/// z value that marks "no position" in `playDigitalSample`'s default argument.
pub const NO_POSITION_Z: f32 = -9999.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// `Stuff::Vector3D(-9999, -9999, -9999)`, the default position argument.
    pub const NONE: Vec3 = Vec3::new(-9999.0, -9999.0, -9999.0);

    /// The C++ test `pos.z != -9999.0f`.
    pub fn has_position(&self) -> bool {
        self.z != NO_POSITION_Z
    }

    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

impl From<[f32; 3]> for Vec3 {
    fn from(a: [f32; 3]) -> Self {
        Vec3::new(a[0], a[1], a[2])
    }
}

/// `OppRotate`: rotate `v` about z by `angle` degrees (the opposite sense of `Rotate`).
pub fn opp_rotate(v: Vec3, angle_deg: f32) -> Vec3 {
    let tx = v.x;
    let (sine, cosine) = (angle_deg * DEGREES_TO_RADS).sin_cos();
    let mut out = v;
    out.x *= cosine;
    out.x -= v.y * sine;
    out.y *= cosine;
    out.y += tx * sine;
    out
}

/// The camera as the sound system sees it (`eye->getPosition()`, `eye->getRotation().y`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    /// Camera yaw in degrees (`Camera::cameraRotation`).
    pub rotation_y: f32,
}

/// Result of positioning a sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spatial {
    /// 0..1 multiplier.
    pub distance_volume: f32,
    /// -1 (left) .. 1 (right).
    pub pan: f32,
}

impl Spatial {
    /// A sound with no position: full volume, centred.
    pub const CENTRE: Spatial = Spatial {
        distance_volume: 1.0,
        pan: 0.0,
    };
}

impl Listener {
    /// Attenuation and pan of a sound at `pos`; `None` when it is too far away to play.
    pub fn spatialize(&self, pos: Vec3) -> Option<Spatial> {
        let offset = Vec3::new(
            self.position.x - pos.x,
            self.position.y - pos.y,
            self.position.z - pos.z,
        );
        let dist = offset.length();
        if dist >= FALLOFF_DISTANCE {
            return None;
        }
        let distance_volume = (FALLOFF_DISTANCE - dist) / FALLOFF_DISTANCE;
        let rotated = opp_rotate(offset, self.rotation_y);
        let pan = (rotated.x / (FALLOFF_DISTANCE * 0.5)).clamp(-1.0, 1.0);
        Some(Spatial {
            distance_volume,
            pan,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn distance_volume_is_linear_to_1500() {
        let l = Listener::default();
        let at = |d: f32| l.spatialize(Vec3::new(0.0, d, 0.0));
        assert!(close(at(0.0).unwrap().distance_volume, 1.0));
        assert!(close(at(750.0).unwrap().distance_volume, 0.5));
        assert!(close(at(1200.0).unwrap().distance_volume, 0.2));
        assert!(close(at(1499.0).unwrap().distance_volume, 1.0 / 1500.0));
        // `dist < FALLOFF_DISTANCE` is the C++ test: exactly 1500 is too far.
        assert!(at(1500.0).is_none());
        assert!(at(5000.0).is_none());
    }

    #[test]
    fn distance_is_three_dimensional() {
        let l = Listener::default();
        let s = l.spatialize(Vec3::new(300.0, 0.0, 400.0)).unwrap();
        assert!(close(s.distance_volume, (1500.0 - 500.0) / 1500.0));
        // 3-4-5 triangle in x/z; the camera's height counts too.
        let l2 = Listener {
            position: Vec3::new(0.0, 0.0, 400.0),
            rotation_y: 0.0,
        };
        let s2 = l2.spatialize(Vec3::new(300.0, 0.0, 0.0)).unwrap();
        assert!(close(s2.distance_volume, s.distance_volume));
    }

    #[test]
    fn pan_is_offset_over_half_the_falloff() {
        let l = Listener::default();
        // eye - pos has x = -375 -> pan -0.5; the sign follows the C++ (camera minus source).
        let s = l.spatialize(Vec3::new(375.0, 0.0, 0.0)).unwrap();
        assert!(close(s.pan, -0.5));
        let s = l.spatialize(Vec3::new(-375.0, 0.0, 0.0)).unwrap();
        assert!(close(s.pan, 0.5));
        // Fully to one side at 750 and beyond (the clamp), while still audible.
        assert!(close(
            l.spatialize(Vec3::new(-900.0, 0.0, 0.0)).unwrap().pan,
            1.0
        ));
        assert!(close(
            l.spatialize(Vec3::new(900.0, 0.0, 0.0)).unwrap().pan,
            -1.0
        ));
        // Straight ahead or behind is centred.
        assert!(close(
            l.spatialize(Vec3::new(0.0, 600.0, 0.0)).unwrap().pan,
            0.0
        ));
    }

    #[test]
    fn pan_follows_camera_rotation() {
        // OppRotate(v, a): x' = x cos a - y sin a. Offset (0, -600): a source 600 ahead on +y of the
        // listener's origin; turning the camera 90 degrees moves it into the x axis.
        let mut l = Listener::default();
        let src = Vec3::new(0.0, 600.0, 0.0);
        assert!(close(l.spatialize(src).unwrap().pan, 0.0));
        l.rotation_y = 90.0;
        // offset = (0, -600); x' = 0*cos - (-600)*sin(90) = 600 -> pan 0.8
        assert!(close(l.spatialize(src).unwrap().pan, 0.8));
        l.rotation_y = 180.0;
        assert!(close(l.spatialize(src).unwrap().pan, 0.0));
        l.rotation_y = 270.0;
        assert!(close(l.spatialize(src).unwrap().pan, -0.8));
        l.rotation_y = 360.0;
        assert!(close(l.spatialize(src).unwrap().pan, 0.0));
    }

    #[test]
    fn opp_rotate_matches_the_c_formula() {
        let v = Vec3::new(3.0, 4.0, 5.0);
        let a = 30.0_f32;
        let s = (a * DEGREES_TO_RADS).sin();
        let c = (a * DEGREES_TO_RADS).cos();
        let r = opp_rotate(v, a);
        assert!(close(r.x, 3.0 * c - 4.0 * s));
        assert!(close(r.y, 4.0 * c + 3.0 * s));
        assert_eq!(r.z, 5.0);
        // Rotation preserves the x/y length.
        assert!(close(
            (r.x * r.x + r.y * r.y).sqrt(),
            (v.x * v.x + v.y * v.y).sqrt()
        ));
    }

    #[test]
    fn no_position_sentinel() {
        assert!(!Vec3::NONE.has_position());
        assert!(Vec3::new(0.0, 0.0, 0.0).has_position());
        // Only z is tested, as in the C++.
        assert!(!Vec3::new(1.0, 2.0, -9999.0).has_position());
    }
}
