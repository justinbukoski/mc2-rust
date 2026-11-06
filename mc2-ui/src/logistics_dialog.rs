// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The logistics dialogs (`code/logisticsdialog.cpp`): the yes/no prompt (`LogisticsOKDialog`),
//! the one-button message (`LogisticsOneButtonDialog`) and the variant save and accept dialogs
//! (`LogisticsVariantDialog`, `LogisticsAcceptVariantDialog`) that name the loadout the mech lab
//! keeps.
//!
//! Each dialog is a [`LogisticsScreen`] built from its `.fit` layout. A dialog is open from
//! `begin` until a button closes it; the status then holds `Yes` or `No`, as
//! `LogisticsDialog::getStatus` does. The enter and leave animations (`EnterAnim`/`LeaveAnim`,
//! `InAnim`/`OutAnim` for the list dialogs) slide the panel and fade the backdrop ([`DialogAnim`]):
//! the dialog takes no input until it has entered, and after a button closes it the panel
//! slides out while the caller already has the status. The rules for replacing and deleting
//! variants are the campaign's (`canReplaceVariant`, `canDeleteVariant`, `removeVariant`).

use crate::anim::Animation;
use crate::color::interpolate_color;
use crate::ctx::{Loader, Msg, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::font::FontRef;
use crate::logistics_ids::{button, ids};
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::widgets::{ItemState, ListBox, ListItem, TextListItem};
use mc2_campaign::Logistics;
use mc2_platform::input::KeyStatus;
use mc2_platform::keys::{KEY_ESCAPE, KEY_RETURN};

/// The grey of a disabled row (`0xff373737`, `LogisticsVariantDialog::update`).
pub const DISABLED_ROW_COLOUR: u32 = 0xff37_3737;

/// A row of a logistics list: `text` in `colour`, greyed out and unpickable when `disabled`,
/// as wide as the list. A list row without a colour is black on the black panels.
pub fn list_row(
    assets: &UiAssets,
    font: FontRef,
    text: &str,
    colour: u32,
    disabled: bool,
    width: f32,
) -> TextListItem {
    let mut item = TextListItem::new(assets, font);
    item.set_text(text);
    item.set_color(if disabled {
        DISABLED_ROW_COLOUR
    } else {
        colour
    });
    if disabled {
        item.set_state(ItemState::Disabled);
    }
    let h = item.obj().height();
    item.resize(width, h);
    item
}

/// The enter and leave animations of a `LogisticsDialog` (`enterAnim`, `exitAnim`), with the
/// backdrop fade and the panel offset `LogisticsDialog::render` derives from them.
#[derive(Clone, Debug, Default)]
pub struct DialogAnim {
    pub enter: Animation,
    pub exit: Animation,
    /// Off, the dialog opens and closes at once, takes input at once and draws no backdrop (the
    /// caller dims the screen itself, as the pause window does).
    pub enabled: bool,
}

/// `0x7f000000`: the dimming behind a dialog.
const BACKDROP: u32 = 0x7f00_0000;

impl DialogAnim {
    /// Reads the `enter` and `leave` blocks of the layout `layout` (`file.seekBlock("EnterAnim")`,
    /// `enterAnim.init(&file, "")`, and the same for the leave block).
    pub fn load(
        ld: &mut Loader<'_>,
        layout: &str,
        enter: &str,
        leave: &str,
    ) -> Result<DialogAnim, String> {
        let mut file = ld.art_fit(layout)?;
        Ok(DialogAnim {
            enter: Animation::from_block(&mut file, enter),
            exit: Animation::from_block(&mut file, leave),
            enabled: true,
        })
    }

    /// `LogisticsDialog::begin`.
    pub fn begin(&mut self) {
        if !self.enabled {
            return;
        }
        self.enter.begin();
        self.exit.end();
    }

    /// `LogisticsDialog::end`: the panel leaves.
    pub fn end(&mut self) {
        if !self.enabled {
            return;
        }
        self.enter.end();
        self.exit.begin();
    }

    pub fn update(&mut self, dt: f32) {
        self.enter.update(dt);
        self.exit.update(dt);
    }

    /// `enterAnim.isDone()`: the dialog takes input. A layout without an enter animation is
    /// ready at once.
    pub fn entered(&self) -> bool {
        !self.enabled || self.enter.infos.is_empty() || self.enter.is_done()
    }

    /// The leave animation is playing.
    pub fn leaving(&self) -> bool {
        self.enabled && self.exit.is_animating() && !self.exit.is_done()
    }

    /// The backdrop colour and the panel offset of this frame (`LogisticsDialog::render`).
    pub fn frame(&self) -> (u32, f32, f32) {
        if !self.enabled {
            return (0, 0.0, 0.0);
        }
        let mut colour = BACKDROP;
        if self.enter.is_animating() && !self.enter.is_done() {
            let end = self.enter.max_time();
            if end != 0.0 {
                colour = interpolate_color(0, BACKDROP, self.enter.current_time / end);
            }
        } else if self.exit.is_animating() && !self.exit.is_done() {
            let end = self.exit.max_time();
            if end != 0.0 {
                colour = interpolate_color(BACKDROP, 0, self.exit.current_time / end);
            }
        }
        let (x, y) = if self.enter.is_animating() {
            (self.enter.x_delta(), self.enter.y_delta())
        } else if self.exit.is_animating() {
            (self.exit.x_delta(), self.exit.y_delta())
        } else {
            (0.0, 0.0)
        };
        (colour, x, y)
    }

    /// Draws the backdrop and `screen` at the animation's offset (`LogisticsDialog::render`).
    pub fn render(&self, ctx: &mut RenderCtx<'_>, screen: &mut LogisticsScreen) {
        let (colour, x, y) = self.frame();
        let (w, h) = ctx.dl.screen;
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), colour);
        screen.render_offset(ctx, x as i32, y as i32);
    }
}

/// `sprintf(text, format, name)` for the one `%s` the dialog strings have.
pub fn format_name(format: &str, name: &str) -> String {
    match format.find("%s") {
        Some(i) => format!("{}{name}{}", &format[..i], &format[i + 2..]),
        None => format.to_string(),
    }
}

/// `status = who` for the dialog buttons: Yes or No, anything else keeps the dialog open.
fn close_status(who: i32) -> Option<Status> {
    match who {
        button::YES => Some(Status::Yes),
        button::NO => Some(Status::No),
        _ => None,
    }
}

/// The yes/no prompt (`mcl_dialog.fit`, `LogisticsOKDialog`).
pub struct YesNoDialog {
    pub screen: LogisticsScreen,
    pub status: Status,
    pub anim: DialogAnim,
    open: bool,
}

impl YesNoDialog {
    /// The prompt, sliding in and out with its animations (same as [`Self::load_animated`]).
    pub fn load(ld: &mut Loader<'_>) -> Result<YesNoDialog, String> {
        YesNoDialog::load_animated(ld)
    }

    /// The prompt slides in and out with `EnterAnim`/`LeaveAnim` over a dimmed backdrop, as
    /// `LogisticsOKDialog` does.
    pub fn load_animated(ld: &mut Loader<'_>) -> Result<YesNoDialog, String> {
        Ok(YesNoDialog {
            screen: LogisticsScreen::load(ld, "mcl_dialog", Prefixes::default())?,
            status: Status::Running,
            anim: DialogAnim::load(ld, "mcl_dialog", "EnterAnim", "LeaveAnim")?,
            open: false,
        })
    }

    /// `setText(textID, CancelButton, OKButton)` with the question as a string already looked
    /// up: `No` is the cancel button, `Yes` the OK button.
    pub fn set_text(&mut self, text: &str, no_label: i32, yes_label: i32) {
        if let Some(t) = self.screen.texts.get_mut(0) {
            t.set_text(text);
        }
        if let Some(b) = self.screen.buttons.get_mut(0) {
            b.set_text(no_label);
        }
        if let Some(b) = self.screen.buttons.get_mut(1) {
            b.set_text(yes_label);
        }
    }

    pub fn begin(&mut self) {
        self.status = Status::Running;
        self.open = true;
        self.screen.begin();
        self.anim.begin();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The panel is sliding out after an answer.
    pub fn is_leaving(&self) -> bool {
        !self.open && self.anim.leaving()
    }

    /// Closes the prompt without an answer (the question went away with its screen).
    pub fn close(&mut self) {
        if self.open {
            self.anim.end();
        }
        self.open = false;
    }

    /// Returns the answer on the frame the prompt closes. A prompt that is still entering takes
    /// no input (`LogisticsDialog::update`).
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Status> {
        self.anim.update(ctx.dt);
        if !self.open || !self.anim.entered() {
            return None;
        }
        for m in self.screen.update(ctx) {
            if let Some(s) = close_status(m.who) {
                self.status = s;
                self.open = false;
                self.anim.end();
                return Some(s);
            }
        }
        None
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.open || self.anim.leaving() {
            self.anim.render(ctx, &mut self.screen);
        }
    }
}

/// The one-button message (`mcl_dialog_onebutton.fit`, `LogisticsOneButtonDialog`).
pub struct OneButtonDialog {
    pub screen: LogisticsScreen,
    pub status: Status,
    pub anim: DialogAnim,
    open: bool,
}

impl OneButtonDialog {
    pub fn load(ld: &mut Loader<'_>) -> Result<OneButtonDialog, String> {
        Ok(OneButtonDialog {
            screen: LogisticsScreen::load(ld, "mcl_dialog_onebutton", Prefixes::default())?,
            status: Status::Running,
            anim: DialogAnim::load(ld, "mcl_dialog_onebutton", "EnterAnim", "LeaveAnim")?,
            open: false,
        })
    }

    /// `setText(textID, CancelButton, OKButton)`: the one button shows `button_label`.
    pub fn set_text(&mut self, text: &str, button_label: i32) {
        if let Some(t) = self.screen.texts.get_mut(0) {
            t.set_text(text);
        }
        if let Some(b) = self.screen.buttons.get_mut(0) {
            b.set_text(button_label);
        }
    }

    pub fn begin(&mut self) {
        self.status = Status::Running;
        self.open = true;
        self.screen.begin();
        self.anim.begin();
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The panel is sliding out after an answer.
    pub fn is_leaving(&self) -> bool {
        !self.open && self.anim.leaving()
    }

    /// Returns the button's status on the frame it is pressed.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Status> {
        self.anim.update(ctx.dt);
        if !self.open || !self.anim.entered() {
            return None;
        }
        for m in self.screen.update(ctx) {
            if let Some(s) = close_status(m.who) {
                self.status = s;
                self.open = false;
                self.anim.end();
                return Some(s);
            }
        }
        None
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.open || self.anim.leaving() {
            self.anim.render(ctx, &mut self.screen);
        }
    }
}

/// Which variant dialog it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantDialogKind {
    /// `MCL_SV.fit`, `LogisticsVariantDialog`: "save variant", with the list and Delete.
    Save,
    /// `mcl_sv_acceptchanges.fit`, `LogisticsAcceptVariantDialog`: shown when the new loadout
    /// has components the player does not have yet.
    Accept,
}

/// The prompt a variant dialog has open over itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prompt {
    None,
    /// The typed name is taken: replace it?
    Overwrite,
    /// Delete the named variant?
    Delete,
}

/// The variant save or accept dialog. The name it returns is in [`VariantDialog::name`].
pub struct VariantDialog {
    pub kind: VariantDialogKind,
    pub screen: LogisticsScreen,
    /// The player's variants (orange list over `Rect1`).
    pub list: ListBox,
    /// The name picked or typed (`selectedName`).
    pub name: String,
    pub status: Status,
    pub anim: DialogAnim,
    open: bool,
    prompt: Prompt,
    prompt_dialog: YesNoDialog,
    list_font: FontRef,
}

impl VariantDialog {
    pub fn load(ld: &mut Loader<'_>, kind: VariantDialogKind) -> Result<VariantDialog, String> {
        let layout = match kind {
            VariantDialogKind::Save => "mcl_sv",
            VariantDialogKind::Accept => "mcl_sv_acceptchanges",
        };
        let mut screen = LogisticsScreen::load(ld, layout, Prefixes::default())?;
        // `edits[0].limitEntry(14)`
        if let Some(e) = screen.edits.get_mut(0) {
            e.limit = 14;
        }
        let (x, y, w, h) = screen.rect(1).map_or((0.0, 0.0, 0.0, 0.0), |r| {
            (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height())
        });
        let mut list = ListBox::new(ld, x, y, w, h);
        list.set_orange();
        Ok(VariantDialog {
            kind,
            screen,
            list,
            name: String::new(),
            status: Status::Running,
            anim: DialogAnim::load(ld, layout, "InAnim", "OutAnim")?,
            open: false,
            prompt: Prompt::None,
            prompt_dialog: YesNoDialog::load_animated(ld)?,
            list_font: ld.font(ids::DIALOG_LIST_FONT as u32),
        })
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The panel is sliding out after an answer.
    pub fn is_leaving(&self) -> bool {
        !self.open && self.anim.leaving()
    }

    /// The dialog is done: the panel leaves.
    fn close(&mut self) {
        self.open = false;
        self.anim.end();
    }

    /// `begin`: the field is empty and focused and the list shows the player's variants. The
    /// save dialog's heading is set here (`setText(IDS_DIALOG_VARIANT_SAVE)`).
    pub fn begin(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics) {
        self.status = Status::Running;
        self.prompt = Prompt::None;
        self.open = true;
        self.anim.begin();
        if let Some(e) = self.screen.edits.get_mut(0) {
            e.set_entry(ctx.assets, "", false);
            e.set_focus(ctx, true);
        }
        self.screen.begin();
        if self.kind == VariantDialogKind::Save {
            if let Some(t) = self.screen.texts.get_mut(0) {
                t.set_text_id(ctx.assets, ids::DIALOG_VARIANT_SAVE);
            }
            if let Some(t) = self.screen.texts.get_mut(1) {
                t.set_text_id(ctx.assets, ids::DIALOG_VARIANT_LIST);
            }
            if let Some(b) = self.screen.button_mut(button::YES) {
                b.set_text(ids::DIALOG_SAVE);
            }
        }
        self.fill_list(ctx.assets, logistics);
    }

    /// `initVariantList`: one row per player variant. A name that may not be replaced is greyed
    /// out; the first one that may be is picked and typed into the field.
    pub fn fill_list(&mut self, assets: &UiAssets, logistics: &Logistics) {
        self.list.remove_all_items();
        let mut picked = false;
        for name in logistics.player_variant_names() {
            let replaceable = logistics.can_replace_variant(&name);
            let colour = self.screen.edits.first().map_or(0, |e| e.text_color);
            let row = list_row(
                assets,
                self.list_font,
                &name,
                colour,
                !replaceable,
                self.list.obj.width(),
            );
            let index = self.list.add_item(Box::new(row));
            if replaceable && !picked {
                picked = true;
                if let Some(i) = index {
                    self.list.select_item(i as i32);
                }
                if let Some(e) = self.screen.edits.get_mut(0) {
                    e.set_entry(assets, &name, true);
                }
                self.name = name;
            }
        }
    }

    /// The yes/no prompt this dialog opens over itself.
    pub fn prompt_dialog(&self) -> &YesNoDialog {
        &self.prompt_dialog
    }

    /// The text in the name field.
    pub fn typed_name(&self) -> String {
        self.screen
            .edits
            .first()
            .map(|e| e.entry())
            .unwrap_or_default()
    }

    /// `update`: the open prompt first, then the layout's buttons, the list and the keys
    /// (`LogisticsDialog::update`: Enter accepts when the field is not focused, Escape cancels).
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        self.anim.update(ctx.dt);
        if self.prompt == Prompt::None {
            // an answered prompt slides out while the dialog goes on
            self.prompt_dialog.anim.update(ctx.dt);
        }
        if !self.open {
            return;
        }
        match self.prompt {
            Prompt::Overwrite => {
                if let Some(s) = self.prompt_dialog.update(ctx) {
                    self.prompt = Prompt::None;
                    if s == Status::Yes {
                        self.status = Status::Yes;
                        self.close();
                    }
                }
                return;
            }
            Prompt::Delete => {
                if let Some(s) = self.prompt_dialog.update(ctx) {
                    self.prompt = Prompt::None;
                    if s == Status::Yes {
                        // `removeVariant(selectedName)`: the campaign refuses what it may not
                        // delete, and the button is disabled for those.
                        let _ = logistics.remove_variant(&self.name);
                        self.fill_list(ctx.assets, logistics);
                        if let Some(e) = self.screen.edits.get_mut(0) {
                            e.set_entry(ctx.assets, "", false);
                        }
                    }
                }
                return;
            }
            Prompt::None => {}
        }
        // the panel takes no input until it has entered (`LogisticsDialog::update`)
        if !self.anim.entered() {
            return;
        }

        let mut msgs = self.screen.update(ctx);
        // the list picks the clicked row first (`ListBox::update`), then the click reads it
        self.list.update(ctx);
        if ctx.input.is_left_click() {
            let (x, y) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            if self.list.point_inside(x, y) {
                if let Ok(i) = usize::try_from(self.list.selected_item()) {
                    let text = self
                        .list
                        .item(i)
                        .and_then(|it| it.text())
                        .map(str::to_string);
                    if let Some(text) = text {
                        if let Some(e) = self.screen.edits.get_mut(0) {
                            e.set_entry(ctx.assets, &text, true);
                        }
                        self.name = text;
                    }
                }
            }
        }
        let focused = self.screen.edits.iter().any(|e| e.has_focus());
        if !focused && ctx.input.key_status(KEY_RETURN) == KeyStatus::Released {
            let yes_enabled = self
                .screen
                .button(button::YES)
                .is_none_or(|b| b.is_enabled());
            if yes_enabled {
                msgs.push(Msg::new(0, button::YES));
            }
        }
        if ctx.input.key_status(KEY_ESCAPE) == KeyStatus::Released {
            msgs.push(Msg::new(0, button::NO));
        }
        for m in msgs {
            self.handle(ctx.assets, m.who);
            if !self.open || self.prompt != Prompt::None {
                break;
            }
        }
        self.update_enables(logistics);
    }

    /// `handleMessage`. Yes saves under the typed name, after the overwrite prompt when the name
    /// is in the list; No cancels; Delete asks about the typed name.
    fn handle(&mut self, assets: &UiAssets, who: i32) {
        match who {
            button::YES => {
                self.name = self.typed_name();
                if self.list_has(&self.name) {
                    let text = format_name(
                        assets.string(ids::DIALOG_OVERWRITE_PROMPT as u32),
                        &self.name,
                    );
                    self.open_prompt(&text, Prompt::Overwrite);
                } else {
                    self.status = Status::Yes;
                    self.close();
                }
            }
            button::NO => {
                self.name.clear();
                self.status = Status::No;
                self.close();
            }
            button::DELETE_BUTTON => {
                self.name = self.typed_name();
                let text = format_name(assets.string(ids::DIALOG_DELETE_PROMPT as u32), &self.name);
                self.open_prompt(&text, Prompt::Delete);
            }
            _ => {}
        }
    }

    /// `setText(IDS_DIALOG_QUIT_PROMPT, NO, YES)` then `setText(promptString)`: the prompt's
    /// text replaces the question.
    fn open_prompt(&mut self, text: &str, prompt: Prompt) {
        self.prompt_dialog
            .set_text(text, ids::DIALOG_NO, ids::DIALOG_YES);
        self.prompt_dialog.begin();
        self.prompt = prompt;
    }

    /// Whether the list holds a variant of this name. The campaign finds and replaces variants
    /// ignoring case, so the overwrite prompt does too (`LogisticsVariantDialog::handleMessage`).
    fn list_has(&self, name: &str) -> bool {
        (0..self.list.item_count()).any(|i| {
            self.list
                .item(i)
                .and_then(|it| it.text())
                .is_some_and(|t| t.eq_ignore_ascii_case(name))
        })
    }

    /// The enables of the buttons. Save needs a typed name the campaign lets replace; the accept
    /// dialog's Yes needs the same. Delete needs a variant the campaign lets go.
    fn update_enables(&mut self, logistics: &Logistics) {
        let text = self.typed_name();
        let replaceable = logistics.can_replace_variant(&text);
        let save_ok = match self.kind {
            VariantDialogKind::Save => !text.is_empty() && replaceable,
            VariantDialogKind::Accept => replaceable,
        };
        if let Some(b) = self.screen.button_mut(button::YES) {
            b.disable(!save_ok);
        }
        if self.kind == VariantDialogKind::Save {
            let can_delete = logistics.can_delete_variant(&text);
            if let Some(b) = self.screen.button_mut(button::DELETE_BUTTON) {
                b.disable(!can_delete);
            }
        }
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.open && !self.anim.leaving() {
            return;
        }
        let (colour, x, y) = self.anim.frame();
        let (w, h) = ctx.dl.screen;
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), colour);
        // the list slides with the panel (`gameListBox.move(xOffset, yOffset)`)
        self.list.move_by(x, y);
        self.list.render(ctx);
        self.list.move_by(-x, -y);
        self.screen.render_offset(ctx, x as i32, y as i32);
        self.prompt_dialog.render(ctx);
    }
}

/// `IDS_DIALOG_LOAD_GAME`, `IDS_DIALOG_GAME_LIST` and `IDS_LOAD`: the texts `beginLoad` puts on the
/// campaign list (`LogisticsSaveDialog::beginCampaign` calls `beginLoad` first).
const IDS_DIALOG_LOAD_GAME: i32 = 27661;
const IDS_DIALOG_GAME_LIST: i32 = 27662;
const IDS_LOAD: i32 = 27665;
/// `MAP_STATIC`: the mission picture of `mcl_sm.fit`.
const MAP_STATIC: usize = 10;
/// `edits[0].limitEntry(20)`.
const CAMPAIGN_NAME_LIMIT: usize = 20;
/// `PlayerMessage`-less `YES` button of the list dialog: the `LOAD` button, `buttons[2]`.
const LIST_YES_BUTTON: usize = 2;

/// One campaign the New Campaign list offers (`initDialog`): the script stem
/// (`getHiddenText`), the name shown, and the first mission it starts
/// (`updateCampaignMissionInfo`: `Group0Mission0`'s `FileName`, empty when missing).
#[derive(Clone, Debug, PartialEq)]
pub struct CampaignRow {
    pub stem: String,
    pub name: String,
    pub first_mission: String,
}

impl CampaignRow {
    /// The rows `initDialog` lists for New Campaign: `campaign`, then `tutorial`, each named as
    /// `readCampaignNameFromFile` does (`NameID` through the string table, else `CampaignName`)
    /// and carrying the `FileName` of its `Group0Mission0`.
    pub fn from_data(data: &mc2_campaign::GameData) -> Vec<CampaignRow> {
        let table = crate::strings::StringTable::builtin();
        mc2_campaign::data::SHIPPED_CAMPAIGNS
            .iter()
            .filter_map(|stem| {
                let campaign = data.campaigns.get(*stem)?;
                let name = match (campaign.name_id, &campaign.campaign_name) {
                    (Some(id), _) => u32::try_from(id)
                        .ok()
                        .and_then(|id| table.get(id))
                        .map(str::to_string),
                    (None, Some(name)) => Some(name.clone()),
                    (None, None) => None,
                };
                let first_mission = campaign
                    .groups
                    .first()
                    .and_then(|g| g.missions.first())
                    .map(|m| m.file_name.clone())
                    .unwrap_or_default();
                Some(CampaignRow {
                    stem: (*stem).to_string(),
                    name: name.unwrap_or_else(|| (*stem).to_string()),
                    first_mission,
                })
            })
            .collect()
    }
}

/// What `setMission` shows for a row: the map picture and the mission's name.
#[derive(Clone, Debug)]
struct MissionInfo {
    map: Option<(crate::textures::TexId, u32)>,
    name: String,
}

/// The New Campaign list (`mcl_sm.fit`, `LogisticsSaveDialog::beginCampaign`): the campaigns in a
/// list over `Rect1`, the name edit box, the first mission's map and name, and the Load and
/// Cancel buttons, sliding in and out with `InAnim`/`OutAnim`.
pub struct CampaignDialog {
    pub screen: LogisticsScreen,
    pub list: ListBox,
    pub anim: DialogAnim,
    rows: Vec<CampaignRow>,
    info: Vec<MissionInfo>,
    selected: Option<usize>,
    /// The stem the player confirmed (`getFileName`), valid when the status is `Yes`.
    pub chosen: String,
    pub status: Status,
    open: bool,
    needs_init: bool,
    list_font: FontRef,
}

impl CampaignDialog {
    pub fn load(ld: &mut Loader<'_>) -> Result<CampaignDialog, String> {
        // `((LogisticsScreen*)s_instance)->init(file, "Static", "Text", "Rect", "Button")`
        let mut screen = LogisticsScreen::load(
            ld,
            "mcl_sm",
            Prefixes {
                edits: Some("Edit"),
                ..Prefixes::default()
            },
        )?;
        let (x, y, w, h) = screen.rect(1).map_or((0.0, 0.0, 0.0, 0.0), |r| {
            (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height())
        });
        let mut list = ListBox::new(ld, x, y, w, h);
        list.set_orange();
        if let Some(e) = screen.edits.get_mut(0) {
            e.limit = CAMPAIGN_NAME_LIMIT;
        }
        Ok(CampaignDialog {
            anim: DialogAnim::load(ld, "mcl_sm", "InAnim", "OutAnim")?,
            screen,
            list,
            rows: Vec::new(),
            info: Vec::new(),
            selected: None,
            chosen: String::new(),
            status: Status::Running,
            open: false,
            needs_init: false,
            list_font: ld.font(ids::DIALOG_LIST_FONT as u32),
        })
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The panel is sliding out after an answer.
    pub fn is_leaving(&self) -> bool {
        !self.open && self.anim.leaving()
    }

    pub fn rows(&self) -> &[CampaignRow] {
        &self.rows
    }

    /// The selected campaign's row (`None` when the edit box has been clicked into).
    pub fn selected(&self) -> Option<&CampaignRow> {
        self.selected.and_then(|i| self.rows.get(i))
    }

    /// The mission name under the map picture (`textObjects[2]`).
    pub fn mission_text(&self) -> String {
        self.screen
            .texts
            .get(2)
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    /// The centre of the Load (`yes`) or Cancel button, in UI coordinates.
    pub fn button_centre(&self, yes: bool) -> Option<(f32, f32)> {
        let b = self
            .screen
            .button(if yes { button::YES } else { button::NO })?;
        Some((
            b.global_x() as f32 + b.width() / 2.0,
            b.global_y() as f32 + b.height() / 2.0,
        ))
    }

    /// The centre of list row `i`, in UI coordinates.
    pub fn row_centre(&self, i: usize) -> Option<(f32, f32)> {
        let o = self.list.item(i)?.obj();
        Some((
            o.global_x() as f32 + o.width() / 2.0,
            o.global_y() as f32 + o.height() / 2.0,
        ))
    }

    /// `beginCampaign`: lists `rows`, loading each one's mission picture and name now (the dialog
    /// has no loader later). The first row is selected on the first update.
    pub fn begin(&mut self, ld: &mut Loader<'_>, rows: Vec<CampaignRow>) {
        self.info = rows
            .iter()
            .map(|r| mission_info(ld, &r.first_mission))
            .collect();
        self.rows = rows;
        self.selected = None;
        self.chosen.clear();
        self.status = Status::Running;
        self.open = true;
        self.needs_init = true;
        self.screen.begin();
        self.anim.begin();
    }

    /// `initDialog` and the first-row selection: the list, the heading texts of `beginLoad`, the
    /// edit box holding the first name and its mission info.
    fn init(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.needs_init = false;
        for (id, i) in [(IDS_DIALOG_LOAD_GAME, 0), (IDS_DIALOG_GAME_LIST, 1)] {
            if let Some(t) = self.screen.texts.get_mut(i) {
                t.set_text_id(ctx.assets, id);
            }
        }
        if let Some(t) = self.screen.texts.get_mut(2) {
            t.set_text("");
        }
        if let Some(b) = self.screen.buttons.get_mut(LIST_YES_BUTTON) {
            b.set_text(IDS_LOAD);
        }
        if let Some(e) = self.screen.edits.get_mut(0) {
            e.set_entry(ctx.assets, "", false);
            e.set_focus(ctx, true);
        }
        // `DELETE_BUTTON` removes the campaign file in the original; the port does not delete
        // campaign scripts, so the button stays greyed out (docs/DIFFERENCES.md).
        if let Some(b) = self.screen.button_mut(button::DELETE_BUTTON) {
            b.disable(true);
        }
        self.pick(
            ctx.assets,
            if self.rows.is_empty() { None } else { Some(0) },
        );
    }

    /// Selects row `i` (or none): the list, the edit box and the mission info.
    fn pick(&mut self, assets: &UiAssets, i: Option<usize>) {
        self.selected = i;
        let colour = self.screen.edits.first().map_or(0, |e| e.text_color);
        let highlight = self.screen.edits.first().map_or(0, |e| e.highlight_color);
        self.list.remove_all_items();
        for (n, row) in self.rows.iter().enumerate() {
            let c = if Some(n) == i { highlight } else { colour };
            let item = list_row(
                assets,
                self.list_font,
                &row.name,
                c,
                false,
                self.list.obj.width(),
            );
            self.list.add_item(Box::new(item));
        }
        self.list.select_item(i.map_or(-1, |n| n as i32));
        if let (Some(n), Some(e)) = (i, self.screen.edits.get_mut(0)) {
            e.set_entry(assets, &self.rows[n].name, true);
        }
        self.update_mission_info(i);
    }

    /// `updateMissionInfo` / `setMission`.
    fn update_mission_info(&mut self, i: Option<usize>) {
        let info = i.and_then(|n| self.info.get(n));
        if let Some(s) = self.screen.statics.get_mut(MAP_STATIC) {
            match info.and_then(|m| m.map) {
                Some((tex, width)) => {
                    s.set_texture(Some(tex), width);
                    s.set_uvs(0.0, 127.0, 127.0, 0.0);
                    s.set_color(0xffff_ffff, true);
                }
                None => s.set_color(0, true),
            }
        }
        if let Some(t) = self.screen.texts.get_mut(2) {
            t.set_text(info.map_or("", |m| m.name.as_str()));
        }
    }

    /// The row the edit box names (`displayName.Compare(text, false)`), by the selected row first.
    fn row_named(&self) -> Option<usize> {
        let typed = self.screen.edits.first().map(|e| e.entry())?;
        if let Some(n) = self.selected {
            if self
                .rows
                .get(n)
                .is_some_and(|r| r.name.eq_ignore_ascii_case(&typed))
            {
                return Some(n);
            }
        }
        self.rows
            .iter()
            .position(|r| r.name.eq_ignore_ascii_case(&typed))
    }

    /// Returns the answer on the frame the dialog closes.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Status> {
        self.anim.update(ctx.dt);
        if !self.open {
            return None;
        }
        if self.needs_init {
            self.init(ctx);
        }
        if !self.anim.entered() {
            return None;
        }
        let mut msgs = self.screen.update(ctx);
        self.list.update(ctx);
        if ctx.input.is_left_click() {
            let (x, y) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            if self.list.point_inside(x, y) {
                if let Ok(n) = usize::try_from(self.list.selected_item()) {
                    if n < self.rows.len() {
                        self.pick(ctx.assets, Some(n));
                    }
                }
            } else if self
                .screen
                .edits
                .first()
                .is_some_and(|e| e.obj.point_inside(x, y))
            {
                self.selected = None;
                self.list.select_item(-1);
            }
        }
        // the Load button: on while the edit box names a campaign
        let named = self.row_named();
        if let Some(b) = self.screen.button_mut(button::YES) {
            b.disable(named.is_none());
        }
        let focused = self.screen.edits.iter().any(|e| e.has_focus());
        if !focused && ctx.input.key_status(KEY_RETURN) == KeyStatus::Released {
            let enabled = self
                .screen
                .button(button::YES)
                .is_none_or(|b| b.is_enabled());
            if enabled {
                msgs.push(Msg::new(0, button::YES));
            }
        }
        if ctx.input.key_status(KEY_ESCAPE) == KeyStatus::Released {
            msgs.push(Msg::new(0, button::NO));
        }
        for m in msgs {
            match m.who {
                button::YES => {
                    // `selectedName` is the hidden stem of the matching row
                    if let Some(n) = self.row_named() {
                        self.chosen = self.rows[n].stem.clone();
                        return Some(self.finish(Status::Yes));
                    }
                }
                button::NO => {
                    self.chosen.clear();
                    return Some(self.finish(Status::No));
                }
                _ => {}
            }
        }
        None
    }

    fn finish(&mut self, status: Status) -> Status {
        self.status = status;
        self.open = false;
        self.anim.end();
        status
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.open && !self.anim.leaving() {
            return;
        }
        let (colour, x, y) = self.anim.frame();
        let (w, h) = ctx.dl.screen;
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), colour);
        self.list.move_by(x, y);
        self.list.render(ctx);
        self.list.move_by(-x, -y);
        self.screen.render_offset(ctx, x as i32, y as i32);
    }
}

/// `setMission(fileName)`: the map picture (`getMissionTGA`) and the name from the mission's
/// `[MissionSettings]` (a resource string when `MissionNameUseResourceString`). An empty file
/// name shows neither.
fn mission_info(ld: &mut Loader<'_>, file: &str) -> MissionInfo {
    if file.is_empty() {
        return MissionInfo {
            map: None,
            name: String::new(),
        };
    }
    let map = crate::logistics::briefing::load_map(ld, file);
    let name = match mc2_sim_data::mission::Mission::load_named(
        ld.vfs,
        &format!("data/missions/{file}.fit"),
    ) {
        Ok(m) => {
            let s = &m.settings;
            if s.mission_name_use_resource_string.unwrap_or(false) {
                s.mission_name_resource_string_id
                    .map(|id| ld.assets.string(id).to_string())
                    .unwrap_or_default()
            } else {
                s.mission_name.clone().unwrap_or_default()
            }
        }
        Err(e) => {
            log::warn!("campaign list: mission {file}: {e}");
            String::new()
        }
    };
    MissionInfo { map, name }
}
