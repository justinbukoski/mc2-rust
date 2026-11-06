#![allow(clippy::chunks_exact_to_as_chunks)]

//! MSAA and anisotropic filtering reach the pipelines and samplers (offscreen, no window).
//! A missing GPU adapter fails the tests unless `MC2_SKIP_GPU_TESTS=1` (then one SKIPPED line is
//! printed); the 4x assertion never falls back silently to 1x.

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::fx::{FxBatch, FxBlend, FxRenderer, FxVertex};
use mc2_render::gpu::Gpu;
use mc2_render::math::Mat4;
use mc2_render::scene::SceneRenderer;
use mc2_render::settings::GraphicsSettings;

fn gpu(test: &str, settings: &GraphicsSettings) -> Option<Gpu> {
    common::headless_or_skip(test, &settings.apply_to(&RenderConfig::default()))
}

/// A white triangle with a slanted edge on black, in clip space (identity view-projection).
fn triangle() -> FxBatch {
    let v = |x: f32, y: f32| FxVertex {
        pos: [x, y, 0.5],
        uv: [0.0, 0.0],
        color: [1.0, 1.0, 1.0, 1.0],
    };
    FxBatch {
        texture: None,
        blend: FxBlend::Opaque,
        z_write: true,
        z_compare: true,
        clamp: false,
        ground_fade: false,
        priority: 0,
        vertices: vec![v(-0.8, -0.8), v(0.9, -0.3), v(-0.2, 0.9)],
        indices: vec![0, 1, 2],
    }
}

/// Renders the triangle and counts pixels that are neither the clear colour nor white.
fn edge_pixels(gpu: &Gpu) -> usize {
    let fx = FxRenderer::new(gpu).unwrap();
    let (w, h) = (64, 64);
    let img = fx
        .render_offscreen(gpu, w, h, [0.0, 0.0, 0.0], &Mat4::IDENTITY, &[triangle()])
        .unwrap();
    let lit = img.chunks_exact(4).filter(|p| p[0] > 200).count();
    assert!(lit > 500, "the triangle is drawn ({lit} lit pixels)");
    img.chunks_exact(4)
        .filter(|p| p[0] > 8 && p[0] < 247)
        .count()
}

#[test]
fn four_x_msaa_smooths_edges_that_one_x_leaves_hard() {
    let one = GraphicsSettings {
        msaa: 1,
        ..GraphicsSettings::default()
    };
    let four = GraphicsSettings::default();
    let Some(g4) = gpu("msaa 4x", &four) else {
        return;
    };
    let g1 = gpu("msaa 1x", &one).expect("1x device after 4x device succeeded");
    assert_eq!(g1.samples, 1);
    assert_eq!(
        g4.samples, 4,
        "the adapter must provide 4x MSAA for a 4x request"
    );
    let hard = edge_pixels(&g1);
    let soft = edge_pixels(&g4);
    assert_eq!(hard, 0, "1x has no intermediate edge pixels");
    assert!(
        soft > 20,
        "{}x MSAA gives anti-aliased edges ({soft})",
        g4.samples
    );
    gate_println!("verified {}x MSAA render: {soft} edge pixels", g4.samples);
}

#[test]
fn anisotropic_levels_build_valid_scene_samplers() {
    for aniso in [1u16, 4, 16] {
        let s = GraphicsSettings {
            aniso,
            msaa: 1,
            ..GraphicsSettings::default()
        };
        let Some(g) = gpu(&format!("aniso {aniso}"), &s) else {
            return;
        };
        assert_eq!(g.cfg.aniso, aniso);
        let device = g.device.clone();
        let guard = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _scene = SceneRenderer::new(g).unwrap();
        let err = pollster::block_on(guard.pop());
        assert!(err.is_none(), "aniso {aniso}: {err:?}");
    }
}
