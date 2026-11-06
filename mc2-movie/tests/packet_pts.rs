//! Presentation times when every packet carries its own PTS.
//!
//! The committed B-frame fixture is re-packetised here so that each PES packet starts exactly one
//! picture (packet `k` holds picture `k` in coding order) and carries a PTS chosen by this test.
//! The display-order times the decoder reports must then be the packet PTS of each picture, not
//! times anchored on the first packet. The expected values are worked out below from the schedule,
//! not read back from the decoder.

use mc2_movie::demux::demux;
use mc2_movie::video::{find_start_code, VideoDecoder};

const STREAM: &[u8] = include_bytes!("fixtures/bframes.mpg");
/// 90 kHz clock ticks per second.
const TICKS: u64 = 90_000;
/// Pictures in the fixture: one group of 12 at 25 frames per second.
const PICTURES: usize = 12;

/// One video PES packet with a 5-byte MPEG-1 PTS field and the given payload.
fn pes_with_pts(pts: u64, payload: &[u8]) -> Vec<u8> {
    let len = 5 + payload.len();
    let mut v = vec![0, 0, 1, 0xE0, (len >> 8) as u8, len as u8];
    v.extend([
        0x21 | (((pts >> 30) & 7) as u8) << 1,
        (pts >> 22) as u8,
        ((pts >> 15) as u8 & 0x7F) << 1 | 1,
        (pts >> 7) as u8,
        ((pts as u8) & 0x7F) << 1 | 1,
    ]);
    v.extend_from_slice(payload);
    v
}

/// Byte offsets of every picture start code in a video elementary stream.
fn picture_starts(es: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut p = 0;
    while let Some(sc) = find_start_code(es, p) {
        if es[sc + 3] == 0x00 {
            out.push(sc);
        }
        p = sc + 4;
    }
    out
}

/// The 10-bit temporal reference of the picture starting at `sc`.
fn temporal_reference(es: &[u8], sc: usize) -> usize {
    (usize::from(es[sc + 4]) << 2) | usize::from(es[sc + 5] >> 6)
}

#[test]
fn every_packet_pts_sets_its_picture_time() {
    let es = demux(STREAM).unwrap().video.expect("video stream").data;
    let starts = picture_starts(&es);
    assert_eq!(starts.len(), PICTURES);

    // Packet k spans from picture k's start code to picture k+1's. Packet 0 also takes the
    // sequence and group headers that precede the first picture.
    let mut bounds = vec![0];
    bounds.extend_from_slice(&starts[1..]);
    bounds.push(es.len());

    // Packet k is due at 2 s, plus 0.04 s (one 25 fps frame) per temporal reference, plus
    // 0.02 s per packet index. The 0.02 s term makes the packet times disagree with the
    // temporal references, so a decoder that anchors on the first packet cannot pass.
    let mut ps = Vec::new();
    let mut coded_tr = Vec::new();
    for (k, &sc) in starts.iter().enumerate() {
        let tr = temporal_reference(&es, sc);
        coded_tr.push(tr);
        let pts = 2 * TICKS + 3_600 * tr as u64 + 1_800 * k as u64;
        ps.extend(pes_with_pts(pts, &es[bounds[k]..bounds[k + 1]]));
    }
    // The single group numbers its pictures 0..12 in display order.
    let mut sorted = coded_tr.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        (0..PICTURES).collect::<Vec<_>>(),
        "temporal references"
    );

    // Display frame t is the picture with temporal reference t, which is packet k_t. Its time is
    // 2 + 0.04 t + 0.02 k_t seconds: the value of its own packet.
    let packet_of = |t: usize| coded_tr.iter().position(|&tr| tr == t).unwrap();
    let want: Vec<f64> = (0..PICTURES)
        .map(|t| 2.0 + 0.04 * t as f64 + 0.02 * packet_of(t) as f64)
        .collect();

    let video = demux(&ps).unwrap().video.expect("video stream");
    assert_eq!(
        video.data, es,
        "the re-packetised stream carries the same bytes"
    );
    assert_eq!(video.marks.len(), PICTURES, "one PTS mark per packet");
    let mut dec = VideoDecoder::new(video.data, video.marks);
    dec.set_strict(true);
    let mut times = Vec::new();
    while let Some(f) = dec.next_frame().expect("strict decode") {
        times.push(f.time);
    }
    assert_eq!(times.len(), PICTURES, "frame count");
    for (t, (got, want)) in times.iter().zip(&want).enumerate() {
        assert!(
            (got - want).abs() < 1e-9,
            "display frame {t}: time {got}, want {want}"
        );
    }
}
