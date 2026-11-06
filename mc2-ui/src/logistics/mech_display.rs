// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `LogisticsMechDisplay` (`code/logisticsmechdisplay.cpp`): a mech's name, chassis, weight, range,
//! armour, speed and jump range, with the three attribute meters, from `mcl_mechinfo.fit`, and the
//! mech's loadout list (`componentListBox`, rect 0) and its 3D picture (`mechCamera`). The pilot
//! ready room shows it for the mech a pilot is about to join.
//!
//! [`MechStats`] is the readout both this display and the mech bay show.

use super::attribute_meter::AttributeMeter;
use super::format_weight_class;
use super::loadout::Loadout;
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::preview::{
    draw_preview, preview_texture, rect_size, shape_name, CameraActivation, PreviewKind,
    PreviewRequest,
};
use crate::screen::{LogisticsScreen, Prefixes};
use mc2_campaign::{Logistics, MechKey, VariantKey};

/// `MAX_ARMOR_RANGE`, `MAX_SPEED_RANGE`, `MAX_JUMP_RANGE` (`attributemeter.h`).
pub const MAX_ARMOR_RANGE: f32 = 432.0;
pub const MAX_SPEED_RANGE: f32 = 35.0;
pub const MAX_JUMP_RANGE: f32 = 125.0;
/// The colours of the range bands (`getOptimalRangeString`: short, medium, long).
const RANGE_COLOURS: [u32; 3] = [0xff6e_7c00, 0xff00_5392, 0xffa2_1600];
/// `IDS_HOTKEY1`: the range names are `IDS_HOTKEY1 + band`.
const IDS_HOTKEY1: u32 = 43200;
/// `IDS_MB_MECH_WEIGHT`.
const IDS_MB_MECH_WEIGHT: u32 = 27132;

/// What a mech's statistics panel shows.
#[derive(Clone, Debug, PartialEq)]
pub struct MechStats {
    pub chassis_name: String,
    pub variant_name: String,
    /// "38 Tons Light".
    pub weight_text: String,
    /// The chassis' `maxWeight` in tons (`getMaxWeight`).
    pub max_weight: f32,
    pub range_text: String,
    pub range_colour: u32,
    pub armor: i32,
    pub display_speed: i32,
    /// Jump range in metres: the jump range times 25.
    pub jump: i32,
    /// The meters' fractions of their full scales.
    pub armor_meter: f32,
    pub speed_meter: f32,
    pub jump_meter: f32,
    /// The model the 3D preview shows: the chassis file's shape, and its scale (`setScale`).
    pub model_shape: String,
    pub model_scale: f32,
}

impl MechStats {
    /// The statistics of `mech` (`getArmor`, `getDisplaySpeed`, `getJumpRange`, ...).
    pub fn of(assets: &UiAssets, lg: &Logistics, mech: MechKey) -> Option<MechStats> {
        Self::of_variant(assets, lg, lg.mech(mech)?.variant)
    }

    /// The statistics of a design (a mech for sale has no mech of its own yet).
    pub fn of_variant(assets: &UiAssets, lg: &Logistics, variant: VariantKey) -> Option<MechStats> {
        let v = lg.variant(variant)?;
        let chassis = lg.chassis_of(variant)?;
        let ctx = lg.ctx_for(v);
        let class = assets.string(chassis.weight_class().string_id());
        let band = v.optimal_range_band(&ctx).min(2);
        let armor = v.armor(&ctx);
        let jump = v.jump_range(&ctx) * 25;
        Some(MechStats {
            chassis_name: assets.string(chassis.chassis_name_id as u32).to_string(),
            variant_name: v.name.clone(),
            weight_text: format_weight_class(
                assets.string(IDS_MB_MECH_WEIGHT),
                chassis.max_weight,
                class,
            ),
            max_weight: chassis.max_weight,
            range_text: assets.string(IDS_HOTKEY1 + band as u32).to_string(),
            range_colour: RANGE_COLOURS[band],
            armor,
            display_speed: v.display_speed(&ctx),
            jump,
            armor_meter: armor as f32 / MAX_ARMOR_RANGE,
            speed_meter: v.speed(&ctx) as f32 / MAX_SPEED_RANGE,
            jump_meter: jump as f32 / MAX_JUMP_RANGE,
            model_shape: shape_name(&chassis.file_name),
            model_scale: chassis.scale,
        })
    }

    /// The preview of this mech in a panel `rect` (`(left, top, right, bottom)`): `mechCamera`
    /// shows the chassis in the player's paint (`paint`).
    pub fn model_request(&self, rect: (f32, f32, f32, f32), paint: [u32; 3]) -> PreviewRequest {
        let (width, height) = rect_size(rect);
        PreviewRequest {
            shape: self.model_shape.clone(),
            paint,
            scale: self.model_scale,
            yaw: 0.0,
            width,
            height,
            building: false,
            kind: PreviewKind::Mech,
            activated: false,
        }
    }
}

pub struct MechDisplay {
    /// The layout (`mcl_mechinfo.fit`).
    pub base: LogisticsScreen,
    /// `attributeMeters[3]`: armour, speed, jump.
    pub meters: [AttributeMeter; 3],
    /// `componentListBox`: the loadout of `cur`.
    pub loadout: Loadout,
    /// `pCurMech`.
    pub cur: Option<MechKey>,
    /// The design shown for a mech that is not in the inventory ([`MechDisplay::set_variant`]).
    pub cur_variant: Option<VariantKey>,
    /// `mechCamera`'s panel: `(left, top, right, bottom)` (`rects[1]`, `rects[2]`).
    pub preview_rect: (f32, f32, f32, f32),
    /// The picture of `cur`, once it is asked for.
    pub preview: Option<PreviewRequest>,
    /// `mechCamera`'s activation state: its first mech is lit by the fit light, later ones by the activation daylight.
    pub activation: CameraActivation,
}

impl MechDisplay {
    /// `LogisticsMechDisplay::init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<MechDisplay, String> {
        let base = LogisticsScreen::load(
            ld,
            "mcl_mechinfo",
            Prefixes {
                statics: Some("Static"),
                texts: Some("Text"),
                rects: Some("Rect"),
                buttons: Some("Button"),
                ..Prefixes::default()
            },
        )?;
        // `componentListBox.init(rects[0])`
        let rect = base
            .rect(0)
            .ok_or("mcl_mechinfo.fit has no loadout rectangle")?;
        let (x, y, w, h) = (
            rect.obj.left(),
            rect.obj.top(),
            rect.obj.width(),
            rect.obj.height(),
        );
        let loadout = Loadout::new(ld, x, y, w, h);
        // `mechCamera.init(rects[1].left(), rects[2].top(), rects[2].left(), rects[2].bottom())`
        let (r1, r2) = (
            base.rect(1)
                .ok_or("mcl_mechinfo.fit has no camera rectangle")?,
            base.rect(2)
                .ok_or("mcl_mechinfo.fit has no camera rectangle")?,
        );
        let preview_rect = (r1.obj.left(), r2.obj.top(), r2.obj.left(), r2.obj.bottom());
        let mut file = ld.art_fit("mcl_mechinfo")?;
        let meters = [
            AttributeMeter::from_fit(&mut file, "AttributeMeter0"),
            AttributeMeter::from_fit(&mut file, "AttributeMeter1"),
            AttributeMeter::from_fit(&mut file, "AttributeMeter2"),
        ];
        Ok(MechDisplay {
            base,
            meters,
            loadout,
            cur: None,
            cur_variant: None,
            preview_rect,
            preview: None,
            activation: CameraActivation::default(),
        })
    }

    fn set_text(&mut self, i: usize, s: &str) {
        if let Some(t) = self.base.texts.get_mut(i) {
            t.set_text(s);
        }
    }

    /// `setMech`: shows `mech`'s statistics (or clears them).
    pub fn set_mech(&mut self, assets: &UiAssets, lg: &Logistics, mech: Option<MechKey>) {
        if mech == self.cur && self.cur_variant.is_none() {
            return;
        }
        self.cur = mech;
        self.cur_variant = None;
        let variant = mech.and_then(|m| lg.mech(m)).map(|m| m.variant);
        self.show(assets, lg, variant);
    }

    /// `setMech` for a design the player does not own yet (the purchase screen's mechs for sale).
    pub fn set_variant(&mut self, assets: &UiAssets, lg: &Logistics, variant: Option<VariantKey>) {
        if variant == self.cur_variant && self.cur.is_none() {
            return;
        }
        self.cur = None;
        self.cur_variant = variant;
        self.show(assets, lg, variant);
    }

    fn show(&mut self, assets: &UiAssets, lg: &Logistics, variant: Option<VariantKey>) {
        self.loadout.set_mech(assets, lg, variant);
        let stats = variant.and_then(|v| MechStats::of_variant(assets, lg, v));
        let activation = self.activation;
        self.preview = stats
            .as_ref()
            .map(|s| activation.set(s.model_request(self.preview_rect, assets.player_paint)));
        let Some(s) = stats else {
            for i in 0..6 {
                self.set_text(i, "");
            }
            for m in &mut self.meters {
                m.set_value(0.0);
            }
            return;
        };
        self.set_text(0, &s.chassis_name);
        self.set_text(1, &s.variant_name);
        self.set_text(3, &s.weight_text);
        self.set_text(2, &s.range_text);
        if let Some(t) = self.base.texts.get_mut(2) {
            t.set_color(s.range_colour);
        }
        self.set_text(4, &s.armor.to_string());
        self.set_text(5, &s.display_speed.to_string());
        self.set_text(6, &s.jump.to_string());
        self.meters[0].set_value(s.armor_meter);
        self.meters[1].set_value(s.speed_meter);
        self.meters[2].set_value(s.jump_meter);
    }

    /// Clears the statistics (the C++ `setMech(NULL)`, without the assets the text needs).
    pub fn clear(&mut self) {
        self.cur = None;
        self.cur_variant = None;
        self.preview = None;
        self.loadout.clear();
        for i in 0..6 {
            self.set_text(i, "");
        }
        for m in &mut self.meters {
            m.set_value(0.0);
        }
    }

    /// `update`: the layout's own buttons (none are used here).
    pub fn update_display(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.loadout.update(ctx);
        self.base.update(ctx);
    }

    /// `render`: the loadout, the meters, then the layout.
    pub fn render_display(&mut self, ctx: &mut RenderCtx<'_>) {
        self.loadout.render(ctx);
        // `mechCamera.update()` with a model: `active`, so the next `setMech` relights it
        self.activation.update(self.preview.is_some());
        // accepted option colours repaint the unchanged selection
        if let Some(r) = self.preview.as_mut() {
            r.paint = ctx.assets.player_paint;
        }
        if let Some(tex) = self
            .preview
            .as_ref()
            .and_then(|r| preview_texture(ctx.assets, r))
        {
            draw_preview(ctx.dl, tex, self.preview_rect);
        }
        for m in &self.meters {
            m.render(ctx, 0, 0);
        }
        self.base.render(ctx);
    }
}
