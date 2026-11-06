//! MSB-first bit reader over a byte slice. Reads past the end return zero bits; callers check
//! [`BitReader::overrun`] at syntax-element boundaries.

#![allow(dead_code)]

#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits from the start of `data`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8], byte_pos: usize) -> Self {
        Self {
            data,
            pos: byte_pos * 8,
        }
    }

    /// Bit position from the start of the slice.
    pub fn bit_pos(&self) -> usize {
        self.pos
    }

    /// True once more bits were consumed than the slice holds.
    pub fn overrun(&self) -> bool {
        self.pos > self.data.len() * 8
    }

    pub fn bits_left(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    /// The next 32 bits without consuming them (zero padded past the end).
    #[inline]
    pub fn peek32(&self) -> u32 {
        let byte = self.pos >> 3;
        let sh = (self.pos & 7) as u32;
        let v: u64 = match self.data.get(byte..byte + 5) {
            Some(s) => {
                (u64::from(s[0]) << 32)
                    | (u64::from(s[1]) << 24)
                    | (u64::from(s[2]) << 16)
                    | (u64::from(s[3]) << 8)
                    | u64::from(s[4])
            }
            None => {
                let mut v = 0u64;
                for i in 0..5 {
                    v = (v << 8) | u64::from(self.data.get(byte + i).copied().unwrap_or(0));
                }
                v
            }
        };
        ((v << sh) >> 8) as u32
    }

    /// Peek `n` bits (1..=32).
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        debug_assert!((1..=32).contains(&n));
        self.peek32() >> (32 - n)
    }

    #[inline]
    pub fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    /// Read `n` bits (0..=32).
    #[inline]
    pub fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    #[inline]
    pub fn read_bit(&mut self) -> bool {
        self.read(1) != 0
    }

    pub fn align(&mut self) {
        self.pos = (self.pos + 7) & !7;
    }

    pub fn byte_pos(&self) -> usize {
        self.pos >> 3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_bytes() {
        let d = [0b1010_1100, 0b0101_0011, 0xFF, 0x00, 0x80];
        let mut r = BitReader::new(&d, 0);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.read(7), 0b0110001);
        assert_eq!(r.peek32() >> 24, 0b0100_1111);
        r.align();
        assert_eq!(r.byte_pos(), 2);
        assert_eq!(r.read(8), 0xFF);
        assert_eq!(r.read(9), 0b0_0000_0001);
        assert!(!r.overrun());
        r.skip(100);
        assert!(r.overrun());
        assert_eq!(r.peek32(), 0);
    }
}
