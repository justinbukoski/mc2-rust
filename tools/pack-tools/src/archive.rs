//! `mkarchive (zip|tgz) SRC_DIR OUT EPOCH`: deterministic archive of SRC_DIR's tree.
//!
//! Entries sorted by name, mtime fixed to EPOCH, owner 0, modes normalised to 0755 / 0644 (executable
//! bit kept), gzip header time 0, so the same tree gives the same bytes on every host. The tree is
//! stored under its own directory name (`SRC_DIR/..` is the archive root). The tar is the GNU format
//! (`ustar  ` magic, `././@LongLink` entries for names over 100 bytes), the zip is deflate level 9 with
//! stored directories, as Python's `tarfile` and `zipfile` wrote them.
use crate::{deflate, io, Result};
use std::path::{Path, PathBuf};

pub struct Entry {
    /// Name inside the archive, `/` separated, no trailing slash.
    pub name: String,
    pub is_dir: bool,
    pub mode: u32,
    pub data: Vec<u8>,
}

fn walk(dir: &Path, prefix: &str, entries: &mut Vec<(String, PathBuf)>) -> Result<()> {
    for item in io(std::fs::read_dir(dir), dir)? {
        let item = io(item, dir)?;
        let name = item
            .file_name()
            .into_string()
            .map_err(|n| format!("{}: name is not UTF-8: {n:?}", dir.display()))?;
        let arc = format!("{prefix}/{name}");
        let path = item.path();
        // Like os.walk: a directory is entered unless it is a symlink.
        let descend = io(std::fs::symlink_metadata(&path), &path)?.is_dir();
        entries.push((arc.clone(), path.clone()));
        if descend {
            walk(&path, &arc, entries)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o100 != 0
}

/// Hosts without Unix modes (the Windows workspace check only compiles this tool) have no executable bit to read.
#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Every entry of the tree under `src`, sorted by archive name.
pub fn collect(src: &Path) -> Result<Vec<Entry>> {
    let src = io(std::fs::canonicalize(src), src)?;
    let top = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("source directory has no name")?
        .to_string();
    let mut found = Vec::new();
    walk(&src, &top, &mut found)?;
    found.sort();
    let mut entries = Vec::new();
    for (name, path) in found {
        let meta = io(std::fs::metadata(&path), &path)?;
        let executable = is_executable(&meta);
        let is_dir = meta.is_dir();
        entries.push(Entry {
            name,
            is_dir,
            mode: if is_dir || executable { 0o755 } else { 0o644 },
            data: if is_dir {
                Vec::new()
            } else {
                io(std::fs::read(&path), &path)?
            },
        });
    }
    Ok(entries)
}

// ---- tar (GNU format) ----

fn put(block: &mut [u8; 512], at: usize, bytes: &[u8]) {
    block[at..at + bytes.len()].copy_from_slice(bytes);
}

/// Octal number in a field of `width` bytes: zero padded digits and a NUL.
fn octal(block: &mut [u8; 512], at: usize, width: usize, value: u64) {
    put(
        block,
        at,
        format!("{value:0w$o}\0", w = width - 1).as_bytes(),
    );
}

fn tar_header(name: &[u8], mode: u32, size: u64, mtime: u64, kind: u8) -> [u8; 512] {
    let mut b = [0u8; 512];
    put(&mut b, 0, &name[..name.len().min(100)]);
    octal(&mut b, 100, 8, u64::from(mode));
    octal(&mut b, 108, 8, 0);
    octal(&mut b, 116, 8, 0);
    octal(&mut b, 124, 12, size);
    octal(&mut b, 136, 12, mtime);
    b[156] = kind;
    put(&mut b, 257, b"ustar  \0");
    let sum: u32 = b
        .iter()
        .enumerate()
        .map(|(i, &v)| u32::from(if (148..156).contains(&i) { b' ' } else { v }))
        .sum();
    put(&mut b, 148, format!("{sum:06o}\0 ").as_bytes());
    b
}

fn pad_block(out: &mut Vec<u8>) {
    out.resize(out.len().div_ceil(512) * 512, 0);
}

pub fn tar(entries: &[Entry], epoch: u64) -> Vec<u8> {
    let mut out = Vec::new();
    for e in entries {
        let mut name = e.name.clone().into_bytes();
        if e.is_dir {
            name.push(b'/');
        }
        if name.len() > 100 {
            let mut long = name.clone();
            long.push(0);
            out.extend_from_slice(&tar_header(b"././@LongLink", 0, long.len() as u64, 0, b'L'));
            out.extend_from_slice(&long);
            pad_block(&mut out);
        }
        let size = e.data.len() as u64;
        out.extend_from_slice(&tar_header(
            &name,
            e.mode,
            size,
            epoch,
            if e.is_dir { b'5' } else { b'0' },
        ));
        out.extend_from_slice(&e.data);
        pad_block(&mut out);
    }
    out.resize(out.len() + 1024, 0);
    out.resize(out.len().div_ceil(10240) * 10240, 0);
    out
}

/// A gzip member with no name and time 0, OS byte 255, deflate level 9.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 255];
    out.extend_from_slice(&deflate::raw(data, 9));
    out.extend_from_slice(&crc32fast::hash(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

// ---- zip ----

/// Seconds since 1970 as the (time, date) words of an MS-DOS timestamp, clamped to 1980-01-01.
fn dos_time(epoch: u64) -> (u16, u16) {
    let secs = epoch.max(315_532_800);
    let (days, rem) = (secs / 86400, secs % 86400);
    // civil-from-days (proleptic Gregorian)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let time = ((rem / 3600) << 11) | ((rem % 3600 / 60) << 5) | (rem % 60 / 2);
    let date = (((year - 1980) as u64) << 9) | ((month as u64) << 5) | day as u64;
    (time as u16, date as u16)
}

/// ZIP64 is not written: anything the classic format cannot hold is an error, never a silently truncated archive.
pub fn zip(entries: &[Entry], epoch: u64) -> Result<Vec<u8>> {
    if entries.len() > usize::from(u16::MAX) {
        return Err(format!(
            "zip: {} entries need ZIP64, which is not supported",
            entries.len()
        ));
    }
    let (time, date) = dos_time(epoch);
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for e in entries {
        let name = if e.is_dir {
            format!("{}/", e.name)
        } else {
            e.name.clone()
        };
        let crc = if e.is_dir {
            0
        } else {
            crc32fast::hash(&e.data)
        };
        let (method, body) = if e.is_dir {
            (0u16, Vec::new())
        } else {
            (8u16, deflate::raw(&e.data, 6))
        };
        let flags: u16 = if name.is_ascii() { 0 } else { 0x800 };
        let limit = u64::from(u32::MAX);
        if out.len() as u64 >= limit
            || body.len() as u64 >= limit
            || e.data.len() as u64 >= limit
            || name.len() > usize::from(u16::MAX)
        {
            return Err(format!(
                "zip: {:?} exceeds the classic zip limits (needs ZIP64, not supported)",
                e.name
            ));
        }
        let offset = out.len() as u32;
        let le16 = |v: &mut Vec<u8>, x: u16| v.extend_from_slice(&x.to_le_bytes());
        let le32 = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        // local header
        out.extend_from_slice(b"PK\x03\x04");
        for x in [20, flags, method, time, date] {
            le16(&mut out, x);
        }
        le32(&mut out, crc);
        le32(&mut out, body.len() as u32);
        le32(&mut out, e.data.len() as u32);
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&body);
        // central directory entry: created by Unix (3) with version 2.0, extract version 2.0
        central.extend_from_slice(b"PK\x01\x02");
        for x in [20 | (3 << 8), 20, flags, method, time, date] {
            le16(&mut central, x);
        }
        le32(&mut central, crc);
        le32(&mut central, body.len() as u32);
        le32(&mut central, e.data.len() as u32);
        le16(&mut central, name.len() as u16);
        for x in [0, 0, 0, 0] {
            le16(&mut central, x); // extra, comment, disk, internal attributes
        }
        let kind = if e.is_dir { 0o040_000 } else { 0o100_000 };
        le32(&mut central, (kind | e.mode) << 16);
        le32(&mut central, offset);
        central.extend_from_slice(name.as_bytes());
    }
    if (out.len() + central.len()) as u64 >= u64::from(u32::MAX) {
        return Err(
            "zip: archive exceeds the classic zip limits (needs ZIP64, not supported)".to_string(),
        );
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06");
    for x in [0u16, 0, entries.len() as u16, entries.len() as u16] {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

pub fn run(kind: &str, src: &Path, out: &Path, epoch: u64) -> Result<()> {
    let entries = collect(src)?;
    let bytes = match kind {
        "zip" => zip(&entries, epoch)?,
        "tgz" => gzip(&tar(&entries, epoch)),
        other => return Err(format!("unknown archive kind {other:?} (zip or tgz)")),
    };
    io(std::fs::write(out, bytes), out)
}
