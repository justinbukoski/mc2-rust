// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mission results flow (`code/missionresults.cpp`) and its two screens over
//! `mcui_mr_layout.fit`: the salvage screen (`code/salvagemecharea.cpp`) and the pilot review
//! (`code/pilotreviewarea.cpp`).
//!
//! The stages and every change to the campaign are the campaign's [`ResultsFlow`]: the salvage
//! list and its prices, the ticked mechs that join the bay, the pilot review, the specialty each
//! promoted pilot picks, and the pilots' promotions. This module shows the flow and passes the
//! player's clicks on. The salvage screen plays the C-Bills sounds and counts the C-Bills up or
//! down (`SalvageMechScreen::update`) and refuses an unaffordable row with `LOG_WRONGBUTTON`.
//! The pilot review slides in (`PilotAreaBeginAnimation`) and plays the original's timed reveals
//! (`pilotreviewarea.cpp:560-815`): after a one second delay each active pilot is revealed in turn
//! with its kill markers as unit icons (half a second apart, `LOG_KILLMARKER`), then its medals
//! (`LOG_PILOTMEDAL`) and its promotion (`LOG_PROMOTED`), see [`Reveal`] and
//! [`crate::results_pilots`]; a promoted pilot's pop-up ([`crate::results_promotion`]) follows.
//! The salvage rows and the selected-mech panel are [`crate::results_salvage`]. Done starts the exit
//! (`SalvageAreaDoneAnimation`, `PilotReviewFadeOutAnimation`, or the 0.5 s fade when the review is
//! skipped) and the flow moves on once it has played.

use crate::anim::Animation;
use crate::ctx::{sfx, Loader, RenderCtx, UiAssets, UpdateCtx};
use crate::results_icons::IconArt;
use crate::results_pilots::PilotList;
use crate::results_promotion::{Pick, PromotionArea};
use crate::results_salvage::{RowTemplate, SalvagePanel, SalvageRow, ROW_SKIP, ROW_TOP_SKIP};
use crate::screen::{LogisticsScreen, Prefixes};
use crate::widgets::ListBox;
use mc2_campaign::interface::Outcome;
use mc2_campaign::logistics::SalvageOffer;
use mc2_campaign::pilot::KilledKind;
use mc2_campaign::results::{
    AutosaveRequest, PilotReview, ResultsFlow, ResultsStage, ReviewedPilot,
};
use mc2_campaign::Logistics;

/// The exit of a results screen after its Done (`bDone`, `exitAnim`, `beginFadeOut`): the screen
/// is done only once the layout's exit animation has played (`isDone`), or, with no animation
/// (`fade_only`), once the fade-out time has passed. `salvagemecharea.cpp:222-249`,
/// `pilotreviewarea.cpp:264-281`.
#[derive(Default)]
struct Exit {
    anim: Animation,
    /// `bDone`.
    pressed: bool,
    /// Seconds since Done when the exit is the fade-out alone.
    fade: f32,
    fade_out: f32,
    /// Keep the final animation pose visible for one frame before advancing the flow.
    finished_frame: bool,
}

impl Exit {
    fn load(ld: &mut Loader<'_>, block: &str) -> Exit {
        let anim = ld
            .art_fit(LAYOUT)
            .map(|mut f| Animation::from_block(&mut f, block))
            .unwrap_or_default();
        Exit {
            anim,
            ..Exit::default()
        }
    }

    fn reset(&mut self) {
        self.pressed = false;
        self.fade = 0.0;
        self.fade_out = 0.0;
        self.finished_frame = false;
        self.anim.end();
    }

    /// Done pressed: `exitAnim.begin()`, or `beginFadeOut(fade_only)` when given.
    fn press(&mut self, fade_only: Option<f32>) {
        self.pressed = true;
        self.fade = 0.0;
        self.finished_frame = false;
        match fade_only {
            Some(t) => self.fade_out = t,
            None => {
                self.anim.begin();
                self.anim.update(0.0);
            }
        }
    }

    /// `exitAnim.getXDelta()/getYDelta()` as `render` applies them (`long`), while the exit
    /// animation plays; nothing otherwise.
    fn offset(&self) -> (i32, i32) {
        if self.pressed && self.anim.is_animating() {
            (self.anim.x_delta() as i32, self.anim.y_delta() as i32)
        } else {
            (0, 0)
        }
    }

    /// Advances the exit; `true` once it has finished.
    fn update(&mut self, dt: f32) -> bool {
        if !self.pressed {
            return false;
        }
        self.fade += dt;
        self.anim.update(dt);
        if self.fade_out > 0.0 {
            self.fade > self.fade_out
        } else {
            // a layout without the block has no animation to wait for
            if self.anim.infos.is_empty() {
                true
            } else if self.anim.is_done() {
                std::mem::replace(&mut self.finished_frame, true)
            } else {
                false
            }
        }
    }
}

/// The layout the results flow shares.
const LAYOUT: &str = "mcui_mr_layout";

/// `SalvageMechScreen`'s `SalvageAreaButton0` (id 101): Done.
pub const SALVAGE_DONE: i32 = 101;
/// `PilotReviewButton0` (id 0): Done.
pub const PILOT_DONE: i32 = 0;
/// `PromoteGadgetRightButton0` (id 101): the promotion area's Done, which confirms the pick.
pub const PILOT_CHOOSE: i32 = 101;

/// The timeline of one active pilot's reveal (`ActivePilotListItem::render`, `flashTime`): the kill
/// markers one every half second from 1.5 s, then the medals, then the promotion. `advance` hands
/// back the sounds the C++ plays as each event appears.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    kills: Vec<KilledKind>,
    medals: Vec<usize>,
    promoted: bool,
    /// `currentTime`.
    t: f32,
    kills_shown: usize,
    medals_shown: usize,
    promotion_started: bool,
}

impl Reveal {
    pub fn new(r: &ReviewedPilot) -> Reveal {
        Reveal {
            kills: r.kills.clone(),
            medals: r.new_medals.clone(),
            promoted: r.promoted,
            t: 0.0,
            kills_shown: 0,
            medals_shown: 0,
            promotion_started: false,
        }
    }

    /// `ActivePilotListItem::flashTime`.
    pub fn flash_time(&self) -> f32 {
        let mut t = if self.promoted { 1.5 } else { 0.0 };
        if !self.medals.is_empty() {
            t += 2.0 + 0.5 * self.medals.len() as f32;
        }
        t + 1.5 + 0.5 * self.kills.len() as f32
    }

    /// `currentTime`.
    pub fn time(&self) -> f32 {
        self.t
    }

    /// `PilotListItem::begin`: the clock starts again from 0.
    pub fn restart(&mut self) {
        self.t = 0.0;
    }

    /// Seconds before the medals start (the kill markers' time).
    pub fn medal_start(&self) -> f32 {
        self.medal_offset()
    }

    /// Seconds before the promotion header shows.
    pub fn promotion_start(&self) -> f32 {
        self.promotion_offset()
    }

    /// Seconds before the medals start (the kill markers' time).
    fn medal_offset(&self) -> f32 {
        if self.kills.is_empty() {
            0.0
        } else {
            1.5 + 0.5 * self.kills.len() as f32
        }
    }

    /// Seconds before the promotion shows.
    fn promotion_offset(&self) -> f32 {
        self.medal_offset()
            + if self.medals.is_empty() {
                0.0
            } else {
                1.5 + 0.5 * self.medals.len() as f32
            }
    }

    /// One frame: moves the clock on and pushes the sounds of the events that just appeared.
    pub fn advance(&mut self, dt: f32, sounds: &mut Vec<i32>) {
        let old = self.t;
        self.t += dt;
        // kill markers: `numPossible = 2 * (t - 1.5)`, a marker for every counter <= numPossible
        let possible = |t: f32| {
            if t - 1.5 < 0.0 {
                -1
            } else {
                (2.0 * (t - 1.5)) as i32
            }
        };
        let (old_possible, now_possible) = (possible(old), possible(self.t));
        if old_possible != now_possible && (now_possible as i64) < self.kills.len() as i64 {
            sounds.push(sfx::LOG_KILLMARKER);
        }
        self.kills_shown = ((now_possible + 1).max(0) as usize).min(self.kills.len());
        // medals: one every half second from 1.5 s after the kill markers, a sound each
        let medal_t = self.t - self.medal_offset() - 1.5;
        if self.t - self.medal_offset() > 0.0 && medal_t > 0.0 {
            let shown = (((medal_t / 0.5) as usize) + 1).min(self.medals.len());
            for _ in self.medals_shown..shown {
                sounds.push(sfx::LOG_PILOTMEDAL);
            }
            self.medals_shown = self.medals_shown.max(shown);
        }
        if self.promoted && !self.promotion_started && self.t - self.promotion_offset() > 0.0 {
            self.promotion_started = true;
            sounds.push(sfx::LOG_PROMOTED);
        }
    }

    pub fn kills_shown(&self) -> usize {
        self.kills_shown
    }

    pub fn medals_shown(&self) -> usize {
        self.medals_shown
    }

    /// The promotion line has appeared.
    pub fn promotion_shown(&self) -> bool {
        self.promotion_started
    }

    /// `bDone`: the markers are out (`numPossible > 0`, so after 2 s), the promotion is shown and
    /// `flashTime` has passed.
    pub fn is_done(&self) -> bool {
        self.t > 2.0 && self.t > self.flash_time() && (!self.promoted || self.promotion_started)
    }
}

/// `RP_TEXTID`: the C-Bills text of the salvage screen.
const SALVAGE_CBILLS_TEXT: usize = 3;
/// `helpTextArrayID` of the salvage screen.
const SALVAGE_HELP_TEXT: i32 = 2;
/// `SalvageMechScreen`'s count-up/down time (`countDownTime`, `salvagemecharea.cpp:66`).
const COUNT_DOWN_TIME: f32 = 0.5;
/// `WINDOW_OPEN` / `WINDOW_CLOSE`: the C-Bills went up / down.
const WINDOW_OPEN: i32 = 64;
const WINDOW_CLOSE: i32 = 65;
/// The C-Bills text colours: at rest, counting up, counting down.
const CBILLS_REST: u32 = 0xff00_5392;
const CBILLS_UP: u32 = 0xffc8_e100;
const CBILLS_DOWN: u32 = 0xffa2_1600;

/// The salvage screen (`SalvageMechScreen`): the candidates with a check box each.
pub struct SalvageScreen {
    pub screen: LogisticsScreen,
    /// `salvageListBox`: one row per offer, in the flow's order.
    pub list: ListBox,
    /// `selMechArea`: the picked mech's texts, meters, loadout and 3D view.
    pub panel: SalvagePanel,
    row: RowTemplate,
    /// The offer the panel shows.
    shown: Option<usize>,
    /// The list selection the panel last followed.
    synced_selection: Option<usize>,
    /// `oldCBillsAmount`.
    old_c_bills: i64,
    /// `previousAmount`: the last change (old minus new) the text is still counting off.
    previous_amount: i64,
    /// `curCount`: seconds of that count, 0 when none runs.
    count: f32,
    exit: Exit,
}

impl SalvageScreen {
    /// `SalvageMechScreen::init`.
    pub fn load(ld: &mut Loader<'_>) -> Result<SalvageScreen, String> {
        let prefixes = Prefixes {
            statics: Some("SalvageAreaStatic"),
            texts: Some("SalvageAreaText"),
            rects: Some("SalvageAreaRect"),
            buttons: Some("SalvageAreaButton"),
            edits: None,
            anim_objects: None,
        };
        let mut screen = LogisticsScreen::load(ld, LAYOUT, prefixes)?;
        screen.help_text_array_id = SALVAGE_HELP_TEXT;
        for b in &mut screen.buttons {
            b.set_message_on_release();
        }
        // the list is `SalvageAreaRect3`
        let r = screen
            .rect(3)
            .map(|r| (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height()));
        let (x, y, w, h) = r.unwrap_or_default();
        let mut fit = ld.art_fit(LAYOUT)?;
        let row = RowTemplate::load(ld, &mut fit);
        let panel = SalvagePanel::load(ld, &screen, &mut fit)?;
        let mut list = ListBox::new(ld, x, y, w, h);
        list.skip_amount = ROW_SKIP;
        list.top_skip = ROW_TOP_SKIP;
        Ok(SalvageScreen {
            list,
            panel,
            row,
            shown: None,
            synced_selection: None,
            screen,
            old_c_bills: 0,
            previous_amount: 0,
            count: 0.0,
            exit: Exit::load(ld, "SalvageAreaDoneAnimation"),
        })
    }

    /// `SalvageMechScreen::begin`: one row per offer.
    pub fn begin(&mut self, ctx: &UpdateCtx<'_>, flow: &ResultsFlow, logistics: &Logistics) {
        self.screen.begin();
        // `SalvageMechArea::init` / `begin`: `beginFadeIn(1.0)` (salvagemecharea.cpp:185)
        self.screen.begin_fade_in(1.0);
        self.exit.reset();
        self.old_c_bills = logistics.c_bills();
        self.previous_amount = 0;
        self.count = 0.0;
        self.shown = None;
        self.synced_selection = None;
        self.panel.set_mech(ctx.assets, logistics, None, [0; 3]);
        self.rebuild(ctx, flow, logistics);
        if !flow.offers().is_empty() {
            self.list.select_item(0);
            self.show_offer(ctx.assets, flow, logistics, 0);
            self.sync_enables(flow, logistics);
        }
    }

    /// Whether a row's damage-coloured icon is not composed yet.
    pub fn icons_pending(&self) -> bool {
        self.rows().any(|r| r.icon_pending())
    }

    /// Composes the damage colours of the salvage rows' icons (`MechIcon::update` of each wreck).
    pub fn compose_icons(&mut self, textures: &mut crate::textures::TextureStore) {
        for i in 0..self.list.item_count() {
            if let Some(row) = self
                .list
                .item_mut(i)
                .and_then(|item| item.as_any_mut().downcast_mut::<SalvageRow>())
            {
                if row.icon_pending() {
                    row.compose_icon(textures);
                }
            }
        }
    }

    fn rows(&self) -> impl Iterator<Item = &SalvageRow> {
        (0..self.list.item_count())
            .filter_map(|i| self.list.item(i)?.as_any().downcast_ref::<SalvageRow>())
    }

    /// `SalvageListItem::update`: every frame a row the player cannot pay for (and has not
    /// ticked) disables itself and its check box; a row that became affordable enables again.
    fn sync_enables(&mut self, flow: &ResultsFlow, logistics: &Logistics) {
        for i in 0..flow.offers().len() {
            let disabled = !flow.offer_enabled(logistics, i);
            if let Some(row) = self
                .list
                .item_mut(i)
                .and_then(|item| item.as_any_mut().downcast_mut::<SalvageRow>())
            {
                row.set_disabled(disabled);
            }
        }
    }

    /// `SalvageMechArea::instance->setMech(pVariant, ...)`: the panel shows offer `i`.
    fn show_offer(
        &mut self,
        assets: &UiAssets,
        flow: &ResultsFlow,
        logistics: &Logistics,
        i: usize,
    ) {
        let Some(o) = flow.offers().get(i) else {
            return;
        };
        self.shown = Some(i);
        self.panel
            .set_mech(assets, logistics, Some(o.variant), o.paint_scheme);
    }

    /// Builds one row per offer: the check box, the unit icon, the names, the tonnage and the
    /// price, the rows the player cannot pay for greyed out.
    fn rebuild(&mut self, ctx: &UpdateCtx<'_>, flow: &ResultsFlow, logistics: &Logistics) {
        self.list.remove_all_items();
        for (i, o) in flow.offers().iter().enumerate() {
            let mut row = SalvageRow::new(&self.row, ctx.assets, logistics, o);
            row.set_disabled(!flow.offer_enabled(logistics, i));
            row.set_checked(flow.ticked().contains(&i));
            self.list.add_item(Box::new(row));
        }
        self.show_c_bills(logistics);
    }

    /// After a tick: the rows' enables and checks change in place, so the list keeps its scroll
    /// and its selection (`SalvageListItem::update` changes its own row the same way).
    fn refresh_rows(&mut self, flow: &ResultsFlow, logistics: &Logistics) {
        for i in 0..flow.offers().len() {
            let disabled = !flow.offer_enabled(logistics, i);
            let ticked = flow.ticked().contains(&i);
            let Some(row) = self
                .list
                .item_mut(i)
                .and_then(|item| item.as_any_mut().downcast_mut::<SalvageRow>())
            else {
                continue;
            };
            row.set_disabled(disabled);
            row.set_checked(ticked);
        }
        self.show_c_bills(logistics);
    }

    fn show_c_bills(&mut self, logistics: &Logistics) {
        if let Some(t) = self.screen.texts.get_mut(SALVAGE_CBILLS_TEXT) {
            t.set_text(&format!("{}", logistics.c_bills()));
        }
    }

    /// The first half of `SalvageMechScreen::update`: a change of the C-Bills plays a sound and
    /// the text counts from the old amount to the new one, green going up, red going down.
    fn update_c_bills(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &Logistics) {
        let mut amount = logistics.c_bills();
        let mut colour = CBILLS_REST;
        if amount != self.old_c_bills {
            self.previous_amount = self.old_c_bills - amount;
            self.count = 0.00001;
            self.old_c_bills = amount;
            ctx.play(if self.previous_amount < 0 {
                WINDOW_OPEN
            } else {
                WINDOW_CLOSE
            });
        }
        if self.count != 0.0 && self.count + ctx.dt < COUNT_DOWN_TIME {
            self.count += ctx.dt;
            let remaining = self.previous_amount as f32
                - self.count / COUNT_DOWN_TIME * self.previous_amount as f32;
            // `amount += curAmount` on an int: the float sum is truncated
            amount = (amount as f32 + remaining) as i64;
            colour = if remaining > 0.0 {
                CBILLS_DOWN
            } else {
                CBILLS_UP
            };
        }
        if let Some(t) = self.screen.texts.get_mut(SALVAGE_CBILLS_TEXT) {
            t.set_text(&amount.to_string());
            t.set_color(colour);
        }
    }

    /// One frame: a click left of a row's check box ticks or unticks its offer; Done returns
    /// `true` (the flow then moves on).
    pub fn update(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        flow: &mut ResultsFlow,
        logistics: &mut Logistics,
    ) -> bool {
        self.update_c_bills(ctx, logistics);
        self.sync_enables(flow, logistics);
        let pressed = self
            .screen
            .update(ctx)
            .iter()
            .any(|m| m.who == SALVAGE_DONE);
        if pressed && !self.exit.pressed {
            // `SalvageMechScreen::handleMessage`: a skipped review fades out in half a second
            let skip = logistics.mission_info().skip_pilot_review();
            self.exit.press(skip.then_some(0.5));
            if skip {
                self.screen.begin_fade_out(0.5);
            }
        }
        let done = self.exit.update(ctx.dt);
        // `salvageListBox.update()`: the scroll bar, the row selection and which rows show; each
        // row updates its own check box
        self.list.update(ctx);
        self.panel.update(ctx);
        // a check box the player pressed ticks or unticks its offer
        for i in 0..self.list.item_count() {
            let Some(row) = self
                .list
                .item(i)
                .and_then(|item| item.as_any().downcast_ref::<SalvageRow>())
            else {
                continue;
            };
            if row.is_checked() != flow.ticked().contains(&i) {
                // a refused tick (not enough C-Bills) leaves the list as it was
                let _ = flow.toggle_salvage(logistics, i);
                self.refresh_rows(flow, logistics);
                break;
            }
        }
        if ctx.input.is_left_click() {
            let (x, y) = (ctx.input.mouse_x(), ctx.input.mouse_y());
            for i in 0..self.list.item_count() {
                let Some(item) = self.list.item(i) else {
                    continue;
                };
                let o = item.obj();
                let inside_row = o.show
                    && self.list.point_inside(x, y)
                    && x as f32 >= o.left()
                    && x as f32 <= o.right()
                    && y as f32 >= o.top()
                    && y as f32 <= o.bottom();
                if inside_row {
                    if !flow.offer_enabled(logistics, i) {
                        // `SalvageListItem::update`: a row that costs more than the C-Bills
                        // (and is not ticked) answers a click with the wrong-button sound
                        ctx.play(sfx::LOG_WRONGBUTTON);
                    }
                    // every click on a row shows its mech in the panel
                    self.show_offer(ctx.assets, flow, logistics, i);
                    break;
                }
            }
        }
        // a row selected by the list box (the first one, at the start) fills the panel
        // (a clicked row the list cannot select, a mech too dear, stays shown: `salvagemecharea.cpp:466-476`)
        let selected = usize::try_from(self.list.selected_item()).ok();
        if selected != self.synced_selection {
            self.synced_selection = selected;
            if let Some(sel) = selected {
                if self.shown != Some(sel) {
                    self.show_offer(ctx.assets, flow, logistics, sel);
                }
            }
        }
        done
    }

    /// `SalvageMechScreen::render`: once Done is pressed the list, the panel and the screen slide
    /// by the exit animation's offsets.
    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        let (x, y) = self.exit.offset();
        self.list.move_by(x as f32, y as f32);
        self.list.render(ctx);
        self.list.move_by(-x as f32, -y as f32);
        self.panel.render(ctx, x, y);
        self.screen.render_offset(ctx, x, y);
    }
}

/// The pilot review (`PilotReviewScreen`): the killed pilots, then the active ones with their
/// skill gains, kill markers, medals and promotions, revealed in turn. A promoted pilot's card is
/// followed by the promotion pop-up (`PilotPromotionArea`), where they pick a specialty skill; the
/// review's Done waits for the list to finish.
pub struct PilotScreen {
    pub screen: LogisticsScreen,
    /// `pilotListBox`.
    pub list: PilotList,
    /// `s_curPromotion`: the pop-up.
    pub promotion: PromotionArea,
    /// `entryAnim` (`PilotAreaBeginAnimation`): the review slides in from the right.
    entry: Animation,
    exit: Exit,
    /// A promotion is going to happen: Done stays disabled until the list is done.
    promoted_any: bool,
}

impl PilotScreen {
    /// `PilotReviewScreen::init`, with the promotion area from the same layout.
    pub fn load(ld: &mut Loader<'_>) -> Result<PilotScreen, String> {
        let prefixes = Prefixes {
            statics: Some("PilotReviewStatic"),
            texts: Some("PilotReviewText"),
            rects: Some("PilotReviewRect"),
            buttons: Some("PilotReviewButton"),
            edits: None,
            anim_objects: None,
        };
        let mut screen = LogisticsScreen::load(ld, LAYOUT, prefixes)?;
        for b in &mut screen.buttons {
            b.set_message_on_release();
        }
        let mut fit = ld.art_fit(LAYOUT)?;
        let rect = screen
            .rect(0)
            .map(|r| (r.obj.left(), r.obj.top(), r.obj.width(), r.obj.height()))
            .unwrap_or_default();
        let list = PilotList::load(ld, &mut fit, rect);
        let art = IconArt::load(ld);
        let promotion = PromotionArea::load(ld, &mut fit, art)?;
        let entry = Animation::from_block(&mut fit, "PilotAreaBeginAnimation");
        Ok(PilotScreen {
            list,
            promotion,
            entry,
            promoted_any: false,
            screen,
            exit: Exit::load(ld, "PilotReviewFadeOutAnimation"),
        })
    }

    /// `PilotReviewScreen::init`'s roster pass: the killed pilots under their heading, then the
    /// active ones under theirs, and the slide-in begins.
    pub fn begin(&mut self, ctx: &UpdateCtx<'_>, flow: &ResultsFlow, logistics: &Logistics) {
        self.screen.begin();
        self.exit.reset();
        self.promotion.close();
        match flow.pilot_review() {
            Some(review) => self.list.fill(ctx.assets, logistics, review),
            None => self
                .list
                .fill(ctx.assets, logistics, &PilotReview::default()),
        }
        self.promoted_any = flow
            .pilot_review()
            .is_some_and(|r| r.active.iter().any(|p| p.promoted));
        self.entry.begin();
        // `if ( bJustPromoted ) buttons[0].disable( true )`
        if let Some(b) = self.screen.button_mut(PILOT_DONE) {
            b.disable(self.promoted_any);
        }
    }

    /// The promoted pilot whose pop-up is open.
    pub fn picker_pilot(&self) -> Option<usize> {
        self.promotion.pilot()
    }

    /// The confirm button of the pop-up (`PromoteGadgetRightButton0`).
    pub fn choose_button(&self) -> Option<&crate::widgets::AnimButton> {
        self.promotion.confirm()
    }

    /// How far the slide-in has the review from its place (`entryAnim.getXDelta()`), in pixels;
    /// `(0, 0)` once it has played.
    pub fn slide_offset(&self) -> (i32, i32) {
        if self.entry.is_done() {
            (0, 0)
        } else {
            (self.entry.x_delta() as i32, self.entry.y_delta() as i32)
        }
    }

    /// The slide-in has played.
    pub fn entered(&self) -> bool {
        self.entry.is_done()
    }

    /// One frame (`PilotReviewScreen::update`): the pop-up while one is open; otherwise the list,
    /// Done, and the slide animations. Done returns `true` once its exit has played.
    pub fn update(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        flow: &mut ResultsFlow,
        logistics: &mut Logistics,
    ) -> bool {
        if self.promotion.is_open() && !self.exit.pressed {
            if let Some(Pick { pilot, skill }) = self.promotion.update(ctx) {
                let _ = flow.choose_specialty(logistics, pilot, skill);
            }
            if self.promotion.take_confirmed() {
                self.screen.begin_fade_in(1.0);
            }
            if self.promotion.is_done() {
                if let Some(pilot) = self.promotion.pilot() {
                    self.list.promotion_closed(pilot);
                }
                self.promotion.close();
            }
            return false;
        }
        let entered = self.entry.is_done();
        if entered {
            let mut sounds = Vec::new();
            let due = self.list.update(ctx, &mut sounds);
            for id in sounds {
                ctx.play(id);
            }
            if let Some(pilot) = due {
                self.open_promotion(ctx, flow, logistics, pilot);
            }
        }
        let msgs = self.screen.update(ctx);
        let pressed = msgs.iter().any(|m| m.who == PILOT_DONE) && entered;
        if self.list.is_done() {
            if let Some(b) = self.screen.button_mut(PILOT_DONE) {
                b.disable(false);
            }
        }
        // `PilotReviewScreen::handleMessage`: Done starts the exit animation and a one second
        // fade; the flow moves on once the animation is done. A pending pick holds the flow.
        if pressed && !self.exit.pressed && flow.pending_specialty(logistics).is_none() {
            self.exit.press(None);
            self.screen.begin_fade_out(1.0);
        }
        self.entry.update(ctx.dt);
        self.exit.update(ctx.dt)
    }

    /// `ActivePilotListItem::update`: the pop-up for `pilot`, when the flow waits on a pick.
    fn open_promotion(
        &mut self,
        ctx: &mut UpdateCtx<'_>,
        flow: &ResultsFlow,
        logistics: &Logistics,
        pilot: usize,
    ) {
        if flow.pending_specialty(logistics) != Some(pilot) {
            // nothing left to pick: the promotion needs no pop-up
            self.list.promotion_closed(pilot);
            return;
        }
        let photo = self
            .list
            .active_cards()
            .iter()
            .find(|c| c.pilot == pilot)
            .map_or(0, |c| c.pilot_photo());
        self.promotion.set_pilot(ctx, logistics, pilot, photo);
        self.screen.begin_fade_out(1.0);
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        // the slide in, or the slide out once Done is pressed
        let (x, y) = if !self.entry.is_done() {
            (self.entry.x_delta() as i32, self.entry.y_delta() as i32)
        } else {
            self.exit.offset()
        };
        self.list.move_by(x as f32, y as f32);
        self.list.render(ctx);
        self.list.move_by(-x as f32, -y as f32);
        self.screen.render_offset(ctx, x, y);
        self.promotion.render(ctx);
    }
}

/// The results flow: the screens for the stages it has, and the flow itself.
pub struct MissionResults {
    pub salvage: SalvageScreen,
    pub pilots: PilotScreen,
    flow: Option<ResultsFlow>,
}

impl MissionResults {
    pub fn load(ld: &mut Loader<'_>) -> Result<MissionResults, String> {
        Ok(MissionResults {
            salvage: SalvageScreen::load(ld)?,
            pilots: PilotScreen::load(ld)?,
            flow: None,
        })
    }

    /// `MissionResults::begin` for a mission the player just won: the campaign builds the flow
    /// (`Logistics::begin_results`), and the screens show its salvage list and pilot review.
    pub fn begin(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics, outcome: &Outcome) {
        let flow = logistics.begin_results(outcome);
        self.salvage.begin(ctx, &flow, logistics);
        self.pilots.begin(ctx, &flow, logistics);
        self.flow = Some(flow);
    }

    /// Whether the pilot review has kill markers whose damage colours are not composed yet (the
    /// list is made when the review starts, after the salvage screen).
    pub fn kill_icons_pending(&self) -> bool {
        self.pilots.list.kill_icons_pending() || self.salvage.icons_pending()
    }

    /// Composes the damage colours of the pilot review's kill markers into their own textures
    /// (`MechIcon::update` on each killed unit); a marker not composed draws its icon undamaged.
    pub fn compose_kill_icons(&mut self, textures: &mut crate::textures::TextureStore) {
        self.pilots.list.compose_kill_icons(textures);
        self.salvage.compose_icons(textures);
    }

    /// The salvage list the flow offers.
    pub fn offers(&self) -> &[SalvageOffer] {
        self.flow.as_ref().map_or(&[], |f| f.offers())
    }

    /// The running flow, if any.
    pub fn flow(&self) -> Option<&ResultsFlow> {
        self.flow.as_ref()
    }

    /// What the pilot review shows, once the flow has one.
    pub fn pilot_review(&self) -> Option<&PilotReview> {
        self.flow.as_ref().and_then(|f| f.pilot_review())
    }

    /// The stage the flow is in, if one is running.
    pub fn stage(&self) -> Option<ResultsStage> {
        self.flow.as_ref().map(|f| f.stage())
    }

    /// `MissionResults::isDone`: `bDone` starts false (`missionresults.cpp:26`), so a flow that
    /// has not begun is not done.
    pub fn is_done(&self) -> bool {
        self.flow.as_ref().is_some_and(|f| f.is_done())
    }

    /// One frame: the stage's screen, and the flow moves on when its Done is pressed.
    pub fn update(&mut self, ctx: &mut UpdateCtx<'_>, logistics: &mut Logistics) {
        let Some(flow) = self.flow.as_mut() else {
            return;
        };
        match flow.stage() {
            ResultsStage::Salvage => {
                if self.salvage.update(ctx, flow, logistics) {
                    flow.finish_salvage(logistics);
                    self.pilots.begin(ctx, flow, logistics);
                }
            }
            ResultsStage::PilotReview => {
                if self.pilots.update(ctx, flow, logistics) {
                    flow.finish_pilot_review(logistics);
                }
            }
            ResultsStage::Done => {}
        }
    }

    /// `MissionResults::end`'s autosave request, once the flow is done.
    pub fn autosave(&self, logistics: &Logistics) -> Option<AutosaveRequest> {
        self.flow.as_ref().and_then(|f| f.end(logistics))
    }

    pub fn render(&mut self, ctx: &mut RenderCtx<'_>) {
        match self.stage() {
            Some(ResultsStage::Salvage) => self.salvage.render(ctx),
            Some(ResultsStage::PilotReview) => self.pilots.render(ctx),
            _ => {}
        }
    }
}
