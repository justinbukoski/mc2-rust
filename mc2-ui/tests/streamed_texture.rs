//! A streamed texture (a movie frame re-inserted every frame) never probes the hi-res pack, so
//! its upload logs no fallback line however often it is drawn. A static texture keeps the lookup
//! (the control: it logs its missing entry). A missing adapter skips only when
//! `MC2_SKIP_GPU_TESTS=1`.

use mc2_gfx::config::RenderConfig;
use mc2_gfx::texture::with_fallback_log;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::{Blend, DrawList};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::textures::TexId;
use mc2_ui::UiAssets;

fn gpu(hires_dir: std::path::PathBuf) -> Option<Gpu> {
    let cfg = RenderConfig {
        hires_dir: Some(hires_dir),
        ..RenderConfig::default()
    };
    match Gpu::headless(&cfg) {
        Ok(g) => Some(g),
        Err(e)
            if e.to_string().contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                    == Some(std::ffi::OsStr::new("1")) =>
        {
            eprintln!("SKIPPED: MC2_SKIP_GPU_TESTS=1 and no usable GPU adapter ({e})");
            None
        }
        Err(e) => panic!("UI render test needs a GPU adapter: {e}"),
    }
}

/// One frame: a single full-screen quad of `tex`, drawn through the public 2D pipeline.
fn draw_frame(r: &mut UiRenderer, gpu: &Gpu, assets: &UiAssets, tex: TexId) {
    let size = (64, 64);
    let mut dl = DrawList::new(64, 64);
    dl.rect_quad(
        Some(tex),
        Blend::Opaque,
        (0.0, 0.0, 64.0, 64.0),
        (0.0, 0.0, 1.0, 1.0),
        0xffff_ffff,
    );
    let t = Offscreen::new(gpu, size.0, size.1);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    t.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new((64, 64), size, ScaleMode::Fit);
    r.draw(gpu, &mut enc, &t.target(), &layout, &dl, assets);
    t.read_rgba(gpu, enc).unwrap();
}

#[test]
fn streamed_frames_never_probe_the_hires_pack() {
    let root = option_env!("CARGO_TARGET_TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target"));
    let dir = root.join(format!("mc2_ui_streamed_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // the pack exists but holds no entry for either name, so a lookup would log "missing"
    std::fs::create_dir_all(&dir).unwrap();
    let Some(gpu) = gpu(dir.clone()) else {
        return;
    };
    let mut r = UiRenderer::new(&gpu);
    let mut assets = UiAssets::default();
    let rgba = vec![200u8; 2 * 2 * 4];

    // control: a static texture is looked up (and logs its missing entry) on upload
    let static_id =
        assets
            .textures
            .insert("data/art/streamed_probe_static.tga", 2, 2, rgba.clone());
    let (_, static_logs) = with_fallback_log(|| draw_frame(&mut r, &gpu, &assets, static_id));
    assert!(
        static_logs
            .iter()
            .any(|l| l.contains("streamed_probe_static")),
        "control: a static texture must still log its missing pack entry, got {static_logs:?}"
    );

    // the movie frame is re-inserted with new pixels every frame, as the HUD does
    let mut frame_logs = Vec::new();
    for n in 0..16u8 {
        let frame = vec![n; 2 * 2 * 4];
        let id = assets
            .textures
            .insert_streamed("mission_video", 2, 2, frame);
        let (_, logs) = with_fallback_log(|| draw_frame(&mut r, &gpu, &assets, id));
        frame_logs.extend(logs);
    }
    assert!(
        frame_logs.iter().all(|l| !l.contains("mission_video")),
        "a streamed frame must not probe the hi-res pack, got {frame_logs:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
