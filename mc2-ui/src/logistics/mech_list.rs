// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MechListBox` and `MechListBoxItem` (`code/mechlistbox.cpp`): the storage list of the mech bay
//! (and the purchase screen), one entry per variant with a count of how many are in the inventory.
//!
//! An entry shows its mech's chassis, variant, weight, count and cost, and reports what the
//! player did with it (chosen, dragged, double-clicked) as [`EntryEvent`]s; the screen acts on
//! them, as the C++ entry calls `MechBayScreen::instance()`. The list keeps its entries sorted by
//! weight (heaviest first), then by chassis and variant name, as `MechListBox::AddItem` does.

use super::mech_icon::cell_uvs;
use crate::anim::{AnimGroup, AnimState};
use crate::ctx::{sfx, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::widgets::{ItemState, ListBox, ListItem, Object, Rect, Text};
use mc2_campaign::{Logistics, MechKey};
use mc2_formats::ini::FitIni;
use std::any::Any;

/// `ANIMATION_COUNT`: the animation groups per state set.
const ANIMATION_COUNT: usize = 4;
/// The entry's parts in the C++ `addChild` order: weight icon, mech icon, cost icon, chassis,
/// weight, count, variant, cost.
const PART_COUNT: usize = 8;
/// The entry colour of the mech bay's list (`0xffC66600` is the orange set, else white).
const TEXT_COLOUR: u32 = 0xffff_ffff;

/// What an entry asks of its screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryEvent {
    /// The entry was clicked (or clicked while disabled): show its mech.
    SetMech(MechKey),
    /// The entry was dragged: the screen starts a drag of its mech.
    StartDrag(MechKey),
    /// The entry was double-clicked: add its mech to the force group.
    Add(MechKey),
}

/// The parts of an entry shared by all entries (`MechListBoxItem::init`, `s_templateItem`).
#[derive(Clone, Debug)]
pub struct EntryTemplate {
    pub width: f32,
    pub height: f32,
    pub outline: Rect,
    pub line: Rect,
    weight_icon: Object,
    mech_icon: Object,
    cost_icon: Object,
    chassis: Text,
    weight: Text,
    count: Text,
    variant: Text,
    cost: Text,
    /// `animations[bOrange][i]`: [0] the normal set, [1] the orange set.
    animations: [[AnimGroup; ANIMATION_COUNT]; 2],
    /// `animationIDs`: the animation group of each part (-1 for none).
    ids: [i32; PART_COUNT],
}

impl EntryTemplate {
    /// `MechListBoxItem::init`: `mcl_gn_availablemechentry.fit`.
    /// `MechListBox::initIcon`: the mech picture for the drag icon, from the cell of `icon_index`.
    pub fn drag_icon(&self, icon_index: i64) -> Object {
        let mut icon = self.mech_icon.clone();
        let (u, v, u2, v2) = cell_uvs(icon_index, 10, icon.width(), icon.height());
        icon.file_width = 256.0;
        icon.set_uvs(u, v, u2, v2);
        icon
    }

    /// `MechListBoxItem::init`: `mcl_gn_availablemechentry.fit`.
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni) -> EntryTemplate {
        // the entry is as big as its outline (the layout has no `MainBox` block)
        let outline = Rect::from_fit(file, "Rect0");
        let (width, height) = (outline.obj.width(), outline.obj.height());
        let line = Rect::from_fit(file, "Rect1");
        let weight_icon = Object::from_fit(ld, file, "Static0");
        let mech_icon = Object::from_fit(ld, file, "Static1");
        let cost_icon = Object::from_fit(ld, file, "Static2");
        let chassis = Text::from_fit(ld, file, "Text0");
        let weight = Text::from_fit(ld, file, "Text1");
        let count = Text::from_fit(ld, file, "Text2");
        let variant = Text::from_fit(ld, file, "Text3");
        let cost = Text::from_fit(ld, file, "Text4");
        let blocks = [
            "Static0", "Static1", "Static2", "Text0", "Text1", "Text2", "Text3", "Text4",
        ];
        let ids = blocks.map(|b| block_animation(file, b));
        let animations = [
            std::array::from_fn(|i| AnimGroup::from_block(file, &format!("Animation{i}"))),
            std::array::from_fn(|i| AnimGroup::from_block(file, &format!("OrangeAnimation{i}"))),
        ];
        // the C++ keeps the orange set at index 1
        let [normal, orange] = animations;
        EntryTemplate {
            width,
            height,
            outline,
            line,
            weight_icon,
            mech_icon,
            cost_icon,
            chassis,
            weight,
            count,
            variant,
            cost,
            animations: [normal, orange],
            ids,
        }
    }
}

impl EntryTemplate {
    pub(crate) fn art_colour(&self, orange: bool, slot: usize) -> Option<u32> {
        let i = usize::try_from(self.ids[slot]).ok()?;
        let g = self.animations[usize::from(orange)].get(i)?;
        Some(g.current_color(g.state()))
    }

    /// Moves the parts (a row of the purchase screen is a template that moves with its list).
    pub(crate) fn move_art(&mut self, dx: f32, dy: f32) {
        self.outline.obj.move_by(dx, dy);
        self.line.obj.move_by(dx, dy);
        self.weight_icon.move_by(dx, dy);
        self.mech_icon.move_by(dx, dy);
        self.cost_icon.move_by(dx, dy);
    }

    /// `MechListBoxItem::update`'s state handling: the animation set follows the item's state.
    pub(crate) fn update_art(&mut self, state: ItemState, orange: bool, dt: f32) {
        let s = match state {
            ItemState::Selected => AnimState::Pressed,
            ItemState::Highlite => AnimState::Highlight,
            ItemState::Disabled => AnimState::Disabled,
            _ => AnimState::Normal,
        };
        for a in &mut self.animations[usize::from(orange)] {
            a.update(dt);
            a.set_state(s);
        }
    }

    /// The icons, the box and its rule of `MechListBoxItem::render` (the texts are the caller's).
    pub(crate) fn render_art(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        orange: bool,
        icon_index: i64,
        dim: bool,
    ) {
        let colours: [Option<u32>; PART_COUNT] =
            std::array::from_fn(|i| self.art_colour(orange, i));
        let (u, v, u2, v2) = cell_uvs(
            icon_index,
            10,
            self.mech_icon.width(),
            self.mech_icon.height(),
        );
        self.mech_icon.file_width = 256.0;
        self.mech_icon.set_uvs(u, v, u2, v2);
        paint(&mut self.weight_icon, colours[0], ctx);
        if dim {
            self.mech_icon.set_color(0xa000_0000, false);
        }
        paint(
            &mut self.mech_icon,
            if dim { None } else { colours[1] },
            ctx,
        );
        paint(&mut self.cost_icon, colours[2], ctx);
        let g = &self.animations[usize::from(orange)][2];
        let edge = g.current_color(g.state());
        self.outline.obj.set_color(edge, false);
        self.outline.render(ctx);
        self.line.obj.set_color(edge, false);
        self.line.render(ctx);
    }
}

/// Draws an object in its animation colour (when it has one).
fn paint(o: &mut Object, colour: Option<u32>, ctx: &mut RenderCtx<'_>) {
    if let Some(c) = colour {
        o.set_color(c, false);
    }
    o.render(ctx);
}

/// Draws a text in its animation colour (when it has one).
pub(crate) fn paint_text(t: &mut Text, colour: Option<u32>, ctx: &mut RenderCtx<'_>) {
    if let Some(c) = colour {
        t.obj.set_color(c, false);
    }
    t.render(ctx);
}

/// `block_animation`: the animation index a block names.
fn block_animation(file: &mut FitIni, block: &str) -> i32 {
    if file.seek_block(block).is_err() {
        return -1;
    }
    file.string("Animation")
        .and_then(|s| s.chars().find(|c| c.is_ascii_digit()))
        .and_then(|c| c.to_digit(10))
        .map_or(-1, |d| d as i32)
}

/// One variant in the list (`MechListBoxItem`).
pub struct MechEntry {
    pub obj: Object,
    pub state: ItemState,
    /// `pMech`: the mech this entry stands for (the first one not in the force group).
    pub mech: MechKey,
    /// `mechCount`: how many of the variant the inventory holds.
    pub count: i32,
    /// `bIncludeForceGroup`: whether the count includes the force group.
    pub include_force_group: bool,
    /// `bOrange`.
    pub orange: bool,
    /// `bDim`: drawn dimmed (the mech is in use elsewhere).
    pub dim: bool,
    /// The list's `s_DrawCBills`: whether the cost column is drawn.
    pub draw_cbills: bool,
    /// `animTime`: the count's blink after it changed.
    anim_time: f32,
    /// The events of this frame, drained by the list's owner.
    pub events: Vec<EntryEvent>,
    /// `weight`, `chassis`, `name` the list sorts by.
    pub weight: f32,
    pub chassis_name: String,
    pub variant_name: String,
    /// Variant name contains "Prime" (sorts first among the same variant).
    pub prime: bool,
    variant: mc2_campaign::VariantKey,
    outline: Rect,
    line: Rect,
    weight_icon: Object,
    mech_icon: Object,
    cost_icon: Object,
    chassis_text: Text,
    weight_text: Text,
    count_text: Text,
    variant_text: Text,
    cost_text: Text,
    animations: [[AnimGroup; ANIMATION_COUNT]; 2],
    ids: [i32; PART_COUNT],
}

impl MechEntry {
    /// `MechListBoxItem(pMech, count)`: the entry for `mech`, its texts and mech picture filled from
    /// the campaign.
    pub fn new(
        t: &EntryTemplate,
        assets: &UiAssets,
        lg: &Logistics,
        mech: MechKey,
        include_force_group: bool,
    ) -> Option<MechEntry> {
        let m = lg.mech(mech)?;
        let chassis = lg.chassis_of(m.variant)?;
        let weight = lg.mech_max_weight(mech)?;
        let cost = lg.mech_cost(mech)?;
        let variant_name = lg.mech_name(mech)?.to_string();
        let chassis_name = assets.string(chassis.chassis_name_id as u32).to_string();
        let count = lg.variants_in_inventory(m.variant, include_force_group) as i32;

        let mut e = MechEntry {
            obj: Object::new(0.0, t.outline.obj.top(), t.width, t.height),
            state: ItemState::Enabled,
            mech,
            count,
            include_force_group,
            orange: false,
            dim: false,
            draw_cbills: true,
            anim_time: 0.0,
            events: Vec::new(),
            weight,
            chassis_name: chassis_name.clone(),
            prime: variant_name.contains("Prime"),
            variant_name: variant_name.clone(),
            variant: m.variant,
            outline: t.outline.clone(),
            line: t.line.clone(),
            weight_icon: t.weight_icon.clone(),
            mech_icon: t.mech_icon.clone(),
            cost_icon: t.cost_icon.clone(),
            chassis_text: t.chassis.clone(),
            weight_text: t.weight.clone(),
            count_text: t.count.clone(),
            variant_text: t.variant.clone(),
            cost_text: t.cost.clone(),
            animations: t.animations.clone(),
            ids: t.ids,
        };
        e.chassis_text.set_text(&chassis_name);
        e.cost_text.set_text(&cost.to_string());
        e.count_text.set_text(&count.to_string());
        e.variant_text.set_text(&variant_name);
        e.weight_text.set_text(&format!("{:.0}", weight));
        let icon = chassis.icon_picture_index;
        let (u, v, u2, v2) = cell_uvs(icon, 10, e.mech_icon.width(), e.mech_icon.height());
        e.mech_icon.file_width = 256.0;
        e.mech_icon.set_uvs(u, v, u2, v2);
        Some(e)
    }

    /// The mech's variant (`getVariant`).
    pub fn variant(&self) -> mc2_campaign::VariantKey {
        self.variant
    }

    /// Re-reads the count from the campaign (before the list updates).
    pub fn refresh(&mut self, lg: &Logistics) {
        self.count = lg.variants_in_inventory(self.variant, self.include_force_group) as i32;
    }

    /// `pointInside`: over the entry's box (edges excluded, as the C++ compares).
    pub fn point_inside_box(&self, x: i32, y: i32) -> bool {
        let r = self.outline.obj.global_rect();
        r.left < x && x < r.right && r.top < y && y < r.bottom
    }

    /// The entry's box in screen coordinates.
    pub fn box_rect(&self) -> GuiRect {
        self.outline.obj.global_rect()
    }

    /// Drawn with the mech icon dimmed (`bDim`).
    pub fn set_dim(&mut self, dim: bool) {
        self.dim = dim;
    }

    fn part_colour(&self, slot: usize) -> Option<u32> {
        let id = self.ids[slot];
        let i = usize::try_from(id).ok()?;
        let g = self.animations[usize::from(self.orange)].get(i)?;
        Some(g.current_color(g.state()))
    }

    fn set_all_states(&mut self, s: AnimState) {
        let set = usize::from(self.orange);
        for a in &mut self.animations[set] {
            a.set_state(s);
        }
    }

    /// `MechListBoxItem::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        // the count blinks for a second after it changes
        if self.count_text.text != self.count.to_string() {
            self.count_text.set_text(&self.count.to_string());
            self.anim_time = 0.0001;
        }
        if self.anim_time != 0.0 {
            let t = self.anim_time;
            let colour = if t < 0.25 || (t > 0.5 && t <= 0.75) {
                0
            } else {
                TEXT_COLOUR
            };
            self.count_text.set_color(colour);
            self.anim_time += ctx.dt;
            if self.anim_time > 1.0 {
                self.anim_time = 0.0;
            }
        }

        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let inside = self.point_inside_box(mx, my);
        let set = usize::from(self.orange);
        for a in &mut self.animations[set] {
            a.update(ctx.dt);
        }

        if self.state == ItemState::Selected {
            self.set_all_states(AnimState::Pressed);
            let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
            if ctx.input.is_left_drag() && self.point_inside_box(dx, dy) {
                self.events.push(EntryEvent::StartDrag(self.mech));
            }
        } else if self.state == ItemState::Highlite {
            self.set_all_states(AnimState::Highlight);
        } else if self.state == ItemState::Disabled && self.obj.show {
            if ctx.input.is_left_click() && inside {
                ctx.play(sfx::LOG_WRONGBUTTON);
                self.events.push(EntryEvent::SetMech(self.mech));
            }
            self.set_all_states(AnimState::Disabled);
        } else {
            self.set_all_states(AnimState::Normal);
        }

        if ctx.input.is_left_double_click()
            && inside
            && self.state != ItemState::Disabled
            && self.obj.show
        {
            self.events.push(EntryEvent::Add(self.mech));
        }
    }

    /// `MechListBoxItem::render`: the parts in their state colours, the dimmed icon, then the box
    /// and its rule. `draw_cbills` is the list's cost flag.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>, draw_cbills: bool) {
        let blink = self.anim_time != 0.0;
        let colours: [Option<u32>; PART_COUNT] = std::array::from_fn(|i| self.part_colour(i));
        paint(&mut self.weight_icon, colours[0], ctx);
        if self.dim {
            self.mech_icon.set_color(0xa000_0000, false);
        }
        paint(&mut self.mech_icon, colours[1], ctx);
        if draw_cbills {
            paint(&mut self.cost_icon, colours[2], ctx);
        }
        paint_text(&mut self.chassis_text, colours[3], ctx);
        paint_text(&mut self.weight_text, colours[4], ctx);
        if blink {
            self.count_text.render(ctx);
        } else {
            paint_text(&mut self.count_text, colours[5], ctx);
        }
        paint_text(&mut self.variant_text, colours[6], ctx);
        if draw_cbills {
            paint_text(&mut self.cost_text, colours[7], ctx);
        }

        let edge = self.part_colour_edge();
        self.outline.obj.set_color(edge, false);
        self.outline.render(ctx);
        self.line.obj.set_color(edge, false);
        self.line.render(ctx);
    }

    /// The box and rule take the state colour of the third animation (`animations[bOrange][2]`).
    fn part_colour_edge(&self) -> u32 {
        let g = &self.animations[usize::from(self.orange)][2];
        g.current_color(g.state())
    }

    /// Moves the entry (and its parts) by `dx, dy`.
    pub fn move_entry(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.outline.obj.move_by(dx, dy);
        self.line.obj.move_by(dx, dy);
        self.weight_icon.move_by(dx, dy);
        self.mech_icon.move_by(dx, dy);
        self.cost_icon.move_by(dx, dy);
        self.chassis_text.obj.move_by(dx, dy);
        self.weight_text.obj.move_by(dx, dy);
        self.count_text.obj.move_by(dx, dy);
        self.variant_text.obj.move_by(dx, dy);
        self.cost_text.obj.move_by(dx, dy);
    }
}

impl ListItem for MechEntry {
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
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        MechEntry::update(self, ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let draw = self.draw_cbills;
        MechEntry::render(self, ctx, draw);
    }
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.move_entry(dx, dy);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `MechListBox`: the list of variants, with the mech bay's options.
pub struct MechList {
    pub list: ListBox,
    /// `bDeleteIfNoInventory`: an entry whose count drops to zero leaves the list.
    pub delete_if_no_inventory: bool,
    /// `bIncludeForceGroup`.
    pub include_force_group: bool,
    /// `s_DrawCBills`: whether the cost column is drawn (the mech bay turns it off).
    pub draw_cbills: bool,
    /// `MechListBoxItem::bAddCalledThisFrame`: one double-click adds one mech per frame.
    add_called_this_frame: bool,
}

impl MechList {
    pub fn new(list: ListBox, delete_if_no_inventory: bool, include_force_group: bool) -> MechList {
        MechList {
            list,
            delete_if_no_inventory,
            include_force_group,
            draw_cbills: false,
            add_called_this_frame: false,
        }
    }

    /// The entries, in list order.
    pub fn entries(&self) -> impl Iterator<Item = &MechEntry> {
        self.list
            .items
            .iter()
            .filter_map(|i| i.as_any().downcast_ref::<MechEntry>())
    }

    pub fn entry_mut(&mut self, i: usize) -> Option<&mut MechEntry> {
        self.list
            .items
            .get_mut(i)
            .and_then(|b| b.as_any_mut().downcast_mut::<MechEntry>())
    }

    /// `GetItemCount`.
    pub fn item_count(&self) -> usize {
        self.list.items.len()
    }

    /// `removeAllItems(true)`.
    pub fn remove_all(&mut self) {
        self.list.remove_all_items();
    }

    /// `MechListBox::AddItem`: inserts `entry` in sort order (heaviest first, then by chassis and
    /// variant name, the "Prime" variants before their siblings).
    pub fn add(&mut self, entry: MechEntry) -> Option<usize> {
        let mut at = self.list.items.len();
        for (i, existing) in self.entries().enumerate() {
            let before = if existing.weight < entry.weight {
                true
            } else if existing.weight == entry.weight {
                let by_chassis = existing
                    .chassis_name
                    .as_str()
                    .cmp(entry.chassis_name.as_str());
                if by_chassis == std::cmp::Ordering::Greater {
                    true
                } else if by_chassis == std::cmp::Ordering::Equal {
                    entry.prime
                        || (!existing.prime
                            && existing.variant_name.as_str() > entry.variant_name.as_str())
                } else {
                    false
                }
            } else {
                false
            };
            if before {
                at = i;
                break;
            }
        }
        let mut entry = entry;
        entry.include_force_group = self.include_force_group;
        if at >= self.list.items.len() {
            self.list.add_item(Box::new(entry))
        } else {
            self.list.insert_item(Box::new(entry), at)
        }
    }

    /// `setOrange`: the orange colour set on the entries and the scroll bar.
    pub fn set_orange(&mut self, orange: bool) {
        for i in 0..self.list.items.len() {
            if let Some(e) = self.entry_mut(i) {
                e.orange = orange;
            }
        }
        if let Some(s) = self.list.scroll_bar.as_mut() {
            if orange {
                s.set_orange();
            } else {
                s.set_green();
            }
        }
    }

    /// `getCurrentMech`: the selected entry's mech.
    pub fn current_mech(&self) -> Option<MechKey> {
        let i = usize::try_from(self.list.selected_item()).ok()?;
        self.entries().nth(i).map(|e| e.mech)
    }

    pub fn select(&mut self, i: i32) {
        self.list.select_item(i);
    }

    pub fn selected(&self) -> i32 {
        self.list.selected_item()
    }

    /// `disableItemsThatCanNotGoInFG`: an entry whose mech cannot join the force group is
    /// disabled; a selection that becomes disabled moves to the first enabled entry.
    pub fn disable_items_that_cannot_go_in_force_group(&mut self, lg: &Logistics) {
        let sel = self.list.selected_item();
        let mut disabled_sel = false;
        for i in 0..self.list.items.len() {
            let Some(e) = self.entry_mut(i) else { continue };
            if !lg.can_add_mech_to_force_group(e.mech) {
                if sel == i as i32 {
                    disabled_sel = true;
                }
                e.state = ItemState::Disabled;
            } else if e.state == ItemState::Disabled {
                e.state = ItemState::Enabled;
            }
        }
        if disabled_sel {
            self.select_first_enabled_or_none();
        }
    }

    fn select_first_enabled_or_none(&mut self) {
        let first = (0..self.list.items.len()).find(|&i| {
            self.entries()
                .nth(i)
                .is_some_and(|e| e.state != ItemState::Disabled)
        });
        match first {
            Some(i) => {
                self.list.select_item(i as i32);
            }
            None => {
                self.list.select_item(-1);
            }
        }
    }

    /// `MechListBox::update`: counts are re-read, entries update, then the ones with no mech left
    /// go (when the list deletes them), and the selection is kept on an enabled entry.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &Logistics) -> Vec<EntryEvent> {
        for i in 0..self.list.items.len() {
            if let Some(e) = self.entry_mut(i) {
                e.refresh(lg);
            }
        }
        self.list.update(ctx);
        self.add_called_this_frame = false;

        let mut events = Vec::new();
        for i in 0..self.list.items.len() {
            if let Some(e) = self.entry_mut(i) {
                events.append(&mut e.events);
            }
        }
        // a double-click adds one mech per frame (the list scrolls under the pointer)
        if events.iter().any(|e| matches!(e, EntryEvent::Add(_))) {
            if self.add_called_this_frame {
                events.retain(|e| !matches!(e, EntryEvent::Add(_)));
            } else {
                self.add_called_this_frame = true;
                let first = events.iter().position(|e| matches!(e, EntryEvent::Add(_)));
                if let Some(first) = first {
                    let keep = events[first];
                    events.retain(|e| !matches!(e, EntryEvent::Add(_)));
                    events.insert(0, keep);
                }
            }
        }

        if self.delete_if_no_inventory {
            let mut i = 0;
            while i < self.list.items.len() {
                let gone = self.entry_mut(i).is_some_and(|e| e.count == 0);
                if gone {
                    self.list.remove_item(i);
                    self.disable_items_that_cannot_go_in_force_group(lg);
                    let sel = self.list.selected_item();
                    let bad = usize::try_from(sel)
                        .ok()
                        .and_then(|s| self.entries().nth(s))
                        .is_none_or(|e| e.state == ItemState::Disabled);
                    if self.list.items.is_empty() {
                        self.list.select_item(-1);
                    } else if sel == -1 || sel as usize >= self.list.items.len() || bad {
                        self.select_first_enabled_or_none();
                    }
                } else {
                    i += 1;
                }
            }
        }
        events
    }

    /// `render`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let draw = self.draw_cbills;
        for i in 0..self.list.items.len() {
            if let Some(e) = self.entry_mut(i) {
                e.draw_cbills = draw;
            }
        }
        self.list.render(ctx);
    }
}
