// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The info window's own unit icon (`InfoWindow::setUnit`, infowindow.cpp:311-348): a `MechIcon`
//! or `VehicleIcon` with `setDrawBack(true)`, so a mech draws a front cell and a rear cell, and the
//! pilot's portrait (`renderPilotIcon`, mechicon.cpp:1108-1111).
//!
//! The front cell is the unit's art recoloured per damage state with the torsos' rear armour kept
//! apart (`MechIcon::update` with `bDrawBack`, mechicon.cpp:572-778). The rear cell is the second
//! row of the atlas masked to the rear shades (`MechIcon::setDrawBack`, mechicon.cpp:300-396).

use super::HudOp;
use crate::draw::Vertex;
use crate::textures::{TexId, TextureStore};
use crate::unitbar::damage::{self, DAMAGE_COLORS};
use crate::unitbar::{
    portrait_index, portrait_uv, Armor, Loc, MechArmor, UnitBarUnit, ICON_H, ICON_W,
};

/// The atlases the window draws icons and portraits from.
#[derive(Clone, Copy, Debug)]
pub struct IconAtlases {
    pub mech: TexId,
    pub vehicle: TexId,
    pub pilot: TexId,
}

/// One icon, built for one unit.
#[derive(Clone, Debug)]
pub struct InfoIcon {
    unit_id: u32,
    vehicle: bool,
    front: TexId,
    back: Option<TexId>,
    front_damage: [u8; 8],
    back_state: u8,
    portrait_uv: Option<(f32, f32, f32, f32)>,
    atlases: IconAtlases,
}

/// A mech's state from a ratio (`mechicon.cpp:589-626`), with the thresholds of the rear torsos.
fn state(t: f32, one: f32) -> u8 {
    if t < 0.0 {
        3
    } else if t < 0.3 {
        2
    } else if t < one {
        1
    } else {
        0
    }
}

fn locstate(l: &Loc, one: f32) -> u8 {
    state(if l.destroyed { -1.0 } else { l.ratio() }, one)
}

/// `MechIcon::update` front states with `bDrawBack` set: torsos use only the front armour.
fn front_states(a: &MechArmor) -> [u8; 8] {
    [
        locstate(&a.lleg, 0.6),
        locstate(&a.rleg, 0.6),
        locstate(&a.larm, 0.6),
        locstate(&a.rarm, 0.6),
        locstate(&a.rtorso, 0.6),
        locstate(&a.ltorso, 0.6),
        locstate(&a.ctorso, 0.6),
        locstate(&a.head, 0.6),
    ]
}

/// The state of the rear cell's one recoloured location (index 7, `mechicon.cpp:764-776`): the
/// rear torsos' states weighted by their maximum armour, plus a half, truncated.
fn back_state(a: &MechArmor) -> u8 {
    let r4 = f32::from(locstate(&a.rrtorso, 0.4));
    let r5 = f32::from(locstate(&a.rltorso, 0.5));
    let r6 = f32::from(locstate(&a.rctorso, 0.6));
    let (mut lratio, mut rratio) = (1.0f32, 1.0f32);
    if a.rctorso.max != 0.0 {
        lratio = a.rltorso.max / a.rctorso.max;
        rratio = a.rrtorso.max / a.rctorso.max;
    }
    // the C++ stores a `float` into a `char` slot: truncation
    (0.5 + (rratio * r4 + lratio * r5 + r6) / (lratio + rratio + 1.0)) as u8
}

/// The rear cell: row 1 of the atlas, keeping only the pixels of the rear locations 4 to 7
/// (red channel within `[base + 4, base + 7]` of any shade of state 0) and the grey 0xff505050.
fn back_cell(textures: &TextureStore, atlas: TexId, col: u32) -> Vec<u8> {
    let mut out = vec![0u8; (ICON_W * ICON_H * 4) as usize];
    let Some(img) = textures.get(atlas) else {
        return out;
    };
    let (x0, y0) = (col * ICON_W, ICON_H);
    if x0 + ICON_W > img.width || y0 + ICON_H > img.height {
        return out;
    }
    for y in 0..ICON_H {
        for x in 0..ICON_W {
            let src = (((y0 + y) * img.width + x0 + x) * 4) as usize;
            let px = &img.rgba[src..src + 4];
            let argb = damage::argb(px);
            let keep = argb == 0xff50_5050
                || DAMAGE_COLORS[0].iter().any(|c| {
                    let min = (c >> 16) + 4;
                    let red = argb >> 16;
                    red >= min && red <= min + 3
                });
            if keep {
                let dst = ((y * ICON_W + x) * 4) as usize;
                out[dst..dst + 4].copy_from_slice(px);
            }
        }
    }
    out
}

impl InfoIcon {
    /// Builds the icon for `unit` (`setUnit`'s `new MechIcon` / `new VehicleIcon` and `init`).
    pub fn new(textures: &mut TextureStore, atlases: IconAtlases, unit: &UnitBarUnit) -> InfoIcon {
        let vehicle = unit.is_vehicle();
        let atlas = if vehicle {
            atlases.vehicle
        } else {
            atlases.mech
        };
        let pixels = damage::copy_cell(textures, atlas, unit.icon_picture);
        let front = textures.insert("info/icon-front", ICON_W, ICON_H, pixels);
        let back = (!vehicle).then(|| {
            let px = back_cell(textures, atlas, unit.icon_picture);
            textures.insert("info/icon-back", ICON_W, ICON_H, px)
        });
        let mut icon = InfoIcon {
            unit_id: unit.id,
            vehicle,
            front,
            back,
            front_damage: [0; 8],
            back_state: 0,
            portrait_uv: None,
            atlases,
        };
        icon.update(textures, unit);
        icon
    }

    pub fn unit_id(&self) -> u32 {
        self.unit_id
    }

    /// `MechIcon::update` / `VehicleIcon::update`: recolours where the damage states changed.
    pub fn update(&mut self, textures: &mut TextureStore, unit: &UnitBarUnit) {
        self.portrait_uv = portrait_uv(textures.width(self.atlases.pilot), portrait_index(unit));
        match &unit.armor {
            Armor::Mech(a) => {
                let new = front_states(a);
                if damage::differs(&self.front_damage, &new, 8) {
                    if let Some(px) = textures.pixels_mut(self.front) {
                        damage::apply(px, &mut self.front_damage, &new, 8);
                    }
                }
                let b = back_state(a);
                if b != self.back_state {
                    if let Some(px) = self.back.and_then(|t| textures.pixels_mut(t)) {
                        damage::recolor(px, 7, self.back_state, b);
                    }
                    self.back_state = b;
                }
            }
            Armor::Vehicle(v) => {
                let new = damage::vehicle_states(v);
                if damage::differs(&self.front_damage, &new, 5) {
                    if let Some(px) = textures.pixels_mut(self.front) {
                        damage::apply(px, &mut self.front_damage, &new, 5);
                    }
                }
            }
        }
    }

    /// `renderUnitIcon` and `renderUnitIconBack` into the given rectangles.
    pub fn unit_ops(&self, front: [f32; 4], back: [f32; 4]) -> Vec<HudOp> {
        let mut ops = vec![HudOp::rect_tex(self.front, front, (0.0, 0.0, 1.0, 1.0))];
        if let Some(b) = self.back {
            ops.push(HudOp::rect_tex(b, back, (0.0, 0.0, 1.0, 1.0)));
        }
        ops
    }

    /// `renderPilotIcon`: the portrait cell of the pilot atlas, in the rectangle.
    pub fn pilot_op(&self, rect: [f32; 4]) -> Option<HudOp> {
        Some(HudOp::rect_tex(self.atlases.pilot, rect, self.portrait_uv?))
    }

    pub fn is_vehicle(&self) -> bool {
        self.vehicle
    }
}

impl HudOp {
    /// A textured rectangle with the given UVs.
    pub fn rect_tex(tex: TexId, r: [f32; 4], (u0, v0, u1, v1): (f32, f32, f32, f32)) -> HudOp {
        let v = |x, y, u, v| Vertex {
            x,
            y,
            u,
            v,
            argb: 0xffff_ffff,
        };
        HudOp::Tex {
            tex,
            quad: [
                v(r[0], r[1], u0, v0),
                v(r[0], r[3], u0, v1),
                v(r[2], r[3], u1, v1),
                v(r[2], r[1], u1, v0),
            ],
        }
    }
}
