//! Byte-for-byte checks against the goldens the replaced Python tools produced (see `golden/README.md`),
//! plus the notices and product renderings.
use pack_tools::{archive, notices, render};
use std::path::{Path, PathBuf};

fn here(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn golden(name: &str) -> Vec<u8> {
    std::fs::read(here("tests/golden").join(name)).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("pack-tools")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn identity(key: &str) -> Option<String> {
    match key {
        "PRODUCT_NAME" => Some("Second & \"Product\"".into()),
        "PRODUCT_SLUG" => Some("second-product".into()),
        "BUNDLE_ID" => Some("org.example.second-product".into()),
        _ => None,
    }
}

#[test]
fn template_matches_golden() {
    let dir = scratch("template");
    let out = dir.join("README-PLAYERS.md");
    render::template(
        &here("../../packaging/README-PLAYERS.md.in"),
        &out,
        &identity,
    )
    .unwrap();
    assert!(std::fs::read(&out).unwrap() == golden("README-PLAYERS.md"));
}

#[test]
fn template_translates_newlines_and_needs_every_key() {
    let got = render::template_text(
        "@PRODUCT_NAME@\r\n@BUNDLE_ID@\r@PRODUCT_SLUG@@NOPE@",
        &identity,
    )
    .unwrap();
    assert_eq!(
        got,
        "Second & \"Product\"\norg.example.second-product\nsecond-product@NOPE@"
    );
    assert!(render::template_text("x", &|k| (k != "BUNDLE_ID").then(|| "v".to_string())).is_err());
}

#[test]
fn plist_matches_golden() {
    let out = scratch("plist").join("Info.plist");
    render::plist(&out, "1.2.3", "0123456789ab", &identity).unwrap();
    assert!(std::fs::read(&out).unwrap() == golden("Info.plist"));
}

#[test]
fn notices_match_golden() {
    let root = here("tests/fixtures/notices");
    let meta = std::fs::read_to_string(root.join("metadata.json")).unwrap();
    let meta = meta.replace("@ROOT@", root.to_str().unwrap());
    let (text, count) = notices::notices_text(
        &serde_json::from_str(&meta).unwrap(),
        "x86_64-unknown-linux-gnu",
    )
    .unwrap();
    assert_eq!(count, 8);
    assert!(text.as_bytes() == golden("notices.txt").as_slice());
}

#[cfg(unix)]
/// The fixture tree plus what git cannot hold: an empty directory and explicit modes.
fn archive_tree(name: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
                let mode = if entry.file_name() == "run.sh" {
                    0o755
                } else {
                    0o644
                };
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }
    let tree = scratch(name).join("pkg");
    copy(&here("tests/fixtures/archive/pkg"), &tree);
    std::fs::create_dir(tree.join("emptydir")).unwrap();
    tree
}

#[cfg(unix)]
#[test]
fn tgz_matches_golden() {
    let entries = archive::collect(&archive_tree("tree-tgz")).unwrap();
    assert!(archive::gzip(&archive::tar(&entries, 1_700_000_000)) == golden("archive.tar.gz"));
}

#[cfg(unix)]
#[test]
fn zip_matches_golden() {
    let entries = archive::collect(&archive_tree("tree-zip")).unwrap();
    assert!(archive::zip(&entries, 1_700_000_000).unwrap() == golden("archive.zip"));
    // epochs before 1980 clamp to 1980-01-01 00:00:00
    assert!(archive::zip(&entries, 1000).unwrap() == golden("archive-early.zip"));
}


#[test]
fn zip_rejects_what_needs_zip64() {
    let entry = |i: usize| archive::Entry {
        name: format!("d/{i}"),
        is_dir: false,
        mode: 0o644,
        data: Vec::new(),
    };
    let ok: Vec<_> = (0..65_535).map(entry).collect();
    assert!(archive::zip(&ok, 1_700_000_000).is_ok());
    let too_many: Vec<_> = (0..65_536).map(entry).collect();
    assert!(archive::zip(&too_many, 1_700_000_000)
        .unwrap_err()
        .contains("ZIP64"));
}
