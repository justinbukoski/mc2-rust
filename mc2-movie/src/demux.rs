//! MPEG-1 program stream (ISO/IEC 11172-1) demultiplexer.
//!
//! Splits a program stream into one video and one audio elementary stream and keeps the
//! presentation time stamps (PTS) the packet headers carry, as marks keyed by byte offset into
//! the elementary stream. The first video stream (id `0xE0..=0xEF`) and the first audio stream
//! (id `0xC0..=0xDF`) found are kept; others are ignored. MPEG-2 style PES headers and pack
//! headers are accepted too, since some encoders write them into otherwise MPEG-1 files. A bare
//! video elementary stream (starting with a sequence header) is passed through unchanged.
//!
//! Truncation is an error, never a shorter stream: a packet whose declared length runs past the
//! end of the file, a header cut off by the end of the file, or a start code cut off after its
//! first three bytes fails with [`MovieError::Truncated`]. An MPEG-2 PES header that sets the PTS
//! flag but is too short to hold the field fails with a syntax error.

use crate::error::{MovieError, Result};

/// 90 kHz system clock used by PTS/DTS values.
const CLOCK_HZ: f64 = 90_000.0;

/// A time stamp that applies to the payload byte at `offset` of an elementary stream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimestampMark {
    /// Offset into [`ElementaryStream::data`] of the first payload byte of the carrying packet.
    pub offset: usize,
    /// Presentation time stamp in seconds.
    pub pts: f64,
}

/// A reassembled elementary stream.
#[derive(Clone, Debug, Default)]
pub struct ElementaryStream {
    /// PES stream id (0 for a bare elementary stream).
    pub id: u8,
    pub data: Vec<u8>,
    /// PTS marks in increasing offset order.
    pub marks: Vec<TimestampMark>,
}

/// Result of demultiplexing a whole file.
#[derive(Clone, Debug, Default)]
pub struct Demuxed {
    pub video: Option<ElementaryStream>,
    pub audio: Option<ElementaryStream>,
}

fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 < data.len() {
        // Skip quickly: a start code needs a 1 at i+2 and zeros before it.
        if data[i + 2] > 1 {
            i += 3;
        } else if data[i] != 0 {
            i += 1;
        } else if data[i + 1] != 0 {
            i += 2;
        } else if data[i + 2] == 1 {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

/// True when the last three bytes of the file are `00 00 01`: a start code cut off by the end of
/// the file, at or after `from`.
fn ends_in_partial_start_code(data: &[u8], from: usize) -> bool {
    data.len() >= from + 3 && data[data.len() - 3..] == [0, 0, 1]
}

fn read_timestamp(b: &[u8]) -> u64 {
    (u64::from(b[0] >> 1) & 7) << 30
        | u64::from(b[1]) << 22
        | u64::from(b[2] >> 1) << 15
        | u64::from(b[3]) << 7
        | u64::from(b[4] >> 1)
}

/// Parse a PES packet body (the bytes after the 16-bit length). Returns the payload start
/// within `body` and the PTS in seconds, if any. `Ok(None)` means the body holds no payload
/// (empty, or nothing but stuffing bytes).
fn parse_pes_header(body: &[u8]) -> Result<Option<(usize, Option<f64>)>> {
    let Some(&first) = body.first() else {
        return Ok(None);
    };
    if first >> 6 == 0b10 {
        // MPEG-2 PES header: flags, header length, optional fields.
        if body.len() < 3 {
            return Err(MovieError::Truncated);
        }
        let flags = body[1];
        let hlen = usize::from(body[2]);
        let payload = 3 + hlen;
        if payload > body.len() {
            return Err(MovieError::Truncated);
        }
        let pts = if flags & 0x80 != 0 {
            // The PTS flag promises five timestamp bytes inside the header; the DTS flag (0xC0)
            // adds five more after them.
            let needed = if flags & 0x40 != 0 { 10 } else { 5 };
            if hlen < needed {
                return Err(MovieError::Syntax("PES PTS/DTS field length"));
            }
            Some(read_timestamp(&body[3..8]) as f64 / CLOCK_HZ)
        } else {
            None
        };
        return Ok(Some((payload, pts)));
    }
    let mut i = 0;
    while i < body.len() && body[i] == 0xFF {
        i += 1;
    }
    if i >= body.len() {
        return Ok(None);
    }
    if body[i] >> 6 == 0b01 {
        i += 2; // STD buffer scale and size
    }
    let b = *body.get(i).ok_or(MovieError::Truncated)?;
    match b >> 4 {
        0b0010 => {
            let ts = body.get(i..i + 5).ok_or(MovieError::Truncated)?;
            let pts = Some(read_timestamp(ts) as f64 / CLOCK_HZ);
            Ok(Some((i + 5, pts)))
        }
        0b0011 => {
            let ts = body.get(i..i + 10).ok_or(MovieError::Truncated)?;
            let pts = Some(read_timestamp(&ts[..5]) as f64 / CLOCK_HZ);
            Ok(Some((i + 10, pts)))
        }
        _ if b == 0x0F => Ok(Some((i + 1, None))),
        _ => Err(MovieError::Syntax("PES timestamp field")),
    }
}

/// Demultiplex a complete MPEG-1 program stream held in memory.
pub fn demux(data: &[u8]) -> Result<Demuxed> {
    let first = find_start_code(data, 0).ok_or(MovieError::NotMpeg)?;
    if data[first + 3] == 0xB3 {
        // Bare video elementary stream.
        return Ok(Demuxed {
            video: Some(ElementaryStream {
                id: 0,
                data: data[first..].to_vec(),
                marks: Vec::new(),
            }),
            audio: None,
        });
    }
    let mut video: Option<ElementaryStream> = None;
    let mut audio: Option<ElementaryStream> = None;
    let mut p = first;
    loop {
        let Some(i) = find_start_code(data, p) else {
            if ends_in_partial_start_code(data, p) {
                return Err(MovieError::Truncated);
            }
            break;
        };
        let id = data[i + 3];
        p = i + 4;
        match id {
            0xB9 => break,
            0xBA => {
                // MPEG-1 pack headers are 8 bytes; MPEG-2 ones are 10 bytes plus stuffing.
                let fixed = match data.get(p) {
                    Some(b) if b >> 6 == 0b01 => 10,
                    Some(_) => 8,
                    None => return Err(MovieError::Truncated),
                };
                let head = data.get(p..p + fixed).ok_or(MovieError::Truncated)?;
                let stuffing = if fixed == 10 {
                    usize::from(head[9] & 7)
                } else {
                    0
                };
                p += fixed + stuffing;
                if p > data.len() {
                    return Err(MovieError::Truncated);
                }
            }
            0xBB..=0xFF => {
                let lb = data.get(p..p + 2).ok_or(MovieError::Truncated)?;
                let len = usize::from(lb[0]) << 8 | usize::from(lb[1]);
                let body_start = p + 2;
                let body_end = if len == 0 {
                    // Unbounded packet (video only in practice): runs to the next system start code.
                    let mut q = body_start;
                    loop {
                        match find_start_code(data, q) {
                            Some(s) if data[s + 3] >= 0xB9 => break s,
                            Some(s) => q = s + 3,
                            None if ends_in_partial_start_code(data, q) => {
                                return Err(MovieError::Truncated);
                            }
                            None => break data.len(),
                        }
                    }
                } else {
                    let end = body_start + len;
                    if end > data.len() {
                        return Err(MovieError::Truncated);
                    }
                    end
                };
                p = body_end;
                let slot = match id {
                    0xE0..=0xEF => &mut video,
                    0xC0..=0xDF => &mut audio,
                    _ => continue,
                };
                if slot.as_ref().is_some_and(|s| s.id != id) {
                    continue; // a second stream of the same kind: ignored
                }
                let body = &data[body_start..body_end];
                let Some((payload_at, pts)) = parse_pes_header(body)? else {
                    continue;
                };
                let es = slot.get_or_insert_with(|| ElementaryStream {
                    id,
                    ..Default::default()
                });
                if let Some(pts) = pts {
                    es.marks.push(TimestampMark {
                        offset: es.data.len(),
                        pts,
                    });
                }
                es.data.extend_from_slice(&body[payload_at..]);
            }
            _ => {}
        }
    }
    if video.is_none() && audio.is_none() {
        return Err(MovieError::NotMpeg);
    }
    Ok(Demuxed { video, audio })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pes(id: u8, header: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut v = vec![0, 0, 1, id];
        let len = header.len() + payload.len();
        v.push((len >> 8) as u8);
        v.push(len as u8);
        v.extend_from_slice(header);
        v.extend_from_slice(payload);
        v
    }

    #[test]
    fn splits_streams_and_reads_pts() {
        let mut d = vec![0, 0, 1, 0xBA, 0x21, 0, 1, 0, 1, 0xC3, 0x33, 0x67];
        // Video packet with PTS = 48750 (the value used by the shipped movies' first packet).
        d.extend(pes(
            0xE0,
            &[0xFF, 0x21, 0x00, 0x03, 0x7C, 0xDD],
            &[0, 0, 1, 0xB3, 1, 2, 3],
        ));
        d.extend(pes(0xC0, &[0x0F], &[0xFF, 0xFD, 9, 9]));
        d.extend(pes(0xE0, &[0x0F], &[4, 5]));
        d.extend([0, 0, 1, 0xB9]);
        let m = demux(&d).unwrap();
        let v = m.video.unwrap();
        assert_eq!(v.data, [0, 0, 1, 0xB3, 1, 2, 3, 4, 5]);
        assert_eq!(v.marks.len(), 1);
        assert!((v.marks[0].pts - 48750.0 / 90000.0).abs() < 1e-9);
        let a = m.audio.unwrap();
        assert_eq!(a.data, [0xFF, 0xFD, 9, 9]);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(demux(b"hello world").unwrap_err(), MovieError::NotMpeg);
        assert_eq!(demux(&[]).unwrap_err(), MovieError::NotMpeg);
    }

    #[test]
    fn packet_longer_than_the_file_is_truncated() {
        let mut d = pes(0xE0, &[0x0F], &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        // Declare 4 more bytes than the file holds.
        let len = u16::from_be_bytes([d[4], d[5]]) + 4;
        d[4..6].copy_from_slice(&len.to_be_bytes());
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
    }

    #[test]
    fn packet_header_cut_off_by_the_end_is_truncated() {
        // A PTS-only header declares 5 timestamp bytes but the body holds only 2 of them.
        let d = pes(0xE0, &[0x21, 0x00], &[]);
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
    }

    #[test]
    fn partial_trailing_pack_header_is_truncated() {
        // A complete video packet, then a pack start code whose 8 header bytes are cut off after 1.
        let mut d = pes(0xE0, &[0x0F], &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        d.extend([0, 0, 1, 0xBA, 0x21]);
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
        // MPEG-2 pack headers need 10 bytes.
        let mut d = pes(0xE0, &[0x0F], &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        d.extend([0, 0, 1, 0xBA, 0x44, 0, 1, 0, 1, 0xC3]);
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
    }

    #[test]
    fn packet_length_field_cut_off_by_the_end_is_truncated() {
        let mut d = vec![0, 0, 1, 0xE0, 0x00];
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
        d.clear();
        d.extend([0, 0, 1, 0xBA]);
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
    }

    #[test]
    fn malformed_timestamp_field_is_an_error() {
        // 0x00 is neither a PTS, a PTS+DTS nor the no-timestamp marker.
        let d = pes(0xE0, &[0x00], &[0, 0, 1, 0xB3]);
        assert_eq!(
            demux(&d).unwrap_err(),
            MovieError::Syntax("PES timestamp field")
        );
    }

    #[test]
    fn partial_trailing_start_code_is_truncated() {
        // A complete video packet, then the first three bytes of a start code and nothing else.
        let mut d = pes(0xE0, &[0x0F], &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        d.extend([0, 0, 1]);
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
        // The same inside an unbounded (length 0) video packet, which runs to the next start code.
        let d = [
            0, 0, 1, 0xE0, 0, 0, 0x0F, 0, 0, 1, 0xB3, 1, 2, 3, 4, 0, 0, 1,
        ];
        assert_eq!(demux(&d).unwrap_err(), MovieError::Truncated);
    }

    #[test]
    fn complete_trailing_start_code_is_not_a_partial_one() {
        // A four-byte start code at the end is a real (empty) system packet, not a truncation.
        let mut d = pes(0xE0, &[0x0F], &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        d.extend([0, 0, 1, 0xB9]);
        assert!(demux(&d).unwrap().video.is_some());
    }

    #[test]
    fn mpeg2_pts_flag_with_short_header_is_an_error() {
        // Flags 0x80 promise a PTS, but the header length 4 leaves only 4 bytes for it.
        let d = pes(0xE0, &[0x80, 0x80, 0x04, 1, 2, 3, 4], &[0, 0, 1, 0xB3]);
        assert_eq!(
            demux(&d).unwrap_err(),
            MovieError::Syntax("PES PTS/DTS field length")
        );
    }

    #[test]
    fn mpeg2_pts_and_dts_flags_with_only_a_pts_is_an_error() {
        // Flags 0xC0 promise a PTS and a DTS (10 header bytes), but the header length 5 holds
        // only the PTS, so the DTS field is missing.
        let header = [0x80, 0xC0, 0x05, 0x21, 0x00, 0x05, 0xBF, 0x21];
        let d = pes(0xE0, &header, &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        assert_eq!(
            demux(&d).unwrap_err(),
            MovieError::Syntax("PES PTS/DTS field length")
        );
    }

    #[test]
    fn mpeg2_pts_with_full_header_is_read() {
        // The same packet with a header length of 5 carries the 5-byte PTS 90000 (1 s).
        let pts = [0x21, 0x00, 0x05, 0xBF, 0x21];
        let mut header = vec![0x80, 0x80, 0x05];
        header.extend(pts);
        let d = pes(0xE0, &header, &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        let v = demux(&d).unwrap().video.unwrap();
        assert_eq!(v.marks.len(), 1);
        assert!((v.marks[0].pts - 1.0).abs() < 1e-9);
        assert_eq!(v.data, [0, 0, 1, 0xB3, 1, 2, 3, 4]);
    }

    #[test]
    fn mpeg2_pts_and_dts_with_full_header_is_read() {
        // Header length 10: the PTS 90000 (1 s), then a DTS field that is skipped, not read.
        let pts = [0x21, 0x00, 0x05, 0xBF, 0x21];
        let dts = [0x11, 0x00, 0x05, 0xBF, 0x11];
        let mut header = vec![0x80, 0xC0, 0x0A];
        header.extend(pts);
        header.extend(dts);
        let d = pes(0xE0, &header, &[0, 0, 1, 0xB3, 1, 2, 3, 4]);
        let v = demux(&d).unwrap().video.unwrap();
        assert_eq!(v.marks.len(), 1);
        assert!((v.marks[0].pts - 1.0).abs() < 1e-9);
        assert_eq!(v.data, [0, 0, 1, 0xB3, 1, 2, 3, 4]);
    }
}
