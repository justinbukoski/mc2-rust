//! Side-by-side comparison of one mission view without and with a texture pack
//! (`MC2_HIRES_DIR`), for the P5 before/after PNGs. Headless; used by
//! `examples/compare_pack.rs` and the tests.
//!
//! Original to this project (tooling); nothing here touches the simulation.

use crate::camera::{CameraSettings, GameCamera};
use crate::error::Result;
use crate::game::GameData;
use crate::gpu::Gpu;
use crate::mission::Mission;
use crate::populate::populate;
use crate::scene::{Frame, SceneRenderer};
use crate::terrain_tex::TerrainTextures;
use mc2_gfx::config::RenderConfig;
use std::path::Path;

/// Which camera a pack comparison uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackView {
    /// Designers' start camera, zoomed out: terrain colour map and detail.
    Terrain,
    /// Zoomed in on a placed mech or building: object textures.
    Objects,
}

/// Renders the mission's start view (the designers' camera from the mission fit) at
/// `width` x `height` with `cfg` (its `hires_dir` decides whether a pack is used) and returns
/// tightly packed RGBA8.
pub fn render_start_view(
    cfg: &RenderConfig,
    data_root: &Path,
    mission: &str,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    render_pack_view(cfg, data_root, mission, width, height, PackView::Terrain)
}

/// Renders one pack-comparison view of `mission`.
pub fn render_pack_view(
    cfg: &RenderConfig,
    data_root: &Path,
    mission: &str,
    width: u32,
    height: u32,
    view: PackView,
) -> Result<Vec<u8>> {
    let gpu = Gpu::headless(cfg)?;
    let mut scene = SceneRenderer::new(gpu)?;
    let mut game = GameData::open(data_root)?;
    let mut m = Mission::load(&mut game, mission)?;
    let tex = TerrainTextures::load(&mut game, &m)?;
    scene.set_terrain(&m, &tex);
    let pop = populate(&mut game, &m, &mut scene, 0)?;
    // a unit without its paint would be drawn with raw masks: the comparison is unavailable instead
    pop.report.ensure_painted()?;
    let settings = {
        let mut sys = game.fit("system.cfg")?;
        CameraSettings::from_system_cfg(&mut sys)
    };
    let mut cam = GameCamera::from_fit(settings, &mut m.fit, (width as f32, height as f32));
    let side = m.terrain.world_side();
    let (mut tx, mut ty) = (cam.target.x, cam.target.y);
    if view == PackView::Objects {
        if let Some(&i) = pop.part_instances.first() {
            let p = pop.instances[i].position;
            tx = p.x;
            ty = p.y;
        } else if let Some(t) = pop.terrain_instances.first() {
            tx = t.position.x;
            ty = t.position.y;
        }
        cam.zoom_value(cam.settings.altitude_minimum);
        cam.tilt_value(35.0);
    }
    cam.set_position(
        tx,
        ty,
        side,
        |x, y| m.terrain.elevation_at(x, y),
        m.water.elevation,
    );
    scene.render(&Frame {
        width,
        height,
        camera: &cam,
        light: &m.light,
        water: &m.water,
        terrain_params: &m.terrain_params,
        time: 0.0,
        precise_time: None,
        instances: &pop.instances,
        shadows: !m.light.is_night,
    })
}

/// Puts two equally sized RGBA8 images next to each other with a 4 pixel white divider.
/// Returns the image and its width.
pub fn side_by_side(left: &[u8], right: &[u8], w: u32, h: u32) -> (Vec<u8>, u32) {
    const GAP: u32 = 4;
    let (w, h) = (w as usize, h as usize);
    assert_eq!(left.len(), w * h * 4);
    assert_eq!(right.len(), w * h * 4);
    let total = 2 * w + GAP as usize;
    let mut out = vec![255u8; total * h * 4];
    for y in 0..h {
        let row = y * total * 4;
        out[row..row + w * 4].copy_from_slice(&left[y * w * 4..(y + 1) * w * 4]);
        let r = row + (w + GAP as usize) * 4;
        out[r..r + w * 4].copy_from_slice(&right[y * w * 4..(y + 1) * w * 4]);
    }
    (out, total as u32)
}

/// Writes `left | right` as an indexed PNG (shared median-cut palette) so a 1920x1080
/// pair stays under 1 MB.
pub fn write_side_by_side_png(
    path: &Path,
    left: &[u8],
    right: &[u8],
    w: u32,
    h: u32,
    colors: usize,
) -> std::io::Result<()> {
    let (img, total) = side_by_side(left, right, w, h);
    write_indexed_png(path, &img, total, h, colors)
}

/// Indexed PNG, high deflate, at most `colors` (2..=256) entries.
pub fn write_indexed_png(
    path: &Path,
    rgba: &[u8],
    w: u32,
    h: u32,
    colors: usize,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let (palette, indices) = quantize(rgba, colors);
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut enc = png::Encoder::new(file, w, h);
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_palette(palette);
    enc.set_compression(png::Compression::High);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(&indices).map_err(std::io::Error::other)
}

/// Median-cut quantisation to at most `n` colours: `(RGB palette, one index per pixel)`.
fn quantize(rgba: &[u8], n: usize) -> (Vec<u8>, Vec<u8>) {
    let px: Vec<[u8; 3]> = rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
    let mut hist = std::collections::HashMap::<[u8; 3], u32>::new();
    for p in &px {
        *hist.entry(*p).or_default() += 1;
    }
    let mut boxes: Vec<Vec<([u8; 3], u32)>> = vec![hist.into_iter().collect()];
    while boxes.len() < n.clamp(2, 256) {
        let widest = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| {
                let range = (0..3)
                    .map(|c| {
                        let (lo, hi) = b.iter().fold((255u8, 0u8), |(lo, hi), (p, _)| {
                            (lo.min(p[c]), hi.max(p[c]))
                        });
                        (hi - lo, c)
                    })
                    .max()
                    .unwrap();
                (
                    range.0 as u64 * b.iter().map(|(_, k)| *k as u64).sum::<u64>(),
                    i,
                    range.1,
                )
            })
            .max();
        let Some((_, i, c)) = widest else { break };
        let mut b = boxes.swap_remove(i);
        b.sort_by_key(|(p, _)| p[c]);
        let half = b.iter().map(|(_, k)| *k as u64).sum::<u64>() / 2;
        let mut acc = 0u64;
        let cut = b
            .iter()
            .position(|(_, k)| {
                acc += *k as u64;
                acc >= half
            })
            .unwrap_or(0)
            .min(b.len() - 2)
            + 1;
        let hi = b.split_off(cut);
        boxes.push(b);
        boxes.push(hi);
    }
    let mut palette = Vec::new();
    let mut lookup = std::collections::HashMap::<[u8; 3], u8>::new();
    for (idx, b) in boxes.iter().enumerate() {
        let total: u64 = b.iter().map(|(_, k)| *k as u64).sum();
        let avg = |c: usize| {
            (b.iter().map(|(p, k)| p[c] as u64 * *k as u64).sum::<u64>() / total.max(1)) as u8
        };
        palette.extend([avg(0), avg(1), avg(2)]);
        for (p, _) in b {
            lookup.insert(*p, idx as u8);
        }
    }
    (palette, px.iter().map(|p| lookup[p]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn side_by_side_layout() {
        let a = vec![10u8; 2 * 2 * 4];
        let b = vec![20u8; 2 * 2 * 4];
        let (img, w) = side_by_side(&a, &b, 2, 2);
        assert_eq!(w, 8);
        let px = |x: usize, y: usize| img[(y * 8 + x) * 4];
        assert_eq!((px(0, 0), px(1, 1)), (10, 10));
        assert_eq!((px(2, 0), px(5, 1)), (255, 255));
        assert_eq!((px(6, 0), px(7, 1)), (20, 20));
    }
}
