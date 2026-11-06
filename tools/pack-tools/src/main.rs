//! pack-tools: the packaging tools.
//!
//! ```text
//! pack-tools render-product template SRC DST          # @PRODUCT_NAME@ @PRODUCT_SLUG@ @BUNDLE_ID@ from the environment
//! pack-tools render-product plist DST VERSION REVISION # macOS Info.plist from the same environment
//! pack-tools gen-notices TARGET_TRIPLE OUT_FILE       # run from rust/
//! pack-tools mkarchive zip|tgz SRC_DIR OUT EPOCH
//! ```
use pack_tools::{archive, notices, render};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage:
  pack-tools render-product template SRC DST
  pack-tools render-product plist DST VERSION REVISION
  pack-tools gen-notices TARGET_TRIPLE OUT_FILE
  pack-tools mkarchive zip|tgz SRC_DIR OUT EPOCH";

fn run(args: &[String]) -> Result<(), String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let env = |key: &str| std::env::var(key).ok();
    match args.as_slice() {
        ["render-product", "template", src, dst] => {
            render::template(Path::new(src), Path::new(dst), &env)
        }
        ["render-product", "plist", dst, version, revision] => {
            render::plist(Path::new(dst), version, revision, &env)
        }
        ["gen-notices", target, out] => notices::run(target, Path::new(out)),
        ["mkarchive", kind, src, out, epoch] => {
            let epoch = epoch
                .parse()
                .map_err(|_| format!("EPOCH is not a number: {epoch}"))?;
            archive::run(kind, Path::new(src), Path::new(out), epoch)
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("pack-tools: {message}");
            ExitCode::FAILURE
        }
    }
}
