// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission unit bar: `ForceGroupBar` (`code/forcegroupbar.cpp`) with its `ForceGroupIcon`s,
//! `MechIcon`, `VehicleIcon` and `PilotIcon` (`code/mechicon.cpp`).
//!
//! [`UnitBar::update`] takes plain input ([`UnitBarInput`]: the player's units as the app sees them)
//! and returns plain values ([`UnitBarOutput`]: [`UnitBarIntent`]s, sound ids, help text ids). It never
//! touches the simulation; selection and orders are the app's to apply. Armour and damage states are
//! computed here from the per-location numbers, as `MechIcon::update` does.
//!
//! [`UnitBar::render`] draws the frame and advances the timers (death, message flash, jumper flash) by
//! the frame length in its context, as the C++ does while it renders. Call it once per update, after
//! the update, so a frame is counted once. The app applies the intents of both calls.
//!
//! * [`layout`]: the 16 slot rectangles, read from the `MechIconN` blocks of `buttonlayout800.fit`.
//! * [`damage`]: armour to damage state, and the recolouring of an icon's art.
//! * [`anim`]: the death and flash timings.
//! * `order` (private): the `ForceGroupIcon::sort` comparator.
//! * `paint` (private): drawing into a [`DrawList`](crate::draw::DrawList).
//!
//! Not ported in this slice, and never faked: the repair, guard, eject and info-select cursor modes
//! (control panel), the rear icon (only the info window draws it,
//! `code/infowindow.cpp`), the multiplayer cover icon and reinforcement messages. The bar always works
//! in the normal mode and draws the 800-wide art set, which is the logical 800x600 frame.

pub mod anim;
pub mod damage;
pub mod layout;
mod order;
mod paint;
pub use paint::portrait_uv;

use crate::ctx::{sfx, Loader, RenderCtx};
use crate::draw::GuiRect;
use crate::font::FontRef;
use crate::textures::{TexId, TextureStore};
use crate::widgets::object::Object;
use layout::Layout;
use mc2_formats::ini::FitIni;

/// `MAX_ICONS` (`code/forcegroupbar.h:21`).
pub const MAX_ICONS: usize = 16;
/// `unitIconX` and `unitIconY` at 800 wide (`code/mechicon.cpp:1511-1512`).
pub const ICON_W: u32 = 32;
pub const ICON_H: u32 = 38;
/// `pilotIconX` and `pilotIconY` at 800 wide (`code/mechicon.cpp:1509-1510`).
pub const PILOT_W: u32 = 25;
pub const PILOT_H: u32 = 36;
/// `ForceGroupIcon::ForceGroupColors` (`code/mechicon.cpp:61-73`): groups 1 to 9, the group 0
/// colour, then the grey of a unit in no group.
pub const FORCE_GROUP_COLORS: [u32; 11] = [
    0xff00_5392,
    0xffc6_6600,
    0xff6e_7c00,
    0xffb7_0000,
    0xff62_0053,
    0xffd8_ad00,
    0xff8b_00be,
    0xffff_0000,
    0xffff_8a00,
    0xff00_91ff,
    0xff50_5050,
];
/// `SB_GREEN` (`mclib/dbasegui.h:87`): the health bar colour, alpha 0x6f.
pub const SB_GREEN: u32 = 0x6f00_ff00;
/// The pilot portrait frame (`code/mechicon.cpp:927`).
pub const PILOT_FRAME: u32 = 0xff00_2f55;
/// The portrait cell of every vehicle icon: `VehicleIcon::init` sets it (`code/mechicon.cpp:1147`).
pub const VEHICLE_PORTRAIT: u32 = 26;

/// `IDS_FORCEGROUP_BAR` and `IDS_FORCEGROUP_BAR_DESC` (`resource.h:1607-1609`).
pub const IDS_FORCEGROUP_BAR: u32 = 30081;
pub const IDS_FORCEGROUP_BAR_DESC: u32 = 30082;
/// `IDS_UNIT_SELECT_HELP` (`resource.h:3549`).
pub const IDS_UNIT_SELECT_HELP: u32 = 45124;

/// The `.fit` block that names the `IconFont` (`code/forcegroupbar.cpp:382-390`).
const FONTS_BLOCK: &str = "Fonts";

/// Armour and body state of one location: `curArmor`, `maxArmor`, and whether the body part is
/// destroyed (`body[...].damageState == IS_DAMAGE_DESTROYED`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Loc {
    pub cur: f32,
    pub max: f32,
    pub destroyed: bool,
}

impl Loc {
    /// `curArmor / (float)maxArmor`, as the icon code divides them (`code/mechicon.cpp:583`).
    pub fn ratio(&self) -> f32 {
        self.cur / self.max
    }
}

/// The armour of a mech, by the `MECH_ARMOR_LOCATION_*` names the icon reads (`code/mechicon.cpp:582-761`).
/// The `r` prefixed locations are the rear armour, which front icons average in.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MechArmor {
    pub head: Loc,
    pub ctorso: Loc,
    pub rctorso: Loc,
    pub ltorso: Loc,
    pub rltorso: Loc,
    pub rtorso: Loc,
    pub rrtorso: Loc,
    pub larm: Loc,
    pub rarm: Loc,
    pub lleg: Loc,
    pub rleg: Loc,
}

/// The armour of a ground vehicle, by `GROUNDVEHICLE_LOCATION_*` (`code/mechicon.cpp:1241-1286`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VehicleArmor {
    pub front: Loc,
    pub rear: Loc,
    pub left: Loc,
    pub right: Loc,
    pub turret: Loc,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Armor {
    Mech(MechArmor),
    Vehicle(VehicleArmor),
}

/// The pilot as the bar shows it: `getName()`, `getPhoto()` (the portrait index) and
/// `getMessagePlaying()`.
#[derive(Clone, Debug, PartialEq)]
pub struct Pilot {
    pub name: String,
    pub photo: u32,
    pub message_playing: bool,
}

/// One player unit as the bar needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitBarUnit {
    /// A stable id the app chooses; intents refer to units by it.
    pub id: u32,
    /// `getIconPictureIndex()`: the column of the unit's art in the icon atlas.
    pub icon_picture: u32,
    /// `getTonnage()`, the sort key after the force groups and vehicles.
    pub tonnage: u32,
    pub pilot: Pilot,
    pub armor: Armor,
    /// `getAppearance()->barStatus`: the health bar fill, 0 to 1.
    pub bar_status: f32,
    /// `getSelected()`.
    pub selected: bool,
    /// `isInUnitGroup(j)`: bit `j` for force group `j` (0 to 9).
    pub groups: u16,
    /// `getStatus() == OBJECT_STATUS_SHUTDOWN`.
    pub shutdown: bool,
    /// `isDestroyed() || isDisabled()`.
    pub destroyed: bool,
    /// `recoverBuddyWID != 0`: a unit being recovered keeps its icon.
    pub recover_pending: bool,
    /// `causeOfDeath == POWER_USED_UP`: the icon leaves at once, without the death animation.
    pub power_used_up: bool,
    /// `canJump()`: draws the jump jet icon and flashes when jumpers are asked for.
    pub can_jump: bool,
}

impl UnitBarUnit {
    pub fn is_vehicle(&self) -> bool {
        matches!(self.armor, Armor::Vehicle(_))
    }
}

/// The portrait cell an icon draws: a mech shows its pilot (`MechIcon::init` calls `PilotIcon::setPilot`,
/// `code/mechicon.cpp:485`), a vehicle always the same cell (`VehicleIcon::init`, `:1147`).
pub fn portrait_index(unit: &UnitBarUnit) -> u32 {
    if unit.is_vehicle() {
        VEHICLE_PORTRAIT
    } else {
        unit.pilot.photo
    }
}

/// Which mouse profile maps the buttons (`useLeftRightMouseProfile`). The preference defaults to
/// [`MouseProfile::LeftRight`] (`code/prefs.cpp:65`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MouseProfile {
    #[default]
    LeftRight,
    Classic,
}

/// The pointer for one frame, in UI coordinates, with the button events the frame saw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    pub left_click: bool,
    pub left_double_click: bool,
    pub right_click: bool,
    pub right_drag: bool,
}

/// What the bar needs for one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitBarInput {
    /// The player's units. A unit the bar has not seen gets an icon, and a unit that leaves the list
    /// loses its icon. A unit whose death animation ended is not given an icon again while it stays
    /// in the list.
    pub units: Vec<UnitBarUnit>,
    pub pointer: Pointer,
    pub shift_down: bool,
    pub profile: MouseProfile,
}

/// Something the app must do with the simulation or another lane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitBarIntent {
    /// Replace the selection with these units.
    Select(Vec<u32>),
    AddToSelection(u32),
    RemoveFromSelection(u32),
    /// Centre the camera on the unit (`GameCamera::setTarget`).
    CenterOn(u32),
    /// Select every unit of force group `j` (0 to 9), `MissionInterfaceManager::selectForceGroup`.
    SelectForceGroup(u8),
    /// Show the unit in the info window (another lane draws it).
    ShowInfo(u32),
    /// The unit left every force group: its death animation ended (`addToUnitGroup(-1)`).
    LeaveAllGroups(u32),
}

/// A help text pair for the status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HelpText {
    pub header: u32,
    pub text: u32,
}

/// What one frame of the bar produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitBarOutput {
    pub intents: Vec<UnitBarIntent>,
    /// Sound ids to play, `LOG_SELECT` for each click (`code/mechicon.cpp:427`).
    pub sounds: Vec<i32>,
    /// The unit under the pointer (`setTargeted(true)`).
    pub hovered: Option<u32>,
    /// Rollover help of a unit under the pointer.
    pub rollover_help: Option<u32>,
    /// The bar's own help, while the pointer is over the bar.
    pub bar_help: Option<HelpText>,
}

/// The art and font the bar draws with.
pub struct Art {
    /// The mech icon atlas, front row on top (`mcui_med4.tga`).
    pub mech_atlas: TexId,
    /// The vehicle icon atlas (`mcui_med5.tga`).
    pub vehicle_atlas: TexId,
    /// The pilot portraits (`mcui_med2.tga`).
    pub pilot_atlas: TexId,
    /// The jump jet icon (`JumpJetIcon`).
    pub jump_jet: Object,
    /// `IconFont` (`40018`).
    pub font: FontRef,
}

impl Art {
    /// Loads the atlases, the jump jet and the font the bar draws with. A missing piece is an error,
    /// named in it: a bar without its names or icons would report success and draw them not at all.
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni) -> Result<Art, String> {
        file.seek_block(FONTS_BLOCK)
            .map_err(|e| format!("unit bar: block {FONTS_BLOCK}: {e}"))?;
        let font_id = file
            .read_i64("IconFont")
            .map_err(|e| format!("unit bar: IconFont: {e}"))?;
        let font = ld.font(font_id as u32);
        let mech = ld.art("mcui_med4.tga");
        let vehicle = ld.art("mcui_med5.tga");
        let pilot = ld.art("mcui_med2.tga");
        let jump_jet = Object::from_fit(ld, file, "JumpJetIcon");
        Art::assemble(mech, vehicle, pilot, jump_jet, font)
    }

    /// The art from its parts, each `None` or without a texture when it did not load. Every part is
    /// required, so the first missing one is the error.
    pub fn assemble(
        mech: Option<TexId>,
        vehicle: Option<TexId>,
        pilot: Option<TexId>,
        jump_jet: Object,
        font: FontRef,
    ) -> Result<Art, String> {
        let missing = |what: &str| format!("unit bar art missing: {what}");
        if font.id.is_none() {
            return Err(missing("IconFont"));
        }
        if jump_jet.texture.is_none() {
            return Err(missing("JumpJetIcon"));
        }
        Ok(Art {
            mech_atlas: mech.ok_or_else(|| missing("mcui_med4.tga"))?,
            vehicle_atlas: vehicle.ok_or_else(|| missing("mcui_med5.tga"))?,
            pilot_atlas: pilot.ok_or_else(|| missing("mcui_med2.tga"))?,
            jump_jet,
            font,
        })
    }
}

/// `ForceGroupIcon::render`'s colour for a unit's force groups (`code/mechicon.cpp:892-903`):
/// the highest group the unit is in, where group 0 counts as colour 10 (`ForceGroupColors[9]`), and
/// the grey when it is in none.
pub fn force_group_color(groups: u16) -> u32 {
    let mut highest = None;
    for j in 0..10 {
        if groups >> j & 1 == 1 {
            highest = Some(j);
        }
    }
    let index = match highest {
        None => 10,
        Some(0) => 9,
        Some(j) => j - 1,
    };
    FORCE_GROUP_COLORS[index]
}

/// One icon: a unit the bar draws, with its own copy of the art and its damage state.
pub(super) struct Icon {
    /// The unit as the last input described it.
    pub(super) unit: UnitBarUnit,
    /// Creation order, the tie break the sort falls back on.
    pub(super) seq: u64,
    /// This icon's art in the texture store (`ForceGroupIcon::damageIconIndex`).
    pub(super) image: TexId,
    /// Which of the [`MAX_ICONS`] image names the art lives under.
    pub(super) image_slot: usize,
    /// The damage state each location is drawn in (`MechIcon::damage`, `VehicleIcon::damage`).
    pub(super) damage: [u8; 8],
    /// The slot the icon sits in after the sort (`locationIndex`).
    pub(super) slot: usize,
    /// `deathAnimationTime`: 0 while not dying.
    pub(super) death_time: f32,
    /// Set once the death animation has released the unit's force groups.
    pub(super) death_reported: bool,
    /// `msgPlayTime`: the flashing of a pilot's message.
    pub(super) msg_time: f32,
    /// A flashing jumper in the blank part of its flash: not drawn this frame, and its timers stand
    /// still, since they run inside the icon's render (`code/forcegroupbar.cpp:327-336`).
    pub(super) hidden: bool,
}

impl Icon {
    pub(super) fn is_vehicle(&self) -> bool {
        self.unit.is_vehicle()
    }

    /// `MechIcon::update` / `VehicleIcon::update`: recomputes the states and recolours the art where
    /// they changed.
    fn refresh_damage(&mut self, textures: &mut TextureStore) {
        let (new, count) = match &self.unit.armor {
            Armor::Mech(a) => (damage::mech_states(a), 8),
            Armor::Vehicle(v) => (damage::vehicle_states(v), 5),
        };
        if damage::differs(&self.damage, &new, count) {
            if let Some(px) = textures.pixels_mut(self.image) {
                damage::apply(px, &mut self.damage, &new, count);
            }
        }
    }
}

/// The in-mission unit bar. See the module docs for the contract.
pub struct UnitBar {
    layout: Layout,
    art: Art,
    icons: Vec<Icon>,
    /// Which of the [`MAX_ICONS`] art names are in use.
    image_slots: [bool; MAX_ICONS],
    /// Units whose icon has gone (death animation ended, or power used up): not given an icon again
    /// while they stay in the input.
    retired: Vec<u32>,
    seq: u64,
    /// `forceNumFlashes` and `forceFlashTime` (`code/forcegroupbar.cpp:66-72`).
    flash_count: u32,
    flash_time: f32,
}

impl UnitBar {
    pub fn new(layout: Layout, art: Art) -> UnitBar {
        UnitBar {
            layout,
            art,
            icons: Vec::new(),
            image_slots: [false; MAX_ICONS],
            retired: Vec::new(),
            seq: 0,
            flash_count: 0,
            flash_time: 0.0,
        }
    }

    /// The number of icons on the bar.
    pub fn icon_count(&self) -> usize {
        self.icons.len()
    }

    /// The slot a unit's icon sits in, after the last [`UnitBar::update`].
    pub fn slot_of(&self, id: u32) -> Option<usize> {
        self.icons
            .iter()
            .find(|ic| ic.unit.id == id)
            .map(|ic| ic.slot)
    }

    /// Where a pilot video plays for the unit with `id`: from the left of the unit icon to the right of the pilot's
    /// portrait, over the icon's height (`ForceGroupBar::setPilotVideo`'s `vRect`, `code/forcegroupbar.cpp:459-463`).
    /// `None` when the unit has no icon.
    pub fn pilot_video_rect(&self, id: u32) -> Option<GuiRect> {
        let slot = self.layout.slots[self.slot_of(id)?];
        Some(GuiRect::new(
            slot.icon_area.left,
            slot.icon_area.top,
            slot.pilot_area.right,
            slot.icon_area.bottom,
        ))
    }

    /// Whether the unit's pilot is shown as talking (`MechWarrior::messagePlaying`) after the last
    /// [`UnitBar::update`]; `None` when the unit has no icon.
    pub fn message_playing(&self, id: u32) -> Option<bool> {
        self.icons
            .iter()
            .find(|ic| ic.unit.id == id)
            .map(|ic| ic.unit.pilot.message_playing)
    }

    /// `ForceGroupBar::flashJumpers`: flash the units that can jump `count` times.
    pub fn flash_jumpers(&mut self, count: u32) {
        self.flash_count = count;
        self.flash_time = 0.0;
    }

    /// The bookkeeping of `ForceGroupBar::update`: syncs the icons with the input, removes the dead,
    /// sorts, and answers the pointer. It advances no timer: the clocks run in [`UnitBar::render`],
    /// as they do in C++.
    pub fn update(&mut self, input: &UnitBarInput, textures: &mut TextureStore) -> UnitBarOutput {
        let mut out = UnitBarOutput::default();
        self.sync(input, textures);
        self.remove_dead();
        order::sort(&mut self.icons);
        for (i, ic) in self.icons.iter_mut().enumerate() {
            ic.slot = i;
        }
        self.pointer(&input.pointer, input.shift_down, input.profile, &mut out);
        for ic in &mut self.icons {
            ic.refresh_damage(textures);
        }
        out
    }

    /// Draws the bar into the frame's draw list, and advances its timers by the frame length
    /// `ctx.dt`. The death animation and the pilot's message flash run inside each icon's render, and
    /// the jumper flash after the loop (`code/forcegroupbar.cpp:327-363`, `code/mechicon.cpp:1003-1012`).
    /// The intents it adds (`LeaveAllGroups`, `RemoveFromSelection`) go into `out` with the update's.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>, out: &mut UnitBarOutput) {
        // forcegroupbar.cpp:327-336 reads the flash time before this frame adds to it, and hides the
        // jumpers while it is at most .25
        let hide_jumpers = self.flash_count > 0 && self.flash_time <= 0.25;
        for ic in &mut self.icons {
            ic.hidden = hide_jumpers && ic.unit.can_jump;
            if ic.hidden {
                continue;
            }
            advance_icon(ic, ctx.dt, out);
            paint::icon(ic, &self.layout.slots[ic.slot], &self.art, ctx);
        }
        if self.flash_count > 0 {
            // forcegroupbar.cpp:355-363
            self.flash_time += ctx.dt;
            if self.flash_time > 0.5 {
                self.flash_time = 0.0;
                self.flash_count -= 1;
            }
        }
    }

    /// `ForceGroupBar::update`'s icon bookkeeping: `addMech`, `addVehicle` and `removeMover`.
    fn sync(&mut self, input: &UnitBarInput, textures: &mut TextureStore) {
        let present = |id: u32| input.units.iter().any(|u| u.id == id);
        let mut i = 0;
        while i < self.icons.len() {
            if present(self.icons[i].unit.id) {
                i += 1;
            } else {
                self.remove_at(i);
            }
        }
        self.retired.retain(|&id| present(id));
        for u in &input.units {
            match self.icons.iter().position(|ic| ic.unit.id == u.id) {
                Some(i) => self.icons[i].unit = u.clone(),
                None if !self.retired.contains(&u.id) => self.add(u.clone(), textures),
                None => {}
            }
        }
    }

    fn add(&mut self, unit: UnitBarUnit, textures: &mut TextureStore) {
        if self.icons.len() >= MAX_ICONS {
            log::warn!("unit bar is full: unit {} gets no icon", unit.id);
            return;
        }
        let Some(image_slot) = self.image_slots.iter().position(|used| !used) else {
            return;
        };
        self.image_slots[image_slot] = true;
        let atlas = match unit.armor {
            Armor::Mech(_) => self.art.mech_atlas,
            Armor::Vehicle(_) => self.art.vehicle_atlas,
        };
        let pixels = damage::copy_cell(textures, atlas, unit.icon_picture);
        let image = textures.insert(&format!("unitbar/icon{image_slot}"), ICON_W, ICON_H, pixels);
        self.seq += 1;
        self.icons.push(Icon {
            unit,
            seq: self.seq,
            image,
            image_slot,
            damage: [0; 8],
            slot: 0,
            death_time: 0.0,
            death_reported: false,
            msg_time: 0.0,
            hidden: false,
        });
    }

    fn remove_at(&mut self, i: usize) {
        let ic = self.icons.remove(i);
        self.image_slots[ic.image_slot] = false;
    }

    /// The dead leave the bar after their death animation (`ForceGroupBar::update`,
    /// `code/forcegroupbar.cpp:170-185`).
    fn remove_dead(&mut self) {
        let mut i = 0;
        while i < self.icons.len() {
            let ic = &mut self.icons[i];
            if ic.unit.destroyed && !ic.unit.recover_pending {
                if ic.death_time == 0.0 {
                    // beginDeathAnimation
                    ic.death_time = anim::DEATH_START;
                }
                if anim::over(ic.death_time) || ic.unit.power_used_up {
                    let id = ic.unit.id;
                    self.remove_at(i);
                    self.retired.push(id);
                    // the C++ loop's ++t steps past the icon the memmove shifted into slot i, so
                    // that icon is not examined until the next update
                    i += 1;
                    continue;
                }
            }
            i += 1;
        }
    }

    /// The pointer part of `ForceGroupBar::update` and `ForceGroupIcon::click`
    /// (`code/forcegroupbar.cpp:118-292`, `code/mechicon.cpp:412-428`), for the normal mode.
    /// The selection changes here, before the render, so the outline of a click shows in its own frame.
    /// The unselect-all of a plain click is folded into the click's `Select`: a plain click on an
    /// icon always selects that icon alone, so the two give the same selection.
    fn pointer(
        &mut self,
        p: &Pointer,
        shift: bool,
        profile: MouseProfile,
        out: &mut UnitBarOutput,
    ) {
        let lr = profile == MouseProfile::LeftRight;
        // forcegroupbar.cpp:121-127 (the bSelect reset is at :126-127)
        let camera = if lr {
            p.left_double_click
        } else {
            p.right_click && !p.right_drag
        };
        let select = p.left_click && !camera;
        let command = if lr { p.right_click } else { p.left_click };
        let force_group = p.left_double_click;

        // the bar's own help: inside the bar, strictly between its outer edges (forcegroupbar.cpp:133-147)
        // FORCEGROUP_LEFT, FORCEGROUP_WIDTH and FORCEGROUP_TOP (forcegroupbar.cpp:36-38, selectionRect[0] and [7])
        let first = self.layout.slots[0].selection;
        let last = self.layout.slots[7].selection;
        let left = first.left;
        let width = last.right - left;
        if p.x > left && p.x < left + width && p.y > first.top {
            out.bar_help = Some(HelpText {
                header: IDS_FORCEGROUP_BAR,
                text: IDS_FORCEGROUP_BAR_DESC,
            });
        }

        for i in 0..self.icons.len() {
            let ic = &self.icons[i];
            let rect = self.layout.slots[ic.slot].selection;
            if !contains(rect, p.x, p.y) {
                continue;
            }
            let (id, groups) = (ic.unit.id, ic.unit.groups);
            out.hovered = Some(id);
            out.rollover_help = Some(IDS_UNIT_SELECT_HELP);
            if select {
                click(&mut self.icons, i, shift, out);
                out.intents.push(UnitBarIntent::ShowInfo(id));
            }
            if command {
                out.intents.push(UnitBarIntent::ShowInfo(id));
            }
            if camera {
                out.intents.push(UnitBarIntent::CenterOn(id));
            }
            if force_group {
                for j in 0..10 {
                    if groups >> j & 1 == 1 {
                        out.intents.push(UnitBarIntent::SelectForceGroup(j));
                    }
                }
            }
        }
    }
}

/// The timers of one drawn icon: the death animation (`ForceGroupIcon::drawDeathEffect`,
/// `code/mechicon.cpp:1440-1502`) and the pilot's message flash (`code/mechicon.cpp:1003-1012`).
/// Called from the icon's render only, so a hidden jumper's clocks stand still.
fn advance_icon(ic: &mut Icon, dt: f32, out: &mut UnitBarOutput) {
    if ic.death_time != 0.0 {
        let id = ic.unit.id;
        if ic.unit.selected {
            // drawDeathEffect: unit->setSelected(0)
            out.intents.push(UnitBarIntent::RemoveFromSelection(id));
        }
        ic.death_time += dt;
        if !ic.death_reported && anim::death_frame(ic.death_time) == anim::Frame::Finished {
            out.intents.push(UnitBarIntent::LeaveAllGroups(id));
            ic.death_reported = true;
        }
    }
    if ic.unit.pilot.message_playing {
        // mechicon.cpp:1004-1012
        ic.msg_time += dt;
        if ic.msg_time > 0.25 {
            ic.msg_time = 0.0;
        }
    } else {
        ic.msg_time = 0.0;
    }
}

/// `inRegion`: inclusive on all four edges (`code/mechicon.cpp:401-405`).
fn contains(r: GuiRect, x: i32, y: i32) -> bool {
    x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
}

/// `ForceGroupIcon::click` (`code/mechicon.cpp:412-428`): shift toggles the icon's unit, a plain
/// click selects it. C++ sets the selection inside the click, so the bar's own view of the selection
/// changes here too, and the icon's outline is right in the frame of the click. A plain click first
/// unselects every unit (`code/forcegroupbar.cpp:151-165`).
fn click(icons: &mut [Icon], i: usize, shift: bool, out: &mut UnitBarOutput) {
    let id = icons[i].unit.id;
    if shift {
        let on = !icons[i].unit.selected;
        icons[i].unit.selected = on;
        out.intents.push(if on {
            UnitBarIntent::AddToSelection(id)
        } else {
            UnitBarIntent::RemoveFromSelection(id)
        });
    } else {
        for ic in icons.iter_mut() {
            ic.unit.selected = false;
        }
        icons[i].unit.selected = true;
        out.intents.push(UnitBarIntent::Select(vec![id]));
    }
    out.sounds.push(sfx::LOG_SELECT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ctx::UiAssets;
    use crate::draw::DrawList;
    use crate::font::FontId;

    #[test]
    fn force_group_colour_takes_the_highest_group() {
        // no group: the grey (mechicon.cpp:903 with forceGroup -1)
        assert_eq!(force_group_color(0), 0xff50_5050);
        // group 1 (index 1 in the loop) is ForceGroupColors[0]
        assert_eq!(force_group_color(1 << 1), 0xff00_5392);
        // group 0 alone counts as colour 10, so ForceGroupColors[9] (mechicon.cpp:901-903)
        assert_eq!(force_group_color(1 << 0), 0xff00_91ff);
        // groups 2 and 5: the highest, 5, is ForceGroupColors[4] (mechicon.cpp:893-903)
        assert_eq!(force_group_color((1 << 2) | (1 << 5)), 0xff62_0053);
    }

    use layout::Slot;

    /// Slot `i` selects 62 pixels further right than the one before, as the shipped layout does.
    fn layout() -> Layout {
        Layout {
            slots: std::array::from_fn(|i| {
                let k = 62 * i as i32;
                Slot {
                    selection: GuiRect::new(178 + k, 486, 238 + k, 540),
                    ..Slot::default()
                }
            }),
            ..Layout::default_for_tests()
        }
    }

    /// The same as [`layout`]'s slot `i` centre, on the bar's middle line.
    fn slot_x(i: usize) -> i32 {
        208 + 62 * i as i32
    }

    fn at(x: i32) -> Pointer {
        Pointer {
            x,
            y: 513,
            ..Pointer::default()
        }
    }

    fn bar(atlas: TexId) -> UnitBar {
        let art = Art {
            mech_atlas: atlas,
            vehicle_atlas: atlas,
            pilot_atlas: atlas,
            jump_jet: Object::default(),
            font: FontRef::default(),
        };
        UnitBar::new(layout(), art)
    }

    fn full() -> Loc {
        Loc {
            cur: 1.0,
            max: 1.0,
            destroyed: false,
        }
    }

    /// A mech at full armour; the pilot is named after its id.
    fn mech(id: u32, tonnage: u32) -> UnitBarUnit {
        let f = full();
        UnitBarUnit {
            id,
            icon_picture: 0,
            tonnage,
            pilot: Pilot {
                name: format!("PILOT {id}"),
                photo: 0,
                message_playing: false,
            },
            armor: Armor::Mech(MechArmor {
                head: f,
                ctorso: f,
                rctorso: f,
                ltorso: f,
                rltorso: f,
                rtorso: f,
                rrtorso: f,
                larm: f,
                rarm: f,
                lleg: f,
                rleg: f,
            }),
            bar_status: 1.0,
            selected: false,
            groups: 0,
            shutdown: false,
            destroyed: false,
            recover_pending: false,
            power_used_up: false,
            can_jump: false,
        }
    }

    /// The lance of the click tests. Sorted lightest first (mechicon.cpp:1407-1418), unit 3 (20 t)
    /// is in slot 0, unit 1 (50 t) in slot 1 and unit 2 (90 t) in slot 2.
    fn lance() -> Vec<UnitBarUnit> {
        vec![mech(1, 50), mech(2, 90), mech(3, 20)]
    }

    fn frame(
        bar: &mut UnitBar,
        tex: &mut TextureStore,
        units: Vec<UnitBarUnit>,
        p: Pointer,
    ) -> UnitBarOutput {
        let input = UnitBarInput {
            units,
            pointer: p,
            ..UnitBarInput::default()
        };
        bar.update(&input, tex)
    }

    /// One frame as the app runs it: the update, then the render with the frame length `dt`. Returns
    /// the update's output with the render's intents added, and the draw list the render made.
    fn tick(
        bar: &mut UnitBar,
        input: &UnitBarInput,
        dt: f32,
        tex: &mut TextureStore,
    ) -> (UnitBarOutput, DrawList) {
        let mut out = bar.update(input, tex);
        let assets = UiAssets::default();
        let mut dl = DrawList::new(800, 600);
        bar.render(
            &mut RenderCtx {
                dl: &mut dl,
                assets: &assets,
                dt,
            },
            &mut out,
        );
        (out, dl)
    }

    /// The draw list a frame makes, compared as the GPU would read it.
    fn assert_same_frame(a: &DrawList, b: &DrawList, what: &str) {
        assert_eq!(a.vertices, b.vertices, "{what}: vertices");
        assert_eq!(a.batches, b.batches, "{what}: batches");
    }

    #[test]
    fn icons_fill_the_slots_in_sorted_order_for_one_to_sixteen_units() {
        // forcegroupbar.cpp:187-192: qsort, then setLocationIndex(i). Unit n is the lightest here,
        // so it takes slot 0 and unit 1 takes the last slot.
        for n in 1..=MAX_ICONS as u32 {
            let mut tex = TextureStore::new();
            let mut bar = bar(TexId(0));
            let units: Vec<_> = (1..=n).map(|id| mech(id, 100 - id)).collect();
            frame(&mut bar, &mut tex, units, Pointer::default());
            assert_eq!(bar.icon_count(), n as usize, "{n} units");
            for id in 1..=n {
                assert_eq!(
                    bar.slot_of(id),
                    Some((n - id) as usize),
                    "{n} units, unit {id}"
                );
            }
        }
    }

    #[test]
    fn a_seventeenth_unit_gets_no_icon() {
        // MAX_ICONS is 16 (forcegroupbar.h:21): the sixteen first given keep their icons
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let units: Vec<_> = (1..=17).map(|id| mech(id, 100 - id)).collect();
        frame(&mut bar, &mut tex, units, Pointer::default());
        assert_eq!(bar.icon_count(), MAX_ICONS);
        assert_eq!(bar.slot_of(17), None);
    }

    /// An art set with every part, for the tests that take one piece away.
    fn whole_art() -> (Option<TexId>, Option<TexId>, Option<TexId>, Object, FontRef) {
        let jump = Object {
            texture: Some(TexId(3)),
            ..Object::default()
        };
        let font = FontRef {
            id: Some(FontId(4)),
            ..FontRef::default()
        };
        (Some(TexId(0)), Some(TexId(1)), Some(TexId(2)), jump, font)
    }

    #[test]
    fn art_needs_every_atlas_the_jump_jet_and_the_font() {
        // ForceGroupBar::init's art (forcegroupbar.cpp:379-399): none of these may be missing
        let (m, v, p, j, f) = whole_art();
        assert!(Art::assemble(m, v, p, j.clone(), f).is_ok());
        let err = Art::assemble(None, v, p, j.clone(), f).err().expect("err");
        assert!(err.contains("mcui_med4.tga"), "{err}");
        let err = Art::assemble(m, None, p, j.clone(), f).err().expect("err");
        assert!(err.contains("mcui_med5.tga"), "{err}");
        let err = Art::assemble(m, v, None, j.clone(), f).err().expect("err");
        assert!(err.contains("mcui_med2.tga"), "{err}");
        let err = Art::assemble(m, v, p, Object::default(), f)
            .err()
            .expect("err");
        assert!(err.contains("JumpJetIcon"), "{err}");
        let err = Art::assemble(m, v, p, j, FontRef::default())
            .err()
            .expect("err");
        assert!(err.contains("IconFont"), "{err}");
    }

    #[test]
    fn a_vehicle_draws_portrait_26_whatever_its_pilot_photo() {
        // VehicleIcon::init sets the texture index to 26 (mechicon.cpp:1147), and only the mech path
        // hands over a pilot (:485), so a vehicle's pilot photo is never drawn
        let mut vehicle = mech(1, 30);
        vehicle.armor = Armor::Vehicle(VehicleArmor::default());
        vehicle.pilot.photo = 0;
        assert_eq!(portrait_index(&vehicle), 26);
        // a mech with photo 7 draws its own pilot
        let mut pilot = mech(2, 50);
        pilot.pilot.photo = 7;
        assert_eq!(portrait_index(&pilot), 7);
        // a 250 texel atlas holds ten portraits to a row (PilotIcon::render, mechicon.cpp:1584-1594), so
        // cell 26 is column 6 of row 2: x 150..175 and y 72..108, with the .1 texel nudge of :1590-1594
        let (u0, v0, u1, v1) = paint::portrait_uv(250, portrait_index(&vehicle)).expect("cell");
        let nudge = 0.1 / 250.0;
        for (got, want) in [
            (u0, 150.0 / 250.0 + nudge),
            (v0, 72.0 / 250.0 + nudge),
            (u1, 175.0 / 250.0 + nudge),
            (v1, 108.0 / 250.0 + nudge),
        ] {
            assert!((got - want).abs() < 1e-6, "{got} != {want}");
        }
    }

    #[test]
    fn a_destroyed_leg_repaints_its_location_in_the_destroyed_colour() {
        // the intact colour of location 0 is damageColors[0][0] with red + 0: 0xFFC2E5FF (mechicon.cpp:828-838)
        let mut tex = TextureStore::new();
        let mut rgba = vec![0u8; (ICON_W * ICON_H * 4) as usize];
        rgba[..4].copy_from_slice(&[0xc2, 0xe5, 0xff, 0xff]);
        let atlas = tex.insert("atlas", ICON_W, ICON_H, rgba);
        let mut bar = bar(atlas);
        let mut unit = mech(1, 50);
        frame(&mut bar, &mut tex, vec![unit.clone()], Pointer::default());
        let icon = bar.icons[0].image;
        assert_eq!(&tex.get(icon).unwrap().rgba[..4], &[0xc2, 0xe5, 0xff, 0xff]);
        // a destroyed left leg reads as -1, state 3 (mechicon.cpp:708-711); damageColors[3][0] is
        // 0xFF5E6565 (mechicon.cpp:54-59)
        if let Armor::Mech(a) = &mut unit.armor {
            a.lleg.destroyed = true;
        }
        frame(&mut bar, &mut tex, vec![unit], Pointer::default());
        assert_eq!(bar.icons[0].damage[0], 3);
        assert_eq!(&tex.get(icon).unwrap().rgba[..4], &[0x5e, 0x65, 0x65, 0xff]);
    }

    #[test]
    fn a_plain_click_selects_only_that_icon_and_opens_its_info() {
        // ForceGroupIcon::click (mechicon.cpp:412-428): a plain click sets the unit selected alone, with
        // the unselect-all of forcegroupbar.cpp:151-165 folded in; setInfoWndMover follows (:242)
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let p = Pointer {
            left_click: true,
            ..at(slot_x(1))
        };
        let out = frame(&mut bar, &mut tex, lance(), p);
        assert_eq!(
            out.intents,
            [UnitBarIntent::Select(vec![1]), UnitBarIntent::ShowInfo(1)]
        );
        assert_eq!(out.sounds, [sfx::LOG_SELECT]);
        assert_eq!(out.hovered, Some(1));
        assert_eq!(out.rollover_help, Some(IDS_UNIT_SELECT_HELP));
    }

    #[test]
    fn shift_click_toggles_the_icon_in_the_selection() {
        // mechicon.cpp:412-428: shift adds an unselected unit and removes a selected one
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let mut units = lance();
        units[0].selected = true;
        let shift = |x| UnitBarInput {
            units: units.clone(),
            pointer: Pointer {
                left_click: true,
                ..at(x)
            },
            shift_down: true,
            ..UnitBarInput::default()
        };
        // slot 1 is unit 1, selected: removed
        let out = bar.update(&shift(slot_x(1)), &mut tex);
        assert_eq!(
            out.intents,
            [
                UnitBarIntent::RemoveFromSelection(1),
                UnitBarIntent::ShowInfo(1)
            ]
        );
        // slot 2 is unit 2, not selected: added
        let out = bar.update(&shift(slot_x(2)), &mut tex);
        assert_eq!(
            out.intents,
            [UnitBarIntent::AddToSelection(2), UnitBarIntent::ShowInfo(2)]
        );
    }

    #[test]
    fn a_shift_click_draws_the_toggled_outline_in_its_own_frame() {
        // mechicon.cpp:412-428 sets the selection inside the click, so the frame of the click draws
        // what the next input would: unit 1 loses its outline and unit 2 gains one (the app applies
        // the intents and sends them back as the selection).
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let mut units = lance();
        units[0].selected = true;
        let clicked = |x| UnitBarInput {
            units: units.clone(),
            pointer: Pointer {
                left_click: true,
                ..at(x)
            },
            shift_down: true,
            ..UnitBarInput::default()
        };
        let (out, dl) = tick(&mut bar, &clicked(slot_x(1)), 0.0, &mut tex);
        assert_eq!(
            out.intents[0],
            UnitBarIntent::RemoveFromSelection(1),
            "unit 1 leaves the selection"
        );
        let mut after = units.clone();
        after[0].selected = false;
        let (_, next) = tick(
            &mut bar,
            &UnitBarInput {
                units: after,
                ..UnitBarInput::default()
            },
            0.0,
            &mut tex,
        );
        assert_same_frame(&dl, &next, "shift click on unit 1");

        let (out, dl) = tick(&mut bar, &clicked(slot_x(2)), 0.0, &mut tex);
        assert_eq!(out.intents[0], UnitBarIntent::AddToSelection(2));
        let mut after = units.clone();
        after[1].selected = true;
        let (_, next) = tick(
            &mut bar,
            &UnitBarInput {
                units: after,
                ..UnitBarInput::default()
            },
            0.0,
            &mut tex,
        );
        assert_same_frame(&dl, &next, "shift click on unit 2");
    }

    #[test]
    fn a_plain_click_draws_the_new_selection_in_its_own_frame() {
        // forcegroupbar.cpp:151-165 and mechicon.cpp:412-428: the plain click unselects the others and
        // selects its unit inside the click. Unit 2 was selected before, so the frame of the click
        // draws unit 1's outline alone, what the next input (Select([1]) applied) would draw.
        let mut tex = TextureStore::new();
        let mut none = bar(TexId(0));
        let mut bar = bar(TexId(0));
        let mut units = lance();
        units[1].selected = true;
        let p = Pointer {
            left_click: true,
            ..at(slot_x(1))
        };
        let (out, dl) = tick(
            &mut bar,
            &UnitBarInput {
                units: units.clone(),
                pointer: p,
                ..UnitBarInput::default()
            },
            0.0,
            &mut tex,
        );
        assert_eq!(out.intents[0], UnitBarIntent::Select(vec![1]));
        let mut after = lance();
        after[0].selected = true;
        let (_, next) = tick(
            &mut bar,
            &UnitBarInput {
                units: after,
                ..UnitBarInput::default()
            },
            0.0,
            &mut tex,
        );
        assert_same_frame(&dl, &next, "plain click on unit 1");
        // and the frame is not the one with nothing selected: the outline is really drawn
        let (_, unselected) = tick(
            &mut none,
            &UnitBarInput {
                units: lance(),
                ..UnitBarInput::default()
            },
            0.0,
            &mut tex,
        );
        assert_ne!(dl.vertices, unselected.vertices);
    }

    #[test]
    fn a_click_outside_every_icon_does_nothing() {
        // inRegion is inclusive on the selection rectangles only (mechicon.cpp:401-405)
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let out = frame(&mut bar, &mut tex, lance(), at(10));
        assert!(out.intents.is_empty());
        assert!(out.sounds.is_empty());
        assert_eq!(out.hovered, None);
        assert_eq!(out.bar_help, None);
    }

    #[test]
    fn a_double_click_centres_on_the_unit_and_selects_its_force_groups() {
        // forcegroupbar.cpp:123-124: in the left-right profile a double click is both the camera
        // and the force group; a click that is a camera does not select (:126-127, the bSelect reset)
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        // a grouped unit sorts first (mechicon.cpp:1395-1400), so unit 1 is in slot 0 here
        let mut units = lance();
        units[0].groups = (1 << 2) | (1 << 5);
        let p = Pointer {
            left_click: true,
            left_double_click: true,
            ..at(slot_x(0))
        };
        let out = frame(&mut bar, &mut tex, units, p);
        assert_eq!(
            out.intents,
            [
                UnitBarIntent::CenterOn(1),
                UnitBarIntent::SelectForceGroup(2),
                UnitBarIntent::SelectForceGroup(5),
            ]
        );
        assert!(out.sounds.is_empty());
    }

    #[test]
    fn right_click_opens_the_info_window_in_the_left_right_profile() {
        // forcegroupbar.cpp:124-125: the command button is the right one
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let p = Pointer {
            right_click: true,
            ..at(slot_x(1))
        };
        let out = frame(&mut bar, &mut tex, lance(), p);
        assert_eq!(out.intents, [UnitBarIntent::ShowInfo(1)]);
    }

    #[test]
    fn right_click_centres_in_the_classic_profile() {
        // forcegroupbar.cpp:123: bCamera is a right click that is not a drag
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let p = Pointer {
            right_click: true,
            ..at(slot_x(1))
        };
        let input = UnitBarInput {
            units: lance(),
            pointer: p,
            profile: MouseProfile::Classic,
            ..UnitBarInput::default()
        };
        let out = bar.update(&input, &mut tex);
        assert_eq!(out.intents, [UnitBarIntent::CenterOn(1)]);
    }

    #[test]
    fn a_dead_unit_leaves_once_its_animation_ends_and_is_not_given_an_icon_again() {
        // forcegroupbar.cpp:170-185 and drawDeathEffect's LeaveAllGroups at the end (mechicon.cpp:1493-1501)
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        let mut units = lance();
        units[0].destroyed = true;
        units[0].groups = 1 << 3;
        let input = UnitBarInput {
            units,
            ..UnitBarInput::default()
        };
        let mut leaves = 0;
        for _ in 0..30 {
            let (out, _) = tick(&mut bar, &input, 0.5, &mut tex);
            leaves += out
                .intents
                .iter()
                .filter(|i| **i == UnitBarIntent::LeaveAllGroups(1))
                .count();
            if bar.slot_of(1).is_none() {
                break;
            }
        }
        assert_eq!(leaves, 1);
        assert_eq!(bar.slot_of(1), None);
        // the unit is still in the input, but it has its icon no more
        tick(&mut bar, &input, 0.5, &mut tex);
        assert_eq!(bar.slot_of(1), None);
        assert_eq!(bar.icon_count(), 2);
    }

    #[test]
    fn two_dead_side_by_side_leave_on_consecutive_updates() {
        // forcegroupbar.cpp:170-185: the removal deletes icons[t] and memmoves the next icon into
        // slot t, but the loop's ++t then moves past it. So the icon shifted into the freed slot
        // stays for one more update and leaves on the next.
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        // a warm-up update sorts the icons by tonnage: 3 (20t), 1 (50t), 2 (90t)
        let alive = UnitBarInput {
            units: lance(),
            ..UnitBarInput::default()
        };
        bar.update(&alive, &mut tex);
        let mut units = lance();
        units[0].destroyed = true; // id 1
        units[2].destroyed = true; // id 3
        let input = UnitBarInput {
            units,
            ..UnitBarInput::default()
        };
        // both dead start their death animations in the same update, so they finish together
        let mut before = bar.icon_count();
        for _ in 0..30 {
            tick(&mut bar, &input, 0.5, &mut tex);
            if bar.icon_count() < before {
                break;
            }
            before = bar.icon_count();
        }
        // one dead leaves; the other, shifted into its slot, is still on the bar
        assert_eq!(bar.icon_count(), 2);
        assert_eq!(bar.slot_of(1), Some(0));
        assert_eq!(bar.slot_of(3), None);
        assert_eq!(bar.slot_of(2), Some(1));
        tick(&mut bar, &input, 0.5, &mut tex);
        assert_eq!(bar.icon_count(), 1);
        assert_eq!(bar.slot_of(1), None);
        assert_eq!(bar.slot_of(3), None);
    }

    #[test]
    fn updates_without_a_render_advance_no_clock() {
        // the death, message and flash clocks run in the render (forcegroupbar.cpp:327-363,
        // mechicon.cpp:1003-1018), so two updates with no render leave them where they were: the
        // death animation does not finish, and no LeaveAllGroups is sent
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        bar.flash_jumpers(1);
        let mut units = lance();
        units[0].destroyed = true;
        units[0].groups = 1 << 3;
        units[2].pilot.message_playing = true;
        let input = UnitBarInput {
            units,
            ..UnitBarInput::default()
        };
        let mut intents = Vec::new();
        for _ in 0..2 {
            intents.extend(bar.update(&input, &mut tex).intents);
        }
        assert!(intents.is_empty(), "{intents:?}");
        assert_eq!(
            bar.icons[bar.slot_of(1).unwrap()].death_time,
            anim::DEATH_START
        );
        assert_eq!(bar.icons[bar.slot_of(3).unwrap()].msg_time, 0.0);
        assert_eq!(bar.flash_time, 0.0);
        assert_eq!(bar.flash_count, 1);
    }

    #[test]
    fn one_render_advances_each_clock_once_by_the_frame_length() {
        // the frame length is the render's ctx.dt; the clocks take it once, in the icon's render
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        bar.flash_jumpers(1);
        let mut units = lance();
        units[0].destroyed = true;
        units[2].pilot.message_playing = true;
        let input = UnitBarInput {
            units,
            ..UnitBarInput::default()
        };
        let (out, _) = tick(&mut bar, &input, 0.1, &mut tex);
        assert!(out.intents.is_empty(), "{:?}", out.intents);
        let dying = bar.slot_of(1).unwrap();
        assert_eq!(bar.icons[dying].death_time, anim::DEATH_START + 0.1);
        assert_eq!(bar.icons[bar.slot_of(3).unwrap()].msg_time, 0.1);
        assert_eq!(bar.flash_time, 0.1);
    }

    fn jumper(id: u32) -> UnitBarUnit {
        UnitBarUnit {
            can_jump: true,
            ..mech(id, 50)
        }
    }

    #[test]
    fn a_flashing_jumper_is_hidden_until_its_flash_time_passes_a_quarter_second() {
        // flashJumpers sets forceFlashTime to 0 (forcegroupbar.cpp:66-72). The icon is drawn only when
        // forceFlashTime > .25 (:327-336), and the frame length is added after the loop (:355-363).
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        bar.flash_jumpers(1);
        let input = UnitBarInput {
            units: vec![jumper(1)],
            ..UnitBarInput::default()
        };
        tick(&mut bar, &input, 0.3, &mut tex);
        assert!(
            bar.icons[0].hidden,
            "first frame: flash time 0 is not above .25"
        );
        assert_eq!(bar.flash_time, 0.3);
        tick(&mut bar, &input, 0.0, &mut tex);
        assert!(
            !bar.icons[0].hidden,
            "second frame: flash time 0.3 is above .25"
        );
    }

    #[test]
    fn a_hidden_dying_jumper_does_not_advance_its_death_animation() {
        // ForceGroupIcon::render is skipped for a hidden icon (forcegroupbar.cpp:327-336), and
        // drawDeathEffect runs inside render (mechicon.cpp:1016-1018), so its time stands still, and
        // its selection is not cleared (mechicon.cpp:1444 is inside drawDeathEffect).
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        bar.flash_jumpers(1);
        let mut dying = jumper(1);
        dying.destroyed = true;
        dying.selected = true;
        let input = UnitBarInput {
            units: vec![dying],
            ..UnitBarInput::default()
        };
        let (out, _) = tick(&mut bar, &input, 0.3, &mut tex);
        assert!(bar.icons[0].hidden);
        assert_eq!(bar.icons[0].death_time, anim::DEATH_START);
        assert!(out.intents.is_empty());
        // the flash is now above .25: the icon is drawn, and the death advances by this frame's 0.1
        let (out, _) = tick(&mut bar, &input, 0.1, &mut tex);
        assert!(!bar.icons[0].hidden);
        assert_eq!(bar.icons[0].death_time, anim::DEATH_START + 0.1);
        assert_eq!(out.intents, [UnitBarIntent::RemoveFromSelection(1)]);
    }

    #[test]
    fn a_hidden_jumper_does_not_advance_its_message_flash() {
        // the message timer is in ForceGroupIcon::render (mechicon.cpp:1003-1012), so a hidden icon's
        // msgPlayTime stays where it was
        let mut tex = TextureStore::new();
        let mut bar = bar(TexId(0));
        bar.flash_jumpers(1);
        let mut talking = jumper(1);
        talking.pilot.message_playing = true;
        let input = UnitBarInput {
            units: vec![talking],
            ..UnitBarInput::default()
        };
        tick(&mut bar, &input, 0.3, &mut tex);
        assert!(bar.icons[0].hidden);
        assert_eq!(bar.icons[0].msg_time, 0.0);
        tick(&mut bar, &input, 0.1, &mut tex);
        assert_eq!(bar.icons[0].msg_time, 0.1);
    }
}
