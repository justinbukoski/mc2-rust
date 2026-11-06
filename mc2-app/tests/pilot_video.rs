// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Pilot talking-head videos in the unit bar (`ForceGroupBar::setPilotVideo`, `code/forcegroupbar.cpp:411-512`):
//! a `PilotVideoStart` plays `data/movies/<pilot><code>.mpg` in the pilot's unit-bar slot. Headless (no window or GPU);
//! skipped without `MC2_DATA`.

use mc2_app::hud::{HudFrameInput, HudRuntime};
use mc2_app::pilot_video::video_name;
use mc2_app::userinput::UserInput;
use mc2_formats::vfs::Vfs;
use mc2_platform::input::InputEvent;
use mc2_platform::movie::Movie;
use mc2_render::camera::{CameraSettings, GameCamera};
use std::path::PathBuf;

struct Fixture {
    vfs: Vfs,
    hud: HudRuntime,
    pilots: Vec<(u32, String)>,
    snapshot: mc2_app::hud_data::HudSnapshot,
}

/// One HUD frame, so the unit bar has its icons.
fn update_once(hud: &mut HudRuntime, snapshot: &mc2_app::hud_data::HudSnapshot) {
    let camera = GameCamera::new(CameraSettings::default(), hud_size(hud));
    let user_input = UserInput::new();
    let speaking = std::collections::BTreeSet::new();
    let elevation = |_: f32, _: f32| 0.0;
    let events: [InputEvent; 0] = [];
    hud.update(HudFrameInput {
        events: &events,
        user_input: &user_input,
        fired: &[],
        camera: &camera,
        game_size: hud_size(hud),
        dt: 1.0 / 30.0,
        drag_threshold_px: 5.0,
        left_right_profile: true,
        shift: false,
        waypoint_queue: false,
        turn: 0,
        camera_rotate: false,
        scenario_time: 0.0,
        mouse_world: None,
        line_of_sight: true,
        passable: true,
        elevation: &elevation,
        snapshot: snapshot.clone(),
        speaking_pilots: &speaking,
    });
}

fn hud_size(hud: &HudRuntime) -> (f32, f32) {
    let r = hud.resolution();
    (r.0 as f32, r.1 as f32)
}

fn fixture() -> Option<Fixture> {
    let root = std::env::var_os("MC2_DATA").filter(|v| !v.is_empty())?;
    let mut vfs = Vfs::from_system_cfg(root).expect("mount game data");
    let world = mc2_sim::mission::init::load_world(
        &mut vfs,
        &mc2_sim::mission::init::LoadSpec::quickstart(
            "mc2_01",
            1,
            mc2_sim::rng::RngMode::LegacyMsvc,
        ),
    )
    .expect("load mc2_01");
    let sim = mc2_sim::Sim::new(world);
    let snapshot =
        mc2_app::hud_data::snapshot(&sim.world, &mc2_app::hud_data::PickBoxes::default());
    let mut hud = HudRuntime::load(&mut vfs, (1920.0, 1200.0), "mc2_01", snapshot.clone())
        .expect("load production HUD");
    update_once(&mut hud, &snapshot);
    let pilots = hud.pilot_names();
    Some(Fixture {
        vfs,
        hud,
        pilots,
        snapshot,
    })
}

#[test]
fn the_movie_name_is_seven_letters_of_the_pilot_and_the_code() {
    assert_eq!(video_name("Claymore", b'3'), "claymor3");
    assert_eq!(video_name("Bubba", b'1'), "bubba1");
}

#[test]
fn pilot_video_start_plays_the_pilots_video_in_the_unit_bar_slot() {
    let Some(mut f) = fixture() else {
        eprintln!("SKIPPED: MC2_DATA is unset");
        return;
    };
    // a pilot whose video exists in the shipped data
    let (slot, name) = f
        .pilots
        .iter()
        .find(|(_, n)| {
            matches!(
                f.vfs
                    .read(&format!("data/movies/{}.mpg", video_name(n, b'1'))),
                Ok(Some(_))
            )
        })
        .cloned()
        .expect("a home pilot with a shipped video");
    assert!(f.hud.pilot_video_view().is_none(), "nothing plays yet");
    // the Pilot Videos option off: nothing plays
    assert!(!f
        .hud
        .start_pilot_video(&mut f.vfs, slot, b'1', false, false));
    assert!(f.hud.pilot_video_view().is_none());
    // a mission video playing keeps it off (`isMoviePlaying`)
    assert!(!f.hud.start_pilot_video(&mut f.vfs, slot, b'1', true, true));
    // PilotVideoStart
    assert!(
        f.hud.start_pilot_video(&mut f.vfs, slot, b'1', true, false),
        "the video of {name} starts"
    );
    for _ in 0..35 {
        f.hud.advance_pilot_video(&mut f.vfs, 1.0 / 30.0);
    }
    let (rect, frame) = f.hud.pilot_video_view().expect("a picture in the slot");
    assert!(frame.width > 0 && frame.height > 0);
    assert_eq!(
        Some(rect),
        f.hud.pilot_video_slot_rect(slot),
        "the pilot's slot"
    );
    assert!(rect.right > rect.left && rect.bottom > rect.top);
    // one at a time
    assert!(!f.hud.start_pilot_video(&mut f.vfs, slot, b'2', true, false));
    // PilotVideoEnd
    f.hud.end_pilot_video();
    assert!(f.hud.pilot_video_view().is_none());
}

#[test]
fn flash_video_keeps_the_still_portrait_until_its_first_picture_is_replaced() {
    let Some(mut f) = fixture() else {
        eprintln!("SKIPPED: MC2_DATA is unset");
        return;
    };
    let (slot, _) = f
        .pilots
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case("Flash"))
        .expect("Flash is in the first mission")
        .clone();
    assert!(f.hud.start_pilot_video(&mut f.vfs, slot, b'1', true, false));
    assert!(
        f.hud.pilot_video_view().is_none(),
        "still portrait at start"
    );
    for _ in 0..1 {
        f.hud.advance_pilot_video(&mut f.vfs, 1.0 / 30.0);
        assert!(
            f.hud.pilot_video_view().is_none(),
            "still portrait remains visible while the unusable first picture is current"
        );
    }
    for _ in 0..5 {
        f.hud.advance_pilot_video(&mut f.vfs, 1.0 / 30.0);
    }
    assert!(
        f.hud.pilot_video_view().is_some(),
        "the video appears as soon as a later picture is available"
    );
}

#[test]
fn a_playing_pilot_video_survives_a_hud_layout_change() {
    let Some(mut f) = fixture() else {
        eprintln!("SKIPPED: MC2_DATA is unset");
        return;
    };
    let (slot, _) = f
        .pilots
        .iter()
        .find(|(_, n)| {
            matches!(
                f.vfs
                    .read(&format!("data/movies/{}.mpg", video_name(n, b'1'))),
                Ok(Some(_))
            )
        })
        .cloned()
        .expect("a home pilot with a shipped video");
    assert!(f.hud.start_pilot_video(&mut f.vfs, slot, b'1', true, false));
    for _ in 0..10 {
        f.hud.advance_pilot_video(&mut f.vfs, 1.0 / 30.0);
    }
    let old_rect = f.hud.pilot_video_slot_rect(slot);
    // the resize rebuilds the HUD for the smaller layout
    let mut fresh = HudRuntime::load(&mut f.vfs, (800.0, 600.0), "mc2_01", f.snapshot.clone())
        .expect("load the 800x600 HUD");
    assert_ne!(fresh.resolution(), f.hud.resolution(), "a layout boundary");
    fresh.carry_state_from(&mut f.hud);
    update_once(&mut fresh, &f.snapshot);
    for _ in 0..25 {
        fresh.advance_pilot_video(&mut f.vfs, 1.0 / 30.0);
    }
    let (rect, frame) = fresh.pilot_video_view().expect("playback continues");
    assert!(frame.width > 0);
    assert_eq!(
        Some(rect),
        fresh.pilot_video_slot_rect(slot),
        "the new slot"
    );
    assert_ne!(Some(rect), old_rect, "in the new layout's rectangle");
}

#[test]
fn flash_pilot_movie_first_three_decoded_frames_are_complete() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("SKIPPED: MC2_DATA is unset");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).expect("mount game data");
    let mut movie = Movie::open_vfs(&mut vfs, "data/movies/flash1.mpg")
        .expect("open Flash's first pilot video");
    for index in 0..10 {
        let frame = movie
            .try_next_video_frame()
            .expect("decode frame")
            .expect("frame");
        eprintln!(
            "flash1 frame {index}: {:?}, tr={}, {}x{}, t={:.4}, Y/Cb/Cr={}/{}/{} bytes",
            frame.picture_type,
            frame.temporal_reference,
            frame.width,
            frame.height,
            frame.time,
            frame.y.len(),
            frame.cb.len(),
            frame.cr.len()
        );
        assert!(frame.width > 0 && frame.height > 0);
        assert!(frame.y.len() >= frame.y_stride as usize * frame.height as usize);
        assert!(frame.cb.len() >= frame.c_stride as usize * (frame.height as usize).div_ceil(2));
        assert_eq!(frame.cb.len(), frame.cr.len());
        assert_eq!(
            frame.picture_type,
            if index == 0 {
                mc2_movie::PictureType::I
            } else {
                mc2_movie::PictureType::P
            },
            "Flash startup picture {index}"
        );

        if index < 3 {
            let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
                .join(format!("pilot-video/flash1-frame-{index}.png"));
            std::fs::create_dir_all(path.parent().unwrap())
                .expect("create target artifact directory");
            let file = std::fs::File::create(path).expect("create frame PNG");
            let mut encoder = png::Encoder::new(file, frame.width, frame.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .expect("write PNG header")
                .write_image_data(&frame.to_rgba())
                .expect("write decoded frame");
        }
    }
}
