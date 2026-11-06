// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The results flow driven through its screens, from a synthetic `mc2-campaign` outcome: one
//! wrecked mech is salvaged and one pilot is promoted. The campaign state is checked after the
//! screens are done. Skips cleanly without `MC2_DATA`.

use std::sync::Arc;

use mc2_campaign::interface::{MechSummary, MissionResult, MoverReport, Outcome, SalvageCandidate};
use mc2_campaign::pilot::{KilledKind, PilotReport, RANK_REGULAR};
use mc2_campaign::results::ResultsStage;
use mc2_campaign::{GameData, Logistics, VariantKey};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_ui::ctx::UpdateCtx;
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::mission_results::{MissionResults, PILOT_DONE, SALVAGE_DONE};
use mc2_ui::results_salvage::{SalvageRow, ROW_DISABLED};
use mc2_ui::Loader;

struct Setup {
    h: Harness,
    vfs: Vfs,
    logistics: Logistics,
}

fn setup() -> Option<Setup> {
    let root = std::env::var_os("MC2_DATA")?;
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let logistics = Logistics::with_campaign(data, "campaign").expect("campaign");
    Some(Setup {
        h: Harness::new(),
        vfs,
        logistics,
    })
}

/// The sounds every frame of the test asked for, in order.
fn heard(s: &Setup, sounds: &mut Vec<i32>) {
    sounds.extend_from_slice(&s.h.out.sounds);
}

/// A press and release on a point, after the double-click clock has run out.
fn frame(s: &mut Setup, results: &mut MissionResults, events: &[InputEvent]) {
    s.h.frame(events, |ctx| results.update(ctx, &mut s.logistics));
}

/// Plays frames until the exit animation after Done has finished and the flow moved on.
fn run_exit(s: &mut Setup, results: &mut MissionResults) {
    let start = results.stage();
    for _ in 0..600 {
        if results.stage() != start {
            return;
        }
        frame(s, results, &[]);
    }
    panic!("the exit never finished");
}

fn click_at(s: &mut Setup, results: &mut MissionResults, (x, y): (f32, f32)) {
    for _ in 0..8 {
        frame(s, results, &[]);
    }
    frame(s, results, &[move_to(x, y)]);
    frame(s, results, &[left(true)]);
    frame(s, results, &[left(false)]);
    frame(s, results, &[]);
}

/// The campaign's own results for a won mission: the pilot of the first mech of the force
/// gains skill (`pmwdagger`, 50/48 green, is promoted to regular by a 54/52 mission) and a
/// second design is wrecked, so the salvage screen offers it.
fn outcome(pilot_file: &str, variant: &str, wreck: &str) -> Outcome {
    let mover = MoverReport {
        on_gui: true,
        is_battle_mech: true,
        home_commander: true,
        is_air: false,
        destroyed: false,
        disabled: false,
        variant_name: variant.to_string(),
        pilot_file: Some(pilot_file.to_string()),
        paint_scheme: [0; 3],
        pilot: PilotReport {
            gunnery_rank: 50.0,
            piloting_rank: 48.0,
            gunnery_points: 100.0,
            piloting_points: 400.0,
            ..PilotReport::default()
        },
    };
    Outcome {
        result: MissionResult::PlayerWinBig,
        movers: vec![mover],
        mechs: vec![MechSummary::wreck(wreck, 50.0)],
        ..Outcome::won()
    }
}

#[test]
fn salvage_one_mech_and_promote_one_pilot_through_the_screens() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    // the first mission of the campaign is won without a salvage screen (its `play_salvage` is
    // off), so the flow under test is the second mission's
    s.logistics
        .apply_outcome(&Outcome::won())
        .expect("the first mission");
    let pilot = s
        .logistics
        .pilot_index("pmwdagger")
        .expect("the pilot is in the roster");
    let designs: Vec<VariantKey> = s.logistics.purchasable_mechs();
    let (carried, wrecked) = (designs[0], designs[1]);
    let carried_name = s.logistics.variant(carried).expect("variant").name.clone();
    let wrecked_name = s.logistics.variant(wrecked).expect("variant").name.clone();

    // the force: one mech with the pilot, in the force group
    let mech = s.logistics.purchase_mech(carried).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");
    let rank_before = s.logistics.pilots()[pilot].rank;
    let c_bills_before = s.logistics.c_bills();

    // the mission ends in a win: the campaign applies it, then the flow starts
    let won = outcome("pmwdagger", &carried_name, &wrecked_name);
    s.logistics.apply_outcome(&won).expect("apply_outcome");
    assert!(
        !s.logistics.skip_salvage_screen(),
        "the second mission salvages"
    );

    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    // `bDone` starts false (`missionresults.cpp:26`): a flow that has not begun is not done, and
    // nothing is autosaved for it
    assert!(!results.is_done(), "not done before begin");
    assert!(results.autosave(&s.logistics).is_none());
    s.h.frame(&[], |ctx| results.begin(ctx, &mut s.logistics, &won));
    assert_eq!(
        results.stage(),
        Some(ResultsStage::Salvage),
        "the salvage screen is first"
    );
    assert!(!results.is_done());
    // the autosave runs only after the results finish (`missionresults.cpp:113`)
    assert!(
        results.autosave(&s.logistics).is_none(),
        "no autosave while the salvage is open"
    );
    assert_eq!(results.salvage.list.item_count(), 1, "one wreck is offered");

    // salvage: tick the offer through its check box (the left edge of the row)
    let row = results.salvage.list.item(0).expect("row").obj();
    let box_at = (row.left() + 10.0, row.top() + row.height() / 2.0);
    click_at(&mut s, &mut results, box_at);
    let cost = s.logistics.c_bills();
    assert!(cost < c_bills_before, "ticking charges the salvage price");
    let price = c_bills_before - cost;
    assert!(price > 0);
    assert!(
        results
            .salvage
            .list
            .item(0)
            .and_then(|i| i.as_any().downcast_ref::<SalvageRow>())
            .is_some_and(SalvageRow::is_checked),
        "the offer shows as ticked"
    );

    // Done on the salvage screen moves to the pilot review
    let done = centre(results.salvage.screen.button(SALVAGE_DONE).expect("Done"));
    click_at(&mut s, &mut results, done);
    run_exit(&mut s, &mut results);
    assert_eq!(results.stage(), Some(ResultsStage::PilotReview));
    assert!(!results.is_done());
    // the autosave runs only after the results finish (`missionresults.cpp:113`)
    assert!(
        results.autosave(&s.logistics).is_none(),
        "no autosave during the pilot review"
    );
    let salvaged = s
        .logistics
        .inventory()
        .iter()
        .filter(|m| {
            s.logistics
                .variant(m.variant)
                .is_some_and(|v| v.name == wrecked_name)
        })
        .count();
    assert_eq!(salvaged, 1, "the salvaged mech joined the bay");

    // the pilot review: the promotion is reported and the screen lists the pilot
    let review = results.pilot_review().expect("a review");
    let promoted = review
        .active
        .iter()
        .find(|r| r.pilot == pilot)
        .expect("the pilot is active");
    assert!(promoted.promoted, "the pilot moves up a rank");
    assert!(
        results.pilots.list.rows() >= 2,
        "the review lists the pilot"
    );

    // the promoted pilot picks a specialty once their reveal has run: the pop-up opens and the
    // review's Done waits for the pick
    let mut waited = 0;
    while results.pilots.picker_pilot().is_none() {
        frame(&mut s, &mut results, &[]);
        waited += 1;
        assert!(waited < 2000, "the promotion pop-up never opened");
    }
    assert_eq!(results.pilots.picker_pilot(), Some(pilot));
    assert!(
        !results
            .pilots
            .screen
            .button(PILOT_DONE)
            .expect("Done")
            .is_enabled(),
        "Done is disabled until the pick"
    );
    pick_first_specialty(&mut s, &mut results);
    assert_eq!(results.pilots.picker_pilot(), None, "the pick was taken");
    assert_eq!(
        s.logistics.pilots()[pilot].specialty_skill_indices(),
        vec![0],
        "the first skill on offer was written to the pilot"
    );

    // Done on the review finishes the flow
    let done = centre(results.pilots.screen.button(PILOT_DONE).expect("Done"));
    // the shipped `PilotReviewFadeOutAnimation` has no offsets, so the exit shows as the
    // one second fade to black (`beginFadeOut(1.0)`): the black overlay's alpha climbs
    let black_alpha = |s: &Setup, results: &mut MissionResults| {
        let dl = s.h.render(|ctx| results.render(ctx));
        let (w, h) = (dl.screen.0 as f32, dl.screen.1 as f32);
        dl.vertices
            .chunks(4)
            .filter(|q| {
                q.iter().all(|v| v.argb & 0x00ff_ffff == 0)
                    && q.iter().any(|v| v.x == 0.0 && v.y == 0.0)
                    && q.iter().any(|v| v.x == w && v.y == h)
            })
            .flat_map(|q| q.iter().map(|v| v.argb >> 24))
            .max()
            .unwrap_or(0)
    };
    // the review fades back in after the promotion pop-up: let that finish first
    for _ in 0..90 {
        frame(&mut s, &mut results, &[]);
        black_alpha(&s, &mut results);
    }
    let at_rest = black_alpha(&s, &mut results);
    click_at(&mut s, &mut results, done);
    let start = results.stage();
    let mut peak = at_rest;
    for _ in 0..600 {
        if results.stage() != start {
            break;
        }
        peak = peak.max(black_alpha(&s, &mut results));
        frame(&mut s, &mut results, &[]);
    }
    assert!(
        peak > at_rest,
        "the pilot review never faded: {at_rest} {peak}"
    );
    assert!(results.is_done());

    // the campaign state after the results
    let rank_after = s.logistics.pilots()[pilot].rank;
    assert_eq!(rank_after, RANK_REGULAR, "promoted to regular");
    assert!(rank_after > rank_before);
    let autosave = results
        .autosave(&s.logistics)
        .expect("an autosave after a campaign mission");
    assert_eq!(autosave.mission(), s.logistics.last_mission());
}

fn centre(b: &mc2_ui::widgets::AnimButton) -> (f32, f32) {
    (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    )
}

/// The promoted pilot's picker: the first skill on offer is clicked, then the picker's Done.
fn pick_first_specialty(s: &mut Setup, results: &mut MissionResults) {
    // the pop-up slides in first
    for _ in 0..120 {
        frame(s, results, &[]);
    }
    let promotion = &results.pilots.promotion;
    let skill_row = (0..promotion.list.item_count())
        .find(|&i| promotion.rows.get(i).copied().flatten().is_some())
        .expect("a skill to pick");
    let row = promotion.list.item(skill_row).expect("row").obj();
    let row_at = (row.left() + 10.0, row.top() + row.height() / 2.0);
    click_at(s, results, row_at);
    let choose = centre(results.pilots.choose_button().expect("choose"));
    click_at(s, results, choose);
    // both halves slide away
    for _ in 0..240 {
        frame(s, results, &[]);
    }
}

#[test]
fn a_skipped_salvage_screen_goes_straight_to_the_review() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    let pilot = s.logistics.pilot_index("pmwdagger").expect("roster");
    let designs = s.logistics.purchasable_mechs();
    let mech = s.logistics.purchase_mech(designs[0]).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");
    // no wreck: nothing to salvage, so the flow starts at the review
    let won = Outcome {
        mechs: Vec::new(),
        ..outcome(
            "pmwdagger",
            &s.logistics
                .variant(designs[0])
                .expect("variant")
                .name
                .clone(),
            "",
        )
    };
    s.logistics.apply_outcome(&won).expect("apply_outcome");
    // the mission's salvage screen is off (`skipSalvageScreen`), so the flow does not offer any
    assert!(
        s.logistics.skip_salvage_screen(),
        "the salvage screen is off for this mission"
    );
    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx: &mut UpdateCtx<'_>| {
        results.begin(ctx, &mut s.logistics, &won)
    });
    if results.stage() == Some(ResultsStage::PilotReview) {
        let mut waited = 0;
        while !results.pilots.list.is_done() && waited < 2000 {
            frame(&mut s, &mut results, &[]);
            if results.pilots.picker_pilot().is_some() {
                pick_first_specialty(&mut s, &mut results);
            }
            waited += 1;
        }
        let done = centre(results.pilots.screen.button(PILOT_DONE).expect("Done"));
        click_at(&mut s, &mut results, done);
        run_exit(&mut s, &mut results);
        assert!(results.is_done());
    } else {
        assert!(
            results.is_done(),
            "no salvage and no review left: the flow is done"
        );
    }
}

/// Wrecks offered by the salvage screen in the scroll test: more rows than its box holds.
const OFFERS: usize = 40;

#[test]
fn a_long_salvage_list_scrolls_to_its_last_offer_and_ticks_it() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    s.logistics
        .apply_outcome(&Outcome::won())
        .expect("the first mission");
    let pilot = s
        .logistics
        .pilot_index("pmwdagger")
        .expect("the pilot is in the roster");
    let designs: Vec<VariantKey> = s.logistics.purchasable_mechs();
    let (carried, wrecked) = (designs[0], designs[1]);
    let carried_name = s.logistics.variant(carried).expect("variant").name.clone();
    let wrecked_name = s.logistics.variant(wrecked).expect("variant").name.clone();
    let mech = s.logistics.purchase_mech(carried).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");

    let mut won = outcome("pmwdagger", &carried_name, &wrecked_name);
    won.mechs = (0..OFFERS)
        .map(|_| MechSummary::wreck(&wrecked_name, 50.0))
        .collect();
    s.logistics.apply_outcome(&won).expect("apply_outcome");
    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx| results.begin(ctx, &mut s.logistics, &won));
    assert_eq!(results.salvage.list.item_count(), OFFERS);

    // the offers past the box are not shown, so they cannot be picked yet
    let last = OFFERS - 1;
    let shown = |r: &MissionResults, i: usize| r.salvage.list.item(i).expect("row").obj().show;
    frame(&mut s, &mut results, &[]);
    assert!(!shown(&results, last), "the last offer is below the box");

    // the scroll bar moves the list to its end (`aListBox::scroll` stops there)
    for _ in 0..1000 {
        results.salvage.list.scroll(10);
    }
    frame(&mut s, &mut results, &[]);
    assert!(shown(&results, last), "the last offer scrolls into view");
    assert!(!shown(&results, 0), "the first offer scrolls out");

    // and its check box ticks it
    let before = s.logistics.c_bills();
    let scroll = results.salvage.list.scroll_pos();
    let row = results.salvage.list.item(last).expect("row").obj();
    let box_at = (row.left() + 10.0, row.top() + row.height() / 2.0);
    click_at(&mut s, &mut results, box_at);
    assert!(s.logistics.c_bills() < before, "ticking charges the price");
    // the list is updated in place: it keeps its scroll, so the ticked offer stays in view
    assert_eq!(
        results.salvage.list.scroll_pos(),
        scroll,
        "the scroll stays"
    );
    assert!(shown(&results, last), "the ticked offer stays in view");
    assert!(results
        .salvage
        .list
        .item(last)
        .and_then(|i| i.as_any().downcast_ref::<SalvageRow>())
        .is_some_and(SalvageRow::is_checked));
}

/// `SalvageListItem::update`: a box whose price is more than the C-Bills stays disabled, and a
/// click on it changes nothing. The offer comes from the production path (`salvage_offers`).
/// The shipped data has no fractional 80% price (every variant costs a multiple of five), so the
/// float edge is covered by the campaign's `a_disabled_box_charges_nothing_when_ticked`.
#[test]
fn a_disabled_salvage_box_does_not_tick_on_a_click() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    s.logistics
        .apply_outcome(&Outcome::won())
        .expect("the first mission");
    let pilot = s.logistics.pilot_index("pmwdagger").expect("roster");
    let designs: Vec<VariantKey> = s.logistics.purchasable_mechs();
    let (carried, wrecked) = (designs[0], designs[1]);
    let carried_name = s.logistics.variant(carried).expect("variant").name.clone();
    let wrecked_name = s.logistics.variant(wrecked).expect("variant").name.clone();
    let mech = s.logistics.purchase_mech(carried).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");
    let won = outcome("pmwdagger", &carried_name, &wrecked_name);
    s.logistics.apply_outcome(&won).expect("apply_outcome");

    // the price of the wreck, and one C-Bill less than it: the box is greyed out
    let price = s
        .logistics
        .salvage_offers(&[SalvageCandidate {
            variant_name: wrecked_name.clone(),
            tonnage: 50.0,
            paint_scheme: [0; 3],
            armor: [(0.0, 0.0, false); 11],
        }])
        .remove(0)
        .cost;
    assert!(price > 1, "the wreck has a price");
    let have = i64::from(price) - 1;
    let spend = s.logistics.c_bills() - have;
    s.logistics.decrement_c_bills(spend);
    assert_eq!(s.logistics.c_bills(), have);

    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx| results.begin(ctx, &mut s.logistics, &won));
    assert_eq!(results.stage(), Some(ResultsStage::Salvage));
    assert_eq!(results.salvage.list.item_count(), 1, "one wreck is offered");
    let selected = results.salvage.list.selected_item();
    let row = results.salvage.list.item(0).expect("the offer");
    // the row is also the list's selection, which replaces its state, so the grey is checked
    assert_eq!(
        row.as_any()
            .downcast_ref::<SalvageRow>()
            .expect("a salvage row")
            .colour(),
        ROW_DISABLED,
        "the box is greyed out"
    );
    let o = row.obj();
    let box_at = (o.left() + 10.0, o.top() + o.height() / 2.0);
    let mut sounds = Vec::new();
    for _ in 0..8 {
        frame(&mut s, &mut results, &[]);
    }
    for events in [
        vec![move_to(box_at.0, box_at.1)],
        vec![left(true)],
        vec![left(false)],
        vec![],
    ] {
        frame(&mut s, &mut results, &events);
        heard(&s, &mut sounds);
    }
    assert!(
        sounds.contains(&mc2_ui::ctx::sfx::LOG_WRONGBUTTON),
        "the refused row plays the wrong-button sound: {sounds:?}"
    );

    assert_eq!(s.logistics.c_bills(), have, "nothing is charged");
    assert!(
        results
            .salvage
            .list
            .item(0)
            .and_then(|i| i.as_any().downcast_ref::<SalvageRow>())
            .is_some_and(|r| !r.is_checked()),
        "the offer stays unticked"
    );
    assert_eq!(
        results.salvage.list.selected_item(),
        selected,
        "the selection holds"
    );
}

/// `pilotreviewarea.cpp`: the skills are cast to `long` before they are shown, so a gunnery of
/// 50.6 reads 50, where rounding would read 51. The review has no wreck to salvage here, so the
/// flow starts at the pilot review.
#[test]
fn a_fractional_skill_shows_the_integer_the_original_cast_gives() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    let pilot = s.logistics.pilot_index("pmwdagger").expect("roster");
    let designs = s.logistics.purchasable_mechs();
    let mech = s.logistics.purchase_mech(designs[0]).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");
    let name = s
        .logistics
        .variant(designs[0])
        .expect("variant")
        .name
        .clone();
    let mut won = Outcome {
        mechs: Vec::new(),
        ..outcome("pmwdagger", &name, "")
    };
    won.movers[0].pilot.gunnery_rank = 50.6;
    s.logistics.apply_outcome(&won).expect("apply_outcome");

    let gunnery = s.logistics.pilots()[pilot].gunnery;
    assert_ne!(
        gunnery.round() as i64,
        gunnery as i64,
        "the fixture keeps a fraction that rounding would change: {gunnery}"
    );
    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx: &mut UpdateCtx<'_>| {
        results.begin(ctx, &mut s.logistics, &won)
    });
    assert_eq!(results.stage(), Some(ResultsStage::PilotReview));

    let cards = results.pilots.list.active_cards();
    let card = cards
        .iter()
        .find(|c| c.pilot == pilot)
        .expect("the active pilot has a card");
    assert_eq!(
        card.texts()[3],
        (gunnery as i64).to_string(),
        "the card shows the truncated gunnery"
    );
}

#[test]
fn ticking_an_offer_plays_the_window_sound_and_counts_the_c_bills_down() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    s.logistics
        .apply_outcome(&Outcome::won())
        .expect("the first mission");
    let pilot = s.logistics.pilot_index("pmwdagger").expect("roster");
    let designs: Vec<VariantKey> = s.logistics.purchasable_mechs();
    let (carried, wrecked) = (designs[0], designs[1]);
    let carried_name = s.logistics.variant(carried).expect("variant").name.clone();
    let wrecked_name = s.logistics.variant(wrecked).expect("variant").name.clone();
    let mech = s.logistics.purchase_mech(carried).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");
    let won = outcome("pmwdagger", &carried_name, &wrecked_name);
    s.logistics.apply_outcome(&won).expect("apply_outcome");
    let before = s.logistics.c_bills();

    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx| results.begin(ctx, &mut s.logistics, &won));
    assert_eq!(results.stage(), Some(ResultsStage::Salvage));
    let o = results.salvage.list.item(0).expect("the offer").obj();
    let box_at = (o.left() + 10.0, o.top() + o.height() / 2.0);
    for _ in 0..8 {
        frame(&mut s, &mut results, &[]);
    }
    let mut sounds = Vec::new();
    let mut shown = Vec::new();
    let text = |r: &MissionResults| r.salvage.screen.texts[3].text.clone();
    for events in [
        vec![move_to(box_at.0, box_at.1)],
        vec![left(true)],
        vec![left(false)],
    ] {
        frame(&mut s, &mut results, &events);
        heard(&s, &mut sounds);
        shown.push(text(&results));
    }
    let after = s.logistics.c_bills();
    assert!(after < before, "the offer was bought");
    assert!(
        sounds.contains(&65),
        "spending C-Bills plays WINDOW_CLOSE: {sounds:?}"
    );
    // the text counts down from the old amount to the new one over half a second
    for _ in 0..30 {
        frame(&mut s, &mut results, &[]);
        shown.push(text(&results));
    }
    assert!(
        shown.iter().any(|t| *t == before.to_string())
            || shown
                .iter()
                .any(|t| t.parse::<i64>().is_ok_and(|v| v < before && v > after)),
        "the text passes through the amounts between: {shown:?}"
    );
    assert_eq!(shown.last().unwrap(), &after.to_string());
}

/// The pilot review plays the C++ reveal: after the start delay the pilot's kill markers come one
/// every half second (`LOG_KILLMARKER`), then the Valor medal (`LOG_PILOTMEDAL`) and the
/// promotion (`LOG_PROMOTED`), and the list shows each as it appears
/// (`pilotreviewarea.cpp:560-815`, `ActivePilotListItem::render`).
#[test]
fn the_pilot_review_reveals_kills_medals_and_promotion_in_order_with_sounds() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    let pilot = s.logistics.pilot_index("pmwdagger").expect("roster");
    let designs = s.logistics.purchasable_mechs();
    let mech = s.logistics.purchase_mech(designs[0]).expect("purchase");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics
        .add_mech_to_force_group(mech, 1)
        .expect("force group");
    let name = s
        .logistics
        .variant(designs[0])
        .expect("variant")
        .name
        .clone();
    let mut won = Outcome {
        mechs: Vec::new(),
        ..outcome("pmwdagger", &name, "")
    };
    // three mechs and a vehicle: the Valor medal (3-6 non-air mechs)
    won.movers[0].pilot.killed = vec![
        KilledKind::BattleMech { air: false },
        KilledKind::GroundVehicle,
        KilledKind::BattleMech { air: false },
        KilledKind::BattleMech { air: false },
    ];
    s.logistics.apply_outcome(&won).expect("apply_outcome");
    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx: &mut UpdateCtx<'_>| {
        results.begin(ctx, &mut s.logistics, &won)
    });
    let review = results.pilot_review().expect("review");
    let r = review
        .active
        .iter()
        .find(|r| r.pilot == pilot)
        .expect("active");
    assert_eq!(r.kills.len(), 4, "the review carries the pilot's kills");
    assert!(r.promoted && !r.new_medals.is_empty());
    let medal_count = r.new_medals.len();

    let shown = |results: &MissionResults| -> usize {
        results
            .pilots
            .list
            .active_cards()
            .iter()
            .map(|c| c.kills_shown())
            .sum()
    };
    // before the start delay no event is revealed
    assert_eq!(shown(&results), 0);

    let mut sounds = Vec::new();
    let mut kill_rows = Vec::new();
    for _ in 0..700 {
        frame(&mut s, &mut results, &[]);
        heard(&s, &mut sounds);
        kill_rows.push(shown(&results));
    }
    let reveal: Vec<i32> = sounds
        .iter()
        .copied()
        .filter(|&id| (102..=104).contains(&id))
        .collect();
    let mut expected = vec![103; 4];
    expected.extend(vec![102; medal_count]);
    expected.push(104);
    assert_eq!(
        reveal, expected,
        "four kill markers, each medal, then the promotion"
    );
    assert_eq!(shown(&results), 4, "all four kill markers are listed");
    assert!(
        kill_rows.contains(&1),
        "the markers were revealed over time"
    );
}

#[test]
fn a_reveal_timeline_follows_flash_time() {
    use mc2_campaign::results::ReviewedPilot;
    use mc2_ui::mission_results::Reveal;
    let r = ReviewedPilot {
        pilot: 0,
        promoted: true,
        gain: (0.0, 0.0),
        new_medals: vec![1],
        kills: vec![KilledKind::GroundVehicle; 2],
        kill_units: vec![Default::default(); 2],
    };
    let mut rv = Reveal::new(&r);
    // flashTime = 1.5 (promoted) + 2.0 + 0.5 (medal) + 1.5 + 1.0 (kills)
    assert!((rv.flash_time() - 6.5).abs() < 1e-6);
    let mut sounds = Vec::new();
    let mut t = 0.0;
    while !rv.is_done() {
        rv.advance(0.05, &mut sounds);
        t += 0.05;
        assert!(t < 20.0, "the reveal finishes");
    }
    assert!(t > 6.4 && t < 6.7, "done just after flash time: {t}");
    assert_eq!(sounds, vec![103, 103, 102, 104]);
}

/// Done starts the exit and the flow moves on only once it has played
/// (`salvagemecharea.cpp:222-249`, `pilotreviewarea.cpp:264-281`).
#[test]
fn done_waits_for_the_exit_animation() {
    let Some(mut s) = setup() else { return };
    s.logistics.add_c_bills(10_000_000);
    s.logistics
        .apply_outcome(&Outcome::won())
        .expect("the first mission");
    let designs: Vec<VariantKey> = s.logistics.purchasable_mechs();
    let name = |s: &Setup, k| s.logistics.variant(k).expect("variant").name.clone();
    let (a, b) = (name(&s, designs[0]), name(&s, designs[1]));
    let mech = s.logistics.purchase_mech(designs[0]).expect("purchase");
    let pilot = s.logistics.pilot_index("pmwdagger").expect("pilot");
    s.logistics.set_mech_pilot(mech, Some(pilot));
    s.logistics.add_mech_to_force_group(mech, 1).expect("group");
    let won = outcome("pmwdagger", &a, &b);
    s.logistics.apply_outcome(&won).expect("apply_outcome");
    let mut results =
        MissionResults::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("results");
    s.h.frame(&[], |ctx| results.begin(ctx, &mut s.logistics, &won));

    let bbox = |s: &Setup, results: &mut MissionResults| {
        let dl = s.h.render(|ctx| results.render(ctx));
        dl.vertices
            .iter()
            .fold((f32::MAX, f32::MAX), |(x, y), v| (x.min(v.x), y.min(v.y)))
    };
    let at_rest = bbox(&s, &mut results);
    let done = centre(results.salvage.screen.button(SALVAGE_DONE).expect("Done"));
    for _ in 0..8 {
        frame(&mut s, &mut results, &[]);
    }
    frame(&mut s, &mut results, &[move_to(done.0, done.1)]);
    frame(&mut s, &mut results, &[left(true)]);
    frame(&mut s, &mut results, &[left(false)]);
    // The release starts the authored 0.7 s / -800 px exit animation.
    let mut exit_frames = 1;
    assert_eq!(results.stage(), Some(ResultsStage::Salvage));
    let mut endpoint_dx = 0.0;
    while results.stage() == Some(ResultsStage::Salvage) {
        // the exit slides the screen (`SalvageMechScreen::render` with `exitAnim` offsets)
        endpoint_dx = bbox(&s, &mut results).0 - at_rest.0;
        frame(&mut s, &mut results, &[]);
        exit_frames += 1;
        assert!(exit_frames < 600, "the exit never finished");
    }
    assert_eq!(
        endpoint_dx, -800.0,
        "the exit reaches its authored endpoint"
    );
    assert!(
        (21..=23).contains(&exit_frames),
        "the 0.7 s exit took {exit_frames} frames at 30 Hz"
    );
    assert_eq!(results.stage(), Some(ResultsStage::PilotReview));
}
