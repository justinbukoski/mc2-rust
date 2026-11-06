// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Raw block lists for packets. Used for classes with no typed loader, and kept on
//! every loaded type as [`super::ObjectType::raw_blocks`].
//!
//! A block starts with a line whose first character is `[`; the id runs to the
//! first `]` (the FIT rule in `mc2_formats::ini`). Lines are kept as written, with
//! only the trailing CR removed. Blank lines are dropped. Lines before the first
//! block (the `FITini` header) are dropped.

/// One `[Block]` of a packet and the lines under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBlock {
    /// Id between the brackets. Matched case-sensitively, as `seekBlock` does.
    pub id: String,
    /// Body lines, in file order. Each is `tag name = value`, a comment, or the
    /// continuation of a multi-line value.
    pub lines: Vec<String>,
}

/// Splits a packet into its blocks.
pub fn parse_blocks(packet: &[u8]) -> Vec<RawBlock> {
    let text = String::from_utf8_lossy(packet);
    let mut blocks: Vec<RawBlock> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix('[') {
            let id = rest.split(']').next().unwrap_or("").to_string();
            blocks.push(RawBlock {
                id,
                lines: Vec::new(),
            });
        } else if let Some(block) = blocks.last_mut() {
            if !line.trim().is_empty() {
                block.lines.push(line.to_string());
            }
        }
    }
    blocks
}

/// Finds a block by id (first match, case-sensitive).
pub fn find_block<'a>(blocks: &'a [RawBlock], id: &str) -> Option<&'a RawBlock> {
    blocks.iter().find(|b| b.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_blocks_and_keeps_lines() {
        let packet = b"FITini\r\n// head\r\n[ObjectClass]\r\nl ObjectTypeNum = 5\r\n\r\n[Other]\r\nst Name = \"x\"\n";
        let blocks = parse_blocks(packet);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].id, "ObjectClass");
        assert_eq!(blocks[0].lines, vec!["l ObjectTypeNum = 5".to_string()]);
        assert_eq!(blocks[1].id, "Other");
        assert_eq!(blocks[1].lines, vec!["st Name = \"x\"".to_string()]);
        assert!(find_block(&blocks, "Other").is_some());
        assert!(find_block(&blocks, "other").is_none());
    }
}
