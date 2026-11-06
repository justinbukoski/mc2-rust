// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Error types and the message tables from `mclib/ablerr.cpp`.
//!
//! The C++ implementation treats the first syntax error as fatal (`MAX_SYNTAX_ERRORS` is 1 and the game's fatal
//! callback is `STOP`), so every fallible operation here returns the first error as an [`AblError`].

use std::fmt;

/// Syntax error codes (`SyntaxErrorType` in `ablerr.h`). The numeric value is the C++ enum value and is part of the
/// formatted message, so it must not change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
#[allow(missing_docs)]
pub enum SyntaxCode {
    None = 0,
    Error,
    TooManyErrors,
    SourceFileOpen,
    UnexpectedEof,
    InvalidNumber,
    InvalidFraction,
    InvalidExponent,
    TooManyDigits,
    RealOutOfRange,
    IntegerOutOfRange,
    MissingRParen,
    InvalidExpression,
    UndefinedIdentifier,
    RedefinedIdentifier,
    UnexpectedToken,
    IncompatibleTypes,
    NestingTooDeep,
    CodeSegmentOverflow,
    MissingEqual,
    MissingSemicolon,
    InvalidConstant,
    NotAConstantIdentifier,
    NoRecordTypes,
    MissingColon,
    NotATypeIdentifier,
    InvalidType,
    MissingEnd,
    InvalidIdentifierUsage,
    TooManySubscripts,
    MissingRBracket,
    IncompatibleAssignment,
    MissingUntil,
    MissingThen,
    InvalidForControl,
    MissingIdentifier,
    MissingTo,
    MissingPeriod,
    MissingModule,
    MissingLibrary,
    AlreadyForwarded,
    InvalidRefParam,
    WrongNumberOfParams,
    MissingBegin,
    MissingEndVar,
    NoFunctionNesting,
    MissingCode,
    MissingEndIf,
    MissingEndWhile,
    MissingEndFor,
    MissingEndFunction,
    MissingEndModule,
    MissingEndLibrary,
    MissingDo,
    InvalidIndexType,
    MissingComma,
    TooManyStaticVars,
    MissingEndCase,
    MissingEndSwitch,
    MissingConstant,
    BadLanguageDirectiveUsage,
    UnknownLanguageDirective,
    TooManyFormalParameters,
    TooManyLocalVariables,
    OrderReturnType,
    TooManyOrders,
    MissingStateIdentifier,
    MissingEndFsm,
}

impl SyntaxCode {
    /// The message text from `syntaxErrorMessages[]`.
    pub fn message(self) -> &'static str {
        use SyntaxCode::*;
        match self {
            None => "No syntax error",
            Error => "Syntax error",
            TooManyErrors => "Too many errors",
            SourceFileOpen => "Cannot open source file",
            UnexpectedEof => "Unexpected end-of-file",
            InvalidNumber => "Invalid number",
            InvalidFraction => "Invalid fraction",
            InvalidExponent => "Invalid exponent",
            TooManyDigits => "Too many digits",
            RealOutOfRange => "Real out of range",
            IntegerOutOfRange => "Integer out of range",
            MissingRParen => "Missing right parenthesis",
            InvalidExpression => "Invalid expression",
            UndefinedIdentifier => "Undefined identifier",
            RedefinedIdentifier => "Redefined identifier",
            UnexpectedToken => "Unexpected token",
            IncompatibleTypes => "Incompatible types",
            NestingTooDeep => "Nesting too deep",
            CodeSegmentOverflow => "Code segment overflow",
            MissingEqual => "Missing equal",
            MissingSemicolon => "Missing semi-colon",
            InvalidConstant => "Invalid constant",
            NotAConstantIdentifier => "Not a constant identifier",
            NoRecordTypes => "No record types",
            MissingColon => "Missing colon (ouch)",
            NotATypeIdentifier => "Not a type identifier",
            InvalidType => "Invalid type",
            MissingEnd => "Missing end",
            InvalidIdentifierUsage => "Invalid identifier usage",
            TooManySubscripts => "Too many subscripts",
            MissingRBracket => "Missing right bracket",
            IncompatibleAssignment => "Incompatible assignment",
            MissingUntil => "Missing until",
            MissingThen => "Missing then",
            InvalidForControl => "Invalid for control",
            MissingIdentifier => "Missing identifier",
            MissingTo => "Missing to",
            MissingPeriod => "Missing period",
            MissingModule => "Missing module",
            MissingLibrary => "Missing library",
            AlreadyForwarded => "Already forwarded",
            InvalidRefParam => "Invalid reference parameter",
            WrongNumberOfParams => "Wrong number of parameters",
            MissingBegin => "Missing begin",
            MissingEndVar => "Missing endvar",
            NoFunctionNesting => "No function nesting",
            MissingCode => "Missing code",
            MissingEndIf => "Missing endif",
            MissingEndWhile => "Missing endwhile",
            MissingEndFor => "Missing endfor",
            MissingEndFunction => "Missing endfunction",
            MissingEndModule => "Missing endmodule",
            MissingEndLibrary => "Missing endlibrary",
            MissingDo => "Missing do",
            InvalidIndexType => "Invalid index type",
            MissingComma => "Missing comma",
            TooManyStaticVars => "Too many static variables",
            MissingEndCase => "Missing endcase",
            MissingEndSwitch => "Missing endswitch",
            MissingConstant => "Missing constant",
            BadLanguageDirectiveUsage => "Bad language directive parameter",
            UnknownLanguageDirective => "Unknown language directive",
            TooManyFormalParameters => "Too Many Formal Parameters",
            TooManyLocalVariables => "Too Many Local Variables",
            OrderReturnType => "Order must have Integer Return Type",
            TooManyOrders => "Too many orders",
            MissingStateIdentifier => "Missing state for transition",
            MissingEndFsm => "Missing endfsm",
        }
    }
}

/// Runtime error codes (`RuntimeErrorType` in `ablerr.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
#[allow(missing_docs)]
pub enum RuntimeCode {
    StackOverflow = 0,
    InfiniteLoop,
    NestedFunctionCall,
    UnimplementedFeature,
    ValueOutOfRange,
    DivisionByZero,
    InvalidFunctionArgument,
    InvalidCaseValue,
    Abort,
    NullPrevState,
}

impl RuntimeCode {
    /// The message text from `runtimeErrorMessages[]`.
    pub fn message(self) -> &'static str {
        match self {
            RuntimeCode::StackOverflow => "Runtime stack overflow",
            RuntimeCode::InfiniteLoop => "Infinite Loop",
            RuntimeCode::NestedFunctionCall => "Nested function call",
            RuntimeCode::UnimplementedFeature => "Unimplemented feature",
            RuntimeCode::ValueOutOfRange => "Value out of range",
            RuntimeCode::DivisionByZero => "Division by zero",
            RuntimeCode::InvalidFunctionArgument => "Invalid function argument",
            RuntimeCode::InvalidCaseValue => "Invalid case value",
            RuntimeCode::Abort => "Abort",
            RuntimeCode::NullPrevState => "No Previous State",
        }
    }
}

/// Which stage raised the error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// `syntaxError()`; carries a [`SyntaxCode`].
    Syntax(SyntaxCode),
    /// `runtimeError()`; carries a [`RuntimeCode`].
    Runtime(RuntimeCode),
    /// `ABL_Fatal()` with a free-form message (limits exceeded, script `fatal(...)`, `assert(...)`, ...).
    Fatal,
}

/// An ABL error with the file/line context the C++ formats into its message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AblError {
    /// Error category and code.
    pub kind: ErrorKind,
    /// Source file the error refers to (empty when unavailable).
    pub file: String,
    /// 1-based source line (0 when unavailable).
    pub line: u32,
    /// Fully formatted message in the C++ format.
    pub message: String,
}

impl AblError {
    /// A free-form fatal (`ABL_Fatal(0, s)`).
    pub fn fatal(message: impl Into<String>) -> AblError {
        AblError {
            kind: ErrorKind::Fatal,
            file: String::new(),
            line: 0,
            message: message.into(),
        }
    }

    /// Format a syntax error exactly like `syntaxError()` in `ablerr.cpp`.
    pub fn syntax(code: SyntaxCode, file: &str, line: u32, word: &str) -> AblError {
        AblError {
            kind: ErrorKind::Syntax(code),
            file: file.to_string(),
            line,
            message: format!(
                "SYNTAX ERROR {} [line {}] - (type {}) {} \"{}\"\n",
                file,
                line,
                code as i32,
                code.message(),
                word
            ),
        }
    }

    /// Format a runtime error exactly like `runtimeError()` in `ablerr.cpp`.
    pub fn runtime(code: RuntimeCode, file: &str, line: u32) -> AblError {
        AblError {
            kind: ErrorKind::Runtime(code),
            file: if file.is_empty() {
                "unavailable".to_string()
            } else {
                file.to_string()
            },
            line,
            message: format!(
                "ABL RUNTIME ERROR {} [line {}] - (type {}) {}\n",
                if file.is_empty() { "unavailable" } else { file },
                line,
                code as i32,
                code.message()
            ),
        }
    }

    /// The syntax code if this is a syntax error.
    pub fn syntax_code(&self) -> Option<SyntaxCode> {
        match self.kind {
            ErrorKind::Syntax(c) => Some(c),
            _ => None,
        }
    }

    /// The runtime code if this is a runtime error.
    pub fn runtime_code(&self) -> Option<RuntimeCode> {
        match self.kind {
            ErrorKind::Runtime(c) => Some(c),
            _ => None,
        }
    }
}

impl fmt::Display for AblError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message.trim_end())
    }
}

impl std::error::Error for AblError {}

/// Result alias used throughout the crate.
pub type AblResult<T> = Result<T, AblError>;
