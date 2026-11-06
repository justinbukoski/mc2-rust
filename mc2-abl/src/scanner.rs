// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The ABL scanner (`mclib/ablscan.cpp`): a character-level state machine over source lines with `#` directives,
//! comments, include files and the reserved-word table.
//!
//! The port keeps the C++ structure on purpose (shared line buffer, `DumbGetCharOn`, the nested `getChar` on include
//! open/close) because the observable behaviour of comments, directives and includes depends on it.

use std::rc::Rc;

use crate::error::{AblError, AblResult, SyntaxCode};
use crate::source::{normalize_path, SourceProvider};

/// `CHAR_EOF` from `ablgen.h`.
pub const CHAR_EOF: u8 = 0x7f;
const TAB_SIZE: i32 = 4;
const MAX_DIGIT_COUNT: i32 = 20;
const MAX_EXPONENT: i32 = 20;
/// `MAXLEN_SOURCELINE`.
const MAXLEN_SOURCELINE: usize = 2048;
/// `MAX_INCLUDE_DEPTH`.
pub const MAX_INCLUDE_DEPTH: usize = 6;
/// `MAX_SOURCE_FILES`.
pub const MAX_SOURCE_FILES: usize = 256;

/// Token codes (`TokenCodeType`). Only the tokens the scanner produces are listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[allow(missing_docs)]
pub enum Tok {
    None,
    Identifier,
    Number,
    Type,
    String,
    Star,
    LParen,
    RParen,
    Minus,
    Plus,
    Equal,
    LBracket,
    RBracket,
    Colon,
    Semicolon,
    Lt,
    Gt,
    Comma,
    Period,
    FSlash,
    EqualEqual,
    Le,
    Ge,
    Ne,
    Eof,
    Error,
    Code,
    Order,
    State,
    And,
    Switch,
    Case,
    Const,
    Div,
    Do,
    Of,
    Else,
    EndIf,
    EndWhile,
    EndFor,
    EndFunction,
    EndOrder,
    EndState,
    EndModule,
    EndFsm,
    EndLibrary,
    EndCase,
    EndSwitch,
    For,
    Function,
    If,
    Mod,
    Not,
    Or,
    Repeat,
    Then,
    To,
    Until,
    Var,
    Ref,
    While,
    Elsif,
    Module,
    Fsm,
    Trans,
    TransBack,
    Library,
    Eternal,
    Static,
    Pound,
}

/// A numeric or string literal as scanned.
#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    /// Integer literal.
    Int(i32),
    /// Real literal (single precision, as the C++ `float`).
    Real(f32),
    /// String literal bytes (without quotes).
    Str(Vec<u8>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharCode {
    Letter,
    Digit,
    DQuote,
    Special,
    Eof,
}

fn char_code(c: u8) -> CharCode {
    match c {
        b'a'..=b'z' | b'A'..=b'Z' => CharCode::Letter,
        b'0'..=b'9' => CharCode::Digit,
        b'"' => CharCode::DQuote,
        CHAR_EOF => CharCode::Eof,
        _ => CharCode::Special,
    }
}

fn reserved_word(word: &str) -> Option<Tok> {
    // Same set (and the same 2..=11 length window) as the C++ `reservedWordTable`.
    Some(match word {
        "if" => Tok::If,
        "or" => Tok::Or,
        "do" => Tok::Do,
        "to" => Tok::To,
        "and" => Tok::And,
        "for" => Tok::For,
        "mod" => Tok::Mod,
        "not" => Tok::Not,
        "var" => Tok::Var,
        "fsm" => Tok::Fsm,
        "else" => Tok::Else,
        "then" => Tok::Then,
        "case" => Tok::Case,
        "code" => Tok::Code,
        "type" => Tok::Type,
        "const" => Tok::Const,
        "until" => Tok::Until,
        "while" => Tok::While,
        "endif" => Tok::EndIf,
        "order" => Tok::Order,
        "state" => Tok::State,
        "trans" => Tok::Trans,
        "module" => Tok::Module,
        "repeat" => Tok::Repeat,
        "endfor" => Tok::EndFor,
        "switch" => Tok::Switch,
        "static" => Tok::Static,
        "endfsm" => Tok::EndFsm,
        "endcase" => Tok::EndCase,
        "eternal" => Tok::Eternal,
        "library" => Tok::Library,
        "function" => Tok::Function,
        "endwhile" => Tok::EndWhile,
        "endorder" => Tok::EndOrder,
        "endstate" => Tok::EndState,
        "endswitch" => Tok::EndSwitch,
        "endmodule" => Tok::EndModule,
        "transback" => Tok::TransBack,
        "endlibrary" => Tok::EndLibrary,
        "endfunction" => Tok::EndFunction,
        _ => return None,
    })
}

struct OpenFile {
    data: Rc<Vec<u8>>,
    pos: usize,
    file_number: i32,
    /// Saved line number of the *parent* when a nested file was opened (restored on close).
    line_number: i32,
}

/// The scanner state (`ablscan.cpp` globals gathered into a struct).
pub struct Scanner {
    provider: Rc<dyn SourceProvider>,
    open_files: Vec<OpenFile>,
    /// `SourceFiles[]`: every file opened for the module being compiled; index 0 is the module itself.
    pub source_files: Vec<String>,
    buffer: Vec<u8>,
    bufferp: usize,
    buffer_offset: i32,
    /// Current character (`curChar`).
    pub cur_char: u8,
    /// Current token (`curToken`).
    pub tok: Tok,
    /// Current literal (`curLiteral`).
    pub literal: Literal,
    /// True when the current literal is real (otherwise integer), valid for `Tok::Number`.
    token_text: String,
    /// Lower-cased last word (`wordString`).
    pub word: String,
    /// Current line number (`lineNumber`).
    pub line_number: i32,
    /// Current file number (`FileNumber`).
    pub file_number: i32,
    dumb: bool,
    digit_count: i32,
    count_error: bool,
    /// `AssertEnabled`.
    pub assert_enabled: bool,
    /// `PrintEnabled`.
    pub print_enabled: bool,
    /// `StringFunctionsEnabled`.
    pub string_functions_enabled: bool,
    /// `DebugCodeEnabled`.
    pub debug_code_enabled: bool,
    /// The word that ends the module (`endmodule`, `endlibrary` or `endfsm`) and must not swallow a following `.`.
    pub end_word: &'static str,
    /// Number of lines read, summed over all files (for statistics).
    pub lines_read: usize,
}

type Res<T> = AblResult<T>;

impl Scanner {
    /// Create a scanner with no file open.
    pub fn new(provider: Rc<dyn SourceProvider>) -> Scanner {
        Scanner {
            provider,
            open_files: Vec::new(),
            source_files: Vec::new(),
            buffer: Vec::new(),
            bufferp: 0,
            buffer_offset: 0,
            cur_char: b' ',
            tok: Tok::None,
            literal: Literal::Int(0),
            token_text: String::new(),
            word: String::new(),
            line_number: 0,
            file_number: 0,
            dumb: false,
            digit_count: 0,
            count_error: false,
            assert_enabled: false,
            print_enabled: false,
            string_functions_enabled: true,
            debug_code_enabled: false,
            end_word: "endmodule",
            lines_read: 0,
        }
    }

    /// Reset per-module state (the start of `ABLi_preProcess`).
    pub fn reset_for_module(&mut self, debugger_active: bool) {
        self.open_files.clear();
        self.source_files.clear();
        self.buffer.clear();
        self.bufferp = 0;
        self.buffer_offset = 0;
        self.line_number = 0;
        self.file_number = 0;
        self.dumb = false;
        self.digit_count = 0;
        self.count_error = false;
        self.assert_enabled = debugger_active;
        self.print_enabled = debugger_active;
        self.string_functions_enabled = true;
        self.debug_code_enabled = debugger_active;
        self.word.clear();
        self.token_text.clear();
        self.tok = Tok::None;
        self.cur_char = b' ';
        self.end_word = "endmodule";
        self.lines_read = 0;
    }

    /// Build a syntax error carrying the current file/line/word like `syntaxError()`.
    pub fn syntax_error(&self, code: SyntaxCode) -> AblError {
        let file = self
            .source_files
            .get(self.file_number.max(0) as usize)
            .map(String::as_str)
            .unwrap_or("");
        AblError::syntax(code, file, self.line_number.max(0) as u32, &self.word)
    }

    /// Number of currently open files.
    pub fn num_open_files(&self) -> usize {
        self.open_files.len()
    }

    // ------------------------------------------------------------------------------------------
    // source file management
    // ------------------------------------------------------------------------------------------

    /// `openSourceFile`. On success the first character is already loaded.
    pub fn open_source_file(&mut self, name: &str) -> Res<()> {
        if self.open_files.len() == MAX_INCLUDE_DEPTH || self.source_files.len() == MAX_SOURCE_FILES
        {
            return Err(self.syntax_error(SyntaxCode::SourceFileOpen));
        }
        let norm = normalize_path(name);
        let data = match self.provider.read(&norm) {
            Some(d) => Rc::new(d),
            None => {
                return Err(AblError::fatal(format!(
                    " unable to open ABL File {}",
                    name
                )))
            }
        };
        self.source_files.push(name.to_string());
        self.file_number = (self.source_files.len() - 1) as i32;
        if let Some(parent) = self.open_files.last_mut() {
            parent.line_number = self.line_number;
        }
        self.open_files.push(OpenFile {
            data,
            pos: 0,
            file_number: self.file_number,
            line_number: 0,
        });
        self.line_number = 0;
        self.buffer.clear();
        self.bufferp = 0;
        self.get_char()
    }

    /// `closeSourceFile`.
    pub fn close_source_file(&mut self) {
        if self.open_files.pop().is_none() {
            return;
        }
        if let Some(parent) = self.open_files.last() {
            self.file_number = parent.file_number;
            self.line_number = parent.line_number;
        }
    }

    /// `getSourceLine`: read the next line (including its newline) into the shared line buffer.
    fn get_source_line(&mut self) -> bool {
        let Some(f) = self.open_files.last_mut() else {
            return false;
        };
        if f.pos >= f.data.len() {
            return false;
        }
        let rest = &f.data[f.pos..];
        let limit = rest.len().min(MAXLEN_SOURCELINE);
        let mut n = 0;
        while n < limit && rest[n] != b'\n' {
            n += 1;
        }
        if n < rest.len() && rest[n] == b'\n' {
            n += 1;
        }
        self.buffer.clear();
        self.buffer.extend_from_slice(&rest[..n]);
        f.pos += n;
        self.line_number += 1;
        self.lines_read += 1;
        true
    }

    // ------------------------------------------------------------------------------------------
    // character level
    // ------------------------------------------------------------------------------------------

    fn skip_line_comment(&mut self) {
        // `*bufferp = '\0'`: drop the rest of the current line.
        self.buffer.truncate(self.bufferp);
        self.cur_char = b' ';
    }

    fn skip_block_comment(&mut self) -> Res<()> {
        self.dumb = true;
        self.get_char()?;
        self.get_char()?;
        loop {
            while self.cur_char != b'*' && self.cur_char != CHAR_EOF {
                self.get_char()?;
            }
            if self.cur_char == CHAR_EOF {
                return Err(self.syntax_error(SyntaxCode::UnexpectedEof));
            }
            self.get_char()?;
            if self.cur_char == b'/' {
                break;
            }
        }
        self.cur_char = b' ';
        self.dumb = false;
        Ok(())
    }

    /// `skipBlanks`.
    pub fn skip_blanks(&mut self) -> Res<()> {
        while self.cur_char == b' ' {
            self.get_char()?;
        }
        Ok(())
    }

    fn read_directive_word(&mut self) -> Res<String> {
        let mut d = String::new();
        while self.cur_char != b' '
            && self.cur_char != b'\n'
            && self.cur_char != b'\r'
            && d.len() < 31
        {
            d.push(self.cur_char as char);
            self.get_char()?;
        }
        Ok(d.to_ascii_lowercase())
    }

    fn read_include_name(&mut self) -> Res<Option<String>> {
        // after the directive word: skip one char, expect an opening quote.
        self.get_char()?;
        if self.cur_char != b'"' {
            return Ok(None);
        }
        self.get_char()?;
        let mut name = String::new();
        while self.cur_char != b'"' && name.len() < 127 {
            name.push(self.cur_char as char);
            self.get_char()?;
        }
        Ok(Some(name))
    }

    fn language_directive(&mut self) -> Res<()> {
        self.dumb = true;
        self.get_char()?;
        let directive = self.read_directive_word()?;
        match directive.as_str() {
            "include" => match self.read_include_name()? {
                Some(name) => {
                    self.dumb = false;
                    self.open_source_file(&name)?;
                }
                None => return Err(self.syntax_error(SyntaxCode::BadLanguageDirectiveUsage)),
            },
            "include_" => match self.read_include_name()? {
                Some(name) => {
                    self.dumb = false;
                    // Resolve relative to the directory of the module's own source file (SourceFiles[0]).
                    let base = self.source_files.first().cloned().unwrap_or_default();
                    let full = match base.rfind(['\\', '/']) {
                        Some(i) => format!("{}{}", &base[..=i], name),
                        None => name,
                    };
                    self.open_source_file(&full)?;
                }
                None => return Err(self.syntax_error(SyntaxCode::BadLanguageDirectiveUsage)),
            },
            "assert_on" | "assert_off" | "print_on" | "print_off" | "stringfuncs_on"
            | "stringfuncs_off" | "debug_on" | "debug_off" => {
                self.dumb = false;
                self.cur_char = b' ';
                let on = directive.ends_with("_on");
                match directive.as_str() {
                    "assert_on" | "assert_off" => self.assert_enabled = on,
                    "print_on" | "print_off" => self.print_enabled = on,
                    "stringfuncs_on" | "stringfuncs_off" => self.string_functions_enabled = on,
                    _ => self.debug_code_enabled = on,
                }
            }
            "debug_start" => {
                if !self.debug_code_enabled {
                    let mut nested = 1;
                    self.get_char()?;
                    let mut still_cutting = true;
                    while still_cutting {
                        while self.cur_char != b'#' && self.cur_char != CHAR_EOF {
                            self.get_char()?;
                        }
                        if self.cur_char == b'#' {
                            let mut d2 = String::new();
                            while self.cur_char != b' '
                                && self.cur_char != b'\n'
                                && self.cur_char != b'\r'
                                && d2.len() < 31
                            {
                                d2.push(self.cur_char as char);
                                self.get_char()?;
                            }
                            let d2 = d2.to_ascii_lowercase();
                            if d2 == "#debug_start" {
                                nested += 1;
                            } else if d2 == "#debug_end" {
                                nested -= 1;
                                if nested == 0 {
                                    still_cutting = false;
                                }
                            }
                        } else if self.cur_char == CHAR_EOF {
                            return Err(self.syntax_error(SyntaxCode::UnexpectedEof));
                        } else {
                            self.get_char()?;
                        }
                    }
                }
                self.cur_char = b' ';
                self.dumb = false;
            }
            "debug_end" => {
                self.cur_char = b' ';
                self.dumb = false;
            }
            _ => return Err(self.syntax_error(SyntaxCode::UnknownLanguageDirective)),
        }
        Ok(())
    }

    /// `getChar`.
    pub fn get_char(&mut self) -> Res<()> {
        if self.bufferp >= self.buffer.len() {
            if !self.get_source_line() {
                if self.open_files.len() > 1 {
                    self.close_source_file();
                    return Ok(());
                }
                self.cur_char = CHAR_EOF;
                return Ok(());
            }
            self.bufferp = 0;
            self.buffer_offset = 0;
        }
        self.cur_char = self.buffer[self.bufferp];
        self.bufferp += 1;

        if self.dumb {
            self.buffer_offset += 1;
            return Ok(());
        }

        match self.cur_char {
            b'\t' => {
                self.buffer_offset += TAB_SIZE - self.buffer_offset % TAB_SIZE;
                self.cur_char = b' ';
            }
            b'\n' | b'\r' => {
                self.buffer_offset += 1;
                self.cur_char = b' ';
            }
            b'/' => {
                let next = self.buffer.get(self.bufferp).copied();
                if next == Some(b'/') {
                    self.skip_line_comment();
                } else if next == Some(b'*') {
                    self.skip_block_comment()?;
                } else {
                    self.buffer_offset += 1;
                }
            }
            b'#' => self.language_directive()?,
            _ => self.buffer_offset += 1,
        }
        Ok(())
    }

    // ------------------------------------------------------------------------------------------
    // tokens
    // ------------------------------------------------------------------------------------------

    /// `getToken`.
    pub fn get_token(&mut self) -> Res<()> {
        self.skip_blanks()?;
        self.token_text.clear();
        match char_code(self.cur_char) {
            CharCode::Letter => self.get_word(),
            CharCode::Digit => self.get_number(),
            CharCode::DQuote => self.get_string(),
            CharCode::Eof => {
                self.tok = Tok::Eof;
                Ok(())
            }
            CharCode::Special => self.get_special(),
        }
    }

    fn word_char(&self) -> bool {
        matches!(char_code(self.cur_char), CharCode::Letter | CharCode::Digit)
            || self.cur_char == b'_'
    }

    fn get_word(&mut self) -> Res<()> {
        while self.word_char() {
            self.token_text.push(self.cur_char as char);
            self.get_char()?;
        }
        self.word = self.token_text.to_ascii_lowercase();

        if self.cur_char == b'.' && self.word != self.end_word {
            // Library-qualified identifier (`lib.member`): keep reading.
            self.token_text.push('.');
            self.get_char()?;
            while self.word_char() {
                self.token_text.push(self.cur_char as char);
                self.get_char()?;
            }
            self.word = self.token_text.to_ascii_lowercase();
        }

        let len = self.word.len();
        match (2..=11)
            .contains(&len)
            .then(|| reserved_word(&self.word))
            .flatten()
        {
            Some(t) => self.tok = t,
            None => self.tok = Tok::Identifier,
        }
        Ok(())
    }

    fn accumulate_value(&mut self, value: &mut f32, err: SyntaxCode) -> Res<()> {
        if char_code(self.cur_char) != CharCode::Digit {
            return Err(self.syntax_error(err));
        }
        loop {
            self.token_text.push(self.cur_char as char);
            self.digit_count += 1;
            if self.digit_count <= MAX_DIGIT_COUNT {
                *value = 10.0f32 * *value + (self.cur_char - b'0') as f32;
            } else {
                self.count_error = true;
            }
            self.get_char()?;
            if char_code(self.cur_char) != CharCode::Digit {
                break;
            }
        }
        Ok(())
    }

    fn get_number(&mut self) -> Res<()> {
        let mut number_value: f32 = 0.0;
        let mut exponent_value: f32 = 0.0;
        self.digit_count = 0;
        self.count_error = false;
        self.tok = Tok::None;
        let mut is_real = false;
        let mut decimal_offset: i32 = 0;

        self.accumulate_value(&mut number_value, SyntaxCode::InvalidNumber)?;
        let whole_count = self.digit_count;

        if self.cur_char == b'.' {
            self.get_char()?;
            is_real = true;
            self.token_text.push('.');
            self.accumulate_value(&mut number_value, SyntaxCode::InvalidFraction)?;
            decimal_offset = whole_count - self.digit_count;
        }

        if self.cur_char == b'E' || self.cur_char == b'e' {
            // The C++ never advances past the 'E' before looking for a sign, so the exponent digits are never
            // found and every literal with an exponent is a syntax error. Reproduced as-is.
            is_real = true;
            self.token_text.push(self.cur_char as char);
            self.accumulate_value(&mut exponent_value, SyntaxCode::InvalidExponent)?;
        }

        if self.count_error {
            return Err(self.syntax_error(SyntaxCode::TooManyDigits));
        }

        let exponent = (exponent_value + decimal_offset as f32) as i32;
        if !(-MAX_EXPONENT..=MAX_EXPONENT).contains(&(exponent + whole_count)) {
            return Err(self.syntax_error(SyntaxCode::RealOutOfRange));
        }
        if exponent != 0 {
            number_value *= mc2_libm::pow(10.0, exponent as f64) as f32;
        }

        if is_real {
            self.literal = Literal::Real(number_value);
        } else {
            self.literal = Literal::Int(f32_to_i32_trunc(number_value));
        }
        self.tok = Tok::Number;
        Ok(())
    }

    fn get_string(&mut self) -> Res<()> {
        let mut bytes = Vec::new();
        self.get_char()?;
        while self.cur_char != CHAR_EOF {
            if self.cur_char == b'"' {
                break;
            }
            bytes.push(self.cur_char);
            self.get_char()?;
        }
        self.cur_char = b' ';
        self.token_text = String::from_utf8_lossy(&bytes).into_owned();
        self.literal = Literal::Str(bytes);
        self.tok = Tok::String;
        Ok(())
    }

    fn get_special(&mut self) -> Res<()> {
        let c = self.cur_char;
        match c {
            b'*' => self.tok = Tok::Star,
            b'(' => self.tok = Tok::LParen,
            b')' => self.tok = Tok::RParen,
            b'-' => self.tok = Tok::Minus,
            b'+' => self.tok = Tok::Plus,
            b'[' => self.tok = Tok::LBracket,
            b']' => self.tok = Tok::RBracket,
            b';' => self.tok = Tok::Semicolon,
            b'@' => self.tok = Tok::Ref,
            b'#' => self.tok = Tok::Pound,
            b',' => self.tok = Tok::Comma,
            b'/' => self.tok = Tok::FSlash,
            b'.' => self.tok = Tok::Period,
            b':' => self.tok = Tok::Colon,
            b'=' => {
                self.get_char()?;
                if self.cur_char == b'=' {
                    self.tok = Tok::EqualEqual;
                    self.get_char()?;
                } else {
                    self.tok = Tok::Equal;
                }
                return Ok(());
            }
            b'<' => {
                self.get_char()?;
                if self.cur_char == b'=' {
                    self.tok = Tok::Le;
                    self.get_char()?;
                } else if self.cur_char == b'>' {
                    self.tok = Tok::Ne;
                    self.get_char()?;
                } else {
                    self.tok = Tok::Lt;
                }
                return Ok(());
            }
            b'>' => {
                self.get_char()?;
                if self.cur_char == b'=' {
                    self.tok = Tok::Ge;
                    self.get_char()?;
                } else {
                    self.tok = Tok::Gt;
                }
                return Ok(());
            }
            _ => self.tok = Tok::Error,
        }
        self.get_char()
    }
}

/// `(int)floatValue` with the x86 `cvttss2si` behaviour for out-of-range values (the original Windows build):
/// NaN and out-of-range values become `i32::MIN`.
pub fn f32_to_i32_trunc(v: f32) -> i32 {
    if v.is_nan() || v >= 2147483648.0 || v < -2147483648.0 {
        i32::MIN
    } else {
        v as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MemorySource;

    fn scanner_for(text: &str) -> Scanner {
        let mut m = MemorySource::new();
        m.insert("a.abl", text);
        let mut sc = Scanner::new(Rc::new(m));
        sc.reset_for_module(false);
        sc.open_source_file("a.abl").unwrap();
        sc
    }

    fn all_tokens(text: &str) -> Vec<(Tok, String)> {
        let mut sc = scanner_for(text);
        let mut out = Vec::new();
        loop {
            sc.get_token().unwrap();
            if sc.tok == Tok::Eof {
                break;
            }
            out.push((sc.tok, sc.word.clone()));
        }
        out
    }

    fn kinds(text: &str) -> Vec<Tok> {
        all_tokens(text).into_iter().map(|t| t.0).collect()
    }

    #[test]
    fn tokens_and_reserved_words() {
        assert_eq!(
            kinds("if X <= 3 then y = 4.5; ENDIF;"),
            vec![
                Tok::If,
                Tok::Identifier,
                Tok::Le,
                Tok::Number,
                Tok::Then,
                Tok::Identifier,
                Tok::Equal,
                Tok::Number,
                Tok::Semicolon,
                Tok::EndIf,
                Tok::Semicolon
            ]
        );
        assert_eq!(
            kinds("a == b <> c >= d < e > f"),
            vec![
                Tok::Identifier,
                Tok::EqualEqual,
                Tok::Identifier,
                Tok::Ne,
                Tok::Identifier,
                Tok::Ge,
                Tok::Identifier,
                Tok::Lt,
                Tok::Identifier,
                Tok::Gt,
                Tok::Identifier
            ]
        );
        // `div` and `elsif` have token codes but no reserved-word table entry in the C++.
        assert_eq!(kinds("div elsif return"), vec![Tok::Identifier; 3]);
        // identifiers are case-folded in `word`
        assert_eq!(
            all_tokens("FooBar_2")[0],
            (Tok::Identifier, "foobar_2".to_string())
        );
        // an unknown character is an error token, not a syntax error by itself
        assert_eq!(
            kinds("a ? b"),
            vec![Tok::Identifier, Tok::Error, Tok::Identifier]
        );
    }

    #[test]
    fn dotted_identifiers_and_the_module_terminator() {
        let t = all_tokens("lib.member endmodule.");
        assert_eq!(t[0], (Tok::Identifier, "lib.member".to_string()));
        assert_eq!(t[1].0, Tok::EndModule);
        assert_eq!(t[2].0, Tok::Period);
    }

    #[test]
    fn numbers() {
        let mut sc = scanner_for("42 0.05 3.25 16777217 007");
        let mut vals = Vec::new();
        for _ in 0..5 {
            sc.get_token().unwrap();
            assert_eq!(sc.tok, Tok::Number);
            vals.push(sc.literal.clone());
        }
        assert_eq!(vals[0], Literal::Int(42));
        match vals[1] {
            Literal::Real(v) => assert!((v - 0.05).abs() < 1e-7, "{v}"),
            _ => panic!(),
        }
        assert_eq!(vals[2], Literal::Real(3.25));
        // integer literals are accumulated in single precision: 2^24 + 1 is not representable
        assert_eq!(vals[3], Literal::Int(16_777_216));
        assert_eq!(vals[4], Literal::Int(7));
    }

    #[test]
    fn number_errors() {
        for (src, code) in [
            ("5.", SyntaxCode::InvalidFraction),
            ("5.x", SyntaxCode::InvalidFraction),
            ("5e3", SyntaxCode::InvalidExponent),
            ("123456789012345678901", SyntaxCode::TooManyDigits),
        ] {
            let mut sc = scanner_for(src);
            let e = sc.get_token().unwrap_err();
            assert_eq!(e.syntax_code(), Some(code), "{src}");
        }
    }

    #[test]
    fn x86_float_to_int_conversion() {
        assert_eq!(f32_to_i32_trunc(3.9), 3);
        assert_eq!(f32_to_i32_trunc(-3.9), -3);
        assert_eq!(f32_to_i32_trunc(3.0e10), i32::MIN);
        assert_eq!(f32_to_i32_trunc(f32::NAN), i32::MIN);
        assert_eq!(f32_to_i32_trunc(-2147483648.0), i32::MIN);
    }

    #[test]
    fn strings_keep_everything_up_to_the_closing_quote() {
        let mut sc = scanner_for("\"a b\\c\" x");
        sc.get_token().unwrap();
        assert_eq!(sc.tok, Tok::String);
        assert_eq!(sc.literal, Literal::Str(b"a b\\c".to_vec()));
        sc.get_token().unwrap();
        assert_eq!((sc.tok, sc.word.as_str()), (Tok::Identifier, "x"));
    }

    #[test]
    fn comments_directives_and_line_numbers() {
        let mut sc = scanner_for("a // gone\nb /* multi\nline */ c\n#print_on\nd");
        let mut seen = Vec::new();
        loop {
            sc.get_token().unwrap();
            if sc.tok == Tok::Eof {
                break;
            }
            seen.push((sc.word.clone(), sc.line_number));
        }
        assert_eq!(seen[0], ("a".into(), 1));
        assert_eq!(seen[1], ("b".into(), 2));
        assert_eq!(seen[2], ("c".into(), 3));
        assert_eq!(seen[3].0, "d");
        assert!(sc.print_enabled);
        assert!(!sc.assert_enabled);
    }
}
