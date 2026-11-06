//! 8x8 inverse DCT, as specified by ISO/IEC 11172-2 Annex A (the ideal real-valued transform),
//! evaluated in fixed point with enough precision that results match the real-valued transform
//! rounded to nearest, except where the exact value lies within about 2^-14 of a half.
//!
//! One dimension is `x[n] = 1/2 * sum_k C(k) X[k] cos((2n+1) k pi / 16)` with `C(0) = 1/sqrt(2)`
//! and `C(k) = 1` otherwise; the 2-D transform applies it to the rows, then to the columns.

/// Fractional bits of the cosine constants.
const FRAC: u32 = 24;

/// `round(cos(m pi / 16) * 2^FRAC)` for m = 0..=8.
const COS16: [i64; 9] = [
    16777216, 16454846, 15500126, 13949745, 11863283, 9320922, 6420363, 3273072, 0,
];

/// `cos(m pi / 16)` for any integer m, from the table above.
const fn cos16(m: usize) -> i64 {
    let m = m % 32;
    let m = if m > 16 { 32 - m } else { m };
    if m > 8 {
        -COS16[16 - m]
    } else {
        COS16[m]
    }
}

/// `M[u][x] = C(u) * cos((2x+1) u pi / 16)` in Q(FRAC).
const fn matrix() -> [[i64; 8]; 8] {
    let mut m = [[0i64; 8]; 8];
    let mut u = 0;
    while u < 8 {
        let mut x = 0;
        while x < 8 {
            m[u][x] = if u == 0 {
                COS16[4]
            } else {
                cos16((2 * x + 1) * u)
            };
            x += 1;
        }
        u += 1;
    }
    m
}

const M: [[i64; 8]; 8] = matrix();

/// Fractional bits kept between the two passes.
const MID_BITS: u32 = 12;

/// In-place inverse DCT of a dequantised block (row major, 12-bit signed range). The result is
/// the residual / sample values, rounded but not clamped.
pub fn idct8x8(block: &mut [i32; 64]) {
    // DC only: every output equals X0 / 8.
    if block[1..].iter().all(|&c| c == 0) {
        let v = (block[0] + 4) >> 3;
        block.fill(v);
        return;
    }
    // Rows. Output scaled by 2^MID_BITS: sum(M * X) is scaled by 2^FRAC, and the 1/2 of the 1-D
    // transform removes one more bit.
    let mut tmp = [0i64; 64];
    let shift_a = FRAC + 1 - MID_BITS;
    for r in 0..8 {
        let row = &block[r * 8..r * 8 + 8];
        if row.iter().all(|&c| c == 0) {
            continue;
        }
        for x in 0..8 {
            let mut acc = 0i64;
            for u in 0..8 {
                acc += M[u][x] * i64::from(row[u]);
            }
            tmp[r * 8 + x] = (acc + (1 << (shift_a - 1))) >> shift_a;
        }
    }
    // Columns.
    let shift_b = FRAC + 1 + MID_BITS;
    for c in 0..8 {
        for y in 0..8 {
            let mut acc = 0i64;
            for u in 0..8 {
                acc += M[u][y] * tmp[u * 8 + c];
            }
            block[y * 8 + c] = ((acc + (1 << (shift_b - 1))) >> shift_b) as i32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(block: &[i32; 64]) -> [f64; 64] {
        let mut out = [0f64; 64];
        let c = |k: usize| if k == 0 { 0.5f64.sqrt() } else { 1.0 };
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0.0;
                for v in 0..8 {
                    for u in 0..8 {
                        s += c(u)
                            * c(v)
                            * f64::from(block[v * 8 + u])
                            * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos()
                            * ((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI / 16.0).cos();
                    }
                }
                out[y * 8 + x] = s / 4.0;
            }
        }
        out
    }

    /// Small deterministic generator (xorshift) so the test needs no dependencies.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn range(&mut self, lo: i32, hi: i32) -> i32 {
            lo + (self.next() % (hi - lo + 1) as u64) as i32
        }
    }

    #[test]
    fn matches_the_real_valued_transform() {
        let mut rng = Rng(0x1234_5678_9abc_def1);
        let mut worst = 0.0f64;
        let mut inexact = 0;
        let mut total = 0;
        for iter in 0..4000 {
            let mut b = [0i32; 64];
            let (lo, hi, density) = match iter % 4 {
                0 => (-256, 255, 64),
                1 => (-2048, 2047, 64),
                2 => (-5, 5, 64),
                _ => (-300, 300, 6),
            };
            for v in &mut b {
                if rng.next() % 64 < density {
                    *v = rng.range(lo, hi);
                }
            }
            let want = reference(&b);
            let mut got = b;
            idct8x8(&mut got);
            for i in 0..64 {
                let d = (f64::from(got[i]) - want[i]).abs();
                worst = worst.max(d);
                total += 1;
                if got[i] != want[i].round() as i32 {
                    inexact += 1;
                }
            }
        }
        // Never more than half a unit plus the fixed-point error from the ideal value.
        assert!(worst <= 0.5 + 2e-3, "worst error {worst}");
        // Differences from round-to-nearest only for values within ~1e-3 of a half.
        assert!(
            (inexact as f64) < total as f64 * 1e-3,
            "{inexact}/{total} differ"
        );
    }

    #[test]
    fn dc_only_block_is_flat() {
        let mut b = [0i32; 64];
        b[0] = 1024;
        idct8x8(&mut b);
        assert!(b.iter().all(|&v| v == 128));
        let mut b = [0i32; 64];
        b[0] = -3;
        idct8x8(&mut b);
        assert!(b.iter().all(|&v| v == 0));
    }
}
