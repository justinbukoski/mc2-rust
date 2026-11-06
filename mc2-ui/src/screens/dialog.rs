// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `LogisticsDialog`, `LogisticsOKDialog`, `LogisticsOneButtonDialog` and `LogisticsLegalDialog`
//! (`code/logisticsdialog.cpp`): a modal box that slides in, dims the screen behind it and closes
//! with the id of the button pressed (Return = yes, Escape or a click outside = no).
//!
//! The original keeps one global instance of each kind (`mcl_dialog.fit`,
//! `mcl_dialog_onebutton.fit`, `mcl_dialoglegal.fit`); here each owner holds its own.

use crate::anim::Animation;
use crate::color::interpolate_color;
use crate::ctx::{msg, sfx, Loader, RenderCtx, UpdateCtx};
use crate::draw::GuiRect;
use crate::font::FontRef;
use crate::screen::{LogisticsScreen, Prefixes, Status};
use mc2_platform::input::KeyStatus;
use mc2_platform::keys;

pub const YES: i32 = Status::Yes as i32;
pub const NO: i32 = Status::No as i32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// Two buttons (`mcl_dialog.fit`): cancel (`NO`) and okay (`YES`).
    Ok,
    /// One button (`mcl_dialog_onebutton.fit`).
    OneButton,
    /// The legal notice (`mcl_dialoglegal.fit`).
    Legal,
}

#[derive(Clone, Debug)]
pub struct Dialog {
    pub screen: LogisticsScreen,
    pub kind: DialogKind,
    pub enter: Animation,
    pub exit: Animation,
    done: bool,
    /// The id of the button that closed the dialog (`status`).
    pub result: i32,
    old_font: Option<FontRef>,
    pending_sound: bool,
}

impl Dialog {
    /// `LogisticsOKDialog::init` and friends.
    pub fn load(ld: &mut Loader<'_>, kind: DialogKind) -> Result<Dialog, String> {
        let name = match kind {
            DialogKind::Ok => "mcl_dialog",
            DialogKind::OneButton => "mcl_dialog_onebutton",
            DialogKind::Legal => "mcl_dialoglegal",
        };
        let mut file = ld.art_fit(name)?;
        let mut screen = LogisticsScreen::from_fit(ld, &mut file, Prefixes::default());
        let enter = Animation::from_block(&mut file, "EnterAnim");
        let exit = Animation::from_block(&mut file, "LeaveAnim");
        for b in &mut screen.buttons {
            b.set_message_on_release();
        }
        Ok(Dialog {
            screen,
            kind,
            enter,
            exit,
            done: false,
            result: Status::Running as i32,
            old_font: None,
            pending_sound: false,
        })
    }

    /// `LogisticsDialog::begin`.
    pub fn begin(&mut self) {
        self.enter.begin();
        self.exit.end();
        self.result = Status::Running as i32;
        self.screen.status = Status::Running;
        self.done = false;
        self.pending_sound = true;
    }

    /// `LogisticsDialog::end`: slides out and restores the text font.
    pub fn end(&mut self) {
        self.enter.end();
        self.exit.begin();
        if let Some(f) = self.old_font.take() {
            if let Some(t) = self.screen.texts.first_mut() {
                t.font = f;
            }
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// `setText(textID, cancel, ok)`: the message and the button labels.
    pub fn set_text_ids(&mut self, ld_assets: &crate::UiAssets, text: i32, cancel: i32, ok: i32) {
        if let Some(t) = self.screen.texts.first_mut() {
            t.set_text_id(ld_assets, text);
        }
        if let Some(b) = self.screen.buttons.first_mut() {
            b.set_text(cancel);
        }
        if self.kind == DialogKind::Ok {
            if let Some(b) = self.screen.buttons.get_mut(1) {
                b.set_text(ok);
            }
        }
    }

    /// `setText(mainText)`.
    pub fn set_text(&mut self, s: &str) {
        if let Some(t) = self.screen.texts.first_mut() {
            t.set_text(s);
        }
    }

    /// `setFont(resID)`: until `end`.
    pub fn set_font(&mut self, font: FontRef) {
        if let Some(t) = self.screen.texts.first_mut() {
            self.old_font.get_or_insert(t.font);
            t.font = font;
        }
    }

    /// `handleMessage(_, who)`: closes with `who`.
    fn close(&mut self, who: i32) {
        self.result = who;
        self.exit.begin();
        self.enter.end();
    }

    /// `LogisticsDialog::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        if self.pending_sound {
            ctx.play(sfx::LOG_MAINMENUBUTTON);
            ctx.input.flush_keys();
            self.pending_sound = false;
        }
        self.enter.update(ctx.dt);
        self.exit.update(ctx.dt);
        if self.exit.is_done() {
            self.done = true;
        }
        ctx.input.draw_mouse = true;
        ctx.input.mouse_state = crate::cursor::M_STATE_NORMAL;
        let focus = self.screen.edits.iter().any(|e| e.focus);
        if self.enter.is_done() {
            for m in self.screen.update(ctx) {
                if m.msg == msg::LEFTMOUSEDOWN {
                    self.close(m.who);
                }
            }
        }
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        if ctx.input.left_mouse_released()
            && self.enter.is_done()
            && !self.screen.inside(ctx.assets, mx, my)
            && !self.screen.inside(ctx.assets, dx, dy)
        {
            self.close(NO);
        }
        if !focus
            && self.enter.is_done()
            && ctx.input.key_status(keys::KEY_RETURN) == KeyStatus::Released
        {
            match self.screen.button(YES) {
                Some(b) if !b.is_enabled() => {}
                _ => self.close(YES),
            }
        }
        if ctx.input.key_status(keys::KEY_ESCAPE) == KeyStatus::Released {
            if let Some(b) = self.screen.button(NO) {
                if b.is_enabled() {
                    self.close(NO);
                }
            }
        }
    }

    /// `LogisticsDialog::render`: the dimmed screen, then the dialog offset by its slide.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let mut color = 0x7f00_0000;
        if self.enter.is_animating() && !self.enter.is_done() {
            let end = self.enter.max_time();
            if end != 0.0 {
                color = interpolate_color(0, 0x7f00_0000, self.enter.current_time / end);
            }
        } else if self.exit.is_animating() && !self.exit.is_done() {
            let end = self.exit.max_time();
            if end != 0.0 {
                color = interpolate_color(0x7f00_0000, 0, self.exit.current_time / end);
            }
        }
        let (w, h) = ctx.screen();
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), color);
        let (mut x, mut y) = (0.0, 0.0);
        if self.enter.is_animating() {
            x = self.enter.x_delta();
            y = self.enter.y_delta();
        } else if self.exit.is_animating() {
            x = self.exit.x_delta();
            y = self.exit.y_delta();
        }
        self.screen.render_offset(ctx, x as i32, y as i32);
    }
}
