// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! FIT/INI text files (`FITini` ... `FITend`).
//!
//! Port of `mclib/inifile.{h,cpp}` (`FitIniFile`) and `mclib/dinifile.h`.
//!
//! A file is a header line `FITini`, then blocks. A block starts with a line
//! whose first character is `[`; the id runs up to the first `]`. Block ids
//! match case-sensitively. Keys are one per line, written as a type tag, a
//! space, the key name and `=`:
//!
//! ```text
//! f  name = 1.5            float (f32 or f64)
//! l  name = 12             long (i64); hex with 0x
//! ul / us / uc / s / c     unsigned long / unsigned short / unsigned char /
//!                          short / char
//! b  name = TRUE           boolean: FALSE or a leading 0 or F is false
//! st name = "text"         string; may span lines until a closing quote
//! f[3] name = 1, 2, 3      array: "[count]" then values split by space, tab
//!                          or comma; values may continue on following lines
//! ```
//!
//! Key names match case-insensitively. Lines are read as the game reads them:
//! at most 254 bytes (2047 for strings), CR and LF end a line, and a CR skips
//! the byte after it. The line-reading rules are kept so that odd files parse
//! the same way. The reader works on an in-memory image; the writer produces
//! the layout the game writes.

use crate::error::{FormatError, Result};
use crate::text::{self, atof, bool_value, c_str, find, int_bits, is_c_space};
use std::path::Path;

const FORMAT: &str = "FIT";
const HEADER: &[u8] = b"FITini";
/// Maximum line length for ordinary keys (C buffers of 255 bytes).
const KEY_LINE: usize = 254;
/// Maximum line length when a string value may continue.
const STRING_LINE: usize = 2047;
/// Maximum word length for array elements.
const ELEMENT_WORD: usize = 9;

#[derive(Debug, Clone)]
struct Block {
    id: String,
    offset: usize,
}

/// A parsed FIT/INI file with a current block selected by [`FitIni::seek_block`].
#[derive(Debug, Clone)]
pub struct FitIni {
    data: Vec<u8>,
    blocks: Vec<Block>,
    current: Option<usize>,
}

/// Reads one line the way `File::readLine` does on disk and in archives.
///
/// Returns the bytes before the terminator (truncated at a NUL) and advances
/// `pos` past the terminator. A CR also skips the next byte. At end of data the
/// position still advances by one, as the C code does.
pub(crate) fn read_line(data: &[u8], pos: &mut usize, max_len: usize) -> Vec<u8> {
    let start = *pos;
    let avail = data.len().saturating_sub(start);
    let limit = max_len.min(avail);
    let mut n = 0;
    while n < limit && data[start + n] != b'\r' && data[start + n] != b'\n' {
        n += 1;
    }
    let mut advance = n + 1;
    if n < limit && data[start + n] == b'\r' {
        advance += 1;
    }
    // `start` may lie past the end after an earlier read; the line is then empty.
    let begin = start.min(data.len());
    let line = text::c_str(&data[begin..begin + n]).to_vec();
    *pos = start + advance;
    line
}

/// Finds the next block header at or after `pos`, as `findNextBlockStart` does.
/// Returns the header line; `pos` is left just after it.
fn next_block_header(data: &[u8], pos: &mut usize) -> Option<Vec<u8>> {
    loop {
        let line = read_line(data, pos, KEY_LINE);
        if *pos >= data.len() {
            return None;
        }
        if line.first() == Some(&b'[') {
            return Some(line);
        }
    }
}

fn scan_blocks(data: &[u8]) -> Result<Vec<Block>> {
    let mut pos = 0usize;
    let header = read_line(data, &mut pos, 11);
    if find(&header, HEADER).is_none() {
        return Err(FormatError::BadSignature {
            format: FORMAT,
            found: String::from_utf8_lossy(&header).into_owned(),
        });
    }
    let mut blocks = Vec::new();
    while let Some(line) = next_block_header(data, &mut pos) {
        let close =
            line[1..]
                .iter()
                .position(|&b| b == b']')
                .ok_or_else(|| FormatError::Corrupt {
                    format: FORMAT,
                    detail: format!(
                        "block header without ']': {}",
                        String::from_utf8_lossy(&line)
                    ),
                })?;
        blocks.push(Block {
            id: String::from_utf8_lossy(&line[1..1 + close]).into_owned(),
            offset: pos,
        });
    }
    Ok(blocks)
}

/// `strncasecmp`-style key match followed by optional white space and `=`.
fn key_matches(line: &[u8], search: &[u8]) -> bool {
    if line.len() < search.len() || !line[..search.len()].eq_ignore_ascii_case(search) {
        return false;
    }
    let rest = &line[search.len()..];
    let skip = rest
        .iter()
        .position(|&b| !is_c_space(b))
        .unwrap_or(rest.len());
    rest.get(skip) == Some(&b'=')
}

/// Text after the first `=` in a line.
fn after_equals(line: &[u8]) -> Result<&[u8]> {
    find(line, b"=")
        .map(|i| &line[i + 1..])
        .ok_or_else(|| FormatError::Corrupt {
            format: FORMAT,
            detail: format!("no '=' in line: {}", String::from_utf8_lossy(line)),
        })
}

/// A key or array header found while scanning a block.
struct Hit {
    line: Vec<u8>,
    pos: usize,
    end: usize,
}

/// Element types that can be read from a FIT array.
pub trait FitArrayElement: Sized {
    /// Type tag preceding `[`, such as `f` or `ul`.
    const TAG: &'static str;
    /// Converts one array word the way the game's `textToX` helper does.
    fn parse_word(word: &[u8]) -> Self;
}

impl FitArrayElement for f32 {
    const TAG: &'static str = "f";
    fn parse_word(word: &[u8]) -> Self {
        atof(word) as f32
    }
}

impl FitArrayElement for i64 {
    const TAG: &'static str = "l";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word) as i64
    }
}

impl FitArrayElement for i32 {
    const TAG: &'static str = "l";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word) as i32
    }
}

impl FitArrayElement for u64 {
    const TAG: &'static str = "ul";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word)
    }
}

impl FitArrayElement for i16 {
    const TAG: &'static str = "s";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word) as i16
    }
}

impl FitArrayElement for u16 {
    const TAG: &'static str = "us";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word) as u16
    }
}

impl FitArrayElement for i8 {
    const TAG: &'static str = "c";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word) as i8
    }
}

impl FitArrayElement for u8 {
    const TAG: &'static str = "uc";
    fn parse_word(word: &[u8]) -> Self {
        int_bits(word) as u8
    }
}

/// Next array word, as `getNextWord`. `Ok(None)` means the caller should read
/// the next line (`GET_NEXT_LINE`).
fn next_word(text: &[u8], at: &mut usize) -> Result<Option<Vec<u8>>> {
    let is_sep = |b: u8| matches!(b, b' ' | b'\t' | b',');
    if *at >= text.len() || text[*at] == b'/' {
        return Ok(None);
    }
    while *at < text.len() && is_sep(text[*at]) {
        *at += 1;
    }
    if *at >= text.len() || text[*at] == b'/' {
        return Ok(None);
    }
    let start = *at;
    while *at < text.len() && !is_sep(text[*at]) {
        *at += 1;
    }
    let word = &text[start..*at];
    if word.len() > ELEMENT_WORD {
        return Err(FormatError::BufferTooSmall {
            what: "array element",
        });
    }
    Ok(Some(word.to_vec()))
}

impl FitIni {
    /// Parses a FIT file image. Fails when the first line does not contain `FITini`.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        let blocks = scan_blocks(&data)?;
        Ok(Self {
            data,
            blocks,
            current: None,
        })
    }

    /// Reads and parses a FIT file from disk.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_bytes(std::fs::read(path)?)
    }

    /// Number of blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Block ids in file order (duplicates included).
    pub fn blocks(&self) -> impl Iterator<Item = &str> + '_ {
        self.blocks.iter().map(|b| b.id.as_str())
    }

    /// Whether a block with exactly this id exists.
    pub fn has_block(&self, id: &str) -> bool {
        self.blocks.iter().any(|b| b.id == id)
    }

    /// Selects the first block whose id equals `id` (case-sensitive).
    pub fn seek_block(&mut self, id: &str) -> Result<()> {
        match self.blocks.iter().position(|b| b.id == id) {
            Some(i) => {
                self.current = Some(i);
                Ok(())
            }
            None => Err(FormatError::NotFound {
                what: format!("block [{id}]"),
            }),
        }
    }

    /// Id of the selected block.
    pub fn current_block(&self) -> Option<&str> {
        self.current.map(|i| self.blocks[i].id.as_str())
    }

    /// Byte range of the selected block's body, as `currentBlockOffset` and
    /// `currentBlockOffset + currentBlockSize`.
    fn block_range(&self) -> Result<(usize, usize)> {
        let i = self.current.ok_or_else(|| FormatError::NotFound {
            what: "no block selected".to_string(),
        })?;
        let start = self.blocks[i].offset;
        let end = self.blocks.get(i + 1).map_or(self.data.len(), |b| b.offset);
        Ok((start, end.max(start)))
    }

    /// Scans the selected block for a line matching `search`.
    ///
    /// The scan stops at a match or at the end of the block. As in the C code,
    /// a match on the very last line of the block counts as not found.
    fn find_key(&self, search: &[u8], max_len: usize) -> Result<Hit> {
        let (start, end) = self.block_range()?;
        let mut pos = start;
        let mut line;
        loop {
            line = read_line(&self.data, &mut pos, max_len);
            if key_matches(&line, search) || pos >= end {
                break;
            }
        }
        if pos >= end {
            return Err(FormatError::NotFound {
                what: format!("key '{}'", String::from_utf8_lossy(search)),
            });
        }
        Ok(Hit { line, pos, end })
    }

    fn scalar_line(&self, tag: &str, name: &str, max_len: usize) -> Result<Vec<u8>> {
        let search = format!("{tag} {name}");
        Ok(self.find_key(search.as_bytes(), max_len)?.line)
    }

    /// `f name` as `f32` (`readIdFloat`).
    pub fn read_f32(&self, name: &str) -> Result<f32> {
        let line = self.scalar_line("f", name, KEY_LINE)?;
        Ok(atof(after_equals(&line)?) as f32)
    }

    /// `f name` as `f64` (`readIdDouble`).
    pub fn read_f64(&self, name: &str) -> Result<f64> {
        let line = self.scalar_line("f", name, KEY_LINE)?;
        Ok(atof(after_equals(&line)?))
    }

    /// `b name` (`readIdBoolean`).
    pub fn read_bool(&self, name: &str) -> Result<bool> {
        let line = self.scalar_line("b", name, KEY_LINE)?;
        Ok(bool_value(after_equals(&line)?, b""))
    }

    /// `l name` as `i64` (`readIdLong`).
    pub fn read_i64(&self, name: &str) -> Result<i64> {
        let line = self.scalar_line("l", name, KEY_LINE)?;
        Ok(int_bits(after_equals(&line)?) as i64)
    }

    /// `l name` as `i32` (`readIdInt`).
    pub fn read_i32(&self, name: &str) -> Result<i32> {
        Ok(self.read_i64(name)? as i32)
    }

    /// `s name` (`readIdShort`).
    pub fn read_i16(&self, name: &str) -> Result<i16> {
        let line = self.scalar_line("s", name, KEY_LINE)?;
        Ok(int_bits(after_equals(&line)?) as i16)
    }

    /// `c name` (`readIdChar`, signed).
    pub fn read_i8(&self, name: &str) -> Result<i8> {
        let line = self.scalar_line("c", name, KEY_LINE)?;
        Ok(int_bits(after_equals(&line)?) as i8)
    }

    /// `ul name` (`readIdULong`).
    pub fn read_u64(&self, name: &str) -> Result<u64> {
        let line = self.scalar_line("ul", name, KEY_LINE)?;
        Ok(int_bits(after_equals(&line)?))
    }

    /// `ul name` truncated to 32 bits (the `DWORD` overload).
    pub fn read_u32(&self, name: &str) -> Result<u32> {
        Ok(self.read_u64(name)? as u32)
    }

    /// `us name` (`readIdUShort`).
    pub fn read_u16(&self, name: &str) -> Result<u16> {
        let line = self.scalar_line("us", name, KEY_LINE)?;
        Ok(int_bits(after_equals(&line)?) as u16)
    }

    /// `uc name` (`readIdUChar`).
    pub fn read_u8(&self, name: &str) -> Result<u8> {
        let line = self.scalar_line("uc", name, KEY_LINE)?;
        Ok(int_bits(after_equals(&line)?) as u8)
    }

    /// Length in bytes of `st name` plus one, as `getIdStringLength` returns it.
    pub fn string_length(&self, name: &str) -> Result<usize> {
        let line = self.scalar_line("st", name, KEY_LINE)?;
        let open = line
            .iter()
            .position(|&b| b == b'"')
            .ok_or_else(|| FormatError::Corrupt {
                format: FORMAT,
                detail: format!("string '{name}' has no opening quote"),
            })?;
        let body = &line[open + 1..];
        let close = body
            .iter()
            .position(|&b| b == b'"')
            .ok_or_else(|| FormatError::Corrupt {
                format: FORMAT,
                detail: format!("string '{name}' has no closing quote"),
            })?;
        Ok(close + 1)
    }

    /// `st name` (`readIdString`). `buffer_size` mirrors the C buffer: the
    /// value must be shorter than it, otherwise `BufferTooSmall` is returned.
    ///
    /// A string whose closing quote is on a later line is joined with the
    /// lines in between. Each intermediate line gets a `\n` appended, and the
    /// closing line does not.
    pub fn read_string(&self, name: &str, buffer_size: usize) -> Result<String> {
        let search = format!("st {name}");
        let Hit { line, mut pos, end } = self.find_key(search.as_bytes(), STRING_LINE)?;
        let eq = find(&line, b"=").ok_or_else(|| FormatError::Corrupt {
            format: FORMAT,
            detail: format!("string '{name}' has no '='"),
        })?;
        let mut value = line[eq + 1..].to_vec();

        if let Some(first) = value.iter().position(|&b| b == b'"') {
            if !value[first + 1..].contains(&b'"') {
                let mut cur_len = value.len();
                loop {
                    let next = read_line(&self.data, &mut pos, STRING_LINE);
                    let found = next.contains(&b'"');
                    let added = next.len() + 1;
                    if cur_len + added >= 2048 {
                        break;
                    }
                    value.extend_from_slice(&next);
                    value.push(b'\n');
                    cur_len += added;
                    if found || pos >= end {
                        break;
                    }
                }
            }
        }

        let value = c_str(&value);
        let open = find(value, b"\"").ok_or_else(|| FormatError::Corrupt {
            format: FORMAT,
            detail: format!("string '{name}' has no opening quote"),
        })?;
        let body = &value[open + 1..];
        let Some(close) = body.iter().position(|&b| b == b'"') else {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("string '{name}' is unterminated"),
            });
        };
        if close >= buffer_size {
            return Err(FormatError::BufferTooSmall { what: "string" });
        }
        Ok(String::from_utf8_lossy(&body[..close]).into_owned())
    }

    /// Element count written in `[n]` of `tag[n] name`, as the
    /// `get*ArrayElements` functions return it.
    pub fn array_len<T: FitArrayElement>(&self, name: &str) -> Result<usize> {
        Ok(self.array_header::<T>(name)?.count)
    }

    /// Reads `tag[n] name = v1, v2, ...` into a `Vec`, refusing when `n`
    /// exceeds `capacity` (`USER_ARRAY_TOO_SMALL`).
    pub fn read_array<T: FitArrayElement>(&self, name: &str, capacity: usize) -> Result<Vec<T>> {
        let header = self.array_header::<T>(name)?;
        if header.count > capacity {
            return Err(FormatError::BufferTooSmall { what: "array" });
        }
        let Hit { line, mut pos, end } = header.hit;
        let eq = find(&line, b"=").ok_or_else(|| FormatError::Corrupt {
            format: FORMAT,
            detail: format!("array '{name}' has no '='"),
        })?;
        let mut text = line[eq + 1..].to_vec();
        let mut at = 0usize;
        // The count is checked against the caller capacity above; reserve no more than a small bound up front.
        let mut out = Vec::with_capacity(header.count.min(256));
        while pos < end && out.len() < header.count {
            match next_word(&text, &mut at)? {
                Some(word) => out.push(T::parse_word(&word)),
                None => {
                    text = read_line(&self.data, &mut pos, KEY_LINE);
                    at = 0;
                }
            }
        }
        if pos >= end && out.len() < header.count {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!(
                    "array '{name}' has {} of {} elements",
                    out.len(),
                    header.count
                ),
            });
        }
        Ok(out)
    }

    fn array_header<T: FitArrayElement>(&self, name: &str) -> Result<ArrayHeader> {
        let (start, end) = self.block_range()?;
        let front = format!("{}[", T::TAG);
        let back = format!("] {name}");
        let mut pos = start;
        let mut line;
        let mut found;
        loop {
            line = read_line(&self.data, &mut pos, KEY_LINE);
            found = find(&line, front.as_bytes()).zip(find(&line, back.as_bytes()));
            if found.is_some() || pos >= end {
                break;
            }
        }
        let Some((f, b)) = found.filter(|_| pos < end) else {
            return Err(FormatError::NotFound {
                what: format!("array '{name}'"),
            });
        };
        let digits_start = f + front.len();
        if b < digits_start {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("array '{name}' has malformed brackets"),
            });
        }
        let digits = &line[digits_start..b];
        if digits.len() > 9 {
            return Err(FormatError::BufferTooSmall {
                what: "array count",
            });
        }
        // The count is decimal (or hex) text. A negative value is malformed,
        // not a huge count.
        let raw = int_bits(digits) as i64;
        if raw < 0 {
            return Err(FormatError::Corrupt {
                format: FORMAT,
                detail: format!("array '{name}' has a negative element count"),
            });
        }
        let count = raw as usize;
        Ok(ArrayHeader {
            count,
            hit: Hit { line, pos, end },
        })
    }
}

struct ArrayHeader {
    count: usize,
    hit: Hit,
}

/// Builds a FIT file in the layout the game writes.
///
/// Each block is preceded by a blank line, and the file ends with the
/// `FITend` footer. Array formatting copies the C writers: `f` and `uc` put a
/// space before the second and later elements, while `l` and `us` do not.
#[derive(Debug, Clone)]
pub struct FitWriter {
    buf: Vec<u8>,
}

impl Default for FitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl FitWriter {
    /// Starts a file with the `FITini` header.
    pub fn new() -> Self {
        Self {
            buf: b"FITini \r\n".to_vec(),
        }
    }

    fn line(&mut self, text: &str) {
        self.buf.extend_from_slice(text.as_bytes());
        self.buf.extend_from_slice(b"\r\n");
    }

    /// Starts a block: a blank line, then `[id]`.
    pub fn block(&mut self, id: &str) {
        self.buf.extend_from_slice(b"\r\n");
        self.line(&format!("[{id}]"));
    }

    /// `f name = value` with six decimals, as `%f`.
    pub fn write_f32(&mut self, name: &str, value: f32) {
        self.line(&format!("f {name} = {value:.6}"));
    }

    /// `b name = TRUE` or `FALSE`.
    pub fn write_bool(&mut self, name: &str, value: bool) {
        self.line(&format!(
            "b {name} = {}",
            if value { "TRUE" } else { "FALSE" }
        ));
    }

    /// `l name = value`.
    pub fn write_i64(&mut self, name: &str, value: i64) {
        self.line(&format!("l {name} = {value}"));
    }

    /// `s name = value`.
    pub fn write_i16(&mut self, name: &str, value: i16) {
        self.line(&format!("s {name} = {value}"));
    }

    /// `c name = value` (signed decimal).
    pub fn write_i8(&mut self, name: &str, value: i8) {
        self.line(&format!("c {name} = {value}"));
    }

    /// `ul name = value`.
    pub fn write_u64(&mut self, name: &str, value: u64) {
        self.line(&format!("ul {name} = {value}"));
    }

    /// `us name = value`.
    pub fn write_u16(&mut self, name: &str, value: u16) {
        self.line(&format!("us {name} = {value}"));
    }

    /// `uc name = value`.
    pub fn write_u8(&mut self, name: &str, value: u8) {
        self.line(&format!("uc {name} = {value}"));
    }

    /// `st name = "value"`. The value is written without escaping.
    pub fn write_string(&mut self, name: &str, value: &str) {
        self.line(&format!("st {name} = \"{value}\""));
    }

    /// `f[n] name = a.aa, b.bb, ...` (two decimals, `, ` between elements).
    pub fn write_f32_array(&mut self, name: &str, values: &[f32]) {
        let mut s = format!("f[{}] {name} = ", values.len());
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&format!("{v:.2},"));
        }
        self.line(&s);
    }

    /// `l[n] name = a,b,c` (no spaces).
    pub fn write_i64_array(&mut self, name: &str, values: &[i64]) {
        let mut s = format!("l[{}] {name} = ", values.len());
        for v in values {
            s.push_str(&format!("{v},"));
        }
        self.line(&s);
    }

    /// `us[n] name = a,b,c` (no spaces).
    pub fn write_u16_array(&mut self, name: &str, values: &[u16]) {
        let mut s = format!("us[{}] {name} = ", values.len());
        for v in values {
            s.push_str(&format!("{v},"));
        }
        self.line(&s);
    }

    /// `uc[n] name = a, b, c` (a space before every element after the first).
    pub fn write_u8_array(&mut self, name: &str, values: &[u8]) {
        let mut s = format!("uc[{}] {name} = ", values.len());
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(&format!("{v},"));
        }
        self.line(&s);
    }

    /// Appends the `FITend` footer and returns the file image.
    pub fn finish(mut self) -> Vec<u8> {
        self.buf.extend_from_slice(b"FITend \r\n");
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        b"FITini \r\n\
          [alpha]\r\n\
          f speed = 2.5\r\n\
          l Count = 0x1F // hex\r\n\
          b flag = FALSE\r\n\
          st title = \"Hello World\"\r\n\
          f[3] pos = 1.0, 2.0\r\n\
          \t 3.5\r\n\
          ul[2] big = 400000000 , 7\r\n\
          uc[2] small = 200 201\r\n\
          [Beta]\r\n\
          s neg = -12\r\n\
          st multi = \"line one\r\n\
          line two\"\r\n\
          FITend \r\n"
            .to_vec()
    }

    #[test]
    fn blocks_are_listed_in_order_and_case_sensitive() {
        let mut ini = FitIni::from_bytes(sample()).unwrap();
        assert_eq!(ini.blocks().collect::<Vec<_>>(), ["alpha", "Beta"]);
        assert!(ini.seek_block("alpha").is_ok());
        assert!(matches!(
            ini.seek_block("ALPHA"),
            Err(FormatError::NotFound { .. })
        ));
        assert_eq!(ini.current_block(), Some("alpha"));
    }

    #[test]
    fn scalars_are_typed_and_keys_case_insensitive() {
        let mut ini = FitIni::from_bytes(sample()).unwrap();
        ini.seek_block("alpha").unwrap();
        assert_eq!(ini.read_f32("speed").unwrap(), 2.5);
        assert_eq!(ini.read_f32("SPEED").unwrap(), 2.5);
        assert_eq!(ini.read_i64("count").unwrap(), 31);
        assert_eq!(ini.read_i32("count").unwrap(), 31);
        assert!(!ini.read_bool("flag").unwrap());
        assert!(matches!(
            ini.read_u64("big"),
            Err(FormatError::NotFound { .. })
        ));
        assert!(matches!(
            ini.read_i64("speed"),
            Err(FormatError::NotFound { .. })
        ));
    }

    #[test]
    fn strings_arrays_and_multiline() {
        let mut ini = FitIni::from_bytes(sample()).unwrap();
        ini.seek_block("alpha").unwrap();
        assert_eq!(ini.read_string("title", 64).unwrap(), "Hello World");
        assert_eq!(ini.string_length("title").unwrap(), 12);
        assert!(matches!(
            ini.read_string("title", 11),
            Err(FormatError::BufferTooSmall { .. })
        ));
        assert_eq!(
            ini.read_array::<f32>("pos", 3).unwrap(),
            vec![1.0, 2.0, 3.5]
        );
        assert_eq!(ini.array_len::<f32>("pos").unwrap(), 3);
        assert!(matches!(
            ini.read_array::<f32>("pos", 2),
            Err(FormatError::BufferTooSmall { .. })
        ));
        assert_eq!(ini.read_array::<u8>("small", 2).unwrap(), vec![200, 201]);
        assert_eq!(
            ini.read_array::<u64>("big", 2).unwrap(),
            vec![400_000_000, 7]
        );
        // Words longer than nine bytes overflow the C element buffer.
        let mut long = FitIni::from_bytes(sample_with_long_word()).unwrap();
        long.seek_block("alpha").unwrap();
        assert!(matches!(
            long.read_array::<u64>("big", 1),
            Err(FormatError::BufferTooSmall { .. })
        ));

        ini.seek_block("Beta").unwrap();
        assert_eq!(ini.read_i16("neg").unwrap(), -12);
        // Quirk kept from the C code: the newline is appended after each added
        // line, so the first continuation line joins with no separator.
        assert_eq!(ini.read_string("multi", 100).unwrap(), "line oneline two");
    }

    #[test]
    fn writer_output_reads_back() {
        let mut w = FitWriter::new();
        w.block("setup");
        w.write_f32("gain", 0.25);
        w.write_bool("on", true);
        w.write_i64("count", -40);
        w.write_i16("short", -3);
        w.write_i8("ch", -5);
        w.write_u64("total", 9_999_999_999);
        w.write_u16("port", 65535);
        w.write_u8("level", 7);
        w.write_string("name", "Mech");
        w.write_f32_array("v", &[1.0, 2.5, -0.5]);
        w.write_i64_array("ids", &[1, -2, 3]);
        w.write_u16_array("u16s", &[9, 10]);
        w.write_u8_array("bytes", &[1, 2, 3]);
        let image = w.finish();
        assert!(image.starts_with(b"FITini \r\n"));
        assert!(image.ends_with(b"FITend \r\n"));

        let mut ini = FitIni::from_bytes(image).unwrap();
        ini.seek_block("setup").unwrap();
        assert_eq!(ini.read_f32("gain").unwrap(), 0.25);
        assert!(ini.read_bool("on").unwrap());
        assert_eq!(ini.read_i64("count").unwrap(), -40);
        assert_eq!(ini.read_i16("short").unwrap(), -3);
        assert_eq!(ini.read_i8("ch").unwrap(), -5);
        assert_eq!(ini.read_u64("total").unwrap(), 9_999_999_999);
        assert_eq!(ini.read_u16("port").unwrap(), 65535);
        assert_eq!(ini.read_u8("level").unwrap(), 7);
        assert_eq!(ini.read_string("name", 16).unwrap(), "Mech");
        assert_eq!(ini.read_array::<f32>("v", 3).unwrap(), vec![1.0, 2.5, -0.5]);
        assert_eq!(ini.read_array::<i64>("ids", 3).unwrap(), vec![1, -2, 3]);
        assert_eq!(ini.read_array::<u16>("u16s", 2).unwrap(), vec![9, 10]);
        assert_eq!(ini.read_array::<u8>("bytes", 3).unwrap(), vec![1, 2, 3]);
    }

    fn sample_with_long_word() -> Vec<u8> {
        b"FITini \r\n[alpha]\r\nul[1] big = 4000000000\r\nFITend \r\n".to_vec()
    }

    #[test]
    fn rejects_non_fit_files_and_unterminated_blocks() {
        assert!(matches!(
            FitIni::from_bytes(b"NOPE\r\n[x]\r\n".to_vec()),
            Err(FormatError::BadSignature { .. })
        ));
        assert!(matches!(
            FitIni::from_bytes(b"FITini \r\n[broken\r\nFITend \r\n".to_vec()),
            Err(FormatError::Corrupt { .. })
        ));
    }

    #[test]
    fn crlf_and_lf_both_parse() {
        let lf = b"FITini\n[a]\nl n = 5\nFITend\n".to_vec();
        let mut ini = FitIni::from_bytes(lf).unwrap();
        ini.seek_block("a").unwrap();
        assert_eq!(ini.read_i64("n").unwrap(), 5);
    }
}
