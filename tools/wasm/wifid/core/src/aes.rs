//! AES-128 (FIPS-197) + AES Key Unwrap (RFC 3394).
//!
//! WPA2 msg3 carries the GTK in `key_data`, AES-key-wrapped with the KEK (the
//! second 16 bytes of the PTK). We only need encrypt (the unwrap KDF runs the
//! cipher in the *decrypt* direction, so we also need the inverse cipher).
//! Small table-free implementation — correctness over speed (a handshake runs a
//! handful of blocks).

// ── S-box / inverse S-box (FIPS-197) ─────────────────────────────────────
const SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

fn inv_sbox(b: u8) -> u8 {
    // Derived from SBOX (avoids a second 256-byte table).
    SBOX.iter().position(|&x| x == b).unwrap() as u8
}

fn xtime(a: u8) -> u8 {
    let h = (a >> 7) & 1;
    (a << 1) ^ (h * 0x1b)
}

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if b & 1 != 0 {
            p ^= a;
        }
        let hi = a & 0x80;
        a <<= 1;
        if hi != 0 {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

/// Expanded AES-128 key schedule (11 round keys × 16 bytes).
pub struct Aes128 {
    rk: [u8; 176],
}

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Self {
        let mut rk = [0u8; 176];
        rk[..16].copy_from_slice(key);
        let rcon: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];
        let mut i = 16;
        let mut r = 0;
        while i < 176 {
            let mut t = [rk[i - 4], rk[i - 3], rk[i - 2], rk[i - 1]];
            if i % 16 == 0 {
                t.rotate_left(1);
                for x in &mut t {
                    *x = SBOX[*x as usize];
                }
                t[0] ^= rcon[r];
                r += 1;
            }
            for j in 0..4 {
                rk[i + j] = rk[i - 16 + j] ^ t[j];
            }
            i += 4;
        }
        Aes128 { rk }
    }

    pub fn encrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let mut s = *block;
        add_round_key(&mut s, &self.rk[0..16]);
        for round in 1..10 {
            sub_bytes(&mut s);
            shift_rows(&mut s);
            mix_columns(&mut s);
            add_round_key(&mut s, &self.rk[round * 16..round * 16 + 16]);
        }
        sub_bytes(&mut s);
        shift_rows(&mut s);
        add_round_key(&mut s, &self.rk[160..176]);
        s
    }

    pub fn decrypt_block(&self, block: &[u8; 16]) -> [u8; 16] {
        let mut s = *block;
        add_round_key(&mut s, &self.rk[160..176]);
        for round in (1..10).rev() {
            inv_shift_rows(&mut s);
            inv_sub_bytes(&mut s);
            add_round_key(&mut s, &self.rk[round * 16..round * 16 + 16]);
            inv_mix_columns(&mut s);
        }
        inv_shift_rows(&mut s);
        inv_sub_bytes(&mut s);
        add_round_key(&mut s, &self.rk[0..16]);
        s
    }
}

fn add_round_key(s: &mut [u8; 16], rk: &[u8]) {
    for i in 0..16 {
        s[i] ^= rk[i];
    }
}
fn sub_bytes(s: &mut [u8; 16]) {
    for b in s.iter_mut() {
        *b = SBOX[*b as usize];
    }
}
fn inv_sub_bytes(s: &mut [u8; 16]) {
    for b in s.iter_mut() {
        *b = inv_sbox(*b);
    }
}
// State is column-major (AES standard): s[r + 4c].
fn shift_rows(s: &mut [u8; 16]) {
    let t = *s;
    for r in 1..4 {
        for c in 0..4 {
            s[r + 4 * c] = t[r + 4 * ((c + r) % 4)];
        }
    }
}
fn inv_shift_rows(s: &mut [u8; 16]) {
    let t = *s;
    for r in 1..4 {
        for c in 0..4 {
            s[r + 4 * c] = t[r + 4 * ((c + 4 - r) % 4)];
        }
    }
}
fn mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
        s[4 * c] = xtime(col[0]) ^ (xtime(col[1]) ^ col[1]) ^ col[2] ^ col[3];
        s[4 * c + 1] = col[0] ^ xtime(col[1]) ^ (xtime(col[2]) ^ col[2]) ^ col[3];
        s[4 * c + 2] = col[0] ^ col[1] ^ xtime(col[2]) ^ (xtime(col[3]) ^ col[3]);
        s[4 * c + 3] = (xtime(col[0]) ^ col[0]) ^ col[1] ^ col[2] ^ xtime(col[3]);
    }
}
fn inv_mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let col = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
        s[4 * c] = gmul(col[0], 14) ^ gmul(col[1], 11) ^ gmul(col[2], 13) ^ gmul(col[3], 9);
        s[4 * c + 1] = gmul(col[0], 9) ^ gmul(col[1], 14) ^ gmul(col[2], 11) ^ gmul(col[3], 13);
        s[4 * c + 2] = gmul(col[0], 13) ^ gmul(col[1], 9) ^ gmul(col[2], 14) ^ gmul(col[3], 11);
        s[4 * c + 3] = gmul(col[0], 11) ^ gmul(col[1], 13) ^ gmul(col[2], 9) ^ gmul(col[3], 14);
    }
}

// ── AES Key Wrap (RFC 3394) — inverse of unwrap ───────────────────────────
// Wraps `key` (n×8 bytes, n≥2) into `out` ((n+1)×8 bytes). Only used by the
// dev-harness to build a synthetic msg3 for the self-consistency test; the
// supplicant itself only ever unwraps.
pub fn aes_wrap(kek: &[u8; 16], key: &[u8], out: &mut [u8]) -> bool {
    if key.len() < 16 || key.len() % 8 != 0 || out.len() < key.len() + 8 {
        return false;
    }
    let n = key.len() / 8;
    let aes = Aes128::new(kek);
    let mut a = [0xa6u8; 8];
    let mut r = [0u8; 8 * 64];
    r[..n * 8].copy_from_slice(key);
    for j in 0..6 {
        for i in 1..=n {
            let mut blk = [0u8; 16];
            blk[..8].copy_from_slice(&a);
            blk[8..16].copy_from_slice(&r[(i - 1) * 8..(i - 1) * 8 + 8]);
            let b = aes.encrypt_block(&blk);
            a.copy_from_slice(&b[..8]);
            let t = (n * j + i) as u64;
            for k in 0..8 {
                a[k] ^= ((t >> (8 * (7 - k))) & 0xff) as u8;
            }
            r[(i - 1) * 8..(i - 1) * 8 + 8].copy_from_slice(&b[8..16]);
        }
    }
    out[..8].copy_from_slice(&a);
    out[8..(n + 1) * 8].copy_from_slice(&r[..n * 8]);
    true
}

// ── AES Key Unwrap (RFC 3394) ─────────────────────────────────────────────
// Unwrap `n` 64-bit blocks (ciphertext is (n+1)*8 bytes). Returns the unwrapped
// key (n*8 bytes) into `out`, true if the integrity check (A == 0xA6A6A6A6A6A6A6A6)
// passes. Used to decrypt msg3's key_data with the KEK.
pub fn aes_unwrap(kek: &[u8; 16], wrapped: &[u8], out: &mut [u8]) -> bool {
    if wrapped.len() < 16 || wrapped.len() % 8 != 0 {
        return false;
    }
    let n = wrapped.len() / 8 - 1;
    if out.len() < n * 8 {
        return false;
    }
    let aes = Aes128::new(kek);
    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[..8]);
    let mut r = [0u8; 8 * 64]; // up to 64 blocks
    for i in 0..n {
        r[i * 8..i * 8 + 8].copy_from_slice(&wrapped[(i + 1) * 8..(i + 2) * 8]);
    }
    for j in (0..6).rev() {
        for i in (1..=n).rev() {
            // B = AES-decrypt( (A ^ t) || R[i] ), t = n*j + i
            let t = (n * j + i) as u64;
            let mut blk = [0u8; 16];
            blk[..8].copy_from_slice(&a);
            for k in 0..8 {
                blk[k] ^= ((t >> (8 * (7 - k))) & 0xff) as u8;
            }
            blk[8..16].copy_from_slice(&r[(i - 1) * 8..(i - 1) * 8 + 8]);
            let b = aes.decrypt_block(&blk);
            a.copy_from_slice(&b[..8]);
            r[(i - 1) * 8..(i - 1) * 8 + 8].copy_from_slice(&b[8..16]);
        }
    }
    out[..n * 8].copy_from_slice(&r[..n * 8]);
    a == [0xa6; 8]
}
