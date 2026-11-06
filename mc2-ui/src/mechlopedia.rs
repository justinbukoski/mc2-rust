// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The Mech Encyclopedia (`code/mechlopedia.cpp`).  Its category FITs own detail panes;
//! the single group list is the list rectangle of the common `mcl_en.fit` layout.

use crate::ctx::{msg, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::font::FontRef;
use crate::logistics::loadout::{component_rows, loadout_rows};
use crate::preview::{
    draw_preview, preview_failed, preview_texture, rect_size, shape_name, PreviewKind,
    PreviewRequest, DEFAULT_PAINT,
};
use crate::screen::{LogisticsScreen, Status};
use crate::textures::TexId;
use crate::widgets::{ListBox, TextListItem};
use mc2_campaign::{GameData, Logistics, VariantKey};
use mc2_sim_data::component::ComponentForm;
use std::collections::HashMap;
use std::sync::Arc;

const LIST_FONT: u32 = 27711; // IDS_EN_LISTBOX_FONT, the mcl_en_sub row font.
const WEAPON_FONT: u32 = 27726;
const IDS_HOUSE0: u32 = 45131;
const IDS_VEHICLE_CLASS: u32 = 27005;
const IDS_EN_BUILDING_WEIGHT: u32 = 27734;
const IDS_HOTKEY1: u32 = 43200;
const OFFSET: (f32, f32) = (285.0, 58.0);
const DESC: u32 = 0xff00_5392;
const INITIAL_SELECTION: u32 = 0xff86_6234;
const ENCYCLO_MECHS: i32 = 130;
const ENCYCLO_BUILD: i32 = 131;
const ENCYCLO_VEHIC: i32 = 132;
const ENCYCLO_WEAPONS: i32 = 133;
const ENCYCLO_PILOTS: i32 = 134;
const ENCYCLO_HISTORY: i32 = 135;
const ENCYCLO_MM: i32 = 136;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Mechs = 0,
    Buildings = 1,
    Vehicles = 2,
    Weapons = 3,
    Personalities = 4,
    History = 5,
}

trait Screen {
    fn populate(
        &mut self,
        group: &mut ListBox,
        assets: &UiAssets,
        data: &GameData,
        logistics: &Logistics,
    );
    fn select(
        &mut self,
        row: usize,
        assets: &UiAssets,
        data: &GameData,
        logistics: &Logistics,
        unlimited_ammo: bool,
    );
    fn load_visuals(&mut self, _ld: &mut Loader<'_>, _data: &GameData) {}
    /// The host either has an offscreen mech-preview renderer or names that dependency plainly.
    fn set_preview_available(&mut self, _available: bool) {}
    /// `SimpleCamera::setMech` receives the two highlight masks followed by the base mask.
    fn set_preview_paint(&mut self, _paint: [u32; 3]) {}
    fn update(&mut self, ctx: &mut UpdateCtx<'_>);
    fn render(&mut self, ctx: &mut RenderCtx<'_>);
    fn details(&self) -> Vec<String>;
    fn component_details(&self) -> Vec<String> {
        Vec::new()
    }
    fn end(&mut self) {}
}

pub struct Mechlopedia {
    pub base: LogisticsScreen,
    /// C++ `groupListBox`, made from `mcl_en.fit`, not a category rectangle.
    pub group_list: ListBox,
    pub current_screen: Category,
    screens: [Box<dyn Screen>; 6],
    data: Option<Arc<GameData>>,
    logistics: Option<Arc<Logistics>>,
    unlimited_ammo: bool,
}

/// What a row of the mech or vehicle list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MechRow {
    /// A "Prime" designer variant (`getEncyclopediaMechs`).
    Mech(VariantKey),
    /// An `object2.pak` vehicle (`LogisticsVehicle`), by index into `GameData::vehicles`.
    Vehicle(usize),
    /// A helicopter: a designer variant listed with the vehicles (`getHelicopters`).
    Copter(VariantKey),
}

struct MechScreen {
    base: LogisticsScreen,
    description: ListBox,
    stats: ListBox,
    components: ListBox,
    rows: Vec<MechRow>,
    vehicle: bool,
    preview: Option<PreviewRequest>,
    preview_available: bool,
    preview_failed: bool,
    preview_paint: [u32; 3],
    font: FontRef,
}
struct WeaponScreen {
    base: LogisticsScreen,
    description: ListBox,
    stats: ListBox,
    rows: Vec<usize>,
    icons: HashMap<usize, (TexId, u32)>,
    /// `camera.setComponent(getPictureFileName())` and `setScale(1.5)`.
    preview: Option<PreviewRequest>,
    preview_available: bool,
    preview_failed: bool,
    preview_paint: [u32; 3],
    font: FontRef,
}
struct PersonalityScreen {
    base: LogisticsScreen,
    description: ListBox,
    rows: Vec<i32>,
    history: bool,
    font: FontRef,
    portraits: HashMap<i32, (TexId, u32)>,
}
struct BuildingScreen {
    base: LogisticsScreen,
    description: ListBox,
    components: ListBox,
    /// Indices into `GameData::buildings`, in list order.
    rows: Vec<usize>,
    preview: Option<PreviewRequest>,
    preview_available: bool,
    preview_failed: bool,
    preview_paint: [u32; 3],
    font: FontRef,
}

fn detail_box(ld: &mut Loader<'_>, base: &LogisticsScreen, n: usize) -> Result<ListBox, String> {
    let r = base
        .rect(n)
        .ok_or_else(|| format!("category FIT lacks Rect{n}"))?;
    let mut box_ = ListBox::new(ld, r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height());
    box_.move_by(OFFSET.0, OFFSET.1);
    box_.click_sfx = -1;
    box_.highlight_sfx = -1;
    box_.disabled_sfx = -1;
    Ok(box_)
}
fn clear(box_: &mut ListBox) {
    box_.remove_all_items();
}
fn strings(box_: &ListBox) -> Vec<String> {
    box_.items
        .iter()
        .filter_map(|i| i.text().map(str::to_owned))
        .collect()
}
fn add(box_: &mut ListBox, assets: &UiAssets, font: FontRef, text: &str, colour: u32) {
    let mut item = TextListItem::new(assets, font);
    item.set_text(text);
    item.set_color(colour);
    item.size_to_text(assets);
    item.obj.resize(
        box_.obj.width() - box_.scroll_bar_width() - 18.0,
        item.obj.height(),
    );
    let _ = box_.add_item(Box::new(item));
}
fn add_first(box_: &mut ListBox, assets: &UiAssets, font: FontRef, text: &str, colour: u32) {
    let mut item = TextListItem::new(assets, font);
    item.set_text(text);
    item.set_color(colour);
    item.size_to_text(assets);
    item.obj.resize(
        box_.obj.width() - box_.scroll_bar_width() - 18.0,
        item.obj.height(),
    );
    let _ = box_.insert_item(Box::new(item), 0);
}
fn header(base: &mut LogisticsScreen, text: &str) {
    if let Some(t) = base.texts.get_mut(0) {
        t.set_text(&text.to_uppercase());
    }
}
fn tint_initial_selection(group: &mut ListBox) {
    if let Some(item) = group.item_mut(0) {
        if let Some(text) = item.as_any_mut().downcast_mut::<TextListItem>() {
            text.set_color(INITIAL_SELECTION);
        }
    }
}

impl Mechlopedia {
    pub fn load(ld: &mut Loader<'_>) -> Result<Self, String> {
        let mut base = LogisticsScreen::load(ld, "mcl_en", Default::default())
            .map_err(|e| format!("mcl_en.fit: {e}"))?;
        base.help_text_array_id = 12;
        for b in &mut base.buttons {
            b.set_message_on_release();
        }
        let r = base
            .rect(0)
            .ok_or("mcl_en.fit has no group list rectangle")?;
        let mut group_list =
            ListBox::new(ld, r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height());
        group_list.set_orange();
        group_list.click_sfx = -1;
        group_list.highlight_sfx = -1;
        group_list.disabled_sfx = -1;
        let font = ld.font(LIST_FONT);
        let screens: [Box<dyn Screen>; 6] = [
            Box::new(MechScreen::load(ld, false, font).map_err(|e| format!("mechs: {e}"))?),
            Box::new(BuildingScreen::load(ld, font).map_err(|e| format!("buildings: {e}"))?),
            Box::new(MechScreen::load(ld, true, font).map_err(|e| format!("vehicles: {e}"))?),
            Box::new(WeaponScreen::load(ld, font).map_err(|e| format!("weapons: {e}"))?),
            Box::new(
                PersonalityScreen::load(ld, false, font)
                    .map_err(|e| format!("personalities: {e}"))?,
            ),
            Box::new(PersonalityScreen::load(ld, true, font).map_err(|e| format!("history: {e}"))?),
        ];
        Ok(Self {
            base,
            group_list,
            current_screen: Category::Mechs,
            screens,
            data: None,
            logistics: None,
            unlimited_ammo: true,
        })
    }
    pub fn begin(&mut self, assets: &UiAssets, data: &GameData, logistics: &Logistics) {
        self.data = Some(Arc::new(data.clone()));
        self.logistics = Some(Arc::new(logistics.clone()));
        self.base.status = Status::Running;
        self.activate(Category::Mechs, assets);
    }
    /// Preload the category art whose names are shipped in the component table and strings.
    /// This is separate from `begin` because loading a FIT owns the VFS while `begin` only needs
    /// immutable game data.
    pub fn load_visuals(&mut self, ld: &mut Loader<'_>) {
        let Some(data) = self.data.as_deref() else {
            return;
        };
        for screen in &mut self.screens {
            screen.load_visuals(ld, data);
        }
    }
    /// The C++ hides ammunition when the player's Unlimited Ammo preference is enabled.
    pub fn set_unlimited_ammo(&mut self, enabled: bool, assets: &UiAssets) {
        if self.unlimited_ammo == enabled {
            return;
        }
        self.unlimited_ammo = enabled;
        let selected = self.group_list.selected_item();
        if selected >= 0 {
            let data = self
                .data
                .as_deref()
                .expect("Mechlopedia::begin before selection");
            let logistics = self
                .logistics
                .as_deref()
                .expect("Mechlopedia::begin before selection");
            self.screens[self.current_screen as usize].select(
                selected as usize,
                assets,
                data,
                logistics,
                self.unlimited_ammo,
            );
        }
    }
    /// Applies `options.cfg`'s base and highlight colours to the selected chassis preview.
    pub fn set_preview_paint(&mut self, base: u32, highlight: u32, assets: &UiAssets) {
        let paint = [highlight, highlight, base];
        for screen in &mut self.screens {
            screen.set_preview_paint(paint);
        }
        self.refresh_selection(assets);
    }
    /// A failed host preview initialization must not leave a blank, unexplained model panel.
    pub fn set_preview_available(&mut self, available: bool, assets: &UiAssets) {
        for screen in &mut self.screens {
            screen.set_preview_available(available);
        }
        self.refresh_selection(assets);
    }
    /// Selects a category for deterministic headless rendering. Player interaction still reaches
    /// this through the category buttons in [`Self::update`].
    pub fn select_category(&mut self, category: Category, assets: &UiAssets) {
        self.activate(category, assets);
    }
    fn activate(&mut self, cat: Category, assets: &UiAssets) {
        self.current_screen = cat;
        let data = self
            .data
            .as_deref()
            .expect("Mechlopedia::begin before category activation");
        let logistics = self
            .logistics
            .as_deref()
            .expect("Mechlopedia::begin before category activation");
        let screen = &mut self.screens[cat as usize];
        screen.populate(&mut self.group_list, assets, data, logistics);
        if self.group_list.item_count() != 0 {
            self.group_list.select_item(0);
            // Each C++ category `begin` colours its first selected entry amber.
            tint_initial_selection(&mut self.group_list);
            screen.select(0, assets, data, logistics, self.unlimited_ammo);
        }
        self.group_list.set_scroll_pos(0);
    }
    fn refresh_selection(&mut self, assets: &UiAssets) {
        let selected = self.group_list.selected_item();
        if selected < 0 {
            return;
        }
        let data = self
            .data
            .as_deref()
            .expect("Mechlopedia::begin before selection");
        let logistics = self
            .logistics
            .as_deref()
            .expect("Mechlopedia::begin before selection");
        self.screens[self.current_screen as usize].select(
            selected as usize,
            assets,
            data,
            logistics,
            self.unlimited_ammo,
        );
    }
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        for m in self.base.update(ctx) {
            if m.msg == msg::LEFTMOUSEDOWN {
                self.handle(m.who, ctx.assets);
            }
        }
        let before = self.group_list.selected_item();
        self.group_list.update(ctx);
        // `aListBox::update` normally makes this selection. Keep the common group list's
        // hit-test authoritative even on the first frame a newly populated row becomes visible.
        // This avoids the old port's reset-to-row-zero behaviour without bypassing list-box
        // clipping, scrolling, or row state.
        if ctx.input.is_left_click()
            && self
                .group_list
                .point_inside(ctx.input.mouse_x(), ctx.input.mouse_y())
        {
            let (x, y) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            if let Some(index) = self
                .group_list
                .items
                .iter()
                .position(|item| item.obj().show && item.point_inside(x, y))
            {
                self.group_list.select_item(index as i32);
            }
        }
        let selected = self.group_list.selected_item();
        if ctx.input.is_left_click() && selected >= 0 && selected != before {
            let data = self
                .data
                .as_deref()
                .expect("Mechlopedia::begin before selection");
            let logistics = self
                .logistics
                .as_deref()
                .expect("Mechlopedia::begin before selection");
            self.screens[self.current_screen as usize].select(
                selected as usize,
                ctx.assets,
                data,
                logistics,
                self.unlimited_ammo,
            );
        }
        self.screens[self.current_screen as usize].update(ctx);
    }
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.group_list.render(ctx);
        self.screens[self.current_screen as usize].render(ctx);
        self.base.render(ctx);
    }
    fn handle(&mut self, who: i32, assets: &UiAssets) {
        for id in ENCYCLO_MECHS..ENCYCLO_MM {
            if let Some(b) = self.base.button_mut(id) {
                b.press(false);
            }
        }
        if who == ENCYCLO_MM {
            self.screens[self.current_screen as usize].end();
            self.base.status = Status::Next;
            return;
        }
        let Some(cat) = Self::category_from_button_id(who) else {
            return;
        };
        if let Some(b) = self.base.button_mut(who) {
            b.press(true);
        }
        self.screens[self.current_screen as usize].end();
        self.activate(cat, assets);
    }
    pub fn button_id(cat: Category) -> i32 {
        match cat {
            Category::Mechs => ENCYCLO_MECHS,
            Category::Buildings => ENCYCLO_BUILD,
            Category::Vehicles => ENCYCLO_VEHIC,
            Category::Weapons => ENCYCLO_WEAPONS,
            Category::Personalities => ENCYCLO_PILOTS,
            Category::History => ENCYCLO_HISTORY,
        }
    }
    pub fn category_from_button_id(id: i32) -> Option<Category> {
        match id {
            ENCYCLO_MECHS => Some(Category::Mechs),
            ENCYCLO_BUILD => Some(Category::Buildings),
            ENCYCLO_VEHIC => Some(Category::Vehicles),
            ENCYCLO_WEAPONS => Some(Category::Weapons),
            ENCYCLO_PILOTS => Some(Category::Personalities),
            ENCYCLO_HISTORY => Some(Category::History),
            _ => None,
        }
    }
    /// Evidence-only inspection of what a player can read after scripted input.
    pub fn group_text(&self) -> Vec<String> {
        strings(&self.group_list)
    }
    pub fn detail_text(&self) -> Vec<String> {
        self.screens[self.current_screen as usize].details()
    }
    /// The selected mech's component-list rows, kept separate for shipped-data evidence.
    pub fn component_text(&self) -> Vec<String> {
        self.screens[self.current_screen as usize].component_details()
    }
}

/// `S_stricmp`: the order the list insertions use.
fn stricmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase())
}

impl MechScreen {
    /// `MechScreen::begin` for the vehicle list (`mechlopedia.cpp:278-362`): the `object2.pak`
    /// vehicles, equal names gathered by the swap loop, then the helicopters, each inserted before
    /// the first row whose label is alphabetically after its name.
    fn populate_vehicles(
        &mut self,
        group: &mut ListBox,
        assets: &UiAssets,
        data: &GameData,
        logistics: &Logistics,
    ) {
        let mut order: Vec<usize> = (0..data.vehicles.len()).collect();
        for i in 1..order.len() {
            let cur = order[i];
            if let Some(j) =
                (0..i).find(|&j| data.vehicles[order[j]].name_id == data.vehicles[cur].name_id)
            {
                order[i] = order[j];
                order[j] = cur;
            }
        }
        let mut entries: Vec<(String, MechRow)> = Vec::new();
        let mut insert = |name: String, row: MechRow| {
            let at = entries
                .iter()
                .position(|(label, _)| stricmp(&name, label).is_lt())
                .unwrap_or(entries.len());
            entries.insert(at, (name.to_uppercase(), row));
        };
        for index in order {
            insert(
                assets
                    .string(data.vehicles[index].name_id as u32)
                    .to_string(),
                MechRow::Vehicle(index),
            );
        }
        for key in logistics.helicopters() {
            if let Some(v) = logistics.variant(key) {
                insert(v.name.clone(), MechRow::Copter(key));
            }
        }
        for (label, row) in entries {
            add(group, assets, self.font, &label, 0xffff_ffff);
            self.rows.push(row);
        }
    }

    /// `MechScreen::setVehicle` (`mechlopedia.cpp:450-537`).
    fn select_vehicle(
        &mut self,
        assets: &UiAssets,
        data: &GameData,
        vehicle: &mc2_campaign::data::VehicleRecord,
    ) {
        header(&mut self.base, assets.string(vehicle.name_id as u32));
        if !self.preview_available {
            add(
                &mut self.description,
                assets,
                self.font,
                "MECH MODEL NOT AVAILABLE (PREVIEW RENDERER UNAVAILABLE)",
                DESC,
            );
        }
        add(
            &mut self.description,
            assets,
            self.font,
            assets.string(vehicle.encyclopedia_id as u32),
            DESC,
        );
        // the vehicle's weight class is the fixed `IDS_VEHICLE_CLASS`
        for text in [
            format!("WEIGHT : {:.0} tons", vehicle.max_weight),
            format!("CLASS : {}", assets.string(IDS_VEHICLE_CLASS)),
            format!(
                "ARMOR: {} : {}",
                assets.string(vehicle.armor_class_string_id()),
                vehicle.base_armor
            ),
            format!("SPEED : {} Km/h", vehicle.display_speed()),
        ] {
            add(&mut self.stats, assets, self.font, &text, 0xffff_ffff);
        }
        for component in component_rows(assets, &data.catalog, &vehicle.components) {
            add(
                &mut self.components,
                assets,
                self.font,
                &component.text,
                component.colour,
            );
        }
        self.request_preview(&vehicle.file_name, vehicle.scale);
    }

    /// `SimpleCamera::setMech`/`setVehicle` into the model rectangle.
    fn request_preview(&mut self, file_name: &str, scale: f32) {
        if !self.preview_available {
            return;
        }
        if let Some(panel) = self.base.statics.get(4) {
            let (w, h) = rect_size((
                panel.left() + OFFSET.0,
                panel.top() + OFFSET.1,
                panel.right() + OFFSET.0,
                panel.bottom() + OFFSET.1,
            ));
            self.preview = Some(PreviewRequest {
                shape: shape_name(file_name),
                paint: self.preview_paint,
                scale,
                yaw: 0.0,
                width: w,
                height: h,
                building: false,
                kind: PreviewKind::Mech,
                activated: false,
            });
        }
    }

    fn load(ld: &mut Loader<'_>, vehicle: bool, font: FontRef) -> Result<Self, String> {
        let mut base = LogisticsScreen::load(ld, "mcl_en_mechs", Default::default())
            .map_err(|e| format!("mcl_en_mechs.fit: {e}"))?;
        base.help_text_array_id = 12;
        // `MechScreen::init` hides this FIT placeholder (`statics[4].setColor(0)`) before
        // `SimpleCamera` draws its model into the same rectangle.
        if let Some(panel) = base.statics.get_mut(4) {
            panel.set_color(0, false);
        }
        Ok(Self {
            description: detail_box(ld, &base, 0)?,
            stats: detail_box(ld, &base, 1)?,
            components: detail_box(ld, &base, 2)?,
            base,
            rows: Vec::new(),
            vehicle,
            preview: None,
            preview_available: true,
            preview_failed: false,
            preview_paint: [DEFAULT_PAINT; 3],
            font,
        })
    }
}
impl Screen for MechScreen {
    fn populate(
        &mut self,
        group: &mut ListBox,
        assets: &UiAssets,
        data: &GameData,
        logistics: &Logistics,
    ) {
        clear(group);
        clear(&mut self.description);
        clear(&mut self.stats);
        clear(&mut self.components);
        self.rows.clear();
        self.preview = None;
        self.preview_failed = false;
        if self.vehicle {
            self.populate_vehicles(group, assets, data, logistics);
            return;
        }
        let mut rows = logistics.encyclopedia_mechs();
        // `MechScreen::begin` inserts its visible chassis labels with `S_stricmp` ordering.
        // Sort by that same label, rather than by the private variant filename/name.
        rows.sort_by_cached_key(|key| {
            logistics
                .variant(*key)
                .and_then(|v| logistics.data().chassis.get(v.chassis))
                .map(|c| assets.string(c.chassis_name_id as u32).to_ascii_lowercase())
                .unwrap_or_default()
        });
        for key in rows {
            if let Some(v) = logistics.variant(key) {
                if let Some(c) = logistics.data().chassis.get(v.chassis) {
                    add(
                        group,
                        assets,
                        self.font,
                        &assets.string(c.chassis_name_id as u32).to_uppercase(),
                        0xffff_ffff,
                    );
                    self.rows.push(MechRow::Mech(key));
                }
            }
        }
    }
    fn select(
        &mut self,
        row: usize,
        assets: &UiAssets,
        data: &GameData,
        logistics: &Logistics,
        _unlimited_ammo: bool,
    ) {
        clear(&mut self.description);
        clear(&mut self.stats);
        clear(&mut self.components);
        self.preview_failed = false;
        let Some(&picked) = self.rows.get(row) else {
            return;
        };
        let (key, show_house_and_jump) = match picked {
            MechRow::Vehicle(index) => {
                if let Some(vehicle) = data.vehicles.get(index) {
                    self.select_vehicle(assets, data, vehicle);
                }
                return;
            }
            MechRow::Mech(key) => (key, true),
            MechRow::Copter(key) => (key, false),
        };
        let Some(v) = logistics.variant(key) else {
            return;
        };
        let Some(c) = data.chassis.get(v.chassis) else {
            return;
        };
        header(&mut self.base, assets.string(c.chassis_name_id as u32));
        if !self.preview_available {
            add(
                &mut self.description,
                assets,
                self.font,
                "MECH MODEL NOT AVAILABLE (PREVIEW RENDERER UNAVAILABLE)",
                DESC,
            );
        }
        add(
            &mut self.description,
            assets,
            self.font,
            assets.string(c.encyclopedia_id as u32),
            DESC,
        );
        let variant_ctx = logistics.ctx_for(v);
        if show_house_and_jump {
            add(
                &mut self.stats,
                assets,
                self.font,
                &format!("HOUSE : {}", assets.string(IDS_HOUSE0 + c.house_id as u32)),
                0xffff_ffff,
            );
        }
        add(
            &mut self.stats,
            assets,
            self.font,
            &format!("WEIGHT : {:.0} tons", c.max_weight),
            0xffff_ffff,
        );
        add(
            &mut self.stats,
            assets,
            self.font,
            &format!("CLASS : {}", assets.string(c.weight_class().string_id())),
            0xffff_ffff,
        );
        add(
            &mut self.stats,
            assets,
            self.font,
            &format!(
                "ARMOR: {} : {}",
                assets.string(c.armor_class_string_id()),
                v.armor(&variant_ctx)
            ),
            0xffff_ffff,
        );
        add(
            &mut self.stats,
            assets,
            self.font,
            &format!("SPEED : {} Km/h", c.display_speed()),
            0xffff_ffff,
        );
        if show_house_and_jump {
            add(
                &mut self.stats,
                assets,
                self.font,
                &format!("JUMP : {} m", v.jump_range(&variant_ctx) * 25),
                0xffff_ffff,
            );
        }
        for component in loadout_rows(assets, logistics, key) {
            add(
                &mut self.components,
                assets,
                self.font,
                &component.text,
                component.colour,
            );
        }
        self.request_preview(&c.file_name, c.scale);
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.description.update(ctx);
        self.stats.update(ctx);
        self.components.update(ctx);
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if let (Some(r), Some(p)) = (&self.preview, self.base.statics.get(4)) {
            if let Some(t) = preview_texture(ctx.assets, r) {
                draw_preview(
                    ctx.dl,
                    t,
                    (
                        p.left() + OFFSET.0,
                        p.top() + OFFSET.1,
                        p.right() + OFFSET.0,
                        p.bottom() + OFFSET.1,
                    ),
                );
            } else if preview_failed(ctx.assets, r) && !self.preview_failed {
                // ModelPreviews records a per-shape decode/render error after this screen has
                // selected the chassis.  Put the fallback at the top of the visible pane, like
                // the unavailable vehicle and building categories, rather than leaving it blank.
                add_first(
                    &mut self.description,
                    ctx.assets,
                    self.font,
                    "MECH MODEL NOT AVAILABLE (PREVIEW RENDER FAILED)",
                    DESC,
                );
                self.preview_failed = true;
            }
        }
        self.description.render(ctx);
        self.stats.render(ctx);
        self.components.render(ctx);
        self.base.render_offset(ctx, 285, 58);
    }
    fn set_preview_available(&mut self, available: bool) {
        self.preview_available = available;
        self.preview_failed = false;
        if !available {
            self.preview = None;
        }
    }
    fn set_preview_paint(&mut self, paint: [u32; 3]) {
        self.preview_paint = paint;
    }
    fn details(&self) -> Vec<String> {
        [
            strings(&self.description),
            strings(&self.stats),
            strings(&self.components),
        ]
        .concat()
    }
    fn component_details(&self) -> Vec<String> {
        strings(&self.components)
    }
    fn end(&mut self) {
        self.preview = None;
    }
}

impl WeaponScreen {
    fn load(ld: &mut Loader<'_>, _list_font: FontRef) -> Result<Self, String> {
        let mut base = LogisticsScreen::load(ld, "mcl_en_wep", Default::default())
            .map_err(|e| format!("mcl_en_wep.fit: {e}"))?;
        base.help_text_array_id = 12;
        // `WeaponScreen::init` uses this rectangle for `SimpleCamera` and hides the FIT art.
        if let Some(panel) = base.statics.get_mut(4) {
            panel.set_color(0, false);
        }
        let font = ld.font(WEAPON_FONT);
        Ok(Self {
            description: detail_box(ld, &base, 0)?,
            stats: detail_box(ld, &base, 2)?,
            base,
            rows: Vec::new(),
            icons: HashMap::new(),
            preview: None,
            preview_available: true,
            preview_failed: false,
            preview_paint: [DEFAULT_PAINT; 3],
            font,
        })
    }
}
impl Screen for WeaponScreen {
    fn populate(
        &mut self,
        group: &mut ListBox,
        assets: &UiAssets,
        data: &GameData,
        _logistics: &Logistics,
    ) {
        clear(group);
        clear(&mut self.description);
        clear(&mut self.stats);
        self.rows = (0..data.catalog.len()).collect();
        self.rows.sort_by_cached_key(|i| {
            data.catalog
                .get(*i)
                .map(|c| assets.string(c.string_id as u32).to_ascii_lowercase())
                .unwrap_or_default()
        });
        for &i in &self.rows {
            if let Some(c) = data.catalog.get(i) {
                add(
                    group,
                    assets,
                    self.font,
                    &assets.string(c.string_id as u32).to_uppercase(),
                    0xffff_ffff,
                );
            }
        }
    }
    fn select(
        &mut self,
        row: usize,
        assets: &UiAssets,
        data: &GameData,
        _logistics: &Logistics,
        unlimited_ammo: bool,
    ) {
        clear(&mut self.description);
        clear(&mut self.stats);
        let Some(c) = self.rows.get(row).and_then(|i| data.catalog.get(*i)) else {
            return;
        };
        header(&mut self.base, assets.string(c.string_id as u32));
        self.preview = None;
        self.preview_failed = false;
        if let (Some(file), true, Some(panel)) = (
            c.picture_file.as_deref(),
            self.preview_available,
            self.base.statics.get(4),
        ) {
            let (width, height) = rect_size((
                panel.left() + OFFSET.0,
                panel.top() + OFFSET.1,
                panel.right() + OFFSET.0,
                panel.bottom() + OFFSET.1,
            ));
            self.preview = Some(PreviewRequest {
                shape: shape_name(file),
                paint: self.preview_paint,
                scale: 1.5,
                yaw: 0.0,
                width,
                height,
                building: true,
                kind: PreviewKind::Component,
                activated: false,
            });
        }
        if c.picture_file.is_some() && !self.preview_available {
            add(
                &mut self.description,
                assets,
                self.font,
                "COMPONENT MODEL NOT AVAILABLE (PREVIEW RENDERER UNAVAILABLE)",
                DESC,
            );
        }
        add(
            &mut self.description,
            assets,
            self.font,
            assets.string(c.help_string_id as u32),
            DESC,
        );
        if c.is_weapon() {
            add(
                &mut self.stats,
                assets,
                self.font,
                &format!(
                    "RANGE : {}",
                    assets.string(IDS_HOTKEY1 + c.range_type as u32)
                ),
                0xffff_ffff,
            );
            add(
                &mut self.stats,
                assets,
                self.font,
                &format!("DAMAGE : {:.0}", c.damage),
                0xffff_ffff,
            );
        } else if c.form == ComponentForm::Bulk {
            add(
                &mut self.stats,
                assets,
                self.font,
                "ARMOR : 32",
                0xffff_ffff,
            );
        }
        if c.recycle_time != 0.0 {
            add(
                &mut self.stats,
                assets,
                self.font,
                &format!("RATE OF FIRE: {:.6}", 10.0 / c.recycle_time),
                0xffff_ffff,
            );
        }
        add(
            &mut self.stats,
            assets,
            self.font,
            &format!("HEAT : {:.0}", c.heat),
            0xffff_ffff,
        );
        if !unlimited_ammo && c.ammo != 0 {
            add(
                &mut self.stats,
                assets,
                self.font,
                &format!("AMMO : {}", c.ammo),
                0xffff_ffff,
            );
        }
        add(
            &mut self.stats,
            assets,
            self.font,
            &format!("COST: {}", c.cost),
            0xffff_ffff,
        );
        let icon_centre = self.base.rect(1).map(|rect| {
            (
                (rect.obj.left() + rect.obj.right()) * 0.5,
                (rect.obj.top() + rect.obj.bottom()) * 0.5,
            )
        });
        if let Some(icon) = self.base.statics.get_mut(9) {
            match self.icons.get(&self.rows[row]).copied() {
                Some((texture, width)) => {
                    // `WeaponScreen::setWeapon` resizes this static from the selected
                    // component's grid footprint, then re-centres it in Rect1.
                    let (centre_x, centre_y) = icon_centre.unwrap_or((
                        icon.left() + icon.width() * 0.5,
                        icon.top() + icon.height() * 0.5,
                    ));
                    let (icon_width, icon_height) = (c.width as f32 * 48.0, c.height as f32 * 32.0);
                    icon.set_texture(Some(texture), width);
                    icon.resize(icon_width, icon_height);
                    icon.set_uvs(0.0, 0.0, c.width as f32 * 48.0, c.height as f32 * 32.0);
                    icon.move_to(centre_x - icon_width * 0.5, centre_y - icon_height * 0.5);
                    icon.set_color(0xffff_ffff, false);
                }
                None => icon.set_color(0, false),
            }
        }
    }
    fn load_visuals(&mut self, ld: &mut Loader<'_>, data: &GameData) {
        self.icons.clear();
        for i in 0..data.catalog.len() {
            let Some(component) = data.catalog.get(i) else {
                continue;
            };
            let Some(texture) = ld.art(&component.icon_file) else {
                continue;
            };
            self.icons
                .insert(i, (texture, ld.assets.textures.width(texture)));
        }
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.description.update(ctx);
        self.stats.update(ctx)
    }
    fn set_preview_available(&mut self, available: bool) {
        self.preview_available = available;
        self.preview_failed = false;
        if !available {
            self.preview = None;
        }
    }
    fn set_preview_paint(&mut self, paint: [u32; 3]) {
        self.preview_paint = paint;
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.description.render(ctx);
        self.stats.render(ctx);
        self.base.render_offset(ctx, 285, 58);
        // `camera.render()` follows `LogisticsScreen::render(285, 58)`.
        if let (Some(r), Some(p)) = (&self.preview, self.base.statics.get(4)) {
            if let Some(t) = preview_texture(ctx.assets, r) {
                draw_preview(
                    ctx.dl,
                    t,
                    (
                        p.left() + OFFSET.0,
                        p.top() + OFFSET.1,
                        p.right() + OFFSET.0,
                        p.bottom() + OFFSET.1,
                    ),
                );
            } else if preview_failed(ctx.assets, r) && !self.preview_failed {
                log::warn!("component picture {} did not render", r.shape);
                add_first(
                    &mut self.description,
                    ctx.assets,
                    self.font,
                    "COMPONENT MODEL NOT AVAILABLE (PREVIEW RENDER FAILED)",
                    DESC,
                );
                self.preview_failed = true;
            }
        }
    }
    fn end(&mut self) {
        self.preview = None;
    }
    fn details(&self) -> Vec<String> {
        [strings(&self.description), strings(&self.stats)].concat()
    }
}

impl PersonalityScreen {
    fn load(ld: &mut Loader<'_>, history: bool, font: FontRef) -> Result<Self, String> {
        let mut base = LogisticsScreen::load(ld, "mcl_en_person", Default::default())
            .map_err(|e| format!("mcl_en_person.fit: {e}"))?;
        base.help_text_array_id = 12;
        let description = detail_box(ld, &base, 0)?;
        Ok(Self {
            base,
            description,
            rows: Vec::new(),
            history,
            font,
            portraits: HashMap::new(),
        })
    }
}
impl Screen for PersonalityScreen {
    fn populate(&mut self, group: &mut ListBox, assets: &UiAssets, _: &GameData, _: &Logistics) {
        clear(group);
        clear(&mut self.description);
        let (first, count) = if self.history {
            (45360, 5)
        } else {
            (45300, 11)
        };
        self.rows = (0..count).collect();
        if !self.history {
            self.rows
                .sort_by_cached_key(|i| assets.string((first + *i) as u32).to_ascii_lowercase())
        }
        for &i in &self.rows {
            add(
                group,
                assets,
                self.font,
                &assets.string((first + i) as u32).to_uppercase(),
                0xffff_ffff,
            )
        }
    }
    fn select(
        &mut self,
        row: usize,
        assets: &UiAssets,
        _: &GameData,
        _: &Logistics,
        _unlimited_ammo: bool,
    ) {
        clear(&mut self.description);
        let Some(&i) = self.rows.get(row) else { return };
        let (name, desc) = if self.history {
            (45360 + i, 45371 + i)
        } else {
            (45300 + i, 45316 + i)
        };
        header(&mut self.base, assets.string(name as u32));
        add(
            &mut self.description,
            assets,
            self.font,
            assets.string(desc as u32),
            DESC,
        );
        if let Some(picture) = self.base.statics.get_mut(4) {
            match self.portraits.get(&i).copied() {
                Some((texture, width)) => {
                    picture.set_texture(Some(texture), width);
                    picture.set_uvs(0.0, 0.0, picture.width(), picture.height());
                    picture.set_color(0xffff_ffff, false);
                }
                None => picture.set_color(0, false),
            }
        }
    }
    fn load_visuals(&mut self, ld: &mut Loader<'_>, _data: &GameData) {
        self.portraits.clear();
        let (first, count) = if self.history {
            (45381, 5)
        } else {
            (45340, 11)
        };
        for i in 0..count {
            let name = ld.assets.string((first + i) as u32).to_owned();
            let Some(texture) = ld.art(&name) else {
                continue;
            };
            self.portraits
                .insert(i, (texture, ld.assets.textures.width(texture)));
        }
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.description.update(ctx)
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.description.render(ctx);
        self.base.render_offset(ctx, 285, 58)
    }
    fn details(&self) -> Vec<String> {
        strings(&self.description)
    }
}

impl BuildingScreen {
    fn load(ld: &mut Loader<'_>, font: FontRef) -> Result<Self, String> {
        let mut base = LogisticsScreen::load(ld, "mcl_en_bldg", Default::default())
            .map_err(|e| format!("mcl_en_bldg.fit: {e}"))?;
        base.help_text_array_id = 12;
        // `BuildingScreen::init` hides the placeholder before the camera draws into its rectangle
        if let Some(panel) = base.statics.get_mut(4) {
            panel.set_color(0, false);
        }
        if let Some(t) = base.texts.get_mut(0) {
            t.set_text("");
        }
        let description = detail_box(ld, &base, 1)?;
        let components = detail_box(ld, &base, 2)?;
        Ok(Self {
            base,
            description,
            components,
            rows: Vec::new(),
            preview: None,
            preview_available: true,
            preview_failed: false,
            preview_paint: [DEFAULT_PAINT; 3],
            font,
        })
    }

    /// The label of a building: its name in capitals without "LIAO " (`BuildingScreen::begin`).
    fn label(assets: &UiAssets, name_id: i64) -> String {
        assets
            .string(name_id as u32)
            .to_uppercase()
            .replace("LIAO ", "")
    }
}
impl Screen for BuildingScreen {
    fn populate(&mut self, group: &mut ListBox, assets: &UiAssets, data: &GameData, _: &Logistics) {
        clear(group);
        clear(&mut self.description);
        clear(&mut self.components);
        self.rows.clear();
        self.preview = None;
        // each label goes before the first row that sorts after it (`S_stricmp`)
        let mut entries: Vec<(String, usize)> = Vec::new();
        for (index, building) in data.buildings.iter().enumerate() {
            let label = Self::label(assets, building.name_id);
            let at = entries
                .iter()
                .position(|(other, _)| stricmp(&label, other).is_lt())
                .unwrap_or(entries.len());
            entries.insert(at, (label, index));
        }
        for (label, index) in entries {
            add(group, assets, self.font, &label, 0xffff_ffff);
            self.rows.push(index);
        }
    }
    fn select(
        &mut self,
        row: usize,
        assets: &UiAssets,
        data: &GameData,
        _: &Logistics,
        _unlimited_ammo: bool,
    ) {
        clear(&mut self.description);
        clear(&mut self.components);
        self.preview_failed = false;
        // `getBuilding(nameID)`: the first building with the row's name
        let Some(&listed) = self.rows.get(row) else {
            return;
        };
        let Some(listed) = data.buildings.get(listed) else {
            return;
        };
        let Some(building) = data.buildings.iter().find(|b| b.name_id == listed.name_id) else {
            return;
        };
        header(&mut self.base, &Self::label(assets, building.name_id));
        if let Some(t) = self.base.texts.get_mut(1) {
            t.set_text(
                &assets
                    .string(IDS_EN_BUILDING_WEIGHT)
                    .replace("%ld", &building.weight.to_string()),
            );
        }
        if !self.preview_available {
            add(
                &mut self.description,
                assets,
                self.font,
                "BUILDING MODEL NOT AVAILABLE (PREVIEW RENDERER UNAVAILABLE)",
                DESC,
            );
        }
        add(
            &mut self.description,
            assets,
            self.font,
            assets.string(building.encyclopedia_id as u32),
            DESC,
        );
        let weapons: Vec<usize> = building
            .component_ids
            .iter()
            .filter(|&&id| id != 0)
            .filter_map(|&id| data.catalog.find(id as i32))
            .collect();
        for component in component_rows(assets, &data.catalog, &weapons) {
            add(
                &mut self.components,
                assets,
                self.font,
                &component.text,
                component.colour,
            );
        }
        if self.preview_available {
            if let Some(panel) = self.base.statics.get(4) {
                let (w, h) = rect_size((
                    panel.left() + OFFSET.0,
                    panel.top() + OFFSET.1,
                    panel.right() + OFFSET.0,
                    panel.bottom() + OFFSET.1,
                ));
                self.preview = Some(PreviewRequest {
                    shape: shape_name(&building.file_name),
                    paint: self.preview_paint,
                    scale: building.scale,
                    yaw: 0.0,
                    width: w,
                    height: h,
                    building: false,
                    kind: PreviewKind::Building,
                    activated: false,
                });
            }
        }
    }
    fn set_preview_available(&mut self, available: bool) {
        self.preview_available = available;
        self.preview_failed = false;
        if !available {
            self.preview = None;
        }
    }
    fn set_preview_paint(&mut self, paint: [u32; 3]) {
        self.preview_paint = paint;
    }
    fn update(&mut self, ctx: &mut UpdateCtx<'_>) {
        self.description.update(ctx);
        self.components.update(ctx)
    }
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if let (Some(r), Some(p)) = (&self.preview, self.base.statics.get(4)) {
            if let Some(t) = preview_texture(ctx.assets, r) {
                draw_preview(
                    ctx.dl,
                    t,
                    (
                        p.left() + OFFSET.0,
                        p.top() + OFFSET.1,
                        p.right() + OFFSET.0,
                        p.bottom() + OFFSET.1,
                    ),
                );
            } else if preview_failed(ctx.assets, r) && !self.preview_failed {
                add_first(
                    &mut self.description,
                    ctx.assets,
                    self.font,
                    "BUILDING MODEL NOT AVAILABLE (PREVIEW RENDER FAILED)",
                    DESC,
                );
                self.preview_failed = true;
            }
        }
        self.description.render(ctx);
        self.components.render(ctx);
        self.base.render_offset(ctx, 285, 58)
    }
    fn details(&self) -> Vec<String> {
        let weight = self
            .base
            .texts
            .get(1)
            .map(|t| t.text.clone())
            .unwrap_or_default();
        [
            vec![weight],
            strings(&self.description),
            strings(&self.components),
        ]
        .concat()
    }
    fn end(&mut self) {
        self.preview = None;
    }
}
