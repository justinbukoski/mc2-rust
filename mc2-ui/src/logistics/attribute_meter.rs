// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `AttributeMeter` (`code/attributemeter.cpp`): a row of bars showing a mech's armour, speed or
//! jump range (or a pilot's skill), with a second, lighter run for an added amount.

use crate::color::interpolate_color;
use crate::ctx::RenderCtx;
use crate::draw::GuiRect;
use crate::fit::FitExt;
use mc2_formats::ini::FitIni;

#[derive(Clone, Debug)]
pub struct AttributeMeter {
    /// `outsideRect`: the frame, in the layout's coordinates.
    pub outside: GuiRect,
    pub unit_width: i32,
    /// `skipWidth`: the gap between bars (and the frame's inset).
    pub skip: i32,
    /// `numBars`.
    pub bars: i32,
    /// `rectColor`: the frame and the empty bars.
    pub rect_color: u32,
    pub color_min: u32,
    pub color_max: u32,
    pub added_color_min: u32,
    pub added_color_max: u32,
    /// `percent` and `addedPercent`, each 0..1 of the bars.
    pub percent: f32,
    pub added_percent: f32,
    /// `bShow`.
    pub show: bool,
    /// `helpID`.
    pub help_id: i32,
}

impl Default for AttributeMeter {
    fn default() -> Self {
        AttributeMeter {
            outside: GuiRect::new(0, 0, 0, 0),
            unit_width: 0,
            skip: 0,
            bars: 0,
            rect_color: 0,
            color_min: 0xff00_5392,
            color_max: 0xffcd_eaff,
            added_color_min: 0,
            added_color_max: 0,
            percent: 0.0,
            added_percent: 0.0,
            show: true,
            help_id: 0,
        }
    }
}

impl AttributeMeter {
    /// `AttributeMeter::init`: the frame as `left/right/top/bottom` or as
    /// `XLocation/YLocation/Width/Height`, then the bar metrics and colours.
    pub fn from_fit(file: &mut FitIni, block: &str) -> AttributeMeter {
        let mut m = AttributeMeter::default();
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find block {block}");
            return m;
        }
        let (left, top, right, bottom) = match file.long("left") {
            Some(left) => (
                left,
                file.long_or("top", 0),
                file.long_or("right", 0),
                file.long_or("bottom", 0),
            ),
            None => {
                let (x, y) = (file.long_or("XLocation", 0), file.long_or("YLocation", 0));
                let (w, h) = (file.long_or("Width", 0), file.long_or("Height", 0));
                (x, y, x + w, y + h)
            }
        };
        m.outside = GuiRect::new(left as i32, top as i32, right as i32, bottom as i32);
        m.unit_width = file.long_or("UnitWidth", 0) as i32;
        m.skip = file.long_or("Skip", 0) as i32;
        m.bars = file.long_or("NumberUnits", 0) as i32;
        m.rect_color = file.color("Color").unwrap_or(0);
        m.help_id = file.long_or("HelpDesc", 0) as i32;
        if let Some(c) = file.color("ColorMin") {
            m.color_min = c;
        }
        if let Some(c) = file.color("ColorMax") {
            m.color_max = c;
        }
        m
    }

    /// `pointInside`: the frame, edges included.
    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.outside.left <= x
            && self.outside.right >= x
            && self.outside.top <= y
            && self.outside.bottom >= y
    }

    pub fn set_value(&mut self, v: f32) {
        self.percent = v.max(0.0);
    }

    pub fn set_added_value(&mut self, v: f32) {
        self.added_percent = v.max(0.0);
    }

    /// `AttributeMeter::render(xOffset, yOffset)`.
    pub fn render(&self, ctx: &mut RenderCtx<'_>, x: i32, y: i32) {
        if !self.show {
            return;
        }
        let bar_count = self.percent * self.bars as f32;
        let n_bars = bar_count as i32;
        let remainder = self.added_percent * self.bars as f32 + (bar_count - n_bars as f32);
        let n_added = remainder as i32;

        let mut frame = self.outside;
        frame.left += x;
        frame.right += x;
        frame.top += y;
        frame.bottom += y;
        ctx.dl.outline_rect(frame, self.rect_color, self.rect_color);

        let mut r = GuiRect::new(
            self.outside.left + 2 * self.skip + x,
            self.outside.top + 2 * self.skip + y,
            0,
            self.outside.bottom - 2 * self.skip + y,
        );
        r.right = r.left + self.unit_width;
        let mut color = self.color_min;
        r.bottom += self.skip;
        let step = self.unit_width + self.skip;
        for i in 0..n_bars {
            ctx.dl.fill_rect(r, color);
            color = interpolate_color(self.color_min, self.color_max, i as f32 / self.bars as f32);
            r.left += step;
            r.right = r.left + self.unit_width;
        }
        for i in 0..n_added {
            ctx.dl.fill_rect(r, color);
            color = interpolate_color(
                self.added_color_min,
                self.added_color_max,
                i as f32 / self.bars as f32,
            );
            r.left += step;
            r.right = r.left + self.unit_width;
        }
        r.bottom -= self.skip;
        r.right -= 1;
        for _ in (n_bars + n_added)..self.bars {
            ctx.dl.outline_rect(r, self.rect_color, self.rect_color);
            r.left += step;
            r.right = r.left + self.unit_width - 1;
        }
    }
}
