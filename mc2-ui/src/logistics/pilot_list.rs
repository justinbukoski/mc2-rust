// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `LogisticsPilotListBox` and `LogisticsPilotListBoxItem` (`code/logisticspilotlistbox.cpp`): the
//! pilots not yet in a mech, in the pilot ready room. Sorted by rank, then gunnery, then name; a
//! pilot is listed once. A click shows the pilot, a double-click puts it in the selected mech, a
//! drag takes it to a mech's slot.

use super::mech_icon::cell_uvs;
use super::strings::{ACE, GREEN};
use crate::anim::AnimGroup;
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::widgets::{ItemState, ListBox, ListItem, Object, Rect, Text};
use mc2_campaign::Logistics;
use mc2_formats::ini::FitIni;
use std::any::Any;

/// `PILOT_LIST_BOX_CHILD_COUNT`: name, rank, icon, rank icon, outline, line, pilot outline.
const CHILD_COUNT: usize = 7;

/// What a pilot entry asks of the pilot ready screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PilotEvent {
    /// The entry was clicked: show the pilot.
    Show(usize),
    /// The entry was double-clicked: put the pilot in the selected mech.
    Add(usize),
    /// The entry was dragged: start a drag of the pilot.
    StartDrag(usize),
}

/// The parts of a pilot entry (`LogisticsPilotListBoxItem::s_templateItem`).
#[derive(Clone)]
pub struct PilotEntryTemplate {
    pub width: f32,
    pub height: f32,
    name: Text,
    rank: Text,
    icon: Object,
    rank_icon: Object,
    outline: Rect,
    line: Rect,
    pilot_outline: Rect,
    anims: [AnimGroup; 3],
    /// The animation group of each child, in child order (`pChildAnimations`).
    child_anims: [i32; CHILD_COUNT],
}

impl PilotEntryTemplate {
    /// The portrait of a drag: a copy of the entry's portrait, set to the pilot's cell by the
    /// caller.
    pub fn drag_icon(&self) -> Object {
        self.icon.clone()
    }

    /// `LogisticsPilotListBoxItem::init`: `mcl_pr_availablepilotentry.fit`.
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni) -> PilotEntryTemplate {
        let outline = Rect::from_fit(file, "Rect0");
        let (width, height) = (outline.obj.width(), outline.obj.height());
        let name = Text::from_fit(ld, file, "Text0");
        let rank = Text::from_fit(ld, file, "Text1");
        let icon = Object::from_fit(ld, file, "Static0");
        let rank_icon = Object::from_fit(ld, file, "Static1");
        let pilot_outline = Rect::from_fit(file, "Rect1");
        let line = Rect::from_fit(file, "Rect2");
        // the animation of each part, by slot: name, rank, icon, rank icon, outline, pilot
        // outline, line (the C++ assigns the children in this order and reads them back by
        // child index, so the line and the pilot outline swap their groups)
        let slots = [
            "Text0", "Text1", "Static0", "Static1", "Rect0", "Rect1", "Rect2",
        ];
        let slot_anims = slots.map(|b| block_animation(file, b));
        let anims = std::array::from_fn(|i| AnimGroup::from_block(file, &format!("Animation{i}")));
        PilotEntryTemplate {
            width,
            height,
            name,
            rank,
            icon,
            rank_icon,
            outline,
            line,
            pilot_outline,
            anims,
            child_anims: [
                slot_anims[0],
                slot_anims[1],
                slot_anims[2],
                slot_anims[3],
                slot_anims[4],
                slot_anims[5],
                slot_anims[6],
            ],
        }
    }
}

/// The animation group a block names: its first digit, or -1.
fn block_animation(file: &mut FitIni, block: &str) -> i32 {
    if file.seek_block(block).is_err() {
        return -1;
    }
    file.read_string("Animation", 63)
        .ok()
        .and_then(|s| s.chars().find(|c| c.is_ascii_digit()))
        .and_then(|c| c.to_digit(10))
        .map_or(-1, |d| d as i32)
}

/// One pilot in the list (`LogisticsPilotListBoxItem`).
pub struct PilotEntry {
    pub obj: Object,
    pub state: ItemState,
    /// The pilot's index in the campaign's roster.
    pub pilot: usize,
    pub rank: i32,
    pub gunnery: f32,
    /// The pilot's name, upper case.
    pub name: String,
    photo: i64,
    name_text: Text,
    rank_text: Text,
    icon: Object,
    rank_icon: Object,
    outline: Rect,
    line: Rect,
    pilot_outline: Rect,
    anims: [AnimGroup; 3],
    child_anims: [i32; CHILD_COUNT],
    /// Events of this frame, drained by the owner.
    pub events: Vec<PilotEvent>,
}

impl PilotEntry {
    /// `LogisticsPilotListBoxItem(pilot)`: the entry's name, rank and portrait cell.
    pub fn new(
        t: &PilotEntryTemplate,
        assets: &UiAssets,
        lg: &Logistics,
        pilot: usize,
    ) -> Option<PilotEntry> {
        let p = lg.pilots().get(pilot)?;
        let name = assets.string(p.name_id as u32).to_uppercase();
        let rank = p.rank;
        let mut e = PilotEntry {
            obj: Object::new(0.0, 0.0, t.width, t.height),
            state: ItemState::Enabled,
            pilot,
            rank,
            gunnery: p.gunnery,
            name: name.clone(),
            photo: p.photo_index,
            name_text: t.name.clone(),
            rank_text: t.rank.clone(),
            icon: t.icon.clone(),
            rank_icon: t.rank_icon.clone(),
            outline: t.outline.clone(),
            line: t.line.clone(),
            pilot_outline: t.pilot_outline.clone(),
            anims: t.anims.clone(),
            child_anims: t.child_anims,
            events: Vec::new(),
        };
        e.name_text.set_text(&name);
        let rank_name = if rank != 4 {
            assets.string(GREEN + rank as u32)
        } else {
            assets.string(ACE)
        };
        e.rank_text.set_text(rank_name);
        let (rx, ry) = (rank as f32 * 15.0, 96.0);
        e.rank_icon.set_uvs(rx, ry, rx + 15.0, ry + 15.0);
        set_portrait(&mut e.icon, p.photo_index);
        Some(e)
    }

    /// The pilot's index as the campaign numbers it.
    pub fn pilot(&self) -> usize {
        self.pilot
    }

    /// The pilot's portrait cell.
    pub fn photo(&self) -> i64 {
        self.photo
    }

    /// `pointInside`: the entry's box.
    pub fn point_inside_box(&self, x: i32, y: i32) -> bool {
        self.obj.point_inside(x, y)
    }

    /// The entry's box in screen coordinates.
    pub fn box_rect(&self) -> GuiRect {
        self.obj.global_rect()
    }

    /// The colour of child `child`: its animation group in the entry's state.
    fn colour(&self, child: usize) -> u32 {
        let id = self.child_anims[child];
        match usize::try_from(id).ok().and_then(|i| self.anims.get(i)) {
            Some(g) => g.current_color(self.state.anim_state()),
            None => 0xffff_ffff,
        }
    }

    /// `LogisticsPilotListBoxItem::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let inside = self.point_inside_box(mx, my);
        for a in &mut self.anims {
            a.update(ctx.dt);
        }
        match self.state {
            ItemState::Selected => {
                for a in &mut self.anims {
                    a.set_state(crate::anim::AnimState::Pressed);
                }
                if ctx.input.is_left_double_click() && inside {
                    self.events.push(PilotEvent::Show(self.pilot));
                    self.events.push(PilotEvent::Add(self.pilot));
                }
                if ctx.input.is_left_click() && inside {
                    self.events.push(PilotEvent::Show(self.pilot));
                }
                let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
                if ctx.input.is_left_drag() && inside && self.point_inside_box(dx, dy) {
                    self.events.push(PilotEvent::StartDrag(self.pilot));
                    self.icon.set_color(0x7f00_0000, false);
                } else if !ctx.input.is_left_drag() {
                    self.icon.set_color(0xffff_ffff, false);
                }
            }
            ItemState::Highlite => {
                for a in &mut self.anims {
                    a.set_state(crate::anim::AnimState::Highlight);
                }
            }
            _ => {
                for a in &mut self.anims {
                    a.set_state(crate::anim::AnimState::Normal);
                }
            }
        }
    }

    /// `render`: each part in its animation colour for the entry's state.
    pub fn render_entry(&mut self, ctx: &mut RenderCtx<'_>) {
        let colours: [u32; CHILD_COUNT] = std::array::from_fn(|i| self.colour(i));
        self.name_text.obj.set_color(colours[0], false);
        self.name_text.render(ctx);
        self.rank_text.obj.set_color(colours[1], false);
        self.rank_text.render(ctx);
        self.icon.set_color(colours[2], false);
        self.icon.render(ctx);
        self.rank_icon.set_color(colours[3], false);
        self.rank_icon.render(ctx);
        self.outline.obj.set_color(colours[4], false);
        self.outline.render(ctx);
        self.line.obj.set_color(colours[5], false);
        self.line.render(ctx);
        self.pilot_outline.obj.set_color(colours[6], false);
        self.pilot_outline.render(ctx);
    }
}

/// `LogisticsPilotListBox::makeUVs`: the portrait cell of a pilot (12 to a row).
pub fn set_portrait(icon: &mut Object, photo: i64) {
    let (u, v, u2, v2) = cell_uvs(photo, 12, icon.width(), icon.height());
    icon.file_width = 256.0;
    icon.set_uvs(u, v, u2, v2);
}

impl ListItem for PilotEntry {
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
        PilotEntry::update(self, ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.render_entry(ctx);
    }
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.name_text.obj.move_by(dx, dy);
        self.rank_text.obj.move_by(dx, dy);
        self.icon.move_by(dx, dy);
        self.rank_icon.move_by(dx, dy);
        self.outline.obj.move_by(dx, dy);
        self.line.obj.move_by(dx, dy);
        self.pilot_outline.obj.move_by(dx, dy);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `LogisticsPilotListBox`: the pilots that are not in a mech.
pub struct PilotList {
    pub list: ListBox,
}

impl PilotList {
    pub fn new(list: ListBox) -> PilotList {
        PilotList { list }
    }

    /// The entries in list order.
    pub fn entries(&self) -> impl Iterator<Item = &PilotEntry> {
        self.list
            .items
            .iter()
            .filter_map(|i| i.as_any().downcast_ref::<PilotEntry>())
    }

    pub fn entry_mut(&mut self, i: usize) -> Option<&mut PilotEntry> {
        self.list
            .items
            .get_mut(i)
            .and_then(|b| b.as_any_mut().downcast_mut::<PilotEntry>())
    }

    pub fn item_count(&self) -> usize {
        self.list.items.len()
    }

    /// `removeAllItems(true)`.
    pub fn remove_all(&mut self) {
        self.list.remove_all_items();
    }

    /// `LogisticsPilotListBox::AddItem`: a pilot already listed by name is not added twice; the
    /// rest go in rank order, then gunnery, then name (the one that sorts first goes first).
    pub fn add(&mut self, entry: PilotEntry) -> Option<usize> {
        if let Some(bar) = self.list.scroll_bar.as_mut() {
            bar.set_orange();
        }
        let mut at = None;
        for (i, existing) in self.entries().enumerate() {
            if existing.name == entry.name {
                return None;
            }
            let before = entry.rank > existing.rank
                || (entry.rank == existing.rank
                    && (entry.gunnery > existing.gunnery
                        || (entry.gunnery == existing.gunnery && entry.name < existing.name)));
            if before {
                at = Some(i);
                break;
            }
        }
        match at {
            Some(i) => self.list.insert_item(Box::new(entry), i),
            None => self.list.add_item(Box::new(entry)),
        }
    }

    /// `removePilot`: the entry of `pilot` leaves the list.
    pub fn remove_pilot(&mut self, pilot: usize) {
        let at = self.entries().position(|e| e.pilot == pilot);
        if let Some(i) = at {
            self.list.remove_item(i);
        }
    }

    /// `LogisticsPilotListBox::update`: the list updates, then the pilots that have been put in a
    /// mech leave it. Returns the events of this frame.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &Logistics) -> Vec<PilotEvent> {
        self.list.update(ctx);
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.list.items.len() {
            let Some(e) = self.entry_mut(i) else {
                i += 1;
                continue;
            };
            events.append(&mut e.events);
            let used = lg.pilots().get(e.pilot).is_some_and(|p| p.used);
            if used {
                self.list.remove_item(i);
            } else {
                i += 1;
            }
        }
        events
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.list.render(ctx);
    }
}
