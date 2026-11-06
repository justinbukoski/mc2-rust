//! Dev helper: dump packet N of a pak from the VFS.
use std::io::Write;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let name = std::env::args().nth(1).expect("name");
    let idx: Option<usize> = std::env::args().nth(2).and_then(|s| s.parse().ok());
    let b = vfs.read(&name).unwrap().expect("missing");
    let pak = mc2_formats::pak::PakFile::from_bytes(b).unwrap();
    match idx {
        None => {
            for i in 0..pak.packet_count() {
                let info = pak.info(i).unwrap();
                println!("{i} {:?}", info);
            }
        }
        Some(i) => std::io::stdout()
            .write_all(&pak.read_packet(i).unwrap())
            .unwrap(),
    }
}
