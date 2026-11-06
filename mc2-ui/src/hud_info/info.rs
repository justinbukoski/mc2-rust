// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `code/infowindow.cpp`: the details panel for the selected unit.
//!
//! [`InfoWindow::layout`] walks the content the way `InfoWindow::drawScrollingStuff` does
//! (infowindow.cpp:592-915): one running `cur_y` goes down the icon, the weapon groups, the
//! equipment and the pilot block, and every row is emitted only where the C++ would draw it
//! (`curY > NAMETOP`). [`InfoWindow::render`] then adds the header, the health bar and the frames
//! in the order `InfoWindow::render` (infowindow.cpp:358-481) draws them. The scroll and drag rules
//! of `InfoWindow::update` (infowindow.cpp:483-590) run on plain [`InfoMouse`] values.
//!
//! Hi-res offsets are taken as zero: the layout is `buttonlayout1024.fit` at 1024x768.

use super::icon::{IconAtlases, InfoIcon};
use super::{HudOp, Metrics};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::font::FontRef;
use crate::textures::TextureStore;
use crate::unitbar::UnitBarUnit;
use crate::widgets::button::{Button, ButtonState};
use crate::widgets::object::Object;
use mc2_formats::ini::FitIni;

/// `IDS_NOUNIT` (resource.h:3494): the header text when no unit is selected.
const IDS_NOUNIT: u32 = 45033;
/// `IDS_NOPILOT` (resource.h:3493).
const IDS_NOPILOT: u32 = 45032;
/// `IDS_COMP_ABBR0` (resource.h:2702): a component's abbreviation is `IDS_COMP_ABBR0 + masterID`.
const IDS_COMP_ABBR0: u32 = 33000;
/// `IDS_MISSION_SHOTSLEFT` (resource.h:1496): `"  %ld ammo"` (string 30022).
const IDS_MISSION_SHOTSLEFT: u32 = 30022;
/// `IDS_GREEN` (resource.h:1639): the rank names are `IDS_GREEN + rank`, ACE is `IDS_ACE`.
const IDS_GREEN: u32 = 30112;
/// `IDS_ACE` (resource.h:1722).
const IDS_ACE: u32 = 30192;
/// `IDS_SPECIALTY` (resource.h:3293): `IDS_SPECIALTY + i` for specialty `i`.
const IDS_SPECIALTY: u32 = 41001;
/// `warrior.h:231`: `NUM_SPECIALTY_SKILLS` (the number of specialty flags a pilot carries).
pub const NUM_SPECIALTY_SKILLS: usize = 23;
/// `WARRIOR_RANK_ACE` (warrior.h): the rank whose name is not contiguous with the others.
const RANK_ACE: u8 = 4;
/// `infowindow.cpp:624`: `char disabledCount[60][2]`, so at most 60 groups are drawn.
const MAX_WEAPON_GROUPS: usize = 60;
/// `infowindow.cpp:621`: the weapon-group colours for short, medium and long range.
const WEAPON_COLOURS: [u32; 3] = [0xff6e_7c00, 0xff00_5392, 0xffa2_1600];
/// `infowindow.cpp:786`: ECM, probe, jump jet and sensor names.
const EQUIPMENT_COLOUR: u32 = 0xffc2_9b00;
/// `infowindow.cpp:1000`, `infowindow.cpp:831`, `infowindow.cpp:853`: the blue text colour.
const BLUE: u32 = 0xff00_5392;
/// `infowindow.cpp:354` and `:436`: the name text colour and the health bar frame.
const NAME_COLOUR: u32 = 0xff5c_96c2;
/// `infowindow.cpp:377`: the scroll thumb colour.
const SCROLL_THUMB: u32 = 0xff5c_96c2;
/// `infowindow.cpp:454`: black, and the dark outline of the boxes (`0xff002f55`).
const BLACK: u32 = 0xff00_0000;
const OUTLINE: u32 = 0xff00_2f55;
/// `dbasegui.h:85-87`: `SB_GREEN`, `SB_YELLOW` and `SB_RED` (alpha 0x6f).
const SB_GREEN: u32 = 0x6f00_ff00;
const SB_YELLOW: u32 = 0x6fff_ff00;
const SB_RED: u32 = 0x6fff_0000;
/// `infowindow.cpp:634`: `skill` indices for the two skill bars: `{ MWS_GUNNERY, MWS_PILOTING }`
/// (infowindow.cpp:842), with `MWS_PILOTING = 0`, `MWS_SENSORS = 1`, `MWS_GUNNERY = 2` (warrior.h).
const BAR_SKILLS: [usize; 2] = [2, 0];

/// The part of the HUD layout the window reads (`buttonlayout1024.fit`, `[InfoWindow]` and its
/// sub-blocks). Every field is in UI coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct InfoGeometry {
    /// `Environment.screenWidth`: `SCROLLAMOUNT = 3 * screenWidth / 640` (infowindow.cpp:77).
    pub screen_width: i32,
    /// `SkipBetweenSections` (`SECTIONSKIP`).
    pub section_skip: i32,
    /// `YLocation` and `Height` of `[InfoWindow]` (`INFOTOP`, `INFOHEIGHT`).
    pub info_top: i32,
    pub info_height: i32,
    /// `InfoHeaderBox*` (`NameRect`): the frame around the name.
    pub name_box: GuiRect,
    /// `NameLocation*` (`NAMELEFT`, `NAMETOP`, `NAMERIGHT`, `NAMEBOTTOM`).
    pub name_rect: GuiRect,
    /// `ScrollBar*` (`SCROLLLEFT`, `SCROLLTOP`, `SCROLLRIGHT`, `SCROLLBOTTOM`) and its colour.
    pub scroll_bar: GuiRect,
    pub scroll_color: u32,
    /// `ScrollContentBox*` (`SCROLLBOXLEFT`, `SCROLLBOXTOP`, `SCROLLBOXRIGHT`, `SCROLLBOXBOTTOM`).
    pub content_box: GuiRect,
    /// `HealthBar*` (`HEALTHLEFT`, `HEALTHTOP`, `HEALTHRIGHT`, `HEALTHBOTTOM`).
    pub health_rect: GuiRect,
    /// `DividerLeft`, `DividerRight` and `DividerColor`.
    pub divider_x: (i32, i32),
    pub divider_colour: u32,
    /// `PilotLeft`, `PilotRight`, `PilotHeight`, `PilotNameLeft`.
    pub pilot_x: (i32, i32),
    pub pilot_height: i32,
    pub pilot_name_left: i32,
    /// `MechIconHeight` (`MECHHEIGHT`).
    pub mech_icon_height: i32,
    /// `MechIconLeft`/`Right` and `MechIconBackLeft`/`Right` (`MECHLEFT`, `MECHRIGHT`,
    /// `MECHBACKLEFT`, `MECHBACKRIGHT`).
    pub mech_x: (i32, i32),
    pub mech_back_x: (i32, i32),
    /// `[SkillMeter]` `Left`/`Right` (`SKILLLEFT`, `SKILLRIGHT`), `Height`, `Skip`, `UnitWidth`
    /// and `NumberUnits` (`NUMBERSKILLBARS`).
    pub skill_x: (i32, i32),
    pub skill_height: i32,
    pub skill_skip: i32,
    pub skill_unit_width: i32,
    pub skill_bars: i32,
    /// `[Skill0]` to `[Skill6]`: the label boxes (`XLocation`, `Width`, `Height`).
    pub skill_labels: [SkillLabel; 7],
    /// `[ScrollButton]` `Min` (`SCROLLMIN`), `XLocation` (`SCROLLBUTTONX`), `Width` and `Height`.
    pub scroll_min: i32,
    pub scroll_button_x: i32,
    pub scroll_button_size: (i32, i32),
    /// `[InfoButton0]` (scroll up) and `[InfoButton1]` (scroll down) as `GuiRect`s.
    pub up_button: GuiRect,
    pub down_button: GuiRect,
}

/// One `[SkillN]` label box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkillLabel {
    pub x: i32,
    pub width: i32,
    pub height: i32,
}

impl InfoGeometry {
    /// Reads the `[InfoWindow]` family of `buttonlayout*.fit` as `InfoWindow::init`
    /// (infowindow.cpp:102-294) does, with the hi-res offsets at zero. Returns `None` when a
    /// block or key the window needs is missing.
    pub fn from_fit(file: &mut FitIni, screen_width: i32) -> Option<InfoGeometry> {
        file.seek_block("InfoWindow").ok()?;
        let (info_top, info_height) = (int(file, "YLocation")?, int(file, "Height")?);
        let section_skip = int(file, "SkipBetweenSections")?;
        let name_box = rect(file, "InfoHeaderBox")?;
        let name_rect = rect(file, "NameLocation")?;
        let scroll_bar = rect(file, "ScrollBar")?;
        let scroll_color = colour(file, "ScrollBarColor")?;
        // The fit calls the box's vertical extent `ScrollContentBoxBarTop/BarBottom` (infowindow.cpp:192-195).
        let content_box = GuiRect::new(
            int(file, "ScrollContentBoxLeft")?,
            int(file, "ScrollContentBoxBarTop")?,
            int(file, "ScrollContentBoxRight")?,
            int(file, "ScrollContentBoxBarBottom")?,
        );
        let health_rect = rect(file, "HealthBar")?;
        let divider_x = (int(file, "DividerLeft")?, int(file, "DividerRight")?);
        let divider_colour = colour(file, "DividerColor")?;
        let pilot_x = (int(file, "PilotLeft")?, int(file, "PilotRight")?);
        let pilot_height = int(file, "PilotHeight")?;
        let pilot_name_left = int(file, "PilotNameLeft")?;
        let mech_icon_height = int(file, "MechIconHeight")?;
        let mech_x = (int(file, "MechIconLeft")?, int(file, "MechIconRight")?);
        let mech_back_x = (
            int(file, "MechIconBackLeft")?,
            int(file, "MechIconBackRight")?,
        );

        file.seek_block("SkillMeter").ok()?;
        let skill_x = (int(file, "Left")?, int(file, "Right")?);
        let skill_height = int(file, "Height")?;
        let skill_skip = int(file, "Skip")?;
        let skill_unit_width = int(file, "UnitWidth")?;
        let skill_bars = int(file, "NumberUnits")?;

        let mut skill_labels = [SkillLabel {
            x: 0,
            width: 0,
            height: 0,
        }; 7];
        for (i, label) in skill_labels.iter_mut().enumerate() {
            file.seek_block(&format!("Skill{i}")).ok()?;
            *label = SkillLabel {
                x: int(file, "XLocation")?,
                width: int(file, "Width")?,
                height: int(file, "Height")?,
            };
        }

        file.seek_block("ScrollButton").ok()?;
        let scroll_min = int(file, "Min")?;
        let scroll_button_x = int(file, "XLocation")?;
        let scroll_button_size = (int(file, "Width")?, int(file, "Height")?);

        let up_button = button(file, "InfoButton0")?;
        let down_button = button(file, "InfoButton1")?;

        Some(InfoGeometry {
            screen_width,
            section_skip,
            info_top,
            info_height,
            name_box,
            name_rect,
            scroll_bar,
            scroll_color,
            content_box,
            health_rect,
            divider_x,
            divider_colour,
            pilot_x,
            pilot_height,
            pilot_name_left,
            mech_icon_height,
            mech_x,
            mech_back_x,
            skill_x,
            skill_height,
            skill_skip,
            skill_unit_width,
            skill_bars,
            skill_labels,
            scroll_min,
            scroll_button_x,
            scroll_button_size,
            up_button,
            down_button,
        })
    }
}

fn int(file: &FitIni, key: &str) -> Option<i32> {
    file.long(key).map(|v| v as i32)
}

fn colour(file: &FitIni, key: &str) -> Option<u32> {
    file.color(key)
}

/// `<prefix>Left/Top/Right/Bottom` as a `GuiRect`.
fn rect(file: &FitIni, prefix: &str) -> Option<GuiRect> {
    Some(GuiRect::new(
        int(file, &format!("{prefix}Left"))?,
        int(file, &format!("{prefix}Top"))?,
        int(file, &format!("{prefix}Right"))?,
        int(file, &format!("{prefix}Bottom"))?,
    ))
}

/// A `[InfoButtonN]` block as the rect its `ControlButton` covers (`XLocation`, `YLocation`,
/// `Width`, `Height`).
fn button(file: &mut FitIni, block: &str) -> Option<GuiRect> {
    file.seek_block(block).ok()?;
    let x = int(file, "XLocation")?;
    let y = int(file, "YLocation")?;
    Some(GuiRect::new(
        x,
        y,
        x + int(file, "Width")?,
        y + int(file, "Height")?,
    ))
}

/// One weapon of the unit (`inventory[numOther .. numOther + numWeapons]`, infowindow.cpp:644).
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponInfo {
    /// `masterID`: the component type. Its name is string `IDS_COMP_ABBR0 + master_id`.
    pub master_id: u8,
    /// `getWeaponRange()`: 0 short, 1 medium, 2 long.
    pub range: u8,
    /// True when the weapon uses no ammunition, or `useUnlimitedAmmo` is set (infowindow.cpp:670).
    pub unlimited_ammo: bool,
    /// `getWeaponShots(weapon)`.
    pub shots: i32,
    pub disabled: bool,
    /// `inventory.health`: a weapon counts as working only above zero.
    pub health: f32,
}

/// The pilot of a unit (`MechWarrior`, infowindow.cpp:813-907).
#[derive(Clone, Debug, PartialEq)]
pub struct PilotInfo {
    /// `active()`: a dead pilot shows `IDS_NOPILOT` instead of the name.
    pub active: bool,
    pub name: String,
    /// `getRank()`: 0 green to 4 ace.
    pub rank: u8,
    /// `getSkill(MWS_*)`, indexed by the skill: `[piloting, sensors, gunnery]`.
    pub skills: [u8; 3],
    /// `specialtySkills[i]`, `NUM_SPECIALTY_SKILLS` entries.
    pub specialties: Vec<bool>,
}

/// The selected unit as the window reads it. The app fills this from the sim each frame.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitInfo {
    /// Identity of the mover. A change of id resets the scroll (infowindow.cpp:311-325).
    pub id: u32,
    /// `getIfaceName()`.
    pub name: String,
    /// `isMech()`: vehicles have no pilot block and no sensor row.
    pub is_mech: bool,
    /// `getTeamId() == Team::home->getId()`.
    pub team_is_home: bool,
    /// `getContactStatus(home) == CONTACT_VISUAL`.
    pub contact_visual: bool,
    /// `getTeam() != NULL`.
    pub has_team: bool,
    /// `!Team::home->isEnemy(getTeam())`.
    pub team_not_enemy: bool,
    pub disabled: bool,
    pub destroyed: bool,
    /// `getAppearance()->barStatus`, clamped to 1 when drawn (infowindow.cpp:439).
    pub health: f32,
    /// The unit's weapons, in inventory order.
    pub weapons: Vec<WeaponInfo>,
    /// ECM, probe, jump jets and sensor: the `masterID` of each, or `None` (`count[]`,
    /// infowindow.cpp:763-768). The sensor is ignored unless `is_mech`.
    pub equipment: [Option<u8>; 4],
    pub pilot: PilotInfo,
}

/// `InfoWindow::setUnit` refuses a unit of another team that is not in visual contact and not
/// disabled (infowindow.cpp:304-309).
fn refused(u: &UnitInfo) -> bool {
    !u.team_is_home && !u.contact_visual && !u.disabled
}

/// Which scroll button a click came from (`SCROLLUP` and `SCROLLDOWN`, infowindow.h:22-23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoButton {
    Up,
    Down,
}

/// The mouse as the scroll logic reads it (`userInput` in infowindow.cpp:493-590), in UI
/// coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InfoMouse {
    pub x: i32,
    pub y: i32,
    /// `getMouseLeftButtonState() == MC2_MOUSE_DOWN` this frame.
    pub left_down: bool,
    /// `leftMouseReleased()` this frame.
    pub left_released: bool,
    /// `getMouseLeftHeld()`, in seconds.
    pub left_held: f32,
    /// `isLeftDrag()`.
    pub dragging: bool,
    /// `getMouseDragX/Y()`: where the press that started the drag was.
    pub drag_origin: (i32, i32),
}

impl Default for InfoMouse {
    fn default() -> Self {
        InfoMouse {
            x: 0,
            y: 0,
            left_down: false,
            left_released: false,
            left_held: 0.0,
            dragging: false,
            drag_origin: (0, 0),
        }
    }
}

impl InfoMouse {
    /// The mouse at `(x, y)` with no button state; the drag origin is the same point.
    pub fn at(x: i32, y: i32) -> InfoMouse {
        InfoMouse {
            x,
            y,
            drag_origin: (x, y),
            ..InfoMouse::default()
        }
    }
}

/// The fonts the window draws with: `InfoWndFont` for the name, `ComponentFont` for the rows
/// (`[Fonts]` of `buttonlayout*.fit`, infowindow.cpp:138-152).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InfoFonts {
    pub name: FontRef,
    pub component: FontRef,
}

/// The content ops of one layout pass and the `curY` it ends on (`end_y`).
#[derive(Clone, Debug, PartialEq)]
pub struct InfoLayout {
    pub ops: Vec<HudOp>,
    pub end_y: f32,
}

/// One group of weapons with the same `masterID` (infowindow.cpp:644-687).
#[derive(Clone, Debug, PartialEq)]
struct WeaponGroup {
    master_id: u8,
    range: u8,
    /// `None` is the C++ `-1`: no ammo row.
    ammo: Option<i32>,
    working: u32,
    total: u32,
}

/// `infowindow.cpp:644-687`: groups by `masterID` in first-seen order. The ammo comes from the
/// first weapon of a group (`ammo[i] += getWeaponShots`, run once per group), while a later
/// weapon of the same type only adds to the counts.
fn weapon_groups(weapons: &[WeaponInfo]) -> Vec<WeaponGroup> {
    let mut groups: Vec<WeaponGroup> = Vec::new();
    for w in weapons {
        let working = !w.disabled && w.shots > 0 && w.health > 0.0;
        if let Some(g) = groups.iter_mut().find(|g| g.master_id == w.master_id) {
            g.total += 1;
            g.working += u32::from(working);
        } else {
            groups.push(WeaponGroup {
                master_id: w.master_id,
                range: w.range,
                ammo: if w.unlimited_ammo {
                    None
                } else {
                    Some(w.shots)
                },
                working: u32::from(working),
                total: 1,
            });
        }
    }
    groups.truncate(MAX_WEAPON_GROUPS);
    groups
}

/// The details panel. Holds the selected unit, the scroll state and the drag state.
#[derive(Clone, Debug)]
pub struct InfoWindow {
    geom: InfoGeometry,
    fonts: InfoFonts,
    unit: Option<UnitInfo>,
    /// `scrollPos`, in pixels of content.
    scroll_pos: i32,
    /// `infoLength`: the content height below the scroll box, set once per unit (infowindow.cpp:913).
    info_length: f32,
    /// `lastYClick`: the scroll position when a thumb drag started (`None` is `-1`).
    last_y_click: Option<i32>,
    /// The unit's icon and portrait art (`InfoWindow::icon`), set by the app for the shown unit.
    icon: Option<InfoIcon>,
    /// `skillInfos[0..7]`: the skill label art (`[Skill0]` to `[Skill6]`).
    skill_art: Option<Box<[Object; 7]>>,
    atlases: Option<IconAtlases>,
    /// `buttons[0..2]`: the scroll up and down buttons' art (`[InfoButton0]`, `[InfoButton1]`).
    scroll_buttons: Option<Box<[Button; 2]>>,
    /// The scroll button the mouse is holding down.
    button_down: Option<InfoButton>,
}

impl InfoWindow {
    pub fn new(geom: InfoGeometry, fonts: InfoFonts) -> InfoWindow {
        InfoWindow {
            geom,
            fonts,
            unit: None,
            scroll_pos: 0,
            info_length: 0.0,
            last_y_click: None,
            icon: None,
            skill_art: None,
            atlases: None,
            scroll_buttons: None,
            button_down: None,
        }
    }

    /// Gives the window the skill label art (`InfoWindow::init`, infowindow.cpp:283-288).
    pub fn set_skill_art(&mut self, art: [Object; 7]) {
        self.skill_art = Some(Box::new(art));
    }

    /// Gives the window the scroll buttons' art (`ControlButton::initButtons(file, 2, ..., "InfoButton")`).
    pub fn set_scroll_buttons(&mut self, up: Button, down: Button) {
        self.scroll_buttons = Some(Box::new([up, down]));
    }

    /// Gives the window the atlases its icon and portrait come from.
    pub fn set_atlases(&mut self, atlases: IconAtlases) {
        self.atlases = Some(atlases);
    }

    /// Keeps the icon in step with the shown unit, as `setUnit` (a new icon for a new unit) and
    /// `InfoWindow::update` (`icon->update()`, infowindow.cpp:498) do. `bar` is the unit's icon
    /// data; a unit without it (or no unit) has no icon.
    pub fn sync_icon(&mut self, textures: &mut TextureStore, bar: Option<&UnitBarUnit>) {
        let (Some(atlases), Some(bar)) = (self.atlases, bar) else {
            self.icon = None;
            return;
        };
        match &mut self.icon {
            Some(icon) if icon.unit_id() == bar.id => icon.update(textures, bar),
            slot => *slot = Some(InfoIcon::new(textures, atlases, bar)),
        }
    }

    /// The icon of the shown unit (`InfoWindow::icon`); `None` clears it.
    pub fn set_icon(&mut self, icon: Option<InfoIcon>) {
        self.icon = icon;
    }

    pub fn icon(&self) -> Option<&InfoIcon> {
        self.icon.as_ref()
    }

    pub fn geometry(&self) -> &InfoGeometry {
        &self.geom
    }

    pub fn unit(&self) -> Option<&UnitInfo> {
        self.unit.as_ref()
    }

    pub fn scroll_pos(&self) -> i32 {
        self.scroll_pos
    }

    pub fn info_length(&self) -> f32 {
        self.info_length
    }

    pub fn last_y_click(&self) -> Option<i32> {
        self.last_y_click
    }

    /// `InfoWindow::setUnit` (infowindow.cpp:301-348). A refused unit leaves the window as it was.
    /// A different unit (by id) resets the scroll and the content length; the same unit has its
    /// data replaced, so health and counts follow the sim.
    pub fn set_unit(&mut self, unit: Option<UnitInfo>) {
        if unit.as_ref().is_some_and(refused) {
            return;
        }
        let changed = self.unit.as_ref().map(|u| u.id) != unit.as_ref().map(|u| u.id);
        self.unit = unit;
        if changed {
            self.scroll_pos = 0;
            self.info_length = 0.0;
        }
    }

    /// `InfoWindow::update` (infowindow.cpp:483-590): clicks on the scroll buttons, the thumb,
    /// the track and the drag.
    pub fn update(&mut self, mouse: &InfoMouse) {
        if self.unit.as_ref().is_some_and(refused) {
            self.set_unit(None);
            return;
        }
        let enabled = self.scroll_enabled();
        self.button_down = None;
        for (button, id) in [
            (self.geom.up_button, InfoButton::Up),
            (self.geom.down_button, InfoButton::Down),
        ] {
            if button.contains(mouse.x, mouse.y) && mouse.left_down && enabled {
                self.button_down = Some(id);
                self.handle_click(id);
                return;
            }
        }

        let g = &self.geom;
        let in_bar = |x: i32, y: i32| {
            g.scroll_bar.left < x
                && x < g.scroll_bar.right
                && g.scroll_bar.top < y
                && y < g.scroll_bar.bottom
        };
        if mouse.dragging {
            if let Some(last) = self.last_y_click {
                // infowindow.cpp:516-524
                let dy = mouse.y - mouse.drag_origin.1;
                self.set_scroll_pos(last + dy);
                return;
            }
        } else if in_bar(mouse.x, mouse.y)
            && enabled
            && in_bar(mouse.drag_origin.0, mouse.drag_origin.1)
        {
            // infowindow.cpp:537-582
            let thumb = self.thumb_height(self.info_length);
            let thumb_top = (self.scroll_pos + g.scroll_min) as f32;
            if (mouse.y as f32) > thumb_top && (mouse.y as f32) < thumb_top + thumb {
                self.last_y_click = Some(self.scroll_pos);
            } else if mouse.left_released || mouse.left_held > 0.5 {
                self.last_y_click = None;
                if mouse.y > g.up_button.bottom && mouse.y < g.down_button.top {
                    // Above the thumb pages up, below it pages down (infowindow.cpp:561-568).
                    let mut new_pos = self.scroll_pos as f32;
                    if (mouse.y as f32) < thumb_top {
                        new_pos = self.scroll_pos as f32 - thumb;
                    } else if (mouse.y as f32) > thumb_top + thumb {
                        new_pos = self.scroll_pos as f32 + thumb;
                    }
                    new_pos = new_pos.clamp(0.0, self.info_length.max(0.0));
                    self.set_scroll_pos(new_pos as i32);
                }
            }
        }
        if mouse.left_released {
            self.last_y_click = None;
        }
    }

    /// `InfoWindow::handleClick` (infowindow.cpp:917-930): one scroll step, `SCROLLAMOUNT`.
    fn handle_click(&mut self, id: InfoButton) {
        let amount = 3.0 * self.geom.screen_width as f32 / 640.0;
        let pos = self.scroll_pos as f32;
        let next = match id {
            InfoButton::Up => pos - amount,
            InfoButton::Down => pos + amount,
        };
        self.set_scroll_pos(next as i32);
    }

    /// `InfoWindow::setScrollPos` (infowindow.cpp:1002-1010). The range is the scroll range of
    /// the current content, and zero while scrolling is off.
    fn set_scroll_pos(&mut self, where_: i32) {
        let (enabled, max) = self.scroll_limits_for(self.info_length);
        let range = if enabled {
            (max - self.geom.scroll_min).max(0)
        } else {
            0
        };
        self.scroll_pos = where_.clamp(0, range);
    }

    /// `scrollBarLength` (infowindow.cpp:373): the track between the scroll buttons.
    pub fn scroll_bar_length(&self) -> i32 {
        let g = &self.geom;
        g.down_button.top - g.up_button.bottom - 4 - g.scroll_button_size.1
    }

    /// The thumb height for `info` pixels of content (infowindow.cpp:399-407).
    pub fn thumb_height(&self, info: f32) -> f32 {
        let g = &self.geom;
        let physical = (g.down_button.top - g.up_button.bottom) as f32;
        (physical * physical / (physical + info)).max(g.scroll_button_size.1 as f32)
    }

    /// `(enabled, SCROLLMAX)` for `info` pixels of content (infowindow.cpp:388-409). Disabled
    /// content gives `SCROLLMAX = 0`.
    pub fn scroll_limits_for(&self, info: f32) -> (bool, i32) {
        if info < self.scroll_bar_length() as f32 {
            return (false, 0);
        }
        let max = self.geom.down_button.top as f32 - self.thumb_height(info) - 2.0;
        (true, max as i32)
    }

    /// The scroll bar is live when a unit is shown and its content overflows.
    fn scroll_enabled(&self) -> bool {
        self.unit.is_some() && self.scroll_limits_for(self.info_length).0
    }

    /// `drawScrollingStuff`'s `offset` (infowindow.cpp:597-602).
    fn scroll_offset(&self) -> f32 {
        if self.info_length == 0.0 {
            return 0.0;
        }
        let (_, max) = self.scroll_limits_for(self.info_length);
        let span = (max - self.geom.scroll_min) as f32;
        if span == 0.0 {
            return 0.0;
        }
        -(self.info_length / span) * self.scroll_pos as f32
    }

    /// The content of the selected unit at the current scroll, as `drawScrollingStuff` draws it.
    /// The first call with a unit sets the content length (infowindow.cpp:913-914).
    pub fn layout(&mut self, m: &impl Metrics) -> InfoLayout {
        let Some(unit) = self.unit.clone() else {
            return InfoLayout {
                ops: Vec::new(),
                end_y: 0.0,
            };
        };
        let layout = self.layout_for(&unit, self.scroll_offset(), m);
        if self.info_length == 0.0 {
            // infowindow.cpp:913-914 with SCROLLTOP and SCROLLBOTTOM, the scroll bar's extent.
            let bar = self.geom.scroll_bar;
            self.info_length = layout.end_y - bar.top as f32 - (bar.bottom - bar.top) as f32;
        }
        layout
    }

    /// The content ops for `unit` with its rows moved up by `offset` pixels.
    pub fn layout_for(&self, unit: &UnitInfo, offset: f32, m: &impl Metrics) -> InfoLayout {
        let g = &self.geom;
        let skip = g.section_skip as f32;
        let line = m.line_height(self.fonts.component);
        let h = line as f32;
        let top = g.name_rect.top as f32;
        let bottom = g.name_rect.bottom as f32;
        let mut ops = Vec::new();
        let mut cur = g.content_box.top as f32 + skip + offset;

        // The unit icon (infowindow.cpp:606-617): the front and the rear cell.
        if let Some(icon) = &self.icon {
            if g.scroll_bar.top as f32 + offset > top {
                let h = g.mech_icon_height as f32;
                let rect = |x: (i32, i32)| [x.0 as f32, cur, x.1 as f32, cur + h];
                ops.extend(icon.unit_ops(rect(g.mech_x), rect(g.mech_back_x)));
            }
        }
        cur += g.mech_icon_height as f32 + skip;
        if cur > bottom {
            self.divider(&mut ops, cur);
        }
        cur += skip;

        // Weapons, one block per range class (infowindow.cpp:689-757).
        let groups = weapon_groups(&unit.weapons);
        for range in 0..3u8 {
            if !groups.iter().any(|gr| gr.range == range) {
                continue;
            }
            let colour = WEAPON_COLOURS[range as usize];
            for gr in groups.iter().filter(|gr| gr.range == range) {
                let name = m.string(IDS_COMP_ABBR0 + u32::from(gr.master_id));
                if cur > top {
                    let label = format!("{}/{}  {}", gr.working, gr.total, name);
                    ops.push(self.text(
                        label,
                        g.pilot_x.0,
                        cur,
                        g.scroll_bar.left - g.pilot_x.0,
                        line,
                        colour,
                    ));
                }
                cur += h;
                if let Some(ammo) = gr.ammo {
                    if cur > top {
                        let shots =
                            m.string(IDS_MISSION_SHOTSLEFT)
                                .replacen("%ld", &ammo.to_string(), 1);
                        ops.push(self.text(
                            shots,
                            g.pilot_x.0,
                            cur,
                            g.scroll_bar.left - g.pilot_x.0,
                            line,
                            colour,
                        ));
                    }
                    cur += h;
                }
            }
            cur += skip;
            if cur > bottom {
                self.divider(&mut ops, cur);
            }
            cur += skip;
        }

        // Equipment (infowindow.cpp:782-803): the sensor counts only for a mech.
        for (slot, master) in unit.equipment.iter().enumerate() {
            let Some(master) = master else { continue };
            if slot == 3 && !unit.is_mech {
                continue;
            }
            let name = m.string(IDS_COMP_ABBR0 + u32::from(*master)).to_string();
            if cur > top {
                ops.push(self.text(
                    name,
                    g.pilot_x.0,
                    cur,
                    g.scroll_bar.left - g.pilot_x.0,
                    line,
                    EQUIPMENT_COLOUR,
                ));
            }
            cur += h + skip;
        }
        if cur > bottom {
            self.divider(&mut ops, cur);
        }
        cur += skip;

        // Pilot (infowindow.cpp:810-910). Enemy, disabled and destroyed mechs show no pilot.
        if unit.has_team && unit.team_not_enemy && unit.is_mech && !unit.disabled && !unit.destroyed
        {
            self.pilot_block(unit, &mut cur, &mut ops, m);
        } else {
            cur += 10.0 * skip;
        }
        InfoLayout { ops, end_y: cur }
    }

    fn pilot_block(&self, unit: &UnitInfo, cur: &mut f32, ops: &mut Vec<HudOp>, m: &impl Metrics) {
        let g = &self.geom;
        let skip = g.section_skip as f32;
        let line = m.line_height(self.fonts.component);
        let top = g.name_rect.top as f32;
        let pilot = &unit.pilot;
        let (px0, px1) = g.pilot_x;
        let pilot_h = g.pilot_height as f32;

        if *cur > top {
            // The portrait, then its frame (infowindow.cpp:820-822).
            if pilot.active {
                let rect = [px0 as f32, *cur, px1 as f32, *cur + pilot_h];
                ops.extend(self.icon.as_ref().and_then(|i| i.pilot_op(rect)));
            }
            ops.push(HudOp::Outline {
                rect: GuiRect::new(px0, *cur as i32, px1 + 1, (*cur + pilot_h + 1.0) as i32),
                top_left: g.scroll_color,
                bottom_right: g.scroll_color,
            });
            let name = if pilot.active {
                pilot.name.clone()
            } else {
                m.string(IDS_NOPILOT).to_string()
            };
            // `top = curY + PILOTHEIGHT/2 - height/2` (infowindow.cpp:825), integer halves.
            let text_top = *cur + (g.pilot_height / 2) as f32 - (line / 2) as f32;
            let width = g.scroll_bar.left - g.pilot_name_left;
            ops.push(self.text(name, g.pilot_name_left, text_top, width, line, BLUE));
        }
        *cur += pilot_h + skip;
        *cur += skip;

        // Rank name and skill meters (infowindow.cpp:841-907).
        let rank_name = if pilot.rank != RANK_ACE {
            m.string(IDS_GREEN + u32::from(pilot.rank))
        } else {
            m.string(IDS_ACE)
        };
        if *cur > top {
            let width = g.scroll_bar.left - g.skill_x.0;
            ops.push(self.text(rank_name.to_string(), g.skill_x.0, *cur, width, line, BLUE));
        }
        let mut current = pilot.rank as usize;
        for j in 0..3usize {
            let label = g.skill_labels[current];
            let label_h = label.height as f32;
            if *cur > top {
                let (x0, y0) = (label.x as f32, *cur);
                let (x1, y1) = (x0 + label.width as f32, y0 + label_h);
                // The label frame: (x0 - .5, y0 - .5, x1 + 1.5, y1 + 1.5) (infowindow.cpp:873).
                ops.push(HudOp::Outline {
                    rect: GuiRect::new(
                        (x0 - 0.5) as i32,
                        (y0 - 0.5) as i32,
                        (x1 + 1.5) as i32,
                        (y1 + 1.5) as i32,
                    ),
                    top_left: OUTLINE,
                    bottom_right: OUTLINE,
                });
                // The label art: the `[SkillN]` quad with its y moved to this row (:861-879).
                if let Some(obj) = self.skill_art.as_ref().map(|a| &a[current]) {
                    if let Some(tex) = obj.texture {
                        let mut quad = obj.loc;
                        for v in &mut quad {
                            v.y = y0;
                        }
                        quad[1].y = y0 + label_h;
                        quad[2].y = y0 + label_h;
                        ops.push(HudOp::Tex { tex, quad });
                    }
                }
                if j != 0 {
                    let skill = pilot.skills[BAR_SKILLS[j - 1]];
                    ops.extend(self.skill_bar_ops(i32::from(skill), *cur, label_h));
                }
            }
            *cur += label_h + skip;
            current = j + 5;
        }

        for (i, has) in pilot.specialties.iter().enumerate() {
            if !*has {
                continue;
            }
            if *cur > top {
                let name = m.string(IDS_SPECIALTY + i as u32).to_string();
                let width = g.name_rect.right - g.name_rect.left;
                ops.push(self.text(name, g.name_rect.left, *cur, width, line, BLUE));
            }
            *cur += line as f32;
        }
    }

    /// `drawSkillBar` (infowindow.cpp:960-1000): the bar of `skill / NUMBERSKILLBARS` units, with
    /// its frame and its number.
    pub fn skill_bar_ops(&self, skill: i32, y: f32, height: f32) -> Vec<HudOp> {
        let g = &self.geom;
        let (left, right) = (g.skill_x.0 as f32, g.skill_x.1 as f32);
        let skip = g.skill_skip;
        let bar_count = skill / g.skill_bars;
        let (mut red, mut green, mut blue) = (0u32, 0u32, 0u32);
        if bar_count != 0 {
            red = ((205 / bar_count) as u32) << 16;
            green = (((234 - 83) / bar_count) as u32) << 8;
            blue = ((255 - 146) / bar_count) as u32;
        }
        let mut ops = Vec::new();
        // `(long)(left - SKILLSKIP + .5f)` and friends (infowindow.cpp:979): the frame.
        let outside = GuiRect::new(
            (left - skip as f32 + 0.5) as i32,
            (y - 0.5) as i32,
            (right + skip as f32 + 0.5) as i32,
            (y + height + 1.5) as i32,
        );
        ops.push(HudOp::Fill {
            rect: outside,
            argb: BLACK,
        });
        ops.push(HudOp::Outline {
            rect: outside,
            top_left: OUTLINE,
            bottom_right: OUTLINE,
        });

        let mut colour = BLUE;
        let mut bar = GuiRect::new(
            (left + skip as f32) as i32,
            (y + skip as f32 + 0.5) as i32,
            (left + skip as f32 + g.skill_unit_width as f32) as i32,
            (y + height - skip as f32 + 0.5) as i32,
        );
        for _ in 0..bar_count {
            ops.push(HudOp::Fill {
                rect: bar,
                argb: colour,
            });
            colour = colour
                .wrapping_add(red)
                .wrapping_add(green)
                .wrapping_add(blue);
            bar.left += g.skill_unit_width + 1;
            bar.right = bar.left + g.skill_unit_width;
        }

        ops.push(self.text(
            skill.to_string(),
            g.skill_x.1 + 2,
            y,
            g.scroll_bar.left - g.skill_x.1 - 2,
            g.skill_height,
            BLUE,
        ));
        ops
    }

    /// The header: the name in its box, or "Select a Unit" (infowindow.cpp:350-356, 469-473).
    pub fn header_ops(&self, unit: Option<&UnitInfo>, m: &impl Metrics) -> Vec<HudOp> {
        let g = &self.geom;
        let name = match unit {
            Some(u) => u.name.clone(),
            None => m.string(IDS_NOUNIT).to_string(),
        };
        let nr = g.name_rect;
        vec![
            HudOp::Fill {
                rect: GuiRect::new(nr.left, nr.top, nr.right, g.scroll_bar.top),
                argb: BLACK,
            },
            HudOp::Outline {
                rect: g.name_box,
                top_left: OUTLINE,
                bottom_right: OUTLINE,
            },
            HudOp::Text {
                text: name,
                font: self.fonts.name,
                x: nr.left,
                y: nr.top as f32,
                w: nr.right - nr.left,
                h: nr.bottom - nr.top,
                colour: NAME_COLOUR,
                align: 3,
            },
        ]
    }

    /// The health bar (infowindow.cpp:436-465): a dim track and the filled part, coloured green
    /// at half health or more, yellow above a fifth, red below.
    pub fn health_ops(&self, unit: Option<&UnitInfo>) -> Vec<HudOp> {
        let Some(u) = unit else { return Vec::new() };
        let g = &self.geom;
        let status = u.health.min(1.0);
        let colour = if status >= 0.5 {
            SB_GREEN
        } else if status > 0.2 {
            SB_YELLOW
        } else {
            SB_RED
        };
        let r = g.health_rect;
        let fill_right = r.left as f32 + (r.right - r.left) as f32 * status;
        vec![
            HudOp::Quad {
                x0: r.left as f32,
                y0: r.top as f32,
                x1: r.right as f32,
                y1: r.bottom as f32,
                argb: colour & 0xff7f_7f7f,
            },
            HudOp::Quad {
                x0: r.left as f32,
                y0: r.top as f32,
                x1: fill_right,
                y1: r.bottom as f32,
                argb: colour | 0xff00_0000,
            },
        ]
    }

    /// The whole window for this frame: content, the scroll thumb, header and health bar, and the
    /// frames, in the order `InfoWindow::render` draws them.
    pub fn render(&mut self, m: &impl Metrics) -> Vec<HudOp> {
        let mut ops = self.layout(m).ops;
        let g = self.geom.clone();
        ops.push(HudOp::Fill {
            rect: GuiRect::new(
                g.content_box.left,
                g.content_box.bottom,
                g.content_box.right,
                g.info_top + g.info_height,
            ),
            argb: BLACK,
        });
        if self.scroll_enabled() {
            // `buttons[i].render()` for the enabled scroll buttons (infowindow.cpp:365-368)
            let down = self.button_down;
            if let Some(buttons) = self.scroll_buttons.as_mut() {
                for (b, id) in buttons.iter_mut().zip([InfoButton::Up, InfoButton::Down]) {
                    b.state = if down == Some(id) {
                        ButtonState::Pressed
                    } else {
                        ButtonState::Enabled
                    };
                    b.make_uvs();
                    if let Some(tex) = b.obj.texture {
                        ops.push(HudOp::Tex {
                            tex,
                            quad: b.obj.loc,
                        });
                    }
                }
            }
            let thumb = self.thumb_height(self.info_length);
            let y0 = (g.scroll_min + self.scroll_pos) as f32;
            ops.push(HudOp::Quad {
                x0: g.scroll_button_x as f32,
                y0,
                x1: (g.scroll_button_x + g.scroll_button_size.0) as f32,
                y1: y0 + thumb,
                argb: SCROLL_THUMB,
            });
        }
        let unit = self.unit.clone();
        ops.extend(self.header_ops(unit.as_ref(), m));
        ops.extend(self.health_ops(unit.as_ref()));
        ops.push(HudOp::Outline {
            rect: g.content_box,
            top_left: g.scroll_color,
            bottom_right: g.scroll_color,
        });
        ops.push(HudOp::Outline {
            rect: g.scroll_bar,
            top_left: g.scroll_color,
            bottom_right: g.scroll_color,
        });
        ops
    }

    fn divider(&self, ops: &mut Vec<HudOp>, y: f32) {
        ops.push(HudOp::Divider {
            x0: self.geom.divider_x.0,
            x1: self.geom.divider_x.1,
            y,
            argb: self.geom.divider_colour,
        });
    }

    fn text(&self, text: String, x: i32, y: f32, w: i32, h: i32, colour: u32) -> HudOp {
        HudOp::Text {
            text,
            font: self.fonts.component,
            x,
            y,
            w,
            h,
            colour,
            align: 0,
        }
    }

    /// Test helper: sets the content length directly, as a layout of that height would.
    #[cfg(test)]
    pub(crate) fn set_info_length_for_test(&mut self, length: f32) {
        self.info_length = length;
    }

    /// Test helper: the scroll position as the window stores it.
    #[cfg(test)]
    pub(crate) fn set_scroll_for_test(&mut self, pos: i32) {
        self.set_scroll_pos(pos);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hud_info::test_support::{FixedMetrics, LINE};

    const M: FixedMetrics = FixedMetrics;

    /// `buttonlayout1024.fit` (1024x768, zero hi-res offsets): `[InfoWindow]` lines 2191-2221 and
    /// 2232-2250, `[SkillMeter]` and `[Skill0..6]`, `[ScrollButton]`, `[InfoButton0/1]`.
    fn geom() -> InfoGeometry {
        InfoGeometry {
            screen_width: 1024,
            section_skip: 5,
            info_top: 556,
            info_height: 208,
            name_box: GuiRect::new(9, 561, 206, 600),
            name_rect: GuiRect::new(10, 562, 205, 587),
            scroll_bar: GuiRect::new(185, 609, 204, 756),
            scroll_color: 0xff00_2f55,
            content_box: GuiRect::new(9, 607, 206, 758),
            health_rect: GuiRect::new(18, 588, 197, 594),
            divider_x: (11, 183),
            divider_colour: 0xff00_2f55,
            pilot_x: (16, 48),
            pilot_height: 46,
            pilot_name_left: 52,
            mech_icon_height: 48,
            mech_x: (15, 54),
            mech_back_x: (60, 99),
            skill_x: (87, 149),
            skill_height: 21,
            skill_skip: 1,
            skill_unit_width: 4,
            skill_bars: 10,
            skill_labels: [SkillLabel {
                x: 16,
                width: 65,
                height: 20,
            }; 7],
            scroll_min: 621,
            scroll_button_x: 187,
            scroll_button_size: (16, 14),
            up_button: GuiRect::new(187, 611, 203, 619),
            down_button: GuiRect::new(187, 748, 203, 756),
        }
    }

    fn fonts() -> InfoFonts {
        InfoFonts::default()
    }

    fn wpn(
        master_id: u8,
        range: u8,
        unlimited_ammo: bool,
        shots: i32,
        disabled: bool,
    ) -> WeaponInfo {
        WeaponInfo {
            master_id,
            range,
            unlimited_ammo,
            shots,
            disabled,
            health: 100.0,
        }
    }

    fn pilot(active: bool) -> PilotInfo {
        PilotInfo {
            active,
            name: "Mueller".into(),
            rank: 2,
            skills: [18, 0, 25], // MWS_PILOTING, MWS_SENSORS, MWS_GUNNERY
            specialties: vec![false; NUM_SPECIALTY_SKILLS],
        }
    }

    /// A home-team mech. `pilot_section` gives it a team that is not an enemy, so it shows its pilot.
    fn unit(weapons: Vec<WeaponInfo>, pilot_section: bool) -> UnitInfo {
        UnitInfo {
            id: 1,
            name: "Atlas".into(),
            is_mech: true,
            team_is_home: true,
            contact_visual: true,
            has_team: pilot_section,
            team_not_enemy: pilot_section,
            disabled: false,
            destroyed: false,
            health: 1.0,
            weapons,
            equipment: [None; 4],
            pilot: pilot(true),
        }
    }

    fn texts(ops: &[HudOp]) -> Vec<(String, f32, u32)> {
        ops.iter()
            .filter_map(|op| match op {
                HudOp::Text {
                    text, y, colour, ..
                } => Some((text.clone(), *y, *colour)),
                _ => None,
            })
            .collect()
    }

    fn dividers(ops: &[HudOp]) -> Vec<f32> {
        ops.iter()
            .filter_map(|op| match op {
                HudOp::Divider { y, .. } => Some(*y),
                _ => None,
            })
            .collect()
    }

    fn window() -> InfoWindow {
        InfoWindow::new(geom(), fonts())
    }

    /// A window with `unit` shown and its content length set (scroll tests need overflow).
    fn scrolled_window(info: f32) -> InfoWindow {
        let mut w = window();
        w.set_unit(Some(unit(vec![], true)));
        w.set_info_length_for_test(info);
        w
    }

    // ---- geometry and scrolling (infowindow.cpp:373-409, 77, 917-1010) ----------------------

    #[test]
    fn scroll_bar_and_thumb_arithmetic() {
        // infowindow.cpp:373: scrollBarLength = down.top - up.bottom - 4 - buttonHeight = 111.
        let w = window();
        assert_eq!(w.scroll_bar_length(), 111);
        // infowindow.cpp:399-409: physicalRange 129; info 400 gives a thumb of 129^2/529 px,
        // SCROLLMAX = 748 - thumb - 2 truncated to long = 714, so the range is 714 - 621 = 93.
        let (enabled, max) = w.scroll_limits_for(400.0);
        assert!(enabled);
        assert_eq!(max, 714);
        assert_eq!(max - w.geometry().scroll_min, 93);
        // infowindow.cpp:388-393: info shorter than the bar disables scrolling and sets max 0.
        assert_eq!(w.scroll_limits_for(39.0), (false, 0));
    }

    #[test]
    fn thumb_height_has_a_floor() {
        // infowindow.cpp:404-407: 129^2 / (129 + 400) = 31.457466..., and never below 14.
        let w = window();
        assert!((w.thumb_height(400.0) - 31.457_466).abs() < 0.000_01);
        assert_eq!(w.thumb_height(1.0e9), 14.0);
    }

    #[test]
    fn set_scroll_pos_clamps_to_range() {
        // infowindow.cpp:1002-1010.
        let mut w = scrolled_window(400.0);
        w.set_scroll_for_test(200);
        assert_eq!(w.scroll_pos(), 93);
        w.set_scroll_for_test(-3);
        assert_eq!(w.scroll_pos(), 0);
        w.set_scroll_for_test(50);
        assert_eq!(w.scroll_pos(), 50);
    }

    #[test]
    fn scroll_buttons_step_by_truncated_amount() {
        // infowindow.cpp:77 SCROLLAMOUNT = 3 * 1024 / 640 = 4.8; setScrollPos takes an int, so
        // the float is truncated toward zero (infowindow.cpp:922-926, 1002).
        let mut w = scrolled_window(400.0);
        w.update(&InfoMouse {
            left_down: true,
            ..InfoMouse::at(195, 748 + 2)
        }); // down button: 0 + 4.8 -> 4
        assert_eq!(w.scroll_pos(), 4);
        w.update(&InfoMouse {
            left_down: true,
            ..InfoMouse::at(195, 748 + 2)
        }); // 4 + 4.8 -> 8
        assert_eq!(w.scroll_pos(), 8);
        w.update(&InfoMouse {
            left_down: true,
            ..InfoMouse::at(195, 613)
        }); // up button: 8 - 4.8 = 3.2 -> 3
        assert_eq!(w.scroll_pos(), 3);
        w.update(&InfoMouse {
            left_down: true,
            ..InfoMouse::at(195, 613)
        }); // 3 - 4.8 = -1.8 -> -1 -> clamped to 0
        assert_eq!(w.scroll_pos(), 0);
    }

    #[test]
    fn thumb_press_then_drag_moves_scroll_by_mouse_delta() {
        // infowindow.cpp:548-551: pressing inside the thumb saves lastYClick = scrollPos;
        // 516-524: while dragging, scrollPos = lastYClick + (mouseY - dragStartY).
        let mut w = scrolled_window(400.0);
        w.set_scroll_for_test(50);
        // thumb spans 621 + 50 = 671 .. 702.46; press at y 676.
        let press = InfoMouse {
            left_down: true,
            ..InfoMouse::at(190, 676)
        };
        w.update(&press);
        assert_eq!(w.last_y_click(), Some(50));
        let drag = InfoMouse {
            x: 190,
            y: 686,
            dragging: true,
            drag_origin: (190, 676),
            ..InfoMouse::default()
        };
        w.update(&drag);
        assert_eq!(w.scroll_pos(), 60);
        let release = InfoMouse {
            left_released: true,
            ..InfoMouse::at(190, 686)
        };
        w.update(&release);
        assert_eq!(w.last_y_click(), None);
    }

    #[test]
    fn track_hold_below_thumb_pages_down_by_thumb_height() {
        // infowindow.cpp:552-580: held track below the thumb: newScroll = scrollPos + thumb,
        // clamped to 0..infoLength (577-578), then setScrollPos clamps again.
        let mut w = scrolled_window(400.0);
        let held = InfoMouse {
            left_held: 0.6,
            ..InfoMouse::at(190, 700)
        };
        w.update(&held);
        assert_eq!(w.scroll_pos(), 31); // 0 + 31.457 truncated
    }

    #[test]
    fn enemy_unit_is_refused_unless_visible() {
        // infowindow.cpp:304-309: a unit of another team that is not in visual contact and not
        // disabled is refused.
        let mut w = window();
        let mut hidden = unit(vec![], true);
        hidden.team_is_home = false;
        hidden.contact_visual = false;
        w.set_unit(Some(hidden.clone()));
        assert!(w.unit().is_none());
        hidden.contact_visual = true;
        w.set_unit(Some(hidden));
        assert!(w.unit().is_some());
    }

    #[test]
    fn new_unit_resets_scroll_and_length_but_refresh_keeps_them() {
        // infowindow.cpp:311-325: a different mover resets scrollPos and infoLength; the same
        // mover keeps them while its data is refreshed.
        let mut w = scrolled_window(400.0);
        w.set_scroll_for_test(50);
        let mut same = unit(vec![], true);
        same.health = 0.1;
        w.set_unit(Some(same));
        assert_eq!(w.scroll_pos(), 50);
        assert_eq!(w.info_length(), 400.0);
        let mut other = unit(vec![], true);
        other.id = 2;
        w.set_unit(Some(other));
        assert_eq!(w.scroll_pos(), 0);
        assert_eq!(w.info_length(), 0.0);
    }

    // ---- layout ------------------------------------------------------------------------------

    #[test]
    fn weapon_groups_and_rows_match_cpp() {
        // Inventory order: PPC (145, medium), LRM/15 (121, medium, 16 shots), Machine Gun (105,
        // short, 200 shots, disabled), second PPC (145, disabled). Names: string table 33000+id.
        // infowindow.cpp:644-687 groups by masterID in first-seen order; 702-757 draws each range
        // class j = 0 (short), 1 (medium) with a header row, a shots row if ammo != -1, then
        // SECTIONSKIP and a divider if curY > NAMEBOTTOM.
        let weapons = vec![
            wpn(145, 1, true, 9999, false),
            wpn(121, 1, false, 16, false),
            wpn(105, 0, false, 200, true),
            wpn(145, 1, true, 9999, true),
        ];
        let w = window();
        let full = w.layout_for(&unit(weapons, false), 0.0, &M);
        assert_eq!(
            texts(&full.ops),
            vec![
                ("0/1  Machine Gun".to_string(), 670.0, 0xff6e_7c00),
                ("  200 ammo".to_string(), 680.0, 0xff6e_7c00),
                ("1/2  PPC".to_string(), 700.0, 0xff00_5392),
                ("1/1  LRM/15".to_string(), 710.0, 0xff00_5392),
                ("  16 ammo".to_string(), 720.0, 0xff00_5392),
            ]
        );
        assert_eq!(dividers(&full.ops), vec![665.0, 695.0, 735.0, 740.0]);
        // 740 divider, +5, then the pilot block is absent: +10 * SECTIONSKIP = 795.
        assert_eq!(full.end_y, 795.0);
    }

    #[test]
    fn weapon_text_geometry_matches_cpp() {
        // infowindow.cpp:728: componentFont.render(text, COMPONENTLEFT, curY, SCROLLLEFT -
        // COMPONENTLEFT, height, color, 0, 0): x = PILOTLEFT 16, width 185 - 16 = 169.
        let w = window();
        let layout = w.layout_for(&unit(vec![wpn(105, 0, false, 200, false)], false), 0.0, &M);
        let rect = layout.ops.iter().find_map(|op| match op {
            HudOp::Text { text, x, w, h, .. } if text == "1/1  Machine Gun" => Some((*x, *w, *h)),
            _ => None,
        });
        assert_eq!(rect, Some((16, 169, LINE)));
    }

    #[test]
    fn pilot_section_and_skill_bars_match_cpp() {
        // No weapons. icon: +48+5 (infowindow.cpp:614), divider 665 (616), +5 (689), trailing
        // divider 670 (805-806), +5 (808) -> 675. Pilot block (811-908):
        // portrait box (16, 675, 49, 722) and name at top = 675 + 23 - 5 = 693 (825-831);
        // +46 +5 +5 -> 731; rank text "Veteran" (30114) at 731; skill labels 0, 5 and 6 at
        // 731, 756, 781 with 20 px height and 5 px skip; gunnery 25 -> two bars (drawSkillBar,
        // 972-974: barCount 2, red +102<<16, green +75<<8, blue +54); piloting 18 -> one bar.
        let w = window();
        let layout = w.layout_for(&unit(vec![], true), 0.0, &M);
        assert_eq!(
            texts(&layout.ops),
            vec![
                ("Mueller".to_string(), 693.0, 0xff00_5392),
                ("Veteran".to_string(), 731.0, 0xff00_5392),
                ("25".to_string(), 756.0, 0xff00_5392),
                ("18".to_string(), 781.0, 0xff00_5392),
            ]
        );
        assert_eq!(dividers(&layout.ops), vec![665.0, 670.0]);
        assert!(layout.ops.contains(&HudOp::Outline {
            rect: GuiRect::new(16, 675, 49, 722),
            top_left: 0xff00_2f55,
            bottom_right: 0xff00_2f55,
        }));
        // Label outline for gunnery at curY 756: (16 - .5, 756 - .5, 81 + 1.5, 776 + 1.5).
        assert!(layout.ops.contains(&HudOp::Outline {
            rect: GuiRect::new(15, 755, 82, 777),
            top_left: 0xff00_2f55,
            bottom_right: 0xff00_2f55,
        }));
        // Two bars for 25: colours 0xff005392, then 0xff005392 + 0x660000 + 0x4b00 + 0x36 =
        // 0xff669ec8: left 88 and 93, top 757, bottom 775.
        let fills: Vec<(GuiRect, u32)> = layout
            .ops
            .iter()
            .filter_map(|op| match op {
                HudOp::Fill { rect, argb } => Some((*rect, *argb)),
                _ => None,
            })
            .collect();
        assert!(fills.contains(&(GuiRect::new(88, 757, 92, 775), 0xff00_5392)));
        assert!(fills.contains(&(GuiRect::new(93, 757, 97, 775), 0xff66_9ec8)));
        // Outside box of drawSkillBar (infowindow.cpp:979): (86, 755, 150, 777), black fill.
        assert!(fills.contains(&(GuiRect::new(86, 755, 150, 777), 0xff00_0000)));
        // End of the pilot block: 806 (no divider after it).
        assert_eq!(layout.end_y, 806.0);
    }

    #[test]
    fn skill_bar_with_too_few_points_has_no_bars() {
        // infowindow.cpp:965: barCount = skill / NUMBERSKILLBARS = 7 / 10 = 0, so only the
        // outside box and the number are drawn.
        let w = window();
        let ops = w.skill_bar_ops(7, 700.0, 20.0);
        let bars = ops
            .iter()
            .filter(|op| {
                matches!(
                    op,
                    HudOp::Fill {
                        argb: 0xff00_5392,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(bars, 0);
        assert_eq!(texts(&ops), vec![("7".to_string(), 700.0, 0xff00_5392)]);
    }

    #[test]
    fn health_bar_colour_and_width() {
        // infowindow.cpp:437-465: >= 0.5 green, > 0.2 yellow, else red; the filled part is
        // HEALTHLEFT + (HEALTHRIGHT - HEALTHLEFT) * status. The background is the colour
        // masked with 0xff7f7f7f (line 454).
        let w = window();
        let mut u = unit(vec![], false);
        u.health = 0.35;
        let ops = w.health_ops(Some(&u));
        assert_eq!(
            ops[0],
            HudOp::Quad {
                x0: 18.0,
                y0: 588.0,
                x1: 197.0,
                y1: 594.0,
                argb: 0x6f7f_7f00,
            }
        );
        match ops[1] {
            HudOp::Quad { x1, argb, .. } => {
                assert!((x1 - 80.65).abs() < 0.01);
                assert_eq!(argb, 0xffff_ff00);
            }
            _ => panic!("expected the filled health quad"),
        }
        u.health = 0.5;
        assert_eq!(quad_colour(&w.health_ops(Some(&u))), 0xff00_ff00);
        u.health = 0.2;
        assert_eq!(quad_colour(&w.health_ops(Some(&u))), 0xffff_0000);
        u.health = 1.7; // clamped to 1
        match w.health_ops(Some(&u))[1] {
            HudOp::Quad { x1, .. } => assert_eq!(x1, 197.0),
            _ => panic!(),
        }
    }

    fn quad_colour(ops: &[HudOp]) -> u32 {
        match ops[1] {
            HudOp::Quad { argb, .. } => argb,
            _ => panic!(),
        }
    }

    #[test]
    fn no_unit_shows_select_a_unit_in_the_header() {
        // infowindow.cpp:470-472: IDS_NOUNIT = 45033 "Select a Unit" in the name box.
        if crate::strings::StringTable::builtin_or_skip().is_none() {
            return;
        }
        let w = window();
        let ops = w.header_ops(None, &M);
        assert!(texts(&ops).iter().any(|(t, _, _)| t == "Select a Unit"));
    }

    #[test]
    fn pilot_block_is_skipped_for_enemies_and_wrecks() {
        // infowindow.cpp:811-910: no pilot rows; curY += 10 * SECTIONSKIP instead.
        let w = window();
        let mut u = unit(vec![], true);
        u.disabled = true;
        let layout = w.layout_for(&u, 0.0, &M);
        assert!(texts(&layout.ops).iter().all(|(t, _, _)| t != "Mueller"));
        // 612 + 53 (icon) + 5 = 670 divider + 5 = 675 -> +50 = 725 (after the trailing divider).
        assert_eq!(layout.end_y, 725.0);
    }

    #[test]
    fn equipment_rows_follow_cpp() {
        // infowindow.cpp:782-803: ECM, probe, jump jets and sensor rows in that order, each one
        // line plus SECTIONSKIP, names from IDS_COMP_ABBR0 + masterID (105 = "Machine Gun",
        // string 33105), colour 0xffc29b00 (786). The sensor counts only for a mech (767).
        let w = window();
        let mut u = unit(vec![], false);
        u.equipment = [Some(105), None, None, Some(105)];
        // icon 612 + 53 + 5 = 670 (divider), no weapons; ECM at 670, sensor at 685 (670 + 10 + 5),
        // divider 700, +5 = 705, pilot absent: +50 = 755.
        let layout = w.layout_for(&u, 0.0, &M);
        assert_eq!(
            texts(&layout.ops),
            vec![
                ("Machine Gun".to_string(), 670.0, 0xffc2_9b00),
                ("Machine Gun".to_string(), 685.0, 0xffc2_9b00),
            ]
        );
        assert_eq!(layout.end_y, 755.0);
        // A vehicle has no sensor row: the ECM at 670, divider 685, +5 = 690, +50 = 740.
        u.is_mech = false;
        let layout = w.layout_for(&u, 0.0, &M);
        assert_eq!(
            texts(&layout.ops),
            vec![("Machine Gun".to_string(), 670.0, 0xffc2_9b00)]
        );
        assert_eq!(layout.end_y, 740.0);
    }

    #[test]
    fn info_length_is_set_by_the_first_layout_below_the_scroll_box() {
        // infowindow.cpp:913-914: infoLength = curY - SCROLLTOP - (SCROLLBOTTOM - SCROLLTOP);
        // the pilot layout ends at 806: 806 - 609 - (756 - 609) = 50.
        let mut w = window();
        w.set_unit(Some(unit(vec![], true)));
        let _ = w.layout(&M);
        assert_eq!(w.info_length(), 50.0);
    }

    #[test]
    fn render_puts_the_frames_after_the_content() {
        // infowindow.cpp:358-481: content first, then the black box below it, the thumb (none
        // here: the content is short), the header, the health bar and the two frames.
        let mut w = window();
        w.set_unit(Some(unit(vec![], true)));
        let ops = w.render(&M);
        let black = ops
            .iter()
            .position(|op| matches!(op, HudOp::Fill { argb: 0xff00_0000, rect } if rect.top == 758))
            .expect("the black box below the scroll content");
        let first_text = ops
            .iter()
            .position(|op| matches!(op, HudOp::Text { .. }))
            .unwrap();
        assert!(first_text < black);
        assert_eq!(
            ops.last(),
            Some(&HudOp::Outline {
                rect: GuiRect::new(185, 609, 204, 756),
                top_left: 0xff00_2f55,
                bottom_right: 0xff00_2f55,
            })
        );
    }

    #[test]
    fn fit_values_match_buttonlayout_1024() {
        // Reads the real 1024x768 layout when the game data is present; skips otherwise.
        let Ok(root) = std::env::var("MC2_DATA") else {
            return;
        };
        let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
        let mut fit = vfs
            .read_fit("data/art/buttonlayout1024.fit")
            .expect("read")
            .expect("buttonlayout1024.fit");
        let g = InfoGeometry::from_fit(&mut fit, 1024);
        assert_eq!(g, Some(geom()));
    }
}
