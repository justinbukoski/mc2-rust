//! Upscaled art by default.
//!
//! Microsoft art cannot ship in the package, so the upscaled texture pack is built on the
//! player's machine from the player's own data (`mc2_texpack::builtin`) into the user's cache
//! folder, on the first run and again whenever it is missing, stale for the configured data or
//! damaged.  The build is bounded by a time budget and resumes on the next start; whatever
//! exists is used, and every texture not yet in the pack keeps its original pixels.
//!
//! `--hires-dir off` / `hires = off` keeps the original art; a folder uses a ready-made pack
//! (for example the AI-upscaled one) exactly as before.

use crate::locate::Hires;
use mc2_gfx::config::RenderConfig;
use mc2_texpack::builtin::{self, Background, BuildOptions, BuildOutcome, PackStatus};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Longest the first-run build may hold up the start, in seconds (`$MC2_HIRES_BUDGET_SECS`).
pub const DEFAULT_BUDGET_SECS: u64 = 90;

/// Whether [`prepare`] may build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    /// Build or continue, for at most this long (`None`: until done).
    Allowed(Option<Duration>),
    /// Use a finished pack if there is one; never build (self-test, headless tools).
    Never,
}

/// The outcome: the pack folder the renderer reads, or `None` for the original art.
#[derive(Clone, Debug)]
pub struct Choice {
    pub dir: Option<PathBuf>,
    /// Set when reading the textures outlasted the start-up budget: the pack stays off this run
    /// and the work goes on in the background (dropping this is fine; tests may `wait`).
    pub background: Option<Arc<Background>>,
}

impl PartialEq for Choice {
    fn eq(&self, other: &Self) -> bool {
        self.dir == other.dir
    }
}
impl Eq for Choice {}

impl Choice {
    fn dir(dir: Option<PathBuf>) -> Self {
        Choice {
            dir,
            background: None,
        }
    }
}

/// Puts the chosen pack into the render settings.  `pinned` (the app resolved the setting
/// itself, `off` included) makes `dir` final: an explicit `off` must not fall back to
/// `$MC2_HIRES_DIR`, which `RenderConfig::from_env` has already read into `base`.
pub fn pin_render_config(base: &mut RenderConfig, dir: Option<&Path>, pinned: bool) {
    if pinned || dir.is_some() {
        base.hires_dir = dir.map(Path::to_path_buf);
    }
}

impl Choice {
    /// `off` or the pack folder, for the self-test line.
    pub fn describe(&self) -> String {
        self.dir
            .as_deref()
            .map_or_else(|| "off".to_owned(), |p| p.display().to_string())
    }
}

/// The budget from the environment: unset is [`DEFAULT_BUDGET_SECS`], `0` means unlimited.
pub fn budget_from_env(value: Option<&str>) -> Option<Duration> {
    match value.and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(0) => None,
        Some(s) => Some(Duration::from_secs(s)),
        None => Some(Duration::from_secs(DEFAULT_BUDGET_SECS)),
    }
}

/// Resolves the setting to the pack folder to use.  Never fails: a pack that cannot be built
/// (no cache folder, disk full, unreadable data) means the original art, with a message.
pub fn prepare(
    setting: &Hires,
    data: &Path,
    cache: Option<&Path>,
    build: Build,
    progress: &mut dyn FnMut(&str),
) -> Choice {
    match setting {
        Hires::Off => Choice::dir(None),
        Hires::Dir(d) => Choice::dir(Some(d.clone())),
        Hires::Auto => {
            let Some(pack) = cache else {
                progress("upscaled textures: no cache folder found, using the original art");
                return Choice::dir(None);
            };
            match build {
                // a pack with unreadable textures is still intact and usable
                Build::Never => {
                    // not game data (rejected later, as before): no pack work, no folder walk
                    if mc2_formats::vfs::Vfs::from_system_cfg(data).is_err() {
                        return Choice::dir(None);
                    }
                    let fp = builtin::data_fingerprint(data);
                    let status = builtin::pack_status(pack, &fp);
                    Choice::dir(
                        matches!(status, PackStatus::Valid | PackStatus::Incomplete(_))
                            .then(|| pack.to_path_buf()),
                    )
                }
                // the data check, fingerprinting and validation run inside the budgeted job, not before it
                Build::Allowed(budget) => {
                    let opts = BuildOptions {
                        budget,
                        read_delay: Duration::ZERO,
                    };
                    match builtin::ensure_pack(data, pack, &opts, progress) {
                        Ok(BuildOutcome::Complete { .. }) => Choice::dir(Some(pack.to_path_buf())),
                        Ok(BuildOutcome::NotGameData) => Choice::dir(None),
                        Ok(BuildOutcome::Collecting(bg)) => Choice {
                            dir: None,
                            background: Some(bg),
                        },
                        // a partial pack is served: missing keys fall back to the original
                        Ok(BuildOutcome::Partial { done, .. }) => {
                            Choice::dir((done > 0).then(|| pack.to_path_buf()))
                        }
                        Err(e) => {
                            progress(&format!(
                                "upscaled textures: build failed ({e}); using the original art"
                            ));
                            Choice::dir(None)
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_parsing() {
        assert_eq!(
            budget_from_env(None),
            Some(Duration::from_secs(DEFAULT_BUDGET_SECS))
        );
        assert_eq!(budget_from_env(Some("0")), None);
        assert_eq!(budget_from_env(Some("5")), Some(Duration::from_secs(5)));
        assert_eq!(
            budget_from_env(Some("x")),
            Some(Duration::from_secs(DEFAULT_BUDGET_SECS))
        );
    }

    #[test]
    fn off_and_explicit_dir_never_touch_the_cache() {
        let mut say = |_: &str| panic!("no progress expected");
        let nowhere = Path::new("/mc2-no-such-data");
        let c = prepare(&Hires::Off, nowhere, None, Build::Never, &mut say);
        assert_eq!(c.dir, None);
        assert_eq!(c.describe(), "off");
        let c = prepare(
            &Hires::Dir("/p".into()),
            nowhere,
            None,
            Build::Never,
            &mut say,
        );
        assert_eq!(c.dir.as_deref(), Some(Path::new("/p")));
    }
}
