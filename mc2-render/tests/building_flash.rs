// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! A flashing building is drawn with the flash colour added to its lit vertex colours (`SetARGBHighLight`, `tgl.cpp:2207-2227`).
//! Set `MC2_DATA` to the data root; without it the test returns early. A missing GPU adapter fails the test unless
//! `MC2_SKIP_GPU_TESTS=1`.
#![allow(clippy::chunks_exact_to_as_chunks)]

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::appearance::{AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::flash::BuildingFlash;
use mc2_render::game::GameData;
use mc2_render::math::{Mat4, Vec3};
use mc2_render::mission::Mission;
use mc2_render::scene::{Frame, SceneRenderer, ShapeInstance, ShapeStyle};
use mc2_render::shape::{Batch, NodeInfo, NodeKind, ShapeModel, ShapeVertex};
use mc2_render::terrain_tex::TerrainTextures;

/// A box of `h` half-size drawn from both sides of every face, so the camera sees it whatever the axis convention.
fn cube(h: f32, kind: NodeKind) -> ShapeModel {
    let mut vertices = Vec::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let p = |a: f32, b: f32| {
                let mut v = [0.0; 3];
                v[axis] = sign * h;
                v[(axis + 1) % 3] = a * h;
                v[(axis + 2) % 3] = b * h;
                v
            };
            let mut n = [0.0; 3];
            n[axis] = sign;
            let quad = [p(-1.0, -1.0), p(1.0, -1.0), p(1.0, 1.0), p(-1.0, 1.0)];
            for tri in [[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]] {
                for i in tri {
                    vertices.push(ShapeVertex {
                        position: quad[i],
                        node: 0,
                        normal: n,
                        argb: 0,
                        uv: [0.5, 0.5],
                    });
                }
            }
        }
    }
    let vertex_count = vertices.len() as u32;
    let bounds = (Vec3::new(-h, -h, -h), Vec3::new(h, h, h));
    ShapeModel {
        name: "flash-cube".into(),
        nodes: vec![NodeInfo {
            id: "root".into(),
            parent: None,
            center: Vec3::ZERO,
            relative_center: Vec3::ZERO,
            kind,
        }],
        // missing on purpose: the renderer falls back to its grey texture
        texture_names: vec!["no_such_texture".into()],
        texture_alpha: vec![false],
        vertices,
        batches: vec![Batch {
            texture: 0,
            alpha: false,
            kind,
            first_vertex: 0,
            vertex_count,
        }],
        bounds,
        cone_extent: h * 2.0,
        building_bounds: bounds,
        gestures: Vec::new(),
    }
}

#[test]
fn a_flashing_building_is_drawn_with_the_flash_colour_added() {
    let Ok(mut game) = GameData::from_env() else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("building flash frame", &cfg) else {
        return;
    };
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mut m = Mission::load(&mut game, "mc2_10").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let model = scene.add_model(
        &mut game,
        cube(30.0, NodeKind::Normal),
        ShapeStyle::default(),
    );

    let (w, h) = (640u32, 360u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    let building = |highlight: u32| ShapeInstance {
        model,
        shapes: ShapeSet {
            kind: AppearanceKind::Building,
            lods: [Some(model), None, None],
            ..Default::default()
        },
        state: AppearanceState::default(),
        choice: ShapeChoice::default(),
        pose: vec![Mat4::IDENTITY],
        pose_key: None,
        turret: None,
        position: Vec3::new(cam.target.x, cam.target.y, cam.target.z + 30.0),
        yaw_degrees: 0.0,
        infinite_light: [0.3; 3],
        ambient_override: Some([0.1; 3]),
        casts_shadow: false,
        in_view: true,
        root_height_difference: 0.0,
        highlight,
    };
    let render = |inst: ShapeInstance| {
        let instances = [inst];
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
                instances: &instances,
                shadows: false,
            })
            .unwrap()
    };

    // the tint the flash state machine gives right after `flashBuilding` (red, the enemy colour)
    let mut flash = BuildingFlash::default();
    flash.start_structure(0x007f_0000);
    let tint = flash.tint().expect("a new flash is lit");
    let plain = render(building(0));
    let empty = scene
        .render(&Frame {
            width: w,
            height: h,
            camera: &cam,
            light: &m.light,
            water: &m.water,
            terrain_params: &m.terrain_params,
            time: 0.0,
            precise_time: None,
            instances: &[],
            shadows: false,
        })
        .unwrap();
    let drawn = plain
        .chunks_exact(4)
        .zip(empty.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        drawn > 500,
        "the test building is not on screen ({drawn} pixels)"
    );
    let flashed = render(building(tint));
    // the unlit half-period draws the plain building
    for _ in 0..16 {
        flash.advance(1.0 / 30.0);
    }
    assert_eq!(flash.tint(), None, "off after 0.5 s");
    assert_eq!(render(building(flash.tint().unwrap_or(0))), plain);

    let (mut redder, mut other) = (0usize, 0usize);
    for (a, b) in plain.chunks_exact(4).zip(flashed.chunks_exact(4)) {
        if a[0] != b[0] && b[0] > a[0] {
            redder += 1;
        }
        if a[1] != b[1] || a[2] != b[2] {
            other += 1;
        }
    }
    gate_println!("building flash: {redder} pixels redder, {other} with green/blue changed");
    assert!(redder > 200, "only {redder} pixels got the red tint");
    assert_eq!(other, 0, "the red tint touched green or blue");
}

/// The spotlight cone takes the flash tint too (`SetARGBHighLight` reaches every shape node, `msl.h:467`, `tgl.cpp:2207`): at
/// night a flashed `SpotLight_` node draws brighter red than the unlit one.
#[test]
fn a_flashing_spotlight_cone_is_tinted_at_night() {
    let Ok(mut game) = GameData::from_env() else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("spotlight flash frame", &cfg) else {
        return;
    };
    let mut scene = SceneRenderer::new(gpu).unwrap();
    let mut m = Mission::load(&mut game, "mc2_10").unwrap();
    let tex = TerrainTextures::load(&mut game, &m).unwrap();
    scene.set_terrain(&m, &tex);
    let model = scene.add_model(
        &mut game,
        cube(30.0, NodeKind::Spotlight),
        ShapeStyle::default(),
    );
    m.light.is_night = true;

    let (w, h) = (640u32, 360u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    let render = |highlight: u32, with_cone: bool| {
        let inst = ShapeInstance {
            model,
            shapes: ShapeSet {
                kind: AppearanceKind::Building,
                lods: [Some(model), None, None],
                ..Default::default()
            },
            state: AppearanceState::default(),
            choice: ShapeChoice::default(),
            pose: vec![Mat4::IDENTITY],
            pose_key: None,
            turret: None,
            position: Vec3::new(cam.target.x, cam.target.y, cam.target.z + 30.0),
            yaw_degrees: 0.0,
            infinite_light: [0.3; 3],
            ambient_override: Some([0.1; 3]),
            casts_shadow: false,
            in_view: true,
            root_height_difference: 0.0,
            highlight,
        };
        let instances = [inst];
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
                instances: if with_cone { &instances } else { &[] },
                shadows: false,
            })
            .unwrap()
    };
    let mut flash = BuildingFlash::default();
    flash.start_structure(0x007f_0000);
    let tint = flash.tint().expect("a new flash is lit");
    let empty = render(0, false);
    let plain = render(0, true);
    let flashed = render(tint, true);
    let redder = plain
        .chunks_exact(4)
        .zip(flashed.chunks_exact(4))
        .filter(|(a, b)| b[0] > a[0])
        .count();
    let cone_vs_empty = plain
        .chunks_exact(4)
        .zip(empty.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    gate_println!(
        "spotlight flash: {redder} pixels redder (cone differs from empty in {cone_vs_empty})"
    );
    assert!(
        redder > 200,
        "only {redder} spotlight pixels got the red tint"
    );
}
