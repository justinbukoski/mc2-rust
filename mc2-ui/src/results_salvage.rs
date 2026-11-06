// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The salvage screen's rows and its selected-mech panel (`code/salvagemecharea.cpp`):
//! `SalvageListItem` (check box, unit icon, chassis and variant names, tonnage and salvage price,
//! the weight and C-Bills icons, the box outline) and `SalvageMechArea` (the mech's name, weight,
//! armour, speed, jump and range, the three attribute meters, the loadout list and the 3D view).

use crate::anim::Animation;
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::logistics::attribute_meter::AttributeMeter;
use crate::logistics::loadout::Loadout;
use crate::logistics::mech_display::MechStats;
use crate::preview::{draw_preview, preview_texture, CameraActivation, PreviewRequest};
use crate::results_icons::IconArt;
use crate::screen::{LogisticsScreen, Prefixes};
use crate::textures::{TexId, TextureStore};
use crate::unitbar::{damage, Loc, MechArmor, ICON_H, ICON_W};
use crate::widgets::{AnimButton, ItemState, ListItem, Object, Text};
use mc2_campaign::logistics::SalvageOffer;
use mc2_campaign::{Logistics, VariantKey};
use std::any::Any;

/// `SalvageListItem::render`: a disabled row's colour.
pub const ROW_DISABLED: u32 = 0xff37_3737;
/// `SalvageListItem::SalvageListItem`: the row's top, inside the list.
const ROW_TOP: f32 = 2.0;
/// `SalvageMechScreen`'s `setSpaceBetweenItems(5)` and `setTopSkip(2)`.
pub const ROW_SKIP: i32 = 5;
pub const ROW_TOP_SKIP: i32 = 2;

/// What every row is cloned from (`SalvageListItem::init`): read once from the layout.
#[derive(Clone)]
pub struct RowTemplate {
    check: AnimButton,
    mech_name: Text,
    variant_name: Text,
    weight: Text,
    cost: Text,
    cbills_icon: Object,
    weight_icon: Object,
    width: f32,
    height: f32,
    /// `iconRect`: the C++ reads `MechEntryIcon`'s `YLocation` into `iconRect.right`, so `top`
    /// stays 0 (a static): the icon is drawn from `top + 5` to `bottom = height` of the box.
    icon_left: f32,
    icon_right: f32,
    icon_bottom: f32,
    normal: Animation,
    pressed: Animation,
    highlight: Animation,
    art: IconArt,
}

impl RowTemplate {
    pub fn load(ld: &mut Loader<'_>, file: &mut mc2_formats::ini::FitIni) -> RowTemplate {
        let check = AnimButton::from_fit(ld, file, "SalvageAreaCheckBoxButton");
        let cbills_icon = Object::from_fit(ld, file, "CBillsIcon");
        let weight_icon = Object::from_fit(ld, file, "WeightIcon");
        let mech_name = Text::from_fit(ld, file, "MechNameText");
        let variant_name = Text::from_fit(ld, file, "VariantNameText");
        let cost = Text::from_fit(ld, file, "CBillsText");
        let weight = Text::from_fit(ld, file, "WeightText");
        let _ = file.seek_block("SalvageAreaMechEntryBox");
        let width = file.long_or("Width", 0) as f32;
        let _ = file.seek_block("SalvageAreaMechEntryBox");
        let normal = Animation::from_fit(file, "Normal");
        let pressed = Animation::from_fit(file, "Pressed");
        let highlight = Animation::from_fit(file, "Highlight");
        let height = file.long_or("Height", 0) as f32;
        let _ = file.seek_block("MechEntryIcon");
        let icon_left = file.long_or("XLocation", 0) as f32;
        let icon_width = file.long_or("Width", 0) as f32;
        RowTemplate {
            check,
            mech_name,
            variant_name,
            weight,
            cost,
            cbills_icon,
            weight_icon,
            width,
            height,
            icon_left,
            icon_right: icon_left + icon_width,
            icon_bottom: file.long_or("Height", 0) as f32,
            normal,
            pressed,
            highlight,
            art: IconArt::load(ld),
        }
    }
}

/// One offer of the salvage list.
pub struct SalvageRow {
    obj: Object,
    state: ItemState,
    t: RowTemplate,
    pub variant: VariantKey,
    pub paint: [u32; 3],
    picture: i32,
    /// The wreck's armour per location (`MechIcon::init` from the salvaged mech).
    armor: [(f32, f32, bool); 11],
    /// The icon with the wreck's damage colours, once [`SalvageRow::compose_icon`] has run.
    image: Option<TexId>,
    /// The offer's candidate index, naming the composed texture.
    candidate: usize,
    variant_text: String,
    prev_state: ItemState,
}

impl SalvageRow {
    /// `SalvageListItem::SalvageListItem`.
    pub fn new(
        t: &RowTemplate,
        assets: &UiAssets,
        logistics: &Logistics,
        offer: &SalvageOffer,
    ) -> SalvageRow {
        let mut t = t.clone();
        let chassis = logistics.chassis_of(offer.variant);
        t.mech_name.set_text(
            chassis
                .map(|c| assets.string(c.chassis_name_id as u32))
                .unwrap_or_default(),
        );
        t.variant_name.set_text(&offer.variant_name);
        t.cost.set_text(&offer.cost.to_string());
        t.weight.set_text(&(offer.tonnage as i64).to_string());
        t.normal.begin();
        t.check.button.move_by(0.0, ROW_TOP);
        let mut obj = Object::new(0.0, ROW_TOP, t.width, t.height);
        obj.id = 0;
        SalvageRow {
            obj,
            state: ItemState::Enabled,
            picture: chassis.map_or(0, |c| c.icon_picture_index as i32),
            variant: offer.variant,
            paint: offer.paint_scheme,
            armor: offer.armor,
            image: None,
            candidate: offer.candidate,
            variant_text: offer.variant_name.clone(),
            prev_state: ItemState::Enabled,
            t,
        }
    }

    /// Whether the damage colours of the icon are still to be composed.
    pub fn icon_pending(&self) -> bool {
        self.image.is_none()
    }

    /// The icon with the wreck's damage colours (`MechIcon::update`), composed into its own
    /// texture; a row not composed draws the undamaged cell.
    pub fn compose_icon(&mut self, textures: &mut TextureStore) {
        let Some(atlas) = self.t.art.atlas(false) else {
            return;
        };
        let loc = |i: usize| {
            let (cur, max, destroyed) = self.armor[i];
            Loc {
                cur,
                max,
                destroyed,
            }
        };
        let a = MechArmor {
            head: loc(0),
            ctorso: loc(1),
            ltorso: loc(2),
            rtorso: loc(3),
            larm: loc(4),
            rarm: loc(5),
            lleg: loc(6),
            rleg: loc(7),
            rctorso: loc(8),
            rltorso: loc(9),
            rrtorso: loc(10),
        };
        let mut pixels = damage::copy_cell(textures, atlas, self.picture.max(0) as u32);
        damage::apply(&mut pixels, &mut [0; 8], &damage::mech_states(&a), 8);
        let name = format!("results/salvage{}_{}", self.candidate, self.picture);
        self.image = Some(textures.insert(&name, ICON_W, ICON_H, pixels));
    }

    /// The composed icon, if any.
    pub fn icon_image(&self) -> Option<TexId> {
        self.image
    }

    pub fn is_checked(&self) -> bool {
        self.t.check.is_pressed()
    }

    /// The row's box: the check box and the unit icon are drawn inside it.
    pub fn check_button(&self) -> &AnimButton {
        &self.t.check
    }

    /// `SalvageListItem::update`'s enable and disable: a row that costs more than the C-Bills (and
    /// is not ticked) is greyed out together with its check box.
    pub fn set_disabled(&mut self, disabled: bool) {
        if disabled {
            self.state = ItemState::Disabled;
        } else if self.state == ItemState::Disabled {
            self.state = ItemState::Enabled;
        }
        self.t.check.disable(disabled);
    }

    pub fn set_checked(&mut self, checked: bool) {
        self.t.check.press(checked);
    }

    /// The colour the row, its texts and its outline draw in.
    pub fn colour(&self) -> u32 {
        match self.state {
            ItemState::Disabled => ROW_DISABLED,
            ItemState::Selected => self.t.pressed.color(),
            ItemState::Highlite => self.t.highlight.color(),
            _ => self.t.normal.color(),
        }
    }

    /// The unit icon's rectangle (`SalvageListItem::render`): `iconRect.left + x + 3`,
    /// `iconRect.top (0) + y + 5` to `iconRect.right + x`, `iconRect.bottom + y`.
    pub fn unit_icon_rect(&self) -> (f32, f32, f32, f32) {
        let (x, y) = (self.obj.global_x() as f32, self.obj.global_y() as f32);
        (
            self.t.icon_left + x + 3.0,
            y + 5.0,
            self.t.icon_right + x,
            self.t.icon_bottom + y,
        )
    }

    /// The row's outline (`drawEmptyRect(tmp, color, color)`).
    pub fn outline(&self) -> GuiRect {
        let left = self.obj.global_x() + self.t.check.width() as i32 + 3;
        let top = self.obj.global_y();
        GuiRect::new(
            left,
            top,
            left + self.t.width as i32,
            self.obj.global_bottom(),
        )
    }
}

impl ListItem for SalvageRow {
    fn obj(&self) -> &Object {
        &self.obj
    }
    fn obj_mut(&mut self) -> &mut Object {
        &mut self.obj
    }
    fn state(&self) -> ItemState {
        self.state
    }
    fn set_state(&mut self, s: ItemState) {
        self.state = s;
    }
    fn is_checked(&self) -> bool {
        SalvageRow::is_checked(self)
    }
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.t.check.button.move_by(dx, dy);
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        match self.state {
            ItemState::Highlite => {
                if self.prev_state != ItemState::Highlite || !self.t.highlight.is_animating() {
                    self.t.highlight.begin();
                }
                self.t.highlight.update(ctx.dt);
            }
            ItemState::Selected => {
                if !self.t.pressed.is_animating() {
                    self.t.pressed.begin();
                }
                self.t.pressed.update(ctx.dt);
            }
            _ => {}
        }
        self.prev_state = self.state;
        self.t.normal.update(ctx.dt);
        self.t.check.button.obj.show = self.obj.show;
        self.t.check.update(ctx);
        self.obj.update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let colour = self.colour();
        let (ox, oy) = (self.obj.global_x() as f32, self.obj.global_y() as f32);
        let icon = self.unit_icon_rect();
        let outline = self.outline();
        let t = &mut self.t;
        for text in [
            &mut t.mech_name,
            &mut t.variant_name,
            &mut t.weight,
            &mut t.cost,
        ] {
            text.set_color(colour);
            text.render_at(ctx, ox, oy);
        }
        for icon in [&mut t.cbills_icon, &mut t.weight_icon] {
            icon.set_color(colour, false);
            icon.render_at(ctx, ox, oy);
        }
        t.check.render(ctx);
        match self.image {
            Some(image) => t.art.image(ctx, image, icon),
            None => t.art.unit(ctx, false, self.picture, icon),
        }
        ctx.dl.outline_rect(outline, colour, colour);
    }
    fn text(&self) -> Option<&str> {
        Some(&self.variant_text)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `SalvageMechArea`: the panel for the mech the player has picked.
pub struct SalvagePanel {
    /// `SalvageAreaMechText0..6` (name, payload title, weight, armour, speed, jump, range).
    texts: LogisticsScreen,
    meters: [AttributeMeter; 3],
    pub loadout: Loadout,
    /// `mechCamera`'s rectangle: `SalvageAreaRect6`.
    pub camera: (f32, f32, f32, f32),
    /// `unit`.
    unit: Option<VariantKey>,
    /// The 3D view's request for `unit`.
    pub preview: Option<PreviewRequest>,
    /// `mechCamera`'s activation state: its first mech is lit by the fit light, later ones by the activation daylight.
    pub activation: CameraActivation,
}

impl SalvagePanel {
    /// `SalvageMechArea::init`.
    pub fn load(
        ld: &mut Loader<'_>,
        screen: &LogisticsScreen,
        fit: &mut mc2_formats::ini::FitIni,
    ) -> Result<SalvagePanel, String> {
        let r = |i: usize| screen.rect(i).map(|r| &r.obj);
        let list = r(8).ok_or("the layout has no loadout rectangle")?;
        let loadout = Loadout::new(ld, list.left(), list.top(), list.width(), list.height());
        let cam = r(6).ok_or("the layout has no mech camera rectangle")?;
        let texts = LogisticsScreen::from_fit(
            ld,
            fit,
            Prefixes {
                statics: None,
                texts: Some("SalvageAreaMechText"),
                rects: None,
                buttons: None,
                edits: None,
                anim_objects: None,
            },
        );
        Ok(SalvagePanel {
            texts,
            meters: [
                AttributeMeter::from_fit(fit, "AttributeMeter0"),
                AttributeMeter::from_fit(fit, "AttributeMeter1"),
                AttributeMeter::from_fit(fit, "AttributeMeter2"),
            ],
            loadout,
            camera: (cam.left(), cam.top(), cam.right(), cam.bottom()),
            unit: None,
            preview: None,
            activation: CameraActivation::default(),
        })
    }

    pub fn unit(&self) -> Option<VariantKey> {
        self.unit
    }

    /// The text of `SalvageAreaMechText{i}`.
    pub fn text(&self, i: usize) -> &str {
        self.texts.texts.get(i).map_or("", |t| t.text.as_str())
    }

    pub fn text_colour(&self, i: usize) -> u32 {
        self.texts.texts.get(i).map_or(0, |t| t.obj.color())
    }

    /// The meters' fractions of their scales (armour, speed, jump).
    pub fn meter_values(&self) -> [f32; 3] {
        [
            self.meters[0].percent,
            self.meters[1].percent,
            self.meters[2].percent,
        ]
    }

    fn set_text(&mut self, i: usize, s: &str) {
        if let Some(t) = self.texts.texts.get_mut(i) {
            t.set_text(s);
        }
    }

    /// `SalvageMechArea::setMech`: shows `variant` painted with `paint`, or clears the panel.
    pub fn set_mech(
        &mut self,
        assets: &UiAssets,
        lg: &Logistics,
        variant: Option<VariantKey>,
        paint: [u32; 3],
    ) {
        if variant == self.unit {
            return;
        }
        self.unit = variant;
        self.loadout.set_mech(assets, lg, variant);
        let stats = variant.and_then(|v| MechStats::of_variant(assets, lg, v));
        let Some(s) = stats else {
            for i in [2, 3, 4, 5] {
                self.set_text(i, "");
            }
            for m in &mut self.meters {
                m.set_value(0.0);
            }
            self.preview = None;
            return;
        };
        self.set_text(0, &s.variant_name);
        // the C++ prints the weight with "%f" ("55.000000"); the tons are shown whole
        self.set_text(2, &format!("{}", s.max_weight as i64));
        self.set_text(3, &s.armor.to_string());
        self.set_text(4, &s.display_speed.to_string());
        self.set_text(5, &s.jump.to_string());
        self.set_text(6, &s.range_text);
        if let Some(t) = self.texts.texts.get_mut(6) {
            t.set_color(s.range_colour);
        }
        self.meters[0].set_value(s.armor_meter);
        self.meters[1].set_value(s.speed_meter);
        self.meters[2].set_value(s.jump_meter);
        let mut req = s.model_request(self.camera, assets.player_paint);
        // `(base, highlight1, highlight2)` of the mech -> the request's `(h1, h2, base)`; a mech
        // with no paint of its own wears the player's prefs
        req.paint = if paint == [0; 3] {
            assets.player_paint
        } else {
            [paint[1], paint[2], paint[0]]
        };
        self.preview = Some(self.activation.set(req));
    }

    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.loadout.update(ctx);
    }

    /// `SalvageMechArea::render(xOffset, yOffset)`: the loadout, then, once a mech is picked, the
    /// texts, the meters and the 3D view (which does not slide).
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>, x: i32, y: i32) {
        self.loadout.list.move_by(x as f32, y as f32);
        self.loadout.render(ctx);
        self.loadout.list.move_by(-x as f32, -y as f32);
        if self.unit.is_none() {
            return;
        }
        for t in &mut self.texts.texts {
            t.render_at(ctx, x as f32, y as f32);
        }
        for m in &self.meters {
            m.render(ctx, x, y);
        }
        if x == 0 && y == 0 {
            // `mechCamera.update()` with a model: `active`, so the next `setMech` relights it
            self.activation.update(self.preview.is_some());
            if let Some(tex) = self
                .preview
                .as_ref()
                .and_then(|r| preview_texture(ctx.assets, r))
            {
                draw_preview(ctx.dl, tex, self.camera);
            }
        }
    }
}
