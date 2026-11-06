//! Headless contact sheets of weapon effects, rendered through `mc2-render`'s offscreen `Gpu`
//! over a flat ground. Never opens a window.
//!
//! usage: MC2_DATA=<data root> cargo run --release -p mc2-fx --example fx_sheets -- [--out DIR]
//!        [--frame WxH] [--cols N]
//!
//! One PNG per scenario: a row of frames at fixed times. Scenarios follow `WeaponBolt`: a muzzle
//! flash on the shooter, the in-flight effect travelling to the target, then the hit effect.

use mc2_formats::vfs::Vfs;
use mc2_fx::math::Vec3;
use mc2_fx::render::{self, FxTextures};
use mc2_fx::system::{FxHandle, FxSystem, FxTransform, SpawnParams, WeaponFx};
use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::fx::{FxBatch, FxBlend, FxRenderer, FxVertex};
use mc2_render::gpu::{write_png, Gpu};
use std::path::PathBuf;

const SHOOTER: Vec3 = Vec3::new(150.0, 0.0, 30.0);
const TARGET: Vec3 = Vec3::new(-150.0, 0.0, 30.0);
/// Where the shot leaves the shooter's front face and strikes the target's (boxes are 40 wide).
const MUZZLE_AT: Vec3 = Vec3::new(SHOOTER.x - 26.0, SHOOTER.y, SHOOTER.z);
const HIT_AT: Vec3 = Vec3::new(TARGET.x + 26.0, TARGET.y, TARGET.z);
const DT: f64 = 1.0 / 60.0;

/// A weapon from `effects.csv`.
struct Scenario {
    name: &'static str,
    id: i64,
    /// Frames of the sheet, in seconds after the shot.
    times: &'static [f64],
    kind: Kind,
    /// Camera target in game world coordinates (the ground point and height it looks at).
    focus: [f32; 3],
}

enum Kind {
    /// Muzzle flash, travelling trail, hit (`speed` in world units per second).
    Bolt { speed: f32 },
    /// Beam: the in-flight effect plays at the target (as `WeaponBolt` does for beams).
    Beam,
    /// A single effect (the weapon table's "effect name") at the target.
    Explosion,
    /// Only the hit effect at the target.
    Hit,
}

const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "ppc",
        focus: [0.0, 0.0, 40.0],
        id: 4,
        times: &[0.05, 0.15, 0.3, 0.45, 0.6, 0.75, 0.9, 1.2],
        kind: Kind::Bolt { speed: 600.0 },
    },
    Scenario {
        name: "large_laser",
        focus: [0.0, 0.0, 40.0],
        id: 15,
        times: &[0.05, 0.15, 0.3, 0.45, 0.6, 0.8, 1.0, 1.3],
        kind: Kind::Beam,
    },
    Scenario {
        name: "lrm_missile_trail",
        focus: [0.0, 0.0, 40.0],
        id: 9,
        times: &[0.1, 0.25, 0.4, 0.55, 0.7, 0.9, 1.2, 1.6],
        kind: Kind::Bolt { speed: 360.0 },
    },
    Scenario {
        name: "autocannon_hit",
        focus: [-124.0, 0.0, 30.0],
        id: 18,
        times: &[0.05, 0.12, 0.2, 0.3, 0.45, 0.6, 0.8, 1.1],
        kind: Kind::Hit,
    },
    Scenario {
        name: "mech_explosion",
        focus: [0.0, 0.0, 30.0],
        id: 39,
        times: &[0.1, 0.3, 0.6, 0.9, 1.3, 1.8, 2.5, 3.5],
        kind: Kind::Explosion,
    },
];

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut out = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/out"));
    let (mut fw, mut fh) = (320u32, 200u32);
    let mut cols = 4usize;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(args.next().ok_or("--out DIR")?),
            "--cols" => cols = args.next().ok_or("--cols N")?.parse()?,
            "--frame" => {
                let v = args.next().ok_or("--frame WxH")?;
                let (w, h) = v.split_once('x').ok_or("--frame WxH")?;
                fw = w.parse()?;
                fh = h.parse()?;
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }
    std::fs::create_dir_all(&out)?;
    let root = std::env::var("MC2_DATA").map_err(|_| "set MC2_DATA")?;
    let mut vfs = Vfs::from_system_cfg(root)?;
    let gpu = Gpu::headless(&RenderConfig::default())?;
    println!(
        "gpu: {} ({}), msaa {}",
        gpu.adapter_name, gpu.backend, gpu.samples
    );
    let mut renderer = FxRenderer::new(&gpu)?;
    let mut textures = FxTextures::default();

    let mut camera = GameCamera::new(CameraSettings::default(), (fw as f32, fh as f32));
    camera.projection_angle = 22.0;
    camera.rotation = 0.0;

    for sc in SCENARIOS {
        let mut fx = FxSystem::from_vfs(&mut vfs, 7)?;
        let mut camera = camera.clone();
        camera.target = mc2_render::math::Vec3::new(sc.focus[0], sc.focus[1], sc.focus[2]);
        let wide = sc.focus[0] == 0.0 && matches!(sc.kind, Kind::Bolt { .. } | Kind::Beam);
        camera.altitude = if wide { 700.0 } else { 600.0 };
        let camera = &camera;
        let frames = render_scenario(
            sc,
            &mut fx,
            &mut vfs,
            &mut renderer,
            &mut textures,
            &gpu,
            camera,
            fw,
            fh,
        )?;
        let rows = frames.len().div_ceil(cols);
        let (sw, sh) = (fw as usize * cols, fh as usize * rows);
        let mut sheet = vec![0u8; sw * sh * 4];
        for (i, f) in frames.iter().enumerate() {
            let (cx, cy) = ((i % cols) * fw as usize, (i / cols) * fh as usize);
            for y in 0..fh as usize {
                let src = &f[y * fw as usize * 4..(y + 1) * fw as usize * 4];
                let dst = ((cy + y) * sw + cx) * 4;
                sheet[dst..dst + src.len()].copy_from_slice(src);
            }
        }
        let path = out.join(format!("fx_{}.png", sc.name));
        write_png(&path, &sheet, sw as u32, sh as u32)?;
        println!("wrote {} ({}x{})", path.display(), sw, sh);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_scenario(
    sc: &Scenario,
    fx: &mut FxSystem,
    vfs: &mut Vfs,
    renderer: &mut FxRenderer,
    textures: &mut FxTextures,
    gpu: &Gpu,
    camera: &GameCamera,
    fw: u32,
    fh: u32,
) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error>> {
    let mut frames = Vec::new();
    let mut t = 0.0f64;
    let mut next = 0usize;

    let mut trail: Option<FxHandle> = None;
    let mut hit_started = false;
    let mut bolt_pos = MUZZLE_AT;
    let travel = (MUZZLE_AT - HIT_AT).length();

    // t = 0: the shot
    match sc.kind {
        Kind::Bolt { .. } => {
            fx.spawn_weapon(
                sc.id,
                WeaponFx::MuzzleFlash,
                SpawnParams::at(FxTransform::muzzle(MUZZLE_AT, 90.0)),
            );
            trail = fx.spawn_weapon(
                sc.id,
                WeaponFx::Trail,
                SpawnParams::at(FxTransform::bolt(bolt_pos, HIT_AT)).looped(),
            );
        }
        Kind::Beam => {
            fx.spawn_weapon(
                sc.id,
                WeaponFx::MuzzleFlash,
                SpawnParams::at(FxTransform::muzzle(MUZZLE_AT, 90.0)),
            );
            trail = fx.spawn_weapon(
                sc.id,
                WeaponFx::Trail,
                SpawnParams::at(FxTransform::bolt(HIT_AT, MUZZLE_AT)).looped(),
            );
        }
        Kind::Explosion => {
            fx.spawn_weapon(
                sc.id,
                WeaponFx::Trail,
                SpawnParams::at(FxTransform::at(mc2_fx::system::game_to_fx(0.0, 0.0, 30.0))),
            );
        }
        Kind::Hit => {
            fx.spawn_weapon(
                sc.id,
                WeaponFx::Hit,
                SpawnParams::at(FxTransform::at(mc2_fx::system::game_to_fx(
                    HIT_AT.x, HIT_AT.y, HIT_AT.z,
                ))),
            );
        }
    }

    let last = *sc.times.last().ok_or("no times")?;
    while next < sc.times.len() && t <= last + DT {
        // move the projectile
        if let Kind::Bolt { speed } = sc.kind {
            if !hit_started {
                let flown = speed * t as f32;
                if flown >= travel {
                    hit_started = true;
                    if let Some(h) = trail.take() {
                        fx.kill(h);
                    }
                    fx.spawn_weapon(
                        sc.id,
                        WeaponFx::Hit,
                        SpawnParams::at(FxTransform::at(mc2_fx::system::game_to_fx(
                            HIT_AT.x, HIT_AT.y, HIT_AT.z,
                        ))),
                    );
                } else {
                    let f = flown / travel;
                    bolt_pos = MUZZLE_AT + (HIT_AT - MUZZLE_AT) * f;
                    if let Some(h) = trail {
                        fx.set_transform(h, FxTransform::bolt(bolt_pos, HIT_AT));
                    }
                }
            }
        }
        if let Kind::Beam = sc.kind {
            if t >= 0.5 && !hit_started {
                hit_started = true;
                if let Some(h) = trail.take() {
                    fx.stop(h);
                }
                fx.spawn_weapon(
                    sc.id,
                    WeaponFx::Hit,
                    SpawnParams::at(FxTransform::at(mc2_fx::system::game_to_fx(
                        HIT_AT.x, HIT_AT.y, HIT_AT.z,
                    ))),
                );
            }
        }
        fx.update(t);
        if t + 1e-9 >= sc.times[next] {
            frames.push(draw_frame(
                fx, vfs, renderer, textures, gpu, camera, fw, fh,
            )?);
            next += 1;
        }
        t += DT;
    }
    Ok(frames)
}

#[allow(clippy::too_many_arguments)]
fn draw_frame(
    fx: &FxSystem,
    vfs: &mut Vfs,
    renderer: &mut FxRenderer,
    textures: &mut FxTextures,
    gpu: &Gpu,
    camera: &GameCamera,
    fw: u32,
    fh: u32,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut list = mc2_fx::draw::DrawList::default();
    fx.draw(&render::camera_to_world(camera), &mut list);
    let mut batches = ground_and_props();
    batches.extend(render::batches(&list, vfs, textures, &mut |n, w, h, px| {
        renderer.add_texture(gpu, n, w, h, px)
    }));

    Ok(renderer.render_offscreen(
        gpu,
        fw,
        fh,
        [0.42, 0.5, 0.58],
        &camera.gpu_view_proj(),
        &batches,
    )?)
}

fn quad(pos: [[f32; 3]; 4], color: [f32; 4]) -> (Vec<FxVertex>, Vec<u32>) {
    let v = pos
        .iter()
        .map(|p| FxVertex {
            pos: *p,
            uv: [0.0, 0.0],
            color,
        })
        .collect();
    (v, vec![0, 1, 2, 0, 2, 3])
}

/// A checkered flat ground and two box "mechs" (game world coordinates), all opaque.
fn ground_and_props() -> Vec<FxBatch> {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    let mut push = |(v, i): (Vec<FxVertex>, Vec<u32>)| {
        let base = vertices.len() as u32;
        vertices.extend(v);
        indices.extend(i.iter().map(|i| base + i));
    };
    let n = 12;
    let size = 128.0f32;
    for gx in 0..n {
        for gy in 0..n {
            let x0 = (gx as f32 - n as f32 / 2.0) * size;
            let y0 = (gy as f32 - n as f32 / 2.0) * size;
            let shade = if (gx + gy) % 2 == 0 { 0.30 } else { 0.26 };
            push(quad(
                [
                    [x0, y0, 0.0],
                    [x0 + size, y0, 0.0],
                    [x0 + size, y0 + size, 0.0],
                    [x0, y0 + size, 0.0],
                ],
                [shade, shade * 0.95, shade * 0.8, 1.0],
            ));
        }
    }
    for (c, color) in [
        (SHOOTER, [0.2, 0.3, 0.5, 1.0]),
        (TARGET, [0.5, 0.25, 0.2, 1.0]),
    ] {
        let (hx, hy, h) = (20.0, 20.0, 60.0);
        let (x, y) = (c.x, c.y);
        let faces: [[[f32; 3]; 4]; 5] = [
            [
                [x - hx, y - hy, h],
                [x + hx, y - hy, h],
                [x + hx, y + hy, h],
                [x - hx, y + hy, h],
            ],
            [
                [x - hx, y - hy, 0.0],
                [x + hx, y - hy, 0.0],
                [x + hx, y - hy, h],
                [x - hx, y - hy, h],
            ],
            [
                [x - hx, y + hy, 0.0],
                [x + hx, y + hy, 0.0],
                [x + hx, y + hy, h],
                [x - hx, y + hy, h],
            ],
            [
                [x - hx, y - hy, 0.0],
                [x - hx, y + hy, 0.0],
                [x - hx, y + hy, h],
                [x - hx, y - hy, h],
            ],
            [
                [x + hx, y - hy, 0.0],
                [x + hx, y + hy, 0.0],
                [x + hx, y + hy, h],
                [x + hx, y - hy, h],
            ],
        ];
        for (k, f) in faces.iter().enumerate() {
            let s = 1.0 - 0.12 * k as f32;
            push(quad(*f, [color[0] * s, color[1] * s, color[2] * s, 1.0]));
        }
    }
    vec![FxBatch {
        texture: None,
        blend: FxBlend::Opaque,
        z_write: true,
        z_compare: true,
        clamp: false,
        ground_fade: false,
        priority: 0,
        vertices,
        indices,
    }]
}
