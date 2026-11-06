// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The in-mission objectives panel: `ControlGui::startObjectives`, `RenderObjectives`,
//! `renderObjective` and `renderPlayerStatus` (code/controlgui.cpp:469-478, 603-752, 756-786,
//! 827-933), with the bars and icons they place from `buttonlayout*.fit` ([`StaticInfo`],
//! mclib/utilities.cpp:171-354).
//!
//! The panel reads a plain [`ObjectivesView`]: the objectives as the sim evaluated them (status,
//! priority, flags, string ids) and the mission clock. It keeps no sim state and makes no sim
//! calls. Its only output besides drawing is the kill-marker click, sent through
//! [`UpdateCtx::play`].
//!
//! Not ported here, and not faked either:
//! * the multiplayer scoreboard: `MPStatsEntry::render` (code/mpstats.cpp) needs the `mpstats`
//!   layout and the sim's player records. [`player_status_layout`] gives the backdrop and the row
//!   slots the original computes, but nothing draws them: [`ObjectivesPanel::render`] returns
//!   [`PlayerStatusUnavailable`] in a multiplayer frame;
//! * the ranking call in `startObjectives` (controlgui.cpp:479-482), which belongs to the sim;
//! * `texturesRotated`: no objectives block sets it, so `StaticInfo` ignores it.

use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::{Blend, DrawList, GuiRect};
use crate::fit::FitExt;
use crate::font::text::{self, TextAttrs};
use crate::font::FontRef;
use crate::textures::TexId;
use crate::widgets::basic::font_width;
use mc2_formats::ini::FitIni;
use std::collections::HashSet;

/// `LOG_KILLMARKER` (mclib/sounds.h:115), played once per successful objective when its total
/// is revealed.
pub const LOG_KILLMARKER: i32 = 103;

/// The results must have run this long before the objectives start (controlgui.cpp:611).
pub const RESULTS_DELAY: f32 = 0.24;

/// An activated objective flashes for this many seconds (controlgui.cpp:834) at this rate
/// (controlgui.cpp:837).
const FLASH_SECONDS: f64 = 20.0;
const FLASH_HZ: f64 = 2.0;

/// `IDS_PRIMARY`, `IDS_SECONDARY` and `IDS_TOTAL` (test_scripts/res_conv/strings.res.h:1181,
/// 1457, 1109).
const IDS_PRIMARY: u32 = 40010;
const IDS_SECONDARY: u32 = 40011;
const IDS_TOTAL: u32 = 45034;

/// `MAX_MC_PLAYERS` (code/multplyr.h:101); the scoreboard has one header and this many rows.
pub const MAX_MC_PLAYERS: usize = 8;

const WHITE: u32 = 0xffff_ffff;
const BLACK: u32 = 0xff00_0000;
const DOTS_20: &str = "....................";

/// `objectiveMoveInfo` (controlgui.cpp:130-133). Both times are 0.0 in the original, so the
/// slide snaps (see [`ObjectivesPanel::slide_delta`]).
pub const OBJECTIVE_MOVE: [MoveInfo; 2] = [
    MoveInfo {
        time: 0.0,
        position: -640.0,
    },
    MoveInfo {
        time: 0.0,
        position: 0.0,
    },
];

/// `MoveInfo` (code/pausewindow.h:26-30): a time in seconds and a position in pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MoveInfo {
    pub time: f32,
    pub position: f32,
}

// ---------------------------------------------------------------------------------------------
// plain inputs

/// `objective_status_type` (code/objective.h:77-80).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ObjectiveStatus {
    #[default]
    Undetermined,
    Successful,
    Failed,
}

/// The text an objective shows: a literal, or a resource string id
/// (`LocalizedDescription`, objective.cpp:2049-2058).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectiveText {
    Literal(String),
    Resource(u32),
}

impl Default for ObjectiveText {
    fn default() -> Self {
        ObjectiveText::Literal(String::new())
    }
}

/// One objective as the panel needs it. `title` is carried for the app; the panel draws only
/// the description, as the original does (`LocalizedDescription`, controlgui.cpp:845, 866).
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectiveView {
    pub title: ObjectiveText,
    pub description: ObjectiveText,
    /// `Status(Team::home->objectives)`, evaluated by the sim.
    pub status: ObjectiveStatus,
    /// `Priority()`: 1 is a primary objective, anything else a secondary one.
    pub priority: u32,
    /// `ResourcePoints()`: the total shown for a successful objective.
    pub resource_points: u32,
    /// `IsActive()`.
    pub is_active: bool,
    /// `IsHiddenTrigger()`.
    pub is_hidden_trigger: bool,
    /// `ActivateOnFlag()`.
    pub activate_on_flag: bool,
    /// `ActivationTime()`, in mission seconds (`-1` until activated).
    pub activation_time: f64,
}

impl Default for ObjectiveView {
    /// The constructor defaults of `CObjective` (objective.cpp:1463-1488): primary, active.
    fn default() -> Self {
        ObjectiveView {
            title: ObjectiveText::default(),
            description: ObjectiveText::default(),
            status: ObjectiveStatus::Undetermined,
            priority: 1,
            resource_points: 0,
            is_active: true,
            is_hidden_trigger: false,
            activate_on_flag: false,
            activation_time: -1.0,
        }
    }
}

/// One player on the scoreboard. Rows come in display order (the caller applies `sortStats`,
/// code/mpstats.cpp:70).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRow {
    pub team: u32,
}

/// The multiplayer scoreboard: `mpStats[0]`'s rectangle and the rows (controlgui.cpp:756-786).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerStatusView {
    pub header: GuiRect,
    pub rows: Vec<PlayerRow>,
}

/// Everything the panel reads in one frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectivesView {
    pub objectives: Vec<ObjectiveView>,
    /// `mission->actualTime`, in seconds (a `float` in the original, mission.h:190).
    pub mission_time: f32,
    /// `renderStatusInfo`: the mission has ended and the results are on screen.
    pub render_status_info: bool,
    /// `resultsTime`, in seconds since the results started.
    pub results_time: f32,
    /// Set in a multiplayer game (`MPlayer` with a mission type other than `OTHER`): the
    /// scoreboard replaces the objectives (controlgui.cpp:664-667).
    pub player_status: Option<PlayerStatusView>,
}

// ---------------------------------------------------------------------------------------------
// static quads (StaticInfo)

/// `StaticInfo` (mclib/utilities.cpp): one textured quad, from a `.fit` block or set by hand.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaticInfo {
    /// `None` draws nothing (the original would draw texture handle 0).
    pub texture: Option<TexId>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// `u0, v0, u1, v1`, with the original's 0.1 texel offset.
    pub uv: [f32; 4],
}

impl StaticInfo {
    /// A quad without a texture at the given rectangle.
    pub fn at(x: f32, y: f32, width: f32, height: f32) -> StaticInfo {
        StaticInfo {
            x,
            y,
            width,
            height,
            ..StaticInfo::default()
        }
    }

    /// `StaticInfo::init` (utilities.cpp:253-334). Reads `XLocation`, `YLocation`, `Width`,
    /// `Height`, `FileName`, `UNormal`, `VNormal`, `UWidth`, `VHeight` from `block`. Leaves the
    /// file on `block`, as the original does, for the caller's reads after it.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni, block: &str) -> StaticInfo {
        let mut s = StaticInfo::default();
        if file.seek_block(block).is_err() {
            log::warn!("couldn't find static block {block}");
            return s;
        }
        s.x = file.long_or("XLocation", 0) as f32;
        s.y = file.long_or("YLocation", 0) as f32;
        s.width = file.long_or("Width", 0) as f32;
        s.height = file.long_or("Height", 0) as f32;
        let name = file.string("FileName").unwrap_or_default();
        s.texture = ld.art(&name);
        let tw = s.texture.map_or(0, |t| ld.assets.textures.width(t)) as f32;
        let u = file.long_or("UNormal", 0) as f32;
        let v = file.long_or("VNormal", 0) as f32;
        let uw = file.long_or("UWidth", 0) as f32;
        let vh = file.long_or("VHeight", 0) as f32;
        if tw > 0.0 {
            let off = 0.1 / tw;
            s.uv = [
                u / tw + off,
                v / tw + off,
                (u + uw) / tw + off,
                (v + vh) / tw + off,
            ];
        }
        s
    }

    /// `StaticInfo::setLocation`: moves the top-left corner, keeping the size (utilities.cpp:336).
    pub fn set_location(&mut self, x: f32, y: f32) {
        self.x = x;
        self.y = y;
    }

    /// `StaticInfo::move` (utilities.cpp:347).
    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.x += dx;
        self.y += dy;
    }

    /// The rectangle as integer UI coordinates.
    pub fn rect(&self) -> GuiRect {
        GuiRect::new(
            self.x as i32,
            self.y as i32,
            (self.x + self.width) as i32,
            (self.y + self.height) as i32,
        )
    }

    /// `StaticInfo::render` (utilities.cpp:171-180): one quad, alpha blended, white.
    pub fn draw(&self, dl: &mut DrawList) {
        let Some(tex) = self.texture else { return };
        let [u0, v0, u1, v1] = self.uv;
        dl.rect_quad(
            Some(tex),
            Blend::Alpha,
            (self.x, self.y, self.x + self.width, self.y + self.height),
            (u0, v0, u1, v1),
            WHITE,
        );
    }
}

// ---------------------------------------------------------------------------------------------
// the rules

/// The check or the cross next to a row (controlgui.cpp:907-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Check,
    Cross,
}

/// The description colour by status: failed `0xffc61c00`, successful `0xff41c700`, otherwise
/// `0xffffc600` (controlgui.cpp:830-831).
pub fn status_color(status: ObjectiveStatus) -> u32 {
    match status {
        ObjectiveStatus::Failed => 0xffc6_1c00,
        ObjectiveStatus::Successful => 0xff41_c700,
        ObjectiveStatus::Undetermined => 0xffff_c600,
    }
}

/// The icon a row shows: the check when successful, the cross when failed, none otherwise
/// (controlgui.cpp:907, 920).
pub fn icon_for(status: ObjectiveStatus) -> Option<Icon> {
    match status {
        ObjectiveStatus::Successful => Some(Icon::Check),
        ObjectiveStatus::Failed => Some(Icon::Cross),
        ObjectiveStatus::Undetermined => None,
    }
}

/// The description colour of `renderObjective`: the status colour, but white on the even
/// half-seconds of an activation flash (controlgui.cpp:827-843).
pub fn objective_color(obj: &ObjectiveView, mission_time: f32) -> u32 {
    let mut color = status_color(obj.status);
    // the float clock widens to double against `m_activationTime` (objective.h:665)
    let elapsed = f64::from(mission_time) - obj.activation_time;
    if obj.activate_on_flag && FLASH_SECONDS > elapsed {
        // `(int)(2.0 * elapsed)`: the C cast truncates toward zero, as `as` does
        let temp = (FLASH_HZ * elapsed) as i64;
        if temp % 2 == 0 {
            color = WHITE;
        }
    }
    color
}

/// The leader dots between a description and its total (controlgui.cpp:870-900). Returns the
/// number of dots drawn and the width they span. `None` when the dots have no width (the
/// original asserts `0 < dotWidth` here).
pub fn dot_leader(
    total_right: i32,
    total_width: i32,
    x: i32,
    desc_width: i32,
    dot_width: i32,
) -> Option<(usize, f32)> {
    if dot_width <= 0 {
        return None;
    }
    let dot_real = dot_width as f32 / 20.0;
    let total_length = (total_right - total_width - x - desc_width) as f32;
    let mut count = (total_length / dot_real) as i64;
    if count < 3 {
        count = 3;
    }
    let dots = (count - 2) as usize;
    Some((dots, dot_real * (count - 2) as f32))
}

/// The `printf("%ld", n)` the total string uses (`IDS_TOTAL` is `"Total:   %ld"`).
fn format_long(fmt: &str, n: i64) -> String {
    let Some(at) = fmt.find('%') else {
        return fmt.to_string();
    };
    let rest = &fmt[at + 1..];
    let spec = if rest.starts_with("ld") {
        2
    } else if rest.starts_with('d') {
        1
    } else {
        return fmt.to_string();
    };
    format!("{}{n}{}", &fmt[..at], &rest[spec..])
}

// ---------------------------------------------------------------------------------------------
// layout and listing

/// The values of `ControlGui`'s objectives block: `[Objectives]`, `[PrimaryObjHeader]`, the
/// bars and icons, and the font. `left` and `header_left` move with the slide.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelLayout {
    /// `OBJECTIVESLEFT`.
    pub left: i32,
    /// `OBJECTIVESTOP`: the top of the primary header.
    pub top: i32,
    /// `OBJECTIVEHEADERLEFT`.
    pub header_left: i32,
    /// `OBJECTIVESSKIP`: the step between rows and headers.
    pub item_skip: i32,
    /// `OBJEECTIVESHEADERSKIP`: from the last primary row to the secondary header.
    pub secondary_skip: i32,
    /// `OBJECTIVESTOTALRIGHT`: the right edge the totals align to.
    pub total_right: i32,
    /// `OBJECTIVEBOXSKIP`, read from `[PrimaryObjBox]`.
    pub box_skip: i32,
    /// `OBJECTIVECHECKSKIP`, read from `[PrimaryObjCheck]` (the cross uses it too).
    pub check_skip: i32,
    /// `guiFont`, from `ObjectiveFont` in `[Fonts]`.
    pub font: FontRef,
    /// `objectiveInfos[0..ObjStaticCount]`: the bars at the panel's edges.
    pub edges: Vec<StaticInfo>,
    pub box_frame: StaticInfo,
    pub check: StaticInfo,
    pub cross: StaticInfo,
}

/// One listed objective: where it is, its place in the list (the `objectiveCount` the reveal
/// compares against), and its row position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// Index into [`ObjectivesView::objectives`].
    pub objective: usize,
    /// Position among the listed rows, primaries first (`objectiveCount` / `objectiveNum`).
    pub order: usize,
    pub primary: bool,
    /// Top of the row.
    pub y: i32,
}

/// The rows and the header and total lines' heights.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub rows: Vec<Row>,
    pub primary_header_y: i32,
    pub secondary_header_y: i32,
    pub total_y: i32,
}

/// Where a scoreboard row goes and whether it shows its team's score.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerSlot {
    /// Index into [`PlayerStatusView::rows`].
    pub player: usize,
    /// The row's top, relative to the header (`i * (height + 4)`, controlgui.cpp:800).
    pub y: i32,
    /// The first row of each team shows the score (controlgui.cpp:499-503).
    pub show_score: bool,
}

/// The scoreboard's backdrop and row slots (controlgui.cpp:787-800).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerStatusLayout {
    pub backdrop: GuiRect,
    pub rows: Vec<PlayerSlot>,
}

/// The multiplayer scoreboard is not ported, so a frame that shows it cannot be drawn in full.
/// Returned by [`ObjectivesPanel::render`] after the shared bars are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerStatusUnavailable;

impl std::fmt::Display for PlayerStatusUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the multiplayer scoreboard rows are not ported (MPStatsEntry::render)")
    }
}

impl std::error::Error for PlayerStatusUnavailable {}

/// `renderPlayerStatus`'s geometry: the header grown by 5 on each side, `+ height + 4` per
/// shown row, the offset added before the `(long)` cast (controlgui.cpp:787-796), and at most
/// [`MAX_MC_PLAYERS`] rows (the loop at controlgui.cpp:496).
pub fn player_status_layout(view: &PlayerStatusView, x_delta: f32) -> PlayerStatusLayout {
    let h = view.header;
    let step = h.height() + 4;
    let shown = view.rows.len().min(MAX_MC_PLAYERS);
    let mut teams_seen = HashSet::new();
    let rows = (0..shown)
        .map(|player| PlayerSlot {
            player,
            y: (player as i32 + 1) * step,
            show_score: teams_seen.insert(view.rows[player].team),
        })
        .collect();
    let backdrop = GuiRect::new(
        (h.left as f32 - 5.0 + x_delta) as i32,
        h.top - 5,
        (h.right as f32 + 5.0 + x_delta) as i32,
        h.bottom + 5 + step * shown as i32,
    );
    PlayerStatusLayout { backdrop, rows }
}

// ---------------------------------------------------------------------------------------------
// the panel

/// The objectives panel (`ControlGui`'s objectives state and drawing).
pub struct ObjectivesPanel {
    layout: PanelLayout,
    moves: [MoveInfo; 2],
    /// `renderObjectives`.
    open: bool,
    /// `objectiveTime`, in seconds.
    objective_time: f32,
    /// `playedObjectiveClick`, by row order.
    played: Vec<bool>,
}

impl ObjectivesPanel {
    /// A closed panel over `layout`, with the shipped move table.
    pub fn new(layout: PanelLayout) -> ObjectivesPanel {
        ObjectivesPanel {
            layout,
            moves: OBJECTIVE_MOVE,
            open: false,
            objective_time: 0.0,
            played: Vec::new(),
        }
    }

    /// Replaces the move table (tests and modding; the shipped table is [`OBJECTIVE_MOVE`]).
    pub fn with_moves(mut self, moves: [MoveInfo; 2]) -> ObjectivesPanel {
        self.moves = moves;
        self
    }

    /// Reads the objectives block of a button layout: `[Fonts]` `ObjectiveFont`, `[Objectives]`,
    /// `[PrimaryObjHeader]`, `ObjStatic0..`, `[PrimaryObjBox]`, `[PrimaryObjCheck]`, `[PrimaryObjX]`
    /// (controlgui.cpp:2452-2484 and 2714-2722). `file` is left on the last block read.
    pub fn from_fit(ld: &mut Loader<'_>, file: &mut FitIni) -> Result<ObjectivesPanel, String> {
        file.seek_block("Fonts")
            .map_err(|e| format!("couldn't find the font block: {e}"))?;
        let font_id = file.long_or("ObjectiveFont", 0) as u32;
        let font = ld.font(font_id);
        file.seek_block("Objectives")
            .map_err(|e| format!("couldn't find the objective block: {e}"))?;
        let count = file.long_or("ObjStaticCount", 0);
        let left = file.long_or("ObjectiveLeft", 0) as i32;
        let item_skip = file.long_or("ObjectiveSkip", 0) as i32;
        let secondary_skip = file.long_or("SecondObjSkip", 0) as i32;
        let total_right = file.long_or("totalRight", 0) as i32;
        if file.seek_block("PrimaryObjHeader").is_err() {
            log::warn!("couldn't find the primary objective header block");
        }
        let header_left = file.long_or("HeaderLeft", 0) as i32;
        let top = file.long_or("HeaderTop", 0) as i32;
        let mut edges = Vec::new();
        for i in 0..count {
            edges.push(StaticInfo::from_fit(ld, file, &format!("ObjStatic{i}")));
        }
        let box_frame = StaticInfo::from_fit(ld, file, "PrimaryObjBox");
        // `objBoxSkip` is read from whichever block the file is on (controlgui.cpp:2479, 2481)
        let box_skip = file.long_or("objBoxSkip", 0) as i32;
        let check = StaticInfo::from_fit(ld, file, "PrimaryObjCheck");
        let check_skip = file.long_or("objBoxSkip", 0) as i32;
        let cross = StaticInfo::from_fit(ld, file, "PrimaryObjX");
        Ok(ObjectivesPanel::new(PanelLayout {
            left,
            top,
            header_left,
            item_skip,
            secondary_skip,
            total_right,
            box_skip,
            check_skip,
            font,
            edges,
            box_frame,
            check,
            cross,
        }))
    }

    pub fn layout(&self) -> &PanelLayout {
        &self.layout
    }

    /// `renderObjectives`: the panel's own open state, as the button shows it.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// `objectiveTime`, which the results screen reads (`resultsDone`, controlgui.cpp:2903).
    pub fn objective_time(&self) -> f32 {
        self.objective_time
    }

    /// `startObjectives(bStart)` (controlgui.cpp:469-476). Opening from closed starts the timer
    /// at 0; re-opening during the results sets it to the last move time.
    pub fn start(&mut self, start: bool, render_status_info: bool) {
        if !self.open {
            self.objective_time = 0.0;
        } else if render_status_info {
            self.objective_time = self.moves[self.moves.len() - 1].time;
        }
        self.open = start;
    }

    /// `renderObjectives = 0`, the objectives button's close (controlgui.cpp:1613-1619).
    pub fn close(&mut self) {
        self.open = false;
    }

    /// The horizontal slide for this frame (controlgui.cpp:626-649).
    ///
    /// The segment search checks `time[j] <= t < time[j+1]`. The shipped table has equal times,
    /// so it never matches and the else branch takes the last position unscaled:
    /// `delta = position - edgeX`. Inside a segment the positions scale by
    /// `screenWidth / 640`, interpolated linearly.
    pub fn slide_delta(&self, objective_time: f32, screen_width: f32, edge_x: f32) -> f32 {
        let mut t0 = 0.0;
        let mut t1 = 0.0;
        let mut p0 = 0.0;
        let mut p1 = 0.0;
        for pair in self.moves.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a.time <= objective_time && b.time > objective_time {
                t0 = a.time;
                t1 = b.time;
                p0 = a.position * screen_width / 640.0;
                p1 = b.position * screen_width / 640.0;
                break;
            }
        }
        if t1 != 0.0 {
            let d_t = objective_time - t0;
            let current = p0 + d_t * ((p1 - p0) / (t1 - t0));
            current - edge_x
        } else {
            self.moves[self.moves.len() - 1].position - edge_x
        }
    }

    /// The rows the panel lists, in list order with primaries first, and the header and total
    /// heights (controlgui.cpp:678-745). Only active, non-hidden objectives are listed.
    pub fn listing(&self, objectives: &[ObjectiveView]) -> Listing {
        let l = &self.layout;
        let listed = |o: &ObjectiveView| o.is_active && !o.is_hidden_trigger;
        let primaries: Vec<usize> = (0..objectives.len())
            .filter(|&i| listed(&objectives[i]) && objectives[i].priority == 1)
            .collect();
        let secondaries: Vec<usize> = (0..objectives.len())
            .filter(|&i| listed(&objectives[i]) && objectives[i].priority != 1)
            .collect();
        let mut rows = Vec::with_capacity(primaries.len() + secondaries.len());
        for (k, &objective) in primaries.iter().enumerate() {
            rows.push(Row {
                objective,
                order: rows.len(),
                primary: true,
                y: l.top + l.item_skip * (k as i32 + 1),
            });
        }
        let secondary_header_y = l.top + l.item_skip * primaries.len() as i32 + l.secondary_skip;
        for (j, &objective) in secondaries.iter().enumerate() {
            rows.push(Row {
                objective,
                order: rows.len(),
                primary: false,
                y: secondary_header_y + l.item_skip * (j as i32 + 1),
            });
        }
        let total_y = secondary_header_y + l.item_skip * (secondaries.len() as i32 + 1);
        Listing {
            rows,
            primary_header_y: l.top,
            secondary_header_y,
            total_y,
        }
    }

    /// The check or cross for `status` at row `y`, placed at `y + check_skip`, and the box is
    /// separate (controlgui.cpp:907-930).
    pub fn icon_quad(&self, status: ObjectiveStatus, y: i32) -> Option<(Icon, StaticInfo)> {
        let icon = icon_for(status)?;
        let base = match icon {
            Icon::Check => &self.layout.check,
            Icon::Cross => &self.layout.cross,
        };
        let mut quad = base.clone();
        quad.set_location(quad.x, y as f32 + self.layout.check_skip as f32);
        Some((icon, quad))
    }

    /// The row's box at `y + box_skip` (controlgui.cpp:897-905).
    pub fn box_quad(&self, y: i32) -> StaticInfo {
        let mut quad = self.layout.box_frame.clone();
        quad.set_location(quad.x, y as f32 + self.layout.box_skip as f32);
        quad
    }

    /// Advances the panel one frame: `RenderObjectives`' timer, slide and kill-marker clicks
    /// (controlgui.cpp:611-661, 689-698). Nothing advances while closed, or while the results are
    /// still coming in.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, view: &ObjectivesView) {
        if !(self.open || view.render_status_info) {
            return;
        }
        if view.render_status_info && view.results_time < RESULTS_DELAY {
            self.played.clear();
            return;
        }
        self.objective_time += ctx.dt;
        let screen_w = ctx.assets.screen.0 as f32;
        let edge_x = self.layout.edges.first().map_or(0.0, |e| e.x);
        let delta = self.slide_delta(self.objective_time, screen_w, edge_x);
        for edge in &mut self.layout.edges {
            edge.move_by(delta, 0.0);
        }
        if delta != 0.0 {
            // `long += float` truncates back to an integer
            self.layout.header_left = (self.layout.header_left as f32 + delta) as i32;
            self.layout.left = (self.layout.left as f32 + delta) as i32;
        }
        if view.player_status.is_some() {
            return;
        }
        for row in self.listing(&view.objectives).rows {
            let obj = &view.objectives[row.objective];
            if obj.status != ObjectiveStatus::Successful || !self.revealed(view, row.order) {
                continue;
            }
            if self.played.len() <= row.order {
                self.played.resize(row.order + 1, false);
            }
            if !self.played[row.order] {
                self.played[row.order] = true;
                ctx.play(LOG_KILLMARKER);
            }
        }
    }

    /// Draws the panel (controlgui.cpp:603-752 and 827-933). Draws nothing while closed or while
    /// the results are still coming in.
    ///
    /// In a multiplayer frame (`player_status` set) the edge bars are drawn, then the call returns
    /// [`PlayerStatusUnavailable`]: the objectives are replaced by the scoreboard, which is not
    /// ported, so no objective and no scoreboard backdrop is drawn.
    pub fn render(
        &self,
        ctx: &mut RenderCtx<'_>,
        view: &ObjectivesView,
    ) -> Result<(), PlayerStatusUnavailable> {
        if !(self.open || view.render_status_info) {
            return Ok(());
        }
        if view.render_status_info && view.results_time < RESULTS_DELAY {
            return Ok(());
        }
        for edge in &self.layout.edges {
            edge.draw(ctx.dl);
        }
        if view.player_status.is_some() {
            return Err(PlayerStatusUnavailable);
        }
        let assets = ctx.assets;
        let listing = self.listing(&view.objectives);
        let l = &self.layout;
        shadow_text(
            ctx,
            l.font,
            assets.string(IDS_PRIMARY),
            l.header_left,
            listing.primary_header_y,
            true,
            WHITE,
        );
        let mut sum: i64 = 0;
        for row in listing.rows.iter().filter(|r| r.primary) {
            let obj = &view.objectives[row.objective];
            let revealed = self.revealed(view, row.order);
            self.draw_objective(ctx, obj, row.y, view.mission_time, revealed);
            if revealed && obj.status == ObjectiveStatus::Successful {
                sum += i64::from(obj.resource_points);
            }
        }
        shadow_text(
            ctx,
            l.font,
            assets.string(IDS_SECONDARY),
            l.header_left,
            listing.secondary_header_y,
            true,
            WHITE,
        );
        for row in listing.rows.iter().filter(|r| !r.primary) {
            let obj = &view.objectives[row.objective];
            let revealed = self.revealed(view, row.order);
            self.draw_objective(ctx, obj, row.y, view.mission_time, revealed);
            if revealed && obj.status == ObjectiveStatus::Successful {
                sum += i64::from(obj.resource_points);
            }
        }
        if view.render_status_info {
            let total = format_long(assets.string(IDS_TOTAL), sum);
            let width = font_width(assets, l.font, &total);
            shadow_text(
                ctx,
                l.font,
                &total,
                l.total_right - width,
                listing.total_y,
                false,
                WHITE,
            );
        }
        Ok(())
    }

    /// `bTotal`: the totals show from the row's order once the timer passes it (controlgui.cpp:689).
    fn revealed(&self, view: &ObjectivesView, order: usize) -> bool {
        view.render_status_info && (order as f32) < self.objective_time
    }

    /// `renderObjective` (controlgui.cpp:827-933) for one row.
    fn draw_objective(
        &self,
        ctx: &mut RenderCtx<'_>,
        obj: &ObjectiveView,
        y: i32,
        mission_time: f32,
        revealed: bool,
    ) {
        let assets = ctx.assets;
        let l = &self.layout;
        let color = objective_color(obj, mission_time);
        let desc = match &obj.description {
            ObjectiveText::Literal(s) => s.as_str(),
            ObjectiveText::Resource(id) => assets.string(*id),
        };
        shadow_text(ctx, l.font, desc, l.left, y, true, color);
        if revealed {
            let amount = if obj.status == ObjectiveStatus::Successful {
                i64::from(obj.resource_points)
            } else {
                0
            };
            let total = amount.to_string();
            let width = font_width(assets, l.font, &total);
            shadow_text(ctx, l.font, &total, l.total_right - width, y, false, WHITE);
            let desc_width = font_width(assets, l.font, desc);
            let dot_width = font_width(assets, l.font, DOTS_20);
            if let Some((dots, span)) =
                dot_leader(l.total_right, width, l.left, desc_width, dot_width)
            {
                let x = ((l.total_right - width) as f32 - span) as i32;
                shadow_text(ctx, l.font, &".".repeat(dots), x, y, false, color);
            }
        }
        self.box_quad(y).draw(ctx.dl);
        if let Some((_, icon)) = self.icon_quad(obj.status, y) {
            icon.draw(ctx.dl);
        }
    }
}

/// `drawShadowText` (mclib/utilities.cpp:377-389): the shadow one pixel left and down, then the
/// text. The region is the whole screen, and `proportional` is the original's flag.
fn shadow_text(
    ctx: &mut RenderCtx<'_>,
    font: FontRef,
    s: &str,
    left: i32,
    top: i32,
    proportional: bool,
    color: u32,
) {
    let assets: &UiAssets = ctx.assets;
    let Some(f) = assets.font(font) else {
        return;
    };
    let (sw, sh) = ctx.screen();
    let region = GuiRect::new(0, 0, sw, sh);
    let attrs = |c| TextAttrs {
        color: c,
        size: font.size as f32,
        // `gos_TextSetAttributes(..., false, proportional, ...)`: no wrap (utilities.cpp:381)
        word_wrap: false,
        proportional,
        ..TextAttrs::default()
    };
    text::draw(ctx.dl, f, s, &attrs(BLACK), region, (left - 1, top + 1));
    text::draw(ctx.dl, f, s, &attrs(color), region, (left, top));
}
