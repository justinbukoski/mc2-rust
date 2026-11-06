// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Virtual file system: the game's lookup order for data files.
//!
//! Port of the lookup in `mclib/file.cpp` (`File::open`) and the archive
//! mount in `code/mechcmd2.cpp` (`[FastFiles]` in `system.cfg`).
//!
//! 1. A loose file under the data root wins. `File::open` lowercases the name,
//!    and the Windows open is case-insensitive, so the lowercase path is tried
//!    first, then a case-insensitive walk. Both `/` and `\` separate
//!    components, as the Windows open and the LINUX_BUILD conversion allow.
//! 2. Otherwise the archives are searched in mount order and the first match
//!    wins. Archive matching is exact apart from ASCII case; see
//!    [`crate::fst::FastFile::index_of`].
//!
//! Names must stay inside the data root. Absolute names, drive prefixes and
//! `..` components are refused, and a loose file whose canonical path leaves
//! the root is not used.

use crate::error::{FormatError, Result};
use crate::fst::FastFile;
use crate::ini::FitIni;
use std::fs::File;
use std::path::{Path, PathBuf};

/// Where a name resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// A file on disk.
    Loose(PathBuf),
    /// Entry `entry` of archive number `archive` (in mount order).
    Archive { archive: usize, entry: usize },
}

struct Mounted {
    path: PathBuf,
    archive: FastFile<File>,
}

/// Data root plus the archives mounted from it.
pub struct Vfs {
    root: PathBuf,
    archives: Vec<Mounted>,
}

/// Lowercases a name and turns `\` into `/`, the form used for loose files.
pub fn normalize_name(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b == b'\\' {
                '/'
            } else {
                char::from(b.to_ascii_lowercase())
            }
        })
        .collect()
}

/// Splits a name into a path relative to the data root. Returns `None` for
/// absolute names, names with a drive or stream colon, and `..` components.
/// Empty and `.` components are dropped.
fn relative_path(name: &str) -> Option<PathBuf> {
    if name.starts_with(['/', '\\']) || name.contains(':') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            p => out.push(p),
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn invalid_path(name: &str) -> FormatError {
    FormatError::InvalidPath {
        detail: format!("{name:?} is absolute, has a drive prefix, or contains '..'"),
    }
}

impl Vfs {
    /// Creates a VFS with no archives mounted.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            archives: Vec::new(),
        }
    }

    /// Builds the VFS the game sets up at start-up, from `<root>/system.cfg`.
    ///
    /// Mirrors `code/mechcmd2.cpp`: `NumFastFiles` is read first, and a missing
    /// or zero value mounts nothing. A positive count requires every `FileN`
    /// through that count to mount successfully. A `FileN` at or past the count
    /// is an error, because the C++ `FastFileInit` fails when the table is full.
    pub fn from_system_cfg(root: impl Into<PathBuf>) -> Result<Self> {
        let mut vfs = Self::new(root);
        let mut cfg = FitIni::open(vfs.root.join("system.cfg"))?;
        cfg.seek_block("FastFiles")?;
        let count = match cfg.read_i64("NumFastFiles") {
            Ok(n) if n < 0 => {
                return Err(FormatError::Corrupt {
                    format: "FIT",
                    detail: format!("NumFastFiles is negative ({n})"),
                })
            }
            Ok(n) => n as u64,
            Err(FormatError::NotFound { .. }) => 0,
            Err(e) => return Err(e),
        };
        for i in 0..count {
            let name = cfg.read_string(&format!("File{i}"), 99)?;
            vfs.mount_archive(&name)?;
        }
        // With NumFastFiles at zero the C++ skips the block, so no extra check applies.
        if count > 0 && cfg.read_string(&format!("File{count}"), 99).is_ok() {
            return Err(FormatError::Corrupt {
                format: "FIT",
                detail: format!("File{count} is present but NumFastFiles is {count}"),
            });
        }
        Ok(vfs)
    }

    /// The data root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Mounts an archive at the end of the search order. The name is taken from
    /// the data root and must not leave it.
    pub fn mount_archive(&mut self, name: &str) -> Result<()> {
        let rel = relative_path(name).ok_or_else(|| invalid_path(name))?;
        let full = self.root.join(rel);
        // Symlinks can point anywhere, so the archive must canonicalise to a
        // file under the canonical root, as loose files must. The archive is
        // opened from the canonical path.
        let canonical = full.canonicalize()?;
        if !canonical.starts_with(self.root.canonicalize()?) {
            return Err(FormatError::InvalidPath {
                detail: format!("archive {name:?} resolves outside the data root"),
            });
        }
        let archive = FastFile::open(&canonical)?;
        self.archives.push(Mounted {
            path: full,
            archive,
        });
        Ok(())
    }

    /// Paths of the mounted archives, in search order.
    pub fn archive_paths(&self) -> impl Iterator<Item = &Path> + '_ {
        self.archives.iter().map(|m| m.path.as_path())
    }

    /// Resolves `name` without reading it. Names that leave the root never
    /// resolve.
    pub fn locate(&self, name: &str) -> Option<Source> {
        let rel = relative_path(name)?;
        if let Some(path) = self.find_loose(&rel) {
            return Some(Source::Loose(path));
        }
        self.archives.iter().enumerate().find_map(|(archive, m)| {
            m.archive
                .index_of(name)
                .map(|entry| Source::Archive { archive, entry })
        })
    }

    /// Contents of `name`, or `None` when neither a loose file nor an archive
    /// entry has it. A name that would leave the data root is an error.
    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        if relative_path(name).is_none() {
            return Err(invalid_path(name));
        }
        match self.locate(name) {
            None => Ok(None),
            Some(Source::Loose(path)) => Ok(Some(std::fs::read(path)?)),
            Some(Source::Archive { archive, entry }) => {
                self.archives[archive].archive.read_index(entry).map(Some)
            }
        }
    }

    /// The names of the files directly inside `dir` (`FindFirstFile(dir + "*")`): the archived
    /// entries and the loose files, lower-cased, with `dir` in front, sorted and without
    /// duplicates. A directory that leaves the data root lists nothing.
    pub fn list_dir(&self, dir: &str) -> Vec<String> {
        let Some(rel) = relative_path(dir) else {
            return Vec::new();
        };
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect();
        let prefix = format!("{}/", parts.join("/"));
        let mut names: Vec<String> = Vec::new();
        for mounted in &self.archives {
            for entry in mounted.archive.entries() {
                let lower = entry.name.to_ascii_lowercase();
                if let Some(rest) = lower.strip_prefix(&prefix) {
                    if !rest.is_empty() && !rest.contains(['/', '\\']) {
                        names.push(lower);
                    }
                }
            }
        }
        self.list_loose(&self.root, &parts, &prefix, &mut names);
        names.sort();
        names.dedup();
        names
    }

    /// The loose files of `dir` below `base`, matching each path component without regard to case.
    fn list_loose(&self, base: &Path, wanted: &[String], prefix: &str, out: &mut Vec<String>) {
        let Some((want, rest)) = wanted.split_first() else {
            let Ok(read) = std::fs::read_dir(base) else {
                return;
            };
            for entry in read.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file() && self.inside_root(&path) {
                    out.push(format!(
                        "{prefix}{}",
                        entry.file_name().to_string_lossy().to_ascii_lowercase()
                    ));
                }
            }
            return;
        };
        let Ok(read) = std::fs::read_dir(base) else {
            return;
        };
        let mut next: Vec<PathBuf> = read
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(want))
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        next.sort();
        for dir in next {
            self.list_loose(&dir, rest, prefix, out);
        }
    }

    /// Parses `name` as a FIT/INI file, or returns `None` when it is absent.
    pub fn read_fit(&mut self, name: &str) -> Result<Option<FitIni>> {
        match self.read(name)? {
            Some(bytes) => FitIni::from_bytes(bytes).map(Some),
            None => Ok(None),
        }
    }

    fn find_loose(&self, rel: &Path) -> Option<PathBuf> {
        let lower: PathBuf = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
            .collect();
        let direct = self.root.join(&lower);
        if direct.is_file() && self.inside_root(&direct) {
            return Some(direct);
        }
        let wanted: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        self.walk_ci(&self.root, &wanted)
    }

    /// Case-insensitive (ASCII) walk of `wanted` below `dir`, independent of
    /// the host file system's own case rules. On a case-sensitive host several
    /// entries can match one component (`Data` and `data`); every match is
    /// tried, in sorted name order so the result is deterministic.
    fn walk_ci(&self, dir: &Path, wanted: &[String]) -> Option<PathBuf> {
        let (want, rest) = wanted.split_first()?;
        let mut matches: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .filter(|(name, _)| name.eq_ignore_ascii_case(want))
            .collect();
        matches.sort();
        matches.into_iter().find_map(|(_, path)| {
            if rest.is_empty() {
                (path.is_file() && self.inside_root(&path)).then_some(path)
            } else {
                self.walk_ci(&path, rest)
            }
        })
    }

    /// Whether the canonical form of `path` is under the canonical data root.
    fn inside_root(&self, path: &Path) -> bool {
        match (path.canonicalize(), self.root.canonicalize()) {
            (Ok(p), Ok(r)) => p.starts_with(r),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mc2-formats-vfs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Data")).unwrap();
        dir
    }

    #[test]
    fn list_dir_names_the_loose_files_directly_inside_a_directory() {
        let dir = scratch_dir("list");
        std::fs::create_dir_all(dir.join("Data").join("Missions").join("sub")).unwrap();
        std::fs::write(dir.join("Data").join("Missions").join("B.FIT"), b"b").unwrap();
        std::fs::write(dir.join("Data").join("Missions").join("a.fit"), b"a").unwrap();
        std::fs::write(
            dir.join("Data").join("Missions").join("sub").join("c.fit"),
            b"c",
        )
        .unwrap();
        let vfs = Vfs::new(&dir);
        assert_eq!(
            vfs.list_dir("data/missions"),
            vec!["data/missions/a.fit", "data/missions/b.fit"]
        );
        assert!(vfs.list_dir("../etc").is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn loose_file_wins_and_lookup_is_case_insensitive() {
        let dir = scratch_dir("loose");
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data").join("x.txt"), b"loose").unwrap();
        std::fs::write(dir.join("Data").join("Other.TXT"), b"other").unwrap();

        let mut vfs = Vfs::new(&dir);
        assert_eq!(vfs.read("DATA\\X.txt").unwrap(), Some(b"loose".to_vec()));
        assert_eq!(vfs.read("data/other.txt").unwrap(), Some(b"other".to_vec()));
        assert_eq!(vfs.read("nope.txt").unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The lookup follows the game's case rules itself, not the host file system's: it must find mixed-case names, and
    /// it must not stop at the first case-variant directory when another one holds the file (`Art/` and `art/` can both
    /// exist on a case-sensitive host).
    #[test]
    fn lookup_does_not_depend_on_host_case_rules() {
        let dir = scratch_dir("ci");
        std::fs::create_dir_all(dir.join("Art/Sub")).unwrap();
        std::fs::create_dir_all(dir.join("art/sub")).unwrap();
        std::fs::create_dir_all(dir.join("ART")).unwrap();
        std::fs::write(dir.join("Art/Sub/First.TXT"), b"first").unwrap();
        std::fs::write(dir.join("art/sub/second.txt"), b"second").unwrap();
        std::fs::write(dir.join("ART/Third.Txt"), b"third").unwrap();

        let mut vfs = Vfs::new(&dir);
        for (name, want) in [
            ("art/sub/first.txt", &b"first"[..]),
            ("ART\\SUB\\SECOND.TXT", b"second"),
            ("Art/Third.txt", b"third"),
        ] {
            assert_eq!(vfs.read(name).unwrap().as_deref(), Some(want), "{name}");
        }
        assert_eq!(vfs.read("art/sub/missing.txt").unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn archive_is_used_when_loose_file_is_missing() {
        let dir = scratch_dir("archive");
        let payload = b"from archive".to_vec();
        let packed = {
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&payload).unwrap();
            enc.finish().unwrap()
        };
        let mut image = crate::fst::FST_VERSION_LZ.to_le_bytes().to_vec();
        image.extend_from_slice(&1u32.to_le_bytes());
        image.extend_from_slice(&(8u32 + 266).to_le_bytes());
        image.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        image.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        image.extend_from_slice(&crate::fst::elf_hash(b"data/inarc.txt").to_le_bytes());
        let mut name = [0u8; 250];
        name[..14].copy_from_slice(b"data/inarc.txt");
        image.extend_from_slice(&name);
        image.extend_from_slice(&packed);
        let mut f = std::fs::File::create(dir.join("test.fst")).unwrap();
        f.write_all(&image).unwrap();
        drop(f);

        let mut vfs = Vfs::new(&dir);
        vfs.mount_archive("test.fst").unwrap();
        assert!(matches!(
            vfs.locate("DATA/INARC.TXT"),
            Some(Source::Archive {
                archive: 0,
                entry: 0
            })
        ));
        assert_eq!(vfs.read("data/inarc.txt").unwrap(), Some(payload));
        // Archive names are not separator-equivalent (FastFileFind has no retry).
        assert_eq!(vfs.read("data\\inarc.txt").unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn zero_byte_configured_archive_is_rejected_with_an_explicit_error() {
        let dir = scratch_dir("empty-archive");
        std::fs::write(dir.join("empty.fst"), []).unwrap();

        let mut vfs = Vfs::new(&dir);
        let error = vfs.mount_archive("empty.fst").unwrap_err();
        assert!(error
            .to_string()
            .contains("0 bytes is shorter than the 8-byte header"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn names_that_leave_the_root_are_refused() {
        let dir = scratch_dir("escape");
        let outside = dir.join(format!("outside-{}.txt", std::process::id()));
        std::fs::write(&outside, b"secret").unwrap();
        let root = dir.join("Data");
        let outside_name = outside.file_name().unwrap().to_string_lossy().into_owned();

        let mut vfs = Vfs::new(&root);
        assert!(vfs.read(&format!("../{outside_name}")).is_err());
        assert!(vfs.read(&format!("..\\{outside_name}")).is_err());
        assert!(vfs.read(&outside.to_string_lossy()).is_err());
        assert!(vfs.locate("../x").is_none());
        assert!(vfs.mount_archive("../x.fst").is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
