//! Baseline JPEG decoder (ITU T.81) to BGRA32: the frames of a camera's
//! MJPEG stream and pictures alike.
//!
//! Sequential Huffman (SOF0, SOF1 with 8-bit samples), one or three
//! components, any sampling factors up to 2, restart intervals. Frames
//! without DHT use the tables of Annex K.3, as MJPEG streams (AVI1)
//! assume. The IDCT is libjpeg's `jidctint` (islow); chroma is upsampled
//! by replication. Progressive and arithmetic coding are refused.
//!
//! [`Decoder::decode_scaled`] decodes at 1/2, 1/4 or 1/8 of the size with
//! the reduced IDCTs of libjpeg's `jidctred.c`: the entropy data is read
//! whole, but the transform and colour work shrink with the picture, so
//! a preview window costs a fraction of the full frame.
//!
//! A [`Decoder`] keeps its planes between frames, so a stream decodes
//! without allocating once the size is known. Corrupt input gives an
//! error, never a panic: every read is bounds-checked.

use alloc::vec;
use alloc::vec::Vec;

/// Largest picture decoded, per side and in pixels.
pub const MAX_SIDE: u32 = 16384;
pub const MAX_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotJpeg,
    Truncated,
    /// Progressive, arithmetic, 12-bit, or a layout this decoder lacks.
    Unsupported,
    TooLarge,
    /// A table, marker or code that cannot be.
    Corrupt,
}

/// Natural order of the coefficient at zigzag position `k`.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14,
    21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53,
    60, 61, 54, 47, 55, 62, 63,
];

// Annex K.3: the typical Huffman tables.
const DC_LUMA_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const DC_CHROMA_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const AC_LUMA_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const AC_LUMA_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71,
    0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72,
    0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37,
    0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83,
    0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3,
    0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
const AC_CHROMA_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const AC_CHROMA_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22,
    0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1,
    0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36,
    0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
    0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a,
    0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba,
    0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// Bits looked up at once; longer codes take the slow path.
const FAST_BITS: u32 = 9;

/// A Huffman table (Annex C) with a `FAST_BITS` lookup.
#[derive(Clone)]
struct Huff {
    /// `len << 8 | symbol`, 0 = not a short code.
    fast: Vec<u16>,
    /// Per length 1..=16: the largest code (-1 = none) and where its
    /// symbols start in `vals`, minus the smallest code.
    maxcode: [i32; 18],
    valptr: [i32; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn new(bits: &[u8; 16], vals: &[u8]) -> Result<Huff, Error> {
        let total: usize = bits.iter().map(|&b| b as usize).sum();
        if total > 256 || total > vals.len() { return Err(Error::Corrupt); }
        let mut h = Huff { fast: vec![0; 1 << FAST_BITS], maxcode: [-1; 18], valptr: [0; 17], vals: vals[..total].to_vec() };
        let mut code: u32 = 0;
        let mut k: usize = 0;
        for len in 1..=16usize {
            let n = bits[len - 1] as usize;
            h.valptr[len] = k as i32 - code as i32;
            for _ in 0..n {
                if len as u32 <= FAST_BITS {
                    let shift = FAST_BITS - len as u32;
                    let base = (code << shift) as usize;
                    for j in 0..(1usize << shift) {
                        if let Some(slot) = h.fast.get_mut(base + j) {
                            *slot = ((len as u16) << 8) | h.vals[k] as u16;
                        }
                    }
                }
                code += 1;
                k += 1;
            }
            if n > 0 { h.maxcode[len] = code as i32 - 1; }
            if code > (1 << len) { return Err(Error::Corrupt); }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        Ok(h)
    }
}

/// The entropy-coded data, with 0xFF00 unstuffed and markers held back.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u64,
    count: u32,
    /// A marker was met: zeros are fed from here on.
    marker: Option<u8>,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], pos: usize) -> Self { Bits { data, pos, acc: 0, count: 0, marker: None } }

    fn fill(&mut self) {
        while self.count <= 56 {
            let mut b = 0u8;
            if self.marker.is_none() {
                match self.data.get(self.pos) {
                    Some(&0xFF) => {
                        let next = self.data.get(self.pos + 1).copied().unwrap_or(0xD9);
                        if next == 0x00 {
                            b = 0xFF;
                            self.pos += 2;
                        } else {
                            self.marker = Some(next);
                        }
                    }
                    Some(&v) => {
                        b = v;
                        self.pos += 1;
                    }
                    None => self.marker = Some(0xD9),
                }
            }
            self.acc |= (b as u64) << (56 - self.count);
            self.count += 8;
        }
    }

    fn peek(&mut self, n: u32) -> u32 {
        if self.count < n { self.fill(); }
        (self.acc >> (64 - n)) as u32
    }

    fn skip(&mut self, n: u32) {
        self.acc <<= n;
        self.count -= n;
    }

    fn get(&mut self, n: u32) -> u32 {
        if n == 0 { return 0; }
        let v = self.peek(n);
        self.skip(n);
        v
    }

    fn decode(&mut self, h: &Huff) -> Result<u8, Error> {
        let look = self.peek(FAST_BITS) as usize;
        let f = h.fast[look];
        if f != 0 {
            self.skip((f >> 8) as u32);
            return Ok(f as u8);
        }
        let code16 = self.peek(16) as i32;
        for len in FAST_BITS as usize + 1..=16 {
            let code = code16 >> (16 - len);
            if code <= h.maxcode[len] {
                self.skip(len as u32);
                let idx = h.valptr[len] + code;
                return h.vals.get(idx as usize).copied().ok_or(Error::Corrupt);
            }
        }
        Err(Error::Corrupt)
    }

    /// `RECEIVE` + `EXTEND` (F.2.2.1).
    fn extend(&mut self, s: u8) -> i32 {
        if s == 0 { return 0; }
        let s = s.min(16) as u32;
        let v = self.get(s) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }

    /// The restart marker that should come now: byte alignment, the
    /// marker, a fresh start. False when it is not there.
    fn restart(&mut self) -> bool {
        self.acc = 0;
        self.count = 0;
        match self.marker.take() {
            Some(m) if (0xD0..=0xD7).contains(&m) => {
                self.pos += 2;
                true
            }
            Some(other) => {
                self.marker = Some(other);
                false
            }
            None => {
                // The marker was not reached yet: look for it.
                while self.pos + 1 < self.data.len() {
                    if self.data[self.pos] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.pos + 1]) {
                        self.pos += 2;
                        return true;
                    }
                    self.pos += 1;
                }
                false
            }
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    td: usize,
    ta: usize,
    /// Plane size in samples (whole MCUs).
    pw: usize,
    ph: usize,
}

/// Reusable decoder state.
pub struct Decoder {
    qt: [[u16; 64]; 4],
    dc: [Option<Huff>; 4],
    ac: [Option<Huff>; 4],
    planes: [Vec<u8>; 3],
}

impl Default for Decoder {
    fn default() -> Self { Self::new() }
}

fn be16(d: &[u8], at: usize) -> Result<usize, Error> {
    match (d.get(at), d.get(at + 1)) {
        (Some(&a), Some(&b)) => Ok(((a as usize) << 8) | b as usize),
        _ => Err(Error::Truncated),
    }
}

impl Decoder {
    pub fn new() -> Self {
        Decoder { qt: [[1; 64]; 4], dc: [None, None, None, None], ac: [None, None, None, None], planes: [Vec::new(), Vec::new(), Vec::new()] }
    }

    /// Install Annex K tables where a frame left a table out.
    fn defaults(&mut self) -> Result<(), Error> {
        if self.dc[0].is_none() { self.dc[0] = Some(Huff::new(&DC_LUMA_BITS, &DC_VALS)?); }
        if self.dc[1].is_none() { self.dc[1] = Some(Huff::new(&DC_CHROMA_BITS, &DC_VALS)?); }
        if self.ac[0].is_none() { self.ac[0] = Some(Huff::new(&AC_LUMA_BITS, &AC_LUMA_VALS)?); }
        if self.ac[1].is_none() { self.ac[1] = Some(Huff::new(&AC_CHROMA_BITS, &AC_CHROMA_VALS)?); }
        Ok(())
    }

    /// Decode `data` into `out` (BGRA32, resized to `w * h * 4`). Returns
    /// the size.
    pub fn decode(&mut self, data: &[u8], out: &mut Vec<u8>) -> Result<(u32, u32), Error> {
        self.decode_scaled(data, 1, out)
    }

    /// `decode` at 1/`scale` of the size (1, 2, 4 or 8), each side
    /// rounded up. Returns the decoded size.
    pub fn decode_scaled(&mut self, data: &[u8], scale: u32, out: &mut Vec<u8>) -> Result<(u32, u32), Error> {
        if !matches!(scale, 1 | 2 | 4 | 8) { return Err(Error::Unsupported); }
        let bs = 8 / scale as usize;
        if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 { return Err(Error::NotJpeg); }
        // Tables of the previous frame do not carry over: a stream that
        // leaves DHT out means the defaults, not the last frame's.
        self.dc = [None, None, None, None];
        self.ac = [None, None, None, None];
        let mut pos = 2;
        let mut comps: [Comp; 3] = [Comp::default(); 3];
        let mut ncomp = 0usize;
        let (mut width, mut height) = (0usize, 0usize);
        let (mut hmax, mut vmax) = (1usize, 1usize);
        let mut restart = 0usize;
        loop {
            // Next marker; fill bytes 0xFF may precede it.
            while data.get(pos) == Some(&0xFF) && data.get(pos + 1) == Some(&0xFF) { pos += 1; }
            if data.get(pos) != Some(&0xFF) { return Err(Error::Corrupt); }
            let m = *data.get(pos + 1).ok_or(Error::Truncated)?;
            pos += 2;
            match m {
                0xD8 | 0x01 | 0xD0..=0xD7 => continue,
                0xD9 => return Err(Error::Truncated),
                _ => {}
            }
            let len = be16(data, pos)?;
            if len < 2 || pos + len > data.len() { return Err(Error::Truncated); }
            let seg = &data[pos + 2..pos + len];
            match m {
                0xDB => {
                    let mut p = 0;
                    while p < seg.len() {
                        let pq = seg[p] >> 4;
                        let tq = (seg[p] & 0xF) as usize;
                        if tq > 3 { return Err(Error::Corrupt); }
                        p += 1;
                        for k in 0..64 {
                            let v = if pq == 0 {
                                *seg.get(p + k).ok_or(Error::Truncated)? as u16
                            } else {
                                be16(seg, p + 2 * k)? as u16
                            };
                            self.qt[tq][k] = v;
                        }
                        p += if pq == 0 { 64 } else { 128 };
                    }
                }
                0xC4 => {
                    let mut p = 0;
                    while p < seg.len() {
                        let tc = seg[p] >> 4;
                        let th = (seg[p] & 0xF) as usize;
                        if th > 3 || tc > 1 { return Err(Error::Corrupt); }
                        let bits: [u8; 16] = seg.get(p + 1..p + 17).ok_or(Error::Truncated)?.try_into().map_err(|_| Error::Truncated)?;
                        let total: usize = bits.iter().map(|&b| b as usize).sum();
                        let vals = seg.get(p + 17..p + 17 + total).ok_or(Error::Truncated)?;
                        let h = Huff::new(&bits, vals)?;
                        if tc == 0 { self.dc[th] = Some(h); } else { self.ac[th] = Some(h); }
                        p += 17 + total;
                    }
                }
                0xC0 | 0xC1 => {
                    if seg.len() < 6 || seg[0] != 8 { return Err(Error::Unsupported); }
                    height = be16(seg, 1)?;
                    width = be16(seg, 3)?;
                    ncomp = seg[5] as usize;
                    if width == 0 || height == 0 || (ncomp != 1 && ncomp != 3) { return Err(Error::Unsupported); }
                    if width as u32 > MAX_SIDE || height as u32 > MAX_SIDE || (width as u64 * height as u64) > MAX_PIXELS {
                        return Err(Error::TooLarge);
                    }
                    if seg.len() < 6 + 3 * ncomp { return Err(Error::Truncated); }
                    for (i, c) in comps.iter_mut().enumerate().take(ncomp) {
                        let d = &seg[6 + 3 * i..9 + 3 * i];
                        c.id = d[0];
                        c.h = (d[1] >> 4) as usize;
                        c.v = (d[1] & 0xF) as usize;
                        c.tq = (d[2] & 3) as usize;
                        if !(1..=2).contains(&c.h) || !(1..=2).contains(&c.v) { return Err(Error::Unsupported); }
                    }
                    hmax = comps[..ncomp].iter().map(|c| c.h).max().unwrap_or(1);
                    vmax = comps[..ncomp].iter().map(|c| c.v).max().unwrap_or(1);
                    if ncomp == 1 { comps[0].h = 1; comps[0].v = 1; hmax = 1; vmax = 1; }
                }
                0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Err(Error::Unsupported),
                0xDD => restart = be16(seg, 0)?,
                0xDA => {
                    if ncomp == 0 { return Err(Error::Corrupt); }
                    let ns = *seg.first().ok_or(Error::Truncated)? as usize;
                    if ns != ncomp || seg.len() < 1 + 2 * ns { return Err(Error::Unsupported); }
                    for i in 0..ns {
                        let id = seg[1 + 2 * i];
                        let t = seg[2 + 2 * i];
                        let c = comps[..ncomp].iter_mut().find(|c| c.id == id).ok_or(Error::Corrupt)?;
                        c.td = (t >> 4) as usize & 3;
                        c.ta = (t & 0xF) as usize & 3;
                    }
                    self.defaults()?;
                    self.scan(data, pos + len, &mut comps[..ncomp], width, height, hmax, vmax, restart, bs)?;
                    let (ow, oh) = (width.div_ceil(8 / bs), height.div_ceil(8 / bs));
                    self.color(&comps[..ncomp], ow, oh, hmax, vmax, out);
                    return Ok((ow as u32, oh as u32));
                }
                _ => {}
            }
            pos += len;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn scan(&mut self, data: &[u8], start: usize, comps: &mut [Comp], w: usize, h: usize,
            hmax: usize, vmax: usize, restart: usize, bs: usize) -> Result<(), Error> {
        let mcux = w.div_ceil(8 * hmax);
        let mcuy = h.div_ceil(8 * vmax);
        for (i, c) in comps.iter_mut().enumerate() {
            c.pw = mcux * c.h * bs;
            c.ph = mcuy * c.v * bs;
            let need = c.pw * c.ph;
            if self.planes[i].len() != need { self.planes[i] = vec![0; need]; }
        }
        let mut bits = Bits::new(data, start);
        let mut pred = [0i32; 3];
        let mut coef = [0i32; 64];
        let mut todo = restart;
        for my in 0..mcuy {
            for mx in 0..mcux {
                if restart != 0 {
                    if todo == 0 {
                        if !bits.restart() { return Err(Error::Corrupt); }
                        pred = [0; 3];
                        todo = restart;
                    }
                    todo -= 1;
                }
                for (ci, c) in comps.iter().enumerate() {
                    let dc = self.dc[c.td].as_ref().ok_or(Error::Corrupt)?;
                    let ac = self.ac[c.ta].as_ref().ok_or(Error::Corrupt)?;
                    let q = &self.qt[c.tq];
                    for by in 0..c.v {
                        for bx in 0..c.h {
                            coef.fill(0);
                            let s = bits.decode(dc)?;
                            pred[ci] = pred[ci].wrapping_add(bits.extend(s));
                            coef[0] = pred[ci].wrapping_mul(q[0] as i32);
                            let mut k = 1;
                            while k < 64 {
                                let rs = bits.decode(ac)?;
                                let r = (rs >> 4) as usize;
                                let s = rs & 0xF;
                                if s == 0 {
                                    if r == 15 { k += 16; continue; }
                                    break;
                                }
                                k += r;
                                if k > 63 { return Err(Error::Corrupt); }
                                coef[ZIGZAG[k] as usize] = bits.extend(s).wrapping_mul(q[k] as i32);
                                k += 1;
                            }
                            let x = (mx * c.h + bx) * bs;
                            let y = (my * c.v + by) * bs;
                            let at = y * c.pw + x;
                            let plane = &mut self.planes[ci];
                            match bs {
                                8 => idct_islow(&coef, plane, at, c.pw),
                                4 => idct_4x4(&coef, plane, at, c.pw),
                                2 => idct_2x2(&coef, plane, at, c.pw),
                                _ => idct_1x1(&coef, plane, at),
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// YCbCr (JFIF, full range) to BGRA, chroma replicated.
    fn color(&self, comps: &[Comp], w: usize, h: usize, hmax: usize, vmax: usize, out: &mut Vec<u8>) {
        out.resize(w * h * 4, 0);
        let y_plane = &self.planes[0];
        let yw = comps[0].pw;
        if comps.len() == 1 {
            for y in 0..h {
                let row = &y_plane[y * yw..y * yw + w];
                let dst = &mut out[y * w * 4..(y + 1) * w * 4];
                for (d, &l) in dst.chunks_exact_mut(4).zip(row) {
                    d[0] = l; d[1] = l; d[2] = l; d[3] = 255;
                }
            }
            return;
        }
        let (cb, cr) = (&self.planes[1], &self.planes[2]);
        let cw = comps[1].pw;
        let (sx, sy) = (hmax / comps[1].h, vmax / comps[1].v);
        let ysx = hmax / comps[0].h;
        let ysy = vmax / comps[0].v;
        for y in 0..h {
            let yrow = (y / ysy) * yw;
            let crow = (y / sy) * cw;
            let dst = &mut out[y * w * 4..(y + 1) * w * 4];
            for (x, d) in dst.chunks_exact_mut(4).enumerate() {
                let l = (y_plane[yrow + x / ysx] as i32) << 16;
                let ci = crow + x / sx;
                let b = cb[ci] as i32 - 128;
                let r = cr[ci] as i32 - 128;
                let rr = (l + 91881 * r + 32768) >> 16;
                let gg = (l - 22554 * b - 46802 * r + 32768) >> 16;
                let bb = (l + 116130 * b + 32768) >> 16;
                d[0] = bb.clamp(0, 255) as u8;
                d[1] = gg.clamp(0, 255) as u8;
                d[2] = rr.clamp(0, 255) as u8;
                d[3] = 255;
            }
        }
    }
}

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

#[inline(always)]
fn descale(x: i32, n: i32) -> i32 { x.wrapping_add(1 << (n - 1)) >> n }

/// One 8-point pass of `jpeg_idct_islow`: even and odd parts, the eight
/// outputs before descaling.
#[inline(always)]
fn idct8(i: [i32; 8]) -> [i32; 8] {
    let z1 = i[2].wrapping_add(i[6]).wrapping_mul(FIX_0_541196100);
    let tmp2 = z1.wrapping_add(i[6].wrapping_mul(-FIX_1_847759065));
    let tmp3 = z1.wrapping_add(i[2].wrapping_mul(FIX_0_765366865));
    let tmp0 = i[0].wrapping_add(i[4]) << CONST_BITS;
    let tmp1 = i[0].wrapping_sub(i[4]) << CONST_BITS;
    let tmp10 = tmp0.wrapping_add(tmp3);
    let tmp13 = tmp0.wrapping_sub(tmp3);
    let tmp11 = tmp1.wrapping_add(tmp2);
    let tmp12 = tmp1.wrapping_sub(tmp2);

    let (t0, t1, t2, t3) = (i[7], i[5], i[3], i[1]);
    let z1 = t0.wrapping_add(t3);
    let z2 = t1.wrapping_add(t2);
    let z3 = t0.wrapping_add(t2);
    let z4 = t1.wrapping_add(t3);
    let z5 = z3.wrapping_add(z4).wrapping_mul(FIX_1_175875602);
    let t0 = t0.wrapping_mul(FIX_0_298631336);
    let t1 = t1.wrapping_mul(FIX_2_053119869);
    let t2 = t2.wrapping_mul(FIX_3_072711026);
    let t3 = t3.wrapping_mul(FIX_1_501321110);
    let z1 = z1.wrapping_mul(-FIX_0_899976223);
    let z2 = z2.wrapping_mul(-FIX_2_562915447);
    let z3 = z3.wrapping_mul(-FIX_1_961570560).wrapping_add(z5);
    let z4 = z4.wrapping_mul(-FIX_0_390180644).wrapping_add(z5);
    let t0 = t0.wrapping_add(z1).wrapping_add(z3);
    let t1 = t1.wrapping_add(z2).wrapping_add(z4);
    let t2 = t2.wrapping_add(z2).wrapping_add(z3);
    let t3 = t3.wrapping_add(z1).wrapping_add(z4);
    [
        tmp10.wrapping_add(t3), tmp11.wrapping_add(t2), tmp12.wrapping_add(t1), tmp13.wrapping_add(t0),
        tmp13.wrapping_sub(t0), tmp12.wrapping_sub(t1), tmp11.wrapping_sub(t2), tmp10.wrapping_sub(t3),
    ]
}

/// libjpeg's `jpeg_idct_islow`: columns, then rows, into `out` at `at`
/// with row stride `stride`.
fn idct_islow(c: &[i32; 64], out: &mut [u8], at: usize, stride: usize) {
    let mut ws = [0i32; 64];
    for col in 0..8 {
        let g = |r: usize| c[r * 8 + col];
        if (1..8).all(|r| g(r) == 0) {
            let dc = g(0) << PASS1_BITS;
            for r in 0..8 { ws[r * 8 + col] = dc; }
            continue;
        }
        let o = idct8([g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7)]);
        for r in 0..8 { ws[r * 8 + col] = descale(o[r], CONST_BITS - PASS1_BITS); }
    }
    for row in 0..8 {
        let w = &ws[row * 8..row * 8 + 8];
        let Some(dst) = out.get_mut(at + row * stride..at + row * stride + 8) else { return };
        if w[1..].iter().all(|&v| v == 0) {
            let v = (descale(w[0], PASS1_BITS + 3) + 128).clamp(0, 255) as u8;
            dst.fill(v);
            continue;
        }
        let o = idct8([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
        for (d, v) in dst.iter_mut().zip(o) {
            *d = (descale(v, CONST_BITS + PASS1_BITS + 3) + 128).clamp(0, 255) as u8;
        }
    }
}

const FIX_0_211164243: i32 = 1730;
const FIX_0_509795579: i32 = 4176;
const FIX_0_601344887: i32 = 4926;
const FIX_0_720959822: i32 = 5906;
const FIX_0_850430095: i32 = 6967;
const FIX_1_061594337: i32 = 8697;
const FIX_1_272758580: i32 = 10426;
const FIX_1_451774981: i32 = 11893;
const FIX_2_172734803: i32 = 17799;
const FIX_3_624509785: i32 = 29692;

#[inline(always)]
fn clamp8(v: i32) -> u8 { (v + 128).clamp(0, 255) as u8 }

/// The 4-point pass of `jpeg_idct_4x4`: the even part from inputs 0, 2,
/// 6, the odd part from 1, 3, 5, 7.
#[inline(always)]
fn idct4(i: [i32; 8]) -> [i32; 4] {
    let tmp0 = i[0] << (CONST_BITS + 1);
    let tmp2 = i[2].wrapping_mul(FIX_1_847759065).wrapping_add(i[6].wrapping_mul(-FIX_0_765366865));
    let tmp10 = tmp0.wrapping_add(tmp2);
    let tmp12 = tmp0.wrapping_sub(tmp2);
    let (z1, z2, z3, z4) = (i[7], i[5], i[3], i[1]);
    let o0 = z1.wrapping_mul(-FIX_0_211164243).wrapping_add(z2.wrapping_mul(FIX_1_451774981))
        .wrapping_add(z3.wrapping_mul(-FIX_2_172734803)).wrapping_add(z4.wrapping_mul(FIX_1_061594337));
    let o2 = z1.wrapping_mul(-FIX_0_509795579).wrapping_add(z2.wrapping_mul(-FIX_0_601344887))
        .wrapping_add(z3.wrapping_mul(FIX_0_899976223)).wrapping_add(z4.wrapping_mul(FIX_2_562915447));
    [tmp10.wrapping_add(o2), tmp12.wrapping_add(o0), tmp12.wrapping_sub(o0), tmp10.wrapping_sub(o2)]
}

/// libjpeg's `jpeg_idct_4x4` (jidctred.c): an 8x8 block to 4x4 samples.
fn idct_4x4(c: &[i32; 64], out: &mut [u8], at: usize, stride: usize) {
    let mut ws = [0i32; 32];
    for col in 0..8 {
        if col == 4 { continue; }
        let g = |r: usize| c[r * 8 + col];
        if [1, 2, 3, 5, 6, 7].iter().all(|&r| g(r) == 0) {
            let dc = g(0) << PASS1_BITS;
            for r in 0..4 { ws[r * 8 + col] = dc; }
            continue;
        }
        let o = idct4([g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7)]);
        for r in 0..4 { ws[r * 8 + col] = descale(o[r], CONST_BITS - PASS1_BITS + 1); }
    }
    for row in 0..4 {
        let w = &ws[row * 8..row * 8 + 8];
        let Some(dst) = out.get_mut(at + row * stride..at + row * stride + 4) else { return };
        if [1, 2, 3, 5, 6, 7].iter().all(|&k| w[k] == 0) {
            dst.fill(clamp8(descale(w[0], PASS1_BITS + 3)));
            continue;
        }
        let o = idct4([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
        for (d, v) in dst.iter_mut().zip(o) {
            *d = clamp8(descale(v, CONST_BITS + PASS1_BITS + 3 + 1));
        }
    }
}

/// The 2-point pass of `jpeg_idct_2x2`.
#[inline(always)]
fn idct2(i: [i32; 8]) -> [i32; 2] {
    let tmp10 = i[0] << (CONST_BITS + 2);
    let tmp0 = i[7].wrapping_mul(-FIX_0_720959822).wrapping_add(i[5].wrapping_mul(FIX_0_850430095))
        .wrapping_add(i[3].wrapping_mul(-FIX_1_272758580)).wrapping_add(i[1].wrapping_mul(FIX_3_624509785));
    [tmp10.wrapping_add(tmp0), tmp10.wrapping_sub(tmp0)]
}

/// libjpeg's `jpeg_idct_2x2`: an 8x8 block to 2x2 samples.
fn idct_2x2(c: &[i32; 64], out: &mut [u8], at: usize, stride: usize) {
    let mut ws = [0i32; 16];
    for col in [0usize, 1, 3, 5, 7] {
        let g = |r: usize| c[r * 8 + col];
        if [1, 3, 5, 7].iter().all(|&r| g(r) == 0) {
            let dc = g(0) << PASS1_BITS;
            ws[col] = dc;
            ws[8 + col] = dc;
            continue;
        }
        let o = idct2([g(0), g(1), g(2), g(3), g(4), g(5), g(6), g(7)]);
        ws[col] = descale(o[0], CONST_BITS - PASS1_BITS + 2);
        ws[8 + col] = descale(o[1], CONST_BITS - PASS1_BITS + 2);
    }
    for row in 0..2 {
        let w = &ws[row * 8..row * 8 + 8];
        let Some(dst) = out.get_mut(at + row * stride..at + row * stride + 2) else { return };
        if [1, 3, 5, 7].iter().all(|&k| w[k] == 0) {
            dst.fill(clamp8(descale(w[0], PASS1_BITS + 3)));
            continue;
        }
        let o = idct2([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]);
        dst[0] = clamp8(descale(o[0], CONST_BITS + PASS1_BITS + 3 + 2));
        dst[1] = clamp8(descale(o[1], CONST_BITS + PASS1_BITS + 3 + 2));
    }
}

/// libjpeg's `jpeg_idct_1x1`: the DC alone.
fn idct_1x1(c: &[i32; 64], out: &mut [u8], at: usize) {
    if let Some(d) = out.get_mut(at) { *d = clamp8(descale(c[0], 3)); }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn annex_k_tables_have_162_ac_symbols() {
        assert_eq!(AC_LUMA_BITS.iter().map(|&b| b as usize).sum::<usize>(), 162);
        assert_eq!(AC_CHROMA_BITS.iter().map(|&b| b as usize).sum::<usize>(), 162);
        assert!(Huff::new(&AC_LUMA_BITS, &AC_LUMA_VALS).is_ok());
        assert!(Huff::new(&AC_CHROMA_BITS, &AC_CHROMA_VALS).is_ok());
    }

    #[test]
    fn idct_of_dc_only_is_flat() {
        let mut c = [0i32; 64];
        c[0] = 80;
        let mut out = [0u8; 64];
        idct_islow(&c, &mut out, 0, 8);
        assert!(out.iter().all(|&v| v == 138));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        let mut d = Decoder::new();
        let mut out = Vec::new();
        assert_eq!(d.decode(b"nope", &mut out), Err(Error::NotJpeg));
        let mut junk = vec![0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0, 8, 0, 8, 1, 1, 0x11, 0, 0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 63, 0];
        junk.extend(core::iter::repeat_n(0x5A, 64));
        let _ = d.decode(&junk, &mut out);
    }

    /// Every picture at 1/2, 1/4 and 1/8 against djpeg -scale, which
    /// make.sh writes as `<name>.<n>.ppm`.
    #[test]
    fn scaled_decodes_like_libjpeg() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jpeg");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jpg") { continue; }
            let jpg = std::fs::read(&p).unwrap();
            for scale in [2u32, 4, 8] {
                let Ok(ppm) = std::fs::read(p.with_extension(alloc::format!("{}.ppm", scale))) else { continue };
                let mut d = Decoder::new();
                let mut out = Vec::new();
                let (w, h) = d.decode_scaled(&jpg, scale, &mut out).unwrap();
                // A grey picture comes back from djpeg as PGM, one channel.
                let ch = if ppm.starts_with(b"P5") { 1 } else { 3 };
                let px = &ppm[ppm.len() - (w * h) as usize * ch..];
                let mut se = 0f64;
                for i in 0..(w * h) as usize {
                    for c in 0..3 {
                        let a = out[i * 4 + 2 - c] as f64;
                        let b = px[i * ch + c.min(ch - 1)] as f64;
                        se += (a - b) * (a - b);
                    }
                }
                let psnr = 10.0 * (255.0f64 * 255.0 / (se / (w * h * 3) as f64).max(1e-9)).log10();
                std::println!("{} 1/{}: {}x{} psnr {:.1} dB", p.file_name().unwrap().to_string_lossy(), scale, w, h, psnr);
                assert!(psnr > 30.0, "{} 1/{}: psnr {:.1}", p.display(), scale, psnr);
            }
        }
    }

    /// Files from `tests/jpeg/` (made by tests/jpeg/make.sh) against the
    /// PPM libjpeg-turbo decodes them to; replicated chroma against its
    /// fancy upsampling stays above 30 dB on smooth pictures.
    #[test]
    fn decodes_like_libjpeg() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/jpeg");
        let Ok(entries) = std::fs::read_dir(&dir) else { return };
        let mut n = 0;
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("jpg") { continue; }
            let jpg = std::fs::read(&p).unwrap();
            let ppm = std::fs::read(p.with_extension("ppm")).unwrap();
            let mut d = Decoder::new();
            let mut out = Vec::new();
            let (w, h) = d.decode(&jpg, &mut out).unwrap_or_else(|e| panic!("{}: {:?}", p.display(), e));
            let px = &ppm[ppm.len() - (w * h * 3) as usize..];
            let mut se = 0f64;
            for i in 0..(w * h) as usize {
                for c in 0..3 {
                    let a = out[i * 4 + 2 - c] as f64;
                    let b = px[i * 3 + c] as f64;
                    se += (a - b) * (a - b);
                }
            }
            let mse = se / (w * h * 3) as f64;
            let psnr = 10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10();
            std::println!("{}: {}x{} psnr {:.1} dB", p.file_name().unwrap().to_string_lossy(), w, h, psnr);
            assert!(psnr > 30.0, "{}: psnr {:.1}", p.display(), psnr);
            n += 1;
        }
        assert!(n > 0, "no test pictures in {}", dir.display());
    }
}
