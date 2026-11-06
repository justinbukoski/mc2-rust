//! The ui, cursor, effect and insignia load sites hand the replacement hook the VFS names
//! TEXPACK.md documents, checked against a synthetic game folder and pack folder: a replacement
//! is used, a key of the wrong size is ignored, a missing file means the original pixels.
//! Headless; a missing GPU adapter fails unless `MC2_SKIP_GPU_TESTS=1` (one SKIPPED line).

/// Prints past the test harness's output capture so the gate log shows the GPU evidence.
macro_rules! gate_println {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($arg)*);
    }};
}

use mc2_formats::vfs::Vfs;
use mc2_fx::render::FxTextures;
use mc2_fx::state::TextureRef;
use mc2_gfx::config::RenderConfig;
use mc2_gfx::texture as tx;
use mc2_render::fx::FxTextureId;
use mc2_render::gpu::Gpu;
use mc2_texpack::model::Category;
use mc2_ui::draw::{Blend, DrawList};
use mc2_ui::render::{Offscreen, ScaleMode, UiLayout, UiRenderer};
use mc2_ui::UiAssets;
use std::path::{Path, PathBuf};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];

fn temp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc2_pack_hook_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An uncompressed 32-bit top-left TGA of one colour.
fn tga(w: u16, h: u16, rgba: [u8; 4]) -> Vec<u8> {
    let mut f = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    f.extend_from_slice(&w.to_le_bytes());
    f.extend_from_slice(&h.to_le_bytes());
    f.extend_from_slice(&[32, 0x28]);
    for _ in 0..(w as usize * h as usize) {
        f.extend_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
    }
    f
}

fn put(root: &Path, name: &str, bytes: &[u8]) {
    let p = root.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, bytes).unwrap();
}

fn write_pack(pack: &Path, key: &str, w: u32, h: u32, rgba: [u8; 4]) {
    let px: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
    tx::write_png(
        &tx::replacement_path(pack, key),
        &px,
        w,
        h,
        mc2_gfx::MC2GFX_TEXFMT_RGBA8,
    )
    .unwrap();
}

/// A headless device with the pack folder; a missing adapter fails unless `MC2_SKIP_GPU_TESTS=1`,
/// which prints one SKIPPED line and returns `None`.
fn gpu(test: &str, pack: &Path) -> Option<Gpu> {
    let cfg = RenderConfig {
        hires_dir: Some(pack.to_path_buf()),
        msaa: 1,
        ..RenderConfig::default()
    };
    match Gpu::headless(&cfg) {
        Ok(g) => {
            gate_println!(
                "GPU {test}: adapter {} ({}), {} samples",
                g.adapter_name,
                g.backend,
                g.samples
            );
            Some(g)
        }
        Err(e)
            if e.to_string().contains("adapter")
                && std::env::var("MC2_SKIP_GPU_TESTS").is_ok_and(|v| v == "1") =>
        {
            gate_println!("SKIPPED {test}: MC2_SKIP_GPU_TESTS=1 and no usable GPU ({e})");
            None
        }
        Err(e) => panic!("{test}: no usable GPU adapter ({e}); set MC2_SKIP_GPU_TESTS=1 to skip"),
    }
}

fn px(img: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let o = ((y * w + x) * 4) as usize;
    [img[o], img[o + 1], img[o + 2], img[o + 3]]
}

#[test]
fn categories_are_marked_hooked() {
    for c in [
        Category::Ui,
        Category::Cursor,
        Category::Effect,
        Category::Insignia,
    ] {
        assert!(c.hooked(), "{}", c.as_str());
    }
    assert!(!Category::Font.hooked());
    assert!(!Category::Other.hooked());
}

#[test]
fn ui_art_draws_the_replacement_ignores_wrong_size_and_falls_back() {
    let game = temp("ui_game");
    let pack = temp("ui_pack");
    put(&game, "data/art/hires.tga", &tga(2, 2, RED));
    put(&game, "data/art/wrong.tga", &tga(2, 2, RED));
    put(&game, "data/art/absent.tga", &tga(2, 2, RED));
    // the documented key: sanitised VFS path + the original's size
    let key = tx::texture_key(Some("data/art/hires.tga"), &[], 2, 2, 0);
    assert_eq!(key, "n_data_art_hires.tga_2x2");
    write_pack(&pack, &key, 8, 8, GREEN);
    // a pack made for a 4x4 original must not apply to this 2x2 image
    write_pack(&pack, "n_data_art_wrong.tga_4x4", 8, 8, GREEN);
    let Some(gpu) = gpu("ui_art", &pack) else {
        return;
    };
    let mut vfs = Vfs::new(&game);
    let mut assets = UiAssets::default();
    let ids: Vec<_> = ["hires", "wrong", "absent"]
        .iter()
        .map(|n| assets.textures.load_art(&mut vfs, n).unwrap())
        .collect();

    let mut r = UiRenderer::new(&gpu);
    let mut shown = Vec::new();
    for id in ids {
        let mut dl = DrawList::new(4, 4);
        dl.rect_quad(
            Some(id),
            Blend::Alpha,
            (0.0, 0.0, 4.0, 4.0),
            (0.0, 0.0, 1.0, 1.0),
            0xffff_ffff,
        );
        let t = Offscreen::new(&gpu, 4, 4);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        t.clear(&mut enc, [0.0, 0.0, 0.0, 1.0]);
        let layout = UiLayout::new((4, 4), (4, 4), ScaleMode::Fit);
        r.draw(&gpu, &mut enc, &t.target(), &layout, &dl, &assets);
        shown.push(px(&t.read_rgba(&gpu, enc).unwrap(), 4, 2, 2));
    }
    assert_eq!(shown, vec![GREEN, RED, RED]);
    gate_println!("GPU ui_art: replaced 1 of 3 textures (wrong-size and absent keys ignored)");
}

#[test]
fn cursor_effect_and_insignia_names_match_the_documented_keys() {
    let game = temp("names_game");
    let pack = temp("names_pack");
    put(&game, "data/art/cursors.tga", &tga(2, 2, RED));
    put(&game, "data/tgl/128/flash.tga", &tga(2, 2, RED));
    put(&game, "data/multiplayer/insignia/unit.tga", &tga(2, 2, RED));
    for k in [
        "n_data_art_cursors.tga_2x2",
        "n_data_tgl_128_flash.tga_2x2",
        "n_data_multiplayer_insignia_unit.tga_2x2",
    ] {
        write_pack(&pack, k, 6, 6, GREEN);
    }
    let Some(gpu) = gpu("names", &pack) else {
        return;
    };
    let mut vfs = Vfs::new(&game);

    // cursor art (and every other interface art file) goes through the texture store
    let mut assets = UiAssets::default();
    let id = assets.textures.load_art(&mut vfs, "cursors").unwrap();
    let img = assets.textures.get(id).unwrap();
    assert_eq!(img.name, "data/art/cursors.tga");
    let (p, w, h) = gpu.replacement(&img.name, img.width, img.height, &img.rgba);
    assert_eq!((w, h, &p[..4]), (6, 6, &GREEN[..]));

    // insignia files load through the same store
    let id = assets
        .textures
        .load_tga(&mut vfs, "data/multiplayer/insignia/unit.tga")
        .unwrap();
    let img = assets.textures.get(id).unwrap();
    let (p, w, _) = gpu.replacement(&img.name, img.width, img.height, &img.rgba);
    assert_eq!((w, &p[..4]), (6, &GREEN[..]));

    // effect textures: the upload callback gets the full VFS path
    let mut seen = Vec::new();
    let mut fx = FxTextures::default();
    let t = TextureRef {
        name: "flash".into(),
        instance: 0,
    };
    fx.resolve(
        &mut vfs,
        &t,
        &mut |name: &str, w: u32, h: u32, rgba: &[u8]| {
            seen.push(gpu.replacement(name, w, h, rgba));
            FxTextureId(0)
        },
    )
    .unwrap();
    assert_eq!((seen[0].1, seen[0].2, &seen[0].0[..4]), (6, 6, &GREEN[..]));
    // a missing texture file stays untextured
    let gone = TextureRef {
        name: "nope".into(),
        instance: 0,
    };
    let none = fx.resolve(&mut vfs, &gone, &mut |_: &str, _: u32, _: u32, _: &[u8]| {
        FxTextureId(0)
    });
    assert!(none.is_none());
    gate_println!("GPU names: replaced 3 textures (cursor, insignia, effect)");
}
