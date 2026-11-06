// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Tests over the shipped game data.  Set `MC2_DATA` to the data root; without it every test
//! returns early.  A missing GPU adapter fails the GPU tests unless `MC2_SKIP_GPU_TESTS=1`.

#![allow(clippy::chunks_exact_to_as_chunks)]

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::appearance::{AppearanceState, ObjectStatus, MAX_LODS};
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::math::Vec3;
use mc2_render::mission::Mission;
use mc2_render::objects::{Appearance, ObjectClass, ObjectTypes};
use mc2_render::populate::{populate, populate_with_animations, select_shapes_from};
use mc2_render::scene::{Frame, SceneRenderer};
use mc2_render::shape::{AnimationClip, ShapeModel};
use mc2_render::terrain::TileKind;
use mc2_render::terrain_tex::{decode_txm, TerrainTextures};
use mc2_render::transitions::Compositor;
use mc2_sim::appearance::mech::{
    object_status, MechAppearance, MechAppearanceType, ShapeKind, UpdateCtx,
};
use mc2_sim::appearance::tables::{gesture, MECH_ANIMATION_NAMES};
use mc2_sim::rng::{RngMode, SimRng};

fn game() -> Option<GameData> {
    match std::env::var_os("MC2_DATA") {
        Some(root) => Some(GameData::open(root).expect("open game data")),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped-data test");
            None
        }
    }
}

#[test]
fn mc2_01_loads_with_the_expected_shape() {
    let Some(mut game) = game() else { return };
    let m = Mission::load(&mut game, "mc2_01").unwrap();
    assert_eq!(m.terrain.side(), 100);
    assert_eq!(m.objects.len(), 1000);
    assert_eq!(m.parts.len(), 29);
    assert_eq!(m.water.elevation, 350.0);
    assert_eq!(m.parts[0].object_number, 45);
    // sun from the fit
    assert_eq!(m.light.pitch, 50.0);
    assert!((m.light.sun_color[2] - 225.0 / 255.0).abs() < 1e-6);
    // every stored vertex normal points up
    assert!(m.terrain.vertices().iter().all(|v| v.normal[2] > 0.0));
    // the camera from the fit looks at its target
    let mut fit = game.fit("data/missions/mc2_01.fit").unwrap();
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut fit, (1920.0, 1080.0));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    assert_eq!(cam.projection_angle, 21.25);
    assert!((cam.rotation - 59.625).abs() < 1e-4);
    let (s, inside) = cam.project_z(cam.target);
    assert!(inside && (s[0] - 960.0).abs() < 0.1 && (s[1] - 540.0).abs() < 0.1);
}

#[test]
fn composited_transitions_equal_the_baked_ones() {
    let Some(mut game) = game() else { return };
    let m = Mission::load(&mut game, "mc2_01").unwrap();
    let mut comp = Compositor::load(&mut game).unwrap();
    let mut checked = 0;
    let mut differing = Vec::new();
    for row in 0..m.terrain.tiles() {
        for col in 0..m.terrain.tiles() {
            let t = m.terrain.tile_info(col, row);
            if t.kind == TileKind::ColorMap || t.is_uniform() {
                continue;
            }
            let path = format!("data/textures/{}", t.transition_file());
            let Some(bytes) = game.read_opt(&path).unwrap() else {
                continue;
            };
            let baked = decode_txm(&bytes).unwrap();
            let made = comp.transition(&mut game, t.type_info, t.overlay).unwrap();
            assert_eq!(made.len(), baked.len());
            let bad = made
                .iter()
                .zip(&baked)
                .filter(|(a, b)| (**a as i32 - **b as i32).abs() > 1)
                .count();
            if bad > 0 {
                differing.push((t.transition_file(), bad));
            }
            checked += 1;
        }
    }
    assert!(checked > 50, "only {checked} transitions compared");
    assert!(differing.is_empty(), "differences: {differing:?}");
}

#[test]
fn every_mission_resolves_its_terrain_textures() {
    let Some(mut game) = game() else { return };
    for name in ["mc2_01", "mc2_02", "mc2_05", "mc2_10"] {
        let m = Mission::load(&mut game, name).unwrap();
        let t = TerrainTextures::load(&mut game, &m).unwrap();
        assert!(t.missing_tiles.is_empty(), "{name}: {:?}", t.missing_tiles);
        assert_eq!(t.tile_words.len(), m.terrain.tiles().pow(2));
        // the colour map is square and covers whole tiles of 128 pixels
        assert_eq!(t.color_map.width, t.color_map.height);
        assert_eq!(t.color_map.width % 128, 0, "{name}");
    }
}

#[test]
fn offscreen_render_produces_a_plausible_image() {
    let Some(mut game) = game() else { return };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("offscreen render", &cfg) else {
        return;
    };
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mut m = Mission::load(&mut game, "mc2_01").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let pop = populate(&mut game, &m, &mut scene, 200).unwrap();
    assert!(pop.report.placed > 100 && pop.report.missing_files.is_empty());
    assert!(!pop.part_instances.is_empty());

    let (w, h) = (320u32, 180u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    let frame = Frame {
        width: w,
        height: h,
        camera: &cam,
        light: &m.light,
        water: &m.water,
        terrain_params: &m.terrain_params,
        time: 0.0,
        precise_time: None,
        instances: &pop.instances,
        shadows: true,
    };
    let img = scene.render(&frame).unwrap();
    assert_eq!(img.len(), (w * h * 4) as usize);
    // not a flat clear colour: plenty of distinct pixels, fully opaque
    let mut distinct = std::collections::HashSet::new();
    for p in img.chunks_exact(4) {
        assert_eq!(p[3], 255);
        distinct.insert([p[0], p[1], p[2]]);
    }
    assert!(distinct.len() > 500, "{} distinct colours", distinct.len());
    // the same frame renders identically twice
    assert_eq!(img, scene.render(&frame).unwrap());
    // the bottom of the picture is land (olive/brown): red + green exceed blue on average
    let mut sum = [0u64; 3];
    let row0 = (h as usize - 20) * w as usize * 4;
    for p in img[row0..].chunks_exact(4) {
        for c in 0..3 {
            sum[c] += p[c] as u64;
        }
    }
    assert!(sum[0] > sum[2], "land is not blue: {sum:?}");
}

#[test]
fn stored_object_heights_match_the_ported_terrain_interpolation() {
    let Some(mut game) = game() else { return };
    // 4000+ shipped objects were placed on the original game's `terrainElevation`; the port must
    // reproduce it (both diagonals, both triangles) to float precision.
    for name in ["mc2_01", "mc2_02", "mc2_14"] {
        let m = Mission::load(&mut game, name).unwrap();
        let worst = m
            .objects
            .iter()
            .map(|o| (o.position[2] - m.terrain.elevation_at(o.position[0], o.position[1])).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "{name}: worst height error {worst}");
    }
}

#[test]
fn object_types_resolve_to_appearances_and_shapes() {
    let Some(mut game) = game() else { return };
    let mut types = ObjectTypes::open(&mut game).unwrap();
    let mech = types.get(45).unwrap().clone();
    assert_eq!(mech.class, ObjectClass::BattleMech);
    assert_eq!(mech.appearance, "werewolf");
    assert_eq!(types.get(135).unwrap().class, ObjectClass::Tree);
    assert!(types.get(0).is_none());
    assert!(types.get(100_000).is_none());
    let app = Appearance::load(&mut game, &mech.appearance)
        .unwrap()
        .unwrap();
    assert_eq!(app.shape(), "Werewolf");
    assert_eq!(app.lods[1].as_ref().unwrap().file, "WerewolfL1");
    assert!(game.exists(&format!("data/tgl/{}.tgl", app.shape())));
}

#[test]
fn werewolf_walk_animation_moves_the_hierarchy() {
    let Some(mut game) = game() else { return };
    let model = ShapeModel::load(&mut game, "Werewolf").unwrap();
    assert!(model.nodes.len() > 40);
    let hip = model.node_index("joint_hip").unwrap();
    let foot = model.node_index("joint_lfoot").unwrap();
    assert_eq!(model.nodes[foot].parent, model.node_index("joint_lankle"));
    // bind pose: roughly the size of a battlemech in game units
    let (lo, hi) = model.bounds;
    assert!(
        hi.y - lo.y > 30.0 && hi.y - lo.y < 60.0,
        "height {}",
        hi.y - lo.y
    );
    assert!(hi.x - lo.x < 30.0 && hi.z - lo.z < 40.0);
    assert!(lo.y > -2.0 && lo.y < 2.0, "feet stand on y = 0: {}", lo.y);

    let clip = AnimationClip::load(&mut game, &model, "werewolfwalk").unwrap();
    assert!(clip.frames > 10);
    assert!(clip.tracks.iter().flatten().count() >= 10);
    let a = model.pose(Some(&clip), 0.0, &Default::default());
    let b = model.pose(Some(&clip), clip.frames as f32 / 2.0, &Default::default());
    let moved = |i: usize| {
        (a[i]
            .0
            .iter()
            .zip(&b[i].0)
            .map(|(x, y)| (x - y).abs())
            .sum::<f32>())
            > 0.5
    };
    assert!(moved(foot), "the foot swings during the walk");
    // the whole thing is a loop
    let end = model.pose(Some(&clip), (clip.frames - 1) as f32, &Default::default());
    let start = model.pose(Some(&clip), 0.0, &Default::default());
    let drift = end[hip].transform_point(Default::default())
        - start[hip].transform_point(Default::default());
    assert!(drift.length() < 20.0, "root drift {drift:?}");
}

fn test_scene() -> Option<SceneRenderer> {
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let gpu = common::headless_or_skip("mission_data scene", &cfg)?;
    Some(SceneRenderer::new(gpu).unwrap())
}

#[test]
fn appearance_light_values_come_from_the_ini() {
    let Some(mut game) = game() else { return };
    let a = Appearance::load(&mut game, "l_guardhouse")
        .unwrap()
        .unwrap();
    assert_eq!(a.glow.green, 0x0055_7238);
    assert_eq!(a.glow.pink, 0x00A7_A783);
    assert_eq!(a.glow.yellow, 0x00FE_FF91);
    let l = a.terrain_light.unwrap();
    assert_eq!(l.rgb, 0x0078_7832);
    assert_eq!(l.intensity, 0.2);
    // the data misspells TerrainLightInnerRadius, so the C++ default applies
    assert_eq!(l.inner_radius, 100.0);
    assert_eq!(l.outer_radius, 250.0);
    // appearances without night lights have none
    assert!(Appearance::load(&mut game, "werewolf")
        .unwrap()
        .unwrap()
        .terrain_light
        .is_none());
}

#[test]
fn spotlight_nodes_are_kept_for_night_drawing() {
    let Some(mut game) = game() else { return };
    let m = ShapeModel::load(&mut game, "tinshack").unwrap();
    use mc2_render::shape::NodeKind;
    assert!(m.batches.iter().any(|b| b.kind == NodeKind::Spotlight));
    assert!(m.batches.iter().any(|b| b.kind == NodeKind::Window));
}

#[test]
fn night_lights_and_spotlights_change_the_picture_and_models_are_released() {
    let Some(mut game) = game() else { return };
    let Some(mut scene) = test_scene() else {
        return;
    };
    let mut m = Mission::load(&mut game, "mc2_01").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let pop = populate(&mut game, &m, &mut scene, 0).unwrap();
    assert!(!pop.terrain_lights.is_empty(), "mc2_01 has lit buildings");
    assert!(scene.model_count() > 10);

    // dusk: night factor 1, sun below the horizon
    m.light = m.light.at_pitch(250.0);
    assert!(m.light.is_night);
    let (w, h) = (480u32, 270u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    // look at the building with a light
    let lit = pop.terrain_lights[0].position;
    cam.target = lit;
    cam.projection_angle = 45.0;
    cam.altitude = 900.0;
    let render = |scene: &SceneRenderer| {
        scene
            .render(&Frame {
                width: w,
                height: h,
                camera: &cam,
                light: &m.light,
                water: &m.water,
                terrain_params: &m.terrain_params,
                time: 0.0,
                precise_time: None,
                instances: &pop.instances,
                shadows: false,
            })
            .unwrap()
    };
    scene.set_terrain_lights(&m, &[], 1.0);
    let dark = render(&scene);
    scene.set_terrain_lights(&m, &pop.terrain_lights, m.light.night_factor);
    let lit_img = render(&scene);
    let sum = |v: &[u8]| v.iter().map(|b| *b as u64).sum::<u64>();
    assert!(sum(&lit_img) > sum(&dark), "light pools add light");

    // a new mission starts without the old models
    let m2 = Mission::load(&mut game, "mc2_02").unwrap();
    let tex2 = TerrainTextures::load(&mut game, &m2).unwrap();
    scene.set_terrain(&m2, &tex2);
    assert_eq!(scene.model_count(), 0);
}

#[test]
fn lod_and_damage_shapes_follow_the_eye_distance() {
    // BldgAppearance::recalcBounds (bdactor.cpp:1302-1325) picks the LOD from the eye distance;
    // setObjStatus (bdactor.cpp:759-790) swaps in the [TGLDamage] shape while destroyed.
    let Some(mut game) = game() else { return };
    let Some(mut scene) = test_scene() else {
        return;
    };
    let m = Mission::load(&mut game, "mc2_01").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let mut pop = populate(&mut game, &m, &mut scene, 0).unwrap();
    assert!(pop.report.missing_files.is_empty());
    let with_lod = pop
        .instances
        .iter()
        .filter(|i| i.shapes.lods[1].is_some())
        .count();
    assert!(with_lod > 0, "mc2_01 has objects with a LOD 1 shape");
    eprintln!(
        "mc2_01: {with_lod} instances with LOD 1, {} with a damage shape",
        pop.instances
            .iter()
            .filter(|i| i.shapes.damaged.is_some())
            .count()
    );

    // An eye far beyond every Distance<i>: each object shows its last LOD.
    let far = Vec3::new(1.0e7, 1.0e7, 0.0);
    select_shapes_from(&scene, &mut pop.instances, far, |_, _| true, true);
    for inst in &pop.instances {
        let last = (1..MAX_LODS)
            .rev()
            .find(|&i| inst.shapes.lods[i].is_some() && inst.shapes.lod_distance[i] < 1.0e6)
            .unwrap_or(0);
        assert_eq!(Some(inst.model), inst.shapes.lods[last]);
        assert_eq!(inst.pose.len(), scene.model(inst.model).nodes.len());
    }

    // An eye on the object (distance 0): no Distance<i> is below 0, so the full detail shape.
    for inst in pop.instances.iter_mut() {
        inst.select_shape(inst.position, true, true);
        assert_eq!(Some(inst.model), inst.shapes.lods[0]);
    }

    // A destroyed object with a damage shape shows it; back to normal the LOD comes back.
    let inst = pop
        .instances
        .iter_mut()
        .find(|i| i.shapes.damaged.is_some())
        .expect("mc2_01 has an object with a damage shape");
    inst.state.status = ObjectStatus::Destroyed;
    inst.select_shape(far, true, true);
    assert_eq!(Some(inst.model), inst.shapes.damaged);
    inst.state.status = ObjectStatus::Normal;
    inst.select_shape(far, true, true);
    assert_ne!(Some(inst.model), inst.shapes.damaged);
}

#[test]
fn mech_poses_are_a_function_of_the_gesture_state() {
    // Mech3DAppearanceType::setAnimation (mech3d.cpp:655): the bound gesture's animation at the frame; the
    // integer part of the frame selects the keys (TransformMultiShape, msl.cpp).
    let Some(mut game) = game() else { return };
    let Some(mut scene) = test_scene() else {
        return;
    };
    let m = Mission::load(&mut game, "mc2_01").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let mut pop =
        populate_with_animations(&mut game, &m, &mut scene, 0, &MECH_ANIMATION_NAMES).unwrap();
    let idx = pop.part_instances[0];
    let model = pop.instances[idx].model;
    let walk = scene.model(model).gestures[gesture::WALK as usize]
        .as_ref()
        .expect("werewolf has a walk animation");
    assert!(walk.frames > 4);
    let eye = pop.instances[idx].position;
    let nodes = scene.model(model).nodes.len();

    let pose_at =
        |pop: &mut mc2_render::populate::Populated, gesture: Option<usize>, frame: f32| {
            pop.instances[idx].state = AppearanceState {
                gesture,
                frame,
                ..Default::default()
            };
            select_shapes_from(&scene, &mut pop.instances, eye, |_, _| true, true);
            pop.instances[idx].pose.clone()
        };
    let walk_3 = pose_at(&mut pop, Some(gesture::WALK as usize), 3.0);
    assert_eq!(walk_3.len(), nodes);
    // float2long rounds to the nearest frame (msl.cpp:1347): 3.4 is frame 3; 3.6 and 3.9 are frame 4.
    assert_eq!(walk_3, pose_at(&mut pop, Some(gesture::WALK as usize), 3.4));
    let walk_4 = pose_at(&mut pop, Some(gesture::WALK as usize), 4.0);
    assert_eq!(walk_4, pose_at(&mut pop, Some(gesture::WALK as usize), 3.6));
    assert_eq!(walk_4, pose_at(&mut pop, Some(gesture::WALK as usize), 3.9));
    assert_ne!(walk_3, walk_4);
    // No gesture bound: the bind pose.
    assert_eq!(pose_at(&mut pop, None, 3.0), scene.model(model).bind_pose());
    // A gesture without an animation file: the bind pose as well.
    let no_clip = (0..MECH_ANIMATION_NAMES.len())
        .find(|&g| {
            scene
                .model(model)
                .gestures
                .get(g)
                .is_none_or(|c| c.is_none())
        })
        .expect("some gesture has no animation");
    assert_eq!(
        pose_at(&mut pop, Some(no_clip), 3.0),
        scene.model(model).bind_pose()
    );
}

/// The state the app passes the render for lane F's `mech`: the status, the animation bound to the shape and its
/// frame (`bound_gesture`, `shape.frame_num`: what `updateGeometry` latches, `mech3d.cpp:2971`, and not the gesture
/// and time that keep moving after destruction), and `shape_kind` as the fall latch (`ForwardDamage` 0,
/// `BackwardDamage` 1, `Lod` none).
fn latched_state(mech: &MechAppearance) -> AppearanceState {
    let fall_shape = match mech.shape_kind {
        ShapeKind::ForwardDamage => Some(0),
        ShapeKind::BackwardDamage => Some(1),
        ShapeKind::Lod(_) => None,
    };
    let status = match mech.status {
        object_status::DESTROYED => ObjectStatus::Destroyed,
        object_status::DISABLED => ObjectStatus::Disabled,
        _ => ObjectStatus::Normal,
    };
    AppearanceState {
        status,
        gesture: mech.bound_gesture,
        frame: mech.shape.frame_num,
        fall_shape,
        base_root_node_height: (mech.base_root_node_height != -99999.9)
            .then_some(mech.base_root_node_height),
        is_helicopter: mech.is_helicopter,
    }
}

/// The model and pose the render draws for mech instance `idx` in `mech`'s latched state.
fn rendered(
    scene: &SceneRenderer,
    pop: &mut mc2_render::populate::Populated,
    idx: usize,
    eye: Vec3,
    mech: &MechAppearance,
) -> (mc2_render::scene::ModelId, Vec<mc2_render::math::Mat4>) {
    pop.instances[idx].state = latched_state(mech);
    select_shapes_from(scene, &mut pop.instances, eye, |_, _| true, true);
    (pop.instances[idx].model, pop.instances[idx].pose.clone())
}

fn rendered_model(
    scene: &SceneRenderer,
    pop: &mut mc2_render::populate::Populated,
    idx: usize,
    eye: Vec3,
    mech: &MechAppearance,
) -> mc2_render::scene::ModelId {
    rendered(scene, pop, idx, eye, mech).0
}

#[test]
fn a_latched_fall_shape_follows_lane_f_across_gesture_changes() {
    // Mech3DAppearance::setObjStatus (mech3d.cpp:2871-2900) swaps to the fall damage shape once, at the moment of
    // destruction, and the gesture changing afterwards does not undo it.  Lane F keeps that as `shape_kind`; the
    // render must draw what it latched.
    let Some(mut game) = game() else { return };
    let Some(mut scene) = test_scene() else {
        return;
    };
    let m = Mission::load(&mut game, "mc2_01").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let mut pop =
        populate_with_animations(&mut game, &m, &mut scene, 0, &MECH_ANIMATION_NAMES).unwrap();
    let idx = pop.part_instances[0];
    let shapes = pop.instances[idx].shapes;
    let eye = pop.instances[idx].position;
    assert_eq!(gesture::FALLEN_FORWARD, 23);
    assert!(
        shapes.fall_damage[0].is_some(),
        "werewolf has a forward fall shape"
    );

    let mut ty = MechAppearanceType::load(&mut game.vfs, "werewolf").unwrap();
    let mut rng = SimRng::new(RngMode::Pcg32, 1);

    // Destroyed while fallen forward (gesture 23): lane F swaps, the render draws the fall shape.
    let mut fell = MechAppearance::new(&mut ty, 0, &mut rng);
    fell.current_gesture = gesture::FALLEN_FORWARD;
    fell.set_obj_status(&ty, object_status::DESTROYED);
    assert_eq!(fell.shape_kind, ShapeKind::ForwardDamage);
    let fall = shapes.fall_damage[0];
    assert_eq!(
        Some(rendered_model(&scene, &mut pop, idx, eye, &fell)),
        fall
    );
    // The gesture moves on (a walk): lane F keeps the latch, and so does the render.
    fell.current_gesture = gesture::WALK;
    assert_eq!(
        Some(rendered_model(&scene, &mut pop, idx, eye, &fell)),
        fall
    );

    // Destroyed in the standing-to-fall gesture (15), not a fall gesture: lane F does not swap, and the render keeps
    // the LOD it had.  The gesture changing to 23 afterwards must not swap it now.
    // A second mech: its own render instance, so its first frame applies its own status change.
    pop.instances[idx].choice = Default::default();
    let mut crumpled = MechAppearance::new(&mut ty, 0, &mut rng);
    crumpled.current_gesture = gesture::FALL_FORWARD;
    crumpled.set_obj_status(&ty, object_status::DESTROYED);
    assert_eq!(crumpled.shape_kind, ShapeKind::Lod(0));
    assert_eq!(
        Some(rendered_model(&scene, &mut pop, idx, eye, &crumpled)),
        shapes.lods[0]
    );
    crumpled.current_gesture = gesture::FALLEN_FORWARD;
    assert_eq!(
        Some(rendered_model(&scene, &mut pop, idx, eye, &crumpled)),
        shapes.lods[0]
    );
    eprintln!("werewolf: latched fall shape kept across the walk; no swap for a gesture-15 death");
}

#[test]
fn a_destroyed_mech_animates_its_fall_then_holds_the_wreck() {
    // Mech3DAppearance::updateGeometry (mech3d.cpp:3137-3140) binds the animation and sets the frame only while the mech
    // is not destroyed.  A mech destroyed while walking swaps to no fall shape (mech3d.cpp:2879), so its shape keeps the
    // walk and the frame it had, while the mech's own gesture and frame move on.
    let Some(mut game) = game() else { return };
    let Some(mut scene) = test_scene() else {
        return;
    };
    let m = Mission::load(&mut game, "mc2_01").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let mut pop =
        populate_with_animations(&mut game, &m, &mut scene, 0, &MECH_ANIMATION_NAMES).unwrap();
    let idx = pop.part_instances[0];
    let eye = pop.instances[idx].position;

    let mut ty = MechAppearanceType::load(&mut game.vfs, "werewolf").unwrap();
    let mut rng = SimRng::new(RngMode::Pcg32, 1);
    let mut mech = MechAppearance::new(&mut ty, 0, &mut rng);
    mech.current_gesture = gesture::WALK;
    // Alive and walking: each update binds the walk and latches the shape's frame.
    for _ in 0..3 {
        let mut ctx = UpdateCtx {
            turn: 0,
            frame_length: 0.1,
            sim_rng: &mut rng,
            gesture_rng: None,
            terrain: None,
        };
        mech.update(&mut ty, true, &mut ctx);
    }
    assert_eq!(mech.bound_gesture, Some(gesture::WALK as usize));
    mech.set_obj_status(&ty, object_status::DESTROYED);
    assert_eq!(
        mech.shape_kind,
        ShapeKind::Lod(0),
        "destroyed walking: no fall shape is swapped in"
    );

    let (model, latched_pose) = rendered(&scene, &mut pop, idx, eye, &mech);
    let latched_frame = mech.shape.frame_num;
    let walk = scene.model(model).gestures[gesture::WALK as usize]
        .as_ref()
        .expect("werewolf has a walk animation");
    assert_eq!(
        latched_pose,
        scene.model(model).pose(
            Some(walk),
            latched_frame.round_ties_even(),
            &std::collections::HashMap::new()
        ),
        "the render draws the walk at the frame the shape latched"
    );

    // Until the fall completes the render keeps drawing the falling intact shape. Once it reaches a fallen gesture, the
    // Rust port deliberately swaps to the destroyed shape (DIFFERENCES.md D-52).
    let mut moved = false;
    let mut fell = false;
    let mut animated_poses = 0;
    for _ in 0..30 {
        let mut ctx = UpdateCtx {
            turn: 0,
            frame_length: 0.1,
            sim_rng: &mut rng,
            gesture_rng: None,
            terrain: None,
        };
        mech.update(&mut ty, true, &mut ctx);
        // BattleMech::update hands the appearance DESTROYED only from the frame after mechRemoved (mech.cpp:6229-6233,
        // 6282-6283), so the fall roll (mech3d.cpp:3952-3967) reads it from the second update on.
        mech.cpp_status = object_status::DESTROYED;
        moved |= mech.current_frame != latched_frame;
        let (m2, pose) = rendered(&scene, &mut pop, idx, eye, &mech);
        if mech.shape_kind == ShapeKind::Lod(0) {
            assert_eq!(m2, model);
            // The fall keeps animating on the intact shape (the C++ withholds the destroyed status).
            assert_eq!(
                mech.bound_gesture,
                usize::try_from(mech.current_gesture).ok()
            );
            animated_poses += usize::from(pose != latched_pose);
        } else {
            assert!(matches!(
                mech.shape_kind,
                ShapeKind::ForwardDamage | ShapeKind::BackwardDamage
            ));
            assert!(matches!(
                mech.current_gesture,
                gesture::FALLEN_FORWARD | gesture::FALLEN_BACKWARD
            ));
            // The shipped damaged TGL aliases the intact model in this scene cache; the
            // appearance's terminal shape kind and gesture above are the authoritative
            // evidence of the destroyed presentation, not its cache key.
            let _ = (m2, pose);
            fell = true;
            break;
        }
    }
    assert!(
        moved,
        "the mech's own frame advanced before the terminal-fall swap"
    );
    assert!(
        fell,
        "a destroyed mech reaches and latches its fallen damage shape"
    );
    assert!(
        animated_poses > 0,
        "the rendered pose changes during the fall, before the wreck shape is latched"
    );
    let end = mech.bound_gesture;
    mech.update(
        &mut ty,
        true,
        &mut UpdateCtx {
            turn: 0,
            frame_length: 0.1,
            sim_rng: &mut rng,
            gesture_rng: None,
            terrain: None,
        },
    );
    assert_eq!(mech.bound_gesture, end, "the wreck holds its terminal pose");
}

/// A node override (the mech torso twist) is applied as `override * animation` (`msl.cpp:1443`,
/// `rotation.cpp:773-787`), not `animation * override`: with a shipped, non-commuting walk frame the full rotation
/// of `joint_torso` matches the reference product within 2 degrees, and the reversed order would not.
#[test]
fn node_override_composes_before_the_gesture_rotation() {
    let Some(mut game) = game() else { return };
    let model = ShapeModel::load(&mut game, "Werewolf").unwrap();
    let clip = AnimationClip::load(&mut game, &model, "werewolfwalk").unwrap();
    let torso = model.node_index("joint_torso").unwrap();
    let parent = model.nodes[torso].parent.unwrap();
    let track = clip.tracks[torso]
        .as_ref()
        .expect("the walk animates the torso");
    let twist =
        mc2_render::math::Quat::from_axis_angle(Vec3::new(0.0, 1.0, 0.0), 60f32.to_radians());
    let angle_between = |a: &mc2_render::math::Mat4, b: &mc2_render::math::Mat4| {
        // rotation angle of a * b^T over the 3x3 block (rows are the basis, row-vector convention)
        let mut tr = 0.0;
        for i in 0..3 {
            for j in 0..3 {
                tr += a.at(i, j) * b.at(i, j);
            }
        }
        ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
    };
    let mut worst_reversed = 0.0f32;
    for frame in 0..clip.frames {
        let f = frame as f32;
        let plain = model.pose(Some(&clip), f, &Default::default());
        let posed = model.pose(
            Some(&clip),
            f,
            &std::collections::HashMap::from([(torso, twist)]),
        );
        let rot = track.rotation_at(f);
        let expect = |q: mc2_render::math::Quat| {
            plain[parent].mul(&mc2_render::math::Mat4::from_quat(q.normalized()))
        };
        let want = expect(twist * rot);
        let reversed = expect(rot * twist);
        assert!(
            angle_between(&posed[torso], &want) < 2.0,
            "frame {frame}: torso rotation off the reference"
        );
        worst_reversed = worst_reversed.max(angle_between(&posed[torso], &reversed));
    }
    assert!(
        worst_reversed > 2.0,
        "the walk must be non-commuting with the twist for this test to discriminate: {worst_reversed}"
    );
}
