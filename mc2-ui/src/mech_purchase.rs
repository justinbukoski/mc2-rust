// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mech purchase screen (`code/mechpurchasescreen.cpp`) over `mcl_m$.fit`: the mechs the
//! player owns on the left, the mechs for sale on the right, C-Bills at the top.
//!
//! Buying (Add) and selling (Remove) go through the campaign (`purchase_mech`, `sell_mech`,
//! `can_purchase_mech`), which moves the C-Bills. Accept keeps the purchases; Cancel (and any
//! exit other than the main menu) sells everything and buys back the starting inventory, as
//! `MechPurchaseScreen::end` does. Drag and drop between the lists and the list's double click
//! (`MechPurchaseScreen::beginDrag`/`endDrag`) and the double click of a mech for sale
//! (`MechListBoxItem::doAdd`) buy and sell like the Add and Remove buttons. The selected mech is
//! shown by the mech display widget.

use crate::ctx::{sfx, Loader, RenderCtx, UpdateCtx};
use crate::fit::FitExt;
use crate::logistics::mech_display::MechDisplay;
use crate::logistics::mech_list::paint_text;
use crate::logistics::mech_list::EntryTemplate;
use crate::logistics_dialog::DISABLED_ROW_COLOUR;
use crate::logistics_ids::button;
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::widgets::{ItemState, ListBox, ListItem, Object, Text};
use mc2_campaign::{Logistics, MechKey, VariantKey};
use mc2_formats::ini::FitIni;
use std::any::Any;

/// `helpTextArrayID` of the purchase screen.
const HELP_TEXT: i32 = 4;

/// A mech the player had when the screen opened (`prevInventory`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owned {
    variant: VariantKey,
    pilot: Option<usize>,
    force_group: i32,
}

/// A mech picked up from one of the lists (`pDragMech`, `dragStartLeft`, `dragIcon`).
struct Drag {
    variant: VariantKey,
    /// The drag started in the owned list (`dragStartLeft`).
    from_inventory: bool,
    icon: Object,
}

/// The purchase screen.
pub struct MechPurchase {
    pub screen: LogisticsScreen,
    /// `inventoryListBox`: one row per mech design the player owns outside the force group.
    pub inventory: ListBox,
    /// `variantListBox`: the mechs for sale.
    pub variants: ListBox,
    /// The status the screen reports (`status`): running, down (done), main menu.
    pub status: Status,
    /// `acceptPressed`: the purchases stay.
    accept_pressed: bool,
    /// `prevInventory`: what the player had when the screen opened.
    previous: Vec<Owned>,
    /// The design of each inventory row.
    inventory_rows: Vec<VariantKey>,
    /// The design of each variant row.
    variant_rows: Vec<VariantKey>,
    /// The row every mech list entry is copied from.
    template: MechEntry,
    /// The colours of the two lists: the headings' colours of the layout.
    inventory_colour: u32,
    variant_colour: u32,
    /// The mech being dragged between the lists.
    drag: Option<Drag>,
    /// The parts of a mech list entry, for the drag icon (`MechListBox::initIcon`).
    icon_template: EntryTemplate,
    /// `mechDisplay`: the statistics, loadout and 3D picture of the picked mech.
    pub display: MechDisplay,
}

impl MechPurchase {
    /// `MechPurchaseScreen::init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<MechPurchase, String> {
        let mut screen = LogisticsScreen::load(ld, "mcl_m$", Prefixes::default())?;
        screen.help_text_array_id = HELP_TEXT;
        // every button acts on release, and the buttons play the click (`init`)
        for b in &mut screen.buttons {
            b.set_message_on_release();
            b.set_press_fx(sfx::LOG_CLICKONBUTTON);
        }
        let list = |screen: &LogisticsScreen, i: usize| {
            screen
                .rect(i)
                .map(|r| (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height()))
        };
        let (x0, y0, w0, h0) = list(&screen, 0).unwrap_or_default();
        let (x1, y1, w1, h1) = list(&screen, 1).unwrap_or_default();
        let mut inventory = ListBox::new(ld, x0, y0, w0, h0);
        inventory.set_orange();
        let variants = ListBox::new(ld, x1, y1, w1, h1);
        let mut entry = ld.art_fit("mcl_gn_availablemechentry")?;
        let template = MechEntry::from_fit(ld, &mut entry);
        let icon_template = EntryTemplate::load(ld, &mut entry);
        let colour = |i: usize| screen.texts.get(i).map_or(0xffff_ffff, |t| t.obj.color());
        let (inventory_colour, variant_colour) = (colour(2), colour(3));
        Ok(MechPurchase {
            screen,
            inventory,
            variants,
            status: Status::Running,
            accept_pressed: false,
            previous: Vec::new(),
            inventory_rows: Vec::new(),
            variant_rows: Vec::new(),
            template,
            inventory_colour,
            variant_colour,
            drag: None,
            icon_template,
            display: MechDisplay::load(ld)?,
        })
    }

    /// `begin`: the owned designs and the mechs for sale; the first owned design is selected,
    /// else the first mech for sale. Remembers the inventory for Cancel.
    pub fn begin(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics) {
        self.status = Status::Running;
        self.accept_pressed = false;
        self.drag = None;
        self.previous = logistics
            .inventory()
            .iter()
            .filter(|m| m.force_group == 0)
            .map(|m| Owned {
                variant: m.variant,
                pilot: m.pilot,
                force_group: m.force_group,
            })
            .collect();
        self.screen.begin();
        self.rebuild(ctx, logistics);
        // `begin`: the first owned design, else the first mech for sale
        if !self.inventory_rows.is_empty() {
            self.inventory.select_item(0);
        } else if !self.variant_rows.is_empty() {
            self.variants.select_item(0);
        }
    }

    /// Rebuilds both lists from the campaign: one row per owned design (outside the force
    /// group), one per mech for sale (greyed out when the player cannot pay for it). The
    /// selections stay on the same rows when they still exist, as the list rows do in the
    /// original.
    fn rebuild(&mut self, ctx: &UpdateCtx<'_>, logistics: &Logistics) {
        let (inv_pick, var_pick) = (
            self.inventory.selected_item(),
            self.variants.selected_item(),
        );
        self.inventory.remove_all_items();
        self.variants.remove_all_items();
        self.inventory_rows.clear();
        self.variant_rows.clear();
        for m in logistics.inventory() {
            if m.force_group == 0 && !self.inventory_rows.contains(&m.variant) {
                self.inventory_rows.push(m.variant);
            }
        }
        for &v in &self.inventory_rows {
            let mut row = self.entry(ctx, logistics, v, self.inventory_colour, false);
            row.orange = true;
            self.inventory.add_item(Box::new(row));
        }
        for v in logistics.purchasable_mechs() {
            let affordable = logistics.can_purchase_mech(v).is_ok();
            let row = self.entry(ctx, logistics, v, self.variant_colour, !affordable);
            self.variants.add_item(Box::new(row));
            self.variant_rows.push(v);
        }
        if let Ok(i) = usize::try_from(inv_pick) {
            if i < self.inventory_rows.len() {
                self.inventory.select_item(i as i32);
            }
        }
        if let Ok(i) = usize::try_from(var_pick) {
            if i < self.variant_rows.len() {
                self.variants.select_item(i as i32);
            }
        }
    }

    /// One row of a mech list for design `v`: the chassis, the tonnage, the mechs owned outside
    /// the force group, the name and the cost (`MechListBoxItem`'s texts), greyed out when
    /// `disabled`.
    fn entry(
        &self,
        ctx: &UpdateCtx<'_>,
        logistics: &Logistics,
        v: VariantKey,
        colour: u32,
        disabled: bool,
    ) -> MechEntry {
        let mut row = self.template.clone();
        let chassis = logistics.variant_chassis(v);
        row.set_texts(
            &chassis.map_or_else(String::new, |c| {
                ctx.assets.string(c.chassis_name_id as u32).to_string()
            }),
            &chassis.map_or_else(String::new, |c| format!("{:.0}", c.max_weight)),
            &logistics.variants_in_inventory(v, false).to_string(),
            logistics.variant(v).map_or("", |v| v.name.as_str()),
            &logistics.variant_cost(v).unwrap_or(0).to_string(),
        );
        row.set_colour(colour, disabled);
        row.icon_index = chassis.map_or(0, |c| c.icon_picture_index);
        row
    }

    /// `selectFirstBuyableMech`.
    fn select_first_buyable(&mut self, logistics: &Logistics) -> bool {
        let first = self
            .variant_rows
            .iter()
            .position(|&v| logistics.can_purchase_mech(v).is_ok());
        match first {
            Some(i) => {
                self.variants.select_item(i as i32);
                true
            }
            None => false,
        }
    }

    /// `MechPurchaseScreen::update`: the buttons, the list selections (one list at a time), the
    /// enables and the C-Bills.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        if self.status != Status::Running {
            return;
        }
        for m in self.screen.update(ctx) {
            self.handle(ctx, logistics, m.who);
            if self.status != Status::Running {
                return;
            }
        }
        let before_inv = self.inventory.selected_item();
        self.inventory.update(ctx);
        if self.inventory.selected_item() != before_inv && self.inventory.selected_item() != -1 {
            self.variants.select_item(-1);
        }
        let before_var = self.variants.selected_item();
        self.variants.update(ctx);
        if self.variants.selected_item() != before_var && self.variants.selected_item() != -1 {
            self.inventory.select_item(-1);
        }
        self.disable_items_that_cost_more(logistics);
        self.update_drag(ctx, logistics);
        self.refresh(logistics);
        self.display.update_display(ctx);
        self.show_pick(ctx, logistics);
    }

    /// The `setMech` of `MechPurchaseScreen::update` (`mechpurchasescreen.cpp:171-181`): the mech
    /// for sale that can be paid for, else the picked owned one.
    fn show_pick(&mut self, ctx: &UpdateCtx<'_>, logistics: &Logistics) {
        let buy = self.affordable_pick(logistics);
        let sell = usize::try_from(self.inventory.selected_item())
            .ok()
            .and_then(|row| self.inventory_rows.get(row).copied());
        if let Some(v) = buy.or(sell) {
            self.display.set_variant(ctx.assets, logistics, Some(v));
        }
    }

    /// `MechListBox::disableItemsThatCostMoreThanRP`: a design dearer than the C-Bills is
    /// greyed out each frame (and enabled again once it is affordable); a picked row that was
    /// greyed out gives the pick to the first row that is not, else to none.
    fn disable_items_that_cost_more(&mut self, logistics: &Logistics) {
        let c_bills = logistics.c_bills();
        let mut disabled_pick = false;
        let picked = self.variants.selected_item();
        for (i, &v) in self.variant_rows.iter().enumerate() {
            let Some(item) = self.variants.item_mut(i) else {
                continue;
            };
            if i64::from(logistics.variant_cost(v).unwrap_or(0)) > c_bills {
                item.set_state(ItemState::Disabled);
                disabled_pick |= picked == i as i32;
            } else if item.state() == ItemState::Disabled {
                item.set_state(ItemState::Enabled);
            }
        }
        if disabled_pick {
            let first = (0..self.variant_rows.len()).find(|&i| {
                self.variants
                    .item(i)
                    .is_some_and(|it| it.state() != ItemState::Disabled)
            });
            self.variants.select_item(first.map_or(-1, |i| i as i32));
        }
    }

    /// The list row under `(x, y)`: `(owned list, row, enabled)`.
    fn row_at(&self, x: i32, y: i32) -> Option<(bool, usize, bool)> {
        for (owned, list) in [(true, &self.inventory), (false, &self.variants)] {
            if !list.point_inside(x, y) {
                continue;
            }
            for i in 0..list.item_count() {
                let item = list.item(i)?;
                if item.obj().point_inside(x, y) {
                    return Some((owned, i, item.state() != ItemState::Disabled));
                }
            }
        }
        None
    }

    /// The drag and the double click of `MechListBoxItem::update` and `MechPurchaseScreen::update`
    /// (`mechlistbox.cpp:355-390`, `mechpurchasescreen.cpp:193-205`): a drag of the picked row
    /// lifts its mech (`beginDrag`), the icon follows the pointer, and releasing it over the owned
    /// list buys (from the sale list), anywhere else sells (from the owned list); a double click on
    /// an enabled row acts as Add.
    fn update_drag(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if let Some(drag) = self.drag.as_mut() {
            let (w, h) = (drag.icon.width(), drag.icon.height());
            drag.icon.move_to(mx as f32 - w / 2.0, my as f32 - h / 2.0);
            if ctx.input.left_mouse_released() {
                let drag = self.drag.take().expect("dragging");
                let over_owned = self.inventory.point_inside(mx, my);
                if over_owned && !drag.from_inventory {
                    if logistics.purchase_mech(drag.variant).is_ok() {
                        self.rebuild(ctx, logistics);
                    }
                } else if !over_owned && drag.from_inventory {
                    self.sell_variant(ctx, logistics, drag.variant);
                }
            }
            return;
        }
        let origin = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        if ctx.input.is_left_drag() {
            // only the picked row of a list starts a drag, and not a greyed-out one
            let picked = |list: &ListBox| usize::try_from(list.selected_item()).ok();
            let lifted = [
                (
                    true,
                    picked(&self.inventory),
                    &self.inventory,
                    &self.inventory_rows,
                ),
                (
                    false,
                    picked(&self.variants),
                    &self.variants,
                    &self.variant_rows,
                ),
            ]
            .into_iter()
            .find_map(|(owned, row, list, rows)| {
                let row = row?;
                let item = list.item(row)?;
                (item.state() != ItemState::Disabled && item.obj().point_inside(origin.0, origin.1))
                    .then(|| rows.get(row).map(|&v| (owned, v)))
                    .flatten()
            });
            if let Some((from_inventory, variant)) = lifted {
                let icon = logistics
                    .chassis_of(variant)
                    .map_or(0, |c| c.icon_picture_index);
                self.drag = Some(Drag {
                    variant,
                    from_inventory,
                    icon: self.icon_template.drag_icon(icon),
                });
            }
        } else if ctx.input.is_left_double_click() {
            if let Some((_, _, true)) = self.row_at(mx, my) {
                // `doAdd` → `addSelectedMech`: with no mech for sale picked, the owned list's
                // first row is
                self.add_selected(ctx, logistics);
                if self.variants.selected_item() == -1 {
                    self.inventory.select_item(0);
                }
            }
        }
    }

    /// `handleMessage`: Add buys the selected design, Remove sells the selected one, Accept and
    /// Cancel leave the screen (`DOWN`), Main Menu goes to the menu.
    fn handle(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics, who: i32) {
        match who {
            button::MB_MSG_ADD => self.add_selected(ctx, logistics),
            button::MB_MSG_REMOVE => self.remove_selected(ctx, logistics),
            button::MB_MSG_NEXT => {
                self.accept_pressed = true;
                self.status = Status::Down;
            }
            button::MB_MSG_PREV => self.status = Status::Down,
            button::MB_MSG_MAINMENU => self.status = Status::MainMenu,
            _ => {}
        }
    }

    /// The design of the selected variant row, if it can be bought now.
    fn affordable_pick(&self, logistics: &Logistics) -> Option<VariantKey> {
        let row = usize::try_from(self.variants.selected_item()).ok()?;
        let v = *self.variant_rows.get(row)?;
        logistics.can_purchase_mech(v).is_ok().then_some(v)
    }

    /// `addSelectedMech` → `addMech`: buys the design and lists it among the owned ones.
    fn add_selected(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        let Some(v) = self.affordable_pick(logistics) else {
            return;
        };
        if logistics.purchase_mech(v).is_ok() {
            self.rebuild(ctx, logistics);
            // `addSelectedMech`: with no mech for sale picked, the owned list's first row is
            if self.variants.selected_item() == -1 {
                self.inventory.select_item(0);
            }
        }
    }

    /// `removeSelectedMech` → `removeMech`: sells one mech of the selected design; the design
    /// leaves the list once none is left.
    fn remove_selected(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        let Ok(row) = usize::try_from(self.inventory.selected_item()) else {
            return;
        };
        let Some(&v) = self.inventory_rows.get(row) else {
            return;
        };
        self.sell_variant(ctx, logistics, v);
    }

    /// `removeMech`: sells one mech of design `v`.
    fn sell_variant(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics, v: VariantKey) {
        let Some(mech) = sellable_mech(logistics, v) else {
            return;
        };
        if logistics.sell_mech(mech).is_ok() {
            self.rebuild(ctx, logistics);
            if self.inventory_rows.is_empty() {
                self.select_first_buyable(logistics);
            } else if self.inventory.selected_item() == -1 {
                self.inventory.select_item(0);
            }
        }
    }

    /// The enables of Add (a design for sale that can be paid for is picked) and Remove (an owned
    /// design is picked), and the C-Bills.
    fn refresh(&mut self, logistics: &Logistics) {
        let inv_pick = self.inventory.selected_item() != -1;
        let buy = self.affordable_pick(logistics).is_some();
        if let Some(b) = self.screen.button_mut(button::MB_MSG_ADD) {
            b.disable(!buy);
        }
        if let Some(b) = self.screen.button_mut(button::MB_MSG_REMOVE) {
            b.disable(!inv_pick);
        }
        if let Some(t) = self.screen.texts.get_mut(1) {
            t.set_text(&format!("{} ", logistics.c_bills()));
        }
    }

    /// `MechPurchaseScreen::end`: unless the player accepted, every mech that is not in the force
    /// group is sold and the starting inventory bought back.
    pub fn end(&mut self, logistics: &mut Logistics) {
        if self.accept_pressed || self.status == Status::MainMenu {
            return;
        }
        let bay: Vec<MechKey> = logistics.inventory().iter().rev().map(|m| m.key).collect();
        for key in bay {
            let _ = logistics.sell_mech(key);
        }
        for o in &self.previous {
            let _ =
                logistics.add_mech_to_inventory_with_pilot(o.variant, o.pilot, o.force_group, true);
        }
    }

    /// Draws the lists, the screen and the C-Bills.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.screen.render(ctx);
        self.inventory.render(ctx);
        self.variants.render(ctx);
        self.display.render_display(ctx);
        if let Some(drag) = &self.drag {
            drag.icon.render(ctx);
        }
    }

    /// The design being dragged, while a mech is lifted.
    pub fn dragging(&self) -> Option<VariantKey> {
        self.drag.as_ref().map(|d| d.variant)
    }
}

/// A row of a mech list (`MechListBoxItem`): the chassis, the tonnage, the number owned, the
/// name and the cost, each at its text's place in `mcl_gn_availablemechentry.fit`. The texts move
/// with the row, drawn with the entry's icons and its box in the state colours.
#[derive(Clone, Debug)]
pub struct MechEntry {
    obj: Object,
    state: ItemState,
    chassis: Text,
    weight: Text,
    owned: Text,
    name: Text,
    cost: Text,
    /// The icons, box and rule of the entry (`mcl_gn_availablemechentry.fit`).
    art: EntryTemplate,
    icon_index: i64,
    orange: bool,
}

impl MechEntry {
    /// `MechListBoxItem::init`: the row is the size of the outline `Rect0` (at its top, as
    /// `aObject::init(0, outline.top(), ...)`), and the texts `Text0` to `Text4` are placed
    /// from that origin.
    fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni) -> MechEntry {
        let _ = file.seek_block("Rect0");
        let obj = Object::new(
            0.0,
            file.long_or("YLocation", 0) as f32,
            file.long_or("Width", 0) as f32,
            file.long_or("Height", 0) as f32,
        );
        let art = EntryTemplate::load(ld, file);
        let mut text = |block: &str| Text::from_fit(ld, file, block);
        MechEntry {
            art,
            icon_index: 0,
            orange: false,
            obj,
            state: ItemState::Enabled,
            chassis: text("Text0"),
            weight: text("Text1"),
            owned: text("Text2"),
            name: text("Text3"),
            cost: text("Text4"),
        }
    }

    /// The chassis, tonnage, owned count, name and cost texts, in that order.
    pub fn texts(&self) -> [&Text; 5] {
        [
            &self.chassis,
            &self.weight,
            &self.owned,
            &self.name,
            &self.cost,
        ]
    }

    fn texts_mut(&mut self) -> [&mut Text; 5] {
        [
            &mut self.chassis,
            &mut self.weight,
            &mut self.owned,
            &mut self.name,
            &mut self.cost,
        ]
    }

    fn set_texts(&mut self, chassis: &str, weight: &str, owned: &str, name: &str, cost: &str) {
        let values = [chassis, weight, owned, name, cost];
        for (t, v) in self.texts_mut().into_iter().zip(values) {
            t.set_text(v);
        }
    }

    /// The row's colour, and its state: a design the player cannot pay for is greyed out.
    fn set_colour(&mut self, colour: u32, disabled: bool) {
        let colour = if disabled {
            DISABLED_ROW_COLOUR
        } else {
            colour
        };
        for t in self.texts_mut() {
            t.set_color(colour);
        }
        self.state = if disabled {
            ItemState::Disabled
        } else {
            ItemState::Enabled
        };
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
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.art.move_art(dx, dy);
        for t in self.texts_mut() {
            t.obj.move_by(dx, dy);
        }
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.art.update_art(self.state, self.orange, ctx.dt);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.obj.show {
            // the disabled look comes from the animation set (`bDim` is the drag dimming)
            self.art
                .render_art(ctx, self.orange, self.icon_index, false);
            let orange = self.orange;
            let colours: [Option<u32>; 5] =
                std::array::from_fn(|i| self.art.art_colour(orange, 3 + i));
            for (t, c) in self.texts_mut().into_iter().zip(colours) {
                paint_text(t, c, ctx);
            }
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A mech of design `v` outside the force group, for `sell_mech` (which picks the bay's last one
/// of that design itself).
fn sellable_mech(logistics: &Logistics, v: VariantKey) -> Option<MechKey> {
    logistics
        .inventory()
        .iter()
        .find(|m| m.variant == v && m.force_group == 0)
        .map(|m| m.key)
}
