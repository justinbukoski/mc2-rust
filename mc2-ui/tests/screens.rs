// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Headless input-driven tests for the first game screens.

use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_ui::harness::{events, Harness};
use mc2_ui::screens::{MainMenu, MainMenuAction, OptionsEnv, OptionsScreen, Prefs};
use mc2_ui::{Loader, UiAssets};

fn data() -> Option<Vfs> {
    let root = std::env::var_os("MC2_DATA")?;
    Some(Vfs::from_system_cfg(root).expect("MC2 VFS"))
}

fn click(point: (f32, f32)) -> [InputEvent; 3] {
    [
        events::move_to(point.0, point.1),
        events::left(true),
        events::left(false),
    ]
}

#[test]
fn menu_navigation_opens_options() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let mut menu = MainMenu::load(&mut loader).expect("main menu FITs");
    menu.set_draw_background(false);
    menu.skip_intro();
    let b = menu.screen.button(95).expect("options button");
    let point = (
        b.global_x() as f32 + b.width() * 0.5,
        b.global_y() as f32 + b.height() * 0.5,
    );
    let mut h = Harness::with_assets(assets);
    let mut action = None;
    for event in click(point) {
        action = h
            .frame(std::slice::from_ref(&event), |ctx| menu.update(ctx))
            .action
            .or(action);
    }
    assert_eq!(action, Some(MainMenuAction::Options));
}

#[test]
fn menu_resume_clears_the_encyclopedia_fade_out() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let mut menu = MainMenu::load(&mut loader).expect("main menu FITs");
    menu.screen.begin_fade_out(1.0);

    menu.resume();

    assert_eq!(menu.screen.fade_out_time, 0.0);
}

#[test]
fn options_offset_uses_active_viewport_not_saved_resolution() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let saved_wide = Prefs {
        resolution_x: 1600,
        resolution_y: 1200,
        ..Prefs::default()
    };
    let options = OptionsScreen::load(
        &mut loader,
        saved_wide,
        Prefs::default(),
        &OptionsEnv::default(),
    )
    .expect("options FITs");

    let panel_position = options.screen.rects[2].obj.loc[0];
    let active_wide_env = OptionsEnv {
        viewport_width: 1600,
        ..OptionsEnv::default()
    };
    let active_wide = OptionsScreen::load(
        &mut loader,
        Prefs::default(),
        Prefs::default(),
        &active_wide_env,
    )
    .expect("options FITs");
    let active_wide_panel_position = active_wide.screen.rects[2].obj.loc[0];
    assert_eq!(panel_position.x, active_wide_panel_position.x - 400.0);
    assert_eq!(panel_position.y, active_wide_panel_position.y - 270.0);
}

#[test]
fn options_tab_and_audio_slider_change_through_harness() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let prefs = Prefs::default();
    let mut options = OptionsScreen::load(
        &mut loader,
        prefs.clone(),
        Prefs::default(),
        &OptionsEnv::default(),
    )
    .expect("options FITs");
    let mut h = Harness::with_assets(assets);
    h.idle(10, |_| {});
    options.select_tab(1);
    h.frame(&[], |ctx| options.update(ctx));
    assert_eq!(options.cur_tab, 1);
    let before = options.edited_prefs().music_volume;
    let point = options.volume_step_point(1).expect("music slider");
    for event in click(point) {
        h.frame(std::slice::from_ref(&event), |ctx| options.update(ctx));
    }
    let after = options.edited_prefs().music_volume;
    assert!(
        after > before,
        "right slider button should raise music volume: {before} -> {after}"
    );
}

#[test]
fn preferences_round_trip_after_screen_state_changes() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let mut prefs = Prefs {
        music_volume: 177,
        full_screen: true,
        resolution_x: 1920,
        resolution_y: 1080,
        use_left_right_mouse_profile: false,
        way_point_key: 41,
        ..Prefs::default()
    };
    prefs.player_name[0] = "Pilot One".into();
    let mut options =
        OptionsScreen::load(&mut loader, prefs, Prefs::default(), &OptionsEnv::default())
            .expect("options FITs");
    let mut h = Harness::with_assets(assets);
    options.select_tab(1);
    h.idle(10, |_| {});
    h.frame(&[], |ctx| options.update(ctx));
    let point = options.volume_step_point(1).expect("music slider");
    for event in click(point) {
        h.frame(std::slice::from_ref(&event), |ctx| options.update(ctx));
    }
    let edited = options.edited_prefs();
    let decoded =
        Prefs::from_fit(&mut mc2_formats::ini::FitIni::from_bytes(edited.to_fit()).unwrap());
    assert!(decoded.music_volume > 177);
    assert_eq!(decoded, edited);
}

#[test]
fn loading_screen_progress_uses_the_headless_harness() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let mut loading = mc2_ui::screens::LoadingScreen::load(&mut loader).expect("loading FIT");
    loading.begin();
    loading.set_progress(42.0);
    let mut h = Harness::with_assets(assets);
    let status = h.frame(&[], |ctx| loading.update(ctx));
    let draws = h.render(|ctx| loading.render(ctx));
    assert_eq!(loading.progress, 42.0);
    assert!(draws.quad_count() > 0);
    assert_eq!(status, mc2_ui::screen::Status::Running);
    assert!(loading.waiting_texture.is_some());
    assert_ne!(loading.waiting_texture, loading.progress_fill);
}

#[test]
fn loading_progress_during_entrance_keeps_ready_handoff() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let mut loading = mc2_ui::screens::LoadingScreen::load(&mut loader).expect("loading FIT");
    loading.begin();
    loading.set_progress(24.0);
    let mut h = Harness::with_assets(assets);
    let mut saw_ready = false;
    for _ in 0..300 {
        let status = h.frame(&[], |ctx| loading.update(ctx));
        if status == mc2_ui::screen::Status::ReadyToLoad {
            saw_ready = true;
            break;
        }
    }
    assert!(
        saw_ready,
        "entrance must expose READYTOLOAD with early progress"
    );
    assert_eq!(loading.progress, 24.0);

    loading.begin_outro();
    assert_eq!(
        h.frame(&[], |ctx| loading.update(ctx)),
        mc2_ui::screen::Status::Running,
        "outro consumes READYTOLOAD so the app receives the handoff only once"
    );
    let mut advanced_early = false;
    for _ in 0..300 {
        if h.frame(&[], |ctx| loading.update(ctx)) == mc2_ui::screen::Status::Next {
            advanced_early = true;
            break;
        }
    }
    assert!(!advanced_early, "outro must wait for at least 99% progress");
    loading.set_progress(99.0);
    assert_eq!(
        h.frame(&[], |ctx| loading.update(ctx)),
        mc2_ui::screen::Status::Next,
        "a completed outro may advance once loading reaches 99%"
    );
}

/// `LoadScreen::begin()` (code/loadscreen.cpp:362-375) resets `loadProgress`, which is what
/// selects the waiting art (`loadProgress == 1000`). A screen reused after a multiplayer wait
/// must therefore draw the progress bar again, exactly like a freshly loaded one.
#[test]
fn loading_screen_reuse_after_waiting_draws_progress_bar() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let mut reused = mc2_ui::screens::LoadingScreen::load(&mut loader).expect("loading FIT");
    let mut fresh = mc2_ui::screens::LoadingScreen::load(&mut loader).expect("loading FIT");

    reused.begin();
    reused.set_waiting_for_players(true);
    reused.begin();
    assert!(!reused.waiting_for_players);
    reused.set_progress(50.0);

    fresh.begin();
    fresh.set_progress(50.0);

    let mut h = Harness::with_assets(assets);
    h.frame(&[], |ctx| reused.update(ctx));
    let reused_draws = h.render(|ctx| reused.render(ctx));
    h.frame(&[], |ctx| fresh.update(ctx));
    let fresh_draws = h.render(|ctx| fresh.render(ctx));
    assert!(reused_draws.quad_count() > 0);
    assert_eq!(reused_draws.quad_count(), fresh_draws.quad_count());
}

#[test]
fn loading_screen_uses_resolution_specific_fit() {
    let Some(mut vfs) = data() else { return };
    let mut assets = UiAssets::default();
    let mut loader = Loader::new(&mut vfs, &mut assets);
    let loading = mc2_ui::screens::LoadingScreen::load_for_resolution(&mut loader, 1024)
        .expect("1024 loading FIT");
    assert!(loading.progress_background.is_some());
    assert!(loading.waiting_texture.is_some());
}
