// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! At night a terrain light pool lights shapes too, not only the ground (the `TG_LIGHT_TERRAIN` branch of `MultiTransformShape`,
//! `tgl.cpp:2023-2051`). Set `MC2_DATA` to the data root; without it the test returns early. A missing GPU adapter fails the test
//! unless `MC2_SKIP_GPU_TESTS=1`. The frames are written to `$CARGO_TARGET_TMPDIR/visual/night-light-shape-*.png`.
#![allow(clippy::chunks_exact_to_as_chunks)]

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::appearance::{AppearanceKind, AppearanceState, ShapeChoice, ShapeSet};
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::gpu::write_png;
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
fn a_shape_inside_a_night_light_pool_is_brighter_than_one_outside() {
    let Ok(mut game) = GameData::from_env() else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    let Some(gpu) = common::headless_or_skip("night light on shapes", &cfg) else {
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
    let at = Vec3::new(cam.target.x, cam.target.y, cam.target.z + 30.0);
    let instances = [ShapeInstance {
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
        position: at,
        yaw_degrees: 0.0,
        infinite_light: [0.3; 3],
        ambient_override: Some([0.1; 3]),
        casts_shadow: false,
        in_view: true,
        root_height_difference: 0.0,
        highlight: 0,
    }];
    let render = |scene: &mut SceneRenderer| {
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
                // the production night frame: the sun casts no shadows at night (game.rs `shadows && !is_night`)
                shadows: false,
            })
            .unwrap()
    };
    let light = |position: Vec3| TerrainLight {
        position,
        rgb: 0x00ff_c864,
        intensity: 1.0,
        inner_radius: 120.0,
        outer_radius: 260.0,
    };
    let ground = Vec3::new(at.x, at.y, cam.target.z);

    let dark = render(&mut scene);
    // the pool is centred on the shape
    scene.set_terrain_lights(&m, &[light(ground)], 1.0);
    let inside = render(&mut scene);
    // the same pool a long way from the shape
    scene.set_terrain_lights(&m, &[light(Vec3::new(at.x + 900.0, at.y, ground.z))], 1.0);
    let outside = render(&mut scene);

    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, img) in [("dark", &dark), ("inside", &inside), ("outside", &outside)] {
        write_png(
            &dir.join(format!("night-light-shape-{name}.png")),
            img,
            w,
            h,
        )
        .unwrap();
    }

    // the cube's pixels: the box around the screen centre where it stands
    let sum = |img: &[u8]| -> u64 {
        let mut s = 0u64;
        for y in (h / 2 - 20)..(h / 2 + 20) {
            for x in (w / 2 - 20)..(w / 2 + 20) {
                let i = ((y * w + x) * 4) as usize;
                s += u64::from(img[i]) + u64::from(img[i + 1]) + u64::from(img[i + 2]);
            }
        }
        s
    };
    let (d, i, o) = (sum(&dark), sum(&inside), sum(&outside));
    gate_println!("night light on shapes: dark {d}, inside the pool {i}, outside {o}");
    assert!(
        i > d + d / 10,
        "the shape in the pool is not brighter: {i} vs {d}"
    );
    assert_eq!(o, d, "a pool that does not reach the shape changed it");
}
