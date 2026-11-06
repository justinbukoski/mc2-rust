// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! In-mission pause panel from `code/pausewindow.cpp` and the PauseWindow blocks in
//! `buttonlayout*.fit`.

use crate::control::{ButtonArt, ButtonVisualState, ControlButton, ControlRect};
use crate::ctx::{Loader, RenderCtx, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::input::UiInput;
use crate::logistics_dialog::YesNoDialog;
use crate::screen::Status;

const GAME_PAUSED: u32 = 40099;
const PAUSED_HEADER_FONT: u32 = 40017;
const PAUSE_BUTTON_FONT: u32 = 40031;

/// `IDS_DIALOG_ABORT_PROMPT`, `IDS_DIALOG_QUIT_PROMPT`, `IDS_DIALOG_NO`, `IDS_DIALOG_YES`
/// (`strings.res.h`).
const ABORT_PROMPT: u32 = 27677;
const QUIT_PROMPT: u32 = 27654;
const DIALOG_NO: i32 = 27653;
const DIALOG_YES: i32 = 27652;

/// What the pause panel asks the game to do (`PauseWindow::handleClick`, `pausewindow.cpp:365-425`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseIntent {
    /// RETURN (and OPTIONS, after it): `togglePause`.
    Resume,
    ToggleObjectives,
    /// SAVE: `saveInMissionSave = true`. The game refuses it in multiplayer and single missions
    /// (the button then plays `INVALID_GUI`).
    Save,
    /// LOAD: `loadInMissionSave = true`, with the same refusals.
    Load,
    /// OPTIONS: `bInvokeOptionsScreenFlag = true`; the panel resumes the game as well (a
    /// separate [`PauseIntent::Resume`] follows).
    Options,
    /// RESTART, answered Yes: relaunch the current mission from its original load specification.
    /// The C++ button instead aborts the mission (`pausewindow.cpp:74-97`); owner instruction
    /// for this port explicitly requires relaunch.
    RestartMission,
    /// EXIT, answered Yes in the quit prompt: `gos_TerminateApplication` (`pausewindow.cpp:80`).
    QuitGame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prompt {
    Abort,
    Quit,
}

pub struct PauseWindow {
    buttons: Vec<ControlButton>,
    statics: Vec<(ControlRect, ButtonArt)>,
    backgrounds: [GuiRect; 2],
    header_font: crate::font::FontRef,
    /// `IDS_PAUSEBUTTON800`, the font every button label uses (`pausewindow.cpp:314`).
    button_font: crate::font::FontRef,
    /// Per button: `TextID` and the `TextNormal/Pressed/Disabled/Highlight` colours
    /// (`ButtonData::textID`/`textColors`, `controlgui.cpp:2269-2273`).
    labels: Vec<(u32, [u32; 4])>,
    objectives_on: bool,
    started: bool,
    elapsed: f32,
    current_x: f32,
    screen_width: f32,
    pressed_button: Option<i32>,
    released_button: Option<i32>,
    pressed_at: Option<(i32, i32)>,
    dragged: bool,
    /// The yes/no question RESTART and EXIT ask (`LogisticsOKDialog`); `bPromptToAbort`/`bPromptToQuit`.
    dialog: YesNoDialog,
    prompt: Option<Prompt>,
    /// The prompt was chosen this frame; the dialog opens on the next [`Self::update_dialog`].
    prompt_to_open: bool,
    /// How far the 800x600 dialog layout is moved to the middle of the screen.
    dialog_shift: (f32, f32),
}

impl std::fmt::Debug for PauseWindow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PauseWindow")
            .field("started", &self.started)
            .field("prompt", &self.prompt)
            .finish_non_exhaustive()
    }
}

impl PauseWindow {
    /// Loads the same resolution-specific FIT used by ControlGui, then reads its PauseWindow
    /// blocks (`controlgui.cpp:2799`, `pausewindow.cpp:290-360`).
    pub fn load(ld: &mut Loader<'_>, width: u32, height: u32) -> Result<Self, String> {
        Self::load_wide(ld, width, height, 0)
    }

    /// [`PauseWindow::load`] on a screen `extra` layout pixels wider than the layout's block: the panel
    /// is the right edge's, so it moves right by `extra` (a port extension, see `ControlPanel::load_wide`);
    /// the slide ends at the wide view's right edge.
    pub fn load_wide(
        ld: &mut Loader<'_>,
        width: u32,
        _height: u32,
        extra: i32,
    ) -> Result<Self, String> {
        let name = match width {
            640 => "buttonlayout640",
            800 => "buttonlayout800",
            1024 => "buttonlayout1024",
            1280 => "buttonlayout1280",
            1600 => "buttonlayout1600",
            1920 => "buttonlayout1920",
            _ => return Err(format!("unsupported pause layout width {width}")),
        };
        let mut file = ld.art_fit(name)?;
        file.seek_block("HiresOffsets").ok();
        // a wide screen puts the HUD block at its left edge (see `ControlPanel::load_wide`)
        let hires = if extra > 0 {
            0
        } else {
            file.long_or("xOffset", 0) as i32
        };
        let x_offset = hires + extra;
        file.seek_block("PauseWindow").map_err(|e| e.to_string())?;
        let count = file.long_or("ButtonCount", -1);
        if count != 7 {
            return Err(format!("PauseWindow ButtonCount={count}, expected 7"));
        }
        let mut buttons = Vec::with_capacity(7);
        let mut labels = Vec::with_capacity(7);
        for index in 0..7 {
            file.seek_block(&format!("PauseButton{index}"))
                .map_err(|e| e.to_string())?;
            labels.push((
                file.long_or("TextID", 0) as u32,
                [
                    file.long_or("TextNormal", 0) as u32,
                    file.long_or("TextPressed", 0) as u32,
                    file.long_or("TextDisabled", 0) as u32,
                    file.long_or("TextHighlight", 0) as u32,
                ],
            ));
            let texture = file.string("FileName").and_then(|path| ld.art(&path));
            let texture_size = texture
                .and_then(|id| ld.assets.textures.get(id))
                .map_or((0.0, 0.0), |image| {
                    (image.width as f32, image.height as f32)
                });
            let denom_x = texture_size.0.max(1.0);
            let denom_y = texture_size.1.max(1.0);
            let uv = [
                ("UNormal", "VNormal"),
                ("UPressed", "VPressed"),
                ("UDisabled", "VDisabled"),
                ("UAmbiguous", "VAmbiguous"),
            ]
            .map(|(u, v)| {
                let u = if u == "UAmbiguous" {
                    file.long_or(u, file.long_or("UHighlight", -1))
                } else {
                    file.long_or(u, -1)
                };
                let v = if v == "VAmbiguous" {
                    file.long_or(v, file.long_or("VHighlight", -1))
                } else {
                    file.long_or(v, -1)
                };
                (u as f32 / denom_x, v as f32 / denom_y)
            });
            let art = ButtonArt {
                texture,
                uv,
                uv_size: (
                    file.long_or("UWidth", 0) as f32 / denom_x,
                    file.long_or("VHeight", 0) as f32 / denom_y,
                ),
                texture_size,
                rotated: file.boolean("texturesRotated").unwrap_or(false),
            };
            buttons.push(ControlButton {
                id: None,
                cxx_id: file.long_or("ID", index) as i32,
                rect: ControlRect::from_fit(
                    &file,
                    "XLocation",
                    "YLocation",
                    "Width",
                    "Height",
                    // `ControlButton::initButtons` adds the hi-res offsets and `PauseWindow::init`
                    // takes the y one off again (`pausewindow.cpp:317-319`): the buttons keep the
                    // FIT's y
                    x_offset,
                    0,
                ),
                help_header: file.long_or("HelpCaption", 0) as u32,
                help_text: file.long_or("HelpDesc", 0) as u32,
                art,
                state: ButtonVisualState::Enabled,
            });
        }
        file.seek_block("PauseWindow").map_err(|e| e.to_string())?;
        let static_count = file.long_or("staticCount", 0).max(0) as usize;
        let mut statics = Vec::with_capacity(static_count);
        for index in 0..static_count {
            file.seek_block(&format!("PauseStatic{index}"))
                .map_err(|e| e.to_string())?;
            let texture = file.string("FileName").and_then(|path| ld.art(&path));
            let texture_size = texture
                .and_then(|id| ld.assets.textures.get(id))
                .map_or((0.0, 0.0), |image| {
                    (image.width as f32, image.height as f32)
                });
            let denom_x = texture_size.0.max(1.0);
            let denom_y = texture_size.1.max(1.0);
            let uv = [(
                file.long_or("UNormal", -1) as f32 / denom_x,
                file.long_or("VNormal", -1) as f32 / denom_y,
            ); 4];
            let art = ButtonArt {
                texture,
                uv,
                uv_size: (
                    file.long_or("UWidth", 0) as f32 / denom_x,
                    file.long_or("VHeight", 0) as f32 / denom_y,
                ),
                texture_size,
                rotated: file.boolean("texturesRotated").unwrap_or(false),
            };
            statics.push((
                ControlRect::from_fit(
                    &file,
                    "XLocation",
                    "YLocation",
                    "Width",
                    "Height",
                    x_offset,
                    0,
                ),
                art,
            ));
        }
        let mut backgrounds = [GuiRect::default(); 2];
        for (index, block) in ["PauseBackRect", "PauseText"].into_iter().enumerate() {
            file.seek_block(block).map_err(|e| e.to_string())?;
            backgrounds[index] = GuiRect::new(
                file.long_or("left", 0) as i32 + x_offset,
                file.long_or("top", 0) as i32,
                file.long_or("right", 0) as i32 + x_offset,
                file.long_or("bottom", 0) as i32,
            );
        }
        Ok(Self {
            buttons,
            statics,
            backgrounds,
            header_font: ld.font(PAUSED_HEADER_FONT),
            button_font: ld.font(PAUSE_BUTTON_FONT),
            labels,
            objectives_on: false,
            started: false,
            elapsed: 0.0,
            current_x: backgrounds[0].left as f32,
            screen_width: if extra > 0 {
                (width.min(1024) as i32 + extra) as f32
            } else {
                width as f32
            },
            pressed_button: None,
            released_button: None,
            pressed_at: None,
            dragged: false,
            dialog: YesNoDialog::load_animated(ld)?,
            prompt: None,
            prompt_to_open: false,
            dialog_shift: (0.0, 0.0),
        })
    }

    /// Starts the same slide-in as `PauseWindow::begin` (`pausewindow.cpp:453-460`).
    pub fn begin(&mut self, objectives_on: bool) {
        self.elapsed = 0.0;
        self.started = true;
        self.objectives_on = objectives_on;
        self.current_x = self.backgrounds[0].left as f32;
        if let Some(button) = self.buttons.iter_mut().find(|button| button.cxx_id == 0) {
            button.state = if objectives_on {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            };
        }
    }

    pub fn end(&mut self) {
        self.prompt = None;
        self.prompt_to_open = false;
        self.dialog.close();
        self.elapsed = 5.0;
        self.started = false;
        self.pressed_button = None;
        self.released_button = None;
        self.pressed_at = None;
        self.dragged = false;
    }

    /// A yes/no question is waiting or open: it takes the pointer (`LogisticsOKDialog`).
    pub fn prompt_open(&self) -> bool {
        self.prompt.is_some()
    }

    /// The prompt half of `PauseWindow::update` (`pausewindow.cpp:74-97`): opens the question
    /// RESTART or EXIT chose, runs the dialog, and on Yes returns the action.
    pub fn update_dialog(&mut self, ctx: &mut UpdateCtx<'_>) -> Vec<PauseIntent> {
        let Some(prompt) = self.prompt else {
            // an answered question slides out (`LeaveAnim`)
            if self.dialog.is_leaving() {
                let _ = self.dialog.update(ctx);
            }
            return Vec::new();
        };
        if self.prompt_to_open {
            self.prompt_to_open = false;
            let text = ctx
                .assets
                .string(match prompt {
                    Prompt::Abort => ABORT_PROMPT,
                    Prompt::Quit => QUIT_PROMPT,
                })
                .to_string();
            self.dialog.set_text(&text, DIALOG_NO, DIALOG_YES);
            // the dialog's layout is for 800x600; the mission can be any size
            let shift = (
                ((ctx.assets.screen.0 - 800) / 2) as f32,
                ((ctx.assets.screen.1 - 600) / 2) as f32,
            );
            self.dialog
                .screen
                .move_by(shift.0 - self.dialog_shift.0, shift.1 - self.dialog_shift.1);
            self.dialog_shift = shift;
            self.dialog.begin();
            return Vec::new();
        }
        match self.dialog.update(ctx) {
            Some(Status::Yes) => {
                self.prompt = None;
                vec![match prompt {
                    Prompt::Abort => PauseIntent::RestartMission,
                    Prompt::Quit => PauseIntent::QuitGame,
                }]
            }
            Some(_) => {
                self.prompt = None;
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn objectives_on(&self) -> bool {
        self.objectives_on
    }

    pub fn has_completed_button_click(&self) -> bool {
        self.released_button.is_some()
    }

    pub fn set_objectives_on(&mut self, enabled: bool) {
        self.objectives_on = enabled;
        if let Some(button) = self.buttons.first_mut() {
            button.state = if enabled {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            };
        }
    }

    pub fn button_rect(&self, index: usize) -> Option<ControlRect> {
        self.buttons
            .get(index)
            .map(|button| self.translated_button_rect(button.rect))
    }

    pub fn button_rect_for_id(&self, id: i32) -> Option<ControlRect> {
        self.buttons
            .iter()
            .find(|button| button.cxx_id == id)
            .map(|button| self.translated_button_rect(button.rect))
    }

    fn panel_offset(&self) -> i32 {
        self.current_x.round() as i32 - self.backgrounds[0].left
    }

    fn translated_button_rect(&self, rect: ControlRect) -> ControlRect {
        let dx = self.panel_offset();
        ControlRect {
            left: rect.left + dx,
            top: rect.top,
            right: rect.right + dx,
            bottom: rect.bottom,
        }
    }

    pub fn panel_rect(&self) -> GuiRect {
        let rect = self.backgrounds[0];
        let dx = self.panel_offset();
        GuiRect::new(rect.left + dx, rect.top, rect.right + dx, rect.bottom)
    }

    /// Updates slide animation, FIT hit states, and button intents. Unsupported app actions are
    /// explicit refusals so the caller cannot mistake a visual click for a completed action.
    pub fn update(&mut self, mouse: (i32, i32), clicked: bool, dt: f32) -> Vec<PauseIntent> {
        if !self.started {
            self.begin(self.objectives_on);
        }
        if self.prompt.is_some() {
            self.elapsed += dt.max(0.0);
            return Vec::new();
        }
        self.elapsed += dt.max(0.0);
        if self.elapsed == 0.0 {
            self.elapsed = 0.0001;
        }
        let (position, next) = animation_position(self.elapsed);
        let wanted_x = self.screen_width - (800.0 - position);
        self.current_x = wanted_x;
        for button in &mut self.buttons {
            if button.state == ButtonVisualState::Ambiguous {
                button.state = ButtonVisualState::Enabled;
            }
        }
        let _ = next;
        let clicked_id = if clicked {
            self.released_button.take()
        } else {
            self.released_button = None;
            None
        };
        let Some(index) = self.buttons.iter().position(|button| {
            let hit = self
                .translated_button_rect(button.rect)
                .contains(mouse.0, mouse.1)
                && button.state != ButtonVisualState::Disabled;
            hit && clicked_id.is_none_or(|id| id == button.cxx_id)
        }) else {
            return Vec::new();
        };
        if !clicked {
            if self.buttons[index].state != ButtonVisualState::Pressed {
                self.buttons[index].state = ButtonVisualState::Ambiguous;
            }
            return Vec::new();
        }
        let action = match self.buttons[index].cxx_id {
            0 => {
                self.objectives_on = !self.objectives_on;
                self.buttons[index].state = if self.objectives_on {
                    ButtonVisualState::Pressed
                } else {
                    ButtonVisualState::Enabled
                };
                PauseIntent::ToggleObjectives
            }
            1 => PauseIntent::Save,
            2 => PauseIntent::Load,
            3 => PauseIntent::Options,
            4 => {
                self.prompt = Some(Prompt::Abort);
                self.prompt_to_open = true;
                return Vec::new();
            }
            5 => {
                self.prompt = Some(Prompt::Quit);
                self.prompt_to_open = true;
                return Vec::new();
            }
            6 => PauseIntent::Resume,
            _ => return Vec::new(),
        };
        if self.buttons[index].cxx_id == 3 {
            vec![action, PauseIntent::Resume]
        } else {
            vec![action]
        }
    }

    /// Objective hotkey behavior in `PauseWindow::update` (`pausewindow.cpp:106-114`).
    pub fn toggle_objectives(&mut self) -> PauseIntent {
        self.objectives_on = !self.objectives_on;
        if let Some(button) = self.buttons.iter_mut().find(|button| button.cxx_id == 0) {
            button.state = if self.objectives_on {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            };
        }
        PauseIntent::ToggleObjectives
    }

    /// Routes a press only when the pointer lies in the visible pause panel. The caller still
    /// feeds releases to `UiInput`, matching the shared GUI input ownership rule.
    pub fn route_event(
        &mut self,
        input: &mut UiInput,
        event: &mc2_platform::input::InputEvent,
    ) -> bool {
        let pointer = match event {
            mc2_platform::input::InputEvent::CursorMoved { x, y } => (*x as i32, *y as i32),
            _ => input.pending_pointer(),
        };
        if self.prompt.is_some() {
            // the question is modal: the pointer belongs to its buttons
            input.handle_routed(event, true);
            return true;
        }
        let panel_contains = self.is_started() && self.panel_rect().contains(pointer.0, pointer.1);
        let consumed = match event {
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            } => {
                self.pressed_button = self.buttons.iter().find_map(|button| {
                    (self
                        .translated_button_rect(button.rect)
                        .contains(pointer.0, pointer.1)
                        && button.state != ButtonVisualState::Disabled)
                        .then_some(button.cxx_id)
                });
                self.pressed_at = self.pressed_button.map(|_| pointer);
                self.dragged = false;
                panel_contains
            }
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            } => {
                self.released_button = self.pressed_button.take().filter(|id| {
                    !self.dragged
                        && self.buttons.iter().any(|button| {
                            button.cxx_id == *id
                                && self
                                    .translated_button_rect(button.rect)
                                    .contains(pointer.0, pointer.1)
                        })
                });
                self.pressed_at = None;
                self.dragged = false;
                panel_contains || self.released_button.is_some()
            }
            mc2_platform::input::InputEvent::CursorMoved { .. } => {
                if let Some((x, y)) = self.pressed_at {
                    let size = input.size();
                    self.dragged |= ((pointer.0 - x) as f32 / size.0.max(1) as f32).abs()
                        > input.drag_threshold
                        || ((pointer.1 - y) as f32 / size.1.max(1) as f32).abs()
                            > input.drag_threshold;
                }
                panel_contains || self.pressed_button.is_some()
            }
            _ => panel_contains || self.pressed_button.is_some(),
        };
        input.handle_routed(event, consumed);
        consumed
    }

    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        if !self.started || self.elapsed == 0.0 {
            return;
        }
        let dx = self.panel_offset();
        for background in self.backgrounds {
            ctx.dl.fill_rect(
                GuiRect::new(
                    background.left + dx,
                    background.top,
                    background.right + dx,
                    background.bottom,
                ),
                0xff000000,
            );
        }
        for (rect, art) in &self.statics {
            crate::control::draw_button(
                ctx,
                ControlRect {
                    left: rect.left + dx,
                    top: rect.top,
                    right: rect.right + dx,
                    bottom: rect.bottom,
                },
                ButtonVisualState::Enabled,
                *art,
            );
        }
        for (button, &(text_id, colors)) in self.buttons.iter().zip(&self.labels) {
            let rect = self.translated_button_rect(button.rect);
            crate::control::draw_button(ctx, rect, button.state, button.art);
            // `ControlButton::render` (`controlgui.cpp:1457-1468`)
            let state_index = match button.state {
                ButtonVisualState::Enabled => 0,
                ButtonVisualState::Pressed => 1,
                ButtonVisualState::Disabled => 2,
                ButtonVisualState::Ambiguous => 3,
                ButtonVisualState::Hidden => continue,
            };
            if text_id != 0 {
                let label = ctx.assets.string(text_id).to_string();
                let height = crate::widgets::basic::font_height(ctx.assets, self.button_font);
                crate::widgets::basic::font_render(
                    ctx,
                    self.button_font,
                    &label,
                    rect.left,
                    (rect.top + rect.bottom) / 2 - height / 2 + 1,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    colors[state_index],
                    false,
                    2,
                );
            }
        }
        let header = ctx.assets.string(GAME_PAUSED);
        crate::widgets::basic::font_render(
            ctx,
            self.header_font,
            header,
            self.backgrounds[1].left + dx,
            self.backgrounds[1].top,
            self.backgrounds[1].width(),
            self.backgrounds[1].height(),
            0xff5c96c2,
            false,
            3,
        );
    }

    /// The question over the panel and the game (drawn last).
    pub fn render_dialog(&mut self, ctx: &mut RenderCtx<'_>) {
        // the dialog's animation dims the whole screen first (`logisticsdialog.cpp:119-120`)
        self.dialog.render(ctx);
    }

    /// The centre of a dialog button (`0` No, `1` Yes) in screen pixels, while the question is open.
    pub fn dialog_button_centre(&self, index: usize) -> Option<(f32, f32)> {
        if !self.dialog.is_open() {
            return None;
        }
        let b = self.dialog.screen.buttons.get(index)?;
        Some((
            b.obj.global_x() as f32 + b.obj.width() / 2.0,
            b.obj.global_y() as f32 + b.obj.height() / 2.0,
        ))
    }
}

fn animation_position(elapsed: f32) -> (f32, f32) {
    const KEYS: [(f32, f32); 8] = [
        (0.0, 820.0),
        (0.04, 820.0),
        (0.28, 661.0),
        (0.36, 678.0),
        (0.44, 661.0),
        (0.52, 666.0),
        (0.60, 661.0),
        (3.0, 661.0),
    ];
    for pair in KEYS.windows(2) {
        let [(t0, x0), (t1, x1)] = pair else {
            unreachable!()
        };
        if *t0 <= elapsed && elapsed < *t1 {
            let t = (elapsed - t0) / (t1 - t0);
            return (x0 + (x1 - x0) * t, *t1);
        }
    }
    (661.0, 3.0)
}
