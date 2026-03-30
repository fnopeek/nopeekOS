//! Cryptographic primitives for nopeekOS
//!
//! - ChaCha20 stream cipher (RFC 7539)
//! - Poly1305 MAC (RFC 7539)
//! - ChaCha20-Poly1305 AEAD (RFC 8439)
//! - BLAKE3-based KDF for key derivation
//!
//! All implementations target no_std bare metal.

use alloc::vec::Vec;
use spin::Mutex;

// ============================================================
// Global Master Key (set after passphrase auth)
// ============================================================

static MASTER_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

pub fn set_master_key(key: [u8; 32]) {
    *MASTER_KEY.lock() = Some(key);
}

pub fn get_master_key() -> Option<[u8; 32]> {
    *MASTER_KEY.lock()
}

pub fn clear_master_key() {
    *MASTER_KEY.lock() = None;
}

// ============================================================
// ChaCha20 (RFC 7539)
// ============================================================

fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]); s[d] ^= s[a]; s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]); s[b] ^= s[c]; s[b] = s[b].rotate_left(7);
}

pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut s = [0u32; 16];
    s[0] = 0x61707865; s[1] = 0x3320646e;
    s[2] = 0x79622d32; s[3] = 0x6b206574;
    for i in 0..8 {
        let o = i * 4;
        s[4 + i] = u32::from_le_bytes([key[o], key[o + 1], key[o + 2], key[o + 3]]);
    }
    s[12] = counter;
    for i in 0..3 {
        let o = i * 4;
        s[13 + i] = u32::from_le_bytes([nonce[o], nonce[o + 1], nonce[o + 2], nonce[o + 3]]);
    }
    let initial = s;
    for _ in 0..10 {
        quarter_round(&mut s, 0, 4,  8, 12);
        quarter_round(&mut s, 1, 5,  9, 13);
        quarter_round(&mut s, 2, 6, 10, 14);
        quarter_round(&mut s, 3, 7, 11, 15);
        quarter_round(&mut s, 0, 5, 10, 15);
        quarter_round(&mut s, 1, 6, 11, 12);
        quarter_round(&mut s, 2, 7,  8, 13);
        quarter_round(&mut s, 3, 4,  9, 14);
    }
    let mut out = [0u8; 64];
    for i in 0..16 {
        let val = s[i].wrapping_add(initial[i]);
        out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
    }
    out
}

/// XOR data with ChaCha20 keystream. Works for both encrypt and decrypt.
fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {
    let mut ctr = counter;
    let mut offset = 0;
    while offset < data.len() {
        let block = chacha20_block(key, ctr, nonce);
        let take = (data.len() - offset).min(64);
        for i in 0..take {
            data[offset + i] ^= block[i];
        }
        offset += take;
        ctr = ctr.wrapping_add(1);
    }
}

// ============================================================
// Poly1305 MAC (RFC 7539) — 5-limb, 26-bit, reference algorithm
// ============================================================

struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    pad: [u32; 4],
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        let r0 = u32::from_le_bytes([key[0],  key[1],  key[2],  key[3]])  & 0x3ffffff;
        let r1 = (u32::from_le_bytes([key[3],  key[4],  key[5],  key[6]])  >> 2) & 0x3ffff03;
        let r2 = (u32::from_le_bytes([key[6],  key[7],  key[8],  key[9]])  >> 4) & 0x3ffc0ff;
        let r3 = (u32::from_le_bytes([key[9],  key[10], key[11], key[12]]) >> 6) & 0x3f03fff;
        let r4 = (u32::from_le_bytes([key[12], key[13], key[14], key[15]]) >> 8) & 0x00fffff;

        let pad = [
            u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
            u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
            u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
            u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
        ];

        Poly1305 {
            r: [r0, r1, r2, r3, r4],
            h: [0; 5],
            pad,
        }
    }

    fn block(&mut self, msg: &[u8], hibit: u32) {
        let r0 = self.r[0] as u64;
        let r1 = self.r[1] as u64;
        let r2 = self.r[2] as u64;
        let r3 = self.r[3] as u64;
        let r4 = self.r[4] as u64;

        let s1 = r1 * 5;
        let s2 = r2 * 5;
        let s3 = r3 * 5;
        let s4 = r4 * 5;

        let mut h0 = self.h[0] as u64;
        let mut h1 = self.h[1] as u64;
        let mut h2 = self.h[2] as u64;
        let mut h3 = self.h[3] as u64;
        let mut h4 = self.h[4] as u64;

        // Add message block
        h0 += (u32::from_le_bytes([msg[0],  msg[1],  msg[2],  msg[3]])       ) as u64 & 0x3ffffff;
        h1 += (u32::from_le_bytes([msg[3],  msg[4],  msg[5],  msg[6]])  >> 2 ) as u64 & 0x3ffffff;
        h2 += (u32::from_le_bytes([msg[6],  msg[7],  msg[8],  msg[9]])  >> 4 ) as u64 & 0x3ffffff;
        h3 += (u32::from_le_bytes([msg[9],  msg[10], msg[11], msg[12]]) >> 6 ) as u64 & 0x3ffffff;
        h4 += (u32::from_le_bytes([msg[12], msg[13], msg[14], msg[15]]) >> 8 ) as u64 | (hibit as u64);

        // h *= r (mod 2^130 - 5)
        let d0 = h0*r0 + h1*s4 + h2*s3 + h3*s2 + h4*s1;
        let d1 = h0*r1 + h1*r0 + h2*s4 + h3*s3 + h4*s2;
        let d2 = h0*r2 + h1*r1 + h2*r0 + h3*s4 + h4*s3;
        let d3 = h0*r3 + h1*r2 + h2*r1 + h3*r0 + h4*s4;
        let d4 = h0*r4 + h1*r3 + h2*r2 + h3*r1 + h4*r0;

        // Carry propagation
        let mut c: u64;
        c = d0 >> 26; h0 = d0 & 0x3ffffff; h1 = d1 + c;
        c = h1 >> 26; h1 &= 0x3ffffff;     h2 = d2 + c;
        c = h2 >> 26; h2 &= 0x3ffffff;     h3 = d3 + c;
        c = h3 >> 26; h3 &= 0x3ffffff;     h4 = d4 + c;
        c = h4 >> 26; h4 &= 0x3ffffff;     h0 += c * 5;
        c = h0 >> 26; h0 &= 0x3ffffff;     h1 += c;

        self.h = [h0 as u32, h1 as u32, h2 as u32, h3 as u32, h4 as u32];
    }

    fn finish(self) -> [u8; 16] {
        // Final carry
        let mut h0 = self.h[0] as u64;
        let mut h1 = self.h[1] as u64;
        let mut h2 = self.h[2] as u64;
        let mut h3 = self.h[3] as u64;
        let mut h4 = self.h[4] as u64;

        let mut c: u64;
        c = h1 >> 26; h1 &= 0x3ffffff; h2 += c;
        c = h2 >> 26; h2 &= 0x3ffffff; h3 += c;
        c = h3 >> 26; h3 &= 0x3ffffff; h4 += c;
        c = h4 >> 26; h4 &= 0x3ffffff; h0 += c * 5;
        c = h0 >> 26; h0 &= 0x3ffffff; h1 += c;

        // Compute h + -(2^130-5) = h - p
        let mut g0 = h0.wrapping_add(5); c = g0 >> 26; g0 &= 0x3ffffff;
        let mut g1 = h1.wrapping_add(c); c = g1 >> 26; g1 &= 0x3ffffff;
        let mut g2 = h2.wrapping_add(c); c = g2 >> 26; g2 &= 0x3ffffff;
        let mut g3 = h3.wrapping_add(c); c = g3 >> 26; g3 &= 0x3ffffff;
        let g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);

        // Select h or g based on overflow
        let mask = (g4 >> 63).wrapping_sub(1); // all 1s if g4 >= 0
        let nmask = !mask;
        h0 = (h0 & nmask) | (g0 & mask);
        h1 = (h1 & nmask) | (g1 & mask);
        h2 = (h2 & nmask) | (g2 & mask);
        h3 = (h3 & nmask) | (g3 & mask);
        h4 = (h4 & nmask) | (g4 & mask);

        // h = h mod 2^128 + pad
        let mut f: u64;
        f = (h0 | (h1 << 26)) + self.pad[0] as u64;
        let b0 = f as u32;
        f = ((h1 >> 6) | (h2 << 20)) + self.pad[1] as u64 + (f >> 32);
        let b1 = f as u32;
        f = ((h2 >> 12) | (h3 << 14)) + self.pad[2] as u64 + (f >> 32);
        let b2 = f as u32;
        f = ((h3 >> 18) | (h4 << 8)) + self.pad[3] as u64 + (f >> 32);
        let b3 = f as u32;

        let mut tag = [0u8; 16];
        tag[0..4].copy_from_slice(&b0.to_le_bytes());
        tag[4..8].copy_from_slice(&b1.to_le_bytes());
        tag[8..12].copy_from_slice(&b2.to_le_bytes());
        tag[12..16].copy_from_slice(&b3.to_le_bytes());
        tag
    }
}

fn poly1305_mac(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    let mut poly = Poly1305::new(key);
    let mut i = 0;
    while i + 16 <= message.len() {
        poly.block(&message[i..i + 16], 1 << 24);
        i += 16;
    }
    if i < message.len() {
        let mut last = [0u8; 16];
        let remaining = message.len() - i;
        last[..remaining].copy_from_slice(&message[i..]);
        last[remaining] = 0x01;
        poly.block(&last, 0);
    }
    poly.finish()
}

// ============================================================
// ChaCha20-Poly1305 AEAD (RFC 8439)
// ============================================================

pub const TAG_SIZE: usize = 16;

/// Encrypt and authenticate. Returns ciphertext || 16-byte tag.
pub fn aead_encrypt(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    // Poly1305 key from ChaCha20 block 0
    let poly_block = chacha20_block(key, 0, nonce);
    let mut poly_key = [0u8; 32];
    poly_key.copy_from_slice(&poly_block[..32]);

    // Encrypt with ChaCha20 starting at counter 1
    let mut ciphertext = plaintext.to_vec();
    chacha20_xor(key, nonce, 1, &mut ciphertext);

    // MAC over: pad(AAD) || pad(ciphertext) || len(AAD) || len(CT)
    let mac_input = build_mac_input(&[], &ciphertext);
    let tag = poly1305_mac(&poly_key, &mac_input);

    ciphertext.extend_from_slice(&tag);
    ciphertext
}

/// Decrypt and verify. Returns plaintext or None if authentication fails.
pub fn aead_decrypt(key: &[u8; 32], nonce: &[u8; 12], ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> {
    if ciphertext_and_tag.len() < TAG_SIZE {
        return None;
    }

    let ct_len = ciphertext_and_tag.len() - TAG_SIZE;
    let ciphertext = &ciphertext_and_tag[..ct_len];
    let tag = &ciphertext_and_tag[ct_len..];

    let poly_block = chacha20_block(key, 0, nonce);
    let mut poly_key = [0u8; 32];
    poly_key.copy_from_slice(&poly_block[..32]);

    let mac_input = build_mac_input(&[], ciphertext);
    let expected_tag = poly1305_mac(&poly_key, &mac_input);

    // Constant-time comparison
    let mut diff = 0u8;
    for i in 0..TAG_SIZE {
        diff |= tag[i] ^ expected_tag[i];
    }
    if diff != 0 {
        return None;
    }

    let mut plaintext = ciphertext.to_vec();
    chacha20_xor(key, nonce, 1, &mut plaintext);
    Some(plaintext)
}

fn build_mac_input(aad: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let mut input = Vec::new();
    input.extend_from_slice(aad);
    let pad_aad = (16 - (aad.len() % 16)) % 16;
    input.resize(input.len() + pad_aad, 0);
    input.extend_from_slice(ciphertext);
    let pad_ct = (16 - (ciphertext.len() % 16)) % 16;
    input.resize(input.len() + pad_ct, 0);
    input.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    input.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    input
}

// ============================================================
// Key Derivation (BLAKE3-based)
// ============================================================

/// Derive a 256-bit master key from a passphrase and salt.
pub fn derive_master_key(passphrase: &[u8], salt: &[u8]) -> [u8; 32] {
    let mut input = Vec::new();
    input.extend_from_slice(b"nopeekOS.master-key.v1");
    input.extend_from_slice(salt);
    input.extend_from_slice(passphrase);
    *blake3::hash(&input).as_bytes()
}

/// Derive a per-object encryption key from master key and object content hash.
pub fn derive_object_key(master_key: &[u8; 32], content_hash: &[u8; 32]) -> [u8; 32] {
    let mut input = [0u8; 64];
    input[..32].copy_from_slice(master_key);
    input[32..].copy_from_slice(content_hash);
    *blake3::hash(&input).as_bytes()
}

/// Derive a nonce from the content hash (first 12 bytes).
pub fn derive_nonce(content_hash: &[u8; 32]) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&content_hash[..12]);
    nonce
}
