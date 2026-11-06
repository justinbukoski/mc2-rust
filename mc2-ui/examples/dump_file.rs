//! Dev helper: writes one file of the shipped game data (loose or inside an archive) to stdout.
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example dump_file -- data/art/mcl_sm.fit`
use std::io::Write;

fn main() {
    let name = std::env::args().nth(1).expect("usage: dump_file NAME");
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let bytes = vfs
        .read(&name)
        .expect("read")
        .unwrap_or_else(|| panic!("{name}: not found"));
    std::io::stdout().write_all(&bytes).expect("write");
}
