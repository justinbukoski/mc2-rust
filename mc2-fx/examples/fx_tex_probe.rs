//! Dev helper: report whether each named effect texture resolves under `data/tgl/128/`.
use mc2_formats::vfs::Vfs;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let mut vfs = Vfs::from_system_cfg(root).unwrap();
    for n in std::env::args().skip(1).filter(|a| a != "-g") {
        let path = format!("data/tgl/128/{n}.tga");
        match vfs.read(&path) {
            Ok(Some(b)) => match mc2_assets::tga::decode(&b) {
                Ok(i) => {
                    let a: Vec<u8> = i.pixels.chunks(4).map(|p| p[3]).collect();
                    if std::env::args().any(|a| a == "-g") {
                        for y in (0..i.height as usize).step_by(8) {
                            let row: Vec<String> = (0..i.width as usize)
                                .step_by(8)
                                .map(|x| {
                                    let p = &i.pixels[(y * i.width as usize + x) * 4..];
                                    format!("{:02x}{:02x}{:02x}", p[0], p[1], p[2])
                                })
                                .collect();
                            println!("{}", row.join(" "));
                        }
                    }
                    println!(
                        "{path}: {}x{} alpha min {} max {}",
                        i.width,
                        i.height,
                        a.iter().min().unwrap(),
                        a.iter().max().unwrap()
                    );
                }
                Err(e) => println!("{path}: decode error {e:?}"),
            },
            _ => println!("{path}: MISSING"),
        }
    }
}
