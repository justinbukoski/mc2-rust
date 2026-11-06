//! Small helpers over [`mc2_formats::ini::FitIni`] for the reads the GUI code makes.
//!
//! The C++ reads into uninitialised locals and ignores the error code (`file->readIdLong("X", x)`),
//! so a missing key leaves stack garbage. Here a missing key reads as `None`, and callers pick the
//! value the layouts rely on (normally 0); each such choice is listed in the porting log.

use mc2_formats::ini::FitIni;

pub(crate) trait FitExt {
    fn long(&self, key: &str) -> Option<i64>;
    fn float(&self, key: &str) -> Option<f32>;
    fn boolean(&self, key: &str) -> Option<bool>;
    fn string(&self, key: &str) -> Option<String>;

    fn long_or(&self, key: &str, default: i64) -> i64 {
        self.long(key).unwrap_or(default)
    }
    /// A `long` read as the 32-bit `DWORD` colour it holds.
    fn color(&self, key: &str) -> Option<u32> {
        self.long(key).map(|v| v as u32)
    }
}

impl FitExt for FitIni {
    fn long(&self, key: &str) -> Option<i64> {
        // `readIdLong` is 32 bit: hex colours above 0x7fffffff wrap to negative values there.
        self.read_i64(key).ok().map(|v| v as i32 as i64)
    }
    fn float(&self, key: &str) -> Option<f32> {
        self.read_f32(key).ok()
    }
    fn boolean(&self, key: &str) -> Option<bool> {
        self.read_bool(key).ok()
    }
    fn string(&self, key: &str) -> Option<String> {
        self.read_string(key, 2048).ok()
    }
}
