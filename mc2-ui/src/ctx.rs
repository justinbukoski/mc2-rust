// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Shared state the widgets read and write instead of the original's globals.
//!
//! | original                                  | here                                  |
//! |-------------------------------------------|---------------------------------------|
//! | `userInput`, `gos_GetKey`                 | [`UpdateCtx::input`]                  |
//! | `frameLength`                             | [`UpdateCtx::dt`], [`RenderCtx::dt`]  |
//! | `helpTextID` / `helpTextHeaderID`         | [`UiOutput::help_text_id`] / `help_header_id` |
//! | `sndSystem->playDigitalSample(id)`        | [`UiOutput::sounds`] (ids from `mclib/sounds.h`) |
//! | `cLoadString`, `aFont`, `mcTextureManager`| [`UiAssets`]                          |
//! | `parent->handleMessage(msg, id)`          | return values ([`Msg`])               |

use crate::draw::DrawList;
use crate::font::{Font, FontCache, FontRef};
use crate::input::UiInput;
use crate::strings::StringTable;
use crate::textures::{TexId, TextureStore};
use mc2_formats::ini::FitIni;
use mc2_formats::vfs::Vfs;
use std::sync::Arc;

/// Sound effect ids (`mclib/sounds.h`).
pub mod sfx {
    pub const LOG_WRONGBUTTON: i32 = 89;
    /// `LOG_PILOTMEDAL`, `LOG_KILLMARKER`, `LOG_PROMOTED` (mclib/sounds.h:114-116).
    pub const LOG_PILOTMEDAL: i32 = 102;
    pub const LOG_KILLMARKER: i32 = 103;
    pub const LOG_PROMOTED: i32 = 104;
    pub const LOG_VIDEOBUTTONS: i32 = 90;
    pub const LOG_SELECT: i32 = 90;
    pub const LOG_MAINMENUBUTTON: i32 = 95;
    pub const LOG_HIGHLIGHTBUTTONS: i32 = 96;
    pub const LOG_DIGITALHIGHLIGHT: i32 = 96;
    pub const LOG_EXITGAME: i32 = 97;
    pub const LOG_CLICKONBUTTON: i32 = 98;
    pub const LOAD_DOORS_CLOSING: i32 = 110;
    pub const LOAD_DOORS_OPENING: i32 = 111;
}

/// Betty voice samples (`mclib/sounds.h`, `playBettySample`).
pub mod betty {
    /// `BETTY_PROMOREG` .. `BETTY_PROMOACE` (`mclib/sounds.h:212-215`): the promotion pop-up.
    pub const PROMO_REGULAR: i32 = 42;
    pub const PROMO_VETERAN: i32 = 43;
    pub const PROMO_ELITE: i32 = 44;
    pub const PROMO_ACE: i32 = 45;
    pub const NEW_PILOTS: i32 = 47;
}

/// GUI message ids (`aMSG_*`, `gui/asystem.h`).
pub mod msg {
    pub const LEFTMOUSEDOWN: u32 = 1;
    pub const MIDDLEMOUSEDOWN: u32 = 2;
    pub const RIGHTMOUSEDOWN: u32 = 3;
    pub const LEFTMOUSEUP: u32 = 4;
    pub const MOUSEMOVE: u32 = 7;
    pub const LEFTMOUSEDBLCLICK: u32 = 16;
    pub const LEFTMOUSEHELD: u32 = 18;
    pub const RIGHTMOUSEHELD: u32 = 19;
    pub const SCROLLUP: u32 = 101;
    pub const SCROLLDOWN: u32 = 102;
    pub const PAGEUP: u32 = 103;
    pub const PAGEDOWN: u32 = 104;
    pub const TRACKTAB: u32 = 106;
    pub const SCROLLTO: u32 = 107;
    pub const SCROLLPARENT: u32 = 108;
    pub const SELECTME: u32 = 109;
    pub const DONE: u32 = 110;
    pub const BUTTONCLICKED: u32 = 111;
    pub const SELCHANGED: u32 = 112;
}

/// A message a widget sends to its parent (`handleMessage(msg, who)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Msg {
    pub msg: u32,
    pub who: i32,
}

impl Msg {
    pub fn new(msg: u32, who: i32) -> Msg {
        Msg { msg, who }
    }
}

/// Strings, fonts and textures shared by every widget and screen.
pub struct UiAssets {
    pub strings: Arc<StringTable>,
    pub fonts: FontCache,
    pub textures: TextureStore,
    /// Model pictures the screens asked for and that are not rendered yet (see [`crate::preview`]).
    pub previews: crate::preview::PreviewQueue,
    /// The logical screen size (`Environment.screenWidth/Height`): 800x600 for the menus.
    pub screen: (i32, i32),
    /// The player's paint `(highlight, highlight, base)` from `prefs` as `options.cfg` stores them
    /// (`setMech(.., prefs.baseColor, prefs.highlightColor, prefs.highlightColor)` -> `resetPaintScheme(h1, h2,
    /// base)`, `mechbayscreen.cpp:776`, `logisticsmechdisplay.cpp:148`, `simplecamera.cpp:355`).
    pub player_paint: [u32; 3],
}

impl Default for UiAssets {
    fn default() -> Self {
        UiAssets::new(Arc::new(StringTable::for_product()))
    }
}

impl UiAssets {
    pub fn new(strings: Arc<StringTable>) -> UiAssets {
        UiAssets {
            strings,
            fonts: FontCache::new(),
            textures: TextureStore::new(),
            previews: Default::default(),
            screen: (800, 600),
            player_paint: [crate::preview::DEFAULT_PAINT; 3],
        }
    }

    /// Sets [`UiAssets::player_paint`] from the prefs' base and highlight colours.
    pub fn set_player_paint(&mut self, base: u32, highlight: u32) {
        self.player_paint = [highlight, highlight, base];
    }

    pub fn font(&self, f: FontRef) -> Option<&Font> {
        f.id.map(|id| self.fonts.get(id))
    }

    /// `cLoadString`.
    pub fn string(&self, id: u32) -> &str {
        self.strings.load(id)
    }
}

/// Access to the data while a screen is built from its `.fit` files.
pub struct Loader<'a> {
    pub vfs: &'a mut Vfs,
    pub assets: &'a mut UiAssets,
}

impl<'a> Loader<'a> {
    pub fn new(vfs: &'a mut Vfs, assets: &'a mut UiAssets) -> Loader<'a> {
        Loader { vfs, assets }
    }

    /// An art texture by `.fit` file name.
    pub fn art(&mut self, file_name: &str) -> Option<TexId> {
        self.assets.textures.load_art(self.vfs, file_name)
    }

    /// An art texture loaded keyed (`gos_Texture_Keyed`).
    pub fn art_keyed(&mut self, file_name: &str) -> Option<TexId> {
        self.assets.textures.load_art_keyed(self.vfs, file_name)
    }

    /// A texture by data path.
    pub fn texture(&mut self, path: &str) -> Option<TexId> {
        self.assets.textures.load_tga(self.vfs, path)
    }

    /// `aFont::init(resID)`.
    pub fn font(&mut self, res_id: u32) -> FontRef {
        let UiAssets {
            strings,
            fonts,
            textures,
            ..
        } = &mut *self.assets;
        fonts.load_resource(self.vfs, textures, strings, res_id)
    }

    /// Opens `data/art/<name>.fit` (the `FullPathFileName(artPath, name, ".fit")` pattern).
    pub fn art_fit(&mut self, name: &str) -> Result<FitIni, String> {
        let path = if name.ends_with(".fit") {
            format!("{}{name}", crate::textures::ART_PATH)
        } else {
            format!("{}{name}.fit", crate::textures::ART_PATH)
        };
        self.fit(&path)
    }

    pub fn fit(&mut self, path: &str) -> Result<FitIni, String> {
        self.vfs
            .read_fit(path)
            .map_err(|e| format!("{path}: {e}"))?
            .ok_or_else(|| format!("couldn't open file {path}"))
    }
}

/// What a frame's update produced besides widget state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiOutput {
    /// Sound effects to play, in order.
    pub sounds: Vec<i32>,
    /// Betty voice samples to play, in order (`sndSystem->playBettySample`, see [`betty`]).
    pub betty: Vec<i32>,
    pub help_text_id: i32,
    pub help_header_id: i32,
}

/// Per-frame update context.
pub struct UpdateCtx<'a> {
    pub input: &'a mut UiInput,
    pub assets: &'a UiAssets,
    /// Seconds since the previous frame (`frameLength`).
    pub dt: f32,
    pub out: &'a mut UiOutput,
}

impl UpdateCtx<'_> {
    /// `sndSystem->playDigitalSample(id)`; negative ids (`setPressFX(-1)`) are silent.
    pub fn play(&mut self, id: i32) {
        if id >= 0 {
            self.out.sounds.push(id);
        }
    }

    /// `soundSystem->playBettySample(id)`.
    pub fn play_betty(&mut self, id: i32) {
        self.out.betty.push(id);
    }
}

/// Per-frame render context.
pub struct RenderCtx<'a> {
    pub dl: &'a mut DrawList,
    pub assets: &'a UiAssets,
    pub dt: f32,
}

impl RenderCtx<'_> {
    pub fn screen(&self) -> (i32, i32) {
        self.dl.screen
    }
}
