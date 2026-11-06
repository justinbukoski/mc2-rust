//! Dev helper: describe a library effect and count what it draws over time.
use mc2_formats::vfs::Vfs;
use mc2_fx::draw::DrawList;
use mc2_fx::math::Mat;
use mc2_fx::spec::SpecKind;
use mc2_fx::system::{FxSystem, SpawnParams};
fn describe(fx: &FxSystem, id: usize, depth: usize) {
    let s = fx.library().spec(id).unwrap();
    println!(
        "{}{} [{}] life {} state blend {:?} prio {} tex {:?} events {}",
        "  ".repeat(depth),
        s.name(),
        s.class().name(),
        s.base.life_span.value,
        s.base.state.blend(),
        s.base.state.priority(),
        s.base.state.texture.as_ref().map(|t| t.name.clone()),
        s.base.events.len()
    );
    if let SpecKind::EffectCloud(c) = &s.kind {
        describe(fx, c.particle_effect_id as usize, depth + 1);
    }
    for e in &s.base.events {
        println!(
            "{}  event at {} flags {}",
            "  ".repeat(depth),
            e.time,
            e.flags
        );
        describe(fx, e.effect_id as usize, depth + 2);
    }
}
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let mut fx = FxSystem::from_vfs(&mut vfs, 1).unwrap();
    let name = std::env::args().nth(1).expect("effect name");
    let id = fx
        .library()
        .find(&name)
        .map(|s| s.base.effect_id as usize)
        .expect("no such effect");
    describe(&fx, id, 0);
    fx.spawn(&name, SpawnParams::at(Mat::IDENTITY));
    let mut t = 0.0;
    for _ in 0..40 {
        t += 0.05;
        fx.update(t);
        let mut l = DrawList::default();
        fx.draw(
            &Mat::from_translation(mc2_fx::math::Vec3::new(0.0, 100.0, 300.0)),
            &mut l,
        );
        println!(
            "t={t:.2} active={} tris={} batches={}",
            fx.active_count(),
            l.triangle_count(),
            l.batches.len()
        );
        if fx.active_count() == 0 {
            break;
        }
    }
}
