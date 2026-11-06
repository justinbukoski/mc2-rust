//! Committed B-frame fixture, checked against an independent decoder.
//!
//! `fixtures/bframes.mpg` is a 12-picture MPEG-1 program stream (64x48, I B B P B B P B B P B P in
//! coding order I P B B P B B P B B P B, i.e. display order I B B P B B P B B P B P) written by
//! ffmpeg's `mpeg1video` encoder with `-bf 2`. `fixtures/bframes_ffmpeg_ref.yuv` is ffmpeg's own
//! decode of that file to planar YUV 4:2:0 in display order. ffmpeg is a reference only: none of
//! its source was read. The tolerance is the IDCT rounding difference between ffmpeg's integer
//! transform and the ideal transform of ISO/IEC 11172-2 Annex A: no sample differs by more than 2.

use mc2_movie::demux::demux;
use mc2_movie::video::{find_start_code, PictureType, VideoDecoder};
use mc2_movie::{Frame, MovieError};

const STREAM: &[u8] = include_bytes!("fixtures/bframes.mpg");
const REFERENCE: &[u8] = include_bytes!("fixtures/bframes_ffmpeg_ref.yuv");
const WIDTH: usize = 64;
const HEIGHT: usize = 48;
/// Bytes per frame in the reference: Y, then Cb, then Cr, each planar.
const FRAME_BYTES: usize = WIDTH * HEIGHT * 3 / 2;
/// Largest allowed difference for any sample, in 8-bit levels.
const MAX_DIFF: u8 = 2;

fn video_es(stream: &[u8]) -> Vec<u8> {
    demux(stream).unwrap().video.expect("video stream").data
}

/// The visible planes of a frame, packed without stride padding.
fn packed(frame: &Frame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
    for r in 0..h {
        out.extend_from_slice(&frame.y[r * frame.y_stride as usize..][..w]);
    }
    for plane in [&frame.cb, &frame.cr] {
        for r in 0..ch {
            out.extend_from_slice(&plane[r * frame.c_stride as usize..][..cw]);
        }
    }
    out
}

fn type_letter(t: PictureType) -> char {
    match t {
        PictureType::I => 'I',
        PictureType::P => 'P',
        PictureType::B => 'B',
    }
}

#[test]
fn b_frames_match_the_reference_in_display_order() {
    let mut dec = VideoDecoder::new(video_es(STREAM), Vec::new());
    dec.set_strict(true);
    let mut types = String::new();
    let mut frames = Vec::new();
    while let Some(f) = dec
        .next_frame()
        .expect("strict decode of the committed fixture")
    {
        types.push(type_letter(f.picture_type));
        frames.push(packed(&f));
    }
    assert_eq!(types, "IBBPBBPBBPBP", "display order of picture types");
    assert_eq!(frames.len() * FRAME_BYTES, REFERENCE.len(), "frame count");

    for (i, got) in frames.iter().enumerate() {
        let want = &REFERENCE[i * FRAME_BYTES..(i + 1) * FRAME_BYTES];
        let worst = got
            .iter()
            .zip(want)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            worst <= MAX_DIFF,
            "frame {i} ({}): largest sample difference {worst}",
            types.as_bytes()[i] as char
        );
    }
}

/// Display-order presentation times of the fixture, from ffprobe's `best_effort_timestamp_time`
/// (ffprobe gives none for the last picture; its 0.98 s is the 25 fps spacing from 0.54 s).
/// Several pictures of the one group carry their own PTS, and the P pictures have none.
const FRAME_TIMES: [f64; 12] = [
    0.54, 0.58, 0.62, 0.66, 0.70, 0.74, 0.78, 0.82, 0.86, 0.90, 0.94, 0.98,
];

#[test]
fn frame_times_follow_the_packet_pts() {
    let video = demux(STREAM).unwrap().video.expect("video stream");
    let mut dec = VideoDecoder::new(video.data, video.marks);
    dec.set_strict(true);
    let mut times = Vec::new();
    while let Some(f) = dec.next_frame().expect("strict decode of the fixture") {
        times.push(f.time);
    }
    assert_eq!(times.len(), FRAME_TIMES.len(), "frame count");
    for (i, (got, want)) in times.iter().zip(FRAME_TIMES).enumerate() {
        assert!(
            (got - want).abs() < 1e-6,
            "frame {i}: presentation time {got}, want {want}"
        );
    }
}

#[test]
fn truncated_stream_fails_loudly() {
    // Cut into the last pictures: either the demuxer or the picture decoder must report it.
    let cut = &STREAM[..STREAM.len() - 60];
    let Ok(d) = demux(cut) else {
        return; // demux error: the truncation was reported
    };
    let mut dec = VideoDecoder::new(d.video.expect("video stream").data, Vec::new());
    dec.set_strict(true);
    loop {
        match dec.next_frame() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("a truncated stream decoded to the end without an error"),
            Err(MovieError::Truncated | MovieError::Syntax(_)) => break,
            Err(e) => panic!("unexpected error {e:?}"),
        }
    }
}

/// The video stream with every slice of the first picture removed (its header is kept).
fn without_first_picture_slices(es: &[u8]) -> Vec<u8> {
    let mut starts = Vec::new();
    let mut p = 0;
    while let Some(s) = find_start_code(es, p) {
        starts.push(s);
        p = s + 4;
    }
    let first_pic = starts
        .iter()
        .copied()
        .find(|&s| es[s + 3] == 0x00)
        .expect("a picture");
    let first_slice = starts
        .iter()
        .copied()
        .find(|&s| s > first_pic && (0x01..=0xAF).contains(&es[s + 3]))
        .expect("a slice");
    let mut out = es[..first_slice].to_vec();
    let mut dropping = true;
    for (i, &s) in starts.iter().enumerate() {
        if s < first_slice {
            continue;
        }
        let end = starts.get(i + 1).copied().unwrap_or(es.len());
        let code = es[s + 3];
        if dropping && (0x01..=0xAF).contains(&code) {
            continue;
        }
        dropping = false;
        out.extend_from_slice(&es[s..end]);
    }
    out
}

#[test]
fn picture_with_missing_slices_is_an_error_in_strict_mode() {
    let es = without_first_picture_slices(&video_es(STREAM));

    let mut strict = VideoDecoder::new(es.clone(), Vec::new());
    strict.set_strict(true);
    assert!(
        strict.next_frame().is_err(),
        "a picture with no slices must fail in strict mode"
    );

    let mut lenient = VideoDecoder::new(es, Vec::new());
    let mut frames = 0;
    while lenient
        .next_frame()
        .expect("lenient decode conceals")
        .is_some()
    {
        frames += 1;
    }
    assert_eq!(frames, 12, "lenient mode still outputs every picture");
    assert!(lenient.concealed_errors >= 1, "the damage is counted");
}
