// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The unit and pilot pictures the results screens draw: `ForceGroupIcon::renderUnitIcon`
//! (`code/mechicon.cpp:1024-1064`) for the salvage rows and the kill markers, and
//! `PilotIcon::render` (`code/mechicon.cpp:1568-1603`) for the pilot portraits.
//!
//! The atlases are the unit bar's: `mcui_med4.tga` (mechs), `mcui_med5.tga` (vehicles) and
//! `mcui_med2.tga` (pilots).

use crate::ctx::{Loader, RenderCtx};
use crate::draw::Blend;
use crate::textures::TexId;
use crate::unitbar::{ICON_H, ICON_W, PILOT_H, PILOT_W};

/// `PilotIcon::DEAD_PILOT_INDEX`: the skull cell of the portrait atlas.
pub const DEAD_PILOT_INDEX: u32 = 27;

/// The icon atlases the results screens draw from.
#[derive(Clone, Copy, Debug, Default)]
pub struct IconArt {
    mech: Option<TexId>,
    vehicle: Option<TexId>,
    pilot: Option<TexId>,
}

impl IconArt {
    pub fn load(ld: &mut Loader<'_>) -> IconArt {
        IconArt {
            mech: ld.art("mcui_med4.tga"),
            vehicle: ld.art("mcui_med5.tga"),
            pilot: ld.art("mcui_med2.tga"),
        }
    }

    /// The atlas the unit icons of mechs (or vehicles) are cells of.
    pub fn atlas(&self, vehicle: bool) -> Option<TexId> {
        if vehicle {
            self.vehicle
        } else {
            self.mech
        }
    }

    /// `renderUnitIcon` of a picture already composed into its own texture (`image`, `ICON_W` by
    /// `ICON_H`): the whole picture stretched over `rect`.
    pub fn image(&self, ctx: &mut RenderCtx<'_>, image: TexId, rect: (f32, f32, f32, f32)) {
        ctx.dl.rect_quad(
            Some(image),
            Blend::Alpha,
            rect,
            (0.0, 0.0, 1.0, 1.0),
            0xffff_ffff,
        );
    }

    /// `renderUnitIcon`: the front view of unit icon `picture` (a mech's `getIconPictureIndex`, or
    /// a vehicle's when `vehicle`) stretched over `(left, top, right, bottom)`.
    pub fn unit(
        &self,
        ctx: &mut RenderCtx<'_>,
        vehicle: bool,
        picture: i32,
        rect: (f32, f32, f32, f32),
    ) {
        let Some(atlas) = (if vehicle { self.vehicle } else { self.mech }) else {
            return;
        };
        let Some(img) = ctx.assets.textures.get(atlas) else {
            return;
        };
        let (w, h) = (img.width as f32, img.height as f32);
        let col = picture.max(0) as u32;
        if (col + 1) * ICON_W > img.width || ICON_H > img.height {
            return;
        }
        // the C++ nudges every UV by `.1 / 256`
        let bias = 0.1 / 256.0;
        let u0 = (col * ICON_W) as f32 / w + bias;
        let uv = (u0, bias, u0 + ICON_W as f32 / w, ICON_H as f32 / h + bias);
        ctx.dl
            .rect_quad(Some(atlas), Blend::Alpha, rect, uv, 0xffff_ffff);
    }

    /// `PilotIcon::render`: portrait cell `index` of the pilot atlas over `rect`.
    pub fn pilot(&self, ctx: &mut RenderCtx<'_>, index: u32, rect: (f32, f32, f32, f32)) {
        let Some(atlas) = self.pilot else { return };
        let width = ctx.assets.textures.width(atlas);
        let per_row = width / PILOT_W;
        if per_row == 0 {
            return;
        }
        let (x, y) = ((index % per_row) * PILOT_W, (index / per_row) * PILOT_H);
        let w = width as f32;
        let bias = 0.1 / w;
        let uv = (
            x as f32 / w + bias,
            y as f32 / w + bias,
            (x + PILOT_W) as f32 / w + bias,
            (y + PILOT_H) as f32 / w + bias,
        );
        ctx.dl
            .rect_quad(Some(atlas), Blend::Alpha, rect, uv, 0xffff_ffff);
    }
}
