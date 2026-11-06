// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! The headless front end: the same game core and render path as the window, drawing into an
//! offscreen target, with input from a script and PNG screenshots.  Never opens a window.

use crate::audio::AppAudio;
use crate::game::{self, GameConfig, GameCore};
use crate::live_sim::LiveSim;
use crate::script::{Action, InputScript};
use crate::sim::{NullSim, SimDriver};
use crate::state::GameState;
use mc2_gfx::config::RenderConfig;
use mc2_platform::input::InputEvent;
use mc2_platform::paths::UserPaths;
use mc2_render::game::GameData;
use mc2_render::gpu::{write_png, Gpu, OffscreenTarget};
use mc2_render::settings::GraphicsSettings;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// The one production headless-driver selection: the live `mc2_sim::Sim` adapter.  The P3 capability
/// checks and the progress runner follow the same path as `mc2 --headless`.  [`NullSim`] is only for
/// the viewer and for tests that ask for it ([`HeadlessRun::viewer`]).
pub type ProductionDriver = LiveSim;

/// Run the packaged self-test through the simulation without creating a renderer or GPU device.
/// Rendering acceptance belongs to GPU-capable hosts; release smoke checks must also run on
/// headless build and CI hosts.
#[derive(Debug)]
pub struct SimSelfTestReport {
    pub frames: u64,
    pub result: u32,
    pub unavailable: Vec<(String, u64)>,
}

pub fn run_sim_selftest(
    data: &Path,
    config: &GameConfig,
    frames: u64,
) -> Result<SimSelfTestReport, String> {
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(data).map_err(|e| e.to_string())?;
    let spec = crate::live_sim::sim_load_spec("mc2_01", config);
    let loaded = mc2_sim::mission::init::load_world(&mut vfs, &spec)
        .map_err(|e| format!("simulation load of mc2_01: {e}"))?;
    let mut sim = mc2_sim::Sim::new(loaded);
    let visibility = Rc::new(mc2_sim::visibility::AllVisible);
    let mut result = 0;
    let mut unavailable = std::collections::BTreeMap::<String, u64>::new();
    for frame in 0..frames {
        let output = sim
            .step(mc2_sim::FrameInput::fixed(
                config.sim_hz,
                frame,
                visibility.clone(),
                Vec::new(),
            ))
            .map_err(|e| format!("mc2_01 self-test frame {frame}: {e}"))?;
        result = output.result;
        for item in output.unavailable {
            *unavailable.entry(format!("{item:?}")).or_default() += 1;
        }
    }
    Ok(SimSelfTestReport {
        frames,
        result,
        unavailable: unavailable.into_iter().collect(),
    })
}

/// `mis_PLAYER_WIN_BIG` (`code/mission.h:41-46`).
const MISSION_WIN_BIG: u32 = 5;

/// Judges one hook-played mission: the sim reached the big win and every objective the player is told about succeeded.
/// Independent of any scripted player's tactics; it reads only world-state outcomes.
pub fn judge_objective_run(run: &mc2_sim::test_hooks::MissionRun) -> Result<String, String> {
    use mc2_sim::mission::objective::ObjectiveStatus;
    if run.result != MISSION_WIN_BIG {
        return Err(format!(
            "{} ended with result {} (expected {MISSION_WIN_BIG})",
            run.mission, run.result
        ));
    }
    if run.visible.is_empty() {
        return Err(format!("{} lists no visible objectives", run.mission));
    }
    for v in &run.visible {
        let status = run.runtime.get(v.index).copied();
        if status != Some(ObjectiveStatus::Successful) {
            return Err(format!(
                "{} objective {} ({}) is {status:?}, not Successful",
                run.mission, v.index, v.title
            ));
        }
    }
    Ok(format!(
        "{} objectives={} result={} frames={}",
        run.mission,
        run.visible.len(),
        run.result,
        run.frames
    ))
}

/// The package self-test's objective check: plays `mc2_01` to its win with the same production hooks the objective harness uses
/// (`mc2_sim::test_hooks::Harness`) and judges the outcome.  `Ok` carries the detail line, `Err` the failure reason.
pub fn run_objective_check(data: &Path) -> Result<String, String> {
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(data).map_err(|e| e.to_string())?;
    let run = mc2_sim::test_hooks::Harness::play_mission(&mut vfs, "mc2_01")?;
    judge_objective_run(&run)
}

/// Settings of a headless run.
#[derive(Clone, Debug)]
pub struct HeadlessOptions {
    pub data: PathBuf,
    pub mission: String,
    /// Frames to render.
    pub frames: u64,
    /// Frame size in pixels (logical = physical).
    pub size: (u32, u32),
    /// Written after the last frame.
    pub screenshot: Option<PathBuf>,
    pub script: InputScript,
    /// Optional per-frame camera/visibility trace, aligned with the oracle `--vis-trace` format.
    pub visibility_trace: Option<PathBuf>,
    /// Render frames per second of virtual time (the simulation runs at `game.sim_hz`).
    pub render_fps: u32,
    /// Keep the audio mixed offline in [`HeadlessRun::audio_samples`] (interleaved stereo f32).  The audio is
    /// mixed every frame either way, because the sound's channels free up only as the mixer is pulled.
    pub record_audio: bool,
    pub game: GameConfig,
    /// Upscaled texture pack. `None` leaves `$MC2_HIRES_DIR` (or off).
    pub hires_dir: Option<PathBuf>,
    /// `hires_dir` is the app's final answer (`None` = original art, ignoring
    /// `$MC2_HIRES_DIR`); `false` leaves the environment in charge when `hires_dir` is `None`.
    pub hires_pinned: bool,
    /// Original art pack. `None` leaves `$MC2_ART_DIR` (or off).
    pub art_dir: Option<PathBuf>,
    /// The user config directory (`options.cfg`) the session's UI host reads and writes.  `None`
    /// takes a fresh private directory ([`private_config_dir`]): a headless run never touches the
    /// player's real `UserPaths::resolve()` directory.
    pub user_config_dir: Option<PathBuf>,
}

/// Scratch space for private user roots: `$CARGO_TARGET_TMPDIR` (cargo sets it when running tests),
/// else `target/tmp` (cargo's own test scratch directory) derived from the running binary's
/// `target/<profile>/deps`, else the system temp directory (only for a binary run outside cargo).
fn scratch_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_TMPDIR") {
        return PathBuf::from(dir);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(deps) = exe.parent().filter(|d| d.ends_with("deps")) {
            if let Some(target) = deps.parent().and_then(|profile| profile.parent()) {
                return target.join("tmp");
            }
        }
    }
    std::env::temp_dir()
}

/// A fresh, empty config directory private to this process and call (under approved scratch, see
/// [`scratch_root`]).  Created on first write.  Its sibling `saves` directory holds campaign saves.
pub fn private_config_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let root = scratch_root().join(format!(
        "mc2-headless-user-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    UserPaths::under(&root).config
}

/// A switch is on when either the game config or the graphics settings (environment) asks for it.
pub fn merge_visual(
    a: mc2_render::visual::VisualOptions,
    b: mc2_render::visual::VisualOptions,
) -> mc2_render::visual::VisualOptions {
    mc2_render::visual::VisualOptions {
        enhanced_lighting: a.enhanced_lighting || b.enhanced_lighting,
        enhanced_water: a.enhanced_water || b.enhanced_water,
    }
}

impl HeadlessOptions {
    pub fn new(data: impl Into<PathBuf>, mission: &str) -> Self {
        Self {
            data: data.into(),
            mission: mission.to_string(),
            frames: 120,
            size: (1920, 1080),
            screenshot: None,
            script: InputScript::default(),
            visibility_trace: None,
            render_fps: 30,
            record_audio: false,
            game: GameConfig::default(),
            hires_dir: None,
            hires_pinned: false,
            art_dir: None,
            user_config_dir: None,
        }
    }
}

/// A headless session that can be stepped frame by frame (tests drive it directly).
pub struct HeadlessRun<D: SimDriver + 'static> {
    pub core: GameCore<D>,
    pub target: OffscreenTarget,
    pub script: InputScript,
    pub frame: u64,
    dt: Duration,
    scheduled: bool,
    pending_shots: Vec<(u64, PathBuf)>,
    /// Screenshots written so far.
    pub screenshots: Vec<PathBuf>,
    visibility_records: Vec<String>,
    visibility_note: String,
    /// Mix the audio each frame (the production sessions).
    mix_audio: bool,
    record_audio: bool,
    /// The audio of the run so far, when [`HeadlessOptions::record_audio`] is set.
    pub audio_samples: Vec<f32>,
    mix_scratch: Vec<f32>,
}

impl HeadlessRun<ProductionDriver> {
    /// The production headless session used by `mc2 --headless`: the live simulation with the
    /// production effects and battle sound sinks registered ([`crate::production::install_sinks`])
    /// and the audio mixed offline every frame.
    pub fn production(opts: &HeadlessOptions) -> Result<Self, String> {
        let mut run = Self::live_bare(opts)?;
        crate::production::install_sinks(&mut run.core)?;
        run.mix_audio = true;
        Ok(run)
    }

    /// [`Self::production`] (the name the live-simulation tests use).
    pub fn live(opts: &HeadlessOptions) -> Result<Self, String> {
        Self::production(opts)
    }

    /// The live simulation with its HUD audio mixed offline, but without the production battle
    /// sound and effects sinks.  HUD-audio tests use this to hear only the UI cues they inject;
    /// the production battle sink can keep ambient weapon and mover sounds alive indefinitely.
    pub fn live_hud_audio(opts: &HeadlessOptions) -> Result<Self, String> {
        let mut run = Self::live_bare(opts)?;
        // ABL `PlayVideo` uses the normal movie channel.  This route deliberately measures only
        // HUD cues, so presentation video cannot leave an unrelated audio source playing.
        run.core.movie_audio = false;
        run.mix_audio = true;
        Ok(run)
    }

    /// The live simulation alone: no effects, no battle sound.  For tests that register their own
    /// sinks or look only at the simulation.
    pub fn live_bare(opts: &HeadlessOptions) -> Result<Self, String> {
        Self::with_driver(
            opts,
            Box::new(|c| LiveSim::load(c.game, c.view, c.config, c.prefs.high_object_detail)),
        )
    }
}

impl HeadlessRun<NullSim> {
    /// The mission viewer (no simulation): for the viewer's tests.
    pub fn viewer(opts: &HeadlessOptions) -> Result<Self, String> {
        Self::with_driver(opts, Box::new(|_| Ok(NullSim::default())))
    }
}

/// What the real production headless construction can do for P3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProductionCapabilities {
    pub live_sim: bool,
    /// Physical player input reaches selection/order logic that emits commands
    /// to the production simulation route.
    pub orders_reach_sim_commands: bool,
    pub results_screen: bool,
}

/// Reports the two selections used by the production app path without
/// constructing the offscreen renderer.  This keeps an unavailable replay
/// diagnosable on a headless host with no GPU adapter.  The aliases above and
/// [`game::ProductionUi`] are the single production selections that P2/P3
/// replace when their adapters land.
pub fn production_capabilities() -> ProductionCapabilities {
    ProductionCapabilities {
        live_sim: ProductionDriver::IS_LIVE_SIM,
        orders_reach_sim_commands: game::production_orders_reach_sim_commands(),
        results_screen: game::production_ui_has_results_screen(),
    }
}

/// The render settings of a headless run: `env` with the pack the options name.
pub fn base_render_config(opts: &HeadlessOptions, mut env: RenderConfig) -> RenderConfig {
    crate::hires::pin_render_config(&mut env, opts.hires_dir.as_deref(), opts.hires_pinned);
    env
}

impl<D: SimDriver + 'static> HeadlessRun<D> {
    pub fn with_driver(
        opts: &HeadlessOptions,
        factory: crate::game::DriverFactory<D>,
    ) -> Result<Self, String> {
        let base = base_render_config(opts, RenderConfig::from_env());
        let mut base = base;
        if let Some(dir) = &opts.art_dir {
            base.art_dir = Some(dir.clone());
        }
        let settings = GraphicsSettings::from_render_config(&base)?;
        let cfg = settings.apply_to(&base);
        let gpu = Arc::new(Gpu::headless(&cfg).map_err(|e| e.to_string())?);
        let mut game = GameData::open(&opts.data).map_err(|e| e.to_string())?;
        // Sound with no device behind it: the offline mixer is only ever rendered by a test.
        let audio = match AppAudio::open(&opts.data, &mut game, AppAudio::offline_mixer(), false) {
            Ok(a) => Some(a),
            Err(e) => {
                log::warn!("no game sound: {e}");
                None
            }
        };
        let mut config = opts.game.clone();
        // no mission on the command line: the game starts at the main menu
        config.mission = Some(opts.mission.clone()).filter(|m| !m.is_empty());
        config.visual = merge_visual(config.visual, settings.visual());
        let (w, h) = (opts.size.0.max(1), opts.size.1.max(1));
        let target = OffscreenTarget::new(&gpu, w, h);
        let mut core = GameCore::new(gpu, game, config, (w, h), (w as f64, h as f64), factory)
            .map_err(|e| e.to_string())?;
        core.audio = audio;
        let config_dir = opts
            .user_config_dir
            .clone()
            .unwrap_or_else(private_config_dir);
        if let Some(host) = core.ui_host_mut() {
            host.set_private_user_directory(config_dir);
        }
        Ok(Self {
            core,
            target,
            script: opts.script.clone(),
            frame: 0,
            scheduled: false,
            pending_shots: Vec::new(),
            dt: Duration::from_nanos(1_000_000_000 / opts.render_fps.clamp(1, 1000) as u64),
            screenshots: Vec::new(),
            visibility_records: Vec::new(),
            visibility_note: String::new(),
            mix_audio: false,
            record_audio: opts.record_audio,
            audio_samples: Vec::new(),
            mix_scratch: Vec::new(),
        })
    }

    /// Delivers an event as the window would (window logical points).
    pub fn event(&mut self, e: InputEvent) {
        self.core.handle_event(e);
    }

    /// Resizes the frame (logical = physical, as on a 1x display).
    pub fn resize(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        let gpu = self.core.scene.gpu_arc().clone();
        self.target.resize(&gpu, w, h);
        self.core.handle_event(InputEvent::Resized {
            logical_w: w as f64,
            logical_h: h as f64,
        });
        self.core.resize((w, h));
    }

    /// Runs one frame: script actions, game update, render (one submit); writes `capture` and the
    /// script's screenshots of this frame.
    pub fn step(&mut self, capture: Option<&Path>) -> Result<(), String> {
        let mut shots: Vec<PathBuf> = capture.map(Path::to_path_buf).into_iter().collect();
        // Script frames are mission simulation frames. Boot and mission loading consume render
        // frames but the oracle starts applying frame 0 at its first mission tick.  Input events
        // are handed to the core, which delivers each exactly once before its simulation frame.
        // Resizes are delivered by the core at their simulation frame, and screenshots stop the
        // core right after their frame's tick, so several ticks per render frame skip neither.
        if self.core.mission.is_some() && !self.scheduled {
            self.scheduled = true;
            for (f, a) in &self.script.actions {
                match a {
                    Action::Event(e) => {
                        self.core
                            .scheduled_events
                            .entry(*f)
                            .or_default()
                            .push(e.clone());
                    }
                    Action::Resize(w, h) => {
                        self.core
                            .scheduled_resizes
                            .entry(*f)
                            .or_default()
                            .push((*w, *h));
                    }
                    Action::Screenshot(p) => {
                        self.core.capture_frames.insert(*f);
                        self.pending_shots.push((*f, PathBuf::from(p)));
                    }
                }
            }
        }
        self.core.frame(self.dt);
        if self.mix_audio {
            if let Some(audio) = self.core.audio.as_mut() {
                // the frame's sounds, started by the sinks in this frame, are mixed in this frame
                self.mix_scratch.clear();
                audio.mix_frame(self.dt.as_secs_f32(), &mut self.mix_scratch);
                if self.record_audio {
                    self.audio_samples.extend_from_slice(&self.mix_scratch);
                }
            }
        }
        if let Some((w, h)) = self.core.resize_request.take() {
            let gpu = self.core.scene.gpu_arc().clone();
            self.target.resize(&gpu, w, h);
        }
        for f in std::mem::take(&mut self.core.captured) {
            let mut i = 0;
            while i < self.pending_shots.len() {
                if self.pending_shots[i].0 == f {
                    shots.push(self.pending_shots.remove(i).1);
                } else {
                    i += 1;
                }
            }
        }
        if let Some(mission) = &self.core.mission {
            self.visibility_records.push(describe_visibility(
                mission.turn,
                &mission.rig,
                &mission.visibility,
            ));
            self.visibility_note = mission.driver.visibility_note();
        }
        if let Some(e) = self.core.last_error.take() {
            return Err(e);
        }
        let gpu = self.core.scene.gpu_arc().clone();
        let target = self.target.target();
        let mut enc = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.core
            .render(&mut enc, &target)
            .map_err(|e| e.to_string())?;
        let readback = if shots.is_empty() {
            None
        } else {
            Some(
                target
                    .encode_readback(&gpu, &mut enc)
                    .map_err(|e| e.to_string())?,
            )
        };
        gpu.queue.submit(Some(enc.finish()));
        if let Some(rb) = readback {
            let rgba = rb.finish(&gpu).map_err(|e| e.to_string())?;
            let (w, h) = target.size();
            for p in shots {
                if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                write_png(&p, &rgba, w, h).map_err(|e| format!("{}: {e}", p.display()))?;
                self.screenshots.push(p);
            }
        } else {
            // keep the device from queueing unbounded work
            let _ = gpu.device.poll(wgpu::PollType::Poll);
        }
        self.frame += 1;
        Ok(())
    }
}

/// What a headless run reports.
#[derive(Clone, Debug)]
pub struct HeadlessReport {
    pub frames: u64,
    pub sim_frames: u64,
    /// `scenarioResult` when the production mission reached [`GameState::Results`].
    /// Viewer runs leave this `None`; P3 acceptance requires `Some(mis_PLAYER_WIN_BIG)`.
    pub result: Option<u32>,
    /// `true` only after the real Results UI was composed into the offscreen
    /// production frame.  A state transition alone leaves this false.
    pub results_screen_drawn: bool,
    pub state: String,
    pub camera: Option<String>,
    pub active_vertices: usize,
    pub active_blocks: usize,
    pub screenshots: Vec<PathBuf>,
    pub adapter: String,
    /// Simulation behaviour that was unavailable (unported) during the run, with counts.
    pub sim_unavailable: Vec<String>,
}

/// `mc2 --headless`: runs `frames` frames of the mission with the live simulation.  A mission whose
/// mechs or vehicles lack a paint source fails on its first frame, as unavailable.
pub fn run(opts: &HeadlessOptions) -> Result<HeadlessReport, String> {
    run_with_progress(opts, |_| {})
}

/// Runs the production path and reports each completed render frame.  P3 uses
/// this to turn its fixed player-input route into actual execution progress;
/// ordinary callers use [`run`] without an observer.
pub fn run_with_progress(
    opts: &HeadlessOptions,
    progress: impl FnMut(u64),
) -> Result<HeadlessReport, String> {
    run_driven(opts, |_| {}, false, progress)
}

/// [`run_with_progress`] with `before_step` called with the session before every frame (a driver
/// that feeds input from what it sees), and, when `stop_at_results`, ending the run on the first
/// frame that composed the Results screen.
pub fn run_driven(
    opts: &HeadlessOptions,
    before_step: impl FnMut(&mut HeadlessRun<ProductionDriver>),
    stop_at_results: bool,
    progress: impl FnMut(u64),
) -> Result<HeadlessReport, String> {
    run_driven_keep(opts, before_step, stop_at_results, progress).map(|(_, report)| report)
}

/// [`run_driven`], also handing back the finished session, so the caller can read what the
/// mission left behind (the campaign driver reads the mission's `Outcome` from it).
pub fn run_driven_keep(
    opts: &HeadlessOptions,
    mut before_step: impl FnMut(&mut HeadlessRun<ProductionDriver>),
    stop_at_results: bool,
    mut progress: impl FnMut(u64),
) -> Result<(HeadlessRun<ProductionDriver>, HeadlessReport), String> {
    let mut run = HeadlessRun::live(opts)?;
    for i in 0..opts.frames {
        let last = i + 1 == opts.frames;
        let cap = if last {
            opts.screenshot.as_deref()
        } else {
            None
        };
        before_step(&mut run);
        run.step(cap)?;
        progress(run.frame);
        if stop_at_results && run.core.results_screen_drawn() {
            break;
        }
    }
    // The result is the last mission's, whatever the game did after it: a mission started from the
    // command line quits the game when it is over, so the run may end in `exit`.
    let result = run.core.last_result;
    match run.core.state() {
        GameState::InMission | GameState::Results(_) => {}
        GameState::Exit if result.is_some() => {}
        state => {
            return Err(format!(
                "the run ended in state {} instead of in the mission or results",
                state.name()
            ));
        }
    }
    let report = {
        let rt = run.core.mission.as_ref();
        if let Some(path) = &opts.visibility_trace {
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let mut trace = visibility_trace_header(&opts.mission, opts.size, &run.visibility_note);
            trace.push_str(&run.visibility_records.join(""));
            std::fs::write(path, trace).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        HeadlessReport {
            frames: run.frame,
            sim_frames: rt.map(|m| m.clock.frame()).unwrap_or(0),
            result,
            results_screen_drawn: run.core.results_screen_drawn(),
            state: run.core.state().name().into(),
            camera: rt.map(|m| describe_camera(&m.rig)),
            active_vertices: rt.map(|m| m.visibility.active_vertex_count()).unwrap_or(0),
            active_blocks: rt.map(|m| m.visibility.active_block_count()).unwrap_or(0),
            screenshots: run.screenshots.clone(),
            sim_unavailable: rt
                .map(|m| {
                    let mut v = m.driver.unavailable_summary();
                    v.extend(m.hud.iter().flat_map(|h| h.unavailable_summary()));
                    v
                })
                .unwrap_or_default(),
            adapter: format!(
                "{} ({}), msaa x{}",
                run.core.gpu().adapter_name,
                run.core.gpu().backend,
                run.core.gpu().samples
            ),
        }
    };
    Ok((run, report))
}

fn visibility_trace_header(mission: &str, size: (u32, u32), note: &str) -> String {
    format!(
        "# mission={mission} viewport={}x{}\n# {note}\n",
        size.0, size.1
    )
}

fn describe_visibility(
    frame: i64,
    rig: &crate::camera::CameraRig,
    visibility: &crate::visibility::CameraVisibility,
) -> String {
    use std::fmt::Write as _;
    let mut s = format!("T {frame}\n{}\nI", describe_camera_record(frame, rig));
    for part in &visibility.movers_in_view {
        let _ = write!(s, " {part}");
    }
    s.push_str("\nS");
    for handle in &visibility.structures_in_view {
        let _ = write!(s, " {handle}");
    }
    s.push_str("\nLM");
    for (part, lod) in &visibility.mover_lods {
        let _ = write!(s, " {part}:{lod}");
    }
    s.push_str("\nLS");
    for (handle, lod) in &visibility.structure_lods {
        let _ = write!(s, " {handle}:{lod}");
    }
    s.push_str("\nB");
    for (i, active) in visibility.blocks.iter().enumerate() {
        if *active {
            let _ = write!(s, " {i}");
        }
    }
    s.push_str("\nV");
    let mut i = 0;
    while i < visibility.vertices.len() {
        if !visibility.vertices[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i + 1 < visibility.vertices.len() && visibility.vertices[i + 1] {
            i += 1;
        }
        let _ = write!(s, " {start}-{i}");
        i += 1;
    }
    s.push('\n');
    s
}

/// The camera record uses the oracle `--vis-camera` schema and frame convention.
fn describe_camera_record(frame: i64, rig: &crate::camera::CameraRig) -> String {
    format!(
        "C {frame} {:.9} {:.9} {:.9} {:.9} {:.9} {:.9} {:.9}",
        rig.position.x,
        rig.position.y,
        rig.position.z,
        rig.projection_angle,
        rig.rotation,
        rig.altitude,
        rig.los_pitch(),
    )
}

/// One line with the camera state.
pub fn describe_camera(r: &crate::camera::CameraRig) -> String {
    format!(
        "position ({:.1}, {:.1}, {:.1}) tilt {:.3} view-pitch {:.3} rotation {:.3} altitude {:.1} scale {:.4}",
        r.position.x,
        r.position.y,
        r.position.z,
        r.projection_angle,
        r.view_angle,
        r.rotation,
        r.altitude,
        r.scale_factor
    )
}

#[cfg(test)]
mod trace_tests {
    use super::{describe_camera_record, visibility_trace_header, HeadlessOptions, HeadlessRun};
    use crate::camera::{CameraData, CameraRig, MissionCameraStart};
    use mc2_render::camera::CameraSettings;

    #[test]
    fn app_camera_record_matches_oracle_schema_and_derived_los_pitch() {
        let rig = CameraRig::new(
            CameraSettings::default(),
            &MissionCameraStart::default(),
            CameraData::default(),
            [(1200.0, 35.0); 4],
            (800.0, 600.0),
        );
        let line = describe_camera_record(0, &rig);
        let fields: Vec<_> = line.split_whitespace().collect();
        assert_eq!(&fields[..2], &["C", "0"]);
        assert_eq!(fields.len(), 9);
        let expected = rig.los_pitch();
        assert_eq!(fields[8].parse::<f32>().unwrap(), expected);
    }

    #[test]
    fn null_sim_visibility_trace_declares_object_sets_unavailable() {
        use crate::sim::{NullSim, SimDriver};
        let note = NullSim::default().visibility_note();
        let header = visibility_trace_header("mc2_01", (800, 600), &note);
        assert!(header.contains("object-visibility=unavailable driver=NullSim"));
        assert!(header.contains("I/S/LM/LS are incomplete"));
    }

    #[test]
    fn unbound_live_object_emits_an_incomplete_visibility_trace() {
        let Some(data) = std::env::var_os("MC2_DATA") else {
            eprintln!("MC2_DATA not set; skipping unbound live-object trace test");
            return;
        };
        let mut opts = HeadlessOptions::new(std::path::PathBuf::from(data), "mc2_01");
        opts.size = (800, 600);
        opts.game.cursor = false;
        let mut run = match HeadlessRun::live_bare(&opts) {
            Ok(run) => run,
            Err(error)
                if error.contains("no suitable GPU adapter")
                    || (error.contains("adapter")
                        && std::env::var_os("MC2_SKIP_GPU_TESTS").as_deref()
                            == Some(std::ffi::OsStr::new("1"))) =>
            {
                eprintln!("no GPU adapter ({error}); skipping live visibility trace test");
                return;
            }
            Err(error) => panic!("create live headless run: {error}"),
        };
        while run.core.mission.is_none() {
            run.step(None).expect("load mission");
        }
        let driver = &mut run.core.mission.as_mut().expect("mission").driver;
        let handle = *driver
            .sim()
            .world
            .objects
            .movers
            .first()
            .expect("mc2_01 has a mover");
        driver.sim_mut().world.appearance.by_handle[handle.0 as usize] = None;
        run.step(None).expect("step with an unbound appearance");
        let header = visibility_trace_header("mc2_01", (800, 600), &run.visibility_note);
        assert!(
            header.contains("object-visibility=incomplete driver=LiveSim"),
            "{header}"
        );
        assert!(!header.contains("object-visibility=production"), "{header}");
    }
}
