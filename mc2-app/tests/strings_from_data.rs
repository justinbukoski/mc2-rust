//! The UI string table comes from the player's own resource library, never from the program:
//! the game loads it from the data folder at start-up and stops with a first-run message when
//! the folder lacks the library.

use mc2_ui::strings::{parse_fixture, StringTable};
use std::path::{Path, PathBuf};
use std::process::Command;

fn data() -> Option<PathBuf> {
    match std::env::var_os("MC2_DATA") {
        Some(d) => Some(PathBuf::from(d)),
        None => {
            eprintln!("MC2_DATA not set; skipping");
            None
        }
    }
}

/// The table converted from the C++ source (a private test fixture, absent from the public export).
fn fixture() -> Option<Vec<(u32, String)>> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../mc2-ui/tests/fixtures/mc2res-strings.txt");
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(parse_fixture(&text).expect("fixture")),
        Err(_) => {
            eprintln!("{} not found; skipping", path.display());
            None
        }
    }
}

#[test]
fn the_game_loads_the_fixture_table_from_the_players_data() {
    let Some(data) = data() else { return };
    let Some(fixture) = fixture() else { return };
    let table = StringTable::init_from_data_root(&data).expect("string table");
    assert_eq!(table.len(), fixture.len(), "id count");
    let differ: Vec<u32> = fixture
        .iter()
        .filter(|(id, s)| table.get(*id) != Some(s.as_str()))
        .map(|(id, _)| *id)
        .collect();
    assert!(
        differ.is_empty(),
        "{} differ, e.g. {:?}",
        differ.len(),
        &differ[..differ.len().min(5)]
    );
    // the table the screens use is the one loaded from the data
    assert!(std::ptr::eq(StringTable::builtin(), table));
}

#[test]
fn a_data_folder_without_the_resource_library_stops_before_any_screen() {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("no-strings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_mc2"))
        .args(["--list-missions", "--data"])
        .arg(&dir)
        .env_remove("MC2_DATA")
        .output()
        .expect("run mc2");
    std::fs::remove_dir_all(&dir).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains(&format!(
            "game data incomplete: the string resource library mc2res.dll was not found in {}",
            dir.display()
        )),
        "{stderr}"
    );
    assert!(
        stderr.contains("copy mc2res.dll from your retail installation"),
        "{stderr}"
    );
    assert!(!stderr.contains("libmc2res"), "{stderr}");
    assert!(out.stdout.is_empty());
}

#[test]
fn the_game_starts_with_the_players_library() {
    let Some(data) = data() else { return };
    let out = Command::new(env!("CARGO_BIN_EXE_mc2"))
        .args(["--list-missions", "--data"])
        .arg(&data)
        .output()
        .expect("run mc2");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
