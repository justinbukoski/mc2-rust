// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Logistics screens (lane U2a) over the shipped game data. Set `MC2_DATA`; without it every test
//! returns early.

use std::sync::Arc;

use mc2_campaign::interface::{LoadType, MissionOptions};
use mc2_campaign::{GameData, Logistics};
use mc2_formats::vfs::Vfs;
use mc2_ui::harness::events::{left, move_to};
use mc2_ui::input::UiInput;
use mc2_ui::logistics::briefing::Briefing;
use mc2_ui::logistics::callout::Callout;
use mc2_ui::logistics::mission_select::MissionSelect;
use mc2_ui::logistics::sequence::Flow;
use mc2_ui::logistics::sequence::{ScreenVisits, Sequence};
use mc2_ui::logistics::{ids, Page};
use mc2_ui::screen::LogisticsScreen;
use mc2_ui::{Loader, UiAssets, UiOutput, UpdateCtx};

struct Game {
    vfs: Vfs,
    assets: UiAssets,
    data: Arc<GameData>,
}

fn game() -> Option<Game> {
    let root = std::env::var_os("MC2_DATA")?;
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let data = Arc::new(GameData::load(&mut vfs).expect("game data"));
    Some(Game {
        vfs,
        assets: UiAssets::default(),
        data,
    })
}

/// The campaign started at its first mission.
fn campaign(g: &Game) -> Logistics {
    Logistics::with_campaign(Arc::clone(&g.data), "campaign").expect("campaign")
}

/// One frame of input over `page`'s update, the way the app drives a page.
fn frame(
    ld: &mut Loader<'_>,
    input: &mut UiInput,
    events: &[mc2_platform::input::InputEvent],
    lg: &mut Logistics,
    update: impl FnOnce(&mut UpdateCtx<'_>, &mut Logistics),
) -> UiOutput {
    for e in events {
        input.handle(e);
    }
    input.begin_frame(1.0 / 30.0);
    let mut out = UiOutput::default();
    {
        let assets: &UiAssets = &*ld.assets;
        let mut ctx = UpdateCtx {
            input,
            assets,
            dt: 1.0 / 30.0,
            out: &mut out,
        };
        update(&mut ctx, lg);
    }
    input.end_frame();
    out
}

/// The centre of a button of `base`, in UI coordinates.
fn centre(base: &LogisticsScreen, id: i32) -> (f32, f32) {
    let b = base.button(id).expect("button");
    (
        b.obj.global_x() as f32 + b.obj.width() / 2.0,
        b.obj.global_y() as f32 + b.obj.height() / 2.0,
    )
}

#[test]
fn logistics_layouts_load_from_the_shipped_data() {
    let Some(mut g) = game() else { return };
    let lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let select = MissionSelect::load(&mut ld).expect("selection layout");
    assert_eq!(select.base.buttons.len(), 6, "launch button is the sixth");
    assert_eq!(select.movie_rect(), Some((117.0, 88.0, 263.0, 263.0)));
    let mut briefing = Briefing::load(&mut ld).expect("briefing layout");
    assert!(!briefing.base.statics.is_empty());
    let _ = (&mut briefing, lg);
}

#[test]
fn selection_presses_the_first_mission_and_lists_it() {
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut select = MissionSelect::load(&mut ld).unwrap();
    select.begin_screen(&mut ld, &mut lg).unwrap();

    assert_eq!(lg.current_mission(), "mc2_01");
    assert_eq!(select.pressed_mission(), Some("mc2_01"));
    assert_eq!(
        select.description.item_count(),
        4,
        "divider, name, divider, blurb"
    );
    assert_eq!(select.base.texts[4].text.trim(), lg.c_bills().to_string());
    let pressed: Vec<bool> = select
        .operation
        .buttons
        .iter()
        .map(|b| b.is_pressed())
        .collect();
    assert!(pressed[0] && pressed[1..].iter().all(|p| !p), "{pressed:?}");
    let shown = select
        .operation
        .buttons
        .iter()
        .filter(|b| b.obj.show)
        .count();
    assert_eq!(shown, lg.current_missions().len());
}

/// RC5 (triage c0b6a303): the funds readout must carry the player's C-Bills when the screen
/// opens (the first mission pressed), never the layout's placeholder: the test plants the
/// placeholder text first and checks `begin_screen` replaces it.
#[test]
fn selection_funds_replace_the_layout_placeholder_when_the_screen_opens() {
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    lg.add_c_bills(102_588);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut select = MissionSelect::load(&mut ld).unwrap();
    select.base.texts[4].set_text("88888");
    select.begin_screen(&mut ld, &mut lg).unwrap();
    assert_eq!(select.pressed_mission(), Some("mc2_01"));
    let want = lg.c_bills().to_string();
    assert_eq!(select.base.texts[4].text.trim(), want);
    assert_ne!(select.base.texts[4].text.trim(), "88888");
}

#[test]
fn briefing_lists_the_objectives_of_mission_one() {
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut briefing = Briefing::load(&mut ld).unwrap();
    briefing.begin_screen(&mut ld, &mut lg);

    let objectives = briefing.models.len();
    assert!(objectives > 0, "mission one has objectives");
    assert_eq!(briefing.objective_buttons.len(), objectives);
    // divider, name, divider, heading, divider, objectives..., divider, blurb
    assert_eq!(briefing.description.item_count(), 7 + objectives);
    assert!(
        briefing.drop_zone.is_some(),
        "mission one has a player part"
    );
    assert_eq!(briefing.base.texts[1].text.trim(), lg.c_bills().to_string());
}

/// RC6 (triage c0b6a303): the objective list cycles every three seconds until the player clicks;
/// the current entry and its map marker turn red (missionbriefingscreen.cpp:182-185, 233-235).
/// The C++ reference was shot after the first cycle, the old harness shot at one second.
#[test]
fn briefing_turns_the_first_objective_red_after_three_seconds() {
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut briefing = Briefing::load(&mut ld).unwrap();
    briefing.begin_screen(&mut ld, &mut lg);
    let mut input = UiInput::new(800, 600);
    let red = |b: &Briefing| {
        (0..b.description.item_count())
            .filter(|&i| b.description.item(i).unwrap().obj().color() == 0xffff_0000)
            .count()
    };
    assert_eq!(red(&briefing), 0, "no entry is red before the first cycle");
    for _ in 0..100 {
        frame(&mut ld, &mut input, &[], &mut lg, |ctx, lg| {
            briefing.update_screen(ctx, lg)
        });
    }
    assert_eq!(red(&briefing), 1, "the current objective is red");
    assert_eq!(
        briefing
            .objective_buttons
            .iter()
            .filter(|b| b.color() == 0xffff_0000)
            .count(),
        1,
        "and so is its map marker"
    );
}

/// Triage 3d111c56 (visual-qa settle): `SimpleCamera::setObject` draws a `BLDG_TYPE` objective model
/// through `setBuilding`/`setComponent` (the `<name>enc.ini` appearance; its `resetPaintScheme` is the empty
/// `Appearance` one, appear.h:390-392), not as a bare shape painted with the objective's colours
/// (simplecamera.cpp:438-520, 523-554). Mission 1's first objective is the `Hangar` building.
#[test]
fn briefing_draws_a_building_objective_as_a_building_appearance() {
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut briefing = Briefing::load(&mut ld).unwrap();
    briefing.begin_screen(&mut ld, &mut lg);
    let mut input = UiInput::new(800, 600);
    for _ in 0..100 {
        frame(&mut ld, &mut input, &[], &mut lg, |ctx, lg| {
            briefing.update_screen(ctx, lg)
        });
    }
    let first = &briefing.models[0];
    assert_eq!(
        (first.name.as_str(), first.model_type),
        ("Hangar", Some(14))
    );
    let req = briefing.preview.as_ref().expect("objective 1 has a model");
    assert!(req.building, "BLDG_TYPE goes through setComponent");
    assert_eq!(req.shape, "hangar");
    assert_eq!(req.paint, [0; 3], "the building paint scheme is a no-op");
    assert_eq!(req.scale, 0.7, "setScale(modelScales[ID])");
}

/// RC7 (triage c0b6a303): the Options resolution text. `OptionsGraphics::begin` selects the
/// current mode (`aDropList::SelectItem`), which leaves the item `SELECTED`; its "Pressed"
/// animation starts white (the C++ reference was shot on that frame). The first
/// `aDropList::update` then clears the list's selection (`ListBox().SelectItem(-1)`, alistbox.cpp
/// :850-857), so the closed box settles on the item's Normal colour (brown), exactly like the
/// video-card box next to it. The port does the same; this pins both ends so neither a stuck
/// white nor a stuck dim state can slip in unnoticed.
#[test]
fn the_options_drop_list_selection_is_white_on_its_first_frame_then_settles_like_the_original() {
    use mc2_ui::widgets::listbox::{DropList, ItemState};
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut file = ld.art_fit("mcl_options_combobox0").unwrap();
    let mut list = DropList::from_fit(&mut ld, &mut file, "PlayerNameComboBox");
    list.add_text("1024x768x32", 0xffff_ffff);
    list.add_text("800x600x32", 0xffff_ffff);
    list.select_item(0);
    let color = |l: &mut DropList| {
        // what `aAnimTextListItem::render` does first: set the animation state, read its colour
        let it = l.list.item_mut(0).unwrap();
        let s = it.state();
        let item = it
            .as_any_mut()
            .downcast_mut::<mc2_ui::widgets::listbox::AnimTextListItem>()
            .unwrap();
        item.anim.set_state(s.anim_state());
        (s, item.anim.current_color(s.anim_state()))
    };
    assert_eq!(color(&mut list), (ItemState::Selected, 0xffff_ffff));
    let mut input = UiInput::new(800, 600);
    frame(&mut ld, &mut input, &[], &mut lg, |ctx, _| list.update(ctx));
    assert_eq!(color(&mut list), (ItemState::Enabled, 0xff84_6643));
}

/// Centre of an entry of the list, in UI coordinates.
fn entry_centre(bay: &mc2_ui::logistics::mech_bay::MechBay, i: usize) -> (f32, f32) {
    let e = bay.list.entries().nth(i).expect("entry");
    let r = e.box_rect();
    (
        (r.left + r.right) as f32 / 2.0,
        (r.top + r.bottom) as f32 / 2.0,
    )
}

/// A click the way a player makes one: the pointer comes over the target, the button goes down,
/// then up, each in its own frame.
fn click(
    ld: &mut Loader<'_>,
    input: &mut UiInput,
    lg: &mut Logistics,
    at: (f32, f32),
    mut update: impl FnMut(&mut UpdateCtx<'_>, &mut Logistics),
) {
    idle(ld, input, lg, &mut update, DOUBLE_CLICK_FRAMES);
    frame(ld, input, &[move_to(at.0, at.1)], lg, &mut update);
    frame(ld, input, &[left(true)], lg, &mut update);
    frame(ld, input, &[left(false)], lg, &mut update);
    idle(ld, input, lg, &mut update, DOUBLE_CLICK_FRAMES);
}

/// Frames the double-click clock needs to run out (0.2 s at 30 frames a second): a click
/// within it of the last one is a double click, as a player's would be.
const DOUBLE_CLICK_FRAMES: usize = 8;

/// `n` frames without input.
fn idle(
    ld: &mut Loader<'_>,
    input: &mut UiInput,
    lg: &mut Logistics,
    mut update: impl FnMut(&mut UpdateCtx<'_>, &mut Logistics),
    n: usize,
) {
    for _ in 0..n {
        frame(ld, input, &[], lg, &mut update);
    }
}

#[test]
fn mech_bay_lists_the_inventory_and_adds_a_selected_mech() {
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut bay = MechBay::load(&mut ld).unwrap();
    // the campaign starts with an empty bay: buy the cheapest mech twice
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    let cheapest = variants[0];
    lg.add_c_bills(200_000);
    lg.purchase_mech(cheapest).expect("purchase");
    lg.purchase_mech(cheapest).expect("purchase");
    Page::begin(&mut bay, &mut ld, &mut lg);

    assert_eq!(
        bay.list.item_count(),
        1,
        "both mechs are one variant: one entry"
    );
    assert_eq!(bay.list.entries().next().unwrap().count, 2);
    assert!(lg.force_group().is_empty());
    assert!(bay.icons.iter().all(|i| i.content.mech.is_none()));

    let mut input = UiInput::new(800, 600);
    let at = entry_centre(&bay, 0);
    click(&mut ld, &mut input, &mut lg, at, |ctx, lg| {
        bay.update(ctx, lg)
    });
    assert_eq!(bay.list.selected(), 0, "the entry is selected by the click");
    let picked = bay.list.current_mech().expect("selected mech");

    // the ADD button puts it in the force group, in slot 1
    let add = centre(&bay.base, ids::MB_MSG_ADD);
    click(&mut ld, &mut input, &mut lg, add, |ctx, lg| {
        bay.update(ctx, lg)
    });
    assert_eq!(
        lg.force_group(),
        vec![picked],
        "the selected mech joined the force group"
    );
    assert_eq!(bay.icons[0].content.mech, Some(picked), "slot 1 shows it");
    assert_eq!(lg.mech(picked).map(|m| m.force_group), Some(1));
    assert_eq!(
        bay.list.entries().next().unwrap().count,
        1,
        "one left in storage"
    );
}

#[test]
fn mech_bay_drags_a_mech_into_the_force_group_and_back() {
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut bay = MechBay::load(&mut ld).unwrap();
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    lg.add_c_bills(200_000);
    lg.purchase_mech(variants[0]).expect("purchase");
    lg.purchase_mech(variants[0]).expect("purchase");
    Page::begin(&mut bay, &mut ld, &mut lg);
    let mut input = UiInput::new(800, 600);

    // press on the entry, drag to slot 2, release: the mech takes slot 1 (the first free slot)
    let from = entry_centre(&bay, 0);
    idle(
        &mut ld,
        &mut input,
        &mut lg,
        |ctx, lg| bay.update(ctx, lg),
        DOUBLE_CLICK_FRAMES,
    );
    let to = centre_of_icon(&bay, 0);
    frame(
        &mut ld,
        &mut input,
        &[move_to(from.0, from.1)],
        &mut lg,
        |ctx, lg| bay.update(ctx, lg),
    );
    frame(&mut ld, &mut input, &[left(true)], &mut lg, |ctx, lg| {
        bay.update(ctx, lg)
    });
    frame(
        &mut ld,
        &mut input,
        &[move_to(to.0, to.1)],
        &mut lg,
        |ctx, lg| bay.update(ctx, lg),
    );
    frame(&mut ld, &mut input, &[left(false)], &mut lg, |ctx, lg| {
        bay.update(ctx, lg)
    });
    assert_eq!(
        lg.force_group().len(),
        1,
        "dropped on the slots: in the force group"
    );
    assert_eq!(bay.icons[0].content.mech, Some(lg.force_group()[0]));
    assert!(bay.drag_mech.is_none());

    // press on slot 1 and drag it out to the list's area: it leaves the force group again
    let slot = centre_of_icon(&bay, 0);
    idle(
        &mut ld,
        &mut input,
        &mut lg,
        |ctx, lg| bay.update(ctx, lg),
        DOUBLE_CLICK_FRAMES,
    );
    let out = (300.0, 380.0);
    frame(
        &mut ld,
        &mut input,
        &[move_to(slot.0, slot.1)],
        &mut lg,
        |ctx, lg| bay.update(ctx, lg),
    );
    frame(&mut ld, &mut input, &[left(true)], &mut lg, |ctx, lg| {
        bay.update(ctx, lg)
    });
    frame(
        &mut ld,
        &mut input,
        &[move_to(out.0, out.1)],
        &mut lg,
        |ctx, lg| bay.update(ctx, lg),
    );
    frame(&mut ld, &mut input, &[left(false)], &mut lg, |ctx, lg| {
        bay.update(ctx, lg)
    });
    assert!(
        lg.force_group().is_empty(),
        "dropped in storage: out of the force group"
    );
    assert_eq!(bay.list.entries().next().unwrap().count, 2);
}

#[test]
fn mech_bay_remove_button_takes_the_selected_slot_out() {
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut bay = MechBay::load(&mut ld).unwrap();
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    lg.add_c_bills(200_000);
    lg.purchase_mech(variants[0]).expect("purchase");
    lg.purchase_mech(variants[0]).expect("purchase");
    Page::begin(&mut bay, &mut ld, &mut lg);
    let mut input = UiInput::new(800, 600);
    let at = entry_centre(&bay, 0);
    click(&mut ld, &mut input, &mut lg, at, |ctx, lg| {
        bay.update(ctx, lg)
    });
    let add = centre(&bay.base, ids::MB_MSG_ADD);
    click(&mut ld, &mut input, &mut lg, add, |ctx, lg| {
        bay.update(ctx, lg)
    });
    let add = centre(&bay.base, ids::MB_MSG_ADD);
    let _ = add;
    assert_eq!(lg.force_group().len(), 1);

    // select slot 1, then REMOVE
    let slot = centre_of_icon(&bay, 0);
    click(&mut ld, &mut input, &mut lg, slot, |ctx, lg| {
        bay.update(ctx, lg)
    });
    let remove = centre(&bay.base, ids::MB_MSG_REMOVE);
    click(&mut ld, &mut input, &mut lg, remove, |ctx, lg| {
        bay.update(ctx, lg)
    });
    assert!(lg.force_group().is_empty(), "REMOVE takes the slot out");
    assert!(bay.icons[0].content.mech.is_none());
    assert_eq!(bay.list.entries().next().unwrap().count, 2);
}

/// Centre of a force group slot, in UI coordinates.
fn centre_of_icon(bay: &mc2_ui::logistics::mech_bay::MechBay, i: usize) -> (f32, f32) {
    let r = bay.icons[i].outline.global_rect();
    (
        (r.left + r.right) as f32 / 2.0,
        (r.top + r.bottom) as f32 / 2.0,
    )
}

/// A campaign with two mechs bought and the first in the force group (the pilot room's start).
fn bay_with_a_mech(g: &mut Game) -> Logistics {
    let mut lg = campaign(g);
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    lg.add_c_bills(200_000);
    lg.purchase_mech(variants[0]).expect("purchase");
    lg.purchase_mech(variants[0]).expect("purchase");
    let first = lg.inventory()[0].key;
    lg.add_mech_to_force_group(first, 1).expect("force group");
    lg
}

#[test]
fn pilot_room_lists_the_free_pilots_and_assigns_one_with_add() {
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = bay_with_a_mech(&mut g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    Page::begin(&mut room, &mut ld, &mut lg);

    let free = lg
        .pilots()
        .iter()
        .filter(|p| !p.used && p.is_available())
        .count();
    assert_eq!(room.list.item_count(), free, "every free pilot is listed");
    assert!(free > 0);
    let mech = room.icons[0].content.mech.expect("slot 1 has the mech");
    assert_eq!(lg.mech(mech).and_then(|m| m.pilot), None, "no pilot yet");
    assert_eq!(
        room.cur_pilot,
        room.list.entries().next().map(|e| e.pilot())
    );

    // ADD takes the listed pilot the list has selected into the selected slot
    let mut input = UiInput::new(800, 600);
    let add = centre(&room.base, mc2_ui::logistics::ids::PR_MSG_ADD);
    click(&mut ld, &mut input, &mut lg, add, |ctx, lg| {
        room.update(ctx, lg)
    });
    let pilot = lg
        .mech(mech)
        .and_then(|m| m.pilot)
        .expect("a pilot joined the mech");
    assert!(lg.pilots()[pilot].used);
    assert!(room.icons[0].content.pilot_photo.is_some());
    // the slot shows the pilot's display name (`getName()`), not the profile stem
    let name_id = lg.pilots()[pilot].name_id as u32;
    assert_eq!(room.icons[0].content.pilot_name, ld.assets.string(name_id));
    assert_ne!(
        room.icons[0].content.pilot_name,
        lg.pilots()[pilot].file_name
    );
    assert!(
        room.list.entries().all(|e| e.pilot() != pilot),
        "the pilot left the list"
    );

    // the record of an assigned pilot, off the free list, still shows the pilot's rank
    room.set_pilot(ld.assets, &lg, Some(pilot));
    assert_eq!(room.cur_rank, Some(lg.pilots()[pilot].rank));
}

#[test]
fn pilot_room_drags_a_pilot_onto_a_slot_and_removes_it_again() {
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = bay_with_a_mech(&mut g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    Page::begin(&mut room, &mut ld, &mut lg);
    let mech = room.icons[0].content.mech.expect("slot 1 has the mech");
    let mut input = UiInput::new(800, 600);

    // press the second listed pilot and drag it onto slot 1
    let from = {
        let r = room
            .list
            .entries()
            .nth(1)
            .expect("two pilots listed")
            .box_rect();
        (
            (r.left + r.right) as f32 / 2.0,
            (r.top + r.bottom) as f32 / 2.0,
        )
    };
    let to = slot_centre(&room.icons, 0);
    idle(
        &mut ld,
        &mut input,
        &mut lg,
        |ctx, lg| room.update(ctx, lg),
        DOUBLE_CLICK_FRAMES,
    );
    frame(
        &mut ld,
        &mut input,
        &[move_to(from.0, from.1)],
        &mut lg,
        |ctx, lg| room.update(ctx, lg),
    );
    frame(&mut ld, &mut input, &[left(true)], &mut lg, |ctx, lg| {
        room.update(ctx, lg)
    });
    // a pilot's drag starts while the pointer is still over its entry (`isInside`)
    frame(
        &mut ld,
        &mut input,
        &[move_to(from.0 + 20.0, from.1)],
        &mut lg,
        |ctx, lg| room.update(ctx, lg),
    );
    frame(
        &mut ld,
        &mut input,
        &[move_to(to.0, to.1)],
        &mut lg,
        |ctx, lg| room.update(ctx, lg),
    );
    frame(&mut ld, &mut input, &[left(false)], &mut lg, |ctx, lg| {
        room.update(ctx, lg)
    });
    let pilot = lg
        .mech(mech)
        .and_then(|m| m.pilot)
        .expect("the dragged pilot joined the mech");
    assert!(room.drag_pilot.is_none(), "the drag ended");

    // select slot 1 and press REMOVE: the pilot goes back to the list
    idle(
        &mut ld,
        &mut input,
        &mut lg,
        |ctx, lg| room.update(ctx, lg),
        DOUBLE_CLICK_FRAMES,
    );
    click(&mut ld, &mut input, &mut lg, to, |ctx, lg| {
        room.update(ctx, lg)
    });
    let remove = centre(&room.base, mc2_ui::logistics::ids::PR_MSG_REMOVE);
    click(&mut ld, &mut input, &mut lg, remove, |ctx, lg| {
        room.update(ctx, lg)
    });
    assert_eq!(
        lg.mech(mech).and_then(|m| m.pilot),
        None,
        "REMOVE released the pilot"
    );
    assert!(
        room.list.entries().any(|e| e.pilot() == pilot),
        "the pilot is listed again"
    );
}

#[test]
fn an_assigned_pilots_drag_icon_shows_the_pilots_own_portrait() {
    use mc2_ui::logistics::pilot_list::set_portrait;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = bay_with_a_mech(&mut g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    Page::begin(&mut room, &mut ld, &mut lg);
    let mut input = UiInput::new(800, 600);
    let add = centre(&room.base, mc2_ui::logistics::ids::PR_MSG_ADD);
    click(&mut ld, &mut input, &mut lg, add, |ctx, lg| {
        room.update(ctx, lg)
    });
    let pilot = room.icons[0]
        .content
        .mech
        .and_then(|m| lg.mech(m))
        .and_then(|m| m.pilot)
        .expect("a pilot joined the mech");
    // the assigned pilot is off the free list, so only the campaign record knows its portrait
    assert!(room.list.entries().all(|e| e.pilot() != pilot));

    room.begin_drag(&lg, pilot);
    let photo = lg.pilots()[pilot].photo_index;
    let mut want = room.drag_portrait().clone();
    set_portrait(&mut want, photo);
    for i in 0..4 {
        let got = &room.drag_portrait().loc[i];
        assert_eq!(got.u, want.loc[i].u, "corner {i} u");
        assert_eq!(got.v, want.loc[i].v, "corner {i} v");
    }
}

#[test]
fn the_pilot_room_acknowledges_new_pilots_on_a_visit() {
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    // unlock every bonus list the data has: one of them offers a pilot the campaign does not have
    let bonus: Vec<String> = g.data.purchase_lists.keys().cloned().collect();
    for name in &bonus {
        lg.add_new_bonus_purchase_file(name);
    }
    lg.update_availability().expect("availability");
    assert!(lg.new_pilots_available(), "a bonus list offers a new pilot");

    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    Page::begin(&mut room, &mut ld, &mut lg);
    assert!(lg.new_pilots_available(), "the flag waits for the room");

    let mut input = UiInput::new(800, 600);
    frame(&mut ld, &mut input, &[], &mut lg, |ctx, lg| {
        room.update(ctx, lg)
    });
    assert!(
        !lg.new_pilots_available(),
        "the room's visit acknowledges the new pilots"
    );
}

/// `PilotReadyScreen::render` (`code/pilotreadyscreen.cpp:284-289`): at rest, `BETTY_NEW_PILOTS`
/// plays and then the new pilots are acknowledged. The sample is in the frame's output; the flag
/// clears in the same frame, after it.
#[test]
fn the_new_pilot_cue_plays_before_the_flag_clears() {
    use mc2_ui::ctx::betty;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let bonus: Vec<String> = g.data.purchase_lists.keys().cloned().collect();
    for name in &bonus {
        lg.add_new_bonus_purchase_file(name);
    }
    lg.update_availability().expect("availability");
    assert!(lg.new_pilots_available(), "a bonus list offers a new pilot");

    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    Page::begin(&mut room, &mut ld, &mut lg);
    let mut input = UiInput::new(800, 600);
    let out = frame(&mut ld, &mut input, &[], &mut lg, |ctx, lg| {
        room.update(ctx, lg)
    });
    assert_eq!(
        out.betty,
        vec![betty::NEW_PILOTS],
        "BETTY_NEW_PILOTS (mclib/sounds.h:217)"
    );
    assert!(!lg.new_pilots_available());

    // a second visit has nothing new to announce
    let out = frame(&mut ld, &mut input, &[], &mut lg, |ctx, lg| {
        room.update(ctx, lg)
    });
    assert!(
        out.betty.is_empty(),
        "the cue plays once per acknowledgement"
    );
}

/// Centre of a slot's box, in UI coordinates.
fn slot_centre(icons: &[mc2_ui::logistics::mech_icon::LogisticsMechIcon], i: usize) -> (f32, f32) {
    let r = icons[i].outline.global_rect();
    (
        (r.left + r.right) as f32 / 2.0,
        (r.top + r.bottom) as f32 / 2.0,
    )
}

/// One frame of the logistics sequence (`Sequence::update`), with the frame's events.
fn seq_frame(
    ld: &mut Loader<'_>,
    input: &mut UiInput,
    seq: &mut Sequence,
    lg: &mut Logistics,
    events: &[mc2_platform::input::InputEvent],
) -> Flow {
    for e in events {
        input.handle(e);
    }
    input.begin_frame(1.0 / 30.0);
    let mut out = UiOutput::default();
    let flow = seq.update(ld, input, &mut out, 1.0 / 30.0, lg);
    input.end_frame();
    flow
}

/// A click on `at` through the sequence; returns the flow the release frame reported.
fn seq_click(
    ld: &mut Loader<'_>,
    input: &mut UiInput,
    seq: &mut Sequence,
    lg: &mut Logistics,
    at: (f32, f32),
) -> Flow {
    for _ in 0..DOUBLE_CLICK_FRAMES {
        seq_frame(ld, input, seq, lg, &[]);
    }
    seq_frame(ld, input, seq, lg, &[move_to(at.0, at.1)]);
    seq_frame(ld, input, seq, lg, &[left(true)]);
    let flow = seq_frame(ld, input, seq, lg, &[left(false)]);
    for _ in 0..DOUBLE_CLICK_FRAMES {
        seq_frame(ld, input, seq, lg, &[]);
    }
    flow
}

/// The centre of a button of page `page`, in UI coordinates.
fn page_button(seq: &mut Sequence, page: usize, id: i32) -> (f32, f32) {
    centre(seq.page_mut(page).base(), id)
}

#[test]
fn the_logistics_sequence_runs_from_selection_to_launch() {
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::sequence::{slot, Sequence};
    use mc2_ui::logistics::{ids, Placeholder};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    // two mechs in storage, none in the force group
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    lg.add_c_bills(200_000);
    lg.purchase_mech(variants[0]).expect("purchase");
    lg.purchase_mech(variants[0]).expect("purchase");

    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let pages: Vec<Box<dyn Page>> = vec![
        Box::new(MissionSelect::load(&mut ld).unwrap()),
        Box::new(Briefing::load(&mut ld).unwrap()),
        Box::new(MechBay::load(&mut ld).unwrap()),
        Box::new(PilotReady::load(&mut ld).unwrap()),
        Box::new(Placeholder::unavailable("purchase")),
        Box::new(Placeholder::unavailable("mech lab")),
        // the loading screen is a stand-in: the test launches, which needs it to report done
        Box::new(Placeholder::new("loading")),
        Box::new(Placeholder::unavailable("options")),
    ];
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(slot::SELECT));
    assert_eq!(lg.current_mission(), "mc2_01");

    // 1. the selection screen's NEXT goes on to the briefing
    let next = page_button(&mut seq, slot::SELECT, ids::MB_MSG_NEXT);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(flow, Flow::Running);
    assert_eq!(
        seq.current_page(),
        Some(slot::BRIEFING),
        "selection -> briefing"
    );
    assert_eq!(
        lg.current_mission(),
        "mc2_01",
        "the first mission stays chosen"
    );

    // 2. the briefing's NEXT goes on to the mech bay
    let next = page_button(&mut seq, slot::BRIEFING, ids::MB_MSG_NEXT);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(
        seq.current_page(),
        Some(slot::MECH_BAY),
        "briefing -> mech bay"
    );
    assert!(lg.force_group().is_empty());

    // 3. a mech from storage is dragged onto the deployment slots
    let (from, to) = {
        let bay = seq
            .page_mut(slot::MECH_BAY)
            .as_any_mut()
            .downcast_mut::<MechBay>()
            .expect("the mech bay");
        (entry_centre(bay, 0), slot_centre(&bay.icons, 0))
    };
    for _ in 0..DOUBLE_CLICK_FRAMES {
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    }
    seq_frame(
        &mut ld,
        &mut input,
        &mut seq,
        &mut lg,
        &[move_to(from.0, from.1)],
    );
    seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[left(true)]);
    seq_frame(
        &mut ld,
        &mut input,
        &mut seq,
        &mut lg,
        &[move_to(to.0, to.1)],
    );
    seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[left(false)]);
    assert_eq!(lg.force_group().len(), 1, "the mech is in the force group");
    let mech = lg.force_group()[0];
    assert_eq!(lg.mech(mech).map(|m| m.force_group), Some(1), "in slot 1");

    // 4. the mech bay's NEXT goes on to the pilot room
    let next = page_button(&mut seq, slot::MECH_BAY, ids::MB_MSG_NEXT);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(
        seq.current_page(),
        Some(slot::PILOT_READY),
        "mech bay -> pilot room"
    );
    assert_eq!(lg.mech(mech).and_then(|m| m.pilot), None);

    // 5. ADD puts the listed pilot in the mech's slot
    let add = page_button(&mut seq, slot::PILOT_READY, ids::PR_MSG_ADD);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, add);
    let pilot = lg
        .mech(mech)
        .and_then(|m| m.pilot)
        .expect("a pilot is assigned");
    assert!(lg.pilots()[pilot].used);

    // 6. with every slot crewed, NEXT goes on to the loading screen
    let next = page_button(&mut seq, slot::PILOT_READY, ids::MB_MSG_NEXT);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(
        seq.current_page(),
        Some(slot::LOADING),
        "pilot room -> loading"
    );

    // 7. the loading screen reports done: the sequence launches the mission
    seq.page_mut(slot::LOADING).set_status(Status::Next);
    let flow = seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    assert_eq!(flow, Flow::Launch);

    // 8. the campaign hands the mission over. Mission one plays without logistics
    // (`skipLogistics`), so its force comes from the mission file, not from the bay.
    assert!(lg.skip_logistics());
    let spec = lg.load_spec(MissionOptions::default());
    assert_eq!(spec.mission, "mc2_01");
    assert_eq!(spec.load_type, LoadType::SpQuickstart);
    assert!(spec.force.is_empty());
}

#[test]
fn sequence_moves_between_screens_on_each_status() {
    use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
    use mc2_ui::logistics::Placeholder;
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let names = [
        "select", "brief", "bay", "pilot", "purchase", "lab", "loading", "options",
    ];
    let pages: Vec<Box<dyn Page>> = names
        .iter()
        .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>)
        .collect();
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    let mut out = UiOutput::default();
    let mut step =
        |seq: &mut Sequence, ld: &mut Loader<'_>, lg: &mut Logistics, input: &mut UiInput| {
            input.begin_frame(1.0 / 30.0);
            let flow = seq.update(ld, input, &mut out, 1.0 / 30.0, lg);
            input.end_frame();
            flow
        };
    let set = |seq: &mut Sequence, page: usize, s: Status| seq.page_mut(page).set_status(s);

    seq.begin(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(slot::SELECT));
    set(&mut seq, slot::SELECT, Status::Next);
    assert_eq!(step(&mut seq, &mut ld, &mut lg, &mut input), Flow::Running);
    assert_eq!(seq.current_page(), Some(slot::BRIEFING), "next moves right");

    set(&mut seq, slot::BRIEFING, Status::Previous);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(
        seq.current_page(),
        Some(slot::SELECT),
        "previous moves left"
    );

    set(&mut seq, slot::SELECT, Status::Next);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    set(&mut seq, slot::BRIEFING, Status::Next);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY));

    set(&mut seq, slot::MECH_BAY, Status::Up);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(
        seq.current_page(),
        Some(slot::PURCHASE),
        "up goes to the purchase screen"
    );
    set(&mut seq, slot::PURCHASE, Status::Down);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY), "down comes back");

    set(&mut seq, slot::MECH_BAY, Status::Down);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(
        seq.current_page(),
        Some(slot::MECH_LAB),
        "down goes to the mech lab"
    );
    set(&mut seq, slot::MECH_LAB, Status::Up);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY));

    set(&mut seq, slot::MECH_BAY, Status::Next);
    step(&mut seq, &mut ld, &mut lg, &mut input);
    set(&mut seq, slot::PILOT_READY, Status::SkipOneNext);
    let flow = step(&mut seq, &mut ld, &mut lg, &mut input);
    assert_eq!(
        flow,
        Flow::Launch,
        "skipping past the loading screen launches"
    );

    // a restart from the main menu starts the run again: with logistics skipped (as mission one
    // is) it goes straight to the loading screen (`MissionBegin::update`, RESTART)
    let first = if lg.skip_logistics() {
        slot::LOADING
    } else {
        slot::SELECT
    };
    seq.restart(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(first));
    set(&mut seq, first, Status::MainMenu);
    assert_eq!(step(&mut seq, &mut ld, &mut lg, &mut input), Flow::MainMenu);
    assert_eq!(
        step(&mut seq, &mut ld, &mut lg, &mut input),
        Flow::MainMenu,
        "stays on the menu"
    );
    seq.restart(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(first));
    assert!(!seq.ready_to_load());
}

/// One frame of the sequence with no input; returns the flow it reported.
fn step_sequence(
    seq: &mut Sequence,
    ld: &mut Loader<'_>,
    lg: &mut Logistics,
    input: &mut UiInput,
) -> Flow {
    input.begin_frame(1.0 / 30.0);
    let mut out = UiOutput::default();
    let flow = seq.update(ld, input, &mut out, 1.0 / 30.0, lg);
    input.end_frame();
    flow
}

/// Walks the placeholder sequence from its first screen to the loading screen by NEXT.
fn walk_to_loading(
    seq: &mut Sequence,
    ld: &mut Loader<'_>,
    lg: &mut Logistics,
    input: &mut UiInput,
) {
    use mc2_ui::logistics::sequence::slot;
    use mc2_ui::screen::Status;
    for page in [
        slot::SELECT,
        slot::BRIEFING,
        slot::MECH_BAY,
        slot::PILOT_READY,
    ] {
        seq.page_mut(page).set_status(Status::Next);
        step_sequence(seq, ld, lg, input);
    }
    assert_eq!(seq.current_page(), Some(slot::LOADING));
}

#[test]
fn a_new_visit_clears_the_ready_to_load_flag() {
    use mc2_ui::logistics::sequence::{slot, Sequence};
    use mc2_ui::logistics::Placeholder;
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let names = [
        "select", "brief", "bay", "pilot", "purchase", "lab", "loading", "options",
    ];
    let pages: Vec<Box<dyn Page>> = names
        .iter()
        .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>)
        .collect();
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);

    // the loading screen reports ready: the flag is set, and the mission launches on NEXT
    seq.begin(&mut ld, &mut lg);
    walk_to_loading(&mut seq, &mut ld, &mut lg, &mut input);
    seq.page_mut(slot::LOADING).set_status(Status::ReadyToLoad);
    assert_eq!(
        step_sequence(&mut seq, &mut ld, &mut lg, &mut input),
        Flow::Running
    );
    assert!(seq.ready_to_load());

    // a new visit from the briefing (`setToMissionBriefing`) clears it
    seq.skip_to_briefing(&mut ld, &mut lg);
    assert!(!seq.ready_to_load(), "skip to briefing clears the flag");
    walk_to_loading(&mut seq, &mut ld, &mut lg, &mut input);
    seq.page_mut(slot::LOADING).set_status(Status::ReadyToLoad);
    step_sequence(&mut seq, &mut ld, &mut lg, &mut input);
    assert!(seq.ready_to_load());

    // and a new visit from the start (`MissionBegin::begin`) clears it too
    seq.begin(&mut ld, &mut lg);
    assert!(!seq.ready_to_load(), "begin clears the flag");
}

#[test]
fn the_tutorial_logistics_brain_loads_and_runs_on_the_sequence() {
    use mc2_ui::logistics::sequence::{slot, Sequence};
    use mc2_ui::logistics::Placeholder;
    let Some(mut g) = game() else { return };
    // the campaign's first operation carries `Tutorial0` (the golden's first three: Tutorial0-2)
    let mut lg = campaign(&g);
    assert_eq!(
        lg.mission_info().current_abl_script_name(),
        Some("Tutorial0")
    );
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let names = [
        "select", "brief", "bay", "pilot", "purchase", "lab", "loading", "options",
    ];
    let pages: Vec<Box<dyn Page>> = names
        .iter()
        .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>)
        .collect();
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    assert!(seq.has_brain(), "Tutorial0 is loaded from the game data");
    // a brain that fails to execute is dropped, so it is still there after frames on two screens
    for _ in 0..5 {
        step_sequence(&mut seq, &mut ld, &mut lg, &mut input);
    }
    assert!(seq.has_brain(), "Tutorial0 executes without error");
    seq.page_mut(slot::SELECT)
        .set_status(mc2_ui::screen::Status::Next);
    step_sequence(&mut seq, &mut ld, &mut lg, &mut input);
    for _ in 0..5 {
        step_sequence(&mut seq, &mut ld, &mut lg, &mut input);
    }
    assert!(seq.has_brain(), "and on the next screen");
}

/// The movie block of the selection's `begin` and its controls (`code/missionselectionscreen.cpp:228-267`,
/// `:326-376`). The mission's movie is opened, playing, and the campaign's `videoShown` flag is set, as the
/// C++ sets it. A revisit ends the screen (its movie is closed) and opens the movie again stopped, because
/// the campaign has shown it. Play, Pause and Stop are requests to the app's movie host, never unavailable.
#[test]
fn the_movie_controls_send_requests_and_are_never_unavailable() {
    use mc2_ui::logistics::mission_select::MissionSelect;
    use mc2_ui::logistics::movie::MovieRequest;
    use mc2_ui::logistics::sequence::{slot, Sequence};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut pages = stub_pages();
    pages[slot::SELECT] = Box::new(MissionSelect::load(&mut ld).unwrap());
    let mut seq = Sequence::single_player(pages);
    seq.connect_movie_host();
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(slot::SELECT));
    let movie = selection_movie(&mut seq).expect("the first mission names a movie");
    assert!(
        lg.video_shown(),
        "the movie is opened and the campaign marks it shown ({movie})"
    );
    assert_eq!(
        seq.take_movie_requests(),
        vec![MovieRequest::Open(movie.clone())],
        "a first showing opens the movie playing, with nothing to stop"
    );

    let pause = page_button(&mut seq, slot::SELECT, ids::MN_MSG_PAUSE);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, pause);
    assert_ne!(flow, Flow::Unavailable("mission movie playback"));
    assert_eq!(seq.take_movie_requests(), vec![MovieRequest::Pause(true)]);
    assert!(
        selection_pressed(&mut seq, ids::MN_MSG_PAUSE),
        "the Pause button shows the hold"
    );

    let pause = page_button(&mut seq, slot::SELECT, ids::MN_MSG_PAUSE);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, pause);
    assert_ne!(flow, Flow::Unavailable("mission movie playback"));
    assert_eq!(seq.take_movie_requests(), vec![MovieRequest::Pause(false)]);
    assert!(!selection_pressed(&mut seq, ids::MN_MSG_PAUSE));

    let stop = page_button(&mut seq, slot::SELECT, ids::MN_MSG_STOP);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, stop);
    assert_ne!(flow, Flow::Unavailable("mission movie playback"));
    assert_eq!(seq.take_movie_requests(), vec![MovieRequest::Stop]);
    assert_eq!(
        seq.page_mut(slot::SELECT).status(),
        Status::Running,
        "the screen stays where it is"
    );

    let play = page_button(&mut seq, slot::SELECT, ids::MN_MSG_PLAY);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, play);
    assert_ne!(flow, Flow::Unavailable("mission movie playback"));
    assert_eq!(seq.take_movie_requests(), vec![MovieRequest::Play]);

    // a revisit: the screen is ended (its movie closed), then begun again; the campaign has shown
    // the movie, so it opens stopped
    seq.page_mut(slot::SELECT).end(&mut lg);
    seq.page_mut(slot::SELECT).begin(&mut ld, &mut lg);
    assert_eq!(
        seq.take_movie_requests(),
        vec![
            MovieRequest::Close,
            MovieRequest::Open(movie.clone()),
            MovieRequest::Stop
        ]
    );
    assert_eq!(selection_movie(&mut seq).as_deref(), Some(movie.as_str()));
}

/// With no movie host connected, the selection screen's movie is the one unavailable part it has: it
/// is reported on entry, its controls are refused (the buttons keep their state), it sends no request,
/// and the campaign does not mark the movie shown, since nothing plays. Connecting the host makes the
/// controls send requests instead (the test above).
#[test]
fn the_movie_is_unavailable_until_a_movie_host_connects() {
    use mc2_ui::logistics::mission_select::MissionSelect;
    use mc2_ui::logistics::sequence::{slot, Sequence};
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut pages = stub_pages();
    pages[slot::SELECT] = Box::new(MissionSelect::load(&mut ld).unwrap());
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    let movie = selection_movie(&mut seq).expect("the first mission names a movie");
    assert!(
        !lg.video_shown(),
        "nothing plays without a host, so the movie is not marked shown ({movie})"
    );
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Unavailable("mission movie playback"),
        "the movie is reported on entry"
    );
    assert!(seq.take_movie_requests().is_empty());

    let pause = page_button(&mut seq, slot::SELECT, ids::MN_MSG_PAUSE);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, pause);
    assert_eq!(flow, Flow::Unavailable("mission movie playback"));
    assert!(
        seq.take_movie_requests().is_empty(),
        "a refused Pause sends nothing"
    );
}

/// The movie the selection screen names, if any.
fn selection_movie(seq: &mut Sequence) -> Option<String> {
    use mc2_ui::logistics::mission_select::MissionSelect;
    let page = seq.page_mut(mc2_ui::logistics::sequence::slot::SELECT);
    page.as_any_mut()
        .downcast_mut::<MissionSelect>()
        .and_then(|select| select.movie.clone())
}

/// Whether a button of the selection screen is pressed.
fn selection_pressed(seq: &mut Sequence, id: i32) -> bool {
    seq.page_mut(mc2_ui::logistics::sequence::slot::SELECT)
        .base()
        .button(id)
        .is_some_and(|b| b.is_pressed())
}

/// The final video (`code/logistics.cpp:144-157`): a campaign that is not over asks for none; a campaign
/// over (its last group reached through a save) asks for the group's final video, which the game data holds,
/// and no video when the mission is a single-mission run.
#[test]
fn the_final_video_is_requested_once_the_campaign_is_over() {
    use mc2_ui::logistics::movie::{final_video_request, movie_vfs_path, MovieRequest};
    let Some(mut g) = game() else { return };
    let lg = campaign(&g);
    assert_eq!(final_video_request(&lg), None, "the campaign goes on");

    // the campaign's stage moved to its end: the last group is won, so the campaign is over
    let text = lg.save_to_string().expect("the campaign saves");
    let over = text.replacen(
        "current_stage: 0,",
        &format!("current_stage: {},", lg.mission_info().group_count()),
        1,
    );
    assert_ne!(over, text, "the save names its stage");
    let done = Logistics::load_from_str(Arc::clone(&g.data), &over)
        .expect("the campaign over loads")
        .logistics;
    assert!(done.campaign_over());
    // the first group's campaign file names this video (`FinalVideo`); pinned, not read back from the
    // getter the request is made from
    let name = "cinema5.bik";
    assert_eq!(done.mission_info().final_video(), name);
    assert_eq!(
        final_video_request(&done),
        Some(MovieRequest::FinalVideo(name.to_string()))
    );
    let ld = Loader::new(&mut g.vfs, &mut g.assets);
    assert!(
        ld.vfs.locate(&movie_vfs_path(name)).is_some(),
        "the requested video is in the game data: {name}"
    );
}

#[test]
fn the_tutorial_screen_id_reports_a_first_visit_once() {
    let Some(g) = game() else { return };
    let mut lg = campaign(&g);
    let mut visits = ScreenVisits::default();
    assert!(lg.first_time_purchase() && lg.first_time_mech_lab());
    // the first visit to the purchase screen (grid cell 2,0) reports its id, and leaving it clears
    // the campaign's flag
    assert_eq!(visits.screen_id((2, 0), &mut lg), 20);
    assert_eq!(
        visits.screen_id((2, 0), &mut lg),
        20,
        "still the first visit"
    );
    assert_eq!(visits.screen_id((2, 1), &mut lg), 21);
    assert!(!lg.first_time_purchase());
    // a repeat visit reports 0
    assert_eq!(visits.screen_id((2, 0), &mut lg), 0);
    assert_eq!(visits.screen_id((2, 1), &mut lg), 21);
    assert_eq!(visits.screen_id((2, 0), &mut lg), 0);
    // and the mech lab (2,2) the same way, in a session that never visited the purchase screen
    let mut visits = ScreenVisits::default();
    let mut lg = campaign(&g);
    assert_eq!(visits.screen_id((2, 2), &mut lg), 22);
    assert_eq!(visits.screen_id((2, 1), &mut lg), 21);
    assert!(!lg.first_time_mech_lab());
    assert_eq!(visits.screen_id((2, 2), &mut lg), 0);
    // the C++ `else if` in getCurrentScreenId: once the purchase screen was visited this session,
    // leaving the mech lab never clears its flag, so it keeps reporting 22 (ported as is)
    let mut visits = ScreenVisits::default();
    let mut lg = campaign(&g);
    assert_eq!(visits.screen_id((2, 0), &mut lg), 20);
    assert_eq!(visits.screen_id((2, 1), &mut lg), 21);
    assert_eq!(visits.screen_id((2, 2), &mut lg), 22);
    assert_eq!(visits.screen_id((2, 1), &mut lg), 21);
    assert!(lg.first_time_mech_lab());
    assert_eq!(visits.screen_id((2, 2), &mut lg), 22);
}

#[test]
fn a_callout_glides_the_pointer_to_a_button_and_flashes_it() {
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut briefing = Briefing::load(&mut ld).unwrap();
    briefing.begin_screen(&mut ld, &mut lg);
    let target = centre(&briefing.base, ids::MB_MSG_NEXT);
    let dt = 1.0 / 30.0;
    let mut input = UiInput::new(800, 600);
    let mut callout = Callout::default();

    assert!(callout.start(ids::MB_MSG_NEXT, true, 0.5, 2));
    assert!(
        !callout.start(ids::MB_MSG_NEXT, true, 0.5, 2),
        "one callout at a time"
    );
    // the first frame moves the pointer part of the way, and the callout is running
    input.begin_frame(dt);
    callout.update(&mut briefing.base, &mut input, dt);
    input.end_frame();
    assert!(callout.in_callout());
    let (x, y) = input.real_mouse_pos();
    assert!(
        x > 0.0 && x < target.0 && y > 0.0 && y < target.1,
        "gliding"
    );
    assert_eq!(
        input.mouse_state,
        mc2_ui::logistics::callout::M_STATE_TUTORIALS
    );

    // it runs out on its own: half a second of glide, then two flashes
    let mut frames = 1;
    while callout.in_callout() && frames < 300 {
        input.begin_frame(dt);
        callout.update(&mut briefing.base, &mut input, dt);
        input.end_frame();
        frames += 1;
    }
    assert!(!callout.in_callout(), "the flashes run out");
    assert!(frames > 15, "half a second of glide and two flashes");
    let (x, y) = input.real_mouse_pos();
    assert!(
        (x - target.0).abs() < 0.5 && (y - target.1).abs() < 0.5,
        "on the target"
    );
    let next = briefing.base.button(ids::MB_MSG_NEXT).expect("next button");
    assert_eq!(next.obj.loc[0].argb, 0xffff_ffff, "the button ends white");

    // a target that is not on the screen ends the callout at once
    assert!(callout.start(9999, true, 0.0, 0));
    input.begin_frame(dt);
    callout.update(&mut briefing.base, &mut input, dt);
    input.end_frame();
    assert!(!callout.in_callout());
}

#[test]
fn the_ready_room_lists_a_pilot_kept_on_a_mech_outside_the_force_group() {
    // `PilotReadyScreen::begin` (code/pilotreadyscreen.cpp:164-220) clears every pilot's use flag,
    // then marks the pilots of the force group's mechs. A pilot left on a mech that was taken out
    // of the force group is free again and must be listed.
    use mc2_ui::logistics::pilot_ready::PilotReady;
    let Some(mut g) = game() else { return };
    let mut lg = bay_with_a_mech(&mut g);
    let kept = lg.unassigned_pilots()[0];
    let first = lg.inventory()[0].key;
    let second = lg.inventory()[1].key;
    lg.set_mech_pilot(first, Some(kept));
    lg.remove_mech_from_force_group(first, false)
        .expect("out of the force group");
    // the pilot stays on the mech but its use flag is stale: the case the room must repair
    assert!(lg.pilots()[kept].used);

    // a second pilot, on a mech that stays in the force group, is not listed
    let busy = lg.unassigned_pilots()[0];
    lg.set_mech_pilot(second, Some(busy));
    lg.add_mech_to_force_group(second, 2).expect("force group");

    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    room.begin_screen(&mut ld, &mut lg);

    let listed: Vec<usize> = room.list.entries().map(|e| e.pilot()).collect();
    assert!(listed.contains(&kept), "the free pilot is listed");
    assert!(!listed.contains(&busy), "a pilot in the force group is not");
    assert!(lg.pilots()[busy].used, "its flag is rebuilt as used");
    assert!(
        !lg.pilots()[kept].used,
        "the pilot on a mech outside the force group is free"
    );
}

#[test]
fn options_open_over_the_current_screen_and_return_to_it() {
    // The options screen is reached from the main menu in the C++ (`MM_MSG_OPTIONS`,
    // code/mainmenu.cpp:332), not from the grid. The sequence shows it over the current screen,
    // holds the grid still while it is up, and resumes the grid when it reports Done.
    use mc2_ui::logistics::sequence::slot;
    use mc2_ui::logistics::Placeholder;
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let names = [
        "select", "brief", "bay", "pilot", "purchase", "lab", "loading", "options",
    ];
    let pages: Vec<Box<dyn Page>> = names
        .iter()
        .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>)
        .collect();
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(slot::SELECT));

    assert!(
        seq.open_options(&mut ld, &mut lg).is_ok(),
        "options open over the selection"
    );
    assert!(seq.in_options());
    assert!(!seq.open_options(&mut ld, &mut lg).is_ok(), "not twice");
    assert_eq!(
        seq.current_page(),
        Some(slot::SELECT),
        "the grid cell is unchanged"
    );

    // while the options are up, a grid page's status does not move the sequence
    seq.page_mut(slot::SELECT).set_status(Status::Next);
    let flow = seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    assert_eq!(flow, Flow::Running);
    assert!(seq.in_options());
    assert_eq!(seq.current_page(), Some(slot::SELECT));

    // the options report Done: the grid resumes, and its pending status moves it on
    seq.page_mut(slot::OPTIONS).set_status(Status::Done);
    seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    assert!(!seq.in_options(), "the options close");
    assert_eq!(
        seq.current_page(),
        Some(slot::SELECT),
        "the closing frame does not run the grid"
    );
    seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    assert_eq!(
        seq.current_page(),
        Some(slot::BRIEFING),
        "the grid resumes from the screen the options opened over"
    );

    // Previous closes the options too, and nothing is opened from an empty sequence
    let mut bare = Sequence::single_player(
        names
            .iter()
            .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>)
            .collect(),
    );
    assert!(
        !bare.open_options(&mut ld, &mut lg).is_ok(),
        "no screen has begun yet"
    );
    bare.begin(&mut ld, &mut lg);
    assert!(bare.open_options(&mut ld, &mut lg).is_ok());
    bare.page_mut(slot::OPTIONS).set_status(Status::Previous);
    seq_frame(&mut ld, &mut input, &mut bare, &mut lg, &[]);
    assert!(!bare.in_options());
    assert_eq!(bare.current_page(), Some(slot::SELECT));
}

#[test]
fn the_loadout_groups_the_mechs_components_by_range() {
    // `ComponentListBox::setComponents` (code/simplecomponentlistbox.cpp): each range band in
    // turn (short, medium, long, bandless), each component once with its count, then the sensor
    // and the ECM (`getSensorID`, `getECM`, logisticsvariant.cpp:189-209).
    use mc2_ui::logistics::loadout::loadout_rows;
    let Some(mut g) = game() else { return };
    let lg = bay_with_a_mech(&mut g);
    let mech = lg.inventory()[0].key;
    let variant = lg.mech(mech).expect("mech").variant;
    let placed = lg.variant(variant).expect("variant").components.len();
    let ld = Loader::new(&mut g.vfs, &mut g.assets);
    let rows = loadout_rows(&*ld.assets, &lg, variant);
    assert!(!rows.is_empty(), "a mech with components lists them");

    // every placed component is counted once, jump jets included
    let counted: usize = rows.iter().map(|r| r.count).sum();
    assert_eq!(counted, placed, "each placed component is counted");

    // the bands run in their order: short, medium, long, then the bandless and the sensor and ECM
    let bands = [0xff6e_7c00u32, 0xff00_5392, 0xffa2_1600, 0xffc2_9b00];
    let mut last = 0usize;
    for row in &rows {
        let band = bands
            .iter()
            .position(|&c| c == row.colour)
            .expect("a band colour");
        assert!(band >= last, "bands are listed in order");
        last = band;
    }

    // a counted component reads "N/N name"; a jump jet reads its name alone
    for row in rows.iter().filter(|r| r.count > 0) {
        let prefix = format!("{0}/{0} ", row.count);
        assert!(
            row.text.starts_with(&prefix) || !row.text.contains('/'),
            "{}",
            row.text
        );
    }
    let chassis = lg.chassis_of(variant).expect("chassis");
    if chassis.sensor_id() > 0 {
        let sensor = ld.assets.string(32000 + chassis.sensor_id() as u32);
        assert!(
            rows.iter().any(|r| r.text == sensor),
            "the sensor is listed"
        );
    }
}

#[test]
fn the_mech_screens_list_the_loadout_of_the_selected_mech() {
    // `MechBayScreen::setMech` (code/mechbayscreen.cpp:769) and `LogisticsMechDisplay::setMech`
    // (code/logisticsmechdisplay.cpp:151) hand the variant to the loadout list; a cleared
    // statistics panel empties it (`componentListBox.setMech(0)`, logisticsmechdisplay.cpp:163).
    use mc2_ui::logistics::loadout::loadout_rows;
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    let Some(mut g) = game() else { return };
    let mut lg = bay_with_a_mech(&mut g);
    let mech = lg.inventory()[0].key;
    let variant = lg.mech(mech).expect("mech").variant;
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let expected = loadout_rows(&*ld.assets, &lg, variant).len();
    assert!(expected > 0);

    // the mech bay: `begin` selects the first force group slot; the statistics (and the loadout)
    // follow the selection, as `update` does with the picked mech
    let mut bay = MechBay::load(&mut ld).unwrap();
    bay.begin_screen(&mut ld, &mut lg);
    bay.set_mech(&*ld.assets, &lg, Some(mech), false);
    assert_eq!(bay.cur_mech, Some(mech));
    assert_eq!(
        bay.loadout.list.item_count(),
        expected,
        "the bay lists the loadout"
    );
    bay.set_mech(&*ld.assets, &lg, None, false);
    assert_eq!(
        bay.loadout.list.item_count(),
        0,
        "a cleared bay empties the list"
    );

    // the ready room's mech display, shown for a slot's mech
    let mut room = PilotReady::load(&mut ld).unwrap();
    room.begin_screen(&mut ld, &mut lg);
    room.set_mech(&*ld.assets, &lg, Some(mech));
    assert_eq!(
        room.mech_display.loadout.list.item_count(),
        expected,
        "the display lists the loadout"
    );
    mc2_ui::logistics::Page::end(&mut room, &mut lg);
    assert_eq!(room.mech_display.loadout.list.item_count(), 0);
}

/// The picture a screen asked for, rendered: the screen draws once so it asks, the previews render
/// the queue, and the picture is stored under the request's key. Returns the picture's opaque
/// pixel count (zero when nothing of the model was drawn).
fn render_picture(
    ld: &mut Loader<'_>,
    previews: &mut mc2_ui::preview::ModelPreviews,
    req: &mc2_ui::preview::PreviewRequest,
) -> usize {
    previews.resolve(ld.assets);
    let tex = ld
        .assets
        .textures
        .find(&req.key())
        .expect("the picture is rendered and stored");
    let image = ld.assets.textures.get(tex).expect("image");
    image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] != 0)
        .count()
}

fn model_previews(root: &std::ffi::OsStr) -> Option<mc2_ui::preview::ModelPreviews> {
    use mc2_render::game::GameData;
    use mc2_render::gpu::Gpu;
    let gpu = match Gpu::headless(&mc2_gfx::config::RenderConfig::from_env()) {
        Ok(gpu) => gpu,
        Err(e)
            if std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1")
                && e.to_string().contains("adapter") =>
        {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            return None;
        }
        Err(e) => panic!("gpu: {e}"),
    };
    let data = GameData::open(root).expect("render data");
    Some(mc2_ui::preview::ModelPreviews::new(gpu, data).expect("previews"))
}

#[test]
fn the_mech_bay_and_the_display_draw_the_selected_mech() {
    // `MechBayScreen::render` draws `mechCamera` for the picked mech (code/mechbayscreen.cpp:250-252),
    // set by `setMech` with the chassis file (mechbayscreen.cpp:776-777). The ready room's display
    // does the same (logisticsmechdisplay.cpp:87, :148-149, :56-57).
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::Page;
    use mc2_ui::preview::shape_name;
    let Some(mut g) = game() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let Some(mut previews) = model_previews(&root) else {
        return;
    };
    let mut lg = bay_with_a_mech(&mut g);
    let mech = lg.inventory()[0].key;
    let chassis_file = lg
        .chassis_of(lg.mech(mech).expect("mech").variant)
        .expect("chassis")
        .file_name
        .clone();
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);

    let mut bay = MechBay::load(&mut ld).unwrap();
    bay.begin_screen(&mut ld, &mut lg);
    bay.set_mech(&*ld.assets, &lg, Some(mech), false);
    let req = bay
        .preview
        .clone()
        .expect("the bay asks for the mech's model");
    assert_eq!(req.shape, shape_name(&chassis_file));
    assert!(req.width > 0 && req.height > 0);
    {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets: &*ld.assets,
            dt: 0.0,
        };
        bay.render_screen(&mut ctx);
    }
    assert!(
        render_picture(&mut ld, &mut previews, &req) > 0,
        "the bay's picture shows the mech"
    );

    let mut room = PilotReady::load(&mut ld).unwrap();
    room.begin_screen(&mut ld, &mut lg);
    room.set_mech(&*ld.assets, &lg, Some(mech));
    let shown = room
        .mech_display
        .preview
        .clone()
        .expect("the display asks for the mech's model");
    assert_eq!(shown.shape, req.shape);
    {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets: &*ld.assets,
            dt: 0.0,
        };
        room.render_screen(&mut ctx);
    }
    assert!(render_picture(&mut ld, &mut previews, &shown) > 0);
    Page::end(&mut room, &mut lg);
    assert!(
        room.mech_display.preview.is_none(),
        "a cleared display shows no model"
    );
}

#[test]
fn the_briefing_draws_the_selected_objectives_model() {
    // `camera.setObject(objectiveModels[ID], ...)` and `setScale` on selecting an objective
    // (code/missionbriefingscreen.cpp:168-171, :236-238), drawn in the `camera.init` rectangle.
    use mc2_ui::logistics::briefing::Briefing;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let Some(mut previews) = model_previews(&root) else {
        return;
    };
    let mut lg = campaign(&g);
    let mut input = UiInput::new(800, 600);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut brief = Briefing::load(&mut ld).unwrap();
    Page::begin(&mut brief, &mut ld, &mut lg);

    // a player clicks the objective's marker: the list entry is selected inside the frame, and the
    // selection's focus shows the model
    let with_model = brief
        .models
        .iter()
        .position(|m| !m.name.is_empty())
        .expect("mission 1 has an objective with a model");
    let marker = &brief.objective_buttons[with_model];
    let at = (
        (marker.left() + marker.right()) as i32 / 2,
        (marker.top() + marker.bottom()) as i32 / 2,
    );
    click(
        &mut ld,
        &mut input,
        &mut lg,
        (at.0 as f32, at.1 as f32),
        |ctx, lg| brief.update(ctx, lg),
    );
    assert!(
        brief.preview.is_some(),
        "the focused objective shows its model"
    );
    let req = brief.preview.clone().expect("the model");
    {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets: &*ld.assets,
            dt: 0.0,
        };
        brief.render_screen(&mut ctx);
    }
    assert!(
        render_picture(&mut ld, &mut previews, &req) > 0,
        "the video panel shows the model"
    );
}

/// `MissionBriefingScreen::render` (`code/missionbriefingscreen.cpp:82-84`) draws the camera before
/// `LogisticsScreen::render`, so the screen's fade covers the objective's model as it covers the
/// rest of the screen. Headless: the draw list's quads show the model under the fade.
#[test]
fn the_briefing_model_is_drawn_under_the_fade() {
    use mc2_ui::logistics::briefing::Briefing;
    use mc2_ui::logistics::Page;
    let Some(mut g) = game() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let Some(mut previews) = model_previews(&root) else {
        return;
    };
    let mut lg = campaign(&g);
    let mut input = UiInput::new(800, 600);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut brief = Briefing::load(&mut ld).unwrap();
    Page::begin(&mut brief, &mut ld, &mut lg);
    let with_model = brief
        .models
        .iter()
        .position(|m| !m.name.is_empty())
        .expect("mission 1 has an objective with a model");
    let marker = &brief.objective_buttons[with_model];
    let at = (
        (marker.left() + marker.right()) / 2.0,
        (marker.top() + marker.bottom()) / 2.0,
    );
    click(&mut ld, &mut input, &mut lg, at, |ctx, lg| {
        brief.update(ctx, lg)
    });
    let req = brief
        .preview
        .clone()
        .expect("the focused objective shows its model");
    // the first frame asks for the picture; the picture is rendered and stored
    let mut dl = mc2_ui::draw::DrawList::new(800, 600);
    brief.render_screen(&mut mc2_ui::RenderCtx {
        dl: &mut dl,
        assets: &*ld.assets,
        dt: 0.0,
    });
    previews.resolve(ld.assets);
    let tex = mc2_ui::preview::preview_texture(&*ld.assets, &req).expect("the model's texture");

    // a fade-in still at its start: the whole screen is covered
    brief.begin_fade_in(1.0);
    let mut dl = mc2_ui::draw::DrawList::new(800, 600);
    brief.render_screen(&mut mc2_ui::RenderCtx {
        dl: &mut dl,
        assets: &*ld.assets,
        dt: 0.0,
    });
    let mut quads = Vec::new();
    for b in &dl.batches {
        for q in b.first_quad..b.first_quad + b.quad_count {
            quads.push((b.texture, q));
        }
    }
    let model: Vec<usize> = quads
        .iter()
        .filter(|(t, _)| *t == Some(tex))
        .map(|(_, q)| *q as usize)
        .collect();
    assert!(!model.is_empty(), "the model is drawn");
    let fade = quads
        .iter()
        .find(|(t, q)| {
            let v = &dl.vertices[*q as usize * 4..*q as usize * 4 + 4];
            t.is_none() && v[0].x == 0.0 && v[0].y == 0.0 && v[2].x == 800.0 && v[2].y == 600.0
        })
        .map(|(_, q)| *q as usize)
        .expect("the fade covers the screen");
    assert!(
        model.iter().all(|&m| m < fade),
        "the model (quads {model:?}) is drawn before the fade (quad {fade}), as the camera is in the C++"
    );
}

#[test]
fn the_mech_bay_draws_the_loadout_and_the_mech_picture() {
    // `MechBayScreen::render` draws `loadoutListBox` and `mechCamera` for the picked mech
    // (code/mechbayscreen.cpp:250-252); a cleared bay draws neither.
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    let Some(mut g) = game() else { return };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let Some(mut previews) = model_previews(&root) else {
        return;
    };
    let mut lg = bay_with_a_mech(&mut g);
    let mech = lg.inventory()[0].key;
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut bay = MechBay::load(&mut ld).unwrap();
    bay.begin_screen(&mut ld, &mut lg);

    let draw = |bay: &mut MechBay, assets: &UiAssets| {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets,
            dt: 0.0,
        };
        bay.render_screen(&mut ctx);
        dl
    };
    bay.set_mech(&*ld.assets, &lg, None, false);
    let empty = draw(&mut bay, &*ld.assets).quad_count();
    bay.set_mech(&*ld.assets, &lg, Some(mech), false);
    let listed = draw(&mut bay, &*ld.assets).quad_count();
    assert!(listed > empty, "the loadout's rows are drawn");

    // the first draw queues the picture; once resolved, the next draw shows it
    let req = bay.preview.clone().expect("the model");
    let _ = draw(&mut bay, &*ld.assets);
    previews.resolve(ld.assets);
    let tex = ld.assets.textures.find(&req.key()).expect("picture");
    let dl = draw(&mut bay, &*ld.assets);
    assert!(
        dl.batches.iter().any(|b| b.texture == Some(tex)),
        "the second draw shows the mech's picture"
    );

    // the ready room's display: its loadout and picture are drawn with the room (`PilotReadyScreen`
    // renders `mechDisplay`, logisticsmechdisplay.cpp:87)
    let mut room = PilotReady::load(&mut ld).unwrap();
    room.begin_screen(&mut ld, &mut lg);
    let room_draw = |room: &mut PilotReady, assets: &UiAssets| {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets,
            dt: 0.0,
        };
        room.render_screen(&mut ctx);
        dl.quad_count()
    };
    room.set_mech(&*ld.assets, &lg, None);
    let bare_room = room_draw(&mut room, &*ld.assets);
    room.set_mech(&*ld.assets, &lg, Some(mech));
    assert!(
        room_draw(&mut room, &*ld.assets) > bare_room,
        "the ready room's display draws the loadout"
    );
}

/// [`bay_with_a_mech`] in a single-mission game on mission one's map, which plays purchasing.
/// Mission one of the campaign skips it (its BUY and CHANGE LOADOUT buttons are disabled, as in
/// the C++), so the purchase and mech lab hooks are reached this way.
fn purchasing_bay(g: &mut Game) -> Logistics {
    use mc2_sim_data::campaign::MissionSettings;
    let mut lg = campaign(g);
    lg.set_single_mission("mc2_01", &MissionSettings::default(), None);
    assert!(!lg.skip_purchasing(), "the mission plays purchasing");
    let mut variants = lg.purchasable_mechs();
    variants.sort_by_key(|v| lg.variant_cost(*v));
    lg.add_c_bills(200_000);
    lg.purchase_mech(variants[0]).expect("purchase");
    lg.purchase_mech(variants[0]).expect("purchase");
    let first = lg.inventory()[0].key;
    lg.add_mech_to_force_group(first, 1).expect("force group");
    lg
}

/// The eight pages of a sequence: the stand-in `Placeholder::new` for each, which runs until its
/// status is set and reads no files. A test replaces the pages it is about.
fn stub_pages() -> Vec<Box<dyn mc2_ui::logistics::Page>> {
    use mc2_ui::logistics::Placeholder;
    [
        "select", "brief", "bay", "pilot", "purchase", "lab", "loading", "options",
    ]
    .iter()
    .map(|n| Box::new(Placeholder::new(n)) as Box<dyn Page>)
    .collect()
}

/// A scratch data root for the fault tests, removed when dropped.
struct TempRoot(std::path::PathBuf);

impl TempRoot {
    /// Each call gets its own directory. Tests in one process run in parallel, so two tests with the
    /// same tag must not share a root, or one test's drop deletes the files the other is writing.
    fn new(tag: &str) -> TempRoot {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("mc2-u2a-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch data root");
        TempRoot(dir)
    }

    /// A VFS over the scratch root alone: no archives, so every file the screens read is missing.
    fn vfs(&self) -> Vfs {
        Vfs::new(&self.0)
    }

    /// Writes `text` as the loose file `rel` under the root.
    fn write(&self, rel: &str, text: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
        std::fs::write(path, text).expect("a file");
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_selection_branches_and_a_pick_sets_the_mission() {
    // After the fifth win the operation branches: mc2_06, mc2_07 and mc2_08 are the choices
    // (the campaign's `currentMissions`). Each choice is a button of the operation map; a player's
    // press on one sets the campaign's mission (missionselectionscreen.cpp, the radio group).
    use mc2_campaign::interface::Outcome;
    use mc2_ui::logistics::mission_select::MissionSelect;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    for _ in 0..5 {
        lg.set_mission_completed(&Outcome::won())
            .expect("completed");
    }
    assert_eq!(lg.current_missions(), ["mc2_06", "mc2_07", "mc2_08"]);

    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut select = MissionSelect::load(&mut ld).unwrap();
    let mut input = UiInput::new(800, 600);
    select.begin_screen(&mut ld, &mut lg).unwrap();
    let shown = select
        .operation
        .buttons
        .iter()
        .filter(|b| b.obj.show)
        .count();
    assert_eq!(shown, 3, "three branches are offered");

    let b = &select.operation.buttons[1];
    let at = (
        b.obj.global_x() as f32 + b.obj.width() / 2.0,
        b.obj.global_y() as f32 + b.obj.height() / 2.0,
    );
    click(&mut ld, &mut input, &mut lg, at, |ctx, lg| {
        select.update_screen(ctx, lg)
    });
    assert_eq!(
        lg.current_mission(),
        "mc2_07",
        "the pick is the campaign's mission"
    );
    assert_eq!(select.pressed_mission(), Some("mc2_07"));
}

#[test]
fn a_faulting_options_screen_is_reported_and_stops_the_sequence() {
    // The options open over the grid (the C++ options wrapper, code/mainmenu.cpp:332). A layout
    // that fails is the sequence's failure, as a grid screen's is: the open reports the error, and
    // a fault while the options run stops the sequence rather than leaving it running.
    use mc2_ui::logistics::sequence::slot;
    use mc2_ui::logistics::Placeholder;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut input = UiInput::new(800, 600);

    // the options fail to begin: the open reports the fault, and the next update stops
    let mut pages = stub_pages();
    pages[slot::OPTIONS] = Box::new(Placeholder::failing(
        "options",
        "mcl_options.fit: no such layout",
    ));
    let mut seq = Sequence::single_player(pages);
    seq.begin(&mut ld, &mut lg);
    let err = seq
        .open_options(&mut ld, &mut lg)
        .expect_err("the options do not open");
    assert!(
        err.contains("mcl_options.fit"),
        "the reason is reported: {err}"
    );
    assert_eq!(seq.fault(), Some(err.as_str()));
    assert!(!seq.in_options());
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );

    // the options open, then fault while they run: the sequence stops, and stays stopped
    let mut seq = Sequence::single_player(stub_pages());
    seq.begin(&mut ld, &mut lg);
    seq.open_options(&mut ld, &mut lg)
        .expect("the options open");
    seq.page_mut(slot::OPTIONS)
        .as_any_mut()
        .downcast_mut::<Placeholder>()
        .expect("the options placeholder")
        .fail("mcl_options.fit: bad button");
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
    assert_eq!(seq.fault(), Some("mcl_options.fit: bad button"));
    assert!(!seq.in_options(), "the faulting options close");
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
}

#[test]
fn the_unwired_screens_report_unavailable_through_the_included_screens() {
    // The purchase screen is reached by the mech bay's BUY (`MechBayScreen::handleMessage`,
    // code/mechbayscreen.cpp, status UP), the mech lab by CHANGE_LOADOUT (status DOWN), the
    // loading screen by the ready room's NEXT (`PilotReadyScreen`), and the options from the main
    // menu. None of them is wired up here: each is reported, and the sequence does not move onto it.
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::logistics::sequence::{slot, Sequence};
    use mc2_ui::logistics::{ids, Placeholder};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = purchasing_bay(&mut g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut pages = stub_pages();
    pages[slot::SELECT] = Box::new(MissionSelect::load(&mut ld).unwrap());
    pages[slot::BRIEFING] = Box::new(Briefing::load(&mut ld).unwrap());
    pages[slot::MECH_BAY] = Box::new(MechBay::load(&mut ld).unwrap());
    pages[slot::PILOT_READY] = Box::new(PilotReady::load(&mut ld).unwrap());
    pages[slot::PURCHASE] = Box::new(Placeholder::unavailable("purchase"));
    pages[slot::MECH_LAB] = Box::new(Placeholder::unavailable("mech lab"));
    pages[slot::LOADING] = Box::new(Placeholder::unavailable("loading"));
    pages[slot::OPTIONS] = Box::new(Placeholder::unavailable("options"));
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.skip_to_briefing(&mut ld, &mut lg);
    assert_eq!(seq.current_page(), Some(slot::BRIEFING));

    let next = page_button(&mut seq, slot::BRIEFING, ids::MB_MSG_NEXT);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY));

    // BUY: the purchase screen is not wired up; the bay stays, and its status is cleared
    let buy = page_button(&mut seq, slot::MECH_BAY, ids::MB_MSG_BUY);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, buy);
    assert_eq!(flow, Flow::Unavailable("purchase"));
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY));
    assert_eq!(
        seq.page_mut(slot::MECH_BAY).status(),
        Status::Running,
        "the refused move is not asked for again"
    );

    // CHANGE LOADOUT: the mech lab is not wired up either. The refused move starts no edit: the
    // mech keeps its variant, no edit is open, and the campaign saves as it was
    // (`MechBayScreen::handleMessage` hands the mech to the lab, code/mechbayscreen.cpp:641-646,
    // and the lab is not reached here)
    let cur = seq
        .page_mut(slot::MECH_BAY)
        .as_any_mut()
        .downcast_mut::<MechBay>()
        .expect("the mech bay")
        .cur_mech
        .expect("a mech is selected to change");
    let variant = lg.mech(cur).expect("mech").variant;
    let saved = lg.save_to_string().expect("save");
    let loadout = page_button(&mut seq, slot::MECH_BAY, ids::MB_MSG_CHANGE_LOADOUT);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, loadout);
    assert_eq!(flow, Flow::Unavailable("mech lab"));
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY));
    assert_eq!(lg.mech(cur).expect("mech").variant, variant, "same variant");
    assert!(lg.mech_to_modify().is_none(), "no edit is open");
    assert_eq!(
        lg.save_to_string().expect("save"),
        saved,
        "the save is unchanged"
    );

    // NEXT to the ready room, crew the slot, then NEXT: the loading screen is not wired up
    let next = page_button(&mut seq, slot::MECH_BAY, ids::MB_MSG_NEXT);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(seq.current_page(), Some(slot::PILOT_READY));
    assert!(
        lg.mech_to_modify().is_none(),
        "leaving the bay after a refused change starts no edit"
    );
    let add = page_button(&mut seq, slot::PILOT_READY, ids::PR_MSG_ADD);
    seq_click(&mut ld, &mut input, &mut seq, &mut lg, add);
    let next = page_button(&mut seq, slot::PILOT_READY, ids::MB_MSG_NEXT);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, next);
    assert_eq!(flow, Flow::Unavailable("loading"));
    assert_eq!(seq.current_page(), Some(slot::PILOT_READY));

    // the options cannot open over the screen either
    assert!(
        seq.open_options(&mut ld, &mut lg).is_err(),
        "the options are not available"
    );
    assert!(!seq.in_options());
}

#[test]
fn the_mech_lab_starts_the_edit_when_the_bay_hands_over() {
    // The edit starts when the bay is left for the mech lab, which is what the C++
    // `setMechToModify` on the click (code/mechbayscreen.cpp:641-646) comes to: the lab is
    // reached with the mech to modify already set.
    use mc2_ui::logistics::mech_bay::MechBay;
    use mc2_ui::logistics::sequence::slot;
    use mc2_ui::logistics::Placeholder;
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = purchasing_bay(&mut g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut pages = stub_pages();
    pages[slot::MECH_BAY] = Box::new(MechBay::load(&mut ld).unwrap());
    pages[slot::MECH_LAB] = Box::new(Placeholder::new("lab"));
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    seq.skip_to_briefing(&mut ld, &mut lg);
    seq.page_mut(slot::BRIEFING).set_status(Status::Next);
    seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    assert_eq!(seq.current_page(), Some(slot::MECH_BAY));
    // the bay's first update picks the first mech of the force group
    seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);

    let cur = seq
        .page_mut(slot::MECH_BAY)
        .as_any_mut()
        .downcast_mut::<MechBay>()
        .expect("the mech bay")
        .cur_mech
        .expect("a mech is selected to change");
    let original = lg.mech(cur).expect("mech").variant;
    let loadout = page_button(&mut seq, slot::MECH_BAY, ids::MB_MSG_CHANGE_LOADOUT);
    let flow = seq_click(&mut ld, &mut input, &mut seq, &mut lg, loadout);
    assert_eq!(flow, Flow::Running);
    assert_eq!(seq.current_page(), Some(slot::MECH_LAB));
    assert_eq!(
        lg.mech_to_modify(),
        Some(cur),
        "the lab edits the selected mech"
    );
    assert_ne!(
        lg.mech(cur).expect("mech").variant,
        original,
        "the mech works on an editing copy"
    );
    assert_eq!(lg.original_variant(), Some(original));
}

#[test]
fn a_missing_operation_layout_stops_the_sequence_on_selection() {
    // `MissionSelectionScreen::begin` reads the operation layout on entry (code/missionselectionscreen.cpp:208-223).
    // Without it the selection cannot run, so the sequence reports the failure and does not move on.
    use mc2_ui::logistics::mission_select::MissionSelect;
    use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let scratch = TempRoot::new("no-operation");
    let mut pages = stub_pages();
    pages[slot::SELECT] = {
        let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
        Box::new(MissionSelect::load(&mut ld).unwrap())
    };
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    let mut vfs = scratch.vfs();
    let mut ld = Loader::new(&mut vfs, &mut g.assets);
    seq.begin(&mut ld, &mut lg);
    assert!(
        seq.fault()
            .is_some_and(|f| f.starts_with("mission selection:")),
        "the missing layout is reported: {:?}",
        seq.fault()
    );
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );

    seq.page_mut(slot::SELECT).set_status(Status::Next);
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
    assert_eq!(seq.current_page(), Some(slot::SELECT), "no progress");
}

#[test]
fn a_missing_mission_file_stops_the_sequence_on_the_briefing() {
    // `MissionBriefingScreen::begin` reads the mission's file (code/missionbriefingscreen.cpp);
    // without it there is no objective list to brief from, so the sequence stops there.
    use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let scratch = TempRoot::new("no-mission");
    let mut pages = stub_pages();
    pages[slot::BRIEFING] = {
        let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
        Box::new(Briefing::load(&mut ld).unwrap())
    };
    let mut seq = Sequence::single_player(pages);
    let mut input = UiInput::new(800, 600);
    let mut vfs = scratch.vfs();
    let mut ld = Loader::new(&mut vfs, &mut g.assets);
    seq.skip_to_briefing(&mut ld, &mut lg);
    assert!(
        seq.fault()
            .is_some_and(|f| f.starts_with("briefing: data/missions/")),
        "the missing mission is reported: {:?}",
        seq.fault()
    );
    seq.page_mut(slot::BRIEFING).set_status(Status::Next);
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
    assert_eq!(seq.current_page(), Some(slot::BRIEFING));
}

#[test]
fn a_tutorial_script_that_does_not_load_stops_the_sequence() {
    // `MissionBegin::init` loads the mission's script; the C++ asserts when it cannot be prepared,
    // so the sequence stops rather than running without the tutorial's behaviour.
    use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    assert_eq!(
        lg.mission_info().current_abl_script_name(),
        Some("Tutorial0")
    );
    let scratch = TempRoot::new("corrupt-script");
    scratch.write(
        "data/missions/tutorial0.abl",
        "module t : integer;\nthis is not ABL\n",
    );
    let mut vfs = scratch.vfs();
    let mut ld = Loader::new(&mut vfs, &mut g.assets);
    let mut seq = Sequence::single_player(stub_pages());
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    assert!(!seq.has_brain(), "the script did not load");
    assert!(
        seq.fault().is_some_and(|f| f.contains("tutorial0.abl")),
        "the script's error is reported: {:?}",
        seq.fault()
    );
    seq.page_mut(slot::SELECT).set_status(Status::Next);
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
    assert_eq!(seq.current_page(), Some(slot::SELECT));
}

/// A tutorial brain that counts its runs in a module variable and fails on the third. The fault
/// only comes if the brain keeps its state: a reloaded brain starts its count from one again.
const COUNTING_BRAIN: &str = "module t : integer;\nvar\n static integer runs;\ncode\nruns = runs + 1;\nif (runs == 3) then fatal(7, \"boom\"); endif;\nreturn(0);\nendmodule.\n";

/// Begins the sequence on the counting brain, runs two frames, then hands the sequence to
/// `transition` and runs one more frame. That frame is the brain's third run.
fn counting_brain_across(
    transition: impl FnOnce(&mut Sequence, &mut Loader<'_>, &mut Logistics),
) -> Option<(Flow, Option<String>)> {
    use mc2_ui::logistics::sequence::Flow;
    let mut g = game()?;
    let mut lg = campaign(&g);
    let scratch = TempRoot::new("counting-script");
    scratch.write("data/missions/tutorial0.abl", COUNTING_BRAIN);
    let mut vfs = scratch.vfs();
    let mut ld = Loader::new(&mut vfs, &mut g.assets);
    let mut seq = Sequence::single_player(stub_pages());
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    for _ in 0..2 {
        assert_eq!(
            seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
            Flow::Running
        );
    }
    assert!(
        seq.fault().is_none(),
        "two runs are fine: {:?}",
        seq.fault()
    );
    transition(&mut seq, &mut ld, &mut lg);
    let flow = seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]);
    Some((flow, seq.fault().map(str::to_string)))
}

/// `MissionBegin::update`'s main-menu restart (`code/missionbegin.cpp`, the `RESTART` branch)
/// switches screens and never calls `MissionBegin::begin`, which is the only place the C++ loads
/// `logisticsBrain`. The running brain keeps its module state, so its third run is the fault.
#[test]
fn a_restart_keeps_the_running_tutorial_brain() {
    let Some((flow, fault)) = counting_brain_across(|seq, ld, lg| seq.restart(ld, lg)) else {
        return;
    };
    assert_eq!(
        flow,
        Flow::Failed,
        "the brain's third run fails: fault {fault:?}"
    );
    assert!(fault.is_some_and(|f| f.contains("boom")));
}

/// `MissionBegin::setToMissionBriefing` (`code/missionbegin.cpp:1139`) switches to the briefing and
/// never reloads `logisticsBrain`, so the count carries on across the skip too.
#[test]
fn a_skip_to_the_briefing_keeps_the_running_tutorial_brain() {
    let Some((flow, fault)) = counting_brain_across(|seq, ld, lg| seq.skip_to_briefing(ld, lg))
    else {
        return;
    };
    assert_eq!(
        flow,
        Flow::Failed,
        "the brain's third run fails: fault {fault:?}"
    );
    assert!(fault.is_some_and(|f| f.contains("boom")));
}

#[test]
fn a_tutorial_script_that_fails_while_running_stops_the_sequence() {
    // `logisticsBrain->execute()` runs each frame; an error there stops the sequence too. The
    // script below loads and then calls `fatal` on its first run.
    use mc2_ui::logistics::sequence::{slot, Flow, Sequence};
    use mc2_ui::screen::Status;
    let Some(mut g) = game() else { return };
    let mut lg = campaign(&g);
    let scratch = TempRoot::new("failing-script");
    scratch.write(
        "data/missions/tutorial0.abl",
        "module t : integer;\nvar\n\ncode\nfatal(7, \"boom\");\nreturn(0);\nendmodule.\n",
    );
    let mut vfs = scratch.vfs();
    let mut ld = Loader::new(&mut vfs, &mut g.assets);
    let mut seq = Sequence::single_player(stub_pages());
    let mut input = UiInput::new(800, 600);
    seq.begin(&mut ld, &mut lg);
    assert!(seq.has_brain(), "the script loads");
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
    assert!(
        seq.fault().is_some_and(|f| f.contains("boom")),
        "{:?}",
        seq.fault()
    );
    assert!(!seq.has_brain());
    seq.page_mut(slot::SELECT).set_status(Status::Next);
    assert_eq!(
        seq_frame(&mut ld, &mut input, &mut seq, &mut lg, &[]),
        Flow::Failed
    );
    assert_eq!(seq.current_page(), Some(slot::SELECT));
}

/// The data root a test replaces mission files in: a mirror of the shipped root with `files`
/// written as loose files on top. The VFS reads loose files before the archives, so a written
/// mission file replaces the shipped one. The VFS refuses links that leave its root, so the mirror
/// holds hard links to the shipped files (copies where a link cannot be made, across volumes).
/// The mirror is removed on drop.
struct Overlay(std::path::PathBuf);

impl Overlay {
    fn new(tag: &str, files: &[(&str, String)]) -> Overlay {
        let real = std::path::PathBuf::from(std::env::var_os("MC2_DATA").expect("MC2_DATA"));
        assert!(
            !real.join("data/missions").exists(),
            "the overlay writes the loose missions itself"
        );
        let root = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("overlay-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        let overlay = Overlay(root.clone());
        mirror(&real, &root);
        for (rel, text) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().expect("a directory")).expect("overlay dir");
            std::fs::write(path, text).expect("overlay file");
        }
        overlay
    }

    fn vfs(&self) -> Vfs {
        Vfs::from_system_cfg(&self.0).expect("overlay vfs")
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Mirrors the tree `from` into `to`: directories are made, and each file is hard-linked (copied
/// where a link cannot be made).
fn mirror(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mirror dir");
    for entry in std::fs::read_dir(from).expect("shipped data") {
        let entry = entry.expect("entry");
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type().expect("file type").is_dir() {
            mirror(&src, &dst);
        } else if std::fs::hard_link(&src, &dst).is_err() {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

/// The shipped file `rel`, as text.
fn shipped_text(rel: &str) -> String {
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(root).expect("vfs");
    let bytes = vfs.read(rel).expect("read").expect("the file is shipped");
    String::from_utf8_lossy(&bytes).into_owned()
}

/// [`game`] over `vfs`.
fn game_on(mut vfs: Vfs) -> Game {
    let data = Arc::new(GameData::load(&mut vfs).expect("game data"));
    Game {
        vfs,
        assets: UiAssets::default(),
        data,
    }
}

/// The briefing's map static (`MAP_INDEX`).
const BRIEFING_MAP: usize = 32;

#[test]
fn a_model_the_mission_file_names_that_does_not_load_shows_the_no_model_static() {
    // The objective's model is the mission file's `ObjectiveModel` (objective.rs reads it). Mission
    // one's first objective names "Hangar"; here the mission file names a model that cannot be
    // drawn. The briefing shows the "no model" static, as for an objective with no model
    // (`camera.setObject`, code/missionbriefingscreen.cpp:168-177).
    use mc2_ui::logistics::briefing::Briefing;
    use mc2_ui::logistics::Page;
    use mc2_ui::preview::preview_failed;
    let Some(root) = std::env::var_os("MC2_DATA") else {
        return;
    };
    let original = shipped_text("data/missions/mc2_01.fit");
    let edited = original.replacen(
        "st ObjectiveModel = \"Hangar\"",
        "st ObjectiveModel = \"no_such_model\"",
        1,
    );
    assert_ne!(edited, original, "mission one names the Hangar model");
    let overlay = Overlay::new("failed-model", &[("data/missions/mc2_01.fit", edited)]);
    let mut g = game_on(overlay.vfs());
    let Some(mut previews) = model_previews(&root) else {
        return;
    };
    let mut lg = campaign(&g);
    let mut input = UiInput::new(800, 600);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut brief = Briefing::load(&mut ld).unwrap();
    Page::begin(&mut brief, &mut ld, &mut lg);

    let with_model = brief
        .models
        .iter()
        .position(|m| m.name == "no_such_model")
        .expect("the mission file names the model");
    let marker = &brief.objective_buttons[with_model];
    let at = (
        (marker.left() + marker.right()) as i32 / 2,
        (marker.top() + marker.bottom()) as i32 / 2,
    );
    click(
        &mut ld,
        &mut input,
        &mut lg,
        (at.0 as f32, at.1 as f32),
        |ctx, lg| brief.update(ctx, lg),
    );
    let req = brief
        .preview
        .clone()
        .expect("the focused objective asks for its model");
    assert!(
        !brief.shows_no_model(),
        "the model is requested, not yet failed"
    );

    let draw = |brief: &mut Briefing, assets: &UiAssets| {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets,
            dt: 0.0,
        };
        brief.render_screen(&mut ctx);
        dl
    };
    draw(&mut brief, &*ld.assets);
    previews.resolve(ld.assets);
    assert!(preview_failed(&*ld.assets, &req), "the render failed");

    let second = draw(&mut brief, &*ld.assets);
    assert!(
        brief.shows_no_model(),
        "the fallback static shows on the second draw"
    );
    assert!(
        ld.assets.textures.find(&req.key()).is_none(),
        "no picture is stored for the failed model"
    );
    assert!(!second.batches.is_empty(), "the panel's statics are drawn");

    // the failure is not retried: a later draw asks for nothing new
    draw(&mut brief, &*ld.assets);
    assert!(brief.shows_no_model());
}

#[test]
fn the_briefing_forgets_the_last_missions_model_and_map() {
    // `MissionBriefingScreen::begin` sets the map from the mission's picture (code/missionbriefingscreen.cpp:
    // 344-347) and `end` clears the camera (:638). A revisit for another mission therefore shows
    // neither the last mission's model nor its map. Mission B is mission one's file under another
    // name, with no map packet: the shipped archives have none for it.
    use mc2_sim_data::campaign::MissionSettings;
    use mc2_ui::logistics::briefing::Briefing;
    use mc2_ui::logistics::Page;
    let Some(root) = std::env::var_os("MC2_DATA") else {
        return;
    };
    let overlay = Overlay::new(
        "revisit",
        &[(
            "data/missions/u2a_b.fit",
            shipped_text("data/missions/mc2_01.fit"),
        )],
    );
    let mut g = game_on(overlay.vfs());
    let Some(mut previews) = model_previews(&root) else {
        return;
    };
    let mut lg = campaign(&g);
    let mut input = UiInput::new(800, 600);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut brief = Briefing::load(&mut ld).unwrap();
    Page::begin(&mut brief, &mut ld, &mut lg);

    // mission one: an objective with a model is selected, and the map is shown
    let with_model = brief
        .models
        .iter()
        .position(|m| !m.name.is_empty())
        .expect("mission 1 has an objective with a model");
    let marker = &brief.objective_buttons[with_model];
    let at = (
        (marker.left() + marker.right()) as i32 / 2,
        (marker.top() + marker.bottom()) as i32 / 2,
    );
    click(
        &mut ld,
        &mut input,
        &mut lg,
        (at.0 as f32, at.1 as f32),
        |ctx, lg| brief.update(ctx, lg),
    );
    let req = brief.preview.clone().expect("the model");
    let draw = |brief: &mut Briefing, assets: &UiAssets| {
        let mut dl = mc2_ui::draw::DrawList::new(800, 600);
        let mut ctx = mc2_ui::RenderCtx {
            dl: &mut dl,
            assets,
            dt: 0.0,
        };
        brief.render_screen(&mut ctx);
        dl
    };
    draw(&mut brief, &*ld.assets);
    previews.resolve(ld.assets);
    let model_tex = ld.assets.textures.find(&req.key()).expect("the picture");
    let map_tex = brief.base.statics[BRIEFING_MAP]
        .texture
        .expect("mission one's map is shown");

    // mission B: no objective is selected, and its map packet is missing
    Page::end(&mut brief, &mut lg);
    lg.set_single_mission("u2a_b", &MissionSettings::default(), None);
    Page::begin(&mut brief, &mut ld, &mut lg);
    assert!(brief.preview.is_none(), "no model is selected");
    assert!(!brief.shows_no_model(), "the video panel is clear");
    assert_eq!(
        brief.base.statics[BRIEFING_MAP].texture, None,
        "the missing map is the zero texture"
    );
    let dl = draw(&mut brief, &*ld.assets);
    assert!(
        dl.batches
            .iter()
            .all(|b| b.texture != Some(model_tex) && b.texture != Some(map_tex)),
        "neither the last mission's model nor its map is drawn"
    );
}

/// `BoxOutline` of `mcl_gn_deploymentteams.fit` is a bitmap, not a rect: the slot of a mech with
/// no pilot must not draw a filled block in the animation colour.
#[test]
fn a_deployment_slot_outline_is_a_bitmap_not_a_filled_rect() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        return;
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(root).expect("vfs");
    let mut h = mc2_ui::harness::Harness::new();
    let mut ld = mc2_ui::Loader::new(&mut vfs, &mut h.assets);
    let mut file = ld.art_fit("mcl_gn_deploymentteams").expect("fit");
    let icon = mc2_ui::logistics::mech_icon::LogisticsMechIcon::load(&mut ld, &mut file);
    assert_eq!(
        (icon.outline.width(), icon.outline.height()),
        (75.0, 64.0),
        "the box bitmap's size"
    );
    assert!(icon.outline.texture.is_some(), "the box is texture backed");
}

/// The specialty text of a populated Pilot Ready panel (`mcl_pr_layout.fit` `Text9`..): it is
/// right aligned in its own rect, which ends at the skill icon, so the label stays inside it and
/// clear of the icon (the `PilotSpecialtySkill*` rect is the 31x31 icon box, not the label's).
#[test]
fn a_pilots_specialty_labels_stay_inside_their_rects_beside_the_icons() {
    use mc2_ui::logistics::pilot_ready::PilotReady;
    use mc2_ui::widgets::basic::font_width;
    let Some(mut g) = game() else { return };
    let lg = campaign(&g);
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let mut room = PilotReady::load(&mut ld).unwrap();
    let (i, _) = lg
        .pilots()
        .iter()
        .enumerate()
        .find(|(_, p)| !p.specialty_skill_indices().is_empty())
        .expect("a pilot with a specialty");
    room.set_pilot(ld.assets, &lg, Some(i));
    let mut shown = 0;
    for k in 0..4 {
        let t = &room.base.texts[9 + k];
        if t.text.is_empty() {
            continue;
        }
        shown += 1;
        let rect = t.obj.global_rect();
        let w = font_width(ld.assets, t.font, &t.text);
        assert!(w <= rect.right - rect.left, "{:?} fits its rect", t.text);
        assert!(
            rect.right <= room.skill_locations[k].left,
            "the label ends before its icon"
        );
    }
    assert!(shown > 0);
}

/// Mission one's briefing map is drawn as the C++ has it (`missionbriefingscreen.cpp:314,346`):
/// the packet-3 TGA's scanlines reversed, then the `setUVs(0,127,127,0)` flip. Pins the texture's
/// first and last row so the orientation cannot silently flip back.
#[test]
fn briefing_map_keeps_the_cpp_orientation() {
    let Some(mut g) = game() else { return };
    let mut ld = Loader::new(&mut g.vfs, &mut g.assets);
    let (tex, _) = mc2_ui::logistics::briefing::load_map(&mut ld, "mc2_01").expect("map");
    let img = ld.assets.textures.get(tex).unwrap();
    let w = img.width as usize;
    let px = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * w + x) * 4;
        img.rgba[i..i + 4].try_into().unwrap()
    };
    let last = img.height as usize - 1;
    assert_eq!((w, img.height), (128, 128));
    // the file stores these two corners the other way round (top-origin TGA, descriptor 0x20)
    assert_eq!(px(0, 0), [92, 88, 47, 255]);
    assert_eq!(px(0, last), [162, 136, 91, 255]);
}
