//! Drawing hook checks on the headless GPU (offscreen, no window). A missing adapter skips only
//! when `MC2_SKIP_GPU_TESTS=1`; the data test also needs `MC2_DATA`.

use mc2_formats::vfs::Vfs;
use mc2_fx::draw::DrawList;
use mc2_fx::render::{self, FxTextures};
use mc2_fx::system::{FxSystem, SpawnParams};
use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::fx::{FxBatch, FxBlend, FxRenderer, FxVertex};
use mc2_render::gpu::Gpu;

fn gpu() -> Option<Gpu> {
    match Gpu::headless(&RenderConfig::default()) {
        Ok(gpu) => Some(gpu),
        Err(error)
            if error.to_string().contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
        {
            eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU adapter ({error})");
            None
        }
        Err(error) => panic!("headless effects renderer needs a GPU adapter: {error}"),
    }
}

fn camera(w: u32, h: u32) -> GameCamera {
    let mut c = GameCamera::new(CameraSettings::default(), (w as f32, h as f32));
    c.projection_angle = 60.0;
    c.altitude = 700.0;
    c
}

fn quad(z: f32, half: f32, color: [f32; 4], blend: FxBlend, z_write: bool) -> FxBatch {
    let v = |x: f32, y: f32| FxVertex {
        pos: [x, y, z],
        uv: [0.0, 0.0],
        color,
    };
    FxBatch {
        texture: None,
        blend,
        z_write,
        z_compare: true,
        clamp: false,
        ground_fade: false,
        priority: 0,
        vertices: vec![
            v(-half, -half),
            v(half, -half),
            v(half, half),
            v(-half, half),
        ],
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

#[test]
fn additive_and_alpha_batches_blend_over_the_ground_and_respect_depth() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter: skipping");
        return;
    };
    let fx = FxRenderer::new(&gpu).unwrap();
    let (w, h) = (64u32, 64u32);
    let cam = camera(w, h);
    let vp = cam.gpu_view_proj();
    let ground = quad(0.0, 2000.0, [0.5, 0.5, 0.5, 1.0], FxBlend::Opaque, true);
    let mut add = quad(1.0, 60.0, [0.25, 0.0, 0.0, 1.0], FxBlend::Additive, false);
    add.priority = 5;
    let mut alpha = quad(2.0, 20.0, [0.0, 1.0, 0.0, 0.5], FxBlend::Alpha, false);
    alpha.priority = 6;
    // a quad under the ground never shows (z compare)
    let hidden = quad(-5.0, 30.0, [1.0, 1.0, 1.0, 1.0], FxBlend::Additive, false);

    let base = fx
        .render_offscreen(
            &gpu,
            w,
            h,
            [0.0, 0.0, 0.0],
            &vp,
            std::slice::from_ref(&ground),
        )
        .unwrap();
    let img = fx
        .render_offscreen(
            &gpu,
            w,
            h,
            [0.0, 0.0, 0.0],
            &vp,
            &[ground, add, alpha, hidden],
        )
        .unwrap();
    let c = px(&img, w, w / 2, h / 2);
    let g = px(&base, w, w / 2, h / 2);
    // ground (0.5 grey = 127/128) + additive red 0.25, then 50% green over it
    assert!((124..=131).contains(&g[0]), "ground {g:?}");
    // red: 127.5 + 63.75 = 191 (additive), then alpha blend 0.5 of (0,1,0): r halves -> ~95
    assert!(c[1] > g[1] + 40, "green not blended in: {c:?} vs {g:?}");
    assert!(c[0] < g[0], "red not reduced by the alpha quad: {c:?}");
    // a point outside the alpha quad but inside the additive one: red raised, green unchanged
    let edge = px(&img, w, w / 2 + 12, h / 2);
    let edge_base = px(&base, w, w / 2 + 12, h / 2);
    assert!(
        edge[0] > edge_base[0] + 40 || edge == edge_base,
        "additive red expected: {edge:?} vs {edge_base:?}"
    );
}

#[test]
fn a_shipped_explosion_draws_pixels_through_the_hook() {
    let Some(gpu) = gpu() else {
        return;
    };
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let mut fx = FxSystem::from_vfs(&mut vfs, 3).unwrap();
    let mut renderer = FxRenderer::new(&gpu).unwrap();
    let mut textures = FxTextures::default();
    let (w, h) = (160u32, 100u32);
    let mut cam = camera(w, h);
    cam.target = mc2_render::math::Vec3::new(0.0, 0.0, 30.0);
    cam.projection_angle = 22.0;
    cam.altitude = 600.0;
    let place = mc2_fx::system::FxTransform::at(mc2_fx::system::game_to_fx(0.0, 0.0, 30.0));
    fx.spawn("Mech_Explosion", SpawnParams::at(place))
        .expect("Mech_Explosion");
    let mut t = 0.0;
    for _ in 0..165 {
        t += 1.0 / 60.0;
        fx.update(t);
    }
    let mut list = DrawList::default();
    fx.draw(&render::camera_to_world(&cam), &mut list);
    assert!(list.triangle_count() > 0);
    let batches = render::batches(&list, &mut vfs, &mut textures, &mut |n, tw, th, px| {
        renderer.add_texture(&gpu, n, tw, th, px)
    });
    assert!(batches.iter().any(|batch| batch.ground_fade));
    let clear = [0.2, 0.2, 0.2];
    let with = renderer
        .render_offscreen(&gpu, w, h, clear, &cam.gpu_view_proj(), &batches)
        .unwrap();
    let without = renderer
        .render_offscreen(&gpu, w, h, clear, &cam.gpu_view_proj(), &[])
        .unwrap();
    let changed = with
        .chunks(4)
        .zip(without.chunks(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 150, "only {changed} pixels changed");
}

#[test]
fn shipped_water_wake_does_not_receive_explosion_ground_fade() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let mut fx = FxSystem::from_vfs(&mut vfs, 3).unwrap();
    for name in ["Mech_Wake", "Hovercraft_Wake"] {
        fx.spawn(name, SpawnParams::at(mc2_fx::math::Mat::IDENTITY))
            .unwrap_or_else(|| panic!("{name}"));
    }
    for frame in 1..=12 {
        fx.update(frame as f64 / 60.0);
    }
    let mut list = DrawList::default();
    fx.draw(
        &mc2_fx::render::camera_to_world(&camera(160, 100)),
        &mut list,
    );
    assert!(!list.batches.is_empty(), "the shipped wakes should draw");
    assert!(list.batches.iter().all(|batch| !batch.ground_fade));
}

#[test]
fn shipped_explosion_batches_receive_ground_fade() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let mut fx = FxSystem::from_vfs(&mut vfs, 3).unwrap();
    let place = mc2_fx::system::game_to_fx(0.0, 0.0, 30.0);
    fx.spawn(
        "Mech_Explosion",
        SpawnParams::at(mc2_fx::math::Mat::from_translation(place)),
    )
    .expect("Mech_Explosion");
    for frame in 1..=165 {
        fx.update(frame as f64 / 60.0);
    }
    let mut list = DrawList::default();
    fx.draw(
        &mc2_fx::render::camera_to_world(&camera(160, 100)),
        &mut list,
    );
    assert!(
        !list.batches.is_empty(),
        "the shipped explosion should draw"
    );
    assert!(list.batches.iter().any(|batch| batch.ground_fade));
}
