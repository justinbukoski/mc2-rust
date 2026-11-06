use mc2_fx::library::EffectLibrary;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let bytes = vfs.read("data/effects/mc2.fx").unwrap().expect("mc2.fx");
    println!("{} bytes", bytes.len());
    match EffectLibrary::load(&bytes) {
        Ok(lib) => {
            println!("version {} effects {}", lib.version, lib.len());
            for (c, n) in lib.counts_by_class() {
                println!("{:>14} {}", c.name(), n);
            }
        }
        Err(e) => println!("ERR {e}"),
    }
}
