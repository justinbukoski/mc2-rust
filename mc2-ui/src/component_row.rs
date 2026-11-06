// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `ComponentListItem` (`code/componentlistbox.cpp`): a row of the mech lab's component list. The
//! component's icon art over its cells, its name, its cost and heat with their small icons, in
//! an outline box and an icon outline, all placed by the blocks of `mcl_mc.fit`
//! (`ComponentEntryBox`, `ComponentIconOutlineRect`, `ComponentIcon`, `ComponentNameText`,
//! `ComponentCostText`, `ComponentHeatText`, `ComponentCostIcon`, `ComponentHeatIcon`,
//! `ComponentDisabledText`) and coloured by the item's animation for its state.

use crate::anim::AnimGroup;
use crate::ctx::{Loader, RenderCtx};
use crate::fit::FitExt;
use crate::mech_lab::{XICON_FACTOR, YICON_FACTOR};
use crate::textures::TexId;
use crate::widgets::{ItemState, ListItem, Object, Rect, Text};
use mc2_formats::ini::FitIni;
use std::any::Any;

/// `COMP_ANIMATION_COUNT`: the animations the template reads (`Animation0..5`).
const ANIMATIONS: usize = 6;

/// The children of a row, in `addChild` order: the animation each is coloured by is the one its
/// block names (`assignAnimation`).
const CHILDREN: usize = 6;

/// The template row (`ComponentListItem::s_templateItem`).
#[derive(Clone, Debug)]
pub struct RowTemplate {
    outline: Rect,
    icon_outline: Rect,
    icon: Object,
    name: Text,
    cost: Text,
    heat: Text,
    cost_icon: Object,
    heat_icon: Object,
    disabled: Text,
    animations: [AnimGroup; ANIMATIONS],
    /// The animation of the icon, name, cost, heat, cost icon and heat icon (`pChildAnims`).
    anim_of: [Option<usize>; CHILDREN],
}

/// The index of the animation a block names (`Animation = Animation3`), if it names one.
fn animation_of(file: &mut FitIni, block: &str) -> Option<usize> {
    file.seek_block(block).ok()?;
    let name = file.string("Animation")?;
    let digits: String = name.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&i| i < ANIMATIONS)
}

impl RowTemplate {
    /// `ComponentListItem::init(file)` over the layout `mcl_mc.fit`.
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni) -> RowTemplate {
        let animations =
            std::array::from_fn(|i| AnimGroup::from_block(file, &format!("Animation{i}")));
        let anim_of = [
            "ComponentIcon",
            "ComponentNameText",
            "ComponentCostText",
            "ComponentHeatText",
            "ComponentCostIcon",
            "ComponentHeatIcon",
        ]
        .map(|b| animation_of(file, b));
        RowTemplate {
            icon: Object::from_fit(ld, file, "ComponentIcon"),
            name: Text::from_fit(ld, file, "ComponentNameText"),
            cost: Text::from_fit(ld, file, "ComponentCostText"),
            heat: Text::from_fit(ld, file, "ComponentHeatText"),
            cost_icon: Object::from_fit(ld, file, "ComponentCostIcon"),
            heat_icon: Object::from_fit(ld, file, "ComponentHeatIcon"),
            disabled: Text::from_fit(ld, file, "ComponentDisabledText"),
            icon_outline: Rect::from_fit(file, "ComponentIconOutlineRect"),
            outline: Rect::from_fit(file, "ComponentEntryBox"),
            animations,
            anim_of,
        }
    }

    /// `ComponentListItem::ComponentListItem(pComp)`: a row for one component. `icon` is its art
    /// and the file's width, `cells` its size in lab cells.
    pub fn row(
        &self,
        index: usize,
        name: &str,
        cost: i64,
        heat: f32,
        icon: Option<(TexId, u32)>,
        cells: (i32, i32),
    ) -> ComponentRow {
        let (size_x, size_y) = cells;
        let mut row = ComponentRow {
            obj: Object::default(),
            state: ItemState::Enabled,
            index,
            icon: self.icon.clone(),
            name: self.name.clone(),
            cost: self.cost.clone(),
            heat: self.heat.clone(),
            cost_icon: self.cost_icon.clone(),
            heat_icon: self.heat_icon.clone(),
            disabled: self.disabled.clone(),
            outline: self.outline.clone(),
            icon_outline: self.icon_outline.clone(),
            icon_outline_offset: (self.icon_outline.obj.left(), self.icon_outline.obj.top()),
            animations: self.animations.clone(),
            anim_of: self.anim_of,
            has_icon: icon.is_some(),
        };
        row.name.set_text(name);
        row.cost.set_text(&cost.to_string());
        row.heat.set_text(&format!("{heat:.0}"));
        if let Some((tex, width)) = icon {
            row.icon.set_texture(Some(tex), width);
        }
        row.icon
            .resize(size_x as f32 * XICON_FACTOR, size_y as f32 * YICON_FACTOR);
        row.icon
            .set_uvs(0.0, 0.0, size_x as f32 * 48.0, size_y as f32 * 32.0);
        // the row is as big as the template's, grown by what the icon is bigger than the template's
        let delta_y = row.icon.height() - self.icon.height();
        let delta_x = row.icon.width() - self.icon.width();
        let (ow, oh) = (
            self.outline.obj.width(),
            self.outline.obj.height() + delta_y,
        );
        row.outline.obj.resize(ow, oh);
        let (iw, ih) = (
            self.icon_outline.obj.width() + delta_x,
            self.icon_outline.obj.height() + delta_y,
        );
        row.icon_outline.obj.resize(iw, ih);
        row.disabled.obj.resize(ow, oh);
        row.disabled.obj.move_to(0.0, 0.0);
        row.disabled.alignment = 3;
        row.obj = Object::new(self.outline.obj.left(), self.outline.obj.top(), ow, oh);
        row
    }
}

/// One row of the component list.
#[derive(Clone, Debug)]
pub struct ComponentRow {
    obj: Object,
    state: ItemState,
    /// The catalogue index of the component.
    pub index: usize,
    icon: Object,
    name: Text,
    cost: Text,
    heat: Text,
    cost_icon: Object,
    heat_icon: Object,
    disabled: Text,
    outline: Rect,
    icon_outline: Rect,
    icon_outline_offset: (f32, f32),
    animations: [AnimGroup; ANIMATIONS],
    anim_of: [Option<usize>; CHILDREN],
    has_icon: bool,
}

impl ComponentRow {
    /// The name, as shown.
    pub fn name(&self) -> &str {
        &self.name.text
    }

    /// The cost text, as shown.
    pub fn cost_text(&self) -> &str {
        &self.cost.text
    }

    /// The heat text, as shown.
    pub fn heat_text(&self) -> &str {
        &self.heat.text
    }

    /// The reason the row is greyed out (`ComponentDisabledText`), empty when none is given.
    pub fn disabled_text(&self) -> &str {
        &self.disabled.text
    }

    /// Whether the row's icon art is loaded.
    pub fn has_icon(&self) -> bool {
        self.has_icon
    }

    /// `disabledText.setText(...)`.
    pub fn set_disabled_text(&mut self, text: &str) {
        self.disabled.set_text(text);
    }

    /// The colour the animation of child `i` gives in the row's state: the first key of the
    /// normal animation, the last key of the state's own.
    fn colour(&self, i: usize) -> Option<u32> {
        let group = &self.animations[self.anim_of[i]?];
        let a = &group.animations[self.state.anim_state() as usize];
        if a.infos.is_empty() {
            return None;
        }
        let t = if self.state == ItemState::Enabled {
            0.0
        } else {
            a.max_time()
        };
        Some(a.color_at(t))
    }
}

impl ListItem for ComponentRow {
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
    fn text(&self) -> Option<&str> {
        Some(&self.name.text)
    }
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.icon.move_by(dx, dy);
        self.name.obj.move_by(dx, dy);
        self.cost.obj.move_by(dx, dy);
        self.heat.obj.move_by(dx, dy);
        self.cost_icon.move_by(dx, dy);
        self.heat_icon.move_by(dx, dy);
        self.disabled.obj.move_by(dx, dy);
    }
    /// `ComponentListItem::render`: the children, then the two outlines, all in the animation
    /// colour of the state; the icon is grey when the lab cannot take the component.
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        let colours: [Option<u32>; CHILDREN] = std::array::from_fn(|i| self.colour(i));
        if let Some(c) = colours[1] {
            self.name.set_color(c);
        }
        if let Some(c) = colours[2] {
            self.cost.set_color(c);
        }
        if let Some(c) = colours[3] {
            self.heat.set_color(c);
        }
        if let Some(c) = colours[4] {
            self.cost_icon.set_color(c, false);
        }
        if let Some(c) = colours[5] {
            self.heat_icon.set_color(c, false);
        }
        self.icon.set_color(
            if self.state == ItemState::Disabled {
                0xff40_4040
            } else {
                0xffff_ffff
            },
            false,
        );
        self.icon.render(ctx);
        self.name.render(ctx);
        self.cost.render(ctx);
        self.heat.render(ctx);
        self.cost_icon.render(ctx);
        self.heat_icon.render(ctx);
        self.disabled.render(ctx);
        let outline_colour = colours[0].or_else(|| {
            self.animations[0].animations[self.state.anim_state() as usize]
                .infos
                .last()
                .map(|k| k.color)
        });
        self.outline
            .obj
            .move_to(self.obj.global_x() as f32, self.obj.global_y() as f32);
        self.icon_outline.obj.move_to(
            self.obj.global_x() as f32 + self.icon_outline_offset.0,
            self.obj.global_y() as f32 + self.icon_outline_offset.1,
        );
        if let Some(c) = outline_colour {
            self.outline.obj.set_color(c, false);
            self.icon_outline.obj.set_color(c, false);
        }
        self.outline.render(ctx);
        self.icon_outline.render(ctx);
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
