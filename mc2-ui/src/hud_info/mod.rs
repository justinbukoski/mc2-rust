// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! In-mission HUD windows of lane U3: the selected unit's details panel (`code/infowindow.cpp`)
//! and the chat/message window (`code/chatwindow.cpp`).
//!
//! * [`info::InfoWindow`]: name and health bar, weapon groups with working/total counts and shots
//!   left, equipment, pilot name, rank and skill meters, in a box that scrolls.
//! * [`chat::ChatWindow`]: the message list (collapsed and expanded), the text entry and the
//!   team-only and help toggles.
//!
//! Inputs are plain data ([`info::UnitInfo`], [`chat::ChatMessage`]) that the app fills from the
//! sim; nothing here calls the sim. Outputs are values ([`chat::ChatIntent`]) and [`HudOp`] lists
//! that [`draw_ops`] records into a [`crate::draw::DrawList`]. The info window draws its unit icon,
//! pilot portrait and skill labels ([`icon`]); `chat::UNPORTED` lists what the chat window leaves out.

pub mod chat;
pub mod icon;
pub mod info;

use crate::ctx::{RenderCtx, UiAssets};
use crate::draw::{Blend, GuiRect, Vertex};
use crate::font::FontRef;
use crate::textures::TexId;
use crate::widgets::basic::{font_height, font_render, font_text_height, font_width};

/// Text measurement and strings, as `aFont` and `cLoadString` provide them. [`UiAssets`] is the
/// real implementation; tests substitute fixed metrics.
pub trait Metrics {
    /// `aFont::height()`: the height of one line.
    fn line_height(&self, font: FontRef) -> i32;
    /// `aFont::width(s)`.
    fn text_width(&self, font: FontRef, s: &str) -> i32;
    /// `aFont::height(s, areaWidth)`: the height `s` needs wrapped to `width`.
    fn wrapped_height(&self, font: FontRef, s: &str, width: i32) -> i32;
    /// `cLoadString(id)`.
    fn string(&self, id: u32) -> &str;
}

impl Metrics for UiAssets {
    fn line_height(&self, font: FontRef) -> i32 {
        font_height(self, font)
    }
    fn text_width(&self, font: FontRef, s: &str) -> i32 {
        font_width(self, font, s)
    }
    fn wrapped_height(&self, font: FontRef, s: &str, width: i32) -> i32 {
        font_text_height(self, font, s, width)
    }
    fn string(&self, id: u32) -> &str {
        UiAssets::string(self, id)
    }
}

/// One recorded drawing step of a HUD window. Coordinates are UI coordinates (the logical
/// screen), and every value is what the C++ passes to `drawRect`, `drawEmptyRect`,
/// `gos_DrawQuads`, `gos_DrawLines`, `aFont::render` or the list clip.
#[derive(Clone, Debug, PartialEq)]
pub enum HudOp {
    /// `drawRect(rect, argb)`: a filled rectangle, right and bottom exclusive.
    Fill {
        rect: GuiRect,
        argb: u32,
    },
    /// `drawEmptyRect(rect, topLeft, bottomRight)`: a one-pixel frame.
    Outline {
        rect: GuiRect,
        top_left: u32,
        bottom_right: u32,
    },
    /// A flat-coloured quad (`gos_DrawQuads` with no texture).
    Quad {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        argb: u32,
    },
    /// `drawDivider`: a horizontal line from `x0` to `x1` at `y`.
    Divider {
        x0: i32,
        x1: i32,
        y: f32,
        argb: u32,
    },
    /// `aFont::render(text, x, y, w, h, colour, 0, align)`.
    Text {
        text: String,
        font: FontRef,
        x: i32,
        y: f32,
        w: i32,
        h: i32,
        colour: u32,
        align: i32,
    },
    /// A textured quad (`gos_DrawQuads` with a texture bound): unit icon, pilot portrait or skill
    /// label art. Corners: top-left, bottom-left, bottom-right, top-right.
    Tex {
        tex: TexId,
        quad: [Vertex; 4],
    },
    /// Restricts the following ops to `rect` (the list box clip).
    PushClip(GuiRect),
    PopClip,
}

/// Records `ops` into `ctx.dl`, in order.
pub fn draw_ops(ctx: &mut RenderCtx<'_>, ops: &[HudOp]) {
    for op in ops {
        match op {
            HudOp::Fill { rect, argb } => ctx.dl.fill_rect(*rect, *argb),
            HudOp::Outline {
                rect,
                top_left,
                bottom_right,
            } => ctx.dl.outline_rect(*rect, *top_left, *bottom_right),
            HudOp::Quad {
                x0,
                y0,
                x1,
                y1,
                argb,
            } => {
                let v = |x: f32, y: f32| Vertex {
                    x,
                    y,
                    u: 0.0,
                    v: 0.0,
                    argb: *argb,
                };
                // Top-left, bottom-left, bottom-right, top-right: the DrawList quad order.
                let quad = [v(*x0, *y0), v(*x0, *y1), v(*x1, *y1), v(*x1, *y0)];
                ctx.dl.quad(None, Blend::Alpha, quad);
            }
            HudOp::Divider { x0, x1, y, argb } => {
                let y = *y as i32;
                ctx.dl.fill_rect(GuiRect::new(*x0, y, *x1, y + 1), *argb);
            }
            HudOp::Text {
                text,
                font,
                x,
                y,
                w,
                h,
                colour,
                align,
            } => font_render(
                ctx, *font, text, *x, *y as i32, *w, *h, *colour, false, *align,
            ),
            HudOp::Tex { tex, quad } => ctx.dl.quad(Some(*tex), Blend::Alpha, *quad),
            HudOp::PushClip(rect) => ctx.dl.push_clip(*rect),
            HudOp::PopClip => ctx.dl.pop_clip(),
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::strings::StringTable;

    /// Fixed metrics: 10-pixel lines, 7 pixels a character. Text wraps at `width` pixels, so a
    /// text needs `ceil(chars * 7 / width)` lines (at least one). Strings are the built-in table.
    pub struct FixedMetrics;

    pub const LINE: i32 = 10;
    pub const CHAR_W: i32 = 7;

    impl Metrics for FixedMetrics {
        fn line_height(&self, _: FontRef) -> i32 {
            LINE
        }
        fn text_width(&self, _: FontRef, s: &str) -> i32 {
            s.chars().count() as i32 * CHAR_W
        }
        fn wrapped_height(&self, _: FontRef, s: &str, width: i32) -> i32 {
            let px = s.chars().count() as i32 * CHAR_W;
            if px == 0 {
                return 0;
            }
            ((px + width - 1) / width).max(1) * LINE
        }
        fn string(&self, id: u32) -> &str {
            StringTable::builtin().load(id)
        }
    }
}
