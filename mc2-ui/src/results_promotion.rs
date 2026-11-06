// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The promotion pop-up of the pilot review (`PilotPromotionArea`, `SpecialtyListItem`,
//! `code/pilotreviewarea.cpp:1217-1789`): when a promoted pilot's reveal has run, the pilot's card
//! (photo, rank, skill meters, the specialties they have) slides in from the left and the list of
//! specialty skills they can pick from slides in from the right. Picking one and pressing the
//! confirm button slides both away.

use crate::anim::Animation;
use crate::ctx::{betty, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::font::FontRef;
use crate::logistics::attribute_meter::AttributeMeter;
use crate::logistics_dialog::format_name;
use crate::logistics_ids::ids;
use crate::results_icons::IconArt;
use crate::results_pilots::{format_count, rank_text};
use crate::screen::{LogisticsScreen, Prefixes};
use crate::widgets::{
    AnimButton, Button, ItemState, ListBox, ListItem, Object, Rect, Text, TextListItem,
};
use mc2_campaign::pilot::{FIRST_ACE_SPECIALTY, FIRST_ELITE_SPECIALTY, FIRST_VETERAN_SPECIALTY};
use mc2_campaign::Logistics;
use mc2_formats::ini::FitIni;
use mc2_sim_data::pilot::{SpecialtySkill, NUM_SPECIALTY_SKILLS};
use std::any::Any;

/// `PromoteGadgetRightButton0` (id 101): the confirm button.
pub const CONFIRM: i32 = 101;
/// `IDS_PROMOTION_SKILL`: the skill descriptions are `IDS_PROMOTION_SKILL + skill`.
const IDS_PROMOTION_SKILL: u32 = 27024;
/// `IDS_PROMOTION_LISTBOX_FONT`.
const IDS_PROMOTION_LISTBOX_FONT: u32 = 40050;
const IDS_PILOT_NAME: u32 = 27014;
const IDS_PILOT_MISSIONS: u32 = 27016;
/// The rank pictures of the left gadget are statics 12 to 16.
const RANK_STATIC: usize = 12;
/// The left gadget's text of the pilot's first specialty; the picked one follows the others.
const SPECIALTY_TEXT: usize = 8;

/// What a row of the skill list is cloned from (`SpecialtyListItem::init`).
#[derive(Clone)]
struct RowTemplate {
    description: Text,
    icons: Vec<Object>,
    radio: Button,
    outline: Rect,
    normal: Animation,
    pressed: Animation,
    highlight: Animation,
    area: (f32, f32, f32, f32),
}

/// `SpecialtyListItem`: one skill the pilot can pick.
pub struct SkillRow {
    obj: Object,
    state: ItemState,
    t: RowTemplate,
    pub skill: usize,
    description: Text,
    icon: Object,
    radio: Button,
    outline: Rect,
    text: String,
}

impl SkillRow {
    fn new(t: &RowTemplate, assets: &UiAssets, skill: usize) -> SkillRow {
        let (l, top, r, b) = t.area;
        let kind = SpecialtySkill::ALL
            .get(skill)
            .map_or(3, |s| s.kind() as usize);
        let mut description = t.description.clone();
        description.set_text(assets.string(IDS_PROMOTION_SKILL + skill as u32));
        let mut outline = t.outline.clone();
        outline.outline = true;
        let mut normal = t.normal.clone();
        normal.begin();
        SkillRow {
            // `aObject::init(left, top, right, bottom)`: the area's right and bottom are the size
            obj: Object::new(l, top, r, b),
            state: ItemState::Enabled,
            skill,
            icon: t.icons.get(kind).cloned().unwrap_or_default(),
            radio: t.radio.clone(),
            text: description.text.clone(),
            description,
            outline,
            t: RowTemplate {
                normal,
                ..t.clone()
            },
        }
    }

    pub fn description(&self) -> &str {
        &self.description.text
    }

    pub fn is_pressed(&self) -> bool {
        self.radio.is_pressed()
    }

    /// Where the skill's type icon is drawn.
    pub fn icon_rect(&self) -> (f32, f32, f32, f32) {
        let l = &self.icon.loc;
        (l[0].x, l[0].y, l[2].x, l[2].y)
    }

    pub fn icon_kind_uvs(&self) -> [f32; 4] {
        [
            self.icon.loc[0].u,
            self.icon.loc[0].v,
            self.icon.loc[2].u,
            self.icon.loc[2].v,
        ]
    }

    fn colour(&self) -> u32 {
        match self.state {
            ItemState::Selected => self.t.pressed.color(),
            ItemState::Highlite => self.t.highlight.color(),
            _ => self.t.normal.color(),
        }
    }
}

impl ListItem for SkillRow {
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
    fn is_checked(&self) -> bool {
        self.radio.is_pressed()
    }
    fn set_check(&mut self, checked: bool) {
        self.radio.press(checked);
        self.state = if checked {
            ItemState::Selected
        } else {
            ItemState::Enabled
        };
    }
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.obj.move_by(dx, dy);
        self.description.obj.move_by(dx, dy);
        self.icon.move_by(dx, dy);
        self.radio.move_by(dx, dy);
        self.outline.obj.move_by(dx, dy);
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.t.normal.update(ctx.dt);
        match self.state {
            ItemState::Selected => {
                if !self.radio.is_pressed() {
                    self.radio.press(true);
                }
                if !self.t.pressed.is_animating() {
                    self.t.pressed.begin();
                }
                self.t.pressed.update(ctx.dt);
            }
            ItemState::Highlite => {
                if !self.t.highlight.is_animating() {
                    self.t.highlight.begin();
                }
                self.t.highlight.update(ctx.dt);
            }
            _ => {}
        }
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        let c = self.colour();
        self.description.set_color(c);
        self.icon.set_color(c, false);
        self.radio.obj.set_color(c, false);
        self.outline.obj.set_color(c, false);
        // the background stays black
        self.obj.draw_quad(ctx);
        self.description.render(ctx);
        self.icon.render(ctx);
        self.radio.render(ctx);
        self.outline.render(ctx);
    }
    fn text(&self) -> Option<&str> {
        Some(&self.text)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// What the area reports: the player confirmed `skill` for the pilot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pick {
    pub pilot: usize,
    pub skill: usize,
}

/// `PilotPromotionArea`.
pub struct PromotionArea {
    left: LogisticsScreen,
    right: LogisticsScreen,
    meters: [AttributeMeter; 2],
    left_enter: Animation,
    right_enter: Animation,
    left_exit: Animation,
    right_exit: Animation,
    selection: Animation,
    /// `skillListBox`: headings and skills.
    pub list: ListBox,
    /// The skill each row of `list` picks; `None` for the headings.
    pub rows: Vec<Option<usize>>,
    row: RowTemplate,
    heading_font: FontRef,
    pilot: Option<usize>,
    photo: u32,
    /// How many specialties the pilot had when the area opened (`getSpecialtySkillCount`).
    owned: usize,
    done: bool,
    /// The confirm was pressed and the screen has not been told yet.
    confirmed: bool,
    last_check: i32,
    art: IconArt,
}

impl PromotionArea {
    /// `PilotPromotionArea::init` and `SpecialtyListItem::init`.
    pub fn load(
        ld: &mut Loader<'_>,
        file: &mut FitIni,
        art: IconArt,
    ) -> Result<PromotionArea, String> {
        let left = LogisticsScreen::from_fit(
            ld,
            file,
            Prefixes {
                statics: Some("PromoteGadgetLeftStatic"),
                texts: Some("PromoteGadgetLeftText"),
                rects: Some("PromoteGadgetLeftRect"),
                buttons: None,
                edits: None,
                anim_objects: None,
            },
        );
        let mut right = LogisticsScreen::from_fit(
            ld,
            file,
            Prefixes {
                statics: Some("PromoteGadgetRightStatic"),
                texts: Some("PromoteGadgetRightText"),
                rects: Some("PromoteGadgetRightRect"),
                buttons: Some("PromoteGadgetRightButton"),
                edits: None,
                anim_objects: None,
            },
        );
        for b in &mut right.buttons {
            b.set_message_on_release();
            b.disable(true);
        }
        let anim = |file: &mut FitIni, block: &str| Animation::from_block(file, block);
        let mut meters = [
            AttributeMeter::from_fit(file, "PromoteGadgetLeftSkillMeter0"),
            AttributeMeter::from_fit(file, "PromoteGadgetLeftSkillMeter1"),
        ];
        for m in &mut meters {
            m.added_color_max = 0xffff_ffff;
            m.added_color_min = 0xffff_ffff;
        }
        // `SpecialtyListItem::init`
        let radio = Button::from_fit(ld, file, "PromoteGadgetRightRadioButton");
        let icons = (0..4)
            .map(|i| Object::from_fit(ld, file, &format!("SkillTypeIcon{i}")))
            .collect();
        let description = Text::from_fit(ld, file, "SkillDescriptionText");
        let _ = file.seek_block("PromoteGadgetRightSkillEntryBox");
        let highlight = Animation::from_fit(file, "Highlight");
        let normal = Animation::from_fit(file, "Normal");
        let pressed = Animation::from_fit(file, "Pressed");
        let outline = Rect::from_fit(file, "PromoteGadgetRightSkillEntryOutline");
        let area = Rect::from_fit(file, "PromoteGadgetRightSkillEntryBox");
        let a = &area.obj;
        let row = RowTemplate {
            description,
            icons,
            radio,
            outline,
            normal,
            pressed,
            highlight,
            area: (a.left(), a.top(), a.right(), a.bottom()),
        };
        let r = right
            .rect(2)
            .map(|r| (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height()))
            .ok_or("the promotion area has no skill list rectangle")?;
        let mut list = ListBox::new(ld, r.0, r.1, r.2, r.3);
        list.skip_amount = 5;
        let heading_font = ld.font(IDS_PROMOTION_LISTBOX_FONT);
        Ok(PromotionArea {
            left_enter: anim(file, "PromoteGadgetLeftEnterAnimation"),
            right_enter: anim(file, "PromoteGadgetRightEnterAnimation"),
            left_exit: anim(file, "PromoteGadgetLeftExitAnimation"),
            right_exit: anim(file, "PromoteGadgetRightExitAnimation"),
            selection: anim(file, "SkillSelectionAnimation"),
            left,
            right,
            meters,
            list,
            rows: Vec::new(),
            row,
            heading_font,
            pilot: None,
            photo: 0,
            owned: 0,
            done: false,
            confirmed: false,
            last_check: -1,
            art,
        })
    }

    pub fn pilot(&self) -> Option<usize> {
        self.pilot
    }

    /// The pop-up is open for `pilot`.
    pub fn picker_pilot_is(&self, pilot: usize) -> bool {
        self.pilot == Some(pilot)
    }

    pub fn is_open(&self) -> bool {
        self.pilot.is_some()
    }

    /// Both halves have slid in: the list can be clicked where it is drawn.
    pub fn entered(&self) -> bool {
        self.left_enter.is_done() && self.right_enter.is_done()
    }

    /// `isDone`: the confirm was pressed and both halves have slid away.
    pub fn is_done(&self) -> bool {
        self.done && self.right_exit.is_done() && self.left_exit.is_done()
    }

    /// Whether the confirm was pressed since this was last asked (the review fades back in).
    pub fn take_confirmed(&mut self) -> bool {
        std::mem::take(&mut self.confirmed)
    }

    /// The area closes (`s_curPromotion = NULL`).
    pub fn close(&mut self) {
        self.pilot = None;
        self.done = false;
        self.confirmed = false;
    }

    /// The confirm button.
    pub fn confirm(&self) -> Option<&AnimButton> {
        self.right.button(CONFIRM)
    }

    /// The text `i` of the left half.
    pub fn left_text(&self, i: usize) -> &str {
        self.left.texts.get(i).map_or("", |t| t.text.as_str())
    }

    /// The rank pictures shown (statics 12 to 16 that are visible).
    pub fn rank_pictures_shown(&self) -> Vec<usize> {
        (0..5)
            .filter(|&i| {
                self.left
                    .statics
                    .get(RANK_STATIC + i)
                    .is_some_and(|s| s.show)
            })
            .collect()
    }

    /// The left half's slide offset right now.
    pub fn left_offset(&self) -> (i32, i32) {
        if self.done {
            (
                self.left_exit.x_delta() as i32,
                self.left_exit.y_delta() as i32,
            )
        } else {
            (
                self.left_enter.x_delta() as i32,
                self.left_enter.y_delta() as i32,
            )
        }
    }

    pub fn right_offset(&self) -> (i32, i32) {
        if self.done {
            (
                self.right_exit.x_delta() as i32,
                self.right_exit.y_delta() as i32,
            )
        } else {
            (
                self.right_enter.x_delta() as i32,
                self.right_enter.y_delta() as i32,
            )
        }
    }

    /// Where the pilot's photo is drawn now (the left half slides).
    pub fn photo_rect(&self) -> Option<(f32, f32, f32, f32)> {
        let (lx, ly) = self.left_offset();
        let r = &self.left.rect(3)?.obj;
        Some((
            r.left() + lx as f32 + 1.0,
            r.top() + ly as f32 + 1.0,
            r.right() + lx as f32 - 1.0,
            r.bottom() + ly as f32 - 1.0,
        ))
    }

    pub fn meters(&self) -> &[AttributeMeter; 2] {
        &self.meters
    }

    /// `PilotPromotionArea::setPilot`: the area for `pilot`, who has just been promoted.
    pub fn set_pilot(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        logistics: &Logistics,
        pilot: usize,
        photo: u32,
    ) {
        let Some(p) = logistics.pilots().get(pilot) else {
            return;
        };
        self.pilot = Some(pilot);
        self.photo = photo;
        self.done = false;
        self.last_check = -1;
        let assets = ctx.assets;
        let set = |s: &mut LogisticsScreen, i: usize, text: &str| {
            if let Some(t) = s.texts.get_mut(i) {
                t.set_text(text);
            }
        };
        let missions = format_count(
            assets.string(IDS_PILOT_MISSIONS),
            p.missions_completed as i64,
        );
        set(&mut self.left, 2, &missions);
        set(
            &mut self.left,
            1,
            &format_name(
                assets.string(IDS_PILOT_NAME),
                assets.string(p.name_id as u32),
            ),
        );
        for i in 0..5 {
            if let Some(s) = self.left.statics.get_mut(RANK_STATIC + i) {
                s.show = i as i32 == p.rank;
            }
        }
        set(&mut self.left, 3, &rank_text(assets, p.rank));
        set(&mut self.left, 4, &(p.gunnery as i64).to_string());
        set(&mut self.left, 5, &(p.piloting as i64).to_string());
        let owned = p.specialty_skill_indices();
        self.owned = owned.len();
        for j in 0..4 {
            let name = owned
                .get(j)
                .map(|&s| {
                    assets
                        .string((ids::SPECIALTY + s as i32) as u32)
                        .to_string()
                })
                .unwrap_or_default();
            set(&mut self.left, SPECIALTY_TEXT + j, &name);
        }
        self.meters[0].set_value(p.gunnery / 100.0 - p.new_gunnery / 100.0);
        self.meters[1].set_value(p.piloting / 100.0 - p.new_piloting / 100.0);
        self.meters[0].set_added_value(p.new_gunnery / 100.0);
        self.meters[1].set_added_value(p.new_piloting / 100.0);
        self.left_enter.begin();
        self.right_enter.begin();
        self.list.remove_all_items();
        self.list.set_scroll_pos(0);
        self.rows.clear();
        let max_skill = p.specialty_choice_end();
        ctx.play_betty(match max_skill {
            NUM_SPECIALTY_SKILLS => betty::PROMO_ACE,
            FIRST_ACE_SPECIALTY => betty::PROMO_ELITE,
            FIRST_ELITE_SPECIALTY => betty::PROMO_VETERAN,
            _ => betty::PROMO_REGULAR,
        });
        let width = self.list.obj.width();
        for s in 0..max_skill {
            let row = SkillRow::new(&self.row, assets, s);
            let heading = match s {
                0 => Some(ids::REGULAR_SKILLS),
                FIRST_VETERAN_SPECIALTY => Some(ids::VETERAN_SKILLS),
                FIRST_ELITE_SPECIALTY => Some(ids::ELITE_SKILLS3),
                FIRST_ACE_SPECIALTY => Some(ids::ACE_SKILLS),
                _ => None,
            };
            if let Some(id) = heading {
                let mut item = TextListItem::new(assets, self.heading_font);
                item.set_text(assets.string(id as u32));
                item.set_color(0xffff_ffff);
                item.resize(width, row.obj.height() / 2.0);
                self.list.add_item(Box::new(item));
                self.rows.push(None);
            }
            if !owned.contains(&s) {
                self.list.add_item(Box::new(row));
                self.rows.push(Some(s));
            }
        }
        let rows = self.list.item_count();
        for i in 0..rows {
            if self.rows[i].is_none() {
                if let Some(item) = self.list.item_mut(i) {
                    item.set_state(ItemState::Disabled);
                }
            }
        }
        if let Some(b) = self.right.button_mut(CONFIRM) {
            b.disable(true);
        }
    }

    /// The skill the player has ticked, if any.
    pub fn checked_skill(&self) -> Option<usize> {
        usize::try_from(self.list.checked_item())
            .ok()
            .and_then(|i| self.rows.get(i).copied().flatten())
    }

    /// `PilotPromotionArea::update`: returns the pick once the confirm has been pressed.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) -> Option<Pick> {
        let pilot = self.pilot?;
        for a in [
            &mut self.left_enter,
            &mut self.right_enter,
            &mut self.right_exit,
            &mut self.left_exit,
        ] {
            a.update(ctx.dt);
        }
        let _ = self.left.update(ctx);
        let msgs = self.right.update(ctx);
        let mut pick = None;
        if !self.done && msgs.iter().any(|m| m.who == CONFIRM) {
            // `handleMessage(aMSG_DONE)`
            self.right_exit.begin();
            self.left_exit.begin();
            self.done = true;
            self.confirmed = true;
        }
        if !self.done {
            self.list.update(ctx);
        }
        let index = self.list.checked_item();
        if index != -1 && (index != self.last_check || self.done) {
            let skill = self.rows.get(index as usize).copied().flatten();
            if let Some(skill) = skill {
                if self.done {
                    pick = Some(Pick { pilot, skill });
                    if let Some(item) = self.list.item_mut(index as usize) {
                        item.set_check(false);
                    }
                } else {
                    // the skill's name takes the next free specialty line, flashing
                    let at = SPECIALTY_TEXT + self.owned;
                    if at < self.left.texts.len() {
                        let name = ctx.assets.string((ids::SPECIALTY + skill as i32) as u32);
                        self.left.texts[at].set_text(name);
                    }
                    self.selection.begin();
                }
            }
            self.last_check = index;
            if let Some(b) = self.right.button_mut(CONFIRM) {
                b.disable(false);
            }
        } else if index == -1 {
            if let Some(b) = self.right.button_mut(CONFIRM) {
                b.disable(true);
            }
        }
        self.selection.update(ctx.dt);
        let at = SPECIALTY_TEXT + self.owned;
        let colour = self.selection.color();
        if let Some(t) = self.left.texts.get_mut(at) {
            t.set_color(colour);
        }
        pick
    }

    /// `PilotPromotionArea::render`.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if self.pilot.is_none() {
            return;
        }
        let (lx, ly) = self.left_offset();
        self.left.render_offset(ctx, lx, ly);
        let rect = self.left.rect(3).map(|r| &r.obj);
        if let Some(r) = rect {
            let (l, t, rr, b) = (r.left(), r.top(), r.right(), r.bottom());
            self.art.pilot(
                ctx,
                self.photo,
                (
                    l + lx as f32 + 1.0,
                    t + ly as f32 + 1.0,
                    rr + lx as f32 - 1.0,
                    b + ly as f32 - 1.0,
                ),
            );
        }
        for m in &self.meters {
            m.render(ctx, lx, ly);
        }
        let (rx, ry) = self.right_offset();
        self.list.move_by(rx as f32, ry as f32);
        self.list.obj.set_color(0xff00_0000, false);
        self.list.render(ctx);
        self.list.move_by(-rx as f32, -ry as f32);
        self.right.render_offset(ctx, rx, ry);
    }
}
