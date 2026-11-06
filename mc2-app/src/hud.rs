// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission HUD on live simulation state (`ControlGui` + `MissionInterfaceManager`'s GUI
//! half, `code/controlgui.cpp`, `code/missiongui.cpp`).
//!
//! [`HudRuntime`] owns the `mc2-ui` parts (control panel, unit bar, info window, tactical map,
//! objectives, pause window, mission chat) and the composed [`MissionHud`].  Every game frame
//! [`HudRuntime::update`] builds their plain inputs from a [`HudSnapshot`] (the live simulation
//! read once, see [`crate::hud_data`]) and the app's selection, runs [`MissionHud::update`] with
//! the real mouse and keyboard events, and routes what comes back:
//!
//! * **selection** intents (unit bar clicks, select-visible, world click and drag-select) change [`Selection`], the app's selection;
//! * **camera** intents (tactical map click, unit bar centre) are returned for the caller to
//!   apply to the camera rig ([`HudFrameOutput::camera_target`]);
//! * **pause** is the HUD's (`MissionHud::paused`); the caller feeds it to the simulation as
//!   `SetPaused`, which stops the object update exactly as `Mission::update` does while
//!   `isPaused()`;
//! * **order** intents (control panel buttons, order hot keys, tactical map move orders) go to
//!   the order bridge (`orders::intents_to_commands`), whose `PlayerCommand`s are returned in
//!   [`HudFrameOutput::commands`] for the same frame's simulation step. Force groups use the
//!   simulation's saved per-mover membership; the HUD owns only the transient selection.
//!
//! The pointer is in game pixels; the HUD lives in the `.fit` layout space chosen by
//! [`hud_resolution`], to which pointer and picture are scaled uniformly.

use crate::audio::{cues_from_ui, Cue};
use crate::hotkeys::{Command, Fired};
use crate::hud_data::{HudMessage, HudSnapshot, HudText, HudUnit, PickTarget};
use crate::orders::{
    self, AttackTargetKind, BridgeContext, OrdersState, Unavailable, ViewportOrderClick,
};
use crate::overlay::{OverlayBatch, OverlayPainter};
use crate::pilot_video::PilotVideo;
use crate::userinput::UserInput;
use crate::world_view::{self, HoldFireIcon, SelectedMark};
use mc2_audio::ids::{betty, sfx};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_render::camera::GameCamera;
use mc2_render::gpu::{Gpu, RenderTarget};
use mc2_render::math::Vec3;
use mc2_sim::combat::artillery_math::{ARTILLERY_LARGE, ARTILLERY_SENSOR};
use mc2_sim::command::{OrderCode, OrderOrigin, PlayerCommand, TacticalOrder};
use mc2_sim::ids::{PartId, WatchId};
use mc2_ui::color::interpolate_color;
use mc2_ui::control::{
    ButtonVisualState, ControlPanel, ControlPanelInput, ControlRect, UnitCapabilities,
    VehicleAvailability,
};
use mc2_ui::cursor::Cursors;
use mc2_ui::draw::{Blend, DrawList, GuiRect};
use mc2_ui::font::text::{self, TextAttrs};
use mc2_ui::font::FontRef;
use mc2_ui::hud_info::icon::IconAtlases;
use mc2_ui::hud_info::info::{InfoFonts, InfoGeometry, InfoMouse, InfoWindow};
use mc2_ui::input::UiInput;
use mc2_ui::mission_hud::{
    HitTarget, HudInput, HudIntent, HudLayout, HudOutput, HudRect, HudRenderParts, MissionChat,
    MissionChatGeometry, MissionChatLine, MissionHud, MissionHudInput, MissionTab,
};
use mc2_ui::mission_keys::MissionCommand;
use mc2_ui::objectives::{ObjectivesPanel, ObjectivesView, StaticInfo, OBJECTIVE_MOVE};
use mc2_ui::pause_window::{PauseIntent, PauseWindow};
use mc2_ui::render::{ScaleMode, UiLayout, UiRenderer};
use mc2_ui::tacmap::{TacMap, TacMapArt, TacMapInput, TacTeam, TacUnit, TerrainBounds};
use mc2_ui::unitbar::layout::Layout;
use mc2_ui::unitbar::{
    Art, MouseProfile, Pointer, UnitBar, UnitBarInput, UnitBarIntent, UnitBarOutput,
};
use mc2_ui::{Loader, RenderCtx, UiAssets, UiOutput, UpdateCtx};
use std::collections::BTreeMap;

/// The resolutions `buttonlayout<N>.fit` are laid out for (width, height): the HUD of each is a block
/// that the `HiresOffsets` and the screen height place inside that screen size.
pub const LAYOUT_SIZES: [(u32, u32); 6] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 1024),
    (1600, 1200),
    (1920, 1200),
];

/// How long a mission message stays in the log, seconds.
/// `missionResultsMoveInfo[RESULTS_MOVE_COUNT - 1].time` (`controlgui.cpp:136-141`): the status frame has arrived.
const RESULTS_MOVE_END: f32 = 0.24;
/// `missionResultsMoveInfo` (`controlgui.cpp:136-142`).
const RESULTS_MOVE: [(f32, f32); 3] = [(0.0, -65.0), (0.16, 0.0), (0.24, -10.0)];
const CHAT_SECONDS: f32 = 10.0;
/// Lines the log keeps.
const CHAT_LINES: usize = 6;
/// `IDS_OUTLINE_CHAT_FONT`-class fonts the C++ uses for the log: name, message.
const CHAT_FONTS: (u32, u32) = (26087, 26086);
/// A click selects the nearest unit within this share of the screen height.
const CLICK_PICK_RADIUS: f32 = 0.04;
/// Selection rectangle colour while dragging (`MissionInterfaceManager::render` white box).
const DRAG_BOX_COLOUR: u32 = 0xffff_ffff;
/// `ControlGui` button ids (`cxx_id`, `code/controlgui.h`) whose click sound is not the default.
const STOP_COMMAND: i32 = 8;
const OBJECTIVES_COMMAND: i32 = 10;

/// Vertical location of the results frame. Unlike the C++ mutable `StaticInfo`s this derives
/// every frame from the authored positions, so resize/reload cannot accumulate animation drift.
fn results_status_position(results_time: f32, screen_width: u32) -> f32 {
    let scale = screen_width as f32 / 640.0;
    for window in RESULTS_MOVE.windows(2) {
        let [(t0, p0), (t1, p1)] = *window else {
            unreachable!()
        };
        if t0 <= results_time && results_time < t1 {
            return (p0 + (results_time - t0) * (p1 - p0) / (t1 - t0)) * scale;
        }
    }
    RESULTS_MOVE[2].1 * scale
}

/// The samples a left click on a control button plays (`ControlGui::handleClick`, `code/controlgui.cpp:1538-1666`).
/// A hidden button takes no click (`:1540`). A disabled one plays `INVALID_GUI` and does nothing else
/// (`:1543-1546`). Otherwise the click plays `LOG_SELECT` (`:1552`), or `LOG_CLICKONBUTTON` for the objectives
/// button (`:1621`). Stop runs `doStop` first, which plays `BUTTON5` (`:1673`).
fn control_click_samples(cxx_id: i32, state: ButtonVisualState) -> Vec<i32> {
    match state {
        ButtonVisualState::Hidden => Vec::new(),
        ButtonVisualState::Disabled => vec![sfx::INVALID_GUI as i32],
        _ if cxx_id == STOP_COMMAND => vec![sfx::BUTTON5 as i32, sfx::LOG_SELECT as i32],
        _ if cxx_id == OBJECTIVES_COMMAND => vec![sfx::LOG_CLICKONBUTTON as i32],
        _ => vec![sfx::LOG_SELECT as i32],
    }
}

/// The samples a left click on a support-vehicle button plays (`ControlGui::handleVehicleClick`,
/// `code/controlgui.cpp:1947-1991`). The click itself is only made on a button that is not already
/// pressed (`:1788`). A disabled button plays `INVALID_GUI` (`:1955`); an enabled one `LOG_SELECT` (`:1991`).
fn vehicle_click_samples(state: ButtonVisualState) -> Vec<i32> {
    match state {
        ButtonVisualState::Hidden | ButtonVisualState::Pressed => Vec::new(),
        ButtonVisualState::Disabled => vec![sfx::INVALID_GUI as i32],
        ButtonVisualState::Enabled | ButtonVisualState::Ambiguous => vec![sfx::LOG_SELECT as i32],
    }
}

/// A control-panel button under the pointer: a control button by its `cxx_id`, a support-vehicle button by its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlPress {
    Button(i32),
    Vehicle(i32),
}

/// The Betty warnings of the mission timer (`ControlGui::render`, `code/controlgui.cpp:375-400`): the two-minute
/// line once the time left is 120 s or less, then the thirty-second line once it is 30 s or less. Each plays once
/// per mission and the `else if` means one line per update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TimerWarnings {
    two_min_played: bool,
    thirty_second_played: bool,
}

/// The layout space of the HUD for a game screen: the largest shipped layout that fits inside it.
/// The HUD is drawn in that space, fitted (uniform scale, centred) into the screen, and the pointer
/// is mapped back through the same fit.
pub fn hud_resolution(game_size: (f32, f32)) -> (u32, u32) {
    LAYOUT_SIZES
        .iter()
        .rev()
        .copied()
        .find(|(w, h)| *w as f32 <= game_size.0.round() && *h as f32 <= game_size.1.round())
        .unwrap_or(LAYOUT_SIZES[0])
}

/// The width of a layout's HUD block, the control geometry the FIT files were authored for: the layout itself
/// for 640 and 800, 1024 for every larger one (the larger layouts centre it with `HiresOffsets`).
fn block_width(res: (u32, u32)) -> i32 {
    res.0.min(1024) as i32
}

/// How a screen wider than 4:3 is filled (a port extension: the original has no wide screens). The HUD block
/// ([`block_width`]) stays 1:1 in layout pixels and moves to the screen's left edge (the layout's `xOffset` is
/// dropped); `extra` more layout pixels fit the screen's aspect, and the HUD's right group (command panel, unit
/// bar frame end, pause panel) moves right by them, so the minimap hugs the bottom left and the command panel the
/// bottom right. 0 for 4:3 and narrower, where the layout is exactly the original's.
pub fn wide_extra(game_size: (f32, f32), res: (u32, u32)) -> i32 {
    let (gw, gh) = (game_size.0.round().max(1.0), game_size.1.round().max(1.0));
    if gw * 3.0 <= gh * 4.0 + 1.0 {
        return 0;
    }
    let visible = (res.1 as f32 * gw / gh).round() as i32;
    (visible - block_width(res)).max(0)
}

/// The width of the layout space the HUD draws in: the layout's own, or the wide screen's.
fn view_width(res: (u32, u32), extra: i32) -> i32 {
    if extra > 0 {
        block_width(res) + extra
    } else {
        res.0 as i32
    }
}

/// The app's transient selection (`ObjectManager` selected flags). Force-group membership belongs
/// to each simulated mover, exactly as C++ `Mover::unitGroup` does.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    /// Part ids of the selected units, in the order they were selected.
    pub selected: Vec<i32>,
}

impl Selection {
    pub fn is_selected(&self, part: i32) -> bool {
        self.selected.contains(&part)
    }

    fn select_only(&mut self, parts: impl IntoIterator<Item = i32>) {
        self.selected.clear();
        self.add(parts);
    }

    fn add(&mut self, parts: impl IntoIterator<Item = i32>) {
        for p in parts {
            if !self.selected.contains(&p) {
                self.selected.push(p);
            }
        }
    }

    fn remove(&mut self, part: i32) {
        self.selected.retain(|p| *p != part);
    }
}

/// Runs the frame's order intents (and a completed world click) through the order bridge
/// (`orders::intents_to_commands`) and records the intents in `log`.  The bridge's selection is the
/// HUD's, copied in first: the bridge never changes it (selection intents do not go through it).
fn apply_order_intents(
    log: &mut Vec<HudIntent>,
    state: &mut OrdersState,
    selection: &Selection,
    intents: &[HudIntent],
    context: &BridgeContext,
) -> orders::BridgeOutput {
    log.extend(intents.iter().cloned());
    state.selected = selection.selected.iter().map(|p| PartId(*p)).collect();
    orders::intents_to_commands(intents, state, context)
}

/// Whether an intent is an order (as opposed to selection, camera, pause, tabs).
fn is_order_intent(intent: &HudIntent) -> bool {
    use MissionCommand as M;
    match intent {
        HudIntent::Control(_) | HudIntent::TacMapMoveOrder(_) => true,
        HudIntent::Mission(c) => matches!(
            c,
            M::AttackShort
                | M::AttackMedium
                | M::AttackLong
                | M::EnergyWeapons
                | M::DefaultAttack
                | M::Jump
                | M::StopJump
                | M::FireFromCurrentPos
                | M::StopFireFromCurrentPos
                | M::Guard
                | M::StopGuard
                | M::ConserveAmmo
                | M::ForceShot
                | M::GotoNextNavMarker
                | M::SelectVisible
                | M::AddVisibleToSelection
                | M::InfoCommand
                | M::InfoButtonReleased
                | M::AimLeg
                | M::AimArm
                | M::AimHead
                | M::RemoveCommand
                | M::PowerDown
                | M::PowerUp
                | M::Eject
                | M::ChangeSpeed
                | M::StopChangeSpeed
                | M::VehicleCommand
                | M::SendAirstrike
                | M::SendSensorStrike
                | M::ToggleHoldPosition
        ),
        _ => false,
    }
}

/// The mission command a non-camera hot key runs.
fn mission_command(c: Command) -> Option<MissionCommand> {
    use Command as C;
    use MissionCommand as M;
    Some(match c {
        C::AttackShort => M::AttackShort,
        C::AttackMedium => M::AttackMedium,
        C::AttackLong => M::AttackLong,
        C::EnergyWeapons => M::EnergyWeapons,
        C::DefaultAttack => M::DefaultAttack,
        C::Jump => M::Jump,
        C::StopJump => M::StopJump,
        C::FireFromCurrentPos => M::FireFromCurrentPos,
        C::StopFireFromCurrentPos => M::StopFireFromCurrentPos,
        C::Guard => M::Guard,
        C::StopGuard => M::StopGuard,
        C::ConserveAmmo => M::ConserveAmmo,
        C::SelectVisible => M::SelectVisible,
        C::AddVisibleToSelection => M::AddVisibleToSelection,
        C::GotoNextNavMarker => M::GotoNextNavMarker,
        C::InfoCommand => M::InfoCommand,
        C::InfoButtonReleased => M::InfoButtonReleased,
        C::ForceShot => M::ForceShot,
        C::AimLeg => M::AimLeg,
        C::AimArm => M::AimArm,
        C::AimHead => M::AimHead,
        C::RemoveCommand => M::RemoveCommand,
        C::PowerDown => M::PowerDown,
        C::PowerUp => M::PowerUp,
        C::Eject => M::Eject,
        C::ChangeSpeed => M::ChangeSpeed,
        C::StopChangeSpeed => M::StopChangeSpeed,
        C::VehicleCommand => M::VehicleCommand,
        C::ShowObjectives => M::ShowObjectives,
        C::TogglePause => M::TogglePause,
        C::ToggleHotKeys => M::ToggleHotKeys,
        C::SwitchTab => M::SwitchTab,
        C::ReverseSwitchTab => M::ReverseSwitchTab,
        C::SendAirstrike => M::SendAirstrike,
        C::SendSensorStrike => M::SendSensorStrike,
        C::ToggleHoldPosition => M::ToggleHoldPosition,
        _ => return None,
    })
}

/// What the order bridge needs of one frame besides the intents.
struct OrderFrame {
    scenario_time: f32,
    waypoint_queue: bool,
    /// A completed world command click this frame.
    command_click: bool,
    /// ...that landed on an object of another commander in contact (`getObjectFromScreenPos`).
    picked: Option<PickTarget>,
    mouse_world: Option<Vec3>,
}

/// A mouse gesture in the world view.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Gesture {
    Click((f32, f32)),
    Box((f32, f32), (f32, f32)),
}

/// What the caller gives [`HudRuntime::update`] each game frame.
pub struct HudFrameInput<'a> {
    /// The input events since the last frame, pointer in game pixels.
    pub events: &'a [InputEvent],
    pub user_input: &'a UserInput,
    /// The hot keys that fired this frame and that are not camera commands.
    pub fired: &'a [Fired],
    /// The camera, its viewport in native pixels.
    pub camera: &'a GameCamera,
    pub game_size: (f32, f32),
    /// `frameLength`.
    pub dt: f32,
    /// Pixels the pointer must move before a held button drags (`mouseDragThreshold`).
    pub drag_threshold_px: f32,
    pub left_right_profile: bool,
    pub shift: bool,
    /// The waypoint modifier is held (`getKeyDown(WAYPOINT_KEY)`, left control).
    pub waypoint_queue: bool,
    /// `turn` (`missiongui.cpp:5597`): `calcRotation` returns `mState_NORMAL` until turn 4.
    pub turn: u32,
    /// The camera button is held and `moveCameraAround` took the mouse (`mState_ROTATE_CAMERA`).
    pub camera_rotate: bool,
    /// `Mission::scenarioTime`, stamped on the orders.
    pub scenario_time: f32,
    /// The ground point under the pointer (x, y, terrain elevation), if the pointer is over it.
    pub mouse_world: Option<Vec3>,
    /// `Team::home->teamLineOfSight(wPos, 0)` (`missiongui.cpp:753`).
    pub line_of_sight: bool,
    /// `GameMap->getPassable` at the pointer (`missiongui.cpp:3502-3507`).
    pub passable: bool,
    /// Terrain elevation at a world (x, y), for the tactical map's move orders.
    pub elevation: &'a dyn Fn(f32, f32) -> f32,
    pub snapshot: HudSnapshot,
    /// Pilots currently speaking, from `GameSoundSystem`'s exact radio callbacks.
    pub speaking_pilots: &'a std::collections::BTreeSet<u32>,
}

/// What [`HudRuntime::update`] returns for the caller to apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HudFrameOutput {
    /// The HUD's pause state after this frame.
    pub paused: bool,
    /// A tactical map click: the camera goes to this world (x, y).
    pub camera_target: Option<(f32, f32)>,
    /// The unit bar asked to centre the camera on this part.
    pub center_on: Option<i32>,
    /// The pointer is over a HUD panel (the camera and world clicks leave it alone).
    pub over_hud: bool,
    /// What the order bridge refused this frame (reported, not done).
    pub unavailable: Vec<Unavailable>,
    /// What the pause panel asked the game to do this frame (`PauseWindow::handleClick`).
    pub pause_actions: Vec<PauseIntent>,
    /// The player orders for this frame's simulation step.
    pub commands: Vec<PlayerCommand>,
    /// The sounds the HUD asks for this frame, in order: its button and pointer sounds, the unit bar's selection
    /// sound, the parts' own sounds (`UiOutput`), and the mission timer's Betty warnings.
    pub cues: Vec<Cue>,
}

/// Which support options the mission file allows (`[MissionSettings]` `Enable*`, read by `LogisticsData::canHave*`).
#[derive(Clone, Copy, Debug)]
struct SupportEnabled {
    air_strike: bool,
    sensor_strike: bool,
    guard_tower: bool,
    repair_truck: bool,
    scout: bool,
    mine_layer: bool,
    salvage: bool,
}

/// The in-mission HUD.
pub struct HudRuntime {
    assets: UiAssets,
    res: (u32, u32),
    /// How far the screen is wider than the layout's block ([`wide_extra`]), and the layout width the HUD draws in.
    wide_extra: i32,
    view_w: i32,
    hud: MissionHud,
    control: ControlPanel,
    unit_bar: UnitBar,
    /// The `[MissionSettings]` support flags of the mission file (`LogisticsData::canHave*`).
    support_enabled: SupportEnabled,
    /// `paintingMyVtol` last frame: when it ends the bought vehicle's button is released (`unPressAllVehicleButtons`).
    vtol_was_in_flight: bool,
    unit_bar_output: UnitBarOutput,
    /// The pilot's talking-head video in the unit bar (`ForceGroupIcon::bMovie`).
    pilot_video: PilotVideo,
    info: InfoWindow,
    chat: MissionChat,
    tac_map: TacMap,
    objectives: ObjectivesPanel,
    objectives_view: ObjectivesView,
    /// `MissionResults` / `ResultsTextBox` of `buttonlayout*.fit`.
    result_statics: Vec<StaticInfo>,
    result_rect: GuiRect,
    /// First `MissionResults` static's initial y; `renderResults` moves every status element
    /// relative to this same anchor (`controlgui.cpp:531-565`).
    result_origin_y: f32,
    result_color: u32,
    result_font: FontRef,
    /// `ControlGui::resultsTime`: seconds since the in-mission results started (0 before).
    results_time: f32,
    /// Set only by [`Self::draw_results_status`] after it has recorded the in-mission status
    /// frame into this production frame's draw list.  Headless integration tests use this to
    /// distinguish a decided objective from a status frame that was actually rendered.
    results_status_frame_drawn: bool,
    /// The grey-out and fade colours `renderResults` draws over the whole screen (0: none).
    results_grey: bool,
    results_fade: u32,
    /// The text that frame carried.
    results_status_text: Option<&'static str>,
    pause_window: PauseWindow,
    ui_input: UiInput,
    ui_out: UiOutput,
    tac_map_input: TacMapInput,
    renderer: Option<UiRenderer>,
    last_output: HudOutput,
    /// The app's selection.
    pub selection: Selection,
    snapshot: HudSnapshot,
    /// The unit the info window was asked to show (`ShowInfo`).
    info_unit: Option<i32>,
    /// Pointer in game pixels.
    mouse: (f32, f32),
    /// Where the world press started, when it started over the world view.
    world_press: Option<(f32, f32)>,
    /// The drag rectangle being drawn, game pixels.
    drag_box: Option<((f32, f32), (f32, f32))>,
    chat_lines: Vec<(f32, MissionChatLine)>,
    /// `ControlGui::videoRect`: where a mission video plays (HUD coordinates).
    video_window: GuiRect,
    /// Order intents given to the order bridge.
    pub order_log: Vec<HudIntent>,
    orders_state: OrdersState,
    /// The `energyWeapons` key (A) is down: `commands[ENERGY_WEAPON_INDEX]`, set by its press and cleared by its release.
    energy_weapons_held: bool,
    /// `KEY_HELD` on `FORCE_FIRE_KEY` (`KEY_F` / `ForceShot`, `missiongui.cpp:3929, 3991`).
    force_fire_held: bool,
    /// Unavailable behaviour reached, with counts.
    unavailable: BTreeMap<String, u64>,
    game_size: (f32, f32),
    /// Shipped cursor sprites (`cursors.fit`); `None` if the art did not load.
    cursors: Option<Cursors>,
    /// The `mState_*` the order cursor last chose.
    order_cursor: u32,
    /// The tutorial pointer (game pixels) while a callout runs or the GUI is frozen: drawn with the `mState_TUTORIALS` cursor
    /// instead of the player's pointer.
    callout_pointer: Option<(f32, f32)>,
    /// Selected-unit brackets and health bars in game pixels, rebuilt each update.
    selected_marks: Vec<SelectedMark>,
    /// `Camera::newScaleFactor` of the last update (`drawSelectBrackets` offset).
    scale_factor: f32,
    /// Last `teamLineOfSight` / `GameMap` passable facts fed to the order cursor.
    last_line_of_sight: bool,
    last_passable: bool,
    last_mouse_world: Option<Vec3>,
    last_can_jump: bool,
    overlay: Option<OverlayPainter>,
    /// Pointer is inside the window (`UserInput::cursor_inside_window`).
    cursor_inside: bool,
    /// `blip.tga` 5×5 region for the hold-fire icon (`mover.cpp:2373-2378`).
    hold_fire_icon: Option<HoldFireIcon>,
    /// Keep the `MC2_P3_TRACE` command-click lines in [`Self::trace_log`] even when the environment
    /// switch is off (tests read the trace this way).
    pub capture_trace: bool,
    /// The command-click trace lines captured while [`Self::capture_trace`] is on.
    pub trace_log: Vec<String>,
    /// The mission timer's Betty warnings already played (`twoMinWarningPlayed`, `thirtySecondWarningPlayed`).
    timer_warnings: TimerWarnings,
    /// The pointer in HUD pixels at the last frame: a button is entered when it is inside now and was not before.
    hover_prev: Option<(i32, i32)>,
    /// The control-panel button the left button was pressed on; a click needs the same button on the release.
    control_press: Option<ControlPress>,
    /// Where that press began (HUD pixels), and whether the pointer has moved past the UI drag threshold since: the
    /// UI router latches a drag on any such move, even one that comes back, and a latched drag cancels the click.
    control_press_at: Option<(i32, i32)>,
    control_dragged: bool,
    /// Keep the sounds the HUD asked for in [`Self::sound_log`] (tests read them this way).
    pub capture_sounds: bool,
    /// The sounds captured while [`Self::capture_sounds`] is on, oldest first.
    pub sound_log: Vec<Cue>,
}

impl HudRuntime {
    /// Takes over the player-visible state of `old` when the HUD is rebuilt for a new layout
    /// (a resize): selection, groups, the recorded orders and the pause state, so a resize can
    /// neither drop the selection nor resume a paused mission.
    pub fn carry_state_from(&mut self, old: &mut HudRuntime) {
        // a playing talking head continues in the new layout's slot (the rect is looked up per frame)
        std::mem::swap(&mut self.pilot_video, &mut old.pilot_video);
        self.selection = old.selection.clone();
        self.order_log = old.order_log.clone();
        self.orders_state = old.orders_state.clone();
        self.energy_weapons_held = old.energy_weapons_held;
        self.force_fire_held = old.force_fire_held;
        self.chat_lines = old.chat_lines.clone();
        self.results_time = old.results_time;
        self.timer_warnings = old.timer_warnings;
        self.hover_prev = old.hover_prev;
        for (k, n) in &old.unavailable {
            // a fresh report already lists the static limitations once; only a recurring one
            // keeps the larger count, so a resize adds nothing
            let e = self.unavailable.entry(k.clone()).or_insert(0);
            *e = (*e).max(*n);
        }
        self.hud.paused = old.hud.paused;
        self.hud.paused_without_menu = old.hud.paused_without_menu;
        self.hud.tab = old.hud.tab;
        self.hud.gui_visible = old.hud.gui_visible;
        self.hud.objectives_visible = old.hud.objectives_visible;
        self.order_cursor = old.order_cursor;
        self.selected_marks = old.selected_marks.clone();
        self.scale_factor = old.scale_factor;
        // an armed support purchase has already been paid for (`orders.rs`): the new panel keeps it, so a
        // placement or a cancel still settles it
        self.control.selected_vehicle = old.control.selected_vehicle;
        self.control.vehicle_tab = old.control.vehicle_tab;
        // a flight that ends between the last update and the rebuild must still clear the purchase
        self.vtol_was_in_flight = old.vtol_was_in_flight;
    }

    /// Loads the HUD for a mission of `game_size` from the mounted data.  `snapshot` is the
    /// simulation's first state (the map size comes from it).
    pub fn load(
        vfs: &mut Vfs,
        game_size: (f32, f32),
        mission: &str,
        snapshot: HudSnapshot,
    ) -> Result<Self, String> {
        let res = hud_resolution(game_size);
        let layout_name = format!("buttonlayout{}", res.0);
        let wide_extra = wide_extra(game_size, res);
        let view_w = view_width(res, wide_extra);
        let mut assets = UiAssets {
            screen: (view_w, res.1 as i32),
            ..UiAssets::default()
        };
        let parts = {
            let mut ld = Loader::new(vfs, &mut assets);
            let control = ControlPanel::load_wide(&mut ld, res.0, res.1, wide_extra)?;
            let pause_window = PauseWindow::load_wide(&mut ld, res.0, res.1, wide_extra)?;
            let mut fit = ld.art_fit(&layout_name)?;
            let bar_layout = Layout::from_fit_wide(&mut fit, wide_extra > 0)?;
            let art = Art::load(&mut ld, &mut fit)?;
            let atlases = IconAtlases {
                mech: art.mech_atlas,
                vehicle: art.vehicle_atlas,
                pilot: art.pilot_atlas,
            };
            let unit_bar = UnitBar::new(bar_layout, art);
            let mut fit = ld.art_fit(&layout_name)?;
            let info_geometry = InfoGeometry::from_fit(&mut fit, res.0 as i32)
                .ok_or_else(|| format!("{layout_name}: no InfoWindow geometry"))?;
            let fonts = {
                let mut f = ld.art_fit(&layout_name)?;
                f.seek_block("Fonts")
                    .map_err(|e| format!("{layout_name}: Fonts: {e}"))?;
                let id = |key: &str| f.read_i64(key).map(|v| v as u32).unwrap_or(0);
                let (name, component) = (id("InfoWndFont"), id("ComponentFont"));
                InfoFonts {
                    name: ld.font(name),
                    component: ld.font(component),
                }
            };
            let mut info = InfoWindow::new(info_geometry.clone(), fonts);
            info.set_atlases(atlases);
            let mut fit = ld.art_fit(&layout_name)?;
            let up = mc2_ui::widgets::button::Button::from_fit(&mut ld, &mut fit, "InfoButton0");
            let down = mc2_ui::widgets::button::Button::from_fit(&mut ld, &mut fit, "InfoButton1");
            info.set_scroll_buttons(up, down);
            let mut fit = ld.art_fit(&layout_name)?;
            info.set_skill_art(std::array::from_fn(|i| {
                mc2_ui::widgets::object::Object::from_fit(&mut ld, &mut fit, &format!("Skill{i}"))
            }));
            let mut fit = ld.art_fit(&layout_name)?;
            let chat_geometry = MissionChatGeometry::from_fit(&mut fit)?;
            let mut fit = ld.art_fit(&layout_name)?;
            let chat =
                MissionChat::from_fit(&mut fit, ld.font(CHAT_FONTS.0), ld.font(CHAT_FONTS.1))?;
            let mut fit = ld.art_fit(&layout_name)?;
            let objectives = ObjectivesPanel::from_fit(&mut ld, &mut fit)?;
            let mut fit = ld.art_fit(&layout_name)?;
            let result_statics: Vec<StaticInfo> = {
                fit.seek_block("MissionResults").ok();
                let long_or = |key, default| {
                    fit.read_i64(key)
                        .ok()
                        .map(|value| value as i32 as i64)
                        .unwrap_or(default)
                };
                let count = long_or("ResultsStaticCount", 0).max(0) as usize;
                (0..count)
                    .map(|i| StaticInfo::from_fit(&mut ld, &mut fit, &format!("ResultsStatic{i}")))
                    .collect()
            };
            let result_rect = {
                fit.seek_block("ResultsTextBox").ok();
                let long_or = |key, default| {
                    fit.read_i64(key)
                        .ok()
                        .map(|value| value as i32 as i64)
                        .unwrap_or(default)
                };
                GuiRect::new(
                    long_or("left", 0) as i32,
                    long_or("top", 0) as i32,
                    long_or("right", 0) as i32,
                    long_or("bottom", 0) as i32,
                )
            };
            let result_color = fit
                .read_i64("ResultsTextColor")
                .ok()
                .map(|value| value as i32 as u32)
                .unwrap_or(0xffff_ffff);
            let result_font = {
                fit.seek_block("Fonts").ok();
                ld.font(fit.read_i64("MissionResultsFont").unwrap_or(0) as u32)
            };
            (
                control,
                pause_window,
                bar_layout,
                unit_bar,
                info_geometry,
                info,
                chat_geometry,
                chat,
                objectives,
                result_statics
                    .first()
                    .map_or(result_rect.top as f32, |static_info| static_info.y),
                result_statics,
                result_rect,
                result_color,
                result_font,
            )
        };
        let (
            control,
            pause_window,
            bar_layout,
            unit_bar,
            info_geometry,
            info,
            chat_geometry,
            chat,
            objectives,
            result_origin_y,
            result_statics,
            result_rect,
            result_color,
            result_font,
        ) = parts;
        let video_window = bar_layout.video_window.rect;
        let layout = HudLayout::from_parts(
            res,
            &control,
            &bar_layout,
            &info_geometry,
            chat_geometry,
            Some(HudRect::from_gui(pause_window.panel_rect())),
        )
        .with_objectives(&objectives);
        let map_rect = layout
            .tac_map_region
            .ok_or_else(|| format!("{layout_name}: the control layout has no map rectangle"))?;
        let mut tac_map = TacMap::new(TerrainBounds::from_vertices(
            snapshot.vertices_per_side.max(1),
        ));
        tac_map.set_pos(GuiRect::new(
            map_rect.left,
            map_rect.top,
            map_rect.right,
            map_rect.bottom,
        ));
        tac_map.left_right_profile = true;
        let mut art = TacMapArt::load(&mut assets.textures, vfs);
        art.marker_font = mc2_ui::ctx::Loader::new(vfs, &mut assets)
            .font(mc2_ui::tacmap::OBJECTIVE_MARKER_FONT_ID);
        let mut unavailable = BTreeMap::new();
        // `MissionInterfaceManager::initTacMap`: packet 3 of the mission pak is the colour map.
        match mission_colour_map(vfs, mission) {
            Ok(bytes) => {
                if art
                    .set_terrain_packet(&mut assets.textures, &format!("{mission}colmap"), &bytes)
                    .is_none()
                {
                    unavailable.insert(format!("tac map: {mission} colour map is not a TGA"), 1);
                }
            }
            Err(e) => {
                unavailable.insert(format!("tac map: {e}"), 1);
            }
        }
        tac_map.art = art;
        for note in crate::hud_data::UNAVAILABLE {
            unavailable.insert((*note).to_string(), 1);
        }
        let settings = mission_settings(vfs, mission);
        let support_enabled = match settings {
            Ok(s) => SupportEnabled {
                air_strike: s.enable_air_strike,
                sensor_strike: s.enable_sensor_strike,
                guard_tower: s.enable_artillery_piece,
                repair_truck: s.enable_repair_truck,
                scout: s.enable_scout_copter,
                mine_layer: s.enable_mine_layer,
                salvage: s.enable_salvage_craft,
            },
            Err(e) => {
                unavailable.insert(format!("control panel: {e}; support reads enabled"), 1);
                SupportEnabled {
                    air_strike: true,
                    sensor_strike: true,
                    guard_tower: true,
                    repair_truck: true,
                    scout: true,
                    mine_layer: true,
                    salvage: true,
                }
            }
        };
        let cursors = match Cursors::load(vfs, &mut assets.textures, "cursors", res.0) {
            Ok(c) => Some(c),
            Err(e) => {
                unavailable.insert(format!("order cursor art: {e}"), 1);
                None
            }
        };
        let hold_fire_icon = match assets
            .textures
            .load_tga(vfs, &mc2_ui::textures::art_file_path("blip"))
        {
            Some(id) => assets.textures.get(id).and_then(|img| {
                world_view::hold_fire_icon_from_rgba(img.width, img.height, &img.rgba)
            }),
            None => {
                unavailable.insert(
                    "hold-fire icon: data/art/blip.tga did not load; the status bit is not drawn"
                        .into(),
                    1,
                );
                None
            }
        };
        if hold_fire_icon.is_none() && !unavailable.keys().any(|k| k.starts_with("hold-fire icon:"))
        {
            unavailable.insert(
                "hold-fire icon: blip.tga loaded but the drawIcon UV region is empty".into(),
                1,
            );
        }
        for (k, n) in &unavailable {
            log::warn!("hud unavailable: {k} (x{n})");
        }
        Ok(Self {
            assets,
            res,
            wide_extra,
            view_w,
            hud: MissionHud::new(layout),
            control,
            unit_bar,
            support_enabled,
            vtol_was_in_flight: false,
            unit_bar_output: UnitBarOutput::default(),
            pilot_video: PilotVideo::new(),
            info,
            chat,
            tac_map,
            objectives,
            objectives_view: ObjectivesView::default(),
            result_statics,
            result_rect,
            result_origin_y,
            result_color,
            result_font,
            results_time: 0.0,
            video_window,
            results_status_frame_drawn: false,
            results_grey: false,
            results_fade: 0,
            results_status_text: None,
            pause_window,
            ui_input: UiInput::new(view_w as u32, res.1),
            ui_out: UiOutput::default(),
            tac_map_input: TacMapInput::default(),
            renderer: None,
            last_output: HudOutput::default(),
            selection: Selection::default(),
            snapshot,
            info_unit: None,
            mouse: (0.0, 0.0),
            world_press: None,
            drag_box: None,
            chat_lines: Vec::new(),
            order_log: Vec::new(),
            orders_state: OrdersState::default(),
            energy_weapons_held: false,
            force_fire_held: false,
            unavailable,
            game_size,
            cursors,
            order_cursor: mc2_ui::cursor::M_STATE_NORMAL,
            callout_pointer: None,
            selected_marks: Vec::new(),
            scale_factor: 1.0,
            last_line_of_sight: false,
            last_passable: false,
            last_mouse_world: None,
            last_can_jump: false,
            overlay: None,
            cursor_inside: false,
            hold_fire_icon,
            capture_trace: false,
            trace_log: Vec::new(),
            timer_warnings: TimerWarnings::default(),
            hover_prev: None,
            control_press: None,
            control_press_at: None,
            control_dragged: false,
            capture_sounds: false,
            sound_log: Vec::new(),
        })
    }

    /// The layout space the HUD draws in.
    pub fn resolution(&self) -> (u32, u32) {
        self.res
    }

    /// Whether this HUD's layout is the one [`HudRuntime::load`] picks for a screen of `game_size`.
    pub fn fits(&self, game_size: (f32, f32)) -> bool {
        let res = hud_resolution(game_size);
        res == self.res && wide_extra(game_size, res) == self.wide_extra
    }

    pub fn paused(&self) -> bool {
        self.hud.paused
    }

    pub fn tab(&self) -> MissionTab {
        self.hud.tab
    }

    /// `drawGUIOn` (`controlgui.cpp:372`). The C++ flips it only from the debug `toggleGUI` hot key,
    /// which the shipped table omits; headless tests set it to prove that nothing GUI-owned
    /// (the results) runs while it is off.
    pub fn set_gui_visible(&mut self, visible: bool) {
        self.hud.gui_visible = visible;
    }

    pub fn snapshot(&self) -> &HudSnapshot {
        &self.snapshot
    }

    /// Whether the production HUD recorded the in-mission `MISSION SUCCESS` / `MISSION FAILED`
    /// status frame in its most recently rendered frame.
    pub fn results_status_frame_drawn(&self) -> bool {
        self.results_status_frame_drawn
    }

    /// The text the last drawn status frame carried (`IDS_MISSION_SUCCESS` / `IDS_MISSION_FAILED`,
    /// `controlgui.cpp:565-569`); `None` before a frame has drawn one.
    pub fn results_status_text(&self) -> Option<&'static str> {
        self.results_status_text
    }

    /// The objectives panel (`ControlGui::RenderObjectives`): its slide and one-per-second rows.
    pub fn objectives_panel(&self) -> &ObjectivesPanel {
        &self.objectives
    }

    /// The composed HUD's last output (hit target, intents, tab).
    pub fn last_output(&self) -> &HudOutput {
        &self.last_output
    }

    /// Which panel a pointer position (game pixels) is over.
    pub fn hit_test(&self, game_px: (i32, i32)) -> HitTarget {
        if !self.hud.gui_visible {
            return HitTarget::World;
        }
        let at = self.to_hud((game_px.0 as f32, game_px.1 as f32));
        self.hud.hit_test(at, self.unit_bar.icon_count())
    }

    pub fn unit_bar(&self) -> &UnitBar {
        &self.unit_bar
    }

    pub fn tac_map(&self) -> &TacMap {
        &self.tac_map
    }

    pub fn tac_map_input(&self) -> &TacMapInput {
        &self.tac_map_input
    }

    pub fn info_window(&self) -> &InfoWindow {
        &self.info
    }

    /// The `mState_*` of the context-sensitive order cursor (`choose_cursor`).
    pub fn order_cursor(&self) -> u32 {
        self.order_cursor
    }

    /// `Team::home->teamLineOfSight` of the last cursor choice.
    pub fn cursor_line_of_sight(&self) -> bool {
        self.last_line_of_sight
    }

    /// `GameMap->getPassable` of the last cursor choice.
    pub fn cursor_passable(&self) -> bool {
        self.last_passable
    }

    /// Ground point under the pointer of the last update.
    pub fn cursor_mouse_world(&self) -> Option<Vec3> {
        self.last_mouse_world
    }

    /// `canJumpToWPos` of the last cursor choice.
    pub fn cursor_can_jump(&self) -> bool {
        self.last_can_jump
    }

    /// Scene-pass brackets and bars of the last update, in game pixels.
    pub fn selected_marks(&self) -> &[SelectedMark] {
        &self.selected_marks
    }

    /// `Camera::newScaleFactor` used to draw the last marks.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// `true` when the HUD draws the shipped cursor sprites (the stand-in is skipped).
    pub fn draws_cursor(&self) -> bool {
        self.cursors.is_some()
    }

    /// Pointer is inside the window; the order cursor is drawn only then.
    pub fn cursor_inside(&self) -> bool {
        self.cursor_inside
    }

    /// Shipped `blip.tga` region used for the hold-fire status bit.
    pub fn hold_fire_icon(&self) -> Option<&HoldFireIcon> {
        self.hold_fire_icon.as_ref()
    }

    pub fn objectives_view(&self) -> &ObjectivesView {
        &self.objectives_view
    }

    pub fn chat_text(&self) -> Vec<String> {
        self.chat
            .lines
            .iter()
            .map(|l| format!("{}: {}", l.player_name, l.message))
            .collect()
    }

    pub fn control_panel(&self) -> &ControlPanel {
        &self.control
    }

    /// The centre of pause panel button `id` (`PauseWindow` FIT `ID`) in game pixels: where to click it.
    pub fn pause_button_point(&self, id: i32) -> Option<(f32, f32)> {
        let r = self.pause_window.button_rect_for_id(id)?;
        Some(self.hud_to_game(((r.left + r.right) / 2, (r.top + r.bottom) / 2)))
    }

    /// Pause panel button `id`'s rectangle `(x, y, w, h)` in game pixels (the visual QA harness's label region).
    pub fn pause_button_rect(&self, id: i32) -> Option<(f32, f32, f32, f32)> {
        let r = self.pause_window.button_rect_for_id(id)?;
        let a = self.hud_to_game((r.left, r.top));
        let b = self.hud_to_game((r.right, r.bottom));
        Some((a.0, a.1, b.0 - a.0, b.1 - a.1))
    }

    /// The centre of the pause prompt's No (`0`) or Yes (`1`) button in game pixels, while it is open.
    pub fn pause_dialog_point(&self, index: usize) -> Option<(f32, f32)> {
        let (x, y) = self.pause_window.dialog_button_centre(index)?;
        Some(self.hud_to_game((x.round() as i32, y.round() as i32)))
    }

    /// Unavailable behaviour reached, with counts.
    pub fn unavailable_summary(&self) -> Vec<String> {
        self.unavailable
            .iter()
            .map(|(k, n)| format!("{k} (x{n})"))
            .collect()
    }

    fn note(&mut self, what: String) {
        *self.unavailable.entry(what).or_insert(0) += 1;
    }

    /// The fit of the HUD's layout space (the layout's block plus the wide screen's extra) into the game screen.
    fn fit(&self) -> UiLayout {
        UiLayout::new(
            (self.view_w as u32, self.res.1),
            (
                self.game_size.0.round().max(1.0) as u32,
                self.game_size.1.round().max(1.0) as u32,
            ),
            ScaleMode::Fit,
        )
    }

    /// A game-pixel point in the HUD's layout space.
    fn to_hud(&self, p: (f32, f32)) -> (i32, i32) {
        let (x, y) = self.fit().to_ui(p.0, p.1);
        (x.round() as i32, y.round() as i32)
    }

    /// A point of the HUD's layout space (a button, the map) in game pixels: where to click it.
    pub fn hud_to_game(&self, p: (i32, i32)) -> (f32, f32) {
        self.fit().to_target(p.0 as f32, p.1 as f32)
    }

    /// `ControlGui::animateTacMap` (`controlgui.cpp:307-310`): the tac-map flash of objective `objective_id` (1-based).
    pub fn animate_tac_map(&mut self, objective_id: i32, flashes: i32) {
        self.tac_map.animate_objective(objective_id, flashes);
    }

    /// The callout's flashing control button and its colour for this frame's draw (`None` when no callout flashes).
    pub fn set_callout_flash(&mut self, flash: Option<(i32, u32)>) {
        self.control.button_color = flash;
    }

    /// The centre of a callout's target in game pixels (`missiongui.cpp:466-474`, `:535-538`): a control button (`getButton`, the
    /// centre of its four corners) or, for `!is_button`, a control rect (`getRect`; the resource-point total `RPTOTAL_CALLOUT` is the
    /// rect at the points' origin, `controlgui.cpp:2447-2450`). `None` when the panel has no such target, which ends the C++'s callout.
    pub fn callout_target_point(&self, target: i32, is_button: bool) -> Option<(f32, f32)> {
        const RPTOTAL_CALLOUT: i32 = 21185;
        let centre = if is_button {
            let b = self.control.buttons.iter().find(|b| b.cxx_id == target)?;
            (
                (b.rect.left + b.rect.right) / 2,
                (b.rect.top + b.rect.bottom) / 2,
            )
        } else if target == RPTOTAL_CALLOUT {
            self.control.resource_points_origin
        } else {
            let r = &self.control.rects.get(usize::try_from(target).ok()?)?.0;
            ((r.left + r.right) / 2, (r.top + r.bottom) / 2)
        };
        Some(self.hud_to_game(centre))
    }

    /// `setMousePos`: the game's pointer (drawing and click hit-testing) moves to `pos` and holds until the next real
    /// `CursorMoved`.
    pub fn warp_mouse(&mut self, pos: (f32, f32)) {
        self.mouse = pos;
        self.cursor_inside = true;
        // the UI router's pending pointer (`MissionHud` button routing) follows the warp too
        let (hx, hy) = self.to_hud(pos);
        self.ui_input.handle(&InputEvent::CursorMoved {
            x: f64::from(hx),
            y: f64::from(hy),
        });
    }

    /// The pointer the HUD draws and hit-tests with, in game pixels.
    pub fn mouse(&self) -> (f32, f32) {
        self.mouse
    }

    /// The pointer the UI router routes button presses with, in HUD layout pixels.
    pub fn router_pointer(&self) -> (i32, i32) {
        self.ui_input.pending_pointer()
    }

    /// The `cxx_id` of the control-panel button under the HUD's pointer, if any.
    pub fn button_under_mouse(&self) -> Option<i32> {
        match self.control_at(self.to_hud(self.mouse)) {
            Some(ControlPress::Button(id)) => Some(id),
            _ => None,
        }
    }

    /// Sets the tutorial pointer drawn instead of the player's (`None`: the player's).
    pub fn set_callout_pointer(&mut self, pointer: Option<(f32, f32)>) {
        self.callout_pointer = pointer;
    }

    /// The synthetic press/release `MissionInterfaceManager::update` performs when a tutorial
    /// callout ends with `targetIsPressed`. Only a live, drawn control is eligible, matching
    /// `ControlGui::getButton(targetButtonId)` before C++ calls `pushButton`.
    pub fn callout_button_events(&self, cxx_id: i32) -> Option<Vec<InputEvent>> {
        let button =
            self.control.buttons.iter().find(|button| {
                button.cxx_id == cxx_id && button.state != ButtonVisualState::Hidden
            })?;
        let point = self.hud_to_game((
            (button.rect.left + button.rect.right) / 2,
            (button.rect.top + button.rect.bottom) / 2,
        ));
        Some(vec![
            InputEvent::CursorMoved {
                x: f64::from(point.0),
                y: f64::from(point.1),
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: true,
            },
            InputEvent::Button {
                button: MouseButton::Left,
                pressed: false,
            },
        ])
    }

    fn resolve(&self, text: &HudText) -> String {
        text.resolve(&|id| self.assets.string(id).to_string())
    }

    /// Where the camera shows `p`, in game pixels.
    fn project(&self, camera: &GameCamera, p: (f32, f32, f32)) -> Option<((f32, f32), bool)> {
        let (s, inside) = camera.project_z(Vec3::new(p.0, p.1, p.2));
        // `w < 0` is in front of the camera in the game's clip space (`project_z`'s `rhw` is |1/w|)
        if !s[0].is_finite() || !s[1].is_finite() || !(0.0..=1.0).contains(&s[2]) {
            return None;
        }
        let k = self.game_size.1 / camera.viewport.1.max(1.0);
        Some(((s[0] * k, s[1] * k), inside))
    }

    fn visible_units(&self, camera: &GameCamera) -> Vec<i32> {
        self.snapshot
            .units
            .iter()
            .filter(|u| !u.bar.destroyed)
            .filter(|u| {
                self.project(camera, u.position)
                    .is_some_and(|(_, inside)| inside)
            })
            .map(|u| u.part_id)
            .collect()
    }

    /// The nearest unit of ours within the click radius of game pixel `pt`.
    fn pick_unit(&self, camera: &GameCamera, pt: (f32, f32)) -> Option<i32> {
        let radius = CLICK_PICK_RADIUS * self.game_size.1;
        self.snapshot
            .units
            .iter()
            .filter(|u| !u.bar.destroyed)
            .filter_map(|u| {
                self.project(camera, u.position)
                    .map(|(p, _)| (u.part_id, (p.0 - pt.0).hypot(p.1 - pt.1)))
            })
            .filter(|(_, d)| *d <= radius)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    /// Applies a world gesture to the selection.
    fn world_select(&mut self, camera: &GameCamera, gesture: Gesture, add: bool) {
        let candidates: Vec<(i32, (f32, f32))> = self
            .snapshot
            .units
            .iter()
            .filter(|u| !u.bar.destroyed)
            .filter_map(|u| {
                self.project(camera, u.position)
                    .map(|(p, _)| (u.part_id, p))
            })
            .collect();
        match gesture {
            Gesture::Box(a, b) => {
                let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
                let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
                let hit: Vec<i32> = candidates
                    .iter()
                    .filter(|(_, p)| p.0 >= x0 && p.0 <= x1 && p.1 >= y0 && p.1 <= y1)
                    .map(|(id, _)| *id)
                    .collect();
                if add {
                    self.selection.add(hit);
                } else {
                    self.selection.select_only(hit);
                }
            }
            Gesture::Click(pt) => match self.pick_unit(camera, pt) {
                Some(id) if add && self.selection.is_selected(id) => self.selection.remove(id),
                Some(id) if add => self.selection.add([id]),
                Some(id) => self.selection.select_only([id]),
                None if add => {}
                None => self.selection.selected.clear(),
            },
        }
    }

    /// One game frame: builds the HUD's inputs, runs it, routes its intents.
    pub fn update(&mut self, input: HudFrameInput<'_>) -> HudFrameOutput {
        self.game_size = input.game_size;
        let dt = input.dt;
        self.snapshot = input.snapshot;
        // units that are gone leave the selection (a destroyed unit is deselected by the bar once its
        // death animation ends; one that no longer exists cannot stay selected)
        let existing: Vec<i32> = self.snapshot.units.iter().map(|u| u.part_id).collect();
        self.selection.selected.retain(|p| existing.contains(p));
        // intents the bar produced while drawing (a death animation ended): `LeaveAllGroups`
        let mut late = HudFrameOutput::default();
        let units_now = self.snapshot.units.clone();
        for i in std::mem::take(&mut self.unit_bar_output.intents) {
            self.apply_unit_bar_intent(&i, &units_now, &mut late);
        }

        // ---- the pointer and the world gesture, in event order ----
        let icon_count = self.unit_bar.icon_count();
        let mut gestures = Vec::new();
        let mut hud_events = Vec::with_capacity(input.events.len());
        let mut left_double = false;
        // the control button a completed click lands on: the press and the release must be on the same one
        let mut control_click = None;
        for e in input.events {
            match e {
                InputEvent::CursorMoved { x, y } => {
                    self.cursor_inside = true;
                    self.mouse = (*x as f32, *y as f32);
                    let (hx, hy) = self.to_hud(self.mouse);
                    if let Some((ox, oy)) = self.control_press_at {
                        // the router's test (`MissionHud::route_control_panel_event`), on every move
                        let size = self.ui_input.size();
                        let t = self.ui_input.drag_threshold;
                        self.control_dragged |= ((hx - ox) as f32 / size.0.max(1) as f32).abs() > t
                            || ((hy - oy) as f32 / size.1.max(1) as f32).abs() > t;
                    }
                    hud_events.push(InputEvent::CursorMoved {
                        x: f64::from(hx),
                        y: f64::from(hy),
                    });
                    if let Some(start) = self.world_press {
                        let moved = (self.mouse.0 - start.0).hypot(self.mouse.1 - start.1);
                        if moved > input.drag_threshold_px || self.drag_box.is_some() {
                            self.drag_box = Some((start, self.mouse));
                        }
                    }
                    continue;
                }
                InputEvent::CursorEntered => {
                    self.cursor_inside = true;
                }
                InputEvent::CursorLeft => {
                    self.cursor_inside = false;
                }
                InputEvent::Button {
                    button: MouseButton::Left,
                    pressed,
                } => {
                    let at = self.to_hud(self.mouse);
                    let over_world = !self.hud.gui_visible
                        || self.hud.hit_test(at, icon_count) == HitTarget::World;
                    if *pressed {
                        self.world_press = over_world.then_some(self.mouse);
                        self.control_press = self.control_at(at);
                        self.control_press_at = self.control_press.map(|_| at);
                        self.control_dragged = false;
                    } else {
                        control_click = self
                            .control_press
                            .take()
                            .filter(|p| Some(*p) == self.control_at(at) && !self.control_dragged);
                        self.control_press_at = None;
                        self.control_dragged = false;
                        if let Some(start) = self.world_press.take() {
                            gestures.push(match self.drag_box.take() {
                                Some((a, _)) => Gesture::Box(a, self.mouse),
                                None => Gesture::Click(start),
                            });
                        }
                        self.drag_box = None;
                    }
                }
                _ => {}
            }
            hud_events.push(e.clone());
        }
        left_double |= input.user_input.is_left_double_click();
        // The command click (`commandClicked`, missiongui.cpp:852/872): the AOE profile's completed
        // right click, or the classic profile's left click that picks no unit of ours while
        // something is selected (it orders instead of deselecting).
        let over_world = !self.hud.gui_visible
            || self.hud.hit_test(self.to_hud(self.mouse), icon_count) == HitTarget::World;
        let mut command_click = input.left_right_profile
            && over_world
            && input.user_input.right_mouse_released()
            && !input.user_input.was_right_drag()
            && !input.user_input.is_left_drag();
        // `updateAOEStyle`'s select click also runs `addAirstrike` while a strike is armed (missiongui.cpp:1609)
        let strike_armed = !self.snapshot.vtol_in_flight
            && matches!(
                self.control.selected_vehicle,
                Some(
                    mc2_ui::control::VehicleKind::Airstrike
                        | mc2_ui::control::VehicleKind::SensorProbe
                )
            );
        for g in gestures {
            if let Gesture::Click(pt) = g {
                if input.left_right_profile && strike_armed && over_world {
                    command_click = true;
                    continue;
                }
                // Classic profile: `addAirstrike` runs before target selection (missiongui.cpp:1340), so an armed strike takes
                // the click whatever it hit and with nothing selected.
                if !input.left_right_profile && strike_armed && over_world {
                    command_click = true;
                    continue;
                }
                if !input.left_right_profile
                    && !self.selection.selected.is_empty()
                    && self.pick_unit(input.camera, pt).is_none()
                {
                    command_click = true;
                    continue;
                }
            }
            self.world_select(input.camera, g, input.shift);
        }
        let picked = if command_click {
            // `findObjectByMouse` (objmgr.cpp:2160-2214): the first object whose projected selection
            // box holds the mouse (inclusive) and, for a structure, whose visible faces (`PerPolySelect`) hold it too;
            // a mover is taken on the box alone (objmgr.cpp:2253).
            let k = self.game_size.1 / input.camera.viewport.1.max(1.0);
            pick_at(&self.snapshot.picks, k, self.mouse)
        } else {
            None
        };
        let env_trace = crate::game::p3_trace_enabled();
        if command_click && (env_trace || self.capture_trace) {
            let line = describe_command_click(
                input.scenario_time,
                self.mouse,
                &self.selection.selected,
                picked.as_ref().map(|p| p.watch_id),
                input.mouse_world.map(|p| (p.x, p.y)),
            );
            if env_trace {
                eprintln!("{line}");
            }
            if self.capture_trace {
                self.trace_log.push(line);
            }
        }
        let frame = OrderFrame {
            scenario_time: input.scenario_time,
            waypoint_queue: input.waypoint_queue,
            command_click,
            picked,
            mouse_world: input.mouse_world,
        };

        // `ForceShot` has no release function (`commands[FORCE_FIRE_KEY]`, missiongui.cpp:209):
        // `makeTargetCursor` / `makeNoTargetCursor` test `KEY_HELD` on that key each frame.
        self.force_fire_held = input
            .fired
            .iter()
            .any(|f| matches!(f, Fired::Run(Command::ForceShot)));

        // ---- hot keys that change the selection ----
        let mut mission_commands = Vec::new();
        for f in input.fired {
            match *f {
                Fired::ForceGroup { group, ctrl, shift } => {
                    self.apply_force_group(group, ctrl, shift, &mut late.commands)
                }
                Fired::WaypointsReleased => {
                    self.execute_waypoint_queue(input.scenario_time, &mut late.commands)
                }
                Fired::Run(Command::SelectVisible) => {
                    let visible = self.visible_units(input.camera);
                    self.selection.select_only(visible);
                }
                Fired::Run(Command::AddVisibleToSelection) => {
                    let visible = self.visible_units(input.camera);
                    self.selection.add(visible);
                }
                _ => {}
            }
            // Run and Release both run a mission command: a held key's release function
            // (StopChangeSpeed, StopGuard, StopJump, ...) clears the mode the press set.
            if let Fired::Run(c) | Fired::Release(c) = *f {
                if c == Command::EnergyWeapons {
                    self.energy_weapons_held = matches!(f, Fired::Run(_));
                }
                if let Some(m) = mission_command(c) {
                    mission_commands.push(m);
                }
            }
        }

        // ---- the sounds of this frame, in the order the C++ makes them ----
        let mut cues = Vec::new();
        let released = input.events.iter().any(|e| {
            matches!(
                e,
                InputEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                }
            )
        }) && !input.user_input.is_left_drag()
            && !input.user_input.was_left_drag();
        self.pointer_sounds(released, control_click, &mut cues);

        // ---- the parts' inputs ----
        let mut units = self.snapshot.units.clone();
        for unit in &mut units {
            unit.bar.pilot.message_playing = input.speaking_pilots.contains(&unit.pilot_slot);
        }
        let bar_units: Vec<_> = units
            .iter()
            .map(|u| {
                let mut b = u.bar.clone();
                b.pilot.name = self.resolve(&u.pilot_name);
                b.selected = self.selection.is_selected(u.part_id);
                // This is `Mover::isInUnitGroup`; membership comes from the live/saved sim
                // snapshot, not a HUD mirror.
                b
            })
            .collect();
        let hud_mouse = self.to_hud(self.mouse);
        let bar_input = UnitBarInput {
            units: bar_units,
            pointer: Pointer {
                x: hud_mouse.0,
                y: hud_mouse.1,
                // `UserInput::isLeftClick` is a press-edge (`mclib/userinput.cpp:232-244`).
                // `left_clicked` is the release-edge used only for world gestures above; feeding
                // it to the bar made the release after a double-click select the one icon again,
                // undoing `ForceGroupBar::update`'s group selection.
                left_click: input.user_input.is_left_click(),
                left_double_click: left_double,
                right_click: input.user_input.is_right_click(),
                right_drag: input.user_input.is_right_drag(),
            },
            shift_down: input.shift,
            profile: if input.left_right_profile {
                MouseProfile::LeftRight
            } else {
                MouseProfile::Classic
            },
        };
        let selected_units: Vec<&HudUnit> = units
            .iter()
            .filter(|u| self.selection.is_selected(u.part_id))
            .collect();
        let caps: Vec<UnitCapabilities> = selected_units.iter().map(|u| u.caps).collect();
        // the info window shows the unit asked for, else the one selected unit
        let info_part = self
            .info_unit
            .or_else(|| self.selection.selected.first().copied());
        // `InfoWindow::update` clears a foreign unit that is no longer seen (infowindow.cpp:483-489):
        // the live snapshot only holds units the window may keep, so a lost contact drops out here.
        let info_unit = info_part.and_then(|p| {
            units
                .iter()
                .find(|u| u.part_id == p)
                .or_else(|| self.snapshot.info_units.get(&p))
        });
        if info_unit.is_none() && self.info_unit.is_some() {
            self.info_unit = None;
        }
        self.info.set_unit(info_unit.map(|u| {
            let mut i = u.info.clone();
            i.name = self.resolve(&u.name);
            i.pilot.name = self.resolve(&u.pilot_name);
            i
        }));
        // `InfoWindow::setUnit` / `update`: the icon follows the unit the window shows (a unit the
        // window refused leaves it empty).
        let shown = self.info.unit().and(info_unit).map(|u| &u.bar);
        self.info.sync_icon(&mut self.assets.textures, shown);
        self.tac_map_input = self.build_tac_map(input.camera, &units);
        self.objectives_view = ObjectivesView {
            objectives: self.snapshot.objectives.clone(),
            mission_time: self.snapshot.mission_time,
            ..ObjectivesView::default()
        };

        // The unit bar updates here rather than inside `MissionHud::update` (which cannot borrow
        // the texture store beside its context): `ForceGroupBar::update` runs in the same slot of
        // `ControlGui::update`, its intents join the frame's, and its hover state goes to the draw.
        let bar_result = self.unit_bar.update(&bar_input, &mut self.assets.textures);
        // `ForceGroupIcon::click` plays `LOG_SELECT` (`code/mechicon.cpp:427`)
        cues.extend(bar_result.sounds.iter().map(|&id| Cue::Sound(id)));
        let hud_input = HudInput {
            mission_commands,
            tac_map_data: self.tac_map_input.clone(),
            unit_bar_icon_count: self.unit_bar.icon_count(),
            unit_bar_intents: bar_result.intents.clone(),
            ..HudInput::default()
        };
        self.unit_bar_output = UnitBarOutput {
            intents: Vec::new(),
            sounds: Vec::new(),
            ..bar_result
        };
        // `updateVTol` ends the flight: `controlGui.unPressAllVehicleButtons()` (missiongui.cpp:1111)
        if self.vtol_was_in_flight && !self.snapshot.vtol_in_flight {
            self.control.unpress_vehicle_buttons();
        }
        self.vtol_was_in_flight = self.snapshot.vtol_in_flight;
        let control_input = ControlPanelInput {
            selected_units: &caps,
            mouse: hud_mouse,
            paused: self.hud.paused,
            vtol_in_flight: self.snapshot.vtol_in_flight,
            resource_points: self.snapshot.resource_points,
            // Every vehicle button follows the mission file's flags (`ControlGui::updateVehicleTab`, controlgui.cpp:1860-1890). A strike
            // is armed by its press and called by the next placement click (`CallStrike`); the support vehicles are called in
            // by `BeginVtol`.
            vehicle_availability: VehicleAvailability {
                can_have_airstrike: self.support_enabled.air_strike,
                can_have_sensor_probe: self.support_enabled.sensor_strike,
                can_have_guard_tower: self.support_enabled.guard_tower,
                can_have_repair_truck: self.support_enabled.repair_truck,
                can_have_scout: self.support_enabled.scout,
                can_have_minelayer: self.support_enabled.mine_layer,
                can_have_recovery_team: self.support_enabled.salvage,
                // `ForceGroupBar::getIconCount() < MAX_ICONS` (forcegroupbar.h:21)
                unit_capacity_available: self.snapshot.units.len() < 16,
                pilots_remaining: self.snapshot.pilots_remaining,
                player_destroyed: false,
            },
            click: None,
            vehicle_click: None,
        };
        // The vehicle tab is a pressed tab button in C++ (`switchTabs` presses the new one and the tab
        // content is chosen by `getButton(VEHICLE_TAB)->state & PRESSED`, controlgui.cpp:2808-2844 and
        // :326). The router reads `ControlPanel::vehicle_tab`, which only a click on the tab button sets, so a
        // Tab key left it false and the drawn vehicle buttons could not be clicked. Follow the drawn tab.
        self.control.vehicle_tab = self.vehicle_tab_shown();
        self.control.line_of_sight = input.line_of_sight;
        self.control.left_right_profile = input.left_right_profile;
        let output = {
            let mut update_ctx = UpdateCtx {
                input: &mut self.ui_input,
                assets: &self.assets,
                dt,
                out: &mut self.ui_out,
            };
            self.hud.update(MissionHudInput {
                state: hud_input,
                control: &mut self.control,
                control_input,
                tac_map: &mut self.tac_map,
                update_ctx: &mut update_ctx,
                pause_window: &mut self.pause_window,
                unit_bar: None,
                events: &hud_events,
                dt,
            })
        };
        cues.extend(cues_from_ui(&self.ui_out));
        self.ui_out = UiOutput::default();
        self.advance_results(dt, &mut cues);
        self.timer_warnings(&mut cues);

        // Info window mouse (`InfoWindow::update`, infowindow.cpp:483-590): HUD layout coords.
        // C++ `ControlGui::update` only calls it while the Info tab is pressed
        // (`controlgui.cpp:1209-1212`); other tabs must not scroll a hidden window.
        let hud_mouse = self.to_hud(self.mouse);
        if self.hud.gui_visible && self.hud.tab == MissionTab::Info {
            let drag = self.to_hud(input.user_input.drag_start());
            self.info.update(&InfoMouse {
                x: hud_mouse.0,
                y: hud_mouse.1,
                left_down: input.user_input.left_button_down(),
                left_released: input.user_input.left_mouse_released(),
                left_held: input.user_input.left_held_time(),
                dragging: input.user_input.is_left_drag(),
                drag_origin: drag,
            });
        }

        // Selection brackets (`setSelected` → DRAW_BRACKETS|DRAW_BARS) and the order cursor.
        // `upperLeft`/`lowerRight` come from `recalcBounds`; without that projected box the C++
        // still has the appearance's -999 uninitialised corners (`appear.h:89`), which is not a
        // player-visible 48×64 guess. Missing bounds stay explicit.
        let k = self.game_size.1 / input.camera.viewport.1.max(1.0);
        self.scale_factor = input.camera.mover_scale_factor.max(0.05);
        let mut marks = Vec::new();
        let mut missing_bracket_box = false;
        for u in &selected_units {
            match u.screen_box {
                Some(b) => marks.push(world_view::mark_for(u, b.map(|v| v * k))),
                None => missing_bracket_box = true,
            }
        }
        self.selected_marks = marks;
        if missing_bracket_box {
            self.note("selection brackets: projected box is missing".into());
        }
        let hover = pick_at(&self.snapshot.picks, k, self.mouse);
        let hit = if !self.hud.gui_visible {
            HitTarget::World
        } else {
            self.hud.hit_test(hud_mouse, self.unit_bar.icon_count())
        };
        self.last_line_of_sight = input.line_of_sight;
        self.last_passable = input.passable;
        self.last_mouse_world = input.mouse_world;
        self.last_can_jump =
            world_view::can_jump_to_wpos(&selected_units, input.mouse_world, input.passable);
        let choice = world_view::order_cursor(
            &selected_units,
            hover.as_ref(),
            &self.control,
            input.camera,
            input.mouse_world,
            hit,
            input.waypoint_queue,
            self.energy_weapons_held,
            self.force_fire_held,
            input.turn,
            input.camera_rotate,
            input.line_of_sight,
            input.passable,
        );
        self.order_cursor = choice.cursor;
        if let Some(c) = self.cursors.as_mut() {
            c.update(self.order_cursor, dt);
        }

        // the bar's own intents are in the output; its render-time ones are added by `draw`
        let mut out = self.route(output.clone(), &units, &frame, input.elevation);
        cues.append(&mut out.cues);
        if output.intents.iter().any(|intent| {
            matches!(
                intent,
                HudIntent::Mission(MissionCommand::GotoNextNavMarker)
            )
        }) && self.snapshot.next_nav_marker.is_none()
        {
            cues.push(Cue::Sound(sfx::INVALID_GUI as i32));
        }
        out.unavailable.splice(0..0, late.unavailable);
        out.commands.splice(0..0, late.commands);
        out.over_hud = output.hit.is_some_and(|h| h != HitTarget::World);
        out.paused = self.hud.paused;
        if self.capture_sounds {
            self.sound_log.extend(cues.iter().cloned());
        }
        out.cues = cues;
        self.last_output = output;
        self.tick_chat(dt);
        out
    }

    /// The in-mission results (`ControlGui::render`, `RenderObjectives`, `renderResults`;
    /// `controlgui.cpp:431-436`, `511-557`, `603-662`): once the mission is ending the objective list
    /// comes in and counts its rows off, one per second, until [`Self::results_done`].
    fn advance_results(&mut self, dt: f32, cues: &mut Vec<Cue>) {
        // `renderResults` is called only inside `drawGUIOn` (`controlgui.cpp:374`, `431-436`): with
        // the GUI hidden the results neither start, advance nor draw.
        // `renderStatusInfo` is never cleared once set (controlgui.cpp:431-436), so a later reset
        // of the objectives does not stop the presentation.
        let started =
            (self.snapshot.results_started || self.results_time > 0.0) && self.hud.gui_visible;
        if started && self.results_time == 0.0 {
            self.results_time = 0.001;
        }
        // `renderStatusInfo` is never cleared once set, so `resultsDone` still sees it.
        self.objectives_view.render_status_info = started || self.results_time > 0.0;
        self.objectives_view.results_time = self.results_time;
        if !started {
            return;
        }
        // `renderResults` kills the info window's unit (`controlgui.cpp:514-515`)
        self.info.set_unit(None);
        // `RenderObjectives`: the objective timer
        {
            let mut ctx = UpdateCtx {
                input: &mut self.ui_input,
                assets: &self.assets,
                dt,
                out: &mut self.ui_out,
            };
            self.objectives.update(&mut ctx, &self.objectives_view);
        }
        cues.extend(cues_from_ui(&self.ui_out));
        self.ui_out = UiOutput::default();
        // `renderResults`: the results clock, and (the status frame having arrived) the objectives
        // are opened, which restarts their timer (`startObjectives(1)`, controlgui.cpp:549-550)
        self.results_time += dt;
        // `RenderObjectives` returns before it draws until `resultsTime >= .24f`
        // (controlgui.cpp:611-615), and the draw it does not make does not touch the panel: it is
        // opened only after the status frame has arrived.
        if self.results_time >= RESULTS_MOVE_END && !self.objectives.is_open() {
            self.objectives.start(true, true);
            self.hud.objectives_visible = true;
        }
    }

    /// The full-screen overlays of `renderResults` (`drawEmptyRect`/`drawRect` over
    /// `Environment.screenWidth/Height`, controlgui.cpp:540,589-596) as a draw list in
    /// target pixels, so a wide window is covered edge to edge. `grey` selects the grey-out,
    /// otherwise the fade.
    pub fn results_overlay(&self, width: i32, height: i32, grey: bool) -> DrawList {
        let mut dl = DrawList::new(width, height);
        let colour = if grey {
            if self.results_grey {
                0x4400_0000
            } else {
                0
            }
        } else {
            self.results_fade
        };
        dl.fill_rect(GuiRect::new(0, 0, width, height), colour);
        dl
    }

    /// `ControlGui::renderResults` (`controlgui.cpp:510-600`): the results art and text slide
    /// down, settle ten pixels above their authored position, then grey the game behind them.
    fn draw_results_status(&mut self, dl: &mut DrawList) {
        self.results_status_frame_drawn = false;
        self.results_status_text = None;
        self.results_grey = false;
        self.results_fade = 0;
        if !self.objectives_view.render_status_info || !self.hud.gui_visible {
            return;
        }
        let position = results_status_position(self.results_time, self.res.0);
        self.results_grey = self.results_time >= RESULTS_MOVE_END;
        // controlgui.cpp:589-596: the last second of a normal mission fades to black.
        let fade_time = self.objectives.objective_time()
            - OBJECTIVE_MOVE[OBJECTIVE_MOVE.len() - 1].time
            - 3.0
            - self.snapshot.objectives.len() as f32;
        self.results_fade = if !self.snapshot.never_ending_story && fade_time > 0.0 {
            interpolate_color(0x0000_0000, 0xff00_0000, fade_time / 1.0)
        } else {
            0
        };
        let delta = position - self.result_origin_y;
        for art in &self.result_statics {
            let mut art = art.clone();
            art.move_by(0.0, delta);
            art.draw(dl);
        }
        let rect = GuiRect::new(
            self.result_rect.left,
            self.result_rect.top + delta.round() as i32,
            self.result_rect.right,
            self.result_rect.bottom + delta.round() as i32,
        );
        dl.fill_rect(rect, 0xff00_0000);
        // `Team::home->objectives.Status() == OS_SUCCESSFUL` (controlgui.cpp:565-569): an
        // undetermined objective is not a success. The sim's list status is evaluated here, not
        // only the objectives shown in the panel (an inactive activated-later primary keeps the
        // mission undetermined although it is not drawn).
        let text = if self.snapshot.home_successful {
            "MISSION SUCCESS"
        } else {
            "MISSION FAILED"
        };
        if let Some(font) = self.assets.font(self.result_font) {
            text::draw(
                dl,
                font,
                text,
                &TextAttrs {
                    color: self.result_color,
                    word_wrap: false,
                    align: 3,
                    size: self.result_font.size as f32,
                    ..TextAttrs::default()
                },
                rect,
                (rect.left, rect.top),
            );
        }
        self.results_status_frame_drawn = true;
        self.results_status_text = Some(text);
    }

    /// Whether the vehicle tab is the one drawn (`MissionHud::render_control_buttons` draws the vehicle buttons from
    /// `self.tab`). `ControlPanel::vehicle_tab` is not that: only a click on the tab button sets it, so a Tab key
    /// shows the vehicle buttons while it stays false, and it stays true after the tab is left.
    fn vehicle_tab_shown(&self) -> bool {
        self.hud.tab == MissionTab::Vehicle
    }

    /// The control-panel button under HUD pixel `now`, if the pointer is over a shown one (a support-vehicle button
    /// only while the vehicle tab is open).
    fn control_at(&self, now: (i32, i32)) -> Option<ControlPress> {
        if !self.hud.gui_visible {
            return None;
        }
        let shown = |s: ButtonVisualState, r: &ControlRect| {
            s != ButtonVisualState::Hidden && r.contains(now.0, now.1)
        };
        if let Some(b) = self
            .control
            .buttons
            .iter()
            .find(|b| shown(b.state, &b.rect))
        {
            return Some(ControlPress::Button(b.cxx_id));
        }
        if self.vehicle_tab_shown() {
            if let Some(v) = self
                .control
                .vehicles
                .iter()
                .find(|v| shown(v.state, &v.rect))
            {
                return Some(ControlPress::Vehicle(v.id));
            }
        }
        None
    }

    /// The pointer's sounds (`ControlGui::update`, `code/controlgui.cpp:977-1000` and `:1775-1800`): a highlight as the
    /// pointer enters a shown button (a support-vehicle button only while the vehicle tab is open), and the click of a
    /// completed left click. `click` is the button that press and release both landed on (see `control_press`);
    /// `released` is the release edge without a drag.
    fn pointer_sounds(&mut self, released: bool, click: Option<ControlPress>, cues: &mut Vec<Cue>) {
        let now = self.to_hud(self.mouse);
        let before = self.hover_prev.replace(now).unwrap_or(now);
        if !self.hud.gui_visible {
            return;
        }
        let entered = |r: &ControlRect| r.contains(now.0, now.1) && !r.contains(before.0, before.1);
        for b in &self.control.buttons {
            if b.state != ButtonVisualState::Hidden && entered(&b.rect) {
                cues.push(Cue::Sound(sfx::LOG_HIGHLIGHTBUTTONS as i32));
            }
        }
        if self.vehicle_tab_shown() {
            for v in &self.control.vehicles {
                if v.state != ButtonVisualState::Hidden && entered(&v.rect) {
                    cues.push(Cue::Sound(sfx::LOG_HIGHLIGHTBUTTONS as i32));
                }
            }
        }
        if !released {
            return;
        }
        match click {
            Some(ControlPress::Button(id)) => {
                if let Some(b) = self.control.buttons.iter().find(|b| b.cxx_id == id) {
                    cues.extend(
                        control_click_samples(b.cxx_id, b.state)
                            .into_iter()
                            .map(Cue::Sound),
                    );
                }
            }
            Some(ControlPress::Vehicle(id)) => {
                if let Some(v) = self.control.vehicles.iter().find(|v| v.id == id) {
                    cues.extend(vehicle_click_samples(v.state).into_iter().map(Cue::Sound));
                }
            }
            None => {}
        }
    }

    /// The mission timer's Betty warnings (`ControlGui::render`, `code/controlgui.cpp:375-400`), made while the GUI is
    /// drawn and the mission has a time limit (`m_timeLimit > 0`). `actualTime` is `scenarioTime` (`mission.cpp:461`).
    fn timer_warnings(&mut self, cues: &mut Vec<Cue>) {
        let limit = self.snapshot.time_limit;
        if !self.hud.gui_visible || limit <= 0.0 {
            return;
        }
        // `int time = (int)fTime`: whole seconds left, truncated toward zero
        let time = (limit - self.snapshot.mission_time) as i32;
        let warned = &mut self.timer_warnings;
        if time <= 120 && !warned.two_min_played {
            warned.two_min_played = true;
            cues.push(Cue::Betty(betty::BETTY_TWO_MIN_LEFT as i32));
        } else if time <= 30 && !warned.thirty_second_played {
            warned.thirty_second_played = true;
            cues.push(Cue::Betty(betty::BETTY_THIRTY_LEFT as i32));
        }
    }

    /// `ControlGui::resultsDone` (`controlgui.cpp:2901-2912`): the results are on screen and the
    /// objective timer has passed the objective count plus four seconds.
    pub fn results_done(&self) -> bool {
        self.objectives_view.render_status_info
            && self.objectives.objective_time() > (self.snapshot.objectives.len() + 4) as f32
    }

    /// Reports what the bridge refused: counted, said in the chat log, returned in `out`.
    fn refuse(&mut self, items: Vec<Unavailable>, out: &mut HudFrameOutput) {
        for u in &items {
            self.note(format!("order {}: {}", u.action, u.reason));
            self.say("ORDERS", "That order is not available yet.");
        }
        out.unavailable.extend(items);
    }

    /// The bridge's world facts for this frame.  Movers' jump capability, attack ranges and picked
    /// objects are not in the HUD's snapshot, so they are left absent (the bridge then reports the
    /// order that needs them as unavailable rather than assuming).
    fn bridge_context(
        &self,
        units: &[HudUnit],
        frame: &OrderFrame,
        intents: &[HudIntent],
        elevation: &dyn Fn(f32, f32) -> f32,
    ) -> BridgeContext {
        let parts: Vec<PartId> = units.iter().map(|u| PartId(u.part_id)).collect();
        let world_point = intents.iter().find_map(|i| match i {
            HudIntent::TacMapMoveOrder((x, y)) => Some(mc2_sim::math::Vec3 {
                x: *x,
                y: *y,
                z: elevation(*x, *y),
            }),
            _ => None,
        });
        let mouse_world = frame.mouse_world.map(|p| mc2_sim::math::Vec3 {
            x: p.x,
            y: p.y,
            z: p.z,
        });
        let viewport_click = match &frame.picked {
            Some(p) => Some(ViewportOrderClick::Object(WatchId(p.watch_id))),
            None if frame.command_click => mouse_world.map(ViewportOrderClick::Terrain),
            None => None,
        };
        let pick = frame.picked.as_ref();
        let any_selected_non_area_weapon = self.selection.selected.iter().any(|part| {
            units
                .iter()
                .any(|unit| unit.part_id == *part && unit.has_non_area_weapon)
        });
        let mut aim = pick.and_then(|p| p.aim);
        if let Some(facts) = &mut aim {
            facts.any_selected_non_area_weapon = any_selected_non_area_weapon;
        }
        BridgeContext {
            scenario_time: frame.scenario_time,
            invulnerable: self.snapshot.invulnerable,
            world_point: world_point.or(mouse_world),
            command_clicked: frame.command_click,
            waypoint_queue: frame.waypoint_queue,
            roster_order: parts.clone(),
            home_commander_order: parts.clone(),
            unit_bar_parts: units
                .iter()
                .map(|u| (u.part_id as u32, PartId(u.part_id)))
                .collect(),
            force_group_members: std::array::from_fn(|group| {
                units
                    .iter()
                    .filter(|u| !u.bar.destroyed && u.bar.groups & (1_u16 << group) != 0)
                    .map(|u| PartId(u.part_id))
                    .collect()
            }),
            viewport_click,
            next_nav_marker: self.snapshot.next_nav_marker,
            visible_movers: units
                .iter()
                .filter(|u| u.in_view && !u.bar.destroyed)
                .map(|u| PartId(u.part_id))
                .collect(),
            energy_weapons_held: self.energy_weapons_held,
            target: pick.map(|p| WatchId(p.watch_id)),
            target_part: pick.and_then(|p| p.part_id).map(PartId),
            // `doAttack` (missiongui.cpp:1817): capture branch first, else anything the home team is
            // not friendly with (an enemy or neutral) is attacked.
            target_kind: pick.map(|p| match p.team {
                _ if p.captureable => AttackTargetKind::Captureable,
                TacTeam::Enemy | TacTeam::Neutral => AttackTargetKind::Enemy,
                _ => AttackTargetKind::Friendly,
            }),
            target_disabled: pick.map(|p| p.disabled),
            capture_movers: units
                .iter()
                .map(|u| (PartId(u.part_id), !u.bar.is_vehicle()))
                .collect(),
            capture_blockers: pick
                .map(|p| {
                    p.capture_blockers
                        .iter()
                        .map(|(&part, &blocked)| (PartId(part), blocked))
                        .collect()
                })
                .unwrap_or_default(),
            aim,
            mover_attack_ranges: units
                .iter()
                .map(|u| (PartId(u.part_id), u.attack_range))
                .collect(),
            jump_capable: units
                .iter()
                .map(|u| (PartId(u.part_id), u.bar.can_jump))
                .collect(),
            unit_caps: units.iter().map(|u| (PartId(u.part_id), u.caps)).collect(),
            needs_refit: units
                .iter()
                .map(|u| (PartId(u.part_id), u.needs_refit))
                .collect(),
            // `ForceGroupBar::getIconCount() < MAX_ICONS` (forcegroupbar.h:21)
            force_bar_full: units.len() >= 16,
            support_target: pick.map(|p| p.support),
            // `controlGui.isAddingVehicle()` / `isAddingSalvage()` and not `paintingVtol` (missiongui.cpp:1324)
            adding_vehicle: self
                .control
                .selected_vehicle
                .filter(|k| {
                    !matches!(
                        k,
                        mc2_ui::control::VehicleKind::Airstrike
                            | mc2_ui::control::VehicleKind::SensorProbe
                    )
                })
                .filter(|_| !self.snapshot.vtol_in_flight),
            jump_landing_ok: Some(world_view::can_jump_to_wpos(
                &units
                    .iter()
                    .filter(|u| self.selection.selected.contains(&u.part_id))
                    .collect::<Vec<_>>(),
                frame.mouse_world,
                self.last_passable,
            )),
            // `ControlGui::isAddingAirstrike()` with the pressed button's `ARTILLERY_*` id (`addAirstrike`, missiongui.cpp:4133)
            strike: match self.control.selected_vehicle {
                Some(mc2_ui::control::VehicleKind::Airstrike) => Some(ARTILLERY_LARGE),
                Some(mc2_ui::control::VehicleKind::SensorProbe) => Some(ARTILLERY_SENSOR),
                _ => None,
            }
            .filter(|_| !self.snapshot.vtol_in_flight),
        }
    }

    /// Routes the composed HUD's intents.
    fn route(
        &mut self,
        output: HudOutput,
        units: &[HudUnit],
        frame: &OrderFrame,
        elevation: &dyn Fn(f32, f32) -> f32,
    ) -> HudFrameOutput {
        let mut out = HudFrameOutput::default();
        let mut orders = Vec::new();
        for intent in &output.intents {
            match intent {
                HudIntent::UnitBar(i) => self.apply_unit_bar_intent(i, units, &mut out),
                HudIntent::TacMapCameraTarget(p) => out.camera_target = Some(*p),
                HudIntent::Pause(PauseIntent::Resume | PauseIntent::ToggleObjectives) => {}
                // the pause panel's actions belong to the game (save, load, options, abort, quit)
                HudIntent::Pause(action) => out.pause_actions.push(*action),
                HudIntent::Panel(_) => {}
                // `sendAirstrike` / `sendSensorStrike` press the panel button (controlgui.h:252-274); the press is the same
                // `BuildVehicle` intent a click makes. Pressing an armed button again does nothing (`!PRESSED`).
                HudIntent::Mission(
                    command @ (MissionCommand::SendAirstrike | MissionCommand::SendSensorStrike),
                ) => {
                    orders.push(intent.clone());
                    let kind = if *command == MissionCommand::SendAirstrike {
                        mc2_ui::control::VehicleKind::Airstrike
                    } else {
                        mc2_ui::control::VehicleKind::SensorProbe
                    };
                    let in_flight = self.snapshot.vtol_in_flight;
                    if let Some(press) = self.control.press_vehicle(kind, in_flight) {
                        orders.push(HudIntent::Control(press));
                    }
                }
                other if is_order_intent(other) => orders.push(other.clone()),
                HudIntent::Control(_) | HudIntent::Mission(_) => {}
                _ => {}
            }
        }
        if !orders.is_empty() || frame.command_click {
            let context = self.bridge_context(units, frame, &orders, elevation);
            let result = apply_order_intents(
                &mut self.order_log,
                &mut self.orders_state,
                &self.selection,
                &orders,
                &context,
            );
            // the panel shows the bridge's `curOrder`; a called strike un-presses its button (no refund)
            self.control.set_order_bits(result.panel_order_bits);
            self.control.lay_mines = self.orders_state.lay_mines;
            if result.strike_called {
                self.control.unpress_vehicle_buttons();
            }
            if result.play_invalid_gui {
                out.cues.push(Cue::Sound(sfx::INVALID_GUI as i32));
            }
            self.refuse(result.unavailable, &mut out);
            if let Some(part) = result.show_info {
                self.info_unit = Some(part.0);
                self.hud.tab = MissionTab::Info;
            }
            self.selection
                .select_only(result.selected.iter().map(|part| part.0));
            out.commands = result.commands;
        }
        out
    }

    fn apply_unit_bar_intent(
        &mut self,
        intent: &UnitBarIntent,
        units: &[HudUnit],
        out: &mut HudFrameOutput,
    ) {
        match intent {
            UnitBarIntent::Select(ids) => self.selection.select_only(ids.iter().map(|i| *i as i32)),
            UnitBarIntent::AddToSelection(id) => self.selection.add([*id as i32]),
            UnitBarIntent::RemoveFromSelection(id) => self.selection.remove(*id as i32),
            UnitBarIntent::CenterOn(id) => out.center_on = Some(*id as i32),
            UnitBarIntent::SelectForceGroup(g) => self.select_force_group(*g, true, units),
            UnitBarIntent::ShowInfo(id) => {
                self.info_unit = units
                    .iter()
                    .any(|u| u.part_id == *id as i32)
                    .then_some(*id as i32);
            }
            UnitBarIntent::LeaveAllGroups(id) => {
                out.commands.push(PlayerCommand::ClearForceGroup {
                    mover: PartId(*id as i32),
                })
            }
        }
    }

    /// Digit hotkeys from `MissionInterfaceManager::update` (`code/missiongui.cpp:1741-1752`).
    /// Ctrl writes the sim-owned group (`makeForceGroup`, :3284-3307); Shift toggles its live
    /// members and a plain digit replaces the selection (`selectForceGroup`, :3249-3282).
    fn apply_force_group(
        &mut self,
        group: u8,
        ctrl: bool,
        shift: bool,
        commands: &mut Vec<PlayerCommand>,
    ) {
        if ctrl {
            commands.push(PlayerCommand::SetForceGroup {
                group,
                members: self
                    .selection
                    .selected
                    .iter()
                    .copied()
                    .map(PartId)
                    .collect(),
            });
        } else {
            self.select_force_group(group, !shift, &self.snapshot.units.clone());
        }
    }

    /// The first branch of `MissionInterfaceManager::update`: release queued player orders when
    /// the waypoint key is released (`code/missiongui.cpp:1708-1728`). The simulation owns the
    /// per-pilot `queue_executing` guard and queue transition.
    fn execute_waypoint_queue(&self, scenario_time: f32, commands: &mut Vec<PlayerCommand>) {
        if self.selection.selected.is_empty() {
            return;
        }
        commands.push(PlayerCommand::Order {
            movers: self
                .selection
                .selected
                .iter()
                .copied()
                .map(PartId)
                .collect(),
            order: TacticalOrder::new(
                OrderOrigin::Player,
                OrderCode::WaypointsDone,
                false,
                scenario_time,
            ),
            queue: false,
        });
    }

    /// `MissionInterfaceManager::selectForceGroup` (`code/missiongui.cpp:3249-3282`).
    fn select_force_group(&mut self, group: u8, deselect: bool, units: &[HudUnit]) {
        if group >= 10 {
            return;
        }
        let members: Vec<_> = units
            .iter()
            .filter(|unit| !unit.bar.destroyed && unit.bar.groups & (1_u16 << group) != 0)
            .map(|unit| unit.part_id)
            .collect();
        if deselect {
            self.selection.select_only(members);
        } else if members.iter().all(|part| self.selection.is_selected(*part)) {
            for member in members {
                self.selection.remove(member);
            }
        } else {
            self.selection.add(members);
        }
    }

    fn build_tac_map(&self, camera: &GameCamera, units: &[HudUnit]) -> TacMapInput {
        let mut tac_units: Vec<TacUnit> = self.snapshot.contacts.clone();
        tac_units.extend(units.iter().map(|u| TacUnit {
            team: TacTeam::Ours,
            artillery: false,
            sensor_entry: false,
            pos: (u.position.0, u.position.1),
            selected: self.selection.is_selected(u.part_id),
            // `gametacmap.cpp:373-376`: a shut-down unit keeps its blip but its ring colour is 0.
            sensor_range_m: if u.bar.shutdown {
                0.0
            } else {
                u.sensor_range_m
            },
        }));
        TacMapInput {
            buildings: Vec::new(),
            units: tac_units,
            objectives: self.snapshot.tac_objectives.clone(),
            objective_total: self.snapshot.tac_objective_total,
            view_corners: view_corners(camera),
            // `isAddingAirstrike() && isButtonPressed(SENSOR_PROBE)` (gametacmap.cpp:91)
            placing_airstrike: self.control.selected_vehicle
                == Some(mc2_ui::control::VehicleKind::SensorProbe),
        }
    }

    /// The mission's video window in HUD coordinates (`ControlGui::videoRect`, `buttonlayout<N>.fit`'s `VideoWindow`).
    /// `ControlGui::playPilotVideo`: the radio message of the pilot in `pilot_slot` starts its video in the unit bar.
    /// `enabled` is the Pilot Videos option and `mission_video` whether a mission video plays (`isMoviePlaying`);
    /// returns whether the video plays.
    pub fn start_pilot_video(
        &mut self,
        vfs: &mut Vfs,
        pilot_slot: u32,
        movie_code: u8,
        enabled: bool,
        mission_video: bool,
    ) -> bool {
        if mission_video {
            return false;
        }
        let Some(unit) = self
            .snapshot
            .units
            .iter()
            .find(|u| u.pilot_slot == pilot_slot)
        else {
            return false;
        };
        // only a pilot with an icon on the bar has a slot to play in
        if self.unit_bar.slot_of(unit.part_id as u32).is_none() {
            return false;
        }
        let name = self.resolve(&unit.pilot_name);
        self.pilot_video
            .start(vfs, pilot_slot, &name, movie_code, enabled)
    }

    /// The pilots with a unit on the HUD: `(pilot slot, name)`.
    pub fn pilot_names(&self) -> Vec<(u32, String)> {
        self.snapshot
            .units
            .iter()
            .map(|u| (u.pilot_slot, self.resolve(&u.pilot_name)))
            .collect()
    }

    /// Where a video of the pilot in `pilot_slot` plays, in HUD pixels.
    pub fn pilot_video_slot_rect(&self, pilot_slot: u32) -> Option<GuiRect> {
        let unit = self
            .snapshot
            .units
            .iter()
            .find(|u| u.pilot_slot == pilot_slot)?;
        self.unit_bar.pilot_video_rect(unit.part_id as u32)
    }

    /// `ControlGui::endPilotVideo`.
    pub fn end_pilot_video(&mut self) {
        self.pilot_video.end();
    }

    /// Moves the pilot video on by `dt` seconds; a unit that shuts down ends it (`mechicon.cpp:962-969`).
    pub fn advance_pilot_video(&mut self, vfs: &mut Vfs, dt: f32) {
        if let Some(pilot) = self.pilot_video.pilot() {
            let stopped = self
                .snapshot
                .units
                .iter()
                .find(|u| u.pilot_slot == pilot)
                .is_none_or(|u| u.bar.shutdown);
            if stopped {
                self.pilot_video.end();
                return;
            }
        }
        self.pilot_video.advance(vfs, dt);
    }

    /// The pilot video's rectangle (in HUD pixels) and current picture, while one plays in its unit's slot.
    pub fn pilot_video_view(&self) -> Option<(GuiRect, &mc2_platform::movie::VideoFrame)> {
        let pilot = self.pilot_video.pilot()?;
        let frame = self.pilot_video.frame()?;
        let unit = self.snapshot.units.iter().find(|u| u.pilot_slot == pilot)?;
        let rect = self.unit_bar.pilot_video_rect(unit.part_id as u32)?;
        Some((rect, frame))
    }

    pub fn video_window_rect(&self) -> GuiRect {
        self.video_window
    }

    /// The tutorial's lines this frame (`MissionInterfaceManager::setTutorialText`, `missiongui.cpp:418-422`: the text under
    /// `IDS_TUTORIAL` in the chat log).
    pub fn tutorial_says(&mut self, lines: &[(String, String)]) {
        for (speaker, message) in lines {
            self.say(speaker, message);
        }
    }

    /// Adds a mission message line (`ControlGui::addChatText`).
    pub fn say(&mut self, who: &str, message: &str) {
        self.chat_lines.push((
            CHAT_SECONDS,
            MissionChatLine {
                player_name: who.to_string(),
                message: message.to_string(),
                background_color: 0xff40_4040,
                text_color: 0xffff_ffff,
            },
        ));
        if self.chat_lines.len() > CHAT_LINES {
            self.chat_lines.remove(0);
        }
        self.sync_chat();
    }

    /// The mission's `DisplayTextMessage` / `DisplayResourceTextMessage` lines of a frame
    /// (`CDisplayTextMessage::Execute`, `objective.cpp:1233`, `1269`: the only gameplay callers
    /// of `ControlGui::setChatText`). Radio events are the pilot's audio (`Radio::playMessage`),
    /// never chat text, so nothing else is written here.
    pub fn messages(&mut self, messages: &[HudMessage]) {
        for m in messages {
            let text = match m {
                HudMessage::Text(t) => t.clone(),
                HudMessage::Resource(id) => self.assets.string(*id).to_string(),
            };
            self.say("COMMAND", &text);
        }
    }

    fn tick_chat(&mut self, dt: f32) {
        for l in &mut self.chat_lines {
            l.0 -= dt;
        }
        self.chat_lines.retain(|l| l.0 > 0.0);
        self.sync_chat();
    }

    fn sync_chat(&mut self) {
        self.chat.lines = self.chat_lines.iter().map(|l| l.1.clone()).collect();
    }

    /// Draws the HUD over the frame in `target` (loading, not clearing, what is there). `movie` is the mission video window's
    /// current picture (`ControlGui::render`'s `bMovie->render()`, `controlgui.cpp:367-372`), drawn in `video_window` over the
    /// GUI; `None` when no mission video plays.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &RenderTarget,
        dt: f32,
        movie: Option<&mc2_platform::movie::VideoFrame>,
        pointer: Option<(f32, f32)>,
    ) -> Result<(), String> {
        // Scene-pass overlays: selection brackets and health bars in native pixels
        // (`Appearance::render` draws them with the shape, before the GUI).
        if !self.selected_marks.is_empty() {
            let native = target.size().1 as f32 / self.game_size.1.max(1.0);
            let mut batch = OverlayBatch::default();
            world_view::draw_marks(
                &mut batch,
                &self.selected_marks,
                self.scale_factor,
                native,
                self.hold_fire_icon.as_ref(),
            );
            if self
                .overlay
                .as_ref()
                .is_none_or(|o| o.format() != target.display_format)
            {
                self.overlay = Some(OverlayPainter::new(gpu, target.display_format));
            }
            if let Some(overlay) = self.overlay.as_ref() {
                overlay.draw(
                    gpu,
                    encoder,
                    &target.display_view,
                    target.size(),
                    &batch,
                    None,
                );
            }
        }
        let mut dl = DrawList::new(self.view_w, self.res.1 as i32);
        {
            let mut ctx = RenderCtx {
                dl: &mut dl,
                assets: &self.assets,
                dt,
            };
            let mut parts = HudRenderParts {
                control: &mut self.control,
                unit_bar: &mut self.unit_bar,
                unit_bar_output: &mut self.unit_bar_output,
                info: &mut self.info,
                mission_chat: &self.chat,
                tac_map: &mut self.tac_map,
                tac_map_input: &self.tac_map_input,
                metrics: &self.assets,
                objectives: &mut self.objectives,
                objectives_view: &self.objectives_view,
                pause_window: &mut self.pause_window,
            };
            self.hud
                .render(&mut ctx, &mut parts)
                .map_err(|e| e.to_string())?;
        }
        let mut status_dl = DrawList::new(self.view_w, self.res.1 as i32);
        self.draw_results_status(&mut status_dl);
        if let Some((a, b)) = self.drag_box {
            let (ha, hb) = (self.to_hud(a), self.to_hud(b));
            dl.outline_rect(
                GuiRect::new(
                    ha.0.min(hb.0),
                    ha.1.min(hb.1),
                    ha.0.max(hb.0),
                    ha.1.max(hb.1),
                ),
                DRAG_BOX_COLOUR,
                DRAG_BOX_COLOUR,
            );
        }
        if let Some(frame) = movie {
            {
                let r = self.video_window_rect();
                let id = self.assets.textures.insert_streamed(
                    "mission_video",
                    frame.width,
                    frame.height,
                    i420_to_rgba(frame),
                );
                dl.rect_quad(
                    Some(id),
                    Blend::Opaque,
                    (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32),
                    (0.0, 0.0, 1.0, 1.0),
                    0xffff_ffff,
                );
            }
        }
        if let Some((r, frame)) = self.pilot_video_view() {
            let id = self.assets.textures.insert_streamed(
                "pilot_video",
                frame.width,
                frame.height,
                i420_to_rgba(frame),
            );
            dl.rect_quad(
                Some(id),
                Blend::Opaque,
                (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32),
                (0.0, 0.0, 1.0, 1.0),
                0xffff_ffff,
            );
        }
        let layout = UiLayout::new(self.fit().ui_size, target.size(), ScaleMode::Fit);
        let (tw, th) = target.size();
        let grey = self.results_overlay(tw as i32, th as i32, true);
        let fade = self.results_overlay(tw as i32, th as i32, false);
        let renderer = self.renderer.get_or_insert_with(|| UiRenderer::new(gpu));
        renderer.draw(gpu, encoder, target, &layout, &dl, &self.assets);
        // `renderResults`: grey over the whole screen, the status frame, then the fade to black.
        let full = UiLayout::new((tw, th), (tw, th), ScaleMode::Fit);
        if self.results_grey {
            renderer.draw(gpu, encoder, target, &full, &grey, &self.assets);
        }
        renderer.draw(gpu, encoder, target, &layout, &status_dl, &self.assets);
        if self.results_fade != 0 {
            renderer.draw(gpu, encoder, target, &full, &fade, &self.assets);
        }
        if self.cursor_inside || self.callout_pointer.is_some() {
            if let Some(c) = &self.cursors {
                let (state, at) = match self.callout_pointer {
                    Some(p) => (mc2_ui::logistics::callout::M_STATE_TUTORIALS, p),
                    None => (self.order_cursor, pointer.unwrap_or(self.mouse)),
                };
                let (gw, gh) = (
                    self.game_size.0.round().max(1.0) as u32,
                    self.game_size.1.round().max(1.0) as u32,
                );
                let mut cursor_dl = DrawList::new(gw as i32, gh as i32);
                c.draw(
                    &mut cursor_dl,
                    state,
                    (at.0.round() as i32, at.1.round() as i32),
                );
                let cursor_layout = UiLayout::new((gw, gh), target.size(), ScaleMode::Fit);
                renderer.draw(
                    gpu,
                    encoder,
                    target,
                    &cursor_layout,
                    &cursor_dl,
                    &self.assets,
                );
            }
        }
        Ok(())
    }
}

/// An I420 picture (the movie decoder's Y, Cb, Cr planes) as RGBA, for the UI texture store: BT.601 limited range, the
/// conversion the movie's colours were encoded with.
fn i420_to_rgba(frame: &mc2_platform::movie::VideoFrame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let (ys, xs) = (y * frame.y_stride as usize, x);
            let (ci, cx) = ((y / 2) * frame.c_stride as usize, x / 2);
            let luma = f32::from(frame.y[ys + xs]) - 16.0;
            let cb = f32::from(frame.cb[ci + cx]) - 128.0;
            let cr = f32::from(frame.cr[ci + cx]) - 128.0;
            let r = 1.164 * luma + 1.596 * cr;
            let g = 1.164 * luma - 0.392 * cb - 0.813 * cr;
            let b = 1.164 * luma + 2.017 * cb;
            for c in [r, g, b] {
                out.push(c.clamp(0.0, 255.0) as u8);
            }
            out.push(255);
        }
    }
    out
}

/// `GameTacMap::render`'s view quad: the camera's ground corners, top-left, bottom-left,
/// bottom-right, top-right; the lower pair at two thirds of the screen height
/// (gametacmap.cpp:217-243).
fn view_corners(camera: &GameCamera) -> [(f32, f32); 4] {
    let (w, h) = camera.viewport;
    let low = h * 0.6667 - 1.0;
    let ground = camera.target.z;
    let at = |x: f32, y: f32| {
        camera
            .pixel_to_ground(x, y, ground)
            .map_or((camera.target.x, camera.target.y), |p| (p.x, p.y))
    };
    [at(0.0, 0.0), at(0.0, low), at(w, low), at(w, 0.0)]
}

/// `[MissionSettings]` of `data/missions/<mission>.fit`.
fn mission_settings(
    vfs: &mut Vfs,
    mission: &str,
) -> Result<mc2_sim_data::campaign::MissionSettings, String> {
    let path = format!("data/missions/{mission}.fit");
    let bytes = vfs
        .read(&path)
        .map_err(|e| format!("{path}: {e}"))?
        .ok_or_else(|| format!("{path} is not in the data"))?;
    let mut fit =
        mc2_formats::ini::FitIni::from_bytes(bytes).map_err(|e| format!("{path}: {e}"))?;
    mc2_sim_data::campaign::MissionSettings::from_fit(&mut fit).map_err(|e| format!("{path}: {e}"))
}

/// Packet 3 of `data/missions/<mission>.pak`: the colour-map TGA (`initTacMap`, missiongui.cpp:3085).
fn mission_colour_map(vfs: &mut Vfs, mission: &str) -> Result<Vec<u8>, String> {
    let path = format!("data/missions/{mission}.pak");
    let bytes = vfs
        .read(&path)
        .map_err(|e| format!("{path}: {e}"))?
        .ok_or_else(|| format!("{path} is not in the data"))?;
    let pak = mc2_formats::pak::PakFile::from_bytes(bytes).map_err(|e| format!("{path}: {e}"))?;
    pak.read_packet(3)
        .map_err(|e| format!("{path}: colour map packet: {e}"))
}

/// Whether `p` is inside (or on) the screen triangle `[x0, y0, x1, y1, x2, y2]`, either winding
/// (`TG_Shape::PerPolySelect`, `mclib/tgl.cpp:2763`).
/// The `MC2_P3_TRACE` line for one completed command click: where the pointer was, which units were
/// selected, what the picker resolved (`None` is a miss) and the ground point under the pointer.
fn describe_command_click(
    scenario_time: f32,
    mouse: (f32, f32),
    selected: &[i32],
    picked: Option<u32>,
    mouse_world: Option<(f32, f32)>,
) -> String {
    let watch = picked.map_or("none".to_string(), |w| w.to_string());
    format!(
        "p3-trace click t {scenario_time:.2} mouse {mouse:?} selected {selected:?} picked watch {watch} mouse_world {mouse_world:?}"
    )
}

/// The first of `picks` (already in `findObjectByMouse` search order) the mouse holds: inside its
/// projected box (inclusive) and, unless it is a mover, on one of its visible faces.  `k` scales
/// viewport pixels to game pixels.
fn pick_at(picks: &[PickTarget], k: f32, (mx, my): (f32, f32)) -> Option<PickTarget> {
    picks
        .iter()
        .find(|c| {
            let [l, t, r, b] = c.screen_box.map(|v| v * k);
            mx >= l
                && mx <= r
                && my >= t
                && my <= b
                && (c.mover
                    || c.triangles
                        .iter()
                        .any(|t| triangle_holds(t.map(|v| v * k), (mx, my))))
        })
        .cloned()
}

fn triangle_holds(t: [f32; 6], p: (f32, f32)) -> bool {
    let edge = |ax: f32, ay: f32, bx: f32, by: f32| (bx - ax) * (p.1 - ay) - (by - ay) * (p.0 - ax);
    let d0 = edge(t[0], t[1], t[2], t[3]);
    let d1 = edge(t[2], t[3], t[4], t[5]);
    let d2 = edge(t[4], t[5], t[0], t[1]);
    (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_click_trace_names_a_hit_and_a_miss() {
        let hit = describe_command_click(1.5, (10.0, 20.0), &[3, 4], Some(77), Some((1.0, 2.0)));
        assert!(hit.contains("picked watch 77") && hit.contains("selected [3, 4]"));
        let miss = describe_command_click(1.5, (10.0, 20.0), &[3], None, None);
        assert!(miss.contains("picked watch none") && miss.contains("mouse_world None"));
    }

    /// Hand-placed pick fixture (not produced by the projection code): game pixels, `k` = 1.
    fn target(id: u32, mover: bool, b: [f32; 4], tris: Vec<[f32; 6]>) -> PickTarget {
        PickTarget {
            captureable: false,
            capture_blockers: Default::default(),
            watch_id: id,
            part_id: mover.then_some(id as i32),
            position: (0.0, 0.0, 0.0),
            screen_box: b,
            triangles: tris.into(),
            mover,
            team: TacTeam::Enemy,
            disabled: false,
            class: if mover {
                mc2_ui::mission_keys::ObjectClass::Mech
            } else {
                mc2_ui::mission_keys::ObjectClass::Building
            },
            landbridge: false,
            support: Default::default(),
            aim: None,
            info_unit: None,
        }
    }

    /// `findObjectByMouse` (objmgr.cpp:2253-2262): a mover is taken anywhere in its box ("Movers are NOT
    /// per poly"); a structure only on a visible face (`PerPolySelect`).  Box edges are inclusive.
    #[test]
    fn a_mover_is_picked_on_its_box_a_structure_only_on_its_faces() {
        let mover = target(1, true, [100.0, 100.0, 200.0, 200.0], vec![]);
        // a face covering only the box's upper-left half
        let face = [300.0, 100.0, 400.0, 100.0, 300.0, 200.0];
        let building = target(2, false, [300.0, 100.0, 400.0, 200.0], vec![face]);
        let picks = [mover, building];
        let at = |p: (f32, f32)| pick_at(&picks, 1.0, p).map(|t| t.watch_id);
        assert_eq!(
            at((195.0, 195.0)),
            Some(1),
            "box corner of a mover, no faces"
        );
        assert_eq!(at((100.0, 100.0)), Some(1), "the box edge is inside");
        assert_eq!(at((200.5, 150.0)), None, "just outside the box");
        assert_eq!(at((320.0, 120.0)), Some(2), "on the structure's face");
        assert_eq!(
            at((390.0, 190.0)),
            None,
            "in the structure's box, off its faces"
        );
        assert_eq!(at((350.0, 150.0)), Some(2), "on the face's hypotenuse");
    }

    #[test]
    fn a_pick_scales_with_the_game_size() {
        let picks = [target(1, true, [100.0, 100.0, 200.0, 200.0], vec![])];
        assert!(pick_at(&picks, 2.0, (390.0, 390.0)).is_some());
        assert!(pick_at(&picks, 2.0, (190.0, 190.0)).is_none());
    }

    #[test]
    fn wide_screens_get_extra_layout_width_and_the_original_sizes_get_none() {
        // 4:3 and narrower are exactly the original layout
        assert_eq!(wide_extra((1024.0, 768.0), (1024, 768)), 0);
        assert_eq!(wide_extra((800.0, 600.0), (800, 600)), 0);
        assert_eq!(wide_extra((1280.0, 1024.0), (1280, 1024)), 0);
        // 16:9 on the 1024 layout: 768 * 16/9 = 1365 visible, the block is 1024 wide
        assert_eq!(wide_extra((1600.0, 900.0), (1024, 768)), 341);
        // the 1280 layout's block is 1024 wide too (centred by its xOffset in the original)
        assert_eq!(wide_extra((1920.0, 1080.0), (1280, 1024)), 796);
        // the 1920 layout keeps the 1024 control geometry (its xOffset 288 is not 1920 - 1024 over two)
        assert_eq!(wide_extra((1920.0, 1200.0), (1920, 1200)), 896);
        // 1200 * 2560/1440 = 2133 visible
        assert_eq!(wide_extra((2560.0, 1440.0), (1920, 1200)), 1109);
    }

    #[test]
    fn the_hud_uses_the_largest_shipped_layout_that_fits() {
        assert_eq!(hud_resolution((640.0, 480.0)), (640, 480));
        assert_eq!(hud_resolution((800.0, 600.0)), (800, 600));
        assert_eq!(hud_resolution((1024.0, 768.0)), (1024, 768));
        assert_eq!(hud_resolution((1920.0, 1080.0)), (1280, 1024));
        assert_eq!(hud_resolution((1920.0, 1200.0)), (1920, 1200));
        assert_eq!(hud_resolution((3840.0, 2160.0)), (1920, 1200));
        // smaller than every layout: the smallest, scaled down by the fit
        assert_eq!(hud_resolution((320.0, 200.0)), (640, 480));
    }

    /// `ControlGui::renderResults`'s own arithmetic (`controlgui.cpp:525-550`) over `RESULTS_MOVE`,
    /// including its scale of the authored positions and its settle on the last row of the table.
    fn cxx_results_position(results_time: f32, screen_width: u32) -> f32 {
        let scale = screen_width as f32 / 640.0;
        let mut t0 = 0.0;
        let mut t1 = 0.0;
        let mut p0 = 0.0;
        let mut p1 = 0.0;
        let mut in_segment = false;
        for j in 0..RESULTS_MOVE.len() - 1 {
            if RESULTS_MOVE[j].0 <= results_time && RESULTS_MOVE[j + 1].0 > results_time {
                t0 = RESULTS_MOVE[j].0;
                t1 = RESULTS_MOVE[j + 1].0;
                p0 = RESULTS_MOVE[j].1 * scale;
                p1 = RESULTS_MOVE[j + 1].1 * scale;
                in_segment = true;
                break;
            }
        }
        if in_segment {
            p0 + (results_time - t0) * ((p1 - p0) / (t1 - t0))
        } else {
            // the C++ settles on `missionResultsMoveInfo[OBJECTVE_MOVE_COUNT - 1].position`, i.e.
            // row 1 (0), unscaled (controlgui.cpp:545-550): a bug, fixed to the last row and the
            // screen scale (DIFFERENCES.md)
            RESULTS_MOVE[RESULTS_MOVE.len() - 1].1 * scale
        }
    }

    #[test]
    fn results_status_frame_follows_the_authored_slide() {
        // The authored points and screen-width scale are exact UI behaviour. Interpolation is
        // f32 (`0.2f - 0.16f` is `0.040000916`), so allow 0.001 px of rounding and check every
        // point against the C++ formula rather than a typed value.
        let assert_position = |time: f32, width: u32, expected: f32| {
            let actual = results_status_position(time, width);
            assert!(
                (actual - expected).abs() <= 0.001,
                "at {time}s, width {width}: {actual} != {expected} within 0.001 px"
            );
        };
        for &time in &[-1.0, 0.0, 0.08, 0.16, 0.20, 0.24, 1.0] {
            for &width in &[640, 1280] {
                assert_position(time, width, cxx_results_position(time, width));
            }
        }
        // the authored rows themselves, at quarter of the way into the first move and on the last
        assert_position(0.0, 640, -65.0);
        assert_position(0.08, 640, -32.5);
        assert_position(0.16, 640, 0.0);
        assert_position(0.0, 1280, -130.0);
        assert_position(0.24, 640, -10.0);
        assert_position(1.0, 640, -10.0);
    }

    #[test]
    fn order_intents_go_through_the_bridge_and_are_logged() {
        let mut log = Vec::new();
        let mut state = OrdersState::default();
        let mut selection = Selection::default();
        selection.select_only([5]);
        let context = BridgeContext {
            home_commander_order: vec![PartId(5)],
            world_point: Some(mc2_sim::math::Vec3 {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            }),
            ..BridgeContext::default()
        };
        let intents = [HudIntent::TacMapMoveOrder((1.0, 2.0))];
        let out = apply_order_intents(&mut log, &mut state, &selection, &intents, &context);
        assert_eq!(log, intents);
        assert_eq!(out.commands.len(), 1, "{out:?}");
        assert!(out.unavailable.is_empty());
        assert_eq!(state.selected, vec![PartId(5)]);
    }

    #[test]
    fn only_orders_are_order_intents() {
        assert!(is_order_intent(&HudIntent::Mission(MissionCommand::Jump)));
        assert!(is_order_intent(&HudIntent::Control(
            mc2_ui::control::ControlIntent::ToggleJump
        )));
        assert!(!is_order_intent(&HudIntent::Mission(
            MissionCommand::TogglePause
        )));
        assert!(is_order_intent(&HudIntent::Mission(
            MissionCommand::SelectVisible
        )));
        assert!(!is_order_intent(&HudIntent::TacMapCameraTarget((0.0, 0.0))));
        assert!(!is_order_intent(&HudIntent::UnitBar(
            UnitBarIntent::CenterOn(1)
        )));
    }

    #[test]
    fn every_non_camera_hot_key_has_a_mission_command() {
        for c in crate::hotkeys::COMMANDS {
            if !c.function.is_camera() {
                // chat keys and the compass/nav handlers are not mission commands of this slice
                let known = mission_command(c.function).is_some()
                    || matches!(
                        c.function,
                        Command::InfoCommand
                            | Command::InfoButtonReleased
                            | Command::GotoNextNavMarker
                            | Command::ToggleCompass
                            | Command::ChatKey
                            | Command::TeamChatKey
                            | Command::RotateLightLeft
                            | Command::RotateLightRight
                            | Command::RotateLightUp
                            | Command::RotateLightDown
                    );
                assert!(known, "{:?}", c.function);
            }
        }
        assert_eq!(
            mission_command(Command::GotoNextNavMarker),
            Some(MissionCommand::GotoNextNavMarker)
        );
        assert_eq!(
            mission_command(Command::InfoCommand),
            Some(MissionCommand::InfoCommand)
        );
        assert_eq!(
            mission_command(Command::InfoButtonReleased),
            Some(MissionCommand::InfoButtonReleased)
        );
    }

    #[test]
    fn an_aspect_only_resize_keeps_the_armed_support_purchase() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED aspect resize test: MC2_DATA is unset");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).expect("mount game data");
        let world = mc2_sim::mission::init::load_world(
            &mut vfs,
            &mc2_sim::mission::init::LoadSpec::quickstart(
                "mc2_01",
                1,
                mc2_sim::rng::RngMode::LegacyMsvc,
            ),
        )
        .expect("load mc2_01");
        let sim = mc2_sim::Sim::new(world);
        let snapshot =
            crate::hud_data::snapshot(&sim.world, &crate::hud_data::PickBoxes::default());
        let mut old = HudRuntime::load(&mut vfs, (1920.0, 1080.0), "mc2_01", snapshot.clone())
            .expect("load 1920x1080");
        old.control.selected_vehicle = Some(mc2_ui::control::VehicleKind::Airstrike);
        // 1900x1080 has the same shipped layout, a different aspect: the HUD is rebuilt
        assert!(!old.fits((1900.0, 1080.0)));
        let mut fresh = HudRuntime::load(&mut vfs, (1900.0, 1080.0), "mc2_01", snapshot)
            .expect("load 1900x1080");
        assert_eq!(fresh.resolution(), old.resolution());
        assert_eq!(fresh.control.selected_vehicle, None);
        fresh.carry_state_from(&mut old);
        assert_eq!(
            fresh.control.selected_vehicle,
            Some(mc2_ui::control::VehicleKind::Airstrike),
            "the paid purchase survives the rebuild, so its placement or cancel still settles it"
        );

        // the delivery flight finished between the last update and the rebuild: the next update clears the purchase
        let mut old =
            HudRuntime::load(&mut vfs, (1920.0, 1080.0), "mc2_01", fresh.snapshot.clone())
                .expect("load 1920x1080");
        old.control.selected_vehicle = Some(mc2_ui::control::VehicleKind::Airstrike);
        old.vtol_was_in_flight = true;
        let mut fresh =
            HudRuntime::load(&mut vfs, (1900.0, 1080.0), "mc2_01", fresh.snapshot.clone())
                .expect("load 1900x1080");
        fresh.carry_state_from(&mut old);
        assert!(fresh.vtol_was_in_flight);
        let camera = GameCamera::new(
            mc2_render::camera::CameraSettings::default(),
            (1900.0, 1080.0),
        );
        let user_input = UserInput::new();
        let speaking = std::collections::BTreeSet::new();
        let elevation = |_, _| 0.0;
        let mut snap = fresh.snapshot.clone();
        snap.vtol_in_flight = false;
        fresh.update(HudFrameInput {
            events: &[],
            user_input: &user_input,
            fired: &[],
            camera: &camera,
            game_size: (1900.0, 1080.0),
            dt: 1.0 / 30.0,
            drag_threshold_px: 5.0,
            left_right_profile: true,
            shift: false,
            waypoint_queue: false,
            turn: 0,
            camera_rotate: false,
            scenario_time: 0.0,
            mouse_world: None,
            line_of_sight: true,
            passable: true,
            elevation: &elevation,
            snapshot: snap,
            speaking_pilots: &speaking,
        });
        assert_eq!(
            fresh.control.selected_vehicle, None,
            "a flight that finished across a resize still clears the purchase"
        );
    }

    #[test]
    fn fired_nav_and_info_hotkeys_reach_hud_runtime() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED fired HUD hotkey test: MC2_DATA is unset");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).expect("mount game data");
        let world = mc2_sim::mission::init::load_world(
            &mut vfs,
            &mc2_sim::mission::init::LoadSpec::quickstart(
                "mc2_01",
                1,
                mc2_sim::rng::RngMode::LegacyMsvc,
            ),
        )
        .expect("load mc2_01");
        assert!(world.complete, "{:?}", world.missing_hooks);
        let sim = mc2_sim::Sim::new(world);
        let mut snapshot =
            crate::hud_data::snapshot(&sim.world, &crate::hud_data::PickBoxes::default());
        let selected = snapshot.units.first().expect("home mover").part_id;
        let mut runtime = HudRuntime::load(&mut vfs, (1920.0, 1200.0), "mc2_01", snapshot.clone())
            .expect("load production HUD");
        runtime.selection.select_only([selected]);
        let camera = GameCamera::new(
            mc2_render::camera::CameraSettings::default(),
            (1920.0, 1200.0),
        );
        let user_input = UserInput::new();
        let speaking = std::collections::BTreeSet::new();
        let elevation = |_, _| 0.0;
        let update = |runtime: &mut HudRuntime, fired: &[Fired], snapshot: &HudSnapshot| {
            runtime.update(HudFrameInput {
                events: &[],
                user_input: &user_input,
                fired,
                camera: &camera,
                game_size: (1920.0, 1200.0),
                dt: 1.0 / 30.0,
                drag_threshold_px: 5.0,
                left_right_profile: true,
                shift: false,
                waypoint_queue: false,
                turn: 0,
                camera_rotate: false,
                scenario_time: 0.0,
                mouse_world: None,
                line_of_sight: true,
                passable: true,
                elevation: &elevation,
                snapshot: snapshot.clone(),
                speaking_pilots: &speaking,
            })
        };
        let nav = update(
            &mut runtime,
            &[Fired::Run(Command::GotoNextNavMarker)],
            &snapshot,
        );
        assert!(runtime
            .order_log
            .contains(&HudIntent::Mission(MissionCommand::GotoNextNavMarker)));
        if snapshot.next_nav_marker.is_some() {
            assert!(nav.commands.iter().any(|command| matches!(
                command,
                PlayerCommand::Order { order, .. }
                    | PlayerCommand::OrderQueueExecute { order, .. }
                    if order.code == OrderCode::MoveToPoint
            )));
        } else {
            assert!(nav.cues.contains(&Cue::Sound(sfx::INVALID_GUI as i32)));
        }
        update(&mut runtime, &[Fired::Run(Command::InfoCommand)], &snapshot);
        assert!(runtime.orders_state.info_selecting);
        let home_unit = snapshot.units.first().expect("home unit").clone();
        let home_pick = PickTarget {
            watch_id: 900,
            part_id: Some(home_unit.part_id),
            position: home_unit.position,
            screen_box: [0.0; 4],
            triangles: Default::default(),
            mover: true,
            team: TacTeam::Ours,
            disabled: false,
            captureable: false,
            capture_blockers: Default::default(),
            class: mc2_ui::mission_keys::ObjectClass::Mech,
            landbridge: false,
            support: Default::default(),
            aim: None,
            info_unit: Some(home_unit.clone()),
        };
        let info_frame = |picked| OrderFrame {
            scenario_time: 0.0,
            waypoint_queue: false,
            command_click: true,
            picked: Some(picked),
            mouse_world: None,
        };
        runtime.route(
            HudOutput::default(),
            &snapshot.units,
            &info_frame(home_pick),
            &elevation,
        );
        assert_eq!(runtime.info_unit, Some(home_unit.part_id));
        assert_eq!(runtime.hud.tab, MissionTab::Info);

        update(&mut runtime, &[Fired::Run(Command::InfoCommand)], &snapshot);
        let mut enemy_unit = home_unit;
        enemy_unit.part_id = 9001;
        enemy_unit.info.id = 9001;
        enemy_unit.info.team_is_home = false;
        enemy_unit.info.team_not_enemy = false;
        let enemy_pick = PickTarget {
            watch_id: 901,
            part_id: Some(enemy_unit.part_id),
            position: enemy_unit.position,
            screen_box: [0.0; 4],
            triangles: Default::default(),
            mover: true,
            team: TacTeam::Enemy,
            disabled: false,
            captureable: false,
            capture_blockers: Default::default(),
            class: mc2_ui::mission_keys::ObjectClass::Mech,
            landbridge: false,
            support: Default::default(),
            aim: None,
            info_unit: Some(enemy_unit.clone()),
        };
        let enemy_unit_live = enemy_unit;
        snapshot.picks.push(enemy_pick.clone());
        runtime.route(
            HudOutput::default(),
            &snapshot.units,
            &info_frame(enemy_pick),
            &elevation,
        );
        assert_eq!(runtime.info_unit, Some(9001));
        assert_eq!(runtime.hud.tab, MissionTab::Info);
        // A camera pan removes the enemy from the pick snapshot but the home team still sees it:
        // `InfoWindow` keeps the unit (infowindow.cpp:483-489), fed from the live contact set.
        snapshot.picks.clear();
        snapshot.info_units.insert(9001, enemy_unit_live.clone());
        update(&mut runtime, &[], &snapshot);
        assert_eq!(runtime.info_unit, Some(9001));
        assert!(
            runtime.info_window().unit().is_some(),
            "the Info window retains its picked enemy after it leaves view"
        );
        // Visual contact is then lost: the live set drops the mover and the window clears.
        snapshot.info_units.clear();
        update(&mut runtime, &[], &snapshot);
        assert!(
            runtime.info_window().unit().is_none(),
            "the Info window clears an enemy the home team no longer sees"
        );
        update(
            &mut runtime,
            &[Fired::Release(Command::InfoButtonReleased)],
            &snapshot,
        );
        assert!(!runtime.orders_state.info_selecting);
    }
    /// The vehicle key (V, row 21) reaches the order bridge through the HUD's hot key path: it is
    /// logged as the mission command, toggles the mine orders of a selected minelayer, and is not
    /// reported unavailable.
    #[test]
    fn the_vehicle_hot_key_toggles_mine_orders_and_is_not_refused() {
        let Some(root) = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty()) else {
            eprintln!("SKIPPED vehicle hot key test: MC2_DATA is unset");
            return;
        };
        let mut vfs = Vfs::from_system_cfg(root).expect("mount game data");
        let world = mc2_sim::mission::init::load_world(
            &mut vfs,
            &mc2_sim::mission::init::LoadSpec::quickstart(
                "mc2_01",
                1,
                mc2_sim::rng::RngMode::LegacyMsvc,
            ),
        )
        .expect("load mc2_01");
        assert!(world.complete, "{:?}", world.missing_hooks);
        let sim = mc2_sim::Sim::new(world);
        let mut snapshot =
            crate::hud_data::snapshot(&sim.world, &crate::hud_data::PickBoxes::default());
        let selected = snapshot.units.first().expect("home mover").part_id;
        snapshot.units[0].caps.can_lay_mines = true;
        let mut runtime = HudRuntime::load(&mut vfs, (1920.0, 1200.0), "mc2_01", snapshot.clone())
            .expect("load production HUD");
        runtime.selection.select_only([selected]);
        let camera = GameCamera::new(
            mc2_render::camera::CameraSettings::default(),
            (1920.0, 1200.0),
        );
        let user_input = UserInput::new();
        let speaking = std::collections::BTreeSet::new();
        let elevation = |_, _| 0.0;
        let fired = [Fired::Run(Command::VehicleCommand)];
        runtime.update(HudFrameInput {
            events: &[],
            user_input: &user_input,
            fired: &fired,
            camera: &camera,
            game_size: (1920.0, 1200.0),
            dt: 1.0 / 30.0,
            drag_threshold_px: 5.0,
            left_right_profile: true,
            shift: false,
            waypoint_queue: false,
            turn: 0,
            camera_rotate: false,
            scenario_time: 0.0,
            mouse_world: None,
            line_of_sight: true,
            passable: true,
            elevation: &elevation,
            snapshot,
            speaking_pilots: &speaking,
        });
        assert!(runtime
            .order_log
            .contains(&HudIntent::Mission(MissionCommand::VehicleCommand)));
        assert!(runtime.orders_state.lay_mines);
        assert!(!runtime
            .unavailable_summary()
            .iter()
            .any(|s| s.contains("vehicle command")));
    }
}
