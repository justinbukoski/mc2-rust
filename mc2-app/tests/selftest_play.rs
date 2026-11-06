use std::path::Path;
use std::process::Command;

#[test]
fn selftest_play_runs_9000_frames_and_rejects_missing_or_broken_data() {
    let Some(data) = std::env::var_os("MC2_DATA") else {
        panic!("MC2_DATA must point at the shipped mc2exe data for this acceptance test");
    };
    let exe = env!("CARGO_BIN_EXE_mc2");
    let valid = Command::new(exe)
        .args(["--selftest-play", "--data"])
        .arg(&data)
        .output()
        .expect("run packaged selftest command");
    assert!(
        valid.status.success(),
        "selftest failed: {}\n{}",
        String::from_utf8_lossy(&valid.stdout),
        String::from_utf8_lossy(&valid.stderr)
    );
    let stdout = String::from_utf8_lossy(&valid.stdout);
    assert!(stdout.contains(&format!("selftest data: {}", Path::new(&data).display())));
    assert!(stdout.contains("selftest mission=mc2_01 frames=9000"));
    assert!(stdout.contains("objective check: pass"), "stdout: {stdout}");
    assert!(!stdout.contains("objective check: pending"));

    // hires is reported correctly: the default (no pack built yet, empty cache) and an explicit
    // `off` keep the original art; a finished pack in the cache is reported as on.
    let home = std::env::temp_dir().join(format!("mc2_selftest_hires_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let selftest = |extra: &[&str]| {
        let out = Command::new(exe)
            .args(["--selftest-play", "--data"])
            .arg(&data)
            .args(extra)
            .env("HOME", &home)
            .env("XDG_CACHE_HOME", home.join("xdg"))
            .env("LOCALAPPDATA", &home)
            .env_remove("MC2_HIRES_DIR")
            .output()
            .expect("run selftest");
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(selftest(&[]).contains("selftest hires: off\n"));
    assert!(selftest(&["--hires-dir", "off"]).contains("selftest hires: off\n"));
    let cache = mc2_app::locate::cache_dir(&|k| match k {
        "HOME" | "LOCALAPPDATA" => Some(home.clone().into_os_string()),
        "XDG_CACHE_HOME" => Some(home.join("xdg").into_os_string()),
        _ => None,
    })
    .unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    let marker = mc2_texpack::builtin::Marker {
        version: mc2_texpack::builtin::BUILD_VERSION,
        fingerprint: mc2_texpack::builtin::data_fingerprint(Path::new(&data)),
        files: Default::default(),
        failures: Vec::new(),
    };
    std::fs::write(
        cache.join(mc2_texpack::builtin::MARKER),
        serde_json::to_vec(&marker).unwrap(),
    )
    .unwrap();
    std::fs::write(
        cache.join("manifest.json"),
        r#"{"format":"mc2-texpack-pack/1","generator":"t","created":"","models":[],"entries":[]}"#,
    )
    .unwrap();
    let on = selftest(&[]);
    assert!(
        on.contains(&format!(
            "selftest hires: {} (upscaled pack)",
            cache.display()
        )),
        "{on}"
    );
    assert!(selftest(&["--hires-dir", "off"]).contains("selftest hires: off\n"));
    std::fs::remove_dir_all(&home).unwrap();

    let missing = Command::new(exe)
        .args([
            "--selftest-play",
            "--data",
            "/mc2-selftest-data-does-not-exist",
        ])
        .output()
        .expect("run selftest with missing data");
    assert!(!missing.status.success());

    let broken = Command::new(exe)
        .args(["--selftest-play", "--data", "/"])
        .output()
        .expect("run selftest with an invalid data root");
    assert!(!broken.status.success());

    // a data dir that only holds a symlink to / must be rejected quickly, not walked
    #[cfg(unix)]
    {
        let dir = std::env::temp_dir().join(format!("mc2-selftest-link-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = std::os::unix::fs::symlink("/", dir.join("rootlink"));
        let t0 = std::time::Instant::now();
        let linked = Command::new(exe)
            .args(["--selftest-play", "--data"])
            .arg(&dir)
            .output()
            .expect("run selftest with a symlinked data root");
        assert!(!linked.status.success());
        assert!(t0.elapsed() < std::time::Duration::from_secs(30));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn objective_judge_fails_a_run_that_did_not_win() {
    let Some(data) = std::env::var_os("MC2_DATA") else {
        panic!("MC2_DATA must point at the shipped mc2exe data for this acceptance test");
    };
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(Path::new(&data)).expect("mount data");
    let run = mc2_sim::test_hooks::Harness::play_mission(&mut vfs, "mc2_01").expect("play");
    assert!(mc2_app::headless::judge_objective_run(&run).is_ok());

    let mut lost = run.clone();
    lost.result = 0;
    let why = mc2_app::headless::judge_objective_run(&lost).unwrap_err();
    assert!(why.contains("result 0"), "{why}");

    let mut unmet = run.clone();
    let i = unmet.visible[0].index;
    unmet.runtime[i] = mc2_sim::mission::objective::ObjectiveStatus::Undetermined;
    let why = mc2_app::headless::judge_objective_run(&unmet).unwrap_err();
    assert!(why.contains("not Successful"), "{why}");

    let mut none = run;
    none.visible.clear();
    assert!(mc2_app::headless::judge_objective_run(&none).is_err());
}
