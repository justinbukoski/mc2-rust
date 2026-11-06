// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! gosFX F-curves (`mclib/gosfx/fcurve.{hpp,cpp}`).
//!
//! Five plain curves and a seeded product:
//! - `ConstantCurve`: `v`.
//! - `LinearCurve`: `slope * t + v`.
//! - `SplineCurve`: `((a * t + b) * t + slope) * t + v`.
//! - `ComplexCurve`: piecewise linear keys, each key a line `slope * (t - key.time) + value`
//!   (so the curve may jump at a key).
//! - `Seeded<C, S>`: `age_curve(age)` times, when `seeded`, `seed_curve(seed)`.
//!
//! The C++ tells the seeded combinations apart with a `CurveType` tag and casts; here they are
//! one generic type, loaded in the same byte order: age curve, seed curve, `seeded` flag.

use crate::error::Result;
use crate::math::SMALL;
use crate::stream::Stream;

/// What every curve can do. Plain curves ignore the `seed` argument, exactly like the C++
/// `ComputeValue(t, seed)` overloads.
pub trait FCurve: Sized + Default {
    fn compute(&self, t: f32, seed: f32) -> f32;
    /// `ComputeRange`: the (low, high) the curve reaches over `t` in `0..=1`.
    fn range(&self) -> (f32, f32);
    fn load(s: &mut Stream, gfx_version: i32) -> Result<Self>;
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ConstantCurve {
    pub value: f32,
}

impl ConstantCurve {
    pub fn set_curve(&mut self, v: f32) {
        self.value = v;
    }
}

impl FCurve for ConstantCurve {
    fn compute(&self, _t: f32, _seed: f32) -> f32 {
        self.value
    }
    fn range(&self) -> (f32, f32) {
        (self.value, self.value)
    }
    fn load(s: &mut Stream, _v: i32) -> Result<Self> {
        Ok(Self { value: s.f32()? })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct LinearCurve {
    pub value: f32,
    pub slope: f32,
}

impl LinearCurve {
    /// `SetCurve(v0, v1)`.
    pub fn set_curve(&mut self, v0: f32, v1: f32) {
        self.slope = v1 - v0;
        self.value = v0;
    }
}

impl FCurve for LinearCurve {
    fn compute(&self, t: f32, _seed: f32) -> f32 {
        self.slope * t + self.value
    }
    fn range(&self) -> (f32, f32) {
        let mut hi = self.value;
        let mut lo = self.value;
        let t = self.slope + self.value;
        if t > hi {
            hi = t;
        } else if t < lo {
            lo = t;
        }
        (lo, hi)
    }
    fn load(s: &mut Stream, _v: i32) -> Result<Self> {
        let value = s.f32()?;
        let slope = s.f32()?;
        Ok(Self { value, slope })
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SplineCurve {
    pub value: f32,
    pub slope: f32,
    pub b: f32,
    pub a: f32,
}

impl SplineCurve {
    /// `SetCurve(v0, s0, v1, s1)`: the cubic Hermite through the end values and slopes.
    pub fn set_curve_hermite(&mut self, v0: f32, s0: f32, v1: f32, s1: f32) {
        self.a = (s1 + s0) + 2.0 * (v0 - v1);
        self.b = (s1 - s0 - 3.0 * self.a) * 0.5;
        self.slope = s0;
        self.value = v0;
    }
    pub fn compute_slope(&self, t: f32) -> f32 {
        (3.0 * self.a * t + 2.0 * self.b) * t + self.slope
    }
}

impl FCurve for SplineCurve {
    fn compute(&self, t: f32, _seed: f32) -> f32 {
        ((self.a * t + self.b) * t + self.slope) * t + self.value
    }
    /// `SplineCurve::ComputeRange`: the ends plus the interior local extrema (the roots of
    /// the derivative inside `(0, 1)`).
    fn range(&self) -> (f32, f32) {
        let mut hi = self.value;
        let mut lo = self.value;
        let take = |t: f32, lo: &mut f32, hi: &mut f32| {
            if t < *lo {
                *lo = t;
            } else if t > *hi {
                *hi = t;
            }
        };
        let end = self.compute(1.0, 0.0);
        if end > hi {
            hi = end;
        } else if end < lo {
            lo = end;
        }
        if self.a == 0.0 {
            if self.b != 0.0 {
                let t = -0.5 * self.slope / self.b;
                if t > 0.0 && t < 1.0 {
                    take(self.compute(t, 0.0), &mut lo, &mut hi);
                }
            }
            return (lo, hi);
        }
        let mut da = 3.0 * self.a;
        let mut db = 2.0 * self.b;
        let mut range = db * db - 4.0 * da * self.slope;
        if range < 0.0 {
            return (lo, hi);
        }
        da = 0.5 / da;
        db = -db * da;
        range = range.sqrt() * da;
        let t = db - range;
        if t > 0.0 && t < 1.0 {
            take(self.compute(t, 0.0), &mut lo, &mut hi);
        }
        let t = db + range;
        if t > 0.0 && t < 1.0 {
            take(self.compute(t, 0.0), &mut lo, &mut hi);
        }
        (lo, hi)
    }
    fn load(s: &mut Stream, _v: i32) -> Result<Self> {
        // LinearCurve (value, slope), then `m_b`, then `m_a`.
        let value = s.f32()?;
        let slope = s.f32()?;
        let b = s.f32()?;
        let a = s.f32()?;
        Ok(Self { value, slope, b, a })
    }
}

/// `CurveKey`: the line `slope * (t - time) + value` from `time` to the next key.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct CurveKey {
    pub time: f32,
    pub slope: f32,
    pub value: f32,
}

impl CurveKey {
    pub fn compute(&self, dt: f32) -> f32 {
        self.slope * dt + self.value
    }
    pub fn set_linear_key(&mut self, key_time: f32, v0: f32, v1: f32, dt: f32) {
        self.time = key_time;
        self.slope = (v1 - v0) / dt;
        self.value = v0;
    }
    /// `CurveKey::ComputeRange`.
    pub fn range(&self, dt: f32) -> (f32, f32) {
        let mut hi = self.value;
        let mut lo = self.value;
        if dt < SMALL {
            return (lo, hi);
        }
        let t = self.compute(dt);
        if t > hi {
            hi = t;
        } else if t < lo {
            lo = t;
        }
        (lo, hi)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComplexCurve {
    pub keys: Vec<CurveKey>,
}

impl ComplexCurve {
    /// `GetKeyIndex`: index of the last key whose time is not after `time` (-1 before the
    /// first key).
    pub fn key_index(&self, time: f32) -> isize {
        let mut i = 0usize;
        while i < self.keys.len() {
            if self.keys[i].time > time {
                break;
            }
            i += 1;
        }
        i as isize - 1
    }

    /// `SetCurve(v)`: one constant key.
    pub fn set_constant(&mut self, v: f32) {
        self.keys = vec![CurveKey {
            time: 0.0,
            slope: 0.0,
            value: v,
        }];
    }

    /// `SetCurve(start, end)`: one linear key over `0..1`.
    pub fn set_linear(&mut self, start: f32, end: f32) {
        let mut k = CurveKey::default();
        k.set_linear_key(0.0, start, end, 1.0);
        self.keys = vec![k];
    }
}

impl FCurve for ComplexCurve {
    /// `ComplexCurve::ComputeValue`. A time before the first key would index key -1 in the
    /// C++; the first key is used instead.
    fn compute(&self, t: f32, _seed: f32) -> f32 {
        if self.keys.is_empty() {
            return 0.0;
        }
        let i = self.key_index(t).max(0) as usize;
        let key = &self.keys[i];
        key.compute(t - key.time)
    }
    fn range(&self) -> (f32, f32) {
        let n = self.keys.len();
        if n == 0 {
            return (0.0, 0.0);
        }
        let last = &self.keys[n - 1];
        let dt = (1.0 - last.time).max(0.0);
        let (mut lo, mut hi) = last.range(dt);
        for i in (0..n - 1).rev() {
            let key = &self.keys[i];
            let (l, h) = key.range(self.keys[i + 1].time - key.time);
            if h > hi {
                hi = h;
            }
            if l < lo {
                lo = l;
            }
        }
        (lo, hi)
    }
    fn load(s: &mut Stream, gfx_version: i32) -> Result<Self> {
        if gfx_version < 15 {
            let _duration = s.f32()?;
        }
        let n = s.count(12)?;
        let mut keys = Vec::with_capacity(n);
        for _ in 0..n {
            let time = s.f32()?;
            let slope = s.f32()?;
            let value = s.f32()?;
            keys.push(CurveKey { time, slope, value });
        }
        // "If we are reading a previous version, make sure the tail doesn't go below zero."
        // (The C++ runs this for every version.)
        if let Some(key) = keys.last_mut() {
            let dt = (1.0 - key.time).max(0.0);
            let (low, _hi) = key.range(dt);
            if low < 0.0 && dt > SMALL {
                let (t, v) = (key.time, key.value);
                key.set_linear_key(t, v, 0.0, dt);
            }
        }
        Ok(Self { keys })
    }
}

/// `SeededCurveOf<C, S>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seeded<C, S> {
    pub age_curve: C,
    pub seed_curve: S,
    pub seeded: bool,
}

impl<C: FCurve, S: FCurve> FCurve for Seeded<C, S> {
    fn compute(&self, age: f32, seed: f32) -> f32 {
        let mut result = self.age_curve.compute(age, 0.0);
        if self.seeded {
            result *= self.seed_curve.compute(seed, 0.0);
        }
        result
    }
    fn range(&self) -> (f32, f32) {
        let (low_age, hi_age) = self.age_curve.range();
        let (low_seed, hi_seed) = self.seed_curve.range();
        let mut lo = low_age * low_seed;
        let mut hi = lo;
        for temp in [low_age * hi_seed, hi_age * low_seed, hi_age * hi_seed] {
            if temp < lo {
                lo = temp;
            } else if temp > hi {
                hi = temp;
            }
        }
        (lo, hi)
    }
    fn load(s: &mut Stream, gfx_version: i32) -> Result<Self> {
        let age_curve = C::load(s, gfx_version)?;
        let seed_curve = S::load(s, gfx_version)?;
        let seeded = s.bool()?;
        Ok(Self {
            age_curve,
            seed_curve,
            seeded,
        })
    }
}

// The combinations the effect specifications use (`Curve::CurveType`).
pub type ComplexLinear = Seeded<ComplexCurve, LinearCurve>;
pub type ComplexComplex = Seeded<ComplexCurve, ComplexCurve>;
pub type ComplexSpline = Seeded<ComplexCurve, SplineCurve>;
pub type ConstantComplex = Seeded<ConstantCurve, ComplexCurve>;
pub type ConstantLinear = Seeded<ConstantCurve, LinearCurve>;
pub type ConstantSpline = Seeded<ConstantCurve, SplineCurve>;
pub type SplineLinear = Seeded<SplineCurve, LinearCurve>;
pub type SplineSpline = Seeded<SplineCurve, SplineCurve>;

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    fn bytes(vals: &[f32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    #[test]
    fn constant_and_linear() {
        let c = ConstantCurve { value: 2.5 };
        assert_eq!(c.compute(0.7, 0.3), 2.5);
        assert_eq!(c.range(), (2.5, 2.5));
        // slope * t + value: 3 * 0.25 + 1 = 1.75
        let l = LinearCurve {
            value: 1.0,
            slope: 3.0,
        };
        assert!(close(l.compute(0.25, 0.9), 1.75));
        assert_eq!(l.range(), (1.0, 4.0));
        let down = LinearCurve {
            value: 1.0,
            slope: -3.0,
        };
        assert_eq!(down.range(), (-2.0, 1.0));
        let mut s = LinearCurve::default();
        s.set_curve(2.0, 5.0);
        assert_eq!((s.value, s.slope), (2.0, 3.0));
    }

    #[test]
    fn spline_value_and_hermite() {
        // a=1 b=2 slope=3 v=4 at t=0.5: ((1*.5+2)*.5+3)*.5+4 = 4.0 + 2.125 = 6.125
        let s = SplineCurve {
            value: 4.0,
            slope: 3.0,
            b: 2.0,
            a: 1.0,
        };
        assert!(close(s.compute(0.5, 0.0), 6.125));
        // slope: (3*1*.5 + 2*2)*.5 + 3 = (1.5+4)*.5+3 = 5.75
        assert!(close(s.compute_slope(0.5), 5.75));
        // Hermite through (0, v0=1, slope 0) and (1, v1=3, slope 0): smoothstep scaled, mid = 2
        let mut h = SplineCurve::default();
        h.set_curve_hermite(1.0, 0.0, 3.0, 0.0);
        assert!(close(h.compute(0.0, 0.0), 1.0));
        assert!(close(h.compute(1.0, 0.0), 3.0));
        assert!(close(h.compute(0.5, 0.0), 2.0));
        assert!(close(h.compute_slope(1.0), 0.0));
    }

    #[test]
    fn spline_range_finds_interior_extremum() {
        // v = t - t^2... as a cubic with a=0: value 0, slope 1, b=-1: peak at t=0.5, value 0.25
        let s = SplineCurve {
            value: 0.0,
            slope: 1.0,
            b: -1.0,
            a: 0.0,
        };
        let (lo, hi) = s.range();
        assert!(close(lo, 0.0));
        assert!(close(hi, 0.25));
        // cubic 4t^3 - 6t^2 + 2t: the derivative 12t^2 - 12t + 2 vanishes at 0.5 -+ sqrt(3)/6
        let c = SplineCurve {
            value: 0.0,
            slope: 2.0,
            b: -6.0,
            a: 4.0,
        };
        let (lo, hi) = c.range();
        let at = |t: f32| 4.0 * t * t * t - 6.0 * t * t + 2.0 * t;
        let d = 3f32.sqrt() / 6.0;
        assert!(close(hi, at(0.5 - d)));
        assert!(close(lo, at(0.5 + d)));
    }

    #[test]
    fn complex_curve_keys() {
        // key 0: 0 -> 10 over [0, .5); key 1 at .5: constant 4 then slope -8 ->0 at 1
        let c = ComplexCurve {
            keys: vec![
                CurveKey {
                    time: 0.0,
                    slope: 20.0,
                    value: 0.0,
                },
                CurveKey {
                    time: 0.5,
                    slope: -8.0,
                    value: 4.0,
                },
            ],
        };
        assert!(close(c.compute(0.25, 0.0), 5.0));
        // exactly on a key time: the later key wins (the discontinuity jumps to 4)
        assert!(close(c.compute(0.5, 0.0), 4.0));
        assert!(close(c.compute(0.75, 0.0), 2.0));
        assert!(close(c.compute(1.0, 0.0), 0.0));
        let (lo, hi) = c.range();
        assert!(close(lo, 0.0));
        assert!(close(hi, 10.0));
        assert_eq!(c.key_index(-1.0), -1);
        assert_eq!(c.key_index(0.6), 1);
    }

    #[test]
    fn complex_curve_set_helpers() {
        let mut c = ComplexCurve::default();
        c.set_constant(3.0);
        assert!(close(c.compute(0.9, 0.0), 3.0));
        c.set_linear(2.0, 6.0);
        assert!(close(c.compute(0.5, 0.0), 4.0));
    }

    #[test]
    fn seeded_product_and_range() {
        let s = ConstantLinear {
            age_curve: ConstantCurve { value: 2.0 },
            seed_curve: LinearCurve {
                value: 1.0,
                slope: 2.0,
            },
            seeded: true,
        };
        // 2 * (2*0.25 + 1) = 3
        assert!(close(s.compute(0.9, 0.25), 3.0));
        let (lo, hi) = s.range();
        assert!(close(lo, 2.0) && close(hi, 6.0));
        let unseeded = ConstantLinear {
            seeded: false,
            ..s.clone()
        };
        assert!(close(unseeded.compute(0.9, 0.25), 2.0));
    }

    #[test]
    fn load_byte_orders() {
        // Spline: value, slope, b, a
        let b = bytes(&[4.0, 3.0, 2.0, 1.0]);
        let s = SplineCurve::load(&mut Stream::new(&b), 17).unwrap();
        assert_eq!((s.value, s.slope, s.b, s.a), (4.0, 3.0, 2.0, 1.0));
        // Seeded<Constant, Linear>: age(1) seed(2) flag
        let mut b = bytes(&[7.0, 1.0, 2.0]);
        b.push(1);
        let sc = ConstantLinear::load(&mut Stream::new(&b), 17).unwrap();
        assert!(sc.seeded);
        assert_eq!(sc.age_curve.value, 7.0);
        assert_eq!((sc.seed_curve.value, sc.seed_curve.slope), (1.0, 2.0));
        // Complex: count + keys (time, slope, value); version >= 15 has no duration
        let mut b = 2u32.to_le_bytes().to_vec();
        b.extend(bytes(&[0.0, 10.0, 1.0, 0.5, 0.0, 6.0]));
        let c = ComplexCurve::load(&mut Stream::new(&b), 17).unwrap();
        assert_eq!(c.keys.len(), 2);
        assert_eq!(c.keys[1].value, 6.0);
        // version 14 has the leading duration float
        let mut b = bytes(&[99.0]);
        b.extend(1u32.to_le_bytes());
        b.extend(bytes(&[0.0, 0.0, 2.0]));
        let c = ComplexCurve::load(&mut Stream::new(&b), 14).unwrap();
        assert_eq!(c.keys[0].value, 2.0);
    }

    #[test]
    fn complex_load_clamps_a_tail_that_goes_negative() {
        // last key at 0.5 with value 1 and slope -4: reaches -1 at t = 1 -> rewritten to
        // finish at exactly 0: slope = (0 - 1) / 0.5 = -2
        let mut b = 1u32.to_le_bytes().to_vec();
        b.extend(bytes(&[0.5, -4.0, 1.0]));
        let c = ComplexCurve::load(&mut Stream::new(&b), 17).unwrap();
        assert!(close(c.keys[0].slope, -2.0));
        assert!(close(c.compute(1.0, 0.0), 0.0));
    }
}
