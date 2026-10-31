//! The workloads, shared by the wasm module and the native build.
//!
//! Each one stresses a different kind of code a compiler has to get right:
//! calls, integer ALU work, table loads, branchy compares, byte-wise memory
//! traffic, float loops, an interpreter's dispatch, allocation and string
//! handling. Inputs come from a fixed generator and every workload returns a
//! checksum, so the native and the forge run can be compared for both time
//! and result.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt::Write;

pub struct Work {
    pub name: &'static str,
    pub run: fn() -> u64,
}

pub const ALL: &[Work] = &[
    Work { name: "calls", run: calls },
    Work { name: "sha256", run: sha256 },
    Work { name: "crc32", run: crc32 },
    Work { name: "sort", run: sort },
    Work { name: "lz", run: lz },
    Work { name: "matmul", run: matmul },
    Work { name: "nbody", run: nbody },
    Work { name: "vm", run: vm },
    Work { name: "btree", run: btree },
    Work { name: "strings", run: strings },
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

fn fib(n: u32) -> u64 {
    if n < 2 { n as u64 } else { fib(n - 1) + fib(n - 2) }
}

#[inline(never)]
fn calls() -> u64 {
    fib(core::hint::black_box(37))
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

fn sha256_blocks(state: &mut [u32; 8], data: &[u8]) {
    for block in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([block[4 * i], block[4 * i + 1], block[4 * i + 2], block[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

fn bytes(n: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng(seed);
    (0..n).map(|_| r.next() as u8).collect()
}

#[inline(never)]
fn sha256() -> u64 {
    let data = bytes(16 << 20, 1);
    let mut st = [
        0x6a09e667u32, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];
    sha256_blocks(&mut st, &data);
    ((st[0] as u64) << 32) | st[1] as u64
}

#[inline(never)]
fn crc32() -> u64 {
    let mut table = [0u32; 256];
    for i in 0..256u32 {
        let mut c = i;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        table[i as usize] = c;
    }
    let data = bytes(16 << 20, 2);
    let mut crc = !0u32;
    for &b in &data {
        crc = table[((crc ^ b as u32) & 0xff) as usize] ^ (crc >> 8);
    }
    !crc as u64
}

#[inline(never)]
fn sort() -> u64 {
    let mut r = Rng(3);
    let mut v: Vec<u32> = (0..2_000_000).map(|_| r.next() as u32).collect();
    v.sort_unstable();
    let mut s = 0u64;
    for (i, x) in v.iter().enumerate().step_by(997) {
        s = s.wrapping_mul(31).wrapping_add(*x as u64 ^ i as u64);
    }
    s
}

/// Text-like input: words from a small vocabulary, so it compresses.
fn text(n: usize) -> Vec<u8> {
    const WORDS: [&str; 16] = [
        "the ", "kernel ", "module ", "capability ", "intent ", "forge ", "memory ", "a ",
        "content ", "address ", "sandbox ", "of ", "and ", "runs ", "code ", "\n",
    ];
    let mut r = Rng(4);
    let mut out = Vec::with_capacity(n + 16);
    while out.len() < n {
        let w = WORDS[(r.next() % 16) as usize];
        out.extend_from_slice(w.as_bytes());
        if r.next() % 7 == 0 {
            out.push(b'0' + (r.next() % 10) as u8);
        }
    }
    out.truncate(n);
    out
}

/// A greedy LZ77 with a hash of the next four bytes: literal runs and
/// (offset, length) matches, then decoded and checked.
fn lz_compress(src: &[u8]) -> Vec<u8> {
    const HBITS: u32 = 14;
    let mut table = vec![0u32; 1 << HBITS];
    let mut out = Vec::with_capacity(src.len() / 2);
    let mut lit_start = 0usize;
    let mut i = 0usize;
    let flush = |out: &mut Vec<u8>, lits: &[u8]| {
        for chunk in lits.chunks(127) {
            out.push(chunk.len() as u8);
            out.extend_from_slice(chunk);
        }
    };
    while i + 4 <= src.len() {
        let key = u32::from_le_bytes([src[i], src[i + 1], src[i + 2], src[i + 3]]);
        let h = (key.wrapping_mul(2_654_435_761) >> (32 - HBITS)) as usize;
        let cand = table[h] as usize;
        table[h] = i as u32;
        if cand < i && i - cand < 65536 && src[cand..cand + 4] == src[i..i + 4] {
            let mut len = 4;
            while i + len < src.len() && len < 130 && src[cand + len] == src[i + len] {
                len += 1;
            }
            flush(&mut out, &src[lit_start..i]);
            let off = i - cand;
            out.push(0x80 | (len - 3) as u8);
            out.push(off as u8);
            out.push((off >> 8) as u8);
            i += len;
            lit_start = i;
        } else {
            i += 1;
        }
    }
    flush(&mut out, &src[lit_start..]);
    out
}

fn lz_decompress(src: &[u8], n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while i < src.len() {
        let t = src[i];
        i += 1;
        if t & 0x80 == 0 {
            out.extend_from_slice(&src[i..i + t as usize]);
            i += t as usize;
        } else {
            let len = (t & 0x7f) as usize + 3;
            let off = src[i] as usize | (src[i + 1] as usize) << 8;
            i += 2;
            let start = out.len() - off;
            for k in 0..len {
                let b = out[start + k];
                out.push(b);
            }
        }
    }
    out
}

#[inline(never)]
fn lz() -> u64 {
    let src = text(8 << 20);
    let c = lz_compress(&src);
    let d = lz_decompress(&c, src.len());
    if d != src {
        return 0;
    }
    c.len() as u64
}

#[inline(never)]
fn matmul() -> u64 {
    const N: usize = 512;
    let mut r = Rng(5);
    let a: Vec<f64> = (0..N * N).map(|_| (r.next() % 1000) as f64 / 100.0).collect();
    let b: Vec<f64> = (0..N * N).map(|_| (r.next() % 1000) as f64 / 100.0).collect();
    let mut c = vec![0.0f64; N * N];
    for i in 0..N {
        for k in 0..N {
            let aik = a[i * N + k];
            for j in 0..N {
                c[i * N + j] += aik * b[k * N + j];
            }
        }
    }
    let s: f64 = c.iter().step_by(31).sum();
    s as u64
}

#[inline(never)]
fn nbody() -> u64 {
    const N: usize = 5;
    let mut pos = [[0.0f64, 0.0, 0.0], [4.84, -1.16, -0.10], [8.34, 4.12, -0.40], [12.89, -15.11, -0.22], [15.37, -25.91, 0.17]];
    let mut vel = [[0.0f64, 0.0, 0.0], [0.606, 2.81, -0.02], [-1.01, 1.82, 0.008], [1.08, 0.868, -0.01], [0.979, 0.594, -0.034]];
    let mass = [39.47, 0.037, 0.011, 0.0017, 0.002];
    let dt = 0.01;
    for _ in 0..1_200_000 {
        for i in 0..N {
            for j in i + 1..N {
                let d = [pos[i][0] - pos[j][0], pos[i][1] - pos[j][1], pos[i][2] - pos[j][2]];
                let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
                let mag = dt / (d2 * sqrt(d2));
                for k in 0..3 {
                    vel[i][k] -= d[k] * mass[j] * mag;
                    vel[j][k] += d[k] * mass[i] * mag;
                }
            }
        }
        for i in 0..N {
            for k in 0..3 {
                pos[i][k] += dt * vel[i][k];
            }
        }
    }
    (pos[1][0] * 1e9) as i64 as u64
}

/// `f64::sqrt` lives in std; in `no_std` this is the instruction itself.
#[inline(always)]
fn sqrt(x: f64) -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        core::arch::wasm32::f64_sqrt(x)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // SAFETY: SSE2 is part of x86-64.
        unsafe {
            use core::arch::x86_64::*;
            _mm_cvtsd_f64(_mm_sqrt_sd(_mm_setzero_pd(), _mm_set_sd(x)))
        }
    }
}

#[derive(Clone, Copy)]
enum Op {
    Push(i64),
    Load(u8),
    Store(u8),
    Add,
    Mul,
    Rem,
    Lt,
    JmpIfNot(u16),
    Jmp(u16),
    Halt,
}

/// A stack machine counting the primes below a bound by trial division:
/// a dispatch loop over a `match`, as an interpreter is built.
#[inline(never)]
fn vm() -> u64 {
    use Op::*;
    // vars: 0 n, 1 count, 2 d, 3 is_prime, 4 limit
    let prog = [
        Push(2), Store(0), Push(0), Store(1), Push(150_000), Store(4),
        // 6: while n < limit
        Load(0), Load(4), Lt, JmpIfNot(42),
        Push(1), Store(3), Push(2), Store(2),
        // 14: while d * d < n + 1
        Load(2), Load(2), Mul, Load(0), Push(1), Add, Lt, JmpIfNot(33),
        // 22: if n % d == 0, composite
        Load(0), Load(2), Rem, JmpIfNot(31),
        Load(2), Push(1), Add, Store(2), Jmp(14),
        // 31
        Push(0), Store(3),
        // 33: count += is_prime; n += 1
        Load(1), Load(3), Add, Store(1),
        Load(0), Push(1), Add, Store(0), Jmp(6),
        // 42
        Halt,
    ];
    let mut vars = [0i64; 8];
    let mut stack = [0i64; 16];
    let mut sp = 0usize;
    let mut pc = 0usize;
    let mut steps = 0u64;
    loop {
        steps += 1;
        match prog[pc] {
            Push(v) => { stack[sp] = v; sp += 1; }
            Load(i) => { stack[sp] = vars[i as usize]; sp += 1; }
            Store(i) => { sp -= 1; vars[i as usize] = stack[sp]; }
            Add => { sp -= 1; stack[sp - 1] = stack[sp - 1].wrapping_add(stack[sp]); }
            Mul => { sp -= 1; stack[sp - 1] = stack[sp - 1].wrapping_mul(stack[sp]); }
            Rem => { sp -= 1; stack[sp - 1] %= stack[sp]; }
            Lt => { sp -= 1; stack[sp - 1] = (stack[sp - 1] < stack[sp]) as i64; }
            JmpIfNot(t) => {
                sp -= 1;
                if stack[sp] == 0 { pc = t as usize; continue; }
            }
            Jmp(t) => { pc = t as usize; continue; }
            Halt => break,
        }
        pc += 1;
    }
    (vars[1] as u64) << 32 | (steps & 0xffff_ffff)
}

#[inline(never)]
fn btree() -> u64 {
    let src = text(1 << 20);
    let mut counts: BTreeMap<&[u8], u32> = BTreeMap::new();
    for w in src.split(|&b| b == b' ' || b == b'\n') {
        if !w.is_empty() {
            *counts.entry(w).or_insert(0) += 1;
        }
    }
    let mut r = Rng(6);
    let mut m: BTreeMap<u64, u64> = BTreeMap::new();
    for i in 0..300_000u64 {
        m.insert(r.next() % 100_000, i);
        if i % 3 == 0 {
            m.remove(&(r.next() % 100_000));
        }
    }
    let mut s = m.len() as u64;
    for (k, v) in counts {
        s = s.wrapping_mul(131).wrapping_add(k.len() as u64 * v as u64);
    }
    s
}

#[inline(never)]
fn strings() -> u64 {
    let mut s = String::new();
    let mut total = 0u64;
    let mut r = Rng(7);
    for i in 0..600_000u32 {
        s.clear();
        let _ = write!(s, "{{\"id\":{},\"v\":{},\"name\":\"item{}\"}}", i, r.next() % 100_000, i % 977);
        let mut num = 0u64;
        for part in s.split(',') {
            if let Some(p) = part.find(':') {
                let digits = part[p + 1..].trim_matches(|c| c == '}' || c == '"');
                num = num.wrapping_add(digits.bytes().filter(u8::is_ascii_digit).fold(0u64, |a, b| a * 10 + (b - b'0') as u64));
            }
        }
        total = total.wrapping_add(num ^ s.len() as u64);
    }
    total
}
