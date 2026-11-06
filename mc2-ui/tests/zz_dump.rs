use mc2_formats::vfs::Vfs;
#[test]
fn dump() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        return;
    };
    let Some(list) = std::env::var_os("DUMP_FILES") else {
        return;
    };
    let out = std::env::var("DUMP_OUT").unwrap();
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    for n in list.to_string_lossy().split(',') {
        let b = vfs.read(n).unwrap();
        let name = n.rsplit('/').next().unwrap();
        match b {
            Some(b) => std::fs::write(format!("{out}/{name}"), b).unwrap(),
            None => eprintln!("missing {n}"),
        }
    }
}
