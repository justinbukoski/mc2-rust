// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The windowed front end: `mc2_platform::window::App` on a wgpu surface.  It shares everything
//! with the headless front end (`GameCore`); only the event source (winit) and the frame
//! destination (the surface texture) differ.

use crate::audio::AppAudio;
use crate::game::{GameConfig, GameCore};
use crate::live_sim::LiveSim;
use crate::state::GameState;
use mc2_gfx::config::RenderConfig;
use mc2_platform::audio::device::AudioOutput;
use mc2_platform::audio::APP_MIXER_CHANNELS;
use mc2_platform::clock::FrameTimer;
use mc2_platform::input::InputEvent;
use mc2_platform::window::{App, Context, WindowConfig};
use mc2_platform::winit::dpi::PhysicalSize;
use mc2_render::game::GameData;
use mc2_render::gpu::{FrameAttachments, Gpu, GpuSurface};
use mc2_render::present::ScaledPresenter;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// The game in a window.
pub struct WindowedApp {
    data: PathBuf,
    config: GameConfig,
    hires_dir: Option<PathBuf>,
    inner: Option<Running>,
    timer: FrameTimer,
    /// Fatal error during start-up (reported after the loop ends).
    error: Rc<RefCell<Option<String>>>,
}

struct Running {
    core: GameCore<LiveSim>,
    surface: GpuSurface,
    attachments: FrameAttachments,
    /// The adapter this run actually got (Hardware Rasterizer), and what a changed choice may still do.
    adapter: AdapterState,
    /// The chosen-resolution frame of a scaled fullscreen.
    presenter: ScaledPresenter,
    /// A notice for the player that the first frame reports (the asked-for adapter was not available).
    notice: Option<String>,
    /// The output device the game sound plays through; kept alive with the game.
    _sound: Option<AudioOutput>,
}

impl WindowedApp {
    /// `error` receives a fatal start-up error.
    pub fn new(
        data: PathBuf,
        config: GameConfig,
        hires_dir: Option<PathBuf>,
        error: Rc<RefCell<Option<String>>>,
    ) -> Self {
        Self {
            data,
            config,
            hires_dir,
            inner: None,
            // a frame that took longer than this (window drag, breakpoint) counts as this long
            timer: FrameTimer::new(Duration::from_millis(250)),
            error,
        }
    }

    fn start(&mut self, ctx: &mut Context<'_>) -> Result<Running, String> {
        let size = ctx.physical_size();
        let (w, h) = (size.width.max(1), size.height.max(1));
        let base = base_render_config(self.hires_dir.as_deref(), RenderConfig::from_env());
        let settings = mc2_render::settings::GraphicsSettings::from_render_config(&base)?;
        let mut cfg = settings.apply_to(&base);
        // Hardware Rasterizer (`renderer` in options.cfg): anything but 0 asks for the software adapter at start-up
        let user_config = mc2_platform::paths::UserPaths::resolve().map(|p| p.config);
        apply_saved_adapter(&mut cfg, user_config.as_deref(), &self.data);
        let (gpu, surface) =
            Gpu::for_surface(&cfg, ctx.window().clone(), w, h).map_err(|e| e.to_string())?;
        // always printed: a run of the packaged game with the saved choice shows which adapter it used
        println!("{}", gpu.startup_line());
        log::info!(
            "adapter {} ({}), msaa x{}, surface {:?}",
            gpu.adapter_name,
            gpu.backend,
            gpu.samples,
            surface.format()
        );
        let gpu = Arc::new(gpu);
        let attachments = FrameAttachments::new(&gpu, w, h);
        let mut game = GameData::open(&self.data).map_err(|e| e.to_string())?;
        // Without an output device (or without sound data) the game runs silent.
        let (sound, audio) = match AudioOutput::open_with_channels(APP_MIXER_CHANNELS) {
            Ok(out) => match AppAudio::open(&self.data, &mut game, out.mixer(), true) {
                Ok(a) => (Some(out), Some(a)),
                Err(e) => {
                    log::warn!("no game sound: {e}");
                    (Some(out), None)
                }
            },
            Err(e) => {
                log::warn!("no audio device: {e}");
                (None, None)
            }
        };
        let scale = ctx.scale_factor();
        let logical = (w as f64 / scale, h as f64 / scale);
        let mut config = self.config.clone();
        config.visual = crate::headless::merge_visual(config.visual, settings.visual());
        let mut core = GameCore::new(
            gpu,
            game,
            config,
            (w, h),
            logical,
            Box::new(|c| LiveSim::load(c.game, c.view, c.config, c.prefs.high_object_detail)),
        )
        .map_err(|e| e.to_string())?;
        core.audio = audio;
        // the production effects and battle sound sinks, as the headless front end registers them
        crate::production::install_sinks(&mut core)?;
        let (adapter, notice) = AdapterState::new(
            cfg.software_adapter,
            core.scene.gpu_arc().is_software_adapter(),
        );
        Ok(Running {
            core,
            surface,
            attachments,
            adapter,
            presenter: ScaledPresenter::new(),
            notice,
            _sound: sound,
        })
    }
}

impl App for WindowedApp {
    fn init(&mut self, ctx: &mut Context<'_>) {
        match self.start(ctx) {
            Ok(r) => {
                ctx.set_cursor_visible(!self.config.cursor);
                self.inner = Some(r);
            }
            Err(e) => {
                *self.error.borrow_mut() = Some(e);
                ctx.request_exit();
            }
        }
    }

    fn resized(&mut self, _ctx: &mut Context<'_>, physical: PhysicalSize<u32>, _scale: f64) {
        if let Some(r) = &mut self.inner {
            let gpu = r.core.scene.gpu_arc().clone();
            if r.surface.resize(&gpu, physical.width, physical.height) {
                r.attachments.resize(&gpu, physical.width, physical.height);
                r.core.resize((physical.width, physical.height));
            }
        }
    }

    fn input_event(&mut self, _ctx: &mut Context<'_>, event: &InputEvent) {
        if let Some(r) = &mut self.inner {
            r.core.handle_event(event.clone());
        }
    }

    fn split_frames(&self) -> bool {
        // Async Mouse: the render waits one redraw for the pointer events that arrive after the update
        self.inner
            .as_ref()
            .is_some_and(|r| r.core.prefs.async_mouse)
    }

    fn frame(&mut self, ctx: &mut Context<'_>) {
        if self.update_half(ctx) {
            self.render_half();
        }
    }

    fn update(&mut self, ctx: &mut Context<'_>) {
        self.update_half(ctx);
    }

    fn render(&mut self, _ctx: &mut Context<'_>) {
        self.render_half();
    }
}

impl WindowedApp {
    /// The game's update and the window requests it makes; `false` when the frame must not be drawn.
    fn update_half(&mut self, ctx: &mut Context<'_>) -> bool {
        let dt = self.timer.tick();
        let Some(r) = &mut self.inner else {
            return false;
        };
        if let Some(n) = r.notice.take() {
            r.core.ui.report(&n);
        }
        r.core.frame(dt);
        if let Some((w, h)) = r.core.take_window_request() {
            // the Resolution option: the game screen was resized, the window follows (fullscreen keeps its display mode and stretches the new game screen to it)
            ctx.set_game_resolution(w, h);
            if ctx.is_fullscreen() {
                // fullscreen switches the display mode to the size, as the C++ did; the surface follows the resize event
                let exact = ctx.set_fullscreen_resolution(w, h);
                if !exact {
                    log::warn!("no {w} x {h} display mode: borderless fullscreen, the game screen is scaled");
                }
                // without the mode, the scene renders at the chosen size and the surface stretches it
                r.core.set_scaled_fullscreen(!exact);
            } else {
                r.core.set_scaled_fullscreen(false);
                ctx.fit_window_to_game();
            }
            log::info!("resolution {w} x {h}");
        }
        if let Some(e) = r.core.last_error.take() {
            log::error!("{e}");
        }
        // Hardware Rasterizer: the adapter is fixed when the device is created, so an accepted change starts the
        // game again with the saved choice, wherever it was accepted (the pause menu included)
        if r.core.settings_applied() {
            match r.adapter.apply(r.core.prefs.renderer, relaunch) {
                AdapterOutcome::Unchanged => {}
                AdapterOutcome::Relaunched => {
                    log::info!("restarting with the new Hardware Rasterizer choice");
                    ctx.request_exit();
                    return false;
                }
                AdapterOutcome::Failed(e) => {
                    log::error!("{e}");
                    r.core.ui.report(&e);
                }
            }
        }
        if matches!(r.core.state(), GameState::Exit) {
            ctx.request_exit();
            return false;
        }
        let gpu = r.core.scene.gpu_arc().clone();
        if r.surface.is_lost() {
            if let Err(e) = r.surface.recreate(&gpu, ctx.window().clone()) {
                log::error!("{e}");
                return false;
            }
        }
        true
    }

    /// Draws the frame (scaled up from the chosen resolution in a scaled fullscreen) and presents it.
    fn render_half(&mut self) {
        let Some(r) = &mut self.inner else { return };
        let gpu = r.core.scene.gpu_arc().clone();
        let Some(texture) = r.surface.acquire(&gpu) else {
            return;
        };
        let target = r.surface.target(&texture, &r.attachments);
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        if let Err(e) = r.core.render_presented(&mut enc, &target, &mut r.presenter) {
            log::error!("render: {e}");
        }
        gpu.queue.submit(Some(enc.finish()));
        drop(target);
        gpu.queue.present(texture);
    }
}

/// The render settings of the windowed game: `env` (from `RenderConfig::from_env`) with the
/// pack the app resolved.  The app's choice is final, so `--hires-dir off` / `hires = off` beats
/// `$MC2_HIRES_DIR`.
pub fn base_render_config(
    hires_dir: Option<&std::path::Path>,
    mut env: RenderConfig,
) -> RenderConfig {
    crate::hires::pin_render_config(&mut env, hires_dir, true);
    env
}

/// Whether the Hardware Rasterizer choice (`renderer`) names another adapter than the running one.
pub fn restart_needed(running_software: bool, renderer: i32) -> bool {
    running_software != (renderer != 0)
}

/// What an accepted Hardware Rasterizer choice did ([`AdapterState::apply`]).
#[derive(Debug, PartialEq, Eq)]
pub enum AdapterOutcome {
    /// The running adapter already is the chosen one, or the choice cannot be met.
    Unchanged,
    /// The game started again with the new choice; this process should exit.
    Relaunched,
    /// The game could not start again; the message is for the player.
    Failed(String),
}

/// The adapter the device was actually created on, against the Hardware Rasterizer choice.
#[derive(Debug)]
pub struct AdapterState {
    /// The running adapter is a software one (what the device reports, not what was asked for).
    pub software: bool,
    /// A choice that a relaunch cannot give (no such adapter on this machine, or the relaunch failed): not tried again.
    blocked: Option<bool>,
}

impl AdapterState {
    /// `requested_software` is what the start-up asked for, `actual_software` what the device is.  When they differ
    /// the choice is unavailable here: the message says so, and the game does not restart for it again and again.
    pub fn new(requested_software: bool, actual_software: bool) -> (Self, Option<String>) {
        let kind = |s: bool| if s { "software" } else { "hardware" };
        let unavailable = requested_software != actual_software;
        let notice = unavailable.then(|| {
            format!(
                "Hardware Rasterizer: this machine has no {} adapter; the game runs on the {} one",
                kind(requested_software),
                kind(actual_software)
            )
        });
        (
            Self {
                software: actual_software,
                blocked: unavailable.then_some(requested_software),
            },
            notice,
        )
    }

    /// The saved choice is `renderer` (`!= 0` software): when it names another adapter than the running one,
    /// `relaunch` starts the game again with it.
    pub fn apply(
        &mut self,
        renderer: i32,
        relaunch: impl FnOnce() -> Result<(), String>,
    ) -> AdapterOutcome {
        let want = renderer != 0;
        if !restart_needed(self.software, renderer) || self.blocked == Some(want) {
            return AdapterOutcome::Unchanged;
        }
        match relaunch() {
            Ok(()) => AdapterOutcome::Relaunched,
            Err(e) => {
                self.blocked = Some(want);
                AdapterOutcome::Failed(format!(
                    "Hardware Rasterizer: the game could not restart with the new choice ({e}); it is used the next time the game starts"
                ))
            }
        }
    }
}

/// Starts this program again with the arguments it was started with.
fn relaunch() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The start-up adapter selection: asks for the software adapter when the saved choice (see `saved_renderer`) says so.
pub fn apply_saved_adapter(
    cfg: &mut RenderConfig,
    config_dir: Option<&std::path::Path>,
    data: &std::path::Path,
) {
    cfg.software_adapter = saved_renderer(config_dir, data) != 0;
}

/// The saved Hardware Rasterizer choice (`renderer`): the user's `options.cfg` in `config_dir`, else the game's
/// own; 0 (hardware) when neither exists.
pub fn saved_renderer(config_dir: Option<&std::path::Path>, data: &std::path::Path) -> i32 {
    let user = config_dir
        .and_then(|d| std::fs::read(mc2_platform::paths::options_read_path(d)).ok())
        .and_then(|b| mc2_formats::ini::FitIni::from_bytes(b).ok())
        .map(|mut f| mc2_ui::screens::Prefs::from_fit(&mut f));
    let prefs = user.or_else(|| {
        let mut g = GameData::open(data).ok()?;
        let mut f = g.fit("options.cfg").ok()?;
        Some(mc2_ui::screens::Prefs::from_fit(&mut f))
    });
    prefs.map_or(0, |p| p.renderer)
}

/// Opens the window and runs the game until it exits (main thread only).  Only for players:
/// development and tests use the headless front end.
pub fn run(
    data: PathBuf,
    config: GameConfig,
    fullscreen: bool,
    size: (u32, u32),
    hires_dir: Option<PathBuf>,
) -> Result<(), String> {
    let window = WindowConfig {
        title: mc2_platform::PRODUCT_NAME.into(),
        game_width: size.0,
        game_height: size.1,
        fullscreen,
        resizable: true,
        fit: mc2_platform::viewport::Fit::Stretch,
        hide_os_cursor: config.cursor,
    };
    let error = Rc::new(RefCell::new(None));
    let app = WindowedApp::new(data, config, hires_dir, error.clone());
    let result = mc2_platform::window::run(window, app).map_err(|e| e.to_string());
    if let Some(e) = error.borrow_mut().take() {
        return Err(e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{restart_needed, saved_renderer, AdapterOutcome, AdapterState};

    #[test]
    fn the_adapter_state_follows_the_actual_adapter_and_never_loops() {
        // hardware running, software chosen: the relaunch happens once
        let (mut a, notice) = AdapterState::new(false, false);
        assert!(notice.is_none());
        assert_eq!(
            a.apply(0, || panic!("unchanged")),
            AdapterOutcome::Unchanged
        );
        assert_eq!(a.apply(3, || Ok(())), AdapterOutcome::Relaunched);
        // a failed relaunch is reported, not retried for the same choice, and retried for another
        let (mut a, _) = AdapterState::new(false, false);
        assert!(
            matches!(a.apply(3, || Err("no exe".into())), AdapterOutcome::Failed(m) if m.contains("no exe"))
        );
        assert_eq!(
            a.apply(3, || panic!("not retried")),
            AdapterOutcome::Unchanged
        );
        assert_eq!(
            a.apply(0, || panic!("unchanged")),
            AdapterOutcome::Unchanged
        );
        // software was asked for at start-up but the machine only had the GPU: the notice says so and the
        // choice does not relaunch forever
        let (mut a, notice) = AdapterState::new(true, false);
        assert!(notice.unwrap().contains("no software adapter"));
        assert!(
            !a.software,
            "the actual adapter is recorded, not the request"
        );
        assert_eq!(
            a.apply(3, || panic!("would loop")),
            AdapterOutcome::Unchanged
        );
        // and hardware is what runs, so choosing it changes nothing
        assert_eq!(
            a.apply(0, || panic!("unchanged")),
            AdapterOutcome::Unchanged
        );
    }

    #[test]
    fn a_changed_rasterizer_choice_restarts_with_the_new_adapter() {
        // hardware running, software chosen (and the other way round); an unchanged choice stays
        assert!(restart_needed(false, 3));
        assert!(restart_needed(true, 0));
        assert!(!restart_needed(false, 0));
        assert!(!restart_needed(true, 3));
    }

    #[test]
    fn the_saved_rasterizer_choice_is_read_from_the_user_config() {
        let dir = std::env::temp_dir().join(format!("mc2-renderer-pref-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prefs = mc2_ui::screens::Prefs {
            renderer: 3,
            ..Default::default()
        };
        std::fs::write(dir.join("options.cfg"), prefs.to_fit()).unwrap();
        let nowhere = std::path::Path::new("/nonexistent-mc2-data");
        assert_eq!(saved_renderer(Some(&dir), nowhere), 3);
        assert_eq!(saved_renderer(None, nowhere), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
