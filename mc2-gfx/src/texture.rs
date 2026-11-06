// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! CPU-side texture helpers: pixel format expansion, mip chains, stable content keys
//! and the optional high-resolution replacement lookup.
//!
//! Provenance: written from the gos_NewTextureFromFile / gos_NewTextureFromMemory /
//! gos_LockTexture / gos_UpdateTexture contracts in `GameOS/include/gameos.hpp`.
//! The replacement-key scheme is original to this project (see PORTING-LOG.md).

use crate::abi::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// Bytes per pixel of the *stored* (GPU) representation.
pub fn stored_bytes_per_pixel(fmt: u32) -> usize {
    if fmt == MC2GFX_TEXFMT_R8 {
        1
    } else {
        4
    }
}

/// Bytes per pixel of caller-supplied data.
pub fn input_bytes_per_pixel(fmt: u32) -> usize {
    match fmt {
        MC2GFX_TEXFMT_R8 => 1,
        MC2GFX_TEXFMT_RGB8 => 3,
        _ => 4,
    }
}

/// Converts caller data to the stored representation (RGB8 gains an opaque alpha).
pub fn to_stored(fmt: u32, w: u32, h: u32, data: &[u8]) -> Option<Vec<u8>> {
    let n = (w as usize) * (h as usize);
    if data.len() < n * input_bytes_per_pixel(fmt) {
        return None;
    }
    Some(match fmt {
        MC2GFX_TEXFMT_RGB8 => {
            let mut out = Vec::with_capacity(n * 4);
            for px in data[..n * 3].chunks_exact(3) {
                out.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            out
        }
        MC2GFX_TEXFMT_R8 => data[..n].to_vec(),
        _ => data[..n * 4].to_vec(),
    })
}

pub fn mip_level_count(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

/// One box-filtered step down.  Odd edges clamp (the last row/column is reused).
pub fn downsample(src: &[u8], w: u32, h: u32, bpp: usize) -> (Vec<u8>, u32, u32) {
    let nw = (w / 2).max(1);
    let nh = (h / 2).max(1);
    let mut out = vec![0u8; (nw * nh) as usize * bpp];
    for y in 0..nh {
        for x in 0..nw {
            let x0 = (x * 2).min(w - 1);
            let x1 = (x * 2 + 1).min(w - 1);
            let y0 = (y * 2).min(h - 1);
            let y1 = (y * 2 + 1).min(h - 1);
            for c in 0..bpp {
                let at = |xx: u32, yy: u32| src[((yy * w + xx) as usize) * bpp + c] as u32;
                let sum = at(x0, y0) + at(x1, y0) + at(x0, y1) + at(x1, y1);
                out[((y * nw + x) as usize) * bpp + c] = ((sum + 2) / 4) as u8;
            }
        }
    }
    (out, nw, nh)
}

/// Full mip chain, level 0 first.  Each entry is (pixels, width, height).
pub fn build_mip_chain(base: Vec<u8>, w: u32, h: u32, bpp: usize) -> Vec<(Vec<u8>, u32, u32)> {
    let levels = mip_level_count(w, h);
    let mut chain = Vec::with_capacity(levels as usize);
    chain.push((base, w, h));
    for _ in 1..levels {
        let (p, pw, ph) = {
            let last = chain.last().unwrap();
            downsample(&last.0, last.1, last.2, bpp)
        };
        chain.push((p, pw, ph));
    }
    chain
}

fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn sanitize(name: &str) -> String {
    let mut s: String = name
        .replace('\\', "/")
        .trim_start_matches("./")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.truncate(120);
    s
}

/// Stable identifier for a texture, used to name replacement files.
///
/// * named sources (file path known): `n_<sanitised path>_<w>x<h>`
/// * otherwise: `h_<64-bit hash of the original pixels>_<w>x<h>_<fmt>`
///
/// The size is part of both so that a replacement can never be mistaken for a
/// differently-sized original.
pub fn texture_key(name: Option<&str>, stored_pixels: &[u8], w: u32, h: u32, fmt: u32) -> String {
    match name {
        Some(n) if !n.is_empty() => format!("n_{}_{}x{}", sanitize(n), w, h),
        _ => format!("h_{:016x}_{}x{}_{}", fnv1a64(stored_pixels), w, h, fmt),
    }
}

/// A loaded replacement image (always RGBA8).
#[derive(Debug)]
pub struct Replacement {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// Production texture class the replacement hook reports. The four P5 classes a pack is
/// required to cover are [`TextureClass::ColorMap`], [`TextureClass::Detail`],
/// [`TextureClass::Object`] and [`TextureClass::Ui`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureClass {
    /// Mission colour map (`*.burnin.tga`).
    ColorMap,
    /// Detail, water, water-detail and terrain-tile bitmaps.
    Detail,
    /// Object / mech / effect textures (`data/tgl/...`).
    Object,
    /// Interface art, cursors and insignia (`data/art/...`).
    Ui,
    /// Anything else (fonts, compositor masks). Not a P5 production class.
    Other,
}

impl TextureClass {
    /// Classifies a VFS path (the name the hook is given).
    pub fn from_name(name: &str) -> TextureClass {
        let n = name.replace('\\', "/").to_ascii_lowercase();
        if n.contains(".burnin.") {
            TextureClass::ColorMap
        } else if n.contains(".detail.")
            || n.ends_with("detail.tga")
            || n.contains(".water.")
            || n.contains(".water0")
            || n.ends_with("water.tga")
            || n.contains("/textures/64/")
            || n.ends_with(".txm")
        {
            TextureClass::Detail
        } else if n.contains("/tgl/") {
            TextureClass::Object
        } else if n.contains("/art/") {
            TextureClass::Ui
        } else {
            TextureClass::Other
        }
    }
}

/// Result of looking up one replacement file.
#[derive(Debug)]
pub enum ReplacementLookup {
    /// No file for this key (the original texture is used). Logged when the pack directory exists.
    Missing,
    Loaded(Replacement),
    /// A pack entry was present but unusable. Callers keep the original pixels.
    Failed {
        path: PathBuf,
        reason: String,
    },
}

/// Reason string for a configured pack that has no `<key>.png` (and no readable ktx2).
pub const MISSING_PACK_ENTRY: &str = "missing pack entry";

pub fn replacement_path(dir: &Path, key: &str) -> PathBuf {
    dir.join(format!("{key}.png"))
}

thread_local! {
    static FALLBACK_LOG: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

fn emit_fallback(msg: String) {
    log::warn!("{msg}");
    eprintln!("{msg}");
    FALLBACK_LOG.with(|slot| {
        if let Some(buf) = slot.borrow_mut().as_mut() {
            buf.push(msg);
        }
    });
}

fn log_fallback(path: &Path, reason: &str) {
    emit_fallback(format!(
        "[mc2gfx] hi-res replacement {} unreadable ({reason}): using the original",
        path.display()
    ));
}

fn log_missing(path: &Path) {
    emit_fallback(format!(
        "[mc2gfx] hi-res replacement {} missing: using the original",
        path.display()
    ));
}

/// Runs `f` and returns its result plus hi-res fallback log lines emitted on this thread.
pub fn with_fallback_log<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    FALLBACK_LOG.with(|slot| {
        *slot.borrow_mut() = Some(Vec::new());
    });
    let result = f();
    let logs = FALLBACK_LOG.with(|slot| slot.borrow_mut().take().unwrap_or_default());
    (result, logs)
}

fn decode_png(path: &Path) -> Result<Replacement, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![
        0u8;
        reader.output_buffer_size().ok_or_else(|| {
            "png output is larger than the decoder accepts".to_string()
        })?
    ];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    if w == 0 || h == 0 {
        return Err(format!("empty image {w}x{h}"));
    }
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        other => return Err(format!("unsupported png colour type {other:?}")),
    };
    let expect = (w as usize).saturating_mul(h as usize).saturating_mul(4);
    if rgba.len() != expect {
        return Err(format!(
            "decoded {} bytes, expected {expect} for {w}x{h}",
            rgba.len()
        ));
    }
    Ok(Replacement { rgba, w, h })
}

/// Looks up `<dir>/<key>.png`. A missing file is [`ReplacementLookup::Missing`] and, when the
/// pack directory exists, logs a fallback line; a present but corrupt or unsupported file is
/// logged and [`ReplacementLookup::Failed`].
pub fn try_load_replacement(dir: &Path, key: &str) -> ReplacementLookup {
    let p = replacement_path(dir, key);
    if p.exists() && !p.is_file() {
        let reason = "not a regular file".to_string();
        log_fallback(&p, &reason);
        return ReplacementLookup::Failed { path: p, reason };
    }
    if !p.is_file() {
        let ktx = dir.join(format!("{key}.ktx2"));
        if ktx.is_file() {
            let reason = "ktx2 decoding is not implemented yet".to_string();
            log_fallback(&ktx, &reason);
            return ReplacementLookup::Failed { path: ktx, reason };
        }
        if dir.is_dir() {
            log_missing(&p);
        }
        return ReplacementLookup::Missing;
    }
    match decode_png(&p) {
        Ok(r) => {
            log::info!(
                "mc2gfx: using replacement {} ({}x{})",
                p.display(),
                r.w,
                r.h
            );
            ReplacementLookup::Loaded(r)
        }
        Err(reason) => {
            log_fallback(&p, &reason);
            ReplacementLookup::Failed { path: p, reason }
        }
    }
}

/// Loads `<dir>/<key>.png` if present and valid.  `.ktx2` is recognised but not yet decoded.
/// A missing or corrupt entry returns `None` (the caller keeps the original pixels).
pub fn load_replacement(dir: &Path, key: &str) -> Option<Replacement> {
    match try_load_replacement(dir, key) {
        ReplacementLookup::Loaded(r) => Some(r),
        ReplacementLookup::Missing | ReplacementLookup::Failed { .. } => None,
    }
}

/// Writes an RGBA8 or R8 image as PNG.
pub fn write_png(path: &Path, pixels: &[u8], w: u32, h: u32, fmt: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(if fmt == MC2GFX_TEXFMT_R8 {
        png::ColorType::Grayscale
    } else {
        png::ColorType::Rgba
    });
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(pixels).map_err(std::io::Error::other)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_expands_with_opaque_alpha() {
        let out = to_stored(MC2GFX_TEXFMT_RGB8, 2, 1, &[1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(out, vec![1, 2, 3, 255, 4, 5, 6, 255]);
        assert!(to_stored(MC2GFX_TEXFMT_RGB8, 2, 1, &[1, 2, 3]).is_none());
    }

    #[test]
    fn mip_counts() {
        assert_eq!(mip_level_count(1, 1), 1);
        assert_eq!(mip_level_count(256, 256), 9);
        assert_eq!(mip_level_count(256, 64), 9);
        assert_eq!(mip_level_count(5, 3), 3);
    }

    #[test]
    fn box_filter_averages() {
        let src = [0u8, 100, 200, 100]; // 2x2 single channel
        let (o, w, h) = downsample(&src, 2, 2, 1);
        assert_eq!((w, h, o[0]), (1, 1, 100));
    }

    #[test]
    fn chain_reaches_one_by_one() {
        let chain = build_mip_chain(vec![255; 8 * 4 * 4], 8, 4, 4);
        let dims: Vec<_> = chain.iter().map(|c| (c.1, c.2)).collect();
        assert_eq!(dims, vec![(8, 4), (4, 2), (2, 1), (1, 1)]);
    }

    #[test]
    fn keys_are_stable_and_distinguish() {
        let a = texture_key(None, &[1, 2, 3, 4], 1, 1, 0);
        assert_eq!(a, texture_key(None, &[1, 2, 3, 4], 1, 1, 0));
        assert_ne!(a, texture_key(None, &[1, 2, 3, 5], 1, 1, 0));
        assert_ne!(a, texture_key(None, &[1, 2, 3, 4], 1, 1, 2));
        let n = texture_key(Some("data\\textures/Mech 1.tga"), &[], 64, 32, 0);
        assert_eq!(n, "n_data_textures_Mech_1.tga_64x32");
    }

    #[test]
    fn replacement_round_trip() {
        let dir = std::env::temp_dir().join(format!("mc2gfx_test_{}", std::process::id()));
        let key = "h_test_2x1_0";
        write_png(
            &replacement_path(&dir, key),
            &[10, 20, 30, 40, 50, 60, 70, 80],
            2,
            1,
            0,
        )
        .unwrap();
        let r = load_replacement(&dir, key).unwrap();
        assert_eq!((r.w, r.h), (2, 1));
        assert_eq!(r.rgba, vec![10, 20, 30, 40, 50, 60, 70, 80]);
        assert!(load_replacement(&dir, "missing").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(tag: &str) -> PathBuf {
        let root = option_env!("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        let d = root.join(format!("mc2gfx_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn classifies_the_four_production_texture_classes() {
        assert_eq!(
            TextureClass::from_name("data/textures/mc2_01.burnin.tga"),
            TextureClass::ColorMap
        );
        assert_eq!(
            TextureClass::from_name("data/textures/mc2_01.detail.tga"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/textures/mc2_01.water.tga"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/textures/defaults/water.tga"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/textures/defaults/detail.tga"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/textures/mc2_01.water0003.tga"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/textures/64/grass.tga"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/textures/1.2.txm"),
            TextureClass::Detail
        );
        assert_eq!(
            TextureClass::from_name("data/tgl/128/a_tree1.tga"),
            TextureClass::Object
        );
        assert_eq!(
            TextureClass::from_name("data/art/mcl_mm_1.tga"),
            TextureClass::Ui
        );
        assert_eq!(
            TextureClass::from_name("assets/graphics/font.bmp"),
            TextureClass::Other
        );
    }

    #[test]
    fn corrupt_replacement_falls_back_to_none() {
        let dir = scratch("corrupt");
        let key = "n_data_art_broken.tga_2x2";
        std::fs::write(replacement_path(&dir, key), b"not a png").unwrap();
        match try_load_replacement(&dir, key) {
            ReplacementLookup::Failed { reason, .. } => {
                assert!(!reason.is_empty(), "{reason}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(load_replacement(&dir, key).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_pack_entry_logs_and_keeps_none() {
        let dir = scratch("missing");
        let key = "n_data_tgl_128_a.tga_2x1";
        let path = replacement_path(&dir, key);
        let (lookup, logs) = with_fallback_log(|| try_load_replacement(&dir, key));
        assert!(matches!(lookup, ReplacementLookup::Missing));
        assert!(load_replacement(&dir, key).is_none());
        assert!(
            logs.iter()
                .any(|l| l.contains("missing") && l.contains(&path.display().to_string())),
            "expected a missing-entry log for {}, got {logs:?}",
            path.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_pack_directory_is_silent_per_key() {
        let dir = scratch("missing_dir");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!dir.exists());
        let (lookup, logs) = with_fallback_log(|| try_load_replacement(&dir, "n_x_1x1"));
        assert!(matches!(lookup, ReplacementLookup::Missing));
        assert!(
            logs.is_empty(),
            "no pack directory: per-key missing logs must stay off, got {logs:?}"
        );
    }
}
