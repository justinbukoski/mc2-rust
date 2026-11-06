// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `code/chatwindow.cpp`: the message list, its line cap, the text entry and the toggles.
//!
//! Messages arrive as plain [`ChatMessage`]s, one batch per frame, and are appended in the order
//! `ChatWindow::refillListBox` appends them (chatwindow.cpp:241-299). Networking is not part of
//! this slice: the send path (`MultiPlayer::sendChat`, multplyr.cpp:775) and the message source
//! (`MultiPlayer::getChatMessages`, multplyr.cpp:1194) are empty in this tree, so a submitted
//! message is returned as a [`ChatIntent`] and nothing here sends it.
//!
//! The C++ has no fade and no timed expiry for chat lines: neither chatwindow.cpp nor the list box
//! (gui/alistbox.cpp) keeps a timer or an alpha. A line leaves the list only when newer lines push
//! it past the line cap, and that is what [`ChatList::refill`] does.

use super::{HudOp, Metrics};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::font::FontRef;
use mc2_formats::ini::FitIni;
use std::ops::Range;

/// `playerText.resize(437 - playerRect.width() - 2, ...)` (chatwindow.cpp:365): the width of the
/// message area of an item, the name box included.
const LIST_WIDTH: i32 = 437;
/// `edits[0].limitEntry(128)` (chatwindow.cpp:86).
const ENTRY_LIMIT: usize = 128;
/// `refillListBox(..., 128)` (chatwindow.cpp:207): the expanded list's line cap.
const EXPANDED_CAP: i32 = 128;
/// `chatwindow.cpp:265`: the text colour of team messages, and the default.
const TEAM_TEXT: u32 = 0xff41_c700;
const TEXT_WHITE: u32 = 0xffff_ffff;
/// `chatwindow.cpp:394-397`: names on dark player colours are white, otherwise black.
const NAME_WHITE: u32 = 0xffff_ffff;
const NAME_BLACK: u32 = 0xff00_0000;
/// `chatwindow.cpp:155`: the black behind the entry.
const BLACK: u32 = 0xff00_0000;
/// `Rect2` of the .fit: the line between the list and the entry.
const SEPARATOR_COLOUR: u32 = 0xff00_2f55;
/// `IDS` of the help text: `Text0 TextID = 27700` in `mcl_mp_chatwidget.fit`.
const HELP_TEXT_ID: u32 = 27700;

/// The part of the chat layout the window reads (`mcl_mp_chatwidget.fit` and
/// `mcl_mp_chatwidget_up.fit`). Every rect is in UI coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChatGeometry {
    /// `[Rect1]` of the collapsed widget: the list (`listBox`, 39 px high).
    pub list: GuiRect,
    /// `[Rect2]`: the line between the list and the entry.
    pub separator: GuiRect,
    /// `[Edit0]`: the entry.
    pub entry: GuiRect,
    /// `[Rect2]` of the expanded widget: its list (131 px high).
    pub expanded_list: GuiRect,
    /// `[Text0]`: where the help text is drawn.
    pub help: GuiRect,
}

impl ChatGeometry {
    /// Reads both chat layouts. `main` is `mcl_mp_chatwidget.fit`, `up` is `..._up.fit`.
    pub fn from_fits(main: &mut FitIni, up: &mut FitIni) -> Option<ChatGeometry> {
        Some(ChatGeometry {
            list: block_rect(main, "Rect1")?,
            separator: block_rect(main, "Rect2")?,
            entry: block_rect(main, "Edit0")?,
            expanded_list: block_rect(up, "Rect2")?,
            help: block_rect(main, "Text0")?,
        })
    }
}

/// `XLocation`, `YLocation`, `Width`, `Height` of a block, as a `GuiRect`.
fn block_rect(file: &mut FitIni, block: &str) -> Option<GuiRect> {
    file.seek_block(block).ok()?;
    let x = file.long("XLocation")? as i32;
    let y = file.long("YLocation")? as i32;
    let w = file.long("Width")? as i32;
    let h = file.long("Height")? as i32;
    Some(GuiRect::new(x, y, x + w, y + h))
}

/// The fonts the chat draws with: `IDS_CHAT_FONT` (25999) for names, messages and the entry, and
/// the help text's font (`Text0 Font = 25998`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChatFonts {
    pub chat: FontRef,
    pub help: FontRef,
}

/// One chat message as the app hands it over. The flags of `getChatMessages` map to the bools:
/// `hide_name` is `0x01000000` and `team_text` is `0x10000000` (chatwindow.cpp:258, 264).
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    /// The sender's name (`player->name`).
    pub sender: String,
    /// The sender's colour (`MPlayer->colors[baseColor[0]]`), `0xAARRGGBB`.
    pub sender_colour: u32,
    pub text: String,
    pub hide_name: bool,
    pub team_text: bool,
}

/// One row of a list: a sender box and a message, as `ChatMessageItem` lays them out.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatItem {
    /// The name drawn in the box (empty when the name is hidden).
    pub name: String,
    pub name_colour: u32,
    /// The box colour, the sender's colour.
    pub box_colour: u32,
    /// `name.font.width(name)`.
    pub name_width: i32,
    pub text: String,
    pub text_colour: u32,
    /// The item height: the wrapped text height, at most two lines (chatwindow.cpp:376-385).
    pub height: i32,
    /// `setText`'s return value: 1 or 2 (chatwindow.cpp:381-382).
    pub line_count: i32,
    /// The line height of the chat font.
    pub line_height: i32,
}

impl ChatItem {
    /// `ChatMessageItem::setPlayerName`, `setPlayerColor`, `setText` and `setTextColor` for one
    /// message (chatwindow.cpp:257-265), in that order.
    fn new(m: &impl Metrics, font: FontRef, msg: &ChatMessage) -> ChatItem {
        let name = if msg.hide_name {
            String::new()
        } else {
            msg.sender.clone()
        };
        let name_width = m.text_width(font, &name);
        let line_height = m.line_height(font);
        let text_width = LIST_WIDTH - (name_width + 2) - 2;
        let mut height = m.wrapped_height(font, &msg.text, text_width);
        let mut line_count = 1;
        if height > line_height * 2 {
            height = line_height * 2;
            line_count = 2;
        }
        ChatItem {
            name,
            name_colour: name_colour(msg.sender_colour),
            box_colour: msg.sender_colour,
            name_width,
            text: msg.text.clone(),
            text_colour: if msg.team_text { TEAM_TEXT } else { TEXT_WHITE },
            height,
            line_count,
            line_height,
        }
    }

    /// `playerRect.resize(tmpWidth + 2, ...)`: the box is the name plus two pixels.
    pub fn rect_width(&self) -> i32 {
        self.name_width + 2
    }

    /// `playerText.moveTo(globalX() + tmpWidth + 3, ...)`.
    pub fn text_x(&self) -> i32 {
        self.name_width + 3
    }

    /// `playerText.resize(437 - playerRect.width() - 2, ...)`.
    pub fn text_width(&self) -> i32 {
        LIST_WIDTH - self.rect_width() - 2
    }
}

/// `ChatMessageItem::setPlayerColor` (chatwindow.cpp:394-397): the average of the three colour
/// bytes below 85 gives white text, otherwise black.
fn name_colour(colour: u32) -> u32 {
    let avg = ((colour & 0xff) + ((colour >> 8) & 0xff) + ((colour >> 16) & 0xff)) / 3;
    if avg < 85 {
        NAME_WHITE
    } else {
        NAME_BLACK
    }
}

/// A `aListBox` of chat items: a line cap, the items, and the pixel scroll.
#[derive(Clone, Debug)]
pub struct ChatList {
    /// `maxCount`: the cap in lines.
    cap: i32,
    /// The list box height in pixels.
    box_height: i32,
    items: Vec<ChatItem>,
    /// The scroll offset in pixels (`scrollBar->GetScrollPos()`).
    scroll: i32,
}

impl ChatList {
    pub fn new(cap: i32, box_height: i32) -> ChatList {
        ChatList {
            cap,
            box_height,
            items: Vec::new(),
            scroll: 0,
        }
    }

    pub fn capacity(&self) -> i32 {
        self.cap
    }

    pub fn items(&self) -> &[ChatItem] {
        &self.items
    }

    /// The number of lines the items take (`getLineCount` summed, chatwindow.cpp:270-274).
    pub fn lines(&self) -> i32 {
        self.items.iter().map(|i| i.line_count).sum()
    }

    /// The height of all items, with no space between them (`setSpaceBetweenItems(0)`,
    /// chatwindow.cpp:69, 330).
    pub fn total_height(&self) -> i32 {
        self.items.iter().map(|i| i.height).sum()
    }

    pub fn scroll(&self) -> i32 {
        self.scroll
    }

    /// The top of item `i` in list coordinates, before the scroll.
    pub fn item_top(&self, i: usize) -> i32 {
        self.items[..i].iter().map(|it| it.height).sum()
    }

    /// The scroll bar's height: `aListBox::init` gives it `h - 4` (gui/alistbox.cpp:59-60).
    fn scrollbar_height(&self) -> i32 {
        self.box_height - 4
    }

    /// `SetScrollMax(itemsTotalHeight - scrollBar->height())` (gui/alistbox.cpp:377-382), or 0.
    pub fn scroll_max(&self) -> i32 {
        let total = self.total_height();
        if total > self.scrollbar_height() {
            total - self.scrollbar_height()
        } else {
            0
        }
    }

    /// Moves the scroll by `dy` pixels, clamped to the range.
    pub fn scroll_by(&mut self, dy: i32) {
        self.scroll_to(self.scroll + dy);
    }

    pub fn scroll_to(&mut self, y: i32) {
        self.scroll = y.clamp(0, self.scroll_max());
    }

    /// The items that show at least one pixel in the box, newest last.
    pub fn visible(&self) -> Range<usize> {
        let mut top = 0;
        let mut first = None;
        let mut end = 0;
        for (i, item) in self.items.iter().enumerate() {
            let y0 = top - self.scroll;
            let y1 = y0 + item.height;
            if y1 > 0 && y0 < self.box_height {
                first.get_or_insert(i);
                end = i + 1;
            }
            top += item.height;
        }
        match first {
            Some(f) => f..end,
            None => 0..0,
        }
    }

    /// `ChatWindow::refillListBox` (chatwindow.cpp:241-299) for one frame's messages.
    pub fn refill(&mut self, m: &impl Metrics, font: FontRef, messages: &[ChatMessage]) {
        // chatwindow.cpp:244: only the first `maxCount` messages of the frame are used.
        let new: Vec<ChatItem> = messages
            .iter()
            .take(self.cap.max(0) as usize)
            .map(|msg| ChatItem::new(m, font, msg))
            .collect();
        let to_add: i32 = new.iter().map(|i| i.line_count).sum();

        // chatwindow.cpp:275-289: the oldest items go until the new lines fit under the cap.
        let mut lines = self.lines();
        while lines + to_add > self.cap && !self.items.is_empty() {
            lines -= self.items.remove(0).line_count;
        }
        self.scroll = self.scroll.clamp(0, self.scroll_max());

        let added = !new.is_empty();
        self.items.extend(new);
        self.scroll = self.scroll.clamp(0, self.scroll_max());

        // chatwindow.cpp:295-296: SelectItem(last) when the cap is above 2, which brings the
        // newest line into view: the list scrolls to its end (gui/alistbox.cpp:515-526). The C++
        // calls it only inside the loop that adds a message, so an empty batch keeps the scroll.
        if self.cap > 2 && added && self.total_height() - self.scroll > self.box_height {
            self.scroll = self.scroll_max();
        }
    }
}

/// The entry box: its text and focus (`aEdit` in chatwindow.cpp:86, 214-236).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChatEntry {
    pub text: String,
    pub focused: bool,
}

impl ChatEntry {
    /// Typed characters go in only while the entry has focus, up to 128 characters.
    pub fn type_text(&mut self, s: &str) {
        if !self.focused {
            return;
        }
        for c in s.chars() {
            if self.text.chars().count() >= ENTRY_LIMIT {
                break;
            }
            self.text.push(c);
        }
    }

    pub fn backspace(&mut self) {
        self.text.pop();
    }
}

/// The buttons of the chat window (`MP_CHAT_HELPTOGGLE` 300, `MP_CHAT_EXPAND` 301,
/// `MP_CHAT_TEAMONLY` 302).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatButton {
    HelpToggle,
    Expand,
    TeamOnly,
}

/// What the window shows: the expanded list, the collapsed list, or the help text
/// (chatwindow.cpp:130-148).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatView {
    Collapsed,
    Expanded,
    Help,
}

/// This frame's input to the window.
#[derive(Clone, Copy, Debug)]
pub struct ChatFrame<'a> {
    /// The messages that arrived since the last frame.
    pub messages: &'a [ChatMessage],
    /// `KEY_RETURN` was pressed this frame.
    pub enter_pressed: bool,
    /// `MPlayer->getPlayerInfo(commanderID)->team`: the team a team-only message goes to.
    pub commander_team: i32,
}

/// What the window asks the app to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatIntent {
    /// `MPlayer->sendChat(0, team, text)` (chatwindow.cpp:224). `team` is -1 for everyone.
    Submit { team: i32, text: String },
}

/// The chat window: the collapsed and expanded lists, the entry, and the toggles.
#[derive(Clone, Debug)]
pub struct ChatWindow {
    geom: ChatGeometry,
    fonts: ChatFonts,
    /// `listBox`: the collapsed list, capped at `listBox.height() / itemHeight` lines.
    list: ChatList,
    /// `chatWidget.listBox`: the expanded list.
    widget: ChatList,
    pub entry: ChatEntry,
    help_toggle: bool,
    expand: bool,
    team_only: bool,
}

impl ChatWindow {
    /// `ChatWindow::initInstance` (chatwindow.cpp:52-89): the collapsed cap is
    /// `listBox.height() / itemHeight`, integer division (chatwindow.cpp:84).
    pub fn new(geom: ChatGeometry, fonts: ChatFonts, m: &impl Metrics) -> ChatWindow {
        let item_height = m.line_height(fonts.chat);
        let cap = if item_height > 0 {
            geom.list.height() / item_height
        } else {
            0
        };
        ChatWindow {
            geom,
            fonts,
            list: ChatList::new(cap, geom.list.height()),
            widget: ChatList::new(EXPANDED_CAP, geom.expanded_list.height()),
            entry: ChatEntry::default(),
            help_toggle: false,
            expand: false,
            team_only: false,
        }
    }

    pub fn list(&self) -> &ChatList {
        &self.list
    }

    pub fn expanded_list(&self) -> &ChatList {
        &self.widget
    }

    /// The view for the current toggles (chatwindow.cpp:130-148).
    pub fn view(&self) -> ChatView {
        if self.expand {
            ChatView::Expanded
        } else if self.help_toggle {
            ChatView::Help
        } else {
            ChatView::Collapsed
        }
    }

    /// A user click on a toggle: it flips, then `handleMessage` applies the exclusions.
    pub fn click(&mut self, button: ChatButton) {
        let state = match button {
            ChatButton::HelpToggle => &mut self.help_toggle,
            ChatButton::Expand => &mut self.expand,
            ChatButton::TeamOnly => &mut self.team_only,
        };
        *state = !*state;
        self.handle_message(button);
    }

    /// `ChatWindow::handleMessage` (chatwindow.cpp:105-127). `press(0)` releases a toggle.
    fn handle_message(&mut self, button: ChatButton) {
        match button {
            ChatButton::HelpToggle => {
                if self.help_toggle && self.expand {
                    self.expand = false;
                }
            }
            ChatButton::Expand => {
                if self.expand {
                    self.help_toggle = false;
                }
            }
            ChatButton::TeamOnly => self.help_toggle = false,
        }
    }

    /// `ChatWindow::update` (chatwindow.cpp:175-239): both lists take this frame's messages; Enter
    /// focuses the entry, or, with focus, sends the text (to the commander's team when the
    /// team-only toggle is on) and clears it.
    pub fn update(&mut self, m: &impl Metrics, frame: &ChatFrame<'_>) -> Option<ChatIntent> {
        self.list.refill(m, self.fonts.chat, frame.messages);
        self.widget.refill(m, self.fonts.chat, frame.messages);

        if !frame.enter_pressed {
            return None;
        }
        if self.entry.focused {
            let team = if self.team_only {
                frame.commander_team
            } else {
                -1
            };
            let text = std::mem::take(&mut self.entry.text);
            self.entry.focused = false;
            if text.is_empty() {
                return None;
            }
            Some(ChatIntent::Submit { team, text })
        } else {
            // chatwindow.cpp:231-234: a press of Enter with help open releases help.
            if self.help_toggle {
                self.help_toggle = false;
            }
            self.entry.focused = true;
            None
        }
    }

    /// The scroll of the list the current view shows, in pixels.
    pub fn scroll_list(&mut self, dy: i32) {
        match self.view() {
            ChatView::Expanded => self.widget.scroll_by(dy),
            _ => self.list.scroll_by(dy),
        }
    }

    /// The ops of the window for the current view: the list, the entry or the help text.
    pub fn render(&self, m: &impl Metrics) -> Vec<HudOp> {
        let g = self.geom;
        let mut ops = Vec::new();
        match self.view() {
            ChatView::Help => {
                ops.push(HudOp::Text {
                    text: m.string(HELP_TEXT_ID).to_string(),
                    font: self.fonts.help,
                    x: g.help.left,
                    y: g.help.top as f32,
                    w: g.help.width(),
                    h: g.help.height(),
                    colour: TEXT_WHITE,
                    align: 0,
                });
            }
            view => {
                // chatwindow.cpp:132-169: the separator, the list, then the entry box and text.
                ops.push(HudOp::Fill {
                    rect: GuiRect::new(
                        g.separator.left,
                        g.separator.top,
                        g.separator.right,
                        g.separator.top + 1,
                    ),
                    argb: SEPARATOR_COLOUR,
                });
                let (list, rect) = if view == ChatView::Expanded {
                    (&self.widget, g.expanded_list)
                } else {
                    (&self.list, g.list)
                };
                ops.extend(list_ops(list, rect, self.fonts.chat));
                ops.push(HudOp::Fill {
                    rect: GuiRect::new(g.entry.left, g.entry.top, g.list.right, g.entry.bottom),
                    argb: BLACK,
                });
                ops.push(HudOp::Text {
                    text: self.entry.text.clone(),
                    font: self.fonts.chat,
                    x: g.entry.left,
                    y: g.entry.top as f32,
                    w: g.entry.width(),
                    h: g.entry.height(),
                    colour: TEXT_WHITE,
                    align: 0,
                });
            }
        }
        ops
    }
}

/// The ops of a list: each visible item's box and its name and message, clipped to the list.
fn list_ops(list: &ChatList, rect: GuiRect, font: FontRef) -> Vec<HudOp> {
    let mut ops = vec![HudOp::PushClip(rect)];
    // gui/alistbox.cpp:270: the list's own quad, opaque black from aObject::init (gui/asystem.cpp:72),
    // is drawn before the items, so the scene behind the list does not show through.
    ops.push(HudOp::Fill { rect, argb: BLACK });
    for i in list.visible() {
        let item = &list.items()[i];
        // Item coordinates: the box and the name are one pixel below the item's top (chatwindow.cpp:
        // 344-346, 360, 364).
        let top = rect.top + list.item_top(i) - list.scroll() + 1;
        let box_rect = GuiRect::new(
            rect.left,
            top,
            rect.left + item.rect_width(),
            top + item.line_height - 2,
        );
        ops.push(HudOp::Fill {
            rect: box_rect,
            argb: item.box_colour,
        });
        if !item.name.is_empty() {
            ops.push(HudOp::Text {
                text: item.name.clone(),
                font,
                x: rect.left + 1,
                y: (top) as f32,
                w: item.name_width,
                h: item.line_height,
                colour: item.name_colour,
                align: 0,
            });
        }
        ops.push(HudOp::Text {
            text: item.text.clone(),
            font,
            x: rect.left + item.text_x(),
            y: top as f32,
            w: item.text_width(),
            h: item.height,
            colour: item.text_colour,
            align: 0,
        });
    }
    ops.push(HudOp::PopClip);
    ops
}

/// Parts of the chat window the drawing leaves out. Every one is a missing art asset or a
/// piece of the networked chat, and nothing is faked in their place.
pub const UNPORTED: &[&str] = &[
    "button art of the three chat toggles (mcl_mp_chatwidget_01.tga, rollover rects)",
    "the list scroll bar art (aListBox::init's mcScrollBar, gui/alistbox.cpp:59-62); the scroll position is modelled, the bar is not drawn",
    "the entry caret and selection highlight (aEdit cursor art)",
    "message sending and the message source: MultiPlayer::sendChat and getChatMessages are empty (multplyr.cpp:775, 1194)",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud_info::test_support::FixedMetrics;

    const M: FixedMetrics = FixedMetrics;

    /// Sender "Kurita": 42 px name (6 chars x 7 px), so the message text is 437 - 44 - 2 = 391 px.
    fn msg(text: &str) -> ChatMessage {
        ChatMessage {
            sender: "Kurita".into(),
            sender_colour: 0x00e0_e0e0,
            text: text.into(),
            hide_name: false,
            team_text: false,
        }
    }

    /// `n` characters: 7n px, so n = 100 wraps to 2 lines and n = 112 to 3 lines at 391 px.
    fn chars(n: usize) -> String {
        "x".repeat(n)
    }

    fn texts(list: &ChatList) -> Vec<String> {
        list.items().iter().map(|i| i.text.clone()).collect()
    }

    fn frame<'a>(messages: &'a [ChatMessage], enter: bool) -> ChatFrame<'a> {
        ChatFrame {
            messages,
            enter_pressed: enter,
            commander_team: 3,
        }
    }

    fn window() -> ChatWindow {
        ChatWindow::new(geom(), ChatFonts::default(), &M)
    }

    fn geom() -> ChatGeometry {
        // mcl_mp_chatwidget.fit: [Rect1] 145,541 459x39; [Rect2] 147,578 455x0; [Edit0]
        // 146,579 452x18; [Text0] 147,541 457x55; mcl_mp_chatwidget_up.fit: [Rect2] 145,447
        // 459x131.
        ChatGeometry {
            list: GuiRect::new(145, 541, 604, 580),
            separator: GuiRect::new(147, 578, 602, 578),
            entry: GuiRect::new(146, 579, 598, 597),
            expanded_list: GuiRect::new(145, 447, 604, 578),
            help: GuiRect::new(147, 541, 604, 596),
        }
    }

    // ---- item construction (chatwindow.cpp:337-403) -------------------------------------------

    #[test]
    fn set_text_counts_lines_as_the_original_does() {
        // chatwindow.cpp:376-383: height = wrapped height; if height > 2 * lineHeight, height is
        // clamped to 2 lines and the count is 2. A text that wraps to exactly 2 lines keeps the
        // count at 1 and a height of 2 lines (line 378 is strict).
        let mut list = ChatList::new(100, 39);
        list.refill(
            &M,
            ChatFonts::default().chat,
            &[msg("hi"), msg(&chars(100)), msg(&chars(112))],
        );
        let items = list.items();
        assert_eq!(
            items.iter().map(|i| i.line_count).collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        assert_eq!(
            items.iter().map(|i| i.height).collect::<Vec<_>>(),
            vec![10, 20, 20]
        );
        assert_eq!(list.lines(), 4);
    }

    #[test]
    fn name_and_text_geometry_follow_set_player_name() {
        // chatwindow.cpp:357-366: rect width = nameWidth + 2; text x = nameWidth + 3 (relative
        // to the item); text width = 437 - rect width - 2.
        let mut list = ChatList::new(100, 39);
        list.refill(&M, ChatFonts::default().chat, &[msg("hi")]);
        let item = &list.items()[0];
        assert_eq!(item.name_width, 42);
        assert_eq!(item.rect_width(), 44);
        assert_eq!(item.text_x(), 45);
        assert_eq!(item.text_width(), 391);
    }

    #[test]
    fn name_colour_is_white_on_dark_players() {
        // chatwindow.cpp:394-397: average of the three colour bytes < 85 gives white text,
        // otherwise black. 85 itself is not < 85.
        let cases = [
            (0x0010_2030, 0xffff_ffff),
            (0x00e0_e0e0, 0xff00_0000),
            (0x0055_5555, 0xff00_0000),
            (0x0054_5454, 0xffff_ffff),
        ];
        for (colour, want) in cases {
            let mut m = msg("hi");
            m.sender_colour = colour;
            let mut list = ChatList::new(100, 39);
            list.refill(&M, ChatFonts::default().chat, &[m]);
            assert_eq!(
                list.items()[0].name_colour,
                want,
                "player colour {colour:#x}"
            );
        }
    }

    #[test]
    fn team_text_is_green_and_hidden_names_are_blank() {
        // chatwindow.cpp:257-264: flag 0x10000000 gives 0xff41c700; flag 0x01000000 blanks the
        // name (so the rect is 0 + 2 wide).
        let mut team = msg("hi");
        team.team_text = true;
        let mut hidden = msg("hi");
        hidden.hide_name = true;
        let mut list = ChatList::new(100, 39);
        list.refill(&M, ChatFonts::default().chat, &[team, hidden]);
        let items = list.items();
        assert_eq!(items[0].text_colour, 0xff41_c700);
        assert_eq!(items[1].text_colour, 0xffff_ffff);
        assert_eq!(items[1].name, "");
        assert_eq!(items[1].rect_width(), 2);
    }

    // ---- refill order, cap and trimming (chatwindow.cpp:241-299) ------------------------------

    #[test]
    fn oldest_lines_are_dropped_to_stay_within_the_cap() {
        // Cap 3 lines. [a, b] -> 2 lines. The 112-character message wraps to 3 lines but counts
        // as 2 (chatwindow.cpp:378-382), so 2 + 2 > 3 and the oldest item goes (lines 275-289):
        // [b] + 2 = 3 fits.
        let mut list = ChatList::new(3, 39);
        list.refill(&M, ChatFonts::default().chat, &[msg("a"), msg("b")]);
        assert_eq!(texts(&list), vec!["a", "b"]);
        list.refill(&M, ChatFonts::default().chat, &[msg(&chars(112))]);
        assert_eq!(texts(&list), vec!["b".to_string(), chars(112)]);
        assert_eq!(list.lines(), 3);
    }

    #[test]
    fn only_the_first_cap_messages_of_a_frame_are_used() {
        // chatwindow.cpp:244 and 291: the loops stop at min(itemCount, maxCount), so with a cap
        // of 2 lines the third message of the frame is not added at all.
        let mut list = ChatList::new(2, 39);
        list.refill(
            &M,
            ChatFonts::default().chat,
            &[msg("a"), msg("b"), msg("c")],
        );
        assert_eq!(texts(&list), vec!["a", "b"]);
    }

    #[test]
    fn newest_item_is_in_view_after_a_refill() {
        // chatwindow.cpp:295-296: SelectItem(last) when the cap is above 2, which scrolls the
        // list to its end (gui/alistbox.cpp:515-526) once the newest item is below the box.
        let mut list = ChatList::new(100, 25);
        let texts_: Vec<ChatMessage> = (0..5).map(|_| msg(&chars(100))).collect();
        list.refill(&M, ChatFonts::default().chat, &texts_);
        // 5 items of 20 px: 100 px; the scrollbar is 21 px, so the end is at scroll 79.
        assert_eq!(list.scroll(), 79);
        assert_eq!(list.visible(), 3..5);
    }

    #[test]
    fn visible_items_fit_the_box_and_scroll_moves_them() {
        // Items: a (10 px), b (10 px), c (20 px). A 25 px box with a 21 px scrollbar: total 40,
        // so the scroll range is 19 (gui/alistbox.cpp:377). After the refill the list is at its
        // end: a is above the box, b is cut at the top, c is in view.
        let mut list = ChatList::new(100, 25);
        list.refill(
            &M,
            ChatFonts::default().chat,
            &[msg("a"), msg("b"), msg(&chars(100))],
        );
        assert_eq!(list.scroll(), 19);
        assert_eq!(list.visible(), 1..3);
        // Scrolled to the top, all three show.
        list.scroll_to(0);
        assert_eq!(list.visible(), 0..3);
    }

    #[test]
    fn empty_refill_keeps_the_scroll_position() {
        // chatwindow.cpp:275-299: SelectItem runs only inside the loop that adds a message, so a
        // frame with no new messages leaves the scroll where the player put it.
        let mut list = ChatList::new(100, 25);
        let five: Vec<ChatMessage> = (0..5).map(|_| msg(&chars(100))).collect();
        list.refill(&M, ChatFonts::default().chat, &five);
        assert_eq!(list.scroll(), 79);
        list.scroll_by(-30);
        assert_eq!(list.scroll(), 49);
        list.refill(&M, ChatFonts::default().chat, &[]);
        assert_eq!(list.scroll(), 49);
    }

    #[test]
    fn a_new_message_brings_the_newest_item_into_view_after_scrolling_up() {
        // Five 2-line items (100 px), scrolled up to 49. One more 2-line item: total 120 px, scroll
        // range 120 - 21 = 99 (gui/alistbox.cpp:377). The newest item is below the box, so SelectItem
        // (chatwindow.cpp:295-296) scrolls to the end.
        let mut list = ChatList::new(100, 25);
        let five: Vec<ChatMessage> = (0..5).map(|_| msg(&chars(100))).collect();
        list.refill(&M, ChatFonts::default().chat, &five);
        list.scroll_by(-30);
        list.refill(&M, ChatFonts::default().chat, &[msg(&chars(100))]);
        assert_eq!(list.scroll(), 99);
        assert!(list.visible().contains(&5));
    }

    #[test]
    fn list_background_is_drawn_before_the_items_in_both_views() {
        // gui/alistbox.cpp:270: aListBox::render draws its own quad first. That quad is the object's
        // location, filled with 0xff000000 by aObject::init (gui/asystem.cpp:72), then the items.
        let mut w = window();
        w.update(&M, &frame(&[msg("a")], false));
        assert_background_first(&w.render(&M), geom().list);
        w.click(ChatButton::Expand);
        assert_background_first(&w.render(&M), geom().expanded_list);
    }

    /// The op after the list clip is the opaque black background over the whole list, and there are
    /// item ops after it.
    fn assert_background_first(ops: &[HudOp], rect: GuiRect) {
        let clip = ops
            .iter()
            .position(|op| matches!(op, HudOp::PushClip(r) if *r == rect))
            .expect("list clip");
        assert!(
            matches!(&ops[clip + 1], HudOp::Fill { rect: r, argb: BLACK } if *r == rect),
            "first op after the clip is {:?}",
            ops[clip + 1]
        );
        assert!(ops.len() > clip + 2, "the item ops follow the background");
    }

    #[test]
    fn list_capacities_match_the_fit() {
        // chatwindow.cpp:84: maxItems = listBox.height() / itemHeight = 39 / 10 = 3 lines in the
        // collapsed list; chatwindow.cpp:207: 128 in the expanded widget.
        let w = window();
        assert_eq!(w.list.capacity(), 3);
        assert_eq!(w.widget.capacity(), 128);
    }

    // ---- entry (chatwindow.cpp:86, 214-236) --------------------------------------------------

    #[test]
    fn entry_accepts_text_up_to_128_characters_only_when_focused() {
        // chatwindow.cpp:86: limitEntry(128).
        let mut e = ChatEntry::default();
        e.type_text("ignored");
        assert_eq!(e.text, "");
        e.focused = true;
        e.type_text("abc");
        e.type_text(&chars(200));
        assert_eq!(e.text.chars().count(), 128);
        e.backspace();
        assert_eq!(e.text.chars().count(), 127);
    }

    #[test]
    fn enter_focuses_then_submits_and_clears() {
        // chatwindow.cpp:214-236: unfocused Enter focuses the entry; focused Enter with text
        // sends it to everyone (team -1) and clears and unfocuses.
        let mut w = window();
        assert_eq!(w.update(&M, &frame(&[], true)), None);
        assert!(w.entry.focused);
        w.entry.type_text("hello");
        let out = w.update(&M, &frame(&[], true));
        assert_eq!(
            out,
            Some(ChatIntent::Submit {
                team: -1,
                text: "hello".into()
            })
        );
        assert_eq!(w.entry.text, "");
        assert!(!w.entry.focused);
    }

    #[test]
    fn team_only_sends_to_the_commander_team() {
        // chatwindow.cpp:220-221: team = TEAMONLY pressed ? commander's team : -1.
        let mut w = window();
        w.click(ChatButton::TeamOnly);
        w.update(&M, &frame(&[], true)); // focus
        w.entry.type_text("yo");
        let out = w.update(&M, &frame(&[], true));
        assert_eq!(
            out,
            Some(ChatIntent::Submit {
                team: 3,
                text: "yo".into()
            })
        );
    }

    #[test]
    fn empty_enter_only_unfocuses() {
        // chatwindow.cpp:222-227: nothing is sent for empty text, but the entry still clears
        // and loses focus.
        let mut w = window();
        w.update(&M, &frame(&[], true));
        assert_eq!(w.update(&M, &frame(&[], true)), None);
        assert!(!w.entry.focused);
    }

    #[test]
    fn enter_while_help_is_open_releases_help_and_focuses() {
        // chatwindow.cpp:231-234.
        let mut w = window();
        w.click(ChatButton::HelpToggle);
        assert_eq!(w.view(), ChatView::Help);
        w.update(&M, &frame(&[], true));
        assert!(!w.help_toggle);
        assert!(w.entry.focused);
    }

    // ---- toggles (chatwindow.cpp:105-127) ----------------------------------------------------

    #[test]
    fn help_toggle_shows_help_and_hides_the_list() {
        let mut w = window();
        assert_eq!(w.view(), ChatView::Collapsed);
        w.click(ChatButton::HelpToggle);
        assert_eq!(w.view(), ChatView::Help);
        w.click(ChatButton::HelpToggle);
        assert_eq!(w.view(), ChatView::Collapsed);
    }

    #[test]
    fn expand_releases_help_and_help_collapses_expansion() {
        // chatwindow.cpp:115-118: pressing expand releases help; 107-113: help pressed while
        // expanded releases expand.
        let mut w = window();
        w.click(ChatButton::HelpToggle);
        w.click(ChatButton::Expand);
        assert!(!w.help_toggle);
        assert_eq!(w.view(), ChatView::Expanded);
        w.click(ChatButton::HelpToggle);
        assert!(!w.expand);
        assert_eq!(w.view(), ChatView::Help);
    }

    #[test]
    fn team_only_releases_help() {
        // chatwindow.cpp:120-124: TEAMONLY always releases help (press(0)), whatever its own state.
        let mut w = window();
        w.click(ChatButton::HelpToggle);
        w.click(ChatButton::TeamOnly);
        assert!(!w.help_toggle);
        assert!(w.team_only);
    }

    #[test]
    fn every_frame_refills_both_lists() {
        // chatwindow.cpp:206-207: both lists are refilled from the same drain.
        let mut w = window();
        w.update(&M, &frame(&[msg("a")], false));
        assert_eq!(texts(&w.list), vec!["a"]);
        assert_eq!(texts(&w.widget), vec!["a"]);
    }

    #[test]
    fn chat_geometry_matches_the_fits() {
        // Reads the real chat layouts when the game data is present; skips otherwise.
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
        let mut main = vfs
            .read_fit("data/art/mcl_mp_chatwidget.fit")
            .expect("read")
            .expect("mcl_mp_chatwidget.fit");
        let mut up = vfs
            .read_fit("data/art/mcl_mp_chatwidget_up.fit")
            .expect("read")
            .expect("mcl_mp_chatwidget_up.fit");
        assert_eq!(ChatGeometry::from_fits(&mut main, &mut up), Some(geom()));
    }
}
