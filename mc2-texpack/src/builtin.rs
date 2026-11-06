//! The pack the game builds on the player's own machine, from the player's own data.
//!
//! Microsoft art never ships in a package, so the default upscaled pack cannot either.  On the
//! first run (or when the pack is missing, stale for the configured data, or damaged) the app
//! calls [`ensure_pack`]: it enumerates every texture with [`collect`], then writes a 2x
//! Catmull-Rom replacement for each hooked one into the pack folder, in the same flat
//! `<key>.png` + `manifest.json` layout `mc2-texpack verify` checks.
//!
//! * Resumable: every PNG is written to a temporary name and renamed.  A later run keeps the
//!   files already there only when they were made from the same data (the fingerprint stamped
//!   beside the dump) and they decode at the right size.  `builtin.json` is written last; a
//!   folder without it is a partial build and is continued, not trusted.
//! * Bounded: [`BuildOptions::budget`] covers the whole call, fingerprinting and validation
//!   included.  The job runs on a helper thread; if it outlasts the budget
//!   [`BuildOutcome::Collecting`] is returned, the pack stays off this run, and the thread goes
//!   on to finish the pack in the background.  Between textures the build itself stops when the
//!   budget runs out (the game then starts with what exists; the renderer falls back to the
//!   original pixels for the rest).
//! * Honest: the marker records a content hash of every PNG, and the collection failures
//!   (unreadable or unwritable textures).  A pack with failures is served but never `Valid`; the
//!   next start collects again.
//! * Deterministic: plain `f32` `+`/`*` only, so the pixels are identical on every platform.
//!
//! Paint-mask textures (pure red/green/blue pixels the mech paint scheme recolours) are
//! enlarged by pixel repetition so those pixels stay exactly pure.

use crate::collect::{collect, Options};
use crate::model::*;
use mc2_gfx::texture as tx;
use mc2_render::game::GameData;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Bumped when the output of this builder changes; part of the staleness check.
pub const BUILD_VERSION: u32 = 2;
/// Marker file written after the last texture.
pub const MARKER: &str = "builtin.json";
/// The factor this builder uses.
pub const SCALE: u32 = 2;
const WORK_DIR: &str = ".work";
/// Loose files up to this size are fingerprinted by content, larger ones by length and mtime.
const LOOSE_HASH_LIMIT: u64 = 16 << 20;

/// Length and content hash of one pack PNG.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileRec {
    pub len: u64,
    pub hash: String,
}

/// `builtin.json`: proof that a build finished, and what it was built from.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Marker {
    pub version: u32,
    pub fingerprint: String,
    /// Length and content hash of every pack PNG, to catch truncated or altered files.
    pub files: BTreeMap<String, FileRec>,
    /// Textures the collection could not read or write; non-empty means "retry next start".
    #[serde(default)]
    pub failures: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackStatus {
    /// No folder, or no finished-build marker (also a build that was interrupted).
    Missing,
    /// Finished, but for different game data or by an older builder.
    Stale,
    /// Finished, but a file is missing or does not match its recorded hash.
    Corrupt(String),
    /// Finished and intact, but some textures could not be collected; usable, retried next start.
    Incomplete(usize),
    Valid,
}

/// A collection that outlived the start-up budget and carries on (then builds the pack).
pub struct Background {
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Background {
    /// Blocks until the background work is over (tests; the game never waits).
    pub fn wait(&self) {
        let h = self.handle.lock().ok().and_then(|mut g| g.take());
        if let Some(h) = h {
            let _ = h.join();
        }
    }
}

impl std::fmt::Debug for Background {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Background")
    }
}

#[derive(Debug)]
pub enum BuildOutcome {
    /// The pack is complete (`textures` replacements).
    Complete { textures: usize },
    /// The time budget ran out; `done` of `total` replacements exist.
    Partial { done: usize, total: usize },
    /// Reading the textures is still going on; nothing is usable this run.
    Collecting(Arc<Background>),
    /// The folder is not game data (no readable `system.cfg`/archive table): nothing was done.
    NotGameData,
}

pub struct BuildOptions {
    /// Stop after this long (the whole call, collection included); `None` builds everything.
    pub budget: Option<Duration>,
    /// Slept after every megabyte read while fingerprinting and validating (tests simulate slow
    /// storage with it); zero in the game.
    pub read_delay: Duration,
}

/// A fast non-cryptographic 64-bit hash (FNV-style, eight bytes at a time).
struct Fnv(u64);
impl Fnv {
    const P: u64 = 0x0000_0100_0000_01b3;
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, b: &[u8]) {
        let mut words = b.chunks_exact(8);
        for w in &mut words {
            let v = u64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
            self.0 = (self.0 ^ v).wrapping_mul(Self::P).rotate_left(29);
        }
        for &x in words.remainder() {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(Self::P);
        }
        // length-delimit successive writes
        self.0 = (self.0 ^ b.len() as u64).wrapping_mul(Self::P);
    }
    fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Hashes a file's content in 1 MiB pieces; `None` when it cannot be read.
fn hash_file(path: &Path, pace: Duration) -> Option<(u64, u64)> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Fnv::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.write(&buf[..n]);
        total += n as u64;
        if !pace.is_zero() {
            std::thread::sleep(pace);
        }
    }
    Some((total, h.0))
}

fn mtime_nanos(m: &std::fs::Metadata) -> u128 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
}

/// Loose files below `dir` in sorted order (symlinked directories are not followed).
/// Hard cap on loose files and walk depth: the game data is a few thousand files in a shallow tree.
const LOOSE_FILE_CAP: usize = 100_000;
const LOOSE_DEPTH_CAP: u32 = 6;

#[cfg(unix)]
fn device_of(meta: &std::fs::Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::dev(meta)
}
#[cfg(not(unix))]
fn device_of(_: &std::fs::Metadata) -> u64 {
    0
}

/// Loose files under the data folder: never follows symlinks, never leaves the data folder's
/// filesystem, and stops at a fixed depth and file count.
fn loose_files(dir: &Path, rel: &str, depth: u32, dev: u64, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kids: Vec<_> = rd.flatten().collect();
    kids.sort_by_key(|e| e.file_name());
    for e in kids {
        if out.len() >= LOOSE_FILE_CAP {
            return;
        }
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        let rel_name = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        // DirEntry::file_type does not follow symlinks
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let same_fs = e.metadata().map(|m| device_of(&m) == dev).unwrap_or(false);
            if depth < LOOSE_DEPTH_CAP && same_fs {
                loose_files(&e.path(), &rel_name, depth + 1, dev, out);
            }
        } else if !name.ends_with(".fst") {
            out.push((rel_name, e.path()));
        }
    }
}

/// Identity of the game data a pack was made from, as the production file lookup sees it: the
/// content of `system.cfg` and of every mounted archive, and every loose file that can override
/// an archived one (name, length, content; mtime for the very large).  Changes when the player
/// swaps, patches or overrides any of it.
pub fn data_fingerprint(data: &Path) -> String {
    data_fingerprint_paced(data, Duration::ZERO)
}

/// [`data_fingerprint`] with `pace` slept after every megabyte read (a slow-disk stand-in).
pub fn data_fingerprint_paced(data: &Path, pace: Duration) -> String {
    let mut h = Fnv::new();
    h.write(&BUILD_VERSION.to_le_bytes());
    match mc2_formats::vfs::Vfs::from_system_cfg(data) {
        Ok(vfs) => {
            for p in vfs.archive_paths() {
                h.write(p.file_name().unwrap_or_default().as_encoded_bytes());
                match hash_file(p, pace) {
                    Some((len, hash)) => {
                        h.write(&len.to_le_bytes());
                        h.write(&hash.to_le_bytes());
                    }
                    None => h.write(b"unreadable archive"),
                }
            }
        }
        Err(e) => {
            // not a game data folder: nothing to walk
            h.write(format!("no vfs: {e}").as_bytes());
            return h.hex();
        }
    }
    let mut files = Vec::new();
    let dev = std::fs::metadata(data).map(|m| device_of(&m)).unwrap_or(0);
    loose_files(data, "", 0, dev, &mut files);
    for (rel, path) in files {
        h.write(rel.as_bytes());
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        h.write(&meta.len().to_le_bytes());
        if meta.len() <= LOOSE_HASH_LIMIT {
            if let Some((_, hash)) = hash_file(&path, pace) {
                h.write(&hash.to_le_bytes());
            }
        } else {
            h.write(&mtime_nanos(&meta).to_le_bytes());
        }
    }
    h.hex()
}

fn read_marker(pack: &Path) -> Option<Marker> {
    serde_json::from_slice(&std::fs::read(pack.join(MARKER)).ok()?).ok()
}

fn file_rec(path: &Path, pace: Duration) -> Option<FileRec> {
    let (len, hash) = hash_file(path, pace)?;
    Some(FileRec {
        len,
        hash: format!("{hash:016x}"),
    })
}

/// Judges the pack folder against the data it should match.
pub fn pack_status(pack: &Path, fingerprint: &str) -> PackStatus {
    pack_status_paced(pack, fingerprint, Duration::ZERO)
}

/// [`pack_status`] with `pace` slept after every megabyte read.
pub fn pack_status_paced(pack: &Path, fingerprint: &str, pace: Duration) -> PackStatus {
    let Some(marker) = read_marker(pack) else {
        return PackStatus::Missing;
    };
    if marker.version != BUILD_VERSION || marker.fingerprint != fingerprint {
        return PackStatus::Stale;
    }
    for (name, rec) in &marker.files {
        match file_rec(&pack.join(name), pace) {
            Some(r) if r == *rec => {}
            Some(r) => {
                return PackStatus::Corrupt(format!(
                    "{name}: {} bytes do not match the recorded {} bytes / hash",
                    r.len, rec.len
                ))
            }
            None => return PackStatus::Corrupt(format!("{name}: unreadable")),
        }
    }
    if crate::verify::read_pack_manifest(pack).is_err() {
        return PackStatus::Corrupt("manifest.json unreadable".into());
    }
    if marker.failures.is_empty() {
        PackStatus::Valid
    } else {
        PackStatus::Incomplete(marker.failures.len())
    }
}

/// Fully decodes an RGBA8 PNG; `None` for anything damaged or not 8-bit RGBA.
fn read_rgba_opt(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    read_rgba(path).ok()
}

fn read_rgba(path: &Path) -> Result<(Vec<u8>, u32, u32), String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut r = png::Decoder::new(std::io::BufReader::new(f))
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut buf = vec![0u8; r.output_buffer_size().ok_or("png too large")?];
    let info = r
        .next_frame(&mut buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    buf.truncate(info.buffer_size());
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{}: not 8-bit RGBA", path.display()));
    }
    Ok((buf, info.width, info.height))
}

/// What reading the game's textures produced.
struct Dump {
    manifest: DumpManifest,
    failures: Vec<String>,
}

fn stamp_matches(work: &Path, fp: &str) -> bool {
    std::fs::read_to_string(work.join("fingerprint")).is_ok_and(|s| s == fp)
}

/// The dump of the originals, reused when an interrupted build already made it for this data.
fn load_dump(work: &Path, fp: &str) -> Option<Dump> {
    if !stamp_matches(work, fp) {
        return None;
    }
    let manifest = crate::collect::read_dump_manifest(&work.join("manifest.json")).ok()?;
    let failures = serde_json::from_slice(&std::fs::read(work.join("failures.json")).ok()?).ok()?;
    Some(Dump { manifest, failures })
}

/// Reads every texture of the data into `work` (originals as PNGs plus the inventory).
fn collect_into_work(data: &Path, work: &Path, fp: &str) -> Result<Dump, String> {
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
    let mut game = GameData::open(data).map_err(|e| format!("{}: {e}", data.display()))?;
    let res = collect(
        &mut game,
        &Options {
            missions: None,
            originals: Some(work.to_path_buf()),
            verbose: false,
        },
    );
    let json = serde_json::to_vec(&res.manifest).map_err(|e| e.to_string())?;
    std::fs::write(work.join("manifest.json"), json).map_err(|e| e.to_string())?;
    let json = serde_json::to_vec(&res.failures).map_err(|e| e.to_string())?;
    std::fs::write(work.join("failures.json"), json).map_err(|e| e.to_string())?;
    // the stamp goes last: a dump without it is redone
    std::fs::write(work.join("fingerprint"), fp).map_err(|e| e.to_string())?;
    Ok(Dump {
        manifest: res.manifest,
        failures: res.failures,
    })
}

enum Msg {
    Say(String),
    Done(Result<BuildOutcome, String>),
}

/// Makes `pack` a finished pack for `data`, building or continuing it as needed.  `progress`
/// gets one human-readable line per phase and every few hundred textures.
///
/// With a budget the whole job (fingerprinting, validation, reading the textures, building) runs
/// on a helper thread and the call returns by the deadline: when the work is not over by then,
/// [`BuildOutcome::Collecting`] comes back, nothing is usable this run, and the thread finishes
/// the pack unbounded.
pub fn ensure_pack(
    data: &Path,
    pack: &Path,
    opts: &BuildOptions,
    progress: &mut dyn FnMut(&str),
) -> Result<BuildOutcome, String> {
    let started = Instant::now();
    let Some(budget) = opts.budget else {
        return run_job(
            data,
            pack,
            None,
            started,
            opts.read_delay,
            &AtomicBool::new(false),
            progress,
        );
    };
    let (tx, rx) = mpsc::channel::<Msg>();
    let abandoned = Arc::new(AtomicBool::new(false));
    let (d, p, ab, pace) = (
        data.to_path_buf(),
        pack.to_path_buf(),
        abandoned.clone(),
        opts.read_delay,
    );
    let handle = std::thread::spawn(move || {
        let mut say = |m: &str| {
            if ab.load(Ordering::Relaxed) {
                eprintln!("{m}");
            } else {
                let _ = tx.send(Msg::Say(m.to_owned()));
            }
        };
        let res = run_job(&d, &p, Some(budget), started, pace, &ab, &mut say);
        let _ = tx.send(Msg::Done(res));
    });
    let finished = |msg: Msg, progress: &mut dyn FnMut(&str)| match msg {
        Msg::Say(m) => {
            progress(&m);
            None
        }
        Msg::Done(r) => Some(r),
    };
    loop {
        match rx.recv_timeout(budget.saturating_sub(started.elapsed())) {
            Ok(m) => {
                if let Some(r) = finished(m, progress) {
                    let _ = handle.join();
                    return r;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                abandoned.store(true, Ordering::Relaxed);
                // the result may have landed in the instant before the flag
                while let Ok(m) = rx.try_recv() {
                    if let Some(r) = finished(m, progress) {
                        let _ = handle.join();
                        return r;
                    }
                }
                progress(
                    "upscaled textures: still preparing, the pack will be ready on a later start",
                );
                return Ok(BuildOutcome::Collecting(Arc::new(Background {
                    handle: Mutex::new(Some(handle)),
                })));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the pack builder stopped unexpectedly".into())
            }
        }
    }
}

/// The whole job.  `budget` bounds the build between textures unless `abandoned` says nobody is
/// waiting any more.
fn run_job(
    data: &Path,
    pack: &Path,
    budget: Option<Duration>,
    started: Instant,
    pace: Duration,
    abandoned: &AtomicBool,
    progress: &mut dyn FnMut(&str),
) -> Result<BuildOutcome, String> {
    // the source-reading preflight (system.cfg, archive table) runs under the same deadline
    if mc2_formats::vfs::Vfs::from_system_cfg(data).is_err() {
        return Ok(BuildOutcome::NotGameData);
    }
    let fp = data_fingerprint_paced(data, pace);
    match pack_status_paced(pack, &fp, pace) {
        PackStatus::Valid => {
            let n = read_marker(pack).map_or(0, |m| m.files.len());
            return Ok(BuildOutcome::Complete { textures: n });
        }
        PackStatus::Stale => {
            progress("upscaled textures: game data changed, rebuilding");
            clear_pack(pack);
        }
        PackStatus::Corrupt(why) => {
            progress(&format!(
                "upscaled textures: damaged pack ({why}), rebuilding"
            ));
            // keep nothing we cannot vouch for
            clear_pack(pack);
        }
        PackStatus::Incomplete(n) => {
            progress(&format!(
                "upscaled textures: {n} textures could not be read last time, trying again"
            ));
        }
        PackStatus::Missing => {}
    }
    std::fs::create_dir_all(pack).map_err(|e| format!("{}: {e}", pack.display()))?;
    let work = pack.join(WORK_DIR);
    // PNGs may be kept only when they provably come from this data: the dump stamp of an
    // interrupted build, or the marker of a finished (incomplete) one.
    let trusted = stamp_matches(&work, &fp)
        || read_marker(pack).is_some_and(|m| m.version == BUILD_VERSION && m.fingerprint == fp);
    if !trusted {
        clear_pack(pack);
        std::fs::create_dir_all(pack).map_err(|e| format!("{}: {e}", pack.display()))?;
    }
    let dump = match load_dump(&work, &fp) {
        Some(d) => d,
        None => {
            progress("upscaled textures: reading the game's textures (first run only)");
            collect_into_work(data, &work, &fp)?
        }
    };
    let clock = Clock {
        budget,
        started,
        abandoned,
    };
    build(pack, &work, &dump, &fp, clock, progress)
}

/// The build's time limit: `budget` from `started`, void once `abandoned` is set.
#[derive(Clone, Copy)]
struct Clock<'a> {
    budget: Option<Duration>,
    started: Instant,
    abandoned: &'a AtomicBool,
}

/// Writes the replacements for `dump` and, when all are there, the marker.
fn build(
    pack: &Path,
    work: &Path,
    dump: &Dump,
    fp: &str,
    clock: Clock<'_>,
    progress: &mut dyn FnMut(&str),
) -> Result<BuildOutcome, String> {
    let Clock {
        budget,
        started,
        abandoned,
    } = clock;
    let todo: Vec<&Entry> = dump
        .manifest
        .entries
        .iter()
        .filter(|e| e.hooked && !e.ambiguous && e.alpha != AlphaClass::Zero)
        .filter(|e| scale_for(e) > 1 && !e.file.is_empty())
        .collect();
    let total = todo.len();
    let mut files: BTreeMap<String, FileRec> = BTreeMap::new();
    let mut made: Vec<PackEntry> = Vec::new();
    let mut done = 0usize;
    for (i, e) in todo.iter().enumerate() {
        if let Some(b) = budget {
            if !abandoned.load(Ordering::Relaxed) && started.elapsed() >= b {
                let _ = write_manifest(pack, &made);
                progress(&format!(
                    "upscaled textures: {done}/{total} ready, the rest continues on the next start"
                ));
                return Ok(BuildOutcome::Partial { done, total });
            }
        }
        let scale = scale_for(e);
        let name = format!("{}.png", e.key);
        let out = pack.join(&name);
        let want = [e.width * scale, e.height * scale];
        // a kept file must decode completely at the right size, not merely look like a PNG
        let reuse = read_rgba_opt(&out).is_some_and(|(_, w, h)| (w, h) == (want[0], want[1]));
        if !reuse {
            let (px, w, h) = read_rgba(&work.join(&e.file))?;
            if (w, h) != (e.width, e.height) {
                return Err(format!("{}: dump size mismatch", e.key));
            }
            let up = upscale(&px, w, h, scale, e);
            let tmp = pack.join(format!("{name}.tmp"));
            tx::write_png(&tmp, &up, want[0], want[1], mc2_gfx::MC2GFX_TEXFMT_RGBA8)
                .and_then(|()| std::fs::rename(&tmp, &out))
                .map_err(|er| format!("{}: {er}", out.display()))?;
        }
        let rec = file_rec(&out, Duration::ZERO)
            .ok_or_else(|| format!("{}: unreadable", out.display()))?;
        files.insert(name, rec);
        made.push(PackEntry {
            key: e.key.clone(),
            name: e.name.clone(),
            category: e.category,
            original: [e.width, e.height],
            size: want,
            scale,
            model: MODEL_NAME.into(),
            steps: steps_for(e),
            source_hash: e.hash.clone(),
        });
        done += 1;
        if i % 500 == 499 {
            progress(&format!("upscaled textures: {done}/{total}"));
        }
    }
    write_manifest(pack, &made)?;
    let marker = Marker {
        version: BUILD_VERSION,
        fingerprint: fp.to_owned(),
        files,
        failures: dump.failures.iter().take(50).cloned().collect(),
    };
    let json = serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?;
    let tmp = pack.join(format!("{MARKER}.tmp"));
    std::fs::write(&tmp, json)
        .and_then(|()| std::fs::rename(&tmp, pack.join(MARKER)))
        .map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(work);
    if dump.failures.is_empty() {
        progress(&format!("upscaled textures: {done} ready"));
    } else {
        progress(&format!(
            "upscaled textures: {done} ready, {} textures could not be read (retried next start)",
            dump.failures.len()
        ));
    }
    Ok(BuildOutcome::Complete { textures: done })
}

const MODEL_NAME: &str = "builtin-catmull-rom-2x";

fn scale_for(e: &Entry) -> u32 {
    SCALE.min(e.recommended_scale)
}

fn steps_for(e: &Entry) -> Vec<String> {
    let mut s = Vec::new();
    if e.paint_mask_pixels > 0 {
        s.push("paint_mask_repeat".to_string());
    } else {
        s.push("catmull_rom".to_string());
        if e.tileable {
            s.push("wrap_pad".to_string());
        }
        if e.alpha != AlphaClass::None {
            s.push("alpha_premultiplied".to_string());
        }
    }
    s
}

fn clear_pack(pack: &Path) {
    let _ = std::fs::remove_file(pack.join(MARKER));
    if let Ok(rd) = std::fs::read_dir(pack) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "png" || x == "tmp") {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    let _ = std::fs::remove_file(pack.join("manifest.json"));
    let _ = std::fs::remove_dir_all(pack.join(WORK_DIR));
}

fn write_manifest(pack: &Path, entries: &[PackEntry]) -> Result<(), String> {
    let m = PackManifest {
        format: PACK_FORMAT.into(),
        generator: "mc2 builtin pack builder".into(),
        created: String::new(),
        models: vec![ModelInfo {
            name: MODEL_NAME.into(),
            licence: "none (plain resampling of the player's own data)".into(),
            source: "built on the player's machine".into(),
            sha256: String::new(),
        }],
        entries: entries.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?;
    std::fs::write(pack.join("manifest.json"), json).map_err(|e| e.to_string())
}

/// Where the pack of a data folder lives under the player's cache directory.
pub fn pack_path(cache_root: &Path) -> PathBuf {
    cache_root.join("hires")
}

// ---- resampling -----------------------------------------------------------------------------

/// Catmull-Rom weights for the sample at fractional offset `t` between taps 1 and 2.
fn weights(t: f32) -> [f32; 4] {
    let t2 = t * t;
    let t3 = t2 * t;
    [
        -0.5 * t3 + t2 - 0.5 * t,
        1.5 * t3 - 2.5 * t2 + 1.0,
        -1.5 * t3 + 2.0 * t2 + 0.5 * t,
        0.5 * t3 - 0.5 * t2,
    ]
}

/// Enlarges RGBA8 `px` by the integer `scale`.
pub fn upscale(px: &[u8], w: u32, h: u32, scale: u32, e: &Entry) -> Vec<u8> {
    let (w, h, s) = (w as usize, h as usize, scale as usize);
    let (ow, oh) = (w * s, h * s);
    let mut out = vec![0u8; ow * oh * 4];
    if e.paint_mask_pixels > 0 {
        for y in 0..oh {
            for x in 0..ow {
                let src = ((y / s) * w + x / s) * 4;
                let dst = (y * ow + x) * 4;
                out[dst..dst + 4].copy_from_slice(&px[src..src + 4]);
            }
        }
        return out;
    }
    let wrap = e.tileable;
    let idx = |i: isize, n: usize| -> usize {
        if wrap {
            i.rem_euclid(n as isize) as usize
        } else {
            i.clamp(0, n as isize - 1) as usize
        }
    };
    let phase: Vec<[f32; 4]> = (0..s)
        .map(|k| weights((k as f32 + 0.5) / s as f32))
        .collect();
    // premultiplied source: [r*a, g*a, b*a, a] in 0..255 units
    let pre = |i: usize| -> [f32; 4] {
        let a = f32::from(px[i * 4 + 3]);
        let k = a / 255.0;
        [
            f32::from(px[i * 4]) * k,
            f32::from(px[i * 4 + 1]) * k,
            f32::from(px[i * 4 + 2]) * k,
            a,
        ]
    };
    // horizontal pass: h rows of ow texels
    let mut mid = vec![0f32; ow * h * 4];
    for y in 0..h {
        for x in 0..ow {
            let wt = &phase[x % s];
            let cx = (x / s) as isize;
            let mut acc = [0f32; 4];
            for (t, wv) in wt.iter().enumerate() {
                let p = pre(y * w + idx(cx + t as isize - 1, w));
                for c in 0..4 {
                    acc[c] += p[c] * wv;
                }
            }
            mid[(y * ow + x) * 4..][..4].copy_from_slice(&acc);
        }
    }
    for y in 0..oh {
        let wt = &phase[y % s];
        let cy = (y / s) as isize;
        let rows = [idx(cy - 1, h), idx(cy, h), idx(cy + 1, h), idx(cy + 2, h)];
        for x in 0..ow {
            let mut acc = [0f32; 4];
            for (t, wv) in wt.iter().enumerate() {
                let m = &mid[(rows[t] * ow + x) * 4..][..4];
                for c in 0..4 {
                    acc[c] += m[c] * wv;
                }
            }
            let src = ((y / s) * w + x / s) * 4;
            let dst = (y * ow + x) * 4;
            let a = acc[3].clamp(0.0, 255.0);
            let alpha = if e.alpha == AlphaClass::Smooth {
                a
            } else {
                // cut-outs and opaque images keep their hard edge
                f32::from(px[src + 3])
            };
            for c in 0..3 {
                out[dst + c] = if a >= 1.0 {
                    (acc[c] * 255.0 / a + 0.5).clamp(0.0, 255.0) as u8
                } else {
                    px[src + c]
                };
            }
            out[dst + 3] = (alpha + 0.5) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tileable: bool, alpha: AlphaClass, paint: u32) -> Entry {
        Entry {
            key: "k".into(),
            name: "n".into(),
            category: Category::Ui,
            width: 0,
            height: 0,
            alpha,
            alpha_used: true,
            paint_mask_pixels: paint,
            tileable,
            hooked: true,
            recommended_scale: 4,
            hash: String::new(),
            ambiguous: false,
            aliases: vec![],
            use_count: 0,
            used_by: vec![],
            file: String::new(),
        }
    }

    #[test]
    fn flat_colour_stays_flat_and_size_doubles() {
        let px: Vec<u8> = (0..16).flat_map(|_| [10, 200, 30, 255]).collect();
        let out = upscale(&px, 4, 4, 2, &entry(false, AlphaClass::None, 0));
        assert_eq!(out.len(), 8 * 8 * 4);
        assert!(out.chunks(4).all(|p| p == [10, 200, 30, 255]));
    }

    #[test]
    fn paint_mask_pixels_stay_pure() {
        let px = [255, 0, 0, 255, 0, 255, 0, 255];
        let out = upscale(&px, 2, 1, 2, &entry(false, AlphaClass::None, 2));
        assert_eq!(&out[..8], &[255, 0, 0, 255, 255, 0, 0, 255]);
        assert_eq!(&out[8..16], &[0, 255, 0, 255, 0, 255, 0, 255]);
    }

    #[test]
    fn keyed_alpha_stays_hard_and_transparent_colour_does_not_bleed() {
        // left opaque white, right transparent black
        let px = [255, 255, 255, 255, 0, 0, 0, 0];
        let out = upscale(&px, 2, 1, 2, &entry(false, AlphaClass::Keyed, 0));
        for p in out.chunks(4) {
            assert!(p[3] == 0 || p[3] == 255);
            if p[3] == 255 {
                assert_eq!(&p[..3], &[255, 255, 255]);
            }
        }
    }

    #[test]
    fn deterministic() {
        let px: Vec<u8> = (0..64u32).map(|i| (i * 37 % 251) as u8).collect();
        let e = entry(true, AlphaClass::Smooth, 0);
        assert_eq!(upscale(&px, 4, 4, 2, &e), upscale(&px, 4, 4, 2, &e));
    }
}

#[cfg(test)]
mod walk_tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn fingerprint_walk_ignores_symlinks_and_is_bounded() {
        let dir = std::env::temp_dir().join(format!("mc2-fp-walk-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::write(dir.join("a/x.txt"), b"x").unwrap();
        std::os::unix::fs::symlink("/", dir.join("rootlink")).unwrap();
        let t0 = std::time::Instant::now();
        let mut out = Vec::new();
        loose_files(
            &dir,
            "",
            0,
            device_of(&std::fs::metadata(&dir).unwrap()),
            &mut out,
        );
        assert!(t0.elapsed() < std::time::Duration::from_secs(5));
        assert_eq!(out.len(), 1, "{out:?}");
        // a non-game folder is not walked at all
        let _ = data_fingerprint(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
