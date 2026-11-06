// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Local Shadows (`useLocalShadows`, `mclib/tgl.cpp:3049`): at night a building light casts the shadows of the shapes
//! near it; with the option off nothing but the sun does, and the sun is off at night.  Set `MC2_DATA` to the data root;
//! without it the test returns early. A missing GPU adapter fails the test unless `MC2_SKIP_GPU_TESTS=1`.
#![allow(clippy::chunks_exact_to_as_chunks)]

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::appearance::{AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::math::{Mat4, Vec3};
use mc2_render::mission::Mission;
use mc2_render::scene::{Frame, SceneRenderer, ShapeInstance, ShapeStyle};
use mc2_render::shape::{Batch, NodeInfo, NodeKind, ShapeModel, ShapeVertex};
use mc2_render::terrain_lights::TerrainLight;
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
fn local_shadows_change_the_night_frame_only_when_on() {
    let Ok(mut game) = GameData::from_env() else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("local shadows frame", &cfg) else {
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
    m.light.is_night = true;
    m.light.night_factor = 1.0;
    let (w, h) = (640u32, 360u32);
    let mut cam = GameCamera::from_fit(CameraSettings::default(), &mut m.fit, (w as f32, h as f32));
    cam.target.z = m.terrain.elevation_at(cam.target.x, cam.target.y);
    let at = cam.target;
    // a bright building light 150 units to one side of the building, both inside its radius
    let light = TerrainLight {
        position: Vec3::new(at.x + 150.0, at.y, at.z + 120.0),
        rgb: 0x00ff_ffff,
        intensity: 1.0,
        inner_radius: 100.0,
        outer_radius: 400.0,
    };
    scene.set_terrain_lights(&m, &[light], 1.0);
    let instance = ShapeInstance {
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
        position: Vec3::new(at.x, at.y, at.z + 30.0),
        yaw_degrees: 0.0,
        infinite_light: [0.3; 3],
        ambient_override: Some([0.1; 3]),
        casts_shadow: true,
        in_view: true,
        root_height_difference: 0.0,
        highlight: 0,
    };
    let mut render = |local: bool, sun: bool| {
        scene.set_local_shadows(local);
        let instances = [instance.clone()];
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
                shadows: sun,
            })
            .unwrap()
    };
    let on = render(true, false);
    let off = render(false, false);
    let changed = on.iter().zip(&off).filter(|(a, b)| a != b).count();
    gate_println!("local shadows: {changed} bytes differ between on and off");
    assert!(changed > 100, "Local Shadows changed only {changed} bytes");
    // deterministic, and the option never touches a sun-shadowed frame
    assert_eq!(on, render(true, false));
    assert_eq!(render(true, true), render(false, true));
}
