//! Count bounds at the loader level: unrelated and duplicate blocks must not satisfy a
//! declared count (synthetic FIT text; no data needed).

use mc2_formats::ini::FitIni;
use mc2_sim_data::mission::Mission;

fn fit_text(text: &str) -> FitIni {
    FitIni::from_bytes(format!("FITini\r\n\r\n{text}\r\nFITend\r\n").into_bytes()).expect("parse")
}

/// The blocks every mission needs before `[Parts]`, followed by `body`.
fn mission(parts_block: &str, junk: usize) -> String {
    let mut text = String::from(
        "[MissionSettings]\r\nl ResourcePoints = 0\r\nuc scenarioTuneNum = 1\r\n\
         [Script]\r\nst ScenarioScript = \"none\"\r\n\
         [Warriors]\r\nul NumWarriors = 0\r\n",
    );
    text.push_str(parts_block);
    for i in 0..junk {
        text.push_str(&format!("[Junk{i}]\r\nl x = {i}\r\n"));
    }
    text
}

#[test]
fn unrelated_blocks_do_not_satisfy_a_part_count() {
    // 20,000 unrelated blocks; NumParts = 3 but only Part1 exists.
    let text = mission(
        "[Parts]\r\nul NumParts = 3\r\n[Part1]\r\nul ObjectNumber = 3\r\n",
        20_000,
    );
    let mut f = fit_text(&text);
    let err = Mission::load(&mut f, "test").expect_err("Part2 and Part3 are missing");
    assert!(err.to_string().contains("Part2"), "{err}");
}

#[test]
fn duplicate_part_blocks_do_not_satisfy_a_part_count() {
    // Part1 repeated 10,000 times is one index; NumParts = 2 needs Part2.
    let mut parts = String::from("[Parts]\r\nul NumParts = 2\r\n");
    for _ in 0..10_000 {
        parts.push_str("[Part1]\r\nul ObjectNumber = 3\r\n");
    }
    let mut f = fit_text(&mission(&parts, 0));
    let err = Mission::load(&mut f, "test").expect_err("Part2 is missing");
    assert!(err.to_string().contains("Part2"), "{err}");
}

#[test]
fn huge_part_count_is_refused_without_a_loop() {
    let mut f = fit_text(&mission("[Parts]\r\nul NumParts = 4000000000\r\n", 0));
    let err = Mission::load(&mut f, "test").expect_err("above the entry limit");
    assert!(err.to_string().contains("NumParts"), "{err}");
}
