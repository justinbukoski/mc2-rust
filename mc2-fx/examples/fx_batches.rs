//! Dev helper: spawn a library effect and print the paint colour of its first vertex over time.
use mc2_formats::vfs::Vfs;
use mc2_fx::draw::DrawList;
use mc2_fx::math::Mat;
use mc2_fx::system::{FxSystem, SpawnParams};
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let mut fx = FxSystem::from_vfs(&mut vfs, 1).unwrap();
    let name = std::env::args().nth(1).expect("effect name");
    fx.spawn(&name, SpawnParams::at(Mat::IDENTITY));
    for step in 1..=16 {
        let t = step as f32 * 0.05;
        fx.update(t.into());
        let mut l = DrawList::default();
        fx.draw(
            &Mat::from_translation(mc2_fx::math::Vec3::new(0.0, 100.0, 300.0)),
            &mut l,
        );
        for b in &l.batches {
            if step == 4 {
                for v in &b.vertices {
                    println!("  pos {:?} uv {:?}", v.pos, v.uv);
                }
            }
            let ys: Vec<f32> = b.vertices.iter().map(|v| v.pos[1]).collect();
            let r = b
                .vertices
                .iter()
                .map(|v| (v.pos[0] * v.pos[0] + v.pos[2] * v.pos[2]).sqrt())
                .fold(0.0f32, f32::max);
            println!(
                "t={t:.2} {:?} tris {} color {:?} ymin {:.1} ymax {:.1} rmax {:.1}",
                b.state.blend,
                b.indices.len() / 3,
                b.vertices.first().map(|v| v.color),
                ys.iter().cloned().fold(f32::MAX, f32::min),
                ys.iter().cloned().fold(f32::MIN, f32::max),
                r
            );
        }
    }
}
