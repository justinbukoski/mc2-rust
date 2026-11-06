//! MPEG-1 video decoder (ISO/IEC 11172-2): I, P and B pictures, half-pel motion compensation,
//! frame reordering, planar YCbCr 4:2:0 output.
//!
//! The decoder works on a complete in-memory video elementary stream plus the PTS marks the
//! demuxer collected. Pictures come out in display order with presentation times in seconds
//! of the stream clock.

#![allow(clippy::needless_range_loop, unused_assignments)]

mod idct;
mod picture;
pub mod tables;

use crate::bits::BitReader;
use crate::demux::TimestampMark;
use crate::error::{MovieError, Result};
use idct::idct8x8;
use picture::{McScratch, Picture};
use std::collections::VecDeque;
use tables::*;

pub use picture::Frame;

/// Picture coding type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PictureType {
    I,
    P,
    B,
}

/// Parameters from the sequence header.
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceInfo {
    pub width: u32,
    pub height: u32,
    pub frame_rate: f64,
    pub aspect_code: u8,
    /// Bits per second, 0 for variable bit rate (0x3FFFF).
    pub bit_rate: u32,
}

#[derive(Clone)]
struct Sequence {
    info: SequenceInfo,
    mb_w: usize,
    mb_h: usize,
    intra_matrix: [u8; 64],
    non_intra_matrix: [u8; 64],
}

/// Find the next `00 00 01 xx` start code at or after `from`; returns the offset of the `00`.
pub fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 < data.len() {
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

/// Decoder over a whole video elementary stream.
pub struct VideoDecoder {
    es: Vec<u8>,
    marks: Vec<TimestampMark>,
    last_used_mark: Option<usize>,
    pos: usize,
    seq: Option<Sequence>,
    past: Option<Picture>,
    future: Option<Picture>,
    spare: Option<Picture>,
    scratch: McScratch,
    ready: VecDeque<Frame>,
    finished: bool,
    // Timing.
    anchor: Option<(f64, i32)>,
    gop_base: f64,
    max_time: f64,
    pictures: u64,
    /// Number of damaged slices that were concealed instead of reported.
    pub concealed_errors: u32,
    strict: bool,
}

impl VideoDecoder {
    /// `es` is the video elementary stream; `marks` its PTS marks (may be empty).
    pub fn new(es: Vec<u8>, marks: Vec<TimestampMark>) -> Self {
        Self {
            es,
            marks,
            last_used_mark: None,
            pos: 0,
            seq: None,
            past: None,
            future: None,
            spare: None,
            scratch: McScratch::default(),
            ready: VecDeque::new(),
            finished: false,
            anchor: None,
            gop_base: 0.0,
            max_time: 0.0,
            pictures: 0,
            concealed_errors: 0,
            strict: false,
        }
    }

    /// In strict mode a damaged slice is an error; otherwise it is concealed and counted.
    pub fn set_strict(&mut self, strict: bool) {
        self.strict = strict;
    }

    /// The sequence parameters, once a sequence header has been read.
    pub fn sequence(&self) -> Option<&SequenceInfo> {
        self.seq.as_ref().map(|s| &s.info)
    }

    /// Read headers up to the first picture so [`sequence`](Self::sequence) is available.
    pub fn read_headers(&mut self) -> Result<SequenceInfo> {
        while self.seq.is_none() {
            let Some(sc) = find_start_code(&self.es, self.pos) else {
                return Err(MovieError::NotMpeg);
            };
            self.pos = sc + 4;
            if self.es[sc + 3] == 0xB3 {
                self.parse_sequence_header()?;
            }
        }
        Ok(self.seq.as_ref().unwrap().info.clone())
    }

    /// Decode the next picture in display order; `None` at the end of the stream.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        loop {
            if let Some(f) = self.ready.pop_front() {
                return Ok(Some(f));
            }
            if self.finished {
                return Ok(None);
            }
            self.advance()?;
        }
    }

    /// Number of coded pictures in the stream (counts picture start codes; cheap).
    pub fn count_pictures(&self) -> u64 {
        let mut n = 0;
        let mut p = 0;
        while let Some(sc) = find_start_code(&self.es, p) {
            if self.es[sc + 3] == 0x00 {
                n += 1;
            }
            p = sc + 4;
        }
        n
    }

    fn advance(&mut self) -> Result<()> {
        loop {
            let Some(sc) = find_start_code(&self.es, self.pos) else {
                self.flush();
                return Ok(());
            };
            match self.es[sc + 3] {
                0xB3 => {
                    self.pos = sc + 4;
                    self.parse_sequence_header()?;
                }
                0xB8 => {
                    self.pos = sc + 4;
                    self.parse_gop_header();
                }
                0x00 => return self.decode_picture(sc),
                _ => self.pos = sc + 4,
            }
        }
    }

    /// End of stream: the last anchor picture is still waiting for display.
    fn flush(&mut self) {
        if let Some(p) = self.future.take() {
            self.ready.push_back(self.frame_of(&p));
        }
        self.finished = true;
    }

    fn parse_sequence_header(&mut self) -> Result<()> {
        let mut br = BitReader::new(&self.es, self.pos);
        let width = br.read(12);
        let height = br.read(12);
        let aspect_code = br.read(4) as u8;
        let rate_code = br.read(4) as usize;
        let bit_rate_units = br.read(18);
        if !br.read_bit() {
            return Err(MovieError::Syntax("sequence header marker bit"));
        }
        br.skip(10); // vbv_buffer_size
        br.skip(1); // constrained_parameters_flag
        let mut intra_matrix = DEFAULT_INTRA_MATRIX;
        let mut non_intra_matrix = [16u8; 64];
        if br.read_bit() {
            for &z in &ZIGZAG {
                intra_matrix[usize::from(z)] = br.read(8) as u8;
            }
        }
        if br.read_bit() {
            for &z in &ZIGZAG {
                non_intra_matrix[usize::from(z)] = br.read(8) as u8;
            }
        }
        if br.overrun() {
            return Err(MovieError::Truncated);
        }
        if width == 0 || height == 0 {
            return Err(MovieError::Syntax("zero picture size"));
        }
        if !(1..=8).contains(&rate_code) {
            return Err(MovieError::Unsupported("frame rate code"));
        }
        if intra_matrix.contains(&0) || non_intra_matrix.contains(&0) {
            return Err(MovieError::Syntax("zero quantiser matrix entry"));
        }
        let info = SequenceInfo {
            width,
            height,
            frame_rate: FRAME_RATES[rate_code],
            aspect_code,
            bit_rate: if bit_rate_units == 0x3FFFF {
                0
            } else {
                bit_rate_units * 400
            },
        };
        let seq = Sequence {
            mb_w: (width as usize).div_ceil(16),
            mb_h: (height as usize).div_ceil(16),
            info,
            intra_matrix,
            non_intra_matrix,
        };
        let changed = self
            .seq
            .as_ref()
            .is_none_or(|s| s.mb_w != seq.mb_w || s.mb_h != seq.mb_h);
        if changed {
            self.past = None;
            self.future = None;
            self.spare = None;
        }
        self.seq = Some(seq);
        self.pos = br.byte_pos();
        Ok(())
    }

    fn parse_gop_header(&mut self) {
        // time_code, closed_gop and broken_link carry nothing the decoder needs; timing comes
        // from the PES time stamps and temporal references. A new group restarts the temporal
        // reference numbering.
        self.anchor = None;
        let fps = self.seq.as_ref().map_or(25.0, |s| s.info.frame_rate);
        self.gop_base = if self.pictures == 0 {
            0.0
        } else {
            self.max_time + 1.0 / fps
        };
    }

    /// PTS of the packet in which the picture starting at `pic_start` begins, if that picture is
    /// the first to start in it.
    fn take_pts(&mut self, pic_start: usize) -> Option<f64> {
        let i = self.marks.partition_point(|m| m.offset <= pic_start);
        if i == 0 {
            return None;
        }
        let idx = i - 1;
        if self.last_used_mark == Some(idx) {
            return None;
        }
        self.last_used_mark = Some(idx);
        Some(self.marks[idx].pts)
    }

    fn frame_of(&self, p: &Picture) -> Frame {
        let seq = self.seq.as_ref().expect("sequence");
        p.to_frame(seq.info.width, seq.info.height, seq.mb_w, seq.mb_h)
    }

    fn decode_picture(&mut self, sc: usize) -> Result<()> {
        let Some(seq) = self.seq.clone() else {
            // Pictures before any sequence header cannot be decoded; skip them.
            self.pos = sc + 4;
            return Ok(());
        };
        let mut br = BitReader::new(&self.es, sc + 4);
        let tr = br.read(10) as i32;
        let kind = match br.read(3) {
            1 => PictureType::I,
            2 => PictureType::P,
            3 => PictureType::B,
            _ => {
                self.pos = sc + 4;
                return if self.strict {
                    Err(MovieError::Unsupported("D picture"))
                } else {
                    Ok(())
                };
            }
        };
        br.skip(16); // vbv_delay
        let mut full_pel = [false; 2];
        let mut f_code = [1u32; 2];
        if kind != PictureType::I {
            full_pel[0] = br.read_bit();
            f_code[0] = br.read(3);
            if kind == PictureType::B {
                full_pel[1] = br.read_bit();
                f_code[1] = br.read(3);
            }
            if f_code[0] == 0 || f_code[1] == 0 {
                self.pos = sc + 4;
                return Err(MovieError::Syntax("f_code of zero"));
            }
        }
        while br.read_bit() {
            br.skip(8); // extra_information_picture
            if br.overrun() {
                return Err(MovieError::Truncated);
            }
        }
        let mut p = br.byte_pos() + usize::from(br.bit_pos() & 7 != 0);

        // Timing: a picture that starts a packet with a PTS takes that time and becomes the anchor
        // of its group. Later pictures of the group are placed from the anchor by the difference of
        // their temporal references; before any anchor, from the group start.
        let fps = seq.info.frame_rate;
        let time = match self.take_pts(sc) {
            Some(pts) => {
                self.anchor = Some((pts, tr));
                pts
            }
            None => match self.anchor {
                Some((t0, tr0)) => {
                    let mut d = tr - tr0;
                    if d < -512 {
                        d += 1024;
                    } else if d > 512 {
                        d -= 1024;
                    }
                    t0 + f64::from(d) / fps
                }
                None => self.gop_base + f64::from(tr) / fps,
            },
        };
        self.max_time = self.max_time.max(time);
        self.pictures += 1;

        // Pictures this one predicts from.
        let mut cur = self
            .spare
            .take()
            .unwrap_or_else(|| Picture::new(seq.mb_w, seq.mb_h));
        cur.time = time;
        cur.kind = kind;
        cur.temporal_reference = tr as u16;
        let gray;
        let (fwd, bwd) = match kind {
            PictureType::I => (None, None),
            PictureType::P => (self.future.as_ref(), None),
            PictureType::B => (self.past.as_ref(), self.future.as_ref()),
        };
        let (fwd, bwd) = if kind == PictureType::I {
            (None, None)
        } else {
            // Missing references (stream starts with P/B pictures) predict from mid-grey.
            gray = Picture::gray(seq.mb_w, seq.mb_h);
            (
                Some(fwd.or(bwd).unwrap_or(&gray)),
                if kind == PictureType::B {
                    Some(bwd.or(fwd).unwrap_or(&gray))
                } else {
                    None
                },
            )
        };

        // Every macroblock must be decoded (coded or skipped) by some slice.
        let mut covered = vec![false; seq.mb_w * seq.mb_h];
        let mut ctx = PicCtx {
            seq: &seq,
            kind,
            full_pel,
            f_code,
            cur: &mut cur,
            fwd,
            bwd,
            t: tables(),
            scratch: &mut self.scratch,
            covered: &mut covered,
        };
        let mut error: Option<MovieError> = None;
        let mut next_pos = None;
        loop {
            let Some(s) = find_start_code(&self.es, p) else {
                next_pos = Some(self.es.len());
                break;
            };
            let code = self.es[s + 3];
            match code {
                0x01..=0xAF => {
                    let mut sbr = BitReader::new(&self.es, s + 4);
                    if let Err(e) = ctx.decode_slice(&mut sbr, usize::from(code) - 1) {
                        error.get_or_insert(e);
                    }
                    p = s + 4;
                }
                0xB2 | 0xB5 => p = s + 4,
                _ => {
                    next_pos = Some(s);
                    break;
                }
            }
        }
        if covered.contains(&false) {
            // A picture with no slices, or one with a slice missing, is not a picture we decoded.
            error.get_or_insert(MovieError::Syntax(
                "picture does not cover every macroblock",
            ));
        }
        self.pos = next_pos.unwrap_or(self.es.len());
        if let Some(e) = error {
            if self.strict {
                self.spare = Some(cur);
                return Err(e);
            }
            self.concealed_errors += 1;
        }

        match kind {
            PictureType::I | PictureType::P => {
                // `future` was the newest anchor; it is now complete in display order and the
                // new picture waits for the next anchor.
                let old_past = self.past.take();
                let shown = self.future.take();
                self.past = shown;
                self.future = Some(cur);
                self.spare = old_past;
                if let Some(s) = self.past.as_ref() {
                    let f = self.frame_of(s);
                    self.ready.push_back(f);
                }
            }
            PictureType::B => {
                let f = self.frame_of(&cur);
                self.ready.push_back(f);
                self.spare = Some(cur);
            }
        }
        Ok(())
    }
}

/// Per-block, per-macroblock state that resets at slice boundaries.
struct SliceState {
    qscale: i32,
    /// Last DC value of each component (Y, Cb, Cr), in units of the 8-bit sample.
    dc_pred: [i32; 3],
    /// Motion vector predictors: [forward, backward][horizontal, vertical], in the units of the
    /// coded vectors (half or full pels).
    mv_pred: [[i32; 2]; 2],
    /// Direction flags (MB_FWD | MB_BWD) of the previous macroblock, for skipped B macroblocks.
    last_motion: i16,
}

struct PicCtx<'a> {
    seq: &'a Sequence,
    kind: PictureType,
    full_pel: [bool; 2],
    f_code: [u32; 2],
    cur: &'a mut Picture,
    fwd: Option<&'a Picture>,
    bwd: Option<&'a Picture>,
    t: &'static Tables,
    scratch: &'a mut McScratch,
    /// Macroblocks decoded or skipped so far, by address.
    covered: &'a mut [bool],
}

impl PicCtx<'_> {
    fn decode_slice(&mut self, br: &mut BitReader, row: usize) -> Result<()> {
        let t = self.t;
        let mb_w = self.seq.mb_w;
        let mb_total = mb_w * self.seq.mb_h;
        if row >= self.seq.mb_h {
            return Err(MovieError::Syntax("slice row outside the picture"));
        }
        let qscale = br.read(5) as i32;
        if qscale == 0 {
            return Err(MovieError::Syntax("quantiser_scale of zero"));
        }
        while br.read_bit() {
            br.skip(8); // extra_information_slice
            if br.overrun() {
                return Err(MovieError::Truncated);
            }
        }
        let mut st = SliceState {
            qscale,
            dc_pred: [128; 3],
            mv_pred: [[0; 2]; 2],
            last_motion: 0,
        };
        let mut addr = (row * mb_w) as isize - 1;
        let mut first = true;
        loop {
            // macroblock_address_increment
            let mut inc = 0usize;
            loop {
                let e = t.mb_addr.lookup(br.peek32());
                if e.len == 0 {
                    return Err(MovieError::Syntax("macroblock_address_increment"));
                }
                br.skip(u32::from(e.len));
                match e.val {
                    ADDR_ESCAPE => inc += 33,
                    ADDR_STUFFING => {}
                    v => {
                        inc += v as usize;
                        break;
                    }
                }
            }
            if br.overrun() {
                return Err(MovieError::Truncated);
            }
            let target = addr + inc as isize;
            if target >= mb_total as isize {
                return Err(MovieError::Syntax("macroblock address past the picture"));
            }
            // Skipped macroblocks lie between the previous macroblock and this one. For the first
            // macroblock of a slice the ones before it in the row are treated as skipped too.
            let skip_from = if first {
                (row * mb_w) as isize
            } else {
                addr + 1
            };
            if self.kind != PictureType::I {
                for a in skip_from..target {
                    self.skip_macroblock(a as usize, &mut st);
                }
            }
            for a in skip_from..=target {
                self.covered[a as usize] = true;
            }
            first = false;
            addr = target;
            self.macroblock(br, addr as usize, &mut st)?;
            if br.overrun() {
                return Err(MovieError::Truncated);
            }
            // The slice ends where the next start code prefix (23 zero bits) begins.
            if br.peek(23) == 0 {
                return Ok(());
            }
        }
    }

    fn skip_macroblock(&mut self, addr: usize, st: &mut SliceState) {
        st.dc_pred = [128; 3];
        let (mbx, mby) = (addr % self.seq.mb_w, addr / self.seq.mb_w);
        match self.kind {
            PictureType::P => {
                st.mv_pred = [[0; 2]; 2];
                self.predict(mbx, mby, MB_FWD, [[0; 2]; 2]);
            }
            PictureType::B => {
                let flags = if st.last_motion == 0 {
                    MB_FWD
                } else {
                    st.last_motion
                };
                self.predict(mbx, mby, flags, st.mv_pred);
            }
            PictureType::I => return,
        }
        self.store_prediction(mbx, mby);
    }

    fn macroblock(&mut self, br: &mut BitReader, addr: usize, st: &mut SliceState) -> Result<()> {
        let t = self.t;
        let (mbx, mby) = (addr % self.seq.mb_w, addr / self.seq.mb_w);
        let table = match self.kind {
            PictureType::I => &t.mb_type_i,
            PictureType::P => &t.mb_type_p,
            PictureType::B => &t.mb_type_b,
        };
        let e = table.lookup(br.peek32());
        if e.len == 0 {
            return Err(MovieError::Syntax("macroblock_type"));
        }
        br.skip(u32::from(e.len));
        let flags = e.val;
        if flags & MB_QUANT != 0 {
            st.qscale = br.read(5) as i32;
            if st.qscale == 0 {
                return Err(MovieError::Syntax("quantiser_scale of zero"));
            }
        }
        let intra = flags & MB_INTRA != 0;
        if intra {
            st.mv_pred = [[0; 2]; 2];
            st.last_motion = 0;
        } else {
            st.dc_pred = [128; 3];
            let mut vectors = st.mv_pred;
            if flags & MB_FWD != 0 {
                self.read_motion(br, 0, st)?;
                vectors[0] = st.mv_pred[0];
            } else if self.kind == PictureType::P {
                st.mv_pred = [[0; 2]; 2];
                vectors = [[0; 2]; 2];
            }
            if flags & MB_BWD != 0 {
                self.read_motion(br, 1, st)?;
                vectors[1] = st.mv_pred[1];
            }
            st.last_motion = flags & (MB_FWD | MB_BWD);
            let dirs = if self.kind == PictureType::P {
                MB_FWD
            } else {
                flags & (MB_FWD | MB_BWD)
            };
            self.predict(mbx, mby, dirs, vectors);
        }
        let cbp = if flags & MB_PATTERN != 0 {
            let e = t.cbp.lookup(br.peek32());
            if e.len == 0 {
                return Err(MovieError::Syntax("coded_block_pattern"));
            }
            br.skip(u32::from(e.len));
            e.val as u32
        } else if intra {
            63
        } else {
            0
        };
        if intra {
            for b in 0..6 {
                let mut coefs = [0i32; 64];
                self.read_block(br, true, b, st, &mut coefs)?;
                idct8x8(&mut coefs);
                self.put_block(mbx, mby, b, &coefs, false);
            }
        } else {
            self.store_prediction_partial(mbx, mby, cbp);
            for b in 0..6 {
                if cbp & (32 >> b) == 0 {
                    continue;
                }
                let mut coefs = [0i32; 64];
                self.read_block(br, false, b, st, &mut coefs)?;
                idct8x8(&mut coefs);
                self.put_block(mbx, mby, b, &coefs, true);
            }
        }
        Ok(())
    }

    /// Read one motion vector (both components) for direction `dir` and update the predictor.
    fn read_motion(&self, br: &mut BitReader, dir: usize, st: &mut SliceState) -> Result<()> {
        let f_code = self.f_code[dir];
        let r_size = f_code - 1;
        let f = 1i32 << r_size;
        for comp in 0..2 {
            let e = self.t.motion.lookup(br.peek32());
            if e.len == 0 {
                return Err(MovieError::Syntax("motion_code"));
            }
            br.skip(u32::from(e.len));
            let code = i32::from(e.val);
            let mut delta = code;
            if code != 0 && f != 1 {
                let r = br.read(r_size) as i32;
                delta = ((code.abs() - 1) * f + r + 1) * code.signum();
            }
            let mut v = st.mv_pred[dir][comp] + delta;
            if v < -16 * f {
                v += 32 * f;
            } else if v > 16 * f - 1 {
                v -= 32 * f;
            }
            st.mv_pred[dir][comp] = v;
        }
        if br.overrun() {
            return Err(MovieError::Truncated);
        }
        Ok(())
    }

    /// Read and dequantise one block into `out` (natural order).
    fn read_block(
        &self,
        br: &mut BitReader,
        intra: bool,
        block: usize,
        st: &mut SliceState,
        out: &mut [i32; 64],
    ) -> Result<()> {
        let t = self.t;
        let q = st.qscale;
        let matrix = if intra {
            &self.seq.intra_matrix
        } else {
            &self.seq.non_intra_matrix
        };
        let mut idx = 0usize;
        let mut first = true;
        if intra {
            let comp = if block < 4 { 0 } else { block - 3 };
            let tbl = if comp == 0 { &t.dc_luma } else { &t.dc_chroma };
            let e = tbl.lookup(br.peek32());
            if e.len == 0 {
                return Err(MovieError::Syntax("dct_dc_size"));
            }
            br.skip(u32::from(e.len));
            let size = e.val as u32;
            let diff = if size == 0 {
                0
            } else {
                let v = br.read(size) as i32;
                if v < (1 << (size - 1)) {
                    v + 1 - (1 << size)
                } else {
                    v
                }
            };
            st.dc_pred[comp] += diff;
            out[0] = st.dc_pred[comp] * 8;
            idx = 1;
            first = false;
        }
        loop {
            let (run, level_mag, negative);
            if first && br.peek(1) == 1 {
                // First coefficient of a non-intra block: "1s" means run 0, level 1.
                br.skip(1);
                run = 0;
                level_mag = 1;
                negative = br.read_bit();
            } else {
                let e = t.coef.lookup(br.peek32());
                if e.len == 0 {
                    return Err(MovieError::Syntax("dct_coeff"));
                }
                br.skip(u32::from(e.len));
                match e.val {
                    COEF_EOB => break,
                    COEF_ESCAPE => {
                        run = br.read(6) as usize;
                        let l = br.read(8) as i32;
                        let level = match l {
                            0 => br.read(8) as i32,
                            128 => br.read(8) as i32 - 256,
                            l if l > 128 => l - 256,
                            l => l,
                        };
                        if level == 0 {
                            return Err(MovieError::Syntax("escaped level of zero"));
                        }
                        negative = level < 0;
                        level_mag = level.abs();
                    }
                    v => {
                        run = (v >> 8) as usize;
                        level_mag = i32::from(v & 0xFF);
                        negative = br.read_bit();
                    }
                }
            }
            first = false;
            idx += run;
            if idx > 63 {
                return Err(MovieError::Syntax("coefficient index past 63"));
            }
            let pos = usize::from(ZIGZAG[idx]);
            let w = i32::from(matrix[pos]);
            let mut v = if intra {
                (level_mag * q * w) >> 3
            } else {
                ((2 * level_mag + 1) * q * w) >> 4
            };
            // Mismatch control: odd values only (a zero stays zero).
            if v != 0 && v & 1 == 0 {
                v -= 1;
            }
            let v = if negative { -v } else { v };
            out[pos] = v.clamp(-2048, 2047);
            idx += 1;
            if br.overrun() {
                return Err(MovieError::Truncated);
            }
        }
        Ok(())
    }

    /// Motion-compensated prediction of a whole macroblock into the scratch buffers.
    fn predict(&mut self, mbx: usize, mby: usize, dirs: i16, vectors: [[i32; 2]; 2]) {
        let seq = self.seq;
        let (yw, yh) = (seq.mb_w * 16, seq.mb_h * 16);
        let mut have = false;
        for dir in 0..2 {
            let bit = if dir == 0 { MB_FWD } else { MB_BWD };
            if dirs & bit == 0 {
                continue;
            }
            let Some(r) = (if dir == 0 { self.fwd } else { self.bwd }) else {
                continue;
            };
            let scale = if self.full_pel[dir] { 1 } else { 0 };
            let mvx = vectors[dir][0] << scale;
            let mvy = vectors[dir][1] << scale;
            // Luma: vectors are in half pels.
            let s = &mut *self.scratch;
            let (mc_y, mc_cb, mc_cr) = if have {
                (&mut s.y2, &mut s.cb2, &mut s.cr2)
            } else {
                (&mut s.y, &mut s.cb, &mut s.cr)
            };
            picture::mc_block(
                mc_y,
                16,
                &r.y,
                yw,
                yh,
                (mbx * 16) as i32,
                (mby * 16) as i32,
                mvx,
                mvy,
                16,
            );
            // Chroma: the luma vector halved (truncating toward zero), in half pels of the
            // half-resolution planes.
            let (cx, cy) = (mvx / 2, mvy / 2);
            picture::mc_block(
                mc_cb,
                8,
                &r.cb,
                yw / 2,
                yh / 2,
                (mbx * 8) as i32,
                (mby * 8) as i32,
                cx,
                cy,
                8,
            );
            picture::mc_block(
                mc_cr,
                8,
                &r.cr,
                yw / 2,
                yh / 2,
                (mbx * 8) as i32,
                (mby * 8) as i32,
                cx,
                cy,
                8,
            );
            if have {
                s.average();
            }
            have = true;
        }
        if !have {
            self.scratch.fill_gray();
        }
    }

    fn store_prediction(&mut self, mbx: usize, mby: usize) {
        self.store_prediction_partial(mbx, mby, 0);
    }

    /// Copy the predicted macroblock into the picture (blocks with residual are overwritten
    /// later by `put_block`, so only uncoded blocks matter, but copying all is simplest).
    fn store_prediction_partial(&mut self, mbx: usize, mby: usize, _cbp: u32) {
        let s = &*self.scratch;
        let ys = self.seq.mb_w * 16;
        let cs = self.seq.mb_w * 8;
        for r in 0..16 {
            let o = (mby * 16 + r) * ys + mbx * 16;
            self.cur.y[o..o + 16].copy_from_slice(&s.y[r * 16..r * 16 + 16]);
        }
        for r in 0..8 {
            let o = (mby * 8 + r) * cs + mbx * 8;
            self.cur.cb[o..o + 8].copy_from_slice(&s.cb[r * 8..r * 8 + 8]);
            self.cur.cr[o..o + 8].copy_from_slice(&s.cr[r * 8..r * 8 + 8]);
        }
    }

    /// Write a decoded block: either the samples themselves (intra) or a residual added to the
    /// prediction already stored in the picture.
    fn put_block(&mut self, mbx: usize, mby: usize, block: usize, v: &[i32; 64], add: bool) {
        let ys = self.seq.mb_w * 16;
        let cs = self.seq.mb_w * 8;
        let (plane, stride, x0, y0) = match block {
            0..=3 => (
                &mut self.cur.y,
                ys,
                mbx * 16 + (block & 1) * 8,
                mby * 16 + (block >> 1) * 8,
            ),
            4 => (&mut self.cur.cb, cs, mbx * 8, mby * 8),
            _ => (&mut self.cur.cr, cs, mbx * 8, mby * 8),
        };
        for r in 0..8 {
            let row = &mut plane[(y0 + r) * stride + x0..(y0 + r) * stride + x0 + 8];
            for (c, px) in row.iter_mut().enumerate() {
                let s = v[r * 8 + c] + if add { i32::from(*px) } else { 0 };
                *px = s.clamp(0, 255) as u8;
            }
        }
    }
}
