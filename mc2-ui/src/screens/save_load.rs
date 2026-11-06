// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The Save / Load game dialog (`LogisticsSaveDialog`, `code/logisticsdialog.cpp`) over the
//! original `mcl_sm.fit` art: the framed GAME LIST with its scroll bar, the name box, the info
//! boxes and the CANCEL / LOAD (or SAVE) / DELETE buttons.
//!
//! This is the presentation only. The slot rules (which rows exist, what a name means, what
//! saving over an existing slot does) stay with the host, which feeds the dialog its rows and
//! the current selection and acts on the [`SaveLoadEvent`]s it reports.

use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::font::FontRef;
use crate::logistics_dialog::{format_name, list_row, DialogAnim};
use crate::logistics_ids::{button, ids};
use crate::screen::{LogisticsScreen, Prefixes};
use crate::screens::dialog::{Dialog, DialogKind, NO, YES};
use crate::textures::TexId;
use crate::widgets::{ItemState, ListBox, ListItem, Object, TextListItem};
use std::any::Any;

/// `IDS_DIALOG_SAVE_GAME`, `IDS_DIALOG_LOAD_GAME`, `IDS_DIALOG_GAME_LIST`, `IDS_DIALOG_SAVE`,
/// `IDS_LOAD`.
const IDS_DIALOG_SAVE_GAME: i32 = 27660;
const IDS_DIALOG_LOAD_GAME: i32 = 27661;
const IDS_DIALOG_GAME_LIST: i32 = 27662;
const IDS_DIALOG_SAVE: i32 = 27659;
const IDS_LOAD: i32 = 27665;
const IDS_DIALOG_OVERWRITE_PROMPT: i32 = 27664;
/// `LIST_YES_BUTTON`: `buttons[2]` is the SAVE / LOAD button.
const YES_BUTTON: usize = 2;
/// `MAP_STATIC`: the mission picture of `mcl_sm.fit`.
const MAP_STATIC: usize = 10;
/// `edits[0].limitEntry(20)`.
const NAME_LIMIT: usize = 20;

/// A row of the GAME LIST that shows its state: the text takes the highlight colour under the
/// cursor and on the selected row, which also gets a band behind it.
struct SaveRow {
    item: TextListItem,
    base: u32,
    highlight: u32,
}

impl ListItem for SaveRow {
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
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let state = self.item.state;
        let lit = matches!(
            state,
            ItemState::Selected | ItemState::Highlite | ItemState::PressedHighlite
        );
        if state == ItemState::Selected {
            let o = &self.item.obj.loc;
            let band = (self.highlight & 0x00ff_ffff) | 0x4000_0000;
            ctx.dl.fill_rect(
                GuiRect::new(o[0].x as i32, o[0].y as i32, o[2].x as i32, o[2].y as i32),
                band,
            );
        }
        self.item
            .set_color(if lit { self.highlight } else { self.base });
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

/// What the host knows about one save: the list name, the info text under it and the mission
/// whose picture the info box shows.
#[derive(Clone, Debug, Default)]
pub struct SaveRowInfo {
    pub name: String,
    pub text: String,
    pub mission_file: Option<String>,
}

/// What the player did in the dialog this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveLoadEvent {
    /// A row of the list was clicked.
    Row(usize),
    /// The SAVE / LOAD button.
    Confirm,
    /// The CANCEL button.
    Cancel,
    /// The overwrite prompt was answered (`true` = replace).
    Overwrite(bool),
    /// The DELETE button (`DELETE_BUTTON`, `LogisticsSaveDialog::handleMessage`).
    Delete,
    /// The delete prompt was answered (`true` = delete the save).
    DeleteAnswer(bool),
}

pub struct SaveLoadDialog {
    pub screen: LogisticsScreen,
    pub list: ListBox,
    pub anim: DialogAnim,
    /// `IDS_DIALOG_OVERWRITE_PROMPT` with NO / YES.
    prompt: Dialog,
    prompting: bool,
    /// The prompt up is the delete prompt (`bDeletePrompt`), not the overwrite one.
    deleting: bool,
    list_font: FontRef,
    rows: Vec<SaveRowInfo>,
    pictures: Vec<Option<(TexId, u32)>>,
    shown: bool,
}

/// Pixels the info box grows by (the gaps around it are free): a long mission name wraps in
/// the original's box too, and the date and time then fell out of the frame.
const INFO_GROW: f32 = 12.0;
/// How much of the growth goes above the box (the rest below).
const INFO_GROW_UP: f32 = 6.0;

/// Makes the info text box, and the frame around it, taller so that campaign, a wrapped mission
/// name, C-Bills, date and time all show.
fn grow_info_box(screen: &mut LogisticsScreen) {
    let Some(text) = screen.texts.get(2).map(|t| t.obj.clone()) else {
        return;
    };
    let frame = (0..)
        .map_while(|i| screen.rect(i).map(|r| (i, r)))
        .filter(|(_, r)| {
            r.obj.left() <= text.left()
                && r.obj.right() >= text.right()
                && r.obj.top() <= text.top()
                && r.obj.bottom() >= text.bottom()
        })
        .min_by_key(|(_, r)| (r.obj.width() * r.obj.height()) as i64)
        .map(|(i, _)| i);
    if let Some(t) = screen.texts.get_mut(2) {
        t.obj.move_to(text.left(), text.top() - INFO_GROW_UP);
        t.obj.resize(text.width(), text.height() + INFO_GROW);
    }
    if let Some(r) = frame.and_then(|i| screen.rects.get_mut(i)) {
        let (x, y, w, h) = (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height());
        r.obj.move_to(x, y - INFO_GROW_UP);
        r.obj.resize(w, h + INFO_GROW);
    }
}

impl SaveLoadDialog {
    pub fn load(ld: &mut Loader<'_>) -> Result<SaveLoadDialog, String> {
        let mut screen = LogisticsScreen::load(ld, "mcl_sm", Prefixes::default())?;
        // the host reads the typed name itself, possibly before the first update
        screen.keep_keys();
        let (x, y, w, h) = screen.rect(1).map_or((0.0, 0.0, 0.0, 0.0), |r| {
            (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height())
        });
        let mut list = ListBox::new(ld, x, y, w, h);
        list.set_orange();
        if let Some(e) = screen.edits.first_mut() {
            e.limit = NAME_LIMIT;
        }
        grow_info_box(&mut screen);
        let anim = DialogAnim::load(ld, "mcl_sm", "InAnim", "OutAnim")?;
        Ok(SaveLoadDialog {
            anim,
            prompt: Dialog::load(ld, DialogKind::Ok)?,
            prompting: false,
            deleting: false,
            screen,
            list,
            list_font: ld.font(ids::DIALOG_LIST_FONT as u32),
            rows: Vec::new(),
            pictures: Vec::new(),
            shown: false,
        })
    }

    /// `begin` / `beginLoad`: the heading texts, the button label and the list of `names`.
    pub fn begin(&mut self, ld: &mut Loader<'_>, load: bool, rows: &[SaveRowInfo]) {
        let assets: &UiAssets = ld.assets;
        let (title, label) = if load {
            (IDS_DIALOG_LOAD_GAME, IDS_LOAD)
        } else {
            (IDS_DIALOG_SAVE_GAME, IDS_DIALOG_SAVE)
        };
        for (id, i) in [(title, 0), (IDS_DIALOG_GAME_LIST, 1)] {
            if let Some(t) = self.screen.texts.get_mut(i) {
                t.set_text_id(assets, id);
            }
        }
        if let Some(t) = self.screen.texts.get_mut(2) {
            t.set_text("");
        }
        if let Some(b) = self.screen.buttons.get_mut(YES_BUTTON) {
            b.set_text(label);
        }
        // no mission picture until a row is selected (`statics[MAP_STATIC].setColor(0)`)
        if let Some(m) = self.screen.statics.get_mut(MAP_STATIC) {
            m.set_color(0, true);
        }
        self.fill_rows(ld, rows);
        // no `screen.begin()`: it would flush the keys typed before the first frame
        self.anim.begin();
        self.shown = true;
    }

    /// `initDialog`: the GAME LIST is rebuilt from `rows` (also after a delete).
    pub fn fill_rows(&mut self, ld: &mut Loader<'_>, rows: &[SaveRowInfo]) {
        let assets: &UiAssets = ld.assets;
        self.pictures = Vec::new();
        let colour = self.screen.edits.first().map_or(0, |e| e.text_color);
        let highlight = self.screen.edits.first().map_or(0, |e| e.highlight_color);
        self.list.remove_all_items();
        for row in rows {
            let item = list_row(
                assets,
                self.list_font,
                &row.name,
                colour,
                false,
                self.list.obj.width(),
            );
            self.list.add_item(Box::new(SaveRow {
                item,
                base: colour,
                highlight,
            }));
        }
        self.rows = rows.to_vec();
        // `MissionBriefingScreen::getMissionTGA`
        self.pictures = rows
            .iter()
            .map(|r| {
                r.mission_file
                    .as_deref()
                    .and_then(|f| crate::logistics::briefing::load_map(ld, f))
            })
            .collect();
    }

    /// `bPromptOverwrite`: asks whether to replace the save called `name`.
    pub fn ask_overwrite(&mut self, assets: &UiAssets, name: &str) {
        self.prompt.set_text_ids(
            assets,
            ids::DIALOG_QUIT_PROMPT,
            ids::DIALOG_NO,
            ids::DIALOG_YES,
        );
        self.prompt.set_text(&format_name(
            assets.string(IDS_DIALOG_OVERWRITE_PROMPT as u32),
            name,
        ));
        self.prompt.begin();
        self.prompting = true;
        self.deleting = false;
    }

    /// `bDeletePrompt` (logisticsdialog.cpp:1119-1145): asks whether to delete the save `name`,
    /// `IDS_DIALOG_DELETE_PROMPT` with NO / YES.
    pub fn ask_delete(&mut self, assets: &UiAssets, name: &str) {
        self.prompt.set_text_ids(
            assets,
            ids::DIALOG_QUIT_PROMPT,
            ids::DIALOG_NO,
            ids::DIALOG_YES,
        );
        self.prompt.set_text(&format_name(
            assets.string(ids::DIALOG_DELETE_PROMPT as u32),
            name,
        ));
        self.prompt.begin();
        self.prompting = true;
        self.deleting = true;
    }

    /// The overwrite or delete prompt is up.
    pub fn is_prompting(&self) -> bool {
        self.prompting
    }

    /// The dialog has slid in and takes input.
    pub fn entered(&self) -> bool {
        self.anim.entered()
    }

    /// The panel starts sliding out (`LogisticsDialog::end`).
    pub fn end(&mut self) {
        self.anim.end();
    }

    /// The leave animation is playing.
    pub fn leaving(&self) -> bool {
        self.anim.leaving()
    }

    /// The name box has the keyboard (it was clicked into): it edits the name itself.
    pub fn edit_focused(&self) -> bool {
        self.screen.edits.first().is_some_and(|e| e.has_focus())
    }

    /// The text in the name box.
    pub fn entry(&self) -> String {
        self.screen
            .edits
            .first()
            .map(|e| e.entry())
            .unwrap_or_default()
    }

    /// The selection, the name in the edit box and the info text and picture of the selection.
    pub fn show(&mut self, assets: &UiAssets, selected: Option<usize>, name: &str) {
        let selected = selected.filter(|&n| n < self.rows.len());
        let n = selected.map_or(-1, |n| n as i32);
        if self.list.selected_item() != n {
            self.list.select_item(n);
        }
        if let Some(e) = self.screen.edits.first_mut() {
            if e.entry() != name {
                e.set_entry(assets, name, false);
            }
        }
        let info = selected.map_or("", |n| self.rows[n].text.as_str());
        if let Some(t) = self.screen.texts.get_mut(2) {
            if t.text != info {
                t.set_text(info);
            }
        }
        let picture = selected.and_then(|n| self.pictures.get(n).copied().flatten());
        if let Some(s) = self.screen.statics.get_mut(MAP_STATIC) {
            match picture {
                Some((tex, width)) => {
                    s.set_texture(Some(tex), width);
                    s.set_uvs(0.0, 127.0, 127.0, 0.0);
                    s.set_color(0xffff_ffff, true);
                }
                None => s.set_color(0, true),
            }
        }
    }

    /// The selected row's info text (`textObjects[2]`).
    pub fn info_text(&self) -> String {
        self.screen
            .texts
            .get(2)
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    /// Whether the mission picture is showing.
    pub fn picture_shown(&self) -> bool {
        self.screen
            .statics
            .get(MAP_STATIC)
            .is_some_and(|s| s.color() != 0)
    }

    /// The centre of list row `i`, in UI coordinates.
    pub fn row_centre(&self, i: usize) -> Option<(f32, f32)> {
        let o = self.list.item(i)?.obj();
        Some((
            o.global_x() as f32 + o.width() / 2.0,
            o.global_y() as f32 + o.height() / 2.0,
        ))
    }

    /// The centre of the name box, in UI coordinates.
    pub fn edit_centre(&self) -> Option<(f32, f32)> {
        let o = &self.screen.edits.first()?.obj;
        Some((
            o.global_x() as f32 + o.width() / 2.0,
            o.global_y() as f32 + o.height() / 2.0,
        ))
    }

    /// A point inside the list under its last row (`None` when the rows fill it).
    pub fn blank_point(&self) -> Option<(f32, f32)> {
        let o = &self.list.obj;
        let bottom = (0..self.rows.len())
            .filter_map(|i| self.list.item(i))
            .map(|it| it.obj().global_y() as f32 + it.obj().height())
            .fold(o.global_y() as f32, f32::max);
        let y = bottom + 4.0;
        let x = o.global_x() as f32 + 6.0;
        (y < o.global_y() as f32 + o.height()).then_some((x, y))
    }

    /// The centre of the overwrite prompt's YES or NO button, in UI coordinates.
    pub fn prompt_button_centre(&self, yes: bool) -> Option<(f32, f32)> {
        let b = self.prompt.screen.button(if yes { YES } else { NO })?;
        Some((
            b.global_x() as f32 + b.width() / 2.0,
            b.global_y() as f32 + b.height() / 2.0,
        ))
    }

    /// The centre of the DELETE button and whether it is enabled.
    pub fn delete_button(&self) -> Option<((f32, f32), bool)> {
        let b = self.screen.button(button::DELETE_BUTTON)?;
        Some((
            (
                b.global_x() as f32 + b.width() / 2.0,
                b.global_y() as f32 + b.height() / 2.0,
            ),
            b.button.is_enabled(),
        ))
    }

    /// The centre of the SAVE / LOAD button and whether it is enabled.
    pub fn confirm_button(&self) -> Option<((f32, f32), bool)> {
        self.button_state(button::YES)
    }

    /// The centre of the CANCEL button and whether it is enabled.
    pub fn cancel_button(&self) -> Option<((f32, f32), bool)> {
        self.button_state(button::NO)
    }

    fn button_state(&self, id: i32) -> Option<((f32, f32), bool)> {
        let b = self.screen.button(id)?;
        Some((
            (
                b.global_x() as f32 + b.width() / 2.0,
                b.global_y() as f32 + b.height() / 2.0,
            ),
            b.button.is_enabled(),
        ))
    }

    /// Enables the SAVE / LOAD button.
    pub fn set_confirm_enabled(&mut self, on: bool) {
        if let Some(b) = self.screen.button_mut(button::YES) {
            b.disable(!on);
        }
    }

    /// Runs the buttons and the list; reports what was clicked.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Vec<SaveLoadEvent> {
        self.anim.update(ctx.dt);
        let mut out = Vec::new();
        if self.prompting {
            self.prompt.update(ctx);
            if self.prompt.is_done() {
                self.prompting = false;
                let yes = self.prompt.result == YES;
                out.push(if self.deleting {
                    SaveLoadEvent::DeleteAnswer(yes)
                } else {
                    SaveLoadEvent::Overwrite(yes)
                });
            }
            return out;
        }
        if !self.shown || !self.anim.entered() || self.anim.leaving() {
            return out;
        }
        // `LogisticsSaveDialog::update`: `buttons[0].disable( 0 )` every frame, so DELETE is
        // always enabled (logisticsdialog.cpp:684-685)
        if let Some(b) = self.screen.button_mut(button::DELETE_BUTTON) {
            b.disable(false);
        }
        for m in self.screen.update(ctx) {
            match m.who {
                button::DELETE_BUTTON => out.push(SaveLoadEvent::Delete),
                button::YES => out.push(SaveLoadEvent::Confirm),
                button::NO => out.push(SaveLoadEvent::Cancel),
                _ => {}
            }
        }
        self.list.update(ctx);
        if ctx.input.is_left_click() {
            // only a click on a row selects it, not the empty space under the rows
            let (x, y) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            if self.list.point_inside(x, y) && !self.list.point_in_scroll_bar(x, y) {
                let hit = (0..self.rows.len())
                    .find(|&i| self.list.item(i).is_some_and(|it| it.point_inside(x, y)));
                if let Some(n) = hit {
                    out.push(SaveLoadEvent::Row(n));
                }
            }
        }
        out
    }

    /// Draws the dimming and the panel, sliding in with `InAnim` and out with `OutAnim`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let (colour, x, y) = self.anim.frame();
        if colour != 0 {
            let (w, h) = ctx.dl.screen;
            ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), colour);
        }
        self.list.move_by(x, y);
        self.list.render(ctx);
        self.list.move_by(-x, -y);
        self.screen.render_offset(ctx, x as i32, y as i32);
        if self.prompting {
            self.prompt.render(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<SaveRowInfo> {
        ["Autosave", "Slot 01", "Slot 02"]
            .iter()
            .map(|n| SaveRowInfo {
                name: (*n).to_string(),
                text: format!("{n}\nC-BILLS : 5"),
                mission_file: None,
            })
            .collect()
    }

    fn quads(dialog: &mut SaveLoadDialog, assets: &UiAssets) -> usize {
        let mut dl = crate::draw::DrawList::new(800, 600);
        let mut ctx = RenderCtx {
            dl: &mut dl,
            assets,
            dt: 0.0,
        };
        dialog.list.render(&mut ctx);
        dl.vertices.len()
    }

    #[test]
    fn the_dialog_lists_rows_and_labels_its_buttons_from_the_original_art() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = UiAssets::default();
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut dialog = SaveLoadDialog::load(&mut ld).unwrap();
        dialog.begin(&mut ld, true, &rows());
        assert!(!dialog.entered(), "the panel slides in first");
        assert!(dialog.row_centre(1).is_some());
        assert!(dialog.row_centre(3).is_none());
        assert_eq!(dialog.screen.texts[0].text, assets.string(27661));
        assert_eq!(dialog.screen.texts[1].text, assets.string(27662));
        dialog.show(&assets, Some(1), "Slot 01");
        assert_eq!(dialog.list.selected_item(), 1);
        assert!(dialog.info_text().starts_with("Slot 01"));
        assert!(!dialog.picture_shown(), "no mission, no picture");
        dialog.show(&assets, Some(2), "Slot 02");
        assert!(dialog.info_text().starts_with("Slot 02"));
        let mut ld = Loader::new(&mut vfs, &mut assets);
        dialog.begin(&mut ld, false, &rows());
        assert_eq!(dialog.screen.texts[0].text, assets.string(27660));
    }

    #[test]
    fn the_selected_row_looks_different_from_the_others() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = UiAssets::default();
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut dialog = SaveLoadDialog::load(&mut ld).unwrap();
        dialog.begin(&mut ld, true, &rows());
        dialog.show(&assets, None, "");
        let plain = quads(&mut dialog, &assets);
        dialog.show(&assets, Some(1), "Slot 01");
        let selected = quads(&mut dialog, &assets);
        assert!(selected > plain, "the selected row gets a band");
        // keyboard navigation moves the band with the selection
        dialog.show(&assets, Some(2), "Slot 02");
        assert_eq!(dialog.list.selected_item(), 2);
        let state = |d: &SaveLoadDialog, i| d.list.item(i).unwrap().state();
        assert_eq!(state(&dialog, 2), ItemState::Selected);
        assert_ne!(state(&dialog, 1), ItemState::Selected);
    }
}
