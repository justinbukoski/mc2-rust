// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The options screen (`code/optionsscreenwrapper.cpp`, `code/optionsarea.cpp`): `OptionsXScreen`
//! with its four tab areas (`OptionsGraphics`, `OptionsAudio`, `OptionsGamePlay`,
//! `OptionsHotKeys`), the horizontal volume sliders (`ScrollX`) and the hot-key list items
//! (`HotKeyListItem`).
//!
//! The screen edits a [`Prefs`] and returns it: [`OptionsScreen::update`] reports
//! [`OptionsResult::Accepted`] (with the new prefs and whether the resolution changed) or
//! [`OptionsResult::Cancelled`]; saving `options.cfg` and applying the settings is the app's job
//! (the original's `prefs.save()` / `applyPrefs`). While the audio tab is open the volumes are
//! reported every frame so the app can let the player hear them, as the original did.
//!
//! The gameplay tab spins a Bushwacker in the player's base and highlight colours in `Rect1`
//! (`OptionsGamePlay::camera`, a `SimpleCamera`; not drawn while a mission is running), from the
//! pictures [`crate::preview::ModelPreviews`] renders.
//!
//! Not here: the video card list beyond the names the app supplies.

use super::dialog::{Dialog, DialogKind, NO, YES};
use super::prefs::Prefs;
use crate::anim::AnimGroup;
use crate::ctx::{msg, sfx, Loader, Msg, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::hotkeys::{self, COMMANDS};
use crate::preview::{rect_size, PreviewKind, PreviewRequest, Spinner};
use crate::screen::{LogisticsScreen, Prefixes};
use crate::widgets::listbox::{ItemState, ListItem};
use crate::widgets::{AnimButton, DropList, ListBox, Object, Rect, Text};
use mc2_platform::input::KeyStatus;
use mc2_platform::keys;
use std::any::Any;

pub const MSB_TAB0: i32 = 200;
pub const MSB_TAB3: i32 = 203;
pub const MSG_TERRAIN_DETAIL: i32 = 301;
pub const MSG_PILOT_VIDS: i32 = 302;
pub const MSG_OBJECT_DETAIL: i32 = 303;
pub const MSG_SHADOWS: i32 = 304;
pub const MSG_NON_WEAPON: i32 = 305;
pub const MSG_LOCAL_SHADOWS: i32 = 306;
pub const MSG_ASYNC_MOUSE: i32 = 307;
pub const MSG_HARDWARE_RASTERIZER: i32 = 308;
pub const MSG_RESET: i32 = 309;
pub const MSG_GREEN: i32 = 400;
pub const MSG_ELITE: i32 = 403;
pub const MSG_BASE: i32 = 404;
pub const MSG_ACCENT: i32 = 405;
pub const MSG_UNLIMITED_AMMO: i32 = 406;
pub const MSG_LEFT_CLICK: i32 = 407;
/// `IDS_SWAP_RESOLUTION_WARNING`, `IDS_OPTIONS_HOTKEY_ERROR`, `IDS_DIALOG_OK/NO/YES`.
const IDS_SWAP_RESOLUTION_WARNING: i32 = 60027;
const IDS_OPTIONS_HOTKEY_ERROR: i32 = 26592;
const IDS_DIALOG_OK: i32 = 27650;
const IDS_DIALOG_NO: i32 = 27653;
const IDS_DIALOG_YES: i32 = 27652;

/// A display mode the graphics tab offers (`gos_GetDisplayModeByIndex`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: i32,
    pub height: i32,
    pub bit_depth: i32,
}

/// What the app tells the screen about the machine and the game state.
#[derive(Clone, Debug)]
pub struct OptionsEnv {
    pub display_modes: Vec<DisplayMode>,
    /// Width of the active UI viewport, matching `Environment.screenWidth`.
    /// This is distinct from the saved resolution in `Prefs`.
    pub viewport_width: i32,
    /// Video device names (the original listed devices with enough video memory).
    pub devices: Vec<String>,
    /// Selected index in `devices`, matching `Environment.FullScreenDevice`.
    pub active_device: usize,
    /// Opened from a running mission: the resolution and the gameplay tab are locked.
    pub in_mission: bool,
}

impl Default for OptionsEnv {
    fn default() -> Self {
        let modes = [
            (800, 600),
            (1024, 768),
            (1280, 720),
            (1280, 1024),
            (1600, 1200),
            (1920, 1080),
            (2560, 1440),
            (3840, 2160),
        ];
        OptionsEnv {
            viewport_width: 800,
            display_modes: modes
                .iter()
                .map(|&(width, height)| DisplayMode {
                    width,
                    height,
                    bit_depth: 32,
                })
                .collect(),
            devices: vec!["Default".to_string()],
            active_device: 0,
            in_mission: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum OptionsResult {
    Accepted {
        prefs: Box<Prefs>,
        resolution_changed: bool,
    },
    Cancelled,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptionsFrame {
    pub result: Option<OptionsResult>,
    /// Volumes being edited (master, music, sfx, radio, betty), every frame on the audio tab.
    pub volumes: Option<[i32; 5]>,
}

// ---------------------------------------------------------------------------------------------
// ScrollX

/// `ScrollX`: a horizontal slider made of the left, right and tab buttons of the audio tab.
#[derive(Clone, Debug, Default)]
pub struct ScrollX {
    pub obj: Object,
    pub buttons: [AnimButton; 3],
    pub scroll_max: f32,
    pub scroll_pos: f32,
    pub scroll_inc: f32,
    pub page_inc: f32,
    last_x: i32,
}

impl ScrollX {
    /// `ScrollX::init(left, right, tab)`.
    pub fn new(mut left: AnimButton, mut right: AnimButton, mut tab: AnimButton) -> ScrollX {
        let (x, y) = (left.global_x() as f32, left.obj.top());
        let w = right.obj.global_right() as f32 - x;
        let h = right.obj.bottom() - right.obj.top();
        left.set_press_fx(-1);
        left.set_highlight_fx(-1);
        left.set_disabled_fx(-1);
        for b in [&mut left, &mut right, &mut tab] {
            b.set_hold_time(0.01);
        }
        let lw = left.width();
        left.move_to(0.0, 0.0);
        let right_width = right.width();
        right.move_to(w - right_width, 0.0);
        tab.move_to(lw + 1.0, -1.0);
        // addChild: children are relative to the slider
        for b in [&mut left, &mut right, &mut tab] {
            b.move_by(x, y);
        }
        let mut obj = Object::new(x, y, w, h);
        obj.set_color(0, false);
        ScrollX {
            obj,
            buttons: [left, right, tab],
            scroll_max: 0.0,
            scroll_pos: 0.0,
            scroll_inc: 1.0,
            page_inc: 5.0,
            last_x: 0,
        }
    }

    pub fn set_scroll_max(&mut self, m: f32) {
        self.scroll_max = m;
        self.buttons[2].obj.show = m != 0.0;
        self.resize_areas();
    }

    pub fn set_scroll_pos(&mut self, mut p: f32) {
        if p < 0.0 {
            p = 0.0;
        }
        if p > self.scroll_max {
            p = self.scroll_max;
        }
        self.scroll_pos = p;
        self.resize_areas();
    }

    fn resize_areas(&mut self) {
        if self.scroll_max == 0.0 {
            return;
        }
        let [b0, b1, b2] = &self.buttons;
        let range = self.obj.width() - b0.width() - b1.width() - b2.width() - 4.0;
        let position = range * self.scroll_pos / self.scroll_max;
        let x = self.obj.global_x() as f32 + position + b0.width() + 2.0;
        let y = self.obj.global_y() as f32 + 1.0;
        self.buttons[2].move_to(x, y);
    }

    /// `ScrollX::handleMessage` from a child: steps for `SCROLLUP`/`SCROLLDOWN` ids.
    fn child_message(&mut self, who: i32) {
        if who == msg::SCROLLUP as i32 {
            self.set_scroll_pos(self.scroll_pos - self.scroll_inc);
        } else if who == msg::SCROLLDOWN as i32 {
            self.set_scroll_pos(self.scroll_pos + self.scroll_inc);
        }
    }

    /// The child buttons' own update (the original runs it from the tab screen's update and again
    /// from `aObject::update`, so arrows step twice per frame; both calls are kept).
    pub fn update_children(&mut self, ctx: &mut UpdateCtx<'_>) {
        for i in 0..3 {
            if let Some(m) = self.buttons[i].update(ctx) {
                self.child_message(m.who);
            }
        }
    }

    /// `ScrollX::update`.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        let phys = |s: &ScrollX| {
            s.obj.width() - s.buttons[0].width() - s.buttons[1].width() - s.buttons[2].width()
        };
        let delta_of =
            |s: &ScrollX| mx as f32 - (s.buttons[0].global_x() as f32 + s.buttons[0].width());
        if ctx.input.is_left_drag() && self.last_x != 0 {
            let delta = delta_of(self);
            let mut p = phys(self);
            if p == 0.0 {
                p = self.scroll_max;
            }
            let new = (0.5 + delta * self.scroll_max / p).clamp(0.0, self.scroll_max);
            self.set_scroll_pos(new);
        } else if self.obj.point_inside(mx, my) {
            if ctx.input.is_left_click()
                || ctx.input.key_status(keys::KEY_LMOUSE) == KeyStatus::Held
            {
                self.last_x = 0;
                if self.buttons[2].point_inside(mx, my) && self.buttons[2].point_inside(dx, dy) {
                    self.last_x = mx;
                } else {
                    self.buttons[2].press(false);
                    if !self.buttons[0].point_inside(mx, my)
                        && !self.buttons[1].point_inside(mx, my)
                        && self.obj.point_inside(dx, dy)
                    {
                        let p = phys(self);
                        let delta = delta_of(self);
                        let mut new = delta * self.scroll_max / p;
                        if my < self.buttons[2].obj.top() as i32 {
                            if self.scroll_pos - new > self.page_inc {
                                new = self.scroll_pos - self.page_inc;
                            }
                        } else if new - self.scroll_pos > self.page_inc {
                            new = self.scroll_pos + self.page_inc;
                        }
                        self.set_scroll_pos(new);
                    }
                }
            } else {
                self.buttons[2].press(false);
            }
        } else {
            self.buttons[2].press(false);
        }
        if ctx.input.left_mouse_released() {
            self.last_x = 0;
        }
        self.obj.update(ctx);
        self.update_children(ctx);
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.obj.draw_quad(ctx);
        for b in &mut self.buttons {
            b.render(ctx);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// HotKeyListItem

#[derive(Clone, Debug, Default)]
pub struct HotKeyListItem {
    pub obj: Object,
    pub state: ItemState,
    pub description: Text,
    pub text: Text,
    pub animations: [AnimGroup; 3],
    pub rects: [Rect; 2],
    pub hot_key: u32,
    pub command: usize,
}

impl HotKeyListItem {
    /// The template (`HotKeyListItem::init`, `mcl_options_combobox1.fit`).
    pub fn template(ld: &mut Loader<'_>) -> Result<HotKeyListItem, String> {
        let mut file = ld.art_fit("mcl_options_combobox1")?;
        let description = Text::from_fit(ld, &mut file, "Text0");
        let text = Text::from_fit(ld, &mut file, "Text1");
        let animations = [
            AnimGroup::from_block(&mut file, "Animation0"),
            AnimGroup::from_block(&mut file, "Animation1"),
            AnimGroup::from_block(&mut file, "Animation2"),
        ];
        let rects = [
            Rect::from_fit(&mut file, "Rect0"),
            Rect::from_fit(&mut file, "Rect1"),
        ];
        let obj = Object::new(0.0, 0.0, rects[1].obj.right(), rects[0].obj.bottom() + 1.0);
        Ok(HotKeyListItem {
            obj,
            state: ItemState::Enabled,
            description,
            text,
            animations,
            rects,
            hot_key: 0,
            command: 0,
        })
    }
}

impl ListItem for HotKeyListItem {
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
    fn update(&mut self, _ctx: &mut UpdateCtx<'_>) {}
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.description.obj.move_by(dx, dy);
        self.text.obj.move_by(dx, dy);
        for r in &mut self.rects {
            r.obj.move_by(dx, dy);
        }
    }
    /// `HotKeyListItem::render` (the animations advance at render, as in the original).
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let s = self.state.anim_state();
        for a in &mut self.animations {
            a.set_state(s);
            a.update(ctx.dt);
        }
        self.text.set_color(self.animations[1].current_color(s));
        self.description
            .set_color(self.animations[0].current_color(s));
        let c = self.animations[2].current_color(s);
        self.rects[1].obj.set_color(c, false);
        if !self.obj.show {
            return;
        }
        self.obj.draw_quad(ctx);
        self.rects[1].render(ctx);
        self.description.render(ctx);
        self.text.render(ctx);
        self.rects[0].render(ctx);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn hot_item(l: &ListBox, i: usize) -> Option<&HotKeyListItem> {
    l.item(i)?.as_any().downcast_ref()
}

fn hot_item_mut(l: &mut ListBox, i: usize) -> Option<&mut HotKeyListItem> {
    l.item_mut(i)?.as_any_mut().downcast_mut()
}

// ---------------------------------------------------------------------------------------------
// tabs

fn set_tab_button_fx(s: &mut LogisticsScreen) {
    for b in &mut s.buttons {
        b.set_press_fx(sfx::LOG_VIDEOBUTTONS);
        b.set_highlight_fx(sfx::LOG_DIGITALHIGHLIGHT);
        b.set_disabled_fx(sfx::LOG_WRONGBUTTON);
    }
}

fn press(s: &mut LogisticsScreen, id: i32, on: bool) {
    if let Some(b) = s.button_mut(id) {
        b.press(on);
    }
}

fn pressed(s: &LogisticsScreen, id: i32) -> bool {
    s.button(id).is_some_and(|b| b.is_pressed())
}

struct Graphics {
    screen: LogisticsScreen,
    resolution: DropList,
    cards: DropList,
    modes: Vec<DisplayMode>,
    expanded: bool,
}

impl Graphics {
    fn init(
        ld: &mut Loader<'_>,
        screen: LogisticsScreen,
        env: &OptionsEnv,
    ) -> Result<Graphics, String> {
        let mut screen = screen;
        let (gx, gy) = (screen.obj.global_x() as f32, screen.obj.global_y() as f32);
        if let Some(b) = screen.button_mut(MSG_RESET) {
            b.set_message_on_release();
        }
        let mut file = ld.art_fit("mcl_options_combobox0")?;
        let mut resolution = DropList::from_fit(ld, &mut file, "PlayerNameComboBox");
        resolution.move_by(gx, gy);
        resolution.list.set_orange();
        for m in &env.display_modes {
            resolution.add_text(
                &format!("{}x{}x{}", m.width, m.height, m.bit_depth),
                0xffff_ffff,
            );
        }
        let mut file = ld.art_fit("mcl_options_combobox2")?;
        let mut cards = DropList::from_fit(ld, &mut file, "PlayerNameComboBox");
        cards.move_by(gx, gy);
        cards.list.set_orange();
        for d in &env.devices {
            cards.add_text(d, 0xffff_ffff);
        }
        if !env.devices.is_empty() {
            let active = env.active_device.min(env.devices.len() - 1);
            cards.select_item(active as i32);
        }
        set_tab_button_fx(&mut screen);
        screen.help_text_array_id = 1;
        Ok(Graphics {
            screen,
            resolution,
            cards,
            modes: env.display_modes.clone(),
            expanded: false,
        })
    }

    fn reset(&mut self, p: &Prefs) {
        let s = &mut self.screen;
        press(s, MSG_TERRAIN_DETAIL, p.use_water_interest_texture);
        press(s, MSG_PILOT_VIDS, p.pilot_videos);
        press(s, MSG_OBJECT_DETAIL, p.use_high_object_detail);
        press(s, MSG_SHADOWS, p.use_shadows);
        press(s, MSG_NON_WEAPON, p.use_non_weapon_effects);
        press(s, MSG_LOCAL_SHADOWS, p.use_local_shadows);
        press(s, MSG_ASYNC_MOUSE, p.async_mouse);
        press(s, MSG_HARDWARE_RASTERIZER, p.renderer != 3);
        let idx = self
            .modes
            .iter()
            .position(|m| {
                m.width == p.resolution_x
                    && m.height == p.resolution_y
                    && m.bit_depth == p.bit_depth
            })
            .unwrap_or(0);
        self.resolution.select_item(idx as i32);
    }

    fn end(&self, p: &mut Prefs) {
        let s = &self.screen;
        p.use_water_interest_texture = pressed(s, MSG_TERRAIN_DETAIL);
        p.pilot_videos = pressed(s, MSG_PILOT_VIDS);
        p.use_high_object_detail = pressed(s, MSG_OBJECT_DETAIL);
        p.use_shadows = pressed(s, MSG_SHADOWS);
        p.use_non_weapon_effects = pressed(s, MSG_NON_WEAPON);
        p.use_local_shadows = pressed(s, MSG_LOCAL_SHADOWS);
        p.async_mouse = pressed(s, MSG_ASYNC_MOUSE);
        p.renderer = if pressed(s, MSG_HARDWARE_RASTERIZER) {
            0
        } else {
            3
        };
        let sel = self.resolution.selected_item();
        if sel > -1 {
            if let Some(m) = self.modes.get(sel as usize) {
                p.resolution_x = m.width;
                p.resolution_y = m.height;
                p.bit_depth = m.bit_depth;
            }
        }
        let card = self.cards.selected_item();
        if card != -1 && p.renderer != 3 {
            p.renderer = card;
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, in_mission: bool, original: &Prefs) {
        if self.resolution.is_expanded() {
            if let Some(t) = self.screen.texts.get_mut(1) {
                t.set_text("");
            }
            ctx.out.help_text_id = 0;
            self.resolution.update(ctx);
            let help = ctx.out.help_text_id;
            if help != 0 {
                if let Some(t) = self.screen.texts.get_mut(1) {
                    t.set_text_id(ctx.assets, help);
                }
            }
            self.expanded = true;
        } else {
            if !self.expanded {
                for m in self.screen.update(ctx) {
                    if m.who == MSG_RESET && m.msg == msg::LEFTMOUSEDOWN {
                        self.reset(original);
                    }
                }
            }
            if ctx.input.left_mouse_released() {
                self.expanded = false;
            }
        }
        self.cards.update(ctx);
        if !in_mission {
            if !self.expanded {
                self.resolution.update(ctx);
            }
        } else if ctx.input.is_left_click()
            && self
                .resolution
                .point_inside(ctx.input.mouse_x(), ctx.input.mouse_y())
        {
            ctx.play(sfx::LOG_WRONGBUTTON);
        }
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.screen.render(ctx);
        self.resolution.render(ctx);
        self.cards.render(ctx);
    }
}

struct Audio {
    screen: LogisticsScreen,
    sliders: Vec<ScrollX>,
}

impl Audio {
    fn init(screen: LogisticsScreen) -> Audio {
        let mut screen = screen;
        if let Some(b) = screen.button_mut(MSG_RESET) {
            b.set_message_on_release();
        }
        set_tab_button_fx(&mut screen);
        // buttons 0..9 are the low/high arrows, 11..15 the tabs; they move into the sliders
        let mut sliders = Vec::new();
        if screen.buttons.len() >= 16 {
            for i in 0..5 {
                let l = screen.buttons[i * 2].clone();
                let r = screen.buttons[i * 2 + 1].clone();
                let t = screen.buttons[i + 11].clone();
                let mut s = ScrollX::new(l, r, t);
                s.set_scroll_max(255.0);
                sliders.push(s);
            }
            // keep only the reset button in the screen
            let reset = screen.buttons[10].clone();
            screen.buttons = vec![reset];
        }
        screen.help_text_array_id = 15;
        Audio { screen, sliders }
    }

    fn reset(&mut self, p: &Prefs) {
        let v = [
            p.digital_master_volume,
            p.music_volume,
            p.sfx_volume,
            p.radio_volume,
            p.betty_volume,
        ];
        for (s, v) in self.sliders.iter_mut().zip(v) {
            s.set_scroll_pos(v as f32);
        }
    }

    fn volumes(&self) -> [i32; 5] {
        let mut v = [0; 5];
        for (o, s) in v.iter_mut().zip(&self.sliders) {
            *o = s.scroll_pos as i32;
        }
        v
    }

    fn end(&self, p: &mut Prefs) {
        let v = self.volumes();
        p.digital_master_volume = v[0];
        p.music_volume = v[1];
        p.sfx_volume = v[2];
        p.radio_volume = v[3];
        p.betty_volume = v[4];
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, original: &Prefs) {
        for m in self.screen.update(ctx) {
            if m.who == MSG_RESET && m.msg == msg::LEFTMOUSEDOWN {
                self.reset(original);
            }
        }
        // the slider buttons are still in the original's button array: updated there, then again
        // as the slider's children
        for s in &mut self.sliders {
            s.update_children(ctx);
        }
        for s in &mut self.sliders {
            s.update(ctx);
        }
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.screen.render(ctx);
        for s in &mut self.sliders {
            s.render(ctx);
        }
    }
}

struct GamePlay {
    screen: LogisticsScreen,
    /// `camera`: the turn of the 'Mech in `Rect1`.
    spin: Spinner,
    /// Whether the camera exists (it is not set up while a mission runs).
    camera: bool,
}

/// The chassis `OptionsGamePlay` shows (`camera.setMech("Bushwacker", ...)`).
const PREVIEW_MECH: &str = "bushwacker";

impl GamePlay {
    fn init(screen: LogisticsScreen) -> GamePlay {
        let mut screen = screen;
        press(&mut screen, MSG_BASE, true);
        press(&mut screen, MSG_ACCENT, false);
        set_tab_button_fx(&mut screen);
        screen.help_text_array_id = 2;
        GamePlay {
            screen,
            spin: Spinner::default(),
            camera: true,
        }
    }

    /// The picture of the 'Mech: `setMech("Bushwacker", base, highlight, highlight)` paints it
    /// `(highlight, highlight, base)`, with the swatch colours as they are (`optionsarea.cpp:949-950`;
    /// the prefs are the swatch colours, `:962-963`).
    fn model(&self) -> Option<PreviewRequest> {
        let (base, accent, panel) = (
            self.screen.rects.get(36)?,
            self.screen.rects.get(37)?,
            self.screen.rects.get(1)?,
        );
        let (width, height) = rect_size((
            panel.obj.global_x() as f32,
            panel.obj.global_y() as f32,
            panel.obj.global_right() as f32,
            panel.obj.global_bottom() as f32,
        ));
        Some(PreviewRequest {
            shape: PREVIEW_MECH.to_string(),
            paint: [accent.color(), accent.color(), base.color()],
            scale: 0.0,
            yaw: 0.0,
            width,
            height,
            building: false,
            kind: PreviewKind::Mech,
            activated: false,
        })
    }

    fn reset(&mut self, p: &Prefs) {
        for i in MSG_GREEN..=MSG_ELITE {
            press(&mut self.screen, i, false);
        }
        press(&mut self.screen, MSG_GREEN + p.game_difficulty, true);
        if self.screen.rects.len() > 37 {
            self.screen.rects[36].obj.set_color(p.base_color, false);
            self.screen.rects[37]
                .obj
                .set_color(p.highlight_color, false);
        }
        press(&mut self.screen, MSG_UNLIMITED_AMMO, p.use_unlimited_ammo);
        press(
            &mut self.screen,
            MSG_LEFT_CLICK,
            p.use_left_right_mouse_profile,
        );
    }

    fn end(&self, p: &mut Prefs) {
        for i in MSG_GREEN..=MSG_ELITE {
            if pressed(&self.screen, i) {
                p.game_difficulty = i - MSG_GREEN;
            }
        }
        if self.screen.rects.len() > 37 {
            p.base_color = self.screen.rects[36].color();
            p.highlight_color = self.screen.rects[37].color();
        }
        p.use_unlimited_ammo = pressed(&self.screen, MSG_UNLIMITED_AMMO);
        p.use_left_right_mouse_profile = pressed(&self.screen, MSG_LEFT_CLICK);
    }

    fn handle(&mut self, who: i32, original: &Prefs) {
        if (MSG_GREEN..=MSG_ELITE).contains(&who) {
            for i in MSG_GREEN..=MSG_ELITE {
                press(&mut self.screen, i, false);
            }
            press(&mut self.screen, who, true);
        }
        match who {
            MSG_RESET => self.reset(original),
            MSG_BASE | MSG_ACCENT => {
                press(&mut self.screen, MSG_BASE, false);
                press(&mut self.screen, MSG_ACCENT, false);
                press(&mut self.screen, who, true);
            }
            _ => {}
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>, original: &Prefs) {
        // `camera.update()`
        self.spin.update(ctx.dt);
        for m in self.screen.update(ctx) {
            if m.msg == msg::LEFTMOUSEDOWN {
                self.handle(m.who, original);
            }
        }
        if self.screen.rects.len() < 38 || !ctx.input.is_left_click() {
            return;
        }
        let target = if pressed(&self.screen, MSG_BASE) {
            36
        } else {
            37
        };
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if let Some(c) = (4..36)
            .find(|&i| self.screen.rects[i].obj.point_inside(mx, my))
            .map(|i| self.screen.rects[i].color())
        {
            self.screen.rects[target].obj.set_color(c, false);
        }
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.screen.render(ctx);
        if self.screen.rects.len() < 38 {
            return;
        }
        let r = &self.screen.rects;
        let to_match = if pressed(&self.screen, MSG_BASE) {
            r[36].color()
        } else {
            r[37].color()
        };
        if let Some(i) = (4..36).find(|&i| r[i].color() == to_match) {
            let o = &r[i].obj;
            let g = GuiRect::new(
                o.global_x() - 2,
                o.global_y() - 2,
                o.global_right() + 1,
                o.global_bottom() + 1,
            );
            ctx.dl.outline_rect(g, 0xffff_ffff, 0xffff_ffff);
        }
        r[37].render(ctx);
        r[36].render(ctx);
        // `camera.render()`
        if self.camera {
            if let (Some(req), Some(panel)) = (self.model(), self.screen.rects.get(1)) {
                let o = &panel.obj;
                let rect = (
                    o.global_x() as f32,
                    o.global_y() as f32,
                    o.global_right() as f32,
                    o.global_bottom() as f32,
                );
                self.spin.draw(ctx.dl, ctx.assets, &req, rect);
            }
        }
    }
}

struct HotKeys {
    screen: LogisticsScreen,
    list: ListBox,
    template: HotKeyListItem,
    dialog: Dialog,
    show_dialog: bool,
    cur_hot_key: u32,
}

impl HotKeys {
    fn init(ld: &mut Loader<'_>, screen: LogisticsScreen) -> Result<HotKeys, String> {
        let r = screen
            .rects
            .first()
            .map(|r| r.obj.clone())
            .unwrap_or_default();
        let mut list = ListBox::new(ld, r.loc[0].x, r.loc[0].y, r.width(), r.height());
        list.set_orange();
        let template = HotKeyListItem::template(ld)?;
        let dialog = Dialog::load(ld, DialogKind::Ok)?;
        let mut screen = screen;
        screen.help_text_array_id = 2;
        Ok(HotKeys {
            screen,
            list,
            template,
            dialog,
            show_dialog: false,
            cur_hot_key: 0,
        })
    }

    /// `reset(useOld)`: one item per listed command, with the current or the default key.
    fn reset(&mut self, assets: &UiAssets, keys_now: &[u32], use_defaults: bool) {
        self.list.remove_all_items();
        for (i, c) in COMMANDS.iter().enumerate() {
            if c.description == -1 {
                continue;
            }
            let key = if use_defaults {
                c.key
            } else {
                keys_now.get(i).copied().unwrap_or(c.key)
            };
            let mut item = self.template.clone();
            item.description
                .set_text(assets.string(c.description as u32));
            item.text
                .set_text(&hotkeys::key_string(&assets.strings, key));
            item.hot_key = key;
            item.command = i;
            self.list.add_item(Box::new(item));
        }
    }

    fn end(&self, p: &mut Prefs) {
        for i in 0..self.list.item_count() {
            if let Some(it) = hot_item(&self.list, i) {
                if let Some(k) = p.keys.get_mut(it.command) {
                    *k = it.hot_key;
                }
            }
        }
    }

    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        if self.show_dialog {
            self.dialog.update(ctx);
            if self.dialog.is_done() {
                self.show_dialog = false;
                if self.dialog.result == YES {
                    self.apply_with_swap(ctx.assets);
                }
            }
            return;
        }
        for m in self.screen.update(ctx) {
            if m.who == MSG_RESET && m.msg == msg::LEFTMOUSEDOWN {
                let defaults: Vec<u32> = COMMANDS.iter().map(|c| c.key).collect();
                self.reset(ctx.assets, &defaults, true);
            }
        }
        self.list.update(ctx);
        loop {
            let index = self.list.selected_item();
            if index < 0 {
                break;
            }
            let key = ctx.input.pop_key();
            if key == 0 {
                break;
            }
            let Some(hot) =
                hotkeys::input_hot_key(key, ctx.input.shift(), ctx.input.ctrl(), ctx.input.alt())
            else {
                return;
            };
            self.cur_hot_key = hot;
            let clash = (0..self.list.item_count()).any(|i| {
                i as i32 != index && hot_item(&self.list, i).is_some_and(|it| it.hot_key == hot)
            });
            if clash {
                self.dialog.set_text_ids(
                    ctx.assets,
                    IDS_OPTIONS_HOTKEY_ERROR,
                    IDS_DIALOG_NO,
                    IDS_DIALOG_YES,
                );
                self.dialog.begin();
                self.show_dialog = true;
                break;
            }
            let s = hotkeys::key_string(&ctx.assets.strings, hot);
            if let Some(it) = hot_item_mut(&mut self.list, index as usize) {
                it.hot_key = hot;
                it.text.set_text(&s);
            }
            self.list.select_item(-1);
        }
    }

    /// The "yes" branch of the clash dialog: the other command gets its default key when that is
    /// free, otherwise the edited command's old key; the edited command gets the new key.
    fn apply_with_swap(&mut self, assets: &UiAssets) {
        let index = self.list.selected_item();
        if index < 0 {
            return;
        }
        let new_key = self.cur_hot_key;
        let old_of_target = hot_item(&self.list, index as usize).map_or(0, |i| i.hot_key);
        for i in 0..self.list.item_count() {
            if i as i32 == index {
                continue;
            }
            let Some(it) = hot_item(&self.list, i) else {
                continue;
            };
            if it.hot_key != new_key {
                continue;
            }
            let mut default = COMMANDS[it.command].key;
            if (0..self.list.item_count())
                .any(|j| hot_item(&self.list, j).is_some_and(|o| o.hot_key == default))
            {
                default = u32::MAX;
            }
            let old = if default != u32::MAX {
                default
            } else {
                old_of_target
            };
            let s = hotkeys::key_string(&assets.strings, old);
            if let Some(it) = hot_item_mut(&mut self.list, i) {
                it.hot_key = old;
                it.text.set_text(&s);
            }
        }
        let s = hotkeys::key_string(&assets.strings, new_key);
        if let Some(it) = hot_item_mut(&mut self.list, index as usize) {
            it.text.set_text(&s);
            it.hot_key = new_key;
        }
        self.list.select_item(-1);
    }

    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.list.render(ctx);
        self.screen.render(ctx);
        if self.show_dialog {
            self.dialog.render(ctx);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// the screen

/// `OptionsXScreen` + `OptionsScreenWrapper`.
pub struct OptionsScreen {
    pub screen: LogisticsScreen,
    graphics: Graphics,
    audio: Audio,
    gameplay: GamePlay,
    hotkeys: HotKeys,
    pub cur_tab: usize,
    show_warning: bool,
    warning: Dialog,
    pub prefs: Prefs,
    pub original: Prefs,
    in_mission: bool,
    done: bool,
}

impl OptionsScreen {
    /// `OptionsXScreen::init` with the current prefs and the defaults the reset buttons restore
    /// (`orgprefs.cfg`).
    pub fn load(
        ld: &mut Loader<'_>,
        prefs: Prefs,
        original: Prefs,
        env: &OptionsEnv,
    ) -> Result<OptionsScreen, String> {
        let mut screen = LogisticsScreen::load(
            ld,
            "mcl_options",
            Prefixes {
                anim_objects: None,
                ..Prefixes::default()
            },
        )?;
        screen.help_text_array_id = 1;
        let (tx, ty) = screen
            .rects
            .get(2)
            .map_or((0.0, 0.0), |r| (r.obj.loc[0].x, r.obj.loc[0].y));
        let (x_offset, y_offset) = match env.viewport_width {
            640 => (-80.0, -90.0),
            1024 => (13.0, 54.0),
            1280 => (240.0, 182.0),
            1600 => (400.0, 270.0),
            _ => (0.0, 0.0),
        };
        screen.move_by(x_offset, y_offset);
        let p = Prefixes {
            anim_objects: None,
            ..Prefixes::default()
        };
        let mut tab = |name: &str| -> Result<LogisticsScreen, String> {
            let mut s = LogisticsScreen::load(ld, name, p)?;
            s.move_to(tx + x_offset, ty + y_offset);
            Ok(s)
        };
        let g = tab("mcl_optionsgraphics")?;
        let a = tab("mcl_optionsaudio")?;
        let gp = tab("mcl_optionsgameplay")?;
        let hk = tab("mcl_optionshotkeys")?;
        let graphics = Graphics::init(ld, g, env)?;
        let audio = Audio::init(a);
        let gameplay = GamePlay::init(gp);
        let hotkeys = HotKeys::init(ld, hk)?;
        let warning = Dialog::load(ld, DialogKind::OneButton)?;
        let mut o = OptionsScreen {
            screen,
            graphics,
            audio,
            gameplay,
            hotkeys,
            cur_tab: 0,
            show_warning: false,
            warning,
            prefs,
            original,
            in_mission: env.in_mission,
            done: false,
        };
        // tabArea->begin(): show the current prefs
        let pr = o.prefs.clone();
        o.graphics.reset(&pr);
        o.audio.reset(&pr);
        o.gameplay.reset(&pr);
        let keys_now = pr.keys.clone();
        o.hotkeys.reset(ld.assets, &keys_now, false);
        if env.in_mission {
            if let Some(b) = o.screen.button_mut(MSB_TAB0 + 2) {
                b.disable(true);
            }
            // no camera in a mission: the model would outlive the logistics heaps
            o.gameplay.camera = false;
        }
        for b in &mut o.screen.buttons {
            if !(MSB_TAB0..=MSB_TAB3).contains(&b.id()) {
                b.set_message_on_release();
            }
        }
        press(&mut o.screen, MSB_TAB0, true);
        Ok(o)
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The picture of the 'Mech the gameplay tab spins, as it is asked for this frame (`None`
    /// while a mission runs, when the tab has no camera).
    pub fn gameplay_preview(&self) -> Option<PreviewRequest> {
        if !self.gameplay.camera {
            return None;
        }
        let req = self.gameplay.model()?;
        Some(self.gameplay.spin.request(&req))
    }

    /// Drops the "takes effect on the next mission" dialog and finishes the screen, for a host
    /// that does not apply the new resolution.
    pub fn dismiss_warning(&mut self) {
        if self.show_warning {
            self.show_warning = false;
            self.done = true;
        }
    }

    /// The gameplay tab's colour swatches (rects 4..36) as `(centre, 0xAARRGGBB)`, for scripted
    /// UI input.
    pub fn gameplay_swatches(&self) -> Vec<((f32, f32), u32)> {
        (4..36)
            .filter_map(|i| self.gameplay.screen.rects.get(i))
            .map(|r| {
                let o = &r.obj;
                let at = (
                    o.global_x() as f32 + o.width() * 0.5,
                    o.global_y() as f32 + o.height() * 0.5,
                );
                (at, r.color())
            })
            .collect()
    }

    /// Selects a tab (as a click on its button does).
    pub fn select_tab(&mut self, tab: usize) {
        for i in MSB_TAB0..=MSB_TAB3 {
            press(&mut self.screen, i, false);
        }
        press(&mut self.screen, MSB_TAB0 + tab as i32, true);
        self.cur_tab = tab.min(3);
    }

    /// Picks the `idx`-th display mode of the graphics tab (as choosing it from the list does).
    pub fn select_resolution(&mut self, idx: usize) -> bool {
        self.graphics.resolution.select_item(idx as i32)
    }

    /// `tabAreas[i]->end()` for every tab: the prefs as edited.
    pub fn edited_prefs(&self) -> Prefs {
        let mut p = self.prefs.clone();
        self.graphics.end(&mut p);
        self.audio.end(&mut p);
        self.gameplay.end(&mut p);
        self.hotkeys.end(&mut p);
        p
    }

    /// Center of the `i`-th row of the hot keys list, for scripted/headless UI input.
    pub fn hotkey_row_point(&self, i: usize) -> Option<(f32, f32)> {
        let o = self.hotkeys.list.item(i)?.obj();
        Some((
            o.global_x() as f32 + o.width() * 0.5,
            o.global_y() as f32 + o.height() * 0.5,
        ))
    }

    /// Center of the resolution drop list's expand button, for scripted/headless UI input.
    pub fn resolution_expand_point(&self) -> (f32, f32) {
        let b = &self.graphics.resolution.expand;
        (
            b.global_x() as f32 + b.width() * 0.5,
            b.global_y() as f32 + b.height() * 0.5,
        )
    }

    /// Center of the `i`-th entry of the expanded resolution list, for scripted/headless UI input.
    pub fn resolution_item_point(&self, i: usize) -> Option<(f32, f32)> {
        let o = self.graphics.resolution.list.item(i)?.obj();
        Some((
            o.global_x() as f32 + o.width() * 0.5,
            o.global_y() as f32 + o.height() * 0.5,
        ))
    }

    /// Center of a control of the shown tab (a check box or radio button), for scripted/headless
    /// UI input.
    pub fn tab_control_point(&self, id: i32) -> Option<(f32, f32)> {
        let screen = match self.cur_tab {
            0 => &self.graphics.screen,
            1 => &self.audio.screen,
            2 => &self.gameplay.screen,
            _ => &self.hotkeys.screen,
        };
        let b = screen.button(id)?;
        Some((
            b.obj.global_x() as f32 + b.obj.width() * 0.5,
            b.obj.global_y() as f32 + b.obj.height() * 0.5,
        ))
    }

    /// Center of an audio slider's right-step button, for scripted/headless UI input.
    pub fn volume_step_point(&self, slider: usize) -> Option<(f32, f32)> {
        let control = self.audio.sliders.get(slider)?.buttons.get(1)?;
        Some((
            control.global_x() as f32 + control.width() * 0.5,
            control.global_y() as f32 + control.height() * 0.5,
        ))
    }

    /// Center of an audio slider's left-step button, for scripted/headless UI input.
    pub fn volume_decrease_point(&self, slider: usize) -> Option<(f32, f32)> {
        let control = self.audio.sliders.get(slider)?.buttons.first()?;
        Some((
            control.global_x() as f32 + control.width() * 0.5,
            control.global_y() as f32 + control.height() * 0.5,
        ))
    }

    fn handle(&mut self, ctx: &mut UpdateCtx<'_>, m: Msg, frame: &mut OptionsFrame) {
        if m.msg != msg::LEFTMOUSEDOWN {
            return;
        }
        // YES/NO are also used by modal dialogs. Only buttons owned by the main options FIT
        // may accept or cancel the screen.
        if !self
            .screen
            .buttons
            .iter()
            .any(|button| button.id() == m.who)
        {
            return;
        }
        match m.who {
            w if (MSB_TAB0..=MSB_TAB3).contains(&w) => self.select_tab((w - MSB_TAB0) as usize),
            YES => {
                let p = self.edited_prefs();
                let changed = p.resolution_x != self.prefs.resolution_x
                    || p.resolution_y != self.prefs.resolution_y
                    || p.bit_depth != self.prefs.bit_depth;
                frame.result = Some(OptionsResult::Accepted {
                    prefs: Box::new(p.clone()),
                    resolution_changed: changed,
                });
                self.prefs = p;
                if changed {
                    self.warning.set_text_ids(
                        ctx.assets,
                        IDS_SWAP_RESOLUTION_WARNING,
                        IDS_DIALOG_OK,
                        IDS_DIALOG_OK,
                    );
                    self.warning.begin();
                    self.show_warning = true;
                } else {
                    self.done = true;
                }
            }
            NO => {
                frame.result = Some(OptionsResult::Cancelled);
                self.done = true;
            }
            _ => {}
        }
    }

    /// `OptionsXScreen::update`. The result comes back on the frame the player accepts or
    /// cancels; after a resolution change the warning dialog is still shown until
    /// [`OptionsScreen::is_done`].
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> OptionsFrame {
        let mut frame = OptionsFrame::default();
        ctx.input.mouse_state = crate::cursor::M_STATE_NORMAL;
        if self.done {
            return frame;
        }
        if self.show_warning {
            self.warning.update(ctx);
            if self.warning.is_done() {
                self.done = true;
            }
            return frame;
        }
        let msgs = self.screen.update(ctx);
        for m in msgs {
            self.handle(ctx, m, &mut frame);
        }
        if self.done || self.show_warning {
            return frame;
        }
        let original = self.original.clone();
        match self.cur_tab {
            0 => self.graphics.update(ctx, self.in_mission, &original),
            1 => {
                self.audio.update(ctx, &original);
                frame.volumes = Some(self.audio.volumes());
            }
            2 => self.gameplay.update(ctx, &original),
            _ => self.hotkeys.update(ctx),
        }
        frame
    }

    /// `OptionsXScreen::render`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let (w, h) = ctx.screen();
        ctx.dl.fill_rect(GuiRect::new(0, 0, w, h), 0xff00_0000);
        if let Some(r) = self.screen.rects.get_mut(1) {
            r.obj.set_color(0xff00_0000, false);
            r.render(ctx);
        }
        match self.cur_tab {
            0 => self.graphics.render(ctx),
            1 => self.audio.render(ctx),
            _ => {}
        }
        if let Some(r) = self.screen.rects.get_mut(1) {
            r.obj.set_color(0, false);
        }
        self.screen.render(ctx);
        let tab_id = MSB_TAB0 + self.cur_tab as i32;
        if let Some(b) = self.screen.buttons.iter_mut().find(|b| b.id() == tab_id) {
            b.render(ctx);
        }
        match self.cur_tab {
            2 => self.gameplay.render(ctx),
            3 => self.hotkeys.render(ctx),
            _ => {}
        }
        if self.show_warning {
            self.warning.render(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::listbox::AnimTextListItem;
    use crate::{Loader, UiAssets};
    use mc2_formats::vfs::Vfs;

    /// The colour `AnimTextListItem::render` (`aAnimTextListItem::render`) gives an item: the
    /// anim group's colour for the item's state, with the animation freshly begun on a change.
    fn drawn_colour(it: &dyn ListItem) -> u32 {
        let a = it
            .as_any()
            .downcast_ref::<AnimTextListItem>()
            .expect("anim text item");
        let mut anim = a.anim.clone();
        let s = it.state().anim_state();
        anim.set_state(s);
        anim.current_color(s)
    }

    /// `ref-recapture-hires` shows the resolution value white where Rust drew it brown. The C++
    /// draws the value by the item's state (`aDropList::render`, `aAnimTextListItem::render`,
    /// gui/alistbox.cpp:773, :1403): `SelectItem` leaves it SELECTED, whose "Pressed" animation
    /// starts white; `aListBox::update` (:119, skipped while the list is hidden, so the animation
    /// never advances) returns it to ENABLED, whose "Normal" colour is `ff846643`. It is never the
    /// DISABLED grey (`ff6b6b6b`). The reference is white because `OptionsGraphics::bExpanded` is
    /// never initialised (code/optionsarea.cpp:362) and its `update()` (:554) skipped the resolution
    /// list until the first mouse release; the Rust screen starts it at false, so it updates at once.
    #[test]
    fn resolution_value_colour_follows_item_state_never_disabled_grey() {
        let Some(root) = std::env::var_os("MC2_DATA") else {
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).unwrap();
        let mut assets = UiAssets::default();
        let mut ld = Loader::new(&mut vfs, &mut assets);
        let mut o = OptionsScreen::load(
            &mut ld,
            Prefs::default(),
            Prefs::default(),
            &OptionsEnv::default(),
        )
        .unwrap();
        let res = &mut o.graphics.resolution;
        let sel = res.selected_item();
        assert!(sel >= 0);
        let it = res.list.item(sel as usize).unwrap();
        assert_eq!(it.state(), ItemState::Selected);
        assert_eq!(drawn_colour(it), 0xffff_ffff);
        res.list.select_item(-1);
        let it = res.list.item(sel as usize).unwrap();
        assert_eq!(it.state(), ItemState::Enabled);
        assert_eq!(drawn_colour(it), 0xff84_6643);
        // the video card list is built from the same template and must agree
        let card = &o.graphics.cards;
        let c = card
            .list
            .item(card.selected_item().max(0) as usize)
            .unwrap();
        assert_ne!(drawn_colour(c), 0xff6b_6b6b);
    }
}
