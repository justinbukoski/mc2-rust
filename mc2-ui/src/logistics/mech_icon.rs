// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `LogisticsMechIcon` (`code/logisticsmechicon.cpp`): one slot of the force group in the mech bay
//! and the pilot ready room. A box with the mech's picture and chassis name, the pilot's name and
//! portrait, and the connector between them. Built from `mcl_gn_deploymentteams.fit`.
//!
//! Every part is held in screen coordinates and moved with the box (`move`), so an icon is a
//! grid cell that can be placed anywhere.

use super::strings;
use crate::anim::{AnimGroup, AnimState};
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::fit::FitExt;
use crate::widgets::{ItemState, Object, Text};
use mc2_campaign::{Logistics, MechKey};
use mc2_formats::ini::FitIni;

/// `ICON_ANIM_COUNT`.
pub const ICON_ANIM_COUNT: usize = 4;

/// The animation group index a part takes its colour from (`-1`: no group, drawn white).
fn animation_id(file: &FitIni) -> i32 {
    // the C++ keeps the first digit of the `Animation` key
    file.string("Animation")
        .and_then(|s| s.chars().find(|c| c.is_ascii_digit()))
        .and_then(|c| c.to_digit(10))
        .map_or(-1, |d| d as i32)
}

/// The texture cell of a sheet laid out `per_row` cells across: `(u, v, u2, v2)`.
pub fn cell_uvs(index: i64, per_row: i64, width: f32, height: f32) -> (f32, f32, f32, f32) {
    let x = (index % per_row) as f32;
    let y = (index / per_row) as f32;
    (x * width, y * height, (x + 1.0) * width, (y + 1.0) * height)
}

/// The mech and pilot the icon shows, as the screen read them from the campaign.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconContent {
    pub mech: Option<MechKey>,
    pub chassis_name: String,
    /// The pilot's name, or "no pilot".
    pub pilot_name: String,
    /// The pilot's portrait cell (`photo_index`), when a pilot is assigned.
    pub pilot_photo: Option<i64>,
}

#[derive(Clone)]
pub struct LogisticsMechIcon {
    pub outline: Object,
    pub icon: Object,
    pub chassis_name: Text,
    pub pilot_name: Text,
    pub connector: Object,
    pub pilot_icon: Object,
    /// `animations[ICON_ANIM_COUNT]`.
    pub anims: [AnimGroup; ICON_ANIM_COUNT],
    /// The animation index of each part (`pilotID`, `chassisNameID`, ...); -1 for none.
    outline_id: i32,
    icon_id: i32,
    chassis_id: i32,
    pilot_id: i32,
    connector_id: i32,
    pilot_icon_id: i32,
    /// `state`: enabled, highlighted or selected.
    pub state: ItemState,
    /// `bJustSelected`: set for the frame the icon was selected.
    pub just_selected: bool,
    /// `bDisabled`: a slot the campaign does not allow (drawn as an empty black box).
    pub disabled: bool,
    pub content: IconContent,
    /// `helpID`.
    pub help_id: i32,
}

impl LogisticsMechIcon {
    /// `LogisticsMechIcon::init` (the template) for one icon of the grid.
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni) -> LogisticsMechIcon {
        // `BoxOutline` is a bitmap (`aObject outline`), not a rect
        let outline = Object::from_fit(ld, file, "BoxOutline");
        let outline_id = block_animation(file, "BoxOutline");
        let icon = Object::from_fit(ld, file, "MechEntryIcon");
        let icon_id = block_animation(file, "MechEntryIcon");
        let chassis_name = Text::from_fit(ld, file, "MechNameText");
        let chassis_id = block_animation(file, "MechNameText");
        let pilot_name = Text::from_fit(ld, file, "PilotNameText");
        let pilot_id = block_animation(file, "PilotNameText");
        let connector = Object::from_fit(ld, file, "IconConnector");
        let connector_id = block_animation(file, "IconConnector");
        let pilot_icon = Object::from_fit(ld, file, "PilotIcon");
        let pilot_icon_id = block_animation(file, "PilotIcon");
        let anims = std::array::from_fn(|i| AnimGroup::from_block(file, &format!("Animation{i}")));
        LogisticsMechIcon {
            outline,
            icon,
            chassis_name,
            pilot_name,
            connector,
            pilot_icon,
            anims,
            outline_id,
            icon_id,
            chassis_id,
            pilot_id,
            connector_id,
            pilot_icon_id,
            state: ItemState::Enabled,
            just_selected: false,
            disabled: false,
            content: IconContent::default(),
            help_id: 0,
        }
    }

    /// `move(x, y)`: the box moves; its parts are placed relative to it when drawn (the C++
    /// `LogisticsMechIcon::render` draws them at the box's global position).
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.outline.move_by(dx, dy);
    }

    /// The box's top-left corner.
    pub fn origin(&self) -> (f32, f32) {
        (self.outline.left(), self.outline.top())
    }

    pub fn width(&self) -> f32 {
        self.outline.width()
    }

    pub fn height(&self) -> f32 {
        self.outline.height()
    }

    /// `setMech`: shows `mech` (or nothing) with its picture and names.
    pub fn set_mech(&mut self, assets: &UiAssets, lg: &Logistics, mech: Option<MechKey>) {
        self.content.mech = mech;
        let Some(key) = mech else {
            self.connector.show_window(false);
            self.content.chassis_name.clear();
            self.content.pilot_name.clear();
            self.content.pilot_photo = None;
            self.icon.set_uvs(0.0, 0.0, 0.0, 0.0);
            return;
        };
        let Some(chassis) = lg.mech(key).and_then(|m| lg.chassis_of(m.variant)) else {
            return;
        };
        let (u, v, u2, v2) = cell_uvs(
            chassis.icon_picture_index,
            10,
            self.icon.width(),
            self.icon.height(),
        );
        self.icon.file_width = 256.0;
        self.icon.set_uvs(u, v, u2, v2);
        self.content.chassis_name = assets.string(chassis.chassis_name_id as u32).to_string();
        self.chassis_name.set_text(&self.content.chassis_name);
        self.connector.show_window(true);
        let pilot = lg
            .mech(key)
            .and_then(|m| m.pilot)
            .and_then(|p| lg.pilots().get(p));
        match pilot {
            Some(p) => {
                self.content.pilot_name = assets.string(p.name_id as u32).to_string();
                self.content.pilot_photo = Some(p.photo_index);
            }
            None => {
                self.content.pilot_name = assets.string(strings::NOPILOT).to_string();
                self.content.pilot_photo = None;
            }
        }
        self.pilot_name.set_text(&self.content.pilot_name);
    }

    /// `setPilot`: the pilot's portrait and name on the icon (the campaign assignment is made by
    /// the screen, through `Logistics::set_mech_pilot`).
    pub fn set_pilot(&mut self, assets: &UiAssets, lg: &Logistics) {
        let pilot = self
            .content
            .mech
            .and_then(|k| lg.mech(k))
            .and_then(|m| m.pilot)
            .and_then(|p| lg.pilots().get(p));
        match pilot {
            Some(p) => {
                let (u, v, u2, v2) = cell_uvs(
                    p.photo_index,
                    12,
                    self.pilot_icon.width(),
                    self.pilot_icon.height(),
                );
                self.pilot_icon.set_uvs(u, v, u2, v2);
                self.content.pilot_name = assets.string(p.name_id as u32).to_string();
                self.content.pilot_photo = Some(p.photo_index);
            }
            None => {
                self.pilot_icon.set_uvs(0.0, 0.0, 0.0, 0.0);
                self.content.pilot_name = assets.string(strings::NOPILOT).to_string();
                self.content.pilot_photo = None;
            }
        }
        self.pilot_name.set_text(&self.content.pilot_name);
    }

    /// `dimPilot`: the portrait dimmed while it is dragged.
    pub fn dim_pilot(&mut self, dim: bool) {
        self.pilot_icon
            .set_color(if dim { 0x7f00_0000 } else { 0xffff_ffff }, false);
    }

    /// `select`: selects (pressed look) or clears the icon.
    pub fn select(&mut self, select: bool) {
        if !select || self.disabled {
            self.state = ItemState::Enabled;
            for a in &mut self.anims {
                a.set_state(AnimState::Normal);
            }
        } else {
            self.state = ItemState::Selected;
            for a in &mut self.anims {
                a.set_state(AnimState::Pressed);
            }
            self.just_selected = true;
        }
    }

    pub fn is_selected(&self) -> bool {
        self.state == ItemState::Selected
    }

    /// `pointInside`: over the box.
    pub fn point_inside(&self, x: i32, y: i32) -> bool {
        self.outline.point_inside(x, y)
    }

    /// `update`: hover and click states. A click selects the icon and plays the select sound;
    /// hovering it sets the help text (the caller reads [`LogisticsMechIcon::help_id`]).
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.just_selected = false;
        if self.content.mech.is_none() {
            return;
        }
        for a in &mut self.anims {
            a.update(ctx.dt);
        }
        let (x, y) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if self.outline.point_inside(x, y) {
            if (ctx.input.is_left_click() || ctx.input.is_left_double_click())
                && self.content.mech.is_some()
            {
                for a in &mut self.anims {
                    a.set_state(AnimState::Pressed);
                }
                self.just_selected = true;
                if self.state != ItemState::Selected {
                    ctx.play(crate::ctx::sfx::LOG_SELECT);
                }
                self.state = ItemState::Selected;
            } else if matches!(self.state, ItemState::Enabled | ItemState::Highlite) {
                for a in &mut self.anims {
                    a.set_state(AnimState::Highlight);
                }
                if self.state != ItemState::Highlite {
                    ctx.play(crate::ctx::sfx::LOG_HIGHLIGHTBUTTONS);
                }
                self.state = ItemState::Highlite;
            }
            ctx.out.help_text_id = self.help_id;
        } else if self.state != ItemState::Selected {
            for a in &mut self.anims {
                a.set_state(AnimState::Normal);
            }
            self.state = ItemState::Enabled;
        }
    }

    fn colour(&self, id: i32) -> Option<u32> {
        let i = usize::try_from(id).ok()?;
        let group = self.anims.get(i)?;
        Some(group.current_color(group.state()))
    }

    /// `render(xOffset, yOffset)`: the box, then the parts in the C++ order.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.disabled {
            let r = self.outline.global_rect();
            ctx.dl.fill_rect(r, 0xff00_0000);
            return;
        }
        if self.content.mech.is_none() {
            return;
        }
        let outline_color = self.colour(self.outline_id).unwrap_or(0xffff_ffff);
        self.outline.set_color(outline_color, false);
        self.outline.render(ctx);

        let white = |c: Option<u32>| c.unwrap_or(0xffff_ffff);
        let (ox, oy) = (self.outline.left(), self.outline.top());
        let (icon, chassis, pilot, connector) = (
            white(self.colour(self.icon_id)),
            white(self.colour(self.chassis_id)),
            white(self.colour(self.pilot_id)),
            white(self.colour(self.connector_id)),
        );
        self.icon.set_color(icon, false);
        self.icon.render_at(ctx, ox, oy);
        self.chassis_name.obj.set_color(chassis, false);
        self.chassis_name.render_at(ctx, ox, oy);
        self.pilot_name.obj.set_color(pilot, false);
        self.pilot_name.render_at(ctx, ox, oy);
        self.connector.set_color(connector, false);
        self.connector.render_at(ctx, ox, oy);
        self.icon.set_color(icon, false);
        self.icon.render_at(ctx, ox, oy);
        if self.content.pilot_photo.is_some() {
            let c = white(self.colour(self.pilot_icon_id));
            self.pilot_icon.set_color(c, false);
            self.pilot_icon.render_at(ctx, ox, oy);
        }
    }
}

/// The animation index of the block `block` (its `Animation` key).
fn block_animation(file: &mut FitIni, block: &str) -> i32 {
    if file.seek_block(block).is_err() {
        return -1;
    }
    animation_id(file)
}

/// The twelve slots: copies of the template, moved to a grid four across and three down
/// (`MechBayScreen::init`: each step right is the box width plus 4, each row down the box height
/// plus 5).
pub fn icon_grid(template: LogisticsMechIcon) -> Vec<LogisticsMechIcon> {
    use super::mech_bay::{ICON_COUNT_X, ICON_COUNT_Y};
    let mut icons = Vec::with_capacity(ICON_COUNT_X * ICON_COUNT_Y);
    let (mut x, mut y) = (0.0f32, 0.0f32);
    for _ in 0..ICON_COUNT_Y {
        for _ in 0..ICON_COUNT_X {
            let mut icon = template.clone();
            icon.move_by(x, y);
            x += icon.width() + 4.0;
            icons.push(icon);
        }
        y += template.height() + 5.0;
        x = 0.0;
    }
    icons
}
