// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Little-endian cursor over a `Stuff::MemoryStream`.
//!
//! Every gosFX read in the C++ is `ReadBytes(&value, sizeof(value))` on a little-endian
//! x86 file, so each primitive is read with an explicit `from_le_bytes`.

use crate::error::{FxError, Result};
use crate::math::{Mat, Vec3};

pub struct Stream<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    /// `MemoryStream::RewindPointer`.
    pub fn rewind(&mut self, n: usize) {
        self.pos = self.pos.saturating_sub(n);
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len());
        match end {
            Some(end) => {
                let s = &self.bytes[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => Err(FxError::new(format!(
                "truncated stream at {}: need {n} bytes, {} left",
                self.pos,
                self.remaining()
            ))),
        }
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    /// `bool` is one byte in the MSVC build that wrote the data.
    pub fn bool(&mut self) -> Result<bool> {
        Ok(self.u8()? != 0)
    }
    pub fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_bits(self.u32()?))
    }
    pub fn vec3(&mut self) -> Result<Vec3> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    /// `Stuff::LinearMatrix4D`: 12 floats, `entries[(column << 2) + row]`.
    pub fn mat(&mut self) -> Result<Mat> {
        let mut e = [0.0f32; 12];
        for v in e.iter_mut() {
            *v = self.f32()?;
        }
        Ok(Mat::from_entries(e))
    }
    /// `MemoryStreamIO::Read(MString)`: u32 length, then length + 1 bytes (with the NUL) when
    /// the length is non-zero.
    pub fn mstring(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        if len == 0 {
            return Ok(String::new());
        }
        if len > 1 << 20 {
            return Err(FxError::new(format!("unreasonable string length {len}")));
        }
        let b = self.take(len + 1)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(len);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }
    /// A length prefix that is about to size a vector of `elem` byte elements: refuses counts
    /// the remaining data cannot hold.
    pub fn count(&mut self, elem: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.checked_mul(elem.max(1))
            .is_none_or(|b| b > self.remaining())
        {
            return Err(FxError::new(format!(
                "count {n} x {elem} bytes exceeds the {} bytes left",
                self.remaining()
            )));
        }
        Ok(n)
    }
}
