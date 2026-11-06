// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The building hit / ownership flash (`BldgAppearance::flashBuilding`, `mclib/bdactor.cpp:2397-2404`).
//!
//! `flashBuilding(duration, flashDuration, color)` starts a blink: for `duration` seconds the appearance toggles
//! `drawFlash` every `flashDuration` seconds, and while `drawFlash` is set the shape is drawn with `SetARGBHighLight(color)`
//! (`bdactor.cpp:1528-1531`), which `TG_Shape` adds to every vertex colour and clamps at 255 (`tgl.cpp:2207-2227`).
//! [`BuildingFlash::advance`] is the per-frame block of `BldgAppearance::update` (`bdactor.cpp:1936-1951`), which runs
//! whether or not the building is in view.

/// `flashBuilding(5.0, 0.5, ...)`: how long a building blinks after a hit or an ownership change
/// (`bldng.cpp:957`, `turret.cpp:389`, `gate.cpp:577`).
pub const STRUCTURE_FLASH_DURATION: f32 = 5.0;
/// The second argument of those calls: the length of each lit / unlit half-period.
pub const STRUCTURE_FLASH_PERIOD: f32 = 0.5;

/// The flash state of one appearance (`duration`, `flashDuration`, `currentFlash`, `flashColor`, `drawFlash`).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct BuildingFlash {
    duration: f32,
    flash_duration: f32,
    current_flash: f32,
    color: u32,
    draw_flash: bool,
}

impl BuildingFlash {
    /// `flashBuilding(dur, fDuration, color)`: the blink starts lit.
    pub fn start(&mut self, duration: f32, flash_duration: f32, color: u32) {
        self.duration = duration;
        self.flash_duration = flash_duration;
        self.color = color;
        self.draw_flash = true;
        self.current_flash = flash_duration;
    }

    /// The structure flash the game raises on a hit or capture (`flashBuilding(5.0, 0.5, color)`).
    pub fn start_structure(&mut self, color: u32) {
        self.start(STRUCTURE_FLASH_DURATION, STRUCTURE_FLASH_PERIOD, color);
    }

    /// The per-frame update (`bdactor.cpp:1936-1951`).
    pub fn advance(&mut self, frame_length: f32) {
        if self.duration > 0.0 {
            self.duration -= frame_length;
            self.current_flash -= frame_length;
            if self.current_flash < 0.0 {
                self.draw_flash ^= true;
                self.current_flash = self.flash_duration;
            }
        } else {
            self.draw_flash = false;
        }
    }

    /// The ARGB highlight the shape is drawn with this frame (`SetARGBHighLight(flashColor)` when `drawFlash`).
    pub fn tint(&self) -> Option<u32> {
        self.draw_flash.then_some(self.color)
    }

    /// Whether the state can still change (a finished flash that has been cleared needs no more updates).
    pub fn is_active(&self) -> bool {
        self.duration > 0.0 || self.draw_flash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 30.0;

    /// The tint over time for the shipped call `flashBuilding(5.0, 0.5, 0x007f0000)`, stepped like the C++ update.
    #[test]
    fn structure_flash_blinks_every_half_second_for_five_seconds() {
        let mut f = BuildingFlash::default();
        assert_eq!(f.tint(), None);
        f.start_structure(0x007f_0000);
        assert_eq!(f.tint(), Some(0x007f_0000), "lit at once");
        let mut lit_frames = 0;
        let mut toggles = 0;
        let mut last = true;
        let mut frames = 0;
        while f.is_active() && frames < 1000 {
            f.advance(DT);
            frames += 1;
            let now = f.tint().is_some();
            if now != last {
                toggles += 1;
                last = now;
            }
            lit_frames += now as u32;
        }
        // 5.0 s of duration at 1/30 s per frame, then the first update with duration <= 0 clears the flash
        assert!((149..=152).contains(&frames), "{frames} frames");
        assert_eq!(f.tint(), None);
        // a toggle per 0.5 s (+ one frame, `currentFlash < 0`): 5 s gives about 9-10 toggles
        assert!((8..=10).contains(&toggles), "{toggles} toggles");
        assert!(lit_frames > 0 && lit_frames < frames);
    }

    #[test]
    fn first_toggle_is_after_the_flash_period() {
        let mut f = BuildingFlash::default();
        f.start(5.0, 0.5, 0x0000_007f);
        let mut t = 0.0;
        while f.tint().is_some() {
            f.advance(DT);
            t += DT;
        }
        // lit for flashDuration, off at the first frame `currentFlash` goes below zero
        assert!((0.5..0.5 + 2.0 * DT).contains(&t), "{t}");
        // and lit again one period later
        while f.tint().is_none() {
            f.advance(DT);
            t += DT;
        }
        assert!((1.0..1.0 + 4.0 * DT).contains(&t), "{t}");
    }

    #[test]
    fn restarting_resets_the_blink() {
        let mut f = BuildingFlash::default();
        f.start_structure(0x007f_0000);
        for _ in 0..40 {
            f.advance(DT);
        }
        f.start_structure(0x0000_7f00);
        assert_eq!(f.tint(), Some(0x0000_7f00));
    }
}
