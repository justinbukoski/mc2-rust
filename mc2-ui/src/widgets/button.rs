// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aButton` and `aAnimButton` (`gui/abutton.cpp`).
//!
//! A button is a quad whose UVs switch between per-state cells of its texture, with an optional
//! label (a string id drawn in the label rectangle) and static child bitmaps (`<Block>Bmp<n>`).
//! Clicking reports `aMSG_LEFTMOUSEDOWN` with the button id to the parent, which here is the
//! return value of `update`.

use super::object::{uv_bias, Object};
use crate::anim::Animation;
use crate::ctx::{msg, sfx, Loader, Msg, RenderCtx, UpdateCtx};
use crate::draw::{Blend, GuiRect};
use crate::fit::FitExt;
use crate::font::text::{self, TextAttrs};
use crate::font::FontRef;
use mc2_formats::ini::FitIni;

/// `aButton::States`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ButtonState {
    #[default]
    Enabled = 0,
    Pressed = 1,
    Disabled = 2,
    Ambiguous = 3,
    Highlight = 4,
    Hidden = 5,
}

/// `aButton::aButtonData`.
#[derive(Clone, Debug, Default)]
pub struct ButtonData {
    pub id: i32,
    pub text_id: i32,
    pub text_colors: [u32; 5],
    pub font: FontRef,
    pub file_name: String,
    /// `[state][u, v]` in texture pixels; -1 when the state has no cell.
    pub state_coords: [[f32; 2]; 5],
    /// `UWidth` / `VHeight`.
    pub texture_width: f32,
    pub texture_height: f32,
    pub file_width: f32,
    pub file_height: f32,
    pub texture_rotated: bool,
    pub text_rect: GuiRect,
    pub outline_text: bool,
    pub outline: bool,
    pub text_align: i32,
}

#[derive(Clone, Debug)]
pub struct Button {
    pub obj: Object,
    pub data: ButtonData,
    pub state: ButtonState,
    pub toggle: bool,
    pub single_press: bool,
    pub message_on_release: bool,
    pub hold_time: f32,
    pub click_sfx: i32,
    pub highlight_sfx: i32,
    pub disabled_sfx: i32,
}

impl Default for Button {
    fn default() -> Self {
        Button {
            obj: Object::default(),
            data: ButtonData {
                text_align: 2,
                ..ButtonData::default()
            },
            state: ButtonState::Enabled,
            toggle: false,
            single_press: false,
            message_on_release: false,
            hold_time: 0.5,
            click_sfx: sfx::LOG_CLICKONBUTTON,
            highlight_sfx: sfx::LOG_HIGHLIGHTBUTTONS,
            disabled_sfx: sfx::LOG_WRONGBUTTON,
        }
    }
}

impl Button {
    /// `aButton::init(x, y, w, h)`.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Button {
        Button {
            obj: Object::new(x, y, w, h),
            ..Button::default()
        }
    }

    /// `aButton::init(file, block)`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> Button {
        let mut b = Button::default();
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find button {block}");
            return b;
        }
        let d = &mut b.data;
        d.id = file.long_or("ID", 0) as i32;
        d.file_name = file.string("FileName").unwrap_or_default();
        b.obj.help_header = file.long_or("HelpCaption", 0) as i32;
        b.obj.help_id = file.long_or("HelpDesc", 0) as i32;
        d.text_id = file.long_or("TextID", 0) as i32;
        d.text_colors[0] = file.color("TextNormal").unwrap_or(0);
        d.text_colors[1] = file.color("TextPressed").unwrap_or(0);
        d.text_colors[2] = file.color("TextDisabled").unwrap_or(0);
        b.toggle = file.boolean("Toggle").unwrap_or(false);
        d.outline = file.boolean("outline").unwrap_or(false);
        let font_id = file.long_or("Font", 0);
        let x = file.long_or("XLocation", 0) as f32;
        let y = file.long_or("YLocation", 0) as f32;
        let w = file.long_or("Width", 0) as f32;
        let h = file.long_or("Height", 0) as f32;
        d.texture_rotated = file.boolean("texturesRotated").unwrap_or(false);
        d.text_align = file.long("Alignment").map_or(2, |a| a as i32);
        let coord = |u: &str, v: &str| {
            [
                file.long(u).map_or(-1.0, |x| x as f32),
                file.long(v).map_or(-1.0, |x| x as f32),
            ]
        };
        d.state_coords[0] = coord("UNormal", "VNormal");
        d.state_coords[1] = coord("UPressed", "VPressed");
        d.state_coords[2] = coord("UDisabled", "VDisabled");
        d.state_coords[3] = coord("UAmbiguous", "VAmbiguous");
        let hu = file.long("UHighlight").map(|x| x as f32);
        let hv = file.long("VHighlight").map(|x| x as f32);
        d.state_coords[4] = [
            hu.unwrap_or(d.state_coords[0][0]),
            hv.unwrap_or(d.state_coords[0][1]),
        ];
        d.texture_width = file.long_or("UWidth", 0) as f32;
        d.texture_height = file.long_or("VHeight", 0) as f32;
        if d.text_id != 0 {
            d.outline_text = file.boolean("TextOutline").unwrap_or(false);
        }
        d.text_rect = match file.long("XTextLocation") {
            Some(left) => {
                let top = file.long_or("YTextLocation", 0);
                let tw = file.long_or("TextWidth", 0);
                let th = file.long_or("TextHeight", 0);
                d.outline_text = file.boolean("TextOutline").unwrap_or(d.outline_text);
                GuiRect::new(
                    left as i32,
                    top as i32,
                    (left + tw) as i32,
                    (top + th) as i32,
                )
            }
            None => GuiRect::new(x as i32, y as i32, (x + w) as i32, (y + h) as i32),
        };
        b.obj.set_rect(x, y, w, h);
        for v in &mut b.obj.loc {
            v.argb = 0xffff_ffff;
            v.u = 0.0;
            v.v = 0.0;
        }
        if !b.data.file_name.is_empty() {
            let name = b.data.file_name.clone();
            if let Some(t) = ld.art(&name) {
                b.obj.texture = Some(t);
                b.data.file_width = ld.assets.textures.width(t) as f32;
                b.data.file_height = b.data.file_width;
            }
        }
        if font_id != 0 {
            b.data.font = ld.font(font_id as u32);
        }
        // static child bitmaps <block>Bmp0, Bmp1, ... (global coordinates)
        let mut n = 0;
        loop {
            let name = format!("{block}Bmp{n}");
            if !file.has_block(&name) {
                break;
            }
            let child = Object::from_fit(ld, file, &name);
            b.obj.children.push(child);
            n += 1;
        }
        let _ = file.seek_block(block);
        b.disable(false);
        b.press(false);
        b
    }

    pub fn id(&self) -> i32 {
        self.data.id
    }
    pub fn set_id(&mut self, id: i32) {
        self.data.id = id;
    }
    /// `setText(newID)`: the label's string id.
    pub fn set_text(&mut self, id: i32) {
        self.data.text_id = id;
    }
    pub fn set_hold_time(&mut self, t: f32) {
        self.hold_time = t;
    }
    pub fn set_single_press(&mut self) {
        self.single_press = true;
    }
    pub fn set_message_on_release(&mut self) {
        self.message_on_release = true;
    }
    pub fn set_press_fx(&mut self, id: i32) {
        self.click_sfx = id;
    }
    pub fn set_highlight_fx(&mut self, id: i32) {
        self.highlight_sfx = id;
    }
    pub fn set_disabled_fx(&mut self, id: i32) {
        self.disabled_sfx = id;
    }

    pub fn is_pressed(&self) -> bool {
        self.state == ButtonState::Pressed
    }

    /// `isEnabled`: enabled, pressed, ambiguous or highlighted.
    pub fn is_enabled(&self) -> bool {
        matches!(
            self.state,
            ButtonState::Enabled
                | ButtonState::Pressed
                | ButtonState::Ambiguous
                | ButtonState::Highlight
        )
    }

    pub fn is_showing(&self) -> bool {
        self.obj.show
    }

    /// `aButton::makeUVs` for the current state.
    pub fn make_uvs(&mut self) {
        let s = (self.state as usize).min(4);
        let d = &self.data;
        let (left, top) = (d.state_coords[s][0], d.state_coords[s][1]);
        if left == -1.0 || top == -1.0 {
            log::trace!("makeUVs given an invalid state for button {}", d.id);
        }
        let right = left + d.texture_width;
        let bottom = top + d.texture_height;
        let (fw, fh) = (d.file_width, d.file_height);
        if fw == 0.0 || fh == 0.0 {
            return;
        }
        let (bw, bh) = (uv_bias(fw), uv_bias(fh));
        let l = &mut self.obj.loc;
        if d.texture_rotated {
            l[0].u = right / fw + bw;
            l[1].u = left / fw + bw;
            l[2].u = left / fw + bw;
            l[3].u = right / fw + bw;
            l[0].v = top / fh + bw;
            l[1].v = top / fh + bw;
            l[2].v = bottom / fh + bh;
            l[3].v = bottom / fh + bh;
        } else {
            l[0].u = left / fw + bw;
            l[1].u = l[0].u;
            l[2].u = right / fw + bw;
            l[3].u = l[2].u;
            l[0].v = top / fh + bw;
            l[3].v = l[0].v;
            l[1].v = bottom / fh + bh;
            l[2].v = l[1].v;
        }
    }

    /// `aButton::press`.
    pub fn press(&mut self, pressed: bool) {
        if !self.is_enabled() {
            return;
        }
        if !pressed && self.state == ButtonState::Highlight {
            return;
        }
        self.state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Enabled
        };
        self.make_uvs();
    }

    /// `aButton::toggle` is declared but never defined in the original; a toggle flips pressed.
    pub fn toggle_press(&mut self) {
        let p = self.is_pressed();
        self.press(!p);
    }

    pub fn make_ambiguous(&mut self, ambiguous: bool) {
        self.state = if ambiguous {
            ButtonState::Ambiguous
        } else {
            ButtonState::Enabled
        };
        self.make_uvs();
    }

    /// `aButton::disable`.
    pub fn disable(&mut self, disable: bool) {
        if !disable {
            if self.state == ButtonState::Disabled {
                self.state = ButtonState::Enabled;
            }
        } else {
            self.state = ButtonState::Disabled;
        }
        self.make_uvs();
    }

    /// `aButton::hide`.
    pub fn hide(&mut self, hide: bool) {
        self.state = if hide {
            ButtonState::Hidden
        } else {
            ButtonState::Enabled
        };
        if self.state != ButtonState::Hidden {
            self.make_uvs();
        }
    }

    /// `aButton::pointInside`: the quad or the label rectangle.
    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.obj.point_inside(x, y) || self.data.text_rect.contains(x, y)
    }

    /// `aButton::move`: the label rectangle moves too.
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        let r = &mut self.data.text_rect;
        r.left += dx as i32;
        r.right += dx as i32;
        r.top += dy as i32;
        r.bottom += dy as i32;
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        let (dx, dy) = (x - self.obj.loc[0].x, y - self.obj.loc[0].y);
        self.move_by(dx, dy);
    }

    pub fn global_x(&self) -> i32 {
        self.obj.global_x()
    }
    pub fn global_y(&self) -> i32 {
        self.obj.global_y()
    }
    pub fn width(&self) -> f32 {
        self.obj.width()
    }
    pub fn height(&self) -> f32 {
        self.obj.height()
    }

    /// `aButton::update` (the plain, non-animated button).
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Msg> {
        if !self.obj.show {
            return None;
        }
        let mut out = None;
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if self.point_inside(mx, my) {
            let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
            if self.message_on_release
                && ctx.input.left_mouse_released()
                && self.point_inside(dx, dy)
            {
                self.press(false);
                out = Some(Msg::new(msg::LEFTMOUSEDOWN, self.data.id));
                if self.state != ButtonState::Disabled && self.state != ButtonState::Hidden {
                    ctx.play(self.click_sfx);
                } else {
                    ctx.play(self.disabled_sfx);
                }
            }
            if ctx.input.is_left_click() {
                self.press(true);
                if !self.message_on_release && self.point_inside(dx, dy) {
                    out = Some(Msg::new(msg::LEFTMOUSEDOWN, self.data.id));
                }
                if self.state != ButtonState::Disabled {
                    ctx.play(self.click_sfx);
                } else {
                    ctx.play(self.disabled_sfx);
                }
            } else if ctx.input.mouse_left_held() > self.hold_time
                && !self.message_on_release
                && self.point_inside(dx, dy)
            {
                // handleMessage(aMSG_LEFTMOUSEHELD) on itself: aObject ignores it
            } else {
                if self.state != ButtonState::Highlight
                    && self.state != ButtonState::Disabled
                    && self.state != ButtonState::Hidden
                {
                    ctx.play(self.highlight_sfx);
                }
                self.state = ButtonState::Highlight;
                self.make_uvs();
            }
        } else if self.state == ButtonState::Pressed && self.message_on_release {
            self.state = ButtonState::Enabled;
        }
        self.obj.update(ctx);
        out
    }

    /// `aButton::render`.
    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        if self.state == ButtonState::Hidden {
            return;
        }
        ctx.dl.quad(self.obj.texture, Blend::Alpha, self.obj.loc);
        let d = &self.data;
        if d.text_id != 0 {
            if let Some(f) = ctx.assets.font(d.font) {
                let label = ctx.assets.string(d.text_id as u32);
                let color = d.text_colors[(self.state as usize).min(4)];
                let attrs = TextAttrs {
                    color,
                    size: d.font.size as f32,
                    word_wrap: true,
                    proportional: true,
                    bold: false,
                    italic: false,
                    align: d.text_align.max(0) as u32,
                    disable_codes: false,
                };
                let (_, h) = text::string_size(f, label, &attrs);
                let r = d.text_rect;
                let y = (r.top + r.bottom) / 2 - h as i32 / 2 + 1;
                text::draw(ctx.dl, f, label, &attrs, r, (r.left, y));
                if d.outline_text {
                    ctx.dl.outline_rect(r, color, color);
                }
            }
        }
        if d.outline {
            let l = &self.obj.loc;
            let r = GuiRect::new(l[0].x as i32, l[0].y as i32, l[2].x as i32, l[2].y as i32);
            ctx.dl.outline_rect(r, l[0].argb, l[0].argb);
        }
        for c in &self.obj.children {
            c.render(ctx);
        }
    }
}

/// `aAnimButton`: a button whose colour (and label colour) follows per-state animations.
#[derive(Clone, Debug, Default)]
pub struct AnimButton {
    pub button: Button,
    pub normal: Animation,
    pub highlight: Animation,
    pub pressed: Animation,
    pub disabled: Animation,
    pub animate_text: bool,
    pub animate_bmp: bool,
    pub animate_children: bool,
}

impl std::ops::Deref for AnimButton {
    type Target = Button;
    fn deref(&self) -> &Button {
        &self.button
    }
}

impl std::ops::DerefMut for AnimButton {
    fn deref_mut(&mut self) -> &mut Button {
        &mut self.button
    }
}

impl AnimButton {
    pub fn new(button: Button) -> AnimButton {
        AnimButton {
            button,
            animate_text: true,
            animate_bmp: true,
            animate_children: true,
            ..AnimButton::default()
        }
    }

    /// `aAnimButton::init`: the button plus `Normal`/`Pressed`/`Highlight`/`Disabled` animations,
    /// `AnimateBmp`, `AnimateText` and `AnimateChildren`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> AnimButton {
        if !file.has_block(block) {
            log::warn!("couldn't find block {block}");
            return AnimButton::new(Button::default());
        }
        let button = Button::from_fit(ld, file, block);
        let mut a = AnimButton::new(button);
        a.normal = Animation::from_fit(file, "Normal");
        a.pressed = Animation::from_fit(file, "Pressed");
        a.highlight = Animation::from_fit(file, "Highlight");
        a.disabled = Animation::from_fit(file, "Disabled");
        a.normal.begin();
        a.animate_bmp = file.boolean("AnimateBmp").unwrap_or(true);
        a.animate_text = file.boolean("AnimateText").unwrap_or(true);
        if let Some(b) = file.boolean("AnimateChildren") {
            a.animate_children = b;
        }
        a
    }

    /// `setAnimationInfo`.
    pub fn set_animation_info(
        &mut self,
        normal: Option<&Animation>,
        highlight: Option<&Animation>,
        pressed: Option<&Animation>,
        disabled: Option<&Animation>,
    ) {
        self.normal = normal.cloned().unwrap_or_default();
        self.highlight = highlight.cloned().unwrap_or_default();
        self.pressed = pressed.cloned().unwrap_or_default();
        self.disabled = disabled.cloned().unwrap_or_default();
    }

    /// `aAnimButton::update`. Returns the message for the parent, if any.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Msg> {
        if !self.button.obj.show {
            return None;
        }
        let mut out = None;
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        let inside = self.button.point_inside(mx, my);
        let b = &mut self.button;
        if inside && b.state == ButtonState::Disabled {
            ctx.out.help_text_id = b.obj.help_id;
            ctx.out.help_header_id = b.obj.help_header;
        }
        if inside && b.state != ButtonState::Disabled && b.state != ButtonState::Hidden {
            ctx.out.help_text_id = b.obj.help_id;
            ctx.out.help_header_id = b.obj.help_header;
            if ctx.input.is_left_click() {
                if b.toggle {
                    if b.state == ButtonState::Pressed {
                        b.press(false);
                        self.pressed.end();
                    } else {
                        self.pressed.begin();
                        self.highlight.end();
                        b.press(true);
                    }
                } else {
                    b.press(true);
                    self.pressed.begin();
                    self.highlight.end();
                }
                ctx.play(b.click_sfx);
                if !b.message_on_release && b.point_inside(dx, dy) {
                    out = Some(Msg::new(msg::LEFTMOUSEDOWN, b.data.id));
                }
            } else if ctx.input.mouse_left_held() > b.hold_time
                && !b.message_on_release
                && b.point_inside(dx, dy)
            {
                out = Some(Msg::new(msg::LEFTMOUSEHELD, b.data.id));
            } else if ctx.input.left_mouse_released() && b.message_on_release {
                if b.point_inside(dx, dy) {
                    out = Some(Msg::new(msg::LEFTMOUSEDOWN, b.data.id));
                    if !b.toggle {
                        b.state = ButtonState::Enabled;
                    }
                }
            } else if b.state != ButtonState::Pressed {
                if b.state != ButtonState::Highlight
                    && b.state != ButtonState::Disabled
                    && b.state != ButtonState::Hidden
                    && b.obj.show
                {
                    ctx.play(b.highlight_sfx);
                }
                if !self.highlight.is_animating() {
                    self.highlight.begin();
                }
                b.state = ButtonState::Highlight;
                b.make_uvs();
            }
        } else if b.state == ButtonState::Pressed {
            // clicked inside and released outside
            if ctx.input.left_mouse_released()
                && (b.message_on_release || b.single_press)
                && b.point_inside(dx, dy)
            {
                b.state = ButtonState::Enabled;
            } else {
                if !self.pressed.is_animating() {
                    self.pressed.begin();
                }
                if b.single_press && self.pressed.is_done() {
                    b.press(false);
                }
            }
        } else if b.state == ButtonState::Disabled {
            if b.point_inside(mx, my) && ctx.input.is_left_click() {
                ctx.play(b.disabled_sfx);
            }
            if !self.disabled.is_animating() {
                self.disabled.begin();
            }
            self.disabled.update(ctx.dt);
        } else {
            self.highlight.end();
            if b.state == ButtonState::Highlight {
                b.state = ButtonState::Enabled;
            }
            b.press(false);
        }
        if self.pressed.is_animating() && b.state != ButtonState::Pressed {
            self.pressed.end();
            b.press(false);
        }
        if b.state != ButtonState::Disabled {
            self.disabled.end();
        }
        self.highlight.update(ctx.dt);
        self.pressed.update(ctx.dt);
        self.normal.update(ctx.dt);
        out
    }

    /// `aAnimButton::render`: picks the running animation (disabled, pressed, highlight, normal)
    /// and draws the button in its colour.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.button.obj.show {
            return;
        }
        let anim = if self.disabled.is_animating() {
            &self.disabled
        } else if self.pressed.is_animating() {
            &self.pressed
        } else if self.highlight.is_animating() {
            &self.highlight
        } else {
            &self.normal
        };
        let color = anim.color();
        let (xd, yd) = (anim.x_delta(), anim.y_delta());
        let (sx, sy) = (anim.scale_x(), anim.scale_x());
        let b = &mut self.button;
        if self.animate_bmp {
            b.obj.set_color(color, false);
        }
        if self.animate_text {
            let s = (b.state as usize).min(4);
            b.data.text_colors[s] = color;
        }
        if self.animate_children {
            for c in &mut b.obj.children {
                c.set_color(color, false);
            }
        }
        // The original moves the button by the animation offset and never moves it back; the
        // shipped button animations have no position keys, so the offset is applied for the
        // draw only.
        b.move_by(xd, yd);
        if sx != 1.0 && sy != 1.0 {
            // scale about the centre (the original's resize multiplies by half the width
            // again, which only `mcl_cm_standinop.fit` would ever hit)
            let (w, h) = (b.width(), b.height());
            let (ol, ot) = (b.obj.loc[0].x, b.obj.loc[0].y);
            let (mx, my) = (ol + 0.5 * w, ot + 0.5 * h);
            b.obj
                .move_to_no_recurse(mx - 0.5 * sx * w, my - 0.5 * sy * h);
            b.obj.resize(w * sx, h * sy);
            b.render(ctx);
            b.obj.resize(w, h);
            b.obj.move_to_no_recurse(ol, ot);
        } else {
            b.render(ctx);
        }
        b.move_by(-xd, -yd);
    }
}
