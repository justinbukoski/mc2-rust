//! Random numbers for audio decisions.
//!
//! The C++ draws radio style rolls from `RandomNumber(range)` (`gos_rand() % range`) in `Radio::playMessage`.
//! The port never uses global randomness: the caller passes an [`AudioRng`], normally the sim's *visual*
//! stream (the pinned design makes radio chatter a visual draw), so replays stay deterministic and the
//! sim's own stream is never disturbed.

/// Source of `RandomNumber(range)`.
pub trait AudioRng {
    /// A value in `0..range` (`range > 0`).
    fn random_number(&mut self, range: i32) -> i32;
}

impl<F: FnMut(i32) -> i32> AudioRng for F {
    fn random_number(&mut self, range: i32) -> i32 {
        self(range)
    }
}

/// A fixed script of results, cycled; each is reduced `% range`. For tests.
#[derive(Clone, Debug)]
pub struct ScriptedRng {
    values: Vec<i32>,
    next: usize,
    /// Every `range` asked for, in order.
    pub calls: Vec<i32>,
}

impl ScriptedRng {
    pub fn new(values: &[i32]) -> Self {
        assert!(!values.is_empty());
        Self {
            values: values.to_vec(),
            next: 0,
            calls: Vec::new(),
        }
    }
}

impl AudioRng for ScriptedRng {
    fn random_number(&mut self, range: i32) -> i32 {
        self.calls.push(range);
        let v = self.values[self.next % self.values.len()];
        self.next += 1;
        if range <= 0 {
            0
        } else {
            v.rem_euclid(range)
        }
    }
}

/// A small seeded generator (splitmix64) for tools and tests that need a stream of draws.
#[derive(Clone, Debug)]
pub struct SeededRng(u64);

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
}

impl AudioRng for SeededRng {
    fn random_number(&mut self, range: i32) -> i32 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        if range <= 0 {
            0
        } else {
            ((z >> 17) % range as u64) as i32
        }
    }
}
