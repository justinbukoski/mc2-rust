#![allow(clippy::chunks_exact_to_as_chunks)]

//! A synthetic pack changes the rendered mc2_01 view; no pack leaves it as is. Needs `MC2_DATA`
//! (skips cleanly without it) and a GPU adapter: a missing adapter fails unless
//! `MC2_SKIP_GPU_TESTS=1`.

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_gfx::texture as tx;
use mc2_render::compare::{render_start_view, side_by_side};
use std::path::PathBuf;

#[test]
fn synthetic_pack_changes_the_view_and_no_pack_does_not() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let root = PathBuf::from(root);
    let dir = std::env::temp_dir().join(format!("mc2_compare_pack_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (w, h) = (192u32, 108u32);
    // 1. original, dumping every texture it asks for
    let dump_cfg = RenderConfig {
        msaa: 1,
        hires_dir: Some(dir.clone()),
        hires_dump: true,
        ..RenderConfig::default()
    };
    if common::headless_or_skip("compare_pack", &dump_cfg).is_none() {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    // any failure here (no adapter, mission load, render) fails the test
    let plain = render_start_view(&dump_cfg, &root, "mc2_01", w, h).expect("render without a pack");
    gate_println!(
        "GPU compare_pack: rendered {} bytes without a pack",
        plain.len()
    );
    // 2. a pack with a 2x, red-shifted colour map; the dump folder has no replacements yet
    let key = std::fs::read_dir(dir.join("_dump"))
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.contains(".burnin.tga"))
        .expect("the colour map was requested");
    let key = key.strip_suffix(".png").unwrap();
    let orig = tx::load_replacement(&dir.join("_dump"), key).unwrap();
    let (nw, nh) = (orig.w * 2, orig.h * 2);
    let mut px = Vec::with_capacity((nw * nh * 4) as usize);
    for y in 0..nh {
        for x in 0..nw {
            let o = (((y / 2) * orig.w + x / 2) * 4) as usize;
            px.extend_from_slice(&[255, orig.rgba[o + 1] / 2, orig.rgba[o + 2] / 2, 255]);
        }
    }
    let pack = dir.join("pack");
    std::fs::create_dir_all(&pack).unwrap();
    tx::write_png(
        &tx::replacement_path(&pack, key),
        &px,
        nw,
        nh,
        mc2_gfx::MC2GFX_TEXFMT_RGBA8,
    )
    .unwrap();

    let with_cfg = RenderConfig {
        msaa: 1,
        hires_dir: Some(pack.clone()),
        ..RenderConfig::default()
    };
    let none_cfg = RenderConfig {
        msaa: 1,
        hires_dir: None,
        ..RenderConfig::default()
    };
    let packed = render_start_view(&with_cfg, &root, "mc2_01", w, h).unwrap();
    let again = render_start_view(&none_cfg, &root, "mc2_01", w, h).unwrap();
    assert_eq!(plain, again, "no pack, no change (and the dump is passive)");
    let differing = plain
        .chunks_exact(4)
        .zip(packed.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(
        differing > (w * h / 4) as usize,
        "{differing} pixels differ"
    );
    let (img, total) = side_by_side(&plain, &packed, w, h);
    assert_eq!(img.len(), (total * h * 4) as usize);
    gate_println!(
        "GPU compare_pack: {differing} of {} pixels differ with the pack",
        w * h
    );
    let _ = std::fs::remove_dir_all(&dir);
}
