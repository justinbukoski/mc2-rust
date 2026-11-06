//! Widget behaviour without game data: hover, press and release, toggles, list selection and
//! scrolling, text entry.

use mc2_platform::keys;
use mc2_ui::ctx::{msg, sfx, Msg};
use mc2_ui::harness::{events::*, Harness};
use mc2_ui::widgets::{
    AnimButton, Button, ButtonState, Edit, ItemState, ListBox, Object, TextListItem,
};

fn button(id: i32) -> AnimButton {
    let mut b = AnimButton::new(Button::new(100.0, 100.0, 200.0, 30.0));
    b.set_id(id);
    b.obj.help_id = 1234;
    b
}

/// Lets the double-click clock run out so the next press is a plain click.
fn settle(h: &mut Harness, b: &mut AnimButton) {
    for _ in 0..10 {
        h.frame(&[], |ctx| b.update(ctx));
    }
}

#[test]
fn hover_highlights_and_publishes_help() {
    let mut h = Harness::new();
    let mut b = button(90);
    h.frame(&[move_to(150.0, 110.0)], |ctx| b.update(ctx));
    assert_eq!(b.state, ButtonState::Highlight);
    assert_eq!(h.out.sounds, vec![sfx::LOG_HIGHLIGHTBUTTONS]);
    assert_eq!(h.out.help_text_id, 1234);
    assert!(b.highlight.is_animating());
    // staying over it does not replay the sound
    h.frame(&[], |ctx| b.update(ctx));
    assert!(h.out.sounds.is_empty());
    // leaving returns to enabled
    h.frame(&[move_to(10.0, 10.0)], |ctx| b.update(ctx));
    assert_eq!(b.state, ButtonState::Enabled);
    assert!(!b.highlight.is_animating());
}

#[test]
fn press_reports_on_mouse_down() {
    let mut h = Harness::new();
    let mut b = button(90);
    settle(&mut h, &mut b);
    h.frame(&[move_to(150.0, 110.0)], |ctx| b.update(ctx));
    let m = h.frame(&[left(true)], |ctx| b.update(ctx));
    assert_eq!(m, Some(Msg::new(msg::LEFTMOUSEDOWN, 90)));
    assert_eq!(b.state, ButtonState::Pressed);
    assert_eq!(h.out.sounds, vec![sfx::LOG_CLICKONBUTTON]);
    // holding past the hold time reports "held"
    let mut held = None;
    for _ in 0..20 {
        held = h.frame(&[], |ctx| b.update(ctx)).or(held);
    }
    assert_eq!(held, Some(Msg::new(msg::LEFTMOUSEHELD, 90)));
}

#[test]
fn message_on_release_fires_only_when_released_inside() {
    let mut h = Harness::new();
    let mut b = button(91);
    b.set_message_on_release();
    settle(&mut h, &mut b);
    h.frame(&[move_to(150.0, 110.0)], |ctx| b.update(ctx));
    assert_eq!(h.frame(&[left(true)], |ctx| b.update(ctx)), None);
    assert_eq!(b.state, ButtonState::Pressed);
    let m = h.frame(&[left(false)], |ctx| b.update(ctx));
    assert_eq!(m, Some(Msg::new(msg::LEFTMOUSEDOWN, 91)));
    assert_eq!(b.state, ButtonState::Enabled);

    // press inside, drag out, release: nothing
    settle(&mut h, &mut b);
    h.frame(&[left(true)], |ctx| b.update(ctx));
    h.frame(&[move_to(500.0, 500.0)], |ctx| b.update(ctx));
    let m = h.frame(&[left(false)], |ctx| b.update(ctx));
    assert_eq!(m, None);
}

#[test]
fn toggle_buttons_flip_and_disabled_buttons_refuse() {
    let mut h = Harness::new();
    let mut b = button(5);
    b.toggle = true;
    settle(&mut h, &mut b);
    h.frame(&[move_to(150.0, 110.0), left(true)], |ctx| b.update(ctx));
    assert!(b.is_pressed());
    h.frame(&[left(false)], |ctx| b.update(ctx));
    settle(&mut h, &mut b);
    assert!(b.is_pressed(), "a toggle stays down");
    h.frame(&[left(true)], |ctx| b.update(ctx));
    assert!(!b.is_pressed());
    h.frame(&[left(false)], |ctx| b.update(ctx));

    let mut d = button(6);
    d.disable(true);
    settle(&mut h, &mut d);
    let m = h.frame(&[left(true)], |ctx| d.update(ctx));
    assert_eq!(m, None);
    assert_eq!(d.state, ButtonState::Disabled);
    assert_eq!(h.out.sounds, vec![sfx::LOG_WRONGBUTTON]);
    assert!(!d.is_enabled());
}

#[test]
fn hidden_button_draws_nothing() {
    let h = Harness::new();
    let mut b = button(1);
    let dl = h.render(|ctx| b.render(ctx));
    assert_eq!(dl.quad_count(), 1);
    b.hide(true);
    let dl = h.render(|ctx| b.render(ctx));
    assert_eq!(dl.quad_count(), 0);
}

fn list_with_items(n: usize) -> ListBox {
    let mut l = ListBox {
        obj: Object::new(50.0, 50.0, 200.0, 100.0),
        ..ListBox::default()
    };
    for _ in 0..n {
        let it = TextListItem {
            obj: Object::new(0.0, 0.0, 180.0, 20.0),
            ..TextListItem::default()
        };
        l.add_item(Box::new(it)).unwrap();
    }
    l
}

#[test]
fn list_items_stack_select_and_highlight() {
    let mut h = Harness::new();
    let mut l = list_with_items(8);
    // stacked 20 high with one pixel between
    assert_eq!(l.items[0].obj().global_y(), 50);
    assert_eq!(l.items[1].obj().global_y(), 71);
    assert_eq!(l.items[7].obj().global_y(), 50 + 7 * 21);
    for _ in 0..10 {
        h.frame(&[], |ctx| l.update(ctx));
    }
    // hover item 1
    h.frame(&[move_to(100.0, 75.0)], |ctx| l.update(ctx));
    assert_eq!(l.items[1].state(), ItemState::Highlite);
    assert_eq!(h.out.sounds, vec![sfx::LOG_DIGITALHIGHLIGHT]);
    // items below the box are hidden
    assert!(!l.items[7].obj().show);
    // click selects it
    h.frame(&[left(true)], |ctx| l.update(ctx));
    assert_eq!(l.selected_item(), 1);
    assert_eq!(l.items[1].state(), ItemState::Selected);
    assert_eq!(h.out.sounds, vec![sfx::LOG_SELECT]);
    h.frame(&[left(false), move_to(100.0, 96.0)], |ctx| l.update(ctx));
    // the selection survives hovering another item
    assert_eq!(l.items[1].state(), ItemState::Selected);
    assert_eq!(l.items[2].state(), ItemState::Highlite);
}

#[test]
fn list_scrolls_within_its_ends() {
    let mut l = list_with_items(8);
    // the original only refuses to scroll up once the first item is below the top edge, so one
    // step past the top is possible, the next is not
    l.scroll(-5);
    assert_eq!(l.items[0].obj().global_y(), 55);
    l.scroll(-5);
    assert_eq!(l.items[0].obj().global_y(), 55);
    l.scroll(15);
    assert_eq!(l.items[0].obj().global_y(), 40);
    // the last item ends at 50 + 8*21 - 1 - 10 = 207 > 150: still scrolls
    for _ in 0..20 {
        l.scroll(10);
    }
    let last = l.items[7].obj();
    let bottom = last.global_y() as f32 + last.height();
    assert!(bottom < 150.0 + 10.0, "stopped near the bottom ({bottom})");
    let before = l.items[0].obj().global_y();
    l.scroll(10);
    assert_eq!(
        l.items[0].obj().global_y(),
        before,
        "no scrolling past the end"
    );
    // removing an item closes the gap
    let y2 = l.items[2].obj().global_y();
    l.remove_item(1);
    assert_eq!(l.items[1].obj().global_y(), y2 - 21);
}

#[test]
fn edit_takes_focus_types_and_edits() {
    let mut h = Harness::new();
    let mut e = Edit::new(100.0, 100.0, 150.0, 20.0, Default::default());
    for _ in 0..10 {
        h.frame(&[], |ctx| e.update(ctx));
    }
    h.frame(&[move_to(120.0, 110.0), left(true)], |ctx| e.update(ctx));
    assert!(e.has_focus());
    h.frame(&[left(false)], |ctx| e.update(ctx));
    let typed = [
        key(keys::KEY_H, true, Some("h")),
        key(keys::KEY_H, false, None),
        key(keys::KEY_I, true, Some("i")),
        key(keys::KEY_SLASH, true, Some("/")), // eaten
        key(keys::KEY_1, true, Some("1")),
    ];
    h.frame(&typed, |ctx| e.update(ctx));
    assert_eq!(e.entry(), "hi1");
    assert_eq!((e.ins1, e.ins2), (3, 3));
    h.frame(&[key(keys::KEY_BACK, true, None)], |ctx| e.update(ctx));
    assert_eq!(e.entry(), "hi");
    h.frame(
        &[
            key(keys::KEY_HOME, true, None),
            key(keys::KEY_DELETE, true, None),
        ],
        |ctx| e.update(ctx),
    );
    assert_eq!(e.entry(), "i");
    assert_eq!(e.ins1, 0);
    // numeric fields refuse letters
    e.set_numeric(true);
    h.frame(
        &[
            key(keys::KEY_A, true, Some("a")),
            key(keys::KEY_7, true, Some("7")),
        ],
        |ctx| e.update(ctx),
    );
    assert_eq!(e.entry(), "7i");
    // the length limit plays the error sound
    e.limit = 2;
    h.frame(&[key(keys::KEY_8, true, Some("8"))], |ctx| e.update(ctx));
    assert_eq!(e.entry(), "7i");
    assert_eq!(h.out.sounds, vec![sfx::LOG_WRONGBUTTON]);
    // Return drops the focus; a click outside too
    h.frame(&[key(keys::KEY_RETURN, true, None)], |ctx| e.update(ctx));
    assert!(!e.has_focus());
    // without focus keys are ignored
    h.frame(&[key(keys::KEY_9, true, Some("9"))], |ctx| e.update(ctx));
    assert_eq!(e.entry(), "7i");
}

#[test]
fn screens_take_gestures_that_start_on_them() {
    use mc2_platform::input::InputEvent;
    use mc2_ui::screen::LogisticsScreen;
    let h = Harness::new();
    let mut s = LogisticsScreen::default();
    s.buttons.push(button(1));
    s.edits
        .push(Edit::new(400.0, 400.0, 100.0, 20.0, Default::default()));
    let a = &h.assets;
    assert!(s.consumes(a, &move_to(150.0, 110.0), (150, 110)));
    assert!(!s.consumes(a, &move_to(10.0, 10.0), (10, 10)));
    // press on the button, release far away: both are the UI's
    assert!(s.consumes(a, &left(true), (150, 110)));
    assert!(s.consumes(a, &move_to(600.0, 10.0), (600, 10)));
    assert!(s.consumes(a, &left(false), (600, 10)));
    // a press elsewhere goes to the game, and so does the drag across the UI and the release
    assert!(!s.consumes(a, &left(true), (600, 10)));
    assert!(!s.consumes(a, &move_to(150.0, 110.0), (150, 110)));
    assert!(!s.consumes(a, &left(false), (150, 110)));
    // and the pointer is the UI's again afterwards
    assert!(s.consumes(a, &move_to(151.0, 110.0), (151, 110)));
    // an edit field counts as part of the screen; keys only while it has focus
    assert!(s.consumes(a, &left(true), (410, 405)));
    assert!(s.consumes(a, &left(false), (410, 405)));
    assert!(!s.consumes(a, &key(keys::KEY_A, true, Some("a")), (410, 405)));
    s.edits[0].focus = true;
    assert!(s.consumes(a, &key(keys::KEY_A, true, Some("a")), (410, 405)));
    assert!(!s.consumes(
        a,
        &InputEvent::Resized {
            logical_w: 1.0,
            logical_h: 1.0
        },
        (0, 0)
    ));
}

#[test]
fn gesture_capture_is_per_button_and_hidden_screens_take_nothing() {
    use mc2_platform::input::{InputEvent, MouseButton};
    use mc2_ui::screen::LogisticsScreen;
    let h = Harness::new();
    let mut s = LogisticsScreen::default();
    s.buttons.push(button(1));
    let a = &h.assets;
    let right = |pressed| InputEvent::Button {
        button: MouseButton::Right,
        pressed,
    };
    // left pressed on the UI: a right click during the gesture is the UI's too, and its release
    // does not end the left capture
    assert!(s.consumes(a, &left(true), (150, 110)));
    assert!(s.consumes(a, &right(true), (600, 10)));
    assert!(s.consumes(a, &right(false), (600, 10)));
    assert!(s.consumes(a, &move_to(600.0, 12.0), (600, 12)));
    assert!(s.consumes(a, &left(false), (600, 10)));
    // the other way round: a game-owned left drag keeps a right click over the UI for the game
    assert!(!s.consumes(a, &left(true), (600, 10)));
    assert!(!s.consumes(a, &right(true), (150, 110)));
    assert!(!s.consumes(a, &right(false), (150, 110)));
    assert!(!s.consumes(a, &left(false), (150, 110)));
    // hidden parts and hidden screens take nothing
    s.buttons[0].hide(true);
    assert!(!s.consumes(a, &left(true), (150, 110)));
    assert!(!s.consumes(a, &left(false), (150, 110)));
    s.buttons[0].hide(false);
    // hidden in the middle of a gesture: the rest of it goes to the game
    assert!(s.consumes(a, &left(true), (150, 110)));
    s.obj.show = false;
    assert!(!s.consumes(a, &move_to(150.0, 110.0), (150, 110)));
    assert!(!s.consumes(a, &left(false), (150, 110)));
    s.obj.show = true;
    assert!(
        !s.consumes(a, &move_to(600.0, 10.0), (600, 10)),
        "capture was dropped"
    );
}

#[test]
fn a_game_owned_press_never_reaches_the_widgets() {
    use mc2_ui::screen::LogisticsScreen;
    let mut h = Harness::new();
    let mut s = LogisticsScreen::default();
    s.edits
        .push(Edit::new(400.0, 400.0, 100.0, 20.0, Default::default()));
    for _ in 0..10 {
        h.routed_frame(&mut s, &[], |s, ctx| s.update(ctx));
    }
    // press in the game, then move onto the edit field within the same frame
    let (taken, _) = h.routed_frame(
        &mut s,
        &[move_to(600.0, 10.0), left(true), move_to(410.0, 405.0)],
        |s, ctx| s.update(ctx),
    );
    assert_eq!(taken, vec![false, false, false]);
    assert!(
        !s.edits[0].has_focus(),
        "the game's click did not focus the edit"
    );
    let (taken, _) = h.routed_frame(&mut s, &[left(false)], |s, ctx| s.update(ctx));
    assert_eq!(taken, vec![false]);
    // a click of its own does
    for _ in 0..10 {
        h.routed_frame(&mut s, &[], |s, ctx| s.update(ctx));
    }
    let (taken, _) = h.routed_frame(&mut s, &[left(true)], |s, ctx| s.update(ctx));
    assert_eq!(taken, vec![true]);
    assert!(s.edits[0].has_focus());
}

#[test]
fn routing_keeps_focus_and_held_state_consistent() {
    use mc2_platform::input::KeyStatus;
    use mc2_ui::screen::LogisticsScreen;
    let mut h = Harness::new();
    let mut s = LogisticsScreen::default();
    s.buttons.push(button(1));
    s.edits
        .push(Edit::new(400.0, 400.0, 100.0, 20.0, Default::default()));
    for _ in 0..10 {
        h.routed_frame(&mut s, &[], |s, ctx| s.update(ctx));
    }
    // focus the edit, then click in the game: the focus goes, keys go to the game
    h.routed_frame(&mut s, &[move_to(410.0, 405.0)], |s, ctx| s.update(ctx));
    h.routed_frame(&mut s, &[left(true)], |s, ctx| s.update(ctx));
    h.routed_frame(&mut s, &[left(false)], |s, ctx| s.update(ctx));
    assert!(s.edits[0].has_focus());
    for _ in 0..10 {
        h.routed_frame(&mut s, &[], |s, ctx| s.update(ctx));
    }
    let (taken, _) = h.routed_frame(&mut s, &[move_to(600.0, 10.0), left(true)], |s, ctx| {
        s.update(ctx)
    });
    assert_eq!(taken, vec![false, false]);
    assert!(!s.edits[0].has_focus());
    let (taken, _) = h.routed_frame(
        &mut s,
        &[left(false), key(keys::KEY_A, true, Some("a"))],
        |s, ctx| s.update(ctx),
    );
    assert_eq!(taken, vec![false, false]);
    // a press the UI took and a screen hidden before the release: the release still reaches the
    // UI input, so the button is not left held
    for _ in 0..10 {
        h.routed_frame(&mut s, &[], |s, ctx| s.update(ctx));
    }
    h.routed_frame(&mut s, &[move_to(150.0, 110.0)], |s, ctx| s.update(ctx));
    let (taken, _) = h.routed_frame(&mut s, &[left(true)], |s, ctx| s.update(ctx));
    assert_eq!(taken, vec![true]);
    s.obj.show = false;
    let (taken, _) = h.routed_frame(&mut s, &[left(false)], |s, ctx| s.update(ctx));
    assert_eq!(taken, vec![false]);
    assert_eq!(h.input.key_status(keys::KEY_LMOUSE), KeyStatus::Free);
    s.obj.show = true;
    for _ in 0..10 {
        h.routed_frame(&mut s, &[], |s, ctx| s.update(ctx));
    }
    let (_, msgs) = h.routed_frame(&mut s, &[left(true)], |s, ctx| s.update(ctx));
    assert_eq!(
        msgs,
        vec![Msg::new(msg::LEFTMOUSEDOWN, 1)],
        "the next press clicks"
    );
}
