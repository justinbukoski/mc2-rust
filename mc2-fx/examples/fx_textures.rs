use mc2_fx::library::EffectLibrary;
use std::collections::BTreeMap;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).expect("vfs");
    let bytes = vfs.read("data/effects/mc2.fx").unwrap().expect("mc2.fx");
    let lib = EffectLibrary::load(&bytes).unwrap();
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    let mut blends: BTreeMap<String, usize> = BTreeMap::new();
    for s in lib.specs() {
        if let Some(t) = &s.base.state.texture {
            *names
                .entry(format!("{} inst={}", t.name, t.instance))
                .or_default() += 1;
        }
        *blends
            .entry(format!(
                "{:?} prio={} zw={}",
                s.base.state.blend(),
                s.base.state.priority(),
                s.base.state.z_write()
            ))
            .or_default() += 1;
    }
    for (n, c) in &names {
        let base = n.split(' ').next().unwrap();
        let found = ["data/tgl/128/", "data/textures/", "data/effects/", ""]
            .iter()
            .find_map(|p| {
                for ext in [".tga", ".TGA", ""] {
                    let path = format!("{p}{base}{ext}");
                    if vfs.locate(&path).is_some() {
                        return Some(path);
                    }
                }
                None
            });
        println!("{n:40} x{c:3} {found:?}");
    }
    println!("{blends:?}");
}
