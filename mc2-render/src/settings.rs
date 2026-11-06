// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Graphics settings for the upscaled renderer (P5): native output size, MSAA and anisotropic
//! filtering in one struct.
//!
//! There is no UI scale setting: the C++ options offer none (`code/optionsscreen*.cpp`,
//! `code/prefs.cpp` have no such entry), and the screens always fit the window
//! (`mc2_ui::render::UiLayout`). [`GraphicsSettings::parse`] and
//! [`GraphicsSettings::from_render_config`] refuse a `ui_scale` / `MC2_UI_SCALE` that asks for
//! anything else, naming that.
//!
//! Original to this project (platform-level code); nothing here touches the simulation.
//! Defaults equal the renderer's behaviour before P5: 1024x768 output, 4x MSAA,
//! 8x anisotropic filtering.
//!
//! [`GraphicsSettings::parse`] is the settings-text form (`hires_dir = PATH` / `off`). The
//! production `mc2` binary does not load that file: it applies `--hires-dir` and `$MC2_HIRES_DIR`
//! through [`RenderConfig`].

use mc2_gfx::config::RenderConfig;
use std::path::PathBuf;

/// Size of the default output (the layout the screens were authored for).
const LAYOUT_WIDTH: u32 = 1024;
const LAYOUT_HEIGHT: u32 = 768;

/// Why a `ui_scale` is refused.
pub const NO_UI_SCALE: &str =
    "there is no UI scale option: MechCommander 2 has none and the screens always fit the window";

#[derive(Clone, Debug, PartialEq)]
pub struct GraphicsSettings {
    /// Native output (window / render target) size in pixels, independent of the layout.
    pub output_width: u32,
    pub output_height: u32,
    /// 1, 2 or 4 samples.
    pub msaa: u32,
    /// Anisotropic filtering clamp, `1..=16`.
    pub aniso: u16,
    /// Per-pixel terrain / object lighting (see [`crate::visual`]); off = today's output.
    pub enhanced_lighting: bool,
    /// Smoother animated water with blended detail frames and a sun glint; off = today's output.
    pub enhanced_water: bool,
    /// Upscaled texture pack directory (`<key>.png` replacements). `None` is off.
    pub hires_dir: Option<PathBuf>,
    /// `art = DIR`: original art pack (glTF models keyed by shape name); `off` keeps the original shapes.
    pub art_dir: Option<PathBuf>,
}

impl Default for GraphicsSettings {
    fn default() -> Self {
        let rc = RenderConfig::default();
        GraphicsSettings {
            output_width: LAYOUT_WIDTH,
            output_height: LAYOUT_HEIGHT,
            msaa: rc.msaa,
            aniso: rc.aniso,
            enhanced_lighting: false,
            enhanced_water: false,
            hires_dir: None,
            art_dir: None,
        }
    }
}

fn parse_switch(v: &str) -> Option<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => Some(true),
        "0" | "off" | "false" | "no" => Some(false),
        _ => None,
    }
}

impl GraphicsSettings {
    /// Parses `key = value` lines (`#` starts a comment): `width`, `height`, `msaa`, `aniso`,
    /// `enhanced_lighting`, `enhanced_water` (`on`/`off`), `hires_dir` (a pack folder, or `off`)
    /// (`ui_scale` is refused, see [`NO_UI_SCALE`]). Missing keys keep the defaults; unknown keys
    /// and bad values are errors.
    pub fn parse(text: &str) -> Result<GraphicsSettings, String> {
        let mut s = GraphicsSettings::default();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("line {}: expected key = value", i + 1))?;
            let (k, v) = (k.trim(), v.trim());
            let bad = |what: &str| format!("line {}: bad {what} {v:?}", i + 1);
            match k {
                "width" => s.output_width = v.parse().map_err(|_| bad("width"))?,
                "height" => s.output_height = v.parse().map_err(|_| bad("height"))?,
                "ui_scale" => return Err(format!("line {}: ui_scale: {NO_UI_SCALE}", i + 1)),
                "msaa" => s.msaa = v.parse().map_err(|_| bad("msaa"))?,
                "aniso" => s.aniso = v.parse().map_err(|_| bad("aniso"))?,
                "enhanced_lighting" => {
                    s.enhanced_lighting = parse_switch(v).ok_or_else(|| bad("switch"))?
                }
                "enhanced_water" => {
                    s.enhanced_water = parse_switch(v).ok_or_else(|| bad("switch"))?
                }
                "hires_dir" => {
                    s.hires_dir = if v.is_empty()
                        || v.eq_ignore_ascii_case("off")
                        || v.eq_ignore_ascii_case("none")
                        || v == "0"
                    {
                        None
                    } else {
                        Some(PathBuf::from(v))
                    };
                }
                "art" => {
                    s.art_dir = if v.is_empty()
                        || v.eq_ignore_ascii_case("off")
                        || v.eq_ignore_ascii_case("none")
                        || v == "0"
                    {
                        None
                    } else {
                        Some(PathBuf::from(v))
                    };
                }
                _ => return Err(format!("line {}: unknown key {k:?}", i + 1)),
            }
        }
        Ok(s.sanitized())
    }

    /// The settings the environment asks for (`MC2_MSAA`, `MC2_ANISO`, via
    /// [`RenderConfig::from_env`]); the output size stays at the layout size until the front end
    /// sets it from its window or target. A non-default `MC2_UI_SCALE` is an error.
    pub fn from_render_config(rc: &RenderConfig) -> Result<GraphicsSettings, String> {
        Self::from_render_config_with(rc, |k| std::env::var(k).ok())
    }

    /// [`Self::from_render_config`] with the environment given by `get`: besides the render
    /// config, `MC2_ENHANCED_LIGHTING` and `MC2_ENHANCED_WATER` (`on`/`off`, `1`/`0`...) switch
    /// the two visual upgrades; unset means off, an unparsable value is an error.
    pub fn from_render_config_with(
        rc: &RenderConfig,
        get: impl Fn(&str) -> Option<String>,
    ) -> Result<GraphicsSettings, String> {
        if rc.ui_scale != 1.0 {
            return Err(format!("MC2_UI_SCALE: {NO_UI_SCALE}"));
        }
        let switch = |key: &str| -> Result<bool, String> {
            match get(key) {
                None => Ok(false),
                Some(v) => parse_switch(v.trim())
                    .ok_or_else(|| format!("{key}: expected a switch (on/off), got {v:?}")),
            }
        };
        Ok(GraphicsSettings {
            msaa: rc.msaa,
            aniso: rc.aniso,
            enhanced_lighting: switch("MC2_ENHANCED_LIGHTING")?,
            enhanced_water: switch("MC2_ENHANCED_WATER")?,
            hires_dir: rc.hires_dir.clone(),
            art_dir: rc.art_dir.clone(),
            ..GraphicsSettings::default()
        }
        .sanitized())
    }

    /// Clamps every field to its supported range.
    pub fn sanitized(mut self) -> GraphicsSettings {
        self.output_width = self.output_width.clamp(1, 16384);
        self.output_height = self.output_height.clamp(1, 16384);
        self.msaa = match self.msaa {
            0 | 1 => 1,
            2 | 3 => 2,
            _ => 4,
        };
        self.aniso = self.aniso.clamp(1, 16);
        self
    }

    /// The device configuration these settings imply, on top of `base` (hi-res folder, vsync...).
    pub fn apply_to(&self, base: &RenderConfig) -> RenderConfig {
        let s = self.clone().sanitized();
        RenderConfig {
            msaa: s.msaa,
            aniso: s.aniso,
            hires_dir: s.hires_dir.clone().or_else(|| base.hires_dir.clone()),
            art_dir: s.art_dir.clone().or_else(|| base.art_dir.clone()),
            ..base.clone()
        }
    }

    /// The visual-upgrade switches for [`crate::scene::SceneRenderer::set_visual`].
    pub fn visual(&self) -> crate::visual::VisualOptions {
        crate::visual::VisualOptions {
            enhanced_lighting: self.enhanced_lighting,
            enhanced_water: self.enhanced_water,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_todays_behaviour() {
        let s = GraphicsSettings::default();
        let rc = RenderConfig::default();
        assert_eq!((s.output_width, s.output_height), (1024, 768));
        assert_eq!((s.msaa, s.aniso), (4, 8));
        assert_eq!(s.apply_to(&rc).msaa, rc.msaa);
        assert_eq!(s.apply_to(&rc).aniso, rc.aniso);
        assert_eq!(GraphicsSettings::parse("").unwrap(), s);
        // both visual upgrades default OFF
        assert!(!s.enhanced_lighting && !s.enhanced_water);
        assert_eq!(s.visual(), crate::visual::VisualOptions::default());
        assert!(s.hires_dir.is_none());
    }

    #[test]
    fn hires_dir_parses_and_applies() {
        let on = GraphicsSettings::parse("hires_dir = /pack").unwrap();
        assert_eq!(on.hires_dir.as_deref(), Some(std::path::Path::new("/pack")));
        let off = GraphicsSettings::parse("hires_dir = off").unwrap();
        assert!(off.hires_dir.is_none());
        let rc = on.apply_to(&RenderConfig::default());
        assert_eq!(rc.hires_dir.as_deref(), Some(std::path::Path::new("/pack")));
        let env = RenderConfig {
            hires_dir: Some(PathBuf::from("/from-env")),
            ..RenderConfig::default()
        };
        assert_eq!(
            GraphicsSettings::default().apply_to(&env).hires_dir,
            env.hires_dir
        );
        let from_rc = GraphicsSettings::from_render_config_with(&env, |_| None).unwrap();
        assert_eq!(from_rc.hires_dir, env.hires_dir);
    }

    #[test]
    fn visual_switches_parse_independently() {
        let l = GraphicsSettings::parse("enhanced_lighting = on").unwrap();
        assert!(l.enhanced_lighting && !l.enhanced_water);
        let w = GraphicsSettings::parse("enhanced_water = TRUE # glint\nmsaa = 2").unwrap();
        assert!(w.enhanced_water && !w.enhanced_lighting && w.msaa == 2);
        let both = GraphicsSettings::parse("enhanced_water=1\nenhanced_lighting=1").unwrap();
        assert!(both.visual().enhanced_water && both.visual().enhanced_lighting);
        let off = GraphicsSettings::parse("enhanced_water = off\nenhanced_lighting = 0").unwrap();
        assert_eq!(off, GraphicsSettings::default());
        assert!(GraphicsSettings::parse("enhanced_water = maybe").is_err());
        // not part of the device configuration
        assert_eq!(both.apply_to(&RenderConfig::default()).msaa, 4);
    }

    #[test]
    fn environment_switches_the_upgrades_independently() {
        let rc = RenderConfig::default();
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            GraphicsSettings::from_render_config_with(&rc, move |k| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            })
        };
        assert_eq!(env(&[]).unwrap().visual(), Default::default());
        let l = env(&[("MC2_ENHANCED_LIGHTING", "on")]).unwrap();
        assert!(l.visual().enhanced_lighting && !l.visual().enhanced_water);
        let w = env(&[("MC2_ENHANCED_WATER", "1")]).unwrap();
        assert!(w.visual().enhanced_water && !w.visual().enhanced_lighting);
        assert!(env(&[("MC2_ENHANCED_WATER", "maybe")]).is_err());
    }

    #[test]
    fn parse_reads_keys_and_clamps() {
        let s =
            GraphicsSettings::parse("width = 2560\nheight=1440 # qhd\n\nmsaa = 2\naniso = 16\n")
                .unwrap();
        assert_eq!((s.output_width, s.output_height), (2560, 1440));
        assert_eq!((s.msaa, s.aniso), (2, 16));
        let c = GraphicsSettings::parse("msaa = 8\naniso = 99\nwidth = 0").unwrap();
        assert_eq!((c.msaa, c.aniso, c.output_width), (4, 16, 1));
        assert!(GraphicsSettings::parse("bogus = 1").is_err());
        assert!(GraphicsSettings::parse("width = wide").is_err());
        assert!(GraphicsSettings::parse("width").is_err());
        let rc = s.apply_to(&RenderConfig::default());
        assert_eq!((rc.msaa, rc.aniso), (2, 16));
    }

    #[test]
    fn a_ui_scale_is_refused_because_the_original_has_none() {
        for v in ["0.5", "1", "1.0", "2"] {
            let e = GraphicsSettings::parse(&format!("ui_scale = {v}")).unwrap_err();
            assert!(e.contains("no UI scale option"), "{v}: {e}");
        }
        let rc = RenderConfig {
            ui_scale: 0.5,
            ..RenderConfig::default()
        };
        let e = GraphicsSettings::from_render_config(&rc).unwrap_err();
        assert!(
            e.contains("MC2_UI_SCALE") && e.contains("no UI scale option"),
            "{e}"
        );
        assert!(GraphicsSettings::from_render_config(&RenderConfig::default()).is_ok());
    }
}
