// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Drawing of one icon: `ForceGroupIcon::render` (`code/mechicon.cpp:886-1022`), with `drawBar`
//! (`:1363-1381`), `renderUnitIcon` (`:1024-1064`), `PilotIcon::render` (`:1568-1603`).
//!
//! The order of the calls is the C++ order, so overlapping areas paint the same way.

use super::anim::{self, Frame};
use super::layout::Slot;
use super::{force_group_color, Art, Icon, PILOT_FRAME, PILOT_H, PILOT_W, SB_GREEN};
use crate::ctx::RenderCtx;
use crate::draw::{Blend, DrawList, GuiRect};
use crate::font::text::{self, TextAttrs};
use crate::textures::{TexId, TextureStore};
use crate::widgets::object::Object;

/// `ForceGroupIcon::render` for one icon in its slot.
pub(super) fn icon(ic: &Icon, slot: &Slot, art: &Art, ctx: &mut RenderCtx<'_>) {
    let dl = &mut *ctx.dl;
    let assets = ctx.assets;
    let u = &ic.unit;
    let sel = slot.selection;

    // the text area in the force group colour, then the pilot's name in capitals
    dl.fill_rect(slot.text_area, force_group_color(u.groups));
    if let Some(font) = assets.font(art.font) {
        let name = u.pilot.name.to_ascii_uppercase();
        let attrs = TextAttrs {
            word_wrap: false,
            ..TextAttrs::default()
        };
        let region = GuiRect::new(0, 0, dl.screen.0, dl.screen.1);
        // gosFontHandle->render(buffer, textArea.left + 1, pilotTextTop)
        text::draw(
            dl,
            font,
            &name,
            &attrs,
            region,
            (slot.text_area.left + 1, slot.pilot_text_top),
        );
    }

    health_bar(dl, slot.health_bar, u.bar_status);

    // the unit's icon, then the pilot's portrait and its frame
    dl.rect_quad(
        Some(ic.image),
        Blend::Alpha,
        rect_f(slot.icon_area),
        (0.0, 0.0, 1.0, 1.0),
        0xffff_ffff,
    );
    pilot_icon(
        dl,
        &assets.textures,
        art.pilot_atlas,
        super::portrait_index(u),
        slot.pilot_area,
    );
    dl.outline_rect(slot.pilot_rect, PILOT_FRAME, PILOT_FRAME);

    if u.can_jump {
        jump_jet(dl, &art.jump_jet, sel.left, sel.bottom);
    }
    if u.shutdown {
        dl.fill_rect(sel, 0xaa00_0000);
    }
    if u.selected {
        dl.outline_rect(sel, 0xffff_ffff, 0xffff_ffff);
    }
    if u.pilot.message_playing {
        // the message flash: yellow, then nothing, each quarter second (mechicon.cpp:1003-1009)
        let c = if ic.msg_time > 0.125 { 0 } else { 0xffff_ff00 };
        dl.outline_rect(sel, c, c);
    }
    if ic.death_time != 0.0 {
        match anim::death_frame(ic.death_time) {
            Frame::Dying { fill, outline } => {
                dl.fill_rect(sel, fill);
                if let Some(c) = outline {
                    dl.outline_rect(sel, c, c);
                }
            }
            Frame::Finished => dl.fill_rect(sel, anim::FINAL_COLOR),
        }
    }
}

fn rect_f(r: GuiRect) -> (f32, f32, f32, f32) {
    (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32)
}

/// `ForceGroupIcon::drawBar`: the health fill, then its outline (`code/mechicon.cpp:1363-1381`).
/// The outline's bottom and right edges are `SB_GREEN & 0x7f7f7f`, which has no alpha, so they
/// draw nothing, as in the original.
fn health_bar(dl: &mut DrawList, r: GuiRect, status: f32) {
    let len = (r.width() as f32 * status) as i32;
    dl.fill_rect(
        GuiRect::new(r.left, r.top, r.left + len, r.bottom),
        SB_GREEN,
    );
    dl.outline_rect(r, SB_GREEN, SB_GREEN & 0x007f_7f7f);
}

/// `PilotIcon::render`: the portrait cell `index` of the pilot atlas, drawn into `r`.
fn pilot_icon(dl: &mut DrawList, textures: &TextureStore, atlas: TexId, index: u32, r: GuiRect) {
    let Some(uv) = portrait_uv(textures.width(atlas), index) else {
        return;
    };
    dl.rect_quad(Some(atlas), Blend::Alpha, rect_f(r), uv, 0xffff_ffff);
}

/// The texture coordinates of portrait cell `index` in an atlas `width` texels wide: a grid of
/// `PILOT_W` by `PILOT_H` cells, `width / PILOT_W` to a row (`code/mechicon.cpp:1584-1594`). `None`
/// when the atlas is too narrow for one cell.
pub fn portrait_uv(width: u32, index: u32) -> Option<(f32, f32, f32, f32)> {
    let per_row = width / PILOT_W;
    if per_row == 0 {
        return None;
    }
    let (x, y) = (index % per_row, index / per_row);
    let (xl, yl) = ((x * PILOT_W) as f32, (y * PILOT_H) as f32);
    let w = width as f32;
    // the .1 texel nudge of the C++ UVs (`.1f/s_pilotTextureWidth`)
    let nudge = 0.1 / w;
    Some((
        xl / w + nudge,
        yl / w + nudge,
        (xl + PILOT_W as f32) / w + nudge,
        (yl + PILOT_H as f32) / w + nudge,
    ))
}

/// `jumpJetIcon->move(left, bottom); render()`: the jump jet's quad moved by the offset.
fn jump_jet(dl: &mut DrawList, icon: &Object, dx: i32, dy: i32) {
    let Some(tex) = icon.texture else { return };
    let mut quad = icon.loc;
    for v in &mut quad {
        v.x += dx as f32;
        v.y += dy as f32;
    }
    dl.quad(Some(tex), Blend::Alpha, quad);
}
