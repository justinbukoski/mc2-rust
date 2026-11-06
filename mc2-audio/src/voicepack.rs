//! Optional pilot voice pack: replacement clips for single packets of the pilot voice paks.
//!
//! A pack is a directory laid out as `<dir>/<pak>/<NN>.wav`, e.g. `pilota/19.wav` replaces packet 19 of
//! `pilota.pak`. Pak names compare lower-case, the packet number is decimal with at least two digits. Clips are
//! ordinary RIFF/WAVE files (PCM or MS-ADPCM, any rate the mixer takes); the mixer resamples. A clip that is
//! missing or does not decode leaves the original packet in place, so a partial pack works.
//!
//! The pack only changes how a pilot sounds. Which message plays, when, and the static from `noise.pak` are
//! untouched. It is off unless the app is given a directory (see `rust/docs/VOICE-PACK.md`).

use mc2_platform::audio::Sound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A directory of replacement clips.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicePack {
    dir: PathBuf,
}

impl VoicePack {
    /// Open a pack directory. `None` when `dir` is not a directory.
    pub fn open(dir: impl Into<PathBuf>) -> Option<Self> {
        let dir = dir.into();
        dir.is_dir().then_some(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the clip for packet `index` of pak `pak` lives.
    pub fn clip_path(&self, pak: &str, index: usize) -> PathBuf {
        self.dir
            .join(pak.to_ascii_lowercase())
            .join(format!("{index:02}.wav"))
    }

    /// The decoded clip for `(pak, index)`, or `None` when the pack has none (or it is unreadable).
    pub fn clip(&self, pak: &str, index: usize) -> Option<Arc<Sound>> {
        let bytes = std::fs::read(self.clip_path(pak, index)).ok()?;
        Sound::from_wav(&bytes).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_lower_case_pak_and_two_digit_packet() {
        let p = VoicePack {
            dir: PathBuf::from("/v"),
        };
        assert_eq!(p.clip_path("PilotA", 7), Path::new("/v/pilota/07.wav"));
        assert_eq!(p.clip_path("pilot1a", 39), Path::new("/v/pilot1a/39.wav"));
        assert_eq!(p.clip_path("pilotb", 100), Path::new("/v/pilotb/100.wav"));
    }

    #[test]
    fn open_needs_a_directory() {
        assert!(VoicePack::open("/definitely/not/here").is_none());
        assert!(VoicePack::open(std::env::temp_dir()).is_some());
    }
}
