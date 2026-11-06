// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! CPU-side 2D draw list: the `gos_DrawQuads` / `gos_DrawLines` calls the GUI makes, recorded in
//! UI coordinates (the logical screen, 800x600 for the menus) and turned into GPU draws by
//! [`crate::render::UiRenderer`]. Recording on the CPU keeps every widget testable headless.

use crate::textures::TexId;

/// `gos_State_AlphaMode` values the GUI uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Blend {
    /// `gos_Alpha_AlphaInvAlpha`: `src * a + dst * (1 - a)`.
    #[default]
    Alpha,
    /// `gos_Alpha_OneOne`: additive.
    Additive,
    /// `gos_Alpha_OneZero`: opaque replace.
    Opaque,
}

/// One corner of a quad (`gos_VERTEX` without z/rhw/specular).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub u: f32,
    pub v: f32,
    /// `0xAARRGGBB`.
    pub argb: u32,
}

/// Integer rectangle in UI coordinates (`GUI_RECT`: right/bottom exclusive for fills).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct GuiRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl GuiRect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> GuiRect {
        GuiRect {
            left,
            top,
            right,
            bottom,
        }
    }
    pub fn width(&self) -> i32 {
        self.right - self.left
    }
    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.left <= x && x <= self.right && self.top <= y && y <= self.bottom
    }
    pub fn intersect(&self, o: &GuiRect) -> GuiRect {
        GuiRect {
            left: self.left.max(o.left),
            top: self.top.max(o.top),
            right: self.right.min(o.right).max(self.left.max(o.left)),
            bottom: self.bottom.min(o.bottom).max(self.top.max(o.top)),
        }
    }
}

/// A run of quads that share texture, blend mode and clip rectangle.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub texture: Option<TexId>,
    pub blend: Blend,
    /// Scissor in UI coordinates; `None` is the whole screen.
    pub clip: Option<GuiRect>,
    pub first_quad: u32,
    pub quad_count: u32,
}

/// Recorded draws for one frame.
#[derive(Clone, Debug, Default)]
pub struct DrawList {
    /// Four vertices per quad in `gos_DrawQuads` order: top-left, bottom-left, bottom-right,
    /// top-right.
    pub vertices: Vec<Vertex>,
    pub batches: Vec<Batch>,
    clip_stack: Vec<GuiRect>,
    /// The logical screen size (`Environment.screenWidth/Height`), used by `drawRect` clamping.
    pub screen: (i32, i32),
}

impl DrawList {
    pub fn new(screen_w: i32, screen_h: i32) -> DrawList {
        DrawList {
            screen: (screen_w, screen_h),
            ..DrawList::default()
        }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.batches.clear();
        self.clip_stack.clear();
    }

    pub fn quad_count(&self) -> usize {
        self.vertices.len() / 4
    }

    fn current_clip(&self) -> Option<GuiRect> {
        self.clip_stack.last().copied()
    }

    /// Restricts following draws to `r` (intersected with the current clip).
    pub fn push_clip(&mut self, r: GuiRect) {
        let r = match self.current_clip() {
            Some(c) => c.intersect(&r),
            None => r,
        };
        self.clip_stack.push(r);
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Starts the next quad in a fresh batch: the original's `gos_SetRenderState` calls between
    /// `gos_DrawQuads` make each pass its own draw, which the state-merging batches cannot see.
    /// The marker is a zero-length batch, skipped by the renderer and pruned when the next
    /// batch opens.
    pub fn flush_batch(&mut self) {
        self.batches.push(Batch {
            texture: None,
            blend: Blend::Opaque,
            clip: self.current_clip(),
            first_quad: self.quad_count() as u32,
            quad_count: 0,
        });
    }

    /// `gos_DrawQuads` with one quad.
    pub fn quad(&mut self, texture: Option<TexId>, blend: Blend, v: [Vertex; 4]) {
        let clip = self.current_clip();
        if let Some(c) = clip {
            if c.width() <= 0 || c.height() <= 0 {
                return;
            }
        }
        let quad_index = self.quad_count() as u32;
        // A zero-length batch is a flush marker: the next quad opens a fresh batch whatever its
        // state, then the marker (whose clip is the current one, so it draws nothing) is pruned.
        let marker = self
            .batches
            .last()
            .is_some_and(|b| b.quad_count == 0)
            .then(|| self.batches.pop())
            .flatten();
        let merge = |b: &Batch| b.texture == texture && b.blend == blend && b.clip == clip;
        match self.batches.last_mut() {
            Some(b) if marker.is_none() && merge(b) => {
                b.quad_count += 1;
            }
            _ => self.batches.push(Batch {
                texture,
                blend,
                clip,
                first_quad: quad_index,
                quad_count: 1,
            }),
        }
        self.vertices.extend_from_slice(&v);
    }

    /// An axis-aligned quad with one colour.
    pub fn rect_quad(
        &mut self,
        texture: Option<TexId>,
        blend: Blend,
        (x0, y0, x1, y1): (f32, f32, f32, f32),
        (u0, v0, u1, v1): (f32, f32, f32, f32),
        argb: u32,
    ) {
        let vx = |x, y, u, v| Vertex { x, y, u, v, argb };
        self.quad(
            texture,
            blend,
            [
                vx(x0, y0, u0, v0),
                vx(x0, y1, u0, v1),
                vx(x1, y1, u1, v1),
                vx(x1, y0, u1, v0),
            ],
        );
    }

    /// `drawRect`: a filled rectangle clamped to the screen, skipped when fully transparent.
    pub fn fill_rect(&mut self, r: GuiRect, argb: u32) {
        if argb & 0xff00_0000 == 0 {
            return;
        }
        let (sw, sh) = self.screen;
        let cx = |v: i32| v.clamp(0, sw);
        let cy = |v: i32| v.clamp(0, sh);
        let (l, t, rr, b) = (cx(r.left), cy(r.top), cx(r.right), cy(r.bottom));
        self.rect_quad(
            None,
            Blend::Alpha,
            (l as f32, t as f32, rr as f32, b as f32),
            (0.0, 0.0, 0.0, 0.0),
            argb,
        );
    }

    /// `drawEmptyRect`: one-pixel outline, top and left in `top_left`, right and bottom in
    /// `bottom_right` (the bottom line reaches one pixel further right, as the original's line
    /// list does).
    pub fn outline_rect(&mut self, r: GuiRect, top_left: u32, bottom_right: u32) {
        let (sw, sh) = self.screen;
        if (r.left < 0 && r.right < 0)
            || (r.right > sw && r.left > sw)
            || (r.top < 0 && r.bottom < 0)
            || (r.top > sh && r.bottom > sh)
        {
            return;
        }
        let cx = |v: i32| v.clamp(0, sw - 1) as f32;
        let cy = |v: i32| v.clamp(0, sh - 1) as f32;
        let (l, t, rr, b) = (cx(r.left), cy(r.top), cx(r.right), cy(r.bottom));
        let z = (0.0, 0.0, 0.0, 0.0);
        // top and left
        self.rect_quad(None, Blend::Alpha, (l, t, rr, t + 1.0), z, top_left);
        self.rect_quad(None, Blend::Alpha, (l, t, l + 1.0, b), z, top_left);
        // right and bottom
        self.rect_quad(None, Blend::Alpha, (rr, t, rr + 1.0, b), z, bottom_right);
        self.rect_quad(
            None,
            Blend::Alpha,
            (l, b, rr + 1.0, b + 1.0),
            z,
            bottom_right,
        );
    }

    /// A one-pixel vertical line from `(x, y0)` to `(x, y1)` (`gos_DrawLines` for the edit cursor).
    pub fn vline(&mut self, x: f32, y0: f32, y1: f32, argb: u32) {
        self.rect_quad(
            None,
            Blend::Alpha,
            (x, y0, x + 1.0, y1),
            (0.0, 0.0, 0.0, 0.0),
            argb,
        );
    }

    /// Bounding box of everything drawn with colour alpha > 0, in UI units (for tests).
    pub fn bounds(&self) -> Option<(f32, f32, f32, f32)> {
        let mut it = self.vertices.iter().filter(|v| v.argb >> 24 != 0);
        let first = it.next()?;
        let mut b = (first.x, first.y, first.x, first.y);
        for v in it {
            b.0 = b.0.min(v.x);
            b.1 = b.1.min(v.y);
            b.2 = b.2.max(v.x);
            b.3 = b.3.max(v.y);
        }
        Some(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_merge_and_split() {
        let mut dl = DrawList::new(800, 600);
        dl.fill_rect(GuiRect::new(0, 0, 10, 10), 0xff00_0000);
        dl.fill_rect(GuiRect::new(-5, 590, 900, 700), 0x80ff_ffff);
        dl.fill_rect(GuiRect::new(0, 0, 10, 10), 0x00ff_ffff); // invisible: skipped
        assert_eq!(dl.quad_count(), 2);
        assert_eq!(dl.batches.len(), 1);
        assert_eq!(dl.vertices[4].x, 0.0);
        assert_eq!(
            dl.vertices[6],
            Vertex {
                x: 800.0,
                y: 600.0,
                u: 0.0,
                v: 0.0,
                argb: 0x80ff_ffff
            }
        );
        dl.push_clip(GuiRect::new(0, 0, 100, 100));
        dl.fill_rect(GuiRect::new(0, 0, 10, 10), 0xff00_0000);
        dl.pop_clip();
        assert_eq!(dl.batches.len(), 2);
        assert_eq!(dl.batches[1].clip, Some(GuiRect::new(0, 0, 100, 100)));
        dl.outline_rect(GuiRect::new(10, 10, 20, 20), 0xffff_ffff, 0xff00_0000);
        assert_eq!(dl.quad_count(), 7);
    }
}
