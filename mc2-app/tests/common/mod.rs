pub mod link_helper;

/// Return true only for a missing GPU adapter when the explicit test opt-out is enabled.
/// Other setup errors and an unset/false opt-out stay test failures.
#[allow(dead_code)]
pub fn skip_gpu_test_without_adapter(error: &str) -> bool {
    if error.contains("adapter")
        && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref() == Some(std::ffi::OsStr::new("1"))
    {
        eprintln!("no GPU: {error}; MC2_SKIP_GPU_TESTS=1, skipping");
        true
    } else {
        false
    }
}

/// Builds a synthetic mission script from a checked-in `.tail` (our own program) plus the declaration section of a shipped
/// mission's `.abl` read from the player's data (`data` is the `MC2_DATA` root), so no game script text is stored in the tree.
/// Tail header: `//@ base=<mission> premodule=<0|1> keep=<n> module=<name>`. Twin of `synth_abl` in `tools/oracle-tools/src/scenarios.rs`; keep them equal.
#[allow(dead_code)]
pub fn synth_abl(data: &std::path::Path, tail_text: &str) -> String {
    let (header, tail) = tail_text.split_once('\n').expect("tail header");
    let header = header.strip_prefix("//@ ").expect("tail header marker");
    let field = |key: &str| {
        header
            .split_whitespace()
            .find_map(|p| p.strip_prefix(key).and_then(|v| v.strip_prefix('=')))
            .unwrap_or_else(|| panic!("tail header lacks {key}"))
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(data).unwrap();
    let base = vfs
        .read(&format!("data/missions/{}.abl", field("base")))
        .unwrap()
        .expect("base mission script");
    let base: Vec<String> = base
        .iter()
        .map(|&b| b as char)
        .collect::<String>()
        .replace("\r\n", "\n")
        .split('\n')
        .map(|l| l.trim_end().to_string())
        .collect();
    let m = base
        .iter()
        .position(|l| l.to_ascii_lowercase().starts_with("module "))
        .expect("module line");
    let keep: usize = field("keep").parse().unwrap();
    let mut out: Vec<String> = Vec::new();
    if field("premodule") == "1" {
        out.extend_from_slice(&base[..m]);
    }
    out.push(format!("module {} : integer;", field("module")));
    out.extend_from_slice(&base[m + 1..m + 1 + keep]);
    out.join("\n") + "\n" + tail
}
