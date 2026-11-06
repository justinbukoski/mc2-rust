//! Integration tests over the shipped game data.
//!
//! Set `MC2_DATA` to the data root (the directory holding the `.fst` files and
//! `system.cfg`). Without it every test returns early. Run with
//! `-- --nocapture` to see the per-file summary.

use mc2_formats::csv::CsvFile;
use mc2_formats::fst::{elf_hash, FastFile};
use mc2_formats::ini::FitIni;
use mc2_formats::lzw;
use mc2_formats::pak::{PakFile, StorageType};
use mc2_formats::vfs::normalize_name;
use mc2_formats::vfs::Vfs;
use std::fs::File;
use std::path::{Path, PathBuf};

fn data_root() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(v) => Some(PathBuf::from(v)),
        None => {
            eprintln!("MC2_DATA not set; skipping shipped-data test");
            None
        }
    }
}

fn fst_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("read MC2_DATA")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fst")))
        .collect();
    paths.sort();
    paths
}

/// All regular files under `root`, relative to it.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read dir").flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p.strip_prefix(root).expect("under root").to_path_buf());
            }
        }
    }
    out.sort();
    out
}

fn ext_lower(name: &str) -> String {
    name.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}

#[test]
fn shipped_archives_decompress_every_entry() {
    let Some(root) = data_root() else { return };
    let mut files = 0usize;
    let mut entries = 0usize;
    let mut failures: Vec<String> = Vec::new();
    let mut bytes_out = 0u64;
    let mut backslash_names = 0usize;
    let mut hash_raw = 0usize;
    let mut hash_lower = 0usize;
    let mut hash_neither = 0usize;

    for path in fst_paths(&root) {
        let mut ff = FastFile::open(&path).expect("open archive");
        files += 1;
        let kind = ff.kind();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let count = ff.entries().len();
        println!("{name}: {kind:?}, {count} entries");
        for i in 0..count {
            entries += 1;
            let entry = ff.entries()[i].clone();
            if entry.name.contains('\\') {
                backslash_names += 1;
            }
            if entry.hash == elf_hash(entry.name.as_bytes()) {
                hash_raw += 1;
            } else if entry.hash == elf_hash(entry.name.to_ascii_lowercase().as_bytes()) {
                hash_lower += 1;
            } else {
                hash_neither += 1;
            }
            match ff.read_index(i) {
                Ok(data) => {
                    bytes_out += data.len() as u64;
                }
                Err(e) => failures.push(format!("{name}:{} {e}", entry.name)),
            }
        }
    }

    println!(
        "FST summary: {files} archives, {entries} entries, {} bytes decompressed",
        bytes_out
    );
    println!("FST summary: {} failures", failures.len());
    println!("FST names: {backslash_names} with backslashes; hash matches raw name {hash_raw}, lowercased name {hash_lower}, neither {hash_neither}");
    assert!(files > 0, "no .fst files in MC2_DATA");
    assert!(
        failures.is_empty(),
        "entries failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn shipped_text_files_parse() {
    let Some(root) = data_root() else { return };
    let mut vfs = Vfs::from_system_cfg(&root).expect("system.cfg");
    let mut names: Vec<(String, Vec<u8>)> = Vec::new();

    // Files inside the archives, by their stored names.
    let mut archive_names = Vec::new();
    for path in fst_paths(&root) {
        let ff = FastFile::open(&path).expect("open archive");
        for e in ff.entries() {
            let ext = ext_lower(&e.name);
            if matches!(ext.as_str(), "fit" | "ini" | "csv" | "cfg") {
                archive_names.push(e.name.clone());
            }
        }
    }
    for name in &archive_names {
        if let Some(bytes) = vfs.read(name).expect("read archive text") {
            names.push((format!("archive:{name}"), bytes));
        }
    }
    // Loose files on disk.
    for rel in walk(&root) {
        let name = rel.to_string_lossy().replace('\\', "/");
        if matches!(ext_lower(&name).as_str(), "fit" | "ini" | "csv" | "cfg") {
            names.push((
                format!("disk:{name}"),
                std::fs::read(root.join(&rel)).expect("read disk"),
            ));
        }
    }

    let (mut fit, mut csv, mut blocks, mut keys_checked) = (0usize, 0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    let mut not_fit = Vec::new();
    for (label, bytes) in names {
        let ext = ext_lower(&label);
        if ext == "csv" {
            match CsvFile::from_bytes(bytes) {
                Ok(c) => {
                    csv += 1;
                    if c.rows() > 0 {
                        let _ = c.cell(1, 1);
                    }
                }
                Err(e) => failures.push(format!("{label}: {e}")),
            }
            continue;
        }
        match FitIni::from_bytes(bytes) {
            Ok(ini) => {
                fit += 1;
                blocks += ini.block_count();
                for id in ini.blocks().map(str::to_owned).collect::<Vec<_>>() {
                    let mut ini = ini.clone();
                    ini.seek_block(&id).expect("listed block");
                    keys_checked += 1;
                }
            }
            Err(e) => {
                if label.ends_with(".cfg") {
                    not_fit.push(format!("{label}: {e}"));
                } else {
                    failures.push(format!("{label}: {e}"));
                }
            }
        }
    }
    println!(
        "FIT/INI summary: {fit} files parsed, {blocks} blocks, {keys_checked} blocks re-selected"
    );
    println!("CSV summary: {csv} files parsed");
    println!("Non-FIT .cfg files (informational): {}", not_fit.len());
    for line in &not_fit {
        println!("  {line}");
    }
    assert!(
        failures.is_empty(),
        "text files failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn shipped_pak_packets_all_read() {
    let Some(root) = data_root() else { return };
    let mut vfs = Vfs::from_system_cfg(&root).expect("system.cfg");
    let mut sources: Vec<(String, Vec<u8>)> = Vec::new();
    let mut archive_pak_names = Vec::new();
    for path in fst_paths(&root) {
        let ff = FastFile::open(&path).expect("open archive");
        for e in ff.entries() {
            if ext_lower(&e.name) == "pak" {
                archive_pak_names.push(e.name.clone());
            }
        }
    }
    for name in &archive_pak_names {
        if let Some(bytes) = vfs.read(name).expect("read archive pak") {
            sources.push((format!("archive:{name}"), bytes));
        }
    }
    for rel in walk(&root) {
        let name = rel.to_string_lossy().replace('\\', "/");
        if ext_lower(&name) == "pak" {
            sources.push((
                format!("disk:{name}"),
                std::fs::read(root.join(&rel)).expect("read disk"),
            ));
        }
    }

    let (mut files, mut packets, mut bytes_out) = (0usize, 0usize, 0u64);
    let mut kinds = std::collections::BTreeMap::<String, usize>::new();
    let mut failures = Vec::new();
    let (mut lzd_checked, mut lzd_exact) = (0usize, 0usize);
    for (label, bytes) in sources {
        let pak = match PakFile::from_bytes(bytes) {
            Ok(p) => p,
            Err(e) => {
                failures.push(format!("{label}: {e}"));
                continue;
            }
        };
        files += 1;
        for i in 0..pak.packet_count() {
            let info = pak.info(i).expect("packet info");
            *kinds.entry(format!("{:?}", info.storage)).or_default() += 1;
            packets += 1;
            match pak.read_packet(i) {
                Ok(data) => {
                    bytes_out += data.len() as u64;
                    if info.storage == StorageType::Lzd && !data.is_empty() {
                        lzd_checked += 1;
                        let body = &pak.stored_bytes(i).expect("stored bytes")[4..];
                        if lzw::compress(&data) == body {
                            lzd_exact += 1;
                        }
                    }
                }
                Err(e) => failures.push(format!("{label} packet {i}: {e}")),
            }
        }
    }
    println!("PAK summary: {files} files, {packets} packets, {bytes_out} bytes decompressed");
    println!("PAK storage types: {kinds:?}");
    println!("PAK summary: {} failures", failures.len());
    println!("LZD packets: {lzd_exact} of {lzd_checked} re-encode byte-identical");
    assert!(
        failures.is_empty(),
        "packets failed:\n{}",
        failures.join("\n")
    );
}

#[test]
fn vfs_mounts_system_cfg_archives_in_order() {
    let Some(root) = data_root() else { return };
    let mut vfs = Vfs::from_system_cfg(&root).expect("system.cfg");
    let mounted: Vec<String> = vfs
        .archive_paths()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    println!("VFS archives in order: {mounted:?}");
    assert_eq!(
        mounted,
        [
            "mission.fst",
            "tgl.fst",
            "art.fst",
            "textures.fst",
            "misc.fst",
            "camera.fst",
            "effect.fst",
            "insignia.fst"
        ]
    );
    let cfg = vfs
        .read_fit("system.cfg")
        .expect("read")
        .expect("system.cfg present");
    assert!(cfg.has_block("FastFiles"));
    assert!(vfs.locate("system.cfg").is_some());
    // Name normalisation is shared by the index and the VFS.
    assert_eq!(normalize_name("DATA\\Art/X.TGA"), "data/art/x.tga");
    let _ = File::open(root.join("system.cfg")).expect("system.cfg on disk");
}
