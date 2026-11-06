use mc2_movie::{demux::demux, AudioDecoder, VideoDecoder};
use std::process::Command;

const MOVIES: [&str; 3] = ["mc2_24d.mpg", "mc2_24e.mpg", "v1d.mpg"];

fn ffprobe_stream_info(path: &std::path::Path) -> (usize, u32) {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=channels,sample_rate",
            "-of",
            "csv=p=0:s=x",
        ])
        .arg(path)
        .output()
        .expect("ffprobe is required when MC2_DATA is set");
    assert!(
        output.status.success(),
        "ffprobe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let info = String::from_utf8(output.stdout).expect("ffprobe output is UTF-8");
    let (rate, channels) = info
        .trim()
        .split_once('x')
        .expect("sample rate and channels");
    (
        channels.parse().expect("ffprobe channel count"),
        rate.parse().expect("ffprobe sample rate"),
    )
}

fn ffmpeg_pcm(path: &std::path::Path, channels: usize, rate: u32) -> Vec<i16> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-f",
            "s16le",
            "-acodec",
            "pcm_s16le",
            "-ac",
        ])
        .arg(channels.to_string())
        .args(["-ar"])
        .arg(rate.to_string())
        .arg("pipe:1")
        .output()
        .expect("ffmpeg is installed");
    assert!(
        output.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (words, trailing) = output.stdout.as_chunks::<2>();
    assert!(trailing.is_empty());
    words.iter().map(|b| i16::from_le_bytes(*b)).collect()
}

#[test]
fn shipped_movies_match_ffmpeg_mp2_decode_and_preserve_pts() {
    let Ok(root) = std::env::var("MC2_DATA") else {
        eprintln!("MC2_DATA not set; skipping shipped movie audio comparison");
        return;
    };
    assert!(
        Command::new("ffmpeg").arg("-version").output().is_ok(),
        "ffmpeg is required when MC2_DATA is set; independent audio comparison did not run"
    );
    assert!(
        Command::new("ffprobe").arg("-version").output().is_ok(),
        "ffprobe is required when MC2_DATA is set; input stream properties were not verified"
    );
    let movie_dir = std::path::Path::new(&root).join("data/movies");
    for movie in MOVIES {
        let path = movie_dir.join(movie);
        let (reference_channels, reference_rate) = ffprobe_stream_info(&path);
        let bytes = std::fs::read(&path).expect("read shipped movie");
        let streams = demux(&bytes).expect("demux shipped movie");
        let audio_es = streams.audio.expect("movie audio stream");
        let video_es = streams.video.expect("movie video stream");
        let audio_start = audio_es.marks.first().expect("audio PTS mark").pts;
        let video_start = video_es.marks.first().expect("video PTS mark").pts;
        let mut video = VideoDecoder::new(video_es.data.clone(), video_es.marks.clone());
        let first_video_frame = video
            .next_frame()
            .expect("decode first video frame")
            .expect("video frame");
        let decoded = AudioDecoder::from_stream(audio_es)
            .decode()
            .expect("decode MP2");
        let actual = decoded.to_i16();
        assert_eq!(
            decoded.channels, reference_channels,
            "{movie}: ffprobe channels"
        );
        assert_eq!(
            decoded.sample_rate, reference_rate,
            "{movie}: ffprobe sample rate"
        );
        let reference = ffmpeg_pcm(&path, reference_channels, reference_rate);
        assert_eq!(actual.len(), reference.len(), "{movie}: sample length");
        assert_eq!(
            decoded.channels,
            if movie == "v1d.mpg" { 2 } else { 1 },
            "{movie}: channels"
        );
        assert_eq!(decoded.sample_rate, 44_100, "{movie}: sample rate");
        let (max_index, max_error) = actual
            .iter()
            .zip(&reference)
            .enumerate()
            .map(|(i, (&a, &b))| (i, (i32::from(a) - i32::from(b)).unsigned_abs()))
            .max_by_key(|&(_, e)| e)
            .unwrap_or((0, 0));
        assert!(
            max_error <= 2,
            "{movie}: max PCM error {max_error} LSB at sample {max_index}, tolerance is 2 LSB"
        );
        assert_eq!(decoded.pts, Some(audio_start), "{movie}: audio start PTS");
        assert!(
            (first_video_frame.time - video_start).abs() <= 1e-9,
            "{movie}: video first-frame PTS must preserve the stream's {video_start}, got {}",
            first_video_frame.time
        );
        let audio_video_skew = decoded.pts.unwrap() - first_video_frame.time;
        let stream_mark_skew = audio_start - video_start;
        assert!(
            (audio_video_skew - stream_mark_skew).abs() <= 1e-9,
            "{movie}: audio/video start skew {audio_video_skew:.9}s differs from stream-mark skew {stream_mark_skew:.9}s"
        );
        eprintln!(
            "{movie}: ffmpeg comparison ran: channels={reference_channels}, rate={reference_rate}, samples={}, max_error={max_error} LSB (limit 2), A/V skew={audio_video_skew:.9}s",
            actual.len()
        );
    }
}
