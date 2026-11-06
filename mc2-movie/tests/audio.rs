//! Audio tests: verify demuxer extracts audio from shipped .mpg files.

use mc2_movie::demux::demux;

#[test]
fn movies_have_audio() {
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

    let mut with_audio = 0;
    let mut without_audio = 0;

    for path in &mpgs {
        let data = std::fs::read(path).unwrap();
        let d = demux(&data).unwrap();

        if let Some(a) = &d.audio {
            with_audio += 1;
            println!(
                "{}: audio {} bytes",
                path.file_name().unwrap().to_string_lossy(),
                a.data.len()
            );
        } else {
            without_audio += 1;
        }
    }

    println!("With audio: {with_audio}, Without audio: {without_audio}");
    assert!(with_audio > 0, "Expected some .mpg files to have audio");
}
