//! Dev helper: tile atlas statistics for every shipped mission.
use mc2_render::{game::GameData, mission::Mission, terrain_tex::TerrainTextures};
fn main() {
    let mut game = GameData::from_env().unwrap();
    let mut names: Vec<String> = (1..=24).map(|i| format!("mc2_{i:02}")).collect();
    names.extend((1..=10).map(|i| format!("mc2_m{i:02}")));
    names.extend((1..=5).map(|i| format!("tut_{i:02}")));
    names.push("e3demo".into());
    for n in names {
        let Ok(m) = Mission::load(&mut game, &n) else {
            println!("{n}: no mission");
            continue;
        };
        match TerrainTextures::load(&mut game, &m) {
            Ok(t) => {
                let mut uniq = std::collections::HashSet::new();
                for l in &t.tile_layers {
                    uniq.insert(l.clone());
                }
                println!(
                    "{n}: side {} layers {} unique {} missing {} objects {} parts {}",
                    m.terrain.side(),
                    t.tile_layers.len(),
                    uniq.len(),
                    t.missing_tiles.len(),
                    m.objects.len(),
                    m.parts.len()
                );
            }
            Err(e) => println!("{n}: tex error {e}"),
        }
    }
}
