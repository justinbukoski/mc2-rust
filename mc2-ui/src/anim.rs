// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aAnimation` / `aAnimGroup` (`gui/aanim.cpp`): key-framed position, scale and colour tracks.

use crate::color::interpolate_color;
use crate::fit::FitExt;
use mc2_formats::ini::FitIni;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveInfo {
    /// Seconds.
    pub time: f32,
    pub position_x: f32,
    pub position_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub color: u32,
}

/// A key-framed animation. `current_time == -1` means "not animating".
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub current_time: f32,
    pub infos: Vec<MoveInfo>,
    pub ref_x: f32,
    pub ref_y: f32,
    pub direction: f32,
    pub loops: bool,
}

impl Default for Animation {
    fn default() -> Self {
        Animation {
            current_time: -1.0,
            infos: Vec::new(),
            ref_x: 0.0,
            ref_y: 0.0,
            direction: 1.0,
            loops: false,
        }
    }
}

impl Animation {
    /// `aAnimation::init(file, prefix)`: reads `<prefix>AnimationTimeStamps` (or
    /// `<prefix>NumTimeStamps`), `<prefix>AnimationLoops` and per key `Time`, `Color`
    /// (default white), `Pos<i>X/Y`, `Scale` or `ScaleX`/`ScaleY` (default 1) from the current block.
    pub fn from_fit(file: &FitIni, prefix: &str) -> Animation {
        let mut a = Animation::default();
        let count = file
            .long(&format!("{prefix}AnimationTimeStamps"))
            .or_else(|| file.long(&format!("{prefix}NumTimeStamps")))
            .unwrap_or(0)
            .max(0) as usize;
        if let Some(l) = file.boolean(&format!("{prefix}AnimationLoops")) {
            a.loops = l;
        }
        for i in 0..count {
            let time = file.float(&format!("{prefix}Time{i}")).unwrap_or(0.0);
            let color = file
                .color(&format!("{prefix}Color{i}"))
                .unwrap_or(0xffff_ffff);
            let position_x = file.long(&format!("{prefix}Pos{i}X")).unwrap_or(0) as f32;
            let position_y = file.long(&format!("{prefix}Pos{i}Y")).unwrap_or(0) as f32;
            let (scale_x, scale_y) = match file.float(&format!("{prefix}Scale{i}")) {
                Some(s) => (s, s),
                None => match file.float(&format!("{prefix}ScaleX{i}")) {
                    Some(sx) => (sx, file.float(&format!("{prefix}ScaleY{i}")).unwrap_or(1.0)),
                    None => (1.0, 1.0),
                },
            };
            a.infos.push(MoveInfo {
                time,
                position_x,
                position_y,
                scale_x,
                scale_y,
                color,
            });
        }
        a
    }

    /// `initWithBlockName`: seeks `block` and reads it with an empty prefix. A missing block
    /// gives an empty animation.
    pub fn from_block(file: &mut FitIni, block: &str) -> Animation {
        if file.seek_block(block).is_err() {
            log::warn!("animation block {block} not found");
            return Animation::default();
        }
        Animation::from_fit(file, "")
    }

    pub fn destroy(&mut self) {
        *self = Animation {
            ref_x: self.ref_x,
            ref_y: self.ref_y,
            ..Animation::default()
        };
    }

    pub fn begin(&mut self) {
        self.current_time = 0.0;
        self.direction = 1.0;
    }

    pub fn reverse_begin(&mut self) {
        self.current_time = self.infos.last().map_or(0.0, |l| l.time);
        self.direction = -1.0;
    }

    pub fn end(&mut self) {
        self.current_time = -1.0;
    }

    pub fn is_animating(&self) -> bool {
        self.current_time != -1.0
    }

    fn last_time(&self) -> Option<f32> {
        self.infos.last().map(|l| l.time)
    }

    /// Advances by `dt` seconds (`frameLength`).
    pub fn update(&mut self, dt: f32) {
        if self.current_time != -1.0 {
            self.current_time += self.direction * dt;
        }
        if self.direction < 0.0 && self.loops && !self.infos.is_empty() {
            if self.current_time < 0.0 {
                self.current_time += self.last_time().unwrap_or(0.0);
            }
        } else if let Some(last) = self.last_time() {
            if self.current_time > last && self.loops {
                self.current_time -= last;
            }
        }
    }

    pub fn is_done(&self) -> bool {
        if self.loops {
            return false;
        }
        if self.direction < 0.0 {
            return self.current_time < 0.0;
        }
        matches!(self.last_time(), Some(last) if self.current_time > last)
    }

    pub fn max_time(&self) -> f32 {
        self.last_time().unwrap_or(-1.0)
    }

    /// The key segment containing `time`: `(t0, t1, i)` with `infos[i].time <= time <
    /// infos[i+1].time`. As in the C++, a segment whose end time is 0 counts as "past the end".
    fn segment(&self, time: f32) -> Option<(f32, f32, usize)> {
        for j in 0..self.infos.len().saturating_sub(1) {
            if self.infos[j].time <= time && self.infos[j + 1].time > time {
                let t1 = self.infos[j + 1].time;
                return (t1 != 0.0).then_some((self.infos[j].time, t1, j));
            }
        }
        None
    }

    fn track(&self, get: impl Fn(&MoveInfo) -> f32, default: f32) -> f32 {
        match self.segment(self.current_time) {
            Some((t0, t1, j)) => {
                let (p0, p1) = (get(&self.infos[j]), get(&self.infos[j + 1]));
                p0 + (self.current_time - t0) * ((p1 - p0) / (t1 - t0))
            }
            None => self.infos.last().map_or(default, get),
        }
    }

    pub fn x_delta(&self) -> f32 {
        if self.infos.is_empty() {
            return 0.0;
        }
        self.track(|m| m.position_x, 0.0) - self.ref_x
    }

    pub fn y_delta(&self) -> f32 {
        if self.infos.is_empty() {
            return 0.0;
        }
        self.track(|m| m.position_y, 0.0) - self.ref_y
    }

    pub fn scale_x(&self) -> f32 {
        self.track(|m| m.scale_x, 1.0)
    }

    pub fn scale_y(&self) -> f32 {
        self.track(|m| m.scale_y, 1.0)
    }

    pub fn color(&self) -> u32 {
        self.color_at(self.current_time)
    }

    pub fn color_at(&self, mut time: f32) -> u32 {
        if let Some(last) = self.last_time() {
            if time > last && self.loops {
                let cycles = if last != 0.0 { time / last } else { 0.0 };
                time -= (cycles as i32) as f32 * last;
            }
        }
        match self.segment(time) {
            Some((t0, t1, j)) => interpolate_color(
                self.infos[j].color,
                self.infos[j + 1].color,
                (time - t0) / (t1 - t0),
            ),
            None => self.infos.last().map_or(0xffff_ffff, |l| l.color),
        }
    }

    pub fn set_reference_points(&mut self, x: f32, y: f32) {
        self.ref_x = x;
        self.ref_y = y;
    }
}

/// `aAnimGroup::STATE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AnimState {
    #[default]
    Normal = 0,
    Pressed = 1,
    Highlight = 2,
    PressedHighlight = 3,
    Disabled = 4,
}

impl AnimState {
    pub fn from_index(i: usize) -> AnimState {
        match i {
            1 => AnimState::Pressed,
            2 => AnimState::Highlight,
            3 => AnimState::PressedHighlight,
            4 => AnimState::Disabled,
            _ => AnimState::Normal,
        }
    }
}

/// `aAnimGroup`: one animation per state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimGroup {
    pub animations: [Animation; 5],
    pub cur_state: AnimState,
}

impl AnimGroup {
    /// `aAnimGroup::init`: block `block` with `Normal`, `Highlight`, `Pressed`,
    /// `HighlightPressed` and `Disabled` prefixes.
    pub fn from_block(file: &mut FitIni, block: &str) -> AnimGroup {
        let mut g = AnimGroup::default();
        if file.seek_block(block).is_err() {
            log::warn!("animation group block {block} not found");
            return g;
        }
        g.animations[AnimState::Normal as usize] = Animation::from_fit(file, "Normal");
        g.animations[AnimState::Highlight as usize] = Animation::from_fit(file, "Highlight");
        g.animations[AnimState::Pressed as usize] = Animation::from_fit(file, "Pressed");
        g.animations[AnimState::PressedHighlight as usize] =
            Animation::from_fit(file, "HighlightPressed");
        g.animations[AnimState::Disabled as usize] = Animation::from_fit(file, "Disabled");
        g
    }

    pub fn set_state(&mut self, s: AnimState) {
        if s != self.cur_state {
            self.cur_state = s;
            if s != AnimState::Normal {
                self.animations[s as usize].begin();
            }
        }
    }

    pub fn state(&self) -> AnimState {
        self.cur_state
    }

    pub fn current_color(&self, s: AnimState) -> u32 {
        self.animations[s as usize].color()
    }

    pub fn update(&mut self, dt: f32) {
        self.animations[AnimState::Normal as usize].update(dt);
        let s = self.cur_state as usize;
        self.animations[s].update(dt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        let mut f = FitIni::from_bytes(format!("FITini\n{text}\nFITend\n").into_bytes()).unwrap();
        f.seek_block("A").unwrap();
        f
    }

    #[test]
    fn reads_keys_and_interpolates() {
        let f = fit("[A]\nl AnimationTimeStamps = 3\nb AnimationLoops = FALSE\nf Time0 = 0\nl Pos0X = 0\nl Pos0Y = -100\nl Color0 = 0x00000000\nf Time1 = 1.0\nl Pos1X = 10\nl Pos1Y = 0\nl Color1 = 0xff000000\nf Time2 = 2.0\nl Pos2X = 10\nl Pos2Y = 50\nf Scale2 = 2.0\n[B]\n");
        let mut a = Animation::from_fit(&f, "");
        assert_eq!(a.infos.len(), 3);
        assert_eq!(a.infos[2].color, 0xffff_ffff);
        assert_eq!(a.infos[2].scale_y, 2.0);
        assert!(!a.is_animating());
        a.begin();
        a.update(0.5);
        assert_eq!(a.x_delta(), 5.0);
        assert_eq!(a.y_delta(), -50.0);
        assert_eq!(a.color(), 0x7f00_0000);
        a.update(1.0);
        assert_eq!(a.y_delta(), 25.0);
        assert!(!a.is_done());
        a.update(1.0);
        assert!(a.is_done());
        // past the end: last key
        assert_eq!(a.y_delta(), 50.0);
        assert_eq!(a.scale_x(), 2.0);
    }

    #[test]
    fn looping_wraps_time() {
        let f = fit("[A]\nl AnimationTimeStamps = 2\nb AnimationLoops = TRUE\nf Time0 = 0\nl Color0 = 0xff000000\nf Time1 = 2.0\nl Color1 = 0xff0000ff\n");
        let mut a = Animation::from_fit(&f, "");
        a.begin();
        a.update(2.5);
        assert!((a.current_time - 0.5).abs() < 1e-6);
        assert!(!a.is_done());
        assert_eq!(a.color_at(4.5), a.color_at(0.5));
        a.reverse_begin();
        a.update(2.5);
        assert!((a.current_time - 1.5).abs() < 1e-6);
    }
}
