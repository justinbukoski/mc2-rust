//! The mech purchase screen driven through its buttons on the shipped data: buying and selling
//! move the C-Bills in `mc2-campaign`, and leaving the screen without accepting gives the
//! starting inventory back. Skips cleanly without `MC2_DATA`.

use std::sync::Arc;

use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::logistics_ids::button;
use mc2_ui::mech_purchase::{MechEntry, MechPurchase};
use mc2_ui::screen::Status;
use mc2_ui::widgets::ListBox;
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
    let mut logistics = Logistics::with_campaign(data, "campaign").expect("campaign");
    logistics.add_c_bills(10_000_000);
    Some(Setup {
        h: Harness::new(),
        vfs,
        logistics,
    })
}

fn frame(s: &mut Setup, screen: &mut MechPurchase, events: &[InputEvent]) {
    s.h.frame(events, |ctx| screen.update(ctx, &mut s.logistics));
}

/// A press and release on a point, one frame each, after the double-click clock has run out.
fn click_at(s: &mut Setup, screen: &mut MechPurchase, (x, y): (f32, f32)) {
    for _ in 0..8 {
        frame(s, screen, &[]);
    }
    frame(s, screen, &[move_to(x, y)]);
    frame(s, screen, &[left(true)]);
    frame(s, screen, &[left(false)]);
    frame(s, screen, &[]);
}

fn press_button(s: &mut Setup, screen: &mut MechPurchase, id: i32) {
    let b = screen
        .screen
        .button(id)
        .unwrap_or_else(|| panic!("button {id}"));
    let at = (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    );
    click_at(s, screen, at);
}

/// Picks row `i` of a list by clicking it.
fn pick_row(s: &mut Setup, screen: &mut MechPurchase, owned: bool, i: usize) {
    let list = if owned {
        &screen.inventory
    } else {
        &screen.variants
    };
    let obj = list.item(i).expect("row").obj();
    let at = (
        obj.global_x() as f32 + obj.width() / 2.0,
        obj.global_y() as f32 + obj.height() / 2.0,
    );
    click_at(s, screen, at);
}

fn open(s: &mut Setup) -> MechPurchase {
    let mut screen =
        MechPurchase::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("purchase");
    s.h.frame(&[], |ctx| screen.begin(ctx, &s.logistics));
    screen
}

fn owned_designs(l: &Logistics) -> Vec<String> {
    l.inventory()
        .iter()
        .filter(|m| m.force_group == 0)
        .map(|m| l.variant(m.variant).expect("variant").name.clone())
        .collect()
}

#[test]
fn buy_then_sell_moves_c_bills_and_leaves_the_inventory_as_it_was() {
    let Some(mut s) = setup() else { return };
    let start = s.logistics.c_bills();
    let rp = s.logistics.resource_points();
    let mut screen = open(&mut s);
    assert!(
        owned_designs(&s.logistics).is_empty(),
        "a new campaign owns no mech"
    );

    // buy: the first mech for sale, Add
    pick_row(&mut s, &mut screen, false, 0);
    press_button(&mut s, &mut screen, button::MB_MSG_ADD);
    let bought = owned_designs(&s.logistics);
    assert_eq!(bought.len(), 1, "one mech bought");
    let cost = s
        .logistics
        .variant_cost(s.logistics.inventory()[0].variant)
        .expect("cost");
    assert_eq!(
        s.logistics.c_bills(),
        start - i64::from(cost),
        "C-Bills paid"
    );
    assert_eq!(
        screen.screen.texts[1].text,
        format!("{} ", s.logistics.c_bills())
    );

    // sell: the owned design, Remove
    pick_row(&mut s, &mut screen, true, 0);
    press_button(&mut s, &mut screen, button::MB_MSG_REMOVE);
    assert!(owned_designs(&s.logistics).is_empty(), "the mech is sold");
    assert_eq!(s.logistics.c_bills(), start, "the full cost is refunded");
    assert_eq!(screen.screen.texts[1].text, format!("{} ", start));

    // the in-mission resource points are not what buying moves
    assert_eq!(s.logistics.resource_points(), rp);
    assert_eq!(screen.status, Status::Running);
}

#[test]
fn leaving_without_accepting_gives_the_starting_inventory_back() {
    let Some(mut s) = setup() else { return };
    // the player owns one mech before the screen opens
    let design = s.logistics.purchasable_mechs()[0];
    s.logistics.purchase_mech(design).expect("purchase");
    let owned_before = owned_designs(&s.logistics);
    let start = s.logistics.c_bills();

    let mut screen = open(&mut s);
    pick_row(&mut s, &mut screen, false, 0);
    press_button(&mut s, &mut screen, button::MB_MSG_ADD);
    assert_eq!(owned_designs(&s.logistics).len(), 2, "a second mech bought");
    assert!(s.logistics.c_bills() < start);

    press_button(&mut s, &mut screen, button::MB_MSG_PREV);
    assert_eq!(screen.status, Status::Down);
    screen.end(&mut s.logistics);
    assert_eq!(
        owned_designs(&s.logistics),
        owned_before,
        "the starting inventory"
    );
    assert_eq!(s.logistics.c_bills(), start, "the C-Bills of the start");
}

#[test]
fn accepting_keeps_the_purchases() {
    let Some(mut s) = setup() else { return };
    let start = s.logistics.c_bills();
    let mut screen = open(&mut s);
    pick_row(&mut s, &mut screen, false, 0);
    press_button(&mut s, &mut screen, button::MB_MSG_ADD);
    let bought = s.logistics.inventory()[0].variant;
    let cost = s.logistics.variant_cost(bought).expect("cost");

    press_button(&mut s, &mut screen, button::MB_MSG_NEXT);
    assert_eq!(screen.status, Status::Down);
    screen.end(&mut s.logistics);
    assert_eq!(owned_designs(&s.logistics).len(), 1, "the purchase stays");
    assert_eq!(s.logistics.c_bills(), start - i64::from(cost));
}

/// The five texts of a mech row: chassis, tonnage, owned count, name and cost.
fn row_texts(list: &ListBox, i: usize) -> Vec<String> {
    list.item(i)
        .and_then(|it| it.as_any().downcast_ref::<MechEntry>())
        .expect("a mech row")
        .texts()
        .iter()
        .map(|t| t.text.clone())
        .collect()
}

#[test]
fn the_rows_show_chassis_tonnage_owned_count_name_and_cost() {
    let Some(mut s) = setup() else { return };
    let mut screen = open(&mut s);
    let v = s.logistics.purchasable_mechs()[0];
    let chassis = s.logistics.variant_chassis(v).expect("chassis");
    let expected = vec![
        s.h.assets
            .string(chassis.chassis_name_id as u32)
            .to_string(),
        format!("{:.0}", chassis.max_weight),
        "0".to_string(),
        s.logistics.variant(v).expect("variant").name.clone(),
        s.logistics.variant_cost(v).expect("cost").to_string(),
    ];
    assert_eq!(row_texts(&screen.variants, 0), expected, "a mech for sale");

    // once bought, the owned list and the sale row count the mech
    pick_row(&mut s, &mut screen, false, 0);
    press_button(&mut s, &mut screen, button::MB_MSG_ADD);
    assert_eq!(row_texts(&screen.inventory, 0)[2], "1");
    assert_eq!(row_texts(&screen.variants, 0)[2], "1");
    assert_eq!(row_texts(&screen.inventory, 0)[4], expected[4]);
}

fn centre_of(obj: &mc2_ui::widgets::Object) -> (f32, f32) {
    (
        obj.global_x() as f32 + obj.width() / 2.0,
        obj.global_y() as f32 + obj.height() / 2.0,
    )
}

/// Presses on `from`, moves to `to` in several steps, and releases there: a drag as a hand makes it.
fn drag(s: &mut Setup, screen: &mut MechPurchase, from: (f32, f32), to: (f32, f32)) {
    for _ in 0..8 {
        frame(s, screen, &[]);
    }
    frame(s, screen, &[move_to(from.0, from.1)]);
    frame(s, screen, &[left(true)]);
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let at = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        frame(s, screen, &[move_to(at.0, at.1)]);
    }
    frame(s, screen, &[left(false)]);
    frame(s, screen, &[]);
}

#[test]
fn dragging_a_mech_for_sale_onto_the_owned_list_buys_it_and_dragging_it_out_sells_it() {
    let Some(mut s) = setup() else { return };
    let start = s.logistics.c_bills();
    let mut screen = open(&mut s);
    let design = s.logistics.purchasable_mechs()[0];
    let cost = i64::from(s.logistics.variant_cost(design).expect("cost"));

    // pick the row, then drag it onto the owned list
    pick_row(&mut s, &mut screen, false, 0);
    let from = centre_of(screen.variants.item(0).expect("row").obj());
    let owned_list = centre_of(&screen.inventory.obj);
    drag(&mut s, &mut screen, from, owned_list);
    assert_eq!(
        owned_designs(&s.logistics).len(),
        1,
        "the drop bought a mech"
    );
    assert_eq!(s.logistics.c_bills(), start - cost, "and paid for it");
    assert!(screen.dragging().is_none(), "the drag ended");
    assert_eq!(screen.inventory.item_count(), 1, "the owned list shows it");

    // dropping a sale row back on the sale list buys nothing
    pick_row(&mut s, &mut screen, false, 0);
    let from = centre_of(screen.variants.item(0).expect("row").obj());
    drag(&mut s, &mut screen, from, (from.0, from.1 + 4.0));
    assert_eq!(owned_designs(&s.logistics).len(), 1, "no second purchase");

    // pick the owned row and drag it out over the sale list: sold
    pick_row(&mut s, &mut screen, true, 0);
    let from = centre_of(screen.inventory.item(0).expect("owned row").obj());
    let sale_list = centre_of(&screen.variants.obj);
    drag(&mut s, &mut screen, from, sale_list);
    assert!(owned_designs(&s.logistics).is_empty(), "the drop sold it");
    assert_eq!(s.logistics.c_bills(), start, "and refunded the cost");
    assert_eq!(screen.inventory.item_count(), 0);
}

#[test]
fn a_mech_the_player_cannot_pay_for_cannot_be_dragged() {
    let Some(mut s) = setup() else { return };
    let mut screen = open(&mut s);
    // spend nearly everything, so the first design is greyed out
    let have = s.logistics.c_bills();
    s.logistics.add_c_bills(-have);
    s.h.frame(&[], |ctx| screen.begin(ctx, &s.logistics));
    frame(&mut s, &mut screen, &[]);
    let row = screen.variants.item(0).expect("row");
    assert_eq!(row.state(), mc2_ui::widgets::ItemState::Disabled);
    let from = centre_of(row.obj());
    let owned_list = centre_of(&screen.inventory.obj);
    drag(&mut s, &mut screen, from, owned_list);
    assert!(owned_designs(&s.logistics).is_empty());
    assert!(screen.dragging().is_none());
}

#[test]
fn double_clicking_a_mech_for_sale_buys_it() {
    let Some(mut s) = setup() else { return };
    let start = s.logistics.c_bills();
    let mut screen = open(&mut s);
    let design = s.logistics.purchasable_mechs()[0];
    let cost = i64::from(s.logistics.variant_cost(design).expect("cost"));
    let at = centre_of(screen.variants.item(0).expect("row").obj());
    for _ in 0..8 {
        frame(&mut s, &mut screen, &[]);
    }
    frame(&mut s, &mut screen, &[move_to(at.0, at.1)]);
    for _ in 0..2 {
        frame(&mut s, &mut screen, &[left(true)]);
        frame(&mut s, &mut screen, &[left(false)]);
    }
    frame(&mut s, &mut screen, &[]);
    assert_eq!(
        owned_designs(&s.logistics).len(),
        1,
        "the double click bought"
    );
    assert_eq!(s.logistics.c_bills(), start - cost);
}

#[test]
fn the_picked_mech_is_shown_in_the_display_with_its_stats_loadout_and_picture() {
    let Some(mut s) = setup() else { return };
    let mut screen = open(&mut s);
    let design = s.logistics.purchasable_mechs()[0];
    pick_row(&mut s, &mut screen, false, 0);
    assert_eq!(screen.display.cur_variant, Some(design));
    let chassis = s.logistics.variant_chassis(design).expect("chassis");
    assert_eq!(
        screen.display.base.texts[0].text,
        s.h.assets.string(chassis.chassis_name_id as u32),
        "the chassis name"
    );
    assert_eq!(
        screen.display.base.texts[1].text,
        s.logistics.variant(design).expect("variant").name,
        "the design's name"
    );
    let request = screen
        .display
        .preview
        .as_ref()
        .expect("a 3D picture request");
    assert_eq!(
        request.shape,
        mc2_ui::preview::shape_name(&chassis.file_name)
    );
    assert!(
        screen.display.loadout.list.item_count() > 0,
        "the loadout rows"
    );

    // buy it, then pick the owned row: the same design stays shown
    press_button(&mut s, &mut screen, button::MB_MSG_ADD);
    pick_row(&mut s, &mut screen, true, 0);
    assert_eq!(screen.display.cur_variant, Some(design));
}

/// The colours the five texts of a row took in the last render.
fn row_colours(list: &ListBox, i: usize) -> Vec<u32> {
    list.item(i)
        .and_then(|it| it.as_any().downcast_ref::<MechEntry>())
        .expect("a mech row")
        .texts()
        .iter()
        .map(|t| t.obj.color())
        .collect()
}

/// `MechListBoxItem::render` takes each part's colour from the animation of the row's state:
/// the texts follow selection and the disabled state, and a disabled row's icon is drawn in the
/// animation colour (the `0xa0000000` dim is only the drag's).
#[test]
fn purchase_rows_follow_their_state_animations() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("GameData::load"));
    let logistics = Logistics::with_campaign(data, "campaign").expect("campaign");
    let mut s = Setup {
        h: Harness::new(),
        vfs,
        logistics,
    };
    let mut screen = open(&mut s);
    let rows = screen.variants.item_count();
    let disabled = (0..rows).find(|&i| {
        screen.variants.item(i).map(|it| it.state()) == Some(mc2_ui::widgets::ItemState::Disabled)
    });
    let enabled = (0..rows).find(|&i| {
        screen.variants.item(i).map(|it| it.state()) == Some(mc2_ui::widgets::ItemState::Enabled)
    });
    let (Some(disabled), Some(enabled)) = (disabled, enabled) else {
        return;
    };
    for _ in 0..30 {
        frame(&mut s, &mut screen, &[]);
    }
    let dl = s.h.render(|ctx| screen.render(ctx));
    assert!(
        dl.vertices.iter().all(|v| v.argb != 0xa000_0000),
        "no drag-dimmed icon"
    );
    let off = row_colours(&screen.variants, disabled);
    let on = row_colours(&screen.variants, enabled);
    assert_ne!(off, on, "a disabled row is not drawn like an enabled one");

    pick_row(&mut s, &mut screen, false, enabled);
    for _ in 0..30 {
        frame(&mut s, &mut screen, &[]);
    }
    let _ = s.h.render(|ctx| screen.render(ctx));
    let picked = row_colours(&screen.variants, enabled);
    assert_ne!(
        picked, on,
        "the selected row's texts take the selected colours"
    );
}
