//! Inventory summary (counts, sizes, alpha usage) written next to the manifest.

use crate::model::*;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Default, Clone, Debug, PartialEq)]
pub struct CategoryStats {
    pub count: usize,
    pub bytes: u64,
    pub alpha_none: usize,
    pub alpha_keyed: usize,
    pub alpha_smooth: usize,
    pub alpha_zero: usize,
    pub paint_mask: usize,
    pub min_side: u32,
    pub max_side: u32,
    pub sizes: BTreeMap<(u32, u32), usize>,
}

pub fn stats_by_category(entries: &[Entry]) -> BTreeMap<Category, CategoryStats> {
    let mut out: BTreeMap<Category, CategoryStats> = BTreeMap::new();
    for e in entries {
        let s = out.entry(e.category).or_insert_with(|| CategoryStats {
            min_side: u32::MAX,
            ..Default::default()
        });
        s.count += 1;
        s.bytes += e.width as u64 * e.height as u64 * 4;
        match e.alpha {
            AlphaClass::None => s.alpha_none += 1,
            AlphaClass::Keyed => s.alpha_keyed += 1,
            AlphaClass::Smooth => s.alpha_smooth += 1,
            AlphaClass::Zero => s.alpha_zero += 1,
        }
        if e.paint_mask_pixels > 0 {
            s.paint_mask += 1;
        }
        s.min_side = s.min_side.min(e.width.min(e.height));
        s.max_side = s.max_side.max(e.width.max(e.height));
        *s.sizes.entry((e.width, e.height)).or_default() += 1;
    }
    out
}

fn mib(b: u64) -> String {
    format!("{:.1}", b as f64 / (1024.0 * 1024.0))
}

/// Size of the output at the recommended scales, in RGBA bytes.
pub fn planned_bytes(entries: &[Entry]) -> u64 {
    entries
        .iter()
        .filter(|e| e.recommended_scale > 1)
        .map(|e| {
            let s = e.recommended_scale as u64;
            e.width as u64 * s * e.height as u64 * s * 4
        })
        .sum()
}

pub fn markdown(m: &DumpManifest, warnings: &[String], missing_refs: &[String]) -> String {
    let st = stats_by_category(&m.entries);
    let mut s = String::new();
    let _ = writeln!(s, "# Texture inventory\n");
    let _ = writeln!(
        s,
        "{} textures from {} missions ({}). Keys are the replacement hook's `n_<path>_<w>x<h>`.\n",
        m.entries.len(),
        m.missions.len(),
        m.generator
    );
    let _ = writeln!(
        s,
        "| category | count | RGBA MiB | smallest side | largest side | alpha none/keyed/smooth/zero | paint-mask | hooked today |"
    );
    let _ = writeln!(s, "|---|---:|---:|---:|---:|---|---:|---|");
    let mut total = 0u64;
    for (cat, c) in &st {
        total += c.bytes;
        let _ = writeln!(
            s,
            "| {} | {} | {} | {} | {} | {}/{}/{}/{} | {} | {} |",
            cat.as_str(),
            c.count,
            mib(c.bytes),
            c.min_side,
            c.max_side,
            c.alpha_none,
            c.alpha_keyed,
            c.alpha_smooth,
            c.alpha_zero,
            c.paint_mask,
            if cat.hooked() { "yes" } else { "no" }
        );
    }
    let _ = writeln!(
        s,
        "\nTotal {} MiB of originals; the recommended scales would produce {} MiB of RGBA before PNG compression.\n",
        mib(total),
        mib(planned_bytes(&m.entries))
    );
    let _ = writeln!(s, "## Sizes per category\n");
    for (cat, c) in &st {
        let sizes: Vec<String> = c
            .sizes
            .iter()
            .map(|((w, h), n)| format!("{w}x{h} x{n}"))
            .collect();
        let shown = if sizes.len() > 14 {
            format!("{} ... ({} distinct)", sizes[..14].join(", "), sizes.len())
        } else {
            sizes.join(", ")
        };
        let _ = writeln!(s, "- **{}**: {}", cat.as_str(), shown);
    }
    if !missing_refs.is_empty() {
        let _ = writeln!(
            s,
            "\nShapes name {} texture files the data does not contain (shadow textures, `nulltxm`); the game skips them too, so they are not in the dump. First few: {}.",
            missing_refs.len(),
            missing_refs.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        );
    }
    let ambiguous = m.entries.iter().filter(|e| e.ambiguous).count();
    let _ = writeln!(s, "\nAmbiguous keys: {ambiguous}.");
    if !warnings.is_empty() {
        let _ = writeln!(s, "\n## Warnings ({})\n", warnings.len());
        for w in warnings.iter().take(60) {
            let _ = writeln!(s, "- {w}");
        }
        if warnings.len() > 60 {
            let _ = writeln!(s, "- ... {} more in warnings.txt", warnings.len() - 60);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(cat: Category, w: u32, h: u32, alpha: AlphaClass) -> Entry {
        Entry {
            key: format!("k{w}x{h}"),
            name: "n".into(),
            category: cat,
            width: w,
            height: h,
            alpha,
            alpha_used: true,
            paint_mask_pixels: 0,
            tileable: false,
            hooked: true,
            recommended_scale: recommended_scale(cat, w, h),
            hash: String::new(),
            ambiguous: false,
            aliases: vec![],
            use_count: 0,
            used_by: vec![],
            file: String::new(),
        }
    }

    #[test]
    fn counts_and_scales() {
        let es = vec![
            entry(Category::Shape, 128, 128, AlphaClass::None),
            entry(Category::Shape, 128, 128, AlphaClass::Keyed),
            entry(Category::ColorMap, 4096, 4096, AlphaClass::None),
            entry(Category::Font, 256, 128, AlphaClass::Smooth),
        ];
        let st = stats_by_category(&es);
        assert_eq!(st[&Category::Shape].count, 2);
        assert_eq!(st[&Category::Shape].alpha_keyed, 1);
        assert_eq!(st[&Category::Shape].sizes[&(128, 128)], 2);
        // 4096 at x4 would be 16384: capped to x2
        assert_eq!(es[2].recommended_scale, 2);
        assert_eq!(es[3].recommended_scale, 1);
        assert_eq!(planned_bytes(&es), 2 * 512 * 512 * 4 + 8192 * 8192 * 4);
        assert!(markdown(
            &DumpManifest {
                format: DUMP_FORMAT.into(),
                generator: "t".into(),
                missions: vec![],
                entries: es
            },
            &[],
            &[]
        )
        .contains("| shape | 2 |"));
    }
}
