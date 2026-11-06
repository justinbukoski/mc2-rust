//! String-table access for display text.
//!
//! The original resolves names through `cLoadString(id, ...)` against the `mc2res` string
//! resource, which is not part of the shipped data files. The campaign state therefore keeps ids
//! (and, where the C++ reads a literal, the literal) and leaves resolution to the UI crate. This
//! trait is the seam: anything that can look up a string id can render names for the campaign
//! screens, and tests can use [`NoStrings`].

/// Looks up a string-table entry by id (`cLoadString`).
pub trait StringTable {
    /// The text for `id`, or `None` when the table has no such entry.
    fn get(&self, id: u32) -> Option<String>;

    /// The text for `id`, or a placeholder (`#<id>`) when missing.
    fn get_or_placeholder(&self, id: u32) -> String {
        self.get(id).unwrap_or_else(|| format!("#{id}"))
    }
}

/// A table with no entries; every lookup falls back to the placeholder.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoStrings;

impl StringTable for NoStrings {
    fn get(&self, _id: u32) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_entries_use_a_placeholder() {
        assert_eq!(NoStrings.get_or_placeholder(27006), "#27006");
    }
}
