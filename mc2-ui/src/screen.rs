// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `LogisticsScreen` (`gui/logisticsscreen.cpp`): a screen built from a `.fit` layout.
//!
//! A layout lists its parts by kind, each with a count block: `Statics` (`staticCount`,
//! `Static0..`), `Rects` (`rectCount`), `Buttons` (`buttonCount`, animated buttons), `Texts`
//! (`TextEntryCount` or `TextCount`), `Edits` (`EditCount`) and `AnimObjects` (`Count`). The prefixes
//! are parameters, as in the original. Button clicks come back from [`LogisticsScreen::update`] as
//! messages for the owning screen's `handleMessage`.

use crate::color::interpolate_color;
use crate::ctx::{Loader, Msg, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::input::UiInput;
use crate::widgets::{AnimButton, AnimObject, Edit, Object, Rect, Text};
use mc2_formats::ini::FitIni;
use mc2_platform::input::InputEvent;

/// `LogisticsScreen::Status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Running = 0,
    Next = 1,
    Previous = 2,
    Done = 3,
    Paused = 4,
    Up,
    Down,
    Yes,
    No,
    MainMenu,
    Restart,
    MultiplayerRestart,
    SkipOneNext,
    SkipOnePrevious,
    FadeDown,
    FadeUp,
    ReadyToLoad,
    GotoSplash,
}

impl Status {
    /// The status value a button id stands for (dialog buttons use these ids).
    pub fn from_id(id: i32) -> Option<Status> {
        use Status::*;
        let all = [
            Running,
            Next,
            Previous,
            Done,
            Paused,
            Up,
            Down,
            Yes,
            No,
            MainMenu,
            Restart,
            MultiplayerRestart,
            SkipOneNext,
            SkipOnePrevious,
            FadeDown,
            FadeUp,
            ReadyToLoad,
            GotoSplash,
        ];
        all.get(usize::try_from(id).ok()?).copied()
    }
}

/// Block-name prefixes for [`LogisticsScreen::from_fit`] (`None` skips that kind).
#[derive(Clone, Copy, Debug)]
pub struct Prefixes<'a> {
    pub statics: Option<&'a str>,
    pub texts: Option<&'a str>,
    pub rects: Option<&'a str>,
    pub buttons: Option<&'a str>,
    pub edits: Option<&'a str>,
    pub anim_objects: Option<&'a str>,
}

impl Default for Prefixes<'_> {
    /// `"Static", "Text", "Rect", "Button", "Edit", "AnimObject"`.
    fn default() -> Self {
        Prefixes {
            statics: Some("Static"),
            texts: Some("Text"),
            rects: Some("Rect"),
            buttons: Some("Button"),
            edits: Some("Edit"),
            anim_objects: Some("AnimObject"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogisticsScreen {
    pub obj: Object,
    pub statics: Vec<Object>,
    pub rects: Vec<Rect>,
    pub texts: Vec<Text>,
    pub buttons: Vec<AnimButton>,
    pub edits: Vec<Edit>,
    pub anim_objects: Vec<AnimObject>,
    pub fade_in_time: f32,
    pub fade_out_time: f32,
    pub fade_time: f32,
    pub status: Status,
    pub fade_out_max_color: u32,
    /// Index of the text that shows the help string (`helpTextArrayID`), or -1.
    pub help_text_array_id: i32,
    /// Help id published during the last update (the original clears its global at the end).
    pub last_help_id: i32,
    flush_keys: bool,
    /// Mouse buttons (`MouseButton::mask_bit`) whose press the screen took.
    captured_buttons: u32,
    /// Mouse buttons whose press went to the game.
    game_buttons: u32,
}

impl Default for LogisticsScreen {
    fn default() -> Self {
        LogisticsScreen {
            obj: Object::default(),
            statics: Vec::new(),
            rects: Vec::new(),
            texts: Vec::new(),
            buttons: Vec::new(),
            edits: Vec::new(),
            anim_objects: Vec::new(),
            fade_in_time: 0.0,
            fade_out_time: 0.0,
            fade_time: 0.0,
            status: Status::Running,
            fade_out_max_color: 0xff00_0000,
            help_text_array_id: -1,
            last_help_id: 0,
            flush_keys: false,
            captured_buttons: 0,
            game_buttons: 0,
        }
    }
}

fn count(file: &mut FitIni, block: &str, keys: &[&str]) -> usize {
    if file.seek_block(block).is_err() {
        return 0;
    }
    keys.iter().find_map(|k| file.long(k)).unwrap_or(0).max(0) as usize
}

impl LogisticsScreen {
    /// `LogisticsScreen::init`.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, p: Prefixes<'_>) -> LogisticsScreen {
        let mut s = LogisticsScreen::default();
        if let Some(name) = p.statics {
            let n = count(file, &format!("{name}s"), &["staticCount"]);
            s.statics = (0..n)
                .map(|i| Object::from_fit(ld, file, &format!("{name}{i}")))
                .collect();
        }
        if let Some(name) = p.rects {
            let n = count(file, &format!("{name}s"), &["rectCount"]);
            s.rects = (0..n)
                .map(|i| Rect::from_fit(file, &format!("{name}{i}")))
                .collect();
        }
        if let Some(name) = p.buttons {
            let n = count(file, &format!("{name}s"), &["buttonCount"]);
            // addChild(&buttons[i]) moves each by the screen's position, which is (0, 0) here
            s.buttons = (0..n)
                .map(|i| AnimButton::from_fit(ld, file, &format!("{name}{i}")))
                .collect();
        }
        if let Some(name) = p.texts {
            let n = count(file, &format!("{name}s"), &["TextEntryCount", "TextCount"]);
            s.texts = (0..n)
                .map(|i| Text::from_fit(ld, file, &format!("{name}{i}")))
                .collect();
        }
        if let Some(name) = p.edits {
            let n = count(file, &format!("{name}s"), &["EditCount"]);
            s.edits = (0..n)
                .map(|i| Edit::from_fit(ld, file, &format!("{name}{i}")))
                .collect();
        }
        if let Some(name) = p.anim_objects {
            let n = count(file, &format!("{name}s"), &["Count"]);
            s.anim_objects = (0..n)
                .map(|i| AnimObject::from_fit(ld, file, &format!("{name}{i}")))
                .collect();
        }
        s
    }

    /// Loads `data/art/<name>.fit` with the given prefixes.
    pub fn load(
        ld: &mut Loader<'_>,
        name: &str,
        p: Prefixes<'_>,
    ) -> Result<LogisticsScreen, String> {
        let mut file = ld.art_fit(name)?;
        Ok(LogisticsScreen::from_fit(ld, &mut file, p))
    }

    /// `getButton(id)`: the button with that id.
    pub fn button(&self, id: i32) -> Option<&AnimButton> {
        self.buttons.iter().find(|b| b.id() == id)
    }

    pub fn button_mut(&mut self, id: i32) -> Option<&mut AnimButton> {
        self.buttons.iter_mut().find(|b| b.id() == id)
    }

    /// `getRect(i)`.
    pub fn rect(&self, i: usize) -> Option<&Rect> {
        self.rects.get(i)
    }

    /// Drops the pending key flush, for a screen whose owner reads keys before its first update.
    pub fn keep_keys(&mut self) {
        self.flush_keys = false;
    }

    /// `begin`: restarts the animated objects; the keyboard queue is flushed on the next update.
    pub fn begin(&mut self) {
        for a in &mut self.anim_objects {
            a.begin();
        }
        self.status = Status::Running;
        self.flush_keys = true;
    }

    pub fn begin_fade_in(&mut self, t: f32) {
        self.fade_in_time = t;
        self.fade_out_time = 0.0;
        self.fade_time = 0.0;
    }

    pub fn begin_fade_out(&mut self, t: f32) {
        self.fade_in_time = 0.0;
        self.fade_out_time = t;
        self.fade_time = 0.0;
    }

    /// `getStatus`: a finished screen reports `Running` until its fade-out has played.
    pub fn get_status(&self) -> Status {
        if self.status != Status::Running && self.fade_out_time != 0.0 {
            if self.fade_time > self.fade_out_time {
                return self.status;
            }
            return Status::Running;
        }
        self.status
    }

    /// `LogisticsScreen::update`. Returns the buttons' messages in button order.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Vec<Msg> {
        if self.flush_keys {
            ctx.input.flush_keys();
            self.flush_keys = false;
        }
        let mut msgs = Vec::new();
        for s in &self.statics {
            s.update(ctx);
        }
        for b in &mut self.buttons {
            if let Some(m) = b.update(ctx) {
                msgs.push(m);
            }
        }
        for t in &self.texts {
            t.update(ctx);
        }
        for r in &self.rects {
            r.update(ctx);
        }
        self.apply_help_text(ctx.assets, ctx.out.help_text_id);
        for e in &mut self.edits {
            e.update(ctx);
        }
        for a in &mut self.anim_objects {
            a.update(ctx.dt);
        }
        self.last_help_id = ctx.out.help_text_id;
        ctx.out.help_text_id = 0;
        msgs
    }

    /// Shows the string `help_id` (or nothing) in the help text.
    pub fn apply_help_text(&mut self, assets: &UiAssets, help_id: i32) {
        if self.help_text_array_id < 0 {
            return;
        }
        if let Some(t) = self.texts.get_mut(self.help_text_array_id as usize) {
            if help_id != 0 {
                t.set_text(assets.string(help_id as u32));
            } else {
                t.set_text("");
            }
        }
    }

    fn draw_fade(&mut self, ctx: &mut RenderCtx<'_>, max: u32) {
        let (w, h) = ctx.screen();
        if self.fade_out_time != 0.0 {
            self.fade_time += ctx.dt;
            let c = interpolate_color(0, max, self.fade_time / self.fade_out_time);
            ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), c);
        } else if self.fade_in_time != 0.0 && self.fade_in_time > self.fade_time {
            self.fade_time += ctx.dt;
            let c = interpolate_color(max, 0, self.fade_time / self.fade_in_time);
            ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), c);
        }
    }

    /// `LogisticsScreen::render`: opaque filled rects, statics, outlines, translucent rects,
    /// buttons, texts, edits, animated objects, then the fade.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        for r in &self.rects {
            if !r.outline && r.color() & 0xff00_0000 == 0xff00_0000 {
                r.render(ctx);
            }
        }
        for s in &self.statics {
            s.render(ctx);
        }
        for r in &self.rects {
            if r.outline {
                r.render(ctx);
            }
        }
        for r in &self.rects {
            if r.color() & 0xff00_0000 != 0xff00_0000 {
                r.render(ctx);
            }
        }
        for b in &mut self.buttons {
            b.render(ctx);
        }
        for t in &self.texts {
            t.render(ctx);
        }
        for e in &self.edits {
            e.render(ctx);
        }
        for a in &mut self.anim_objects {
            a.render(ctx);
        }
        let max = self.fade_out_max_color;
        self.draw_fade(ctx, max);
    }

    /// `LogisticsScreen::render(x, y)`: everything drawn shifted (the fade uses opaque black).
    pub fn render_offset(&mut self, ctx: &mut RenderCtx<'_>, x: i32, y: i32) {
        if !self.obj.show {
            return;
        }
        let (fx, fy) = (x as f32, y as f32);
        for r in &self.rects {
            if !r.outline && r.color() & 0xff00_0000 == 0xff00_0000 && r.obj.show {
                r.render_at(ctx, x, y);
            }
        }
        for s in &mut self.statics {
            s.render_at(ctx, fx, fy);
        }
        for r in &self.rects {
            if r.outline && r.obj.show {
                r.render_at(ctx, x, y);
            }
        }
        for r in &self.rects {
            if r.color() & 0xff00_0000 != 0xff00_0000 && r.obj.show {
                r.render_at(ctx, x, y);
            }
        }
        for b in &mut self.buttons {
            b.move_by(fx, fy);
            b.render(ctx);
            b.move_by(-fx, -fy);
        }
        for t in &mut self.texts {
            t.render_at(ctx, fx, fy);
        }
        for e in &mut self.edits {
            e.move_by(fx, fy);
            e.render(ctx);
            e.move_by(-fx, -fy);
        }
        for a in &mut self.anim_objects {
            a.obj.move_by(fx, fy);
            a.render(ctx);
            a.obj.move_by(-fx, -fy);
        }
        self.draw_fade(ctx, 0xff00_0000);
    }

    /// `LogisticsScreen::move`.
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        for r in &mut self.rects {
            r.obj.move_by(dx, dy);
        }
        for s in &mut self.statics {
            s.move_by(dx, dy);
        }
        for b in &mut self.buttons {
            b.move_by(dx, dy);
        }
        for t in &mut self.texts {
            t.obj.move_by(dx, dy);
        }
        for e in &mut self.edits {
            e.move_by(dx, dy);
        }
        for a in &mut self.anim_objects {
            a.obj.move_by(dx, dy);
        }
    }

    /// `LogisticsScreen::moveTo`: the screen becomes an 800x600 area at `(x, y)`.
    pub fn move_to(&mut self, x: f32, y: f32) {
        let dx = x - self.obj.loc[0].x;
        let dy = y - self.obj.loc[0].y;
        self.obj = Object::new(x, y, 800.0, 600.0);
        self.move_by(dx, dy);
    }

    /// `inside`: is the point over any static, button, text, rect or animated object.
    pub fn inside(&self, assets: &UiAssets, x: i32, y: i32) -> bool {
        self.statics.iter().any(|s| s.point_inside(x, y))
            || self.buttons.iter().any(|b| b.point_inside(x, y))
            || self.texts.iter().any(|t| t.point_inside(assets, x, y))
            || self.rects.iter().any(|r| r.obj.point_inside(x, y))
            || self.anim_objects.iter().any(|a| a.obj.point_inside(x, y))
    }

    /// Routes one event for a non-modal screen (the HUD): asks [`LogisticsScreen::consumes`] and
    /// feeds `input` accordingly ([`UiInput::handle_routed`]). Returns whether the UI took it; the
    /// app passes the event on when it did not.
    pub fn route(&mut self, input: &mut UiInput, assets: &UiAssets, ev: &InputEvent) -> bool {
        let pointer = match ev {
            InputEvent::CursorMoved { x, y } => (x.round() as i32, y.round() as i32),
            _ => input.pending_pointer(),
        };
        let taken = self.consumes(assets, ev, pointer);
        if !taken && matches!(ev, InputEvent::Button { pressed: true, .. }) {
            // the widgets never see this click, but an edit field loses its focus on any click
            // outside it (`aEdit::handleMouse`)
            let mut dropped = false;
            for e in &mut self.edits {
                dropped |= e.focus;
                e.focus = false;
            }
            if dropped {
                input.flush_keys();
            }
        }
        input.handle_routed(ev, taken);
        taken
    }

    /// The parts that are drawn: shown statics, rects, texts, animated objects and edit fields,
    /// and shown buttons that are not hidden.
    fn over_visible(&self, assets: &UiAssets, x: i32, y: i32) -> bool {
        if !self.obj.show {
            return false;
        }
        self.statics.iter().any(|s| s.show && s.point_inside(x, y))
            || self.buttons.iter().any(|b| {
                b.obj.show && b.state != crate::widgets::ButtonState::Hidden && b.point_inside(x, y)
            })
            || self
                .texts
                .iter()
                .any(|t| t.obj.show && t.point_inside(assets, x, y))
            || self
                .rects
                .iter()
                .any(|r| r.obj.show && r.obj.point_inside(x, y))
            || self
                .anim_objects
                .iter()
                .any(|a| a.obj.show && a.obj.point_inside(x, y))
            || self
                .edits
                .iter()
                .any(|e| e.obj.show && e.obj.point_inside(x, y))
    }

    /// Whether an event is the screen's (the plan's input routing: the UI sees every event first
    /// and the app only acts on the ones it did not take). Taken are: pointer events over one of
    /// the screen's visible parts (edit fields included); the release of a mouse button exactly
    /// when its press was taken, so a gesture (press, moves, release, further
    /// buttons) stays with whoever it started on (tracked per button);
    /// and keys while an edit field has focus. A hidden screen takes nothing. Modal screens (the
    /// menus) take every event instead.
    pub fn consumes(&mut self, assets: &UiAssets, ev: &InputEvent, pointer: (i32, i32)) -> bool {
        if !self.obj.show {
            // a screen hidden mid-gesture lets go of the pointer
            self.captured_buttons = 0;
            self.game_buttons = 0;
            return false;
        }
        match ev {
            InputEvent::Key { .. } | InputEvent::Text(_) => {
                self.obj.show && self.edits.iter().any(|e| e.focus && e.obj.show)
            }
            InputEvent::CursorMoved { x, y } => {
                if self.captured_buttons != 0 {
                    true
                } else if self.game_buttons != 0 {
                    // a drag the game owns stays with the game
                    false
                } else {
                    self.over_visible(assets, *x as i32, *y as i32)
                }
            }
            InputEvent::Button {
                button,
                pressed: true,
            } => {
                let hit = self.captured_buttons != 0
                    || (self.game_buttons == 0 && self.over_visible(assets, pointer.0, pointer.1));
                if hit {
                    self.captured_buttons |= button.mask_bit();
                } else {
                    self.game_buttons |= button.mask_bit();
                }
                hit
            }
            InputEvent::Button {
                button,
                pressed: false,
            } => {
                let bit = button.mask_bit();
                // a release belongs to whoever took the press
                let captured = self.captured_buttons & bit != 0;
                self.captured_buttons &= !bit;
                self.game_buttons &= !bit;
                captured
            }
            InputEvent::Wheel { .. } => self.over_visible(assets, pointer.0, pointer.1),
            InputEvent::FocusLost => {
                self.captured_buttons = 0;
                self.game_buttons = 0;
                false
            }
            _ => false,
        }
    }
}
