//! Fixed-step game clock.
//!
//! The simulation advances in whole, fixed-length ticks regardless of how fast frames are drawn;
//! rendering interpolates between the last two ticks with [`FixedClock::alpha`]. Time is kept in
//! integer nanoseconds so no drift accumulates.

use std::time::{Duration, Instant};

/// Accumulates real elapsed time and hands out whole simulation ticks.
#[derive(Clone, Debug)]
pub struct FixedClock {
    step_ns: u64,
    acc_ns: u64,
    max_steps: u32,
    ticks: u64,
    paused: bool,
    time_scale: f64,
    /// Fractional nanoseconds left over from time scaling.
    scale_carry: f64,
    dropped_ns: u64,
}

impl FixedClock {
    /// A clock ticking `hz` times per second.
    pub fn from_hz(hz: u32) -> Self {
        assert!(hz > 0, "tick rate must be positive");
        Self::new(Duration::from_nanos(1_000_000_000 / hz as u64))
    }

    pub fn new(step: Duration) -> Self {
        let step_ns = step.as_nanos() as u64;
        assert!(step_ns > 0, "tick length must be positive");
        Self {
            step_ns,
            acc_ns: 0,
            // Spiral-of-death guard: never simulate more than this many ticks per frame.
            max_steps: 8,
            ticks: 0,
            paused: false,
            time_scale: 1.0,
            scale_carry: 0.0,
            dropped_ns: 0,
        }
    }

    /// Cap on ticks returned per [`advance`](Self::advance); excess time is discarded.
    pub fn with_max_steps(mut self, n: u32) -> Self {
        self.max_steps = n.max(1);
        self
    }

    pub fn step(&self) -> Duration {
        Duration::from_nanos(self.step_ns)
    }

    /// Seconds of game time per tick (before time scaling).
    pub fn step_seconds(&self) -> f64 {
        self.step_ns as f64 / 1e9
    }

    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Game speed multiplier (1.0 normal, 2.0 double speed, 0.0 stopped).
    pub fn set_time_scale(&mut self, scale: f64) {
        self.time_scale = scale.max(0.0);
    }

    pub fn time_scale(&self) -> f64 {
        self.time_scale
    }

    /// Add `real_dt` of wall-clock time; returns how many ticks to simulate now.
    pub fn advance(&mut self, real_dt: Duration) -> u32 {
        if self.paused {
            return 0;
        }
        let scaled = if self.time_scale == 1.0 {
            real_dt.as_nanos() as u64
        } else {
            let exact = real_dt.as_nanos() as f64 * self.time_scale + self.scale_carry;
            let whole = exact.floor();
            self.scale_carry = exact - whole;
            whole as u64
        };
        self.acc_ns = self.acc_ns.saturating_add(scaled);
        let mut n = (self.acc_ns / self.step_ns).min(u32::MAX as u64) as u32;
        if n > self.max_steps {
            // Too far behind: drop the surplus instead of spiralling.
            let surplus = (n - self.max_steps) as u64 * self.step_ns;
            self.acc_ns -= surplus;
            self.dropped_ns += surplus;
            n = self.max_steps;
        }
        self.acc_ns -= n as u64 * self.step_ns;
        self.ticks += n as u64;
        n
    }

    /// Fraction `0.0..1.0` of the way to the next tick, for render interpolation.
    pub fn alpha(&self) -> f32 {
        (self.acc_ns as f64 / self.step_ns as f64) as f32
    }

    /// Total simulated game time.
    pub fn game_time(&self) -> Duration {
        Duration::from_nanos(self.ticks.saturating_mul(self.step_ns))
    }

    /// Time discarded because the simulation fell too far behind.
    pub fn dropped(&self) -> Duration {
        Duration::from_nanos(self.dropped_ns)
    }
}

/// Wall-clock frame timer that clamps pathological frame times (breakpoints, window drags).
#[derive(Debug)]
pub struct FrameTimer {
    last: Instant,
    max_frame: Duration,
}

impl FrameTimer {
    pub fn new(max_frame: Duration) -> Self {
        Self {
            last: Instant::now(),
            max_frame,
        }
    }

    /// Time since the previous call, clamped to the configured maximum.
    pub fn tick(&mut self) -> Duration {
        let now = Instant::now();
        let dt = now.saturating_duration_since(self.last);
        self.last = now;
        dt.min(self.max_frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn steps_accumulate_exactly() {
        let mut c = FixedClock::from_hz(50); // 20 ms
        assert_eq!(c.advance(15 * MS), 0);
        assert_eq!(c.advance(15 * MS), 1); // 30 ms -> one tick, 10 ms left
        assert!((c.alpha() - 0.5).abs() < 1e-6);
        assert_eq!(c.advance(10 * MS), 1);
        assert_eq!(c.alpha(), 0.0);
        assert_eq!(c.ticks(), 2);
        assert_eq!(c.game_time(), 40 * MS);
    }

    #[test]
    fn no_drift_over_many_uneven_frames() {
        let mut c = FixedClock::from_hz(60).with_max_steps(1000);
        let mut total = 0u64;
        let mut frame = 1u64;
        // 1 hour of 7..=23 ms frames.
        let mut elapsed = 0u64;
        while elapsed < 3_600_000 {
            let ms = 7 + (frame * 7919) % 17;
            elapsed += ms;
            total += c.advance(Duration::from_millis(ms)) as u64;
            frame += 1;
        }
        let expected = elapsed * 1_000_000 / (1_000_000_000 / 60);
        assert_eq!(total, expected);
    }

    #[test]
    fn catch_up_is_capped_and_surplus_dropped() {
        let mut c = FixedClock::new(10 * MS).with_max_steps(5);
        assert_eq!(c.advance(Duration::from_secs(1)), 5);
        assert_eq!(c.dropped(), 950 * MS);
        assert_eq!(c.alpha(), 0.0);
        assert_eq!(c.ticks(), 5);
    }

    #[test]
    fn pause_and_time_scale() {
        let mut c = FixedClock::new(10 * MS);
        c.set_paused(true);
        assert_eq!(c.advance(100 * MS), 0);
        c.set_paused(false);
        c.set_time_scale(2.0);
        assert_eq!(c.advance(20 * MS), 4);
        c.set_time_scale(0.5);
        assert_eq!(c.advance(40 * MS), 2);
        c.set_time_scale(0.0);
        assert_eq!(c.advance(Duration::from_secs(5)), 0);
    }

    #[test]
    fn fractional_scale_does_not_lose_time() {
        let mut c = FixedClock::new(10 * MS).with_max_steps(1000);
        c.set_time_scale(0.3333333333);
        let mut n = 0;
        for _ in 0..3000 {
            n += c.advance(MS);
        }
        // 3000 ms at one third speed = 1000 ms of game time = 100 ticks (carry keeps it exact).
        assert!((99..=100).contains(&n), "{n}");
    }

    #[test]
    fn frame_timer_clamps() {
        let mut t = FrameTimer::new(Duration::from_millis(100));
        std::thread::sleep(Duration::from_millis(5));
        let dt = t.tick();
        assert!(dt >= Duration::from_millis(5) && dt <= Duration::from_millis(100));
    }
}
