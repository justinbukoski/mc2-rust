//! Dev helper (rc-vqa-vertexdump): writes the per-vertex lit colour of a mech preview, in the pose and light
//! `ModelPreviews::render` draws it, without a GPU (`mc2_ui::preview::dump_preview_vertex_light`).
//!
//! `MC2_DATA=... cargo run -p mc2-ui --example vertex_light_dump -- SHAPE WIDTH HEIGHT OUT_DIR [activated]`
//! (`activated`: the camera had shown a model before, so the activation daylight lights it)
use mc2_ui::preview::{dump_preview_vertex_light, PreviewKind, PreviewRequest, DEFAULT_PAINT};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [shape, width, height, out, rest @ ..] = args.as_slice() else {
        panic!("usage: vertex_light_dump SHAPE WIDTH HEIGHT OUT_DIR [activated]");
    };
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut game = mc2_render::game::GameData::open(&root).expect("open MC2_DATA");
    let req = PreviewRequest {
        shape: shape.to_ascii_lowercase(),
        paint: [DEFAULT_PAINT; 3],
        scale: 0.0,
        yaw: 0.0,
        width: width.parse().expect("WIDTH"),
        height: height.parse().expect("HEIGHT"),
        building: false,
        kind: PreviewKind::Mech,
        activated: rest.first().is_some_and(|a| a == "activated"),
    };
    std::fs::create_dir_all(out).expect("create OUT_DIR");
    let path = dump_preview_vertex_light(&mut game, &req, std::path::Path::new(out)).expect("dump");
    println!("{}", path.display());
}
