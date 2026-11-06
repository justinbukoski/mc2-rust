//! MechCommander 2 GUI framework (lane U1 of the Rust port).
//!
//! * widgets ported from `gui/*.cpp`: [`widgets::object::Object`] (`aObject`), rects, texts,
//!   animated objects, buttons, edits, scroll bars, list boxes, drop lists and combo boxes;
//! * [`screen::LogisticsScreen`]: a screen built from its `.fit` layout;
//! * [`font`]: the game's bitmap fonts and the GameOS text layout rules;
//! * [`strings`]: the `mc2res` string table, read from the player's resource library;
//! * [`render`]: a 2D wgpu pipeline drawing a [`draw::DrawList`] into a caller's render target;
//! * [`screens`]: main menu, options and loading screens.
//!
//! Everything a widget does is driven by [`input::UiInput`] (events in UI coordinates) and
//! reported back as values: messages, sound ids, help text ids and screen results.

#![allow(clippy::chunks_exact_to_as_chunks)]

pub mod anim;
pub mod color;
pub mod component_row;
pub mod control;
pub mod cp1252;
pub mod ctx;
pub mod cursor;
pub mod draw;
pub(crate) mod fit;
pub mod font;
pub mod harness;
pub mod hotkeys;
pub mod hud_info;
pub mod input;
pub mod logistics;
pub mod logistics_dialog;
pub mod logistics_ids;
pub mod mech_lab;
pub mod mech_purchase;
pub mod mechlopedia;
pub mod mission_hud;
pub mod mission_keys;
pub mod mission_results;
pub mod mission_select;
pub mod objectives;
pub mod pause_window;
pub mod preview;
pub mod render;
pub mod res_lib;
pub mod results_icons;
pub mod results_pilots;
pub mod results_promotion;
pub mod results_salvage;
pub mod screen;
pub mod screens;
pub mod solo_missions;
pub mod strings;
pub mod tacmap;
pub mod textures;
pub mod unitbar;
pub mod widgets;

pub use ctx::{Loader, Msg, RenderCtx, UiAssets, UiOutput, UpdateCtx};
