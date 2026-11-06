//! `mc2`: MechCommander 2.

use mc2_app::cli::{Cli, USAGE};
use mc2_app::headless::{self, HeadlessOptions};
use mc2_app::mission_view::list_missions;

fn main() {
    if let Err(e) = run() {
        eprintln!("mc2: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let cli = Cli::parse(std::env::args().skip(1)).map_err(|e| format!("{e}\n\n{USAGE}"))?;
    if cli.help {
        print!("{USAGE}");
        return Ok(());
    }
    if cli.version {
        println!("{}", mc2_app::locate::version_string());
        return Ok(());
    }
    let data = cli.data_dir()?;
    if !data.is_dir() {
        return Err(format!(
            "game data folder {} does not exist; point mc2 at your game data with --data, MC2_DATA or mc2.cfg",
            data.display()
        ));
    }
    // the UI text comes from the player's own resource library; without it nothing can be shown
    mc2_ui::strings::StringTable::init_from_data_root(&data)?;
    let hires_setting = cli.hires_setting()?;
    let cache = mc2_app::locate::cache_dir(&|k| std::env::var_os(k));
    // Only the windowed game builds the pack; the self-test and headless tools use a finished
    // one if it exists, so they never stall on a first-run build.
    let hires_choice = |build| {
        mc2_app::hires::prepare(&hires_setting, &data, cache.as_deref(), build, &mut |m| {
            eprintln!("{m}")
        })
    };
    if let Some(dir) = cli.art_dir()? {
        // read by `RenderConfig::from_env` in the headless and windowed start-up alike
        std::env::set_var("MC2_ART_DIR", dir);
    }
    mc2_app::audio::set_voice_dir(cli.voice_dir()?);
    if cli.selftest_play {
        let hires_dir = hires_choice(mc2_app::hires::Build::Never);
        println!("selftest data: {}", data.display());
        println!(
            "selftest hires: {}",
            match (&hires_setting, &hires_dir.dir) {
                (_, None) => "off".to_owned(),
                (mc2_app::locate::Hires::Auto, Some(p)) =>
                    format!("{} (upscaled pack)", p.display()),
                (_, Some(p)) => p.display().to_string(),
            }
        );
        let mut config = cli.game_config();
        config.mission = Some("mc2_01".into());
        let report = headless::run_sim_selftest(&data, &config, 9_000)?;
        println!(
            "selftest mission=mc2_01 frames={} simulation_frames={} result={} renderer=not-required",
            report.frames, report.frames, report.result
        );
        if report.unavailable.is_empty() {
            println!("simulation unavailable: none");
        } else {
            for (what, frames) in report.unavailable {
                println!("simulation unavailable: {what} reached in {frames} frames");
            }
        }
        return match headless::run_objective_check(&data) {
            Ok(detail) => {
                println!("objective check: pass ({detail})");
                Ok(())
            }
            Err(why) => {
                println!("objective check: fail {why}");
                Err(format!("objective check failed: {why}"))
            }
        };
    }
    if cli.list_missions {
        let game = mc2_render::game::GameData::open(&data).map_err(|e| e.to_string())?;
        for m in list_missions(&game) {
            println!("{m}");
        }
        return Ok(());
    }
    if cli.headless {
        let mission = cli.mission.clone().unwrap_or_default();
        let mut opts = HeadlessOptions::new(data.clone(), &mission);
        opts.frames = cli.frames.unwrap_or(120);
        opts.size = cli.size.unwrap_or((1920, 1080));
        opts.screenshot = cli.screenshot.clone();
        opts.script = cli.script()?;
        opts.visibility_trace = cli.visibility_trace.clone();
        opts.render_fps = cli.fps.unwrap_or(30);
        opts.game = cli.game_config();
        opts.hires_dir = hires_choice(mc2_app::hires::Build::Never).dir;
        opts.hires_pinned = true;
        let t0 = std::time::Instant::now();
        let r = headless::run(&opts)?;
        println!("adapter {}", r.adapter);
        println!(
            "{} frames ({} simulation frames) of {} in {:.2} s, state {}",
            r.frames,
            r.sim_frames,
            mission,
            t0.elapsed().as_secs_f64(),
            r.state
        );
        if let Some(c) = &r.camera {
            println!("camera {c}");
        }
        println!(
            "visibility: {} active vertices, {} active blocks",
            r.active_vertices, r.active_blocks
        );
        for u in &r.sim_unavailable {
            println!("simulation unavailable: {u}");
        }
        for s in &r.screenshots {
            println!("wrote {}", s.display());
        }
        return Ok(());
    }
    let budget =
        mc2_app::hires::budget_from_env(std::env::var("MC2_HIRES_BUDGET_SECS").ok().as_deref());
    let hires_dir = hires_choice(mc2_app::hires::Build::Allowed(budget)).dir;
    let mut config = cli.game_config();
    config.use_saved_resolution = cli.size.is_none();
    mc2_app::windowed::run(
        data,
        config,
        cli.fullscreen,
        cli.size.unwrap_or((1280, 720)),
        hires_dir,
    )
}
