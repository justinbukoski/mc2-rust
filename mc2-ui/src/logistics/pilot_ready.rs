// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `PilotReadyScreen` (`code/pilotreadyscreen.cpp`): the pilot ready room. The force group's slots
//! take a pilot each; the pilots not in a mech are listed, and the selected pilot's record (rank,
//! skills, medals, portrait) shows on the right. A pilot is put in a slot by a double-click, by
//! dragging it there, or with the ADD button; REMOVE takes one back out.
//!
//! The mech's statistics show (`MechDisplay`) when a slot's mech is picked. The room plays Betty's
//! new-pilot sample (`BETTY_NEW_PILOTS`) when new pilots arrive, then acknowledges them.

use std::collections::HashMap;

use super::attribute_meter::AttributeMeter;
use super::mech_display::MechDisplay;
use super::mech_icon::{icon_grid, LogisticsMechIcon};
use super::pilot_list::{set_portrait, PilotEntry, PilotEntryTemplate, PilotEvent, PilotList};
use super::{ids, strings, Page};
use crate::ctx::{betty, sfx, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::textures::TexId;
use crate::widgets::{ListBox, Object};
use mc2_campaign::{Logistics, MechKey};
use mc2_sim_data::pilot::SpecialtySkill;

/// `ICON_COUNT_X * ICON_COUNT_Y`: the force group.
const SLOTS: usize = 12;
/// `helpTextArrayID`.
const HELP_TEXT: i32 = 13;
/// `statics[]` of `mcl_pr_layout.fit`.
const PORTRAIT: usize = 67;
const COVER_LEFT: usize = 27;
const COVER_RIGHT: usize = 28;
/// `textObjects[]`: the RP, the rank, gunnery, piloting, kills, name, the four skills.
const TEXT_CBILLS: usize = 1;
const TEXT_RANK: usize = 4;
const TEXT_GUNNERY: usize = 5;
const TEXT_PILOTING: usize = 6;
const TEXT_KILLS: usize = 7;
const TEXT_NAME: usize = 8;
const TEXT_SKILL: usize = 9;
const TEXT_HELP: usize = 13;
/// The pilot scales (`/80`).
const MAX_SKILL: f32 = 80.0;
/// The panel the mech statistics cover (`GUI_RECT` in `PilotReadyScreen::render`).
const MECH_PANEL: GuiRect = GuiRect {
    left: 77,
    top: 317,
    right: 720,
    bottom: 515,
};
/// Below this height the help text is cleared (`mouseY > 317`).
const HELP_CLEAR_Y: i32 = 317;
/// `MB_MSG_*` and `PR_MSG_*` as the buttons send them.
const PR_MSG_ADD: i32 = ids::PR_MSG_ADD;
const PR_MSG_REMOVE: i32 = ids::PR_MSG_REMOVE;
/// Button indices of the layout (`buttons[1..3]` in the C++).
const BUTTON_NEXT: usize = 1;
const BUTTON_ADD: usize = 2;
const BUTTON_REMOVE: usize = 3;
/// The rank names are `GREEN + rank`, the highest rank is `ACE`.
const ACE_RANK: i32 = 4;

pub struct PilotReady {
    /// The layout (`mcl_pr_layout.fit`).
    pub base: LogisticsScreen,
    /// The pilots not in a mech (`pilotListBox`).
    pub list: PilotList,
    /// The force group's slots (`pIcons`).
    pub icons: Vec<LogisticsMechIcon>,
    /// `attributeMeters[2]`: gunnery and piloting.
    pub meters: [AttributeMeter; 2],
    /// `rankIcons[5]`.
    rank_icons: Vec<Object>,
    /// `medalIcons[16]` and where each medal is drawn (`medalLocations`).
    medal_icons: Vec<Object>,
    medal_locations: Vec<GuiRect>,
    /// `specialtySkillIcons[4]` by skill kind, and the four skill slots drawn from them.
    specialty_icons: Vec<Object>,
    pub skill_locations: Vec<GuiRect>,
    skill_icons: Vec<Object>,
    /// `mechDisplay`.
    pub mech_display: MechDisplay,
    entry_template: PilotEntryTemplate,
    /// The portrait of each pilot, loaded when the room begins.
    portraits: HashMap<usize, (TexId, u32)>,
    /// `pCurPilot`.
    pub cur_pilot: Option<usize>,
    /// The rank of `cur_pilot`, read from the campaign when its record shows (`pCurPilot->getRank()`).
    /// An assigned pilot is off the free list, so the list cannot give it.
    pub cur_rank: Option<i32>,
    /// `mechSelected`: the mech statistics show instead of the pilot's record.
    pub mech_selected: bool,
    /// `pDragPilot`.
    pub drag_pilot: Option<usize>,
    drag_icon: Object,
    /// `forceGroupCount`.
    force_group_count: usize,
}

/// The pilot a slot's mech carries.
fn slot_pilot(lg: &Logistics, icon: &LogisticsMechIcon) -> Option<usize> {
    icon.content
        .mech
        .and_then(|m| lg.mech(m))
        .and_then(|m| m.pilot)
}

impl PilotReady {
    /// `PilotReadyScreen::init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<PilotReady, String> {
        let mut base = LogisticsScreen::load(
            ld,
            "mcl_pr_layout",
            Prefixes {
                statics: Some("PilotReadyStatic"),
                texts: Some("PilotReadyTextEntry"),
                rects: Some("PilotReadyRect"),
                buttons: Some("PilotReadyButton"),
                ..Prefixes::default()
            },
        )?;
        base.help_text_array_id = HELP_TEXT;
        for b in &mut base.buttons {
            b.set_message_on_release();
        }
        let mut file = ld.art_fit("mcl_pr_layout")?;

        let rect = base
            .rect(0)
            .ok_or("mcl_pr_layout.fit has no list rectangle")?;
        let (x, y, w, h) = (
            rect.obj.left(),
            rect.obj.top(),
            rect.obj.width(),
            rect.obj.height(),
        );
        let mut entry_file = ld.art_fit("mcl_pr_availablepilotentry")?;
        let entry_template = PilotEntryTemplate::load(ld, &mut entry_file);
        let list = PilotList::new(ListBox::new(ld, x, y, w, h));

        let mut icon_file = ld.art_fit("mcl_gn_deploymentteams")?;
        let icons = icon_grid(LogisticsMechIcon::load(ld, &mut icon_file));

        let meters = [
            AttributeMeter::from_fit(&mut file, "AttributeMeter0"),
            AttributeMeter::from_fit(&mut file, "AttributeMeter1"),
        ];
        let rank_icons = (0..5)
            .map(|i| Object::from_fit(ld, &mut file, &format!("PilotReadyRankIcon{i}")))
            .collect();
        let medal_icons: Vec<Object> = (0..16)
            .map(|i| Object::from_fit(ld, &mut file, &format!("PilotMedal{i}")))
            .collect();
        let medal_locations = (0..16)
            .map(|i| read_rect(&mut file, &format!("PilotMedalPlacement{i}")))
            .collect();
        let specialty_icons = (0..4)
            .map(|i| Object::from_fit(ld, &mut file, &format!("PilotSpecialtySkillIcon{i}")))
            .collect();
        let skill_locations: Vec<GuiRect> = (0..4)
            .map(|i| read_rect(&mut file, &format!("PilotSpecialtySkill{i}")))
            .collect();
        let skill_icons = (0..4).map(|_| Object::default()).collect();
        let drag_icon = entry_template.drag_icon();

        let mut mech_display = MechDisplay::load(ld)?;
        mech_display.cur = None;

        let mut screen = PilotReady {
            base,
            list,
            icons,
            meters,
            rank_icons,
            medal_icons,
            medal_locations,
            specialty_icons,
            skill_locations,
            skill_icons,
            mech_display,
            entry_template,
            portraits: HashMap::new(),
            cur_pilot: None,
            cur_rank: None,
            mech_selected: false,
            drag_pilot: None,
            drag_icon,
            force_group_count: 0,
        };
        if let Some(b) = screen.base.button_mut(ids::MB_MSG_MAINMENU) {
            b.set_press_fx(-1);
        }
        Ok(screen)
    }

    /// `begin`: the force group is read from the campaign, the pilots not in a mech are listed,
    /// and the first pilot (or the one in the first slot) is shown.
    pub fn begin_screen(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.base.status = Status::Running;
        self.set_button_disabled(ids::MB_MSG_PREV, false);
        self.set_button_disabled(ids::MB_MSG_MAINMENU, false);
        self.force_group_count = 0;
        self.drag_pilot = None;
        self.cur_pilot = None;
        self.cur_rank = None;
        self.mech_selected = false;
        // `PilotReadyScreen::begin`: "update pilot use here... things can get screwed up"
        lg.rebuild_pilot_use();

        // the portraits of the roster, loaded once they are needed
        for i in 0..lg.pilots().len() {
            if self.portraits.contains_key(&i) {
                continue;
            }
            let name = ld
                .assets
                .string(lg.pilots()[i].name_id as u32)
                .to_lowercase();
            let path = format!("{}mcl_pr_{name}.tga", crate::textures::ART_PATH);
            if let Some(tex) = ld.texture(&path) {
                let width = ld.assets.textures.width(tex);
                self.portraits.insert(i, (tex, width));
            }
        }

        let assets: &UiAssets = &*ld.assets;
        for icon in &mut self.icons {
            icon.set_mech(assets, lg, None);
            icon.select(false);
        }
        self.list.remove_all();

        let mut has_pilot: Option<usize> = None;
        let inventory: Vec<(MechKey, i32, Option<usize>)> = lg
            .inventory()
            .iter()
            .map(|m| (m.key, m.force_group, m.pilot))
            .collect();
        for (key, group, pilot) in inventory {
            if group == 0 {
                continue;
            }
            let slot = group as usize - 1;
            if slot >= SLOTS {
                continue;
            }
            self.icons[slot].set_mech(assets, lg, Some(key));
            if pilot.is_some() {
                self.icons[slot].set_pilot(assets, lg);
                has_pilot = Some(slot);
            }
            self.force_group_count += 1;
        }

        for i in 0..lg.pilots().len() {
            let p = &lg.pilots()[i];
            if !p.used && p.is_available() {
                if let Some(e) = PilotEntry::new(&self.entry_template, assets, lg, i) {
                    self.list.add(e);
                }
            }
        }

        match has_pilot {
            Some(slot) => {
                self.icons[slot].select(true);
                let pilot = slot_pilot(lg, &self.icons[slot]);
                self.set_pilot(assets, lg, pilot);
            }
            None => {
                if self.list.item_count() > 0 {
                    self.list.list.select_item(0);
                    let pilot = self.list.entries().next().map(|e| e.pilot);
                    self.set_pilot(assets, lg, pilot);
                }
                self.icons[0].select(true);
            }
        }
    }

    fn set_button_disabled(&mut self, id: i32, disabled: bool) {
        if let Some(b) = self.base.button_mut(id) {
            b.disable(disabled);
        }
    }

    /// `setPilot`: the pilot's record and portrait show (`None` clears them).
    pub fn set_pilot(&mut self, assets: &UiAssets, lg: &Logistics, pilot: Option<usize>) {
        self.mech_selected = false;
        if pilot != self.cur_pilot {
            self.cur_pilot = pilot;
            match pilot.and_then(|p| self.portraits.get(&p).copied()) {
                Some((tex, width)) => {
                    if let Some(portrait) = self.portrait_mut() {
                        portrait.set_texture(Some(tex), width);
                        portrait.set_color(0xffff_ffff, false);
                        portrait.set_uvs(0.0, 0.0, 92.0, 128.0);
                    }
                }
                None if pilot.is_none() => {
                    if let Some(portrait) = self.portrait_mut() {
                        portrait.set_color(0, false);
                    }
                    return;
                }
                None => {}
            }
        }
        self.cur_rank = pilot.and_then(|i| lg.pilots().get(i)).map(|p| p.rank);
        let Some(p) = pilot.and_then(|i| lg.pilots().get(i)) else {
            for i in 4..13 {
                self.set_text(i, "");
            }
            if let Some(portrait) = self.portrait_mut() {
                portrait.set_color(0, false);
            }
            return;
        };

        let rank = if p.rank != ACE_RANK {
            assets.string(strings::GREEN + p.rank as u32)
        } else {
            assets.string(strings::ACE)
        }
        .to_string();
        self.set_text(TEXT_RANK, &rank);
        self.set_text(TEXT_GUNNERY, &(p.gunnery as i64).to_string());
        self.meters[0].set_value(p.gunnery / MAX_SKILL);
        self.set_text(TEXT_PILOTING, &(p.piloting as i64).to_string());
        self.meters[1].set_value(p.piloting / MAX_SKILL);
        self.set_text(TEXT_KILLS, &p.mech_kills.to_string());
        self.set_text(TEXT_NAME, &assets.string(p.name_id as u32).to_uppercase());

        let skills = p.specialty_skill_indices();
        for i in 0..4 {
            match skills.get(i) {
                Some(&skill) => {
                    self.set_text(
                        TEXT_SKILL + i,
                        assets.string(strings::SPECIALTY + skill as u32),
                    );
                    let kind = SpecialtySkill::ALL[skill].kind() as usize;
                    let rect = self.skill_locations[i];
                    let mut icon = self.specialty_icons[kind].clone();
                    icon.resize(
                        (rect.right - rect.left) as f32,
                        (rect.bottom - rect.top) as f32,
                    );
                    icon.move_to(rect.left as f32, rect.top as f32);
                    icon.set_color(0xffff_ffff, false);
                    icon.help_id = strings::SPECIALTY_HELP1 as i32 + skill as i32;
                    self.skill_icons[i] = icon;
                }
                None => {
                    self.set_text(TEXT_SKILL + i, "");
                    self.skill_icons[i].set_color(0, false);
                    self.skill_icons[i].help_id = 0;
                }
            }
        }

        let mut count = 0usize;
        for (i, &earned) in p.medals.iter().enumerate() {
            let Some(icon) = self.medal_icons.get_mut(i) else {
                continue;
            };
            if earned {
                let rect = self.medal_locations[count];
                icon.resize(
                    (rect.right - rect.left) as f32,
                    (rect.bottom - rect.top) as f32,
                );
                icon.move_to(rect.left as f32, rect.top as f32);
                icon.set_color(0xffff_ffff, false);
                icon.show_window(true);
                icon.help_id = strings::HELP_MEDAL1 as i32 + i as i32;
                count += 1;
            } else {
                icon.show_window(false);
            }
        }
    }

    fn portrait_mut(&mut self) -> Option<&mut Object> {
        self.base.statics.get_mut(PORTRAIT)
    }

    fn set_text(&mut self, i: usize, s: &str) {
        if let Some(t) = self.base.texts.get_mut(i) {
            t.set_text(s);
        }
    }

    /// `setMech`: the mech's statistics take the panel.
    pub fn set_mech(&mut self, assets: &UiAssets, lg: &Logistics, mech: Option<MechKey>) {
        self.mech_selected = true;
        self.mech_display.set_mech(assets, lg, mech);
    }

    /// `beginDrag`: a pilot follows the pointer.
    /// The icon that follows the pointer while a pilot is dragged.
    pub fn drag_portrait(&self) -> &Object {
        &self.drag_icon
    }

    /// The portrait is the pilot's own: an assigned pilot is not in the free list.
    pub fn begin_drag(&mut self, lg: &Logistics, pilot: usize) {
        if self.drag_pilot.is_none() {
            self.drag_pilot = Some(pilot);
        }
        if let Some(p) = lg.pilots().get(pilot) {
            set_portrait(&mut self.drag_icon, p.photo_index);
        }
    }

    /// `addSelectedPilot`: the selected pilot goes to the selected slot without a pilot (or the
    /// first slot that has a mech and no pilot); the next pilot in the list then shows.
    pub fn add_selected_pilot(&mut self, assets: &UiAssets, lg: &mut Logistics) {
        let index = self.list.list.selected_item();
        let pilot = usize::try_from(index)
            .ok()
            .and_then(|i| self.list.entries().nth(i))
            .map(|e| e.pilot());
        let Some(pilot) = pilot else {
            return;
        };
        let mut found = false;
        for i in 0..SLOTS {
            let crewable = self.icons[i].is_selected()
                && self.icons[i].content.mech.is_some()
                && slot_pilot(lg, &self.icons[i]).is_none();
            if crewable {
                self.assign(assets, lg, i, Some(pilot));
                self.icons[i].select(false);
                let mut next = i + 1;
                if next >= SLOTS || self.icons[next].content.mech.is_none() {
                    next = 0;
                }
                self.icons[next].select(true);
                found = true;
                break;
            }
        }
        if !found {
            for i in 0..SLOTS {
                if self.icons[i].content.mech.is_some() && slot_pilot(lg, &self.icons[i]).is_none()
                {
                    self.assign(assets, lg, i, Some(pilot));
                    found = true;
                    break;
                }
            }
        }
        if found {
            let next = usize::try_from(index + 1)
                .ok()
                .and_then(|i| self.list.entries().nth(i))
                .map(|e| e.pilot());
            if let Some(next) = next {
                self.set_pilot(assets, lg, Some(next));
            }
        }
    }

    /// `removeSelectedPilot`: the selected slot's pilot goes back to the list, and the next slot
    /// shows.
    pub fn remove_selected_pilot(&mut self, assets: &UiAssets, lg: &mut Logistics) {
        let Some(i) = (0..SLOTS).find(|&i| self.icons[i].is_selected()) else {
            return;
        };
        let Some(pilot) = slot_pilot(lg, &self.icons[i]) else {
            return;
        };
        let old_sel = self.list.list.selected_item();
        let new_index =
            PilotEntry::new(&self.entry_template, assets, lg, pilot).and_then(|e| self.list.add(e));
        self.assign(assets, lg, i, None);
        self.icons[i].select(false);
        let mut next = i + 1;
        if next >= SLOTS || self.icons[next].content.mech.is_none() {
            next = 0;
        }
        self.icons[next].select(true);
        match slot_pilot(lg, &self.icons[next]) {
            Some(p) => {
                if let Some(added) = new_index {
                    if added as i32 <= old_sel {
                        self.list.list.select_item(old_sel + 1);
                    }
                }
                self.set_pilot(assets, lg, Some(p));
            }
            None => {
                if let Some(added) = new_index {
                    self.list.list.select_item(added as i32);
                    let shown = self.list.entries().nth(added).map(|e| e.pilot());
                    if let Some(p) = shown {
                        self.set_pilot(assets, lg, Some(p));
                    }
                }
            }
        }
    }

    /// Puts `pilot` in slot `i` (or takes the slot's pilot out with `None`) in the campaign and on
    /// the slot.
    fn assign(&mut self, assets: &UiAssets, lg: &mut Logistics, i: usize, pilot: Option<usize>) {
        if let Some(m) = self.icons[i].content.mech {
            lg.set_mech_pilot(m, pilot);
            self.icons[i].set_pilot(assets, lg);
        }
    }

    /// `putBackPilot`: a pilot leaves its slot and is listed again.
    fn put_back_pilot(&mut self, assets: &UiAssets, lg: &mut Logistics, pilot: usize) {
        for i in 0..SLOTS {
            if slot_pilot(lg, &self.icons[i]) == Some(pilot) {
                self.assign(assets, lg, i, None);
                self.icons[i].dim_pilot(false);
            }
        }
        if let Some(e) = PilotEntry::new(&self.entry_template, assets, lg, pilot) {
            self.list.add(e);
        }
    }

    /// `endDrag`: the dragged pilot lands on the slot under the pointer, or goes back.
    fn end_drag(&mut self, assets: &UiAssets, lg: &mut Logistics, slot: Option<usize>) {
        let Some(pilot) = self.drag_pilot else {
            return;
        };
        match slot {
            Some(i) => {
                if let Some(old) = slot_pilot(lg, &self.icons[i]) {
                    self.put_back_pilot(assets, lg, old);
                }
                self.assign(assets, lg, i, Some(pilot));
                self.list.remove_pilot(pilot);
            }
            None => self.put_back_pilot(assets, lg, pilot),
        }
        self.drag_pilot = None;
    }

    /// `handleMessage`.
    pub fn handle_message(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics, who: i32) {
        let assets = ctx.assets;
        match who {
            ids::MB_MSG_NEXT => self.base.status = Status::Next,
            ids::MB_MSG_PREV => self.base.status = Status::Previous,
            PR_MSG_ADD => self.add_selected_pilot(assets, lg),
            PR_MSG_REMOVE => self.remove_selected_pilot(assets, lg),
            ids::MB_MSG_MAINMENU => self.base.status = Status::MainMenu,
            _ => {}
        }
    }

    /// `update`.
    pub fn update_screen(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        let assets = ctx.assets;
        // `PilotReadyScreen::render`: at rest, `BETTY_NEW_PILOTS` plays, then the flag clears. The
        // sample goes out in `UiOutput::betty`; the voice channel plays it. `Page::render` has no
        // campaign state, so the check runs here; the sequence updates the current page each frame,
        // as it renders it.
        if !lg.mission_info().is_single_mission() && lg.new_pilots_available() {
            ctx.play_betty(betty::NEW_PILOTS);
            lg.set_new_pilots_acknowledged();
        }
        let prev_enabled = self
            .base
            .button(ids::MB_MSG_PREV)
            .is_some_and(|b| b.is_enabled());
        if !prev_enabled {
            // the room has been left for the launch: nothing updates
            return;
        }
        self.set_text(TEXT_CBILLS, &format!("{} ", lg.c_bills()));

        let mut all_full = true;
        let mut new_sel: Option<usize> = None;
        let mut old_sel: Option<usize> = None;
        let mut has_mech = false;
        let mut new_right: Option<usize> = None;
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        let drag_origin = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        for i in 0..SLOTS {
            self.icons[i].update(ctx);
            if self.icons[i].just_selected {
                new_sel = Some(i);
            } else if self.icons[i].is_selected() {
                old_sel = Some(i);
            }
            let inside = self.icons[i].point_inside(mx, my);
            let mech = self.icons[i].content.mech.is_some();
            if ctx.input.is_left_double_click() && inside && mech {
                self.remove_selected_pilot(assets, lg);
            }
            if ctx.input.is_right_click() && inside && mech {
                new_right = Some(i);
            } else if ctx.input.is_left_drag()
                && inside
                && self.icons[i].point_inside(drag_origin.0, drag_origin.1)
                && mech
            {
                if let Some(p) = slot_pilot(lg, &self.icons[i]) {
                    self.begin_drag(lg, p);
                    self.icons[i].dim_pilot(true);
                    self.assign(assets, lg, i, None);
                }
            }
            if mech && slot_pilot(lg, &self.icons[i]).is_none() {
                all_full = false;
            }
            if mech {
                has_mech = true;
            }
        }

        if let Some(n) = new_sel {
            if let Some(o) = old_sel {
                self.icons[o].select(false);
            }
            match slot_pilot(lg, &self.icons[n]) {
                Some(p) => self.set_pilot(assets, lg, Some(p)),
                None => {
                    let mech = self.icons[n].content.mech;
                    self.set_mech(assets, lg, mech);
                }
            }
        }
        if let Some(r) = new_right {
            let mech = self.icons[r].content.mech;
            self.set_mech(assets, lg, mech);
        }

        let cur_sel = new_sel.or(old_sel);
        let msgs = self.base.update(ctx);
        for m in msgs {
            self.handle_message(ctx, lg, m.who);
        }

        if self.mech_selected {
            if my > HELP_CLEAR_Y {
                if let Some(t) = self.base.texts.get_mut(TEXT_HELP) {
                    t.set_text("");
                }
            }
            self.mech_display.update_display(ctx);
        }

        // NEXT waits for every mech to have a pilot; ADD needs a free slot
        if !all_full {
            self.set_button_index_disabled(BUTTON_NEXT, true);
            let add_off = match cur_sel {
                None => true,
                Some(c) => slot_pilot(lg, &self.icons[c]).is_some(),
            };
            self.set_button_index_disabled(BUTTON_ADD, add_off);
        } else {
            self.set_button_index_disabled(BUTTON_NEXT, !has_mech);
            self.set_button_index_disabled(BUTTON_ADD, true);
        }
        let remove_off = match cur_sel {
            Some(c) if new_sel.is_some() || old_sel.is_some() => {
                slot_pilot(lg, &self.icons[c]).is_none()
            }
            _ => true,
        };
        self.set_button_index_disabled(BUTTON_REMOVE, remove_off);

        if self.drag_pilot.is_some() {
            let (w, h) = (self.drag_icon.width(), self.drag_icon.height());
            self.drag_icon
                .move_to(mx as f32 - w / 2.0, my as f32 - h / 2.0);
            let under = (0..SLOTS).find(|&i| {
                self.icons[i].point_inside(mx, my) && self.icons[i].content.mech.is_some()
            });
            if ctx.input.left_mouse_released() {
                self.end_drag(assets, lg, under);
            }
        }

        let events = self.list.update(ctx, lg);
        for e in events {
            match e {
                PilotEvent::Show(p) => self.set_pilot(assets, lg, Some(p)),
                PilotEvent::Add(_) => {
                    self.add_selected_pilot(assets, lg);
                    ctx.play(sfx::LOG_SELECT);
                }
                PilotEvent::StartDrag(p) => self.begin_drag(lg, p),
            }
        }
    }

    fn set_button_index_disabled(&mut self, index: usize, disabled: bool) {
        if let Some(b) = self.base.buttons.get_mut(index) {
            b.disable(disabled);
        }
    }

    /// `render`: the list, the record or the mech panel, the slots, the drag.
    pub fn render_screen(&mut self, ctx: &mut RenderCtx<'_>) {
        self.list.render(ctx);
        for m in &self.meters {
            m.render(ctx, 0, 0);
        }
        if let Some(rank) = self.cur_rank {
            if let Some(icon) = self.rank_icons.get_mut(rank as usize) {
                icon.render(ctx);
            }
        }
        for icon in &self.skill_icons {
            icon.render(ctx);
        }
        for icon in &self.medal_icons {
            icon.render(ctx);
        }
        self.base.render(ctx);
        for icon in &mut self.icons {
            icon.render(ctx);
        }
        if self.mech_selected {
            ctx.dl.fill_rect(MECH_PANEL, 0xff00_0000);
            self.mech_display.render_display(ctx);
            if let Some(s) = self.base.statics.get_mut(COVER_LEFT) {
                s.render(ctx);
            }
            if let Some(s) = self.base.statics.get_mut(COVER_RIGHT) {
                s.render(ctx);
            }
        }
        if self.drag_pilot.is_some() {
            self.drag_icon.render(ctx);
        }
    }
}

/// A rectangle from a `left/top/right/bottom` block.
fn read_rect(file: &mut mc2_formats::ini::FitIni, block: &str) -> GuiRect {
    if file.seek_block(block).is_err() {
        return GuiRect::new(0, 0, 0, 0);
    }
    GuiRect::new(
        file.long_or("left", 0) as i32,
        file.long_or("top", 0) as i32,
        file.long_or("right", 0) as i32,
        file.long_or("bottom", 0) as i32,
    )
}

impl Page for PilotReady {
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.begin_screen(ld, lg);
    }
    fn end(&mut self, _lg: &mut Logistics) {
        self.mech_display.clear();
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        self.update_screen(ctx, lg);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.render_screen(ctx);
    }
    fn begin_fade_in(&mut self, t: f32) {
        self.base.begin_fade_in(t);
    }
    fn status(&self) -> Status {
        self.base.get_status()
    }
    fn set_status(&mut self, status: Status) {
        self.base.status = status;
    }
    fn base(&self) -> &LogisticsScreen {
        &self.base
    }
    fn base_mut(&mut self) -> &mut LogisticsScreen {
        &mut self.base
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
