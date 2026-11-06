// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The pilot review's list (`code/pilotreviewarea.cpp`): `PilotListBox` with its
//! `DeadPilotListItem` (a box with the pilot's photo and skull, name, rank, missions and kills) and
//! `ActivePilotListItem` (photo, rank, gunnery and piloting meters with the gain flashing, the
//! kill markers as unit icons, the medals awarded and the promotion header), revealed in turn.

use crate::anim::Animation;
use crate::ctx::{Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::draw::GuiRect;
use crate::fit::FitExt;
use crate::font::FontRef;
use crate::logistics::attribute_meter::AttributeMeter;
use crate::logistics_dialog::format_name;
use crate::logistics_ids::ids;
use crate::mission_results::Reveal;
use crate::results_icons::{IconArt, DEAD_PILOT_INDEX};
use crate::textures::TexId;
use crate::unitbar::damage;
use crate::unitbar::{Loc, MechArmor, VehicleArmor, ICON_H, ICON_W};
use crate::widgets::{ItemState, ListBox, ListItem, Object, Rect, Text, TextListItem};
use mc2_campaign::pilot::{KilledKind, KilledUnit, MAX_MEDAL, RANK_ACE, RANK_GREEN};
use mc2_campaign::results::{PilotReview, ReviewedPilot};
use mc2_campaign::Logistics;
use mc2_formats::ini::FitIni;
use std::any::Any;
use std::rc::Rc;

/// `PilotListBox::update`: seconds before the first pilot is revealed.
pub const REVEAL_START_DELAY: f32 = 1.0;
/// `APPEARTIME`: how long a dead pilot's box takes to count as shown.
const APPEAR_TIME: f32 = 0.5;
/// `IDS_PILOT_NAME`, `IDS_PILOT_RANK`, `IDS_PILOT_MISSIONS`, `IDS_PILOT_KILLS`,
/// `IDS_KILLS_NO_COUNT`, `IDS_ONE_MEDAL_AWARD`, `IDS_ONE_MEDAL_AWARD2`, `IDS_PILOT_PROMOTED`,
/// `IDS_MEDAL`.
const IDS_PILOT_NAME: u32 = 27014;
const IDS_PILOT_RANK: u32 = 27015;
const IDS_PILOT_MISSIONS: u32 = 27016;
const IDS_PILOT_KILLS: u32 = 27017;
const IDS_KILLS_NO_COUNT: u32 = 27020;
const IDS_ONE_MEDAL_AWARD: u32 = 27058;
const IDS_ONE_MEDAL_AWARD2: u32 = 27059;
const IDS_PILOT_PROMOTED: u32 = 27060;
const IDS_MEDAL: u32 = 27061;

/// `sprintf` of the one `%d`/`%ld` a pilot review string has.
pub fn format_count(format: &str, n: i64) -> String {
    for spec in ["%ld", "%LD", "%d"] {
        if let Some(i) = format.find(spec) {
            return format!("{}{n}{}", &format[..i], &format[i + spec.len()..]);
        }
    }
    format.to_string()
}

/// The rank's name (`IDS_GREEN + rank`, the Ace's is not contiguous: `IDS_ACE`).
pub fn rank_text(assets: &UiAssets, rank: i32) -> String {
    let id = if rank == RANK_ACE {
        ids::ACE
    } else {
        ids::GREEN + rank.max(RANK_GREEN)
    };
    assets.string(id as u32).to_string()
}

/// What the dead pilots' boxes share (`DeadPilotListItem::init`).
pub struct DeadTemplate {
    area: Rect,
    live_icon: Rect,
    dead_icon: Rect,
    name: Text,
    rank: Text,
    missions: Text,
    kills: Text,
    art: IconArt,
}

impl DeadTemplate {
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni, art: IconArt) -> DeadTemplate {
        DeadTemplate {
            area: Rect::from_fit(file, "PilotReviewKIABox"),
            live_icon: Rect::from_fit(file, "KIABoxPilotIcon"),
            dead_icon: Rect::from_fit(file, "KIABoxDeadIcon"),
            missions: Text::from_fit(ld, file, "KIAMissionsText"),
            rank: Text::from_fit(ld, file, "KIARankText"),
            name: Text::from_fit(ld, file, "KIAPilotNameText"),
            kills: Text::from_fit(ld, file, "KIAKillsText"),
            art,
        }
    }
}

/// `DeadPilotListItem`: a pilot killed in the mission.
pub struct DeadCard {
    obj: Object,
    state: ItemState,
    t: Rc<DeadTemplate>,
    name: Text,
    rank: Text,
    missions: Text,
    kills: Text,
    photo: u32,
    /// `currentTime`.
    time: f32,
}

impl DeadCard {
    pub fn new(
        t: &Rc<DeadTemplate>,
        assets: &UiAssets,
        logistics: &Logistics,
        pilot: usize,
    ) -> Self {
        let a = &t.area.obj;
        let mut obj = Object::new(a.left(), a.top(), a.width(), a.height());
        obj.show = false;
        let (mut name, mut rank, mut missions, mut kills) = (
            t.name.clone(),
            t.rank.clone(),
            t.missions.clone(),
            t.kills.clone(),
        );
        let mut photo = 0;
        if let Some(p) = logistics.pilots().get(pilot) {
            photo = p.photo_index.max(0) as u32;
            missions.set_text(&format_count(
                assets.string(IDS_PILOT_MISSIONS),
                p.missions_completed as i64,
            ));
            let total = p.infantry_kills + p.mech_kills + p.vehicle_kills;
            kills.set_text(&format_count(assets.string(IDS_PILOT_KILLS), total as i64));
            name.set_text(&format_name(
                assets.string(IDS_PILOT_NAME),
                assets.string(p.name_id as u32),
            ));
            rank.set_text(&format_name(
                assets.string(IDS_PILOT_RANK),
                &rank_text(assets, p.rank),
            ));
        }
        DeadCard {
            obj,
            state: ItemState::Enabled,
            t: Rc::clone(t),
            name,
            rank,
            missions,
            kills,
            photo,
            time: 0.0,
        }
    }

    pub fn begin(&mut self) {
        self.time = 0.0;
    }

    pub fn step(&mut self, dt: f32) {
        self.time += dt;
    }

    pub fn is_done(&self) -> bool {
        self.time > APPEAR_TIME
    }

    pub fn name_text(&self) -> &str {
        &self.name.text
    }

    fn icon_rect(&self, rect: &Rect) -> (f32, f32, f32, f32) {
        let (gx, gy) = (self.obj.global_x() as f32, self.obj.global_y() as f32);
        (
            gx + rect.obj.left(),
            gy + rect.obj.top(),
            gx + rect.obj.right(),
            gy + rect.obj.bottom(),
        )
    }

    /// Where the pilot's photo is drawn.
    pub fn photo_rect(&self) -> (f32, f32, f32, f32) {
        self.icon_rect(&self.t.live_icon)
    }

    /// Where the skull is drawn.
    pub fn skull_rect(&self) -> (f32, f32, f32, f32) {
        self.icon_rect(&self.t.dead_icon)
    }
}

fn rect_f(r: GuiRect) -> (f32, f32, f32, f32) {
    (r.left as f32, r.top as f32, r.right as f32, r.bottom as f32)
}

impl ListItem for DeadCard {
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
    fn update(&mut self, _ctx: &mut UpdateCtx<'_>) {}
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        let (gx, gy) = (self.obj.global_x(), self.obj.global_y());
        self.obj.draw_quad(ctx);
        for text in [
            &mut self.name,
            &mut self.rank,
            &mut self.missions,
            &mut self.kills,
        ] {
            text.render_at(ctx, gx as f32, gy as f32);
        }
        let area = self.obj.global_rect();
        let c = self.t.area.color();
        ctx.dl.outline_rect(area, c, c);
        for (rect, index) in [
            (&self.t.live_icon, self.photo),
            (&self.t.dead_icon, DEAD_PILOT_INDEX),
        ] {
            let r = GuiRect::new(
                gx + rect.obj.left() as i32,
                gy + rect.obj.top() as i32,
                gx + rect.obj.right() as i32,
                gy + rect.obj.bottom() as i32,
            );
            self.t.art.pilot(ctx, index, rect_f(r));
            let c = rect.color();
            ctx.dl.outline_rect(r, c, c);
        }
    }
    fn text(&self) -> Option<&str> {
        Some(&self.name.text)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// What every active pilot's box shares (`ActivePilotListItem::init`).
pub struct ActiveTemplate {
    area: Rect,
    icon: Rect,
    kill_icon: Rect,
    outlines: Vec<Rect>,
    /// `ActiveDutyBoxSkill0/1`, then the rank icons: `icons[3 + rank]` in the C++ numbering.
    skill_icons: [Object; 2],
    rank_icons: Vec<Object>,
    meters: [AttributeMeter; 2],
    name: Text,
    missions: Text,
    rank: Text,
    gunnery: Text,
    piloting: Text,
    kills: Text,
    promotion: Text,
    medal_text: Text,
    medal_awarded: Text,
    medals: Vec<Object>,
    skill_anim: Animation,
    medal_anim: Animation,
    promoted_anim: Animation,
    /// `s_totalWidth`: the width of the line over each box.
    total_width: f32,
    art: IconArt,
}

impl ActiveTemplate {
    pub fn load(
        ld: &mut Loader<'_>,
        file: &mut FitIni,
        art: IconArt,
        list_width: f32,
    ) -> ActiveTemplate {
        let name = Text::from_fit(ld, file, "ActivePilotNameText");
        let missions = Text::from_fit(ld, file, "ActiveMissionsText");
        let rank = Text::from_fit(ld, file, "ActiveRankText");
        let gunnery = Text::from_fit(ld, file, "ActiveGunneryText");
        let piloting = Text::from_fit(ld, file, "ActivePilotingText");
        let kills = Text::from_fit(ld, file, "ActiveKillsNameText");
        let promotion = Text::from_fit(ld, file, "PromotionHeader");
        let medal_text = Text::from_fit(ld, file, "MedalDescriptionText");
        let medal_awarded = Text::from_fit(ld, file, "MedalAwardedHeader");
        let medals = (0..MAX_MEDAL)
            .map(|i| Object::from_fit(ld, file, &format!("MedalIcon{i}")))
            .collect();
        let outlines = (0..5)
            .map(|i| Rect::from_fit(file, &format!("ActiveDutyBoxRect{i}")))
            .collect();
        let skill_icons = [
            Object::from_fit(ld, file, "ActiveDutyBoxSkill0"),
            Object::from_fit(ld, file, "ActiveDutyBoxSkill1"),
        ];
        let mut meters = [
            AttributeMeter::from_fit(file, "ActiveDutyBoxSkillMeter0"),
            AttributeMeter::from_fit(file, "ActiveDutyBoxSkillMeter1"),
        ];
        for m in &mut meters {
            m.added_color_max = 0xffff_ffff;
            m.added_color_min = 0xffff_ffff;
        }
        let rank_icons = (0..5)
            .map(|i| Object::from_fit(ld, file, &format!("ActiveDutyBoxRankIcon{i}")))
            .collect();
        ActiveTemplate {
            area: Rect::from_fit(file, "ActiveDutyBox"),
            icon: Rect::from_fit(file, "ActiveDutyBoxPilotIconRect"),
            kill_icon: Rect::from_fit(file, "KillIcons"),
            outlines,
            skill_icons,
            rank_icons,
            meters,
            name,
            missions,
            rank,
            gunnery,
            piloting,
            kills,
            promotion,
            medal_text,
            medal_awarded,
            medals,
            skill_anim: Animation::from_block(file, "SkillMeterAnimation"),
            medal_anim: Animation::from_block(file, "MedalAwardedAnimation"),
            promoted_anim: Animation::from_block(file, "PilotPromotedHeaderAnimation"),
            total_width: list_width - 70.0,
            art,
        }
    }
}

/// One medal of the card: its icon, its description, and where they sit.
struct MedalEntry {
    icon: Object,
    text: Text,
}

/// `ActivePilotListItem`: a pilot still on active duty.
pub struct ActiveCard {
    obj: Object,
    state: ItemState,
    t: Rc<ActiveTemplate>,
    pub pilot: usize,
    photo: u32,
    name: Text,
    missions: Text,
    rank: Text,
    gunnery: Text,
    piloting: Text,
    kills: Text,
    promotion: Text,
    medal_awarded: Text,
    medals: Vec<MedalEntry>,
    meters: [AttributeMeter; 2],
    rank_icon: Object,
    /// The kill markers in kill order: whether it was a vehicle, and the unit.
    kill_icons: Vec<(bool, KilledUnit)>,
    /// The icon of each kill with its damage colours ([`ActiveCard::compose_kill_icons`]); a kill
    /// without one draws the atlas cell as it is.
    kill_images: Vec<Option<TexId>>,
    /// [`ActiveCard::compose_kill_icons`] has run.
    composed: bool,
    promoted: bool,
    pub reveal: Reveal,
    /// `promotionShown`: the promotion needs no more of the review (it is not one, or its
    /// pop-up has closed).
    promotion_shown: bool,
}

impl ActiveCard {
    /// `ActivePilotListItem::ActivePilotListItem`.
    pub fn new(
        t: &Rc<ActiveTemplate>,
        assets: &UiAssets,
        logistics: &Logistics,
        r: &ReviewedPilot,
    ) -> ActiveCard {
        let medal_count = r.new_medals.len();
        let kill_count = r.kills.len();
        let mut offset = 0.0;
        if kill_count > 32 {
            offset = ((kill_count - 32) / 8 + 1) as f32 * t.kill_icon.obj.height();
        }
        let mut medals = Vec::new();
        for (n, &m) in r.new_medals.iter().enumerate() {
            let Some(src) = t.medals.get(m) else { continue };
            let mut icon = src.clone();
            let mut text = t.medal_text.clone();
            let dy = n as f32 * icon.height() + offset;
            icon.move_by(0.0, dy);
            text.obj.move_by(0.0, dy);
            text.set_text(assets.string(IDS_MEDAL + m as u32));
            icon.show = false;
            text.obj.show = false;
            medals.push(MedalEntry { icon, text });
        }
        let mut promotion = t.promotion.clone();
        if let Some(last) = medals.last() {
            let left = promotion.obj.left();
            promotion.obj.move_to(left, last.icon.bottom() + 15.0);
        }
        let mut medal_awarded = t.medal_awarded.clone();
        medal_awarded.set_text(assets.string(if medal_count > 1 {
            IDS_ONE_MEDAL_AWARD2
        } else {
            IDS_ONE_MEDAL_AWARD
        }));
        let pilot = logistics.pilots().get(r.pilot);
        let rank = pilot.map_or(RANK_GREEN, |p| p.rank);
        promotion.set_text(&format_name(
            assets.string(IDS_PILOT_PROMOTED),
            &rank_text(assets, rank).to_uppercase(),
        ));
        // the box is as tall as what it shows: past the promotion header, or past the last medal
        let promotion_height = 34.0;
        let bottom = if r.promoted {
            promotion.obj.top() + font_line(assets, promotion.font) + promotion_height
        } else if let Some(last) = medals.last() {
            last.icon.bottom() + promotion_height
        } else {
            promotion.obj.top() + promotion_height
        };
        let obj = {
            let mut o = Object::new(30.0, t.area.obj.top(), t.area.obj.width(), bottom);
            o.show = false;
            o
        };
        let (mut name, mut missions, mut rank_text_obj, mut gunnery, mut piloting, mut kills) = (
            t.name.clone(),
            t.missions.clone(),
            t.rank.clone(),
            t.gunnery.clone(),
            t.piloting.clone(),
            t.kills.clone(),
        );
        let mut meters = t.meters.clone();
        if let Some(p) = pilot {
            missions.set_text(&format_count(
                assets.string(IDS_PILOT_MISSIONS),
                p.missions_completed as i64,
            ));
            name.set_text(&format_name(
                assets.string(IDS_PILOT_NAME),
                assets.string(p.name_id as u32),
            ));
            rank_text_obj.set_text(&rank_text(assets, p.rank));
            gunnery.set_text(&(p.gunnery as i64).to_string());
            piloting.set_text(&(p.piloting as i64).to_string());
            meters[0].set_value(p.gunnery / 100.0 - p.new_gunnery / 100.0);
            meters[1].set_value(p.piloting / 100.0 - p.new_piloting / 100.0);
            meters[0].set_added_value(p.new_gunnery / 100.0);
            meters[1].set_added_value(p.new_piloting / 100.0);
        }
        kills.set_text(assets.string(IDS_KILLS_NO_COUNT));
        let kill_icons = r
            .kills
            .iter()
            .enumerate()
            .map(|(i, k)| {
                (
                    matches!(k, KilledKind::GroundVehicle),
                    r.kill_units.get(i).copied().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        let kill_images = vec![None; kill_icons.len()];
        let composed = kill_icons.is_empty();
        ActiveCard {
            obj,
            state: ItemState::Enabled,
            t: Rc::clone(t),
            pilot: r.pilot,
            photo: pilot.map_or(0, |p| p.photo_index.max(0) as u32),
            name,
            missions,
            rank: rank_text_obj,
            gunnery,
            piloting,
            kills,
            promotion,
            medal_awarded,
            medals,
            meters,
            rank_icon: t
                .rank_icons
                .get(rank.max(0) as usize)
                .cloned()
                .unwrap_or_default(),
            kill_icons,
            kill_images,
            composed,
            promoted: r.promoted,
            reveal: Reveal::new(r),
            promotion_shown: !r.promoted,
        }
    }

    /// `MechIcon::init` / `VehicleIcon::init` and `update` for each kill: the unit's icon cell,
    /// recoloured by the state of each location of the killed unit.
    pub fn compose_kill_icons(&mut self, textures: &mut crate::textures::TextureStore) {
        let t = Rc::clone(&self.t);
        self.composed = true;
        for (n, (vehicle, unit)) in self.kill_icons.iter().enumerate() {
            let Some(atlas) = t.art.atlas(*vehicle) else {
                continue;
            };
            let mut pixels = damage::copy_cell(textures, atlas, unit.picture.max(0) as u32);
            let loc = |i: usize| {
                let (cur, max, destroyed) = unit.armor[i];
                Loc {
                    cur,
                    max,
                    destroyed,
                }
            };
            let (states, count) = if *vehicle {
                // `GROUNDVEHICLE_LOCATION_*`: front, left, right, rear, turret
                let a = VehicleArmor {
                    front: loc(0),
                    left: loc(1),
                    right: loc(2),
                    rear: loc(3),
                    turret: loc(4),
                };
                (damage::vehicle_states(&a), 5)
            } else {
                // `MECH_ARMOR_LOCATION_*`
                let a = MechArmor {
                    head: loc(0),
                    ctorso: loc(1),
                    ltorso: loc(2),
                    rtorso: loc(3),
                    larm: loc(4),
                    rarm: loc(5),
                    lleg: loc(6),
                    rleg: loc(7),
                    rctorso: loc(8),
                    rltorso: loc(9),
                    rrtorso: loc(10),
                };
                (damage::mech_states(&a), 8)
            };
            damage::apply(&mut pixels, &mut [0; 8], &states, count);
            let name = format!("results/kill{}_{n}", self.pilot);
            self.kill_images[n] = Some(textures.insert(&name, ICON_W, ICON_H, pixels));
        }
    }

    /// `PilotListItem::begin`.
    pub fn begin(&mut self) {
        // the clock restarts; the events already revealed stay so
        self.reveal.restart();
    }

    pub fn promoted(&self) -> bool {
        self.promoted
    }

    pub fn promotion_shown(&self) -> bool {
        self.promotion_shown
    }

    /// The promotion pop-up has closed (`promotionShown = true`).
    pub fn set_promotion_shown(&mut self) {
        self.promotion_shown = true;
    }

    /// `currentTime > flashTime() + .5`: the promotion pop-up is due.
    pub fn popup_due(&self) -> bool {
        self.promoted
            && !self.promotion_shown
            && self.reveal.time() > self.reveal.flash_time() + 0.5
    }

    /// `ActivePilotListItem::update` for the reveal's clock; the sounds it asks for go to `sounds`.
    pub fn step(&mut self, dt: f32, sounds: &mut Vec<i32>) {
        if !self.obj.show {
            return;
        }
        self.reveal.advance(dt, sounds);
    }

    /// `bDone`: the markers are out (`numPossible > 0`), the promotion is shown and `flashTime` has
    /// passed.
    pub fn is_done(&self) -> bool {
        self.reveal.time() > 2.0
            && self.reveal.time() > self.reveal.flash_time()
            && self.promotion_shown
    }

    pub fn kills_shown(&self) -> usize {
        self.reveal.kills_shown()
    }

    pub fn medals_shown(&self) -> usize {
        self.reveal.medals_shown()
    }

    /// The rectangle of kill marker `n`, rows of eight (`ActivePilotListItem::render`).
    pub fn kill_icon_rect(&self, n: usize) -> (f32, f32, f32, f32) {
        let k = &self.t.kill_icon.obj;
        let (w, h) = (k.width(), k.height());
        let x = self.obj.global_x() as f32 + k.left() + (n % 8) as f32 * (w + 1.0);
        let y = self.obj.global_y() as f32 + k.top() + (n / 8) as f32 * (h + 1.0);
        (x, y, x + w, y + h)
    }

    pub fn medal_rect(&self, n: usize) -> Option<GuiRect> {
        self.medals.get(n).map(|m| {
            let (gx, gy) = (self.obj.global_x(), self.obj.global_y());
            let o = &m.icon;
            GuiRect::new(
                gx + o.left() as i32,
                gy + o.top() as i32,
                gx + o.right() as i32,
                gy + o.bottom() as i32,
            )
        })
    }

    pub fn medal_count(&self) -> usize {
        self.medals.len()
    }

    pub fn promotion_header(&self) -> (&str, u32) {
        (&self.promotion.text, self.promotion.obj.color())
    }

    pub fn pilot_photo(&self) -> u32 {
        self.photo
    }

    /// The card's texts: name, rank, missions, gunnery number, piloting number.
    pub fn texts(&self) -> [&str; 5] {
        [
            &self.name.text,
            &self.rank.text,
            &self.missions.text,
            &self.gunnery.text,
            &self.piloting.text,
        ]
    }

    /// Where the pilot's photo is drawn.
    pub fn photo_rect(&self) -> (f32, f32, f32, f32) {
        let (gx, gy) = (self.obj.global_x() as f32, self.obj.global_y() as f32);
        let i = &self.t.icon.obj;
        (gx + i.left(), gy + i.top(), gx + i.right(), gy + i.bottom())
    }

    pub fn rank_icon(&self) -> &Object {
        &self.rank_icon
    }

    pub fn meters(&self) -> &[AttributeMeter; 2] {
        &self.meters
    }

    pub fn meter_added_colour(&self) -> u32 {
        self.meters[0].added_color_max
    }

    /// The colours the reveal gives the headers this frame.
    fn colour_headers(&mut self) {
        let t = self.reveal.time();
        let kills_offset = self.reveal.medal_start();
        let medals_t = t - kills_offset;
        if medals_t > 0.0 {
            self.medal_awarded
                .set_color(self.t.medal_anim.color_at(medals_t));
            if medals_t > 1.5 {
                let mut tmp = medals_t - 1.5;
                for m in &mut self.medals {
                    let show = tmp > 0.0;
                    m.icon.show = show;
                    m.text.obj.show = show;
                    tmp -= 0.5;
                }
            }
        } else {
            self.medal_awarded.set_color(0);
            for m in &mut self.medals {
                m.icon.show = false;
                m.text.obj.show = false;
            }
        }
        let promo_t = t - self.reveal.promotion_start();
        if promo_t > 0.0 {
            self.promotion
                .set_color(self.t.promoted_anim.color_at(promo_t));
        } else {
            self.promotion.set_color(0);
        }
    }
}

/// `aFont::height()` as the card uses it.
fn font_line(assets: &UiAssets, font: FontRef) -> f32 {
    crate::widgets::basic::font_height(assets, font) as f32
}

impl ListItem for ActiveCard {
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
    fn update(&mut self, _ctx: &mut UpdateCtx<'_>) {}
    fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        if !self.obj.show {
            return;
        }
        self.colour_headers();
        let (gx, gy) = (self.obj.global_x(), self.obj.global_y());
        let (fx, fy) = (gx as f32, gy as f32);
        // `aObject::render`: the box's black quad, then its children
        self.obj.draw_quad(ctx);
        let t = Rc::clone(&self.t);
        for icon in [&t.skill_icons[0], &t.skill_icons[1], &self.rank_icon] {
            let mut icon = icon.clone();
            icon.move_by(fx, fy);
            icon.render(ctx);
        }
        for text in [
            &mut self.kills,
            &mut self.name,
            &mut self.rank,
            &mut self.missions,
            &mut self.gunnery,
            &mut self.piloting,
        ] {
            text.render_at(ctx, fx, fy);
        }
        if !self.medals.is_empty() {
            self.medal_awarded.render_at(ctx, fx, fy);
            for m in &mut self.medals {
                if m.icon.show {
                    m.icon.render_at(ctx, fx, fy);
                    m.text.render_at(ctx, fx, fy);
                }
            }
        }
        if self.promoted {
            self.promotion.render_at(ctx, fx, fy);
        }
        // the box itself: `tmp.left = s_area->left()` is the layout's absolute x
        let a = &t.area.obj;
        let box_rect = GuiRect::new(
            a.left() as i32,
            gy + a.top() as i32,
            a.left() as i32 + a.width() as i32,
            gy + a.top() as i32 + a.height() as i32,
        );
        let c = t.area.color();
        ctx.dl.outline_rect(box_rect, c, c);
        // the line over the box
        let line_top = box_rect.top - 12;
        ctx.dl.fill_rect(
            GuiRect::new(
                box_rect.left,
                line_top,
                box_rect.left + t.total_width as i32,
                line_top + 2,
            ),
            0xff00_2f55,
        );
        let icon = &t.icon.obj;
        t.art.pilot(
            ctx,
            self.photo,
            (
                fx + icon.left(),
                fy + icon.top(),
                fx + icon.right(),
                fy + icon.bottom(),
            ),
        );
        // the gained points flash
        let flash = t.skill_anim.color_at(self.reveal.time());
        for m in &mut self.meters {
            m.added_color_max = flash;
            m.added_color_min = flash;
            m.render(ctx, gx, gy);
        }
        for o in &t.outlines {
            let r = GuiRect::new(
                gx + o.obj.left() as i32,
                gy + o.obj.top() as i32,
                gx + o.obj.right() as i32,
                gy + o.obj.bottom() as i32,
            );
            let c = o.color();
            ctx.dl.outline_rect(r, c, c);
        }
        for n in 0..self.reveal.kills_shown().min(self.kill_icons.len()) {
            let (vehicle, unit) = self.kill_icons[n];
            match self.kill_images[n] {
                Some(image) => t.art.image(ctx, image, self.kill_icon_rect(n)),
                None => t
                    .art
                    .unit(ctx, vehicle, unit.picture, self.kill_icon_rect(n)),
            }
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// What kind of row an item of the list is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    /// A heading or the blank line: not a `PilotListItem`.
    Text,
    Dead,
    Active,
}

/// `PilotListBox`: the review's list and the clock that reveals its items.
pub struct PilotList {
    pub list: ListBox,
    rows: Vec<Row>,
    /// `timeSinceStart`.
    since_start: f32,
    /// `curItem`.
    cur: usize,
    old_scroll: i32,
    new_scroll: i32,
    scroll_time: f32,
    /// `bDone`.
    done: bool,
    font: FontRef,
    colour: u32,
    dead: Rc<DeadTemplate>,
    active: Rc<ActiveTemplate>,
    skip: i32,
}

impl PilotList {
    pub fn load(ld: &mut Loader<'_>, file: &mut FitIni, rect: (f32, f32, f32, f32)) -> PilotList {
        let art = IconArt::load(ld);
        let _ = file.seek_block("PilotReviewMisc");
        let skip = file.long_or("PilotReviewSmallSkip", 5) as i32;
        let colour = file.color("color").unwrap_or(0xffff_8a00);
        let font = ld.font(file.long_or("Font", 0) as u32);
        let (x, y, w, h) = rect;
        let mut list = ListBox::new(ld, x, y, w, h);
        list.skip_amount = skip;
        list.top_skip = 2;
        let dead = Rc::new(DeadTemplate::load(ld, file, art));
        let active = Rc::new(ActiveTemplate::load(ld, file, art, w));
        PilotList {
            list,
            rows: Vec::new(),
            since_start: 0.0,
            cur: 0,
            old_scroll: 0,
            new_scroll: 0,
            scroll_time: 0.0,
            done: false,
            font,
            colour,
            dead,
            active,
            skip,
        }
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn rows(&self) -> usize {
        self.rows.len()
    }

    /// The `n`th active card, in list order.
    pub fn active_cards(&self) -> Vec<&ActiveCard> {
        (0..self.list.item_count())
            .filter_map(|i| self.list.item(i)?.as_any().downcast_ref::<ActiveCard>())
            .collect()
    }

    /// Whether a card has kill markers whose damage colours are not composed yet.
    pub fn kill_icons_pending(&self) -> bool {
        self.active_cards().iter().any(|c| !c.composed)
    }

    /// Composes the damage-coloured icon of every kill marker not composed yet (the textures need
    /// the store, which the screens only read while they run).
    pub fn compose_kill_icons(&mut self, textures: &mut crate::textures::TextureStore) {
        for i in 0..self.list.item_count() {
            if let Some(card) = self
                .list
                .item_mut(i)
                .and_then(|it| it.as_any_mut().downcast_mut::<ActiveCard>())
            {
                if !card.composed {
                    card.compose_kill_icons(textures);
                }
            }
        }
    }

    pub fn dead_cards(&self) -> Vec<&DeadCard> {
        (0..self.list.item_count())
            .filter_map(|i| self.list.item(i)?.as_any().downcast_ref::<DeadCard>())
            .collect()
    }

    fn heading(&mut self, assets: &UiAssets, text: &str, extra: f32, hidden: bool) {
        let mut item = TextListItem::new(assets, self.font);
        item.set_text(text);
        item.set_color(self.colour);
        if extra != 0.0 {
            let (w, h) = (item.obj.width(), item.obj.height());
            item.obj.resize(w, h + extra);
        }
        item.obj.show = !hidden;
        self.list.add_item(Box::new(item));
        self.rows.push(Row::Text);
    }

    /// `PilotReviewScreen::init`'s roster pass.
    pub fn fill(&mut self, assets: &UiAssets, logistics: &Logistics, review: &PilotReview) {
        self.list.remove_all_items();
        self.rows.clear();
        self.since_start = 0.0;
        self.cur = 0;
        self.old_scroll = 0;
        self.new_scroll = 0;
        self.scroll_time = 0.0;
        self.done = false;
        let has_dead = !review.killed.is_empty();
        if has_dead {
            self.heading(
                assets,
                assets.string(ids::KILLED_IN_ACTION as u32),
                0.0,
                false,
            );
            let mut placed = Vec::new();
            for (n, &p) in review.killed.iter().enumerate() {
                let mut card = DeadCard::new(&self.dead, assets, logistics, p);
                if n == 0 {
                    card.begin();
                }
                let at = self.list.add_item(Box::new(card));
                placed.push(at);
                // the even ones go to the right of the one before (`PilotListBox::AddItem`)
                if n % 2 == 1 {
                    let left = placed[n - 1].and_then(|i| {
                        let o = self.list.item(i)?.obj();
                        Some((o.global_x() as f32, o.global_y() as f32, o.width()))
                    });
                    if let (Some((lx, ly, lw)), Some(i)) = (left, at) {
                        let right = self.list.obj.global_right() as f32;
                        if let Some(item) = self.list.item_mut(i) {
                            item.move_to(lx + lw + self.skip as f32, ly);
                            let over = item.obj().global_right() as f32 - right;
                            if over > 0.0 {
                                let (w, h) = (item.obj().width(), item.obj().height());
                                item.resize(w - over, h);
                            }
                        }
                    }
                }
                self.rows.push(Row::Dead);
            }
            self.heading(assets, "", 0.0, false);
        }
        if !review.active.is_empty() {
            self.heading(
                assets,
                assets.string(ids::ACTIVE_DUTY as u32),
                20.0,
                has_dead,
            );
            for r in &review.active {
                let card = ActiveCard::new(&self.active, assets, logistics, r);
                self.list.add_item(Box::new(card));
                self.rows.push(Row::Active);
            }
        }
    }

    /// `PilotListBox::update`. Sounds go to `sounds`; `popup` is set to the index of the active
    /// pilot whose promotion pop-up is due.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, sounds: &mut Vec<i32>) -> Option<usize> {
        // the scroll bar is a child of the list: it still answers the pointer
        if let Some(sb) = &mut self.list.scroll_bar {
            for m in sb.update(ctx) {
                self.list.handle_message(m);
            }
        }
        self.since_start += ctx.dt;
        if self.since_start < REVEAL_START_DELAY {
            return None;
        }
        let n = self.list.item_count();
        if self.cur >= n {
            for i in 0..n {
                if let Some(it) = self.list.item_mut(i) {
                    it.obj_mut().show = true;
                }
            }
            self.done = true;
            return None;
        }
        self.done = false;
        let mut popup = None;
        for i in 0..n {
            if i < self.cur {
                self.show(i, true);
            } else if i == self.cur {
                match self.rows[i] {
                    Row::Text => self.cur += 1,
                    Row::Dead | Row::Active => {
                        if self.item_done(i) {
                            // on to the next pilot item, past the headings
                            let mut next = self.cur + 1;
                            while next < n && self.rows[next] == Row::Text {
                                next += 1;
                            }
                            self.cur = next;
                            self.scroll_time = 0.0;
                            self.old_scroll = self.list.scroll_pos() as i32;
                            if next < n {
                                self.begin_item(next);
                            }
                        } else {
                            popup = self.step_item(i, ctx.dt, sounds);
                        }
                    }
                }
            } else {
                self.show(i, false);
            }
        }
        self.scroll(ctx.dt);
        popup
    }

    fn show(&mut self, i: usize, show: bool) {
        if let Some(it) = self.list.item_mut(i) {
            it.obj_mut().show = show;
        }
    }

    fn item_done(&self, i: usize) -> bool {
        let Some(item) = self.list.item(i) else {
            return false;
        };
        match self.rows[i] {
            Row::Dead => item
                .as_any()
                .downcast_ref::<DeadCard>()
                .is_some_and(DeadCard::is_done),
            Row::Active => item
                .as_any()
                .downcast_ref::<ActiveCard>()
                .is_some_and(ActiveCard::is_done),
            Row::Text => true,
        }
    }

    fn begin_item(&mut self, i: usize) {
        let Some(item) = self.list.item_mut(i) else {
            return;
        };
        if let Some(c) = item.as_any_mut().downcast_mut::<DeadCard>() {
            c.begin();
        } else if let Some(c) = item.as_any_mut().downcast_mut::<ActiveCard>() {
            c.begin();
        }
    }

    /// One frame of item `i`'s clock; the active pilot index when its promotion pop-up is due.
    fn step_item(&mut self, i: usize, dt: f32, sounds: &mut Vec<i32>) -> Option<usize> {
        let item = self.list.item_mut(i)?;
        if let Some(c) = item.as_any_mut().downcast_mut::<DeadCard>() {
            c.obj_mut().show = true;
            c.step(dt);
        } else if let Some(c) = item.as_any_mut().downcast_mut::<ActiveCard>() {
            c.obj_mut().show = true;
            c.step(dt, sounds);
            if c.popup_due() {
                return Some(c.pilot);
            }
        }
        None
    }

    /// The promotion pop-up of active pilot `pilot` has closed.
    pub fn promotion_closed(&mut self, pilot: usize) {
        for i in 0..self.list.item_count() {
            if let Some(c) = self
                .list
                .item_mut(i)
                .and_then(|it| it.as_any_mut().downcast_mut::<ActiveCard>())
            {
                if c.pilot == pilot {
                    c.set_promotion_shown();
                }
            }
        }
    }

    /// The scrolling that brings the pilot being revealed into view (`PilotListBox::update`'s last
    /// part): a half-second glide of 140 units a second.
    fn scroll(&mut self, dt: f32) {
        let n = self.list.item_count();
        let Some(top) = (self.cur < n)
            .then(|| self.list.item(self.cur))
            .flatten()
            .map(|it| it.obj().global_y())
        else {
            return;
        };
        let bottom = self.list.obj.global_bottom();
        let h = self.list.obj.height() as i32;
        if top > bottom - 3 * h / 4 && self.new_scroll == 0 {
            self.old_scroll = self.list.scroll_pos() as i32;
            self.new_scroll = self.old_scroll + top - bottom + 3 * h / 4;
            if self.new_scroll < self.old_scroll {
                self.new_scroll = 0;
            }
        } else if self.new_scroll == 0 {
            self.show(self.cur, true);
        }
        if self.new_scroll != 0 {
            self.scroll_time += dt;
            let delta = (140.0 * self.scroll_time) as i32;
            let max = self
                .list
                .scroll_bar
                .as_ref()
                .map_or(0, |s| s.scroll_max as i32);
            if delta + self.old_scroll < self.new_scroll && delta + self.old_scroll < max {
                self.list.set_scroll_pos(self.old_scroll + delta);
            } else {
                self.list.set_scroll_pos(self.new_scroll);
                self.show(self.cur, true);
                self.new_scroll = 0;
                self.scroll_time = 0.0;
            }
        }
    }

    pub fn move_by(&mut self, dx: f32, dy: f32) {
        self.list.move_by(dx, dy);
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        self.list.render(ctx);
    }
}
