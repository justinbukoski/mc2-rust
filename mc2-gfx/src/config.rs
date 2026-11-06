//! Runtime configuration (environment variables for now).
//!
//! | variable            | meaning                                             | default |
//! |---------------------|-----------------------------------------------------|---------|
//! | MC2_RENDER_SCALE    | internal render size = drawable size * scale        | 1.0     |
//! | MC2_UI_SCALE        | extra zoom of draws flagged as UI layer             | 1.0     |
//! | MC2_MSAA            | 1, 2 or 4 samples                                   | 4       |
//! | MC2_ANISO           | anisotropic filtering clamp (1..16)                 | 8       |
//! | MC2_MIPMAPS         | 0 disables mip generation                           | 1       |
//! | MC2_HIRES_DIR       | folder with `<key>.png` replacement textures        | unset   |
//! | MC2_ART_DIR        | original art pack folder (`<ShapeName>.glb`)        | unset   |
//! | MC2_HIRES_DUMP      | 1 = write original textures to `<dir>/_dump/<key>.png` | 0    |
//! | MC2_VSYNC           | 0 = present immediately                             | 1       |

use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct RenderConfig {
    pub render_scale: f32,
    pub ui_scale: f32,
    pub msaa: u32,
    pub aniso: u16,
    pub mipmaps: bool,
    pub hires_dir: Option<PathBuf>,
    pub hires_dump: bool,
    /// Original art pack folder (`<ShapeName>.glb` models); `None` keeps the original shapes.
    pub art_dir: Option<PathBuf>,
    pub vsync: bool,
    /// Reproduce the legacy GL renderer's R/B swap on untextured vertex colours.
    pub gl_compat_untextured_rb: bool,
    /// Hardware Rasterizer off (`renderer` != 0): ask for the software (fallback) adapter first.
    pub software_adapter: bool,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            render_scale: 1.0,
            ui_scale: 1.0,
            msaa: 4,
            aniso: 8,
            mipmaps: true,
            hires_dir: None,
            hires_dump: false,
            art_dir: None,
            vsync: true,
            gl_compat_untextured_rb: false,
            software_adapter: false,
        }
    }
}

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_flag(name: &str) -> Option<bool> {
    let v = std::env::var(name).ok()?;
    Some(!matches!(v.trim(), "0" | "false" | "off" | "no" | ""))
}

impl RenderConfig {
    pub fn from_env() -> Self {
        let mut c = Self::default();
        if let Some(v) = env_f32("MC2_RENDER_SCALE") {
            c.render_scale = v.clamp(0.25, 4.0);
        }
        if let Some(v) = env_f32("MC2_UI_SCALE") {
            c.ui_scale = v.clamp(0.25, 8.0);
        }
        if let Some(v) = env_f32("MC2_MSAA") {
            c.msaa = match v as u32 {
                0 | 1 => 1,
                2 | 3 => 2,
                _ => 4,
            };
        }
        if let Some(v) = env_f32("MC2_ANISO") {
            c.aniso = (v as u16).clamp(1, 16);
        }
        if let Some(v) = env_flag("MC2_MIPMAPS") {
            c.mipmaps = v;
        }
        if let Ok(d) = std::env::var("MC2_HIRES_DIR") {
            if !d.is_empty() {
                c.hires_dir = Some(PathBuf::from(d));
            }
        }
        if let Ok(d) = std::env::var("MC2_ART_DIR") {
            if !d.is_empty() {
                c.art_dir = Some(PathBuf::from(d));
            }
        }
        if let Some(v) = env_flag("MC2_HIRES_DUMP") {
            c.hires_dump = v;
        }
        if let Some(v) = env_flag("MC2_VSYNC") {
            c.vsync = v;
        }
        c
    }

    /// Size of the internal render target for a given drawable size.
    pub fn target_size(&self, drawable_w: u32, drawable_h: u32) -> (u32, u32) {
        let s = |v: u32| ((v as f32 * self.render_scale).round() as u32).max(1);
        (s(drawable_w), s(drawable_h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_size_scales_and_never_zero() {
        let c = RenderConfig {
            render_scale: 2.0,
            ..Default::default()
        };
        assert_eq!(c.target_size(100, 50), (200, 100));
        let c = RenderConfig {
            render_scale: 0.25,
            ..Default::default()
        };
        assert_eq!(c.target_size(1, 1), (1, 1));
    }
}
