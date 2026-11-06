// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Bounds for counts read from FIT files.
//!
//! The C++ loaders trust `NumParts`, `NumWarriors`, `GroupCount` and similar keys and loop
//! over them. A small file with a huge count, or with many unrelated or duplicate blocks,
//! would otherwise loop or allocate without limit. Two rules apply here:
//! - a count is never above [`MAX_ENTRIES`], the hard limit for any counted list;
//! - a list whose C++ reader asserts on each missing block (parts, warriors) is accepted
//!   only when every indexed block it needs (`PartN`, `WarriorN`) exists. Only blocks that
//!   match the exact `<prefix><digits><suffix>` pattern are counted, and each index is
//!   counted once, however many times its block repeats.
//!
//! The campaign lists (`GroupN`, `GroupNMissionM`) tolerate missing entries: the C++ records
//! a missing group and continues (`logisticsmissioninfo.cpp`), so for those only the hard
//! limit applies.

use std::collections::BTreeSet;

use mc2_formats::ini::FitIni;

use crate::error::{Result, SimDataError};

/// Hard maximum for any counted list (parts, warriors, groups, missions). A count above this
/// is an error, before any loop or allocation.
pub(crate) const MAX_ENTRIES: usize = 4096;

/// Indices `N` (each once) of blocks named exactly `<prefix><N><suffix>`, where `N` is a
/// decimal number of one or more digits and `N >= 1`. Other blocks are ignored.
fn indexed(fit: &FitIni, prefix: &str, suffix: &str) -> BTreeSet<u64> {
    fit.blocks()
        .filter_map(|id| {
            let rest = id.strip_prefix(prefix)?.strip_suffix(suffix)?;
            if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let n: u64 = rest.parse().ok()?;
            (n >= 1).then_some(n)
        })
        .collect()
}

fn too_large(file: &str, what: &str, count: u64) -> SimDataError {
    SimDataError::schema(
        file,
        format!("{what} = {count} is above the limit of {MAX_ENTRIES} entries"),
    )
}

/// A required indexed list. The count must not exceed [`MAX_ENTRIES`], and the blocks
/// `<prefix><first>..<prefix><first + count - 1><suffix>` must all exist. Returns the count.
pub(crate) fn required_indexed(
    fit: &FitIni,
    file: &str,
    what: &str,
    count: u64,
    prefix: &str,
    suffix: &str,
    first: u64,
) -> Result<usize> {
    if count > MAX_ENTRIES as u64 {
        return Err(too_large(file, what, count));
    }
    let have = indexed(fit, prefix, suffix);
    for n in first..first + count {
        if !have.contains(&n) {
            return Err(SimDataError::schema(
                file,
                format!("{what} = {count} but block [{prefix}{n}{suffix}] is missing"),
            ));
        }
    }
    Ok(count as usize)
}

/// A count for a list whose missing entries are legal (campaign groups and missions).
/// Only the hard limit applies. A negative count is treated as zero, as the C++ loop does.
pub(crate) fn capped_count(file: &str, what: &str, count: i64) -> Result<usize> {
    let count = count.max(0) as u64;
    if count > MAX_ENTRIES as u64 {
        return Err(too_large(file, what, count));
    }
    Ok(count as usize)
}

/// An empty vector with room for `count` entries. Reservation is fallible, so an
/// allocation failure is an error rather than an abort.
pub(crate) fn reserve_entries<T>(file: &str, count: usize) -> Result<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count)
        .map_err(|_| SimDataError::out_of_range(file, format!("cannot reserve {count} entries")))?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(format!("FITini\r\n\r\n{text}\r\nFITend\r\n").into_bytes())
            .expect("parse")
    }

    #[test]
    fn huge_declared_count_is_refused_by_the_limit() {
        let f = fit("[Parts]\r\nul NumParts = 2000000000\r\n");
        let err =
            required_indexed(&f, "t.fit", "NumParts", 2_000_000_000, "Part", "", 1).unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");
    }

    #[test]
    fn missing_indexed_block_is_an_error() {
        let f = fit("[Parts]\r\nul NumParts = 2\r\n[Part1]\r\nul ObjectNumber = 3\r\n");
        let err = required_indexed(&f, "t.fit", "NumParts", 2, "Part", "", 1).unwrap_err();
        assert!(err.to_string().contains("Part2"), "{err}");
    }

    #[test]
    fn many_unrelated_blocks_do_not_count() {
        // 20,000 unrelated blocks and a declared count of 3 with only one Part block.
        let mut text =
            String::from("[Parts]\r\nul NumParts = 3\r\n[Part1]\r\nul ObjectNumber = 3\r\n");
        for i in 0..20_000 {
            text.push_str(&format!("[Junk{i}]\r\nl x = {i}\r\n"));
        }
        let f = fit(&text);
        assert!(required_indexed(&f, "t.fit", "NumParts", 3, "Part", "", 1).is_err());
    }

    #[test]
    fn duplicate_blocks_count_once() {
        // Part1 repeated 10,000 times is still one index; Part2 is missing.
        let mut text = String::from("[Parts]\r\nul NumParts = 2\r\n");
        for _ in 0..10_000 {
            text.push_str("[Part1]\r\nul ObjectNumber = 3\r\n");
        }
        let f = fit(&text);
        assert!(required_indexed(&f, "t.fit", "NumParts", 2, "Part", "", 1).is_err());
        assert_eq!(
            required_indexed(&f, "t.fit", "NumParts", 1, "Part", "", 1).unwrap(),
            1
        );
    }

    #[test]
    fn near_miss_names_do_not_count() {
        // "Parts", "Part1x", "Part" and "Part0" are not Part1..PartN.
        let f = fit("[Parts]\r\nul NumParts = 1\r\n[Part1x]\r\nl a = 1\r\n[Part]\r\nl a = 1\r\n[Part0]\r\nl a = 1\r\n");
        assert!(required_indexed(&f, "t.fit", "NumParts", 1, "Part", "", 1).is_err());
    }

    #[test]
    fn campaign_style_counts_only_apply_the_limit() {
        assert_eq!(capped_count("c.fit", "GroupCount", -4).unwrap(), 0);
        assert_eq!(capped_count("c.fit", "GroupCount", 13).unwrap(), 13);
        assert!(capped_count("c.fit", "GroupCount", 4097).is_err());
    }

    #[test]
    fn reservation_is_fallible_and_bounded_by_the_limit() {
        let v: Vec<u8> = reserve_entries("t.fit", MAX_ENTRIES).unwrap();
        assert!(v.capacity() >= MAX_ENTRIES);
        // An absurd reservation is refused as an error, not an abort.
        assert!(reserve_entries::<u64>("t.fit", usize::MAX / 8).is_err());
    }
}
