//! Picture buffers, half-pel motion compensation and output frames.

use super::PictureType;

/// A decoded picture: three planes padded to whole macroblocks.
pub struct Picture {
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub time: f64,
    pub kind: PictureType,
    pub temporal_reference: u16,
}

impl Picture {
    pub fn new(mb_w: usize, mb_h: usize) -> Self {
        Self {
            y: vec![0; mb_w * mb_h * 256],
            cb: vec![0; mb_w * mb_h * 64],
            cr: vec![0; mb_w * mb_h * 64],
            time: 0.0,
            kind: PictureType::I,
            temporal_reference: 0,
        }
    }

    /// A mid-grey picture, used when a stream predicts from a reference it never carried.
    pub fn gray(mb_w: usize, mb_h: usize) -> Self {
        let mut p = Self::new(mb_w, mb_h);
        p.y.fill(128);
        p.cb.fill(128);
        p.cr.fill(128);
        p
    }

    pub fn to_frame(&self, width: u32, height: u32, mb_w: usize, _mb_h: usize) -> Frame {
        Frame {
            time: self.time,
            width,
            height,
            y: self.y.clone(),
            y_stride: (mb_w * 16) as u32,
            cb: self.cb.clone(),
            cr: self.cr.clone(),
            c_stride: (mb_w * 8) as u32,
            picture_type: self.kind,
            temporal_reference: self.temporal_reference,
        }
    }
}

/// One decoded video frame: planar YCbCr 4:2:0. Planes are padded to whole 16x16 macroblocks;
/// only the top-left `width` x `height` luma pixels are visible.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Presentation time in seconds of the stream clock.
    pub time: f64,
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub y_stride: u32,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
    pub c_stride: u32,
    pub picture_type: PictureType,
    pub temporal_reference: u16,
}

impl Frame {
    /// Convert the visible area to tightly packed RGBA8 (BT.601 limited range, as MPEG-1 video
    /// is normally mastered). Chroma samples are replicated (nearest) across their 2x2 block.
    pub fn to_rgba(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = vec![255u8; w * h * 4];
        for row in 0..h {
            let yrow = &self.y[row * self.y_stride as usize..];
            let crow = (row / 2) * self.c_stride as usize;
            for col in 0..w {
                let y = i32::from(yrow[col]);
                let cb = i32::from(self.cb[crow + col / 2]) - 128;
                let cr = i32::from(self.cr[crow + col / 2]) - 128;
                let yy = (y - 16) * 298;
                let r = (yy + 409 * cr + 128) >> 8;
                let g = (yy - 100 * cb - 208 * cr + 128) >> 8;
                let b = (yy + 516 * cb + 128) >> 8;
                let o = (row * w + col) * 4;
                out[o] = r.clamp(0, 255) as u8;
                out[o + 1] = g.clamp(0, 255) as u8;
                out[o + 2] = b.clamp(0, 255) as u8;
            }
        }
        out
    }

    /// The visible luma area as tightly packed 8-bit grey.
    pub fn luma_visible(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut out = Vec::with_capacity(w * h);
        for row in 0..h {
            let o = row * self.y_stride as usize;
            out.extend_from_slice(&self.y[o..o + w]);
        }
        out
    }

    /// Create a new frame with the specified time.
    pub fn with_time(&self, time: f64) -> Self {
        Self {
            time,
            ..self.clone()
        }
    }
}

/// Prediction buffers for one macroblock (two sets so bidirectional prediction can average).
pub struct McScratch {
    pub y: [u8; 256],
    pub cb: [u8; 64],
    pub cr: [u8; 64],
    pub y2: [u8; 256],
    pub cb2: [u8; 64],
    pub cr2: [u8; 64],
}

impl Default for McScratch {
    fn default() -> Self {
        Self {
            y: [128; 256],
            cb: [128; 64],
            cr: [128; 64],
            y2: [128; 256],
            cb2: [128; 64],
            cr2: [128; 64],
        }
    }
}

impl McScratch {
    /// Average the second prediction into the first: `(a + b + 1) >> 1`.
    pub fn average(&mut self) {
        for (a, b) in self.y.iter_mut().zip(&self.y2) {
            *a = ((u16::from(*a) + u16::from(*b) + 1) >> 1) as u8;
        }
        for (a, b) in self.cb.iter_mut().zip(&self.cb2) {
            *a = ((u16::from(*a) + u16::from(*b) + 1) >> 1) as u8;
        }
        for (a, b) in self.cr.iter_mut().zip(&self.cr2) {
            *a = ((u16::from(*a) + u16::from(*b) + 1) >> 1) as u8;
        }
    }

    pub fn fill_gray(&mut self) {
        self.y.fill(128);
        self.cb.fill(128);
        self.cr.fill(128);
    }
}

/// Predict a `size` x `size` block (16 for luma, 8 for chroma) from the reference plane `src`
/// (`pw` x `ph`, stride `pw`). `(x0, y0)` is the block position, `(mvx, mvy)` the vector in half
/// samples. Reference coordinates outside the plane are clamped to the nearest edge sample.
/// Half-sample positions average the neighbours with the standard's rounding: two taps
/// `(a + b + 1) >> 1`, four taps `(a + b + c + d + 2) >> 2`.
#[allow(clippy::too_many_arguments)]
pub fn mc_block(
    dst: &mut [u8],
    dst_stride: usize,
    src: &[u8],
    pw: usize,
    ph: usize,
    x0: i32,
    y0: i32,
    mvx: i32,
    mvy: i32,
    size: usize,
) {
    let hx = (mvx & 1) as usize;
    let hy = (mvy & 1) as usize;
    let sx = x0 + (mvx >> 1);
    let sy = y0 + (mvy >> 1);
    let need_w = size + hx;
    let need_h = size + hy;
    const S: usize = 17;
    let mut buf = [0u8; S * S];
    if sx >= 0 && sy >= 0 && sx as usize + need_w <= pw && sy as usize + need_h <= ph {
        for r in 0..need_h {
            let o = (sy as usize + r) * pw + sx as usize;
            buf[r * S..r * S + need_w].copy_from_slice(&src[o..o + need_w]);
        }
    } else {
        for r in 0..need_h {
            let yy = (sy + r as i32).clamp(0, ph as i32 - 1) as usize;
            for c in 0..need_w {
                let xx = (sx + c as i32).clamp(0, pw as i32 - 1) as usize;
                buf[r * S + c] = src[yy * pw + xx];
            }
        }
    }
    for r in 0..size {
        let d = &mut dst[r * dst_stride..r * dst_stride + size];
        let a = &buf[r * S..r * S + size + 1];
        let b = &buf[(r + 1) * S..(r + 1) * S + size + 1];
        match (hx, hy) {
            (0, 0) => d.copy_from_slice(&a[..size]),
            (1, 0) => {
                for c in 0..size {
                    d[c] = ((u16::from(a[c]) + u16::from(a[c + 1]) + 1) >> 1) as u8;
                }
            }
            (0, 1) => {
                for c in 0..size {
                    d[c] = ((u16::from(a[c]) + u16::from(b[c]) + 1) >> 1) as u8;
                }
            }
            _ => {
                for c in 0..size {
                    d[c] = ((u16::from(a[c])
                        + u16::from(a[c + 1])
                        + u16::from(b[c])
                        + u16::from(b[c + 1])
                        + 2)
                        >> 2) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_pel_rounds_up() {
        // 4x4 plane of 0, 3, 6, ...; 2x2 block at the origin.
        let src: Vec<u8> = (0..16).map(|i| i as u8 * 3).collect();
        let mut dst = [0u8; 4];
        mc_block(&mut dst, 2, &src, 4, 4, 0, 0, 1, 0, 2);
        assert_eq!(dst, [2, 5, 14, 17]);
        mc_block(&mut dst, 2, &src, 4, 4, 0, 0, 0, 1, 2);
        assert_eq!(dst, [6, 9, 18, 21]);
        mc_block(&mut dst, 2, &src, 4, 4, 0, 0, 1, 1, 2);
        assert_eq!(dst, [8, 11, 20, 23]);
        // Integer vector: plain copy; vector pointing outside clamps to the edge.
        mc_block(&mut dst, 2, &src, 4, 4, 0, 0, 2, 2, 2);
        assert_eq!(dst, [15, 18, 27, 30]);
        mc_block(&mut dst, 2, &src, 4, 4, 0, 0, -8, -8, 2);
        assert_eq!(dst, [0, 0, 0, 0]);
    }
}
