// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `aListBox`, its items (`aListItem`, `aTextListItem`, `aAnimTextListItem`,
//! `aLocalizedListItem`), `aDropList` and `aComboBox` (`gui/alistbox.cpp`).
//!
//! Items are trait objects so other screens can supply their own (the logistics lists, the hot key
//! list); [`ListItem`] is the `aListItem` interface.

use super::basic::{font_height, font_render, font_text_height, font_width, Rect};
use super::button::AnimButton;
use super::edit::Edit;
use super::object::Object;
use super::scroll::ScrollBar;
use crate::anim::{AnimGroup, AnimState};
use crate::ctx::{msg, sfx, Loader, Msg, RenderCtx, UiAssets, UpdateCtx};
use crate::fit::FitExt;
use crate::font::FontRef;
use mc2_formats::ini::FitIni;
use mc2_platform::keys;
use std::any::Any;

pub const MAX_LIST_ITEMS: usize = 128;

/// `aListItem::State`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ItemState {
    #[default]
    Enabled = 0,
    Selected = 1,
    Highlite = 2,
    PressedHighlite = 3,
    Disabled = 4,
}

impl ItemState {
    pub fn anim_state(self) -> AnimState {
        AnimState::from_index(self as usize)
    }
}

/// `aListItem`.
pub trait ListItem: Any {
    fn obj(&self) -> &Object;
    fn obj_mut(&mut self) -> &mut Object;
    fn state(&self) -> ItemState;
    fn set_state(&mut self, s: ItemState);
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.obj().update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.obj().render(ctx);
    }
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj_mut().move_by(dx, dy);
    }
    fn resize(&mut self, w: f32, h: f32) {
        self.obj_mut().resize(w, h);
    }
    fn is_checked(&self) -> bool {
        false
    }
    fn set_check(&mut self, _checked: bool) {}
    /// The text of a text item (the original's `dynamic_cast<aTextListItem*>`).
    fn text(&self) -> Option<&str> {
        None
    }
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;

    fn move_to(&mut self, x: f32, y: f32) {
        let (dx, dy) = (x - self.obj().loc[0].x, y - self.obj().loc[0].y);
        self.move_by(dx, dy);
    }
    fn point_inside(&self, x: i32, y: i32) -> bool {
        self.obj().point_inside(x, y)
    }
    fn deselect(&mut self) {
        if self.state() != ItemState::Disabled {
            self.set_state(ItemState::Enabled);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// text items

/// `aTextListItem`.
#[derive(Clone, Debug, Default)]
pub struct TextListItem {
    pub obj: Object,
    pub state: ItemState,
    pub font: FontRef,
    pub text: String,
    pub alignment: i32,
    pub force_to_top: bool,
}

impl TextListItem {
    /// `aTextListItem(font)`: sized from the font (`"> "` width, 1.25 line heights tall).
    pub fn new(assets: &UiAssets, font: FontRef) -> TextListItem {
        let w = font_width(assets, font, "> ");
        let h = font_height(assets, font);
        TextListItem {
            obj: Object::new(
                w as f32,
                0.0,
                assets.screen.0 as f32,
                (h as f32 * 1.25) as i32 as f32,
            ),
            font,
            ..TextListItem::default()
        }
    }

    /// `aTextListItem::init(file, block)`: position, size and colour from `block`.
    pub fn init_fit(&mut self, file: &mut FitIni, block: &str) {
        let _ = file.seek_block(block);
        self.obj = Object::new(
            file.long_or("XLocation", 0) as f32,
            file.long_or("YLocation", 0) as f32,
            file.long_or("Width", 0) as f32,
            file.long_or("Height", 0) as f32,
        );
        if let Some(c) = file.color("Color") {
            self.obj.set_color(c, false);
        }
    }

    pub fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
    }

    pub fn set_text_id(&mut self, assets: &UiAssets, id: u32) {
        self.text = assets.string(id).to_string();
    }

    pub fn set_color(&mut self, c: u32) {
        self.obj.set_color(c, false);
    }

    /// `sizeToText`: keeps the width, takes the wrapped text height.
    pub fn size_to_text(&mut self, assets: &UiAssets) {
        let h = font_text_height(assets, self.font, &self.text, self.obj.width() as i32);
        let w = self.obj.width();
        self.obj.resize(w, h as f32);
    }

    /// `aTextListItem::render`: one-line text is centred vertically, wrapped text starts a
    /// quarter line down.
    pub fn render_text(&self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        let l = &self.obj.loc;
        let mut y = l[2].y - l[0].y;
        let line = font_height(ctx.assets, self.font) as f32;
        if y > line
            && font_text_height(ctx.assets, self.font, &self.text, self.obj.width() as i32) as f32
                <= line
            && !self.force_to_top
        {
            y = (l[2].y + l[0].y) / 2.0 - line / 2.0;
        } else {
            y = l[0].y + line / 4.0;
        }
        font_render(
            ctx,
            self.font,
            &self.text,
            l[0].x as i32,
            y as i32,
            (l[2].x - l[0].x) as i32,
            (l[2].y - l[0].y) as i32,
            l[0].argb,
            false,
            self.alignment,
        );
    }
}

impl ListItem for TextListItem {
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
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.render_text(ctx);
    }
    fn text(&self) -> Option<&str> {
        Some(&self.text)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `aAnimTextListItem`: a text item coloured by a per-state animation group.
#[derive(Clone, Debug, Default)]
pub struct AnimTextListItem {
    pub item: TextListItem,
    pub anim: AnimGroup,
}

impl AnimTextListItem {
    pub fn new(assets: &UiAssets, font: FontRef) -> AnimTextListItem {
        AnimTextListItem {
            item: TextListItem::new(assets, font),
            anim: AnimGroup::default(),
        }
    }

    /// `aAnimTextListItem::init(file, block = "Text0")`.
    pub fn init_fit(&mut self, file: &mut FitIni, block: &str) {
        self.item.init_fit(file, block);
        if let Some(a) = file.string("Animation") {
            self.anim = AnimGroup::from_block(file, &a);
        }
    }
}

impl ListItem for AnimTextListItem {
    fn obj(&self) -> &Object {
        &self.item.obj
    }
    fn obj_mut(&mut self) -> &mut Object {
        &mut self.item.obj
    }
    fn state(&self) -> ItemState {
        self.item.state
    }
    fn set_state(&mut self, s: ItemState) {
        self.item.state = s;
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.anim.update(ctx.dt);
        self.item.obj.update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let s = self.item.state.anim_state();
        self.anim.set_state(s);
        let c = self.anim.current_color(s);
        self.item.obj.set_color(c, false);
        self.item.render_text(ctx);
    }
    fn text(&self) -> Option<&str> {
        Some(&self.item.text)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// `aLocalizedListItem`: an animated text item whose font, text and position come from a block.
#[derive(Clone, Debug, Default)]
pub struct LocalizedListItem {
    pub item: AnimTextListItem,
    pub hidden_text: String,
}

impl LocalizedListItem {
    /// `aLocalizedListItem::init`. As in the original, the block's `Color` is read but not
    /// applied (the animation sets the colour).
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> LocalizedListItem {
        let _ = file.seek_block(block);
        let x = file.long_or("XLocation", 0) as f32;
        let y = file.long_or("YLocation", 0) as f32;
        let font = ld.font(file.long_or("Font", 0) as u32);
        let text_id = file.long_or("TextID", 0) as u32;
        let mut it = LocalizedListItem {
            item: AnimTextListItem::new(ld.assets, font),
            hidden_text: String::new(),
        };
        it.item.item.set_text_id(ld.assets, text_id);
        if let Some(a) = file.string("Animation") {
            if !a.is_empty() {
                it.item.anim = AnimGroup::from_block(file, &a);
            }
        }
        it.move_to(x, y);
        it
    }
}

impl ListItem for LocalizedListItem {
    fn obj(&self) -> &Object {
        self.item.obj()
    }
    fn obj_mut(&mut self) -> &mut Object {
        self.item.obj_mut()
    }
    fn state(&self) -> ItemState {
        self.item.state()
    }
    fn set_state(&mut self, s: ItemState) {
        self.item.set_state(s);
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.item.update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let s = self.state().anim_state();
        if self.item.anim.state() != s {
            self.item.anim.set_state(s);
        }
        let c = self.item.anim.current_color(self.item.anim.state());
        self.item.item.obj.set_color(c, false);
        self.item.item.render_text(ctx);
    }
    fn text(&self) -> Option<&str> {
        self.item.text()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------------------------
// list box

/// `aListBox`.
pub struct ListBox {
    pub obj: Object,
    pub items: Vec<Box<dyn ListItem>>,
    pub item_selected: i32,
    pub skip_amount: i32,
    pub scroll_bar: Option<ScrollBar>,
    pub single_check: bool,
    pub click_sfx: i32,
    pub highlight_sfx: i32,
    pub disabled_sfx: i32,
    pub top_skip: i32,
}

impl Default for ListBox {
    fn default() -> Self {
        ListBox {
            obj: Object::default(),
            items: Vec::new(),
            item_selected: -1,
            skip_amount: 1,
            scroll_bar: None,
            single_check: true,
            click_sfx: sfx::LOG_SELECT,
            highlight_sfx: sfx::LOG_DIGITALHIGHLIGHT,
            disabled_sfx: sfx::LOG_WRONGBUTTON,
            top_skip: 0,
        }
    }
}

impl ListBox {
    /// `aListBox::init(x, y, w, h)`: a black box with a scroll bar along its right edge.
    pub fn new(ld: &mut Loader<'_>, x: f32, y: f32, w: f32, h: f32) -> ListBox {
        let mut sb = ScrollBar::new(ld, w - 17.0, 2.0, 15.0, h - 4.0);
        sb.set_scroll_max(0.0);
        sb.move_by(x, y);
        ListBox {
            obj: Object::new(x, y, w, h),
            scroll_bar: Some(sb),
            ..ListBox::default()
        }
    }

    /// `aListBox::init(file, block)`: reads the current block (the original does not seek).
    pub fn from_current_block(ld: &mut Loader<'_>, file: &FitIni) -> ListBox {
        let mut l = ListBox::new(
            ld,
            file.long_or("XLocation", 0) as f32,
            file.long_or("YLocation", 0) as f32,
            file.long_or("Width", 0) as f32,
            file.long_or("Height", 0) as f32,
        );
        l.obj.help_header = file.long_or("HelpCaption", 0) as i32;
        l.obj.help_id = file.long_or("HelpDesc", 0) as i32;
        l
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }
    pub fn selected_item(&self) -> i32 {
        self.item_selected
    }
    pub fn item(&self, i: usize) -> Option<&dyn ListItem> {
        self.items.get(i).map(|b| b.as_ref())
    }
    pub fn item_mut(&mut self, i: usize) -> Option<&mut Box<dyn ListItem>> {
        self.items.get_mut(i)
    }
    pub fn is_showing(&self) -> bool {
        self.obj.show
    }
    pub fn show_window(&mut self, s: bool) {
        self.obj.show = s;
    }
    pub fn scroll_pos(&self) -> f32 {
        self.scroll_bar.as_ref().map_or(0.0, |s| s.scroll_pos)
    }
    pub fn scroll_bar_width(&self) -> f32 {
        self.scroll_bar.as_ref().map_or(0.0, |s| s.width())
    }

    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.obj.point_inside(x, y)
    }

    pub fn point_in_scroll_bar(&self, x: i32, y: i32) -> bool {
        self.scroll_bar
            .as_ref()
            .is_some_and(|s| s.point_inside(x, y))
    }

    pub fn set_orange(&mut self) {
        if let Some(s) = &mut self.scroll_bar {
            s.set_orange();
        }
    }

    /// `aListBox::handleMessage` (from the scroll bar).
    pub fn handle_message(&mut self, m: Msg) {
        match m.msg {
            msg::SCROLLUP => self.scroll(-1),
            msg::SCROLLDOWN => self.scroll(1),
            msg::PAGEUP => self.scroll(10),
            msg::PAGEDOWN => self.scroll(-10),
            msg::SCROLLTO => {
                let before = self
                    .scroll_bar
                    .as_ref()
                    .map_or(0.0, |s| s.pos_before_scroll_to);
                self.scroll(-(before as i32));
                self.scroll(m.who);
            }
            _ => {}
        }
    }

    /// `aListBox::scroll`: moves every item up by `amount`, unless already at that end.
    pub fn scroll(&mut self, amount: i32) {
        let Some(first) = self.items.first() else {
            return;
        };
        if amount < 0 && first.obj().global_y() > self.obj.global_y() {
            return;
        }
        let last = self.items.last().expect("non-empty");
        if amount > 0
            && (last.obj().global_y() as f32 + last.obj().height())
                < self.obj.global_y() as f32 + self.obj.height()
        {
            return;
        }
        for it in &mut self.items {
            it.move_by(0.0, -amount as f32);
        }
    }

    fn scroll_bar_set_scroll(&mut self, pos: i32) {
        if let Some(sb) = &mut self.scroll_bar {
            let m = sb.set_scroll(pos);
            self.handle_message(m);
        }
    }

    /// `setScrollPos` (scrolls, then `SetScroll`, which scrolls again through the message, as the
    /// original does).
    pub fn set_scroll_pos(&mut self, pos: i32) {
        let cur = self.scroll_pos() as i32;
        self.scroll(pos - cur);
        self.scroll_bar_set_scroll(pos);
    }

    fn update_scroll_max(&mut self, with_skip: bool) {
        let Some(h) = self.scroll_bar.as_ref().map(|s| s.height()) else {
            return;
        };
        let (Some(first), Some(last)) = (self.items.first(), self.items.last()) else {
            return;
        };
        let mut total = (last.obj().bottom() - first.obj().top()) as i32;
        if with_skip {
            total += self.skip_amount;
        }
        if total as f32 > h {
            if let Some(sb) = &mut self.scroll_bar {
                sb.set_scroll_max(total as f32 - h);
            }
        } else {
            if with_skip {
                self.scroll_bar_set_scroll(0);
            }
            if let Some(sb) = &mut self.scroll_bar {
                sb.set_scroll_max(0.0);
            }
        }
    }

    /// `AddItem`: stacks the item under the last one. Returns its index, or `None` when the box
    /// is full (`TOO_MANY_ITEMS`).
    pub fn add_item(&mut self, mut item: Box<dyn ListItem>) -> Option<usize> {
        if self.items.len() >= MAX_LIST_ITEMS {
            return None;
        }
        let last_x = (self.obj.global_x() + item.obj().global_x()) as f32;
        let mut last_y = (self.obj.global_y() + self.top_skip) as f32;
        if let Some(last) = self.items.last() {
            last_y = last.obj().global_y() as f32 + last.obj().height() + self.skip_amount as f32;
        }
        item.move_to(last_x, last_y);
        if item.obj().global_right() > self.obj.global_right() {
            let shrink = (self.obj.global_right() - item.obj().global_right()) as f32;
            let (w, h) = (item.obj().width(), item.obj().height());
            item.resize(w + shrink, h);
        }
        self.items.push(item);
        self.update_scroll_max(true);
        Some(self.items.len() - 1)
    }

    /// `InsertItem`.
    pub fn insert_item(&mut self, mut item: Box<dyn ListItem>, at: usize) -> Option<usize> {
        if self.items.len() >= MAX_LIST_ITEMS {
            return None;
        }
        let at = at.min(self.items.len());
        let step = item.obj().height() + self.skip_amount as f32;
        for it in &mut self.items[at..] {
            it.move_by(0.0, step);
        }
        let last_x = self.obj.loc[0].x + item.obj().loc[0].x;
        let mut last_y = self.obj.loc[0].y;
        if !self.items.is_empty() {
            if at > 0 {
                last_y = self.items[at - 1].obj().bottom() + self.skip_amount as f32;
            } else {
                // the item that is now at `at` moved down already
                last_y = self.items[at].obj().top() - self.skip_amount as f32 - item.obj().height();
            }
        }
        item.move_to(last_x, last_y);
        item.obj_mut().show = true;
        if item.obj().global_right() > self.obj.global_right() {
            let shrink = (self.obj.global_right() - item.obj().global_right()) as f32;
            let (w, h) = (item.obj().width(), item.obj().height());
            item.resize(w + shrink, h);
        }
        self.items.insert(at, item);
        self.update_scroll_max(false);
        Some(at)
    }

    /// `RemoveItem`: the items below move up. Returns the removed item.
    pub fn remove_item(&mut self, index: usize) -> Option<Box<dyn ListItem>> {
        if index >= self.items.len() {
            return None;
        }
        let height = self.items[index].obj().height();
        let removed = self.items.remove(index);
        for it in &mut self.items[index..] {
            it.move_by(0.0, -height - self.skip_amount as f32);
        }
        if index == self.items.len() && self.item_selected == index as i32 {
            self.item_selected = -1;
        }
        if self.item_selected != -1 {
            self.select_item(self.item_selected);
        }
        Some(removed)
    }

    /// `removeAllItems`.
    pub fn remove_all_items(&mut self) {
        self.items.clear();
        self.item_selected = -1;
        self.scroll_bar_set_scroll(0);
    }

    /// `SelectItem` (`-1` clears). Scrolls the bar so the item is in view.
    pub fn select_item(&mut self, n: i32) -> bool {
        if n >= self.items.len() as i32 {
            return false;
        }
        for it in &mut self.items {
            it.deselect();
        }
        self.item_selected = n;
        if n > -1 {
            let it = &mut self.items[n as usize];
            it.set_state(ItemState::Selected);
            let (gy, h) = (it.obj().global_y() as f32, it.obj().height());
            let show = it.obj().show;
            let (by, bh) = (self.obj.global_y() as f32, self.obj.height());
            if !show || gy + h > by + bh || gy + h < by {
                if let Some(sb) = &self.scroll_bar {
                    let p = (gy + h - by) - sb.height() + sb.scroll_pos;
                    self.scroll_bar_set_scroll(p as i32);
                }
            }
        }
        true
    }

    /// `GetCheckedItem`.
    pub fn checked_item(&self) -> i32 {
        self.items
            .iter()
            .position(|i| i.is_checked())
            .map_or(-1, |p| p as i32)
    }

    pub fn enable_all_items(&mut self) {
        for it in &mut self.items {
            if it.state() == ItemState::Disabled {
                it.set_state(ItemState::Enabled);
            }
        }
    }

    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        if let Some(s) = &mut self.scroll_bar {
            s.move_by(dx, dy);
        }
        for it in &mut self.items {
            it.move_by(dx, dy);
        }
    }

    pub fn move_to(&mut self, x: f32, y: f32) {
        let (dx, dy) = (x - self.obj.loc[0].x, y - self.obj.loc[0].y);
        self.move_by(dx, dy);
    }

    /// `aListBox::resize`.
    pub fn resize(&mut self, w: f32, h: f32) {
        self.obj.resize(w, h);
        if let Some(s) = &mut self.scroll_bar {
            let sw = s.width();
            s.resize(sw, h - 4.0);
        }
    }

    /// `aListBox::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        if !self.obj.show {
            return;
        }
        if let Some(sb) = &mut self.scroll_bar {
            let msgs = sb.update(ctx);
            for m in msgs {
                self.handle_message(m);
            }
        }
        let prev_check = self.checked_item();
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let inside = self.point_inside(mx, my);
        if inside && self.obj.help_id != 0 {
            ctx.out.help_text_id = self.obj.help_id;
        }
        let (mut sel, mut hi) = (-1i32, -1i32);
        let (top, bottom) = (self.obj.global_y(), self.obj.global_bottom());
        for (i, it) in self.items.iter_mut().enumerate() {
            if inside
                && it.point_inside(mx, my)
                && it.obj().show
                && it.state() != ItemState::Disabled
            {
                if ctx.input.is_left_click() {
                    sel = i as i32;
                    if it.state() != ItemState::Selected {
                        ctx.play(self.click_sfx);
                    }
                } else {
                    hi = i as i32;
                }
            }
            let vis = it.obj().global_bottom() > top && it.obj().global_y() < bottom;
            it.obj_mut().show = vis;
        }
        for (i, it) in self.items.iter_mut().enumerate() {
            let i = i as i32;
            if sel == i {
                it.set_state(ItemState::Selected);
                self.item_selected = i;
            } else if hi == i && it.state() != ItemState::Selected {
                if it.state() != ItemState::Highlite {
                    ctx.play(self.highlight_sfx);
                }
                it.set_state(ItemState::Highlite);
            } else if (sel != -1 || it.state() != ItemState::Selected)
                && it.state() != ItemState::Disabled
            {
                it.set_state(ItemState::Enabled);
            }
            it.update(ctx);
        }
        let new_check = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, it)| it.is_checked() && *i as i32 != prev_check)
            .map(|(i, _)| i as i32)
            .next_back()
            .unwrap_or(-1);
        if self.single_check && new_check != -1 && prev_check != -1 {
            self.items[prev_check as usize].set_check(false);
        }
    }

    /// `aListBox::render`: background, the visible items, black covers over items that hang over
    /// the top or bottom edge, and the scroll bar (enabled only when something is out of view).
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.obj.draw_quad(ctx);
        let (gt, gb) = (self.obj.global_y(), self.obj.global_bottom());
        let (mut b_top, mut b_bottom, mut out_of_range) = (false, false, false);
        let (mut top_h, mut bottom_h) = (0, 0);
        for it in &mut self.items {
            let (it_t, it_b) = (it.obj().global_y(), it.obj().global_bottom());
            if it_b > gt && it_t < gb {
                it.render(ctx);
                if it_t < gt {
                    top_h = it.obj().height() as i32;
                    b_top = true;
                    out_of_range = true;
                }
                if it_b >= gb {
                    b_bottom = true;
                    if bottom_h == 0 {
                        bottom_h = it.obj().height() as i32;
                    }
                    out_of_range = true;
                }
            } else {
                out_of_range = true;
            }
        }
        if b_top || b_bottom {
            let (gx, gw) = (self.obj.global_x(), self.obj.width() as i32);
            let gy = self.obj.global_y();
            let h = self.obj.height() as i32;
            if b_top {
                ctx.dl.fill_rect(
                    crate::draw::GuiRect::new(gx, gy - top_h, gx + gw, gy),
                    0xff00_0000,
                );
            }
            if b_bottom {
                ctx.dl.fill_rect(
                    crate::draw::GuiRect::new(gx, gy + h + 1, gx + gw, gy + h + bottom_h + 2),
                    0xff00_0000,
                );
            }
        }
        if let Some(sb) = &mut self.scroll_bar {
            sb.enable(out_of_range);
            sb.render(ctx);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// drop list and combo box

/// The template item drop lists and combo boxes clone for their entries (font 27333).
fn template_item(ld: &mut Loader<'_>, file: &mut FitIni) -> AnimTextListItem {
    let font = ld.font(27333);
    let mut t = AnimTextListItem::new(ld.assets, font);
    t.init_fit(file, "Text0");
    t
}

fn read_rects(file: &mut FitIni) -> Vec<Rect> {
    let _ = file.seek_block("Rects");
    let n = file.long_or("Rectcount", 0).max(0) as usize;
    (0..n)
        .map(|i| Rect::from_fit(file, &format!("Rect{i}")))
        .collect()
}

/// `aDropList`: a closed box showing the selection; the expand button opens a list below.
pub struct DropList {
    pub obj: Object,
    pub rects: Vec<Rect>,
    pub expand: AnimButton,
    pub list: ListBox,
    pub list_max_height: f32,
    pub selection: i32,
    pub template: AnimTextListItem,
    pub text_left: i32,
    pub text_top: i32,
}

impl DropList {
    /// `aDropList::init`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> DropList {
        let _ = file.seek_block(block);
        let x = file.long_or("XLocation", 0) as f32;
        let y = file.long_or("YLocation", 0) as f32;
        let mut w = file.long_or("Width", 0) as f32;
        let mut h = file.long_or("Height", 0) as f32;
        let mut expand = AnimButton::from_fit(ld, file, "ExpandButton");
        let mut rects = read_rects(file);
        while rects.len() < 3 {
            rects.push(Rect::default());
        }
        let max_h = rects[2].obj.height();
        let r2 = &rects[2].obj;
        let mut list = ListBox::new(
            ld,
            r2.global_x() as f32,
            r2.global_y() as f32,
            r2.width(),
            max_h,
        );
        rects[2].obj.show = false;
        list.show_window(false);
        w = w.max(rects[1].obj.width());
        h = h.max(rects[1].obj.height());
        let (mut tl, mut tt) = (0, 0);
        if file.seek_block("Text0").is_ok() {
            tl = file.long_or("XLocation", 0) as i32;
            tt = file.long_or("YLocation", 0) as i32;
        }
        expand.set_press_fx(sfx::LOG_VIDEOBUTTONS);
        expand.set_highlight_fx(sfx::LOG_DIGITALHIGHLIGHT);
        expand.set_disabled_fx(sfx::LOG_WRONGBUTTON);
        expand.set_message_on_release();
        let template = template_item(ld, file);
        let mut d = DropList {
            obj: Object::new(0.0, 0.0, w, h),
            rects,
            expand,
            list,
            list_max_height: max_h,
            selection: -1,
            template,
            text_left: tl,
            text_top: tt,
        };
        d.move_by(x, y);
        d
    }

    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        for r in &mut self.rects {
            r.obj.move_by(dx, dy);
        }
        self.list.move_by(dx, dy);
        self.expand.move_by(dx, dy);
    }

    pub fn is_expanded(&self) -> bool {
        self.list.is_showing()
    }

    /// `IsExpanded(bool)`.
    pub fn set_expanded(&mut self, e: bool) {
        self.list.show_window(e);
        if e {
            self.list.select_item(-1);
        }
        if let Some(r) = self.rects.get_mut(2) {
            r.obj.show = e;
        }
    }

    /// `disable`.
    pub fn disable(&mut self, d: bool) {
        if d {
            self.set_expanded(false);
        }
        self.expand.obj.show = !d;
        self.rects[0].obj.show = !d;
        self.rects[1].obj.show = !d;
    }

    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.obj.point_inside(x, y) || (self.list.is_showing() && self.list.point_inside(x, y))
    }

    /// `AddItem(item)`: grows the open list up to its maximum height.
    pub fn add_item(&mut self, item: Box<dyn ListItem>) -> Option<usize> {
        let ih = item.obj().height();
        let r = self.list.add_item(item);
        let mut nh = (ih + self.list.skip_amount as f32) * self.list.item_count() as f32 + 4.0;
        if nh > self.list_max_height {
            nh = self.list_max_height;
        }
        let lw = self.list.obj.width();
        self.list.resize(lw, nh);
        let rw = self.rects[2].obj.width();
        self.rects[2].obj.resize(rw, nh);
        r
    }

    /// `AddItem(text, color)`: a copy of the template item.
    pub fn add_text(&mut self, text: &str, color: u32) -> Option<usize> {
        let mut it = self.template.clone();
        it.item.set_text(text);
        it.item.set_color(color);
        self.add_item(Box::new(it))
    }

    /// `SelectItem`.
    pub fn select_item(&mut self, n: i32) -> bool {
        self.list.show_window(false);
        self.rects[2].obj.show = false;
        self.selection = n;
        self.list.select_item(n)
    }

    pub fn selected_item(&self) -> i32 {
        self.selection
    }

    /// `aDropList::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        if self.obj.show {
            // aObject::update over the children: rect 1, list, rect 2, expand button, rect 0
            self.obj.update(ctx);
            self.rects[1].update(ctx);
            self.list.update(ctx);
            self.rects[2].update(ctx);
            // the button's message goes to aObject::handleMessage (the drop list's handler has
            // another signature), so only the click test below opens the list
            let _ = self.expand.update(ctx);
            self.rects[0].update(ctx);
            let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            if ctx.input.is_left_click() {
                if self.expand.point_inside(mx, my) {
                    let e = self.list.is_showing();
                    self.set_expanded(!e);
                }
                if self.list.is_showing()
                    && (!self.point_inside(mx, my)
                        || (self.list.point_inside(mx, my)
                            && !self.list.point_in_scroll_bar(mx, my)))
                {
                    self.list.show_window(false);
                    self.rects[2].obj.show = false;
                }
            }
            if self.list.selected_item() >= 0 {
                self.selection = self.list.selected_item();
                if self.list.is_showing() {
                    self.set_expanded(false);
                }
                self.list.select_item(-1);
            } else if self.list.is_showing() && ctx.input.key_down(keys::KEY_RETURN) {
                self.list.show_window(false);
                self.rects[2].obj.show = false;
            }
        }
        self.list.obj.help_id = self.obj.help_id;
    }

    /// `aDropList::render`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.rects[1].render(ctx);
        self.list.render(ctx);
        self.rects[2].render(ctx);
        self.expand.render(ctx);
        self.rects[0].render(ctx);
        if self.selection >= 0 {
            let avail = (self.expand.obj.global_x() - 1) as f32 - (self.obj.global_x() + 1) as f32;
            let target = (
                (self.obj.global_x() + self.text_left) as f32,
                (self.obj.global_y() + self.text_top) as f32,
            );
            if let Some(it) = self.list.item_mut(self.selection as usize) {
                let (l, t) = (it.obj().loc[0].x, it.obj().loc[0].y);
                let (w, h) = (it.obj().width(), it.obj().height());
                let shrink = avail > 0.0 && w > avail;
                if shrink {
                    it.resize(avail, h);
                }
                let showing = it.obj().show;
                it.move_to(target.0, target.1);
                it.obj_mut().show = true;
                it.render(ctx);
                it.move_to(l, t);
                it.obj_mut().show = showing;
                if shrink {
                    it.resize(w, h);
                }
            }
        }
    }
}

/// `aComboBox`: an edit field with a drop-down list of choices.
pub struct ComboBox {
    pub obj: Object,
    pub entry: Edit,
    pub rects: Vec<Rect>,
    pub expand: AnimButton,
    pub list: ListBox,
    pub list_max_height: f32,
    pub selection: i32,
    pub template: AnimTextListItem,
}

impl ComboBox {
    /// `aComboBox::init`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> ComboBox {
        let _ = file.seek_block(block);
        let x = file.long_or("XLocation", 0) as f32;
        let y = file.long_or("YLocation", 0) as f32;
        let mut w = file.long_or("Width", 0) as f32;
        let mut h = file.long_or("Height", 0) as f32;
        let mut rects = read_rects(file);
        while rects.len() < 3 {
            rects.push(Rect::default());
        }
        let entry = Edit::from_fit(ld, file, "EditBox");
        let mut expand = AnimButton::from_fit(ld, file, "ExpandButton");
        expand.set_press_fx(sfx::LOG_VIDEOBUTTONS);
        expand.set_highlight_fx(sfx::LOG_DIGITALHIGHLIGHT);
        expand.set_disabled_fx(sfx::LOG_WRONGBUTTON);
        let max_h = rects[2].obj.height();
        let r2 = &rects[2].obj;
        let mut list = ListBox::new(
            ld,
            r2.global_x() as f32,
            r2.global_y() as f32,
            r2.width(),
            max_h,
        );
        rects[2].obj.show = false;
        list.show_window(false);
        w = w.max(entry.obj.width());
        h = h.max(entry.obj.height());
        let template = template_item(ld, file);
        expand.set_message_on_release();
        let mut c = ComboBox {
            obj: Object::new(0.0, 0.0, w, h),
            entry,
            rects,
            expand,
            list,
            list_max_height: max_h,
            selection: -1,
            template,
        };
        c.move_by(x, y);
        c
    }

    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.list.move_by(dx, dy);
        self.entry.move_by(dx, dy);
        self.expand.move_by(dx, dy);
        for r in &mut self.rects {
            r.obj.move_by(dx, dy);
        }
    }

    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.obj.point_inside(x, y) || (self.list.is_showing() && self.list.point_inside(x, y))
    }

    pub fn add_item(&mut self, item: Box<dyn ListItem>) -> Option<usize> {
        let mut nh = (item.obj().height() + self.list.skip_amount as f32)
            * (self.list.item_count() as f32 + 1.0)
            + 4.0;
        if nh > self.list_max_height {
            nh = self.list_max_height;
        }
        let lw = self.list.obj.width();
        self.list.resize(lw, nh);
        let rw = self.rects[2].obj.width();
        self.rects[2].obj.resize(rw, nh);
        self.list.add_item(item)
    }

    pub fn add_text(&mut self, text: &str, color: u32) -> Option<usize> {
        let mut it = self.template.clone();
        it.item.set_text(text);
        it.item.set_color(color);
        self.add_item(Box::new(it))
    }

    pub fn select_item(&mut self, n: i32) -> bool {
        self.list.show_window(false);
        self.rects[2].obj.show = false;
        self.selection = n;
        self.list.select_item(n)
    }

    /// `aComboBox::update`. Returns `aMSG_SELCHANGED` with the box id when a choice is taken.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Msg> {
        let mut out = None;
        if self.obj.show {
            self.obj.update(ctx);
            self.list.update(ctx);
            self.entry.update(ctx);
            let _ = self.expand.update(ctx);
            for r in &self.rects {
                r.update(ctx);
            }
            let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            if ctx.input.is_left_click() {
                if self.expand.point_inside(mx, my) {
                    let s = !self.list.is_showing();
                    self.list.show_window(s);
                    self.rects[2].obj.show = s;
                } else if self.list.is_showing()
                    && (!self.point_inside(mx, my)
                        || (self.list.point_inside(mx, my)
                            && !self.list.point_in_scroll_bar(mx, my)))
                {
                    self.list.show_window(false);
                    self.rects[2].obj.show = false;
                }
            }
            if self.list.selected_item() >= 0 {
                self.selection = self.list.selected_item();
                let text = self
                    .list
                    .item(self.selection as usize)
                    .and_then(|i| i.text().map(str::to_string));
                if let Some(t) = text {
                    self.entry.set_entry(ctx.assets, &t, false);
                }
                if self.list.is_showing() {
                    self.list.show_window(false);
                    self.rects[2].obj.show = false;
                }
                out = Some(Msg::new(msg::SELCHANGED, self.obj.id));
                self.list.select_item(-1);
            } else if self.list.is_showing() && ctx.input.key_down(keys::KEY_RETURN) {
                self.list.show_window(false);
                self.rects[2].obj.show = false;
            }
        }
        self.list.obj.help_id = self.obj.help_id;
        out
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.list.render(ctx);
        self.entry.render(ctx);
        self.expand.render(ctx);
        for r in &self.rects {
            r.render(ctx);
        }
    }
}
