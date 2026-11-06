//! Dev helper: write a file from the game VFS to stdout (or --out path).
use std::io::Write;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let name = std::env::args().nth(1).expect("name");
    match vfs.read(&name).unwrap() {
        Some(b) => std::io::stdout().write_all(&b).unwrap(),
        None => {
            eprintln!("not found");
            std::process::exit(1)
        }
    }
}
