// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The campaign pages of the logistics sequence (purchase, mech lab, in-campaign options) driven
//! headlessly through the production page adapters of [`mc2_app::pages`] over the shipped data.
//!
//! Each test builds the page list the production host builds (`ui_host::build_sequence`,
//! `rust/mc2-app/src/ui_host.rs:347`), swaps the three placeholders for the real pages with
//! [`mc2_app::pages::install`], and drives the sequence by input only: the pointer for the
//! screens, the keyboard for the lab's save dialog's name. The expected
//! values come from the shipped data through `mc2-campaign` (variant costs, the component
//! catalogue, the C-Bill rules of `code/mechpurchasescreen.cpp` and `code/mechlabscreen.cpp`);
//! every frame is asserted to report neither [`Flow::Unavailable`] nor [`Flow::Failed`].
//!
//! Skips cleanly without `MC2_DATA`.

use std::path::PathBuf;
use std::sync::Arc;

use mc2_app::pages::{self, CampaignOptions, MechLabPage, PurchasePage};
use mc2_campaign::interface::Outcome;
use mc2_campaign::{GameData, Logistics, MechKey, VariantKey};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_platform::keys;
use mc2_sim_data::component::ComponentForm;
use mc2_ui::ctx::{Loader, UiAssets, UiOutput};
use mc2_ui::harness::events::{left, move_to};
use mc2_ui::input::UiInput;
use mc2_ui::logistics::briefing::Briefing;
use mc2_ui::logistics::mech_bay::MechBay;
use mc2_ui::logistics::mission_select::MissionSelect;
use mc2_ui::logistics::pilot_ready::PilotReady;
use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
use mc2_ui::logistics::{ids, Page, Placeholder};
use mc2_ui::logistics_ids::{button, form};
use mc2_ui::mech_purchase::MechEntry;
use mc2_ui::screens::dialog::{NO, YES};
use mc2_ui::screens::options::MSB_TAB0;
use mc2_ui::screens::{OptionsEnv, Prefs};

/// Seconds per frame (the app's fixed UI step).
const DT: f32 = 1.0 / 30.0;
/// The frames the double-click clock needs to run out (0.2 s at 30 fps): a click within it of the
/// last one is a double click, as a player's would be.
const DOUBLE_CLICK_FRAMES: usize = 8;
/// The name the lab tests type for the edited loadout: the save dialog's field takes 14
/// characters (`edits[0].limitEntry(14)`, `code/logisticsdialog.cpp`), so the name fits what a
/// player can type.
const LAB_VARIANT_NAME: &str = "Pages Test Fit";

struct Setup {
    vfs: Vfs,
    assets: UiAssets,
    lg: Logistics,
    input: UiInput,
    seq: Sequence,
    prefs: Prefs,
}

/// The page list of the production host (`ui_host::build_sequence`): the real screens, with the
/// purchase, mech lab and options slots as the placeholders the host ships with.
fn host_pages(ld: &mut Loader<'_>) -> Vec<Box<dyn Page>> {
    vec![
        Box::new(MissionSelect::load(ld).expect("mission selection")),
        Box::new(Briefing::load(ld).expect("briefing")),
        Box::new(MechBay::load(ld).expect("mech bay")),
        Box::new(PilotReady::load(ld).expect("pilot ready")),
        Box::new(Placeholder::unavailable("purchase")),
        Box::new(Placeholder::unavailable("mech lab")),
        Box::new(Placeholder::new("loading")),
        Box::new(Placeholder::unavailable("options")),
    ]
}

/// The game over the shipped data, at a mission whose logistics the purchase screen and the lab
/// are part of: the first mission won, so the campaign is at `mc2_02` and `skipPurchasing` is off
/// (`code/logisticsdata.cpp`, the campaign script's mission attributes). With `install`, the
/// three placeholders are swapped for the production pages; without them the host's placeholders
/// stand, as they do before the coordinator wires the pages in.
fn setup(installed: bool, volumes_live: bool, persist: Option<PathBuf>) -> Option<Setup> {
    let root = std::env::var_os("MC2_DATA")?;
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let mut lg = Logistics::with_campaign(data, "campaign").expect("campaign");
    lg.set_mission_completed(&Outcome::won())
        .expect("the first mission is won");
    assert_eq!(lg.current_mission(), "mc2_02");
    assert!(!lg.skip_logistics(), "this mission plays the logistics");
    assert!(
        !lg.skip_purchasing(),
        "this mission has the purchase screen"
    );

    let mut assets = UiAssets::default();
    let prefs = Prefs::default();
    let mut pages = host_pages(&mut Loader::new(&mut vfs, &mut assets));
    if installed {
        pages::install(
            &mut vfs,
            &mut assets,
            &mut pages,
            pages::OptionsSetup {
                prefs: prefs.clone(),
                original: prefs.clone(),
                env: OptionsEnv::default(),
                volumes_live,
                persist_to: persist,
            },
        )
        .expect("the production pages install over the placeholders");
    }
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut Loader::new(&mut vfs, &mut assets), &mut lg);
    input.flush_keys();
    // Entering mission selection reports the mission's deferred movie once (`mission_select.rs`
    // names it; playback is the movie lane's, not this slice's wiring), so the notice is consumed
    // here, by name: every frame the tests drive after it must be clean.
    input.begin_frame(DT);
    let mut out = UiOutput::default();
    let mut ld = Loader::new(&mut vfs, &mut assets);
    let flow = seq.update(&mut ld, &mut input, &mut out, DT, &mut lg);
    input.end_frame();
    assert!(
        matches!(
            flow,
            Flow::Running | Flow::Unavailable("mission movie playback")
        ),
        "only the deferred mission movie may be unavailable here: {flow:?}"
    );
    Some(Setup {
        vfs,
        assets,
        lg,
        input,
        seq,
        prefs,
    })
}

/// One frame of the sequence with the frame's events, with the flow as it comes: the frames a
/// test expects to fail (`Flow::Failed`) are driven through this, not through [`frame`].
fn frame_as_is(s: &mut Setup, events: &[InputEvent]) -> Flow {
    for e in events {
        s.input.handle(e);
    }
    s.input.begin_frame(DT);
    let mut out = UiOutput::default();
    let mut ld = Loader::new(&mut s.vfs, &mut s.assets);
    let flow = s.seq.update(&mut ld, &mut s.input, &mut out, DT, &mut s.lg);
    s.input.end_frame();
    flow
}

/// One frame of the sequence with the frame's events. The flow is asserted: a page that reports
/// itself unavailable, or a fault, is a failure of this slice, not an outcome to step over.
fn frame(s: &mut Setup, events: &[InputEvent]) -> Flow {
    let flow = frame_as_is(s, events);
    assert!(
        !matches!(flow, Flow::Unavailable(_)),
        "a page the sequence needs is not wired up: {flow:?}"
    );
    assert!(
        flow != Flow::Failed,
        "the sequence failed: {:?}",
        s.seq.fault()
    );
    flow
}

/// Types `text` into the focused edit control, one key press a frame, the way a player types:
/// `gos_GetKey` queues the key index in bits 8.. and the character in the low byte, which is what
/// `aEdit` reads (`rust/mc2-ui/src/widgets/edit.rs`, `gui/aedit.cpp`).
fn type_text(s: &mut Setup, text: &str) {
    for c in text.chars() {
        let key = match c {
            ' ' => keys::KEY_SPACE,
            '0'..='9' => keys::KEY_0 + u32::from(c as u8 - b'0'),
            'a'..='z' | 'A'..='Z' => keys::KEY_A + u32::from(c.to_ascii_uppercase() as u8 - b'A'),
            _ => panic!("the tests type letters, digits and spaces only: {c:?}"),
        };
        frame(
            s,
            &[InputEvent::Key {
                key,
                phys_id: key as u16,
                pressed: true,
                repeat: false,
                text: Some(c.to_string()),
            }],
        );
    }
}

/// A click the way a player makes one: the pointer comes over the target, the button goes down,
/// then up, each in its own frame, after the double-click clock has run out.
fn click(s: &mut Setup, at: (f32, f32)) -> Flow {
    for _ in 0..DOUBLE_CLICK_FRAMES {
        frame(s, &[]);
    }
    frame(s, &[move_to(at.0, at.1)]);
    frame(s, &[left(true)]);
    let flow = frame(s, &[left(false)]);
    for _ in 0..DOUBLE_CLICK_FRAMES {
        frame(s, &[]);
    }
    flow
}

/// The centre of a button of the page `page`, in UI coordinates.
fn page_button(s: &mut Setup, page: usize, id: i32) -> (f32, f32) {
    let b = s
        .seq
        .page_mut(page)
        .base()
        .button(id)
        .unwrap_or_else(|| panic!("button {id} of page {page}"));
    (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    )
}

/// The purchase page of the sequence.
fn purchase_page(s: &mut Setup) -> &mut PurchasePage {
    s.seq
        .page_mut(slot::PURCHASE)
        .as_any_mut()
        .downcast_mut::<PurchasePage>()
        .expect("the production purchase page")
}

/// The centre of row `i` of one of the purchase screen's lists.
fn purchase_row(s: &mut Setup, owned: bool, i: usize) -> (f32, f32) {
    let p = purchase_page(s);
    let list = if owned {
        &p.purchase.inventory
    } else {
        &p.purchase.variants
    };
    let o = list.item(i).expect("a row").obj();
    (
        o.global_x() as f32 + o.width() / 2.0,
        o.global_y() as f32 + o.height() / 2.0,
    )
}

/// The mech lab page of the sequence.
fn lab_page(s: &mut Setup) -> &mut MechLabPage {
    s.seq
        .page_mut(slot::MECH_LAB)
        .as_any_mut()
        .downcast_mut::<MechLabPage>()
        .expect("the production mech lab page")
}

/// The in-campaign options page of the sequence.
fn options_page(s: &mut Setup) -> &mut CampaignOptions {
    s.seq
        .page_mut(slot::OPTIONS)
        .as_any_mut()
        .downcast_mut::<CampaignOptions>()
        .expect("the production options page")
}

/// The centre of a button of the options screen's own layout (the tabs and Yes/No).
fn options_button(s: &mut Setup, id: i32) -> (f32, f32) {
    let b = options_page(s)
        .options
        .screen
        .button(id)
        .unwrap_or_else(|| panic!("options button {id}"));
    (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    )
}

/// A click on a button of a page of the sequence, at `page_button`'s centre.
fn click_page_button(s: &mut Setup, page: usize, id: i32) -> Flow {
    let at = page_button(s, page, id);
    click(s, at)
}

/// A click on a row of one of the purchase screen's lists, at `purchase_row`'s centre.
fn click_purchase_row(s: &mut Setup, owned: bool, i: usize) -> Flow {
    let at = purchase_row(s, owned, i);
    click(s, at)
}

/// A click on the options screen's own layout (the tabs and Yes/No), at `options_button`'s
/// centre.
fn click_options_button(s: &mut Setup, id: i32) -> Flow {
    let at = options_button(s, id);
    click(s, at)
}

/// The campaign's logistics through the selection and briefing screens to the mech bay, the way
/// the host runs it after a mission's results (`MissionBegin`'s grid, `code/missionbegin.cpp`).
fn go_to_bay(s: &mut Setup) {
    assert_eq!(s.seq.current_page(), Some(slot::SELECT));
    click_page_button(s, slot::SELECT, ids::MB_MSG_NEXT);
    assert_eq!(s.seq.current_page(), Some(slot::BRIEFING));
    click_page_button(s, slot::BRIEFING, ids::MB_MSG_NEXT);
    assert_eq!(s.seq.current_page(), Some(slot::MECH_BAY));
}

/// What the player owns outside the force group, as the purchase screen's Cancel restores it:
/// design, pilot and force group of every mech (`prevInventory`).
fn bay_snapshot(lg: &Logistics) -> Vec<(VariantKey, Option<usize>, i32)> {
    lg.inventory()
        .iter()
        .map(|m| (m.variant, m.pilot, m.force_group))
        .collect()
}

/// The first mech for sale the campaign lets the player pay for, with its row (the screen lists
/// `purchasable_mechs` in order) and its price from the shipped data.
fn first_affordable(lg: &Logistics) -> (usize, VariantKey, i32) {
    for (row, &v) in lg.purchasable_mechs().iter().enumerate() {
        if lg.can_purchase_mech(v).is_ok() {
            let cost = lg.variant_cost(v).expect("the variant's price");
            return (row, v, cost);
        }
    }
    panic!("no mech for sale is affordable at this campaign state");
}

/// Cost, weight, heat and max heat of a variant, through the campaign.
fn figures(lg: &Logistics, key: VariantKey) -> [i32; 4] {
    let v = lg.variant(key).expect("variant");
    let ctx = lg.ctx_for(v);
    [v.cost(&ctx), v.weight(&ctx), v.heat(&ctx), v.max_heat(&ctx)]
}

/// The placed parts of a variant: origin cell and catalogue index of each.
fn loadout(lg: &Logistics, key: VariantKey) -> Vec<(i64, i64, usize)> {
    lg.variant(key)
        .expect("variant")
        .components
        .iter()
        .map(|p| (p.x, p.y, p.component))
        .collect()
}

/// A catalogue component.
fn part(lg: &Logistics, index: usize) -> mc2_campaign::component::Component {
    lg.data().catalog.get(index).expect("catalogue").clone()
}

/// The first energy weapon the lab can take (the campaign's `lab_can_add_component`).
fn first_addable_energy(lg: &Logistics) -> Option<usize> {
    lg.available_components().into_iter().find(|&i| {
        let is_energy = lg
            .data()
            .catalog
            .get(i)
            .is_some_and(|c| c.form == ComponentForm::WeaponEnergy);
        let (mut x, mut y) = (-1, -1);
        is_energy && lg.lab_can_add_component(i, &mut x, &mut y).is_ok()
    })
}

/// A purchased mech, the placed part to remove from it, and the energy weapon that then goes in:
/// removing the part makes room, and the energy weapon costs more than the part, so the accept's
/// C-Bill check has a charge to make. The trial mechs are sold back; the found one is kept.
fn swap_candidate(lg: &mut Logistics) -> (MechKey, (i64, i64), usize) {
    for variant in lg.purchasable_mechs() {
        let Ok(mech) = lg.purchase_mech(variant) else {
            continue;
        };
        lg.set_mech_to_modify(mech)
            .expect("the bay opens the lab on this mech");
        let key = lg.variant_being_edited().expect("the edit's working copy");
        let mut found = None;
        for (x, y, removed) in loadout(lg, key) {
            let mut trial = lg.clone();
            if trial.lab_remove_component(x, y).is_err() {
                continue;
            }
            let Some(added) = first_addable_energy(&trial) else {
                continue;
            };
            let cost = |i: usize| lg.data().catalog.get(i).map_or(0, |c| c.cost);
            if cost(added) > cost(removed) {
                found = Some(((x, y), added));
                break;
            }
        }
        lg.cancel_mech_modifications()
            .expect("the trial edit is dropped");
        if let Some((cell, added)) = found {
            return (mech, cell, added);
        }
        lg.sell_mech(mech).expect("the trial mech is sold back");
    }
    panic!("no purchasable mech has a part that an energy weapon can replace at a profit");
}

/// A unique config file for one test, under the cargo target tmp dir (`CARGO_TARGET_TMPDIR`, which
/// follows `CARGO_TARGET_DIR` when cargo redirects its output, so the gate's checkout stays clean).
fn config_file(tag: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(&dir).expect("the target's tmp dir");
    let path = dir.join(format!("pages-options-{tag}.cfg"));
    let _ = std::fs::remove_file(&path);
    path
}

/// The prefs a written config file holds (`Prefs::from_fit` of its bytes).
fn read_prefs(path: &std::path::Path) -> Prefs {
    let bytes = std::fs::read(path).expect("the config file");
    let mut fit = FitIni::from_bytes(bytes).expect("the options.cfg format");
    Prefs::from_fit(&mut fit)
}

#[test]
fn the_purchase_page_buys_a_mech_and_accept_keeps_it() {
    let Some(mut s) = setup(true, true, Some(config_file("purchase-accept"))) else {
        return;
    };
    let start = s.lg.c_bills();
    let snapshot = bay_snapshot(&s.lg);

    go_to_bay(&mut s);
    // the bay's buy button: the sequence moves up to the purchase screen (`MechBayScreen`'s BUY)
    click_page_button(&mut s, slot::MECH_BAY, ids::MB_MSG_BUY);
    assert_eq!(s.seq.current_page(), Some(slot::PURCHASE));

    // the first affordable design, Add (`addSelectedMech`): the campaign moves the C-Bills
    let (row, variant, cost) = first_affordable(&s.lg);
    let name = {
        let item = purchase_page(&mut s)
            .purchase
            .variants
            .item(row)
            .expect("row");
        item.as_any()
            .downcast_ref::<MechEntry>()
            .expect("a mech row")
            .texts()[3]
            .text
            .clone()
    };
    assert_eq!(
        name,
        s.lg.variant(variant).expect("variant").name,
        "the row is the design the campaign priced"
    );
    click_purchase_row(&mut s, false, row);
    click_page_button(&mut s, slot::PURCHASE, button::MB_MSG_ADD);
    assert_eq!(
        s.lg.c_bills(),
        start - i64::from(cost),
        "the C-Bill check took the shipped price"
    );
    let mut owned = bay_snapshot(&s.lg);
    assert_eq!(owned.len(), snapshot.len() + 1, "one more mech is owned");
    assert_eq!(owned.pop(), Some((variant, None, 0)), "the bought mech");

    // Accept (`end` keeps the buys): back down to the bay, the mech and the C-Bills stay
    click_page_button(&mut s, slot::PURCHASE, button::MB_MSG_NEXT);
    assert_eq!(s.seq.current_page(), Some(slot::MECH_BAY));
    assert_eq!(s.lg.c_bills(), start - i64::from(cost));
    assert_eq!(bay_snapshot(&s.lg).len(), snapshot.len() + 1);
}

#[test]
fn the_purchase_page_cancel_restores_the_inventory_and_the_c_bills() {
    let Some(mut s) = setup(true, true, Some(config_file("purchase-cancel"))) else {
        return;
    };
    let start = s.lg.c_bills();
    let snapshot = bay_snapshot(&s.lg);

    go_to_bay(&mut s);
    click_page_button(&mut s, slot::MECH_BAY, ids::MB_MSG_BUY);
    let (row, _, cost) = first_affordable(&s.lg);
    click_purchase_row(&mut s, false, row);
    click_page_button(&mut s, slot::PURCHASE, button::MB_MSG_ADD);
    assert_eq!(s.lg.c_bills(), start - i64::from(cost), "the buy is paid");

    // Cancel (`MechPurchaseScreen::end` without Accept): the buys are sold, the starting
    // inventory bought back
    click_page_button(&mut s, slot::PURCHASE, button::MB_MSG_PREV);
    assert_eq!(s.seq.current_page(), Some(slot::MECH_BAY));
    assert_eq!(s.lg.c_bills(), start, "the C-Bills are back");
    assert_eq!(bay_snapshot(&s.lg), snapshot, "the inventory is restored");
}

#[test]
fn without_install_the_placeholders_report_purchase_unavailable() {
    let Some(mut s) = setup(false, true, None) else {
        return;
    };
    go_to_bay(&mut s);
    // the host's placeholder: the move is refused and the bay stays
    let b = s
        .seq
        .page_mut(slot::MECH_BAY)
        .base()
        .button(ids::MB_MSG_BUY);
    let at = (
        b.expect("the buy button").global_x() as f32 + b.unwrap().width() / 2.0,
        b.unwrap().global_y() as f32 + b.unwrap().height() / 2.0,
    );
    for _ in 0..DOUBLE_CLICK_FRAMES {
        frame_as_is(&mut s, &[]);
    }
    frame_as_is(&mut s, &[move_to(at.0, at.1)]);
    frame_as_is(&mut s, &[left(true)]);
    // the unavailable move is an outcome this test expects, so the frame is driven as it comes
    let flow = frame_as_is(&mut s, &[left(false)]);
    assert_eq!(flow, Flow::Unavailable("purchase"));
    assert_eq!(s.seq.current_page(), Some(slot::MECH_BAY));
}

#[test]
fn install_without_a_config_file_fails_and_the_placeholders_stand() {
    let Some(mut s) = setup(false, true, None) else {
        return;
    };
    // a fresh page list, the way the host builds it: the real screens around the three
    // placeholders
    let mut pages = host_pages(&mut Loader::new(&mut s.vfs, &mut s.assets));
    // the options page is the last of the three to load, and the only one that can fail here
    // (the purchase and mech lab layouts loaded fine above): without a config file to persist
    // to, the install refuses rather than letting a later accept report a success that never
    // happened
    let err = pages::install(
        &mut s.vfs,
        &mut s.assets,
        &mut pages,
        pages::OptionsSetup {
            prefs: s.prefs.clone(),
            original: s.prefs.clone(),
            env: OptionsEnv::default(),
            volumes_live: true,
            persist_to: None,
        },
    )
    .expect_err("the install refuses without a config file");
    assert!(
        err.contains("persist_to"),
        "the host is told what to pass: {err}"
    );
    // the swap is all-or-nothing: the earlier pages loaded, but no slot was replaced, so all
    // three placeholders stand and report their pages unavailable
    for (slot, name) in [
        (slot::PURCHASE, "purchase"),
        (slot::MECH_LAB, "mech lab"),
        (slot::OPTIONS, "options"),
    ] {
        let p = pages[slot]
            .as_any_mut()
            .downcast_mut::<Placeholder>()
            .unwrap_or_else(|| panic!("the {name} placeholder stands"));
        assert_eq!(p.name, name);
        assert!(
            !p.is_available(),
            "the {name} placeholder still reports the page unavailable"
        );
    }
}

#[test]
fn the_mech_lab_page_accepts_a_loadout_under_a_new_name_and_charges_the_difference() {
    let Some(mut s) = setup(true, true, Some(config_file("lab-accept"))) else {
        return;
    };
    let (mech, cell, added) = swap_candidate(&mut s.lg);
    let base = s.lg.mech(mech).expect("mech").variant;
    let before = figures(&s.lg, base);
    let base_parts = loadout(&s.lg, base);
    let gone = part(
        &s.lg,
        base_parts
            .iter()
            .find(|p| (p.0, p.1) == cell)
            .expect("the part's cell")
            .2,
    );
    let new = part(&s.lg, added);

    go_to_bay(&mut s);
    // the bought mech is picked in the bay, Change Loadout starts the edit: the sequence moves
    // down to the mech lab and the bay's end runs `setMechToModify`
    let entry = {
        let bay = s
            .seq
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        bay.list
            .entries()
            .position(|e| e.mech == mech)
            .expect("the purchased mech is in the bay")
    };
    let at = {
        let bay = s
            .seq
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        let e = bay.list.entries().nth(entry).expect("the entry");
        let r = e.box_rect();
        (
            (r.left + r.right) as f32 / 2.0,
            (r.top + r.bottom) as f32 / 2.0,
        )
    };
    click(&mut s, at);
    click_page_button(&mut s, slot::MECH_BAY, ids::MB_MSG_CHANGE_LOADOUT);
    assert_eq!(s.seq.current_page(), Some(slot::MECH_LAB));
    assert!(
        s.lg.variant_being_edited().is_some(),
        "the bay's end started the edit"
    );

    // the swap: pick the part in the diagram, Remove, the energy form, its row, Add
    let at = {
        let lg = s.lg.clone();
        lab_page(&mut s)
            .lab
            .cell_centre(&lg, cell.0, cell.1)
            .expect("the part's cell")
    };
    click(&mut s, at);
    click_page_button(&mut s, slot::MECH_LAB, button::MB_MSG_REMOVE);
    click_page_button(&mut s, slot::MECH_LAB, form::WEAPON_ENERGY);
    // the energy weapon's own row in the lab's list (the larger areas come first, then names)
    let wanted = s
        .assets
        .string(part(&s.lg, added).string_id as u32)
        .to_string();
    let row_at = {
        let lab = &lab_page(&mut s).lab;
        let row = (0..lab.components.item_count())
            .find(|&i| {
                lab.components
                    .item(i)
                    .and_then(|it| it.text())
                    .is_some_and(|t| t == wanted)
            })
            .expect("the energy weapon's row");
        let o = lab.components.item(row).expect("row").obj();
        (
            o.global_x() as f32 + o.width() / 2.0,
            o.global_y() as f32 + o.height() / 2.0,
        )
    };
    click(&mut s, row_at);
    click_page_button(&mut s, slot::MECH_LAB, button::MB_MSG_ADD);

    // the edit moved the numbers by exactly the parts' differences (`LogisticsVariant`)
    let edited = s.lg.variant_being_edited().expect("the edit is open");
    let mid = figures(&s.lg, edited);
    assert_eq!(mid[0] - before[0], new.cost - gone.cost, "cost");
    assert!(
        (f64::from(mid[1] - before[1]) - f64::from(new.weight - gone.weight)).abs() <= 2.0,
        "weight"
    );
    assert!(
        (f64::from(mid[2] - before[2]) - f64::from(new.heat - gone.heat)).abs() <= 1.0,
        "heat"
    );

    // Accept (`acceptMechModifications`): the save dialog asks for the new loadout's name
    let c_before = s.lg.c_bills();
    click_page_button(&mut s, slot::MECH_LAB, button::MB_MSG_NEXT);
    assert!(lab_page(&mut s).lab.dialog_open(), "the save dialog is up");
    // the name a player types: the dialog's field is focused when it opens (`VariantDialog::begin`
    // sets the focus), so the keys go through the sequence's input (`gos_GetKey` -> `aEdit`)
    type_text(&mut s, LAB_VARIANT_NAME);
    assert_eq!(
        lab_page(&mut s).lab.save.typed_name(),
        LAB_VARIANT_NAME,
        "the typed name is in the field"
    );
    let yes = {
        let b = lab_page(&mut s)
            .lab
            .save
            .screen
            .button(button::YES)
            .expect("the save button");
        (
            b.global_x() as f32 + b.width() / 2.0,
            b.global_y() as f32 + b.height() / 2.0,
        )
    };
    click(&mut s, yes);

    // the lab hands back to the bay; the mech runs the saved loadout under the new name
    assert_eq!(s.seq.current_page(), Some(slot::MECH_BAY));
    let saved = s.lg.mech(mech).expect("mech").variant;
    assert_eq!(
        s.lg.variant(saved).expect("variant").name,
        LAB_VARIANT_NAME,
        "the loadout is saved under the typed name"
    );
    let after = figures(&s.lg, saved);
    assert_eq!(after[0] - before[0], new.cost - gone.cost, "cost");
    assert_eq!(after[3], before[3], "max heat");
    let mut expected: Vec<usize> = base_parts.iter().map(|p| p.2).collect();
    expected.remove(
        base_parts
            .iter()
            .position(|p| (p.0, p.1) == cell)
            .expect("the part"),
    );
    expected.push(added);
    expected.sort_unstable();
    let mut got: Vec<usize> = loadout(&s.lg, saved).iter().map(|p| p.2).collect();
    got.sort_unstable();
    assert_eq!(got, expected, "the saved loadout is the swap");
    // the accept refunded the old loadout and charged the saved one
    assert_eq!(
        s.lg.c_bills(),
        c_before - i64::from(new.cost - gone.cost),
        "the C-Bill difference was charged"
    );
    assert!(s.lg.mech_to_modify().is_none(), "the edit is closed");
}

#[test]
fn the_mech_lab_page_cancel_gives_the_mech_its_loadout_back() {
    let Some(mut s) = setup(true, true, Some(config_file("lab-cancel"))) else {
        return;
    };
    let (mech, cell, added) = swap_candidate(&mut s.lg);
    let base = s.lg.mech(mech).expect("mech").variant;
    let before = figures(&s.lg, base);
    let _ = added;

    go_to_bay(&mut s);
    let at = {
        let bay = s
            .seq
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        let entry = bay
            .list
            .entries()
            .position(|e| e.mech == mech)
            .expect("the purchased mech is in the bay");
        let r = bay.list.entries().nth(entry).expect("the entry").box_rect();
        (
            (r.left + r.right) as f32 / 2.0,
            (r.top + r.bottom) as f32 / 2.0,
        )
    };
    click(&mut s, at);
    click_page_button(&mut s, slot::MECH_BAY, ids::MB_MSG_CHANGE_LOADOUT);
    assert_eq!(s.seq.current_page(), Some(slot::MECH_LAB));

    // the same swap, then Cancel (`cancelMechModfications`): the mech goes back to its loadout
    let at = {
        let lg = s.lg.clone();
        lab_page(&mut s)
            .lab
            .cell_centre(&lg, cell.0, cell.1)
            .expect("the part's cell")
    };
    click(&mut s, at);
    click_page_button(&mut s, slot::MECH_LAB, button::MB_MSG_REMOVE);
    click_page_button(&mut s, slot::MECH_LAB, form::WEAPON_ENERGY);
    let c_before = s.lg.c_bills();
    click_page_button(&mut s, slot::MECH_LAB, button::MB_MSG_PREV);
    assert_eq!(s.seq.current_page(), Some(slot::MECH_BAY));
    assert_eq!(s.lg.mech(mech).expect("mech").variant, base);
    assert_eq!(figures(&s.lg, base), before, "the numbers are restored");
    assert_eq!(s.lg.c_bills(), c_before, "nothing was charged");
    assert!(s.lg.mech_to_modify().is_none(), "the edit is closed");
}

#[test]
fn the_options_page_accept_persists_the_edited_prefs_and_closes_over_the_screen() {
    let persist = config_file("accept");
    let Some(mut s) = setup(true, true, Some(persist.clone())) else {
        return;
    };
    assert_eq!(s.seq.current_page(), Some(slot::SELECT));
    let mut ld = Loader::new(&mut s.vfs, &mut s.assets);
    s.seq
        .open_options(&mut ld, &mut s.lg)
        .expect("the options open over the current screen");
    assert!(s.seq.in_options());

    // the audio tab, the music volume's right step (`optionsarea.cpp`'s sliders), then Yes
    let before = s.prefs.music_volume;
    click_options_button(&mut s, MSB_TAB0 + 1);
    let step = options_page(&mut s)
        .options
        .volume_step_point(1)
        .expect("the music slider's step");
    click(&mut s, step);
    click_options_button(&mut s, YES);

    // the options closed over the screen they were opened on, which resumes unchanged
    assert!(!s.seq.in_options());
    assert_eq!(s.seq.current_page(), Some(slot::SELECT));

    // the accepted prefs carry the raised volume for the host to apply (`prefs.save()`)
    let accepted = options_page(&mut s).take_accepted().expect("the accept");
    assert!(
        accepted.music_volume > before,
        "the music volume was raised: {before} -> {}",
        accepted.music_volume
    );
    assert_eq!(
        read_prefs(&persist),
        accepted,
        "the host's config file holds the accepted prefs"
    );
    // the host's volumes cue carries the edited values while the options were open
    let volumes = options_page(&mut s)
        .take_volumes()
        .expect("the volumes cue");
    assert!(volumes[1] > before, "the cue carries the edit: {volumes:?}");
    assert!(
        options_page(&mut s).take_notices().is_empty(),
        "sound is live, so nothing was refused"
    );
}

#[test]
fn the_options_page_cancel_keeps_the_saved_prefs() {
    let persist = config_file("cancel");
    let Some(mut s) = setup(true, true, Some(persist.clone())) else {
        return;
    };
    // a config file an earlier accept wrote
    std::fs::write(&persist, s.prefs.to_fit()).expect("the config file");
    let bytes = std::fs::read(&persist).expect("the config file");

    let mut ld = Loader::new(&mut s.vfs, &mut s.assets);
    s.seq
        .open_options(&mut ld, &mut s.lg)
        .expect("the options open");
    let before = s.prefs.music_volume;
    click_options_button(&mut s, MSB_TAB0 + 1);
    let step = options_page(&mut s)
        .options
        .volume_step_point(1)
        .expect("the music slider's step");
    click(&mut s, step);
    click_options_button(&mut s, NO);

    assert!(!s.seq.in_options());
    // No (`prefs.load()`): the edits are dropped, the saved prefs stand, nothing is written
    assert!(
        options_page(&mut s).take_accepted().is_none(),
        "no prefs were handed over"
    );
    assert_eq!(
        std::fs::read(&persist).expect("the config file"),
        bytes,
        "the config file is unchanged"
    );
    let volumes = options_page(&mut s)
        .take_volumes()
        .expect("the volumes cue");
    assert_eq!(volumes[1], before, "the cue returns the saved volumes");
    assert!(
        options_page(&mut s).take_notices().is_empty(),
        "a cancel refuses nothing"
    );
}

#[test]
fn the_options_page_reopens_after_a_cancel_and_the_second_visit_accepts() {
    let persist = config_file("reopen");
    let Some(mut s) = setup(true, true, Some(persist.clone())) else {
        return;
    };
    let mut ld = Loader::new(&mut s.vfs, &mut s.assets);
    s.seq
        .open_options(&mut ld, &mut s.lg)
        .expect("the options open");
    // a volume edit, then No: the edits are dropped, nothing is written
    let before = s.prefs.music_volume;
    click_options_button(&mut s, MSB_TAB0 + 1);
    let step = options_page(&mut s)
        .options
        .volume_step_point(1)
        .expect("the music slider's step");
    click(&mut s, step);
    click_options_button(&mut s, NO);
    assert!(!s.seq.in_options());
    assert!(
        options_page(&mut s).take_accepted().is_none(),
        "the cancel hands no prefs over"
    );

    // the wrapper reopens: the page rebuilds the screen the cancel closed (`OptionsXScreen::begin`
    // restarts it, `code/optionsarea.cpp:198`), so the second visit offers the tabs again
    // instead of closing on the first visit's `bDone`
    let mut ld = Loader::new(&mut s.vfs, &mut s.assets);
    s.seq
        .open_options(&mut ld, &mut s.lg)
        .expect("the options reopen");
    assert!(s.seq.in_options());
    frame(&mut s, &[]);
    assert!(
        s.seq.in_options(),
        "the reopened screen runs, it does not close on the first frame"
    );

    // the second visit edits and accepts: the tab takes the click, the slider moves, Yes persists
    click_options_button(&mut s, MSB_TAB0 + 1);
    let step = options_page(&mut s)
        .options
        .volume_step_point(1)
        .expect("the music slider's step");
    click(&mut s, step);
    click_options_button(&mut s, YES);
    assert!(!s.seq.in_options());
    let accepted = options_page(&mut s).take_accepted().expect("the accept");
    assert!(
        accepted.music_volume > before,
        "the volume was raised: {before} -> {}",
        accepted.music_volume
    );
    assert_eq!(
        read_prefs(&persist),
        accepted,
        "the host's config file holds the accepted prefs"
    );
}

#[test]
fn the_options_page_refuses_what_the_windowed_app_cannot_honour() {
    let persist = config_file("refused");
    let Some(mut s) = setup(true, false, Some(persist.clone())) else {
        return;
    };
    let mut ld = Loader::new(&mut s.vfs, &mut s.assets);
    s.seq
        .open_options(&mut ld, &mut s.lg)
        .expect("the options open");
    let before = s.prefs.music_volume;

    // no audio output is installed (volumes_live false): a volume edit would change nothing
    // audible, so the page keeps the old value and tells the player
    click_options_button(&mut s, MSB_TAB0 + 1);
    let step = options_page(&mut s)
        .options
        .volume_step_point(1)
        .expect("the music slider's step");
    click(&mut s, step);
    click_options_button(&mut s, YES);

    let accepted = options_page(&mut s).take_accepted().expect("the accept");
    assert_eq!(accepted.music_volume, before, "the volume edit is not kept");
    let notices = options_page(&mut s).take_notices();
    assert!(
        notices.iter().any(|n| n.contains("Volumes")),
        "the player is told: {notices:?}"
    );
    assert_eq!(
        read_prefs(&persist).music_volume,
        before,
        "the config file holds the kept setting"
    );
}
