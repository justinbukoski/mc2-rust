//! Developer tool: decode a movie headless and report/dump it. Never opens a window.
//!
//! `cargo run --release -p mc2-movie --example probe -- FILE.mpg [--yuv OUT.yuv]`

use mc2_movie::demux::demux;
use mc2_movie::video::VideoDecoder;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args.first().expect("usage: probe FILE.mpg [--yuv OUT]");
    let data = std::fs::read(path).expect("read");
    let d = demux(&data).expect("demux");
    let v = d.video.expect("video stream");
    let mut dec = VideoDecoder::new(v.data, v.marks);
    dec.set_strict(true);
    let mut out = args
        .iter()
        .position(|a| a == "--yuv")
        .map(|i| std::io::BufWriter::new(std::fs::File::create(&args[i + 1]).unwrap()));
    let mut n = 0;
    let mut first = None;
    let mut last = 0.0;
    let mut types = String::new();
    while let Some(f) = dec.next_frame().expect("decode") {
        if first.is_none() {
            first = Some(f.time);
        }
        last = f.time;
        if n < 40 {
            types.push(match f.picture_type {
                mc2_movie::video::PictureType::I => 'I',
                mc2_movie::video::PictureType::P => 'P',
                mc2_movie::video::PictureType::B => 'B',
            });
        }
        if let Some(o) = out.as_mut() {
            for r in 0..f.height as usize {
                o.write_all(&f.y[r * f.y_stride as usize..][..f.width as usize])
                    .unwrap();
            }
            for pl in [&f.cb, &f.cr] {
                for r in 0..(f.height as usize).div_ceil(2) {
                    o.write_all(&pl[r * f.c_stride as usize..][..(f.width as usize).div_ceil(2)])
                        .unwrap();
                }
            }
        }
        n += 1;
    }
    println!(
        "{path}: {:?} frames={n} first={:?} last={last:.3} gop={types}",
        dec.sequence(),
        first
    );
}
