//! MC2_DATA-gated inventory: every shipped .mpg decodes and we record frame count/size/duration.

use mc2_movie::demux::demux;
use mc2_movie::video::VideoDecoder;
use std::collections::HashMap;
use std::io::Write;

#[test]
fn all_movies_decode() {
    let Ok(mc2_data) = std::env::var("MC2_DATA") else {
        println!("MC2_DATA not set, skipping");
        return;
    };
    let movies_dir = std::path::Path::new(&mc2_data).join("data/movies");
    let mpgs = std::fs::read_dir(&movies_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|e| e == "mpg"))
        .map(|e| e.path())
        .collect::<Vec<_>>();

    println!("Found {} .mpg files", mpgs.len());
    assert!(mpgs.len() > 180, "Expected ~185 shipped .mpg files");

    let mut results = Vec::new();
    for path in &mpgs {
        let data = std::fs::read(path).unwrap();
        let d = demux(&data).unwrap();
        let v = d.video.expect("video stream");
        let mut dec = VideoDecoder::new(v.data, v.marks);
        dec.set_strict(true);

        let mut frames = 0;
        let mut first_time: Option<f64> = None;
        let mut last_time = 0.0;
        let mut sizes = HashMap::new();
        let mut types = String::new();

        while let Some(f) = dec.next_frame().unwrap() {
            if first_time.is_none() {
                first_time = Some(f.time);
            }
            last_time = f.time;
            sizes.insert((f.width, f.height), true);
            types.push(match f.picture_type {
                mc2_movie::video::PictureType::I => 'I',
                mc2_movie::video::PictureType::P => 'P',
                mc2_movie::video::PictureType::B => 'B',
            });

            frames += 1;
        }
        assert!(
            frames > 0,
            "{}: no frames decoded",
            path.file_name().unwrap().to_string_lossy()
        );

        let duration = last_time - first_time.unwrap_or(0.0);
        let seq = dec.sequence().cloned();
        let seq_info = seq.map(|s| format!("{}x{}@{:.3}fps", s.width, s.height, s.frame_rate));

        results.push((
            path.file_name().unwrap().to_string_lossy().to_string(),
            frames,
            duration,
            seq_info.clone(),
        ));

        println!(
            "{}: {} frames, {:.3}s, {}{}",
            path.file_name().unwrap().to_string_lossy(),
            frames,
            duration,
            seq_info.as_deref().unwrap_or("unknown"),
            if frames > 0 && types.len() > 10 {
                format!("... [{}]", types)
            } else if frames > 0 {
                format!(" [{}]", types)
            } else {
                String::new()
            }
        );
    }

    // Verify we found 185 files
    assert_eq!(mpgs.len(), 185, "Expected exactly 185 .mpg files");

    // Write inventory to file for later inspection
    let out_dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(out_dir).unwrap();
    let mut out = std::fs::File::create(out_dir.join("mc2-movie-inventory.txt")).unwrap();
    for (name, frames, duration, seq_info) in &results {
        writeln!(
            out,
            "{}\t{}\t{:.3}\t{}",
            name,
            frames,
            duration,
            seq_info.as_deref().unwrap_or("none")
        )
        .unwrap();
    }
}
