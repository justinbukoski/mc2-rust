use mc2_campaign::GameData;
use mc2_formats::vfs::Vfs;
use std::time::Instant;
fn main() {
    let root = std::env::var_os("MC2_DATA").unwrap();
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    let t = Instant::now();
    let d = GameData::load(&mut vfs).unwrap();
    println!("loaded in {:?}", t.elapsed());
    println!("components {}", d.catalog.len());
    println!(
        "chassis {} designer variants {}",
        d.chassis.len(),
        d.designer_variants.len()
    );
    println!("pilots {}", d.pilots.len());
    println!("campaigns {:?}", d.campaigns.keys().collect::<Vec<_>>());
    println!(
        "purchase lists {:?}",
        d.purchase_lists.keys().collect::<Vec<_>>()
    );
    for c in d.catalog.iter() {
        println!(
            "C id={} form={:?} w={} h={} cost={} heat={} dmg={} rec={} ammo={} wh={}x{} str={}",
            c.id,
            c.form,
            c.weight,
            c.heat,
            c.cost,
            c.heat,
            c.damage,
            c.recycle_time,
            c.ammo,
            c.width,
            c.height,
            c.string_id
        );
    }
    for (ci, c) in d.chassis.iter().enumerate() {
        println!("CH{} {} id={:x} w={} cost={} heat={} armor={}/{} spd={} area={}x{} jj={} ecm={} adv={} fit={}", ci, c.file_name, c.id, c.max_weight, c.base_cost, c.max_heat, c.base_armor, c.max_armor, c.speed, c.component_area_width, c.component_area_height, c.can_have_jump_jets, c.can_have_ecm, c.can_have_adv_sensor, c.fit_id);
    }
    for (ci, v) in &d.designer_variants {
        println!(
            "V ch{} file={} '{}' hidden={} comps={}",
            ci,
            v.file_id,
            v.name,
            v.hidden,
            v.components.len()
        );
    }
}
