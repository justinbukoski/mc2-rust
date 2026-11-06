// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `MissionBriefingScreen` (`code/missionbriefingscreen.cpp`): the mission's name, its objectives
//! (a list, and a marker on the map for each one), the blurb, and the drop zone marker.
//!
//! The list cycles through the objectives every three seconds until the player clicks; the
//! selected objective's marker is drawn red. The objective's 3D model (the C++ `camera`) is drawn in
//! the video panel; an objective without a model, or whose model fails to render, shows the "no
//! model" static. The map picture (`getMissionTGA`) is set on the layout's map static when the screen
//! begins (`load_map`), and the layout draws it.

use super::{ids, strings, text_item, Page};
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::font::FontRef;
use crate::preview::{
    draw_preview, preview_failed, preview_texture, rect_size, shape_name, PreviewKind,
    PreviewRequest, DEFAULT_PAINT,
};
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::textures::TexId;
use crate::widgets::{ListBox, Object, TextListItem};
use mc2_campaign::Logistics;
use mc2_sim_data::mission::{Mission, Objective, ObjectiveModel};

/// `statics[]` of `mcl_mn.fit` the screen uses.
const MAP_INDEX: usize = 32;
const VIDEO_SCREEN: usize = 33;
const BUTTON_TEXT: usize = 34;
/// The static shown when the selected objective has no model.
const NO_MODEL: usize = 35;
/// `RP_INDEX`: the C-Bills text.
const RP_INDEX: usize = 1;
/// `helpTextArrayID`.
const HELP_TEXT: i32 = 4;
/// Colours: list text, headings, the selected objective's marker.
const WHITE: u32 = 0xffff_ffff;
const HEADING: u32 = 0xff00_5392;
const SELECTED: u32 = 0xffff_0000;
/// `BLDG_TYPE` and `TREED_TYPE` (`mclib/daprtype.h:30-31`): the objective model types
/// `SimpleCamera::setObject` draws as a building appearance.
const BLDG_TYPE: i32 = 0x0e;
const TREED_TYPE: i32 = 0x0f;
/// `missionPath`: where the mission files are.
const MISSION_PATH: &str = "data/missions/";
/// `getMissionTGA`: the map picture is packet 3 of the mission's `.pak` (a TGA), with its width.
pub fn load_map(ld: &mut Loader<'_>, name: &str) -> Option<(TexId, u32)> {
    let key = format!("{MISSION_PATH}{name}.pak#3");
    if let Some(id) = ld.assets.textures.find(&key) {
        return Some((id, ld.assets.textures.width(id)));
    }
    let bytes = ld.vfs.read(&format!("{MISSION_PATH}{name}.pak")).ok()??;
    let pak = mc2_formats::pak::PakFile::from_bytes(bytes).ok()?;
    if pak.packet_count() <= 3 {
        return None;
    }
    let tga = pak.read_packet(3).ok()?;
    let img = match mc2_assets::tga::decode(&tga) {
        Ok(img) => img,
        Err(e) => {
            log::warn!("{key}: {e}");
            return None;
        }
    };
    let img = flip_top_to_bottom(&tga, img);
    let width = u32::from(img.width);
    let id = ld
        .assets
        .textures
        .insert(&key, width, u32::from(img.height), img.pixels);
    Some((id, width))
}

/// `flipTopToBottom((BYTE*)(pHeader + 1), ..)` (`missionbriefingscreen.cpp:314`): the C++ reverses the
/// scanlines of the TGA as stored in the file, ignoring the origin bit, and `setUVs(0, 127, 127, 0)`
/// flips them back. `tga::decode` already puts the top row first (it honours the origin bit), which
/// is the file order reversed only for a bottom-origin file; a top-origin file (descriptor bit 5)
/// needs its rows reversed to land where the C++ has them.
fn flip_top_to_bottom(
    tga: &[u8],
    mut img: mc2_assets::tga::RgbaImage,
) -> mc2_assets::tga::RgbaImage {
    if tga.get(17).is_some_and(|d| d & 0x20 != 0) {
        let row = usize::from(img.width) * 4;
        let rows: Vec<&[u8]> = img.pixels.chunks_exact(row).rev().collect();
        img.pixels = rows.concat();
    }
    img
}

/// The terrain extent when the mission gives none (`120 * 128`).
const DEFAULT_EXTENT: f32 = 120.0 * 128.0;

pub struct Briefing {
    /// The briefing layout (`mcl_mn.fit`).
    pub base: LogisticsScreen,
    /// The text list (`missionListBox`).
    pub description: ListBox,
    font: FontRef,
    /// `objectiveButtons`: one marker per objective, index = the objective's list id.
    pub objective_buttons: Vec<Object>,
    /// The `ID` of each list entry: the objective's index, or -1 for text entries.
    list_ids: Vec<i32>,
    /// `objectiveModels`, by objective index.
    pub models: Vec<ObjectiveModel>,
    /// The picture of the selected objective's model in the video panel (`camera`), once asked for.
    pub preview: Option<PreviewRequest>,
    /// The drop zone marker, when the mission has a player part.
    pub drop_zone: Option<Object>,
    run_time: f32,
    clicked: bool,
    /// What stopped the briefing from loading its mission (the sequence reports it as a failure).
    fault: Option<String>,
}

impl Briefing {
    /// `MissionBriefingScreen::init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<Briefing, String> {
        let mut base = LogisticsScreen::load(
            ld,
            "mcl_mn",
            Prefixes {
                statics: Some("Static"),
                texts: Some("Text"),
                rects: Some("Rect"),
                buttons: Some("Button"),
                ..Prefixes::default()
            },
        )?;
        base.help_text_array_id = HELP_TEXT;
        for b in &mut base.buttons {
            b.set_message_on_release();
        }
        let rect = base.rect(1).ok_or("mcl_mn.fit has no list rectangle")?;
        let (x, y, w, h) = (
            rect.obj.left(),
            rect.obj.top(),
            rect.obj.width(),
            rect.obj.height(),
        );
        let font = ld.font(strings::MN_LB_FONT);
        let mut description = ListBox::new(ld, x, y, w, h);
        description.click_sfx = -1;
        description.highlight_sfx = -1;
        description.disabled_sfx = -1;
        for id in [ids::MN_MSG_PLAY, ids::MN_MSG_STOP, ids::MN_MSG_PAUSE] {
            if let Some(b) = base.button_mut(id) {
                b.obj.show_window(false);
            }
        }
        Ok(Briefing {
            base,
            description,
            font,
            objective_buttons: Vec::new(),
            list_ids: Vec::new(),
            models: Vec::new(),
            preview: None,
            drop_zone: None,
            run_time: 0.0,
            clicked: false,
            fault: None,
        })
    }

    /// `begin`: fills the list from the current mission's file.
    pub fn begin_screen(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.description.remove_all_items();
        self.list_ids.clear();
        self.objective_buttons.clear();
        self.models.clear();
        self.drop_zone = None;
        self.clear_model();
        self.run_time = 0.0;
        self.clicked = false;
        self.fault = None;
        self.base.status = Status::Running;
        if let Some(s) = self.base.statics.get_mut(VIDEO_SCREEN) {
            s.set_color(0, true);
        }

        let name = lg.current_mission().to_string();
        let map = load_map(ld, &name);
        if let Some(s) = self.base.statics.get_mut(MAP_INDEX) {
            // a map that did not load is the zero texture, not the last mission's picture
            let (tex, width) = map.map_or((None, 0), |(tex, width)| (Some(tex), width));
            s.set_texture(tex, width);
            s.set_uvs(0.0, 127.0, 127.0, 0.0);
            s.set_color(WHITE, true);
        }
        let assets: &UiAssets = &*ld.assets;
        let path = format!("{MISSION_PATH}{name}.fit");
        let mission = match Mission::load_named(ld.vfs, &path) {
            Ok(m) => m,
            Err(e) => {
                log::error!("briefing: {path}: {e}");
                self.fault = Some(format!("briefing: {path}: {e}"));
                return;
            }
        };

        self.add_string(assets, strings::MN_DIVIDER, HEADING, -1);
        let settings = &mission.settings;
        if settings.mission_name_use_resource_string.unwrap_or(false) {
            if let Some(id) = settings.mission_name_resource_string_id {
                self.add_string(assets, id, HEADING, -1);
            }
        } else {
            let name = settings.mission_name.clone().unwrap_or_default();
            self.add_text(assets, &name, HEADING, -1, true);
        }
        self.add_string(assets, strings::MN_DIVIDER, HEADING, -1);
        self.add_string(assets, strings::MN_MISSION_OBJECTIVES, HEADING, -1);
        self.add_string(assets, strings::MN_DIVIDER, HEADING, -1);

        // the terrain extent scales the markers; a missing or zero extent is the default
        let extent = |v: Option<f32>| match v {
            Some(v) if v != 0.0 => v.abs(),
            _ => DEFAULT_EXTENT,
        };
        let terrain = mission.terrain.as_ref();
        let (ext_x, ext_y) = (
            extent(terrain.and_then(|t| t.terrain_min_x)),
            extent(terrain.and_then(|t| t.terrain_min_y)),
        );

        // the primary objectives, then the secondary ones (`j` is the priority)
        let objectives: Vec<Objective> = mission
            .teams
            .iter()
            .find(|t| t.team == 0)
            .map(|t| t.objectives.clone())
            .unwrap_or_default();
        let mut count = 0i32;
        for priority in 1..3u32 {
            let mut button_count = 0i32;
            for o in &objectives {
                if o.is_hidden_trigger || !o.is_active || o.priority != priority {
                    continue;
                }
                self.add_objective_button(
                    (o.marker_x, o.marker_y),
                    button_count,
                    priority,
                    (ext_x, ext_y),
                    o.display_marker,
                );
                let text = if o.description_use_resource_string {
                    assets.string(o.description_resource_string_id).to_string()
                } else {
                    o.description.clone()
                };
                self.add_text(assets, &text, WHITE, count, true);
                self.models.push(o.model.clone());
                count += 1;
                button_count += 1;
            }
        }

        self.add_string(assets, strings::MN_DIVIDER, HEADING, -1);
        let mut blurb = settings.blurb.clone().unwrap_or_default();
        if settings.blurb_use_resource_string == Some(true) {
            if let Some(id) = settings.blurb_resource_string_id {
                blurb = assets.string(id).to_string();
            }
        }
        self.add_text(assets, &blurb, HEADING, -1, true);

        if let Some(t) = self.base.texts.get_mut(RP_INDEX) {
            t.set_text(&format!("{} ", lg.c_bills()));
        }

        // the designers did not place drop zones: the first player part is it
        if let Some(part) = mission.parts.iter().find(|p| p.player_part) {
            let (fx, fy) = part.position;
            self.setup_drop_zone((fx, fy), (ext_x, ext_y));
        }
        if let Some(s) = self.base.statics.get_mut(BUTTON_TEXT) {
            s.show_window(false);
        }
    }

    /// `addItem(stringID, color, id)`: a text entry taken from the string table.
    fn add_string(&mut self, assets: &UiAssets, string_id: u32, color: u32, id: i32) {
        let mut item = self.entry(assets, color);
        item.set_text_id(assets, string_id);
        self.push(item, id);
    }

    /// `addLBItem(text, color, id)`: a literal text entry, sized to its text when `sized`.
    fn add_text(&mut self, assets: &UiAssets, text: &str, color: u32, id: i32, sized: bool) {
        let mut item = self.entry(assets, color);
        item.set_text(text);
        if sized {
            item.size_to_text(assets);
        }
        item.force_to_top = true;
        self.push(item, id);
    }

    fn entry(&self, assets: &UiAssets, color: u32) -> TextListItem {
        let width = self.description.obj.width() - self.description.scroll_bar_width() - 10.0;
        text_item(assets, self.font, width, "", color)
    }

    fn push(&mut self, item: TextListItem, id: i32) {
        self.description.add_item(Box::new(item));
        self.list_ids.push(id);
    }

    /// The map position of a point of the mission (`addObjectiveButton`, `setupDropZone`):
    /// the map's centre plus the point scaled to half the map's size.
    fn map_point(&self, p: (f32, f32), extent: (f32, f32)) -> Option<(f32, f32)> {
        let map = self.base.statics.get(MAP_INDEX)?;
        let (bmp_x, bmp_y) = (map.width(), map.height());
        let x = p.0 / extent.0 * bmp_x / 2.0 + map.global_x() as f32 + bmp_x / 2.0;
        let y = -p.1 / extent.1 * bmp_y / 2.0 + map.global_y() as f32 + bmp_y / 2.0;
        Some((x, y))
    }

    /// `addObjectiveButton`: a marker on the map, its texture cell chosen by `count` (primary
    /// objectives start 12 cells in).
    fn add_objective_button(
        &mut self,
        marker: (f32, f32),
        count: i32,
        priority: u32,
        extent: (f32, f32),
        display: bool,
    ) {
        let count = if priority == 1 { count + 12 } else { count };
        let Some((x, y)) = self.map_point(marker, extent) else {
            return;
        };
        let Some(template) = self.base.statics.get(BUTTON_TEXT) else {
            return;
        };
        let mut b = template.clone();
        b.show_window(display);
        let (tw, th) = (b.width(), b.height());
        let per_line = ((128.0 / tw) as i32).max(1);
        let (i, j) = (count % per_line, count / per_line);
        b.set_uvs(
            i as f32 * tw,
            j as f32 * th,
            (i + 1) as f32 * tw,
            (j + 1) as f32 * th,
        );
        b.move_to(x, y);
        self.objective_buttons.push(b);
    }

    /// `setupDropZone`: the player's drop point, an 18 by 18 cell of the marker texture.
    fn setup_drop_zone(&mut self, p: (f32, f32), extent: (f32, f32)) {
        let Some((x, y)) = self.map_point(p, extent) else {
            return;
        };
        let Some(template) = self.base.statics.get(BUTTON_TEXT) else {
            return;
        };
        let mut b = template.clone();
        b.resize(18.0, 18.0);
        b.set_uvs(100.0, 100.0, 118.0, 118.0);
        b.move_to(x, y);
        b.show_window(true);
        self.drop_zone = Some(b);
    }

    /// Every list entry and marker back to white.
    fn whiten_all(&mut self) {
        for i in 0..self.list_ids.len() {
            if self.list_ids[i] != -1 {
                self.set_item_color(i, WHITE);
                if let Some(b) = self.objective_buttons.get_mut(self.list_ids[i] as usize) {
                    b.set_color(WHITE, true);
                }
            }
        }
    }

    fn set_item_color(&mut self, i: usize, color: u32) {
        if let Some(item) = self.description.item_mut(i) {
            if let Some(t) = item.as_any_mut().downcast_mut::<TextListItem>() {
                t.set_color(color);
            }
        }
    }

    /// Selects an objective: its entry and marker turn red, and the model area shows the model
    /// or, for an objective without one, its "no model" static.
    fn focus(&mut self, i: usize, id: i32) {
        let model = self.models.get(id as usize).filter(|m| !m.name.is_empty());
        let request = model.map(|m| self.model_request(m));
        self.set_no_model_static(request.is_none());
        self.preview = request;
        self.set_item_color(i, SELECTED);
        if let Some(b) = self.objective_buttons.get_mut(id as usize) {
            b.set_color(SELECTED, true);
        }
    }

    /// `camera.setObject(objectiveModels[ID], modelTypes[ID], ...)` and `setScale`: the model in
    /// the video panel (`statics[VIDEO_SCREEN]`, the `camera.init` rectangle). A `BLDG_TYPE` or
    /// `TREED_TYPE` objective goes through `setBuilding`, i.e. `setComponent`: the building
    /// appearance (`<name>enc.ini` when it exists, else `<name>.ini`), drawn with its own
    /// texture: the `resetPaintScheme` that `setComponent` calls (simplecamera.cpp:495) is the
    /// empty `Appearance` one for a building (appear.h:390-392), and the objective's colours
    /// only reach a mech or a vehicle (simplecamera.cpp:523-554).
    fn model_request(&self, m: &ObjectiveModel) -> PreviewRequest {
        let (width, height) = self.base.statics.get(VIDEO_SCREEN).map_or((1, 1), |r| {
            rect_size((r.left(), r.top(), r.right(), r.bottom()))
        });
        let building = matches!(m.model_type, Some(BLDG_TYPE | TREED_TYPE));
        let paint = if building {
            [0; 3]
        } else {
            [m.base_color.map_or(DEFAULT_PAINT, |c| c as u32); 3]
        };
        PreviewRequest {
            shape: shape_name(&m.name),
            paint,
            scale: m.scale,
            yaw: 0.0,
            width,
            height,
            building,
            kind: if building {
                PreviewKind::Building
            } else {
                PreviewKind::Mech
            },
            activated: false,
        }
    }

    fn set_no_model_static(&mut self, show: bool) {
        if let Some(s) = self.base.statics.get_mut(NO_MODEL) {
            s.show_window(show);
        }
    }

    /// `camera.setMech(NULL)`: no model is shown, and the video panel's fallback is hidden.
    fn clear_model(&mut self) {
        self.preview = None;
        self.set_no_model_static(false);
    }

    /// `update`.
    pub fn update_screen(&mut self, ctx: &mut UpdateCtx<'_>, lg: &mut Logistics) {
        let single = lg.mission_info().is_single_mission();
        if let Some(b) = self.base.button_mut(ids::MB_MSG_PREV) {
            b.disable(single);
        }

        let old_sel = self.description.selected_item();
        self.description.update(ctx);

        if ctx.input.is_left_click() {
            self.clicked = true;
            let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            for k in 0..self.objective_buttons.len() {
                if !self.objective_buttons[k].point_inside(mx, my) {
                    continue;
                }
                if let Some(j) = self.list_ids.iter().position(|&id| id == k as i32) {
                    self.description.select_item(j as i32);
                }
            }
        }

        self.run_time += ctx.dt;
        let sel = self.description.selected_item();
        let id = self.item_id(sel);
        if sel != -1 && old_sel != sel {
            self.clicked = true;
            self.whiten_all();
            if id != -1 {
                self.focus(sel as usize, id);
            }
        }

        // every three seconds the selection moves on, until the player clicks
        if !self.clicked && self.run_time > 3.0 {
            self.run_time = 0.0;
            if sel != -1 && id != -1 {
                self.set_item_color(sel as usize, WHITE);
                if let Some(b) = self.objective_buttons.get_mut(id as usize) {
                    b.set_color(WHITE, true);
                }
            }
            let count = self.description.item_count() as i32;
            let mut next = sel + 1;
            if next >= count {
                next = 0;
            }
            loop {
                if next >= count {
                    next = -1;
                    break;
                } else if self.list_ids[next as usize] == -1 {
                    next += 1;
                } else {
                    break;
                }
            }
            self.description.select_item(next);
            if next != -1 {
                let nid = self.item_id(next);
                if nid != -1 {
                    self.focus(next as usize, nid);
                } else {
                    self.set_no_model_static(true);
                }
            }
        }

        let msgs = self.base.update(ctx);
        for m in msgs {
            self.handle_message(m.who);
        }
    }

    /// The objective index of list entry `i`, or -1 (also for no entry).
    fn item_id(&self, i: i32) -> i32 {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.list_ids.get(i).copied())
            .unwrap_or(-1)
    }

    /// `handleMessage` of the briefing screen.
    fn handle_message(&mut self, who: i32) {
        match who {
            ids::MB_MSG_NEXT => self.base.status = Status::Next,
            ids::MB_MSG_PREV => self.base.status = Status::Previous,
            ids::MB_MSG_MAINMENU => self.base.status = Status::MainMenu,
            _ => {}
        }
    }

    /// Whether the video panel shows its "no model" static.
    pub fn shows_no_model(&self) -> bool {
        self.base
            .statics
            .get(NO_MODEL)
            .is_some_and(|s| s.is_showing())
    }

    /// `render`: the list, the layout, the markers, the drop zone.
    pub fn render_screen(&mut self, ctx: &mut RenderCtx<'_>) {
        // a model that failed to render shows the "no model" static, as an objective without one does
        let failed = self
            .preview
            .as_ref()
            .is_some_and(|r| preview_failed(ctx.assets, r));
        if failed {
            self.set_no_model_static(true);
        }
        self.description.render(ctx);
        // the camera is drawn before the layout, so the screen's fade covers the model
        // (`MissionBriefingScreen::render`, code/missionbriefingscreen.cpp:82-84)
        if let (Some(req), Some(r)) = (&self.preview, self.base.statics.get(VIDEO_SCREEN)) {
            if let Some(tex) = preview_texture(ctx.assets, req) {
                draw_preview(ctx.dl, tex, (r.left(), r.top(), r.right(), r.bottom()));
            }
        }
        self.base.render(ctx);
        for b in &self.objective_buttons {
            b.render(ctx);
        }
        if let Some(d) = &self.drop_zone {
            d.render(ctx);
        }
    }
}

impl Page for Briefing {
    fn begin(&mut self, ld: &mut Loader<'_>, lg: &mut Logistics) {
        self.begin_screen(ld, lg);
    }
    fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }
    fn end(&mut self, _lg: &mut Logistics) {
        self.clear_model();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 1x2 uncompressed 32-bit TGA, first stored row red, second blue.
    fn tga(descriptor: u8) -> Vec<u8> {
        let mut b = vec![
            0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 2, 0, 32, descriptor,
        ];
        b.extend([0, 0, 255, 255]); // BGRA red
        b.extend([255, 0, 0, 255]); // BGRA blue
        b
    }

    fn top_pixel(bytes: &[u8]) -> [u8; 4] {
        let img = mc2_assets::tga::decode(bytes).unwrap();
        let img = flip_top_to_bottom(bytes, img);
        img.pixels[..4].try_into().unwrap()
    }

    /// The C++ texture's first row is the file's last stored row, whatever the origin bit.
    #[test]
    fn first_texture_row_is_the_last_stored_row() {
        let last_row = [0, 0, 255, 255];
        assert_eq!(top_pixel(&tga(0x00)), last_row);
        assert_eq!(top_pixel(&tga(0x20)), last_row);
    }
}
