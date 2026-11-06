//! The 2D pipeline drawn offscreen and read back. A missing adapter skips only when
//! `MC2_SKIP_GPU_TESTS=1`.

use mc2_gfx::config::RenderConfig;
use mc2_render::gpu::Gpu;
use mc2_ui::draw::{Blend, DrawList, GuiRect};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::UiAssets;

fn gpu() -> Option<Gpu> {
    let cfg = RenderConfig {
        hires_dir: None,
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

fn draw(gpu: &Gpu, assets: &UiAssets, dl: &DrawList, ui: (u32, u32), size: (u32, u32)) -> Vec<u8> {
    let mut r = UiRenderer::new(gpu);
    let t = Offscreen::new(gpu, size.0, size.1);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    t.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
    let layout = UiLayout::new(ui, size, ScaleMode::Fit);
    r.draw(gpu, &mut enc, &t.target(), &layout, dl, assets);
    t.read_rgba(gpu, enc).unwrap()
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let o = ((y * w + x) * 4) as usize;
    [img[o], img[o + 1], img[o + 2], img[o + 3]]
}

#[test]
fn flat_quads_blend_and_clip_at_one_to_one() {
    let Some(gpu) = gpu() else { return };
    let assets = UiAssets::default();
    let mut dl = DrawList::new(16, 12);
    dl.fill_rect(GuiRect::new(2, 1, 6, 5), 0xffff_0000);
    // half-transparent white over part of it (blended in the original's gamma space)
    dl.fill_rect(GuiRect::new(4, 3, 8, 7), 0x80ff_ffff);
    dl.push_clip(GuiRect::new(10, 0, 12, 12));
    dl.fill_rect(GuiRect::new(8, 0, 16, 2), 0xff00_ff00);
    dl.pop_clip();
    let img = draw(&gpu, &assets, &dl, (16, 12), (16, 12));
    assert_eq!(px(&img, 16, 2, 1), [255, 0, 0, 255]);
    assert_eq!(px(&img, 16, 5, 4)[1], 128, "50% white over red");
    assert_eq!(
        px(&img, 16, 6, 5)[0..3],
        [128, 128, 128],
        "50% white over black"
    );
    assert_eq!(px(&img, 16, 1, 1), [0, 0, 0, 255]);
    // the green bar only inside the clip
    assert_eq!(px(&img, 16, 9, 0), [0, 0, 0, 255]);
    assert_eq!(px(&img, 16, 10, 0), [0, 255, 0, 255]);
    assert_eq!(px(&img, 16, 12, 0), [0, 0, 0, 255]);
}

#[test]
fn art_scales_up_crisply_and_centred() {
    let Some(gpu) = gpu() else { return };
    let mut assets = UiAssets::default();
    // 2x2 texture: red, green / blue, white
    let tex = assets.textures.insert(
        "test/checker.tga",
        2,
        2,
        vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    );
    let mut dl = DrawList::new(4, 3);
    dl.rect_quad(
        Some(tex),
        Blend::Alpha,
        (1.0, 1.0, 3.0, 3.0),
        (0.0, 0.0, 1.0, 1.0),
        0xffff_ffff,
    );
    // UI 4x3 into 40x40: scale 10, 5 pixels of bars at top and bottom
    let img = draw(&gpu, &assets, &dl, (4, 3), (40, 40));
    let l = UiLayout::new((4, 3), (40, 40), ScaleMode::Fit);
    assert_eq!((l.scale, l.offset), (10.0, (0.0, 5.0)));
    // texel centres are exact
    assert_eq!(px(&img, 40, 15, 20), [255, 0, 0, 255]);
    assert_eq!(px(&img, 40, 25, 20), [0, 255, 0, 255]);
    assert_eq!(px(&img, 40, 15, 30), [0, 0, 255, 255]);
    assert_eq!(px(&img, 40, 25, 30), [255, 255, 255, 255]);
    // inside a texel the colour is flat up to the pixel at its edge
    for x in 10..19 {
        assert_eq!(px(&img, 40, x, 16), [255, 0, 0, 255], "x {x}");
    }
    // outside the quad: the clear colour
    assert_eq!(px(&img, 40, 5, 20), [0, 0, 0, 255]);
}

#[test]
fn nothing_spills_into_the_letterbox_bars() {
    let Some(gpu) = gpu() else { return };
    let assets = UiAssets::default();
    // a layout part starting left of the screen and one below it
    let mut dl = DrawList::new(4, 3);
    dl.rect_quad(
        None,
        Blend::Alpha,
        (-2.0, 0.0, 2.0, 3.0),
        (0.0, 0.0, 0.0, 0.0),
        0xffff_0000,
    );
    dl.rect_quad(
        None,
        Blend::Alpha,
        (2.0, 2.0, 4.0, 5.0),
        (0.0, 0.0, 0.0, 0.0),
        0xff00_ff00,
    );
    // UI 4x3 into 60x30: scale 10, 10 pixel pillars left and right
    let img = draw(&gpu, &assets, &dl, (4, 3), (60, 30));
    assert_eq!(px(&img, 60, 5, 15), [0, 0, 0, 255], "left bar stays clear");
    assert_eq!(px(&img, 60, 15, 15), [255, 0, 0, 255]);
    assert_eq!(px(&img, 60, 35, 25), [0, 255, 0, 255]);
}

#[test]
fn ui_draws_to_the_real_target_over_existing_scene_pixels() {
    let Some(gpu) = gpu() else { return };
    let assets = UiAssets::default();
    let (w, h) = (8u32, 8u32);
    let target = Offscreen::new(&gpu, w, h);
    let real_target = target.target();
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    target.clear(&mut enc, [0.0, 0.0, 1.0, 1.0]);
    let mut dl = DrawList::new(8, 8);
    dl.fill_rect(GuiRect::new(0, 0, 4, 8), 0xffff_0000);
    let mut r = UiRenderer::new(&gpu);
    r.draw(
        &gpu,
        &mut enc,
        &real_target,
        &UiLayout::new((8, 8), (w, h), ScaleMode::Fit),
        &dl,
        &assets,
    );
    let data = target.read_rgba(&gpu, enc).unwrap();
    assert_eq!(&px(&data, w, 1, 1)[..3], [255, 0, 0], "UI on the left");
    assert_eq!(
        px(&data, w, 6, 1),
        [0, 0, 255, 255],
        "scene kept on the right"
    );
}

/// A cell with the original's `+0.1` texel UV bias, upscaled: no pixel along its edges takes the
/// neighbouring cell's colour (the stray lines around DONE).
#[test]
fn biased_cell_has_no_neighbour_bleed_when_upscaled() {
    let Some(gpu) = gpu() else { return };
    let mut assets = UiAssets::default();
    // 16x4 atlas: left 8 texels red, right 8 green
    let mut rgba = Vec::new();
    for _y in 0..4 {
        for x in 0..16 {
            rgba.extend_from_slice(if x < 8 {
                &[255, 0, 0, 255]
            } else {
                &[0, 255, 0, 255]
            });
        }
    }
    let tex = assets.textures.insert("test/cells.tga", 16, 4, rgba);
    let b = 0.1 / 16.0;
    let mut dl = DrawList::new(16, 4);
    // the right cell (texels 8..16) first, then the left cell (0..8) on the left half
    dl.rect_quad(
        Some(tex),
        Blend::Alpha,
        (0.0, 0.0, 8.0, 4.0),
        (b, 0.1 / 4.0, 0.5 + b, 1.0 + 0.1 / 4.0),
        0xffff_ffff,
    );
    // 16x4 into 1024x256 (scale 64 non-integer-texel edges are exercised at 1.28 too)
    for size in [(1024, 256), (20, 5)] {
        let img = draw(&gpu, &assets, &dl, (16, 4), size);
        let half = size.0 / 2;
        for y in 0..size.1 {
            for x in 0..half {
                let p = px(&img, size.0, x, y);
                assert!(
                    p[1] < 8 && p[0] > 247,
                    "neighbour bleed at {x},{y} of {size:?}: {p:?}"
                );
            }
        }
    }
}

/// Glyph quads use exact texel UVs and must sample like they did before the edge inset: at 1:1
/// the output equals the atlas.
#[test]
fn unbiased_glyph_cells_sample_exact_texels_at_native_size() {
    let Some(gpu) = gpu() else { return };
    let mut assets = UiAssets::default();
    // 8x8 atlas with a one-texel checker, glyph cell is the whole thing
    let mut rgba = Vec::new();
    for y in 0..8u32 {
        for x in 0..8u32 {
            let on = (x + y) % 2 == 0;
            rgba.extend_from_slice(&[255, 255, 255, if on { 255 } else { 0 }]);
        }
    }
    let tex = assets.textures.insert("test/glyph.tga", 8, 8, rgba);
    let mut dl = DrawList::new(8, 8);
    dl.rect_quad(
        Some(tex),
        Blend::Alpha,
        (0.0, 0.0, 8.0, 8.0),
        (0.0, 0.0, 1.0, 1.0),
        0xffff_ffff,
    );
    let img = draw(&gpu, &assets, &dl, (8, 8), (8, 8));
    for y in 0..8u32 {
        for x in 0..8u32 {
            let want = if (x + y) % 2 == 0 { 255 } else { 0 };
            assert_eq!(px(&img, 8, x, y)[0], want, "texel {x},{y}");
        }
    }
}
