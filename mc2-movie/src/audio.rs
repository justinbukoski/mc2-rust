//! MPEG-1 Audio Layer II (MP2) frame parsing and decoding.
//!
//! The decoder follows ISO/IEC 11172-3:1993 clauses 2.4.2.3 and 2.4.3.3 and
//! Annex A synthesis steps. Numeric tables are in [`tables`].

use crate::bits::BitReader;
use crate::demux::{ElementaryStream, TimestampMark};
use crate::error::{MovieError, Result};

#[path = "audio/tables.rs"]
mod tables;

const SAMPLE_RATES: [u32; 3] = [44_100, 48_000, 32_000];
const LAYER2_BITRATES_KBPS: [u32; 14] = [
    32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
];
const MP2_SYNC_MASK: u16 = 0xFFE0;

/// The fields required to decode one MPEG-1 Layer II frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    pub sample_rate: u32,
    pub channels: usize,
    pub bitrate_kbps: u32,
    pub mode: u8,
    pub mode_extension: u8,
    pub has_crc: bool,
    pub frame_len: usize,
}

/// Interleaved decoded PCM and its stream timing.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioPcm {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: usize,
    /// PTS for the first PCM sample, when the demuxer found one.
    pub pts: Option<f64>,
    /// Number of decoded MP2 frames (384 PCM samples per channel each).
    pub frame_count: usize,
}

impl AudioPcm {
    /// Convert normalized samples to signed 16-bit PCM with saturation.
    pub fn to_i16(&self) -> Vec<i16> {
        self.samples
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0).round() as i16)
            .collect()
    }
}

/// MPEG-1 Layer II elementary stream decoder.
pub struct AudioDecoder {
    es: Vec<u8>,
    marks: Vec<TimestampMark>,
}

impl AudioDecoder {
    /// Construct a decoder from an audio elementary stream without PTS marks.
    pub fn new(es: Vec<u8>) -> Self {
        Self { es, marks: vec![] }
    }

    /// Construct a decoder from the demuxer's stream, preserving PES PTS marks.
    pub fn from_stream(es: ElementaryStream) -> Self {
        Self {
            es: es.data,
            marks: es.marks,
        }
    }

    /// Decode all complete Layer II frames to interleaved normalized `f32` PCM.
    pub fn decode(&self) -> Result<AudioPcm> {
        let mut offset = 0;
        let mut pcm = Vec::new();
        let mut header: Option<FrameHeader> = None;
        let mut synthesis = [Synthesis::new(), Synthesis::new()];
        let mut frames = 0usize;
        let mut first_pts = None;
        let mut mark_index = 0;

        while let Some(sync) = find_mp2_sync(&self.es, offset) {
            let h = parse_header(&self.es[sync..])?;
            if sync + h.frame_len > self.es.len() {
                return Err(MovieError::Truncated);
            }
            if let Some(old) = header {
                if old.sample_rate != h.sample_rate || old.channels != h.channels {
                    return Err(MovieError::Unsupported("midstream audio format change"));
                }
            } else {
                header = Some(h);
            }
            let frame = &self.es[sync..sync + h.frame_len];
            let payload = 4 + usize::from(h.has_crc) * 2;
            let mark_pts = latest_mark(&self.marks, &mut mark_index, sync);
            if frames == 0 {
                first_pts = mark_pts;
            }
            let decoded = decode_frame(frame, h, payload, &mut synthesis)?;
            pcm.extend(decoded);
            frames += 1;
            offset = sync + h.frame_len;
        }

        let h = header.ok_or(MovieError::NotMpeg)?;
        // Preserve the first frame's stream PTS when present. A later PES mark cannot stand in
        // for a missing start PTS because it applies to a later audio frame.
        Ok(AudioPcm {
            samples: pcm,
            sample_rate: h.sample_rate,
            channels: h.channels,
            pts: first_pts,
            frame_count: frames,
        })
    }

    /// Decode all frames and return only the interleaved normalized samples.
    pub fn decode_samples(&self) -> Result<Vec<f32>> {
        Ok(self.decode()?.samples)
    }

    /// Sample rate and channel count of the first valid Layer II frame header.
    pub fn read_headers(&self) -> Result<(u32, usize)> {
        let mut first_err = None;
        let mut from = 0;
        while let Some(sc) = find_mp2_sync(&self.es, from) {
            match parse_header(&self.es[sc..]) {
                Ok(h) => return Ok((h.sample_rate, h.channels)),
                Err(e) => {
                    first_err.get_or_insert(e);
                    from = sc + 1;
                }
            }
        }
        Err(first_err.unwrap_or(MovieError::NotMpeg))
    }
}

fn latest_mark(marks: &[TimestampMark], index: &mut usize, offset: usize) -> Option<f64> {
    while *index < marks.len() && marks[*index].offset <= offset {
        *index += 1;
    }
    index
        .checked_sub(1)
        .and_then(|i| marks.get(i))
        .map(|m| m.pts)
}

/// Parse a Layer II frame header at the start of `b`.
pub fn parse_header(b: &[u8]) -> Result<FrameHeader> {
    if b.len() < 4 {
        return Err(MovieError::Truncated);
    }
    let synch = u16::from_be_bytes([b[0], b[1]]);
    if synch & MP2_SYNC_MASK != MP2_SYNC_MASK {
        return Err(MovieError::Syntax("frame sync"));
    }
    if (b[1] >> 3) & 1 != 1 {
        return Err(MovieError::Unsupported("MPEG-2 audio"));
    }
    if (b[1] >> 1) & 3 != 2 {
        return Err(MovieError::Unsupported("audio layer other than II"));
    }
    let bitrate_index = usize::from(b[2] >> 4);
    let bitrate_kbps = *LAYER2_BITRATES_KBPS
        .get(
            bitrate_index
                .checked_sub(1)
                .ok_or(MovieError::Unsupported("free-format audio"))?,
        )
        .ok_or(MovieError::Syntax("bitrate_index"))?;
    let sample_index = usize::from((b[2] >> 2) & 3);
    let sample_rate = *SAMPLE_RATES
        .get(sample_index)
        .ok_or(MovieError::Syntax("sampling_frequency"))?;
    let padding = usize::from((b[2] >> 1) & 1);
    let mode = b[3] >> 6;
    let channels = if mode == 3 { 1 } else { 2 };
    let frame_len = 144 * (bitrate_kbps as usize * 1000) / sample_rate as usize + padding;
    Ok(FrameHeader {
        sample_rate,
        channels,
        bitrate_kbps,
        mode,
        mode_extension: (b[3] >> 4) & 3,
        has_crc: b[1] & 1 == 0,
        frame_len,
    })
}

fn find_mp2_sync(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < data.len() {
        if u16::from_be_bytes([data[i], data[i + 1]]) & MP2_SYNC_MASK == MP2_SYNC_MASK {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[derive(Clone, Copy)]
struct Synthesis {
    history: [f32; 1024],
}

impl Synthesis {
    fn new() -> Self {
        Self {
            history: [0.0; 1024],
        }
    }

    fn run(&mut self, subband: &[f32; 32], out: &mut [f32; 32]) {
        self.history.copy_within(0..960, 64);
        for i in 0..64 {
            let mut sum = 0.0f32;
            for (k, &x) in subband.iter().enumerate() {
                let angle = ((16 + i) * (2 * k + 1)) as f64 * std::f64::consts::PI / 64.0;
                sum += x * (angle.cos() as f32);
            }
            self.history[i] = sum;
        }
        for (j, sample) in out.iter_mut().enumerate() {
            let mut sum = 0.0f32;
            for block in 0..16 {
                let m = j + 32 * block;
                let i = m / 64;
                let col = m % 64;
                let v_index = if col < 32 {
                    128 * i + col
                } else {
                    128 * i + 96 + col - 32
                };
                sum += self.history[v_index] * tables::SYNTHESIS_WINDOW[m];
            }
            *sample = 2.0 * sum;
        }
    }
}

fn allocation_table(h: FrameHeader) -> (usize, usize) {
    let per_channel = if h.channels == 1 {
        h.bitrate_kbps
    } else {
        h.bitrate_kbps / 2
    };
    let index = match (h.sample_rate, per_channel) {
        (48_000, 32 | 48) | (44_100, 32 | 48) => 2,
        (32_000, 32 | 48) => 3,
        (48_000, 56..=192) | (44_100 | 32_000, 56..=80) => 0,
        _ => 1,
    };
    let sblimit = [27, 30, 8, 12][index];
    (index, sblimit)
}

fn class_for(steps: u16) -> Result<tables::QuantClass> {
    tables::QUANT_CLASSES
        .iter()
        .copied()
        .find(|class| class.steps == steps)
        .ok_or(MovieError::Syntax("quantization class"))
}

fn read_allocation(bits: &mut BitReader<'_>, table: usize, sb: usize, nbal: u8) -> Result<u16> {
    if nbal == 0 {
        return Ok(0);
    }
    let levels: &[u16] = match table {
        0 | 1 if sb < 3 => &tables::B2AB_ROW_0_TO_2,
        0 | 1 if sb < 11 => &tables::B2AB_ROW_3_TO_10,
        0 | 1 if nbal == 3 => &tables::B2AB_ROW_NBAL3,
        0 | 1 => &tables::B2AB_ROW_NBAL2,
        2 | 3 if sb < 2 => &tables::B2CD_ROW_0_TO_1,
        2 | 3 => &tables::B2CD_ROW_2_TO_7,
        _ => return Err(MovieError::Syntax("allocation table")),
    };
    levels
        .get(bits.read(u32::from(nbal)) as usize)
        .copied()
        .ok_or(MovieError::Syntax("allocation index"))
}

fn dequantize(code: u32, class: tables::QuantClass) -> f32 {
    let nbits = (u16::BITS - (class.steps - 1).leading_zeros()) as u8;
    let sign = 1u32 << (nbits - 1);
    let code = code ^ sign;
    let frac = if code & sign != 0 {
        -1.0 + (code & (sign - 1)) as f32 / sign as f32
    } else {
        code as f32 / sign as f32
    };
    class.c * (class.d + frac)
}

fn read_triplet(bits: &mut BitReader<'_>, class: tables::QuantClass) -> [f32; 3] {
    if class.grouped {
        let mut word = bits.read(u32::from(class.code_bits));
        let mut codes = [0; 3];
        for code in &mut codes {
            *code = word % u32::from(class.steps);
            word /= u32::from(class.steps);
        }
        codes.map(|code| dequantize(code, class))
    } else {
        std::array::from_fn(|_| dequantize(bits.read(u32::from(class.code_bits)), class))
    }
}

// The nested index loops follow ISO's subband/channel/sample bitstream order and address the
// decoder's fixed channel × subband arrays in that same order.
#[allow(clippy::needless_range_loop)]
fn decode_frame(
    frame: &[u8],
    h: FrameHeader,
    payload: usize,
    synthesis: &mut [Synthesis; 2],
) -> Result<Vec<f32>> {
    let (table_index, sblimit) = allocation_table(h);
    let joint_bound = if h.mode == 1 {
        (4 * (usize::from(h.mode_extension) + 1)).min(sblimit)
    } else {
        sblimit
    };
    let mut bits = BitReader::new(frame, payload);
    let mut alloc = [[0u16; 32]; 2];
    for sb in 0..sblimit {
        for ch in 0..if sb < joint_bound { h.channels } else { 1 } {
            let nbal = tables::ALLOCATION_NBAL[table_index][sb];
            alloc[ch][sb] = read_allocation(&mut bits, table_index, sb, nbal)?;
            if sb >= joint_bound {
                alloc[1][sb] = alloc[0][sb];
            }
        }
    }
    let mut scfsi = [[0u8; 32]; 2];
    for sb in 0..sblimit {
        for ch in 0..h.channels {
            if alloc[ch][sb] != 0 {
                scfsi[ch][sb] = bits.read(2) as u8;
            }
        }
    }
    let mut scale = [[[1.0f32; 3]; 32]; 2];
    for sb in 0..sblimit {
        for ch in 0..h.channels {
            if alloc[ch][sb] == 0 {
                continue;
            }
            let read_sf = |bits: &mut BitReader<'_>| tables::SCALE_FACTORS[bits.read(6) as usize];
            match scfsi[ch][sb] {
                0 => {
                    for factor in &mut scale[ch][sb] {
                        *factor = read_sf(&mut bits);
                    }
                }
                1 => {
                    scale[ch][sb][0] = read_sf(&mut bits);
                    scale[ch][sb][1] = scale[ch][sb][0];
                    scale[ch][sb][2] = read_sf(&mut bits);
                }
                2 => {
                    let sf = read_sf(&mut bits);
                    scale[ch][sb] = [sf; 3];
                }
                3 => {
                    scale[ch][sb][0] = read_sf(&mut bits);
                    scale[ch][sb][1] = read_sf(&mut bits);
                    scale[ch][sb][2] = scale[ch][sb][1];
                }
                _ => unreachable!(),
            }
        }
    }
    let mut pcm = Vec::with_capacity(1152 * h.channels);
    for group in 0..12 {
        let part = group / 4;
        let mut triplets = [[[0.0f32; 3]; 32]; 2];
        for sb in 0..sblimit {
            if alloc[0][sb] == 0 {
                continue;
            }
            if sb < joint_bound {
                for ch in 0..h.channels {
                    let class = class_for(alloc[ch][sb])?;
                    let triplet = read_triplet(&mut bits, class);
                    for (sample, value) in triplet.into_iter().enumerate() {
                        triplets[ch][sb][sample] = value * scale[ch][sb][part];
                    }
                }
            } else {
                let class = class_for(alloc[0][sb])?;
                let triplet = read_triplet(&mut bits, class);
                for ch in 0..h.channels {
                    for (sample, value) in triplet.into_iter().enumerate() {
                        triplets[ch][sb][sample] = value * scale[ch][sb][part];
                    }
                }
            }
        }
        for sample in 0..3 {
            let mut subband = [[0.0f32; 32]; 2];
            for ch in 0..h.channels {
                for sb in 0..sblimit {
                    subband[ch][sb] = triplets[ch][sb][sample];
                }
            }
            let mut output = [[0.0f32; 32]; 2];
            for ch in 0..h.channels {
                synthesis[ch].run(&subband[ch], &mut output[ch]);
            }
            for i in 0..32 {
                for ch_output in output.iter().take(h.channels) {
                    pcm.push(ch_output[i]);
                }
            }
        }
    }
    if bits.overrun() {
        return Err(MovieError::Truncated);
    }
    Ok(pcm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fields_cover_mono_and_joint_stereo() {
        assert_eq!(
            parse_header(&[0xFF, 0xFD, 0xA0, 0x40]),
            Ok(FrameHeader {
                sample_rate: 44_100,
                channels: 2,
                bitrate_kbps: 192,
                mode: 1,
                mode_extension: 0,
                has_crc: false,
                frame_len: 626,
            })
        );
        assert_eq!(
            parse_header(&[0xFF, 0xFD, 0xA2, 0xC0]),
            Ok(FrameHeader {
                sample_rate: 44_100,
                channels: 1,
                bitrate_kbps: 192,
                mode: 3,
                mode_extension: 0,
                has_crc: false,
                frame_len: 627,
            })
        );
    }

    #[test]
    fn reports_invalid_or_truncated_headers() {
        assert_eq!(
            parse_header(&[0xFF, 0xFD, 0x90]),
            Err(MovieError::Truncated)
        );
        assert_eq!(
            parse_header(&[0xFF, 0xFD, 0xF0, 0x40]),
            Err(MovieError::Syntax("bitrate_index"))
        );
        assert_eq!(
            parse_header(&[0xFF, 0xFD, 0x9C, 0x40]),
            Err(MovieError::Syntax("sampling_frequency"))
        );
        assert_eq!(
            AudioDecoder::new(vec![0, 0]).read_headers(),
            Err(MovieError::NotMpeg)
        );
    }

    #[test]
    fn tables_have_expected_sizes_values_and_window_symmetry() {
        assert_eq!(tables::SCALE_FACTORS.len(), 64);
        assert_eq!(tables::ALLOCATION_NBAL.len(), 4);
        assert_eq!(tables::SYNTHESIS_WINDOW.len(), 512);
        assert_eq!(tables::QUANT_CLASSES.len(), 17);
        assert_eq!(tables::SCALE_FACTORS[0], 1.0);
        assert_eq!(tables::SCALE_FACTORS[3], 0.5);
        assert!((tables::SCALE_FACTORS[1] - 0.793_700_5).abs() < 1e-8);
        assert_eq!(tables::B2AB_ROW_0_TO_2[2], 7);
        assert_eq!(tables::B2AB_ROW_3_TO_10[2], 5);
        assert_eq!(tables::B2AB_ROW_NBAL3[7], 65_535);
        assert_eq!(tables::B2CD_ROW_2_TO_7[3], 9);
        assert_eq!(
            tables::ALLOCATION_NBAL.map(|row| row.iter().map(|&n| n as usize).sum::<usize>()),
            [88, 94, 26, 38]
        );
        assert_eq!(
            tables::QUANT_CLASSES[0],
            tables::QuantClass {
                steps: 3,
                grouped: true,
                code_bits: 5,
                c: 4.0 / 3.0,
                d: 0.5,
            }
        );
        assert!(!tables::QUANT_CLASSES[2].grouped);
        assert_eq!(tables::QUANT_CLASSES[2].d, 0.25);
        assert_eq!(tables::QUANT_CLASSES[3].code_bits, 10);
        assert_eq!(tables::SYNTHESIS_WINDOW[0], 0.0);
        // Table B.3's alternating signed lobes have these exact mirrored values around the
        // synthesis-window midpoint; include both sign-preserving and sign-inverting pairs.
        for (left, right, sign) in [(1, 511, -1.0), (64, 448, 1.0), (128, 384, 1.0)] {
            assert!(
                (tables::SYNTHESIS_WINDOW[left] - sign * tables::SYNTHESIS_WINDOW[right]).abs()
                    < 1e-8
            );
        }
        assert!(tables::SYNTHESIS_WINDOW[256].abs() > 1.0);
    }

    #[test]
    fn grouped_and_ungrouped_samples_follow_their_quantizer_classes() {
        let mut grouped = BitReader::new(&[0b0010_1000], 0);
        assert_eq!(
            read_triplet(&mut grouped, class_for(3).unwrap()),
            [2.0 / 3.0, 0.0, -2.0 / 3.0]
        );

        // Three independently encoded 3-bit samples: 2, 5, and 6 in the 7-step class.
        let mut ungrouped = BitReader::new(&[0b0101_0111, 0], 0);
        let samples = read_triplet(&mut ungrouped, class_for(7).unwrap());
        let expected = [-2.0 / 7.0, 4.0 / 7.0, 6.0 / 7.0];
        for (actual, expected) in samples.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }
}
