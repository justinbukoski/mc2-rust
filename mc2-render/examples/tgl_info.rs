//! Dev helper: print structure of a TGL shape and AGL animation.
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = mc2_formats::vfs::Vfs::from_system_cfg(&root).unwrap();
    let name = std::env::args().nth(1).expect("name");
    let b = vfs.read(&name).unwrap().expect("missing");
    if name.ends_with(".agl") {
        let anims = mc2_assets::tgl::decode_animation(&b).unwrap();
        for a in anims {
            println!(
                "{:?} shape_id={} frames q={} p={} fr={} tr={}",
                a.node_id,
                a.shape_id,
                a.quaternions.len(),
                a.positions.len(),
                a.frame_rate,
                a.tick_rate
            );
        }
        return;
    }
    let s = mc2_assets::tgl::decode_shape(&b).unwrap();
    for t in &s.textures {
        println!("TEX {:?} node={} alpha={}", t.name, t.node_index, t.alpha);
    }
    for (i, n) in s.nodes.iter().enumerate() {
        let (nv, nt) = n
            .shape
            .as_ref()
            .map(|s| (s.vertices.len(), s.triangles.len()))
            .unwrap_or((0, 0));
        println!(
            "NODE {i} {:?} parent={:?}({:?}) center={:?} rel={:?} v={nv} t={nt}",
            n.node_id, n.parent_id, n.parent_index, n.node_center, n.relative_node_center
        );
        if let Some(sh) = &n.shape {
            let mut mn = [1e9f32; 3];
            let mut mx = [-1e9f32; 3];
            for v in &sh.vertices {
                for k in 0..3 {
                    mn[k] = mn[k].min(v.position[k]);
                    mx[k] = mx[k].max(v.position[k]);
                }
            }
            println!(
                "   bounds {:?} {:?} textures={:?} flags0={:?}",
                mn,
                mx,
                sh.textures,
                sh.triangles.first().map(|t| (t.texture, t.render_flags))
            );
        }
    }
}
