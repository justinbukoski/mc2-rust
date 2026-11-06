// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! In-mission orders and vehicle controls (`code/controlgui.cpp`).
//!
//! This module owns only the orders and vehicle tabs. Objectives, results, info, tac map and
//! movies are unavailable here; the app supplies a plain snapshot and consumes plain intents.

use crate::ctx::{Loader, RenderCtx};
use crate::draw::{Blend, GuiRect, Vertex};
use crate::fit::FitExt;
use crate::font::FontRef;
use crate::textures::TexId;
use mc2_formats::ini::FitIni;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ControlRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl ControlRect {
    pub(crate) fn from_fit(
        file: &FitIni,
        x: &str,
        y: &str,
        w: &str,
        h: &str,
        dx: i32,
        dy: i32,
    ) -> Self {
        let x = file.long_or(x, 0) as i32 + dx;
        let y = file.long_or(y, 0) as i32 + dy;
        let w = file.long_or(w, 0) as i32;
        let h = file.long_or(h, 0) as i32;
        Self {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.left <= x && x <= self.right && self.top <= y && y <= self.bottom
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlId {
    DefaultRange,
    ShortRange,
    MediumRange,
    LongRange,
    CurrentRange,
    Jump,
    Run,
    Guard,
    FireFromCurrentPosition,
    Stop,
    LayMines,
    Repair,
    Salvage,
    GuardTower,
    VehicleTab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireRange {
    Optimal,
    Short,
    Medium,
    Long,
    Current,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleKind {
    Airstrike,
    GuardTower,
    SensorProbe,
    RepairTruck,
    Scout,
    Minelayer,
    RecoveryTeam,
}

/// `ControlGui::vehicleCosts[]` (controlgui.cpp:85): what the vehicle takes from the resource points when its button is pressed.
pub fn vehicle_cost(kind: VehicleKind) -> i32 {
    VEHICLE_KINDS
        .iter()
        .position(|k| *k == Some(kind))
        .map_or(0, |i| VEHICLE_COSTS[i])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VehicleAction {
    Build(VehicleKind),
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlIntent {
    SetRun(bool),
    ToggleJump,
    SetGuard(bool),
    SetRange(FireRange),
    SetFireFromCurrentPosition(bool),
    ToggleMines,
    ToggleRepair,
    ToggleSalvage,
    SetGuardTower(bool),
    Stop,
    BuildVehicle(VehicleKind),
    CancelVehicle { kind: VehicleKind, refund: i32 },
}

/// Upstream vehicle purchase restrictions read by `ControlGui::updateVehicleTab`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VehicleAvailability {
    pub can_have_airstrike: bool,
    pub can_have_guard_tower: bool,
    pub can_have_sensor_probe: bool,
    pub can_have_repair_truck: bool,
    pub can_have_scout: bool,
    pub can_have_minelayer: bool,
    pub can_have_recovery_team: bool,
    pub unit_capacity_available: bool,
    pub pilots_remaining: bool,
    pub player_destroyed: bool,
}

impl VehicleAvailability {
    fn permits(self, kind: VehicleKind) -> bool {
        let mission_allows = match kind {
            VehicleKind::Airstrike => self.can_have_airstrike,
            VehicleKind::GuardTower => self.can_have_guard_tower,
            VehicleKind::SensorProbe => self.can_have_sensor_probe,
            VehicleKind::RepairTruck => self.can_have_repair_truck,
            VehicleKind::Scout => self.can_have_scout,
            VehicleKind::Minelayer => self.can_have_minelayer,
            VehicleKind::RecoveryTeam => self.can_have_recovery_team && self.pilots_remaining,
        };
        mission_allows
            && !self.player_destroyed
            && (matches!(kind, VehicleKind::Airstrike | VehicleKind::SensorProbe)
                || self.unit_capacity_available)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnitCapabilities {
    pub can_jump: bool,
    pub can_lay_mines: bool,
    pub can_salvage: bool,
    pub can_repair: bool,
    pub is_guard_tower: bool,
    pub guard_tower_suppression: bool,
    pub attack_range: Option<FireRange>,
    pub has_queued_orders: bool,
}

/// Frame snapshot supplied by the app. `click` is a completed, non-drag left click.
#[derive(Clone, Copy, Debug)]
pub struct ControlPanelInput<'a> {
    pub selected_units: &'a [UnitCapabilities],
    pub mouse: (i32, i32),
    pub paused: bool,
    /// Mirrors `paintingMyVtol`: a dispatched VTOL cannot be cancelled.
    pub vtol_in_flight: bool,
    pub resource_points: i32,
    pub vehicle_availability: VehicleAvailability,
    pub click: Option<ControlId>,
    pub vehicle_click: Option<VehicleAction>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonVisualState {
    #[default]
    Enabled,
    Pressed,
    Disabled,
    Ambiguous,
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlButton {
    pub id: Option<ControlId>,
    pub cxx_id: i32,
    pub rect: ControlRect,
    pub help_header: u32,
    pub help_text: u32,
    pub art: ButtonArt,
    pub state: ButtonVisualState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VehicleButton {
    pub kind: Option<VehicleKind>,
    pub id: i32,
    pub rect: ControlRect,
    pub help_header: u32,
    pub help_text: u32,
    pub cost: i32,
    pub art: ButtonArt,
    pub state: ButtonVisualState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ButtonArt {
    pub texture: Option<TexId>,
    pub uv: [(f32, f32); 4],
    pub uv_size: (f32, f32),
    pub texture_size: (f32, f32),
    pub rotated: bool,
}

#[derive(Clone, Debug)]
pub struct ControlPanel {
    pub buttons: Vec<ControlButton>,
    pub vehicles: Vec<VehicleButton>,
    pub statics: Vec<(ControlRect, ButtonArt)>,
    pub rects: Vec<(ControlRect, u32)>,
    pub help_area_left: i32,
    pub help_area_bottom: i32,
    pub resource_points_origin: (i32, i32),
    pub run: bool,
    pub jump: bool,
    pub guard: bool,
    pub order_bits: u8,
    pub fire_from_current_position: bool,
    pub lay_mines: bool,
    /// The callout's flashing target: the control button's id and its colour (`getButton(id)->setColor`, `missiongui.cpp:507-511`).
    pub button_color: Option<(i32, u32)>,
    pub repair: bool,
    pub salvage: bool,
    pub guard_tower_suppression: bool,
    pub vehicle_tab: bool,
    /// `bLOS` of `ControlGui::update`: the pointer's world point is in the team's line of sight (picks the strike rollover help).
    pub line_of_sight: bool,
    /// `useLeftRightMouseProfile`: `setRolloverHelpText` bumps the help id to its "Click"/"Right-click" twin (controlgui.cpp:2919).
    pub left_right_profile: bool,
    pub selected_vehicle: Option<VehicleKind>,
    vehicle_availability: VehicleAvailability,
    selected_count: usize,
    pub resource_points: i32,
    pub help_header: u32,
    pub help_text: u32,
    pub help_font: Option<FontRef>,
    pub vehicle_font: Option<FontRef>,
    pub unavailable: &'static [&'static str],
}

fn control_id(id: i32) -> Option<ControlId> {
    Some(match id {
        0 => ControlId::DefaultRange,
        1 => ControlId::ShortRange,
        2 => ControlId::MediumRange,
        3 => ControlId::LongRange,
        4 => ControlId::Jump,
        5 => ControlId::Run,
        6 => ControlId::Guard,
        7 => ControlId::FireFromCurrentPosition,
        8 => ControlId::Stop,
        14 => ControlId::VehicleTab,
        15 => ControlId::LayMines,
        16 => ControlId::Repair,
        17 => ControlId::Salvage,
        18 => ControlId::GuardTower,
        19 => ControlId::CurrentRange,
        _ => return None,
    })
}

/// The unit bar's backdrop for a wide screen: the slot grid's four blocks (`Static10`..`13`) repeated in order
/// from where the grid ends (`grid`, in layout space, `offset` moves it to the screen) across `extra` pixels;
/// the last block is cut to fit, so nothing reaches the right group. A port extension: the original has no wide
/// screens, so its own grid art is all there is to draw.
fn grid_fill(
    grid: ControlRect,
    offset: (i32, i32),
    extra: i32,
    blocks: &[(ControlRect, ButtonArt)],
) -> Vec<(ControlRect, ButtonArt)> {
    let mut out = Vec::new();
    let end = grid.right + offset.0 + extra;
    let mut left = grid.right + offset.0;
    for (rect, art) in blocks.iter().cycle() {
        if left >= end {
            break;
        }
        let width = rect.right - rect.left;
        let shown = width.min(end - left);
        let mut art = *art;
        if shown < width {
            // the part of the art that stays: rotated art runs along its v axis
            let keep = shown as f32 / width as f32;
            if art.rotated {
                art.uv_size.1 *= keep;
            } else {
                art.uv_size.0 *= keep;
            }
        }
        out.push((
            ControlRect {
                left,
                right: left + shown,
                ..*rect
            },
            art,
        ));
        left += shown;
    }
    out
}

pub(crate) fn draw_button(
    ctx: &mut RenderCtx<'_>,
    rect: ControlRect,
    state: ButtonVisualState,
    art: ButtonArt,
) {
    draw_button_tinted(ctx, rect, state, art, 0xffffffff);
}

/// Per-channel product of two ARGB colours (the button colour `setColor` multiplies into the art).
fn modulate(a: u32, b: u32) -> u32 {
    (0..4).fold(0, |acc, i| {
        let sh = i * 8;
        let c = (((a >> sh) & 0xff) * ((b >> sh) & 0xff) + 127) / 255;
        acc | (c << sh)
    })
}

/// [`draw_button`] with the button's colour (`aObject::setColor`, `0xffffffff` when untouched).
pub(crate) fn draw_button_tinted(
    ctx: &mut RenderCtx<'_>,
    rect: ControlRect,
    state: ButtonVisualState,
    art: ButtonArt,
    color: u32,
) {
    let state_index = match state {
        ButtonVisualState::Enabled => 0,
        ButtonVisualState::Pressed => 1,
        ButtonVisualState::Disabled => 2,
        ButtonVisualState::Ambiguous => 3,
        ButtonVisualState::Hidden => return,
    };
    let tint = match state {
        ButtonVisualState::Pressed => 0xffffffff,
        ButtonVisualState::Disabled => 0xff777777,
        _ => 0xffffffff,
    };
    let tint = modulate(tint, color);
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
        let fill = match state {
            ButtonVisualState::Pressed => 0xff2478a8,
            ButtonVisualState::Disabled => 0xff45515a,
            ButtonVisualState::Ambiguous => 0xff75612d,
            _ => 0xff005392,
        };
        ctx.dl.fill_rect(
            GuiRect::new(rect.left, rect.top, rect.right, rect.bottom),
            modulate(fill, color),
        );
    }
    if state == ButtonVisualState::Pressed {
        ctx.dl.outline_rect(
            GuiRect::new(rect.left, rect.top, rect.right - 1, rect.bottom - 1),
            0xffffffff,
            0xffffffff,
        );
    }
}
const VEHICLE_KINDS: [Option<VehicleKind>; 8] = [
    Some(VehicleKind::Airstrike),
    Some(VehicleKind::GuardTower),
    Some(VehicleKind::SensorProbe),
    Some(VehicleKind::RepairTruck),
    Some(VehicleKind::Scout),
    Some(VehicleKind::Minelayer),
    Some(VehicleKind::RecoveryTeam),
    None,
];
const VEHICLE_IDS: [i32; 8] = [100, 101, 102, 103, 104, 105, 106, 107];
const VEHICLE_COSTS: [i32; 8] = [7000, 8000, 2000, 6000, 2000, 4000, 10000, 0];

impl ControlPanel {
    fn set_button_state(&mut self, id: ControlId, state: ButtonVisualState) {
        if let Some(button) = self.buttons.iter_mut().find(|button| button.id == Some(id)) {
            button.state = state;
        }
    }

    fn button_state(&self, id: ControlId) -> ButtonVisualState {
        self.buttons
            .iter()
            .find(|button| button.id == Some(id))
            .map_or(ButtonVisualState::Hidden, |button| button.state)
    }

    /// Reads the C++ `ControlButtons`, `VehicleButtons`, `Statics`, and `Rects` layout blocks.
    pub fn from_fit(
        ld: &mut Loader<'_>,
        file: &mut FitIni,
        offset: (i32, i32),
    ) -> Result<Self, String> {
        Self::from_fit_wide(ld, file, offset, 0)
    }

    /// [`ControlPanel::from_fit`] with `extra` pixels of widescreen (see [`ControlPanel::load_wide`]).
    /// The right group starts at the left edge of `Static15`, the unit bar's right shoulder.
    pub fn from_fit_wide(
        ld: &mut Loader<'_>,
        file: &mut FitIni,
        offset: (i32, i32),
        extra: i32,
    ) -> Result<Self, String> {
        let right_from = if extra == 0 {
            i32::MAX
        } else {
            file.seek_block("Statics").map_err(|e| e.to_string())?;
            file.seek_block("Static15").map_err(|e| e.to_string())?;
            file.long_or("XLocation", i32::MAX as i64) as i32
        };
        // the pixels a thing whose layout x is `x` moves right by
        let shift = move |x: i32| if x >= right_from { extra } else { 0 };
        fn block(file: &mut FitIni, name: &str) -> Result<(), String> {
            file.seek_block(name).map_err(|e| e.to_string())
        }
        fn button_art(ld: &mut Loader<'_>, file: &FitIni) -> ButtonArt {
            let texture = file.string("FileName").and_then(|name| ld.art(&name));
            let texture_size = texture
                .and_then(|id| ld.assets.textures.get(id))
                .map_or((0.0, 0.0), |image| {
                    (image.width as f32, image.height as f32)
                });
            let mut uv = [
                (
                    file.long_or("UNormal", -1) as f32,
                    file.long_or("VNormal", -1) as f32,
                ),
                (
                    file.long_or("UPressed", -1) as f32,
                    file.long_or("VPressed", -1) as f32,
                ),
                (
                    file.long_or("UDisabled", -1) as f32,
                    file.long_or("VDisabled", -1) as f32,
                ),
                (
                    file.long_or("UAmbiguous", file.long_or("UHighlight", -1)) as f32,
                    file.long_or("VAmbiguous", file.long_or("VHighlight", -1)) as f32,
                ),
            ];
            let denom_x = texture_size.0.max(1.0);
            let denom_y = texture_size.1.max(1.0);
            for coord in &mut uv {
                coord.0 /= denom_x;
                coord.1 /= denom_y;
            }
            if file.boolean("texturesRotated").unwrap_or(false) {
                // Button sheets rotate the atlas cell; the original swaps which corner gets
                // each edge UV. Keep the normalized cell coordinates and mark the rotation.
            }
            ButtonArt {
                texture,
                uv,
                uv_size: (
                    file.long_or("UWidth", 0) as f32 / denom_x,
                    file.long_or("VHeight", 0) as f32 / denom_y,
                ),
                texture_size,
                rotated: file.boolean("texturesRotated").unwrap_or(false),
            }
        }
        fn buttons(
            ld: &mut Loader<'_>,
            file: &mut FitIni,
            group: &str,
            prefix: &str,
            count_expected: usize,
            offset: (i32, i32),
            shift: &dyn Fn(i32) -> i32,
        ) -> Result<Vec<ControlButton>, String> {
            block(file, group)?;
            let n = file.long_or("ButtonCount", -1);
            if n != count_expected as i64 {
                return Err(format!(
                    "{group} ButtonCount={n}, expected {count_expected}"
                ));
            }
            let mut out = Vec::with_capacity(count_expected);
            for i in 0..count_expected {
                block(file, &format!("{prefix}{i}"))?;
                let cxx_id = file.long_or("ID", -1) as i32;
                let art = button_art(ld, file);
                out.push(ControlButton {
                    id: control_id(cxx_id),
                    cxx_id,
                    rect: ControlRect::from_fit(
                        file,
                        "XLocation",
                        "YLocation",
                        "Width",
                        "Height",
                        offset.0 + shift(file.long_or("XLocation", 0) as i32),
                        offset.1,
                    ),
                    help_header: file.long_or("HelpCaption", 0) as u32,
                    help_text: file.long_or("HelpDesc", 0) as u32,
                    art,
                    state: ButtonVisualState::Enabled,
                });
            }
            Ok(out)
        }
        let control_buttons = buttons(ld, file, "ControlButtons", "Button", 20, offset, &shift)?;
        block(file, "VehicleButtons")?;
        let vehicle_count = file.long_or("ButtonCount", -1);
        if vehicle_count != 8 {
            return Err(format!(
                "VehicleButtons ButtonCount={vehicle_count}, expected 8"
            ));
        }
        let mut vehicles = Vec::with_capacity(8);
        for i in 0..8 {
            block(file, &format!("Vehicle{i}"))?;
            let art = button_art(ld, file);
            vehicles.push(VehicleButton {
                kind: VEHICLE_KINDS[i],
                id: VEHICLE_IDS[i],
                rect: ControlRect::from_fit(
                    file,
                    "XLocation",
                    "YLocation",
                    "Width",
                    "Height",
                    offset.0 + shift(file.long_or("XLocation", 0) as i32),
                    offset.1,
                ),
                help_header: file.long_or("HelpCaption", 0) as u32,
                help_text: file.long_or("HelpDesc", 0) as u32,
                cost: VEHICLE_COSTS[i],
                art,
                state: ButtonVisualState::Enabled,
            });
        }
        block(file, "Statics")?;
        let static_count = file.long_or("staticCount", 0).max(0) as usize;
        let mut statics = Vec::with_capacity(static_count.saturating_sub(2));
        // the unit bar's slot grid (`Static10`..`13`) ends where the right group starts: more of the same grid
        // art, tiled, fills the widened part of its backdrop
        let mut grid: Option<ControlRect> = None;
        let mut grid_blocks: Vec<(ControlRect, ButtonArt)> = Vec::new();
        for i in 0..static_count {
            block(file, &format!("Static{i}"))?;
            if i == 18 || i == 20 {
                // Static18 is the tac-map compass, Static20 the multiplayer cover icon (hidden in
                // C++ by `showGUIWindow(0)`)
                continue;
            }
            let raw =
                ControlRect::from_fit(file, "XLocation", "YLocation", "Width", "Height", 0, 0);
            // a static that ends where the right group starts (the hazard bar) stretches; one that
            // starts there moves
            let (left_shift, right_shift) = if raw.left >= right_from {
                (extra, extra)
            } else if raw.right == right_from {
                (0, extra)
            } else {
                (0, 0)
            };
            if (10..=13).contains(&i) {
                grid = Some(grid.map_or(raw, |g| ControlRect {
                    right: g.right.max(raw.right),
                    ..g
                }));
            }
            let rect = ControlRect {
                left: raw.left + offset.0 + left_shift,
                top: raw.top + offset.1,
                right: raw.right + offset.0 + right_shift,
                bottom: raw.bottom + offset.1,
            };
            let art = button_art(ld, file);
            if (10..=13).contains(&i) {
                grid_blocks.push((rect, art));
            }
            statics.push((rect, art));
        }
        block(file, "Rects")?;
        let rect_count = file.long_or("rectCount", 0).max(0) as usize;
        let mut rects = Vec::with_capacity(rect_count + 1);
        for i in 0..rect_count {
            block(file, &format!("Rect{i}"))?;
            let (left, right) = (
                file.long_or("left", 0) as i32,
                file.long_or("right", 0) as i32,
            );
            let r = ControlRect {
                left: left + offset.0 + shift(left),
                top: file.long_or("top", 0) as i32 + offset.1,
                right: right + offset.0 + shift(left),
                bottom: file.long_or("bottom", 0) as i32 + offset.1,
            };
            rects.push((r, file.long_or("color", 0) as u32));
        }
        if let Some(g) = grid.filter(|_| extra != 0 && !grid_blocks.is_empty()) {
            statics.extend(grid_fill(g, offset, extra, &grid_blocks));
        }
        block(file, "HelpText")?;
        let help_area_left = file.long_or("HelpTextLeft", 0) as i32 + offset.0;
        let help_area_bottom = file.long_or("HelpTextBottom", 0) as i32 + offset.1;
        block(file, "ResourcePoints")?;
        let resource_points_origin = (
            file.long_or("RPTextLeft", 0) as i32 + offset.0,
            file.long_or("RPTextTop", 0) as i32 + offset.1,
        );
        block(file, "Fonts")?;
        let help_font = Some(ld.font(file.long_or("HelpFont", 0) as u32));
        let vehicle_font = Some(ld.font(file.long_or("vehicleFont", 0) as u32));
        Ok(Self {
            buttons: control_buttons,
            vehicles,
            statics,
            rects,
            help_area_left,
            help_area_bottom,
            resource_points_origin,
            run: false,
            jump: false,
            guard: false,
            order_bits: 0,
            fire_from_current_position: false,
            lay_mines: false,
            button_color: None,
            repair: false,
            salvage: false,
            guard_tower_suppression: false,
            vehicle_tab: false,
            line_of_sight: false,
            left_right_profile: false,
            selected_vehicle: None,
            vehicle_availability: VehicleAvailability::default(),
            selected_count: 0,
            resource_points: 0,
            help_header: 0,
            help_text: 0,
            help_font,
            vehicle_font,
            unavailable: &[
                "objectives",
                "results",
                "info tab",
                "tactical map",
                "movie playback",
                "unit bar",
            ],
        })
    }

    pub fn load(
        ld: &mut Loader<'_>,
        resolution_width: u32,
        resolution_height: u32,
    ) -> Result<Self, String> {
        Self::load_wide(ld, resolution_width, resolution_height, 0)
    }

    /// [`ControlPanel::load`] for a screen `extra` layout pixels wider than the layout's own block (a
    /// port extension; the original has no wide screens): the command panel and the right end of the
    /// unit bar frame move right by `extra`, the hazard bar between them stretches and a black fill
    /// closes the unit bar backdrop, while the minimap side stays where the layout puts it.
    pub fn load_wide(
        ld: &mut Loader<'_>,
        resolution_width: u32,
        resolution_height: u32,
        extra: i32,
    ) -> Result<Self, String> {
        let name = match resolution_width {
            640 => "buttonlayout640",
            800 => "buttonlayout800",
            1024 => "buttonlayout1024",
            1280 => "buttonlayout1280",
            1600 => "buttonlayout1600",
            1920 => "buttonlayout1920",
            _ => {
                return Err(format!(
                    "unsupported control layout width {resolution_width}"
                ))
            }
        };
        let mut file = ld.art_fit(name)?;
        let x_offset = if file.seek_block("HiresOffsets").is_ok() {
            file.long_or("xOffset", 0) as i32
        } else {
            0
        };
        let base_height = match resolution_width {
            640 => 480,
            800 => 600,
            _ => 768,
        };
        // a wide screen puts the HUD block at its left edge: the layout's own xOffset centres the block
        let x_offset = if extra > 0 { 0 } else { x_offset };
        let offset = (x_offset, resolution_height as i32 - base_height);
        Self::from_fit_wide(ld, &mut file, offset, extra)
    }

    pub fn update(&mut self, input: ControlPanelInput<'_>) -> Vec<ControlIntent> {
        let mut intents = Vec::new();
        self.resource_points = input.resource_points;
        let n = input.selected_units.len();
        self.selected_count = n;
        let all = |f: fn(&UnitCapabilities) -> bool| n > 0 && input.selected_units.iter().all(f);
        let can_jump = all(|u| u.can_jump);
        let mining = all(|u| u.can_lay_mines);
        let salvaging = all(|u| u.can_salvage);
        let repairing = all(|u| u.can_repair);
        let towers = all(|u| u.is_guard_tower);
        let specialty = if mining {
            Some(ControlId::LayMines)
        } else if salvaging {
            Some(ControlId::Salvage)
        } else if repairing {
            Some(ControlId::Repair)
        } else if towers {
            Some(ControlId::GuardTower)
        } else {
            None
        };
        let mut ranges = [0usize; 5];
        let mut queued = false;
        for unit in input.selected_units {
            if let Some(range) = unit.attack_range {
                ranges[range as usize] += 1;
            }
            queued |= unit.has_queued_orders;
        }
        for button in &mut self.buttons {
            button.state = ButtonVisualState::Enabled;
        }
        for id in [
            ControlId::DefaultRange,
            ControlId::ShortRange,
            ControlId::MediumRange,
            ControlId::LongRange,
            ControlId::Jump,
            ControlId::Run,
            ControlId::Guard,
            ControlId::FireFromCurrentPosition,
        ] {
            self.set_button_state(
                id,
                if n == 0 {
                    ButtonVisualState::Disabled
                } else {
                    ButtonVisualState::Enabled
                },
            );
        }
        self.set_button_state(
            ControlId::Jump,
            if can_jump {
                ButtonVisualState::Enabled
            } else {
                ButtonVisualState::Disabled
            },
        );
        self.set_button_state(
            ControlId::Stop,
            if n == 0 && !queued && self.selected_vehicle.is_none() {
                ButtonVisualState::Disabled
            } else {
                ButtonVisualState::Enabled
            },
        );
        for id in [
            ControlId::DefaultRange,
            ControlId::ShortRange,
            ControlId::MediumRange,
            ControlId::LongRange,
        ] {
            // These four IDs are marked DEFUNCT in the shipped FIT and hidden by
            // ControlGui::update after the range state has been computed.
            self.set_button_state(id, ButtonVisualState::Hidden);
        }
        self.set_button_state(
            ControlId::CurrentRange,
            if n == 0 {
                ButtonVisualState::Disabled
            } else if ranges[4] == 0 {
                ButtonVisualState::Enabled
            } else if ranges[4] == n {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Ambiguous
            },
        );
        for (id, eligible) in [
            (ControlId::LayMines, specialty == Some(ControlId::LayMines)),
            (ControlId::Repair, specialty == Some(ControlId::Repair)),
            (ControlId::Salvage, specialty == Some(ControlId::Salvage)),
            (
                ControlId::GuardTower,
                specialty == Some(ControlId::GuardTower),
            ),
        ] {
            self.set_button_state(
                id,
                if eligible {
                    ButtonVisualState::Enabled
                } else {
                    ButtonVisualState::Hidden
                },
            );
        }
        self.set_button_state(
            ControlId::LayMines,
            if specialty == Some(ControlId::LayMines) && self.lay_mines {
                ButtonVisualState::Pressed
            } else if specialty == Some(ControlId::LayMines) {
                ButtonVisualState::Enabled
            } else {
                ButtonVisualState::Hidden
            },
        );
        self.set_button_state(
            ControlId::Repair,
            if specialty == Some(ControlId::Repair) {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Hidden
            },
        );
        self.set_button_state(
            ControlId::Salvage,
            if specialty == Some(ControlId::Salvage) {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Hidden
            },
        );
        self.guard_tower_suppression = specialty == Some(ControlId::GuardTower)
            && input
                .selected_units
                .iter()
                .all(|unit| unit.guard_tower_suppression);
        self.set_button_state(
            ControlId::GuardTower,
            if specialty == Some(ControlId::GuardTower) && self.guard_tower_suppression {
                ButtonVisualState::Pressed
            } else if specialty == Some(ControlId::GuardTower) {
                ButtonVisualState::Enabled
            } else {
                ButtonVisualState::Hidden
            },
        );
        for (id, pressed) in [
            (ControlId::Jump, self.jump),
            (ControlId::Run, self.run),
            (ControlId::Guard, self.guard),
            (
                ControlId::FireFromCurrentPosition,
                self.fire_from_current_position,
            ),
            (ControlId::VehicleTab, self.vehicle_tab),
        ] {
            if pressed {
                self.set_button_state(id, ButtonVisualState::Pressed);
            }
        }
        let over = self.buttons.iter().find(|b| {
            b.id.is_some()
                && b.state != ButtonVisualState::Hidden
                && b.rect.contains(input.mouse.0, input.mouse.1)
        });
        if let Some(b) = over {
            self.help_header = b.help_header;
            self.help_text = b.help_text;
        }
        let over_vehicle = self.vehicle_tab
            && self
                .vehicles
                .iter()
                .any(|b| b.rect.contains(input.mouse.0, input.mouse.1));
        let in_region = self
            .buttons
            .iter()
            .any(|b| b.id.is_some() && b.rect.contains(input.mouse.0, input.mouse.1))
            || self
                .statics
                .iter()
                .any(|(rect, _)| rect.contains(input.mouse.0, input.mouse.1))
            || self
                .rects
                .iter()
                .any(|(rect, _)| rect.contains(input.mouse.0, input.mouse.1));
        if !in_region {
            self.help_header = 0;
            self.help_text = 0;
        } else if !over_vehicle && over.is_none() {
            let default_help = match specialty {
                Some(ControlId::LayMines) => 45100, // IDS_MINELAYER_HELP
                Some(ControlId::Salvage) => 45120,  // IDS_RECOVER_HELP
                Some(ControlId::Repair) => 45102,   // IDS_REPAIR_HELP
                _ => 0,
            };
            if default_help != 0 {
                self.help_header = 0;
                self.help_text = default_help;
            }
        }
        if self.vehicle_tab {
            // `updateVehicleTab` (controlgui.cpp:1763-1772): a pressed button's help is IDS_AIRSTRIKE_HELP_NO_LOS + 2 * index, the
            // airstrike's is IDS_AIRSTRIKE_HELP while the pointer is in line of sight.
            if let Some(i) = self
                .vehicles
                .iter()
                .position(|b| b.kind.is_some() && b.kind == self.selected_vehicle)
            {
                self.help_header = 0;
                let base = if i == 0 && self.line_of_sight {
                    45104
                } else {
                    45106 + 2 * i as u32
                };
                self.help_text = base + u32::from(self.left_right_profile);
            }
            for b in &mut self.vehicles {
                b.state = if b.id == VEHICLE_IDS[7] {
                    ButtonVisualState::Enabled
                } else if b.kind.is_none()
                    || !input
                        .vehicle_availability
                        .permits(b.kind.expect("kind checked above"))
                    || (b.cost > input.resource_points && self.selected_vehicle != b.kind)
                    || (self.selected_vehicle.is_some() && self.selected_vehicle != b.kind)
                {
                    ButtonVisualState::Disabled
                } else {
                    ButtonVisualState::Enabled
                };
                if self.selected_vehicle.is_some() && b.kind == self.selected_vehicle {
                    b.state = ButtonVisualState::Pressed;
                }
            }
            if let Some(b) = self
                .vehicles
                .iter()
                .find(|b| b.rect.contains(input.mouse.0, input.mouse.1))
            {
                self.help_header = b.help_header;
                self.help_text = b.help_text;
            }
            if let Some(action) = input.vehicle_click {
                match action {
                    VehicleAction::Build(kind) => {
                        let vehicle = self.vehicles.iter().find(|b| {
                            b.kind == Some(kind) && b.state == ButtonVisualState::Enabled
                        });
                        if let Some(vehicle) = vehicle {
                            self.selected_vehicle = Some(kind);
                            self.resource_points -= vehicle.cost;
                            intents.push(ControlIntent::BuildVehicle(kind));
                        }
                    }
                    VehicleAction::Stop => {
                        if !input.vtol_in_flight
                            && self.vehicles.iter().any(|b| b.id == VEHICLE_IDS[7])
                        {
                            if let Some(kind) = self.selected_vehicle.take() {
                                let refund = self
                                    .vehicles
                                    .iter()
                                    .find(|b| b.kind == Some(kind))
                                    .map_or(0, |b| b.cost);
                                self.resource_points += refund;
                                intents.push(ControlIntent::CancelVehicle { kind, refund });
                            }
                        }
                    }
                }
            }
        }
        if let Some(id) = input.click {
            let eligible = match id {
                ControlId::Jump => can_jump,
                ControlId::LayMines
                | ControlId::Repair
                | ControlId::Salvage
                | ControlId::GuardTower => specialty == Some(id),
                ControlId::CurrentRange
                | ControlId::FireFromCurrentPosition
                | ControlId::Run
                | ControlId::Guard => n > 0,
                ControlId::Stop => n > 0 || self.selected_vehicle.is_some(),
                ControlId::VehicleTab => true,
                ControlId::DefaultRange
                | ControlId::ShortRange
                | ControlId::MediumRange
                | ControlId::LongRange => false,
            };
            if eligible {
                match id {
                    ControlId::Jump => {
                        self.order_bits ^= 0x04;
                        self.order_bits &= 0x04;
                        self.sync_order_bits();
                        intents.push(ControlIntent::ToggleJump);
                    }
                    ControlId::Run => {
                        self.order_bits ^= 0x01;
                        self.order_bits &= 0x03;
                        self.sync_order_bits();
                        intents.push(ControlIntent::SetRun(self.run));
                    }
                    ControlId::Guard => {
                        self.order_bits ^= 0x02;
                        self.order_bits &= 0x03;
                        self.sync_order_bits();
                        intents.push(ControlIntent::SetGuard(self.guard));
                    }
                    ControlId::DefaultRange => {
                        intents.push(ControlIntent::SetRange(FireRange::Optimal))
                    }
                    ControlId::ShortRange => {
                        intents.push(ControlIntent::SetRange(FireRange::Short))
                    }
                    ControlId::MediumRange => {
                        intents.push(ControlIntent::SetRange(FireRange::Medium))
                    }
                    ControlId::LongRange => intents.push(ControlIntent::SetRange(FireRange::Long)),
                    ControlId::CurrentRange => {
                        let current = self.button_state(ControlId::CurrentRange)
                            == ButtonVisualState::Enabled;
                        intents.push(ControlIntent::SetRange(if current {
                            FireRange::Current
                        } else {
                            FireRange::Optimal
                        }));
                    }
                    ControlId::FireFromCurrentPosition => {
                        self.fire_from_current_position = !self.fire_from_current_position;
                        intents.push(ControlIntent::SetFireFromCurrentPosition(
                            self.fire_from_current_position,
                        ));
                    }
                    ControlId::Stop => {
                        self.order_bits = 0;
                        self.sync_order_bits();
                        self.fire_from_current_position = false;
                        self.lay_mines = false;
                        self.repair = false;
                        self.salvage = false;
                        if n > 0 {
                            intents.push(ControlIntent::Stop);
                        }
                        if !input.vtol_in_flight {
                            if let Some(kind) = self.selected_vehicle.take() {
                                let refund = self
                                    .vehicles
                                    .iter()
                                    .find(|button| button.kind == Some(kind))
                                    .map_or(0, |button| button.cost);
                                self.resource_points += refund;
                                intents.push(ControlIntent::CancelVehicle { kind, refund });
                            }
                        }
                    }
                    ControlId::LayMines => {
                        self.lay_mines = !self.lay_mines;
                        intents.push(ControlIntent::ToggleMines);
                    }
                    ControlId::Repair => {
                        self.repair = !self.repair;
                        intents.push(ControlIntent::ToggleRepair);
                    }
                    ControlId::Salvage => {
                        self.salvage = !self.salvage;
                        intents.push(ControlIntent::ToggleSalvage);
                    }
                    ControlId::GuardTower => {
                        self.guard_tower_suppression = !self.guard_tower_suppression;
                        intents.push(ControlIntent::SetGuardTower(self.guard_tower_suppression));
                    }
                    ControlId::VehicleTab => self.vehicle_tab = true,
                }
            }
        }
        // `handleClick` presses toggle buttons before ControlGui refreshes them for rendering.
        self.set_button_state(
            ControlId::Jump,
            if !can_jump {
                ButtonVisualState::Disabled
            } else if self.jump {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            },
        );
        self.set_button_state(
            ControlId::Run,
            if n == 0 {
                ButtonVisualState::Disabled
            } else if self.run {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            },
        );
        self.set_button_state(
            ControlId::Guard,
            if n == 0 {
                ButtonVisualState::Disabled
            } else if self.guard {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            },
        );
        self.set_button_state(
            ControlId::FireFromCurrentPosition,
            if self.fire_from_current_position {
                ButtonVisualState::Pressed
            } else if n == 0 {
                ButtonVisualState::Disabled
            } else {
                ButtonVisualState::Enabled
            },
        );
        self.set_button_state(
            ControlId::VehicleTab,
            if self.vehicle_tab {
                ButtonVisualState::Pressed
            } else {
                ButtonVisualState::Enabled
            },
        );
        self.vehicle_availability = input.vehicle_availability;
        self.refresh_vehicle_buttons();
        if self.selected_vehicle.is_some() {
            self.set_button_state(ControlId::Stop, ButtonVisualState::Enabled);
        }
        intents
    }

    /// Vehicle button states from the selection, availability and resource points.
    fn refresh_vehicle_buttons(&mut self) {
        let (selected, availability, points) = (
            self.selected_vehicle,
            self.vehicle_availability,
            self.resource_points,
        );
        for button in &mut self.vehicles {
            if button.id == VEHICLE_IDS[7] {
                button.state = ButtonVisualState::Enabled;
            } else if let Some(kind) = button.kind {
                button.state = if selected == Some(kind) {
                    ButtonVisualState::Pressed
                } else if selected.is_some() || !availability.permits(kind) || button.cost > points
                {
                    ButtonVisualState::Disabled
                } else {
                    ButtonVisualState::Enabled
                };
            }
        }
    }

    /// Force `curOrder` (`RUN | GUARD | JUMP`), e.g. `setDefaultSpeed` after a move order.
    pub fn set_order_bits(&mut self, bits: u8) {
        self.order_bits = bits & 0x07;
        self.sync_order_bits();
    }

    /// `ControlGui::unPressAllVehicleButtons` (controlgui.cpp:2073): the VTOL has left, so the bought vehicle's button is released and
    /// nothing is refunded.
    pub fn unpress_vehicle_buttons(&mut self) {
        if self.selected_vehicle.take().is_some() {
            self.refresh_vehicle_buttons();
            if self.selected_count == 0 {
                self.set_button_state(ControlId::Stop, ButtonVisualState::Disabled);
            }
        }
    }

    /// `ControlGui::pressAirstrikeButton` / `pressSensorStrikeButton` (controlgui.h:252-274): the hot key presses the button as
    /// a click would. The C++ presses it even when the button reads disabled (no cost check, no restriction check); that is a
    /// bug and is not reproduced: only an enabled button can be pressed (rust/docs/DIFFERENCES.md).
    pub fn press_vehicle(
        &mut self,
        kind: VehicleKind,
        vtol_in_flight: bool,
    ) -> Option<ControlIntent> {
        if vtol_in_flight {
            return None;
        }
        let vehicle = self
            .vehicles
            .iter()
            .find(|b| b.kind == Some(kind) && b.state == ButtonVisualState::Enabled)?;
        let cost = vehicle.cost;
        self.selected_vehicle = Some(kind);
        self.resource_points -= cost;
        self.refresh_vehicle_buttons();
        Some(ControlIntent::BuildVehicle(kind))
    }

    /// Un-press the support button armed by the last `BuildVehicle` intent and refund its cost
    /// (a cancelled strike: nothing was called, so nothing is spent).
    pub fn release_vehicle_button(&mut self) {
        if let Some(kind) = self.selected_vehicle.take() {
            let refund = self
                .vehicles
                .iter()
                .find(|b| b.kind == Some(kind))
                .map_or(0, |b| b.cost);
            self.resource_points += refund;
            self.refresh_vehicle_buttons();
            if self.selected_count == 0 {
                self.set_button_state(ControlId::Stop, ButtonVisualState::Disabled);
            }
        }
    }

    fn sync_order_bits(&mut self) {
        self.run = self.order_bits & 0x01 != 0;
        self.guard = self.order_bits & 0x02 != 0;
        self.jump = self.order_bits & 0x04 != 0;
    }

    pub fn render(&self, ctx: &mut RenderCtx<'_>) {
        let (w, h) = ctx.dl.screen;
        // `ControlGui::render` draws fit rectangles, static art, then the tab and buttons.
        for (rect, color) in &self.rects {
            ctx.dl.fill_rect(
                GuiRect::new(rect.left, rect.top, rect.right, rect.bottom),
                *color,
            );
        }
        for (rect, art) in &self.statics {
            draw_button(ctx, *rect, ButtonVisualState::Enabled, *art);
        }
        for b in &self.buttons {
            if b.id.is_none() {
                continue;
            }
            let color = match self.button_color {
                Some((id, c)) if id == b.cxx_id => c,
                _ => 0xffffffff,
            };
            draw_button_tinted(ctx, b.rect, b.state, b.art, color);
        }
        if self.vehicle_tab {
            // `renderVehicleTab` (`controlgui.cpp:1905-1938`) draws the resource points; the other tabs do not
            if let Some(font) = self.vehicle_font {
                crate::widgets::basic::font_render(
                    ctx,
                    font,
                    &self.resource_points.to_string(),
                    self.resource_points_origin.0,
                    self.resource_points_origin.1,
                    140,
                    32,
                    0xff5c96c2,
                    false,
                    0,
                );
            }
            for b in &self.vehicles {
                draw_button(ctx, b.rect, b.state, b.art);
                if b.cost > 0 {
                    if let Some(font) = self.vehicle_font {
                        crate::widgets::basic::font_render(
                            ctx,
                            font,
                            &b.cost.to_string(),
                            b.rect.left,
                            b.rect.bottom + (5 * h / 600),
                            b.rect.right - b.rect.left,
                            30 + (5 * h / 600),
                            if b.state == ButtonVisualState::Disabled {
                                0xff7f7f7f
                            } else {
                                0xff005392
                            },
                            false,
                            2,
                        );
                    }
                }
            }
        }
        if self.help_text != 0 {
            if let Some(font) = self.help_font {
                let body = ctx.assets.string(self.help_text);
                let line = crate::widgets::basic::font_height(ctx.assets, font).max(1);
                let height = crate::widgets::basic::font_text_height(
                    ctx.assets,
                    font,
                    body,
                    (w - self.help_area_left).max(0),
                );
                crate::widgets::basic::font_render(
                    ctx,
                    font,
                    body,
                    self.help_area_left,
                    self.help_area_bottom - height,
                    w - self.help_area_left,
                    (line * 3).max(height),
                    0xffffffff,
                    false,
                    0,
                );
                if self.help_header != 0 {
                    let header = ctx.assets.string(self.help_header);
                    crate::widgets::basic::font_render(
                        ctx,
                        font,
                        header,
                        self.help_area_left,
                        self.help_area_bottom - height - line,
                        w - self.help_area_left,
                        line,
                        0xffffffff,
                        true,
                        0,
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_vehicle_availability() -> VehicleAvailability {
        VehicleAvailability {
            can_have_airstrike: true,
            can_have_guard_tower: true,
            can_have_sensor_probe: true,
            can_have_repair_truck: true,
            can_have_scout: true,
            can_have_minelayer: true,
            can_have_recovery_team: true,
            unit_capacity_available: true,
            pilots_remaining: true,
            player_destroyed: false,
        }
    }

    #[test]
    fn rotated_button_art_maps_the_atlas_cell_clockwise() {
        let assets = crate::UiAssets::default();
        let mut draw = crate::draw::DrawList::new(100, 100);
        let mut ctx = RenderCtx {
            dl: &mut draw,
            assets: &assets,
            dt: 0.0,
        };
        draw_button(
            &mut ctx,
            ControlRect {
                left: 10,
                top: 20,
                right: 30,
                bottom: 40,
            },
            ButtonVisualState::Enabled,
            ButtonArt {
                texture: Some(TexId(7)),
                uv: [(0.1, 0.2); 4],
                uv_size: (0.3, 0.4),
                texture_size: (1000.0, 1000.0),
                rotated: true,
            },
        );
        let actual: Vec<_> = draw.vertices.iter().map(|v| (v.u, v.v)).collect();
        let bias = 0.1 / 1000.0;
        assert_eq!(
            actual,
            [
                (0.4 + bias, 0.2 + bias),
                (0.1 + bias, 0.2 + bias),
                (0.1 + bias, 0.6 + bias),
                (0.4 + bias, 0.6 + bias),
            ]
        );
    }

    fn panel() -> ControlPanel {
        ControlPanel {
            buttons: (0..20)
                .map(|i| ControlButton {
                    id: control_id(i),
                    cxx_id: i,
                    art: ButtonArt::default(),
                    rect: ControlRect {
                        left: i * 20,
                        top: 0,
                        right: i * 20 + 16,
                        bottom: 16,
                    },
                    help_header: 0,
                    help_text: 0,
                    state: ButtonVisualState::Enabled,
                })
                .collect(),
            vehicles: vec![],
            statics: vec![],
            rects: vec![],
            help_area_left: 0,
            help_area_bottom: 0,
            resource_points_origin: (0, 0),
            run: false,
            jump: false,
            guard: false,
            order_bits: 0,
            fire_from_current_position: false,
            lay_mines: false,
            button_color: None,
            repair: false,
            salvage: false,
            guard_tower_suppression: false,
            vehicle_tab: false,
            line_of_sight: false,
            left_right_profile: false,
            selected_vehicle: None,
            vehicle_availability: VehicleAvailability::default(),
            selected_count: 0,
            resource_points: 0,
            help_header: 0,
            help_text: 0,
            help_font: None,
            vehicle_font: None,
            unavailable: &[],
        }
    }

    #[test]
    fn orders_follow_controlgui_bitmask_transitions() {
        // `toggleJump`, `toggleDefaultSpeed`, `toggleGuard`: controlgui.cpp:1351-1397.
        let unit = [UnitCapabilities {
            can_jump: true,
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        assert_eq!(
            p.update(ControlPanelInput {
                selected_units: &unit,
                mouse: (-1, -1),
                paused: false,
                vtol_in_flight: false,
                resource_points: 0,
                vehicle_availability: VehicleAvailability::default(),
                click: Some(ControlId::Run),
                vehicle_click: None
            }),
            [ControlIntent::SetRun(true)]
        );
        assert_eq!(
            p.update(ControlPanelInput {
                selected_units: &unit,
                mouse: (-1, -1),
                paused: false,
                vtol_in_flight: false,
                resource_points: 0,
                vehicle_availability: VehicleAvailability::default(),
                click: Some(ControlId::Jump),
                vehicle_click: None
            }),
            [ControlIntent::ToggleJump]
        );
        assert!(p.jump && !p.run && !p.guard);
        assert_eq!(
            p.update(ControlPanelInput {
                selected_units: &unit,
                mouse: (-1, -1),
                paused: false,
                vtol_in_flight: false,
                resource_points: 0,
                vehicle_availability: VehicleAvailability::default(),
                click: Some(ControlId::Run),
                vehicle_click: None,
            }),
            [ControlIntent::SetRun(true)]
        );
        assert!(p.run && !p.jump && !p.guard);
        p.update(ControlPanelInput {
            selected_units: &unit,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: Some(ControlId::Guard),
            vehicle_click: None,
        });
        assert!(p.run && p.guard && !p.jump);
    }

    #[test]
    fn shipped_defunct_ranges_stay_hidden_and_cannot_emit_intents() {
        // The four range buttons are hidden after every update: controlgui.cpp:1235-1238.
        let unit = [UnitCapabilities {
            attack_range: Some(FireRange::Long),
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        let intents = p.update(ControlPanelInput {
            selected_units: &unit,
            mouse: (25, 5),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: Some(ControlId::ShortRange),
            vehicle_click: None,
        });
        assert!(intents.is_empty());
        for id in [
            ControlId::DefaultRange,
            ControlId::ShortRange,
            ControlId::MediumRange,
            ControlId::LongRange,
        ] {
            assert_eq!(p.button_state(id), ButtonVisualState::Hidden, "{id:?}");
        }
        assert_eq!(
            p.button_state(ControlId::CurrentRange),
            ButtonVisualState::Enabled
        );
        assert_eq!(p.help_text, 0, "hidden buttons cannot publish hover help");
    }

    #[test]
    fn clicked_toggle_visuals_are_ready_for_render_in_the_same_update() {
        // handleClick precedes button refresh in the C++ update/render cycle: controlgui.cpp:934-979, 1538-1599.
        let unit = [UnitCapabilities {
            can_jump: true,
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        let input = |click| ControlPanelInput {
            selected_units: &unit,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: Some(click),
            vehicle_click: None,
        };
        p.update(input(ControlId::Run));
        assert_eq!(p.button_state(ControlId::Run), ButtonVisualState::Pressed);
        let assets = crate::UiAssets::default();
        let mut draw = crate::draw::DrawList::new(800, 600);
        p.render(&mut RenderCtx {
            dl: &mut draw,
            assets: &assets,
            dt: 0.0,
        });
        assert!(
            draw.vertices.iter().any(|vertex| {
                vertex.x == 100.0 && vertex.y == 0.0 && vertex.argb == 0xff2478a8
            }),
            "Run is rendered pressed in the same update"
        );
        p.update(input(ControlId::Jump));
        assert_eq!(p.button_state(ControlId::Jump), ButtonVisualState::Pressed);
        assert_eq!(p.button_state(ControlId::Run), ButtonVisualState::Enabled);
        p.update(input(ControlId::Guard));
        assert_eq!(p.button_state(ControlId::Guard), ButtonVisualState::Pressed);
        p.update(input(ControlId::VehicleTab));
        assert_eq!(
            p.button_state(ControlId::VehicleTab),
            ButtonVisualState::Pressed
        );
        p.update(input(ControlId::Stop));
        for id in [
            ControlId::Run,
            ControlId::Jump,
            ControlId::Guard,
            ControlId::FireFromCurrentPosition,
        ] {
            assert_ne!(p.button_state(id), ButtonVisualState::Pressed, "{id:?}");
        }
        assert_eq!(p.button_state(ControlId::Stop), ButtonVisualState::Enabled);
    }

    #[test]
    fn each_vehicle_permission_capacity_pilot_and_destroyed_restriction_blocks_purchase() {
        // Mission flags, max icons, pilot availability and all-units-destroyed gates:
        // controlgui.cpp:1810-1883. IDs/costs are independently cited in controlgui.cpp:85-95.
        let costs = [7000, 8000, 2000, 6000, 2000, 4000, 10000];
        let ids = [100, 101, 102, 103, 104, 105, 106];
        let kinds = [
            VehicleKind::Airstrike,
            VehicleKind::GuardTower,
            VehicleKind::SensorProbe,
            VehicleKind::RepairTruck,
            VehicleKind::Scout,
            VehicleKind::Minelayer,
            VehicleKind::RecoveryTeam,
        ];
        let permissive = VehicleAvailability {
            can_have_airstrike: true,
            can_have_guard_tower: true,
            can_have_sensor_probe: true,
            can_have_repair_truck: true,
            can_have_scout: true,
            can_have_minelayer: true,
            can_have_recovery_team: true,
            unit_capacity_available: true,
            pilots_remaining: true,
            player_destroyed: false,
        };
        let mut p = panel();
        p.vehicle_tab = true;
        p.vehicles = kinds
            .iter()
            .zip(ids)
            .zip(costs)
            .enumerate()
            .map(|(i, ((kind, id), cost))| VehicleButton {
                kind: Some(*kind),
                id,
                rect: ControlRect {
                    left: i as i32 * 10,
                    top: 20,
                    right: i as i32 * 10 + 8,
                    bottom: 28,
                },
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .chain(std::iter::once(VehicleButton {
                kind: None,
                id: 107,
                rect: ControlRect::default(),
                help_header: 0,
                help_text: 0,
                cost: 0,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            }))
            .collect();
        for (index, kind) in kinds.into_iter().enumerate() {
            let mut blocked = permissive;
            match kind {
                VehicleKind::Airstrike => blocked.can_have_airstrike = false,
                VehicleKind::GuardTower => blocked.can_have_guard_tower = false,
                VehicleKind::SensorProbe => blocked.can_have_sensor_probe = false,
                VehicleKind::RepairTruck => blocked.can_have_repair_truck = false,
                VehicleKind::Scout => blocked.can_have_scout = false,
                VehicleKind::Minelayer => blocked.can_have_minelayer = false,
                VehicleKind::RecoveryTeam => blocked.can_have_recovery_team = false,
            }
            assert_purchase_blocked(&mut p, kind, ids[index], costs[index], blocked);
        }
        for (kind, blocked) in [
            (
                VehicleKind::RepairTruck,
                VehicleAvailability {
                    unit_capacity_available: false,
                    ..permissive
                },
            ),
            (
                VehicleKind::RecoveryTeam,
                VehicleAvailability {
                    pilots_remaining: false,
                    ..permissive
                },
            ),
            (
                VehicleKind::Airstrike,
                VehicleAvailability {
                    player_destroyed: true,
                    ..permissive
                },
            ),
        ] {
            let i = kinds
                .iter()
                .position(|candidate| *candidate == kind)
                .unwrap();
            assert_purchase_blocked(&mut p, kind, ids[i], costs[i], blocked);
        }
    }

    fn assert_purchase_blocked(
        panel: &mut ControlPanel,
        kind: VehicleKind,
        id: i32,
        cost: i32,
        availability: VehicleAvailability,
    ) {
        let no_units = [];
        let result = panel.update(ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: cost,
            vehicle_availability: availability,
            click: None,
            vehicle_click: Some(VehicleAction::Build(kind)),
        });
        let vehicle = panel
            .vehicles
            .iter()
            .find(|button| button.id == id)
            .unwrap();
        assert_eq!(vehicle.cost, cost);
        assert_eq!(vehicle.state, ButtonVisualState::Disabled, "{kind:?}");
        assert!(result.is_empty(), "{kind:?}");
    }

    #[test]
    fn selected_vehicle_tab_stays_open_and_stop_emits_explicit_refund() {
        // The vehicle tab stays pressed; STOP_VEHICLE cancels and refunds the prior selection:
        // controlgui.cpp:1600, 1971-2050.
        let no_units = [];
        let mut p = panel();
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .enumerate()
            .map(|(i, ((kind, id), cost))| VehicleButton {
                kind,
                id,
                rect: ControlRect {
                    left: i as i32 * 10,
                    top: 20,
                    right: i as i32 * 10 + 8,
                    bottom: 28,
                },
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        let allowed = VehicleAvailability {
            can_have_sensor_probe: true,
            ..VehicleAvailability::default()
        };
        let input = |click, vehicle_click, resource_points| ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points,
            vehicle_availability: allowed,
            click,
            vehicle_click,
        };
        p.update(input(Some(ControlId::VehicleTab), None, 5000));
        p.update(input(
            None,
            Some(VehicleAction::Build(VehicleKind::SensorProbe)),
            5000,
        ));
        p.update(input(Some(ControlId::VehicleTab), None, 3000));
        assert!(p.vehicle_tab);
        assert_eq!(p.selected_vehicle, Some(VehicleKind::SensorProbe));
        assert_eq!(
            p.update(input(None, Some(VehicleAction::Stop), 3000)),
            [ControlIntent::CancelVehicle {
                kind: VehicleKind::SensorProbe,
                refund: 2000
            }]
        );
        assert!(p.vehicle_tab);
        assert_eq!(p.selected_vehicle, None);
    }

    #[test]
    fn vehicle_stop_only_cancels_purchase_and_never_stops_selected_units() {
        // `handleVehicleClick(STOP_VEHICLE)` only clears the pending vehicle selection:
        // controlgui.cpp:2013-2029. Tactical stop is `doStop`: :1669-1700.
        let units = [UnitCapabilities {
            has_queued_orders: true,
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        p.vehicle_tab = true;
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .map(|((kind, id), cost)| VehicleButton {
                kind,
                id,
                rect: ControlRect::default(),
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        let availability = VehicleAvailability {
            can_have_sensor_probe: true,
            ..VehicleAvailability::default()
        };
        let input = |vehicle_click| ControlPanelInput {
            selected_units: &units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 3000,
            vehicle_availability: availability,
            click: None,
            vehicle_click,
        };
        p.update(input(Some(VehicleAction::Build(VehicleKind::SensorProbe))));
        assert_eq!(
            p.update(input(Some(VehicleAction::Stop))),
            [ControlIntent::CancelVehicle {
                kind: VehicleKind::SensorProbe,
                refund: 2000
            }]
        );
        assert_eq!(p.selected_vehicle, None);
        assert!(!p.run && !p.jump && !p.guard);
        assert!(p.update(input(Some(VehicleAction::Stop))).is_empty());
    }

    #[test]
    fn main_stop_cancels_purchase_even_with_no_selected_units() {
        // `doStop` cancels adding artillery/vehicle/salvage regardless of mover selection:
        // controlgui.cpp:1697-1701.
        let no_units = [];
        let mut p = panel();
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .map(|((kind, id), cost)| VehicleButton {
                kind,
                id,
                rect: ControlRect::default(),
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        let permitted = VehicleAvailability {
            can_have_airstrike: true,
            ..VehicleAvailability::default()
        };
        let idle = ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 7000,
            vehicle_availability: permitted,
            click: None,
            vehicle_click: None,
        };
        p.vehicle_tab = true;
        p.update(ControlPanelInput {
            vehicle_click: Some(VehicleAction::Build(VehicleKind::Airstrike)),
            ..idle
        });
        assert_eq!(p.button_state(ControlId::Stop), ButtonVisualState::Enabled);
        assert_eq!(p.resource_points, 0);
        assert_eq!(
            p.update(ControlPanelInput {
                resource_points: 0,
                click: Some(ControlId::Stop),
                ..idle
            }),
            [ControlIntent::CancelVehicle {
                kind: VehicleKind::Airstrike,
                refund: 7000
            }]
        );
        assert_eq!(p.selected_vehicle, None);
        assert_eq!(p.resource_points, 7000);
    }

    #[test]
    fn main_stop_stops_selected_units_before_cancelling_pending_purchase() {
        // `doStop` sends tactical STOP before handling STOP_VEHICLE:
        // controlgui.cpp:1669-1688, then :1697-1701.
        let units = [UnitCapabilities {
            has_queued_orders: true,
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .map(|((kind, id), cost)| VehicleButton {
                kind,
                id,
                rect: ControlRect::default(),
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        p.vehicle_tab = true;
        let build = ControlPanelInput {
            selected_units: &units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 2000,
            vehicle_availability: VehicleAvailability {
                can_have_sensor_probe: true,
                ..VehicleAvailability::default()
            },
            click: None,
            vehicle_click: Some(VehicleAction::Build(VehicleKind::SensorProbe)),
        };
        assert_eq!(
            p.update(build),
            [ControlIntent::BuildVehicle(VehicleKind::SensorProbe)]
        );
        assert_eq!(p.selected_vehicle, Some(VehicleKind::SensorProbe));
        assert_eq!(p.resource_points, 0);
        assert_eq!(
            p.update(ControlPanelInput {
                click: Some(ControlId::Stop),
                vehicle_click: None,
                resource_points: 0,
                ..build
            }),
            [
                ControlIntent::Stop,
                ControlIntent::CancelVehicle {
                    kind: VehicleKind::SensorProbe,
                    refund: 2000,
                },
            ]
        );
        assert_eq!(p.resource_points, 2000);
    }

    #[test]
    fn vehicle_stop_is_rejected_in_flight_and_allowed_before_dispatch() {
        // `handleVehicleClick` rejects STOP_VEHICLE while `paintingMyVtol` is true:
        // controlgui.cpp:1959. The normal cancellation path follows at :1971-2014.
        let no_units = [];
        let mut p = panel();
        p.vehicle_tab = true;
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .map(|((kind, id), cost)| VehicleButton {
                kind,
                id,
                rect: ControlRect::default(),
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        let input = |vtol_in_flight, vehicle_click, resource_points| ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight,
            resource_points,
            vehicle_availability: VehicleAvailability {
                can_have_airstrike: true,
                ..VehicleAvailability::default()
            },
            click: None,
            vehicle_click,
        };
        assert_eq!(
            p.update(input(
                false,
                Some(VehicleAction::Build(VehicleKind::Airstrike)),
                7000
            )),
            [ControlIntent::BuildVehicle(VehicleKind::Airstrike)]
        );
        assert!(p
            .update(input(true, Some(VehicleAction::Stop), 0))
            .is_empty());
        assert_eq!(p.selected_vehicle, Some(VehicleKind::Airstrike));
        assert_eq!(p.resource_points, 0);
        assert_eq!(
            p.update(input(false, Some(VehicleAction::Stop), 0)),
            [ControlIntent::CancelVehicle {
                kind: VehicleKind::Airstrike,
                refund: 7000,
            }]
        );
        assert_eq!(p.selected_vehicle, None);
        assert_eq!(p.resource_points, 7000);
    }

    #[test]
    fn specialty_help_defaults_on_blank_panel_and_clears_outside() {
        // Blank panel uses the specialty fallback at controlgui.cpp:1117; inactive vehicle
        // buttons are not hover targets at :1764-1779; missiongui.cpp:731 clears help outside.
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        let mut p = ControlPanel::load(&mut loader, 1024, 768).unwrap();
        let units = [UnitCapabilities {
            can_lay_mines: true,
            ..UnitCapabilities::default()
        }];
        let input = |mouse| ControlPanelInput {
            selected_units: &units,
            mouse,
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: None,
            vehicle_click: None,
        };
        let inactive_vehicle = p.vehicles[0].rect;
        let vehicle_mouse = (
            (inactive_vehicle.left + inactive_vehicle.right) / 2,
            (inactive_vehicle.top + inactive_vehicle.bottom) / 2,
        );
        p.update(input(vehicle_mouse));
        assert_eq!(p.help_header, 0);
        assert_eq!(p.help_text, 45100); // IDS_MINELAYER_HELP

        let mine_button = p
            .buttons
            .iter()
            .find(|button| button.id == Some(ControlId::LayMines))
            .unwrap();
        let mine_mouse = (
            (mine_button.rect.left + mine_button.rect.right) / 2,
            (mine_button.rect.top + mine_button.rect.bottom) / 2,
        );
        let expected_hover_help = mine_button.help_text;
        p.update(input(mine_mouse));
        assert_eq!(p.help_text, expected_hover_help);

        p.update(input((200, 200)));
        assert_eq!(p.help_header, 0);
        assert_eq!(p.help_text, 0);
    }

    #[test]
    fn toggles_are_disabled_after_selection_changes_and_reject_clicks() {
        // The per-frame selection update disables unsupported controls:
        // controlgui.cpp:1117-1168 and :1169-1172.
        let jumper = [UnitCapabilities {
            can_jump: true,
            ..UnitCapabilities::default()
        }];
        let non_jumper = [UnitCapabilities::default()];
        let no_units = [];
        let mut p = panel();
        let input = |selected_units, click| ControlPanelInput {
            selected_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click,
            vehicle_click: None,
        };
        p.update(input(&jumper, Some(ControlId::Jump)));
        assert!(p.jump);
        p.update(input(&non_jumper, None));
        assert_eq!(p.button_state(ControlId::Jump), ButtonVisualState::Disabled);
        assert!(p
            .update(input(&non_jumper, Some(ControlId::Jump)))
            .is_empty());
        p.update(input(&jumper, Some(ControlId::Run)));
        assert!(p.run);
        p.update(input(&no_units, None));
        for id in [ControlId::Run, ControlId::Guard] {
            assert_eq!(p.button_state(id), ButtonVisualState::Disabled, "{id:?}");
        }
        assert!(p.update(input(&no_units, Some(ControlId::Run))).is_empty());
        assert_eq!(p.button_state(ControlId::Run), ButtonVisualState::Disabled);
    }

    #[test]
    fn specialty_actions_follow_cpp_visibility_priority() {
        // `LAYMINES`, `SALVAGE`, `REPAIR`, then guard tower is an if/else-if chain:
        // controlgui.cpp:1117-1165.
        let combined = [UnitCapabilities {
            can_lay_mines: true,
            can_salvage: true,
            can_repair: true,
            is_guard_tower: true,
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        let update = |click| ControlPanelInput {
            selected_units: &combined,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click,
            vehicle_click: None,
        };
        p.update(update(None));
        assert_eq!(
            p.button_state(ControlId::LayMines),
            ButtonVisualState::Enabled
        );
        for hidden in [ControlId::Salvage, ControlId::Repair, ControlId::GuardTower] {
            assert_eq!(
                p.button_state(hidden),
                ButtonVisualState::Hidden,
                "{hidden:?}"
            );
            assert!(p.update(update(Some(hidden))).is_empty(), "{hidden:?}");
        }
    }

    #[test]
    fn purchase_updates_vehicle_states_and_resource_display_immediately() {
        // `handleVehicleClick` charges cost and disables alternatives while the selection is pending:
        // controlgui.cpp:1971-1980; `updateVehicleTab`: :1760-1780.
        let no_units = [];
        let mut p = panel();
        p.vehicle_tab = true;
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .map(|((kind, id), cost)| VehicleButton {
                kind,
                id,
                rect: ControlRect::default(),
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        p.update(ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 5000,
            vehicle_availability: all_vehicle_availability(),
            click: None,
            vehicle_click: Some(VehicleAction::Build(VehicleKind::SensorProbe)),
        });
        assert_eq!(p.selected_vehicle, Some(VehicleKind::SensorProbe));
        assert_eq!(p.vehicles[2].state, ButtonVisualState::Pressed);
        assert!(p.vehicles[..2]
            .iter()
            .all(|button| button.state == ButtonVisualState::Disabled));
        assert_eq!(p.resource_points, 3000);
    }

    #[test]
    fn ranges_and_disabled_capabilities_match_update_logic() {
        // Selection/range/jump availability: controlgui.cpp:1006-1114, 1169-1172.
        let unit = [UnitCapabilities {
            can_jump: false,
            attack_range: Some(FireRange::Long),
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        p.update(ControlPanelInput {
            selected_units: &unit,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: None,
            vehicle_click: None,
        });
        assert_eq!(
            p.button_state(ControlId::LongRange),
            ButtonVisualState::Hidden
        );
        assert_eq!(p.button_state(ControlId::Jump), ButtonVisualState::Disabled);
        assert_eq!(
            p.button_state(ControlId::LayMines),
            ButtonVisualState::Hidden
        );
        let none = [];
        p.update(ControlPanelInput {
            selected_units: &none,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: None,
            vehicle_click: None,
        });
        assert_eq!(
            p.button_state(ControlId::DefaultRange),
            ButtonVisualState::Hidden
        );
        for id in [
            ControlId::ShortRange,
            ControlId::MediumRange,
            ControlId::LongRange,
            ControlId::Jump,
            ControlId::Run,
            ControlId::Guard,
            ControlId::FireFromCurrentPosition,
        ] {
            let expected = if matches!(
                id,
                ControlId::ShortRange | ControlId::MediumRange | ControlId::LongRange
            ) {
                ButtonVisualState::Hidden
            } else {
                ButtonVisualState::Disabled
            };
            assert_eq!(p.button_state(id), expected, "{id:?}");
        }
    }

    #[test]
    fn vehicle_selection_requires_resource_points_and_emits_build_intent() {
        // Vehicle costs and disabled-state checks: controlgui.cpp:85-95, 1760-1817,
        // and handleVehicleClick charges/starts the selected category at :1971-2012.
        let mut p = panel();
        p.vehicle_tab = true;
        p.vehicles = VEHICLE_KINDS
            .into_iter()
            .zip(VEHICLE_IDS)
            .zip(VEHICLE_COSTS)
            .enumerate()
            .map(|(index, ((kind, id), cost))| VehicleButton {
                kind,
                id,
                rect: ControlRect {
                    left: index as i32 * 10,
                    top: 20,
                    right: index as i32 * 10 + 8,
                    bottom: 28,
                },
                help_header: 0,
                help_text: 0,
                cost,
                art: ButtonArt::default(),
                state: ButtonVisualState::Enabled,
            })
            .collect();
        let no_units = [];
        let permitted = all_vehicle_availability();
        let result = p.update(ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 1999,
            vehicle_availability: permitted,
            click: None,
            vehicle_click: Some(VehicleAction::Build(VehicleKind::SensorProbe)),
        });
        assert!(result.is_empty());
        assert_eq!(p.vehicles[2].state, ButtonVisualState::Disabled);
        assert_eq!(p.vehicles[7].state, ButtonVisualState::Enabled);
        let result = p.update(ControlPanelInput {
            selected_units: &no_units,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 2000,
            vehicle_availability: permitted,
            click: None,
            vehicle_click: Some(VehicleAction::Build(VehicleKind::SensorProbe)),
        });
        assert_eq!(
            result,
            [ControlIntent::BuildVehicle(VehicleKind::SensorProbe)]
        );
        assert!(p
            .update(ControlPanelInput {
                selected_units: &no_units,
                mouse: (-1, -1),
                paused: false,
                vtol_in_flight: false,
                resource_points: 2000,
                vehicle_availability: permitted,
                click: None,
                vehicle_click: Some(VehicleAction::Build(VehicleKind::SensorProbe)),
            })
            .is_empty());
        assert_eq!(
            p.update(ControlPanelInput {
                selected_units: &no_units,
                mouse: (-1, -1),
                paused: false,
                vtol_in_flight: false,
                resource_points: 2000,
                vehicle_availability: VehicleAvailability::default(),
                click: None,
                vehicle_click: Some(VehicleAction::Stop),
            }),
            [ControlIntent::CancelVehicle {
                kind: VehicleKind::SensorProbe,
                refund: 2000
            }]
        );
    }

    #[test]
    fn range_stop_fire_and_vehicle_actions_emit_values() {
        // Click dispatch: controlgui.cpp:1538-1599, 1669-1713, 1947-2014.
        let unit = [UnitCapabilities {
            can_jump: true,
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        let input = |click| ControlPanelInput {
            selected_units: &unit,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 20000,
            vehicle_availability: VehicleAvailability::default(),
            click: Some(click),
            vehicle_click: None,
        };
        assert!(p.update(input(ControlId::ShortRange)).is_empty());
        assert_eq!(
            p.update(input(ControlId::FireFromCurrentPosition)),
            [ControlIntent::SetFireFromCurrentPosition(true)]
        );
        assert_eq!(p.update(input(ControlId::Stop)), [ControlIntent::Stop]);
    }

    #[test]
    fn current_range_button_toggles_to_and_from_optimal() {
        // `toggleHoldPosition`: controlgui.cpp:2931-2940.
        let unit = [UnitCapabilities {
            attack_range: Some(FireRange::Optimal),
            ..UnitCapabilities::default()
        }];
        let mut p = panel();
        let idle = ControlPanelInput {
            selected_units: &unit,
            mouse: (-1, -1),
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: None,
            vehicle_click: None,
        };
        p.update(idle);
        assert_eq!(
            p.update(ControlPanelInput {
                click: Some(ControlId::CurrentRange),
                ..idle
            }),
            [ControlIntent::SetRange(FireRange::Current)]
        );
        let held = [UnitCapabilities {
            attack_range: Some(FireRange::Current),
            ..UnitCapabilities::default()
        }];
        p.update(ControlPanelInput {
            selected_units: &held,
            ..idle
        });
        assert_eq!(
            p.update(ControlPanelInput {
                selected_units: &held,
                click: Some(ControlId::CurrentRange),
                ..idle
            }),
            [ControlIntent::SetRange(FireRange::Optimal)]
        );
    }

    #[test]
    fn cxx_fit_rect_conversion_is_inclusive_and_offsets_each_edge() {
        // `initButtons` adds the resolution offsets before setting x/y and width/height:
        // controlgui.cpp:2277-2302. `initRects` offsets all four stored edges: :2621-2630.
        let mut file = FitIni::from_bytes(
            b"FITini\n[B]\nl XLocation = 20\nl YLocation = 30\nl Width = 40\nl Height = 10\nFITend\n"
                .to_vec(),
        )
        .unwrap();
        file.seek_block("B").unwrap();
        let r = ControlRect::from_fit(&file, "XLocation", "YLocation", "Width", "Height", 3, 5);
        assert_eq!(
            r,
            ControlRect {
                left: 23,
                top: 35,
                right: 63,
                bottom: 45
            }
        );
        assert!(r.contains(63, 45));
    }

    #[test]
    fn wide_screens_move_the_right_group_and_stretch_the_hazard_bar() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        let plain = ControlPanel::load(&mut loader, 1024, 768).unwrap();
        let wide = ControlPanel::load_wide(&mut loader, 1024, 768, 341).unwrap();
        let moved = |a: &ControlRect, b: &ControlRect| (b.left - a.left, b.right - a.right);
        for (p, w) in plain.buttons.iter().zip(&wide.buttons) {
            // the three tabs are the minimap's; every other button is the command panel's
            let dx = if (12..=14).contains(&p.cxx_id) {
                0
            } else {
                341
            };
            assert_eq!(moved(&p.rect, &w.rect), (dx, dx), "button {}", p.cxx_id);
        }
        for (p, w) in plain.vehicles.iter().zip(&wide.vehicles) {
            assert_eq!(
                moved(&p.rect, &w.rect),
                (0, 0),
                "vehicle buttons are the minimap's"
            );
        }
        let stretched: Vec<_> = plain
            .statics
            .iter()
            .zip(&wide.statics)
            .filter(|(p, w)| moved(&p.0, &w.0) == (0, 341))
            .collect();
        assert_eq!(stretched.len(), 1, "one static stretches: the hazard bar");
        assert!(wide
            .statics
            .iter()
            .zip(&plain.statics)
            .any(|(w, p)| moved(&p.0, &w.0) == (341, 341)));
        // the map's hit area stays the first rect, and no black fill is added: the unit bar backdrop is the
        // layout's own grid art, tiled (see `the_wide_unit_bar_backdrop_is_covered_by_grid_art`)
        assert_eq!(wide.rects.len(), plain.rects.len());
        // 4:3 is untouched
        assert_eq!(
            plain.rects.len(),
            ControlPanel::load_wide(&mut loader, 1024, 768, 0)
                .unwrap()
                .rects
                .len()
        );
    }

    /// RC1 of the VQA triage: on 16:9 the unit bar's slot grid ended after 8 slots and a flat black rectangle
    /// filled up to the command panel. The old test only checked that a black rect of the right width existed.
    /// What the player sees: every pixel of the grid's row between the minimap and the command panel is textured
    /// panel art, with no solid fill drawn over the gap.
    #[test]
    fn the_wide_unit_bar_backdrop_is_covered_by_grid_art() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        for (w, extra) in [(1024, 341), (1280, 796), (1920, 896), (1920, 1109)] {
            let plain = ControlPanel::load(&mut loader, w, 768.max(w * 3 / 4)).unwrap();
            let wide = ControlPanel::load_wide(&mut loader, w, 768.max(w * 3 / 4), extra).unwrap();
            let is_grid =
                |r: &ControlRect| r.bottom - r.top == 149 && r.left >= plain.rects[0].0.right;
            let plain_grid: Vec<_> = plain.statics.iter().filter(|s| is_grid(&s.0)).collect();
            assert!(plain_grid.len() >= 4);
            let grid_right = plain_grid.iter().map(|s| s.0.right).max().unwrap();
            let grid_left = plain_grid.iter().map(|s| s.0.left).min().unwrap();
            let x_shift = wide.statics[0].0.left - plain.statics[0].0.left;
            let (from, to) = (grid_left + x_shift, grid_right + x_shift + extra);
            // no solid fill over the gap
            assert!(
                wide.rects
                    .iter()
                    .all(|(r, _)| r.right <= from || r.left >= to),
                "{w}: a solid rect covers the unit bar backdrop"
            );
            // the grid row is covered edge to edge by textured statics
            let mut row: Vec<_> = wide
                .statics
                .iter()
                .filter(|(r, a)| {
                    a.texture.is_some() && r.bottom - r.top == 149 && r.left >= from && r.left < to
                })
                .map(|(r, _)| (r.left, r.right))
                .collect();
            row.sort_unstable();
            let mut x = from;
            for (l, r) in row {
                assert_eq!(l, x, "{w}+{extra}: gap in the backdrop at x={x}");
                x = r;
            }
            assert_eq!(x, to, "{w}+{extra}: backdrop ends at {x}, not {to}");
        }
    }

    #[test]
    fn the_1920_layout_anchors_the_command_panel_to_the_wide_screens_right_edge() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        // the block is the 1024-wide control geometry whatever the layout's xOffset (288): at 1920x1200 the
        // screen is 1920 wide; 2560x1440 and 3840x2160 (both 16:9) show 2133 layout columns
        for (visible, extra) in [(1920, 896), (2133, 1109), (2133, 1109)] {
            let plain = ControlPanel::load(&mut loader, 1920, 1200).unwrap();
            let wide = ControlPanel::load_wide(&mut loader, 1920, 1200, extra).unwrap();
            let right = wide.buttons.iter().map(|b| b.rect.right).max().unwrap();
            assert!(
                right <= visible && visible - right <= 40,
                "{visible}: the command panel ends at {right}"
            );
            // the minimap side hugs the left edge: the xOffset is dropped
            let map = wide.rects[0].0;
            assert_eq!(map.left, plain.rects[0].0.left - 288);
            assert!(map.left < 20, "{map:?}");
            let pause = crate::pause_window::PauseWindow::load_wide(&mut loader, 1920, 1200, extra)
                .unwrap();
            assert!(pause.panel_rect().right <= visible + 8);
        }
    }

    #[test]
    fn the_unit_bar_backdrop_statics_are_drawn_and_the_compass_and_cover_icon_are_not() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        let panel = ControlPanel::load(&mut loader, 1024, 768).unwrap();
        // Static10..13 are the 2x8 slot grid (four blocks, side by side, below the hazard bar)
        let grid: Vec<_> = panel
            .statics
            .iter()
            .filter(|(r, _)| r.bottom - r.top == 149 && r.right - r.left == 160)
            .collect();
        assert_eq!(grid.len(), 4, "{:?}", panel.statics);
        assert!(grid.windows(2).all(|w| w[0].0.right == w[1].0.left));
    }

    #[test]
    fn real_1024_layout_loads_control_and_vehicle_rects() {
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        let panel = ControlPanel::load(&mut loader, 1024, 768).unwrap();
        assert_eq!(panel.buttons.len(), 20);
        assert_eq!(panel.vehicles.len(), 8);
        assert_eq!(panel.statics.len(), 25);
        assert_eq!(
            panel
                .vehicles
                .iter()
                .map(|button| button.id)
                .collect::<Vec<_>>(),
            [100, 101, 102, 103, 104, 105, 106, 107]
        );
        assert_eq!(
            panel
                .vehicles
                .iter()
                .map(|button| button.cost)
                .collect::<Vec<_>>(),
            [7000, 8000, 2000, 6000, 2000, 4000, 10000, 0]
        );
        // Hand-read from buttonlayout1024.fit, then translated by initButtons/initRects:
        // controlgui.cpp:2277-2302 and :2621-2630.
        assert_eq!(
            panel.buttons.iter().find(|b| b.cxx_id == 0).unwrap().rect,
            ControlRect {
                left: 884,
                top: 595,
                right: 948,
                bottom: 635
            }
        );
        assert_eq!(
            panel.vehicles.iter().find(|b| b.id == 100).unwrap().rect,
            ControlRect {
                left: 9,
                top: 561,
                right: 49,
                bottom: 609
            }
        );
        assert_eq!(
            panel.rects[0].0,
            ControlRect {
                left: 5,
                top: 557,
                right: 211,
                bottom: 763
            }
        );
        let current_range = panel
            .buttons
            .iter()
            .find(|button| button.id == Some(ControlId::CurrentRange))
            .unwrap();
        let expected_help = current_range.help_text;
        let mouse = (
            (current_range.rect.left + current_range.rect.right) / 2,
            (current_range.rect.top + current_range.rect.bottom) / 2,
        );
        let selected = [UnitCapabilities {
            can_jump: true,
            ..UnitCapabilities::default()
        }];
        let mut panel = panel;
        panel.update(ControlPanelInput {
            selected_units: &selected,
            mouse,
            paused: false,
            vtol_in_flight: false,
            resource_points: 0,
            vehicle_availability: VehicleAvailability::default(),
            click: None,
            vehicle_click: None,
        });
        // The current-range button remains active after the four defunct range buttons hide:
        // controlgui.cpp:1235-1238.
        assert_eq!(panel.help_text, expected_help);
        let short = panel
            .buttons
            .iter()
            .find(|button| button.id == Some(ControlId::ShortRange))
            .unwrap();
        let short_mouse = (
            (short.rect.left + short.rect.right) / 2,
            (short.rect.top + short.rect.bottom) / 2,
        );
        assert!(panel
            .update(ControlPanelInput {
                selected_units: &selected,
                mouse: short_mouse,
                paused: false,
                vtol_in_flight: false,
                resource_points: 0,
                vehicle_availability: VehicleAvailability::default(),
                click: Some(ControlId::ShortRange),
                vehicle_click: None,
            })
            .is_empty());
        assert_eq!(
            panel.button_state(ControlId::ShortRange),
            ButtonVisualState::Hidden
        );
    }

    #[test]
    fn high_resolution_layout_offsets_match_shipped_fit_hiresoffsets() {
        // FIT HiresOffsets are applied to buttons, statics and rects by swapResolutions/initButtons/initStatics/initRects:
        // controlgui.cpp:2277-2302, 2374-2444, 2621-2630, 2640-2679.
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let expected = [(1280, 128), (1600, 288), (1920, 288)];
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).unwrap();
        let mut assets = crate::UiAssets::default();
        let mut loader = Loader::new(&mut vfs, &mut assets);
        for (width, expected_x_offset) in expected {
            let mut panel = ControlPanel::load(&mut loader, width, 768).unwrap();
            let name = format!("buttonlayout{width}");
            let mut fit = loader.art_fit(&name).unwrap();
            fit.seek_block("HiresOffsets").unwrap();
            let x_offset = fit.long_or("xOffset", 0) as i32;
            assert_eq!(x_offset, expected_x_offset, "{name}.fit HiresOffsets");
            fit.seek_block("Button0").unwrap();
            let raw_x = fit.long_or("XLocation", 0) as i32;
            let raw_y = fit.long_or("YLocation", 0) as i32;
            assert_eq!(
                panel
                    .buttons
                    .iter()
                    .find(|button| button.cxx_id == 0)
                    .unwrap()
                    .rect
                    .left,
                raw_x + expected_x_offset
            );
            assert_eq!(
                panel
                    .buttons
                    .iter()
                    .find(|button| button.cxx_id == 0)
                    .unwrap()
                    .rect
                    .top,
                raw_y
            );
            fit.seek_block("Static0").unwrap();
            let static_x = fit.long_or("XLocation", 0) as i32;
            assert_eq!(panel.statics[0].0.left, static_x + expected_x_offset);
            fit.seek_block("Rect0").unwrap();
            let rect_left = fit.long_or("left", 0) as i32;
            assert_eq!(panel.rects[0].0.left, rect_left + expected_x_offset);
            panel.update(ControlPanelInput {
                selected_units: &[],
                mouse: (-1, -1),
                paused: false,
                vtol_in_flight: false,
                resource_points: 0,
                vehicle_availability: VehicleAvailability::default(),
                click: None,
                vehicle_click: None,
            });
            assert_eq!(
                panel.button_state(ControlId::DefaultRange),
                ButtonVisualState::Hidden
            );
        }
    }
}
