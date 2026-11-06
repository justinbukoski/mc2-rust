//! Where audio files come from.
//!
//! The C++ opens `<soundPath>*.snd`, `.csv` and the streamed `.wav` files, and `<CDsoundPath>*.pak`, through its
//! own file layer; here every read goes through [`AudioData`], implemented for the game [`Vfs`] (loose files
//! first, then archives, case-insensitive) and for tests. The two directories come from `system.cfg`
//! ([`SoundPaths`]).

use mc2_formats::ini::FitIni;
use mc2_formats::vfs::{Source, Vfs};
use mc2_platform::audio::{RealtimeSource, Sound, StreamingSource, WavStream};
use std::fs::File;

/// Default of both sound directories in the shipped `system.cfg`.
pub const DEFAULT_SOUND_PATH: &str = "data/sound/";

/// The two sound directories of `system.cfg` `[systemPaths]`: `soundPath` (the `.snd` table, `radio.csv`,
/// streamed music and voice-over `.wav` files) and `CDsoundPath` (the `.pak` banks: `sound`, `betty`,
/// `support`, `noise` and the pilot voice sets). Names are appended to the directory as written; a missing
/// trailing separator is added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundPaths {
    pub sound_path: String,
    pub cd_sound_path: String,
}

impl Default for SoundPaths {
    fn default() -> Self {
        Self {
            sound_path: DEFAULT_SOUND_PATH.to_string(),
            cd_sound_path: DEFAULT_SOUND_PATH.to_string(),
        }
    }
}

fn with_separator(dir: &str) -> String {
    if dir.is_empty() || dir.ends_with(['/', '\\']) {
        dir.to_string()
    } else {
        format!("{dir}/")
    }
}

impl SoundPaths {
    /// Read `soundPath` and `CDsoundPath` from the `[systemPaths]` block (80 byte buffers in the C++).
    /// A missing block or key keeps the default.
    pub fn from_system_cfg(cfg: &mut FitIni) -> Self {
        let mut p = Self::default();
        if cfg.seek_block("systemPaths").is_ok() {
            if let Ok(v) = cfg.read_string("soundPath", 80) {
                p.sound_path = with_separator(&v);
            }
            if let Ok(v) = cfg.read_string("CDsoundPath", 80) {
                p.cd_sound_path = with_separator(&v);
            }
        }
        p
    }

    /// `soundPath + name + ext`.
    pub fn sound(&self, name: &str, ext: &str) -> String {
        format!("{}{name}{ext}", self.sound_path)
    }

    /// `CDsoundPath + name + ext`.
    pub fn cd_sound(&self, name: &str, ext: &str) -> String {
        format!("{}{name}{ext}", self.cd_sound_path)
    }
}

/// Source of audio data files.
pub trait AudioData {
    /// The sound directories (`soundPath`, `CDsoundPath`).
    fn paths(&self) -> &SoundPaths;

    /// Whole file contents, `None` if absent or unreadable.
    fn read(&mut self, name: &str) -> Option<Vec<u8>>;

    /// Whether the file exists (`fileExists`).
    fn exists(&self, name: &str) -> bool;

    /// Open a WAV file for playback on a mixer channel (`gosAudio_StreamedFile`). The default decodes
    /// the whole file into memory; implementations backed by real files stream it instead.
    fn open_stream(&mut self, name: &str) -> Option<Box<dyn RealtimeSource>> {
        let bytes = self.read(name)?;
        Some(Sound::from_wav(&bytes).ok()?.source())
    }
}

/// [`AudioData`] over the game VFS.
pub struct VfsData {
    vfs: Vfs,
    stream_loose_files: bool,
    paths: SoundPaths,
}

impl VfsData {
    /// `stream_loose_files` decides how music and voice-overs found as loose files are played: `true`
    /// decodes them on a worker thread while they play (what the game does at run time); `false`
    /// decodes the whole file up front, which makes offline renders independent of thread timing.
    ///
    /// The sound directories are read from `system.cfg` in the data root (defaults if it has none).
    pub fn new(mut vfs: Vfs, stream_loose_files: bool) -> Self {
        let paths = match vfs.read_fit("system.cfg") {
            Ok(Some(mut cfg)) => SoundPaths::from_system_cfg(&mut cfg),
            _ => SoundPaths::default(),
        };
        Self::with_paths(vfs, stream_loose_files, paths)
    }

    /// As [`new`](Self::new) with explicit sound directories.
    pub fn with_paths(vfs: Vfs, stream_loose_files: bool, paths: SoundPaths) -> Self {
        Self {
            vfs,
            stream_loose_files,
            paths,
        }
    }

    /// The wrapped VFS.
    pub fn vfs_mut(&mut self) -> &mut Vfs {
        &mut self.vfs
    }
}

impl AudioData for VfsData {
    fn paths(&self) -> &SoundPaths {
        &self.paths
    }

    fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        self.vfs.read(name).ok().flatten()
    }

    fn exists(&self, name: &str) -> bool {
        self.vfs.locate(name).is_some()
    }

    fn open_stream(&mut self, name: &str) -> Option<Box<dyn RealtimeSource>> {
        if self.stream_loose_files {
            if let Some(Source::Loose(path)) = self.vfs.locate(name) {
                let file = File::open(path).ok()?;
                let wav = WavStream::new(file).ok()?;
                return Some(Box::new(StreamingSource::spawn(Box::new(wav))));
            }
        }
        let bytes = self.read(name)?;
        Some(Sound::from_wav(&bytes).ok()?.source())
    }
}

/// In-memory files, for tests and tools.
#[derive(Default)]
pub struct MemoryData {
    files: std::collections::HashMap<String, Vec<u8>>,
    paths: SoundPaths,
}

impl MemoryData {
    pub fn new() -> Self {
        Self::default()
    }

    /// Use other sound directories than the default `data/sound/`.
    pub fn with_paths(mut self, paths: SoundPaths) -> Self {
        self.paths = paths;
        self
    }

    /// Add a file; names compare case-insensitively with `\` as `/`.
    pub fn insert(&mut self, name: &str, bytes: Vec<u8>) {
        self.files
            .insert(mc2_formats::vfs::normalize_name(name), bytes);
    }
}

impl AudioData for MemoryData {
    fn paths(&self) -> &SoundPaths {
        &self.paths
    }

    fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        self.files
            .get(&mc2_formats::vfs::normalize_name(name))
            .cloned()
    }

    fn exists(&self, name: &str) -> bool {
        self.files
            .contains_key(&mc2_formats::vfs::normalize_name(name))
    }
}
