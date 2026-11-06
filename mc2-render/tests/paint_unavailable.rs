//! The comparison tool refuses a view with a mech or vehicle that has no paint source, instead of
//! returning a picture of it on raw masks (`PopulateReport::ensure_painted`).  The negative launch
//! is a private fixture root: the shipped archives hard-linked from `MC2_DATA` (read only), with
//! `options.cfg` left out.  Needs `MC2_DATA` (skips cleanly without it) and a GPU adapter.

#[macro_use]
mod common;

use mc2_gfx::config::RenderConfig;
use mc2_render::compare::render_start_view;
use std::path::{Path, PathBuf};

use common::link_helper::link_tree;

/// The shipped data root with every file hard-linked into `target/`, except `options.cfg`.
/// Hard links share the archives without copying them; nothing under `MC2_DATA` is written.
fn root_without_options_cfg(src: &Path) -> PathBuf {
    let dst = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("mc2-render-paint-unavailable");
    let _ = std::fs::remove_dir_all(&dst);
    link_tree(src, &dst);
    dst
}

#[test]
fn compare_is_unavailable_without_the_prefs_colours() {
    let Some(data) = std::env::var_os("MC2_DATA") else {
        eprintln!("MC2_DATA not set: skipping");
        return;
    };
    let cfg = RenderConfig {
        msaa: 1,
        ..RenderConfig::default()
    };
    if common::headless_or_skip("paint_unavailable", &cfg).is_none() {
        return;
    }
    let root = root_without_options_cfg(Path::new(&data));
    let err = render_start_view(&cfg, &root, "mc2_01", 192, 108)
        .expect_err("a view with units on raw masks must not be returned");
    let msg = err.to_string();
    assert!(msg.contains("paint unavailable"), "{msg}");
    assert!(msg.contains("part "), "{msg}");
    assert!(msg.contains("options.cfg"), "{msg}");
}
