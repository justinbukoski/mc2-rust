//! Dev helper: convert a TGA from the VFS to a PNG (RGBA).  usage: tga_png NAME OUT.png
fn main() {
    let mut game = mc2_render::game::GameData::from_env().unwrap();
    let name = std::env::args().nth(1).expect("name");
    let out = std::env::args().nth(2).expect("out");
    let img = game.tga(&name).unwrap();
    mc2_render::gpu::write_png(
        std::path::Path::new(&out),
        &img.pixels,
        img.width as u32,
        img.height as u32,
    )
    .unwrap();
    println!("{}x{}", img.width, img.height);
}
