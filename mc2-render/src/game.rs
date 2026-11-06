//! Access to the shipped game data through the VFS (`mc2-formats`).

use crate::error::{RenderError, Result};
use mc2_assets::tga::{self, RgbaImage};
use mc2_formats::ini::FitIni;
use mc2_formats::pak::PakFile;
use mc2_formats::vfs::Vfs;
use std::path::Path;

/// The data root plus the archives from `system.cfg`.
pub struct GameData {
    pub vfs: Vfs,
}

impl GameData {
    /// Opens `<root>/system.cfg` and mounts its fast files.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            vfs: Vfs::from_system_cfg(root.as_ref())?,
        })
    }

    /// Reads `MC2_DATA`.
    pub fn from_env() -> Result<Self> {
        let root = std::env::var_os("MC2_DATA")
            .ok_or_else(|| RenderError::Other("MC2_DATA is not set".into()))?;
        Self::open(root)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.vfs.locate(name).is_some()
    }

    pub fn read(&mut self, name: &str) -> Result<Vec<u8>> {
        self.vfs
            .read(name)?
            .ok_or_else(|| RenderError::Missing(name.to_string()))
    }

    pub fn read_opt(&mut self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.vfs.read(name)?)
    }

    pub fn fit(&mut self, name: &str) -> Result<FitIni> {
        Ok(FitIni::from_bytes(self.read(name)?)?)
    }

    pub fn pak(&mut self, name: &str) -> Result<PakFile> {
        Ok(PakFile::from_bytes(self.read(name)?)?)
    }

    /// Decodes a TGA to top-left RGBA8.
    pub fn tga(&mut self, name: &str) -> Result<RgbaImage> {
        let bytes = self.read(name)?;
        tga::decode(&bytes).map_err(|e| RenderError::Other(format!("{name}: {e}")))
    }

    /// First existing name of `candidates`, decoded.
    pub fn tga_first(&mut self, candidates: &[&str]) -> Result<Option<(String, RgbaImage)>> {
        for c in candidates {
            if self.exists(c) {
                return Ok(Some((c.to_string(), self.tga(c)?)));
            }
        }
        Ok(None)
    }
}
