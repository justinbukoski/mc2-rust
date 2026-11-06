// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Source file access for the ABL loader.
//!
//! The C++ code opens files through callbacks (`ablFileOpenCB`) which lower-case the file name before hashing it into
//! the game's `.fst` archives. [`SourceProvider`] is the Rust equivalent; implementations receive a normalized path
//! (lower case, forward slashes) and return the whole file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

/// Normalize a path the way the game does before lookup: lower case, `\` becomes `/`.
pub fn normalize_path(path: &str) -> String {
    path.chars()
        .map(|c| {
            if c == '\\' {
                '/'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Supplies ABL source text.
pub trait SourceProvider {
    /// Return the full contents of `path` (already normalized with [`normalize_path`]), or `None` if missing.
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// In-memory source set, used by tests and by hosts that unpack archives up front. Cloning is cheap (shared).
#[derive(Default, Clone, Debug)]
pub struct MemorySource {
    files: Rc<HashMap<String, Vec<u8>>>,
}

impl MemorySource {
    /// An empty source set.
    pub fn new() -> MemorySource {
        MemorySource::default()
    }

    /// Add (or replace) a file. The path is normalized.
    pub fn insert(&mut self, path: &str, contents: impl Into<Vec<u8>>) {
        Rc::make_mut(&mut self.files).insert(normalize_path(path), contents.into());
    }

    /// Number of files held.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// True when no files are held.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

impl SourceProvider for MemorySource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }
}

/// Reads source files from a directory tree on disk. Lookup is case-insensitive by trying the normalized
/// (lower-case) path under the root.
#[derive(Clone, Debug)]
pub struct DirSource {
    root: PathBuf,
}

impl DirSource {
    /// Create a provider rooted at `root`.
    pub fn new(root: impl Into<PathBuf>) -> DirSource {
        DirSource { root: root.into() }
    }
}

impl SourceProvider for DirSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.root.join(path)).ok()
    }
}
