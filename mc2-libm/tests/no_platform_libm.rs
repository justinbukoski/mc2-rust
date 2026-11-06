//! The simulation crates must not call the platform's transcendental functions (they differ between targets in the last
//! bit; see the crate docs): everything goes through `mc2_libm`. This test scans their sources.

use std::path::{Path, PathBuf};

const FORBIDDEN: &[&str] = &[
    ".sin(",
    ".cos(",
    ".tan(",
    ".asin(",
    ".acos(",
    ".atan(",
    ".atan2(",
    ".sinh(",
    ".cosh(",
    ".tanh(",
    ".sin_cos(",
    ".exp(",
    ".exp2(",
    ".exp_m1(",
    ".ln(",
    ".ln_1p(",
    ".log(",
    ".log2(",
    ".log10(",
    ".powf(",
    ".hypot(",
    ".cbrt(",
];

/// UFCS forms (`f32::sin(x)`, `f64::powf(..)`) of the same functions.
const FORBIDDEN_UFCS: &[&str] = &[
    "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "sinh", "cosh", "tanh", "sin_cos", "exp",
    "exp2", "exp_m1", "ln", "ln_1p", "log", "log2", "log10", "powf", "hypot", "cbrt",
];

/// Crates that take part in the deterministic simulation (and the formats/loaders they feed from).
const CRATES: &[&str] = &[
    "mc2-sim",
    "mc2-sim-data",
    "mc2-assets",
    "mc2-path",
    "mc2-abl",
    "mc2-campaign",
    "mc2-formats",
];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn simulation_crates_use_no_platform_transcendentals() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut hits = Vec::new();
    for krate in CRATES {
        let mut files = Vec::new();
        sources(&root.join(krate).join("src"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                if let Some(f) = FORBIDDEN.iter().find(|f| code.contains(**f)) {
                    hits.push(format!("{}:{}: {f}", file.display(), n + 1));
                }
                for ty in ["f32", "f64"] {
                    for name in FORBIDDEN_UFCS {
                        let call = format!("{ty}::{name}(");
                        if code.contains(&call) {
                            hits.push(format!("{}:{}: {call}", file.display(), n + 1));
                        }
                    }
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "use mc2_libm instead of the platform libm:\n{}",
        hits.join("\n")
    );
}
