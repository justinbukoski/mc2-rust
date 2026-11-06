// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mech lab (`code/mechlabscreen.cpp`): the loadout editor for one mech, over the `mcl_mc.fit`
//! layout.
//!
//! The rules live in `mc2-campaign` (`Logistics::lab_*`, `LogisticsVariant`): what may be added
//! and where, what may be removed, what it costs, which variants may be replaced. This screen
//! sequences them as the original does: the form buttons pick the component list, Add and Remove
//! change the diagram, Accept runs the cost check and then the save or accept dialog, Cancel
//! drops the edit. The campaign state is passed in; the screen keeps what the lab shows.
//!
//! The picture panel (`rects[4]`) is `SimpleCamera`'s: it spins the picked component's 3D model
//! (`setComponent(getPictureFileName())`, 90 degrees a second), drawn from pictures that
//! [`crate::preview::ModelPreviews`] renders. The attribute meters (`AttributeMeter0..8`) show the
//! picked component's figures, the heat and armor numbers count up when a part changes them
//! (`updateHeatMeter`, `updateArmorMeter`), the diagram and the drag draw each component's icon
//! art, and the ammo row follows `useUnlimitedAmmo`.

use crate::component_row::{ComponentRow, RowTemplate};
use crate::ctx::{sfx, Loader, Msg, RenderCtx, UpdateCtx};
use crate::logistics::attribute_meter::AttributeMeter;
use crate::logistics::mech_display::MAX_JUMP_RANGE;
use crate::logistics_dialog::{OneButtonDialog, VariantDialog, VariantDialogKind};
use crate::logistics_ids::{button, form, ids};
use crate::preview::{rect_size, PreviewKind, PreviewRequest, Spinner};
use crate::screen::{LogisticsScreen, Prefixes, Status};
use crate::textures::TexId;
use crate::widgets::Object;
use crate::widgets::{ComboBox, ItemState, ListBox, ListItem};
use mc2_campaign::variant::{Ctx, PlacedComponent, Variant};
use mc2_campaign::{Logistics, LogisticsError, VariantKey};
use mc2_sim_data::component::logistics::{WeaponRangeType, COMPONENT_FORMS};
use mc2_sim_data::component::ComponentForm;
use std::collections::HashMap;

/// Pixels per lab grid cell (`LogisticsComponent::XICON_FACTOR`, `YICON_FACTOR`).
pub const XICON_FACTOR: f32 = 48.0;
pub const YICON_FACTOR: f32 = 32.0;

/// The number of attribute meters of the layout (`MECH_LAB_ATTRIBUTE_METER_COUNT`).
pub const ATTRIBUTE_METER_COUNT: usize = 9;
/// `LogisticsComponent::MAX_HEAT`, `MAX_DAMAGE`, `MAX_RECYCLE` (`logisticscomponent.cpp:22-24`).
const MAX_HEAT: f32 = 20.0;
const MAX_DAMAGE: f32 = 15.0;
const MAX_RECYCLE: f32 = 10.75;
/// The ammo meter's full scale (`mechlabscreen.cpp:1320`).
const MAX_AMMO: f32 = 164.0;
/// `resetPaintScheme(0xffff7e00, 0xffff7e00, 0xffbcbcbc)`, the paint of a component model
/// (`SimpleCamera::setComponent`'s building appearance).
const COMPONENT_PAINT: [u32; 3] = [0xffff_7e00, 0xffff_7e00, 0xffbc_bcbc];

/// `IDS_MC_COMPONENT_TOO_HOT`, `IDS_MC_COMPONENT_TOO_MUCH_ARMOR`.
const IDS_MC_COMPONENT_TOO_HOT: u32 = 27334;
const IDS_MC_COMPONENT_TOO_MUCH_ARMOR: u32 = 27335;

/// `helpTextArrayID` of the lab: the text slot the help string is shown in.
const HELP_TEXT: i32 = 14;

/// A rectangle of the layout in screen pixels (`GUI_RECT`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Area {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

impl Area {
    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

/// Which dialog is open over the lab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Open {
    None,
    Save,
    Accept,
    Error,
}

impl Open {
    fn is_dialog(self) -> bool {
        self != Open::None
    }
}

/// The mech lab.
pub struct MechLab {
    pub screen: LogisticsScreen,
    /// `variantList`: the variant drop-down over `ComboBox0` (`mcl_mc_combobox0.fit`).
    pub variant_combo: ComboBox,
    /// `componentListBox`: the components of the chosen form, over `Rect2`.
    pub components: ListBox,
    pub save: VariantDialog,
    pub accept: VariantDialog,
    pub error: OneButtonDialog,
    /// The status the screen reports (running, back to the mech bay, main menu).
    pub status: Status,
    open: Open,
    /// The dialog that has answered and is sliding out.
    closing: Open,
    /// The status to report once the answered dialog has slid out (the screen change waits).
    after_leave: Option<Status>,
    /// `pVariant`: the variant being edited while the lab is in use.
    edited: Option<VariantKey>,
    /// `originalCost`: the cost of the mech as it came into the lab.
    original_cost: i32,
    /// `varName`: the name the combo box holds.
    var_name: String,
    /// The form button pressed (`COMPONENT_FORM_*`).
    form: i32,
    /// Catalogue index of each row of the component list.
    rows: Vec<usize>,
    /// `selI`, `selJ`: the origin cell of the placed component picked in the diagram.
    selected_cell: Option<(i64, i64)>,
    /// `pCurComponent`: the catalogue component the info panel was last drawn for, or `Some(None)`
    /// for an empty panel. `None` means nothing is drawn since the last reset, so the next refresh
    /// draws the pick, even when that is no component (the old text must go).
    shown: Option<Option<usize>>,
    /// `rects[13]`: the diagram.
    diagram: Area,
    /// `rects[6]`: the jump jet slot.
    jump_slot: Area,
    /// `ComponentListItem::s_templateItem`.
    row_template: RowTemplate,
    /// `IDS_MC_COMPONENT_TOO_HOT` and `IDS_MC_COMPONENT_TOO_MUCH_ARMOR`: why a row is greyed out.
    too_hot: String,
    too_much_armor: String,
    /// `attributeMeters`.
    pub meters: [AttributeMeter; ATTRIBUTE_METER_COUNT],
    /// `payloadIcon`: the slot art drawn under the diagram's cells.
    payload: Object,
    /// `rects[4]`: `camera`'s panel.
    camera: Area,
    /// `camera`: the turn of the model in the panel.
    pub spin: Spinner,
    /// The model `camera` shows (`setComponent`), none after `setMech(NULL)`.
    pub model: Option<PreviewRequest>,
    /// The icon art of each catalogue component: texture, the file's width and the size in cells.
    icons: HashMap<usize, (TexId, u32, i32, i32)>,
    /// `useUnlimitedAmmo`.
    unlimited_ammo: bool,
    /// `oldHeat`, `heatTime`, `flashHeatRange`: the heat count-up.
    heat_anim: CountUp,
    /// `oldArmor`, `armorTime`: the armor count-up.
    armor_anim: CountUp,
    /// The component dragged out of the list, and where the pointer is (`dragIcon`).
    drag: Option<(usize, f32, f32)>,
}

/// The count-up of a figure when a part changes it (`updateHeatMeter`, `updateArmorMeter`): the
/// number runs for .6 s, then flashes until 1.6 s.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CountUp {
    old: i32,
    /// Seconds into the animation; zero when it is not running (`heatTime`).
    time: f32,
    /// `flashHeatRange`: the flash hides the denominator instead of the meter.
    flash_range: bool,
}

/// What a figure shows this frame: the number, the meter's fraction (before dividing by the
/// maximum) and whether the "/max" is shown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CountShown {
    pub number: i32,
    pub meter: f32,
    pub show_denominator: bool,
}

impl CountUp {
    /// Starts the animation from `old` (`heatTime = .0001f`).
    pub fn start(&mut self, old: i32, flash_range: bool) {
        self.old = old;
        self.flash_range = flash_range;
        self.time = 0.0001;
    }

    /// Whether the animation is running.
    pub fn running(&self) -> bool {
        self.time != 0.0
    }

    /// One frame of `updateHeatMeter`/`updateArmorMeter` for the figure whose real value is
    /// `cur`. `heat` is true for the heat figure, whose count-up also has the range flash.
    pub fn step(&mut self, cur: i32, dt: f32, heat: bool) -> CountShown {
        let mut number = cur;
        let mut meter = cur as f32;
        let mut show_denominator = true;
        if self.time != 0.0 {
            if self.time < 0.6 {
                if self.old >= cur {
                    number = (self.old as f32 - (self.old - cur) as f32 * self.time / 0.6) as i32;
                } else {
                    number = (self.old as f32 * self.time / 0.6) as i32;
                }
                meter = number as f32;
                if heat && self.flash_range {
                    number = cur;
                }
                self.time += dt;
            } else if self.time > 1.6 || self.old >= cur {
                self.time = 0.0;
            } else {
                let flash = self.time - 0.6;
                if flash < 0.25 || (flash > 0.5 && flash < 0.75) {
                } else if heat && self.flash_range {
                    show_denominator = false;
                } else {
                    meter = self.old as f32;
                }
                self.time += dt;
            }
        }
        CountShown {
            number,
            meter,
            show_denominator,
        }
    }
}

impl MechLab {
    /// `MechLabScreen::init`: the layout, the variant combo box and the dialogs.
    pub fn load(ld: &mut Loader<'_>) -> Result<MechLab, String> {
        let mut screen = LogisticsScreen::load(ld, "mcl_mc", Prefixes::default())?;
        screen.help_text_array_id = HELP_TEXT;
        // `MechLabScreen::init`: the form buttons play the video sounds, the others act on release
        for b in &mut screen.buttons {
            if matches!(
                b.id(),
                form::WEAPON_ENERGY | form::WEAPON_BALLISTIC | form::WEAPON_MISSILE | form::JUMPJET
            ) {
                b.set_press_fx(sfx::LOG_VIDEOBUTTONS);
                b.set_highlight_fx(sfx::LOG_DIGITALHIGHLIGHT);
            } else {
                b.set_message_on_release();
                b.set_press_fx(sfx::LOG_CLICKONBUTTON);
            }
        }
        let mut combo_file = ld.art_fit("mcl_mc_combobox0")?;
        let variant_combo = ComboBox::from_fit(ld, &mut combo_file, "VariantComboBox");
        let mut layout = ld.art_fit("mcl_mc")?;
        let meters = std::array::from_fn(|i| {
            AttributeMeter::from_fit(&mut layout, &format!("AttributeMeter{i}"))
        });
        let payload = Object::from_fit(ld, &mut layout, "PayloadUnit");
        let row_template = RowTemplate::load(ld, &mut layout);
        let list = area_of(&screen, 2);
        let components = ListBox::new(
            ld,
            list.left,
            list.top,
            list.right - list.left,
            list.bottom - list.top,
        );
        Ok(MechLab {
            save: VariantDialog::load(ld, VariantDialogKind::Save)?,
            accept: VariantDialog::load(ld, VariantDialogKind::Accept)?,
            error: OneButtonDialog::load(ld)?,
            diagram: area_of(&screen, 13),
            camera: area_of(&screen, 4),
            meters,
            payload,
            spin: Spinner::default(),
            model: None,
            icons: HashMap::new(),
            unlimited_ammo: true,
            heat_anim: CountUp::default(),
            armor_anim: CountUp::default(),
            drag: None,
            jump_slot: area_of(&screen, 6),
            row_template,
            too_hot: String::new(),
            too_much_armor: String::new(),
            screen,
            variant_combo,
            components,
            status: Status::Running,
            open: Open::None,
            closing: Open::None,
            after_leave: None,
            edited: None,
            original_cost: 0,
            var_name: String::new(),
            form: form::WEAPON_ENERGY,
            rows: Vec::new(),
            selected_cell: None,
            shown: None,
        })
    }

    /// The variant being edited, while the lab is in use.
    pub fn edited(&self) -> Option<VariantKey> {
        self.edited
    }

    /// Loads the art the lab draws from files: the chassis' paper doll (`MechLabScreen::begin`:
    /// `MCL_MC_<mech>_A.tga` on `statics[50]`, `_B` on `statics[51..54]`) and, the first time, the
    /// icon art of every component of the catalogue (`artPath + getIconFileName()`). Run before
    /// [`MechLab::begin`], with the mech to edit chosen.
    pub fn load_art(&mut self, ld: &mut Loader<'_>, logistics: &Logistics) {
        if let Some(chassis) = logistics
            .variant_being_edited()
            .and_then(|key| logistics.chassis_of(key))
        {
            let mech = crate::preview::shape_name(&chassis.file_name);
            for (slots, suffix) in [(50..51, 'a'), (51..54, 'b')] {
                let tex = ld.art_keyed(&format!("mcl_mc_{mech}_{suffix}"));
                let width = tex.map_or(0, |t| ld.assets.textures.width(t));
                for i in slots {
                    if let Some(s) = self.screen.statics.get_mut(i) {
                        s.set_texture(tex, width);
                    }
                }
            }
        }
        if !self.icons.is_empty() {
            return;
        }
        let mut index = 0;
        while let Some(c) = logistics.data().catalog.get(index) {
            if let Some(tex) = ld.art(&c.icon_file) {
                let width = ld.assets.textures.width(tex);
                self.icons
                    .insert(index, (tex, width, c.width.max(1), c.height.max(1)));
            }
            index += 1;
        }
    }

    /// `useUnlimitedAmmo`: the ammo row and its meter hide when the player has unlimited ammo.
    pub fn set_unlimited_ammo(&mut self, unlimited: bool) {
        if self.unlimited_ammo != unlimited {
            self.unlimited_ammo = unlimited;
            self.shown = None;
        }
    }

    /// Whether the lab hides the ammo row.
    pub fn unlimited_ammo(&self) -> bool {
        self.unlimited_ammo
    }

    /// Whether a dialog is open over the lab.
    pub fn dialog_open(&self) -> bool {
        self.open != Open::None
    }

    /// `MechLabScreen::begin`: takes the mech the mech bay chose (`setMechToModify` ran there).
    /// The drop-down lists the chassis' variants whose components are all available, with the
    /// edited one selected; the ballistic weapons form is shown, with the first placed component
    /// picked (else the first list row).
    pub fn begin(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics) {
        self.status = Status::Running;
        self.open = Open::None;
        self.closing = Open::None;
        self.after_leave = None;
        self.selected_cell = None;
        self.shown = None;
        self.too_hot = ctx.assets.string(IDS_MC_COMPONENT_TOO_HOT).to_string();
        self.too_much_armor = ctx
            .assets
            .string(IDS_MC_COMPONENT_TOO_MUCH_ARMOR)
            .to_string();
        self.model = None;
        self.drag = None;
        self.heat_anim = CountUp::default();
        self.armor_anim = CountUp::default();
        self.screen.begin();
        let Some(key) = logistics.variant_being_edited() else {
            self.edited = None;
            return;
        };
        self.edited = Some(key);
        self.original_cost = logistics.variant_cost(key).unwrap_or(0);
        let Some(v) = logistics.variant(key) else {
            return;
        };
        let name = v.name.clone();
        let chassis = v.chassis;

        self.variant_combo.list.remove_all_items();
        let mut selected = None;
        let mut added = 0;
        for other in logistics.chassis_variants(chassis) {
            if !logistics.variant_all_components_available(other) {
                continue;
            }
            let Some(o) = logistics.variant(other) else {
                continue;
            };
            let index = self.variant_combo.add_text(&o.name, 0xffff_ffff);
            if o.name.eq_ignore_ascii_case(&name) {
                selected = index;
            }
            added += 1;
        }
        if added == 0 {
            // a mech the player has no access to yet: only its own name is listed
            selected = self.variant_combo.add_text(&name, 0xffff_ffff);
        }
        if let Some(i) = selected {
            self.variant_combo.select_item(i as i32);
        }
        self.variant_combo.entry.set_entry(ctx.assets, &name, true);
        self.var_name = name;

        if let Some(t) = self.screen.texts.get_mut(3) {
            let chassis_name = logistics.chassis_of(key).map_or(0, |c| c.chassis_name_id);
            t.set_text(ctx.assets.string(chassis_name as u32));
        }
        // the jump jet tab only appears for chassis that can carry jump jets
        let jump_allowed = logistics
            .chassis_of(key)
            .is_some_and(|c| c.can_have_jump_jets);
        if let Some(s) = self.screen.statics.get_mut(54) {
            s.show_window(jump_allowed);
        }
        // `updateDiagram`, then `selectFirstDiagramComponent`, else `selectFirstLBComponent`
        self.set_form(ctx, logistics, form::WEAPON_BALLISTIC);
        self.select_first_diagram_component(logistics);
        if self.selected_cell.is_none() {
            self.select_first_list_component(logistics);
        }
    }

    /// `MechLabScreen::end`.
    pub fn end(&mut self) {
        // `camera.setMech(NULL)`
        self.model = None;
        self.spin.reset();
        self.edited = None;
        self.open = Open::None;
    }

    /// `MechLabScreen::update`: an open dialog first; otherwise the screen's buttons, the
    /// drop-down, the list and the diagram, then the enables and the numbers.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        // a dialog that has answered keeps sliding out (`LeaveAnim`) while the lab goes on
        self.tick_closing(ctx, logistics);
        if let Some(status) = self.after_leave {
            if self.closing == Open::None {
                self.after_leave = None;
                self.status = status;
            }
            return;
        }
        match self.open {
            Open::Save | Open::Accept => {
                let dialog = if self.open == Open::Save {
                    &mut self.save
                } else {
                    &mut self.accept
                };
                dialog.update(ctx, logistics);
                if !dialog.is_open() {
                    let answer = dialog.status;
                    let name = dialog.name.clone();
                    self.closing = self.open;
                    self.open = Open::None;
                    if answer == Status::Yes {
                        // `acceptMechModifications(getFileName())`; the cost was checked before
                        // the dialog opened, and the campaign refuses names it may not replace.
                        let _ = logistics.accept_mech_modifications(&name);
                        self.after_leave = Some(Status::Up);
                        self.edited = None;
                    }
                }
                return;
            }
            Open::Error => {
                if self.error.update(ctx).is_some() {
                    self.closing = Open::Error;
                    self.open = Open::None;
                }
                return;
            }
            Open::None => {}
        }
        if self.edited.is_none() || self.status != Status::Running {
            return;
        }
        // `camera.update()`
        self.spin.update(ctx.dt);

        for m in self.screen.update(ctx) {
            self.handle(ctx, logistics, m);
            if self.status != Status::Running || self.open != Open::None {
                return;
            }
        }

        // the drop-down: a different name loads that variant's loadout under this name
        let _ = self.variant_combo.update(ctx);
        let entered = self.variant_combo.entry.entry();
        if entered != self.var_name {
            self.swap_variant(logistics, &entered);
        }

        let before = self.components.selected_item();
        self.components.update(ctx);
        if self.components.selected_item() != before && self.components.selected_item() != -1 {
            // a list pick replaces the diagram pick (`pSelectedComponent = NULL`)
            self.selected_cell = None;
        }
        self.pick_in_diagram(ctx, logistics);
        self.track_drag(ctx);
        self.drop_from_list(ctx, logistics);
        self.refresh(ctx, logistics);
    }

    /// Runs the leave slide of the dialog that just answered, until it is off the screen.
    fn tick_closing(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        if self.closing == self.open && self.open.is_dialog() {
            self.closing = Open::None;
        }
        match self.closing {
            Open::Save => {
                self.save.update(ctx, logistics);
                if !self.save.is_leaving() {
                    self.closing = Open::None;
                }
            }
            Open::Accept => {
                self.accept.update(ctx, logistics);
                if !self.accept.is_leaving() {
                    self.closing = Open::None;
                }
            }
            Open::Error => {
                let _ = self.error.update(ctx);
                if !self.error.is_leaving() {
                    self.closing = Open::None;
                }
            }
            Open::None => {}
        }
    }

    /// `handleMessage` for the screen's buttons.
    fn handle(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics, m: Msg) {
        match m.who {
            button::MB_MSG_NEXT => self.accept_pressed(ctx, logistics),
            button::MB_MSG_PREV => {
                // `cancelMechModfications`: the mech goes back to its own loadout
                let _ = logistics.cancel_mech_modifications();
                self.edited = None;
                self.status = Status::Up;
            }
            button::MB_MSG_MAINMENU => self.status = Status::MainMenu,
            button::MB_MSG_ADD => self.add_selected(logistics),
            button::MB_MSG_REMOVE => self.remove_selected(ctx, logistics),
            id @ (form::WEAPON_ENERGY
            | form::WEAPON_BALLISTIC
            | form::WEAPON_MISSILE
            | form::JUMPJET) => {
                self.set_form(ctx, logistics, id);
            }
            _ => {}
        }
    }

    /// The Accept button (`MB_MSG_NEXT`): the cost check first; then, when the drop-down's
    /// variant is the same loadout, the accept goes straight through; otherwise the save dialog,
    /// or the accept dialog when some component is not available yet.
    fn accept_pressed(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        if logistics.lab_can_afford().is_err() {
            self.error.set_text(
                ctx.assets.string(ids::MC_INSUFFICIENT_CBILLS as u32),
                ids::DIALOG_OK,
            );
            self.error.begin();
            self.open = Open::Error;
            return;
        }
        let name = self.var_name.clone();
        if logistics.lab_matches_variant(&name) {
            let _ = logistics.accept_mech_modifications_use_old_variant(&name);
            self.status = Status::Up;
            self.edited = None;
            return;
        }
        let Some(edited) = self.edited else { return };
        let named_incomplete = logistics
            .variant_by_name(&name)
            .is_some_and(|k| !logistics.variant_all_components_available(k));
        if named_incomplete || !logistics.variant_all_components_available(edited) {
            self.accept.begin(ctx, logistics);
            self.open = Open::Accept;
        } else {
            self.save.begin(ctx, logistics);
            self.open = Open::Save;
        }
    }

    /// `swapVariant`: the drop-down names another variant; the edit takes its loadout and keeps
    /// its own name.
    fn swap_variant(&mut self, logistics: &mut Logistics, name: &str) {
        self.var_name = name.to_string();
        if let Some(src) = logistics.variant_by_name(name) {
            let _ = logistics.lab_copy_loadout_from(src);
        }
        self.selected_cell = None;
        self.shown = None;
        self.select_first_diagram_component(logistics);
        if self.selected_cell.is_none() {
            self.select_first_list_component(logistics);
        }
    }

    /// `setType(form)` on the list box: the components of the form that the player has, with
    /// the rows the lab cannot take now greyed out. A jump jet tab also lists bulk and heat sinks
    /// (`setType(JUMPJET, BULK, HEATSINK)`).
    fn set_form(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics, form_id: i32) {
        self.form = form_id;
        for id in [
            form::WEAPON_ENERGY,
            form::WEAPON_BALLISTIC,
            form::WEAPON_MISSILE,
            form::JUMPJET,
        ] {
            if let Some(b) = self.screen.button_mut(id) {
                b.press(id == form_id);
            }
        }
        // `setType`: `scrollBar->setGreen()`.
        if let Some(s) = self.components.scroll_bar.as_mut() {
            s.set_green();
        }
        self.components.remove_all_items();
        self.rows.clear();
        let forms: &[i32] = if form_id == form::JUMPJET {
            &[form::JUMPJET, form::BULK, form::HEATSINK]
        } else {
            std::slice::from_ref(&form_id)
        };
        // `ComponentIconListBox::addSortedItem`: the larger grid areas first, then by name
        let mut sorted: Vec<(usize, String, i32)> = Vec::new();
        for index in logistics.available_components() {
            let Some(c) = logistics.data().catalog.get(index) else {
                continue;
            };
            if !forms.contains(&component_form_id(c.form)) {
                continue;
            }
            let name = ctx.assets.string(c.string_id as u32).to_string();
            sorted.push((index, name, c.width * c.height));
        }
        sorted.sort_by(|a, b| {
            b.2.cmp(&a.2)
                .then_with(|| a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
        });
        for (index, name, _) in sorted {
            let Some(c) = logistics.data().catalog.get(index) else {
                continue;
            };
            let icon = self
                .icons
                .get(&index)
                .map(|&(tex, width, _, _)| (tex, width));
            let row = self.row_template.row(
                index,
                &name,
                i64::from(c.cost),
                c.heat,
                icon,
                (c.width.max(1), c.height.max(1)),
            );
            self.components.add_item(Box::new(row));
            self.rows.push(index);
        }
        self.selected_cell = None;
        self.shown = None;
        self.select_first_list_component(logistics);
    }

    /// `ComponentListBox::update`: each row's legality is asked again, since a part added or
    /// removed changes what the lab takes. A row the lab cannot take now is greyed out; one it
    /// can take again is enabled.
    fn refresh_rows(&mut self, logistics: &Logistics) {
        for (i, &index) in self.rows.iter().enumerate() {
            let (mut x, mut y) = (-1, -1);
            let why = logistics.lab_can_add_component(index, &mut x, &mut y);
            let disabled = why.is_err();
            let Some(item) = self.components.item_mut(i) else {
                continue;
            };
            let was_disabled = item.state() == ItemState::Disabled;
            let Some(row) = item.as_any_mut().downcast_mut::<ComponentRow>() else {
                continue;
            };
            // `ComponentListItem::update`: the reason a part cannot be added, in the row
            let reason = match why {
                Err(LogisticsError::ComponentTooHot) => self.too_hot.as_str(),
                Err(LogisticsError::NoMoreArmor) => self.too_much_armor.as_str(),
                _ => "",
            };
            if row.disabled_text() != reason {
                row.set_disabled_text(reason);
            }
            if disabled == was_disabled {
                continue;
            }
            row.set_state(if disabled {
                ItemState::Disabled
            } else {
                ItemState::Enabled
            });
        }
    }

    /// `selectFirstLBComponent`: the first enabled row of the list, after the rows are checked.
    /// The panel follows the pick in `refresh`, which compares it with what the panel shows.
    fn select_first_list_component(&mut self, logistics: &Logistics) {
        self.refresh_rows(logistics);
        let first = (0..self.components.item_count()).find(|&i| {
            self.components
                .item(i)
                .is_some_and(|it| it.state() != ItemState::Disabled)
        });
        match first {
            Some(i) => self.components.select_item(i as i32),
            None => self.components.select_item(-1),
        };
    }

    /// `selectFirstDiagramComponent`: the first placed component, scanning rows then columns.
    fn select_first_diagram_component(&mut self, logistics: &Logistics) {
        let Some(key) = self.edited else { return };
        let Some(v) = logistics.variant(key) else {
            return;
        };
        let ctx = logistics.ctx_for(v);
        let found = (0..ctx.chassis.component_area_height)
            .flat_map(|j| (0..ctx.chassis.component_area_width).map(move |i| (i, j)))
            .find_map(|(i, j)| v.component_at(&ctx, i, j).map(|idx| v.components[idx]));
        match found {
            Some(p) => {
                self.selected_cell = Some((p.x, p.y));
                self.components.select_item(-1);
            }
            None => self.selected_cell = None,
        }
    }

    /// `MB_MSG_ADD`: `addComponent(getComponent(), -1, -1)`; the campaign finds the room.
    fn add_selected(&mut self, logistics: &mut Logistics) {
        let Some(&index) = usize::try_from(self.components.selected_item())
            .ok()
            .and_then(|row| self.rows.get(row))
        else {
            return;
        };
        self.begin_count_up(logistics, index);
        let (mut x, mut y) = (-1, -1);
        let _ = logistics.lab_add_component(index, &mut x, &mut y);
        self.selected_cell = None;
    }

    /// `MechLabScreen::addComponent`: a part that makes heat (or a heat sink, which lowers the
    /// heat by one at once) starts the heat count-up from the old heat; armor plating starts the
    /// armor count-up.
    fn begin_count_up(&mut self, logistics: &Logistics, index: usize) {
        let (Some(c), Some(v)) = (
            logistics.data().catalog.get(index),
            self.edited.and_then(|k| logistics.variant(k)),
        ) else {
            return;
        };
        let vctx = logistics.ctx_for(v);
        if c.heat != 0.0 || c.form == ComponentForm::HeatSink {
            let sink = c.form == ComponentForm::HeatSink;
            let old = v.heat(&vctx) - i32::from(sink);
            self.heat_anim.start(old, sink);
        } else if c.form == ComponentForm::Bulk {
            self.armor_anim.start(v.armor(&vctx), false);
        }
    }

    /// `MB_MSG_REMOVE`: removes the picked component. A component whose technology is not
    /// available gets the warning first, as the original shows it.
    fn remove_selected(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        let Some((x, y)) = self.selected_cell else {
            return;
        };
        let unavailable = self
            .placed_at(logistics, x, y)
            .is_some_and(|p| !logistics.component_available(p.component));
        if unavailable {
            self.error.set_text(
                ctx.assets.string(ids::MC_UNAVAILABLE_TECH as u32),
                ids::DIALOG_OK,
            );
            self.error.begin();
            self.open = Open::Error;
        }
        let before = self.figures(logistics);
        let _ = logistics.lab_remove_component(x, y);
        // `removeComponent`: whichever of heat and armor changed counts from its old value
        if let (Some((heat, armor)), Some((new_heat, new_armor))) =
            (before, self.figures(logistics))
        {
            if heat != new_heat {
                self.heat_anim.start(heat, false);
            } else if armor != new_armor {
                self.armor_anim.start(armor, false);
            }
        }
        self.selected_cell = None;
        self.select_first_diagram_component(logistics);
        if self.selected_cell.is_none() {
            self.select_first_list_component(logistics);
        }
    }

    /// The heat and the armor of the variant being edited.
    fn figures(&self, logistics: &Logistics) -> Option<(i32, i32)> {
        let v = logistics.variant(self.edited?)?;
        let vctx = logistics.ctx_for(v);
        Some((v.heat(&vctx), v.armor(&vctx)))
    }

    /// The placed component whose cells cover `(x, y)`.
    fn placed_at(&self, logistics: &Logistics, x: i64, y: i64) -> Option<PlacedComponent> {
        let v = logistics.variant(self.edited?)?;
        let ctx = logistics.ctx_for(v);
        v.component_at(&ctx, x, y).map(|i| v.components[i])
    }

    /// The left edge and top of the grid, and its bottom (`minX`, `minY`, `maxY`): the grid's
    /// columns are `rects[13 - width]`, its bottom is `rects[7]`'s bottom.
    fn grid(&self, screen: &LogisticsScreen, ctx: &Ctx<'_>) -> (f32, f32) {
        let col = usize::try_from((13 - ctx.chassis.component_area_width).max(0)).unwrap_or(0);
        let min_x = area_of(screen, col).left;
        let max_y = area_of(screen, 7).bottom;
        let min_y = max_y - ctx.chassis.component_area_height as f32 * YICON_FACTOR;
        (min_x, min_y)
    }

    /// `getMouseDiagramCoords`: the cell under a screen point. The jump jet slot is `(-2, -2)`.
    fn cell_at(&self, logistics: &Logistics, px: f32, py: f32) -> (i64, i64) {
        let Some(v) = self.edited.and_then(|k| logistics.variant(k)) else {
            return (-1, -1);
        };
        let ctx = logistics.ctx_for(v);
        if self.jump_slot.contains(px, py) && ctx.chassis.can_have_jump_jets {
            return (-2, -2);
        }
        let (min_x, min_y) = self.grid(&self.screen, &ctx);
        (
            ((px - min_x) / XICON_FACTOR).trunc() as i64,
            ((py - min_y) / YICON_FACTOR).trunc() as i64,
        )
    }

    /// The centre of the jump jet slot on screen, for scripted input (the tests click it to pick
    /// the jump jets).
    pub fn jump_slot_centre(&self) -> (f32, f32) {
        (
            (self.jump_slot.left + self.jump_slot.right) / 2.0,
            (self.jump_slot.top + self.jump_slot.bottom) / 2.0,
        )
    }

    /// The centre of a grid cell on screen, for scripted input (the app and the tests click
    /// the diagram with it).
    pub fn cell_centre(&self, logistics: &Logistics, x: i64, y: i64) -> Option<(f32, f32)> {
        let v = self.edited.and_then(|k| logistics.variant(k))?;
        let ctx = logistics.ctx_for(v);
        let (min_x, min_y) = self.grid(&self.screen, &ctx);
        Some((
            min_x + (x as f32 + 0.5) * XICON_FACTOR,
            min_y + (y as f32 + 0.5) * YICON_FACTOR,
        ))
    }

    /// A click in the diagram picks the placed component under it (`updateDiagramInput`).
    fn pick_in_diagram(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics) {
        let (mx, my) = ctx.input.mouse_pos_f();
        if !(ctx.input.is_left_click() && self.diagram.contains(mx, my)) {
            return;
        }
        let (x, y) = self.cell_at(logistics, mx, my);
        match self.placed_at(logistics, x, y) {
            Some(p) => {
                self.selected_cell = Some((p.x, p.y));
                self.components.select_item(-1);
            }
            None => self.selected_cell = None,
        }
    }

    /// `beginDrag`/`endDrag`: while a list row is dragged, its icon follows the pointer.
    fn track_drag(&mut self, ctx: &UpdateCtx<'_>) {
        self.drag = None;
        if !ctx.input.is_left_drag() {
            return;
        }
        let (dx, dy) = (ctx.input.mouse_drag_x(), ctx.input.mouse_drag_y());
        if !self.components.point_inside(dx, dy) {
            return;
        }
        let (mx, my) = ctx.input.mouse_pos_f();
        if let Some(&index) = usize::try_from(self.components.selected_item())
            .ok()
            .and_then(|row| self.rows.get(row))
        {
            self.drag = Some((index, mx, my));
        }
    }

    /// A list row dragged onto the diagram is added where it is dropped (`endDrag`): the jump
    /// slot takes jump jets, anything else takes its cells, and the new piece is picked.
    fn drop_from_list(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        if !(ctx.input.left_mouse_released() && ctx.input.was_left_drag()) {
            return;
        }
        let (mx, my) = ctx.input.mouse_pos_f();
        if !self.diagram.contains(mx, my) {
            return;
        }
        let (dx, dy) = (
            ctx.input.mouse_drag_x() as f32,
            ctx.input.mouse_drag_y() as f32,
        );
        let list = self.components.point_inside(dx as i32, dy as i32);
        let Some(&index) = usize::try_from(self.components.selected_item())
            .ok()
            .and_then(|row| self.rows.get(row))
        else {
            return;
        };
        if !list {
            return;
        }
        let (cell_x, cell_y) = self.cell_at(logistics, mx, my);
        if (cell_x, cell_y) == (-2, -2) {
            let (mut x, mut y) = (cell_x, cell_y);
            self.begin_count_up(logistics, index);
            let _ = logistics.lab_add_component(index, &mut x, &mut y);
            return;
        }
        let Some((mut x, mut y)) = self.drop_origin(logistics, index, (cell_x, cell_y), mx) else {
            return;
        };
        self.begin_count_up(logistics, index);
        if logistics.lab_add_component(index, &mut x, &mut y).is_ok() {
            self.selected_cell = Some((x, y));
            self.components.select_item(-1);
        }
    }

    /// The origin a dragged piece is dropped at (`updateDiagramInput`): the piece is centred on
    /// the cell under the pointer, moved one cell right when the pointer is in the right half of
    /// that cell and the piece is at least two wide, then kept inside the grid.
    fn drop_origin(
        &self,
        logistics: &Logistics,
        index: usize,
        (cell_x, cell_y): (i64, i64),
        px: f32,
    ) -> Option<(i64, i64)> {
        let v = self.edited.and_then(|k| logistics.variant(k))?;
        let vctx = logistics.ctx_for(v);
        let c = logistics.data().catalog.get(index)?;
        let (w, h) = (i64::from(c.width), i64::from(c.height));
        let (min_x, _) = self.grid(&self.screen, &vctx);
        let cell_left = min_x + cell_x as f32 * XICON_FACTOR;
        let mut x = cell_x - w / 2;
        let mut y = cell_y - h / 2;
        if px - cell_left > XICON_FACTOR / 2.0 && w / 2 != 0 {
            x += 1;
        }
        let area_w = vctx.chassis.component_area_width;
        let area_h = vctx.chassis.component_area_height;
        x = x.clamp(0, (area_w - 1).max(0));
        y = y.clamp(0, (area_h - 1).max(0));
        if x + w > area_w {
            x = area_w - w;
        }
        if y + h > area_h {
            y = area_h - h;
        }
        Some((x, y))
    }

    /// The enables of Add and Remove, the info panel, and the numbers under the diagram
    /// (`MechLabScreen::update`'s tail).
    fn refresh(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics) {
        let Some(key) = self.edited else { return };
        let Some(v) = logistics.variant(key) else {
            return;
        };
        let vctx = logistics.ctx_for(v);

        self.refresh_rows(logistics);
        // Add: a list row that is not disabled and no diagram pick
        let list_pick = usize::try_from(self.components.selected_item())
            .ok()
            .and_then(|i| self.components.item(i))
            .is_some_and(|it| it.state() != ItemState::Disabled);
        // Remove: a diagram pick the campaign lets go
        let placed = self.selected_cell;
        let removable = placed.is_some_and(|(x, y)| logistics.lab_can_remove_component(x, y));
        if let Some(b) = self.screen.button_mut(button::MB_MSG_ADD) {
            b.disable(placed.is_some() || !list_pick);
        }
        if let Some(b) = self.screen.button_mut(button::MB_MSG_REMOVE) {
            b.disable(!removable);
        }

        // the panel shows the diagram's pick, else the list's
        let shown = match placed.and_then(|(x, y)| self.placed_at(logistics, x, y)) {
            Some(p) => Some(p.component),
            None if list_pick => usize::try_from(self.components.selected_item())
                .ok()
                .and_then(|i| self.rows.get(i).copied()),
            None => None,
        };
        if self.shown != Some(shown) {
            self.shown = Some(shown);
            self.show_component(ctx, logistics, shown, v);
        }

        // the cost change against the mech as it came in, the heat and armor counts
        let change = logistics.variant_cost(key).unwrap_or(0) - self.original_cost;
        self.set_text(5, &format!("{change} "));
        self.set_color(5, 0xffff_ffff);
        // `updateHeatMeter`, `updateArmorMeter`
        let (max_heat, max_armor) = (v.max_heat(&vctx), vctx.chassis.max_armor as i32);
        let heat = self.heat_anim.step(v.heat(&vctx), ctx.dt, true);
        if heat.show_denominator {
            self.set_text(12, &format!("{}/{max_heat}", heat.number));
        } else {
            self.set_text(12, &format!("{}/", heat.number));
        }
        self.meters[ATTRIBUTE_METER_COUNT - 4].set_value(heat.meter / max_heat as f32);
        let armor = self.armor_anim.step(v.armor(&vctx), ctx.dt, false);
        self.set_text(11, &format!("{}/{max_armor}", armor.number));
        self.meters[ATTRIBUTE_METER_COUNT - 3].set_value(armor.meter / max_armor as f32);
        self.set_text(1, &format!("{} ", logistics.c_bills()));
    }

    /// `setComponent(pComponent)`: the panel's labels and numbers for one component, and which
    /// of its rows are shown.
    fn show_component(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        logistics: &Logistics,
        comp: Option<usize>,
        v: &Variant,
    ) {
        // ammo doesn't deserve a meter
        self.meters[3].show = false;
        let Some(c) = comp.and_then(|i| logistics.data().catalog.get(i)) else {
            for i in [7, 8, 9, 10, 13] {
                self.set_text(i, "");
            }
            self.set_text(6, "");
            for m in &mut self.meters[..5] {
                m.set_value(0.0);
                m.show = false;
            }
            self.model = None;
            self.spin.reset();
            return;
        };
        // the little spinning figure (`camera.setComponent(getPictureFileName())`)
        self.model = c.picture_file.as_ref().map(|file| {
            let (width, height) = rect_size((
                self.camera.left,
                self.camera.top,
                self.camera.right,
                self.camera.bottom,
            ));
            PreviewRequest {
                shape: crate::preview::shape_name(file),
                paint: COMPONENT_PAINT,
                scale: 0.0,
                yaw: 0.0,
                width,
                height,
                building: false,
                kind: PreviewKind::Component,
                activated: false,
            }
        });
        self.spin.reset();
        self.set_text(6, ctx.assets.string(c.string_id as u32));
        match c.form {
            ComponentForm::JumpJet => {
                self.show_jump_jet_items(true);
                self.meters[4].set_value((v.max_jump_range() * 25) as f32 / MAX_JUMP_RANGE);
                self.set_text(7, &format!("{}", v.max_jump_range() * 25));
                self.set_color(7, 0xff00_5392);
                self.meters[7].set_value(c.heat / MAX_HEAT);
                self.meters[7].show = true;
                self.set_text(15, &format!("{:.1}", c.heat));
                for i in [8, 9, 10, 13] {
                    self.set_text(i, "");
                }
                self.meters[0].show = false;
            }
            ComponentForm::HeatSink => {
                // `mechlabscreen.cpp`: the jump range and its rows hide, the heat row shows
                self.show_jump_jet_items(true);
                self.meters[4].show = false;
                self.set_visible(7, false);
                self.set_text(7, "");
                self.set_static(47, false);
                self.set_text(15, "");
                self.set_static(48, false);
                self.set_static(49, false);
                self.set_static(30, false);
                self.set_static(55, true);
                self.meters[7].show = false;
                // negative heat is stored in the damage of a heat sink
                self.meters[8].set_value(c.damage / MAX_HEAT);
                self.meters[8].show = true;
                self.set_text(16, &format!("+{:.1}", c.damage));
                self.set_visible(16, true);
                for i in [8, 9, 10, 13] {
                    self.set_text(i, "");
                }
                self.meters[0].show = false;
            }
            ComponentForm::Bulk => {
                // the armour row shows in place of the jump range
                self.show_jump_jet_items(true);
                self.set_static(48, false);
                self.set_static(49, true);
                self.set_static(30, false);
                self.set_text(7, &format!("{:.1}", c.damage));
                self.set_color(7, 0xff00_5392);
                for i in [8, 9, 10, 15, 13] {
                    self.set_text(i, "");
                }
                self.meters[7].show = false;
                for m in &mut self.meters[..4] {
                    m.show = false;
                }
            }
            _ => {
                self.show_jump_jet_items(false);
                let range: u32 = match c.range_type {
                    WeaponRangeType::Short | WeaponRangeType::None => 0,
                    WeaponRangeType::Medium => 1,
                    WeaponRangeType::Long => 2,
                };
                let range_colours = [0xff6e_7c00u32, 0xff00_5392, 0xffa2_1600];
                let range_name = ctx
                    .assets
                    .string(ids::SHORT_RANGE as u32 + range)
                    .to_string();
                self.set_text(7, &range_name);
                self.set_color(7, range_colours[range as usize]);
                self.meters[0].show = true;
                self.meters[0].set_value(c.damage / MAX_DAMAGE);
                self.set_text(8, &format!("{:.1}", c.damage));
                let rate = if c.recycle_time != 0.0 {
                    10.0 / c.recycle_time
                } else {
                    0.0
                };
                self.meters[1].set_value(rate / MAX_RECYCLE);
                self.set_text(9, &format!("{rate:.1}"));
                self.meters[2].set_value(c.heat / MAX_HEAT);
                self.set_text(10, &format!("{:.1}", c.heat));
                self.meters[7].show = false;
                // `if ( !useUnlimitedAmmo )`: the ammo count and its meter
                if self.unlimited_ammo {
                    self.set_text(13, "");
                } else {
                    self.set_text(13, &format!("{}", c.ammo));
                    self.meters[3].set_value(c.ammo as f32 / MAX_AMMO);
                }
                self.set_text(15, "");
            }
        }
    }

    /// `showJumpJetItems`: the rows that belong to the jump jets and heat sinks (`bShow`) or to
    /// the weapons (`!bShow`). The ammo label and meter show with the weapons only when the
    /// player does not have unlimited ammo (`useUnlimitedAmmo`).
    fn show_jump_jet_items(&mut self, show: bool) {
        self.set_static(47, show);
        self.set_static(48, show);
        self.set_static(49, false);
        self.meters[4].show = show;
        self.meters[0].show = true;
        self.set_static(30, !show);
        self.set_visible(7, true);
        // all the heat sink items hide, whoever asks
        self.meters[8].show = false;
        self.set_visible(16, false);
        self.set_static(55, false);
        for i in 31..35 {
            self.set_static(i, !show);
        }
        // but not the heat one
        self.set_static(33, true);
        let ammo = !self.unlimited_ammo && !show;
        self.set_static(34, ammo);
        self.meters[3].show = ammo;
        for m in &mut self.meters[1..3] {
            m.show = !show;
        }
    }

    fn set_static(&mut self, i: usize, show: bool) {
        if let Some(s) = self.screen.statics.get_mut(i) {
            s.show_window(show);
        }
    }

    fn set_visible(&mut self, slot: usize, show: bool) {
        if let Some(t) = self.screen.texts.get_mut(slot) {
            t.obj.show_window(show);
        }
    }

    fn set_text(&mut self, slot: usize, s: &str) {
        if let Some(t) = self.screen.texts.get_mut(slot) {
            t.set_text(s);
        }
    }

    fn set_color(&mut self, slot: usize, c: u32) {
        if let Some(t) = self.screen.texts.get_mut(slot) {
            t.set_color(c);
        }
    }

    /// Draws the layout, the placed components, the list and the open dialog.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>, logistics: &Logistics) {
        // `componentListBox.render()` comes first (mechlabscreen.cpp:770): the black covers it draws
        // over items hanging past its edges are then overdrawn by the layout's frame and buttons
        self.components.render(ctx);
        for m in &self.meters {
            m.render(ctx, 0, 0);
        }
        // `camera.render()`, under the layout's frame
        if let Some(req) = &self.model {
            let r = (
                self.camera.left,
                self.camera.top,
                self.camera.right,
                self.camera.bottom,
            );
            self.spin.draw(ctx.dl, ctx.assets, req, r);
        }
        self.screen.render(ctx);
        self.render_diagram(ctx, logistics);
        // `variantList.render()` comes after the diagram, so an open drop-down is drawn over it
        self.variant_combo.render(ctx);
        self.render_drag(ctx, logistics);
        for which in [self.closing, self.open] {
            match which {
                Open::Save => self.save.render(ctx),
                Open::Accept => self.accept.render(ctx),
                Open::Error => self.error.render(ctx),
                Open::None => {}
            }
        }
    }

    /// Draws `component`'s icon over `(x, y)`, `width` by `height` cells (`setTexture`, `resize`,
    /// `setUVs(0, 0, sizeX * 48, sizeY * 32)`). False when the component has no icon art loaded.
    fn draw_icon(&self, ctx: &mut RenderCtx<'_>, component: usize, x: f32, y: f32) -> bool {
        let Some(&(tex, file_width, size_x, size_y)) = self.icons.get(&component) else {
            return false;
        };
        let mut icon = Object::new(
            x,
            y,
            size_x as f32 * XICON_FACTOR,
            size_y as f32 * YICON_FACTOR,
        );
        icon.set_texture(Some(tex), file_width);
        icon.set_uvs(0.0, 0.0, size_x as f32 * 48.0, size_y as f32 * 32.0);
        icon.set_color(0xffff_ffff, false);
        icon.render(ctx);
        true
    }

    /// `dragIcon.render()`: the component dragged out of the list follows the pointer.
    fn render_drag(&self, ctx: &mut RenderCtx<'_>, logistics: &Logistics) {
        let Some((index, mx, my)) = self.drag else {
            return;
        };
        let Some(c) = logistics.data().catalog.get(index) else {
            return;
        };
        let (w, h) = (
            c.width.max(1) as f32 * XICON_FACTOR,
            c.height.max(1) as f32 * YICON_FACTOR,
        );
        self.draw_icon(ctx, index, mx - w / 2.0, my - h / 2.0);
    }

    /// The slots under the diagram, each placed component's icon over its cells, and the pick as
    /// an outline. A component with no icon art is a block of its form's colour.
    fn render_diagram(&self, ctx: &mut RenderCtx<'_>, logistics: &Logistics) {
        let Some(v) = self.edited.and_then(|k| logistics.variant(k)) else {
            return;
        };
        let vctx = logistics.ctx_for(v);
        let (min_x, min_y) = self.grid(&self.screen, &vctx);
        // `payloadIcon` once per cell of the chassis' component area
        for j in 0..vctx.chassis.component_area_height {
            for i in 0..vctx.chassis.component_area_width {
                let mut slot = self.payload.clone();
                slot.move_to(
                    min_x + i as f32 * self.payload.width(),
                    min_y + j as f32 * self.payload.height(),
                );
                slot.render(ctx);
            }
        }
        for p in &v.components {
            let Some(c) = logistics.data().catalog.get(p.component) else {
                continue;
            };
            let (x, y) = if c.form == ComponentForm::JumpJet {
                (self.jump_slot.left, self.jump_slot.top)
            } else {
                (
                    min_x + p.x as f32 * XICON_FACTOR,
                    min_y + p.y as f32 * YICON_FACTOR,
                )
            };
            if self.draw_icon(ctx, p.component, x, y) {
                continue;
            }
            let w = c.width.max(1) as f32 * XICON_FACTOR;
            let h = c.height.max(1) as f32 * YICON_FACTOR;
            let r = crate::draw::GuiRect::new(x as i32, y as i32, (x + w) as i32, (y + h) as i32);
            ctx.dl.fill_rect(r, form_colour(c.form));
            ctx.dl.outline_rect(r, 0xffb0_b0b0, 0xff40_4040);
        }
        if let Some((cx, cy)) = self.selected_cell {
            let (x, y) = if (cx, cy) == (-2, -2) {
                (self.jump_slot.left, self.jump_slot.top)
            } else {
                (
                    min_x + cx as f32 * XICON_FACTOR,
                    min_y + cy as f32 * YICON_FACTOR,
                )
            };
            let r = crate::draw::GuiRect::new(
                x as i32,
                y as i32,
                (x + XICON_FACTOR) as i32,
                (y + YICON_FACTOR) as i32,
            );
            ctx.dl.outline_rect(r, 0xffff_ffff, 0xffff_ffff);
        }
    }
}

/// The `rects[i]` of a layout in screen pixels.
fn area_of(screen: &LogisticsScreen, i: usize) -> Area {
    screen
        .rect(i)
        .map(|r| Area {
            left: r.obj.left(),
            top: r.obj.top(),
            right: r.obj.right(),
            bottom: r.obj.bottom(),
        })
        .unwrap_or_default()
}

/// The `COMPONENT_FORM_*` number of a form (`ComponentFormString` order).
fn component_form_id(f: ComponentForm) -> i32 {
    COMPONENT_FORMS
        .iter()
        .position(|&x| x == f)
        .map_or(-1, |i| i as i32)
}

/// A colour per form for the diagram blocks (the component art is not drawn).
fn form_colour(f: ComponentForm) -> u32 {
    match f {
        ComponentForm::WeaponEnergy => 0xff00_c8e1,
        ComponentForm::WeaponBallistic => 0xff6e_7c00,
        ComponentForm::WeaponMissile => 0xffa2_1600,
        ComponentForm::JumpJet => 0xff00_5392,
        ComponentForm::HeatSink => 0xff80_8080,
        ComponentForm::Bulk => 0xff60_6060,
        _ => 0xff40_4040,
    }
}
