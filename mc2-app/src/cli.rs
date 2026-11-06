//! Command line of the `mc2` binary.

use crate::game::GameConfig;
use crate::locate::Hires;
use crate::script::InputScript;
use std::path::PathBuf;

pub const USAGE: &str = "\
usage: mc2 [--data DIR] [--mission NAME] [--size WxH] [--fullscreen]
           [--hires-dir DIR] [--voice-dir DIR]
       mc2 --headless --mission NAME [--frames N] [--screenshot OUT.png]
           [--input-script FILE] [--size WxH] [--fps N]
       mc2 --selftest-play [--data DIR]
       [--visibility-trace FILE] [--hires-dir DIR]
       mc2 --list-missions [--data DIR]

  --data DIR          game data root (system.cfg and the .fst archives); default $MC2_DATA,
                      then `data = DIR` in mc2.cfg next to mc2 or in the user config dir
  --version           print the version and exit
  --selftest-play     run mc2_01 headless 9,000 frames, then the objective check
  --mission NAME      start this mission (e.g. mc2_01)
  --headless          no window: render offscreen through the same path
  --frames N          headless frames to run (default 120)
  --screenshot FILE   headless: write the last frame as a PNG
  --input-script FILE headless: replay timed input events (see mc2_app::script)
  --visibility-trace FILE headless: write camera and visibility inputs per frame
  --size WxH          frame size in pixels (headless, default 1920x1080) or the
                      window size in points (default 1280x720)
  --fps N             headless render rate in virtual frames per second (default 30)
  --sim-hz N          simulation frames per second (default 30)
  --seed N            seed of the simulation's random streams (default 1)
  --rng MODE          simulation random numbers: msvc (default), glibc or pcg
  --no-shadows        no sun shadows
  --no-interpolation  draw the camera at simulation frames only
  --no-cursor         no stand-in cursor
  --fullscreen        borderless fullscreen window
  --hires-dir DIR|off upscaled textures: built from your data on first run by default; DIR uses a ready-made
                      pack, `off` keeps the original art (also $MC2_HIRES_DIR or `hires =` in mc2.cfg)
  --art-dir DIR       original art pack of .glb mech models (off by default; also $MC2_ART_DIR)
  --voice-dir DIR     optional pilot voice pack (off by default; also $MC2_VOICE_DIR or `voices =` in mc2.cfg)
  --list-missions     print the missions found in the data and exit
";

#[derive(Clone, Debug, Default)]
pub struct Cli {
    pub data: Option<PathBuf>,
    pub mission: Option<String>,
    pub headless: bool,
    pub frames: Option<u64>,
    pub screenshot: Option<PathBuf>,
    pub input_script: Option<PathBuf>,
    pub visibility_trace: Option<PathBuf>,
    pub size: Option<(u32, u32)>,
    pub fps: Option<u32>,
    pub sim_hz: Option<u32>,
    pub seed: Option<u32>,
    pub rng: Option<mc2_sim::rng::RngMode>,
    pub no_shadows: bool,
    pub no_interpolation: bool,
    pub no_cursor: bool,
    pub fullscreen: bool,
    pub list_missions: bool,
    /// `--hires-dir`: a pack folder or `off`. `None` leaves `$MC2_HIRES_DIR` / `mc2.cfg` / auto.
    pub hires_dir: Option<Hires>,
    /// Original art pack (`<ShapeName>.glb` models). `None` leaves `$MC2_ART_DIR` / off.
    pub art_dir: Option<PathBuf>,
    /// Optional pilot voice pack. `None` leaves `$MC2_VOICE_DIR` / mc2.cfg / off.
    pub voice_dir: Option<PathBuf>,
    pub help: bool,
    pub version: bool,
    pub selftest_play: bool,
}

pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    ((1..=16384).contains(&w) && (1..=16384).contains(&h)).then_some((w, h))
}

impl Cli {
    pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Cli, String> {
        let mut c = Cli::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
            match a.as_str() {
                "--data" => c.data = Some(PathBuf::from(val("--data")?)),
                "--mission" => c.mission = Some(val("--mission")?),
                "--headless" => c.headless = true,
                "--frames" => {
                    c.frames = Some(val("--frames")?.parse().map_err(|_| "bad --frames")?)
                }
                "--screenshot" => c.screenshot = Some(PathBuf::from(val("--screenshot")?)),
                "--input-script" => c.input_script = Some(PathBuf::from(val("--input-script")?)),
                "--visibility-trace" => {
                    c.visibility_trace = Some(PathBuf::from(val("--visibility-trace")?))
                }
                "--size" => {
                    let v = val("--size")?;
                    c.size = Some(parse_size(&v).ok_or_else(|| format!("bad --size {v}"))?)
                }
                "--fps" => c.fps = Some(val("--fps")?.parse().map_err(|_| "bad --fps")?),
                "--sim-hz" => {
                    c.sim_hz = Some(val("--sim-hz")?.parse().map_err(|_| "bad --sim-hz")?)
                }
                "--seed" => c.seed = Some(val("--seed")?.parse().map_err(|_| "bad --seed")?),
                "--rng" => {
                    c.rng = Some(match val("--rng")?.as_str() {
                        "msvc" => mc2_sim::rng::RngMode::LegacyMsvc,
                        "glibc" => mc2_sim::rng::RngMode::LegacyGlibc,
                        "pcg" => mc2_sim::rng::RngMode::Pcg32,
                        other => return Err(format!("bad --rng {other} (msvc, glibc or pcg)")),
                    })
                }
                "--no-shadows" => c.no_shadows = true,
                "--no-interpolation" => c.no_interpolation = true,
                "--no-cursor" => c.no_cursor = true,
                "--fullscreen" => c.fullscreen = true,
                "--hires-dir" => c.hires_dir = Some(Hires::parse(&val("--hires-dir")?)),
                "--voice-dir" => c.voice_dir = Some(PathBuf::from(val("--voice-dir")?)),
                "--art-dir" => c.art_dir = Some(PathBuf::from(val("--art-dir")?)),
                "--list-missions" => c.list_missions = true,
                "--selftest-play" => c.selftest_play = true,
                "-h" | "--help" => c.help = true,
                "-V" | "--version" => c.version = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        if c.headless && c.mission.is_none() && !c.list_missions && !c.help {
            return Err("--headless needs --mission".into());
        }
        Ok(c)
    }

    /// `--data`, `$MC2_DATA`, then `mc2.cfg` (see [`crate::locate`]); the error is the first-run message.
    pub fn data_dir(&self) -> Result<PathBuf, String> {
        let env = |k: &str| std::env::var_os(k);
        let cands = crate::locate::config_candidates(crate::locate::exe_dir().as_deref(), &env);
        crate::locate::resolve_data(self.data.as_deref(), &env, &cands)
    }

    /// `--hires-dir`, `$MC2_HIRES_DIR`, then `mc2.cfg`; with none of them the pack is built
    /// from the player's data ([`Hires::Auto`], see [`crate::hires`]).
    pub fn hires_setting(&self) -> Result<Hires, String> {
        let env = |k: &str| std::env::var_os(k);
        let cands = crate::locate::config_candidates(crate::locate::exe_dir().as_deref(), &env);
        crate::locate::resolve_hires(self.hires_dir.as_ref(), &env, &cands)
    }

    /// `--art-dir`, `$MC2_ART_DIR`, then `mc2.cfg` (`art =`); `None` keeps the original models.
    pub fn art_dir(&self) -> Result<Option<PathBuf>, String> {
        let env = |k: &str| std::env::var_os(k);
        let cands = crate::locate::config_candidates(crate::locate::exe_dir().as_deref(), &env);
        crate::locate::resolve_art(self.art_dir.as_deref(), &env, &cands)
    }

    /// `--voice-dir`, `$MC2_VOICE_DIR`, then `mc2.cfg`; `None` keeps the original pilot voices.
    pub fn voice_dir(&self) -> Result<Option<PathBuf>, String> {
        let env = |k: &str| std::env::var_os(k);
        let cands = crate::locate::config_candidates(crate::locate::exe_dir().as_deref(), &env);
        crate::locate::resolve_voices(self.voice_dir.as_deref(), &env, &cands)
    }

    pub fn game_config(&self) -> GameConfig {
        let d = GameConfig::default();
        GameConfig {
            mission: self.mission.clone(),
            sim_hz: self.sim_hz.unwrap_or(d.sim_hz),
            interpolate_camera: !self.no_interpolation,
            shadows: !self.no_shadows,
            cursor: !self.no_cursor,
            sim_seed: self.seed.unwrap_or(d.sim_seed),
            sim_rng: self.rng.unwrap_or(d.sim_rng),
            visual: d.visual,
            campaign_paint: d.campaign_paint,
            campaign_launch: d.campaign_launch,
            use_saved_resolution: false,
        }
    }

    pub fn script(&self) -> Result<InputScript, String> {
        match &self.input_script {
            None => Ok(InputScript::default()),
            Some(p) => {
                let text =
                    std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
                InputScript::parse(&text)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn headless_command_line() {
        let c = Cli::parse(args(
            "--data /d --mission mc2_01 --headless --frames 60 --screenshot o.png --size 3840x2160 --input-script s.txt",
        ))
        .unwrap();
        assert!(c.headless);
        assert_eq!(c.frames, Some(60));
        assert_eq!(c.size, Some((3840, 2160)));
        assert_eq!(c.mission.as_deref(), Some("mc2_01"));
        assert_eq!(c.data_dir().unwrap(), PathBuf::from("/d"));
        assert!(Cli::parse(args("--headless")).is_err());
        assert!(Cli::parse(args("--size 0x10")).is_err());
        assert!(Cli::parse(args("--bogus")).is_err());
        assert!(Cli::parse(args("--frames")).is_err());
        assert!(Cli::parse(args("--list-missions")).unwrap().list_missions);
        let h = Cli::parse(args("--hires-dir /pack --mission mc2_01")).unwrap();
        assert_eq!(h.hires_dir, Some(Hires::Dir("/pack".into())));
        let o = Cli::parse(args("--hires-dir off")).unwrap();
        assert_eq!(o.hires_dir, Some(Hires::Off));
        assert!(Cli::default().hires_dir.is_none());
        let v = Cli::parse(args("--voice-dir /vp --mission mc2_01")).unwrap();
        assert_eq!(v.voice_dir.as_deref(), Some(std::path::Path::new("/vp")));
        assert!(Cli::default().voice_dir.is_none());
        assert!(Cli::parse(args("--version")).unwrap().version);
    }
}
