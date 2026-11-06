// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! RIFF/WAVE parsing and decoding (8/16-bit PCM and Microsoft ADPCM), in memory or streamed.
//!
//! These are the formats the shipped game data uses (see `data/sound/*.wav` and the packed
//! `sound.pak` samples): mono/stereo, 11025/22050/44100 Hz, PCM (tag 1) and MS-ADPCM (tag 2).
//! The Microsoft ADPCM algorithm is the published one (Microsoft RIFF/WAVE format registry,
//! `WAVE_FORMAT_ADPCM`).

use std::io::{self, Read, Seek, SeekFrom};

#[derive(Debug)]
pub enum WavError {
    NotRiff,
    NoFormat,
    NoData,
    Unsupported(String),
    Truncated,
    Io(io::Error),
}

impl std::fmt::Display for WavError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavError::NotRiff => write!(f, "not a RIFF/WAVE file"),
            WavError::NoFormat => write!(f, "missing fmt chunk"),
            WavError::NoData => write!(f, "missing data chunk"),
            WavError::Unsupported(s) => write!(f, "unsupported WAV: {s}"),
            WavError::Truncated => write!(f, "truncated WAV"),
            WavError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for WavError {}

impl From<io::Error> for WavError {
    fn from(e: io::Error) -> Self {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            WavError::Truncated
        } else {
            WavError::Io(e)
        }
    }
}

pub const FORMAT_PCM: u16 = 1;
pub const FORMAT_ADPCM: u16 = 2;

const STD_COEFS: [(i16, i16); 7] = [
    (256, 0),
    (512, -256),
    (0, 0),
    (192, 64),
    (240, 0),
    (460, -208),
    (392, -232),
];
const ADAPT_TABLE: [i32; 16] = [
    230, 230, 230, 230, 307, 409, 512, 614, 768, 614, 512, 409, 307, 230, 230, 230,
];

/// Decoded `fmt ` chunk.
#[derive(Clone, Debug, PartialEq)]
pub struct WavFormat {
    pub tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    /// ADPCM only: samples per channel in each block.
    pub samples_per_block: u16,
    /// ADPCM only: predictor coefficient pairs.
    pub coefs: Vec<(i16, i16)>,
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16le(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl WavFormat {
    fn parse(b: &[u8]) -> Result<Self, WavError> {
        if b.len() < 16 {
            return Err(WavError::Truncated);
        }
        let mut tag = u16le(b, 0);
        let channels = u16le(b, 2);
        let sample_rate = u32le(b, 4);
        let block_align = u16le(b, 12);
        let bits = u16le(b, 14);
        // WAVE_FORMAT_EXTENSIBLE: the real tag is the first two bytes of the sub-format GUID.
        if tag == 0xFFFE && b.len() >= 26 {
            tag = u16le(b, 24);
        }
        if channels == 0 || channels > 2 {
            return Err(WavError::Unsupported(format!("{channels} channels")));
        }
        if !super::mixer::sample_rate_is_valid(sample_rate) {
            return Err(WavError::Unsupported(format!(
                "{sample_rate} Hz sample rate"
            )));
        }
        let mut f = WavFormat {
            tag,
            channels,
            sample_rate,
            block_align,
            bits_per_sample: bits,
            samples_per_block: 0,
            coefs: Vec::new(),
        };
        match tag {
            FORMAT_PCM => {
                if bits != 8 && bits != 16 {
                    return Err(WavError::Unsupported(format!("{bits}-bit PCM")));
                }
                let frame = (bits / 8) * channels;
                if f.block_align != frame {
                    f.block_align = frame;
                }
            }
            FORMAT_ADPCM => {
                if bits != 4 {
                    return Err(WavError::Unsupported(format!("{bits}-bit ADPCM")));
                }
                let min_block = 7 * channels as usize;
                if (block_align as usize) < min_block {
                    return Err(WavError::Unsupported("ADPCM block too small".into()));
                }
                // Extra data: cbSize(2) samples_per_block(2) num_coef(2) coef pairs.
                let mut coefs = Vec::new();
                if b.len() >= 22 && u16le(b, 16) >= 4 {
                    f.samples_per_block = u16le(b, 18);
                    let n = u16le(b, 20) as usize;
                    for i in 0..n {
                        let o = 22 + i * 4;
                        if o + 4 <= b.len() {
                            coefs.push((i16le(b, o), i16le(b, o + 2)));
                        }
                    }
                }
                if coefs.len() < 7 {
                    coefs = STD_COEFS.to_vec();
                }
                f.coefs = coefs;
                if f.samples_per_block == 0 {
                    f.samples_per_block =
                        adpcm_samples_per_block(block_align as usize, channels as usize) as u16;
                }
            }
            other => return Err(WavError::Unsupported(format!("format tag {other}"))),
        }
        Ok(f)
    }

    /// Bytes of source data per decoded unit (a PCM frame group or an ADPCM block).
    fn unit_bytes(&self) -> usize {
        self.block_align as usize
    }
}

/// Samples per channel in a MS-ADPCM block of `block_align` bytes.
pub fn adpcm_samples_per_block(block_align: usize, channels: usize) -> usize {
    (block_align - 7 * channels) * 2 / channels + 2
}

/// Decode one MS-ADPCM block into interleaved i16 samples appended to `out`. A short final block
/// is decoded as far as its data goes.
pub fn decode_adpcm_block(fmt: &WavFormat, block: &[u8], out: &mut Vec<i16>) {
    let ch = fmt.channels as usize;
    if block.len() < 7 * ch {
        return;
    }
    let mut pred = [0usize; 2];
    // 64-bit state: a malformed block (huge delta, repeated large nibbles) must never overflow.
    // Well-formed streams never leave the 32-bit range, so results are unchanged for them.
    let mut delta = [0i64; 2];
    let mut s1 = [0i64; 2];
    let mut s2 = [0i64; 2];
    let mut o = 0;
    for p in pred.iter_mut().take(ch) {
        *p = (block[o] as usize).min(fmt.coefs.len() - 1);
        o += 1;
    }
    for d in delta.iter_mut().take(ch) {
        *d = i16le(block, o) as i64;
        o += 2;
    }
    for s in s1.iter_mut().take(ch) {
        *s = i16le(block, o) as i64;
        o += 2;
    }
    for s in s2.iter_mut().take(ch) {
        *s = i16le(block, o) as i64;
        o += 2;
    }
    // The two seed samples come out first, oldest (sample 2) first.
    out.extend(s2.iter().take(ch).map(|&v| v as i16));
    out.extend(s1.iter().take(ch).map(|&v| v as i16));
    let decode =
        |c: usize, nib: u8, s1: &mut [i64; 2], s2: &mut [i64; 2], delta: &mut [i64; 2]| -> i16 {
            let (c1, c2) = fmt.coefs[pred[c]];
            let predictor = (s1[c] * c1 as i64 + s2[c] * c2 as i64) >> 8;
            let signed = if nib >= 8 {
                nib as i64 - 16
            } else {
                nib as i64
            };
            let v = (predictor + signed * delta[c]).clamp(-32768, 32767);
            s2[c] = s1[c];
            s1[c] = v;
            // Cap the step so it stays far from i64 overflow however long a corrupt block runs;
            // anything past 2^24 already saturates every sample.
            delta[c] = ((ADAPT_TABLE[nib as usize] as i64 * delta[c]) >> 8).clamp(16, 1 << 24);
            v as i16
        };
    for &byte in &block[o..] {
        let hi = byte >> 4;
        let lo = byte & 0x0F;
        if ch == 1 {
            let a = decode(0, hi, &mut s1, &mut s2, &mut delta);
            out.push(a);
            let b = decode(0, lo, &mut s1, &mut s2, &mut delta);
            out.push(b);
        } else {
            // Stereo: high nibble is the left sample, low nibble the right one.
            let a = decode(0, hi, &mut s1, &mut s2, &mut delta);
            let b = decode(1, lo, &mut s1, &mut s2, &mut delta);
            out.push(a);
            out.push(b);
        }
    }
}

fn decode_pcm(fmt: &WavFormat, data: &[u8], out: &mut Vec<i16>) {
    if fmt.bits_per_sample == 8 {
        out.extend(data.iter().map(|&b| ((b as i16) - 128) << 8));
    } else {
        out.extend(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|c| i16::from_le_bytes(*c)),
        );
    }
}

/// A parsed WAV held in memory: format plus the raw `data` chunk bytes.
pub struct Wav<'a> {
    pub format: WavFormat,
    pub data: &'a [u8],
}

/// Find `fmt ` and `data` in a RIFF/WAVE image.
pub fn parse(bytes: &[u8]) -> Result<Wav<'_>, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotRiff);
    }
    let mut pos = 12;
    let mut format = None;
    let mut data = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32le(bytes, pos + 4) as usize;
        let body = pos + 8;
        let end = body.saturating_add(size).min(bytes.len());
        if id == b"fmt " && format.is_none() {
            format = Some(WavFormat::parse(&bytes[body..end])?);
        } else if id == b"data" && data.is_none() {
            data = Some(&bytes[body..end]);
        }
        if format.is_some() && data.is_some() {
            break;
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    match (format, data) {
        (Some(format), Some(data)) => Ok(Wav { format, data }),
        (None, _) => Err(WavError::NoFormat),
        (_, None) => Err(WavError::NoData),
    }
}

impl Wav<'_> {
    /// Decode everything to interleaved signed 16-bit samples.
    pub fn decode_i16(&self) -> Vec<i16> {
        let mut out = Vec::new();
        match self.format.tag {
            FORMAT_PCM => {
                let n = self.data.len() / self.format.unit_bytes() * self.format.unit_bytes();
                decode_pcm(&self.format, &self.data[..n], &mut out)
            }
            _ => {
                let bs = self.format.unit_bytes();
                for block in self.data.chunks(bs) {
                    decode_adpcm_block(&self.format, block, &mut out);
                }
            }
        }
        out
    }
}

/// A WAV decoded on demand from any `Read + Seek` (music files stay on disk).
pub struct WavStream<R: Read + Seek> {
    reader: R,
    format: WavFormat,
    data_start: u64,
    data_len: u64,
    consumed: u64,
    pending: Vec<i16>,
    pending_pos: usize,
    scratch: Vec<u8>,
}

impl<R: Read + Seek> WavStream<R> {
    pub fn new(mut reader: R) -> Result<Self, WavError> {
        reader.seek(SeekFrom::Start(0))?;
        let mut head = [0u8; 12];
        reader.read_exact(&mut head)?;
        if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
            return Err(WavError::NotRiff);
        }
        let mut format: Option<WavFormat> = None;
        // (payload start, declared size) of the first data chunk; it may precede `fmt `.
        let mut data: Option<(u64, u64)> = None;
        loop {
            if let (Some(format), Some((data_start, size))) = (&format, data) {
                let end = reader.seek(SeekFrom::End(0))?;
                let data_len = size.min(end.saturating_sub(data_start));
                reader.seek(SeekFrom::Start(data_start))?;
                return Ok(Self {
                    reader,
                    format: format.clone(),
                    data_start,
                    data_len,
                    consumed: 0,
                    pending: Vec::new(),
                    pending_pos: 0,
                    scratch: Vec::new(),
                });
            }
            let mut ch = [0u8; 8];
            if reader.read_exact(&mut ch).is_err() {
                return Err(if format.is_none() {
                    WavError::NoFormat
                } else {
                    WavError::NoData
                });
            }
            let size = u32le(&ch, 4) as u64;
            let padded = (size + (size & 1)) as i64;
            if &ch[0..4] == b"fmt " && format.is_none() {
                let mut body = vec![0u8; size.min(1024) as usize];
                reader.read_exact(&mut body)?;
                format = Some(WavFormat::parse(&body)?);
                let skip = size - body.len() as u64 + (size & 1);
                reader.seek(SeekFrom::Current(skip as i64))?;
            } else if &ch[0..4] == b"data" && data.is_none() {
                data = Some((reader.stream_position()?, size));
                reader.seek(SeekFrom::Current(padded))?;
            } else {
                reader.seek(SeekFrom::Current(padded))?;
            }
        }
    }

    pub fn format(&self) -> &WavFormat {
        &self.format
    }

    /// Total frames in the file (exact for PCM, block-rounded for ADPCM).
    pub fn total_frames(&self) -> u64 {
        let bs = self.format.unit_bytes() as u64;
        match self.format.tag {
            FORMAT_PCM => self.data_len / bs,
            _ => {
                let blocks = self.data_len.div_ceil(bs);
                blocks * self.format.samples_per_block as u64
            }
        }
    }

    fn refill(&mut self) -> bool {
        self.pending.clear();
        self.pending_pos = 0;
        let bs = self.format.unit_bytes();
        // PCM is read in larger chunks; ADPCM one block at a time.
        let want = if self.format.tag == FORMAT_PCM {
            bs * 1024
        } else {
            bs
        };
        let remaining = (self.data_len - self.consumed) as usize;
        let n = want.min(remaining);
        if n == 0 {
            return false;
        }
        self.scratch.resize(n, 0);
        let mut read = 0;
        while read < n {
            match self.reader.read(&mut self.scratch[read..n]) {
                Ok(0) => break,
                Ok(k) => read += k,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        self.consumed += n as u64;
        if read == 0 {
            // File shorter than its header claimed.
            self.consumed = self.data_len;
            return false;
        }
        let buf = &self.scratch[..read];
        if self.format.tag == FORMAT_PCM {
            let usable = read / bs * bs;
            decode_pcm(&self.format, &buf[..usable], &mut self.pending);
        } else {
            decode_adpcm_block(&self.format, buf, &mut self.pending);
        }
        !self.pending.is_empty()
    }

    /// Read decoded interleaved samples (whole frames only). Returns samples written.
    pub fn read_samples(&mut self, out: &mut [i16]) -> usize {
        let ch = self.format.channels as usize;
        let want = out.len() / ch * ch;
        let mut written = 0;
        while written < want {
            if self.pending_pos >= self.pending.len() && !self.refill() {
                break;
            }
            let avail = (self.pending.len() - self.pending_pos) / ch * ch;
            if avail == 0 {
                // Odd sample count in a block: drop the stray sample.
                self.pending_pos = self.pending.len();
                continue;
            }
            let n = avail.min(want - written);
            out[written..written + n]
                .copy_from_slice(&self.pending[self.pending_pos..self.pending_pos + n]);
            self.pending_pos += n;
            written += n;
        }
        written
    }

    pub fn rewind(&mut self) -> Result<(), WavError> {
        self.reader.seek(SeekFrom::Start(self.data_start))?;
        self.consumed = 0;
        self.pending.clear();
        self.pending_pos = 0;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    /// Build a PCM16 WAV image.
    pub fn pcm16_wav(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
        let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut v = Vec::new();
        v.extend_from_slice(b"RIFF");
        v.extend_from_slice(&((36 + data.len()) as u32).to_le_bytes());
        v.extend_from_slice(b"WAVEfmt ");
        v.extend_from_slice(&16u32.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&channels.to_le_bytes());
        v.extend_from_slice(&rate.to_le_bytes());
        v.extend_from_slice(&rate.wrapping_mul(2 * channels as u32).to_le_bytes());
        v.extend_from_slice(&(2 * channels).to_le_bytes());
        v.extend_from_slice(&16u16.to_le_bytes());
        v.extend_from_slice(b"data");
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&data);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;
    use std::io::Cursor;

    #[test]
    fn parses_pcm16_mono() {
        let w = pcm16_wav(1, 22050, &[0, 100, -100, 32767]);
        let wav = parse(&w).unwrap();
        assert_eq!(wav.format.channels, 1);
        assert_eq!(wav.format.sample_rate, 22050);
        assert_eq!(wav.decode_i16(), vec![0, 100, -100, 32767]);
    }

    #[test]
    fn pcm8_is_unsigned_and_scaled() {
        let mut w = pcm16_wav(1, 11025, &[0, 0]);
        // Rewrite as 8-bit: fmt bits=8, block_align=1, data 3 bytes.
        w.truncate(36);
        w[32..34].copy_from_slice(&1u16.to_le_bytes());
        w[34..36].copy_from_slice(&8u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&3u32.to_le_bytes());
        w.extend_from_slice(&[128, 255, 0]);
        let wav = parse(&w).unwrap();
        assert_eq!(wav.decode_i16(), vec![0, 127 << 8, -128 << 8]);
    }

    #[test]
    fn skips_unknown_chunks_and_pads_odd_sizes() {
        let base = pcm16_wav(2, 44100, &[1, 2, 3, 4]);
        let mut w = base[..12].to_vec();
        w.extend_from_slice(b"junk");
        w.extend_from_slice(&3u32.to_le_bytes());
        w.extend_from_slice(&[9, 9, 9, 0]); // odd size + pad byte
        w.extend_from_slice(&base[12..]);
        let wav = parse(&w).unwrap();
        assert_eq!(wav.decode_i16(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn data_chunk_before_fmt_chunk_is_accepted() {
        // The game's own parser takes the chunks in either order.
        let base = pcm16_wav(1, 22050, &[5, 6, 7]);
        let (fmt, data) = (&base[12..36], &base[36..]);
        let mut w = base[..12].to_vec();
        w.extend_from_slice(data);
        w.extend_from_slice(fmt);
        assert_eq!(parse(&w).unwrap().decode_i16(), vec![5, 6, 7]);
    }

    #[test]
    fn stream_accepts_data_chunk_before_fmt_chunk() {
        let base = pcm16_wav(1, 22050, &[5, 6, 7]);
        let (fmt, data) = (&base[12..36], &base[36..]);
        let mut w = base[..12].to_vec();
        w.extend_from_slice(data);
        w.extend_from_slice(fmt);
        let mut s = WavStream::new(Cursor::new(w)).unwrap();
        let mut out = [0i16; 8];
        assert_eq!(s.read_samples(&mut out), 3);
        assert_eq!(&out[..3], &[5, 6, 7]);
        // fmt but no data / data but no fmt still fail.
        assert!(matches!(
            WavStream::new(Cursor::new(pcm16_wav(1, 8000, &[1])[..36].to_vec())),
            Err(WavError::NoData)
        ));
    }

    #[test]
    fn adpcm_malformed_block_never_overflows() {
        // Worst case from review: delta 32767 and a run of 0x7 nibbles (step grows 3x per sample),
        // plus extreme seeds and coefficients.
        for (ch, delta, seed) in [
            (1u16, 32767i16, 32767i16),
            (1, 32767, -32768),
            (2, 32767, 32767),
        ] {
            let mut fmt = adpcm_fmt(ch);
            fmt.coefs = vec![(32767, 32767); 7];
            let mut block = vec![0u8; ch as usize];
            for _ in 0..ch {
                block.extend_from_slice(&delta.to_le_bytes());
            }
            for _ in 0..2 * ch {
                block.extend_from_slice(&seed.to_le_bytes());
            }
            block.extend(std::iter::repeat_n(0x77u8, 2000));
            block.extend(std::iter::repeat_n(0x88u8, 2000));
            let mut out = Vec::new();
            decode_adpcm_block(&fmt, &block, &mut out);
            assert_eq!(out.len(), 2 * ch as usize + 8000);
        }
    }

    #[test]
    fn rejects_absurd_sample_rates() {
        for rate in [0u32, 999, 192_001, u32::MAX] {
            let w = pcm16_wav(1, rate, &[1, 2, 3]);
            assert!(matches!(parse(&w), Err(WavError::Unsupported(_))), "{rate}");
            assert!(WavStream::new(Cursor::new(w)).is_err(), "{rate}");
        }
        for rate in [1000u32, 8000, 44100, 192_000] {
            assert!(parse(&pcm16_wav(1, rate, &[1])).is_ok(), "{rate}");
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(parse(b"nope"), Err(WavError::NotRiff)));
        let mut w = pcm16_wav(1, 8000, &[1]);
        w[20] = 0x55; // unsupported tag
        assert!(matches!(parse(&w), Err(WavError::Unsupported(_))));
        // data without fmt
        let mut bad = b"RIFF\0\0\0\0WAVEdata".to_vec();
        bad.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(parse(&bad), Err(WavError::NoFormat)));
        // fmt without data
        assert!(matches!(
            parse(&pcm16_wav(1, 8000, &[1])[..36]),
            Err(WavError::NoData)
        ));
    }

    fn adpcm_fmt(ch: u16) -> WavFormat {
        WavFormat {
            tag: FORMAT_ADPCM,
            channels: ch,
            sample_rate: 22050,
            block_align: 7 * ch,
            bits_per_sample: 4,
            samples_per_block: 2,
            coefs: STD_COEFS.to_vec(),
        }
    }

    #[test]
    fn adpcm_mono_hand_computed() {
        // predictor 0 -> (256, 0); delta 16; s1 = 100; s2 = 50.
        let fmt = adpcm_fmt(1);
        let mut block = vec![0u8];
        block.extend_from_slice(&16i16.to_le_bytes());
        block.extend_from_slice(&100i16.to_le_bytes());
        block.extend_from_slice(&50i16.to_le_bytes());
        block.push(0x2F); // nibbles 2, 15 (-1)
        let mut out = Vec::new();
        decode_adpcm_block(&fmt, &block, &mut out);
        // seeds: s2 then s1.
        assert_eq!(&out[..2], &[50, 100]);
        // nibble 2: pred = (100*256 + 50*0)>>8 = 100; +2*16 = 132; delta = 307.. table[2]=230 -> 230*16>>8 = 14 -> max 16.
        assert_eq!(out[2], 132);
        // nibble -1: pred = (132*256)>>8 = 132; + (-1*16) = 116.
        assert_eq!(out[3], 116);
    }

    #[test]
    fn adpcm_delta_adapts_up() {
        let fmt = adpcm_fmt(1);
        let mut block = vec![0u8];
        block.extend_from_slice(&256i16.to_le_bytes());
        block.extend_from_slice(&0i16.to_le_bytes());
        block.extend_from_slice(&0i16.to_le_bytes());
        block.push(0x77); // 7 then 7: delta grows by 614/256 each time.
        let mut out = Vec::new();
        decode_adpcm_block(&fmt, &block, &mut out);
        assert_eq!(out[2], 7 * 256);
        let d2 = (614 * 256) >> 8; // 614
        assert_eq!(out[3], (7 * 256 + 7 * d2).min(32767) as i16);
    }

    #[test]
    fn adpcm_stereo_nibble_order_and_clamp() {
        let fmt = adpcm_fmt(2);
        let mut block = vec![0u8, 0u8]; // predictors
        block.extend_from_slice(&1000i16.to_le_bytes()); // delta L
        block.extend_from_slice(&1000i16.to_le_bytes()); // delta R
        block.extend_from_slice(&0i16.to_le_bytes()); // s1 L
        block.extend_from_slice(&0i16.to_le_bytes()); // s1 R
        block.extend_from_slice(&0i16.to_le_bytes()); // s2 L
        block.extend_from_slice(&0i16.to_le_bytes()); // s2 R
        block.push(0x78); // L nibble 7, R nibble 8 (-8)
        let mut out = Vec::new();
        decode_adpcm_block(&fmt, &block, &mut out);
        assert_eq!(out.len(), 6);
        assert_eq!(&out[..4], &[0, 0, 0, 0]);
        assert_eq!(out[4], 7000);
        assert_eq!(out[5], -8000);
        // Clamp: huge delta saturates.
        let mut block2 = block.clone();
        block2[2..4].copy_from_slice(&30000i16.to_le_bytes());
        let mut out2 = Vec::new();
        decode_adpcm_block(&fmt, &block2, &mut out2);
        assert_eq!(out2[4], 32767);
    }

    #[test]
    fn samples_per_block_formula() {
        // Standard 1024-byte mono block: 2036 samples.
        assert_eq!(adpcm_samples_per_block(1024, 1), 2036);
        assert_eq!(adpcm_samples_per_block(2048, 2), 2036);
    }

    #[test]
    fn stream_matches_in_memory_decode_across_odd_reads() {
        let samples: Vec<i16> = (0..5000).map(|i| ((i * 37) % 2000 - 1000) as i16).collect();
        let w = pcm16_wav(2, 44100, &samples);
        let expect = parse(&w).unwrap().decode_i16();
        let mut s = WavStream::new(Cursor::new(w)).unwrap();
        assert_eq!(s.total_frames(), 2500);
        let mut got = Vec::new();
        let mut buf = [0i16; 770];
        loop {
            let n = s.read_samples(&mut buf);
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        assert_eq!(got, expect);
        s.rewind().unwrap();
        let mut again = vec![0i16; 10];
        assert_eq!(s.read_samples(&mut again), 10);
        assert_eq!(&again[..], &expect[..10]);
    }
}
