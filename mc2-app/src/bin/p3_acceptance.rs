//! Headless-only progress reporter for the P3 mc2_01 acceptance replay.

fn main() {
    let ready = mc2_app::p3_acceptance::progress_report(|line| println!("{line}"));
    if !ready {
        std::process::exit(1);
    }
}
