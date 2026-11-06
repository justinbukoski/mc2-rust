//! Enumerates every texture the game can show, through the same code the renderer uses, and
//! files each one under the replacement hook's key.
//!
//! Terrain textures come from running the real loaders (`Mission::load` and
//! `TerrainTextures::load`) over every mission, so colour maps, detail and water bitmaps and the
//! composited road/transition tiles carry exactly the names and pixels `mc2-render` hands to the
//! hook.  Shape textures come from decoding every `.tgl` and resolving the texture list the way
//! `ShapeModel` does.  The rest is a sweep of the archives (interface art, cursors, insignia, the
//! baked `.txm` tiles) plus the effect library's texture names and the bitmap fonts on disk.

use crate::model::*;
use crate::stats;
use mc2_formats::fst::FastFile;
use mc2_gfx::texture as tx;
use mc2_render::game::GameData;
use mc2_render::mission::Mission;
use mc2_render::shape::{texture_has_alpha, texture_path, ShapeModel};
use mc2_render::terrain_tex::{decode_txm, TerrainTextures};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// Most `used_by` names kept per texture.
const USED_BY_CAP: usize = 6;
/// Most `aliases` kept per tile.
const ALIAS_CAP: usize = 8;

pub struct Options {
    /// Only these missions (default: every mission that has a `.fit` and a `.pak`).
    pub missions: Option<Vec<String>>,
    /// Write the original PNGs to `<dir>/originals/<key>.png`.
    pub originals: Option<PathBuf>,
    pub verbose: bool,
}

pub struct Collected {
    pub manifest: DumpManifest,
    pub warnings: Vec<String>,
    /// The warnings that mean a texture the data has was lost (unreadable, undecodable, not
    /// written), as opposed to references the data simply does not contain.  A pack built from
    /// a collection with failures is not certified complete.
    pub failures: Vec<String>,
    /// Texture files shapes name that the data does not contain (shadow textures such as
    /// `*x.tga`, `nulltxm`): the original game skips them too.
    pub missing_refs: Vec<String>,
}

struct Acc {
    failures: Vec<String>,
    entries: BTreeMap<String, Entry>,
    uses: BTreeMap<String, BTreeSet<String>>,
    originals: Option<PathBuf>,
    warnings: Vec<String>,
    missing_refs: Vec<String>,
    seen_names: HashSet<String>,
}

impl Acc {
    fn fail(&mut self, msg: String) {
        self.warnings.push(msg.clone());
        self.failures.push(msg);
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        name: &str,
        category: Category,
        w: u32,
        h: u32,
        rgba: Option<&[u8]>,
        alpha_used: bool,
        used_by: Option<&str>,
    ) {
        self.seen_names.insert(name.to_string());
        let key = tx::texture_key(Some(name), &[], w, h, mc2_gfx::MC2GFX_TEXFMT_RGBA8);
        let hash = rgba.map(stats::pixel_hash).unwrap_or_default();
        if let Some(by) = used_by {
            self.uses
                .entry(key.clone())
                .or_default()
                .insert(by.to_string());
        }
        if let Some(e) = self.entries.get_mut(&key) {
            if e.hash != hash && !hash.is_empty() && !e.hash.is_empty() {
                e.ambiguous = true;
                self.warnings
                    .push(format!("{key}: two different images behind one key"));
            }
            e.alpha_used |= alpha_used;
            return;
        }
        let (alpha, mask) = match rgba {
            Some(p) => (
                stats::alpha_class(p),
                // only shapes go through the paint scheme
                if category == Category::Shape {
                    stats::paint_mask_pixels(p)
                } else {
                    0
                },
            ),
            None => (AlphaClass::None, 0),
        };
        let mut file = String::new();
        if let (Some(dir), Some(px)) = (&self.originals, rgba) {
            let rel = format!("originals/{key}.png");
            let path = dir.join(&rel);
            match tx::write_png(&path, px, w, h, mc2_gfx::MC2GFX_TEXFMT_RGBA8) {
                Ok(()) => file = rel,
                Err(e) => self.fail(format!("could not write {}: {e}", path.display())),
            }
        }
        self.entries.insert(
            key.clone(),
            Entry {
                key,
                name: name.to_string(),
                category,
                width: w,
                height: h,
                alpha,
                alpha_used,
                paint_mask_pixels: mask,
                tileable: category.tileable(),
                hooked: category.hooked(),
                recommended_scale: recommended_scale(category, w, h),
                hash,
                ambiguous: false,
                aliases: Vec::new(),
                use_count: 0,
                used_by: Vec::new(),
                file,
            },
        );
    }
}

/// Dimensions of a baseline/progressive JPEG (the SOFn marker), without decoding it.
pub fn jpeg_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.get(0..2)? != [0xff, 0xd8] {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        if m == 0xff {
            i += 1;
            continue;
        }
        if (0xd0..=0xd9).contains(&m) || m == 0x01 {
            i += 2;
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if matches!(m, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let h = u16::from_be_bytes([*b.get(i + 5)?, *b.get(i + 6)?]) as u32;
            let w = u16::from_be_bytes([*b.get(i + 7)?, *b.get(i + 8)?]) as u32;
            return Some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// Archive entry names (lowercase) of every mounted archive, with the archive file name.
fn archive_entries(game: &GameData) -> Vec<String> {
    let mut out = Vec::new();
    for path in game.vfs.archive_paths() {
        match FastFile::open(path) {
            Ok(ff) => out.extend(ff.entries().iter().map(|e| e.name.to_ascii_lowercase())),
            Err(e) => eprintln!("warning: cannot list {}: {e}", path.display()),
        }
    }
    out
}

fn classify_path(name: &str) -> Category {
    let n = name;
    if n.starts_with("data/textures/64mask/") || n.starts_with("data/textures/64overlays/") {
        Category::TerrainInput
    } else if n.starts_with("data/textures/64/") {
        Category::TerrainTile
    } else if n.ends_with(".burnin.tga") {
        Category::ColorMap
    } else if n.ends_with(".detail.tga") || n.contains("default_detail") {
        Category::Detail
    } else if n.contains("/defaults/") && n.contains("water") && n.contains("detail") {
        Category::WaterDetail
    } else if n.ends_with(".water.tga") || n.contains("default_water") {
        Category::Water
    } else if is_water_frame(n) {
        Category::WaterDetail
    } else if n.contains("/insignia/") {
        Category::Insignia
    } else if n.starts_with("data/art/") && n.contains("cursor") {
        Category::Cursor
    } else if n.starts_with("data/art/") {
        Category::Ui
    } else if n.starts_with("data/tgl/") {
        Category::Shape
    } else {
        Category::Other
    }
}

/// `<base>.waterNNNN.tga`
fn is_water_frame(n: &str) -> bool {
    let Some(stem) = n.strip_suffix(".tga") else {
        return false;
    };
    match stem.rfind(".water") {
        Some(i) => {
            let digits = &stem[i + 6..];
            digits.len() == 4 && digits.bytes().all(|b| b.is_ascii_digit())
        }
        None => false,
    }
}

fn alpha_used_for(category: Category) -> bool {
    !matches!(category, Category::ColorMap | Category::Water)
}

pub fn collect(game: &mut GameData, opts: &Options) -> Collected {
    let mut acc = Acc {
        entries: BTreeMap::new(),
        uses: BTreeMap::new(),
        originals: opts.originals.clone(),
        warnings: Vec::new(),
        failures: Vec::new(),
        missing_refs: Vec::new(),
        seen_names: HashSet::new(),
    };
    let names = archive_entries(game);
    let say = |msg: &str| {
        if opts.verbose {
            eprintln!("{msg}");
        }
    };

    // ---- terrain, per mission ------------------------------------------------------------
    let mut missions: Vec<String> = match &opts.missions {
        Some(m) => m.clone(),
        None => {
            let set: BTreeSet<&str> = names.iter().map(String::as_str).collect();
            names
                .iter()
                .filter_map(|n| n.strip_prefix("data/missions/")?.strip_suffix(".fit"))
                .filter(|m| {
                    !m.contains('/') && set.contains(format!("data/missions/{m}.pak").as_str())
                })
                .map(str::to_string)
                .collect()
        }
    };
    missions.sort();
    missions.dedup();
    let mut loaded = Vec::new();
    for m in &missions {
        let res = Mission::load(game, m).and_then(|mission| {
            let t = TerrainTextures::load(game, &mission)?;
            Ok((mission, t))
        });
        match res {
            Ok((_mission, t)) => {
                say(&format!(
                    "[{m}] {} tile layers, colour map {}",
                    t.tile_layers.len(),
                    t.color_map_name
                ));
                loaded.push(m.clone());
                let by = format!("mission:{m}");
                let cm = &t.color_map;
                acc.add(
                    &t.color_map_name,
                    Category::ColorMap,
                    cm.width as u32,
                    cm.height as u32,
                    Some(&cm.pixels),
                    false,
                    Some(&by),
                );
                acc.add(
                    &t.detail_name,
                    Category::Detail,
                    t.detail.width as u32,
                    t.detail.height as u32,
                    Some(&t.detail.pixels),
                    true,
                    Some(&by),
                );
                acc.add(
                    &t.water_name,
                    Category::Water,
                    t.water.width as u32,
                    t.water.height as u32,
                    Some(&t.water.pixels),
                    false,
                    Some(&by),
                );
                for (n, im) in t.water_detail_names.iter().zip(&t.water_detail) {
                    acc.add(
                        n,
                        Category::WaterDetail,
                        im.width as u32,
                        im.height as u32,
                        Some(&im.pixels),
                        true,
                        Some(&by),
                    );
                }
                for (layer, px) in t.tile_layers.iter().enumerate() {
                    let ids = t.tile_names.get(layer).cloned().unwrap_or_default();
                    for n in &ids {
                        acc.add(n, Category::TerrainTile, 64, 64, Some(px), true, Some(&by));
                    }
                }
            }
            Err(e) => acc.fail(format!("mission {m}: terrain textures not loaded: {e}")),
        }
    }

    // ---- shapes ---------------------------------------------------------------------------
    // path -> (alpha flag union, users)
    let mut shape_tex: BTreeMap<String, (bool, BTreeSet<String>)> = BTreeMap::new();
    for n in names.iter().filter(|n| n.ends_with(".tgl")) {
        let Some(stem) = n
            .strip_prefix("data/tgl/")
            .and_then(|s| s.strip_suffix(".tgl"))
        else {
            continue;
        };
        match ShapeModel::load(game, stem) {
            Ok(model) => {
                for tn in &model.texture_names {
                    let e = shape_tex
                        .entry(texture_path(tn))
                        .or_insert((false, BTreeSet::new()));
                    e.0 |= texture_has_alpha(tn);
                    e.1.insert(format!("tgl:{stem}"));
                }
            }
            Err(e) => acc.fail(format!("{n}: {e}")),
        }
    }
    say(&format!("{} shape textures referenced", shape_tex.len()));
    for (path, (alpha, users)) in &shape_tex {
        match game.tga(path) {
            Ok(img) => {
                let (w, h) = (img.width as u32, img.height as u32);
                for u in users {
                    acc.add(
                        path,
                        Category::Shape,
                        w,
                        h,
                        Some(&img.pixels),
                        *alpha,
                        Some(u),
                    );
                }
            }
            Err(_) if !game.exists(path) => acc
                .missing_refs
                .push(format!("{path} (referenced by {} shapes)", users.len())),
            Err(e) => acc.fail(format!(
                "{path}: referenced by {} shapes but unreadable: {e}",
                users.len()
            )),
        }
    }

    // ---- effects ---------------------------------------------------------------------------
    match game.read(mc2_fx::library::LIBRARY_PATH) {
        Ok(bytes) => match mc2_fx::library::EffectLibrary::load(&bytes) {
            Ok(lib) => {
                let mut eff: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
                for spec in lib.specs() {
                    if let Some(t) = &spec.base.state.texture {
                        eff.entry(t.name.to_ascii_lowercase())
                            .or_default()
                            .insert(format!("fx:{}", spec.name()));
                    }
                }
                say(&format!("{} effect texture names", eff.len()));
                for (tn, users) in &eff {
                    match resolve_effect_texture(game, tn) {
                        Some(path) => match game.tga(&path) {
                            Ok(img) => {
                                for u in users {
                                    // an effect texture also used by a shape stays a shape's:
                                    // same key, the first category wins (shapes run first)
                                    acc.add(
                                        &path,
                                        Category::Effect,
                                        img.width as u32,
                                        img.height as u32,
                                        Some(&img.pixels),
                                        true,
                                        Some(u),
                                    );
                                }
                            }
                            Err(e) => acc.fail(format!("{path}: {e}")),
                        },
                        None => acc
                            .warnings
                            .push(format!("effect texture {tn:?} not found in the data")),
                    }
                }
            }
            Err(e) => acc.fail(format!("effect library: {e}")),
        },
        // optional data that is simply absent is a warning; an existing library that could
        // not be read lost its effect textures, so the collection is incomplete
        Err(_) if !game.exists(mc2_fx::library::LIBRARY_PATH) => acc
            .warnings
            .push("effect library: not in the data".to_owned()),
        Err(e) => acc.fail(format!("effect library: {e}")),
    }

    // ---- everything else in the archives -------------------------------------------------
    for n in &names {
        let is_tga = n.ends_with(".tga");
        if is_tga {
            let cat = classify_path(n);
            // already filed by a loader above?
            if acc.seen_names.contains(n) {
                continue;
            }
            match game.tga(n) {
                Ok(img) => {
                    let (w, h) = (img.width as u32, img.height as u32);
                    let alpha_used = if cat == Category::Shape {
                        false
                    } else {
                        alpha_used_for(cat)
                    };
                    // unreferenced shape-folder images are not known to be shown by anything
                    let cat = if cat == Category::Shape {
                        Category::Other
                    } else {
                        cat
                    };
                    acc.add(n, cat, w, h, Some(&img.pixels), alpha_used, None);
                }
                Err(e) => acc.fail(format!("{n}: {e}")),
            }
        } else if n.ends_with(".txm") && n.starts_with("data/textures/") {
            if acc.seen_names.contains(n) {
                continue;
            }
            match game.read(n).and_then(|b| decode_txm(&b)) {
                Ok(px) => acc.add(n, Category::TerrainTile, 64, 64, Some(&px), true, None),
                Err(e) => acc.fail(format!("{n}: {e}")),
            }
        } else if n.ends_with(".jpg") {
            match game.read(n) {
                Ok(b) => match jpeg_size(&b) {
                    Some((w, h)) => acc.add(n, Category::Other, w, h, None, false, None),
                    None => acc.fail(format!("{n}: not a readable JPEG")),
                },
                Err(e) => acc.fail(format!("{n}: {e}")),
            }
        }
    }

    // ---- bitmap fonts on disk ---------------------------------------------------------------
    let font_dir = game.vfs.root().join("assets/graphics");
    if let Ok(rd) = std::fs::read_dir(&font_dir) {
        let mut files: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("bmp")))
            .collect();
        files.sort();
        for p in files {
            let name = format!(
                "assets/graphics/{}",
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .to_ascii_lowercase()
            );
            match std::fs::read(&p)
                .map_err(|e| e.to_string())
                .and_then(|b| crate::bmp::decode(&b))
            {
                Ok(img) => acc.add(
                    &name,
                    Category::Font,
                    img.width,
                    img.height,
                    Some(&img.rgba),
                    true,
                    None,
                ),
                Err(e) => acc.fail(format!("{name}: {e}")),
            }
        }
    }

    finish(acc, loaded)
}

fn finish(mut acc: Acc, missions: Vec<String>) -> Collected {
    // use counts, and aliases (names that share the exact pixels of a tile)
    for (key, users) in &acc.uses {
        if let Some(e) = acc.entries.get_mut(key) {
            e.use_count = users.len() as u32;
            e.used_by = users.iter().take(USED_BY_CAP).cloned().collect();
        }
    }
    let mut by_hash: BTreeMap<(Category, String), Vec<String>> = BTreeMap::new();
    for e in acc.entries.values() {
        if e.category == Category::TerrainTile && !e.hash.is_empty() {
            by_hash
                .entry((e.category, e.hash.clone()))
                .or_default()
                .push(e.name.clone());
        }
    }
    for e in acc.entries.values_mut() {
        if e.category == Category::TerrainTile {
            if let Some(group) = by_hash.get(&(e.category, e.hash.clone())) {
                e.aliases = group
                    .iter()
                    .filter(|n| **n != e.name)
                    .take(ALIAS_CAP)
                    .cloned()
                    .collect();
            }
        }
    }
    let mut entries: Vec<Entry> = acc.entries.into_values().collect();
    entries.sort_by(|a, b| (a.category, &a.key).cmp(&(b.category, &b.key)));
    Collected {
        missing_refs: acc.missing_refs,
        manifest: DumpManifest {
            format: DUMP_FORMAT.to_string(),
            generator: format!("mc2-texpack {}", env!("CARGO_PKG_VERSION")),
            missions,
            entries,
        },
        warnings: acc.warnings,
        failures: acc.failures,
    }
}

/// Where an effect's texture file lives.  The texture manager names them by base name; shapes
/// and effects share the 128 pixel folder.
fn resolve_effect_texture(game: &GameData, base: &str) -> Option<String> {
    let base = base.rsplit(['\\', '/']).next().unwrap_or(base);
    let with_ext = if base.contains('.') {
        base.to_string()
    } else {
        format!("{base}.tga")
    };
    ["data/tgl/128/", "data/art/", "data/textures/"]
        .iter()
        .map(|d| format!("{d}{with_ext}"))
        .find(|p| game.exists(p))
}

/// Reads `<dir>/manifest.json`.
pub fn read_dump_manifest(path: &Path) -> Result<DumpManifest, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let m: DumpManifest =
        serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", path.display()))?;
    if m.format != DUMP_FORMAT {
        return Err(format!("{}: unknown format {:?}", path.display(), m.format));
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_follows_the_data_layout() {
        use Category::*;
        assert_eq!(classify_path("data/textures/mc2_01.burnin.tga"), ColorMap);
        assert_eq!(classify_path("data/textures/mc2_01.detail.tga"), Detail);
        assert_eq!(classify_path("data/textures/mc2_01.water.tga"), Water);
        assert_eq!(
            classify_path("data/textures/mc2_01.water0007.tga"),
            WaterDetail
        );
        assert_eq!(classify_path("data/textures/64/water.tga"), TerrainTile);
        assert_eq!(classify_path("data/textures/64mask/x.tga"), TerrainInput);
        assert_eq!(classify_path("data/multiplayer/insignia/a.tga"), Insignia);
        assert_eq!(classify_path("data/art/cursors1.tga"), Cursor);
        assert_eq!(classify_path("data/art/blip.tga"), Ui);
        assert_eq!(classify_path("data/tgl/128/a_tree.tga"), Shape);
        assert_eq!(classify_path("testtxm.tga"), Other);
    }

    #[test]
    fn jpeg_dimensions_come_from_the_frame_header() {
        // SOI, APP0 (len 4), SOF0 len 11: precision, height 0x0102, width 0x0304
        let b = [
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0x00, 0x00, 0xff, 0xc0, 0x00, 0x0b, 0x08, 0x01,
            0x02, 0x03, 0x04, 0x01, 0x01, 0x11, 0x00,
        ];
        assert_eq!(jpeg_size(&b), Some((0x0304, 0x0102)));
        assert_eq!(jpeg_size(b"nope"), None);
    }
}
