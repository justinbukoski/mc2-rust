//! Per-user configuration, save and cache directories for each operating system.
//!
//! | OS | config (`options.cfg`) | saves |
//! |---|---|---|
//! | Windows | `%APPDATA%\mc2` | `%APPDATA%\MechCommander2\saves` |
//! | macOS | `~/Library/Application Support/mc2` | `.../MechCommander2/saves` |
//! | Linux / Steam Deck | `$XDG_CONFIG_HOME/mc2` | `$XDG_DATA_HOME/mechcommander2/saves` |
//!
//! The config folder is `mc2` on every OS, the same folder `mc2.cfg` is read from (`locate.rs`) and
//! the one the player docs name. Before that rule `options.cfg` was kept in the `MechCommander2`
//! folder (`mechcommander2` on Linux); it is still read from there when the new folder has none.
//!
//! The environment variable `MC2_USER_DIR` relocates everything under one root (portable mode,
//! tests).

use std::io;
use std::path::{Path, PathBuf};

/// Environment variable that overrides all user directories.
pub const USER_DIR_ENV: &str = "MC2_USER_DIR";

/// The per-user config folder name, on every OS.
pub const CONFIG_DIR: &str = "mc2";

/// The config file holding the player's options.
pub const OPTIONS_FILE: &str = "options.cfg";

#[cfg(any(target_os = "windows", target_os = "macos"))]
const APP_DIR: &str = "MechCommander2";
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const APP_DIR: &str = "mechcommander2";

/// `<base>/APP_DIR`, or the already-final `config` directory when the OS has no such base.
fn app_dir_under(base: Option<PathBuf>, config: &Path) -> PathBuf {
    base.map_or_else(|| config.to_path_buf(), |d| d.join(APP_DIR))
}

/// The config folder under the OS's config base (`%APPDATA%`, `~/Library/Application Support`,
/// `$XDG_CONFIG_HOME`).
pub fn config_dir_at(config_base: &Path) -> PathBuf {
    config_base.join(CONFIG_DIR)
}

/// The `options.cfg` to read for a config folder: the one in `config`, else the copy the old
/// `MechCommander2` folder holds next to it (only when `config` is the `mc2` folder of a real OS
/// config base). Writes always go to `config`.
pub fn options_read_path(config: &Path) -> PathBuf {
    let own = config.join(OPTIONS_FILE);
    if own.is_file() || config.file_name() != Some(std::ffi::OsStr::new(CONFIG_DIR)) {
        return own;
    }
    let Some(base) = config.parent() else {
        return own;
    };
    let legacy = base.join(APP_DIR).join(OPTIONS_FILE);
    if legacy.is_file() {
        legacy
    } else {
        own
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserPaths {
    /// Preferences (`prefs.cfg`, options, key bindings).
    pub config: PathBuf,
    /// Campaign progress and mission saves.
    pub saves: PathBuf,
    /// Regenerable data (shader caches, converted textures, logs).
    pub cache: PathBuf,
}

impl UserPaths {
    /// Resolve for the current user, honouring `MC2_USER_DIR`. `None` when the OS reports no
    /// home directory.
    pub fn resolve() -> Option<Self> {
        if let Some(root) = std::env::var_os(USER_DIR_ENV).filter(|v| !v.is_empty()) {
            return Some(Self::under(Path::new(&root)));
        }
        let config = config_dir_at(&dirs::config_dir()?);
        let data = app_dir_under(dirs::data_dir(), &config);
        let cache = app_dir_under(dirs::cache_dir(), &config);
        Some(Self {
            config,
            saves: data.join("saves"),
            cache,
        })
    }

    /// Everything below a single root.
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            saves: root.join("saves"),
            cache: root.join("cache"),
        }
    }

    /// Create the directories if missing.
    pub fn ensure(&self) -> io::Result<()> {
        for d in [&self.config, &self.saves, &self.cache] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn under_layout_and_ensure() {
        let root = std::env::temp_dir().join(format!("mc2-paths-test-{}", std::process::id()));
        let p = UserPaths::under(&root);
        assert_eq!(p.config, root.join("config"));
        p.ensure().unwrap();
        assert!(p.saves.is_dir() && p.cache.is_dir() && p.config.is_dir());
        // Idempotent.
        p.ensure().unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn fallback_does_not_duplicate_app_dir() {
        let config = PathBuf::from("/home/u/.config").join(APP_DIR);
        assert_eq!(app_dir_under(None, &config), config);
        assert_eq!(
            app_dir_under(Some(PathBuf::from("/home/u/.local/share")), &config),
            PathBuf::from("/home/u/.local/share").join(APP_DIR)
        );
    }

    #[test]
    fn config_folder_is_mc2_on_every_os() {
        let base = Path::new("base");
        assert_eq!(CONFIG_DIR, "mc2");
        assert_eq!(config_dir_at(base), base.join("mc2"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_config_folder_is_appdata_mc2() {
        let appdata = PathBuf::from(r"C:\Users\u\AppData\Roaming");
        assert_eq!(
            config_dir_at(&appdata),
            PathBuf::from(r"C:\Users\u\AppData\Roaming\mc2")
        );
    }

    #[test]
    fn legacy_folder_names_are_the_old_app_dir() {
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        assert_eq!(APP_DIR, "MechCommander2");
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        assert_eq!(APP_DIR, "mechcommander2");
    }

    #[test]
    fn options_read_falls_back_to_the_old_folder_only_for_the_mc2_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("target")
            .join("tmp")
            .join(format!("mc2-options-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let base = root.join("base");
        let config = config_dir_at(&base);
        let legacy = base.join(APP_DIR);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(OPTIONS_FILE), b"old").unwrap();

        // Nothing in the mc2 folder yet: the old copy is read (a write still goes to `config`).
        assert_eq!(options_read_path(&config), legacy.join(OPTIONS_FILE));

        // Once the mc2 folder has options, they win.
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(OPTIONS_FILE), b"new").unwrap();
        assert_eq!(options_read_path(&config), config.join(OPTIONS_FILE));

        // A portable (`MC2_USER_DIR`) config folder is not the mc2 folder: no fallback there.
        let portable = base.join("config");
        assert_eq!(options_read_path(&portable), portable.join(OPTIONS_FILE));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn resolve_default_is_per_user_and_named() {
        // Avoid touching the environment (tests run in parallel); just check the default branch
        // when the override is unset.
        if std::env::var_os(USER_DIR_ENV).is_none() {
            if let Some(p) = UserPaths::resolve() {
                assert!(p.config.ends_with(CONFIG_DIR));
                assert!(p.saves.ends_with("saves"));
                assert!(p.cache.ends_with(APP_DIR));
                assert!(p.config.is_absolute());
            }
        }
    }
}
