// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Key readers shared by the mission loaders.
//!
//! The FIT reader reports an absent key as `FormatError::NotFound`, which is
//! the C++ `result != NO_ERR` case for a missing key. The helpers here turn
//! that case into `None` for optional keys (the C++ keeps a default) or into a
//! schema error for required keys (the C++ asserts). Every other format error
//! is passed through unchanged.

use crate::error::{Result, SimDataError};
use mc2_formats::ini::FitIni;
use mc2_formats::FormatError;

/// Outcome of reading one entry (an objective, condition, action, ...).
///
/// The C++ loaders delete an entry whose `Read` returns false and carry on
/// with the next one. `Dropped` carries the reason so the caller can record it
/// as a warning.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Read<T> {
    Loaded(T),
    Dropped(String),
}

/// Selects block `id`. Returns `false` when the block is absent.
pub(crate) fn select(fit: &mut FitIni, id: &str) -> Result<bool> {
    match fit.seek_block(id) {
        Ok(()) => Ok(true),
        Err(FormatError::NotFound { .. }) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Maps a missing key to `None`; every other error is kept.
pub(crate) fn opt<T>(value: mc2_formats::Result<T>) -> Result<Option<T>> {
    match value {
        Ok(v) => Ok(Some(v)),
        Err(FormatError::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Maps a missing key to a schema error that names the file and the key.
pub(crate) fn req<T>(value: mc2_formats::Result<T>, file: &str, key: &str) -> Result<T> {
    match value {
        Ok(v) => Ok(v),
        Err(FormatError::NotFound { .. }) => {
            Err(SimDataError::schema(file, format!("missing key {key}")))
        }
        Err(e) => Err(e.into()),
    }
}

/// Optional `st` value read with a C buffer limit of `size` bytes.
pub(crate) fn opt_text(fit: &FitIni, key: &str, size: usize) -> Result<Option<String>> {
    opt(fit.read_string(key, size))
}

/// Optional objective-side string. `sReadIdString` (objective.cpp:268) reads
/// with a 2000 byte buffer and turns each `\n` into `\r\n` through `sReplace` (objective.cpp:243).
pub(crate) fn objective_text(fit: &FitIni, key: &str) -> Result<Option<String>> {
    Ok(opt(fit.read_string(key, 2000))?.map(crlf))
}

/// The `\n` to `\r\n` step of `sReplace` (objective.cpp:243-266).
pub(crate) fn crlf(s: String) -> String {
    s.replace('\n', "\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("{text}\n").into_bytes()).expect("fit")
    }

    #[test]
    fn select_reports_absent_block_as_false() {
        let mut f = fit("FITini\n[A]\nul x = 1\n");
        assert!(select(&mut f, "A").unwrap());
        assert!(!select(&mut f, "B").unwrap());
    }

    #[test]
    fn opt_and_req_map_missing_keys() {
        let mut f = fit("FITini\n[A]\nul x = 7\n");
        f.seek_block("A").unwrap();
        assert_eq!(opt(f.read_u32("x")).unwrap(), Some(7));
        assert_eq!(opt(f.read_u32("y")).unwrap(), None);
        assert_eq!(req(f.read_u32("x"), "t.fit", "x").unwrap(), 7);
        assert!(matches!(
            req(f.read_u32("y"), "t.fit", "y"),
            Err(SimDataError::Schema { .. })
        ));
    }

    #[test]
    fn crlf_turns_newlines_into_crlf() {
        assert_eq!(crlf("one\ntwo".to_string()), "one\r\ntwo");
    }

    #[test]
    fn objective_text_reads_single_line_strings() {
        let mut f = fit("FITini\n[A]\nst T = \"one\"\n");
        f.seek_block("A").unwrap();
        assert_eq!(objective_text(&f, "T").unwrap().as_deref(), Some("one"));
        assert_eq!(objective_text(&f, "U").unwrap(), None);
    }
}
