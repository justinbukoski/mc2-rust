//! Upscaled art by default: the pack is built from the player's own data on first run, a damaged
//! or partial pack is rebuilt, `off` keeps the original art.  The fixture is a tiny data folder
//! (one fast file with two interface textures); no game data, GPU or window is needed.

use mc2_app::hires::{prepare, Build, Choice};
use mc2_app::locate::Hires;
use mc2_formats::fst::{elf_hash, FST_VERSION_ZLIB};
use mc2_gfx::texture as tx;
use mc2_texpack::builtin::{self, PackStatus};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const ART: [&str; 2] = ["data/art/mcl_test_a.tga", "data/art/mcl_test_b.tga"];

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mc2_hires_default_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// 8x8 uncompressed 32-bit top-left TGA with a horizontal gradient.
fn tga(seed: u8) -> Vec<u8> {
    let mut f = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 0, 8, 0, 32, 0x28];
    for _ in 0..8 {
        for x in 0..8u8 {
            f.extend_from_slice(&[x * 30, seed, 255 - x * 30, 255]);
        }
    }
    f
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

fn fixture_data(dir: &Path, seed: u8) {
    let files: Vec<(&str, Vec<u8>)> = ART.iter().map(|n| (*n, tga(seed))).collect();
    fixture_files(dir, files);
}

fn fixture_files(dir: &Path, files: Vec<(&str, Vec<u8>)>) {
    let mut at = (8 + files.len() * 266) as u32;
    let (mut table, mut payload) = (Vec::new(), Vec::new());
    for (name, raw) in &files {
        let packed = zlib(raw);
        table.extend_from_slice(&at.to_le_bytes());
        table.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        table.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        table.extend_from_slice(&elf_hash(name.as_bytes()).to_le_bytes());
        let mut field = [0u8; 250];
        field[..name.len()].copy_from_slice(name.as_bytes());
        table.extend_from_slice(&field);
        at += packed.len() as u32;
        payload.extend(packed);
    }
    let mut img = FST_VERSION_ZLIB.to_le_bytes().to_vec();
    img.extend_from_slice(&(files.len() as u32).to_le_bytes());
    img.extend(table);
    img.extend(payload);
    std::fs::write(dir.join("fixture.fst"), img).unwrap();
    std::fs::write(
        dir.join("system.cfg"),
        "FITini \r\n[FastFiles]\r\nl NumFastFiles = 1\r\nst File0=\"fixture.fst\"\r\nFITend \r\n",
    )
    .unwrap();
}

fn key(name: &str) -> String {
    tx::texture_key(Some(name), &[], 8, 8, 0)
}

fn auto(data: &Path, pack: &Path, build: Build) -> (Choice, Vec<String>) {
    let mut log = Vec::new();
    let c = prepare(&Hires::Auto, data, Some(pack), build, &mut |m| {
        log.push(m.to_owned())
    });
    (c, log)
}

#[test]
fn first_run_builds_the_pack_and_the_renderer_hook_serves_it() {
    let root = scratch("first");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);

    let (c, log) = auto(&data, &pack, Build::Allowed(None));
    assert_eq!(c.dir.as_deref(), Some(pack.as_path()), "{log:?}");
    assert_eq!(
        builtin::pack_status(&pack, &builtin::data_fingerprint(&data)),
        PackStatus::Valid
    );
    for n in ART {
        // exactly what the renderer's texture hook asks for
        let r = tx::load_replacement(&pack, &key(n)).unwrap_or_else(|| panic!("{n} not served"));
        assert_eq!((r.w, r.h), (16, 16));
    }
    // second start reuses it without building or printing
    let (again, log) = auto(&data, &pack, Build::Allowed(None));
    assert_eq!(again, c);
    assert!(log.is_empty(), "{log:?}");
    // the build left no work files behind
    assert!(!pack.join(".work").exists());
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn explicit_off_keeps_original_art_and_never_builds() {
    let root = scratch("off");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    let c = prepare(
        &Hires::Off,
        &data,
        Some(&pack),
        Build::Allowed(None),
        &mut |_| {},
    );
    assert_eq!(c.dir, None);
    assert!(!pack.exists());
    // Build::Never (self-test, headless) does not build either, and uses a finished pack
    let (c, _) = auto(&data, &pack, Build::Never);
    assert_eq!(c.dir, None);
    assert!(!pack.exists());
    auto(&data, &pack, Build::Allowed(None));
    let (c, _) = auto(&data, &pack, Build::Never);
    assert_eq!(c.dir.as_deref(), Some(pack.as_path()));
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn corrupt_partial_and_stale_packs_are_rebuilt() {
    let root = scratch("repair");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    auto(&data, &pack, Build::Allowed(None));
    let fp = builtin::data_fingerprint(&data);
    let file = tx::replacement_path(&pack, &key(ART[0]));
    let good = std::fs::read(&file).unwrap();

    // truncated file
    std::fs::write(&file, &good[..good.len() / 2]).unwrap();
    assert!(matches!(
        builtin::pack_status(&pack, &fp),
        PackStatus::Corrupt(_)
    ));
    let (c, log) = auto(&data, &pack, Build::Allowed(None));
    assert!(
        c.dir.is_some() && log.iter().any(|l| l.contains("damaged")),
        "{log:?}"
    );
    assert_eq!(std::fs::read(&file).unwrap(), good, "rebuilt identically");

    // missing file
    std::fs::remove_file(&file).unwrap();
    auto(&data, &pack, Build::Allowed(None));
    assert_eq!(std::fs::read(&file).unwrap(), good);

    // interrupted build: no marker (as after a kill); it is continued, never trusted
    std::fs::remove_file(pack.join(builtin::MARKER)).unwrap();
    assert_eq!(builtin::pack_status(&pack, &fp), PackStatus::Missing);
    let (c, _) = auto(&data, &pack, Build::Never);
    assert_eq!(
        c.dir, None,
        "an unfinished pack is not used by tools that never build"
    );
    auto(&data, &pack, Build::Allowed(None));
    assert_eq!(builtin::pack_status(&pack, &fp), PackStatus::Valid);

    // different game data: stale, rebuilt from the new data
    fixture_data(&data, 200);
    let fp2 = builtin::data_fingerprint(&data);
    assert_ne!(fp, fp2);
    assert_eq!(builtin::pack_status(&pack, &fp2), PackStatus::Stale);
    auto(&data, &pack, Build::Allowed(None));
    assert_eq!(builtin::pack_status(&pack, &fp2), PackStatus::Valid);
    assert_ne!(std::fs::read(&file).unwrap(), good);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_spent_budget_starts_the_game_with_a_partial_pack_and_resumes_later() {
    let root = scratch("budget");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    let fp = builtin::data_fingerprint(&data);
    // a zero budget stops before the first texture: nothing usable yet, original art
    let (c, log) = auto(&data, &pack, Build::Allowed(Some(Duration::ZERO)));
    assert_eq!(c.dir, None, "{log:?}");
    if let Some(bg) = &c.background {
        bg.wait();
    }
    // the next start carries on and finishes
    let (c, _) = auto(&data, &pack, Build::Allowed(None));
    assert!(c.dir.is_some());
    assert_eq!(builtin::pack_status(&pack, &fp), PackStatus::Valid);
    std::fs::remove_dir_all(&root).unwrap();
}

// ---- review 2 blockers -------------------------------------------------------------------

fn pixels(pack: &Path, name: &str) -> Vec<u8> {
    std::fs::read(tx::replacement_path(pack, &key(name))).unwrap()
}

#[test]
fn explicit_off_beats_the_environment_pack_in_both_front_ends() {
    use mc2_app::headless::{base_render_config as headless_cfg, HeadlessOptions};
    use mc2_app::locate::resolve_hires;
    use mc2_app::windowed::base_render_config as windowed_cfg;
    use mc2_gfx::config::RenderConfig;
    let env_pack = PathBuf::from("/from/MC2_HIRES_DIR");
    let env_cfg = || RenderConfig {
        hires_dir: Some(env_pack.clone()),
        ..RenderConfig::default()
    };
    // `--hires-dir off` through the production resolution, prepare and config path
    let setting = resolve_hires(Some(&Hires::Off), &|_| None, &[]).unwrap();
    let choice = prepare(
        &setting,
        Path::new("/none"),
        None,
        Build::Never,
        &mut |_| {},
    );
    assert_eq!(choice.dir, None);
    assert_eq!(
        windowed_cfg(choice.dir.as_deref(), env_cfg()).hires_dir,
        None
    );
    let mut opts = HeadlessOptions::new(PathBuf::from("/none"), "m");
    opts.hires_dir = choice.dir.clone();
    opts.hires_pinned = true;
    assert_eq!(headless_cfg(&opts, env_cfg()).hires_dir, None);
    // a chosen pack still wins over the environment, and an unpinned tool leaves the env alone
    let pack = PathBuf::from("/chosen");
    assert_eq!(
        windowed_cfg(Some(&pack), env_cfg()).hires_dir,
        Some(pack.clone())
    );
    let unpinned = HeadlessOptions::new(PathBuf::from("/none"), "m");
    assert_eq!(headless_cfg(&unpinned, env_cfg()).hires_dir, Some(env_pack));
}

#[test]
fn the_start_up_budget_covers_reading_the_textures() {
    let root = scratch("collect_budget");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    let fp = builtin::data_fingerprint(&data);
    let t0 = std::time::Instant::now();
    let (c, log) = auto(&data, &pack, Build::Allowed(Some(Duration::ZERO)));
    assert_eq!(c.dir, None, "pack stays off this run: {log:?}");
    assert!(t0.elapsed() < Duration::from_secs(5));
    // reading went on in the background and, once done, built the whole pack without another start
    if let Some(bg) = &c.background {
        bg.wait();
        assert_eq!(builtin::pack_status(&pack, &fp), PackStatus::Valid);
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn the_fingerprint_covers_loose_overrides_and_same_length_archive_changes() {
    let root = scratch("fingerprint");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    auto(&data, &pack, Build::Allowed(None));
    let fp = builtin::data_fingerprint(&data);
    let before = pixels(&pack, ART[0]);

    // a loose file that overrides an archived texture
    std::fs::create_dir_all(data.join("data/art")).unwrap();
    std::fs::write(data.join(ART[0]), tga(220)).unwrap();
    assert_ne!(builtin::data_fingerprint(&data), fp);
    assert_eq!(
        builtin::pack_status(&pack, &builtin::data_fingerprint(&data)),
        PackStatus::Stale
    );
    auto(&data, &pack, Build::Allowed(None));
    assert_ne!(
        pixels(&pack, ART[0]),
        before,
        "the override is what is upscaled"
    );
    std::fs::remove_file(data.join(ART[0])).unwrap();

    // an archive whose bytes change but not its length, with its modification time put back
    auto(&data, &pack, Build::Allowed(None));
    let fst = data.join("fixture.fst");
    let (len, mtime) = {
        let m = std::fs::metadata(&fst).unwrap();
        (m.len(), m.modified().unwrap())
    };
    let fp = builtin::data_fingerprint(&data);
    let mut bytes = std::fs::read(&fst).unwrap();
    let last = bytes.len() - 3;
    bytes[last] ^= 0x01;
    std::fs::write(&fst, bytes).unwrap();
    let f = std::fs::OpenOptions::new().write(true).open(&fst).unwrap();
    f.set_modified(mtime).unwrap();
    assert_eq!(std::fs::metadata(&fst).unwrap().len(), len, "same length");
    assert_ne!(builtin::data_fingerprint(&data), fp);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_resumed_build_never_certifies_pngs_made_from_other_data() {
    let root = scratch("resume_stale");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    let fresh = root.join("fresh/hires");
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    auto(&data, &pack, Build::Allowed(None));
    // interrupted: PNGs exist, the marker does not
    std::fs::remove_file(pack.join(builtin::MARKER)).unwrap();
    // the data changes before the next start (same dimensions)
    fixture_data(&data, 200);
    auto(&data, &pack, Build::Allowed(None));
    auto(&data, &fresh, Build::Allowed(None));
    for n in ART {
        assert_eq!(
            pixels(&pack, n),
            pixels(&fresh, n),
            "{n} must come from the new data"
        );
    }
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn a_png_with_a_good_header_and_length_but_a_damaged_body_is_not_valid() {
    let root = scratch("png_body");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    auto(&data, &pack, Build::Allowed(None));
    let fp = builtin::data_fingerprint(&data);
    let file = tx::replacement_path(&pack, &key(ART[0]));
    let good = std::fs::read(&file).unwrap();
    let damage = |bytes: &[u8]| {
        let mut b = bytes.to_vec();
        let mid = b.len() / 2;
        b[mid] ^= 0x55;
        b
    };
    std::fs::write(&file, damage(&good)).unwrap();
    assert!(matches!(
        builtin::pack_status(&pack, &fp),
        PackStatus::Corrupt(_)
    ));
    auto(&data, &pack, Build::Allowed(None));
    assert_eq!(std::fs::read(&file).unwrap(), good);
    // and a damaged file left by an interrupted build is redone, not kept
    std::fs::remove_file(pack.join(builtin::MARKER)).unwrap();
    std::fs::write(&file, damage(&good)).unwrap();
    auto(&data, &pack, Build::Allowed(None));
    assert_eq!(std::fs::read(&file).unwrap(), good);
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn unreadable_textures_do_not_make_a_pack_that_is_called_complete() {
    let root = scratch("failures");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    let mut files: Vec<(&str, Vec<u8>)> = ART.iter().map(|n| (*n, tga(40))).collect();
    files.push(("data/art/mcl_test_bad.tga", b"not a tga at all".to_vec()));
    fixture_files(&data, files);
    let fp = builtin::data_fingerprint(&data);
    let (c, _) = auto(&data, &pack, Build::Allowed(None));
    assert!(c.dir.is_some(), "what could be read is served");
    assert!(
        matches!(builtin::pack_status(&pack, &fp), PackStatus::Incomplete(n) if n > 0),
        "{:?}",
        builtin::pack_status(&pack, &fp)
    );
    let (c, _) = auto(&data, &pack, Build::Never);
    assert!(c.dir.is_some());
    // the next start tries again instead of trusting the marker
    let (_, log) = auto(&data, &pack, Build::Allowed(None));
    assert!(log.iter().any(|l| l.contains("trying again")), "{log:?}");
    std::fs::remove_dir_all(&root).unwrap();
}

// ---- review 3 blockers -------------------------------------------------------------------

#[test]
fn config_file_off_beats_the_environment_pack_in_both_front_ends() {
    use mc2_app::headless::{base_render_config as headless_cfg, HeadlessOptions};
    use mc2_app::locate::resolve_hires;
    use mc2_app::windowed::base_render_config as windowed_cfg;
    use mc2_gfx::config::RenderConfig;
    let root = scratch("cfg_off");
    let cfg = root.join("mc2.cfg");
    std::fs::write(&cfg, "hires = off\n").unwrap();
    let env = |k: &str| (k == "MC2_HIRES_DIR").then(|| std::ffi::OsString::from("/env/pack"));
    let setting = resolve_hires(None, &env, std::slice::from_ref(&cfg)).unwrap();
    assert_eq!(setting, Hires::Off);
    let choice = prepare(
        &setting,
        Path::new("/none"),
        None,
        Build::Never,
        &mut |_| {},
    );
    let env_cfg = || RenderConfig {
        hires_dir: Some(PathBuf::from("/env/pack")),
        ..RenderConfig::default()
    };
    assert_eq!(
        windowed_cfg(choice.dir.as_deref(), env_cfg()).hires_dir,
        None
    );
    let mut opts = HeadlessOptions::new(PathBuf::from("/none"), "m");
    opts.hires_dir = choice.dir.clone();
    opts.hires_pinned = true;
    assert_eq!(headless_cfg(&opts, env_cfg()).hires_dir, None);
    // without `off` in the file the environment pack still applies
    std::fs::write(&cfg, "hires = mine\n").unwrap();
    assert_eq!(
        resolve_hires(None, &env, std::slice::from_ref(&cfg)).unwrap(),
        Hires::Dir(PathBuf::from("/env/pack"))
    );
    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn slow_storage_cannot_hold_the_start_past_the_budget_and_the_pack_still_completes() {
    use mc2_texpack::builtin::{ensure_pack, BuildOptions, BuildOutcome};
    let root = scratch("slow");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    fixture_data(&data, 40);
    let opts = BuildOptions {
        budget: Some(Duration::from_millis(50)),
        // every megabyte read (archives, loose files) takes this long: fingerprinting alone
        // needs well over a second
        read_delay: Duration::from_millis(600),
    };
    let t = std::time::Instant::now();
    let out = ensure_pack(&data, &pack, &opts, &mut |_| {}).unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "returned after {:?}",
        t.elapsed()
    );
    let BuildOutcome::Collecting(bg) = out else {
        panic!("expected the work to carry on in the background, got {out:?}");
    };
    bg.wait();
    assert_eq!(
        builtin::pack_status(&pack, &builtin::data_fingerprint(&data)),
        PackStatus::Valid
    );
    std::fs::remove_dir_all(&root).unwrap();
}

/// A loose `data/effects/mc2.fx` that exists but cannot be read must not be certified as a
/// complete collection; once readable the next start rebuilds with it.
#[cfg(unix)]
#[test]
fn an_unreadable_effect_library_is_an_incomplete_collection_and_is_retried() {
    use std::os::unix::fs::PermissionsExt;
    let root = scratch("fx");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(data.join("data/effects")).unwrap();
    fixture_data(&data, 40);
    let fx = data.join("data/effects/mc2.fx");
    std::fs::write(&fx, b"junk").unwrap();
    std::fs::set_permissions(&fx, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&fx).is_ok() {
        return; // running as root: permissions do not block reads
    }
    let (_, _) = auto(&data, &pack, Build::Allowed(None));
    let marker: builtin::Marker =
        serde_json::from_slice(&std::fs::read(pack.join(builtin::MARKER)).unwrap()).unwrap();
    assert!(
        marker.failures.iter().any(|f| f.contains("effect library")),
        "{:?}",
        marker.failures
    );
    std::fs::set_permissions(&fx, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (_, log) = auto(&data, &pack, Build::Allowed(None));
    // the data identity changed with the readable file: rebuilt, and no longer silently short
    assert!(!log.is_empty(), "{log:?}");
    std::fs::remove_dir_all(&root).unwrap();
}

/// The source-reading preflight (`system.cfg`) must be inside the start-up budget: a
/// `system.cfg` whose read stalls (a FIFO with no writer) cannot hold `prepare` past it.
#[cfg(unix)]
#[test]
fn a_stalled_system_cfg_read_cannot_hold_prepare_past_the_budget() {
    let root = scratch("stalled-cfg");
    let (data, pack) = (root.join("data"), root.join("cache/hires"));
    std::fs::create_dir_all(&data).unwrap();
    let cfg = data.join("system.cfg");
    // no unsafe allowed: make the FIFO with the mkfifo tool
    let ok = std::process::Command::new("mkfifo")
        .arg(&cfg)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "mkfifo failed");
    let t = std::time::Instant::now();
    let mut log = Vec::new();
    let choice = mc2_app::hires::prepare(
        &Hires::Auto,
        &data,
        Some(&pack),
        Build::Allowed(Some(Duration::from_millis(100))),
        &mut |m| log.push(m.to_owned()),
    );
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "returned after {:?}",
        t.elapsed()
    );
    assert_eq!(choice.dir, None);
    assert!(
        choice.background.is_some(),
        "the stalled read must go on in the background"
    );
    // release the reader so the helper thread ends
    drop(std::fs::OpenOptions::new().write(true).open(&cfg));
    choice.background.unwrap().wait();
    std::fs::remove_dir_all(&root).unwrap();
}
