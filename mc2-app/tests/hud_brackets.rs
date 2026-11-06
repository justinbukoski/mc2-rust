// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! World-view selection brackets, the order cursor, and info-window mouse input.
//! Headless production path (`GameCore` + `LiveSim` + shipped data). Skips without `MC2_DATA`.
//! A missing GPU is skipped only when `MC2_SKIP_GPU_TESTS=1` is set.

use mc2_app::camera::Ground;
use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::hud::HudRuntime;
use mc2_app::live_sim::LiveSim;
use mc2_app::script;
use mc2_app::world_view::{self, HoldFireIcon};
use mc2_formats::vfs::Vfs;
use mc2_platform::input::{InputEvent, MouseButton};
use mc2_render::camera::GameCamera;
use mc2_render::math::Vec3;
use mc2_sim::combat::{loadout_of_mut, HOME_COMMANDER};
use mc2_sim::command::FIRERANGE_CURRENT;
use mc2_sim::ids::{CommanderId, WatchId};
use mc2_sim::object::Kind;
use mc2_sim_data::objtype::building::LAND_BRIDGE_TYPE_INDEX;
use mc2_ui::mission_hud::{HitTarget, MissionTab};
use mc2_ui::mission_keys::{state, ObjectClass};
use mc2_ui::tacmap::TacTeam;
use std::path::PathBuf;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

fn gpu_skip_requested() -> bool {
    std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1")
}

fn start(sim_frames: u64) -> Option<HeadlessRun<LiveSim>> {
    let data = data()?;
    let mut opts = HeadlessOptions::new(&data, "mc2_01");
    opts.size = (1920, 1080);
    opts.game.cursor = false;
    let mut run = match HeadlessRun::live_bare(&opts) {
        Ok(r) => r,
        Err(e) if e.contains("no suitable GPU adapter") && gpu_skip_requested() => {
            eprintln!("no GPU adapter; MC2_SKIP_GPU_TESTS=1, skipping hud_brackets: {e}");
            return None;
        }
        Err(e) => panic!("hud_brackets could not start the production path: {e}"),
    };
    eprintln!("hud_brackets: MC2_DATA and GPU path entered");
    while run.core.mission.is_none() {
        run.step(None).unwrap();
    }
    while sim_frame(&run) < sim_frames {
        run.step(None).unwrap();
    }
    // mc2_01 is still in its opening tutorial movie at frame 130. These tests
    // intentionally isolate HUD brackets and cursors; movie flow itself is
    // exercised in presentation_live. Clear the presentation fixture without
    // advancing the mission through the several-second cancellation cinematic,
    // which would move its authored HUD targets out of this fixture's view.
    if run
        .core
        .mission
        .as_ref()
        .unwrap()
        .driver
        .sim()
        .world
        .presentation
        .movie_mode
    {
        run.core
            .mission
            .as_mut()
            .unwrap()
            .driver
            .sim_mut()
            .world
            .presentation = Default::default();
        // The app copies presentation state from the simulation after a frame.
        run.step(None).unwrap();
    }
    Some(run)
}

fn sim_frame(run: &HeadlessRun<LiveSim>) -> u64 {
    run.core.mission.as_ref().unwrap().clock.frame()
}

fn hud(run: &HeadlessRun<LiveSim>) -> &HudRuntime {
    run.core
        .mission
        .as_ref()
        .unwrap()
        .hud
        .as_ref()
        .expect("the live simulation has a HUD")
}

fn mouse(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    run.event(InputEvent::CursorMoved { x, y });
}

fn button(run: &mut HeadlessRun<LiveSim>, pressed: bool) {
    run.event(InputEvent::Button {
        button: MouseButton::Left,
        pressed,
    });
}

fn click(run: &mut HeadlessRun<LiveSim>, x: f64, y: f64) {
    mouse(run, x, y);
    run.step(None).unwrap();
    button(run, true);
    run.step(None).unwrap();
    button(run, false);
    run.step(None).unwrap();
}

fn key(run: &mut HeadlessRun<LiveSim>, name: &str) {
    let code = script::key_code(name).expect("key name");
    for e in script::key_events(code, true) {
        run.event(e);
    }
    run.step(None).unwrap();
    for e in script::key_events(code, false) {
        run.event(e);
    }
    run.step(None).unwrap();
}

fn hold_key(run: &mut HeadlessRun<LiveSim>, name: &str, pressed: bool) {
    let code = script::key_code(name).expect("key name");
    for e in script::key_events(code, pressed) {
        run.event(e);
    }
    run.step(None).unwrap();
}

fn open_info_tab(run: &mut HeadlessRun<LiveSim>) {
    for _ in 0..4 {
        if hud(run).tab() == MissionTab::Info {
            break;
        }
        key(run, "Tab");
    }
    assert_eq!(hud(run).tab(), MissionTab::Info);
}

fn rgba_at(rgba: &[u8], w: u32, x: i32, y: i32) -> Option<[u8; 4]> {
    if x < 0 || y < 0 {
        return None;
    }
    let (x, y) = (x as u32, y as u32);
    if x >= w {
        return None;
    }
    let i = ((y * w + x) * 4) as usize;
    rgba.get(i..i + 4).map(|p| [p[0], p[1], p[2], p[3]])
}

fn region_diff(a: &[u8], b: &[u8], w: u32, x0: i32, y0: i32, x1: i32, y1: i32) -> u32 {
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if rgba_at(a, w, x, y) != rgba_at(b, w, x, y) {
                n += 1;
            }
        }
    }
    n
}

fn world_of(run: &HeadlessRun<LiveSim>) -> &mc2_sim::World {
    &run.core.mission.as_ref().unwrap().driver.sim().world
}

fn mover_handle(run: &HeadlessRun<LiveSim>, part: i32) -> mc2_sim::ObjectHandle {
    let w = world_of(run);
    w.objects
        .movers
        .iter()
        .copied()
        .find(|&h| w.objects.get(h).is_some_and(|o| o.c.part_id.0 == part))
        .expect("a mover with that part id")
}

fn set_attack_range(run: &mut HeadlessRun<LiveSim>, part: i32, range: i32) {
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    for h in w.objects.movers.clone() {
        if let Some(o) = w.objects.get_mut(h) {
            if o.c.part_id.0 != part {
                continue;
            }
            match &mut o.kind {
                Kind::Mech(m) => m.m.attack_range = range,
                Kind::Vehicle(v) => v.m.attack_range = range,
                _ => {}
            }
        }
    }
}

fn scale_armor(run: &mut HeadlessRun<LiveSim>, part: i32, fraction: f32) {
    let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
    for h in w.objects.movers.clone() {
        let Some(o) = w.objects.get_mut(h) else {
            continue;
        };
        if o.c.part_id.0 != part {
            continue;
        }
        if let Some(l) = loadout_of_mut(o) {
            for (cur, max) in &mut l.armor {
                *cur = f32::from(*max) * fraction;
            }
        }
    }
}

fn shipped_blip_icon(data: &std::path::Path) -> HoldFireIcon {
    let mut vfs = Vfs::from_system_cfg(data).expect("vfs");
    let mut store = mc2_ui::textures::TextureStore::new();
    let id = store
        .load_tga(&mut vfs, &mc2_ui::textures::art_file_path("blip"))
        .expect("data/art/blip.tga");
    let img = store.get(id).expect("blip image");
    world_view::hold_fire_icon_from_rgba(img.width, img.height, &img.rgba)
        .expect("drawIcon UV region")
}

fn camera_of(run: &HeadlessRun<LiveSim>) -> GameCamera {
    run.core.mission.as_ref().unwrap().rig.view()
}

fn camera_k(run: &HeadlessRun<LiveSim>) -> f32 {
    1080.0 / camera_of(run).viewport.1.max(1.0)
}

fn world_pos(run: &HeadlessRun<LiveSim>, part: i32) -> (f32, f32, f32) {
    let w = world_of(run);
    let o = w.objects.get(mover_handle(run, part)).expect("mover");
    (o.c.position.x, o.c.position.y, o.c.position.z)
}

/// Game-pixel projection of a world point through the camera. Independent of the HUD snapshot.
fn project_world(run: &HeadlessRun<LiveSim>, pos: (f32, f32, f32)) -> (f32, f32) {
    let (s, _) = camera_of(run).project_z(Vec3::new(pos.0, pos.1, pos.2));
    let k = camera_k(run);
    (s[0] * k, s[1] * k)
}

fn screen_of(run: &HeadlessRun<LiveSim>, part: i32) -> (f32, f32) {
    project_world(run, world_pos(run, part))
}

/// The mech's `[3DBounds]` box parsed in the test from the shipped `data/tgl/<name>.ini`
/// (`apprtype.cpp:80-121`: six signed longs, stored verbatim).
fn raw_appearance_extents(run: &HeadlessRun<LiveSim>, part: i32) -> ([f32; 3], [f32; 3]) {
    use mc2_sim::appearance::Appearance;
    let w = world_of(run);
    let handle = mover_handle(run, part);
    let Some(Some(Appearance::Mech(a))) = w.appearance.by_handle.get(handle.0 as usize) else {
        panic!("selected unit is a mech with an appearance");
    };
    let name = w.appearance.mech_types[a.type_index].name.clone();
    let mut vfs = Vfs::from_system_cfg(data().unwrap()).expect("vfs");
    let mut fit = vfs
        .read_fit(&format!("data/tgl/{name}.ini"))
        .expect("read")
        .expect("mech appearance ini");
    if fit.seek_block("3DBounds").is_ok() {
        // Designer-authored box, stored verbatim in game axes (`apprtype.cpp:80-121`).
        let rd = |k: &str| fit.read_i64(k).expect("3DBounds key") as f32;
        return (
            [rd("UpperLeftX"), rd("UpperLeftY"), rd("UpperLeftZ")],
            [rd("LowerRightX"), rd("LowerRightY"), rd("LowerRightZ")],
        );
    }
    // Otherwise the box is the shape's vertex extents (bind pose: root at its node centre, each
    // child offset by its relative centre; `_PAB`/`LOS_`/`SpotLight` nodes excluded), expanded by
    // `EXPAND_FACTOR` 1.25 and stored as (x, z, y) (`mech3d.cpp:100`, `1282-1299`, `msl.cpp:236-270`).
    fit.seek_block("TGLData").expect("[TGLData]");
    let file = fit
        .read_string("FileName", 511)
        .or_else(|_| fit.read_string("FileName0", 511))
        .expect("mech shape file name");
    let bytes = vfs
        .read(&format!("data/tgl/{}.tgl", file.to_ascii_lowercase()))
        .expect("read")
        .expect("mech tgl");
    let shape = mc2_assets::tgl::decode_shape(&bytes).expect("tgl");
    let mut origin = vec![[0.0f32; 3]; shape.nodes.len()];
    for (i, n) in shape.nodes.iter().enumerate() {
        origin[i] = match n.parent_index {
            None => n.node_center,
            Some(p) => {
                let (po, rc) = (origin[p], n.relative_node_center);
                [po[0] + rc[0], po[1] + rc[1], po[2] + rc[2]]
            }
        };
    }
    let (mut mn, mut mx) = ([f32::MAX; 3], [f32::MIN; 3]);
    for (i, n) in shape.nodes.iter().enumerate() {
        let id = n.node_id.to_ascii_lowercase();
        if id.starts_with("_pab") || id.starts_with("los_") || id.starts_with("spotlight") {
            continue;
        }
        let Some(sh) = &n.shape else { continue };
        for v in &sh.vertices {
            for a in 0..3 {
                let q = v.position[a] + origin[i][a];
                mn[a] = mn[a].min(q);
                mx[a] = mx[a].max(q);
            }
        }
    }
    let game = |m: [f32; 3]| [m[0] * 1.25, m[2] * 1.25, m[1] * 1.25];
    (game(mn), game(mx))
}

/// `Camera::projectZ` written out in the test from the camera's public parameters (target,
/// orbit yaw/tilt/altitude, fov, viewport): world (x,y,z) -> Stuff (-x,z,y), camera axes of
/// `BuildRotation(-tilt, yaw)`, then the perspective divide of `setOrthogonal`; game pixels.
fn test_project(cam: &GameCamera, p: [f32; 3]) -> (f32, f32) {
    let (tilt, yaw) = (cam.projection_angle.to_radians(), cam.rotation.to_radians());
    let (st, ct) = tilt.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let tgt = [-cam.target.x, cam.target.z, cam.target.y];
    let eye = [
        tgt[0] + cam.altitude * ct * sy,
        tgt[1] + cam.altitude * st,
        tgt[2] + cam.altitude * ct * cy,
    ];
    let d = [-p[0] - eye[0], p[2] - eye[1], p[1] - eye[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let cx = dot([cy, 0.0, -sy], d);
    let cyy = dot([-st * sy, ct, -st * cy], d);
    let cz = dot([ct * sy, st, ct * cy], d);
    let t = (cam.settings.fov_degrees.to_radians() * 0.5).tan();
    let h2w = cam.viewport.1 / cam.viewport.0;
    (
        (0.5 - cx / (2.0 * t * cz)) * cam.viewport.0,
        (0.5 + cyy / (2.0 * t * h2w * cz)) * cam.viewport.1,
    )
}

/// Expected selection box: the eight corners of the raw shipped `[3DBounds]` added to the
/// unit's world position (`mech3d.cpp:2173-2203`), projected by the test's own camera math.
/// Compared with rendered pixels within `BOX_TOLERANCE_PX` (root-node height offset and
/// sub-pixel rounding are not modelled).
const BOX_TOLERANCE_PX: f32 = 6.0;

fn independent_box(run: &HeadlessRun<LiveSim>, part: i32) -> [f32; 4] {
    let (lo, hi) = raw_appearance_extents(run, part);
    let (wx, wy, wz) = world_pos(run, part);
    let cam = camera_of(run);
    let k = camera_k(run);
    let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for i in 0..8 {
        let c = [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let (px, py) = test_project(&cam, [wx + c[0], wy + c[1], wz + c[2]]);
        let (px, py) = (px * k, py * k);
        min = (min.0.min(px), min.1.min(py));
        max = (max.0.max(px), max.1.max(py));
    }
    assert!(
        max.0 - min.0 > 4.0 && max.1 - min.1 > 4.0,
        "non-degenerate box"
    );
    [min.0, min.1, max.0, max.1]
}

fn expected_range(los: bool) -> u32 {
    expected_attack(los)
}

fn tmp_png(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hud-brackets");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn select_lance(run: &mut HeadlessRun<LiveSim>) {
    key(run, "E");
    assert!(
        !hud(run).selection.selected.is_empty(),
        "E selects the visible lance"
    );
}

fn is_green(rgba: &[u8], w: u32, x: u32, y: u32) -> bool {
    let i = ((y * w + x) * 4) as usize;
    if i + 2 >= rgba.len() {
        return false;
    }
    let (r, g, b) = (rgba[i], rgba[i + 1], rgba[i + 2]);
    g > r.saturating_add(40) && g > b.saturating_add(40) && g > 80
}

fn sample(rgba: &[u8], w: u32, h: u32, x: i32, y: i32) -> bool {
    if x < 0 || y < 0 {
        return false;
    }
    let (x, y) = (x as u32, y as u32);
    x < w && y < h && is_green(rgba, w, x, y)
}

/// Cursor ids listed in the shipped `cursors.fit` (`MouseCursorData::initCursors`).
fn shipped_cursor_ids(data: &std::path::Path) -> Vec<u32> {
    let mut vfs = Vfs::from_system_cfg(data).expect("vfs");
    let mut fit = vfs
        .read_fit("data/art/cursors.fit")
        .expect("read")
        .expect("cursors.fit");
    fit.seek_block("Main").expect("Main");
    let count = fit.read_i64("NumCursors").unwrap_or(0).max(0) as u32;
    (0..count)
        .filter(|&i| fit.seek_block(&format!("Cursor{i}")).is_ok())
        .collect()
}

fn expected_attack(los: bool) -> u32 {
    if los {
        state::ATTACK_LOS
    } else {
        state::GENERIC_ATTACK
    }
}

fn is_walk(cursor: u32, los: bool) -> bool {
    if los {
        (state::WALKLOS1..state::WALKLOS1 + 32).contains(&cursor)
    } else {
        (state::WALK1..state::WALK1 + 32).contains(&cursor)
    }
}

fn is_jump(cursor: u32) -> bool {
    (state::JUMP1..state::JUMP1 + 32).contains(&cursor)
        || (state::JUMPLOS1..state::JUMPLOS1 + 32).contains(&cursor)
        || cursor == state::JUMP
        || cursor == state::JUMP_LOS
        || cursor == state::JUMPWAYPT
        || cursor == state::JUMPWAYPT_LOS
}

fn open_terrain(run: &HeadlessRun<LiveSim>) -> (i32, i32) {
    let k = camera_k(run);
    for y in (200..700).step_by(40) {
        for x in (400..1400).step_by(40) {
            if hud(run).hit_test((x, y)) != HitTarget::World {
                continue;
            }
            let hit_pick = hud(run).snapshot().picks.iter().any(|p| {
                let [l, t, r, b] = p.screen_box.map(|v| v * k);
                (x as f32) >= l && (x as f32) <= r && (y as f32) >= t && (y as f32) <= b
            });
            if !hit_pick {
                return (x, y);
            }
        }
    }
    panic!("open terrain in the world view");
}

fn hover_enemy(run: &mut HeadlessRun<LiveSim>) -> Option<(f32, f32, bool)> {
    let k = camera_k(run);
    let mut enemies: Vec<_> = hud(run)
        .snapshot()
        .picks
        .iter()
        .filter(|p| p.team == TacTeam::Enemy && !p.disabled && (p.mover || !p.triangles.is_empty()))
        .cloned()
        .collect();
    enemies.sort_by_key(|p| !p.mover);
    for p in &enemies {
        let [l, t, r, b] = p.screen_box.map(|v| v * k);
        let (mx, my) = ((l + r) / 2.0, (t + b) / 2.0);
        if mx < 20.0 || my < 20.0 || mx > 1900.0 || my > 1000.0 {
            continue;
        }
        if hud(run).hit_test((mx as i32, my as i32)) != HitTarget::World {
            continue;
        }
        if r - l < 4.0 || b - t < 4.0 {
            continue;
        }
        mouse(run, f64::from(mx), f64::from(my));
        run.step(None).unwrap();
        let los = hud(run).cursor_line_of_sight();
        if hud(run).order_cursor() == expected_attack(los) {
            return Some((mx, my, los));
        }
    }
    None
}

#[test]
fn selecting_a_mech_draws_bracket_pixels_around_its_projected_box() {
    let Some(mut run) = start(130) else { return };
    let unselected = tmp_png("brackets-unselected.png");
    run.step(Some(&unselected)).unwrap();
    let (uw, uh, urgba) = read_png(&unselected);
    assert_eq!((uw, uh), (1920, 1080));

    select_lance(&mut run);
    let part = hud(&run).selection.selected[0];
    let (sx, sy) = screen_of(&run, part);
    assert!(
        sx > 40.0 && sy > 40.0 && sx < 1880.0 && sy < 1000.0,
        "selected unit is on screen at ({sx}, {sy})"
    );
    // Appearance `recalcBounds` box (declared input) plus an independent `projectZ` of the
    // unit's world position through the camera — not `HudSnapshot.screen_box`.
    let box_px = independent_box(&run, part);
    let shot = tmp_png("brackets.png");
    run.step(Some(&shot)).unwrap();
    let (w, h, rgba) = read_png(&shot);
    assert_eq!((w, h), (1920, 1080));
    let scale = hud(&run).scale_factor();
    let o = (5.0 * scale).max(1.0);
    let [l, t, r, b] = box_px;
    let corners = [
        (l, t),
        (l + o, t),
        (l, t + o),
        (r, t),
        (r - o, t),
        (r, t + o),
        (l, b),
        (l + o, b),
        (l, b - o),
        (r, b),
        (r - o, b),
        (r, b - o),
    ];
    let mut new_green = 0u32;
    for (x, y) in corners {
        let (ix, iy) = (x.round() as i32, y.round() as i32);
        let tol = BOX_TOLERANCE_PX as i32;
        for dx in -tol..=tol {
            for dy in -tol..=tol {
                if sample(&rgba, w, h, ix + dx, iy + dy)
                    && !sample(&urgba, uw, uh, ix + dx, iy + dy)
                {
                    new_green += 1;
                }
            }
        }
    }
    assert!(
        new_green >= 8,
        "bracket ticks at the projected-box corners were not drawn (new green samples {new_green}, box {box_px:?})"
    );

    let marks = hud(&run).selected_marks();
    assert!(!marks.is_empty());
    let mech = hud(&run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.part_id == part)
        .unwrap();
    if mech.info.is_mech {
        assert_eq!(marks[0].bar_color, 0x0000_ff00, "home mech barColor");
    }
    let bar_top = (box_px[1] - 8.0 * scale - 4.0 * scale) as i32;
    let bar_mid_x = ((box_px[0] + box_px[2]) / 2.0) as i32;
    let mut bar_green = 0u32;
    for dx in -20..=20 {
        for dy in -2..=6 {
            if sample(&rgba, w, h, bar_mid_x + dx, bar_top + dy) {
                bar_green += 1;
            }
        }
    }
    assert!(
        bar_green >= 4,
        "health bar fill at the C++ bar location ({bar_mid_x},{bar_top}) is the unit colour"
    );

    if std::env::var_os("MC2_WRITE_HUD_BRACKETS_PNG").is_some() {
        let left = (sx - 80.0).max(0.0) as u32;
        let top = (sy - 80.0).max(0.0) as u32;
        let right = (sx + 80.0).min(w as f32 - 1.0) as u32;
        let bottom = (sy + 80.0).min(h as f32 - 1.0) as u32;
        write_docs_png(&rgba, w, h, left, top, right, bottom);
    }
}

#[test]
fn hover_enemy_with_lance_selected_is_the_attack_cursor_from_shipped_art() {
    let data = match data() {
        Some(d) => d,
        None => return,
    };
    let shipped = shipped_cursor_ids(&data);
    assert!(
        shipped.contains(&state::GENERIC_ATTACK) && shipped.contains(&state::ATTACK_LOS),
        "cursors.fit lists the attack states: {shipped:?}"
    );
    let Some(mut run) = start(130) else { return };
    assert!(hud(&run).draws_cursor(), "shipped cursor art loaded");
    select_lance(&mut run);
    let mut last_cursor = hud(&run).order_cursor();
    let mut tried = Vec::new();
    for _ in 0..200 {
        let k = 1080.0
            / run
                .core
                .mission
                .as_ref()
                .unwrap()
                .rig
                .view()
                .viewport
                .1
                .max(1.0);
        let enemies: Vec<_> = hud(&run)
            .snapshot()
            .picks
            .iter()
            .filter(|p| {
                p.team == TacTeam::Enemy && !p.disabled && (p.mover || !p.triangles.is_empty())
            })
            .cloned()
            .collect();
        let mut enemies = enemies;
        enemies.sort_by_key(|p| !p.mover);
        for p in &enemies {
            let [l, t, r, b] = p.screen_box.map(|v| v * k);
            let (mx, my) = ((l + r) / 2.0, (t + b) / 2.0);
            if mx < 20.0 || my < 20.0 || mx > 1900.0 || my > 1000.0 {
                continue;
            }
            if hud(&run).hit_test((mx as i32, my as i32)) != HitTarget::World {
                continue;
            }
            if r - l < 4.0 || b - t < 4.0 {
                continue;
            }
            mouse(&mut run, f64::from(mx), f64::from(my));
            run.step(None).unwrap();
            last_cursor = hud(&run).order_cursor();
            let los = hud(&run).cursor_line_of_sight();
            tried.push((p.watch_id, p.mover, [l, t, r, b], last_cursor, los));
            let want = expected_attack(los);
            if last_cursor == want {
                assert!(
                    shipped.contains(&last_cursor),
                    "cursor {last_cursor} is a shipped mState"
                );
                return;
            }
        }
        run.step(None).unwrap();
    }
    panic!("no attack cursor over an enemy matching team LOS; last={last_cursor}, tried={tried:?}");
}

#[test]
fn hover_terrain_with_lance_selected_is_the_move_cursor() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let (x, y) = open_terrain(&run);
    mouse(&mut run, f64::from(x), f64::from(y));
    run.step(None).unwrap();
    let cursor = hud(&run).order_cursor();
    let los = hud(&run).cursor_line_of_sight();
    assert!(
        is_walk(cursor, los),
        "hovering terrain with a lance selected is the move cursor for team LOS {los}, got {cursor}"
    );
}

#[test]
fn holding_f_shows_the_force_fire_range_cursor() {
    let data = match data() {
        Some(d) => d,
        None => return,
    };
    let shipped = shipped_cursor_ids(&data);
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let (x, y) = open_terrain(&run);
    mouse(&mut run, f64::from(x), f64::from(y));
    run.step(None).unwrap();
    let walk = hud(&run).order_cursor();
    let los = hud(&run).cursor_line_of_sight();
    assert!(
        is_walk(walk, los),
        "terrain without F is the move cursor, got {walk}"
    );

    hold_key(&mut run, "F", true);
    mouse(&mut run, f64::from(x), f64::from(y));
    run.step(None).unwrap();
    let held = hud(&run).order_cursor();
    let los_held = hud(&run).cursor_line_of_sight();
    let want = expected_range(los_held);
    assert_eq!(
        held, want,
        "F held over terrain is makeRangeCursor (`missiongui.cpp:3991-3995`), got {held}"
    );
    assert!(
        shipped.contains(&held),
        "force-fire cursor {held} is a shipped mState"
    );

    // A hoverable enemy is required: step the mission until one is on screen and pickable.
    hold_key(&mut run, "F", false);
    let mut enemy = None;
    for _ in 0..200 {
        enemy = hover_enemy(&mut run);
        if enemy.is_some() {
            break;
        }
        run.step(None).unwrap();
    }
    let (ex, ey, _) = enemy.expect("a hoverable enemy target for the F-over-target case");
    // Same pointer position, F not held: it resolves to the enemy (shipped attack cursor id).
    let hovered = hud(&run).order_cursor();
    let los_h = hud(&run).cursor_line_of_sight();
    assert_eq!(
        hovered,
        expected_attack(los_h),
        "pointer over the enemy without F is the attack cursor, got {hovered}"
    );
    assert!(
        shipped.contains(&hovered),
        "attack cursor {hovered} is shipped"
    );
    hold_key(&mut run, "F", true);
    mouse(&mut run, f64::from(ex), f64::from(ey));
    run.step(None).unwrap();
    let over_target = hud(&run).order_cursor();
    let los_t = hud(&run).cursor_line_of_sight();
    assert_eq!(
        over_target,
        expected_range(los_t),
        "F held over a target is makeRangeCursor (`missiongui.cpp:3929-3932`), got {over_target}"
    );
    // Both branches end in `makeRangeCursor` (`missiongui.cpp:3929-3932`), so at the default
    // optimal range the attack and force-fire ids coincide; F must not lose the attack id.
    assert_eq!(over_target, hovered);
    assert!(
        shipped.contains(&over_target),
        "force-fire cursor {over_target} is a shipped mState"
    );
    mouse(&mut run, f64::from(x), f64::from(y));
    run.step(None).unwrap();

    hold_key(&mut run, "F", false);
    mouse(&mut run, f64::from(x), f64::from(y));
    run.step(None).unwrap();
    let released = hud(&run).order_cursor();
    let los_rel = hud(&run).cursor_line_of_sight();
    assert!(
        is_walk(released, los_rel),
        "releasing F restores the move cursor, got {released}"
    );
    assert_ne!(
        released, held,
        "releasing F changes the cursor off the force-fire range state"
    );
}

#[test]
fn missing_projection_bounds_do_not_invent_brackets() {
    let Some(mut run) = start(130) else { return };
    let unselected = tmp_png("missing-bounds-unselected.png");
    run.step(Some(&unselected)).unwrap();
    let (uw, uh, urgba) = read_png(&unselected);

    select_lance(&mut run);
    let part = hud(&run).selection.selected[0];
    let (sx, sy) = screen_of(&run, part);
    click(&mut run, f64::from(sx), f64::from(sy));
    run.step(None).unwrap();
    assert_eq!(hud(&run).selection.selected, vec![part]);
    assert!(
        run.core
            .mission
            .as_ref()
            .unwrap()
            .driver
            .pick_boxes()
            .movers
            .contains_key(&part),
        "the real-bounds case has a recalcBounds box"
    );
    assert!(
        !hud(&run).selected_marks().is_empty(),
        "real projected bounds draw a mark"
    );

    run.core
        .mission
        .as_mut()
        .unwrap()
        .driver
        .pick_boxes_mut()
        .movers
        .remove(&part);
    let missing = tmp_png("missing-bounds.png");
    run.step(Some(&missing)).unwrap();
    assert!(
        hud(&run).selected_marks().is_empty(),
        "a selected unit without a projected box is not given guessed brackets"
    );
    assert!(
        hud(&run)
            .unavailable_summary()
            .iter()
            .any(|s| s.contains("selection brackets: projected box is missing")),
        "missing bounds stay explicit: {:?}",
        hud(&run).unavailable_summary()
    );

    let guessed = [sx - 24.0, sy - 48.0, sx + 24.0, sy + 16.0];
    let (w, h, rgba) = read_png(&missing);
    let scale = hud(&run).scale_factor();
    let o = (5.0 * scale).max(1.0);
    let [l, t, r, b] = guessed;
    let mut new_green = 0u32;
    for (x, y) in [
        (l, t),
        (l + o, t),
        (l, t + o),
        (r, t),
        (r - o, t),
        (r, t + o),
        (l, b),
        (l + o, b),
        (l, b - o),
        (r, b),
        (r - o, b),
        (r, b - o),
    ] {
        let (ix, iy) = (x.round() as i32, y.round() as i32);
        if sample(&rgba, w, h, ix, iy) && !sample(&urgba, uw, uh, ix, iy) {
            new_green += 1;
        }
    }
    assert!(
        new_green < 4,
        "guessed 48×64 corners must not grow bracket ticks (new green {new_green})"
    );
}

#[test]
fn info_window_click_scrolls_like_the_cpp() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    // TacMap is the default tab; InfoWindow::render (which sets infoLength) runs on the Info tab
    // (`mission_hud.rs` `MissionTab::Info`). TAB is `switchTabs(+1)`: TacMap -> Vehicle -> Info.
    key(&mut run, "Tab");
    key(&mut run, "Tab");
    let part = hud(&run).selection.selected[0];
    let (sx, sy) = screen_of(&run, part);
    click(&mut run, f64::from(sx), f64::from(sy));
    run.step(None).unwrap();
    assert_eq!(
        hud(&run).hit_test({
            let down = hud(&run).info_window().geometry().down_button;
            let (gx, gy) =
                hud(&run).hud_to_game(((down.left + down.right) / 2, (down.top + down.bottom) / 2));
            (gx as i32, gy as i32)
        }),
        HitTarget::Info,
        "the info tab is showing, so the down button is an info-window hit"
    );
    let info = hud(&run).info_window();
    assert!(
        info.unit().is_some(),
        "a selected unit fills the info window"
    );
    assert!(
        info.info_length() > info.scroll_bar_length() as f32,
        "the fixture needs an overflowing info window (C++ enables the bar only then); length {} track {}",
        info.info_length(),
        info.scroll_bar_length()
    );
    let before = hud(&run).info_window().scroll_pos();
    let down = hud(&run).info_window().geometry().down_button;
    let (gx, gy) =
        hud(&run).hud_to_game(((down.left + down.right) / 2, (down.top + down.bottom) / 2));
    click(&mut run, f64::from(gx), f64::from(gy));
    let after = hud(&run).info_window().scroll_pos();
    assert!(
        after > before,
        "a down-button click scrolls the info window ({before} -> {after})"
    );
}

#[test]
fn jump_cursor_is_dont_when_the_landing_is_out_of_range() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let jumper = hud(&run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.bar.can_jump && hud(&run).selection.selected.contains(&u.part_id))
        .map(|u| u.part_id);
    if let Some(part) = jumper {
        let (sx, sy) = screen_of(&run, part);
        click(&mut run, f64::from(sx), f64::from(sy));
    }
    // `commands[KEY_J].release` is `stopJump` (`missiongui.cpp:203`): jump is armed while J is held.
    let j = script::key_code("J").expect("J");
    for e in script::key_events(j, true) {
        run.event(e);
    }
    run.step(None).unwrap();
    assert!(
        hud(&run).control_panel().jump,
        "holding J arms jump when a jumper is selected"
    );
    let mut saw_invalid = false;
    let mut saw_valid = false;
    let mut first_invalid = None;
    for y in (200..900).step_by(50) {
        for x in (200..1700).step_by(50) {
            if hud(&run).hit_test((x, y)) != HitTarget::World {
                continue;
            }
            mouse(&mut run, f64::from(x), f64::from(y));
            run.step(None).unwrap();
            if hud(&run).hit_test((x, y)) != HitTarget::World {
                continue;
            }
            if !hud(&run).control_panel().jump {
                continue;
            }
            let cursor = hud(&run).order_cursor();
            if cursor == state::NORMAL {
                continue;
            }
            if !hud(&run).cursor_can_jump() {
                assert_eq!(
                    cursor,
                    state::DONT,
                    "jump mode without canJumpToWPos is mState_DONT, got {cursor}"
                );
                if !saw_invalid {
                    first_invalid = Some((x, y));
                    saw_invalid = true;
                }
            } else {
                assert!(
                    is_jump(cursor),
                    "canJumpToWPos is a jump cursor, got {cursor}"
                );
                saw_valid = true;
            }
            if saw_invalid && saw_valid {
                break;
            }
        }
        if saw_invalid && saw_valid {
            break;
        }
    }
    if !saw_valid {
        if let Some(part) = jumper {
            let (sx, sy) = screen_of(&run, part);
            for (dx, dy) in [(40.0, 40.0), (-40.0, 40.0), (60.0, 0.0), (0.0, 60.0)] {
                let (x, y) = ((sx + dx) as i32, (sy + dy) as i32);
                if hud(&run).hit_test((x, y)) != HitTarget::World {
                    continue;
                }
                mouse(&mut run, f64::from(x), f64::from(y));
                run.step(None).unwrap();
                if hud(&run).cursor_can_jump() {
                    let cursor = hud(&run).order_cursor();
                    assert!(
                        is_jump(cursor),
                        "canJumpToWPos is a jump cursor, got {cursor}"
                    );
                    saw_valid = true;
                    break;
                }
            }
        }
    }
    // A refused jump click may drop the selection, so it is made after the scan.
    if let Some((x, y)) = first_invalid {
        mouse(&mut run, f64::from(x), f64::from(y));
        run.step(None).unwrap();
        click(&mut run, f64::from(x), f64::from(y));
        assert_eq!(
            hud(&run).order_cursor(),
            state::DONT,
            "a click on an invalid jump landing keeps the DONT cursor"
        );
        assert!(
            hud(&run)
                .unavailable_summary()
                .iter()
                .all(|s| !s.contains("landing passability and jump range are missing")),
            "jump facts are supplied; the order is refused because the landing is invalid, not because they are absent"
        );
    }
    assert!(
        saw_invalid,
        "need a passable hover the jump facts reject (out of range or cannot jump)"
    );
    assert!(
        saw_valid,
        "need a valid jump landing on screen (missiongui.cpp:4065-4080)"
    );
}

#[test]
fn hovering_the_info_window_is_the_normal_cursor_even_over_a_world_object() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    run.step(None).unwrap();
    let info = hud(&run).info_window().geometry().name_box;
    let (gx, gy) =
        hud(&run).hud_to_game(((info.left + info.right) / 2, (info.top + info.bottom) / 2));
    mouse(&mut run, f64::from(gx), f64::from(gy));
    run.step(None).unwrap();
    assert_eq!(
        hud(&run).hit_test((gx as i32, gy as i32)),
        HitTarget::Info,
        "the click stays on the info window"
    );
    assert_eq!(
        hud(&run).order_cursor(),
        state::NORMAL,
        "GUI under the pointer is mState_NORMAL (`missiongui.cpp:736-737, 1296-1306`)"
    );
}

fn watch_handle(run: &HeadlessRun<LiveSim>, watch: u32) -> mc2_sim::ObjectHandle {
    world_of(run)
        .objects
        .by_watch_id(WatchId(watch))
        .expect("watch id")
}

fn hover_pick(run: &mut HeadlessRun<LiveSim>, p: &mc2_app::hud_data::PickTarget) -> Option<u32> {
    let k = camera_k(run);
    let [l, t, r, b] = p.screen_box.map(|v| v * k);
    let (mx, my) = ((l + r) / 2.0, (t + b) / 2.0);
    if mx < 20.0 || my < 20.0 || mx > 1900.0 || my > 1000.0 {
        return None;
    }
    if hud(run).hit_test((mx as i32, my as i32)) != HitTarget::World {
        return None;
    }
    mouse(run, f64::from(mx), f64::from(my));
    run.step(None).unwrap();
    Some(hud(run).order_cursor())
}

#[test]
fn structure_picks_keep_bridge_and_landbridge_classes() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let picks = hud(&run).snapshot().picks.clone();
    assert!(
        picks.iter().filter(|p| p.mover).all(|p| matches!(
            p.class,
            ObjectClass::Mech | ObjectClass::GroundVehicle | ObjectClass::Elemental
        )),
        "movers are not collapsed to Building"
    );
    // Fixture targets must stay picked across the steps below: only keep structures that are
    // still picked after the same number of unmutated steps.
    for _ in 0..3 {
        run.step(None).unwrap();
    }
    let stable: Vec<u32> = hud(&run)
        .snapshot()
        .picks
        .iter()
        .map(|p| p.watch_id)
        .collect();
    let mut structures: Vec<_> = picks
        .iter()
        .filter(|p| !p.mover && !p.triangles.is_empty() && stable.contains(&p.watch_id))
        .filter(|p| {
            let k = camera_k(&run);
            let [l, t, r, b] = p.screen_box.map(|v| v * k);
            let (mx, my) = (((l + r) / 2.0) as i32, ((t + b) / 2.0) as i32);
            l > 40.0
                && t > 40.0
                && r < 1880.0
                && b < 980.0
                && hud(&run).hit_test((mx, my)) == HitTarget::World
        })
        .cloned()
        .collect();
    structures.sort_by(|a, b| {
        let mid = camera_of(&run).viewport.1 * 0.5;
        let ay = ((a.screen_box[1] + a.screen_box[3]) * 0.5 - mid).abs();
        let by = ((b.screen_box[1] + b.screen_box[3]) * 0.5 - mid).abs();
        ay.total_cmp(&by)
    });
    assert!(
        structures.len() >= 2,
        "need two on-screen structures to host bridge and landbridge fixtures"
    );

    {
        let h = watch_handle(&run, structures[0].watch_id);
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        if let Some(o) = w.objects.get_mut(h) {
            o.c.type_handle = LAND_BRIDGE_TYPE_INDEX as i32;
        }
    }
    {
        let h = watch_handle(&run, structures[1].watch_id);
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        if let Some(o) = w.objects.get_mut(h) {
            o.c.class = mc2_sim::object::ObjectClass::Bridge;
        }
    }
    // One frame publishes the altered object class to the HUD. Further frames
    // advance the authored cinematic camera and can move this visual fixture
    // behind the HUD without adding evidence.
    for _ in 0..1 {
        run.step(None).unwrap();
    }

    let picks = hud(&run).snapshot().picks.clone();
    let land = picks
        .iter()
        .find(|p| p.watch_id == structures[0].watch_id && p.landbridge)
        .cloned()
        .expect("a landbridge pick after the type_handle fixture");
    let bridge = picks
        .iter()
        .find(|p| p.watch_id == structures[1].watch_id && p.class == ObjectClass::Bridge)
        .cloned()
        .expect("a Bridge pick after the class fixture");

    let land_cursor = hover_pick(&mut run, &land).expect("landbridge is hoverable");
    if hud(&run).cursor_passable() {
        assert!(
            (state::WALK1..state::WALK1 + 32).contains(&land_cursor),
            "a passable landbridge is the no-target walk cursor (`missiongui.cpp:3858-3880`), got {land_cursor}"
        );
    } else {
        assert_eq!(
            land_cursor,
            state::DONT,
            "an impassable landbridge is mState_DONT, got {land_cursor}"
        );
    }

    let bridge_cursor = hover_pick(&mut run, &bridge).expect("bridge is hoverable");
    assert_eq!(
        bridge_cursor,
        state::DONT,
        "a Bridge is makeNoTargetCursor(false, ...) (`missiongui.cpp:3909-3912`), got {bridge_cursor}"
    );
}

#[test]
fn hidden_info_window_does_not_scroll_on_other_tabs() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    open_info_tab(&mut run);
    let part = hud(&run).selection.selected[0];
    let (sx, sy) = screen_of(&run, part);
    click(&mut run, f64::from(sx), f64::from(sy));
    run.step(None).unwrap();
    assert!(
        hud(&run).info_window().info_length() > hud(&run).info_window().scroll_bar_length() as f32,
        "need overflowing content so the down button is live"
    );
    let down = hud(&run).info_window().geometry().down_button;
    let (gx, gy) =
        hud(&run).hud_to_game(((down.left + down.right) / 2, (down.top + down.bottom) / 2));
    let before = hud(&run).info_window().scroll_pos();
    click(&mut run, f64::from(gx), f64::from(gy));
    let after_info = hud(&run).info_window().scroll_pos();
    assert!(
        after_info > before,
        "Info tab: a down-button click scrolls ({before} -> {after_info})"
    );
    key(&mut run, "Tab");
    assert_eq!(hud(&run).tab(), MissionTab::TacMap);
    click(&mut run, f64::from(gx), f64::from(gy));
    assert_eq!(
        hud(&run).info_window().scroll_pos(),
        after_info,
        "TacMap tab: the hidden info window ignores the click (controlgui.cpp:1200-1222)"
    );
    key(&mut run, "Tab");
    assert_eq!(hud(&run).tab(), MissionTab::Vehicle);
    click(&mut run, f64::from(gx), f64::from(gy));
    assert_eq!(
        hud(&run).info_window().scroll_pos(),
        after_info,
        "Vehicle tab: the hidden info window ignores the click"
    );
}

#[test]
fn order_cursor_is_not_drawn_after_leaving_the_window() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    key(&mut run, "P");
    mouse(&mut run, 400.0, 400.0);
    let inside = tmp_png("cursor-inside.png");
    run.step(Some(&inside)).unwrap();
    assert!(hud(&run).cursor_inside(), "CursorMoved enters the window");
    assert!(hud(&run).draws_cursor());
    let (w, _, with) = read_png(&inside);
    run.event(InputEvent::CursorLeft);
    let left = tmp_png("cursor-left.png");
    run.step(Some(&left)).unwrap();
    assert!(
        !hud(&run).cursor_inside(),
        "CursorLeft leaves the window (game.rs:878)"
    );
    let (_, _, without) = read_png(&left);
    let changed = region_diff(&with, &without, w, 360, 360, 440, 440);
    assert!(
        changed > 0,
        "leaving the window removes the cursor that was at (400,400); changed={changed}"
    );
}

#[test]
fn hold_fire_draws_the_shipped_blip_icon() {
    let data = match data() {
        Some(d) => d,
        None => return,
    };
    let shipped = shipped_blip_icon(&data);
    assert!(
        shipped.rgba.iter().any(|&b| b != 0),
        "blip.tga's drawIcon region is not empty"
    );
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let part = hud(&run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.info.is_mech && hud(&run).selection.selected.contains(&u.part_id))
        .map(|u| u.part_id)
        .expect("a selected mech");
    let hud_icon = hud(&run)
        .hold_fire_icon()
        .cloned()
        .expect("HudRuntime loaded blip.tga");
    assert_eq!(
        hud_icon, shipped,
        "production HUD samples the same drawIcon UV region the test reads from blip.tga"
    );
    let off = tmp_png("hold-fire-off.png");
    run.step(Some(&off)).unwrap();
    let (w, _, before) = read_png(&off);
    set_attack_range(&mut run, part, FIRERANGE_CURRENT);
    run.step(None).unwrap();
    let mark = hud(&run)
        .selected_marks()
        .first()
        .copied()
        .expect("selected mark");
    assert!(mark.hold_fire, "FIRERANGE_CURRENT sets the hold-fire bit");
    let on = tmp_png("hold-fire-on.png");
    run.step(Some(&on)).unwrap();
    let (_, _, after) = read_png(&on);
    let native = 1080.0
        / run
            .core
            .mission
            .as_ref()
            .unwrap()
            .rig
            .view()
            .viewport
            .1
            .max(1.0);
    let box_px = mark.box_px.map(|v| v * native.max(0.5));
    let (ix, iy) =
        world_view::hold_fire_icon_origin(box_px, hud(&run).scale_factor() * native.max(0.5));
    let changed = region_diff(
        &before,
        &after,
        w,
        ix.floor() as i32 - 1,
        iy.floor() as i32 - 1,
        ix.floor() as i32 + 6,
        iy.floor() as i32 + 6,
    );
    assert!(
        changed > 0,
        "hold-fire icon pixels appear at the C++ origin ({ix},{iy}); changed={changed}"
    );
    let mut opaque = 0u32;
    for y in 0..5 {
        for x in 0..5 {
            let i = ((y * 5 + x) * 4) as usize;
            if shipped.rgba[i + 3] == 0 {
                continue;
            }
            opaque += 1;
            let px = rgba_at(&after, w, ix.floor() as i32 + x, iy.floor() as i32 + y)
                .expect("icon texel on screen");
            assert_ne!(
                px,
                rgba_at(&before, w, ix.floor() as i32 + x, iy.floor() as i32 + y).unwrap(),
                "opaque blip texel ({x},{y}) is drawn"
            );
        }
    }
    assert!(opaque > 0, "the shipped icon has at least one opaque texel");
}

#[test]
fn queued_waypoint_is_the_jump_range_origin() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let jumper = hud(&run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.bar.can_jump && hud(&run).selection.selected.contains(&u.part_id))
        .cloned()
        .expect("a jumper in the lance");
    let (sx, sy) = screen_of(&run, jumper.part_id);
    click(&mut run, f64::from(sx), f64::from(sy));
    let range = jumper.jump_range_wu;
    let pos = jumper.position;
    let waypoint = (pos.0 + range * 0.8, pos.1);
    let h = mover_handle(&run, jumper.part_id);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let slot = match &w.objects.get(h).unwrap().kind {
            Kind::Mech(m) => m.m.pilot_slot,
            Kind::Vehicle(v) => v.m.pilot_slot,
            _ => panic!("jumper is a mover"),
        };
        w.pilots
            .get_mut(slot)
            .unwrap()
            .queue
            .push(mc2_sim::ai::pilot::QueuedOrder {
                point: mc2_sim::math::Vec3 {
                    x: waypoint.0,
                    y: waypoint.1,
                    z: pos.2,
                },
                ..Default::default()
            });
    }
    run.step(None).unwrap();
    let u = hud(&run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.part_id == jumper.part_id)
        .unwrap();
    assert!(
        (u.last_jump_from.0 - waypoint.0).abs() < 1.0
            && (u.last_jump_from.1 - waypoint.1).abs() < 1.0,
        "peekQueuedTacOrder(-1) is the jump origin, got {:?}",
        u.last_jump_from
    );
    hold_key(&mut run, "J", true);
    assert!(hud(&run).control_panel().jump);
    let landing_from_wp = Vec3::new(waypoint.0 + range * 0.5, waypoint.1, pos.2);
    let landing_from_now = Vec3::new(pos.0 - range * 0.5, pos.1, pos.2);
    let selected: Vec<_> = hud(&run)
        .snapshot()
        .units
        .iter()
        .filter(|u| hud(&run).selection.selected.contains(&u.part_id))
        .collect();
    assert!(
        world_view::can_jump_to_wpos(&selected, Some(landing_from_wp), true),
        "landing in range of the queued waypoint is valid"
    );
    assert!(
        !world_view::can_jump_to_wpos(&selected, Some(landing_from_now), true),
        "landing in range of the current position only is invalid once a waypoint is queued"
    );

    let valid = hover_landing(&mut run, landing_from_wp);
    assert!(
        is_jump(valid) && hud(&run).cursor_can_jump(),
        "hovering a landing in range of the queued waypoint is a jump cursor, got {valid}"
    );
    let invalid = hover_landing(&mut run, landing_from_now);
    assert_eq!(
        invalid,
        state::DONT,
        "hovering a landing only in range of the current position is mState_DONT once a waypoint is queued, got {invalid}"
    );
    hold_key(&mut run, "J", false);
}

fn hover_landing(run: &mut HeadlessRun<LiveSim>, landing: Vec3) -> u32 {
    let (sx, sy) = project_world(run, (landing.x, landing.y, landing.z));
    if sx < 40.0 || sy < 40.0 || sx > 1880.0 || sy > 1000.0 {
        pan_to(run, landing.x, landing.y);
        hold_key(run, "J", true);
    }
    let (sx, sy) = project_world(run, (landing.x, landing.y, landing.z));
    mouse(run, f64::from(sx), f64::from(sy));
    run.step(None).unwrap();
    hud(run).order_cursor()
}

fn pan_to(run: &mut HeadlessRun<LiveSim>, x: f32, y: f32) {
    struct G {
        elev: f32,
        side: f32,
        water: f32,
    }
    impl Ground for G {
        fn elevation(&self, _: f32, _: f32) -> f32 {
            self.elev
        }
        fn world_side(&self) -> f32 {
            self.side
        }
        fn water_elevation(&self) -> f32 {
            self.water
        }
    }
    {
        let rt = run.core.mission.as_mut().unwrap();
        let water = rt.view.mission.water.elevation;
        let elev = rt.view.mission.terrain.elevation_at(x, y);
        let side = rt.view.mission.terrain.world_side();
        rt.rig
            .set_position(Vec3::new(x, y, 0.0), false, &G { elev, side, water }, 0.05);
    }
    for _ in 0..8 {
        run.step(None).unwrap();
    }
}

#[test]
fn hovering_info_over_an_enemy_keeps_the_normal_cursor() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    open_info_tab(&mut run);
    let g = hud(&run).info_window().geometry();
    let panel = g.content_box;
    let (gx, gy) = hud(&run).hud_to_game((
        (panel.left + panel.right) / 2,
        (panel.top + panel.bottom) / 2,
    ));
    let rt = run.core.mission.as_ref().unwrap();
    let cam = rt.rig.view();
    let k = 1080.0 / cam.viewport.1.max(1.0);
    let ground = cam
        .pixel_to_ground(gx * k, gy * k, cam.target.z)
        .expect("ground under the info panel");
    let enemy = {
        let w = world_of(&run);
        w.objects
            .movers
            .iter()
            .copied()
            .find(|&h| {
                w.objects.get(h).is_some_and(|o| {
                    o.exists() && !o.c.is_disabled() && o.commander_id() != HOME_COMMANDER
                })
            })
            .expect("an enemy mover")
    };
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let o = w.objects.get_mut(enemy).unwrap();
        o.c.position.x = ground.x;
        o.c.position.y = ground.y;
        o.c.position.z = ground.z;
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    let k = 1080.0
        / run
            .core
            .mission
            .as_ref()
            .unwrap()
            .rig
            .view()
            .viewport
            .1
            .max(1.0);
    let (pl, pt) = hud(&run).hud_to_game((panel.left, panel.top));
    let (pr, pb) = hud(&run).hud_to_game((panel.right, panel.bottom));
    let overlap = hud(&run).snapshot().picks.iter().find_map(|p| {
        if p.team != TacTeam::Enemy {
            return None;
        }
        let [l, t, r, b] = p.screen_box.map(|v| v * k);
        let ol = l.max(pl.min(pr));
        let or = r.min(pl.max(pr));
        let ot = t.max(pt.min(pb));
        let ob = b.min(pt.max(pb));
        (or > ol && ob > ot).then_some(((ol + or) / 2.0, (ot + ob) / 2.0))
    });
    let (hx, hy) = overlap.expect("an enemy's projected box overlaps the info panel");
    mouse(&mut run, f64::from(hx), f64::from(hy));
    run.step(None).unwrap();
    let hit = hud(&run).hit_test((hx as i32, hy as i32));
    assert_ne!(
        hit,
        HitTarget::World,
        "the pointer is on the info panel ({hit:?}), not the world object behind it"
    );
    assert_eq!(
        hud(&run).order_cursor(),
        state::NORMAL,
        "GUI under the pointer is mState_NORMAL even with an enemy behind it"
    );
}

#[test]
fn damaged_mech_and_vehicle_bars_match_fill_length() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let units: Vec<_> = hud(&run)
        .snapshot()
        .units
        .iter()
        .filter(|u| hud(&run).selection.selected.contains(&u.part_id))
        .cloned()
        .collect();
    let mech = units.iter().find(|u| u.info.is_mech).map(|u| u.part_id);
    let vehicle = home_vehicle_part(&mut run);
    let mech = mech.expect("a selected mech");
    let (sx, sy) = screen_of(&run, mech);
    click(&mut run, f64::from(sx), f64::from(sy));
    run.step(None).unwrap();
    let full = tmp_png("bar-mech-full.png");
    run.step(Some(&full)).unwrap();
    let (w, _, full_rgba) = read_png(&full);
    let full_count = bar_colour_samples(&full_rgba, w, hud(&run), true);
    scale_armor(&mut run, mech, 0.4);
    run.step(None).unwrap();
    assert_bar_shorter(&mut run, mech, true, w, full_count);
    let v = vehicle.expect("a home vehicle for the bar-colour fixture");
    hud_mut(&mut run).selection.selected = vec![v];
    run.step(None).unwrap();
    let full_v = tmp_png("bar-veh-full.png");
    run.step(Some(&full_v)).unwrap();
    let (vw, _, full_v_rgba) = read_png(&full_v);
    let full_v_count = bar_colour_samples(&full_v_rgba, vw, hud(&run), false);
    scale_armor(&mut run, v, 0.4);
    run.step(None).unwrap();
    assert_bar_shorter(&mut run, v, false, vw, full_v_count);
}

fn hud_mut(run: &mut HeadlessRun<LiveSim>) -> &mut HudRuntime {
    run.core
        .mission
        .as_mut()
        .unwrap()
        .hud
        .as_mut()
        .expect("the live simulation has a HUD")
}

fn home_vehicle_part(run: &mut HeadlessRun<LiveSim>) -> Option<i32> {
    if let Some(v) = hud(run)
        .snapshot()
        .units
        .iter()
        .find(|u| !u.info.is_mech)
        .map(|u| u.part_id)
    {
        return Some(v);
    }
    let mech_pos = hud(run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.info.is_mech)
        .map(|u| u.position)?;
    let vehicle = {
        let w = world_of(run);
        w.objects.movers.iter().copied().find(|&h| {
            w.objects
                .get(h)
                .is_some_and(|o| matches!(o.kind, Kind::Vehicle(_)) && o.exists())
        })
    }?;
    let part = {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        let o = w.objects.get_mut(vehicle)?;
        if let Kind::Vehicle(v) = &mut o.kind {
            v.m.commander = CommanderId(HOME_COMMANDER as i8);
            v.m.flags.on_gui = true;
        }
        o.c.position.x = mech_pos.0 + 40.0;
        o.c.position.y = mech_pos.1;
        o.c.part_id.0
    };
    for _ in 0..6 {
        run.step(None).unwrap();
    }
    hud(run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.part_id == part && !u.info.is_mech)
        .map(|u| u.part_id)
}

fn is_bar_pixel(rgba: &[u8], w: u32, x: i32, y: i32, mech: bool) -> bool {
    let Some(p) = rgba_at(rgba, w, x, y) else {
        return false;
    };
    if mech {
        is_green(rgba, w, x as u32, y as u32)
    } else {
        let (r, g, b) = (p[0], p[1], p[2]);
        r > 40 && (r.abs_diff(g) < 30) && (g.abs_diff(b) < 30)
    }
}

fn bar_colour_samples(rgba: &[u8], w: u32, hud: &HudRuntime, mech: bool) -> u32 {
    let mark = *hud.selected_marks().first().expect("a selected mark");
    let scale = hud.scale_factor().max(0.5);
    let box_px = mark.box_px;
    let true_width = 19.0 * scale * 2.0;
    let true_height = 4.0 * scale;
    let top_y = box_px[1] - 8.0 * scale - true_height;
    let left_x = ((box_px[0] + box_px[2]) / 2.0 - true_width / 2.0).floor();
    let mut n = 0u32;
    for dy in 0..=3 {
        let y = top_y as i32 + dy;
        for x in (left_x as i32)..(left_x + true_width) as i32 {
            if is_bar_pixel(rgba, w, x, y, mech) {
                n += 1;
            }
        }
    }
    n
}

fn assert_bar_shorter(
    run: &mut HeadlessRun<LiveSim>,
    part: i32,
    mech: bool,
    w: u32,
    full_count: u32,
) {
    let health = hud(run)
        .snapshot()
        .units
        .iter()
        .find(|u| u.part_id == part)
        .unwrap()
        .bar
        .bar_status;
    assert!(
        health > 0.05 && health < 0.95,
        "damaged fixture health is {health}"
    );
    let shot = tmp_png(&format!(
        "bar-{}-{part}.png",
        if mech { "mech" } else { "veh" }
    ));
    run.step(Some(&shot)).unwrap();
    let (_, _, rgba) = read_png(&shot);
    let mark = *hud(run).selected_marks().first().expect("a selected mark");
    if mech {
        assert_eq!(mark.bar_color, 0x0000_ff00);
    } else {
        assert_eq!(mark.bar_color, 0xff7f_7f7f);
    }
    assert!((mark.health - health).abs() < 0.02);
    let damaged_count = bar_colour_samples(&rgba, w, hud(run), mech);
    assert!(
        damaged_count > 0,
        "damaged {part} still draws a health fill"
    );
    assert!(
        damaged_count < full_count,
        "damaged {part} bar is shorter than full health ({damaged_count} < {full_count} samples, health {health})"
    );
}

#[test]
fn team_los_uses_unselected_allies_and_occluded_points() {
    let Some(mut run) = start(130) else { return };
    select_lance(&mut run);
    let home: Vec<_> = hud(&run)
        .snapshot()
        .units
        .iter()
        .filter(|u| !u.bar.destroyed)
        .cloned()
        .collect();
    assert!(home.len() >= 2, "need two home units for the ally fixture");
    let a = home[0].part_id;
    let b = home[1].part_id;
    let ha = mover_handle(&run, a);
    let hb = mover_handle(&run, b);
    let va = mc2_sim::object::hooks::visual_range(world_of(&run), ha).unwrap_or(0.0);
    let vb = mc2_sim::object::hooks::visual_range(world_of(&run), hb).unwrap_or(0.0);
    let (sax, say) = screen_of(&run, a);
    click(&mut run, f64::from(sax), f64::from(say));
    assert_eq!(hud(&run).selection.selected, vec![a]);
    let origin = home[1].position;
    let far = va.max(vb) * 2.0 + 200.0;
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        if let Some(o) = w.objects.get_mut(ha) {
            o.c.position.x = origin.0 + far;
            o.c.position.y = origin.1;
        }
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    assert_eq!(hud(&run).selection.selected, vec![a]);
    let (sbx, sby) = screen_of(&run, b);
    // Ground next to B, not on B's pick box.
    let hover_b = (sbx + 30.0, sby + 30.0);
    mouse(&mut run, f64::from(hover_b.0), f64::from(hover_b.1));
    run.step(None).unwrap();
    let world_b = hud(&run).cursor_mouse_world().expect("ground under B");
    let da = {
        let u = hud(&run)
            .snapshot()
            .units
            .iter()
            .find(|u| u.part_id == a)
            .unwrap();
        (
            (u.position.0 - world_b.x).hypot(u.position.1 - world_b.y),
            va,
        )
    };
    let db = {
        let u = hud(&run)
            .snapshot()
            .units
            .iter()
            .find(|u| u.part_id == b)
            .unwrap();
        (
            (u.position.0 - world_b.x).hypot(u.position.1 - world_b.y),
            vb,
        )
    };
    assert!(
        da.0 > da.1,
        "selected unit A is outside its visual range of the hover ({:?})",
        da
    );
    assert!(
        db.0 <= db.1,
        "unselected ally B is inside its visual range of the hover ({:?})",
        db
    );
    assert!(
        hud(&run).cursor_line_of_sight(),
        "teamLineOfSight is true because of the unselected ally"
    );
    let los_cursor = hud(&run).order_cursor();
    assert!(
        is_walk(los_cursor, true) || los_cursor == expected_attack(true),
        "ally-visible hover is the LOS cursor, got {los_cursor}"
    );

    let max_range = hud(&run)
        .snapshot()
        .units
        .iter()
        .map(|u| {
            mc2_sim::object::hooks::visual_range(world_of(&run), mover_handle(&run, u.part_id))
                .unwrap_or(0.0)
        })
        .fold(0.0_f32, f32::max);
    {
        let w = &mut run.core.mission.as_mut().unwrap().driver.sim_mut().world;
        for h in w.objects.movers.clone() {
            if let Some(o) = w.objects.get_mut(h) {
                if o.commander_id() == HOME_COMMANDER {
                    o.c.position.x = origin.0 + max_range * 3.0 + 400.0;
                    o.c.position.y = origin.1;
                }
            }
        }
    }
    for _ in 0..4 {
        run.step(None).unwrap();
    }
    mouse(&mut run, f64::from(hover_b.0), f64::from(hover_b.1));
    run.step(None).unwrap();
    let wpos = hud(&run)
        .cursor_mouse_world()
        .expect("ground under the old ally hover");
    let all_out = hud(&run).snapshot().units.iter().all(|u| {
        let r = mc2_sim::object::hooks::visual_range(world_of(&run), mover_handle(&run, u.part_id))
            .unwrap_or(0.0);
        (u.position.0 - wpos.x).hypot(u.position.1 - wpos.y) > r
    });
    assert!(
        all_out,
        "every home unit is outside visual range of the occluded hover"
    );
    assert!(
        !hud(&run).cursor_line_of_sight(),
        "a point outside every home visual range is occluded"
    );
    let occluded = hud(&run).order_cursor();
    assert!(
        is_walk(occluded, false) || occluded == expected_attack(false) || occluded == state::DONT,
        "occluded hover is the non-LOS cursor, got {occluded}"
    );
}

/// Palette-quantised crop around the selected unit, written under the cargo target tmp dir (screenshots of game art are not committed).
fn write_docs_png(rgba: &[u8], w: u32, h: u32, left: u32, top: u32, right: u32, bottom: u32) {
    let cw = (right - left + 1).min(w);
    let ch = (bottom - top + 1).min(h);
    let mut crop = Vec::with_capacity((cw * ch * 4) as usize);
    for y in top..top + ch {
        let row = (y * w + left) as usize * 4;
        crop.extend_from_slice(&rgba[row..row + cw as usize * 4]);
    }
    let dest = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hud-brackets-mc2_01.png");
    let mut enc = png::Encoder::new(
        std::io::BufWriter::new(std::fs::File::create(&dest).unwrap()),
        cw,
        ch,
    );
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    enc.set_filter(png::Filter::Adaptive);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(&crop).unwrap();
    wr.finish().unwrap();
    let size = std::fs::metadata(&dest).unwrap().len();
    assert!(size < 1_000_000, "{} is {size} bytes", dest.display());
}
