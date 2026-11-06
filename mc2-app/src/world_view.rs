// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! World-view selection brackets, health bars and the context-sensitive order cursor.
//!
//! Brackets and bars follow `Appearance::drawSelectBrackets` / `drawBars` (`mclib/appear.cpp:249-556`)
//! and are drawn in the scene pass over the projected selection box. Cursor choice is
//! [`mc2_ui::mission_keys::choose_cursor`] (`code/missiongui.cpp:3428-3458`, `3700-4130`), filled
//! from the HUD's selection, the object under the mouse and the control-panel modes.

use crate::hud_data::{HudUnit, PickTarget};
use crate::overlay::OverlayBatch;
use mc2_render::camera::GameCamera;
use mc2_render::math::Vec3;
use mc2_ui::control::ControlPanel;
use mc2_ui::mission_hud::HitTarget;
use mc2_ui::mission_keys::{
    self, choose_cursor, Controls, CursorChoice, CursorContext, FireRange, Relation, SelectedMover,
    Target,
};
use mc2_ui::tacmap::TacTeam;

/// `SB_GREEN | 0xff000000` (`mclib/dbasegui.h:87`, `mech3d.cpp:2357`): a selected home unit.
pub const BRACKET_FRIENDLY: u32 = 0xff00_ff00;
/// `SB_GREEN` (`dbasegui.h:87`): health fill at ≥ 50%.
const SB_GREEN: u32 = 0x6f00_ff00;
/// `SB_YELLOW`.
const SB_YELLOW: u32 = 0x6fff_ff00;
/// `SB_RED`.
const SB_RED: u32 = 0x6fff_0000;
const BLACK: u32 = 0xff00_0000;
/// `appear.h:38-39`: bar half-width and height, before `getScaleFactor`.
const BAR_HALF_WIDTH: f32 = 19.0;
const BAR_HEIGHT: f32 = 4.0;
/// `appear.cpp:251`: corner length of `drawSelectBrackets`.
const BRACKET_OFFSET: f32 = 5.0;
/// `appear.cpp:474`: gap between the box and the health bar.
const BAR_OFFSET: f32 = 8.0;
/// `Stuff::SMALL`.
const SMALL: f32 = 1.0e-6;
/// `missiongui.cpp:5543`.
const SLOPE_TEST: [f32; 8] = [
    0.098_491_4,
    0.303_346_7,
    0.534_511_15,
    0.820_678_8,
    1.218_503_5,
    1.870_868_4,
    3.296_558_1,
    10.153_171,
];

/// One selected (or targeted) object the scene pass draws brackets and a health bar for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectedMark {
    /// `[left, top, right, bottom]` in game pixels (`upperLeft`/`lowerRight`).
    pub box_px: [f32; 4],
    /// `barStatus` in 0..1 (`getTotalEffectiveness`).
    pub health: f32,
    /// Bracket colour (`colorTable[homeTeamRelationship]`, opaque).
    pub color: u32,
    /// `Appearance::barColor` (`mech.cpp:6491`, `gvehicl.cpp:3983`); 0 uses damage colours.
    pub bar_color: u32,
    /// `attackRange == FIRERANGE_CURRENT` (`mech.cpp:6512`, `gvehicl.cpp:4002`).
    pub hold_fire: bool,
}

/// `Appearance::drawSelectBrackets` (`appear.cpp:249-376`): eight 1-pixel corner ticks.
pub fn draw_select_brackets(
    batch: &mut OverlayBatch,
    box_px: [f32; 4],
    color: [f32; 4],
    scale_factor: f32,
    thickness: f32,
) {
    let [ul_x, ul_y, br_x, br_y] = box_px;
    let o = BRACKET_OFFSET * scale_factor;
    let t = thickness;
    batch.hline(ul_x, ul_x + o, ul_y, t, color);
    batch.vline(ul_x, ul_y, ul_y + o, t, color);
    batch.vline(br_x, ul_y, ul_y + o, t, color);
    batch.hline(br_x, br_x - o, ul_y, t, color);
    batch.vline(ul_x, br_y, br_y - o, t, color);
    batch.hline(ul_x, ul_x + o, br_y, t, color);
    batch.vline(br_x, br_y, br_y - o, t, color);
    batch.hline(br_x, br_x - o, br_y, t, color);
}

/// `Appearance::drawBars` (`appear.cpp:470-556`): the health fill and its black frame.
/// `bar_color` is `Appearance::barColor`; 0 uses the damage-threshold colours (`:485-497`).
pub fn draw_bars(
    batch: &mut OverlayBatch,
    box_px: [f32; 4],
    health: f32,
    bar_color: u32,
    scale_factor: f32,
    thickness: f32,
) {
    let [ul_x, ul_y, br_x, _] = box_px;
    let offset = BAR_OFFSET * scale_factor;
    let true_width = BAR_HALF_WIDTH * scale_factor * 2.0;
    let true_height = BAR_HEIGHT * scale_factor;
    let top_y = ul_y - offset - true_height;
    let left_x = ((ul_x + br_x) / 2.0 - true_width / 2.0).floor();
    let health = health.clamp(0.0, 1.0);
    let fill = if bar_color != 0 {
        bar_color
    } else if health >= 0.5 {
        SB_GREEN
    } else if health > 0.2 {
        SB_YELLOW
    } else if health > 0.0 {
        SB_RED
    } else {
        0
    };
    let bar_length = true_width * health;
    let x0 = left_x - 1.0;
    let y0 = top_y - 1.0;
    let y1 = top_y + true_height + 1.0;
    if fill != 0 {
        let c = argb(fill | 0xff00_0000);
        batch.rect(x0, y0, bar_length + 2.0, y1 - y0, c);
    }
    let frame = argb(BLACK);
    let t = thickness;
    let right = left_x + true_width + 1.0;
    batch.hline(x0, right, y0, t, frame);
    batch.vline(right, y0, y1, t, frame);
    batch.hline(x0, right, y1, t, frame);
    batch.vline(x0, y0, y1, t, frame);
}

/// The 5×5 region `drawIcon` samples from `blip.tga` (`appear.cpp:452-458`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldFireIcon {
    /// Always 5 (`bmpWidth` in `mech.cpp:6513`).
    pub width: u32,
    /// Always 5 (`bmpHeight`).
    pub height: u32,
    /// RGBA8, `width * height` texels, top-left origin.
    pub rgba: Vec<u8>,
}

/// UVs `drawIcon` hard-codes (`appear.cpp:452-458`): `u` 0.25..9/16, `v` 0..5/16.
const HOLD_FIRE_U0: f32 = 0.25;
const HOLD_FIRE_U1: f32 = 9.0 / 16.0;
const HOLD_FIRE_V0: f32 = 0.0;
const HOLD_FIRE_V1: f32 = 5.0 / 16.0;
const HOLD_FIRE_BMP: u32 = 5;

/// Samples the shipped `blip.tga` the way `drawIcon` does.
pub fn hold_fire_icon_from_rgba(width: u32, height: u32, rgba: &[u8]) -> Option<HoldFireIcon> {
    if width == 0 || height == 0 {
        return None;
    }
    if rgba.len() < (width * height * 4) as usize {
        return None;
    }
    let mut out = Vec::with_capacity((HOLD_FIRE_BMP * HOLD_FIRE_BMP * 4) as usize);
    for y in 0..HOLD_FIRE_BMP {
        for x in 0..HOLD_FIRE_BMP {
            let u = HOLD_FIRE_U0
                + (x as f32 + 0.5) / HOLD_FIRE_BMP as f32 * (HOLD_FIRE_U1 - HOLD_FIRE_U0);
            let v = HOLD_FIRE_V0
                + (y as f32 + 0.5) / HOLD_FIRE_BMP as f32 * (HOLD_FIRE_V1 - HOLD_FIRE_V0);
            let px = ((u * width as f32).floor() as u32).min(width - 1);
            let py = ((v * height as f32).floor() as u32).min(height - 1);
            let i = ((py * width + px) * 4) as usize;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    Some(HoldFireIcon {
        width: HOLD_FIRE_BMP,
        height: HOLD_FIRE_BMP,
        rgba: out,
    })
}

/// Top-left of the hold-fire icon in the same pixel space as `box_px` (`appear.cpp:437-440`).
pub fn hold_fire_icon_origin(box_px: [f32; 4], scale_factor: f32) -> (f32, f32) {
    let [ul_x, ul_y, br_x, _] = box_px;
    let offset = BAR_OFFSET * scale_factor;
    let true_height = BAR_HEIGHT * scale_factor;
    let bmp_w = HOLD_FIRE_BMP as f32;
    let bmp_h = HOLD_FIRE_BMP as f32;
    let y = ul_y - offset - true_height - 2.0 * bmp_h;
    let x = (ul_x + br_x) / 2.0 - bmp_w / 2.0;
    (x, y)
}

/// `Appearance::drawIcon` (`appear.cpp:432-467`) for the hold-fire blip (`mech.cpp:6512-6513`).
/// Vertex colour is `barColor | 0xff000000`; the texture's RGB/A modulate it.
pub fn draw_hold_fire_icon(
    batch: &mut OverlayBatch,
    box_px: [f32; 4],
    color: u32,
    scale_factor: f32,
    icon: &HoldFireIcon,
) {
    let (x0, y0) = hold_fire_icon_origin(box_px, scale_factor);
    let tint = argb(color | 0xff00_0000);
    for y in 0..icon.height {
        for x in 0..icon.width {
            let i = ((y * icon.width + x) * 4) as usize;
            let Some(px) = icon.rgba.get(i..i + 4) else {
                continue;
            };
            if px[3] == 0 {
                continue;
            }
            let c = [
                tint[0] * f32::from(px[0]) / 255.0,
                tint[1] * f32::from(px[1]) / 255.0,
                tint[2] * f32::from(px[2]) / 255.0,
                tint[3] * f32::from(px[3]) / 255.0,
            ];
            batch.rect(x0 + x as f32, y0 + y as f32, 1.0, 1.0, c);
        }
    }
}

/// Draws every selected mark in game pixels; `native_scale` maps to the target's pixels.
pub fn draw_marks(
    batch: &mut OverlayBatch,
    marks: &[SelectedMark],
    scale_factor: f32,
    native_scale: f32,
    hold_fire: Option<&HoldFireIcon>,
) {
    let s = native_scale.max(0.5);
    let t = s;
    for m in marks {
        let box_px = m.box_px.map(|v| v * s);
        // `mech3d.cpp:2380-2387`: bars, then brackets; the hold-fire icon is after `render`.
        draw_bars(batch, box_px, m.health, m.bar_color, scale_factor * s, t);
        draw_select_brackets(batch, box_px, argb(m.color), scale_factor * s, t);
        if m.hold_fire {
            if let Some(icon) = hold_fire {
                draw_hold_fire_icon(batch, box_px, m.bar_color, scale_factor * s, icon);
            }
        }
    }
}

/// `0xAARRGGBB` as a display-space overlay colour.
pub fn argb(c: u32) -> [f32; 4] {
    [
        ((c >> 16) & 0xff) as f32 / 255.0,
        ((c >> 8) & 0xff) as f32 / 255.0,
        (c & 0xff) as f32 / 255.0,
        ((c >> 24) & 0xff) as f32 / 255.0,
    ]
}

/// `MissionInterfaceManager::calcRotation` (`missiongui.cpp:5578-5683`).
///
/// Returns a 0..31 cursor-rotation index. With no selected movers, or before turn 4, the C++
/// returns `mState_NORMAL` (21) and that value is used as the rotation.
pub fn calc_rotation(
    selected: &[&HudUnit],
    mouse_world: Option<Vec3>,
    camera: &GameCamera,
    turn: u32,
) -> u32 {
    let mut pos = Vec3::ZERO;
    let mut count = 0.0_f32;
    for u in selected {
        pos.x += u.position.0;
        pos.y += u.position.1;
        pos.z += u.position.2;
        count += 1.0;
    }
    if count == 0.0 || turn <= 3 {
        return mission_keys::state::NORMAL;
    }
    pos.x /= count;
    pos.y /= count;
    pos.z /= count;
    let Some(w_pos) = mouse_world else {
        return mission_keys::state::NORMAL;
    };
    let mut actual = Vec3::new(pos.x - w_pos.x, pos.y - w_pos.y, 0.0);
    let len = (actual.x * actual.x + actual.y * actual.y).sqrt();
    if len > SMALL {
        actual.x /= len;
        actual.y /= len;
    }
    actual.x *= 200.0;
    actual.y *= 200.0;
    let cam = camera.position_world();
    let mover = Vec3::new(cam.x + actual.x, cam.y + actual.y, cam.z + actual.z);
    let (screen_mover, _) = camera.project_z(mover);
    let (screen_goal, _) = camera.project_z(cam);
    let dx = (screen_goal[0] - screen_mover[0]).abs();
    let slope = if dx > SMALL {
        (screen_goal[1] - screen_mover[1]).abs() / dx
    } else {
        0.0
    };
    let mut counter = 0;
    while counter < 8 && slope > SLOPE_TEST[counter] {
        counter += 1;
    }
    let rotation = if screen_goal[1] < screen_mover[1] {
        if screen_goal[0] < screen_mover[0] {
            counter
        } else {
            8 + (8 - counter)
        }
    } else if screen_goal[0] > screen_mover[0] {
        16 + counter
    } else {
        let r = 24 + (8 - counter);
        if r == 32 {
            0
        } else {
            r
        }
    };
    rotation as u32
}

/// `ControlGui::getCurrentRange` (`controlgui.cpp:1420-1439`).
pub fn current_range(selected: &[&HudUnit]) -> FireRange {
    current_range_of(selected.iter().map(|u| u.attack_range))
}

/// `getCurrentRange` over the selected movers' `attackRange` values.
pub fn current_range_of(ranges: impl IntoIterator<Item = i32>) -> FireRange {
    let mut range: Option<i32> = None;
    for r in ranges {
        match range {
            None => range = Some(r),
            Some(prev) if prev != r => return FireRange::Optimal,
            Some(_) => {}
        }
    }
    match range.unwrap_or(-1) {
        0 => FireRange::Short,
        1 => FireRange::Medium,
        2 => FireRange::Long,
        3 => FireRange::Current,
        _ => FireRange::Optimal,
    }
}

/// The facts `choose_cursor` needs for the object under the mouse.
pub fn cursor_target(p: &PickTarget, passable: bool) -> Target {
    Target {
        class: p.class,
        relation: match p.team {
            TacTeam::Ours | TacTeam::PlayerAlt => Relation::Friendly,
            TacTeam::Neutral => Relation::Neutral,
            TacTeam::Enemy => Relation::Enemy,
        },
        disabled: p.disabled,
        is_mover: p.mover,
        captureable: p.captureable,
        can_salvage: false,
        can_repair: false,
        refit_bay: false,
        can_repair_bay: false,
        landbridge: p.landbridge,
        landbridge_passable: p.landbridge && passable,
    }
}

/// `MissionInterfaceManager::canJumpToWPos` (`missiongui.cpp:3500-3551`): passable landing
/// and every selected home unit can jump to `mouse_world` from its last queued waypoint when
/// one exists, else from its current position (`peekQueuedTacOrder(-1)`, `:3524-3538`).
pub fn can_jump_to_wpos(selected: &[&HudUnit], mouse_world: Option<Vec3>, passable: bool) -> bool {
    if !passable {
        return false;
    }
    let Some(wpos) = mouse_world else {
        return false;
    };
    if selected.is_empty() {
        return false;
    }
    selected.iter().all(|u| {
        if !u.bar.can_jump {
            return false;
        }
        let dx = u.last_jump_from.0 - wpos.x;
        let dy = u.last_jump_from.1 - wpos.y;
        (dx * dx + dy * dy).sqrt() <= u.jump_range_wu
    })
}

/// `Appearance::barColor` for a visible home-commander unit (`mech.cpp:6491-6498`,
/// `gvehicl.cpp:3983-3990`).
pub fn unit_bar_color(unit: &HudUnit) -> u32 {
    if unit.info.is_mech {
        0x0000_ff00
    } else {
        0xff7f_7f7f
    }
}

/// One selected mover as `updateTarget`'s capture branch reads it (missiongui.cpp:3792-3816).
fn selected_mover(part_id: i32, is_mech: bool, target: Option<&PickTarget>) -> SelectedMover {
    SelectedMover {
        mech_on_ground: is_mech,
        // `getCaptureBlocker(mover)` (missiongui.cpp:3802); no sim fact reads as blocked, as
        // `doAttack` refuses it.
        capture_blocked: target.is_none_or(|p| p.capture_blockers.get(&part_id) != Some(&false)),
    }
}

/// Fills [`CursorContext`] from the HUD and returns [`choose_cursor`].
#[allow(clippy::too_many_arguments)]
pub fn order_cursor(
    selected: &[&HudUnit],
    target: Option<&PickTarget>,
    control: &ControlPanel,
    camera: &GameCamera,
    mouse_world: Option<Vec3>,
    hit: HitTarget,
    waypoint_queue: bool,
    energy_weapons: bool,
    force_fire_held: bool,
    turn: u32,
    camera_rotate: bool,
    line_of_sight: bool,
    passable: bool,
) -> CursorChoice {
    if camera_rotate {
        return CursorChoice {
            cursor: mission_keys::state::ROTATE_CAMERA,
            targeted: false,
            target_cleared: false,
        };
    }
    let adding = control.selected_vehicle;
    let adding_vehicle = adding.is_some_and(|k| {
        !matches!(
            k,
            mc2_ui::control::VehicleKind::Airstrike | mc2_ui::control::VehicleKind::SensorProbe
        )
    });
    let adding_airstrike = adding.is_some_and(|k| {
        matches!(
            k,
            mc2_ui::control::VehicleKind::Airstrike | mc2_ui::control::VehicleKind::SensorProbe
        )
    });
    let gui = hit != HitTarget::World;
    // `updateTarget` (`missiongui.cpp:1218`) clears the world target under the GUI;
    // `updateOldStyle` (`:1296-1306`) then keeps `mState_NORMAL` unless adding a vehicle
    // or airstrike.
    if gui && !adding_airstrike && !adding_vehicle {
        return CursorChoice {
            cursor: mission_keys::state::NORMAL,
            targeted: false,
            target_cleared: true,
        };
    }
    let movers = selected.len() as u32;
    let rotation = calc_rotation(selected, mouse_world, camera, turn);
    let ctx = CursorContext {
        line_of_sight,
        waypoint_key_down: waypoint_queue,
        ctrl_down: waypoint_queue,
        gui,
        passable,
        movers,
        non_movers: 0,
        rotation,
        force_fire_held,
        painting_vtol: false,
        selection_helicopters: false,
        energy_weapons,
        range: current_range(selected),
        fire_from_current_pos: control.fire_from_current_position,
        controls: Controls {
            info_select: false,
            adding_vehicle,
            adding_salvage: adding == Some(mc2_ui::control::VehicleKind::RecoveryTeam),
            adding_airstrike,
            sensor_probe_pressed: adding == Some(mc2_ui::control::VehicleKind::SensorProbe),
            sensor_probe_held: adding == Some(mc2_ui::control::VehicleKind::SensorProbe),
            over_tac_map: hit == HitTarget::TacMap,
            salvage: control.salvage,
            repair: control.repair,
            mines: control.lay_mines,
            guard_tower: control.guard_tower_suppression,
            guard: control.guard,
            jump: control.jump,
            run: control.run,
        },
        can_add_vehicle: false,
        can_recover: false,
        can_jump_to_wpos: can_jump_to_wpos(selected, mouse_world, passable),
        selected: selected
            .iter()
            .map(|u| selected_mover(u.part_id, u.info.is_mech, target))
            .collect(),
        target: if gui {
            None
        } else {
            target.map(|p| cursor_target(p, passable))
        },
    };
    choose_cursor(&ctx)
}

/// A mark for one selected home unit whose projected box is known.
pub fn mark_for(unit: &HudUnit, box_px: [f32; 4]) -> SelectedMark {
    SelectedMark {
        box_px,
        health: unit.bar.bar_status,
        color: BRACKET_FRIENDLY,
        bar_color: unit_bar_color(unit),
        hold_fire: unit.attack_range == mc2_sim::command::FIRERANGE_CURRENT && !unit.bar.destroyed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc2_ui::mission_keys::{state, ObjectClass};

    #[test]
    fn brackets_are_eight_corner_ticks_outside_the_box() {
        let mut b = OverlayBatch::default();
        draw_select_brackets(
            &mut b,
            [100.0, 50.0, 200.0, 150.0],
            argb(BRACKET_FRIENDLY),
            1.0,
            1.0,
        );
        assert_eq!(b.len(), 16, "eight lines, two tris each");
    }

    #[test]
    fn a_full_health_bar_is_green_and_framed() {
        let mut b = OverlayBatch::default();
        draw_bars(&mut b, [100.0, 50.0, 200.0, 150.0], 1.0, 0, 1.0, 1.0);
        assert!(b.len() >= 10);
    }

    #[test]
    fn a_damaged_home_mech_bar_keeps_the_supplied_unit_color() {
        let mut b = OverlayBatch::default();
        draw_bars(
            &mut b,
            [100.0, 50.0, 200.0, 150.0],
            0.3,
            0x0000_ff00,
            1.0,
            1.0,
        );
        let green = argb(0xff00_ff00);
        assert!(
            b.colors().iter().any(|c| {
                (c[0] - green[0]).abs() < 1e-5
                    && (c[1] - green[1]).abs() < 1e-5
                    && (c[2] - green[2]).abs() < 1e-5
            }),
            "barColor 0x0000ff00 is green at any health (`appear.cpp:496-497`)"
        );
        let yellow = argb(SB_YELLOW | 0xff00_0000);
        assert!(
            b.colors()
                .iter()
                .all(|c| { (c[0] - yellow[0]).abs() > 1e-5 || (c[1] - yellow[1]).abs() > 1e-5 }),
            "a home mech does not use the damage-threshold yellow"
        );
    }

    #[test]
    fn hold_fire_icon_sits_above_the_bar() {
        let icon = HoldFireIcon {
            width: 5,
            height: 5,
            rgba: vec![255; 5 * 5 * 4],
        };
        let mut b = OverlayBatch::default();
        draw_hold_fire_icon(&mut b, [100.0, 50.0, 200.0, 150.0], 0x0000_ff00, 1.0, &icon);
        assert_eq!(b.len(), 50, "5x5 texels, two tris each");
        let (x, y) = hold_fire_icon_origin([100.0, 50.0, 200.0, 150.0], 1.0);
        assert!((x - 147.5).abs() < 1e-5 && (y - 28.0).abs() < 1e-5);
    }

    #[test]
    fn jump_range_is_measured_from_the_last_queued_waypoint() {
        let unit = |from: (f32, f32), pos: (f32, f32), range: f32| HudUnit {
            part_id: 1,
            pilot_slot: 0,
            position: (pos.0, pos.1, 0.0),
            bar: mc2_ui::unitbar::UnitBarUnit {
                id: 1,
                icon_picture: 0,
                tonnage: 50,
                pilot: mc2_ui::unitbar::Pilot {
                    name: String::new(),
                    photo: 0,
                    message_playing: false,
                },
                armor: mc2_ui::unitbar::Armor::Mech(mc2_ui::unitbar::MechArmor::default()),
                bar_status: 1.0,
                selected: true,
                groups: 0,
                shutdown: false,
                destroyed: false,
                recover_pending: false,
                power_used_up: false,
                can_jump: true,
            },
            pilot_name: crate::hud_data::HudText::Literal(String::new()),
            name: crate::hud_data::HudText::Literal(String::new()),
            info: mc2_ui::hud_info::info::UnitInfo {
                id: 1,
                name: String::new(),
                is_mech: true,
                team_is_home: true,
                contact_visual: true,
                has_team: true,
                team_not_enemy: true,
                disabled: false,
                destroyed: false,
                health: 1.0,
                weapons: Vec::new(),
                equipment: [None; 4],
                pilot: mc2_ui::hud_info::info::PilotInfo {
                    active: true,
                    name: String::new(),
                    rank: 0,
                    skills: [0; 3],
                    specialties: Vec::new(),
                },
            },
            caps: Default::default(),
            sensor_range_m: 0.0,
            attack_range: -1,
            has_non_area_weapon: false,
            jump_range_wu: range,
            screen_box: None,
            in_view: true,
            needs_refit: false,
            last_jump_from: from,
        };
        let current = (0.0, 0.0);
        let waypoint = (200.0, 0.0);
        let landing_from_waypoint = Vec3::new(250.0, 0.0, 0.0);
        let landing_from_current = Vec3::new(50.0, 0.0, 0.0);
        let queued = unit(waypoint, current, 100.0);
        assert!(
            can_jump_to_wpos(&[&queued], Some(landing_from_waypoint), true),
            "a landing in range of the queued waypoint is valid"
        );
        assert!(
            !can_jump_to_wpos(&[&queued], Some(landing_from_current), true),
            "a landing in range of the current position only is DONT once a waypoint is queued"
        );
        let unqueued = unit(current, current, 100.0);
        assert!(can_jump_to_wpos(
            &[&unqueued],
            Some(landing_from_current),
            true
        ));
        assert!(!can_jump_to_wpos(
            &[&unqueued],
            Some(landing_from_waypoint),
            true
        ));
    }

    #[test]
    fn hold_fire_icon_samples_the_drawicon_uv_region() {
        // 16×16, unique colour at the C++ UV texel (4,0): 0.25*16 = 4, v=0.
        let mut rgba = vec![0u8; 16 * 16 * 4];
        let mut mark = |x: u32, y: u32, c: [u8; 4]| {
            let i = ((y * 16 + x) * 4) as usize;
            rgba[i..i + 4].copy_from_slice(&c);
        };
        mark(4, 0, [10, 20, 30, 40]);
        mark(8, 4, [50, 60, 70, 80]);
        let icon = hold_fire_icon_from_rgba(16, 16, &rgba).unwrap();
        assert_eq!(icon.width, 5);
        assert_eq!(&icon.rgba[0..4], &[10, 20, 30, 40]);
        let last = ((5 * 5 - 1) * 4) as usize;
        assert_eq!(&icon.rgba[last..last + 4], &[50, 60, 70, 80]);
    }

    #[test]
    fn jump_is_invalid_without_a_passable_in_range_landing() {
        assert!(!can_jump_to_wpos(&[], Some(Vec3::new(0.0, 0.0, 0.0)), true));
        assert!(!can_jump_to_wpos(
            &[],
            Some(Vec3::new(0.0, 0.0, 0.0)),
            false
        ));
    }

    #[test]
    fn current_range_is_optimal_when_the_selection_disagrees() {
        assert_eq!(current_range_of([0]), FireRange::Short);
        assert_eq!(current_range_of([0, 1]), FireRange::Optimal);
        assert_eq!(current_range_of(std::iter::empty()), FireRange::Optimal);
        assert_eq!(current_range_of([-1]), FireRange::Optimal);
        assert_eq!(current_range_of([3]), FireRange::Current);
    }

    #[test]
    fn opaque_green_is_full_alpha() {
        let c = argb(BRACKET_FRIENDLY);
        assert!((c[1] - 1.0).abs() < 1e-5 && (c[3] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn landbridge_and_bridge_keep_their_classes() {
        let land = PickTarget {
            watch_id: 1,
            // Structures never participate in the player-eject path.
            part_id: None,
            position: (0.0, 0.0, 0.0),
            screen_box: [0.0; 4],
            triangles: Default::default(),
            mover: false,
            team: TacTeam::Neutral,
            disabled: false,
            captureable: false,
            capture_blockers: Default::default(),
            class: ObjectClass::Building,
            landbridge: true,
            support: Default::default(),
            aim: None,
            info_unit: None,
        };
        let t = cursor_target(&land, true);
        assert!(t.landbridge && t.landbridge_passable);
        let bridge = PickTarget {
            class: ObjectClass::Bridge,
            landbridge: false,
            ..land
        };
        assert_eq!(cursor_target(&bridge, true).class, ObjectClass::Bridge);
    }

    fn cursor_ctx(target: Target, passable: bool, los: bool) -> CursorContext {
        CursorContext {
            line_of_sight: los,
            passable,
            movers: 1,
            rotation: mission_keys::state::NORMAL,
            range: FireRange::Optimal,
            selected: vec![SelectedMover {
                mech_on_ground: true,
                capture_blocked: false,
            }],
            target: Some(target),
            ..CursorContext::default()
        }
    }

    #[test]
    fn a_blocked_capture_shows_the_x_capture_cursor() {
        let mut target = PickTarget {
            watch_id: 9,
            part_id: None,
            position: (0.0, 0.0, 0.0),
            screen_box: [0.0; 4],
            triangles: Default::default(),
            mover: false,
            team: TacTeam::Enemy,
            disabled: false,
            captureable: true,
            capture_blockers: [(1, false), (2, true)].into(),
            class: ObjectClass::Building,
            landbridge: false,
            support: Default::default(),
            aim: None,
            info_unit: None,
        };
        let movers = vec![
            selected_mover(1, true, Some(&target)),
            selected_mover(2, true, Some(&target)),
        ];
        assert_eq!(
            movers.iter().map(|m| m.capture_blocked).collect::<Vec<_>>(),
            [false, true]
        );
        let mut ctx = cursor_ctx(cursor_target(&target, true), true, false);
        ctx.selected = movers;
        ctx.movers = 2;
        assert_eq!(choose_cursor(&ctx).cursor, state::XCAPTURE);
        target.capture_blockers.insert(2, false);
        ctx.selected = vec![
            selected_mover(1, true, Some(&target)),
            selected_mover(2, true, Some(&target)),
        ];
        assert_eq!(choose_cursor(&ctx).cursor, state::CAPTURE);
    }

    #[test]
    fn landbridge_and_bridge_cursors_match_make_target_cursor() {
        let land = Target {
            class: ObjectClass::Building,
            landbridge: true,
            landbridge_passable: true,
            ..Target::default()
        };
        let walk = choose_cursor(&cursor_ctx(land, true, false));
        assert!(
            (state::WALK1..state::WALK1 + 32).contains(&walk.cursor) || walk.cursor == state::MOVE,
            "a passable landbridge without force-fire is the no-target walk cursor, got {}",
            walk.cursor
        );
        assert!(walk.target_cleared);
        let blocked = Target {
            class: ObjectClass::Building,
            landbridge: true,
            landbridge_passable: false,
            ..Target::default()
        };
        let dont = choose_cursor(&cursor_ctx(blocked, false, false));
        assert_eq!(
            dont.cursor,
            state::DONT,
            "an impassable landbridge without force-fire is mState_DONT"
        );
        let bridge = Target {
            class: ObjectClass::Bridge,
            ..Target::default()
        };
        let over_bridge = choose_cursor(&cursor_ctx(bridge, true, true));
        assert_eq!(
            over_bridge.cursor,
            state::DONT,
            "a Bridge calls makeNoTargetCursor(false, ...) (`missiongui.cpp:3909-3912`)"
        );
        assert!(over_bridge.target_cleared);
    }
}
