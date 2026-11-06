// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aObject` (`gui/asystem.cpp`): a textured or flat quad in absolute screen coordinates that
//! carries help ids and, for buttons, static child bitmaps.
//!
//! As in the original every object stores its four corners in global coordinates; moving a parent
//! moves its children by the same offset.

use crate::ctx::{Loader, RenderCtx, UpdateCtx};
use crate::draw::{Blend, GuiRect, Vertex};
use crate::fit::FitExt;
use crate::textures::TexId;
use mc2_formats::ini::FitIni;

#[derive(Clone, Debug)]
pub struct Object {
    /// Corners: 0 top-left, 1 bottom-left, 2 bottom-right, 3 top-right.
    pub loc: [Vertex; 4],
    pub texture: Option<TexId>,
    /// Texture width the UVs were divided by (`fileWidth`).
    pub file_width: f32,
    pub show: bool,
    pub id: i32,
    pub help_header: i32,
    pub help_id: i32,
    pub children: Vec<Object>,
}

impl Default for Object {
    fn default() -> Self {
        Object {
            loc: [Vertex::default(); 4],
            texture: None,
            file_width: 0.0,
            show: true,
            id: 0,
            help_header: 0,
            help_id: 0,
            children: Vec::new(),
        }
    }
}

/// `.1f / fileWidth`: the half-texel style bias the original adds to every UV.
pub(crate) fn uv_bias(fw: f32) -> f32 {
    0.1 / fw
}

impl Object {
    /// `aObject::init(x, y, w, h)`: an untextured black quad.
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Object {
        let mut o = Object::default();
        o.set_rect(x, y, w, h);
        for v in &mut o.loc {
            v.argb = 0xff00_0000;
        }
        o
    }

    pub(crate) fn set_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.loc[0].x = x;
        self.loc[0].y = y;
        self.loc[1].x = x;
        self.loc[1].y = y + h;
        self.loc[2].x = x + w;
        self.loc[2].y = y + h;
        self.loc[3].x = x + w;
        self.loc[3].y = y;
    }

    /// `aObject::init(file, block)` for statics: position, size, help ids, `fileName` (`.tga`
    /// appended, `Alpha` selects alpha vs keyed), `UNormal`/`VNormal`/`UWidth`/`VHeight` and
    /// `texturesRotated`. Missing numbers read as 0.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> Object {
        let mut o = Object {
            file_width: 256.0,
            ..Object::default()
        };
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find static block {block}");
            return o;
        }
        let x = file.long_or("XLocation", 0) as f32;
        let y = file.long_or("YLocation", 0) as f32;
        let w = file.long_or("Width", 0) as f32;
        let h = file.long_or("Height", 0) as f32;
        o.help_header = file.long_or("HelpCaption", 0) as i32;
        o.help_id = file.long_or("HelpDesc", 0) as i32;
        if let Some(name) = file.string("fileName") {
            // `Alpha` selects gos_Texture_Alpha, otherwise the art is keyed (one bit of alpha)
            let alpha = file.boolean("Alpha").unwrap_or(false);
            let tex = if alpha {
                ld.art(&name)
            } else {
                ld.art_keyed(&name)
            };
            if let Some(t) = tex {
                o.texture = Some(t);
                o.file_width = ld.assets.textures.width(t) as f32;
            }
        }
        let u = file.long_or("UNormal", 0) as f32;
        let v = file.long_or("VNormal", 0) as f32;
        let uw = file.long_or("UWidth", 0) as f32;
        let vh = file.long_or("VHeight", 0) as f32;
        let rotated = file.boolean("texturesRotated").unwrap_or(false);
        let fw = o.file_width;
        let b = uv_bias(fw);
        for k in &mut o.loc {
            *k = Vertex {
                x,
                y,
                u: u / fw + b,
                v: v / fw + b,
                argb: 0xffff_ffff,
            };
        }
        o.loc[3].x = x + w;
        o.loc[2].x = x + w;
        o.loc[2].y = y + h;
        o.loc[1].y = y + h;
        o.loc[2].u = (u + uw) / fw + b;
        o.loc[3].u = o.loc[2].u;
        o.loc[1].v = (v + vh) / fw + b;
        o.loc[2].v = o.loc[1].v;
        if rotated {
            o.loc[0].u = (u + uw) / fw + b;
            o.loc[1].u = u / fw + b;
            o.loc[2].u = u / fw + b;
            o.loc[3].u = (u + uw) / fw + b;
            o.loc[0].v = v / fw + b;
            o.loc[1].v = v / fw + b;
            o.loc[2].v = (v + vh) / fw + b;
            o.loc[3].v = (v + vh) / fw + b;
        }
        o
    }

    pub fn width(&self) -> f32 {
        self.loc[2].x - self.loc[0].x
    }
    pub fn height(&self) -> f32 {
        self.loc[2].y - self.loc[0].y
    }
    pub fn global_x(&self) -> i32 {
        self.loc[0].x as i32
    }
    pub fn global_y(&self) -> i32 {
        self.loc[0].y as i32
    }
    pub fn left(&self) -> f32 {
        self.loc[0].x
    }
    pub fn top(&self) -> f32 {
        self.loc[0].y
    }
    pub fn right(&self) -> f32 {
        self.loc[0].x + self.width()
    }
    pub fn bottom(&self) -> f32 {
        self.loc[0].y + self.height()
    }
    pub fn global_right(&self) -> i32 {
        self.global_x() + self.width() as i32
    }
    pub fn global_bottom(&self) -> i32 {
        self.global_y() + self.height() as i32
    }
    pub fn global_rect(&self) -> GuiRect {
        GuiRect::new(
            self.global_x(),
            self.global_y(),
            self.global_right(),
            self.global_bottom(),
        )
    }
    pub fn color(&self) -> u32 {
        self.loc[0].argb
    }
    pub fn is_showing(&self) -> bool {
        self.show
    }
    pub fn show_window(&mut self, show: bool) {
        self.show = show;
    }

    /// `aObject::pointInside`: inclusive bounds.
    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        let (x, y) = (x as f32, y as f32);
        self.loc[0].x <= x && self.loc[3].x >= x && self.loc[0].y <= y && self.loc[1].y >= y
    }

    /// `aObject::move`: shifts this object and its children.
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.move_no_recurse(dx, dy);
        for c in &mut self.children {
            c.move_by(dx, dy);
        }
    }

    pub fn move_no_recurse(&mut self, dx: f32, dy: f32) {
        for v in &mut self.loc {
            v.x += dx;
            v.y += dy;
        }
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        let (dx, dy) = (x - self.loc[0].x, y - self.loc[0].y);
        self.move_by(dx, dy);
    }

    pub fn move_to_no_recurse(&mut self, x: f32, y: f32) {
        let (dx, dy) = (x - self.loc[0].x, y - self.loc[0].y);
        self.move_no_recurse(dx, dy);
    }

    pub fn resize(&mut self, w: f32, h: f32) {
        self.loc[2].x = self.loc[0].x + w;
        self.loc[3].x = self.loc[0].x + w;
        self.loc[1].y = self.loc[0].y + h;
        self.loc[2].y = self.loc[0].y + h;
    }

    pub fn set_color(&mut self, c: u32, recurse: bool) {
        for v in &mut self.loc {
            v.argb = c;
        }
        if recurse {
            for ch in &mut self.children {
                ch.set_color(c, true);
            }
        }
    }

    /// `aObject::setUVs` (pixel coordinates divided by `fileWidth`).
    pub fn set_uvs(&mut self, u1: f32, v1: f32, u2: f32, v2: f32) {
        let fw = self.file_width;
        let b = uv_bias(fw);
        self.loc[0].u = u1 / fw + b;
        self.loc[1].u = self.loc[0].u;
        self.loc[2].u = u2 / fw + b;
        self.loc[3].u = self.loc[2].u;
        self.loc[0].v = v1 / fw + b;
        self.loc[3].v = self.loc[0].v;
        self.loc[1].v = v2 / fw + b;
        self.loc[2].v = self.loc[1].v;
    }

    /// `aObject::setTexture`.
    pub fn set_texture(&mut self, t: Option<TexId>, width: u32) {
        self.texture = t;
        self.file_width = if t.is_some() { width as f32 } else { 256.0 };
    }

    /// `aObject::update`: publishes the help id while the pointer is over a shown object.
    pub fn update(&self, ctx: &mut UpdateCtx<'_>) {
        if self.help_id != 0
            && self.show
            && self.point_inside(ctx.input.mouse_x(), ctx.input.mouse_y())
        {
            ctx.out.help_text_id = self.help_id;
        }
        for c in &self.children {
            c.update(ctx);
        }
    }

    /// Draws just this quad (no children).
    pub fn draw_quad(&self, ctx: &mut RenderCtx<'_>) {
        ctx.dl.quad(self.texture, Blend::Alpha, self.loc);
    }

    /// `aObject::render`.
    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        if !self.show {
            return;
        }
        self.draw_quad(ctx);
        for c in &self.children {
            c.render(ctx);
        }
    }

    /// `aObject::render(x, y)`: drawn shifted.
    pub fn render_at(&mut self, ctx: &mut RenderCtx<'_>, x: f32, y: f32) {
        self.move_by(x, y);
        self.render(ctx);
        self.move_by(-x, -y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_and_moves() {
        let mut o = Object::new(10.0, 20.0, 30.0, 40.0);
        o.children.push(Object::new(15.0, 25.0, 5.0, 5.0));
        assert_eq!((o.width(), o.height()), (30.0, 40.0));
        assert!(o.point_inside(10, 20) && o.point_inside(40, 60) && !o.point_inside(41, 60));
        o.move_to(0.0, 0.0);
        assert_eq!(o.children[0].global_x(), 5);
        o.resize(8.0, 9.0);
        assert_eq!(o.global_rect(), GuiRect::new(0, 0, 8, 9));
        assert_eq!(o.color(), 0xff00_0000);
    }
}
