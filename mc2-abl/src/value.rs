// Portions derived from Microsoft's MechCommander 2 shared-source release.
// Copyright (C) Microsoft Corporation. Used under the Microsoft Shared Source Limited Permissive License (see LICENSE-MS-SSLPL.txt).
//! Runtime values: stack cells, locations and the array heap.
//!
//! Scalars are stored as raw 32-bit patterns, exactly like the `StackItem` union in the C++ (integer, boolean,
//! enum, char and real all share a 32-bit slot). Arrays live in a byte heap so that `char[]` strings, integer and real
//! arrays and nested dimensions keep the C++ memory layout (element sizes 1 / 4 / 4 bytes, row-major).

/// Reference to a byte range inside the array heap: array id plus byte offset (for sub-arrays and elements).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArrRef {
    /// Heap array id.
    pub id: u32,
    /// Byte offset into the array.
    pub off: u32,
}

/// How to interpret a memory location holding a scalar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemKind {
    /// 32-bit integer / boolean / enum.
    I32,
    /// 32-bit float.
    F32,
    /// 8-bit char.
    U8,
}

/// A scalar storage location (the C++ passes raw pointers; this is the safe equivalent).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loc {
    /// Absolute index into the runtime stack.
    Stack(u32),
    /// Static data slot of a module instance (instance id, slot).
    Static(u32, u32),
    /// Eternal variable slot.
    Eternal(u32),
    /// A scalar element inside an array.
    Mem(ArrRef, MemKind),
}

/// One stack slot / value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cell {
    /// A 32-bit scalar pattern (integer, boolean, enum, char or float bits).
    Bits(u32),
    /// Address of array data (array variables and sub-arrays).
    Arr(ArrRef),
    /// Address of a scalar variable (value of `@` reference parameters).
    Ref(Loc),
}

impl Cell {
    /// The zero scalar.
    pub const ZERO: Cell = Cell::Bits(0);

    /// An integer cell.
    pub fn int(v: i32) -> Cell {
        Cell::Bits(v as u32)
    }

    /// A real cell.
    pub fn real(v: f32) -> Cell {
        Cell::Bits(v.to_bits())
    }

    /// Reinterpret as integer (`StackItem::integer`).
    pub fn as_int(self) -> i32 {
        match self {
            Cell::Bits(b) => b as i32,
            _ => 0,
        }
    }

    /// Reinterpret as real (`StackItem::real`).
    pub fn as_real(self) -> f32 {
        match self {
            Cell::Bits(b) => f32::from_bits(b),
            _ => 0.0,
        }
    }

    /// Low byte (`StackItem::byte`).
    pub fn as_byte(self) -> u8 {
        match self {
            Cell::Bits(b) => b as u8,
            _ => 0,
        }
    }
}

/// The byte heap that backs every array and string.
#[derive(Default, Debug)]
pub struct Heap {
    arrays: Vec<Vec<u8>>,
    free: Vec<u32>,
    bytes: usize,
}

impl Heap {
    /// Allocate a zeroed array of `size` bytes.
    pub fn alloc(&mut self, size: usize) -> u32 {
        self.bytes += size;
        if let Some(id) = self.free.pop() {
            let a = &mut self.arrays[id as usize];
            a.clear();
            a.resize(size, 0);
            id
        } else {
            self.arrays.push(vec![0; size]);
            (self.arrays.len() - 1) as u32
        }
    }

    /// Allocate an array holding `bytes` followed by a NUL terminator (a C string).
    pub fn alloc_cstr(&mut self, bytes: &[u8]) -> u32 {
        let id = self.alloc(bytes.len() + 1);
        self.arrays[id as usize][..bytes.len()].copy_from_slice(bytes);
        id
    }

    /// Release an array.
    pub fn release(&mut self, id: u32) {
        if let Some(a) = self.arrays.get_mut(id as usize) {
            self.bytes -= a.len();
            a.clear();
            self.free.push(id);
        }
    }

    /// Whole array contents.
    pub fn bytes(&self, id: u32) -> &[u8] {
        &self.arrays[id as usize]
    }

    /// Whole array contents, mutable.
    pub fn bytes_mut(&mut self, id: u32) -> &mut [u8] {
        &mut self.arrays[id as usize]
    }

    /// Bytes from `r` to the end of its array.
    pub fn tail(&self, r: ArrRef) -> &[u8] {
        let a = &self.arrays[r.id as usize];
        let off = (r.off as usize).min(a.len());
        &a[off..]
    }

    /// Mutable bytes from `r` to the end of its array.
    pub fn tail_mut(&mut self, r: ArrRef) -> &mut [u8] {
        let a = &mut self.arrays[r.id as usize];
        let off = (r.off as usize).min(a.len());
        &mut a[off..]
    }

    /// Read a little-endian 32-bit word; out-of-range reads yield 0.
    pub fn read_u32(&self, r: ArrRef) -> u32 {
        let t = self.tail(r);
        if t.len() >= 4 {
            u32::from_le_bytes([t[0], t[1], t[2], t[3]])
        } else {
            0
        }
    }

    /// Write a little-endian 32-bit word; out-of-range writes are ignored.
    pub fn write_u32(&mut self, r: ArrRef, v: u32) {
        let t = self.tail_mut(r);
        if t.len() >= 4 {
            t[..4].copy_from_slice(&v.to_le_bytes());
        }
    }

    /// Read one byte; out-of-range reads yield 0.
    pub fn read_u8(&self, r: ArrRef) -> u8 {
        self.tail(r).first().copied().unwrap_or(0)
    }

    /// Write one byte; out-of-range writes are ignored.
    pub fn write_u8(&mut self, r: ArrRef, v: u8) {
        if let Some(b) = self.tail_mut(r).first_mut() {
            *b = v;
        }
    }

    /// The C string starting at `r` (up to the first NUL or the end of the array).
    pub fn cstr(&self, r: ArrRef) -> &[u8] {
        let t = self.tail(r);
        let n = t.iter().position(|&b| b == 0).unwrap_or(t.len());
        &t[..n]
    }

    /// Total bytes held by live arrays.
    pub fn live_bytes(&self) -> usize {
        self.bytes
    }

    /// Number of live arrays (diagnostics / leak tests).
    pub fn live(&self) -> usize {
        self.arrays.len() - self.free.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_reuses_released_ids_and_zeroes_them() {
        let mut h = Heap::default();
        let a = h.alloc(8);
        let b = h.alloc_cstr(b"hi");
        assert_eq!(h.live(), 2);
        h.write_u32(ArrRef { id: a, off: 4 }, 0xdead_beef);
        assert_eq!(h.read_u32(ArrRef { id: a, off: 4 }), 0xdead_beef);
        assert_eq!(h.cstr(ArrRef { id: b, off: 0 }), b"hi");
        h.release(a);
        assert_eq!(h.live(), 1);
        let c = h.alloc(8);
        assert_eq!(c, a);
        assert_eq!(h.read_u32(ArrRef { id: c, off: 4 }), 0);
    }

    #[test]
    fn out_of_range_access_is_harmless() {
        let mut h = Heap::default();
        let a = h.alloc(4);
        assert_eq!(h.read_u32(ArrRef { id: a, off: 2 }), 0);
        h.write_u32(ArrRef { id: a, off: 2 }, 5);
        assert_eq!(h.read_u8(ArrRef { id: a, off: 100 }), 0);
    }

    #[test]
    fn cells_reinterpret_bits_like_the_stack_union() {
        assert_eq!(Cell::real(1.0).as_int(), 0x3f80_0000);
        assert_eq!(Cell::int(1).as_real(), f32::from_bits(1));
        assert_eq!(Cell::int(-1).as_byte(), 0xff);
    }
}
