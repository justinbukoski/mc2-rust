//! Dev helper: print statistics about a mission's terrain packet.
use mc2_render::{game::GameData, mission::Mission};
use std::collections::BTreeMap;
fn main() {
    let name = std::env::args().nth(1).unwrap_or("mc2_01".into());
    let mut game = GameData::from_env().unwrap();
    let m = Mission::load(&mut game, &name).unwrap();
    let t = &m.terrain;
    println!(
        "side {} elev {:?} water {:?} tp {:?}",
        t.side(),
        t.elevation_range(),
        m.water,
        m.terrain_params
    );
    println!("camera {:?}", m.light);
    let mut types = BTreeMap::new();
    let mut overlays = BTreeMap::new();
    let mut kinds = BTreeMap::new();
    let mut nlen = (9f32, 0f32);
    let mut water = 0;
    for v in t.vertices() {
        *types.entry(v.terrain_type).or_insert(0) += 1;
        let l = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
        nlen = (nlen.0.min(l), nlen.1.max(l));
        if v.water & 1 != 0 {
            water += 1;
        }
        *overlays.entry(v.texture_data >> 16).or_insert(0) += 1;
    }
    let mut txm = BTreeMap::new();
    for r in 0..t.tiles() {
        for c in 0..t.tiles() {
            let ti = t.tile_info(c, r);
            *kinds.entry(format!("{:?}", ti.kind)).or_insert(0) += 1;
            if ti.kind != mc2_render::terrain::TileKind::ColorMap {
                *txm.entry(ti.transition_file()).or_insert(0) += 1;
            }
        }
    }
    println!(
        "types {:?}\noverlays {:?}\nkinds {:?}\nnormal len {:?} water verts {}",
        types, overlays, kinds, nlen, water
    );
    let missing: Vec<_> = txm
        .keys()
        .filter(|k| !game.exists(&format!("data/textures/{k}")))
        .collect();
    println!(
        "unique transition tiles {} missing {:?}",
        txm.len(),
        missing
    );
    println!("objects {} parts {}", m.objects.len(), m.parts.len());
    let mut ot = BTreeMap::new();
    for o in &m.objects {
        *ot.entry(o.object_type).or_insert(0) += 1;
    }
    println!("object types {:?}", ot);
    for o in m.objects.iter().take(5) {
        println!("{:?}", o);
    }
    for p in m.parts.iter().take(5) {
        println!("{:?}", p);
    }
}
