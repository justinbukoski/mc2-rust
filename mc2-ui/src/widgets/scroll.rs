// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aScrollBar`, `mcScrollBar` and `mcScrollButton` (`gui/ascroll.cpp`): a vertical bar with
//! up/down buttons and a draggable tab. Scrolling is reported to the parent (a list box) as
//! messages: `aMSG_SCROLLTO` with the new position, and `aMSG_SCROLLUP` / `aMSG_SCROLLDOWN`.

use super::button::{AnimButton, ButtonState};
use super::object::Object;
use crate::anim::Animation;
use crate::ctx::{msg, Loader, Msg, RenderCtx, UpdateCtx};
use crate::draw::GuiRect;
use mc2_platform::input::KeyStatus;
use mc2_platform::keys;

/// `mcScrollButton`: the tab, drawn flat in `regular_color` with a bevel.
#[derive(Clone, Debug, Default)]
pub struct ScrollButton {
    pub button: AnimButton,
    pub light_edge: u32,
    pub dark_edge: u32,
    pub regular_color: u32,
}

impl ScrollButton {
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let b = &mut self.button.button;
        if b.obj.show && b.state != ButtonState::Disabled {
            b.obj.set_color(self.regular_color, false);
            b.render(ctx);
            let r = GuiRect::new(
                b.obj.global_x(),
                b.obj.global_y(),
                b.obj.global_right() - 1,
                b.obj.global_bottom() - 1,
            );
            ctx.dl.outline_rect(r, self.light_edge, self.dark_edge);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScrollBar {
    pub obj: Object,
    pub scroll_max: f32,
    pub scroll_pos: f32,
    pub top: AnimButton,
    pub bottom: AnimButton,
    pub tab: ScrollButton,
    last_y: i32,
    pub scroll_inc: i32,
    pub page_inc: i32,
    /// Outline colour.
    pub color: u32,
    /// Position when the last `aMSG_SCROLLTO` was sent (the original's parent reads the bar while
    /// it still has the old position).
    pub pos_before_scroll_to: f32,
    orange: [Animation; 4],
    green: [Animation; 4],
}

impl ScrollBar {
    /// `mcScrollBar::init(x, y, w, h)`: buttons and colours from `data/art/scrollbar.fit`.
    pub fn new(ld: &mut Loader<'_>, x: f32, y: f32, w: f32, h: f32) -> ScrollBar {
        let mut s = ScrollBar::default();
        match ld.art_fit("scrollbar") {
            Ok(mut file) => {
                s.top = AnimButton::from_fit(ld, &mut file, "ScrollButton0");
                s.bottom = AnimButton::from_fit(ld, &mut file, "ScrollButton1");
                let bx = s.bottom.obj.loc[0].x;
                let bh = s.bottom.height();
                s.bottom.move_to(bx, h - bh - 2.0);
                s.tab.button = AnimButton::from_fit(ld, &mut file, "ScrollButton2");
                let tx = s.tab.button.obj.loc[0].x;
                let tb = s.top.obj.bottom();
                s.tab.button.move_to(tx, tb + 1.0);
                if file.seek_block("Orange").is_ok() {
                    s.orange = [
                        Animation::from_fit(&file, "Normal"),
                        Animation::from_fit(&file, "Highlight"),
                        Animation::from_fit(&file, "Pressed"),
                        Animation::from_fit(&file, "Disabled"),
                    ];
                }
                if file.seek_block("Green").is_ok() {
                    s.green = [
                        Animation::from_fit(&file, "Normal"),
                        Animation::from_fit(&file, "Highlight"),
                        Animation::from_fit(&file, "Pressed"),
                        Animation::from_fit(&file, "Disabled"),
                    ];
                }
            }
            Err(e) => log::warn!("{e}"),
        }
        s.obj = Object::new(x, y, w, h);
        s.obj.set_color(0, false);
        s.color = 0xff00_2f55;
        // aScrollBar::init
        s.top.set_id(msg::SCROLLUP as i32);
        s.bottom.set_id(msg::SCROLLDOWN as i32);
        for b in [&mut s.top, &mut s.bottom, &mut s.tab.button] {
            b.set_single_press();
            b.set_hold_time(0.001);
            b.set_press_fx(-1);
            b.set_highlight_fx(-1);
            b.set_disabled_fx(-1);
            // addChild: children are laid out relative to the bar
            b.move_by(x, y);
        }
        s.tab.button.obj.texture = None;
        s.set_scroll_max(0.0);
        s.set_scroll_pos(0.0);
        s.scroll_inc = 1;
        s.page_inc = 10;
        s.tab.light_edge = 0xff00_5392;
        s.tab.dark_edge = 0xff00_2d51;
        s.tab.regular_color = 0xff00_4275;
        s
    }

    pub fn height(&self) -> f32 {
        self.obj.height()
    }
    pub fn width(&self) -> f32 {
        self.obj.width()
    }

    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.top.move_by(dx, dy);
        self.bottom.move_by(dx, dy);
        self.tab.button.move_by(dx, dy);
    }

    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.obj.point_inside(x, y)
    }

    /// `mcScrollBar::resize`.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.obj.resize(w, h);
        let bx = self.bottom.obj.loc[0].x;
        let bh = self.bottom.height();
        self.bottom
            .move_to(bx, self.obj.global_y() as f32 + h - bh - 2.0);
        let tx = self.tab.button.obj.loc[0].x;
        let ty = self.top.obj.global_y() as f32 + self.top.height() + 1.0;
        self.tab.button.move_to(tx, ty);
        self.resize_areas();
    }

    pub fn set_scroll_max(&mut self, m: f32) {
        self.scroll_max = m;
        self.tab.button.obj.show = m != 0.0;
        self.resize_areas();
    }

    /// `SetScrollPos`.
    pub fn set_scroll_pos(&mut self, mut p: f32) {
        if p < 0.0 {
            p = 0.0;
        }
        if p > self.scroll_max {
            p = self.scroll_max;
        }
        self.scroll_pos = p;
        self.resize_areas();
    }

    /// `SetScroll`: clamps, tells the parent (`aMSG_SCROLLTO`) and moves the tab. The parent
    /// sees the position from before the change in [`ScrollBar::pos_before_scroll_to`].
    pub fn set_scroll(&mut self, p: i32) -> Msg {
        let mut p = p;
        if p < 0 {
            p = 0;
        }
        if p as f32 > self.scroll_max {
            p = self.scroll_max as i32;
        }
        self.pos_before_scroll_to = self.scroll_pos;
        let m = Msg::new(msg::SCROLLTO, p);
        self.set_scroll_pos(p as f32);
        m
    }

    fn resize_areas(&mut self) {
        if self.scroll_max == 0.0 {
            return;
        }
        let phys = self.height() - self.top.height() - self.bottom.height() - 6.0;
        self.page_inc = phys as i32;
        let mut tab = phys * phys / (phys + self.scroll_max);
        let tw = self.tab.button.width();
        if tab < tw {
            tab = tw;
        }
        self.tab.button.obj.resize(tw, tab);
        let range = self.height() - self.top.height() - self.bottom.height() - tab - 6.0;
        let position = range * self.scroll_pos / self.scroll_max;
        let x = self.obj.global_x() as f32 + 2.0;
        let y = self.top.obj.global_y() as f32 + self.top.height() + position + 1.0;
        self.tab.button.move_to(x, y);
    }

    /// `Enable`.
    pub fn enable(&mut self, on: bool) {
        self.tab.button.disable(!on);
        self.top.disable(!on);
        self.bottom.disable(!on);
    }

    pub fn set_orange(&mut self) {
        let o = self.orange.clone();
        for b in [&mut self.top, &mut self.bottom, &mut self.tab.button] {
            b.set_animation_info(Some(&o[0]), Some(&o[1]), Some(&o[2]), Some(&o[3]));
        }
        self.tab.light_edge = 0xffc6_6600;
        self.tab.dark_edge = 0xff5c_2f00;
        self.tab.regular_color = 0xff9e_5200;
        self.color = 0xff43_311c;
    }

    pub fn set_green(&mut self) {
        let g = self.green.clone();
        for b in [&mut self.top, &mut self.bottom, &mut self.tab.button] {
            b.set_animation_info(Some(&g[0]), Some(&g[1]), Some(&g[2]), Some(&g[3]));
        }
        self.tab.light_edge = 0xff6e_7c00;
        self.tab.dark_edge = 0xff30_3600;
        self.tab.regular_color = 0xff58_6300;
        self.color = 0xff6e_7c00;
    }

    /// `aScrollBar::handleMessage` for a child button: steps the position and forwards the id.
    fn child_message(&mut self, who: i32, out: &mut Vec<Msg>) {
        if who == msg::SCROLLUP as i32 {
            self.set_scroll_pos(self.scroll_pos - self.scroll_inc as f32);
        } else if who == msg::SCROLLDOWN as i32 {
            self.set_scroll_pos(self.scroll_pos + self.scroll_inc as f32);
        }
        out.push(Msg::new(who as u32, who));
    }

    /// `aScrollBar::update`: tab dragging, paging on the track, child buttons. Returns the messages
    /// for the parent in order.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Vec<Msg> {
        let mut out = Vec::new();
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        if ctx.input.is_left_drag() && self.last_y != 0 {
            let final_pos = (my - dy + self.last_y) as f32;
            let mut phys = self.height()
                - self.top.height()
                - self.bottom.height()
                - self.tab.button.height()
                - 2.0;
            let real = self.scroll_max;
            if phys == 0.0 {
                phys = real;
            }
            if phys.abs() > 1e-4 && real.abs() > 1e-4 {
                let p = (0.5 + final_pos * real / phys).clamp(0.0, self.scroll_max);
                out.push(self.set_scroll(p as i32));
                self.tab.button.press(true);
            }
        } else if self.point_inside(mx, my) {
            let released = ctx.input.left_mouse_released();
            if ctx.input.is_left_click()
                || ctx.input.key_status(keys::KEY_LMOUSE) == KeyStatus::Held
                || released
            {
                self.last_y = 0;
                if self.tab.button.point_inside(mx, my) && !released {
                    self.last_y = (self.tab.button.obj.top() - self.top.obj.bottom()) as i32;
                } else {
                    if !self.top.point_inside(mx, my)
                        && !self.bottom.point_inside(mx, my)
                        && !self.top.point_inside(dx, dy)
                        && !self.bottom.point_inside(dx, dy)
                        && my > self.top.obj.global_bottom()
                        && my < self.bottom.obj.global_y()
                        && self.point_inside(dx, dy)
                        && !self.tab.button.point_inside(mx, my)
                        && (released || ctx.input.mouse_left_held() > 0.5)
                    {
                        let mut p = self.scroll_pos;
                        if my < self.tab.button.obj.global_y() {
                            p = self.scroll_pos - self.page_inc as f32;
                        } else if my > self.tab.button.obj.global_bottom() {
                            p = self.scroll_pos + self.page_inc as f32;
                        }
                        let p = p.clamp(0.0, self.scroll_max.max(0.0));
                        self.pos_before_scroll_to = self.scroll_pos;
                        out.push(Msg::new(msg::SCROLLTO, p as i32));
                        self.set_scroll_pos(p);
                    }
                    self.tab.button.press(false);
                }
            }
        }
        if ctx.input.left_mouse_released() {
            self.last_y = 0;
        }
        // aObject::update: the children
        self.obj.update(ctx);
        for i in 0..3 {
            let m = match i {
                0 => self.top.update(ctx),
                1 => self.bottom.update(ctx),
                _ => self.tab.button.update(ctx),
            };
            if let Some(m) = m {
                self.child_message(m.who, &mut out);
            }
        }
        out
    }

    /// `aScrollBar::render`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let x = self.obj.global_x() as f32 + 2.0;
        for b in [&mut self.top, &mut self.bottom, &mut self.tab.button] {
            let y = b.obj.loc[0].y;
            b.move_to(x, y);
        }
        if self.obj.show {
            self.obj.draw_quad(ctx);
            self.top.render(ctx);
            self.bottom.render(ctx);
            if self.tab.button.obj.show {
                self.tab.render(ctx);
            }
        }
        let c = self.color;
        ctx.dl.outline_rect(self.obj.global_rect(), c, c);
    }
}
