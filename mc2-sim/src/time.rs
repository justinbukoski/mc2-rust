// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Time model: `turn`, `frameLength`, `scenarioTime` and how they advance.
//!
//! The original is *not* fixed-step. `DoGameLogic` sets `frameLength = 1/frameRate` (frameRate measured by GameOS, capped
//! so `frameLength <= 0.25`), and `Mission::update` independently adds the measured wall-clock milliseconds between two
//! calls (`timeGetTime()` deltas) to `scenarioTime`. Both then drive the simulation (movement uses `frameLength`, timers
//! compare against `scenarioTime`). A reproducible port needs both numbers to be inputs: [`FrameClock`] reproduces the
//! arithmetic exactly and [`FixedStep`] produces the inputs for a fixed rate (the same rounding the oracle driver uses),
//! so recorded runs can be compared bit for bit.
//!
//! Provenance: `code/mechcmd2.cpp` `DoGameLogic` (lines ~2060-2075 frame length, 2112 `timerManager->update()`),
//! `code/mission.cpp` `Mission::update` (lines ~318-440), `mclib/timing.cpp` and `timing.h` (initial values, `Timer`).
//!
//! There is ONE clock: `World::clock`. (`MissionState` does not own a copy.)

/// `Stuff::SMALL` (`mclib/stuff/style.cpp`).
pub const STUFF_SMALL: f32 = 1e-4;

/// `frameLength = 1 / frameRate`, capped at 0.25 s; `frameRate < SMALL` is replaced by 4 first
/// (`DoGameLogic`). The division is done in double and rounded to float, like the C++ (`1.0 / frameRate`).
pub fn frame_length_from_rate(frame_rate: f32) -> f32 {
    let rate = if frame_rate < STUFF_SMALL {
        4.0
    } else {
        frame_rate
    };
    let len = (1.0f64 / rate as f64) as f32;
    if len > 0.25 {
        0.25
    } else {
        len
    }
}

/// The three global counters (`turn`, `frameLength`, `scenarioTime`) plus the millisecond baseline
/// (`LastTimeGetTime`).
#[derive(Clone, Debug)]
pub struct FrameClock {
    /// Frames since mission start (`turn`); incremented at the top of `Mission::update`.
    pub turn: i64,
    /// Seconds the current frame represents (`frameLength`).
    pub frame_length: f32,
    /// Seconds of unpaused play (`scenarioTime`).
    pub scenario_time: f32,
    /// `LastTimeGetTime`; `None` is the C++ sentinel `0xffffffff` ("first frame: just record the time").
    last_ms: Option<u32>,
}

impl Default for FrameClock {
    fn default() -> Self {
        // mclib/timing.cpp initial values: turn = 0, frameLength = 0.05, scenarioTime = 0.
        FrameClock {
            turn: 0,
            frame_length: 0.05,
            scenario_time: 0.0,
            last_ms: None,
        }
    }
}

impl FrameClock {
    /// `timeGetTime()` of the frame being run (the `now_ms` of the last [`Self::begin_frame`]); `None` before the first frame.
    pub fn now_ms(&self) -> Option<u32> {
        self.last_ms
    }

    /// `Mission::init` resets: `turn = 0`, `scenarioTime = 0`, baseline cleared.
    pub fn reset(&mut self) {
        self.turn = 0;
        self.scenario_time = 0.0;
        self.last_ms = None;
    }

    /// Start of a frame. `frame_rate` is what GameOS measured (set `frameLength`), `now_ms` is `timeGetTime()`.
    /// While `paused` the baseline follows the clock but `scenarioTime` does not advance.
    pub fn begin_frame(&mut self, frame_rate: f32, now_ms: u32, paused: bool) {
        self.frame_length = frame_length_from_rate(frame_rate);
        self.turn += 1;
        if !paused {
            if let Some(last) = self.last_ms {
                let milliseconds = now_ms.wrapping_sub(last) as f32;
                self.scenario_time += milliseconds / 1000.0f32;
            }
        }
        self.last_ms = Some(now_ms);
    }
}

/// `Timer` (`mclib/timing.h`): a countdown that `TimerManager::update` ticks once per frame by `frameLength`. Script timers
/// (`settimer`/`checktimer`) are these; they keep ticking while the mission is paused because `gamePaused` is never set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timer {
    start_time: f32,
    current_time: f32,
    expired: bool,
    update_while_paused: bool,
}

impl Default for Timer {
    fn default() -> Self {
        Timer {
            start_time: 0.0,
            current_time: 0.0,
            expired: true,
            update_while_paused: false,
        }
    }
}

impl Timer {
    /// `Timer::update`: `currentTime -= frameLength`. The C++ guard `updateWhilePaused || !gamePaused` always passes: the
    /// *global* `gamePaused` (mclib/timing.h) is only ever read (`light.cpp`, `timing.h`); `SoundSystem::setIsPaused()` assigns
    /// the identically named *member* `SoundSystem::gamePaused` (soundsys.h:152, 294), which shadows it. So timers tick while the
    /// mission is paused.
    pub fn update(&mut self, frame_length: f32) {
        self.current_time -= frame_length;
        self.expired = self.current_time < 0.0;
    }
    /// `setTimer(newTime)`.
    pub fn set(&mut self, new_time: f32) {
        self.start_time = new_time;
        self.current_time = new_time;
        self.expired = new_time > 0.0;
    }
    /// `isExpired`.
    pub fn is_expired(&self) -> bool {
        self.expired
    }
    /// `getCurrentTime`.
    pub fn current(&self) -> f32 {
        self.current_time
    }
    /// `setUpdateWhilePaused`.
    pub fn set_update_while_paused(&mut self, on: bool) {
        self.update_while_paused = on;
    }
}

/// `MAX_TIMERS`.
pub const MAX_TIMERS: usize = 32;

/// `TimerManager`: 32 timers ticked by `DoGameLogic` before `Mission::update` (mechcmd2.cpp:2112).
#[derive(Clone, Debug, Default)]
pub struct TimerManager {
    /// The timers.
    pub timers: [Timer; MAX_TIMERS],
}

impl TimerManager {
    /// `TimerManager::update`.
    pub fn update(&mut self, frame_length: f32) {
        for t in &mut self.timers {
            t.update(frame_length);
        }
    }
    /// `getTimer(index)`; `None` outside `0..32`.
    pub fn get(&mut self, index: i32) -> Option<&mut Timer> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.timers.get_mut(i))
    }
}

/// Produces `timeGetTime()` values for a fixed frame rate: frame `f` happens at `BASE_MS + round(f * 1000 / fps)`.
/// This is the clock `oracle_main.cpp` uses.
#[derive(Clone, Copy, Debug)]
pub struct FixedStep {
    /// Frames per second (`frameRate`).
    pub fps: u32,
}

/// Arbitrary non-zero start of the virtual clock (only differences matter).
pub const BASE_MS: u32 = 100_000;

impl FixedStep {
    /// Virtual `timeGetTime()` at frame `f` (1-based; frame 0 is the state right after mission load).
    pub fn now_ms(&self, f: u64) -> u32 {
        BASE_MS + ((f as f64) * 1000.0 / self.fps as f64 + 0.5) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_oracle_first_frames() {
        // oracle digest, 30 fps: F1 scenarioTime 0, F2 0.034, F3 0.067 (see oracle/DIGEST.md)
        let step = FixedStep { fps: 30 };
        let mut c = FrameClock::default();
        let mut got = vec![];
        for f in 1..=3 {
            c.begin_frame(30.0, step.now_ms(f), false);
            got.push(c.scenario_time);
        }
        assert_eq!(got[0], 0.0);
        assert!((got[1] - 0.034).abs() < 1e-7);
        assert!((got[2] - 0.067).abs() < 1e-7);
        assert_eq!(c.turn, 3);
        assert!((c.frame_length - 0.033333335).abs() < 1e-9);
    }

    #[test]
    fn pause_freezes_scenario_time_but_follows_clock() {
        let mut c = FrameClock::default();
        c.begin_frame(30.0, 1000, false);
        c.begin_frame(30.0, 1033, true);
        assert_eq!(c.scenario_time, 0.0);
        c.begin_frame(30.0, 1066, false);
        assert!((c.scenario_time - 0.033).abs() < 1e-7);
    }

    #[test]
    fn timers_tick_by_frame_length() {
        let mut m = TimerManager::default();
        assert!(m.timers[3].is_expired());
        m.get(3).unwrap().set(0.1);
        assert!(m.timers[3].is_expired());
        m.update(1.0 / 30.0);
        assert!(!m.timers[3].is_expired());
        m.update(1.0 / 30.0);
        m.update(1.0 / 30.0);
        m.update(1.0 / 30.0);
        assert!(m.timers[3].is_expired());
        assert!(m.get(32).is_none() && m.get(-1).is_none());
    }

    #[test]
    fn frame_length_cap() {
        assert_eq!(frame_length_from_rate(2.0), 0.25);
        assert_eq!(frame_length_from_rate(0.0), 0.25);
        assert_eq!(frame_length_from_rate(30.0), (1.0f64 / 30.0) as f32);
    }
}

// ---- save/load (DESIGN.md section 9): the clock and the script timers are dynamic state ----

crate::snapshot_struct!(FrameClock {
    turn,
    frame_length,
    scenario_time,
    last_ms,
});

crate::snapshot_struct!(Timer {
    start_time,
    current_time,
    expired,
    update_while_paused,
});

crate::snapshot_struct!(TimerManager { timers });
