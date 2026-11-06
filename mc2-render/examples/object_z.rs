//! Dev helper: compare stored object heights with the interpolated terrain height.
use mc2_render::{game::GameData, mission::Mission};
fn main() {
    let mut game = GameData::from_env().unwrap();
    let name = std::env::args().nth(1).unwrap_or("mc2_01".into());
    let m = Mission::load(&mut game, &name).unwrap();
    let mut hist = [0usize; 6];
    let mut worst = (0f32, 0i32);
    for o in &m.objects {
        let e = m.terrain.elevation_at(o.position[0], o.position[1]);
        let d = (o.position[2] - e).abs();
        if d > worst.0 {
            worst = (d, o.object_type);
        }
        let b = if d < 0.5 {
            0
        } else if d < 2.0 {
            1
        } else if d < 10.0 {
            2
        } else if d < 50.0 {
            3
        } else if d < 200.0 {
            4
        } else {
            5
        };
        hist[b] += 1;
    }
    println!(
        "{name}: |z - ground| buckets <0.5,<2,<10,<50,<200,more: {:?} worst {:?}",
        hist, worst
    );
}
