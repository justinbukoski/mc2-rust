//! Dev helper: list Shape / ShapeCloud effects with each primitive's blend, texture and z state.
use mc2_formats::vfs::Vfs;
use mc2_fx::spec::SpecKind;
use mc2_fx::system::FxSystem;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let fx = FxSystem::from_vfs(&mut vfs, 1).unwrap();
    for s in fx.library().specs() {
        let shape = match &s.kind {
            SpecKind::Shape(x) => &x.shape,
            SpecKind::ShapeCloud(x) => &x.shape,
            _ => continue,
        };
        println!(
            "{} [{}] life {} effect blend {:?} tex {:?}",
            s.name(),
            s.class().name(),
            s.base.life_span.value,
            s.base.state.blend(),
            s.base.state.texture.as_ref().map(|t| t.name.clone())
        );
        for p in &shape.primitives {
            println!(
                "    class {} {:?} verts {} idx {} blend {:?} tex {:?} zw {} zc {} prio {} colors {}",
                p.class_id, p.kind, p.coords.len(), p.index.len(), p.state.blend(),
                p.state.texture.as_ref().map(|t| t.name.clone()),
                p.state.z_write(), p.state.z_compare(), p.state.priority(), p.colors.len()
            );
        }
    }
}
