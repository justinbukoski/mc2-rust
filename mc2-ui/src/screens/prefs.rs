// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! `CPrefs` (`code/prefs.cpp`): the player's options as the options screen edits them, read from
//! and written to `options.cfg` (`[MechCommander2]` and `[Keyboard]` blocks).
//!
//! The Microsoft code stored the resolution as an index (`Resolution`, 0..4 = 640, 800, 1024, 1280,
//! 1600 wide) and the bit depth as 0/1; alariq's build stores `ResolutionX`/`ResolutionY` and a bit
//! count. Both are read; the explicit size wins. Device checks of `load` (video memory per
//! renderer) have no counterpart.

use crate::fit::FitExt;
use crate::hotkeys::{COMMANDS, MAX_COMMAND};
use mc2_formats::ini::{FitIni, FitWriter};

#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub digital_master_volume: i32,
    pub music_volume: i32,
    pub sfx_volume: i32,
    pub radio_volume: i32,
    pub betty_volume: i32,
    pub use_shadows: bool,
    /// "Terrain detail" (`useWaterInterestTexture`, key `DetailTexture`).
    pub use_water_interest_texture: bool,
    pub use_high_object_detail: bool,
    /// 0 green .. 3 elite.
    pub game_difficulty: i32,
    pub use_unlimited_ammo: bool,
    /// 0..2 a video device, 3 the software rasterizer.
    pub renderer: i32,
    pub resolution_x: i32,
    pub resolution_y: i32,
    pub bit_depth: i32,
    pub display_number: i32,
    pub full_screen: bool,
    pub gamma_level: i32,
    pub use_left_right_mouse_profile: bool,
    pub base_color: u32,
    pub highlight_color: u32,
    pub faction: i32,
    pub insignia_file: String,
    pub unit_name: [String; 10],
    pub player_name: [String; 10],
    pub ip_addresses: [String; 10],
    pub pilot_videos: bool,
    pub use_non_weapon_effects: bool,
    pub use_local_shadows: bool,
    pub async_mouse: bool,
    pub fog_pos: i32,
    pub save_transcripts: bool,
    pub tutorials: bool,
    /// Hot keys by command index (`[Keyboard] Key<n>`).
    pub keys: Vec<u32>,
    pub way_point_key: i32,
}

impl Default for Prefs {
    /// The `CPrefs` constructor.
    fn default() -> Prefs {
        Prefs {
            digital_master_volume: 255,
            music_volume: 64,
            sfx_volume: 64,
            radio_volume: 64,
            betty_volume: 64,
            use_shadows: true,
            use_water_interest_texture: true,
            use_high_object_detail: true,
            game_difficulty: 0,
            use_unlimited_ammo: true,
            renderer: 0,
            resolution_x: 800,
            resolution_y: 600,
            bit_depth: 32,
            display_number: 0,
            full_screen: false,
            gamma_level: 0,
            use_left_right_mouse_profile: true,
            base_color: 0xffff_7e00,
            highlight_color: 0xffff_7e00,
            faction: 0,
            insignia_file: String::new(),
            unit_name: Default::default(),
            player_name: Default::default(),
            ip_addresses: Default::default(),
            pilot_videos: true,
            use_non_weapon_effects: false,
            use_local_shadows: true,
            async_mouse: false,
            fog_pos: 50,
            save_transcripts: false,
            tutorials: true,
            keys: COMMANDS.iter().map(|c| c.key).collect(),
            way_point_key: 0,
        }
    }
}

const RES_WIDTHS: [(i32, i32); 5] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 1024),
    (1600, 1200),
];

impl Prefs {
    /// `CPrefs::load` over a parsed `options.cfg`, with its per-key defaults.
    pub fn from_fit(file: &mut FitIni) -> Prefs {
        let mut p = Prefs::default();
        if file.seek_block("MechCommander2").is_ok() {
            let l = |f: &FitIni, k: &str, d: i32| f.long(k).map_or(d, |v| v as i32);
            let b = |f: &FitIni, k: &str, d: bool| f.boolean(k).unwrap_or(d);
            p.digital_master_volume = l(file, "DigitalMasterVolume", 255);
            p.music_volume = l(file, "MusicVolume", 64);
            p.radio_volume = l(file, "RadioVolume", 64);
            p.sfx_volume = l(file, "SFXVolume", 64);
            p.betty_volume = l(file, "BettyVolume", 64);
            p.use_shadows = b(file, "Shadows", true);
            p.use_water_interest_texture = b(file, "DetailTexture", true);
            p.use_high_object_detail = b(file, "HighObjectDetail", true);
            p.game_difficulty = l(file, "Difficulty", 1);
            p.use_unlimited_ammo = b(file, "UnlimitedAmmo", true);
            p.renderer = l(file, "Rasterizer", 0);
            if !(0..=3).contains(&p.renderer) {
                p.renderer = 0;
            }
            if let Some(idx) = file.long("Resolution") {
                if let Some(&(w, h)) = RES_WIDTHS.get(idx.clamp(0, 4) as usize) {
                    p.resolution_x = w;
                    p.resolution_y = h;
                }
            }
            p.resolution_x = l(file, "ResolutionX", p.resolution_x);
            p.resolution_y = l(file, "ResolutionY", p.resolution_y);
            p.full_screen = b(file, "FullScreen", false);
            p.display_number = l(file, "DisplayNumber", 0);
            p.gamma_level = l(file, "Brightness", 0);
            p.use_left_right_mouse_profile = b(file, "useLeftRightMouseProfile", true);
            for i in 0..10 {
                match file.string(&format!("PlayerName{i}")) {
                    Some(s) => p.player_name[i] = s,
                    None if i == 0 => {
                        p.player_name[0] = file.string("PlayerName").unwrap_or_default();
                        p.unit_name[0] = file.string("UnitName").unwrap_or_default();
                        break;
                    }
                    None => {}
                }
                if let Some(s) = file.string(&format!("IPAddress{i}")) {
                    p.ip_addresses[i] = s;
                }
                if let Some(s) = file.string(&format!("UnitName{i}")) {
                    p.unit_name[i] = s;
                }
            }
            p.base_color = file.color("BaseColor").unwrap_or(0xffff_7e00);
            p.highlight_color = file.color("Highlightcolor").unwrap_or(0xffff_7e00);
            p.faction = l(file, "faction", 0);
            p.insignia_file = file.string("InsigniaFile").unwrap_or_default();
            p.pilot_videos = b(file, "PilotVideos", true);
            p.use_local_shadows = b(file, "UseLocalShadows", true);
            p.use_non_weapon_effects = b(file, "UseNonWeaponEffects", false);
            p.async_mouse = b(file, "AsyncMouse", false);
            p.fog_pos = l(file, "FogPos", 50);
            p.bit_depth = match file.read_i8("BitDepth") {
                // the Microsoft format stored 0 = 16 bit, 1 = 32 bit
                Ok(0) => 16,
                Ok(1) => 32,
                Ok(v) => i32::from(v),
                Err(_) => 32,
            };
            if p.bit_depth != 0 && p.renderer == 3 {
                p.bit_depth = 16;
            }
            p.save_transcripts = b(file, "SaveTranscripts", false);
            p.tutorials = b(file, "Tutorials", true);
        }
        if file.seek_block("Keyboard").is_ok() {
            for (i, k) in p.keys.iter_mut().enumerate().take(MAX_COMMAND) {
                if let Some(v) = file.long(&format!("Key{i}")) {
                    *k = v as u32;
                }
            }
            p.way_point_key = file.long_or("WayPointKey", 0) as i32;
            if let Some(v) = file.boolean("UseLeftRightMouseProfile") {
                p.use_left_right_mouse_profile = v;
            }
        }
        p
    }

    /// `CPrefs::save` + `saveHotKeys`: the `options.cfg` bytes.
    pub fn to_fit(&self) -> Vec<u8> {
        let mut w = FitWriter::new();
        w.block("MechCommander2");
        w.write_i64("DigitalMasterVolume", self.digital_master_volume.into());
        w.write_i64("MusicVolume", self.music_volume.into());
        w.write_i64("RadioVolume", self.radio_volume.into());
        w.write_i64("SFXVolume", self.sfx_volume.into());
        w.write_i64("BettyVolume", self.betty_volume.into());
        w.write_bool("Shadows", self.use_shadows);
        w.write_bool("DetailTexture", self.use_water_interest_texture);
        w.write_bool("HighObjectDetail", self.use_high_object_detail);
        w.write_i64("Difficulty", self.game_difficulty.into());
        w.write_bool("UnlimitedAmmo", self.use_unlimited_ammo);
        w.write_i64("Rasterizer", self.renderer.into());
        w.write_i64("ResolutionX", self.resolution_x.into());
        w.write_i64("ResolutionY", self.resolution_y.into());
        w.write_bool("FullScreen", self.full_screen);
        w.write_i64("DisplayNumber", self.display_number.into());
        w.write_i64("Brightness", self.gamma_level.into());
        w.write_bool(
            "useLeftRightMouseProfile",
            self.use_left_right_mouse_profile,
        );
        for i in 0..10 {
            w.write_string(&format!("PlayerName{i}"), &self.player_name[i]);
            w.write_string(&format!("UnitName{i}"), &self.unit_name[i]);
            w.write_string(&format!("IPAddress{i}"), &self.ip_addresses[i]);
        }
        w.write_i64("BaseColor", i64::from(self.base_color as i32));
        w.write_i64("Highlightcolor", i64::from(self.highlight_color as i32));
        w.write_i64("faction", self.faction.into());
        w.write_string("InsigniaFile", &self.insignia_file);
        w.write_bool("PilotVideos", self.pilot_videos);
        w.write_bool("UseLocalShadows", self.use_local_shadows);
        w.write_bool("UseNonWeaponEffects", self.use_non_weapon_effects);
        w.write_bool("AsyncMouse", self.async_mouse);
        w.write_i64("FogPos", self.fog_pos.into());
        w.write_i8("BitDepth", self.bit_depth as i8);
        w.write_bool("SaveTranscripts", self.save_transcripts);
        w.write_bool("Tutorials", self.tutorials);
        w.block("Keyboard");
        for (i, k) in self.keys.iter().enumerate() {
            w.write_i64(&format!("Key{i}"), i64::from(*k as i32));
        }
        w.write_i64("WayPointKey", self.way_point_key.into());
        w.write_bool(
            "UseLeftRightMouseProfile",
            self.use_left_right_mouse_profile,
        );
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_options_cfg() {
        let mut p = Prefs {
            music_volume: 12,
            base_color: 0xfffa_e525,
            bit_depth: 16,
            resolution_x: 1920,
            resolution_y: 1080,
            ..Prefs::default()
        };
        p.player_name[0] = "Justin".into();
        p.keys[3] = 0x41 | crate::hotkeys::CTRL;
        let bytes = p.to_fit();
        let q = Prefs::from_fit(&mut FitIni::from_bytes(bytes).unwrap());
        assert_eq!(p, q);
    }

    #[test]
    fn reads_the_microsoft_resolution_index() {
        let src = "FITini\r\n[MechCommander2]\r\nl Resolution = 2\r\nc BitDepth = 0\r\nl Difficulty = 3\r\nl BaseColor = 0xfae525\r\n[Keyboard]\r\nl Key0 = 77\r\n[End]\r\nFITend\r\n";
        let p = Prefs::from_fit(&mut FitIni::from_bytes(src.as_bytes().to_vec()).unwrap());
        assert_eq!(
            (p.resolution_x, p.resolution_y, p.bit_depth),
            (1024, 768, 16)
        );
        assert_eq!(p.game_difficulty, 3);
        assert_eq!(p.base_color, 0x00fa_e525);
        assert_eq!(p.keys[0], 77);
        assert_eq!(p.keys[1], COMMANDS[1].key);
    }
}
