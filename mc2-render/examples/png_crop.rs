//! Dev helper: crop a region out of a PNG.  usage: png_crop IN.png OUT.png X Y W H
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (x, y, w, h): (usize, usize, usize, usize) = (
        a[3].parse().unwrap(),
        a[4].parse().unwrap(),
        a[5].parse().unwrap(),
        a[6].parse().unwrap(),
    );
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&a[1]).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let mut out = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        for col in x..x + w {
            let p = (row * info.width as usize + col) * ch;
            out.extend_from_slice(&[buf[p], buf[p + 1], buf[p + 2], 255]);
        }
    }
    mc2_render::gpu::write_png(std::path::Path::new(&a[2]), &out, w as u32, h as u32).unwrap();
}
