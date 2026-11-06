#![allow(clippy::chunks_exact_to_as_chunks)]

//! Headless before/after: renders the same mission view without and with a texture pack and
//! writes one side-by-side PNG (left original, right with the pack). Never opens a window.
//!
//! usage: MC2_DATA=<data root> cargo run --release -p mc2-render --example compare_pack --
//!        [--pack DIR (default: MC2_HIRES_DIR)] [--out FILE.png] [--size WxH]
//!        [--view terrain|objects] [MISSION (mc2_01)]

use mc2_gfx::config::RenderConfig;
use mc2_render::compare::{render_pack_view, write_side_by_side_png, PackView};
use std::path::PathBuf;

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> mc2_render::Result<()> {
    let mut pack = std::env::var_os("MC2_HIRES_DIR").map(PathBuf::from);
    let mut out = PathBuf::from("compare_pack.png");
    let (mut w, mut h) = (1920u32, 1080u32);
    let mut mission = "mc2_01".to_string();
    let mut view = PackView::Terrain;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--pack" => pack = Some(PathBuf::from(args.next().expect("--pack DIR"))),
            "--out" => out = PathBuf::from(args.next().expect("--out FILE")),
            "--view" => {
                view = match args.next().expect("--view terrain|objects").as_str() {
                    "terrain" => PackView::Terrain,
                    "objects" => PackView::Objects,
                    other => panic!("--view {other}: terrain or objects"),
                };
            }
            "--size" => {
                let s = args.next().expect("--size WxH");
                let (a, b) = s.split_once('x').expect("--size WxH");
                (w, h) = (a.parse().expect("width"), b.parse().expect("height"));
            }
            other => mission = other.to_string(),
        }
    }
    let Some(pack) = pack else {
        eprintln!("no pack: pass --pack DIR or set MC2_HIRES_DIR");
        std::process::exit(2);
    };
    let root = std::env::var_os("MC2_DATA").expect("MC2_DATA must point at the game data");
    let base = RenderConfig {
        hires_dir: None,
        ..RenderConfig::from_env()
    };
    let with = RenderConfig {
        hires_dir: Some(pack),
        ..base.clone()
    };
    let before = render_pack_view(&base, root.as_ref(), &mission, w, h, view)?;
    let after = render_pack_view(&with, root.as_ref(), &mission, w, h, view)?;
    let changed = before
        .chunks_exact(4)
        .zip(after.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    println!("{changed} of {} pixels differ", w * h);
    write_side_by_side_png(&out, &before, &after, w, h, 128)?;
    let bytes = std::fs::metadata(&out)?.len();
    println!("wrote {} ({bytes} bytes)", out.display());
    Ok(())
}
