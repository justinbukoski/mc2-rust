//! Deflate (levels 6 and 9) that is byte-for-byte what zlib 1.2.x/1.3.x writes for
//! `deflateInit2(level, Z_DEFLATED, 15 or -15, memLevel 8, Z_DEFAULT_STRATEGY)` fed the whole input at
//! once and finished (`deflate_slow` plus `trees.c`).
//!
//! flate2's backends (miniz_oxide, zlib-rs) are valid deflate but choose different matches and block
//! splits, which would change the archive; the Python tools these replace used
//! the system zlib. This is a direct port of that algorithm, pinned by the goldens in `tests/golden`.
//!
//! This file is an altered (Rust) adaptation of zlib 1.2.12 `deflate.c`/`trees.c`/`deflate.h`/`trees.h`
//! (https://github.com/madler/zlib/tree/v1.2.12), not the original software.
//! zlib (C) 1995-2022 Jean-loup Gailly and Mark Adler; its complete license notice is kept in
//! `ZLIB-LICENSE.txt` next to this crate's `Cargo.toml` and ships with the source:
//!   This software is provided 'as-is', without any express or implied warranty. Permission is granted to anyone
//!   to use it for any purpose, and to alter and redistribute it freely, subject to: 1. the origin must not be
//!   misrepresented; 2. altered source versions must be plainly marked as such and not misrepresented as the
//!   original; 3. this notice may not be removed or altered from any source distribution.

// The port keeps zlib's index-based loops over its tables so it can be compared with trees.c line by line.
#![allow(clippy::needless_range_loop)]

const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MIN_LOOKAHEAD: usize = MAX_MATCH + MIN_MATCH + 1;
const W_BITS: usize = 15;
const W_SIZE: usize = 1 << W_BITS;
const W_MASK: usize = W_SIZE - 1;
const MAX_DIST: usize = W_SIZE - MIN_LOOKAHEAD;
const WINDOW_SIZE: usize = 2 * W_SIZE;
const HASH_BITS: u32 = 15; // memLevel 8 + 7
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: usize = HASH_SIZE - 1;
const HASH_SHIFT: u32 = HASH_BITS.div_ceil(MIN_MATCH as u32);
const LIT_BUFSIZE: usize = 1 << 14; // memLevel 8 + 6
const SYM_END: usize = LIT_BUFSIZE - 1;
const TOO_FAR: usize = 4096;
/// zlib's `configuration_table` row for a `deflate_slow` level: good_length, max_lazy, nice_length, max_chain.
fn config(level: u8) -> (usize, usize, usize, usize) {
    match level {
        6 => (8, 16, 128, 128),
        9 => (32, 258, 258, 4096),
        _ => panic!("deflate level {level} is not ported (6 and 9 are used)"),
    }
}

const LENGTH_CODES: usize = 29;
const LITERALS: usize = 256;
const L_CODES: usize = LITERALS + 1 + LENGTH_CODES;
const D_CODES: usize = 30;
const BL_CODES: usize = 19;
const HEAP_SIZE: usize = 2 * L_CODES + 1;
const MAX_BITS: usize = 15;
const MAX_BL_BITS: usize = 7;
const END_BLOCK: usize = 256;
const REP_3_6: usize = 16;
const REPZ_3_10: usize = 17;
const REPZ_11_138: usize = 18;

const EXTRA_LBITS: [u8; LENGTH_CODES] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const EXTRA_DBITS: [u8; D_CODES] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
const EXTRA_BLBITS: [u8; BL_CODES] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 3, 7];
const BL_ORDER: [usize; BL_CODES] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// A tree node. zlib overlays Freq/Code and Dad/Len in unions; none of the overlays is observable.
#[derive(Clone, Copy, Default)]
struct Ct {
    freq: u16,
    code: u16,
    dad: u16,
    len: u16,
}

fn bi_reverse(mut code: u32, mut len: u32) -> u32 {
    let mut res = 0;
    loop {
        res |= code & 1;
        code >>= 1;
        res <<= 1;
        len -= 1;
        if len == 0 {
            break;
        }
    }
    res >> 1
}

/// Assigns canonical codes to the `len`s in `tree[..=max_code]` (zlib `gen_codes`).
fn gen_codes(tree: &mut [Ct], max_code: i32, bl_count: &[u16; MAX_BITS + 1]) {
    let mut next_code = [0u32; MAX_BITS + 1];
    let mut code = 0u32;
    for bits in 1..=MAX_BITS {
        code = (code + u32::from(bl_count[bits - 1])) << 1;
        next_code[bits] = code & 0xffff;
    }
    for n in 0..=max_code.max(-1) {
        let n = n as usize;
        let len = u32::from(tree[n].len);
        if len == 0 {
            continue;
        }
        tree[n].code = bi_reverse(next_code[len as usize], len) as u16;
        next_code[len as usize] += 1;
    }
}

/// The constant tables of `trees.c` (`tr_static_init`).
struct Statics {
    length_code: [u8; 256],
    dist_code: [u8; 512],
    base_length: [u16; LENGTH_CODES],
    base_dist: [u16; D_CODES],
    ltree: [Ct; L_CODES + 2],
    dtree: [Ct; D_CODES],
}

fn statics() -> Statics {
    let mut s = Statics {
        length_code: [0; 256],
        dist_code: [0; 512],
        base_length: [0; LENGTH_CODES],
        base_dist: [0; D_CODES],
        ltree: [Ct::default(); L_CODES + 2],
        dtree: [Ct::default(); D_CODES],
    };
    let mut length = 0usize;
    for code in 0..LENGTH_CODES - 1 {
        s.base_length[code] = length as u16;
        for _ in 0..1 << EXTRA_LBITS[code] {
            s.length_code[length] = code as u8;
            length += 1;
        }
    }
    s.length_code[length - 1] = (LENGTH_CODES - 1) as u8;
    let mut dist = 0usize;
    for code in 0..16 {
        s.base_dist[code] = dist as u16;
        for _ in 0..1 << EXTRA_DBITS[code] {
            s.dist_code[dist] = code as u8;
            dist += 1;
        }
    }
    dist >>= 7;
    for code in 16..D_CODES {
        s.base_dist[code] = (dist << 7) as u16;
        for _ in 0..1 << (EXTRA_DBITS[code] - 7) {
            s.dist_code[256 + dist] = code as u8;
            dist += 1;
        }
    }
    let mut bl_count = [0u16; MAX_BITS + 1];
    for n in 0..L_CODES + 2 {
        let len = match n {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
        s.ltree[n].len = len;
        bl_count[len as usize] += 1;
    }
    gen_codes(&mut s.ltree, (L_CODES + 1) as i32, &bl_count);
    for n in 0..D_CODES {
        s.dtree[n].len = 5;
        s.dtree[n].code = bi_reverse(n as u32, 5) as u16;
    }
    s
}

#[derive(Clone, Copy, PartialEq)]
enum Which {
    Lit,
    Dist,
    Bl,
}

struct Deflater<'a> {
    st: Statics,
    cfg: (usize, usize, usize, usize),
    input: &'a [u8],
    in_pos: usize,
    out: Vec<u8>,
    bit_buf: u64,
    bit_count: u32,
    // window and hash chains
    window: Vec<u8>,
    prev: Vec<u16>,
    head: Vec<u16>,
    ins_h: usize,
    insert: usize,
    strstart: usize,
    lookahead: usize,
    block_start: i64,
    match_length: usize,
    prev_match: usize,
    match_available: bool,
    match_start: usize,
    prev_length: usize,
    // trees
    dyn_ltree: [Ct; HEAP_SIZE],
    dyn_dtree: [Ct; 2 * D_CODES + 1],
    bl_tree: [Ct; 2 * BL_CODES + 1],
    l_max_code: i32,
    d_max_code: i32,
    bl_max_code: i32,
    bl_count: [u16; MAX_BITS + 1],
    heap: [usize; HEAP_SIZE],
    heap_len: usize,
    heap_max: usize,
    depth: [u8; HEAP_SIZE],
    syms: Vec<(u16, u8)>,
    opt_len: u32,
    static_len: u32,
}

impl<'a> Deflater<'a> {
    fn new(input: &'a [u8], level: u8) -> Self {
        let mut d = Deflater {
            st: statics(),
            cfg: config(level),
            input,
            in_pos: 0,
            out: Vec::new(),
            bit_buf: 0,
            bit_count: 0,
            window: vec![0; WINDOW_SIZE],
            prev: vec![0; W_SIZE],
            head: vec![0; HASH_SIZE],
            ins_h: 0,
            insert: 0,
            strstart: 0,
            lookahead: 0,
            block_start: 0,
            match_length: MIN_MATCH - 1,
            prev_match: 0,
            match_available: false,
            match_start: 0,
            prev_length: MIN_MATCH - 1,
            dyn_ltree: [Ct::default(); HEAP_SIZE],
            dyn_dtree: [Ct::default(); 2 * D_CODES + 1],
            bl_tree: [Ct::default(); 2 * BL_CODES + 1],
            l_max_code: 0,
            d_max_code: 0,
            bl_max_code: 0,
            bl_count: [0; MAX_BITS + 1],
            heap: [0; HEAP_SIZE],
            heap_len: 0,
            heap_max: 0,
            depth: [0; HEAP_SIZE],
            syms: Vec::with_capacity(SYM_END),
            opt_len: 0,
            static_len: 0,
        };
        d.init_block();
        d
    }

    // ---- bit output ----

    fn send_bits(&mut self, value: u32, len: u32) {
        self.bit_buf |= u64::from(value) << self.bit_count;
        self.bit_count += len;
        while self.bit_count >= 8 {
            self.out.push(self.bit_buf as u8);
            self.bit_buf >>= 8;
            self.bit_count -= 8;
        }
    }

    fn bi_windup(&mut self) {
        if self.bit_count > 0 {
            self.out.push(self.bit_buf as u8);
        }
        self.bit_buf = 0;
        self.bit_count = 0;
    }

    fn send_code(&mut self, c: usize, which: Which, stat: bool) {
        let node = match (which, stat) {
            (Which::Lit, false) => self.dyn_ltree[c],
            (Which::Lit, true) => self.st.ltree[c],
            (Which::Dist, false) => self.dyn_dtree[c],
            (Which::Dist, true) => self.st.dtree[c],
            (Which::Bl, _) => self.bl_tree[c],
        };
        self.send_bits(u32::from(node.code), u32::from(node.len));
    }

    // ---- trees.c ----

    fn init_block(&mut self) {
        for n in 0..L_CODES {
            self.dyn_ltree[n].freq = 0;
        }
        for n in 0..D_CODES {
            self.dyn_dtree[n].freq = 0;
        }
        for n in 0..BL_CODES {
            self.bl_tree[n].freq = 0;
        }
        self.dyn_ltree[END_BLOCK].freq = 1;
        self.opt_len = 0;
        self.static_len = 0;
        self.syms.clear();
    }

    fn tree(&mut self, which: Which) -> &mut [Ct] {
        match which {
            Which::Lit => &mut self.dyn_ltree,
            Which::Dist => &mut self.dyn_dtree,
            Which::Bl => &mut self.bl_tree,
        }
    }

    fn smaller(&self, tree: &[Ct], n: usize, m: usize) -> bool {
        tree[n].freq < tree[m].freq
            || (tree[n].freq == tree[m].freq && self.depth[n] <= self.depth[m])
    }

    fn pqdownheap(&mut self, tree: &[Ct], mut k: usize) {
        let v = self.heap[k];
        let mut j = k << 1;
        while j <= self.heap_len {
            if j < self.heap_len && self.smaller(tree, self.heap[j + 1], self.heap[j]) {
                j += 1;
            }
            if self.smaller(tree, v, self.heap[j]) {
                break;
            }
            self.heap[k] = self.heap[j];
            k = j;
            j <<= 1;
        }
        self.heap[k] = v;
    }

    /// `(static tree, extra bits, extra base, elems, max length)` of the three descriptors.
    fn desc(which: Which) -> (bool, &'static [u8], usize, usize, usize) {
        match which {
            Which::Lit => (true, &EXTRA_LBITS, LITERALS + 1, L_CODES, MAX_BITS),
            Which::Dist => (true, &EXTRA_DBITS, 0, D_CODES, MAX_BITS),
            Which::Bl => (false, &EXTRA_BLBITS, 0, BL_CODES, MAX_BL_BITS),
        }
    }

    fn gen_bitlen(&mut self, tree: &mut [Ct], which: Which, max_code: i32) {
        let (has_static, extra, base, _, max_length) = Self::desc(which);
        let mut overflow = 0i32;
        self.bl_count = [0; MAX_BITS + 1];
        tree[self.heap[self.heap_max]].len = 0;
        let mut h = self.heap_max + 1;
        while h < HEAP_SIZE {
            let n = self.heap[h];
            let mut bits = usize::from(tree[usize::from(tree[n].dad)].len) + 1;
            if bits > max_length {
                bits = max_length;
                overflow += 1;
            }
            tree[n].len = bits as u16;
            h += 1;
            if n as i32 > max_code {
                continue;
            }
            self.bl_count[bits] += 1;
            let xbits = if n >= base {
                usize::from(extra[n - base])
            } else {
                0
            };
            let f = u32::from(tree[n].freq);
            self.opt_len = self.opt_len.wrapping_add(f * (bits + xbits) as u32);
            if has_static {
                let slen = match which {
                    Which::Lit => self.st.ltree[n].len,
                    _ => self.st.dtree[n].len,
                };
                self.static_len = self
                    .static_len
                    .wrapping_add(f * (u32::from(slen) + xbits as u32));
            }
        }
        if overflow == 0 {
            return;
        }
        loop {
            let mut bits = max_length - 1;
            while self.bl_count[bits] == 0 {
                bits -= 1;
            }
            self.bl_count[bits] -= 1;
            self.bl_count[bits + 1] += 2;
            self.bl_count[max_length] -= 1;
            overflow -= 2;
            if overflow <= 0 {
                break;
            }
        }
        let mut h = HEAP_SIZE;
        for bits in (1..=max_length).rev() {
            let mut n = self.bl_count[bits];
            while n != 0 {
                h -= 1;
                let m = self.heap[h];
                if m as i32 > max_code {
                    continue;
                }
                if usize::from(tree[m].len) != bits {
                    let delta = (bits as i64 - i64::from(tree[m].len)) * i64::from(tree[m].freq);
                    self.opt_len = (i64::from(self.opt_len) + delta) as u32;
                    tree[m].len = bits as u16;
                }
                n -= 1;
            }
        }
    }

    fn build_tree(&mut self, which: Which) -> i32 {
        let (has_static, _, _, elems, _) = Self::desc(which);
        let mut tree = self.tree(which).to_vec();
        let mut max_code: i32 = -1;
        self.heap_len = 0;
        self.heap_max = HEAP_SIZE;
        for n in 0..elems {
            if tree[n].freq != 0 {
                self.heap_len += 1;
                self.heap[self.heap_len] = n;
                max_code = n as i32;
                self.depth[n] = 0;
            } else {
                tree[n].len = 0;
            }
        }
        while self.heap_len < 2 {
            let node = if max_code < 2 {
                max_code += 1;
                max_code as usize
            } else {
                0
            };
            self.heap_len += 1;
            self.heap[self.heap_len] = node;
            tree[node].freq = 1;
            self.depth[node] = 0;
            self.opt_len = self.opt_len.wrapping_sub(1);
            if has_static {
                let slen = match which {
                    Which::Lit => self.st.ltree[node].len,
                    _ => self.st.dtree[node].len,
                };
                self.static_len = self.static_len.wrapping_sub(u32::from(slen));
            }
        }
        for n in (1..=self.heap_len / 2).rev() {
            self.pqdownheap(&tree, n);
        }
        let mut node = elems;
        loop {
            let n = self.heap[1];
            self.heap[1] = self.heap[self.heap_len];
            self.heap_len -= 1;
            self.pqdownheap(&tree, 1);
            let m = self.heap[1];
            self.heap_max -= 1;
            self.heap[self.heap_max] = n;
            self.heap_max -= 1;
            self.heap[self.heap_max] = m;
            tree[node].freq = tree[n].freq.wrapping_add(tree[m].freq);
            self.depth[node] = self.depth[n].max(self.depth[m]) + 1;
            tree[n].dad = node as u16;
            tree[m].dad = node as u16;
            self.heap[1] = node;
            node += 1;
            self.pqdownheap(&tree, 1);
            if self.heap_len < 2 {
                break;
            }
        }
        self.heap_max -= 1;
        self.heap[self.heap_max] = self.heap[1];
        self.gen_bitlen(&mut tree, which, max_code);
        let counts = self.bl_count;
        gen_codes(&mut tree, max_code, &counts);
        self.tree(which).copy_from_slice(&tree);
        max_code
    }

    /// Counts the code-length codes of `tree[..=max_code]` into `bl_tree` (`scan_tree`).
    fn scan_tree(&mut self, which: Which, max_code: i32) {
        let mut tree = self.tree(which).to_vec();
        let mut prevlen: i32 = -1;
        let mut nextlen = i32::from(tree[0].len);
        let mut count = 0;
        let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
        tree[(max_code + 1) as usize].len = 0xffff;
        for n in 0..=max_code as usize {
            let curlen = nextlen;
            nextlen = i32::from(tree[n + 1].len);
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                self.bl_tree[curlen as usize].freq += count as u16;
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.bl_tree[curlen as usize].freq += 1;
                }
                self.bl_tree[REP_3_6].freq += 1;
            } else if count <= 10 {
                self.bl_tree[REPZ_3_10].freq += 1;
            } else {
                self.bl_tree[REPZ_11_138].freq += 1;
            }
            count = 0;
            prevlen = curlen;
            (max_count, min_count) = if nextlen == 0 {
                (138, 3)
            } else if curlen == nextlen {
                (6, 3)
            } else {
                (7, 4)
            };
        }
        self.tree(which).copy_from_slice(&tree);
    }

    fn send_tree(&mut self, which: Which, max_code: i32) {
        let tree = self.tree(which).to_vec();
        let mut prevlen: i32 = -1;
        let mut nextlen = i32::from(tree[0].len);
        let mut count = 0;
        let (mut max_count, mut min_count) = if nextlen == 0 { (138, 3) } else { (7, 4) };
        for n in 0..=max_code as usize {
            let curlen = nextlen;
            nextlen = i32::from(tree[n + 1].len);
            count += 1;
            if count < max_count && curlen == nextlen {
                continue;
            } else if count < min_count {
                while count != 0 {
                    self.send_code(curlen as usize, Which::Bl, false);
                    count -= 1;
                }
            } else if curlen != 0 {
                if curlen != prevlen {
                    self.send_code(curlen as usize, Which::Bl, false);
                    count -= 1;
                }
                self.send_code(REP_3_6, Which::Bl, false);
                self.send_bits(count as u32 - 3, 2);
            } else if count <= 10 {
                self.send_code(REPZ_3_10, Which::Bl, false);
                self.send_bits(count as u32 - 3, 3);
            } else {
                self.send_code(REPZ_11_138, Which::Bl, false);
                self.send_bits(count as u32 - 11, 7);
            }
            count = 0;
            prevlen = curlen;
            (max_count, min_count) = if nextlen == 0 {
                (138, 3)
            } else if curlen == nextlen {
                (6, 3)
            } else {
                (7, 4)
            };
        }
    }

    fn build_bl_tree(&mut self) -> usize {
        self.scan_tree(Which::Lit, self.l_max_code);
        self.scan_tree(Which::Dist, self.d_max_code);
        self.bl_max_code = self.build_tree(Which::Bl);
        let mut max_blindex = BL_CODES - 1;
        while max_blindex >= 3 {
            if self.bl_tree[BL_ORDER[max_blindex]].len != 0 {
                break;
            }
            max_blindex -= 1;
        }
        self.opt_len = self
            .opt_len
            .wrapping_add(3 * (max_blindex as u32 + 1) + 5 + 5 + 4);
        max_blindex
    }

    fn compress_block(&mut self, stat: bool) {
        let syms = std::mem::take(&mut self.syms);
        for &(dist, lc) in &syms {
            if dist == 0 {
                self.send_code(usize::from(lc), Which::Lit, stat);
            } else {
                let lc = usize::from(lc);
                let code = usize::from(self.st.length_code[lc]);
                self.send_code(code + LITERALS + 1, Which::Lit, stat);
                let extra = u32::from(EXTRA_LBITS[code]);
                if extra != 0 {
                    self.send_bits((lc - usize::from(self.st.base_length[code])) as u32, extra);
                }
                let dist = usize::from(dist) - 1;
                let code = if dist < 256 {
                    self.st.dist_code[dist]
                } else {
                    self.st.dist_code[256 + (dist >> 7)]
                };
                let code = usize::from(code);
                self.send_code(code, Which::Dist, stat);
                let extra = u32::from(EXTRA_DBITS[code]);
                if extra != 0 {
                    self.send_bits((dist - usize::from(self.st.base_dist[code])) as u32, extra);
                }
            }
        }
        self.syms = syms;
        self.send_code(END_BLOCK, Which::Lit, stat);
    }

    fn flush_block(&mut self, last: bool) {
        let stored_len = (self.strstart as i64 - self.block_start) as usize;
        self.l_max_code = self.build_tree(Which::Lit);
        self.d_max_code = self.build_tree(Which::Dist);
        let max_blindex = self.build_bl_tree();
        let mut opt_lenb = (self.opt_len + 3 + 7) >> 3;
        let static_lenb = (self.static_len + 3 + 7) >> 3;
        if static_lenb <= opt_lenb {
            opt_lenb = static_lenb;
        }
        if stored_len as u32 + 4 <= opt_lenb && self.block_start >= 0 {
            self.send_bits(u32::from(last), 3);
            self.bi_windup();
            self.out
                .extend_from_slice(&(stored_len as u16).to_le_bytes());
            self.out
                .extend_from_slice(&(!(stored_len as u16)).to_le_bytes());
            let start = self.block_start as usize;
            self.out
                .extend_from_slice(&self.window[start..start + stored_len]);
        } else if static_lenb == opt_lenb {
            self.send_bits((1 << 1) + u32::from(last), 3);
            self.compress_block(true);
        } else {
            self.send_bits((2 << 1) + u32::from(last), 3);
            let (lcodes, dcodes, blcodes) = (
                self.l_max_code as u32 + 1,
                self.d_max_code as u32 + 1,
                max_blindex as u32 + 1,
            );
            self.send_bits(lcodes - 257, 5);
            self.send_bits(dcodes - 1, 5);
            self.send_bits(blcodes - 4, 4);
            for &order in BL_ORDER.iter().take(blcodes as usize) {
                self.send_bits(u32::from(self.bl_tree[order].len), 3);
            }
            self.send_tree(Which::Lit, lcodes as i32 - 1);
            self.send_tree(Which::Dist, dcodes as i32 - 1);
            self.compress_block(false);
        }
        self.init_block();
        if last {
            self.bi_windup();
        }
        self.block_start = self.strstart as i64;
    }

    /// Records a literal (`dist` 0) or a match; true when the block is full.
    fn tally(&mut self, dist: usize, lc: usize) -> bool {
        self.syms.push((dist as u16, lc as u8));
        if dist == 0 {
            self.dyn_ltree[lc].freq += 1;
        } else {
            let d = dist - 1;
            self.dyn_ltree[usize::from(self.st.length_code[lc]) + LITERALS + 1].freq += 1;
            let dcode = if d < 256 {
                self.st.dist_code[d]
            } else {
                self.st.dist_code[256 + (d >> 7)]
            };
            self.dyn_dtree[usize::from(dcode)].freq += 1;
        }
        self.syms.len() == SYM_END
    }

    // ---- deflate.c ----

    fn update_hash(&mut self, c: u8) {
        self.ins_h = ((self.ins_h << HASH_SHIFT) ^ usize::from(c)) & HASH_MASK;
    }

    /// `INSERT_STRING`: returns the previous head of the chain.
    fn insert_string(&mut self, s: usize) -> usize {
        self.update_hash(self.window[s + MIN_MATCH - 1]);
        let head = self.head[self.ins_h];
        self.prev[s & W_MASK] = head;
        self.head[self.ins_h] = s as u16;
        usize::from(head)
    }

    fn slide_hash(&mut self) {
        for p in self.head.iter_mut().chain(self.prev.iter_mut()) {
            *p = if usize::from(*p) >= W_SIZE {
                *p - W_SIZE as u16
            } else {
                0
            };
        }
    }

    fn fill_window(&mut self) {
        loop {
            let mut more = WINDOW_SIZE - self.lookahead - self.strstart;
            if self.strstart >= W_SIZE + MAX_DIST {
                self.window.copy_within(W_SIZE..W_SIZE + (W_SIZE - more), 0);
                self.match_start = self.match_start.wrapping_sub(W_SIZE);
                self.strstart -= W_SIZE;
                self.block_start -= W_SIZE as i64;
                self.slide_hash();
                more += W_SIZE;
            }
            if self.in_pos == self.input.len() {
                break;
            }
            let n = more.min(self.input.len() - self.in_pos);
            let at = self.strstart + self.lookahead;
            self.window[at..at + n].copy_from_slice(&self.input[self.in_pos..self.in_pos + n]);
            self.in_pos += n;
            self.lookahead += n;
            if self.lookahead + self.insert >= MIN_MATCH {
                let mut str = self.strstart - self.insert;
                self.ins_h = usize::from(self.window[str]);
                self.update_hash(self.window[str + 1]);
                while self.insert != 0 {
                    self.update_hash(self.window[str + MIN_MATCH - 1]);
                    self.prev[str & W_MASK] = self.head[self.ins_h];
                    self.head[self.ins_h] = str as u16;
                    str += 1;
                    self.insert -= 1;
                    if self.lookahead + self.insert < MIN_MATCH {
                        break;
                    }
                }
            }
            if !(self.lookahead < MIN_LOOKAHEAD && self.in_pos != self.input.len()) {
                break;
            }
        }
    }

    fn longest_match(&mut self, mut cur_match: usize) -> usize {
        let (good_match, _, nice_length, max_chain) = self.cfg;
        let mut chain_length = max_chain;
        let scan = self.strstart;
        let mut best_len = self.prev_length;
        let mut nice_match = nice_length;
        let limit = self.strstart.saturating_sub(MAX_DIST);
        if self.prev_length >= good_match {
            chain_length >>= 2;
        }
        if nice_match > self.lookahead {
            nice_match = self.lookahead;
        }
        let w = &self.window;
        let mut scan_end1 = w[scan + best_len - 1];
        let mut scan_end = w[scan + best_len];
        loop {
            let matchp = cur_match;
            if w[matchp + best_len] == scan_end
                && w[matchp + best_len - 1] == scan_end1
                && w[matchp] == w[scan]
                && w[matchp + 1] == w[scan + 1]
            {
                // bytes 0, 1 are equal; byte 2 is equal because the hash keys are; compare from byte 3
                let mut len = 3;
                while len < MAX_MATCH && w[scan + len] == w[matchp + len] {
                    len += 1;
                }
                if len > best_len {
                    self.match_start = cur_match;
                    best_len = len;
                    if len >= nice_match {
                        break;
                    }
                    scan_end1 = w[scan + best_len - 1];
                    scan_end = w[scan + best_len];
                }
            }
            cur_match = usize::from(self.prev[cur_match & W_MASK]);
            if cur_match <= limit {
                break;
            }
            chain_length -= 1;
            if chain_length == 0 {
                break;
            }
        }
        best_len.min(self.lookahead)
    }

    fn deflate_slow(&mut self) {
        loop {
            if self.lookahead < MIN_LOOKAHEAD {
                self.fill_window();
                if self.lookahead == 0 {
                    break;
                }
            }
            let mut hash_head = 0;
            if self.lookahead >= MIN_MATCH {
                hash_head = self.insert_string(self.strstart);
            }
            self.prev_length = self.match_length;
            self.prev_match = self.match_start;
            self.match_length = MIN_MATCH - 1;
            if hash_head != 0
                && self.prev_length < self.cfg.1
                && self.strstart - hash_head <= MAX_DIST
            {
                self.match_length = self.longest_match(hash_head);
                if self.match_length <= 5
                    && self.match_length == MIN_MATCH
                    && self.strstart - self.match_start > TOO_FAR
                {
                    self.match_length = MIN_MATCH - 1;
                }
            }
            if self.prev_length >= MIN_MATCH && self.match_length <= self.prev_length {
                let max_insert = self.strstart + self.lookahead - MIN_MATCH;
                let bflush = self.tally(
                    self.strstart - 1 - self.prev_match,
                    self.prev_length - MIN_MATCH,
                );
                self.lookahead -= self.prev_length - 1;
                self.prev_length -= 2;
                loop {
                    self.strstart += 1;
                    if self.strstart <= max_insert {
                        self.insert_string(self.strstart);
                    }
                    self.prev_length -= 1;
                    if self.prev_length == 0 {
                        break;
                    }
                }
                self.match_available = false;
                self.match_length = MIN_MATCH - 1;
                self.strstart += 1;
                if bflush {
                    self.flush_block(false);
                }
            } else if self.match_available {
                let bflush = self.tally(0, usize::from(self.window[self.strstart - 1]));
                if bflush {
                    self.flush_block(false);
                }
                self.strstart += 1;
                self.lookahead -= 1;
            } else {
                self.match_available = true;
                self.strstart += 1;
                self.lookahead -= 1;
            }
        }
        if self.match_available {
            self.tally(0, usize::from(self.window[self.strstart - 1]));
            self.match_available = false;
        }
        self.flush_block(true);
    }
}

fn run(data: &[u8], level: u8) -> Vec<u8> {
    let mut d = Deflater::new(data, level);
    d.deflate_slow();
    d.out
}

/// A raw deflate stream (zip entries, the gzip body).
pub fn raw(data: &[u8], level: u8) -> Vec<u8> {
    run(data, level)
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += u32::from(x);
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// `zlib.compress(data, 9)`: the 0x78 0xDA header, the deflate stream, the Adler-32.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0xda];
    out.extend_from_slice(&run(data, 9));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}
