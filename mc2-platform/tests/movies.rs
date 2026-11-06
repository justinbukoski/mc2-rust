//! Decodes shipped cut-scenes headless. Skipped when `MC2_DATA` is unset.

use mc2_formats::vfs::Vfs;
use mc2_platform::audio::{Mixer, PlayMode, QueueSource};
use mc2_platform::movie::{Movie, MovieError, MoviePlayer};
use std::path::PathBuf;

fn movie_path(name: &str) -> Option<PathBuf> {
    let root = std::env::var_os("MC2_DATA")?;
    let p = PathBuf::from(root).join("data/movies").join(name);
    if p.exists() {
        Some(p)
    } else {
        eprintln!("skipping: {} not found", p.display());
        None
    }
}

#[test]
fn rejects_non_mpeg_data() {
    assert!(matches!(
        Movie::from_bytes(Vec::new()),
        Err(MovieError::NotMpeg)
    ));
    assert!(matches!(
        Movie::from_bytes(b"this is definitely not an MPEG program stream".to_vec()),
        Err(MovieError::NotMpeg)
    ));
}

/// Exercise the same VFS -> MoviePlayer -> QueueSource -> Mixer path the app host uses. The
/// two-second virtual run must stay within one frame of the movie clock and within 40 ms at the
/// first audio/video PTS; mixer rendering is headless and never opens an output device.
#[test]
fn intro_movie_production_path_advances_audio_syncs_and_skips() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("skipping intro playback evidence: MC2_DATA is unset");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(PathBuf::from(root)).expect("open data VFS");
    let movie = Movie::open_vfs(&mut vfs, "data/movies/msft.mpg").expect("open intro movie");
    let info = movie.info();
    assert!(info.frame_rate > 1.0 && info.frame_rate < 120.0, "{info:?}");
    assert!(info.has_audio && info.audio_sample_rate > 0, "{info:?}");

    let mut pts_probe = Movie::open_vfs(&mut vfs, "data/movies/msft.mpg").unwrap();
    let first_video_pts = pts_probe.next_video_frame().unwrap().time;
    let first_audio_pts = pts_probe.next_audio_chunk().unwrap().time;
    assert!(
        (first_video_pts - first_audio_pts).abs() <= 0.040,
        "intro A/V PTS skew {:.3} ms exceeds 40 ms",
        (first_video_pts - first_audio_pts).abs() * 1000.0
    );

    let (source, writer) = QueueSource::new(2, info.audio_sample_rate);
    let mut player = MoviePlayer::new(movie, Some(writer.clone()));
    let mut mixer = Mixer::new(info.audio_sample_rate, 1);
    mixer.assign(0, Box::new(source));
    mixer.set_mode(0, PlayMode::PlayOnce);

    let run_seconds = 2.0;
    let steps = (run_seconds * 30.0) as usize;
    let dt = run_seconds / steps as f64;
    let mut frames = 0usize;
    let mut rendered_audio_frames = 0usize;
    let mut max_queued = 0usize;
    let mut out = vec![0.0f32; (info.audio_sample_rate as usize / 30) * 2];
    for _ in 0..steps {
        if player.advance(dt).is_some() {
            frames += 1;
        }
        max_queued = max_queued.max(writer.queued_frames());
        mixer.render(&mut out);
        rendered_audio_frames += out.len() / 2;
    }
    let expected = run_seconds * info.frame_rate;
    assert!(
        (frames as f64 - expected).abs() <= 2.0,
        "got {frames} frames in {run_seconds}s at {} fps (expected {expected:.1})",
        info.frame_rate
    );
    assert!(rendered_audio_frames >= info.audio_sample_rate as usize * 19 / 10);
    assert!(max_queued > 0, "movie PCM was not queued for the mixer");
    assert!(player.clock() >= run_seconds - 0.001);
    player.skip();
    assert!(
        player.is_finished(),
        "skip must end video and audio playback"
    );
    assert_eq!(writer.queued_frames(), 0, "skip must clear queued samples");
}

#[test]
fn natural_completion_drains_audio_past_the_video_end() {
    let Some(path) = movie_path("mc2_24c.mpg") else {
        return;
    };
    let mut probe = Movie::open(&path).expect("open tail-audio movie");
    let info = probe.info();
    let mut first_audio_pts = None;
    let mut last_audio_end = 0.0f64;
    let mut audio_frames = 0usize;
    while let Some(chunk) = probe.next_audio_chunk() {
        first_audio_pts.get_or_insert(chunk.time);
        let count = chunk.samples.len() / 2;
        audio_frames += count;
        last_audio_end = chunk.time + count as f64 / info.audio_sample_rate as f64;
    }
    assert!(info.has_audio && audio_frames > 0);
    assert!(
        last_audio_end > info.duration,
        "fixture must exercise an audio tail: audio {last_audio_end:.3}s, video {:.3}s",
        info.duration
    );

    let movie = Movie::open(&path).expect("reopen tail-audio movie");
    let (source, writer) = QueueSource::new(2, info.audio_sample_rate);
    let mut mixer = Mixer::new(info.audio_sample_rate, 1);
    mixer.assign(0, Box::new(source));
    mixer.set_mode(0, PlayMode::PlayOnce);
    let mut player = MoviePlayer::new(movie, Some(writer.clone()));
    let step_frames = (info.audio_sample_rate / 30) as usize;
    let mut output = vec![0.0f32; step_frames * 2];
    let mut rendered_frames = 0usize;
    let mut steps = 0usize;
    let max_steps = ((last_audio_end + 2.0) * 30.0) as usize;
    while !player.is_finished() && steps < max_steps {
        player.advance(1.0 / 30.0);
        mixer.render(&mut output);
        rendered_frames += step_frames;
        steps += 1;
    }
    assert!(player.is_finished(), "natural playback did not finish");
    assert_eq!(
        writer.queued_frames(),
        0,
        "finished playback must drain PCM"
    );
    let mixed_time = rendered_frames as f64 / f64::from(info.audio_sample_rate);
    let audio_start = first_audio_pts.unwrap_or(0.0);
    assert!(
        mixed_time + audio_start + 0.040 >= last_audio_end,
        "mixer rendered through {mixed_time:.3}s + PTS {audio_start:.3}s, before final audio PTS {last_audio_end:.3}s"
    );
    assert!(
        player.clock() + 0.040 >= last_audio_end,
        "natural completion clock {:.3}s must reach final audio PTS {last_audio_end:.3}s",
        player.clock()
    );
}

#[test]
fn writes_intro_frame_png_under_target_dir() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        eprintln!("skipping intro PNG evidence: MC2_DATA is unset");
        return;
    };
    let mut vfs = Vfs::from_system_cfg(PathBuf::from(root)).expect("open data VFS");
    let mut movie = Movie::open_vfs(&mut vfs, "data/movies/msft.mpg").expect("open intro movie");
    let info = movie.info();
    let mut frame = movie.next_video_frame().expect("decode intro frame");
    for _ in 0..(info.frame_rate * 10.0) as usize {
        frame = movie
            .next_video_frame()
            .expect("decode intro frame at 10 seconds");
    }
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("test-artifacts/intro-frame.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(&path).expect("create intro frame PNG");
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&frame.to_rgba())
        .unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size < 300_000, "intro frame PNG is {size} bytes");
}

#[test]
fn decodes_first_frames_of_a_cinematic() {
    let Some(p) = movie_path("cinema3.mpg") else {
        return;
    };
    let mut m = Movie::open(&p).expect("open");
    let info = m.info();
    eprintln!("cinema3: {info:?}");
    assert_eq!((info.width, info.height), (320, 180));
    assert_eq!(info.frame_rate, 24.0);
    assert!(info.has_audio && info.audio_sample_rate == 44100);
    assert!((info.duration - 44.75).abs() < 1.0 / info.frame_rate);
    assert!(info.frame_rate > 1.0 && info.frame_rate < 120.0);
    m.set_audio_enabled(false);

    let mut last = -1.0;
    let mut n = 0;
    while n < 30 {
        let Some(f) = m.next_video_frame() else { break };
        assert_eq!((f.width, f.height), (info.width, info.height));
        assert!(f.y_stride >= f.width && f.c_stride >= f.width / 2);
        assert_eq!(
            f.y.len(),
            f.y_stride as usize * (f.y.len() / f.y_stride as usize)
        );
        assert!(
            f.time > last,
            "frame times must increase: {} then {}",
            last,
            f.time
        );
        assert!(
            (f.time - n as f64 / info.frame_rate).abs() < 0.05,
            "frame {n} at {}",
            f.time
        );
        last = f.time;
        n += 1;
    }
    assert_eq!(n, 30, "movie shorter than 30 frames");
}

#[test]
fn frames_have_picture_content() {
    let Some(p) = movie_path("cinema3.mpg") else {
        return;
    };
    let mut m = Movie::open(&p).unwrap();
    m.set_audio_enabled(false);
    // Skip a few frames so we are past any fade-in.
    let mut f = m.next_video_frame().unwrap();
    for _ in 0..60 {
        f = m.next_video_frame().unwrap();
    }
    let rgba = f.to_rgba();
    assert_eq!(rgba.len(), (f.width * f.height * 4) as usize);
    let lo = rgba
        .chunks(4)
        .map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32)
        .min()
        .unwrap();
    let hi = rgba
        .chunks(4)
        .map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32)
        .max()
        .unwrap();
    assert!(hi > lo + 60, "frame looks flat: {lo}..{hi}");
    assert!(rgba.chunks(4).all(|p| p[3] == 255));
}

#[test]
fn whole_short_movie_decodes_every_frame() {
    // 76x54 is not a multiple of the 16-pixel macroblock: exercises the padded-plane strides.
    let Some(p) = movie_path("bubba1.mpg") else {
        return;
    };
    let mut m = Movie::open(&p).unwrap();
    let info = m.info();
    assert_eq!((info.width, info.height), (76, 54));
    m.set_audio_enabled(false);
    let mut n = 0u64;
    let mut last_time = 0.0;
    while let Some(f) = m.next_video_frame() {
        assert_eq!(f.y_stride, 80);
        assert_eq!(f.c_stride, 40);
        // Frames come at exactly the nominal rate: no dropped or duplicated frames.
        assert!(
            (f.time - n as f64 / info.frame_rate).abs() < 1e-3,
            "frame {n} at {}",
            f.time
        );
        last_time = f.time;
        n += 1;
        assert!(n < 100_000);
    }
    assert!(m.has_ended());
    eprintln!(
        "bubba1: {n} frames, last at {last_time}, header duration {}",
        info.duration
    );
    // The header duration is estimated from timestamps and can fall a few frames short.
    assert!(n as f64 >= info.duration * info.frame_rate - 1.0);
    assert!(n as f64 <= info.duration * info.frame_rate + 8.0);
    m.rewind();
    assert!(m.next_video_frame().is_some());
}

#[test]
fn audio_track_decodes_when_present() {
    let Some(p) = movie_path("mc2_24d.mpg") else {
        return;
    };
    let mut m = Movie::open(&p).unwrap();
    let info = m.info();
    if !info.has_audio {
        eprintln!("bubba1 has no audio track");
        assert!(m.next_audio_chunk().is_none());
        return;
    }
    assert!(
        matches!(info.audio_sample_rate, 22050 | 32000 | 44100 | 48000),
        "{info:?}"
    );
    m.set_video_enabled(false);
    let mut samples = 0usize;
    let mut peak = 0f32;
    while let Some(c) = m.next_audio_chunk() {
        assert_eq!(c.samples.len() % 2, 0);
        samples += c.samples.len() / 2;
        peak = c.samples.iter().fold(peak, |a, s| a.max(s.abs()));
    }
    let secs = samples as f64 / info.audio_sample_rate as f64;
    eprintln!("audio: {secs:.2} s, peak {peak}");
    assert!(
        (secs - info.duration).abs() < 0.5,
        "audio {secs}s vs duration {}",
        info.duration
    );
    assert!(peak > 0.01 && peak <= 1.0 + 1e-3);
}

#[test]
fn player_feeds_the_mixer_headless() {
    // A longer movie with audio exercises the interleaved demux path.
    let Some(p) = movie_path("cinema3.mpg") else {
        return;
    };
    let movie = Movie::open(&p).unwrap();
    let info = movie.info();
    if !info.has_audio {
        return;
    }
    let (src, writer) = QueueSource::new(2, info.audio_sample_rate);
    let mut player = MoviePlayer::new(movie, Some(writer.clone()));
    let mut mixer = Mixer::new(info.audio_sample_rate, 1);
    mixer.assign(0, Box::new(src));
    mixer.set_mode(0, PlayMode::PlayOnce);

    let mut frames = 0;
    let mut out = vec![0.0f32; (info.audio_sample_rate as usize / 30) * 2];
    let mut peak = 0f32;
    let dt = 1.0 / 30.0;
    for _ in 0..90 {
        if player.advance(dt).is_some() {
            frames += 1;
        }
        mixer.render(&mut out);
        peak = out.iter().fold(peak, |a, s| a.max(s.abs()));
    }
    // 3 seconds of wall clock: about 3 * frame_rate frames (some may be skipped, none duplicated).
    assert!(
        frames >= (2.5 * info.frame_rate.min(30.0)) as usize,
        "{frames} frames"
    );
    assert!(player.clock() > 2.9);
    assert!(peak > 0.0, "no audio reached the mixer output");
}

#[test]
fn every_shipped_movie_opens_and_yields_a_frame() {
    let Some(root) = std::env::var_os("MC2_DATA") else {
        return;
    };
    let dir = PathBuf::from(root).join("data/movies");
    if !dir.is_dir() {
        return;
    }
    let mut count = 0;
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.extension().is_none_or(|x| x != "mpg") {
            continue;
        }
        let mut m = Movie::open(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        let info = m.info();
        assert!(
            info.width > 0 && info.height > 0 && info.frame_rate > 0.0,
            "{}",
            p.display()
        );
        m.set_audio_enabled(false);
        let f = m
            .next_video_frame()
            .unwrap_or_else(|| panic!("{}: no frame", p.display()));
        assert_eq!(
            (f.width, f.height),
            (info.width, info.height),
            "{}",
            p.display()
        );
        count += 1;
    }
    eprintln!("{count} movies opened");
    assert!(count > 100);
}
