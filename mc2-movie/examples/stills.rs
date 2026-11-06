//! Developer tool: writes decoded pictures of an MPEG-1 file as PNGs (`Frame::to_rgba`), `STEP`
//! pictures apart, for the first `N` pictures.
//!
//! `cargo run --release -p mc2-movie --example stills -- FILE.mpg OUT_PREFIX N STEP`

use mc2_movie::demux::demux;
use mc2_movie::video::VideoDecoder;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let data = std::fs::read(&a[0]).expect("read");
    let n: usize = a[2].parse().expect("N");
    let step: usize = a[3].parse().expect("STEP");
    let v = demux(&data).expect("demux").video.expect("video stream");
    let mut dec = VideoDecoder::new(v.data, v.marks);
    for i in 0..n {
        let Some(f) = dec.next_frame().expect("decode") else {
            break;
        };
        if i % step != 0 {
            continue;
        }
        let path = format!("{}_{i:03}.png", a[1]);
        let file = std::fs::File::create(&path).expect("create");
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), f.width, f.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .expect("header")
            .write_image_data(&f.to_rgba())
            .expect("data");
        println!("pic {i} time={:.3} -> {path}", f.time);
    }
}
