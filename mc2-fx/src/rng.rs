// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The visual random stream.
//!
//! gosFX draws every random number from `Stuff::Random::GetFraction()` (`mclib/stuff/random.cpp`):
//! an additive lagged-Fibonacci style table of 250 integers (each new value is the XOR of
//! entry `Index` and the entry 103 places on), seeded from `gos_rand()`. gosFX never calls the
//! C library `rand()` directly. That table is a *global* shared with the rest of the game in the
//! C++ (so effects and, for example, sim code interleave draws); the port keeps an effect-only
//! stream that nothing in the simulation can see or disturb. The generator itself is the same
//! table algorithm, seeded from a small LCG (the Microsoft CRT `rand()` recurrence) so that a
//! given seed always plays back identically.

const TABLE: usize = 250;
const RAND_MAX: i32 = (1 << 15) - 1;

#[derive(Clone, Debug)]
pub struct VisualRng {
    numbers: [i32; TABLE],
    index: usize,
}

impl VisualRng {
    pub fn new(seed: u32) -> Self {
        let mut state = seed;
        let mut next = || {
            // Microsoft C runtime rand(): 15 bits from a 32-bit LCG.
            state = state.wrapping_mul(214013).wrapping_add(2531011);
            ((state >> 16) & 0x7fff) as i32
        };
        let mut numbers = [0i32; TABLE];
        for n in numbers.iter_mut() {
            *n = next();
        }
        // `Random::Init`: keep a good mix for the XOR by forcing 15 words to have a descending
        // leading-one pattern (01xx..., 001xx..., ...).
        let mut mask = RAND_MAX >> 1;
        let mut msb = mask + 1;
        let mut rand_size = 0;
        while msb & (1 << rand_size) == 0 {
            rand_size += 1;
        }
        let mut i = 14usize;
        while rand_size > 0 {
            rand_size -= 1;
            numbers[i] &= mask;
            numbers[i] |= msb;
            mask >>= 1;
            msb >>= 1;
            i += 11;
        }
        Self { numbers, index: 0 }
    }

    /// `Random::GetRandomInt`.
    pub fn get_int(&mut self) -> i32 {
        let indent = if self.index >= 147 {
            self.index - 147
        } else {
            self.index + 103
        };
        let result = self.numbers[self.index] ^ self.numbers[indent];
        self.numbers[self.index] = result;
        self.index += 1;
        if self.index == TABLE {
            self.index = 0;
        }
        result
    }

    /// `Random::GetFraction`: `0.0 <= x < 1.0`.
    pub fn fraction(&mut self) -> f32 {
        self.get_int() as f32 / (RAND_MAX + 1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = VisualRng::new(7);
        let mut b = VisualRng::new(7);
        for _ in 0..2000 {
            let x = a.fraction();
            assert_eq!(x, b.fraction());
            assert!((0.0..1.0).contains(&x));
        }
        let mut c = VisualRng::new(8);
        assert_ne!(VisualRng::new(7).fraction(), c.fraction());
    }

    #[test]
    fn xor_recurrence() {
        // After construction the first output is Numbers[0] ^ Numbers[103].
        let r = VisualRng::new(1);
        let expect = r.numbers[0] ^ r.numbers[103];
        let mut r = r;
        assert_eq!(r.get_int(), expect);
    }
}
