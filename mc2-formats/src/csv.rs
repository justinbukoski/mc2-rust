// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! CSV tables as the game reads them (read only).
//!
//! Port of `mclib/csvfile.{h,cpp}` (`CSVFile`). The game cannot write CSV
//! files, and `create` is a fatal error in the C++, so there is no writer.
//!
//! Dialect:
//! - Lines end at CR or LF, and reading is limited to 2047 bytes per line.
//! - Fields are separated by commas. Leading spaces and tabs in a field are
//!   skipped. Trailing spaces stay in the field. A field that is empty or
//!   starts with `/` counts as absent.
//! - Rows are 1-based: row 1 is the first line of the file. Columns are
//!   1-based too, but column 0 reads the first field as well
//!   (`seekRowCol` starts its comma walk at `colCount = 1`).
//! - The game's bounds (`countRows`, `countCols`) leave the last line and the
//!   last field unreachable. The row count is the number of lines after the
//!   first, not counting the last one (`countRows` loops `while (!eof())`). The
//!   column count is the number of commas on the first line, and a column
//!   number above it is rejected.
//! - Typed reads return `None` where the C code silently returns zero.
//! - A string cell ends at its first `"`, so a cell that starts with a quote
//!   reads as empty.

use crate::error::{FormatError, Result};
use crate::ini::read_line;
use crate::text::{atof, bool_value, c_str, int_bits};
use std::path::Path;

const LINE_LIMIT: usize = 2047;

/// Parsed CSV table over an in-memory image.
#[derive(Debug, Clone)]
pub struct CsvFile {
    data: Vec<u8>,
    rows: u32,
    cols: u32,
}

/// Counts rows like `CSVFile::countRows`.
fn count_rows(data: &[u8]) -> u32 {
    let mut pos = 0usize;
    let _ = read_line(data, &mut pos, LINE_LIMIT);
    let mut count = 0u32;
    while pos < data.len() {
        count += 1;
        let _ = read_line(data, &mut pos, LINE_LIMIT);
    }
    count
}

/// Counts columns like `CSVFile::countCols`: commas on the first line.
fn count_cols(data: &[u8]) -> u32 {
    let mut pos = 0usize;
    let first = read_line(data, &mut pos, LINE_LIMIT);
    first.iter().filter(|&&b| b == b',').count() as u32
}

impl CsvFile {
    /// Parses a CSV image.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let rows = count_rows(&data);
        let cols = count_cols(&data);
        Ok(Self { data, rows, cols })
    }

    /// Reads and parses a CSV file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// Number of addressable rows (1..=rows).
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Highest addressable column index (0..=cols).
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// Raw text of a cell, after leading blanks are skipped. `None` for
    /// absent or empty cells, and for out-of-range coordinates.
    pub fn cell(&self, row: u32, col: u32) -> Option<String> {
        self.cell_bytes(row, col)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    fn cell_bytes(&self, row: u32, col: u32) -> Option<Vec<u8>> {
        // Row 0 never matches in the C loop, which runs off the end of the file.
        if row == 0 || row > self.rows || col > self.cols {
            return None;
        }
        let mut pos = 0usize;
        let mut line = Vec::new();
        for _ in 0..row {
            line = read_line(&self.data, &mut pos, LINE_LIMIT);
        }
        let mut at = 0usize;
        if col > 0 {
            let mut seen = 1u32;
            while seen != col {
                seen += 1;
                let comma = line[at..].iter().position(|&b| b == b',')?;
                at += comma + 1;
            }
        }
        field_at(&line, at)
    }

    /// `f` cell as `f32` (`readFloat`).
    pub fn read_f32(&self, row: u32, col: u32) -> Option<f32> {
        self.cell_bytes(row, col).map(|b| atof(&b) as f32)
    }

    /// Boolean cell (`readBoolean`): `F`, `0` and `N` are false, other text true.
    pub fn read_bool(&self, row: u32, col: u32) -> Option<bool> {
        self.cell_bytes(row, col).map(|b| bool_value(&b, b"N"))
    }

    /// Integer cell as `i64` (`readLong`); hex with `0x` is accepted.
    pub fn read_i64(&self, row: u32, col: u32) -> Option<i64> {
        self.cell_bytes(row, col).map(|b| int_bits(&b) as i64)
    }

    /// Integer cell as `i32` (`readInt`).
    pub fn read_i32(&self, row: u32, col: u32) -> Option<i32> {
        self.read_i64(row, col).map(|v| v as i32)
    }

    /// Integer cell as `i16` (`readShort`).
    pub fn read_i16(&self, row: u32, col: u32) -> Option<i16> {
        self.cell_bytes(row, col).map(|b| int_bits(&b) as i16)
    }

    /// Integer cell as `i8` (`readChar`).
    pub fn read_i8(&self, row: u32, col: u32) -> Option<i8> {
        self.cell_bytes(row, col).map(|b| int_bits(&b) as i8)
    }

    /// Integer cell as `u64` (`readULong`).
    pub fn read_u64(&self, row: u32, col: u32) -> Option<u64> {
        self.cell_bytes(row, col).map(|b| int_bits(&b))
    }

    /// Integer cell as `u16` (`readUShort`).
    pub fn read_u16(&self, row: u32, col: u32) -> Option<u16> {
        self.cell_bytes(row, col).map(|b| int_bits(&b) as u16)
    }

    /// Integer cell as `u8` (`readUChar`).
    pub fn read_u8(&self, row: u32, col: u32) -> Option<u8> {
        self.cell_bytes(row, col).map(|b| int_bits(&b) as u8)
    }

    /// String cell (`readString`). The value ends at the first `"`. A value of
    /// `buffer_size` bytes or more is an error, as in the C code.
    pub fn read_string(&self, row: u32, col: u32, buffer_size: usize) -> Result<Option<String>> {
        let Some(cell) = self.cell_bytes(row, col) else {
            return Ok(None);
        };
        let cell = c_str(&cell);
        let take = cell
            .iter()
            .take_while(|&&b| b != b'"')
            .take(buffer_size)
            .count();
        if take == buffer_size {
            return Err(FormatError::BufferTooSmall { what: "CSV string" });
        }
        Ok(Some(String::from_utf8_lossy(&cell[..take]).into_owned()))
    }
}

/// `getNextWord` for a CSV field starting at `at` in `line`.
fn field_at(line: &[u8], at: usize) -> Option<Vec<u8>> {
    let rest = line.get(at..)?;
    let first = *rest.first()?;
    if first == b'/' || first == b',' {
        return None;
    }
    let skip = rest
        .iter()
        .position(|&b| !matches!(b, b' ' | b'\t' | b','))
        .unwrap_or(rest.len());
    let body = &rest[skip..];
    if body.is_empty() || body[0] == b'/' {
        return None;
    }
    let len = body.iter().position(|&b| b == b',').unwrap_or(body.len());
    if len > LINE_LIMIT {
        return None;
    }
    Some(body[..len].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &[u8] = b"name,speed,flag,hex\r\n\
        Atlas,2.5,TRUE,0x10\r\n\
        Hunter,,N,7\r\n\
        Spider,  9.0 ,0,/comment\r\n\
        last,1,1,1\r\n";

    #[test]
    fn counts_follow_the_game_rules() {
        let csv = CsvFile::from_bytes(SAMPLE.to_vec()).unwrap();
        // Five lines: countRows counts lines 2..=N, so rows 1..=4 are addressable
        // and line 5 is not. Columns: commas on line 1, so 1..=3 are addressable.
        assert_eq!(csv.rows(), 4);
        assert_eq!(csv.cols(), 3);
    }

    #[test]
    fn typed_cells_use_one_based_columns() {
        // Row 1 is the first line of the file (the header).
        let csv = CsvFile::from_bytes(SAMPLE.to_vec()).unwrap();
        assert_eq!(csv.cell(1, 1).as_deref(), Some("name"));
        assert_eq!(
            csv.cell(1, 0).as_deref(),
            Some("name"),
            "column 0 aliases column 1"
        );
        assert_eq!(csv.cell(2, 1).as_deref(), Some("Atlas"));
        assert_eq!(csv.read_f32(2, 2), Some(2.5));
        assert_eq!(csv.read_bool(2, 3), Some(true));
        assert_eq!(csv.read_f32(3, 2), None, "empty cell is absent");
        assert_eq!(csv.read_bool(3, 3), Some(false));
        assert_eq!(csv.read_f32(4, 2), Some(9.0), "leading blanks skipped");
        assert_eq!(csv.read_f32(2, 4), None, "column past the comma count");
        assert_eq!(csv.cell(5, 1), None, "last line is not addressable");
        assert_eq!(csv.cell(0, 1), None);
    }

    #[test]
    fn comment_fields_are_absent() {
        let csv = CsvFile::from_bytes(b"a,b,c\r\nx,/note,3\r\ny,z,w\r\n".to_vec()).unwrap();
        assert_eq!(csv.rows(), 2);
        assert_eq!(csv.read_i64(2, 2), None);
        assert_eq!(csv.cell(2, 1).as_deref(), Some("x"));
    }

    #[test]
    fn string_cells_stop_at_quote() {
        let csv =
            CsvFile::from_bytes(b"a,b,c\r\n\"quoted\",plain,z\r\nx,y,w\r\n".to_vec()).unwrap();
        assert_eq!(csv.read_string(2, 1, 32).unwrap(), Some(String::new()));
        assert_eq!(
            csv.read_string(2, 2, 32).unwrap(),
            Some("plain".to_string())
        );
        assert!(csv.read_string(2, 2, 5).is_err());
    }
}
