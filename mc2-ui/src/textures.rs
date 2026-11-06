// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! CPU-side texture store for GUI art (`mcTextureManager->loadTexture` for the interface).
//!
//! Every image is kept as RGBA8 with the name it was loaded under; that name is the key of the
//! hi-res replacement hook (`MC2_HIRES_DIR`, applied when [`crate::render::UiRenderer`] uploads it
//! through `mc2_render::gpu::Gpu::texture_2d`). Widgets only ever hold a [`TexId`] and the width the
//! original computed its UVs from, so a replacement of any size keeps the same normalised UVs.

use mc2_formats::vfs::Vfs;
use std::collections::HashMap;

/// Handle of an image in a [`TextureStore`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TexId(pub u32);

#[derive(Clone, Debug)]
pub struct Image {
    /// Lower-case data path, e.g. `data/art/mcl_mm_1.tga`.
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Bumped whenever the pixels change (the renderer re-uploads).
    pub version: u32,
    /// A per-frame stream (a movie frame): no hi-res pack entry can exist for it, so the upload
    /// skips the replacement lookup instead of probing the pack directory every frame.
    pub streamed: bool,
}

#[derive(Default)]
pub struct TextureStore {
    images: Vec<Image>,
    by_name: HashMap<String, TexId>,
}

const KEYED_SUFFIX: &str = "#keyed";

/// `artPath` from `system.cfg`.
pub const ART_PATH: &str = "data/art/";

/// The path the GUI loads an art file name from: `artPath + name`, lower-cased, `.tga` appended
/// when the name has none (`aObject::init`, `aButton::init`).
pub fn art_file_path(file_name: &str) -> String {
    let mut p = format!("{ART_PATH}{file_name}").to_ascii_lowercase();
    if !p.contains(".tga") {
        p.push_str(".tga");
    }
    p
}

impl TextureStore {
    pub fn new() -> TextureStore {
        TextureStore::default()
    }

    pub fn len(&self) -> usize {
        self.images.len()
    }

    pub fn is_empty(&self) -> bool {
        self.images.is_empty()
    }

    pub fn get(&self, id: TexId) -> Option<&Image> {
        self.images.get(id.0 as usize)
    }

    /// Width the GUI divides UVs by (`textureData.Width`).
    pub fn width(&self, id: TexId) -> u32 {
        self.get(id).map_or(0, |i| i.width)
    }

    pub fn find(&self, name: &str) -> Option<TexId> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    /// Adds (or replaces, when the name is already present) an RGBA8 image.
    pub fn insert(&mut self, name: &str, width: u32, height: u32, rgba: Vec<u8>) -> TexId {
        self.insert_image(name, width, height, rgba, false)
    }

    /// Adds (or replaces) an RGBA8 image that is re-inserted every frame (a movie frame). Its
    /// upload never consults the hi-res pack, so the pack directory is not probed per frame.
    pub fn insert_streamed(&mut self, name: &str, width: u32, height: u32, rgba: Vec<u8>) -> TexId {
        self.insert_image(name, width, height, rgba, true)
    }

    fn insert_image(
        &mut self,
        name: &str,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        streamed: bool,
    ) -> TexId {
        let key = name.to_ascii_lowercase();
        assert_eq!(rgba.len(), (width * height * 4) as usize, "image size");
        if let Some(&id) = self.by_name.get(&key) {
            let img = &mut self.images[id.0 as usize];
            img.width = width;
            img.height = height;
            img.rgba = rgba;
            img.version += 1;
            img.streamed = streamed;
            return id;
        }
        let id = TexId(self.images.len() as u32);
        self.images.push(Image {
            name: key.clone(),
            width,
            height,
            rgba,
            version: 0,
            streamed,
        });
        self.by_name.insert(key, id);
        id
    }

    /// Pixels of an image for in-place edits (bumps the version).
    pub fn pixels_mut(&mut self, id: TexId) -> Option<&mut Vec<u8>> {
        let img = self.images.get_mut(id.0 as usize)?;
        img.version += 1;
        Some(&mut img.rgba)
    }

    /// Loads a `.tga` from the VFS once (`gos_Texture_Alpha`); later calls return the same
    /// handle.
    pub fn load_tga(&mut self, vfs: &mut Vfs, path: &str) -> Option<TexId> {
        self.load_tga_mode(vfs, path, false)
    }

    /// Loads a `.tga` as a keyed texture (`gos_Texture_Keyed`: 1555, a single bit of alpha), its
    /// alpha cut at 128. Kept apart from the alpha version of the same file.
    pub fn load_tga_keyed(&mut self, vfs: &mut Vfs, path: &str) -> Option<TexId> {
        self.load_tga_mode(vfs, path, true)
    }

    fn load_tga_mode(&mut self, vfs: &mut Vfs, path: &str, keyed: bool) -> Option<TexId> {
        let key = if keyed {
            format!("{}{KEYED_SUFFIX}", path.to_ascii_lowercase())
        } else {
            path.to_ascii_lowercase()
        };
        if let Some(&id) = self.by_name.get(&key) {
            return Some(id);
        }
        let bytes = match vfs.read(path) {
            Ok(Some(b)) => b,
            Ok(None) => {
                log::warn!("texture {path} not found");
                return None;
            }
            Err(e) => {
                log::warn!("texture {path}: {e}");
                return None;
            }
        };
        match mc2_assets::tga::decode(&bytes) {
            Ok(mut img) => {
                if keyed {
                    for a in img.pixels.iter_mut().skip(3).step_by(4) {
                        *a = if *a >= 128 { 255 } else { 0 };
                    }
                }
                let id = self.insert(
                    &key,
                    u32::from(img.width),
                    u32::from(img.height),
                    img.pixels,
                );
                // the image keeps the file's path as its name (the hi-res replacement key)
                self.images[id.0 as usize].name = path.to_ascii_lowercase();
                Some(id)
            }
            Err(e) => {
                log::warn!("texture {path}: {e}");
                None
            }
        }
    }

    /// Loads a GUI art file by the name a `.fit` gives it.
    pub fn load_art(&mut self, vfs: &mut Vfs, file_name: &str) -> Option<TexId> {
        self.load_tga(vfs, &art_file_path(file_name))
    }

    /// Loads a GUI art file as a keyed texture.
    pub fn load_art_keyed(&mut self, vfs: &mut Vfs, file_name: &str) -> Option<TexId> {
        self.load_tga_keyed(vfs, &art_file_path(file_name))
    }

    pub fn iter(&self) -> impl Iterator<Item = (TexId, &Image)> + '_ {
        self.images
            .iter()
            .enumerate()
            .map(|(i, img)| (TexId(i as u32), img))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_paths_follow_the_original() {
        assert_eq!(art_file_path("mcl_mm_12.tga"), "data/art/mcl_mm_12.tga");
        assert_eq!(art_file_path("Cursors1"), "data/art/cursors1.tga");
    }

    #[test]
    fn insert_reuses_names_and_bumps_versions() {
        let mut s = TextureStore::new();
        let a = s.insert("X.tga", 1, 1, vec![0; 4]);
        let b = s.insert("x.tga", 2, 1, vec![1; 8]);
        assert_eq!(a, b);
        assert_eq!(s.get(a).unwrap().version, 1);
        assert_eq!(s.width(a), 2);
        assert_eq!(s.find("X.TGA"), Some(a));
    }

    #[test]
    fn streamed_images_are_flagged_and_keep_the_flag_on_reinsert() {
        let mut s = TextureStore::new();
        let frame = s.insert_streamed("mission_video", 2, 2, vec![0; 16]);
        assert!(s.get(frame).unwrap().streamed);
        let again = s.insert_streamed("mission_video", 2, 2, vec![1; 16]);
        assert_eq!(frame, again);
        assert!(s.get(frame).unwrap().streamed);
        assert_eq!(s.get(frame).unwrap().version, 1);
        let still = s.insert("data/art/mcl_mm_1.tga", 1, 1, vec![0; 4]);
        assert!(!s.get(still).unwrap().streamed);
    }
}
