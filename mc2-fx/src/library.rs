// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The effect library: `mc2.fx` (`data/effects/mc2.fx`, packed in `effect.fst`).
//!
//! `EffectLibrary::Load` (`mclib/gosfx/effectlibrary.cpp`): the `GFX#` signature, the version
//! (at most 17), a count, then that many specifications, each preceded by its class id. An
//! effect's id is its index in this list; events and `EffectCloud`s refer to each other by it.

use crate::error::{FxError, Result};
use crate::spec::{EffectClass, EffectSpec};
use crate::stream::Stream;
use std::collections::HashMap;
use std::sync::Arc;

pub const GFX_SIGNATURE: u32 = 0x4746_5823; // 'GFX#'
pub const CURRENT_GFX_VERSION: i32 = 17;

/// Where the game keeps the library inside the data root.
pub const LIBRARY_PATH: &str = "data/effects/mc2.fx";

pub struct EffectLibrary {
    pub version: i32,
    specs: Vec<Arc<EffectSpec>>,
    /// Lowercased name -> first index, as `EffectLibrary::Find` (`S_stricmp`, first match).
    by_name: HashMap<String, usize>,
}

impl EffectLibrary {
    /// `ReadGFXVersion` + the specification loop. Fails on trailing bytes: the real file is
    /// consumed exactly.
    pub fn load(bytes: &[u8]) -> Result<EffectLibrary> {
        let mut s = Stream::new(bytes);
        let sig = s.u32()?;
        if sig != GFX_SIGNATURE {
            return Err(FxError::new(format!("bad gosFX signature {sig:#010x}")));
        }
        let version = s.i32()?;
        if version > CURRENT_GFX_VERSION {
            return Err(FxError::new(format!(
                "gosFX data version {version} is newer than {CURRENT_GFX_VERSION}"
            )));
        }
        // Each specification is at least a class id, a name length and a few counts.
        let count = s.count(8)?;
        let mut specs = Vec::with_capacity(count);
        for i in 0..count {
            let at = s.pos();
            let mut spec = EffectSpec::create(&mut s, version)
                .map_err(|e| FxError::new(format!("effect {i} (stream offset {at}): {e}")))?;
            spec.base.effect_id = i as u32;
            specs.push(Arc::new(spec));
        }
        if s.remaining() != 0 {
            return Err(FxError::new(format!(
                "{} trailing bytes after {count} effects",
                s.remaining()
            )));
        }
        let mut by_name = HashMap::new();
        for (i, spec) in specs.iter().enumerate() {
            by_name
                .entry(spec.base.name.to_ascii_lowercase())
                .or_insert(i);
        }
        Ok(EffectLibrary {
            version,
            specs,
            by_name,
        })
    }

    /// A library from specifications built in code; effect ids are assigned by position.
    pub fn from_specs(version: i32, specs: Vec<EffectSpec>) -> EffectLibrary {
        let specs: Vec<Arc<EffectSpec>> = specs
            .into_iter()
            .enumerate()
            .map(|(i, mut s)| {
                s.base.effect_id = i as u32;
                Arc::new(s)
            })
            .collect();
        let mut by_name = HashMap::new();
        for (i, spec) in specs.iter().enumerate() {
            by_name
                .entry(spec.base.name.to_ascii_lowercase())
                .or_insert(i);
        }
        EffectLibrary {
            version,
            specs,
            by_name,
        }
    }

    pub fn len(&self) -> usize {
        self.specs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    pub fn spec(&self, id: usize) -> Option<&Arc<EffectSpec>> {
        self.specs.get(id)
    }

    pub fn specs(&self) -> &[Arc<EffectSpec>] {
        &self.specs
    }

    /// `EffectLibrary::Find`: case-insensitive name lookup.
    pub fn find(&self, name: &str) -> Option<&Arc<EffectSpec>> {
        self.by_name
            .get(&name.to_ascii_lowercase())
            .map(|&i| &self.specs[i])
    }

    /// Effects per class, for the load report.
    pub fn counts_by_class(&self) -> Vec<(EffectClass, usize)> {
        let mut out = Vec::new();
        for class in EffectClass::ALL {
            let n = self.specs.iter().filter(|s| s.class() == class).count();
            if n > 0 {
                out.push((class, n));
            }
        }
        out
    }
}
