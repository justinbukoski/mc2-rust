use mc2_formats::fst::FastFile;
fn main() {
    let root = std::env::var("MC2_DATA").expect("MC2_DATA");
    let pat = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    for a in [
        "mission", "tgl", "art", "textures", "misc", "camera", "effect", "insignia",
    ] {
        let f = FastFile::open(format!("{root}/{a}.fst")).unwrap();
        for e in f.entries() {
            if e.name.to_lowercase().contains(&pat) {
                println!("{a}.fst\t{}\t{}", e.name, e.real_size);
            }
        }
    }
}
