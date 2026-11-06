// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission tactical map (`code/gametacmap.cpp` and the `TacMap` base it uses in
//! `mclib/tacmap.cpp`).
//!
//! The widget is driven by plain data: the terrain colour thumbnail, a unit list with team,
//! position and selection, the camera's view corners and the mission bounds ([`TacMapInput`] and
//! [`TerrainBounds]). It never touches the simulation: the app fills the input each frame and
//! reads the results ([`TacMap::camera_target`], [`TacMap::last_move_order`]) back.
//!
//! | original | here |
//! |---|---|
//! | `land->realVerticesMapSide`, `land->worldUnitsMapSide` | [`TerrainBounds`] |
//! | the `.pak` colour-map packet (`MissionInterfaceManager::initTacMap`, missiongui.cpp:3085) | `terrain` texture + [`TacMap::set_terrain_size`] |
//! | the `.pak` building packet (`GameTacMap::initBuildings`, gametacmap.cpp:438-463) | [`TacMapInput::buildings`] |
//! | `ObjectManager` movers + `SensorManager` sensors (gametacmap.cpp:274-411) | [`TacMapInput::units`] |
//! | `eye->inverseProjectZ` of the screen corners (gametacmap.cpp:212-243) | [`TacMapInput::view_corners`] |
//! | `eye->setPosition` / `MissionInterfaceManager::doMove` | [`TacMap::camera_target`] / [`TacMap::last_move_order`] |
//! | `userInput` | [`crate::input::UiInput`] through [`UpdateCtx`] |
//! | `helpTextID = IDS_TACMAP_HELP` | [`UpdateCtx::out`] (id 45126) |

use crate::ctx::{Msg, RenderCtx, UpdateCtx};
use crate::draw::{Blend, DrawList, GuiRect, Vertex};
use crate::font::text::{self, TextAttrs};
use crate::font::FontRef;
use crate::input::UiInput;
use crate::screen::LogisticsScreen;
use crate::textures::ART_PATH;
use crate::widgets::basic::{font_height, font_width};

/// `IDS_OBJECTIVE_MARKER_FONT` (strings.res.h:1193, `arialBlack9.d3f`), the font `CObjective::Read`
/// loads for the marker numerals (objective.cpp:1679-1683).
pub const OBJECTIVE_MARKER_FONT_ID: u32 = 45128;

/// `MAPCELL_DIM` (`mclib/terrain.h:57`).
const MAPCELL_DIM: f32 = 3.0;
/// `Terrain::worldUnitsPerVertex` (`mclib/terrain.cpp:76`).
const WORLD_UNITS_PER_VERTEX: f32 = 128.0;
/// `Terrain::metersPerWorldUnit` (`mclib/terrain.cpp:64`: `metersPerWorldUnit = 0.2f`).
const METERS_PER_WORLD_UNIT: f32 = 0.2;
/// `Terrain::worldUnitsPerCell` (`mclib/terrain.cpp:77`: `worldUnitsPerVertex / MAPCELL_DIM`).
const WORLD_UNITS_PER_CELL: f32 = WORLD_UNITS_PER_VERTEX / MAPCELL_DIM;
/// `Terrain::metersPerCell` (`mclib/terrain.cpp:79`: `worldUnitsPerCell * metersPerWorldUnit`).
const METERS_PER_CELL: f32 = WORLD_UNITS_PER_CELL * METERS_PER_WORLD_UNIT;

/// The owner's team relative to the player's (`Team::home`), the input to the colour rules of
/// `GameTacMap::render`. The colour also depends on the pass the unit is drawn in, so see
/// [`TacTeam::colors`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TacTeam {
    /// Our commander's units.
    Ours,
    /// Another commander of the player's team.
    PlayerAlt,
    /// A team that is neutral to home (`isNeutral(Team::home)`, gametacmap.cpp:313).
    Neutral,
    /// A team that is an enemy of home (`isEnemy(Team::home)`, gametacmap.cpp:320).
    Enemy,
}

impl TacTeam {
    /// `(colorBlip, colorRing)` for a unit in its pass (gametacmap.cpp:309-325 for a non-mover
    /// sensor entry, 372-396 for a mover). The two passes disagree on some teams:
    ///
    /// - Sensor entry: neutral is blue (line 315), enemy red (line 322), and every other owner
    ///   green (line 309), including another commander's units.
    /// - Mover: ours is green (line 372), another commander is dark blue (line 382), and every
    ///   foreign contact is red (line 396), neutral teams included.
    ///
    /// The ring is the unselected colour in every branch (`colorRing` never takes the selected
    /// value); selection only brightens the blip.
    pub fn colors(self, sensor_entry: bool, selected: bool) -> (u32, u32) {
        let (unselected, selected_colour) = match (sensor_entry, self) {
            (true, TacTeam::Neutral) => (UNIT_COLOR_NEUTRAL, UNIT_COLOR_NEUTRAL_SELECTED),
            (true, TacTeam::Enemy) => (UNIT_COLOR_ENEMY, UNIT_COLOR_ENEMY_SELECTED),
            (true, TacTeam::Ours | TacTeam::PlayerAlt) => {
                (UNIT_COLOR_OURS, UNIT_COLOR_OURS_SELECTED)
            }
            (false, TacTeam::Ours) => (UNIT_COLOR_OURS, UNIT_COLOR_OURS_SELECTED),
            (false, TacTeam::PlayerAlt) => (UNIT_COLOR_PLAYER_ALT, UNIT_COLOR_PLAYER_ALT_SELECTED),
            (false, TacTeam::Neutral | TacTeam::Enemy) => {
                (UNIT_COLOR_ENEMY, UNIT_COLOR_ENEMY_SELECTED)
            }
        };
        let blip = if selected {
            selected_colour
        } else {
            unselected
        };
        (blip, unselected)
    }
}

pub const UNIT_COLOR_OURS: u32 = 0xff00_cc00; // gametacmap.cpp:309, 372
pub const UNIT_COLOR_OURS_SELECTED: u32 = 0xff4b_ff4b; // gametacmap.cpp:309, 372
pub const UNIT_COLOR_PLAYER_ALT: u32 = 0xff00_00cc; // gametacmap.cpp:382
pub const UNIT_COLOR_PLAYER_ALT_SELECTED: u32 = 0xff4b_4bff; // gametacmap.cpp:382
pub const UNIT_COLOR_NEUTRAL: u32 = 0xff00_00ff; // gametacmap.cpp:315
pub const UNIT_COLOR_NEUTRAL_SELECTED: u32 = 0xff4c_4cff; // gametacmap.cpp:315
pub const UNIT_COLOR_ENEMY: u32 = 0xffff_0000; // gametacmap.cpp:322
pub const UNIT_COLOR_ENEMY_SELECTED: u32 = 0xffff_3f3f; // gametacmap.cpp:322

/// `IDS_TACMAP_HELP` (`resource.h:3551`, used at gametacmap.cpp:81).
pub const IDS_TACMAP_HELP: i32 = 45126;

/// The map size the coordinate maps need (`land->realVerticesMapSide`, `land->worldUnitsMapSide`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainBounds {
    /// `realVerticesMapSide * MAPCELL_DIM`: the move-map cells across the mission.
    pub cells_per_side: i32,
    /// `worldUnitsMapSide = realVerticesMapSide * worldUnitsPerVertex` (terrain.cpp:370).
    pub map_side: f32,
}

impl TerrainBounds {
    /// Bounds of a map with `vertices_per_side` terrain vertices (`realVerticesMapSide`).
    pub fn from_vertices(vertices_per_side: i32) -> TerrainBounds {
        TerrainBounds {
            cells_per_side: vertices_per_side * MAPCELL_DIM as i32,
            map_side: vertices_per_side as f32 * WORLD_UNITS_PER_VERTEX,
        }
    }

    /// `mapTopLeft3d.x` (terrain.cpp:445).
    pub fn top_left_x(&self) -> f32 {
        -self.map_side / 2.0
    }
    /// `mapTopLeft3d.y` (terrain.cpp:446).
    pub fn top_left_y(&self) -> f32 {
        self.map_side / 2.0
    }
}

/// One unit to blip on the map: the presentation half of what `GameTacMap::render` reads from
/// `ObjectManager`'s movers and `SensorManager` (gametacmap.cpp:274-411). The app decides which
/// units appear (contact status, `isOnGUI`, shutdown, ECM, the `ShowMovers` debug flag) and hands
/// over the survivors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TacUnit {
    /// The owner's team: with the pass, the colours of the blip and ring ([`TacTeam::colors`]).
    pub team: TacTeam,
    /// `objClass == ARTILLERY` (gametacmap.cpp:327). Artillery keeps its owner's colours and
    /// blinks to invisible with the global `s_lastBlinkTime` timer ([`TacMap::blink_time`]).
    pub artillery: bool,
    /// `true` when the C++ lists this unit from `SensorManager`'s non-mover sensors
    /// (gametacmap.cpp:274-345) rather than from `ObjectManager`'s movers (lines 347-406). Those
    /// entries come first in the array, so they draw first (rings and blips alike), and they sit
    /// in the unselected blip pass whatever their owner's selection (`selected[count] = 0`,
    /// line 345). Selection still brightens the blip.
    pub sensor_entry: bool,
    /// World position (x, y); z is ignored, as in the C++ (`worldToTacMap` only reads x and y).
    pub pos: (f32, f32),
    pub selected: bool,
    /// `pSensor->getRange()` in metres; 0 draws no sensor ring. The ring uses the whole metres
    /// of this value (see [`TacMap::draw_sensor`]).
    pub sensor_range_m: f32,
}

/// One objective marker for the map: what `CObjective::RenderMarkers` reads (objective.cpp:2256-2303).
/// The app lists only objectives that draw: unresolved, active, `DisplayMarker` set and with marker
/// text (the NAV markers draw nothing).
#[derive(Clone, Debug, PartialEq)]
pub struct TacObjective {
    /// The objective's place in `Team::home->objectives` (the `count` of gametacmap.cpp:179-205), which the flash targets.
    pub index: usize,
    /// `m_markerX`, `m_markerY`, world coordinates.
    pub pos: (f32, f32),
    /// `m_markerText`: the primary number or the secondary letter.
    pub text: String,
}

/// What the app fills every frame.
#[derive(Clone, Debug, Default)]
pub struct TacMapInput {
    /// Building footprints, in tac-map pixels relative to the widget rect, exactly as the
    /// mission `.pak` packet stores them (`GameTacMap::initBuildings`, gametacmap.cpp:438-463).
    /// Parsed but never drawn: the C++ keeps `buildingPoints` and `GameTacMap::render` never
    /// reads them, so a nonempty list changes no pixels.
    pub buildings: Vec<(i32, i32)>,
    pub units: Vec<TacUnit>,
    /// The objective markers in `Team::home->objectives` order (gametacmap.cpp:178-205).
    pub objectives: Vec<TacObjective>,
    /// `Team::home->objectives.Count()`: the flash timer only runs while its objective is in this list.
    pub objective_total: usize,
    /// The camera's ground corners, world (x, y), in the C++'s `corners[0..4]` order: top-left,
    /// bottom-left, bottom-right, top-right (gametacmap.cpp:217-243). The two lower corners are
    /// inverse-projected at `nScreen.y = screenHeight * 0.6667f - 1` (lines 221 and 230), not at
    /// the bottom edge. Quad order is also this order, so the draw list needs no reordering.
    pub view_corners: [(f32, f32); 4],
    /// `ControlGui::isAddingAirstrike() && isButtonPressed(SENSOR_PROBE)` (gametacmap.cpp:91-92,
    /// `controlgui.h:235`): while the airstrike placement mode is armed, a left click orders a
    /// move (`doMove`) instead of moving the camera.
    pub placing_airstrike: bool,
}

/// The textures the original loads in `GameTacMap::init` (gametacmap.cpp:48-66) and
/// `EllipseElement::init` (cellip.cpp:103-108). All optional: without them the blips, rings and
/// view rect draw as flat quads.
#[derive(Clone, Debug, Default)]
pub struct TacMapArt {
    /// The terrain colour thumbnail (`textureHandle`, from the mission `.pak`).
    pub terrain: Option<crate::textures::TexId>,
    /// `data/art/viewingrect.tga` (`viewRectHandle`).
    pub view_rect: Option<crate::textures::TexId>,
    /// `data/art/blip.tga` (`blipHandle`).
    pub blip: Option<crate::textures::TexId>,
    /// `data/art/ring.tga` (the `EllipseElement` texture).
    pub ring: Option<crate::textures::TexId>,
    /// `bmpWidth`/`bmpHeight` from the colour-map TGA header (gametacmap.cpp:54-55).
    pub terrain_size: (u32, u32),
    /// `CObjective::s_markerFont`; unset draws no numerals.
    pub marker_font: FontRef,
}

impl TacMapArt {
    /// Loads `viewingrect.tga`, `blip.tga` and `ring.tga` from the VFS (the `artPath` names of
    /// gametacmap.cpp:58-65 and cellip.cpp:105).
    pub fn load(
        store: &mut crate::textures::TextureStore,
        vfs: &mut mc2_formats::vfs::Vfs,
    ) -> TacMapArt {
        TacMapArt {
            view_rect: store.load_tga(vfs, &format!("{ART_PATH}viewingrect.tga")),
            blip: store.load_tga(vfs, &format!("{ART_PATH}blip.tga")),
            ring: store.load_tga(vfs, &format!("{ART_PATH}ring.tga")),
            ..TacMapArt::default()
        }
    }

    /// The mission's colour-map thumbnail: the packet bytes as `GameTacMap::init` receives them
    /// (missiongui.cpp:3096-3102 hands them to `tacMap.init`), uploaded as the
    /// `gos_Texture_Solid` texture of gametacmap.cpp:57 and sized from the TGA header
    /// (`bmpWidth`/`bmpHeight`, lines 54-55). `None` (and size `(0, 0)`) when the bytes are not
    /// a decodable TGA; a failed packet also drops the thumbnail an earlier packet set, so the
    /// previous mission's map cannot be drawn.
    pub fn set_terrain_packet(
        &mut self,
        store: &mut crate::textures::TextureStore,
        name: &str,
        packet: &[u8],
    ) -> Option<crate::textures::TexId> {
        self.terrain = None;
        self.terrain_size = (0, 0);
        let mut img = mc2_assets::tga::decode(packet).ok()?;
        // the thumbnail is drawn `gos_Alpha_OneZero`, so its alpha channel (the shipped colour maps are
        // 32 bit with the water at alpha 0) is never read: without this the water draws black
        for px in img.pixels.chunks_exact_mut(4) {
            px[3] = 0xff;
        }
        self.terrain_size = (u32::from(img.width), u32::from(img.height));
        let id = store.insert(
            name,
            u32::from(img.width),
            u32::from(img.height),
            img.pixels,
        );
        self.terrain = Some(id);
        Some(id)
    }
}

/// The tactical map widget (`GameTacMap`).
#[derive(Clone, Debug)]
pub struct TacMap {
    /// An empty screen whose `Rect` part covers the tac-map rect: the HUD routes mouse events
    /// over it to the widget (`ControlGui::inRegion`'s inclusive rect pass,
    /// controlgui.cpp:1278-1285).
    pub screen: LogisticsScreen,
    pub bounds: TerrainBounds,
    pub art: TacMapArt,
    /// `useLeftRightMouseProfile` (`code/mechcmd2.cpp`, read at gametacmap.cpp:100): a right
    /// click orders a move.
    pub left_right_profile: bool,
    /// The camera's ground target (`eye->getPosition()`); a click outside the cell range keeps it
    /// (tacmap.cpp:49-53).
    pub camera_target: Option<(f32, f32)>,
    /// Set when a click moved the camera this frame (`eye->setPosition`, gametacmap.cpp:96-97).
    pub last_click: Option<(f32, f32)>,
    /// Set when a click ordered a move (`doMove`, gametacmap.cpp:93 or 109).
    pub last_move_order: Option<(f32, f32)>,
    /// `GameTacMap::s_lastBlinkTime` (gametacmap.cpp:25-26): the artillery blink timer. The
    /// sensor pass of [`TacMap::render`] adds `frameLength` for each artillery sensor and resets
    /// it when it passes `s_blinkLength = 0.5` (gametacmap.cpp:329-338).
    pub blink_time: f32,
    /// `GameTacMap::objectiveAnimationId` (gametacmap.cpp:43, 118): the 0-based objective that flashes, -1 before any
    /// `animate`, 0 once a flash has ended (as the C++ leaves it).
    objective_anim_id: i32,
    /// `objectiveFlashTime`.
    objective_flash_time: f32,
    /// `objectiveNumFlashes`.
    objective_num_flashes: i32,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Default for TacMap {
    fn default() -> TacMap {
        TacMap::new(TerrainBounds {
            cells_per_side: 0,
            map_side: 0.0,
        })
    }
}

impl TacMap {
    pub fn new(bounds: TerrainBounds) -> TacMap {
        TacMap {
            screen: LogisticsScreen::default(),
            bounds,
            art: TacMapArt::default(),
            left_right_profile: false,
            camera_target: None,
            last_click: None,
            last_move_order: None,
            blink_time: 0.0,
            objective_anim_id: -1,
            objective_flash_time: 0.0,
            objective_num_flashes: 0,
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        }
    }

    /// `GameTacMap::setPos` (gametacmap.cpp:553-559). The tac-map area is one of the control
    /// panel's `rectInfos` (`rectInfos[0]`, controlgui.cpp:2612-2615), so the pointer over it is
    /// taken by the UI: `ControlGui::inRegion`'s rect pass (controlgui.cpp:1278-1285) is
    /// inclusive (`left <= x && right >= x`), unlike `GameTacMap::inRegion` (gametacmap.h:44),
    /// which stays strict inside for the click and help-text logic. The rect is registered as a
    /// `Rect` part of the routing screen, which routes with the inclusive `aObject` bounds.
    ///
    /// Like the C++'s plain bounds assignment, a new position replaces the old one: the routing
    /// screen holds only this rect, so the area a previous layout gave the map stops taking the
    /// pointer.
    pub fn set_pos(&mut self, r: GuiRect) {
        self.left = r.left;
        self.top = r.top;
        self.right = r.right;
        self.bottom = r.bottom;
        self.screen.rects = vec![crate::widgets::Rect::new(
            r.left as f32,
            r.top as f32,
            (r.right - r.left) as f32,
            (r.bottom - r.top) as f32,
            0xff00_0000,
            false,
        )];
    }

    /// The tac-map rect (`GameTacMap::setPos`'s `GUI_RECT`).
    pub fn rect(&self) -> GuiRect {
        GuiRect::new(self.left, self.top, self.right, self.bottom)
    }

    /// The colour-map size (`bmpWidth`/`bmpHeight`, gametacmap.cpp:54-55).
    pub fn set_terrain_size(&mut self, width: u32, height: u32) {
        self.art.terrain_size = (width, height);
    }

    /// `GameTacMap::inRegion` (gametacmap.h:44): strict, exclusive borders.
    pub fn in_region(&self, x: i32, y: i32) -> bool {
        self.left < x && x < self.right && self.top < y && y < self.bottom
    }

    // -----------------------------------------------------------------------------------------
    // The coordinate maps (`mclib/tacmap.cpp`)
    // -----------------------------------------------------------------------------------------

    /// `TacMap::worldToTacMap` (mclib/tacmap.cpp:20-41) as `GameTacMap` calls it
    /// (gametacmap.cpp:434-437: `left`, `top`, `right - left`, `bottom - top`).
    ///
    /// `land->worldToCell` (terrain.h:346) with C truncation, clamped to
    /// `0..=realVerticesMapSide * MAPCELL_DIM` (tacmap.cpp:26-33), then scaled to the rect
    /// (tacmap.cpp:35-40).
    pub fn world_to_tac_map(&self, world_x: f32, world_y: f32) -> (f32, f32) {
        let cells = self.bounds.cells_per_side;
        // worldToCell (terrain.h:346-350): `(oneOverWorldUnitsPerVertex*3.0f)` in f32, then
        // `(int)` truncates toward zero.
        let k = (1.0 / WORLD_UNITS_PER_VERTEX) * 3.0;
        let mut tac_x = ((world_x - self.bounds.top_left_x()) * k) as i32;
        let mut tac_y = ((self.bounds.top_left_y() - world_y) * k) as i32;
        if tac_x < 0 {
            tac_x = 0;
        }
        if tac_y < 0 {
            tac_y = 0;
        }
        if tac_x > cells {
            tac_x = cells;
        }
        if tac_y > cells {
            tac_y = cells;
        }
        let x_size = (self.right - self.left) as f32;
        let y_size = (self.bottom - self.top) as f32;
        // tacmap.cpp:35-40 in the C++'s f32 order: `(float)tacX * (float)xSize / ((float)
        // realVerticesMapSide * MAPCELL_DIM)` then `+= xOffset`. A degenerate grid gives the
        // C++'s inf, which we keep as the clamp corner instead (an empty map draws nothing).
        let scale = |cell: i32, size: f32, offset: i32| -> f32 {
            if cells == 0 {
                return offset as f32;
            }
            cell as f32 * size / cells as f32 + offset as f32
        };
        (
            scale(tac_x, x_size, self.left),
            scale(tac_y, y_size, self.top),
        )
    }

    /// `TacMap::tacMapToWorld` (mclib/tacmap.cpp:43-61). `click` is the mouse position relative
    /// to the rect (gametacmap.cpp:85-86 subtract `left`/`top` first).
    ///
    /// `None` is the C++'s `world = eye->getPosition()` fallback (tacmap.cpp:52): the cell is
    /// outside the map, so the camera stays where it is. The world position is the *corner* of
    /// the cell (`cellColToWorldCoord`/`cellRowToWorldCoord`, terrain.cpp:275-279), not its
    /// centre.
    pub fn tac_map_to_world(&self, click_x: i32, click_y: i32) -> Option<(f32, f32)> {
        let cells = self.bounds.cells_per_side;
        let x_size = (self.right - self.left) as f32;
        let y_size = (self.bottom - self.top) as f32;
        if x_size <= 0.0 || y_size <= 0.0 || cells <= 0 {
            return None;
        }
        let cell_x = (click_x as f32 / x_size * cells as f32) as i32;
        let cell_y = (click_y as f32 / y_size * cells as f32) as i32;
        if cell_x < 0 || cell_x >= cells || cell_y < 0 || cell_y >= cells {
            return None;
        }
        // cellColToWorldCoord[cellX] / cellRowToWorldCoord[cellY] (terrain.cpp:275-279), built
        // with `worldUnitsPerCell = worldUnitsPerVertex / MAPCELL_DIM` (terrain.cpp:77)
        let x = cell_x as f32 * WORLD_UNITS_PER_CELL - self.bounds.map_side / 2.0;
        let y = self.bounds.map_side / 2.0 - cell_y as f32 * WORLD_UNITS_PER_CELL;
        Some((x, y))
    }

    /// The same map for an absolute UI point: subtract the rect origin (gametacmap.cpp:85-86).
    /// `None` is the C++'s `world = eye->getPosition()` fallback (tacmap.cpp:52): the camera
    /// stays where it is.
    fn click_to_world(&self, input: &UiInput) -> Option<(f32, f32)> {
        self.tac_map_to_world(input.mouse_x() - self.left, input.mouse_y() - self.top)
    }

    // -----------------------------------------------------------------------------------------
    // Update (`GameTacMap::update`, gametacmap.cpp:68-112)
    // -----------------------------------------------------------------------------------------

    /// One frame. Clears [`TacMap::last_click`] / [`TacMap::last_move_order`] and maps clicks to
    /// world positions. Returns the widget messages (there are none: the original sends no
    /// `handleMessage`).
    ///
    /// The click is the frame's `leftClick`/`rightClick` (userinput.cpp:171, 242, 304): the press
    /// edge for the left button, so a double click (`leftDoubleClick`, userinput.cpp:237) and a
    /// held button do not click. The right button is set on every held frame as well
    /// (userinput.cpp:327), which [`crate::input::UiInput::is_right_click`] mirrors.
    ///
    /// The rollover help text (gametacmap.cpp:78-81) is published *after* the region test, so a
    /// pointer outside the rect leaves [`crate::ctx::UiOutput::help_text_id`] untouched, as the
    /// original's early `return` does.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, input: &TacMapInput) -> Vec<Msg> {
        self.last_click = None;
        self.last_move_order = None;
        let (mx, my) = (ctx.input.mouse_x(), ctx.input.mouse_y());
        if !self.in_region(mx, my) {
            return Vec::new();
        }
        // ControlGui::setRolloverHelpText(IDS_TACMAP_HELP) (gametacmap.cpp:81)
        ctx.out.help_text_id = IDS_TACMAP_HELP;
        if ctx.input.is_left_click() {
            // gametacmap.cpp:91-98: with the airstrike probe armed a left click orders a move
            // (`doMove`); otherwise it moves the camera (`eye->setPosition`, setTarget(0)).
            if let Some(world) = self.click_to_world(ctx.input) {
                self.last_click = Some(world);
                if input.placing_airstrike {
                    self.last_move_order = Some(world);
                } else {
                    self.camera_target = Some(world);
                }
            }
        } else if self.left_right_profile && ctx.input.is_right_click() {
            // MissionInterfaceManager::doMove(world) (gametacmap.cpp:109)
            if let Some(world) = self.click_to_world(ctx.input) {
                self.last_move_order = Some(world);
            }
        }
        Vec::new()
    }

    // -----------------------------------------------------------------------------------------
    // Render (`GameTacMap::render`, gametacmap.cpp:128-432)
    // -----------------------------------------------------------------------------------------

    /// Records the map into the frame's draw list. The artillery blink timer advances here, as
    /// the C++'s does in the sensor pass of `render` (gametacmap.cpp:329-338).
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>, input: &TacMapInput) {
        // "Terrain not setup yet.  Left,Right,Top,Bottom are poopy!" (gametacmap.cpp:130-131)
        if self.bounds.cells_per_side <= 0 || self.right <= self.left || self.bottom <= self.top {
            return;
        }
        let colors = self.unit_colors(&input.units, ctx.dt);
        let rect = self.rect();
        ctx.dl.push_clip(rect);
        self.render_terrain(ctx.dl);
        // The objective numerals are drawn with the text region and no tac-map clip
        // (`drawShadowText`, utilities.cpp:382), between the terrain and the view rect.
        ctx.dl.pop_clip();
        self.render_objectives(ctx, input);
        ctx.dl.push_clip(rect);
        self.render_view_rect(ctx.dl, input);
        // The C++'s render-state switches between the passes (`gos_SetRenderState` around each
        // `gos_DrawQuads`, gametacmap.cpp:465-551, cellip.cpp:56-66) make the rings and the
        // blips separate draws; the state-merging batches cannot see that, so they are flushed
        // apart here.
        ctx.dl.flush_batch();
        self.render_units(ctx.dl, input, &colors);
        ctx.dl.pop_clip();
    }

    /// `CObjective::RenderMarkers` for the numeric markers (objective.cpp:2290-2301): the text is
    /// centred on the objective's tac-map position (`pos.x - width/2`, `pos.y - height/2`, the
    /// integer halves), white with a black shadow two pixels left and down (`drawShadowText`, offsets
    /// -2, +2), in the marker font at its own size.
    fn render_objectives(&mut self, ctx: &mut RenderCtx<'_>, input: &TacMapInput) {
        let flash = self.step_objective_flash(input, ctx.dt);
        let font = self.art.marker_font;
        let assets: &crate::ctx::UiAssets = ctx.assets;
        let Some(f) = assets.font(font) else {
            return;
        };
        let (sw, sh) = ctx.screen();
        let region = GuiRect::new(0, 0, sw, sh);
        let height = font_height(assets, font);
        let attrs = |color| TextAttrs {
            color,
            size: font.size as f32,
            word_wrap: false,
            proportional: true,
            ..TextAttrs::default()
        };
        for o in &input.objectives {
            if flash == Some(o.index) {
                continue;
            }
            let (x, y) = self.world_to_tac_map(o.pos.0, o.pos.1);
            let width = font_width(assets, font, &o.text);
            let left = (x - (width / 2) as f32) as i32;
            let top = (y - (height / 2) as f32) as i32;
            text::draw(
                ctx.dl,
                f,
                &o.text,
                &attrs(0xff00_0000),
                region,
                (left - 2, top + 2),
            );
            text::draw(ctx.dl, f, &o.text, &attrs(0xffff_ffff), region, (left, top));
        }
    }

    /// `GameTacMap::animate` (gametacmap.cpp:114-126), called with `abs(buttonId)` when a callout flashes a tac-map objective
    /// (`ControlGui::animateTacMap`). Only the first call takes effect, as in the C++. Returns whether it did.
    pub fn animate_objective(&mut self, objective_id: i32, flashes: i32) -> bool {
        if self.objective_anim_id != -1 {
            return false;
        }
        self.objective_anim_id = objective_id - 1;
        self.objective_flash_time = 0.0;
        self.objective_num_flashes = flashes;
        if flashes <= 0 {
            self.objective_anim_id = -1;
        }
        true
    }

    /// The flash block of the objective loop (gametacmap.cpp:184-205), run once per render: advances the timer while the
    /// animated objective is in the list and returns its index when its marker is hidden this frame (the first quarter second
    /// of each flash, and the frame the flash wraps). The marker is shown from 0.25 s to 0.5 s; when the last flash wraps the
    /// animation returns to idle (-1, where the C++ stores 0 and so never accepts another; DIFFERENCES.md) and the marker draws steadily again.
    fn step_objective_flash(&mut self, input: &TacMapInput, dt: f32) -> Option<usize> {
        let id = usize::try_from(self.objective_anim_id).ok()?;
        if id >= input.objective_total || self.objective_num_flashes == 0 {
            return None;
        }
        self.objective_flash_time += dt;
        let mut hidden = true;
        if self.objective_flash_time > 0.5 {
            self.objective_flash_time = 0.0;
            self.objective_num_flashes -= 1;
        } else if self.objective_flash_time > 0.25 {
            hidden = false;
        }
        if self.objective_num_flashes == 0 {
            self.objective_anim_id = -1;
        }
        hidden.then_some(id)
    }

    /// The terrain thumbnail: the colour map stretched over the rect with the UV window of
    /// gametacmap.cpp:160-167 (`2/bmpWidth .. 128/bmpWidth`, `2/bmpWidth .. 128/bmpHeight`),
    /// opaque (`gos_Alpha_OneZero`, line 135).
    fn render_terrain(&self, dl: &mut DrawList) {
        let (bw, bh) = (
            self.art.terrain_size.0.max(1) as f32,
            self.art.terrain_size.1.max(1) as f32,
        );
        let (l, t, r, b) = (
            self.left as f32,
            self.top as f32,
            self.right as f32,
            self.bottom as f32,
        );
        // gametacmap.cpp:154-167, read as the assignments are (the `corners[3]` lines execute
        // first, so corners[0] and corners[2] keep their 2/bmp values):
        //   corners[0] = (left,  top,    2/bmpWidth,  2/bmpWidth)
        //   corners[1] = (right, top,    128/bmpWidth, 2/bmpWidth)
        //   corners[2] = (left,  bottom, 2/bmpWidth,  128/bmpHeight)
        //   corners[3] = (right, bottom, 128/bmpWidth, 128/bmpHeight)
        let u0 = 2.0 / bw;
        let u1 = 128.0 / bw;
        let v0 = 2.0 / bw; // line 164: v from bmpWidth, as in the C++
        let v1 = 128.0 / bh;
        let white = 0xffff_ffff;
        let vx = |x, y, u, v| Vertex {
            x,
            y,
            u,
            v,
            argb: white,
        };
        // The C++ draws the two triangles (0,1,2) and (1,3,2); the draw list wants one quad in
        // its own top-left/bottom-left/bottom-right/top-right order, i.e. [c0, c2, c3, c1].
        let corners = [
            vx(l, t, u0, v0),
            vx(l, b, u0, v1),
            vx(r, b, u1, v1),
            vx(r, t, u1, v0),
        ];
        match self.art.terrain {
            Some(tex) => dl.quad(Some(tex), Blend::Opaque, corners),
            // No thumbnail loaded yet: a flat panel so the blips still read (the C++ always has
            // its texture by the time it renders).
            None => dl.rect_quad(
                None,
                Blend::Opaque,
                (l, t, r, b),
                (0.0, 0.0, 0.0, 0.0),
                0xff20_2020,
            ),
        }
    }

    /// The "little viewing rect" (gametacmap.cpp:212-272): the camera's ground corners through
    /// `worldToTacMap`, in [`TacMapInput::view_corners`] order, drawn as `viewingrect.tga` with
    /// the UVs of lines 249-252 (by corner index, as the C++ assigns them).
    fn render_view_rect(&self, dl: &mut DrawList, input: &TacMapInput) {
        let mut v: [Vertex; 4] = [Vertex::default(); 4];
        for (i, c) in input.view_corners.iter().enumerate() {
            let (x, y) = self.world_to_tac_map(c.0, c.1);
            v[i] = Vertex {
                x,
                y,
                u: 0.0,
                v: 0.0,
                argb: 0xffff_ffff,
            };
        }
        v[0].u = 0.078125;
        v[1].u = 0.078125;
        v[2].u = 0.99875;
        v[3].u = 0.99875;
        v[0].v = 0.078125;
        v[1].v = 0.99875;
        v[2].v = 0.99875;
        v[3].v = 0.078125;
        dl.quad(self.art.view_rect, Blend::Alpha, v);
    }

    /// Sensor rings first, then the blips (gametacmap.cpp:413-428: `drawSensor` for every entry
    /// in array order, then `drawBlip` in two passes). The array holds the sensor entries before
    /// the movers (lines 274-345 precede 347-406), so each group draws in that order.
    /// `colors` is [`TacMap::unit_colors`]' result, per unit.
    fn render_units(&self, dl: &mut DrawList, input: &TacMapInput, colors: &[(u32, u32)]) {
        for sensor_entry in [true, false] {
            for (u, (_, ring)) in input.units.iter().zip(colors) {
                if u.sensor_entry == sensor_entry {
                    self.draw_sensor(dl, u.pos, u.sensor_range_m, *ring);
                }
            }
        }
        // The blip passes (lines 417-428): sensor entries and unselected movers in the first, the
        // selected movers last. A selected sensor entry stays in the first pass, so an unselected
        // mover drawn over it lands on top, as in the C++.
        for pass in [0, 1, 2] {
            for (u, (blip, _)) in input.units.iter().zip(colors) {
                if blip_pass(u) == pass && *blip != 0 {
                    self.draw_blip(dl, u.pos, *blip);
                }
            }
        }
    }

    /// `colorBlip`/`colorRing` for each unit, including the artillery blink
    /// (gametacmap.cpp:309-345, 366-406). A colour of 0 means "skip the draw".
    ///
    /// The colours come from [`TacTeam::colors`], so the pass decides them (see there).
    ///
    /// The blink runs in the sensor pass, in unit order, once per artillery sensor whose range
    /// passes the pass's `getRange() < 1.1` filter (lines 316-317, 327-329): `s_lastBlinkTime`
    /// gains `frameLength`, and past `s_blinkLength = 0.5` (line 333) that artillery's colours
    /// are 0 and the timer resets to 0 (lines 335-337).
    fn unit_colors(&mut self, units: &[TacUnit], dt: f32) -> Vec<(u32, u32)> {
        units
            .iter()
            .map(|u| {
                let (blip, ring) = u.team.colors(u.sensor_entry, u.selected);
                if u.artillery && u.sensor_range_m >= 1.1 {
                    self.blink_time += dt;
                    if self.blink_time > BLINK_LENGTH {
                        self.blink_time = 0.0;
                        return (0, 0);
                    }
                }
                (blip, ring)
            })
            .collect()
    }

    /// `GameTacMap::drawSensor` (gametacmap.cpp:465-511): the sensor radius in metres, fudged by
    /// `metersPerCell * 6` above range 1, scaled to rect pixels by
    /// `(right-left) / (metersPerCell * realVerticesMapSide * 3)`, drawn as the `ring.tga`
    /// ellipse of `EllipseElement` (cellip.cpp:24-53) with radii `2 * trunc(radius + 0.5)`.
    fn draw_sensor(&self, dl: &mut DrawList, pos: (f32, f32), range_m: f32, color: u32) {
        // `ranges[]` is an `unsigned long` (gametacmap.cpp:277), filled from `getRange()` at lines
        // 344 and 405 and passed on at line 415: the range is cut to whole metres before the
        // ring is sized. A range of 108.9 m draws as 108 m (a 14 px ring here, not 16).
        let range_m = range_m as u32 as f32;
        if color == 0 || range_m <= 0.0 {
            return;
        }
        let real_vertices = self.bounds.cells_per_side as f32 / MAPCELL_DIM;
        let mut radius = range_m;
        if radius > 1.0 {
            radius += METERS_PER_CELL * 6.0; // "a little fudge" (line 472)
        }
        radius *= (self.right - self.left) as f32 / (METERS_PER_CELL * real_vertices * MAPCELL_DIM);
        // radii.x = 2.0f * float2long(radius + .5) (line 500); float2long truncates (mathfunc.h:115)
        let radii = 2.0 * (radius + 0.5) as i32 as f32;
        if radii <= 0.0 {
            return;
        }
        let (bx, by) = self.world_to_tac_map(pos.0, pos.1);
        // center = (float2long(sqare[1].x), float2long(sqare[1].y)), sqare[1] = (x, y + 1)
        // (lines 484-487, 498-499); float2long truncates
        let cx = bx.trunc();
        let cy = (by + 1.0).trunc();
        // EllipseElement (cellip.cpp:39-52): the ring texture over the bounding box
        dl.rect_quad(
            self.art.ring,
            Blend::Alpha,
            (
                cx - radii / 2.0,
                cy - radii / 2.0,
                cx + radii / 2.0,
                cy + radii / 2.0,
            ),
            (0.0, 0.0, 1.0, 1.0),
            color,
        );
    }

    /// `GameTacMap::drawBlip` (gametacmap.cpp:513-551): the `blip.tga` quad, `x -= 2.f` and
    /// `y -= 2.f` from the unit's tac-map pixel, `4.1f` across and down, UV `0 .. 0.250001`.
    fn draw_blip(&self, dl: &mut DrawList, pos: (f32, f32), color: u32) {
        let (bx, by) = self.world_to_tac_map(pos.0, pos.1);
        let x0 = bx - 2.0;
        let y0 = by - 2.0;
        dl.rect_quad(
            self.art.blip,
            Blend::Alpha,
            (x0, y0, x0 + 4.1, y0 + 4.1),
            (0.0, 0.0, 0.250001, 0.250001),
            color,
        );
    }
}

/// `GameTacMap::initBuildings` (gametacmap.cpp:438-463): a count, then `2 * count` int32s
/// (little-endian), the tac-map pixels relative to the rect (`pTmp->x = *pData++ + left`,
/// lines 452-453). A packet that ends inside an entry keeps the complete entries before it.
pub fn parse_buildings(data: &[u8]) -> Vec<(i32, i32)> {
    let word = |i: usize| -> Option<i32> {
        let b = data.get(i * 4..i * 4 + 4)?;
        Some(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let Some(count) = word(0) else {
        return Vec::new();
    };
    (0..count.max(0) as usize)
        .map_while(|i| Some((word(1 + 2 * i)?, word(2 + 2 * i)?)))
        .collect()
}

/// `GameTacMap::s_blinkLength` (gametacmap.cpp:25).
pub const BLINK_LENGTH: f32 = 0.5;

/// The blip pass a unit draws in (gametacmap.cpp:417-428): 0 for a sensor entry, which is always
/// unselected (line 345); 1 for an unselected mover; 2 for a selected mover.
fn blip_pass(u: &TacUnit) -> u8 {
    if u.sensor_entry {
        0
    } else if u.selected {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32 bit uncompressed TGA of one blue, fully transparent pixel (the shipped colour maps' water).
    fn water_tga() -> Vec<u8> {
        let mut t = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 1, 0, 32, 8];
        t.extend_from_slice(&[0x80, 0x50, 0x20, 0x00]);
        t
    }

    #[test]
    fn the_thumbnail_is_opaque_so_water_is_not_drawn_black() {
        let mut store = crate::textures::TextureStore::default();
        let mut art = TacMapArt::default();
        let id = art
            .set_terrain_packet(&mut store, "colmap", &water_tga())
            .expect("a decodable TGA");
        let px = &store.get(id).expect("uploaded").rgba[..4];
        assert_eq!(px[3], 0xff, "alpha of the water pixel");
        assert_eq!(&px[..3], &[0x20, 0x50, 0x80], "its colour is kept");
    }
}
