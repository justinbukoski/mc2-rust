//! A headless session never reads or writes the player's real user directory.  Lives in its own
//! test binary because it points `HOME`/`XDG_*` at a temp dir for the whole process.

use mc2_app::headless::{HeadlessOptions, HeadlessRun};
use mc2_app::sim::NullSim;
use mc2_app::ui_host::UiHost;
use std::path::PathBuf;

#[test]
fn headless_session_ignores_the_players_real_options_cfg() {
    let Some(data) = std::env::var_os("MC2_DATA").map(PathBuf::from) else {
        eprintln!("MC2_DATA not set; skipping");
        return;
    };
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("poisoned-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    // Single test in this process: nothing else reads the environment concurrently.
    std::env::remove_var("MC2_USER_DIR");
    // Exercise the exe-derived fallback, as when run without cargo's runtime env.
    std::env::remove_var("CARGO_TARGET_TMPDIR");
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", home.join(".config"));
    std::env::set_var("XDG_DATA_HOME", home.join(".local/share"));
    std::env::set_var("XDG_CACHE_HOME", home.join(".cache"));
    std::env::set_var("APPDATA", home.join("AppData"));
    let real = mc2_platform::paths::UserPaths::resolve()
        .expect("user paths")
        .config;
    assert!(
        real.starts_with(&home),
        "{real:?} must be under the fake home"
    );
    std::fs::create_dir_all(&real).unwrap();
    let poisoned = real.join("options.cfg");
    let poison =
        b"FITini \nFITini \n\n[MechCommander2]\nl MusicVolume = 255\nl SfxVolume = 1\n".to_vec();
    std::fs::write(&poisoned, &poison).unwrap();
    let mut fit = mc2_formats::ini::FitIni::from_bytes(poison.clone()).expect("poison parses");
    assert_eq!(
        mc2_ui::screens::prefs::Prefs::from_fit(&mut fit).music_volume,
        255,
        "the fixture really is a maxed slider"
    );

    // a fake player save, which a headless session must neither list, load nor overwrite
    std::env::remove_var("MC2_SAVE_DIR");
    let real_saves = mc2_app::campaign_save::default_save_directory().expect("save dir");
    assert!(
        real_saves.starts_with(&home),
        "{real_saves:?} under fake home"
    );
    std::fs::create_dir_all(&real_saves).unwrap();
    let sentinel = real_saves.join("slot1.sav");
    std::fs::write(&sentinel, b"sentinel").unwrap();

    let mut opts = HeadlessOptions::new(data, "");
    opts.size = (800, 600);
    let mut run = match HeadlessRun::with_driver(&opts, Box::new(|_| Ok(NullSim::default()))) {
        Ok(r) => r,
        Err(e)
            if e.contains("adapter")
                && std::env::var_os("MC2_SKIP_GPU_TESTS").is_some_and(|v| v == "1") =>
        {
            eprintln!("no GPU: {e}; skipping");
            return;
        }
        Err(e) => panic!("{e}"),
    };
    for _ in 0..6 {
        run.step(None).expect("frame");
    }
    let host: &mut UiHost = run.core.ui_host_mut().expect("production UI host");
    let prefs = host.prefs().expect("prefs loaded after the first frames");
    assert_eq!(
        prefs.music_volume, 128,
        "prefs must come from the shipped data-root options.cfg"
    );
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let saves = host
        .campaign_save_directory()
        .expect("save dir")
        .to_path_buf();
    assert!(
        saves.starts_with(&target) && !saves.starts_with(&home),
        "{saves:?}"
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"sentinel");
    assert_eq!(std::fs::read_dir(&real_saves).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(&poisoned).unwrap(),
        poison,
        "the player's file is untouched"
    );
    assert_eq!(
        std::fs::read_dir(&real).unwrap().count(),
        1,
        "nothing else was written there"
    );
    let _ = std::fs::remove_dir_all(&home);
}
