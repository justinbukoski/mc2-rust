// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Composition of the supported in-mission HUD parts.
//!
//! Ported from `code/controlgui.cpp` and `code/missiongui.cpp`. This module owns composition
//! only: the landed tactical map is composed here; world selection/orders remain unavailable
//! until their lane lands. Panel geometry comes from the existing `.fit`-backed part layouts.

use crate::control::{ButtonArt, ButtonVisualState};
use crate::control::{ControlIntent, ControlPanel, ControlPanelInput, ControlRect};
use crate::draw::GuiRect;
use crate::draw::{Blend, Vertex};
use crate::font::FontRef;
use crate::hud_info::info::{InfoGeometry, InfoWindow};
use crate::hud_info::{HudOp, Metrics};
use crate::mission_keys::MissionCommand;
use crate::objectives::{ObjectivesPanel, PanelLayout};
use crate::pause_window::{PauseIntent, PauseWindow};
use crate::tacmap::{TacMap, TacMapInput};
use crate::unitbar::{layout::Layout, UnitBar, UnitBarIntent, UnitBarOutput};

/// Visible control tab. `ControlGui::switchTabs` cycles TacMap -> Vehicle -> Info for direction +1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MissionTab {
    #[default]
    TacMap,
    Info,
    Vehicle,
}

/// A screen-coordinate rectangle with the inclusive edges used by `ControlGui::inRegion`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HudRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl HudRect {
    pub const fn contains(self, x: i32, y: i32) -> bool {
        self.left <= x && x <= self.right && self.top <= y && y <= self.bottom
    }

    pub const fn from_gui(rect: GuiRect) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }

    pub const fn from_control(rect: ControlRect) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

/// Geometry of the composed, supported panels. Caller passes the same loaded FIT parts used to
/// render each panel, so hit testing cannot drift from their art placement.
#[derive(Clone, Debug, PartialEq)]
pub struct HudLayout {
    pub resolution: (u32, u32),
    pub control: Vec<HudRect>,
    pub unit_bar: Vec<HudRect>,
    pub info: HudRect,
    pub chat: HudRect,
    pub pause: Option<HudRect>,
    /// The first control `Rect`, `rectInfos[0]`, is the map's hit area in `isOverTacMap`.
    pub tac_map_region: Option<HudRect>,
    /// FIT-derived objectives drawing layout, supplied by the merged objectives component.
    pub objectives: Option<PanelLayout>,
}

impl HudLayout {
    pub fn from_parts(
        resolution: (u32, u32),
        control: &ControlPanel,
        unit_bar: &Layout,
        info: &InfoGeometry,
        chat: MissionChatGeometry,
        pause: Option<HudRect>,
    ) -> Self {
        let mut controls: Vec<_> = control
            .buttons
            .iter()
            .map(|b| HudRect::from_control(b.rect))
            .collect();
        controls.extend(
            control
                .vehicles
                .iter()
                .map(|b| HudRect::from_control(b.rect)),
        );
        controls.extend(
            control
                .statics
                .iter()
                .map(|(r, _)| HudRect::from_control(*r)),
        );
        controls.extend(control.rects.iter().map(|(r, _)| HudRect::from_control(*r)));
        // InfoWindow spans the name header through the scroll content and skill meters.
        let info_rect = HudRect {
            left: info
                .name_box
                .left
                .min(info.content_box.left)
                .min(info.scroll_bar.left),
            top: info.info_top,
            right: info
                .name_box
                .right
                .max(info.content_box.right)
                .max(info.scroll_bar.right),
            bottom: (info.info_top + info.info_height).max(info.content_box.bottom),
        };
        HudLayout {
            resolution,
            control: controls,
            unit_bar: unit_bar
                .slots
                .iter()
                .map(|s| HudRect::from_gui(s.selection))
                .collect(),
            info: info_rect,
            chat: chat.rect,
            pause,
            tac_map_region: control
                .rects
                .first()
                .map(|(r, _)| HudRect::from_control(*r)),
            objectives: None,
        }
    }

    /// Carries the real objectives component layout into the mission-level composition.
    pub fn with_objectives(mut self, panel: &ObjectivesPanel) -> Self {
        self.objectives = Some(panel.layout().clone());
        self
    }
}

/// The first surface in missiongui's mouse pass to claim the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitTarget {
    TacMap,
    Control,
    Info,
    UnitBar,
    Pause,
    World,
}

/// Plain frame data supplied by the app. Inputs are already translated into intents by the
/// independent component modules; this envelope orders them and carries mission-level toggles.
/// It contains no `mc2-sim` types.
#[derive(Clone, Debug, Default)]
pub struct HudInput {
    pub mouse: (i32, i32),
    /// A completed left click at the FIT button under `mouse`.
    #[cfg(test)]
    pub control_click: bool,
    pub control_intents: Vec<ControlIntent>,
    /// Click results for the control panel's non-order buttons (pause, tabs, objectives).
    /// The app maps the clicked FIT button id to these because the part emits no `ControlIntent`.
    pub panel_actions: Vec<MissionPanelAction>,
    pub unit_bar_intents: Vec<UnitBarIntent>,
    pub mission_commands: Vec<MissionCommand>,
    pub toggle_pause: bool,
    pub switch_tab: i8,
    pub toggle_gui: bool,
    /// Current live icon count from `ForceGroupBar::iconCount`; empty FIT slots do not claim input.
    pub unit_bar_icon_count: usize,
    /// Plain camera/map/unit presentation data consumed by the landed tactical-map part.
    pub tac_map_data: TacMapInput,
}

/// The production frame entry point. It owns the whole in-mission HUD pass: the current FIT
/// control input, tactical-map routing/update, selection results, and mission hotkeys.
pub struct MissionHudInput<'a, 'b> {
    pub state: HudInput,
    pub control: &'a mut ControlPanel,
    pub control_input: ControlPanelInput<'b>,
    pub tac_map: &'a mut TacMap,
    pub update_ctx: &'a mut crate::ctx::UpdateCtx<'b>,
    pub pause_window: &'a mut PauseWindow,
    /// When present, selection is updated from the current unit-bar snapshot in this same pass.
    pub unit_bar: Option<(
        &'a mut UnitBar,
        &'b crate::unitbar::UnitBarInput,
        &'a mut crate::textures::TextureStore,
    )>,
    pub events: &'b [mc2_platform::input::InputEvent],
    pub dt: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum HudIntent {
    Control(ControlIntent),
    Panel(MissionPanelAction),
    UnitBar(UnitBarIntent),
    Mission(MissionCommand),
    TacMapCameraTarget((f32, f32)),
    TacMapMoveOrder((f32, f32)),
    Pause(PauseIntent),
}

/// Non-simulation click actions handled by `MissionInterfaceManager` / `ControlGui`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionPanelAction {
    TogglePause,
    SelectTab(MissionTab),
    ToggleObjectives,
}

fn mission_panel_action(cxx_id: i32) -> Option<MissionPanelAction> {
    Some(match cxx_id {
        10 => MissionPanelAction::ToggleObjectives,
        11 => MissionPanelAction::TogglePause,
        12 => MissionPanelAction::SelectTab(MissionTab::TacMap),
        13 => MissionPanelAction::SelectTab(MissionTab::Info),
        14 => MissionPanelAction::SelectTab(MissionTab::Vehicle),
        _ => return None,
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudOutput {
    pub hit: Option<HitTarget>,
    pub intents: Vec<HudIntent>,
    pub paused: bool,
    pub tab: MissionTab,
    /// C++ `drawGUIOn`; false means no HUD panel draw is requested.
    pub gui_visible: bool,
    /// `ControlGui::renderObjectives`; the panel renderer remains owned by `objectives`.
    pub objectives_visible: bool,
    /// False when the objectives lane was not supplied to the composition.
    pub objectives_available: bool,
    pub missing_components: Vec<&'static str>,
    /// Mirrors `ControlGui::isOverTacMap`: the computed mouse-area local is not returned;
    /// only the TacMap tab's pressed state is observable (controlgui.cpp:1242-1255).
    pub over_tac_map: bool,
    /// Selection and world orders remain unavailable until the u3-select lane lands.
    pub unavailable: [&'static str; 1],
}

/// Borrowed panel state consumed by the real composed HUD renderer for one frame.
pub struct HudRenderParts<'a> {
    pub control: &'a mut ControlPanel,
    pub unit_bar: &'a mut UnitBar,
    pub unit_bar_output: &'a mut UnitBarOutput,
    pub info: &'a mut InfoWindow,
    /// Mission `ControlGui::renderChatText` overlay; this is distinct from the multiplayer chat
    /// widget used by logistics screens.
    pub mission_chat: &'a MissionChat,
    pub tac_map: &'a mut TacMap,
    pub tac_map_input: &'a TacMapInput,
    pub metrics: &'a crate::ctx::UiAssets,
    pub objectives: &'a mut ObjectivesPanel,
    pub objectives_view: &'a crate::objectives::ObjectivesView,
    pub pause_window: &'a mut PauseWindow,
}

/// One already-filtered mission status/chat line supplied by the application.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionChatLine {
    pub player_name: String,
    pub message: String,
    pub background_color: u32,
    pub text_color: u32,
}

/// The noninteractive `[ChatTextRect]` used by mission status/chat text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MissionChatGeometry {
    pub rect: HudRect,
}

impl MissionChatGeometry {
    /// `ControlGui::initChat` reads `[ChatTextRect]` from the resolution's button layout.
    pub fn from_fit(fit: &mut mc2_formats::ini::FitIni) -> Result<Self, String> {
        use crate::fit::FitExt;
        fit.seek_block("ChatTextRect")
            .map_err(|e| format!("ChatTextRect: {e}"))?;
        let value = |key: &str| {
            fit.long(key)
                .map(|v| v as i32)
                .ok_or_else(|| format!("ChatTextRect missing {key}"))
        };
        let x = value("xlocation")?;
        let y = value("yLocation")?;
        let width = value("Width")?;
        let height = value("Height")?;
        Ok(Self {
            rect: HudRect {
                left: x,
                top: y,
                right: x + width,
                bottom: y + height,
            },
        })
    }
}

/// The mission-only text overlay painted by `ControlGui::renderChatText`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionChat {
    pub rect: HudRect,
    pub name_font: FontRef,
    pub message_font: FontRef,
    pub lines: Vec<MissionChatLine>,
}

impl MissionChat {
    /// `ControlGui::initChat` reads `[ChatTextRect]` and initializes the two resource fonts.
    pub fn from_fit(
        fit: &mut mc2_formats::ini::FitIni,
        name_font: FontRef,
        message_font: FontRef,
    ) -> Result<Self, String> {
        let geometry = MissionChatGeometry::from_fit(fit)?;
        Ok(Self {
            rect: geometry.rect,
            name_font,
            message_font,
            lines: Vec::new(),
        })
    }

    fn ops(&self, metrics: &impl Metrics) -> Vec<HudOp> {
        let mut ops = Vec::new();
        let mut y = self.rect.top;
        for line in &self.lines {
            if y >= self.rect.bottom {
                break;
            }
            let name_width = metrics.text_width(self.name_font, &line.player_name)
                + metrics.text_width(self.name_font, " ")
                + 8;
            let rgb_sum = (line.background_color & 0xff)
                + ((line.background_color >> 8) & 0xff)
                + ((line.background_color >> 16) & 0xff);
            let name_text_color = if rgb_sum / 3 < 85 {
                0xffff_ffff
            } else {
                0xff00_0000
            };
            ops.push(HudOp::Fill {
                rect: GuiRect::new(
                    self.rect.left,
                    y,
                    self.rect.left + name_width,
                    y + metrics.line_height(self.name_font),
                ),
                argb: line.background_color,
            });
            ops.push(HudOp::Text {
                text: line.player_name.clone(),
                font: self.name_font,
                x: self.rect.left + 4,
                y: y as f32,
                w: name_width - 8,
                h: metrics.line_height(self.name_font),
                colour: name_text_color,
                align: 0,
            });
            ops.push(HudOp::Text {
                text: line.message.clone(),
                font: self.message_font,
                x: self.rect.left + name_width + 4,
                y: y as f32,
                w: (self.rect.right - self.rect.left - name_width - 4).max(0),
                h: metrics.line_height(self.message_font),
                colour: line.text_color,
                align: 0,
            });
            y += metrics.line_height(self.message_font) + 1;
        }
        ops
    }
}

/// Stateful mission-level behavior around the independent HUD parts.
#[derive(Clone, Debug)]
pub struct MissionHud {
    pub layout: HudLayout,
    pub paused: bool,
    pub paused_without_menu: bool,
    pub tab: MissionTab,
    pub gui_visible: bool,
    pub objectives_visible: bool,
    pause_objectives_before: Option<bool>,
    control_pressed: Option<ControlClickTarget>,
    control_released: Option<(ControlClickTarget, (i32, i32))>,
    control_press_origin: Option<(i32, i32)>,
    control_dragged: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlClickTarget {
    Button(i32),
    Vehicle(i32),
}

impl MissionHud {
    pub fn new(layout: HudLayout) -> Self {
        Self {
            layout,
            paused: false,
            paused_without_menu: false,
            tab: MissionTab::TacMap,
            gui_visible: true,
            objectives_visible: false,
            pause_objectives_before: None,
            control_pressed: None,
            control_released: None,
            control_press_origin: None,
            control_dragged: false,
        }
    }

    /// Resolves input using `missiongui.cpp`'s ControlGui region check first. Rect0 is then
    /// identified as its TacMap child; remaining info/pause regions and the live force-group bar
    /// follow, while the noninteractive mission-chat text leaves world input untouched.
    pub fn hit_test(&self, mouse: (i32, i32), unit_bar_icon_count: usize) -> HitTarget {
        let (x, y) = mouse;
        if self.layout.tac_map_region.is_some_and(|r| r.contains(x, y)) {
            return HitTarget::TacMap;
        }
        // a live icon sits on the slot grid (a control static), so it is tested before the panel's rects
        if self
            .layout
            .unit_bar
            .iter()
            .take(unit_bar_icon_count)
            .any(|r| r.contains(x, y))
        {
            return HitTarget::UnitBar;
        }
        if self.layout.control.iter().any(|r| r.contains(x, y)) {
            return HitTarget::Control;
        }
        if self.paused
            && !self.paused_without_menu
            && self.layout.pause.is_some_and(|r| r.contains(x, y))
        {
            return HitTarget::Pause;
        }
        if self.layout.info.contains(x, y) {
            return HitTarget::Info;
        }
        HitTarget::World
    }

    pub fn is_over_tac_map_at(&self, _mouse: (i32, i32)) -> bool {
        self.tab == MissionTab::TacMap
    }

    /// Routes clicks on the FIT-backed pause, tab and objectives buttons before composing the
    /// frame. C++ dispatches these IDs from `ControlGui::handleClick` during its control update.
    /// The independent control part continues to own order-button behavior.
    #[cfg(test)]
    fn update_with_control_panel(
        &mut self,
        mut input: HudInput,
        control: &ControlPanel,
    ) -> HudOutput {
        if input.control_click {
            if let Some(button) = control.buttons.iter().find(|button| {
                matches!(
                    button.state,
                    crate::control::ButtonVisualState::Enabled
                        | crate::control::ButtonVisualState::Pressed
                        | crate::control::ButtonVisualState::Ambiguous
                ) && button.rect.contains(input.mouse.0, input.mouse.1)
            }) {
                let action = match button.cxx_id {
                    10 => Some(MissionPanelAction::ToggleObjectives),
                    11 => Some(MissionPanelAction::TogglePause),
                    12 => Some(MissionPanelAction::SelectTab(MissionTab::TacMap)),
                    13 => Some(MissionPanelAction::SelectTab(MissionTab::Info)),
                    14 => Some(MissionPanelAction::SelectTab(MissionTab::Vehicle)),
                    _ => None,
                };
                if let Some(action) = action {
                    input.panel_actions.push(action);
                }
            }
        }
        self.update_state(input)
    }

    /// One production update pass for the composed HUD. The order follows the in-mission C++:
    /// pause window, control-panel FIT buttons, selected TacMap, unit-bar selection, then mission
    /// hotkeys. It routes the supplied events and advances the shared UI input frame. All results
    /// are plain intents for the application.
    pub fn update(&mut self, input: MissionHudInput<'_, '_>) -> HudOutput {
        let was_pause_menu_active = self.paused && !self.paused_without_menu;
        let left_release_event = input.events.iter().any(|event| {
            matches!(
                event,
                mc2_platform::input::InputEvent::Button {
                    button: mc2_platform::input::MouseButton::Left,
                    pressed: false,
                }
            )
        });
        let objective_hotkey = input
            .state
            .mission_commands
            .contains(&MissionCommand::ShowObjectives);

        self.control_released = None;
        for event in input.events {
            if self.paused
                && !self.paused_without_menu
                && input
                    .pause_window
                    .route_event(input.update_ctx.input, event)
            {
                continue;
            }
            if self.route_control_panel_event(input.control, input.update_ctx.input, event) {
                continue;
            }
            self.route_tac_map_event(
                input.tac_map,
                input.update_ctx.input,
                input.update_ctx.assets,
                event,
            );
        }

        input.update_ctx.input.begin_frame(input.dt);
        let mouse = (
            input.update_ctx.input.mouse_x(),
            input.update_ctx.input.mouse_y(),
        );
        let fit_target = self.control_released.take().filter(|_| left_release_event);
        let fit_click = fit_target.is_some();
        let click_mouse = fit_target.map_or(mouse, |(_, point)| point);

        let mut pause_intents = if self.paused && !self.paused_without_menu {
            let mut intents = input.pause_window.update(
                mouse,
                left_release_event && input.pause_window.has_completed_button_click(),
                input.dt,
            );
            if objective_hotkey {
                intents.insert(0, input.pause_window.toggle_objectives());
            }
            intents.extend(input.pause_window.update_dialog(input.update_ctx));
            intents
        } else {
            Vec::new()
        };
        for intent in &pause_intents {
            if *intent == PauseIntent::Resume {
                self.paused = false;
            }
        }
        if self.paused && !self.paused_without_menu && self.layout.objectives.is_some() {
            if pause_intents.contains(&PauseIntent::ToggleObjectives) {
                self.objectives_visible = input.pause_window.objectives_on();
                if !self.objectives_visible {
                    self.pause_objectives_before = Some(false);
                }
            } else {
                input
                    .pause_window
                    .set_objectives_on(self.objectives_visible);
            }
        }
        let mut panel_actions = input.state.panel_actions.clone();
        let mut control_input = input.control_input;
        control_input.mouse = click_mouse;
        control_input.paused = self.paused && !self.paused_without_menu;
        if fit_click {
            // ControlGui recomputes enabled states before it dispatches the click.
            control_input.click = None;
            control_input.vehicle_click = None;
            input.control.update(control_input);
        }
        let clicked_button = match fit_target.map(|(target, _)| target) {
            Some(ControlClickTarget::Button(cxx_id)) => {
                input.control.buttons.iter().find(|button| {
                    button.cxx_id == cxx_id
                        && button.rect.contains(click_mouse.0, click_mouse.1)
                        && matches!(
                            button.state,
                            ButtonVisualState::Enabled
                                | ButtonVisualState::Pressed
                                | ButtonVisualState::Ambiguous
                        )
                })
            }
            _ => None,
        };
        if let Some(button) = clicked_button {
            if control_input.click.is_none() {
                control_input.click = button.id;
            }
            if let Some(action) = mission_panel_action(button.cxx_id) {
                panel_actions.push(action);
            }
        }
        if let Some(ControlClickTarget::Vehicle(id)) = fit_target.map(|(target, _)| target) {
            if input.control.vehicle_tab {
                if let Some(vehicle) = input.control.vehicles.iter().find(|button| button.id == id)
                {
                    control_input.vehicle_click = if vehicle.id == 107 {
                        Some(crate::control::VehicleAction::Stop)
                    } else {
                        vehicle.kind.map(crate::control::VehicleAction::Build)
                    };
                }
            }
        }
        let control_intents = input.control.update(control_input);

        let map_active = self.tab == MissionTab::TacMap;
        if map_active {
            input
                .tac_map
                .update(input.update_ctx, &input.state.tac_map_data);
        }
        let mut map_intents = Vec::new();
        if map_active {
            if input.state.tac_map_data.placing_airstrike {
                if let Some(world) = input.tac_map.last_move_order {
                    map_intents.push(HudIntent::TacMapMoveOrder(world));
                }
            } else if let Some(world) = input.tac_map.last_move_order {
                map_intents.push(HudIntent::TacMapMoveOrder(world));
            } else if let Some(world) = input.tac_map.last_click {
                map_intents.push(HudIntent::TacMapCameraTarget(world));
            }
        }

        let mut selection_intents = Vec::new();
        let mut selection_count = None;
        if let Some((unit_bar, unit_bar_input, textures)) = input.unit_bar {
            let result = unit_bar.update(unit_bar_input, textures);
            selection_intents.extend(result.intents);
            selection_count = Some(unit_bar.icon_count());
        }

        let mut state = input.state;
        state.mouse = mouse;
        state.control_intents = control_intents;
        state.panel_actions = panel_actions;
        if let Some(icon_count) = selection_count {
            state.unit_bar_icon_count = icon_count;
        }
        state.unit_bar_intents.extend(selection_intents);
        let map_insert_at = state.control_intents.len() + state.panel_actions.len();
        let mut output = self.update_state(state);
        output
            .intents
            .splice(map_insert_at..map_insert_at, map_intents);

        let pause_menu_active = self.paused && !self.paused_without_menu;
        if !was_pause_menu_active && pause_menu_active {
            self.pause_objectives_before = Some(self.objectives_visible);
            input.pause_window.begin(self.objectives_visible);
        } else if was_pause_menu_active && !pause_menu_active {
            input.pause_window.end();
            if let Some(was_visible) = self.pause_objectives_before.take() {
                self.objectives_visible = was_visible;
                input.pause_window.set_objectives_on(was_visible);
            }
        }
        if pause_menu_active && self.layout.objectives.is_some() {
            if pause_intents.contains(&PauseIntent::ToggleObjectives) {
                self.objectives_visible = input.pause_window.objectives_on();
                if !self.objectives_visible {
                    self.pause_objectives_before = Some(false);
                }
            } else {
                input
                    .pause_window
                    .set_objectives_on(self.objectives_visible);
            }
        }
        output.paused = self.paused;
        output.objectives_visible = self.objectives_visible;
        output.missing_components.clear();
        output
            .intents
            .splice(0..0, pause_intents.drain(..).map(HudIntent::Pause));
        input.update_ctx.input.end_frame();
        output
    }

    /// Applies mission state transitions for tests and the composed production entry point.
    fn update_state(&mut self, input: HudInput) -> HudOutput {
        if input.toggle_pause {
            if self.paused_without_menu {
                self.paused_without_menu = false;
            } else {
                self.paused = !self.paused;
            }
        }
        if input.switch_tab != 0 {
            let n = match self.tab {
                MissionTab::TacMap => 0_i8,
                MissionTab::Info => 1,
                MissionTab::Vehicle => 2,
            };
            // The C++ ids are TacMap=0, Info=1, Vehicle=2, but the positive direction
            // follows TACMAP -> VEHICLE -> INFO -> TACMAP (switchTabs lines 2824-2844).
            self.tab = match (n - input.switch_tab.signum()).rem_euclid(3) {
                0 => MissionTab::TacMap,
                1 => MissionTab::Info,
                _ => MissionTab::Vehicle,
            };
        }
        if input.toggle_gui {
            self.gui_visible = !self.gui_visible;
        }
        for action in &input.panel_actions {
            match action {
                MissionPanelAction::TogglePause => self.toggle_pause(),
                MissionPanelAction::SelectTab(tab) => self.tab = *tab,
                MissionPanelAction::ToggleObjectives if self.layout.objectives.is_some() => {
                    self.objectives_visible = !self.objectives_visible;
                }
                MissionPanelAction::ToggleObjectives => {}
            }
        }
        for command in &input.mission_commands {
            match command {
                MissionCommand::TogglePause => self.toggle_pause(),
                MissionCommand::SwitchTab => self.switch_tabs(1),
                MissionCommand::ReverseSwitchTab => self.switch_tabs(-1),
                MissionCommand::ShowObjectives if self.layout.objectives.is_some() => {
                    self.objectives_visible = !self.objectives_visible;
                }
                // This debug-only handler is omitted from release command tables, but remains
                // representable by `mission_keys` when built from a non-FINAL C++ table.
                MissionCommand::Unavailable("toggleGUI") => self.gui_visible = !self.gui_visible,
                _ => {}
            }
        }
        let hit = self.hit_test(input.mouse, input.unit_bar_icon_count);
        // controlgui.update invokes its unit bar and chat update before missiongui dispatches keys.
        let mut intents = Vec::with_capacity(
            input.control_intents.len()
                + input.panel_actions.len()
                + input.unit_bar_intents.len()
                + input.mission_commands.len(),
        );
        intents.extend(input.control_intents.into_iter().map(HudIntent::Control));
        intents.extend(input.panel_actions.into_iter().map(HudIntent::Panel));
        intents.extend(input.unit_bar_intents.into_iter().map(HudIntent::UnitBar));
        intents.extend(input.mission_commands.into_iter().map(HudIntent::Mission));
        HudOutput {
            hit: Some(hit),
            intents,
            paused: self.paused,
            tab: self.tab,
            gui_visible: self.gui_visible,
            objectives_visible: self.objectives_visible,
            objectives_available: self.layout.objectives.is_some(),
            missing_components: Vec::new(),
            over_tac_map: self.is_over_tac_map_at(input.mouse),
            unavailable: ["selection and world orders"],
        }
    }

    /// `MissionHudInput.events` are routed through the active component; this map snapshot stays
    /// plain data.
    #[cfg(test)]
    fn update_with_tac_map(
        &mut self,
        input: HudInput,
        tac_map: &mut TacMap,
        ctx: &mut crate::ctx::UpdateCtx<'_>,
    ) -> HudOutput {
        // ControlGui::update updates the selected map before MissionInterfaceManager dispatches
        // its keyboard commands, so use the currently active tab and insert map intents between
        // control-panel and unit-bar intents.
        let map_active = self.tab == MissionTab::TacMap;
        let map_insert_at = input.control_intents.len() + input.panel_actions.len();
        let map_data = &input.tac_map_data;
        let mut map_intents = Vec::new();
        if map_active {
            tac_map.update(ctx, map_data);
            if map_data.placing_airstrike {
                if let Some(world) = tac_map.last_move_order {
                    map_intents.push(HudIntent::TacMapMoveOrder(world));
                }
            } else if let Some(world) = tac_map.last_click {
                map_intents.push(HudIntent::TacMapCameraTarget(world));
            }
        }
        let mut output = self.update_state(input);
        output
            .intents
            .splice(map_insert_at..map_insert_at, map_intents);
        output
    }

    /// Routes events through the tac-map's real FIT rectangle while its tab is active. The
    /// application remains responsible for feeding the shared `UiInput`'s frame clock.
    fn route_tac_map_event(
        &self,
        tac_map: &mut TacMap,
        input: &mut crate::input::UiInput,
        assets: &crate::ctx::UiAssets,
        event: &mc2_platform::input::InputEvent,
    ) -> bool {
        if self.tab == MissionTab::TacMap {
            tac_map.screen.route(input, assets, event)
        } else {
            input.handle_routed(event, false);
            false
        }
    }

    fn route_control_panel_event(
        &mut self,
        control: &ControlPanel,
        input: &mut crate::input::UiInput,
        event: &mc2_platform::input::InputEvent,
    ) -> bool {
        let pointer = match event {
            mc2_platform::input::InputEvent::CursorMoved { x, y } => {
                (x.round() as i32, y.round() as i32)
            }
            _ => input.pending_pointer(),
        };
        let button = control
            .buttons
            .iter()
            .find(|button| {
                button.rect.contains(pointer.0, pointer.1)
                    && matches!(
                        button.state,
                        ButtonVisualState::Enabled
                            | ButtonVisualState::Pressed
                            | ButtonVisualState::Ambiguous
                    )
            })
            .map(|button| ControlClickTarget::Button(button.cxx_id));
        let vehicle = (control.vehicle_tab)
            .then(|| {
                control
                    .vehicles
                    .iter()
                    .find(|button| button.rect.contains(pointer.0, pointer.1))
            })
            .flatten()
            .map(|button| ControlClickTarget::Vehicle(button.id));
        let target = button.or(vehicle);
        let taken = target.is_some();
        match event {
            mc2_platform::input::InputEvent::CursorMoved { .. } => {
                if let Some((x, y)) = self.control_press_origin {
                    let size = input.size();
                    self.control_dragged |= ((pointer.0 - x) as f32 / size.0.max(1) as f32).abs()
                        > input.drag_threshold
                        || ((pointer.1 - y) as f32 / size.1.max(1) as f32).abs()
                            > input.drag_threshold;
                }
            }
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            } => {
                self.control_pressed = target;
                self.control_press_origin = target.map(|_| pointer);
                self.control_dragged = false;
            }
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            } => {
                self.control_released = self
                    .control_pressed
                    .take()
                    .filter(|pressed| Some(*pressed) == target && !self.control_dragged)
                    .map(|pressed| (pressed, pointer));
                self.control_press_origin = None;
                self.control_dragged = false;
            }
            _ => {}
        }
        input.handle_routed(event, taken);
        taken
    }

    fn toggle_pause(&mut self) {
        if self.paused_without_menu {
            self.paused_without_menu = false;
        } else {
            self.paused = !self.paused;
        }
    }

    fn switch_tabs(&mut self, direction: i8) {
        self.tab = match (self.tab as i8 - direction.signum()).rem_euclid(3) {
            0 => MissionTab::TacMap,
            1 => MissionTab::Info,
            _ => MissionTab::Vehicle,
        };
    }

    /// Draws the supported panels through their production renderers in ControlGui order.
    /// World selection/order rendering remains absent until the u3-select lane lands.
    pub fn render(
        &self,
        ctx: &mut crate::ctx::RenderCtx<'_>,
        parts: &mut HudRenderParts<'_>,
    ) -> Result<(), crate::objectives::PlayerStatusUnavailable> {
        if !self.gui_visible {
            return Ok(());
        }
        self.render_control_backdrop(ctx, parts.control);
        if self.tab == MissionTab::TacMap {
            if let Some(rect) = self.layout.tac_map_region {
                let rect = GuiRect::new(rect.left, rect.top, rect.right, rect.bottom);
                if parts.tac_map.rect() != rect {
                    parts.tac_map.set_pos(rect);
                }
            }
            parts.tac_map.render(ctx, parts.tac_map_input);
        } else if self.tab == MissionTab::Info {
            crate::hud_info::draw_ops(ctx, &parts.info.render(parts.metrics));
        }
        parts.unit_bar.render(ctx, parts.unit_bar_output);
        self.render_control_buttons(ctx, parts.control);
        if parts.objectives.is_open() != self.objectives_visible {
            parts.objectives.start(
                self.objectives_visible,
                parts.objectives_view.render_status_info,
            );
        }
        if self.objectives_visible || parts.objectives_view.render_status_info {
            parts.objectives.render(ctx, parts.objectives_view)?;
        }
        crate::hud_info::draw_ops(ctx, &parts.mission_chat.ops(parts.metrics));
        if self.paused && !self.paused_without_menu {
            parts.pause_window.render(ctx);
            parts.pause_window.render_dialog(ctx);
        }
        Ok(())
    }

    /// Draw the ControlGui FIT rectangles and statics before the selected tab content.
    fn render_control_backdrop(&self, ctx: &mut crate::ctx::RenderCtx<'_>, control: &ControlPanel) {
        let mut display = control.clone();
        display.buttons.clear();
        display.vehicles.clear();
        display.vehicle_tab = false;
        display.render(ctx);
    }

    /// Draw control and vehicle buttons after the selected tab and force-group bar, matching
    /// `ControlGui::render` (controlgui.cpp:324-357). The part retains ownership of its own state.
    fn render_control_buttons(&self, ctx: &mut crate::ctx::RenderCtx<'_>, control: &ControlPanel) {
        let mut display = control.clone();
        display.rects.clear();
        display.statics.clear();
        display.vehicle_tab = self.tab == MissionTab::Vehicle;
        for button in &mut display.buttons {
            button.state = composed_button_state(
                button.cxx_id,
                button.state,
                self.paused && !self.paused_without_menu,
                self.objectives_visible,
                self.tab,
            );
        }
        display.render(ctx);
        for button in display.buttons.iter().filter(|button| button.id.is_none()) {
            let tint = match control.button_color {
                Some((id, color)) if id == button.cxx_id => color,
                _ => 0xffff_ffff,
            };
            draw_composed_control_button(ctx, button.rect, button.state, button.art, tint);
        }
    }

    /// Updates the composed frame output with intents emitted during the real render pass.
    /// Unit-bar death completion occurs while rendering (`mechicon.cpp:1493-1501`), after the
    /// update-side output was built.
    pub fn render_frame(
        &self,
        ctx: &mut crate::ctx::RenderCtx<'_>,
        parts: &mut HudRenderParts<'_>,
        mut output: HudOutput,
    ) -> Result<HudOutput, crate::objectives::PlayerStatusUnavailable> {
        let before = parts.unit_bar_output.intents.len();
        self.render(ctx, parts)?;
        output.intents.extend(
            parts.unit_bar_output.intents[before..]
                .iter()
                .cloned()
                .map(HudIntent::UnitBar),
        );
        Ok(output)
    }
}

fn composed_button_state(
    cxx_id: i32,
    original: ButtonVisualState,
    paused: bool,
    objectives_visible: bool,
    tab: MissionTab,
) -> ButtonVisualState {
    let selected = matches!(
        (cxx_id, tab),
        (12, MissionTab::TacMap) | (13, MissionTab::Info) | (14, MissionTab::Vehicle)
    );
    if cxx_id == 10 {
        if objectives_visible {
            ButtonVisualState::Pressed
        } else {
            ButtonVisualState::Enabled
        }
    } else if cxx_id == 11 {
        if paused {
            ButtonVisualState::Pressed
        } else {
            ButtonVisualState::Enabled
        }
    } else if (12..=14).contains(&cxx_id) {
        if selected {
            ButtonVisualState::Pressed
        } else {
            ButtonVisualState::Enabled
        }
    } else {
        original
    }
}

/// Same FIT art/state mapping as `control::draw_button`, applied here to command buttons without
/// a `ControlId` so the independent control component's public behavior stays unchanged.
/// Per-channel product of two ARGB colours (a vertex tint over the flat fallback fill).
fn modulate_argb(color: u32, tint: u32) -> u32 {
    (0..4).fold(0, |out, i| {
        let shift = i * 8;
        let c = (color >> shift) & 0xff;
        let t = (tint >> shift) & 0xff;
        out | ((c * t / 255) << shift)
    })
}

fn draw_composed_control_button(
    ctx: &mut crate::ctx::RenderCtx<'_>,
    rect: crate::control::ControlRect,
    state: ButtonVisualState,
    art: ButtonArt,
    tint: u32,
) {
    let state_index = match state {
        ButtonVisualState::Enabled => 0,
        ButtonVisualState::Pressed => 1,
        ButtonVisualState::Disabled => 2,
        ButtonVisualState::Ambiguous => 3,
        ButtonVisualState::Hidden => return,
    };
    if let Some(texture) = art.texture {
        let (u, v) = art.uv[state_index];
        let bias_x = 0.1 / art.texture_size.0.max(1.0);
        let bias_y = 0.1 / art.texture_size.1.max(1.0);
        let (u0, u1) = (u + bias_x, u + art.uv_size.0 + bias_x);
        let (v0, v1) = (v + bias_x, v + art.uv_size.1 + bias_y);
        if art.rotated {
            ctx.dl.quad(
                Some(texture),
                Blend::Alpha,
                [
                    Vertex {
                        x: rect.left as f32,
                        y: rect.top as f32,
                        u: u1,
                        v: v0,
                        argb: tint,
                    },
                    Vertex {
                        x: rect.left as f32,
                        y: rect.bottom as f32,
                        u: u0,
                        v: v0,
                        argb: tint,
                    },
                    Vertex {
                        x: rect.right as f32,
                        y: rect.bottom as f32,
                        u: u0,
                        v: v1,
                        argb: tint,
                    },
                    Vertex {
                        x: rect.right as f32,
                        y: rect.top as f32,
                        u: u1,
                        v: v1,
                        argb: tint,
                    },
                ],
            );
        } else {
            ctx.dl.rect_quad(
                Some(texture),
                Blend::Alpha,
                (
                    rect.left as f32,
                    rect.top as f32,
                    rect.right as f32,
                    rect.bottom as f32,
                ),
                (u0, v0, u1, v1),
                tint,
            );
        }
    } else {
        let color = match state {
            ButtonVisualState::Pressed => 0xff2478a8,
            ButtonVisualState::Disabled => 0xff45515a,
            ButtonVisualState::Ambiguous => 0xff75612d,
            _ => 0xff005392,
        };
        ctx.dl.fill_rect(
            GuiRect::new(rect.left, rect.top, rect.right, rect.bottom),
            modulate_argb(color, tint),
        );
    }
    if state == ButtonVisualState::Pressed {
        ctx.dl.outline_rect(
            GuiRect::new(rect.left, rect.top, rect.right - 1, rect.bottom - 1),
            0xffff_ffff,
            0xffff_ffff,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::ControlPanel;
    use crate::ctx::{Loader, UiAssets};
    use crate::fit::FitExt;
    use crate::hud_info::info::InfoGeometry;
    use crate::objectives::ObjectivesPanel;
    use crate::unitbar::layout::Layout;

    fn actual_parts(
        width: u32,
        height: u32,
    ) -> Option<(
        ControlPanel,
        Layout,
        InfoGeometry,
        MissionChatGeometry,
        HudRect,
        ObjectivesPanel,
    )> {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED {}: MC2_DATA is unset", module_path!());
            return None;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        let control =
            ControlPanel::load(&mut loader, width, height).expect("load control panel FIT");
        let name = match width {
            640 => "buttonlayout640",
            800 => "buttonlayout800",
            1024 => "buttonlayout1024",
            1280 => "buttonlayout1280",
            1600 => "buttonlayout1600",
            1920 => "buttonlayout1920",
            _ => return None,
        };
        let mut fit = loader.art_fit(name).expect("load unit bar FIT");
        let bar = Layout::from_fit(&mut fit).expect("read unit bar FIT");
        let mut fit = loader.art_fit(name).expect("load info FIT");
        let info = InfoGeometry::from_fit(&mut fit, width as i32).expect("read info FIT");
        let mut chat_fit = loader.art_fit(name).expect("load mission chat FIT");
        let chat = MissionChatGeometry::from_fit(&mut chat_fit).expect("read ChatTextRect");
        let mut pause_fit = loader.art_fit(name).expect("load pause FIT");
        let offset_x = if pause_fit.seek_block("HiresOffsets").is_ok() {
            pause_fit.long_or("xOffset", 0) as i32
        } else {
            0
        };
        pause_fit
            .seek_block("PauseBackRect")
            .expect("PauseBackRect block");
        let pause = HudRect {
            left: pause_fit.long_or("left", 0) as i32 + offset_x,
            top: pause_fit.long_or("top", 0) as i32,
            right: pause_fit.long_or("right", 0) as i32 + offset_x,
            bottom: pause_fit.long_or("bottom", 0) as i32,
        };
        let mut objective_fit = loader.art_fit(name).expect("load objectives FIT");
        let objectives = ObjectivesPanel::from_fit(&mut loader, &mut objective_fit)
            .expect("read objectives FIT");
        // PauseWindow::init adds hiResOffsetX to PauseBackRect but leaves top/bottom untouched
        // (pausewindow.cpp:336-347).
        Some((control, bar, info, chat, pause, objectives))
    }

    fn rasterize_draw_list(
        list: &crate::draw::DrawList,
        assets: &UiAssets,
        background: [u8; 4],
    ) -> Vec<u8> {
        let (width, height) = (list.screen.0 as u32, list.screen.1 as u32);
        let mut pixels = vec![0; (width * height * 4) as usize];
        for px in pixels.chunks_exact_mut(4) {
            px.copy_from_slice(&background);
        }
        for batch in &list.batches {
            for index in batch.first_quad..batch.first_quad + batch.quad_count {
                let quad = &list.vertices[index as usize * 4..index as usize * 4 + 4];
                let left = quad.iter().map(|v| v.x).fold(f32::INFINITY, f32::min);
                let top = quad.iter().map(|v| v.y).fold(f32::INFINITY, f32::min);
                let right = quad.iter().map(|v| v.x).fold(f32::NEG_INFINITY, f32::max);
                let bottom = quad.iter().map(|v| v.y).fold(f32::NEG_INFINITY, f32::max);
                if right <= left || bottom <= top {
                    continue;
                }
                let x0 = left.floor().max(0.0) as u32;
                let y0 = top.floor().max(0.0) as u32;
                let x1 = right.ceil().min(width as f32) as u32;
                let y1 = bottom.ceil().min(height as f32) as u32;
                let image = batch.texture.and_then(|id| assets.textures.get(id));
                for y in y0..y1 {
                    for x in x0..x1 {
                        if batch.clip.is_some_and(|clip| {
                            x < clip.left.max(0) as u32
                                || y < clip.top.max(0) as u32
                                || x >= clip.right.max(0) as u32
                                || y >= clip.bottom.max(0) as u32
                        }) {
                            continue;
                        }
                        let fx = ((x as f32 + 0.5 - left) / (right - left)).clamp(0.0, 1.0);
                        let fy = ((y as f32 + 0.5 - top) / (bottom - top)).clamp(0.0, 1.0);
                        let [tl, bl, br, tr] = quad else {
                            unreachable!()
                        };
                        let interp = |a: f32, b: f32, c: f32, d: f32| {
                            (a * (1.0 - fx) + d * fx) * (1.0 - fy) + (b * (1.0 - fx) + c * fx) * fy
                        };
                        let u = interp(tl.u, bl.u, br.u, tr.u);
                        let v = interp(tl.v, bl.v, br.v, tr.v);
                        let color = [
                            interp(
                                (tl.argb >> 16 & 255) as f32,
                                (bl.argb >> 16 & 255) as f32,
                                (br.argb >> 16 & 255) as f32,
                                (tr.argb >> 16 & 255) as f32,
                            ),
                            interp(
                                (tl.argb >> 8 & 255) as f32,
                                (bl.argb >> 8 & 255) as f32,
                                (br.argb >> 8 & 255) as f32,
                                (tr.argb >> 8 & 255) as f32,
                            ),
                            interp(
                                (tl.argb & 255) as f32,
                                (bl.argb & 255) as f32,
                                (br.argb & 255) as f32,
                                (tr.argb & 255) as f32,
                            ),
                            interp(
                                (tl.argb >> 24) as f32,
                                (bl.argb >> 24) as f32,
                                (br.argb >> 24) as f32,
                                (tr.argb >> 24) as f32,
                            ),
                        ];
                        let mut texel = [255.0; 4];
                        if let Some(image) = image {
                            let tx = (u * image.width as f32)
                                .floor()
                                .clamp(0.0, image.width.saturating_sub(1) as f32)
                                as u32;
                            let ty = (v * image.height as f32)
                                .floor()
                                .clamp(0.0, image.height.saturating_sub(1) as f32)
                                as u32;
                            let start = ((ty * image.width + tx) * 4) as usize;
                            for (dst, &src) in texel.iter_mut().zip(&image.rgba[start..start + 4]) {
                                *dst = src as f32;
                            }
                        }
                        let start = ((y * width + x) * 4) as usize;
                        let alpha = texel[3] * color[3] / (255.0 * 255.0);
                        for channel in 0..3 {
                            let source = texel[channel] * color[channel] / 255.0;
                            let dest = pixels[start + channel] as f32;
                            pixels[start + channel] = match batch.blend {
                                Blend::Alpha => {
                                    (source * alpha + dest * (1.0 - alpha)).round() as u8
                                }
                                Blend::Additive => (source + dest).min(255.0).round() as u8,
                                Blend::Opaque => source.round() as u8,
                            };
                        }
                    }
                }
            }
        }
        pixels
    }

    #[test]
    fn real_fit_panel_rectangles_cover_supported_resolutions() {
        let rect = |left, top, right, bottom| HudRect {
            left,
            top,
            right,
            bottom,
        };
        let expected = [
            (
                640,
                480,
                HudRect {
                    left: 554,
                    top: 373,
                    right: 593,
                    bottom: 398,
                },
                HudRect {
                    left: 2,
                    top: 348,
                    right: 132,
                    bottom: 478,
                },
            ),
            (
                800,
                600,
                HudRect {
                    left: 691,
                    top: 465,
                    right: 741,
                    bottom: 496,
                },
                HudRect {
                    left: 3,
                    top: 434,
                    right: 165,
                    bottom: 597,
                },
            ),
            (
                1024,
                768,
                HudRect {
                    left: 884,
                    top: 595,
                    right: 948,
                    bottom: 635,
                },
                HudRect {
                    left: 4,
                    top: 556,
                    right: 212,
                    bottom: 764,
                },
            ),
            (
                1280,
                1024,
                HudRect {
                    left: 1012,
                    top: 851,
                    right: 1076,
                    bottom: 891,
                },
                HudRect {
                    left: 132,
                    top: 812,
                    right: 340,
                    bottom: 1020,
                },
            ),
            (
                1600,
                1200,
                HudRect {
                    left: 1172,
                    top: 1027,
                    right: 1236,
                    bottom: 1067,
                },
                HudRect {
                    left: 292,
                    top: 988,
                    right: 500,
                    bottom: 1196,
                },
            ),
            (
                1920,
                1200,
                HudRect {
                    left: 1172,
                    top: 1027,
                    right: 1236,
                    bottom: 1067,
                },
                HudRect {
                    left: 292,
                    top: 988,
                    right: 500,
                    bottom: 1196,
                },
            ),
        ];
        for (w, h, button0, rect0) in expected {
            let Some((control, bar, info, chat, pause, objectives)) = actual_parts(w, h) else {
                continue;
            };
            let layout = HudLayout::from_parts((w, h), &control, &bar, &info, chat, Some(pause))
                .with_objectives(&objectives);
            let obj = layout.objectives.as_ref().expect("objectives panel layout");
            assert_eq!(layout.resolution, (w, h));
            // `[Objectives]`/`[PrimaryObjHeader]` values are hand-read from each shipped
            // buttonlayout*.fit; the panel uses these in RenderObjectives (controlgui.cpp:603-752).
            assert_eq!(
                (
                    obj.left,
                    obj.top,
                    obj.header_left,
                    obj.item_skip,
                    obj.secondary_skip,
                    obj.total_right,
                ),
                (45, 87, 30, 15, 25, 475),
                "{w}x{h} objectives anchors"
            );
            let edge_y = if w == 640 { 71 } else { 76 };
            assert_eq!(
                obj.edges
                    .iter()
                    .map(|e| (e.x as i32, e.y as i32, e.width as i32, e.height as i32))
                    .collect::<Vec<_>>(),
                vec![(0, edge_y, 14, 118), (0, edge_y + 118, 14, 118)],
                "{w}x{h} objective edge bars"
            );
            assert_eq!(layout.unit_bar.len(), 16);
            let expected_bar_slot0 = match w {
                640 => rect(143, 389, 191, 432),
                800 => rect(178, 486, 238, 540),
                1024 => rect(228, 623, 305, 692),
                1280 => rect(356, 879, 433, 948),
                1600 | 1920 => rect(516, 1055, 593, 1124),
                _ => unreachable!(),
            };
            // MechIcon0 selection bounds are read from the same FIT and HiresOffsets as
            // ForceGroupIcon::init (mechicon.cpp:197-296).
            assert_eq!(layout.unit_bar[0], expected_bar_slot0, "{w}x{h} MechIcon0");
            // InfoWindow and mission ChatTextRect come from infowindow.cpp:102-294 and
            // controlgui.cpp:2571-2584 respectively.
            let expected_info = match w {
                640 => rect(5, 348, 128, 478),
                800 => rect(8, 434, 159, 597),
                _ => rect(9, 556, 206, 764),
            };
            assert_eq!(layout.info, expected_info, "{w}x{h} InfoWindow");
            // ChatTextRect is read by ControlGui::initChat (controlgui.cpp:2571-2584);
            // these values are hand-read from each resolution's buttonlayout*.fit.
            let expected_chat = rect(102, 1, 476, 12);
            assert_eq!(chat.rect, expected_chat, "{w}x{h} ChatTextRect FIT values");
            assert_eq!(layout.chat, expected_chat, "{w}x{h} mission chat geometry");
            assert!(layout
                .control
                .iter()
                .all(|r| r.left <= r.right && r.top <= r.bottom));
            // Expected top command button and control background rectangles are hand-derived
            // from each buttonlayout*.fit; C++ applies HiresOffsets to buttons and rects in
            // controlgui.cpp:2277-2302 and :2621-2630, and vertical resolution delta in :2671-2689.
            assert_eq!(layout.control[0], button0, "{w}x{h} Button0");
            assert_eq!(
                *layout.control.last().unwrap(),
                rect0,
                "{w}x{h} final region"
            );
            let expected_tac_map_region = match w {
                640 => rect(3, 347, 132, 477),
                800 => rect(4, 435, 165, 596),
                1024 => rect(5, 557, 211, 763),
                1280 => rect(133, 813, 339, 1019),
                1600 | 1920 => rect(293, 989, 499, 1195),
                _ => unreachable!(),
            };
            // Rect0 in each buttonlayout*.fit is the tac-map rectangle; inRegion claims it first
            // (controlgui.cpp:1278-1284) and isOverTacMap's press-bit result is tested below.
            assert!(
                layout.control.contains(&expected_tac_map_region),
                "{w}x{h} Rect0"
            );
            assert_eq!(layout.tac_map_region, Some(expected_tac_map_region));
            // Hand-read PauseBackRect plus HiresOffsets in each buttonlayout*.fit; the source
            // applies only xOffset to this background (pausewindow.cpp:336-347).
            let expected_pause = match w {
                640 => rect(501, 91, 635, 304),
                800 => rect(661, 91, 795, 304),
                1024 => rect(885, 91, 1019, 304),
                1280 => rect(1013, 91, 1147, 304),
                1600 | 1920 => rect(1173, 91, 1307, 304),
                _ => unreachable!(),
            };
            assert_eq!(layout.pause, Some(expected_pause), "{w}x{h} PauseBackRect");
        }
    }

    #[test]
    fn real_fit_routing_uses_cpp_region_order_and_live_unit_count() {
        let Some((mut control, bar, info, chat, pause, objectives)) = actual_parts(1024, 768)
        else {
            eprintln!("SKIPPED: MC2_DATA is unset");
            return;
        };
        let layout = HudLayout::from_parts((1024, 768), &control, &bar, &info, chat, Some(pause))
            .with_objectives(&objectives);
        let mut hud = MissionHud::new(layout);
        // C++ missiongui.cpp:722 checks ControlGui::inRegion before all world/panel routing;
        // the tac-map Rect0 masks the overlapping InfoWindow region at this point.
        assert_eq!(hud.hit_test((10, 560), 1), HitTarget::TacMap);
        // Mission `renderChatText` is a noninteractive overlay; the unrelated chat-window FIT
        // rectangle does not claim world input (missiongui.cpp:722-757).
        assert_eq!(hud.hit_test((400, 480), 1), HitTarget::World);
        // the slot grid (`Static10`..`13`) is a control static, and `ControlGui::inRegion` tests every
        // static (controlgui.cpp:1272-1276): empty slots are the panel's, not the world's; a live icon wins
        assert_eq!(hud.hit_test((228, 623), 1), HitTarget::UnitBar);
        assert_eq!(hud.hit_test((350, 623), 1), HitTarget::Control);
        assert_eq!(hud.hit_test((350, 623), 16), HitTarget::UnitBar);
        assert_eq!(hud.hit_test((500, 400), 16), HitTarget::World);
        // `isOverTacMap` computes the mouse-in-Rect0 local but returns only the pressed
        // TacMap tab state (controlgui.cpp:1242-1255).
        assert!(hud.is_over_tac_map_at((10, 560)));
        hud.tab = MissionTab::Info;
        assert!(!hud.is_over_tac_map_at((10, 560)));
        assert!(!hud.is_over_tac_map_at((500, 400)));
        hud.tab = MissionTab::TacMap;
        let map_hover = hud.update_state(HudInput {
            mouse: (10, 560),
            switch_tab: 1,
            ..Default::default()
        });
        assert_eq!(map_hover.tab, MissionTab::Vehicle);
        assert!(!map_hover.over_tac_map);
        let outside_map = hud.update_state(HudInput {
            mouse: (500, 400),
            ..Default::default()
        });
        assert!(!outside_map.over_tac_map);
        // ControlGui::inRegion checks regular regions before the pause window (controlgui.cpp:1278-1310).
        hud.paused = true;
        assert_eq!(hud.hit_test((900, 100), 16), HitTarget::Pause);
        hud.paused_without_menu = true;
        assert_eq!(hud.hit_test((900, 100), 16), HitTarget::World);
        hud.paused_without_menu = false;
        hud.paused = false;

        // Feed the production control-panel update's output into the composition, as the app
        // adapter does; `ControlGui::handleClick` precedes its state refresh (controlgui.cpp:934-979).
        let selected = [crate::control::UnitCapabilities {
            can_jump: true,
            ..Default::default()
        }];
        let control_intents = control.update(crate::control::ControlPanelInput {
            selected_units: &selected,
            mouse: (0, 0),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: Default::default(),
            click: Some(crate::control::ControlId::Run),
            vehicle_click: None,
        });
        assert_eq!(control_intents, [ControlIntent::SetRun(true)]);
        let output = hud.update_state(HudInput {
            control_intents,
            mission_commands: vec![MissionCommand::TogglePause],
            ..Default::default()
        });
        assert!(output.paused);
        assert_eq!(
            output.intents[0],
            HudIntent::Control(ControlIntent::SetRun(true))
        );
        assert_eq!(
            output.intents[1],
            HudIntent::Mission(MissionCommand::TogglePause)
        );
    }

    #[test]
    fn routing_pause_tabs_toggle_and_intent_order_match_cpp() {
        let rect = |l, t, r, b| HudRect {
            left: l,
            top: t,
            right: r,
            bottom: b,
        };
        let layout = HudLayout {
            resolution: (1024, 768),
            control: vec![rect(0, 500, 100, 700)],
            unit_bar: vec![rect(200, 700, 260, 740)],
            info: rect(800, 400, 1000, 650),
            chat: rect(10, 500, 300, 590),
            pause: Some(rect(100, 500, 300, 590)),
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        };
        let mut hud = MissionHud::new(layout);
        // missiongui.cpp:722 tests ControlGui::inRegion before it derives bGui/world targeting;
        // missiongui.cpp:2323 excludes forceGroupBar.inRegion from its own world-click path.
        assert_eq!(hud.hit_test((50, 550), 1), HitTarget::Control);
        // rectInfos[0] is in ControlGui::inRegion; isOverTacMap itself returns only the pressed
        // tab state, despite computing a pointer-area local (controlgui.cpp:1242-1255).
        assert!(hud.is_over_tac_map_at((50, 550)));
        assert_eq!(hud.hit_test((900, 450), 1), HitTarget::Info);
        assert_eq!(hud.hit_test((150, 550), 1), HitTarget::World);
        assert_eq!(hud.hit_test((220, 720), 1), HitTarget::UnitBar);
        assert_eq!(hud.hit_test((500, 400), 1), HitTarget::World);
        let out = hud.update_state(HudInput {
            toggle_pause: true,
            switch_tab: 1,
            toggle_gui: true,
            mouse: (150, 550),
            ..Default::default()
        });
        assert!(out.paused);
        assert!(!out.gui_visible);
        assert!(!out.over_tac_map);
        // controlgui.cpp:2802-2810 begins/ends the pause window with SAVE_COMMAND;
        // missiongui.cpp:4610-4617 routes togglePause through those calls.
        assert_eq!(hud.hit_test((150, 550), 1), HitTarget::Pause);
        assert_eq!(
            hud.update_state(HudInput {
                switch_tab: 1,
                mouse: (50, 650),
                ..Default::default()
            })
            .tab,
            MissionTab::Info
        );
        // controlgui.cpp:2813-2844 cycles TACMAP_TAB, INFO_TAB, VEHICLE_TAB and wraps.
        assert_eq!(
            hud.update_state(HudInput {
                switch_tab: 1,
                toggle_gui: true,
                mouse: (50, 650),
                ..Default::default()
            })
            .tab,
            MissionTab::TacMap
        );
        // missiongui.cpp:2716-2722 toggles drawGUIOn.
        assert!(hud.gui_visible);
    }

    #[test]
    fn pause_without_menu_toggle_keeps_paused_state() {
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: None,
        });
        hud.paused = true;
        hud.paused_without_menu = true;
        let out = hud.update_state(HudInput {
            toggle_pause: true,
            ..Default::default()
        });
        assert!(out.paused);
        assert!(!hud.paused_without_menu);
    }

    #[test]
    fn component_intents_are_returned_in_cpp_control_bar_hotkey_order() {
        let layout = HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        };
        let mut hud = MissionHud::new(layout);
        let out = hud.update_state(HudInput {
            mouse: (0, 0),
            control_intents: vec![ControlIntent::Stop],
            panel_actions: vec![MissionPanelAction::TogglePause],
            unit_bar_intents: vec![UnitBarIntent::SelectForceGroup(2)],
            mission_commands: vec![MissionCommand::ShowObjectives],
            ..Default::default()
        });
        assert_eq!(
            out.intents,
            vec![
                HudIntent::Control(ControlIntent::Stop),
                HudIntent::Panel(MissionPanelAction::TogglePause),
                HudIntent::UnitBar(UnitBarIntent::SelectForceGroup(2)),
                HudIntent::Mission(MissionCommand::ShowObjectives),
            ]
        );
    }

    #[test]
    fn actual_hotkey_commands_drive_pause_tabs_gui_and_objectives() {
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        // `togglePause` dispatches to beginPause/endPause in missiongui.cpp:4596-4617.
        assert!(
            hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::TogglePause],
                ..Default::default()
            })
            .paused
        );
        assert!(
            !hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::TogglePause],
                ..Default::default()
            })
            .paused
        );
        // `switchTabs(direction)` in controlgui.cpp:2813-2844: +1 is TacMap -> Vehicle -> Info.
        assert_eq!(
            hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::SwitchTab],
                ..Default::default()
            })
            .tab,
            MissionTab::Vehicle
        );
        assert_eq!(
            hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::SwitchTab],
                ..Default::default()
            })
            .tab,
            MissionTab::Info
        );
        assert_eq!(
            hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::ReverseSwitchTab],
                ..Default::default()
            })
            .tab,
            MissionTab::Vehicle
        );
        // `toggleGUI` is the debug command at missiongui.cpp:2716-2722.
        assert!(
            !hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::Unavailable("toggleGUI")],
                ..Default::default()
            })
            .gui_visible
        );
        assert!(
            hud.update_state(HudInput {
                mission_commands: vec![MissionCommand::ShowObjectives],
                ..Default::default()
            })
            .objectives_visible
        );
    }

    #[test]
    fn mission_state_does_not_report_the_composed_pause_window_missing() {
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: None,
        });
        let out = hud.update_state(HudInput {
            mission_commands: vec![MissionCommand::TogglePause],
            ..Default::default()
        });
        assert!(out.paused);
        assert!(out.missing_components.is_empty());
    }

    #[test]
    fn pause_and_objectives_buttons_use_their_active_fit_cells() {
        assert_eq!(
            composed_button_state(
                11,
                ButtonVisualState::Enabled,
                true,
                false,
                MissionTab::TacMap
            ),
            ButtonVisualState::Pressed,
            "ControlGui::beginPause presses SAVE_COMMAND (controlgui.cpp:2802-2805)"
        );
        assert_eq!(
            composed_button_state(
                11,
                ButtonVisualState::Enabled,
                false,
                false,
                MissionTab::TacMap
            ),
            ButtonVisualState::Enabled,
            "ControlGui::endPause releases SAVE_COMMAND (controlgui.cpp:2807-2810)"
        );
        assert_eq!(
            composed_button_state(10, ButtonVisualState::Enabled, false, true, MissionTab::TacMap),
            ButtonVisualState::Pressed,
            "ControlGui update synchronizes OBJECTIVES_COMMAND with renderObjectives (controlgui.cpp:1228)"
        );
    }

    #[test]
    fn production_update_orders_real_fit_click_before_armed_tacmap_intent() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED MissionHud production update: MC2_DATA is unset");
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets {
            screen: (1024, 768),
            ..UiAssets::default()
        };
        let (mut control, mut pause_window, objectives) = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            let control = ControlPanel::load(&mut loader, 1024, 768).expect("control FIT");
            let pause_window = PauseWindow::load(&mut loader, 1024, 768).expect("pause FIT");
            let mut fit = loader.art_fit("buttonlayout1024").expect("objectives FIT");
            let objectives =
                ObjectivesPanel::from_fit(&mut loader, &mut fit).expect("objectives component");
            (control, pause_window, objectives)
        };
        let pause_button = control
            .buttons
            .iter()
            .find(|button| button.cxx_id == 11)
            .expect("real pause FIT button");
        let control_mouse = (
            (pause_button.rect.left + pause_button.rect.right) / 2,
            (pause_button.rect.top + pause_button.rect.bottom) / 2,
        );
        let map_rect = HudRect::from_control(control.rects[0].0);
        let map_mouse = (
            (map_rect.left + map_rect.right) / 2,
            (map_rect.top + map_rect.bottom) / 2,
        );
        let mut tac_map = TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100));
        tac_map.set_pos(GuiRect::new(
            map_rect.left,
            map_rect.top,
            map_rect.right,
            map_rect.bottom,
        ));
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: control
                .buttons
                .iter()
                .map(|button| HudRect::from_control(button.rect))
                .collect(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: Some(HudRect::from_gui(pause_window.panel_rect())),
            tac_map_region: Some(map_rect),
            objectives: Some(objectives.layout().clone()),
        });
        let events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: control_mouse.0 as f64,
                y: control_mouse.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
            mc2_platform::input::InputEvent::CursorMoved {
                x: map_mouse.0 as f64,
                y: map_mouse.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
        ];
        let selected = [];
        let mut input = crate::input::UiInput::new(1024, 768);
        let mut ui_output = crate::ctx::UiOutput::default();
        let output = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput {
                    tac_map_data: TacMapInput {
                        placing_airstrike: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: control_mouse,
                    paused: false,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &events,
                dt: 0.016,
            })
        };
        assert!(output.paused);
        assert!(pause_window.is_started());
        assert_eq!(
            output.intents,
            [
                HudIntent::Panel(MissionPanelAction::TogglePause),
                HudIntent::TacMapMoveOrder((0.0, 0.0)),
            ],
            "single update handles the FIT pause click and map input; left airstrike click emits no camera intent"
        );

        let release = [mc2_platform::input::InputEvent::Button {
            button: mc2_platform::input::MouseButton::Left,
            pressed: false,
        }];
        {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput::default(),
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: map_mouse,
                    paused: true,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &release,
                dt: 0.5,
            });
        }
        let ordinary_events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: map_mouse.0 as f64,
                y: map_mouse.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
        ];
        let ordinary = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput::default(),
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: map_mouse,
                    paused: true,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &ordinary_events,
                dt: 0.016,
            })
        };
        assert_eq!(
            ordinary.intents,
            [HudIntent::TacMapCameraTarget((0.0, 0.0))],
            "the FIT-center ordinary click targets world origin and emits only the camera intent"
        );

        let mut right_map = TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100));
        right_map.set_pos(GuiRect::new(
            map_rect.left,
            map_rect.top,
            map_rect.right,
            map_rect.bottom,
        ));
        right_map.left_right_profile = true;
        let right_events = [
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Right,
                pressed: true,
            },
        ];
        let right_order = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput::default(),
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: map_mouse,
                    paused: true,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut right_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &right_events,
                dt: 0.016,
            })
        };
        assert_eq!(
            right_order.intents,
            [HudIntent::TacMapMoveOrder((0.0, 0.0))],
            "the FIT-center right click targets world origin and emits only a move order"
        );

        let hotkey = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput {
                    mission_commands: vec![MissionCommand::ShowObjectives],
                    ..Default::default()
                },
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: map_mouse,
                    paused: true,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &[],
                dt: 0.016,
            })
        };
        assert!(hotkey
            .intents
            .contains(&HudIntent::Pause(PauseIntent::ToggleObjectives)));
        assert!(hotkey.objectives_visible);

        // Finish the panel's slide, then route an actual pointer and button press through the
        // pause hit region to the RETURN FIT button.
        pause_window.update((0, 0), false, 3.0);
        {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput::default(),
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: map_mouse,
                    paused: true,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &release,
                dt: 0.5,
            });
        }
        let return_rect = pause_window.button_rect(6).expect("RETURN FIT button");
        let return_point = (
            (return_rect.left + return_rect.right) / 2,
            (return_rect.top + return_rect.bottom) / 2,
        );
        let resume_events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: return_point.0 as f64,
                y: return_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let resumed = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.0,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput::default(),
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: return_point,
                    paused: true,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &resume_events,
                dt: 0.0,
            })
        };
        assert!(!resumed.paused);
        assert!(resumed
            .intents
            .contains(&HudIntent::Pause(PauseIntent::Resume)));
        assert!(
            !resumed.objectives_visible,
            "pause-opened objectives close on resume when initially closed"
        );
        macro_rules! update_composed {
            ($state:expr, $events:expr, $point:expr, $dt:expr) => {{
                let events = $events;
                let point = $point;
                let dt = $dt;
                let paused = hud.paused && !hud.paused_without_menu;
                let mut update_ctx = crate::ctx::UpdateCtx {
                    input: &mut input,
                    assets: &assets,
                    dt,
                    out: &mut ui_output,
                };
                hud.update(MissionHudInput {
                    state: $state,
                    control: &mut control,
                    control_input: ControlPanelInput {
                        selected_units: &selected,
                        mouse: point,
                        paused,
                        vtol_in_flight: false,
                        resource_points: 0,
                        vehicle_availability: Default::default(),
                        click: None,
                        vehicle_click: None,
                    },
                    tac_map: &mut tac_map,
                    update_ctx: &mut update_ctx,
                    pause_window: &mut pause_window,
                    unit_bar: None,
                    events,
                    dt,
                })
            }};
        }
        // Starting open, close objectives in Pause and resume with the hotkey. The changed
        // visibility is the value C++ restores when returning from Pause.
        let open_objectives = update_composed!(
            HudInput {
                mission_commands: vec![MissionCommand::ShowObjectives],
                ..Default::default()
            },
            &[],
            map_mouse,
            0.016
        );
        assert!(open_objectives.objectives_visible);
        let enter_pause = update_composed!(
            HudInput {
                mission_commands: vec![MissionCommand::TogglePause],
                ..Default::default()
            },
            &[],
            map_mouse,
            0.016
        );
        assert!(enter_pause.paused);
        let close_objectives = update_composed!(
            HudInput {
                mission_commands: vec![MissionCommand::ShowObjectives],
                ..Default::default()
            },
            &[],
            map_mouse,
            0.016
        );
        assert!(!close_objectives.objectives_visible);
        assert!(close_objectives
            .intents
            .contains(&HudIntent::Pause(PauseIntent::ToggleObjectives)));
        let resume_hotkey = update_composed!(
            HudInput {
                mission_commands: vec![MissionCommand::TogglePause],
                ..Default::default()
            },
            &[],
            map_mouse,
            0.016
        );
        assert!(!resume_hotkey.paused);
        assert!(!resume_hotkey.objectives_visible);

        // Repeat from initially open objectives, close them with the Pause OBJECTIVES FIT
        // button, then select RETURN. Both public update paths must preserve the closure.
        update_composed!(
            HudInput {
                mission_commands: vec![MissionCommand::ShowObjectives],
                ..Default::default()
            },
            &[],
            map_mouse,
            0.016
        );
        update_composed!(
            HudInput {
                mission_commands: vec![MissionCommand::TogglePause],
                ..Default::default()
            },
            &[],
            map_mouse,
            0.016
        );
        pause_window.update((0, 0), false, 3.0);
        let objectives_point = {
            let rect = pause_window
                .button_rect_for_id(0)
                .expect("OBJECTIVES FIT button");
            ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2)
        };
        let objective_click = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: objectives_point.0 as f64,
                y: objectives_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let objective_click_result = update_composed!(
            HudInput::default(),
            &objective_click,
            objectives_point,
            0.016
        );
        assert!(!objective_click_result.objectives_visible);
        let return_rect = pause_window
            .button_rect_for_id(6)
            .expect("RETURN FIT button");
        let return_point = (
            (return_rect.left + return_rect.right) / 2,
            (return_rect.top + return_rect.bottom) / 2,
        );
        let return_click = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: return_point.0 as f64,
                y: return_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let return_result =
            update_composed!(HudInput::default(), &return_click, return_point, 0.016);
        assert!(!return_result.paused);
        assert!(!return_result.objectives_visible);

        let paused_by_hotkey = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput {
                    mission_commands: vec![MissionCommand::TogglePause],
                    ..Default::default()
                },
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: return_point,
                    paused: false,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &[],
                dt: 0.016,
            })
        };
        assert!(paused_by_hotkey.paused);
        assert!(pause_window.is_started());
        pause_window.update((0, 0), false, 3.0);
        let action_cases = [
            (0, vec![PauseIntent::ToggleObjectives]),
            (1, vec![PauseIntent::Save]),
            (2, vec![PauseIntent::Load]),
            (3, vec![PauseIntent::Options, PauseIntent::Resume]),
            // RESTART and EXIT ask first (`bPromptToAbort`, `bPromptToQuit`): no action yet
            (4, vec![]),
            (5, vec![]),
            (6, vec![PauseIntent::Resume]),
        ];
        for (id, expected) in action_cases {
            let rect = pause_window
                .button_rect_for_id(id)
                .expect("pause button ID in loaded FIT");
            let point = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
            assert_eq!(
                pause_window.update(point, true, 0.0),
                expected,
                "button ID {id}"
            );
            assert_eq!(
                pause_window.prompt_open(),
                id == 4 || id == 5,
                "button {id}"
            );
            if pause_window.prompt_open() {
                // the question is answered elsewhere; closing the panel drops it
                pause_window.end();
                pause_window.begin(false);
                pause_window.update((0, 0), false, 3.0);
            }
        }
    }

    #[test]
    fn production_update_ignores_pause_window_input_when_paused_without_menu() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED MissionHud no-menu pause routing: MC2_DATA is unset");
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets {
            screen: (1024, 768),
            ..UiAssets::default()
        };
        let (mut control, mut pause_window) = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            (
                ControlPanel::load(&mut loader, 1024, 768).expect("control FIT"),
                PauseWindow::load(&mut loader, 1024, 768).expect("pause FIT"),
            )
        };
        let return_rect = pause_window
            .button_rect_for_id(6)
            .expect("RETURN FIT button");
        let point = (
            (return_rect.left + return_rect.right) / 2,
            (return_rect.top + return_rect.bottom) / 2,
        );
        let events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: point.0 as f64,
                y: point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: Vec::new(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: Some(HudRect::from_gui(pause_window.panel_rect())),
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        hud.paused = true;
        hud.paused_without_menu = true;
        let mut tac_map = TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100));
        let selected = [];
        let mut input = crate::input::UiInput::new(1024, 768);
        let mut ui_output = crate::ctx::UiOutput::default();
        let result = {
            let mut update_ctx = crate::ctx::UpdateCtx {
                input: &mut input,
                assets: &assets,
                dt: 0.016,
                out: &mut ui_output,
            };
            hud.update(MissionHudInput {
                state: HudInput::default(),
                control: &mut control,
                control_input: ControlPanelInput {
                    selected_units: &selected,
                    mouse: point,
                    paused: false,
                    vtol_in_flight: false,
                    resource_points: 0,
                    vehicle_availability: Default::default(),
                    click: None,
                    vehicle_click: None,
                },
                tac_map: &mut tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut pause_window,
                unit_bar: None,
                events: &events,
                dt: 0.016,
            })
        };
        assert!(result.paused);
        assert!(!pause_window.is_started());
        assert!(
            result
                .intents
                .iter()
                .all(|intent| !matches!(intent, HudIntent::Pause(_))),
            "no pause-window input produces pause intents without the menu"
        );
    }

    #[test]
    fn tac_map_routes_a_real_click_and_returns_the_camera_target_intent() {
        let rect = HudRect {
            left: 10,
            top: 10,
            right: 130,
            bottom: 110,
        };
        let mut hud = MissionHud::new(HudLayout {
            resolution: (800, 600),
            control: vec![rect],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: Some(rect),
            objectives: None,
        });
        let mut tac_map = TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100));
        tac_map.set_pos(GuiRect::new(rect.left, rect.top, rect.right, rect.bottom));
        tac_map.set_terrain_size(128, 128);
        let assets = UiAssets::default();
        let mut ui_input = crate::input::UiInput::new(800, 600);
        for _ in 0..10 {
            ui_input.begin_frame(1.0 / 30.0);
            ui_input.end_frame();
        }
        let move_event = mc2_platform::input::InputEvent::CursorMoved { x: 70.0, y: 60.0 };
        assert!(hud.route_tac_map_event(&mut tac_map, &mut ui_input, &assets, &move_event));
        ui_input.begin_frame(1.0 / 30.0);
        ui_input.end_frame();
        let click = mc2_platform::input::InputEvent::Button {
            button: mc2_platform::input::MouseButton::Left,
            pressed: true,
        };
        assert!(hud.route_tac_map_event(&mut tac_map, &mut ui_input, &assets, &click));
        ui_input.begin_frame(1.0 / 30.0);
        let mut ui_output = crate::ctx::UiOutput::default();
        let mut update_ctx = crate::ctx::UpdateCtx {
            input: &mut ui_input,
            assets: &assets,
            dt: 1.0 / 30.0,
            out: &mut ui_output,
        };
        let output = hud.update_with_tac_map(
            HudInput {
                mouse: (70, 60),
                mission_commands: vec![MissionCommand::SwitchTab],
                ..Default::default()
            },
            &mut tac_map,
            &mut update_ctx,
        );
        assert_eq!(output.hit, Some(HitTarget::TacMap));
        assert_eq!(output.tab, MissionTab::Vehicle);
        assert_eq!(
            output.intents,
            [
                HudIntent::TacMapCameraTarget((0.0, 0.0)),
                HudIntent::Mission(MissionCommand::SwitchTab),
            ],
            "ControlGui updates the map before missiongui dispatches tab hotkeys"
        );
        ui_input.end_frame();
    }

    #[test]
    fn callout_flash_tints_the_composed_target_button_only() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED {}: MC2_DATA is unset", module_path!());
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets::default();
        let mut control = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            ControlPanel::load(&mut loader, 1024, 768).expect("load control panel FIT")
        };
        let hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: Vec::new(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        let tint_of = |control: &ControlPanel, id: i32| {
            let button = control.buttons.iter().find(|b| b.cxx_id == id).unwrap();
            let mut list = crate::draw::DrawList::new(1024, 768);
            {
                let mut ctx = crate::ctx::RenderCtx {
                    dl: &mut list,
                    assets: &assets,
                    dt: 0.0,
                };
                hud.render_control_buttons(&mut ctx, control);
            }
            list.vertices
                .chunks_exact(4)
                .find(|quad| {
                    quad.iter()
                        .any(|v| v.x == button.rect.left as f32 && v.y == button.rect.top as f32)
                })
                .map(|quad| quad[0].argb)
        };
        assert_eq!(tint_of(&control, 11), Some(0xffff_ffff));
        control.button_color = Some((11, 0xff7f_7f7f));
        assert_eq!(tint_of(&control, 11), Some(0xff7f_7f7f), "target flashes");
        assert_eq!(tint_of(&control, 10), Some(0xffff_ffff), "others do not");
    }

    #[test]
    fn composition_draws_every_control_button_and_marks_the_active_tab() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED {}: MC2_DATA is unset", module_path!());
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets::default();
        let control = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            ControlPanel::load(&mut loader, 1024, 768).expect("load control panel FIT")
        };
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: Vec::new(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        let info_tab = control
            .buttons
            .iter()
            .find(|button| button.cxx_id == 13)
            .expect("Info tab from buttonlayout1024.fit");
        let click = (
            (info_tab.rect.left + info_tab.rect.right) / 2,
            (info_tab.rect.top + info_tab.rect.bottom) / 2,
        );
        hud.update_with_control_panel(
            HudInput {
                mouse: click,
                control_click: true,
                ..Default::default()
            },
            &control,
        );
        assert_eq!(hud.tab, MissionTab::Info);
        for id in [11, 10] {
            let button = control.buttons.iter().find(|b| b.cxx_id == id).unwrap();
            let mouse = (
                (button.rect.left + button.rect.right) / 2,
                (button.rect.top + button.rect.bottom) / 2,
            );
            hud.update_with_control_panel(
                HudInput {
                    mouse,
                    control_click: true,
                    ..Default::default()
                },
                &control,
            );
        }
        assert!(hud.paused && hud.objectives_visible);
        let mut list = crate::draw::DrawList::new(1024, 768);
        {
            let mut ctx = crate::ctx::RenderCtx {
                dl: &mut list,
                assets: &assets,
                dt: 0.0,
            };
            hud.render_control_backdrop(&mut ctx, &control);
            hud.render_control_buttons(&mut ctx, &control);
        }

        for id in [10, 11, 12, 13, 14] {
            let button = control
                .buttons
                .iter()
                .find(|button| button.cxx_id == id)
                .expect("control button from buttonlayout1024.fit");
            assert!(
                list.vertices.chunks_exact(4).any(|quad| {
                    quad.iter()
                        .any(|v| v.x == button.rect.left as f32 && v.y == button.rect.top as f32)
                }),
                "C++ command button {id} has a composed draw"
            );
        }
        for (id, state_index) in [(10, 1), (11, 1), (12, 0), (13, 1), (14, 0)] {
            let button = control.buttons.iter().find(|b| b.cxx_id == id).unwrap();
            let quad = list
                .vertices
                .chunks_exact(4)
                .find(|quad| {
                    quad.iter()
                        .any(|v| v.x == button.rect.left as f32 && v.y == button.rect.top as f32)
                })
                .expect("composed button quad");
            let (u, v) = button.art.uv[state_index];
            let bias_x = 0.1 / button.art.texture_size.0.max(1.0);
            let bias_y = 0.1 / button.art.texture_size.1.max(1.0);
            let min_u = quad.iter().map(|v| v.u).fold(f32::INFINITY, f32::min);
            let max_u = quad.iter().map(|v| v.u).fold(f32::NEG_INFINITY, f32::max);
            let min_v = quad.iter().map(|v| v.v).fold(f32::INFINITY, f32::min);
            let max_v = quad.iter().map(|v| v.v).fold(f32::NEG_INFINITY, f32::max);
            assert_eq!(min_u, u + bias_x, "button {id} selected UV cell");
            assert_eq!(
                max_u,
                u + button.art.uv_size.0 + bias_x,
                "button {id} selected UV cell"
            );
            assert_eq!(min_v, v + bias_y, "button {id} selected UV cell");
            assert_eq!(
                max_v,
                v + button.art.uv_size.1 + bias_y,
                "button {id} selected UV cell"
            );
        }
        for id in [12, 13, 14] {
            let button = control
                .buttons
                .iter()
                .find(|button| button.cxx_id == id)
                .expect("tab button from buttonlayout1024.fit");
            assert_eq!(
                button.state,
                crate::control::ButtonVisualState::Enabled,
                "composed rendering leaves part-owned tab button {id} unchanged"
            );
        }
        hud.update_state(HudInput {
            mission_commands: vec![MissionCommand::TogglePause, MissionCommand::ShowObjectives],
            ..Default::default()
        });
        assert!(!hud.paused && !hud.objectives_visible);
        let mut inactive = crate::draw::DrawList::new(1024, 768);
        {
            let mut ctx = crate::ctx::RenderCtx {
                dl: &mut inactive,
                assets: &assets,
                dt: 0.0,
            };
            hud.render_control_backdrop(&mut ctx, &control);
            hud.render_control_buttons(&mut ctx, &control);
        }
        for id in [10, 11] {
            let button = control.buttons.iter().find(|b| b.cxx_id == id).unwrap();
            let quad = inactive
                .vertices
                .chunks_exact(4)
                .find(|quad| {
                    quad.iter()
                        .any(|v| v.x == button.rect.left as f32 && v.y == button.rect.top as f32)
                })
                .expect("composed inactive button quad");
            let (u, v) = button.art.uv[0];
            assert!(quad.iter().any(|vertex| vertex.u
                == u + 0.1 / button.art.texture_size.0.max(1.0)
                && vertex.v == v + 0.1 / button.art.texture_size.1.max(1.0)));
        }
        assert_eq!(
            [12, 13, 14].map(|id| composed_button_state(
                id,
                crate::control::ButtonVisualState::Enabled,
                false,
                false,
                MissionTab::TacMap,
            )),
            [
                crate::control::ButtonVisualState::Pressed,
                crate::control::ButtonVisualState::Enabled,
                crate::control::ButtonVisualState::Enabled,
            ],
            "missiongui's selected tab drives the composed tab art"
        );
    }

    #[test]
    fn control_panel_pause_tab_and_objective_clicks_are_composed_in_cpp_order() {
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        let out = hud.update_state(HudInput {
            panel_actions: vec![
                MissionPanelAction::TogglePause,
                MissionPanelAction::SelectTab(MissionTab::Info),
                MissionPanelAction::ToggleObjectives,
            ],
            control_intents: vec![ControlIntent::Stop],
            unit_bar_intents: vec![UnitBarIntent::SelectForceGroup(1)],
            mission_commands: vec![MissionCommand::SwitchTab],
            ..Default::default()
        });
        // SAVE_COMMAND->togglePause (controlgui.cpp:1557); tab buttons and objectives button
        // (controlgui.cpp:1600-1617) run during the control update before mission hotkeys.
        assert!(out.paused);
        assert_eq!(out.tab, MissionTab::TacMap); // Info click, then switchTabs(+1), wraps to TacMap.
        assert!(out.objectives_visible);
        assert_eq!(
            out.intents,
            vec![
                HudIntent::Control(ControlIntent::Stop),
                HudIntent::Panel(MissionPanelAction::TogglePause),
                HudIntent::Panel(MissionPanelAction::SelectTab(MissionTab::Info)),
                HudIntent::Panel(MissionPanelAction::ToggleObjectives),
                HudIntent::UnitBar(UnitBarIntent::SelectForceGroup(1)),
                HudIntent::Mission(MissionCommand::SwitchTab),
            ]
        );
    }

    #[test]
    fn loaded_fit_pause_tab_and_objectives_clicks_produce_mission_actions() {
        let Some((mut control, bar, info, chat, pause, objectives)) = actual_parts(1024, 768)
        else {
            eprintln!("SKIPPED: MC2_DATA is unset");
            return;
        };
        let layout = HudLayout::from_parts((1024, 768), &control, &bar, &info, chat, Some(pause))
            .with_objectives(&objectives);
        let mut hud = MissionHud::new(layout);
        for (cxx_id, action) in [
            (11, MissionPanelAction::TogglePause),
            (13, MissionPanelAction::SelectTab(MissionTab::Info)),
            (14, MissionPanelAction::SelectTab(MissionTab::Vehicle)),
            (10, MissionPanelAction::ToggleObjectives),
        ] {
            let button = control
                .buttons
                .iter()
                .find(|button| button.cxx_id == cxx_id)
                .expect("button from buttonlayout1024.fit");
            let mouse = (
                (button.rect.left + button.rect.right) / 2,
                (button.rect.top + button.rect.bottom) / 2,
            );
            let out = hud.update_with_control_panel(
                HudInput {
                    mouse,
                    control_click: true,
                    ..Default::default()
                },
                &control,
            );
            assert_eq!(out.hit, Some(HitTarget::Control));
            assert_eq!(out.intents, [HudIntent::Panel(action)]);
            assert!(match action {
                MissionPanelAction::TogglePause => out.paused,
                MissionPanelAction::SelectTab(tab) => out.tab == tab,
                MissionPanelAction::ToggleObjectives => out.objectives_visible,
            });
        }

        let pause_button = control
            .buttons
            .iter_mut()
            .find(|button| button.cxx_id == 11)
            .expect("pause button from buttonlayout1024.fit");
        pause_button.state = crate::control::ButtonVisualState::Disabled;
        let mouse = (
            (pause_button.rect.left + pause_button.rect.right) / 2,
            (pause_button.rect.top + pause_button.rect.bottom) / 2,
        );
        assert!(hud
            .update_with_control_panel(
                HudInput {
                    mouse,
                    control_click: true,
                    ..Default::default()
                },
                &control,
            )
            .intents
            .is_empty());
    }

    #[test]
    fn objectives_command_reports_unavailable_without_a_panel() {
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: Vec::new(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: None,
        });
        let out = hud.update_state(HudInput {
            mission_commands: vec![MissionCommand::ShowObjectives],
            ..Default::default()
        });
        assert!(!out.objectives_visible);
        assert!(!out.objectives_available);
        assert_eq!(out.unavailable, ["selection and world orders"]);
    }

    #[test]
    fn only_live_unit_bar_icons_claim_their_fit_slots() {
        let r = |l, t, rr, b| HudRect {
            left: l,
            top: t,
            right: rr,
            bottom: b,
        };
        let hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![r(200, 600, 250, 650), r(260, 600, 310, 650)],
            info: r(0, 0, 0, 0),
            chat: r(0, 0, 0, 0),
            pause: None,
            tac_map_region: None,
            objectives: None,
        });
        assert_eq!(hud.hit_test((275, 625), 0), HitTarget::World);
        assert_eq!(hud.hit_test((225, 625), 1), HitTarget::UnitBar);
        assert_eq!(hud.hit_test((275, 625), 1), HitTarget::World);
        assert_eq!(hud.hit_test((275, 625), 16), HitTarget::UnitBar);
    }

    #[test]
    fn headless_composition_png_uses_mission_hud_renderers_when_requested() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("MC2_DATA not set; skipping the composed HUD render");
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets {
            screen: (1024, 768),
            ..UiAssets::default()
        };
        let (mut control, mut unit_bar, mut info, _chat_geometry, mut objectives, mut pause_window) = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            let control = ControlPanel::load(&mut loader, 1024, 768).expect("control FIT");
            let pause_window = PauseWindow::load(&mut loader, 1024, 768).expect("pause FIT");
            let mut fit = loader.art_fit("buttonlayout1024").expect("unit bar FIT");
            let layout = Layout::from_fit(&mut fit).expect("unit bar layout");
            let art = crate::unitbar::Art::load(&mut loader, &mut fit).expect("unit bar art");
            let unit_bar = UnitBar::new(layout, art);
            let mut fit = loader.art_fit("buttonlayout1024").expect("info FIT");
            let info_geometry = InfoGeometry::from_fit(&mut fit, 1024).expect("info geometry");
            let info = InfoWindow::new(info_geometry, Default::default());
            let mut chat_fit = loader
                .art_fit("buttonlayout1024")
                .expect("mission chat FIT");
            let chat_geometry =
                MissionChatGeometry::from_fit(&mut chat_fit).expect("mission ChatTextRect");
            let mut fit = loader.art_fit("buttonlayout1024").expect("objectives FIT");
            let mut objectives =
                ObjectivesPanel::from_fit(&mut loader, &mut fit).expect("objectives panel");
            objectives.start(true, false);
            // Run the panel's real update once so its slide geometry matches a live HUD frame.
            (
                control,
                unit_bar,
                info,
                chat_geometry,
                objectives,
                pause_window,
            )
        };
        let unit = crate::unitbar::UnitBarUnit {
            id: 77,
            icon_picture: 0,
            tonnage: 50,
            pilot: crate::unitbar::Pilot {
                name: "SYNTHETIC".into(),
                photo: 0,
                message_playing: false,
            },
            armor: crate::unitbar::Armor::Mech(Default::default()),
            bar_status: 1.0,
            selected: false,
            groups: 1,
            shutdown: false,
            destroyed: false,
            recover_pending: false,
            power_used_up: false,
            can_jump: false,
        };
        unit_bar.update(
            &crate::unitbar::UnitBarInput {
                units: vec![unit.clone()],
                ..Default::default()
            },
            &mut assets.textures,
        );
        let mut bar_output = unit_bar.update(
            &crate::unitbar::UnitBarInput {
                units: vec![
                    crate::unitbar::UnitBarUnit {
                        destroyed: true,
                        ..unit.clone()
                    },
                    crate::unitbar::UnitBarUnit { id: 78, ..unit },
                ],
                ..Default::default()
            },
            &mut assets.textures,
        );
        let mut chat_fit = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            loader
                .art_fit("buttonlayout1024")
                .expect("mission chat FIT")
        };
        let mission_chat = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            let mut chat =
                MissionChat::from_fit(&mut chat_fit, loader.font(26087), loader.font(26086))
                    .expect("mission ChatTextRect");
            chat.lines.push(MissionChatLine {
                player_name: "COMMAND".into(),
                message: "Move to the ridge".into(),
                background_color: 0xffee_eecc,
                text_color: 0xffff_ffff,
            });
            chat
        };
        let mut tac_map = TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100));
        let mut terrain = Vec::with_capacity(128 * 128 * 4);
        for y in 0..128 {
            for x in 0..128 {
                let ridge = (x / 8 + y / 11) % 2 == 0;
                terrain.extend_from_slice(if ridge {
                    &[58, 78, 61, 255]
                } else {
                    &[43, 57, 47, 255]
                });
            }
        }
        let terrain_id = assets
            .textures
            .insert("synthetic-mission-map", 128, 128, terrain);
        tac_map.art = crate::tacmap::TacMapArt::load(&mut assets.textures, &mut vfs);
        tac_map.art.terrain = Some(terrain_id);
        tac_map.set_terrain_size(128, 128);
        let tac_map_input = TacMapInput {
            units: vec![crate::tacmap::TacUnit {
                team: crate::tacmap::TacTeam::Ours,
                artillery: false,
                sensor_entry: false,
                pos: (-640.0, 320.0),
                selected: true,
                sensor_range_m: 220.0,
            }],
            view_corners: [
                (-900.0, 900.0),
                (-900.0, -900.0),
                (900.0, -900.0),
                (900.0, 900.0),
            ],
            ..Default::default()
        };
        control.update(crate::control::ControlPanelInput {
            selected_units: &[],
            mouse: (0, 0),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: Default::default(),
            click: None,
            vehicle_click: None,
        });
        let map_region = HudRect::from_control(control.rects[0].0);
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![map_region],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: mission_chat.rect,
            pause: None,
            tac_map_region: Some(map_region),
            objectives: Some(objectives.layout().clone()),
        });
        hud.objectives_visible = true;
        let objectives_view = crate::objectives::ObjectivesView::default();
        let mut list = crate::draw::DrawList::new(1024, 768);
        {
            let mut ctx = crate::ctx::RenderCtx {
                dl: &mut list,
                assets: &assets,
                dt: 6.65,
            };
            let mut parts = HudRenderParts {
                control: &mut control,
                unit_bar: &mut unit_bar,
                unit_bar_output: &mut bar_output,
                info: &mut info,
                mission_chat: &mission_chat,
                tac_map: &mut tac_map,
                tac_map_input: &tac_map_input,
                metrics: &assets,
                objectives: &mut objectives,
                objectives_view: &objectives_view,
                pause_window: &mut pause_window,
            };
            let output = hud
                .render_frame(&mut ctx, &mut parts, HudOutput::default())
                .expect("render supported panels");
            assert_eq!(
                output.intents,
                [HudIntent::UnitBar(UnitBarIntent::LeaveAllGroups(77))],
                "render-generated death intent joins the composed frame output"
            );
        }

        assert!(
            list.quad_count() > 10,
            "MissionHud records supported component draws"
        );
        hud.paused = true;
        pause_window.begin(hud.objectives_visible);
        pause_window.update((0, 0), false, 3.0);
        let mut paused_list = crate::draw::DrawList::new(1024, 768);
        {
            let mut ctx = crate::ctx::RenderCtx {
                dl: &mut paused_list,
                assets: &assets,
                dt: 3.0,
            };
            let mut parts = HudRenderParts {
                control: &mut control,
                unit_bar: &mut unit_bar,
                unit_bar_output: &mut bar_output,
                info: &mut info,
                mission_chat: &mission_chat,
                tac_map: &mut tac_map,
                tac_map_input: &tac_map_input,
                metrics: &assets,
                objectives: &mut objectives,
                objectives_view: &objectives_view,
                pause_window: &mut pause_window,
            };
            hud.render(&mut ctx, &mut parts)
                .expect("composed paused HUD render");
        }
        assert!(
            paused_list.quad_count() > list.quad_count(),
            "the composed renderer adds the FIT-backed pause panel while paused"
        );
        hud.paused_without_menu = true;
        let mut no_menu_list = crate::draw::DrawList::new(1024, 768);
        {
            let mut ctx = crate::ctx::RenderCtx {
                dl: &mut no_menu_list,
                assets: &assets,
                dt: 3.0,
            };
            let mut parts = HudRenderParts {
                control: &mut control,
                unit_bar: &mut unit_bar,
                unit_bar_output: &mut bar_output,
                info: &mut info,
                mission_chat: &mission_chat,
                tac_map: &mut tac_map,
                tac_map_input: &tac_map_input,
                metrics: &assets,
                objectives: &mut objectives,
                objectives_view: &objectives_view,
                pause_window: &mut pause_window,
            };
            hud.render(&mut ctx, &mut parts)
                .expect("composed pause-without-menu render");
        }
        assert_eq!(
            no_menu_list.quad_count(),
            list.quad_count(),
            "paused without menu draws no pause window"
        );
        hud.paused_without_menu = false;
        // Rasterize the production DrawList and loaded texture pixels on the CPU so this artifact
        // remains verifiable on a headless host with no available GPU adapter.
        let rgba = rasterize_draw_list(&list, &assets, [14, 20, 24, 255]);
        // The picture is game art, so none is committed: `UPDATE_MISSION_HUD_PNG=<file>` writes it for a look.
        assert_eq!(rgba.len(), 1024 * 768 * 4);
        let distinct = rgba
            .chunks_exact(4)
            .take(200_000)
            .collect::<std::collections::HashSet<_>>();
        assert!(distinct.len() > 8, "the composed HUD is not a blank frame");
        if let Some(path) = std::env::var_os("UPDATE_MISSION_HUD_PNG") {
            mc2_render::gpu::write_png(std::path::Path::new(&path), &rgba, 1024, 768)
                .expect("write composed HUD PNG");
        }
    }

    #[test]
    fn show_objectives_command_toggles_the_merged_panel_and_remains_an_intent() {
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: vec![],
            unit_bar: vec![],
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        // missiongui.cpp:4584-4593 calls startObjectives(!objectivesStarted());
        // controlgui.cpp:464-471 owns the panel timer/open state.
        let opened = hud.update_state(HudInput {
            mission_commands: vec![MissionCommand::ShowObjectives],
            ..Default::default()
        });
        assert!(opened.objectives_visible);
        assert_eq!(opened.unavailable, ["selection and world orders"]);
        assert_eq!(
            opened.intents,
            vec![HudIntent::Mission(MissionCommand::ShowObjectives)]
        );
        let closed = hud.update_state(HudInput {
            mission_commands: vec![MissionCommand::ShowObjectives],
            ..Default::default()
        });
        assert!(!closed.objectives_visible);
    }

    #[test]
    fn pause_buttons_stay_aligned_across_repeated_animation_at_supported_widths() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED {}: MC2_DATA is unset", module_path!());
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        for (width, height) in [
            (640_u32, 480_u32),
            (800, 600),
            (1024, 768),
            (1280, 1024),
            (1600, 1200),
            (1920, 1200),
        ] {
            let mut assets = UiAssets {
                screen: (width as i32, height as i32),
                ..UiAssets::default()
            };
            let mut pause = {
                let mut loader = Loader::new(&mut vfs, &mut assets);
                PauseWindow::load(&mut loader, width, height).expect("pause FIT")
            };
            let base_panel = pause.panel_rect();
            let base_button = pause.button_rect_for_id(6).expect("RETURN button");
            pause.begin(false);
            for dt in [0.016, 0.024, 0.21, 0.08, 0.31, 2.35] {
                pause.update((0, 0), false, dt);
                let panel = pause.panel_rect();
                let button = pause.button_rect_for_id(6).expect("RETURN button");
                assert_eq!(
                    button.left - base_button.left,
                    panel.left - base_panel.left,
                    "button/panel x offset during first animation at {width}x{height}"
                );
            }
            pause.end();
            pause.begin(false);
            for dt in [0.016, 0.047, 0.19, 0.11, 0.24] {
                pause.update((0, 0), false, dt);
                let panel = pause.panel_rect();
                let button = pause.button_rect_for_id(6).expect("RETURN button");
                assert_eq!(
                    button.left - base_button.left,
                    panel.left - base_panel.left,
                    "button/panel x offset during repeated animation at {width}x{height}"
                );
            }
        }
    }

    #[test]
    fn production_update_routes_real_control_tab_objectives_and_pause_fit_clicks() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED {}: MC2_DATA is unset", module_path!());
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets {
            screen: (1024, 768),
            ..UiAssets::default()
        };
        let (mut control, mut pause_window) = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            (
                ControlPanel::load(&mut loader, 1024, 768).expect("control FIT"),
                PauseWindow::load(&mut loader, 1024, 768).expect("pause FIT"),
            )
        };
        let tab_rect = control
            .buttons
            .iter()
            .find(|button| button.cxx_id == 13)
            .expect("Info tab FIT button")
            .rect;
        let objectives_rect = control
            .buttons
            .iter()
            .find(|button| button.cxx_id == 10)
            .expect("objectives FIT button")
            .rect;
        let pause_rect = control
            .buttons
            .iter()
            .find(|button| button.cxx_id == 11)
            .expect("pause FIT button")
            .rect;
        let mut tac_map = TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100));
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: control
                .buttons
                .iter()
                .map(|button| HudRect::from_control(button.rect))
                .collect(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        let mut input = crate::input::UiInput::new(1024, 768);
        let mut output = crate::ctx::UiOutput::default();
        let selected = [];

        macro_rules! click_fit {
            ($rect:expr) => {{
                let rect = $rect;
                let point = ((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2);
                let events = [
                    mc2_platform::input::InputEvent::CursorMoved {
                        x: point.0 as f64,
                        y: point.1 as f64,
                    },
                    mc2_platform::input::InputEvent::Button {
                        button: mc2_platform::input::MouseButton::Left,
                        pressed: true,
                    },
                    mc2_platform::input::InputEvent::Button {
                        button: mc2_platform::input::MouseButton::Left,
                        pressed: false,
                    },
                ];
                let mut update_ctx = crate::ctx::UpdateCtx {
                    input: &mut input,
                    assets: &assets,
                    dt: 0.016,
                    out: &mut output,
                };
                hud.update(MissionHudInput {
                    state: HudInput::default(),
                    control: &mut control,
                    control_input: ControlPanelInput {
                        selected_units: &selected,
                        mouse: point,
                        paused: hud.paused,
                        vtol_in_flight: false,
                        resource_points: 0,
                        vehicle_availability: Default::default(),
                        click: None,
                        vehicle_click: None,
                    },
                    tac_map: &mut tac_map,
                    update_ctx: &mut update_ctx,
                    pause_window: &mut pause_window,
                    unit_bar: None,
                    events: &events,
                    dt: 0.016,
                })
            }};
        }

        let tab = click_fit!(tab_rect);
        assert_eq!(tab.tab, MissionTab::Info);
        assert_eq!(
            tab.intents,
            [HudIntent::Panel(MissionPanelAction::SelectTab(
                MissionTab::Info
            ))]
        );
        let objectives = click_fit!(objectives_rect);
        assert!(objectives.objectives_visible);
        assert_eq!(
            objectives.intents,
            [HudIntent::Panel(MissionPanelAction::ToggleObjectives)]
        );
        let paused = click_fit!(pause_rect);
        assert!(paused.paused);
        assert!(pause_window.is_started());
        assert_eq!(
            paused.intents,
            [HudIntent::Panel(MissionPanelAction::TogglePause)]
        );
        pause_window.update((0, 0), false, 3.0);
        let return_rect = pause_window
            .button_rect_for_id(6)
            .expect("RETURN FIT button");
        let return_point = (
            (return_rect.left + return_rect.right) / 2,
            (return_rect.top + return_rect.bottom) / 2,
        );
        let resume_events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: return_point.0 as f64,
                y: return_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let mut update_ctx = crate::ctx::UpdateCtx {
            input: &mut input,
            assets: &assets,
            dt: 0.016,
            out: &mut output,
        };
        let resumed = hud.update(MissionHudInput {
            state: HudInput::default(),
            control: &mut control,
            control_input: ControlPanelInput {
                selected_units: &selected,
                mouse: return_point,
                paused: true,
                vtol_in_flight: false,
                resource_points: 0,
                vehicle_availability: Default::default(),
                click: None,
                vehicle_click: None,
            },
            tac_map: &mut tac_map,
            update_ctx: &mut update_ctx,
            pause_window: &mut pause_window,
            unit_bar: None,
            events: &resume_events,
            dt: 0.016,
        });
        assert!(!resumed.paused);
        assert!(
            resumed.objectives_visible,
            "objectives open before pause stay open on resume"
        );
    }

    #[test]
    fn production_update_waits_for_fit_release_and_derives_vehicle_build_and_stop() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            eprintln!("SKIPPED MissionHud click routing: MC2_DATA is unset");
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("open MC2_DATA VFS");
        let mut assets = UiAssets {
            screen: (1024, 768),
            ..UiAssets::default()
        };
        let (mut control, mut pause_window) = {
            let mut loader = Loader::new(&mut vfs, &mut assets);
            (
                ControlPanel::load(&mut loader, 1024, 768).expect("control FIT"),
                PauseWindow::load(&mut loader, 1024, 768).expect("pause FIT"),
            )
        };
        let pause_rect = control
            .buttons
            .iter()
            .find(|button| button.cxx_id == 11)
            .unwrap()
            .rect;
        let pause_point = (
            (pause_rect.left + pause_rect.right) / 2,
            (pause_rect.top + pause_rect.bottom) / 2,
        );
        let mut hud = MissionHud::new(HudLayout {
            resolution: (1024, 768),
            control: control
                .buttons
                .iter()
                .map(|button| HudRect::from_control(button.rect))
                .collect(),
            unit_bar: Vec::new(),
            info: HudRect::default(),
            chat: HudRect::default(),
            pause: None,
            tac_map_region: None,
            objectives: Some(PanelLayout::default()),
        });
        let mut input = crate::input::UiInput::new(1024, 768);
        let mut ui_output = crate::ctx::UiOutput::default();
        let selected = [];
        macro_rules! update {
            ($events:expr, $point:expr, $availability:expr, $resources:expr) => {{
                let events = $events;
                let point = $point;
                let mut update_ctx = crate::ctx::UpdateCtx {
                    input: &mut input,
                    assets: &assets,
                    dt: 0.016,
                    out: &mut ui_output,
                };
                hud.update(MissionHudInput {
                    state: HudInput::default(),
                    control: &mut control,
                    control_input: ControlPanelInput {
                        selected_units: &selected,
                        mouse: point,
                        paused: hud.paused,
                        vtol_in_flight: false,
                        resource_points: $resources,
                        vehicle_availability: $availability,
                        click: None,
                        vehicle_click: None,
                    },
                    tac_map: &mut TacMap::new(crate::tacmap::TerrainBounds::from_vertices(100)),
                    update_ctx: &mut update_ctx,
                    pause_window: &mut pause_window,
                    unit_bar: None,
                    events,
                    dt: 0.016,
                })
            }};
        }
        let press = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: pause_point.0 as f64,
                y: pause_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
        ];
        let held = [];
        let dragged = [mc2_platform::input::InputEvent::CursorMoved { x: 100.0, y: 100.0 }];
        let release = [mc2_platform::input::InputEvent::Button {
            button: mc2_platform::input::MouseButton::Left,
            pressed: false,
        }];
        assert!(!update!(&press, pause_point, Default::default(), 10_000).paused);
        assert!(!update!(&held, pause_point, Default::default(), 10_000).paused);
        update!(&dragged, (100, 100), Default::default(), 10_000);
        assert!(!update!(&release, (100, 100), Default::default(), 10_000).paused);
        let completed_click = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: pause_point.0 as f64,
                y: pause_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        assert!(update!(&completed_click, pause_point, Default::default(), 10_000).paused);

        control.vehicle_tab = true;
        let availability = crate::control::VehicleAvailability {
            can_have_airstrike: true,
            ..Default::default()
        };
        let build = control
            .vehicles
            .iter()
            .find(|button| button.id == 100)
            .unwrap()
            .rect;
        let build_point = (
            (build.left + build.right) / 2,
            (build.top + build.bottom) / 2,
        );
        let build_events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: build_point.0 as f64,
                y: build_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let built = update!(&build_events, build_point, availability, 10_000);
        assert_eq!(
            built.intents,
            [HudIntent::Control(ControlIntent::BuildVehicle(
                crate::control::VehicleKind::Airstrike
            ))]
        );
        assert_eq!(
            control.selected_vehicle,
            Some(crate::control::VehicleKind::Airstrike)
        );

        let stop = control
            .vehicles
            .iter()
            .find(|button| button.id == 107)
            .unwrap()
            .rect;
        let stop_point = ((stop.left + stop.right) / 2, (stop.top + stop.bottom) / 2);
        let stop_events = [
            mc2_platform::input::InputEvent::CursorMoved {
                x: stop_point.0 as f64,
                y: stop_point.1 as f64,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: true,
            },
            mc2_platform::input::InputEvent::Button {
                button: mc2_platform::input::MouseButton::Left,
                pressed: false,
            },
        ];
        let stopped = update!(&stop_events, stop_point, availability, 3_000);
        assert_eq!(
            stopped.intents,
            [HudIntent::Control(ControlIntent::CancelVehicle {
                kind: crate::control::VehicleKind::Airstrike,
                refund: 7_000,
            })]
        );
        assert_eq!(control.selected_vehicle, None);
    }
}
