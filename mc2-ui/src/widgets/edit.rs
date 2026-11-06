// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aEdit` (`gui/aedit.cpp`): a one-line text entry field with selection, scrolling and a
//! blinking caret. Text is held as code-page 1252 bytes, so positions are byte offsets as in the
//! original (1252 has no lead bytes, so every `_mbsinc` step is one byte).
//!
//! The IME calls (`gos_EnableIME`, `gos_PositionIME`, appearance setup) have no counterpart here.
//! The original processes every queued key per update; alariq's build processes one (a platform
//! change), this port processes all of them.

use super::basic::{font_render, font_width};
use super::object::Object;
use crate::cp1252;
use crate::ctx::{sfx, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::font::FontRef;
use mc2_formats::ini::FitIni;
use mc2_platform::input::KeyStatus;
use mc2_platform::keys;

pub const ES_EDITALPHA: u32 = 0x0002_0000;
pub const ES_EDITNUM: u32 = 0x0004_0000;
pub const ES_EDITALPHANUM: u32 = 0x0008_0000;
pub const ES_EDITUPPER: u32 = 0x0010_0000;
pub const ES_EDITREADONLY: u32 = 0x0020_0000;
pub const ES_EDITNOBLANK: u32 = 0x0040_0000;

const ENTRY_MAX_CHARS: usize = 1000;
const ENTRY_MARGIN: f32 = 4.0;

#[derive(Clone, Debug)]
pub struct Edit {
    pub obj: Object,
    pub cursor_color: u32,
    pub highlight_color: u32,
    pub text_color: u32,
    pub selected_color: u32,
    pub outline_color: u32,
    pub limit: usize,
    pub ins1: usize,
    pub ins2: usize,
    pub cursor_visible: bool,
    pub left_offset: i32,
    pub cursor_time: f32,
    pub mouse_down: bool,
    /// Code-page 1252 bytes.
    pub text: Vec<u8>,
    pub font: FontRef,
    pub focus: bool,
    pub style: u32,
    pub weird_chars: bool,
}

impl Default for Edit {
    fn default() -> Self {
        let mut obj = Object::default();
        obj.set_color(0xff00_0000, false);
        Edit {
            obj,
            cursor_color: 0,
            highlight_color: 0,
            text_color: 0,
            selected_color: 0,
            outline_color: 0,
            limit: ENTRY_MAX_CHARS,
            ins1: 0,
            ins2: 0,
            cursor_visible: false,
            left_offset: 0,
            cursor_time: 0.0,
            mouse_down: false,
            text: Vec::new(),
            font: FontRef::default(),
            focus: false,
            style: 0,
            weird_chars: true,
        }
    }
}

impl Edit {
    pub fn new(x: f32, y: f32, w: f32, h: f32, font: FontRef) -> Edit {
        Edit {
            obj: Object::new(x, y, w, h),
            font,
            ..Edit::default()
        }
    }

    /// `aEdit::init`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> Edit {
        let mut e = Edit::default();
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find the text block {block}");
            return e;
        }
        e.obj = Object::new(
            file.long_or("XLocation", 0) as f32,
            file.long_or("YLocation", 0) as f32,
            file.long_or("Width", 0) as f32,
            file.long_or("Height", 0) as f32,
        );
        e.text_color = file.color("Color").unwrap_or(0);
        e.font = ld.font(file.long_or("Font", 0) as u32);
        e.cursor_color = file.color("CursorColor").unwrap_or(0);
        e.highlight_color = file.color("HighlightColor").unwrap_or(0);
        e.selected_color = file.color("SelectedColor").unwrap_or(0);
        e.obj.help_header = file.long_or("HelpCaption", 0) as i32;
        e.obj.help_id = file.long_or("HelpDesc", 0) as i32;
        e
    }

    /// `getEntry`.
    pub fn entry(&self) -> String {
        cp1252::decode(&self.text)
    }

    /// `setEntry(str, byHighlight)`.
    pub fn set_entry(&mut self, assets: &UiAssets, s: &str, highlight: bool) {
        self.text = cp1252::encode(s);
        if highlight {
            self.ins2 = self.text.len();
            self.ins1 = 0;
        } else {
            self.ins1 = self.text.len();
            self.ins2 = self.ins1;
            self.flush_cursor_right(assets);
        }
    }

    pub fn has_focus(&self) -> bool {
        self.focus
    }

    /// `setFocus`; the keyboard queue is flushed as `gos_KeyboardFlush` does.
    pub fn set_focus(&mut self, ctx: &mut UpdateCtx<'_>, focus: bool) {
        self.focus = focus;
        ctx.input.flush_keys();
        if focus {
            self.cursor_visible = true;
        }
    }

    pub fn set_read_only(&mut self, ro: bool) {
        if ro {
            self.style |= ES_EDITREADONLY;
        } else {
            self.style ^= ES_EDITREADONLY;
        }
    }
    pub fn set_numeric(&mut self, n: bool) {
        if n {
            self.style |= ES_EDITNUM;
        } else {
            self.style ^= ES_EDITNUM;
        }
    }
    pub fn set_no_blank(&mut self, n: bool) {
        if n {
            self.style |= ES_EDITNOBLANK;
        } else {
            self.style ^= ES_EDITNOBLANK;
        }
    }

    fn width_of(&self, assets: &UiAssets, bytes: &[u8]) -> i32 {
        font_width(assets, self.font, &cp1252::decode(bytes))
    }

    /// `charXPos`: pixel offset of byte `i`.
    pub fn char_x_pos(&self, assets: &UiAssets, i: usize) -> i32 {
        if i > self.text.len() || self.text.is_empty() {
            return 0;
        }
        self.width_of(assets, &self.text[..i])
    }

    fn char_length(&self, i: usize) -> usize {
        // `isleadbyte` is never true in code page 1252
        if i > self.text.len() {
            0
        } else {
            1
        }
    }

    /// `findChar`: the byte position nearest to pixel `x`.
    pub fn find_char(&self, assets: &UiAssets, x: i32) -> usize {
        let (mut last, mut next) = (0, 0);
        let (mut cur, mut pnext) = (0usize, 0usize);
        if !self.text.is_empty() {
            loop {
                pnext = cur + 1;
                last = next;
                next = self.width_of(assets, &self.text[..pnext.min(self.text.len())]);
                if x > next {
                    cur = pnext;
                }
                if !(cur < self.text.len() && x > next) {
                    break;
                }
            }
        }
        let n = if x - last < next - x { cur } else { pnext };
        n.min(self.text.len())
    }

    fn make_cursor_visible(&mut self, assets: &UiAssets) {
        let x = self.char_x_pos(assets, self.ins2);
        if x < self.left_offset {
            self.left_offset = x;
        }
        let w = self.obj.width() - ENTRY_MARGIN * 2.0;
        if x as f32 > self.left_offset as f32 + w {
            self.left_offset = x - w as i32;
        }
    }

    fn flush_cursor_right(&mut self, assets: &UiAssets) {
        let x = self.char_x_pos(assets, self.ins2);
        let w = self.obj.width() - ENTRY_MARGIN * 2.0;
        if (x as f32) < w {
            self.left_offset = 0;
        } else {
            self.left_offset = x - w as i32;
        }
    }

    fn clear_selection(&mut self, assets: &UiAssets) -> bool {
        if self.ins1 == self.ins2 {
            return false;
        }
        let lo = self.ins1.min(self.ins2);
        let hi = self.ins1.max(self.ins2).min(self.text.len());
        self.text.drain(lo..hi);
        self.ins1 = lo;
        self.ins2 = lo;
        self.flush_cursor_right(assets);
        true
    }

    fn back_space(&mut self, pos: usize) {
        if pos > self.text.len() || pos == 0 {
            return;
        }
        self.text.remove(pos - 1);
        self.ins1 = pos - 1;
        self.ins2 = pos - 1;
    }

    /// `aEdit::update`: caret blink, mouse, keyboard.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.cursor_time += ctx.dt;
        if self.cursor_time > 0.5 {
            self.cursor_visible = !self.cursor_visible;
            if !self.focus {
                self.focus = false;
                self.cursor_visible = false;
            }
            self.cursor_time = 0.0;
        }
        self.handle_mouse(ctx);
        self.handle_keyboard(ctx);
    }

    fn handle_mouse(&mut self, ctx: &mut UpdateCtx<'_>) {
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let click = ctx.input.is_left_click();
        let offset = mx - (self.obj.global_x() + ENTRY_MARGIN as i32) + self.left_offset;
        let n = self.find_char(ctx.assets, offset);
        if self.mouse_down && ctx.input.is_left_drag() && self.focus {
            self.ins2 = n;
        }
        if self.obj.point_inside(mx, my) {
            if click && self.style & ES_EDITREADONLY == 0 {
                self.set_focus(ctx, true);
                self.mouse_down = true;
                self.cursor_visible = true;
                self.ins1 = n;
                if ctx.input.key_status(keys::KEY_LSHIFT) == KeyStatus::Free {
                    self.ins2 = self.ins1;
                }
            }
        } else if click {
            self.set_focus(ctx, false);
        }
    }

    fn handle_keyboard(&mut self, ctx: &mut UpdateCtx<'_>) {
        if !self.focus {
            return;
        }
        loop {
            let key = ctx.input.pop_key();
            if key == 0 {
                return;
            }
            if !self.handle_formatting_keys(ctx, key) {
                // the original's "eat control keys" test
                if (key & 0xff) + ((key & 0xff) << 8) + 0x4000 == key && key != 0x6e2e {
                    continue;
                }
                let ch = (key & 0xff) as u8;
                // control characters have no glyph (a platform difference: the original's
                // keyboard layer never queued them)
                if ch < 0x20 {
                    continue;
                }
                let c = ch as char;
                if self.style & ES_EDITNUM != 0 {
                    if !c.is_ascii_digit() && c != ' ' && c != '.' {
                        continue;
                    }
                } else if self.style & ES_EDITALPHA != 0 {
                    if !c.is_ascii_alphabetic() && c != ' ' {
                        continue;
                    }
                } else if self.style & ES_EDITALPHANUM != 0
                    && !c.is_ascii_alphanumeric()
                    && c != ' '
                {
                    continue;
                }
                if self.style & ES_EDITNOBLANK != 0 && c == ' ' {
                    continue;
                }
                if ctx.input.key_status(keys::KEY_LMENU) != KeyStatus::Free {
                    continue;
                }
                if matches!(c, '%' | '\\' | '/' | '"') {
                    continue;
                }
                if !self.weird_chars
                    && matches!(
                        c,
                        '\\' | ':' | '*' | '\'' | '.' | ',' | '/' | '<' | '>' | '?' | '"' | '|'
                    )
                {
                    continue;
                }
                let sel = self.ins2.abs_diff(self.ins1);
                if self.text.len() - sel.min(self.text.len()) >= self.limit {
                    ctx.play(sfx::LOG_WRONGBUTTON);
                    ctx.input.flush_keys();
                    return;
                }
                self.clear_selection(ctx.assets);
                let at = self.ins1.min(self.text.len());
                let ch = if self.style & ES_EDITUPPER != 0 {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                };
                self.text.insert(at, ch);
                self.ins1 = at + 1;
                self.ins2 = self.ins1;
            }
            self.make_cursor_visible(ctx.assets);
        }
    }

    fn handle_formatting_keys(&mut self, ctx: &mut UpdateCtx<'_>, keycode: u32) -> bool {
        let key = (keycode & 0xff00) >> 8;
        let shift = ctx.input.key_status(keys::KEY_LSHIFT) != KeyStatus::Free;
        let assets = ctx.assets;
        match key {
            keys::KEY_LCONTROL | keys::KEY_TAB | keys::KEY_UP | keys::KEY_DOWN => true,
            keys::KEY_RETURN => {
                if ctx.input.key_status(keys::KEY_RETURN) == KeyStatus::Pressed {
                    self.set_focus(ctx, false);
                }
                true
            }
            keys::KEY_HOME => {
                if shift {
                    self.ins2 = 0;
                } else {
                    self.ins1 = 0;
                    self.ins2 = 0;
                }
                self.left_offset = 0;
                self.cursor_visible = true;
                true
            }
            keys::KEY_END => {
                if shift {
                    self.ins2 = self.text.len();
                } else {
                    self.ins1 = self.text.len();
                    self.ins2 = self.ins1;
                }
                self.make_cursor_visible(assets);
                self.cursor_visible = true;
                true
            }
            keys::KEY_LEFT => {
                if shift {
                    self.ins2 = self.ins2.saturating_sub(1);
                } else {
                    if self.ins1 != self.ins2 {
                        self.ins1 = self.ins1.min(self.ins2);
                    } else if self.ins1 > 0 {
                        self.ins1 -= 1;
                    }
                    self.ins2 = self.ins1;
                }
                self.make_cursor_visible(assets);
                self.cursor_visible = true;
                true
            }
            keys::KEY_RIGHT => {
                if shift {
                    // the original can step one past the end here; clamped
                    self.ins2 = (self.ins2 + self.char_length(self.ins2)).min(self.text.len());
                } else {
                    if self.ins1 != self.ins2 {
                        self.ins1 = self.ins1.max(self.ins2);
                    } else if self.ins1 < self.text.len() {
                        self.ins1 += self.char_length(self.ins1);
                    }
                    self.ins2 = self.ins1;
                }
                self.make_cursor_visible(assets);
                self.cursor_visible = true;
                true
            }
            keys::KEY_DELETE => {
                if !self.clear_selection(assets) {
                    self.back_space(self.ins1 + self.char_length(self.ins1));
                }
                self.make_cursor_visible(assets);
                self.cursor_visible = true;
                true
            }
            keys::KEY_BACK => {
                if !self.clear_selection(assets) {
                    self.back_space(self.ins1);
                }
                self.flush_cursor_right(assets);
                self.make_cursor_visible(assets);
                self.cursor_visible = true;
                true
            }
            keys::KEY_8 => !self.weird_chars && ctx.input.shift(),
            _ => false,
        }
    }

    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
    }

    /// `aEdit::render`.
    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.obj.render(ctx);
        let assets = ctx.assets;
        let mut draw = self.text.clone();
        draw.push(b' ');
        let (gx, gy) = (self.obj.global_x(), self.obj.global_y());
        let (w, h) = (self.obj.width() as i32, self.obj.height() as i32);
        let start = if self.left_offset != 0 {
            self.find_char(assets, self.left_offset)
        } else {
            0
        };
        let tx = gx + ENTRY_MARGIN as i32;
        if self.ins1 != self.ins2 {
            let lo = self.ins1.min(self.ins2);
            let hi = self.ins1.max(self.ins2).min(self.text.len());
            let mut xs = gx + self.char_x_pos(assets, lo) - self.left_offset + ENTRY_MARGIN as i32;
            let mut xe = gx + self.char_x_pos(assets, hi) - self.left_offset + ENTRY_MARGIN as i32;
            xs = xs.max(gx);
            xe = xe.min(gx + w);
            if !self.text.is_empty() {
                font_render(
                    ctx,
                    self.font,
                    &cp1252::decode(&draw[start.min(draw.len())..]),
                    tx,
                    gy,
                    w,
                    h,
                    self.text_color,
                    false,
                    0,
                );
            }
            ctx.dl
                .fill_rect(GuiRect::new(xs, gy, xe, gy + h), self.highlight_color);
            if !self.text.is_empty() {
                // the original passes the selection's right edge as the area width
                font_render(
                    ctx,
                    self.font,
                    &cp1252::decode(&draw[lo..hi]),
                    xs,
                    gy,
                    xe,
                    h,
                    self.selected_color,
                    false,
                    0,
                );
            }
        } else {
            font_render(
                ctx,
                self.font,
                &cp1252::decode(&draw[start.min(draw.len())..]),
                tx,
                gy,
                w,
                h,
                self.text_color,
                false,
                0,
            );
        }
        self.draw_cursor(ctx);
    }

    /// `renderWithDropShadow`: a black copy one pixel left and down first.
    pub fn render_with_drop_shadow(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.font.size == 1 {
            let real = self.text_color;
            self.text_color = 0xff00_0000;
            self.obj.move_by(-1.0, 1.0);
            self.render(ctx);
            self.obj.move_by(1.0, -1.0);
            self.text_color = real;
        }
        self.render(ctx);
    }

    fn draw_cursor(&self, ctx: &mut RenderCtx<'_>) {
        if self.ins1 != self.ins2 || !self.focus {
            return;
        }
        if self.cursor_visible && self.style & ES_EDITREADONLY == 0 {
            let x = self.obj.global_x() as f32
                + (self.char_x_pos(ctx.assets, self.ins1) - self.left_offset) as f32
                + ENTRY_MARGIN;
            let y = self.obj.global_y() as f32;
            ctx.dl.vline(x, y, y + self.obj.height(), self.cursor_color);
        }
    }
}
