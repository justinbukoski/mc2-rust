fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let bytes = vfs.read("data/effects/mc2.fx").unwrap().expect("mc2.fx");
    std::fs::write(std::env::args().nth(1).unwrap(), bytes).unwrap();
}
