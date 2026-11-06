//! The published build carries no AI upscale driver, and the CLI says so.

use std::process::Command;

fn run(args: &[&str]) -> (i32, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_mc2-texpack"))
        .args(args)
        .output()
        .expect("run mc2-texpack");
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn default_build_has_no_ai_upscale() {
    let (code, _, err) = run(&["upscale", "--dump", "d", "--pack", "p"]);
    assert_eq!(code, 2);
    assert!(err.contains("not part of this build"), "{err}");
    let (code, out, _) = run(&["--help"]);
    assert_eq!(code, 0);
    assert!(!out.contains("upscale"), "{out}");
    assert!(!out.to_lowercase().contains("modelhub"), "{out}");
}
