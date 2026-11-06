//! The `DT`/`DeT` detail-texture second pass changes the pixels (headless GPU, offscreen). A missing adapter skips only when
//! `MC2_SKIP_GPU_TESTS=1`. The frame with both passes is written to `$CARGO_TARGET_TMPDIR/visual/detail-pass.png`.

#![allow(clippy::chunks_exact_to_as_chunks)]
use mc2_fx::draw::DrawList;
use mc2_fx::math::{Mat, Rgba, Vec3};
use mc2_fx::mlrshape::{MeshKind, MlrShape, Primitive};
use mc2_fx::render;
use mc2_fx::singleton::draw_shape;
use mc2_fx::state::MlrState;
use mc2_gfx::config::RenderConfig;
use mc2_render::camera::{CameraSettings, GameCamera};
use mc2_render::fx::FxRenderer;
use mc2_render::gpu::{write_png, Gpu};

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
        Err(error) => panic!("needs a GPU adapter: {error}"),
    }
}

/// A big triangle whose first pass is opaque and whose detail pass is additive.
fn shape(with_second_pass: bool) -> MlrShape {
    let mut base = MlrState::game_draw_state();
    base.set_z_write(true);
    base.set_blend_bits(0); // OneZero: opaque
    let mut detail = base.clone();
    detail.set_blend_bits(1); // OneOne: additive
    detail.set_z_write(false);
    MlrShape {
        primitives: vec![Primitive {
            class_id: 0,
            kind: MeshKind::Tri,
            coords: vec![
                Vec3::new(-300.0, -300.0, 0.0),
                Vec3::new(300.0, -300.0, 0.0),
                Vec3::new(0.0, 300.0, 0.0),
            ],
            tex_coords: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            lengths: Vec::new(),
            draw_mode: 0,
            state: base,
            index: vec![0, 1, 2],
            state2: with_second_pass.then_some(detail),
            detail: with_second_pass.then_some([0.0, 0.0, 4.0, 4.0]),
            detail_fade: None,
            colors: vec![Rgba::WHITE; 3],
            normals: Vec::new(),
        }],
    }
}

#[test]
fn the_detail_pass_of_a_det_mesh_changes_the_pixels() {
    let Some(gpu) = gpu() else {
        return;
    };
    let fx = FxRenderer::new(&gpu).unwrap();
    let (w, h) = (160u32, 120u32);
    let mut cam = GameCamera::new(CameraSettings::default(), (w as f32, h as f32));
    cam.projection_angle = 60.0;
    cam.altitude = 700.0;
    let render_frame = |second: bool| {
        let mut list = DrawList::default();
        let mut master = MlrState::game_draw_state();
        master.set_z_write(true);
        let to_world = Mat::IDENTITY;
        draw_shape(
            &shape(second),
            &to_world,
            Vec3::new(0.0, -600.0, 600.0),
            Vec3::new(1.0, 1.0, 1.0),
            Rgba::new(0.25, 0.25, 0.25, 1.0),
            &master,
            &mut list,
        );
        let batches = render::to_fx_batches(&list, |_| None);
        assert_eq!(batches.len(), if second { 2 } else { 1 });
        fx.render_offscreen(&gpu, w, h, [0.0; 3], &cam.gpu_view_proj(), &batches)
            .unwrap()
    };
    let one = render_frame(false);
    let two = render_frame(true);
    let changed = one
        .chunks_exact(4)
        .zip(two.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    let drawn = one.chunks_exact(4).filter(|p| p[..3] != [0, 0, 0]).count();
    assert!(drawn > 100, "the first pass draws nothing ({drawn})");
    assert!(
        changed > drawn / 2,
        "the second pass changed {changed} of {drawn} drawn pixels"
    );
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("visual");
    std::fs::create_dir_all(&dir).unwrap();
    write_png(&dir.join("detail-pass.png"), &two, w, h).unwrap();
}
