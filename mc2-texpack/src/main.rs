//! `mc2-texpack`: dump the game's textures, write the inventory, verify upscaled packs.
//!
//! ```text
//! mc2-texpack dump      --out DIR [--data DIR] [--missions a,b] [--no-originals] [-v]
//! mc2-texpack inventory --out DIR [--data DIR] [--missions a,b] [-v]     (manifest only)
//! mc2-texpack verify    --dump DIR --pack DIR
//! ```
//! The data root defaults to `MC2_DATA`.  Nothing here opens a window.

use mc2_render::game::GameData;
use mc2_texpack::collect::{collect, read_dump_manifest, Options};
use mc2_texpack::{report, verify};
use std::path::PathBuf;
use std::process::ExitCode;

const HELP_BASE: &str = "usage: mc2-texpack <command> [options]\n\ncommands:\n  dump       --out DIR [--data DIR] [--missions a,b] [--no-originals] [-v]\n  inventory  --out DIR [--data DIR] [--missions a,b] [-v]\n  verify     --dump DIR --pack DIR\n";

fn help() -> String {
    HELP_BASE.to_string()
}

fn take(args: &mut std::iter::Peekable<impl Iterator<Item = String>>) -> Option<String> {
    args.next_if(|n| !n.starts_with("--") && n != "-h")
}

fn missing(flag: &str) -> ExitCode {
    eprintln!("mc2-texpack: error: argument {flag}: expected one argument");
    ExitCode::from(2)
}

fn usage() -> ExitCode {
    eprint!("{}", help());
    ExitCode::from(2)
}

fn cmd_help(_cmd: &str) -> String {
    help()
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).peekable();
    let Some(cmd) = args.next() else {
        return usage();
    };
    if cmd == "upscale" {
        eprintln!(
            "mc2-texpack: the AI upscale path is not part of this build. The game builds its own 2x pack from your data on first run (no setup needed)."
        );
        return ExitCode::from(2);
    }
    if cmd == "-h" || cmd == "--help" {
        print!("{}", help());
        return ExitCode::SUCCESS;
    }
    let mut out: Option<PathBuf> = None;
    let mut data: Option<PathBuf> = std::env::var_os("MC2_DATA").map(PathBuf::from);
    let mut missions: Option<Vec<String>> = None;
    let mut originals = cmd == "dump";
    let mut verbose = false;
    let mut dump_dir: Option<PathBuf> = None;
    let mut pack_dir: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => match take(&mut args) {
                Some(v) => out = Some(PathBuf::from(v)),
                None => return missing(&a),
            },
            "--data" => match take(&mut args) {
                Some(v) => data = Some(PathBuf::from(v)),
                None => return missing(&a),
            },
            "--missions" => match take(&mut args) {
                Some(s) => missions = Some(s.split(',').map(str::to_string).collect()),
                None => return missing(&a),
            },
            "--no-originals" => originals = false,
            "--dump" => match take(&mut args) {
                Some(v) => dump_dir = Some(PathBuf::from(v)),
                None => return missing(&a),
            },
            "--pack" => match take(&mut args) {
                Some(v) => pack_dir = Some(PathBuf::from(v)),
                None => return missing(&a),
            },
            "-v" => verbose = true,
            "-h" | "--help" => {
                print!("{}", cmd_help(&cmd));
                return ExitCode::SUCCESS;
            }
            _ => return usage(),
        }
    }
    match cmd.as_str() {
        "dump" | "inventory" => {
            let (Some(out), Some(data)) = (out, data) else {
                eprintln!("--out and a data root (--data or MC2_DATA) are required");
                return ExitCode::from(2);
            };
            let mut game = match GameData::open(&data) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("cannot open {}: {e}", data.display());
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = std::fs::create_dir_all(&out) {
                eprintln!("{}: {e}", out.display());
                return ExitCode::FAILURE;
            }
            let res = collect(
                &mut game,
                &Options {
                    missions,
                    originals: originals.then(|| out.clone()),
                    verbose,
                },
            );
            let manifest = res.manifest;
            let write = |name: &str, bytes: Vec<u8>| {
                if let Err(e) = std::fs::write(out.join(name), bytes) {
                    eprintln!("{name}: {e}");
                    return false;
                }
                true
            };
            let json = serde_json::to_vec_pretty(&manifest).expect("manifest serialises");
            let md = report::markdown(&manifest, &res.warnings, &res.missing_refs);
            let ok = write("manifest.json", json)
                && write("inventory.md", md.clone().into_bytes())
                && write(
                    "warnings.txt",
                    res.warnings
                        .iter()
                        .map(|w| format!("warning: {w}"))
                        .chain(res.missing_refs.iter().map(|m| format!("missing: {m}")))
                        .collect::<Vec<_>>()
                        .join("\n")
                        .into_bytes(),
                );
            println!("{md}");
            println!("wrote {}", out.join("manifest.json").display());
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        "verify" => {
            let (Some(dump_dir), Some(pack_dir)) = (dump_dir, pack_dir) else {
                return usage();
            };
            let dump = match read_dump_manifest(&dump_dir.join("manifest.json")) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            };
            let r = verify::verify(&dump, &pack_dir);
            for e in &r.errors {
                println!("ERROR   {e}");
            }
            for w in &r.warnings {
                println!("warning {w}");
            }
            for (cat, (have, want)) in &r.coverage {
                println!("coverage {:<14} {have}/{want}", cat.as_str());
            }
            println!("{} errors, {} warnings", r.errors.len(), r.warnings.len());
            if r.ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => usage(),
    }
}
