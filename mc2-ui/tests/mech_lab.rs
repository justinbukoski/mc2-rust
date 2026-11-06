// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The mech lab driven through its buttons on the shipped data: a loadout change moves cost,
//! weight and heat in `mc2-campaign`, Cancel gives the mech back its loadout, and Accept runs
//! the cost check. Skips cleanly without `MC2_DATA`.
//!
//! The purchasable mechs of a new campaign are packed, so each test first removes a part and
//! adds an energy weapon in its place ([`swap_candidate`]).

use std::sync::Arc;

use mc2_campaign::{GameData, Logistics, MechKey, VariantKey};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_sim_data::component::logistics::WeaponRangeType;
use mc2_sim_data::component::ComponentForm;
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::logistics_ids::{button, form, ids};
use mc2_ui::mech_lab::MechLab;
use mc2_ui::screen::Status;
use mc2_ui::widgets::{AnimButton, ItemState};
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

/// One frame of the lab with the given input.
fn frame(s: &mut Setup, lab: &mut MechLab, events: &[InputEvent]) {
    s.h.frame(events, |ctx| lab.update(ctx, &mut s.logistics));
}

/// The centre of a button on screen.
fn centre(b: &AnimButton) -> (f32, f32) {
    (
        b.global_x() as f32 + b.width() / 2.0,
        b.global_y() as f32 + b.height() / 2.0,
    )
}

/// A press and release on a point of the screen, one frame each. The clock runs out first, so
/// the press is a click and not the second half of a double click.
fn click_at(s: &mut Setup, lab: &mut MechLab, (x, y): (f32, f32)) {
    for _ in 0..8 {
        frame(s, lab, &[]);
    }
    frame(s, lab, &[move_to(x, y)]);
    frame(s, lab, &[left(true)]);
    frame(s, lab, &[left(false)]);
    frame(s, lab, &[]);
}

/// Lets an opened dialog slide in; it takes no input until it has (`LogisticsDialog::update`).
fn settle(s: &mut Setup, lab: &mut MechLab) {
    for _ in 0..60 {
        frame(s, lab, &[]);
    }
}

/// Presses a button of the lab's layout.
fn press(s: &mut Setup, lab: &mut MechLab, id: i32) {
    let at = centre(
        lab.screen
            .button(id)
            .unwrap_or_else(|| panic!("button {id}")),
    );
    click_at(s, lab, at);
}

/// Cost, weight, heat and max heat of a variant, through the campaign.
fn figures(l: &Logistics, key: VariantKey) -> [i32; 4] {
    let v = l.variant(key).expect("variant");
    let ctx = l.ctx_for(v);
    [v.cost(&ctx), v.weight(&ctx), v.heat(&ctx), v.max_heat(&ctx)]
}

/// The first energy weapon the lab's list offers and can take (the list's first enabled row).
fn first_addable_energy(l: &Logistics) -> Option<usize> {
    l.available_components().into_iter().find(|&i| {
        let is_energy = l
            .data()
            .catalog
            .get(i)
            .is_some_and(|c| c.form == ComponentForm::WeaponEnergy);
        let (mut x, mut y) = (-1, -1);
        is_energy && l.lab_can_add_component(i, &mut x, &mut y).is_ok()
    })
}

/// A purchased mech, the placed part to remove from it, and the energy weapon that then goes in:
/// removing the part makes room, and the energy weapon costs more than the part.
fn swap_candidate(s: &mut Setup) -> (MechKey, (i64, i64), usize) {
    for variant in s.logistics.purchasable_mechs() {
        let Ok(mech) = s.logistics.purchase_mech(variant) else {
            continue;
        };
        s.logistics
            .set_mech_to_modify(mech)
            .expect("set_mech_to_modify");
        let key = s.logistics.variant_being_edited().expect("editing");
        let placed: Vec<(i64, i64, usize)> = s
            .logistics
            .variant(key)
            .expect("variant")
            .components
            .iter()
            .map(|p| (p.x, p.y, p.component))
            .collect();
        let mut found = None;
        for (x, y, removed) in placed {
            let mut trial = s.logistics.clone();
            if trial.lab_remove_component(x, y).is_err() {
                continue;
            }
            let Some(added) = first_addable_energy(&trial) else {
                continue;
            };
            let cost = |i: usize| s.logistics.data().catalog.get(i).map_or(0, |c| c.cost);
            if cost(added) > cost(removed) {
                found = Some(((x, y), added));
                break;
            }
        }
        s.logistics.cancel_mech_modifications().expect("cancel");
        if let Some((cell, added)) = found {
            return (mech, cell, added);
        }
    }
    panic!("no purchasable mech has a part that an energy weapon can replace at a profit");
}

fn open_lab(s: &mut Setup, mech: MechKey) -> MechLab {
    s.logistics
        .set_mech_to_modify(mech)
        .expect("set_mech_to_modify");
    let mut lab = MechLab::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("mech lab");
    s.h.frame(&[], |ctx| lab.begin(ctx, &s.logistics));
    lab
}

/// Removes the part at `cell` through the diagram and the Remove button, then adds the energy
/// weapon through the form button and the Add button.
fn swap_part(s: &mut Setup, lab: &mut MechLab, cell: (i64, i64)) {
    let at = lab
        .cell_centre(&s.logistics, cell.0, cell.1)
        .expect("a cell");
    click_at(s, lab, at);
    press(s, lab, button::MB_MSG_REMOVE);
    press(s, lab, form::WEAPON_ENERGY);
    press(s, lab, button::MB_MSG_ADD);
}

#[test]
fn swapped_part_moves_cost_weight_and_heat_and_cancel_restores_them() {
    let Some(mut s) = setup() else { return };
    let (mech, cell, added) = swap_candidate(&mut s);
    let base = s.logistics.mech(mech).expect("mech").variant;
    let before = figures(&s.logistics, base);
    let removed = s
        .logistics
        .variant(base)
        .expect("variant")
        .components
        .iter()
        .find(|p| (p.x, p.y) == cell)
        .map(|p| p.component)
        .expect("the part at the cell");
    let mut lab = open_lab(&mut s, mech);

    swap_part(&mut s, &mut lab, cell);

    let edited = s.logistics.variant_being_edited().expect("editing");
    let part = |i: usize| {
        s.logistics
            .data()
            .catalog
            .get(i)
            .expect("catalogue")
            .clone()
    };
    let (gone, new) = (part(removed), part(added));
    let placed = *s
        .logistics
        .variant(edited)
        .expect("variant")
        .components
        .last()
        .expect("a part");
    assert_eq!(
        placed.component, added,
        "the energy weapon is the last part placed"
    );
    let after = figures(&s.logistics, edited);
    // cost and heat are the sums of the parts; weight is truncated per step (`logisticsvariant`)
    assert_eq!(after[0] - before[0], new.cost - gone.cost, "cost");
    assert!(
        ((after[1] - before[1]) as f32 - (new.weight - gone.weight)).abs() <= 2.0,
        "weight"
    );
    assert!(
        ((after[2] - before[2]) as f32 - (new.heat - gone.heat)).abs() <= 1.0,
        "heat"
    );
    assert_eq!(after[3], before[3], "max heat");

    // the panel shows what the campaign computed once the heat has counted up (1.6 s)
    for _ in 0..60 {
        frame(&mut s, &mut lab, &[]);
    }
    let texts = &lab.screen.texts;
    assert_eq!(
        texts[5].text,
        format!("{} ", after[0] - before[0]),
        "cost change"
    );
    assert_eq!(texts[12].text, format!("{}/{}", after[2], after[3]), "heat");
    assert_eq!(
        texts[1].text,
        format!("{} ", s.logistics.c_bills()),
        "C-Bills"
    );

    // Cancel: the mech is back on its own loadout and the lab hands over
    press(&mut s, &mut lab, button::MB_MSG_PREV);
    assert_eq!(lab.status, Status::Up);
    assert_eq!(s.logistics.mech(mech).expect("mech").variant, base);
    assert_eq!(figures(&s.logistics, base), before);
    assert!(s.logistics.mech_to_modify().is_none());
}

#[test]
fn accept_without_a_change_leaves_the_lab_on_the_same_loadout() {
    let Some(mut s) = setup() else { return };
    let (mech, _, _) = swap_candidate(&mut s);
    let base = s.logistics.mech(mech).expect("mech").variant;
    let mut lab = open_lab(&mut s, mech);

    press(&mut s, &mut lab, button::MB_MSG_NEXT);
    assert_eq!(lab.status, Status::Up);
    assert!(!lab.dialog_open());
    assert_eq!(s.logistics.mech(mech).expect("mech").variant, base);
}

#[test]
fn accept_refuses_a_loadout_that_costs_more_than_the_c_bills() {
    let Some(mut s) = setup() else { return };
    let (mech, cell, added) = swap_candidate(&mut s);
    // read before the lab takes a copy of the mech's loadout
    let removed = {
        let base = s.logistics.mech(mech).expect("mech").variant;
        let placed = s
            .logistics
            .variant(base)
            .expect("variant")
            .components
            .iter()
            .find(|p| (p.x, p.y) == cell)
            .map(|p| p.component)
            .expect("part");
        s.logistics
            .data()
            .catalog
            .get(placed)
            .expect("catalogue")
            .cost
    };
    let mut lab = open_lab(&mut s, mech);
    swap_part(&mut s, &mut lab, cell);

    // the change costs more than the player has: one C-Bill short of it
    let added_cost = s
        .logistics
        .data()
        .catalog
        .get(added)
        .expect("catalogue")
        .cost;
    let change = i64::from(added_cost - removed);
    assert!(change >= 1, "the swap costs more");
    let c_bills = s.logistics.c_bills();
    s.logistics.decrement_c_bills(c_bills - (change - 1));
    assert_eq!(s.logistics.c_bills(), change - 1);

    press(&mut s, &mut lab, button::MB_MSG_NEXT);
    assert!(lab.dialog_open(), "the error message is up");
    settle(&mut s, &mut lab);
    assert_eq!(lab.status, Status::Running);
    assert_eq!(
        lab.error.screen.texts[0].text,
        s.h.assets.string(ids::MC_INSUFFICIENT_CBILLS as u32)
    );

    // No closes the message and the lab stays in use
    let no = centre(lab.error.screen.button(button::NO).expect("the button"));
    click_at(&mut s, &mut lab, no);
    assert!(!lab.dialog_open());
    assert_eq!(lab.status, Status::Running);
}

/// The energy weapons of the lab's list, in its order: the rows `set_form(WEAPON_ENERGY)` builds.
fn energy_rows(l: &Logistics) -> Vec<usize> {
    l.available_components()
        .into_iter()
        .filter(|&i| {
            l.data()
                .catalog
                .get(i)
                .is_some_and(|c| c.form == ComponentForm::WeaponEnergy)
        })
        .collect()
}

/// Every row is greyed out exactly when the campaign says the lab cannot take its component now.
fn check_rows(s: &Setup, lab: &MechLab) {
    for (row, index) in energy_rows(&s.logistics).into_iter().enumerate() {
        let (mut x, mut y) = (-1, -1);
        let can = s
            .logistics
            .lab_can_add_component(index, &mut x, &mut y)
            .is_ok();
        let state = lab.components.item(row).expect("a row").state();
        assert_eq!(
            state == ItemState::Disabled,
            !can,
            "row {row} (component {index}) follows what the lab can take"
        );
    }
}

#[test]
fn removing_a_part_re_enables_the_rows_it_makes_room_for() {
    let Some(mut s) = setup() else { return };
    let (mech, cell, added) = swap_candidate(&mut s);
    let mut lab = open_lab(&mut s, mech);
    press(&mut s, &mut lab, form::WEAPON_ENERGY);
    check_rows(&s, &lab);

    // the removal makes room for the energy weapon; its row must be selectable again
    let at = lab
        .cell_centre(&s.logistics, cell.0, cell.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    press(&mut s, &mut lab, button::MB_MSG_REMOVE);
    frame(&mut s, &mut lab, &[]);
    check_rows(&s, &lab);
    let row = energy_rows(&s.logistics)
        .iter()
        .position(|&i| i == added)
        .expect("the weapon is in the energy list");
    assert_ne!(
        lab.components.item(row).expect("a row").state(),
        ItemState::Disabled,
        "the room the removal left takes the weapon"
    );
}

#[test]
fn opens_on_the_ballistic_form_with_the_first_placed_part_picked() {
    let Some(mut s) = setup() else { return };
    let (mech, _, _) = swap_candidate(&mut s);
    let mut lab = open_lab(&mut s, mech);
    // the enables are set by the first frame's update, as in the app
    frame(&mut s, &mut lab, &[]);

    // `MechLabScreen::begin`: the ballistic form, and the diagram's first part, not the list's
    let form_button = |id| &lab.screen.button(id).expect("form button").button;
    assert!(form_button(form::WEAPON_BALLISTIC).is_pressed());
    assert!(!form_button(form::WEAPON_ENERGY).is_pressed());
    assert_eq!(lab.components.selected_item(), -1, "no list row is picked");
    // a diagram pick closes Add and opens Remove, when the campaign lets the part go
    let remove = &lab
        .screen
        .button(button::MB_MSG_REMOVE)
        .expect("remove")
        .button;
    let add = &lab.screen.button(button::MB_MSG_ADD).expect("add").button;
    assert!(!add.is_enabled(), "Add is closed while a part is picked");
    assert!(remove.is_enabled(), "Remove is open for the picked part");
}

#[test]
fn the_component_list_puts_the_larger_grid_areas_first_then_by_name() {
    let Some(mut s) = setup() else { return };
    let (mech, _, _) = swap_candidate(&mut s);
    let mut lab = open_lab(&mut s, mech);
    press(&mut s, &mut lab, form::WEAPON_ENERGY);

    // `ComponentIconListBox::addSortedItem`: the grid area, largest first, then the name
    // (case-insensitive) in ascending order
    let mut expected: Vec<(i32, String)> = Vec::new();
    for i in s.logistics.available_components() {
        let Some(c) = s.logistics.data().catalog.get(i) else {
            continue;
        };
        if c.form == ComponentForm::WeaponEnergy {
            let name = s.h.assets.string(c.string_id as u32).to_string();
            expected.push((c.width * c.height, name));
        }
    }
    assert!(expected.len() > 1, "the energy weapons are listed");
    let key = |e: &(i32, String)| (-e.0, e.1.to_ascii_lowercase());
    expected.sort_by_key(key);

    let shown: Vec<String> = (0..lab.components.item_count())
        .filter_map(|i| lab.components.item(i)?.text().map(str::to_string))
        .collect();
    let names: Vec<&str> = expected.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(shown, names, "the rows follow the C++ order");
}

/// A variant's parts as `(x, y, catalogue index)`, in the order the campaign holds them.
fn loadout(l: &Logistics, key: VariantKey) -> Vec<(i64, i64, usize)> {
    l.variant(key)
        .expect("variant")
        .components
        .iter()
        .map(|p| (p.x, p.y, p.component))
        .collect()
}

/// A second loadout of the mech's chassis, saved as a player variant: the mech is opened, one
/// part is swapped and the change is accepted under `name`.
fn save_second_loadout(s: &mut Setup, mech: MechKey, cell: (i64, i64), name: &str) {
    let mut lab = open_lab(s, mech);
    swap_part(s, &mut lab, cell);
    s.logistics
        .accept_mech_modifications(name)
        .expect("accept the second loadout");
}

#[test]
fn the_variant_selector_is_drawn_and_a_pick_swaps_the_loadout() {
    let Some(mut s) = setup() else { return };
    let (mech, cell, _) = swap_candidate(&mut s);
    let base = s.logistics.mech(mech).expect("mech").variant;
    let base_name = s.logistics.variant(base).expect("variant").name.clone();
    let base_loadout = loadout(&s.logistics, base);
    save_second_loadout(&mut s, mech, cell, "Second Loadout");

    // the mech now runs the saved loadout; the drop-down lists both of the chassis' loadouts
    let mut lab = open_lab(&mut s, mech);
    let edited = s.logistics.variant_being_edited().expect("editing");
    assert_eq!(lab.variant_combo.list.item_count(), 2, "two loadouts");
    assert_eq!(lab.variant_combo.entry.entry(), "Second Loadout");
    frame(&mut s, &mut lab, &[]);

    // the selector is drawn: its quads come after the layout's, and they lie over its box
    let with = s.h.render(|ctx| lab.render(ctx, &s.logistics));
    lab.variant_combo.obj.show = false;
    let without = s.h.render(|ctx| lab.render(ctx, &s.logistics));
    lab.variant_combo.obj.show = true;
    let n = without.vertices.len();
    assert_eq!(
        with.vertices[..n],
        without.vertices[..],
        "the layout is drawn as before"
    );
    let entry = &lab.variant_combo.entry.obj;
    let (left, top) = (entry.global_x() as f32, entry.global_y() as f32);
    let (right, bottom) = (left + entry.width(), top + entry.height());
    let selector = &with.vertices[n..];
    assert!(!selector.is_empty(), "the selector draws");
    assert!(
        selector
            .iter()
            .any(|v| v.x >= left && v.x <= right && v.y >= top && v.y <= bottom),
        "the selector is drawn over its entry box"
    );

    // a click on the expand button opens the list; a pick of the other loadout swaps it in
    let expand = centre(&lab.variant_combo.expand);
    click_at(&mut s, &mut lab, expand);
    assert!(lab.variant_combo.list.is_showing(), "the drop-down opens");
    let row = (0..lab.variant_combo.list.item_count())
        .find(|&i| {
            lab.variant_combo
                .list
                .item(i)
                .and_then(|it| it.text())
                .is_some_and(|t| t == base_name)
        })
        .expect("the base loadout is listed");
    let o = lab.variant_combo.list.item(row).expect("row").obj();
    let at = (o.left() + o.width() / 2.0, o.top() + o.height() / 2.0);
    click_at(&mut s, &mut lab, at);
    assert_eq!(
        lab.variant_combo.entry.entry(),
        base_name,
        "the entry shows the pick"
    );
    assert_eq!(
        loadout(&s.logistics, edited),
        base_loadout,
        "the edit takes the picked loadout"
    );
    assert_eq!(
        s.logistics.variant(edited).expect("variant").name,
        "Second Loadout",
        "and keeps its own name"
    );
}

/// `updateDiagramInput`: a piece dropped from the list onto the grid's last column is centred on
/// the cell under the pointer and moved back so that it fits (`code/mechlabscreen.cpp`).
#[test]
fn a_piece_dropped_on_the_grid_edge_is_kept_inside_it() {
    let Some(mut s) = setup() else { return };
    let (mech, _, _) = swap_candidate(&mut s);
    let mut lab = open_lab(&mut s, mech);
    let edited = s.logistics.variant_being_edited().expect("editing");
    // an empty grid: the parts come off first, the campaign lets each one go
    for (x, y, _) in loadout(&s.logistics, edited) {
        s.logistics
            .lab_remove_component(x, y)
            .expect("remove the part");
    }
    // the first weapon, in the lab's order of its form's list (larger areas first, then by name),
    // that is two cells wide and two high
    let mut found = None;
    for (form_id, comp_form) in [
        (form::WEAPON_BALLISTIC, ComponentForm::WeaponBallistic),
        (form::WEAPON_MISSILE, ComponentForm::WeaponMissile),
        (form::WEAPON_ENERGY, ComponentForm::WeaponEnergy),
    ] {
        let mut rows: Vec<(usize, i32, i32, String)> = s
            .logistics
            .available_components()
            .into_iter()
            .filter_map(|i| {
                let c = s.logistics.data().catalog.get(i)?;
                let name = s.h.assets.string(c.string_id as u32).to_string();
                (c.form == comp_form).then_some((i, c.width, c.height, name))
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.1 * b.2)
                .cmp(&(a.1 * a.2))
                .then_with(|| a.3.to_ascii_lowercase().cmp(&b.3.to_ascii_lowercase()))
        });
        if let Some(row) = rows.iter().position(|r| r.1 >= 2 && r.2 >= 2) {
            found = Some((form_id, row, rows[row].0, rows[row].1, rows[row].2));
            break;
        }
    }
    let (form_id, row, index, w, h) = found.expect("a two by two weapon");
    press(&mut s, &mut lab, form_id);
    lab.components.select_item(row as i32);
    let from = {
        let o = lab.components.item(row).expect("row").obj();
        (o.left() + 10.0, o.top() + o.height() / 2.0)
    };

    // the bottom-right cell, the cursor at its centre
    let ctx = s
        .logistics
        .ctx_for(s.logistics.variant(edited).expect("variant"));
    let (area_w, area_h) = (
        ctx.chassis.component_area_width,
        ctx.chassis.component_area_height,
    );
    let to = lab
        .cell_centre(&s.logistics, area_w - 1, area_h - 1)
        .expect("a cell");

    // a drag: press on the row, move past the drag threshold, release over the grid
    frame(&mut s, &mut lab, &[move_to(from.0, from.1)]);
    frame(&mut s, &mut lab, &[left(true)]);
    frame(&mut s, &mut lab, &[move_to(from.0 + 40.0, from.1)]);
    frame(&mut s, &mut lab, &[move_to(to.0, to.1)]);
    frame(&mut s, &mut lab, &[move_to(to.0, to.1), left(false)]);

    // C++: the origin is the cell less half the piece, then moved back to fit the grid's edge
    let expected = (area_w - i64::from(w), area_h - i64::from(h));
    let placed = *s
        .logistics
        .variant(edited)
        .expect("variant")
        .components
        .last()
        .expect("the piece was placed");
    assert_eq!(placed.component, index, "the dragged piece");
    assert_eq!(
        (placed.x, placed.y),
        expected,
        "the piece sits at the grid's edge"
    );
}

#[test]
fn the_overwrite_prompt_ignores_case_so_nothing_is_replaced_before_yes_is_confirmed() {
    let Some(mut s) = setup() else { return };
    let (mech, cell, _) = swap_candidate(&mut s);
    save_second_loadout(&mut s, mech, cell, "Second Loadout");
    let saved = s.logistics.mech(mech).expect("mech").variant;
    let saved_loadout = loadout(&s.logistics, saved);

    // the saved loadout with one part taken out: Accept now asks for a name
    let mut lab = open_lab(&mut s, mech);
    let at = lab
        .cell_centre(&s.logistics, cell.0, cell.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    press(&mut s, &mut lab, button::MB_MSG_REMOVE);
    press(&mut s, &mut lab, button::MB_MSG_NEXT);
    assert!(lab.save.is_open(), "the save dialog asks for a name");
    settle(&mut s, &mut lab);

    // the player types the saved name in other capitals, then Yes
    s.h.frame(&[], |ctx| {
        lab.save.screen.edits[0].set_entry(ctx.assets, "second loadout", false)
    });
    let yes = centre(lab.save.screen.button(button::YES).expect("Yes"));
    click_at(&mut s, &mut lab, yes);

    // the overwrite prompt opens and nothing is replaced yet
    assert!(lab.save.is_open(), "the overwrite prompt is up");
    assert_eq!(lab.save.status, Status::Running, "no answer has been given");
    assert_eq!(lab.status, Status::Running, "the lab is still in use");
    assert_eq!(
        s.logistics.variant(saved).expect("the saved variant").name,
        "Second Loadout"
    );
    assert_eq!(
        loadout(&s.logistics, saved),
        saved_loadout,
        "the saved loadout is intact"
    );
}

/// The catalogue form of a component.
fn form_of(l: &Logistics, index: usize) -> ComponentForm {
    l.data().catalog.get(index).expect("a component").form
}

/// Whether a form is a weapon, the rows of which `showJumpJetItems` turns off for the others.
fn is_weapon(form: ComponentForm) -> bool {
    !matches!(
        form,
        ComponentForm::JumpJet | ComponentForm::HeatSink | ComponentForm::Bulk
    )
}

/// What `MechLabScreen::setComponent` leaves on the panel for a component: its name, the text
/// slots its form fills, and the rows `showJumpJetItems` and the form's branch show or hide.
/// The attribute meters are not ported, so only the text and the statics are checked.
fn assert_panel(s: &Setup, lab: &MechLab, index: usize) {
    let c = s.logistics.data().catalog.get(index).expect("a component");
    let key = s.logistics.variant_being_edited().expect("editing");
    let v = s.logistics.variant(key).expect("variant");
    let name = s.h.assets.string(c.string_id as u32).to_string();
    let text = |slot: usize| lab.screen.texts[slot].text.as_str();
    let slot_shown = |slot: usize| lab.screen.texts[slot].obj.is_showing();
    let row_shown = |i: usize| lab.screen.statics[i].is_showing();
    assert_eq!(text(6), name, "the panel names the component");

    // the jump range and armour rows, then the heat sink's own rows (`mechlabscreen.cpp`)
    let (s47, s48, s49, s55, t7, t16) = match c.form {
        ComponentForm::HeatSink => (false, false, false, true, false, true),
        ComponentForm::Bulk => (true, false, true, false, true, false),
        ComponentForm::JumpJet => (true, true, false, false, true, false),
        _ => (false, false, false, false, true, false),
    };
    let weapon = is_weapon(c.form);
    assert_eq!(row_shown(47), s47, "the jump range label");
    assert_eq!(row_shown(48), s48, "the jump range row");
    assert_eq!(row_shown(49), s49, "the armour row");
    assert_eq!(row_shown(55), s55, "the heat sink row");
    assert_eq!(slot_shown(7), t7, "the jump range or damage slot");
    assert_eq!(slot_shown(16), t16, "the heat sink slot");
    // the weapon rows, with the heat row always shown
    for i in [30, 31, 32] {
        assert_eq!(row_shown(i), weapon, "weapon row {i}");
    }
    // the ammo label follows `useUnlimitedAmmo` (`showJumpJetItems`)
    assert_eq!(
        row_shown(34),
        weapon && !lab.unlimited_ammo(),
        "the ammo row"
    );
    assert!(row_shown(33), "the heat row is always shown");

    match c.form {
        ComponentForm::JumpJet => {
            assert_eq!(text(7), format!("{}", v.max_jump_range() * 25));
            assert_eq!(text(15), format!("{:.1}", c.heat));
        }
        ComponentForm::HeatSink => {
            assert_eq!(text(7), "", "the jump range text is cleared");
            assert_eq!(text(15), "", "the jump heat text is cleared");
            assert_eq!(text(16), format!("+{:.1}", c.damage));
        }
        ComponentForm::Bulk => {
            assert_eq!(text(7), format!("{:.1}", c.damage));
            assert_eq!(text(15), "");
        }
        _ => {
            // `IDS_SHORT_RANGE` + the range type: short and none are the first name
            let range = match c.range_type {
                WeaponRangeType::Short | WeaponRangeType::None => 0,
                WeaponRangeType::Medium => 1,
                WeaponRangeType::Long => 2,
            };
            let name =
                s.h.assets
                    .string(ids::SHORT_RANGE as u32 + range)
                    .to_string();
            assert_eq!(text(7), name, "the weapon's range");
        }
    }
    if weapon {
        assert_eq!(text(8), format!("{:.1}", c.damage));
        assert_eq!(text(10), format!("{:.1}", c.heat));
        assert_eq!(text(15), "");
    } else {
        for slot in [8, 9, 10, 13] {
            assert_eq!(text(slot), "", "slot {slot} is cleared for a non-weapon");
        }
    }
}

#[test]
fn the_panel_follows_each_diagram_pick_and_the_list_pick() {
    let Some(mut s) = setup() else { return };
    let (mech, cell, _) = swap_candidate(&mut s);
    let mut lab = open_lab(&mut s, mech);
    frame(&mut s, &mut lab, &[]);

    // two placed parts of different kinds: each diagram click names the part under it
    let key = s.logistics.variant_being_edited().expect("editing");
    let placed = loadout(&s.logistics, key);
    let first = placed[0];
    let second = *placed
        .iter()
        .find(|p| p.2 != first.2)
        .expect("two kinds of part are placed");
    for (x, y, index) in [first, second] {
        let at = lab.cell_centre(&s.logistics, x, y).expect("a cell");
        click_at(&mut s, &mut lab, at);
        frame(&mut s, &mut lab, &[]);
        assert_panel(&s, &lab, index);
    }

    // a tab with no row to pick leaves no component: the panel's old text goes
    press(&mut s, &mut lab, form::WEAPON_BALLISTIC);
    frame(&mut s, &mut lab, &[]);
    assert_eq!(
        lab.components.selected_item(),
        -1,
        "the ballistic tab is empty here"
    );
    assert_eq!(lab.screen.texts[6].text, "", "no component, no name");
    assert_eq!(lab.screen.texts[8].text, "", "no component, no damage");

    // the removal leaves the panel with no pick; the energy tab's first row is then shown
    let at = lab
        .cell_centre(&s.logistics, cell.0, cell.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    press(&mut s, &mut lab, button::MB_MSG_REMOVE);
    press(&mut s, &mut lab, form::WEAPON_ENERGY);
    frame(&mut s, &mut lab, &[]);
    let row = lab.components.selected_item();
    assert!(row >= 0, "the energy tab has a row to pick");
    let row_name = lab.components.item(row as usize).and_then(|it| it.text());
    assert_eq!(Some(lab.screen.texts[6].text.as_str()), row_name);
}

/// Opens a purchasable mech that carries a jump jet, a bulk part and a weapon, after one weapon
/// is taken out and a heat sink is put in its place through the campaign. The diagram then
/// holds a part of each form the panel shows. Returns the open lab.
fn open_mech_with_every_panel(s: &mut Setup) -> MechLab {
    for variant in s.logistics.purchasable_mechs() {
        let Ok(mech) = s.logistics.purchase_mech(variant) else {
            continue;
        };
        s.logistics
            .set_mech_to_modify(mech)
            .expect("set_mech_to_modify");
        let key = s.logistics.variant_being_edited().expect("editing");
        let placed = loadout(&s.logistics, key);
        let has = |l: &Logistics, f: ComponentForm| placed.iter().any(|p| form_of(l, p.2) == f);
        let weapon = placed
            .iter()
            .find(|p| is_weapon(form_of(&s.logistics, p.2)))
            .copied();
        let heat = s
            .logistics
            .available_components()
            .into_iter()
            .find(|&i| form_of(&s.logistics, i) == ComponentForm::HeatSink);
        let fits = match (weapon, heat) {
            (Some((x, y, _)), Some(heat))
                if has(&s.logistics, ComponentForm::JumpJet)
                    && has(&s.logistics, ComponentForm::Bulk) =>
            {
                let (mut hx, mut hy) = (-1, -1);
                s.logistics.lab_remove_component(x, y).is_ok()
                    && s.logistics
                        .lab_add_component(heat, &mut hx, &mut hy)
                        .is_ok()
            }
            _ => false,
        };
        if !fits {
            s.logistics.cancel_mech_modifications().expect("cancel");
            continue;
        }
        let mut lab =
            MechLab::load(&mut Loader::new(&mut s.vfs, &mut s.h.assets)).expect("mech lab");
        s.h.frame(&[], |ctx| lab.begin(ctx, &s.logistics));
        return lab;
    }
    panic!(
        "no purchasable mech takes a heat sink in place of a weapon next to a jump jet and bulk"
    );
}

#[test]
fn the_panel_switches_between_weapon_jump_jet_heat_sink_and_bulk() {
    let Some(mut s) = setup() else { return };
    let mut lab = open_mech_with_every_panel(&mut s);
    frame(&mut s, &mut lab, &[]);

    let key = s.logistics.variant_being_edited().expect("editing");
    let placed = loadout(&s.logistics, key);
    let part = |form: ComponentForm| {
        placed
            .iter()
            .copied()
            .find(|p| form_of(&s.logistics, p.2) == form)
            .unwrap_or_else(|| panic!("a placed {form:?}"))
    };
    let weapon = placed
        .iter()
        .copied()
        .find(|p| is_weapon(form_of(&s.logistics, p.2)))
        .expect("a placed weapon");
    let (jump, heat, bulk) = (
        part(ComponentForm::JumpJet),
        part(ComponentForm::HeatSink),
        part(ComponentForm::Bulk),
    );

    // each switch leaves no text or row of the previous form behind; the jump jet is picked on
    // its own slot, not on a grid cell
    let at = lab
        .cell_centre(&s.logistics, weapon.0, weapon.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    frame(&mut s, &mut lab, &[]);
    assert_panel(&s, &lab, weapon.2);

    let at = lab.jump_slot_centre();
    click_at(&mut s, &mut lab, at);
    frame(&mut s, &mut lab, &[]);
    assert_panel(&s, &lab, jump.2);

    let at = lab
        .cell_centre(&s.logistics, heat.0, heat.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    frame(&mut s, &mut lab, &[]);
    assert_panel(&s, &lab, heat.2);

    let at = lab
        .cell_centre(&s.logistics, bulk.0, bulk.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    frame(&mut s, &mut lab, &[]);
    assert_panel(&s, &lab, bulk.2);

    let at = lab
        .cell_centre(&s.logistics, weapon.0, weapon.1)
        .expect("a cell");
    click_at(&mut s, &mut lab, at);
    frame(&mut s, &mut lab, &[]);
    assert_panel(&s, &lab, weapon.2);
}

/// The overwrite prompt of the save dialog, open and entered.
fn open_overwrite_prompt(s: &mut Setup) -> MechLab {
    let (mech, cell, _) = swap_candidate(s);
    save_second_loadout(s, mech, cell, "Second Loadout");
    let mut lab = open_lab(s, mech);
    let at = lab
        .cell_centre(&s.logistics, cell.0, cell.1)
        .expect("a cell");
    click_at(s, &mut lab, at);
    press(s, &mut lab, button::MB_MSG_REMOVE);
    press(s, &mut lab, button::MB_MSG_NEXT);
    assert!(lab.save.is_open(), "the save dialog asks for a name");
    settle(s, &mut lab);
    s.h.frame(&[], |ctx| {
        lab.save.screen.edits[0].set_entry(ctx.assets, "second loadout", false)
    });
    let yes = centre(lab.save.screen.button(button::YES).expect("Yes"));
    click_at(s, &mut lab, yes);
    assert!(lab.save.prompt_dialog().is_open(), "the prompt is up");
    settle(s, &mut lab);
    lab
}

#[test]
fn an_answered_overwrite_prompt_slides_out_and_leaves_the_dialog_usable() {
    let Some(mut s) = setup() else { return };
    let mut lab = open_overwrite_prompt(&mut s);
    let no = centre(&lab.save.prompt_dialog().screen.buttons[0]);
    click_at(&mut s, &mut lab, no);
    let prompt = lab.save.prompt_dialog();
    assert!(!prompt.is_open(), "No closes the prompt");
    assert!(
        prompt.anim.leaving() || !prompt.anim.exit.is_animating() || prompt.anim.exit.is_done(),
        "the prompt is sliding out or gone"
    );
    // frames go on: the prompt moves out and then is no longer drawn
    let mut offsets = Vec::new();
    for _ in 0..60 {
        frame(&mut s, &mut lab, &[]);
        offsets.push(lab.save.prompt_dialog().anim.frame().1);
    }
    assert!(
        !lab.save.prompt_dialog().anim.leaving(),
        "the prompt has finished leaving"
    );
    assert!(
        offsets.windows(2).any(|w| w[0] != w[1]) || offsets[0] == 0.0,
        "the prompt moved across frames: {offsets:?}"
    );
    assert!(lab.save.is_open(), "the save dialog is still usable");
    assert_eq!(lab.save.status, Status::Running);
}

#[test]
fn a_dialog_that_answers_keeps_sliding_out_in_the_mech_lab() {
    let Some(mut s) = setup() else { return };
    let mut lab = open_overwrite_prompt(&mut s);
    let yes = centre(&lab.save.prompt_dialog().screen.buttons[1]);
    click_at(&mut s, &mut lab, yes);
    assert_eq!(
        lab.status,
        Status::Running,
        "the lab hands over only after the panel has slid out"
    );
    assert!(!lab.save.is_open());
    assert!(lab.save.is_leaving(), "the panel is still sliding out");
    let mut seen = Vec::new();
    for _ in 0..90 {
        frame(&mut s, &mut lab, &[]);
        if !lab.save.is_leaving() {
            break;
        }
        let (_, x, y) = lab.save.anim.frame();
        seen.push((x, y));
    }
    assert!(
        seen.windows(2).any(|w| w[0] != w[1]),
        "the panel moved across the closing frames: {seen:?}"
    );
    assert!(!lab.save.is_leaving(), "the slide finishes");
    frame(&mut s, &mut lab, &[]);
    assert_eq!(lab.status, Status::Up, "Yes saves and leaves the lab");
}
