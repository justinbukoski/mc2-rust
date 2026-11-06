// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aRect`, `aText` (`gui/asystem.cpp`), `aAnimObject` (`gui/aanimobject.cpp`) and the `aFont`
//! drawing/measuring helpers (`gui/afont.cpp`).

use super::object::Object;
use crate::anim::Animation;
use crate::ctx::UiAssets;
use crate::ctx::{Loader, RenderCtx, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::font::text::{self, TextAttrs};
use crate::font::{Font, FontRef};
use mc2_formats::ini::FitIni;

// ---------------------------------------------------------------------------------------------
// aFont helpers

fn attrs(font: FontRef, color: u32, bold: bool, align: i32) -> TextAttrs {
    TextAttrs {
        color,
        size: font.size as f32,
        word_wrap: true,
        proportional: true,
        bold,
        italic: false,
        align: align.max(0) as u32,
        disable_codes: false,
    }
}

/// `aFont::width`.
pub fn font_width(assets: &UiAssets, font: FontRef, s: &str) -> i32 {
    match assets.font(font) {
        Some(f) => text::string_size(f, s, &attrs(font, 0, false, 0)).0 as i32,
        None => 0,
    }
}

/// `aFont::height()`: the height of one line (`gos_TextStringLength` of `"ABCDE"`).
pub fn font_height(assets: &UiAssets, font: FontRef) -> i32 {
    match assets.font(font) {
        Some(f) => text::string_size(f, "ABCDE", &attrs(font, 0, false, 0)).1 as i32,
        None => 0,
    }
}

/// `aFont::height(st, areaWidth)`: the height `st` needs when wrapped to `area_width`, counted
/// the way the original counts lines (including its extra line whenever the text is wider than
/// the area).
pub fn font_text_height(assets: &UiAssets, font: FontRef, st: &str, area_width: i32) -> i32 {
    let Some(f) = assets.font(font) else { return 0 };
    let a = attrs(font, 0, false, 0);
    let width = |s: &[u8]| text::string_size(f, &crate::cp1252::decode(s), &a).0 as i32;
    let bytes = crate::cp1252::encode(st);
    let mut line_count = 1;
    if width(&bytes) > area_width - 1 {
        let has_spaces = bytes.contains(&b' ') || bytes.contains(&b'\n');
        let mut line: Vec<u8> = Vec::new();
        let mut last_word = 0usize;
        let mut line_start = 0usize;
        let mut words = 0;
        let mut p = 0usize;
        while p < bytes.len() {
            let c = bytes[p];
            if c == b'\n' {
                line_count += 1;
                words = 0;
                line.clear();
                last_word = p + 1;
                line_start = last_word;
            } else if !has_spaces {
                if p > 0 && width(&line) > area_width {
                    line_count += 1;
                    p -= 1;
                    line.clear();
                    words = 0;
                }
            } else if (c as char).is_ascii_whitespace() {
                if width(&line) > area_width {
                    let off = last_word.saturating_sub(line_start).min(line.len());
                    line.truncate(off);
                    if words == 0 || width(&line) > area_width {
                        // a word wider than the area: give up on it
                        last_word = p;
                    }
                    line_count += 1;
                    line.clear();
                    p = last_word;
                    words = 0;
                    line_start = last_word;
                }
                last_word = p;
                words += 1;
            }
            line.push(bytes[p]);
            p += 1;
        }
        if width(&line) > area_width {
            line_count += 1;
        }
        line_count += 1;
    }
    let h = text::string_size(f, "A", &a).1 as i32;
    h * line_count
}

/// `aFont::render(text, x, y, w, h, color, bold, alignment)`.
#[allow(clippy::too_many_arguments)]
pub fn font_render(
    ctx: &mut RenderCtx<'_>,
    font: FontRef,
    s: &str,
    mut x: i32,
    mut y: i32,
    mut w: i32,
    mut h: i32,
    color: u32,
    bold: bool,
    align: i32,
) {
    let Some(f) = ctx.assets.font(font) else {
        return;
    };
    let a = attrs(font, color, bold, align);
    let (sw, sh) = ctx.screen();
    if w < 1 {
        if align == 1 {
            let (tw, _) = text::string_size(f, s, &a);
            x -= tw as i32;
            w = tw as i32 + 1;
        } else {
            w = sw;
        }
    }
    if h < 1 {
        if align == 3 {
            let (_, th) = text::string_size(f, s, &a);
            y -= th as i32;
            h = th as i32 + 1;
        } else {
            h = sh;
        }
    }
    text::draw(ctx.dl, f, s, &a, GuiRect::new(x, y, x + w, y + h), (x, y));
}

/// Draws with explicit GameOS attributes (for callers that set them directly).
pub fn draw_text(
    ctx: &mut RenderCtx<'_>,
    font: &Font,
    s: &str,
    attrs: &TextAttrs,
    region: GuiRect,
    pos: (i32, i32),
) {
    text::draw(ctx.dl, font, s, attrs, region, pos);
}

// ---------------------------------------------------------------------------------------------
// aRect

#[derive(Clone, Debug, Default)]
pub struct Rect {
    pub obj: Object,
    pub outline: bool,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32, color: u32, outline: bool) -> Rect {
        let mut obj = Object::new(x, y, w, h);
        obj.set_color(color, false);
        Rect { obj, outline }
    }

    /// `aRect::init`: `left/top/right/bottom` or `XLocation/YLocation/Width/Height`, `color`
    /// (default opaque black), `outline`, help ids.
    pub fn from_fit(file: &mut FitIni, block: &str) -> Rect {
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find block {block}");
        }
        let obj = match file.long("left") {
            Some(left) => {
                let top = file.long_or("top", 0);
                let right = file.long_or("right", 0);
                let bottom = file.long_or("bottom", 0);
                Object::new(
                    left as f32,
                    top as f32,
                    (right - left) as f32,
                    (bottom - top) as f32,
                )
            }
            None => Object::new(
                file.long_or("XLocation", 0) as f32,
                file.long_or("YLocation", 0) as f32,
                file.long_or("Width", 0) as f32,
                file.long_or("Height", 0) as f32,
            ),
        };
        let mut r = Rect {
            obj,
            outline: false,
        };
        r.obj
            .set_color(file.color("color").unwrap_or(0xff00_0000), false);
        r.outline = file.boolean("outline").unwrap_or(false);
        r.obj.help_header = file.long_or("HelpCaption", 0) as i32;
        r.obj.help_id = file.long_or("HelpDesc", 0) as i32;
        r
    }

    pub fn color(&self) -> u32 {
        self.obj.color()
    }

    pub fn update(&self, ctx: &mut UpdateCtx<'_>) {
        self.obj.update(ctx);
    }

    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.draw(ctx, 0, 0);
    }

    /// `aRect::render(x, y)` (drawn whether shown or not, as in the original).
    pub fn render_at(&self, ctx: &mut RenderCtx<'_>, x: i32, y: i32) {
        self.draw(ctx, x, y);
    }

    fn draw(&self, ctx: &mut RenderCtx<'_>, x: i32, y: i32) {
        let mut r = self.obj.global_rect();
        r.left += x;
        r.right += x;
        r.top += y;
        r.bottom += y;
        let c = self.color();
        if self.outline {
            ctx.dl.outline_rect(r, c, c);
        } else {
            ctx.dl.fill_rect(r, c);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// aText

#[derive(Clone, Debug, Default)]
pub struct Text {
    pub obj: Object,
    pub text: String,
    /// 0 left, 1 right, 2 centred, 3 centred X and Y (`gos_TextSetAttributes` `WrapType`).
    pub alignment: i32,
    pub font: FontRef,
}

impl Text {
    /// `aText::init`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> Text {
        let mut t = Text::default();
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find the text block {block}");
            return t;
        }
        let font_id = file.long_or("Font", 0) as u32;
        t.font = ld.font(font_id);
        // the font load moves nothing; re-seek is not needed (the block stays selected)
        t.obj = Object::new(
            file.long_or("XLocation", 0) as f32,
            file.long_or("YLocation", 0) as f32,
            file.long_or("Width", 0) as f32,
            file.long_or("Height", 0) as f32,
        );
        if let Some(c) = file.color("Color") {
            t.obj.set_color(c, false);
        }
        t.alignment = file.long_or("Alignment", 0) as i32;
        if let Some(id) = file.long("TextID") {
            t.text = ld.assets.string(id as u32).to_string();
        }
        t.obj.help_header = file.long_or("HelpCaption", 0) as i32;
        t.obj.help_id = file.long_or("HelpDesc", 0) as i32;
        t
    }

    pub fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
    }

    /// `aText::setText(resID)`: a missing string shows as `NULL for ID: n`.
    pub fn set_text_id(&mut self, assets: &UiAssets, id: i32) {
        let s = assets.string(id as u32);
        self.text = if s.is_empty() {
            format!("NULL for ID: {id}")
        } else {
            s.to_string()
        };
    }

    pub fn set_color(&mut self, c: u32) {
        self.obj.set_color(c, false);
    }

    pub fn update(&self, ctx: &mut UpdateCtx<'_>) {
        self.obj.update(ctx);
    }

    /// `aText::pointInside`: a text without a size is hit-tested on its rendered extent.
    pub fn point_inside(&self, assets: &UiAssets, x: i32, y: i32) -> bool {
        if self.obj.width() == 0.0 || self.obj.height() == 0.0 {
            let mut left = self.obj.global_x();
            let top = self.obj.global_y();
            let w = font_width(assets, self.font, &self.text);
            let h = font_height(assets, self.font);
            if self.alignment == 1 {
                left -= w;
            }
            if left <= x && left + w >= x && top <= y && top + h >= y {
                return true;
            }
        }
        self.obj.point_inside(x, y)
    }

    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        if self.obj.show && !self.text.is_empty() {
            let o = &self.obj;
            font_render(
                ctx,
                self.font,
                &self.text,
                o.loc[0].x as i32,
                o.loc[0].y as i32,
                (o.loc[2].x - o.loc[0].x) as i32,
                (o.loc[2].y - o.loc[0].y) as i32,
                o.loc[0].argb,
                false,
                self.alignment,
            );
        }
    }

    pub fn render_at(&mut self, ctx: &mut RenderCtx<'_>, x: f32, y: f32) {
        self.obj.move_by(x, y);
        self.render(ctx);
        self.obj.move_by(-x, -y);
    }
}

// ---------------------------------------------------------------------------------------------
// aAnimObject

#[derive(Clone, Debug, Default)]
pub struct AnimObject {
    pub obj: Object,
    pub anim: Animation,
}

impl AnimObject {
    /// `aAnimObject::init`: a static plus `Color` (default white) and the animation in the block
    /// named by `Animation`, started at once.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> AnimObject {
        let mut obj = Object::from_fit(ld, file, block);
        obj.set_color(file.color("Color").unwrap_or(0xffff_ffff), false);
        let mut anim = Animation::default();
        if let Some(name) = file.string("Animation") {
            if file.seek_block(&name).is_ok() {
                anim = Animation::from_fit(file, "");
            }
        }
        anim.begin();
        AnimObject { obj, anim }
    }

    pub fn begin(&mut self) {
        self.anim.begin();
    }
    pub fn end(&mut self) {
        self.anim.end();
    }
    pub fn reverse_begin(&mut self) {
        self.anim.reverse_begin();
    }
    pub fn is_done(&self) -> bool {
        self.anim.is_done()
    }

    /// `aAnimObject::update`: only advances the animation.
    pub fn update(&mut self, dt: f32) {
        self.anim.update(dt);
    }

    /// `aAnimObject::render`: drawn offset by the animation's position (plus half a pixel, as in
    /// the original), in its colour, scaled about the centre.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        let color = self.anim.color();
        let xo = self.anim.x_delta() + 0.5;
        let yo = self.anim.y_delta() + 0.5;
        self.obj.move_by(xo, yo);
        self.obj.set_color(color, false);
        let (sx, sy) = (self.anim.scale_x(), self.anim.scale_y());
        if sx != 1.0 || sy != 1.0 {
            let (w, h) = (self.obj.width(), self.obj.height());
            let (ol, ot) = (self.obj.loc[0].x, self.obj.loc[0].y);
            let mid_x = ol + 0.5 * w;
            let mid_y = ot + 0.5 * h;
            self.obj
                .move_to_no_recurse(mid_x - 0.5 * sx * w, mid_y - 0.5 * sy * h);
            self.obj.resize(sx * w, sy * h);
            self.obj.render(ctx);
            // the original restores `width() + .5`, growing the object every scaled frame;
            // the exact size is restored here
            self.obj.resize(w, h);
            self.obj.move_to_no_recurse(ol, ot);
        } else {
            self.obj.render(ctx);
        }
        self.obj.move_by(-xo, -yo);
    }
}
