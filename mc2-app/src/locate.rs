//! Where the player's own game data and the optional upscaled texture pack are found.
//!
//! The original data is copyrighted and never shipped with a release package.  Lookup order,
//! first hit wins: the command-line flag, the environment variable (`MC2_DATA`, `MC2_HIRES_DIR`, `MC2_VOICE_DIR`),
//! then a config file `mc2.cfg` next to the executable, then `mc2.cfg` in the platform config
//! directory.  The file holds `key = value` lines (`data`, `hires`, `voices`); `#` starts a comment and a
//! relative path is relative to the file's directory.

use mc2_platform::paths::CONFIG_DIR;
use std::path::{Path, PathBuf};

/// The config file name.
pub const CONFIG_NAME: &str = "mc2.cfg";

/// The `data` / `hires` / `voices` entries of one config file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigFile {
    pub data: Option<PathBuf>,
    pub hires: Option<Hires>,
    pub voices: Option<PathBuf>,
    pub art: Option<ArtSetting>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArtSetting {
    Off,
    Dir(PathBuf),
}

/// The `hires` setting: `off` keeps the original art, a folder names a ready-made pack, and
/// with nothing set the app builds its own pack from the player's data (see [`crate::hires`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hires {
    Auto,
    Off,
    Dir(PathBuf),
}

impl Hires {
    /// `off`/`none`/`false` (any case) is [`Hires::Off`], `auto` is [`Hires::Auto`], else a folder.
    pub fn parse(v: &str) -> Hires {
        match v.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "no" => Hires::Off,
            "auto" | "" => Hires::Auto,
            _ => Hires::Dir(PathBuf::from(v.trim())),
        }
    }
}

impl ConfigFile {
    /// Parses `text`; `base` is the directory relative paths are resolved against.
    pub fn parse(text: &str, base: &Path) -> Result<ConfigFile, String> {
        let mut c = ConfigFile::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected key = value", n + 1))?;
            let v = v.trim().trim_matches('"');
            let p = if v.is_empty() {
                None
            } else {
                let p = PathBuf::from(v);
                Some(if p.is_absolute() { p } else { base.join(p) })
            };
            match k.trim() {
                "data" => c.data = p,
                "hires" => {
                    c.hires = match Hires::parse(v) {
                        Hires::Dir(_) => p.map(Hires::Dir),
                        other => Some(other),
                    }
                }
                "voices" => c.voices = p,
                // An explicit disable must shadow later config files.
                "art" if matches!(v.to_ascii_lowercase().as_str(), "off" | "none" | "0") => {
                    c.art = Some(ArtSetting::Off)
                }
                "art" => c.art = p.map(ArtSetting::Dir),
                other => return Err(format!("line {}: unknown key {other}", n + 1)),
            }
        }
        Ok(c)
    }
}

/// The places a config file is looked for, in order.
pub fn config_candidates(
    exe_dir: Option<&Path>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = exe_dir {
        v.push(d.join(CONFIG_NAME));
        // Inside a macOS bundle (`X.app/Contents/MacOS`) also look next to the `.app`.
        if d.ends_with("Contents/MacOS") {
            if let Some(app_parent) = d.ancestors().nth(3) {
                v.push(app_parent.join(CONFIG_NAME));
            }
        }
    }
    let path = |k: &str| env(k).filter(|s| !s.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "windows") {
        if let Some(d) = path("APPDATA") {
            v.push(d.join(CONFIG_DIR).join(CONFIG_NAME));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(d) = path("HOME") {
            v.push(
                d.join("Library")
                    .join("Application Support")
                    .join(CONFIG_DIR)
                    .join(CONFIG_NAME),
            );
        }
    } else if let Some(d) =
        path("XDG_CONFIG_HOME").or_else(|| path("HOME").map(|h| h.join(".config")))
    {
        v.push(d.join(CONFIG_DIR).join(CONFIG_NAME));
    }
    v
}

/// Reads the first config file that sets `key` among `candidates`.
fn read_config_key<T: Clone>(
    candidates: &[PathBuf],
    key: fn(&ConfigFile) -> Option<&T>,
) -> Result<Option<T>, String> {
    for p in candidates {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let base = p.parent().unwrap_or_else(|| Path::new("."));
        let cfg = ConfigFile::parse(&text, base).map_err(|e| format!("{}: {e}", p.display()))?;
        if let Some(value) = key(&cfg) {
            return Ok(Some(value.clone()));
        }
    }
    Ok(None)
}

/// The text shown when no game data can be found.
pub fn missing_data_message(candidates: &[PathBuf]) -> String {
    let mut m = format!(
        "{} game data not found.\n\
         This package does not include the game's data files; point it at your own copy\n\
         (the folder holding system.cfg and the .fst archives), in any one of these ways:\n\
         \x20 1. mc2 --data \"/path/to/mc2exe\"\n\
         \x20 2. set the MC2_DATA environment variable to that folder\n\
         \x20 3. put a line  data = /path/to/mc2exe  in a file named mc2.cfg at one of:\n",
        mc2_platform::PRODUCT_NAME
    );
    for c in candidates {
        m.push_str(&format!("      {}\n", c.display()));
    }
    m
}

/// Resolves the data directory: `flag`, `$MC2_DATA`, then the config file.  Errors carry the
/// first-run message.
pub fn resolve_data(
    flag: Option<&Path>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    candidates: &[PathBuf],
) -> Result<PathBuf, String> {
    if let Some(p) = flag {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = env("MC2_DATA").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    if let Some(d) = read_config_key(candidates, |c| c.data.as_ref())? {
        return Ok(d);
    }
    Err(missing_data_message(candidates))
}

/// Resolves the texture pack setting: `flag`, `$MC2_HIRES_DIR`, then the config file; with
/// nothing set the app builds its own pack ([`Hires::Auto`]).
pub fn resolve_hires(
    flag: Option<&Hires>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    candidates: &[PathBuf],
) -> Result<Hires, String> {
    if let Some(h) = flag {
        return Ok(h.clone());
    }
    // an explicit `off` in the config file beats the environment pack too
    let cfg = read_config_key(candidates, |c| c.hires.as_ref())?;
    if cfg == Some(Hires::Off) {
        return Ok(Hires::Off);
    }
    if let Some(p) = env("MC2_HIRES_DIR").filter(|s| !s.is_empty()) {
        return Ok(Hires::parse(&p.to_string_lossy()));
    }
    Ok(cfg.unwrap_or(Hires::Auto))
}

/// The folder the built pack lives in: the platform cache directory, under `mc2/hires`.
pub fn cache_dir(env: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = |k: &str| env(k).filter(|s| !s.is_empty()).map(PathBuf::from);
    let root = if cfg!(target_os = "windows") {
        path("LOCALAPPDATA").or_else(|| path("APPDATA"))?
    } else if cfg!(target_os = "macos") {
        path("HOME")?.join("Library").join("Caches")
    } else {
        path("XDG_CACHE_HOME").or_else(|| path("HOME").map(|h| h.join(".cache")))?
    };
    Some(root.join("mc2").join("hires"))
}

/// Resolves the optional original art pack: `flag`, `$MC2_ART_DIR`, then the config file
/// (`art =`); `None` leaves the original models.
pub fn resolve_art(
    flag: Option<&Path>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    candidates: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if let Some(p) = flag {
        return Ok(Some(p.to_path_buf()));
    }
    if let Some(p) = env("MC2_ART_DIR").filter(|s| !s.is_empty()) {
        return Ok(Some(PathBuf::from(p)));
    }
    for p in candidates {
        let Ok(text) = std::fs::read_to_string(p) else {
            continue;
        };
        let base = p.parent().unwrap_or_else(|| Path::new("."));
        let cfg = ConfigFile::parse(&text, base).map_err(|e| format!("{}: {e}", p.display()))?;
        if let Some(setting) = cfg.art {
            return Ok(match setting {
                ArtSetting::Off => None,
                ArtSetting::Dir(path) => Some(path),
            });
        }
    }
    Ok(None)
}

/// Resolves the optional pilot voice pack: `flag`, `$MC2_VOICE_DIR`, then the config file; `None` leaves the
/// original pilot voices.
pub fn resolve_voices(
    flag: Option<&Path>,
    env: &dyn Fn(&str) -> Option<std::ffi::OsString>,
    candidates: &[PathBuf],
) -> Result<Option<PathBuf>, String> {
    if let Some(p) = flag {
        return Ok(Some(p.to_path_buf()));
    }
    if let Some(p) = env("MC2_VOICE_DIR").filter(|s| !s.is_empty()) {
        return Ok(Some(PathBuf::from(p)));
    }
    read_config_key(candidates, |c| c.voices.as_ref())
}

/// The directory of the running executable.
pub fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// The version line: package version plus the build's commit when `MC2_BUILD_SHA` was set at
/// compile time (the packaging scripts set it).
pub fn version_string() -> String {
    let v = env!("CARGO_PKG_VERSION");
    match option_env!("MC2_BUILD_SHA") {
        Some(s) if !s.is_empty() => format!("mc2 {v} ({s})"),
        _ => format!("mc2 {v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn config_parse() {
        let c = ConfigFile::parse(
            "# c\ndata = \"/g/mc2exe\"\nhires = pack # rel\n",
            Path::new("/cfg"),
        )
        .unwrap();
        assert_eq!(c.data.as_deref(), Some(Path::new("/g/mc2exe")));
        assert_eq!(c.hires, Some(Hires::Dir(PathBuf::from("/cfg/pack"))));
        let v = ConfigFile::parse("voices = vp\n", Path::new("/cfg")).unwrap();
        assert_eq!(v.voices.as_deref(), Some(Path::new("/cfg/vp")));
        assert!(ConfigFile::parse("bogus", Path::new("/")).is_err());
        assert!(ConfigFile::parse("x = 1", Path::new("/")).is_err());
    }

    #[test]
    fn data_order_flag_env_file_message() {
        let dir = std::env::temp_dir().join(format!("mc2-locate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join(CONFIG_NAME);
        std::fs::write(&cfg, "data = from_file\nhires = hp\n").unwrap();
        let cands = vec![cfg];
        let env = |k: &str| (k == "MC2_DATA").then(|| OsString::from("/env"));
        assert_eq!(
            resolve_data(Some(Path::new("/flag")), &env, &cands).unwrap(),
            PathBuf::from("/flag")
        );
        assert_eq!(
            resolve_data(None, &env, &cands).unwrap(),
            PathBuf::from("/env")
        );
        assert_eq!(
            resolve_data(None, &no_env, &cands).unwrap(),
            dir.join("from_file")
        );
        assert_eq!(
            resolve_hires(None, &no_env, &cands).unwrap(),
            Hires::Dir(dir.join("hp"))
        );
        let e = resolve_data(None, &no_env, &[dir.join("absent.cfg")]).unwrap_err();
        assert!(e.contains("--data") && e.contains("MC2_DATA") && e.contains("absent.cfg"));
        assert_eq!(resolve_hires(None, &no_env, &[]).unwrap(), Hires::Auto);
        let off = |k: &str| (k == "MC2_HIRES_DIR").then(|| OsString::from("off"));
        assert_eq!(resolve_hires(None, &off, &cands).unwrap(), Hires::Off);
        assert_eq!(
            resolve_hires(Some(&Hires::Off), &no_env, &cands).unwrap(),
            Hires::Off
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn art_order_flag_env_file() {
        let dir = std::env::temp_dir().join(format!("mc2-locate-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cfg = dir.join(CONFIG_NAME);
        std::fs::write(&cfg, "art = pack\n").unwrap();
        let cands = vec![cfg];
        let env = |k: &str| (k == "MC2_ART_DIR").then(|| OsString::from("/env"));
        assert_eq!(
            resolve_art(Some(Path::new("/flag")), &env, &cands).unwrap(),
            Some(PathBuf::from("/flag"))
        );
        assert_eq!(
            resolve_art(None, &env, &cands).unwrap(),
            Some(PathBuf::from("/env"))
        );
        assert_eq!(
            resolve_art(None, &no_env, &cands).unwrap(),
            Some(dir.join("pack"))
        );
        assert_eq!(resolve_art(None, &no_env, &[]).unwrap(), None);
        // the config value alone reaches the render settings; `off` restores the originals
        let rc = mc2_gfx::config::RenderConfig {
            art_dir: resolve_art(None, &no_env, &cands).unwrap(),
            ..Default::default()
        };
        let gs =
            mc2_render::settings::GraphicsSettings::from_render_config_with(&rc, |_| None).unwrap();
        assert_eq!(gs.art_dir, Some(dir.join("pack")));
        std::fs::write(&cands[0], "art = off\n").unwrap();
        assert_eq!(resolve_art(None, &no_env, &cands).unwrap(), None);
        let later = dir.join("later.cfg");
        std::fs::write(&cands[0], "art = off\n").unwrap();
        std::fs::write(&later, "art = pack\n").unwrap();
        assert_eq!(
            resolve_art(None, &no_env, &[cands[0].clone(), later]).unwrap(),
            None,
            "an explicit off in an earlier config shadows later config sources"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn candidates_start_next_to_the_executable() {
        let c = config_candidates(Some(Path::new("/app")), &no_env);
        assert_eq!(c[0], Path::new("/app").join(CONFIG_NAME));
        let b = config_candidates(Some(Path::new("/Games/MC2.app/Contents/MacOS")), &no_env);
        assert_eq!(b[1], Path::new("/Games").join(CONFIG_NAME));
    }

    #[test]
    fn user_config_candidate_is_the_mc2_folder_the_docs_name() {
        let env = |k: &str| match k {
            "APPDATA" => Some(OsString::from("APPDATA_DIR")),
            "HOME" => Some(OsString::from("HOME_DIR")),
            "XDG_CONFIG_HOME" => Some(OsString::from("XDG_DIR")),
            _ => None,
        };
        let last = config_candidates(None, &env)
            .pop()
            .expect("a user candidate");
        let want = if cfg!(target_os = "windows") {
            Path::new("APPDATA_DIR").join("mc2")
        } else if cfg!(target_os = "macos") {
            Path::new("HOME_DIR")
                .join("Library")
                .join("Application Support")
                .join("mc2")
        } else {
            Path::new("XDG_DIR").join("mc2")
        };
        assert_eq!(last, want.join(CONFIG_NAME));
        // `options.cfg` lives in the same folder (`%APPDATA%\\mc2` on Windows).
        assert_eq!(
            mc2_platform::paths::config_dir_at(Path::new("APPDATA_DIR")),
            Path::new("APPDATA_DIR").join("mc2")
        );
    }

    #[test]
    fn each_config_key_uses_its_first_setting_across_files() {
        let dir = std::env::temp_dir().join(format!("mc2-locate-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.cfg");
        let second = dir.join("second.cfg");
        let no_env = |_: &str| None;

        std::fs::write(&first, "data = game-a\n").unwrap();
        std::fs::write(&second, "data = game-b\nhires = pack-b\n").unwrap();
        let candidates = vec![first.clone(), second.clone()];
        assert_eq!(
            resolve_data(None, &no_env, &candidates).unwrap(),
            dir.join("game-a")
        );
        assert_eq!(
            resolve_hires(None, &no_env, &candidates).unwrap(),
            Hires::Dir(dir.join("pack-b"))
        );

        std::fs::write(&first, "hires = pack-a\n").unwrap();
        std::fs::write(&second, "data = game-b\nhires = pack-b\n").unwrap();
        assert_eq!(
            resolve_data(None, &no_env, &candidates).unwrap(),
            dir.join("game-b")
        );
        assert_eq!(
            resolve_hires(None, &no_env, &candidates).unwrap(),
            Hires::Dir(dir.join("pack-a"))
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hires_off_in_the_config_file_and_cache_dir() {
        let c = ConfigFile::parse("hires = OFF\n", Path::new("/cfg")).unwrap();
        assert_eq!(c.hires, Some(Hires::Off));
        let env = |k: &str| {
            (k == "HOME" || k == "XDG_CACHE_HOME" || k == "LOCALAPPDATA")
                .then(|| OsString::from("/h"))
        };
        let d = cache_dir(&env).unwrap();
        assert!(d.starts_with("/h") && d.ends_with("mc2/hires"));
        assert!(cache_dir(&no_env).is_none());
    }

    #[test]
    fn version_names_the_package() {
        assert!(version_string().starts_with("mc2 "));
    }
}
