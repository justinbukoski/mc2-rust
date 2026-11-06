// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Mission loading: the `.fit` (camera, light, water, terrain tuning, parts) and the `.pak`
//! packets for the terrain grid and the placed terrain objects.
//!
//! Ported from `code/mission.cpp` (the order in which the fit blocks are read), `Terrain::load`
//! (the `[Water]`/`[Terrain]` blocks, `terrain.cpp`), `TerrainColorMap::init` (the tiling
//! factors, `terrtxm2.cpp`) and `GameObjectManager::countTerrainObjects` (packet layout,
//! `objmgr.cpp`; decoded by `mc2-assets`).

use crate::error::{RenderError, Result};
use crate::game::GameData;
use crate::light::LightModel;
use crate::terrain::TerrainMap;
use crate::weather::WeatherConfig;
use mc2_assets::map::{decode_packet, MissionPacket, TerrainObject};
use mc2_formats::ini::FitIni;

/// `[Water]` block (`Terrain::load`).
#[derive(Clone, Debug, PartialEq)]
pub struct WaterParams {
    /// Water surface height (`Elevation`).
    pub elevation: f32,
    /// Wave frequency in degrees per second (`Frequency`).
    pub frequency: f32,
    /// Wave amplitude (`Ampliture`, sic).
    pub amplitude: f32,
    /// Vertex alpha colours: `0xAARRGGBB`; only the alpha byte matters.
    pub alpha_shallow: u32,
    pub alpha_middle: u32,
    pub alpha_deep: u32,
    pub alpha_depth: f32,
    pub shallow_depth: f32,
}

impl Default for WaterParams {
    fn default() -> Self {
        // Terrain:: statics before load()
        Self {
            elevation: 0.0,
            frequency: 4.0,
            amplitude: 10.0,
            alpha_shallow: 0x3f00_0000,
            alpha_middle: 0xaf00_0000,
            alpha_deep: 0xff00_0000,
            alpha_depth: 0.0,
            shallow_depth: 0.0,
        }
    }
}

/// `[Terrain]` block plus the colour map tiling (`TerrainColorMap::init`).
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainParams {
    pub user_min: i32,
    pub user_max: i32,
    pub detail_tiling: f32,
    pub water_tiling: f32,
    pub water_detail_tiling: f32,
    /// Terrain-only darkening from the weather (`TerrainQuad::rainLightLevel`); `1.0` = none. Set per frame by the
    /// host from [`crate::weather::Weather::terrain_light_scale`].
    pub light_scale: f32,
    /// Terrain-only lightning brightness added after the texture (`TerrainQuad::lighteningLevel`), 0..1; `0.0` = none.
    /// Set per frame from [`crate::weather::Weather::terrain_lightning`].
    pub lightning: f32,
    /// `useWaterInterestTexture` (the options screen's Terrain Detail): the detail bitmap over the ground and the sky
    /// reflection over the water (`quad.cpp:1538`, `:2173`). Set per frame by the host from the player's options.
    pub detail_texture: bool,
}

impl Default for TerrainParams {
    fn default() -> Self {
        Self {
            user_min: 0,
            user_max: 0,
            detail_tiling: 30.0,
            water_tiling: 48.0,
            water_detail_tiling: 45.0,
            light_scale: 1.0,
            lightning: 0.0,
            detail_texture: true,
        }
    }
}

/// One `[PartN]` of the mission: a unit or building placed by the designers.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub index: usize,
    pub object_number: i32,
    pub x: f32,
    pub y: f32,
    /// Degrees (`Rotation`).
    pub rotation: f32,
    pub team_id: i32,
    /// `CommanderId`; the single-player home commander is `0` (`mission.cpp:2081-2082`).  `None`
    /// when the key is absent (the C++ reader asserts it, `mission.cpp:2620-2626`).
    pub commander_id: Option<i32>,
    pub active: bool,
    pub exists: bool,
    pub damage: f32,
    pub csv_file: Option<String>,
    /// Paint colours (`0xAARRGGBB`): `BaseColor`, `HighlightColor1`, `HighlightColor2`; `None`
    /// when the key is absent, which the C++ reader replaces with the prefs colours
    /// (`mission.cpp:2644-2657`).
    pub base_color: Option<u32>,
    pub highlight_color1: Option<u32>,
    pub highlight_color2: Option<u32>,
}

/// A source of the paint colours `(base, highlight)` (`resetPaintScheme(highlight, highlight,
/// base)`, `mission.cpp:1077-1081`).  A launch uses two: the home commander's unit takes its
/// launch's colours (a campaign's, else `options.cfg`'s), and the `[PartN]` colours that are absent
/// take the prefs colours, which are always `options.cfg`'s (`mission.cpp:2644-2657`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PaintSource {
    /// Base and highlight as `0x00RRGGBB`, used unswapped as the Windows build does (see
    /// `apply_paint_scheme`).  `options.cfg` gives these as stored; a campaign launch gives its
    /// `ForceMech::colors` (base, then highlight; both highlight masks take `highlight`).
    Scheme { base: u32, highlight: u32 },
    /// No colours were supplied.  The units that need them keep their raw masks and are reported
    /// by `PopulateReport::unpainted`; no colour is invented for them.
    #[default]
    Unavailable,
}

/// Why a mech or vehicle cannot take its paint (see [`Part::paint`]); such a unit is reported in
/// `PopulateReport::unpainted`, and the launch that has one fails as unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintGap {
    /// The `[PartN]` has no `CommanderId`, so its owner is unknown.
    NoCommander,
    /// The home commander's unit has no colours: the launch's own source is unavailable.
    NoHomeColours,
    /// A `[PartN]` colour is absent and the prefs (`options.cfg`) colours are unavailable.
    NoPrefs,
}

impl PaintGap {
    /// The input that is missing, as the unavailable message names it.
    pub fn missing_input(self) -> &'static str {
        match self {
            PaintGap::NoCommander => "a CommanderId in its [PartN] block",
            PaintGap::NoHomeColours => {
                "the home commander's colours (options.cfg, or the campaign launch's colours)"
            }
            PaintGap::NoPrefs => "the prefs colours (options.cfg) for an absent colour key",
        }
    }
}

impl Part {
    /// The `(red, green, blue)` scheme for this part's mask: red is `HighlightColor1`, green
    /// `HighlightColor2`, blue `BaseColor` (`resetPaintScheme(highlightColor1, highlightColor2,
    /// baseColor)`, `mission.cpp:1077-1085`).  The home commander (`CommanderId` 0) takes `home`
    /// and ignores its own colours (`mission.cpp:1077-1081`).  Any other commander keeps its own
    /// colours, and an absent one takes `prefs` (`mission.cpp:2644-2657`).
    pub fn paint(
        &self,
        home: PaintSource,
        prefs: PaintSource,
    ) -> std::result::Result<(u32, u32, u32), PaintGap> {
        let commander = self.commander_id.ok_or(PaintGap::NoCommander)?;
        if commander == 0 {
            let PaintSource::Scheme { base, highlight } = home else {
                return Err(PaintGap::NoHomeColours);
            };
            return Ok((highlight, highlight, base));
        }
        let (prefs_base, prefs_highlight) = match prefs {
            PaintSource::Scheme { base, highlight } => (Some(base), Some(highlight)),
            PaintSource::Unavailable => (None, None),
        };
        let base = self.base_color.or(prefs_base).ok_or(PaintGap::NoPrefs)?;
        let red = self
            .highlight_color1
            .or(prefs_highlight)
            .ok_or(PaintGap::NoPrefs)?;
        let green = self
            .highlight_color2
            .or(prefs_highlight)
            .ok_or(PaintGap::NoPrefs)?;
        Ok((red, green, base))
    }
}

/// The prefs colours from `options.cfg` (`code/prefs.cpp:230-235`).  Same keys and defaults as
/// `mc2_ui::screens::prefs::Prefs::from_fit`, which the UI uses for the same file.  A missing file
/// is [`PaintSource::Unavailable`]; no colour is invented for it.
pub fn prefs_from_options(game: &mut GameData) -> PaintSource {
    match game.fit("options.cfg") {
        Ok(mut fit) => prefs_from_fit(&mut fit),
        Err(_) => PaintSource::Unavailable,
    }
}

/// [`prefs_from_options`] for an `options.cfg` that is already open.  A missing `MechCommander2`
/// block or key takes the C++ default `0xffff7e00` (`prefs.cpp:73-74`, `230-235`).
pub fn prefs_from_fit(file: &mut FitIni) -> PaintSource {
    const DEFAULT_COLOR: u32 = 0xffff_7e00;
    if file.seek_block("MechCommander2").is_err() {
        return PaintSource::Scheme {
            base: DEFAULT_COLOR,
            highlight: DEFAULT_COLOR,
        };
    }
    PaintSource::Scheme {
        base: color(file, "BaseColor").unwrap_or(DEFAULT_COLOR),
        highlight: color(file, "Highlightcolor").unwrap_or(DEFAULT_COLOR),
    }
}

pub struct Mission {
    pub name: String,
    pub fit: FitIni,
    pub light: LightModel,
    pub water: WaterParams,
    pub terrain_params: TerrainParams,
    /// `[Weather]` when the mission has the block (weather.cpp `Weather::init`).
    pub weather: Option<WeatherConfig>,
    pub terrain: TerrainMap,
    pub objects: Vec<TerrainObject>,
    pub parts: Vec<Part>,
    /// `[ColorMap] ColorMapName` when the mission borrows another mission's colour map.
    pub color_map_name: Option<String>,
}

impl Mission {
    /// Loads `data/missions/<name>.fit` and `.pak` through the VFS.
    pub fn load(game: &mut GameData, name: &str) -> Result<Self> {
        let mut fit = game.fit(&format!("data/missions/{name}.fit"))?;
        let light = LightModel::from_fit(&mut fit);
        let water = read_water(&mut fit);
        let terrain_params = read_terrain_params(&mut fit);
        let weather = WeatherConfig::from_fit(&mut fit);
        let parts = read_parts(&mut fit)?;
        let color_map_name = if fit.seek_block("ColorMap").is_ok() {
            fit.read_string("ColorMapName", 1023).ok()
        } else {
            None
        };

        let pak = game.pak(&format!("data/missions/{name}.pak"))?;
        let terrain = match decode_packet(0, &pak.read_packet(0)?)? {
            MissionPacket::Terrain { side, vertices } => {
                TerrainMap::try_new(side, vertices).map_err(RenderError::Other)?
            }
            _ => return Err(RenderError::Other("packet 0 is not terrain".into())),
        };
        let objects = if pak.packet_count() > 1 {
            match decode_packet(1, &pak.read_packet(1)?)? {
                MissionPacket::Objects(o) => o,
                _ => Vec::new(),
            }
        } else {
            Vec::new()
        };
        Ok(Mission {
            name: name.to_string(),
            fit,
            light,
            water,
            terrain_params,
            weather,
            terrain,
            objects,
            parts,
            color_map_name,
        })
    }
}

fn read_water(fit: &mut FitIni) -> WaterParams {
    let mut w = WaterParams::default();
    if fit.seek_block("Water").is_err() {
        return w;
    }
    w.elevation = fit.read_f32("Elevation").unwrap_or(w.elevation);
    w.frequency = fit.read_f32("Frequency").unwrap_or(w.frequency);
    w.amplitude = fit.read_f32("Ampliture").unwrap_or(w.amplitude);
    // The fit stores these as signed longs written from DWORDs; read them as `l` or `ul`.
    let dword = |fit: &FitIni, name: &str, def: u32| {
        fit.read_u32(name)
            .or_else(|_| fit.read_i32(name).map(|v| v as u32))
            .unwrap_or(def)
    };
    w.alpha_shallow = dword(fit, "AlphaShallow", w.alpha_shallow);
    w.alpha_middle = dword(fit, "AlphaMiddle", w.alpha_middle);
    w.alpha_deep = dword(fit, "AlphaDeep", w.alpha_deep);
    w.alpha_depth = fit.read_f32("AlphaDepth").unwrap_or(w.alpha_depth);
    w.shallow_depth = fit.read_f32("ShallowDepth").unwrap_or(w.shallow_depth);
    w
}

fn read_terrain_params(fit: &mut FitIni) -> TerrainParams {
    let mut t = TerrainParams::default();
    if fit.seek_block("Terrain").is_err() {
        return t;
    }
    t.user_min = fit.read_i32("UserMin").unwrap_or(0);
    t.user_max = fit.read_i32("UserMax").unwrap_or(0);
    t.detail_tiling = fit
        .read_f32("DetailTextureTilingFactor")
        .unwrap_or(t.detail_tiling);
    t.water_tiling = fit
        .read_f32("WaterTextureTilingFactor")
        .unwrap_or(t.water_tiling);
    t.water_detail_tiling = fit
        .read_f32("WaterDetailTilingFactor")
        .unwrap_or(t.water_detail_tiling);
    t
}

/// Colours are written from DWORDs as signed longs.
fn color(fit: &FitIni, name: &str) -> Option<u32> {
    fit.read_u32(name)
        .or_else(|_| fit.read_i32(name).map(|v| v as u32))
        .ok()
}

fn read_parts(fit: &mut FitIni) -> Result<Vec<Part>> {
    let mut parts = Vec::new();
    let n = if fit.seek_block("Parts").is_ok() {
        fit.read_u32("NumParts").unwrap_or(0)
    } else {
        0
    };
    // Every part owns a `[PartN]` block, so a count beyond the block count is corrupt data
    // (the loop below must not run for billions of iterations on it).
    if n as usize > fit.block_count() {
        return Err(RenderError::Other(format!(
            "NumParts = {n} but the mission has only {} blocks",
            fit.block_count()
        )));
    }
    for i in 1..=n as usize {
        if fit.seek_block(&format!("Part{i}")).is_err() {
            continue;
        }
        let object_number = fit
            .read_u32("ObjectNumber")
            .map(|v| v as i32)
            .or_else(|_| fit.read_i32("ObjectNumber"))
            .unwrap_or(0);
        parts.push(Part {
            index: i,
            object_number,
            x: fit.read_f32("PositionX").unwrap_or(0.0),
            y: fit.read_f32("PositionY").unwrap_or(0.0),
            rotation: fit.read_f32("Rotation").unwrap_or(0.0),
            team_id: fit.read_i8("TeamID").map(i32::from).unwrap_or(-1),
            commander_id: fit
                .read_i8("CommanderId")
                .map(i32::from)
                .or_else(|_| fit.read_i32("CommanderId"))
                .ok(),
            active: fit.read_i32("Active").unwrap_or(1) != 0,
            exists: fit.read_i32("Exists").unwrap_or(1) != 0,
            damage: fit.read_f32("Damage").unwrap_or(0.0),
            csv_file: fit.read_string("CSVFile", 255).ok(),
            base_color: color(fit, "BaseColor"),
            highlight_color1: color(fit, "HighlightColor1"),
            highlight_color2: color(fit, "HighlightColor2"),
        });
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fit(text: &str) -> FitIni {
        FitIni::from_bytes(text.as_bytes().to_vec()).unwrap()
    }

    #[test]
    fn water_block_reads_signed_alpha_colours() {
        let mut f = fit(
            "FITini\r\n[Water]\r\nf Elevation = 350.000000\r\nf Frequency = 8.000000\r\nf Ampliture = 3.000000\r\nul AlphaShallow = 2097152000\r\nul AlphaMiddle = -1358954496\r\nul AlphaDeep = -16777216\r\nf AlphaDepth = 15.000000\r\nf ShallowDepth = 20.000000\r\nFITend\r\n",
        );
        let w = read_water(&mut f);
        assert_eq!(w.elevation, 350.0);
        assert_eq!(w.amplitude, 3.0);
        assert_eq!(w.alpha_shallow >> 24, 0x7d);
        assert_eq!(w.alpha_middle >> 24, 0xaf);
        assert_eq!(w.alpha_deep >> 24, 0xff);
        assert_eq!(w.shallow_depth, 20.0);
    }

    #[test]
    fn terrain_params_default_to_the_original_tiling() {
        let mut f = fit("FITini\r\n[Cameras]\r\nf X = 1.0\r\nFITend\r\n");
        assert_eq!(read_terrain_params(&mut f), TerrainParams::default());
        let mut f = fit(
            "FITini\r\n[Terrain]\r\nl UserMin = 0\r\nl UserMax = 500\r\nf DetailTextureTilingFactor = 12.5\r\nf WaterTextureTilingFactor = 48.0\r\nf WaterDetailTilingFactor = 45.0\r\nFITend\r\n",
        );
        let t = read_terrain_params(&mut f);
        assert_eq!(t.user_max, 500);
        assert_eq!(t.detail_tiling, 12.5);
    }

    #[test]
    fn absurd_part_counts_are_rejected_not_looped() {
        let mut f = fit("FITini\r\n[Parts]\r\nul NumParts = 4294967295\r\n[Part1]\r\nul ObjectNumber = 1\r\nFITend\r\n");
        assert!(read_parts(&mut f).is_err());
        // a count that matches the blocks is fine, a missing Parts block means no parts
        let mut f = fit("FITini\r\n[Cameras]\r\nf X = 1.0\r\nFITend\r\n");
        assert!(read_parts(&mut f).unwrap().is_empty());
    }

    #[test]
    fn parts_are_read_in_order() {
        let mut f = fit(
            "FITini\r\n[Parts]\r\nul NumParts = 2\r\n[Part1]\r\nul ObjectNumber = 45\r\nc TeamID = 0\r\nf PositionX = 2496.0\r\nf PositionY = -1941.5\r\nf Rotation = 180.0\r\nl Active = 1\r\nl Exists = 1\r\nf Damage = 0.0\r\nst CSVFile = \"Werewolf\"\r\n[Part2]\r\nul ObjectNumber = 874\r\nc TeamID = 1\r\nf PositionX = 1.0\r\nf PositionY = 2.0\r\nf Rotation = 0.0\r\nl Active = 0\r\nl Exists = 1\r\nf Damage = 10.0\r\nFITend\r\n",
        );
        let parts = read_parts(&mut f).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].object_number, 45);
        assert_eq!(parts[0].csv_file.as_deref(), Some("Werewolf"));
        assert_eq!(parts[0].x, 2496.0);
        assert_eq!(parts[1].object_number, 874);
        assert!(!parts[1].active);
        assert_eq!(parts[1].team_id, 1);
    }

    #[test]
    fn home_commander_parts_take_the_home_paint() {
        // Part1 is the home commander's (CommanderId 0); Part2 is another commander's.  The
        // own colours are small decimals so the order of the three masks is easy to read.
        let mut f = fit(
            "FITini\r\n[Parts]\r\nul NumParts = 2\r\n[Part1]\r\nul ObjectNumber = 45\r\nc TeamID = 0\r\nl CommanderId = 0\r\nul BaseColor = 4278255360\r\nul HighlightColor1 = 4278216704\r\nul HighlightColor2 = 4278216704\r\n[Part2]\r\nul ObjectNumber = 874\r\nc TeamID = 1\r\nl CommanderId = 3\r\nul BaseColor = 51\r\nul HighlightColor1 = 17\r\nul HighlightColor2 = 34\r\nFITend\r\n",
        );
        let parts = read_parts(&mut f).unwrap();
        assert_eq!(parts[0].commander_id, Some(0));
        assert_eq!(parts[1].commander_id, Some(3));
        let prefs = PaintSource::Scheme {
            base: 0x00fa_e525,
            highlight: 0x0007_083e,
        };
        // `resetPaintScheme(highlight, highlight, base)`: red and green masks take the highlight,
        // blue takes the base (`mission.cpp:1077-1081`).  The part's own colours are not used.
        assert_eq!(
            parts[0].paint(prefs, prefs),
            Ok((0x0007_083e, 0x0007_083e, 0x00fa_e525))
        );
        // A campaign launch paints the home unit with its own colours, not the prefs ones.
        let campaign = PaintSource::Scheme {
            base: 0x0010_40c0,
            highlight: 0x00c0_2030,
        };
        assert_eq!(
            parts[0].paint(campaign, prefs),
            Ok((0x00c0_2030, 0x00c0_2030, 0x0010_40c0))
        );
        // No home source: the home unit is a gap, and the other commander keeps its own colours.
        assert_eq!(
            parts[0].paint(PaintSource::Unavailable, prefs),
            Err(PaintGap::NoHomeColours)
        );
        assert_eq!(parts[1].paint(prefs, prefs), Ok((17, 34, 51)));
        assert_eq!(
            parts[1].paint(PaintSource::Unavailable, PaintSource::Unavailable),
            Ok((17, 34, 51))
        );
    }

    #[test]
    fn missing_commander_id_is_a_gap_never_a_colour() {
        // Neither the home scheme nor the part colours may stand in for an unknown owner.
        let mut f = fit("FITini\r\n[Parts]\r\nul NumParts = 1\r\n[Part1]\r\nul ObjectNumber = 1\r\nul BaseColor = 7\r\nul HighlightColor1 = 8\r\nul HighlightColor2 = 9\r\nFITend\r\n");
        let parts = read_parts(&mut f).unwrap();
        assert_eq!(parts[0].commander_id, None);
        let prefs = PaintSource::Scheme {
            base: 0x00fa_e525,
            highlight: 0x0007_083e,
        };
        assert_eq!(parts[0].paint(prefs, prefs), Err(PaintGap::NoCommander));
        assert_eq!(
            parts[0].paint(PaintSource::Unavailable, PaintSource::Unavailable),
            Err(PaintGap::NoCommander)
        );
    }

    #[test]
    fn absent_part_colours_take_the_prefs_colours_never_the_campaign_ones() {
        // `mission.cpp:2644-2657`: a non-home part with no BaseColor / HighlightColor takes
        // prefs.baseColor / prefs.highlightColor; the colours it does carry are kept.  The campaign
        // colours are a different source and must not stand in for them (review finding 4).
        let mut f = fit("FITini\r\n[Parts]\r\nul NumParts = 2\r\n[Part1]\r\nul ObjectNumber = 874\r\nc TeamID = 1\r\nl CommanderId = 2\r\nul HighlightColor1 = 17\r\nFITend\r\n[Part2]\r\nul ObjectNumber = 874\r\nc TeamID = 1\r\nl CommanderId = 2\r\nFITend\r\n");
        let parts = read_parts(&mut f).unwrap();
        assert_eq!(parts[0].base_color, None);
        assert_eq!(parts[0].highlight_color2, None);
        let prefs = PaintSource::Scheme {
            base: 0x00fa_e525,
            highlight: 0x0007_083e,
        };
        let campaign = PaintSource::Scheme {
            base: 0x0010_40c0,
            highlight: 0x00c0_2030,
        };
        assert_eq!(
            parts[0].paint(campaign, prefs),
            Ok((17, 0x0007_083e, 0x00fa_e525))
        );
        assert_eq!(
            parts[1].paint(campaign, prefs),
            Ok((0x0007_083e, 0x0007_083e, 0x00fa_e525))
        );
        // Without prefs there is nothing to fall back to, whatever the campaign supplied.
        assert_eq!(
            parts[0].paint(campaign, PaintSource::Unavailable),
            Err(PaintGap::NoPrefs)
        );
        assert_eq!(
            parts[1].paint(campaign, PaintSource::Unavailable),
            Err(PaintGap::NoPrefs)
        );
    }

    #[test]
    fn options_cfg_gives_the_prefs_with_the_cpp_default_for_a_missing_key() {
        // Keys and default of `prefs.cpp:230-235`.
        let mut f = fit("FITini\r\n[MechCommander2]\r\nul BaseColor = 4278255360\r\nFITend\r\n");
        assert_eq!(
            prefs_from_fit(&mut f),
            PaintSource::Scheme {
                base: 0xff00_ff00,
                highlight: 0xffff_7e00,
            }
        );
        let mut none = fit("FITini\r\n[Other]\r\nul X = 1\r\nFITend\r\n");
        assert_eq!(
            prefs_from_fit(&mut none),
            PaintSource::Scheme {
                base: 0xffff_7e00,
                highlight: 0xffff_7e00,
            }
        );
    }
}
