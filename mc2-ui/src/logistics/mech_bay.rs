// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MechBayScreen` (`code/mechbayscreen.cpp`): the inventory list of mechs not in the force group,
//! the force group's twelve slots (`LogisticsMechIcon`s), the mech's statistics, and the drop
//! weight meter. Drag and drop moves a mech between the list and the slots; the weight meter
//! flashes the weight added or removed.
//!
//! The loadout list of components (`loadoutListBox`, rect 3) is shown beside the statistics, and
//! the mech's 3D picture (`mechCamera`) in its panel. The purchase and mech lab screens are reached
//! through [`Page`]s the sequence controller holds; the screen only asks for them by status (`Up`,
//! `Down`).

use super::attribute_meter::AttributeMeter;
use super::loadout::Loadout;
use super::mech_display::MechStats;
use super::mech_icon::icon_grid;
use super::mech_icon::LogisticsMechIcon;
use super::mech_list::{EntryEvent, EntryTemplate, MechEntry, MechList};
use super::{chassis_icon, format_pair, ids, rect_overlaps, strings, variant_of, Page};
use crate::anim::Animation;
use crate::color::interpolate_color;
use crate::ctx::{sfx, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::{Blend, GuiRect, Vertex};
use crate::fit::FitExt;
use crate::preview::{draw_preview, preview_texture, CameraActivation, PreviewRequest};
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::widgets::{ItemState, ListBox, Object};
use mc2_campaign::{Logistics, MechKey};

/// `ICON_COUNT_X`, `ICON_COUNT_Y`, `ICON_COUNT`: the force group grid.
pub const ICON_COUNT_X: usize = 4;
pub const ICON_COUNT_Y: usize = 3;
pub const ICON_COUNT: usize = ICON_COUNT_X * ICON_COUNT_Y;
/// The largest force group (`maxUnits` in single player).
const MAX_UNITS: usize = 12;
/// `helpTextArrayID`.
const HELP_TEXT: i32 = 13;
/// `rects[0]` of `mcl_mb_layout.fit`: the list.
const LIST_RECT: usize = 0;
/// `rects[3]`: the loadout list.
const LOADOUT_RECT: usize = 3;
/// `textObjects[]` indices.
const TEXT_CBILLS: usize = 1;
const TEXT_WEIGHT_NOW: usize = 6;
const TEXT_CLASS_WEIGHT: usize = 7;
const TEXT_ARMOR: usize = 8;
const TEXT_SPEED: usize = 9;
const TEXT_JUMP: usize = 10;
const TEXT_CHASSIS: usize = 11;
const TEXT_VARIANT: usize = 12;
const TEXT_RANGE: usize = 14;

pub struct MechBay {
    /// The layout (`mcl_mb_layout.fit`).
    pub base: LogisticsScreen,
    /// The inventory list (`mechListBox`).
    pub list: MechList,
    /// `loadoutListBox`: the components of the current mech.
    pub loadout: Loadout,
    /// `mechCamera`'s panel: `(left, top, right, bottom)` (`rects[1]`, `rects[2]`).
    pub preview_rect: (f32, f32, f32, f32),
    /// The picture of `cur_mech`, once it is asked for.
    pub preview: Option<PreviewRequest>,
    /// `mechCamera`'s activation state: its first mech is lit by the fit light, later ones by the activation daylight.
    pub activation: CameraActivation,
    entry_template: EntryTemplate,
    /// The force group's slots (`pIcons`).
    pub icons: Vec<LogisticsMechIcon>,
    /// `attributeMeters[3]`: armour, speed, jump range.
    pub attribute_meters: [AttributeMeter; 3],
    /// `dropWeightMeter`, an `aObject`: the semicircle graphic and mask drawn over the rays.
    pub drop_weight_meter: Object,
    weight_centre: (f32, f32),
    weight_colours: (u32, u32),
    add_weight: Animation,
    remove_weight: Animation,
    add_weight_amount: f32,
    remove_weight_amount: f32,
    /// `pCurMech`: the mech whose statistics show.
    pub cur_mech: Option<MechKey>,
    /// `pDragMech`.
    pub drag_mech: Option<MechKey>,
    /// `dragLeft`: the drag started in the force group.
    drag_left: bool,
    drag_icon: Object,
    /// `forceGroupCount`.
    force_group_count: usize,
    /// The mech whose loadout the player asked to change. C++ `setMechToModify` runs on the click
    /// (mechbayscreen.cpp:645); here it runs when this screen is left for the mech lab (`end`), so
    /// a lab that is not wired up changes nothing.
    pending_edit: Option<MechKey>,
    /// The drop weight the meter shows (set by `update`, read by `render`).
    weight_now: f32,
    weight_max: f32,
}

impl MechBay {
    /// `MechBayScreen::MechBayScreen` and `init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<MechBay, String> {
        let mut base = LogisticsScreen::load(
            ld,
            "mcl_mb_layout",
            Prefixes {
                statics: Some("MechBayStatic"),
                texts: Some("MechBayTextEntry"),
                rects: Some("MechBayRect"),
                buttons: Some("MechBayButton"),
                ..Prefixes::default()
            },
        )?;
        base.help_text_array_id = HELP_TEXT;
        for b in &mut base.buttons {
            b.set_message_on_release();
        }
        let mut file = ld.art_fit("mcl_mb_layout")?;

        // the list is `rects[0]`; its entries come from `mcl_gn_availablemechentry.fit`
        let rect = base
            .rect(LIST_RECT)
            .ok_or("mcl_mb_layout.fit has no list rectangle")?;
        let (x, y, w, h) = (
            rect.obj.left(),
            rect.obj.top(),
            rect.obj.width(),
            rect.obj.height(),
        );
        let mut entry_file = ld.art_fit("mcl_gn_availablemechentry")?;
        let entry_template = EntryTemplate::load(ld, &mut entry_file);
        let list = MechList::new(ListBox::new(ld, x, y, w, h), true, false);
        // `loadoutListBox.init(rects[3])`
        let rect = base
            .rect(LOADOUT_RECT)
            .ok_or("mcl_mb_layout.fit has no loadout rectangle")?;
        let (x, y, w, h) = (
            rect.obj.left(),
            rect.obj.top(),
            rect.obj.width(),
            rect.obj.height(),
        );
        let loadout = Loadout::new(ld, x, y, w, h);
        // `mechCamera->init(rects[1].left(), rects[1].top(), rects[2].right(), rects[1].bottom())`
        let (r1, r2) = (
            base.rect(1)
                .ok_or("mcl_mb_layout.fit has no camera rectangle")?,
            base.rect(2)
                .ok_or("mcl_mb_layout.fit has no camera rectangle")?,
        );
        let preview_rect = (r1.obj.left(), r1.obj.top(), r2.obj.right(), r1.obj.bottom());

        // the force group grid: `mcl_gn_deploymentteams.fit` moved cell by cell
        let mut icon_file = ld.art_fit("mcl_gn_deploymentteams")?;
        let template = super::mech_icon::LogisticsMechIcon::load(ld, &mut icon_file);
        let icons = icon_grid(template);

        let attribute_meters = [
            AttributeMeter::from_fit(&mut file, "AttributeMeter0"),
            AttributeMeter::from_fit(&mut file, "AttributeMeter1"),
            AttributeMeter::from_fit(&mut file, "AttributeMeter2"),
        ];
        let drop_weight_meter = Object::from_fit(ld, &mut file, "DropWeightMeter");
        let centre_x = file.long_or("RayCenterX", 0) as f32;
        let centre_y = file.long_or("RayCenterY", 0) as f32;
        let start = file.color("RayStartColor").unwrap_or(0);
        let end = file.color("RayEndColor").unwrap_or(0);
        file.seek_block("weightAddAnimation").ok();
        let add_weight = Animation::from_fit(&file, "");
        file.seek_block("weightRemoveAnimation").ok();
        let remove_weight = Animation::from_fit(&file, "");
        let drag_icon = entry_template.drag_icon(0);

        let mut bay = MechBay {
            base,
            list,
            loadout,
            preview_rect,
            preview: None,
            activation: CameraActivation::default(),
            entry_template,
            icons,
            attribute_meters,
            drop_weight_meter,
            weight_centre: (centre_x, centre_y),
            weight_colours: (start, end),
            add_weight,
            remove_weight,
            add_weight_amount: 0.0,
            remove_weight_amount: 0.0,
            cur_mech: None,
            drag_mech: None,
            drag_left: false,
            drag_icon,
            force_group_count: 0,
            pending_edit: None,
            weight_now: 0.0,
            weight_max: 0.0,
        };
        for t in [7, 8, 9, 10, 11, 12, 14] {
            if let Some(t) = bay.base.texts.get_mut(t) {
                t.set_text("");
            }
        }
        for m in &mut bay.attribute_meters {
            m.set_value(0.0);
        }
        Ok(bay)
    }

    /// `begin`: the list is rebuilt from the inventory, the force group shown, and the current
    /// mech (or the first viable one) selected.
    pub fn begin_screen(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.base.status = Status::Running;
        self.drag_mech = None;
        self.list.remove_all();
        self.reinit(&*ld.assets, lg);

        let assets: &UiAssets = &*ld.assets;
        let mut cur_valid = false;
        let inventory: Vec<MechKey> = lg.inventory().iter().map(|m| m.key).collect();
        for key in &inventory {
            if Some(*key) == self.cur_mech {
                cur_valid = true;
            }
            let Some(m) = lg.mech(*key) else { continue };
            if m.force_group != 0 {
                continue;
            }
            let variant = m.variant;
            let listed = self.list.entries().any(|e| e.variant() == variant);
            if !listed {
                if let Some(e) = MechEntry::new(&self.entry_template, assets, lg, *key, false) {
                    self.list.add(e);
                }
            }
        }
        self.list.draw_cbills = false;
        self.list.set_orange(true);

        let cur = self.cur_mech;
        if cur.is_none() || !cur_valid {
            if !self.select_first_fg_item() && !self.select_first_viable_lb_mech() {
                self.set_mech(assets, lg, None, false);
            }
        } else {
            self.set_mech(assets, lg, None, false);
            self.set_mech(assets, lg, cur, false);
            let mut found = false;
            for i in 0..self.list.item_count() {
                let is_cur = self
                    .list
                    .entries()
                    .nth(i)
                    .is_some_and(|e| Some(e.mech) == cur);
                if is_cur {
                    self.list.select(i as i32);
                    found = true;
                }
            }
            if !found {
                if let Some(icon) = self.icons.iter_mut().find(|i| i.content.mech == cur) {
                    icon.select(true);
                }
            }
        }
    }

    /// `setMech`: shows `mech` in the statistics (or clears them).
    pub fn set_mech(
        &mut self,
        assets: &UiAssets,
        lg: &Logistics,
        mech: Option<MechKey>,
        from_list: bool,
    ) {
        if mech == self.cur_mech || self.base.status != Status::Running {
            return;
        }
        self.cur_mech = mech;
        if from_list {
            // clicked on a disabled mech
            self.unselect_deployment_team();
            self.list.select(-1);
        }
        let variant = mech.and_then(|key| lg.mech(key)).map(|m| m.variant);
        self.loadout.set_mech(assets, lg, variant);
        let stats = mech.and_then(|key| MechStats::of(assets, lg, key));
        let activation = self.activation;
        self.preview = stats
            .as_ref()
            .map(|s| activation.set(s.model_request(self.preview_rect, assets.player_paint)));
        let Some(stats) = stats else {
            self.clear_stats();
            return;
        };
        self.set_text(TEXT_CLASS_WEIGHT, &stats.weight_text);
        self.set_text(TEXT_RANGE, &stats.range_text);
        if let Some(t) = self.base.texts.get_mut(TEXT_RANGE) {
            t.obj.set_color(stats.range_colour, false);
        }
        self.set_text(TEXT_ARMOR, &stats.armor.to_string());
        self.attribute_meters[0].set_value(stats.armor_meter);
        self.set_text(TEXT_SPEED, &stats.display_speed.to_string());
        self.attribute_meters[1].set_value(stats.speed_meter);
        self.set_text(TEXT_JUMP, &stats.jump.to_string());
        self.attribute_meters[2].set_value(stats.jump_meter);
        self.set_text(TEXT_CHASSIS, &stats.chassis_name);
        self.set_text(TEXT_VARIANT, &stats.variant_name);
    }

    fn clear_stats(&mut self) {
        for t in [
            TEXT_CLASS_WEIGHT,
            TEXT_ARMOR,
            TEXT_SPEED,
            TEXT_JUMP,
            TEXT_CHASSIS,
            TEXT_VARIANT,
            TEXT_RANGE,
        ] {
            self.set_text(t, "");
        }
        for m in &mut self.attribute_meters {
            m.set_value(0.0);
        }
    }

    fn set_text(&mut self, i: usize, s: &str) {
        if let Some(t) = self.base.texts.get_mut(i) {
            t.set_text(s);
        }
    }

    /// `reinitMechs`: every slot is emptied, then the force group is read from the inventory.
    fn reinit(&mut self, assets: &UiAssets, lg: &Logistics) {
        for (i, icon) in self.icons.iter_mut().enumerate() {
            icon.set_mech(assets, lg, None);
            icon.disabled = i >= MAX_UNITS;
        }
        self.force_group_count = 0;
        for m in lg.inventory() {
            if m.force_group == 0 {
                continue;
            }
            let slot = m.force_group as usize;
            if slot <= MAX_UNITS {
                let icon = &mut self.icons[slot - 1];
                icon.set_mech(assets, lg, Some(m.key));
                icon.set_pilot(assets, lg);
                self.force_group_count += 1;
            }
        }
    }

    /// `unselectDeploymentTeam`.
    fn unselect_deployment_team(&mut self) {
        for icon in &mut self.icons {
            icon.select(false);
        }
    }

    /// `selectFirstFGItem`: selects the first occupied slot, if any.
    fn select_first_fg_item(&mut self) -> bool {
        let mut found = false;
        for icon in self.icons.iter_mut() {
            if icon.content.mech.is_some() && !found {
                icon.select(true);
                found = true;
            } else {
                icon.select(false);
            }
        }
        found
    }

    /// `selectFirstViableLBMech`: selects the first entry that is not disabled.
    fn select_first_viable_lb_mech(&mut self) -> bool {
        let first = (0..self.list.item_count()).find(|&i| {
            self.list
                .entries()
                .nth(i)
                .is_some_and(|e| e.state != ItemState::Disabled)
        });
        match first {
            Some(i) => {
                self.list.select(i as i32);
                true
            }
            None => false,
        }
    }

    /// `getFGSelMech`: the selected slot's mech.
    fn fg_selected_mech(&self) -> Option<MechKey> {
        self.icons
            .iter()
            .find(|i| i.content.mech.is_some() && i.is_selected())
            .and_then(|i| i.content.mech)
    }

    /// `addSelectedMech`: the list's selected mech joins the force group in the next slot.
    fn add_selected_mech(&mut self, assets: &UiAssets, lg: &mut Logistics) {
        let Some(cur) = self.list.current_mech() else {
            return;
        };
        if !lg.can_add_mech_to_force_group(cur) {
            return;
        }
        if lg
            .add_mech_to_force_group(cur, self.force_group_count as i32 + 1)
            .is_err()
        {
            // already in the force group (the add was called twice)
            return;
        }
        if self.force_group_count > 11 {
            return;
        }
        self.unselect_deployment_team();
        self.force_group_count += 1;
        self.reinit(assets, lg);

        self.add_weight_amount = lg.mech_max_weight(cur).unwrap_or(0.0);
        self.add_weight.begin();
        self.remove_weight.end();

        let any_left = (0..self.list.item_count()).any(|i| {
            self.list
                .entries()
                .nth(i)
                .is_some_and(|e| lg.can_add_mech_to_force_group(e.mech))
        });
        if !any_left {
            self.select_first_fg_item();
            self.list.select(-1);
        }
    }

    /// `removeSelectedMech`: the selected slot's mech leaves the force group, the slots behind it
    /// move up one.
    fn remove_selected_mech(&mut self, assets: &UiAssets, lg: &mut Logistics) {
        let mut i = 0;
        while i < ICON_COUNT {
            if self.icons[i].is_selected() {
                let Some(mech) = self.icons[i].content.mech else {
                    return;
                };
                if lg.variants_in_inventory(variant_of(lg, mech), false) == 0 {
                    if let Some(e) = MechEntry::new(&self.entry_template, assets, lg, mech, false) {
                        self.list.add(e);
                    }
                }
                let _ = lg.remove_mech_from_force_group(mech, true);
                self.force_group_count = self.force_group_count.saturating_sub(1);
                self.remove_weight_amount = lg.mech_max_weight(mech).unwrap_or(0.0);
                self.remove_weight.begin();
                self.add_weight.end();
                if i >= self.force_group_count {
                    if !self.select_first_fg_item() {
                        self.select_first_viable_lb_mech();
                    }
                } else {
                    self.icons[i].select(true);
                }
                break;
            }
            i += 1;
        }
        // the slots behind the removed one move up
        while i + 1 < ICON_COUNT {
            let next = self.icons[i + 1].content.mech;
            if let Some(m) = next {
                let _ = lg.remove_mech_from_force_group(m, false);
                let _ = lg.add_mech_to_force_group(m, i as i32 + 1);
            }
            i += 1;
        }
        self.reinit(assets, lg);
        if self.force_group_count == 0 {
            self.select_first_viable_lb_mech();
        }
    }

    /// `beginDrag`: a mech is picked up from the list (it joins the force group for the drag) or
    /// from a slot (the slots behind it move down). `mouse` is the drag's start.
    pub fn begin_drag(
        &mut self,
        assets: &UiAssets,
        lg: &mut Logistics,
        mech: MechKey,
        mouse: (i32, i32),
    ) {
        if self.drag_mech.is_some() || self.base.status != Status::Running {
            return;
        }
        self.drag_icon = self.entry_template.drag_icon(
            lg.mech(mech)
                .and_then(|m| chassis_icon(lg, m.variant))
                .unwrap_or(0),
        );
        self.drag_mech = Some(mech);
        if self.list.list.obj.point_inside(mouse.0, mouse.1) {
            let mut mech = mech;
            if lg.mech(mech).is_some_and(|m| m.force_group != 0) {
                if let Some(other) = lg.mech_without_force_group(mech) {
                    mech = other;
                }
            }
            self.drag_mech = Some(mech);
            if !lg.can_add_mech_to_force_group(mech) {
                self.drag_mech = None;
                return;
            }
            let _ = lg.add_mech_to_force_group(mech, self.force_group_count as i32 + 1);
            self.drag_left = false;
        } else {
            for i in 0..ICON_COUNT {
                if !self.icons[i].point_inside(mouse.0, mouse.1) {
                    continue;
                }
                self.force_group_count = self.force_group_count.saturating_sub(1);
                if i == self.force_group_count && self.force_group_count != 0 {
                    self.icons[i].select(false);
                    self.icons[0].select(true);
                } else if self.force_group_count == 0 {
                    self.icons[i].select(false);
                }
                let mut j = i;
                while j < ICON_COUNT - 1 {
                    let next = self.icons[j + 1].content.mech;
                    if let Some(m) = next {
                        let _ = lg.remove_mech_from_force_group(m, false);
                        let _ = lg.add_mech_to_force_group(m, j as i32 + 1);
                    }
                    self.icons[j].set_mech(assets, lg, next);
                    self.icons[j].set_pilot(assets, lg);
                    j += 1;
                }
                self.icons[j].set_mech(assets, lg, None);
                self.icons[j].set_pilot(assets, lg);
                break;
            }
            self.drag_left = true;
        }
    }

    /// `update`: the drag, the slots, the list, the buttons.
    pub fn update_screen(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        self.disable_items_that_cannot_go_in_force_group(lg);
        if self.icons[0].content.mech.is_none() {
            self.set_disabled(ids::MB_MSG_REMOVE, true);
        }

        let cbills = lg.c_bills();
        self.set_text(TEXT_CBILLS, &format!("{cbills} "));

        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if ctx.input.is_left_click() && self.drop_weight_meter.point_inside(mx, my) {
            ctx.play(sfx::LOG_WRONGBUTTON);
        }
        let now = lg.current_drop_weight();
        let max = lg.max_drop_weight();
        self.weight_now = now as f32;
        self.weight_max = max as f32;
        self.set_text(
            TEXT_WEIGHT_NOW,
            &format_pair(ctx.assets.string(strings::MB_WEIGHT), now, max),
        );

        if self.drag_mech.is_some() {
            self.update_drag(ctx, lg);
        }

        // the slots: hover, selection, double-click removes, a drag starts
        let mut new_sel: Option<usize> = None;
        let mut old_sel: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut drag_from: Option<(usize, MechKey)> = None;
        let drag_origin = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        for i in 0..ICON_COUNT {
            let icon = &mut self.icons[i];
            icon.update(ctx);
            if icon.just_selected {
                new_sel = Some(i);
            } else if icon.is_selected() {
                old_sel = Some(i);
            }
            let inside = icon.point_inside(mx, my);
            if inside && ctx.input.is_left_double_click() && icon.content.mech.is_some() {
                remove = Some(i);
            } else if ctx.input.is_left_drag() && icon.point_inside(drag_origin.0, drag_origin.1) {
                if let Some(m) = icon.content.mech {
                    drag_from = Some((i, m));
                }
            }
        }
        if let Some((_, m)) = drag_from {
            let mouse = (drag_origin.0, drag_origin.1);
            self.begin_drag(ctx.assets, lg, m, mouse);
        }

        if new_sel.is_some() {
            if let Some(o) = old_sel {
                self.icons[o].select(false);
            }
            self.list.select(-1);
        }
        if remove.is_some() {
            self.remove_selected_mech(ctx.assets, lg);
        }

        let msgs = self.base.update(ctx);
        for m in msgs {
            self.handle_message(ctx, lg, m.who);
        }

        let remove_enabled = new_sel.is_some() || old_sel.is_some();
        self.set_disabled(ids::MB_MSG_REMOVE, !remove_enabled);

        // the list: the selection changes when a slot was clicked or the list is clicked
        let old = self.list.selected();
        self.loadout.update(ctx);
        let events = self.list.update(ctx, lg);
        let new = self.list.selected();
        if old != new && new != -1 {
            self.unselect_deployment_team();
        }
        for e in events {
            match e {
                EntryEvent::SetMech(m) => {
                    let assets = ctx.assets;
                    self.set_mech(assets, lg, Some(m), true);
                }
                EntryEvent::StartDrag(m) => {
                    let mouse = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
                    self.begin_drag(ctx.assets, lg, m, mouse);
                }
                EntryEvent::Add(_) => {
                    self.add_selected_mech(ctx.assets, lg);
                    ctx.play(sfx::LOG_SELECT);
                }
            }
        }

        for a in [&mut self.add_weight, &mut self.remove_weight] {
            a.update(ctx.dt);
        }

        let pick = self.list.current_mech().or_else(|| self.fg_selected_mech());
        if let Some(p) = pick {
            let assets = ctx.assets;
            self.set_mech(assets, lg, Some(p), false);
        }

        let add_ok = new != -1
            && self
                .list
                .current_mech()
                .is_some_and(|m| lg.can_add_mech_to_force_group(m));
        self.set_disabled(ids::MB_MSG_ADD, !add_ok);

        if lg.skip_purchasing() {
            self.set_disabled(ids::MB_MSG_CHANGE_LOADOUT, true);
            self.set_disabled(ids::MB_MSG_BUY, true);
        } else {
            self.set_disabled(ids::MB_MSG_BUY, false);
            let no_mech = self.cur_mech.is_none();
            self.set_disabled(ids::MB_MSG_CHANGE_LOADOUT, no_mech);
        }
    }

    fn set_disabled(&mut self, id: i32, disabled: bool) {
        if let Some(b) = self.base.button_mut(id) {
            b.disable(disabled);
        }
    }

    /// The part of `update` that moves the dragged icon and drops it.
    fn update_drag(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let (w, h) = (self.drag_icon.width(), self.drag_icon.height());
        self.drag_icon
            .move_to(mx as f32 - w / 2.0, my as f32 - h / 2.0);

        let grid = self.grid_rect();
        let drag = self.drag_icon.global_rect();
        let over_slots = rect_overlaps(drag, grid);
        let Some(mech) = self.drag_mech else {
            return;
        };
        if !(ctx.input.left_mouse_released() || ctx.input.is_left_double_click()) {
            return;
        }
        let assets = ctx.assets;
        if self.drag_left {
            if over_slots {
                let _ = lg.remove_mech_from_force_group(mech, false);
                let _ = lg.add_mech_to_force_group(mech, self.force_group_count as i32 + 1);
                self.force_group_count += 1;
                self.reinit(assets, lg);
            } else {
                if lg.variants_in_inventory(variant_of(lg, mech), false) == 0 {
                    if let Some(e) = MechEntry::new(&self.entry_template, assets, lg, mech, false) {
                        let at = self.list.add(e);
                        if let Some(i) = at {
                            self.list.select(i as i32);
                        }
                    }
                }
                let _ = lg.remove_mech_from_force_group(mech, true);
                self.force_group_count = self.force_group_count.saturating_sub(1);
                self.reinit(assets, lg);
                self.remove_weight_amount = lg.mech_max_weight(mech).unwrap_or(0.0);
                self.remove_weight.begin();
                self.add_weight.end();
            }
        } else if over_slots {
            self.reinit(assets, lg);
            self.add_weight_amount = lg.mech_max_weight(mech).unwrap_or(0.0);
            self.add_weight.begin();
            self.remove_weight.end();
        } else {
            if lg.variants_in_inventory(variant_of(lg, mech), false) == 0 {
                if let Some(e) = MechEntry::new(&self.entry_template, assets, lg, mech, false) {
                    let at = self.list.add(e);
                    if let Some(i) = at {
                        self.list.select(i as i32);
                    }
                }
            }
            let _ = lg.remove_mech_from_force_group(mech, true);
            self.reinit(assets, lg);
        }
        self.drag_mech = None;
    }

    /// The bounding box of the slot grid.
    fn grid_rect(&self) -> GuiRect {
        let first = self.icons[0].outline.global_rect();
        let last = self.icons[ICON_COUNT - 1].outline.global_rect();
        GuiRect::new(first.left, first.top, last.right, last.bottom)
    }

    fn disable_items_that_cannot_go_in_force_group(&mut self, lg: &Logistics) {
        self.list.disable_items_that_cannot_go_in_force_group(lg);
    }

    /// `handleMessage`: the buttons of the screen.
    pub fn handle_message(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics, who: i32) {
        if self.base.status != Status::Running {
            return;
        }
        let assets = ctx.assets;
        match who {
            ids::MB_MSG_NEXT => self.base.status = Status::Next,
            ids::MB_MSG_PREV => self.base.status = Status::Previous,
            ids::MB_MSG_ADD => {
                self.add_selected_mech(assets, lg);
                ctx.play(sfx::LOG_SELECT);
            }
            ids::MB_MSG_REMOVE => self.remove_selected_mech(assets, lg),
            ids::MB_MSG_BUY => self.base.status = Status::Up,
            ids::MB_MSG_CHANGE_LOADOUT => {
                self.base.status = Status::Down;
                self.pending_edit = self.cur_mech;
            }
            ids::MB_MSG_BUY_SELL => {}
            ids::MB_MSG_MAINMENU => self.base.status = Status::MainMenu,
            _ => {}
        }
    }

    /// `drawWeightMeter`: twenty rays of 9 degrees around the centre, lit to the drop weight; the
    /// flashes show the weight being added or removed.
    fn draw_weight_meter(&self, ctx: &mut RenderCtx<'_>) {
        let now = self.weight_now;
        let max = self.weight_max;
        let mut bars = 20i32;
        if max != 0.0 {
            bars = ((now / max * 20.0) + 0.5) as i32;
        }
        let (mut add_bars, mut remove_bars) = (0i32, 0i32);
        let mut add_colour = 0u32;
        let remove_colour = 0u32;
        if self.add_weight.is_animating() && !self.add_weight.is_done() {
            add_colour = self.add_weight.color();
            let p = (now - self.add_weight_amount) / max * 20.0;
            add_bars = bars - ((p + 0.5) as i32);
        }
        if self.remove_weight.is_animating() && !self.remove_weight.is_done() {
            add_colour = self.remove_weight.color();
            let p = self.remove_weight_amount / max * 20.0;
            remove_bars = (p + 0.5) as i32;
        }
        let (cx, cy) = self.weight_centre;
        let mut arc = 180.0f32;
        let point = |deg: f32| {
            let r = deg.to_radians();
            (50.0 * r.cos() + cx, 50.0 * r.sin() + cy)
        };
        let mut p1 = point(arc);
        for i in 0..(bars + remove_bars) {
            arc += 9.0;
            let p2 = point(arc);
            let colour = if i >= bars - add_bars {
                add_colour
            } else if i >= bars {
                remove_colour
            } else {
                interpolate_color(
                    self.weight_colours.0,
                    self.weight_colours.1,
                    i as f32 * 0.05,
                )
            };
            let v = |x: f32, y: f32| Vertex {
                x,
                y,
                u: 0.0,
                v: 0.0,
                argb: colour,
            };
            ctx.dl.quad(
                None,
                Blend::Alpha,
                [v(cx, cy), v(p1.0, p1.1), v(p2.0, p2.1), v(p2.0, p2.1)],
            );
            p1 = p2;
        }
        self.drop_weight_meter.render(ctx);
    }

    /// `render`.
    pub fn render_screen(&mut self, ctx: &mut RenderCtx<'_>) {
        self.list.render(ctx);
        self.loadout.render(ctx);
        // `mechCamera->update()` with a model: `active`, so the next `setMech` relights it
        self.activation.update(self.preview.is_some());
        // accepted option colours repaint the unchanged selection (the C++ re-begins the screen)
        if let Some(r) = self.preview.as_mut() {
            r.paint = ctx.assets.player_paint;
        }
        // `if (!xOffset && !yOffset && pCurMech) mechCamera->render()`
        if let Some(tex) = self
            .preview
            .as_ref()
            .and_then(|r| preview_texture(ctx.assets, r))
        {
            draw_preview(ctx.dl, tex, self.preview_rect);
        }
        for m in &self.attribute_meters {
            m.render(ctx, 0, 0);
        }
        self.draw_weight_meter(ctx);
        self.base.render(ctx);
        for icon in &mut self.icons {
            icon.render(ctx);
        }
        if self.drag_mech.is_some() {
            self.drag_icon.render(ctx);
        }
    }
}

impl Page for MechBay {
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.begin_screen(ld, lg);
    }
    /// Leaving for the mech lab starts the edit (`setMechToModify`). A move the sequence refuses
    /// clears the request through `set_status`, so this does not run.
    fn end(&mut self, lg: &mut Logistics) {
        if let Some(mech) = self.pending_edit.take() {
            if let Err(e) = lg.set_mech_to_modify(mech) {
                log::warn!("mech lab: {e}");
            }
        }
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        self.update_screen(ctx, lg);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.render_screen(ctx);
    }
    fn begin_fade_in(&mut self, t: f32) {
        self.base.begin_fade_in(t);
    }
    fn status(&self) -> Status {
        self.base.get_status()
    }
    fn set_status(&mut self, status: Status) {
        if status != Status::Down {
            self.pending_edit = None;
        }
        self.base.status = status;
    }
    fn base(&self) -> &LogisticsScreen {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.base
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
