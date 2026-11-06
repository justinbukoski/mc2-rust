//! Checks a pack folder against the dump manifest it was made from.
//!
//! The game never depends on a pack (a missing, short or broken pack only means original
//! pixels), so verification is a quality gate for the pipeline, not a runtime check.

use crate::model::*;
use mc2_gfx::texture as tx;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Default, Debug)]
pub struct Report {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Per category: (replacements present, textures the dump recommends upscaling).
    pub coverage: BTreeMap<Category, (usize, usize)>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn read_pack_manifest(dir: &Path) -> Result<PackManifest, String> {
    let p = dir.join("manifest.json");
    let b = std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let m: PackManifest =
        serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", p.display()))?;
    if m.format != PACK_FORMAT {
        return Err(format!("{}: unknown format {:?}", p.display(), m.format));
    }
    Ok(m)
}

pub fn verify(dump: &DumpManifest, pack_dir: &Path) -> Report {
    let mut r = Report::default();
    let by_key: BTreeMap<&str, &Entry> = dump.entries.iter().map(|e| (e.key.as_str(), e)).collect();

    // the dump's own keys must be the hook's keys
    for e in &dump.entries {
        let k = tx::texture_key(Some(&e.name), &[], e.width, e.height, 0);
        if k != e.key {
            r.errors.push(format!(
                "dump entry {:?}: key {} does not match the hook's {}",
                e.name, e.key, k
            ));
        }
    }

    let pack = match read_pack_manifest(pack_dir) {
        Ok(p) => p,
        Err(e) => {
            r.errors.push(e);
            return r;
        }
    };
    let mut present: BTreeSet<&str> = BTreeSet::new();
    for pe in &pack.entries {
        let Some(de) = by_key.get(pe.key.as_str()) else {
            r.warnings
                .push(format!("{}: not in the dump (never requested)", pe.key));
            continue;
        };
        if !present.insert(pe.key.as_str()) {
            r.errors.push(format!("{}: listed twice", pe.key));
            continue;
        }
        if pe.source_hash != de.hash {
            r.warnings.push(format!(
                "{}: made from a different original (stale?)",
                pe.key
            ));
        }
        if pe.original != [de.width, de.height] {
            r.errors.push(format!(
                "{}: manifest original {:?} but the dump says {}x{}",
                pe.key, pe.original, de.width, de.height
            ));
        }
        let Some(rep) = tx::load_replacement(pack_dir, &pe.key) else {
            r.errors
                .push(format!("{}: missing or unreadable PNG", pe.key));
            continue;
        };
        if [rep.w, rep.h] != pe.size {
            r.errors.push(format!(
                "{}: PNG is {}x{} but the manifest says {:?}",
                pe.key, rep.w, rep.h, pe.size
            ));
        }
        if rep.w > MAX_DIMENSION || rep.h > MAX_DIMENSION {
            r.errors.push(format!(
                "{}: {}x{} is over the {MAX_DIMENSION} limit the renderer accepts",
                pe.key, rep.w, rep.h
            ));
        }
        // UVs are normalised, so only the aspect ratio must survive
        if (rep.w as u64) * (de.height as u64) != (rep.h as u64) * (de.width as u64) {
            r.errors.push(format!(
                "{}: aspect ratio changed ({}x{} -> {}x{})",
                pe.key, de.width, de.height, rep.w, rep.h
            ));
        }
        if de.category == Category::TerrainTile && rep.w != rep.h {
            r.errors.push(format!(
                "{}: terrain tile replacement must be square",
                pe.key
            ));
        }
        // an opaque original must not gain holes (the engine would draw them)
        if de.alpha == AlphaClass::None
            && de.alpha_used
            && rep.rgba.chunks_exact(4).any(|p| p[3] != 255)
        {
            r.errors.push(format!(
                "{}: original is opaque but the replacement has transparent pixels",
                pe.key
            ));
        }
        if de.paint_mask_pixels > 0 {
            let n = crate::stats::paint_mask_pixels(&rep.rgba);
            if n == 0 {
                r.warnings.push(format!(
                    "{}: the original has paint-mask pixels, the replacement none (paint schemes would not show)",
                    pe.key
                ));
            }
        }
    }
    // stray files the manifest does not know
    if let Ok(rd) = std::fs::read_dir(pack_dir) {
        for f in rd.filter_map(|e| e.ok()) {
            let n = f.file_name().to_string_lossy().into_owned();
            if let Some(key) = n.strip_suffix(".png") {
                if !present.contains(key) {
                    r.warnings
                        .push(format!("{n}: file not listed in the pack manifest"));
                }
            }
        }
    }
    for e in dump.entries.iter().filter(|e| e.recommended_scale > 1) {
        let c = r.coverage.entry(e.category).or_default();
        c.1 += 1;
        if present.contains(e.key.as_str()) {
            c.0 += 1;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mc2_texpack_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn dump_entry(name: &str, w: u32, h: u32, cat: Category) -> Entry {
        Entry {
            key: tx::texture_key(Some(name), &[], w, h, 0),
            name: name.into(),
            category: cat,
            width: w,
            height: h,
            alpha: AlphaClass::None,
            alpha_used: true,
            paint_mask_pixels: 0,
            tileable: false,
            hooked: true,
            recommended_scale: 4,
            hash: "aa".into(),
            ambiguous: false,
            aliases: vec![],
            use_count: 1,
            used_by: vec![],
            file: String::new(),
        }
    }

    fn write_pack(dir: &Path, e: &Entry, w: u32, h: u32, alpha: u8) {
        let px: Vec<u8> = (0..w * h).flat_map(|_| [10, 20, 30, alpha]).collect();
        tx::write_png(&tx::replacement_path(dir, &e.key), &px, w, h, 0).unwrap();
        let m = PackManifest {
            format: PACK_FORMAT.into(),
            generator: "test".into(),
            created: "now".into(),
            models: vec![],
            entries: vec![PackEntry {
                key: e.key.clone(),
                name: e.name.clone(),
                category: e.category,
                original: [e.width, e.height],
                size: [w, h],
                scale: w / e.width,
                model: "m".into(),
                steps: vec![],
                source_hash: "aa".into(),
            }],
        };
        std::fs::write(dir.join("manifest.json"), serde_json::to_vec(&m).unwrap()).unwrap();
    }

    fn dump_of(e: Entry) -> DumpManifest {
        DumpManifest {
            format: DUMP_FORMAT.into(),
            generator: "t".into(),
            missions: vec![],
            entries: vec![e],
        }
    }

    #[test]
    fn a_good_pack_verifies_and_the_hook_loads_it() {
        let dir = tmp("good");
        let e = dump_entry("data/tgl/128/a.tga", 2, 2, Category::Shape);
        write_pack(&dir, &e, 8, 8, 255);
        let r = verify(&dump_of(e.clone()), &dir);
        assert!(r.ok(), "{:?}", r.errors);
        assert_eq!(r.coverage[&Category::Shape], (1, 1));
        // and the renderer's own lookup finds the same file
        let cfg = mc2_gfx::config::RenderConfig {
            hires_dir: Some(dir.clone()),
            ..Default::default()
        };
        let (_, w, h) = mc2_render::gpu::apply_replacement(&cfg, &e.name, 2, 2, &[0; 16]);
        assert_eq!((w, h), (8, 8));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_aspect_hole_and_non_square_tile_are_errors() {
        let dir = tmp("bad");
        let e = dump_entry("a.tga", 2, 2, Category::Shape);
        write_pack(&dir, &e, 8, 4, 255);
        assert!(verify(&dump_of(e), &dir)
            .errors
            .iter()
            .any(|m| m.contains("aspect")));
        let e = dump_entry("b.tga", 2, 2, Category::Shape);
        write_pack(&dir, &e, 8, 8, 0);
        assert!(verify(&dump_of(e), &dir)
            .errors
            .iter()
            .any(|m| m.contains("transparent")));
        let e = dump_entry("c.tga", 2, 1, Category::TerrainTile);
        write_pack(&dir, &e, 8, 4, 255);
        assert!(verify(&dump_of(e), &dir)
            .errors
            .iter()
            .any(|m| m.contains("square")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_pack_is_an_error_not_a_panic() {
        let e = dump_entry("a.tga", 2, 2, Category::Shape);
        let r = verify(&dump_of(e), Path::new("/nonexistent/pack"));
        assert!(!r.ok());
    }
}
