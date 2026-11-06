//! Packaging tools, one subcommand each (see `main.rs`): `render-product`, `gen-notices`, `mkarchive`. Outputs are pinned by the goldens
//! in `tests/golden`.

pub mod archive;
pub mod deflate;
pub mod notices;
pub mod render;

/// Error type of every subcommand: a message for stderr.
pub type Result<T> = std::result::Result<T, String>;

/// Adds the path to an I/O error.
pub fn io<T>(r: std::io::Result<T>, what: impl AsRef<std::path::Path>) -> Result<T> {
    r.map_err(|e| format!("{}: {e}", what.as_ref().display()))
}

/// Python's text-mode read: `\r\n` and lone `\r` become `\n`.
pub fn universal_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
