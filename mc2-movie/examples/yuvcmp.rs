//! Developer tool: compare two raw yuv420p files frame by frame (luma and chroma PSNR).
//!
//! `cargo run --release -p mc2-movie --example yuvcmp -- A.yuv B.yuv WIDTH HEIGHT [-v]`

fn psnr(sse: f64, n: usize) -> f64 {
    if sse == 0.0 {
        f64::INFINITY
    } else {
        10.0 * (255.0f64 * 255.0 * n as f64 / sse).log10()
    }
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (fa, fb) = (std::fs::read(&a[0]).unwrap(), std::fs::read(&a[1]).unwrap());
    let w: usize = a[2].parse().unwrap();
    let h: usize = a[3].parse().unwrap();
    let verbose = a.iter().any(|s| s == "-v");
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let fsz = w * h + 2 * cw * ch;
    let (na, nb) = (fa.len() / fsz, fb.len() / fsz);
    println!("frames: {na} vs {nb}");
    let n = na.min(nb);
    let (mut min_y, mut min_c) = (f64::INFINITY, f64::INFINITY);
    let mut exact = 0;
    let mut worst_max = 0i32;
    let mut sse_y_tot = 0f64;
    for i in 0..n {
        let (x, y) = (&fa[i * fsz..(i + 1) * fsz], &fb[i * fsz..(i + 1) * fsz]);
        let mut sse = [0f64; 2];
        let mut maxd = 0;
        let mut ndiff = 0;
        for (j, (p, q)) in x.iter().zip(y).enumerate() {
            let d = i32::from(*p) - i32::from(*q);
            let k = usize::from(j >= w * h);
            sse[k] += f64::from(d * d);
            if d != 0 {
                ndiff += 1;
            }
            maxd = maxd.max(d.abs());
        }
        let py = psnr(sse[0], w * h);
        let pc = psnr(sse[1], 2 * cw * ch);
        min_y = min_y.min(py);
        min_c = min_c.min(pc);
        sse_y_tot += sse[0];
        worst_max = worst_max.max(maxd);
        if ndiff == 0 {
            exact += 1;
        }
        if verbose && ndiff > 0 {
            println!(
                "frame {i}: Y {py:.2} dB, C {pc:.2} dB, max diff {maxd}, {ndiff} samples differ"
            );
        }
    }
    println!(
        "{n} frames compared: {exact} bit-exact; min luma PSNR {min_y:.2} dB, min chroma PSNR {min_c:.2} dB, \
         overall luma PSNR {:.2} dB, max |diff| {worst_max}",
        psnr(sse_y_tot, w * h * n)
    );
}
