//! Portable transcendental functions for the simulation.
//!
//! Platform libms disagree in the last bit (macOS arm64, macOS x86_64, glibc and MSVC all differ, and none is correctly
//! rounded), so the platform's `sin`/`cos`/`pow`/... cannot be used where results must be reproducible. Every transcendental
//! the simulation uses goes through this crate, which is a thin facade over the Rust [`libm`] crate (a port of musl libm,
//! pinned to an exact version). That code uses only IEEE-754 `+ - * /`, comparisons and integer operations on `f32`/`f64`
//! (all exactly specified, no fused multiply-add: Rust never contracts, and the architecture intrinsics are disabled), so it
//! returns the same bits on every target.
//!
//! `mc2-sim/oracle/third_party/musl-libm` holds the musl 1.1.21 C sources the `libm` 0.2.16 release was ported from, and
//! `tests/musl_twin.rs` checks the two bit for bit (the oracle build links these same C sources, through
//! `mc2-sim/oracle/libm_override.c`, so the C++ oracle returns the same bits on every host; the goldens recorded before that
//! change were made with the platform libm and are compared with tolerances). `sqrt`, `floor`, `ceil`, `fabs` and `fmod` are exactly specified by IEEE 754 and may use the platform.

pub use libm::{
    acos, acosf, asin, asinf, atan, atan2, atan2f, atanf, cos, cosf, exp, expf, hypot, hypotf, log,
    logf, pow, powf, sin, sinf, tan, tanf,
};

/// `(sinf(x), cosf(x))`, the pair `Stuff::SinCosPair` computes.
pub fn sin_cosf(x: f32) -> (f32, f32) {
    (sinf(x), cosf(x))
}
