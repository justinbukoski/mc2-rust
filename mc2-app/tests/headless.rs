// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
#![allow(clippy::chunks_exact_to_as_chunks)]

//! Headless runs of the real game data through the app's render path (never a window).
//! Skipped when `MC2_DATA` is unset; a missing GPU adapter fails the tests unless
//! `MC2_SKIP_GPU_TESTS` is exactly `1`.

mod common;
pub use common::link_helper;
use mc2_app::camera::CameraRig;
use mc2_app::headless::{self, HeadlessOptions, HeadlessRun};
use mc2_app::script::InputScript;
use mc2_app::sim::NullSim;
use mc2_app::state::GameState;
use mc2_render::mission::PaintSource;
use std::path::{Path, PathBuf};

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

/// The GPU opt-out decision: only the value `1` skips (unset, empty, `0`, `true` do not).
fn skip_decision(var: Option<&std::ffi::OsStr>) -> bool {
    var.is_some_and(|v| v == "1")
}

fn gpu_skip_requested() -> bool {
    skip_decision(std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref())
}

fn skip_before_render_fixture(name: &str) -> bool {
    if gpu_skip_requested() {
        eprintln!("SKIPPED {name}: MC2_SKIP_GPU_TESTS=1");
        true
    } else {
        false
    }
}

#[test]
fn gpu_opt_out_needs_exactly_one() {
    use std::ffi::OsStr;
    assert!(skip_decision(Some(OsStr::new("1"))));
    for v in ["", "0", "true", "yes", " 1", "1 ", "11"] {
        assert!(!skip_decision(Some(OsStr::new(v))), "{v:?} must not skip");
    }
    assert!(!skip_decision(None));
}

fn viewer(opts: &HeadlessOptions) -> Option<HeadlessRun<NullSim>> {
    match HeadlessRun::viewer(opts) {
        Ok(r) => Some(r),
        Err(e) if e.contains("adapter") && gpu_skip_requested() => {
            eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS=1, skipping");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn out_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mc2-app-headless");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A deliberately textureless mover for render-path visibility tests.  The loaded mission still
/// supplies the terrain, camera controller and `GameCore::render` frame; this fixture keeps the
/// screen-edge arithmetic independent of which mission parts the installed data happens to ship.
fn install_lod_vehicle(
    run: &mut HeadlessRun<NullSim>,
    position: mc2_render::math::Vec3,
    lod_distance: f32,
    selection_bounds: (mc2_render::math::Vec3, mc2_render::math::Vec3),
) -> usize {
    use mc2_render::appearance::{AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
    use mc2_render::math::Vec3;
    use mc2_render::scene::{ShapeInstance, ShapeStyle};
    use mc2_render::shape::{NodeInfo, NodeKind, ShapeModel};

    let model = run.core.scene.add_model(
        &mut run.core.game,
        ShapeModel {
            name: "render-edge-fixture".into(),
            nodes: vec![NodeInfo {
                id: "root".into(),
                parent: None,
                center: Vec3::ZERO,
                relative_center: Vec3::ZERO,
                kind: NodeKind::Normal,
            }],
            texture_names: Vec::new(),
            texture_alpha: Vec::new(),
            vertices: Vec::new(),
            batches: Vec::new(),
            bounds: (Vec3::new(-50.0, -50.0, -50.0), Vec3::new(50.0, 50.0, 50.0)),
            cone_extent: 86.60254,
            building_bounds: (Vec3::ZERO, Vec3::ZERO),
            gestures: Vec::new(),
        },
        ShapeStyle::default(),
    );
    let shapes = ShapeSet {
        kind: AppearanceKind::Vehicle,
        lods: [Some(model), Some(model), None],
        lod_distance: [0.0, lod_distance, 0.0],
        selection_bounds: Some(selection_bounds),
        ..Default::default()
    };
    let rt = run.core.mission.as_mut().unwrap();
    let index = rt.view.instances.len();
    rt.view.instances.push(ShapeInstance {
        model,
        shapes,
        state: AppearanceState::default(),
        choice: ShapeChoice::default(),
        pose: Vec::new(),
        pose_key: None,
        turret: None,
        position,
        yaw_degrees: 0.0,
        infinite_light: [1.0; 3],
        ambient_override: None,
        casts_shadow: false,
        in_view: true,
        root_height_difference: 0.0,
        highlight: 0,
    });
    index
}

/// A synthetic building for edge transitions, driven through the real mission's `GameCore::render` path.
fn install_damage_edge_building(
    run: &mut HeadlessRun<NullSim>,
    position: mc2_render::math::Vec3,
    intact_bounds: (mc2_render::math::Vec3, mc2_render::math::Vec3),
    type_bounds: (mc2_render::math::Vec3, mc2_render::math::Vec3),
    damaged_bounds: (mc2_render::math::Vec3, mc2_render::math::Vec3),
) -> usize {
    use mc2_render::math::Vec3;
    use mc2_render::shape::{NodeInfo, NodeKind, ShapeModel};

    let model = |name: &str, bounds| ShapeModel {
        name: name.into(),
        nodes: vec![NodeInfo {
            id: "root".into(),
            parent: None,
            center: Vec3::ZERO,
            relative_center: Vec3::ZERO,
            kind: NodeKind::Normal,
        }],
        texture_names: Vec::new(),
        texture_alpha: Vec::new(),
        vertices: Vec::new(),
        batches: Vec::new(),
        bounds,
        cone_extent: bounds.0.length().max(bounds.1.length()),
        building_bounds: bounds,
        gestures: Vec::new(),
    };
    let intact = model("building-intact-edge", intact_bounds);
    let damaged_model = model("building-damaged-edge", damaged_bounds);
    install_building_models(run, position, intact, damaged_model, type_bounds)
}

fn install_building_models(
    run: &mut HeadlessRun<NullSim>,
    position: mc2_render::math::Vec3,
    intact_model: mc2_render::shape::ShapeModel,
    damaged_model: mc2_render::shape::ShapeModel,
    type_bounds: (mc2_render::math::Vec3, mc2_render::math::Vec3),
) -> usize {
    use mc2_render::appearance::{AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
    use mc2_render::scene::{ShapeInstance, ShapeStyle};
    let intact = run
        .core
        .scene
        .add_model(&mut run.core.game, intact_model, ShapeStyle::default());
    let damaged =
        run.core
            .scene
            .add_model(&mut run.core.game, damaged_model, ShapeStyle::default());
    let rt = run.core.mission.as_mut().unwrap();
    let index = rt.view.instances.len();
    rt.view.instances.push(ShapeInstance {
        model: intact,
        shapes: ShapeSet {
            kind: AppearanceKind::Building,
            lods: [Some(intact), None, None],
            // `BldgAppearanceType::init` establishes this from the base shape; changing
            // `bldgShape` for damage does not replace the type box used by recalcBounds.
            building_type_bounds: Some(type_bounds),
            damaged: Some(damaged),
            ..Default::default()
        },
        state: AppearanceState::default(),
        choice: ShapeChoice::default(),
        pose: Vec::new(),
        pose_key: None,
        turret: None,
        position,
        yaw_degrees: 0.0,
        infinite_light: [1.0; 3],
        ambient_override: None,
        casts_shadow: false,
        in_view: false,
        root_height_difference: 0.0,
        highlight: 0,
    });
    index
}

/// Decodes a PNG written by the app (8-bit RGBA).
fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn missions_render_at_1080p_and_native_4k() {
    let Some(data) = data() else { return };
    for mission in ["mc2_01", "mc2_10", "mc2_17"] {
        for size in [(1920u32, 1080u32), (3840, 2160)] {
            let mut opts = HeadlessOptions::new(&data, mission);
            opts.size = size;
            opts.game.cursor = false;
            let Some(mut run) = viewer(&opts) else { return };
            let shot = out_dir().join(format!("{mission}_{}x{}.png", size.0, size.1));
            for i in 0..12 {
                run.step((i == 11).then_some(shot.as_path())).unwrap();
            }
            assert_eq!(run.core.state(), &GameState::InMission);
            let (w, h, px) = read_png(&shot);
            assert_eq!((w, h), size);
            // A real scene has many distinct colours, unlike the single clear colour.  The
            // headless target can preserve a compact source palette, so do not require more
            // colours than that representation contains (the recorded gate frame has 69).
            let mut colours = std::collections::HashSet::new();
            for p in px.as_chunks::<4>().0.iter().step_by(97) {
                colours.insert((p[0] >> 2, p[1] >> 2, p[2] >> 2));
            }
            assert!(
                colours.len() > 32,
                "{mission} {size:?}: {} colours",
                colours.len()
            );
            let rt = run.core.mission.as_ref().unwrap();
            assert!(rt.visibility.active_vertex_count() > 0);
            assert_eq!(rt.rig.viewport, (size.0 as f32, size.1 as f32));
        }
    }
}

#[test]
fn native_resolution_is_not_an_upscale() {
    // The 4K frame must carry more detail than the 1080p one scaled up: compare the high-frequency
    // energy of the same view.
    let Some(data) = data() else { return };
    let mut energy = Vec::new();
    for size in [(1920u32, 1080u32), (3840, 2160)] {
        let mut opts = HeadlessOptions::new(&data, "mc2_01");
        opts.size = size;
        opts.game.cursor = false;
        let Some(mut run) = viewer(&opts) else { return };
        let shot = out_dir().join(format!("detail_{}x{}.png", size.0, size.1));
        for i in 0..4 {
            run.step((i == 3).then_some(shot.as_path())).unwrap();
        }
        let (w, h, px) = read_png(&shot);
        // box-downsample to 960x540 and measure neighbour differences of the green channel
        let f = w / 960;
        let mut small = vec![0f32; 960 * 540];
        for y in 0..540 {
            for x in 0..960 {
                let mut s = 0f32;
                for dy in 0..f {
                    for dx in 0..f {
                        let i = (((y * f + dy) * w + x * f + dx) * 4 + 1) as usize;
                        s += px[i] as f32;
                    }
                }
                small[(y * 960 + x) as usize] = s / (f * f) as f32;
            }
        }
        let mut e = 0f64;
        for y in 0..539 {
            for x in 0..959 {
                let a = small[y * 960 + x];
                e += ((a - small[y * 960 + x + 1]).abs() + (a - small[(y + 1) * 960 + x]).abs())
                    as f64;
            }
        }
        let _ = h;
        energy.push(e);
    }
    // the same picture: within 35 % of each other (a broken 4K path would differ wildly)
    let ratio = energy[1] / energy[0];
    assert!((0.65..1.35).contains(&ratio), "detail ratio {ratio}");
}

/// `mc2_01` at 1920x1080 through the app's launch path (`GameCore::load_mission`), with `campaign`
/// as the launch's home paint.  Returns the run and its frame.
fn mc2_01_frame(
    data: &std::path::Path,
    campaign: Option<PaintSource>,
    name: &str,
) -> Option<(HeadlessRun<NullSim>, Vec<u8>)> {
    let mut opts = HeadlessOptions::new(data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    opts.game.campaign_paint = campaign;
    let mut run = viewer(&opts)?;
    let shot = out_dir().join(format!("{name}.png"));
    // The camera eases in from the mover's pre-snap Z (`GameCamera::activate`): the lance is on screen from about frame 100.
    for i in 0..100 {
        run.step((i == 99).then_some(shot.as_path())).unwrap();
    }
    assert_eq!(run.core.state(), &GameState::InMission);
    let (w, h, px) = read_png(&shot);
    assert_eq!((w, h), (1920, 1080));
    Some((run, px))
}

#[test]
fn home_mechs_take_the_options_paint_on_the_production_path() {
    let Some(data) = data() else { return };
    let Some((run, _)) = mc2_01_frame(&data, None, "paint_options") else {
        return;
    };
    let view = &run.core.mission.as_ref().unwrap().view;
    // no campaign: the scheme comes from options.cfg, and no unit is left on its raw masks
    assert!(
        matches!(view.home_paint, PaintSource::Scheme { .. }),
        "{:?}",
        view.home_paint
    );
    assert!(
        view.report.unpainted.is_empty(),
        "{:?}",
        view.report.unpainted
    );
}

/// A private data root for a negative launch.  Every archive and loose file of `MC2_DATA` is
/// hard-linked into `target/`, so the launch reads the shipped data, and only the root's own files
/// differ: no `options.cfg` when `without_options`, and `mc2_01.fit` with its first `CommanderId`
/// line removed when `without_commander`.  `MC2_DATA` itself is never written.  Needs `MC2_DATA`.
fn fixture_root(name: &str, without_options: bool, without_commander: bool) -> PathBuf {
    let src = PathBuf::from(std::env::var_os("MC2_DATA").expect("MC2_DATA"));
    let dst = fresh_fixture(&src, name, without_options);
    if without_commander {
        edit_mission_fit(&src, &dst, |fit| {
            without_first_line_with(fit, b"CommanderId")
        });
    }
    dst
}

/// A private data root whose `mc2_01` has no home unit: every `CommanderId` 0 is 1 in the root's
/// own copy of the mission.  No shipped mission has this shape (every one has home units), so the
/// fixture is the only launch with no existing home mech or vehicle.  Needs `MC2_DATA`.
fn no_home_units_root() -> PathBuf {
    let src = PathBuf::from(std::env::var_os("MC2_DATA").expect("MC2_DATA"));
    let dst = fresh_fixture(&src, "no_home_units", false);
    edit_mission_fit(&src, &dst, |fit| commander_ids_as(fit, b'1'));
    dst
}

/// A fresh private root for `name` with every shipped file of `src` hard-linked in (`options.cfg`
/// left out when `skip_options`).
fn fresh_fixture(src: &Path, name: &str, skip_options: bool) -> PathBuf {
    // keyed by process id too: two cargo test runs sharing one target dir must not clear each other's fixture mid-link
    let dst = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("mc2-paint-fixtures")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dst);
    let skip_fn: Option<&dyn Fn(&Path) -> bool> = if skip_options {
        Some(&|p: &Path| p.file_name().unwrap_or_default() == "options.cfg")
    } else {
        None
    };
    common::link_helper::link_tree(src, &dst, skip_fn).unwrap();
    dst
}

/// Writes `edit` of the shipped `mc2_01.fit` as the root's own loose mission file.
fn edit_mission_fit(src: &Path, dst: &Path, edit: impl Fn(&[u8]) -> Vec<u8>) {
    let fit = mc2_render::game::GameData::open(src)
        .unwrap()
        .read("data/missions/mc2_01.fit")
        .unwrap();
    let loose = dst.join("data").join("missions").join("mc2_01.fit");
    // a hard link shares the shipped file: unlink before writing, or the write reaches MC2_DATA
    std::fs::create_dir_all(loose.parent().unwrap()).unwrap();
    let _ = std::fs::remove_file(&loose);
    std::fs::write(&loose, edit(&fit)).unwrap();
}

/// `bytes` with every `CommanderId` whose value is 0 given `to` instead.  The value keeps its line's
/// line ending.  FIT keys are matched without case.
fn commander_ids_as(bytes: &[u8], to: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for line in bytes.split_inclusive(|&b| b == b'\n') {
        let end = line
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')
            .unwrap_or(line.len());
        let (text, ending) = line.split_at(end);
        let eq = text.iter().rposition(|&b| b == b'=');
        let home = text
            .windows(11)
            .any(|w| w.eq_ignore_ascii_case(b"CommanderId"))
            && eq.is_some_and(|eq| text[eq + 1..].trim_ascii() == b"0");
        match eq {
            Some(eq) if home => {
                out.extend_from_slice(&text[..=eq]);
                out.extend_from_slice(&[b' ', to]);
            }
            _ => out.extend_from_slice(text),
        }
        out.extend_from_slice(ending);
    }
    out
}

/// `bytes` without the line that holds the first `key`.  FIT keys are matched without case
/// (`CommanderID` in the shipped missions).
fn without_first_line_with(bytes: &[u8], key: &[u8]) -> Vec<u8> {
    let at = bytes
        .windows(key.len())
        .position(|w| w.eq_ignore_ascii_case(key))
        .expect("the key is in the mission");
    let start = bytes[..at]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let end = bytes[at..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |i| at + i + 1);
    [&bytes[..start], &bytes[end..]].concat()
}

/// The error a launch of `root`'s `mc2_01` ends with, through both app paths: the viewer's frames
/// and `mc2 --headless` (`headless::run`).  Panics if the launch renders.
fn unavailable_launch(root: &Path) -> Option<String> {
    let mut opts = HeadlessOptions::new(root, "mc2_01");
    opts.size = (640, 360);
    opts.game.cursor = false;
    let mut run = viewer(&opts)?;
    let mut error = None;
    for _ in 0..12 {
        if let Err(e) = run.step(None) {
            error = Some(e);
            break;
        }
    }
    let error = error.expect("a launch without its paint must not render");
    let mut cli = opts.clone();
    cli.frames = 3;
    let cli_error = headless::run(&cli).expect_err("mc2 --headless must fail too");
    assert!(cli_error.contains("paint unavailable"), "{cli_error}");
    Some(error)
}

#[test]
fn a_launch_without_options_cfg_is_unavailable_not_drawn_raw() {
    if skip_before_render_fixture("options.cfg paint fixture") {
        return;
    }
    if data().is_none() {
        return;
    }
    let root = fixture_root("no_options_cfg", true, false);
    let Some(error) = unavailable_launch(&root) else {
        return;
    };
    // the units are named, and the missing input is the file
    assert!(error.contains("paint unavailable"), "{error}");
    assert!(error.contains("part "), "{error}");
    assert!(error.contains("options.cfg"), "{error}");
}

#[test]
fn a_mech_without_commander_id_is_unavailable_not_drawn_raw() {
    if skip_before_render_fixture("CommanderId paint fixture") {
        return;
    }
    if data().is_none() {
        return;
    }
    let root = fixture_root("no_commander_id", false, true);
    let Some(error) = unavailable_launch(&root) else {
        return;
    };
    assert!(error.contains("paint unavailable"), "{error}");
    assert!(error.contains("CommanderId"), "{error}");
}

#[test]
fn a_campaign_launch_paints_with_its_own_colours() {
    let Some(data) = data() else { return };
    // Deliberately not the options.cfg colours, which are the file's (`0xfae525` base).
    let campaign = PaintSource::Scheme {
        base: 0xff10_40c0,
        highlight: 0xffc0_2030,
    };
    let Some((from_campaign, c)) = mc2_01_frame(&data, Some(campaign), "paint_campaign") else {
        return;
    };
    let Some((from_file, f)) = mc2_01_frame(&data, None, "paint_file") else {
        return;
    };
    assert_eq!(
        from_campaign.core.mission.as_ref().unwrap().view.home_paint,
        campaign
    );
    assert_ne!(
        from_file.core.mission.as_ref().unwrap().view.home_paint,
        campaign
    );
    let differ = c
        .as_chunks::<4>()
        .0
        .iter()
        .zip(f.as_chunks::<4>().0)
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differ > 500,
        "{differ} pixels differ between the two paints"
    );
}

/// A campaign launch that claims colours it does not have fails at the launch boundary, whatever
/// units the mission holds: no mission is entered and no frame is written.
#[test]
fn a_campaign_launch_without_colours_is_unavailable_not_drawn() {
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (640, 360);
    opts.game.cursor = false;
    opts.game.campaign_paint = Some(PaintSource::Unavailable);
    let Some(mut run) = viewer(&opts) else { return };
    let shot = out_dir().join("paint_campaign_unavailable.png");
    let _ = std::fs::remove_file(&shot);
    let mut error = None;
    for i in 0..12 {
        if let Err(e) = run.step((i == 11).then_some(shot.as_path())) {
            error = Some(e);
            break;
        }
    }
    let error = error.expect("a campaign launch without colours must not render");
    assert!(error.contains("paint unavailable"), "{error}");
    assert!(error.contains("campaign"), "{error}");
    assert!(run.core.mission.is_none(), "the mission was entered");
    assert!(!shot.exists(), "a frame was written");
}

fn rig(run: &HeadlessRun<NullSim>) -> &CameraRig {
    &run.core.mission.as_ref().expect("in mission").rig
}

fn script(text: &str) -> InputScript {
    InputScript::parse(text).unwrap()
}

#[test]
fn scripted_camera_control_respects_the_original_limits() {
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (800, 600);
    opts.game.cursor = false;
    // frames 0 and 1 are boot and loading; the mission runs from frame 2
    opts.script = script(
        "
        # zoom in with + until the minimum altitude
        5   key NumpadAdd down
        200 key NumpadAdd up
        # zoom out with - until the tilt-dependent maximum
        205 key Minus down
        400 key Minus up
        # Shift+Up tilts towards top-down (missiongui tiltUp -> eye->tiltDown)
        405 key SHIFT down
        406 key UP down
        600 key UP up
        # Shift+Left rotates one 11.25 degree step per frame at 30 fps
        605 key LEFT down
        606 key LEFT up
        607 key SHIFT up
        # Left arrow scrolls
        610 key LEFT down
        640 key LEFT up
        # F4: stored view 2 (the mission's loaded prefs set View2Zoom=800, View2Tilt=35).
        650 tap F4
        # the wheel away from the user zooms in 5 key steps
        660 wheel 1
        # middle click: all normal
        670 button middle down
        671 button middle up
        # Alt + mouse: rotate and tilt
        680 move 400 300
        681 key ALT down
        682 move 410 300
        683 move 410 296
        684 key ALT up
        # the pointer at the left edge scrolls left
        690 move 0 300
        720 move 400 300
        ",
    );
    let Some(mut run) = viewer(&opts) else { return };
    let step_to = |run: &mut HeadlessRun<NullSim>, frame: u64| {
        while run
            .core
            .mission
            .as_ref()
            .is_none_or(|mission| mission.clock.frame() < frame)
        {
            run.step(None).unwrap();
        }
    };
    step_to(&mut run, 4);
    let start = rig(&run).clone();
    let s = &start.settings;
    assert_eq!(
        (s.min_perspective, s.max_perspective),
        (20.0, 88.0),
        "system.cfg limits"
    );
    assert_eq!(start.projection_angle, 35.0);
    assert_eq!(start.rotation, -170.0);
    assert_eq!(start.altitude, 1200.0);

    step_to(&mut run, 201);
    assert_eq!(rig(&run).altitude, 560.0, "AltitudeMinimum");
    assert!((rig(&run).scale_factor - 1.0).abs() < 1e-6);

    step_to(&mut run, 401);
    let r = rig(&run);
    assert!(
        (r.altitude - r.test_max()).abs() < 1e-3,
        "{} vs {}",
        r.altitude,
        r.test_max()
    );
    // testMax at the normal tilt: 1500 + 100 * (35 - 20) / 68
    assert!((r.test_max() - (1500.0 + 100.0 * 15.0 / 68.0)).abs() < 1e-3);

    step_to(&mut run, 601);
    assert_eq!(rig(&run).projection_angle, 88.0, "MaxPerspective");
    // the altitude stays under the old cap (cameraAltitudeDesired is below the new one)
    assert!(rig(&run).altitude <= rig(&run).test_max());

    step_to(&mut run, 606);
    let rot = rig(&run).rotation;
    // one step per frame while held (6000 deg/s * frameLength / 15 >= 11.25): frame 605 only
    assert!((rot - (-170.0 + 11.25)).abs() < 1e-3, "rotation {rot}");

    step_to(&mut run, 610);
    let before = rig(&run).position;
    step_to(&mut run, 641);
    let after = rig(&run).position;
    // moveLeft: (1, 0) rotated by worldCameraRotation (OppRotate)
    let a = rig(&run).world_rotation.to_radians();
    let (dx, dy) = (a.cos(), a.sin());
    let moved = (after.x - before.x, after.y - before.y);
    let len = (moved.0 * moved.0 + moved.1 * moved.1).sqrt();
    assert!(len > 100.0, "scrolled {len}");
    assert!(((moved.0 / len) - dx).abs() < 1e-3 && ((moved.1 / len) - dy).abs() < 1e-3);

    step_to(&mut run, 651);
    assert_eq!(
        (rig(&run).altitude, rig(&run).projection_angle),
        (800.0, 35.0)
    );

    step_to(&mut run, 661);
    let r = rig(&run);
    let step = 10.0 * ((1.0 / 30.0) / 15.0) * 5.0 * 1500.0;
    // zoomChoiceIn scales the step by the scale factor before the change
    assert!(
        r.altitude < 800.0 && r.altitude > 800.0 - step,
        "{}",
        r.altitude
    );

    step_to(&mut run, 672);
    let r = rig(&run);
    assert_eq!(
        (r.projection_angle, r.altitude, r.rotation),
        (35.0, 1200.0, -170.0)
    );

    step_to(&mut run, 681);
    let base = rig(&run).clone();
    step_to(&mut run, 685);
    let r = rig(&run);
    // 10 px right: rotateRight(11.25 * 0.1 * 10); 4 px up: tiltUp(11.25 * 0.1 * 4)
    let mut expect = base.rotation - 11.25;
    if expect < -180.0 {
        expect += 360.0;
    }
    assert!(
        (r.rotation - expect).abs() < 1e-3,
        "{} vs {expect}",
        r.rotation
    );
    assert!(
        (r.projection_angle - (35.0 - 4.5)).abs() < 1e-3,
        "{}",
        r.projection_angle
    );

    step_to(&mut run, 690);
    let p0 = rig(&run).position;
    step_to(&mut run, 700);
    let p1 = rig(&run).position;
    assert!(
        (p1.x - p0.x).abs() + (p1.y - p0.y).abs() > 50.0,
        "edge scroll"
    );

    // the camera never leaves the map border
    let side = run
        .core
        .mission
        .as_ref()
        .unwrap()
        .view
        .mission
        .terrain
        .world_side();
    let max_visual = side / 2.0 - 128.0 * 8.0;
    assert!(p1.x.abs() <= max_visual && p1.y.abs() <= max_visual);
    // nothing reached the order hook except what we pressed
    assert_eq!(run.core.state(), &GameState::InMission);
}

#[test]
fn resizing_keeps_rendering() {
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_02");
    opts.size = (640, 480);
    opts.script = script("4 resize 1024 576\n6 move 500 300\n");
    let Some(mut run) = viewer(&opts) else { return };
    let shot = out_dir().join("resized.png");
    for i in 0..8 {
        run.step((i == 7).then_some(shot.as_path())).unwrap();
    }
    let (w, h, _) = read_png(&shot);
    assert_eq!((w, h), (1024, 576));
    assert_eq!(rig(&run).viewport, (1024.0, 576.0));
    assert_eq!(run.core.game_size(), (1024.0, 576.0));
}

#[test]
fn application_render_path_selects_lod_at_the_camera_threshold() {
    // `GameCore::render` must drive `*Appearance::recalcBounds` before it submits the frame:
    // LOD 1 is selected only when the visible object's eye distance is strictly greater than
    // `Distance1` (`bdactor.cpp:1302-1325`, `gvactor.cpp:1732-1755`,
    // `mech3d.cpp:2244-2267`).
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1024, 768);
    opts.game.cursor = false;
    opts.game.interpolate_camera = false;
    let Some(mut run) = viewer(&opts) else { return };
    while run.core.state() != &GameState::InMission {
        run.step(None).unwrap();
    }

    let threshold = 1000.0;
    let position = {
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig
            .set_position(mc2_render::math::Vec3::ZERO, false, &ground, 1.0 / 30.0);
        rt.rig.position
    };
    let index = install_lod_vehicle(
        &mut run,
        position,
        threshold,
        (
            mc2_render::math::Vec3::new(-100.0, -100.0, -100.0),
            mc2_render::math::Vec3::new(100.0, 100.0, 100.0),
        ),
    );

    {
        let rt = run.core.mission.as_mut().unwrap();
        let ground = rt.view.ground();
        rt.rig.set_position(position, false, &ground, 1.0 / 30.0);
        rt.rig.zoom_value(threshold);
    }
    run.step(None).unwrap();
    assert_eq!(
        run.core.mission.as_ref().unwrap().view.instances[index]
            .choice
            .current_lod,
        0,
        "the C++ comparison is strictly greater than Distance1"
    );

    {
        let rt = run.core.mission.as_mut().unwrap();
        rt.rig.zoom_value(threshold + 1.0);
    }
    run.step(None).unwrap();
    assert_eq!(
        run.core.mission.as_ref().unwrap().view.instances[index]
            .choice
            .current_lod,
        1,
        "the submitted application frame selected LOD 1 past Distance1"
    );
}

#[test]
fn interpolated_render_uses_the_cxx_zoom_scale_at_a_screen_edge() {
    // `Camera::zoomValue` refreshes newScaleFactor before GVAppearance's preliminary rectangle
    // (`camera.cpp:2442-2460`, `gvactor.cpp:1619,1632-1640`).  At tilt 20, Max=1500:
    // old scale = 1-(560-560)/1500 = 1; new scale = 1-(1500-560)/1500 = 0.373333;
    // halfway through the drawn frame it is (1+0.373333)/2 = 0.686667.  Place the
    // vehicle's projected centre at x=1024+r: the C++ rectangle reaches x=1024+r-2r*
    // 0.686667 < 1024, but the stale new-camera scale gives 1024+r-2r*0.373333 > 1024.
    // The hand-derived outcome for this visible, Distance1=1 vehicle is therefore inView=true,
    // LOD 1.  `run.step` reaches GameCore::render, not a selection helper.
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1024, 768);
    opts.render_fps = 60;
    opts.game.cursor = false;
    opts.game.interpolate_camera = true;
    let Some(mut run) = viewer(&opts) else { return };
    while run.core.state() != &GameState::InMission {
        run.step(None).unwrap();
    }

    let index = install_lod_vehicle(
        &mut run,
        mc2_render::math::Vec3::ZERO,
        1.0,
        (
            mc2_render::math::Vec3::new(-600.0, -600.0, -600.0),
            mc2_render::math::Vec3::new(600.0, 600.0, 600.0),
        ),
    );
    let radius = {
        let model = run.core.mission.as_ref().unwrap().view.instances[index].model;
        run.core.scene.model(model).extent_radius()
    };
    assert!(radius > 1.0, "vehicle must have a nonzero C++ OBB radius");

    {
        let rt = run.core.mission.as_mut().unwrap();
        let offset = (512.0 + radius) * 2.0 * 1030.0 * 20.0_f32.to_radians().tan() / 1024.0;
        let ground = rt.view.ground();
        rt.rig.set_camera_rotation(0.0, 0.0);
        rt.rig.projection_angle = 20.0;
        rt.rig.zoom_value(560.0);
        // At yaw zero, screen right is world west (-x), so target = object + offset in +x.
        rt.rig.set_position(
            mc2_render::math::Vec3::new(offset, 0.0, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
        rt.view.instances[index].position =
            rt.rig.position - mc2_render::math::Vec3::new(offset, 0.0, 0.0);
    }
    // At 60 Hz render / 30 Hz simulation, wait for the half-step, then the following render
    // publishes this 560-altitude rig state as `cur_camera` at alpha zero.
    while run.core.mission.as_ref().unwrap().clock.alpha() < 0.25 {
        run.step(None).unwrap();
    }
    run.step(None).unwrap();
    // Advance once more to alpha 0.5 before changing the rig. The next frame is then a simulation
    // tick, which makes 1500 the current camera while preserving 560 as the previous one.
    run.step(None).unwrap();
    {
        let rt = run.core.mission.as_mut().unwrap();
        rt.rig.zoom_value(1500.0);
    }
    // One simulation frame publishes the 1500-altitude camera; this render itself is alpha=0.
    run.step(None).unwrap();
    // Reset after the new-camera frame (which is offscreen at its 0.373333 scale), then render
    // the next 60 Hz frame, halfway to the next 30 Hz simulation update. A stale scale retains
    // LOD 0; the drawn scale must select LOD 1.
    run.core.mission.as_mut().unwrap().view.instances[index].choice = Default::default();
    run.step(None).unwrap();

    let rt = run.core.mission.as_ref().unwrap();
    let drawn = rt.render_camera((1024.0, 768.0), true);
    assert!(
        (drawn.altitude - 1030.0).abs() < 1e-3,
        "drawn altitude {}",
        drawn.altitude
    );
    assert!(
        (drawn.mover_scale_factor - 0.686_666_67).abs() < 1e-5,
        "drawn scale {}",
        drawn.mover_scale_factor
    );
    assert!(
        rt.view.instances[index].in_view,
        "C++ edge rectangle should overlap"
    );
    assert_eq!(rt.view.instances[index].choice.current_lod, 1);
}

/// Boots mc2_01 at 1024x768 and returns once the mission runs (non-interpolated camera).
fn edge_run(data: &std::path::Path) -> Option<HeadlessRun<NullSim>> {
    let mut opts = HeadlessOptions::new(data, "mc2_01");
    opts.size = (1024, 768);
    opts.game.cursor = false;
    opts.game.interpolate_camera = false;
    let mut run = viewer(&opts)?;
    while run.core.state() != &GameState::InMission {
        run.step(None).unwrap();
    }
    Some(run)
}

/// Points the rig at the origin (altitude 560, projection angle 20, scale factor 1) and puts
/// building `index` `1.02` focal-plane half-widths to the screen's right: its centre projects to
/// x = 512 * (1 + 1.02) = 1034.24 px, just past the 1024 px edge.
fn park_building_at_right_edge(run: &mut HeadlessRun<NullSim>, index: usize) {
    let rt = run.core.mission.as_mut().unwrap();
    let offset = 560.0 * 20.0_f32.to_radians().tan() * 1.02;
    let ground = rt.view.ground();
    rt.rig.set_camera_rotation(0.0, 0.0);
    rt.rig.projection_angle = 20.0;
    rt.rig.zoom_value(560.0);
    rt.rig.set_position(
        mc2_render::math::Vec3::new(offset, 0.0, 0.0),
        false,
        &ground,
        1.0 / 30.0,
    );
    rt.view.instances[index].position =
        rt.rig.position - mc2_render::math::Vec3::new(offset, 0.0, 0.0);
}

fn destroy(run: &mut HeadlessRun<NullSim>, index: usize) {
    run.core.mission.as_mut().unwrap().view.instances[index]
        .state
        .status = mc2_render::appearance::ObjectStatus::Destroyed;
}

#[test]
fn application_render_rejects_a_zero_radius_base_even_with_a_larger_damaged_shape() {
    // C++ computes OBBRadius and the type box once from the BASE shape (bdactor.cpp:683-741).
    // Base r = 0: the rough rect is [1034.24, 1034.24] horizontally (bdactor.cpp:1168-1173), and
    // 1034.24 > screenResX 1024 fails `upperLeft.x <= resX` (bdactor.cpp:1175-1178), so inView is
    // false.  Destruction runs `setObjStatus` first (bldng.cpp:1497-1499) and swaps in the
    // damaged r=sqrt(3)*100 shape, but neither the rect nor the type box reads it: C++ rejects
    // on the damage frame too.  Driven through `GameCore::render`.
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let zero = (mc2_render::math::Vec3::ZERO, mc2_render::math::Vec3::ZERO);
    let wide = (
        mc2_render::math::Vec3::new(-100.0, -100.0, -100.0),
        mc2_render::math::Vec3::new(100.0, 100.0, 100.0),
    );
    let index =
        install_damage_edge_building(&mut run, mc2_render::math::Vec3::ZERO, zero, zero, wide);
    park_building_at_right_edge(&mut run, index);
    run.step(None).unwrap();
    assert!(!run.core.mission.as_ref().unwrap().view.instances[index].in_view);

    destroy(&mut run, index);
    run.step(None).unwrap();
    let inst = &run.core.mission.as_ref().unwrap().view.instances[index];
    assert_eq!(inst.choice.shown, mc2_render::appearance::Shown::Damaged);
    assert!(
        !inst.in_view,
        "base OBB/type box decide, not the damaged shape"
    );
}

#[test]
fn application_render_rejects_when_only_the_damaged_shape_would_reach_the_type_box_test() {
    // Base shape r = sqrt(3)*100 = 173.2: the rough rect is x in [1034.24 - 346.4, 1034.24 + 346.4]
    // = [687.8, 1380.6], which overlaps [0, 1024] (bdactor.cpp:1168-1178).  The authored base
    // type box is zero-sized, so it projects to the single column x = 1034.24 > 1024 and the
    // type-box test rejects (bdactor.cpp:1187-1190, 1293-1299).  The damaged shape's +/-100 box
    // would reach x = 783..1285, but C++ never uses it for the type box, so it stays rejected
    // both before and on the destruction frame.
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let zero = (mc2_render::math::Vec3::ZERO, mc2_render::math::Vec3::ZERO);
    let wide = (
        mc2_render::math::Vec3::new(-100.0, -100.0, -100.0),
        mc2_render::math::Vec3::new(100.0, 100.0, 100.0),
    );
    let index =
        install_damage_edge_building(&mut run, mc2_render::math::Vec3::ZERO, wide, zero, wide);
    park_building_at_right_edge(&mut run, index);
    run.step(None).unwrap();
    assert!(!run.core.mission.as_ref().unwrap().view.instances[index].in_view);

    destroy(&mut run, index);
    run.step(None).unwrap();
    let inst = &run.core.mission.as_ref().unwrap().view.instances[index];
    assert_eq!(inst.choice.shown, mc2_render::appearance::Shown::Damaged);
    assert!(
        !inst.in_view,
        "the damaged box must not replace the base type box"
    );
}

#[test]
fn application_render_accepts_a_damage_transition_on_the_edge_by_the_base_bounds() {
    // Base r = 173.2 and type box +/-100: rough rect [687.8, 1380.6] and projected type box
    // x ~ 783..1285 both overlap the 1024 px screen (bdactor.cpp:1175-1178, 1293-1299), so C++
    // accepts.  The damaged shape is zero-sized (its extent 0 still passes the cone test at
    // x/forward = 0.371 < 0.4167), and a damaged-shape rect or box would be the single column
    // 1034.24 and reject.  The accepting decision is the base's, on both frames.
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let zero = (mc2_render::math::Vec3::ZERO, mc2_render::math::Vec3::ZERO);
    let wide = (
        mc2_render::math::Vec3::new(-100.0, -100.0, -100.0),
        mc2_render::math::Vec3::new(100.0, 100.0, 100.0),
    );
    let index =
        install_damage_edge_building(&mut run, mc2_render::math::Vec3::ZERO, wide, wide, zero);
    park_building_at_right_edge(&mut run, index);
    run.step(None).unwrap();
    assert!(run.core.mission.as_ref().unwrap().view.instances[index].in_view);

    destroy(&mut run, index);
    run.step(None).unwrap();
    let inst = &run.core.mission.as_ref().unwrap().view.instances[index];
    assert_eq!(inst.choice.shown, mc2_render::appearance::Shown::Damaged);
    assert!(inst.in_view);
}

/// One node of a raw shape: `verts` empty means a node without a shape.
type RawNode<'a> = (
    &'a str,
    &'a str,
    [f32; 3],
    [f32; 3],
    &'a [[f32; 3]],
    &'a [[u32; 3]],
);

/// The bytes of a `.tgl` file (the layout `mc2_assets::tgl::decode_shape` reads).
fn tgl_bytes(nodes: &[RawNode]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend(mc2_assets::tgl::CURRENT_SHAPE_VERSION.to_le_bytes());
    b.extend((nodes.len() as u32).to_le_bytes());
    b.extend(0u32.to_le_bytes()); // no textures
    let id = |s: &str| {
        let mut f = [0u8; 25];
        f[..s.len()].copy_from_slice(s.as_bytes());
        f
    };
    for (node_id, parent, center, rel, verts, tris) in nodes {
        if verts.is_empty() {
            b.extend(0u32.to_le_bytes());
        } else {
            b.extend(1u32.to_le_bytes());
            b.extend((verts.len() as u32).to_le_bytes());
            for p in verts.iter() {
                for c in p.iter().chain(&[0.0, 1.0, 0.0]) {
                    b.extend(c.to_le_bytes());
                }
                b.extend(0u32.to_le_bytes());
            }
            b.extend((tris.len() as u32).to_le_bytes());
            for t in tris.iter() {
                for i in t {
                    b.extend(i.to_le_bytes());
                }
                b.extend(0u32.to_le_bytes()); // texture
                b.extend(0u32.to_le_bytes()); // render flags
                for _ in 0..6 {
                    b.extend(0.0f32.to_le_bytes());
                }
                for c in [0.0f32, 1.0, 0.0] {
                    b.extend(c.to_le_bytes());
                }
            }
            b.extend(0u32.to_le_bytes()); // shape textures
        }
        for c in center.iter().chain(rel) {
            b.extend(c.to_le_bytes());
        }
        b.extend(id(node_id));
        b.extend(id(parent));
    }
    b
}

/// Buildings `(name, nodes)` written as raw `data/tgl/<name>.tgl` files with a `<name>.ini` that has no
/// `[3DBounds]`, then instanced by the production `populate::building_instance` (file -> `decode_shape` ->
/// `ShapeModel::from_multishape` -> the resolver that assigns the type box -> `new_instance`), `ratio`
/// x/forward right of the screen centre, and `GameCore::render` stepped once.  The test supplies no model and
/// no box.  Returns the instance indices in order, with the boxes production assigned.
fn render_buildings_at(
    run: &mut HeadlessRun<NullSim>,
    buildings: &[(&str, &[RawNode])],
    ratio: f32,
) -> (
    Vec<usize>,
    Vec<(mc2_render::math::Vec3, mc2_render::math::Vec3)>,
) {
    use mc2_render::math::Vec3;
    let root = std::env::temp_dir().join(format!(
        "mc2-app-raw-buildings-{}-{}",
        std::process::id(),
        buildings.iter().map(|b| b.0).collect::<Vec<_>>().join("-")
    ));
    let tgl = root.join("data/tgl");
    std::fs::create_dir_all(&tgl).unwrap();
    for (name, nodes) in buildings {
        std::fs::write(tgl.join(format!("{name}.tgl")), tgl_bytes(nodes)).unwrap();
        std::fs::write(
            tgl.join(format!("{name}.ini")),
            format!("FITini\n[TGLData]\nst FileName=\"{name}\"\nFITend\n"),
        )
        .unwrap();
    }
    // No fast files: the raw loose files above are the whole data set.
    std::fs::write(
        root.join("system.cfg"),
        "FITini\n[FastFiles]\nNumFastFiles=0\nFITend\n",
    )
    .unwrap();
    let mut fixture = mc2_render::game::GameData::open(&root).unwrap();
    let mut built = Vec::new();
    for (name, _) in buildings {
        built.push(
            mc2_render::populate::building_instance(
                &mut fixture,
                &mut run.core.scene,
                name,
                Vec3::ZERO,
            )
            .unwrap()
            .expect("the raw building resolves"),
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
    let boxes: Vec<_> = built
        .iter()
        .map(|i| i.shapes.building_type_bounds.expect("production box"))
        .collect();
    let rt = run.core.mission.as_mut().unwrap();
    let first = rt.view.instances.len();
    rt.view.instances.extend(built);
    let indices: Vec<usize> = (first..rt.view.instances.len()).collect();
    let rt = run.core.mission.as_mut().unwrap();
    let offset = 560.0 * ratio;
    let ground = rt.view.ground();
    rt.rig.set_camera_rotation(0.0, 0.0);
    rt.rig.projection_angle = 20.0;
    rt.rig.zoom_value(560.0);
    rt.rig
        .set_position(Vec3::new(offset, 0.0, 0.0), false, &ground, 1.0 / 30.0);
    for &i in &indices {
        rt.view.instances[i].position = rt.rig.position - Vec3::new(offset, 0.0, 0.0);
    }
    run.step(None).unwrap();
    (indices, boxes)
}

#[test]
fn application_render_building_cone_excludes_the_root_offset() {
    // Root node centre (-100,0,0); vertices (-10,0,0), (10,0,0), (0,0,1) in a triangle and an unused
    // (0,0,-10).  C++ box over all four (msl.cpp:245-270): min (-10,0,-10), max (10,0,1), so
    // GetExtentRadius = max(14.142, 10.050) = 14.142 and the type box (+ root centre, bdactor.cpp:
    // 738-741) is (-110,0,-10)..(-90,0,1), OBBRadius 110.45.  At x/forward = 0.45, distance
    // 560*sqrt(1.2025) = 614.1, the cone limit is 0.4167 + 14.142/614.1 = 0.4397 < 0.45: C++ rejects
    // (bdactor.cpp:1104-1118).  Render-space bounds (radius ~110) would give 0.596 and accept.  The
    // type box is derived by production from the raw shape, not supplied.  Through `GameCore::render`.
    use mc2_render::math::Vec3;
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let nodes: [RawNode; 1] = [(
        "root",
        "",
        [-100.0, 0.0, 0.0],
        [0.0; 3],
        &[
            [-10.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -10.0],
        ],
        &[[0, 1, 2]],
    )];
    // Control: the same vertices already in render space (root centre 0), whose radius is ~110.
    let naive_nodes: [RawNode; 1] = [(
        "root",
        "",
        [0.0; 3],
        [0.0; 3],
        &[
            [-110.0, 0.0, 0.0],
            [-90.0, 0.0, 0.0],
            [-100.0, 0.0, 1.0],
            [-100.0, 0.0, -10.0],
        ],
        &[[0, 1, 2]],
    )];
    let (at, boxes) = render_buildings_at(
        &mut run,
        &[("nonzeroroot", &nodes), ("naiveroot", &naive_nodes)],
        0.45,
    );
    let model = run
        .core
        .scene
        .model(run.core.mission.as_ref().unwrap().view.instances[at[0]].model);
    assert!((model.cone_radius() - 14.142_136).abs() < 1e-3);
    assert_eq!(
        boxes[0],
        (Vec3::new(-110.0, 0.0, -10.0), Vec3::new(-90.0, 0.0, 1.0))
    );
    let rt = run.core.mission.as_ref().unwrap();
    assert!(
        !rt.view.instances[at[0]].in_view,
        "C++ radius 14.142 rejects"
    );
    assert!(rt.view.instances[at[1]].in_view, "control accepts");
}

#[test]
fn application_render_building_base_bounds_count_unused_vertices() {
    // One triangle (0,0,0) (1,0,0) (0,0,1) plus an unused vertex at Stuff (-100,0,0): the C++ box scans
    // every type vertex (msl.cpp:245-270), so OBBRadius = 100.005 and the type box (-100,0,0)..(1,0,1)
    // reaches 100 units toward the centre (bdactor.cpp:683-741).  At x/forward 0.40 (cone limit 0.4167
    // + 100/603 > 0.40) the building is at 1074.7 px: rough rect left 874.7 and the type box overlap
    // the screen, C++ accepts.  The triangle-only shape (radius 1.414, rect left 1071.9) rejects.
    // Both type boxes are derived by production from their raw shapes.  Through `GameCore::render`.
    use mc2_render::math::Vec3;
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let tri = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let with_unused: [RawNode; 1] = [(
        "root",
        "",
        [0.0; 3],
        [0.0; 3],
        &[tri[0], tri[1], tri[2], [-100.0, 0.0, 0.0]],
        &[[0, 1, 2]],
    )];
    let triangle_only: [RawNode; 1] = [("root", "", [0.0; 3], [0.0; 3], &tri, &[[0, 1, 2]])];
    let (at, boxes) = render_buildings_at(
        &mut run,
        &[
            ("unusedvertex", &with_unused),
            ("triangleonly", &triangle_only),
        ],
        0.40,
    );
    assert_eq!(
        boxes[0],
        (Vec3::new(-100.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 1.0))
    );
    let rt = run.core.mission.as_ref().unwrap();
    assert!(rt.view.instances[at[0]].in_view, "C++ accepts");
    assert!(
        !rt.view.instances[at[1]].in_view,
        "control: triangle-only bounds reject"
    );
}

#[test]
fn application_render_building_nested_nodes_use_relative_centres() {
    // root (centre 0) -> mid (relative -100, no shape) -> leaf (relative -10, triangle (0,0,0) (1,0,0)
    // (0,0,1)).  C++ (msl.cpp:245-270) adds each vertex's OWN node relative centre only: box
    // (-10,0,0)..(-9,0,1), OBBRadius sqrt(101) = 10.050, GetExtentRadius 10 (+ root centre 0, bdactor.cpp:
    // 683-741).  At x/forward 0.40 (screen 1074.7 px; cone limit 0.4167 + 10/603.1 = 0.433 > 0.40) the
    // rough rect left is 1074.7 - 20.1 = 1054.6 > 1024: C++ rejects.  The accumulated bind pose puts the
    // leaf at x -110..-109 (radius ~110, rect left 854.7, type box toward the centre) and would accept.
    // The control at 0.30 (934 px) is on screen.  Through `GameCore::render`.
    use mc2_render::math::Vec3;
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let nodes: [RawNode; 3] = [
        ("root", "", [0.0; 3], [0.0; 3], &[], &[]),
        ("mid", "root", [0.0; 3], [-100.0, 0.0, 0.0], &[], &[]),
        (
            "leaf",
            "mid",
            [0.0; 3],
            [-10.0, 0.0, 0.0],
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            &[[0, 1, 2]],
        ),
    ];
    let (at, boxes) = render_buildings_at(&mut run, &[("nested", &nodes)], 0.40);
    assert!(
        run.core
            .scene
            .model(run.core.mission.as_ref().unwrap().view.instances[at[0]].model)
            .bounds
            .0
            .x
            < -109.0,
        "the pose-accumulated box differs"
    );
    assert_eq!(
        boxes[0],
        (Vec3::new(-10.0, 0.0, 0.0), Vec3::new(-9.0, 0.0, 1.0))
    );
    assert!(
        !run.core.mission.as_ref().unwrap().view.instances[at[0]].in_view,
        "C++ rejects the nested building"
    );
    let (control, _) = render_buildings_at(&mut run, &[("nested", &nodes)], 0.30);
    assert!(
        run.core.mission.as_ref().unwrap().view.instances[control[0]].in_view,
        "control: the same building on screen"
    );
}

#[test]
fn application_render_building_nested_shape_with_unused_vertex() {
    // The combined case: root (centre 0) -> mid (relative -100, no shape) -> leaf (relative -10) with the
    // triangle (0,0,0) (1,0,0) (0,0,1) plus an unused vertex (-50,0,0); no [3DBounds].  C++ (msl.cpp:
    // 245-270) adds only each vertex's own node relative centre: box (-60,0,0)..(-9,0,1), GetExtentRadius
    // 60, OBBRadius 60.008.  At x/forward 0.40 (screen 1074.7 px, cone limit 0.4167 + 60/603.1 = 0.516)
    // the building is accepted; ignoring the unused vertex (radius 10, limit 0.433, rough rect left
    // 1054.6 > 1024) would reject, as in the nested-nodes test.  At 0.55
    // (dist 639.1, limit 0.4167 + 60/639.1 = 0.511) C++ rejects; the pose-accumulated shape (leaf at
    // -110.., unused at -160, radius ~160, limit 0.667) would accept.  Through `GameCore::render`.
    use mc2_render::math::Vec3;
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let nodes: [RawNode; 3] = [
        ("root", "", [0.0; 3], [0.0; 3], &[], &[]),
        ("mid", "root", [0.0; 3], [-100.0, 0.0, 0.0], &[], &[]),
        (
            "leaf",
            "mid",
            [0.0; 3],
            [-10.0, 0.0, 0.0],
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0],
                [-50.0, 0.0, 0.0],
            ],
            &[[0, 1, 2]],
        ),
    ];
    let (near, boxes) = render_buildings_at(&mut run, &[("nestedunused", &nodes)], 0.40);
    assert_eq!(
        boxes[0],
        (Vec3::new(-60.0, 0.0, 0.0), Vec3::new(-9.0, 0.0, 1.0))
    );
    assert!(
        run.core.mission.as_ref().unwrap().view.instances[near[0]].in_view,
        "C++ accepts: the unused vertex counts"
    );
    let (far, _) = render_buildings_at(&mut run, &[("nestedunused", &nodes)], 0.55);
    assert!(
        !run.core.mission.as_ref().unwrap().view.instances[far[0]].in_view,
        "C++ rejects: relative centres only, not the accumulated pose"
    );
}

#[test]
fn application_render_building_cone_uses_endpoint_lengths() {
    // GetExtentRadius = max(|min|, |max|) (msl.cpp:274-280).  Bounds (-300,0,0)..(0,0,300) give
    // 300 (the farthest-corner OBBRadius is 424.26, used only by the rough rect).  At
    // x/forward = 0.9, distance 560*sqrt(1.81) = 753.4: cone limit 0.4167 + 300/753.4 = 0.815 <
    // 0.9, so C++ rejects (bdactor.cpp:1104-1118) although the rough rect (centre x ~ 1778, half
    // width 848.5) and the 400-unit type box overlap.  The (-300,0,-300)..(300,0,300) control has
    // endpoint length 424.26 (limit 0.980) and is accepted.  Driven through `GameCore::render`.
    use mc2_render::math::Vec3;
    let Some(data) = data() else { return };
    let Some(mut run) = edge_run(&data) else {
        return;
    };
    let asym = (Vec3::new(-300.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 300.0));
    let sym = (Vec3::new(-300.0, 0.0, -300.0), Vec3::new(300.0, 0.0, 300.0));
    let ty = (
        Vec3::new(-400.0, -400.0, -400.0),
        Vec3::new(400.0, 400.0, 400.0),
    );
    let a = install_damage_edge_building(&mut run, Vec3::ZERO, asym, ty, asym);
    let b = install_damage_edge_building(&mut run, Vec3::ZERO, sym, ty, sym);
    for index in [a, b] {
        let rt = run.core.mission.as_mut().unwrap();
        let offset = 560.0 * 0.9;
        let ground = rt.view.ground();
        rt.rig.set_camera_rotation(0.0, 0.0);
        rt.rig.projection_angle = 20.0;
        rt.rig.zoom_value(560.0);
        rt.rig
            .set_position(Vec3::new(offset, 0.0, 0.0), false, &ground, 1.0 / 30.0);
        rt.view.instances[index].position = rt.rig.position - Vec3::new(offset, 0.0, 0.0);
    }
    run.step(None).unwrap();
    let rt = run.core.mission.as_ref().unwrap();
    assert!(!rt.view.instances[a].in_view, "cone radius 300 rejects");
    assert!(rt.view.instances[b].in_view, "cone radius 424.26 accepts");
}

#[test]
fn application_render_uses_damaged_extent_for_building_cone_transition() {
    // `Building::handleDestruction` calls `setObjStatus(DESTROYED)` before `recalcBounds`
    // (`bldng.cpp:1497-1499`). At x / forward = 0.45, C++'s horizontal cone limit is
    // tan(40 * 0.7071 * 1024 / 1280) = 0.4167 (`bdactor.cpp:1088-1118`). An intact
    // r=sqrt(3)*100 adds 0.309 at the 560-unit focal distance and passes; damaged r=0
    // fails. The base OBB rectangle and derived type box remain overlapping, exercising
    // the changed shape through `GameCore::render` rather than a selection helper.
    let Some(data) = data() else { return };
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1024, 768);
    opts.game.cursor = false;
    opts.game.interpolate_camera = false;
    let Some(mut run) = viewer(&opts) else { return };
    while run.core.state() != &GameState::InMission {
        run.step(None).unwrap();
    }

    let intact_bounds = (
        mc2_render::math::Vec3::new(-100.0, -100.0, -100.0),
        mc2_render::math::Vec3::new(100.0, 100.0, 100.0),
    );
    let index = install_damage_edge_building(
        &mut run,
        mc2_render::math::Vec3::ZERO,
        intact_bounds,
        intact_bounds,
        (mc2_render::math::Vec3::ZERO, mc2_render::math::Vec3::ZERO),
    );
    {
        let rt = run.core.mission.as_mut().unwrap();
        let offset = 560.0 * 0.45;
        let ground = rt.view.ground();
        rt.rig.set_camera_rotation(0.0, 0.0);
        rt.rig.projection_angle = 20.0;
        rt.rig.zoom_value(560.0);
        rt.rig.set_position(
            mc2_render::math::Vec3::new(offset, 0.0, 0.0),
            false,
            &ground,
            1.0 / 30.0,
        );
        rt.view.instances[index].position =
            rt.rig.position - mc2_render::math::Vec3::new(offset, 0.0, 0.0);
    }
    run.step(None).unwrap();
    assert!(
        run.core.mission.as_ref().unwrap().view.instances[index].in_view,
        "the intact extent admits the edge centre"
    );

    run.core.mission.as_mut().unwrap().view.instances[index]
        .state
        .status = mc2_render::appearance::ObjectStatus::Destroyed;
    run.step(None).unwrap();
    let inst = &run.core.mission.as_ref().unwrap().view.instances[index];
    assert_eq!(inst.choice.shown, mc2_render::appearance::Shown::Damaged);
    assert!(
        !inst.in_view,
        "the damaged extent must decide the destruction frame's cone gate"
    );
}

#[test]
fn an_srgb_target_shows_the_same_picture() {
    // The surface's preferred format is 8-bit sRGB; drawing goes through its UNORM view, so the
    // stored bytes match the Rgba8Unorm offscreen path exactly.
    let Some(data) = data() else { return };
    let mut frames = Vec::new();
    for srgb in [false, true] {
        let mut opts = HeadlessOptions::new(&data, "mc2_01");
        opts.size = (640, 360);
        opts.game.cursor = false;
        let Some(mut run) = viewer(&opts) else { return };
        if srgb {
            let gpu = run.core.scene.gpu_arc().clone();
            run.target = mc2_render::gpu::OffscreenTarget::with_format(
                &gpu,
                640,
                360,
                wgpu::TextureFormat::Rgba8UnormSrgb,
            );
        }
        let shot = out_dir().join(format!("srgb_{srgb}.png"));
        for i in 0..4 {
            run.step((i == 3).then_some(shot.as_path())).unwrap();
        }
        frames.push(read_png(&shot).2);
        // a view off the map edge at the lowest tilt: much of the frame is the fog clear colour
        {
            let rt = run.core.mission.as_mut().unwrap();
            let edge = rt.view.mission.terrain.world_side() / 2.0;
            let g = rt.view.ground();
            let mut rig = rt.rig.clone();
            rig.set_position(
                mc2_render::math::Vec3::new(0.0, edge, 0.0),
                false,
                &g,
                1.0 / 30.0,
            );
            rig.projection_angle = 20.0;
            rig.zoom_min();
            // yaw 180: the camera sits south of the target and looks north, off the map
            rig.set_camera_rotation(180.0, 180.0);
            rt.rig = rig;
        }
        let shot = out_dir().join(format!("srgb_edge_{srgb}.png"));
        for i in 0..3 {
            run.step((i == 2).then_some(shot.as_path())).unwrap();
        }
        frames.push(read_png(&shot).2);
    }
    // the edge view really shows the clear colour: its top-left pixel repeats a lot
    let edge = &frames[1];
    let first = &edge[0..4];
    let same = edge.chunks_exact(4).filter(|p| *p == first).count();
    assert!(same * 10 > edge.len() / 4, "only {same} clear pixels");
    // Drawing (and blending) goes through the sRGB texture's UNORM view: byte-identical frames,
    // the alpha-blended water of the edge view and its cleared sky included.
    assert!(frames[0] == frames[2], "land view differs");
    assert!(frames[1] == frames[3], "edge view differs");
}

#[test]
fn scripted_events_arrive_exactly_once_at_any_render_rate() {
    // The script is keyed by simulation frame; 60, 30 and 15 render frames per second (sim 30 Hz)
    // must hand every event to the same sim frame exactly once, so the camera ends identical.
    let Some(data) = data() else { return };
    let mut states = Vec::new();
    for fps in [60, 30, 15] {
        let mut opts = HeadlessOptions::new(&data, "mc2_01");
        opts.size = (640, 360);
        opts.game.cursor = false;
        opts.render_fps = fps;
        opts.script =
            script("9 wheel 1\n10 wheel 1\n11 key SHIFT down\n12 key LEFT down\n13 key LEFT up\n");
        let Some(mut run) = viewer(&opts) else { return };
        let mut guard = 0;
        while run.core.mission.as_ref().map_or(0, |m| m.clock.frame()) < 40 {
            run.step(None).unwrap();
            guard += 1;
            assert!(guard < 400, "never reached sim frame 40 at {fps} fps");
        }
        let r = rig(&run);
        states.push(format!("{r:?}"));
    }
    assert_eq!(states[0], states[1], "60 vs 30 render fps");
    assert_eq!(states[2], states[1], "15 vs 30 render fps");
    // and the script did something
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (640, 360);
    opts.game.cursor = false;
    let Some(mut idle) = viewer(&opts) else {
        return;
    };
    while idle.core.mission.as_ref().map_or(0, |m| m.clock.frame()) < 40 {
        idle.step(None).unwrap();
    }
    assert_ne!(
        format!("{:?}", rig(&idle)),
        states[1],
        "the script had no effect"
    );
}

/// P5: each visual switch reaches the production renderer (headless app path) on its own.
#[test]
fn visual_switches_reach_the_game_renderer() {
    use mc2_render::visual::VisualOptions;
    let Some(data) = data() else { return };
    let shot = |opts: VisualOptions, lake: bool, name: &str| -> Option<(VisualOptions, Vec<u8>)> {
        let mut o = HeadlessOptions::new(&data, "mc2_01");
        o.size = (320, 180);
        o.game.cursor = false;
        o.game.visual = opts;
        let mut run = match HeadlessRun::viewer(&o) {
            Ok(r) => r,
            Err(e) if e.contains("adapter") && gpu_skip_requested() => {
                eprintln!("no GPU: {e}; MC2_SKIP_GPU_TESTS set");
                return None;
            }
            Err(e) => panic!("{e}"),
        };
        println!("adapter: {}", run.core.gpu().adapter_name);
        // The start view has no water: look at the middle of the biggest wet patch (the vertex
        // with the most below-water vertices in its 11x11 neighbourhood).
        for _ in 0..8 {
            if !lake || run.core.mission.is_some() {
                break;
            }
            run.step(None).unwrap();
        }
        if lake {
            let rt = run.core.mission.as_mut().unwrap();
            let m = &rt.view.mission;
            let (side, level) = (m.terrain.side(), m.water.elevation);
            let wet = |c: usize, r: usize| m.terrain.vertex_position(c, r).z < level;
            let mut best = (0, 0, 0);
            for r in (5..side - 5).step_by(2) {
                for c in (5..side - 5).step_by(2) {
                    let n = (r - 5..=r + 5)
                        .flat_map(|rr| (c - 5..=c + 5).map(move |cc| (cc, rr)))
                        .filter(|&(cc, rr)| wet(cc, rr))
                        .count();
                    if n > best.2 {
                        best = (c, r, n);
                    }
                }
            }
            assert!(best.2 > 60, "mc2_01 has a lake ({} wet vertices)", best.2);
            let p = m.terrain.vertex_position(best.0, best.1);
            let ground = rt.view.ground();
            rt.rig.set_position(p, false, &ground, 1.0 / 30.0);
        }
        let path = out_dir().join(name);
        // long enough for the water animation to leave its first frame (about 2 s of game time)
        for i in 0..60 {
            run.step((i == 59).then_some(path.as_path())).unwrap();
        }
        Some((run.core.scene.visual(), read_png(&path).2))
    };
    let Some((v0, off)) = shot(VisualOptions::default(), false, "vis_off.png") else {
        return;
    };
    let (_, off_lake) = shot(VisualOptions::default(), true, "vis_off_lake.png").unwrap();
    assert_eq!(v0, VisualOptions::default());
    let light = VisualOptions {
        enhanced_lighting: true,
        ..Default::default()
    };
    let water = VisualOptions {
        enhanced_water: true,
        ..Default::default()
    };
    let (vl, il) = shot(light, false, "vis_light.png").unwrap();
    let (vw, iw) = shot(water, true, "vis_water.png").unwrap();
    assert_eq!(vl, light);
    assert_eq!(vw, water);
    assert_ne!(il, off, "enhanced lighting reached the render");
    assert_ne!(iw, off_lake, "enhanced water reached the render");
}

/// A campaign launch of a mission with no existing home mech or vehicle, and colours it does not
/// have, fails at the same boundary: no mission is entered, no frame is written, and `mc2 --headless`
/// fails too.  The fixture is checked first, so the launch really has no home unit.
#[test]
fn a_campaign_launch_without_home_units_is_unavailable_not_drawn() {
    if skip_before_render_fixture("no-home-units paint fixture") {
        return;
    }
    if data().is_none() {
        return;
    }
    let root = no_home_units_root();
    let mut fixture = mc2_render::game::GameData::open(&root).unwrap();
    let mission = mc2_render::mission::Mission::load(&mut fixture, "mc2_01").unwrap();
    assert!(!mission.parts.is_empty());
    assert_eq!(
        mission
            .parts
            .iter()
            .filter(|p| p.commander_id == Some(0))
            .count(),
        0,
        "the fixture still has a home unit"
    );

    let mut opts = HeadlessOptions::new(&root, "mc2_01");
    opts.size = (640, 360);
    opts.game.cursor = false;
    opts.game.campaign_paint = Some(PaintSource::Unavailable);
    let Some(mut run) = viewer(&opts) else { return };
    let shot = out_dir().join("paint_campaign_no_home_units.png");
    let _ = std::fs::remove_file(&shot);
    let mut error = None;
    for i in 0..12 {
        if let Err(e) = run.step((i == 11).then_some(shot.as_path())) {
            error = Some(e);
            break;
        }
    }
    let error = error.expect("a campaign launch without colours must not render");
    assert!(error.contains("paint unavailable"), "{error}");
    assert!(error.contains("campaign"), "{error}");
    assert!(run.core.mission.is_none(), "the mission was entered");
    assert!(!shot.exists(), "a frame was written");

    let mut cli = opts.clone();
    cli.frames = 3;
    let cli_error = headless::run(&cli).expect_err("mc2 --headless must fail too");
    assert!(cli_error.contains("paint unavailable"), "{cli_error}");
}

#[test]
fn scripted_resize_and_screenshots_land_on_their_sim_frame_at_unequal_rates() {
    // Render 15 fps against sim 30 Hz: two ticks per render frame, so consecutive script frames
    // 6..12 fall on both sides of a render-frame boundary whatever the boot offset is.  Resizes
    // apply before their frame's tick and a screenshot shows the state right after its frame's
    // tick, so the images (size and content) must equal those of a 30 fps run.
    let Some(data) = data() else { return };
    let dir = out_dir().join("sim_frame_actions");
    let mut runs = Vec::new();
    for fps in [30, 15] {
        let sub = dir.join(format!("fps{fps}"));
        let _ = std::fs::remove_dir_all(&sub);
        let mut text = String::from("5 key LEFT down\n8 resize 320 200\n10 resize 400 240\n");
        for f in 6..=12 {
            text.push_str(&format!("{f} screenshot {}/f{f}.png\n", sub.display()));
        }
        let mut opts = HeadlessOptions::new(&data, "mc2_01");
        opts.size = (256, 160);
        opts.game.cursor = false;
        opts.render_fps = fps;
        opts.script = script(&text);
        let Some(mut run) = viewer(&opts) else { return };
        let mut guard = 0;
        while run.core.mission.as_ref().map_or(0, |m| m.clock.frame()) < 16 {
            run.step(None).unwrap();
            guard += 1;
            assert!(guard < 200, "never reached sim frame 16 at {fps} fps");
        }
        assert_eq!(
            run.screenshots.len(),
            7,
            "{fps} fps: one shot per script line"
        );
        runs.push((sub, run.core.physical_size()));
    }
    for f in 6..=12u32 {
        let want = match f {
            6..=7 => (256, 160),
            8..=9 => (320, 200),
            _ => (400, 240),
        };
        let a = read_png(&runs[0].0.join(format!("f{f}.png")));
        let b = read_png(&runs[1].0.join(format!("f{f}.png")));
        assert_eq!((a.0, a.1), want, "30 fps: frame {f} size");
        assert_eq!((b.0, b.1), want, "15 fps: frame {f} size");
        assert!(a.2 == b.2, "frame {f} differs between 30 and 15 render fps");
    }
    // the camera was moving, so neighbouring same-size frames really are different instants
    let p6 = read_png(&runs[1].0.join("f6.png"));
    let p7 = read_png(&runs[1].0.join("f7.png"));
    assert!(p6.2 != p7.2, "frames 6 and 7 are the same image");
    assert_eq!(runs[0].1, (400, 240));
    assert_eq!(runs[1].1, (400, 240));
}
