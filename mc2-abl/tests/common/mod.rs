//! Test helpers: a minimal `.fst` reader (the real one lives in mc2-formats, which this crate must not depend on) and
//! small utilities for running ABL snippets.

#![allow(dead_code)]

use std::io::Read;
use std::path::PathBuf;

use mc2_abl::*;

/// One entry of a `.fst` archive.
pub struct FstEntry {
    pub name: String,
    pub data: Vec<u8>,
}

fn le32(d: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([d[off], d[off + 1], d[off + 2], d[off + 3]])
}

/// Read every entry of a `.fst` file: `u32 magic (0xFADDECAF)`, `u32 count`, then `count` packed entries of
/// `{u32 offset, u32 size, u32 realSize, u32 hash, char name[250]}`; data is zlib-compressed when size != realSize.
pub fn read_fst(path: &std::path::Path) -> Vec<FstEntry> {
    let d = std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert!(
        d.len() >= 8,
        "BLOCKED: truncated fst {} ({} bytes)",
        path.display(),
        d.len()
    );
    let magic = le32(&d, 0);
    assert!(
        magic == 0xFADD_ECAF || magic == 0xCADD_ECAF,
        "bad fst magic {magic:#x}"
    );
    let count = le32(&d, 4) as usize;
    let mut out = Vec::new();
    for i in 0..count {
        let e = 8 + i * 266;
        let off = le32(&d, e) as usize;
        let size = le32(&d, e + 4) as usize;
        let real = le32(&d, e + 8) as usize;
        let name_bytes = &d[e + 16..e + 266];
        let n = name_bytes.iter().position(|&b| b == 0).unwrap_or(250);
        let name = String::from_utf8_lossy(&name_bytes[..n]).into_owned();
        let raw = &d[off..off + size];
        let data = if size != real {
            let mut z = flate2::read::ZlibDecoder::new(raw);
            let mut v = Vec::with_capacity(real);
            z.read_to_end(&mut v).expect("zlib");
            v
        } else {
            raw.to_vec()
        };
        out.push(FstEntry { name, data });
    }
    out
}

/// Resolve a raw `MC2_DATA` value: `Ok(None)` when unset (tests may skip), `Err` when it is set but not a directory.
pub fn resolve_mc2_data(raw: Option<std::ffi::OsString>) -> Result<Option<PathBuf>, String> {
    match raw {
        None => Ok(None),
        Some(v) => {
            let p = PathBuf::from(v);
            if p.is_dir() {
                Ok(Some(p))
            } else {
                Err(format!(
                    "BLOCKED: MC2_DATA is set to {} which is not a directory",
                    p.display()
                ))
            }
        }
    }
}

/// `MC2_DATA`, or `None` when unset (tests skip cleanly). Panics when set to an invalid path.
pub fn mc2_data() -> Option<PathBuf> {
    resolve_mc2_data(std::env::var_os("MC2_DATA")).unwrap_or_else(|e| panic!("{e}"))
}

/// A source set with one file, for unit tests.
pub fn single(path: &str, text: &str) -> MemorySource {
    let mut m = MemorySource::new();
    m.insert(path, text);
    m
}

/// Compile and run a module from source text and return its integer result.
pub fn run_module(text: &str) -> Result<(Env, InstId), AblError> {
    let mut env = Env::new(
        Config::default(),
        single("t.abl", text),
        Box::new(DefaultHost),
    );
    let h = env.pre_process("t.abl")?;
    let inst = env.new_instance(h)?;
    env.execute(inst, &mut NullBindings)?;
    Ok((env, inst))
}
