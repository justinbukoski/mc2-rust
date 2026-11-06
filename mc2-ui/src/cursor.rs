// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mouse cursors (`MouseCursorData::initCursors`, `UserInput::render` in `mclib/userinput.cpp`):
//! `cursors.fit` (or `cursorsa.fit`, the smaller set, at 800 pixels wide or less) lists one
//! `StaticInfo` per `mState_*` with its hot spot, frame count and frame length. Multi-frame cursors
//! step through cells of the cursor's size laid out across the texture.

use crate::draw::{Blend, DrawList};
use crate::fit::FitExt;
use crate::textures::{art_file_path, TexId, TextureStore};
use mc2_formats::vfs::Vfs;

pub const M_STATE_NORMAL: u32 = 21;
pub const M_STATE_LOGISTICS: u32 = 43;

#[derive(Clone, Debug, Default)]
pub struct CursorInfo {
    pub texture: Option<TexId>,
    pub texture_width: f32,
    pub width: f32,
    pub height: f32,
    /// `UNormal`/`VNormal` in pixels.
    pub u: f32,
    pub v: f32,
    /// `UWidth`/`VHeight`: texture extent of a single-frame cursor.
    pub uw: f32,
    pub vh: f32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub frames: u32,
    pub frame_length: f32,
}

#[derive(Default)]
pub struct Cursors {
    pub infos: Vec<CursorInfo>,
    frame: u32,
    frame_time: f32,
}

impl Cursors {
    /// Loads `<name>.fit` (`<name>a.fit` when `screen_width <= 800`).
    pub fn load(
        vfs: &mut Vfs,
        textures: &mut TextureStore,
        name: &str,
        screen_width: u32,
    ) -> Result<Cursors, String> {
        let file = if screen_width <= 800 {
            format!("data/art/{name}a.fit")
        } else {
            format!("data/art/{name}.fit")
        };
        let mut fit = vfs
            .read_fit(&file)
            .map_err(|e| format!("{file}: {e}"))?
            .ok_or_else(|| format!("{file} not found"))?;
        fit.seek_block("Main").map_err(|e| e.to_string())?;
        let count = fit.long("NumCursors").unwrap_or(0).max(0) as usize;
        let mut infos = Vec::with_capacity(count);
        for i in 0..count {
            let mut c = CursorInfo::default();
            if fit.seek_block(&format!("Cursor{i}")).is_ok() {
                c.width = fit.long_or("Width", 0) as f32;
                c.height = fit.long_or("Height", 0) as f32;
                if let Some(f) = fit.string("FileName") {
                    c.texture = textures.load_tga(vfs, &art_file_path(&f));
                    c.texture_width = c.texture.map_or(0.0, |t| textures.width(t) as f32);
                }
                c.u = fit.long_or("UNormal", 0) as f32;
                c.v = fit.long_or("VNormal", 0) as f32;
                c.uw = fit.long_or("UWidth", 0) as f32;
                c.vh = fit.long_or("VHeight", 0) as f32;
                c.hot_x = fit.read_i8("HotSpotX").map_or(0, i32::from);
                c.hot_y = fit.read_i8("HotSpotY").map_or(0, i32::from);
                c.frames = fit.read_u32("NumFrames").unwrap_or(0);
                c.frame_length = fit.float("FrameLength").unwrap_or(0.0);
            }
            infos.push(c);
        }
        Ok(Cursors {
            infos,
            ..Cursors::default()
        })
    }

    /// Advances the animation frame of `state` (the cursor part of `UserInput::update`).
    pub fn update(&mut self, state: u32, dt: f32) {
        let Some(c) = self.infos.get(state as usize) else {
            return;
        };
        if c.frames > 1 {
            self.frame_time += dt;
            if self.frame_time > c.frame_length {
                self.frame += 1;
                if self.frame >= c.frames {
                    self.frame = 0;
                }
                self.frame_time = 0.0;
            }
        }
    }

    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// `UserInput::render`: the cursor quad for `state` at the pointer.
    pub fn draw(&self, dl: &mut DrawList, state: u32, mouse: (i32, i32)) {
        let Some(c) = self.infos.get(state as usize) else {
            return;
        };
        let Some(tex) = c.texture else { return };
        if c.texture_width <= 0.0 || c.width <= 0.0 {
            return;
        }
        let x = (mouse.0 - c.hot_x) as f32;
        let y = (mouse.1 - c.hot_y) as f32;
        let tw = c.texture_width;
        let uv = if c.frames > 1 {
            let per_row = ((tw / c.width) as u32).max(1);
            let (i, j) = ((self.frame % per_row) as f32, (self.frame / per_row) as f32);
            let u0 = (0.1 + c.u) / tw + i * c.width / tw;
            let v0 = (0.1 + c.v) / tw + j * c.height / tw;
            (
                u0,
                v0,
                u0 + (c.width + 0.1) / tw,
                v0 + (c.height + 0.1) / tw,
            )
        } else {
            // StaticInfo::init
            let u0 = c.u / tw + 0.1 / tw;
            let v0 = c.v / tw + 0.1 / tw;
            (
                u0,
                v0,
                (c.u + c.uw) / tw + 0.1 / tw,
                (c.v + c.vh) / tw + 0.1 / tw,
            )
        };
        dl.rect_quad(
            Some(tex),
            Blend::Alpha,
            (x, y, x + c.width, y + c.height),
            uv,
            0xffff_ffff,
        );
    }
}
