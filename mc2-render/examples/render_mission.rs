//! Headless proof of the render front end: loads missions by name through the VFS, builds the
//! terrain, the placed objects and the mission's mechs, sets the camera from the fit and writes
//! PNGs.  Never opens a window.
//!
//! usage: MC2_DATA=<data root> cargo run --release -p mc2-render --example render_mission --
//!        [--out DIR] [--sizes 1920x1080,3840x2160] [--no-shadows] [--list-models] [--time SECONDS] [MISSION...]

use mc2_gfx::config::RenderConfig;
use mc2_render::appearance::AppearanceState;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::game::GameData;
use mc2_render::gpu::{write_png, Gpu};
use mc2_render::mission::Mission;
use mc2_render::populate::{populate_with_animations, select_shapes};
use mc2_render::scene::{Frame, SceneRenderer};
use mc2_render::terrain_tex::TerrainTextures;
use mc2_sim::appearance::tables::{gesture, MECH_ANIMATION_NAMES};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> mc2_render::Result<()> {
    let mut out = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/out"));
    let mut sizes = vec![(1920u32, 1080u32), (3840, 2160)];
    let mut shadows = true;
    let mut time = 0.0f32;
    let mut overview = false;
    let mut showcase = true;
    let mut cam_override: Option<(f32, f32, f32)> = None; // pitch, yaw, altitude
    let mut tag = String::new();
    let mut list_models = false;
    let mut sun_pitch: Option<f32> = None;
    let mut target_override: Option<(f32, f32)> = None;
    let mut missions: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(args.next().expect("--out DIR")),
            "--sizes" => {
                sizes = args
                    .next()
                    .expect("--sizes WxH,WxH")
                    .split(',')
                    .filter_map(|s| {
                        let (w, h) = s.split_once('x')?;
                        Some((w.parse().ok()?, h.parse().ok()?))
                    })
                    .collect()
            }
            "--no-shadows" => shadows = false,
            "--no-showcase" => showcase = false,
            // Top-down, north-up, whole map, no haze: for checking the data against the picture.
            "--overview" => {
                overview = true;
                tag = "_overview".into();
            }
            "--target" => {
                let v: Vec<f32> = args
                    .next()
                    .expect("--target X,Y")
                    .split(',')
                    .map(|s| s.parse().expect("number"))
                    .collect();
                target_override = Some((v[0], v[1]));
            }
            "--sun-pitch" => {
                sun_pitch = Some(
                    args.next()
                        .expect("--sun-pitch DEG")
                        .parse()
                        .expect("number"),
                )
            }
            // Print the shape names the mission places (one `model <name>` line each).
            "--list-models" => list_models = true,
            "--tag" => tag = format!("_{}", args.next().expect("--tag NAME")),
            "--camera" => {
                let v: Vec<f32> = args
                    .next()
                    .expect("--camera PITCH,YAW,ALTITUDE")
                    .split(',')
                    .map(|s| s.parse().expect("number"))
                    .collect();
                cam_override = Some((v[0], v[1], v[2]));
            }
            "--time" => time = args.next().expect("--time S").parse().expect("number"),
            other => missions.push(other.to_string()),
        }
    }
    if missions.is_empty() {
        missions = vec!["mc2_01".into(), "mc2_02".into(), "mc2_10".into()];
    }
    std::fs::create_dir_all(&out)?;

    let cfg = RenderConfig::from_env();
    let gpu = Gpu::headless(&cfg)?;
    println!(
        "adapter {} ({}) msaa x{} aniso {}",
        gpu.adapter_name, gpu.backend, gpu.samples, cfg.aniso
    );
    let mut scene = SceneRenderer::new(gpu)?;
    let mut game = GameData::from_env()?;

    for name in &missions {
        let t0 = Instant::now();
        let mut mission = Mission::load(&mut game, name)?;
        if let Some(p) = sun_pitch {
            // Day/night cycle of `Camera::updateDaylight` evaluated at this sun elevation.
            mission.light = mission.light.at_pitch(p);
        }
        let textures = TerrainTextures::load(&mut game, &mission)?;
        println!(
            "[{name}] terrain {}x{} elevation {:?}, {} tiles with road/pavement textures ({} layers, {} missing), {} objects, {} parts",
            mission.terrain.side(),
            mission.terrain.side(),
            mission.terrain.elevation_range(),
            textures.tile_words.iter().filter(|w| (**w & 0xffff) != 0xffff).count(),
            textures.tile_layers.len(),
            textures.missing_tiles.len(),
            mission.objects.len(),
            mission.parts.len()
        );
        scene.set_terrain(&mission, &textures);
        let mut populated =
            populate_with_animations(&mut game, &mission, &mut scene, 0, &MECH_ANIMATION_NAMES)?;
        println!("[{name}] {:?}", populated.report);
        populated.report.ensure_painted()?;
        scene.set_terrain_lights(
            &mission,
            &populated.terrain_lights,
            mission.light.night_factor,
        );

        if list_models {
            let mut names: Vec<String> = populated
                .instances
                .iter()
                .map(|i| scene.model(i.model).name.to_ascii_lowercase())
                .collect();
            names.sort();
            names.dedup();
            for n in names {
                println!("model {n}");
            }
            // The texture list of every placed model, as the scene asks the replacement hook for
            // it; with MC2_HIRES_DIR set, say whether the pack answers each key.
            let mut paths: Vec<String> = populated
                .instances
                .iter()
                .flat_map(|i| scene.model(i.model).texture_names.iter())
                .map(|n| mc2_render::shape::texture_path(n))
                .collect();
            paths.sort();
            paths.dedup();
            let hires = std::env::var_os("MC2_HIRES_DIR").map(PathBuf::from);
            for p in paths {
                let state = match (&hires, game.tga(&p)) {
                    (Some(dir), Ok(img)) => {
                        let key = mc2_gfx::texture::texture_key(
                            Some(&p),
                            &img.pixels,
                            img.width as u32,
                            img.height as u32,
                            0,
                        );
                        if mc2_gfx::texture::load_replacement(dir, &key).is_some() {
                            " replaced"
                        } else {
                            " MISSING"
                        }
                    }
                    (Some(_), Err(_)) => " (no source TGA)",
                    _ => "",
                };
                println!("texture {p}{state}");
            }
        }

        // Camera from the fit; the target rides the terrain (Camera::setPosition).
        let settings = {
            let mut cfg = game.fit("system.cfg")?;
            CameraSettings::from_system_cfg(&mut cfg)
        };
        let mut cam = GameCamera::from_fit(settings, &mut mission.fit, (1920.0, 1080.0));
        if overview {
            cam.projection_angle = 88.0;
            cam.set_rotation(180.0);
            cam.zoom_value(20000.0);
            cam.settings.min_haze_distance = 1.0e9;
            cam.settings.max_clip_distance = 2.0e9;
            cam.target = mc2_render::math::Vec3::new(0.0, 0.0, 400.0);
        } else if let Some((pitch, yaw, alt)) = cam_override {
            cam.projection_angle = pitch;
            cam.set_rotation(yaw);
            cam.zoom_value(alt);
        }
        let (mut tx, mut ty) = (cam.target.x, cam.target.y);
        if let Some((x, y)) = target_override {
            tx = x;
            ty = y;
        }
        let side = mission.terrain.world_side();
        if !overview {
            cam.set_position(
                tx,
                ty,
                side,
                |x, y| mission.terrain.elevation_at(x, y),
                mission.water.elevation,
            );
        }
        // The simulation's part in this example: the first mission part walks, the others stand.  The
        // frame is the time's (`AnimationClip::frame_at`), so the pose is a function of the state and time.
        for (k, &idx) in populated.part_instances.iter().enumerate() {
            let model = populated.instances[idx].model;
            let (gesture, frame) = if k == 0 {
                let walk = scene
                    .model(model)
                    .gestures
                    .get(gesture::WALK as usize)
                    .and_then(Option::as_ref);
                (gesture::WALK, walk.map_or(0.0, |c| c.frame_at(time + 0.3)))
            } else {
                (gesture::STAND, 0.0)
            };
            populated.instances[idx].state = AppearanceState {
                gesture: Some(gesture as usize),
                frame,
                ..Default::default()
            };
        }
        for &(w, h) in &sizes {
            cam.viewport = (w as f32, h as f32);
            // `recalcBounds` runs once for this final camera/viewport before the frame draws.
            // In particular, the projected selection box (and therefore LOD) may change with
            // the aspect ratio even when the eye stays put.
            select_shapes(&scene, &mut populated.instances, &cam, true);
            let t1 = Instant::now();
            let frame = Frame {
                width: w,
                height: h,
                camera: &cam,
                light: &mission.light,
                water: &mission.water,
                terrain_params: &mission.terrain_params,
                time,
                precise_time: None,
                instances: &populated.instances,
                // `MultiTransformShadows`: the sun casts no shadows at night
                shadows: shadows && !mission.light.is_night,
            };
            let rgba = scene.render(&frame)?;
            let path = out.join(format!("{name}{tag}_{w}x{h}.png"));
            write_png(&path, &rgba, w, h)?;
            println!(
                "[{name}] wrote {} ({} ms render+readback)",
                path.display(),
                t1.elapsed().as_millis()
            );
        }
        // Second view: a 38 degree look at the densest cluster of buildings, to show what the
        // start camera (the designers' low angle) does not.
        if showcase {
            // only buildings count: weight by the number of non-tree instances
            let building_cells: std::collections::HashMap<(i32, i32), usize> = populated
                .instances
                .iter()
                .take(mission.objects.len())
                .filter(|i| scene.model(i.model).bounds.1.y > 12.0 && i.ambient_override.is_none())
                .fold(Default::default(), |mut m, i| {
                    *m.entry((
                        (i.position.x / 1200.0).floor() as i32,
                        (i.position.y / 1200.0).floor() as i32,
                    ))
                    .or_default() += 1;
                    m
                });
            let best = building_cells
                .iter()
                .max_by_key(|(k, v)| (**v, **k))
                .map(|(k, _)| *k);
            if let Some((cx, cy)) = best {
                let (x, y) = ((cx as f32 + 0.5) * 1200.0, (cy as f32 + 0.5) * 1200.0);
                cam.set_position(
                    x,
                    y,
                    side,
                    |x, y| mission.terrain.elevation_at(x, y),
                    mission.water.elevation,
                );
                cam.projection_angle = 38.0;
                cam.zoom_value(1700.0);
                for &(w, h) in &sizes {
                    cam.viewport = (w as f32, h as f32);
                    // The showcase moves both the eye and viewport, so it needs its own
                    // per-frame `recalcBounds`/LOD pass too.
                    select_shapes(&scene, &mut populated.instances, &cam, true);
                    let frame = Frame {
                        width: w,
                        height: h,
                        camera: &cam,
                        light: &mission.light,
                        water: &mission.water,
                        terrain_params: &mission.terrain_params,
                        time,
                        precise_time: None,
                        instances: &populated.instances,
                        shadows: shadows && !mission.light.is_night,
                    };
                    let rgba = scene.render(&frame)?;
                    let path = out.join(format!("{name}_showcase_{w}x{h}.png"));
                    write_png(&path, &rgba, w, h)?;
                    println!("[{name}] wrote {}", path.display());
                }
            }
        }
        println!("[{name}] total {} ms", t0.elapsed().as_millis());
    }
    Ok(())
}
