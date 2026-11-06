use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub struct FxError(pub String);

impl FxError {
    pub fn new(s: impl Into<String>) -> Self {
        FxError(s.into())
    }
}

impl fmt::Display for FxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for FxError {}

pub type Result<T> = std::result::Result<T, FxError>;
