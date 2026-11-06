// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The main menu (`code/mainmenu.cpp`: `MainMenu`, `SplashIntro`), over the animated splash
//! background (`mcl_sp.fit`) and the intro fade (`mcl_splashscreenintro.fit`).
//!
//! What a button leads to comes back from [`MainMenu::update`] as a [`MainMenuAction`] for the app
//! to act on (start a campaign, load a game, show the options...), where the original called into
//! `LogisticsData`, `LogisticsSaveDialog`, `MPLoadMap`, `Mechlopedia` and the options screen
//! itself. Like the original, the actions that leave the menu wait for its slide-out animation;
//! the options and the encyclopedia open at once. Quitting asks first (the OK dialog) and the
//! legal notice is shown here.
//!
//! Not here: the Microsoft logo movie (`msft.bik`, played by the app before the menu), multiplayer
//! disconnect prompts and the "host left" dialog (no multiplayer yet).

use super::dialog::{Dialog, DialogKind, YES};
use crate::anim::Animation;
use crate::color::interpolate_color;
use crate::ctx::{msg, sfx, Loader, RenderCtx, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::font::text::{self, TextAttrs};
use crate::screen::{LogisticsScreen, Prefixes};
use mc2_platform::keys;

pub const MM_MSG_NEW_CAMPAIGN: i32 = 90;
pub const MM_MSG_LOAD: i32 = 91;
pub const MM_MSG_SAVE: i32 = 92;
pub const MM_MSG_MULTIPLAYER: i32 = 93;
pub const MM_MSG_RETURN_TO_GAME: i32 = 94;
pub const MM_MSG_OPTIONS: i32 = 95;
pub const MM_MSG_ENCYCLOPEDIA: i32 = 96;
pub const MM_MSG_EXIT: i32 = 97;
pub const MM_MSG_SINGLE_MISSION: i32 = 98;
pub const MM_MSG_LEGAL: i32 = 99;

/// `IDS_DIALOG_QUIT_PROMPT`, `IDS_DIALOG_NO`, `IDS_DIALOG_YES`, `IDS_DIALOG_OK`,
/// `IDS_LAWYER_BABBLE`, `IDS_LAWYER_BABBLE_FONT`.
const IDS_DIALOG_QUIT_PROMPT: i32 = 27654;
const IDS_DIALOG_NO: i32 = 27653;
const IDS_DIALOG_YES: i32 = 27652;
const IDS_DIALOG_OK: i32 = 27650;
const IDS_LAWYER_BABBLE: u32 = 60016;
const IDS_LAWYER_BABBLE_FONT: u32 = 60018;

/// What the player chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainMenuAction {
    /// "New campaign" (the original then opens the campaign choice and restarts logistics).
    NewCampaign,
    /// "Load game" (the save game dialog).
    LoadGame,
    /// "Save game" (only from a running campaign).
    SaveGame,
    /// "Multiplayer" (disabled, as in the shipped game's menu update).
    Multiplayer,
    /// "Return to game" (only when the menu was opened over a game).
    ReturnToGame,
    /// "Options": show the options screen.
    Options,
    /// "Encyclopedia": show the Mechlopedia.
    Encyclopedia,
    /// "Solo mission" (the single mission list).
    SingleMission,
    /// "Exit", confirmed.
    Quit,
}

/// One frame's result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MainMenuFrame {
    pub action: Option<MainMenuAction>,
    /// Start this music tune (`playDigitalMusic(tuneId)`), once when the menu starts.
    pub music: Option<i32>,
}

pub struct MainMenu {
    pub screen: LogisticsScreen,
    pub background: LogisticsScreen,
    pub intro: LogisticsScreen,
    pub begin_anim: Animation,
    pub end_anim: Animation,
    /// Drawn over the splash background (at start-up) rather than over a game.
    pub draw_background: bool,
    pub intro_over: bool,
    /// Single mission or no campaign running: "save" is disabled.
    pub can_save: bool,
    pub tune_id: i32,
    music_started: bool,
    prompt_to_quit: bool,
    legal_shown: bool,
    pending: Option<MainMenuAction>,
    ok_dialog: Dialog,
    legal_dialog: Dialog,
    legal_font: crate::font::FontRef,
}

impl MainMenu {
    /// `MainMenu::init` (`mcl_mm.fit`, `mcl_sp.fit`, `SplashIntro::init`) plus the dialogs it uses.
    pub fn load(ld: &mut Loader<'_>) -> Result<MainMenu, String> {
        let mut file = ld.art_fit("mcl_mm")?;
        let tune_id = if file.seek_block("Tunes").is_ok() {
            file.long_or("TuneId", -1) as i32
        } else {
            -1
        };
        let p = Prefixes {
            edits: None,
            anim_objects: None,
            ..Prefixes::default()
        };
        let mut screen = LogisticsScreen::from_fit(ld, &mut file, p);
        screen.help_text_array_id = 0;
        for b in &mut screen.buttons {
            b.set_message_on_release();
            b.set_press_fx(sfx::LOG_VIDEOBUTTONS);
            b.set_highlight_fx(sfx::LOG_DIGITALHIGHLIGHT);
        }
        let begin_anim = Animation::from_block(&mut file, "InAnim");
        let end_anim = Animation::from_block(&mut file, "OutAnim");
        // the original passes the four-name init, which still loads "AnimObject"s by default
        let background = LogisticsScreen::load(ld, "mcl_sp", Prefixes::default())?;
        let intro = LogisticsScreen::load(ld, "mcl_splashscreenintro", Prefixes::default())?;
        let ok_dialog = Dialog::load(ld, DialogKind::Ok)?;
        let legal_dialog = Dialog::load(ld, DialogKind::Legal)?;
        let legal_font = ld.font(IDS_LAWYER_BABBLE_FONT);
        let mut m = MainMenu {
            screen,
            background,
            intro,
            begin_anim,
            end_anim,
            draw_background: false,
            intro_over: false,
            can_save: false,
            tune_id,
            music_started: false,
            prompt_to_quit: false,
            legal_shown: false,
            pending: None,
            ok_dialog,
            legal_dialog,
            legal_font,
        };
        m.set_draw_background(true);
        m.begin();
        Ok(m)
    }

    /// `MainMenu::begin`.
    pub fn begin(&mut self) {
        self.screen.status = crate::screen::Status::Running;
        self.prompt_to_quit = false;
        self.begin_anim.begin();
        self.screen.begin_fade_in(0.0);
        self.end_anim.end();
        self.background.begin_fade_in(0.0);
        self.pending = None;
        self.music_started = false;
        self.legal_shown = false;
    }

    /// `setDrawBackground`: the splash intro plays first when the menu is shown at start-up.
    pub fn set_draw_background(&mut self, draw: bool) {
        self.draw_background = draw;
        if draw && !self.intro_over {
            self.intro.begin();
        }
    }

    /// Skips the intro fade (the original's Escape during the intro).
    pub fn skip_intro(&mut self) {
        self.intro_over = true;
        self.finish_intro();
    }

    /// Runs every intro animation to its last key, so the black cover is gone and the plate
    /// behind the menu shows at once (the original lets the 12 s cover play out after Escape).
    fn finish_intro(&mut self) {
        for a in &mut self.intro.anim_objects {
            a.anim.current_time = a.anim.max_time() + 1.0;
        }
    }

    fn intro_done(&self) -> bool {
        self.intro.anim_objects.first().is_none_or(|a| a.is_done())
    }

    /// Whether an event belongs to the menu: it is modal and takes every event.
    pub fn consumes(&self) -> bool {
        true
    }

    fn leave(&mut self, a: MainMenuAction) {
        self.end_anim.begin();
        self.begin_anim.end();
        self.pending = Some(a);
    }

    /// `MainMenu::handleMessage`.
    fn handle(&mut self, ctx: &mut UpdateCtx<'_>, who: i32) -> Option<MainMenuAction> {
        match who {
            MM_MSG_NEW_CAMPAIGN => self.leave(MainMenuAction::NewCampaign),
            MM_MSG_SAVE => self.leave(MainMenuAction::SaveGame),
            MM_MSG_LOAD => self.leave(MainMenuAction::LoadGame),
            MM_MSG_MULTIPLAYER => self.leave(MainMenuAction::Multiplayer),
            MM_MSG_RETURN_TO_GAME => {
                if !self.draw_background {
                    self.leave(MainMenuAction::ReturnToGame);
                    ctx.play(sfx::LOG_MAINMENUBUTTON);
                }
            }
            MM_MSG_OPTIONS => return Some(MainMenuAction::Options),
            MM_MSG_ENCYCLOPEDIA => {
                self.screen.begin_fade_out(1.0);
                return Some(MainMenuAction::Encyclopedia);
            }
            MM_MSG_EXIT => {
                self.prompt_to_quit = true;
                self.ok_dialog.set_text_ids(
                    ctx.assets,
                    IDS_DIALOG_QUIT_PROMPT,
                    IDS_DIALOG_NO,
                    IDS_DIALOG_YES,
                );
                self.ok_dialog.begin();
                if let Some(b) = self.screen.button_mut(who) {
                    b.press(false);
                }
            }
            MM_MSG_SINGLE_MISSION => {
                self.leave(MainMenuAction::SingleMission);
                if let Some(b) = self.screen.button_mut(who) {
                    b.press(false);
                }
            }
            MM_MSG_LEGAL => {
                self.legal_shown = true;
                self.legal_dialog.set_text_ids(
                    ctx.assets,
                    IDS_DIALOG_OK,
                    IDS_DIALOG_OK,
                    IDS_DIALOG_OK,
                );
                // the product id comes from the registry; without one the original prints
                // "INVALID ID"
                let text = ctx
                    .assets
                    .string(IDS_LAWYER_BABBLE)
                    .replacen("%s", "INVALID ID", 1);
                self.legal_dialog.set_text(&text);
                self.legal_dialog.begin();
                self.legal_dialog.set_font(self.legal_font);
                if let Some(b) = self.screen.button_mut(who) {
                    b.press(false);
                }
            }
            _ => {}
        }
        None
    }

    /// Re-shows the menu after the app finished an action that left it (a cancelled load dialog,
    /// the options screen...): the menu slides back in.
    pub fn resume(&mut self) {
        self.screen.begin_fade_in(0.0);
        self.begin_anim.begin();
        self.end_anim.end();
        self.pending = None;
    }

    /// `MainMenu::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> MainMenuFrame {
        let mut frame = MainMenuFrame::default();
        let disable_save = self.draw_background || !self.can_save;
        if let Some(b) = self.screen.button_mut(MM_MSG_SAVE) {
            b.disable(disable_save);
        }
        if let Some(b) = self.screen.button_mut(MM_MSG_MULTIPLAYER) {
            b.disable(true);
        }
        if !self.music_started {
            self.music_started = true;
            if self.tune_id >= 0 {
                frame.music = Some(self.tune_id);
            }
        }
        if self.end_anim.is_done() {
            if let Some(a) = self.pending.take() {
                frame.action = Some(a);
                return frame;
            }
        }
        if self.pending.is_some() {
            // sliding out
            self.end_anim.update(ctx.dt);
            self.begin_anim.update(ctx.dt);
            return frame;
        }
        if self.prompt_to_quit {
            self.ok_dialog.update(ctx);
            if self.ok_dialog.result == YES {
                ctx.play(sfx::LOG_EXITGAME);
                self.prompt_to_quit = false;
                frame.action = Some(MainMenuAction::Quit);
            } else if self.ok_dialog.result == super::dialog::NO && self.ok_dialog.is_done() {
                self.prompt_to_quit = false;
            }
            return frame;
        }
        if self.legal_shown {
            self.legal_dialog.update(ctx);
            if self.legal_dialog.is_done() {
                self.legal_dialog.end();
                self.legal_shown = false;
            }
            return frame;
        }
        if self.draw_background {
            if !self.intro_done() {
                self.intro.update(ctx);
                self.background.update(ctx);
                if ctx.input.key_down(keys::KEY_ESCAPE) {
                    self.intro_over = true;
                    self.finish_intro();
                    ctx.input.draw_mouse = true;
                    ctx.play(sfx::LOG_MAINMENUBUTTON);
                } else if !self.intro_over {
                    return frame;
                }
            } else {
                self.background.update(ctx);
                if !self.intro_over {
                    ctx.play(sfx::LOG_MAINMENUBUTTON);
                }
                self.intro_over = true;
                ctx.input.draw_mouse = true;
            }
        }
        self.begin_anim.update(ctx.dt);
        self.end_anim.update(ctx.dt);
        let msgs = self.screen.update(ctx);
        for m in msgs {
            if m.msg == msg::LEFTMOUSEDOWN {
                if let Some(a) = self.handle(ctx, m.who) {
                    frame.action = Some(a);
                }
            }
        }
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if ctx.input.is_left_click() && !self.screen.inside(ctx.assets, mx, my) {
            if let Some(a) = self.handle(ctx, MM_MSG_RETURN_TO_GAME) {
                frame.action = Some(a);
            }
        }
        frame
    }

    /// The Earth / starfield plate (`mcl_sp.fit`) on its own, for the dialogs the menu opens
    /// (the original draws the Load / Save dialog over it).
    pub fn render_plate(&mut self, ctx: &mut RenderCtx<'_>) {
        let (w, h) = ctx.screen();
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), 0xff00_0000);
        // also in a campaign, where the menu itself skips the plate: Save is only reachable there
        self.background.render(ctx);
    }

    /// `MainMenu::render`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let (mut xd, mut yd) = (0.0f32, 0.0f32);
        let mut color = 0xff00_0000;
        if self.begin_anim.is_animating() && !self.begin_anim.is_done() {
            xd = self.begin_anim.x_delta();
            yd = self.begin_anim.y_delta();
            let end = self.begin_anim.max_time();
            if end != 0.0 {
                color = interpolate_color(0, 0x7f00_0000, self.begin_anim.current_time / end);
            }
        } else if self.end_anim.is_animating() {
            xd = self.end_anim.x_delta();
            yd = self.end_anim.y_delta();
            let (t, end) = (self.end_anim.current_time, self.end_anim.max_time());
            if end != 0.0 && t <= end {
                color = interpolate_color(0x7f00_0000, 0, t / end);
            }
        }
        let (w, h) = ctx.screen();
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), color);
        if self.draw_background {
            self.background.render(ctx);
            self.intro.render(ctx);
            if !self.intro_done() && !self.intro_over {
                return;
            }
        }
        if xd == 0.0 && yd == 0.0 {
            if let Some(t) = self.screen.texts.get(1) {
                if let Some(f) = ctx.assets.font(t.font) {
                    // drawShadowText(0xffc66600, black, ..., centred, offset 1, 1)
                    let o = &t.obj;
                    let r = GuiRect::new(
                        o.global_x(),
                        o.global_y(),
                        o.global_right(),
                        o.global_bottom(),
                    );
                    let mut a = TextAttrs {
                        color: 0xff00_0000,
                        size: t.font.size as f32,
                        align: 2,
                        ..TextAttrs::default()
                    };
                    let shadow = GuiRect::new(r.left + 1, r.top + 1, r.right + 1, r.bottom + 1);
                    text::draw(ctx.dl, f, &t.text, &a, shadow, (shadow.left, shadow.top));
                    a.color = 0xffc6_6600;
                    text::draw(ctx.dl, f, &t.text, &a, r, (r.left, r.top));
                }
            }
        }
        if let Some(t) = self.screen.texts.get_mut(1) {
            t.obj.show = false;
        }
        if self.pending.is_none() || !self.end_anim.is_done() {
            self.screen.render_offset(ctx, xd as i32, yd as i32);
        }
        if self.prompt_to_quit {
            self.ok_dialog.render(ctx);
        }
        if self.legal_shown {
            self.legal_dialog.render(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::Loader;

    #[test]
    fn skipping_the_intro_runs_its_black_cover_out_so_the_plate_shows() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut menu = MainMenu::load(&mut Loader::new(&mut vfs, &mut assets)).unwrap();
        assert!(!menu.intro_done(), "the 12 s intro is playing");
        menu.skip_intro();
        assert!(menu.intro_done());
        for a in &menu.intro.anim_objects {
            assert!(a.anim.color() & 0xff00_0000 != 0xff00_0000 || a.anim.max_time() < 0.0);
        }
    }

    /// Save is only reachable in a campaign, where the menu does not draw its own plate; the
    /// dialog still gets the Earth / starfield behind it, at start-up and in a campaign alike.
    #[test]
    fn the_plate_behind_the_save_load_dialog_is_drawn_in_a_campaign_too() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut menu = MainMenu::load(&mut Loader::new(&mut vfs, &mut assets)).unwrap();
        let mut counts = Vec::new();
        for background in [true, false] {
            menu.set_draw_background(background);
            let mut dl = crate::draw::DrawList::new(800, 600);
            let mut ctx = RenderCtx {
                dl: &mut dl,
                assets: &assets,
                dt: 0.0,
            };
            menu.render_plate(&mut ctx);
            counts.push(dl.vertices.len());
        }
        assert!(counts[0] > 4, "the plate is more than the black fill");
        assert_eq!(counts[0], counts[1]);
    }
}
